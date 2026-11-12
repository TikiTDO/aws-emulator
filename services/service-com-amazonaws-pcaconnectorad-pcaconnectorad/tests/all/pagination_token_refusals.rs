// SPDX-License-Identifier: CC0-1.0

//! What every paginated read of this service answers a `NextToken` it did not
//! issue: the pagination framework's refusal, naming the operation the token was
//! presented to and the wrapping schema the token's own payload declares.
//!
//! Each case drives the real wire path through [`super::harness`], so the modelled
//! `NextToken` constraints are enforced before the handler runs: every token below
//! satisfies the member's pattern
//! `^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$`
//! (`smithy_query(query="shape",shape="NextToken")`), so what refuses it is the
//! handler's cursor read and not the boundary validator.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, encode_label, router};

/// One row of the token table: a value no paginated read of this service ever
/// handed out, and the wrapping schema its payload declares.
///
/// The schema of every row is worked out BY HAND from the token's own text, never
/// read back from the implementation: a pagination token is base64 text, so its
/// first group of four characters spells the three octets the payload begins with,
/// and the schema is the first two of those octets read as a big-endian signed
/// 16-bit integer (base64url alphabet, RFC 4648 §5: `A`-`Z` = 0-25, `a`-`z` =
/// 26-51, `0`-`9` = 52-61, `-` = 62, `_` = 63).
struct TokenCase {
    /// Why this row is here: the way this token is foreign.
    case: &'static str,
    token: &'static str,
    schema: i16,
}

/// The rows span the ways a token can be foreign AND the range of the schema the
/// reading reports: a positive schema, a negative one, zero, the value a short
/// (padded) payload declares, a payload built out of this service's own token tag,
/// and a bare cursor field — the shape a service without a wrapped payload would
/// silently honour. Two rows share a first group with another to pin that the
/// characters past that group do not change the schema.
const TOKEN_CASES: &[TokenCase] = &[
    // n = 39, o = 40, t = 45, A = 0 -> 10011110 10001011 01000000 -> 0x9E8B.
    TokenCase {
        case: "a word, padded out to one group",
        token: "notA",
        schema: -24949,
    },
    // The same first group ("not-", where - = 62) behind twelve more characters:
    // 10011110 10001011 10111110 -> 0x9E8B again.
    TokenCase {
        case: "a sentence of a token, four groups long",
        token: "not-a-token-0123",
        schema: -24949,
    },
    // Y = 24, W = 22, J = 9, j = 35 -> 01100001 01100010 01100011 -> 0x6162.
    TokenCase {
        case: "base64 of plain text",
        token: "YWJj",
        schema: 24930,
    },
    // Four zero sextets: an all-zero payload.
    TokenCase {
        case: "an all-zero payload",
        token: "AAAA",
        schema: 0,
    },
    // _ = 63, w = 48 -> 11111111 -> one octet, so the second octet of the schema is
    // the zero bits the payload does not carry: 0xFF00.
    TokenCase {
        case: "a one-octet payload at the top of the alphabet",
        token: "_w==",
        schema: -256,
    },
    // N = 13, w = 48 -> 00110111 -> one octet, 0x3700.
    TokenCase {
        case: "a one-octet payload",
        token: "Nw==",
        schema: 14080,
    },
    // X = 23, - = 62, _ = 63, a = 26 -> 01011111 11101111 11011010 -> 0x5FEF.
    TokenCase {
        case: "the two URL-safe characters",
        token: "X-_a",
        schema: 24559,
    },
    // p = 41, c = 28, a = 26, d = 29 -> 10100101 11000110 10011101 -> 0xA5C6.
    TokenCase {
        case: "this service's own token tag, with no cursor behind it",
        token: "pcad",
        schema: -23098,
    },
    // The same tag with a cursor field that is not this service's: the first group
    // is unchanged ("pcaD", D = 3, only the last six bits differ), so is the schema.
    TokenCase {
        case: "a wrong-case tag with a cursor field",
        token: "pcaD0000000000000001",
        schema: -23098,
    },
    // 0 = 52 four times -> 11010011 01001101 00110100 -> 0xD34D.
    TokenCase {
        case: "a bare cursor field, with no payload wrapper",
        token: "0000000000000001",
        schema: -11443,
    },
];

/// One paginated read of this service: the operation name the refusal must name and
/// the resource collection it pages.
struct ListOperation {
    name: &'static str,
    path: &'static str,
}

/// The paginated reads this service implements.
const LIST_OPERATIONS: &[ListOperation] = &[
    ListOperation {
        name: "ListConnectors",
        path: "/connectors",
    },
    ListOperation {
        name: "ListDirectoryRegistrations",
        path: "/directoryRegistrations",
    },
];

/// A connector, so the refusals below are not an empty store answering. Its
/// directory is registered first: the create resolves its `DirectoryId` against
/// the directory-registration store.
async fn create_connector(router: &MultiServiceRouter, directory_id: &str) {
    let body = json!({
        "DirectoryId": directory_id,
        "CertificateAuthorityArn":
            "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
        "VpcInformation": { "SecurityGroupIds": ["sg-0123456789abcdef0"] },
    });
    let (status, code, parsed) = call(router, "POST", "/connectors", Some(&body)).await;
    assert_eq!(
        status, 202,
        "case {directory_id}: CreateConnector must answer the modelled 202, \
         got {status} / {code:?} / {parsed}"
    );
}

async fn create_directory_registration(router: &MultiServiceRouter, directory_id: &str) {
    let (status, code, parsed) = call(
        router,
        "POST",
        "/directoryRegistrations",
        Some(&json!({ "DirectoryId": directory_id })),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {directory_id}: CreateDirectoryRegistration must answer the modelled 202, \
         got {status} / {code:?} / {parsed}"
    );
}

/// One page of a paginated read, by `NextToken` alone or with a page size.
async fn list_page(
    router: &MultiServiceRouter,
    operation: &ListOperation,
    query: &str,
) -> (u16, String, Value) {
    call(
        router,
        "GET",
        &format!("{}?{query}", operation.path),
        None,
    )
    .await
}

/// Directory ids spanning the modelled `DirectoryId` pattern `^d-[0-9a-f]{10}$`,
/// one per record each collection needs to hand out a token of its own.
const DIRECTORY_IDS: &[&str] = &["d-0123456789", "d-abcdefabcd", "d-00000000ff"];

#[tokio::test]
async fn a_pagination_token_this_service_did_not_issue_is_refused_naming_its_wrapping_schema() {
    let router = router();
    for directory_id in DIRECTORY_IDS {
        create_directory_registration(&router, directory_id).await;
        create_connector(&router, directory_id).await;
    }

    for operation in LIST_OPERATIONS {
        for row in TOKEN_CASES {
            let case = format!("{} / {} ({})", operation.name, row.token, row.case);
            let (status, code, parsed) = list_page(
                &router,
                operation,
                &format!("NextToken={}", encode_label(row.token)),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a token this service never issued is refused with 400, \
                 got {status} / {parsed}"
            );
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: expected ValidationException, got {code:?} / {parsed}"
            );
            let expected = format!(
                "Pagination token exception in operation '{}': invalid payload wrapping \
                 schema {}",
                operation.name, row.schema
            );
            // The layer that reads the token keys its body at the lowercase
            // `message` member, which is where a client reads this refusal.
            assert_eq!(
                parsed["message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected message {expected:?}, got {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn a_token_the_read_issued_still_continues_the_listing_it_came_from() {
    let router = router();
    for directory_id in DIRECTORY_IDS {
        create_directory_registration(&router, directory_id).await;
        create_connector(&router, directory_id).await;
    }

    // What the refusals above turn on is the payload a presented token carries, so
    // the token this very read hands out is still honoured — on every page, up to
    // the last one, which carries none.
    for operation in LIST_OPERATIONS {
        let (status, code, first) = list_page(&router, operation, "MaxResults=1").await;
        assert_eq!(
            status, 200,
            "case {}: the first page must be served, got {status} / {code:?} / {first}",
            operation.name
        );
        let mut token = first["NextToken"].as_str().map(str::to_string);
        let mut pages = 1;
        while let Some(issued) = token {
            let (status, code, page) = list_page(
                &router,
                operation,
                &format!("MaxResults=1&NextToken={}", encode_label(&issued)),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {} page {pages}: an issued token must be accepted, \
                 got {status} / {code:?} / {page}",
                operation.name
            );
            pages += 1;
            assert!(
                pages <= DIRECTORY_IDS.len() + 1,
                "case {}: paging did not terminate after {pages} pages",
                operation.name
            );
            token = page["NextToken"].as_str().map(str::to_string);
        }
        assert_eq!(
            pages,
            DIRECTORY_IDS.len(),
            "case {}: one page per record was expected, got {pages}",
            operation.name
        );
    }
}
