// SPDX-License-Identifier: CC0-1.0

//! When a compliance inquiry reports itself as last modified, over every call that writes
//! its stored record, end to end over HTTP.
//!
//! RULE under test: `InquiryDetail$updatedAt` is "Timestamp indicating when the resource
//! was last modified" (`smithy_query(query="doc",shape="InquiryDetail",member="updatedAt")`),
//! and the resource is everything its own projection reports — its TAG SET
//! (`GetComplianceInquiryMetadata` reports it beside the timestamp, and
//! `CreateComplianceInquiryRequest$tags` is "Tags to associate with the compliance inquiry
//! resource") as much as its status. So a call that CHANGES any of them reports the
//! resource as last modified at the instant of the change, a call that leaves them as they
//! were leaves the instant where it was, and a read never moves it. The instant therefore
//! never names a moment before a change a later read of the resource can observe, and
//! never falls back behind one.
//!
//! Every case goes through the router — routing, the front door, deserialization, the
//! handler, the persisted state and serialization — and reads the instant back through
//! `GetComplianceInquiryMetadata`, so an answer that is not computed from the persisted
//! record cannot pass.
//!
//! The rows differ along what the rule generalises over: WHICH write is made
//! (`TagResource`, `UntagResource`), how many keys it names, whether each named key is one
//! the resource already carries, whether the value written is the one it already holds (an
//! empty value included, which `TagValue` admits), whether a removal names keys the
//! resource does not carry, and whether the call is accepted, repeated or refused. The
//! clock-owed transition is covered too: a write that arrives after the due instant but
//! before anything materialised it must leave the instant at the write rather than at the
//! transition it applies on the way.
//!
//! `PutComplianceInquiryFeedback` rides along as the CONTRAST row: a feedback record is
//! held against a response revision and no projection of the inquiry reports it, so it is
//! not one of the resource's own attributes and recording one leaves the instant alone —
//! which `compliance_inquiry_feedback.rs` pins for the whole projection.
//!
//! The tag SETS those writes persist are `resource_tag_writes.rs`' subject; this suite
//! asserts them only as the evidence that the record really changed.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router bootstrap, the request sender and the credential scope are the crate's test
// harness': one emulator over one shared engine, signed for one region.
use crate::harness::{call, emulator, Answer, REGION};

/// Long enough for an inquiry's processing span to be over, so a read materialises the
/// transition the clock owed it. The handlers store the due instant rather than sleeping,
/// so only the test waits.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: these suites drive the router
/// directly, with no timer driver to advance.
const PAST_THE_PROCESSING_SPAN: Duration = Duration::from_millis(900);

/// `InquiryStatus`'s value for an inquiry whose processing is over
/// (`smithy_query(query="shape",shape="InquiryStatus")`). An inquiry submitted under
/// `AI_ONLY` reaches it once the clock-owed transition is applied.
const TERMINAL_STATUS: &str = "COMPLETED";

/// How many tags one resource holds at most, from `TagsMap`'s modelled `@length(max: 50)`
/// (`smithy_query(query="shape",shape="TagsMap")`). Spelled out here rather than imported
/// from the crate, so a wrong constant in `src/state.rs` cannot make this suite pass.
const TAG_CEILING: usize = 50;

/// The reserved tag-key prefix: keys under it name tags AWS applies itself, so a request
/// naming one is refused and persists nothing.
const RESERVED_PREFIX: &str = "aws:";

/// The slack a bracket assertion allows around a wall-clock reading. The emulator and this
/// test read the SAME clock, so the only gap is its resolution — not a margin wide enough
/// for another instant in these cases to pass as the write's.
const CLOCK_SLACK: f64 = 0.005;

/// One rating `FeedbackRating` admits (`smithy_query(query="shape",shape="FeedbackRating")`).
const RATING: &str = "THUMBS_UP";

/// One write a caller makes against an inquiry, as the request that performs it.
#[derive(Debug, Clone)]
enum Write {
    /// `TagResource` with these pairs.
    Tag(Vec<(String, String)>),
    /// `UntagResource` naming these keys.
    Untag(Vec<String>),
    /// `PutComplianceInquiryFeedback` with this rating, on the inquiry's response as a
    /// whole (the request names no query, and no `clientToken`, so a repeat is not a
    /// replay).
    Feedback(&'static str),
}

impl Write {
    /// The status this write answers when it is accepted: `TagResourceResponse` and
    /// `UntagResourceResponse` model no members (HTTP 204), the feedback call answers its
    /// `submittedAt`.
    fn accepted_status(&self) -> u16 {
        match self {
            Write::Tag(_) | Write::Untag(_) => 204,
            Write::Feedback(_) => 200,
        }
    }

    /// Send this write against the inquiry `inquiry_id`, whose ARN is `arn`.
    async fn send(&self, router: &MultiServiceRouter, inquiry_id: &str, arn: &str) -> Answer {
        match self {
            Write::Tag(tags) => {
                let body = json!({ "tags": tags_object(tags) });
                call(
                    router,
                    "POST",
                    &format!("/tags/{}", label(arn)),
                    Bytes::from(body.to_string()),
                )
                .await
            }
            Write::Untag(keys) => {
                let query: Vec<String> = keys
                    .iter()
                    .map(|key| format!("tagKeys={}", query_value(key)))
                    .collect();
                call(
                    router,
                    "DELETE",
                    &format!("/tags/{}?{}", label(arn), query.join("&")),
                    Bytes::new(),
                )
                .await
            }
            Write::Feedback(rating) => {
                let body = json!({ "complianceInquiryId": inquiry_id, "rating": rating });
                call(
                    router,
                    "PUT",
                    "/v1/compliance-inquiry/putFeedback",
                    Bytes::from(body.to_string()),
                )
                .await
            }
        }
    }
}

/// One accepted-write row: the resource it is made on, what it does to that resource's
/// record, and what the SAME request answers the second time.
struct Case {
    label: &'static str,
    /// The tag set the inquiry is created carrying.
    initial: Vec<(String, String)>,
    write: Write,
    /// Whether the write CHANGES the stored record — which is what decides whether the
    /// last-modified instant moves.
    changes: bool,
    /// The tag set the resource carries afterwards: the evidence the record changed (or
    /// did not).
    expected: Vec<(String, String)>,
    /// The status the identical request answers when it is sent again.
    repeat_status: u16,
}

/// One refused-write row.
struct Refusal {
    label: &'static str,
    write: Write,
    /// The inquiry id and ARN the refused request NAMES.
    target: (String, String),
    /// The inquiry whose instant must be where it was: the target when the request names
    /// one this account holds, and the held resource otherwise — a refusal modifies
    /// nothing anywhere.
    observed: String,
    status: u16,
}

/// `resourceArn` is an `@httpLabel` on `/tags/{resourceArn}`, and a non-greedy label is
/// ONE path segment, so the `/` inside an ARN is percent-encoded.
fn label(resource_arn: &str) -> String {
    resource_arn.replace('/', "%2F")
}

/// One query-string value, percent-encoded through the crate this service already depends
/// on: a tag key may carry `/`, `=`, `+`, `:` and whitespace, each of which means
/// something else in a query string.
fn query_value(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
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

/// The tags a body reports, in a comparable order. The member is present whatever the tag
/// set, so the empty map and (defensively) an absent member are both the empty set here.
fn reported_tags(body: &Value) -> BTreeMap<String, String> {
    body.get("tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// The expected tag set, in the same comparable order.
fn expected_tags(tags: &[(String, String)]) -> BTreeMap<String, String> {
    tags.iter().cloned().collect()
}

/// `GET /v1/compliance-inquiry/getMetadata` — the reader of the record under test.
async fn metadata(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    let answer = call(
        router,
        "GET",
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"),
        Bytes::new(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the metadata read must answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// Create one compliance inquiry carrying `tags`, and answer its id and the ARN the
/// service minted for it — the ARN the tag operations address it by.
async fn created_inquiry(
    router: &MultiServiceRouter,
    case: &str,
    tags: &[(String, String)],
) -> (String, String) {
    let mut body = json!({
        "name": "a compliance inquiry",
        "inquiryContent": { "query": "Which controls does this report cover?" },
        "supportMode": "AI_ONLY"
    });
    if !tags.is_empty() {
        body["tags"] = tags_object(tags);
    }
    let created = call(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: the create must answer HTTP 202, got {} with {:?}",
        created.status, created.body
    );
    let summary = &created.body["complianceInquirySummary"];
    let id = summary["id"].as_str().unwrap_or_default().to_string();
    let arn = summary["arn"].as_str().unwrap_or_default().to_string();
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "case {case:?}: the create must report an id and an ARN, got {:?}",
        created.body
    );
    (id, arn)
}

/// The instant `member` of the reported `complianceInquiryDetail` names, as epoch seconds.
///
/// Parsed through the wire form the model states (every timestamp of this service targets
/// `TimestampAttribute`, which is `@timestampFormat("date-time")`, so it travels as an
/// RFC 3339 string) rather than compared as text: what is under test is the INSTANT.
fn reported_instant(case: &str, what: &str, answer: &Answer, member: &str) -> f64 {
    let value = &answer.body["complianceInquiryDetail"][member];
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {what} must be reported as an RFC 3339 string, got {value:?}")
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {what} must parse as a date-time ({error}), got {text:?}")
    });
    epoch_seconds(&parsed)
}

/// One instant as epoch seconds, sub-second part included.
fn epoch_seconds(stamp: &DateTime) -> f64 {
    stamp.secs() as f64 + f64::from(stamp.subsec_nanos()) / 1_000_000_000.0
}

/// This test's own reading of the same wall clock the emulator stamps a write with.
fn wall_clock() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is past the epoch")
        .as_secs_f64()
}

/// Assert `instant` names a moment inside the window the write ran in, so the field
/// reports WHEN the record changed rather than any other instant the record holds.
fn assert_within(case: &str, what: &str, instant: f64, from: f64, to: f64) {
    assert!(
        instant >= from - CLOCK_SLACK && instant <= to + CLOCK_SLACK,
        "case {case:?}: {what} must name the instant of the write, between {from} and \
         {to}, got {instant}"
    );
}

/// Assert two readings of the field name the very same instant: the two are parsed from
/// the one wire form, so a record that did not change renders bit-identically.
fn assert_unmoved(case: &str, what: &str, instant: f64, expected: f64) {
    assert!(
        (instant - expected).abs() < f64::EPSILON,
        "case {case:?}: {what} must still name {expected}, got {instant}"
    );
}

/// The accepted writes, one row per thing the rule generalises over.
fn accepted_writes() -> Vec<Case> {
    vec![
        Case {
            label: "a tag write adding a key the resource does not carry",
            initial: pairs(&[("team", "legal")]),
            write: Write::Tag(pairs(&[("env", "prod")])),
            changes: true,
            expected: pairs(&[("env", "prod"), ("team", "legal")]),
            repeat_status: 204,
        },
        Case {
            label: "a tag write changing the value of a key it carries",
            initial: pairs(&[("team", "legal")]),
            write: Write::Tag(pairs(&[("team", "finance")])),
            changes: true,
            expected: pairs(&[("team", "finance")]),
            repeat_status: 204,
        },
        Case {
            label: "a tag write of several keys at once onto an untagged resource",
            initial: Vec::new(),
            write: Write::Tag(pairs(&[("a", "1"), ("b", "2"), ("c", "3")])),
            changes: true,
            expected: pairs(&[("a", "1"), ("b", "2"), ("c", "3")]),
            repeat_status: 204,
        },
        Case {
            // `TagValue` is `@length(min: 0, max: 256)`, so the empty value is a value the
            // model admits — and writing it over a non-empty one changes the record.
            label: "a tag write emptying the value of a key it carries",
            initial: pairs(&[("team", "legal")]),
            write: Write::Tag(pairs(&[("team", "")])),
            changes: true,
            expected: pairs(&[("team", "")]),
            repeat_status: 204,
        },
        Case {
            label: "a tag write of the value the resource already carries",
            initial: pairs(&[("team", "legal")]),
            write: Write::Tag(pairs(&[("team", "legal")])),
            changes: false,
            expected: pairs(&[("team", "legal")]),
            repeat_status: 204,
        },
        Case {
            label: "a removal of the one key the resource carries",
            initial: pairs(&[("team", "legal")]),
            write: Write::Untag(vec!["team".to_string()]),
            changes: true,
            expected: Vec::new(),
            repeat_status: 204,
        },
        Case {
            label: "a removal of one of several keys",
            initial: pairs(&[("team", "legal"), ("env", "prod"), ("owner", "ada")]),
            write: Write::Untag(vec!["env".to_string()]),
            changes: true,
            expected: pairs(&[("owner", "ada"), ("team", "legal")]),
            repeat_status: 204,
        },
        Case {
            label: "a removal naming every key the resource carries",
            initial: pairs(&[("team", "legal"), ("env", "prod")]),
            write: Write::Untag(vec!["team".to_string(), "env".to_string()]),
            changes: true,
            expected: Vec::new(),
            repeat_status: 204,
        },
        Case {
            label: "a removal naming only a key the resource does not carry",
            initial: pairs(&[("team", "legal")]),
            write: Write::Untag(vec!["absent".to_string()]),
            changes: false,
            expected: pairs(&[("team", "legal")]),
            repeat_status: 204,
        },
        Case {
            label: "a removal naming a carried key beside one the resource does not carry",
            initial: pairs(&[("team", "legal")]),
            write: Write::Untag(vec!["team".to_string(), "absent".to_string()]),
            changes: true,
            expected: Vec::new(),
            repeat_status: 204,
        },
        Case {
            // The feedback record is held against a RESPONSE REVISION and no projection of
            // the inquiry reports it, so it is not one of the resource's own attributes and
            // recording one leaves the resource's last-modified instant alone. The
            // identical request again is refused: the response version it names already
            // carries feedback.
            label: "a feedback record on the inquiry's response",
            initial: pairs(&[("team", "legal")]),
            write: Write::Feedback(RATING),
            changes: false,
            expected: pairs(&[("team", "legal")]),
            repeat_status: 409,
        },
    ]
}

#[tokio::test]
async fn a_write_moves_the_last_modified_instant_exactly_when_it_changes_the_record() {
    let router = emulator();
    let cases = accepted_writes();

    // Each row gets its own resource, created up front, and the processing span is waited
    // out ONCE: every row's baseline is then the clock-owed transition's instant rather
    // than a transition still pending.
    let mut resources = Vec::new();
    for case in &cases {
        resources.push(created_inquiry(&router, case.label, &case.initial).await);
    }
    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    for (case, (id, arn)) in cases.iter().zip(&resources) {
        let label = case.label;
        let settled = metadata(&router, id).await;
        let baseline = reported_instant(label, "the instant before the write", &settled, "updatedAt");
        let created = reported_instant(label, "createdAt", &settled, "createdAt");

        let from = wall_clock();
        let answer = case.write.send(&router, id, arn).await;
        let to = wall_clock();
        assert_eq!(
            answer.status,
            case.write.accepted_status(),
            "case {label:?}: the write must answer HTTP {}, got {} with {:?}",
            case.write.accepted_status(),
            answer.status,
            answer.body
        );

        let read = metadata(&router, id).await;
        let updated = reported_instant(label, "updatedAt after the write", &read, "updatedAt");
        assert_eq!(
            reported_tags(&read.body),
            expected_tags(&case.expected),
            "case {label:?}: the write must leave the tag set the rule says it does, got {:?}",
            read.body
        );
        if case.changes {
            assert_within(label, "updatedAt after the write", updated, from, to);
            assert!(
                updated > baseline,
                "case {label:?}: the write modified the resource, so updatedAt must have \
                 moved off the instant before it ({baseline}), got {updated}"
            );
        } else {
            assert_unmoved(
                label,
                "updatedAt after a write that changed nothing",
                updated,
                baseline,
            );
        }
        // Nothing in any of these writes creates the resource again.
        assert_unmoved(
            label,
            "createdAt",
            reported_instant(label, "createdAt after the write", &read, "createdAt"),
            created,
        );

        // The identical request again: an accepted one asks for the state the resource is
        // already in and a refused one writes nothing, so either way the record did not
        // change and the instant stays where the first call left it.
        let repeat = case.write.send(&router, id, arn).await;
        assert_eq!(
            repeat.status, case.repeat_status,
            "case {label:?}: the same request again must answer HTTP {}, got {} with {:?}",
            case.repeat_status, repeat.status, repeat.body
        );
        let after_repeat = metadata(&router, id).await;
        assert_unmoved(
            label,
            "updatedAt after the same request again",
            reported_instant(label, "updatedAt after the repeat", &after_repeat, "updatedAt"),
            updated,
        );
    }
}

#[tokio::test]
async fn a_write_past_the_due_instant_reports_itself_rather_than_the_transition_it_applies() {
    let label = "a tag write arriving after the due instant, with nothing having read the \
                 inquiry";
    let router = emulator();
    let (id, arn) = created_inquiry(&router, label, &pairs(&[("team", "legal")])).await;

    // NOTHING reads the inquiry while its span runs out, so the transition the clock owes
    // it is still unapplied when the write arrives — and the write has to apply it without
    // letting the earlier due instant stand as the record's last modification.
    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    let from = wall_clock();
    let answer = Write::Tag(pairs(&[("env", "prod")]))
        .send(&router, &id, &arn)
        .await;
    let to = wall_clock();
    assert_eq!(
        answer.status, 204,
        "case {label:?}: the tag write must answer HTTP 204, got {} with {:?}",
        answer.status, answer.body
    );

    let read = metadata(&router, &id).await;
    let updated = reported_instant(label, "updatedAt", &read, "updatedAt");
    assert_within(label, "updatedAt", updated, from, to);

    // The transition the clock owed is not lost on the way: the status it moves the
    // inquiry to is reported too.
    assert_eq!(
        read.body["complianceInquiryDetail"]["status"],
        json!(TERMINAL_STATUS),
        "case {label:?}: the clock-owed transition must still be applied, got {:?}",
        read.body
    );
    assert!(
        updated > reported_instant(label, "createdAt", &read, "createdAt"),
        "case {label:?}: the resource was modified after it was created, so updatedAt must \
         be past createdAt, got {updated}"
    );
}

#[tokio::test]
async fn a_refused_write_leaves_the_last_modified_instant_where_it_was() {
    let router = emulator();
    let held = created_inquiry(&router, "the resource under test", &pairs(&[("team", "legal")])).await;
    // A second resource holding as many tags as the model admits, so the write refused for
    // the per-resource limit is refused on a resource that really carries that many.
    let full_set: Vec<(String, String)> = (0..TAG_CEILING)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect();
    let full = created_inquiry(&router, "a resource at the tag ceiling", &full_set).await;
    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    // An ARN of this service, in this region, under this account, whose inquiry id this
    // account holds no resource for.
    let absent_arn = format!(
        "arn:aws:artifact:{REGION}:{}:compliance-inquiry/compliance-inquiry-0000000000000000",
        engine::emulator_account_id()
    );
    let absent_id = "compliance-inquiry-0000000000000000".to_string();
    let reserved_key = format!("{RESERVED_PREFIX}cloudformation:stack-name");

    let refusals = vec![
        Refusal {
            label: "a tag write naming a key in the reserved namespace",
            write: Write::Tag(vec![(reserved_key.clone(), "mine".to_string())]),
            target: held.clone(),
            observed: held.0.clone(),
            status: 400,
        },
        Refusal {
            label: "a removal naming a key in the reserved namespace",
            write: Write::Untag(vec![reserved_key.clone()]),
            target: held.clone(),
            observed: held.0.clone(),
            status: 400,
        },
        Refusal {
            label: "a tag write that would take the resource past its tag limit",
            write: Write::Tag(pairs(&[("one too many", "x")])),
            target: full.clone(),
            observed: full.0.clone(),
            status: 400,
        },
        Refusal {
            label: "a tag write naming an ARN this account holds no resource for",
            write: Write::Tag(pairs(&[("team", "finance")])),
            target: (absent_id.clone(), absent_arn.clone()),
            observed: held.0.clone(),
            status: 401,
        },
        Refusal {
            label: "a removal naming an ARN this account holds no resource for",
            write: Write::Untag(vec!["team".to_string()]),
            target: (absent_id.clone(), absent_arn.clone()),
            observed: held.0.clone(),
            status: 401,
        },
        Refusal {
            label: "a feedback record on an id this account holds no inquiry for",
            write: Write::Feedback(RATING),
            target: (absent_id, absent_arn),
            observed: held.0.clone(),
            status: 401,
        },
    ];

    for refusal in refusals {
        let label = refusal.label;
        let observed_id = &refusal.observed;
        let before = metadata(&router, observed_id).await;
        let baseline = reported_instant(label, "the instant before the refusal", &before, "updatedAt");
        let tags_before = reported_tags(&before.body);

        let (target_id, target_arn) = &refusal.target;
        let answer = refusal.write.send(&router, target_id, target_arn).await;
        assert_eq!(
            answer.status, refusal.status,
            "case {label:?}: the write must be refused HTTP {}, got {} with {:?}",
            refusal.status, answer.status, answer.body
        );

        let after = metadata(&router, observed_id).await;
        assert_unmoved(
            label,
            "updatedAt after the refusal",
            reported_instant(label, "updatedAt after the refusal", &after, "updatedAt"),
            baseline,
        );
        assert_eq!(
            reported_tags(&after.body),
            tags_before,
            "case {label:?}: a refused write persists nothing, so the tag set must be \
             unchanged, got {:?}",
            after.body
        );
    }
}

#[tokio::test]
async fn a_read_never_moves_the_last_modified_instant() {
    let router = emulator();
    let (id, arn) = created_inquiry(&router, "a resource nothing writes", &pairs(&[("team", "legal")])).await;
    std::thread::sleep(PAST_THE_PROCESSING_SPAN);

    let settled = metadata(&router, &id).await;
    let baseline = reported_instant(
        "the settled inquiry",
        "updatedAt once the transition is applied",
        &settled,
        "updatedAt",
    );

    // Every implemented reporting operation that reaches this record. None of them may
    // change what a later caller sees.
    let reads = [
        ("ListTagsForResource", format!("/tags/{}", label(&arn))),
        (
            "GetComplianceInquiryMetadata",
            format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={id}"),
        ),
        (
            "ListComplianceInquiries",
            "/v1/compliance-inquiry/list".to_string(),
        ),
        (
            "ListComplianceInquiryQueries",
            format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={id}"),
        ),
    ];

    for (label, uri) in reads {
        let answer = call(&router, "GET", &uri, Bytes::new()).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: the read must answer HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let read = metadata(&router, &id).await;
        assert_unmoved(
            label,
            "updatedAt after the read",
            reported_instant(label, "updatedAt after the read", &read, "updatedAt"),
            baseline,
        );
    }
}
