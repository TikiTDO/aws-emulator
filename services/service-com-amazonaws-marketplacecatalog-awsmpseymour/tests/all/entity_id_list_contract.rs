// SPDX-License-Identifier: CC0-1.0

//! What `ListChangeSets` reports for `ChangeSetSummaryListItem$EntityIdList`, end to end
//! over HTTP.
//!
//! RULE: a response list member whose contract excludes the empty case is reported only
//! when the store holds something for it — `EntityIdList` is documented as "a list of
//! entity IDs (string) that are a part of a change set. The entity ID list is a maximum of
//! 20 entities. It must contain at least one entity"
//! (`smithy_query(query="shape", shape="ChangeSetSummaryListItem")`), so a summary either
//! names between one and twenty entities or carries no such member at all — never an empty
//! array, which that bound excludes.
//!
//! The entities a change set is a part of are the ones its changes RESOLVED to: the entity
//! each change named, and the entity a change that names none creates — which only an
//! `APPLY` change set does, a validation creating nothing. So a change set every change of
//! which merely validates a creation holds no entity, and reports no member.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rows differ along what the rule generalises over: the intent, how many
//! changes the change set carries (one, two, three, and the twenty its request list's
//! maximum allows), whether each change creates an entity or names an existing one, the
//! form of the identifier it names it by, the change types and entity types involved, and
//! whether two changes reach one entity.

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

/// The maximum number of entities `ChangeSetSummaryListItem$EntityIdList` publishes ("The
/// entity ID list is a maximum of 20 entities"), which is also the maximum number of
/// changes `RequestedChangeList` accepts (`@length(min: 1, max: 20)`).
const MAX_ENTITIES: usize = 20;

/// How many entities the rows that name an existing one need between them.
const FIXTURE_ENTITIES: usize = 7;

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

/// `POST /StartChangeSet` with `changes` as the whole `ChangeSet`, under `intent`, and the
/// change set id it answers.
async fn start(router: &MultiServiceRouter, case: &str, intent: &str, changes: Value) -> String {
    let started = call(
        router,
        "POST",
        "/StartChangeSet",
        Some(json!({"Catalog": CATALOG, "Intent": intent, "ChangeSet": changes})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartChangeSet must answer 200, got {} with {:?}",
        started.status, started.body
    );
    started.text("/ChangeSetId")
}

/// Every `ChangeSetSummaryListItem` the store serves, following `NextToken` so the answer
/// does not depend on how many change sets fit in one page.
async fn summaries(router: &MultiServiceRouter) -> Vec<Value> {
    let mut all: Vec<Value> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut request = json!({"Catalog": CATALOG});
        if let Some(token) = next {
            request["NextToken"] = json!(token);
        }
        let listed = call(router, "POST", "/ListChangeSets", Some(request)).await;
        assert_eq!(
            listed.status, 200,
            "ListChangeSets must answer 200, got {} with {:?}",
            listed.status, listed.body
        );
        all.extend(listed.list("/ChangeSetSummaryList"));
        next = listed
            .body
            .pointer("/NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next.is_none() {
            return all;
        }
    }
}

/// The summary of the change set `id`.
async fn summary_of(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let all = summaries(router).await;
    all.iter()
        .find(|item| item["ChangeSetId"] == json!(id))
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListChangeSets must report the change set {id:?}: {all:?}")
        })
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
        "DetailsDocument": {"ProductTitle": "entity-id-list"},
    })
}

/// Create `FIXTURE_ENTITIES` entities in ONE applied change set and answer their ids, read
/// back out of the `EntityIdList` that change set reports — which is also the `APPLY` side
/// of the rule: a change set that creates entities names every one of them.
async fn fixture_entities(router: &MultiServiceRouter) -> Vec<String> {
    let changes: Vec<Value> = (0..FIXTURE_ENTITIES)
        .map(|_| change("CreateProduct", "AmiProduct", None))
        .collect();
    let id = start(router, "the fixture", "APPLY", json!(changes)).await;
    wait_for_success();

    let summary = summary_of(router, "the fixture", &id).await;
    let ids: Vec<String> = summary["EntityIdList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids.len(),
        FIXTURE_ENTITIES,
        "the fixture's applied change set must name the {FIXTURE_ENTITIES} entities it \
         creates, got {summary:?}"
    );
    ids
}

/// How one row builds its `ChangeSet` from the fixture entities allotted to it.
type BuildChangeSet = fn(&[String]) -> Value;

/// One row: the case, the intent, how many fixture entities it names, how it builds its
/// change set, and what `EntityIdList` must then report — the fixture entities it names
/// (deduplicated) and how many entity ids the change set mints of its own.
struct Row {
    case: &'static str,
    intent: &'static str,
    needs: usize,
    build: BuildChangeSet,
    named: usize,
    minted: usize,
}

/// RULE: `EntityIdList` names the entities the change set's changes resolved to — every
/// entity a change named, plus one per change that creates an entity, which only an
/// `APPLY` change set does — and the member is reported only when there is at least one of
/// them, because the member's documented bound ("a maximum of 20 entities. It must contain
/// at least one entity") excludes the empty list.
#[tokio::test]
async fn an_entity_id_list_is_reported_only_when_it_names_an_entity() {
    let rows: Vec<Row> = vec![
        Row {
            case: "APPLY, one change creating an entity",
            intent: "APPLY",
            needs: 0,
            build: |_| json!([change("CreateProduct", "AmiProduct", None)]),
            named: 0,
            minted: 1,
        },
        Row {
            case: "APPLY, two changes each creating an entity",
            intent: "APPLY",
            needs: 0,
            build: |_| {
                json!([
                    change("CreateProduct", "SaaSProduct", None),
                    change("CreateProduct", "DataProduct", None),
                ])
            },
            named: 0,
            minted: 2,
        },
        Row {
            case: "APPLY, the twenty changes its request list's maximum allows",
            intent: "APPLY",
            needs: 0,
            build: |_| {
                json!((0..MAX_ENTITIES)
                    .map(|_| change("CreateProduct", "ContainerProduct", None))
                    .collect::<Vec<Value>>())
            },
            named: 0,
            minted: MAX_ENTITIES,
        },
        Row {
            case: "APPLY, a create beside a change naming an existing entity",
            intent: "APPLY",
            needs: 1,
            build: |entities| {
                json!([
                    change("CreateProduct", "MachineLearningProduct", None),
                    change("AddRevisions", "AmiProduct", Some(&entities[0])),
                ])
            },
            named: 1,
            minted: 1,
        },
        Row {
            case: "VALIDATE, one change creating an entity",
            intent: "VALIDATE",
            needs: 0,
            build: |_| json!([change("CreateProduct", "AmiProduct", None)]),
            named: 0,
            minted: 0,
        },
        Row {
            case: "VALIDATE, three changes all creating entities",
            intent: "VALIDATE",
            needs: 0,
            build: |_| {
                json!([
                    change("CreateProduct", "AmiProduct", None),
                    change("CreateProduct", "SaaSProduct", None),
                    change("CreateProduct", "DataProduct", None),
                ])
            },
            named: 0,
            minted: 0,
        },
        Row {
            case: "VALIDATE, the twenty changes its request list's maximum allows, all creating",
            intent: "VALIDATE",
            needs: 0,
            build: |_| {
                json!((0..MAX_ENTITIES)
                    .map(|_| change("CreateProduct", "DataProduct", None))
                    .collect::<Vec<Value>>())
            },
            named: 0,
            minted: 0,
        },
        Row {
            case: "VALIDATE, one change naming an existing entity",
            intent: "VALIDATE",
            needs: 1,
            build: |entities| json!([change("AddRevisions", "AmiProduct", Some(&entities[0]))]),
            named: 1,
            minted: 0,
        },
        Row {
            case: "VALIDATE, an existing entity named at a revision",
            intent: "VALIDATE",
            needs: 1,
            build: |entities| {
                json!([change(
                    "AddRevisions",
                    "AmiProduct",
                    Some(&format!("{}@1", entities[0])),
                )])
            },
            named: 1,
            minted: 0,
        },
        Row {
            case: "VALIDATE, a create beside a change naming an existing entity",
            intent: "VALIDATE",
            needs: 1,
            build: |entities| {
                json!([
                    change("CreateProduct", "ContainerProduct", None),
                    change("UpdateInformation", "AmiProduct", Some(&entities[0])),
                ])
            },
            named: 1,
            minted: 0,
        },
        Row {
            case: "VALIDATE, two change types reaching one existing entity",
            intent: "VALIDATE",
            needs: 1,
            build: |entities| {
                json!([
                    change("AddRevisions", "AmiProduct", Some(&entities[0])),
                    change("UpdateInformation", "AmiProduct", Some(&entities[0])),
                ])
            },
            named: 1,
            minted: 0,
        },
        Row {
            case: "VALIDATE, two changes naming two existing entities",
            intent: "VALIDATE",
            needs: 2,
            build: |entities| {
                json!([
                    change("AddRevisions", "AmiProduct", Some(&entities[0])),
                    change("AddRevisions", "AmiProduct", Some(&entities[1])),
                ])
            },
            named: 2,
            minted: 0,
        },
    ];

    let router = emulator();
    let entities = fixture_entities(&router).await;

    // Each row names fixture entities of its own, so no row's change set targets an entity
    // another row's is still running against.
    let mut taken = 0;
    // (case, change set id, the fixture entities allotted to it, how many of them it
    // names, the entity ids it must mint)
    let mut started: Vec<(&str, String, Vec<String>, usize, usize)> =
        Vec::with_capacity(rows.len());
    for row in &rows {
        let allotted = entities[taken..taken + row.needs].to_vec();
        taken += row.needs;
        let id = start(&router, row.case, row.intent, (row.build)(&allotted)).await;

        let summary = summary_of(&router, row.case, &id).await;
        assert_entity_id_list(row.case, &summary, &allotted, row.named, row.minted);
        started.push((row.case, id, allotted, row.named, row.minted));
    }
    assert_eq!(
        taken, FIXTURE_ENTITIES,
        "the rows must use every fixture entity, used {taken} of {FIXTURE_ENTITIES}"
    );

    // Completing changes nothing about which entities a change set is a part of, so the
    // same answer holds once the clock has carried every change set to a terminal status.
    wait_for_success();
    for (case, id, allotted, named, minted) in &started {
        let summary = summary_of(&router, case, id).await;
        assert_entity_id_list(case, &summary, allotted, *named, *minted);
    }

    // And across every summary the store serves: an empty `EntityIdList` is never one of
    // the answers, whatever the change set.
    for summary in summaries(&router).await {
        let held = summary.get("EntityIdList");
        assert!(
            held.is_none_or(|list| list.as_array().is_some_and(|ids| !ids.is_empty())),
            "no summary may carry an empty EntityIdList, got {summary:?}"
        );
    }
}

/// The whole rule, asserted on one summary: the member is absent when the change set is a
/// part of no entity, and otherwise names — within the published bound, each as a
/// `ResourceId`, without repeats — exactly the `named` first entities of `allotted` plus
/// `minted` ids none of them holds.
fn assert_entity_id_list(
    case: &str,
    summary: &Value,
    allotted: &[String],
    named: usize,
    minted: usize,
) {
    let expected_len = named + minted;
    let held = summary.get("EntityIdList");
    if expected_len == 0 {
        assert_eq!(
            held, None,
            "case {case:?}: a change set that is a part of no entity must report no \
             EntityIdList member, got {summary:?}"
        );
        return;
    }

    let ids: Vec<String> = held
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: EntityIdList must be reported as an array, got {summary:?}")
        })
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids.len(),
        expected_len,
        "case {case:?}: expected {expected_len} entity ids, got {ids:?}"
    );
    assert!(
        !ids.is_empty() && ids.len() <= MAX_ENTITIES,
        "case {case:?}: expected between 1 and {MAX_ENTITIES} entity ids, got {ids:?}"
    );
    for id in &ids {
        assert!(
            is_resource_id(id),
            "case {case:?}: every entity id must match ^[\\w\\-]+$, got {id:?}"
        );
        assert_eq!(
            ids.iter().filter(|seen| *seen == id).count(),
            1,
            "case {case:?}: an entity is named once however many changes reach it, got {ids:?}"
        );
    }
    for expected in &allotted[..named] {
        assert!(
            ids.contains(expected),
            "case {case:?}: the entity {expected:?} a change named must be listed, got {ids:?}"
        );
    }
    let fresh = ids.iter().filter(|id| !allotted.contains(id)).count();
    assert_eq!(
        fresh, minted,
        "case {case:?}: expected {minted} entity ids the change set minted of its own, \
         got {fresh} among {ids:?}"
    );
}
