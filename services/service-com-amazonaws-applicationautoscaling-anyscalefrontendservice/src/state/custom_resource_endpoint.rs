// SPDX-License-Identifier: CC0-1.0

//! The endpoint a `custom-resource` scalable target IS, read and written where it lives.
//!
//! Every other namespace this service scales names a resource of another AWS service, whose
//! capacity this service moves through that service ([`super::policy_execution`]). The
//! `custom-resource` namespace names no AWS resource at all: "for a resource provided by your
//! own application or service, use custom-resource instead"
//! (`ServiceNamespace`), and the target's `ResourceId` is the HTTP endpoint that application
//! serves for it — "custom resources are not supported with a resource type […] the unique
//! identifier is defined by the service provider" (the model's own `ResourceId`
//! documentation). So the capacity of such a target is not a value this service holds: it is
//! read from that endpoint and written to it.
//!
//! The contract is the one Application Auto Scaling publishes for a custom resource, and it
//! is two requests on the endpoint the resource id names:
//!
//! * `GET` answers the resource's own capacity, as the `actualCapacity` it is at (falling
//!   back to the `desiredCapacity` it was asked for, for an endpoint that reports only
//!   that); and
//! * `PATCH` with a body of `{"desiredCapacity": <N>}` asks it to move to `N`, which is what
//!   every scaling decision taken for such a target does — a scheduled action firing, a step
//!   policy's alarm breaching, a target-tracking policy's high alarm breaching.
//!
//! Both are made over the IN-PROCESS callback transport
//! (`::smithy_server_runtime::EmbeddedEmulator`), signed as every in-process caller signs
//! (`::service_common::in_process_sigv4`) under the signing name an invoke endpoint
//! authenticates: nothing here opens a socket or leaves the host, and an endpoint this
//! emulator does not serve simply answers nothing, which resolves no capacity rather than
//! failing a decision.

use ::serde_json::Value;

/// The signing name an invoke of an API endpoint is signed under, which is also what an
/// endpoint served by this emulator authenticates a caller's signature against.
const INVOKE_SIGNING_NAME: &str = "execute-api";

/// The media type the capacity document is carried in, both ways.
const APPLICATION_JSON: &str = "application/json";

/// The member of the endpoint's document that reports the capacity the resource IS at.
const ACTUAL_CAPACITY: &str = "actualCapacity";

/// The member that reports the capacity it was last asked for, and the one a request to move
/// it carries. Read as the resource's capacity only where it reports no [`ACTUAL_CAPACITY`].
const DESIRED_CAPACITY: &str = "desiredCapacity";

/// How much of an endpoint's answer this reader copies and parses.
///
/// The EMULATOR's own bound: the document is a handful of members, and the endpoint is one the
/// CALLER named, so the bytes a read of it costs are fixed here rather than by whatever that
/// endpoint answers.
const MAX_ANSWER_BYTES: usize = 64 * 1024;

/// The host and path a `custom-resource` `ResourceId` addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The authority the endpoint is served on, as a `Host` header carries it.
    pub host: String,
    /// The path, with its query, the capacity document is served at.
    pub path: String,
}

/// The endpoint a `custom-resource` `ResourceId` names, or `None` for an id that is no
/// endpoint at all — which is a target whose capacity nothing can read or move.
///
/// The id is an absolute URL, parsed with `::http::Uri` rather than by splitting the string:
/// the authority it names is what the request is addressed to and the path, with its query, is
/// the capacity document. An id that names no authority, or a scheme nothing is served over,
/// addresses no endpoint; one that names no path of its own addresses the root of that
/// endpoint, which is how a URI with an empty path normalises.
pub fn endpoint_of(resource_id: &str) -> Option<Endpoint> {
    let uri: ::http::Uri = resource_id.parse().ok()?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return None;
    }
    let authority = uri.authority()?.as_str().to_string();
    let path = uri
        .path_and_query()
        .map_or("/", ::http::uri::PathAndQuery::as_str)
        .to_string();
    (!authority.is_empty()).then_some(Endpoint {
        host: authority,
        path,
    })
}

/// The capacity the resource behind a `custom-resource` target reports right now, or `None`
/// when this emulator could not read it.
pub async fn read_capacity(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    resource_id: &str,
) -> Option<i32> {
    let endpoint = endpoint_of(resource_id)?;
    let (status, payload) = call(callback, region, "GET", &endpoint, Vec::new()).await?;
    if !(200..300).contains(&status) {
        return None;
    }
    let document: Value = ::serde_json::from_slice(&payload).ok()?;
    let capacity = document
        .get(ACTUAL_CAPACITY)
        .and_then(Value::as_f64)
        .or_else(|| document.get(DESIRED_CAPACITY).and_then(Value::as_f64))?;
    (capacity.is_finite() && capacity >= 0.0 && capacity <= f64::from(i32::MAX))
        .then(|| capacity.round() as i32)
}

/// Ask the resource behind a `custom-resource` target to move to `capacity`, answering
/// whether the endpoint accepted it.
///
/// The body is the one member the contract carries, as the number the endpoint reports its
/// capacity as.
pub async fn set_capacity(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    resource_id: &str,
    capacity: i32,
) -> bool {
    let Some(endpoint) = endpoint_of(resource_id) else {
        return false;
    };
    let body = ::serde_json::json!({DESIRED_CAPACITY: f64::from(capacity)}).to_string();
    let answered = call(callback, region, "PATCH", &endpoint, body.into_bytes()).await;
    answered.is_some_and(|(status, _)| (200..300).contains(&status))
}

/// Make one request on the endpoint over the in-process transport, answering its status and
/// body — or `None` when the request could not be made or served at all.
async fn call(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    method: &str,
    endpoint: &Endpoint,
    body: Vec<u8>,
) -> Option<(u16, Vec<u8>)> {
    let mut request = ::http::Request::builder()
        .method(method)
        .uri(&endpoint.path)
        .header("host", &endpoint.host)
        .header("content-type", APPLICATION_JSON)
        .header("accept", APPLICATION_JSON)
        .body(body)
        .ok()?;
    // The emulator verifies every request signature, so this call carries one too: the
    // credential the framework hands this service, signed through the tree's one in-process
    // signer, under the signing name an invoke endpoint authenticates.
    ::service_common::in_process_sigv4::sign_callback_request(
        &mut request,
        INVOKE_SIGNING_NAME,
        region,
        callback,
    )
    .await
    .ok()?;
    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let request = ::aws_smithy_runtime_api::http::Request::try_from(request).ok()?;
    let connector = callback.http_connector();
    let answer = {
        use ::aws_smithy_runtime_api::client::http::HttpConnector;
        HttpConnector::call(&connector, request).await.ok()?
    };
    let status = answer.status().as_u16();
    // The answer comes from an endpoint the CALLER named, so only the prefix a capacity
    // document can possibly occupy is copied and parsed: everything past that cap is dropped,
    // which leaves a long body an answer this reader cannot read rather than an allocation
    // the request chose the size of.
    let payload = answer
        .body()
        .bytes()
        .unwrap_or_default()
        .iter()
        .take(MAX_ANSWER_BYTES)
        .copied()
        .collect();
    Some((status, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a `custom-resource` resource id is the absolute URL of the capacity document
    /// the caller's own application serves; anything else addresses no document.
    #[test]
    fn a_custom_resource_id_addresses_one_capacity_document() {
        /// (case, the resource id, the endpoint it names)
        type Row<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);
        let rows: &[Row<'_>] = &[
            (
                "the published form: an api stage and the dimension's own path",
                "https://example.execute-api.eu-west-1.amazonaws.com/prod/scalableTargetDimensions/1",
                Some((
                    "example.execute-api.eu-west-1.amazonaws.com",
                    "/prod/scalableTargetDimensions/1",
                )),
            ),
            (
                "an endpoint on an explicit port",
                "https://127.0.0.1:34991/_aws/execute-api/abc/prod/scalableTargetDimensions/7",
                Some((
                    "127.0.0.1:34991",
                    "/_aws/execute-api/abc/prod/scalableTargetDimensions/7",
                )),
            ),
            (
                "plain http",
                "http://localhost:8080/capacity",
                Some(("localhost:8080", "/capacity")),
            ),
            (
                "a query the endpoint reads",
                "https://host/capacity?dimension=1",
                Some(("host", "/capacity?dimension=1")),
            ),
            (
                "no path of its own, which addresses the endpoint's root",
                "https://host",
                Some(("host", "/")),
            ),
            ("an empty path", "https://host/", Some(("host", "/"))),
            ("no host", "https:///capacity", None),
            ("no scheme", "host/capacity", None),
            ("a scheme nothing is served over", "ftp://host/capacity", None),
            ("another namespace's resource id", "table/orders", None),
            ("nothing at all", "", None),
        ];
        for (case, resource_id, expected) in rows {
            let actual = endpoint_of(resource_id);
            let expected = expected.map(|(host, path)| Endpoint {
                host: host.to_string(),
                path: path.to_string(),
            });
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
