// SPDX-License-Identifier: CC0-1.0

//! Firehose resolves the IAM roles a Firehose stream is configured with.
//!
//! The rule under test: Firehose assumes every `RoleARN` a stream's configuration names,
//! so it resolves each of them against IAM when the stream is WRITTEN — not when a record
//! is delivered — and refuses the request with HTTP 400 `InvalidArgumentException`,
//! persisting nothing, unless that role exists in this account and its trust policy lets
//! the Firehose service principal (`firehose.amazonaws.com`) assume it. A request naming
//! roles that do exist and do trust Firehose is served normally.
//!
//! This suite needs TWO services on one engine — Firehose reaches IAM through the
//! in-process callback, so a single-service router has no IAM to ask — which is why it
//! lives here rather than in either service's own crate. The rows span what the rule
//! generalises over: the trust policy's shape (a single `Action`/`Service` string, each as
//! a list, a `Deny` statement, a policy naming another service only), where the role lives
//! (the default path and a path), which account the ARN names, whether the role was ever
//! created, WHICH member of the configuration carries the role (the destination's own, the
//! nested S3 configuration's, a source's), and both operations that take a configuration
//! (`CreateDeliveryStream` and `UpdateDestination`).
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// Long enough to cover the emulator's chosen `CREATING` -> `ACTIVE` span for a Firehose
/// stream (`crate::state::TRANSITION_DELAY` in the firehose crate, mirrored by its own
/// suites' `PAST_THE_TRANSITION`): an `UpdateDestination` lands only on a settled stream,
/// so the one update below waits this out first.
const PAST_THE_TRANSITION: std::time::Duration = std::time::Duration::from_millis(3_400);

/// The Firehose service principal a destination role must trust.
const FIREHOSE_PRINCIPAL: &str = "\"firehose.amazonaws.com\"";

/// An S3 bucket ARN satisfying `BucketARN`'s modelled pattern.
const BUCKET_ARN: &str = "arn:aws:s3:::demo-bucket";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// A trust policy with one statement, built from the pieces that vary between rows: the
/// effect, the action(s) and the service principal(s).
///
/// Written compactly and with no `&`, `=`, `+` or `%`, so it travels in an `awsQuery` form
/// body as written.
fn trust_policy(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}}}]}}"
    )
}

/// A trust policy with TWO statements about the same principal and action: one `Allow` and
/// one explicit `Deny`. IAM's explicit deny wins, so a role carrying this policy cannot be
/// assumed.
fn allow_then_deny_policy(service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Principal\":{{\"Service\":{service}}},\
         \"Action\":\"sts:AssumeRole\"}},\
         {{\"Effect\":\"Deny\",\"Principal\":{{\"Service\":{service}}},\
         \"Action\":\"sts:AssumeRole\"}}]}}"
    )
}

/// One IAM role the table needs, as `CreateRole` parameters.
struct RoleFixture {
    role_name: &'static str,
    path: Option<&'static str>,
    policy: String,
}

/// The roles the rows below name. Each trust policy differs in the shape AWS accepts for
/// `Action` and `Principal.Service` (one string, or a list), in its effect, or in the
/// principal it names.
fn role_fixtures() -> Vec<RoleFixture> {
    vec![
        RoleFixture {
            role_name: "firehose-delivery",
            path: None,
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", FIREHOSE_PRINCIPAL),
        },
        RoleFixture {
            role_name: "firehose-delivery-lists",
            path: None,
            policy: trust_policy(
                "Allow",
                "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                "[\"events.amazonaws.com\",\"firehose.amazonaws.com\"]",
            ),
        },
        RoleFixture {
            role_name: "firehose-delivery-wildcard",
            path: None,
            policy: trust_policy("Allow", "\"sts:*\"", FIREHOSE_PRINCIPAL),
        },
        RoleFixture {
            role_name: "path-scoped-role",
            path: Some("/service-role/"),
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", FIREHOSE_PRINCIPAL),
        },
        RoleFixture {
            role_name: "lambda-only-role",
            path: None,
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
        },
        RoleFixture {
            role_name: "denied-role",
            path: None,
            policy: trust_policy("Deny", "\"sts:AssumeRole\"", FIREHOSE_PRINCIPAL),
        },
        RoleFixture {
            role_name: "allow-then-deny-role",
            path: None,
            policy: allow_then_deny_policy(FIREHOSE_PRINCIPAL),
        },
    ]
}

/// The role ARN of a role this account holds, under the default path.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// One row: the stream a `CreateDeliveryStream` writes, the configuration it carries, and
/// whether the stream must be created.
struct RoleCase {
    case: &'static str,
    /// The stream's name, so each row writes its own resource.
    stream: &'static str,
    /// The create's destination and source members, beside `DeliveryStreamName`.
    configuration: serde_json::Value,
    /// The role ARN the refusal must name, for a row that is refused.
    refused_role: Option<String>,
}

/// An `S3DestinationConfiguration` naming `role_arn`.
fn s3_configuration(role_arn: &str) -> serde_json::Value {
    serde_json::json!({ "RoleARN": role_arn, "BucketARN": BUCKET_ARN, "Prefix": "raw/" })
}

fn role_cases() -> Vec<RoleCase> {
    let trusted = role_arn("firehose-delivery");
    vec![
        RoleCase {
            case: "a role that exists and trusts Firehose",
            stream: "trusted",
            configuration: serde_json::json!({
                "S3DestinationConfiguration": s3_configuration(&trusted),
            }),
            refused_role: None,
        },
        RoleCase {
            case: "a trust policy whose Action and Service are lists",
            stream: "trusted-lists",
            configuration: serde_json::json!({
                "S3DestinationConfiguration":
                    s3_configuration(&role_arn("firehose-delivery-lists")),
            }),
            refused_role: None,
        },
        RoleCase {
            case: "a trust policy allowing the whole sts namespace",
            stream: "trusted-wildcard",
            configuration: serde_json::json!({
                "S3DestinationConfiguration":
                    s3_configuration(&role_arn("firehose-delivery-wildcard")),
            }),
            refused_role: None,
        },
        RoleCase {
            case: "a role under a path",
            stream: "path-scoped",
            configuration: serde_json::json!({
                "S3DestinationConfiguration": s3_configuration(&format!(
                    "arn:aws:iam::{}:role/service-role/path-scoped-role",
                    engine::emulator_account_id()
                )),
            }),
            refused_role: None,
        },
        RoleCase {
            case: "a role that was never created",
            stream: "absent-role",
            configuration: serde_json::json!({
                "S3DestinationConfiguration": s3_configuration(&role_arn("never-created-role")),
            }),
            refused_role: Some(role_arn("never-created-role")),
        },
        RoleCase {
            case: "a role that trusts another service only",
            stream: "lambda-role",
            configuration: serde_json::json!({
                "S3DestinationConfiguration": s3_configuration(&role_arn("lambda-only-role")),
            }),
            refused_role: Some(role_arn("lambda-only-role")),
        },
        RoleCase {
            case: "a role whose statement denies the assume",
            stream: "denied-role",
            configuration: serde_json::json!({
                "S3DestinationConfiguration": s3_configuration(&role_arn("denied-role")),
            }),
            refused_role: Some(role_arn("denied-role")),
        },
        RoleCase {
            // IAM's explicit deny wins over an Allow for the same principal.
            case: "a role that both allows and explicitly denies the assume",
            stream: "allow-then-deny",
            configuration: serde_json::json!({
                "S3DestinationConfiguration": s3_configuration(&role_arn("allow-then-deny-role")),
            }),
            refused_role: Some(role_arn("allow-then-deny-role")),
        },
        RoleCase {
            case: "a role of another account",
            stream: "foreign-role",
            configuration: serde_json::json!({
                "S3DestinationConfiguration":
                    s3_configuration("arn:aws:iam::123456789012:role/firehose-delivery"),
            }),
            refused_role: Some("arn:aws:iam::123456789012:role/firehose-delivery".to_string()),
        },
        RoleCase {
            // Not the member the issue cited: the role of the NESTED S3 configuration is
            // a role Firehose assumes too, so an absent one is refused even though the
            // destination's own role is fine.
            case: "a trusted destination role beside an absent nested S3 role",
            stream: "nested-absent-role",
            configuration: serde_json::json!({
                "SplunkDestinationConfiguration": {
                    "HECEndpoint": "https://splunk.example.com:8088",
                    "HECEndpointType": "Raw",
                    "S3Configuration": s3_configuration(&role_arn("never-created-role")),
                },
            }),
            refused_role: Some(role_arn("never-created-role")),
        },
        RoleCase {
            // Nor is a source role: the same gate covers the role a Kinesis-sourced
            // stream reads its source with.
            case: "a trusted destination role beside an absent source role",
            stream: "source-absent-role",
            configuration: serde_json::json!({
                "DeliveryStreamType": "KinesisStreamAsSource",
                "KinesisStreamSourceConfiguration": {
                    "KinesisStreamARN":
                        format!("arn:aws:kinesis:{REGION}:000000000000:stream/source"),
                    "RoleARN": role_arn("never-created-role"),
                },
                "S3DestinationConfiguration": s3_configuration(&trusted),
            }),
            refused_role: Some(role_arn("never-created-role")),
        },
        RoleCase {
            case: "a Kinesis source whose role is trusted",
            stream: "source-trusted-role",
            configuration: serde_json::json!({
                "DeliveryStreamType": "KinesisStreamAsSource",
                "KinesisStreamSourceConfiguration": {
                    "KinesisStreamARN":
                        format!("arn:aws:kinesis:{REGION}:000000000000:stream/source"),
                    "RoleARN": role_arn("firehose-delivery-lists"),
                },
                "S3DestinationConfiguration": s3_configuration(&trusted),
            }),
            refused_role: None,
        },
    ]
}

/// Send a request and return the status with the body as text.
fn send(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    content_type: &str,
    target: Option<&str>,
    body: String,
) -> (u16, String) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type);
    if let Some(target) = target {
        request = request.header("x-amz-target", target);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Drive one `awsQuery` IAM action: `POST /` with the parameters in the form body. Every
/// value here is free of `&`, `=`, `+` and `%`, so it needs no escaping.
fn iam(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    action: &str,
    parameters: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in parameters {
        body.push_str(&format!("&{key}={value}"));
    }
    send(
        router,
        runtime,
        "iam",
        "application/x-www-form-urlencoded",
        None,
        body,
    )
}

/// Drive one firehose operation (`awsJson1_1`), returning the status and the parsed body.
fn firehose(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: &serde_json::Value,
) -> (u16, serde_json::Value) {
    let (status, text) = send(
        router,
        runtime,
        "firehose",
        "application/x-amz-json-1.1",
        Some(&format!("Firehose_20150804.{operation}")),
        body.to_string(),
    );
    let parsed = if text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// The wire error code, without its namespace.
fn error_code(body: &serde_json::Value) -> String {
    body.get("__type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or("")
        .to_string()
}

/// The error message, whichever casing the envelope carries it under.
fn error_message(body: &serde_json::Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// The sentence Firehose states for a role it cannot assume.
fn unassumable(role_arn: &str) -> String {
    format!("Firehose is unable to assume role {role_arn}. Please check the role provided.")
}

#[test]
fn a_stream_is_written_only_for_roles_iam_holds_and_firehose_may_assume() {
    std::thread::Builder::new()
        .name("firehose-destination-role".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                // The time driver, because the emulator's services run CLOCKS: a service
                // whose state owes a transition spawns a background task that waits on a
                // timer, and a timer on a runtime built without this panics the task — and,
                // on a current-thread runtime, that panic unwinds out of whichever
                // `block_on` happened to poll it. Enabling it is what lets those clocks run
                // here, and it changes nothing this suite asserts.
                .enable_time()
                .build()
                .expect("a current-thread runtime");

            for fixture in role_fixtures() {
                let mut parameters = vec![
                    ("RoleName", fixture.role_name),
                    ("AssumeRolePolicyDocument", fixture.policy.as_str()),
                ];
                if let Some(path) = fixture.path {
                    parameters.push(("Path", path));
                }
                let (status, body) = iam(&router, &runtime, "CreateRole", &parameters);
                assert_eq!(
                    status, 200,
                    "setup: CreateRole {}: got {status} {body}",
                    fixture.role_name
                );
            }

            for case in role_cases() {
                let mut request = case.configuration.clone();
                request["DeliveryStreamName"] = serde_json::json!(case.stream);
                let (status, body) = firehose(&router, &runtime, "CreateDeliveryStream", &request);
                let read = firehose(
                    &router,
                    &runtime,
                    "DescribeDeliveryStream",
                    &serde_json::json!({ "DeliveryStreamName": case.stream }),
                );
                match &case.refused_role {
                    None => {
                        assert_eq!(
                            status, 200,
                            "case {:?}: the create must succeed, got {status} {body}",
                            case.case
                        );
                        // The round trip: the stream the create wrote is readable.
                        assert_eq!(
                            read.0, 200,
                            "case {:?}: the stream must be persisted, got {} {}",
                            case.case, read.0, read.1
                        );
                        assert_eq!(
                            read.1["DeliveryStreamDescription"]["DeliveryStreamARN"],
                            serde_json::json!(format!(
                                "arn:aws:firehose:{REGION}:{}:deliverystream/{}",
                                engine::emulator_account_id(),
                                case.stream
                            )),
                            "case {:?}: got {}",
                            case.case,
                            read.1
                        );
                    }
                    Some(refused_role) => {
                        assert_eq!(
                            (status, error_code(&body).as_str()),
                            (400, "InvalidArgumentException"),
                            "case {:?}: a role Firehose cannot assume must be refused 400 \
                             InvalidArgumentException, got {status} {body}",
                            case.case
                        );
                        assert_eq!(
                            error_message(&body),
                            unassumable(refused_role),
                            "case {:?}: expected the unassumable-role refusal, got {body}",
                            case.case
                        );
                        assert_eq!(
                            (read.0, error_code(&read.1).as_str()),
                            (400, "ResourceNotFoundException"),
                            "case {:?}: a refused create must persist nothing, got {} {}",
                            case.case,
                            read.0,
                            read.1
                        );
                    }
                }
            }

            // `UpdateDestination` applies the same gate, and a refused update leaves the
            // stored destination exactly as it was.
            let trusted = role_arn("firehose-delivery");
            let absent = role_arn("never-created-role");
            let update = |role: &str| {
                serde_json::json!({
                    "DeliveryStreamName": "trusted",
                    "CurrentDeliveryStreamVersionId": "1",
                    "DestinationId": "destinationId-000000000001",
                    "S3DestinationUpdate": { "RoleARN": role, "Prefix": "updated/" },
                })
            };
            let (status, body) =
                firehose(&router, &runtime, "UpdateDestination", &update(&absent));
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidArgumentException"),
                "UpdateDestination onto an absent role must be refused, got {status} {body}"
            );
            assert_eq!(
                error_message(&body),
                unassumable(&absent),
                "UpdateDestination: expected the unassumable-role refusal, got {body}"
            );
            let (status, described) = firehose(
                &router,
                &runtime,
                "DescribeDeliveryStream",
                &serde_json::json!({ "DeliveryStreamName": "trusted" }),
            );
            assert_eq!(
                status, 200,
                "the stream must survive a refused update, got {status} {described}"
            );
            let destination = &described["DeliveryStreamDescription"]["Destinations"][0]
                ["S3DestinationDescription"];
            assert_eq!(
                destination["RoleARN"],
                serde_json::json!(trusted),
                "a refused update must leave the stored role unchanged, got {described}"
            );
            assert_eq!(
                destination["Prefix"],
                serde_json::json!("raw/"),
                "a refused update must leave the stored destination unchanged, got {described}"
            );

            // The same update onto a role IAM does hold succeeds, so the gate is the role
            // and not the operation. (The stream was created above and the clock owes it
            // ACTIVE, so the update waits for that span to elapse.)
            std::thread::sleep(PAST_THE_TRANSITION);
            let (status, body) = firehose(
                &router,
                &runtime,
                "UpdateDestination",
                &update(&role_arn("firehose-delivery-lists")),
            );
            assert_eq!(
                status, 200,
                "UpdateDestination onto a trusted role must succeed, got {status} {body}"
            );
        })
        .expect("spawn the destination-role thread")
        .join()
        .expect("every Firehose role must be resolved against IAM");
}
