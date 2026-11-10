// SPDX-License-Identifier: CC0-1.0

//! What EventBridge SENDS when it invokes an API destination: the credentials
//! the connection holds, and the parameters the connection and the rule's
//! target each contribute to the request.
//!
//! "A connection defines the authorization type and credentials to use for
//! authorization with an API destination HTTP endpoint" (`CreateConnection`),
//! and both the connection and the target may carry header, query-string and
//! body parameters of their own (`ConnectionHttpParameters`,
//! `Target$HttpParameters`). The rule: the invocation carries the connection's
//! authorization, then both sets of parameters merged, and the CONNECTION's
//! value wins where the two name the same key — the connection is what
//! authorizes the call, so a target cannot override what it sends.

use ::std::collections::BTreeMap;

use crate::ops::types::HttpParameters;

use super::*;

/// An `ApiKeyAuthParameters` credential: the header name EventBridge sends the
/// key in, and the key itself.
#[derive(Clone)]
pub struct ApiKeyCredential {
    pub name: String,
    pub value: String,
}

/// A `BasicAuthParameters` credential: the user name and the password the
/// `Authorization: Basic` header is built from.
#[derive(Clone)]
pub struct BasicCredential {
    pub username: String,
    pub password: String,
}

/// Every credential type below renders as WHAT it is and never as what it
/// holds, so no formatting of a connection — a log line, a trace field, a
/// panic message — can put a credential anywhere but in the request it
/// authorizes. The non-secret half of each parameter block is what a read
/// reports ([`ConnectionRecord::auth_parameters`]), and that is where to look
/// for a value.
impl ::std::fmt::Debug for ApiKeyCredential {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("ApiKeyCredential")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl ::std::fmt::Debug for BasicCredential {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("BasicCredential")
            .finish_non_exhaustive()
    }
}

/// The credential half of a connection's authorization parameters — what AWS
/// keeps in the secret `SecretArn` names.
///
/// It is held beside the half a read may report
/// ([`ConnectionRecord::auth_parameters`], which carries a user name, an API key
/// NAME and a client id and never a password, key value or client secret)
/// because an INVOCATION has to carry it: EventBridge sends the connection's
/// credentials and its `InvocationHttpParameters` with every request it makes to
/// a destination. No operation of this service reports it: `DescribeConnection`
/// and `ListConnections` serialize the reported half only, and nothing reads
/// this back out over the wire.
#[derive(Clone, Default)]
pub struct ConnectionSecret {
    /// The API key the invocation carries, for an `API_KEY` connection.
    pub api_key: Option<ApiKeyCredential>,
    /// The credentials the `Authorization: Basic` header is built from, for a
    /// `BASIC` connection.
    pub basic: Option<BasicCredential>,
    /// The client credentials the token request is made with, for an
    /// `OAUTH_CLIENT_CREDENTIALS` connection ([`super::oauth`]). They are not
    /// themselves an authorization an invocation can carry — they are exchanged
    /// for one.
    pub oauth: Option<super::oauth::OAuthCredential>,
    /// The access token the connection's last client-credentials exchange
    /// issued, which is what an invocation through an OAuth connection carries
    /// ([`ConnectionSecret::authorization_header`]) — `None` for a connection
    /// that holds none, which is a connection no invocation is authorized
    /// through.
    pub oauth_token: Option<String>,
    /// `InvocationHttpParameters` as the request gave them, the values it
    /// flagged secret included: "additional parameters for the connection that
    /// are passed through with every invocation to the HTTP endpoint".
    pub invocation_http_parameters: Option<ConnectionHttpParameters>,
}

/// Which credentials this secret holds, and never one of their values — the
/// reason the credential types above render the same way.
impl ::std::fmt::Debug for ConnectionSecret {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("ConnectionSecret")
            .field("api_key", &self.api_key)
            .field("basic", &self.basic)
            .field("oauth", &self.oauth)
            .field(
                "oauth_token",
                &self
                    .oauth_token
                    .as_ref()
                    .map(|_| "<the token the exchange issued>"),
            )
            .field(
                "invocation_http_parameters",
                &self
                    .invocation_http_parameters
                    .as_ref()
                    .map(|_| "<the parameters the invocation carries>"),
            )
            .finish()
    }
}

impl ConnectionSecret {
    /// The authorization header an invocation through this connection carries,
    /// or `None` for a connection that holds no credential to send: one of an
    /// `OAUTH_CLIENT_CREDENTIALS` type whose exchange issued no token, and one
    /// `DeauthorizeConnection` stripped.
    ///
    /// "API_KEY ... BASIC": an api-key connection sends its key under the
    /// header NAME the connection names, and a basic connection sends the
    /// standard `Authorization: Basic <base64(user:password)>` the HTTP basic
    /// scheme defines (RFC 7617), encoded by the `base64` crate rather than by
    /// hand. An OAuth connection sends the token its client-credentials grant
    /// was exchanged for, under the bearer scheme (RFC 6750 §2.1) — the
    /// credentials themselves are never sent to the destination.
    pub fn authorization_header(&self) -> Option<(String, String)> {
        use ::base64::Engine as _;

        if let Some(key) = self.api_key.as_ref() {
            return Some((key.name.clone(), key.value.clone()));
        }
        if let Some(token) = self.oauth_token.as_ref() {
            return Some((
                ::http::header::AUTHORIZATION.to_string(),
                format!("{} {token}", super::oauth::BEARER_SCHEME),
            ));
        }
        let basic = self.basic.as_ref()?;
        let credentials = ::base64::engine::general_purpose::STANDARD
            .encode(format!("{}:{}", basic.username, basic.password));
        Some((
            ::http::header::AUTHORIZATION.to_string(),
            format!("Basic {credentials}"),
        ))
    }
}

/// The HTTP request ONE invocation of an API destination is made as: where it
/// goes, what it carries in its headers, and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The destination's `InvocationEndpoint` with its path wildcards filled
    /// and the merged query-string parameters on it.
    pub uri: String,
    /// The headers the request carries, in name order — the connection's
    /// authorization among them.
    pub headers: Vec<(String, String)>,
    /// The body the request carries: the event, with the connection's body
    /// parameters merged in.
    pub body: String,
}

/// Build the invocation of the destination at `endpoint`, carrying `body`.
///
/// The rule: the destination's `InvocationEndpoint` may hold `*` wildcards in
/// its PATH, which the target's `HttpParameters$PathParameterValues` fill in
/// order; the target's and the connection's `QueryStringParameters` and
/// `HeaderParameters` are both sent, and the connection's value wins where both
/// name one key; the connection's `BodyParameters` are added to the JSON body;
/// and the connection's authorization header is sent with every request
/// ([`ConnectionSecret::authorization_header`]).
///
/// A header name is matched case-INSENSITIVELY, because an HTTP header name is
/// (RFC 9110): one side spelling a header `X-Shared` and the other `x-shared`
/// have named ONE header, so the connection's value wins there too and the
/// request carries that header once. `content_type` and `user_agent` are the
/// values EventBridge sends unless a parameter names those headers, so they are
/// written as the defaults the merge may replace rather than beside it.
///
/// Nothing here is specific to one destination or one target: a target carrying
/// no `HttpParameters` contributes none, and a connection holding no
/// `InvocationHttpParameters` contributes none.
pub fn api_destination_invocation(
    endpoint: &str,
    secret: &ConnectionSecret,
    target: Option<&HttpParameters>,
    body: &str,
    content_type: &str,
    user_agent: &str,
) -> Invocation {
    let connection = secret.invocation_http_parameters.as_ref();
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    let mut query: BTreeMap<String, String> = BTreeMap::new();
    // The headers of the request itself, which a parameter of either side may
    // name and thereby replace.
    headers.insert(
        ::http::header::CONTENT_TYPE.to_string(),
        content_type.to_string(),
    );
    headers.insert(
        ::http::header::USER_AGENT.to_string(),
        user_agent.to_string(),
    );
    // The target's parameters next, so the connection's overwrite them: the
    // connection authorizes the call, so what it sends is not overridden.
    if let Some(target) = target {
        for (key, value) in target.header_parameters.iter().flatten() {
            headers.insert(header_name(key), value.clone());
        }
        for (key, value) in target.query_string_parameters.iter().flatten() {
            query.insert(key.clone(), value.clone());
        }
    }
    for parameter in connection
        .and_then(|parameters| parameters.header_parameters.as_ref())
        .into_iter()
        .flatten()
    {
        if let (Some(key), Some(value)) = (parameter.key.as_ref(), parameter.value.as_ref()) {
            headers.insert(header_name(key), value.clone());
        }
    }
    for parameter in connection
        .and_then(|parameters| parameters.query_string_parameters.as_ref())
        .into_iter()
        .flatten()
    {
        if let (Some(key), Some(value)) = (parameter.key.as_ref(), parameter.value.as_ref()) {
            query.insert(key.clone(), value.clone());
        }
    }
    // The authorization is the connection's own and is not a parameter either
    // side may name: it is written last.
    if let Some((name, value)) = secret.authorization_header() {
        headers.insert(header_name(&name), value);
    }

    let path_values: Vec<String> = target
        .and_then(|parameters| parameters.path_parameter_values.clone())
        .unwrap_or_default();
    Invocation {
        uri: with_query_parameters(&filled_path(endpoint, &path_values), &query),
        headers: headers.into_iter().collect(),
        body: with_body_parameters(
            body,
            connection.and_then(|parameters| parameters.body_parameters.as_ref()),
        ),
    }
}

/// `name` as one header name among the others: lower case, because an HTTP
/// header name is case-insensitive (RFC 9110), so two spellings of one name are
/// one key of the merge and the request carries that header once.
fn header_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// `endpoint` with each `*` of its PATH replaced, in order, by the next of
/// `values`.
///
/// "The path parameter values to be used to populate API Gateway REST API or
/// EventBridge ApiDestination path wildcards ('*')"
/// (`HttpParameters$PathParameterValues`): a wildcard with no value left for it
/// is left as it stands, because nothing says what it stands for, and a value
/// with no wildcard left to fill is not appended to the path.
fn filled_path(endpoint: &str, values: &[String]) -> String {
    // The query and fragment are not the path: a `*` in either is not a path
    // wildcard.
    let (path, rest) = match endpoint.find(['?', '#']) {
        Some(at) => endpoint.split_at(at),
        None => (endpoint, ""),
    };
    let mut filled = String::with_capacity(path.len());
    let mut values = values.iter();
    let mut segments = path.split('*').peekable();
    while let Some(segment) = segments.next() {
        filled.push_str(segment);
        // A segment that is not the last is followed by a wildcard.
        if segments.peek().is_some() {
            match values.next() {
                Some(value) => filled.push_str(value),
                None => filled.push('*'),
            }
        }
    }
    format!("{filled}{rest}")
}

/// `uri` with `parameters` on its query string, appended to whatever query the
/// endpoint already carried.
///
/// The values are percent-encoded by the `percent-encoding` crate, under its
/// own query set, so a parameter carrying a space, an `&` or a non-ASCII
/// character travels as one value rather than breaking the query it is written
/// into.
///
/// Both requests this service makes on the account's behalf build their query
/// string here — an api-destination invocation and a connection's token exchange
/// ([`super::oauth`]) — so one parameter is encoded one way whichever request
/// carries it.
pub fn with_query_parameters(uri: &str, parameters: &BTreeMap<String, String>) -> String {
    /// The characters a query-string value is encoded away from: the delimiters
    /// of the query itself, plus the ones RFC 3986 does not admit in a URI at
    /// all.
    const QUERY: &::percent_encoding::AsciiSet = &::percent_encoding::CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'&')
        .add(b'+')
        .add(b'<')
        .add(b'=')
        .add(b'>')
        .add(b'?')
        .add(b'`');

    if parameters.is_empty() {
        return uri.to_string();
    }
    let encode =
        |text: &str| ::percent_encoding::utf8_percent_encode(text, QUERY).to_string();
    let rendered: Vec<String> = parameters
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect();
    let separator = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{separator}{}", rendered.join("&"))
}

/// `body` with each of the connection's `BodyParameters` added to it.
///
/// "Additional parameters for the connection that are passed through with every
/// invocation to the HTTP endpoint" (`ConnectionHttpParameters$BodyParameters`),
/// and what EventBridge sends an api-destination invocation is a JSON document,
/// so a body parameter is a MEMBER of it. A body that is not a JSON object has
/// no members to add to and is sent exactly as the delivery built it.
fn with_body_parameters(body: &str, parameters: Option<&Vec<ConnectionBodyParameter>>) -> String {
    let Some(parameters) = parameters.filter(|list| !list.is_empty()) else {
        return body.to_string();
    };
    let Ok(::serde_json::Value::Object(mut members)) =
        ::serde_json::from_str::<::serde_json::Value>(body)
    else {
        return body.to_string();
    };
    for parameter in parameters {
        if let (Some(key), Some(value)) = (parameter.key.as_ref(), parameter.value.as_ref()) {
            members.insert(key.clone(), ::serde_json::Value::String(value.clone()));
        }
    }
    ::serde_json::Value::Object(members).to_string()
}
