// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the update handshake of `UpdateApprovalTeam` and what
//! `GetApprovalTeam` reports while it runs.
//!
//! The rule under test: a change to the configuration an ACTIVE team's approvers approved
//! is not applied where it is asked for — it is RECORDED as a requested version and put
//! to the team, so the team keeps the configuration and the version id it is running on,
//! reports `PENDING` with `UPDATE_PENDING_APPROVAL`, names the session that decides the
//! change in `UpdateSessionArn` and describes the requested version in `PendingUpdate`;
//! only when the approval and then the activation come due does the requested
//! configuration become the live one, under the version id the update answered with. A
//! team whose configuration has never been activated has no approved configuration to
//! protect, so there the change is applied in place.
//!
//! The table spans the class the rule generalises over: which configuration members the
//! change names (description, strategy, approvers added, approvers removed, all three at
//! once), how many approvers the team has (1, 2, 3 and the `Approvers` `length(1..20)`
//! maximum) and how many approvals its strategy requires (M = 1, M below N, M = N).
//! Renaming nothing else, the rows differ along what the rule keys on, not along one
//! recorded shape.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the routes, the URI labels, the JSON envelope
//! and the HTTP status are all exercised.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, get_session, wait_for_the_clock, MAX_APPROVERS};
use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};

/// An approver, in the form `approval_sessions::active_team` records them, so a change
/// that KEEPS an approver names the identity the team already carries.
fn approver(n: usize) -> (String, String) {
    (
        format!("identity-{n:04}"),
        format!(
            "arn:aws:mpa:{REGION}:{}:identity-source/ids-{n:012}",
            engine::emulator_account_id()
        ),
    )
}

pub(crate) fn approver_member(n: usize) -> Value {
    let (id, source) = approver(n);
    json!({ "PrimaryIdentityId": id, "PrimaryIdentitySourceArn": source })
}

/// `PUT /sessions/{SessionArn}` — `CancelSession`.
async fn cancel_session(harness: &Harness, session_arn: &str) -> (u16, Value) {
    harness
        .call("PUT", &format!("/sessions/{}", encode_label(session_arn)), None)
        .await
}

/// `DELETE /approval-teams/{Arn}/{VersionId}` — `DeleteInactiveApprovalTeamVersion`.
async fn delete_version(harness: &Harness, team_arn: &str, version_id: &str) -> (u16, Value) {
    harness
        .call(
            "DELETE",
            &format!(
                "/approval-teams/{}/{}",
                encode_label(team_arn),
                encode_label(version_id)
            ),
            None,
        )
        .await
}

/// Pull the instant the team's answer to a requested change comes due into the past, so
/// the clock-owed approval is due on the next call. Only the DUE INSTANT is moved; the
/// approval itself is the service's own, exactly as `approval_team_lifecycle.rs` moves a
/// deletion window's end.
///
/// It is moved to NOW rather than further back, because the activation the approval owes
/// is measured from the instant the approval was due: a due instant long past would make
/// the activation due in the same breath, and this table reads the state in between.
pub(crate) fn make_approval_due(harness: &Harness, team_arn: &str) {
    with_pending_update(harness, team_arn, |update| {
        update.approval_due = Some(SystemTime::now());
    });
}

/// Push that same instant well past the end of a test, so a requested change stays
/// waiting on its approvers for as long as the test needs it to.
pub(crate) fn postpone_approval(harness: &Harness, team_arn: &str) {
    with_pending_update(harness, team_arn, |update| {
        update.approval_due = Some(SystemTime::now() + Duration::from_secs(3600));
    });
}

/// Pull the instant an approved change's activation comes due into the past.
pub(crate) fn make_activation_due(harness: &Harness, team_arn: &str) {
    with_pending_update(harness, team_arn, |update| {
        update.activation_due = Some(SystemTime::now() - Duration::from_secs(1));
    });
}

pub(crate) fn with_pending_update(
    harness: &Harness,
    team_arn: &str,
    edit: impl FnOnce(&mut state::PendingTeamUpdate),
) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    let team = guard.team_mut(team_arn).expect("a stored team to arrange");
    let update = team
        .pending_update
        .as_mut()
        .expect("a requested change to arrange");
    edit(update);
}

/// The `MinApprovalsRequired` a team or a requested version reports.
pub(crate) fn min_approvals(body: &Value, member: Option<&str>) -> Option<i64> {
    let strategy = match member {
        None => &body["ApprovalStrategy"],
        Some(member) => &body[member]["ApprovalStrategy"],
    };
    strategy["MofN"]["MinApprovalsRequired"].as_i64()
}

/// The `ApproverId`s a response lists, in the order it lists them.
fn approver_ids(body: &Value, member: Option<&str>) -> Vec<String> {
    let approvers = match member {
        None => &body["Approvers"],
        Some(member) => &body[member]["Approvers"],
    };
    approvers
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|approver| text(approver, "ApproverId"))
        .collect()
}

/// One row: the team the change is asked of, the change itself, and the configuration the
/// team must report once the change is approved and activated.
///
/// Shared with `handshake_live_version_identity.rs`, which pairs every row with a form of
/// `UpdateActions`: one table of configuration changes, read by both rules.
pub(crate) struct UpdateCase {
    pub(crate) case: &'static str,
    pub(crate) team_approvers: usize,
    pub(crate) team_min_approvals: i64,
    pub(crate) patch: Value,
    pub(crate) description_after: String,
    pub(crate) min_approvals_after: i64,
    pub(crate) approvers_after: usize,
}

pub(crate) fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a description, one approver, M = N",
            team_approvers: 1,
            team_min_approvals: 1,
            patch: json!({ "Description": "approved description" }),
            description_after: "approved description".to_string(),
            min_approvals_after: 1,
            approvers_after: 1,
        },
        UpdateCase {
            case: "a strategy that raises M to N",
            team_approvers: 3,
            team_min_approvals: 1,
            patch: json!({ "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 3 } } }),
            // `active_team` creates every team with this description.
            description_after: "session table".to_string(),
            min_approvals_after: 3,
            approvers_after: 3,
        },
        UpdateCase {
            case: "an Approvers that adds one",
            team_approvers: 2,
            team_min_approvals: 2,
            patch: json!({
                "Approvers": (1..=3).map(approver_member).collect::<Vec<_>>(),
            }),
            description_after: "session table".to_string(),
            min_approvals_after: 2,
            approvers_after: 3,
        },
        UpdateCase {
            case: "an Approvers that drops one",
            team_approvers: 3,
            team_min_approvals: 1,
            patch: json!({
                "Approvers": (1..=2).map(approver_member).collect::<Vec<_>>(),
            }),
            description_after: "session table".to_string(),
            min_approvals_after: 1,
            approvers_after: 2,
        },
        UpdateCase {
            case: "every member at once",
            team_approvers: 2,
            team_min_approvals: 2,
            patch: json!({
                "Description": "d",
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
                "Approvers": [approver_member(1)],
            }),
            // `Description` is `length(min: 1, max: 256)` — the minimum.
            description_after: "d".to_string(),
            min_approvals_after: 1,
            approvers_after: 1,
        },
        UpdateCase {
            case: "the largest team the model allows",
            // `Approvers` is `length(min: 1, max: 20)` on create and on update alike.
            team_approvers: MAX_APPROVERS,
            team_min_approvals: MAX_APPROVERS as i64,
            patch: json!({ "Description": "x".repeat(256) }),
            // `Description` is `length(min: 1, max: 256)` — the maximum.
            description_after: "x".repeat(256),
            min_approvals_after: MAX_APPROVERS as i64,
            approvers_after: MAX_APPROVERS,
        },
    ]
}

/// The whole handshake, over the whole table: the change waits for the team, the live
/// configuration does not move until it is approved and activated, and then it does.
#[tokio::test]
async fn an_update_to_an_active_team_waits_for_the_teams_approval() {
    for (index, case) in update_cases().into_iter().enumerate() {
        let label = case.case;
        let harness = Harness::new();
        let arn = active_team(
            &harness,
            &format!("pending-update-{index}"),
            case.team_approvers,
            case.team_min_approvals,
        )
        .await;

        let (status, before) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {before:?}");
        let live_version = text(&before, "VersionId");
        let live_description = text(&before, "Description");
        let live_min_approvals = min_approvals(&before, None);
        let live_approvers = approver_ids(&before, None);

        let (status, updated) = harness.update(&arn, &case.patch).await;
        assert_eq!(status, 200, "case {label}: update returned {updated:?}");
        let requested_version = text(&updated, "VersionId");
        assert_ne!(
            requested_version, live_version,
            "case {label}: the update must answer with a NEW VersionId, got \
             {requested_version:?}"
        );

        // The team still runs on the configuration its approvers approved.
        let (status, waiting) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {waiting:?}");
        assert_eq!(
            text(&waiting, "VersionId"),
            live_version,
            "case {label}: the live VersionId moved before the team approved: {waiting:?}"
        );
        assert_eq!(
            text(&waiting, "Description"),
            live_description,
            "case {label}: the live Description moved before the team approved: {waiting:?}"
        );
        assert_eq!(
            min_approvals(&waiting, None),
            live_min_approvals,
            "case {label}: the live ApprovalStrategy moved before the team approved: {waiting:?}"
        );
        assert_eq!(
            approver_ids(&waiting, None),
            live_approvers,
            "case {label}: the live Approvers moved before the team approved: {waiting:?}"
        );
        assert_eq!(
            text(&waiting, "Status"),
            state::TeamStatus::Pending.as_wire(),
            "case {label}: a team awaiting its approvers' answer is PENDING: {waiting:?}"
        );
        assert_eq!(
            text(&waiting, "StatusCode"),
            state::TeamStatusCode::UpdatePendingApproval.as_wire(),
            "case {label}: wrong StatusCode while the change waits: {waiting:?}"
        );

        // The requested version is reported beside the live one.
        assert_eq!(
            text(&waiting["PendingUpdate"], "VersionId"),
            requested_version,
            "case {label}: PendingUpdate must carry the version the update answered with: \
             {waiting:?}"
        );
        assert_eq!(
            text(&waiting["PendingUpdate"], "Description"),
            case.description_after,
            "case {label}: wrong PendingUpdate Description: {waiting:?}"
        );
        assert_eq!(
            min_approvals(&waiting, Some("PendingUpdate")),
            Some(case.min_approvals_after),
            "case {label}: wrong PendingUpdate ApprovalStrategy: {waiting:?}"
        );
        assert_eq!(
            waiting["PendingUpdate"]["NumberOfApprovers"].as_u64(),
            Some(case.approvers_after as u64),
            "case {label}: wrong PendingUpdate NumberOfApprovers: {waiting:?}"
        );
        assert_eq!(
            approver_ids(&waiting, Some("PendingUpdate")).len(),
            case.approvers_after,
            "case {label}: PendingUpdate must list the approvers of the requested version: \
             {waiting:?}"
        );
        assert_eq!(
            text(&waiting["PendingUpdate"], "Status"),
            state::TeamStatus::Pending.as_wire(),
            "case {label}: a requested version is PENDING: {waiting:?}"
        );
        assert_eq!(
            text(&waiting["PendingUpdate"], "StatusCode"),
            state::TeamStatusCode::UpdatePendingApproval.as_wire(),
            "case {label}: wrong PendingUpdate StatusCode: {waiting:?}"
        );
        assert!(
            waiting["PendingUpdate"].get("UpdateInitiationTime").is_some(),
            "case {label}: a requested version records when it was initiated: {waiting:?}"
        );

        // The session the change was put to the team in is a real, readable session.
        let session_arn = text(&waiting, "UpdateSessionArn");
        assert!(
            !session_arn.is_empty(),
            "case {label}: the team must name the session that decides the change: {waiting:?}"
        );
        let (status, session) = get_session(&harness, &session_arn).await;
        assert_eq!(status, 200, "case {label}: get session returned {session:?}");
        assert_eq!(
            text(&session, "ActionName"),
            "UpdateApprovalTeam",
            "case {label}: wrong ActionName on the update session: {session:?}"
        );
        assert_eq!(
            text(&session, "ApprovalTeamArn"),
            arn,
            "case {label}: the update session must name its team: {session:?}"
        );
        assert_eq!(
            session["NumberOfApprovers"].as_u64(),
            Some(live_approvers.len() as u64),
            "case {label}: the session asks the approvers the team has now: {session:?}"
        );

        // A second change would ask the approvers to approve two versions at once.
        let (status, conflict) = harness
            .update(&arn, &json!({ "Description": "a second change" }))
            .await;
        assert_eq!(
            status, 409,
            "case {label}: a second update must conflict, got {conflict:?}"
        );
        assert_eq!(
            error_code(&conflict),
            "ConflictException",
            "case {label}: wrong error code for a second update: {conflict:?}"
        );

        // Reading the team does not move the change: two reads agree.
        let (_, again) = harness.get(&arn).await;
        assert_eq!(
            again, waiting,
            "case {label}: a read moved the requested change"
        );

        // The approvers answer: the change waits on its activation from here.
        make_approval_due(&harness, &arn);
        let (status, approved) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {approved:?}");
        assert_eq!(
            text(&approved, "Status"),
            state::TeamStatus::Pending.as_wire(),
            "case {label}: an approved change is still pending its activation: {approved:?}"
        );
        assert_eq!(
            text(&approved, "StatusCode"),
            state::TeamStatusCode::UpdatePendingActivation.as_wire(),
            "case {label}: wrong StatusCode after the approval: {approved:?}"
        );
        assert_eq!(
            text(&approved, "VersionId"),
            live_version,
            "case {label}: the live VersionId moved before the activation: {approved:?}"
        );
        assert_eq!(
            text(&approved["PendingUpdate"], "StatusCode"),
            state::TeamStatusCode::UpdatePendingActivation.as_wire(),
            "case {label}: wrong PendingUpdate StatusCode after the approval: {approved:?}"
        );
        let (status, session) = get_session(&harness, &session_arn).await;
        assert_eq!(status, 200, "case {label}: get session returned {session:?}");
        assert_eq!(
            text(&session, "Status"),
            state::SessionStatus::Approved.as_wire(),
            "case {label}: the update session must carry the approval: {session:?}"
        );
        let approvals = session["ApproverResponses"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|response| {
                text(response, "Response") == state::SessionResponse::Approved.as_wire()
            })
            .count();
        assert_eq!(
            approvals,
            usize::try_from(case.team_min_approvals).expect("a positive threshold"),
            "case {label}: the session records the approvals its strategy requires, no more: \
             {session:?}"
        );

        // The activation: the requested version is the live one from here.
        make_activation_due(&harness, &arn);
        let (status, live) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {live:?}");
        assert_eq!(
            text(&live, "VersionId"),
            requested_version,
            "case {label}: the approved version must be the live one: {live:?}"
        );
        assert_eq!(
            text(&live, "Description"),
            case.description_after,
            "case {label}: wrong live Description after the activation: {live:?}"
        );
        assert_eq!(
            min_approvals(&live, None),
            Some(case.min_approvals_after),
            "case {label}: wrong live ApprovalStrategy after the activation: {live:?}"
        );
        assert_eq!(
            live["NumberOfApprovers"].as_u64(),
            Some(case.approvers_after as u64),
            "case {label}: wrong live NumberOfApprovers after the activation: {live:?}"
        );
        assert_eq!(
            text(&live, "Status"),
            state::TeamStatus::Active.as_wire(),
            "case {label}: the team is ACTIVE again once the change is activated: {live:?}"
        );
        assert!(
            live.get("StatusCode").is_none(),
            "case {label}: an activated team reports no status code: {live:?}"
        );
        assert!(
            live.get("PendingUpdate").is_none(),
            "case {label}: an activated change is no longer pending: {live:?}"
        );
        assert!(
            live.get("UpdateSessionArn").is_none(),
            "case {label}: no update session is outstanding once the change is live: {live:?}"
        );
        assert!(
            live.get("LastUpdateTime").is_some(),
            "case {label}: an activated change records a LastUpdateTime: {live:?}"
        );

        // An approver the change KEEPS keeps the id it already had.
        let kept: Vec<String> = approver_ids(&live, None)
            .into_iter()
            .filter(|id| live_approvers.contains(id))
            .collect();
        let expected_kept = case.approvers_after.min(live_approvers.len());
        assert_eq!(
            kept.len(),
            expected_kept,
            "case {label}: expected {expected_kept} approver(s) to keep their ApproverId, got \
             {kept:?} of {live_approvers:?}"
        );

        // And the team takes a further change, now that nothing is pending.
        let (status, next) = harness
            .update(&arn, &json!({ "Description": "a later change" }))
            .await;
        assert_eq!(
            status, 200,
            "case {label}: the team must accept a later change, got {next:?}"
        );
        assert_ne!(
            text(&next, "VersionId"),
            requested_version,
            "case {label}: a later change must mint a version of its own: {next:?}"
        );
    }
}

/// An update action and a configuration change in ONE request on an ACTIVE team: the
/// action takes effect where it was asked for, while the configuration change waits for
/// the team under the version the call answered with — the only version the call
/// announced, so the live version id stays the one the approvers approved.
#[tokio::test]
async fn an_update_action_applies_at_once_while_the_configuration_change_waits() {
    let harness = Harness::new();
    let arn = active_team(&harness, "action-and-change", 2, 1).await;
    let (_, before) = harness.get(&arn).await;
    let live_version = text(&before, "VersionId");
    let live_description = text(&before, "Description");

    let (status, updated) = harness
        .update(
            &arn,
            &json!({
                "Description": "changed beside an action",
                "UpdateActions": [state::SYNCHRONIZE_MFA_DEVICES_ACTION],
            }),
        )
        .await;
    assert_eq!(status, 200, "update returned {updated:?}");
    let requested_version = text(&updated, "VersionId");

    let (status, waiting) = harness.get(&arn).await;
    assert_eq!(status, 200, "get returned {waiting:?}");
    // The action asks for no configuration member to change, so it is applied at once:
    // every device is read back off the identity source.
    for (index, approver) in waiting["Approvers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let devices: Vec<String> = approver["MfaMethods"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|method| text(method, "SyncStatus"))
            .collect();
        assert_eq!(
            devices,
            vec![state::MfaSyncStatus::InSync.as_wire().to_string()],
            "case approver {index}: the action must be applied at once, got {devices:?}"
        );
    }
    assert_ne!(
        requested_version, live_version,
        "the update must answer with a version of its own, got {requested_version:?}"
    );
    assert_eq!(
        text(&waiting, "VersionId"),
        live_version,
        "the live VersionId moved while the change was still the team's to approve: \
         {waiting:?}"
    );
    // The configuration change is still the team's to approve.
    assert_eq!(
        text(&waiting, "Description"),
        live_description,
        "the live Description moved before the team approved: {waiting:?}"
    );
    assert_eq!(
        text(&waiting["PendingUpdate"], "VersionId"),
        requested_version,
        "PendingUpdate must carry the version the update answered with: {waiting:?}"
    );
    assert_eq!(
        text(&waiting["PendingUpdate"], "Description"),
        "changed beside an action",
        "wrong PendingUpdate Description: {waiting:?}"
    );

    // The approvers answer, and then the approved configuration is activated.
    make_approval_due(&harness, &arn);
    let (status, approved) = harness.get(&arn).await;
    assert_eq!(status, 200, "get returned {approved:?}");
    make_activation_due(&harness, &arn);
    let (status, live) = harness.get(&arn).await;
    assert_eq!(status, 200, "get returned {live:?}");
    assert_eq!(
        text(&live, "VersionId"),
        requested_version,
        "the approved version is the live one: {live:?}"
    );
    assert_eq!(
        text(&live, "Description"),
        "changed beside an action",
        "the approved configuration is the live one: {live:?}"
    );
    assert_eq!(
        text(&live, "Status"),
        state::TeamStatus::Active.as_wire(),
        "the team is ACTIVE again: {live:?}"
    );
}

/// Nothing is arranged: the CLOCK alone carries a requested change through its approval
/// and its activation, inside the spans this service records for them.
#[tokio::test]
async fn the_clock_alone_carries_a_requested_change_through_to_the_live_version() {
    let harness = Harness::new();
    let arn = active_team(&harness, "clock-update", 2, 1).await;
    let (_, before) = harness.get(&arn).await;
    let live_version = text(&before, "VersionId");

    let (status, updated) = harness
        .update(&arn, &json!({ "Description": "carried by the clock" }))
        .await;
    assert_eq!(status, 200, "update returned {updated:?}");
    let requested_version = text(&updated, "VersionId");

    // Straight after the call the change is still waiting: the span is never zero.
    let (_, waiting) = harness.get(&arn).await;
    assert_eq!(
        text(&waiting, "StatusCode"),
        state::TeamStatusCode::UpdatePendingApproval.as_wire(),
        "a change is waiting on its approvers straight after the call: {waiting:?}"
    );

    wait_for_the_clock(state::UPDATE_APPROVAL_DELAY);
    let (_, approved) = harness.get(&arn).await;
    assert_eq!(
        text(&approved, "StatusCode"),
        state::TeamStatusCode::UpdatePendingActivation.as_wire(),
        "once the approval is due the change waits on its activation: {approved:?}"
    );
    assert_eq!(
        text(&approved, "VersionId"),
        live_version,
        "the live version does not move on the approval alone: {approved:?}"
    );

    wait_for_the_clock(state::UPDATE_ACTIVATION_DELAY);
    let (_, live) = harness.get(&arn).await;
    assert_eq!(
        text(&live, "Status"),
        state::TeamStatus::Active.as_wire(),
        "once the activation is due the team is ACTIVE again: {live:?}"
    );
    assert_eq!(
        text(&live, "VersionId"),
        requested_version,
        "the approved version is the live one: {live:?}"
    );
    assert_eq!(
        text(&live, "Description"),
        "carried by the clock",
        "the approved configuration is the live one: {live:?}"
    );
}

/// A team whose configuration has never been activated has no approved configuration to
/// protect, so the change it is asked for is applied where it was asked for — for every
/// form that change takes.
#[tokio::test]
async fn a_change_to_a_version_that_was_never_activated_is_applied_in_place() {
    let patches: Vec<(&str, Value, Option<&str>, Option<i64>)> = vec![
        (
            "a description",
            json!({ "Description": "in place" }),
            Some("in place"),
            None,
        ),
        (
            "a strategy",
            json!({ "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 2 } } }),
            None,
            Some(2),
        ),
        (
            "approvers",
            json!({ "Approvers": (1..=3).map(approver_member).collect::<Vec<_>>() }),
            None,
            None,
        ),
    ];

    for (index, (label, patch, description, min)) in patches.into_iter().enumerate() {
        let harness = Harness::new();
        // Created and NOT waited out: the team is still waiting on its approvers'
        // invitations, so its first version has never been activated.
        let (status, created) = harness
            .create(&json!({
                "Name": format!("draft-update-{index}"),
                "Description": "draft",
                "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
                "Approvers": (1..=2).map(approver_member).collect::<Vec<_>>(),
                "Policies": [{
                    "PolicyArn":
                        "arn:aws:mpa:::aws:policy/IdentityCenter/MPAApprovalTeamPolicy/$DEFAULT"
                }],
            }))
            .await;
        assert_eq!(status, 200, "case {label}: create returned {created:?}");
        let arn = text(&created, "Arn");
        let first_version = text(&created, "VersionId");

        let (status, updated) = harness.update(&arn, &patch).await;
        assert_eq!(status, 200, "case {label}: update returned {updated:?}");
        let version = text(&updated, "VersionId");
        assert_ne!(
            version, first_version,
            "case {label}: an applied change records a new VersionId"
        );

        let (status, team) = harness.get(&arn).await;
        assert_eq!(status, 200, "case {label}: get returned {team:?}");
        assert_eq!(
            text(&team, "VersionId"),
            version,
            "case {label}: the new version is live at once: {team:?}"
        );
        assert!(
            team.get("PendingUpdate").is_none() && team.get("UpdateSessionArn").is_none(),
            "case {label}: a change applied in place waits on no approval: {team:?}"
        );
        assert_eq!(
            text(&team, "StatusCode"),
            state::TeamStatusCode::PendingActivation.as_wire(),
            "case {label}: the team is still waiting on its approvers' invitations: {team:?}"
        );
        if let Some(description) = description {
            assert_eq!(
                text(&team, "Description"),
                description,
                "case {label}: the change was not applied: {team:?}"
            );
        }
        if let Some(min) = min {
            assert_eq!(
                min_approvals(&team, None),
                Some(min),
                "case {label}: the change was not applied: {team:?}"
            );
        }
    }
}

/// A change the team never approved — its session cancelled before the approval came due —
/// leaves the team on the version it is running on, and the version it did not approve is
/// one this service will never activate: `INACTIVE`, and therefore the version
/// `DeleteInactiveApprovalTeamVersion` is there to remove.
#[tokio::test]
async fn a_change_the_team_never_approved_leaves_an_inactive_version_behind() {
    let harness = Harness::new();
    let arn = active_team(&harness, "refused-update", 2, 1).await;
    let (_, before) = harness.get(&arn).await;
    let live_version = text(&before, "VersionId");
    let live_description = text(&before, "Description");

    let (status, updated) = harness
        .update(&arn, &json!({ "Description": "never approved" }))
        .await;
    assert_eq!(status, 200, "update returned {updated:?}");
    let requested_version = text(&updated, "VersionId");
    // The approvers must not answer while this test arranges the cancellation.
    postpone_approval(&harness, &arn);
    let (_, waiting) = harness.get(&arn).await;
    let session_arn = text(&waiting, "UpdateSessionArn");

    // A version still awaiting the team's approval is not an inactive one.
    let (status, refused) = delete_version(&harness, &arn, &requested_version).await;
    assert_eq!(
        status, 409,
        "deleting a version awaiting approval must conflict, got {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "wrong error code: {refused:?}"
    );
    // Neither is the version the team is running on.
    let (status, refused) = delete_version(&harness, &arn, &live_version).await;
    assert_eq!(
        status, 409,
        "deleting the version an active team runs on must conflict, got {refused:?}"
    );

    let (status, cancelled) = cancel_session(&harness, &session_arn).await;
    assert_eq!(status, 200, "cancel session returned {cancelled:?}");

    let (status, team) = harness.get(&arn).await;
    assert_eq!(status, 200, "get returned {team:?}");
    assert_eq!(
        text(&team, "VersionId"),
        live_version,
        "the team keeps the version it is running on: {team:?}"
    );
    assert_eq!(
        text(&team, "Description"),
        live_description,
        "the team keeps the configuration its approvers approved: {team:?}"
    );
    assert_eq!(
        text(&team, "Status"),
        state::TeamStatus::Active.as_wire(),
        "a team whose change was not approved is active again: {team:?}"
    );
    assert_eq!(
        text(&team, "StatusCode"),
        state::TeamStatusCode::UpdateFailedApproval.as_wire(),
        "wrong StatusCode after a change the team did not approve: {team:?}"
    );
    assert_eq!(
        text(&team["PendingUpdate"], "Status"),
        state::TeamStatus::Inactive.as_wire(),
        "the version the team did not approve is inactive: {team:?}"
    );
    assert_eq!(
        text(&team["PendingUpdate"], "StatusCode"),
        state::TeamStatusCode::UpdateFailedApproval.as_wire(),
        "wrong PendingUpdate StatusCode: {team:?}"
    );

    // That inactive version is this operation's to remove, and removing it leaves the
    // team — and the version it runs on — exactly where they were.
    let (status, deleted) = delete_version(&harness, &arn, &requested_version).await;
    assert_eq!(status, 200, "deleting the inactive version returned {deleted:?}");
    let (status, team) = harness.get(&arn).await;
    assert_eq!(status, 200, "the team must survive the delete: {team:?}");
    assert_eq!(
        text(&team, "VersionId"),
        live_version,
        "the live version moved under the delete: {team:?}"
    );
    assert!(
        team.get("PendingUpdate").is_none() && team.get("UpdateSessionArn").is_none(),
        "the deleted version is still reported: {team:?}"
    );
    assert!(
        team.get("StatusCode").is_none(),
        "no outcome is reported for a version that is gone: {team:?}"
    );

    // Repeating the delete is a success that changes nothing, the operation being
    // `@idempotent`.
    let (status, repeated) = delete_version(&harness, &arn, &requested_version).await;
    assert_eq!(status, 200, "the repeated delete returned {repeated:?}");

    // And the team takes a change again, which mints a version of its own rather than
    // reusing the id of the one the team refused.
    let (status, next) = harness
        .update(&arn, &json!({ "Description": "a second attempt" }))
        .await;
    assert_eq!(status, 200, "the team must accept a later change: {next:?}");
    assert_ne!(
        text(&next, "VersionId"),
        requested_version,
        "a version id is never reused: {next:?}"
    );
}

/// Every status token this handshake writes is one the model declares. The catalogs come
/// from the generated `shared_types` value lists, rendered from the model's
/// `ApprovalTeamStatus` / `ApprovalTeamStatusCode` / `SessionStatus` / `SessionResponse`
/// enums.
#[test]
fn the_tokens_this_handshake_writes_are_ones_the_model_declares() {
    for code in [
        state::TeamStatusCode::UpdatePendingApproval,
        state::TeamStatusCode::UpdatePendingActivation,
        state::TeamStatusCode::UpdateFailedApproval,
    ] {
        assert!(
            model::APPROVAL_TEAM_STATUS_CODE_VALUES.contains(&code.as_wire()),
            "case {code:?}: expected a token ApprovalTeamStatusCode declares, got {:?}",
            code.as_wire()
        );
    }

    assert!(
        model::APPROVAL_TEAM_STATUS_VALUES.contains(&state::TeamStatus::Inactive.as_wire()),
        "case INACTIVE: expected a token ApprovalTeamStatus declares, got {:?}",
        state::TeamStatus::Inactive.as_wire()
    );
    assert!(
        model::SESSION_STATUS_VALUES.contains(&state::SessionStatus::Approved.as_wire()),
        "case APPROVED: expected a token SessionStatus declares, got {:?}",
        state::SessionStatus::Approved.as_wire()
    );
    assert!(
        model::SESSION_RESPONSE_VALUES.contains(&state::SessionResponse::Approved.as_wire()),
        "case APPROVED: expected a token SessionResponse declares, got {:?}",
        state::SessionResponse::Approved.as_wire()
    );
}
