// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE mediapackagev2 refuses with, per refusal class, and the member path it
//! states that sentence at.
//!
//! An AWS error's `Message` is part of its contract for the class: the fixed text is the
//! requirement, and only its variable parts come from the request or from state. Two rules
//! are asserted here, each over a TABLE that spans the class rather than over one request:
//!
//! 1. A refusal about the RELATIONSHIP between several members of the request is stated at
//!    the member that CONTAINS them — `request.scheduleConfiguration` for a rule relating
//!    the two instants that structure carries, and the bare `request` for a rule relating
//!    two of the request's own members (`ContainerType` against a manifest list). Naming
//!    one of the related members instead would point a caller that parses the
//!    `Validation error encountered at <member>` prefix at a value that is acceptable on
//!    its own.
//! 2. The sentence after that prefix is the FIXED text of the class, with only the values
//!    the class names interpolated: the container type and manifest family for the
//!    container/manifest rule, and nothing at all for the schedule, policy-document and
//!    tagging classes. None of them echoes an instant, a document or an ARN the caller
//!    sent.
//!
//! The expected sentences are written out here rather than imported from the crate, so
//! this suite pins the WORDING and not merely the code's agreement with itself.

use serde_json::{json, Value};

use crate::rejection_rule_and_negative_paths::{
    encoded, epoch_seconds_from_now, harvest_body, Answer, Emulator,
};

/// The Region every request in this suite is signed for, and the account id every emulated
/// ARN carries (`engine::emulator_account_id`) — the two halves of an ARN that names a
/// resource of this emulator.
const REGION: &str = "us-west-2";
const ACCOUNT: &str = "000000000000";

/// The hierarchy the refused requests address. Nothing in either rule depends on these
/// names, which is part of what the tables assert.
const GROUP: &str = "sentence-grp";
const CHANNEL: &str = "sentence-chan";
const ENDPOINT: &str = "sentence-ep";
const MANIFEST: &str = "index";

/// The fixed prefix every refused request is reported in.
const ENVELOPE: &str = "Validation error encountered at request";

/// The sentence a schedule whose end is not later than its start is refused with, stated at
/// the member that carries BOTH instants.
const END_BEFORE_START: &str = "Validation error encountered at request.scheduleConfiguration: \
    Harvest job end time must be later than the start time. Check your configured start and \
    end times and then try again";

/// The sentence a policy document that is not an IAM policy is refused with, on either
/// policy writer.
const INVALID_POLICY: &str = "Validation error encountered at request.policy: The policy you \
    entered isn't valid. The policy must be a valid IAM policy. Check your policy and then \
    try again";

/// The whole sentence the tagging front end refuses an ARN naming no existing resource
/// with: it says the resource was not found and nothing else.
const RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The `Message` member of an answer, or the `message` of an envelope that names it that
/// way (the tagging front end's own refusals, which are not modelled shapes).
fn message(answer: &Answer) -> &str {
    match answer.string("Message") {
        "" => answer.string("message"),
        named => named,
    }
}

/// Assert one answer is a `ValidationException` (400) whose message is exactly `expected`
/// and whose discriminator is `exception_type`, and that it echoes none of `sent`.
fn assert_sentence(
    case: &str,
    answer: &Answer,
    expected: &str,
    exception_type: &str,
    sent: &[&str],
) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.string("__type"),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {}",
        answer.body
    );
    assert_eq!(
        answer.string("ValidationExceptionType"),
        exception_type,
        "case {case:?}: expected ValidationExceptionType {exception_type}, got {}",
        answer.body
    );
    assert_eq!(
        message(answer),
        expected,
        "case {case:?}: expected the message {expected:?}, got {:?}",
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

/// The group, channel and CMAF origin endpoint the harvest-job and policy rows address.
async fn hierarchy() -> Emulator {
    let emulator = Emulator::new();
    let created = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(
        created.status, 200,
        "setup: CreateChannelGroup got {} with body {}",
        created.status, created.body
    );
    let created = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    assert_eq!(
        created.status, 200,
        "setup: CreateChannel got {} with body {}",
        created.status, created.body
    );
    let created = emulator
        .create_endpoint(
            GROUP,
            CHANNEL,
            ENDPOINT,
            json!({
                "ContainerType": "CMAF",
                "HlsManifests": [{"ManifestName": MANIFEST}],
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "setup: CreateOriginEndpoint got {} with body {}",
        created.status, created.body
    );
    emulator
}

#[tokio::test]
async fn a_window_whose_end_is_not_after_its_start_is_refused_at_the_schedule_member() {
    let emulator = hierarchy().await;
    // (label, start, end): the rows differ in HOW far the end is before the start and in
    // WHERE on the clock the window sits — the two things the rule generalises over — so a
    // sentence built from either instant would differ between them.
    let future = epoch_seconds_from_now(600.0);
    let cases: &[(&str, f64, f64)] = &[
        ("the end is one second before the start", future, future - 1.0),
        ("the end is an hour before the start", future, future - 3600.0),
        ("the end is a day before the start", future, future - 86_400.0),
        ("a window entirely in the past, reversed", 1_600_003_600.0, 1_600_000_000.0),
        ("a window a year out, reversed", future + 31_536_000.0, future),
    ];
    for (label, start, end) in cases {
        let mut body = harvest_body(*start, *end, &[MANIFEST]);
        body["HarvestJobName"] = json!("schedule-row");
        let answer = emulator
            .call(
                "POST",
                &format!(
                    "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}\
                     /harvestJob"
                ),
                Some(body),
            )
            .await;
        let start_seconds = format!("{}", *start as i64);
        let end_seconds = format!("{}", *end as i64);
        assert_sentence(
            label,
            &answer,
            END_BEFORE_START,
            "END_TIME_EARLIER_THAN_START_TIME",
            &[
                start_seconds.as_str(),
                end_seconds.as_str(),
                "schedule-row",
                GROUP,
                CHANNEL,
                ENDPOINT,
                // The leaf members the rule relates: the message names the structure that
                // holds them, never one of them.
                "endTime",
                "startTime",
            ],
        );
    }

    // The other side of the rule: a window whose end IS later than its start is served.
    let mut body = harvest_body(future, future + 1.0, &[MANIFEST]);
    body["HarvestJobName"] = json!("schedule-accepted");
    let accepted = emulator
        .call(
            "POST",
            &format!(
                "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/harvestJob"
            ),
            Some(body),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"a window one second long\": expected HTTP 200, got {} with body {}",
        accepted.status, accepted.body
    );
}

/// Every container type and manifest family the model's `ValidationExceptionType` enum
/// names a `<container>_CONTAINER_TYPE_WITH_<family>_MANIFEST` value for: the request
/// member that carries the family, the name the message gives the family, and the
/// discriminator. Read off that enum, not off one recorded request.
const INCOMPATIBLE: &[(&str, &str, &str, &str)] = &[
    ("TS", "DashManifests", "DASH", "TS_CONTAINER_TYPE_WITH_DASH_MANIFEST"),
    ("TS", "MssManifests", "MSS", "TS_CONTAINER_TYPE_WITH_MSS_MANIFEST"),
    ("CMAF", "MssManifests", "MSS", "CMAF_CONTAINER_TYPE_WITH_MSS_MANIFEST"),
    ("ISM", "HlsManifests", "HLS", "ISM_CONTAINER_TYPE_WITH_HLS_MANIFEST"),
    (
        "ISM",
        "LowLatencyHlsManifests",
        "low-latency HLS",
        "ISM_CONTAINER_TYPE_WITH_LL_HLS_MANIFEST",
    ),
    ("ISM", "DashManifests", "DASH", "ISM_CONTAINER_TYPE_WITH_DASH_MANIFEST"),
];

#[tokio::test]
async fn a_manifest_family_its_container_cannot_serve_is_refused_at_the_request() {
    let emulator = Emulator::new();
    let created = emulator.create_group(GROUP, json!({})).await;
    assert_eq!(created.status, 200, "setup: CreateChannelGroup {}", created.body);
    let created = emulator.create_channel(GROUP, CHANNEL, json!({})).await;
    assert_eq!(created.status, 200, "setup: CreateChannel {}", created.body);

    for (index, (container, member, family, exception_type)) in
        INCOMPATIBLE.iter().enumerate()
    {
        let label = format!("{container} with a {family} manifest");
        // The manifest NAME differs per row, so a sentence that echoed it would differ.
        let manifest = format!("m-{index}");
        let mut body = json!({"ContainerType": container});
        body[*member] = json!([{"ManifestName": manifest}]);
        let answer = emulator
            .create_endpoint(GROUP, CHANNEL, &format!("ep-{index}"), body)
            .await;
        let expected = format!(
            "{ENVELOPE}: {family} manifest is not supported on endpoint with {container} \
             container type. "
        );
        assert_sentence(
            &label,
            &answer,
            &expected,
            exception_type,
            &[manifest.as_str(), GROUP, CHANNEL],
        );
        // AWS terminates this sentence with a space, which is part of the text.
        assert!(
            message(&answer).ends_with(". "),
            "case {label:?}: the sentence must end in a space, got {:?}",
            message(&answer)
        );
    }
}

#[tokio::test]
async fn a_document_that_is_not_an_iam_policy_is_refused_at_the_policy_member() {
    let emulator = hierarchy().await;
    // Documents that are not IAM policies, differing in HOW they fail to be one: not JSON
    // at all, a JSON scalar, a JSON array, an object with no statement, and an empty
    // object. The sentence is the same for all of them and names none of them.
    let documents: &[(&str, &str)] = &[
        ("text that is not JSON", "not-a-policy"),
        ("a JSON string", "\"policy\""),
        ("a JSON number", "17"),
        ("a JSON array of statements", "[{\"Effect\":\"Allow\"}]"),
        (
            "an object with no statement",
            "{\"Version\":\"2012-10-17\",\"Effect\":\"Allow\"}",
        ),
        ("an empty object", "{}"),
    ];
    // Both policy writers, so one rejected document cannot be worded two ways. Each is
    // addressed with the method the model binds it at (`PutChannelPolicy` PUT,
    // `PutOriginEndpointPolicy` POST).
    let writers: &[(&str, &str, String)] = &[
        (
            "PutChannelPolicy",
            "PUT",
            format!("/channelGroup/{GROUP}/channel/{CHANNEL}/policy"),
        ),
        (
            "PutOriginEndpointPolicy",
            "POST",
            format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/policy"),
        ),
    ];
    for (writer, method, path) in writers {
        for (label, document) in documents {
            let case = format!("{writer} with {label}");
            let answer = emulator
                .call(method, path, Some(json!({"Policy": document})))
                .await;
            assert_sentence(
                &case,
                &answer,
                INVALID_POLICY,
                "INVALID_POLICY",
                &[document, GROUP, CHANNEL, ENDPOINT],
            );
        }
    }
}

#[tokio::test]
async fn an_arn_naming_no_resource_is_refused_with_the_terse_not_found_sentence() {
    let emulator = hierarchy().await;
    // Well-formed ARNs of this service naming a resource that is not there, one per level
    // of the hierarchy plus one whose names differ in shape — the rule is about the
    // resource being absent, not about which level or how it is named.
    let group_arn = |group: &str| {
        format!("arn:aws:mediapackagev2:{REGION}:{ACCOUNT}:channelGroup/{group}")
    };
    let absent: Vec<(&str, String)> = vec![
        ("an absent channel group", group_arn("no-such-group")),
        (
            "an absent channel of a group that exists",
            format!("{}/channel/no-such-channel", group_arn(GROUP)),
        ),
        (
            "an absent origin endpoint of a channel that exists",
            format!(
                "{}/channel/{CHANNEL}/originEndpoint/no-such-endpoint",
                group_arn(GROUP)
            ),
        ),
        (
            "an absent harvest job of an endpoint that exists",
            format!(
                "{}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/harvestJob/no-such-job",
                group_arn(GROUP)
            ),
        ),
        (
            "an absent channel group named with digits and underscores",
            group_arn("Grp_0123456789"),
        ),
    ];
    // Every tagging operation resolves the ARN the same way, so each must answer the same
    // class and the same sentence.
    for (label, arn) in &absent {
        for (operation, method, body) in [
            ("ListTagsForResource", "GET", None),
            ("TagResource", "POST", Some(json!({"tags": {"k": "v"}}))),
            ("UntagResource", "DELETE", None),
        ] {
            let case = format!("{operation} with {label}");
            let path = match operation {
                "UntagResource" => format!("/tags/{}?tagKeys=k", encoded(arn)),
                _ => format!("/tags/{}", encoded(arn)),
            };
            let answer = emulator.call(method, &path, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.string("__type"),
                "NotFoundException",
                "case {case:?}: expected NotFoundException, got {}",
                answer.body
            );
            assert_eq!(
                message(&answer),
                RESOURCE_NOT_FOUND,
                "case {case:?}: expected the message {RESOURCE_NOT_FOUND:?}, got {:?}",
                message(&answer)
            );
            assert!(
                !message(&answer).contains(arn.as_str()),
                "case {case:?}: the sentence must not describe the ARN it could not \
                 resolve, got {:?}",
                message(&answer)
            );
        }
    }

    // The other side: an ARN that DOES name a resource is tagged, so the 404s above are
    // about the resource being absent and not about the ARNs being unreadable.
    let arn = format!(
        "arn:aws:mediapackagev2:{REGION}:{ACCOUNT}:channelGroup/{GROUP}/channel/{CHANNEL}"
    );
    let tagged = emulator
        .call(
            "POST",
            &format!("/tags/{}", encoded(&arn)),
            Some(json!({"tags": {"k": "v"}})),
        )
        .await;
    assert_eq!(
        tagged.status, 204,
        "case \"a channel that exists\": expected HTTP 204, got {} with body {}",
        tagged.status, tagged.body
    );
    let listed: Value = emulator
        .call("GET", &format!("/tags/{}", encoded(&arn)), None)
        .await
        .json;
    assert_eq!(
        listed["tags"]["k"],
        json!("v"),
        "case \"a channel that exists\": the tag must read back, got {listed}"
    );
}
