// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CLIENT CONFIGURATION a running python handler builds a client
//! with: Lambda and Amazon CloudWatch on ONE engine, a `python3.x` function whose code does
//! `from botocore.config import Config`, and the datapoint each configured client published
//! read back where a client reads it.
//!
//! The rule pinned here: a managed `python3.x` runtime ships the AWS SDK for Python on the
//! function's import path, so every module of that SDK a handler imports resolves and behaves
//! as the SDK's own — `botocore.config.Config` included, which is the object a handler passes
//! to `boto3.client(..., config=...)`. It holds the SDK's own option set in the SDK's own
//! order, so an option given positionally lands where botocore binds it, each option's
//! default is the SDK's, an option name the SDK does not have is a `TypeError`, a retry
//! configuration the SDK refuses raises the error class botocore raises, `merge` lets the
//! other configuration's options win, and the Region a configuration names is the Region the
//! client built with it calls in. A call made through such a client reaches the service and
//! is answered by it.
//!
//! The rows differ along what the rule generalises over: WHICH options the configuration
//! carries (none, retries off, a retry mode, timeouts and a pool size, a user-agent suffix,
//! a Region), whether the configuration is read, merged, built positionally or refused, which
//! import path `Config` is reached through, and which error class a refusal raises. Each
//! call row is asserted where a client sees it — the datapoint CloudWatch's own metric read
//! reports — so a configured client that was accepted but published nothing fails here.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the function runs in.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `X-Amz-Target` prefix CloudWatch's own model declares
/// (`smithy_query(query="service_info")`: service shape `GraniteServiceVersion20100801`).
const CLOUDWATCH_TARGET: &str = "GraniteServiceVersion20100801";

/// The namespace every datapoint below is published into: a custom one, so nothing here
/// touches a reserved `AWS/` namespace.
const NAMESPACE: &str = "Emulator/ClientConfig";

/// The `Period`, in seconds, every read below is answered at.
const PERIOD: i64 = 60;

/// How far either side of now a read's window reaches, in seconds.
const WINDOW_SECS: f64 = 600.0;

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// The function every row invokes.
const FUNCTION: &str = "configured";

/// The module and entry point the package holds the handler at.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The handler every row runs: it answers what `botocore.config.Config` did, and — for a row
/// that makes a CALL — publishes one datapoint through a client built with the configuration
/// the event names.
const SOURCE: &str = r#"
import boto3
import botocore.exceptions
from botocore.config import Config


def reported(config):
    """The options a handler reads back off a configuration."""
    return {
        "region_name": config.region_name,
        "connect_timeout": config.connect_timeout,
        "read_timeout": config.read_timeout,
        "max_pool_connections": config.max_pool_connections,
        "parameter_validation": config.parameter_validation,
        "retries": config.retries,
        "user_agent_extra": config.user_agent_extra,
        "signature_version": config.signature_version,
    }


def refused(event):
    """Build the configuration the event names and report the refusal it raises."""
    try:
        Config(**event["options"])
    except botocore.exceptions.InvalidRetryModeError as error:
        return {"raised": "InvalidRetryModeError", "message": str(error)}
    except botocore.exceptions.InvalidRetryConfigurationError as error:
        return {"raised": "InvalidRetryConfigurationError", "message": str(error)}
    except TypeError as error:
        return {"raised": "TypeError", "message": str(error)}
    return {"raised": None, "message": ""}


def lambda_handler(event, context):
    case = event["case"]
    if case == "defaults":
        return {"ok": True, "config": reported(Config())}
    if case == "refused":
        return dict({"ok": True}, **refused(event))
    if case == "merged":
        merged = Config(region_name=event["region"], read_timeout=5).merge(
            Config(read_timeout=9, user_agent_extra="merged")
        )
        return {"ok": True, "config": reported(merged)}
    if case == "positional":
        return {"ok": True, "config": reported(Config(event["region"]))}
    if case == "client_import_path":
        from botocore.client import Config as ClientConfig

        client = boto3.client("cloudwatch", config=ClientConfig(region_name=event["region"]))
        return {"ok": True, "region_name": client.meta.region_name}
    config = Config(**event["options"])
    client = boto3.client("cloudwatch", config=config)
    try:
        answered = client.put_metric_data(
            Namespace=event["namespace"],
            MetricData=[
                {"MetricName": event["metric"], "Value": event["value"], "Unit": "Count"}
            ],
        )
    except Exception as error:
        return {"ok": False, "refused": "%s: %s" % (type(error).__name__, error)}
    return {
        "ok": True,
        "status": answered["ResponseMetadata"]["HTTPStatusCode"],
        "region_name": client.meta.region_name,
        "config": reported(client.meta.config),
    }
"#;

/// Lambda and Amazon CloudWatch over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them: the handler's SDK call is dispatched back into this same router, so the series it
/// publishes into is the series this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One CloudWatch call (`awsJson1_0`, `POST /` with `X-Amz-Target`) that must answer 200.
async fn cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("monitoring.{REGION}.amazonaws.com"))
        .header("authorization", authorization("monitoring"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{CLOUDWATCH_TARGET}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status and parsed body.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parsed)
}

/// A .zip deployment package holding [`SOURCE`] as `lambda_function.py`.
fn package() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create the function every row invokes and wait out the provisioning window.
async fn create_function(router: &MultiServiceRouter) {
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package()),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` the function synchronously with `event`, asserting the run itself did not fail.
async fn invoke(router: &MultiServiceRouter, case: &str, event: &Value) -> Value {
    let (status, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{FUNCTION}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    assert_eq!(
        payload.get("ok").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: the handler must have run to completion, got {payload}"
    );
    payload
}

/// The SUM of every datapoint the metric `metric` holds in [`NAMESPACE`].
async fn published(router: &MultiServiceRouter, case: &str, metric: &str) -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    let answered = cloudwatch(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "StartTime": now - WINDOW_SECS,
            "EndTime": now + WINDOW_SECS,
            "Period": PERIOD,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    answered["Datapoints"]
        .as_array()
        .map(|points| {
            points
                .iter()
                .filter_map(|point| point["Sum"].as_f64())
                .sum()
        })
        .unwrap_or_default()
}

/// One row that CALLS through a configured client: the options the configuration carries, the
/// value the call publishes, and the Region the client must then report calling in.
struct CallRow {
    case: &'static str,
    /// The `Config(...)` keyword options, as the event carries them.
    options: fn() -> Value,
    /// The metric the row publishes, distinct per row so the series pins which row wrote it.
    metric: &'static str,
    value: f64,
    /// The Region the client built with those options must report.
    region: &'static str,
}

/// The call rows: a client with no options at all, one with retries turned off, one naming a
/// retry mode, one carrying transport options, one carrying a user-agent suffix, and one
/// naming a Region of its own.
fn call_rows() -> Vec<CallRow> {
    vec![
        CallRow {
            case: "no options at all",
            options: || json!({}),
            metric: "PlainClient",
            value: 3.0,
            region: REGION,
        },
        CallRow {
            case: "retries turned off, as a handler that wants one attempt asks",
            options: || json!({ "retries": { "max_attempts": 0 } }),
            metric: "NoRetries",
            value: 5.0,
            region: REGION,
        },
        CallRow {
            case: "the standard retry mode with a bounded attempt count",
            options: || json!({ "retries": { "mode": "standard", "max_attempts": 3 } }),
            metric: "StandardRetries",
            value: 7.0,
            region: REGION,
        },
        CallRow {
            case: "transport options: both timeouts and a pool size",
            options: || {
                json!({
                    "connect_timeout": 2,
                    "read_timeout": 5,
                    "max_pool_connections": 2,
                })
            },
            metric: "TransportOptions",
            value: 11.0,
            region: REGION,
        },
        CallRow {
            case: "a user-agent suffix",
            options: || json!({ "user_agent_extra": "emulator-suite/1" }),
            metric: "UserAgentExtra",
            value: 13.0,
            region: REGION,
        },
        CallRow {
            case: "a Region named by the configuration rather than by the environment",
            options: || json!({ "region_name": "eu-west-1" }),
            metric: "ConfiguredRegion",
            value: 17.0,
            region: "eu-west-1",
        },
    ]
}

/// RULE: a client a handler builds with `botocore.config.Config` carries the options the
/// configuration names and makes its call, which the service answers — so the datapoint the
/// handler published is readable where a client reads it.
#[tokio::test]
async fn a_handler_calls_through_a_client_built_with_a_botocore_configuration() {
    let router = router();
    create_function(&router).await;

    for row in call_rows() {
        let case = row.case;
        let payload = invoke(
            &router,
            case,
            &json!({
                "case": "call",
                "options": (row.options)(),
                "namespace": NAMESPACE,
                "metric": row.metric,
                "value": row.value,
            }),
        )
        .await;
        assert_eq!(
            payload.get("status").and_then(Value::as_i64),
            Some(200),
            "case {case:?}: the configured client's call must be answered 200, got {payload}"
        );
        assert_eq!(
            payload.get("region_name").and_then(Value::as_str),
            Some(row.region),
            "case {case:?}: the client must report calling in {:?}, got {payload}",
            row.region
        );
        let sum = published(&router, case, row.metric).await;
        assert_eq!(
            sum, row.value,
            "case {case:?}: the datapoint the configured client published must read back as \
             {}, got {sum}",
            row.value
        );
    }
}

/// One row about the configuration OBJECT itself: what the handler asks of it, and what it
/// must answer.
struct ConfigRow {
    case: &'static str,
    /// The event the handler is invoked with.
    event: fn() -> Value,
    /// The `(member, expected)` pairs the answer's `config` must carry, as JSON.
    expected: fn() -> Vec<(&'static str, Value)>,
}

/// The rows: the SDK's own defaults, a merge, and a positional option.
fn config_rows() -> Vec<ConfigRow> {
    vec![
        ConfigRow {
            case: "the defaults the SDK gives an unconfigured client",
            event: || json!({ "case": "defaults" }),
            expected: || {
                vec![
                    ("region_name", Value::Null),
                    ("connect_timeout", json!(60)),
                    ("read_timeout", json!(60)),
                    ("max_pool_connections", json!(10)),
                    ("parameter_validation", json!(true)),
                    ("retries", Value::Null),
                    ("user_agent_extra", Value::Null),
                    ("signature_version", Value::Null),
                ]
            },
        },
        ConfigRow {
            case: "a merge, where the other configuration's options win",
            event: || json!({ "case": "merged", "region": REGION }),
            expected: || {
                vec![
                    ("region_name", json!(REGION)),
                    ("read_timeout", json!(9)),
                    ("user_agent_extra", json!("merged")),
                    ("connect_timeout", json!(60)),
                ]
            },
        },
        ConfigRow {
            case: "an option given positionally, which lands on the first option",
            event: || json!({ "case": "positional", "region": "ap-south-1" }),
            expected: || {
                vec![
                    ("region_name", json!("ap-south-1")),
                    ("read_timeout", json!(60)),
                ]
            },
        },
    ]
}

/// RULE: `Config` holds the SDK's own option set, in the SDK's own order, with the SDK's own
/// defaults, and `merge` lets the other configuration's options win.
#[tokio::test]
async fn a_botocore_configuration_holds_the_sdks_own_options_and_defaults() {
    let router = router();
    create_function(&router).await;

    for row in config_rows() {
        let case = row.case;
        let payload = invoke(&router, case, &(row.event)()).await;
        let config = payload.get("config").cloned().unwrap_or(Value::Null);
        for (member, expected) in (row.expected)() {
            assert_eq!(
                config.get(member).unwrap_or(&Value::Null),
                &expected,
                "case {case:?}: {member} must be {expected}, got {config}"
            );
        }
    }
}

/// One refusal row: the options the handler asks for, the error class the SDK raises for
/// them, and the sentence that error must render.
struct RefusalRow {
    case: &'static str,
    options: fn() -> Value,
    raised: &'static str,
    message: &'static str,
}

/// The refusals: an option name the SDK does not have, a retry member it does not have, a
/// retry mode outside the set it documents, and an attempt count below its floor.
const REFUSALS: &[RefusalRow] = &[
    RefusalRow {
        case: "an option name the SDK does not have",
        options: || json!({ "not_an_option": 1 }),
        raised: "TypeError",
        message: "Got unexpected keyword argument 'not_an_option'",
    },
    RefusalRow {
        case: "a retry member the SDK does not have",
        options: || json!({ "retries": { "retry_forever": true } }),
        raised: "InvalidRetryConfigurationError",
        message: "Cannot provide retry configuration for \"retry_forever\". Valid retry \
                  configuration options are: total_max_attempts, max_attempts, mode",
    },
    RefusalRow {
        case: "a retry mode outside the set the SDK documents",
        options: || json!({ "retries": { "mode": "turbo" } }),
        raised: "InvalidRetryModeError",
        message: "Invalid value provided to \"mode\": \"turbo\" must be one of: legacy, \
                  standard, adaptive",
    },
    RefusalRow {
        case: "a negative attempt count",
        options: || json!({ "retries": { "max_attempts": -1 } }),
        raised: "InvalidMaxRetryAttemptsError",
        message: "Value provided to \"max_attempts\": -1 must be an integer greater than or \
                  equal to 0.",
    },
];

/// RULE (the other half): a configuration the SDK will not build is refused with the SDK's own
/// error class and sentence, so a handler that catches one catches what it would catch on AWS.
#[tokio::test]
async fn a_configuration_the_sdk_will_not_build_raises_the_sdks_own_error() {
    let router = router();
    create_function(&router).await;

    for row in REFUSALS {
        let case = row.case;
        let payload = invoke(
            &router,
            case,
            &json!({ "case": "refused", "options": (row.options)() }),
        )
        .await;
        // `InvalidMaxRetryAttemptsError` is a subclass of `InvalidRetryConfigurationError`,
        // which is the class the handler reports it under.
        let expected = if row.raised == "InvalidMaxRetryAttemptsError" {
            "InvalidRetryConfigurationError"
        } else {
            row.raised
        };
        assert_eq!(
            payload.get("raised").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: the configuration must raise {expected}, got {payload}"
        );
        assert_eq!(
            payload.get("message").and_then(Value::as_str),
            Some(row.message),
            "case {case:?}: the refusal must render {:?}, got {payload}",
            row.message
        );
    }
}

/// RULE (the import path): `Config` is reachable on the other path the SDK exposes it on
/// (`from botocore.client import Config`), and a client built with it calls in the Region it
/// names.
#[tokio::test]
async fn the_configuration_is_reachable_on_the_sdks_other_import_path() {
    let router = router();
    create_function(&router).await;

    let case = "from botocore.client import Config";
    let payload = invoke(
        &router,
        case,
        &json!({ "case": "client_import_path", "region": "us-west-2" }),
    )
    .await;
    assert_eq!(
        payload.get("region_name").and_then(Value::as_str),
        Some("us-west-2"),
        "case {case:?}: the client must report the Region its configuration named, got {payload}"
    );
}
