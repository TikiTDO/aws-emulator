// SPDX-License-Identifier: CC0-1.0

//! One `Filter` rule, shared by every list this service serves.
//!
//! `Filter` carries `Name` and `ValueList`, and the rule behind the pair is one: the
//! filter must name an attribute, that attribute must be one the list being called
//! filters on, and the filter must carry at least one value to match. A list judges a
//! filter it accepts by selecting with it, and refuses one it does not with the modelled
//! `ValidationException` (422) under the member at fault — `FilterList.Name` for the
//! attribute, `FilterList.ValueList` for the values.
//!
//! The table below drives that one decision through BOTH lists — `ListChangeSets`, whose
//! published attributes are the seven `Filter$ValueList` documents for it, and
//! `ListEntities`, whose single published attribute is the `EntityId` `Filter$Name`
//! documents for it — over each published attribute, over an attribute the other list
//! publishes but this one does not, over a filter with no `Name` at all, and over both
//! spellings of "no value to match" — an absent `ValueList`, which the rule itself
//! refuses, and one present but empty, which the modelled `@length(min: 1)` on the list
//! shape refuses at the boundary before the rule is reached.
//!
//! The same rule judges a filter's VALUES against the attribute it selects on wherever the
//! model enumerates that attribute's values: the `Status` filter of `ListChangeSets`
//! selects within `ChangeStatus`, so each member is accepted (selecting an empty page when
//! no change set carries it) and a value naming no member is refused rather than silently
//! matching nothing.

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

/// The entity type the change set under test creates.
const ENTITY_TYPE: &str = "AmiProduct";

/// The attributes `ListChangeSets` filters on, from `Filter$ValueList`'s own doc ("The
/// supported filter names and associated `ValueList`s is as follows: ChangeSetName,
/// Status, EntityId, BeforeStartTime, AfterStartTime, BeforeEndTime, AfterEndTime").
/// Spelled out here so the test judges the service against the model rather than against
/// the service's own constant.
const CHANGE_SET_FILTER_NAMES: &[&str] = &[
    "ChangeSetName",
    "Status",
    "EntityId",
    "BeforeStartTime",
    "AfterStartTime",
    "BeforeEndTime",
    "AfterEndTime",
];

/// The attributes `ListEntities` filters on, from `Filter$Name`'s own doc ("For
/// `ListEntities`, the supported value for this is an `EntityId`").
const ENTITY_FILTER_NAMES: &[&str] = &["EntityId"];

/// The `ListChangeSets` attributes whose `ValueList` is a single ISO-8601 instant rather
/// than a set of values to match ("all change sets that started/ended before/after the
/// filter value").
const TIME_FILTER_NAMES: &[&str] = &[
    "BeforeStartTime",
    "AfterStartTime",
    "BeforeEndTime",
    "AfterEndTime",
];

/// An instant every change set in this store started and ended after, so a `Before…`
/// bound selects none and an `After…` bound selects them all.
const BEFORE_EVERYTHING: &str = "2000-01-01T00:00:00Z";

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

    /// The fault this answer carries: `ValidationException$Message`.
    fn message(&self) -> String {
        self.body["Message"]
            .as_str()
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

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", "application/json")
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
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// One change set creating one entity, and the id of the entity it made.
async fn seed(router: &MultiServiceRouter) -> String {
    let started = call(
        router,
        "/StartChangeSet",
        json!({
            "Catalog": CATALOG,
            "ChangeSetName": "filter-validation",
            "ChangeSet": [{
                "ChangeType": "CreateProduct",
                "Entity": {"Type": format!("{ENTITY_TYPE}@1.0")},
                "Details": json!({"ProductTitle": "filter-validation"}).to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the seed change set must answer 200, got {} with {:?}",
        started.status, started.body
    );
    wait_for_success();

    let entities = call(
        router,
        "/ListEntities",
        json!({"Catalog": CATALOG, "EntityType": ENTITY_TYPE}),
    )
    .await;
    let listed = entities.list("/EntitySummaryList");
    assert_eq!(
        listed.len(),
        1,
        "the seed change set must have made one entity, got {:?}",
        entities.body
    );
    listed[0]["EntityId"]
        .as_str()
        .expect("an entity summary names its id")
        .to_string()
}

/// One list operation and the attributes it does and does not filter on.
struct ListUnderTest {
    /// The operation's name, for the case label.
    list: &'static str,
    /// The URI that reaches it.
    uri: &'static str,
    /// The request members every case sends, beside `FilterList`.
    base: Value,
    /// The attributes this list filters on.
    published: &'static [&'static str],
    /// Attributes this service knows that this list does NOT filter on.
    unpublished: Vec<&'static str>,
}

/// A `ValueList` every attribute of `list` accepts: one instant for a before/after bound,
/// and the value the row carries for everything else.
fn accepted_values(name: &str, entity_id: &str) -> Vec<String> {
    if TIME_FILTER_NAMES.contains(&name) {
        return vec![BEFORE_EVERYTHING.to_string()];
    }
    match name {
        "EntityId" => vec![entity_id.to_string()],
        "Status" => vec!["SUCCEEDED".to_string()],
        _ => vec!["filter-validation".to_string()],
    }
}

/// Every attribute each list publishes is accepted, with a `ValueList` that attribute can
/// compare against: the call is served, not refused.
#[tokio::test]
async fn every_published_filter_attribute_is_accepted_by_its_list() {
    let router = emulator();
    let entity_id = seed(&router).await;

    let lists: Vec<(&str, &str, &[&str], Value)> = vec![
        (
            "ListChangeSets",
            "/ListChangeSets",
            CHANGE_SET_FILTER_NAMES,
            json!({"Catalog": CATALOG}),
        ),
        (
            "ListEntities",
            "/ListEntities",
            ENTITY_FILTER_NAMES,
            json!({"Catalog": CATALOG, "EntityType": ENTITY_TYPE}),
        ),
    ];

    for (list, uri, published, base) in lists {
        for name in published {
            let case = format!("{list} filtering on {name}");
            let mut request = base.clone();
            request["FilterList"] =
                json!([{"Name": name, "ValueList": accepted_values(name, &entity_id)}]);
            let answer = call(&router, uri, request).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a published attribute must be accepted, got {} with {:?}",
                answer.status, answer.body
            );
        }
    }
}

/// The three refusals the one rule produces, driven through BOTH lists: a filter with no
/// attribute, a filter naming an attribute the list does not publish (the attributes the
/// OTHER list publishes, so each row is an attribute this service knows but this list does
/// not filter on), and a filter carrying no value to match, in both spellings.
#[tokio::test]
async fn one_filter_rule_refuses_alike_in_every_list() {
    let router = emulator();
    let entity_id = seed(&router).await;

    let lists = vec![
        ListUnderTest {
            list: "ListChangeSets",
            uri: "/ListChangeSets",
            base: json!({"Catalog": CATALOG}),
            published: CHANGE_SET_FILTER_NAMES,
            unpublished: vec!["Visibility", "LastModifiedDate", "ProductTitle"],
        },
        ListUnderTest {
            list: "ListEntities",
            uri: "/ListEntities",
            base: json!({"Catalog": CATALOG, "EntityType": ENTITY_TYPE}),
            published: ENTITY_FILTER_NAMES,
            unpublished: CHANGE_SET_FILTER_NAMES
                .iter()
                .copied()
                .filter(|name| !ENTITY_FILTER_NAMES.contains(name))
                .collect(),
        },
    ];

    for ListUnderTest {
        list,
        uri,
        base,
        published,
        unpublished,
    } in lists
    {
        // A filter that names no attribute at all is refused under `FilterList.Name`.
        let mut rows: Vec<(String, Value, &str)> = vec![(
            format!("{list}: a filter naming no attribute"),
            json!([{"ValueList": ["anything"]}]),
            "Name",
        )];
        // An attribute this list does not filter on, again under `FilterList.Name`.
        for name in &unpublished {
            rows.push((
                format!("{list}: the unpublished attribute {name}"),
                json!([{"Name": name, "ValueList": ["anything"]}]),
                "Name",
            ));
        }
        // "No value to match", on every attribute this list DOES publish, under
        // `FilterList.ValueList`. The other spelling of it — a `ValueList` present but
        // empty — is the modelled `@length(min: 1)` on the list shape, so it is refused
        // at the boundary and asserted separately below.
        for name in published {
            rows.push((
                format!("{list}: {name} with no ValueList"),
                json!([{"Name": name}]),
                "ValueList",
            ));
        }
        // A well-formed filter beside the offending one does not rescue the request.
        rows.push((
            format!("{list}: a good filter beside one naming no attribute"),
            json!([
                {"Name": published[0], "ValueList": accepted_values(published[0], &entity_id)},
                {"ValueList": ["anything"]},
            ]),
            "Name",
        ));

        for (case, filters, member) in rows {
            let mut request = base.clone();
            request["FilterList"] = filters;
            let answer = call(&router, uri, request).await;
            assert_eq!(
                answer.status, 422,
                "case {case:?}: must be refused with 422, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: must answer ValidationException, got {:?}",
                answer.body
            );
            let message = answer.message();
            assert!(
                message.contains(&format!("FilterList.{member}")),
                "case {case:?}: the fault must name FilterList.{member}, got {message:?}"
            );
        }

        // The empty `ValueList`: `ValueList` is modelled `@length(min: 1, max: 10)`
        // (`smithy_query(query="shape", shape="ValueList")`), so an empty one is a
        // constraint violation refused as a client error naming that member, in both
        // lists alike.
        for name in published {
            let case = format!("{list}: {name} with an empty ValueList");
            let mut request = base.clone();
            request["FilterList"] = json!([{"Name": name, "ValueList": []}]);
            let answer = call(&router, uri, request).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a constraint violation must be refused with 400, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: must answer ValidationException, got {:?}",
                answer.body
            );
            let reported = answer.body["message"]
                .as_str()
                .or_else(|| answer.body["Message"].as_str())
                .unwrap_or_default()
                .to_string();
            assert!(
                reported.to_lowercase().contains("valuelist"),
                "case {case:?}: the fault must name the ValueList member, got {reported:?}"
            );
        }
    }
}

/// The members of the model's `ChangeStatus` enum, which `Filter$ValueList` names as the
/// values the `Status` filter selects within ("`Status` - The supported `ValueList` is a
/// list of statuses for all change set requests", `smithy_query(query="shape",
/// shape="Filter")`; the members themselves from `smithy_query(query="shape",
/// shape="ChangeStatus")`). Spelled out here so the test judges the service against the
/// model rather than against the service's own constant.
const CHANGE_STATUS_VALUES: &[&str] = &["PREPARING", "APPLYING", "SUCCEEDED", "CANCELLED", "FAILED"];

/// Every member of `ChangeStatus` is a `Status` filter value the list accepts — including
/// the statuses nothing in this store carries, which select an empty page rather than being
/// refused — and a value naming NO member is refused instead of selecting nothing.
#[tokio::test]
async fn the_status_filter_judges_its_values_against_the_published_statuses() {
    let router = emulator();
    seed(&router).await;

    // Every published member, one row each, plus a `ValueList` carrying several of them:
    // a value the attribute can hold is served, whether or not any change set holds it.
    let mut accepted: Vec<(String, Vec<&str>)> = CHANGE_STATUS_VALUES
        .iter()
        .map(|status| (format!("the published status {status}"), vec![*status]))
        .collect();
    accepted.push((
        "every published status at once".to_string(),
        CHANGE_STATUS_VALUES.to_vec(),
    ));
    accepted.push((
        "two published statuses".to_string(),
        vec!["SUCCEEDED", "FAILED"],
    ));

    for (case, values) in accepted {
        let answer = call(
            &router,
            "/ListChangeSets",
            json!({
                "Catalog": CATALOG,
                "FilterList": [{"Name": "Status", "ValueList": values}],
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a published status must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // A value outside the published set, varied along how it misses it: the casing of a
    // member, a typo, a status word from another API's vocabulary, a member of another
    // enum of THIS model, an empty string, and a good member beside a bad one.
    let refused: Vec<(String, Vec<&str>, &str)> = vec![
        (
            "a published member in another casing".to_string(),
            vec!["Succeeded"],
            "Succeeded",
        ),
        (
            "a published member lower-cased".to_string(),
            vec!["failed"],
            "failed",
        ),
        ("a typo of a member".to_string(), vec!["SUCEEDED"], "SUCEEDED"),
        (
            "a status word from another API".to_string(),
            vec!["COMPLETED"],
            "COMPLETED",
        ),
        (
            "a member of another enum of this model".to_string(),
            vec!["CLIENT_ERROR"],
            "CLIENT_ERROR",
        ),
        (
            "a member carrying trailing space".to_string(),
            vec!["SUCCEEDED "],
            "SUCCEEDED ",
        ),
        (
            "a published member beside one naming no status".to_string(),
            vec!["SUCCEEDED", "Succeeded"],
            "Succeeded",
        ),
    ];

    for (case, values, offending) in refused {
        let answer = call(
            &router,
            "/ListChangeSets",
            json!({
                "Catalog": CATALOG,
                "FilterList": [{"Name": "Status", "ValueList": values}],
            }),
        )
        .await;
        assert_eq!(
            answer.status, 422,
            "case {case:?}: a value naming no status must be refused with 422, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: must answer ValidationException, got {:?}",
            answer.body
        );
        let message = answer.message();
        assert!(
            message.contains(offending),
            "case {case:?}: the fault must name the rejected value {offending:?}, \
             got {message:?}"
        );
        for status in CHANGE_STATUS_VALUES {
            assert!(
                message.contains(status),
                "case {case:?}: the fault must name the published status {status}, \
                 got {message:?}"
            );
        }
        let served = answer.list("/ChangeSetSummaryList").len();
        assert_eq!(
            served, 0,
            "case {case:?}: a refused call serves no page, got {served} in {:?}",
            answer.body
        );
    }
}

/// The accepted filters select from STATE: the published attributes of each list pick the
/// records that carry the value asked for and leave out the ones that do not.
#[tokio::test]
async fn an_accepted_filter_selects_from_state() {
    let router = emulator();
    let entity_id = seed(&router).await;

    // (case, uri, request, pointer, expected count)
    let cases: Vec<(String, &str, Value, &str, usize)> = vec![
        (
            "ListChangeSets: the Status the seed change set reached".to_string(),
            "/ListChangeSets",
            json!({
                "Catalog": CATALOG,
                "FilterList": [{"Name": "Status", "ValueList": ["SUCCEEDED"]}],
            }),
            "/ChangeSetSummaryList",
            1,
        ),
        (
            "ListChangeSets: a Status no change set here reached".to_string(),
            "/ListChangeSets",
            json!({
                "Catalog": CATALOG,
                "FilterList": [{"Name": "Status", "ValueList": ["FAILED"]}],
            }),
            "/ChangeSetSummaryList",
            0,
        ),
        (
            "ListChangeSets: an instant every change set started after".to_string(),
            "/ListChangeSets",
            json!({
                "Catalog": CATALOG,
                "FilterList": [{"Name": "AfterStartTime", "ValueList": [BEFORE_EVERYTHING]}],
            }),
            "/ChangeSetSummaryList",
            1,
        ),
        (
            "ListChangeSets: an instant no change set started before".to_string(),
            "/ListChangeSets",
            json!({
                "Catalog": CATALOG,
                "FilterList": [{"Name": "BeforeStartTime", "ValueList": [BEFORE_EVERYTHING]}],
            }),
            "/ChangeSetSummaryList",
            0,
        ),
        (
            "ListEntities: the id the seed change set minted".to_string(),
            "/ListEntities",
            json!({
                "Catalog": CATALOG,
                "EntityType": ENTITY_TYPE,
                "FilterList": [{"Name": "EntityId", "ValueList": [entity_id.clone()]}],
            }),
            "/EntitySummaryList",
            1,
        ),
        (
            "ListEntities: an id this store does not hold".to_string(),
            "/ListEntities",
            json!({
                "Catalog": CATALOG,
                "EntityType": ENTITY_TYPE,
                "FilterList": [{"Name": "EntityId", "ValueList": ["ent-000000000000"]}],
            }),
            "/EntitySummaryList",
            0,
        ),
    ];

    for (case, uri, request, pointer, expected) in cases {
        let answer = call(&router, uri, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let served = answer.list(pointer).len();
        assert_eq!(
            served, expected,
            "case {case:?}: the filter must select {expected} record(s) from state, \
             got {served} in {:?}",
            answer.body
        );
    }
}
