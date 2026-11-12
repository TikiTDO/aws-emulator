// SPDX-License-Identifier: CC0-1.0

//! The (entity, change type) uniqueness rule of `StartChangeSet`, under every `Intent`.
//!
//! RULE: "Within a single ChangeSet, you can't start the same change type against the same
//! entity multiple times" (`smithy_query(query="doc", shape="StartChangeSet")`) is a rule
//! about the SUBMITTED REQUEST, so `StartChangeSet` reaches the same verdict under every
//! value of `Intent` — documented "The default is APPLY. To test your request before
//! applying changes to your entities, use VALIDATE"
//! (`smithy_query(query="doc", shape="StartChangeSetRequest", member="Intent")`) — and it
//! holds however the two changes reach ONE entity: by naming the same identifier, by
//! addressing one entity at different revisions, or by one change naming, directly or
//! through a chain of `$<ChangeName>.Entity.Identifier` references, the change that acts on
//! or creates it. A change set that repeats such a pair is refused 422 `ValidationException`
//! naming the repeated change type and the entity, and starts nothing; a change set that
//! repeats no pair is started.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rows differ along what the rule generalises over: how two changes reach
//! one entity (the same bare id, an id and that id at a revision, a reference to a creating
//! change, a reference to a change acting on an existing entity, a chain of two references,
//! two changes naming one reference), whether the entity already exists or the change set
//! creates it, how many changes the set carries, and which change types and entity types
//! they use — and every row is run under each `Intent`.

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

/// The values of `Intent`, which every row is run under.
///
/// Source: `Intent` is an enum of exactly these two (`smithy_query(query="shape",
/// shape="Intent")`).
const INTENTS: [&str; 2] = ["APPLY", "VALIDATE"];

/// How long after the start a change set has certainly reached a terminal status: the two
/// spans the implementation publishes, plus a margin for a slow machine.
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

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> String {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        raw.rsplit('#').next().unwrap_or(raw).to_string()
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

/// `POST /StartChangeSet` with `changes` as the whole `ChangeSet`, under `intent`.
async fn start(router: &MultiServiceRouter, intent: &str, changes: Value) -> Answer {
    call(
        router,
        "POST",
        "/StartChangeSet",
        Some(json!({"Catalog": CATALOG, "Intent": intent, "ChangeSet": changes})),
    )
    .await
}

/// One change: a change type, the entity type it acts under, the identifier it names (none
/// for a change that creates an entity of its own) and the `ChangeName` the other changes
/// may name it by.
fn change(
    change_type: &str,
    entity_type: &str,
    identifier: Option<&str>,
    change_name: Option<&str>,
) -> Value {
    let mut entity = json!({"Type": format!("{entity_type}@1.0")});
    if let Some(identifier) = identifier {
        entity["Identifier"] = json!(identifier);
    }
    let mut built = json!({
        "ChangeType": change_type,
        "Entity": entity,
        "DetailsDocument": {"ProductTitle": "pair uniqueness under every intent"},
    });
    if let Some(name) = change_name {
        built["ChangeName"] = json!(name);
    }
    built
}

/// Every change set the store holds, by id — so a case can assert a refused
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

/// Create one `AmiProduct` through an applied change set and answer the entity id it made,
/// read back out of what the store reports.
async fn created_entity(router: &MultiServiceRouter) -> String {
    let started = start(
        router,
        "APPLY",
        json!([change("CreateProduct", "AmiProduct", None, None)]),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the fixture's create must answer 200, got {} with {:?}",
        started.status, started.body
    );
    let change_set_id = started.text("/ChangeSetId");
    wait_for_the_end_of_the_run();

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
/// rather than taken from the crate so a wrong implementation cannot make this pass.
fn duplicate_pair_message(change_type: &str, entity: &str) -> String {
    format!(
        "[Requested changes contain multiple changes for the same change type \
         '{change_type}' on entity '{entity}'. Entity - change type pairs \
         should be unique.]"
    )
}

/// A message with every minted entity id in it replaced by a placeholder.
///
/// The id of an entity a change set CREATES is minted per change set, so two runs of one
/// request name two different new entities while saying the same thing about them.
fn without_entity_ids(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(at) = rest.find("ent-") {
        out.push_str(&rest[..at]);
        out.push_str("<entity>");
        rest = &rest[at + "ent-".len()..];
        let tail = rest
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        rest = &rest[tail..];
    }
    out.push_str(rest);
    out
}

/// Which entity the refusal of a row names.
enum Named {
    /// The entity the fixture created, named by its own id.
    Fixture,
    /// An entity the change set itself creates, whose id is minted per run.
    Minted,
    /// The identifier text the changes named, when no entity was resolved for it.
    Literal(&'static str),
}

/// What a row expects of `StartChangeSet`, under either intent.
enum Expect {
    /// The change set repeats a pair: refused 422 `ValidationException` naming this change
    /// type and this entity, and nothing started.
    Refused {
        change_type: &'static str,
        entity: Named,
    },
    /// The change set repeats no pair, so it is started.
    Started,
}

/// One row: the case, whether it needs an entity to exist first, how it builds its change
/// set from that entity's id, and what `StartChangeSet` must answer.
struct Row {
    case: &'static str,
    existing: bool,
    build: fn(&str) -> Value,
    expect: Expect,
}

/// RULE: the (entity, change type) uniqueness rule is a rule about the request, so every
/// `Intent` reaches the same verdict on it — and the rule is judged on the entity each
/// change RESOLVES to, which is what sees two changes reaching one entity when one of them
/// names the change that acts on or creates it.
#[tokio::test]
async fn every_intent_judges_the_entity_change_type_pair_the_same_way() {
    let rows: Vec<Row> = vec![
        Row {
            case: "a creating change and a change naming it by reference",
            existing: false,
            build: |_| {
                json!([
                    change("AddRevisions", "AmiProduct", None, Some("Create")),
                    change(
                        "AddRevisions",
                        "AmiProduct",
                        Some("$Create.Entity.Identifier"),
                        None,
                    ),
                ])
            },
            expect: Expect::Refused {
                change_type: "AddRevisions",
                entity: Named::Minted,
            },
        },
        Row {
            case: "two changes reaching the created entity through a chain of two references",
            existing: false,
            build: |_| {
                json!([
                    change("CreateProduct", "SaaSProduct", None, Some("Root")),
                    change(
                        "UpdateInformation",
                        "SaaSProduct",
                        Some("$Root.Entity.Identifier"),
                        Some("Middle"),
                    ),
                    change(
                        "UpdateInformation",
                        "SaaSProduct",
                        Some("$Middle.Entity.Identifier"),
                        None,
                    ),
                ])
            },
            expect: Expect::Refused {
                change_type: "UpdateInformation",
                entity: Named::Minted,
            },
        },
        Row {
            case: "two changes naming one and the same reference",
            existing: false,
            build: |_| {
                json!([
                    change("CreateProduct", "DataProduct", None, Some("Root")),
                    change(
                        "AddDeliveryOptions",
                        "DataProduct",
                        Some("$Root.Entity.Identifier"),
                        None,
                    ),
                    change(
                        "AddDeliveryOptions",
                        "DataProduct",
                        Some("$Root.Entity.Identifier"),
                        None,
                    ),
                ])
            },
            // Both copies NAME the same identifier, so the refusal names that identifier:
            // the pair is taken before any entity is resolved for it.
            expect: Expect::Refused {
                change_type: "AddDeliveryOptions",
                entity: Named::Literal("$Root.Entity.Identifier"),
            },
        },
        Row {
            case: "an existing entity named twice by its id",
            existing: true,
            build: |entity| {
                json!([
                    change("AddRevisions", "AmiProduct", Some(entity), None),
                    change("AddRevisions", "AmiProduct", Some(entity), None),
                ])
            },
            expect: Expect::Refused {
                change_type: "AddRevisions",
                entity: Named::Fixture,
            },
        },
        Row {
            case: "an existing entity named by its id and then at a revision",
            existing: true,
            build: |entity| {
                json!([
                    change("UpdateInformation", "AmiProduct", Some(entity), None),
                    change(
                        "UpdateInformation",
                        "AmiProduct",
                        Some(&format!("{entity}@3")),
                        None,
                    ),
                ])
            },
            expect: Expect::Refused {
                change_type: "UpdateInformation",
                entity: Named::Fixture,
            },
        },
        Row {
            case: "an existing entity named by one change and reached by a reference to that \
                   change",
            existing: true,
            build: |entity| {
                json!([
                    change("ReleaseProduct", "AmiProduct", Some(entity), Some("First")),
                    change(
                        "ReleaseProduct",
                        "AmiProduct",
                        Some("$First.Entity.Identifier"),
                        None,
                    ),
                ])
            },
            expect: Expect::Refused {
                change_type: "ReleaseProduct",
                entity: Named::Fixture,
            },
        },
        Row {
            case: "a creating change and a reference to it under another change type",
            existing: false,
            build: |_| {
                json!([
                    change("CreateProduct", "AmiProduct", None, Some("Create")),
                    change(
                        "AddRevisions",
                        "AmiProduct",
                        Some("$Create.Entity.Identifier"),
                        None,
                    ),
                ])
            },
            expect: Expect::Started,
        },
        Row {
            case: "a chain of three references under three change types",
            existing: false,
            build: |_| {
                json!([
                    change("CreateProduct", "SaaSProduct", None, Some("Root")),
                    change(
                        "AddRevisions",
                        "SaaSProduct",
                        Some("$Root.Entity.Identifier"),
                        Some("Middle"),
                    ),
                    change(
                        "UpdateInformation",
                        "SaaSProduct",
                        Some("$Middle.Entity.Identifier"),
                        None,
                    ),
                ])
            },
            expect: Expect::Started,
        },
        Row {
            case: "two creating changes of one change type, each making its own entity",
            existing: false,
            build: |_| {
                json!([
                    change("CreateProduct", "ContainerProduct", None, None),
                    change("CreateProduct", "ContainerProduct", None, None),
                ])
            },
            expect: Expect::Started,
        },
        Row {
            case: "one change type against an existing entity and against a new one",
            existing: true,
            build: |entity| {
                json!([
                    change("AddRevisions", "AmiProduct", Some(entity), None),
                    change("AddRevisions", "AmiProduct", None, None),
                ])
            },
            expect: Expect::Started,
        },
    ];

    for row in &rows {
        let mut answers: Vec<(&str, u16, String, String)> = Vec::new();
        for intent in INTENTS {
            let case = format!("{} under {intent}", row.case);
            let case = case.as_str();
            let router = emulator();
            let fixture = if row.existing {
                created_entity(&router).await
            } else {
                String::new()
            };
            let started_before = change_set_ids(&router).await;

            let answered = start(&router, intent, (row.build)(&fixture)).await;
            match &row.expect {
                Expect::Refused {
                    change_type,
                    entity,
                } => {
                    assert_eq!(
                        (answered.status, answered.error_code().as_str()),
                        (422, "ValidationException"),
                        "case {case:?}: a change set repeating a change type against one \
                         entity must be refused 422 ValidationException, got {} {:?} with \
                         {:?}",
                        answered.status,
                        answered.error_code(),
                        answered.body
                    );
                    let expected = match entity {
                        Named::Fixture => duplicate_pair_message(change_type, &fixture),
                        Named::Minted => duplicate_pair_message(change_type, "<entity>"),
                        Named::Literal(text) => duplicate_pair_message(change_type, text),
                    };
                    let actual = match entity {
                        Named::Minted => without_entity_ids(&answered.message()),
                        _ => answered.message(),
                    };
                    assert_eq!(
                        actual, expected,
                        "case {case:?}: the refusal must name the repeated change type and \
                         the entity it was repeated on, expected {expected:?}, got {actual:?}"
                    );
                    assert_eq!(
                        change_set_ids(&router).await,
                        started_before,
                        "case {case:?}: a refused StartChangeSet must start nothing, held \
                         {started_before:?} before"
                    );
                }
                Expect::Started => {
                    assert_eq!(
                        answered.status, 200,
                        "case {case:?}: a change set repeating no (entity, change type) pair \
                         must be started, got {} with {:?}",
                        answered.status, answered.body
                    );
                    let id = answered.text("/ChangeSetId");
                    let held = change_set_ids(&router).await;
                    assert!(
                        held.contains(&id),
                        "case {case:?}: the started change set {id:?} must be held by the \
                         store, got {held:?}"
                    );
                }
            }
            answers.push((
                intent,
                answered.status,
                answered.error_code(),
                without_entity_ids(&answered.message()),
            ));
        }

        // The verdict belongs to the request, so it is the same under every intent — the
        // same status, the same error code and the same message about the same pair.
        let (first_intent, first_status, first_code, first_message) = answers[0].clone();
        for (intent, status, code, message) in &answers[1..] {
            assert_eq!(
                (*status, code.as_str(), message.as_str()),
                (first_status, first_code.as_str(), first_message.as_str()),
                "case {case:?}: {intent} must answer what {first_intent} answers for the \
                 same request, got {status} {code:?} {message:?} against {first_status} \
                 {first_code:?} {first_message:?}",
                case = row.case,
            );
        }
    }
}
