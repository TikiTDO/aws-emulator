// SPDX-License-Identifier: CC0-1.0

//! The WORDING of this service's not-found refusal, end to end over HTTP.
//!
//! The wire harness is the sibling suite's ([`crate::kinesis_video_stream_pool_lifecycle`]):
//! one `MultiServiceRouter` with this service mounted on one shared engine, so every
//! assertion here is about the emulator's HTTP behaviour rather than an internal helper.
//!
//! The RULE pinned here: this service answers every absent-resource refusal with ONE
//! `NotFoundException` body — HTTP 404, `Code` `NotFound` and a `Message` that is fixed
//! text — whatever operation was called, whichever resource kind the request named, and
//! whichever identifying form it used; nothing from the request or from the store is
//! interpolated into it, so the identifier the caller sent appears nowhere in the body.
//!
//! The tables span what the rule generalises over, rather than one row:
//!
//! * the OPERATION: every implemented operation that can report a resource absent —
//!   reads, updates, deletes, a create that resolves another resource, and the three
//!   generic tagging routes;
//! * the resource KIND: media insights pipeline configurations, media insights
//!   pipelines, Kinesis Video Stream pools, media capture pipelines, media pipelines and
//!   speaker search tasks;
//! * the identifier FORM and SIZE: a bare name, this service's own ARN form, another
//!   service's ARN, a GUID, a value that is no identifier form at all, and an identifier
//!   at the modelled member's maximum length;
//! * whether the identifier never resolved or named a resource that was created, read
//!   back and then DELETED.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::state::NOT_FOUND_MESSAGE;

use crate::kinesis_video_stream_pool_lifecycle::{
    encode_label, Emulator, Response, ACCOUNT, REGION,
};
use crate::media_insights_pipeline_status::configuration_arn;

/// The routes this suite drives, from `smithy_query(query="digest")`.
const CONFIGURATIONS: &str = "/media-insights-pipeline-configurations";
const INSIGHTS_PIPELINES: &str = "/media-insights-pipelines";
const PIPELINE_STATUS: &str = "/media-insights-pipeline-status";
const POOLS: &str = "/media-pipeline-kinesis-video-stream-pools";
const CAPTURE_PIPELINES: &str = "/sdk-media-capture-pipelines";
const MEDIA_PIPELINES: &str = "/sdk-media-pipelines";
const TAGS: &str = "/tags";

/// The wire error-shape name restJson1 resolves from `x-amzn-errortype`.
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";

/// The `ErrorCode` value the modelled `NotFoundException` carries in its body `Code`
/// member. Source: `smithy_query(query="shape",shape="ErrorCode")` on this service's
/// model — `BadRequest | Forbidden | NotFound | ResourceLimitExceeded | ServiceFailure |
/// ServiceUnavailable | Throttling`.
const ERROR_CODE_NOT_FOUND: &str = "NotFound";

/// The members `NotFoundException` declares
/// (`smithy_query(query="shape",shape="NotFoundException")`), plus the `__type`
/// discriminator restJson1 may put in the body. Nothing else may appear in a 404 body.
const ERROR_BODY_MEMBERS: [&str; 4] = ["Code", "Message", "RequestId", "__type"];

/// A `ResourceAccessRoleArn` satisfying the modelled `Arn` shape (len 1..1024, pattern
/// `^arn[\/\:\-\_\.a-zA-Z0-9]+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/media-insights-access";

/// A GUID the store minted nothing under, satisfying the modelled `GuidString`
/// (len 36, pattern `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`).
const UNMINTED_GUID: &str = "8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e";

/// One request this service can answer the modelled 404 for, already carrying the
/// identifier the row drives it with.
struct Refusal {
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

/// The operations whose identifier is a name-or-ARN `@httpLabel` (`NonEmptyString`,
/// len 0..1024, pattern `\S`), driven with `identifier`.
fn label_refusals(identifier: &str) -> Vec<Refusal> {
    let label = encode_label(identifier);
    vec![
        Refusal {
            operation: "GetMediaInsightsPipelineConfiguration",
            method: "GET",
            path: format!("{CONFIGURATIONS}/{label}"),
            body: None,
        },
        Refusal {
            operation: "UpdateMediaInsightsPipelineConfiguration",
            method: "PUT",
            path: format!("{CONFIGURATIONS}/{label}"),
            body: Some(json!({
                "ResourceAccessRoleArn": ROLE_ARN,
                "Elements": [{
                    "Type": "SqsQueueSink",
                    "SqsQueueSinkConfiguration": {
                        "InsightsTarget": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:insights"),
                    },
                }],
            })),
        },
        Refusal {
            operation: "DeleteMediaInsightsPipelineConfiguration",
            method: "DELETE",
            path: format!("{CONFIGURATIONS}/{label}"),
            body: None,
        },
        Refusal {
            operation: "GetMediaPipelineKinesisVideoStreamPool",
            method: "GET",
            path: format!("{POOLS}/{label}"),
            body: None,
        },
        Refusal {
            operation: "UpdateMediaPipelineKinesisVideoStreamPool",
            method: "PUT",
            path: format!("{POOLS}/{label}"),
            body: Some(json!({ "StreamConfiguration": { "DataRetentionInHours": 2 } })),
        },
        Refusal {
            operation: "DeleteMediaPipelineKinesisVideoStreamPool",
            method: "DELETE",
            path: format!("{POOLS}/{label}"),
            body: None,
        },
        Refusal {
            operation: "UpdateMediaInsightsPipelineStatus",
            method: "PUT",
            path: format!("{PIPELINE_STATUS}/{label}"),
            body: Some(json!({ "UpdateStatus": "Pause" })),
        },
        Refusal {
            operation: "StartSpeakerSearchTask",
            method: "POST",
            path: format!("{INSIGHTS_PIPELINES}/{label}/speaker-search-tasks?operation=start"),
            body: Some(json!({
                "VoiceProfileDomainArn":
                    format!("arn:aws:chime:{REGION}:{ACCOUNT}:voice-profile-domain/domain-a"),
            })),
        },
        Refusal {
            operation: "GetSpeakerSearchTask",
            method: "GET",
            path: format!("{INSIGHTS_PIPELINES}/{label}/speaker-search-tasks/{UNMINTED_GUID}"),
            body: None,
        },
    ]
}

/// The operations whose identifier is a `GuidString` `@httpLabel`, driven with
/// `identifier`.
fn guid_refusals(identifier: &str) -> Vec<Refusal> {
    let label = encode_label(identifier);
    vec![
        Refusal {
            operation: "GetMediaCapturePipeline",
            method: "GET",
            path: format!("{CAPTURE_PIPELINES}/{label}"),
            body: None,
        },
        Refusal {
            operation: "DeleteMediaCapturePipeline",
            method: "DELETE",
            path: format!("{CAPTURE_PIPELINES}/{label}"),
            body: None,
        },
        Refusal {
            operation: "GetMediaPipeline",
            method: "GET",
            path: format!("{MEDIA_PIPELINES}/{label}"),
            body: None,
        },
    ]
}

/// The operations that resolve a resource by an ARN member rather than a path label —
/// `Arn` on the create (len 1..1024) and `AmazonResourceName` on the three tagging
/// routes (len 1..1011) — driven with `arn`.
fn arn_refusals(arn: &str) -> Vec<Refusal> {
    vec![
        Refusal {
            operation: "CreateMediaInsightsPipeline",
            method: "POST",
            path: INSIGHTS_PIPELINES.to_string(),
            body: Some(json!({ "MediaInsightsPipelineConfigurationArn": arn })),
        },
        Refusal {
            operation: "ListTagsForResource",
            method: "GET",
            // `ResourceARN` is an `@httpQuery` member here, so it travels
            // percent-encoded as a query value.
            path: format!("{TAGS}?arn={}", encode_label(arn)),
            body: None,
        },
        Refusal {
            operation: "TagResource",
            method: "POST",
            path: format!("{TAGS}?operation=tag-resource"),
            body: Some(json!({
                "ResourceARN": arn,
                "Tags": [{ "Key": "team", "Value": "voice" }],
            })),
        },
        Refusal {
            operation: "UntagResource",
            method: "POST",
            path: format!("{TAGS}?operation=untag-resource"),
            body: Some(json!({ "ResourceARN": arn, "TagKeys": ["team"] })),
        },
    ]
}

/// Create one Kinesis Video Stream pool named `name` and answer its ARN.
async fn pool_arn(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .call(
            "POST",
            POOLS,
            Some(json!({
                "PoolName": name,
                "StreamConfiguration": { "Region": REGION, "DataRetentionInHours": 2 },
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the pool {name:?} must be created, got {} body {:?}",
        created.status, created.body
    );
    created.str_at("/KinesisVideoStreamPoolConfiguration/PoolArn")
}

/// Create one media capture pipeline and answer its `(MediaPipelineId, ARN)`.
async fn capture_pipeline(emulator: &Emulator, suffix: &str) -> (String, String) {
    let created = emulator
        .call(
            "POST",
            CAPTURE_PIPELINES,
            Some(json!({
                "SourceType": "ChimeSdkMeeting",
                "SourceArn": format!("arn:aws:chime:{REGION}:{ACCOUNT}:meeting/m-{suffix}"),
                "SinkType": "S3Bucket",
                "SinkArn": format!("arn:aws:s3:::bucket-{suffix}/captures"),
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "the capture pipeline {suffix:?} must be created, got {} body {:?}",
        created.status, created.body
    );
    (
        created.str_at("/MediaCapturePipeline/MediaPipelineId"),
        created.str_at("/MediaCapturePipeline/MediaPipelineArn"),
    )
}

/// Assert `response` is this service's one not-found refusal: the modelled 404 and error
/// code, the FIXED message, a `RequestId`, no other member — and no trace of the
/// identifier the request carried.
fn assert_fixed_not_found(
    case: &str,
    identifier: &str,
    response: &Response,
    messages: &mut BTreeSet<String>,
) {
    assert_eq!(
        response.status, 404,
        "case {case:?}: expected the modelled 404, got {} body {:?}",
        response.status, response.body
    );
    assert_eq!(
        response.error_code(),
        NOT_FOUND_EXCEPTION,
        "case {case:?}: expected the modelled error shape, got {:?}",
        response.body
    );
    assert_eq!(
        response.str_at("/Code"),
        ERROR_CODE_NOT_FOUND,
        "case {case:?}: expected the modelled ErrorCode, got {:?}",
        response.body
    );
    let message = response.str_at("/Message");
    assert_eq!(
        message, NOT_FOUND_MESSAGE,
        "case {case:?}: expected the service's one fixed not-found message, got {message:?}"
    );
    assert!(
        !response.str_at("/RequestId").is_empty(),
        "case {case:?}: every error of this service carries a RequestId: {:?}",
        response.body
    );
    let rendered = response.body.to_string();
    assert!(
        !rendered.contains(identifier),
        "case {case:?}: the 404 body must echo no part of the requested identifier \
         {identifier:?}, got {rendered}"
    );
    let members: Vec<String> = response
        .body
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();
    for member in &members {
        assert!(
            ERROR_BODY_MEMBERS.contains(&member.as_str()),
            "case {case:?}: the 404 body carries {member:?}, which NotFoundException does \
             not declare: {rendered}"
        );
    }
    messages.insert(message);
}

/// RULE: every operation of this service answers an absent resource with the one fixed
/// `NotFoundException` body — same status, same `Code`, same `Message` — for every
/// resource kind and every identifier form, and that body echoes nothing the request
/// carried.
#[tokio::test]
async fn every_not_found_refusal_carries_the_services_one_fixed_message() {
    let emulator = Emulator::new();
    // Speaker search is enrolled for out of band, so the voice-analytics rows reach the
    // identifier lookup instead of the enrolment refusal.
    emulator.store().lock().set_voice_analytics_enabled(true);

    // Live neighbours of each kind, so a miss is decided per identifier and not by an
    // empty store.
    let live_configuration = "live-configuration";
    let live_configuration_arn = configuration_arn(&emulator, live_configuration).await;
    let live_pool_arn = pool_arn(&emulator, "live-pool").await;
    let (live_capture_id, live_capture_arn) = capture_pipeline(&emulator, "live").await;

    // Resources created, read back, and then deleted: their identifiers resolved once and
    // must not resolve now.
    let gone_configuration = "deleted-configuration";
    let gone_configuration_arn = configuration_arn(&emulator, gone_configuration).await;
    let read_back = emulator
        .call("GET", &format!("{CONFIGURATIONS}/{gone_configuration}"), None)
        .await;
    assert_eq!(
        read_back.status, 200,
        "the configuration to delete must read back 200 first, got {} body {:?}",
        read_back.status, read_back.body
    );
    let deleted = emulator
        .call(
            "DELETE",
            &format!("{CONFIGURATIONS}/{gone_configuration}"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "the configuration must delete, got {} body {:?}",
        deleted.status, deleted.body
    );

    let gone_pool = "deleted-pool";
    let gone_pool_arn = pool_arn(&emulator, gone_pool).await;
    let pool_deleted = emulator
        .call("DELETE", &format!("{POOLS}/{gone_pool}"), None)
        .await;
    assert_eq!(
        pool_deleted.status, 204,
        "the pool must delete, got {} body {:?}",
        pool_deleted.status, pool_deleted.body
    );

    let (gone_capture_id, gone_capture_arn) = capture_pipeline(&emulator, "deleted").await;
    let capture_deleted = emulator
        .call(
            "DELETE",
            &format!("{CAPTURE_PIPELINES}/{gone_capture_id}"),
            None,
        )
        .await;
    assert_eq!(
        capture_deleted.status, 204,
        "the capture pipeline must delete, got {} body {:?}",
        capture_deleted.status, capture_deleted.body
    );

    // Every message the service answered with, so the rows can be checked to agree on
    // ONE wording rather than each agreeing with the constant separately.
    let mut messages: BTreeSet<String> = BTreeSet::new();

    // The name-or-ARN rows: the form and the size the `NonEmptyString` label admits.
    let label_rows: Vec<(String, String)> = vec![
        (
            "a bare name nothing was created under".to_string(),
            "no-such-resource".to_string(),
        ),
        (
            "this service's own ARN form for a name nothing holds".to_string(),
            format!(
                "arn:aws:chime:{REGION}:{ACCOUNT}:media-insights-pipeline-configuration/no-such-resource"
            ),
        ),
        (
            "an ARN of another resource type in the same namespace".to_string(),
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline/{UNMINTED_GUID}"),
        ),
        (
            "another service's ARN".to_string(),
            format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/insights"),
        ),
        (
            "a GUID, which is no identifier form of these resources".to_string(),
            UNMINTED_GUID.to_string(),
        ),
        (
            "a value that is no identifier form at all".to_string(),
            "nope".to_string(),
        ),
        (
            // The label's own modelled maximum: `NonEmptyString` is len 0..1024.
            "an identifier at the label's 1024-character maximum".to_string(),
            "z".repeat(1024),
        ),
        (
            "a deleted configuration's name".to_string(),
            gone_configuration.to_string(),
        ),
        (
            "a deleted configuration's ARN".to_string(),
            gone_configuration_arn.clone(),
        ),
        ("a deleted pool's name".to_string(), gone_pool.to_string()),
        ("a deleted pool's ARN".to_string(), gone_pool_arn),
    ];
    for (case, identifier) in &label_rows {
        for refusal in label_refusals(identifier) {
            let response = emulator
                .call(refusal.method, &refusal.path, refusal.body.clone())
                .await;
            assert_fixed_not_found(
                &format!("{} / {case}", refusal.operation),
                identifier,
                &response,
                &mut messages,
            );
        }
    }

    // The GUID rows: the only form a `GuidString` label admits, in both hex cases the
    // modelled pattern allows.
    let guid_rows: Vec<(String, String)> = vec![
        (
            "a GUID nothing was minted under".to_string(),
            UNMINTED_GUID.to_string(),
        ),
        (
            "the same GUID in upper-case hex, which the pattern also allows".to_string(),
            UNMINTED_GUID.to_uppercase(),
        ),
        (
            "a deleted capture pipeline's id".to_string(),
            gone_capture_id.clone(),
        ),
    ];
    for (case, identifier) in &guid_rows {
        for refusal in guid_refusals(identifier) {
            let response = emulator
                .call(refusal.method, &refusal.path, refusal.body.clone())
                .await;
            assert_fixed_not_found(
                &format!("{} / {case}", refusal.operation),
                identifier,
                &response,
                &mut messages,
            );
        }
    }

    // The ARN-member rows, sized within the shorter of the two modelled bounds
    // (`AmazonResourceName` is len 1..1011).
    let arn_rows: Vec<(String, String)> = vec![
        (
            "this service's ARN form for a configuration nothing holds".to_string(),
            format!(
                "arn:aws:chime:{REGION}:{ACCOUNT}:media-insights-pipeline-configuration/no-such-resource"
            ),
        ),
        (
            "another service's ARN".to_string(),
            format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/insights"),
        ),
        (
            "an ARN at the tagging member's 1011-character maximum".to_string(),
            format!("arn{}", "a".repeat(1008)),
        ),
        (
            "a deleted configuration's ARN".to_string(),
            gone_configuration_arn.clone(),
        ),
        (
            "a deleted capture pipeline's ARN".to_string(),
            gone_capture_arn.clone(),
        ),
    ];
    for (case, arn) in &arn_rows {
        for refusal in arn_refusals(arn) {
            let response = emulator
                .call(refusal.method, &refusal.path, refusal.body.clone())
                .await;
            assert_fixed_not_found(
                &format!("{} / {case}", refusal.operation),
                arn,
                &response,
                &mut messages,
            );
        }
    }

    assert_eq!(
        messages.iter().cloned().collect::<Vec<String>>(),
        vec![NOT_FOUND_MESSAGE.to_string()],
        "every operation and every resource kind must word the refusal alike, got {messages:?}"
    );

    // The refusals decided nothing about the live neighbours: each still reads back.
    for (case, path) in [
        (
            "the live configuration",
            format!("{CONFIGURATIONS}/{live_configuration}"),
        ),
        (
            "the live configuration by ARN",
            format!("{CONFIGURATIONS}/{}", encode_label(&live_configuration_arn)),
        ),
        (
            "the live pool by ARN",
            format!("{POOLS}/{}", encode_label(&live_pool_arn)),
        ),
        (
            "the live capture pipeline",
            format!("{CAPTURE_PIPELINES}/{live_capture_id}"),
        ),
        (
            "the live capture pipeline's tags",
            format!("{TAGS}?arn={}", encode_label(&live_capture_arn)),
        ),
    ] {
        let response = emulator.call("GET", &path, None).await;
        assert_eq!(
            response.status, 200,
            "case {case:?}: a refused call must leave it readable, got {} body {:?}",
            response.status, response.body
        );
    }
}
