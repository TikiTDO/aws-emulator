// SPDX-License-Identifier: CC0-1.0

//! Where the shared `x-amzn-update-if-match` precondition sits relative to a conditional
//! writer's OWN rules about the stored resource.
//!
//! RULE: an operation that binds the entity-tag precondition judges it FIRST once the
//! addressed resource is in hand — ahead of every rule of its own that compares the
//! request against the STORED resource or that resource's parents — so a request
//! presenting a tag that is not the resource's current one is answered `ConflictException`
//! (409) / `CONFLICTING_OPERATION`, whichever of those rules its body would otherwise
//! break. A rule decided from the REQUEST alone is still answered first, because it needs
//! nothing out of the store at all (`request_refusal_precedence` holds that half).
//!
//! The rows span the class rather than one recorded request: one per (conditional writer,
//! own stored-resource rule), over EVERY such rule the three writers that have one apply —
//! `UpdateChannel`'s input-type restriction, all five of `UpdateOriginEndpoint`'s
//! (immutable container type, both stream-name output-mode rules and both
//! output-timestamp-mode rules) and `CancelHarvestJob`'s eligibility rule. The fourth
//! writer, `UpdateChannelGroup`, has no rule of this class — the description is its only
//! editable member — so `shared_entity_tag_precondition` is where its precondition is
//! pinned.
//!
//! Every row carries both controls: the SAME body with the precondition satisfied (the
//! committed tag, and no tag at all) earns the writer's own 400, so the 409 above stands
//! in for a real refusal rather than for an acceptance, and the resource is read back to
//! show none of the three refusals wrote anything.

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state::{
    VALIDATION_HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION,
    VALIDATION_ONLY_NON_EPOCH_LOCKED_ALLOW_OUTPUT_TIMESTAMP_MODE,
};

// The crate's shared test harness: the control-plane client with its error-envelope reader
// and the smallest acceptable `CreateOriginEndpoint` body (`channel_lifecycle`), the one
// `MultiviewConfiguration` builder (`multiview_configuration_validation`), the
// `CreateHarvestJob` body with its schedule clock (`rejection_rule_and_negative_paths`),
// the tag-map builder (`resource_tags`) and the shared precondition's own header, refusal
// sentence, reader and harvest-job settler (`shared_entity_tag_precondition`).
use crate::channel_lifecycle::{error_type, minimal_endpoint, Emulator};
use crate::multiview_configuration_validation::multiview;
use crate::rejection_rule_and_negative_paths::{epoch_seconds_from_now, harvest_body};
use crate::resource_tags::map_of;
use crate::shared_entity_tag_precondition::{
    ended_job, message, read, IF_MATCH, STALE_ETAG_MESSAGE,
};

/// The channel, origin endpoint and harvest job every row builds inside a channel group of
/// its own.
const CHANNEL: &str = "etp-channel";
/// See [`CHANNEL`].
const ENDPOINT: &str = "etp-endpoint";
/// See [`CHANNEL`].
const JOB: &str = "etp-job";

/// The manifest the endpoint publishes, and so the one a harvest job may harvest:
/// [`minimal_endpoint`] publishes exactly `index`.
const MANIFEST: &str = "index";

/// The tag another request commits over each row's resource with, to rotate its entity tag
/// and so make the tag the row holds stale.
const COMMITTED_BY_ANOTHER: (&str, &str) = ("committed-by", "another-request");

/// A conditional writer that has a rule of its own about the stored resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    Channel,
    OriginEndpoint,
    HarvestJob,
}

impl Writer {
    fn operation(self) -> &'static str {
        match self {
            Writer::Channel => "UpdateChannel",
            Writer::OriginEndpoint => "UpdateOriginEndpoint",
            Writer::HarvestJob => "CancelHarvestJob",
        }
    }
}

/// One row: a conditional writer, the parent channel its rule needs, a request body that
/// rule refuses by reading the STORED resource or its parents, and the
/// `ValidationExceptionType` the refusal names.
struct Row {
    /// What makes this row different from the others.
    label: &'static str,
    writer: Writer,
    /// The `CreateChannel` body for the channel the row's resource lives under.
    channel: fn() -> Value,
    /// The conditional write's body (`None` for `CancelHarvestJob`, which binds none).
    body: fn() -> Option<Value>,
    expected_type: &'static str,
}

/// The `UpdateOriginEndpoint` body every endpoint row starts from: the member its input
/// requires and the manifest set the endpoint was created with, so each row differs only in
/// the member whose rule it breaks.
fn endpoint_body() -> Value {
    minimal_endpoint("TS")
}

/// One row per (conditional writer, own stored-resource rule).
///
/// Every `ValidationExceptionType` below is the model's own
/// (`smithy_query(query="shape",shape="ValidationExceptionType")`), and the enum values
/// the bodies carry come from `InputType` (HLS | CMAF | MULTIVIEW), `OutputLockingMode`
/// (EPOCH_LOCKED | NON_EPOCH_LOCKED), `StreamNameOutputMode` (INDEX | PASSTHROUGH_NAME),
/// `OutputTimestampMode` (PASSTHROUGH | REBASED_TO_CHANNEL_START), `ContainerType` (CMAF |
/// ISM | TS) and `MultiviewLayoutType`.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a multiview configuration on a channel that is not MULTIVIEW",
            writer: Writer::Channel,
            channel: || json!({"InputType": "HLS"}),
            // The sources are never resolved: the input-type restriction is decided off the
            // stored channel before the source list is looked up.
            body: || Some(json!({"MultiviewConfiguration": multiview(&["tile"], &["LAYOUT_2EH"])})),
            expected_type: "MULTIVIEW_CONFIGURATION_NOT_ALLOWED",
        },
        Row {
            label: "a different ContainerType",
            writer: Writer::OriginEndpoint,
            channel: || json!({"InputType": "HLS"}),
            body: || Some(minimal_endpoint("CMAF")),
            expected_type: "CONTAINER_TYPE_IMMUTABLE",
        },
        Row {
            label: "a different ContainerType, with the manifest family that one serves",
            writer: Writer::OriginEndpoint,
            channel: || json!({"InputType": "HLS"}),
            body: || Some(minimal_endpoint("ISM")),
            expected_type: "CONTAINER_TYPE_IMMUTABLE",
        },
        Row {
            label: "a different StreamNameOutputMode",
            writer: Writer::OriginEndpoint,
            channel: || json!({"InputType": "HLS"}),
            body: || {
                let mut body = endpoint_body();
                body["StreamNameOutputMode"] = json!("PASSTHROUGH_NAME");
                Some(body)
            },
            expected_type: "STREAM_NAME_OUTPUT_MODE_IMMUTABLE",
        },
        Row {
            label: "a StreamNameOutputMode under a channel whose input type carries none",
            writer: Writer::OriginEndpoint,
            channel: || json!({"InputType": "CMAF"}),
            body: || {
                let mut body = endpoint_body();
                body["StreamNameOutputMode"] = json!("INDEX");
                Some(body)
            },
            expected_type: "ONLY_HLS_INPUT_TYPE_ALLOW_STREAM_NAME_OUTPUT_MODE",
        },
        Row {
            label: "a Segment.OutputTimestampMode under a channel that carries none",
            writer: Writer::OriginEndpoint,
            channel: || json!({"InputType": "CMAF", "OutputLockingMode": "EPOCH_LOCKED"}),
            body: || {
                let mut body = endpoint_body();
                body["Segment"] = json!({"OutputTimestampMode": "PASSTHROUGH"});
                Some(body)
            },
            expected_type: VALIDATION_ONLY_NON_EPOCH_LOCKED_ALLOW_OUTPUT_TIMESTAMP_MODE,
        },
        Row {
            label: "a different Segment.OutputTimestampMode",
            writer: Writer::OriginEndpoint,
            channel: || json!({"InputType": "CMAF", "OutputLockingMode": "NON_EPOCH_LOCKED"}),
            body: || {
                let mut body = endpoint_body();
                body["Segment"] = json!({"OutputTimestampMode": "REBASED_TO_CHANNEL_START"});
                Some(body)
            },
            expected_type: "OUTPUT_TIMESTAMP_MODE_IMMUTABLE",
        },
        Row {
            label: "a harvest job that is already terminal",
            writer: Writer::HarvestJob,
            channel: || json!({"InputType": "HLS"}),
            body: || None,
            expected_type: VALIDATION_HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION,
        },
    ]
}

/// Build this row's resource inside `group` and return the path its conditional writer and
/// its reader share (each of these operations is addressed on the resource's own path,
/// which its `Get` reads with a `GET`), leaving the resource in the state the row's own
/// rule refuses.
async fn fresh(emulator: &Emulator, row: &Row, group: &str) -> String {
    let label = row.label;
    let (status, body) = emulator.create_group(group, json!({})).await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateChannelGroup {group} expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .create_channel(group, CHANNEL, (row.channel)())
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateChannel in {group} expected 200, got {status} {body}"
    );
    // `UpdateChannel` and `GetChannel` keep the trailing slash the model gives them.
    let channel_path = format!("/channelGroup/{group}/channel/{CHANNEL}/");
    if row.writer == Writer::Channel {
        return channel_path;
    }

    let (status, body) = emulator
        .create_endpoint(group, CHANNEL, ENDPOINT, endpoint_body())
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateOriginEndpoint in {group} expected 200, got {status} {body}"
    );
    let endpoint_path = format!("/channelGroup/{group}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}");
    if row.writer == Writer::OriginEndpoint {
        return endpoint_path;
    }

    // A harvest job whose scheduled window had already CLOSED when it was created, so the
    // clock ends it of its own accord — which is the status `CancelHarvestJob` refuses as
    // ineligible, reached without any write of this suite's own.
    let start = epoch_seconds_from_now(-7200.0);
    let mut request = harvest_body(start, start + 3600.0, &[MANIFEST]);
    request["HarvestJobName"] = json!(JOB);
    let (status, body) = emulator
        .call("POST", &format!("{endpoint_path}/harvestJob"), Some(request))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: CreateHarvestJob in {group} expected 200, got {status} {body}"
    );
    let job_path = format!("{endpoint_path}/harvestJob/{JOB}");
    ended_job(emulator, label, &job_path).await;
    job_path
}

/// RULE: the entity-tag precondition is judged ahead of every rule a conditional writer
/// has of its own about the stored resource, so the caller that lost a race is told so
/// whichever resource it writes and whichever of those rules its body breaks.
#[tokio::test]
async fn the_entity_tag_precondition_outranks_every_stored_resource_rule() {
    let emulator = Emulator::new();

    for (index, row) in rows().iter().enumerate() {
        let case = format!("{} refusing {}", row.writer.operation(), row.label);
        let path = fresh(&emulator, row, &format!("etp-{index}")).await;

        // The tag the caller takes away, and the commit by somebody else that makes it
        // stale: the tags are part of every record's content, so writing one is a commit
        // and rotates the entity tag.
        let held = read(&emulator, &case, &path).await;
        let stale = held["ETag"].as_str().unwrap_or_default().to_string();
        assert!(
            !stale.is_empty(),
            "case {case:?}: the reader must report the resource's committed entity tag, \
             got {held}"
        );
        let arn = held["Arn"].as_str().unwrap_or_default().to_string();
        let (status, body) = emulator
            .tag(&arn, &map_of(&[COMMITTED_BY_ANOTHER]))
            .await;
        assert_eq!(
            status, 204,
            "case {case:?}: TagResource on {arn} expected 204, got {status} {body}"
        );
        let before = read(&emulator, &case, &path).await;
        assert_ne!(
            before["ETag"],
            json!(stale),
            "case {case:?}: a commit by another request must rotate the entity tag, got \
             {before}"
        );
        let committed = before["ETag"].as_str().unwrap_or_default().to_string();

        // The row's body breaks one of this writer's own stored-resource rules AND the tag
        // it presents is no longer the committed one: the lost race is what the caller is
        // told about, because that is the one thing a conditional write exists to report.
        let (status, answer) = emulator
            .call_with_headers("PUT", &path, (row.body)(), &[(IF_MATCH, stale.as_str())])
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a stale precondition must outrank the writer's own refusal, \
             got {status} {answer}"
        );
        assert_eq!(
            error_type(&answer),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {answer}"
        );
        assert_eq!(
            answer["ConflictExceptionType"],
            json!("CONFLICTING_OPERATION"),
            "case {case:?}: expected CONFLICTING_OPERATION, got {answer}"
        );
        assert_eq!(
            message(&answer),
            STALE_ETAG_MESSAGE,
            "case {case:?}: expected the fixed sentence for a stale entity tag, got {answer}"
        );

        // Control: the SAME body with the precondition satisfied — the committed tag, and
        // no tag at all — earns the writer's own refusal, so the 409 above replaced a real
        // 400 rather than an acceptance.
        for (presented, headers) in [
            ("the committed tag", vec![(IF_MATCH, committed.as_str())]),
            ("no tag at all", Vec::new()),
        ] {
            let satisfied = format!("{case}, presenting {presented}");
            let (status, answer) = emulator
                .call_with_headers("PUT", &path, (row.body)(), &headers)
                .await;
            assert_eq!(
                status, 400,
                "case {satisfied:?}: the precondition holds, so the writer's own rule must \
                 answer; got {status} {answer}"
            );
            assert_eq!(
                error_type(&answer),
                "ValidationException",
                "case {satisfied:?}: expected the modelled ValidationException, got {answer}"
            );
            assert_eq!(
                answer["ValidationExceptionType"],
                json!(row.expected_type),
                "case {satisfied:?}: expected {}, got {answer}",
                row.expected_type
            );
        }

        // None of the three refusals wrote anything: the resource is exactly as the commit
        // by the other request left it.
        let after = read(&emulator, &case, &path).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused write must leave the resource exactly as it was; got \
             {after} against {before}"
        );
    }
}
