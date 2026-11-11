// SPDX-License-Identifier: CC0-1.0

//! The `FeatureGroupNameOrArn` constraint, over every operation that takes one.
//!
//! All seven operations name their feature group with the same shape — Smithy
//! `com.amazonaws.sagemakerfeaturestoreruntime#FeatureGroupNameOrArn`, `@length` 1..150
//! and the `@pattern` quoted in [`FEATURE_GROUP_NAME_PATTERN`] — so a value outside it
//! is refused at the boundary, before any store lookup, with HTTP 400
//! `ValidationException` naming the offending MEMBER PATH and the constraint it violated.
//! That is the rule these tests assert: the member path differs per operation (a list
//! element reports as `<member>.<n>.member.<field>`), the constraint text is the model's
//! own, and the recorded wire case is one row of the table.
//!
//! Every case goes through the real front door — the multi-service router and the
//! restJson1 protocol — so what is asserted is the wire behaviour a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_common::feature_store_groups::{FeatureGroupDirectory, FeatureGroupRecord};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService;

/// The `@pattern` of `FeatureGroupNameOrArn` as a refusal QUOTES it: the model's
/// expression without the `^`/`$` anchors, which is how AWS states the requirement.
///
/// Source: `smithy_query(query="shape", shape="FeatureGroupNameOrArn")` on this crate's
/// `smithy-model/` gives `^(arn:aws[a-z\-]*:…)?([a-zA-Z0-9]…){0,63})$`, the same text
/// every operation's module header carries. The refusal message must quote that
/// expression, so it is spelled out once here rather than matched loosely.
const FEATURE_GROUP_NAME_PATTERN: &str =
    r"(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})";

/// A booted emulator holding this service only.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Create a feature group with an OnlineStore, the way the SageMaker control plane's
    /// `CreateFeatureGroup` does: by registering its definition in the ONE shared
    /// feature-group directory this data plane resolves names against. A name that
    /// satisfies the modelled constraint still has to name a group that EXISTS before an
    /// operation will serve it.
    fn create_feature_group(&self, name: &str) {
        self.engine
            .state::<FeatureGroupDirectory>()
            .lock()
            .register(FeatureGroupRecord {
                name: name.to_owned(),
                arn: format!("arn:aws:sagemaker:us-east-1:000000000000:feature-group/{name}"),
                record_identifier_feature_name: "record_id".to_owned(),
                event_time_feature_name: "event_time".to_owned(),
                online_store_enabled: true,
                // Online-only feature groups: no offline store to replicate into.
                offline_store: None,
                default_ttl_seconds: None,
            });
    }

    /// Send one request and return its status and parsed JSON body (`Value::Null` for an
    /// empty body).
    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let (status, _, _, parsed) = self.send_reading_headers(method, uri, body).await;
        (status, parsed)
    }

    /// [`Self::send`], also returning the response's `content-type` and
    /// `x-amzn-errortype` — the two restJson1 error-envelope headers an SDK matches an
    /// error shape on.
    async fn send_reading_headers(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (u16, String, String, Value) {
        // The SigV4 credential scope is how the router picks the claiming service: the
        // 4th field is this service's signing name.
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20240101/us-east-1/sagemaker/aws4_request, \
                 SignedHeaders=host, Signature=0000",
            )
            .header("host", "featurestore-runtime.sagemaker.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .body(
                body.map(|value| bytes::Bytes::from(value.to_string()))
                    .unwrap_or_default(),
            )
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        let content_type = header("content-type");
        let errortype = header("x-amzn-errortype");
        let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
        (status, content_type, errortype, parsed)
    }
}

/// The wire error code of an error response: restJson1 carries it as the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The `message` member of an error response.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// One `FeatureValue`, for the operations whose required `Record`/`Features` member must
/// be present before the name is even looked at.
fn feature(name: &str, value: &str) -> Value {
    json!({ "FeatureName": name, "ValueAsString": value })
}

#[tokio::test]
async fn every_operation_refuses_a_feature_group_name_outside_the_modeled_pattern() {
    let emulator = Emulator::new();

    // The one malformed value, sent to all seven operations. Only the MEMBER PATH
    // differs: a top-level `@httpLabel` reports as `featureGroupName`, while the two
    // batch operations carry the name inside a list element and report
    // `<list>.<1-based index>.member.featureGroupName`.
    //
    // (case, method, uri, body, member path)
    let rows: [(&str, &str, &str, Option<Value>, &str); 7] = [
        (
            "GetRecord",
            "GET",
            "/FeatureGroup/bad!name?RecordIdentifierValueAsString=1",
            None,
            "featureGroupName",
        ),
        (
            "PutRecord",
            "PUT",
            "/FeatureGroup/bad!name",
            Some(json!({ "Record": [feature("f1", "v1")] })),
            "featureGroupName",
        ),
        (
            "DeleteRecord",
            "DELETE",
            "/FeatureGroup/bad!name?RecordIdentifierValueAsString=1&EventTime=2026-01-01T00:00:00Z",
            None,
            "featureGroupName",
        ),
        (
            "ListRecords",
            "POST",
            "/FeatureGroup/bad!name/ListRecords",
            Some(json!({})),
            "featureGroupName",
        ),
        (
            "UpdateRecord",
            "POST",
            "/FeatureGroup/bad!name/Record",
            Some(json!({
                "RecordIdentifierValueAsString": "1",
                "Features": [feature("f1", "v1")],
            })),
            "featureGroupName",
        ),
        (
            "BatchGetRecord",
            "POST",
            "/BatchGetRecord",
            Some(json!({
                "Identifiers": [{
                    "FeatureGroupName": "bad!name",
                    "RecordIdentifiersValueAsString": ["1"],
                }],
            })),
            "identifiers.1.member.featureGroupName",
        ),
        (
            "BatchWriteRecord",
            "POST",
            "/BatchWriteRecord",
            Some(json!({
                "Entries": [{
                    "FeatureGroupName": "bad!name",
                    "Record": [feature("f1", "v1")],
                }],
            })),
            "entries.1.member.featureGroupName",
        ),
    ];

    for (case, method, uri, body, member_path) in rows {
        let (status, parsed) = emulator.send(method, uri, body).await;
        let code = error_code(&parsed);
        let text = message(&parsed);
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException for a feature group name \
             outside the modelled pattern, got {status} {code:?} with message {text:?}"
        );
        let expected = format!(
            "Value 'bad!name' at '{member_path}' failed to satisfy constraint: Member must \
             satisfy regular expression pattern: {FEATURE_GROUP_NAME_PATTERN}"
        );
        assert!(
            text.contains(&expected),
            "case {case:?}: expected the refusal to state {expected:?}, got {text:?}"
        );
    }
}

/// What a row expects of the refusal, or that the name was accepted and the request
/// reached the store.
#[derive(Debug, Clone, Copy)]
enum Verdict {
    /// Accepted: the name satisfies the modelled constraint, so the request reaches the
    /// store. The row's feature group is created first, so what the operation answers is
    /// its own empty result rather than a refusal of the NAME.
    Accepted,
    /// Refused with a clause containing this text.
    Refused(&'static str),
}

#[tokio::test]
async fn the_feature_group_name_constraint_is_the_modeled_one() {
    let emulator = Emulator::new();

    // The pattern admits a bare name — one alphanumeric, then up to 63 more groups of
    // optional `-`/`_` runs followed by an alphanumeric, so at most 64 alphanumerics —
    // or that name behind a `feature-group/` ARN prefix. These rows walk that: the
    // shortest admitted name, both separators, the 64-character bound and one past it,
    // and the three ways the bare form can be malformed.
    //
    // (case, value, verdict)
    let rows: [(&str, String, Verdict); 9] = [
        ("one character", "a".to_string(), Verdict::Accepted),
        ("digits and letters", "FG1".to_string(), Verdict::Accepted),
        (
            "both separators",
            "my_feature-group_1".to_string(),
            Verdict::Accepted,
        ),
        (
            "the pattern's 64-character bound",
            "a".repeat(64),
            Verdict::Accepted,
        ),
        (
            "one character past the pattern's bound",
            "a".repeat(65),
            Verdict::Refused("Member must satisfy regular expression pattern"),
        ),
        (
            "a character the pattern does not admit",
            "bad!name".to_string(),
            Verdict::Refused("Member must satisfy regular expression pattern"),
        ),
        (
            "a separator in the leading position",
            "-leading".to_string(),
            Verdict::Refused("Member must satisfy regular expression pattern"),
        ),
        (
            "a separator in the trailing position",
            "trailing_".to_string(),
            Verdict::Refused("Member must satisfy regular expression pattern"),
        ),
        (
            "a space, as the recorded request sent, percent-encoded in the path",
            "not%20a%20valid%20name!!".to_string(),
            Verdict::Refused("Member must satisfy regular expression pattern"),
        ),
    ];

    for (case, value, verdict) in rows {
        if matches!(verdict, Verdict::Accepted) {
            emulator.create_feature_group(&value);
        }
        let uri = format!("/FeatureGroup/{value}?RecordIdentifierValueAsString=1");
        let (status, parsed) = emulator.send("GET", &uri, None).await;
        let code = error_code(&parsed);
        let text = message(&parsed);
        match verdict {
            Verdict::Accepted => assert_eq!(
                (status, code.as_str(), parsed.get("Record").is_none()),
                (200, "", true),
                "case {case:?}: {value:?} satisfies the modelled constraint, so GetRecord \
                 must reach the store and answer the empty result of a feature group \
                 holding no such record, got {status} {code:?} with body {parsed:?}"
            ),
            Verdict::Refused(clause) => {
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected HTTP 400 ValidationException for {value:?}, got \
                     {status} {code:?} with message {text:?}"
                );
                assert!(
                    text.contains("at 'featureGroupName' failed to satisfy constraint")
                        && text.contains(clause),
                    "case {case:?}: expected the refusal of {value:?} to name \
                     'featureGroupName' and state {clause:?}, got {text:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_name_forms_a_uri_label_cannot_carry_are_constrained_the_same_way() {
    let emulator = Emulator::new();

    // An ARN carries `/`, and the empty name carries nothing, so neither fits a
    // single-segment `@httpLabel`: these forms are exercised through the same shape
    // where it is a BODY member, `BatchGetRecordIdentifier$FeatureGroupName`. The two
    // `@length` bounds (1..150) are only reachable here.
    //
    // (case, value, verdict)
    let rows: [(&str, String, Verdict); 6] = [
        (
            "an ARN in the commercial partition",
            "arn:aws:sagemaker:us-east-1:000000000000:feature-group/fg-1".to_string(),
            Verdict::Accepted,
        ),
        (
            "an ARN in another partition, as the pattern's `aws[a-z\\-]*` admits",
            "arn:aws-us-gov:sagemaker:us-gov-west-1:000000000000:feature-group/fg-1".to_string(),
            Verdict::Accepted,
        ),
        (
            "an ARN whose account id is not 12 digits",
            "arn:aws:sagemaker:us-east-1:12345:feature-group/fg-1".to_string(),
            Verdict::Refused("Member must satisfy regular expression pattern"),
        ),
        (
            "an ARN of another resource type",
            "arn:aws:sagemaker:us-east-1:000000000000:endpoint/fg-1".to_string(),
            Verdict::Refused("Member must satisfy regular expression pattern"),
        ),
        (
            "the empty name, one below the modelled minimum length",
            String::new(),
            Verdict::Refused("Member must have length greater than or equal to 1"),
        ),
        (
            "one character past the modelled maximum length",
            "a".repeat(151),
            Verdict::Refused("Member must have length less than or equal to 150"),
        ),
    ];

    for (case, value, verdict) in rows {
        // Every accepted row names the same feature group (`fg-1`, behind an ARN prefix
        // the pattern makes optional), so creating it once covers them all.
        emulator.create_feature_group("fg-1");
        let body = json!({
            "Identifiers": [{
                "FeatureGroupName": value,
                "RecordIdentifiersValueAsString": ["1"],
            }],
        });
        let (status, parsed) = emulator.send("POST", "/BatchGetRecord", Some(body)).await;
        let code = error_code(&parsed);
        let text = message(&parsed);
        match verdict {
            Verdict::Accepted => {
                assert_eq!(
                    (status, code.as_str()),
                    (200, ""),
                    "case {case:?}: {value:?} satisfies the modelled constraint, so \
                     BatchGetRecord must reach the store, got {status} {code:?} with message \
                     {text:?}"
                );
                // The ARN and the bare name reach ONE feature group, so a name that
                // resolves reports no per-identifier error at all — the identifier is
                // simply absent from the group.
                let reported = parsed.get("Errors").cloned().unwrap_or(Value::Null);
                assert_eq!(
                    reported,
                    json!([]),
                    "case {case:?}: expected {value:?} to resolve to the created feature \
                     group and report no error, got {reported:?} in {parsed:?}"
                );
            }
            Verdict::Refused(clause) => {
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: expected HTTP 400 ValidationException for {value:?}, got \
                     {status} {code:?} with message {text:?}"
                );
                assert!(
                    text.contains("at 'identifiers.1.member.featureGroupName' failed to satisfy \
                                   constraint")
                        && text.contains(clause),
                    "case {case:?}: expected the refusal of {value:?} to name \
                     'identifiers.1.member.featureGroupName' and state {clause:?}, got {text:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_refusal_is_carried_in_this_services_restjson1_error_envelope() {
    let emulator = Emulator::new();

    // restJson1 error documents are `application/json` and name the error shape in
    // `x-amzn-errortype`, which is what an SDK matches an error shape on — the two
    // headers the recorded responses carry, and the two the router's own pre-routing
    // refusal got wrong (it answered an awsJson1_1 `application/x-amz-json-1.1` body).
    // Asserted over a path-bound and a body-bound member, and over a refusal of each
    // class (a pattern and an absent required member), so the envelope is pinned for the
    // whole refusal channel rather than for one request.
    //
    // (case, method, uri, body)
    let rows: [(&str, &str, &str, Option<Value>); 3] = [
        (
            "GetRecord, a path-bound name outside the pattern",
            "GET",
            "/FeatureGroup/bad!name?RecordIdentifierValueAsString=1",
            None,
        ),
        (
            "BatchGetRecord, a body-bound name outside the pattern",
            "POST",
            "/BatchGetRecord",
            Some(json!({
                "Identifiers": [{
                    "FeatureGroupName": "bad!name",
                    "RecordIdentifiersValueAsString": ["1"],
                }],
            })),
        ),
        (
            "BatchGetRecord, a required member absent",
            "POST",
            "/BatchGetRecord",
            Some(json!({})),
        ),
    ];

    for (case, method, uri, body) in rows {
        let (status, content_type, errortype, parsed) =
            emulator.send_reading_headers(method, uri, body).await;
        assert_eq!(
            (status, content_type.as_str(), errortype.as_str()),
            (400, "application/json", "ValidationException"),
            "case {case:?}: expected the restJson1 refusal envelope (HTTP 400, \
             application/json, x-amzn-errortype ValidationException), got {status} \
             {content_type:?} {errortype:?} with body {parsed:?}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: expected the body's __type to name the same error shape as the \
             header, got {parsed:?}"
        );
    }
}
