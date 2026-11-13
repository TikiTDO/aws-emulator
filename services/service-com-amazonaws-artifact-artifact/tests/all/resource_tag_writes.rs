// SPDX-License-Identifier: CC0-1.0

//! `TagResource` and `UntagResource` for AWS Artifact, end to end over HTTP.
//!
//! Every case goes through the router — routing, the front door, deserialization, the
//! handler, the persisted state and serialization — so an answer that is not computed
//! from persisted state cannot pass. The rules under test, asserted over TABLES rather
//! than one recorded request:
//!
//! 1. A tag set BELONGS TO THE RESOURCE its `resourceArn` names and is persisted with it:
//!    `TagResource` writes each named key absolutely (a key the resource carries takes
//!    the request's value, one it does not is added, and keys the request does not name
//!    are left alone) and `UntagResource` removes each named key. Both readers of that
//!    one set — `ListTagsForResource` and `GetComplianceInquiryMetadata` — report what
//!    the write persisted.
//! 2. Both writes are IDEMPOTENT: the same request twice leaves the same tag set, and
//!    `UntagResource` on a key the resource does not carry is not an error.
//! 3. A resource holds at most as many tags as `TagsMap` admits (`@length(max: 50)`): a
//!    write that would take it past that is refused with the modelled 400
//!    `ValidationException`, and the resource keeps the tag set it had.
//! 4. An ARN that names no resource this account holds is refused HTTP 401
//!    `UnauthorizedException` by both writes — whether the account holds the resource is
//!    not information this API discloses — and the refusal changes nothing.
//!
//! The rows differ along what the rules generalise over: how many tags are written and
//! in how many calls, whether a key is new or already carried, the form of the key and
//! of the value (the pattern's whole character set, both length bounds, an empty value),
//! how many keys an untag names and whether the resource carries them, and which field
//! of the ARN names no resource.
//!
//! The READ side of the same tag set — `ListTagsForResource` on its own, including its
//! full table of ARNs this account holds no resource for — is `resource_tags.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for: an inquiry's ARN names the
/// region of the request that created it.
const REGION: &str = "eu-west-1";

/// How many tags one resource holds at most, from `TagsMap`'s modelled
/// `@length(max: 50)` (`smithy_query(query="shape",shape="TagsMap")`). Spelled out here
/// rather than imported from the crate, so a wrong constant in `src/state.rs` cannot make
/// this suite pass.
const TAG_CEILING: usize = 50;

/// `TagKey`'s modelled ceiling, `@length(min: 1, max: 128)`.
const TAG_KEY_CEILING: usize = 128;

/// `TagValue`'s modelled ceiling, `@length(min: 0, max: 256)` — so an EMPTY value is a
/// value the model admits.
const TAG_VALUE_CEILING: usize = 256;

/// Every character `TagKey` and `TagValue`'s pattern `^[a-zA-Z0-9\s_.:/=+\-@]*$` admits
/// beside the alphanumerics, gathered into one key: `_ . : / = + - @` and a space (`\s`).
const PATTERN_CHARS_KEY: &str = "a_b.c:d/e=f+g-h@i j";

/// The front door's authorization refusal: the status and the wire error code every
/// resource lookup in this service answers a target it cannot authorize with.
const UNAUTHORIZED: (u16, &str) = (401, "UnauthorizedException");

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 credential scope for `region` naming this service — what routes the request
/// and what region an inquiry ARN is built for.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

/// Dispatch one request through the router, signed for `REGION`.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    call_signed_for(router, REGION, method, uri, body).await
}

/// Dispatch one request through the router, signed for `region`.
async fn call_signed_for(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", signed_for(region))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// `resourceArn` is an `@httpLabel` on `/tags/{resourceArn}`, and a non-greedy label is
/// ONE path segment: the `/` inside an ARN's resource field has to be written `%2F` or it
/// would split the segment. `:` is a legal path character, so it travels as-is.
fn label(resource_arn: &str) -> String {
    resource_arn.replace('/', "%2F")
}

/// One query-string value, percent-encoded. A tag key may carry `/`, `=`, `+`, `:` and
/// whitespace, each of which means something else in a query string, so the value is
/// encoded rather than interpolated. Written out instead of pulled from
/// `percent-encoding`, which this workspace does not declare: this is the whole of the
/// encoding one query value needs.
fn query_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Tag pairs as the `TagsMap` a request body carries.
fn tags_object(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// `&[(&str, &str)]` as owned, comparable tag pairs.
fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The tags a body reports, in a comparable order. `None` when it carries no `tags`
/// member at all — which is not the same answer as an empty map, so the two are
/// distinguished rather than defaulted together.
fn reported_tags(body: &Value) -> Option<BTreeMap<String, String>> {
    let tags = body.get("tags")?.as_object()?;
    Some(
        tags.iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect(),
    )
}

/// The expected tag set, in the same comparable order.
fn expected_tags(tags: &[(String, String)]) -> BTreeMap<String, String> {
    tags.iter().cloned().collect()
}

/// `POST /tags/{resourceArn}` — `TagResource`.
async fn tag(router: &MultiServiceRouter, resource_arn: &str, tags: &[(String, String)]) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", label(resource_arn)),
        Some(json!({ "tags": tags_object(tags) })),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is a
/// `@httpQuery` list, so each key is one repetition of the parameter.
async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[String]) -> Answer {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect();
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{}", label(resource_arn), query.join("&")),
        None,
    )
    .await
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`, one of the two readers of the tag
/// set a write persisted.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", label(resource_arn)), None).await
}

/// `GET /v1/compliance-inquiry/getMetadata` — the other reader of the same tag set.
async fn get_metadata(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"),
        None,
    )
    .await
}

/// Create one compliance inquiry carrying `tags`, and answer its id and its ARN — the
/// ARN the service minted, which is what the tag operations address it by.
async fn create_inquiry(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(String, String)],
) -> (String, String) {
    let mut body = json!({
        "name": name,
        "inquiryContent": { "query": "Which controls does this report cover?" },
        "supportMode": "AI_ONLY"
    });
    if !tags.is_empty() {
        body["tags"] = tags_object(tags);
    }
    let created = call(router, "POST", "/v1/compliance-inquiry/create", Some(body)).await;
    assert_eq!(
        created.status, 202,
        "creating {name:?} must answer HTTP 202, got {} with {:?}",
        created.status, created.body
    );
    let summary = &created.body["complianceInquirySummary"];
    let id = summary["id"].as_str().unwrap_or_default().to_string();
    let arn = summary["arn"].as_str().unwrap_or_default().to_string();
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "creating {name:?} must report an id and an ARN, got {:?}",
        created.body
    );
    (id, arn)
}

/// Assert that the resource at `resource_arn` (the inquiry `inquiry_id`) carries exactly
/// `expected`, as reported by BOTH readers of that one tag set.
async fn assert_tag_set(
    router: &MultiServiceRouter,
    case: &str,
    inquiry_id: &str,
    resource_arn: &str,
    expected: &[(String, String)],
) {
    let want = expected_tags(expected);
    let listed = list_tags(router, resource_arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListTagsForResource must answer HTTP 200, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        reported_tags(&listed.body),
        Some(want.clone()),
        "case {case:?}: ListTagsForResource must report the persisted tag set (present \
         even when empty), got {:?}",
        listed.body
    );

    // A read changes nothing a later caller sees.
    let again = list_tags(router, resource_arn).await;
    assert_eq!(
        reported_tags(&again.body),
        reported_tags(&listed.body),
        "case {case:?}: a second read must report the same tag set, got {:?} after {:?}",
        again.body,
        listed.body
    );

    let read = get_metadata(router, inquiry_id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: GetComplianceInquiryMetadata must answer HTTP 200, got {} with {:?}",
        read.status, read.body
    );
    // Both readers report the SAME tag set in the same form: the member is present
    // whatever the tag set, and the empty map stands for a resource with no tags.
    let from_metadata = reported_tags(&read.body);
    assert_eq!(
        from_metadata,
        Some(want.clone()),
        "case {case:?}: the metadata read must report the same tag set (present even when \
         empty), got {:?}",
        read.body
    );
}

/// Assert that `answer` carries no members: `TagResourceResponse` and
/// `UntagResourceResponse` model none, so the document the protocol renders for them is
/// empty (or absent altogether).
fn assert_no_members(case: &str, what: &str, answer: &Answer) {
    let empty = match &answer.body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case:?}: {what} answers no members, got {:?}",
        answer.body
    );
}

/// One tag-write case: what the inquiry was created with, the writes applied to it in
/// order, and the tag set it must carry afterwards.
struct WriteCase {
    label: &'static str,
    created_with: Vec<(String, String)>,
    writes: Vec<Vec<(String, String)>>,
    expected: Vec<(String, String)>,
}

/// The tag writes this suite covers.
fn write_cases() -> Vec<WriteCase> {
    // The pattern's character set, both length bounds and an empty value, in one row:
    // the catalogue is `TagKey`/`TagValue`'s own, from
    // `smithy_query(query="shape",shape="TagsMap")`.
    let pattern_forms: Vec<(String, String)> = vec![
        (
            PATTERN_CHARS_KEY.to_string(),
            "every allowed char".to_string(),
        ),
        ("k".to_string(), String::new()),
        ("K".repeat(TAG_KEY_CEILING), "at the key ceiling".to_string()),
        ("long-value".to_string(), "v".repeat(TAG_VALUE_CEILING)),
    ];

    // The published ceiling itself: a resource may hold as many tags as `TagsMap`
    // admits, and this row writes exactly that many in one call.
    let at_the_ceiling: Vec<(String, String)> = (0..TAG_CEILING)
        .map(|index| (format!("key{index:02}"), format!("value{index:02}")))
        .collect();

    vec![
        WriteCase {
            label: "one tag onto an untagged inquiry",
            created_with: Vec::new(),
            writes: vec![pairs(&[("team", "compliance")])],
            expected: pairs(&[("team", "compliance")]),
        },
        WriteCase {
            label: "three tags in one call",
            created_with: Vec::new(),
            writes: vec![pairs(&[
                ("team", "legal"),
                ("env", "prod"),
                ("owner", "a.b@c"),
            ])],
            expected: pairs(&[("team", "legal"), ("env", "prod"), ("owner", "a.b@c")]),
        },
        WriteCase {
            label: "three tags over three calls",
            created_with: Vec::new(),
            writes: vec![
                pairs(&[("team", "legal")]),
                pairs(&[("env", "prod")]),
                pairs(&[("owner", "a.b@c")]),
            ],
            expected: pairs(&[("team", "legal"), ("env", "prod"), ("owner", "a.b@c")]),
        },
        WriteCase {
            label: "a key the create persisted is overwritten, another is added",
            created_with: pairs(&[("team", "compliance"), ("env", "dev")]),
            writes: vec![pairs(&[("env", "prod"), ("owner", "a.b@c")])],
            expected: pairs(&[("team", "compliance"), ("env", "prod"), ("owner", "a.b@c")]),
        },
        WriteCase {
            label: "the same write twice leaves the same tag set",
            created_with: pairs(&[("team", "compliance")]),
            writes: vec![pairs(&[("env", "prod")]), pairs(&[("env", "prod")])],
            expected: pairs(&[("team", "compliance"), ("env", "prod")]),
        },
        WriteCase {
            label: "an empty tags map leaves the tag set alone",
            created_with: pairs(&[("team", "legal")]),
            writes: vec![Vec::new()],
            expected: pairs(&[("team", "legal")]),
        },
        WriteCase {
            label: "no write at all on an untagged inquiry",
            created_with: Vec::new(),
            writes: Vec::new(),
            expected: Vec::new(),
        },
        WriteCase {
            label: "every key and value form the pattern and the bounds admit",
            created_with: Vec::new(),
            writes: vec![pattern_forms.clone()],
            expected: pattern_forms,
        },
        WriteCase {
            label: "as many tags as the map admits, in one call",
            created_with: Vec::new(),
            writes: vec![at_the_ceiling.clone()],
            expected: at_the_ceiling,
        },
    ]
}

#[tokio::test]
async fn a_tag_write_is_absolute_per_key_and_reads_back_through_both_readers() {
    let router = emulator();

    for case in write_cases() {
        let WriteCase {
            label,
            created_with,
            writes,
            expected,
        } = case;
        let (id, arn) = create_inquiry(&router, label, &created_with).await;

        for (index, write) in writes.iter().enumerate() {
            let answer = tag(&router, &arn, write).await;
            assert_eq!(
                answer.status, 204,
                "case {label:?}: write {index} must answer HTTP 204, got {} with {:?}",
                answer.status, answer.body
            );
            assert_no_members(label, &format!("write {index}"), &answer);
        }

        assert_tag_set(&router, label, &id, &arn, &expected).await;
    }
}

/// One untag case: what the inquiry was created with, the key lists removed in order,
/// and the tag set it must carry afterwards.
struct UntagCase {
    label: &'static str,
    created_with: Vec<(String, String)>,
    removals: Vec<Vec<String>>,
    expected: Vec<(String, String)>,
}

fn untag_cases() -> Vec<UntagCase> {
    let three = pairs(&[("team", "legal"), ("env", "prod"), ("owner", "a.b@c")]);
    let keys = |keys: &[&str]| -> Vec<String> { keys.iter().map(|k| (*k).to_string()).collect() };

    vec![
        UntagCase {
            label: "one key of three",
            created_with: three.clone(),
            removals: vec![keys(&["env"])],
            expected: pairs(&[("team", "legal"), ("owner", "a.b@c")]),
        },
        UntagCase {
            label: "two keys at once",
            created_with: three.clone(),
            removals: vec![keys(&["env", "owner"])],
            expected: pairs(&[("team", "legal")]),
        },
        UntagCase {
            label: "every key the resource carries",
            created_with: three.clone(),
            removals: vec![keys(&["team", "env", "owner"])],
            expected: Vec::new(),
        },
        UntagCase {
            label: "the same key twice is idempotent",
            created_with: three.clone(),
            removals: vec![keys(&["env"]), keys(&["env"])],
            expected: pairs(&[("team", "legal"), ("owner", "a.b@c")]),
        },
        UntagCase {
            label: "a key the resource does not carry is not an error",
            created_with: three.clone(),
            removals: vec![keys(&["never-set"])],
            expected: three.clone(),
        },
        UntagCase {
            label: "a carried key beside one the resource does not carry",
            created_with: three.clone(),
            removals: vec![keys(&["team", "never-set"])],
            expected: pairs(&[("env", "prod"), ("owner", "a.b@c")]),
        },
        UntagCase {
            label: "a key spelling the pattern's whole character set",
            created_with: vec![
                (PATTERN_CHARS_KEY.to_string(), "kept out".to_string()),
                ("team".to_string(), "legal".to_string()),
            ],
            removals: vec![vec![PATTERN_CHARS_KEY.to_string()]],
            expected: pairs(&[("team", "legal")]),
        },
    ]
}

#[tokio::test]
async fn an_untag_removes_only_the_named_keys_and_is_idempotent() {
    let router = emulator();

    for case in untag_cases() {
        let UntagCase {
            label,
            created_with,
            removals,
            expected,
        } = case;
        let (id, arn) = create_inquiry(&router, label, &created_with).await;

        for (index, keys) in removals.iter().enumerate() {
            let answer = untag(&router, &arn, keys).await;
            assert_eq!(
                answer.status, 204,
                "case {label:?}: removal {index} must answer HTTP 204, got {} with {:?}",
                answer.status, answer.body
            );
            assert_no_members(label, &format!("removal {index}"), &answer);
        }

        assert_tag_set(&router, label, &id, &arn, &expected).await;
    }
}

#[tokio::test]
async fn a_write_past_the_per_resource_tag_ceiling_is_refused_and_changes_nothing() {
    let router = emulator();

    let at_the_ceiling: Vec<(String, String)> = (0..TAG_CEILING)
        .map(|index| (format!("key{index:02}"), format!("value{index:02}")))
        .collect();
    let (id, arn) = create_inquiry(&router, "at the tag ceiling", &[]).await;

    // The published bound itself is accepted.
    let filled = tag(&router, &arn, &at_the_ceiling).await;
    assert_eq!(
        filled.status, 204,
        "case \"fill to the ceiling\": expected HTTP 204, got {} with {:?}",
        filled.status, filled.body
    );
    assert_tag_set(&router, "fill to the ceiling", &id, &arn, &at_the_ceiling).await;

    // One past it: the key is new, so the resource would hold TAG_CEILING + 1 tags.
    let past = tag(&router, &arn, &pairs(&[("one-key-too-many", "refused")])).await;
    assert_eq!(
        past.status, 400,
        "case \"one key past the ceiling\": expected HTTP 400, got {} with {:?}",
        past.status, past.body
    );
    assert_eq!(
        past.error_code(),
        "ValidationException",
        "case \"one key past the ceiling\": expected the modelled ValidationException, got {:?}",
        past.body
    );
    assert_eq!(
        past.body["reason"].as_str(),
        Some("fieldValidationFailed"),
        "case \"one key past the ceiling\": expected ValidationExceptionReason \
         fieldValidationFailed, got {:?}",
        past.body
    );
    assert_eq!(
        past.body["fieldList"][0]["name"].as_str(),
        Some("tags"),
        "case \"one key past the ceiling\": the fieldList must name the refused member, \
         got {:?}",
        past.body
    );
    assert_tag_set(
        &router,
        "one key past the ceiling",
        &id,
        &arn,
        &at_the_ceiling,
    )
    .await;

    // A key the resource ALREADY carries is still accepted at the ceiling: the bound is
    // on how many tags the resource holds, not on how many a request names.
    let overwrite = tag(&router, &arn, &pairs(&[("key00", "rewritten")])).await;
    assert_eq!(
        overwrite.status, 204,
        "case \"overwrite at the ceiling\": expected HTTP 204, got {} with {:?}",
        overwrite.status, overwrite.body
    );
    let mut rewritten = at_the_ceiling.clone();
    rewritten[0] = ("key00".to_string(), "rewritten".to_string());
    assert_tag_set(&router, "overwrite at the ceiling", &id, &arn, &rewritten).await;

    // One more entry than `TagsMap` admits in a single request is refused before the
    // handler, as a request the gateway cannot bind: `front_door_errors.rs` and
    // `resource_tags.rs` pin that refusal's identity, so this row only asserts that it
    // IS refused and that the resource keeps its tag set.
    let too_many: Vec<(String, String)> = (0..=TAG_CEILING)
        .map(|index| (format!("extra{index:02}"), "refused".to_string()))
        .collect();
    let refused = tag(&router, &arn, &too_many).await;
    assert_eq!(
        refused.status, 400,
        "case \"a request past the map's entry count\": expected HTTP 400, got {} with {:?}",
        refused.status, refused.body
    );
    assert_tag_set(
        &router,
        "a request past the map's entry count",
        &id,
        &arn,
        &rewritten,
    )
    .await;
}

/// Every row is a well-formed ARN: a value the tagging surface cannot read an ARN out of
/// at all is a request-shape failure refused 400 before this rule is reached, which
/// `tagging_arn_syntax.rs` pins.
#[tokio::test]
async fn an_arn_naming_no_resource_this_account_holds_is_refused_unauthorized() {
    let router = emulator();

    let kept = pairs(&[("team", "compliance")]);
    let (id, arn) = create_inquiry(&router, "a tagged inquiry", &kept).await;

    // A report the catalog publishes, read from `ListReports`: it is a resource this
    // service NAMES, with an ARN of its own, and it is service-owned rather than held by
    // the account — so it is not a resource the caller may tag.
    let listed = call(&router, "GET", "/v1/report/list", None).await;
    assert_eq!(
        listed.status, 200,
        "ListReports must serve the catalog, got {} with {:?}",
        listed.status, listed.body
    );
    let report_arn = listed.body["reports"][0]["arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        report_arn.starts_with("arn:aws:artifact:"),
        "the catalog must report a report ARN, got {:?}",
        listed.body["reports"][0]
    );

    let account = engine::emulator_account_id().to_string();
    let suffix = id.strip_prefix("compliance-inquiry-").unwrap_or(&id);
    // One row per field of the ARN that can name no resource this account holds.
    let cases: Vec<(String, String)> = vec![
        (
            "a never-issued inquiry id".to_string(),
            format!(
                "arn:aws:artifact:{REGION}:{account}:compliance-inquiry/\
                 compliance-inquiry-0000000000000000"
            ),
        ),
        (
            "this inquiry in another region".to_string(),
            format!("arn:aws:artifact:us-east-1:{account}:compliance-inquiry/{id}"),
        ),
        (
            "this inquiry in another account".to_string(),
            format!("arn:aws:artifact:{REGION}:111122223333:compliance-inquiry/{id}"),
        ),
        (
            "this inquiry in another partition".to_string(),
            format!("arn:aws-cn:artifact:{REGION}:{account}:compliance-inquiry/{id}"),
        ),
        (
            "this inquiry's suffix under another resource type".to_string(),
            format!("arn:aws:artifact:{REGION}:{account}:report/report-{suffix}"),
        ),
        (
            "another service's ARN".to_string(),
            format!("arn:aws:s3:{REGION}:{account}:bucket/compliance"),
        ),
        ("a report the catalog publishes".to_string(), report_arn),
    ];

    for (case, resource_arn) in cases {
        let answers = [
            (
                "TagResource",
                tag(&router, &resource_arn, &pairs(&[("team", "intruder")])).await,
            ),
            (
                "UntagResource",
                untag(&router, &resource_arn, &["team".to_string()]).await,
            ),
        ];
        for (operation, answer) in answers {
            assert_eq!(
                answer.status,
                UNAUTHORIZED.0,
                "case {case:?} on {operation}: expected HTTP {}, got {} with {:?}",
                UNAUTHORIZED.0,
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.error_code(),
                UNAUTHORIZED.1,
                "case {case:?} on {operation}: expected __type {}, got {:?}",
                UNAUTHORIZED.1,
                answer.body
            );
        }
    }

    // None of those refusals touched the tag set of the resource the account does hold.
    assert_tag_set(&router, "the tagged inquiry survives", &id, &arn, &kept).await;
}

#[tokio::test]
async fn a_tag_set_follows_the_resource_and_not_the_request() {
    let router = emulator();

    // Two inquiries, created in two regions, each with its own tag set: a write
    // addressed to one ARN is a write to that one resource, and the region an inquiry's
    // ARN names is the one its create was signed for.
    let mut created = Vec::new();
    for (region, name, tags) in [
        ("eu-west-1", "first", pairs(&[("team", "legal")])),
        ("us-east-1", "second", pairs(&[("team", "compliance")])),
    ] {
        let body = json!({
            "name": name,
            "inquiryContent": { "query": "Which controls does this report cover?" },
            "supportMode": "AI_ONLY",
            "tags": tags_object(&tags)
        });
        let answer = call_signed_for(
            &router,
            region,
            "POST",
            "/v1/compliance-inquiry/create",
            Some(body),
        )
        .await;
        assert_eq!(
            answer.status, 202,
            "case {name:?}: creating in {region} must answer HTTP 202, got {} with {:?}",
            answer.status, answer.body
        );
        let summary = &answer.body["complianceInquirySummary"];
        let id = summary["id"].as_str().unwrap_or_default().to_string();
        let arn = summary["arn"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            arn,
            format!(
                "arn:aws:artifact:{region}:{}:compliance-inquiry/{id}",
                engine::emulator_account_id()
            ),
            "case {name:?}: the ARN must name the region the create was signed for, got {arn:?}"
        );
        created.push((name, id, arn, tags));
    }

    // Write to the first only; the second keeps the tag set its create persisted.
    let (first_name, first_id, first_arn, first_tags) = created[0].clone();
    let added = pairs(&[("env", "prod")]);
    let answer = tag(&router, &first_arn, &added).await;
    assert_eq!(
        answer.status, 204,
        "case {first_name:?}: expected HTTP 204, got {} with {:?}",
        answer.status, answer.body
    );

    let mut first_expected = first_tags;
    first_expected.extend(added);
    assert_tag_set(&router, first_name, &first_id, &first_arn, &first_expected).await;

    let (second_name, second_id, second_arn, second_tags) = created[1].clone();
    assert_tag_set(&router, second_name, &second_id, &second_arn, &second_tags).await;
}
