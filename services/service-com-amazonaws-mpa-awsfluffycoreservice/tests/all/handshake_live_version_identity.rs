// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the version id and the `LastUpdateTime` an ACTIVE team reports
//! while a change to its configuration is still the team's to approve.
//!
//! The rule under test: a call answers with ONE version id, and a team's live version id
//! and `LastUpdateTime` only ever move to a version a call announced — so where
//! `UpdateApprovalTeam` RECORDS the change for the team's approvers rather than applying
//! it, the version it announces is the requested one, and the live version id and
//! `LastUpdateTime` stay as the approvers left them whatever else the same request applies
//! in place (the modelled update action reads each approver's MFA devices back off the
//! identity source, which is no member of the configuration they approved). The requested
//! version becomes the live one at its activation, and nowhere else. A request that names
//! no configuration member is the other side of the same rule: it is applied where it was
//! asked for, so the version the call announces IS the live one from that call on.
//!
//! The table spans that class along what the rule generalises over: which configuration
//! members the change names (description, strategy, approvers added, approvers dropped,
//! every member at once), the form of the `UpdateActions` list the same request carries
//! (absent, the token once, the token repeated, an empty list) and how many approvers the
//! team has (1, 2, 3 and the `Approvers` `length(1..20)` maximum). The configuration rows
//! are the ones `pending_team_update.rs` already spans the handshake with
//! ([`crate::pending_team_update::update_cases`]) — one table, paired here with each form
//! of `UpdateActions`.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI label, the JSON envelope
//! and the HTTP status are all exercised.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::active_team;
use crate::approval_team_lifecycle::{text, Harness};
use crate::pending_team_update::{
    make_activation_due, make_approval_due, min_approvals, update_cases,
};
use crate::update_while_a_version_is_outstanding::{assert_every_device, sync_statuses};

/// The `LastUpdateTime` a read reports, or `None` when it reports none — a team whose
/// configuration has never been changed carries no update time at all, so the member's
/// ABSENCE is as much a value this rule has to keep as any instant.
fn last_update_time(team: &Value) -> Option<Value> {
    team.get("LastUpdateTime").cloned()
}

/// The forms the `UpdateActions` list of such a request can take. `UpdateActions` is
/// `length(min: 0, max: 100)` and `UpdateAction` declares the single token
/// `SYNCHRONIZE_MFA_DEVICES`, so these are all of them: no member, the token, the token
/// repeated, and an empty list.
fn action_forms() -> Vec<(&'static str, Option<Value>, bool)> {
    vec![
        ("no UpdateActions member", None, false),
        (
            "the action once",
            Some(json!([state::SYNCHRONIZE_MFA_DEVICES_ACTION])),
            true,
        ),
        (
            "the action repeated",
            Some(json!([
                state::SYNCHRONIZE_MFA_DEVICES_ACTION,
                state::SYNCHRONIZE_MFA_DEVICES_ACTION,
            ])),
            true,
        ),
        ("an empty UpdateActions", Some(json!([])), false),
    ]
}

/// The forms the configuration table below pairs every row with: the two that name no
/// action differ in spelling alone, so one of them carries that side of the rule here and
/// `action_forms` keeps both for the rows that stand on the action itself.
fn action_forms_beside_a_change() -> Vec<(&'static str, Option<Value>, bool)> {
    action_forms()
        .into_iter()
        .filter(|(form, _, _)| *form != "an empty UpdateActions")
        .collect()
}

/// A configuration change recorded for the team's approvers leaves the live version id and
/// `LastUpdateTime` exactly as they were — for every configuration change, beside every
/// form of `UpdateActions` — and the version the call announced becomes the live one only
/// at its activation.
#[tokio::test]
async fn a_requested_change_leaves_the_live_version_id_and_update_time_alone() {
    let in_sync = state::MfaSyncStatus::InSync.as_wire();
    let out_of_sync = state::MfaSyncStatus::OutOfSync.as_wire();

    for (change_index, change) in update_cases().into_iter().enumerate() {
        for (form_index, (form, actions, synchronizes)) in
            action_forms_beside_a_change().into_iter().enumerate()
        {
            let label = format!("{}, {form}", change.case);
            let mut patch = change.patch.clone();
            if let Some(actions) = actions {
                patch["UpdateActions"] = actions;
            }

            // A fresh team per row, so each row starts from the version its create
            // announced and from unsynchronized devices.
            let harness = Harness::new();
            let arn = active_team(
                &harness,
                &format!("handshake-version-{change_index}-{form_index}"),
                change.team_approvers,
                change.team_min_approvals,
            )
            .await;

            let (status, before) = harness.get(&arn).await;
            assert_eq!(status, 200, "case {label}: get returned {before:?}");
            let live_version = text(&before, "VersionId");
            let live_description = text(&before, "Description");
            let live_min_approvals = min_approvals(&before, None);
            let live_update_time = last_update_time(&before);

            let (status, updated) = harness.update(&arn, &patch).await;
            assert_eq!(status, 200, "case {label}: update returned {updated:?}");
            let announced = text(&updated, "VersionId");
            assert_ne!(
                announced, live_version,
                "case {label}: the call must answer with a version of its own, got \
                 {announced:?}"
            );

            // Nothing has been approved, so the team still runs on — and reports — the
            // version its approvers approved.
            let (status, waiting) = harness.get(&arn).await;
            assert_eq!(status, 200, "case {label}: get returned {waiting:?}");
            assert_eq!(
                text(&waiting, "VersionId"),
                live_version,
                "case {label}: expected the live VersionId to stay {live_version:?}, got {:?}",
                text(&waiting, "VersionId")
            );
            assert_eq!(
                last_update_time(&waiting),
                live_update_time,
                "case {label}: expected the live LastUpdateTime to stay {live_update_time:?}, \
                 got {:?}",
                last_update_time(&waiting)
            );
            assert_eq!(
                text(&waiting, "Description"),
                live_description,
                "case {label}: the live Description moved before the team approved: {waiting:?}"
            );
            assert_eq!(
                min_approvals(&waiting, None),
                live_min_approvals,
                "case {label}: the live ApprovalStrategy moved before the team approved: \
                 {waiting:?}"
            );
            assert_eq!(
                text(&waiting["PendingUpdate"], "VersionId"),
                announced,
                "case {label}: the requested version must be the one the call announced, got \
                 {:?}",
                text(&waiting["PendingUpdate"], "VersionId")
            );
            assert_eq!(
                text(&waiting, "StatusCode"),
                state::TeamStatusCode::UpdatePendingApproval.as_wire(),
                "case {label}: wrong StatusCode while the change waits: {waiting:?}"
            );

            // The action the same request carried took effect where it was asked for, in
            // both approver sets the team holds — it just mints no version of its own.
            let expected = if synchronizes { in_sync } else { out_of_sync };
            assert_every_device(
                &label,
                "live",
                &sync_statuses(&waiting, None),
                expected,
                change.team_approvers,
            );
            assert_every_device(
                &label,
                "PendingUpdate",
                &sync_statuses(&waiting, Some("PendingUpdate")),
                expected,
                change.approvers_after,
            );

            // The approvers answer: their approval alone moves neither.
            make_approval_due(&harness, &arn);
            let (status, approved) = harness.get(&arn).await;
            assert_eq!(status, 200, "case {label}: get returned {approved:?}");
            assert_eq!(
                text(&approved, "VersionId"),
                live_version,
                "case {label}: expected the live VersionId to stay {live_version:?} until the \
                 activation, got {:?}",
                text(&approved, "VersionId")
            );
            assert_eq!(
                last_update_time(&approved),
                live_update_time,
                "case {label}: expected the live LastUpdateTime to stay {live_update_time:?} \
                 until the activation, got {:?}",
                last_update_time(&approved)
            );

            // The activation: the announced version is the live one from here, under the
            // configuration the approvers approved.
            make_activation_due(&harness, &arn);
            let (status, live) = harness.get(&arn).await;
            assert_eq!(status, 200, "case {label}: get returned {live:?}");
            assert_eq!(
                text(&live, "VersionId"),
                announced,
                "case {label}: expected the activated version to be the announced \
                 {announced:?}, got {:?}",
                text(&live, "VersionId")
            );
            assert_eq!(
                text(&live, "Description"),
                change.description_after,
                "case {label}: wrong live Description after the activation: {live:?}"
            );
            assert_eq!(
                min_approvals(&live, None),
                Some(change.min_approvals_after),
                "case {label}: wrong live ApprovalStrategy after the activation: {live:?}"
            );
            assert_eq!(
                live["NumberOfApprovers"].as_u64(),
                Some(change.approvers_after as u64),
                "case {label}: wrong live NumberOfApprovers after the activation: {live:?}"
            );
            assert_ne!(
                last_update_time(&live),
                live_update_time,
                "case {label}: the activation must record a LastUpdateTime of its own, got \
                 {:?}",
                last_update_time(&live)
            );
        }
    }
}

/// Over a sequence of handshakes, every version id the team ever reports as its LIVE one is
/// a version some call announced — the create's, or an update's.
#[tokio::test]
async fn every_live_version_id_the_team_reports_is_one_a_call_announced() {
    let harness = Harness::new();
    let arn = active_team(&harness, "announced-versions-only", 2, 1).await;

    let (status, before) = harness.get(&arn).await;
    assert_eq!(status, 200, "get returned {before:?}");
    // The version `CreateApprovalTeam` announced and the team runs on.
    let mut announced: BTreeSet<String> = BTreeSet::from([text(&before, "VersionId")]);
    let mut reported_live: BTreeSet<String> = BTreeSet::from([text(&before, "VersionId")]);
    let mut last_announced = text(&before, "VersionId");

    // Two handshakes in a row, each carrying the update action beside the change, so a
    // version the handler mints for the action alone would show up here.
    for round in 1..=2 {
        let label = format!("round {round}");
        let (status, updated) = harness
            .update(
                &arn,
                &json!({
                    "Description": format!("round {round}"),
                    "UpdateActions": [state::SYNCHRONIZE_MFA_DEVICES_ACTION],
                }),
            )
            .await;
        assert_eq!(status, 200, "case {label}: update returned {updated:?}");
        last_announced = text(&updated, "VersionId");
        announced.insert(last_announced.clone());

        let (_, waiting) = harness.get(&arn).await;
        reported_live.insert(text(&waiting, "VersionId"));
        make_approval_due(&harness, &arn);
        let (_, approved) = harness.get(&arn).await;
        reported_live.insert(text(&approved, "VersionId"));
        make_activation_due(&harness, &arn);
        let (_, live) = harness.get(&arn).await;
        reported_live.insert(text(&live, "VersionId"));
        assert_eq!(
            text(&live, "Status"),
            state::TeamStatus::Active.as_wire(),
            "case {label}: the team is ACTIVE again once the change is activated: {live:?}"
        );
    }

    let unannounced: Vec<&String> = reported_live.difference(&announced).collect();
    assert!(
        unannounced.is_empty(),
        "case two handshakes: the team reported live version(s) {unannounced:?} that no call \
         announced; announced {announced:?}, reported {reported_live:?}"
    );
    let (_, final_read) = harness.get(&arn).await;
    assert_eq!(
        text(&final_read, "VersionId"),
        last_announced,
        "case two handshakes: expected the last announced version {last_announced:?} to be the \
         live one, got {:?}",
        text(&final_read, "VersionId")
    );
}

/// The other side of the rule: a request that names NO configuration member is applied
/// where it was asked for, so the version the call announces is the team's live one from
/// that call on — and it records a `LastUpdateTime`.
#[tokio::test]
async fn a_request_that_changes_no_configuration_member_announces_the_live_version() {
    for (index, (form, actions, applies)) in action_forms().into_iter().enumerate() {
        let harness = Harness::new();
        let arn = active_team(&harness, &format!("action-only-{index}"), 2, 1).await;
        let (status, before) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {form}: get returned {before:?}");
        let live_version = text(&before, "VersionId");

        let patch = match actions {
            Some(actions) => json!({ "UpdateActions": actions }),
            None => json!({}),
        };
        let (status, updated) = harness.update(&arn, &patch).await;
        assert_eq!(status, 200, "case {form}: update returned {updated:?}");
        let announced = text(&updated, "VersionId");
        if applies {
            assert_ne!(
                announced, live_version,
                "case {form}: an applied action records a new version, got {announced:?}"
            );
        } else {
            assert_eq!(
                announced, live_version,
                "case {form}: a request with nothing to apply leaves the version as it was, \
                 got {announced:?}"
            );
        }

        let (status, after) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {form}: get returned {after:?}");
        assert_eq!(
            text(&after, "VersionId"),
            announced,
            "case {form}: expected the live VersionId to be the announced {announced:?}, got {:?}",
            text(&after, "VersionId")
        );
        assert_eq!(
            text(&after, "Status"),
            state::TeamStatus::Active.as_wire(),
            "case {form}: a request the approvers have nothing to approve leaves the team \
             ACTIVE: {after:?}"
        );
        assert!(
            after.get("PendingUpdate").is_none(),
            "case {form}: such a request asks the approvers nothing: {after:?}"
        );
        assert_eq!(
            last_update_time(&after).is_some(),
            applies,
            "case {form}: expected a LastUpdateTime to be recorded only where something was \
             applied, got {:?}",
            last_update_time(&after)
        );
    }
}
