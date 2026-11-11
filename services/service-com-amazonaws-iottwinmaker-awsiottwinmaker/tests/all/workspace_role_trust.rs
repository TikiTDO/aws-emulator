// SPDX-License-Identifier: CC0-1.0

//! A workspace carries an execution role only when this service can act as that
//! role, end to end over HTTP.
//!
//! `tests/all/workspace_update.rs` and `tests/all/workspace_update_scope.rs`
//! cover what a workspace's members ARE and how a patch moves them. This suite
//! covers the rule that decides whether a `role` may be stored at all: the
//! service acts AS a workspace's execution role, so a request naming a role of
//! this account whose trust policy withholds `sts:AssumeRole` from
//! `iottwinmaker.amazonaws.com` is refused SYNCHRONOUSLY — the modelled
//! `ValidationException` (HTTP 400) — and nothing is stored: the create persists
//! no workspace, and the update leaves the workspace with the role it had.
//!
//! It is the SAME rule the sync-job create applies to its own role member
//! (`tests/all/sync_job_role_trust.rs`), so both suites drive the SAME table of
//! trust forms (`harness::trust_policy_rows`) and the last test drives one role
//! through every member of this service that names one, to pin that they decide
//! alike.

use serde_json::json;
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    workspace_not_found_message, SYNC_SOURCE_SITEWISE, UNASSUMABLE_ROLE_MESSAGE,
};

use crate::harness::{
    admitting_trust_policy, trust_policy, trust_policy_rows, trusted_role_arn, Emulator,
};

/// The refusal a role this service cannot assume draws, as every member's
/// message words it: ONE fixed sentence, with nothing of the request in it.
fn assert_refusal(case: &str, resp: &crate::harness::Resp) {
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
}

/// RULE: a create whose `role` names a role of this account that this service
/// cannot assume is refused 400 `ValidationException` and stores no workspace;
/// any other role is accepted and reads back as the workspace's role.
#[tokio::test]
async fn a_workspace_role_this_service_cannot_assume_is_refused_and_persists_nothing() {
    let emulator = Emulator::with_iam();

    for (index, (case, document, admitted)) in trust_policy_rows().into_iter().enumerate() {
        // One workspace id per row: a refused create must leave the id free, and
        // an accepted one takes it.
        let workspace_id = format!("roleTrustWs{index}");
        let role = trusted_role_arn(
            &emulator,
            &format!("twinmaker-workspace-{index}"),
            document.as_ref(),
        )
        .await;

        let created = emulator
            .create_workspace_with(
                &workspace_id,
                json!({"role": role, "s3Location": "arn:aws:s3:::twinmaker-role-trust"}),
            )
            .await;
        let read = emulator.get_workspace(&workspace_id).await;

        if admitted {
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the create to be served, got {created:?}"
            );
            assert_eq!(
                read.status, 200,
                "case {case:?}: the created workspace must read back, got {read:?}"
            );
            assert_eq!(
                read.member("role").as_str(),
                Some(role.as_str()),
                "case {case:?}: the workspace must carry the role the create named, got {read:?}"
            );
        } else {
            assert_refusal(case, &created);
            // Nothing was persisted: the read behind the refused create is the
            // service's own not-found refusal for a workspace.
            assert_eq!(
                (read.status, read.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: a refused create must persist no workspace, got {read:?}"
            );
            assert_eq!(
                read.member("message").as_str(),
                Some(workspace_not_found_message(&workspace_id).as_str()),
                "case {case:?}: expected the workspace not-found construction, got {read:?}"
            );
        }
    }
}

/// RULE (the same one on the patch path): an update whose `role` names a role
/// this service cannot assume is refused 400 `ValidationException` and changes
/// nothing — not the role, not the members named beside it.
#[tokio::test]
async fn an_update_naming_a_role_this_service_cannot_assume_changes_nothing() {
    let emulator = Emulator::with_iam();
    let stored_role = emulator
        .create_iam_role("twinmaker-workspace-stored", &admitting_trust_policy())
        .await;
    let workspace_id = "patchedRoleWs";
    let created = emulator
        .create_workspace_with(
            workspace_id,
            json!({"description": "created", "role": stored_role}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the workspace the patches apply to must be created, got {created:?}"
    );

    for (index, (case, document, admitted)) in trust_policy_rows().into_iter().enumerate() {
        let role = trusted_role_arn(
            &emulator,
            &format!("twinmaker-patch-{index}"),
            document.as_ref(),
        )
        .await;
        let updated = emulator
            .update_workspace(
                workspace_id,
                json!({"role": role, "description": format!("patched by {case}")}),
            )
            .await;
        let read = emulator.get_workspace(workspace_id).await;

        if admitted {
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected the patch to be served, got {updated:?}"
            );
            assert_eq!(
                read.member("role").as_str(),
                Some(role.as_str()),
                "case {case:?}: the patch must store the role it named, got {read:?}"
            );
            assert_eq!(
                read.member("description").as_str(),
                Some(format!("patched by {case}").as_str()),
                "case {case:?}: the patch must store the description it named, got {read:?}"
            );
            // The role this row stored is the one the next refused row must be
            // asserted to have left alone.
            assert_eq!(
                emulator
                    .update_workspace(workspace_id, json!({"role": stored_role}))
                    .await
                    .status,
                200,
                "case {case:?}: restoring the stored role must be served"
            );
        } else {
            assert_refusal(case, &updated);
            assert_eq!(
                read.member("role").as_str(),
                Some(stored_role.as_str()),
                "case {case:?}: a refused patch must leave the stored role, got {read:?}"
            );
            assert_ne!(
                read.member("description").as_str(),
                Some(format!("patched by {case}").as_str()),
                "case {case:?}: a refused patch must store none of its members, got {read:?}"
            );
        }
    }
}

/// RULE (from the other side): the rule is about the ROLE, not about the member
/// that named it, so one role of this account is decided the same way by every
/// member of this service that names a role — the workspace create, the
/// workspace patch and the sync-job create.
#[tokio::test]
async fn every_member_naming_a_role_decides_alike() {
    let emulator = Emulator::with_iam();
    let refused_role = emulator
        .create_iam_role(
            "twinmaker-ec2-only",
            &trust_policy("Allow", "\"sts:AssumeRole\"", "\"ec2.amazonaws.com\""),
        )
        .await;
    let accepted_role = emulator
        .create_iam_role("twinmaker-execution", &admitting_trust_policy())
        .await;

    // Every member of this service that names a role, decided against the SAME
    // role: the workspace create, the workspace patch and the sync-job create.
    //
    // The role is read before the resource the call names is resolved, so a
    // patch of a workspace this Region does not hold is refused for the ROLE
    // rather than reported as missing — the same order the sync-job create
    // applies, where the role check also precedes the workspace lookup.
    let refused_before_lookup = emulator
        .update_workspace("noSuchWorkspace", json!({"role": refused_role}))
        .await;
    assert_refusal(
        "UpdateWorkspace role on a workspace that does not exist",
        &refused_before_lookup,
    );

    let refused_create = emulator
        .create_workspace_with("sharedRuleWs", json!({"role": refused_role}))
        .await;
    assert_refusal("CreateWorkspace role", &refused_create);

    // The refused create left the id free, so the same id is created with the
    // assumable role: the two calls differ only in the role they name.
    let created = emulator
        .create_workspace_with("sharedRuleWs", json!({"role": accepted_role}))
        .await;
    assert_eq!(
        created.status, 200,
        "case \"CreateWorkspace role\": expected the create to be served, got {created:?}"
    );

    let refused_patch = emulator
        .update_workspace("sharedRuleWs", json!({"role": refused_role}))
        .await;
    assert_refusal("UpdateWorkspace role", &refused_patch);

    let refused_job = emulator
        .create_sync_job(
            "sharedRuleWs",
            SYNC_SOURCE_SITEWISE,
            json!({"syncRole": refused_role}),
        )
        .await;
    assert_refusal("CreateSyncJob syncRole", &refused_job);
    let accepted_job = emulator
        .create_sync_job(
            "sharedRuleWs",
            SYNC_SOURCE_SITEWISE,
            json!({"syncRole": accepted_role}),
        )
        .await;
    assert_eq!(
        accepted_job.status, 200,
        "case \"CreateSyncJob syncRole\": expected the create to be served, got {accepted_job:?}"
    );

    // The workspace reports the role every served call left it with, read back
    // through `GetWorkspace`.
    let read = emulator.get_workspace("sharedRuleWs").await;
    assert_eq!(
        read.member("role").as_str(),
        Some(accepted_role.as_str()),
        "case \"GetWorkspace role\": the workspace must carry the assumable role, got {read:?}"
    );
}
