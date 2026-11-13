// SPDX-License-Identifier: CC0-1.0

//! A published event REACHES its bus's subscribers: end to end over the real
//! `MultiServiceRouter`, with Amazon SQS mounted beside EventBridge V2 on ONE
//! engine — so a delivery is asserted where a client observes it, in the
//! messages the subscriber's target queue holds.
//!
//! RULE: an event a bus accepts is delivered to each subscriber of that bus
//! whose `State` is `RUNNING` and that is not revoked and every one of whose
//! `FilterConfiguration` filters matches the event in the scope that filter
//! names — one delivery per accepted event per such subscriber, carrying the
//! event in the shape the subscriber's `Transformer` names — and to no other
//! subscriber.
//!
//! The rows span that class rather than one recorded case: they differ along the
//! SCOPE a filter reads (`DATA`, `METADATA`, `SYSTEM_METADATA`), along whether a
//! pattern matches, along the NUMBER of filters a subscriber ANDs, along the
//! subscriber's run state, along the transformer, along the number of events one
//! publish accepts and along the number of subscribers one bus holds.
//!
//! Nothing here waits on the wall clock: a publish performs the deliveries it
//! has made due before it answers, so a read that follows it is a read of a
//! settled queue. The one wait is for the bus's own provisioning transition,
//! which is polled for the CONDITION (`ACTIVE`) under a generous deadline.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use engine::InMemoryEngine;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Operation};

use service_com_amazonaws_eventbridgev2_awseventsv2 as eventsv2;

use eventsv2::ops::create_event_bus::CreateEventBus;
use eventsv2::ops::create_subscriber::CreateSubscriber;
use eventsv2::ops::describe_event_bus::DescribeEventBus;
use eventsv2::ops::describe_subscriber::DescribeSubscriber;
use eventsv2::ops::put_events::PutEvents;
use eventsv2::ops::types::{
    CreateEventBusInput, CreateSubscriberInput, DescribeEventBusInput, DescribeSubscriberInput,
    Filter, FilterConfiguration, InvokeConfiguration, PointInTimeConfiguration, PutEventsInput,
    PutEventsRequestEntry, PutEventsSystemMetadata, Transformer,
};

/// The account segment every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The role a subscriber's `InvokeConfiguration` names, of the modelled
/// `RoleArn` shape.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/event-delivery";

/// How long a poll waits for a condition the emulator owes: far above the
/// provisioning span a bus takes, so a loaded host has ample room.
const PATIENCE: Duration = Duration::from_secs(30);

/// How long one pass of a poll sleeps before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

// ---------------------------------------------------------------------------
// The emulator and the two wire harnesses
// ---------------------------------------------------------------------------

/// EventBridge V2 and Amazon SQS on ONE engine — the shape
/// `core/router/src/lib.rs` mounts them in, which is what makes a handler's
/// in-process callback reach the other service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(eventsv2::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// A SigV4 `Authorization` header naming `region` and `service`, which is how a
/// request names both.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One rpcv2Cbor call against EventBridge V2, answered as its HTTP status and
/// raw body.
async fn eventsv2_call<O: Operation>(
    router: &MultiServiceRouter,
    input: &O::Input,
) -> (u16, bytes::Bytes) {
    let codec = CborCodec::default();
    let mut serializer = codec.create_serializer();
    serializer
        .write_struct(O::INPUT_SCHEMA, input as &dyn SerializableStruct)
        .unwrap_or_else(|e| panic!("{}: encoding the input must succeed, got {e:?}", O::NAME));
    let body = serializer.finish();

    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/service/AWSEventsV2/operation/{}", O::NAME))
        .header("host", format!("eventsv2.{REGION}.amazonaws.com"))
        .header("authorization", authorization("events"))
        .header("content-type", "application/cbor")
        .header("smithy-protocol", "rpc-v2-cbor")
        .body(bytes::Bytes::from(body))
        .unwrap_or_else(|e| panic!("{}: the request must be well formed, got {e:?}", O::NAME));

    let response = router.dispatch(request).await;
    (response.status().as_u16(), response.into_body())
}

/// The same call, requiring a 200 and decoding the typed output.
async fn eventsv2_ok<O: Operation>(
    router: &MultiServiceRouter,
    input: &O::Input,
    case: &str,
) -> O::Output
where
    O::Output: DeserializableStruct,
{
    let (status, body) = eventsv2_call::<O>(router, input).await;
    assert_eq!(
        status,
        200,
        "case {case:?}: {} must answer 200, got {status} with body {body:?}",
        O::NAME
    );
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&body);
    O::Output::deserialize(&mut deserializer).unwrap_or_else(|e| {
        panic!(
            "case {case:?}: {}'s body must decode as its modelled output, got {e:?}",
            O::NAME
        )
    })
}

/// One awsJson1_0 call against Amazon SQS, answered as its status and body.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The same call, requiring a 200.
async fn sqs_ok(router: &MultiServiceRouter, operation: &str, body: &Value, case: &str) -> Value {
    let (status, answer) = sqs(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: SQS {operation} must answer 200, got {status} with {answer}"
    );
    answer
}

// ---------------------------------------------------------------------------
// The resources a row is built from
// ---------------------------------------------------------------------------

/// A queue called `name`: its url, and the ARN a subscriber targets it by.
async fn given_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    let created = sqs_ok(router, "CreateQueue", &json!({ "QueueName": name }), case).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateQueue reported no QueueUrl in {created}"))
        .to_string();
    let arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}");
    (url, arn)
}

/// A bus called `name`, polled until it is `ACTIVE` — the state
/// `CreateSubscriber` requires of it.
async fn given_active_bus(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = eventsv2_ok::<CreateEventBus<InMemoryEngine>>(
        router,
        &CreateEventBusInput {
            name: name.to_string(),
            ..Default::default()
        },
        case,
    )
    .await;
    let arn = created
        .event_bus_arn
        .unwrap_or_else(|| panic!("case {case:?}: CreateEventBus reported no EventBusArn"));

    let deadline = Instant::now() + PATIENCE;
    loop {
        let described = eventsv2_ok::<DescribeEventBus<InMemoryEngine>>(
            router,
            &DescribeEventBusInput {
                event_bus_arn: arn.clone(),
            },
            case,
        )
        .await;
        if described.state.as_deref() == Some("ACTIVE") {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the bus {arn} never became ACTIVE, last state {:?}",
            described.state
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What a create asks for beyond the subscriber's identity and its target: the
/// members a row varies.
#[derive(Default, Clone)]
struct Requested {
    filters: Option<FilterConfiguration>,
    state: Option<&'static str>,
    transformer: Option<Transformer>,
    starting_position: Option<&'static str>,
    point: Option<PointInTimeConfiguration>,
}

/// A subscriber on `event_bus_arn` delivering to `target_arn`.
async fn given_subscriber(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    event_bus_arn: &str,
    target_arn: &str,
    requested: Requested,
) -> String {
    let created = eventsv2_ok::<CreateSubscriber<InMemoryEngine>>(
        router,
        &CreateSubscriberInput {
            name: name.to_string(),
            event_bus_arn: event_bus_arn.to_string(),
            invoke_configuration: InvokeConfiguration {
                role_arn: ROLE_ARN.to_string(),
                target_arn: target_arn.to_string(),
                ..Default::default()
            },
            filter_configuration: requested.filters,
            state: requested.state.map(str::to_string),
            transformer: requested.transformer,
            starting_position: requested.starting_position.map(str::to_string),
            point_in_time_configuration: requested.point,
            ..Default::default()
        },
        case,
    )
    .await;
    created
        .subscriber_arn
        .unwrap_or_else(|| panic!("case {case:?}: CreateSubscriber reported no SubscriberArn"))
}

/// One `Filter`, in the scope it reads.
fn filter(pattern: &str, scope: &str) -> Filter {
    Filter {
        pattern: pattern.to_string(),
        scope: scope.to_string(),
    }
}

/// A `FilterConfiguration` holding `filters`, which are implicitly ANDed.
fn filters(filters: Vec<Filter>) -> FilterConfiguration {
    FilterConfiguration {
        language: None,
        filters: Some(filters),
    }
}

/// One event entry: the source, the detail type, the detail document and the
/// event group it belongs to.
fn entry(source: &str, detail_type: &str, detail: &str, group: Option<&str>) -> PutEventsRequestEntry {
    PutEventsRequestEntry {
        source: source.to_string(),
        detail_type: detail_type.to_string(),
        detail: Some(detail.to_string()),
        system_metadata: group.map(|group| PutEventsSystemMetadata {
            event_group_id: Some(group.to_string()),
            deduplication_id: None,
        }),
        ..Default::default()
    }
}

/// Publish `entries` onto `event_bus_arn`, requiring every entry to be
/// accepted.
async fn publish(
    router: &MultiServiceRouter,
    case: &str,
    event_bus_arn: &str,
    entries: Vec<PutEventsRequestEntry>,
) {
    let answer = eventsv2_ok::<PutEvents<InMemoryEngine>>(
        router,
        &PutEventsInput {
            event_bus_arn: event_bus_arn.to_string(),
            entries,
            deduplication_configuration: None,
        },
        case,
    )
    .await;
    assert_eq!(
        answer.failed_entry_count, 0,
        "case {case:?}: every entry must be accepted, got {answer:?}"
    );
}

/// How many messages the queue at `queue_url` reports available right now.
async fn queue_depth(router: &MultiServiceRouter, case: &str, queue_url: &str) -> u64 {
    let attributes = sqs_ok(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": queue_url, "AttributeNames": ["ApproximateNumberOfMessages"] }),
        case,
    )
    .await;
    let reported = attributes["Attributes"]["ApproximateNumberOfMessages"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: GetQueueAttributes reported no depth in {attributes}")
        });
    reported.parse::<u64>().unwrap_or_else(|error| {
        panic!("case {case:?}: ApproximateNumberOfMessages {reported:?} is no decimal: {error}")
    })
}

/// The bodies of the messages the queue at `queue_url` holds, polled until
/// `expected` of them have been read or the deadline passes.
///
/// Each message is asserted to carry the members a received message carries —
/// `MessageId`, `ReceiptHandle`, `MD5OfBody` and `Body` — and is deleted, so a
/// later read of the same queue reports what is left rather than what this read
/// took.
async fn received_bodies(
    router: &MultiServiceRouter,
    case: &str,
    queue_url: &str,
    expected: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + PATIENCE;
    let mut bodies = Vec::new();
    loop {
        let answer = sqs_ok(
            router,
            "ReceiveMessage",
            &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
            case,
        )
        .await;
        for message in answer["Messages"].as_array().unwrap_or(&Vec::new()) {
            for member in ["MessageId", "ReceiptHandle", "MD5OfBody", "Body"] {
                assert!(
                    message[member].is_string(),
                    "case {case:?}: a received message must carry {member}, got {message}"
                );
            }
            let body = message["Body"].as_str().unwrap_or_default();
            bodies.push(serde_json::from_str(body).unwrap_or_else(|error| {
                panic!("case {case:?}: the delivered body {body:?} is no document: {error}")
            }));
            sqs_ok(
                router,
                "DeleteMessage",
                &json!({
                    "QueueUrl": queue_url,
                    "ReceiptHandle": message["ReceiptHandle"].as_str().unwrap_or_default(),
                }),
                case,
            )
            .await;
        }
        if bodies.len() >= expected || Instant::now() >= deadline {
            return bodies;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

// ---------------------------------------------------------------------------
// The rule, over a table
// ---------------------------------------------------------------------------

/// One row: the case, the subscriber's filters, its `State`, its transformer,
/// and how many of the two events the row publishes the target must receive.
struct Row {
    case: &'static str,
    filters: Option<FilterConfiguration>,
    state: Option<&'static str>,
    transformer: Option<Transformer>,
    delivered: usize,
}

/// A published event reaches a subscriber exactly when that subscriber is
/// running, not revoked, and every one of its filters matches the event in the
/// scope that filter names.
///
/// Each row publishes the SAME two events — one from `orders.service` in group
/// `group-1` carrying `{"orderId":"A-1",…}`, one from `billing.service` in group
/// `group-2` carrying `{"orderId":"B-2",…}` — and states how many of them the
/// row's subscriber is owed, so the rows differ only along the subscriber.
#[tokio::test]
async fn an_event_reaches_the_subscribers_whose_filters_select_it() {
    let rows = vec![
        Row {
            case: "no filter configuration: every event the bus accepts",
            filters: None,
            state: None,
            transformer: None,
            delivered: 2,
        },
        Row {
            case: "a DATA-scoped filter on the event's own payload",
            filters: Some(filters(vec![filter(
                r#"{"orderId":["A-1"]}"#,
                eventsv2::state::FILTER_SCOPE_DATA,
            )])),
            state: None,
            transformer: None,
            delivered: 1,
        },
        Row {
            case: "a DATA-scoped filter no event matches",
            filters: Some(filters(vec![filter(
                r#"{"orderId":["Z-9"]}"#,
                eventsv2::state::FILTER_SCOPE_DATA,
            )])),
            state: None,
            transformer: None,
            delivered: 0,
        },
        Row {
            case: "a METADATA-scoped filter on the publisher's metadata",
            filters: Some(filters(vec![filter(
                r#"{"Source":["billing.service"]}"#,
                eventsv2::state::FILTER_SCOPE_METADATA,
            )])),
            state: None,
            transformer: None,
            delivered: 1,
        },
        Row {
            case: "a SYSTEM_METADATA-scoped filter on the event group",
            filters: Some(filters(vec![filter(
                r#"{"EventGroupId":["group-2"]}"#,
                eventsv2::state::FILTER_SCOPE_SYSTEM_METADATA,
            )])),
            state: None,
            transformer: None,
            delivered: 1,
        },
        Row {
            case: "two filters across two scopes, both matching one event",
            filters: Some(filters(vec![
                filter(
                    r#"{"Source":["orders.service"]}"#,
                    eventsv2::state::FILTER_SCOPE_METADATA,
                ),
                filter(
                    r#"{"orderId":["A-1"]}"#,
                    eventsv2::state::FILTER_SCOPE_DATA,
                ),
            ])),
            state: None,
            transformer: None,
            delivered: 1,
        },
        Row {
            case: "two filters that no one event satisfies together",
            filters: Some(filters(vec![
                filter(
                    r#"{"Source":["orders.service"]}"#,
                    eventsv2::state::FILTER_SCOPE_METADATA,
                ),
                filter(
                    r#"{"orderId":["B-2"]}"#,
                    eventsv2::state::FILTER_SCOPE_DATA,
                ),
            ])),
            state: None,
            transformer: None,
            delivered: 0,
        },
        Row {
            case: "a subscriber a caller created STOPPED",
            filters: None,
            state: Some(eventsv2::state::SUBSCRIBER_STATE_STOPPED),
            transformer: None,
            delivered: 0,
        },
        Row {
            case: "a subscriber a caller created RUNNING",
            filters: None,
            state: Some(eventsv2::state::SUBSCRIBER_STATE_RUNNING),
            transformer: None,
            delivered: 2,
        },
        Row {
            case: "a RAW transformer, which hands the target the payload alone",
            filters: None,
            state: None,
            transformer: Some(Transformer {
                r#type: Some(eventsv2::state::TRANSFORMER_TYPE_RAW.to_string()),
                jsonata_configuration: None,
            }),
            delivered: 2,
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let (queue_url, queue_arn) = given_queue(&router, case, &format!("target-{index}")).await;
        let bus_arn = given_active_bus(&router, case, &format!("bus-{index}")).await;
        given_subscriber(
            &router,
            case,
            &format!("subscriber-{index}"),
            &bus_arn,
            &queue_arn,
            Requested {
                filters: row.filters,
                state: row.state,
                transformer: row.transformer.clone(),
                ..Default::default()
            },
        )
        .await;

        publish(
            &router,
            case,
            &bus_arn,
            vec![
                entry(
                    "orders.service",
                    "order.created",
                    r#"{"orderId":"A-1","total":42}"#,
                    Some("group-1"),
                ),
                entry(
                    "billing.service",
                    "invoice.raised",
                    r#"{"orderId":"B-2","total":7}"#,
                    Some("group-2"),
                ),
            ],
        )
        .await;

        // The publish performs what it owes before it answers, so the queue's
        // own depth is already what the subscriber was owed.
        let depth = queue_depth(&router, case, &queue_url).await;
        assert_eq!(
            depth, row.delivered as u64,
            "case {case:?}: expected {} message(s) on the target queue, got {depth}",
            row.delivered
        );

        let bodies = received_bodies(&router, case, &queue_url, row.delivered).await;
        assert_eq!(
            bodies.len(),
            row.delivered,
            "case {case:?}: expected {} delivered message(s), got {bodies:?}",
            row.delivered
        );

        let raw = row
            .transformer
            .as_ref()
            .and_then(|transformer| transformer.r#type.as_deref())
            == Some(eventsv2::state::TRANSFORMER_TYPE_RAW);
        for body in &bodies {
            if raw {
                // The payload alone: the entry's own detail document.
                assert!(
                    body["orderId"].is_string(),
                    "case {case:?}: a RAW delivery must be the event's payload, got {body}"
                );
                assert!(
                    body.get("Metadata").is_none(),
                    "case {case:?}: a RAW delivery must carry no metadata, got {body}"
                );
            } else {
                assert_eq!(
                    body["EventBusArn"].as_str(),
                    Some(bus_arn.as_str()),
                    "case {case:?}: a delivery must name the bus it was published to, got {body}"
                );
                assert!(
                    body["EventId"].is_string() && body["SequenceNumber"].is_string(),
                    "case {case:?}: a delivery must carry the event's identity, got {body}"
                );
                assert!(
                    body["Metadata"]["Source"].is_string()
                        && body["Metadata"]["DetailType"].is_string(),
                    "case {case:?}: a delivery must carry the publisher's metadata, got {body}"
                );
                assert!(
                    body["Data"]["orderId"].is_string(),
                    "case {case:?}: a delivery must carry the event's payload, got {body}"
                );
            }
        }
    }
}

/// Every accepted event of one publish becomes one message per owed
/// subscriber: the rows here differ along the number of events and the number
/// of subscribers, which is the other half of "one delivery per accepted event
/// per subscriber".
#[tokio::test]
async fn each_accepted_event_reaches_each_owed_subscriber_once() {
    // (case, how many events one publish carries, how many subscribers the bus
    //  holds)
    let rows: [(&str, usize, usize); 4] = [
        ("one event, one subscriber", 1, 1),
        ("one event, two subscribers", 1, 2),
        ("five events, one subscriber", 5, 1),
        // More events than one `SendMessageBatch` carries, so the fan-out is
        // sent as several calls and every event still lands exactly once.
        ("twelve events, two subscribers", 12, 2),
    ];

    for (index, (case, events, subscribers)) in rows.into_iter().enumerate() {
        let router = emulator();
        let bus_arn = given_active_bus(&router, case, &format!("fanout-bus-{index}")).await;
        let mut queues = Vec::new();
        for subscriber in 0..subscribers {
            let (queue_url, queue_arn) =
                given_queue(&router, case, &format!("fanout-{index}-{subscriber}")).await;
            given_subscriber(
                &router,
                case,
                &format!("fanout-subscriber-{index}-{subscriber}"),
                &bus_arn,
                &queue_arn,
                Requested::default(),
            )
            .await;
            queues.push(queue_url);
        }

        let entries: Vec<PutEventsRequestEntry> = (0..events)
            .map(|position| {
                entry(
                    "orders.service",
                    "order.created",
                    &format!(r#"{{"orderId":"A-{position}"}}"#),
                    None,
                )
            })
            .collect();
        publish(&router, case, &bus_arn, entries).await;

        for queue_url in &queues {
            let depth = queue_depth(&router, case, queue_url).await;
            assert_eq!(
                depth, events as u64,
                "case {case:?}: expected {events} message(s) on {queue_url}, got {depth}"
            );
            let bodies = received_bodies(&router, case, queue_url, events).await;
            let mut ids: Vec<String> = bodies
                .iter()
                .map(|body| body["EventId"].as_str().unwrap_or_default().to_string())
                .collect();
            ids.sort();
            ids.dedup();
            assert_eq!(
                ids.len(),
                events,
                "case {case:?}: expected {events} distinct event(s) on {queue_url}, got {bodies:?}"
            );
        }
    }
}

/// One row of the read-back table: the case, the filters the create carried,
/// and the `State` it asked for.
type ReadBackRow = (&'static str, Option<Vec<Filter>>, Option<&'static str>);

/// A created subscriber is read back as it was created: the round trip through
/// `DescribeSubscriber`, over rows that differ along the members a create may
/// leave out.
#[tokio::test]
async fn a_created_subscriber_is_read_back_as_it_was_created() {
    // (case, the filters the create carried, the State it asked for)
    let rows: [ReadBackRow; 3] = [
        ("a create naming no filters and no state", None, None),
        (
            "a create naming one filter",
            Some(vec![filter(
                r#"{"Source":["orders.service"]}"#,
                eventsv2::state::FILTER_SCOPE_METADATA,
            )]),
            Some(eventsv2::state::SUBSCRIBER_STATE_RUNNING),
        ),
        (
            "a create naming two filters and a stopped state",
            Some(vec![
                filter(
                    r#"{"Source":["orders.service"]}"#,
                    eventsv2::state::FILTER_SCOPE_METADATA,
                ),
                filter(r#"{"orderId":["A-1"]}"#, eventsv2::state::FILTER_SCOPE_DATA),
            ]),
            Some(eventsv2::state::SUBSCRIBER_STATE_STOPPED),
        ),
    ];

    for (index, (case, requested, state)) in rows.into_iter().enumerate() {
        let router = emulator();
        let (_, queue_arn) = given_queue(&router, case, &format!("readback-{index}")).await;
        let bus_arn = given_active_bus(&router, case, &format!("readback-bus-{index}")).await;
        let name = format!("readback-subscriber-{index}");
        let subscriber_arn = given_subscriber(
            &router,
            case,
            &name,
            &bus_arn,
            &queue_arn,
            Requested {
                filters: requested.clone().map(filters),
                state,
                ..Default::default()
            },
        )
        .await;

        let expected_prefix = format!("arn:aws:events:{REGION}:{ACCOUNT}:subscriber/{name}/");
        let id = subscriber_arn
            .strip_prefix(&expected_prefix)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a SubscriberArn must start with {expected_prefix:?}, got \
                     {subscriber_arn:?}"
                )
            });
        assert_eq!(
            id.len(),
            eventsv2::state::SUBSCRIBER_ID_LEN,
            "case {case:?}: the ARN id segment must be {} characters, got {id:?}",
            eventsv2::state::SUBSCRIBER_ID_LEN
        );
        assert!(
            id.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "case {case:?}: the ARN id segment must be lowercase alphanumeric, got {id:?}"
        );

        let described = eventsv2_ok::<DescribeSubscriber<InMemoryEngine>>(
            &router,
            &DescribeSubscriberInput {
                subscriber_arn: subscriber_arn.clone(),
            },
            case,
        )
        .await;
        assert_eq!(
            described.subscriber_arn.as_deref(),
            Some(subscriber_arn.as_str()),
            "case {case:?}: DescribeSubscriber must report the ARN it was asked about, got \
             {described:?}"
        );
        assert_eq!(
            described.name.as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the created name, got {described:?}"
        );
        assert_eq!(
            described.event_bus_arn.as_deref(),
            Some(bus_arn.as_str()),
            "case {case:?}: expected the bus the subscriber was created on, got {described:?}"
        );
        assert_eq!(
            described
                .invoke_configuration
                .as_ref()
                .map(|configuration| configuration.target_arn.as_str()),
            Some(queue_arn.as_str()),
            "case {case:?}: expected the created TargetArn, got {described:?}"
        );
        assert_eq!(
            described.state.as_deref(),
            Some(state.unwrap_or(eventsv2::state::SUBSCRIBER_STATE_RUNNING)),
            "case {case:?}: expected the created State, got {described:?}"
        );
        assert_eq!(
            described.r#type.as_deref(),
            Some(eventsv2::state::ORDERING_TYPE_UNORDERED),
            "case {case:?}: expected the default ordering Type, got {described:?}"
        );
        assert_eq!(
            described.starting_position.as_deref(),
            Some(eventsv2::state::STARTING_POSITION_LATEST),
            "case {case:?}: expected the default StartingPosition, got {described:?}"
        );
        assert_eq!(
            described.revoked, None,
            "case {case:?}: a subscriber nobody revoked must report no Revoked, got {described:?}"
        );
        let read_back: Vec<(String, String)> = described
            .filter_configuration
            .as_ref()
            .and_then(|configuration| configuration.filters.as_ref())
            .map(|filters| {
                filters
                    .iter()
                    .map(|filter| (filter.pattern.clone(), filter.scope.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let expected: Vec<(String, String)> = requested
            .unwrap_or_default()
            .iter()
            .map(|filter| (filter.pattern.clone(), filter.scope.clone()))
            .collect();
        assert_eq!(
            read_back, expected,
            "case {case:?}: expected the created filters, got {read_back:?}"
        );
    }
}

/// A subscriber is owed the part of its bus's log its `StartingPosition` puts
/// it at: `LATEST` only what is published after it exists, `POINT_IN_TIME` the
/// retained backlog from the point its `PointInTimeConfiguration` names, and
/// nothing past an `EndPoint`.
///
/// Each row publishes TWO events before the subscriber is created and one after
/// it, and states how many of the three the target must hold.
#[tokio::test]
async fn a_subscriber_is_owed_its_bus_log_from_its_starting_position() {
    // An instant before anything this test publishes, and one well after it:
    // the two sides of a TIMESTAMP point. Both are request VALUES, so nothing
    // here is a wall-clock assertion.
    let before = ::aws_smithy_types::DateTime::from(
        std::time::SystemTime::now() - Duration::from_secs(600),
    );
    let after = ::aws_smithy_types::DateTime::from(
        std::time::SystemTime::now() + Duration::from_secs(600),
    );

    // (case, what the create asks for, how many of the three events land)
    let rows: Vec<(&str, Requested, usize)> = vec![
        (
            "a create naming no starting position, which is LATEST",
            Requested::default(),
            1,
        ),
        (
            "LATEST: the backlog is not this subscriber's",
            Requested {
                starting_position: Some(eventsv2::state::STARTING_POSITION_LATEST),
                ..Default::default()
            },
            1,
        ),
        (
            "POINT_IN_TIME from the HORIZON: the whole retained log",
            Requested {
                starting_position: Some(eventsv2::state::STARTING_POSITION_POINT_IN_TIME),
                point: Some(PointInTimeConfiguration {
                    point_type: eventsv2::state::POINT_TYPE_HORIZON.to_string(),
                    starting_point: None,
                    end_point: None,
                }),
                ..Default::default()
            },
            3,
        ),
        (
            "POINT_IN_TIME from a TIMESTAMP before the backlog",
            Requested {
                starting_position: Some(eventsv2::state::STARTING_POSITION_POINT_IN_TIME),
                point: Some(PointInTimeConfiguration {
                    point_type: eventsv2::state::POINT_TYPE_TIMESTAMP.to_string(),
                    starting_point: Some(before),
                    end_point: None,
                }),
                ..Default::default()
            },
            3,
        ),
        (
            "POINT_IN_TIME from a TIMESTAMP no stored event reaches",
            Requested {
                starting_position: Some(eventsv2::state::STARTING_POSITION_POINT_IN_TIME),
                point: Some(PointInTimeConfiguration {
                    point_type: eventsv2::state::POINT_TYPE_TIMESTAMP.to_string(),
                    starting_point: Some(after),
                    end_point: None,
                }),
                ..Default::default()
            },
            1,
        ),
        (
            "POINT_IN_TIME from the HORIZON, stopping before anything was published",
            Requested {
                starting_position: Some(eventsv2::state::STARTING_POSITION_POINT_IN_TIME),
                point: Some(PointInTimeConfiguration {
                    point_type: eventsv2::state::POINT_TYPE_HORIZON.to_string(),
                    starting_point: None,
                    end_point: Some(before),
                }),
                ..Default::default()
            },
            0,
        ),
        (
            "a STOPPED subscriber from the HORIZON keeps its backlog undelivered",
            Requested {
                starting_position: Some(eventsv2::state::STARTING_POSITION_POINT_IN_TIME),
                point: Some(PointInTimeConfiguration {
                    point_type: eventsv2::state::POINT_TYPE_HORIZON.to_string(),
                    starting_point: None,
                    end_point: None,
                }),
                state: Some(eventsv2::state::SUBSCRIBER_STATE_STOPPED),
                ..Default::default()
            },
            0,
        ),
    ];

    for (index, (case, requested, expected)) in rows.into_iter().enumerate() {
        let router = emulator();
        let (queue_url, queue_arn) = given_queue(&router, case, &format!("backlog-{index}")).await;
        let bus_arn = given_active_bus(&router, case, &format!("backlog-bus-{index}")).await;

        // Two events are on the bus's log before any subscriber exists.
        publish(
            &router,
            case,
            &bus_arn,
            vec![
                entry("orders.service", "order.created", r#"{"orderId":"A-1"}"#, None),
                entry("orders.service", "order.created", r#"{"orderId":"A-2"}"#, None),
            ],
        )
        .await;

        given_subscriber(
            &router,
            case,
            &format!("backlog-subscriber-{index}"),
            &bus_arn,
            &queue_arn,
            requested,
        )
        .await;

        // And one after it, which only a delivering subscriber is owed.
        publish(
            &router,
            case,
            &bus_arn,
            vec![entry(
                "orders.service",
                "order.created",
                r#"{"orderId":"A-3"}"#,
                None,
            )],
        )
        .await;

        let depth = queue_depth(&router, case, &queue_url).await;
        assert_eq!(
            depth, expected as u64,
            "case {case:?}: expected {expected} message(s) on the target queue, got {depth}"
        );
        let bodies = received_bodies(&router, case, &queue_url, expected).await;
        assert_eq!(
            bodies.len(),
            expected,
            "case {case:?}: expected {expected} delivered message(s), got {bodies:?}"
        );
        // Each event is delivered at most once, whichever position the
        // subscriber started from.
        let mut sequence_numbers: Vec<String> = bodies
            .iter()
            .map(|body| {
                body["SequenceNumber"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        sequence_numbers.sort();
        sequence_numbers.dedup();
        assert_eq!(
            sequence_numbers.len(),
            expected,
            "case {case:?}: every delivery must be of a distinct event, got {bodies:?}"
        );
    }
}

/// A subscriber whose target names no resource this emulator can deliver to is
/// not reported as delivered: the publish still accepts the event, and no other
/// subscriber of the bus loses its own delivery.
#[tokio::test]
async fn a_target_this_emulator_cannot_reach_withholds_only_its_own_delivery() {
    // (case, the TargetArn the first subscriber names)
    let rows: [(&str, &str); 3] = [
        (
            "a target of a service no delivery is made for",
            "arn:aws:lambda:us-west-2:000000000000:function:handler",
        ),
        (
            "an SQS ARN whose account is no account id",
            "arn:aws:sqs:us-west-2:my-account:orders",
        ),
        (
            "an SQS ARN naming a queue that does not exist",
            "arn:aws:sqs:us-west-2:000000000000:no-such-queue",
        ),
    ];

    for (index, (case, target)) in rows.into_iter().enumerate() {
        let router = emulator();
        let bus_arn = given_active_bus(&router, case, &format!("partial-bus-{index}")).await;
        given_subscriber(
            &router,
            case,
            &format!("unreachable-{index}"),
            &bus_arn,
            target,
            Requested::default(),
        )
        .await;
        let (queue_url, queue_arn) = given_queue(&router, case, &format!("partial-{index}")).await;
        given_subscriber(
            &router,
            case,
            &format!("reachable-{index}"),
            &bus_arn,
            &queue_arn,
            Requested::default(),
        )
        .await;

        publish(
            &router,
            case,
            &bus_arn,
            vec![entry(
                "orders.service",
                "order.created",
                r#"{"orderId":"A-1"}"#,
                None,
            )],
        )
        .await;

        let depth = queue_depth(&router, case, &queue_url).await;
        assert_eq!(
            depth, 1,
            "case {case:?}: the reachable subscriber must still be delivered its event, got depth \
             {depth}"
        );
    }
}
