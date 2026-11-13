// SPDX-License-Identifier: CC0-1.0

//! The WAFv2 WEB ACL in front of a user pool: how one request to the pool's API is
//! decided before the pool serves it.
//!
//! A web ACL is put in front of a user pool by WAFv2's `AssociateWebACL` over the
//! pool's own ARN — an Amazon Cognito user pool is one of the protected resource
//! kinds that call accepts — and from then on the pool's API operations are decided
//! by that web ACL. The model states which operations those are: every operation
//! that declares [`FORBIDDEN_EXCEPTION`], "thrown when WAF doesn't allow your request
//! based on a web ACL that's associated with your user pool"
//! (`smithy_query(query="shape",shape="ForbiddenException")`), is one a web ACL
//! decides; every other operation of this service declares no such refusal and is
//! served as it is with no web ACL in front of the pool at all. [`GOVERNED_OPERATIONS`]
//! is that list, and the unit test below holds it to the model.
//!
//! The decision itself is WAFv2's own
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]): this
//! module holds no second reading of a rule tree. It does two things the decision
//! needs and only this service can do — resolve WHICH user pool the request addresses
//! ([`pool_arn`]) and read the inbound HTTP request into the components a
//! `FieldToMatch` names ([`inspected_request`]) — and renders the answer:
//!
//! * a decision that lets the request through returns `None`, so the operation runs
//!   exactly as it did before any web ACL was associated;
//! * a terminating action that configures its own `CustomResponse` is served BY WAF,
//!   with that response's status, headers and body;
//! * any other terminating action is answered with this service's own
//!   [`FORBIDDEN_EXCEPTION`], under the status the model declares for it. That covers
//!   a `Captcha` and a `Challenge` action too: those two serve a browser an
//!   interstitial to solve, and this endpoint answers an RPC client that has no
//!   browser, so what it owes such a caller is the one refusal the model declares for
//!   a request a web ACL did not allow rather than a status with no puzzle behind it.
//!
//! A request this module cannot place — a body that is not JSON, a token or an app
//! client this emulator never issued, a pool that no longer exists — resolves no pool
//! ARN and so no web ACL, and is served by the operation, which owes that request its
//! own refusal.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema_server::protocol::ServerProtocol;
use aws_smithy_types::body::SdkBody;
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge as waf_edge;
use service_com_amazonaws_wafv2_awswaf_20190729::service::logs as waf_logs;

use crate::ops::types::{ForbiddenException, SCHEMA_FORBIDDEN_EXCEPTION};
use crate::state::ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState as PoolStore;

/// The error shape this service refuses a request its web ACL did not allow with, and
/// the `__type` that refusal carries.
pub const FORBIDDEN_EXCEPTION: &str = "ForbiddenException";

/// The status [`FORBIDDEN_EXCEPTION`] is answered under, which is the one the model
/// declares for it (`smithy_query(query="shape",shape="ForbiddenException")`:
/// `httpError` 403).
///
/// Not the status this endpoint answers its OWN refusals under
/// ([`super::error_wire`]): a request a web ACL did not allow is refused in FRONT of
/// the pool and never reaches the operation that would have served it, so it is
/// answered with the modelled forbidden status rather than with the status this API
/// words a bad parameter in.
const FORBIDDEN_STATUS: u16 = 403;

/// The sentence Amazon Cognito refuses a request its web ACL did not allow with.
///
/// The WHOLE sentence is fixed text for this error class: `ForbiddenException$message`
/// is "the message returned when WAF doesn't allow your request based on a web ACL
/// that's associated with your user pool", and it carries nothing from the request, from
/// the web ACL or from the pool's state — so every request a web ACL's decision refuses
/// is worded the same, whichever operation was called and whichever rule decided it.
const WAF_REFUSAL_MESSAGE: &str = "Request not allowed due to WAF block.";

/// The media type an `awsJson1_1` body carries, which is what a response served at the
/// edge with no `CustomResponseBody` content type of its own carries.
const JSON_CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// Every operation of this service a web ACL associated with the user pool decides.
///
/// The model's own list: these are exactly the operations that declare
/// `ForbiddenException` (the unit test below recomputes the set from the shipped model
/// and fails if the two disagree), which is how the model states that WAF is consulted
/// for an operation. The admin half of this API, the control plane and the operations a
/// pool's own configuration is read through declare no such refusal, so none of them is
/// decided by a web ACL.
pub const GOVERNED_OPERATIONS: &[&str] = &[
    "AssociateSoftwareToken",
    "ChangePassword",
    "CompleteWebAuthnRegistration",
    "ConfirmDevice",
    "ConfirmForgotPassword",
    "ConfirmSignUp",
    "DeleteUser",
    "DeleteUserAttributes",
    "DeleteWebAuthnCredential",
    "ForgetDevice",
    "ForgotPassword",
    "GetClientToken",
    "GetDevice",
    "GetTokensFromRefreshToken",
    "GetUser",
    "GetUserAttributeVerificationCode",
    "GetUserAuthFactors",
    "GlobalSignOut",
    "InitiateAuth",
    "ListDevices",
    "ListWebAuthnCredentials",
    "ResendConfirmationCode",
    "RespondToAuthChallenge",
    "RevokeToken",
    "SetUserMFAPreference",
    "SetUserSettings",
    "SignUp",
    "StartWebAuthnRegistration",
    "UpdateDeviceStatus",
    "UpdateUserAttributes",
    "VerifySoftwareToken",
    "VerifyUserAttribute",
];

/// Whether a web ACL decides `operation`.
pub fn governs(operation: &str) -> bool {
    GOVERNED_OPERATIONS.contains(&operation)
}

/// The response the web ACL in front of the user pool `req` addresses serves for it, or
/// `None` when the request goes on to the operation — which is every request of an
/// operation no web ACL decides, every request addressing a pool with no web ACL in
/// front of it, and every request the web ACL allowed — beside the record that web ACL's
/// own `LoggingConfiguration` logs about the request.
///
/// The record is WAFv2's ([`waf_logs`]); this service only DELIVERS it, with
/// `waf_logs::deliver`, because the write is a call made from the operation serving the
/// request. `None` when the web ACL logs nothing about this request.
///
/// The two stores are locked one after the other and never at the same time, and the
/// wafv2 side locks nothing of this service's, so the two cannot deadlock.
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    operation: &str,
    req: &RtRequest,
) -> (Option<RtResponse>, Option<waf_logs::LoggedRecord>) {
    if !governs(operation) {
        return (None, None);
    }
    // The pool the request addresses is named in the request's own body, which this
    // service's records resolve to a pool. A body this protocol cannot read at all
    // names no pool: the operation owes that request its own refusal.
    let Some(decided) = decided(engine, req) else {
        return (None, None);
    };
    decided
}

/// [`refusal`]'s body for an operation a web ACL DOES decide, written as a function that
/// answers `None` for every request this module cannot place — a body that is not JSON, a
/// token or an app client this emulator never issued, a pool that no longer exists — so
/// each of those stays one early answer rather than a pair repeated at every step.
fn decided<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
) -> Option<(Option<RtResponse>, Option<waf_logs::LoggedRecord>)> {
    let submitted_body = req.body().bytes()?.to_vec();
    let submitted: serde_json::Value = serde_json::from_slice(&submitted_body).ok()?;
    let pool_arn = {
        let state = crate::state::service_state(engine)?;
        let store = state.lock();
        addressed_pool_arn(&store, &submitted)?
    };

    // The decision, and the lock discipline it needs, are wafv2's own: `decide` reads
    // its records under a shared lock, evaluates the web ACL with the lock RELEASED,
    // writes the data plane's own bookkeeping — the sample `GetSampledRequests` draws
    // from — under an exclusive one, and publishes the `AWS/WAFV2` counters of the
    // dimensions that acted once that lock is gone. It is given the shared engine
    // because reporting what the web ACL did spans two stores, and it answers `None`
    // when the wafv2 service is not mounted in this process at all, so no binding can
    // exist. This service's store is not held across the call.
    let decision = waf_edge::decide(engine, &pool_arn, &inspected_request(req, submitted_body))?;
    let logged = decision.log_record.clone();
    tracing::debug!(
        user_pool_arn = %pool_arn,
        rule = decision.rule_name.as_deref().unwrap_or("<default action>"),
        "the web acl in front of this user pool decided this request"
    );
    if decision.action.is_allow() {
        return Some((None, logged));
    }
    tracing::info!(
        user_pool_arn = %pool_arn,
        rule = decision.rule_name.as_deref().unwrap_or("<default action>"),
        "the web acl in front of this user pool does not let the request through"
    );
    // A terminating action that configures its own `CustomResponse` is served BY WAF:
    // the status, the headers and the body are the web ACL's own, and this service's
    // error envelope is not used, because the pool never produced the response.
    let refused = match decision.custom_response {
        Some(custom) => edge_response(custom),
        None => forbidden_response(),
    };
    Some((Some(refused), logged))
}

/// The ARN of the user pool `submitted` addresses, or `None` when it addresses none
/// this emulator holds.
///
/// The ARN is the pool's OWN stored ARN, because that is the ARN a web ACL was put in
/// front of the pool by: `AssociateWebACL` is given the ARN `CreateUserPool` reported,
/// while a client calling the pool's API reaches this emulator at a host that need not
/// spell any Region, so a Region read off the request would look up a key nothing ever
/// stored.
///
/// Which member names the pool is the operation's business, and the governed half of
/// this API names it exactly three ways — none of them declares a `UserPoolId` member at
/// all:
///
/// * `ClientId`, the app client the call is made through: the unauthenticated surface
///   (`InitiateAuth`, `SignUp`, `ForgotPassword`, `RevokeToken`, …) identifies its pool
///   by that id alone;
/// * `AccessToken`, "a valid access token that Amazon Cognito issued to the currently
///   signed-in user": the self-service surface names its pool by the record of the token
///   it was issued from;
/// * `Session`, for a request part way through a challenge, whose stored session names
///   the pool the challenge belongs to.
///
/// A request carrying more than one of them resolves the first that names a pool this
/// emulator holds, rather than the first that is merely PRESENT: a `ClientId` no app
/// client answers to leaves a request that also carries a session (as
/// `RespondToAuthChallenge` does) addressing the pool that session belongs to, and a
/// request WAF would still decide must not escape the decision because one of the
/// members it carries resolves nothing.
fn addressed_pool_arn(store: &PoolStore, submitted: &serde_json::Value) -> Option<String> {
    let member = |name: &str| {
        submitted
            .get(name)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let of_client = member("ClientId").and_then(|client_id| store.pool_of_client(client_id));
    let of_token = || {
        member("AccessToken")
            .and_then(|token| store.token(token))
            .and_then(|record| store.pool(&record.user_pool_id))
    };
    // A PURE read of the stored session: resolving which pool a request addresses must
    // not sweep, end or otherwise disturb a session the operation has yet to answer.
    let of_session = || {
        member("Session")
            .and_then(|token| store.auth_sessions.get(token))
            .and_then(|session| store.pool(&session.user_pool_id))
    };
    of_client
        .or_else(of_token)
        .or_else(of_session)
        .map(|pool| pool.arn.clone())
}

/// The request as WAFv2 inspects it: every component a `FieldToMatch` can name.
///
/// `body` is the payload already read off the request, so a rule inspecting
/// `FieldToMatch$Body` and the resolution of which pool the request addresses read the
/// SAME bytes: a body this emulator cannot read as bytes resolves no pool, so the
/// inspection never runs against a substituted empty one.
fn inspected_request(req: &RtRequest, body: Vec<u8>) -> waf_edge::InspectedRequest {
    let (uri_path, query_string) = split_uri(req.uri());
    // The one reading of an inbound request as WAF inspects it, which every protected
    // resource's own service shares.
    waf_edge::InspectedRequest::of_http(req, uri_path, query_string, body)
}

/// The path and the query string of `uri`, the two components
/// `FieldToMatch$UriPath` and `FieldToMatch$QueryString` name. The query string is
/// WITHOUT its leading `?`, as that member states it.
fn split_uri(uri: &str) -> (&str, &str) {
    // The authority, when the request line carries an absolute URI, is not part of
    // either component: the path begins at the first `/` after it.
    let after_scheme = match uri.find("://") {
        Some(scheme) => match uri[scheme + 3..].find('/') {
            Some(path) => &uri[scheme + 3 + path..],
            None => "/",
        },
        None => uri,
    };
    match after_scheme.split_once('?') {
        Some((path, query)) => (path, query),
        None => (after_scheme, ""),
    }
}

/// The refusal a request the web ACL did not allow is answered with: this service's
/// modelled [`FORBIDDEN_EXCEPTION`], rendered in the `awsJson1_1` envelope every other
/// error of this endpoint is rendered in and naming its shape in the header an AWS
/// client resolves the error class from.
fn forbidden_response() -> RtResponse {
    let refused = ForbiddenException {
        message: Some(WAF_REFUSAL_MESSAGE.to_string()),
    };
    let rendered = ::aws_smithy_schema_server::rpc::AwsJson11().serialize_error(
        &refused,
        &SCHEMA_FORBIDDEN_EXCEPTION,
        FORBIDDEN_EXCEPTION,
        FORBIDDEN_STATUS,
    );
    match rendered {
        Ok(response) => super::error_wire::naming_the_error_shape(response, FORBIDDEN_EXCEPTION),
        // The envelope is built from this crate's own generated schema and a message
        // of its own, so it cannot fail to render; a failure is still answered rather
        // than letting the caller wait on nothing.
        Err(failed) => {
            tracing::warn!(
                error = %failed,
                "the forbidden refusal could not be rendered in this service's envelope"
            );
            response_with(FORBIDDEN_STATUS, JSON_CONTENT_TYPE, Vec::new())
        }
    }
}

/// The response a web ACL's own `CustomResponse` serves, as it leaves the edge.
fn edge_response(custom: waf_edge::EdgeResponse) -> RtResponse {
    let mut response = response_with(
        custom.status,
        custom.content_type.as_deref().unwrap_or(JSON_CONTENT_TYPE),
        custom.body,
    );
    for (name, value) in custom.headers {
        if let Err(refused) = response.headers_mut().try_insert(name.clone(), value) {
            tracing::warn!(
                header = %name,
                error = %refused,
                "the header the web acl's custom response configures is not one a header \
                 can carry, and is left off the response"
            );
        }
    }
    response
}

/// A response carrying `status`, `content_type` and `body` and nothing else — what a
/// response served at the EDGE carries, which is not a modelled error of this service.
fn response_with(status: u16, content_type: &str, body: Vec<u8>) -> RtResponse {
    let length = body.len();
    let mut response = RtResponse::new(
        StatusCode::try_from(status)
            .unwrap_or_else(|_| StatusCode::try_from(FORBIDDEN_STATUS).expect("403 is a status")),
        SdkBody::from(body),
    );
    let _ = response
        .headers_mut()
        .try_insert("content-type", content_type.to_string());
    let _ = response
        .headers_mut()
        .try_insert("content-length", length.to_string());
    response
}

#[cfg(test)]
mod tests {
    use super::{governs, split_uri, GOVERNED_OPERATIONS};

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json"
    );

    /// The shape id of the refusal an operation a web ACL decides declares.
    const FORBIDDEN_SHAPE: &str = "com.amazonaws.cognitoidentityprovider#ForbiddenException";

    /// RULE: the operations a web ACL decides are exactly the operations the model
    /// declares `ForbiddenException` for.
    ///
    /// The set comes from the MODEL rather than from a second hand list, so an
    /// operation the model adds that refusal to — or removes it from — fails here
    /// instead of silently being left out of (or wrongly subjected to) the web ACL's
    /// decision.
    #[test]
    fn a_web_acl_decides_exactly_the_operations_that_declare_the_forbidden_refusal() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model holds a shape map")
            .iter();
        let mut declared: Vec<String> = shapes
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter(|(_, shape)| {
                shape["errors"]
                    .as_array()
                    .is_some_and(|errors| errors.iter().any(|e| e["target"] == FORBIDDEN_SHAPE))
            })
            .map(|(id, _)| {
                id.rsplit('#')
                    .next()
                    .expect("a shape id names a shape")
                    .to_string()
            })
            .collect();
        declared.sort();
        assert!(
            !declared.is_empty(),
            "case the model's surface: expected the operations that declare {FORBIDDEN_SHAPE}, \
             got none"
        );

        let mut governed: Vec<String> = GOVERNED_OPERATIONS
            .iter()
            .map(|operation| (*operation).to_string())
            .collect();
        governed.sort();
        assert_eq!(
            governed, declared,
            "case the governed set: expected {declared:?}, got {governed:?}"
        );
        for operation in &declared {
            assert!(
                governs(operation),
                "case {operation}: the model declares the forbidden refusal for it, so a web \
                 ACL decides it"
            );
        }
        // One operation the model declares no such refusal for: the admin half of the
        // API is called with credentials and is not decided by the pool's web ACL.
        assert!(
            !governs("AdminInitiateAuth"),
            "case AdminInitiateAuth: the model declares no forbidden refusal for it, so no web \
             ACL decides it"
        );
    }

    /// [`split_uri`] over the request lines that reach this service: the path every
    /// `awsJson` call posts to, an absolute-form request line, a path carrying a query
    /// string, and an authority with no path at all.
    #[test]
    fn a_request_line_is_read_as_a_path_and_a_query_string() {
        // (case, uri, path, query string)
        let rows: &[(&str, &str, &str, &str)] = &[
            ("the path every awsJson call posts to", "/", "/", ""),
            (
                "an absolute request line",
                "https://cognito-idp.us-east-1.amazonaws.com/",
                "/",
                "",
            ),
            (
                "a path carrying a query string",
                "/sign-in?flow=password&retry=1",
                "/sign-in",
                "flow=password&retry=1",
            ),
            (
                "an absolute request line carrying a query string",
                "https://example.test/pools/1?list=all",
                "/pools/1",
                "list=all",
            ),
            (
                "an authority with no path",
                "https://cognito-idp.us-east-1.amazonaws.com",
                "/",
                "",
            ),
            ("an empty query string", "/?", "/", ""),
        ];

        for (case, uri, path, query) in rows {
            let actual = split_uri(uri);
            assert_eq!(
                actual,
                (*path, *query),
                "case {case:?}: expected {:?}, got {actual:?}",
                (path, query)
            );
        }
    }
}
