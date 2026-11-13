// SPDX-License-Identifier: CC0-1.0

//! The ONE `tags` projection every compliance-inquiry operation answers with, driven end
//! to end over HTTP through every operation that reports it.
//!
//! RULE: an operation whose output models a map member reporting a resource's tag set
//! serializes that member as PRESENT, carrying exactly the tag set the stored record
//! holds — the empty map when the record holds none, never an absent member. One rule, in
//! one place ([`ComplianceInquiry::reported_tags`]), so the create, the metadata read, the
//! export and the tag listing cannot disagree about what an untagged inquiry reports.
//!
//! The reporters are `CreateComplianceInquiry`, the metadata read
//! `GetComplianceInquiryMetadata`, `ExportComplianceInquiry` and the tag listing
//! `ListTagsForResource`; every case drives all of them over one stored record, so a
//! projection that differs between them fails.
//!
//! Rows differ along what the rule generalises over — the SIZE and the SHAPE of the tag
//! set: none at all, one entry, several, a value at `TagValue`'s published floor of 0
//! characters, a key and a value at their published ceilings, and a tag set at `TagsMap`'s
//! published entry ceiling. The emptiness the rule turns on is reached from both
//! directions: by creating an untagged inquiry, and by removing every tag of a tagged one.

use std::collections::BTreeMap;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router bootstrap, the credential scope and the region it signs for are the crate's
// test harness': one emulator over one shared engine.
use super::harness::{emulator, signed_for, REGION};

/// The endpoint host every request of this suite addresses the emulator on.
const ENDPOINT: &str = "artifact.eu-west-1.amazonaws.com";

/// `TagsMap`'s published entry ceiling — `@length(max: 50)`
/// (`smithy_query(query="shape",shape="TagsMap")`).
const MAX_TAG_ENTRIES: usize = 50;

/// `TagKey`'s published ceiling — `@length(min: 1, max: 128)` (same query).
const MAX_TAG_KEY: usize = 128;

/// `TagValue`'s published ceiling — `@length(min: 0, max: 256)` (same query); its floor of
/// 0 is why the empty value is a row below.
const MAX_TAG_VALUE: usize = 256;

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The `tags` member of this answer as a map, or `None` when the answer carries no
    /// such member at all. The two are what the rule distinguishes — an untagged resource
    /// reports the empty map, not nothing — so an absent member is never defaulted to the
    /// empty map here.
    fn reported_tags(&self) -> Option<BTreeMap<String, String>> {
        let member = self.body.get("tags")?;
        if member.is_null() {
            return None;
        }
        let object = member
            .as_object()
            .unwrap_or_else(|| panic!("a tags member must be a map, got {member:?}"));
        Some(
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect(),
        )
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", ENDPOINT)
        .header("content-type", "application/json")
        .header("authorization", signed_for(REGION))
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// `resourceArn` is an `@httpLabel` and a non-greedy label is ONE path segment, so the
/// `/` inside an ARN's resource field has to be written `%2F`.
fn label(resource_arn: &str) -> String {
    resource_arn.replace('/', "%2F")
}

/// A `tags` object for a request body.
fn tags_object(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// The tag set a case submitted, in the comparable form [`Answer::reported_tags`] answers
/// in.
fn expected(tags: &[(String, String)]) -> BTreeMap<String, String> {
    tags.iter().cloned().collect()
}

/// `POST /v1/compliance-inquiry/create` carrying `tags` — the first reporter, and what
/// persists the record the others read. `send_member` says whether the request carries a
/// `tags` member at all, which is the dimension the untagged rows differ along: the rule
/// is about what the RESPONSE reports, not about what the request wrote.
async fn create(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(String, String)],
    send_member: bool,
) -> Answer {
    let mut body = json!({
        "name": name,
        "inquiryContent": { "query": "Which controls does this report cover?" },
        "supportMode": "AI_ONLY",
    });
    if send_member {
        body["tags"] = tags_object(tags);
    }
    call(router, "POST", "/v1/compliance-inquiry/create", Some(body)).await
}

/// `GET /v1/compliance-inquiry/getMetadata` — the second reporter.
async fn get_metadata(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"),
        None,
    )
    .await
}

/// `POST /v1/compliance-inquiry/export` — the third reporter.
async fn export(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    call(
        router,
        "POST",
        "/v1/compliance-inquiry/export",
        Some(json!({ "complianceInquiryId": inquiry_id })),
    )
    .await
}

/// `GET /tags/{resourceArn}` — the tag LISTING, the fourth reporter of the same tag set.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", label(resource_arn)), None).await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`; `tagKeys` is a `@httpQuery`
/// list, so each key is one repetition of the parameter.
async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[String]) -> Answer {
    let query: Vec<String> = keys
        .iter()
        .map(|key| {
            format!(
                "tagKeys={}",
                ::percent_encoding::utf8_percent_encode(key, ::percent_encoding::NON_ALPHANUMERIC)
            )
        })
        .collect();
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{}", label(resource_arn), query.join("&")),
        None,
    )
    .await
}

/// One row of the table: a tag set the rule has to project the same way through every
/// reporter, and whether the create that persists it carries a `tags` member at all.
struct Case {
    label: &'static str,
    tags: Vec<(String, String)>,
    /// Whether the create request carries the `tags` member. The two untagged rows differ
    /// along exactly this: the response reports the resource's tag set either way.
    send_member: bool,
}

/// The tag sets under test. Every key and value is drawn from the character class
/// `TagKey`/`TagValue` publish (`^[a-zA-Z0-9\s_.:/=+\-@]*$`, same query as the ceilings
/// above).
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no tags member at all",
            tags: Vec::new(),
            send_member: false,
        },
        Case {
            label: "a tags member present and empty",
            tags: Vec::new(),
            send_member: true,
        },
        Case {
            label: "one tag",
            tags: vec![("Team".to_string(), "Compliance".to_string())],
            send_member: true,
        },
        Case {
            label: "several tags",
            tags: vec![
                ("Team".to_string(), "Compliance".to_string()),
                ("Environment".to_string(), "Production".to_string()),
                ("cost.center".to_string(), "eu/west=1".to_string()),
            ],
            send_member: true,
        },
        Case {
            label: "a value at TagValue's floor of 0 characters",
            tags: vec![
                ("Reviewed".to_string(), String::new()),
                ("Team".to_string(), "Compliance".to_string()),
            ],
            send_member: true,
        },
        Case {
            label: "a key and a value at their published ceilings",
            tags: vec![
                ("k".repeat(MAX_TAG_KEY), "v".repeat(MAX_TAG_VALUE)),
                ("Team".to_string(), "Compliance".to_string()),
            ],
            send_member: true,
        },
        Case {
            label: "a tag set at TagsMap's entry ceiling",
            tags: (0..MAX_TAG_ENTRIES)
                .map(|index| (format!("key.{index}"), format!("value.{index}")))
                .collect(),
            send_member: true,
        },
    ]
}

/// The id and the ARN a create answered with.
fn created_identity(answer: &Answer, case: &str) -> (String, String) {
    let summary = &answer.body["complianceInquirySummary"];
    let id = summary["id"].as_str().unwrap_or_default().to_string();
    let arn = summary["arn"].as_str().unwrap_or_default().to_string();
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "case {case:?}: the create must report an id and an ARN, got {:?}",
        answer.body
    );
    (id, arn)
}

/// Assert every reporter of the projection answers the map `want` for the inquiry
/// `inquiry_id`/`arn` — the create (when this case drove one), the metadata read, the
/// export and the tag listing.
async fn assert_every_reporter_agrees(
    router: &MultiServiceRouter,
    case: &str,
    inquiry_id: &str,
    arn: &str,
    created: Option<&Answer>,
    want: BTreeMap<String, String>,
) {
    if let Some(created) = created {
        assert_eq!(
            created.reported_tags(),
            Some(want.clone()),
            "case {case:?}: CreateComplianceInquiry must report the projection, got {:?}",
            created.body
        );
    }

    let read = get_metadata(router, inquiry_id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the metadata read must be served, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.reported_tags(),
        Some(want.clone()),
        "case {case:?}: GetComplianceInquiryMetadata must report the projection, got {:?}",
        read.body
    );

    let exported = export(router, inquiry_id).await;
    assert_eq!(
        exported.status, 200,
        "case {case:?}: the export must be served, got {} with {:?}",
        exported.status, exported.body
    );
    assert_eq!(
        exported.reported_tags(),
        Some(want.clone()),
        "case {case:?}: ExportComplianceInquiry must report the projection, got {:?}",
        exported.body
    );

    // The LISTING of the same record answers the same one projection, so every reporter
    // of this tag set is covered by the rule.
    let listed = list_tags(router, arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: the tag listing must be served, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.reported_tags(),
        Some(want),
        "case {case:?}: the tag listing reports the map form even when empty, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn every_inquiry_operation_reports_the_one_tags_projection() {
    let router = emulator();

    for case in cases() {
        let created = create(&router, "Annual audit", &case.tags, case.send_member).await;
        assert_eq!(
            created.status, 202,
            "case {:?}: the create must be accepted, got {} with {:?}",
            case.label, created.status, created.body
        );
        let (inquiry_id, arn) = created_identity(&created, case.label);
        let want = expected(&case.tags);

        assert_every_reporter_agrees(&router, case.label, &inquiry_id, &arn, Some(&created), want)
            .await;
    }
}

#[tokio::test]
async fn removing_every_tag_makes_every_operation_report_the_empty_map() {
    let router = emulator();

    for case in cases() {
        if case.tags.is_empty() {
            // Nothing to remove: the untagged row is covered by the table above.
            continue;
        }
        let label = format!("{} then untagged", case.label);
        let created = create(&router, "Annual audit", &case.tags, case.send_member).await;
        assert_eq!(
            created.status, 202,
            "case {label:?}: the create must be accepted, got {} with {:?}",
            created.status, created.body
        );
        let (inquiry_id, arn) = created_identity(&created, &label);

        let keys: Vec<String> = case.tags.iter().map(|(key, _)| key.clone()).collect();
        let removed = untag(&router, &arn, &keys).await;
        assert_eq!(
            removed.status, 204,
            "case {label:?}: removing every tag must be accepted, got {} with {:?}",
            removed.status, removed.body
        );

        // The record now holds no tags, so the same one projection applies: every
        // reporter answers the present, empty map.
        assert_every_reporter_agrees(&router, &label, &inquiry_id, &arn, None, BTreeMap::new())
            .await;
    }
}
