// SPDX-License-Identifier: CC0-1.0

//! Only the spelling this service MINTS resumes a page.
//!
//! The page-token rule says a `NextToken` resumes a page only when this service issued
//! it for the list being called. A token is minted in exactly one way — the list's
//! prefix, a separator, and the offset as bare lowercase hexadecimal with no sign and no
//! leading zeros — so every other way of writing the same offset is a value this service
//! never issued and names no page of the list, exactly like a non-hexadecimal offset.
//!
//! The table below is that class: each paginated operation of this crate
//! (`ListChangeSets`, `ListEntities`, `ListAssessments`, `DescribeAssessment`, which page
//! four nouns under four prefixes) crossed with the offsets a token can carry and with
//! the respellings of a hexadecimal number that are not the minted one (a leading `+`,
//! one or many leading zeros, upper and mixed case digits). The modelled `NextToken`
//! pattern (`^[\w+=.:@\-\/]+$`, from
//! `smithy_query(query="operation", operation="DescribeAssessment")`) admits all of them,
//! so each really reaches the handler. Every row is driven through the router, and the
//! MINTED spelling of the same offset is asked for beside it, so a row proves the
//! refusal is about the spelling rather than the offset.

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

/// The entity type the change set below creates, and the one `ListEntities` pages.
const ENTITY_TYPE: &str = "AmiProduct";

/// The one fault a page token that names no page is refused with.
const PAGE_TOKEN_FAULT: &str = "[Requested page token is invalid.]";

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

/// Start one `Intent=APPLY` change set creating an entity of [`ENTITY_TYPE`] and
/// submitting one delivery option of it, so applying it produces a change set, an entity
/// and an assessment of several controls — one item for each of the four lists paged
/// here.
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

/// One paginated operation and how to page it.
struct Paginated {
    operation: &'static str,
    uri: &'static str,
    /// The request body with no `NextToken` in it.
    request: Value,
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
            prefix: CHANGE_SET_PAGE_PREFIX,
        },
        Paginated {
            operation: "ListEntities",
            uri: "/ListEntities",
            request: json!({"Catalog": CATALOG, "EntityType": ENTITY_TYPE}),
            prefix: ENTITY_PAGE_PREFIX,
        },
        Paginated {
            operation: "ListAssessments",
            uri: "/ListAssessments",
            request: json!({"Catalog": CATALOG}),
            prefix: ASSESSMENT_PAGE_PREFIX,
        },
        Paginated {
            operation: "DescribeAssessment",
            uri: "/DescribeAssessment",
            request: json!({"Catalog": CATALOG, "AssessmentIdentifier": assessment_id}),
            prefix: CONTROL_PAGE_PREFIX,
        },
    ]
}

/// Every token prefix this service issues.
const EVERY_PREFIX: [&str; 4] = [
    CHANGE_SET_PAGE_PREFIX,
    ENTITY_PAGE_PREFIX,
    ASSESSMENT_PAGE_PREFIX,
    CONTROL_PAGE_PREFIX,
];

/// The offsets a token can carry: zero, one, both sides of the hexadecimal digit
/// boundary (so a row has letters to change case of), and a wide one.
const OFFSETS: [usize; 6] = [0, 1, 10, 15, 16, 255];

/// How a client can respell the hexadecimal offset of a token this service issued: the
/// class the row stands for, and how it rewrites the minted digits.
type Respelling = (&'static str, fn(&str) -> String);

/// How a client can respell the hexadecimal offset of a token this service issued, each
/// a class `usize::from_str_radix` reads although the minting format string never writes
/// it.
fn respellings() -> Vec<Respelling> {
    vec![
        ("a leading plus", |digits| format!("+{digits}")),
        ("one leading zero", |digits| format!("0{digits}")),
        ("many leading zeros", |digits| format!("00000{digits}")),
        ("a leading zero and a plus", |digits| format!("+0{digits}")),
        ("uppercase hexadecimal digits", |digits| {
            digits.to_uppercase()
        }),
        ("mixed-case hexadecimal digits", |digits| {
            digits
                .char_indices()
                .map(|(at, digit)| {
                    if at % 2 == 0 {
                        digit.to_ascii_uppercase()
                    } else {
                        digit
                    }
                })
                .collect()
        }),
    ]
}

/// The tokens of the list paging with `prefix` that this service never minted because
/// they respell an offset it does mint: `(case, token, minted token of the same offset)`.
///
/// A respelling that happens to BE the minted spelling (upper-casing `1`, say) is not a
/// row: it is the token the service issued.
fn respelt_tokens(prefix: &str) -> Vec<(String, String, String)> {
    let mut rows = Vec::new();
    for offset in OFFSETS {
        let minted = encode_page_token(prefix, offset);
        let digits = format!("{offset:x}");
        for (case, respell) in respellings() {
            let respelt = respell(&digits);
            if respelt == digits {
                continue;
            }
            rows.push((
                format!("offset {offset} with {case}"),
                format!("{prefix}-{respelt}"),
                minted.clone(),
            ));
        }
    }
    rows
}

/// The store this suite pages, and the id of an assessment the change set produced.
async fn populated() -> (MultiServiceRouter, String) {
    let router = emulator();
    start_change_set(&router, "respelling probe").await;
    // The change set has certainly succeeded by now, so the entity and the assessment
    // applying it produced are observable.
    std::thread::sleep(CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400));

    let listed = call(
        &router,
        "POST",
        "/ListAssessments",
        Some(json!({"Catalog": CATALOG, "MaxResults": 100})),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "the assessments the change set produced must be listable, got {} with {:?}",
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

/// One page of `row`, resuming at `token`.
async fn page(router: &MultiServiceRouter, row: &Paginated, token: &str) -> Answer {
    let mut request = row.request.clone();
    request["NextToken"] = json!(token);
    call(router, "POST", row.uri, Some(request)).await
}

/// A `NextToken` that respells an offset this service does page is a token it never
/// issued, so every paginated operation refuses it with the one page-token fault — while
/// the minted spelling of that same offset is served.
#[tokio::test]
async fn a_respelt_offset_is_a_token_this_service_never_issued() {
    let (router, assessment_id) = populated().await;

    for row in paginated(&assessment_id) {
        for (case, respelt, minted) in respelt_tokens(row.prefix) {
            let refused = page(&router, &row, &respelt).await;
            assert!(
                (400..500).contains(&refused.status),
                "case {}/{case} ({respelt:?}): a token this service never minted must be \
                 refused as a client error, got {} with {:?}",
                row.operation,
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.error_code(),
                "ValidationException",
                "case {}/{case} ({respelt:?}): must answer ValidationException, got {:?}",
                row.operation,
                refused.body
            );
            assert_eq!(
                refused.fault(),
                PAGE_TOKEN_FAULT,
                "case {}/{case} ({respelt:?}): must answer the one page-token fault, got {:?}",
                row.operation,
                refused.fault()
            );

            // The same offset, written the way this service writes it: a page.
            let served = page(&router, &row, &minted).await;
            assert_eq!(
                served.status, 200,
                "case {}/{case} ({minted:?}): the minted spelling of the same offset must \
                 still be served, got {} with {:?}",
                row.operation, served.status, served.body
            );
        }
    }
}

/// The same rule read at the decision itself, for every list this service pages: the
/// minted spelling of an offset resumes at that offset, and every respelling of it names
/// no page.
#[test]
fn the_page_token_decision_accepts_only_the_minted_spelling() {
    for prefix in EVERY_PREFIX {
        for offset in OFFSETS {
            let minted = encode_page_token(prefix, offset);
            let decided = page_offset(prefix, Some(&minted)).unwrap_or_else(|fault| {
                panic!("case {prefix}/{minted:?}: a token this list issued must be served, got {fault:?}")
            });
            assert_eq!(
                decided, offset,
                "case {prefix}/{minted:?}: must resume at {offset}, got {decided}"
            );
        }

        for (case, respelt, minted) in respelt_tokens(prefix) {
            assert_ne!(
                respelt, minted,
                "case {prefix}/{case}: a respelling must differ from the minted token"
            );
            let fault = page_offset(prefix, Some(&respelt))
                .expect_err("a token this service never minted names no page");
            assert_eq!(
                fault.message.as_deref(),
                Some(PAGE_TOKEN_FAULT),
                "case {prefix}/{case} ({respelt:?}): must be the one page-token fault, got {:?}",
                fault.message
            );
            assert!(
                fault.validation_exception_field_list.is_none(),
                "case {prefix}/{case} ({respelt:?}): a page token is opaque, so no field list \
                 names it, got {:?}",
                fault.validation_exception_field_list
            );
        }
    }
}
