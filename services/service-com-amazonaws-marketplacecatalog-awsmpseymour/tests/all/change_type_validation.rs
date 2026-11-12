// SPDX-License-Identifier: CC0-1.0

//! `Change$ChangeType` against the changes this catalog performs, end to end over HTTP.
//!
//! The rule under test: a change type names the change the catalog is to make, so
//! `StartChangeSet` judges it against the changes this catalog PERFORMS — a value inside
//! `ChangeType`'s syntax (`^[A-Z][\w]*$`) that names none of them asks for no change the
//! catalog can make, so the whole change set is refused with the modelled
//! `ValidationException` (HTTP 422) and nothing is started: no change set is stored and no
//! entity is minted. A change type the catalog does perform is accepted, runs, and is
//! reported back from what was stored.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, CHANGE_TYPES, MAX_CHANGES_PER_CHANGE_SET,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// The restJson1 document media type: `application/json`, for answers and refusals alike.
const JSON: &str = "application/json";

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

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status, the response media type and the JSON body.
struct Answer {
    status: u16,
    content_type: String,
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
        builder = builder.header("content-type", JSON);
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
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
    Answer {
        status,
        content_type,
        body,
    }
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

async fn list_change_sets(router: &MultiServiceRouter) -> Answer {
    call(
        router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG})),
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

async fn describe_entity(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeEntity?catalog={CATALOG}&entityId={id}"),
        None,
    )
    .await
}

/// One change: a change type, the entity type it acts under, and the details it carries.
///
/// `Change$Entity$Type` carries an entity type at a schema version, which is the form the
/// service publishes for the member; a row that names its own version keeps it.
fn change(change_type: &str, entity_type: &str, title: &str) -> Value {
    let reference = if entity_type.contains('@') {
        entity_type.to_string()
    } else {
        format!("{entity_type}@1.0")
    };
    json!({
        "ChangeType": change_type,
        "Entity": {"Type": reference},
        "DetailsDocument": {"ProductTitle": title},
    })
}

/// A `StartChangeSet` body carrying `changes` as its whole `ChangeSet`.
fn request(changes: Value) -> Value {
    json!({"Catalog": CATALOG, "ChangeSet": changes})
}

/// The refusal a change type this catalog performs no change for is answered with,
/// spelled out here rather than taken from the crate so a wrong implementation cannot
/// make this pass: the value the request named, and no syntax to satisfy — a value may
/// well be inside `ChangeType`'s pattern and still name no change of the catalog.
fn invalid_change_type_message(change_type: &str) -> String {
    format!("[Requested change type '{change_type}' is invalid.]")
}

/// A change type this catalog performs no such change for is refused with the modelled
/// `ValidationException`, and the change set is started under no circumstances.
///
/// The rows differ along what the rule generalises over: a word that is no change type at
/// all, a near miss of one the catalog does perform (singular, plural, another verb on the
/// same noun, a suffix), a bare verb, a value that names an ENTITY type rather than a
/// change, and the shortest value the pattern admits. Every row satisfies
/// `ChangeType`'s `@pattern` and `@length`, so what is under test is the catalog's set of
/// changes and not the boundary's syntax check.
#[tokio::test]
async fn a_change_type_this_catalog_performs_no_change_for_is_refused() {
    let rows = [
        ("a word that is no change type", "Nonsense"),
        ("the singular of one the catalog performs", "AddRevision"),
        ("the plural of one the catalog performs", "UpdateInformations"),
        ("another verb on the same noun", "RemoveDeliveryOptions"),
        ("one the catalog performs with a suffix", "CreateProductV2"),
        ("the verb on its own", "Create"),
        ("an entity type rather than a change", "AmiProduct"),
        ("the shortest value the pattern admits", "Z"),
    ];

    for (case, change_type) in rows {
        let router = emulator();
        let refused = start_change_set(
            &router,
            request(json!([change(change_type, "AmiProduct", "refused")])),
        )
        .await;

        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {case:?} ({change_type}): expected 422 ValidationException, got {} {:?} \
             with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.content_type, JSON,
            "case {case:?} ({change_type}): expected the refusal as {JSON}, got {:?}",
            refused.content_type
        );
        assert_eq!(
            refused.text("/Message"),
            invalid_change_type_message(change_type),
            "case {case:?} ({change_type}): the refusal must name the change type it \
             rejected, got {:?}",
            refused.body
        );

        // Refused means nothing happened: no change set is stored under the request, and
        // no entity was minted for the change it carried.
        let listed = list_change_sets(&router).await;
        assert!(
            listed.list("/ChangeSetSummaryList").is_empty(),
            "case {case:?} ({change_type}): a refused StartChangeSet starts nothing, got {:?}",
            listed.body
        );
        let entities = list_entities(&router, "AmiProduct").await;
        assert!(
            entities.list("/EntitySummaryList").is_empty(),
            "case {case:?} ({change_type}): a refused StartChangeSet mints no entity, \
             got {:?}",
            entities.body
        );
    }
}

/// One change the catalog cannot make refuses the WHOLE change set, wherever in the
/// `ChangeSet` it sits and however many changes it would otherwise have carried.
#[tokio::test]
async fn one_change_the_catalog_cannot_make_refuses_the_whole_change_set() {
    let rows = [
        (
            "the unsupported change first",
            json!([
                change("Nonsense", "AmiProduct", "first"),
                change("CreateProduct", "AmiProduct", "second"),
            ]),
        ),
        (
            "the unsupported change last",
            json!([
                change("CreateProduct", "AmiProduct", "first"),
                change("UpdateEverything", "AmiProduct", "second"),
            ]),
        ),
        (
            "the unsupported change between two the catalog performs",
            json!([
                change("CreateProduct", "AmiProduct", "first"),
                change("Reticulate", "SaaSProduct", "second"),
                change("UpdateInformation", "AmiProduct", "third"),
            ]),
        ),
    ];

    for (case, changes) in rows {
        let router = emulator();
        let refused = start_change_set(&router, request(changes)).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {case:?}: expected 422 ValidationException, got {} {:?} with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );

        let listed = list_change_sets(&router).await;
        assert!(
            listed.list("/ChangeSetSummaryList").is_empty(),
            "case {case:?}: one change the catalog cannot make starts no change set, got {:?}",
            listed.body
        );
        for entity_type in ["AmiProduct", "SaaSProduct"] {
            let entities = list_entities(&router, entity_type).await;
            assert!(
                entities.list("/EntitySummaryList").is_empty(),
                "case {case:?}: a refused change set mints no {entity_type}, got {:?}",
                entities.body
            );
        }
    }
}

/// A change type this catalog DOES perform is accepted, runs to `SUCCEEDED`, and is
/// reported back from what was stored — with the entity its change describes.
///
/// The rows differ along what the rule generalises over: the verb of the change type, the
/// kind of entity it acts under, and whether the change type names that kind of entity at
/// all. None of them is a change type the issue that prompted this suite cited.
#[tokio::test]
async fn a_change_type_this_catalog_performs_is_accepted_and_applied() {
    let rows = [
        ("adding to a data product", "AddRevisions", "DataProduct"),
        (
            "restricting an AMI product's instance types",
            "RestrictInstanceTypes",
            "AmiProduct",
        ),
        ("updating an offer's legal terms", "UpdateLegalTerms", "Offer"),
        (
            "creating a resale authorization",
            "CreateResaleAuthorization",
            "ResaleAuthorization",
        ),
        (
            "updating a container product's delivery options",
            "UpdateDeliveryOptions",
            "ContainerProduct",
        ),
        (
            "a change type at a schema version of its entity type",
            "UpdateTargeting",
            "SaaSProduct@1.0",
        ),
    ];

    // One emulator for every row: each change names no entity, so each mints its own and
    // no change set holds another's entity locked.
    let router = emulator();
    let mut started = Vec::with_capacity(rows.len());
    for (case, change_type, entity_type) in rows {
        let answer = start_change_set(
            &router,
            request(json!([change(change_type, entity_type, case)])),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a change type this catalog performs must be accepted, got {} \
             with {:?}",
            answer.status, answer.body
        );
        let id = answer.text("/ChangeSetId");
        assert!(
            !id.is_empty(),
            "case {case:?}: the accepted change set must report its id, got {:?}",
            answer.body
        );
        started.push((case, change_type, entity_type, id));
    }

    wait_for_the_end_of_the_run();

    for (case, change_type, entity_type, id) in &started {
        let reported = describe_change_set(&router, id).await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: DescribeChangeSet must report the started change set, got {} \
             with {:?}",
            reported.status, reported.body
        );
        assert_eq!(
            reported.text("/Status"),
            "SUCCEEDED",
            "case {case:?}: the change set must have succeeded, got {:?}",
            reported.body
        );
        assert_eq!(
            reported.body.pointer("/FailureCode"),
            None,
            "case {case:?}: a succeeded change set reports no FailureCode, got {:?}",
            reported.body
        );
        let changes = reported.list("/ChangeSet");
        assert_eq!(
            changes.len(),
            1,
            "case {case:?}: the change set must report the one change it carried, got {:?}",
            reported.body
        );
        assert_eq!(
            changes[0]["ChangeType"],
            json!(change_type),
            "case {case:?}: the reported change type must be the submitted one, got {:?}",
            changes[0]
        );
        assert_eq!(
            changes[0].pointer("/ErrorDetailList"),
            None,
            "case {case:?}: a change the catalog applied carries no ErrorDetailList, got {:?}",
            changes[0]
        );

        // The change it carried is what made an entity exist: read it back through
        // DescribeEntity, under the type the change acted under.
        let entity_ids: Vec<String> = list_change_sets(&router)
            .await
            .list("/ChangeSetSummaryList")
            .into_iter()
            .find(|item| item["ChangeSetId"] == json!(id))
            .map(|item| {
                item["EntityIdList"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            entity_ids.len(),
            1,
            "case {case:?}: the change set must report the one entity it acted on, got \
             {entity_ids:?}"
        );
        let entity = describe_entity(&router, &entity_ids[0]).await;
        assert_eq!(
            entity.status, 200,
            "case {case:?}: the applied change's entity must exist, got {} with {:?}",
            entity.status, entity.body
        );
        // `Entity$Type` addresses the type at an optional schema version; the entity is of
        // the type that reference names.
        let named_type = entity_type.split('@').next().unwrap_or(entity_type);
        assert_eq!(
            entity.text("/EntityType"),
            named_type,
            "case {case:?}: the entity must be of the type the change acted under, got {:?}",
            entity.body
        );
        assert_eq!(
            entity.body.pointer("/DetailsDocument"),
            Some(&json!({"ProductTitle": case})),
            "case {case:?}: the entity must hold the details the change carried, got {:?}",
            entity.body
        );
    }
}

/// Every change type this catalog publishes is accepted: the set is what gates a change
/// set, so no entry of it is refused by accident.
#[tokio::test]
async fn every_change_type_this_catalog_publishes_is_accepted() {
    let router = emulator();
    assert!(
        CHANGE_TYPES.len() > 1,
        "the catalog must publish more than one change type, got {CHANGE_TYPES:?}"
    );
    for change_type in CHANGE_TYPES {
        let answer = start_change_set(
            &router,
            request(json!([change(change_type, "AmiProduct", change_type)])),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "change type {change_type:?}: a change type the catalog publishes must be \
             accepted, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = describe_change_set(&router, &answer.text("/ChangeSetId")).await;
        assert_eq!(
            reported.list("/ChangeSet").first().map(|c| &c["ChangeType"]),
            Some(&json!(change_type)),
            "change type {change_type:?}: the stored change set must carry it, got {:?}",
            reported.body
        );
    }
}

/// `RequestedChangeList` is `@length(min: 1, max: 20)`, and that count is what sizes the
/// work one `StartChangeSet` does: a change set AT the bound is started, and one past it
/// is refused with the modelled `ValidationException`, having started nothing.
///
/// The refusal past the bound comes from the centralized boundary walk, which judges the
/// modelled constraint before the handler runs — `skip_validation` on
/// `StartChangeSetInput` names only `Catalog`, `ChangeSetName` and `ClientRequestToken`,
/// so the list length never reaches the handler's own re-check of it.
#[tokio::test]
async fn a_change_set_past_the_modelled_change_count_is_refused() {
    let changes = |count: usize| {
        json!((0..count)
            .map(|index| change("CreateProduct", "AmiProduct", &format!("change-{index}")))
            .collect::<Vec<Value>>())
    };

    // The bound itself: the largest change set the model admits is accepted.
    let router = emulator();
    let accepted =
        start_change_set(&router, request(changes(MAX_CHANGES_PER_CHANGE_SET))).await;
    assert_eq!(
        accepted.status, 200,
        "a change set of the modelled maximum {MAX_CHANGES_PER_CHANGE_SET} changes must be \
         accepted, got {} with {:?}",
        accepted.status, accepted.body
    );

    for (case, count) in [
        ("one past the bound", MAX_CHANGES_PER_CHANGE_SET + 1),
        ("far past the bound", MAX_CHANGES_PER_CHANGE_SET * 5),
    ] {
        let router = emulator();
        let refused = start_change_set(&router, request(changes(count))).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "ValidationException"),
            "case {case:?} ({count} changes): expected 400 ValidationException, got {} {:?} \
             with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        let message = refused.text("/message");
        assert!(
            message.contains("changeSet")
                && message.contains(&MAX_CHANGES_PER_CHANGE_SET.to_string()),
            "case {case:?} ({count} changes): the refusal must name the member and the \
             bound, got {message:?}"
        );
        let listed = list_change_sets(&router).await;
        assert!(
            listed.list("/ChangeSetSummaryList").is_empty(),
            "case {case:?} ({count} changes): a refused StartChangeSet starts nothing, got {:?}",
            listed.body
        );
    }
}
