// SPDX-License-Identifier: CC0-1.0

//! The entity READ surface of the AWS Marketplace Catalog, end to end over HTTP:
//! `BatchDescribeEntities`, `DescribeEntity` and `ListEntities`.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! Three rules are under test:
//!
//! 1. An entity id is checked against the form this service's own `EntityId` shape
//!    states (`@pattern("^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$")`) BEFORE any lookup:
//!    an id outside it is a 422 `ValidationException`, and only a well-formed id reaches
//!    the lookup that answers 404 `ResourceNotFoundException`.
//! 2. A batch read degrades PER ENTRY: it reports both `EntityDetails` and `Errors`,
//!    empty maps included, with every entity the catalog holds under its id in the first
//!    and every entry it cannot serve under the id that was asked for in the second —
//!    whether that entry named a missing entity or an id that is no entity id at all.
//! 3. A list reports what the store holds for the type and filters asked for — an empty
//!    `EntitySummaryList` (present, not absent) when nothing matches — and refuses a
//!    `Sort$SortBy` outside the attributes it documents with a 422.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, ENTITY_ID_PATTERN, ENTITY_SORT_BY,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`).
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The largest `EntityRequestList` the model accepts: `@length(min: 1, max: 20)`.
const ENTITY_REQUEST_LIST_MAX: usize = 20;

/// How long after the start a change set has certainly reached `SUCCEEDED`.
fn wait_for_success() {
    std::thread::sleep(CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400));
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

    /// The value at `pointer`, or `Value::Null` when the member is absent.
    fn at(&self, pointer: &str) -> Value {
        self.body.pointer(pointer).cloned().unwrap_or(Value::Null)
    }

    /// The keys of the JSON object at `pointer`, sorted, or an empty list when the
    /// member is absent or not an object.
    fn keys(&self, pointer: &str) -> Vec<String> {
        let mut keys: Vec<String> = self
            .body
            .pointer(pointer)
            .and_then(Value::as_object)
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();
        keys
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

async fn describe_entity(router: &MultiServiceRouter, entity_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeEntity?catalog={CATALOG}&entityId={entity_id}"),
        None,
    )
    .await
}

async fn batch_describe_entities(router: &MultiServiceRouter, entity_ids: &[String]) -> Answer {
    let entries: Vec<Value> = entity_ids
        .iter()
        .map(|entity_id| json!({"Catalog": CATALOG, "EntityId": entity_id}))
        .collect();
    call(
        router,
        "POST",
        "/BatchDescribeEntities",
        Some(json!({"EntityRequestList": entries})),
    )
    .await
}

async fn list_entities(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/ListEntities", Some(request)).await
}

/// Create one entity of `entity_type` through a change set, wait for the clock to apply
/// it, and answer the id the catalog minted for it.
async fn created_entity(router: &MultiServiceRouter, entity_type: &str, title: &str) -> String {
    let started = call(
        router,
        "POST",
        "/StartChangeSet",
        Some(json!({
            "Catalog": CATALOG,
            "ChangeSet": [{
                "ChangeType": "CreateProduct",
                "Entity": {"Type": format!("{entity_type}@1.0")},
                "Details": json!({"ProductTitle": title}).to_string(),
            }],
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {title:?}: StartChangeSet must answer 200, got {} with {:?}",
        started.status, started.body
    );
    wait_for_success();
    let listed = list_entities(router, json!({"Catalog": CATALOG, "EntityType": entity_type})).await;
    let found = listed
        .body
        .pointer("/EntitySummaryList")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["Name"] == json!(title))
                .and_then(|item| item["EntityId"].as_str().map(str::to_string))
        });
    found.unwrap_or_else(|| {
        panic!(
            "case {title:?}: the applied change set must leave a readable entity, got {:?}",
            listed.body
        )
    })
}

/// RULE 1: the entity-id FORM decides before the store is consulted — an id outside the
/// `EntityId` pattern is a 422 and a well-formed one the catalog does not hold is a 404.
///
/// The pattern is the model's own (`smithy_query(query="shape", shape="EntityId")`:
/// `^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$`), so the rows walk the classes it
/// distinguishes — a length below the three characters its `[..]+` middle requires, a
/// character `ResourceId` admits but `EntityId` does not (`_`), and a first or last
/// character outside `[a-zA-Z0-9]`.
#[tokio::test]
async fn an_entity_id_outside_the_published_form_is_refused_before_the_lookup() {
    let router = emulator();

    // (case, entity id)
    let refused: Vec<(&str, &str)> = vec![
        ("one character: shorter than the pattern's three", "a"),
        ("two characters: the middle class needs at least one", "ab"),
        (
            "underscore: ResourceId admits it, EntityId does not",
            "prod_abcdef",
        ),
        ("underscore last", "prod-abcdef_"),
        ("leading hyphen", "-prod-abcdef"),
        ("trailing hyphen", "prod-abcdef-"),
    ];
    for (case, entity_id) in refused {
        let answer = describe_entity(&router, entity_id).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (422, "ValidationException"),
            "case {case:?}: entity id {entity_id:?} must be refused 422 ValidationException \
             before any lookup, got {} {:?}",
            answer.status,
            answer.body
        );
        // A refusal says what was wrong with the request it refused: the id it rejected
        // and the form the service requires. This service states that in `Message`
        // alone, with no `ValidationExceptionFieldList` beside it.
        let message = answer.text("/Message");
        assert!(
            message.contains(entity_id) && message.contains(ENTITY_ID_PATTERN),
            "case {case:?}: the refusal must name the rejected id and the required pattern, \
             got {message:?}"
        );
        assert_eq!(
            answer.at("/ValidationExceptionFieldList"),
            Value::Null,
            "case {case:?}: this service reports a refusal in Message alone, got {:?}",
            answer.body
        );
    }

    // The accepting side: a well-formed id the catalog does not hold reaches the lookup,
    // which is the modelled 404 — including at the published length bound (`ResourceId`
    // and `EntityId` are both `@length(min: 1, max: 255)`).
    let longest = "a".repeat(255);
    let not_found: Vec<(&str, &str)> = vec![
        ("three characters: the shortest well-formed id", "a-b"),
        ("digits only", "0-1"),
        (
            "a well-formed product id the catalog does not hold",
            "prod-abcdefghijklmnop01",
        ),
        ("the published length bound: 255 characters", &longest),
    ];
    for (case, entity_id) in not_found {
        let answer = describe_entity(&router, entity_id).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a well-formed id the catalog does not hold must answer 404 \
             ResourceNotFoundException, got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            answer.text("/Message").contains(entity_id),
            "case {case:?}: the message must name the entity asked for, got {:?}",
            answer.body
        );
    }

    // `DescribeEntityRequest$EntityId` targets the wider `ResourceId`, whose `@pattern`
    // (`^[\w\-]+$`) is not the syntax this service publishes for an entity id: it refuses
    // the `.` and the `/` the published syntax admits. The published syntax is what
    // decides, so an id holding them is well formed and reaches the lookup — the modelled
    // 404, naming the entity that was asked for.
    for entity_id in ["a0.b-c/d1", "prod/abcdef", "prod.abcdef"] {
        let answer = describe_entity(&router, entity_id).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "ResourceNotFoundException"),
            "case {entity_id:?}: an id inside the published syntax must reach the lookup, \
             got {} {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        assert_eq!(
            answer.text("/Message"),
            format!("Requested entity '{entity_id}' does not exist."),
            "case {entity_id:?}: the 404 must name the entity asked for, got {:?}",
            answer.body
        );
    }

    // The same two characters do not make an id well formed on their own: one that leaves
    // the published syntax by where it carries them is still the 422, in the sentence that
    // quotes the syntax it had to satisfy.
    for entity_id in ["prod-abcdef.", "/prod-abcdef", ".prod/abcdef"] {
        let answer = describe_entity(&router, entity_id).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (422, "ValidationException"),
            "case {entity_id:?}: an id outside the published syntax must be refused before \
             any lookup, got {} {:?} with {:?}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        assert_eq!(
            answer.text("/Message"),
            format!(
                "[Requested entity id '{entity_id}' is invalid. It should match with \
                 {ENTITY_ID_PATTERN}.]"
            ),
            "case {entity_id:?}: the refusal must name the id and the required syntax, got \
             {:?}",
            answer.body
        );
    }
}

/// One row of the batch-read table: the case name, the ids the call asks for, the ids
/// expected in `EntityDetails` and the ids expected in `Errors`.
struct BatchRow {
    case: &'static str,
    requested: Vec<String>,
    held: Vec<String>,
    missing: Vec<String>,
}

/// RULE 2: a batch read reports BOTH maps and degrades per entity — a missing entity is
/// an `Errors` entry under its own id, never a failed call.
///
/// The rows differ along what the rule generalises over: how many entities are asked
/// for, how many of them exist, whether an id repeats, and the modelled maximum batch.
#[tokio::test]
async fn a_batch_read_reports_held_entities_and_an_error_for_the_rest() {
    let router = emulator();
    let ami = created_entity(&router, "AmiProduct", "batch-ami").await;
    let saas = created_entity(&router, "SaaSProduct", "batch-saas").await;
    let missing_one = "prod-batchmissing01".to_string();
    let missing_two = "prod-batchmissing02".to_string();

    let mut full_batch: Vec<String> = vec![ami.clone(), saas.clone()];
    while full_batch.len() < ENTITY_REQUEST_LIST_MAX {
        full_batch.push(format!("prod-batchfiller{:04}", full_batch.len()));
    }

    // (case, requested ids, ids expected in EntityDetails, ids expected in Errors)
    let rows: Vec<BatchRow> = vec![
        BatchRow {
            case: "none of the requested entities exists",
            requested: vec![missing_one.clone(), missing_two.clone()],
            held: vec![],
            missing: vec![missing_one.clone(), missing_two.clone()],
        },
        BatchRow {
            case: "a single missing entity",
            requested: vec![missing_one.clone()],
            held: vec![],
            missing: vec![missing_one.clone()],
        },
        BatchRow {
            case: "every requested entity exists",
            requested: vec![ami.clone(), saas.clone()],
            held: vec![ami.clone(), saas.clone()],
            missing: vec![],
        },
        BatchRow {
            case: "one held, one missing",
            requested: vec![ami.clone(), missing_two.clone()],
            held: vec![ami.clone()],
            missing: vec![missing_two.clone()],
        },
        BatchRow {
            case: "the same id twice, reported once",
            requested: vec![ami.clone(), ami.clone()],
            held: vec![ami.clone()],
            missing: vec![],
        },
        BatchRow {
            case: "the modelled maximum of 20 entries",
            requested: full_batch.clone(),
            held: vec![ami.clone(), saas.clone()],
            missing: full_batch[2..].to_vec(),
        },
    ];

    for BatchRow {
        case,
        requested,
        held: expected_details,
        missing: expected_errors,
    } in rows
    {
        let answer = batch_describe_entities(&router, &requested).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a batch read answers 200 however many entities are missing, \
             got {} with {:?}",
            answer.status, answer.body
        );
        // BOTH members are present, the empty one as an empty object.
        assert!(
            answer.at("/EntityDetails").is_object() && answer.at("/Errors").is_object(),
            "case {case:?}: both EntityDetails and Errors must be reported, got {:?}",
            answer.body
        );

        let mut details_keys = expected_details.clone();
        details_keys.sort();
        details_keys.dedup();
        let mut error_keys = expected_errors.clone();
        error_keys.sort();
        error_keys.dedup();
        assert_eq!(
            answer.keys("/EntityDetails"),
            details_keys,
            "case {case:?}: EntityDetails must be keyed by the entities the catalog holds, \
             got {:?}",
            answer.body
        );
        assert_eq!(
            answer.keys("/Errors"),
            error_keys,
            "case {case:?}: Errors must be keyed by the entities it does not, got {:?}",
            answer.body
        );

        for entity_id in &error_keys {
            assert_eq!(
                answer.text(&format!("/Errors/{entity_id}/ErrorCode")),
                "ResourceNotFoundException",
                "case {case:?}: a missing entity carries the error a single read raises, \
                 got {:?}",
                answer.body
            );
            assert!(
                answer
                    .text(&format!("/Errors/{entity_id}/ErrorMessage"))
                    .contains(entity_id.as_str()),
                "case {case:?}: the per-entity message must name the missing entity, got {:?}",
                answer.body
            );
            // The same entity, read on its own, is the modelled 404.
            let single = describe_entity(&router, entity_id).await;
            assert_eq!(
                (single.status, single.error_code()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: entity {entity_id:?} must be missing for a single read too, \
                 got {} {:?}",
                single.status,
                single.body
            );
        }

        // ROUND TRIP: what the batch reports for a held entity is what the single read
        // reports for it — the same stored record, not a second copy.
        for entity_id in &details_keys {
            let single = describe_entity(&router, entity_id).await;
            assert_eq!(
                single.status, 200,
                "case {case:?}: entity {entity_id:?} must be readable, got {} with {:?}",
                single.status, single.body
            );
            for member in ["EntityType", "EntityIdentifier", "EntityArn", "LastModifiedDate"] {
                assert_eq!(
                    answer.at(&format!("/EntityDetails/{entity_id}/{member}")),
                    single.at(&format!("/{member}")),
                    "case {case:?}: entity {entity_id:?} must report the same {member} in a \
                     batch read as in a single one, got {:?}",
                    answer.body
                );
            }
            assert_eq!(
                answer.at(&format!("/EntityDetails/{entity_id}/DetailsDocument")),
                single.at("/DetailsDocument"),
                "case {case:?}: entity {entity_id:?} must report the same DetailsDocument in a \
                 batch read as in a single one, got {:?}",
                answer.body
            );
        }
    }
}

/// A batch entry whose id is not in the `EntityId` form is reported PER ENTRY in
/// `Errors` — under the id that was asked for, with the `ValidationException` code a
/// single read of that id would have raised — and the call itself succeeds, so the
/// entries beside it are still served.
#[tokio::test]
async fn a_batch_entry_with_a_malformed_entity_id_is_reported_per_entry() {
    let router = emulator();
    let held = created_entity(&router, "AmiProduct", "batch-partial").await;
    for entity_id in ["a", "prod_abcdef", "-prod-abcdef", "bad entity id!"] {
        let answer =
            batch_describe_entities(&router, &[held.clone(), entity_id.to_string()]).await;
        assert_eq!(
            answer.status, 200,
            "case {entity_id:?}: a batch carrying a malformed id must still answer 200, \
             got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.text(&format!("/Errors/{entity_id}/ErrorCode")),
            "ValidationException",
            "case {entity_id:?}: the malformed entry must be reported under its own id \
             with the ValidationException code, got {:?}",
            answer.body
        );
        assert!(
            answer
                .text(&format!("/Errors/{entity_id}/ErrorMessage"))
                .contains(ENTITY_ID_PATTERN),
            "case {entity_id:?}: the per-entry message must quote the required form, \
             got {:?}",
            answer.body
        );
        assert_eq!(
            answer.keys("/EntityDetails"),
            vec![held.clone()],
            "case {entity_id:?}: the entity the other entry asked for must still be \
             served, and the malformed one must not appear in EntityDetails, got {:?}",
            answer.body
        );
    }
}

/// RULE 3 (empty result): a list that matches nothing answers 200 with an EMPTY
/// `EntitySummaryList` and no `NextToken` — the member is present, not absent.
///
/// The rows differ along what the rule generalises over: which entity type is asked
/// for, whether a filter selects it, how it is sorted, and how large a page is asked
/// for.
#[tokio::test]
async fn a_list_that_matches_nothing_reports_an_empty_list() {
    let router = emulator();
    // One entity exists, of a type none of the rows below asks for — so an empty answer
    // is "nothing matched", not "the store is empty".
    let held = created_entity(&router, "AmiProduct", "list-ami").await;

    // (case, request)
    let rows: Vec<(&str, Value)> = vec![
        (
            "a type with no entities",
            json!({"Catalog": CATALOG, "EntityType": "ContainerProduct"}),
        ),
        (
            "an EntityId filter matching nothing",
            json!({
                "Catalog": CATALOG,
                "EntityType": "AmiProduct",
                "FilterList": [{"Name": "EntityId", "ValueList": ["prod-nosuchentity01"]}],
            }),
        ),
        (
            "an EntityId filter naming an entity of another type",
            json!({
                "Catalog": CATALOG,
                "EntityType": "SaaSProduct",
                "FilterList": [{"Name": "EntityId", "ValueList": [held.clone()]}],
            }),
        ),
        (
            "sorted, with nothing to sort",
            json!({
                "Catalog": CATALOG,
                "EntityType": "Offer",
                "Sort": {"SortBy": "EntityId", "SortOrder": "ASCENDING"},
            }),
        ),
        (
            "shared ownership, which this account owns none of",
            json!({
                "Catalog": CATALOG,
                "EntityType": "AmiProduct",
                "OwnershipType": "SHARED",
                "MaxResults": 50,
            }),
        ),
    ];

    for (case, request) in rows {
        let answer = list_entities(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a list that matches nothing answers 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.at("/EntitySummaryList"),
            json!([]),
            "case {case:?}: EntitySummaryList must be reported as an empty array, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.at("/NextToken"),
            Value::Null,
            "case {case:?}: a last page carries no NextToken, got {:?}",
            answer.body
        );
    }

    // The same list, asked for the type that DOES hold an entity, reports it — so the
    // rows above are empty because nothing matched.
    let matching = list_entities(
        &router,
        json!({
            "Catalog": CATALOG,
            "EntityType": "AmiProduct",
            "FilterList": [{"Name": "EntityId", "ValueList": [held.clone()]}],
        }),
    )
    .await;
    assert_eq!(
        matching
            .at("/EntitySummaryList")
            .as_array()
            .map(|items| items
                .iter()
                .map(|item| item["EntityId"].clone())
                .collect::<Vec<_>>())
            .unwrap_or_default(),
        vec![json!(held)],
        "the entity the store holds must be listed when the filter selects it, got {:?}",
        matching.body
    );
}

/// RULE 3 (sort attribute): `Sort$SortBy` outside the attributes `ListEntities`
/// documents is refused 422 `ValidationException`; each documented attribute is served.
///
/// The accepted set is the model's own doc on `Sort$SortBy` ("For `ListEntities`,
/// supported attributes include `LastModifiedDate` (default) and `EntityId`"), which the
/// crate publishes as `ENTITY_SORT_BY` — asserted here against that list so a row cannot
/// drift from it.
#[tokio::test]
async fn an_unsupported_sort_attribute_is_refused() {
    let router = emulator();
    created_entity(&router, "AmiProduct", "sort-ami").await;

    let refused: Vec<(&str, &str)> = vec![
        ("the recorded unknown attribute", "NotARealSortField"),
        ("an attribute of another list", "StartTime"),
        ("a summary member that is not a sort attribute", "Name"),
        ("a supported attribute in the wrong case", "entityid"),
    ];
    for (case, sort_by) in refused {
        assert!(
            !ENTITY_SORT_BY.contains(&sort_by),
            "case {case:?}: {sort_by:?} must not be one of the supported attributes {:?}",
            ENTITY_SORT_BY
        );
        let answer = list_entities(
            &router,
            json!({
                "Catalog": CATALOG,
                "EntityType": "AmiProduct",
                "Sort": {"SortBy": sort_by},
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (422, "ValidationException"),
            "case {case:?}: sorting by {sort_by:?} must answer 422 ValidationException, \
             got {} {:?}",
            answer.status,
            answer.body
        );
        // The message is built from the request — the attribute it asked for — and renders
        // the allowed-value set the MODEL publishes for `Sort$SortBy`, which declares
        // none, so that slot is the empty list.
        let message = answer.text("/Message");
        assert_eq!(
            message,
            format!("[Requested sort by '{sort_by}' is invalid. It should be one of '[]'.]"),
            "case {case:?}: the refusal must name the rejected attribute and render the \
             unpublished set as an empty list, got {message:?}"
        );
    }

    for sort_by in ENTITY_SORT_BY {
        for sort_order in ["ASCENDING", "DESCENDING"] {
            let answer = list_entities(
                &router,
                json!({
                    "Catalog": CATALOG,
                    "EntityType": "AmiProduct",
                    "Sort": {"SortBy": sort_by, "SortOrder": sort_order},
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {sort_by:?}/{sort_order:?}: a documented sort attribute must be served, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.at("/EntitySummaryList").as_array().map(Vec::len),
                Some(1),
                "case {sort_by:?}/{sort_order:?}: the stored entity must be listed, got {:?}",
                answer.body
            );
        }
    }
}
