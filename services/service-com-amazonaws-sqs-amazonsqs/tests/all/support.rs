// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate shares: one emulator over one engine,
//! and the helpers that drive it (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use.
//!
//! It lives here rather than in each suite so a second suite does not re-derive the
//! request shape, the account id or the url and ARN forms.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the request's SigV4 credential scope names; it is the region every
/// expected ARN carries.
pub const REGION: &str = "us-east-1";

/// The authority the request arrives on, and therefore the endpoint every minted
/// queue url is built from.
pub const HOST: &str = "sqs.us-east-1.amazonaws.com";

/// The endpoint a client configured for `region` addresses, as Amazon SQS names its
/// regional endpoints: every suite driving more than one region builds its host from this
/// rather than re-deriving the form.
pub fn host_in(region: &str) -> String {
    format!("sqs.{region}.amazonaws.com")
}

/// A SigV4-shaped `Authorization` header whose credential scope names `region`: the scope
/// is what routes the request to this service (signing name `sqs`) AND what says which
/// region the call addressed. Request authentication is off in these tests, so the
/// signature itself is never checked.
pub fn authorization_in(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/sqs/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0000"
    )
}

/// One emulator holding only this service, over one shared engine.
pub fn emulator() -> MultiServiceRouter {
    emulator_over_engine().0
}

/// The same emulator, with the engine it was built over handed back.
///
/// A suite that has to observe — or ADVANCE — a span the clock owes uses this: a
/// retention period is a documented duration measured in minutes at the shortest, so a
/// test moves the instants a stored message carries into the past instead of sleeping
/// for them. Everything a request can observe still goes over the wire through the
/// router; the engine is only the seam the clock is reached through.
pub fn emulator_over_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// The account id every generated emulator presents.
pub fn account() -> &'static str {
    engine::emulator_account_id()
}

/// An account id that is not this emulator's, for the rows that check an ARN belonging to
/// somebody else. Every suite asserting that refusal shares it rather than re-deriving
/// one; the suites that use it assert it differs from [`account`].
pub const OTHER_ACCOUNT: &str = "123456789012";

/// The url a queue named `name` must be reachable at: `<endpoint>/<account>/<name>`.
pub fn expected_url(name: &str) -> String {
    expected_url_in(REGION, name)
}

/// The same, for a queue created against `region`'s endpoint.
pub fn expected_url_in(region: &str, name: &str) -> String {
    format!("https://emulator.invalid/sqs/{region}/{}/{name}", account())
}

/// The ARN a queue named `name` must report.
pub fn expected_arn(name: &str) -> String {
    expected_arn_in(REGION, name)
}

/// The same, for a queue created against `region`'s endpoint: a queue ARN carries the
/// region the queue lives in.
pub fn expected_arn_in(region: &str, name: &str) -> String {
    format!("arn:aws:sqs:{region}:{}:{name}", account())
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive
/// the round trip. Every suite here that drives IAM's or STS's awsQuery wire shares it.
pub fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response, which is how
/// those suites read an answer's one member.
pub fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Post `body` VERBATIM as `operation`, and answer the HTTP status with the response's
/// raw bytes.
///
/// The one request builder every helper here goes through. A suite reaches for this
/// rather than [`call`] when it needs the request's byte LENGTH controlled exactly (a
/// `Value` is serialized, so its length is not the suite's to choose) or when it has to
/// read an answer that carries no JSON at all — a transport refusal answers with an empty
/// body, which parses to the same `Value` as a body holding `null`.
pub async fn post_bytes(
    router: &MultiServiceRouter,
    operation: &str,
    body: Vec<u8>,
) -> (u16, Vec<u8>) {
    post_bytes_in(router, REGION, operation, body).await
}

/// The same, addressed to `region`.
pub async fn post_bytes_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Vec<u8>,
) -> (u16, Vec<u8>) {
    let response = router
        .dispatch(request_for(region, operation, body, false))
        .await;
    (response.status().as_u16(), response.body().to_vec())
}

/// The request header a query-compatible SDK opts into the legacy query error report
/// with, and the value it carries. Named here so the suites that drive that report and
/// the ones that drive a native JSON client cannot spell the opt-in differently.
pub const QUERY_MODE_HEADER: &str = "x-amzn-query-mode";

/// The response header that report arrives in.
pub const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// One request as this service's wire protocol carries it: `awsJson1_0` over `POST /`
/// with the target header, and — for `query_mode` — the header a query-compatible SDK
/// sends on every call it makes to this service. It arrives on `region`'s own endpoint,
/// which is where a client configured for that region addresses it.
fn request_for(
    region: &str,
    operation: &str,
    body: Vec<u8>,
    query_mode: bool,
) -> http::Request<Bytes> {
    request_arriving_on(&host_in(region), region, operation, body, query_mode)
}

/// The same request, arriving on `address` — the `Host` header it carries.
///
/// The address a request arrives on is a different thing from the region it was made in,
/// which its SigV4 credential scope names: a client reaches this emulator at whatever
/// address it was pointed at, and this is the seam a suite drives that difference through.
fn request_arriving_on(
    address: &str,
    region: &str,
    operation: &str,
    body: Vec<u8>,
    query_mode: bool,
) -> http::Request<Bytes> {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", address)
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization_in(region));
    if query_mode {
        builder = builder.header(QUERY_MODE_HEADER, "true");
    }
    builder
        .body(Bytes::from(body))
        .expect("a well-formed request")
}

/// Send `operation` as a call made in `region` whose request ARRIVES on `address`, and
/// answer its HTTP status and parsed body.
pub async fn call_on_host(
    router: &MultiServiceRouter,
    address: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = request_arriving_on(
        address,
        region,
        operation,
        serde_json::to_vec(body).expect("the request body serializes"),
        false,
    );
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The same call, which must succeed.
pub async fn ok_on_host(
    router: &MultiServiceRouter,
    address: &str,
    region: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = call_on_host(router, address, region, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {operation} in {region} arriving on {address}: expected HTTP 200, got {status} \
         with {parsed}"
    );
    parsed
}

/// Send one operation and answer its HTTP status, its response HEADERS and its parsed
/// body — the seam for an answer that states something its body does not.
pub async fn call_with_headers(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, http::HeaderMap, Value) {
    answered(router, operation, body, false).await
}

/// The same call as a query-compatible SDK makes it, with the [`QUERY_MODE_HEADER`]
/// opt-in: the seam for asserting the legacy query error report.
pub async fn call_in_query_mode(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, http::HeaderMap, Value) {
    answered(router, operation, body, true).await
}

/// `operation` dispatched through the emulator, as its status, headers and parsed body.
async fn answered(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    query_mode: bool,
) -> (u16, http::HeaderMap, Value) {
    let request = request_for(
        REGION,
        operation,
        serde_json::to_vec(body).expect("the request body serializes"),
        query_mode,
    );
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, headers, parsed)
}

/// Send one operation and return its HTTP status and parsed body.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in(router, REGION, operation, body).await
}

/// The same, addressed to `region`: the request arrives on that region's endpoint and
/// carries a credential scope naming it, which is everything that says which region a
/// call was made in.
pub async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let (status, answer) = post_bytes_in(
        router,
        region,
        operation,
        serde_json::to_vec(body).expect("the request body serializes"),
    )
    .await;
    (status, serde_json::from_slice(&answer).unwrap_or(Value::Null))
}

/// Send one operation that must succeed.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    ok_in(router, REGION, operation, body).await
}

/// The same, addressed to `region`.
pub async fn ok_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = call_in(router, region, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {operation} in {region} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The status a TRANSPORT refusal of an over-sized request body carries (HTTP 413,
/// Request Entity Too Large). Shared by every suite that tells that refusal apart from
/// one the service itself made, so the two cannot name different statuses for it.
pub const REQUEST_ENTITY_TOO_LARGE: u16 = 413;

/// The error SHAPE a refusal names: the body's `__type` discriminator, verbatim.
///
/// On `awsJson1_0` that is the shape id of the error that was thrown — a local shape name
/// for one this service's model declares, and the request-validation framework's own
/// namespaced id for a parameter refusal it raised.
pub fn error_shape(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The wire error CODE of a refusal — the name every suite here knows a refusal class by.
///
/// It is the error shape's own local name, except for the two parameter classes raised by
/// the request-validation framework in front of this service: those name their class with
/// that framework's shape id in the body and with this service's own legacy code in the
/// [`QUERY_ERROR_HEADER`], so the code is read from the pair the service declares rather
/// than cut out of the shape id. [`error_shape`] reports the discriminator itself.
pub fn error_code(body: &Value) -> String {
    use service_com_amazonaws_sqs_amazonsqs::scaffolded as refusals;

    let shape = error_shape(body);
    if shape == refusals::INVALID_PARAMETER_VALUE_SHAPE_ID {
        return refusals::INVALID_PARAMETER_VALUE.to_string();
    }
    if shape == refusals::MISSING_PARAMETER_SHAPE_ID {
        return refusals::MISSING_PARAMETER.to_string();
    }
    shape
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// A JSON object of strings as a sorted map, so two attribute maps compare directly.
pub fn as_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|value| (name.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The longest a receive in [`receive_until`] waits for one message, in seconds. Inside
/// the documented `ReceiveMessageRequest$WaitTimeSeconds` range ("an integer from 0 to
/// 20").
const RECEIVE_WAIT_SECONDS: u64 = 1;

/// Receive from the queue at `url` until `wanted` messages have come over or `deadline`
/// passes, and answer each one's `(MessageId, Body)` in the order they were handed over.
///
/// Every suite that waits for messages a queue is owed shares this loop rather than
/// re-deriving the wait and the receive shape.
pub async fn receive_until(
    router: &MultiServiceRouter,
    url: &str,
    wanted: usize,
    deadline: Instant,
) -> Vec<(String, String)> {
    receive_until_in(router, REGION, url, wanted, deadline).await
}

/// The same, receiving in `region`: a queue belongs to one region, so a suite driving
/// several of them says which one each receive is made in.
pub async fn receive_until_in(
    router: &MultiServiceRouter,
    region: &str,
    url: &str,
    wanted: usize,
    deadline: Instant,
) -> Vec<(String, String)> {
    let mut delivered: Vec<(String, String)> = Vec::with_capacity(wanted);
    while delivered.len() < wanted && Instant::now() < deadline {
        let reply = ok_in(
            router,
            region,
            "ReceiveMessage",
            json!({
                "QueueUrl": url,
                "MaxNumberOfMessages": 10,
                "WaitTimeSeconds": RECEIVE_WAIT_SECONDS,
            }),
        )
        .await;
        for message in messages(&reply) {
            delivered.push((
                message["MessageId"].as_str().unwrap_or_default().to_string(),
                message["Body"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    delivered
}

/// The attribute map a FIFO queue is created with: `FifoQueue` plus `extra`.
///
/// Every suite here that creates a FIFO queue shares it rather than re-deriving the one
/// attribute that makes a queue a FIFO queue. A FIFO queue's NAME always carries the
/// `.fifo` suffix, which is the caller's to supply.
pub fn fifo_attributes(extra: &[(&str, &str)]) -> Value {
    let mut attributes = json!({ "FifoQueue": "true" });
    for (name, value) in extra {
        attributes[*name] = json!(value);
    }
    attributes
}

/// Create a queue and return the raw reply.
pub async fn create(router: &MultiServiceRouter, name: &str, attributes: &Value) -> (u16, Value) {
    create_in(router, REGION, name, attributes).await
}

/// The same, addressed to `region`: the queue is created in that region's namespace.
pub async fn create_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    attributes: &Value,
) -> (u16, Value) {
    let mut body = json!({"QueueName": name});
    if attributes.as_object().is_some_and(|map| !map.is_empty()) {
        body["Attributes"] = attributes.clone();
    }
    call_in(router, region, "CreateQueue", &body).await
}

/// Create a queue that must succeed and return its url.
pub async fn create_ok(router: &MultiServiceRouter, name: &str, attributes: &Value) -> String {
    create_ok_in(router, REGION, name, attributes).await
}

/// The same, addressed to `region`.
pub async fn create_ok_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    attributes: &Value,
) -> String {
    let (status, parsed) = create_in(router, region, name, attributes).await;
    assert_eq!(
        status, 200,
        "case create {name} in {region} with {attributes}: expected HTTP 200, got {status} \
         with {parsed}"
    );
    parsed["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case create {name} in {region}: no QueueUrl in {parsed}"))
        .to_string()
}

/// The `Messages` of a receive reply, as a list — empty when the member is absent,
/// which is how a reply that hands over nothing answers.
pub fn messages(reply: &Value) -> Vec<Value> {
    reply
        .get("Messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every attribute the queue at `url` has a value for (`AttributeNames: ["All"]`).
pub async fn all_attributes(
    router: &MultiServiceRouter,
    url: &str,
) -> BTreeMap<String, String> {
    let reported = ok(
        router,
        "GetQueueAttributes",
        json!({"QueueUrl": url, "AttributeNames": ["All"]}),
    )
    .await;
    as_map(&reported["Attributes"])
}

/// One attribute of the queue at `url` as the integer it reports, which is how every
/// `ApproximateNumberOfMessages*` attribute is carried.
pub async fn attribute_count(router: &MultiServiceRouter, url: &str, attribute: &str) -> i64 {
    let attributes: BTreeMap<String, String> = all_attributes(router, url).await;
    attributes
        .get(attribute)
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or_else(|| panic!("case {attribute} of {url}: not an integer in {attributes:?}"))
}

/// The three message counts the queue at `url` reports, as
/// `(available, delayed, in flight)` — the `ApproximateNumberOfMessages`,
/// `ApproximateNumberOfMessagesDelayed` and `ApproximateNumberOfMessagesNotVisible`
/// attributes. One reading for every suite that asks what a queue holds.
pub async fn message_counts(router: &MultiServiceRouter, url: &str) -> (i64, i64, i64) {
    let attributes = all_attributes(router, url).await;
    let read = |name: &str| {
        attributes
            .get(name)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or_else(|| panic!("case the message counts of {url}: no {name} in {attributes:?}"))
    };
    (
        read("ApproximateNumberOfMessages"),
        read("ApproximateNumberOfMessagesDelayed"),
        read("ApproximateNumberOfMessagesNotVisible"),
    )
}

/// The tags `ListQueueTags` reports for the queue at `url`, as a sorted map. A queue with
/// no tag reports no `Tags` member at all, which is the empty map here.
pub async fn list_tags(router: &MultiServiceRouter, url: &str) -> BTreeMap<String, String> {
    let reported = ok(router, "ListQueueTags", json!({"QueueUrl": url})).await;
    as_map(reported.get("Tags").unwrap_or(&Value::Null))
}

/// Create a queue called `name` and a queue called `<name>-source` whose `RedrivePolicy`
/// drives its messages to it, which is what makes the first queue a dead-letter queue —
/// the only kind of source a message movement task supports.
///
/// Returns `(dead-letter queue url, dead-letter queue ARN, source queue url)`.
pub async fn dead_letter_queue(
    router: &MultiServiceRouter,
    name: &str,
) -> (String, String, String) {
    let dead_letter_url = create_ok(router, name, &json!({})).await;
    let dead_letter_arn = expected_arn(name);
    let source_url = redrives_to(router, &format!("{name}-source"), &dead_letter_arn).await;
    (dead_letter_url, dead_letter_arn, source_url)
}

/// Create a queue called `name` whose `RedrivePolicy` names `dead_letter_arn` as its
/// dead-letter target, and return its url.
pub async fn redrives_to(
    router: &MultiServiceRouter,
    name: &str,
    dead_letter_arn: &str,
) -> String {
    let policy = json!({"deadLetterTargetArn": dead_letter_arn, "maxReceiveCount": "1"});
    create_ok(router, name, &json!({"RedrivePolicy": policy.to_string()})).await
}

/// Send `bodies` to the queue at `url` and return the `MessageId` each send minted, in
/// send order.
pub async fn send_all(router: &MultiServiceRouter, url: &str, bodies: &[String]) -> Vec<String> {
    let mut ids = Vec::with_capacity(bodies.len());
    for body in bodies {
        let reply = ok(
            router,
            "SendMessage",
            json!({"QueueUrl": url, "MessageBody": body}),
        )
        .await;
        ids.push(
            reply["MessageId"]
                .as_str()
                .unwrap_or_else(|| panic!("case send {body}: no MessageId in {reply}"))
                .to_string(),
        );
    }
    ids
}
