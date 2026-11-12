// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusal of a page cursor it never issued, over HTTP.
//!
//! THE RULE: a `NextToken` that is not a position in the listing being read is refused
//! with HTTP 400, the input error that listing models, and AWS's own sentence for that
//! listing's bad-cursor condition. For the listings this table drives that sentence is
//! `"Invalid next token."` — wholly fixed text, carrying no part of the request and in
//! particular never echoing the rejected token — one sentence for all of them and for
//! every form a bad cursor can take. A listing AWS words differently pins its own
//! sentence beside its own operation (`ListEmailIdentities` echoes the token it rejected:
//! `tests/all/email_identity_cursor_refusal_wording.rs`), so nothing here is a claim
//! about a listing this table does not drive.
//!
//! The table spans what the rule generalises over:
//!
//! * the LISTING — one reached by `@httpQuery` on a `GET` and one reached through a
//!   `POST` body, one that models `BadRequestException` for the refusal and one that
//!   models `InvalidNextTokenException`, so the sentence is shown to belong to the
//!   condition rather than to an operation or to an error shape;
//! * the FORM of the bad cursor — base64 of text no listing ever minted, a cursor
//!   ANOTHER listing of this service did mint, text that is not base64 at all, a bare
//!   resource name, and the empty string.
//!
//! Every row is answered over the real wire path (`MultiServiceRouter::dispatch` with
//! this service registered, through the crate's shared JSON wire harness
//! [`crate::json_wire`]), and each listing's positive control — the same call with no
//! cursor at all — is asserted beside the refusals, so a listing that refused everything
//! could not pass.

use serde_json::json;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    encode_tagged_page_token, CONTACT_LISTS_PAGE_TOKEN_TAG,
};

use crate::json_wire::{query_component, Ses};

/// AWS's fixed sentence for a cursor the listings below never issued.
const INVALID_NEXT_TOKEN: &str = "Invalid next token.";

/// Where a listing carries its `NextToken`: the two wire forms this service's listings
/// take.
#[derive(Debug, Clone, Copy)]
enum Cursor {
    /// `NextToken` is bound to `@httpQuery` on a `GET`.
    Query,
    /// `NextToken` is a member of the `POST` body.
    Body,
}

/// One listing under test: how it is addressed, where its cursor goes, and the error
/// shape it models for a cursor that is not a position in it.
struct Listing {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    cursor: Cursor,
    error_code: &'static str,
}

/// The listings the table spans, each with the error code its own model declares for the
/// refusal (`smithy_query(query="operation",operation="…")`).
fn listings() -> Vec<Listing> {
    vec![
        Listing {
            case: "ListDedicatedIpPools",
            method: "GET",
            path: "/v2/email/dedicated-ip-pools",
            cursor: Cursor::Query,
            error_code: "BadRequestException",
        },
        Listing {
            case: "GetDedicatedIps",
            method: "GET",
            path: "/v2/email/dedicated-ips",
            cursor: Cursor::Query,
            error_code: "BadRequestException",
        },
        Listing {
            case: "ListConfigurationSets",
            method: "POST",
            path: "/v2/email/list-configuration-sets",
            cursor: Cursor::Body,
            error_code: "BadRequestException",
        },
        Listing {
            case: "ListExportJobs",
            method: "POST",
            path: "/v2/email/list-export-jobs",
            cursor: Cursor::Body,
            error_code: "BadRequestException",
        },
        Listing {
            case: "ListSuppressedDestinations",
            method: "GET",
            path: "/v2/email/suppression/addresses",
            cursor: Cursor::Query,
            error_code: "InvalidNextTokenException",
        },
    ]
}

/// The forms a cursor this service never issued can arrive in: base64 of text no listing
/// mints, a cursor another listing of this service DID mint, text that is not base64,
/// a bare resource name, and nothing at all.
fn forged_cursors() -> Vec<(&'static str, String)> {
    vec![
        (
            "base64 of text no listing ever minted",
            "c2ltMnJlYWwtZm9yZ2VkLW5leHQtdG9rZW4=".to_string(),
        ),
        (
            "a cursor another listing of this service minted",
            encode_tagged_page_token(CONTACT_LISTS_PAGE_TOKEN_TAG, "a-list"),
        ),
        ("text that is not base64 at all", "!!not-a-token!!".to_string()),
        (
            "a bare resource name",
            ::aws_smithy_types::base64::encode("a-pool"),
        ),
        ("the empty string", String::new()),
    ]
}

#[tokio::test]
async fn a_cursor_this_service_never_issued_is_refused_with_aws_sentence() {
    let ses = Ses::new();
    for listing in listings() {
        // The positive control first: the same listing, with no cursor at all, is served.
        let served = match listing.cursor {
            Cursor::Query => ses.call(listing.method, listing.path, None).await,
            Cursor::Body => {
                ses.call(listing.method, listing.path, Some(json!({})))
                    .await
            }
        };
        assert_eq!(
            served.status,
            200,
            "case {:?} with no cursor: expected the listing to be served, got {} {:?}",
            listing.case,
            served.status,
            served.body
        );

        for (form, token) in forged_cursors() {
            let case = format!("{} with {form}", listing.case);
            let refused = match listing.cursor {
                Cursor::Query => {
                    let path = format!(
                        "{}?NextToken={}",
                        listing.path,
                        query_component(&token)
                    );
                    ses.call(listing.method, &path, None).await
                }
                Cursor::Body => {
                    ses.call(listing.method, listing.path, Some(json!({"NextToken": token})))
                        .await
                }
            };
            assert_eq!(
                (refused.status, refused.error_code().as_str()),
                (400, listing.error_code),
                "case {case:?}: expected 400 {}, got {} {} {:?}",
                listing.error_code,
                refused.status,
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                refused.message(),
                INVALID_NEXT_TOKEN,
                "case {case:?}: expected the refusal to read {INVALID_NEXT_TOKEN:?}, got {:?}",
                refused.message()
            );
            assert!(
                token.is_empty() || !refused.message().contains(&token),
                "case {case:?}: the refusal must not echo the rejected cursor, got {:?}",
                refused.message()
            );
        }
    }
}
