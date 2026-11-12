// SPDX-License-Identifier: CC0-1.0

//! Which refusal `StartChangeSet` answers for an entity id a change set still RUNNING
//! holds, end to end over HTTP.
//!
//! RULE: a running change set locks every entity id it targets — "when a `ChangeSet` is
//! running, all the entities targeted by the different changes are locked until the change
//! set has completed (either succeeded, cancelled, or failed). If you try to start a change
//! set containing a change against an entity that is already locked, you will receive a
//! `ResourceInUseException` error" — and the id of an entity it is still CREATING is one of
//! those ids: the change set reports it in `ChangeSetSummaryListItem$EntityIdList` from the
//! moment it starts and writes the entity into the catalog only when it succeeds. So the
//! lock is what decides the refusal, not whether the catalog already holds a record: a held
//! id is refused 423 `ResourceInUseException` naming the entity and the holder, and an id is
//! reported absent (404 `ResourceNotFoundException`) only when no running change set holds
//! it.
//!
//! The rows differ along what the rule generalises over: how the holder came to hold the id
//! (it is creating the entity, or it is changing one the catalog already holds), how many
//! ids the one holder reserves and which of them is named, the entity type, the change type
//! attempted, and the form of the identifier that names the id (a bare entity id or an id at
//! a revision). The complementary tests pin the other side of the same rule: an id no
//! running change set holds is absent — including one whose holder was CANCELLED, which the
//! catalog therefore never wrote — and a reserved id becomes a startable entity once its
//! holder has succeeded.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.

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

/// What the emulator answered: the status, the JSON body and the headers a restJson1 client
/// reads a media type and an error code from.
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

/// One change: a change type, the entity type it acts under, and the identifier it names
/// (none for a change that creates an entity of its own).
fn change(change_type: &str, entity_type: &str, identifier: Option<&str>) -> Value {
    let mut entity = json!({"Type": format!("{entity_type}@1.0")});
    if let Some(identifier) = identifier {
        entity["Identifier"] = json!(identifier);
    }
    json!({
        "ChangeType": change_type,
        "Entity": entity,
        "DetailsDocument": {"ProductTitle": "entity-lock-in-flight"},
    })
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

async fn cancel_change_set(router: &MultiServiceRouter, change_set_id: &str) -> Answer {
    call(
        router,
        "PATCH",
        &format!("/CancelChangeSet?catalog={CATALOG}&changeSetId={change_set_id}"),
        None,
    )
    .await
}

/// The `ChangeSetSummaryListItem` `ListChangeSets` reports for `change_set_id`.
async fn summary(router: &MultiServiceRouter, change_set_id: &str) -> Value {
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
        .unwrap_or_else(|| {
            panic!(
                "the change set {change_set_id} must be listed: {:?}",
                listed.body
            )
        })
}

/// How many change sets the store holds — so a case can assert that a refused
/// `StartChangeSet` started nothing.
async fn change_set_count(router: &MultiServiceRouter) -> usize {
    call(
        router,
        "POST",
        "/ListChangeSets",
        Some(json!({"Catalog": CATALOG})),
    )
    .await
    .body
    .pointer("/ChangeSetSummaryList")
    .and_then(Value::as_array)
    .map(Vec::len)
    .unwrap_or_default()
}

/// Start `changes` and answer the started change set's id, failing the case if the start was
/// refused.
async fn started_holder(router: &MultiServiceRouter, case: &str, changes: Value) -> String {
    let started = start(router, changes).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: the holding change set must be started, got {} with {:?}",
        started.status, started.body
    );
    started.text("/ChangeSetId")
}

/// The entity ids a change set holds, as `ChangeSetSummaryListItem$EntityIdList` reports
/// them — the documented place a caller reads the id of an entity still being created.
async fn held_entity_ids(
    router: &MultiServiceRouter,
    case: &str,
    change_set_id: &str,
) -> Vec<String> {
    let item = summary(router, change_set_id).await;
    let ids: Vec<String> = item["EntityIdList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !ids.is_empty(),
        "case {case:?}: the running change set {change_set_id} must report the entity ids it \
         holds, got {item:?}"
    );
    ids
}

/// Create one entity of `entity_type` through a change set, wait for the clock to apply it,
/// and answer the entity id it made — read back out of what the store reports.
async fn created_entity(router: &MultiServiceRouter, entity_type: &str) -> String {
    let id = started_holder(
        router,
        "fixture",
        json!([change("CreateProduct", entity_type, None)]),
    )
    .await;
    wait_for_the_end_of_the_run();
    let entity_id = held_entity_ids(router, "fixture", &id).await[0].clone();
    let described = describe_entity(router, &entity_id).await;
    assert_eq!(
        described.status, 200,
        "the fixture's entity {entity_id} must exist once its change set succeeded, got {} \
         with {:?}",
        described.status, described.body
    );
    entity_id
}

/// How the row's holder change set came to hold the entity id the attempt then names.
#[derive(Clone, Copy, Debug)]
enum Holder {
    /// The holder is CREATING entities: it names no identifier, so the ids it holds are
    /// reserved and no entity record exists for them until it succeeds. `changes` is how
    /// many entities it creates and `index` which of the ids it holds the attempt names.
    Creating { changes: usize, index: usize },
    /// The holder is changing an entity the catalog already holds, so the id it holds names
    /// a record for the whole of the lock.
    Changing,
}

/// How the attempt spells the identifier of the id the holder holds.
#[derive(Clone, Copy, Debug)]
enum Named {
    /// The bare entity id.
    Id,
    /// The entity id at a revision (`<id>@<revision>`), the second published form of
    /// `Entity$Identifier` — it names the same entity.
    AtRevision(u32),
}

impl Named {
    fn spell(self, entity_id: &str) -> String {
        match self {
            Named::Id => entity_id.to_string(),
            Named::AtRevision(revision) => format!("{entity_id}@{revision}"),
        }
    }
}

/// One row of the table the in-use rule is asserted over.
#[derive(Clone, Copy, Debug)]
struct Case {
    /// What the row is, named in every assertion.
    name: &'static str,
    /// The entity type the holder and the attempt both act under. Source:
    /// `smithy_query(query="shape", shape="Entity")` leaves `Entity$Type` a string and
    /// points at the per-product reference pages for the values; these are types this
    /// catalog serves, including ones the issue never cited.
    entity_type: &'static str,
    /// How the holder holds the id.
    holder: Holder,
    /// The change type the holder carries against an existing entity ([`Holder::Changing`]).
    held_by: &'static str,
    /// The change type the refused attempt carries — the lock is on the ENTITY, so it is
    /// refused whatever change is asked for.
    attempted: &'static str,
    /// How the attempt spells the identifier.
    named: Named,
}

/// RULE: an entity id a running change set holds is refused 423 `ResourceInUseException`
/// naming the entity and the holder, whether the holder is still CREATING that entity (the
/// id is reserved and the catalog holds no record of it yet) or is changing one the catalog
/// already holds.
#[tokio::test]
async fn a_change_against_an_entity_a_running_change_set_holds_is_refused_as_in_use() {
    let rows = [
        Case {
            name: "the id of an entity the holder is creating",
            entity_type: "AmiProduct",
            holder: Holder::Creating {
                changes: 1,
                index: 0,
            },
            held_by: "CreateProduct",
            attempted: "UpdateInformation",
            named: Named::Id,
        },
        Case {
            name: "the second of two ids the holder is creating",
            entity_type: "ContainerProduct",
            holder: Holder::Creating {
                changes: 2,
                index: 1,
            },
            held_by: "CreateProduct",
            attempted: "AddDeliveryOptions",
            named: Named::Id,
        },
        Case {
            name: "the first of two ids the holder is creating",
            entity_type: "SaaSProduct",
            holder: Holder::Creating {
                changes: 2,
                index: 0,
            },
            held_by: "CreateProduct",
            attempted: "ReleaseProduct",
            named: Named::Id,
        },
        Case {
            name: "a reserved id named at a revision",
            entity_type: "DataProduct",
            holder: Holder::Creating {
                changes: 1,
                index: 0,
            },
            held_by: "CreateProduct",
            attempted: "AddRevisions",
            named: Named::AtRevision(2),
        },
        Case {
            name: "a reserved id named under the same change type the holder carries",
            entity_type: "MachineLearningProduct",
            holder: Holder::Creating {
                changes: 1,
                index: 0,
            },
            held_by: "CreateProduct",
            attempted: "CreateProduct",
            named: Named::Id,
        },
        Case {
            name: "an existing entity the holder is changing",
            entity_type: "SaaSProduct",
            holder: Holder::Changing,
            held_by: "UpdateInformation",
            attempted: "RestrictDeliveryOptions",
            named: Named::Id,
        },
        Case {
            name: "an existing entity the holder is changing, named at a revision",
            entity_type: "AmiProduct",
            holder: Holder::Changing,
            held_by: "AddRevisions",
            attempted: "AddRevisions",
            named: Named::AtRevision(1),
        },
    ];

    for case in rows {
        // Each row runs on its own emulator, so the ids it reads belong to its own holder
        // and the attempt lands inside that holder's run however many rows ran before it.
        let router = emulator();
        let name = case.name;

        let (holder_id, entity_id) = match case.holder {
            Holder::Creating { changes, index } => {
                let submitted: Vec<Value> = (0..changes)
                    .map(|_| change(case.held_by, case.entity_type, None))
                    .collect();
                let holder_id = started_holder(&router, name, json!(submitted)).await;
                let held = held_entity_ids(&router, name, &holder_id).await;
                assert_eq!(
                    held.len(),
                    changes,
                    "case {name:?}: a holder creating {changes} entities must report that many \
                     reserved ids, got {held:?}"
                );
                // The id is reserved, not written: the catalog holds no entity for it while
                // its change set runs, which is the window this row measures.
                let described = describe_entity(&router, &held[index]).await;
                assert_eq!(
                    described.status, 404,
                    "case {name:?}: the entity {:?} the holder is still creating must not be \
                     readable yet, got {} with {:?}",
                    held[index], described.status, described.body
                );
                (holder_id, held[index].clone())
            }
            Holder::Changing => {
                let entity_id = created_entity(&router, case.entity_type).await;
                let holder_id = started_holder(
                    &router,
                    name,
                    json!([change(case.held_by, case.entity_type, Some(&entity_id))]),
                )
                .await;
                (holder_id, entity_id)
            }
        };

        let before = change_set_count(&router).await;
        let identifier = case.named.spell(&entity_id);
        let blocked = start(
            &router,
            json!([change(case.attempted, case.entity_type, Some(&identifier))]),
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
            "case {name:?}: a change against the held entity {entity_id} must be refused 423 \
             ResourceInUseException as application/json, got {} {:?} with {:?}",
            blocked.status,
            blocked.error_code(),
            blocked.body
        );
        let message = blocked.message();
        assert!(
            message.contains(&entity_id) && message.contains(&holder_id),
            "case {name:?}: the refusal must name the locked entity {entity_id:?} and the \
             change set {holder_id:?} holding it, got {message:?}"
        );
        assert_eq!(
            change_set_count(&router).await,
            before,
            "case {name:?}: the refused start must have started nothing, and the store held \
             {before} change sets before it"
        );
    }
}

/// RULE, the other side of it: an id no running change set holds names no entity, so it is
/// reported absent — 404 `ResourceNotFoundException` stating that entity id — including the
/// id a CANCELLED change set once reserved, which the catalog therefore never wrote.
#[tokio::test]
async fn an_entity_id_no_running_change_set_holds_is_reported_absent() {
    let router = emulator();

    // An id a change set reserved and then released by being cancelled: the lock is gone and
    // nothing was ever written under it.
    let cancelled_holder = started_holder(
        &router,
        "a cancelled holder",
        json!([change("CreateProduct", "AmiProduct", None)]),
    )
    .await;
    let released = held_entity_ids(&router, "a cancelled holder", &cancelled_holder).await[0]
        .clone();
    let cancelled = cancel_change_set(&router, &cancelled_holder).await;
    assert_eq!(
        cancelled.status, 200,
        "the fixture's cancel must be served inside the open window, got {} with {:?}",
        cancelled.status, cancelled.body
    );

    // (case, the identifier the change names, the entity id the refusal must state). The
    // rows span the forms of an id that names nothing: one in the minted shape, one that is
    // not, one at a revision, and the released id above.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an id in the minted form that was never minted",
            "ent-ffffffffffff".to_string(),
            "ent-ffffffffffff".to_string(),
        ),
        (
            "an id of another shape entirely",
            "no-such-entity".to_string(),
            "no-such-entity".to_string(),
        ),
        (
            "an absent id named at a revision",
            "ent-ffffffffffff@3".to_string(),
            "ent-ffffffffffff".to_string(),
        ),
        (
            "the id a cancelled change set once reserved",
            released.clone(),
            released.clone(),
        ),
    ];

    for (case, identifier, entity_id) in rows {
        let refused = start(
            &router,
            json!([change("UpdateInformation", "AmiProduct", Some(&identifier))]),
        )
        .await;
        assert_eq!(
            (
                refused.status,
                refused.error_code(),
                refused.content_type.as_str(),
                refused.errortype.as_str(),
            ),
            (
                404,
                "ResourceNotFoundException",
                JSON,
                "ResourceNotFoundException"
            ),
            "case {case:?}: an identifier no running change set holds and no entity answers \
             must be refused 404 ResourceNotFoundException, got {} {:?} with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        // Spelled out here rather than taken from the crate, so a wrong implementation
        // cannot make this pass: the sentence states the entity id without the revision an
        // identifier may address it at.
        assert_eq!(
            refused.message(),
            format!("Requested entity '{entity_id}' does not exist."),
            "case {case:?}: the refusal must state the entity that was asked for, got {:?}",
            refused.message()
        );
    }
}

/// RULE, the end of the lock: once the holder has SUCCEEDED, the id it reserved is an entity
/// of the catalog and nothing holds it, so the same change that was refused as in use is
/// then started — and the entity reads back under that id.
#[tokio::test]
async fn a_reserved_entity_id_is_startable_once_its_holder_has_succeeded() {
    let router = emulator();

    for entity_type in ["AmiProduct", "SaaSProduct"] {
        let holder_id = started_holder(
            &router,
            entity_type,
            json!([change("CreateProduct", entity_type, None)]),
        )
        .await;
        let entity_id = held_entity_ids(&router, entity_type, &holder_id).await[0].clone();

        let blocked = start(
            &router,
            json!([change("UpdateInformation", entity_type, Some(&entity_id))]),
        )
        .await;
        assert_eq!(
            (blocked.status, blocked.error_code()),
            (423, "ResourceInUseException"),
            "case {entity_type:?}: while the holder runs, the reserved entity {entity_id} is \
             locked, got {} {:?}",
            blocked.status,
            blocked.error_code()
        );

        wait_for_the_end_of_the_run();

        // The round trip: the id the holder reserved is now an entity of the catalog.
        let described = describe_entity(&router, &entity_id).await;
        let identifier = described.text("/EntityIdentifier");
        assert_eq!(
            (
                described.status,
                described.text("/EntityType"),
                // `DescribeEntityResponse$EntityIdentifier` is an `Identifier`, so it
                // addresses the entity at its revision — the id it names is the reserved
                // one.
                identifier.split('@').next().unwrap_or_default().to_string(),
            ),
            (200, entity_type.to_string(), entity_id.clone()),
            "case {entity_type:?}: the succeeded holder must have written the entity \
             {entity_id}, got {} with {:?}",
            described.status,
            described.body
        );

        let allowed = start(
            &router,
            json!([change("UpdateInformation", entity_type, Some(&entity_id))]),
        )
        .await;
        assert_eq!(
            allowed.status, 200,
            "case {entity_type:?}: a completed holder releases the entity it held, got {} \
             with {:?}",
            allowed.status, allowed.body
        );
        wait_for_the_end_of_the_run();
    }
}
