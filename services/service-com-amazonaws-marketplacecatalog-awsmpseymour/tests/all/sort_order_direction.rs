// SPDX-License-Identifier: CC0-1.0

//! One `SortOrder` rule, shared by every list this service serves.
//!
//! `Sort$SortOrder` is documented as "The sorting order. Can be `ASCENDING` or
//! `DESCENDING`. The default value is `DESCENDING`" — so exactly the descending member of
//! the `SortOrder` enum reads a page from the greatest sort key down, every other value
//! reads it from the least key up, and an absent member reads it as the documented
//! default. Each list operation turns the value into that one direction, so the table
//! below drives the single decision through `ListChangeSets`, through `ListEntities`'
//! generic `Sort` and through its per-type `EntityTypeSort`, over every value the enum
//! accepts plus the absent member, and over each attribute a list sorts on.

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

/// The values the `SortOrder` enum accepts, from the model
/// (`smithy_query(query="shape", shape="SortOrder")` reports
/// `["ASCENDING", "DESCENDING"]`). Spelled out here so the test judges the service
/// against the model rather than against the service's own constant.
const SORT_ORDER_VALUES: &[&str] = &["ASCENDING", "DESCENDING"];

/// The value among [`SORT_ORDER_VALUES`] that reads a page from the greatest sort key
/// down, and the order `Sort$SortOrder` documents as its default when a request names
/// none.
const DESCENDING: &str = "DESCENDING";

/// The attributes `Sort$SortBy` publishes for `ListChangeSets` ("supported attributes
/// include `StartTime` and `EndTime`").
const CHANGE_SET_ATTRIBUTES: &[&str] = &["StartTime", "EndTime"];

/// The attributes `Sort$SortBy` publishes for `ListEntities` ("`LastModifiedDate`
/// (default) and `EntityId`").
const ENTITY_ATTRIBUTES: &[&str] = &["LastModifiedDate", "EntityId"];

/// The `AmiProductSortBy` members this test orders AMI products by, from
/// `smithy_query(query="shape", shape="AmiProductSortBy")`
/// (`["EntityId", "LastModifiedDate", "ProductTitle", "Visibility"]`): the title is the
/// one whose value every entity here carries distinctly.
const AMI_PRODUCT_ATTRIBUTE: &str = "ProductTitle";

/// The titles the entities under test are created with — distinct, and deliberately not
/// in creation order, so an ordering that merely reports insertion order fails.
const TITLES: &[&str] = &["charlie", "alpha", "delta", "bravo"];

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

/// The `Sort` / `EntityTypeSort` member a case sends: the order alone when the case names
/// no attribute, and nothing at all when it names neither.
fn sort_member(sort_by: Option<&str>, sort_order: Option<&str>) -> Option<Value> {
    let mut sort = json!({});
    if let Some(sort_by) = sort_by {
        sort["SortBy"] = json!(sort_by);
    }
    if let Some(sort_order) = sort_order {
        sort["SortOrder"] = json!(sort_order);
    }
    (!sort.as_object().expect("the sort member is an object").is_empty()).then_some(sort)
}

/// The order `served` is expected in: `keys` sorted up, or down when `sort_order` is the
/// descending member (which an absent `sort_order` is, by the documented default).
fn expected_order(keys: &[(String, String)], sort_order: Option<&str>) -> Vec<(String, String)> {
    let mut expected = keys.to_vec();
    expected.sort();
    if sort_order.unwrap_or(DESCENDING) == DESCENDING {
        expected.reverse();
    }
    expected
}

#[tokio::test]
async fn one_sort_order_rule_serves_every_list() {
    let router = emulator();
    for title in TITLES {
        let started = call(
            &router,
            "/StartChangeSet",
            json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": "CreateProduct",
                    "Entity": {"Type": "AmiProduct@1.0"},
                    "Details": json!({"ProductTitle": title}).to_string(),
                }],
            }),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {title:?}: StartChangeSet must answer 200, got {} with {:?}",
            started.status, started.body
        );
    }
    wait_for_success();

    // Every value the enum accepts, plus the absent member whose default the model
    // documents.
    let orders: Vec<Option<&str>> = SORT_ORDER_VALUES
        .iter()
        .map(|value| Some(*value))
        .chain([None])
        .collect();

    for order in orders {
        let spelling = order.unwrap_or("<absent>");

        // Copy 1's former home: `ListChangeSets` orders change sets by the attribute and
        // direction `Sort` names, with the change set id as the tie-break.
        for attribute in CHANGE_SET_ATTRIBUTES {
            let case = format!("ListChangeSets by {attribute} {spelling}");
            let mut request = json!({"Catalog": CATALOG});
            if let Some(sort) = sort_member(Some(attribute), order) {
                request["Sort"] = sort;
            }
            let answer = call(&router, "/ListChangeSets", request).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: ListChangeSets must answer 200, got {} with {:?}",
                answer.status, answer.body
            );
            let keys: Vec<(String, String)> = answer
                .list("/ChangeSetSummaryList")
                .iter()
                .map(|item| {
                    (
                        item[attribute].as_str().unwrap_or_default().to_string(),
                        item["ChangeSetId"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            assert_eq!(
                keys.len(),
                TITLES.len(),
                "case {case:?}: every change set must be reported, got {keys:?}"
            );
            let expected = expected_order(&keys, order);
            assert_eq!(
                keys, expected,
                "case {case:?}: the page must read ({attribute}, id) {spelling}, \
                 got {keys:?}"
            );
        }

        // Copy 2's former home, first spelling: `ListEntities`' generic `Sort`, with the
        // entity id as the tie-break behind it.
        for attribute in ENTITY_ATTRIBUTES {
            let case = format!("ListEntities by {attribute} {spelling}");
            let mut request = json!({"Catalog": CATALOG, "EntityType": "AmiProduct"});
            if let Some(sort) = sort_member(Some(attribute), order) {
                request["Sort"] = sort;
            }
            let answer = call(&router, "/ListEntities", request).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: ListEntities must answer 200, got {} with {:?}",
                answer.status, answer.body
            );
            let keys: Vec<(String, String)> = answer
                .list("/EntitySummaryList")
                .iter()
                .map(|item| {
                    (
                        item[attribute].as_str().unwrap_or_default().to_string(),
                        item["EntityId"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            assert_eq!(
                keys.len(),
                TITLES.len(),
                "case {case:?}: every entity must be reported, got {keys:?}"
            );
            let expected = expected_order(&keys, order);
            assert_eq!(
                keys, expected,
                "case {case:?}: the page must read ({attribute}, EntityId) {spelling}, \
                 got {keys:?}"
            );
        }

        // Copy 2's former home, second spelling: the per-type `EntityTypeSort`, which
        // orders on one entity type's own field and takes the same `SortOrder`. The
        // titles are distinct, so this primary ordering decides the whole page.
        let case = format!("ListEntities by AmiProductSort {AMI_PRODUCT_ATTRIBUTE} {spelling}");
        let mut request = json!({"Catalog": CATALOG, "EntityType": "AmiProduct"});
        if let Some(sort) = sort_member(Some(AMI_PRODUCT_ATTRIBUTE), order) {
            request["EntityTypeSort"] = json!({"AmiProductSort": sort});
        }
        let answer = call(&router, "/ListEntities", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: ListEntities must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let titles: Vec<String> = answer
            .list("/EntitySummaryList")
            .iter()
            .map(|item| {
                item.pointer("/AmiProductSummary/ProductTitle")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let mut expected: Vec<String> = TITLES.iter().map(|title| title.to_string()).collect();
        expected.sort();
        if order.unwrap_or(DESCENDING) == DESCENDING {
            expected.reverse();
        }
        assert_eq!(
            titles, expected,
            "case {case:?}: the page must read {AMI_PRODUCT_ATTRIBUTE} {spelling}, \
             got {titles:?}"
        );
    }
}

/// An order nobody asked for is the documented default, in every list and in both of
/// `ListEntities`' sort members: the absent member reads exactly as the descending one.
#[tokio::test]
async fn an_absent_sort_order_reads_as_the_documented_default() {
    let router = emulator();
    for title in TITLES {
        call(
            &router,
            "/StartChangeSet",
            json!({
                "Catalog": CATALOG,
                "ChangeSet": [{
                    "ChangeType": "CreateProduct",
                    "Entity": {"Type": "AmiProduct@1.0"},
                    "Details": json!({"ProductTitle": title}).to_string(),
                }],
            }),
        )
        .await;
    }
    wait_for_success();

    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "ListChangeSets",
            "/ListChangeSets",
            "/ChangeSetSummaryList",
            json!({"Catalog": CATALOG, "Sort": {"SortBy": CHANGE_SET_ATTRIBUTES[0]}}),
        ),
        (
            "ListEntities' generic Sort",
            "/ListEntities",
            "/EntitySummaryList",
            json!({
                "Catalog": CATALOG,
                "EntityType": "AmiProduct",
                "Sort": {"SortBy": ENTITY_ATTRIBUTES[0]},
            }),
        ),
        (
            "ListEntities' EntityTypeSort",
            "/ListEntities",
            "/EntitySummaryList",
            json!({
                "Catalog": CATALOG,
                "EntityType": "AmiProduct",
                "EntityTypeSort": {"AmiProductSort": {"SortBy": AMI_PRODUCT_ATTRIBUTE}},
            }),
        ),
    ];

    for (case, uri, pointer, mut request) in cases {
        let defaulted = call(&router, uri, request.clone()).await;
        let member = if request.get("EntityTypeSort").is_some() {
            "EntityTypeSort"
        } else {
            "Sort"
        };
        if member == "EntityTypeSort" {
            request["EntityTypeSort"]["AmiProductSort"]["SortOrder"] = json!(DESCENDING);
        } else {
            request["Sort"]["SortOrder"] = json!(DESCENDING);
        }
        let explicit = call(&router, uri, request).await;
        assert_eq!(
            defaulted.status, explicit.status,
            "case {case:?}: both calls must answer alike, got {} and {}",
            defaulted.status, explicit.status
        );
        let ids = |answer: &Answer| -> Vec<String> {
            answer
                .list(pointer)
                .iter()
                .map(|item| {
                    item.get("ChangeSetId")
                        .or_else(|| item.get("EntityId"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        };
        let served = ids(&defaulted);
        assert_eq!(
            served.len(),
            TITLES.len(),
            "case {case:?}: every record must be reported, got {served:?}"
        );
        assert_eq!(
            served,
            ids(&explicit),
            "case {case:?}: an absent SortOrder must read as {DESCENDING}, got {served:?} \
             against {:?}",
            ids(&explicit)
        );
    }
}
