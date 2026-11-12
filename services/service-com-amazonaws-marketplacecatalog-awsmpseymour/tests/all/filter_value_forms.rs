// SPDX-License-Identifier: CC0-1.0

//! A `Filter$ValueList` entry is judged against the FORM the attribute it selects on
//! takes, and a `FilterList` names each attribute once.
//!
//! A filter selects on an attribute, so a value that attribute's own values can never be
//! names nothing the attribute holds: it is refused with the same `ValidationException`
//! (422) the member carrying such a value answers anywhere else in this service, rather
//! than silently selecting the empty page it would match — which a caller cannot tell
//! from "this catalog holds no such record". A value that IS of the right form and names
//! nothing is the opposite case: it is served, as the empty page it selects.
//!
//! The two forms this service states for the values a caller chooses are the entity id
//! (`EntityId`'s `@pattern("^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$")` with its
//! `@length(min: 1, max: 255)`) and the change set name (`ChangeSetName`'s
//! `@pattern("^[\w\s+=.:@-]+$")` with its `@length(min: 1, max: 100)`), both from
//! `smithy_query(query="shape", shape="EntityId,ChangeSetName")`. The tables below drive
//! each one through every surface that carries such a value: the `EntityId` filter of
//! BOTH lists, the `ChangeSetName` filter of `ListChangeSets`, and
//! `AssessmentTargetFilter$EntityId`, which names an entity through the wider
//! `ResourceId` (`^[\w\-]+$`).
//!
//! Two further rules the `Filter` documentation states, and this suite pins: `Filter$Name`
//! is "the name of the filter, which is unique for each filter that you want to apply", so
//! one `FilterList` names an attribute at most once; and `Filter$ValueList` says per filter
//! whether its values are unique — "a list of unique `EntityId`s" against "a list of
//! non-unique `ChangeSetName`s" — so a repeated value is refused on the first and served on
//! the second.

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

/// The entity type the seed change set creates.
const ENTITY_TYPE: &str = "AmiProduct";

/// The name the seed change set is started with, which the `ChangeSetName` filter selects
/// it by.
const CHANGE_SET_NAME: &str = "filter-value-forms";

/// The form an entity id takes, as the model's own `EntityId` shape states it
/// (`smithy_query(query="shape", shape="EntityId")`). Spelled out here so the test judges
/// the service against the model rather than against the service's own constant — it is
/// also the syntax the refusal has to quote back, so a caller can see what was required.
const ENTITY_ID_PATTERN: &str = "^[a-zA-Z0-9][.a-zA-Z0-9/-]+[a-zA-Z0-9]$";

/// The longest any member of this service naming an entity may be: `EntityId`'s
/// `@length(min: 1, max: 255)`.
const ENTITY_ID_MAX: usize = 255;

/// The longest a change set name may be: `ChangeSetName`'s `@length(min: 1, max: 100)`.
const CHANGE_SET_NAME_MAX: usize = 100;

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
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "catalog.marketplace.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_string()))
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

/// One change set, named, creating one entity — and the id of the entity it made.
async fn seed(router: &MultiServiceRouter) -> String {
    let started = call(
        router,
        "/StartChangeSet",
        json!({
            "Catalog": CATALOG,
            "ChangeSetName": CHANGE_SET_NAME,
            "ChangeSet": [{
                "ChangeType": "CreateProduct",
                "Entity": {"Type": format!("{ENTITY_TYPE}@1.0")},
                "Details": json!({"ProductTitle": "filter-value-forms"}).to_string(),
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

/// One list operation that filters on `EntityId`, and where its page is reported.
struct ListUnderTest {
    list: &'static str,
    uri: &'static str,
    base: Value,
    page: &'static str,
}

fn lists_filtering_on_entity_id() -> Vec<ListUnderTest> {
    vec![
        ListUnderTest {
            list: "ListEntities",
            uri: "/ListEntities",
            base: json!({"Catalog": CATALOG, "EntityType": ENTITY_TYPE}),
            page: "/EntitySummaryList",
        },
        ListUnderTest {
            list: "ListChangeSets",
            uri: "/ListChangeSets",
            base: json!({"Catalog": CATALOG}),
            page: "/ChangeSetSummaryList",
        },
    ]
}

/// Values that are not entity ids at all, each row differing along how it misses the
/// published form: its length, the class of its first or last character, and the
/// characters in between — including the ones the wider `ResourceId` admits.
///
/// Every row is a value a `Filter$ValueList` entry may carry (`FilterValueContent`:
/// `@length(min: 1, max: 255)`, `@pattern("^(.)+$")`), so each one reaches the filter
/// rule rather than the modelled length or pattern at the boundary.
fn not_entity_ids() -> Vec<(String, String)> {
    vec![
        ("a single character".to_string(), "a".to_string()),
        (
            "two characters, one short of the shortest published id".to_string(),
            "ab".to_string(),
        ),
        (
            "an underscore, which only the wider ResourceId admits".to_string(),
            "_x_".to_string(),
        ),
        (
            "characters outside the published class".to_string(),
            "!!".to_string(),
        ),
        ("a leading hyphen".to_string(), "-abc".to_string()),
        ("a trailing dot".to_string(), "abc.".to_string()),
        ("an inner space".to_string(), "ab cd".to_string()),
        ("a non-ASCII letter".to_string(), "ábc".to_string()),
        ("a plus sign".to_string(), "a+b".to_string()),
    ]
}

/// The rows of [`not_entity_ids`] that are also values `ResourceId` (`^[\w\-]+$`) admits,
/// so they reach the handler of a member targeting it instead of that member's modelled
/// pattern at the boundary.
fn not_entity_ids_within_resource_id() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = not_entity_ids()
        .into_iter()
        .filter(|(_, value)| {
            value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                && !value.is_empty()
        })
        .collect();
    rows.push(("an inner underscore".to_string(), "a_b".to_string()));
    rows.push(("a trailing underscore".to_string(), "abc_".to_string()));
    rows
}

/// Values that ARE entity ids, naming nothing this catalog holds: the shortest the pattern
/// admits, each separator it admits, and the longest the length admits.
fn unheld_entity_ids() -> Vec<(String, String)> {
    vec![
        (
            "the shortest id the pattern admits".to_string(),
            "abc".to_string(),
        ),
        (
            "every separator the pattern admits".to_string(),
            "a.b/c-d".to_string(),
        ),
        (
            "an id in the minted form this store does not hold".to_string(),
            "ent-000000000000".to_string(),
        ),
        (
            "an id of the published maximum length".to_string(),
            "a".repeat(ENTITY_ID_MAX),
        ),
    ]
}

/// The rows of [`unheld_entity_ids`] that `ResourceId` (`^[\w\-]+$`) admits too — the
/// pattern of a member that NAMES an entity without targeting `EntityId`, which admits
/// neither the dot nor the slash an entity id may carry.
fn unheld_entity_ids_within_resource_id() -> Vec<(String, String)> {
    unheld_entity_ids()
        .into_iter()
        .filter(|(_, value)| {
            value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        })
        .collect()
}

/// Every list that filters on `EntityId` judges a filter value against the entity id form
/// before it selects: a value no entity id can be is refused as a 422 quoting the value
/// and the pattern, while a well-formed id naming nothing is served the empty page it
/// selects.
#[tokio::test]
async fn an_entity_id_filter_value_is_judged_against_the_entity_id_form() {
    let router = emulator();
    let held = seed(&router).await;

    for ListUnderTest {
        list,
        uri,
        base,
        page,
    } in lists_filtering_on_entity_id()
    {
        for (row, value) in not_entity_ids() {
            let case = format!("{list}: {row}");
            let mut request = base.clone();
            request["FilterList"] = json!([{"Name": "EntityId", "ValueList": [value.clone()]}]);
            let answer = call(&router, uri, request).await;
            assert_eq!(
                answer.status, 422,
                "case {case:?}: a value that is not an entity id must be refused with 422, \
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
                message.contains(&value),
                "case {case:?}: the fault must quote the rejected value {value:?}, \
                 got {message:?}"
            );
            assert!(
                message.contains(ENTITY_ID_PATTERN),
                "case {case:?}: the fault must quote the form required \
                 ({ENTITY_ID_PATTERN}), got {message:?}"
            );
            let served = answer.list(page).len();
            assert_eq!(
                served, 0,
                "case {case:?}: a refused call serves no page, got {served} in {:?}",
                answer.body
            );
        }

        for (row, value) in unheld_entity_ids() {
            let case = format!("{list}: {row}");
            let mut request = base.clone();
            request["FilterList"] = json!([{"Name": "EntityId", "ValueList": [value.clone()]}]);
            let answer = call(&router, uri, request).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: an entity id naming nothing held must be served, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            let served = answer.list(page).len();
            assert_eq!(
                served, 0,
                "case {case:?}: an id nothing holds selects an empty page, got {served} \
                 in {:?}",
                answer.body
            );
        }

        // The id this catalog DOES hold still selects, so the form check narrows nothing
        // a caller may ask for.
        let case = format!("{list}: the id the seed change set minted");
        let mut request = base.clone();
        request["FilterList"] = json!([{"Name": "EntityId", "ValueList": [held.clone()]}]);
        let answer = call(&router, uri, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let served = answer.list(page).len();
        assert_eq!(
            served, 1,
            "case {case:?}: the held id must select the one record naming it, got {served} \
             in {:?}",
            answer.body
        );
    }
}

/// `ListChangeSets` judges a `ChangeSetName` filter value against the form a change set
/// name takes: one outside it is refused quoting the value, and a well-formed name naming
/// no change set selects the empty page.
#[tokio::test]
async fn a_change_set_name_filter_value_is_judged_against_the_change_set_name_form() {
    let router = emulator();
    seed(&router).await;

    // Rows differing along how each misses `ChangeSetName`'s published form: the
    // characters it carries and its length.
    let refused: Vec<(String, String)> = vec![
        (
            "characters outside the published class".to_string(),
            "!!".to_string(),
        ),
        ("a hash".to_string(), "name#1".to_string()),
        ("a slash".to_string(), "name/1".to_string()),
        ("a comma".to_string(), "a,b".to_string()),
        (
            "a name one character past the published maximum".to_string(),
            "a".repeat(CHANGE_SET_NAME_MAX + 1),
        ),
    ];

    for (row, value) in refused {
        let case = format!("ListChangeSets: {row}");
        let answer = call(
            &router,
            "/ListChangeSets",
            json!({
                "Catalog": CATALOG,
                "FilterList": [{"Name": "ChangeSetName", "ValueList": [value.clone()]}],
            }),
        )
        .await;
        assert_eq!(
            answer.status, 422,
            "case {case:?}: a value that is not a change set name must be refused with 422, \
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
            message.contains(&value),
            "case {case:?}: the fault must quote the rejected value {value:?}, got {message:?}"
        );
        let served = answer.list("/ChangeSetSummaryList").len();
        assert_eq!(
            served, 0,
            "case {case:?}: a refused call serves no page, got {served} in {:?}",
            answer.body
        );
    }

    // Names that ARE change set names: the shortest and longest the length admits, each
    // character class the pattern admits, and the one the seed change set carries.
    let accepted: Vec<(String, String, usize)> = vec![
        ("a single character".to_string(), "a".to_string(), 0),
        (
            "every character class the pattern admits".to_string(),
            "a_b+c=d.e:f@g-1 2".to_string(),
            0,
        ),
        (
            "a name of the published maximum length".to_string(),
            "a".repeat(CHANGE_SET_NAME_MAX),
            0,
        ),
        (
            "the name the seed change set carries".to_string(),
            CHANGE_SET_NAME.to_string(),
            1,
        ),
    ];

    for (row, value, expected) in accepted {
        let case = format!("ListChangeSets: {row}");
        let answer = call(
            &router,
            "/ListChangeSets",
            json!({
                "Catalog": CATALOG,
                "FilterList": [{"Name": "ChangeSetName", "ValueList": [value.clone()]}],
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a change set name must be served, got {} with {:?}",
            answer.status, answer.body
        );
        let served = answer.list("/ChangeSetSummaryList").len();
        assert_eq!(
            served, expected,
            "case {case:?}: the filter must select {expected} change set(s) from state, \
             got {served} in {:?}",
            answer.body
        );
    }
}

/// `AssessmentTargetFilter$EntityId` names an entity through the wider `ResourceId`, and is
/// judged against the entity id form all the same: a value no entity id can be is refused
/// rather than selecting the empty page it would match.
#[tokio::test]
async fn an_assessment_target_entity_id_is_judged_against_the_entity_id_form() {
    let router = emulator();

    for (row, value) in not_entity_ids_within_resource_id() {
        let case = format!("ListAssessments: {row}");
        let answer = call(
            &router,
            "/ListAssessments",
            json!({
                "Catalog": CATALOG,
                "AssessmentTargetFilter": {"EntityId": value.clone()},
            }),
        )
        .await;
        assert_eq!(
            answer.status, 422,
            "case {case:?}: a value that is not an entity id must be refused with 422, \
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
            message.contains(&value) && message.contains(ENTITY_ID_PATTERN),
            "case {case:?}: the fault must quote the rejected value {value:?} and the form \
             required ({ENTITY_ID_PATTERN}), got {message:?}"
        );
    }

    for (row, value) in unheld_entity_ids_within_resource_id() {
        let case = format!("ListAssessments: {row}");
        let answer = call(
            &router,
            "/ListAssessments",
            json!({
                "Catalog": CATALOG,
                "AssessmentTargetFilter": {"EntityId": value.clone()},
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: an entity id naming nothing assessed must be served, \
             got {} with {:?}",
            answer.status, answer.body
        );
        let served = answer.list("/AssessmentSummaryList").len();
        assert_eq!(
            served, 0,
            "case {case:?}: an id nothing holds selects an empty page, got {served} in {:?}",
            answer.body
        );
    }
}

/// `Filter$Name` is "unique for each filter that you want to apply": a `FilterList` that
/// names one attribute twice is refused under `FilterList.Name`, while a list naming
/// distinct attributes is served and conjoined as before.
#[tokio::test]
async fn one_filter_list_names_each_attribute_once() {
    let router = emulator();
    let held = seed(&router).await;

    // (case, uri, request, page)
    let refused: Vec<(String, &str, Value, &str)> = vec![
        (
            "ListEntities: the EntityId attribute twice".to_string(),
            "/ListEntities",
            json!({
                "Catalog": CATALOG,
                "EntityType": ENTITY_TYPE,
                "FilterList": [
                    {"Name": "EntityId", "ValueList": [held.clone()]},
                    {"Name": "EntityId", "ValueList": ["ent-000000000000"]},
                ],
            }),
            "/EntitySummaryList",
        ),
        (
            "ListEntities: the EntityId attribute twice with the same value".to_string(),
            "/ListEntities",
            json!({
                "Catalog": CATALOG,
                "EntityType": ENTITY_TYPE,
                "FilterList": [
                    {"Name": "EntityId", "ValueList": [held.clone()]},
                    {"Name": "EntityId", "ValueList": [held.clone()]},
                ],
            }),
            "/EntitySummaryList",
        ),
        (
            "ListChangeSets: the Status attribute twice".to_string(),
            "/ListChangeSets",
            json!({
                "Catalog": CATALOG,
                "FilterList": [
                    {"Name": "Status", "ValueList": ["SUCCEEDED"]},
                    {"Name": "Status", "ValueList": ["FAILED"]},
                ],
            }),
            "/ChangeSetSummaryList",
        ),
        (
            "ListChangeSets: the ChangeSetName attribute twice".to_string(),
            "/ListChangeSets",
            json!({
                "Catalog": CATALOG,
                "FilterList": [
                    {"Name": "ChangeSetName", "ValueList": [CHANGE_SET_NAME]},
                    {"Name": "ChangeSetName", "ValueList": ["another"]},
                ],
            }),
            "/ChangeSetSummaryList",
        ),
        (
            "ListChangeSets: the same time bound twice".to_string(),
            "/ListChangeSets",
            json!({
                "Catalog": CATALOG,
                "FilterList": [
                    {"Name": "AfterStartTime", "ValueList": ["2000-01-01T00:00:00Z"]},
                    {"Name": "AfterStartTime", "ValueList": ["2001-01-01T00:00:00Z"]},
                ],
            }),
            "/ChangeSetSummaryList",
        ),
    ];

    for (case, uri, request, page) in refused {
        let answer = call(&router, uri, request).await;
        assert_eq!(
            answer.status, 422,
            "case {case:?}: an attribute named twice must be refused with 422, \
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
            message.contains("FilterList.Name"),
            "case {case:?}: the fault must name FilterList.Name, got {message:?}"
        );
        let served = answer.list(page).len();
        assert_eq!(
            served, 0,
            "case {case:?}: a refused call serves no page, got {served} in {:?}",
            answer.body
        );
    }

    // Distinct attributes in one list are still conjoined: both hold of the seed change
    // set, so it is the one record served.
    let accepted = call(
        &router,
        "/ListChangeSets",
        json!({
            "Catalog": CATALOG,
            "FilterList": [
                {"Name": "Status", "ValueList": ["SUCCEEDED"]},
                {"Name": "ChangeSetName", "ValueList": [CHANGE_SET_NAME]},
                {"Name": "EntityId", "ValueList": [held.clone()]},
            ],
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case \"ListChangeSets: three distinct attributes\": must answer 200, \
         got {} with {:?}",
        accepted.status, accepted.body
    );
    let served = accepted.list("/ChangeSetSummaryList").len();
    assert_eq!(
        served, 1,
        "case \"ListChangeSets: three distinct attributes\": the filters must conjoin to the \
         one change set they all hold of, got {served} in {:?}",
        accepted.body
    );
}

/// `Filter$ValueList` states per filter whether its values are unique: the `EntityId` list
/// is "a list of unique `EntityId`s", so a value repeated in it is refused, while the
/// `ChangeSetName` list is "a list of non-unique `ChangeSetName`s", so a repeated one is
/// served.
#[tokio::test]
async fn a_value_list_is_judged_by_the_uniqueness_its_filter_documents() {
    let router = emulator();
    let held = seed(&router).await;

    for ListUnderTest {
        list,
        uri,
        base,
        page,
    } in lists_filtering_on_entity_id()
    {
        // Rows differing along where the repetition falls and how many values surround it.
        let repeated: Vec<(String, Vec<String>, String)> = vec![
            (
                "the same held id twice".to_string(),
                vec![held.clone(), held.clone()],
                held.clone(),
            ),
            (
                "an unheld id twice".to_string(),
                vec!["ent-000000000000".to_string(), "ent-000000000000".to_string()],
                "ent-000000000000".to_string(),
            ),
            (
                "a repetition behind two distinct ids".to_string(),
                vec![
                    held.clone(),
                    "ent-000000000000".to_string(),
                    "ent-000000000000".to_string(),
                ],
                "ent-000000000000".to_string(),
            ),
            (
                "a repetition split by a distinct id".to_string(),
                vec![held.clone(), "ent-000000000000".to_string(), held.clone()],
                held.clone(),
            ),
        ];

        for (row, values, offending) in repeated {
            let case = format!("{list}: {row}");
            let mut request = base.clone();
            request["FilterList"] = json!([{"Name": "EntityId", "ValueList": values}]);
            let answer = call(&router, uri, request).await;
            assert_eq!(
                answer.status, 422,
                "case {case:?}: a repeated value in a list documented unique must be refused \
                 with 422, got {} with {:?}",
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
                message.contains("FilterList.ValueList") && message.contains(&offending),
                "case {case:?}: the fault must name FilterList.ValueList and quote the \
                 repeated value {offending:?}, got {message:?}"
            );
            let served = answer.list(page).len();
            assert_eq!(
                served, 0,
                "case {case:?}: a refused call serves no page, got {served} in {:?}",
                answer.body
            );
        }

        // Distinct ids in one `ValueList` are the accepted case of the same rule.
        let case = format!("{list}: two distinct ids");
        let mut request = base.clone();
        request["FilterList"] =
            json!([{"Name": "EntityId", "ValueList": [held.clone(), "ent-000000000000"]}]);
        let answer = call(&router, uri, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: distinct ids must be served, got {} with {:?}",
            answer.status, answer.body
        );
        let served = answer.list(page).len();
        assert_eq!(
            served, 1,
            "case {case:?}: the list must select the record naming either id, got {served} \
             in {:?}",
            answer.body
        );
    }

    // The filters the documentation does NOT state unique take a repeated value: the
    // `ChangeSetName` list is documented non-unique, and the `Status` list is documented
    // only as "a list of statuses".
    let non_unique: Vec<(&str, Value, usize)> = vec![
        (
            "ListChangeSets: the same change set name twice",
            json!([{"Name": "ChangeSetName", "ValueList": [CHANGE_SET_NAME, CHANGE_SET_NAME]}]),
            1,
        ),
        (
            "ListChangeSets: the same status twice",
            json!([{"Name": "Status", "ValueList": ["SUCCEEDED", "SUCCEEDED"]}]),
            1,
        ),
    ];

    for (case, filters, expected) in non_unique {
        let answer = call(
            &router,
            "/ListChangeSets",
            json!({"Catalog": CATALOG, "FilterList": filters}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a list the documentation states non-unique must be served, \
             got {} with {:?}",
            answer.status, answer.body
        );
        let served = answer.list("/ChangeSetSummaryList").len();
        assert_eq!(
            served, expected,
            "case {case:?}: the filter must select {expected} change set(s) from state, \
             got {served} in {:?}",
            answer.body
        );
    }
}
