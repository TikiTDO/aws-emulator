// SPDX-License-Identifier: CC0-1.0

//! Two refusal rules this API states for a request its own parameters rule out,
//! each asserted over a table rather than over one recorded request.
//!
//! * A constraint declared on a MAP's VALUE shape is reported at the MAP MEMBER,
//!   quoting the value that member held — the whole map the request carried, in
//!   the order it carried it — and listing the value shape's WHOLE declared
//!   constraint set, so one offending map is ONE validation error however many of
//!   its entries broke a constraint. The rows differ along how far past the bound
//!   a value is, how many entries of one map are offending, where the offending
//!   entry sits among admitted ones, and the shape of the key it sits at; the
//!   published bounds themselves (and the value one character past the maximum)
//!   are rows of their own.
//! * A member the documentation admits only alongside another one is refused by
//!   naming the member that has to be ADDED and the value it has to carry, for
//!   every form of the enabling member's absence.
//!
//! Every test drives the real wire path — the router, `POST /` with an
//! `X-Amz-Target` and an awsJson1_1 body — so what is asserted is what an SDK
//! client sees.

use std::sync::Arc;

use service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and therefore the Region the
/// endpoint serves.
const REGION: &str = "us-east-1";

/// An ARN of a service no owning crate resolves, so it needs no live resource
/// behind it: this service's own store is its tag home. Every refused request
/// carries it, so nothing but the tags can be what the refusal is about.
const SERVED: &str = "arn:aws:s3:::map-value-clause-bucket";

/// The declared bounds of a tag VALUE, and the expression it has to match.
///
/// Source: `smithy_query(query="shape",shape="TagValue")` — `@length 0..256`,
/// `@pattern ^[\s\S]*$` — which is also what `TagMap`'s value member targets.
const TAG_VALUE_MAX_LENGTH: usize = 256;

/// See [`TAG_VALUE_MAX_LENGTH`].
const TAG_VALUE_MIN_LENGTH: usize = 0;

/// The whole constraint set a tag value has to satisfy, in the order AWS's
/// clauses list it: the `@length` maximum, its minimum, then the `@pattern` —
/// stated WITHOUT the whole-value anchors the model writes it with, since the
/// requirement a caller is told is the expression a value has to satisfy. Source:
/// the same `TagValue` shape as [`TAG_VALUE_MAX_LENGTH`].
fn tag_value_constraint_set() -> String {
    format!(
        "Map value must satisfy constraint: [Member must have length less than or equal to \
         {TAG_VALUE_MAX_LENGTH}, Member must have length greater than or equal to \
         {TAG_VALUE_MIN_LENGTH}, Member must satisfy regular expression pattern: [\\s\\S]*]"
    )
}

/// A router carrying only this service, over its own fresh engine, so each test
/// starts from an empty store and observes the same state across requests.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests exercise handler and refusal behaviour, not SigV4: the
    // `Authorization` header below is present because it is what routes an
    // awsJson request to the `tagging` signing name.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: String,
) -> (u16, serde_json::Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("ResourceGroupsTaggingAPI_20170126.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/tagging/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: serde_json::Value = if response.body().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// The error code an awsJson error body carries (`__type` minus its namespace).
fn error_code(body: &serde_json::Value) -> String {
    let raw = body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("an error body with no __type: {body}"));
    raw.rsplit(['#', ':']).next().unwrap_or(raw).to_string()
}

/// The message an error body carries, under either spelling of the member.
fn error_message(body: &serde_json::Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("an error body with no message: {body}"))
        .to_string()
}

/// A `TagResources` body over one ARN and `tags`, written in the order given —
/// built as TEXT, because a JSON object's member order is what a clause quoting
/// the submitted map has to reproduce and no map type keeps it.
fn tag_resources_body(arn: &str, tags: &[(String, String)]) -> String {
    let entries: Vec<String> = tags
        .iter()
        .map(|(key, value)| {
            format!(
                "{}:{}",
                serde_json::Value::String(key.clone()),
                serde_json::Value::String(value.clone())
            )
        })
        .collect();
    format!(
        "{{\"ResourceARNList\":[{}],\"Tags\":{{{}}}}}",
        serde_json::Value::String(arn.to_string()),
        entries.join(",")
    )
}

/// The map a clause quotes: the entries in the order the request wrote them.
fn rendered(tags: &[(String, String)]) -> String {
    let entries: Vec<String> = tags
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    format!("{{{}}}", entries.join(", "))
}

/// One tag, spelled as a row writes it.
fn tag(key: &str, value: String) -> (String, String) {
    (key.to_string(), value)
}

/// A value of `length` characters.
fn value_of(length: usize) -> String {
    "v".repeat(length)
}

/// A map value past its declared maximum is reported at the MAP member, quoting
/// the whole map the request carried and listing every constraint a tag value has
/// to satisfy — one clause per offending MAP, not per offending entry.
#[tokio::test]
async fn a_tag_value_past_its_maximum_is_reported_at_the_map_with_the_whole_value_set() {
    let router = emulator();

    let over = TAG_VALUE_MAX_LENGTH + 1;
    // Columns: the case, and the tags the request carries in the order it
    // carries them.
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "one character past the maximum",
            vec![tag("valkey", value_of(over))],
        ),
        (
            "far past the maximum",
            vec![tag("valkey", value_of(TAG_VALUE_MAX_LENGTH * 4))],
        ),
        (
            "two offending entries in one map",
            vec![
                tag("first", value_of(over)),
                tag("second", value_of(over + 10)),
            ],
        ),
        (
            "an offending entry between admitted ones",
            vec![
                tag("before", "short".to_string()),
                tag("offender", value_of(over)),
                tag("after", String::new()),
            ],
        ),
        (
            "an offending entry at a dotted key",
            vec![tag("team.cost.centre", value_of(over))],
        ),
        (
            "an offending entry at a key the API's own paths use",
            vec![tag("tags", value_of(over))],
        ),
    ];

    for (case, tags) in cases {
        let (status, body) = call(&router, "TagResources", tag_resources_body(SERVED, &tags)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a tag value past its maximum is refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: got {body}"
        );
        // ONE validation error, stated at the map member, over the map the
        // request carried and the value shape's whole constraint set.
        let expected = format!(
            "1 validation error detected: Value '{}' at 'tags' failed to satisfy constraint: {}",
            rendered(&tags),
            tag_value_constraint_set()
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The published bounds themselves are admitted: a value AT the maximum, and one
/// at the minimum, are written and read back — so the clause above reports a
/// value the model really excludes rather than one the bound includes.
#[tokio::test]
async fn a_tag_value_at_each_published_bound_is_written_and_a_refused_map_writes_nothing() {
    let router = emulator();

    let at_bounds: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "the declared maximum",
            vec![tag("at-max", value_of(TAG_VALUE_MAX_LENGTH))],
        ),
        (
            "the declared minimum",
            vec![tag("at-min", value_of(TAG_VALUE_MIN_LENGTH))],
        ),
    ];

    for (case, tags) in &at_bounds {
        let (status, body) = call(&router, "TagResources", tag_resources_body(SERVED, tags)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a value inside the declared bounds is admitted, got {status} with \
             {body}"
        );
        assert_eq!(
            body["FailedResourcesMap"],
            serde_json::json!({}),
            "case {case:?}: expected no failed resource, got {body}"
        );
    }

    // A refused map writes nothing: the tags held after it are exactly the ones
    // the admitted requests above wrote.
    let (status, body) = call(
        &router,
        "TagResources",
        tag_resources_body(
            SERVED,
            &[tag("rejected", value_of(TAG_VALUE_MAX_LENGTH + 1))],
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a refused map: expected HTTP 400, got {status} with {body}"
    );

    let (status, page) = call(
        &router,
        "GetResources",
        format!("{{\"ResourceARNList\":[\"{SERVED}\"]}}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case read-back: expected HTTP 200, got {status} with {page}"
    );
    let mapping = page["ResourceTagMappingList"]
        .as_array()
        .unwrap_or_else(|| panic!("a GetResources body with no ResourceTagMappingList: {page}"))
        .iter()
        .find(|mapping| mapping["ResourceARN"] == serde_json::json!(SERVED))
        .unwrap_or_else(|| panic!("{SERVED} is not in the page: {page}"));
    let mut held: Vec<String> = mapping["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("a mapping with no Tags: {mapping}"))
        .iter()
        .map(|tag| tag["Key"].as_str().expect("Tag.Key").to_string())
        .collect();
    held.sort();
    assert_eq!(
        held,
        vec!["at-max".to_string(), "at-min".to_string()],
        "case read-back: a refused map writes no tag, got {held:?}"
    );
}

/// `ExcludeCompliantResources` without `IncludeComplianceDetails` set to true is
/// refused by naming the member that has to be ADDED and the value it has to
/// carry — the same text for every form of that member's absence, and never the
/// member the caller did send.
#[tokio::test]
async fn a_member_admitted_only_alongside_another_names_the_member_that_must_be_added() {
    let router = emulator();

    let cases: &[(&str, &str)] = &[
        (
            "the enabling member absent",
            "{\"ExcludeCompliantResources\":true}",
        ),
        (
            "the enabling member explicitly false",
            "{\"ExcludeCompliantResources\":true,\"IncludeComplianceDetails\":false}",
        ),
        (
            "the enabling member false beside another filter",
            "{\"ExcludeCompliantResources\":true,\"IncludeComplianceDetails\":false,\
             \"ResourceTypeFilters\":[\"s3\"]}",
        ),
    ];

    let expected = "You must specify the IncludeComplianceDetails parameter and set it to true.";
    for (case, body) in cases {
        let (status, answer) = call(&router, "GetResources", (*body).to_string()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?}: got {answer}"
        );
        let actual = error_message(&answer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // The refusal is about the ABSENT member, so a request that supplies it is
    // not refused with this text, whatever else it is answered with.
    let (status, answer) = call(
        &router,
        "GetResources",
        "{\"ExcludeCompliantResources\":true,\"IncludeComplianceDetails\":true}".to_string(),
    )
    .await;
    let message = answer["message"]
        .as_str()
        .or_else(|| answer["Message"].as_str())
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        message, expected,
        "case the enabling member supplied: expected another answer than the missing-member \
         refusal, got {status} with {answer}"
    );
}
