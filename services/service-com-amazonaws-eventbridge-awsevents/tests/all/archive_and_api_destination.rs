// SPDX-License-Identifier: CC0-1.0

//! The archive and API-destination resources end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through
//! the real `MultiServiceRouter`: `CreateArchive` writes an archive,
//! `DescribeArchive` reports it and `PutEvents` captures into it;
//! `CreateApiDestination` writes a destination and `DescribeApiDestination`
//! reports it.
//!
//! The RULES asserted here:
//!
//! * a created archive is a PERSISTED resource bound to ONE event bus: it
//!   answers an ARN of the modelled `ArchiveArn` form (`…:archive/<name>`) and
//!   reads back under its name with every member it was created with;
//! * an archive is provisioned by the call that writes it: `CreateArchive`
//!   answers it in the settled `ENABLED` state and a read taken straight after
//!   the create reports that same state, so there is no transient state to poll
//!   out of;
//! * an `ENABLED` archive captures exactly the events of ITS bus that its own
//!   `EventPattern` matches, and "if you do not specify a pattern to filter
//!   events sent to the archive, all events are sent to the archive" — so
//!   `EventCount` and `SizeBytes` are counted off what it holds;
//! * an archive is served by a MANAGED rule on its source bus, whose
//!   `EventPattern` mirrors the archive's: `ListRules` reports it, "you cannot
//!   modify these managed rules by using DisableRule, EnableRule, PutTargets,
//!   PutRule, TagResource, or UntagResource" (`ManagedRuleException`), a
//!   `RemoveTargets` or `DeleteRule` without `Force` is refused the same way, and
//!   the rule survives every refused write;
//! * a created API destination is a PERSISTED resource bound to ONE connection,
//!   and its `ApiDestinationState` FOLLOWS the authorization that connection
//!   settles on: a destination whose connection holds (or is on its way to
//!   holding) authorization is `ACTIVE`, and one whose connection ends up with
//!   none is `INACTIVE`;
//! * each resource's name is its identity (a second create is
//!   `ResourceAlreadyExistsException`), each resource it points at has to exist
//!   (`ResourceNotFoundException`), and a filter or setting the model does not
//!   admit is refused rather than stored.
//!
//! Each table spans the class rather than one recorded case: the rows differ
//! along whether the archive filters, its retention and encryption, the bus it is
//! bound to, the destination's HTTP method, rate limit and endpoint form, and
//! which member of a request is wrong.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-central-1";

/// How long a test waits before reading a resource whose next state the clock
/// owes it: the span itself plus enough slack that a loaded machine does not make
/// the wait the flaky part of the assertion.
fn past_the_transition(span: Duration) -> Duration {
    span + Duration::from_millis(250)
}

/// How long a test keeps LOOKING for a state the clock owes a resource before it
/// gives up: generous, because what is waited on is the condition and not a
/// duration, so a loaded machine cannot make the wait the flaky part of an
/// assertion.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long between two looks at a resource whose next state is still owed.
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// The destination `name`, looked at again until its `ApiDestinationState` is
/// `expected` or [`SETTLE_DEADLINE`] passes; answers the last read either way, so
/// the caller's assertion names what was actually reported.
async fn until_destination_state(
    router: &MultiServiceRouter,
    name: &str,
    expected: &str,
) -> Value {
    let looks = SETTLE_DEADLINE.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    let mut last = Value::Null;
    for _ in 0..looks {
        let (status, described) =
            events(router, "DescribeApiDestination", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "DescribeApiDestination answered {status} {described}"
        );
        if described["ApiDestinationState"] == json!(expected) {
            return described;
        }
        last = described;
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    last
}

/// One `awsJson1_1` call to EventBridge over the router.
async fn events(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// The wire error code of a failure body (`__type` on awsJson), without its
/// namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// The ARN of the bus called `name`, as this service mints it.
fn bus_arn(name: &str) -> String {
    format!(
        "arn:aws:events:{REGION}:{}:event-bus/{name}",
        engine::emulator_account_id()
    )
}

/// A custom event bus called `name`.
async fn given_bus(router: &MultiServiceRouter, name: &str) {
    let (status, body) = events(router, "CreateEventBus", &json!({ "Name": name })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");
}

/// An API-key connection called `name`, already settled into `AUTHORIZED` when
/// `settled`; answers its ARN.
async fn given_connection(router: &MultiServiceRouter, name: &str, settled: bool) -> String {
    let (status, body) = events(
        router,
        "CreateConnection",
        &json!({
            "Name": name,
            "AuthorizationType": "API_KEY",
            "AuthParameters": {
                "ApiKeyAuthParameters": { "ApiKeyName": "x-api-key", "ApiKeyValue": "sekrit" },
            },
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateConnection answered {status} {body}");
    if settled {
        tokio::time::sleep(past_the_transition(state::CONNECTION_TRANSITION)).await;
    }
    body["ConnectionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no ConnectionArn in {body}"))
        .to_string()
}

/// Publish one event with `source` onto `bus`.
async fn publish(router: &MultiServiceRouter, case: &str, bus: &str, source: &str) {
    let (status, body) = events(
        router,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": bus,
                "Source": source,
                "DetailType": "Thing Happened",
                "Detail": r#"{"size":1}"#,
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutEvents reported a failed entry: {body}"
    );
}

/// One archive the create table writes.
struct ArchiveCase {
    case: &'static str,
    /// The bus the archive is bound to: `None` for the account's default bus.
    bus: Option<&'static str>,
    /// Extra members of the `CreateArchive` request.
    extra: Value,
}

/// A created archive carries an ARN of the modelled form, reads back under its
/// name with every member it was created with, and is `ENABLED` from the create
/// on.
#[tokio::test]
async fn a_created_archive_is_read_back_and_is_enabled_from_the_create_on() {
    let cases = [
        ArchiveCase {
            case: "the default bus, no filter and no options",
            bus: None,
            extra: json!({}),
        },
        ArchiveCase {
            case: "a custom bus, a filtering pattern and a description",
            bus: Some("archived-bus"),
            extra: json!({
                "Description": "the orders archive",
                "EventPattern": r#"{"source":["com.demo.orders"]}"#,
            }),
        },
        ArchiveCase {
            case: "a retention window and a customer managed key",
            bus: Some("kept-bus"),
            extra: json!({ "RetentionDays": 7, "KmsKeyIdentifier": "archive-key" }),
        },
    ];

    for ArchiveCase { case, bus, extra } in cases {
        let router = emulator();
        let bus_name = bus.unwrap_or(state::DEFAULT_EVENT_BUS_NAME);
        if let Some(bus) = bus {
            given_bus(&router, bus).await;
        }
        let name = "the-archive";
        let mut request = json!({ "ArchiveName": name, "EventSourceArn": bus_arn(bus_name) });
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            request[member] = value.clone();
        }

        let (status, created) = events(&router, "CreateArchive", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateArchive answered {status} {created}"
        );
        let expected_arn = format!(
            "arn:aws:events:{REGION}:{}:archive/{name}",
            engine::emulator_account_id()
        );
        assert_eq!(
            created["ArchiveArn"],
            json!(expected_arn),
            "case {case:?}: expected the ARN {expected_arn:?}, got {created}"
        );
        assert_eq!(
            created["State"],
            json!(state::ARCHIVE_STATE_ENABLED),
            "case {case:?}: a created archive is answered in {}, got {created}",
            state::ARCHIVE_STATE_ENABLED
        );
        assert!(
            created["CreationTime"].as_f64().is_some(),
            "case {case:?}: expected a CreationTime on the create, got {created}"
        );

        // The settled state is what a read taken STRAIGHT after the create
        // reports: there is no transient state to poll out of.
        let (status, settled) =
            events(&router, "DescribeArchive", &json!({ "ArchiveName": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeArchive answered {status} {settled}"
        );
        assert_eq!(
            settled["State"],
            json!(state::ARCHIVE_STATE_ENABLED),
            "case {case:?}: a read taken at once reports {}, got {settled}",
            state::ARCHIVE_STATE_ENABLED
        );
        assert_eq!(
            settled["ArchiveArn"],
            json!(expected_arn),
            "case {case:?}: a read reports the stored ARN, got {settled}"
        );
        assert_eq!(
            settled["ArchiveName"],
            json!(name),
            "case {case:?}: a read reports the stored name, got {settled}"
        );
        assert_eq!(
            settled["EventSourceArn"],
            json!(bus_arn(bus_name)),
            "case {case:?}: a read reports the bus the archive is bound to, got {settled}"
        );
        assert_eq!(
            settled["EventCount"],
            json!(0),
            "case {case:?}: an archive that captured nothing counts nothing, got {settled}"
        );
        assert_eq!(
            settled["SizeBytes"],
            json!(0),
            "case {case:?}: an archive that captured nothing holds no bytes, got {settled}"
        );
        for (member, expected) in extra.as_object().expect("the extra members are an object") {
            if member == "KmsKeyIdentifier" {
                // An identifier that is not already an ARN names a key in this
                // account and region, which is what the archive stores.
                let reported = settled[member].as_str().unwrap_or_default();
                assert!(
                    reported.starts_with("arn:aws:kms:")
                        && reported.ends_with(expected.as_str().unwrap_or_default()),
                    "case {case:?}: expected {member} to name the key in this account, \
                     got {reported:?}"
                );
                continue;
            }
            assert_eq!(
                settled[member], *expected,
                "case {case:?}: expected {member} to read back as {expected}, got {settled}"
            );
        }

        // Nothing about the archive moves because it was read.
        let (_, again) = events(&router, "DescribeArchive", &json!({ "ArchiveName": name })).await;
        for member in ["ArchiveArn", "State", "CreationTime", "EventCount", "SizeBytes"] {
            assert_eq!(
                again[member], settled[member],
                "case {case:?}: {member} must not change because the archive was read, \
                 got {again}"
            );
        }
    }
}

/// An `ENABLED` archive captures the events of its OWN bus that its own pattern
/// matches, and nothing else.
#[tokio::test]
async fn an_enabled_archive_captures_only_what_its_pattern_matches_on_its_own_bus() {
    let router = emulator();
    let archived = "watched-bus";
    let ignored = "unwatched-bus";
    given_bus(&router, archived).await;
    given_bus(&router, ignored).await;

    // (the archive, the bus it is bound to, its pattern)
    let given: [(&str, &str, Option<&str>); 3] = [
        ("everything", archived, None),
        (
            "orders-only",
            archived,
            Some(r#"{"source":["com.demo.orders"]}"#),
        ),
        ("other-bus", ignored, None),
    ];
    for (name, bus, pattern) in given {
        let mut request = json!({ "ArchiveName": name, "EventSourceArn": bus_arn(bus) });
        if let Some(pattern) = pattern {
            request["EventPattern"] = json!(pattern);
        }
        let (status, body) = events(&router, "CreateArchive", &request).await;
        assert_eq!(status, 200, "CreateArchive {name} answered {status} {body}");
    }

    // An archive is `ENABLED` from the create on, so an event published straight
    // after it is captured.
    publish(&router, "straight after the create", archived, "com.demo.orders").await;
    // (the archive, how many of that first event it holds)
    let immediate: [(&str, i64); 3] = [("everything", 1), ("orders-only", 1), ("other-bus", 0)];
    for (name, count) in immediate {
        let (status, body) = events(&router, "DescribeArchive", &json!({ "ArchiveName": name })).await;
        assert_eq!(status, 200, "DescribeArchive {name} answered {status} {body}");
        assert_eq!(
            body["EventCount"],
            json!(count),
            "case {name:?}: expected {count} captured events straight after the create, \
             got {body}"
        );
    }

    publish(&router, "orders", archived, "com.demo.orders").await;
    publish(&router, "another order", archived, "com.demo.orders").await;
    publish(&router, "shipments", archived, "com.demo.shipments").await;
    publish(&router, "on the other bus", ignored, "com.demo.orders").await;

    // (the archive, how many events it holds)
    let expected: [(&str, i64); 3] = [("everything", 4), ("orders-only", 3), ("other-bus", 1)];
    for (name, count) in expected {
        let (status, body) =
            events(&router, "DescribeArchive", &json!({ "ArchiveName": name })).await;
        assert_eq!(status, 200, "DescribeArchive {name} answered {status} {body}");
        assert_eq!(
            body["EventCount"],
            json!(count),
            "case {name:?}: expected {count} captured events, got {body}"
        );
        let size = body["SizeBytes"].as_i64().unwrap_or_default();
        assert!(
            size > 0,
            "case {name:?}: an archive holding events holds bytes, got {body}"
        );
    }

    // The filtered archive holds strictly less than the unfiltered one on the
    // same bus, because it kept a subset of the same envelopes.
    let (_, everything) = events(
        &router,
        "DescribeArchive",
        &json!({ "ArchiveName": "everything" }),
    )
    .await;
    let (_, filtered) = events(
        &router,
        "DescribeArchive",
        &json!({ "ArchiveName": "orders-only" }),
    )
    .await;
    assert!(
        filtered["SizeBytes"].as_i64().unwrap_or_default()
            < everything["SizeBytes"].as_i64().unwrap_or_default(),
        "a filtering archive holds less than one that keeps everything, got {filtered} \
         and {everything}"
    );
}

/// An archive is served by a MANAGED rule on its source bus: `ListRules` reports
/// it, every customer write to it is refused, and it survives those attempts.
#[tokio::test]
async fn an_archive_is_served_by_a_managed_rule_no_customer_write_can_touch() {
    let router = emulator();
    let bus = "guarded-bus";
    given_bus(&router, bus).await;
    let archive = "guarded-archive";
    let pattern = r#"{"source":["com.demo.orders"]}"#;
    let (status, body) = events(
        &router,
        "CreateArchive",
        &json!({
            "ArchiveName": archive,
            "EventSourceArn": bus_arn(bus),
            "EventPattern": pattern,
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateArchive answered {status} {body}");

    let rule = format!("Events-Archive-{archive}");
    let (status, listed) = events(&router, "ListRules", &json!({ "EventBusName": bus })).await;
    assert_eq!(status, 200, "ListRules answered {status} {listed}");
    let names: Vec<&str> = listed["Rules"]
        .as_array()
        .map(|rules| {
            rules
                .iter()
                .map(|found| found["Name"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.contains(&rule.as_str()),
        "expected ListRules to report {rule:?}, got {names:?}"
    );

    let (status, described) = events(
        &router,
        "DescribeRule",
        &json!({ "Name": rule, "EventBusName": bus }),
    )
    .await;
    assert_eq!(status, 200, "DescribeRule answered {status} {described}");
    // The rule matches what the archive matches, and never a replayed event:
    // "all events are sent to the archive except replayed events".
    let carried: Value = serde_json::from_str(described["EventPattern"].as_str().unwrap_or("null"))
        .unwrap_or(Value::Null);
    assert_eq!(
        carried,
        json!({
            "source": ["com.demo.orders"],
            "replay-name": [{ "exists": false }],
        }),
        "the rule's pattern mirrors the archive's and excludes replays, got {described}"
    );
    assert!(
        described["ManagedBy"].as_str().is_some_and(|owner| !owner.is_empty()),
        "the rule is managed, so it names the service that manages it, got {described}"
    );

    // Its one target is the archive the selected events go to.
    let (status, targets) = events(
        &router,
        "ListTargetsByRule",
        &json!({ "Rule": rule, "EventBusName": bus }),
    )
    .await;
    assert_eq!(status, 200, "ListTargetsByRule answered {status} {targets}");
    let ids: Vec<&str> = targets["Targets"]
        .as_array()
        .map(|found| {
            found
                .iter()
                .map(|target| target["Id"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![rule.as_str()],
        "expected exactly one target named after the rule, got {targets}"
    );
    assert_eq!(
        targets["Targets"][0]["Arn"],
        json!(format!(
            "arn:aws:events:{REGION}:{}:archive/{archive}",
            engine::emulator_account_id()
        )),
        "the target is the archive itself, got {targets}"
    );

    // (case, operation, request) — every customer write the documentation names.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a disable of the managed rule",
            "DisableRule",
            json!({ "Name": rule, "EventBusName": bus }),
        ),
        (
            "an enable of the managed rule",
            "EnableRule",
            json!({ "Name": rule, "EventBusName": bus }),
        ),
        (
            "a PutRule replacing the managed rule",
            "PutRule",
            json!({
                "Name": rule,
                "EventBusName": bus,
                "EventPattern": r#"{"source":["com.demo.anything"]}"#,
            }),
        ),
        (
            "a PutTargets onto the managed rule",
            "PutTargets",
            json!({
                "Rule": rule,
                "EventBusName": bus,
                "Targets": [{
                    "Id": "mine",
                    "Arn": format!(
                        "arn:aws:sqs:{REGION}:{}:a-queue",
                        engine::emulator_account_id()
                    ),
                }],
            }),
        ),
        (
            "a RemoveTargets from the managed rule without Force",
            "RemoveTargets",
            json!({ "Rule": rule, "EventBusName": bus, "Ids": ["mine"] }),
        ),
        (
            "a DeleteRule of the managed rule without Force",
            "DeleteRule",
            json!({ "Name": rule, "EventBusName": bus }),
        ),
        (
            "a TagResource on the managed rule",
            "TagResource",
            json!({
                "ResourceARN": format!(
                    "arn:aws:events:{REGION}:{}:rule/{bus}/{rule}",
                    engine::emulator_account_id()
                ),
                "Tags": [{ "Key": "owner", "Value": "me" }],
            }),
        ),
        (
            "an UntagResource on the managed rule",
            "UntagResource",
            json!({
                "ResourceARN": format!(
                    "arn:aws:events:{REGION}:{}:rule/{bus}/{rule}",
                    engine::emulator_account_id()
                ),
                "TagKeys": ["owner"],
            }),
        ),
    ];

    for (case, operation, request) in rows {
        let (status, body) = events(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ManagedRuleException",
            "case {case:?}: expected ManagedRuleException, got {body}"
        );
    }

    // It survives every one of them, unchanged.
    let (status, again) = events(
        &router,
        "DescribeRule",
        &json!({ "Name": rule, "EventBusName": bus }),
    )
    .await;
    assert_eq!(status, 200, "DescribeRule answered {status} {again}");
    for member in ["Arn", "EventPattern", "State", "ManagedBy"] {
        assert_eq!(
            again[member], described[member],
            "the managed rule must not change because a write was refused, got {again}"
        );
    }
}

/// What the archive operations refuse, and with which modelled error.
#[tokio::test]
async fn the_archive_operations_refuse_what_the_resource_rules_forbid() {
    let router = emulator();
    let (status, body) = events(
        &router,
        "CreateArchive",
        &json!({
            "ArchiveName": "taken",
            "EventSourceArn": bus_arn(state::DEFAULT_EVENT_BUS_NAME),
        }),
    )
    .await;
    assert_eq!(status, 200, "the colliding create answered {status} {body}");

    // (case, operation, request, expected status, expected wire code)
    let rows: [(&str, &str, Value, u16, &str); 6] = [
        (
            "a second archive under one name",
            "CreateArchive",
            json!({
                "ArchiveName": "taken",
                "EventSourceArn": bus_arn(state::DEFAULT_EVENT_BUS_NAME),
            }),
            400,
            "ResourceAlreadyExistsException",
        ),
        (
            "an event-bus ARN naming a bus that does not exist",
            "CreateArchive",
            json!({ "ArchiveName": "orphan", "EventSourceArn": bus_arn("ghost-bus") }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "an EventSourceArn that is not an event-bus ARN",
            "CreateArchive",
            json!({
                "ArchiveName": "misdirected",
                "EventSourceArn": format!(
                    "arn:aws:events:{REGION}:{}:rule/some-rule",
                    engine::emulator_account_id()
                ),
            }),
            400,
            "ValidationException",
        ),
        (
            "a pattern whose leaf value is not wrapped in an array",
            "CreateArchive",
            json!({
                "ArchiveName": "badly-filtered",
                "EventSourceArn": bus_arn(state::DEFAULT_EVENT_BUS_NAME),
                "EventPattern": r#"{"source":"com.demo.orders"}"#,
            }),
            400,
            "InvalidEventPatternException",
        ),
        (
            "a pattern that is not JSON at all",
            "CreateArchive",
            json!({
                "ArchiveName": "unparseable",
                "EventSourceArn": bus_arn(state::DEFAULT_EVENT_BUS_NAME),
                "EventPattern": "{",
            }),
            400,
            "InvalidEventPatternException",
        ),
        (
            "a read of an archive that does not exist",
            "DescribeArchive",
            json!({ "ArchiveName": "absent" }),
            400,
            "ResourceNotFoundException",
        ),
    ];

    for (case, operation, request, expected_status, expected_code) in rows {
        let (status, body) = events(&router, operation, &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the wire code {expected_code}, got {body}"
        );
    }

    // A refused create stored nothing.
    for name in ["orphan", "misdirected", "badly-filtered", "unparseable"] {
        let (status, body) =
            events(&router, "DescribeArchive", &json!({ "ArchiveName": name })).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a refused create must store no archive, got {status} {body}"
        );
    }

    // A bus an archive still sends its events to cannot be deleted: the archive
    // would be left pointing at nothing.
    let bus = "fed-bus";
    given_bus(&router, bus).await;
    let (status, body) = events(
        &router,
        "CreateArchive",
        &json!({ "ArchiveName": "fed-archive", "EventSourceArn": bus_arn(bus) }),
    )
    .await;
    assert_eq!(status, 200, "CreateArchive answered {status} {body}");
    let (status, body) = events(&router, "DeleteEventBus", &json!({ "Name": bus })).await;
    assert_eq!(
        status, 400,
        "a bus an archive targets must not be deleted, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException refusing the delete, got {body}"
    );
    let (status, body) = events(&router, "DescribeEventBus", &json!({ "Name": bus })).await;
    assert_eq!(
        status, 200,
        "the refused delete leaves the bus in place, got {status} {body}"
    );
}

/// A created API destination reads back with every member it was created with,
/// and its state follows the connection it invokes through.
#[tokio::test]
async fn a_created_api_destination_is_read_back_and_follows_its_connection() {
    // (case, extra members of the create, the rate the read reports)
    let rows: [(&str, Value, i64); 3] = [
        (
            "no rate limit, so the account default applies",
            json!({}),
            i64::from(state::DEFAULT_API_DESTINATION_RATE_LIMIT),
        ),
        (
            "a rate limit of its own, and a GET",
            json!({ "InvocationRateLimitPerSecond": 10, "HttpMethod": "GET" }),
            10,
        ),
        (
            "an endpoint that is not a URL, which the model admits",
            json!({ "InvocationEndpoint": "api.example.com/events", "Description": "no scheme" }),
            i64::from(state::DEFAULT_API_DESTINATION_RATE_LIMIT),
        ),
    ];

    for (case, extra, expected_rate) in rows {
        let router = emulator();
        let connection = given_connection(&router, "the-connection", true).await;
        let name = "the-destination";
        let mut request = json!({
            "Name": name,
            "ConnectionArn": connection,
            "InvocationEndpoint": "https://api.example.com/events",
            "HttpMethod": "POST",
        });
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            request[member] = value.clone();
        }

        let (status, created) = events(&router, "CreateApiDestination", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApiDestination answered {status} {created}"
        );
        // `ApiDestinationArn`'s modelled pattern ends
        // `:api-destination/<name>/<id>`, where the id segment is `[\-A-Za-z0-9]+`.
        let arn = created["ApiDestinationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no ApiDestinationArn in {created}"))
            .to_string();
        let prefix = format!(
            "arn:aws:events:{REGION}:{}:api-destination/{name}/",
            engine::emulator_account_id()
        );
        let id = arn.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}")
        });
        assert!(
            !id.is_empty()
                && id
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-'),
            "case {case:?}: expected a hyphen-and-alphanumeric id segment, got {id:?}"
        );
        assert_eq!(
            created["ApiDestinationState"],
            json!(state::API_DESTINATION_STATE_ACTIVE),
            "case {case:?}: a destination on an authorized connection is {}, got {created}",
            state::API_DESTINATION_STATE_ACTIVE
        );

        let (status, described) =
            events(&router, "DescribeApiDestination", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeApiDestination answered {status} {described}"
        );
        assert_eq!(
            described["ApiDestinationArn"],
            json!(arn),
            "case {case:?}: a read reports the stored ARN, got {described}"
        );
        assert_eq!(
            described["Name"],
            json!(name),
            "case {case:?}: a read reports the stored name, got {described}"
        );
        assert_eq!(
            described["ConnectionArn"],
            json!(connection),
            "case {case:?}: a read reports the stored connection, got {described}"
        );
        assert_eq!(
            described["InvocationRateLimitPerSecond"],
            json!(expected_rate),
            "case {case:?}: expected the rate {expected_rate}, got {described}"
        );
        for member in ["InvocationEndpoint", "HttpMethod", "Description"] {
            let expected = request
                .get(member)
                .cloned()
                .unwrap_or(Value::Null);
            if expected.is_null() {
                continue;
            }
            assert_eq!(
                described[member], expected,
                "case {case:?}: expected {member} to read back as {expected}, got {described}"
            );
        }
        for member in ["CreationTime", "LastModifiedTime"] {
            assert!(
                described[member].as_f64().is_some(),
                "case {case:?}: expected a {member} on the read, got {described}"
            );
        }
    }
}

/// The destination's state is COMPUTED from the authorization its connection
/// settles on: it is `ACTIVE` while that connection holds authorization and
/// `INACTIVE` once it has none.
#[tokio::test]
async fn the_destination_state_tracks_the_connections_own_state() {
    let router = emulator();
    // The connection's credentials ARE the authorization, so the create settled
    // it in AUTHORIZED.
    let connection = given_connection(&router, "slow-connection", false).await;
    let name = "tracking-destination";
    let (status, created) = events(
        &router,
        "CreateApiDestination",
        &json!({
            "Name": name,
            "ConnectionArn": connection,
            "InvocationEndpoint": "https://api.example.com/events",
            "HttpMethod": "POST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateApiDestination answered {status} {created}"
    );
    assert_eq!(
        created["ApiDestinationState"],
        json!(state::API_DESTINATION_STATE_ACTIVE),
        "a destination whose connection holds its authorization is {}, got {created}",
        state::API_DESTINATION_STATE_ACTIVE
    );

    // A read taken straight after the create reports the same: the connection's
    // credentials ARE the authorization, so there is no transition to wait out.
    let (status, described) = events(&router, "DescribeApiDestination", &json!({ "Name": name })).await;
    assert_eq!(
        status, 200,
        "DescribeApiDestination answered {status} {described}"
    );
    assert_eq!(
        described["ApiDestinationState"],
        json!(state::API_DESTINATION_STATE_ACTIVE),
        "an authorized connection makes the destination {}, got {described}",
        state::API_DESTINATION_STATE_ACTIVE
    );

    let (status, body) = events(
        &router,
        "DeauthorizeConnection",
        &json!({ "Name": "slow-connection" }),
    )
    .await;
    assert_eq!(status, 200, "DeauthorizeConnection answered {status} {body}");
    // The deauthorized connection reaches DEAUTHORIZED on its own, and carries
    // its destinations with it: the read is taken again until that has happened.
    let described =
        until_destination_state(&router, name, state::API_DESTINATION_STATE_INACTIVE).await;
    assert_eq!(
        described["ApiDestinationState"],
        json!(state::API_DESTINATION_STATE_INACTIVE),
        "a deauthorized connection makes the destination {}, got {described}",
        state::API_DESTINATION_STATE_INACTIVE
    );
}

/// What the API-destination operations refuse, and with which modelled error.
#[tokio::test]
async fn the_api_destination_operations_refuse_what_the_resource_rules_forbid() {
    let router = emulator();
    let connection = given_connection(&router, "shared-connection", true).await;
    let (status, body) = events(
        &router,
        "CreateApiDestination",
        &json!({
            "Name": "taken",
            "ConnectionArn": connection,
            "InvocationEndpoint": "https://api.example.com/events",
            "HttpMethod": "POST",
        }),
    )
    .await;
    assert_eq!(status, 200, "the colliding create answered {status} {body}");

    let unknown_connection = format!(
        "arn:aws:events:{REGION}:{}:connection/absent/0000-1111",
        engine::emulator_account_id()
    );
    // (case, operation, request, expected status, expected wire code)
    let rows: [(&str, &str, Value, u16, &str); 5] = [
        (
            "a second destination under one name",
            "CreateApiDestination",
            json!({
                "Name": "taken",
                "ConnectionArn": connection,
                "InvocationEndpoint": "https://api.example.com/events",
                "HttpMethod": "POST",
            }),
            400,
            "ResourceAlreadyExistsException",
        ),
        (
            "a connection ARN this account holds no connection for",
            "CreateApiDestination",
            json!({
                "Name": "unconnected",
                "ConnectionArn": unknown_connection,
                "InvocationEndpoint": "https://api.example.com/events",
                "HttpMethod": "POST",
            }),
            400,
            "ResourceNotFoundException",
        ),
        (
            "an HTTP method the enum does not have",
            "CreateApiDestination",
            json!({
                "Name": "odd-method",
                "ConnectionArn": connection,
                "InvocationEndpoint": "https://api.example.com/events",
                "HttpMethod": "TRACE",
            }),
            400,
            "ValidationException",
        ),
        (
            "a rate limit past the account's maximum",
            "CreateApiDestination",
            json!({
                "Name": "too-fast",
                "ConnectionArn": connection,
                "InvocationEndpoint": "https://api.example.com/events",
                "HttpMethod": "POST",
                "InvocationRateLimitPerSecond": state::MAX_API_DESTINATION_RATE_LIMIT + 1,
            }),
            400,
            "LimitExceededException",
        ),
        (
            "a read of a destination that does not exist",
            "DescribeApiDestination",
            json!({ "Name": "absent" }),
            400,
            "ResourceNotFoundException",
        ),
    ];

    for (case, operation, request, expected_status, expected_code) in rows {
        let (status, body) = events(&router, operation, &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the wire code {expected_code}, got {body}"
        );
    }

    // A refused create stored nothing.
    for name in ["unconnected", "odd-method", "too-fast"] {
        let (status, body) =
            events(&router, "DescribeApiDestination", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {name:?}: a refused create must store no destination, got {status} {body}"
        );
    }
}
