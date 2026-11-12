// SPDX-License-Identifier: CC0-1.0

//! The APPSYNC_JS sandbox one `EvaluateCode` call is answered from.
//!
//! `EvaluateCode` "evaluates the given code and returns the response … For APPSYNC_JS
//! runtimes, the code defines the request and response functions", so the code is an ES
//! module whose `request`/`response` export is CALLED over the caller's context, and the
//! call's `evaluationResult` is the JSON of whatever that call returned. Nothing is read
//! out of the code's TEXT: it is compiled and executed by the QuickJS engine embedded in
//! this process (`rquickjs`), the same engine a channel namespace's event handlers run in
//! ([`crate::service::event_api::handlers`]), and the result, the logs, the stash and the
//! out-errors this module reports are the ones that run produced.
//!
//! # What the code can reach
//!
//! A FRESH engine per call, so nothing of one evaluation is reachable from the next: no
//! global, no module instance and no value outlives the call that made it. Inside that
//! engine the code reaches the language and three host functions of this module's own (the
//! log, the appended error and the id mint), and a module resolver that answers with ONE
//! specifier — the `util` helpers AppSync's own resolvers import
//! ([`crate::service::event_api::handlers::UTILS_MODULE`]) — so an `import` of anything
//! else fails to resolve rather than reaching a host path. Plain QuickJS registers no
//! filesystem, process, environment, network, host-library or thread API, and this module
//! adds none: `dyn-load` is left out of the build (see this crate's `Cargo.toml`).
//!
//! # What bounds it
//!
//! Every bound lives in host code the evaluated code cannot reach, and the first three are
//! checked by the engine's own interrupt handler as it works:
//!
//! * a WALL CLOCK ([`RUN_WALL_CLOCK`]), so code that never returns is stopped and the
//!   caller is still answered;
//! * a STEP budget ([`RUN_ENGINE_CHECKS`]), so a tight loop that makes no progress is
//!   stopped even where the clock has not run out;
//! * a MEMORY ceiling ([`RUN_MEMORY_BYTES`]) and a STACK ceiling ([`RUN_STACK_BYTES`]),
//!   both the engine's own, so an allocation or a recursion past them ends the run inside
//!   the engine;
//! * a RESULT ceiling ([`MAX_ANSWER_BYTES`]), the modelled maximum length of
//!   `evaluationResult`, and ceilings on how many log lines ([`MAX_LOG_ENTRIES`]), how
//!   long a line ([`MAX_LOG_ENTRY_BYTES`]) and how many out-errors ([`MAX_OUT_ERRORS`]) one
//!   evaluation may produce, so one call cannot make an unbounded answer;
//! * a CALLER WAIT ceiling ([`CALLER_WAIT_CEILING`]) held outside the engine, so the
//!   request is answered even where a stretch of engine-internal work outlasts the run's
//!   own clock — the engine can only be stopped at a check it makes.
//!
//! Which bound was breached is LATCHED in host state ([`Bounds`]) rather than read back out
//! of the engine, so code that catches the exception the engine raises in it cannot make
//! the run report a result: once the latch is set, the run's answer is the stop. The bounds
//! are this sandbox's own rather than the event-handler path's, because what they bound is
//! one API call the caller waits on, and the breach is reported TO that caller.
//!
//! The run happens on a blocking thread ([`evaluate`]), never on the async executor serving
//! the request.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ::rquickjs::function::Func;
use ::rquickjs::loader::{ImportAttributes, Loader, Resolver};
use ::rquickjs::{CatchResultExt, Context, Ctx, Function, Module, Runtime, Value};
use ::serde_json::Value as Json;

use crate::service::event_api::handlers::UTILS_MODULE;
use crate::state::{now_nanos, splitmix64};

/// How long one evaluation may run before it is stopped.
///
/// A bound of this emulator's own, not a documented AppSync timeout: it exists so one
/// `EvaluateCode` call cannot hold a request — or a thread — open for longer than an
/// evaluation takes.
const RUN_WALL_CLOCK: Duration = Duration::from_secs(2);

/// How long the CALLER waits for a run at most, which is longer than the run's own clock
/// ([`RUN_WALL_CLOCK`]) by the margin it takes the engine to notice that clock.
///
/// A ceiling of the HOST's, held outside the engine: the engine can only be stopped at a
/// check it makes, so a stretch of engine-internal work can outlast the run's own clock.
/// This is what answers the request anyway.
const CALLER_WAIT_CEILING: Duration = Duration::from_secs(5);

/// How many engine checks one evaluation may spend. The engine asks the interrupt handler
/// as it works, so a loop that allocates nothing and calls nothing still runs the count
/// down and is stopped by it.
const RUN_ENGINE_CHECKS: u64 = 20_000_000;

/// The memory ceiling one evaluation's engine is built with.
const RUN_MEMORY_BYTES: usize = 32 * 1024 * 1024;

/// The stack ceiling one evaluation's engine is built with, which is what ends a runaway
/// recursion.
///
/// Well below the stack of the blocking thread the run happens on (`tokio`'s own, which is
/// measured in megabytes), so a recursion past this raises INSIDE the engine and is
/// reported as a failed evaluation rather than overflowing the host thread.
const RUN_STACK_BYTES: usize = 512 * 1024;

/// The largest answer one evaluation may hand back, in bytes: the modelled maximum length
/// of `EvaluationResult` (`@length 0..65536`), which is the longest result the response can
/// carry at all. The stash is held to the same ceiling, for the same reason.
pub const MAX_ANSWER_BYTES: usize = 65_536;

/// How many log lines one evaluation's `logs` may carry.
pub const MAX_LOG_ENTRIES: usize = 100;

/// The largest one log line may be, in bytes.
pub const MAX_LOG_ENTRY_BYTES: usize = 4_096;

/// How many errors one evaluation may append to `outErrors`.
pub const MAX_OUT_ERRORS: usize = 100;

/// The `function` names a request may ask for: "The function within the code to be
/// evaluated. If provided, the valid values are request and response"
/// (`EvaluateCodeRequest$function`).
pub const EVALUATED_FUNCTIONS: &[&str] = &["request", "response"];

/// The function evaluated when the request names none: the first of the two the code
/// defines, which is the one a resolver runs first.
pub const DEFAULT_FUNCTION: &str = "request";

/// The `errorType` of a [`CodeFault`] for code the engine could not compile. "Examples
/// include, but aren't limited to: `LINT_ERROR`, `PARSER_ERROR`" (`CodeError$errorType`).
pub const PARSER_ERROR: &str = "PARSER_ERROR";

/// The `errorType` of a [`CodeFault`] for code that compiles but does not define the
/// function the request asked to evaluate.
pub const LINT_ERROR: &str = "LINT_ERROR";

/// The `errorType` of a [`CodeFault`] found by the compile a create or an update of a
/// resolver or a function runs over the `code` it would store ([`compile_faults`]).
///
/// A written definition is compiled rather than evaluated, so the only fault that path can
/// report is code the engine could not read, and recorded behaviour names that one
/// `PARSE_ERROR` — the `errorType` member is free text ("Examples include, but aren't
/// limited to: `LINT_ERROR`, `PARSER_ERROR`", `CodeError$errorType`), so each path reports
/// the type its own compiler names.
pub const PARSE_ERROR: &str = "PARSE_ERROR";

/// The `span` a fault carries when the engine does not report one: "The span/length of the
/// error. Defaults to -1 if unknown" (`CodeErrorLocation$span`).
pub const UNKNOWN_SPAN: i32 = -1;

/// The unit name the evaluated code is compiled under, so a stack frame taken inside it
/// names the caller's own code rather than a host path. It is also the unit a DEFINITION's
/// own diagnostic names (`crate::state::code_definition`), so one piece of caller code is
/// named one way wherever it is read.
pub const CODE_UNIT: &str = "code.js";

/// The member of a context that carries the stash: "An object available inside each
/// resolver and function handler", which the evaluated code may read and write and which
/// the response reports as it stands after the run.
pub const STASH_MEMBER: &str = "stash";

/// One fault found in the code itself, as a `CodeError` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeFault {
    /// The kind of fault: [`PARSER_ERROR`] or [`LINT_ERROR`].
    pub error_type: &'static str,
    /// The user-presentable error, as the engine worded it.
    pub value: String,
    /// The line the fault sits on, or 0 when the engine reported none.
    pub line: i32,
    /// The column the fault sits on, or 0 when the engine reported none.
    pub column: i32,
    /// The span of the fault, or [`UNKNOWN_SPAN`] when it is unknown.
    pub span: i32,
}

/// What evaluating the code came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The evaluated function returned: the JSON text of the value it returned, as the
    /// engine rendered it (`null` for a function that returned nothing).
    Returned(String),
    /// The evaluated code did not return: it raised, or its module body did. The text is
    /// the error's own message.
    Raised(String),
    /// A bound of this module stopped the run. The text says which.
    Stopped(String),
    /// The code defines nothing this call could run: it does not compile, or it does not
    /// export the function the request named.
    Faults(Vec<CodeFault>),
}

/// One whole evaluation: what the run came to, and everything else it produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    /// What the run came to.
    pub outcome: Outcome,
    /// The log lines the run wrote, through `util.log` and `console`.
    pub logs: Vec<String>,
    /// The stash as it stands after the run, as JSON text, or `None` where the run left it
    /// empty.
    pub stash: Option<String>,
    /// The errors the run appended through `util.appendError`, as the JSON text of a list,
    /// or `None` where it appended none.
    pub out_errors: Option<String>,
}

/// Evaluate `function` of `code` over `context`, on a blocking thread.
///
/// The engine, the bounds and the whole run live inside that thread, so nothing of the
/// engine crosses onto the executor serving the request. The CALLER's wait is bounded
/// separately ([`CALLER_WAIT_CEILING`]): a run the engine has not come back from by then is
/// given up on, so the request is answered whatever the engine is doing — the given-up run
/// has already spent its own clock, so it ends at the next check the engine makes and takes
/// nothing else with it.
pub async fn evaluate(code: String, function: String, context: Json) -> Evaluation {
    let running = ::tokio::task::spawn_blocking(move || run(&code, &function, &context));
    match ::tokio::time::timeout(CALLER_WAIT_CEILING, running).await {
        Ok(Ok(evaluation)) => evaluation,
        Ok(Err(error)) => stopped(format!("the code could not be run: {error}")),
        Err(_) => stopped(format!(
            "the code was given up on after {} seconds",
            CALLER_WAIT_CEILING.as_secs()
        )),
    }
}

/// What a compile of a written definition came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compiled {
    /// The engine READ the code: the faults it found doing so, none for code it read to
    /// the end.
    Read(Vec<CodeFault>),
    /// The compile did not happen, or did not come back: the engine did not start, a bound
    /// of this module stopped it, or the host gave up waiting for it. The text says which.
    /// Nothing was learned about the code, which is a different answer from "it compiled".
    Unknown(String),
}

/// Compile `code` without running it, and report what the compile came to.
///
/// This is the check a call that WRITES a definition makes: a resolver or a function is
/// stored only if the engine that would RUN it can compile it, and nothing of the code
/// runs — no host function and no context is installed, and no module body is evaluated.
/// Every bound a run is held to holds here too (the engine's memory and stack ceilings,
/// the wall clock and the step budget, checked by the engine's own interrupt handler), so
/// one definition's nesting or size cannot take the host's stack: the engine raises inside
/// itself instead. The compile happens on a blocking thread, and the caller's wait is
/// bounded the way a run's is.
///
/// A compile that did not happen is reported as [`Compiled::Unknown`] rather than as a
/// clean one: the caller then answers the server fault its model declares instead of
/// storing a definition nothing could read.
pub async fn compile_faults(code: String) -> Compiled {
    let compiling = ::tokio::task::spawn_blocking(move || compile(&code));
    match ::tokio::time::timeout(CALLER_WAIT_CEILING, compiling).await {
        Ok(Ok(compiled)) => compiled,
        Ok(Err(error)) => Compiled::Unknown(format!("the code could not be compiled: {error}")),
        Err(_) => Compiled::Unknown(format!(
            "the compile was given up on after {} seconds",
            CALLER_WAIT_CEILING.as_secs()
        )),
    }
}

/// Compile `code` on THIS thread, under every bound this module declares, and report what
/// the compiler made of it.
fn compile(code: &str) -> Compiled {
    let Ok(runtime) = Runtime::new() else {
        return Compiled::Unknown("the engine did not start".to_string());
    };
    runtime.set_memory_limit(RUN_MEMORY_BYTES);
    // The engine's own stack ceiling, which is what makes a deeply nested definition a
    // bounded failure INSIDE the engine rather than a fault of the host's stack.
    runtime.set_max_stack_size(RUN_STACK_BYTES);
    // The engine resolves every `import` as it compiles, and what the compile asks is
    // whether the code can be READ: a specifier is therefore answered with an empty
    // module rather than with a host path or a resolution failure, so a definition
    // importing a helper module is not reported as code the engine could not read.
    // Nothing of that module is evaluated either — the compile never evaluates.
    runtime.set_loader(AnyResolver, EmptyLoader);
    let bounds = Arc::new(Bounds::new());
    let watched = Arc::clone(&bounds);
    runtime.set_interrupt_handler(Some(Box::new(move || watched.spent())));
    let Ok(engine_context) = Context::full(&runtime) else {
        return Compiled::Unknown("the engine did not start".to_string());
    };
    let faults = engine_context.with(|ctx| {
        match Module::declare(ctx.clone(), CODE_UNIT, code.to_string()).catch(&ctx) {
            Ok(_declared) => Vec::new(),
            Err(caught) => vec![compile_fault(&caught)],
        }
    });
    // A compile the clock or the step budget stopped did not read the code to the end, so
    // what it stopped at is not a fault OF the code: the breach is the answer, the way it
    // is for a run.
    match bounds.breach() {
        Some(breach) => Compiled::Unknown(breach),
        None => Compiled::Read(faults),
    }
}

/// How an `import` specifier is resolved for a COMPILE: to itself, whatever it names, so
/// no specifier fails to resolve and none is looked for on a host path.
#[derive(Debug, Clone, Copy)]
struct AnyResolver;

impl Resolver for AnyResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        _base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> ::rquickjs::Result<String> {
        Ok(name.to_string())
    }
}

/// How a resolved specifier is loaded for a COMPILE: as a module declared from this
/// module's own source. No path is opened and nothing is evaluated, so the import
/// contributes nothing but the fact that it resolved.
#[derive(Debug, Clone, Copy)]
struct EmptyLoader;

impl Loader for EmptyLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        path: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> ::rquickjs::Result<Module<'js>> {
        Module::declare(ctx.clone(), path.to_string(), EMPTY_MODULE_SOURCE)
    }
}

/// The source [`EmptyLoader`] declares every specifier from: a module with one default
/// member, so the import of a helper module resolves and the compile goes on reading the
/// code.
const EMPTY_MODULE_SOURCE: &str = "export default {};\n";

/// The fault code the engine could not compile takes: the engine's own account of where it
/// stopped reading, at the position it states.
///
/// The engine states the failure as one message. Where it carries a position of its own,
/// that is the fault's; where it carries none, the fault takes the values
/// `CodeErrorLocation` documents for an unknown one (`0`), and the caller of this module
/// is what decides whether the code itself states the position instead
/// (`crate::state::code_definition`).
fn compile_fault(caught: &::rquickjs::CaughtError<'_>) -> CodeFault {
    let (value, line, column) = match caught {
        ::rquickjs::CaughtError::Exception(exception) => {
            let stated = |member: &str| {
                exception
                    .get::<_, Option<i32>>(member)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            };
            (
                exception
                    .message()
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| exception.to_string()),
                stated("lineNumber"),
                stated("columnNumber"),
            )
        }
        other => (other.to_string(), 0, 0),
    };
    CodeFault {
        error_type: PARSE_ERROR,
        value,
        line,
        column,
        span: UNKNOWN_SPAN,
    }
}

/// An evaluation that produced nothing but the reason it was stopped.
fn stopped(reason: String) -> Evaluation {
    Evaluation {
        outcome: Outcome::Stopped(reason),
        logs: Vec::new(),
        stash: None,
        out_errors: None,
    }
}

/// Evaluate `function` of `code` over `context` on THIS thread, under every bound this
/// module declares.
fn run(code: &str, function: &str, context: &Json) -> Evaluation {
    let collected = Arc::new(Mutex::new(Collected::default()));
    let Ok(context_json) = ::serde_json::to_string(context) else {
        return finished(
            Outcome::Stopped("the context could not be rendered".to_string()),
            &collected,
            None,
        );
    };
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            return finished(
                Outcome::Stopped(format!("the engine did not start: {error}")),
                &collected,
                None,
            )
        }
    };
    runtime.set_memory_limit(RUN_MEMORY_BYTES);
    runtime.set_max_stack_size(RUN_STACK_BYTES);
    runtime.set_loader(UtilsResolver, UtilsLoader);
    let bounds = Arc::new(Bounds::new());
    let watched = Arc::clone(&bounds);
    // The only thing that can stop code with no exit: a timeout cannot preempt a loop
    // running on this thread, but the engine asks this handler as it works.
    runtime.set_interrupt_handler(Some(Box::new(move || watched.spent())));
    let engine_context = match Context::full(&runtime) {
        Ok(engine_context) => engine_context,
        Err(error) => {
            return finished(
                Outcome::Stopped(format!("the engine did not start: {error}")),
                &collected,
                None,
            )
        }
    };
    let (outcome, stash) = engine_context.with(|ctx| {
        evaluate_in(&ctx, code, function, &context_json, &Arc::clone(&collected))
    });
    // A breached bound is the run's answer, whatever the code then said or returned: the
    // latch is host state the code cannot reach.
    match bounds.breach() {
        Some(breach) => finished(Outcome::Stopped(breach), &collected, stash),
        None => finished(outcome, &collected, stash),
    }
}

/// The whole evaluation: `outcome`, plus the lines and errors the run produced and the
/// stash it left.
fn finished(
    outcome: Outcome,
    collected: &Arc<Mutex<Collected>>,
    stash: Option<String>,
) -> Evaluation {
    let collected = collected.lock().unwrap_or_else(PoisonError::into_inner);
    Evaluation {
        outcome,
        logs: collected.logs.clone(),
        // An empty stash and an empty out-error list carry nothing, so the response states
        // neither: the members are there to report what the run left behind.
        stash: stash.filter(|text| !is_empty_object(text)),
        out_errors: collected.out_errors(),
    }
}

/// Whether `text` is the JSON of an object with no members, which is a stash nothing wrote
/// to.
fn is_empty_object(text: &str) -> bool {
    text.trim() == "{}"
}

/// Compile the module, call `function` and read back what it returned and what the stash
/// holds, inside one engine context.
fn evaluate_in(
    ctx: &Ctx<'_>,
    code: &str,
    function: &str,
    context_json: &str,
    collected: &Arc<Mutex<Collected>>,
) -> (Outcome, Option<String>) {
    if let Err(error) = install_host(ctx, collected) {
        return (Outcome::Stopped(error), None);
    }
    if let Err(caught) = ctx.eval::<Value<'_>, _>(CONSOLE_SOURCE).catch(ctx) {
        return (
            Outcome::Stopped(format!("the sandbox console did not install: {caught}")),
            None,
        );
    }
    // Held BEFORE any of the caller's code is compiled, so the invoker this run reads its
    // answer through is this module's own however the code rebinds the global object.
    let invoker = match ctx.eval::<Function<'_>, _>(INVOKER_SOURCE).catch(ctx) {
        Ok(invoker) => invoker,
        Err(caught) => {
            return (
                Outcome::Stopped(format!("the sandbox did not start: {caught}")),
                None,
            )
        }
    };
    let declared = match Module::declare(ctx.clone(), CODE_UNIT, code.to_string()).catch(ctx) {
        Ok(declared) => declared,
        // The engine resolves every `import` as it compiles, so a module this sandbox
        // serves none for is reported here beside code that does not parse.
        Err(caught) => return (Outcome::Faults(vec![parser_fault(&caught)]), None),
    };
    let (evaluated, settled) = match declared.eval().catch(ctx) {
        Ok(evaluated) => evaluated,
        Err(caught) => return (Outcome::Faults(vec![parser_fault(&caught)]), None),
    };
    // The module's body is evaluated through a promise, and nothing of this run drives a
    // job queue, so a body that RAISED is read off that promise here — otherwise its
    // failure would be lost and the hoisted exports would be called as though the module
    // had loaded.
    if let Err(caught) = settled.finish::<()>().catch(ctx) {
        if !pending(&caught) {
            return (Outcome::Raised(raised_message(&caught)), None);
        }
    }
    let Ok(handler) = evaluated.get::<_, Function<'_>>(function) else {
        return (
            Outcome::Faults(vec![CodeFault {
                error_type: LINT_ERROR,
                value: format!("The code does not export a {function} function."),
                line: 0,
                column: 0,
                span: UNKNOWN_SPAN,
            }]),
            None,
        );
    };
    let argument = match ctx.json_parse(context_json.to_string()) {
        Ok(argument) => argument,
        Err(error) => {
            return (
                Outcome::Stopped(format!("the context could not be read: {error}")),
                None,
            )
        }
    };
    let answered = match invoker
        .call::<_, String>((handler, argument))
        .catch(ctx)
    {
        Ok(answered) => answered,
        // The invoker reports the code's own raise itself, so reaching here means the run
        // was interrupted out of it — which the latch has already recorded.
        Err(caught) => return (Outcome::Raised(raised_message(&caught)), None),
    };
    read_answer(&answered)
}

/// The invoker's answer: what the run came to, and the stash it left.
fn read_answer(answered: &str) -> (Outcome, Option<String>) {
    let Ok(document) = ::serde_json::from_str::<Json>(answered) else {
        return (
            Outcome::Stopped("the sandbox answered with a document that is not JSON".to_string()),
            None,
        );
    };
    let member = |name: &str| {
        document
            .get(name)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let stash = document
        .get(STASH_MEMBER)
        .and_then(Json::as_str)
        .filter(|text| text.len() <= MAX_ANSWER_BYTES)
        .map(str::to_string);
    let outcome = match document.get("outcome").and_then(Json::as_str) {
        Some("returned") => {
            let result = member("result");
            if result.len() > MAX_ANSWER_BYTES {
                Outcome::Stopped(format!(
                    "the code returned more than {MAX_ANSWER_BYTES} bytes"
                ))
            } else {
                Outcome::Returned(result)
            }
        }
        Some("raised" | "unreadable") => Outcome::Raised(member("message")),
        _ => Outcome::Stopped("the sandbox answered with no outcome".to_string()),
    };
    (outcome, stash)
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

/// Whether `caught` is the engine saying the promise has not settled, which is not a
/// failure of the code: APPSYNC_JS code is synchronous, so a module that has not settled
/// after its body ran is one waiting on work nothing in this sandbox can do.
fn pending(caught: &::rquickjs::CaughtError<'_>) -> bool {
    matches!(
        caught,
        ::rquickjs::CaughtError::Error(::rquickjs::Error::WouldBlock)
    )
}

/// The fault code the engine could not compile takes.
///
/// The engine reports the failure as one message, which names where in the code it stopped;
/// it hands back no line or column of its own, so the location takes the values
/// `CodeErrorLocation` documents for an unknown one (0 for the line and the column,
/// [`UNKNOWN_SPAN`] for the span) and the message is reported verbatim as the user
/// presentable error.
fn parser_fault(caught: &::rquickjs::CaughtError<'_>) -> CodeFault {
    let value = match caught {
        ::rquickjs::CaughtError::Exception(exception) => exception
            .message()
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| exception.to_string()),
        other => other.to_string(),
    };
    CodeFault {
        error_type: PARSER_ERROR,
        value,
        line: 0,
        column: 0,
        span: UNKNOWN_SPAN,
    }
}

/// The lines and errors one run produced, under this module's ceilings.
#[derive(Debug, Default)]
struct Collected {
    /// The log lines, in the order the run wrote them.
    logs: Vec<String>,
    /// The appended errors, each as the JSON text of one error, in the order the run
    /// appended them.
    appended: Vec<String>,
}

impl Collected {
    /// Record one log line, under the line and length ceilings.
    fn log(&mut self, level: &str, message: &str) {
        if self.logs.len() >= MAX_LOG_ENTRIES {
            return;
        }
        let mut line = format!("{level} - {message}");
        if line.len() > MAX_LOG_ENTRY_BYTES {
            line.truncate(boundary_at_or_below(&line, MAX_LOG_ENTRY_BYTES));
        }
        self.logs.push(line);
    }

    /// Record one appended error, under the count ceiling. An entry that is not a JSON
    /// document is dropped rather than put in a list the response says is JSON.
    fn append_error(&mut self, entry: &str) {
        if self.appended.len() >= MAX_OUT_ERRORS
            || entry.len() > MAX_ANSWER_BYTES
            || ::serde_json::from_str::<Json>(entry).is_err()
        {
            return;
        }
        self.appended.push(entry.to_string());
    }

    /// The appended errors as the JSON text of a list, or `None` where the run appended
    /// none. Assembled from the texts the engine rendered, so each error reads as the code
    /// wrote it.
    fn out_errors(&self) -> Option<String> {
        if self.appended.is_empty() {
            return None;
        }
        Some(format!("[{}]", self.appended.join(",")))
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

/// Install the host functions this sandbox carries: the log, the appended error and the id
/// mint, each of which only writes to or reads from THIS run.
///
/// Nothing else of the host is reachable from the engine.
fn install_host(ctx: &Ctx<'_>, collected: &Arc<Mutex<Collected>>) -> Result<(), String> {
    let logged = Arc::clone(collected);
    let appended = Arc::clone(collected);
    let globals = ctx.globals();
    globals
        .set(
            "__appsyncLog",
            Func::from(move |level: String, message: String| {
                logged
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .log(&level, &message);
            }),
        )
        .map_err(|error| format!("the sandbox log could not be installed: {error}"))?;
    globals
        .set(
            "__appsyncAppendError",
            Func::from(move |entry: String| {
                appended
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .append_error(&entry);
            }),
        )
        .map_err(|error| format!("the sandbox error list could not be installed: {error}"))?;
    globals
        .set("__appsyncAutoId", Func::from(auto_id))
        .map_err(|error| format!("the sandbox id mint could not be installed: {error}"))
}

/// A fresh identifier for `util.autoId`, in the UUID form AppSync's own ids take.
///
/// Drawn from the clock and from a counter this process advances, both put through the
/// bijection this service already mints its opaque ids with (`crate::state`): the counter
/// is what tells two calls made in the same nanosecond apart, so no two calls — on this
/// thread or on another — answer with the same id.
fn auto_id() -> String {
    static TURNS: AtomicU64 = AtomicU64::new(0);
    let turn = TURNS.fetch_add(1, Ordering::Relaxed);
    let high = splitmix64(now_nanos() ^ turn);
    let low = splitmix64(high ^ turn.rotate_left(32));
    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&high.to_be_bytes());
    bytes[8..].copy_from_slice(&low.to_be_bytes());
    ::uuid::Builder::from_random_bytes(bytes)
        .into_uuid()
        .to_string()
}

/// The invoker: the ONE thing that calls the evaluated function and renders what it came
/// to, as the JSON text of a document the host reads.
///
/// It is an expression rather than a global, so the evaluated code can neither read it nor
/// replace it, and it captures `JSON.stringify` and `String` before any of that code runs,
/// so a module that rebinds either cannot change how its own answer is rendered. The
/// result and the stash are handed back ALREADY rendered, so the response carries the
/// document the engine wrote rather than one re-serialised on the host.
const INVOKER_SOURCE: &str = r#"
(function () {
    const stringify = JSON.stringify;
    const toText = String;
    const render = function (value) {
        if (value === undefined) {
            return "null";
        }
        const text = stringify(value);
        return text === undefined ? "null" : text;
    };
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
            return "the code raised a value that cannot be read";
        }
    };
    const stashed = function (context) {
        try {
            if (context === null || typeof context !== "object") {
                return "{}";
            }
            return render(context.stash === undefined ? {} : context.stash);
        } catch (ignored) {
            return "{}";
        }
    };
    return function (handler, context) {
        let value;
        try {
            value = handler(context);
        } catch (error) {
            // `runtime.earlyReturn(obj)` ends the call with `obj` as its answer, so the
            // raise it unwinds with is the code RETURNING rather than failing.
            if (error !== null && typeof error === "object" && error.__appsyncEarlyReturn === true) {
                return stringify({
                    outcome: "returned",
                    result: render(error.data),
                    stash: stashed(context),
                });
            }
            return stringify({
                outcome: "raised",
                message: described(error),
                stash: stashed(context),
            });
        }
        try {
            return stringify({
                outcome: "returned",
                result: render(value),
                stash: stashed(context),
            });
        } catch (error) {
            return stringify({
                outcome: "unreadable",
                message: described(error),
                stash: stashed(context),
            });
        }
    };
})()
"#;

/// The `util` helpers [`UTILS_MODULE`] exports to the evaluated code, as the module's own
/// source.
///
/// Each one is either the code's OWN control flow or a value this sandbox can answer with:
/// `util.error` and `util.unauthorized` RAISE, which is how a resolver refuses;
/// `util.appendError` "adds an error to the response" without ending the run, which is what
/// `outErrors` reports; `util.log.info` and `util.log.error` write the lines `logs` reports;
/// `util.autoId` mints an identifier; and `util.time` reads the clock through the engine's
/// own `Date`. Nothing else is offered, so code reaching for a helper this emulator does
/// not implement fails loudly rather than silently doing nothing.
const UTILS_MODULE_SOURCE: &str = r#"
const hostLog = __appsyncLog;
const hostAppendError = __appsyncAppendError;
const hostAutoId = __appsyncAutoId;
const rendered = function (value) {
    if (typeof value === "string") {
        return value;
    }
    const text = JSON.stringify(value);
    return text === undefined ? String(value) : text;
};
const joined = function (args) {
    return Array.prototype.map.call(args, rendered).join(" ");
};
const detail = function (message, errorType, data, errorInfo) {
    return {
        message: message === undefined || message === null ? "" : String(message),
        errorType: errorType === undefined || errorType === null ? null : String(errorType),
        data: data === undefined ? null : data,
        errorInfo: errorInfo === undefined ? null : errorInfo,
    };
};
const raise = function (message, errorType, data, errorInfo) {
    const described = detail(message, errorType, data, errorInfo);
    const error = new Error(described.message);
    if (described.errorType !== null) {
        error.name = described.errorType;
    }
    error.data = described.data;
    error.errorInfo = described.errorInfo;
    throw error;
};
export const util = {
    error: raise,
    appendError: function (message, errorType, data, errorInfo) {
        hostAppendError(JSON.stringify(detail(message, errorType, data, errorInfo)));
    },
    unauthorized: function () {
        const error = new Error("Unauthorized");
        error.name = "Unauthorized";
        throw error;
    },
    autoId: function () {
        return hostAutoId();
    },
    log: {
        info: function () { hostLog("INFO", joined(arguments)); },
        error: function () { hostLog("ERROR", joined(arguments)); },
    },
    time: {
        nowISO8601: function () { return new Date().toISOString(); },
        nowEpochSeconds: function () { return Math.floor(Date.now() / 1000); },
        nowEpochMilliSeconds: function () { return Date.now(); },
    },
};
export const runtime = {
    earlyReturn: function (value) {
        const error = new Error("early return");
        error.name = "EarlyReturn";
        error.__appsyncEarlyReturn = true;
        error.data = value === undefined ? null : value;
        throw error;
    },
};
export default { util, runtime };
"#;

/// The `console` the evaluated code writes through, installed by the HOST before any of
/// that code is compiled and holding the log function it writes to at the moment it is
/// installed, so a module that rebinds the host function cannot redirect it.
///
/// `console.log` and `console.error` are what an APPSYNC_JS resolver logs with beside
/// `util.log`, and both land in the `logs` the response reports. Code that replaces
/// `globalThis.console` only stops its OWN lines from being logged.
const CONSOLE_SOURCE: &str = r#"
(function () {
    const hostLog = __appsyncLog;
    const rendered = function (value) {
        if (typeof value === "string") {
            return value;
        }
        const text = JSON.stringify(value);
        return text === undefined ? String(value) : text;
    };
    const writer = function (level) {
        return function () {
            hostLog(level, Array.prototype.map.call(arguments, rendered).join(" "));
        };
    };
    globalThis.console = {
        log: writer("INFO"),
        info: writer("INFO"),
        debug: writer("INFO"),
        warn: writer("ERROR"),
        error: writer("ERROR"),
    };
})()
"#;

/// The run's budget, as the engine's interrupt handler spends it, and the latch that says
/// which bound ended the run.
#[derive(Debug)]
struct Bounds {
    /// When the run must be over.
    deadline: Instant,
    /// How many engine checks are left.
    checks: AtomicU64,
    /// Set once the clock ran out.
    timed_out: AtomicBool,
    /// Set once the checks ran out.
    steps_spent: AtomicBool,
}

impl Bounds {
    /// A budget starting now.
    fn new() -> Self {
        Self {
            deadline: Instant::now() + RUN_WALL_CLOCK,
            checks: AtomicU64::new(RUN_ENGINE_CHECKS),
            timed_out: AtomicBool::new(false),
            steps_spent: AtomicBool::new(false),
        }
    }

    /// Whether the run is over, asked by the engine as it works: the clock, then the step
    /// budget. Each answer latches, so the breach survives code that catches the exception
    /// the engine raises.
    fn spent(&self) -> bool {
        if Instant::now() >= self.deadline {
            self.timed_out.store(true, Ordering::Relaxed);
            return true;
        }
        if self.checks.fetch_sub(1, Ordering::Relaxed) <= 1 {
            self.steps_spent.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// What the breached bound is reported as, or `None` when none was breached.
    fn breach(&self) -> Option<String> {
        if self.timed_out.load(Ordering::Relaxed) {
            return Some(format!(
                "the code was stopped after {} seconds",
                RUN_WALL_CLOCK.as_secs()
            ));
        }
        if self.steps_spent.load(Ordering::Relaxed) {
            return Some(format!(
                "the code was stopped after {RUN_ENGINE_CHECKS} engine steps"
            ));
        }
        None
    }
}

/// How an `import` specifier is resolved: to [`UTILS_MODULE`] and to nothing else, so no
/// host path and no other module is reachable from the evaluated code.
#[derive(Debug, Clone, Copy)]
struct UtilsResolver;

impl Resolver for UtilsResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> ::rquickjs::Result<String> {
        if name == UTILS_MODULE {
            return Ok(UTILS_MODULE.to_string());
        }
        Err(::rquickjs::Error::new_resolving_message(
            base.to_string(),
            name.to_string(),
            format!("the only module an APPSYNC_JS runtime provides is {UTILS_MODULE}"),
        ))
    }
}

/// How the one resolvable module is loaded: from the source this module carries
/// ([`UTILS_MODULE_SOURCE`]). No path is opened for it.
#[derive(Debug, Clone, Copy)]
struct UtilsLoader;

impl Loader for UtilsLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        path: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> ::rquickjs::Result<Module<'js>> {
        if path != UTILS_MODULE {
            return Err(::rquickjs::Error::new_loading(path.to_string()));
        }
        Module::declare(ctx.clone(), path.to_string(), UTILS_MODULE_SOURCE)
    }
}
