// SPDX-License-Identifier: CC0-1.0

//! The refusals this service words as ONE fixed sentence, end to end over HTTP.
//!
//! Most of this service's refusals are built from the request — they name the
//! workspace, the entity or the member that could not be used. Two are not: the
//! refusal of a role this service cannot assume, and the report of a sync job a
//! workspace does not hold. Each of those is a single sentence the service
//! interpolates nothing into, so the text a client matches on is the same
//! whichever operation reported it and whatever the request named — and in
//! particular the caller's role ARN never reaches one.
//!
//! `tests/all/workspace_role_trust.rs` and `tests/all/sync_job_role_trust.rs`
//! cover WHETHER a role is refused and what the refusal leaves behind;
//! `tests/all/sync_job_delete.rs` and `tests/all/sync_resource_listing.rs` cover
//! what each reader of a sync job does. This suite covers what those refusals
//! SAY.
//!
//! For the role refusal, the rows differ along everything the request can vary:
//! the member (the workspace's `role` on the create and on the patch, a sync
//! job's `syncRole`), the role name — hence the ARN — at each bound IAM's
//! `CreateRole$RoleName` publishes (length 1..64, pattern `^[\w+=,.@-]+$`, from
//! `smithy_query(query="operation",operation="CreateRole")` on the IAM model) and
//! the form in which the trust document withholds `sts:AssumeRole`. For the
//! missing sync job, they differ along the sync source the request named and the
//! operation that resolved it.

use serde_json::json;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    SERVICE_PRINCIPAL, SYNC_JOB_NOT_FOUND_MESSAGE, SYNC_SOURCE_SITEWISE, UNASSUMABLE_ROLE_MESSAGE,
};

use crate::harness::{
    admitting_trust_policy, seed_sync_job, trust_policy, Emulator, Resp, SYNC_ROLE,
};

/// The action a trust policy has to grant this service for it to act as the
/// role; named here only to assert the refusal does NOT quote it.
const ASSUME_ROLE_ACTION: &str = "sts:AssumeRole";

/// The roles the refusal is driven with: `(case, roleName, trust document)`,
/// every one of them a document that withholds the grant.
///
/// The role NAMES span IAM's published `RoleName` bounds — one character, the
/// 64-character maximum, and a name using every punctuation character the
/// pattern admits — because the name is what the role's ARN carries, and the ARN
/// is the value the refusal must not quote.
fn withholding_rows() -> Vec<(&'static str, String, String)> {
    let principal = format!("\"{SERVICE_PRINCIPAL}\"");
    vec![
        (
            "another service's principal only, under the shortest role name",
            "r".to_string(),
            trust_policy("Allow", "\"sts:AssumeRole\"", "\"ec2.amazonaws.com\""),
        ),
        (
            "an explicit deny, under the longest role name",
            "d".repeat(64),
            trust_policy("Deny", "\"sts:AssumeRole\"", &principal),
        ),
        (
            "an action that is not assuming the role",
            "twinmaker+role=name,with.every@admitted-char".to_string(),
            trust_policy("Allow", "\"s3:GetObject\"", &principal),
        ),
        (
            "a statement list granting nothing",
            "twinmaker-empty-statements".to_string(),
            "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
        ),
        (
            "a principal that only looks like this service's",
            "twinmaker-lookalike-principal".to_string(),
            trust_policy(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"iottwinmaker.amazonaws.com.example.test\"",
            ),
        ),
    ]
}

/// One refusal, asserted against the rule: the modelled `ValidationException`
/// (HTTP 400) carrying the service's fixed sentence and nothing of the request.
fn assert_fixed_sentence(case: &str, resp: &Resp, role_arn: &str) {
    assert_eq!(
        (resp.status, resp.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected a 400 ValidationException, got {resp:?}"
    );
    let message = resp.member("message").as_str().unwrap_or_default();
    assert_eq!(
        message, UNASSUMABLE_ROLE_MESSAGE,
        "case {case:?}: expected {UNASSUMABLE_ROLE_MESSAGE:?}, got {message:?}"
    );
    // Stated separately from the equality above so a wording change that starts
    // interpolating again says WHICH value leaked.
    for (what, value) in [
        ("the role ARN", role_arn),
        ("this service's principal", SERVICE_PRINCIPAL),
        ("the withheld action", ASSUME_ROLE_ACTION),
    ] {
        assert!(
            !message.contains(value),
            "case {case:?}: the refusal must not quote {what} ({value:?}), got {message:?}"
        );
    }
}

/// RULE: every member of this service that names a role refuses a role it cannot
/// assume with one fixed sentence, whatever role the request named and whatever
/// form withheld the grant; a role the service CAN assume is served and reads
/// back.
#[tokio::test]
async fn every_unassumable_role_refusal_carries_the_same_fixed_sentence() {
    let emulator = Emulator::with_iam();
    // The workspace the patch and the sync-job create act on. It is created
    // without a role, so the rows below are the only roles this service is asked
    // to assume.
    let workspace_id = "fixedSentenceWs";
    assert_eq!(
        emulator.create_workspace(workspace_id).await.status,
        200,
        "the workspace the patch and the sync-job create name must exist"
    );

    for (index, (case, role_name, document)) in withholding_rows().into_iter().enumerate() {
        let role_arn = emulator.create_iam_role(&role_name, &document).await;

        // One member per call, all three naming the SAME role: the workspace's
        // `role` on the create (into an id of its own, which the refusal leaves
        // free), the workspace's `role` on the patch, and a sync job's
        // `syncRole`.
        let created_workspace = emulator
            .create_workspace_with(
                &format!("fixedSentenceWs{index}"),
                json!({"role": role_arn, "s3Location": "arn:aws:s3:::twinmaker-fixed-sentence"}),
            )
            .await;
        let patched_workspace = emulator
            .update_workspace(workspace_id, json!({"role": role_arn}))
            .await;
        let created_job = emulator
            .create_sync_job(
                workspace_id,
                SYNC_SOURCE_SITEWISE,
                json!({"syncRole": role_arn}),
            )
            .await;

        for (member, resp) in [
            ("CreateWorkspace role", &created_workspace),
            ("UpdateWorkspace role", &patched_workspace),
            ("CreateSyncJob syncRole", &created_job),
        ] {
            assert_fixed_sentence(&format!("{case} via {member}"), resp, &role_arn);
        }

        // The refused create persisted no workspace, so a row's refusals leave
        // the service exactly as the previous row did.
        let read = emulator
            .get_workspace(&format!("fixedSentenceWs{index}"))
            .await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: a refused create must persist no workspace, got {read:?}"
        );
    }

    // The same three members, with a role this service CAN assume: the rows
    // above refuse because of the trust document, not because every role is
    // refused, and what each served call stored reads back.
    let assumable = emulator
        .create_iam_role("twinmaker-fixed-sentence", &admitting_trust_policy())
        .await;
    let patched = emulator
        .update_workspace(workspace_id, json!({"role": assumable}))
        .await;
    assert_eq!(
        patched.status, 200,
        "case \"an assumable role via UpdateWorkspace role\": expected the patch to be served, \
         got {patched:?}"
    );
    let created_job = emulator
        .create_sync_job(
            workspace_id,
            SYNC_SOURCE_SITEWISE,
            json!({"syncRole": assumable}),
        )
        .await;
    assert_eq!(
        created_job.status, 200,
        "case \"an assumable role via CreateSyncJob syncRole\": expected the create to be served, \
         got {created_job:?}"
    );
    let created_workspace = emulator
        .create_workspace_with("fixedSentenceServedWs", json!({"role": assumable}))
        .await;
    assert_eq!(
        created_workspace.status, 200,
        "case \"an assumable role via CreateWorkspace role\": expected the create to be served, \
         got {created_workspace:?}"
    );

    for (case, read, member, expected) in [
        (
            "an assumable role via UpdateWorkspace role",
            emulator.get_workspace(workspace_id).await,
            "role",
            &assumable,
        ),
        (
            "an assumable role via CreateWorkspace role",
            emulator.get_workspace("fixedSentenceServedWs").await,
            "role",
            &assumable,
        ),
        (
            "an assumable role via CreateSyncJob syncRole",
            emulator
                .get_sync_job(Some(workspace_id), SYNC_SOURCE_SITEWISE)
                .await,
            "syncRole",
            &assumable,
        ),
    ] {
        assert_eq!(
            read.member(member).as_str(),
            Some(expected.as_str()),
            "case {case:?}: the served call must store the role it named, got {read:?}"
        );
    }
}

/// RULE: every operation that resolves a sync job reports one the workspace does
/// not hold with the service's fixed sentence for that miss, whatever workspace
/// and sync source the request named.
///
/// The rows are sync sources the workspace holds no job for, differing along
/// what the modelled `SyncSource` pattern `^[a-zA-Z_0-9]+$` admits: the one
/// source the create path supports but nobody created, another casing of it, a
/// source belonging to another service, and a value the supported source is a
/// prefix of.
#[tokio::test]
async fn every_reader_reports_a_missing_sync_job_with_the_same_fixed_sentence() {
    let emulator = Emulator::new();
    let workspace_id = "missingJobWs";
    assert_eq!(
        emulator.create_workspace(workspace_id).await.status,
        200,
        "the workspace the rows address must exist, so each miss is the job's"
    );
    // A job for a source NO row addresses: the workspace holds a sync job, so a
    // row misses on the SOURCE rather than on an empty workspace.
    seed_sync_job(&emulator, workspace_id, "KEPT_SRC");

    for (case, sync_source) in [
        ("the supported source, never created", SYNC_SOURCE_SITEWISE),
        ("another casing of the supported source", "sitewise"),
        ("a source of another service", "KINESIS"),
        ("a source the supported one is a prefix of", "SITEWISE2"),
    ] {
        // Every operation that resolves a sync job: the read, the listing of
        // what it synchronised, and the delete. The read and the delete model
        // `ResourceNotFoundException`; the listing models none, so it reports
        // the miss as the `ValidationException` it does model.
        let read = emulator.get_sync_job(Some(workspace_id), sync_source).await;
        let listed = emulator
            .list_sync_resources(workspace_id, sync_source, json!({}))
            .await;
        let deleted = emulator.delete_sync_job(workspace_id, sync_source).await;

        for (reader, resp, expected_status, expected_code) in [
            ("GetSyncJob", &read, 404, "ResourceNotFoundException"),
            ("ListSyncResources", &listed, 400, "ValidationException"),
            ("DeleteSyncJob", &deleted, 404, "ResourceNotFoundException"),
        ] {
            let case = format!("{case} via {reader}");
            assert_eq!(
                (resp.status, resp.error_type.as_deref()),
                (expected_status, Some(expected_code)),
                "case {case:?}: expected {expected_status} {expected_code}, got {resp:?}"
            );
            let message = resp.member("message").as_str().unwrap_or_default();
            assert_eq!(
                message, SYNC_JOB_NOT_FOUND_MESSAGE,
                "case {case:?}: expected {SYNC_JOB_NOT_FOUND_MESSAGE:?}, got {message:?}"
            );
            for (what, value) in [
                ("the workspace id", workspace_id),
                ("the sync source", sync_source),
            ] {
                assert!(
                    !message.contains(value),
                    "case {case:?}: the refusal must not quote {what} ({value:?}), got {message:?}"
                );
            }
        }
    }

    // The job the workspace DOES hold is still read and listed, so the rows
    // above miss on what the store holds rather than on every call failing —
    // and a refused delete removed none of it.
    let held = emulator.get_sync_job(Some(workspace_id), "KEPT_SRC").await;
    assert_eq!(
        (held.status, held.member("syncRole").as_str()),
        (200, Some(SYNC_ROLE)),
        "case \"the job the workspace holds\": expected it to read back, got {held:?}"
    );
    let listed = emulator
        .list_sync_resources(workspace_id, "KEPT_SRC", json!({}))
        .await;
    assert_eq!(
        listed.status, 200,
        "case \"the job the workspace holds\": expected its resources to be listed, got {listed:?}"
    );
}
