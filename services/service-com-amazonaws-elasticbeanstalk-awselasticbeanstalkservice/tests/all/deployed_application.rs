// SPDX-License-Identifier: CC0-1.0

//! The APPLICATION an environment runs, over the service's own HTTP surface.
//!
//! The rule under test: an environment RUNS the application version deployed to
//! it, so the deployed code's own output is what the service reports — the log
//! lines it writes are the lines the compiled environment information carries
//! beside this service's activity log, and the response its own request handler
//! ends is what the address the environment publishes (`CNAME`/`EndpointURL`,
//! which is an address this emulator answers on) serves. Nothing about either is
//! synthesized: a bundle that starts no server of its own serves the FILES it
//! carries, and a bundle with neither serves nothing at all.
//!
//! The table spans what that rule generalises over: how the bundle names its
//! entry point (`app.js`, a `package.json` `main`), what the code does (logs,
//! answers a request, reads an environment property it was configured with,
//! answers a path-dependent body), a bundle with no runnable entry point at all
//! (static files only), and one with neither an entry point nor a document for
//! the path.
//!
//! The HTTP harness (the router, the form encoder, the XML element reader, the
//! object-storage upload and the unsigned GET) is the one `lifecycle.rs` already
//! owns in this same test binary.

use crate::application_version_processing::Bundle;
use crate::environment_info::entries;
use crate::lifecycle::{Answer, Emulator, PAST_DUE, SOLUTION_STACK, TEST_REGION};

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

/// The application every environment below is launched under.
const APPLICATION: &str = "deployed-app";

/// The bucket the suites upload a source bundle into, and the key under it.
const BUNDLE_BUCKET: &str = "eb-deployed-bundles";
const BUNDLE_KEY: &str = "bundles/app.zip";

/// The environment property the launches below configure, and its value: a
/// setting of `aws:elasticbeanstalk:application:environment` is an environment
/// property the application reads out of its own process environment.
const PROPERTY_NAME: &str = "GREETING";
const PROPERTY_VALUE: &str = "hello from the environment";

/// A marker the deployed code logs, carrying non-ASCII text so an encoding
/// regression anywhere in the log path is visible: the bytes the application
/// wrote must come back byte for byte.
const LOG_MARKER: &str = "started: naïve ✓ Ünicode marker ✔";

/// A Node.js application that logs [`LOG_MARKER`] while starting and answers
/// every request with the environment property it was configured with and the
/// path it was asked for.
const APP_JS: &[u8] = br#"
const http = require('http');
console.log('started: nai\u0308ve \u2713 U\u0308nicode marker \u2714'.normalize('NFC'));
http.createServer(function (req, res) {
  res.writeHead(200, { 'Content-Type': 'text/plain; charset=utf-8' });
  res.end(process.env.GREETING + ' at ' + req.url);
}).listen(process.env.PORT, function () {
  console.log('listening on ' + process.env.PORT);
});
"#;

/// The same application under a name only the bundle's `package.json` names.
const MAIN_JS: &[u8] = br#"
const http = require('http');
console.log('started: nai\u0308ve \u2713 U\u0308nicode marker \u2714'.normalize('NFC'));
http.createServer((req, res) => {
  res.statusCode = 200;
  res.setHeader('content-type', 'text/plain; charset=utf-8');
  res.write(process.env.GREETING);
  res.end(' at ' + req.url);
}).listen(Number(process.env.PORT));
"#;

/// A `package.json` whose `main` names that file.
const PACKAGE_JSON: &[u8] = br#"{"name":"demo","version":"1.0.0","main":"./src/main.js"}"#;

/// A document a bundle with no entry point serves its root from.
const INDEX_HTML: &[u8] = b"<!doctype html><title>deployed</title><p>static content\n";

/// Code that never finishes starting: an endless loop before anything is served.
const ENDLESS_LOOP_JS: &[u8] = b"while (true) { }\n";

/// Code with no base case, which the engine's stack bound is what stops.
const ENDLESS_RECURSION_JS: &[u8] = b"function down(n) { return down(n + 1); }\ndown(0);\n";

/// Code that writes without end, which the cap on captured output is what stops.
const ENDLESS_OUTPUT_JS: &[u8] =
    b"for (var i = 0; ; i++) { console.log('flood ' + i + ' ----------------'); }\n";

/// One bundle per way deployed code can fail to finish.
const ENDLESS_LOOP_BUNDLE: &[(&str, &[u8])] = &[("app.js", ENDLESS_LOOP_JS)];
/// See [`ENDLESS_LOOP_BUNDLE`].
const ENDLESS_RECURSION_BUNDLE: &[(&str, &[u8])] = &[("app.js", ENDLESS_RECURSION_JS)];
/// See [`ENDLESS_LOOP_BUNDLE`].
const ENDLESS_OUTPUT_BUNDLE: &[(&str, &[u8])] = &[("app.js", ENDLESS_OUTPUT_JS)];

/// How long a request to an environment running code that never finishes may
/// take before this suite calls it unbounded. Well above the engine's own
/// wall-clock bound, so a loaded machine does not fail the suite, and well below
/// a hang.
const BOUNDED_RUN: std::time::Duration = std::time::Duration::from_secs(30);

/// How much of an answer this suite accepts as bounded: the engine caps what one
/// run captures, so neither the compiled log nor the response can grow with the
/// code that wrote it.
const BOUNDED_BYTES: usize = 1024 * 1024;

/// One deployed bundle, and what the environment running it must then report.
struct Row {
    case: &'static str,
    bundle: Bundle,
    /// The status a GET of the environment's own address must answer.
    status: u16,
    /// What that answer's body must carry.
    body: Option<&'static str>,
    /// Whether the compiled environment information must carry the
    /// application's own log line.
    logs: bool,
}

/// One row per thing the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an app.js that logs and serves",
            bundle: Bundle::Zip(&[("app.js", APP_JS)]),
            status: 200,
            body: Some(PROPERTY_VALUE),
            logs: true,
        },
        Row {
            case: "an entry point named by package.json main",
            bundle: Bundle::Zip(&[
                ("package.json", PACKAGE_JSON),
                ("src/main.js", MAIN_JS),
                ("index.html", INDEX_HTML),
            ]),
            status: 200,
            body: Some(PROPERTY_VALUE),
            logs: true,
        },
        Row {
            case: "a bundle of static files only",
            bundle: Bundle::Zip(&[("index.html", INDEX_HTML)]),
            status: 200,
            body: Some("static content"),
            logs: false,
        },
        Row {
            case: "a bundle with neither an entry point nor a root document",
            bundle: Bundle::Zip(&[("README.txt", b"nothing runnable here\n")]),
            status: 404,
            body: None,
            logs: false,
        },
    ]
}

/// The address an environment with the CNAME prefix `prefix` publishes, which is
/// the address a client dials: the socket this emulator serves the environment
/// plane on, then the environment's Region and prefix in the path.
fn published_address(emulator: &Emulator, prefix: &str) -> String {
    emulator.environment_address(TEST_REGION, prefix)
}

/// The URL this suite DIALS for `prefix` and `path` — the published address,
/// which is the address a client dials as it was given it.
fn dialled(emulator: &Emulator, prefix: &str, path: &str) -> String {
    format!("http://{}{path}", published_address(emulator, prefix))
}

/// Call `action`, failing the named case unless it answered 200.
async fn ok(emulator: &Emulator, case: &str, action: &str, params: &[(&str, &str)]) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// Upload `bundle`, create the application and a version from it, launch an
/// environment deploying that version with the environment property configured,
/// and wait out the launch the clock owes.
async fn deployed(emulator: &Emulator, case: &str, prefix: &str, bundle: Bundle) {
    emulator
        .put_object(BUNDLE_BUCKET, BUNDLE_KEY, &bundle.bytes())
        .await;
    ok(
        emulator,
        case,
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
    ok(
        emulator,
        case,
        "CreateApplicationVersion",
        &[
            ("ApplicationName", APPLICATION),
            ("VersionLabel", "v1"),
            ("SourceBundle.S3Bucket", BUNDLE_BUCKET),
            ("SourceBundle.S3Key", BUNDLE_KEY),
        ],
    )
    .await;
    let launched = ok(
        emulator,
        case,
        "CreateEnvironment",
        &[
            ("ApplicationName", APPLICATION),
            ("EnvironmentName", "deployed-env-1"),
            ("CNAMEPrefix", prefix),
            ("SolutionStackName", SOLUTION_STACK.as_str()),
            ("VersionLabel", "v1"),
            (
                "OptionSettings.member.1.Namespace",
                eb::state::NAMESPACE_APPLICATION_ENVIRONMENT,
            ),
            ("OptionSettings.member.1.OptionName", PROPERTY_NAME),
            ("OptionSettings.member.1.Value", PROPERTY_VALUE),
        ],
    )
    .await;
    let address = published_address(emulator, prefix);
    assert_eq!(
        launched.expect("CNAME", case),
        address,
        "case {case:?}: the environment must publish the address it is served at, got body {}",
        launched.body
    );
    assert_eq!(
        launched.expect("EndpointURL", case),
        address,
        "case {case:?}: the EndpointURL must be that same address, got body {}",
        launched.body
    );
    std::thread::sleep(PAST_DUE);
}

/// RULE: the address an environment publishes is served by the application the
/// deployed version carries — the response its own handler ends, or the file of
/// its bundle the request addresses — and serving it changes nothing a later
/// caller sees.
#[tokio::test]
async fn the_published_address_is_served_by_the_deployed_application() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::with_object_storage();
        deployed(&emulator, case, "deployed-env-1", row.bundle).await;

        let url = dialled(&emulator, "deployed-env-1", "/");
        let served = emulator.get_url(&url).await;
        assert_eq!(
            served.status, row.status,
            "case {case:?}: GET {url} must answer {}, got {} and body {}",
            row.status, served.status, served.body
        );
        if let Some(expected) = row.body {
            assert!(
                served.body.contains(expected),
                "case {case:?}: the answer must carry what the deployed bundle produces \
                 ({expected:?}), got {:?}",
                served.body
            );
        }
        // A read changes nothing a later caller sees: the same request is
        // answered the same way.
        let again = emulator.get_url(&url).await;
        assert_eq!(
            again.body, served.body,
            "case {case:?}: two reads of the address must answer alike, got {:?} then {:?}",
            served.body, again.body
        );
        assert_eq!(
            again.status, served.status,
            "case {case:?}: two reads of the address must answer the same status, got {} then {}",
            served.status, again.status
        );
    }
}

/// RULE: the compiled environment information carries the deployed
/// application's own log lines, byte for byte, beside this service's activity
/// log — and carries only the activity log for an environment running no
/// application of its own.
#[tokio::test]
async fn the_compiled_information_carries_the_applications_own_log() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::with_object_storage();
        deployed(&emulator, case, "deployed-env-1", row.bundle).await;

        ok(
            &emulator,
            case,
            "RequestEnvironmentInfo",
            &[
                ("EnvironmentName", "deployed-env-1"),
                ("InfoType", eb::state::ENVIRONMENT_INFO_TYPE_TAIL),
            ],
        )
        .await;
        std::thread::sleep(PAST_DUE);
        let retrieved = ok(
            &emulator,
            case,
            "RetrieveEnvironmentInfo",
            &[
                ("EnvironmentName", "deployed-env-1"),
                ("InfoType", eb::state::ENVIRONMENT_INFO_TYPE_TAIL),
            ],
        )
        .await;
        let compiled = entries(&retrieved);
        let entry = compiled
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected an entry, got {compiled:?}"));
        let downloaded = emulator.get_url(&entry.message).await;
        assert_eq!(
            downloaded.status, 200,
            "case {case:?}: the compiled file must download, got {} and body {}",
            downloaded.status, downloaded.body
        );
        // This service's own activity is in the compiled information whatever
        // the environment runs.
        assert!(
            downloaded.body.contains("createEnvironment"),
            "case {case:?}: the compiled file must carry the activity log, got {:?}",
            downloaded.body
        );
        assert_eq!(
            downloaded.body.contains(LOG_MARKER),
            row.logs,
            "case {case:?}: the application's own log line ({LOG_MARKER:?}) must be carried \
             exactly when the bundle runs one, got {:?}",
            downloaded.body
        );
        assert_eq!(
            downloaded.body.contains(eb::state::APPLICATION_STDOUT_LOG),
            row.logs,
            "case {case:?}: the application's log file is named exactly when there is one, \
             got {:?}",
            downloaded.body
        );
    }
}

/// RULE: only a LIVE environment's address is served. An address no environment
/// carries is served by nothing, and an environment the clock has terminated
/// ("Terminated: Environment is not running") stops serving the address it held.
#[tokio::test]
async fn an_address_is_served_only_while_an_environment_carries_it() {
    let case = "terminated";
    let emulator = Emulator::with_object_storage();
    deployed(
        &emulator,
        case,
        "deployed-env-1",
        Bundle::Zip(&[("app.js", APP_JS)]),
    )
    .await;
    let url = dialled(&emulator, "deployed-env-1", "/");

    // An address no environment ever took.
    let unknown = dialled(&emulator, "never-taken", "/");
    let missing = emulator.get_url(&unknown).await;
    assert_eq!(
        missing.status, 404,
        "case {case:?}: an address nothing carries must not be served, got {} and body {}",
        missing.status, missing.body
    );

    let live = emulator.get_url(&url).await;
    assert_eq!(
        live.status, 200,
        "case {case:?}: the live environment's address must be served, got {} and body {}",
        live.status, live.body
    );

    ok(
        &emulator,
        case,
        "TerminateEnvironment",
        &[("EnvironmentName", "deployed-env-1")],
    )
    .await;
    std::thread::sleep(PAST_DUE);
    let terminated = emulator.get_url(&url).await;
    assert_eq!(
        terminated.status, 404,
        "case {case:?}: a terminated environment must stop serving its address, got {} and \
         body {}",
        terminated.status, terminated.body
    );
}

/// RULE: a request's own path and method reach the application: the response is
/// computed by the deployed code from the request it was handed, so a different
/// path is answered differently.
#[tokio::test]
async fn the_application_answers_the_request_it_was_handed() {
    let case = "request path";
    let emulator = Emulator::with_object_storage();
    deployed(
        &emulator,
        case,
        "deployed-env-1",
        Bundle::Zip(&[("app.js", APP_JS)]),
    )
    .await;

    // One row per path the client asks for, including a query string, which
    // Node.js carries on `req.url`.
    for path in ["/", "/status", "/items?page=2"] {
        let url = dialled(&emulator, "deployed-env-1", path);
        let served = emulator.get_url(&url).await;
        assert_eq!(
            served.status, 200,
            "case {case:?} {path:?}: expected 200, got {} and body {}",
            served.status, served.body
        );
        assert!(
            served.body.contains(&format!("at {path}")),
            "case {case:?} {path:?}: the application must answer from the request it was \
             handed, got {:?}",
            served.body
        );
    }
}

/// RULE: the engine the deployed code runs on is BOUNDED before anything is
/// evaluated — a step budget, a wall-clock deadline, a stack bound and a cap on
/// what one run captures — so code that never finishes is STOPPED and reported as
/// an application that answered nothing, rather than hanging the request it was
/// handed or growing what it wrote without end.
///
/// Every assertion here is reached only because the run was stopped: a run that
/// was not would never answer at all.
#[tokio::test]
async fn code_that_never_finishes_is_stopped_and_reported() {
    // One row per way deployed code can fail to finish: an endless loop (the
    // step budget and the deadline), recursion with no base case (the stack
    // bound), and output with no end (the cap on what is captured).
    let rows: &[(&str, Bundle)] = &[
        ("an endless loop", Bundle::Zip(ENDLESS_LOOP_BUNDLE)),
        (
            "recursion with no base case",
            Bundle::Zip(ENDLESS_RECURSION_BUNDLE),
        ),
        ("output with no end", Bundle::Zip(ENDLESS_OUTPUT_BUNDLE)),
    ];
    for (case, bundle) in rows {
        let emulator = Emulator::with_object_storage();
        deployed(&emulator, case, "deployed-env-1", *bundle).await;

        // The address answers: the run was stopped, and nothing it never
        // produced is reported as a response.
        let started = std::time::Instant::now();
        let served = emulator
            .get_url(&dialled(&emulator, "deployed-env-1", "/"))
            .await;
        let elapsed = started.elapsed();
        assert!(
            elapsed < BOUNDED_RUN,
            "case {case:?}: the request must be answered inside the engine's own bounds, \
             took {elapsed:?}"
        );
        assert_ne!(
            served.status, 200,
            "case {case:?}: code that never answered a request must not produce a success, \
             got body {:?}",
            served.body
        );
        assert!(
            served.body.is_empty(),
            "case {case:?}: no body is invented for an application that answered nothing, \
             got {:?}",
            served.body
        );

        // And the information compiled from such an environment is bounded too:
        // what the run wrote before it was stopped, and no more.
        ok(
            &emulator,
            case,
            "RequestEnvironmentInfo",
            &[
                ("EnvironmentName", "deployed-env-1"),
                ("InfoType", eb::state::ENVIRONMENT_INFO_TYPE_TAIL),
            ],
        )
        .await;
        std::thread::sleep(PAST_DUE);
        let retrieved = ok(
            &emulator,
            case,
            "RetrieveEnvironmentInfo",
            &[
                ("EnvironmentName", "deployed-env-1"),
                ("InfoType", eb::state::ENVIRONMENT_INFO_TYPE_TAIL),
            ],
        )
        .await;
        let compiled = entries(&retrieved);
        let entry = compiled
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected an entry, got {compiled:?}"));
        let downloaded = emulator.get_url(&entry.message).await;
        assert_eq!(
            downloaded.status, 200,
            "case {case:?}: the compiled file must download, got {} and body {}",
            downloaded.status, downloaded.body
        );
        assert!(
            downloaded.body.len() < BOUNDED_BYTES,
            "case {case:?}: the compiled log must be bounded by what one run may capture, \
             got {} bytes",
            downloaded.body.len()
        );
        assert!(
            downloaded.body.contains("createEnvironment"),
            "case {case:?}: the compiled file must still carry the activity log, got {} bytes",
            downloaded.body.len()
        );
    }
}
