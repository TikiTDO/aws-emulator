// SPDX-License-Identifier: CC0-1.0

//! The metadata transfer job lifecycle, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! The suites assert RULES over a table of inputs rather than one recorded
//! case: what a `CreateMetadataTransferJob` persists is exactly what
//! `GetMetadataTransferJob` reports, whatever end types and optional members
//! the create carried; the create answers the non-terminal `VALIDATING` and the
//! CLOCK — not a caller — settles the job in the outcome its RUN earned; a
//! configuration that does not match the `type` beside it, or that names a
//! workspace this service does not hold, is refused with the modelled error and
//! persists nothing; and a taken id conflicts.
//!
//! No object store is mounted beside this service here, so a job whose source
//! is an Amazon S3 document has nothing to read: that is the FAILURE half of
//! the rule, which this file asserts. The COMPLETED half — a document really
//! read out of the object store and really written into a workspace — needs the
//! owner of objects on the same engine and lives in
//! `tests/all/metadata_transfer_run.rs`.

use std::thread::sleep;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::{
    DestinationConfiguration, SourceConfiguration,
};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    metadata_transfer_job_role, MetadataTransferJob, MAX_METADATA_TRANSFER_JOBS,
    METADATA_TRANSFER_JOB_ERROR_CODE, METADATA_TRANSFER_JOB_SETTLE_SPAN,
    METADATA_TRANSFER_JOB_STATE_ERROR, METADATA_TRANSFER_JOB_STATE_VALIDATING,
};

use crate::harness::{metadata_transfer_job_arn, workspace_arn, Emulator, Resp, REGION};

/// The workspaces every suite here creates: an IoT TwinMaker end of a transfer
/// names a workspace by ARN, and two of them let a row name a different
/// workspace at each end.
const WORKSPACE: &str = "transferWorkspace";
/// See [`WORKSPACE`].
const OTHER_WORKSPACE: &str = "otherTransferWorkspace";

/// Block until the outcome an accepted job owes has fallen due.
///
/// The emulator stores the instant and never sleeps for it, so a test that
/// wants to observe the settled job is the one that has to wait; the margin
/// keeps the wait from racing the span's own boundary.
fn wait_until_settled() {
    sleep(METADATA_TRANSFER_JOB_SETTLE_SPAN + Duration::from_millis(100));
}

/// The `status.state` a response reports for a job.
fn reported_state(resp: &Resp) -> Option<&str> {
    resp.member("status")["state"].as_str()
}

/// Whether `id` is a value the modelled `Id` admits — `@length(1..128)` and
/// `^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$`, from
/// `smithy_query(query="shape",shape="Id")`, which is the constraint a
/// SERVICE-minted `metadataTransferJobId` has to satisfy just as a caller's
/// does.
fn is_modelled_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (2..=128).contains(&id.len())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
        && bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
}

/// An emulator holding the two workspaces the rows name.
async fn emulator_with_workspaces() -> Emulator {
    let emulator = Emulator::new();
    for workspace_id in [WORKSPACE, OTHER_WORKSPACE] {
        assert_eq!(
            emulator.create_workspace(workspace_id).await.status,
            200,
            "the workspace {workspace_id:?} the cases name must exist"
        );
    }
    emulator
}

/// The table every round-trip row is built from: `(case, request)`.
///
/// The rows differ along what the rule generalises over — which `SourceType`
/// and `DestinationType` the ends carry (`ops::types::SOURCE_TYPE_VALUES` and
/// `DESTINATION_TYPE_VALUES` are both `s3`, `iotsitewise`, `iottwinmaker`),
/// whether the end carries the optional filters its configuration admits, how
/// many filters it carries, whether the create names a `description` (and
/// whether that description is empty), and whether it names the
/// `metadataTransferJobId` at all — never along a mere rename.
fn rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "s3 into a workspace",
            json!({
                "metadataTransferJobId": "importJob1",
                "description": "import a metadata document",
                "sources": [{
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/entities.json"},
                }],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                },
            }),
        ),
        (
            "a workspace out to s3",
            json!({
                "sources": [{
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                }],
                "destination": {
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::export-bucket/exports"},
                },
            }),
        ),
        (
            "a filtered workspace out to s3",
            json!({
                "metadataTransferJobId": "exportJob_2",
                "description": "",
                "sources": [{
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {
                        "workspace": workspace_arn(OTHER_WORKSPACE),
                        // Two filters, in an order the service must not
                        // reorder: the filter list is a LIST.
                        "filters": [
                            {"filterByEntity": {"entityId": "pump1"}},
                            {"filterByComponentType": {"componentTypeId": "com.example.pump"}},
                        ],
                    },
                }],
                "destination": {
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::export-bucket/filtered"},
                },
            }),
        ),
        (
            "every IoT SiteWise asset model into a workspace",
            json!({
                // No `iotSiteWiseConfiguration` at all: every member of it is
                // optional, so this names the whole source.
                "sources": [{"type": "iotsitewise"}],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(OTHER_WORKSPACE)},
                },
            }),
        ),
        (
            "filtered IoT SiteWise assets into a workspace",
            json!({
                "metadataTransferJobId": "siteWiseImport_9",
                "description": "one asset model and its offspring",
                "sources": [{
                    "type": "iotsitewise",
                    "iotSiteWiseConfiguration": {"filters": [
                        {"filterByAssetModel": {
                            "assetModelId": "11111111-2222-3333-4444-555555555555",
                            "includeOffspring": true,
                            "includeAssets": false,
                        }},
                    ]},
                }],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                },
            }),
        ),
        (
            "s3 into IoT SiteWise",
            json!({
                "metadataTransferJobId": "siteWiseDestination1",
                // `DestinationConfiguration` defines no member for an IoT
                // SiteWise destination, so this end is the `type` alone.
                "sources": [{
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/assets.json"},
                }],
                "destination": {"type": "iotsitewise"},
            }),
        ),
    ]
}

/// RULE: a metadata transfer job read back reports exactly what its create
/// persisted — the sources and destination it named, verbatim, the description
/// it named or none, the ARN and the id the create answered with, and the
/// service's own transfer role — with `updateDateTime` at the creation instant
/// until the clock moves the job on.
#[tokio::test]
async fn a_metadata_transfer_job_reads_back_exactly_what_was_created() {
    let emulator = emulator_with_workspaces().await;

    for (case, request) in rows() {
        let created = emulator.create_metadata_transfer_job(request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateMetadataTransferJob, got {created:?}"
        );

        // The id is the caller's when the request named one, and the service's
        // own when it did not — minted as a value the modelled `Id` admits.
        let reported_id = created
            .member("metadataTransferJobId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        match request["metadataTransferJobId"].as_str() {
            Some(asked) => assert_eq!(
                reported_id, asked,
                "case {case:?}: expected the requested id, got {created:?}"
            ),
            None => assert!(
                is_modelled_id(&reported_id),
                "case {case:?}: a minted id must satisfy the modelled Id, got {created:?}"
            ),
        }
        assert_eq!(
            created.member("arn").as_str(),
            Some(metadata_transfer_job_arn(&reported_id).as_str()),
            "case {case:?}: expected the minted ARN, got {created:?}"
        );
        assert_eq!(
            reported_state(&created),
            Some(METADATA_TRANSFER_JOB_STATE_VALIDATING),
            "case {case:?}: a create answers the non-terminal VALIDATING, got {created:?}"
        );
        assert_eq!(
            created.member("status")["error"],
            json!({}),
            "case {case:?}: a job that failed at nothing carries an empty error, got {created:?}"
        );
        let creation_date_time = created.member("creationDateTime").as_f64();
        assert!(
            creation_date_time.is_some_and(|seconds| seconds > 0.0),
            "case {case:?}: expected an epoch-seconds creationDateTime, got {created:?}"
        );

        let read = emulator.get_metadata_transfer_job(&reported_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetMetadataTransferJob, got {read:?}"
        );
        assert_eq!(
            (
                read.member("metadataTransferJobId").as_str(),
                read.member("arn").as_str(),
            ),
            (
                Some(reported_id.as_str()),
                Some(metadata_transfer_job_arn(&reported_id).as_str()),
            ),
            "case {case:?}: expected the created identity, got {read:?}"
        );
        assert_eq!(
            read.member("description").as_str(),
            request["description"].as_str(),
            "case {case:?}: expected the created description, got {read:?}"
        );
        // The ends of the transfer are reported VERBATIM — every type-specific
        // configuration, every filter, in the order the create gave them.
        assert_eq!(
            read.member("sources"),
            &request["sources"],
            "case {case:?}: expected the created sources, got {read:?}"
        );
        assert_eq!(
            read.member("destination"),
            &request["destination"],
            "case {case:?}: expected the created destination, got {read:?}"
        );
        // The create accepts no role, so the role reported is the service's
        // own: an IAM role ARN in this emulator's account.
        assert_eq!(
            read.member("metadataTransferJobRole").as_str(),
            Some(metadata_transfer_job_role().as_str()),
            "case {case:?}: expected the service's transfer role, got {read:?}"
        );
        assert!(
            metadata_transfer_job_role().starts_with("arn:aws:iam::000000000000:role/"),
            "case {case:?}: the role must satisfy the modelled RoleArn, got {}",
            metadata_transfer_job_role()
        );
        assert_eq!(
            read.member("creationDateTime").as_f64(),
            creation_date_time,
            "case {case:?}: the read must report the create's own instant, got {read:?}"
        );
        assert_eq!(
            read.member("updateDateTime").as_f64(),
            creation_date_time,
            "case {case:?}: nothing has moved the job yet, so updateDateTime is the creation \
             instant, got {read:?}"
        );
        // Nothing transferred, so there is no count and no report to name.
        assert!(
            read.member("progress").is_null() && read.member("reportUrl").is_null(),
            "case {case:?}: a job that transferred nothing reports no progress and no \
             reportUrl, got {read:?}"
        );
    }
}

/// RULE: an accepted job is reported in the non-terminal `VALIDATING` state
/// until the instant its outcome falls due, and the CLOCK — not a caller's poll
/// — then settles it in the outcome its RUN earned; a run whose source could not
/// be read settles in the modelled failure state carrying the reason, and the
/// settled job is stable across reads.
///
/// Nothing holds the Amazon S3 document these rows name here (this harness
/// mounts no object store), so the outcome the run earns is the failure — and
/// the message names the source it could not read rather than stating a
/// limitation of the transfer itself.
#[tokio::test]
async fn an_accepted_job_settles_in_the_outcome_the_clock_owes() {
    let emulator = emulator_with_workspaces().await;

    // The rows differ in whether a reader observed the job during the settle
    // span, because the transition is the clock's and not a read's.
    let rows = [("polled", true), ("unpolled", false)];
    for (case, poll_while_validating) in rows {
        let created = emulator
            .create_metadata_transfer_job(json!({
                "metadataTransferJobId": case,
                "sources": [{
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/in.json"},
                }],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                },
            }))
            .await;
        assert_eq!(
            reported_state(&created),
            Some(METADATA_TRANSFER_JOB_STATE_VALIDATING),
            "case {case:?}: a create answers VALIDATING, got {created:?}"
        );
        if poll_while_validating {
            let during = emulator.get_metadata_transfer_job(case).await;
            assert_eq!(
                reported_state(&during),
                Some(METADATA_TRANSFER_JOB_STATE_VALIDATING),
                "case {case:?}: a read inside the settle span still reports VALIDATING, \
                 got {during:?}"
            );
        }
    }

    wait_until_settled();

    for (case, _) in rows {
        let settled = emulator.get_metadata_transfer_job(case).await;
        assert_eq!(
            reported_state(&settled),
            Some(METADATA_TRANSFER_JOB_STATE_ERROR),
            "case {case:?}: the outcome has fallen due, and the run had no source document to \
             read, got {settled:?}"
        );
        let error = &settled.member("status")["error"];
        assert_eq!(
            error["code"].as_str(),
            Some(METADATA_TRANSFER_JOB_ERROR_CODE),
            "case {case:?}: a failed run carries the processing error, got {settled:?}"
        );
        // The message names the source the run could not read — the address
        // from the request — rather than restating what the emulator does.
        let message = error["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("metadata-bucket/in.json"),
            "case {case:?}: the reason must name the source it could not read, got {settled:?}"
        );
        // The outcome fell due at the stored instant, not when a caller asked,
        // so two reads of a settled job report the same `updateDateTime` — and
        // it is later than the creation instant.
        let update_date_time = settled.member("updateDateTime").as_f64();
        let creation_date_time = settled.member("creationDateTime").as_f64();
        assert!(
            update_date_time > creation_date_time,
            "case {case:?}: settling is an update, so updateDateTime moved past \
             creationDateTime, got {settled:?}"
        );
        let again = emulator.get_metadata_transfer_job(case).await;
        assert_eq!(
            (
                reported_state(&again),
                again.member("updateDateTime").as_f64(),
            ),
            (Some(METADATA_TRANSFER_JOB_STATE_ERROR), update_date_time),
            "case {case:?}: a read must not move a settled job again, got {again:?}"
        );
        // The run never got as far as walking an entry, so there is nothing to
        // count.
        assert!(
            again.member("progress").is_null(),
            "case {case:?}: a run that read no document counts nothing, got {again:?}"
        );
    }
}

/// RULE: each end of a transfer must carry the one type-specific configuration
/// its `type` names, where the shape defines one, and none belonging to another
/// type; an end that does not is refused with HTTP 400 `ValidationException`
/// naming the member, and the refused create persists nothing.
#[tokio::test]
async fn a_create_whose_configuration_does_not_match_its_type_is_refused() {
    let emulator = emulator_with_workspaces().await;
    let s3_source = json!({
        "type": "s3",
        "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/in.json"},
    });
    let s3_destination = json!({
        "type": "s3",
        "s3Configuration": {"location": "arn:aws:s3:::export-bucket/out"},
    });
    let twin_maker_destination = json!({
        "type": "iottwinmaker",
        "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
    });

    // The rows span both ends of the transfer and both halves of the rule: a
    // configuration the `type` requires and is missing, and one that belongs to
    // a different type.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "s3 source with no s3Configuration",
            json!({"sources": [{"type": "s3"}], "destination": twin_maker_destination.clone()}),
            "sources[0]",
        ),
        (
            "IoT TwinMaker source with no iotTwinMakerConfiguration",
            json!({
                "sources": [{"type": "iottwinmaker"}],
                "destination": s3_destination.clone(),
            }),
            "sources[0]",
        ),
        (
            "IoT SiteWise source carrying an s3Configuration",
            json!({
                "sources": [{
                    "type": "iotsitewise",
                    "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/in.json"},
                }],
                "destination": twin_maker_destination.clone(),
            }),
            "sources[0]",
        ),
        (
            "s3 source carrying an iotTwinMakerConfiguration too",
            json!({
                "sources": [{
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/in.json"},
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                }],
                "destination": s3_destination.clone(),
            }),
            "sources[0]",
        ),
        (
            "s3 destination with no s3Configuration",
            json!({"sources": [s3_source.clone()], "destination": {"type": "s3"}}),
            "destination",
        ),
        (
            "IoT TwinMaker destination with no iotTwinMakerConfiguration",
            json!({
                "sources": [s3_source.clone()],
                "destination": {"type": "iottwinmaker"},
            }),
            "destination",
        ),
        (
            "IoT SiteWise destination carrying an s3Configuration",
            json!({
                "sources": [s3_source.clone()],
                "destination": {
                    "type": "iotsitewise",
                    "s3Configuration": {"location": "arn:aws:s3:::export-bucket/out"},
                },
            }),
            "destination",
        ),
    ];

    for (case, request, member) in rows {
        let refused = emulator.create_metadata_transfer_job(request).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected the modelled refusal, got {refused:?}"
        );
        assert!(
            refused
                .member("message")
                .as_str()
                .is_some_and(|message| message.contains(member)),
            "case {case:?}: the refusal must name the member it refused, got {refused:?}"
        );
    }

    // A `type` outside the model's own enum is refused too, so no job is stored
    // under an end type no read could report.
    let unmodelled = emulator
        .create_metadata_transfer_job(json!({
            "sources": [s3_source.clone()],
            "destination": {"type": "glacier"},
        }))
        .await;
    assert_eq!(
        unmodelled.status, 400,
        "case \"unmodelled destination type\": expected a 400, got {unmodelled:?}"
    );

    assert!(
        emulator
            .state()
            .lock()
            .region(REGION)
            .metadata_transfer_jobs
            .is_empty(),
        "a refused create must persist nothing, got {:?}",
        emulator.state().lock().region(REGION).metadata_transfer_jobs
    );
}

/// RULE: an IoT TwinMaker end of a transfer names a workspace by ARN, and a
/// create naming one this service does not hold is refused with HTTP 404
/// `ResourceNotFoundException` naming it; the refused create persists nothing.
#[tokio::test]
async fn a_create_naming_a_workspace_the_service_does_not_hold_is_refused() {
    let emulator = emulator_with_workspaces().await;
    let absent = workspace_arn("absentWorkspace");

    // The rows differ in which end names the absent workspace.
    let rows: Vec<(&str, Value)> = vec![
        (
            "absent source workspace",
            json!({
                "sources": [{
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": absent},
                }],
                "destination": {
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::export-bucket/out"},
                },
            }),
        ),
        (
            "absent destination workspace",
            json!({
                "sources": [{
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/in.json"},
                }],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": absent},
                },
            }),
        ),
        (
            "present source, absent destination workspace",
            json!({
                "sources": [{
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                }],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": absent},
                },
            }),
        ),
    ];

    for (case, request) in rows {
        let refused = emulator.create_metadata_transfer_job(request).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected the modelled 404, got {refused:?}"
        );
        assert!(
            refused
                .member("message")
                .as_str()
                .is_some_and(|message| message.contains(&absent)),
            "case {case:?}: the refusal must name what was missing, got {refused:?}"
        );
    }

    assert!(
        emulator
            .state()
            .lock()
            .region(REGION)
            .metadata_transfer_jobs
            .is_empty(),
        "a refused create must persist nothing"
    );

    // A read of a job this account does not hold is the same modelled 404.
    let read = emulator.get_metadata_transfer_job("absentJob").await;
    assert_eq!(
        (read.status, read.error_type.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"absent job\": expected the modelled 404, got {read:?}"
    );
}

/// RULE: a `metadataTransferJobId` is unique in the account, so re-creating a
/// taken one is refused with HTTP 409 `ConflictException` and leaves the stored
/// job exactly as the first create wrote it; and a create past the account's
/// declared ceiling is refused with HTTP 402 `ServiceQuotaExceededException`.
#[tokio::test]
async fn a_taken_job_id_conflicts_and_a_full_account_is_refused() {
    let emulator = emulator_with_workspaces().await;
    let first = emulator
        .create_metadata_transfer_job(json!({
            "metadataTransferJobId": "takenJob",
            "description": "the first one",
            "sources": [{
                "type": "s3",
                "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/first.json"},
            }],
            "destination": {
                "type": "iottwinmaker",
                "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
            },
        }))
        .await;
    assert_eq!(
        first.status, 200,
        "the job every row re-creates must exist first, got {first:?}"
    );

    // The rows differ in what the second create would have written.
    let rows: Vec<(&str, Value)> = vec![
        (
            "same request",
            json!({
                "metadataTransferJobId": "takenJob",
                "description": "the first one",
                "sources": [{
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/first.json"},
                }],
                "destination": {
                    "type": "iottwinmaker",
                    "iotTwinMakerConfiguration": {"workspace": workspace_arn(WORKSPACE)},
                },
            }),
        ),
        (
            "other ends",
            json!({
                "metadataTransferJobId": "takenJob",
                "description": "the second one",
                "sources": [{
                    "type": "iotsitewise",
                }],
                "destination": {
                    "type": "s3",
                    "s3Configuration": {"location": "arn:aws:s3:::export-bucket/second"},
                },
            }),
        ),
    ];

    for (case, request) in rows {
        let again = emulator.create_metadata_transfer_job(request).await;
        assert_eq!(
            (again.status, again.error_type.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: re-creating a taken id must conflict, got {again:?}"
        );
        let read = emulator.get_metadata_transfer_job("takenJob").await;
        assert_eq!(
            (
                read.member("description").as_str(),
                read.member("sources")[0]["type"].as_str(),
                read.member("destination")["type"].as_str(),
            ),
            (Some("the first one"), Some("s3"), Some("iottwinmaker")),
            "case {case:?}: a refused create must not overwrite the stored job, got {read:?}"
        );
    }

    // Fill the account to its declared ceiling through the store the handlers
    // read, so the quota refusal is exercised without a thousand round trips.
    {
        let state = emulator.state();
        let mut store = state.lock();
        let store = store.region_mut(REGION);
        for index in store.metadata_transfer_jobs.len()..MAX_METADATA_TRANSFER_JOBS {
            let id = format!("seededJob{index}");
            let job = MetadataTransferJob::accepted(
                id.clone(),
                metadata_transfer_job_arn(&id),
                None,
                vec![SourceConfiguration {
                    r#type: "iotsitewise".to_string(),
                    ..Default::default()
                }],
                DestinationConfiguration {
                    r#type: "iotsitewise".to_string(),
                    ..Default::default()
                },
                SystemTime::now(),
            );
            store.metadata_transfer_jobs.insert(id, job);
        }
    }
    let over = emulator
        .create_metadata_transfer_job(json!({
            "metadataTransferJobId": "oneTooMany",
            "sources": [{"type": "iotsitewise"}],
            "destination": {"type": "iotsitewise"},
        }))
        .await;
    assert_eq!(
        (over.status, over.error_type.as_deref()),
        (402, Some("ServiceQuotaExceededException")),
        "case \"full account\": expected the modelled quota refusal, got {over:?}"
    );
    assert!(
        over.member("message")
            .as_str()
            .is_some_and(|message| message.contains(&MAX_METADATA_TRANSFER_JOBS.to_string())),
        "case \"full account\": the refusal must name the ceiling, got {over:?}"
    );
    assert_eq!(
        emulator
            .state()
            .lock()
            .region(REGION)
            .metadata_transfer_jobs
            .len(),
        MAX_METADATA_TRANSFER_JOBS,
        "a refused create must persist nothing"
    );
}
