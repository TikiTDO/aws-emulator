// SPDX-License-Identifier: CC0-1.0

//! The one page-size rule every paginated operation of this service decides by, end to
//! end over HTTP.
//!
//! The rule: a `MaxResults` inside the range the operation's model declares is the
//! number of items the page holds (with a `NextToken` only while items are left over),
//! a request naming no `MaxResults` serves the page that operation's own member
//! documents as its default, and a size outside that range is refused with the modelled
//! `ValidationException` naming `MaxResults`.
//!
//! Every paginated operation of this crate is a row — `ListChangeSets`, `ListEntities`,
//! `ListAssessments` and `DescribeAssessment`, whose declared ranges differ (1..=20,
//! 1..=50 and 1..=100 twice) — and each is driven through the router, so a row that
//! judged a page size its own way would fail here. Nothing is seeded: the change sets,
//! entities, assessments and control results paged below are what the change sets
//! started here produced.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    page_size, CHANGE_SET_APPLYING, CHANGE_SET_PREPARING,
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

/// The schema version a change addresses [`ENTITY_TYPE`] at: `Change$Entity$Type` carries
/// an entity type at a version (`<EntityType>@<version>`), which is the form the service
/// publishes for that member.
const ENTITY_TYPE_AT_VERSION: &str = "AmiProduct@1.0";

/// How many change sets this suite starts — more than the smallest page it asks for, so
/// a page boundary is crossed, and fewer than any declared maximum or documented
/// default, so a request naming no `MaxResults` can be checked to serve them all.
const STARTED_CHANGE_SETS: usize = 3;

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

    /// The fault this answer carries, whether the service stated it (`Message`) or the
    /// boundary refused the request before the handler ran (`message`).
    fn fault(&self) -> String {
        let stated = self.text("/Message");
        if stated.is_empty() {
            self.text("/message")
        } else {
            stated
        }
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
                "Entity": {"Type": ENTITY_TYPE_AT_VERSION},
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

/// One paginated operation and how to page it: the request it is asked with, where its
/// page of items sits in the answer, and the largest page its own model declares.
struct Paginated {
    operation: &'static str,
    uri: &'static str,
    /// The request body with no `MaxResults` and no `NextToken` in it.
    request: Value,
    /// The JSON pointer of the member one page's items arrive in.
    items: &'static str,
    /// The maximum the operation's `MaxResults` `@range` declares — from the model itself
    /// (`smithy_query(query="operation", operation="ListChangeSets,ListEntities,\
    /// ListAssessments,DescribeAssessment")`: 1..=20, 1..=50, 1..=100, 1..=100).
    limit: i32,
    /// The page this operation serves when the request names no `MaxResults`: the
    /// `@default` its member declares where it has one, else the default its member's
    /// own documentation states (same `smithy_query` call, plus
    /// `smithy_query(query="doc", shape="<Request>", member="MaxResults")`: 20, 20, 20,
    /// 50).
    default: i32,
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
            limit: 20,
            default: 20,
        },
        Paginated {
            operation: "ListEntities",
            uri: "/ListEntities",
            request: json!({"Catalog": CATALOG, "EntityType": ENTITY_TYPE}),
            items: "/EntitySummaryList",
            limit: 50,
            default: 20,
        },
        Paginated {
            operation: "ListAssessments",
            uri: "/ListAssessments",
            request: json!({"Catalog": CATALOG}),
            items: "/AssessmentSummaryList",
            limit: 100,
            default: 20,
        },
        Paginated {
            operation: "DescribeAssessment",
            uri: "/DescribeAssessment",
            request: json!({"Catalog": CATALOG, "AssessmentIdentifier": assessment_id}),
            items: "/ControlAssessments",
            limit: 100,
            default: 50,
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
    call(router, "POST", row.uri, Some(request)).await
}

/// The store this suite pages: three applied change sets, and the id of an assessment
/// one of them produced.
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
        Some(json!({"Catalog": CATALOG, "MaxResults": 100})),
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

/// A `MaxResults` inside the declared range is the size of the page served, and the
/// `NextToken` is present exactly while items are left over — for every paginated
/// operation, whatever its range and whatever it pages.
///
/// The rows are the four paginated operations crossed with the page sizes the rule
/// generalises over: the smallest the range allows, one in between, exactly the number
/// of items there are, the operation's published maximum, and no `MaxResults` at all.
#[tokio::test]
async fn every_paginated_operation_serves_the_page_size_it_was_asked_for() {
    let (router, assessment_id) = populated().await;

    for row in paginated(&assessment_id) {
        // What there is to page, read as one page of the largest size the operation
        // allows — the rows below are the same items cut into pages.
        let whole = page(&router, &row, Some(row.limit), "").await;
        assert_eq!(
            whole.status, 200,
            "case {}/MaxResults={}: the published maximum must be served, got {} with {:?}",
            row.operation, row.limit, whole.status, whole.body
        );
        let all = whole.list(row.items);
        let total = all.len();
        assert!(
            total >= STARTED_CHANGE_SETS,
            "case {}: the applied change sets must leave at least {STARTED_CHANGE_SETS} \
             items to page, got {total}",
            row.operation
        );
        assert_eq!(
            whole.body.pointer("/NextToken"),
            None,
            "case {}/MaxResults={}: a page holding everything carries no NextToken, got {:?}",
            row.operation,
            row.limit,
            whole.body
        );

        let sizes: Vec<Option<i32>> = vec![
            Some(1),
            Some(2),
            Some(i32::try_from(total).expect("the item count fits in an i32")),
            Some(row.limit),
            None,
        ];
        for requested in sizes {
            // Paging the whole list at `requested` a page must yield every item once,
            // in the same order, in pages of exactly the size asked for until the last.
            let mut seen: Vec<Value> = Vec::new();
            let mut token = String::new();
            loop {
                let answer = page(&router, &row, requested, &token).await;
                assert_eq!(
                    answer.status, 200,
                    "case {}/MaxResults={requested:?}: a page size inside the range must be \
                     served, got {} with {:?}",
                    row.operation, answer.status, answer.body
                );
                let items = answer.list(row.items);
                let remaining = total - seen.len();
                let expected = usize::try_from(requested.unwrap_or(row.default))
                    .expect("a positive page size fits in usize")
                    .min(remaining);
                assert_eq!(
                    items.len(),
                    expected,
                    "case {}/MaxResults={requested:?}: a page must hold {expected} items, \
                     got {} with {:?}",
                    row.operation,
                    items.len(),
                    answer.body
                );
                seen.extend(items);
                token = answer.text("/NextToken");
                let left_over = total - seen.len();
                assert_eq!(
                    token.is_empty(),
                    left_over == 0,
                    "case {}/MaxResults={requested:?}: a NextToken is carried exactly while \
                     items are left over ({left_over} left), got {token:?}",
                    row.operation
                );
                if token.is_empty() {
                    break;
                }
            }
            assert_eq!(
                seen, all,
                "case {}/MaxResults={requested:?}: paging must serve every item once, in \
                 order, got {seen:?}",
                row.operation
            );
        }
    }
}

/// A `MaxResults` outside the declared range is refused with the modelled
/// `ValidationException` and its 422, by every paginated operation and whatever the
/// resources it pages — never served, never coerced into a legal page size, never the
/// protocol's bare 400, and never a 5xx.
///
/// The rows are the four paginated operations crossed with the ways a page size leaves
/// a range: zero, negative, one past the published maximum, and the largest a 32-bit
/// `MaxResults` can carry.
#[tokio::test]
async fn a_page_size_outside_the_declared_range_is_refused_the_same_way_everywhere() {
    let (router, assessment_id) = populated().await;

    for row in paginated(&assessment_id) {
        for requested in [0, -1, row.limit + 1, i32::MAX] {
            let answer = page(&router, &row, Some(requested), "").await;
            assert_eq!(
                answer.status, 422,
                "case {}/MaxResults={requested}: a page size outside the declared range must \
                 be refused with the modelled ValidationException's status, got {} with {:?}",
                row.operation, answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {}/MaxResults={requested}: must answer ValidationException, got {:?}",
                row.operation,
                answer.body
            );
            assert_eq!(
                answer.fault(),
                out_of_bounds_message(requested, row.limit),
                "case {}/MaxResults={requested}: the refusal must state the size asked for \
                 and the bounds the member declares, got {:?}",
                row.operation,
                answer.fault()
            );
        }
    }
}

/// The refusal an out-of-range `MaxResults` is answered with, spelled out here rather
/// than taken from the crate so a wrong implementation cannot make this pass: the size
/// the request asked for and the bounds the member's own `@range` declares.
fn out_of_bounds_message(requested: i32, limit: i32) -> String {
    format!(
        "[Provided max results value '{requested}' is out of bounds. It should be between \
         1 and {limit}.]"
    )
}

/// The page-size decision itself, read where the service makes it: the size served for
/// a `MaxResults` inside the declared range, the documented default page for a request
/// naming none, and the refusal built for one outside the range — stating the size asked
/// for and the bounds the member declares.
///
/// The rows are the four paginated operations, each with the range and the documented
/// default it publishes, crossed with the sizes the rule generalises over: both
/// published bounds, one in between, no member at all, and the three ways a size leaves
/// the range.
#[test]
fn the_page_size_rule_judges_every_operation_by_its_own_declared_range() {
    for row in paginated("assessment-1") {
        let limit = row.limit;
        for requested in [Some(1), Some(limit / 2), Some(limit), None] {
            let served = page_size(row.default, limit, requested).unwrap_or_else(|fault| {
                panic!(
                    "case {}/MaxResults={requested:?}: a page size inside the declared \
                     range must be served, got {fault:?}",
                    row.operation
                )
            });
            let expected = usize::try_from(requested.unwrap_or(row.default))
                .expect("a positive page size fits in usize");
            assert_eq!(
                served, expected,
                "case {}/MaxResults={requested:?}: must serve {expected} items a page, got \
                 {served}",
                row.operation
            );
        }

        for requested in [0, -1, limit + 1, i32::MAX] {
            let fault = page_size(row.default, limit, Some(requested))
                .expect_err("a page size outside the declared range is refused");
            assert_eq!(
                fault.message.as_deref(),
                Some(out_of_bounds_message(requested, limit).as_str()),
                "case {}/MaxResults={requested}: the refusal must state the size asked for \
                 and the bounds the member declares, got {:?}",
                row.operation,
                fault.message
            );
        }
    }
}
