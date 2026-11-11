// SPDX-License-Identifier: CC0-1.0

//! ONE rule, over every refusal the harvest-job surface makes and over the tag refusal
//! the creates share: a `ValidationException` this service raises is worded in the
//! crate's single form — `Validation error encountered at request.<member>: <the rule the
//! member failed>` — where `<member>` is the member the request carried the refused value
//! in, and the detail after it states the RULE, so it holds none of the caller's own
//! values.
//!
//! The rule is asserted over a SET of inputs per refusal class: the schedule window is
//! refused at several distances and at several instants, an unharvestable manifest is
//! named in each manifest family the model declares and mixed with published ones, a job
//! whose work the clock has ENDED is refused for cancellation over several closed windows
//! (while a job this operation has already cancelled is answered the same success again,
//! as `@idempotent` requires), a pagination token the
//! service never issued is sent in several forms, and the tag ceiling is crossed on two
//! operations whose wire member name differs. The accepted rows are asserted too: a
//! refusal rule is only a rule if the requests it does NOT cover are still served.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::channel_lifecycle::error_type;
use crate::rejection_rule_and_negative_paths::{
    epoch_seconds_from_now, harvest_body, Answer, Emulator,
};
// The crate's ONE budget for waiting on the clock to END a harvest job, beside the waiter
// that owns it: this suite drives the data-plane-capable client, whose type that waiter
// cannot take, so it shares the budget rather than inventing a second one.
use crate::shared_entity_tag_precondition::{END_POLL_INTERVAL, END_POLL_LIMIT};

/// The fixed envelope every refused request is reported in
/// (`state::validation_message`).
const ENVELOPE: &str = "Validation error encountered at request";

/// The hierarchy every harvest-job row in this suite hangs off. The endpoint's container
/// type is `CMAF`, the one that serves all three harvestable manifest families (`TS`
/// serves no DASH manifest, per the model's `TS_CONTAINER_TYPE_WITH_DASH_MANIFEST`).
const GROUP: &str = "harvest-form-grp";
const CHANNEL: &str = "harvest-form-chan";
const ENDPOINT: &str = "harvest-form-ep";

/// The manifests the endpoint PUBLISHES, one per harvestable family — the set a harvest
/// job's requested manifests are resolved against.
const PUBLISHED_HLS: &str = "pub-hls";
const PUBLISHED_LOW_LATENCY: &str = "pub-llhls";
const PUBLISHED_DASH: &str = "pub-dash";

/// The `Message` member of an error envelope.
fn message(answer: &Answer) -> &str {
    answer
        .json
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// The `ValidationExceptionType` discriminator of an error envelope.
fn validation_type(answer: &Answer) -> &str {
    answer
        .json
        .get("ValidationExceptionType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Assert the rule on one refusal: 400, the modeled code, the expected discriminator (a
/// class the model's `ValidationExceptionType` enum does not name carries none), the
/// crate's envelope at `member_path`, a detail after it, and none of `sent` in the text.
fn assert_refusal_form(
    case: &str,
    answer: &Answer,
    member_path: &str,
    expected_type: Option<&str>,
    sent: &[&str],
) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        error_type(&answer.json),
        "ValidationException",
        "case {case:?}: expected a ValidationException envelope, got {}",
        answer.body
    );
    assert_eq!(
        validation_type(answer),
        expected_type.unwrap_or(""),
        "case {case:?}: expected ValidationExceptionType {:?}, got {}",
        expected_type,
        answer.body
    );
    let prefix = format!("{ENVELOPE}.{member_path}: ");
    assert!(
        message(answer).starts_with(&prefix),
        "case {case:?}: expected a message starting {prefix:?}, got {:?}",
        message(answer)
    );
    assert!(
        message(answer).len() > prefix.len(),
        "case {case:?}: the message must state the rule after the member path, got {:?}",
        message(answer)
    );
    for value in sent {
        assert!(
            !message(answer).contains(value),
            "case {case:?}: the message must not echo the request's {value:?}, got {:?}",
            message(answer)
        );
    }
}

/// Build the group, channel and origin endpoint every harvest-job row addresses.
async fn hierarchy() -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        created.status, 200,
        "the fixture channel group must be created, got {} with body {}",
        created.status, created.body
    );
    let created = emulator
        .create_channel(GROUP, CHANNEL, json!({}))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture channel must be created, got {} with body {}",
        created.status, created.body
    );
    let created = emulator
        .create_endpoint(
            GROUP,
            CHANNEL,
            ENDPOINT,
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": [{"ManifestName": PUBLISHED_HLS}],
                "LowLatencyHlsManifests": [{"ManifestName": PUBLISHED_LOW_LATENCY}],
                "DashManifests": [{"ManifestName": PUBLISHED_DASH}],
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture origin endpoint must be created, got {} with body {}",
        created.status, created.body
    );
    emulator
}

/// `POST .../harvestJob` under the fixture endpoint.
async fn create_job(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!(
                "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/harvestJob"
            ),
            Some(body),
        )
        .await
}

/// The crate's shared `CreateHarvestJob` body ([`harvest_body`]) with its manifest set
/// REPLACED by `harvested`: this suite drives families the shared builder's HLS-only
/// argument cannot name, and takes its schedule and destination from the shared one so
/// there is no second body builder in the crate.
fn job_body(start: f64, end: f64, harvested: Value) -> Value {
    let mut body = harvest_body(start, end, &[]);
    body["HarvestedManifests"] = harvested;
    body
}

#[tokio::test]
async fn a_refused_harvest_window_is_reported_at_the_member_in_the_crates_form() {
    let emulator = hierarchy().await;
    let start = epoch_seconds_from_now(600.0);
    // (label, start, end, the discriminator the model names for that end of the rule).
    // The rows differ in HOW far the window is wrong and in WHERE on the clock it sits —
    // the rule generalises over the distance and the instant, not over one recorded pair.
    let cases: &[(&str, f64, f64, &str)] = &[
        (
            "the end is one second before the start",
            start,
            start - 1.0,
            "END_TIME_EARLIER_THAN_START_TIME",
        ),
        (
            "the end is an hour before the start",
            start,
            start - 3600.0,
            "END_TIME_EARLIER_THAN_START_TIME",
        ),
        (
            "the end is a day before the start",
            start,
            start - 86_400.0,
            "END_TIME_EARLIER_THAN_START_TIME",
        ),
        (
            "a window of zero length in the future",
            start,
            start,
            "INVALID_HARVEST_JOB_DURATION",
        ),
        (
            "a window of zero length in the past",
            1_600_000_000.0,
            1_600_000_000.0,
            "INVALID_HARVEST_JOB_DURATION",
        ),
    ];

    for (label, from, to, expected_type) in cases {
        let mut body = job_body(
            *from,
            *to,
            json!({"HlsManifests": [{"ManifestName": PUBLISHED_HLS}]}),
        );
        body["HarvestJobName"] = json!("window-row");
        let answer = create_job(&emulator, body).await;
        // Neither instant, nor the names the request addressed, may appear in the text.
        let start_seconds = format!("{}", *from as i64);
        let end_seconds = format!("{}", *to as i64);
        assert_refusal_form(
            label,
            &answer,
            // A rule about the RELATIONSHIP between the two instants
            // `ScheduleConfiguration` carries is stated at the member that holds both.
            "scheduleConfiguration",
            Some(expected_type),
            &[
                start_seconds.as_str(),
                end_seconds.as_str(),
                "window-row",
                GROUP,
                CHANNEL,
                ENDPOINT,
            ],
        );
    }

    // Nothing was stored: a refused create leaves the endpoint with no harvest job.
    let listed = emulator
        .call("GET", &format!("/channelGroup/{GROUP}/harvestJob"), None)
        .await;
    assert_eq!(
        listed.status, 200,
        "ListHarvestJobs must answer, got {} with body {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.json["Items"].as_array().map(Vec::len),
        Some(0),
        "a refused create must store nothing, got {}",
        listed.body
    );
}

#[tokio::test]
async fn an_unharvestable_manifest_is_reported_at_the_manifest_member() {
    let emulator = hierarchy().await;
    let start = epoch_seconds_from_now(600.0);
    // (label, the HarvestedManifests the request carries, the names it sent). One row per
    // harvestable family the model declares on `HarvestedManifests` (`HlsManifests`,
    // `LowLatencyHlsManifests`, `DashManifests`), plus a mixture with a published
    // manifest and a row whose only difference from a published name is its case.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "an HLS manifest the endpoint does not publish",
            json!({"HlsManifests": [{"ManifestName": "ghost-hls"}]}),
            vec!["ghost-hls"],
        ),
        (
            "a low-latency HLS manifest the endpoint does not publish",
            json!({"LowLatencyHlsManifests": [{"ManifestName": "ghost-llhls"}]}),
            vec!["ghost-llhls"],
        ),
        (
            "a DASH manifest the endpoint does not publish",
            json!({"DashManifests": [{"ManifestName": "ghost-dash"}]}),
            vec!["ghost-dash"],
        ),
        (
            "a published manifest together with one the endpoint does not publish",
            json!({
                "HlsManifests": [{"ManifestName": PUBLISHED_HLS}],
                "DashManifests": [{"ManifestName": "ghost-dash-2"}],
            }),
            vec!["ghost-dash-2"],
        ),
        (
            "a name differing from a published one only in case",
            json!({"HlsManifests": [{"ManifestName": "PUB-HLS"}]}),
            vec!["PUB-HLS"],
        ),
        (
            "two manifests in one family, the second unpublished",
            json!({
                "HlsManifests": [
                    {"ManifestName": PUBLISHED_HLS},
                    {"ManifestName": "ghost-hls-3"},
                ],
            }),
            vec!["ghost-hls-3"],
        ),
    ];

    for (label, harvested, sent) in cases {
        let mut body = job_body(start, start + 600.0, harvested);
        body["HarvestJobName"] = json!("manifest-row");
        let answer = create_job(&emulator, body).await;
        let mut forbidden = sent.clone();
        forbidden.extend_from_slice(&["manifest-row", GROUP, CHANNEL, ENDPOINT]);
        assert_refusal_form(
            label,
            &answer,
            "harvestedManifests",
            Some("HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT"),
            &forbidden,
        );
    }

    // The rule's other side: a request naming only published manifests is SERVED, in
    // every family and in combination, and what it created reads back.
    let accepted: Vec<(&str, Value, &str)> = vec![
        (
            "the published HLS manifest alone",
            json!({"HlsManifests": [{"ManifestName": PUBLISHED_HLS}]}),
            "accepted-hls",
        ),
        (
            "the published low-latency HLS manifest alone",
            json!({"LowLatencyHlsManifests": [{"ManifestName": PUBLISHED_LOW_LATENCY}]}),
            "accepted-llhls",
        ),
        (
            "the published DASH manifest alone",
            json!({"DashManifests": [{"ManifestName": PUBLISHED_DASH}]}),
            "accepted-dash",
        ),
        (
            "all three published families at once",
            json!({
                "HlsManifests": [{"ManifestName": PUBLISHED_HLS}],
                "LowLatencyHlsManifests": [{"ManifestName": PUBLISHED_LOW_LATENCY}],
                "DashManifests": [{"ManifestName": PUBLISHED_DASH}],
            }),
            "accepted-all",
        ),
    ];

    for (label, harvested, name) in accepted {
        let mut body = job_body(start, start + 600.0, harvested);
        body["HarvestJobName"] = json!(name);
        let answer = create_job(&emulator, body).await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: expected HTTP 200, got {} with body {}",
            answer.status, answer.body
        );
        let read = emulator
            .call(
                "GET",
                &format!(
                    "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}\
                     /harvestJob/{name}"
                ),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetHarvestJob must find the job, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("Arn"),
            answer.string("Arn"),
            "case {label:?}: the stored job must be the one the create reported"
        );
    }
}

#[tokio::test]
async fn cancelling_a_cancelled_job_again_is_the_same_request_and_the_same_answer() {
    let emulator = hierarchy().await;
    let start = epoch_seconds_from_now(600.0);
    // The job names differ in shape (dashes, underscores, digits, length), which is what
    // an answer keyed on the caller's value would differ on.
    for name in ["cancel-a", "Cancel_B-2", "c3", "a-very-long-harvest-job-name-0001"] {
        let mut body = job_body(
            start,
            start + 600.0,
            json!({"HlsManifests": [{"ManifestName": PUBLISHED_HLS}]}),
        );
        body["HarvestJobName"] = json!(name);
        let created = create_job(&emulator, body).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: the job must be created, got {} with body {}",
            created.status, created.body
        );
        let path = format!(
            "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}\
             /harvestJob/{name}"
        );
        let cancelled = emulator.call("PUT", &path, None).await;
        assert_eq!(
            cancelled.status, 200,
            "case {name:?}: the first cancellation must succeed, got {} with body {}",
            cancelled.status, cancelled.body
        );
        let read = emulator.call("GET", &path, None).await;
        assert_eq!(
            read.string("Status"),
            "CANCELLED",
            "case {name:?}: the cancelled job must report CANCELLED, got {}",
            read.body
        );
        // `@idempotent`: the repeat is the same request, so it is answered the same
        // success and commits nothing — the job's entity tag and modification instant
        // are the ones the first cancellation left.
        for repeat in 1..=2 {
            let again = emulator.call("PUT", &path, None).await;
            assert_eq!(
                again.status, 200,
                "case {name:?}: repeat {repeat} of the cancellation must be accepted, got \
                 {} with body {}",
                again.status, again.body
            );
            let read_again = emulator.call("GET", &path, None).await;
            for member in ["Status", "ETag", "ModifiedAt"] {
                assert_eq!(
                    read_again.json[member], read.json[member],
                    "case {name:?}: repeat {repeat} must not move {member}: expected {}, \
                     got {}",
                    read.json[member], read_again.json[member]
                );
            }
        }
    }
}

#[tokio::test]
async fn cancelling_a_job_whose_work_has_ended_is_reported_at_the_job_member() {
    let emulator = hierarchy().await;
    // Windows that had already CLOSED when the job was created, so the clock ends the job
    // of its own accord. The rows differ in how long ago the window closed and how long
    // it was — what a rule keyed on one recorded pair of instants would differ on.
    let cases: &[(&str, f64, f64)] = &[
        ("a window that closed an hour before the job", -7200.0, -3600.0),
        ("a one-second window that closed a day before the job", -86_401.0, -86_400.0),
        ("a window that closed a minute before the job", -3660.0, -60.0),
    ];
    for (index, (label, start_offset, end_offset)) in cases.iter().enumerate() {
        let name = format!("ended-{index}");
        let base = epoch_seconds_from_now(0.0);
        let mut body = job_body(
            base + start_offset,
            base + end_offset,
            json!({"HlsManifests": [{"ManifestName": PUBLISHED_HLS}]}),
        );
        body["HarvestJobName"] = json!(name);
        let created = create_job(&emulator, body).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the job must be created, got {} with body {}",
            created.status, created.body
        );
        let path = format!(
            "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}\
             /harvestJob/{name}"
        );
        // Nobody touches the job: the clock alone ends it, which is what makes the
        // refusal below a property of the status rather than of this request.
        let mut ended = String::new();
        for _ in 0..END_POLL_LIMIT {
            let read = emulator.call("GET", &path, None).await;
            ended = read.string("Status").to_string();
            if ended == "FAILED" {
                break;
            }
            tokio::time::sleep(END_POLL_INTERVAL).await;
        }
        assert_eq!(
            ended, "FAILED",
            "case {label:?}: the clock must end a job whose window had closed, got {ended:?}"
        );
        let refused = emulator.call("PUT", &path, None).await;
        assert_refusal_form(
            label,
            &refused,
            "harvestJobName",
            Some("HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION"),
            &[name.as_str(), GROUP, CHANNEL, ENDPOINT, "FAILED"],
        );
    }
}

#[tokio::test]
async fn a_pagination_token_listharvestjobs_never_issued_is_reported_at_next_token() {
    let emulator = hierarchy().await;
    // Tokens the service did not issue, differing in form: a word, hex, a digit run, and
    // something that looks like one of this service's composite keys.
    for token in ["not-a-token", "deadbeef", "0123456789", "chan-ep-job"] {
        let answer = emulator
            .call(
                "GET",
                &format!("/channelGroup/{GROUP}/harvestJob?nextToken={token}"),
                None,
            )
            .await;
        assert_refusal_form(
            token,
            &answer,
            "nextToken",
            Some("INVALID_PAGINATION_TOKEN"),
            &[token, GROUP],
        );
    }
}

#[tokio::test]
async fn a_tag_map_past_the_ceiling_is_reported_at_the_member_the_request_named_it_in() {
    use service_com_amazonaws_mediapackagev2_mediapackagev2::state::MAX_TAGS_PER_RESOURCE;

    let emulator = hierarchy().await;
    let start = epoch_seconds_from_now(600.0);
    let mut tags: BTreeMap<String, String> = BTreeMap::new();
    for index in 0..=MAX_TAGS_PER_RESOURCE {
        tags.insert(format!("key-{index}"), format!("value-{index}"));
    }
    let tags = serde_json::to_value(&tags).expect("a JSON-representable tag map");

    // The member path follows the WIRE name of the member the tags arrived in, which
    // differs per operation: `Tags` on `CreateHarvestJob` (no `@jsonName`), `tags` on
    // `CreateChannelGroup` (`@jsonName("tags")`).
    let mut body = job_body(
        start,
        start + 600.0,
        json!({"HlsManifests": [{"ManifestName": PUBLISHED_HLS}]}),
    );
    body["HarvestJobName"] = json!("tagged-job");
    body["Tags"] = tags.clone();
    let answer = create_job(&emulator, body).await;
    assert_refusal_form(
        "CreateHarvestJob one tag past the ceiling",
        &answer,
        "Tags",
        None,
        &["key-0", "value-0", "tagged-job", ENDPOINT],
    );

    let answer = emulator
        .create_group("tag-ceiling-grp", json!({"tags": tags}))
        .await;
    assert_refusal_form(
        "CreateChannelGroup one tag past the ceiling",
        &answer,
        "tags",
        None,
        &["key-0", "value-0", "tag-ceiling-grp"],
    );
}
