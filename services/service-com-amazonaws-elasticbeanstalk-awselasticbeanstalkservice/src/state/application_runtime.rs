// SPDX-License-Identifier: CC0-1.0

//! The APPLICATION an environment runs: the engine the deployed source bundle's
//! own code executes in, in this process.
//!
//! An Elastic Beanstalk environment "launches an environment for the specified
//! application" and the environment then RUNS the application version deployed to
//! it: its platform starts the entry point the bundle carries, that process writes
//! to its own log, and it answers the HTTP traffic the environment's address
//! receives. Both of those are observable through the API — the log a
//! `RequestEnvironmentInfo`/`RetrieveEnvironmentInfo` pair compiles carries what
//! the application wrote, and the address `EnvironmentDescription` publishes
//! answers what the application serves — so neither can be synthesized: they are
//! whatever the deployed code produces when it RUNS.
//!
//! So the code runs, on the QuickJS engine embedded in this process
//! (`rquickjs`), under bounds set before anything is evaluated: a wall-clock
//! deadline the engine's interrupt handler enforces ([`RUN_WALL_CLOCK`]), a heap
//! ([`RUN_MEMORY_BYTES`]), a JavaScript stack ([`RUN_STACK_BYTES`]) and a cap on
//! how much output one run reports back ([`MAX_CAPTURED_BYTES`]). There is no
//! module loader, no filesystem, no network and no child process: the only host
//! functions a bundle can reach are the console's two writers and the one the
//! response travels back over, and `require` answers the handful of Node.js
//! modules a web application's entry point uses to START
//! ([`RUNTIME_PRELUDE`]) — anything else throws, the way a missing module does.
//!
//! The run happens on a BLOCKING thread (`tokio::task::spawn_blocking`) at the
//! call sites in [`super::deployed_application`], never on the async executor:
//! the engine is synchronous, and the interrupt handler is what stops a script
//! with no exit.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rquickjs::function::Func;
use rquickjs::{CatchResultExt, Context, Ctx, Runtime, Value};

/// How long one run of a deployed application may take before the engine's
/// interrupt handler stops it.
///
/// An EMULATOR bound, not an AWS one: the code runs in this process, so a loop
/// with no exit would otherwise hold a thread for ever. Long enough for an
/// application's entry point to start and answer one request.
const RUN_WALL_CLOCK: Duration = Duration::from_secs(2);

/// How many times the engine may ask whether to keep going before one run is
/// stopped — the STEP bound beside the wall-clock one, and the one that does not
/// depend on how fast the host is.
///
/// The engine asks its interrupt handler once per fixed number of bytecode
/// operations, so a budget of asks is a budget of operations: code that diverges
/// is stopped after the same amount of WORK on every machine, and the deadline
/// above is what bounds a run that blocks without executing.
const RUN_STEP_BUDGET: u64 = 10_000;

/// The heap one run may use, set on the engine before anything is evaluated.
const RUN_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// The JavaScript stack one run may use. Well inside the stack of the blocking
/// thread the run happens on, so deep recursion in a bundle ends that run with
/// the engine's own stack-overflow exception rather than touching the thread's
/// own guard page.
const RUN_STACK_BYTES: usize = 512 * 1024;

/// The most bytes of output one run reports back, over both streams and the
/// response body. Past it a stream keeps what it already captured and the rest
/// is dropped, so a one-line bundle cannot make an unbounded log or response.
const MAX_CAPTURED_BYTES: usize = 64 * 1024;

/// One HTTP request handed to the deployed application: what the environment's
/// own address received.
#[derive(Debug, Clone, Default)]
pub struct GuestRequest {
    pub method: String,
    /// The request target as the application reads it off `req.url`: the path
    /// with its query string, as Node.js reports it.
    pub path: String,
    pub headers: Vec<(String, String)>,
}

/// What the deployed application answered one request with.
#[derive(Debug, Clone)]
pub struct GuestResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What one run of a deployed application produced: what it wrote to its own
/// output streams, the response it gave the request it was handed (when it was
/// handed one and answered it), and the engine's account of an uncaught failure.
#[derive(Debug, Clone, Default)]
pub struct GuestRun {
    /// Everything the run wrote to standard output, verbatim — the bytes the
    /// application's own log carries, so non-ASCII text survives unchanged.
    pub stdout: String,
    /// Everything it wrote to standard error, verbatim.
    pub stderr: String,
    pub response: Option<GuestResponse>,
    /// The uncaught exception that ended the run, if one did.
    pub failure: Option<String>,
}

/// What a run captured while it was going on.
#[derive(Debug, Default)]
struct Capture {
    stdout: String,
    stderr: String,
    response: Option<GuestResponse>,
}

impl Capture {
    /// Append `text` to one of the streams, inside what is left of
    /// [`MAX_CAPTURED_BYTES`] for it and on a character boundary, so a capped
    /// stream is still a string.
    fn write(&mut self, error_stream: bool, text: &str) {
        let stream = if error_stream {
            &mut self.stderr
        } else {
            &mut self.stdout
        };
        let room = MAX_CAPTURED_BYTES.saturating_sub(stream.len());
        if room == 0 {
            return;
        }
        let mut end = text.len().min(room);
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        stream.push_str(&text[..end]);
    }
}

/// The guard over the capture, recovering a poisoned lock rather than panicking
/// with it: the capture is this run's own, so whatever a panic left in it is
/// still the only output there is.
fn captured(capture: &Arc<Mutex<Capture>>) -> ::std::sync::MutexGuard<'_, Capture> {
    capture.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Run the deployed application's entry point `entry`, with `env` as its process
/// environment, and hand it `request` when there is one to serve.
///
/// Called on a blocking thread with no state guard held. Every bound this module
/// declares is set on the engine before anything is evaluated, so the code
/// cannot outrun them.
pub fn run_application(
    entry: &str,
    env: &[(String, String)],
    request: Option<&GuestRequest>,
) -> GuestRun {
    let capture = Arc::new(Mutex::new(Capture::default()));
    let failure = evaluate(&capture, entry, env, request).err();
    let captured = captured(&capture);
    GuestRun {
        stdout: captured.stdout.clone(),
        stderr: captured.stderr.clone(),
        response: captured.response.clone(),
        failure,
    }
}

/// Build the engine, install the host functions and the runtime prelude over
/// them, evaluate the entry point and — for a run that carries one — hand the
/// request to whatever server the entry point started.
fn evaluate(
    capture: &Arc<Mutex<Capture>>,
    entry: &str,
    env: &[(String, String)],
    request: Option<&GuestRequest>,
) -> Result<(), String> {
    let runtime = Runtime::new().map_err(|error| format!("the engine did not start: {error}"))?;
    runtime.set_memory_limit(RUN_MEMORY_BYTES);
    runtime.set_max_stack_size(RUN_STACK_BYTES);
    let deadline = Instant::now() + RUN_WALL_CLOCK;
    // The only thing that can stop code with no exit: a timeout cannot preempt a
    // loop running on this thread, but the engine asks this handler as it works
    // — so the handler counts the asks (the step bound) and checks the clock.
    let mut steps: u64 = 0;
    runtime.set_interrupt_handler(Some(Box::new(move || {
        steps += 1;
        steps > RUN_STEP_BUDGET || Instant::now() >= deadline
    })));
    let context =
        Context::full(&runtime).map_err(|error| format!("the engine did not start: {error}"))?;

    context.with(|ctx| {
        install_host_functions(&ctx, capture)?;
        set_json(&ctx, "__ebEnv", &environment_object(env))?;
        run_one(&ctx, RUNTIME_PRELUDE)?;
        // The application's own entry point. Whatever it logs while starting is
        // what its log carries, and whatever server it starts is what answers
        // below.
        run_one(&ctx, entry)?;
        let Some(request) = request else {
            return Ok(());
        };
        set_json(&ctx, "__ebRequest", &request_object(request))?;
        run_one(&ctx, DISPATCH_PRELUDE)
    })
}

/// Evaluate one script in `ctx`, turning an uncaught exception into the text the
/// run reports as its failure.
fn run_one(ctx: &Ctx<'_>, script: &str) -> Result<(), String> {
    ctx.eval::<Value<'_>, _>(script)
        .catch(ctx)
        .map(|_| ())
        .map_err(|caught| caught.to_string())
}

/// Define the global `name` as the value `json` parses to, so a value this
/// emulator hands the guest is built by the engine's own `JSON.parse` rather
/// than assembled through the host binding layer.
fn set_json(ctx: &Ctx<'_>, name: &str, json: &str) -> Result<(), String> {
    let script = format!("globalThis.{name} = JSON.parse({});", json_literal(json));
    run_one(ctx, &script)
}

/// `json` as a JavaScript string literal, which is what [`set_json`] parses: JSON
/// is itself valid JSON, so the text is encoded once more by the one encoder this
/// crate already has.
fn json_literal(json: &str) -> String {
    ::serde_json::Value::String(json.to_string()).to_string()
}

/// The process environment, as the JSON `process.env` is parsed from.
fn environment_object(env: &[(String, String)]) -> String {
    let map: ::serde_json::Map<String, ::serde_json::Value> = env
        .iter()
        .map(|(name, value)| (name.clone(), ::serde_json::Value::String(value.clone())))
        .collect();
    ::serde_json::Value::Object(map).to_string()
}

/// One request, as the JSON the `req` object handed to the application is parsed
/// from.
fn request_object(request: &GuestRequest) -> String {
    let headers: ::serde_json::Map<String, ::serde_json::Value> = request
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.to_ascii_lowercase(),
                ::serde_json::Value::String(value.clone()),
            )
        })
        .collect();
    ::serde_json::json!({
        "method": request.method,
        "url": request.path,
        "headers": ::serde_json::Value::Object(headers),
    })
    .to_string()
}

/// Install the three host functions the guest reaches the emulator through: the
/// two console writers and the one a response travels back over.
///
/// These are the ONLY host functions the deployed code can reach. Plain QuickJS
/// exposes no filesystem, process, environment or network API, and nothing here
/// adds one.
fn install_host_functions(ctx: &Ctx<'_>, capture: &Arc<Mutex<Capture>>) -> Result<(), String> {
    let to_stdout = Arc::clone(capture);
    ctx.globals()
        .set(
            "__hostStdout",
            Func::from(move |text: String| captured(&to_stdout).write(false, &text)),
        )
        .map_err(|error| format!("the console could not be installed: {error}"))?;
    let to_stderr = Arc::clone(capture);
    ctx.globals()
        .set(
            "__hostStderr",
            Func::from(move |text: String| captured(&to_stderr).write(true, &text)),
        )
        .map_err(|error| format!("the console could not be installed: {error}"))?;
    let answered = Arc::clone(capture);
    ctx.globals()
        .set(
            "__hostRespond",
            Func::from(move |status: i32, headers: String, body: String| {
                let mut capture = captured(&answered);
                if capture.response.is_some() {
                    // The response is the FIRST one the application ended: a
                    // second `end` on the same response changes nothing a client
                    // sees.
                    return;
                }
                let room = MAX_CAPTURED_BYTES.min(body.len());
                let mut end = room;
                while end > 0 && !body.is_char_boundary(end) {
                    end -= 1;
                }
                capture.response = Some(GuestResponse {
                    status: u16::try_from(status).unwrap_or(200),
                    headers: response_headers(&headers),
                    body: body.as_bytes()[..end].to_vec(),
                });
            }),
        )
        .map_err(|error| format!("the response channel could not be installed: {error}"))?;
    Ok(())
}

/// The headers the guest ended its response with, read back from the JSON object
/// the dispatch prelude collected them in.
fn response_headers(json: &str) -> Vec<(String, String)> {
    let Ok(::serde_json::Value::Object(map)) = ::serde_json::from_str::<::serde_json::Value>(json)
    else {
        return Vec::new();
    };
    map.into_iter()
        .filter_map(|(name, value)| {
            let text = match value {
                ::serde_json::Value::String(text) => text,
                ::serde_json::Value::Null => return None,
                other => other.to_string(),
            };
            Some((name, text))
        })
        .collect()
}

/// The runtime a deployed application starts on: the console it logs through,
/// the `process` it reads its environment from, and the `http` module a web
/// application's entry point creates its server with.
///
/// Written in JavaScript over the host functions installed beside it, so what an
/// application observes is the engine's own semantics. `require` answers only the
/// modules below; any other name throws the way a missing module does, rather
/// than resolving to something this emulator made up.
const RUNTIME_PRELUDE: &str = r#"
(function () {
    var text = function (value) {
        if (typeof value === "string") { return value; }
        if (value instanceof Error) { return value.stack ? String(value.stack) : String(value); }
        try {
            var json = JSON.stringify(value);
            return json === undefined ? String(value) : json;
        } catch (error) {
            return String(value);
        }
    };
    var line = function (args) {
        return Array.prototype.map.call(args, text).join(" ") + "\n";
    };
    var out = function () { __hostStdout(line(arguments)); };
    var err = function () { __hostStderr(line(arguments)); };
    globalThis.console = { log: out, info: out, debug: out, dir: out, warn: err, error: err, trace: err };

    globalThis.process = {
        env: globalThis.__ebEnv || {},
        argv: ["node", "app.js"],
        platform: "linux",
        pid: 1,
        stdout: { write: function (chunk) { __hostStdout(String(chunk)); return true; } },
        stderr: { write: function (chunk) { __hostStderr(String(chunk)); return true; } },
        on: function () { return globalThis.process; },
        exit: function () {},
        nextTick: function (callback) { if (typeof callback === "function") { callback(); } },
        cwd: function () { return "/var/app/current"; },
        hrtime: function () { return [0, 0]; },
    };

    var server = function (handler) {
        var instance = {
            listen: function () {
                var last = arguments[arguments.length - 1];
                if (typeof last === "function") { last(); }
                return instance;
            },
            on: function () { return instance; },
            once: function () { return instance; },
            close: function (callback) { if (typeof callback === "function") { callback(); } return instance; },
            address: function () { return { address: "127.0.0.1", family: "IPv4", port: 8080 }; },
            setTimeout: function () { return instance; },
            unref: function () { return instance; },
        };
        if (typeof handler === "function") { globalThis.__ebRequestHandler = handler; }
        return instance;
    };
    var http = {
        createServer: function (first, second) {
            return server(typeof first === "function" ? first : second);
        },
        Server: function (first, second) {
            return server(typeof first === "function" ? first : second);
        },
        STATUS_CODES: { "200": "OK", "404": "Not Found", "500": "Internal Server Error" },
    };
    var modules = {
        http: http,
        https: http,
        os: {
            hostname: function () { return "localhost"; },
            platform: function () { return "linux"; },
            EOL: "\n",
        },
        path: {
            sep: "/",
            join: function () { return Array.prototype.join.call(arguments, "/"); },
            basename: function (value) { var parts = String(value).split("/"); return parts[parts.length - 1]; },
        },
        url: {
            parse: function (value) {
                var text = String(value);
                var mark = text.indexOf("?");
                return mark < 0
                    ? { pathname: text, search: null, query: null, path: text }
                    : { pathname: text.slice(0, mark), search: text.slice(mark), query: text.slice(mark + 1), path: text };
            },
        },
        events: { EventEmitter: function () { return { on: function () { return this; }, emit: function () { return false; } }; } },
    };
    globalThis.require = function (name) {
        var wanted = String(name).replace(/^node:/, "");
        if (Object.prototype.hasOwnProperty.call(modules, wanted)) { return modules[wanted]; }
        throw new Error("Cannot find module '" + name + "'");
    };
    globalThis.module = { exports: {} };
    globalThis.exports = globalThis.module.exports;
})();
"#;

/// Hand the request in `__ebRequest` to the server the entry point started, and
/// send what it answers back over the host's response channel.
///
/// The `req`/`res` pair is the one Node.js hands an HTTP server's request
/// listener, as far as a request that is already fully read needs: the body is
/// delivered to a `data`/`end` listener the handler registers, the status is
/// whatever `writeHead` or `statusCode` set, and ending the response is what
/// sends it.
const DISPATCH_PRELUDE: &str = r#"
(function () {
    var handler = globalThis.__ebRequestHandler;
    if (typeof handler !== "function") { return false; }
    var incoming = globalThis.__ebRequest;
    var chunks = [];
    var headers = {};
    var written = null;
    var ended = false;
    var res = {
        statusCode: 200,
        statusMessage: "",
        headersSent: false,
        setHeader: function (name, value) { headers[String(name).toLowerCase()] = String(value); return res; },
        getHeader: function (name) { return headers[String(name).toLowerCase()]; },
        removeHeader: function (name) { delete headers[String(name).toLowerCase()]; return res; },
        writeHead: function (status, second, third) {
            written = status;
            var given = (second && typeof second === "object") ? second : third;
            if (given && typeof given === "object") {
                for (var name in given) {
                    if (Object.prototype.hasOwnProperty.call(given, name)) {
                        headers[String(name).toLowerCase()] = String(given[name]);
                    }
                }
            }
            res.headersSent = true;
            return res;
        },
        write: function (chunk) {
            if (chunk !== undefined && chunk !== null) { chunks.push(String(chunk)); }
            return true;
        },
        end: function (chunk) {
            if (chunk !== undefined && chunk !== null && typeof chunk !== "function") { chunks.push(String(chunk)); }
            if (ended) { return res; }
            ended = true;
            var status = written === null ? res.statusCode : written;
            __hostRespond(status, JSON.stringify(headers), chunks.join(""));
            return res;
        },
        on: function () { return res; },
        once: function () { return res; },
        setTimeout: function () { return res; },
    };
    var listeners = {};
    var req = {
        method: incoming.method,
        url: incoming.url,
        headers: incoming.headers,
        httpVersion: "1.1",
        socket: { remoteAddress: "127.0.0.1" },
        on: function (event, callback) {
            listeners[event] = callback;
            if (event === "end" && typeof callback === "function") { callback(); }
            return req;
        },
        once: function (event, callback) { return req.on(event, callback); },
        setEncoding: function () { return req; },
        resume: function () { return req; },
    };
    handler(req, res);
    return true;
})();
"#;
