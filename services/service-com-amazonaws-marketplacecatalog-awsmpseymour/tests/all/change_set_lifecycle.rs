// SPDX-License-Identifier: CC0-1.0

//! The AWS Marketplace Catalog change-set lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one the catalog states for its primary
//! resource: a change set is whatever `StartChangeSet` stored, it runs on the clock, and
//! an `APPLY` change set that succeeds is what makes its entities readable — every
//! operation computing its answer from those records and from nothing else.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, ENTITY_VISIBILITY_VALUES, MAX_DETAILS_DEPTH,
    PRODUCT_ENTITY_TYPES,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`). It is
/// what routes a request here, and its region is what the ARNs this service mints name.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// How long after the start a change set has certainly reached `SUCCEEDED`: the two spans
/// the implementation publishes, plus a margin for a slow machine.
fn wait_for_success() {
    std::thread::sleep(CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400));
}

/// `ResourceId`'s `^[\w\-]+$`, checked here rather than through the crate's own predicate
/// so a wrong implementation cannot make this test pass.
fn is_resource_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// `Identifier`'s `^[\w\-@]+$`.
fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '@')
}

/// `ARN`'s `^[a-zA-Z0-9:*/-]+$`.
fn is_arn(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 2048
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '*' | '/' | '-'))
}

/// `NextToken`'s `^[\w+=.:@\-\/]+$`.
fn is_next_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '=' | '.' | ':' | '@' | '-' | '/'))
}

/// `DateTimeISO8601`'s `^([\d]{4})\-(1[0-2]|0[1-9])\-(3[01]|0[1-9]|[12][\d])T(2[0-3]|[01][\d]):([0-5][\d]):([0-5][\d])Z$`,
/// spelled out as a predicate over the 20 characters the length bound pins.
fn is_iso8601(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 20 {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'Z'
        && digits(0..4)
        && digits(5..7)
        && digits(8..10)
        && digits(11..13)
        && digits(14..16)
        && digits(17..19)
        && (1..=12).contains(&value[5..7].parse::<u32>().unwrap_or(0))
        && (1..=31).contains(&value[8..10].parse::<u32>().unwrap_or(0))
        && value[11..13].parse::<u32>().unwrap_or(99) <= 23
        && value[14..16].parse::<u32>().unwrap_or(99) <= 59
        && value[17..19].parse::<u32>().unwrap_or(99) <= 59
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

async fn list_change_sets(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/ListChangeSets", Some(request)).await
}

async fn describe_entity(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeEntity?catalog={CATALOG}&entityId={id}"),
        None,
    )
    .await
}

async fn list_entities(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/ListEntities", Some(request)).await
}

/// A details document nested `depth` levels deep, as `{"Nested":{"Nested":…{}}}`.
fn nested_details(depth: usize) -> Value {
    let mut value = json!({});
    for _ in 1..depth {
        value = json!({"Nested": value});
    }
    value
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

/// Start a change set, wait for the clock to apply it, and answer the entity id it made.
async fn created_entity(router: &MultiServiceRouter, request: Value) -> String {
    let started = start_change_set(router, request).await;
    assert_eq!(
        started.status, 200,
        "StartChangeSet must answer 200, got {} with {:?}",
        started.status, started.body
    );
    let id = started.text("/ChangeSetId");
    wait_for_success();
    let described = describe_change_set(router, &id).await;
    assert_eq!(
        described.text("/Status"),
        "SUCCEEDED",
        "the change set must have succeeded before its entity is read, got {:?}",
        described.body
    );
    let listed = list_change_sets(router, json!({"Catalog": CATALOG})).await;
    let entity = listed
        .list("/ChangeSetSummaryList")
        .into_iter()
        .find(|item| item["ChangeSetId"] == json!(id))
        .and_then(|item| item["EntityIdList"][0].as_str().map(str::to_string))
        .unwrap_or_else(|| panic!("change set {id} must report its entity: {:?}", listed.body));
    entity
}

/// `StartChangeSet` stores the change set and every later operation reports that record:
/// the ids it answers are the stored ones, `DescribeChangeSet` echoes the submitted
/// changes, `ListChangeSets` names the entity, and the entity itself is readable once the
/// clock has applied the change set.
///
/// The rows differ along what the rule generalises over — how many changes, which change
/// types, whether the change set is named or tagged, which details member carries the
/// details and how deep that document is.
#[tokio::test]
async fn a_started_change_set_is_reported_from_what_was_stored() {
    let cases = vec![
        (
            "one change, details as a JSON string",
            json!({
                "Catalog": CATALOG,
                "ChangeSetName": "bootstrap-test",
                "Intent": "APPLY",
                "ChangeSet": [{
                    "ChangeType": "CreateProduct",
                    "Entity": {"Type": "AmiProduct@1.0"},
                    "Details": "{\"ProductTitle\":\"single\"}",
                }],
            }),
            1_usize,
        ),
        (
            "one change, details as a document",
            json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": "AddRevisions",
                    "Entity": {"Type": "SaaSProduct@1.0"},
                    "DetailsDocument": {
                        "ProductTitle": "nested",
                        "Versions": [{"Name": "v1", "Enabled": true}],
                        "Count": 3,
                    },
                }],
            }),
            1,
        ),
        (
            "three changes against three entities, named and tagged",
            json!({
                "Catalog": CATALOG,
                "ChangeSetName": "three changes",
                "ChangeSetTags": [{"Key": "env", "Value": "test"}],
                "ChangeSet": [
                    {
                        "ChangeType": "CreateProduct",
                        "Entity": {"Type": "AmiProduct@1.0"},
                        "Details": "{\"ProductTitle\":\"first\"}",
                        "EntityTags": [{"Key": "team", "Value": "catalog"}],
                    },
                    {
                        "ChangeType": "CreateProduct",
                        "Entity": {"Type": "ContainerProduct@1.0"},
                        "Details": "{\"ProductTitle\":\"second\"}",
                    },
                    {
                        "ChangeType": "UpdateInformation",
                        "Entity": {"Type": "DataProduct@1.0"},
                        "DetailsDocument": {"ProductTitle": "third"},
                    },
                ],
            }),
            3,
        ),
        (
            "the same change type against two different entities",
            json!({
                "Catalog": CATALOG,
                "ChangeSet": [
                    {
                        "ChangeType": "CreateProduct",
                        "Entity": {"Type": "MachineLearningProduct@1.0"},
                        "Details": "{\"ProductTitle\":\"left\"}",
                    },
                    {
                        "ChangeType": "CreateProduct",
                        "Entity": {"Type": "MachineLearningProduct@1.0"},
                        "Details": "{\"ProductTitle\":\"right\"}",
                    },
                ],
            }),
            2,
        ),
    ];

    for (case, request, change_count) in cases {
        let router = emulator();
        let started = start_change_set(&router, request.clone()).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartChangeSet must answer 200, got {} with {:?}",
            started.status, started.body
        );
        let id = started.text("/ChangeSetId");
        let arn = started.text("/ChangeSetArn");
        assert!(
            is_resource_id(&id),
            "case {case:?}: ChangeSetId must match ^[\\w\\-]+$, got {id:?}"
        );
        assert_eq!(
            arn,
            format!("arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:{CATALOG}/ChangeSet/{id}"),
            "case {case:?}: the ARN must name the minted change set, got {arn:?}"
        );
        assert!(
            is_arn(&arn),
            "case {case:?}: ChangeSetArn must match ^[a-zA-Z0-9:*/-]+$, got {arn:?}"
        );

        // Before the clock has carried it anywhere, the change set is the record the
        // start stored — and it is still running.
        let running = describe_change_set(&router, &id).await;
        assert_eq!(
            running.status, 200,
            "case {case:?}: DescribeChangeSet must answer 200, got {} with {:?}",
            running.status, running.body
        );
        assert_eq!(
            running.text("/ChangeSetId"),
            id,
            "case {case:?}: the described id must be the started one, got {:?}",
            running.body
        );
        assert_eq!(
            running.text("/ChangeSetArn"),
            arn,
            "case {case:?}: the described ARN must be the started one, got {:?}",
            running.body
        );
        assert_eq!(
            running.text("/ChangeSetName"),
            request["ChangeSetName"].as_str().unwrap_or_default(),
            "case {case:?}: the described name must be the submitted one, got {:?}",
            running.body
        );
        assert_eq!(
            running.text("/Intent"),
            request["Intent"].as_str().unwrap_or("APPLY"),
            "case {case:?}: Intent defaults to APPLY, got {:?}",
            running.body
        );
        assert!(
            is_iso8601(&running.text("/StartTime")),
            "case {case:?}: StartTime must be a 20-character ISO-8601 instant, got {:?}",
            running.text("/StartTime")
        );
        assert!(
            ["PREPARING", "APPLYING"].contains(&running.text("/Status").as_str()),
            "case {case:?}: a change set starts out running, got {:?}",
            running.text("/Status")
        );
        assert_eq!(
            running.body.pointer("/FailureCode"),
            None,
            "case {case:?}: only a FAILED change set reports a failure, got {:?}",
            running.body
        );

        let summaries = running.list("/ChangeSet");
        assert_eq!(
            summaries.len(),
            change_count,
            "case {case:?}: every submitted change must be reported, got {:?}",
            running.body
        );
        for (index, submitted) in request["ChangeSet"]
            .as_array()
            .expect("the case submits a change list")
            .iter()
            .enumerate()
        {
            let reported = &summaries[index];
            assert_eq!(
                reported["ChangeType"], submitted["ChangeType"],
                "case {case:?}: change {index} must report its submitted ChangeType, got {reported:?}"
            );
            assert_eq!(
                reported["Entity"]["Type"], submitted["Entity"]["Type"],
                "case {case:?}: change {index} must report its submitted Entity, got {reported:?}"
            );
            assert_eq!(
                reported["Details"], submitted["Details"],
                "case {case:?}: change {index} must report its submitted Details, got {reported:?}"
            );
            assert_eq!(
                reported["DetailsDocument"], submitted["DetailsDocument"],
                "case {case:?}: change {index} must report its submitted DetailsDocument, \
                 got {reported:?}"
            );
        }

        // The clock, not a call, is what finishes the change set.
        wait_for_success();
        let settled = describe_change_set(&router, &id).await;
        assert_eq!(
            settled.text("/Status"),
            "SUCCEEDED",
            "case {case:?}: once the span has passed the change set has succeeded, got {:?}",
            settled.body
        );
        assert!(
            is_iso8601(&settled.text("/EndTime")),
            "case {case:?}: a finished change set carries an EndTime, got {:?}",
            settled.body
        );
        let end_time = settled.text("/EndTime");
        let again = describe_change_set(&router, &id).await;
        assert_eq!(
            again.text("/EndTime"),
            end_time,
            "case {case:?}: reading a change set twice must report the same EndTime, got {:?}",
            again.body
        );

        // `ListChangeSets` reports the same record, and names the entities the change
        // set made.
        let listed = list_change_sets(&router, json!({"Catalog": CATALOG})).await;
        let item = listed
            .list("/ChangeSetSummaryList")
            .into_iter()
            .find(|item| item["ChangeSetId"] == json!(id))
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListChangeSets must report the change set: {:?}", listed.body)
            });
        assert_eq!(
            item["ChangeSetArn"], json!(arn),
            "case {case:?}: the listed ARN must be the stored one, got {item:?}"
        );
        assert_eq!(
            item["Status"], json!("SUCCEEDED"),
            "case {case:?}: the listed status must be the stored one, got {item:?}"
        );
        let entity_ids: Vec<String> = item["EntityIdList"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        assert_eq!(
            entity_ids.len(),
            change_count,
            "case {case:?}: EntityIdList must name one entity per change, got {item:?}"
        );

        // Every named entity is readable, and reports what the change gave it.
        for (index, entity_id) in entity_ids.iter().enumerate() {
            assert!(
                is_resource_id(entity_id),
                "case {case:?}: entity id must match ^[\\w\\-]+$, got {entity_id:?}"
            );
            let entity = describe_entity(&router, entity_id).await;
            assert_eq!(
                entity.status, 200,
                "case {case:?}: DescribeEntity must answer 200, got {} with {:?}",
                entity.status, entity.body
            );
            let submitted = &request["ChangeSet"][index];
            // `Change$Entity$Type` carries the type at a schema version, where the ENTITY
            // it acts on is of the type that reference names.
            let entity_type = submitted["Entity"]["Type"]
                .as_str()
                .unwrap_or_default()
                .split('@')
                .next()
                .unwrap_or_default();
            assert_eq!(
                entity.body["EntityType"], json!(entity_type),
                "case {case:?}: the entity's type must be the change's, got {:?}",
                entity.body
            );
            assert_eq!(
                entity.text("/EntityIdentifier"),
                format!("{entity_id}@1"),
                "case {case:?}: one applied change leaves the entity at revision 1, got {:?}",
                entity.body
            );
            assert!(
                is_identifier(&entity.text("/EntityIdentifier")),
                "case {case:?}: EntityIdentifier must match ^[\\w\\-@]+$, got {:?}",
                entity.text("/EntityIdentifier")
            );
            assert_eq!(
                entity.text("/EntityArn"),
                format!(
                    "arn:aws:aws-marketplace:{REGION}:{ACCOUNT}:{CATALOG}/{entity_type}/{entity_id}"
                ),
                "case {case:?}: the entity ARN must name its type and id, got {:?}",
                entity.body
            );
            assert!(
                is_iso8601(&entity.text("/LastModifiedDate")),
                "case {case:?}: LastModifiedDate must be an ISO-8601 instant, got {:?}",
                entity.text("/LastModifiedDate")
            );

            // The details the change carried are the details the entity holds, whichever
            // member carried them.
            let expected: Value = match submitted["Details"].as_str() {
                Some(text) => serde_json::from_str(text).expect("the case submits valid JSON"),
                None => submitted["DetailsDocument"].clone(),
            };
            assert_eq!(
                entity.body["DetailsDocument"], expected,
                "case {case:?}: the entity's details must be the applied change's, got {:?}",
                entity.body
            );
            let as_string: Value = serde_json::from_str(&entity.text("/Details"))
                .expect("Details must be a JSON object");
            assert_eq!(
                as_string, expected,
                "case {case:?}: Details and DetailsDocument must carry the same details, \
                 got {:?}",
                entity.body
            );

            // And `ListEntities` finds it under its own type, from state rather than from
            // a canned list.
            let mine = list_entities(
                &router,
                json!({"Catalog": CATALOG, "EntityType": entity_type}),
            )
            .await;
            assert!(
                mine.list("/EntitySummaryList")
                    .iter()
                    .any(|summary| summary["EntityId"] == json!(entity_id)),
                "case {case:?}: ListEntities must report the entity under its own type, got {:?}",
                mine.body
            );
            let other = list_entities(
                &router,
                json!({"Catalog": CATALOG, "EntityType": "ResaleAuthorization"}),
            )
            .await;
            assert!(
                other.list("/EntitySummaryList").is_empty(),
                "case {case:?}: a type with no entities must list none, got {:?}",
                other.body
            );
        }
    }
}

/// The same `ClientRequestToken` starts one change set: the second call reports the first
/// change set's id, and the store still holds exactly one.
#[tokio::test]
async fn one_client_request_token_starts_one_change_set() {
    let router = emulator();
    let request = json!({
        "Catalog": CATALOG,
        "ChangeSetName": "bootstrap-test",
        "ClientRequestToken": "tok-1",
        "Intent": "APPLY",
        "ChangeSet": [{
            "ChangeType": "CreateProduct",
            "Entity": {"Type": "AmiProduct@1.0"},
            "Details": "{}",
        }],
    });

    let first = start_change_set(&router, request.clone()).await;
    let second = start_change_set(&router, request.clone()).await;
    assert_eq!(
        first.text("/ChangeSetId"),
        second.text("/ChangeSetId"),
        "a repeated ClientRequestToken must answer the first ChangeSetId, got {:?} then {:?}",
        first.body,
        second.body
    );
    assert_eq!(
        first.text("/ChangeSetArn"),
        second.text("/ChangeSetArn"),
        "a repeated ClientRequestToken must answer the first ARN, got {:?} then {:?}",
        first.body,
        second.body
    );

    let listed = list_change_sets(&router, json!({"Catalog": CATALOG})).await;
    assert_eq!(
        listed.list("/ChangeSetSummaryList").len(),
        1,
        "the retried start must not have stored a second change set, got {:?}",
        listed.body
    );

    // A different token is a different request, so it starts its own change set.
    let mut other = request;
    other["ClientRequestToken"] = json!("tok-2");
    let third = start_change_set(&router, other).await;
    assert_ne!(
        third.text("/ChangeSetId"),
        first.text("/ChangeSetId"),
        "a different ClientRequestToken must start its own change set, got {:?}",
        third.body
    );
}

/// Cancelling a running change set flips it to `CANCELLED`, and the changes it carried are
/// never applied; cancelling one that has completed is refused.
#[tokio::test]
async fn cancelling_a_running_change_set_stops_it() {
    let router = emulator();
    let started = start_change_set(
        &router,
        create_change("AmiProduct", json!({"ProductTitle": "cancelled"})),
    )
    .await;
    let id = started.text("/ChangeSetId");

    let cancelled = cancel_change_set(&router, &id).await;
    assert_eq!(
        cancelled.status, 200,
        "CancelChangeSet must answer 200 for a running change set, got {} with {:?}",
        cancelled.status, cancelled.body
    );
    assert_eq!(
        cancelled.text("/ChangeSetId"),
        id,
        "CancelChangeSet must answer the cancelled change set's id, got {:?}",
        cancelled.body
    );
    assert_eq!(
        cancelled.text("/ChangeSetArn"),
        started.text("/ChangeSetArn"),
        "CancelChangeSet must answer the stored ARN, got {:?}",
        cancelled.body
    );

    let described = describe_change_set(&router, &id).await;
    assert_eq!(
        described.text("/Status"),
        "CANCELLED",
        "a cancelled change set reports CANCELLED, got {:?}",
        described.body
    );
    assert!(
        is_iso8601(&described.text("/EndTime")),
        "a cancelled change set carries an EndTime, got {:?}",
        described.body
    );

    // Waiting past the span the start scheduled changes nothing: a cancelled change set
    // never applies its changes.
    wait_for_success();
    let after = describe_change_set(&router, &id).await;
    assert_eq!(
        after.text("/Status"),
        "CANCELLED",
        "a cancelled change set stays cancelled, got {:?}",
        after.body
    );
    let entities = list_entities(&router, json!({"Catalog": CATALOG, "EntityType": "AmiProduct"}))
        .await;
    assert!(
        entities.list("/EntitySummaryList").is_empty(),
        "a cancelled change set must create no entity, got {:?}",
        entities.body
    );

    let again = cancel_change_set(&router, &id).await;
    assert_eq!(
        again.status, 423,
        "cancelling a completed change set must answer 423, got {} with {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.error_code(),
        "ResourceInUseException",
        "cancelling a completed change set must answer ResourceInUseException, got {:?}",
        again.body
    );

    // The same refusal for a change set the clock has carried to SUCCEEDED.
    let succeeded = start_change_set(
        &router,
        create_change("SaaSProduct", json!({"ProductTitle": "done"})),
    )
    .await;
    wait_for_success();
    let refused = cancel_change_set(&router, &succeeded.text("/ChangeSetId")).await;
    assert_eq!(
        refused.status, 423,
        "cancelling a succeeded change set must answer 423, got {} with {:?}",
        refused.status, refused.body
    );
}

/// `Intent=VALIDATE` tests a request without applying it: the change set runs and
/// succeeds, and the catalog gains no entity.
#[tokio::test]
async fn a_validate_change_set_creates_no_entity() {
    let router = emulator();
    let mut request = create_change("AmiProduct", json!({"ProductTitle": "validated"}));
    request["Intent"] = json!("VALIDATE");
    let started = start_change_set(&router, request).await;
    let id = started.text("/ChangeSetId");
    wait_for_success();

    let described = describe_change_set(&router, &id).await;
    assert_eq!(
        described.text("/Intent"),
        "VALIDATE",
        "the described intent must be the submitted one, got {:?}",
        described.body
    );
    assert_eq!(
        described.text("/Status"),
        "SUCCEEDED",
        "a validation runs to SUCCEEDED, got {:?}",
        described.body
    );

    let listed = list_change_sets(&router, json!({"Catalog": CATALOG})).await;
    let item = &listed.list("/ChangeSetSummaryList")[0];
    assert_eq!(
        item.get("EntityIdList"),
        None,
        "a validation reserves no entity, and EntityIdList must contain at least one \
         entity, so the summary reports no such member at all, got {item:?}"
    );
    let entities = list_entities(&router, json!({"Catalog": CATALOG, "EntityType": "AmiProduct"}))
        .await;
    assert!(
        entities.list("/EntitySummaryList").is_empty(),
        "a validation must create no entity, got {:?}",
        entities.body
    );
}

/// A change naming an existing entity mutates that entity rather than creating another:
/// the details merge, the revision moves and `LastModifiedDate` is restamped.
#[tokio::test]
async fn a_change_against_an_existing_entity_updates_it() {
    let router = emulator();
    let entity_id = created_entity(
        &router,
        create_change("AmiProduct", json!({"ProductTitle": "first", "Description": "a"})),
    )
    .await;

    let update = json!({
        "Catalog": CATALOG,
        "ChangeSet": [{
            "ChangeType": "UpdateInformation",
            "Entity": {"Type": "AmiProduct@1.0", "Identifier": format!("{entity_id}@1")},
            "DetailsDocument": {"ProductTitle": "second", "Support": "email"},
        }],
    });
    let started = start_change_set(&router, update).await;
    assert_eq!(
        started.status, 200,
        "a change against an existing entity must be accepted, got {} with {:?}",
        started.status, started.body
    );
    wait_for_success();

    let entity = describe_entity(&router, &entity_id).await;
    assert_eq!(
        entity.text("/EntityIdentifier"),
        format!("{entity_id}@2"),
        "a second applied change moves the entity to revision 2, got {:?}",
        entity.body
    );
    assert_eq!(
        entity.body["DetailsDocument"],
        json!({"ProductTitle": "second", "Description": "a", "Support": "email"}),
        "the applied details must merge into the entity's, got {:?}",
        entity.body
    );

    let listed = list_entities(&router, json!({"Catalog": CATALOG, "EntityType": "AmiProduct"}))
        .await;
    let summaries = listed.list("/EntitySummaryList");
    assert_eq!(
        summaries.len(),
        1,
        "updating an entity must not create another, got {:?}",
        listed.body
    );
    assert_eq!(
        summaries[0]["Name"], json!("second"),
        "the summary's Name must be the title the last change gave it, got {:?}",
        summaries[0]
    );

    // The change set that updated it names the same entity, so both change sets point at
    // one record.
    let change_sets = list_change_sets(&router, json!({"Catalog": CATALOG})).await;
    for item in change_sets.list("/ChangeSetSummaryList") {
        assert_eq!(
            item["EntityIdList"], json!([entity_id.clone()]),
            "both change sets act on the one entity, got {item:?}"
        );
    }
}

/// An entity's `Visibility` is the one its applied details named, over the whole set the
/// model's `*VisibilityString` enums publish — and a value outside that set is not
/// applied at all.
#[tokio::test]
async fn an_entity_reports_the_visibility_its_details_named() {
    let router = emulator();

    // The default first: an entity whose details name no visibility is `Limited`.
    let default_id = created_entity(
        &router,
        create_change("AmiProduct", json!({"ProductTitle": "unpublished"})),
    )
    .await;
    let listed = list_entities(
        &router,
        json!({"Catalog": CATALOG, "EntityType": "AmiProduct", "FilterList": [
            {"Name": "EntityId", "ValueList": [default_id.clone()]}
        ]}),
    )
    .await;
    assert_eq!(
        listed.list("/EntitySummaryList")[0]["Visibility"],
        json!("Limited"),
        "an entity a change set has just created is visible to limited accounts, got {:?}",
        listed.body
    );

    // Then every published value, including the ones the issue never cited.
    for visibility in ENTITY_VISIBILITY_VALUES {
        let router = emulator();
        let entity_id = created_entity(
            &router,
            create_change(
                "DataProduct",
                json!({"ProductTitle": "published", "Visibility": visibility}),
            ),
        )
        .await;
        let listed = list_entities(
            &router,
            json!({"Catalog": CATALOG, "EntityType": "DataProduct"}),
        )
        .await;
        let summaries = listed.list("/EntitySummaryList");
        assert_eq!(
            summaries.len(),
            1,
            "case {visibility:?}: the created entity must be listed, got {:?}",
            listed.body
        );
        assert_eq!(
            summaries[0]["EntityId"], json!(entity_id),
            "case {visibility:?}: the listed entity must be the created one, got {:?}",
            summaries[0]
        );
        assert_eq!(
            summaries[0]["Visibility"], json!(visibility),
            "case {visibility:?}: the summary must report the stored visibility, got {:?}",
            summaries[0]
        );
        assert_eq!(
            summaries[0]["DataProductSummary"]["Visibility"], json!(visibility),
            "case {visibility:?}: the per-type summary must report it too, got {:?}",
            summaries[0]
        );
    }

    // A visibility the model has no value for is not refused when the change set is
    // accepted — it is inside the details document, which the model constrains nothing
    // inside — but it is not stored either: applying the change set is what finds it, so
    // the change set fails and the entity it would have created never exists.
    let router = emulator();
    let accepted = start_change_set(
        &router,
        create_change("AmiProduct", json!({"Visibility": "Sideways"})),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "an unmodelled Visibility is judged by applying, not by the start, got {} with {:?}",
        accepted.status, accepted.body
    );
    let unapplied = accepted.text("/ChangeSetId");
    wait_for_success();
    let failed = describe_change_set(&router, &unapplied).await;
    assert_eq!(
        (failed.text("/Status"), failed.text("/FailureCode")),
        ("FAILED".to_string(), "CLIENT_ERROR".to_string()),
        "an unmodelled Visibility must fail the change set, got {:?}",
        failed.body
    );
    let entities = list_entities(&router, json!({"Catalog": CATALOG, "EntityType": "AmiProduct"}))
        .await;
    assert_eq!(
        entities.list("/EntitySummaryList"),
        Vec::<Value>::new(),
        "a failed change set must create no entity, got {:?}",
        entities.body
    );
}

/// `EntitySummary` carries the per-type summary of the type the entity is, and none
/// belonging to another type.
#[tokio::test]
async fn an_entity_summary_carries_its_own_types_summary() {
    // The member each product type's summary lands in, as `EntitySummary` names them.
    let members: Vec<(&str, &str)> = vec![
        ("AmiProduct", "AmiProductSummary"),
        ("ContainerProduct", "ContainerProductSummary"),
        ("DataProduct", "DataProductSummary"),
        ("MachineLearningProduct", "MachineLearningProductSummary"),
        ("SaaSProduct", "SaaSProductSummary"),
    ];
    assert_eq!(
        members.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
        PRODUCT_ENTITY_TYPES.to_vec(),
        "this test must cover exactly the product types the implementation fills"
    );

    for (entity_type, member) in &members {
        let router = emulator();
        let entity_id = created_entity(
            &router,
            create_change(entity_type, json!({"ProductTitle": "titled"})),
        )
        .await;
        let listed = list_entities(
            &router,
            json!({"Catalog": CATALOG, "EntityType": entity_type}),
        )
        .await;
        let summary = listed.list("/EntitySummaryList")[0].clone();
        assert_eq!(
            summary["EntityId"], json!(entity_id),
            "case {entity_type:?}: the listed entity must be the created one, got {summary:?}"
        );
        assert_eq!(
            summary["Name"], json!("titled"),
            "case {entity_type:?}: Name must be the ProductTitle the details named, got {summary:?}"
        );
        assert_eq!(
            summary[*member]["ProductTitle"], json!("titled"),
            "case {entity_type:?}: {member} must carry the product title, got {summary:?}"
        );
        for (_, other) in members.iter().filter(|(t, _)| t != entity_type) {
            assert_eq!(
                summary.get(*other), None,
                "case {entity_type:?}: {other} belongs to another type, got {summary:?}"
            );
        }
    }

    // A type outside that set reports its common members and the per-type summary its own
    // type names — `OfferSummary` for an `Offer`, whose members the entity's details carry.
    let router = emulator();
    let entity_id = created_entity(
        &router,
        create_change("Offer", json!({"Name": "an offer", "State": "Released"})),
    )
    .await;
    let listed = list_entities(&router, json!({"Catalog": CATALOG, "EntityType": "Offer"})).await;
    let summary = listed.list("/EntitySummaryList")[0].clone();
    assert_eq!(
        summary["EntityId"], json!(entity_id),
        "an Offer entity must be listed under its type, got {summary:?}"
    );
    assert_eq!(
        summary["Name"], json!("an offer"),
        "an Offer's Name comes from its details' Name member, got {summary:?}"
    );
    assert_eq!(
        summary["OfferSummary"]["Name"], json!("an offer"),
        "OfferSummary must carry the offer's name, got {summary:?}"
    );
    assert_eq!(
        summary["OfferSummary"]["State"], json!("Released"),
        "OfferSummary must carry the state the details named, got {summary:?}"
    );
    for (_, other) in &members {
        assert_eq!(
            summary.get(*other), None,
            "{other} belongs to another type, got {summary:?}"
        );
    }
}

/// Both lists page: each page holds at most `MaxResults` items, the token resumes exactly
/// where the page ended, the last page carries none, and no item repeats or is skipped.
#[tokio::test]
async fn both_lists_page_through_every_record() {
    let router = emulator();
    let total = 7_usize;
    for index in 0..total {
        let started = start_change_set(
            &router,
            create_change("AmiProduct", json!({"ProductTitle": format!("page-{index}")})),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "the fixture must start {total} change sets, got {} with {:?}",
            started.status, started.body
        );
    }
    wait_for_success();

    for page_size in [1_usize, 2, 3, 7, 20] {
        // Change sets first, then the entities those change sets made.
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"Catalog": CATALOG, "MaxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let answer = list_change_sets(&router, request).await;
            assert_eq!(
                answer.status, 200,
                "case {page_size}: ListChangeSets must answer 200, got {} with {:?}",
                answer.status, answer.body
            );
            let items = answer.list("/ChangeSetSummaryList");
            assert!(
                items.len() <= page_size,
                "case {page_size}: a page must hold at most MaxResults items, got {}",
                items.len()
            );
            for item in &items {
                seen.push(item["ChangeSetId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {page_size}: paging must terminate, got {pages} pages"
            );
            match answer.body.get("NextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        is_next_token(next),
                        "case {page_size}: NextToken must match ^[\\w+=.:@\\-\\/]+$, got {next:?}"
                    );
                    assert_eq!(
                        items.len(),
                        page_size,
                        "case {page_size}: a token is only owed when the page was full, got {}",
                        items.len()
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            total,
            "case {page_size}: paging must report every change set exactly once, got {seen:?}"
        );

        let mut entities: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request =
                json!({"Catalog": CATALOG, "EntityType": "AmiProduct", "MaxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let answer = list_entities(&router, request).await;
            let items = answer.list("/EntitySummaryList");
            assert!(
                items.len() <= page_size,
                "case {page_size}: an entity page must hold at most MaxResults items, got {}",
                items.len()
            );
            for item in &items {
                entities.push(item["EntityId"].as_str().unwrap_or_default().to_string());
            }
            match answer.body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut unique = entities.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            total,
            "case {page_size}: paging must report every entity exactly once, got {entities:?}"
        );
    }

    // A token this service never issued names no page.
    for token in ["not-a-token", "ENT-1", "CS", "CS-zz"] {
        let refused = list_change_sets(
            &router,
            json!({"Catalog": CATALOG, "NextToken": token}),
        )
        .await;
        assert_eq!(
            refused.status, 422,
            "case {token:?}: a foreign NextToken must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {token:?}: a foreign NextToken must answer ValidationException, got {:?}",
            refused.body
        );
    }
}

/// `Sort` orders what the list reports, over both attributes each list sorts on and both
/// directions; an attribute the list does not sort on is refused.
#[tokio::test]
async fn sort_orders_each_list() {
    let router = emulator();
    for index in 0..4 {
        start_change_set(
            &router,
            create_change("AmiProduct", json!({"ProductTitle": format!("sorted-{index}")})),
        )
        .await;
    }
    wait_for_success();

    let cases: Vec<(&str, Option<&str>, &str)> = vec![
        ("the default order", None, "DESCENDING"),
        ("StartTime ascending", Some("StartTime"), "ASCENDING"),
        ("StartTime descending", Some("StartTime"), "DESCENDING"),
        ("EndTime ascending", Some("EndTime"), "ASCENDING"),
        ("EndTime descending", Some("EndTime"), "DESCENDING"),
    ];
    for (case, sort_by, order) in cases {
        let mut request = json!({"Catalog": CATALOG});
        if case == "the default order" {
            // No `Sort` at all: the default is StartTime descending.
        } else {
            request["Sort"] = json!({"SortBy": sort_by, "SortOrder": order});
        }
        let answer = list_change_sets(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: ListChangeSets must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let keys: Vec<(String, String)> = answer
            .list("/ChangeSetSummaryList")
            .iter()
            .map(|item| {
                let attribute = match sort_by.unwrap_or("StartTime") {
                    "EndTime" => "EndTime",
                    _ => "StartTime",
                };
                (
                    item[attribute].as_str().unwrap_or_default().to_string(),
                    item["ChangeSetId"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            keys.len(),
            4,
            "case {case:?}: every change set must be reported, got {:?}",
            answer.body
        );
        let mut expected = keys.clone();
        expected.sort();
        if order == "DESCENDING" {
            expected.reverse();
        }
        assert_eq!(
            keys, expected,
            "case {case:?}: the list must be ordered by ({:?}, id) {order}, got {keys:?}",
            sort_by.unwrap_or("StartTime")
        );
    }

    // An attribute neither list sorts on is refused rather than ignored.
    // `SortBy` is a plain `^[a-zA-Z]+$` string, so an attribute the list does not sort on
    // reaches the handler and is refused there with the modelled 422.
    let refused = list_change_sets(
        &router,
        json!({"Catalog": CATALOG, "Sort": {"SortBy": "LastModifiedDate"}}),
    )
    .await;
    assert_eq!(
        refused.status, 422,
        "ListChangeSets sorting by an entity attribute must be refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "an unsupported SortBy must answer ValidationException, got {:?}",
        refused.body
    );

    // `SortOrder` is a modelled enum, so the boundary validator refuses it first.
    let bad_order = list_change_sets(
        &router,
        json!({"Catalog": CATALOG, "Sort": {"SortBy": "StartTime", "SortOrder": "SIDEWAYS"}}),
    )
    .await;
    assert_eq!(
        bad_order.status, 400,
        "an unmodelled SortOrder is refused at the boundary, got {} with {:?}",
        bad_order.status, bad_order.body
    );
    assert_eq!(
        bad_order.error_code(),
        "ValidationException",
        "an unmodelled SortOrder must answer ValidationException, got {:?}",
        bad_order.body
    );

    let entity_sorts: Vec<(&str, &str)> = vec![("EntityId", "ASCENDING"), ("LastModifiedDate", "DESCENDING")];
    for (sort_by, order) in entity_sorts {
        let answer = list_entities(
            &router,
            json!({
                "Catalog": CATALOG,
                "EntityType": "AmiProduct",
                "Sort": {"SortBy": sort_by, "SortOrder": order},
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {sort_by:?}: ListEntities must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let keys: Vec<(String, String)> = answer
            .list("/EntitySummaryList")
            .iter()
            .map(|item| {
                (
                    item[sort_by].as_str().unwrap_or_default().to_string(),
                    item["EntityId"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let mut expected = keys.clone();
        expected.sort();
        if order == "DESCENDING" {
            expected.reverse();
        }
        assert_eq!(
            keys, expected,
            "case {sort_by:?}: ListEntities must order by ({sort_by}, id) {order}, got {keys:?}"
        );
    }

    let refused = list_entities(
        &router,
        json!({"Catalog": CATALOG, "EntityType": "AmiProduct", "Sort": {"SortBy": "StartTime"}}),
    )
    .await;
    assert_eq!(
        refused.status, 422,
        "ListEntities sorting by a change-set attribute must be refused, got {} with {:?}",
        refused.status, refused.body
    );
}

/// `FilterList` selects from state, over every filter name each list publishes.
#[tokio::test]
async fn filters_select_from_state() {
    let router = emulator();
    let mut named = create_change("AmiProduct", json!({"ProductTitle": "filtered"}));
    named["ChangeSetName"] = json!("the-named-one");
    let first = start_change_set(&router, named).await;
    let first_id = first.text("/ChangeSetId");
    let second = start_change_set(
        &router,
        create_change("SaaSProduct", json!({"ProductTitle": "other"})),
    )
    .await;
    let second_id = second.text("/ChangeSetId");
    wait_for_success();

    let listed = list_change_sets(&router, json!({"Catalog": CATALOG})).await;
    let first_entity = listed
        .list("/ChangeSetSummaryList")
        .into_iter()
        .find(|item| item["ChangeSetId"] == json!(first_id))
        .map(|item| item["EntityIdList"][0].as_str().unwrap_or_default().to_string())
        .expect("the first change set must name its entity");
    let start_time = listed.list("/ChangeSetSummaryList")[0]["StartTime"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let before_everything = "2000-01-01T00:00:00Z";
    let after_everything = "2999-01-01T00:00:00Z";

    // Each row names a filter the documentation publishes, and the ids it must select.
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "ChangeSetName",
            json!([{"Name": "ChangeSetName", "ValueList": ["the-named-one"]}]),
            vec![first_id.clone()],
        ),
        (
            "ChangeSetName matching neither",
            json!([{"Name": "ChangeSetName", "ValueList": ["absent"]}]),
            vec![],
        ),
        (
            "Status with both statuses listed",
            json!([{"Name": "Status", "ValueList": ["SUCCEEDED", "FAILED"]}]),
            vec![first_id.clone(), second_id.clone()],
        ),
        (
            "Status matching none",
            json!([{"Name": "Status", "ValueList": ["CANCELLED"]}]),
            vec![],
        ),
        (
            "EntityId",
            json!([{"Name": "EntityId", "ValueList": [first_entity.clone()]}]),
            vec![first_id.clone()],
        ),
        (
            "AfterStartTime before every change set",
            json!([{"Name": "AfterStartTime", "ValueList": [before_everything]}]),
            vec![first_id.clone(), second_id.clone()],
        ),
        (
            "BeforeStartTime before every change set",
            json!([{"Name": "BeforeStartTime", "ValueList": [before_everything]}]),
            vec![],
        ),
        (
            "BeforeEndTime after every change set",
            json!([{"Name": "BeforeEndTime", "ValueList": [after_everything]}]),
            vec![first_id.clone(), second_id.clone()],
        ),
        (
            "AfterEndTime after every change set",
            json!([{"Name": "AfterEndTime", "ValueList": [after_everything]}]),
            vec![],
        ),
        (
            "two filters, which hold together",
            json!([
                {"Name": "Status", "ValueList": ["SUCCEEDED"]},
                {"Name": "ChangeSetName", "ValueList": ["the-named-one"]},
            ]),
            vec![first_id.clone()],
        ),
        (
            "two filters, which cannot both hold",
            json!([
                {"Name": "Status", "ValueList": ["CANCELLED"]},
                {"Name": "ChangeSetName", "ValueList": ["the-named-one"]},
            ]),
            vec![],
        ),
    ];

    for (case, filters, expected) in cases {
        let answer = list_change_sets(
            &router,
            json!({"Catalog": CATALOG, "FilterList": filters}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: ListChangeSets must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let mut got: Vec<String> = answer
            .list("/ChangeSetSummaryList")
            .iter()
            .map(|item| item["ChangeSetId"].as_str().unwrap_or_default().to_string())
            .collect();
        got.sort();
        let mut want = expected;
        want.sort();
        assert_eq!(
            got, want,
            "case {case:?}: the filter must select from state, got {:?}",
            answer.body
        );
    }

    // The `EntityId` filter `ListEntities` publishes, and the entity it must not select.
    let selected = list_entities(
        &router,
        json!({
            "Catalog": CATALOG,
            "EntityType": "AmiProduct",
            "FilterList": [{"Name": "EntityId", "ValueList": [first_entity.clone()]}],
        }),
    )
    .await;
    assert_eq!(
        selected
            .list("/EntitySummaryList")
            .iter()
            .map(|item| item["EntityId"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>(),
        vec![first_entity.clone()],
        "the EntityId filter must select that entity, got {:?}",
        selected.body
    );
    let missed = list_entities(
        &router,
        json!({
            "Catalog": CATALOG,
            "EntityType": "AmiProduct",
            "FilterList": [{"Name": "EntityId", "ValueList": ["ent-ffffffffffff"]}],
        }),
    )
    .await;
    assert!(
        missed.list("/EntitySummaryList").is_empty(),
        "a filter naming no stored entity must select none, got {:?}",
        missed.body
    );

    // `OwnershipType` SHARED names entities another account shared, of which there are
    // none in this store.
    let shared = list_entities(
        &router,
        json!({"Catalog": CATALOG, "EntityType": "AmiProduct", "OwnershipType": "SHARED"}),
    )
    .await;
    assert!(
        shared.list("/EntitySummaryList").is_empty(),
        "no entity in this store was shared from another account, got {:?}",
        shared.body
    );
    let owned = list_entities(
        &router,
        json!({"Catalog": CATALOG, "EntityType": "AmiProduct", "OwnershipType": "SELF"}),
    )
    .await;
    assert_eq!(
        owned.list("/EntitySummaryList").len(),
        1,
        "SELF names the entities this account's change sets made, got {:?}",
        owned.body
    );

    // A filter name neither list publishes, one carrying no value, and a before/after
    // bound that is not an instant this list can compare against, are refused.
    let refusals: Vec<(&str, Value)> = vec![
        (
            "a filter name ListChangeSets does not publish",
            json!({"Catalog": CATALOG, "FilterList": [{"Name": "Visibility", "ValueList": ["Public"]}]}),
        ),
        (
            "a time filter with two bounds",
            json!({"Catalog": CATALOG, "FilterList": [
                {"Name": "BeforeStartTime", "ValueList": [before_everything, after_everything]}
            ]}),
        ),
        (
            "a bound given as epoch seconds",
            json!({"Catalog": CATALOG, "FilterList": [
                {"Name": "AfterStartTime", "ValueList": ["1519739122"]}
            ]}),
        ),
        (
            "a bound with an offset instead of Z",
            json!({"Catalog": CATALOG, "FilterList": [
                {"Name": "BeforeEndTime", "ValueList": ["2018-02-27T13:45:22+00:00"]}
            ]}),
        ),
        (
            "a bound with fractional seconds",
            json!({"Catalog": CATALOG, "FilterList": [
                {"Name": "AfterEndTime", "ValueList": ["2018-02-27T13:45:22.500Z"]}
            ]}),
        ),
        (
            "a bound naming the thirteenth month",
            json!({"Catalog": CATALOG, "FilterList": [
                {"Name": "BeforeStartTime", "ValueList": ["2018-13-27T13:45:22Z"]}
            ]}),
        ),
    ];
    for (case, request) in refusals {
        let refused = list_change_sets(&router, request).await;
        assert_eq!(
            refused.status, 422,
            "case {case:?}: must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: must answer ValidationException, got {:?}",
            refused.body
        );
    }
    // The type-specific filter and sort surfaces select and order the same entities the
    // generic ones do, so each is served rather than refused.
    let type_specific: Vec<(&str, Value)> = vec![
        (
            "EntityTypeFilters",
            json!({"AmiProductFilters": {"EntityId": {"ValueList": [first_entity.clone()]}}}),
        ),
        (
            "EntityTypeSort",
            json!({"AmiProductSort": {"SortBy": "EntityId", "SortOrder": "ASCENDING"}}),
        ),
    ];
    for (member, value) in type_specific {
        let mut request = json!({"Catalog": CATALOG, "EntityType": "AmiProduct"});
        request[member] = value;
        let served = list_entities(&router, request).await;
        assert_eq!(
            served.status, 200,
            "case {member:?}: the per-type surface must be served, got {} with {:?}",
            served.status, served.body
        );
        let ids: Vec<Value> = served
            .list("/EntitySummaryList")
            .iter()
            .map(|item| item["EntityId"].clone())
            .collect();
        assert!(
            ids.contains(&json!(first_entity)),
            "case {member:?}: the AmiProduct entity must be listed, got {ids:?}"
        );
    }

    assert!(
        start_time.len() == 20,
        "the fixture's StartTime must be an ISO-8601 instant, got {start_time:?}"
    );
}

/// A running change set locks the entities it targets, and one change set cannot run the
/// same change type against one entity twice.
#[tokio::test]
async fn a_running_change_set_locks_its_entities() {
    let router = emulator();
    let entity_id = created_entity(
        &router,
        create_change("AmiProduct", json!({"ProductTitle": "locked"})),
    )
    .await;

    let against_entity = json!({
        "Catalog": CATALOG,
        "ChangeSet": [{
            "ChangeType": "UpdateInformation",
            "Entity": {"Type": "AmiProduct@1.0", "Identifier": entity_id.clone()},
            "DetailsDocument": {"ProductTitle": "holder"},
        }],
    });
    let holder = start_change_set(&router, against_entity.clone()).await;
    assert_eq!(
        holder.status, 200,
        "the first change set against the entity must be accepted, got {} with {:?}",
        holder.status, holder.body
    );

    // While it runs, nothing else may touch that entity.
    let blocked = start_change_set(&router, against_entity.clone()).await;
    assert_eq!(
        blocked.status, 423,
        "a change against a locked entity must answer 423, got {} with {:?}",
        blocked.status, blocked.body
    );
    assert_eq!(
        blocked.error_code(),
        "ResourceInUseException",
        "a change against a locked entity must answer ResourceInUseException, got {:?}",
        blocked.body
    );

    // Once the change set has completed, the lock is released.
    wait_for_success();
    let accepted = start_change_set(&router, against_entity).await;
    assert_eq!(
        accepted.status, 200,
        "a completed change set must release its entities, got {} with {:?}",
        accepted.status, accepted.body
    );

    // And one change set may not run one change type against one entity twice: that
    // request contradicts itself, so it is refused as invalid rather than as in use.
    wait_for_success();
    let twice = start_change_set(
        &router,
        json!({
            "Catalog": CATALOG,
            "ChangeSet": [
                {
                    "ChangeType": "AddRevisions",
                    "Entity": {"Type": "AmiProduct@1.0", "Identifier": entity_id.clone()},
                    "Details": "{\"Revision\":1}",
                },
                {
                    "ChangeType": "AddRevisions",
                    "Entity": {"Type": "AmiProduct@1.0", "Identifier": entity_id.clone()},
                    "Details": "{\"Revision\":2}",
                },
            ],
        }),
    )
    .await;
    assert_eq!(
        (twice.status, twice.error_code()),
        (422, "ValidationException"),
        "the same change type twice against one entity must answer 422 ValidationException, \
         got {} {:?} with {:?}",
        twice.status,
        twice.error_code(),
        twice.body
    );
    assert!(
        twice.text("/Message").contains("AddRevisions")
            && twice.text("/Message").contains(&entity_id),
        "the refusal must name the repeated change type and the entity, got {:?}",
        twice.body
    );

    // Two different change types against the same entity are fine.
    let mixed = start_change_set(
        &router,
        json!({
            "Catalog": CATALOG,
            "ChangeSet": [
                {
                    "ChangeType": "AddRevisions",
                    "Entity": {"Type": "AmiProduct@1.0", "Identifier": entity_id.clone()},
                    "Details": "{\"Revision\":1}",
                },
                {
                    "ChangeType": "UpdateInformation",
                    "Entity": {"Type": "AmiProduct@1.0", "Identifier": entity_id.clone()},
                    "Details": "{\"ProductTitle\":\"mixed\"}",
                },
            ],
        }),
    )
    .await;
    assert_eq!(
        mixed.status, 200,
        "two different change types against one entity must be accepted, got {} with {:?}",
        mixed.status, mixed.body
    );
}

/// An id that names no record is a `ResourceNotFoundException`, for every operation that
/// reads one — never a 501 and never an empty success.
#[tokio::test]
async fn an_unknown_id_is_not_found() {
    let router = emulator();
    let cases: Vec<(&str, Answer)> = vec![
        (
            "DescribeChangeSet",
            describe_change_set(&router, "cs-does-not-exist").await,
        ),
        (
            "CancelChangeSet",
            cancel_change_set(&router, "cs-does-not-exist").await,
        ),
        (
            "DescribeEntity",
            describe_entity(&router, "ent-does-not-exist").await,
        ),
        (
            "StartChangeSet against an unknown entity",
            start_change_set(
                &router,
                json!({
                    "Catalog": CATALOG,
                    "ChangeSet": [{
                        "ChangeType": "UpdateInformation",
                        "Entity": {"Type": "AmiProduct@1.0", "Identifier": "ent-does-not-exist"},
                        "Details": "{}",
                    }],
                }),
            )
            .await,
        ),
    ];
    for (case, answer) in cases {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an unknown id must answer 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: an unknown id must answer ResourceNotFoundException, got {:?}",
            answer.body
        );
    }
}

/// Every operation refuses a `Catalog` other than the fixed value its model documents,
/// and refuses a change that carries both details members.
#[tokio::test]
async fn a_request_outside_the_contract_is_refused() {
    let router = emulator();

    // `Catalog` is modelled `^[a-zA-Z]+$`, so each of these reaches the handler and is
    // refused there rather than at the boundary.
    for catalog in ["Marketplace", "awsmarketplace", "AWSMarketplaceX"] {
        let cases: Vec<(&str, Answer)> = vec![
            (
                "StartChangeSet",
                start_change_set(
                    &router,
                    json!({
                        "Catalog": catalog,
                        "ChangeSet": [{
                            "ChangeType": "CreateProduct",
                            "Entity": {"Type": "AmiProduct@1.0"},
                            "Details": "{}",
                        }],
                    }),
                )
                .await,
            ),
            (
                "DescribeChangeSet",
                call(
                    &router,
                    "GET",
                    &format!("/DescribeChangeSet?catalog={catalog}&changeSetId=cs-1"),
                    None,
                )
                .await,
            ),
            (
                "CancelChangeSet",
                call(
                    &router,
                    "PATCH",
                    &format!("/CancelChangeSet?catalog={catalog}&changeSetId=cs-1"),
                    None,
                )
                .await,
            ),
            (
                "DescribeEntity",
                call(
                    &router,
                    "GET",
                    &format!("/DescribeEntity?catalog={catalog}&entityId=ent-1"),
                    None,
                )
                .await,
            ),
            (
                "ListChangeSets",
                list_change_sets(&router, json!({"Catalog": catalog})).await,
            ),
            (
                "ListEntities",
                list_entities(&router, json!({"Catalog": catalog, "EntityType": "AmiProduct"}))
                    .await,
            ),
        ];
        for (case, answer) in cases {
            assert_eq!(
                answer.status, 422,
                "case {catalog:?}/{case}: a catalog other than the fixed value must answer 422, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {catalog:?}/{case}: must answer ValidationException, got {:?}",
                answer.body
            );
        }
    }

    let handler_refusals: Vec<(&str, Value)> = vec![
        (
            "both Details and DetailsDocument",
            json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": "CreateProduct",
                    "Entity": {"Type": "AmiProduct@1.0"},
                    "Details": "{}",
                    "DetailsDocument": {},
                }],
            }),
        ),
        (
            "Details that is not an object",
            json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": "CreateProduct",
                    "Entity": {"Type": "AmiProduct@1.0"},
                    "DetailsDocument": ["not", "an", "object"],
                }],
            }),
        ),
    ];
    for (case, request) in handler_refusals {
        let refused = start_change_set(&router, request).await;
        assert_eq!(
            refused.status, 422,
            "case {case:?}: must answer 422, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: must answer ValidationException, got {:?}",
            refused.body
        );
    }

    // Details are stored as deep as the bound and no deeper, through both members: one
    // level past it is refused rather than walked. (A document deeper than the
    // framework's own deserializer limit never reaches a handler at all, so the rows stay
    // inside it.)
    let depths: Vec<(usize, bool)> = vec![
        (1, true),
        (2, true),
        (MAX_DETAILS_DEPTH, true),
        (MAX_DETAILS_DEPTH + 1, false),
        (MAX_DETAILS_DEPTH + 20, false),
    ];
    for (depth, accepted) in depths {
        for member in ["Details", "DetailsDocument"] {
            let router = emulator();
            let nested = nested_details(depth);
            let mut change = json!({
                "ChangeType": "CreateProduct",
                "Entity": {"Type": "AmiProduct@1.0"},
            });
            change[member] = if member == "Details" {
                json!(nested.to_string())
            } else {
                nested
            };
            let answer = start_change_set(
                &router,
                json!({"Catalog": CATALOG, "ChangeSet": [change]}),
            )
            .await;
            if accepted {
                assert_eq!(
                    answer.status, 200,
                    "case {member}@{depth}: details within the bound must be accepted, got {} \
                     with {:?}",
                    answer.status, answer.body
                );
                // And what was stored is readable back, at that depth.
                let described = describe_change_set(&router, &answer.text("/ChangeSetId")).await;
                assert_eq!(
                    described.status, 200,
                    "case {member}@{depth}: the stored change set must be readable, got {} \
                     with {:?}",
                    described.status, described.body
                );
            } else {
                assert_eq!(
                    answer.status, 422,
                    "case {member}@{depth}: details past the bound must be refused, got {} \
                     with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {member}@{depth}: must answer ValidationException, got {:?}",
                    answer.body
                );
            }
        }
    }

    // The modelled constraints are refused before the handler runs — a 4xx client
    // refusal, never a 501 or a success.
    let boundary: Vec<(&str, Value)> = vec![
        (
            "an empty ChangeSet list",
            json!({"Catalog": CATALOG, "ChangeSet": []}),
        ),
        (
            "a Catalog outside ^[a-zA-Z]+$",
            json!({
                "Catalog": "AWS_Marketplace",
                "ChangeSet": [{
                    "ChangeType": "CreateProduct",
                    "Entity": {"Type": "AmiProduct@1.0"},
                    "Details": "{}",
                }],
            }),
        ),
        (
            "no ChangeSet at all",
            json!({"Catalog": CATALOG}),
        ),
        (
            "an Intent the model has no value for",
            json!({
                "Catalog": CATALOG,
                "Intent": "SIDEWAYS",
                "ChangeSet": [{
                    "ChangeType": "CreateProduct",
                    "Entity": {"Type": "AmiProduct@1.0"},
                    "Details": "{}",
                }],
            }),
        ),
    ];
    for (case, request) in boundary {
        let refused = start_change_set(&router, request).await;
        assert!(
            (400..500).contains(&refused.status),
            "case {case:?}: a modelled constraint must be refused as a client error, got {} \
             with {:?}",
            refused.status,
            refused.body
        );
        assert_ne!(
            refused.status, 501,
            "case {case:?}: a bad request is not an unimplemented operation, got {:?}",
            refused.body
        );
    }
}
