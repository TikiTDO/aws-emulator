// SPDX-License-Identifier: CC0-1.0

//! Which STAGE of reading a presented token failed, as the two token-based authorization
//! operations report it.
//!
//! RULE: reading a token takes two stages, and the refusal says which of them the token
//! failed at. Its body is first DESERIALIZED into the claim set the service requires: a
//! body that is not that claim set reached no verification at all, and is refused as
//! `Failed to decode <member>: Invalid body: JSON error: <what that deserialization
//! reported>` — the claim at fault, named by the deserialization, and the position of it in
//! the body the CALLER sent. Only a token that decoded is then verified against the policy
//! store's identity sources, and a refusal from THAT stage keeps the
//! `Failed to verify <member>: <reason>` wording. The two sentences send a client to
//! different repairs: the token it presented, or the trust its policy store configures.
//!
//! Rows differ along the body a token carries — a required claim absent, a required claim
//! carrying another JSON type, and that same absence in bodies of different lengths, with
//! whitespace, and spread over several lines, so the position each refusal reports is the
//! one in THAT body — and along the member the token arrived in. Every row is asked of both
//! `IsAuthorizedWithToken` and `BatchIsAuthorizedWithToken`, because the token rules are the
//! token's and not the calling operation's.
//!
//! The tokens the rows start from are minted by the Amazon Cognito user-pool service
//! mounted on the same engine and signed with that pool's own key, so the control rows — an
//! untouched token, and one whose signature was tampered with — are decided by the real
//! checks.

use serde_json::{json, Value};

use crate::harness::{self, error_code, fault_message, field_list};
use crate::is_authorized_with_token::{
    app_client, base64url, group_permit, identity_source, policy, sign_in, with_tampered_signature,
    SignedIn, DOCUMENT, GROUP_TYPE, PERMITTED_GROUP,
};

/// The action every request in this suite asks about, as `(actionType, actionId)` — the one
/// the permit policy of [`group_permit`] is written over.
const READ: (&str, &str) = ("App::Action", "read");

/// The entity type the resource every request names is of, likewise.
const DOCUMENT_TYPE: &str = "App::Doc";

/// The claim the decode stage requires of a token's body: the issuer (RFC 7519 §4.1.1),
/// which is what says which identity source — and so which published key set, which
/// accepted clients and which principal type — a presented token belongs to.
const ISSUER_CLAIM: &str = "iss";

/// Which member a row presents its token in.
#[derive(Clone, Copy)]
enum Member {
    Access,
    Identity,
}

impl Member {
    /// The request member's name on the wire.
    fn wire_name(self) -> &'static str {
        match self {
            Self::Access => "accessToken",
            Self::Identity => "identityToken",
        }
    }

    /// How a refusal NAMES this member in its sentence: the API's own capitalisation of it.
    fn reported_name(self) -> &'static str {
        match self {
            Self::Access => "AccessToken",
            Self::Identity => "IdentityToken",
        }
    }

    /// The JSON pointer a `ValidationException`'s `fieldList` names it by.
    fn pointer(self) -> &'static str {
        match self {
            Self::Access => "/accessToken",
            Self::Identity => "/identityToken",
        }
    }

    /// The token this member takes, as the pool issued it to `identity`.
    fn issued(self, identity: &SignedIn) -> String {
        match self {
            Self::Access => identity.access_token.clone(),
            Self::Identity => identity.id_token.clone(),
        }
    }
}

/// The token a row presents.
#[derive(Clone, Copy)]
enum Token {
    /// The issued token with its body replaced by exactly these characters, base64url
    /// encoded: the octets the service decodes are the ones the row names.
    Body(&'static str),
    /// The issued token, untouched.
    Issued,
    /// The issued token with one byte of its signature changed.
    Tampered,
}

/// What a row must be answered with.
enum Expect {
    /// The body is not the claim set the decode requires, because this claim is absent: the
    /// deserialization names the claim and reports the position the body it was reading
    /// ENDED at, which is the position just past that body's closing brace.
    MissingClaim(&'static str),
    /// The body carries the claim with a value of another JSON type: the deserialization
    /// reports what it found and what the claim had to be, at the position of the value.
    IllTypedClaim {
        found: &'static str,
        wanted: &'static str,
    },
    /// The body decoded, so a refusal is the VERIFY stage's and carries this clause.
    RefusedByVerification(&'static str),
    /// The body decoded and the token verified: a decision, not a refusal.
    Decision(&'static str),
}

/// One case.
struct Row {
    case: &'static str,
    member: Member,
    token: Token,
    expect: Expect,
}

/// Where a deserialization that ran out of claims reports its position: the end of the JSON
/// object it was reading, as `(line, column)`.
///
/// A missing claim is only known once the whole object has been read, so the position is the
/// one just past that object's closing brace: the line it is on, counting the body's first
/// line as 1, and the characters consumed on that line. Computed here from the body the row
/// sends, so the assertion is independent of the service's own reading of it.
fn end_of_object(body: &str) -> (usize, usize) {
    let end = body
        .rfind('}')
        .map(|brace| brace + 1)
        .unwrap_or_else(|| panic!("a body a row sends is a JSON object, got {body:?}"));
    let line = 1 + body[..end].matches('\n').count();
    let start_of_line = body[..end].rfind('\n').map(|at| at + 1).unwrap_or_default();
    (line, end - start_of_line)
}

/// `issued` with its body replaced by `body`: its real header and its real signature, over
/// the exact characters the row chose.
fn with_body(issued: &str, body: &str) -> String {
    let parts: Vec<&str> = issued.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "an issued token is three '.'-separated segments, got {issued:?}"
    );
    format!("{}.{}.{}", parts[0], base64url(body.as_bytes()), parts[2])
}

/// The `IsAuthorizedWithToken` body presenting `token` in `member` against `store`.
fn single(store: &str, member: Member, token: &str) -> Value {
    let mut body = json!({
        "policyStoreId": store,
        "action": { "actionType": READ.0, "actionId": READ.1 },
        "resource": { "entityType": DOCUMENT_TYPE, "entityId": DOCUMENT },
    });
    body[member.wire_name()] = json!(token);
    body
}

/// The `BatchIsAuthorizedWithToken` body asking the SAME question of the same token, as the
/// one request of a batch.
fn batch(store: &str, member: Member, token: &str) -> Value {
    let mut body = json!({
        "policyStoreId": store,
        "requests": [{
            "action": { "actionType": READ.0, "actionId": READ.1 },
            "resource": { "entityType": DOCUMENT_TYPE, "entityId": DOCUMENT },
        }],
    });
    body[member.wire_name()] = json!(token);
    body
}

/// The decision an operation answered: the one on the response for the single-request call,
/// and the one of the batch's only result for the batch.
fn decision(operation: &str, answered: &Value) -> Option<String> {
    let decided = match operation {
        "BatchIsAuthorizedWithToken" => &answered["results"][0]["decision"],
        _ => &answered["decision"],
    };
    decided.as_str().map(str::to_string)
}

#[tokio::test]
async fn a_token_body_that_is_no_claim_set_is_refused_as_a_decode_failure() {
    let (router, _engine) = harness::router_with_engine();
    let pool = harness::user_pool(&router, "avp-token-decode-pool").await;
    let client = app_client(&router, &pool, "avp-token-decode-client").await;
    let member = sign_in(
        &router,
        &pool,
        &client,
        "member",
        &[PERMITTED_GROUP],
        &json!([]),
    )
    .await;

    let store = harness::store_with_validation_off(&router).await;
    identity_source(&router, &store, &pool, &[&client], Some(GROUP_TYPE)).await;
    policy(&router, &store, group_permit(GROUP_TYPE, &pool)).await;

    let rows = [
        Row {
            case: "an identity token whose body names no issuer",
            member: Member::Identity,
            token: Token::Body(r#"{"sub":"alice"}"#),
            expect: Expect::MissingClaim(ISSUER_CLAIM),
        },
        Row {
            case: "an access token whose body names no issuer, in a longer body",
            member: Member::Access,
            token: Token::Body(r#"{"sub":"alice","token_use":"access","exp":2000000000}"#),
            expect: Expect::MissingClaim(ISSUER_CLAIM),
        },
        Row {
            case: "a body naming no issuer, written with whitespace between its members",
            member: Member::Identity,
            token: Token::Body(r#"{ "sub": "alice", "exp": 2000000000 }"#),
            expect: Expect::MissingClaim(ISSUER_CLAIM),
        },
        Row {
            case: "a body naming no issuer, spread over three lines",
            member: Member::Identity,
            token: Token::Body("{\n  \"sub\": \"alice\"\n}"),
            expect: Expect::MissingClaim(ISSUER_CLAIM),
        },
        Row {
            case: "a body whose only member is not the issuer claim",
            member: Member::Access,
            token: Token::Body(r#"{"client_id":"an-app-client"}"#),
            expect: Expect::MissingClaim(ISSUER_CLAIM),
        },
        Row {
            case: "a body whose issuer claim is a number",
            member: Member::Identity,
            token: Token::Body(r#"{"iss":42,"sub":"alice"}"#),
            expect: Expect::IllTypedClaim {
                found: "integer `42`",
                wanted: "a string",
            },
        },
        Row {
            case: "a body whose issuer claim is a list of issuers",
            member: Member::Access,
            token: Token::Body(r#"{"iss":["https://a.example.com"],"sub":"alice"}"#),
            expect: Expect::IllTypedClaim {
                found: "sequence",
                wanted: "a string",
            },
        },
        Row {
            case: "a body whose issuer claim is null",
            member: Member::Identity,
            token: Token::Body(r#"{"iss":null,"sub":"alice"}"#),
            expect: Expect::IllTypedClaim {
                found: "null",
                wanted: "a string",
            },
        },
        Row {
            case: "an identity token that decoded and whose signature was tampered with",
            member: Member::Identity,
            token: Token::Tampered,
            expect: Expect::RefusedByVerification("signature is invalid"),
        },
        Row {
            case: "an access token that decoded and whose signature was tampered with",
            member: Member::Access,
            token: Token::Tampered,
            expect: Expect::RefusedByVerification("signature is invalid"),
        },
        Row {
            case: "the identity token exactly as the pool issued it",
            member: Member::Identity,
            token: Token::Issued,
            expect: Expect::Decision("ALLOW"),
        },
        Row {
            case: "the access token exactly as the pool issued it",
            member: Member::Access,
            token: Token::Issued,
            expect: Expect::Decision("ALLOW"),
        },
    ];

    for row in &rows {
        let issued = row.member.issued(&member);
        let presented = match row.token {
            Token::Body(body) => with_body(&issued, body),
            Token::Issued => issued.clone(),
            Token::Tampered => with_tampered_signature(&issued),
        };

        for operation in ["IsAuthorizedWithToken", "BatchIsAuthorizedWithToken"] {
            let body = match operation {
                "BatchIsAuthorizedWithToken" => batch(&store, row.member, &presented),
                _ => single(&store, row.member, &presented),
            };
            let (status, answered) = harness::call(&router, operation, &body).await;
            let case = format!("case {:?} on {operation}", row.case);

            match &row.expect {
                Expect::Decision(expected) => {
                    assert_eq!(
                        status, 200,
                        "{case}: expected a decision, got {status} with {answered}"
                    );
                    assert_eq!(
                        decision(operation, &answered).as_deref(),
                        Some(*expected),
                        "{case}: expected decision {expected}, got {answered}"
                    );
                    continue;
                }
                _ => {
                    assert_eq!(
                        (status, error_code(&answered).as_str()),
                        (400, "ValidationException"),
                        "{case}: expected a 400 ValidationException, got {status} with \
                         {answered}"
                    );
                }
            }

            let message = fault_message(&answered);
            let named: Vec<String> = field_list(&answered)
                .into_iter()
                .map(|(path, _)| path)
                .collect();
            assert_eq!(
                named,
                vec![row.member.pointer().to_string()],
                "{case}: expected fieldList to name the member the token arrived in, got \
                 {answered}"
            );

            let decode_stage = format!(
                "Failed to decode {}: Invalid body: JSON error: ",
                row.member.reported_name()
            );
            match &row.expect {
                Expect::MissingClaim(claim) => {
                    let Token::Body(body) = row.token else {
                        panic!("{case}: a decode refusal is asserted over the body the row sends");
                    };
                    let (line, column) = end_of_object(body);
                    assert_eq!(
                        message,
                        format!("{decode_stage}missing field `{claim}` at line {line} column {column}"),
                        "{case}: expected the decode stage to name the absent claim and its \
                         position in the body sent, got {message:?}"
                    );
                }
                Expect::IllTypedClaim { found, wanted } => {
                    assert!(
                        message.starts_with(&decode_stage),
                        "{case}: expected the decode-stage framing {decode_stage:?}, got \
                         {message:?}"
                    );
                    assert!(
                        message.contains(&format!("invalid type: {found}, expected {wanted}")),
                        "{case}: expected the type the claim carried and the one it had to \
                         be, got {message:?}"
                    );
                    assert!(
                        message.contains(" at line 1 column "),
                        "{case}: expected the position in the body sent, got {message:?}"
                    );
                }
                Expect::RefusedByVerification(clause) => {
                    assert_eq!(
                        message,
                        format!("Failed to verify {}: {clause}", row.member.reported_name()),
                        "{case}: a token that DECODED and then failed a check is refused as \
                         the verify stage, got {message:?}"
                    );
                }
                Expect::Decision(_) => unreachable!("a decision is asserted above"),
            }
        }
    }
}
