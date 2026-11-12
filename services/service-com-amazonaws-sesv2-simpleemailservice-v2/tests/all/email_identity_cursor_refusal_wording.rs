// SPDX-License-Identifier: CC0-1.0

//! How `ListEmailIdentities` WORDS the refusal of a page cursor it never issued, over HTTP.
//!
//! THE RULE: a `NextToken` that is not a position in this listing is refused with HTTP 400,
//! the input error the operation models (`BadRequestException`, the only client error
//! `ListEmailIdentities` declares), and AWS's sentence for this listing's bad-cursor
//! condition: the fixed text `"Invalid NextToken <"` and `">."` around the rejected token,
//! which is the `NextToken` the request carried, whatever it is. The sentence is the
//! contract, so a client branching on it — or an operator reading the cursor back out of it —
//! sees what AWS sends.
//!
//! The table spans what the rule generalises over:
//!
//! * the FORM of the bad cursor — base64 of text no listing ever minted, a cursor ANOTHER
//!   listing of this service did mint, text that is not base64 at all, a bare resource name,
//!   and one of this listing's own cursors with a byte changed — so the sentence is shown to
//!   belong to the condition rather than to one fixture's token;
//! * the WIRE FORM the cursor arrives in — the `@httpQuery` member of the `GET` an SDK sends
//!   and the body member of the modelled `POST` — since one handler reads both.
//!
//! Each run asserts the positive controls beside the refusals: the same listing with no
//! cursor at all, and the real cursor a `PageSize=1` walk hands back, are both served — so a
//! listing that refused everything could not pass.
//!
//! The router, the one call and the reading of a reply are the crate's shared JSON wire
//! harness ([`crate::json_wire`]).

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    encode_tagged_page_token, CONTACT_LISTS_PAGE_TOKEN_TAG,
};

use crate::json_wire::{query_component, Reply, Ses};

/// The error `ListEmailIdentities` models for a request it cannot act on
/// (`smithy_query(query="operation",operation="ListEmailIdentities")`: `BadRequestException`
/// and `TooManyRequestsException`, and nothing else).
const BAD_REQUEST: &str = "BadRequestException";

/// The identities every case lists, so the listing under test has pages to walk.
const IDENTITIES: [&str; 4] = [
    "alpha.example.com",
    "beta.example.com",
    "gamma@example.net",
    "mail.example.org",
];

/// AWS's sentence for a cursor this listing never issued, with the rejected token in it.
fn invalid_next_token(token: &str) -> String {
    format!("Invalid NextToken <{token}>.")
}

/// The identity names one answer reported, as this listing's output names them
/// (`ListEmailIdentitiesResponse$EmailIdentities`, each an `IdentityInfo$IdentityName`).
fn listed(reply: &Reply) -> Vec<String> {
    reply.listed("EmailIdentities", "IdentityName")
}

/// `CreateEmailIdentity`.
async fn create(ses: &Ses, identity: &str) -> Reply {
    ses.call(
        "POST",
        "/v2/email/identities",
        Some(json!({"EmailIdentity": identity})),
    )
    .await
}

/// Where a request carries the listing's `NextToken`: the two wire forms one handler reads.
#[derive(Debug, Clone, Copy)]
enum Wire {
    /// `NextToken` bound to `@httpQuery` on the `GET` an SDK sends.
    Query,
    /// `NextToken` as a member of the modelled `POST` body.
    Body,
}

impl Wire {
    /// One `ListEmailIdentities` call carrying `page_size` and `next_token`.
    async fn list(self, ses: &Ses, page_size: u32, next_token: Option<&str>) -> Reply {
        match self {
            Self::Query => {
                let mut query = format!("PageSize={page_size}");
                if let Some(token) = next_token {
                    query.push_str(&format!("&NextToken={}", query_component(token)));
                }
                ses.call("GET", &format!("/v2/email/identities?{query}"), None)
                    .await
            }
            Self::Body => {
                let mut body = json!({"PageSize": page_size});
                if let Some(token) = next_token {
                    body["NextToken"] = Value::String(token.to_string());
                }
                ses.call("POST", "/v2/email/list-identities", Some(body))
                    .await
            }
        }
    }
}

/// Create every identity the cases list.
async fn seeded() -> Ses {
    let ses = Ses::new();
    for identity in IDENTITIES {
        let created = create(&ses, identity).await;
        assert_eq!(
            created.status, 200,
            "case {identity:?}: expected the identity to be created, got {} {:?}",
            created.status, created.body
        );
    }
    ses
}

/// The forms a cursor this listing never issued can arrive in.
fn forged_cursors(real: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "base64 of text no listing ever minted",
            "c2ltMnJlYWwtZm9yZ2VkLW5leHQtdG9rZW4=".to_string(),
        ),
        (
            "a cursor another listing of this service minted",
            encode_tagged_page_token(CONTACT_LISTS_PAGE_TOKEN_TAG, "a-list"),
        ),
        (
            "text that is not base64 at all",
            "!!not-a-token!!".to_string(),
        ),
        (
            "a bare identity name",
            ::aws_smithy_types::base64::encode("alpha.example.com"),
        ),
        (
            "one of this listing's own cursors with its tag broken",
            ::aws_smithy_types::base64::encode(
                String::from_utf8(::aws_smithy_types::base64::decode(real).expect("base64"))
                    .expect("utf-8")
                    .replacen("sesv2", "sesv3", 1),
            ),
        ),
    ]
}

#[tokio::test]
async fn a_cursor_this_listing_never_issued_is_refused_in_aws_words() {
    let ses = seeded().await;
    for wire in [Wire::Query, Wire::Body] {
        // The positive controls first: the listing serves a page, and the cursor IT handed
        // back resumes that page.
        let first = wire.list(&ses, 1, None).await;
        assert_eq!(
            (first.status, listed(&first).len()),
            (200, 1),
            "case {wire:?} with no cursor: expected one identity on a page, got {} {:?}",
            first.status,
            first.body
        );
        let real = first
            .next_token()
            .unwrap_or_else(|| panic!("case {wire:?}: the first page hands back a cursor"));
        let second = wire.list(&ses, 1, Some(&real)).await;
        assert_eq!(
            (second.status, listed(&second)),
            (200, vec![IDENTITIES[1].to_string()]),
            "case {wire:?} with the cursor the listing handed back: expected the next \
             identity, got {} {:?}",
            second.status,
            second.body
        );

        for (form, token) in forged_cursors(&real) {
            let case = format!("{wire:?} with {form}");
            let refused = wire.list(&ses, 1, Some(&token)).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (400, BAD_REQUEST.to_string()),
                "case {case:?}: expected 400 {BAD_REQUEST}, got {} {} {:?}",
                refused.status,
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                refused.message(),
                invalid_next_token(&token),
                "case {case:?}: expected the refusal to read {:?}, got {:?}",
                invalid_next_token(&token),
                refused.message()
            );
        }
    }
}
