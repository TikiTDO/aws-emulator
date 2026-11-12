// SPDX-License-Identifier: CC0-1.0

//! A `NextToken` names a page only when it is, character for character, the token this
//! service minted for the list being paged.
//!
//! The page-token rule has two halves, and both are invariants over a whole class of
//! values rather than over any particular token:
//!
//! * round trip — the token this service mints for an offset of a list resumes that
//!   list at exactly that offset;
//! * refusal outside the class — every OTHER way of writing that same offset (a sign,
//!   any number of leading zeros, any mixture of upper and lower case hexadecimal
//!   digits, `0x` notation, a digit outside base 16, another separator, another list's
//!   prefix) is a value this service never issued, so it names no page and is the one
//!   modelled page-token fault.
//!
//! This suite ENUMERATES that class instead of listing a few members of it: for each of
//! the four lists this crate pages and each offset magnitude a token can carry, it
//! builds every case mask over the offset's hexadecimal digits, every leading-zero
//! padding up to five, both signs of the sign class, every separator the modelled token
//! syntax admits in place of the minted one, a non-hexadecimal digit at every position,
//! and the same offset under each of the other three lists' prefixes. It is therefore
//! deterministic: no seed, no sampling, nothing on disk.
//!
//! Every generated token stays inside the modelled `NextToken` syntax
//! (`^[\w+=.:@\-\/]+$`, from `smithy_query(query="operation",
//! operation="ListChangeSets,ListEntities,ListAssessments,DescribeAssessment")`) — each
//! case asserts that itself — so the request really reaches the handler and is judged by
//! the page-token decision rather than refused at the protocol boundary.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::state::{
    encode_page_token, page_offset, ASSESSMENT_PAGE_PREFIX, CHANGE_SET_APPLYING,
    CHANGE_SET_PAGE_PREFIX, CHANGE_SET_PREPARING, CONTROL_PAGE_PREFIX, ENTITY_PAGE_PREFIX,
    LIST_ASSESSMENTS_PAGE_LIMIT, LIST_CHANGE_SETS_PAGE_LIMIT, LIST_ENTITIES_PAGE_LIMIT,
};
use service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The catalog every operation's `Catalog` member documents as its fixed value.
const CATALOG: &str = "AWSMarketplace";

/// The entity type the change set below creates, and the one `ListEntities` pages.
const ENTITY_TYPE: &str = "AmiProduct";

/// A SigV4 credential scope naming this service's signing name (`aws-marketplace`),
/// which is what routes a request here.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The one fault a page token that names no page is refused with.
const PAGE_TOKEN_FAULT: &str = "[Requested page token is invalid.]";

/// Every token prefix this service issues, so a generated case can be handed the prefix
/// of ANOTHER list too.
const EVERY_PREFIX: [&str; 4] = [
    CHANGE_SET_PAGE_PREFIX,
    ENTITY_PAGE_PREFIX,
    ASSESSMENT_PAGE_PREFIX,
    CONTROL_PAGE_PREFIX,
];

/// The offsets the class is enumerated over: zero, one, both sides of the hexadecimal
/// digit boundary, a two-digit offset whose digits are all letters, and three wider
/// ones, so a case mask has something to flip at several widths.
const OFFSETS: [usize; 9] = [0, 1, 10, 15, 16, 255, 4_096, 1_048_575, 1_234_567];

/// Every character the modelled `NextToken` pattern (`^[\w+=.:@\-\/]+$`) admits between
/// the prefix and the digits other than the `-` the minted token carries.
const OTHER_SEPARATORS: [char; 6] = ['_', '=', '.', ':', '@', '/'];

/// How many leading zeros a padded respelling is enumerated up to.
const MOST_LEADING_ZEROS: usize = 5;

/// The widest offset whose case masks are enumerated exhaustively; a wider one is masked
/// at every single digit instead, so the enumeration stays linear in the digit count.
const WIDEST_EXHAUSTIVE_MASK: usize = 6;

/// `digits` with the digit at each position named by a bit of `mask` upper-cased.
fn case_mask(digits: &str, mask: u32) -> String {
    digits
        .char_indices()
        .map(|(at, digit)| {
            if mask & (1 << at) == 0 {
                digit
            } else {
                digit.to_ascii_uppercase()
            }
        })
        .collect()
}

/// Every way of upper-casing a subset of `digits` that differs from `digits` itself.
///
/// Exhaustive while the digit count is small enough for that to stay bounded
/// ([`WIDEST_EXHAUSTIVE_MASK`]); beyond it, one case per digit position plus the
/// all-upper-case mask.
fn case_masks(digits: &str) -> Vec<String> {
    let width = digits.chars().count();
    let masks: Vec<u32> = if width <= WIDEST_EXHAUSTIVE_MASK {
        (0..(1_u32 << width)).collect()
    } else {
        (0..width)
            .map(|at| 1_u32 << at)
            .chain(std::iter::once(u32::MAX))
            .collect()
    };
    masks
        .into_iter()
        .map(|mask| case_mask(digits, mask))
        .filter(|masked| masked != digits)
        .collect()
}

/// Every value this service never minted for the list paging with `prefix` although it
/// carries `offset`: `(the class it stands for, the token)`.
///
/// This is the whole respelling class of one minted token, which is what the rule is
/// about: the minted token is the only member of it this service ever issued.
fn values_never_minted(prefix: &str, offset: usize) -> Vec<(String, String)> {
    let digits = format!("{offset:x}");
    let minted = encode_page_token(prefix, offset);
    let mut rows: Vec<(String, String)> = Vec::new();

    for masked in case_masks(&digits) {
        rows.push((
            format!("the case mask {masked}"),
            format!("{prefix}-{masked}"),
        ));
    }
    rows.push(("a leading plus".to_string(), format!("{prefix}-+{digits}")));
    for zeros in 1..=MOST_LEADING_ZEROS {
        rows.push((
            format!("{zeros} leading zero(s)"),
            format!("{prefix}-{}{digits}", "0".repeat(zeros)),
        ));
        rows.push((
            format!("a leading plus and {zeros} leading zero(s)"),
            format!("{prefix}-+{}{digits}", "0".repeat(zeros)),
        ));
    }
    rows.push(("0x notation".to_string(), format!("{prefix}-0x{digits}")));
    // A letter outside base 16, so the respelling cannot be read as a number at all —
    // tried at every position, including both ends.
    for at in 0..=digits.len() {
        rows.push((
            format!("a digit outside base 16 at {at}"),
            format!("{prefix}-{}g{}", &digits[..at], &digits[at..]),
        ));
    }
    for separator in OTHER_SEPARATORS {
        rows.push((
            format!("the separator {separator:?}"),
            format!("{prefix}{separator}{digits}"),
        ));
    }
    rows.push((
        "no separator at all".to_string(),
        format!("{prefix}{digits}"),
    ));
    for other in EVERY_PREFIX {
        if other != prefix {
            rows.push((
                format!("the prefix of the {other} list"),
                encode_page_token(other, offset),
            ));
        }
    }

    rows.retain(|(_, token)| *token != minted);
    rows
}

/// Whether `token` is inside the modelled `NextToken` syntax, so a request carrying it
/// reaches the handler: `^[\w+=.:@\-\/]+$`.
fn inside_the_modelled_syntax(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|at| at.is_ascii_alphanumeric() || "_+=.:@-/".contains(at))
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

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
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

/// Start one `Intent=APPLY` change set creating an entity of [`ENTITY_TYPE`], so the
/// lists paged here have something to resume across.
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
    /// The request body with no `NextToken` and no `MaxResults` in it.
    request: Value,
    /// The JSON pointer of the member one page's items arrive in.
    items: &'static str,
    /// The prefix this list's own tokens carry.
    prefix: &'static str,
    /// The largest page this operation's `MaxResults` admits, so one request reads the
    /// whole of this suite's list whatever the operation's default page is.
    limit: i32,
}

/// The paginated operations a page of which needs no identifier in the request, so one
/// store serves every row.
fn paginated() -> Vec<Paginated> {
    vec![
        Paginated {
            operation: "ListChangeSets",
            uri: "/ListChangeSets",
            request: json!({"Catalog": CATALOG}),
            items: "/ChangeSetSummaryList",
            prefix: CHANGE_SET_PAGE_PREFIX,
            limit: LIST_CHANGE_SETS_PAGE_LIMIT,
        },
        Paginated {
            operation: "ListEntities",
            uri: "/ListEntities",
            request: json!({"Catalog": CATALOG, "EntityType": ENTITY_TYPE}),
            items: "/EntitySummaryList",
            prefix: ENTITY_PAGE_PREFIX,
            limit: LIST_ENTITIES_PAGE_LIMIT,
        },
        Paginated {
            operation: "ListAssessments",
            uri: "/ListAssessments",
            request: json!({"Catalog": CATALOG}),
            items: "/AssessmentSummaryList",
            prefix: ASSESSMENT_PAGE_PREFIX,
            limit: LIST_ASSESSMENTS_PAGE_LIMIT,
        },
    ]
}

/// One page of `row`, resuming at `token` and asking for the largest page this operation
/// serves.
async fn page(router: &MultiServiceRouter, row: &Paginated, token: &str) -> Answer {
    let mut request = row.request.clone();
    request["MaxResults"] = json!(row.limit);
    request["NextToken"] = json!(token);
    call(router, "POST", row.uri, Some(request)).await
}

/// The page-token decision every paginated operation of this service makes, over the
/// whole respelling class of every minted token: the minted token of an offset resumes
/// at that offset, and no other spelling of it names a page.
#[test]
fn only_the_minted_token_of_an_offset_names_a_page() {
    for prefix in EVERY_PREFIX {
        for offset in OFFSETS {
            let minted = encode_page_token(prefix, offset);
            assert!(
                inside_the_modelled_syntax(&minted),
                "case {prefix}/{offset}: the minted token must be inside the modelled NextToken \
                 syntax, got {minted:?}"
            );
            let decided = page_offset(prefix, Some(&minted)).unwrap_or_else(|fault| {
                panic!(
                    "case {prefix}/{minted:?}: a token this list issued must be served, got \
                     {fault:?}"
                )
            });
            assert_eq!(
                decided, offset,
                "case {prefix}/{minted:?}: must resume at {offset}, got {decided}"
            );

            let respellings = values_never_minted(prefix, offset);
            assert!(
                !respellings.is_empty(),
                "case {prefix}/{offset}: the respelling class must have members to check"
            );
            for (class, token) in respellings {
                assert!(
                    inside_the_modelled_syntax(&token),
                    "case {prefix}/{offset}/{class}: the generated token must be inside the \
                     modelled NextToken syntax, got {token:?}"
                );
                let fault = match page_offset(prefix, Some(&token)) {
                    Err(fault) => fault,
                    Ok(resumed) => panic!(
                        "case {prefix}/{offset}/{class} ({token:?}): a value this service never \
                         minted names no page, yet it resumed at offset {resumed}"
                    ),
                };
                assert_eq!(
                    fault.message.as_deref(),
                    Some(PAGE_TOKEN_FAULT),
                    "case {prefix}/{offset}/{class} ({token:?}): must be the one page-token fault, \
                     got {:?}",
                    fault.message
                );
                assert!(
                    fault.validation_exception_field_list.is_none(),
                    "case {prefix}/{offset}/{class} ({token:?}): a page token is opaque, so no \
                     field list names it, got {:?}",
                    fault.validation_exception_field_list
                );
            }
        }
    }
}

/// The same class over the wire: every respelling of a token of the first pages is
/// refused by every paginated operation with the one page-token fault, while the minted
/// token of that same offset still resumes the list there — so the refusal is about the
/// spelling, not the offset.
#[tokio::test]
async fn a_token_this_service_never_minted_is_refused_over_the_wire() {
    let router = emulator();
    start_change_set(&router, "page token respelling probe").await;
    // The change set has certainly reached its terminal status by now, so every list this
    // suite pages holds what applying it produced and stops changing under the cases.
    tokio::time::sleep(CHANGE_SET_PREPARING + CHANGE_SET_APPLYING + Duration::from_millis(400))
        .await;

    for row in paginated() {
        let first = page(&router, &row, &encode_page_token(row.prefix, 0)).await;
        assert_eq!(
            first.status, 200,
            "case {}: the minted token of offset 0 must be served, got {} with {:?}",
            row.operation, first.status, first.body
        );
        let whole = first.list(row.items);
        assert!(
            !whole.is_empty(),
            "case {}: the applied change set must leave this list something to resume across, \
             got {:?}",
            row.operation,
            first.body
        );

        // The offsets a short list really pages, so the minted token asked for beside each
        // respelling resumes at a page that exists as well as one past the end.
        for offset in [0_usize, 1, 2] {
            for (class, token) in values_never_minted(row.prefix, offset) {
                let refused = page(&router, &row, &token).await;
                assert!(
                    (400..500).contains(&refused.status),
                    "case {}/{offset}/{class} ({token:?}): a token this service never minted must \
                     be refused as a client error, got {} with {:?}",
                    row.operation,
                    refused.status,
                    refused.body
                );
                assert_eq!(
                    refused.error_code(),
                    "ValidationException",
                    "case {}/{offset}/{class} ({token:?}): must answer ValidationException, got \
                     {:?}",
                    row.operation,
                    refused.body
                );
                assert_eq!(
                    refused.fault(),
                    PAGE_TOKEN_FAULT,
                    "case {}/{offset}/{class} ({token:?}): must answer the one page-token fault, \
                     got {:?}",
                    row.operation,
                    refused.fault()
                );
                assert!(
                    refused.list(row.items).is_empty(),
                    "case {}/{offset}/{class} ({token:?}): a refusal must carry no page, got {:?}",
                    row.operation,
                    refused.body
                );
            }

            // The same offset written the way this service writes it: a page, resuming
            // exactly where it points, unchanged by everything asked above.
            let minted = encode_page_token(row.prefix, offset);
            let served = page(&router, &row, &minted).await;
            assert_eq!(
                served.status, 200,
                "case {}/{offset} ({minted:?}): the minted token of the same offset must still be \
                 served, got {} with {:?}",
                row.operation, served.status, served.body
            );
            assert_eq!(
                served.list(row.items),
                whole.iter().skip(offset).cloned().collect::<Vec<Value>>(),
                "case {}/{offset} ({minted:?}): must resume at offset {offset} of the list, got \
                 {:?}",
                row.operation,
                served.list(row.items)
            );
        }
    }
}
