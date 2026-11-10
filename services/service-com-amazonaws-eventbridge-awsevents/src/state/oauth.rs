// SPDX-License-Identifier: CC0-1.0

//! The CLIENT-CREDENTIALS grant an `OAUTH_CLIENT_CREDENTIALS` connection
//! authorizes with: the request EventBridge makes at the connection's
//! authorization endpoint, and the access token that request answers with.
//!
//! The rule the service follows: "OAUTH tokens are refreshed when a 401 or 407
//! response is returned" (`CreateConnection$AuthorizationType`), so such a
//! connection's client credentials are NOT themselves the authorization — they
//! are exchanged for a token at the connection's `AuthorizationEndpoint` with
//! its `HttpMethod`, and what every invocation through the connection then
//! carries is that token
//! ([`super::invocation::ConnectionSecret::authorization_header`]).
//!
//! The exchange is the OAuth 2.0 client-credentials grant (RFC 6749 §4.4.2): an
//! `application/x-www-form-urlencoded` body whose fields are `client_id` and
//! `client_secret` from `ClientParameters` beside the connection's own
//! `OAuthHttpParameters$BodyParameters`, with that block's
//! `QueryStringParameters` on the query string and its `HeaderParameters` as
//! request headers. A successful exchange answers a token response
//! (RFC 6749 §5.1), and its `access_token` member is the credential the
//! connection holds from then on; anything else — a non-2xx answer, or a 2xx
//! carrying no token — leaves the connection holding none.
//!
//! The request goes through the IN-PROCESS emulator transport and nothing else,
//! like every other request this service makes on the account's behalf: no
//! socket is opened and no name is resolved, so an authorization endpoint this
//! emulator serves nothing at answers the transport's own refusal, which is an
//! exchange that reached no endpoint ([`super::ENDPOINT_NOT_REACHED`]).

use super::*;

/// The credential half of an `OAUTH_CLIENT_CREDENTIALS` connection's
/// authorization parameters: everything the token request is made of.
///
/// It is held beside the half a read may report
/// ([`ConnectionRecord::auth_parameters`], which carries a client id and never a
/// client secret) because the EXCHANGE has to carry it — a token request without
/// the client secret is not the grant — and the values the request flagged secret
/// are kept exactly as they were given, because the authorization endpoint is
/// sent those values and not their keys alone. No operation of this service
/// reports it.
#[derive(Clone)]
pub struct OAuthCredential {
    /// `ConnectionOAuthClientRequestParameters$ClientID`.
    pub client_id: String,
    /// `ConnectionOAuthClientRequestParameters$ClientSecret`.
    pub client_secret: String,
    /// `AuthorizationEndpoint`, the URL the token request is made against.
    pub authorization_endpoint: String,
    /// One of `ConnectionOAuthHttpMethod`'s values, the method it is made with.
    pub http_method: String,
    /// `OAuthHttpParameters`, as the request gave them: "additional parameters
    /// for the connection that are passed through with every invocation to the
    /// HTTP endpoint" applied to the token request.
    pub http_parameters: Option<ConnectionHttpParameters>,
}

/// Which parameters the credential holds, and never one of their values — the
/// reason every credential type of this service renders that way
/// ([`super::invocation::ConnectionSecret`]).
impl ::std::fmt::Debug for OAuthCredential {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("OAuthCredential")
            .field("client_id", &self.client_id)
            .field("authorization_endpoint", &self.authorization_endpoint)
            .field("http_method", &self.http_method)
            .finish_non_exhaustive()
    }
}

/// The scheme the authorization header of a bearer token names it under
/// (RFC 6750 §2.1), which is what EventBridge sends an invocation of an API
/// destination behind an OAuth connection with.
pub const BEARER_SCHEME: &str = "Bearer";

/// The form field the client-credentials grant carries the client id in
/// (RFC 6749 §2.3.1).
const CLIENT_ID_FIELD: &str = "client_id";

/// The form field it carries the client secret in (RFC 6749 §2.3.1).
const CLIENT_SECRET_FIELD: &str = "client_secret";

/// The media type the grant's body is sent as (RFC 6749 §4.4.2: "the client
/// makes a request to the token endpoint ... with ... the
/// `application/x-www-form-urlencoded` format").
const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// The member of a token response that carries the token (RFC 6749 §5.1:
/// "`access_token` — REQUIRED. The access token issued by the authorization
/// server").
const ACCESS_TOKEN_MEMBER: &str = "access_token";

/// What a connection reports as its `StateReason` when its authorization
/// endpoint answered without issuing a token at all.
pub const NO_TOKEN_ISSUED: &str =
    "The authorization endpoint answered no access token, so the connection holds none.";

/// Obtain the access token `credential` authorizes with, answering
/// `Err(reason)` — the `StateReason` the connection then reports — when the
/// exchange did not produce one.
///
/// The request is the client-credentials grant the module docs state. Every value
/// in it comes from the stored credential, so an endpoint, a method or a
/// parameter that cannot be rendered into a request fails HERE rather than
/// panicking.
pub async fn client_credentials_token(
    transport: &::aws_smithy_runtime_api::client::http::SharedHttpConnector,
    credential: &OAuthCredential,
) -> Result<String, String> {
    use ::aws_smithy_runtime_api::client::http::HttpConnector;

    let named_method = if credential.http_method.is_empty() {
        AUTHORIZATION_EXCHANGE_METHOD
    } else {
        credential.http_method.as_str()
    };
    let Ok(method) = ::http::Method::from_bytes(named_method.as_bytes()) else {
        return Err(format!(
            "The connection's authorization HttpMethod {named_method:?} is not an HTTP method."
        ));
    };
    let parameters = credential.http_parameters.as_ref();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(token_request_uri(credential))
        .header(::http::header::CONTENT_TYPE, FORM_CONTENT_TYPE);
    // "HeaderParameters": the headers the exchange carries beside the content
    // type the grant itself is sent as.
    for parameter in parameters
        .and_then(|parameters| parameters.header_parameters.as_ref())
        .into_iter()
        .flatten()
    {
        if let (Some(key), Some(value)) = (parameter.key.as_ref(), parameter.value.as_ref()) {
            builder = builder.header(key, value);
        }
    }
    let request = builder
        .body(::aws_smithy_types::body::SdkBody::from(
            ::bytes::Bytes::from(grant_body(credential)),
        ))
        .map_err(|error| error.to_string())
        .and_then(|request| {
            ::aws_smithy_runtime_api::http::Request::try_from(request)
                .map_err(|error| error.to_string())
        });
    let request = match request {
        Ok(request) => request,
        Err(_) => return Err(ENDPOINT_NOT_REACHED.to_string()),
    };
    match ::tokio::time::timeout(AUTHORIZATION_EXCHANGE_TIMEOUT, transport.call(request)).await {
        // Nothing answered inside the timeout, or the endpoint could not be
        // reached at all, or this emulator serves nothing at it — one condition
        // as the connection reports it: the endpoint was not reached.
        Err(_) | Ok(Err(_)) => Err(ENDPOINT_NOT_REACHED.to_string()),
        Ok(Ok(response))
            if response
                .body()
                .bytes()
                .is_some_and(is_transport_refusal) =>
        {
            Err(ENDPOINT_NOT_REACHED.to_string())
        }
        Ok(Ok(response)) if (200..=299).contains(&response.status().as_u16()) => {
            // The grant succeeded only if the answer carries a token: a 2xx
            // without one has issued nothing, so the connection holds no
            // credential and says so.
            issued_token(response.body().bytes().unwrap_or_default())
                .ok_or_else(|| NO_TOKEN_ISSUED.to_string())
        }
        Ok(Ok(response)) => Err(format!(
            "The authorization endpoint answered HTTP {} instead of issuing a token.",
            response.status().as_u16()
        )),
    }
}

/// The URI the token request is made against: the connection's
/// `AuthorizationEndpoint` with the `QueryStringParameters` of its
/// `OAuthHttpParameters` appended to whatever query the endpoint already carried.
///
/// The appending is [`super::invocation::with_query_parameters`], the same one an
/// api-destination invocation's query string is built by, so a value carrying a
/// space or an `&` is encoded one way for both requests this service makes.
fn token_request_uri(credential: &OAuthCredential) -> String {
    let query: BTreeMap<String, String> = credential
        .http_parameters
        .as_ref()
        .and_then(|parameters| parameters.query_string_parameters.as_ref())
        .into_iter()
        .flatten()
        .filter_map(|parameter| {
            Some((parameter.key.clone()?, parameter.value.clone()?))
        })
        .collect();
    invocation::with_query_parameters(&credential.authorization_endpoint, &query)
}

/// The grant's body: `client_id`, `client_secret` and every `BodyParameters`
/// entry of the connection's `OAuthHttpParameters`, form-encoded.
///
/// The encoding is the `form_urlencoded` crate's, rather than a hand-rolled one,
/// so a client id, secret or parameter value carrying a reserved character
/// travels as one field.
fn grant_body(credential: &OAuthCredential) -> String {
    let mut form = ::form_urlencoded::Serializer::new(String::new());
    form.append_pair(CLIENT_ID_FIELD, &credential.client_id);
    form.append_pair(CLIENT_SECRET_FIELD, &credential.client_secret);
    for parameter in credential
        .http_parameters
        .as_ref()
        .and_then(|parameters| parameters.body_parameters.as_ref())
        .into_iter()
        .flatten()
    {
        if let (Some(key), Some(value)) = (parameter.key.as_ref(), parameter.value.as_ref()) {
            form.append_pair(key, value);
        }
    }
    form.finish()
}

/// The access token `body` carries, or `None` when it carries none: a token
/// response is a JSON object whose `access_token` member is the token
/// (RFC 6749 §5.1), and an empty one is no token.
fn issued_token(body: &[u8]) -> Option<String> {
    let answer = ::serde_json::from_slice::<::serde_json::Value>(body).ok()?;
    let token = answer.get(ACCESS_TOKEN_MEMBER)?.as_str()?;
    (!token.is_empty()).then(|| token.to_string())
}
