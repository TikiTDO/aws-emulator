// SPDX-License-Identifier: CC0-1.0

//! The window `CancelChangeSet` serves, end to end over HTTP.
//!
//! The rule under test: a cancel is served only while the change request is still open —
//! `CancelChangeSet` documents that it "must be sent before the status of the request
//! changes to `APPLYING`, the final stage of completing your change request" — so a change
//! set in that final stage, or in any terminal status, is past cancelling and is refused
//! with the modelled `ResourceInUseException` (HTTP 423), with its record and its schedule
//! left exactly as they were.
//!
//! The rows span every value of `ChangeStatus` a change set can be in when the cancel
//! arrives, so the table covers the one open status, the final applying stage and each
//! terminal status. Every assertion goes through the router, so each case exercises
//! routing, deserialization, the handler, the persisted state and serialization.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`), which
/// is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// How long after the start a change set has certainly ended its run: the two spans the
/// implementation publishes, plus a margin for a slow machine.
fn wait_for_the_end_of_the_run() {
    std::thread::sleep(CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400));
}

/// `ExceptionMessageContent`'s `@length(min: 1, max: 2048)` and `^(.)+$`.
fn is_message(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 2048 && !value.contains('\n')
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw)
    }

    /// The string at `pointer`, or the empty string.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The error's stated message, under whichever casing the wire carries.
    fn message(&self) -> String {
        let lower = self.text("/message");
        if lower.is_empty() {
            self.text("/Message")
        } else {
            lower
        }
    }

    /// The array at `pointer`, or an empty one.
    fn list(&self, pointer: &str) -> Vec<Value> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn start_change_set(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/StartChangeSet", Some(request)).await
}

async fn describe_change_set(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeChangeSet?catalog={CATALOG}&changeSetId={id}"),
        None,
    )
    .await
}

async fn cancel_change_set(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "PATCH",
        &format!("/CancelChangeSet?catalog={CATALOG}&changeSetId={id}"),
        None,
    )
    .await
}

async fn list_entities(router: &MultiServiceRouter, entity_type: &str) -> Answer {
    call(
        router,
        "POST",
        "/ListEntities",
        Some(json!({"Catalog": CATALOG, "EntityType": entity_type})),
    )
    .await
}

/// A one-change `StartChangeSet` body creating an entity of `entity_type` with `details`.
fn create_change(entity_type: &str, details: Value) -> Value {
    json!({
        "Catalog": CATALOG,
        "ChangeSet": [{
            "ChangeType": "CreateProduct",
            "Entity": {"Type": format!("{entity_type}@1.0")},
            "Details": details.to_string(),
        }],
    })
}

/// One row of [`a_cancel_is_served_only_before_the_request_reaches_its_applying_stage`].
struct Case {
    /// What the row is, named in every assertion.
    name: &'static str,
    /// The `ChangeStatus` the change set is in when the measured cancel arrives.
    status: &'static str,
    /// The entity type the row's change set creates — one per row, so the row's own
    /// `ListEntities` page counts only what the row applied.
    entity_type: &'static str,
    /// The entity details: a value outside the published set is what makes the run end
    /// `FAILED` instead of `SUCCEEDED`.
    details: Value,
    /// How long after the start the measured cancel is sent.
    wait: Duration,
    /// Whether an earlier cancel takes the change set to `CANCELLED` first.
    cancel_first: bool,
    /// The HTTP status the measured cancel must answer.
    expected_http: u16,
    /// The status the change set ends up in, once the clock owes it nothing more.
    final_status: &'static str,
    /// How many entities the change set leaves in the catalog.
    final_entities: usize,
}

/// A cancel is served only while the request is still open: it is accepted in the one
/// status before the final applying stage, and refused with 423
/// `ResourceInUseException` from that stage onwards — in `APPLYING` and in every terminal
/// status — leaving the change set to finish exactly as it would have.
///
/// The rows span every value of `ChangeStatus` (`PREPARING`, `APPLYING`, `SUCCEEDED`,
/// `CANCELLED`, `FAILED` — the enum's values, from
/// `smithy_query(query="shape", shape="ChangeStatus")`), and so differ along how far the
/// clock has carried the change set when the cancel arrives, whether the run succeeds or
/// fails, whether a cancel already landed, and which entity type the change creates.
#[tokio::test]
async fn a_cancel_is_served_only_before_the_request_reaches_its_applying_stage() {
    let cases = vec![
        Case {
            name: "still PREPARING: the request is open",
            status: "PREPARING",
            entity_type: "AmiProduct",
            details: json!({"ProductTitle": "open"}),
            wait: Duration::ZERO,
            cancel_first: false,
            expected_http: 200,
            final_status: "CANCELLED",
            final_entities: 0,
        },
        Case {
            name: "already APPLYING: the final stage of completing the request",
            status: "APPLYING",
            entity_type: "SaaSProduct",
            details: json!({"ProductTitle": "applying"}),
            // Past the preparing span, leaving the whole applying span to answer in.
            wait: CHANGE_SET_PREPARING + Duration::from_millis(25),
            cancel_first: false,
            expected_http: 423,
            final_status: "SUCCEEDED",
            final_entities: 1,
        },
        Case {
            name: "SUCCEEDED: the run is over and its result stands",
            status: "SUCCEEDED",
            entity_type: "ContainerProduct",
            details: json!({"ProductTitle": "done"}),
            wait: CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400),
            cancel_first: false,
            expected_http: 423,
            final_status: "SUCCEEDED",
            final_entities: 1,
        },
        Case {
            name: "FAILED: the run is over and applied nothing",
            status: "FAILED",
            entity_type: "DataProduct",
            // `Visibility` outside the published values is judged by applying, so this
            // change set ends its run FAILED.
            details: json!({"ProductTitle": "failed", "Visibility": "Sideways"}),
            wait: CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400),
            cancel_first: false,
            expected_http: 423,
            final_status: "FAILED",
            final_entities: 0,
        },
        Case {
            name: "CANCELLED: a cancel already landed",
            status: "CANCELLED",
            entity_type: "MachineLearningProduct",
            details: json!({"ProductTitle": "twice"}),
            wait: Duration::ZERO,
            cancel_first: true,
            expected_http: 423,
            final_status: "CANCELLED",
            final_entities: 0,
        },
    ];

    for case in cases {
        let Case {
            name,
            status,
            entity_type,
            details,
            wait,
            cancel_first,
            expected_http,
            final_status,
            final_entities,
        } = case;
        let router = emulator();
        let started = start_change_set(&router, create_change(entity_type, details)).await;
        assert_eq!(
            started.status, 200,
            "case {name:?}: StartChangeSet must answer 200, got {} with {:?}",
            started.status, started.body
        );
        let id = started.text("/ChangeSetId");

        if cancel_first {
            let first = cancel_change_set(&router, &id).await;
            assert_eq!(
                first.status, 200,
                "case {name:?}: the first cancel arrives while the request is open, so it \
                 must answer 200, got {} with {:?}",
                first.status, first.body
            );
        }
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }

        let answer = cancel_change_set(&router, &id).await;
        assert_eq!(
            answer.status, expected_http,
            "case {name:?}: a cancel sent in {status} must answer {expected_http}, got {} \
             with {:?}",
            answer.status, answer.body
        );

        // The status the cancel was judged against: the record is reported after the
        // cancel, so a refusal that left it alone still shows the row's status, and an
        // accepted cancel shows CANCELLED.
        let described = describe_change_set(&router, &id).await;
        let observed = described.text("/Status");

        if expected_http == 200 {
            assert_eq!(
                answer.text("/ChangeSetId"),
                id,
                "case {name:?}: an accepted cancel answers the stored id, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.text("/ChangeSetArn"),
                started.text("/ChangeSetArn"),
                "case {name:?}: an accepted cancel answers the stored ARN, got {:?}",
                answer.body
            );
            assert_eq!(
                observed, "CANCELLED",
                "case {name:?}: an accepted cancel stops the change set, got {:?}",
                described.body
            );
        } else {
            assert_eq!(
                answer.error_code(),
                "ResourceInUseException",
                "case {name:?}: a cancel past the open window answers ResourceInUseException, \
                 got {:?}",
                answer.body
            );
            let message = answer.message();
            assert!(
                is_message(&message) && message.contains(&id),
                "case {name:?}: the refusal must name the change set it refused, got {:?}",
                answer.body
            );
            assert_eq!(
                observed, status,
                "case {name:?}: a refused cancel must leave the change set in the status it \
                 was in, got {:?}",
                described.body
            );
        }

        // And the schedule the start laid down is untouched by a refused cancel: the run
        // ends where it would have ended, applying what it would have applied.
        wait_for_the_end_of_the_run();
        let settled = describe_change_set(&router, &id).await;
        assert_eq!(
            settled.text("/Status"),
            final_status,
            "case {name:?}: the change set must end in {final_status}, got {:?}",
            settled.body
        );
        let entities = list_entities(&router, entity_type).await;
        assert_eq!(
            entities.list("/EntitySummaryList").len(),
            final_entities,
            "case {name:?}: the change set must leave {final_entities} entity/entities in the \
             catalog, got {:?}",
            entities.body
        );
    }
}
