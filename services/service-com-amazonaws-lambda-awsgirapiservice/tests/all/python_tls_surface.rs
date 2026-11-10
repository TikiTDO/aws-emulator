// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TLS LAYER of the standard library a `python3.x` function's run
//! is given: `CreateFunction` then `Invoke` (`POST
//! /2015-03-31/functions/{FunctionName}/invocations`) over the router.
//!
//! The rule under test: the Python a function runs on presents the standard library of the
//! runtime it advertises, so a handler module whose body imports one of those modules
//! IMPORTS CLEANLY and the invocation answers the handler's own value with no
//! `X-Amz-Function-Error`; a capability the runtime cannot serve is refused by the CALL that
//! needs it, with the module's own error for it, and never by the import. For `ssl` that
//! means the names, the error classes, the contexts a client configures and the certificate
//! encodings are the module's own, while a negotiated SESSION — which needs a connection
//! this runtime opens none of — raises `ssl.SSLError`, and a certificate file of the HOST
//! raises the one file refusal a function has.
//!
//! Every row drives real HTTP requests through the router, and the rows differ along what
//! the rule generalises over: which module of the standard library is imported, which
//! handler module name the package carries, which surface of `ssl` is read (names, errors,
//! contexts, encodings, buffers), which call needs a session, and what a certificate's
//! bytes are.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::{is_host_bound, one_invocation_stream_at_a_time};
use crate::node_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The `Timeout` every function this suite creates carries, in seconds. Sized the way the
/// sibling suites that RUN handlers size theirs: it has to cover this test binary's own
/// scheduling, not the handlers' work, which is immediate.
const FUNCTION_TIMEOUT_SECONDS: i32 = 25;

/// How many times a row re-drives its invocation while the answer reports one of this test
/// binary's own host bounds rather than a run.
const HOST_BOUND_ATTEMPTS: usize = 4;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Create a `python3.12` function whose package holds `source` as `<module>.py`, configured
/// with `<module>.handler`, and wait out the provisioning window the create documents.
async fn create_function(router: &MultiServiceRouter, name: &str, module: &str, source: &str) {
    let entry = format!("{module}.py");
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": format!("{module}.handler"),
            "Timeout": FUNCTION_TIMEOUT_SECONDS,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&[(entry.as_str(), source)])),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` on `name` with `event`, re-driven while the answer reports a bound this whole
/// test binary shares rather than a run.
async fn invoke(router: &MultiServiceRouter, name: &str, event: &Value) -> Answer {
    let mut answered = invoke_once(router, name, event).await;
    for _ in 1..HOST_BOUND_ATTEMPTS {
        if !is_host_bound(answered.status, &answered.text()) {
            break;
        }
        answered = invoke_once(router, name, event).await;
    }
    answered
}

/// One `Invoke` request on the function `name` names.
async fn invoke_once(router: &MultiServiceRouter, name: &str, event: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &serde_json::to_vec(event).expect("the event serializes"),
    )
    .await
}

/// What one case of the probe answered, or `None` when the run was ended by a bound this
/// whole test binary shares rather than by anything the case asked for.
async fn answered(router: &MultiServiceRouter, name: &str, case: &str, event: &Value) -> Option<Value> {
    let answer = invoke(router, name, event).await;
    if is_host_bound(answer.status, &answer.text()) {
        return None;
    }
    assert_eq!(
        answer.status,
        200,
        "case {case}: expected 200, got {} {}",
        answer.status,
        answer.text()
    );
    let failure = answer.header("x-amz-function-error");
    assert_eq!(
        failure, None,
        "case {case}: the handler's module imports cleanly and the handler catches each \
         refusal itself, so the invocation carries no function error, got {failure:?} with {}",
        answer.text()
    );
    Some(answer.json())
}

/// The whole rule, driven END TO END as one test: every table below is a real
/// `CreateFunction` and real `Invoke` requests over the router.
///
/// One test and one stream of invocations, the way the sibling runtime suites are built: the
/// memory bound a run is stopped on is read from the WHOLE process
/// (`state::execution::python`) and this binary runs its suites in parallel, so fewer
/// interpreters in flight at once is fewer runs stopped for memory belonging to another
/// suite. A case whose run was stopped by one of those shared bounds says nothing about the
/// rule and is left out of the reading — but not every one of them can be: each table
/// answers how many of its cases a RUN answered, and a test that read none of them at all
/// is a test that asserted nothing, so it fails rather than passing silently.
#[tokio::test]
async fn the_python_runtime_presents_the_tls_layer_of_its_standard_library() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let mut read = a_module_that_imports_the_tls_layer_imports_cleanly(&router).await;
    let probe = "probe-tls-surface";
    create_function(&router, probe, "lambda_function", PROBE_SOURCE).await;
    read += the_modules_names_are_the_runtimes_own(&router, probe).await;
    read += a_context_holds_what_a_client_configures(&router, probe).await;
    read += a_certificate_store_takes_data_and_refuses_a_host_file(&router, probe).await;
    read += a_certificate_encoding_is_computed(&router, probe).await;
    read += a_call_that_needs_a_session_is_refused_as_the_modules_own_error(&router, probe).await;
    read += every_standard_library_module_built_on_the_tls_layer_imports(&router, probe).await;
    assert!(
        read > 0,
        "expected at least one case of the tables above to be answered by a run, got \
         {read}: every invocation reported a bound this whole test binary shares, so nothing \
         of the rule was read"
    );
}

/// The recorded shape, and the generalisation of it: a handler module whose FIRST statement
/// imports the socket interface and the TLS layer imports cleanly and its own value is the
/// answer — whatever the module is named and whichever of the two modules it reaches for.
///
/// The rows differ along the handler module's name (so the import is resolved for a package
/// entry of any name, not only the one the sibling suites upload) and along which of the
/// modules the body imports.
///
/// Answers how many rows a run answered.
async fn a_module_that_imports_the_tls_layer_imports_cleanly(router: &MultiServiceRouter) -> usize {
    // The case, the handler module's name, and the module body.
    let rows: &[(&str, &str, &str)] = &[
        (
            "the recorded probe's own shape: import socket, ssl",
            "index",
            "import socket, ssl\n\n\ndef handler(event, context):\n    return {'ok': True}\n",
        ),
        (
            "a module that imports the TLS layer alone",
            "probe",
            "import ssl\n\n\ndef handler(event, context):\n    return {'ok': True}\n",
        ),
        (
            "a module that imports it under a name of its own",
            "cache_probe",
            "import ssl as tls\n\n\ndef handler(event, context):\n    \
                 return {'ok': isinstance(tls.create_default_context(), tls.SSLContext)}\n",
        ),
        (
            "a module that imports a name out of it",
            "session_probe",
            "from ssl import SSLError, create_default_context\n\n\n\
             def handler(event, context):\n    \
                 return {'ok': issubclass(SSLError, OSError)\n            \
                               and create_default_context().check_hostname}\n",
        ),
    ];
    let mut read = 0_usize;
    for (case, module, source) in rows {
        let name = format!("probe-tls-import-{module}");
        create_function(router, &name, module, source).await;
        let Some(document) = answered(router, &name, case, &json!({})).await else {
            continue;
        };
        read += 1;
        assert_eq!(
            document,
            json!({"ok": true}),
            "case {case}: expected the handler's own value, got {document}"
        );
    }
    read
}

/// The names a program reads off the module are the ones the standard library states.
///
/// Source of each expected value: the `ssl` module's own documented constants — the
/// verification modes, the protocol selector `PROTOCOL_TLS_CLIENT`, the `TLSVersion`
/// members (the protocol version numbers TLS itself assigns), the error-number constants,
/// and the extended-key-usage object identifier a purpose names. The last row is a name the
/// advertised runtime does NOT have: `ssl.wrap_socket` and `ssl.match_hostname` were removed
/// in Python 3.12, so a runtime presenting 3.12 must not carry them.
///
/// Answers how many cases a run answered.
async fn the_modules_names_are_the_runtimes_own(router: &MultiServiceRouter, probe: &str) -> usize {
    let case = "the module's names";
    let Some(document) = answered(router, probe, case, &json!({"case": "names"})).await else {
        return 0;
    };
    let expected = json!({
        "cert_none": 0,
        "cert_optional": 1,
        "cert_required": 2,
        "protocol_tls_client": 16,
        "protocol_tls_server": 17,
        "tls_v1_2": 771,
        "tls_v1_3": 772,
        "minimum_supported": -2,
        "maximum_supported": -1,
        "want_read": 2,
        "zero_return": 6,
        "server_auth_oid": "1.3.6.1.5.5.7.3.1",
        "client_auth_oid": "1.3.6.1.5.5.7.3.2",
        "error_is_os_error": true,
        "verification_error_is_value_error": true,
        "certificate_error_is_the_verification_error": true,
        "want_read_error_is_an_ssl_error": true,
        "has_sni": true,
        "removed_in_3_12": [],
    });
    assert_eq!(
        document, expected,
        "case {case}: expected the standard library's own names, got {document}"
    );
    1
}

/// A context holds what a client configures on it, and keeps the language's own refusal of
/// an inconsistent pair.
///
/// The rows differ along which context is built (the default one for a server's
/// authentication, the unverified one, the one a server authenticates clients with) and
/// along which setting is written.
///
/// Answers how many cases a run answered.
async fn a_context_holds_what_a_client_configures(
    router: &MultiServiceRouter,
    probe: &str,
) -> usize {
    let case = "a context's own settings";
    let Some(document) = answered(router, probe, case, &json!({"case": "contexts"})).await else {
        return 0;
    };
    let expected = json!({
        // `create_default_context()` verifies the peer and checks the host name.
        "default_check_hostname": true,
        "default_verify_mode": 2,
        "default_protocol": 16,
        // `_create_unverified_context()` does neither.
        "unverified_check_hostname": false,
        "unverified_verify_mode": 0,
        // A context built to authenticate clients is the server side of the same settings.
        "client_auth_protocol": 17,
        "client_auth_check_hostname": false,
        // What a caller writes is what it reads back.
        "minimum_version": 771,
        "maximum_version": 772,
        "option_set": true,
        "alpn": ["h2", "http/1.1"],
        "ciphers": "ECDHE+AESGCM",
        // Asking for the host-name check asks for the certificate it is checked against.
        "promoted_verify_mode": 2,
        // Turning verification off while the host name is checked is refused, as is
        // wrapping a socket with no host name to check.
        "verify_none_under_check_hostname": "ValueError",
        "no_server_hostname": "ValueError",
        // The suites a session would be negotiated with are not the module's to state, so
        // the read says so rather than answering "no cipher is enabled".
        "get_ciphers": "SSLError",
    });
    assert_eq!(
        document, expected,
        "case {case}: expected the settings a client configured, got {document}"
    );
    1
}

/// The certificate store takes certificate DATA and refuses a certificate FILE of the host,
/// which is the one file refusal a function has.
///
/// Answers how many cases a run answered.
async fn a_certificate_store_takes_data_and_refuses_a_host_file(
    router: &MultiServiceRouter,
    probe: &str,
) -> usize {
    let case = "the certificate store";
    let Some(document) = answered(router, probe, case, &json!({"case": "store"})).await else {
        return 0;
    };
    let expected = json!({
        // Two certificates passed as data are two certificates in the store, read back as
        // the bytes they carry.
        "loaded": 2,
        "store_stats": 2,
        "read_back": [3, 5],
        // Nothing to load at all is the language's own refusal.
        "nothing_given": "TypeError",
        // A path names a file of the host, which no function opens.
        "cafile": "OSError",
        "capath": "OSError",
        "cert_chain": "OSError",
        // Data that carries no certificate is the module's own error.
        "not_a_certificate": "SSLError",
        // A store nothing was loaded into holds nothing.
        "empty_store": [],
    });
    assert_eq!(
        document, expected,
        "case {case}: expected the loaded store, got {document}"
    );
    1
}

/// The encodings a certificate is carried in are COMPUTED: every certificate the event
/// names makes the PEM round trip, and a PEM text the framing does not admit is refused.
///
/// The rows differ along the certificate's bytes — their length, their byte values and
/// whether the encoding needs padding — because the encoding is a property of neither.
///
/// Answers how many cases a run answered.
async fn a_certificate_encoding_is_computed(router: &MultiServiceRouter, probe: &str) -> usize {
    let case = "the certificate encodings";
    // One row per shape of certificate bytes: the shortest there is, one whose length is a
    // multiple of three (so the encoding pads nothing), one that needs one pad byte, one
    // that needs two, one spanning every byte value, and one long enough to be encoded over
    // several lines.
    let certificates: Vec<Vec<u8>> = vec![
        vec![0x00],
        vec![0x30, 0x82, 0x01],
        vec![0x30, 0x82, 0x01, 0x0a],
        vec![0x30, 0x82, 0x01, 0x0a, 0xff],
        (0..=255_u8).collect(),
        vec![0xa5; 300],
    ];
    let encoded: Vec<String> = certificates
        .iter()
        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
        .collect();
    let event = json!({"case": "encodings", "certificates": encoded});
    let Some(document) = answered(router, probe, case, &event).await else {
        return 0;
    };
    let round_tripped = document
        .get("round_tripped")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        round_tripped.len(),
        encoded.len(),
        "case {case}: expected one answer per certificate, got {document}"
    );
    for (index, (expected, actual)) in encoded.iter().zip(round_tripped.iter()).enumerate() {
        let actual = actual.as_str().unwrap_or("<not reported>");
        assert_eq!(
            actual, expected,
            "case {case} certificate {index}: the bytes a PEM carries are the bytes it was \
             built from, expected {expected:?}, got {actual:?}"
        );
    }
    let framed = document.get("framed").and_then(Value::as_array).cloned().unwrap_or_default();
    assert!(
        framed.iter().all(|held| held == &json!(true)),
        "case {case}: every PEM text carries the certificate framing, got {document}"
    );
    for (row, expected) in [("no_header", "ValueError"), ("no_footer", "ValueError")] {
        let actual = document
            .get(row)
            .and_then(Value::as_str)
            .unwrap_or("<not reported>");
        assert_eq!(
            actual, expected,
            "case {case} {row}: a PEM text the framing does not admit is refused, expected \
             {expected}, got {actual:?}"
        );
    }
    // The certificate TIME format, which is computation too: the documented example of the
    // `ssl` module's own converter, and a two-digit day beside it.
    let times = document.get("times").cloned().unwrap_or(Value::Null);
    assert_eq!(
        times,
        json!([1_515_144_883_i64, 1_767_225_539_i64]),
        "case {case}: expected the seconds the certificate times state, got {times}"
    );
    1
}

/// A call that needs a negotiated session is refused as the module's own error, and the
/// sockets and buffers under it keep answering what they are.
///
/// The rows differ along WHICH call needs the session: the handshake itself, a write, a
/// read, the peer's certificate, the channel binding, a handshake over memory buffers, and
/// the two calls that would open the connection a session runs on.
///
/// Answers how many cases a run answered.
async fn a_call_that_needs_a_session_is_refused_as_the_modules_own_error(
    router: &MultiServiceRouter,
    probe: &str,
) -> usize {
    let case = "a call that needs a session";
    let Some(document) = answered(router, probe, case, &json!({"case": "session"})).await else {
        return 0;
    };
    let expected = json!({
        // Wrapping an unconnected socket negotiates nothing, so it is the handshake and
        // every call through the session that raise the module's own error.
        "wrapped": true,
        "handshake": "SSLError",
        "write": "SSLError",
        "read": "SSLError",
        "unwrap": "SSLError",
        "bio_handshake": "SSLError",
        "peer_certificate": "ValueError",
        "channel_binding": "ValueError",
        // The connection a session would run on: a function opens none.
        "connect": "OSError",
        "create_connection": "OSError",
        "server_certificate": "OSError",
        // Nothing was negotiated, so nothing of a session is reported.
        "version": null,
        "cipher": null,
        "selected_alpn": null,
        "session_reused": null,
        // The buffers the layer would read and write through are real buffers.
        "bio_pending": 5,
        "bio_read": "aGVsbG8=",
        "bio_eof": true,
    });
    assert_eq!(
        document, expected,
        "case {case}: expected the refusal of a session and the surface around it, got \
         {document}"
    );
    1
}

/// The generalisation past `ssl` itself: every module of the standard library that is built
/// on the TLS layer imports cleanly too, because the layer is on the import path.
///
/// Source: the modules of the CPython standard library whose own body imports `ssl` and
/// whose other imports this runtime registers — the protocol clients (`ftplib`, `poplib`,
/// `smtplib`), the HTTP client and the URL opener built on it.
///
/// Answers how many cases a run answered.
async fn every_standard_library_module_built_on_the_tls_layer_imports(
    router: &MultiServiceRouter,
    probe: &str,
) -> usize {
    let case = "the modules built on the TLS layer";
    let modules = ["ssl", "ftplib", "poplib", "smtplib", "http.client", "urllib.request"];
    let event = json!({"case": "modules", "modules": modules});
    let Some(document) = answered(router, probe, case, &event).await else {
        return 0;
    };
    for module in modules {
        let outcome = document
            .get(module)
            .and_then(Value::as_str)
            .unwrap_or("<not reported>");
        assert_eq!(
            outcome, "",
            "case {case} {module}: expected the module to import cleanly, got {outcome:?}"
        );
    }
    1
}

/// The probe every table above drives: its module body imports the socket interface and the
/// TLS layer, exactly as a recorded in-VPC probe's does, and its handler answers the surface
/// the event names.
const PROBE_SOURCE: &str = r#"
import base64
import importlib
import socket
import ssl

# The certificates the store table loads, as PEM text: two certificates in one value, whose
# bytes are three and five long, so what is read back out of the store is checkable.
_FIRST = ssl.DER_cert_to_PEM_cert(b"\x01\x02\x03")
_SECOND = ssl.DER_cert_to_PEM_cert(b"\x04\x05\x06\x07\x08")


def _refused(call):
    """The name of the error `call` raised, or an empty string when it raised none."""
    try:
        call()
    except BaseException as error:
        return type(error).__name__
    return ""


def _names(event):
    removed = [name for name in ("wrap_socket", "match_hostname") if hasattr(ssl, name)]
    return {
        "cert_none": int(ssl.CERT_NONE),
        "cert_optional": int(ssl.CERT_OPTIONAL),
        "cert_required": int(ssl.CERT_REQUIRED),
        "protocol_tls_client": int(ssl.PROTOCOL_TLS_CLIENT),
        "protocol_tls_server": int(ssl.PROTOCOL_TLS_SERVER),
        "tls_v1_2": int(ssl.TLSVersion.TLSv1_2),
        "tls_v1_3": int(ssl.TLSVersion.TLSv1_3),
        "minimum_supported": int(ssl.TLSVersion.MINIMUM_SUPPORTED),
        "maximum_supported": int(ssl.TLSVersion.MAXIMUM_SUPPORTED),
        "want_read": int(ssl.SSL_ERROR_WANT_READ),
        "zero_return": int(ssl.SSL_ERROR_ZERO_RETURN),
        "server_auth_oid": ssl.Purpose.SERVER_AUTH.oid,
        "client_auth_oid": ssl.Purpose.CLIENT_AUTH.oid,
        "error_is_os_error": issubclass(ssl.SSLError, OSError),
        "verification_error_is_value_error": issubclass(
            ssl.SSLCertVerificationError, (ssl.SSLError, ValueError)
        ),
        "certificate_error_is_the_verification_error": (
            ssl.CertificateError is ssl.SSLCertVerificationError
        ),
        "want_read_error_is_an_ssl_error": issubclass(ssl.SSLWantReadError, ssl.SSLError),
        "has_sni": bool(ssl.HAS_SNI),
        "removed_in_3_12": removed,
    }


def _contexts(event):
    default = ssl.create_default_context()
    unverified = ssl._create_unverified_context()
    client_auth = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
    configured = ssl.create_default_context()
    configured.minimum_version = ssl.TLSVersion.TLSv1_2
    configured.maximum_version = ssl.TLSVersion.TLSv1_3
    configured.options |= ssl.OP_NO_TLSv1_1
    configured.set_alpn_protocols(["h2", "http/1.1"])
    configured.set_ciphers("ECDHE+AESGCM")
    promoted = ssl._create_unverified_context()
    promoted.check_hostname = True

    def verify_none():
        default.verify_mode = ssl.CERT_NONE

    def no_hostname():
        default.wrap_socket(socket.socket())

    return {
        "default_check_hostname": default.check_hostname,
        "default_verify_mode": int(default.verify_mode),
        "default_protocol": int(default.protocol),
        "unverified_check_hostname": unverified.check_hostname,
        "unverified_verify_mode": int(unverified.verify_mode),
        "client_auth_protocol": int(client_auth.protocol),
        "client_auth_check_hostname": client_auth.check_hostname,
        "minimum_version": int(configured.minimum_version),
        "maximum_version": int(configured.maximum_version),
        "option_set": bool(configured.options & ssl.OP_NO_TLSv1_1),
        "alpn": list(configured._alpn_protocols),
        "ciphers": configured._ciphers,
        "promoted_verify_mode": int(promoted.verify_mode),
        "verify_none_under_check_hostname": _refused(verify_none),
        "no_server_hostname": _refused(no_hostname),
        "get_ciphers": _refused(configured.get_ciphers),
    }


def _store(event):
    context = ssl.create_default_context()
    context.load_verify_locations(cadata=_FIRST + _SECOND)
    empty = ssl.create_default_context()
    return {
        "loaded": len(context.get_ca_certs(binary_form=True)),
        "store_stats": context.cert_store_stats()["x509"],
        "read_back": [len(held) for held in context.get_ca_certs(binary_form=True)],
        "nothing_given": _refused(lambda: context.load_verify_locations()),
        "cafile": _refused(
            lambda: context.load_verify_locations(cafile="/etc/ssl/certs/ca-bundle.crt")
        ),
        "capath": _refused(lambda: context.load_verify_locations(capath="/etc/ssl/certs")),
        "cert_chain": _refused(lambda: context.load_cert_chain("/var/task/client.pem")),
        "not_a_certificate": _refused(lambda: context.load_verify_locations(cadata="nope")),
        "empty_store": empty.get_ca_certs(),
    }


def _encodings(event):
    round_tripped = []
    framed = []
    for encoded in event["certificates"]:
        der = base64.b64decode(encoded)
        pem = ssl.DER_cert_to_PEM_cert(der)
        framed.append(
            pem.startswith(ssl.PEM_HEADER) and pem.strip().endswith(ssl.PEM_FOOTER)
        )
        round_tripped.append(
            base64.b64encode(ssl.PEM_cert_to_DER_cert(pem)).decode("ascii")
        )
    body = ssl.DER_cert_to_PEM_cert(b"\x01\x02\x03")
    return {
        "round_tripped": round_tripped,
        "framed": framed,
        "no_header": _refused(lambda: ssl.PEM_cert_to_DER_cert(body.split("\n", 1)[1])),
        "no_footer": _refused(
            lambda: ssl.PEM_cert_to_DER_cert(body.replace(ssl.PEM_FOOTER, ""))
        ),
        "times": [
            ssl.cert_time_to_seconds("Jan  5 09:34:43 2018 GMT"),
            ssl.cert_time_to_seconds("Dec 31 23:58:59 2025 GMT"),
        ],
    }


def _session(event):
    context = ssl.create_default_context()
    wrapped = context.wrap_socket(socket.socket(), server_hostname="cache.example")
    incoming = ssl.MemoryBIO()
    outgoing = ssl.MemoryBIO()
    over_buffers = context.wrap_bio(incoming, outgoing, server_hostname="cache.example")
    buffered = ssl.MemoryBIO()
    buffered.write(b"hello")
    pending = buffered.pending
    read = buffered.read()
    buffered.write_eof()
    return {
        "wrapped": isinstance(wrapped, ssl.SSLSocket),
        "handshake": _refused(wrapped.do_handshake),
        "write": _refused(lambda: wrapped.sendall(b"PING\r\n")),
        "read": _refused(lambda: wrapped.recv(64)),
        "unwrap": _refused(wrapped.unwrap),
        "bio_handshake": _refused(over_buffers.do_handshake),
        "peer_certificate": _refused(wrapped.getpeercert),
        "channel_binding": _refused(wrapped.get_channel_binding),
        "connect": _refused(lambda: wrapped.connect(("cache.example", 6379))),
        "create_connection": _refused(
            lambda: socket.create_connection(("cache.example", 6379), 1)
        ),
        "server_certificate": _refused(
            lambda: ssl.get_server_certificate(("cache.example", 6379))
        ),
        "version": wrapped.version(),
        "cipher": wrapped.cipher(),
        "selected_alpn": wrapped.selected_alpn_protocol(),
        "session_reused": wrapped.session_reused,
        "bio_pending": pending,
        "bio_read": base64.b64encode(read).decode("ascii"),
        "bio_eof": buffered.eof,
    }


def _modules(event):
    outcome = {}
    for name in event["modules"]:
        try:
            importlib.import_module(name)
            outcome[name] = ""
        except BaseException as error:
            outcome[name] = "%s: %s" % (type(error).__name__, error)
    return outcome


_CASES = {
    "names": _names,
    "contexts": _contexts,
    "store": _store,
    "encodings": _encodings,
    "session": _session,
    "modules": _modules,
}


def handler(event, context):
    return _CASES[event["case"]](event)
"#;
