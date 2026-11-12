// SPDX-License-Identifier: CC0-1.0

//! Only the spelling this service MINTS resumes a page.
//!
//! The rule under test: a `nextToken` names a page only when it is, character
//! for character, the token this service mints for the query and offset it
//! carries, so every OTHER way of writing the same hexadecimal offset is a value
//! the service never issued and names no page — refused with HTTP 400
//! `ValidationException` / `INVALID_NEXT_TOKEN` exactly like a token of a
//! different shape altogether.
//!
//! The spellings a client can hand back are the ones the parse of a hexadecimal
//! number reads although the minting format never writes them: upper-case and
//! mixed-case digits, and a leading `+` in place of one of the zeros the
//! fixed-width offset half is padded with. `NextToken`'s modelled
//! `^[a-zA-Z0-9+/=_-]+$`
//! (`smithy_query(query="operation", operation="GetAgreementEntitlements")`)
//! admits all of them, so each really reaches the handler rather than being
//! turned away at the boundary.
//!
//! The table spans what the rule generalises over: which part of the token is
//! respelt (the offset half, the digest half), which respelling (case, sign),
//! and the offset the token carries — one whose digits are all decimal, so only
//! a sign can respell it, and one carrying a hexadecimal letter, where case can.
//! The first test drives the rule end to end over paginated operations of three
//! kinds (a read of one agreement, a listing over every agreement, a search),
//! asking for the MINTED spelling of the same offset beside each refusal, so a
//! row is about the spelling rather than about the offset. The second reads the
//! same rule at the decision itself, over a wider set of offsets than a store
//! this size can page to.

use serde_json::{json, Value};
use service_com_amazonaws_marketplaceagreement_awsmpcommerceservice_v20200301::state::{
    listing_scope, page_offset, page_token,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    agreement_over, agreement_type_filter, call, emulator, term, Answer,
};
use crate::purchase_order_updates::agreement_holding;

/// Characters of the digest half of a token this service mints, which is what
/// the respellings below leave untouched when they rewrite the offset half.
const DIGEST_LEN: usize = 16;

/// One paginated operation to drive: the operation, the member its page of items
/// arrives in (the `items` member of its `@paginated` trait,
/// `smithy_query(query="operation", operation="<Operation>")`), the query whose
/// pages it mints tokens for, and the page size that makes it mint a token at an
/// offset worth respelling.
struct Row {
    case: &'static str,
    operation: &'static str,
    items_member: &'static str,
    query: Value,
    page_size: i32,
}

/// How a client can respell a token this service minted: the class the row
/// stands for, and how it rewrites one half of the token.
type Respelling = (&'static str, fn(&str) -> String);

/// Every way of rewriting a half of a minted token that a hexadecimal parse
/// still reads, none of which the minting format writes.
fn respellings() -> Vec<Respelling> {
    vec![
        ("upper-case hexadecimal digits", |half| half.to_uppercase()),
        ("mixed-case hexadecimal digits", |half| {
            half.char_indices()
                .map(|(at, digit)| {
                    if at % 2 == 0 {
                        digit.to_ascii_uppercase()
                    } else {
                        digit
                    }
                })
                .collect()
        }),
        ("a leading plus in place of a padding zero", |half| {
            match half.strip_prefix('0') {
                Some(rest) => format!("+{rest}"),
                None => half.to_string(),
            }
        }),
    ]
}

/// The tokens that respell `minted` — `(case, token)` — leaving out any
/// respelling that happens to BE the minted spelling, which is the token the
/// service issued rather than a respelling of it.
///
/// Both halves are respelt, each with the other left as minted: the offset half,
/// which carries the position, and the digest half, which binds the token to its
/// query.
fn respelt(minted: &str) -> Vec<(String, String)> {
    assert!(
        minted.len() > DIGEST_LEN,
        "a minted token carries a {DIGEST_LEN}-character digest and an offset, got {minted:?}"
    );
    let (digest, offset) = minted.split_at(DIGEST_LEN);
    let mut rows = Vec::new();
    for (case, respell) in respellings() {
        for (half, candidate) in [
            ("the offset half", format!("{digest}{}", respell(offset))),
            ("the digest half", format!("{}{offset}", respell(digest))),
        ] {
            if candidate == minted {
                continue;
            }
            rows.push((format!("{half} with {case}"), candidate));
        }
    }
    assert!(
        !rows.is_empty(),
        "every minted token has at least one respelling, got none for {minted:?}"
    );
    rows
}

/// The page `answer` carries, as the items of the member it reports them in.
fn items_of(answer: &Answer, items_member: &str) -> Vec<Value> {
    answer.body[items_member]
        .as_array()
        .unwrap_or_else(|| panic!("a page arrives in {items_member}, got {:?}", answer.body))
        .clone()
}

/// `operation` paged with `request`, asserted to have succeeded.
async fn served(
    router: &MultiServiceRouter,
    operation: &str,
    request: Value,
    case: &str,
) -> Answer {
    let answer = call(router, operation, request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: {operation} {request} must be served, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// A token this service never minted is refused, with the one refusal a client
/// branches on.
async fn refuses(
    router: &MultiServiceRouter,
    operation: &str,
    query: &Value,
    token: &str,
    case: &str,
) {
    let mut request = query.clone();
    request["nextToken"] = json!(token);
    let answer = call(router, operation, request.clone()).await;
    assert_eq!(
        answer.status, 400,
        "case {case} ({token:?}): {operation} {request} must refuse a token this service \
         never minted, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case} ({token:?}): the refusal is a ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.reason(),
        "INVALID_NEXT_TOKEN",
        "case {case} ({token:?}): the reason names the token, got {:?}",
        answer.body
    );
}

/// A respelling of a token this service minted is a token it never issued, so
/// every paginated operation refuses it — while the minted spelling of that same
/// offset is still served.
#[tokio::test]
async fn a_respelt_token_is_one_this_service_never_minted() {
    let router = emulator();

    // One agreement over two terms, so both the read of its terms and the read
    // of its entitlements page.
    let termed = agreement_over(
        &router,
        "ap-spellingtermed",
        vec![term("eula.v1"), term("SupportTerm")],
    )
    .await;
    // Enough charges across several agreements that a page can end past the
    // ninth item, where the offset half carries a hexadecimal letter.
    for index in 0..3 {
        agreement_holding(&router, &format!("ap-spellingcharged{index}"), 4).await;
    }

    let rows = vec![
        Row {
            case: "the entitlements of one agreement",
            operation: "GetAgreementEntitlements",
            items_member: "agreementEntitlements",
            query: json!({"agreementId": termed}),
            page_size: 1,
        },
        Row {
            case: "the accepted terms of one agreement",
            operation: "GetAgreementTerms",
            items_member: "acceptedTerms",
            query: json!({"agreementId": termed}),
            page_size: 1,
        },
        Row {
            case: "every charge, paged past the ninth",
            operation: "ListAgreementCharges",
            items_member: "items",
            query: json!({}),
            page_size: 10,
        },
        Row {
            case: "every charge, paged past the tenth",
            operation: "ListAgreementCharges",
            items_member: "items",
            query: json!({}),
            page_size: 11,
        },
        Row {
            case: "a search over the stored agreements",
            operation: "SearchAgreements",
            items_member: "agreementViewSummaries",
            query: json!({"filters": [agreement_type_filter()]}),
            page_size: 1,
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut bounded = row.query.clone();
        bounded["maxResults"] = json!(row.page_size);
        let first = served(&router, row.operation, bounded.clone(), case).await;
        let page = items_of(&first, row.items_member);
        assert_eq!(
            page.len(),
            usize::try_from(row.page_size).expect("a page size fits in usize"),
            "case {case}: expected a full page of {}, got {:?}",
            row.page_size,
            first.body
        );
        let minted = first.body["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: a bounded page over more items leaves a token, got {:?}",
                    first.body
                )
            })
            .to_string();

        for (difference, token) in respelt(&minted) {
            let case = format!("{case}: {difference}");
            refuses(&router, row.operation, &bounded, &token, &case).await;
        }

        // The same offset, written the way this service writes it: a page.
        let mut resumed = bounded.clone();
        resumed["nextToken"] = json!(minted);
        let answer = served(
            &router,
            row.operation,
            resumed,
            &format!("{case}: the minted spelling of the same offset"),
        )
        .await;
        assert_ne!(
            items_of(&answer, row.items_member).first(),
            page.first(),
            "case {case}: the minted token resumes PAST the first page, got {:?}",
            answer.body
        );
    }
}

/// The offsets a token can carry: zero, one, both sides of the decimal/letter
/// boundary (so a row has letters to change the case of), a wide one, and the
/// largest the 8-character offset half holds.
const OFFSETS: [usize; 8] = [0, 1, 9, 10, 15, 16, 0x00ab_cdef, 0xffff_ffff];

/// The same rule read at the decision itself, over every scope shape this crate
/// mints tokens for: the minted spelling of an offset resumes at that offset,
/// and every respelling of it names no page.
#[test]
fn the_page_token_decision_accepts_only_the_minted_spelling() {
    // One scope per shape of query this crate pages: an operation with no
    // selector, one keyed by an agreement, and one carrying several selectors.
    let scopes = [
        listing_scope("ListBillingAdjustmentRequests", &[]),
        listing_scope("GetAgreementEntitlements", &["agmt-11112222"]),
        listing_scope(
            "ListAgreementPaymentRequests",
            &["Acceptor", "agmt-33334444", "PENDING"],
        ),
    ];

    for scope in &scopes {
        for offset in OFFSETS {
            let minted = page_token(scope, offset);
            let decided = page_offset(scope, &minted).unwrap_or_else(|| {
                panic!("case {minted:?}: a token this service minted must resume a page")
            });
            assert_eq!(
                decided, offset,
                "case {minted:?}: must resume at {offset}, got {decided}"
            );

            for (difference, token) in respelt(&minted) {
                assert_eq!(
                    page_offset(scope, &token),
                    None,
                    "case offset {offset} with {difference} ({token:?}): a token this \
                     service never minted names no page, got {:?}",
                    page_offset(scope, &token)
                );
            }
        }
    }
}
