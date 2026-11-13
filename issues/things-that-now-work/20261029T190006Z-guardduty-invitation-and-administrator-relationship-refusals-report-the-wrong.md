Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-accept-invitation-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-accept-invitation-error-paths guardduty-accept-invitation-invalid-invitation-error guardduty-administrator-master-account-relationship guardduty-member-account-association-queries-and-disassociate guardduty-decline-and-delete-nonexistent-invitations guardduty-invitation-listing-and-cleanup guardduty-member-invite-and-monitoring guardduty-member-monitoring-workflow
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|AcceptAdministratorInvitation|4xx->4xx|$.message
Issue-Key: guardduty|AcceptInvitation|4xx->4xx|$.message
Issue-Key: guardduty|DisassociateFromAdministratorAccount|4xx->4xx|$.message
Issue-Key: guardduty|DisassociateFromMasterAccount|4xx->4xx|$.message
Issue-Key: guardduty|DeclineInvitations|2xx->2xx|$.unprocessedAccounts[*].result
Issue-Key: guardduty|DeleteInvitations|2xx->2xx|$.unprocessedAccounts[*].result
Issue-Key: guardduty|GetMemberDetectors|2xx->2xx|$.members[*]
Issue-Key: guardduty|GetMemberDetectors|2xx->2xx|$.unprocessedAccounts[*]
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# Invitation and administrator-relationship refusals report the wrong reason

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptAdministratorInvitation

## Description
Across the invitation and administrator-relationship surface the emulator substitutes its own diagnostic for GuardDuty's fixed refusal texts, including on the per-account unprocessedAccounts results; GetMemberDetectors additionally answers with member configurations for accounts that are not members. 10 causes.

Cause 1 of 10: AcceptAdministratorInvitation with an administratorId/invitationId pair that identifies no pending invitation.
AWS: HTTP 400, BadRequestException with $.__type/$.type 'InvalidInputException', message 'The request is rejected because an invalid or out-of-range value is specified as an input parameter.' — the generic input-parameter sentence, entirely fixed, naming neither member.
Emulator: 'The request is rejected because the input administratorId and invitationId do not identify an invitation pending for the current account.'
Why: the emulator volunteers a semantic explanation AWS does not; a client matching GuardDuty's documented text to classify the failure never matches, and tests written against the emulator fail on real AWS.
Fix: the AcceptAdministratorInvitation handler's refusal message.
Traces: guardduty-accept-invitation-error-paths#task#2.

Cause 2 of 10: AcceptInvitation (the legacy master-side spelling) with a masterId/invitationId pair that identifies no pending invitation.
AWS: HTTP 400, InvalidInputException, the same fixed 'invalid or out-of-range value ... input parameter' sentence.
Emulator: 'The request is rejected because the input masterId and invitationId do not identify an invitation pending for the current account.'
Why: as cause 1.
Fix: the AcceptInvitation handler's refusal message.
Traces: guardduty-accept-invitation-error-paths#task#3.

Cause 3 of 10: DisassociateFromAdministratorAccount called by an account that is itself the administrator.
AWS: HTTP 400, InvalidInputException, message 'The request failed because admin account cannot disassociate from itself.' — fixed text naming the actual rule.
Emulator: 'The request is rejected because the current account is not associated with an administrator account.'
Why: the emulator reports the opposite condition. A caller told 'not associated with an administrator' concludes the relationship is already torn down and skips the real remediation (the admin account must not call this at all), so an operator loses the one diagnostic that explains the refusal.
Fix: the DisassociateFromAdministratorAccount handler's self-disassociation branch and its message.
Traces: guardduty-administrator-master-account-relationship#task#4.

Cause 4 of 10: DisassociateFromMasterAccount called by the administrator account.
AWS: HTTP 400, InvalidInputException, 'The request failed because admin account cannot disassociate from itself.'
Emulator: 'The request is rejected because the current account is not associated with an administrator account.'
Why: as cause 3, on the legacy spelling.
Fix: the DisassociateFromMasterAccount handler's self-disassociation branch.
Traces: guardduty-administrator-master-account-relationship#task#5.

Cause 5 of 10: DeclineInvitations for accounts that sent no invitation.
AWS: HTTP 200 with one unprocessedAccounts entry per account, result 'Relationship status transition is invalid.' — fixed text; the accountId comes from the request.
Emulator: result 'The request is rejected because the current account has not received an invitation from the given account ID.'
Why: AWS reports a state-machine refusal; the emulator reports a missing-invitation one. A client that retries on 'no invitation yet' but gives up on an invalid transition retries forever against the emulator.
Fix: the DeclineInvitations per-account result classification and message.
Traces: guardduty-decline-and-delete-nonexistent-invitations#task#2.

Cause 6 of 10: DeleteInvitations for accounts that sent no invitation.
AWS: HTTP 200, per-account result 'The request is rejected either because no such invitation exists, or the current account is still associated to the given master account, or the current account has not yet declined the invitation from the given master account.' — a single fixed sentence enumerating all three reasons, deliberately not disambiguating them.
Emulator: 'The request is rejected because the current account has not received an invitation from the given account ID.'
Why: the emulator commits to one of the three branches AWS refuses to distinguish, so a caller that keys remediation on the exact wording acts on a reason the service never asserted.
Fix: the DeleteInvitations per-account result message.
Traces: guardduty-decline-and-delete-nonexistent-invitations#task#3.

Cause 7 of 10: DeclineInvitations again, in the invitation-listing/cleanup flow.
AWS: per-account result 'Relationship status transition is invalid.'
Emulator: 'The request is rejected because the current account has not received an invitation from the given account ID.'
Why: as cause 5; reproduced on a second fixture, so it is the handler rather than one fixture's setup.
Fix: the DeclineInvitations per-account result message.
Traces: guardduty-invitation-listing-and-cleanup#task#3.

Cause 8 of 10: DeleteInvitations again, in the invitation-listing/cleanup flow.
AWS: the three-reason fixed sentence of cause 6.
Emulator: the single-reason sentence.
Why: as cause 6.
Fix: the DeleteInvitations per-account result message.
Traces: guardduty-invitation-listing-and-cleanup#task#4.

Cause 9 of 10: GetMemberDetectors returns configurations for accounts that are not members.
AWS: for an account that is not an associated member there is no 'members' entry at all; the account appears in unprocessedAccounts with result 'The request is rejected because the given account ID is not an associated member of the current account.'
Emulator: returns a fully populated members entry for each requested account (dataSources plus the whole features list) and no unprocessedAccounts.
Why: the caller is handed invented data-source and feature configuration for accounts it does not administer, and loses the per-account failure report, so an inventory built from GetMemberDetectors contains accounts that are not members.
Fix: the GetMemberDetectors handler's membership check — unknown accounts belong in unprocessedAccounts, not in members.
Traces: guardduty-member-invite-and-monitoring#task#5.

Cause 10 of 10: GetMemberDetectors again invents a members entry and omits the unprocessedAccounts entry.
AWS: the single requested non-member account appears only in unprocessedAccounts with the fixed 'not an associated member of the current account' result.
Emulator: emits a populated members[0] and no unprocessedAccounts[0].
Why: as cause 9; reproduced on a second fixture.
Fix: the GetMemberDetectors membership check.
Traces: guardduty-member-monitoring-workflow#task#3.

Related open issues (neither covers these; neither carries a replay key for them): 20261029T090312Z-guardduty-getmemberdetectors-wrong-behavior.md and 20261029T160343Z-guardduty-declineinvitations-wrong-behavior.md.

## Evidence
Cause 1: $.message expected 'The request is rejected because an invalid or out-of-range value is specified as an input parameter.', got 'The request is rejected because the input administratorId and invitationId do not identify an invitation pending for the current account.' (400 both sides)
Cause 2: same expectation, got the masterId/invitationId variant.
Causes 3-4: $.message expected 'The request failed because admin account cannot disassociate from itself.', got 'The request is rejected because the current account is not associated with an administrator account.'
Causes 5, 7: $.unprocessedAccounts[*].result expected 'Relationship status transition is invalid.', got 'The request is rejected because the current account has not received an invitation from the given account ID.'
Causes 6, 8: $.unprocessedAccounts[*].result expected 'The request is rejected either because no such invitation exists, or the current account is still associated to the given master account, or the current account has not yet declined the invitation from the given master account.', got the single-reason sentence.
Cause 9: $.members[0] and [1] expected absent (None), got full {accountId, dataSources, features} entries; the matching unprocessedAccounts entries are absent.
Cause 10: $.members[0] expected absent, got a full entry; $.unprocessedAccounts[0] expected {'accountId': <id>, 'result': 'The request is rejected because the given account ID is not an associated member of the current account.'}, got None.

## Steps to Reproduce
1. Replay guardduty-accept-invitation-error-paths steps task#2 and task#3 with an invitation pair that matches nothing.
2. Replay guardduty-administrator-master-account-relationship steps task#4 and task#5, calling Disassociate* as the administrator account itself.
3. Replay guardduty-decline-and-delete-nonexistent-invitations steps task#2/task#3 and guardduty-invitation-listing-and-cleanup steps task#3/task#4 and inspect $.unprocessedAccounts[*].result.
4. Replay guardduty-member-invite-and-monitoring step task#5 and guardduty-member-monitoring-workflow step task#3: GetMemberDetectors returns members entries for non-member accounts.
