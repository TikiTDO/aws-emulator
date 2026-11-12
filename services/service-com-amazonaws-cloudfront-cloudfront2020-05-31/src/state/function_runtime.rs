// SPDX-License-Identifier: CC0-1.0

//! RUNNING a CloudFront function: the engine the stored function code executes in, inside
//! this process.
//!
//! "CloudFront runs the function, passing it the event object that you provided, and returns
//! the function's result (the modified event object) in the response. The response also
//! contains function logs and error messages, if any exist." Every one of those three is
//! whatever the caller's OWN code produced, so none of them can be assembled here: the code
//! is compiled and executed by the QuickJS engine embedded in this process (`rquickjs`), the
//! function's global `handler` is CALLED over the event object, and the value it returned,
//! the lines it logged and the failure it raised are read back off that run.
//!
//! # What the code can reach
//!
//! A FRESH engine per run, so nothing of one run is reachable from the next: no global and
//! no value outlives the call that made it. Inside that engine the code reaches the language
//! and ONE host function — the log writer the console is built over, which only appends to
//! this run's own captured lines and is deleted from the global object before any of the
//! caller's code is compiled, so only the console closure still holds it. No module loader
//! is installed, so an `import` resolves to nothing; plain QuickJS registers no filesystem,
//! process, environment, network, host-library or thread API, and nothing here adds one.
//!
//! # What bounds it
//!
//! Every bound lives in host code the running function cannot reach:
//!
//! * a WALL CLOCK ([`RUN_WALL_CLOCK`]) and a STEP budget ([`RUN_ENGINE_CHECKS`]), both spent
//!   by the engine's own interrupt handler as it works, so code that never returns and code
//!   that loops without allocating are each stopped;
//! * a MEMORY ceiling ([`RUN_MEMORY_BYTES`]) and a JavaScript STACK ceiling
//!   ([`RUN_STACK_BYTES`]), both the engine's own, so an allocation or a recursion past them
//!   ends the run inside the engine rather than on the host's stack;
//! * ceilings on what one run may REPORT: how many log lines ([`MAX_LOG_ENTRIES`]), how long
//!   one line ([`MAX_LOG_ENTRY_BYTES`]) and how large the returned event object
//!   ([`MAX_OUTPUT_BYTES`]) may be;
//! * a bound on the JOBS a returned promise may settle through ([`MAX_PENDING_JOBS`]);
//! * a CONCURRENCY permit ([`MAX_CONCURRENT_RUNS`]) and a CALLER WAIT ceiling
//!   ([`CALLER_WAIT_CEILING`]) held outside the engine, so however many callers run
//!   functions at once, what the engine holds is bounded and each request is answered even
//!   where a stretch of engine-internal work outlasts the run's own clock.
//!
//! Which bound was breached is LATCHED in host state ([`Bounds`]) rather than read back out
//! of the engine, so code that catches the exception the engine raises in it cannot make the
//! run report a result: once the latch is set, the run's answer is the stop.
//!
//! The run happens on a BLOCKING thread ([`run_function`]), never on the async executor
//! serving the request, and no store lock is held across it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ::rquickjs::function::Func;
use ::rquickjs::{CatchResultExt, Context, Ctx, Function, Runtime, Value};
use ::serde_json::Value as Json;

/// The global a CloudFront function's code declares and CloudFront calls: "the function code
/// ... declares a function named `handler`", which is handed the event object and whose
/// return value is the result.
pub const HANDLER_NAME: &str = "handler";

/// The member that marks the object a function returned as a RESPONSE rather than a request:
/// a response object "must contain `statusCode`", while a request object has no such member.
/// It is what decides which member of the returned event object the result is reported under.
pub const RESPONSE_STATUS_MEMBER: &str = "statusCode";

/// The member the returned event object is reported under when the function returned a
/// REQUEST — the shape `FunctionOutput` carries ("the event object returned by the
/// function"), which holds the request or the response the function came back with.
pub const OUTPUT_REQUEST_MEMBER: &str = "request";

/// The member the returned event object is reported under when the function returned a
/// RESPONSE, the other half of the pair [`OUTPUT_REQUEST_MEMBER`] documents.
pub const OUTPUT_RESPONSE_MEMBER: &str = "response";

/// The maximum time CloudFront allows one function to run in, which `ComputeUtilization` is
/// a percentage OF: "The amount of time that the function took to run as a percentage of the
/// maximum allowed time." CloudFront's published quota for the execution time of a
/// CloudFront function is 1 millisecond, so that is the denominator; the numerator is the
/// time this engine measured for the run ([`FunctionRun::compute_utilization`]).
///
/// It is NOT what stops a run — this emulator's engine is slower than CloudFront's edge, so
/// stopping at a millisecond would refuse code CloudFront runs. [`RUN_WALL_CLOCK`] is the
/// bound a run is actually held to.
pub const MAX_FUNCTION_EXECUTION_TIME: Duration = Duration::from_millis(1);

/// How long one run may take before the engine's interrupt handler stops it.
///
/// A bound of this EMULATOR's own, not a documented CloudFront one: the code runs in this
/// process, so a function with no exit would otherwise hold a thread for ever.
const RUN_WALL_CLOCK: Duration = Duration::from_secs(2);

/// How long the CALLER waits for a run at most, which is longer than the run's own clock by
/// the margin it takes the engine to notice that clock.
///
/// A ceiling of the HOST's, held outside the engine: the engine can only be stopped at a
/// check it makes, so a stretch of engine-internal work can outlast the run's own clock.
/// This is what answers the request anyway.
const CALLER_WAIT_CEILING: Duration = Duration::from_secs(5);

/// How many engine checks one run may spend — the STEP bound beside the wall-clock one, and
/// the one that does not depend on how fast the host is. The engine asks its interrupt
/// handler once per fixed number of bytecode operations, so a budget of asks is a budget of
/// work: code that diverges is stopped after the same amount of WORK on every machine.
const RUN_ENGINE_CHECKS: u64 = 100_000;

/// The heap one run's engine is built with.
const RUN_MEMORY_BYTES: usize = 32 * 1024 * 1024;

/// The JavaScript stack one run's engine is built with. Well inside the stack of the
/// blocking thread the run happens on, so deep recursion in a function ends that run with
/// the engine's own stack-overflow exception rather than touching the thread's guard page.
const RUN_STACK_BYTES: usize = 512 * 1024;

/// How large the returned event object may be, in bytes, before the run is reported as
/// having failed: the modelled maximum length of `FunctionEventObject` (`@length 0..40960`),
/// which is the largest event object this operation carries in either direction.
pub const MAX_OUTPUT_BYTES: usize = 40_960;

/// How many log lines one run may report, which is what `FunctionExecutionLogs` carries
/// ("the log lines that the function wrote (if any) when running the test").
pub const MAX_LOG_ENTRIES: usize = 100;

/// How long one reported log line may be, in bytes.
pub const MAX_LOG_ENTRY_BYTES: usize = 4_096;

/// How many engine jobs one run may settle a returned promise through, so an async function
/// that keeps queueing work cannot make the drain below unbounded.
const MAX_PENDING_JOBS: usize = 1_000;

/// How many functions this emulator runs AT ONCE.
///
/// An EMULATOR bound: each run holds the engine's own heap on a blocking thread, so this is
/// what bounds what those runs can hold and occupy however many callers arrive together. The
/// permit is held by the RUN rather than by the caller waiting on it
/// ([`run_function`]), so a run the caller gave up on keeps its slot until the engine thread
/// really ends: at most this many engines exist at any instant, whatever a caller does.
const MAX_CONCURRENT_RUNS: usize = 4;

/// How long a caller waits for one of those run slots before the run is reported as one that
/// did not happen, rather than queueing behind other callers' code.
const RUN_PERMIT_WAIT: Duration = Duration::from_secs(5);

/// The run slots [`MAX_CONCURRENT_RUNS`] declares, over every function of this process: the
/// bound is on what the ENGINE may hold at once, which is a property of the process rather
/// than of one function or one request.
///
/// Held as an `Arc` so a permit can be OWNED by the blocking run it bounds rather than by the
/// future awaiting it.
fn function_runs() -> &'static ::std::sync::Arc<::tokio::sync::Semaphore> {
    static RUNS: ::std::sync::OnceLock<::std::sync::Arc<::tokio::sync::Semaphore>> =
        ::std::sync::OnceLock::new();
    RUNS.get_or_init(|| {
        ::std::sync::Arc::new(::tokio::sync::Semaphore::new(MAX_CONCURRENT_RUNS))
    })
}

/// What one run of a function came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionOutcome {
    /// The function returned an event object: the JSON text of it, under the member its
    /// shape names ([`OUTPUT_REQUEST_MEMBER`] or [`OUTPUT_RESPONSE_MEMBER`]).
    Returned(String),
    /// The function FAILED, and this is the error message: the code did not compile, it
    /// declared no handler, it raised, it returned something that is not an event object, or
    /// a bound of this module stopped it.
    Failed(String),
    /// The run did not happen at all — the engine did not start, every run slot was taken,
    /// or the host gave up waiting for it. Nothing was learned about the code, which is a
    /// different answer from "the function failed".
    Unavailable(String),
}

/// Everything one run of a function produced.
#[derive(Debug, Clone)]
pub struct FunctionRun {
    /// What the run came to.
    pub outcome: FunctionOutcome,
    /// The lines the function wrote through the console, in the order it wrote them.
    pub logs: Vec<String>,
    /// How long the engine spent compiling and running the code, which is what the reported
    /// compute utilization is measured from.
    pub elapsed: Duration,
}

impl FunctionRun {
    /// `TestResult$ComputeUtilization` for this run: "the amount of time that the function
    /// took to run as a percentage of the maximum allowed time"
    /// ([`MAX_FUNCTION_EXECUTION_TIME`]), as the whole-number percentage the member carries.
    ///
    /// Rounded UP, so a run that took measurable time never reports none of the budget, and
    /// held to 100: a run that outlasted the allowed time used all of it.
    pub fn compute_utilization(&self) -> String {
        let allowed = MAX_FUNCTION_EXECUTION_TIME.as_micros().max(1);
        let spent = self.elapsed.as_micros();
        let percentage = spent.saturating_mul(100).div_ceil(allowed).min(100);
        percentage.to_string()
    }

    /// A run that did not happen, carrying `reason`.
    fn unavailable(reason: String) -> Self {
        Self {
            outcome: FunctionOutcome::Unavailable(reason),
            logs: Vec::new(),
            elapsed: Duration::ZERO,
        }
    }
}

/// Run `code` over the event object `event` (the JSON text of it), on a blocking thread.
///
/// `code` is the code of a STORED function, so its size is already held to the published
/// function-code quota by the create and the update that wrote it
/// ([`super::MAX_FUNCTION_CODE_BYTES`]); this module bounds what RUNNING it may spend.
///
/// The engine, the bounds and the whole run live inside that thread, so nothing of the
/// engine crosses onto the executor serving the request, and the caller's wait is bounded
/// separately ([`CALLER_WAIT_CEILING`]): a run the engine has not come back from by then is
/// given up on, so the request is answered whatever the engine is doing — the given-up run
/// has already spent its own clock, so it ends at the next check the engine makes.
///
/// The run slot is OWNED by that thread rather than by this future: dropping the join handle
/// does not stop a blocking task, so releasing the permit when the caller stops waiting
/// would let a given-up run keep its engine while its slot was handed to the next caller,
/// and repeated calls could then hold any number of engines at once. Held by the run, the
/// slot comes back only when the engine thread ends, so [`MAX_CONCURRENT_RUNS`] bounds the
/// engines that exist whatever a caller does; a caller that finds every slot still taken is
/// answered as a run that did not happen.
pub async fn run_function(code: Vec<u8>, event: String) -> FunctionRun {
    let waited = ::tokio::time::timeout(
        RUN_PERMIT_WAIT,
        ::std::sync::Arc::clone(function_runs()).acquire_owned(),
    )
    .await;
    let permit = match waited {
        Ok(Ok(permit)) => permit,
        _ => {
            return FunctionRun::unavailable(format!(
                "every one of the {MAX_CONCURRENT_RUNS} run slots this emulator holds was \
                 taken for {} seconds",
                RUN_PERMIT_WAIT.as_secs()
            ))
        }
    };
    let running = ::tokio::task::spawn_blocking(move || {
        // The slot is released where the RUN ends, on every path out of it.
        let _slot = permit;
        run(code, &event)
    });
    match ::tokio::time::timeout(CALLER_WAIT_CEILING, running).await {
        Ok(Ok(run)) => run,
        Ok(Err(error)) => FunctionRun::unavailable(format!(
            "the thread the function ran on did not finish: {error}"
        )),
        Err(_) => FunctionRun::unavailable(format!(
            "the function was given up on after {} seconds",
            CALLER_WAIT_CEILING.as_secs()
        )),
    }
}

/// Run `code` over `event` on THIS thread, under every bound this module declares.
fn run(code: Vec<u8>, event: &str) -> FunctionRun {
    let collected = Arc::new(Mutex::new(Collected::default()));
    // The code is a blob on the wire, so it may be bytes no engine can read at all. That is
    // a property of the code rather than of this emulator, so it is the function failing.
    let Ok(source) = String::from_utf8(code) else {
        return finished(
            FunctionOutcome::Failed(NOT_TEXT.to_string()),
            &collected,
            Duration::ZERO,
        );
    };
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            return FunctionRun::unavailable(format!("the engine did not start: {error}"))
        }
    };
    runtime.set_memory_limit(RUN_MEMORY_BYTES);
    runtime.set_max_stack_size(RUN_STACK_BYTES);
    let bounds = Arc::new(Bounds::new());
    let watched = Arc::clone(&bounds);
    // The only thing that can stop code with no exit: a timeout cannot preempt a loop
    // running on this thread, but the engine asks this handler as it works.
    runtime.set_interrupt_handler(Some(Box::new(move || watched.spent())));
    let context = match Context::full(&runtime) {
        Ok(context) => context,
        Err(error) => {
            return FunctionRun::unavailable(format!("the engine did not start: {error}"))
        }
    };

    let answer: Arc<Mutex<Option<Answered>>> = Arc::new(Mutex::new(None));
    let (invoked, mut elapsed) =
        context.with(|ctx| invoke(&ctx, &source, event, &collected, &answer));
    // A handler that returned a promise has not answered yet: its jobs are what settle it,
    // and nothing else of this run drives them.
    if matches!(invoked, Invoked::Pending) {
        let started = Instant::now();
        drain_jobs(&runtime, &bounds);
        elapsed += started.elapsed();
    }
    // The run is over, and nothing the engine does to tear itself down is the function's
    // work: the interrupt handler is taken off first, then any exception a bound left
    // pending in the context is taken, so nothing of a stopped run is held at teardown.
    runtime.set_interrupt_handler(None);
    context.with(|ctx| drop(ctx.catch()));

    // A breached bound is the run's answer, whatever the code then said or returned: the
    // latch is host state the code cannot reach.
    if let Some(breach) = bounds.breach() {
        return finished(FunctionOutcome::Failed(breach), &collected, elapsed);
    }
    let outcome = match invoked {
        Invoked::Unavailable(reason) => FunctionOutcome::Unavailable(reason),
        Invoked::Failed(message) => FunctionOutcome::Failed(message),
        Invoked::Settled | Invoked::Pending => {
            match answer.lock().unwrap_or_else(PoisonError::into_inner).clone() {
                Some(Answered::Returned(text)) => match event_output(&text) {
                    Ok(output) => FunctionOutcome::Returned(output),
                    Err(message) => FunctionOutcome::Failed(message),
                },
                Some(Answered::Raised(message)) => FunctionOutcome::Failed(message),
                None => FunctionOutcome::Failed(UNSETTLED.to_string()),
            }
        }
    };
    finished(outcome, &collected, elapsed)
}

/// The whole run: `outcome`, plus the lines it wrote and the time it spent.
fn finished(
    outcome: FunctionOutcome,
    collected: &Arc<Mutex<Collected>>,
    elapsed: Duration,
) -> FunctionRun {
    let collected = collected.lock().unwrap_or_else(PoisonError::into_inner);
    FunctionRun {
        outcome,
        logs: collected.logs.clone(),
        elapsed,
    }
}

/// What the invocation itself came to, before the latch is read.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Invoked {
    /// The function answered: the latch holds what it came to.
    Settled,
    /// The function returned a promise, so the latch is written by the job that settles it.
    Pending,
    /// The function failed before it could answer.
    Failed(String),
    /// Nothing of the function ran: the engine could not be prepared for it.
    Unavailable(String),
}

/// What the invoker reported the function came to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Answered {
    /// The JSON text of the value the function returned.
    Returned(String),
    /// The message of what it raised, or of what made its answer unreadable.
    Raised(String),
}

/// Compile the code, call its handler over the event object and report what the call came
/// to, inside one engine context — and how long the compile and the call took.
fn invoke(
    ctx: &Ctx<'_>,
    source: &str,
    event: &str,
    collected: &Arc<Mutex<Collected>>,
    answer: &Arc<Mutex<Option<Answered>>>,
) -> (Invoked, Duration) {
    if let Err(reason) = install_console(ctx, collected) {
        return (Invoked::Unavailable(reason), Duration::ZERO);
    }
    // Held BEFORE any of the caller's code is compiled, so the invoker this run reads its
    // answer through is this module's own however the code rebinds the global object.
    let invoker = match ctx.eval::<Function<'_>, _>(INVOKER_SOURCE).catch(ctx) {
        Ok(invoker) => invoker,
        Err(caught) => {
            return (
                Invoked::Unavailable(format!("the engine did not start: {caught}")),
                Duration::ZERO,
            )
        }
    };
    // The event object, read by the engine's own JSON parser. Its caller admits only a
    // document that already parsed as a JSON object, and the text handed here is that
    // document re-rendered, so a failure here is the ENGINE not reading what the host read —
    // never a caller's malformed event object, which its caller refuses as the invalid
    // argument it is.
    let argument = match ctx.json_parse(event.to_string()) {
        Ok(argument) => argument,
        Err(error) => {
            return (
                Invoked::Unavailable(format!("the event object could not be read: {error}")),
                Duration::ZERO,
            )
        }
    };

    // From here on the clock is the FUNCTION's: its compile, and the call of its handler.
    let started = Instant::now();
    if let Err(caught) = ctx.eval::<Value<'_>, _>(source).catch(ctx) {
        return (Invoked::Failed(raised_message(&caught)), started.elapsed());
    }
    let Ok(handler) = ctx.globals().get::<_, Function<'_>>(HANDLER_NAME) else {
        return (Invoked::Failed(NO_HANDLER.to_string()), started.elapsed());
    };
    let latched = Arc::clone(answer);
    // The channel the invoker reports through is passed as an ARGUMENT rather than installed
    // as a global, so the caller's code cannot reach it; the first answer through it is the
    // run's, so a later one changes nothing.
    let settle = Func::from(move |raised: bool, text: String| {
        let mut held = latched.lock().unwrap_or_else(PoisonError::into_inner);
        if held.is_none() {
            *held = Some(if raised {
                Answered::Raised(text)
            } else {
                Answered::Returned(text)
            });
        }
    });
    if let Err(caught) = invoker.call::<_, ()>((handler, argument, settle)).catch(ctx) {
        // The invoker reports the code's own raise itself, so reaching here means the run
        // was interrupted out of it — which the latch has already recorded.
        return (Invoked::Failed(raised_message(&caught)), started.elapsed());
    }
    let pending = answer
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_none();
    let invoked = if pending {
        Invoked::Pending
    } else {
        Invoked::Settled
    };
    (invoked, started.elapsed())
}

/// Run the engine's pending jobs until the returned promise settles, the queue empties, the
/// job bound is spent or a bound of this run ends it.
fn drain_jobs(runtime: &Runtime, bounds: &Arc<Bounds>) {
    for _ in 0..MAX_PENDING_JOBS {
        if bounds.stop_if_due() {
            return;
        }
        match runtime.execute_pending_job() {
            // A job that ended on an exception nothing in the function handled leaves the
            // promise unsettled, which the latch then reports.
            Ok(true) => {}
            Ok(false) | Err(_) => return,
        }
    }
}

/// The result `TestResult$FunctionOutput` carries for a function that returned `text`: the
/// event object it came back with, under the member its own shape names.
///
/// "The event object returned by the function" is a request or a response, and a response
/// object is the one carrying [`RESPONSE_STATUS_MEMBER`] — so which member the returned
/// object travels under is decided by the object itself. The text the engine rendered is
/// reported as it stands rather than re-serialised here, so what a caller reads is what the
/// function returned.
fn event_output(text: &str) -> Result<String, String> {
    let Ok(returned) = ::serde_json::from_str::<Json>(text) else {
        return Err(RETURNED_UNREADABLE.to_string());
    };
    let Json::Object(members) = returned else {
        return Err(RETURNED_NO_EVENT_OBJECT.to_string());
    };
    let member = if members.contains_key(RESPONSE_STATUS_MEMBER) {
        OUTPUT_RESPONSE_MEMBER
    } else {
        OUTPUT_REQUEST_MEMBER
    };
    let output = format!("{{\"{member}\":{text}}}");
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(format!(
            "The function returned an event object of {} bytes, and at most \
             {MAX_OUTPUT_BYTES} bytes are allowed",
            output.len()
        ));
    }
    Ok(output)
}

/// The message a raise carries: the error's own, as the code worded it.
fn raised_message(caught: &::rquickjs::CaughtError<'_>) -> String {
    match caught {
        ::rquickjs::CaughtError::Exception(exception) => exception
            .message()
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| exception.to_string()),
        other => other.to_string(),
    }
}

/// The error message reported for code that is not text at all, so no engine can read it.
const NOT_TEXT: &str = "The function code is not valid UTF-8 text, so it cannot be compiled.";

/// The error message reported for code that compiles but declares no handler to call.
const NO_HANDLER: &str = "The function code does not declare a handler function.";

/// The error message reported for a function whose returned promise never settled, so the
/// run produced no event object.
const UNSETTLED: &str =
    "The function returned a promise that did not settle, so it produced no event object.";

/// The error message reported for a returned value the engine could not render as JSON.
const RETURNED_UNREADABLE: &str = "The function returned a value that is not valid JSON.";

/// The error message reported for a returned value that is not an event object at all.
const RETURNED_NO_EVENT_OBJECT: &str =
    "The function did not return an event object: a function must return the request or the \
     response object.";

/// The log lines one run wrote, under this module's ceilings.
#[derive(Debug, Default)]
struct Collected {
    logs: Vec<String>,
}

impl Collected {
    /// Record one line, under the count and length ceilings, cut on a character boundary so
    /// a capped line is still a string.
    fn log(&mut self, message: &str) {
        if self.logs.len() >= MAX_LOG_ENTRIES {
            return;
        }
        let mut line = message.to_string();
        if line.len() > MAX_LOG_ENTRY_BYTES {
            line.truncate(boundary_at_or_below(&line, MAX_LOG_ENTRY_BYTES));
        }
        self.logs.push(line);
    }
}

/// The largest byte index at or below `limit` that `text` can be cut at.
fn boundary_at_or_below(text: &str, limit: usize) -> usize {
    text.char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= limit)
        .last()
        .unwrap_or_default()
}

/// Install the console the function logs through: ONE host function, which only appends to
/// this run's captured lines, and the console written over it.
///
/// That host function is the only one the running code can reach, and the console source
/// deletes its global binding once it holds it, so the raw writer is reachable from nothing
/// but the console's own closure.
fn install_console(ctx: &Ctx<'_>, collected: &Arc<Mutex<Collected>>) -> Result<(), String> {
    let logged = Arc::clone(collected);
    ctx.globals()
        .set(
            HOST_LOG_NAME,
            Func::from(move |message: String| {
                logged
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .log(&message);
            }),
        )
        .map_err(|error| format!("the console could not be installed: {error}"))?;
    ctx.eval::<Value<'_>, _>(CONSOLE_SOURCE)
        .catch(ctx)
        .map(|_| ())
        .map_err(|caught| format!("the console could not be installed: {caught}"))
}

/// The global the console's writer is handed to the engine as, and which
/// [`CONSOLE_SOURCE`] removes again.
const HOST_LOG_NAME: &str = "__cfHostLog";

/// The console a CloudFront function writes its log lines through: `console.log` and its
/// neighbours, each rendering its arguments the way the engine renders a value and handing
/// the line to the host writer.
const CONSOLE_SOURCE: &str = r#"
(function () {
    var hostLog = globalThis.__cfHostLog;
    delete globalThis.__cfHostLog;
    var rendered = function (value) {
        if (typeof value === "string") { return value; }
        if (value instanceof Error) { return String(value); }
        try {
            var text = JSON.stringify(value);
            return text === undefined ? String(value) : text;
        } catch (error) {
            return String(value);
        }
    };
    var write = function () {
        hostLog(Array.prototype.map.call(arguments, rendered).join(" "));
    };
    globalThis.console = { log: write, info: write, debug: write, warn: write, error: write };
})()
"#;

/// The invoker: the ONE thing that calls the function's handler and reports what it came to,
/// over the channel it is handed.
///
/// It is an expression rather than a global, so the caller's code can neither read it nor
/// replace it, and it captures `JSON.stringify` and `String` before any of that code runs, so
/// code that rebinds either cannot change how its own answer is rendered. A handler that
/// returned a promise reports through the same channel when that promise settles.
const INVOKER_SOURCE: &str = r#"
(function () {
    const stringify = JSON.stringify;
    const toText = String;
    const described = function (error) {
        if (error === null || error === undefined) {
            return toText(error);
        }
        try {
            const message = error.message;
            if (message !== undefined && message !== null && toText(message).length > 0) {
                return toText(message);
            }
        } catch (ignored) { }
        try {
            return toText(error);
        } catch (ignored) {
            return "The function raised a value that cannot be read.";
        }
    };
    return function (handler, event, settle) {
        const returned = function (value) {
            let text;
            try {
                text = stringify(value);
            } catch (error) {
                settle(true, described(error));
                return;
            }
            settle(false, text === undefined ? "null" : text);
        };
        let value;
        try {
            value = handler(event);
        } catch (error) {
            settle(true, described(error));
            return;
        }
        if (value !== null && typeof value === "object" && typeof value.then === "function") {
            try {
                value.then(returned, function (error) { settle(true, described(error)); });
            } catch (error) {
                settle(true, described(error));
            }
            return;
        }
        returned(value);
    };
})()
"#;

/// The run's budget, as the engine's interrupt handler spends it, and the latch that says
/// which bound ended the run.
#[derive(Debug)]
struct Bounds {
    /// When the run must be over.
    deadline: Instant,
    /// How many engine checks this budget started with, which a breach reports.
    budget: u64,
    /// How many engine checks are left.
    checks: AtomicU64,
    /// Set once the clock ran out.
    timed_out: AtomicBool,
    /// Set once the checks ran out.
    steps_spent: AtomicBool,
}

impl Bounds {
    /// A budget starting now, of the checks and the clock one run is held to.
    fn new() -> Self {
        Self::with_budget(RUN_ENGINE_CHECKS)
    }

    /// A budget of `budget` checks starting now — the one constructor, so a budget small
    /// enough to spend in a test is spent through exactly the path a run's is.
    fn with_budget(budget: u64) -> Self {
        Self {
            deadline: Instant::now() + RUN_WALL_CLOCK,
            budget,
            checks: AtomicU64::new(budget),
            timed_out: AtomicBool::new(false),
            steps_spent: AtomicBool::new(false),
        }
    }

    /// Whether the run is over, asked by the engine as it works: the latch first, then the
    /// clock, then the step budget. Each answer latches, so the breach survives code that
    /// catches the exception the engine raises — and a spent budget STAYS spent: the count is
    /// decremented without underflowing, so however many times the engine asks after the
    /// budget ran out, the answer is still "over".
    fn spent(&self) -> bool {
        if self.steps_spent.load(Ordering::Relaxed) {
            return true;
        }
        if self.stop_if_due() {
            return true;
        }
        // `try_update` answers the count as it stood BEFORE this check spent a step, so a
        // `before` of one is the check that spends the last of the budget.
        let before = self
            .checks
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                Some(left.saturating_sub(1))
            })
            .unwrap_or_default();
        if before <= 1 {
            self.steps_spent.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Whether the run's clock has run out, latching it when it has. Read by the host where
    /// the engine is not running code of its own, so a run is stopped between jobs as well
    /// as inside one.
    fn stop_if_due(&self) -> bool {
        if Instant::now() >= self.deadline {
            self.timed_out.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// What the breached bound is reported as, or `None` when none was breached.
    fn breach(&self) -> Option<String> {
        if self.timed_out.load(Ordering::Relaxed) {
            return Some(format!(
                "The function was stopped after {} seconds without returning.",
                RUN_WALL_CLOCK.as_secs()
            ));
        }
        if self.steps_spent.load(Ordering::Relaxed) {
            return Some(format!(
                "The function was stopped after {} engine steps without returning.",
                self.budget
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::Bounds;

    /// The step budget is what stops code the clock has not caught, so it has to STAY spent:
    /// the engine keeps asking after a bound has been breached (every later check, and every
    /// job of a drained queue), and an answer of "keep going" there would hand the run its
    /// budget again. Asserted over a budget small enough that the clock cannot be what ends
    /// it, so what the run reports is the STEP bound on every machine.
    #[test]
    fn a_spent_step_budget_stays_spent() {
        const BUDGET: u64 = 8;

        let bounds = Bounds::with_budget(BUDGET);
        for step in 1..BUDGET {
            assert!(
                !bounds.spent(),
                "step {step}: a budget of {BUDGET} must not be spent yet"
            );
        }
        for step in BUDGET..(BUDGET + 1_000) {
            assert!(
                bounds.spent(),
                "step {step}: expected a spent budget to leave the run over"
            );
        }
        assert_eq!(
            bounds.breach(),
            Some(format!(
                "The function was stopped after {BUDGET} engine steps without returning."
            )),
            "a spent step budget is what the run reports"
        );
    }
}
