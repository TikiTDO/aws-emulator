// SPDX-License-Identifier: CC0-1.0

//! Which refusal `StartChangeSet` answers when one entity is targeted twice, end to end
//! over HTTP.
//!
//! RULE: `StartChangeSet` states two separate rules about one entity being targeted twice,
//! and they carry different refusals.
//!
//! * Within ONE change set, "you can't start the same change type against the same entity
//!   multiple times": the submitted list contradicts itself, which is a property of the
//!   REQUEST, so it is refused 422 `ValidationException` naming the repeated change type
//!   and the entity id — before the entity is resolved, and whatever the store holds.
//! * ACROSS change sets, "when a `ChangeSet` is running, all the entities targeted by the
//!   different changes are locked until the change set has completed": a change against an
//!   entity another change set holds is refused 423 `ResourceInUseException`, for ANY
//!   change type, and the entity is startable again once the holder has completed.
//!
//! So the status a repeated target gets is decided by WHO ELSE holds the entity, not by the
//! change type being repeated: nothing holds it in the first case, a running change set
//! does in the second.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rows differ along what the rules generalise over: the change type, how
//! many copies of it the change set carries and where they sit, the form of the identifier
//! each copy names (a bare entity id, an id at a revision, two different revisions of one
//! entity, the reference by which one change names the entity another change creates),
//! whether the entity type reference carries a schema version, and whether the entity the
//! changes name exists at all.

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

/// The restJson1 document media type: `application/json`, for answers and refusals alike.
const JSON: &str = "application/json";

/// The change types the rows repeat.
///
/// Source: `Change$ChangeType` is a free string ("Change types are single string values
/// that describe your intention for the change. Each change type is unique for each
/// EntityType provided in the change's scope", `smithy_query(query="shape",
/// shape="Change")`, `@pattern ^[A-Z][\w]*$`), so the rule can key on no fixed list; these
/// are the ones AWS documents for products, including entries the issue never cited.
const CHANGE_TYPES: &[&str] = &[
    "AddRevisions",
    "UpdateInformation",
    "AddDeliveryOptions",
    "ReleaseProduct",
    "RestrictDeliveryOptions",
];

/// How long after the start a change set has certainly reached `SUCCEEDED`: the two spans
/// the implementation publishes, plus a margin for a slow machine.
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

/// What the emulator answered: the status, the JSON body and the header a restJson1 client
/// reads an error code from.
struct Answer {
    status: u16,
    body: Value,
    content_type: String,
    errortype: String,
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

    /// The `Message` of a refusal, under either casing a protocol spells it.
    fn message(&self) -> String {
        let lower = self.text("/message");
        if lower.is_empty() {
            self.text("/Message")
        } else {
            lower
        }
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
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let content_type = header("content-type");
    let errortype = header("x-amzn-errortype");
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
        body,
        content_type,
        errortype,
    }
}

/// `POST /StartChangeSet` with `changes` as the whole `ChangeSet`.
async fn start(router: &MultiServiceRouter, changes: Value) -> Answer {
    call(
        router,
        "POST",
        "/StartChangeSet",
        Some(json!({"Catalog": CATALOG, "ChangeSet": changes})),
    )
    .await
}

/// One change: a change type, the entity type reference it acts under, and the identifier
/// it names (none for a change that creates an entity of its own).
fn change(change_type: &str, entity_type: &str, identifier: Option<&str>) -> Value {
    // `Change$Entity$Type` carries an entity type at a schema version, which is the form
    // the service publishes for the member; a row that names its own version keeps it.
    let reference = if entity_type.contains('@') {
        entity_type.to_string()
    } else {
        format!("{entity_type}@1.0")
    };
    let mut entity = json!({"Type": reference});
    if let Some(identifier) = identifier {
        entity["Identifier"] = json!(identifier);
    }
    json!({
        "ChangeType": change_type,
        "Entity": entity,
        "DetailsDocument": {"ProductTitle": "pair-uniqueness"},
    })
}

/// The change sets the store holds, by id — so a case can assert that a refused
/// `StartChangeSet` started nothing.
async fn change_set_ids(router: &MultiServiceRouter) -> Vec<String> {
    let listed = call(
        router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG})),
    )
    .await;
    listed
        .body
        .pointer("/ChangeSetSummaryList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["ChangeSetId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Create one entity of `entity_type` through a change set, wait for the clock to apply it,
/// and answer the entity id it made — read back out of what the store reports.
async fn created_entity(router: &MultiServiceRouter, entity_type: &str) -> String {
    let started = start(router, json!([change("CreateProduct", entity_type, None)])).await;
    assert_eq!(
        started.status, 200,
        "the fixture's create must answer 200, got {} with {:?}",
        started.status, started.body
    );
    let change_set_id = started.text("/ChangeSetId");
    wait_for_success();

    let listed = call(
        router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG})),
    )
    .await;
    listed
        .body
        .pointer("/ChangeSetSummaryList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|item| item["ChangeSetId"] == json!(change_set_id))
        .and_then(|item| item["EntityIdList"][0].as_str().map(str::to_string))
        .unwrap_or_else(|| {
            panic!(
                "the fixture's change set {change_set_id} must report its entity: {:?}",
                listed.body
            )
        })
}

/// The refusal a change set that contradicts itself is answered with, spelled out here
/// rather than taken from the crate so a wrong implementation cannot make this pass: the
/// repeated change type and the entity id it was repeated on, with the entity id stated
/// without the revision an identifier may address it at.
fn duplicate_pair_message(change_type: &str, entity_id: &str) -> String {
    format!(
        "[Requested changes contain multiple changes for the same change type \
         '{change_type}' on entity '{entity_id}'. Entity - change type pairs \
         should be unique.]"
    )
}

/// How one row of [`a_change_set_that_repeats_no_pair_is_started`] builds its `ChangeSet`
/// out of the two entity ids its fixture created.
type BuildChangeSet = fn(&str, &str) -> Value;

/// RULE, within one change set: the same change type against the same entity more than
/// once is the request contradicting itself — 422 `ValidationException` naming the repeated
/// change type and the entity id — and nothing is started.
///
/// The rows cross every documented change type with the layouts the rule generalises over:
/// two adjacent copies, copies separated by a change of another type, three copies, and the
/// identifier forms that all name ONE entity (a bare id, an id at a revision, two different
/// revisions), under a plain and a versioned entity type reference.
#[tokio::test]
async fn one_change_set_refuses_the_same_change_type_twice_against_one_entity() {
    let router = emulator();
    let entity_id = created_entity(&router, "AmiProduct").await;
    let at_revision_1 = format!("{entity_id}@1");
    let at_revision_2 = format!("{entity_id}@2");

    for change_type in CHANGE_TYPES {
        // (case, the whole ChangeSet)
        let rows: Vec<(String, Value)> = vec![
            (
                "two adjacent copies".to_string(),
                json!([
                    change(change_type, "AmiProduct", Some(&entity_id)),
                    change(change_type, "AmiProduct", Some(&entity_id)),
                ]),
            ),
            (
                "copies separated by a change of another type".to_string(),
                json!([
                    change(change_type, "AmiProduct", Some(&entity_id)),
                    change("UpdateDeliveryOptions", "AmiProduct", Some(&entity_id)),
                    change(change_type, "AmiProduct", Some(&entity_id)),
                ]),
            ),
            (
                "three copies".to_string(),
                json!([
                    change(change_type, "AmiProduct", Some(&entity_id)),
                    change(change_type, "AmiProduct", Some(&entity_id)),
                    change(change_type, "AmiProduct", Some(&entity_id)),
                ]),
            ),
            (
                "the second copy addressing the entity at a revision".to_string(),
                json!([
                    change(change_type, "AmiProduct", Some(&entity_id)),
                    change(change_type, "AmiProduct", Some(&at_revision_1)),
                ]),
            ),
            (
                "two different revisions of one entity".to_string(),
                json!([
                    change(change_type, "AmiProduct", Some(&at_revision_1)),
                    change(change_type, "AmiProduct", Some(&at_revision_2)),
                ]),
            ),
            (
                "a versioned entity type reference".to_string(),
                json!([
                    change(change_type, "AmiProduct@2", Some(&entity_id)),
                    change(change_type, "AmiProduct", Some(&at_revision_1)),
                ]),
            ),
        ];

        for (layout, changes) in rows {
            let case = format!("{change_type} / {layout}");
            let refused = start(&router, changes).await;
            assert_eq!(
                (
                    refused.status,
                    refused.error_code(),
                    refused.content_type.as_str(),
                    refused.errortype.as_str(),
                ),
                (422, "ValidationException", JSON, "ValidationException"),
                "case {case:?}: one change set repeating a change type against one entity \
                 must be refused 422 ValidationException as application/json, got {} {:?} \
                 with {:?}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                refused.message(),
                duplicate_pair_message(change_type, &entity_id),
                "case {case:?}: the refusal must name the repeated change type and the \
                 entity id, got {:?}",
                refused.message()
            );
            assert_eq!(
                change_set_ids(&router).await.len(),
                1,
                "case {case:?}: a refused StartChangeSet must start nothing, so only the \
                 fixture's own change set may be held, got {:?}",
                change_set_ids(&router).await
            );
        }
    }
}

/// The same rule where the entity does NOT exist: it is a rule about the request, so it is
/// reached before the entity is resolved — the contradiction is refused 422 rather than
/// reported as a missing entity (404) or as a resource in use (423).
///
/// The rows differ along the form of the identifier the copies name: an entity id no
/// change set ever created, that id at a revision, and the reference by which one change
/// names the entity another change of the same change set creates.
#[tokio::test]
async fn the_uniqueness_rule_is_reached_before_the_entity_is_resolved() {
    // (case, the identifier both copies name, the entity the refusal names)
    let rows: [(&str, &str, &str); 3] = [
        ("an id nothing created", "prod-abc123", "prod-abc123"),
        (
            "that id at a revision",
            "prod-abc123@7",
            // The refusal names the entity, not the revision it was addressed at.
            "prod-abc123",
        ),
        (
            "the reference to another change's new entity",
            "$Create.Entity.Identifier",
            "$Create.Entity.Identifier",
        ),
    ];

    for (case, identifier, named) in rows {
        let router = emulator();
        let refused = start(
            &router,
            json!([
                change("AddRevisions", "SaaSProduct", Some(identifier)),
                change("AddRevisions", "SaaSProduct", Some(identifier)),
            ]),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (422, "ValidationException"),
            "case {case:?}: the contradiction must be refused before the entity lookup, \
             got {} {:?} with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert_eq!(
            refused.message(),
            duplicate_pair_message("AddRevisions", named),
            "case {case:?}: the refusal must name the repeated change type and the entity, \
             got {:?}",
            refused.message()
        );
        assert_eq!(
            change_set_ids(&router).await,
            Vec::<String>::new(),
            "case {case:?}: a refused StartChangeSet must start nothing"
        );
    }
}

/// The other side of the uniqueness rule: it refuses a repeated (entity, change type) PAIR
/// and nothing wider, so a change set naming one entity under different change types, the
/// same change type against different entities, or several changes that each create an
/// entity of their own, is started.
#[tokio::test]
async fn a_change_set_that_repeats_no_pair_is_started() {
    // (case, how the ChangeSet is built from two entity ids)
    let rows: [(&str, BuildChangeSet); 4] = [
        (
            "two change types against one entity",
            |first, _second| {
                json!([
                    change("AddRevisions", "AmiProduct", Some(first)),
                    change("UpdateInformation", "AmiProduct", Some(first)),
                ])
            },
        ),
        (
            "three change types against one entity",
            |first, _second| {
                json!([
                    change("AddRevisions", "AmiProduct", Some(first)),
                    change("UpdateInformation", "AmiProduct", Some(first)),
                    change("AddDeliveryOptions", "AmiProduct", Some(first)),
                ])
            },
        ),
        (
            "one change type against two entities",
            |first, second| {
                json!([
                    change("AddRevisions", "AmiProduct", Some(first)),
                    change("AddRevisions", "AmiProduct", Some(second)),
                ])
            },
        ),
        (
            "two creates, each making an entity of its own",
            |_first, _second| {
                json!([
                    change("CreateProduct", "AmiProduct", None),
                    change("CreateProduct", "AmiProduct", None),
                ])
            },
        ),
    ];

    for (case, build) in rows {
        let router = emulator();
        let first = created_entity(&router, "AmiProduct").await;
        let second = created_entity(&router, "AmiProduct").await;
        assert_ne!(
            first, second,
            "case {case:?}: the fixture must create two distinct entities, got {first:?} twice"
        );

        let started = start(&router, build(&first, &second)).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: a change set that repeats no (entity, change type) pair must be \
             started, got {} with {:?}",
            started.status, started.body
        );
        let id = started.text("/ChangeSetId");
        assert!(
            change_set_ids(&router).await.contains(&id),
            "case {case:?}: the started change set {id:?} must be held by the store, got {:?}",
            change_set_ids(&router).await
        );
    }
}

/// RULE, across change sets: a running change set locks every entity it targets, so a later
/// change against one of them is refused 423 `ResourceInUseException` naming the holder —
/// for ANY change type, including one the holder does not carry — and the entity is
/// startable again once the holder has completed.
///
/// This is the refusal the lock carries, and it is what the 422 above is NOT: the same
/// entity, the same change type, one change set versus two.
#[tokio::test]
async fn an_entity_a_running_change_set_holds_is_refused_as_in_use() {
    let router = emulator();
    let entity_id = created_entity(&router, "SaaSProduct").await;

    // (the change type the holder takes, the change type attempted against the entity it
    // locked). Each attempted type is tried against a holder carrying that same type and
    // against one carrying another, because the lock is on the ENTITY and not on the
    // (entity, change type) pair the holder took.
    let mut rows: Vec<(String, &str, &str)> = Vec::new();
    for (index, attempted) in CHANGE_TYPES.iter().enumerate() {
        let other = CHANGE_TYPES[(index + 1) % CHANGE_TYPES.len()];
        rows.push((
            format!("{attempted} against a holder of the same type"),
            attempted,
            attempted,
        ));
        rows.push((
            format!("{attempted} against a holder of {other}"),
            other,
            attempted,
        ));
    }

    for (case, held_by, attempted) in rows {
        // Each row starts its own holder and makes its ONE attempt straight after, so the
        // assertion does not depend on how many calls fit inside the holder's run.
        let holder = start(
            &router,
            json!([change(held_by, "SaaSProduct", Some(&entity_id))]),
        )
        .await;
        assert_eq!(
            holder.status, 200,
            "case {case:?}: the holding change set must be started — a completed change set \
             releases the entity it held — got {} with {:?}",
            holder.status, holder.body
        );
        let holder_id = holder.text("/ChangeSetId");

        let blocked = start(
            &router,
            json!([change(attempted, "SaaSProduct", Some(&entity_id))]),
        )
        .await;
        assert_eq!(
            (
                blocked.status,
                blocked.error_code(),
                blocked.content_type.as_str(),
                blocked.errortype.as_str(),
            ),
            (423, "ResourceInUseException", JSON, "ResourceInUseException"),
            "case {case:?}: a change against an entity a running change set holds must be \
             refused 423 ResourceInUseException as application/json, got {} {:?} with {:?}",
            blocked.status,
            blocked.error_code(),
            blocked.body
        );
        let message = blocked.message();
        assert!(
            message.contains(&entity_id) && message.contains(&holder_id),
            "case {case:?}: the refusal must name the locked entity and the change set \
             holding it, got {message:?}"
        );

        // The lock lasts exactly as long as the holder runs, which is what lets the next
        // row start a holder of its own against the same entity.
        wait_for_success();
    }

    // And the entity is startable again once nothing holds it, stated once on its own.
    let released = start(
        &router,
        json!([change(CHANGE_TYPES[0], "SaaSProduct", Some(&entity_id))]),
    )
    .await;
    assert_eq!(
        released.status, 200,
        "a completed change set must release the entity it held, got {} with {:?}",
        released.status, released.body
    );
}
