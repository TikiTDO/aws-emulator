Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4c075dcdeacefbec5f942454c2320d1c6eabf26d

# An UpdateActions beside a configuration change moves an ACTIVE team's live VersionId and LastUpdateTime, which the same branch says the team keeps until its approvers approve

_Filed: 2026-10-29T00:55:03Z_
_Operation: UpdateApprovalTeam | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4c075dcdeacefbec5f942454c2320d1c6eabf26d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateApprovalTeam`'s handshake branch for an ACTIVE team is written on one stated invariant: a change to the configuration the team's approvers approved is RECORDED as a requested version, and the live team keeps "its description, strategy, approvers and version id" until that version is approved and activated. `GetApprovalTeam` reports on the same split: the live members report the configuration the team runs on, `PendingUpdate`/`UpdateSessionArn` report the version it is moving to.

Inside that branch the handler breaks the invariant whenever the same request also carries `UpdateActions: [SYNCHRONIZE_MFA_DEVICES]`: it calls `team.bump_version(now)` on the LIVE team before minting the requested version, and `bump_version` assigns `self.version`, rewrites `self.version_id` and moves `self.last_update_time`. So for a request that carries both a configuration member and the sync action against an ACTIVE team:

* the response carries the REQUESTED version id (`version` = live+1, e.g. "3"), as the handler's own comment says it must;
* the stored live team silently moves from "1" to "2" and gets a new `LastUpdateTime`, while its status becomes `PENDING`/`UPDATE_PENDING_APPROVAL`;
* a following `GetApprovalTeam` therefore reports `VersionId: "2"` — a version no response ever announced, which is neither the configuration the approvers approved ("1") nor the one they are being asked to approve ("3").

The same branch without the sync action keeps the live version id, which is what makes this an internal contradiction rather than a design: the version-id move is a side effect of the update action, not of anything the approvers answered. `ApprovalTeam::bump_version`'s own doc also states which teams it is for — "a team whose configuration no approver has approved yet ... A change to the configuration an ACTIVE team's approvers approved goes through `ApprovalTeam::request_update` instead" — and this is the one call site that reaches it for an ACTIVE team inside the `request_update` path.

## Evidence

The invariant the branch states, services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/update_approval_team.rs:324-328:
```
        // A change to the configuration an ACTIVE team's approvers approved has to be
        // approved by them too, so it is REQUESTED here rather than applied: the team
        // keeps its description, strategy, approvers and version id, and reports the
        // requested version until the team approves it and it is activated.
        if status == crate::state::TeamStatus::Active && changes_configuration {
```

The write inside that same branch, src/ops/update_approval_team.rs:411-419:
```
            let team = state.team_mut(&input.arn).ok_or_else(not_found)?;
            // An update action is applied where it was asked for, to the team as it
            // stands: it changes no member the team's approvers approved.
            if synchronize_mfa_devices {
                team.synchronize_mfa_devices();
                team.bump_version(now);
            }
            let version = team.mint_version();
            team.request_update(crate::state::PendingTeamUpdate::requested(
```

`bump_version` is exactly a change to the live `version_id` (and `last_update_time`), and its doc names the teams it is for, src/state.rs:1301-1310:
```
    /// This is the change a team whose configuration no approver has approved yet takes —
    /// a team still waiting on its activation, or one no longer active. A change to the
    /// configuration an ACTIVE team's approvers approved goes through
    /// [`ApprovalTeam::request_update`] instead.
    pub fn bump_version(&mut self, now: SystemTime) {
        self.version = self.mint_version();
        self.version_id = self.version.to_string();
        self.last_update_time = Some(now);
    }
```

The response names the REQUESTED version, so the live "2" is announced by nothing, src/ops/update_approval_team.rs:427-431:
```
            // The version id the caller is answered with is the REQUESTED one: it is the
            // version the team will carry once this change is approved and activated.
            return Ok(UpdateApprovalTeamOutput {
                version_id: Some(version.to_string()),
            });
```

The read that is supposed to show the split, src/ops/get_approval_team.rs:19-22:
```
//! A change an active team's approvers have still to approve is NOT part of the team's
//! live configuration, so the members above report the configuration the team runs on and
//! `PendingUpdate` / `UpdateSessionArn` report the version it is moving to and the session
//! that decides it ([`crate::state::PendingTeamUpdate`]).
```

Test coverage: the `UpdateActions` suite never reaches this branch, because its teams are still `PENDING` when the update is issued — `create_team` returns straight after the create without waiting for the invitation responses (tests/all/approver_mfa_synchronization.rs:62-69), and the row "the action beside a Description" (tests/all/approver_mfa_synchronization.rs:192-199) then asserts `get must report the VersionId the update returned` (tests/all/approver_mfa_synchronization.rs:246-251), an assertion that can only hold on the apply-in-place path — on the ACTIVE path the update returns the requested version while the read reports the live one. tests/all/pending_team_update.rs exercises the ACTIVE handshake but never sends `UpdateActions`. So no test pins the combination, and keeping the live version id in this branch leaves every existing assertion unchanged.

## Steps to Reproduce

1. Record the management account so the gated create is served, then `POST /approval-teams` with one approver; note `VersionId` ("1").
2. Wait past `crate::state::INVITATION_RESPONSE_DELAY` (500 ms) and `GET /approval-teams/{Arn}`: `Status` is `ACTIVE`, `VersionId` is "1".
3. `PATCH /approval-teams/{Arn}` with `{"Description": "changed", "UpdateActions": ["SYNCHRONIZE_MFA_DEVICES"]}`. The response carries `VersionId: "3"`.
4. `GET /approval-teams/{Arn}`: `Status` is `PENDING` with `StatusCode` `UPDATE_PENDING_APPROVAL`, `PendingUpdate.VersionId` is "3" — but the live `VersionId` now reads "2" and `LastUpdateTime` has moved, although nothing has been approved.
5. Repeat from step 1 with `{"Description": "changed"}` alone: the live `VersionId` stays "1" and only `PendingUpdate.VersionId` names the new version — the behaviour update_approval_team.rs:324-328 states for the branch.
