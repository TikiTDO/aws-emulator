// SPDX-License-Identifier: CC0-1.0

//! `DeleteSyncJob`, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, both `@httpLabel`s, a
//! status code and a serialized body are all exercised, not just a handler body.
//!
//! The suites assert RULES over tables of inputs rather than one recorded case:
//!
//! * a delete removes the sync job its labels address from the workspace that
//!   holds it, answers the `DELETING` state its response models, and stops the
//!   ARN the job was minted under resolving — whatever role and tags the job
//!   carried and whichever lifecycle state the clock had moved it into;
//! * both labels address an EXISTING resource, so one naming something the store
//!   does not hold is the modelled `ResourceNotFoundException` (HTTP 404) that
//!   says what was missing, and it removes nothing;
//! * a delete touches exactly the one job it addresses: every other job of the
//!   workspace, and every job of another workspace, reads and lists back
//!   unchanged, and the freed source can be created again.
//!
//! A workspace holding MORE than one sync job cannot be built over the wire — a
//! sync job is identified inside its workspace by its source, and the only
//! source the service supports is `SITEWISE` — so those rows are seeded straight
//! into the store the handlers read (`harness::seed_sync_job`), while the rule
//! under test is about the jobs the workspace holds, whatever wrote them.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::SYNC_JOB_STATE_VALUES;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    workspace_not_found_message, SYNC_JOB_NOT_FOUND_MESSAGE, SYNC_JOB_SETTLE_SPAN,
    SYNC_JOB_STATE_DELETING, SYNC_SOURCE_SITEWISE,
};

use crate::harness::{seed_sync_job, sync_job_arn, Emulator, Resp, SYNC_ROLE, REGION};

/// Block until the outcome an accepted job owes has fallen due, so a row that
/// deletes a SETTLED job really is deleting one the clock has already moved.
fn wait_until_settled() {
    sleep(SYNC_JOB_SETTLE_SPAN + Duration::from_millis(100));
}

/// An emulator holding the workspaces `workspace_ids` names.
async fn emulator_with_workspaces(workspace_ids: &[&str]) -> Emulator {
    let emulator = Emulator::new();
    for workspace_id in workspace_ids {
        assert_eq!(
            emulator.create_workspace(workspace_id).await.status,
            200,
            "the workspace {workspace_id:?} the cases name must exist"
        );
    }
    emulator
}

/// The `syncSource`s a listing of `workspace_id` reports, in the order served.
async fn listed_sources(emulator: &Emulator, workspace_id: &str) -> Vec<String> {
    let listed = emulator.list_sync_jobs(workspace_id, json!({})).await;
    assert_eq!(
        listed.status, 200,
        "the listing of {workspace_id:?} must succeed, got {listed:?}"
    );
    listed
        .member("syncJobSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["syncSource"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// How many sync jobs the whole store holds, read through the same seam a
/// handler reaches state by — so "nothing was removed" is checked against the
/// store and not only against what a read reports.
fn stored_job_count(emulator: &Emulator) -> usize {
    emulator
        .state()
        .lock()
        .region(REGION)
        .workspaces
        .values()
        .map(|workspace| workspace.sync_jobs.len())
        .sum()
}

/// The `state` member a delete answered with.
fn reported_state(resp: &Resp) -> Option<&str> {
    resp.member("state").as_str()
}

/// The table the removal suite is built from:
/// `(case, workspaceId, syncRole, tags, delete after the job settled)`.
///
/// One workspace per row, because a sync source identifies at most one job
/// inside a workspace. The rows differ along what the rule generalises over —
/// which partition the `syncRole` ARN names and how deep its role path runs (the
/// modelled `RoleArn` is `@length(20..2048)` and
/// `^arn:((aws)|(aws-cn)|(aws-us-gov)):iam::[0-9]{12}:role/`, from
/// `smithy_query(query="operation",operation="CreateSyncJob")`), whether the job
/// carries no `tags` member, an empty one or several entries, and whether the
/// delete arrives while the job is still in its non-terminal state or after the
/// clock has settled it — never along a mere rename.
fn rows() -> Vec<(&'static str, &'static str, String, Option<Value>, bool)> {
    vec![
        (
            "aws partition, no tags member, deleted while creating",
            "deleteSyncA",
            SYNC_ROLE.to_string(),
            None,
            false,
        ),
        (
            "aws-cn partition, empty tag map, deleted while creating",
            "deleteSyncB",
            "arn:aws-cn:iam::123456789012:role/service-role/plant/SyncRole".to_string(),
            Some(json!({})),
            false,
        ),
        (
            "aws-us-gov partition, one tag, deleted once settled",
            "deleteSyncC",
            "arn:aws-us-gov:iam::999999999999:role/s".to_string(),
            Some(json!({"team": "plant"})),
            true,
        ),
        (
            "long role path, several tags, deleted once settled",
            "deleteSyncD",
            format!("arn:aws:iam::000000000000:role/{}", "deep/".repeat(40)),
            Some(json!({"env": "test", "owner": "sync", "tier": "2"})),
            true,
        ),
    ]
}

/// RULE: a delete removes the sync job its labels address from the workspace
/// that holds it — whatever the job carries and whatever lifecycle state it is
/// in — reports the removal with the modelled `DELETING` state, and stops the
/// ARN the job was minted under resolving, so neither a read, nor a listing, nor
/// a read of that ARN's tags finds anything; the source is then free to create
/// again, and the workspace, holding nothing, is deletable.
#[tokio::test]
async fn deleting_a_sync_job_removes_it() {
    let workspaces: Vec<&str> = rows().iter().map(|row| row.1).collect();
    let emulator = emulator_with_workspaces(&workspaces).await;

    for (case, workspace_id, sync_role, tags, settle_first) in rows() {
        let mut request = json!({"syncRole": sync_role});
        if let Some(tags) = tags {
            request["tags"] = tags;
        }
        let created = emulator
            .create_sync_job(workspace_id, SYNC_SOURCE_SITEWISE, request)
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the job the delete removes must be created, got {created:?}"
        );
        if settle_first {
            wait_until_settled();
        }
        let before = emulator
            .get_sync_job(Some(workspace_id), SYNC_SOURCE_SITEWISE)
            .await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the job must be readable before the delete, got {before:?}"
        );

        let deleted = emulator
            .delete_sync_job(workspace_id, SYNC_SOURCE_SITEWISE)
            .await;
        assert_eq!(
            (deleted.status, deleted.error_type.as_deref()),
            (200, None),
            "case {case:?}: expected 200 from DeleteSyncJob, got {deleted:?}"
        );
        assert_eq!(
            reported_state(&deleted),
            Some(SYNC_JOB_STATE_DELETING),
            "case {case:?}: a delete reports the removal it performed, got {deleted:?}"
        );
        assert!(
            reported_state(&deleted).is_some_and(|state| SYNC_JOB_STATE_VALUES.contains(&state)),
            "case {case:?}: the reported state must be a modelled SyncJobState, got {deleted:?}"
        );

        let gone = emulator
            .get_sync_job(Some(workspace_id), SYNC_SOURCE_SITEWISE)
            .await;
        assert_eq!(
            (gone.status, gone.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: the job must be gone, got {gone:?}"
        );
        assert_eq!(
            listed_sources(&emulator, workspace_id).await,
            Vec::<String>::new(),
            "case {case:?}: the listing must report no job for the workspace"
        );
        let arn = sync_job_arn(workspace_id, SYNC_SOURCE_SITEWISE);
        let tags = emulator.list_tags_for_resource(&arn, json!({})).await;
        assert_eq!(
            (tags.status, tags.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: the deleted job's ARN must stop resolving, got {tags:?}"
        );
        // Nothing syncs into the workspace any more, so it links no service.
        let workspace = emulator.get_workspace(workspace_id).await;
        assert!(
            workspace.member("linkedServices").is_null(),
            "case {case:?}: a workspace holding no sync job links nothing, got {workspace:?}"
        );

        // The source is free again: the create that conflicted while the job was
        // held is a 200 now, and it mints the same ARN afresh.
        let recreated = emulator
            .create_sync_job(
                workspace_id,
                SYNC_SOURCE_SITEWISE,
                json!({"syncRole": SYNC_ROLE, "tags": {"round": "two"}}),
            )
            .await;
        assert_eq!(
            (recreated.status, recreated.member("arn").as_str()),
            (200, Some(arn.as_str())),
            "case {case:?}: a deleted source must be free to create again, got {recreated:?}"
        );
        let retagged = emulator.list_tags_for_resource(&arn, json!({})).await;
        assert_eq!(
            (retagged.status, retagged.member("tags")),
            (200, &json!({"round": "two"})),
            "case {case:?}: the new job's ARN resolves to its OWN tags, got {retagged:?}"
        );

        // Nothing a workspace contains may outlive it, so a workspace is
        // deletable only once the delete really removed the job it holds.
        let second = emulator
            .delete_sync_job(workspace_id, SYNC_SOURCE_SITEWISE)
            .await;
        assert_eq!(
            second.status, 200,
            "case {case:?}: the re-created job must be deletable too, got {second:?}"
        );
        let workspace_gone = emulator.delete_workspace(workspace_id).await;
        assert_eq!(
            workspace_gone.status, 200,
            "case {case:?}: the emptied workspace must be deletable, got {workspace_gone:?}"
        );
    }
}

/// RULE: both of this operation's labels address an EXISTING resource, so one
/// naming something the store does not hold is the modelled
/// `ResourceNotFoundException` (HTTP 404) — the same reading `GetSyncJob` gives
/// the same pair — the refusal carries the message this service words that miss
/// with, and it removes nothing.
///
/// The rows differ along WHICH label misses: the workspace, the source inside a
/// workspace that holds another job, a source held only in ANOTHER workspace, a
/// source a previous delete already removed, and sources the service does not
/// support at all (they satisfy the modelled `SyncSource` pattern
/// `^[a-zA-Z_0-9]+$`, so nothing but the store's contents refuses them).
#[tokio::test]
async fn deleting_what_the_store_does_not_hold_is_not_found() {
    const WORKSPACE: &str = "syncDeleteMiss";
    const OTHER_WORKSPACE: &str = "otherSyncDeleteMiss";
    let emulator = emulator_with_workspaces(&[WORKSPACE, OTHER_WORKSPACE]).await;

    // The workspace under test holds a job for a source NO row addresses, so a
    // refused delete has something to leave alone.
    seed_sync_job(&emulator, WORKSPACE, "KEPT_SRC");
    // A source held only in the other workspace, and one this workspace held
    // until it was deleted.
    seed_sync_job(&emulator, OTHER_WORKSPACE, "ELSEWHERE_SRC");
    seed_sync_job(&emulator, WORKSPACE, "ONCE_SRC");
    assert_eq!(
        emulator.delete_sync_job(WORKSPACE, "ONCE_SRC").await.status,
        200,
        "the first delete of the repeated-delete row must succeed"
    );

    // `(case, workspaceId, syncSource, the message the refusal carries)`: a
    // workspace this service does not hold is reported for the id the request
    // named, while a workspace that holds no such job reports the service's
    // fixed sentence for a missing sync job.
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "no-such-workspace",
            "absentSyncWorkspace",
            SYNC_SOURCE_SITEWISE,
            workspace_not_found_message("absentSyncWorkspace"),
        ),
        (
            "no-such-source",
            WORKSPACE,
            SYNC_SOURCE_SITEWISE,
            SYNC_JOB_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "foreign-workspace",
            WORKSPACE,
            "ELSEWHERE_SRC",
            SYNC_JOB_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "already-deleted",
            WORKSPACE,
            "ONCE_SRC",
            SYNC_JOB_NOT_FOUND_MESSAGE.to_string(),
        ),
        // Sources the service does not support: a different case of the
        // supported value, an unrelated service, and a value the supported one
        // is a prefix of.
        (
            "unsupported-lowercase",
            WORKSPACE,
            "sitewise",
            SYNC_JOB_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "unsupported-other-service",
            WORKSPACE,
            "KINESIS",
            SYNC_JOB_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "unsupported-prefixed",
            WORKSPACE,
            "SITEWISE2",
            SYNC_JOB_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];

    let held_before = stored_job_count(&emulator);
    for (case, workspace_id, sync_source, expected) in rows {
        let refused = emulator.delete_sync_job(workspace_id, sync_source).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException, got {refused:?}"
        );
        assert_eq!(
            refused.member("message").as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {refused:?}"
        );
        assert_eq!(
            stored_job_count(&emulator),
            held_before,
            "case {case:?}: a refused delete must remove nothing"
        );
    }

    // The jobs the refused deletes named in the other workspace, and the one
    // this workspace holds, are untouched.
    assert_eq!(
        listed_sources(&emulator, WORKSPACE).await,
        vec!["KEPT_SRC".to_string()],
        "a refused delete must leave the job it did not address alone"
    );
    assert_eq!(
        listed_sources(&emulator, OTHER_WORKSPACE).await,
        vec!["ELSEWHERE_SRC".to_string()],
        "a refused delete must leave another workspace's job alone"
    );
}

/// RULE: a delete touches exactly the one sync job its labels address — every
/// other job of the same workspace, and the same source in another workspace,
/// reads and lists back unchanged.
///
/// The rows differ along which job of the workspace is deleted (the first in
/// source order, a middle one, the last), because the rule generalises over the
/// position of the removal within the workspace's contents.
#[tokio::test]
async fn a_delete_touches_only_the_job_it_addresses() {
    const WORKSPACE: &str = "syncDeleteIsolation";
    const OTHER_WORKSPACE: &str = "syncDeleteNeighbour";
    let emulator = emulator_with_workspaces(&[WORKSPACE, OTHER_WORKSPACE]).await;

    // `sync_jobs` is keyed by source, so the listing serves these in source
    // order: the rows below delete the first, the middle and the last of them.
    let held = ["A_SRC", "M_SRC", "Z_SRC"];
    for target in held {
        for sync_source in held {
            seed_sync_job(&emulator, WORKSPACE, sync_source);
            seed_sync_job(&emulator, OTHER_WORKSPACE, sync_source);
        }

        let deleted = emulator.delete_sync_job(WORKSPACE, target).await;
        assert_eq!(
            (deleted.status, reported_state(&deleted)),
            (200, Some(SYNC_JOB_STATE_DELETING)),
            "case {target:?}: the delete must succeed and report DELETING, got {deleted:?}"
        );

        let expected: Vec<String> = held
            .iter()
            .filter(|source| **source != target)
            .map(|source| (*source).to_string())
            .collect();
        assert_eq!(
            listed_sources(&emulator, WORKSPACE).await,
            expected,
            "case {target:?}: exactly the addressed job must be gone from the workspace"
        );
        for sync_source in held.iter().filter(|source| **source != target) {
            let sibling = emulator.get_sync_job(Some(WORKSPACE), sync_source).await;
            assert_eq!(
                (sibling.status, sibling.member("syncRole").as_str()),
                (200, Some(SYNC_ROLE)),
                "case {target:?}: sibling {sync_source:?} must read back unchanged, \
                 got {sibling:?}"
            );
        }
        // A sync job belongs to the workspace it was created in, so the same
        // source in another workspace is a different job and survives.
        assert_eq!(
            listed_sources(&emulator, OTHER_WORKSPACE).await,
            held.iter().map(|source| (*source).to_string()).collect::<Vec<String>>(),
            "case {target:?}: another workspace's jobs must be untouched"
        );

        // Leave both workspaces empty for the next row.
        for sync_source in held {
            for workspace_id in [WORKSPACE, OTHER_WORKSPACE] {
                let resp = emulator.delete_sync_job(workspace_id, sync_source).await;
                assert!(
                    resp.status == 200 || resp.status == 404,
                    "case {target:?}: clearing {workspace_id:?}/{sync_source:?} must either \
                     remove it or find it already gone, got {resp:?}"
                );
            }
        }
        assert_eq!(
            stored_job_count(&emulator),
            0,
            "case {target:?}: every seeded job must be removable"
        );
    }
}
