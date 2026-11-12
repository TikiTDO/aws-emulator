// SPDX-License-Identifier: CC0-1.0

//! What a paginated operation of this service serves when the request names no
//! `MaxResults`, end to end over HTTP.
//!
//! The rule: a request that names no `MaxResults` is served the page size that
//! operation's own member publishes for an absent one — the `@default` where the member
//! declares one, else the default its documentation states — which is the member's
//! `@range` maximum only where the operation publishes the same number for both. The
//! maximum bounds a size the request ASKS for; it is not the size an absent member
//! asks for.
//!
//! Every paginated operation of this crate is a row — `ListChangeSets`, `ListEntities`,
//! `ListAssessments` and `DescribeAssessment` — and the pairs they publish differ:
//! `ListEntities` documents 20 against a maximum of 50, `ListAssessments` declares
//! `@default(20)` against 100, `DescribeAssessment` `@default(50)` against 100, and
//! `ListChangeSets` documents 20 against a maximum of 20, where the two coincide. The
//! store holds more items than the smaller defaults and fewer than the larger one, so
//! each row is checked both where the default cuts the list into pages and where it
//! holds all of it. Nothing is seeded: the change sets, entities, assessments and
//! control results paged below are what the change sets started here produced.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    CHANGE_SET_APPLYING, CHANGE_SET_PREPARING, DESCRIBE_ASSESSMENT_PAGE_DEFAULT,
    DESCRIBE_ASSESSMENT_PAGE_LIMIT, LIST_ASSESSMENTS_PAGE_DEFAULT, LIST_ASSESSMENTS_PAGE_LIMIT,
    LIST_CHANGE_SETS_PAGE_DEFAULT, LIST_CHANGE_SETS_PAGE_LIMIT, LIST_ENTITIES_PAGE_DEFAULT,
    LIST_ENTITIES_PAGE_LIMIT,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`),
/// which is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The entity type the change sets below create, and the one `ListEntities` pages.
const ENTITY_TYPE: &str = "AmiProduct";

/// How many change sets this suite starts, and so how many change sets, entities and
/// assessments there are to page: MORE than the smallest default any of these
/// operations publishes (20) and fewer than the largest maximum (100), so an absent
/// `MaxResults` can be told apart from both the whole list and the modelled maximum.
const STARTED_CHANGE_SETS: usize = 25;

/// How long after the start a change set has certainly reached `SUCCEEDED`, so the
/// entities and assessments applying its changes produced are observable.
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
    /// The string at `pointer`, or the empty string.
    fn text(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The array at `pointer`, or an empty one when the member is absent.
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

/// Start one `Intent=APPLY` change set that creates a new entity of [`ENTITY_TYPE`] and
/// submits one delivery option of it, so applying it produces an entity AND an
/// assessment of several controls.
async fn start_change_set(router: &MultiServiceRouter, title: &str) {
    let details = json!({
        "ProductTitle": title,
        "Version": {"VersionTitle": "1.0", "ReleaseNotes": "first release"},
        "DeliveryOptions": [{
            "Details": {
                "AmiDeliveryOptionDetails": {
                    "AmiSource": {
                        "AmiId": "ami-0abcdef1234567890",
                        "AccessRoleArn": "arn:aws:iam::123456789012:role/MarketplaceScan",
                        "UserName": "ec2-user",
                        "OperatingSystemName": "AMAZONLINUX",
                        "OperatingSystemVersion": "2023",
                    },
                    "RecommendedInstanceType": "t3.medium",
                    "SecurityGroups": [{
                        "IpProtocol": "tcp",
                        "IpRanges": ["10.0.0.0/16"],
                        "FromPort": 22,
                        "ToPort": 22,
                    }],
                    "UsageInstructions": "Connect over SSH and run /opt/product/start.",
                }
            }
        }],
    });
    let answer = call(
        router,
        "POST",
        "/StartChangeSet",
        Some(json!({
            "Catalog": CATALOG,
            "Intent": "APPLY",
            "ChangeSet": [{
                "ChangeType": "AddDeliveryOptions",
                "Entity": {"Type": format!("{ENTITY_TYPE}@1.0")},
                "Details": details.to_string(),
            }],
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {title:?}: starting the change set must be accepted, got {} with {:?}",
        answer.status, answer.body
    );
}

/// One paginated operation, the two numbers its `MaxResults` publishes, and how to page
/// it.
struct Paginated {
    operation: &'static str,
    uri: &'static str,
    /// The request body with no `MaxResults` and no `NextToken` in it.
    request: Value,
    /// The JSON pointer of the member one page's items arrive in.
    items: &'static str,
    /// The page served for an absent `MaxResults`, and the maximum one may ask for.
    /// Both come from the model: the `@range` maxima from
    /// `smithy_query(query="operation", operation="ListChangeSets,ListEntities,\
    /// ListAssessments,DescribeAssessment")` (20, 50, 100, 100), the defaults from the
    /// `@default` there (`ListAssessments` 20, `DescribeAssessment` 50) and otherwise
    /// from `smithy_query(query="doc", shape="<Request>", member="MaxResults")`
    /// ("By default, this value is 20" / "If a value isn't provided, the default value
    /// is 20").
    default: i32,
    limit: i32,
}

/// Every paginated operation of this service, with the assessment the control-level
/// reader pages filled in.
fn paginated(assessment_id: &str) -> Vec<Paginated> {
    vec![
        Paginated {
            operation: "ListChangeSets",
            uri: "/ListChangeSets",
            request: json!({"Catalog": CATALOG}),
            items: "/ChangeSetSummaryList",
            default: LIST_CHANGE_SETS_PAGE_DEFAULT,
            limit: LIST_CHANGE_SETS_PAGE_LIMIT,
        },
        Paginated {
            operation: "ListEntities",
            uri: "/ListEntities",
            request: json!({"Catalog": CATALOG, "EntityType": ENTITY_TYPE}),
            items: "/EntitySummaryList",
            default: LIST_ENTITIES_PAGE_DEFAULT,
            limit: LIST_ENTITIES_PAGE_LIMIT,
        },
        Paginated {
            operation: "ListAssessments",
            uri: "/ListAssessments",
            request: json!({"Catalog": CATALOG}),
            items: "/AssessmentSummaryList",
            default: LIST_ASSESSMENTS_PAGE_DEFAULT,
            limit: LIST_ASSESSMENTS_PAGE_LIMIT,
        },
        Paginated {
            operation: "DescribeAssessment",
            uri: "/DescribeAssessment",
            request: json!({"Catalog": CATALOG, "AssessmentIdentifier": assessment_id}),
            items: "/ControlAssessments",
            default: DESCRIBE_ASSESSMENT_PAGE_DEFAULT,
            limit: DESCRIBE_ASSESSMENT_PAGE_LIMIT,
        },
    ]
}

/// One page of `row`, asking for `max_results` and resuming at `token`.
async fn page(
    router: &MultiServiceRouter,
    row: &Paginated,
    max_results: Option<i32>,
    token: &str,
) -> Answer {
    let mut request = row.request.clone();
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if !token.is_empty() {
        request["NextToken"] = json!(token);
    }
    let answer = call(router, "POST", row.uri, Some(request)).await;
    assert_eq!(
        answer.status, 200,
        "case {}/MaxResults={max_results:?}: a page inside the modelled range must be \
         served, got {} with {:?}",
        row.operation, answer.status, answer.body
    );
    answer
}

/// Every item `row` has to page, read at the largest page it allows, in the order it
/// serves them.
async fn everything(router: &MultiServiceRouter, row: &Paginated) -> Vec<Value> {
    let mut all: Vec<Value> = Vec::new();
    let mut token = String::new();
    loop {
        let answer = page(router, row, Some(row.limit), &token).await;
        all.extend(answer.list(row.items));
        token = answer.text("/NextToken");
        if token.is_empty() {
            break;
        }
    }
    all
}

/// The store this suite pages: [`STARTED_CHANGE_SETS`] applied change sets, and the id
/// of an assessment one of them produced.
async fn populated() -> (MultiServiceRouter, String) {
    let router = emulator();
    for index in 0..STARTED_CHANGE_SETS {
        start_change_set(&router, &format!("product {index}")).await;
    }
    wait_for_success();

    let listed = call(
        &router,
        "POST",
        "/ListAssessments",
        Some(json!({"Catalog": CATALOG, "MaxResults": LIST_ASSESSMENTS_PAGE_LIMIT})),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "the assessments the change sets produced must be listable, got {} with {:?}",
        listed.status, listed.body
    );
    let assessment_id = listed
        .list("/AssessmentSummaryList")
        .first()
        .and_then(|summary| summary["AssessmentId"].as_str().map(str::to_string))
        .unwrap_or_else(|| {
            panic!(
                "an applied change set must produce an assessment to page, got {:?}",
                listed.body
            )
        });
    (router, assessment_id)
}

/// A request naming no `MaxResults` holds exactly the page its member publishes for an
/// absent one — never the whole list and never the modelled maximum — and carries a
/// `NextToken` exactly while items are left over.
///
/// The rows are the four paginated operations, whose published (default, maximum) pairs
/// differ: (20, 20), (20, 50), (20, 100) and (50, 100). With more than 20 and fewer than
/// 50 items stored, the first three rows exercise a default that cuts the list and the
/// last a default that holds all of it.
#[tokio::test]
async fn an_absent_max_results_serves_the_page_the_member_publishes() {
    let (router, assessment_id) = populated().await;

    for row in paginated(&assessment_id) {
        let all = everything(&router, &row).await;
        let total = all.len();
        assert!(
            total > 0,
            "case {}: the applied change sets must leave items to page, got none",
            row.operation
        );

        let expected = usize::try_from(row.default)
            .expect("a published page default is positive")
            .min(total);
        let first = page(&router, &row, None, "").await;
        let served = first.list(row.items);
        assert_eq!(
            served.len(),
            expected,
            "case {}: a request naming no MaxResults must serve {expected} of {total} items \
             (its published default is {}, its maximum {}), got {}",
            row.operation,
            row.default,
            row.limit,
            served.len()
        );
        assert_eq!(
            served,
            all[..expected],
            "case {}: the page served for an absent MaxResults must be the first {expected} \
             items of the listing, got {served:?}",
            row.operation
        );

        let token = first.text("/NextToken");
        assert_eq!(
            token.is_empty(),
            expected == total,
            "case {}: a NextToken is carried exactly while items are left over ({} left), \
             got {token:?}",
            row.operation,
            total - expected
        );

        // Paging on with no `MaxResults` either must walk the rest of the listing in
        // pages of the same published default, once each and in order.
        let mut seen = served;
        let mut token = token;
        while !token.is_empty() {
            let answer = page(&router, &row, None, &token).await;
            let items = answer.list(row.items);
            let remaining = total - seen.len();
            let wanted = usize::try_from(row.default)
                .expect("a published page default is positive")
                .min(remaining);
            assert_eq!(
                items.len(),
                wanted,
                "case {}: a resumed page with no MaxResults must hold {wanted} items, got {}",
                row.operation,
                items.len()
            );
            seen.extend(items);
            token = answer.text("/NextToken");
        }
        assert_eq!(
            seen, all,
            "case {}: paging with no MaxResults must serve every item once, in order, got \
             {seen:?}",
            row.operation
        );
    }
}

/// Where an operation's maximum is larger than the default its member publishes, asking
/// for the maximum serves MORE than an absent `MaxResults` does: the maximum bounds what
/// a request may ask for and is not what an absent member asks for.
///
/// The rows are the paginated operations whose two numbers differ (`ListEntities` 20 of
/// 50, `ListAssessments` 20 of 100, `DescribeAssessment` 50 of 100) and the one where
/// they coincide (`ListChangeSets`, 20 of 20), where the two answers must instead match.
/// Each row is judged against what it actually has to page, so the operation whose
/// listing is shorter than its default is held to serving all of it.
#[tokio::test]
async fn the_published_maximum_is_not_the_page_an_absent_max_results_asks_for() {
    let (router, assessment_id) = populated().await;

    for row in paginated(&assessment_id) {
        let total = everything(&router, &row).await.len();
        let absent = page(&router, &row, None, "").await.list(row.items).len();
        let at_maximum = page(&router, &row, Some(row.limit), "")
            .await
            .list(row.items)
            .len();

        let expected_absent = usize::try_from(row.default)
            .expect("a published page default is positive")
            .min(total);
        let expected_at_maximum = usize::try_from(row.limit)
            .expect("a published page maximum is positive")
            .min(total);
        assert_eq!(
            absent, expected_absent,
            "case {}: an absent MaxResults serves {expected_absent} of {total} items, got \
             {absent}",
            row.operation
        );
        assert_eq!(
            at_maximum, expected_at_maximum,
            "case {}: MaxResults={} serves {expected_at_maximum} of {total} items, got \
             {at_maximum}",
            row.operation, row.limit
        );
        assert_eq!(
            absent < at_maximum,
            expected_absent < expected_at_maximum,
            "case {}: a default of {} against a maximum of {} over {total} items must serve \
             {expected_absent} against {expected_at_maximum}, got {absent} against \
             {at_maximum}",
            row.operation,
            row.default,
            row.limit
        );
    }
}
