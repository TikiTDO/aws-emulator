// SPDX-License-Identifier: CC0-1.0

//! The EventBridge Pipes pipe lifecycle, end to end over the wire.
//!
//! Every assertion here goes through `MultiServiceRouter::dispatch`, the one path a
//! socket-served request takes, so what is tested is the HTTP behaviour of
//! `CreatePipe`/`DescribePipe`/`ListPipes`/`StartPipe`/`StopPipe`/`DeletePipe` and not a
//! Rust helper. The tables span the class each rule generalises over; the pipe the issue
//! cited is one row.

//! `UpdatePipe` has its own suite in `tests/all/pipe_update.rs`; the harness below is
//! `pub(crate)` so that suite reaches the SAME one request path rather than copying it.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_pipes_pipes::state::{MAX_PIPES, STATE_SETTLE_DELAY};
use service_com_amazonaws_pipes_pipes::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request is signed for unless a case says otherwise.
pub(crate) const REGION: &str = "us-west-2";

/// Long enough after a lifecycle call that the state move it owes is certainly due.
pub(crate) const PAST_SETTLE: Duration = STATE_SETTLE_DELAY.saturating_add(Duration::from_millis(400));

/// Every `CreatePipe` input member that is optional, carries NO declared default, and is
/// reported only when the pipe carries one. Source: the `CreatePipe` input in
/// `OPERATIONS.md` (`DesiredState` is excluded because an absent `DesiredState` defaults to
/// `RUNNING`, so it is reported either way).
///
/// The CONFIGURATION CONTAINERS — `SourceParameters`, `EnrichmentParameters`,
/// `TargetParameters` — and `Tags` are excluded too: a read reports those whether the
/// create request mentioned them or not, which is the rule and the table of
/// `tests/all/pipe_reported_configuration.rs`.
const OPTIONAL_CREATE_MEMBERS: &[&str] = &[
    "Description",
    "Enrichment",
    "LogConfiguration",
    "KmsKeyIdentifier",
];

/// One emulator holding only this service, reached exactly as the router reaches it.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// What one call answered: the HTTP status, the wire error code (empty on success) and
/// the parsed JSON body.
#[derive(Debug, Clone)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_code: String,
    pub(crate) body: Value,
}

impl Reply {
    /// A member of the response body, or `Value::Null` when it carries none.
    pub(crate) fn member(&self, name: &str) -> &Value {
        self.body.get(name).unwrap_or(&Value::Null)
    }

    pub(crate) fn string(&self, name: &str) -> String {
        self.member(name).as_str().unwrap_or_default().to_string()
    }

    pub(crate) fn number(&self, name: &str) -> f64 {
        self.member(name).as_f64().unwrap_or_default()
    }

    /// The exception message a refusal carries, however this protocol's error body spells
    /// the modelled `message` member.
    pub(crate) fn message(&self) -> String {
        for member in ["message", "Message"] {
            if let Some(text) = self.member(member).as_str() {
                return text.to_string();
            }
        }
        String::new()
    }
}

/// Dispatch one signed restJson1 request and hand back the WHOLE response, so a suite can
/// assert the framing — the status line, the body bytes and the headers — and not only the
/// members a body parses into. [`call`] is this plus that parse.
pub(crate) async fn call_raw(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
    region: &str,
) -> http::Response<bytes::Bytes> {
    let payload = body
        .map(|value| serde_json::to_vec(&value).expect("serialize the request body"))
        .unwrap_or_default();
    let host = format!("pipes.{region}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDPIPESTEST/20260101/{region}/pipes/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("build the request");

    router.dispatch(request).await
}

/// Dispatch one signed restJson1 request and read the reply.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
    region: &str,
) -> Reply {
    reply_of(call_raw(router, method, uri, body, region).await)
}

/// What one response says, read the way every suite here reads one: the status, the wire
/// error code (empty on success) and the parsed body. Split out of [`call`] so a suite that
/// needs the WHOLE response ([`call_raw`]) can still read it as a [`Reply`] rather than
/// building a second request path of its own.
pub(crate) fn reply_of(response: http::Response<bytes::Bytes>) -> Reply {
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    // restJson1 carries the error shape in the `x-amzn-errortype` header and in the
    // body's `__type`; read whichever this path set, stripping any namespace prefix.
    let error_code = if header_code.is_empty() {
        body.get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    let error_code = error_code
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    Reply {
        status,
        error_code,
        body,
    }
}

/// A `CreatePipe` body carrying only the three required members.
pub(crate) fn minimal_body(source: &str, target: &str) -> Value {
    json!({
        "Source": source,
        "Target": target,
        "RoleArn": "arn:aws:iam::000000000000:role/pipes-role",
    })
}

pub(crate) async fn create(router: &MultiServiceRouter, name: &str, body: Value) -> Reply {
    call(router, "POST", &format!("/v1/pipes/{name}"), Some(body), REGION).await
}

/// The `SourceParameters` a `Source` of this kind cannot be created without, or `None`
/// for one that needs none.
///
/// A source type whose parameters shape declares a required member — a stream's starting
/// position, a Kafka topic name — has no legal configuration with the member absent, so a
/// fixture naming such a source carries the minimum that makes it legal. An Amazon SQS
/// source needs none.
pub(crate) fn required_source_parameters(source: &str) -> Option<Value> {
    if source.starts_with("smk://") {
        return Some(json!({"SelfManagedKafkaParameters": {"TopicName": "orders"}}));
    }
    if source.starts_with("arn:aws:kinesis:") {
        return Some(json!({"KinesisStreamParameters": {"StartingPosition": "LATEST"}}));
    }
    if source.starts_with("arn:aws:dynamodb:") {
        return Some(json!({"DynamoDBStreamParameters": {"StartingPosition": "LATEST"}}));
    }
    if source.starts_with("arn:aws:kafka:") {
        return Some(json!({"ManagedStreamingKafkaParameters": {"TopicName": "orders"}}));
    }
    None
}

pub(crate) async fn describe(router: &MultiServiceRouter, name: &str) -> Reply {
    call(router, "GET", &format!("/v1/pipes/{name}"), None, REGION).await
}

pub(crate) async fn list(router: &MultiServiceRouter, query: &str) -> Reply {
    call(router, "GET", &format!("/v1/pipes{query}"), None, REGION).await
}

/// The names `ListPipes` reported, in the order it reported them.
pub(crate) fn listed_names(reply: &Reply) -> Vec<String> {
    reply
        .member("Pipes")
        .as_array()
        .map(|pipes| {
            pipes
                .iter()
                .map(|pipe| {
                    pipe.get("Name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: `DescribePipe` reports the stored record — every member `CreatePipe` carried,
/// unchanged — and reports nothing for a member the request did not carry.
///
/// The rows differ along what that rule generalises over: which optional members are
/// present, the form of `Source` (an ARN and the `smk://` URL the other branch of its
/// pattern allows), the shape of the nested parameter structures, the number of tags, and
/// the name's charset and length.
#[tokio::test]
async fn describe_pipe_reports_every_member_create_pipe_stored() {
    let router = router();
    let long_name = "p".repeat(64);
    let rows: Vec<(&str, Value)> = vec![
        (
            "required-only",
            minimal_body(
                "arn:aws:sqs:us-west-2:000000000000:orders",
                "arn:aws:lambda:us-west-2:000000000000:function:handler",
            ),
        ),
        (
            "described-and-tagged",
            json!({
                "Source": "smk://b-1.broker.kafka.us-west-2.amazonaws.com:9092",
                "Target": "arn:aws:sqs:us-west-2:000000000000:results",
                "RoleArn": "arn:aws:iam::000000000000:role/pipes-role",
                // A self-managed Kafka source reads one topic, which is the member its
                // parameters shape requires, so a pipe of this source type carries them.
                "SourceParameters": {"SelfManagedKafkaParameters": {"TopicName": "orders"}},
                "Description": "demo",
                "Tags": {"env": "test", "team": "platform"},
            }),
        ),
        (
            "stopped-with-enrichment",
            json!({
                "Source": "arn:aws:kinesis:us-west-2:000000000000:stream/events",
                "Target": "arn:aws:lambda:us-west-2:000000000000:function:handler",
                "RoleArn": "arn:aws:iam::000000000000:role/pipes-role",
                "DesiredState": "STOPPED",
                // A stream source reads from a position, which is the member its
                // parameters shape requires.
                "SourceParameters": {"KinesisStreamParameters": {"StartingPosition": "LATEST"}},
                "Enrichment": "arn:aws:lambda:us-west-2:000000000000:function:enrich",
                "EnrichmentParameters": {"InputTemplate": "$.detail"},
                "TargetParameters": {
                    "InputTemplate": "$.detail",
                    "SqsQueueParameters": {"MessageGroupId": "g1"},
                },
            }),
        ),
        (
            "source-parameters-and-kms",
            json!({
                "Source": "arn:aws:sqs:us-west-2:000000000000:orders",
                "Target": "arn:aws:lambda:us-west-2:000000000000:function:handler",
                "RoleArn": "arn:aws:iam::000000000000:role/pipes-role",
                "DesiredState": "RUNNING",
                "SourceParameters": {
                    "FilterCriteria": {"Filters": [{"Pattern": "{\"source\":[\"demo\"]}"}]},
                    "SqsQueueParameters": {"BatchSize": 7, "MaximumBatchingWindowInSeconds": 3},
                },
                "KmsKeyIdentifier": "alias/pipes-key",
                "LogConfiguration": {
                    "Level": "INFO",
                    "CloudwatchLogsLogDestination": {
                        "LogGroupArn": "arn:aws:logs:us-west-2:000000000000:log-group:/aws/pipes"
                    },
                    "IncludeExecutionData": ["ALL"],
                },
            }),
        ),
        (
            "name.with-every_allowed.char0",
            json!({
                "Source": "arn:aws:sqs:us-west-2:000000000000:orders",
                "Target": "arn:aws:lambda:us-west-2:000000000000:function:handler",
                "RoleArn": "arn:aws:iam::000000000000:role/pipes-role",
                "Description": "",
                "Tags": {"only": "one"},
                "LogConfiguration": {
                    "Level": "TRACE",
                    "S3LogDestination": {
                        "BucketName": "pipe-logs",
                        "BucketOwner": "000000000000",
                        "OutputFormat": "json",
                        "Prefix": "pipes/"
                    },
                },
            }),
        ),
        (
            long_name.as_str(),
            minimal_body(
                "arn:aws:sqs:us-west-2:000000000000:orders",
                "arn:aws:sqs:us-west-2:000000000000:results",
            ),
        ),
    ];

    for (name, body) in rows {
        let created = create(&router, name, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: CreatePipe must succeed, got {created:?}"
        );
        let expected_arn = format!(
            "arn:aws:pipes:{REGION}:{}:pipe/{name}",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            created.string("Arn"),
            expected_arn,
            "case {name:?}: CreatePipe must mint {expected_arn}, got {created:?}"
        );
        assert_eq!(
            created.string("Name"),
            name,
            "case {name:?}: CreatePipe must echo the name, got {created:?}"
        );
        assert_eq!(
            created.string("CurrentState"),
            "CREATING",
            "case {name:?}: a pipe AWS has just accepted is CREATING, got {created:?}"
        );
        let expected_desired = body
            .get("DesiredState")
            .and_then(Value::as_str)
            .unwrap_or("RUNNING");
        assert_eq!(
            created.string("DesiredState"),
            expected_desired,
            "case {name:?}: DesiredState must be {expected_desired}, got {created:?}"
        );
        assert!(
            created.number("CreationTime") > 0.0 && created.number("LastModifiedTime") > 0.0,
            "case {name:?}: both timestamps must be populated, got {created:?}"
        );

        let described = describe(&router, name).await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: DescribePipe must find the pipe just created, got {described:?}"
        );
        for (member, expected) in body.as_object().expect("the case body is an object") {
            // An explicit empty-string `Description` is stored exactly as an omitted one,
            // so the pipe reports no `Description` at all for it; the rule has its own
            // table in `tests/all/pipe_description.rs`.
            let expected = if member == "Description" && expected.as_str() == Some("") {
                &Value::Null
            } else {
                expected
            };
            assert_eq!(
                described.member(member),
                expected,
                "case {name:?}: DescribePipe must report {member} as created ({expected}), got {:?}",
                described.member(member)
            );
        }
        for member in OPTIONAL_CREATE_MEMBERS {
            if body.get(member).is_none() {
                assert_eq!(
                    described.member(member),
                    &Value::Null,
                    "case {name:?}: {member} was never created, so DescribePipe must report \
                     none, got {:?}",
                    described.member(member)
                );
            }
        }
        assert_eq!(
            described.string("Arn"),
            expected_arn,
            "case {name:?}: DescribePipe must report the stored ARN, got {described:?}"
        );
        assert_eq!(
            described.number("CreationTime"),
            created.number("CreationTime"),
            "case {name:?}: CreationTime must be stable across reads, got {described:?}"
        );
    }
}

/// RULE: a pipe's ARN names the region the request is signed for and the emulator's
/// account, and the stored ARN is what every later read reports.
#[tokio::test]
async fn pipe_arn_names_the_signing_region_and_the_emulator_account() {
    for region in ["us-west-2", "eu-west-1", "ap-southeast-2"] {
        let router = router();
        let body = minimal_body(
            "arn:aws:sqs:us-east-1:000000000000:orders",
            "arn:aws:lambda:us-east-1:000000000000:function:handler",
        );
        let created = call(
            &router,
            "POST",
            "/v1/pipes/regional",
            Some(body),
            region,
        )
        .await;
        let expected = format!(
            "arn:aws:pipes:{region}:{}:pipe/regional",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            created.string("Arn"),
            expected,
            "case {region:?}: expected {expected}, got {created:?}"
        );
        let described = call(&router, "GET", "/v1/pipes/regional", None, region).await;
        assert_eq!(
            described.string("Arn"),
            expected,
            "case {region:?}: DescribePipe must report the stored ARN {expected}, got {described:?}"
        );
    }
}

/// RULE: a lifecycle call puts the pipe into its transient state at once and the move to
/// the settled state is owed to the clock — so a caller acting straight after the call
/// still observes the transient state, and a caller past the span observes the settled
/// one. Each call also advances `LastModifiedTime` and leaves `CreationTime` alone.
#[tokio::test]
async fn lifecycle_states_settle_only_once_the_clock_owes_the_move() {
    let router = router();
    let created = create(
        &router,
        "settling",
        minimal_body(
            "arn:aws:sqs:us-west-2:000000000000:orders",
            "arn:aws:lambda:us-west-2:000000000000:function:handler",
        ),
    )
    .await;
    assert_eq!(created.status, 200, "CreatePipe failed: {created:?}");
    let creation_time = created.number("CreationTime");

    // (the call that starts it, the transient state it reports, the settled state).
    let transitions: Vec<(&str, &str, &str, &str)> = vec![
        ("POST", "/v1/pipes/settling", "CREATING", "RUNNING"),
        ("POST", "/v1/pipes/settling/stop", "STOPPING", "STOPPED"),
        ("POST", "/v1/pipes/settling/start", "STARTING", "RUNNING"),
    ];

    let mut previous_modified = creation_time;
    for (index, (method, uri, transient, settled)) in transitions.iter().enumerate() {
        // The CREATING row is the create above; every later row issues its own call.
        let reply = if index == 0 {
            created.clone()
        } else {
            call(&router, method, uri, None, REGION).await
        };
        assert_eq!(
            reply.status, 200,
            "case {uri:?}: the lifecycle call must succeed, got {reply:?}"
        );
        assert_eq!(
            reply.string("CurrentState"),
            *transient,
            "case {uri:?}: the call must report {transient}, got {reply:?}"
        );

        let immediately = describe(&router, "settling").await;
        assert_eq!(
            immediately.string("CurrentState"),
            *transient,
            "case {uri:?}: a caller acting straight after the call still sees {transient}, \
             got {immediately:?}"
        );
        assert!(
            immediately.number("LastModifiedTime") >= previous_modified,
            "case {uri:?}: LastModifiedTime must not go backwards ({} < {previous_modified}), \
             got {immediately:?}",
            immediately.number("LastModifiedTime")
        );
        assert_eq!(
            immediately.number("CreationTime"),
            creation_time,
            "case {uri:?}: CreationTime must never change, got {immediately:?}"
        );
        previous_modified = immediately.number("LastModifiedTime");

        // Nothing sleeps in the handler; the due instant is stored, so the move shows up
        // on the first read taken past it.
        tokio::time::sleep(PAST_SETTLE).await;
        let settled_read = describe(&router, "settling").await;
        assert_eq!(
            settled_read.string("CurrentState"),
            *settled,
            "case {uri:?}: past the settle span the pipe must be {settled}, got {settled_read:?}"
        );
        assert_eq!(
            settled_read.number("LastModifiedTime"),
            previous_modified,
            "case {uri:?}: settling a state move must not re-stamp LastModifiedTime, \
             got {settled_read:?}"
        );
    }

    let stopped = call(&router, "POST", "/v1/pipes/settling/stop", None, REGION).await;
    assert_eq!(
        stopped.string("DesiredState"),
        "STOPPED",
        "StopPipe must record the desired state, got {stopped:?}"
    );
    // The stop has to LAND before another move is asked for: a lifecycle call that
    // arrives while one is in flight is refused with `ConflictException` (see
    // `pipe_lifecycle_conflicts`).
    tokio::time::sleep(PAST_SETTLE).await;
    let started = call(&router, "POST", "/v1/pipes/settling/start", None, REGION).await;
    assert_eq!(
        started.string("DesiredState"),
        "RUNNING",
        "StartPipe must record the desired state, got {started:?}"
    );
}

/// RULE: `ListPipes` enumerates the persisted pipes in name order and reports exactly
/// those matching EVERY filter the request carries.
#[tokio::test]
async fn list_pipes_honours_every_modelled_filter() {
    let router = router();
    // (name, source, target, desired state)
    let fixture: Vec<(&str, &str, &str, &str)> = vec![
        (
            "alpha-one",
            "arn:aws:sqs:us-west-2:000000000000:orders",
            "arn:aws:lambda:us-west-2:000000000000:function:handler",
            "RUNNING",
        ),
        (
            "alpha-two",
            "arn:aws:sqs:us-west-2:000000000000:orders",
            "arn:aws:sqs:us-west-2:000000000000:results",
            "STOPPED",
        ),
        (
            "beta.one_x",
            "arn:aws:kinesis:us-west-2:000000000000:stream/events",
            "arn:aws:lambda:us-west-2:000000000000:function:handler",
            "RUNNING",
        ),
        (
            "beta-two",
            "smk://b-1.broker.kafka.us-west-2.amazonaws.com:9092",
            "arn:aws:sqs:us-west-2:000000000000:results",
            "STOPPED",
        ),
    ];
    for (name, source, target, desired) in &fixture {
        let mut body = minimal_body(source, target);
        body["DesiredState"] = json!(desired);
        // A source whose own parameters shape requires a member cannot be created without
        // it, so each fixture carries the one its source type needs.
        if let Some(parameters) = required_source_parameters(source) {
            body["SourceParameters"] = parameters;
        }
        let created = create(&router, name, body).await;
        assert_eq!(
            created.status, 200,
            "fixture {name:?}: CreatePipe failed with {created:?}"
        );
    }
    // Past the settle span every pipe holds its settled CurrentState, so the
    // `CurrentState` rows below are about the filter and not about timing.
    tokio::time::sleep(PAST_SETTLE).await;

    // (query string, the names that match it)
    let rows: Vec<(&str, Vec<&str>)> = vec![
        (
            "",
            vec!["alpha-one", "alpha-two", "beta-two", "beta.one_x"],
        ),
        ("?NamePrefix=alpha", vec!["alpha-one", "alpha-two"]),
        ("?NamePrefix=beta", vec!["beta-two", "beta.one_x"]),
        ("?NamePrefix=alpha-one", vec!["alpha-one"]),
        ("?NamePrefix=zzz", vec![]),
        ("?DesiredState=RUNNING", vec!["alpha-one", "beta.one_x"]),
        ("?DesiredState=STOPPED", vec!["alpha-two", "beta-two"]),
        ("?CurrentState=RUNNING", vec!["alpha-one", "beta.one_x"]),
        ("?CurrentState=STOPPED", vec!["alpha-two", "beta-two"]),
        ("?CurrentState=CREATING", vec![]),
        (
            "?SourcePrefix=arn:aws:sqs",
            vec!["alpha-one", "alpha-two"],
        ),
        ("?SourcePrefix=smk://", vec!["beta-two"]),
        (
            "?SourcePrefix=arn:aws:kinesis:us-west-2:000000000000:stream/events",
            vec!["beta.one_x"],
        ),
        (
            "?TargetPrefix=arn:aws:lambda",
            vec!["alpha-one", "beta.one_x"],
        ),
        (
            "?NamePrefix=alpha&DesiredState=STOPPED",
            vec!["alpha-two"],
        ),
        (
            "?NamePrefix=beta&TargetPrefix=arn:aws:lambda",
            vec!["beta.one_x"],
        ),
        ("?NamePrefix=alpha&SourcePrefix=smk://", vec![]),
    ];

    for (query, expected) in rows {
        let listed = list(&router, query).await;
        assert_eq!(
            listed.status, 200,
            "case {query:?}: ListPipes must succeed, got {listed:?}"
        );
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {query:?}: expected {expected:?}, got {:?}",
            listed_names(&listed)
        );
        assert_eq!(
            listed.member("NextToken"),
            &Value::Null,
            "case {query:?}: a single page must carry no NextToken, got {listed:?}"
        );
    }
}

/// RULE: `Limit` bounds one page and `NextToken` resumes immediately after the last pipe
/// returned, so paging a query at ANY limit enumerates every match exactly once, in name
/// order, and the last page carries no token.
#[tokio::test]
async fn list_pipes_pages_through_next_token_at_every_limit() {
    let router = router();
    let all: Vec<String> = (1..=7).map(|n| format!("paged-{n:02}")).collect();
    for name in &all {
        let created = create(
            &router,
            name,
            minimal_body(
                "arn:aws:sqs:us-west-2:000000000000:orders",
                "arn:aws:lambda:us-west-2:000000000000:function:handler",
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "fixture {name:?}: CreatePipe failed with {created:?}"
        );
    }

    // Each limit is a row: 1 (a page per pipe), 2 and 3 (a partial last page), 7 (one
    // page holding everything) and 100 (the modelled maximum, `LimitMax100`).
    for limit in [1_usize, 2, 3, 7, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("?Limit={limit}"),
                Some(token) => format!("?Limit={limit}&NextToken={token}"),
            };
            let listed = list(&router, &query).await;
            assert_eq!(
                listed.status, 200,
                "case limit={limit}: ListPipes must succeed, got {listed:?}"
            );
            let names = listed_names(&listed);
            assert!(
                names.len() <= limit,
                "case limit={limit}: a page must hold at most {limit} pipes, got {names:?}"
            );
            seen.extend(names.clone());
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case limit={limit}: paging did not terminate after {pages} pages"
            );
            match listed.member("NextToken").as_str() {
                Some(next) => {
                    assert!(
                        seen.len() < all.len(),
                        "case limit={limit}: a NextToken was issued after every pipe was \
                         returned, got {listed:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, all,
            "case limit={limit}: paging must enumerate every pipe once in name order, got {seen:?}"
        );
    }
}

/// RULE: `DeletePipe` removes the pipe, reports its final record, and leaves nothing for
/// a later reader — so every reader and a second delete agree it is gone.
#[tokio::test]
async fn delete_pipe_removes_the_pipe_from_every_reader() {
    let router = router();
    for name in ["kept", "doomed"] {
        let created = create(
            &router,
            name,
            minimal_body(
                "arn:aws:sqs:us-west-2:000000000000:orders",
                "arn:aws:lambda:us-west-2:000000000000:function:handler",
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "fixture {name:?}: CreatePipe failed with {created:?}"
        );
    }

    let deleted = call(&router, "DELETE", "/v1/pipes/doomed", None, REGION).await;
    assert_eq!(
        deleted.status, 200,
        "DeletePipe must succeed, got {deleted:?}"
    );
    assert_eq!(
        deleted.string("DesiredState"),
        "STOPPED",
        "DeletePipe must report the run state the pipe is wound down to, got {deleted:?}"
    );
    assert_eq!(
        deleted.string("CurrentState"),
        "DELETING",
        "DeletePipe must report CurrentState DELETING, got {deleted:?}"
    );
    assert_eq!(
        deleted.string("Arn"),
        format!(
            "arn:aws:pipes:{REGION}:{}:pipe/doomed",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "DeletePipe must report the stored ARN, got {deleted:?}"
    );
    assert!(
        deleted.number("CreationTime") > 0.0,
        "DeletePipe must report the stored CreationTime, got {deleted:?}"
    );

    // Every reader, and a second delete, now agree the pipe is gone — and the pipe that
    // was not deleted is untouched.
    let described = describe(&router, "doomed").await;
    assert_eq!(
        (described.status, described.error_code.as_str()),
        (404, "NotFoundException"),
        "DescribePipe after a delete must be 404 NotFoundException, got {described:?}"
    );
    let listed = list(&router, "").await;
    assert_eq!(
        listed_names(&listed),
        vec!["kept".to_string()],
        "ListPipes must no longer report the deleted pipe, got {listed:?}"
    );
    let again = call(&router, "DELETE", "/v1/pipes/doomed", None, REGION).await;
    assert_eq!(
        (again.status, again.error_code.as_str()),
        (404, "NotFoundException"),
        "a second DeletePipe must be 404 NotFoundException, got {again:?}"
    );
}

/// RULE: an operation naming a pipe that is not stored is `NotFoundException` (404), and
/// creating a name that IS stored is `ConflictException` (409).
#[tokio::test]
async fn unknown_pipe_is_not_found_and_a_taken_name_conflicts() {
    let router = router();
    let body = minimal_body(
        "arn:aws:sqs:us-west-2:000000000000:orders",
        "arn:aws:lambda:us-west-2:000000000000:function:handler",
    );
    assert_eq!(
        create(&router, "taken", body.clone()).await.status,
        200,
        "the fixture pipe must be created"
    );

    // Every operation that names one pipe, over a name nothing created.
    let rows: Vec<(&str, &str)> = vec![
        ("GET", "/v1/pipes/never-created"),
        ("POST", "/v1/pipes/never-created/start"),
        ("POST", "/v1/pipes/never-created/stop"),
        ("DELETE", "/v1/pipes/never-created"),
    ];
    for (method, uri) in rows {
        let reply = call(&router, method, uri, None, REGION).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {method} {uri}: expected 404 NotFoundException, got {reply:?}"
        );
    }

    // A second CreatePipe for a stored name is refused, whatever the rest of the request
    // says, and the stored record is left as it was.
    let mut changed = body.clone();
    changed["Description"] = json!("a different request");
    for (case, attempt) in [("identical", body), ("changed", changed)] {
        let conflict = create(&router, "taken", attempt).await;
        assert_eq!(
            (conflict.status, conflict.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {conflict:?}"
        );
    }
    let described = describe(&router, "taken").await;
    assert_eq!(
        described.member("Description"),
        &Value::Null,
        "a refused CreatePipe must not have changed the stored pipe, got {described:?}"
    );
}

/// RULE: a request outside a modelled constraint, or carrying a `NextToken` this service
/// did not issue, is refused with `ValidationException` (400) — never accepted silently.
#[tokio::test]
async fn requests_outside_a_modelled_constraint_are_refused() {
    let router = router();
    let body = minimal_body(
        "arn:aws:sqs:us-west-2:000000000000:orders",
        "arn:aws:lambda:us-west-2:000000000000:function:handler",
    );
    assert_eq!(
        create(&router, "valid", body.clone()).await.status,
        200,
        "the fixture pipe must be created"
    );

    // (case, method, uri, body). Each row is outside one modelled constraint: the
    // `Limit` range, the `PipeName` length and pattern, the `RequestedPipeState` enum,
    // or the `NextToken` this service issues.
    let rows: Vec<(&str, &str, String, Option<Value>)> = vec![
        ("Limit below the range", "GET", "/v1/pipes?Limit=0".into(), None),
        ("Limit above the range", "GET", "/v1/pipes?Limit=101".into(), None),
        ("Limit not a number", "GET", "/v1/pipes?Limit=many".into(), None),
        (
            "NextToken this service never issued",
            "GET",
            "/v1/pipes?NextToken=not-a-token".into(),
            None,
        ),
        (
            "NextToken that is not text",
            "GET",
            "/v1/pipes?NextToken=ff".into(),
            None,
        ),
        (
            "DesiredState outside RequestedPipeState",
            "POST",
            "/v1/pipes/paused-please".into(),
            Some({
                let mut body = body.clone();
                body["DesiredState"] = json!("PAUSED");
                body
            }),
        ),
        (
            "name outside the PipeName pattern",
            "GET",
            "/v1/pipes/bad~name".into(),
            None,
        ),
        (
            "name longer than PipeName allows",
            "GET",
            format!("/v1/pipes/{}", "p".repeat(65)),
            None,
        ),
    ];

    for (case, method, uri, payload) in rows {
        let reply = call(&router, method, &uri, payload, REGION).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {reply:?}"
        );
    }

    // The refused CreatePipe stored nothing.
    let described = describe(&router, "paused-please").await;
    assert_eq!(
        described.status, 404,
        "a refused CreatePipe must not have stored a pipe, got {described:?}"
    );
    // A token this service DID issue is accepted, so the rows above refuse the token
    // rather than refusing paging.
    let first = list(&router, "?Limit=1").await;
    assert_eq!(
        first.status, 200,
        "the fixture list call must succeed, got {first:?}"
    );
}

/// RULE: the store holds at most `MAX_PIPES` records — past the cap `CreatePipe` is
/// refused with `ServiceQuotaExceededException` (402) and stores nothing, so no caller can
/// grow the store without limit, while the pipes already stored stay readable and a
/// delete frees room again.
#[tokio::test]
async fn create_pipe_stops_at_the_record_cap() {
    let router = router();
    let body = minimal_body(
        "arn:aws:sqs:us-west-2:000000000000:orders",
        "arn:aws:lambda:us-west-2:000000000000:function:handler",
    );
    for index in 0..MAX_PIPES {
        let name = format!("capped-{index:04}");
        let created = create(&router, &name, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: a create within the cap must succeed, got {created:?}"
        );
    }

    // Three rows past the cap, differing in the name they ask for: every one is refused.
    for name in ["capped-9999", "one.more", "x"] {
        let refused = create(&router, name, body.clone()).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (402, "ServiceQuotaExceededException"),
            "case {name:?}: past the cap of {MAX_PIPES} expected 402 \
             ServiceQuotaExceededException, got {refused:?}"
        );
        let described = describe(&router, name).await;
        assert_eq!(
            described.status, 404,
            "case {name:?}: a refused create must store nothing, got {described:?}"
        );
    }

    // The pipes already stored are untouched, and freeing a record makes room again.
    let listed = list(&router, &format!("?Limit={MAX_PIPES}")).await;
    assert_eq!(
        listed_names(&listed).len(),
        MAX_PIPES,
        "the store must still hold exactly {MAX_PIPES} pipes, got {listed:?}"
    );
    let deleted = call(&router, "DELETE", "/v1/pipes/capped-0000", None, REGION).await;
    assert_eq!(
        deleted.status, 200,
        "DeletePipe must free a record, got {deleted:?}"
    );
    let after_delete = create(&router, "one.more", body).await;
    assert_eq!(
        after_delete.status, 200,
        "a create below the cap again must succeed, got {after_delete:?}"
    );
}
