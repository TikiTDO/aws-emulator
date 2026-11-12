// SPDX-License-Identifier: CC0-1.0

//! The one page-TOKEN rule every paginated operation of this service decides by, end to
//! end over HTTP and at the decision itself.
//!
//! The rule: a request naming no `NextToken` asks for the first page, a request carrying
//! a token this service issued for the list being called resumes at the page that token
//! names, and anything else — a value outside the modelled `NextToken` syntax, a
//! well-formed value naming no page, or a token another list of this service issued —
//! names no page of this list and is refused with the one modelled page-token
//! `ValidationException`.
//!
//! Every paginated operation of this crate is a row — `ListChangeSets`, `ListEntities`,
//! `ListAssessments` and `DescribeAssessment`, which page four different nouns under four
//! different token prefixes — and each is driven through the router, so an operation that
//! judged a page token its own way (serving one it should refuse, or refusing with a
//! different fault) would fail here. Nothing is seeded: the change sets, entities,
//! assessments and control results paged below are what the change sets started here
//! produced.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    encode_page_token, page_offset, ASSESSMENT_PAGE_PREFIX, CHANGE_SET_APPLYING,
    CHANGE_SET_PAGE_PREFIX, CHANGE_SET_PREPARING, CONTROL_PAGE_PREFIX, ENTITY_PAGE_PREFIX,
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

/// How many change sets this suite starts — more than one, so every list it pages has a
/// second page to resume at.
const STARTED_CHANGE_SETS: usize = 3;

/// The one fault a page token that names no page is refused with.
const PAGE_TOKEN_FAULT: &str = "[Requested page token is invalid.]";

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

/// One paginated operation and how to page it: the request it is asked with, where its
/// page of items sits in the answer, and the `NextToken` prefix its own list pages with.
struct Paginated {
    operation: &'static str,
    uri: &'static str,
    /// The request body with no `MaxResults` and no `NextToken` in it.
    request: Value,
    /// The JSON pointer of the member one page's items arrive in.
    items: &'static str,
    /// The prefix this list's own tokens carry.
    prefix: &'static str,
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
            prefix: CHANGE_SET_PAGE_PREFIX,
        },
        Paginated {
            operation: "ListEntities",
            uri: "/ListEntities",
            request: json!({"Catalog": CATALOG, "EntityType": ENTITY_TYPE}),
            items: "/EntitySummaryList",
            prefix: ENTITY_PAGE_PREFIX,
        },
        Paginated {
            operation: "ListAssessments",
            uri: "/ListAssessments",
            request: json!({"Catalog": CATALOG}),
            items: "/AssessmentSummaryList",
            prefix: ASSESSMENT_PAGE_PREFIX,
        },
        Paginated {
            operation: "DescribeAssessment",
            uri: "/DescribeAssessment",
            request: json!({"Catalog": CATALOG, "AssessmentIdentifier": assessment_id}),
            items: "/ControlAssessments",
            prefix: CONTROL_PAGE_PREFIX,
        },
    ]
}

/// Every token prefix this service issues, so a row can be handed the ones that belong
/// to the OTHER lists.
const EVERY_PREFIX: [&str; 4] = [
    CHANGE_SET_PAGE_PREFIX,
    ENTITY_PAGE_PREFIX,
    ASSESSMENT_PAGE_PREFIX,
    CONTROL_PAGE_PREFIX,
];

/// The ways a `NextToken` can name no page of the list paging with `prefix`, each with
/// the case name describing the class it stands for.
fn tokens_naming_no_page(prefix: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = vec![
        (
            "outside the modelled token syntax".to_string(),
            "not a valid token !!".to_string(),
        ),
        ("no prefix at all".to_string(), "0".to_string()),
        (
            "the prefix without its separator".to_string(),
            prefix.to_string(),
        ),
        (
            "the prefix with the wrong separator".to_string(),
            format!("{prefix}_0"),
        ),
        (
            "an offset that is not hexadecimal".to_string(),
            format!("{prefix}-zz"),
        ),
        (
            "an offset that is empty".to_string(),
            format!("{prefix}-"),
        ),
        (
            "an offset written in 0x notation".to_string(),
            format!("{prefix}-0x10"),
        ),
        (
            "an offset wider than the counter".to_string(),
            format!("{prefix}-{}", "f".repeat(40)),
        ),
    ];
    for other in EVERY_PREFIX {
        if other != prefix {
            rows.push((
                format!("a token the {other} list issued"),
                encode_page_token(other, 1),
            ));
        }
    }
    rows
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

/// One page of `row`, asking for `max_results` and resuming at `token`.
async fn page(
    router: &MultiServiceRouter,
    row: &Paginated,
    max_results: Option<i32>,
    token: Option<&str>,
) -> Answer {
    let mut request = row.request.clone();
    if let Some(max_results) = max_results {
        request["MaxResults"] = json!(max_results);
    }
    if let Some(token) = token {
        request["NextToken"] = json!(token);
    }
    call(router, "POST", row.uri, Some(request)).await
}

/// A request naming no `NextToken` serves the first page, and a token the operation
/// itself issued resumes exactly where that page left off — for every paginated
/// operation, whatever noun it pages.
#[tokio::test]
async fn every_paginated_operation_resumes_at_a_token_it_issued() {
    let (router, assessment_id) = populated().await;

    for row in paginated(&assessment_id) {
        // No `MaxResults`: the page the operation's own member documents for an absent
        // one, which is larger than this suite's list, so the whole list arrives in one
        // answer.
        let whole = page(&router, &row, None, None).await;
        assert_eq!(
            whole.status, 200,
            "case {}: a request naming no NextToken must serve the first page, got {} with \
             {:?}",
            row.operation, whole.status, whole.body
        );
        let all = whole.list(row.items);
        assert!(
            all.len() >= 2,
            "case {}: this suite needs at least two items to resume across, got {}",
            row.operation,
            all.len()
        );

        // One item a page, walked by the tokens the operation itself hands back: every
        // item once, in the order the unpaged answer gave them.
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let answer = page(&router, &row, Some(1), token.as_deref()).await;
            assert_eq!(
                answer.status, 200,
                "case {}/NextToken={token:?}: a token this operation issued must be served, \
                 got {} with {:?}",
                row.operation, answer.status, answer.body
            );
            let items = answer.list(row.items);
            assert_eq!(
                items.len(),
                1,
                "case {}/NextToken={token:?}: one item was asked for, got {} with {:?}",
                row.operation,
                items.len(),
                answer.body
            );
            seen.extend(items);
            let next = answer.text("/NextToken");
            if next.is_empty() {
                break;
            }
            assert!(
                next.starts_with(row.prefix),
                "case {}: the token must name the list it pages ({}), got {next:?}",
                row.operation,
                row.prefix
            );
            token = Some(next);
        }
        assert_eq!(
            seen, all,
            "case {}: resuming at the issued tokens must serve every item once, in order, \
             got {seen:?}",
            row.operation
        );

        // A well-formed token of this list that points past its end decodes to a page —
        // an empty one — rather than being refused: only a token naming no page at all is
        // a fault.
        let past_end = encode_page_token(row.prefix, all.len() + 7);
        let answer = page(&router, &row, None, Some(&past_end)).await;
        assert_eq!(
            answer.status, 200,
            "case {}/NextToken={past_end:?}: a token of this list past its end must serve an \
             empty page, got {} with {:?}",
            row.operation, answer.status, answer.body
        );
        assert!(
            answer.list(row.items).is_empty(),
            "case {}/NextToken={past_end:?}: the page past the end must hold nothing, got \
             {:?}",
            row.operation,
            answer.list(row.items)
        );
    }
}

/// A `NextToken` that names no page of the list being called is refused with the one
/// modelled page-token fault — by every paginated operation, for every way a token can
/// fail to name a page.
///
/// The rows are the four paginated operations crossed with the classes of value that
/// name no page: outside the token syntax, no prefix, the prefix without or with the
/// wrong separator, a non-hexadecimal, empty, `0x`-prefixed or over-wide offset, and a
/// token each of the OTHER three lists issued.
#[tokio::test]
async fn a_page_token_naming_no_page_is_refused_the_same_way_everywhere() {
    let (router, assessment_id) = populated().await;

    for row in paginated(&assessment_id) {
        for (case, token) in tokens_naming_no_page(row.prefix) {
            let answer = page(&router, &row, None, Some(&token)).await;
            assert!(
                (400..500).contains(&answer.status),
                "case {}/{case} ({token:?}): a token naming no page must be refused as a \
                 client error, got {} with {:?}",
                row.operation,
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {}/{case} ({token:?}): must answer ValidationException, got {:?}",
                row.operation,
                answer.body
            );
            assert_eq!(
                answer.fault(),
                PAGE_TOKEN_FAULT,
                "case {}/{case} ({token:?}): must answer the one page-token fault, got {:?}",
                row.operation,
                answer.fault()
            );
        }
    }
}

/// The page-token decision itself, read where the service makes it: no token is the
/// first page, a token this service issued for the list asked about is the offset it
/// encodes, and anything else is the one page-token refusal.
///
/// The rows are the four token prefixes crossed with the offsets a token can carry
/// (zero, one, both sides of the hexadecimal digit boundary, and a large one) and with
/// every class of value that names no page.
#[test]
fn the_page_token_rule_judges_every_list_by_its_own_prefix() {
    for prefix in EVERY_PREFIX {
        let first = page_offset(prefix, None)
            .unwrap_or_else(|fault| panic!("case {prefix}/no NextToken: must be served, got {fault:?}"));
        assert_eq!(
            first, 0,
            "case {prefix}/no NextToken: must ask for the first page, got {first}"
        );

        for offset in [0_usize, 1, 15, 16, 255, 4_096, 1_234_567] {
            let token = encode_page_token(prefix, offset);
            let decided = page_offset(prefix, Some(&token)).unwrap_or_else(|fault| {
                panic!("case {prefix}/{token:?}: a token this list issued must be served, got {fault:?}")
            });
            assert_eq!(
                decided, offset,
                "case {prefix}/{token:?}: must resume at {offset}, got {decided}"
            );
        }

        for (case, token) in tokens_naming_no_page(prefix) {
            let fault = page_offset(prefix, Some(&token))
                .expect_err("a token naming no page of this list is refused");
            assert_eq!(
                fault.message.as_deref(),
                Some(PAGE_TOKEN_FAULT),
                "case {prefix}/{case} ({token:?}): must be the one page-token fault, got {:?}",
                fault.message
            );
            assert!(
                fault.validation_exception_field_list.is_none(),
                "case {prefix}/{case} ({token:?}): a page token is opaque, so no field list \
                 names it, got {:?}",
                fault.validation_exception_field_list
            );
        }
    }
}
