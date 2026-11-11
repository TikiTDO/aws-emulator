// SPDX-License-Identifier: CC0-1.0

//! A sync job is created only when this service can act as the role the request
//! names, end to end over HTTP.
//!
//! `tests/all/sync_job.rs` covers what an accepted job IS — the record, the
//! clock-owed outcome, the listing, the conflict and the unsupported source.
//! This suite covers the rule that decides whether a create is accepted at all:
//! a sync job reads its sync source and writes the workspace AS the role
//! `syncRole` names, so a create naming a role of this account whose trust
//! policy withholds `sts:AssumeRole` from `iottwinmaker.amazonaws.com` is
//! refused SYNCHRONOUSLY — the modelled `ValidationException` (HTTP 400), not an
//! accepted job left to fail later — and persists nothing, so the `GetSyncJob`
//! behind it is a `ResourceNotFoundException`.
//!
//! The role is resolved against the account's IAM records, the very records
//! `CreateRole` wrote in the same engine, and the rows are the shared table of
//! trust forms every suite about this rule drives
//! (`harness::trust_policy_rows`), because ONE rule decides every member of
//! this service that names a role (`tests/all/workspace_role_trust.rs` drives
//! the same table through the workspace's `role`).

use serde_json::json;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::SYNC_JOB_STATE_VALUES;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    SYNC_JOB_NOT_FOUND_MESSAGE, SYNC_SOURCE_SITEWISE, UNASSUMABLE_ROLE_MESSAGE,
};

use crate::harness::{
    admitting_trust_policy, trust_policy, trust_policy_rows, trusted_role_arn, Emulator,
};

/// RULE: a create whose `syncRole` names a role of this account that cannot be
/// assumed by this service is refused 400 `ValidationException` and stores
/// nothing; any other role is accepted and the job reads back in a modelled
/// `SyncJobState`.
#[tokio::test]
async fn a_sync_role_this_service_cannot_assume_is_refused_and_persists_nothing() {
    let emulator = Emulator::with_iam();

    for (index, (case, document, admitted)) in trust_policy_rows().into_iter().enumerate() {
        // One workspace per row: a sync source identifies at most one job inside
        // a workspace, and the recorded refusal is about the create, so each row
        // needs a workspace of its own to create into.
        let workspace_id = format!("syncTrustWs{index}");
        assert_eq!(
            emulator.create_workspace(&workspace_id).await.status,
            200,
            "case {case:?}: the workspace the row creates into must exist"
        );
        let role_arn = trusted_role_arn(
            &emulator,
            &format!("twinmaker-sync-{index}"),
            document.as_ref(),
        )
        .await;

        let created = emulator
            .create_sync_job(
                &workspace_id,
                SYNC_SOURCE_SITEWISE,
                json!({"syncRole": role_arn}),
            )
            .await;
        let read = emulator
            .get_sync_job(Some(&workspace_id), SYNC_SOURCE_SITEWISE)
            .await;

        if admitted {
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the create to be served, got {created:?}"
            );
            assert_eq!(
                read.status, 200,
                "case {case:?}: the created job must read back, got {read:?}"
            );
            let state = read.member("status")["state"].as_str().unwrap_or_default();
            assert!(
                SYNC_JOB_STATE_VALUES.contains(&state),
                "case {case:?}: expected a modelled SyncJobState, got {state:?} in {read:?}"
            );
            assert_eq!(
                read.member("syncRole").as_str(),
                Some(role_arn.as_str()),
                "case {case:?}: the job must carry the role the create named, got {read:?}"
            );
        } else {
            assert_eq!(
                (created.status, created.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: expected a 400 ValidationException, got {created:?}"
            );
            // The refusal carries the service's own fixed sentence, whatever
            // role the request named and whatever form withheld the grant.
            let message = created.member("message").as_str().unwrap_or_default();
            assert_eq!(
                message, UNASSUMABLE_ROLE_MESSAGE,
                "case {case:?}: expected {UNASSUMABLE_ROLE_MESSAGE:?}, got {message:?}"
            );
            // Nothing was persisted: the read behind the refused create is the
            // service's own not-found refusal for a sync job, and the workspace
            // reports no linked service either.
            assert_eq!(
                (read.status, read.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: a refused create must persist no job, got {read:?}"
            );
            assert_eq!(
                read.member("message").as_str(),
                Some(SYNC_JOB_NOT_FOUND_MESSAGE),
                "case {case:?}: expected {SYNC_JOB_NOT_FOUND_MESSAGE:?}, got {read:?}"
            );
            let workspace = emulator.get_workspace(&workspace_id).await;
            assert!(
                workspace.member("linkedServices").is_null(),
                "case {case:?}: a refused create must link no service, got {workspace:?}"
            );
        }
    }
}

/// RULE (the same one, from the other side): the role is read from the IAM
/// records as they stand, so the SAME ARN is refused or accepted according to
/// the trust policy the account's role carries — not according to anything the
/// request spells.
#[tokio::test]
async fn the_verdict_follows_the_stored_trust_policy_not_the_arn() {
    let emulator = Emulator::with_iam();
    let refused_role = emulator
        .create_iam_role(
            "twinmaker-ec2-only",
            &trust_policy("Allow", "\"sts:AssumeRole\"", "\"ec2.amazonaws.com\""),
        )
        .await;
    let accepted_role = emulator
        .create_iam_role("twinmaker-sync", &admitting_trust_policy())
        .await;
    assert_eq!(
        emulator.create_workspace("syncTrustWs").await.status,
        200,
        "the workspace both creates name must exist"
    );

    // The refused create leaves the source free, so the second create — which
    // differs only in the role it names — is not a conflict but the accepted
    // case.
    let refused = emulator
        .create_sync_job(
            "syncTrustWs",
            SYNC_SOURCE_SITEWISE,
            json!({"syncRole": refused_role, "tags": {"team": "plant"}}),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case \"unassumable role\": expected a 400 ValidationException, got {refused:?}"
    );
    let accepted = emulator
        .create_sync_job(
            "syncTrustWs",
            SYNC_SOURCE_SITEWISE,
            json!({"syncRole": accepted_role, "tags": {"team": "plant"}}),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"assumable role\": expected the create to be served, got {accepted:?}"
    );
    let read = emulator
        .get_sync_job(Some("syncTrustWs"), SYNC_SOURCE_SITEWISE)
        .await;
    assert_eq!(
        read.member("syncRole").as_str(),
        Some(accepted_role.as_str()),
        "case \"assumable role\": the stored job must carry the accepted role, got {read:?}"
    );
    assert_eq!(
        read.member("arn").as_str(),
        accepted.member("arn").as_str(),
        "case \"assumable role\": the read must report the ARN the create minted, got {read:?}"
    );
}
