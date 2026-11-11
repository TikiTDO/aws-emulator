// SPDX-License-Identifier: CC0-1.0

//! The documented PAYLOAD FORMAT 2.0 of a function URL: the event a request becomes, and
//! the response a handler's result becomes.
//!
//! "Lambda function URLs use the payload format version 2.0" — so the two halves of this
//! module are the two halves of that contract, and nothing about either depends on the
//! request this emulator happens to be serving:
//!
//! * [`url_event`] maps a request onto the event document the handler is given: `version`,
//!   `routeKey`, `rawPath`, `rawQueryString`, the request's headers LOWER-CASED (duplicates
//!   joined, which is how one header name carrying several values travels in a JSON object),
//!   the `cookies` array the `Cookie` header is split into, `queryStringParameters`, the
//!   `requestContext` describing the call, the `body`, and `isBase64Encoded`, which says
//!   whether that body is the bytes themselves or their base64.
//! * [`mapped_result`] maps what the handler RETURNED onto the HTTP response: a result that
//!   names any of `statusCode`, `headers`, `cookies`, `body` or `isBase64Encoded` is the
//!   response it describes, and any other value is the response Lambda INFERS — "If your
//!   function returns valid JSON and doesn't return a statusCode, Lambda assumes" a 200
//!   with `application/json` and that value as the body. A result that describes a response
//!   it cannot be ([`Mapped::Malformed`]) is answered by the plane as the bad gateway it is;
//!   nothing here invents a status or a body the function did not produce.

use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::{json, Map, Value};

/// The payload format version this plane speaks, which is the one a function URL uses.
const PAYLOAD_FORMAT_VERSION: &str = "2.0";

/// The route key and stage of every function URL request: a function URL has one route and
/// one stage, which the format names `$default`.
const DEFAULT_ROUTE_KEY: &str = "$default";

/// The `content-type` a response that names none carries: "Lambda assumes […] content-type
/// is application/json".
pub const DEFAULT_CONTENT_TYPE: &str = "application/json";

/// The media types whose body reaches the handler VERBATIM, as the format's
/// `isBase64Encoded` divides them: text, and the structured types written as text. Every
/// other media type — and a body arriving under no media type at all — is handed over
/// base64-encoded, so the handler is given exactly the bytes the client sent whatever they
/// are.
const TEXT_MEDIA_TYPES: &[&str] = &[
    "application/json",
    "application/xml",
    "application/javascript",
    "application/x-javascript",
];

/// The caller a request was signed by, for a URL whose `AuthType` is `AWS_IAM`.
#[derive(Debug, Clone)]
pub struct CallerIdentity {
    /// The access key id the request presented — the public half of the credential, which
    /// the format reports as `requestContext.authorizer.iam.accessKey`.
    pub access_key: String,
    /// The account the credential belongs to.
    pub account_id: String,
    /// The ARN of the principal that credential speaks for.
    pub user_arn: String,
    /// That principal's unique id.
    pub user_id: String,
}

/// One request to a function URL, as the plane read it off the wire.
#[derive(Debug)]
pub struct UrlRequest<'r> {
    /// The request method.
    pub method: &'r str,
    /// The path within the URL.
    pub raw_path: &'r str,
    /// The query string as received, without its `?`.
    pub raw_query: &'r str,
    /// The request's headers, in the order they arrived, names as received.
    pub headers: &'r [(String, String)],
    /// The request body, exactly as it arrived.
    pub body: &'r [u8],
    /// The media type the body arrived under, when it named one.
    pub content_type: Option<&'r str>,
    /// The url id the request addressed, which is the URL's own subdomain label.
    pub url_id: &'r str,
    /// The host the function URL publishes, which the format reports as `domainName`.
    pub domain_name: &'r str,
    /// The account the function belongs to.
    pub account_id: &'r str,
    /// This request's id, which is also the invocation's.
    pub request_id: &'r str,
    /// The address the request came from.
    pub source_ip: &'r str,
    /// When the request arrived.
    pub received: ::std::time::SystemTime,
    /// The caller, for a URL that authorizes callers by IAM.
    pub iam: Option<CallerIdentity>,
}

/// The payload-format-2.0 event `request` becomes.
#[must_use]
pub fn url_event(request: &UrlRequest<'_>) -> Vec<u8> {
    let mut event = Map::new();
    event.insert("version".to_string(), json!(PAYLOAD_FORMAT_VERSION));
    event.insert("routeKey".to_string(), json!(DEFAULT_ROUTE_KEY));
    event.insert("rawPath".to_string(), json!(request.raw_path));
    event.insert("rawQueryString".to_string(), json!(request.raw_query));
    let cookies = cookies_of(request.headers);
    if !cookies.is_empty() {
        event.insert("cookies".to_string(), json!(cookies));
    }
    event.insert("headers".to_string(), json!(event_headers(request.headers)));
    let query = query_parameters(request.raw_query);
    if !query.is_empty() {
        event.insert("queryStringParameters".to_string(), json!(query));
    }
    event.insert("requestContext".to_string(), request_context(request));
    if !request.body.is_empty() {
        let (body, base64) = event_body(request.body, request.content_type);
        event.insert("body".to_string(), json!(body));
        event.insert("isBase64Encoded".to_string(), json!(base64));
    } else {
        event.insert("isBase64Encoded".to_string(), json!(false));
    }
    Value::Object(event).to_string().into_bytes()
}

/// The event's `headers`: every header name LOWER-CASED, with the values of a name that
/// arrived more than once joined by `,` — a JSON object holds one value per name, and
/// joining is how the format carries the rest.
///
/// The `Cookie` header is among them, exactly as the client sent it, beside the `cookies`
/// array it is also split into ([`cookies_of`]): a function URL reports the request's headers
/// whole, so a handler that reads the one header and a handler that reads the array both see
/// the cookies the request carried.
fn event_headers(headers: &[(String, String)]) -> BTreeMap<String, String> {
    let mut mapped: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in headers {
        let name = name.to_ascii_lowercase();
        mapped
            .entry(name)
            .and_modify(|held| {
                held.push(',');
                held.push_str(value);
            })
            .or_insert_with(|| value.clone());
    }
    mapped
}

/// The header a client sends its cookies in.
const COOKIE_HEADER: &str = "cookie";

/// The event's `cookies`: one entry per cookie the request's `Cookie` header(s) carried, in
/// the order they arrived, each still the `name=value` pair the client sent.
fn cookies_of(headers: &[(String, String)]) -> Vec<String> {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(COOKIE_HEADER))
        .flat_map(|(_, value)| value.split(';'))
        .map(str::trim)
        .filter(|cookie| !cookie.is_empty())
        .map(str::to_string)
        .collect()
}

/// The event's `queryStringParameters`: the query's pairs, with the values of a key that
/// appears more than once joined by `,` for the reason [`event_headers`] joins header
/// values. Still percent-ENCODED values are decoded, because the format reports the
/// parameter's value and `rawQueryString` is where the raw form is reported.
fn query_parameters(raw_query: &str) -> BTreeMap<String, String> {
    let mut parameters: BTreeMap<String, String> = BTreeMap::new();
    for pair in raw_query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decoded(key);
        let value = percent_decoded(value);
        parameters
            .entry(key)
            .and_modify(|held| {
                held.push(',');
                held.push_str(&value);
            })
            .or_insert(value);
    }
    parameters
}

/// One percent-encoded query component, decoded, with `+` read as the space a form-encoded
/// query writes it as.
fn percent_decoded(component: &str) -> String {
    ::smithy_server_runtime::percent_decode(&component.replace('+', " "))
}

/// The event's `requestContext`: what the format reports about the call itself, every field
/// of it read off this request or off the resource it addressed.
fn request_context(request: &UrlRequest<'_>) -> Value {
    let mut http = Map::new();
    http.insert("method".to_string(), json!(request.method));
    http.insert("path".to_string(), json!(request.raw_path));
    http.insert("protocol".to_string(), json!(HTTP_PROTOCOL));
    http.insert("sourceIp".to_string(), json!(request.source_ip));
    http.insert(
        "userAgent".to_string(),
        json!(header_value(request.headers, "user-agent").unwrap_or_default()),
    );
    let mut context = Map::new();
    context.insert("accountId".to_string(), json!(request.account_id));
    context.insert("apiId".to_string(), json!(request.url_id));
    if let Some(iam) = &request.iam {
        context.insert(
            "authorizer".to_string(),
            json!({
                "iam": {
                    "accessKey": iam.access_key,
                    "accountId": iam.account_id,
                    "callerId": iam.user_id,
                    "userArn": iam.user_arn,
                    "userId": iam.user_id,
                }
            }),
        );
    }
    context.insert("domainName".to_string(), json!(request.domain_name));
    context.insert("domainPrefix".to_string(), json!(request.url_id));
    context.insert("http".to_string(), Value::Object(http));
    context.insert("requestId".to_string(), json!(request.request_id));
    context.insert("routeKey".to_string(), json!(DEFAULT_ROUTE_KEY));
    context.insert("stage".to_string(), json!(DEFAULT_ROUTE_KEY));
    context.insert("time".to_string(), json!(clf_time(request.received)));
    context.insert("timeEpoch".to_string(), json!(epoch_millis(request.received)));
    Value::Object(context)
}

/// The protocol a function URL request is served over.
const HTTP_PROTOCOL: &str = "HTTP/1.1";

/// The first value of the header `name`, when the request carried it.
fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// The event's `body` and `isBase64Encoded`: the body as TEXT when it arrived under a media
/// type the format carries as text, and base64 otherwise — including a body that is not
/// valid UTF-8, which no JSON string can carry as itself.
fn event_body(body: &[u8], content_type: Option<&str>) -> (String, bool) {
    if textual(content_type) {
        if let Ok(text) = ::std::str::from_utf8(body) {
            return (text.to_string(), false);
        }
    }
    (
        ::base64::engine::general_purpose::STANDARD.encode(body),
        true,
    )
}

/// Whether a body arriving under `content_type` travels as text: `text/*`, one of
/// [`TEXT_MEDIA_TYPES`], or a structured syntax written as one of them (`+json`, `+xml`).
/// A body under no media type at all is not read as text.
fn textual(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return false;
    };
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    media_type.starts_with("text/")
        || TEXT_MEDIA_TYPES.contains(&media_type.as_str())
        || media_type.ends_with("+json")
        || media_type.ends_with("+xml")
}

/// The instant `at` in the Common Log Format `requestContext.time` carries
/// (`31/Oct/2026:12:00:00 +0000`).
fn clf_time(at: ::std::time::SystemTime) -> String {
    let at = ::aws_smithy_types::DateTime::from(at);
    let rendered = at
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    // `2026-10-31T21:10:43Z` -> the day, the month's name, and the time of day.
    let (date, rest) = rendered.split_once('T').unwrap_or((rendered.as_str(), ""));
    let time = rest.trim_end_matches('Z');
    let mut parts = date.split('-');
    let (year, month, day) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    let month = month
        .parse::<usize>()
        .ok()
        .and_then(|month| MONTH_NAMES.get(month.saturating_sub(1)).copied())
        .unwrap_or_default();
    format!("{day}/{month}/{year}:{time} +0000")
}

/// The month names the Common Log Format writes, January first.
const MONTH_NAMES: &[&str] = &[
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The instant `at` as the milliseconds since the epoch `requestContext.timeEpoch` carries.
fn epoch_millis(at: ::std::time::SystemTime) -> i64 {
    at.duration_since(::std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or_default()
}

/// What a handler's result maps onto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mapped {
    /// The response the result describes, or the one Lambda infers for a result that
    /// describes none.
    Response {
        /// The HTTP status to answer.
        status: u16,
        /// The response headers, names lower-cased, in the order they are answered — one
        /// `set-cookie` per entry of the result's `cookies`.
        headers: Vec<(String, String)>,
        /// The response body.
        body: Vec<u8>,
    },
    /// The result does not describe a response at all; the string says what about it does
    /// not, for the log of the run.
    Malformed(String),
}

/// The lowest and highest status a response may carry: a result naming anything outside the
/// HTTP status range describes no response.
const MIN_STATUS: i64 = 100;
/// See [`MIN_STATUS`].
const MAX_STATUS: i64 = 599;

/// The status a result that names none carries: "Lambda assumes […] statusCode is 200".
const INFERRED_STATUS: u16 = 200;

/// The members whose presence makes a result the DESCRIPTION of a response rather than the
/// response's body.
const RESPONSE_MEMBERS: &[&str] = &[
    "statusCode",
    "headers",
    "cookies",
    "body",
    "isBase64Encoded",
];

/// The response the handler's result `payload` describes.
#[must_use]
pub fn mapped_result(payload: &[u8]) -> Mapped {
    // A function URL answers what the FUNCTION returned, and what a runtime returns is the
    // JSON of the handler's value, so a payload that is not JSON describes no response.
    let Ok(result) = ::serde_json::from_slice::<Value>(payload) else {
        return Mapped::Malformed("the function's response is not valid JSON".to_string());
    };
    let describes_response = result
        .as_object()
        .is_some_and(|members| RESPONSE_MEMBERS.iter().any(|member| members.contains_key(*member)));
    if !describes_response {
        return inferred(&result);
    }
    described(&result)
}

/// The response Lambda INFERS for a result that describes none: "If your function returns
/// valid JSON and doesn't return a statusCode, Lambda assumes […] statusCode is 200,
/// content-type is application/json, body is your function's response, and
/// isBase64Encoded is false."
///
/// A result that IS a string is that string, not its JSON quoting: the body is the
/// function's response, and a string's response is its text.
fn inferred(result: &Value) -> Mapped {
    let body = match result {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    Mapped::Response {
        status: INFERRED_STATUS,
        headers: vec![(
            CONTENT_TYPE_HEADER.to_string(),
            DEFAULT_CONTENT_TYPE.to_string(),
        )],
        body: body.into_bytes(),
    }
}

/// The header a response's media type travels in.
const CONTENT_TYPE_HEADER: &str = "content-type";

/// The response a result DESCRIBES, member by member, or what about it describes none.
fn described(result: &Value) -> Mapped {
    let status = match result.get("statusCode") {
        None => INFERRED_STATUS,
        Some(Value::Number(status)) => match status.as_i64().filter(|status| {
            (MIN_STATUS..=MAX_STATUS).contains(status)
        }) {
            Some(status) => u16::try_from(status).unwrap_or(INFERRED_STATUS),
            None => {
                return Mapped::Malformed(format!(
                    "the function's response names the statusCode {status}, which is not an \
                     HTTP status between {MIN_STATUS} and {MAX_STATUS}"
                ))
            }
        },
        Some(other) => {
            return Mapped::Malformed(format!(
                "the function's response names a statusCode that is not a number: {other}"
            ))
        }
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    match result.get("headers") {
        None => {}
        Some(Value::Object(named)) => {
            for (name, value) in named {
                let Some(rendered) = header_text(value) else {
                    return Mapped::Malformed(format!(
                        "the function's response names the header {name} with a value no header \
                         can carry: {value}"
                    ));
                };
                let name = name.to_ascii_lowercase();
                if !carriable_header(&name, &rendered) {
                    return Mapped::Malformed(format!(
                        "the function's response names a header no HTTP response can carry: \
                         {name}"
                    ));
                }
                headers.push((name, rendered));
            }
        }
        Some(other) => {
            return Mapped::Malformed(format!(
                "the function's response names headers that are not an object: {other}"
            ))
        }
    }
    // "cookies – An array of cookies […] Lambda sends each cookie as a set-cookie header."
    match result.get("cookies") {
        None => {}
        Some(Value::Array(cookies)) => {
            for cookie in cookies {
                let Some(rendered) = header_text(cookie) else {
                    return Mapped::Malformed(format!(
                        "the function's response names a cookie no set-cookie header can carry: \
                         {cookie}"
                    ));
                };
                if !carriable_header(SET_COOKIE_HEADER, &rendered) {
                    return Mapped::Malformed(
                        "the function's response names a cookie no set-cookie header can carry"
                            .to_string(),
                    );
                }
                headers.push((SET_COOKIE_HEADER.to_string(), rendered));
            }
        }
        Some(other) => {
            return Mapped::Malformed(format!(
                "the function's response names cookies that are not an array: {other}"
            ))
        }
    }
    if !headers
        .iter()
        .any(|(name, _)| name == CONTENT_TYPE_HEADER)
    {
        headers.push((
            CONTENT_TYPE_HEADER.to_string(),
            DEFAULT_CONTENT_TYPE.to_string(),
        ));
    }
    let base64 = match result.get("isBase64Encoded") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(base64)) => *base64,
        Some(other) => {
            return Mapped::Malformed(format!(
                "the function's response names an isBase64Encoded that is not a boolean: {other}"
            ))
        }
    };
    let body = match result.get("body") {
        // "body – […] If the body is absent, Lambda returns an empty body."
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(text)) if base64 => {
            match ::base64::engine::general_purpose::STANDARD.decode(text.as_bytes()) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return Mapped::Malformed(
                        "the function's response says its body is base64-encoded, and it is not"
                            .to_string(),
                    )
                }
            }
        }
        Some(Value::String(text)) => text.clone().into_bytes(),
        Some(other) => {
            return Mapped::Malformed(format!(
                "the function's response names a body that is not a string: {other}"
            ))
        }
    };
    Mapped::Response {
        status,
        headers,
        body,
    }
}

/// Whether `name` and `value` are a header an HTTP response can carry at all.
///
/// A result's headers are the HANDLER's, so the name and the value are untrusted text: a
/// name that is no HTTP token, and a value carrying a control character (a CR or LF, which
/// would split the response), name no header. The readings are the `http` crate's own, so
/// what this plane accepts is exactly what the response it builds can carry — the response
/// is then built with the non-panicking appender, and neither half can be reached by a
/// header the other rejects.
fn carriable_header(name: &str, value: &str) -> bool {
    ::smithy_server_runtime::http::HeaderName::from_bytes(name.as_bytes()).is_ok()
        && ::smithy_server_runtime::http::HeaderValue::from_str(value).is_ok()
}

/// The header a cookie of the result travels in.
const SET_COOKIE_HEADER: &str = "set-cookie";

/// The text a result's header or cookie value carries, or `None` for a value no header can
/// carry at all.
fn header_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request with no body, addressed to `path` with the query `query` and the headers
    /// `headers`.
    fn request<'r>(
        path: &'r str,
        query: &'r str,
        headers: &'r [(String, String)],
    ) -> UrlRequest<'r> {
        UrlRequest {
            method: "GET",
            raw_path: path,
            raw_query: query,
            headers,
            body: &[],
            content_type: None,
            url_id: "0123456789abcdef0123456789abcdef",
            domain_name: "0123456789abcdef0123456789abcdef.lambda-url.us-east-1.on.aws",
            account_id: "000000000000",
            request_id: "9b1e0d4e-0000-4000-8000-0000000000ff",
            source_ip: "127.0.0.1",
            received: ::std::time::UNIX_EPOCH + ::std::time::Duration::from_secs(1_793_979_043),
            iam: None,
        }
    }

    /// One header pair, as the plane reads them off the wire.
    fn header(name: &str, value: &str) -> (String, String) {
        (name.to_string(), value.to_string())
    }

    /// The event carries the request's own path, query and headers, and the `Cookie` header
    /// both as a header and as the format's `cookies` array.
    #[test]
    fn the_event_carries_the_request_lower_cased_and_split() {
        let headers = [
            header("Host", "example.lambda-url.us-east-1.on.aws"),
            header("X-Trace", "one"),
            header("x-trace", "two"),
            header("Cookie", "a=1; b=2"),
        ];
        let event: Value = ::serde_json::from_slice(&url_event(&request(
            "/echo/one",
            "q=a%20b&q=c&flag",
            &headers,
        )))
        .expect("the event is JSON");
        let cases: &[(&str, Value)] = &[
            ("version", json!("2.0")),
            ("rawPath", json!("/echo/one")),
            ("rawQueryString", json!("q=a%20b&q=c&flag")),
            ("cookies", json!(["a=1", "b=2"])),
            (
                "headers",
                json!({
                    "host": "example.lambda-url.us-east-1.on.aws",
                    "x-trace": "one,two",
                    "cookie": "a=1; b=2",
                }),
            ),
            (
                "queryStringParameters",
                json!({ "q": "a b,c", "flag": "" }),
            ),
            ("isBase64Encoded", json!(false)),
        ];
        for (member, expected) in cases {
            assert_eq!(
                event.get(*member),
                Some(expected),
                "case {member:?}: expected {expected}, got {:?}",
                event.get(*member)
            );
        }
        let context = event
            .get("requestContext")
            .expect("the event carries a requestContext");
        assert_eq!(
            context.pointer("/http/method"),
            Some(&json!("GET")),
            "case the request context: expected the method, got {context}"
        );
        assert_eq!(
            context.get("time"),
            Some(&json!("06/Nov/2026:15:30:43 +0000")),
            "case the request context: expected the CLF time, got {context}"
        );
    }

    /// A body travels verbatim under a textual media type and base64 under every other one.
    /// The rows span what the rule generalises over: text, the structured text types, a
    /// structured syntax suffix, a parameterised media type, a binary type, and a body
    /// arriving under no media type at all.
    #[test]
    fn a_body_is_base64_encoded_exactly_when_its_media_type_is_not_text() {
        let cases: &[(Option<&str>, bool)] = &[
            (Some("text/plain"), false),
            (Some("text/html; charset=utf-8"), false),
            (Some("application/json"), false),
            (Some("application/xml"), false),
            (Some("application/vnd.api+json"), false),
            (Some("application/x-www-form-urlencoded"), true),
            (Some("application/octet-stream"), true),
            (Some("image/png"), true),
            (None, true),
        ];
        let headers: [(String, String); 0] = [];
        for (content_type, base64) in cases {
            let mut request = request("/", "", &headers);
            request.body = b"name=value";
            request.content_type = *content_type;
            let event: Value =
                ::serde_json::from_slice(&url_event(&request)).expect("the event is JSON");
            assert_eq!(
                event.get("isBase64Encoded"),
                Some(&json!(base64)),
                "case {content_type:?}: expected isBase64Encoded {base64}, got {event}"
            );
            let expected = if *base64 {
                json!("bmFtZT12YWx1ZQ==")
            } else {
                json!("name=value")
            };
            assert_eq!(
                event.get("body"),
                Some(&expected),
                "case {content_type:?}: expected the body {expected}, got {event}"
            );
        }
    }

    /// Every class of result maps onto the response it describes, and a result that
    /// describes none at all is a malformed response rather than a status this plane made
    /// up.
    #[test]
    fn a_result_maps_onto_the_response_it_describes() {
        let cases: &[(&str, &str, Mapped)] = &[
            (
                "a described response",
                r#"{"statusCode":201,"headers":{"Content-Type":"text/plain"},"body":"made"}"#,
                Mapped::Response {
                    status: 201,
                    headers: vec![("content-type".to_string(), "text/plain".to_string())],
                    body: b"made".to_vec(),
                },
            ),
            (
                "a described response carrying cookies",
                r#"{"statusCode":200,"cookies":["a=1","b=2; Path=/"],"body":"ok"}"#,
                Mapped::Response {
                    status: 200,
                    headers: vec![
                        ("set-cookie".to_string(), "a=1".to_string()),
                        ("set-cookie".to_string(), "b=2; Path=/".to_string()),
                        ("content-type".to_string(), "application/json".to_string()),
                    ],
                    body: b"ok".to_vec(),
                },
            ),
            (
                "a described response with no body and no content type",
                r#"{"statusCode":202}"#,
                Mapped::Response {
                    status: 202,
                    headers: vec![("content-type".to_string(), "application/json".to_string())],
                    body: Vec::new(),
                },
            ),
            (
                "a base64 body",
                r#"{"statusCode":200,"body":"AAEC","isBase64Encoded":true}"#,
                Mapped::Response {
                    status: 200,
                    headers: vec![("content-type".to_string(), "application/json".to_string())],
                    body: vec![0, 1, 2],
                },
            ),
            (
                "an inferred response from a document",
                r#"{"ok":true}"#,
                Mapped::Response {
                    status: 200,
                    headers: vec![("content-type".to_string(), "application/json".to_string())],
                    body: br#"{"ok":true}"#.to_vec(),
                },
            ),
            (
                "an inferred response from a string",
                r#""plain""#,
                Mapped::Response {
                    status: 200,
                    headers: vec![("content-type".to_string(), "application/json".to_string())],
                    body: b"plain".to_vec(),
                },
            ),
            (
                "an inferred response from a list",
                r#"[1,2]"#,
                Mapped::Response {
                    status: 200,
                    headers: vec![("content-type".to_string(), "application/json".to_string())],
                    body: b"[1,2]".to_vec(),
                },
            ),
        ];
        for (case, payload, expected) in cases {
            let mapped = mapped_result(payload.as_bytes());
            assert_eq!(
                mapped, *expected,
                "case {case:?}: expected {expected:?}, got {mapped:?}"
            );
        }
    }

    /// A result that describes no response at all is malformed, one row per way of
    /// describing none.
    #[test]
    fn a_result_that_describes_no_response_is_malformed() {
        let cases: &[(&str, &str)] = &[
            ("a status below the range", r#"{"statusCode":99}"#),
            ("a status above the range", r#"{"statusCode":600}"#),
            ("a status that is not a number", r#"{"statusCode":"200"}"#),
            ("headers that are not an object", r#"{"statusCode":200,"headers":[]}"#),
            ("cookies that are not an array", r#"{"statusCode":200,"cookies":"a=1"}"#),
            ("a body that is not a string", r#"{"statusCode":200,"body":{"a":1}}"#),
            (
                "a body that is not the base64 it says it is",
                r#"{"statusCode":200,"body":"not base64!","isBase64Encoded":true}"#,
            ),
            // A header name or value no HTTP response can carry: the handler's own text
            // reaches these, so they are refused at the mapping rather than built into a
            // response.
            (
                "a header name that is no HTTP token",
                r#"{"statusCode":200,"headers":{"bad name":"v"},"body":"x"}"#,
            ),
            (
                "a header value carrying a line break",
                "{\"statusCode\":200,\"headers\":{\"x-trace\":\"one\\r\\nx-injected: two\"},\
                 \"body\":\"x\"}",
            ),
            (
                "a cookie carrying a line break",
                "{\"statusCode\":200,\"cookies\":[\"a=1\\r\\nx-injected: two\"],\"body\":\"x\"}",
            ),
            ("a response that is not JSON at all", "<html/>"),
        ];
        for (case, payload) in cases {
            let mapped = mapped_result(payload.as_bytes());
            assert!(
                matches!(mapped, Mapped::Malformed(_)),
                "case {case:?}: expected a malformed response, got {mapped:?}"
            );
        }
    }
}
