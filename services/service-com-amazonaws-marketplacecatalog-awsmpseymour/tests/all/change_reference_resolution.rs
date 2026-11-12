// SPDX-License-Identifier: CC0-1.0

//! How one change of a change set names the entity ANOTHER change of the same change set
//! acts on, end to end over HTTP.
//!
//! RULE: an `Entity$Identifier` of the form `$<ChangeName>.Entity.Identifier` names the
//! entity the change carrying that `ChangeName` acts on, so both changes are applied to
//! that ONE entity — whichever of them comes first in the submitted list, and whether the
//! named change creates its entity or names an existing one. A name no change of the same
//! request carries, and a change name two of them carry, name no entity, so both are
//! refused as the request contradicting itself.
//!
//! RULE: a `ChangeName` is one to 72 letters — the characters the modelled `@pattern`
//! admits over the modelled `@length(min: 1, max: 72)`, which is also the length the
//! reference form states for the name inside it — so every change name a reference can
//! point at is a change name a caller may send.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rows differ along what the rules generalise over: the change name
//! (one letter, a realistic multi-letter name, the longest name the form admits), the
//! entity type the changes act under, the change types beside the create, where the
//! reference sits relative to the change it names, how many changes name it, how long the
//! chain of references is, and whether the named change creates an entity or names one the
//! catalog already holds.

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

/// The syntax this service publishes for a `Change$ChangeName`, written out here rather
/// than read from the crate so a wrong implementation cannot make this suite pass by
/// quoting its own pattern: the characters the modelled `@pattern` admits
/// (`smithy_query(query="shape", shape="ChangeName")`, `^[a-zA-Z]$`) over the modelled
/// `@length(min: 1, max: 72)` — the same length the reference form
/// `(^\$[a-zA-Z]{1,72}\.Entity\.Identifier$)` states for the name inside it.
const CHANGE_NAME_PATTERN: &str = "^[a-zA-Z]{1,72}$";

/// The entity types the rows act under.
///
/// Source: the types `ListEntitiesRequest$EntityType` enumerates in prose
/// (`smithy_query(query="doc", shape="ListEntitiesRequest", member="EntityType")`); these
/// include entries the issue never cited (`SaaSProduct`, `ContainerProduct`, `Offer`).
const ENTITY_TYPES: &[&str] = &["AmiProduct", "SaaSProduct", "ContainerProduct", "Offer"];

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

/// One change: its change type, the entity type it acts under, the identifier it names
/// (none for a change that creates an entity of its own), its optional `ChangeName`, and
/// the details it submits.
fn change(
    change_type: &str,
    entity_type: &str,
    identifier: Option<&str>,
    change_name: Option<&str>,
    details: Value,
) -> Value {
    let mut entity = json!({"Type": format!("{entity_type}@1.0")});
    if let Some(identifier) = identifier {
        entity["Identifier"] = json!(identifier);
    }
    let mut change = json!({
        "ChangeType": change_type,
        "Entity": entity,
        "DetailsDocument": details,
    });
    if let Some(change_name) = change_name {
        change["ChangeName"] = json!(change_name);
    }
    change
}

/// The reference form by which a change names the entity the change called `change_name`
/// acts on.
fn reference(change_name: &str) -> String {
    format!("${change_name}.Entity.Identifier")
}

/// The `EntityIdList` the change set `change_set_id` reports — the entities its changes
/// act on, read back out of what the store serves.
async fn entity_ids(router: &MultiServiceRouter, change_set_id: &str) -> Vec<String> {
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
        .and_then(|item| item["EntityIdList"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect()
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

/// `GET /DescribeEntity` for `entity_id`.
async fn describe_entity(router: &MultiServiceRouter, entity_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeEntity?catalog={CATALOG}&entityId={entity_id}"),
        None,
    )
    .await
}

/// `GET /DescribeChangeSet` for `change_set_id`.
async fn describe_change_set(router: &MultiServiceRouter, change_set_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/DescribeChangeSet?catalog={CATALOG}&changeSetId={change_set_id}"),
        None,
    )
    .await
}

/// Create one entity of `entity_type` through a change set of its own, wait for the clock
/// to apply it, and answer the id the catalog minted for it.
async fn created_entity(router: &MultiServiceRouter, entity_type: &str, title: &str) -> String {
    let started = start(
        router,
        json!([change(
            "CreateProduct",
            entity_type,
            None,
            None,
            json!({"ProductTitle": title}),
        )]),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the fixture's create must answer 200, got {} with {:?}",
        started.status, started.body
    );
    let change_set_id = started.text("/ChangeSetId");
    wait_for_success();
    let ids = entity_ids(router, &change_set_id).await;
    ids.first()
        .cloned()
        .unwrap_or_else(|| panic!("the fixture's change set must report its entity, got {ids:?}"))
}

/// The 72-letter change name the reference form's `{1,72}` admits at its bound.
fn longest_change_name() -> String {
    "a".repeat(72)
}

/// RULE: a change naming `$<ChangeName>.Entity.Identifier` acts on the entity of the
/// change carrying that name, so one change set applies both changes to ONE entity and the
/// reads report one entity carrying the details of both.
///
/// The rows cross the change name (one letter, the realistic multi-letter name the model's
/// single-letter pattern would have refused, the 72-letter name at the form's bound) with
/// the entity type, and vary the layout the rule generalises over: the reference after the
/// create, before it, two changes naming one create, and a chain of references.
#[tokio::test]
async fn a_change_resolves_the_entity_of_the_change_it_names() {
    let longest = longest_change_name();
    // (case, the change names to use, how the ChangeSet is laid out, how many changes act)
    let names: [(&str, &str); 3] = [
        ("one letter", "C"),
        ("a realistic multi-letter name", "CreateNewProduct"),
        ("the longest name the form admits", longest.as_str()),
    ];

    for (name_case, name) in names {
        for entity_type in ENTITY_TYPES {
            // (layout case, the whole ChangeSet, how many changes act on the entity)
            let layouts: Vec<(&str, Value, usize)> = vec![
                (
                    "the reference after the change it names",
                    json!([
                        change(
                            "CreateProduct",
                            entity_type,
                            None,
                            Some(name),
                            json!({"ProductTitle": "resolved"}),
                        ),
                        change(
                            "AddDeliveryOptions",
                            entity_type,
                            Some(&reference(name)),
                            None,
                            json!({"Description": "second"}),
                        ),
                    ]),
                    2,
                ),
                (
                    "the reference before the change it names",
                    json!([
                        change(
                            "AddDeliveryOptions",
                            entity_type,
                            Some(&reference(name)),
                            None,
                            json!({"Description": "second"}),
                        ),
                        change(
                            "CreateProduct",
                            entity_type,
                            None,
                            Some(name),
                            json!({"ProductTitle": "resolved"}),
                        ),
                    ]),
                    2,
                ),
                (
                    "two changes naming one create",
                    json!([
                        change(
                            "CreateProduct",
                            entity_type,
                            None,
                            Some(name),
                            json!({"ProductTitle": "resolved"}),
                        ),
                        change(
                            "AddDeliveryOptions",
                            entity_type,
                            Some(&reference(name)),
                            None,
                            json!({"Description": "second"}),
                        ),
                        change(
                            "ReleaseProduct",
                            entity_type,
                            Some(&reference(name)),
                            None,
                            json!({"Released": true}),
                        ),
                    ]),
                    3,
                ),
                (
                    "a chain of references",
                    json!([
                        change(
                            "CreateProduct",
                            entity_type,
                            None,
                            Some(name),
                            json!({"ProductTitle": "resolved"}),
                        ),
                        change(
                            "AddDeliveryOptions",
                            entity_type,
                            Some(&reference(name)),
                            Some("Added"),
                            json!({"Description": "second"}),
                        ),
                        change(
                            "ReleaseProduct",
                            entity_type,
                            Some(&reference("Added")),
                            None,
                            json!({"Released": true}),
                        ),
                    ]),
                    3,
                ),
            ];

            for (layout, changes, acting) in layouts {
                let case = format!("{name_case} / {entity_type} / {layout}");
                let router = emulator();
                let started = start(&router, changes).await;
                assert_eq!(
                    (started.status, started.error_code()),
                    (200, ""),
                    "case {case:?}: a change set whose changes name each other must be \
                     started, got {} {:?} with {:?}",
                    started.status,
                    started.error_code(),
                    started.body
                );
                let change_set_id = started.text("/ChangeSetId");

                // One entity for every change of the set: the reference resolves to the
                // entity the named change reserved, so no second entity is minted.
                let ids = entity_ids(&router, &change_set_id).await;
                assert_eq!(
                    ids.len(),
                    1,
                    "case {case:?}: the {acting} changes act on one entity, so the change \
                     set must report exactly one entity id, got {ids:?}"
                );
                let entity_id = ids[0].clone();

                wait_for_success();
                let described = describe_change_set(&router, &change_set_id).await;
                assert_eq!(
                    (described.status, described.text("/Status").as_str()),
                    (200, "SUCCEEDED"),
                    "case {case:?}: the change set must succeed, got {} {:?} with {:?}",
                    described.status,
                    described.text("/Status"),
                    described.body
                );
                let reported = described
                    .body
                    .pointer("/ChangeSet")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                assert_eq!(
                    reported.len(),
                    acting,
                    "case {case:?}: DescribeChangeSet must report every submitted change, \
                     got {reported:?}"
                );

                // The entity the changes share carries what each of them submitted.
                let entity = describe_entity(&router, &entity_id).await;
                assert_eq!(
                    entity.status, 200,
                    "case {case:?}: the entity the changes share must be readable, got {} \
                     with {:?}",
                    entity.status, entity.body
                );
                let details: Value = serde_json::from_str(&entity.text("/Details"))
                    .unwrap_or_else(|error| {
                        panic!(
                            "case {case:?}: the entity's Details must be JSON ({error}), got \
                             {:?}",
                            entity.text("/Details")
                        )
                    });
                assert_eq!(
                    details["ProductTitle"],
                    json!("resolved"),
                    "case {case:?}: the created entity must carry the details of the change \
                     that created it, got {details:?}"
                );
                assert_eq!(
                    details["Description"],
                    json!("second"),
                    "case {case:?}: the entity must carry the details of the change that \
                     named the create, got {details:?}"
                );
                if acting == 3 {
                    assert_eq!(
                        details["Released"],
                        json!(true),
                        "case {case:?}: the entity must carry the details of every change \
                         that named it, got {details:?}"
                    );
                }
                assert_eq!(
                    entity.text("/EntityType"),
                    *entity_type,
                    "case {case:?}: the entity is of the type the changes named, got {:?}",
                    entity.text("/EntityType")
                );
                // The identifier a reader gets back addresses the entity at the revision
                // the applied changes left it at, one per change.
                assert_eq!(
                    entity.text("/EntityIdentifier"),
                    format!("{entity_id}@{acting}"),
                    "case {case:?}: every change that named the entity must have been \
                     applied to it, got {:?}",
                    entity.text("/EntityIdentifier")
                );
            }
        }
    }
}

/// The same rule where the named change acts on an entity the catalog ALREADY holds: the
/// reference names the change, so it resolves to whatever entity that change acts on —
/// not only to one a create reserved.
#[tokio::test]
async fn a_reference_resolves_to_an_existing_entity_the_named_change_acts_on() {
    for entity_type in ENTITY_TYPES {
        let router = emulator();
        let held = created_entity(&router, entity_type, "already held").await;

        let started = start(
            &router,
            json!([
                change(
                    "UpdateInformation",
                    entity_type,
                    Some(&held),
                    Some("Update"),
                    json!({"Description": "updated"}),
                ),
                change(
                    "AddDeliveryOptions",
                    entity_type,
                    Some(&reference("Update")),
                    None,
                    json!({"Added": "option"}),
                ),
            ]),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {entity_type:?}: a reference to a change naming an existing entity must \
             be started, got {} with {:?}",
            started.status, started.body
        );
        let change_set_id = started.text("/ChangeSetId");
        assert_eq!(
            entity_ids(&router, &change_set_id).await,
            vec![held.clone()],
            "case {entity_type:?}: both changes act on the entity the named change names"
        );

        wait_for_success();
        let entity = describe_entity(&router, &held).await;
        let details: Value =
            serde_json::from_str(&entity.text("/Details")).unwrap_or_else(|error| {
                panic!(
                    "case {entity_type:?}: the entity's Details must be JSON ({error}), got \
                     {:?}",
                    entity.text("/Details")
                )
            });
        assert_eq!(
            (details["Description"].clone(), details["Added"].clone()),
            (json!("updated"), json!("option")),
            "case {entity_type:?}: the existing entity must carry what both changes \
             submitted, got {details:?}"
        );
    }
}

/// RULE: a change name identifies ONE change of the change set, so a reference naming a
/// change the request does not carry, a change name two changes carry, a chain of
/// references that leads back to a change it has already passed, a reference under an
/// entity type other than the named change's, and a reference that repeats a change type
/// already taken against that entity are each refused 422 `ValidationException` — and
/// nothing is started.
#[tokio::test]
async fn a_reference_that_names_no_single_change_is_refused() {
    // (case, the whole ChangeSet, a fragment the refusal must name)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a reference naming no change of the request",
            json!([
                change(
                    "CreateProduct",
                    "AmiProduct",
                    None,
                    Some("Create"),
                    json!({"ProductTitle": "t"}),
                ),
                change(
                    "AddDeliveryOptions",
                    "AmiProduct",
                    Some(&reference("Other")),
                    None,
                    json!({}),
                ),
            ]),
            reference("Other"),
        ),
        (
            "a reference naming a change of no name at all",
            json!([
                change(
                    "CreateProduct",
                    "AmiProduct",
                    None,
                    None,
                    json!({"ProductTitle": "t"}),
                ),
                change(
                    "AddDeliveryOptions",
                    "AmiProduct",
                    Some(&reference("Create")),
                    None,
                    json!({}),
                ),
            ]),
            reference("Create"),
        ),
        (
            "two changes carrying one change name",
            json!([
                change(
                    "CreateProduct",
                    "AmiProduct",
                    None,
                    Some("Create"),
                    json!({"ProductTitle": "t"}),
                ),
                change(
                    "AddDeliveryOptions",
                    "AmiProduct",
                    None,
                    Some("Create"),
                    json!({}),
                ),
            ]),
            "Create".to_string(),
        ),
        (
            "a change naming itself",
            json!([change(
                "AddDeliveryOptions",
                "AmiProduct",
                Some(&reference("Loop")),
                Some("Loop"),
                json!({}),
            )]),
            reference("Loop"),
        ),
        (
            "two changes naming each other",
            json!([
                change(
                    "AddDeliveryOptions",
                    "AmiProduct",
                    Some(&reference("Second")),
                    Some("First"),
                    json!({}),
                ),
                change(
                    "ReleaseProduct",
                    "AmiProduct",
                    Some(&reference("First")),
                    Some("Second"),
                    json!({}),
                ),
            ]),
            reference("Second"),
        ),
        (
            "a reference under another entity type",
            json!([
                change(
                    "CreateProduct",
                    "AmiProduct",
                    None,
                    Some("Create"),
                    json!({"ProductTitle": "t"}),
                ),
                change(
                    "AddDeliveryOptions",
                    "SaaSProduct",
                    Some(&reference("Create")),
                    None,
                    json!({}),
                ),
            ]),
            "SaaSProduct".to_string(),
        ),
        (
            "a reference repeating the named change's own change type",
            json!([
                change(
                    "CreateProduct",
                    "AmiProduct",
                    None,
                    Some("Create"),
                    json!({"ProductTitle": "t"}),
                ),
                change(
                    "CreateProduct",
                    "AmiProduct",
                    Some(&reference("Create")),
                    None,
                    json!({}),
                ),
            ]),
            "CreateProduct".to_string(),
        ),
    ];

    for (case, changes, named) in rows {
        let router = emulator();
        let refused = start(&router, changes).await;
        assert_eq!(
            (
                refused.status,
                refused.error_code(),
                refused.content_type.as_str(),
                refused.errortype.as_str(),
            ),
            (422, "ValidationException", JSON, "ValidationException"),
            "case {case:?}: a reference that names no single change must be refused 422 \
             ValidationException as application/json, got {} {:?} with {:?}",
            refused.status,
            refused.error_code(),
            refused.body
        );
        assert!(
            refused.message().contains(&named),
            "case {case:?}: the refusal must name {named:?}, got {:?}",
            refused.message()
        );
        assert_eq!(
            change_set_ids(&router).await,
            Vec::<String>::new(),
            "case {case:?}: a refused StartChangeSet must start nothing, got {:?}",
            change_set_ids(&router).await
        );
    }
}

/// RULE: a `ChangeName` is one to 72 letters, so every name a reference can point at is a
/// name a caller may send — and a value outside that syntax is refused 422
/// `ValidationException` quoting the value and the syntax, with nothing started.
///
/// The rows run along the length (each published bound and one past it) and along the
/// characters (a digit, a separator, whitespace), including names the issue never cited.
#[tokio::test]
async fn a_change_name_is_up_to_seventy_two_letters() {
    let longest = longest_change_name();
    let too_long = "a".repeat(73);
    // (case, the change name, whether the service accepts it)
    let rows: Vec<(&str, String, bool)> = vec![
        ("one letter, the shortest the length admits", "C".to_string(), true),
        ("two letters", "Ab".to_string(), true),
        ("a realistic multi-letter name", "CreateNewProduct".to_string(), true),
        ("mixed case letters", "aBcDeFgH".to_string(), true),
        ("72 letters, the longest the length admits", longest, true),
        ("73 letters, one past the length", too_long, false),
        ("a digit inside the name", "Create1".to_string(), false),
        ("an underscore inside the name", "Create_New".to_string(), false),
        ("a hyphen inside the name", "Create-New".to_string(), false),
        ("whitespace inside the name", "Create New".to_string(), false),
        ("a letter outside ASCII", "Créate".to_string(), false),
    ];

    for (case, name, accepted) in rows {
        let router = emulator();
        let answered = start(
            &router,
            json!([change(
                "CreateProduct",
                "AmiProduct",
                None,
                Some(&name),
                json!({"ProductTitle": "named"}),
            )]),
        )
        .await;

        if accepted {
            assert_eq!(
                (answered.status, answered.error_code()),
                (200, ""),
                "case {case:?}: a change name inside {CHANGE_NAME_PATTERN} must be \
                 accepted, got {} {:?} with {:?}",
                answered.status,
                answered.error_code(),
                answered.body
            );
            let change_set_id = answered.text("/ChangeSetId");
            wait_for_success();
            let described = describe_change_set(&router, &change_set_id).await;
            assert_eq!(
                described.text("/ChangeSet/0/ChangeName"),
                name,
                "case {case:?}: the change set must report the change name it was started \
                 with, got {:?}",
                described.body
            );
            continue;
        }

        assert_eq!(
            (
                answered.status,
                answered.error_code(),
                answered.content_type.as_str(),
                answered.errortype.as_str(),
            ),
            (422, "ValidationException", JSON, "ValidationException"),
            "case {case:?}: a change name outside {CHANGE_NAME_PATTERN} must be refused \
             422 ValidationException as application/json, got {} {:?} with {:?}",
            answered.status,
            answered.error_code(),
            answered.body
        );
        assert_eq!(
            answered.message(),
            format!(
                "[Requested change name '{name}' is invalid. It should match with \
                 {CHANGE_NAME_PATTERN}.]"
            ),
            "case {case:?}: the refusal must quote the value and the syntax it required, \
             got {:?}",
            answered.message()
        );
        assert_eq!(
            change_set_ids(&router).await,
            Vec::<String>::new(),
            "case {case:?}: a refused StartChangeSet must start nothing"
        );
    }
}
