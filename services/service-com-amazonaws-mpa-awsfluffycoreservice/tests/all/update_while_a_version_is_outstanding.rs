// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `UpdateApprovalTeam` does while the team already has a
//! version it is moving to.
//!
//! The rule under test: the one-change-at-a-time handshake protects the CONFIGURATION the
//! team's approvers approved, so a request that NAMES a configuration member
//! (`Description`, `ApprovalStrategy`, `Approvers`) while a version awaits that approval
//! is refused `ConflictException` (409) — it would ask them to approve two — while a
//! request that names none asks them nothing and is served: what it does ask for takes
//! effect where it was asked for, and the version the team is waiting on keeps its own
//! configuration, version id and status. The one modelled update action is such a request:
//! it reads each approver's MFA devices back off the identity source, and a device belongs
//! to the approver's identity, so every record the team holds — its live approvers and the
//! approvers of the version it is waiting on — is reported `IN_SYNC` afterwards.
//!
//! The tables span that class along what the rule generalises over: whether the request
//! names a configuration member and WHICH one, the form of the `UpdateActions` list (the
//! token, the token repeated, an empty list, no member at all), how far the outstanding
//! version has got (waiting on the approvers, waiting on the activation they approved) and
//! how many approvers the team has (1, 2 and the `Approvers` `length(1..20)` maximum).
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI label, the JSON envelope
//! and the HTTP status are all exercised.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, message, MAX_APPROVERS};
use crate::approval_team_lifecycle::{error_code, text, Harness};
use crate::pending_team_update::{
    approver_member, make_activation_due, make_approval_due, postpone_approval,
    with_pending_update,
};

/// The `Description` the outstanding version carries, so a read can tell the configuration
/// the team runs on from the one it is waiting on.
const PENDING_DESCRIPTION: &str = "the version the team is waiting on";

/// How far the version the team is moving to has got. Both are OUTSTANDING — the team is
/// still moving to that version — so both refuse a second configuration change.
#[derive(Clone, Copy, Debug)]
enum Stage {
    /// Waiting on the team's approval (`UPDATE_PENDING_APPROVAL`).
    AwaitingApproval,
    /// Approved, waiting on the activation (`UPDATE_PENDING_ACTIVATION`).
    AwaitingActivation,
}

impl Stage {
    /// The status code `GetApprovalTeam` reports for a team at this stage.
    fn status_code(self) -> &'static str {
        match self {
            Self::AwaitingApproval => state::TeamStatusCode::UpdatePendingApproval.as_wire(),
            Self::AwaitingActivation => state::TeamStatusCode::UpdatePendingActivation.as_wire(),
        }
    }
}

/// Push the instant an approved change's activation comes due well past the end of a test,
/// so the version stays outstanding for as long as the test needs it to. Only the DUE
/// INSTANT is moved, exactly as `pending_team_update.rs` moves the approval's.
fn postpone_activation(harness: &Harness, team_arn: &str) {
    with_pending_update(harness, team_arn, |update| {
        update.activation_due = Some(SystemTime::now() + Duration::from_secs(3600));
    });
}

/// A team with a version outstanding at `stage`: its ARN, the live version id and
/// description it keeps, and the version id the outstanding change carries.
struct Outstanding {
    arn: String,
    live_version: String,
    live_description: String,
    requested_version: String,
}

async fn team_with_an_outstanding_version(
    harness: &Harness,
    name: &str,
    approvers: usize,
    stage: Stage,
) -> Outstanding {
    let arn = active_team(harness, name, approvers, 1).await;
    let (_, before) = harness.get(&arn).await;
    let live_version = text(&before, "VersionId");
    let live_description = text(&before, "Description");

    let (status, requested) = harness
        .update(&arn, &json!({ "Description": PENDING_DESCRIPTION }))
        .await;
    assert_eq!(
        status, 200,
        "case {name}: the first change must be recorded, got {requested:?}"
    );
    let requested_version = text(&requested, "VersionId");
    // The approvers' answer stays owed in the future, so the version waits.
    postpone_approval(harness, &arn);

    if matches!(stage, Stage::AwaitingActivation) {
        make_approval_due(harness, &arn);
        let (status, approved) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {name}: get returned {approved:?}");
        postpone_activation(harness, &arn);
    }

    let (status, waiting) = harness.get(&arn).await;
    assert_eq!(status, 200, "case {name}: get returned {waiting:?}");
    assert_eq!(
        text(&waiting, "StatusCode"),
        stage.status_code(),
        "case {name}: expected a team at {stage:?}, got {waiting:?}"
    );
    assert_eq!(
        text(&waiting["PendingUpdate"], "VersionId"),
        requested_version,
        "case {name}: the team must be waiting on the version the change answered with: \
         {waiting:?}"
    );

    Outstanding {
        arn,
        live_version,
        live_description,
        requested_version,
    }
}

/// The `SyncStatus` of every device of every approver one read reports, in response order.
/// `from` names the member the approver list hangs off: `None` is the team's live set,
/// `Some("PendingUpdate")` the set of the version it is waiting on.
pub(crate) fn sync_statuses(team: &Value, from: Option<&str>) -> Vec<Vec<String>> {
    let holder = match from {
        Some(member) => &team[member],
        None => team,
    };
    holder["Approvers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|approver| {
            approver["MfaMethods"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|method| text(method, "SyncStatus"))
                .collect()
        })
        .collect()
}

/// Assert every device of every approver in one reported set carries `expected`, and that
/// the set has `approvers` entries.
pub(crate) fn assert_every_device(
    label: &str,
    set: &str,
    statuses: &[Vec<String>],
    expected: &str,
    approvers: usize,
) {
    assert_eq!(
        statuses.len(),
        approvers,
        "case {label}: expected {approvers} approver entries in the {set} set, got {statuses:?}"
    );
    for (index, devices) in statuses.iter().enumerate() {
        assert_eq!(
            devices.as_slice(),
            [expected.to_string()],
            "case {label}: expected the {set} approver {index} to carry one {expected} device, \
             got {devices:?}"
        );
    }
}

/// One row of the served table: a patch that names NO configuration member.
struct ServedCase {
    label: &'static str,
    patch: Value,
    /// Whether the row asks for the modelled update action, and so for every device to be
    /// read back off the identity source.
    synchronizes: bool,
}

/// `UpdateActions` is `length(min: 0, max: 100)` and `UpdateAction` declares the single
/// token `SYNCHRONIZE_MFA_DEVICES`, so these are the forms a request that names no
/// configuration member can take.
fn served_cases() -> Vec<ServedCase> {
    vec![
        ServedCase {
            label: "the action alone",
            patch: json!({ "UpdateActions": [state::SYNCHRONIZE_MFA_DEVICES_ACTION] }),
            synchronizes: true,
        },
        ServedCase {
            label: "the action named twice",
            patch: json!({
                "UpdateActions": [
                    state::SYNCHRONIZE_MFA_DEVICES_ACTION,
                    state::SYNCHRONIZE_MFA_DEVICES_ACTION,
                ],
            }),
            synchronizes: true,
        },
        ServedCase {
            label: "an empty UpdateActions",
            patch: json!({ "UpdateActions": [] }),
            synchronizes: false,
        },
        ServedCase {
            label: "no member at all",
            patch: json!({}),
            synchronizes: false,
        },
    ]
}

/// A request that names no configuration member is SERVED while the team has a version
/// outstanding: the action it asks for is applied where it was asked for, to every approver
/// record the team holds, and the outstanding version keeps its configuration, its version
/// id and its status.
#[tokio::test]
async fn a_request_that_changes_no_configuration_member_is_served_while_a_version_waits() {
    let in_sync = state::MfaSyncStatus::InSync.as_wire().to_string();
    let out_of_sync = state::MfaSyncStatus::OutOfSync.as_wire().to_string();

    for stage in [Stage::AwaitingApproval, Stage::AwaitingActivation] {
        for approvers in [1, 2] {
            for (index, case) in served_cases().into_iter().enumerate() {
                let label = format!("{stage:?}, {approvers} approvers, {}", case.label);
                // A fresh team per row, so each row starts from unsynchronized devices and
                // from the version the first change recorded.
                let harness = Harness::new();
                let outstanding = team_with_an_outstanding_version(
                    &harness,
                    &format!("outstanding-{}-{approvers}-{index}", stage.status_code()),
                    approvers,
                    stage,
                )
                .await;

                let (status, updated) = harness.update(&outstanding.arn, &case.patch).await;
                assert_eq!(
                    status, 200,
                    "case {label}: a request that asks the approvers nothing must be served, \
                     got {updated:?}"
                );

                let version_after = text(&updated, "VersionId");
                if case.synchronizes {
                    assert_ne!(
                        version_after, outstanding.live_version,
                        "case {label}: an applied action records a new live version, got \
                         {version_after}"
                    );
                    assert_ne!(
                        version_after, outstanding.requested_version,
                        "case {label}: the applied action must not take the version the \
                         outstanding change carries, got {version_after}"
                    );
                } else {
                    assert_eq!(
                        version_after, outstanding.live_version,
                        "case {label}: a request with nothing to apply leaves the version as \
                         it was, got {version_after}"
                    );
                }

                let (status, after) = harness.get(&outstanding.arn).await;
                assert_eq!(status, 200, "case {label}: get returned {after:?}");
                assert_eq!(
                    text(&after, "VersionId"),
                    version_after,
                    "case {label}: get must report the VersionId the update answered with: \
                     {after:?}"
                );

                // The devices: synchronized in BOTH sets the team holds, or untouched in
                // both.
                let expected = if case.synchronizes {
                    in_sync.as_str()
                } else {
                    out_of_sync.as_str()
                };
                assert_every_device(
                    &label,
                    "live",
                    &sync_statuses(&after, None),
                    expected,
                    approvers,
                );
                assert_every_device(
                    &label,
                    "PendingUpdate",
                    &sync_statuses(&after, Some("PendingUpdate")),
                    expected,
                    approvers,
                );

                // The version the team is waiting on is untouched, and the team is still
                // waiting on it.
                assert_eq!(
                    text(&after, "Description"),
                    outstanding.live_description,
                    "case {label}: the live Description moved: {after:?}"
                );
                assert_eq!(
                    text(&after, "Status"),
                    state::TeamStatus::Pending.as_wire(),
                    "case {label}: the team is still moving to the outstanding version: \
                     {after:?}"
                );
                assert_eq!(
                    text(&after, "StatusCode"),
                    stage.status_code(),
                    "case {label}: wrong StatusCode after the served request: {after:?}"
                );
                assert_eq!(
                    text(&after["PendingUpdate"], "VersionId"),
                    outstanding.requested_version,
                    "case {label}: the outstanding version id moved: {after:?}"
                );
                assert_eq!(
                    text(&after["PendingUpdate"], "Description"),
                    PENDING_DESCRIPTION,
                    "case {label}: the outstanding version's configuration moved: {after:?}"
                );
                assert_eq!(
                    text(&after["PendingUpdate"], "Status"),
                    state::TeamStatus::Pending.as_wire(),
                    "case {label}: the outstanding version is still pending: {after:?}"
                );
                assert_eq!(
                    text(&after["PendingUpdate"], "StatusCode"),
                    stage.status_code(),
                    "case {label}: wrong PendingUpdate StatusCode: {after:?}"
                );
            }
        }
    }
}

/// The same rule at the largest team either call accepts: `Approvers` is
/// `length(min: 1, max: 20)`, so the action reaches 40 approver records — the live twenty
/// and the twenty of the version the team is waiting on.
#[tokio::test]
async fn the_action_reaches_every_approver_record_of_the_largest_team() {
    let harness = Harness::new();
    let outstanding = team_with_an_outstanding_version(
        &harness,
        "outstanding-max-approvers",
        MAX_APPROVERS,
        Stage::AwaitingApproval,
    )
    .await;

    let (status, updated) = harness
        .update(
            &outstanding.arn,
            &json!({ "UpdateActions": [state::SYNCHRONIZE_MFA_DEVICES_ACTION] }),
        )
        .await;
    assert_eq!(status, 200, "update returned {updated:?}");

    let (status, after) = harness.get(&outstanding.arn).await;
    assert_eq!(status, 200, "get returned {after:?}");
    let in_sync = state::MfaSyncStatus::InSync.as_wire();
    assert_every_device(
        "maximum approvers",
        "live",
        &sync_statuses(&after, None),
        in_sync,
        MAX_APPROVERS,
    );
    assert_every_device(
        "maximum approvers",
        "PendingUpdate",
        &sync_statuses(&after, Some("PendingUpdate")),
        in_sync,
        MAX_APPROVERS,
    );
}

/// A request that NAMES a configuration member while a version is outstanding keeps its
/// 409: it would ask the team's approvers to approve two versions at once. The refusal
/// names the version they are already deciding, and the request writes nothing.
#[tokio::test]
async fn a_second_configuration_change_is_still_refused_while_a_version_waits() {
    for stage in [Stage::AwaitingApproval, Stage::AwaitingActivation] {
        // A refused request writes nothing, so every row of this table runs against the
        // same outstanding version.
        let harness = Harness::new();
        let outstanding = team_with_an_outstanding_version(
            &harness,
            &format!("second-change-{}", stage.status_code()),
            2,
            stage,
        )
        .await;

        // Each row names ONE of the three configuration members `UpdateApprovalTeam`
        // records as a version, and the last one names the update action beside it: a
        // request that asks for a configuration member to change is a second change
        // whatever else it carries.
        let rows: Vec<(&str, Value)> = vec![
            ("a Description", json!({ "Description": "a second change" })),
            (
                "an ApprovalStrategy",
                json!({ "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } } }),
            ),
            (
                "an Approvers that drops one",
                json!({ "Approvers": [approver_member(1)] }),
            ),
            (
                "a Description beside the update action",
                json!({
                    "Description": "a second change beside an action",
                    "UpdateActions": [state::SYNCHRONIZE_MFA_DEVICES_ACTION],
                }),
            ),
        ];

        for (row, patch) in rows {
            let label = format!("{stage:?}, {row}");
            let (status, conflict) = harness.update(&outstanding.arn, &patch).await;
            assert_eq!(
                status, 409,
                "case {label}: a second configuration change must conflict, got {conflict:?}"
            );
            assert_eq!(
                error_code(&conflict),
                "ConflictException",
                "case {label}: wrong error code: {conflict:?}"
            );
            let refusal = message(&conflict);
            assert!(
                refusal.contains(&outstanding.requested_version)
                    && refusal.contains(&outstanding.arn),
                "case {label}: the refusal must name the team and the version its approvers \
                 are deciding, got {refusal:?}"
            );
        }

        // Nothing a refused request named was written, and no device was synchronized.
        let (status, after) = harness.get(&outstanding.arn).await;
        assert_eq!(status, 200, "case {stage:?}: get returned {after:?}");
        assert_eq!(
            text(&after, "VersionId"),
            outstanding.live_version,
            "case {stage:?}: a refused request moved the live version: {after:?}"
        );
        assert_eq!(
            text(&after, "Description"),
            outstanding.live_description,
            "case {stage:?}: a refused request moved the live Description: {after:?}"
        );
        assert_eq!(
            text(&after["PendingUpdate"], "Description"),
            PENDING_DESCRIPTION,
            "case {stage:?}: a refused request moved the outstanding version: {after:?}"
        );
        let out_of_sync = state::MfaSyncStatus::OutOfSync.as_wire();
        assert_every_device(
            &format!("{stage:?} refused rows"),
            "live",
            &sync_statuses(&after, None),
            out_of_sync,
            2,
        );
        assert_every_device(
            &format!("{stage:?} refused rows"),
            "PendingUpdate",
            &sync_statuses(&after, Some("PendingUpdate")),
            out_of_sync,
            2,
        );
    }
}

/// The handshake the in-place action ran beside still completes: the version the team was
/// waiting on is approved and activated under the id its caller was answered with, and the
/// approvers it brings live keep the devices the action synchronized.
#[tokio::test]
async fn the_outstanding_version_still_activates_after_an_in_place_action() {
    let harness = Harness::new();
    let outstanding = team_with_an_outstanding_version(
        &harness,
        "activation-after-action",
        2,
        Stage::AwaitingApproval,
    )
    .await;

    let (status, updated) = harness
        .update(
            &outstanding.arn,
            &json!({ "UpdateActions": [state::SYNCHRONIZE_MFA_DEVICES_ACTION] }),
        )
        .await;
    assert_eq!(status, 200, "the action returned {updated:?}");

    make_approval_due(&harness, &outstanding.arn);
    let (status, approved) = harness.get(&outstanding.arn).await;
    assert_eq!(status, 200, "get returned {approved:?}");
    assert_eq!(
        text(&approved, "StatusCode"),
        state::TeamStatusCode::UpdatePendingActivation.as_wire(),
        "the approvers' answer must still be owed on the outstanding version: {approved:?}"
    );

    make_activation_due(&harness, &outstanding.arn);
    let (status, live) = harness.get(&outstanding.arn).await;
    assert_eq!(status, 200, "get returned {live:?}");
    assert_eq!(
        text(&live, "VersionId"),
        outstanding.requested_version,
        "the activated version is the one the change answered with: {live:?}"
    );
    assert_eq!(
        text(&live, "Description"),
        PENDING_DESCRIPTION,
        "the approved configuration is the live one: {live:?}"
    );
    assert_eq!(
        text(&live, "Status"),
        state::TeamStatus::Active.as_wire(),
        "the team is ACTIVE again: {live:?}"
    );
    assert!(
        live.get("PendingUpdate").is_none(),
        "nothing is pending once the change is live: {live:?}"
    );
    assert_every_device(
        "after the activation",
        "live",
        &sync_statuses(&live, None),
        state::MfaSyncStatus::InSync.as_wire(),
        2,
    );
}
