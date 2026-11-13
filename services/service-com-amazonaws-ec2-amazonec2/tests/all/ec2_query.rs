// SPDX-License-Identifier: CC0-1.0

//! The ec2Query test harness every suite in this crate drives the service through.
//!
//! EC2 speaks `ec2Query`: an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! The envelope is selected by the `ec2` signing name in the request's SigV4 credential
//! scope, and the Region in that scope is the Region the service answers for — so every
//! call here carries one.
//!
//! This module holds only the plumbing: building the form body, dispatching it through
//! the same `MultiServiceRouter` the emulator serves with, and reading elements back out
//! of the XML. What each operation asserts lives in that operation's own suite.

use std::sync::Arc;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

/// The EC2 API version a client sends in an ec2Query body.
pub const API_VERSION: &str = "2016-11-15";

/// The emulator the tests drive: one shared engine with the EC2 service mounted on it,
/// request authentication off (these tests sign nothing — they only carry the
/// `Credential=` scope the envelope and the Region are read from). The engine is handed
/// back too, so a suite can seed the store a handler reads.
pub fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Percent-encode the characters that are significant in an
/// `application/x-www-form-urlencoded` body, leaving everything else as a client's own
/// encoder would (`:` and `/` in a CIDR block).
fn form_value(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            '+' => "%2B".to_string(),
            '%' => "%25".to_string(),
            ' ' => "%20".to_string(),
            '\\' => "%5C".to_string(),
            // Historically accepted as a field separator beside `&`, so a value carrying
            // one is encoded rather than left to the parser's reading of it.
            ';' => "%3B".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// One `action` call signed for `region`, with `params` as extra form fields. Answers the
/// HTTP status and the response body.
pub async fn call(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let (status, _content_type, body) = call_with_content_type(router, region, action, params).await;
    (status, body)
}

/// The same call as [`call`], answering the response's `content-type` header beside the
/// status and the body — `None` when the response declares none at all.
///
/// Separate from [`call`] because the media type is part of the ec2Query envelope rather
/// than of any one operation, so only the suite that asserts on the envelope reads it.
pub async fn call_with_content_type(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, Option<String>, String) {
    let mut body = format!("Action={action}&Version={API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_value(value));
    }
    call_raw(router, region, &[], &body).await
}

/// One POST whose body is `body` exactly as written, with `headers` beside the content type
/// and the credential scope every call carries. Answers the status, the response's
/// `content-type` and the body.
///
/// The one request builder [`call_with_content_type`] itself goes through, so a suite that
/// has to send a body no parameter list can express — one naming no `Action` at all, or one
/// declaring a transfer framing — builds it here rather than assembling a second request of
/// its own.
pub async fn call_raw(
    router: &MultiServiceRouter,
    region: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, Option<String>, String) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/ec2/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    );
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{region}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed ec2Query request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (status, content_type, String::from_utf8_lossy(response.body()).to_string())
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal.
///
/// Stricter than [`form_value`], which leaves a value's `:` and `/` as a client's own
/// encoder would: a component that carries JSON (an IAM policy document) or a path (an IAM
/// instance profile's) has to survive the round trip whole, so every reserved byte goes
/// through. The ONE encoder the suites that drive ANOTHER emulated service share.
pub fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// One query-protocol call to `service` in `region`, presented as signed by the access key
/// id `akid` — the only place the credential appears on the wire, and so what a service
/// reads the caller's identity from.
///
/// The ONE hop for a suite that has to drive another emulated service beside EC2 — IAM for
/// the identities EC2's authorization gate reads and for the instance profiles it resolves a
/// reference against — so the envelope, the credential scope and the encoding are built
/// once rather than per suite. Both names and values are encoded through [`form_encode`].
pub async fn query_call_as(
    router: &MultiServiceRouter,
    region: &str,
    service: &str,
    api_version: &str,
    akid: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={api_version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{service}.{region}.amazonaws.com/"))
        .header("host", format!("{service}.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{region}/{service}/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed query request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (
        status,
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// `vec!["a".to_string(), …]` for a slice of string literals — a table's expected values,
/// as owned `String`s, for comparing against the values a response reported.
pub fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// `ZoneName.N` / `VpcId.N` / `Filter.N.Name` form fields, as ec2Query flattens a list
/// into indexed keys.
pub fn indexed(prefix: &str, values: &[&str]) -> Vec<(String, String)> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| (format!("{prefix}.{}", index + 1), (*value).to_string()))
        .collect()
}

/// The form fields for one `Filters` entry at position `index` (counting from 1).
fn filter_fields(index: usize, name: &str, values: &[&str]) -> Vec<(String, String)> {
    let mut fields = vec![(format!("Filter.{index}.Name"), name.to_string())];
    fields.extend(indexed(&format!("Filter.{index}.Value"), values));
    fields
}

/// The form fields for a whole `Filters` list.
pub fn filters(entries: &[(&str, &[&str])]) -> Vec<(String, String)> {
    entries
        .iter()
        .enumerate()
        .flat_map(|(index, (name, values))| filter_fields(index + 1, name, values))
        .collect()
}

/// The XML namespace this service's response envelopes carry, as the model binds it
/// (`smithy_query(query="service_info")`'s `xmlNamespace`).
pub const XML_NAMESPACE: &str = "http://ec2.amazonaws.com/doc/2016-11-15";

/// The text of every DIRECT child of an operation's `<{action}Response>` envelope named
/// `name`, in document order, having first pinned that the envelope's root is that element
/// in [`XML_NAMESPACE`].
///
/// Direct children only: a `<return>` or a `<requestId>` nested inside a reported resource
/// would be that resource's member, not the envelope's. Lives here, beside the plumbing
/// every suite shares, so the envelope is read one way rather than once per suite.
pub fn envelope_children(case: &str, body: &str, action: &str, name: &str) -> Vec<String> {
    let parsed = super::wire::document(case, body);
    let root = parsed.root_element();
    let expected = format!("{action}Response");
    assert_eq!(
        root.tag_name().name(),
        expected,
        "case {case:?}: expected a <{expected}> root, got {body}"
    );
    assert_eq!(
        root.tag_name().namespace(),
        Some(XML_NAMESPACE),
        "case {case:?}: expected the <{expected}> root in the namespace {XML_NAMESPACE:?}, \
         got {:?} in {body}",
        root.tag_name().namespace()
    );
    root.children()
        .filter(|node| node.is_element() && node.tag_name().name() == name)
        .map(|node| node.text().unwrap_or_default().to_string())
        .collect()
}

/// The text of the first `<name>…</name>` element in `xml`.
pub fn element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// The text of every `<name>…</name>` element in `xml`, in document order.
pub fn elements(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        match rest.find(&close) {
            Some(end) => {
                found.push(rest[..end].to_string());
                rest = &rest[end + close.len()..];
            }
            None => break,
        }
    }
    found
}

/// An ISO 8601 timestamp element as the instant it names, or `None` when it is not one.
///
/// Every suite that asserts a timestamp member reads it the same way, so the parse lives
/// here with the rest of the response plumbing rather than once per suite.
pub fn as_timestamp(value: &str) -> Option<f64> {
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|at| at.as_secs_f64())
}

/// The `<Code>` of a failure body — EC2's error envelope is
/// `<Response><Errors><Error><Code/><Message/></Error></Errors></Response>`.
pub fn error_code(xml: &str) -> String {
    element(xml, "Code").unwrap_or_default()
}

/// The text of the response envelope's immediate `name` child: `None` when the element is
/// absent, `Some("")` when it is present and empty — the two cases a suite asserting an
/// optional member has to tell apart.
///
/// Read with the XML parser rather than by substring, so a member of the ENVELOPE is never
/// confused with one of the same name nested inside a result element.
pub fn member(body: &str, name: &str) -> Option<String> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    document
        .root_element()
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == name)
        .map(|node| node.text().unwrap_or_default().to_string())
}

/// The `instanceState/name` `DescribeInstances` reports for one instance, as a string the
/// "nothing moved" assertions of any suite compare.
pub async fn described_instance_state(
    router: &MultiServiceRouter,
    region: &str,
    instance_id: &str,
) -> String {
    let (status, body) = call(
        router,
        region,
        "DescribeInstances",
        &[("InstanceId.1".to_string(), instance_id.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstances for {instance_id} must succeed, got {status}: {body}"
    );
    let document = roxmltree::Document::parse(&body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "instanceState")
        .and_then(|state| {
            state
                .children()
                .find(|node| node.is_element() && node.tag_name().name() == "name")
        })
        .and_then(|name| name.text())
        .unwrap_or_default()
        .to_string()
}

/// The id of the PRIMARY network interface of `instance_id`: the interface attached to it at
/// the lowest device index, which is the one its launch created and the one an Elastic IP
/// association named with an `InstanceId` binds to.
///
/// Read back through `DescribeNetworkInterfaces` narrowed by `attachment.instance-id`, so an
/// expectation about it is the interface the account really holds. Lives here, beside the
/// plumbing every suite shares, because more than one suite needs that id.
pub async fn primary_network_interface(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    instance_id: &str,
) -> String {
    let (status, body) = call(
        router,
        region,
        "DescribeNetworkInterfaces",
        &filters(&[("attachment.instance-id", &[instance_id])]),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeNetworkInterfaces must report the interfaces of \
         {instance_id}, got {status} with body {body}"
    );
    let parsed = super::wire::document(case, &body);
    let mut attached: Vec<(i32, String)> = parsed
        .descendants()
        .filter(|node| {
            node.is_element() && super::wire::child(*node, "networkInterfaceId").is_some()
        })
        .filter_map(|node| {
            let network_interface_id = super::wire::child_text(node, "networkInterfaceId")?;
            let device_index = super::wire::child(node, "attachment")
                .and_then(|attachment| super::wire::child_text(attachment, "deviceIndex"))
                .and_then(|index| index.parse::<i32>().ok())?;
            Some((device_index, network_interface_id))
        })
        .collect();
    attached.sort();
    attached
        .into_iter()
        .map(|(_, network_interface_id)| network_interface_id)
        .next()
        .unwrap_or_else(|| {
            panic!("case {case:?}: {instance_id} must hold a primary network interface: {body}")
        })
}

/// XML text as the service wrote it, with the five entities resolved — an error message
/// quoting a filter name reaches the wire as `&apos;…&apos;`.
pub fn unescaped(xml_text: &str) -> String {
    xml_text
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The `<Message>` of a failure body, with the XML entities resolved — part of the same
/// envelope as [`error_code`], so every suite that asserts a refusal's wording reads it
/// the one way.
pub fn error_message(xml: &str) -> String {
    unescaped(&element(xml, "Message").unwrap_or_default())
}
