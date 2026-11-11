Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanrooms.delete-member-creator-rejected
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms.delete-member-creator-rejected|run#23|AssertionError|assert error["Message"] == (
Issue-Commit: f3853c2ce3831f59f19a9d4fb097e3f9d5244398

# DeleteMember creator refusal is emulator-worded instead of AWS's "Cannot delete member for the creator of the collaboration until the collaboration is deleted."

> _Note: this issue was identified against the code base at commit `f3853c2ce3831f59f19a9d4fb097e3f9d5244398`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMember

## Description
One net-new root cause on the cleanrooms control plane, reached through the DeleteMember creator-removal refusal path. 1 cause must be closed before this issue is submitted.

Cause 1 of 1: DeleteMember addressed at the collaboration's own creator account is refused with the correct error code and HTTP status, but with a truncated, emulator-authored message sentence.

AWS: Smithy (services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/smithy-model/com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda.json): DeleteMember is DELETE /collaborations/{collaborationIdentifier}/member/{accountId}, @idempotent, success HTTP 204 with an empty output, and models ConflictException (httpError 409) among AccessDeniedException(403) | InternalServerException(500) | ResourceNotFoundException(404) | ThrottlingException(429) | ValidationException(400). ConflictException carries message + optional resourceId + resourceType (ResourceType enum) + reason (ConflictExceptionReason = ALREADY_EXISTS | SUBRESOURCES_EXIST | INVALID_STATE); for this class the applicable reason is INVALID_STATE. The model does not state the refusal text, so the recorded behavior is the authority: AWS refuses removal of the creator with HTTP 409, Code 'ConflictException', and the message "Cannot delete member for the creator of the collaboration until the collaboration is deleted." That whole sentence is FIXED text — it has no variable part, no account id, no collaboration id and no member name are interpolated into it, and it ends with a period. The creator account id in any single replay is a per-run value and is not part of the requirement; it is only what selects this path (accountId in the request path equals the collaboration's creatorAccountId).

Emulator: answers HTTP 409 with Code 'ConflictException' (code and status correct) but the message "Cannot delete member for the creator of the collaboration." — AWS's sentence truncated at "collaboration", dropping the clause " until the collaboration is deleted" that states WHEN the removal becomes possible. Everything else in this check passes: the invited member is added via a collaboration change request and appears in ListMembers, DeleteMember on that invited member succeeds and flips its MemberStatus to REMOVED in place, the creator's membership still reads ACTIVE after the rejected removal, the creator is still listed in ListMembers, and GetCollaboration still reports the right creatorAccountId. Only the refusal wording diverges.

Why: a client or operational tooling that matches, logs or surfaces AWS's refusal sentence to tell "this member cannot be removed while the collaboration lives, delete the collaboration first" apart from other 409 conflicts (ALREADY_EXISTS, SUBRESOURCES_EXIST) never matches the emulator's string, so teardown and member-lifecycle flows that key remediation off that message take a different branch against the emulator than against AWS. The dropped clause is precisely the actionable half of the sentence, so a human reading emulator logs is told the removal is impossible rather than that it is possible after DeleteCollaboration. Message-asserting conformance tests fail even though the code and status are right.

Fix: the DeleteMember handler's creator-guard refusal path in the cleanrooms service crate (and the shared cleanrooms conflict-message builder it uses) — emit AWS's full fixed sentence for this refusal class, and populate the modeled ConflictException members consistently (resourceId/resourceType for the addressed member, reason INVALID_STATE). Note the already-fixed issue 20261108T183934Z-cleanrooms-deletemember-stub.md stood this operation up and both its success path and its creator guard now answer, so this is NOT a reopening of it: the call is correctly refused with the correct code, only the text is emulator-worded. The open issue 20261102T062702Z-cleanrooms-deleteanalysistemplate-not-found-refusal-is-emulator-worded-instead.md records the same CLASS of emulator-worded refusal text, but for DeleteAnalysisTemplate's not-found path, and names neither DeleteMember nor this check, so it does not cover this cause.

Traces: cleanrooms.delete-member-creator-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator: 13/24 passed overall; cleanrooms 13/22 passed with 9 failed, cleanrooms__cleanroomsml__cloudformation 0/2. Nine of those ten failures are already filed (five blocked on unimplemented scaffold stubs — ListPrivacyBudgetTemplates, DeleteConfiguredTable, DeleteConfiguredTableAssociationAnalysisRule, UpdateProtectedQuery, UpdateConfiguredTableAssociationAnalysisRule — and four marked OWNED by open issues). The failure below is the only net-new one.

Cause 1 — cleanrooms.delete-member-creator-rejected [mismatch], check line 91:
  assert error["Message"] == (
      "Cannot delete member for the creator of the collaboration until the collaboration is deleted."
  ), error
failed with
  AssertionError: {'Message': 'Cannot delete member for the creator of the collaboration.', 'Code': 'ConflictException'}

Expected: HTTP 409, Code 'ConflictException', Message "Cannot delete member for the creator of the collaboration until the collaboration is deleted." (entirely fixed text, no interpolated values).
Actual:   HTTP 409, Code 'ConflictException' (correct), Message "Cannot delete member for the creator of the collaboration." — the same sentence truncated after "collaboration", with the clause " until the collaboration is deleted" missing.

The assertion immediately before it, `assert error["Code"] == "ConflictException"` (check line 90), passed, as did every earlier assertion in the check: CreateCollaboration + CreateMembership succeeded; CreateCollaborationChangeRequest with a MEMBER specification for account 111122223333 was accepted and committed; ListMembers showed that invited account; DeleteMember on the invited account succeeded and left it listed with status REMOVED. The assertions after it also describe state the emulator holds correctly — the creator membership still reads ACTIVE and the creator is still in ListMembers — so the divergence is isolated to the refusal message text.

Model evidence: DeleteMember — http {method: DELETE, uri: /collaborations/{collaborationIdentifier}/member/{accountId}, code: 204}, idempotent; input collaborationIdentifier (required, httpLabel, length 36/36, uuid pattern) and accountId (required, httpLabel, length 12/12, pattern ^\d+$); errors include ConflictException httpError 409. ConflictException members: message, resourceId, resourceType (ResourceType), reason (ConflictExceptionReason = ALREADY_EXISTS | SUBRESOURCES_EXIST | INVALID_STATE); doc "Updating or deleting a resource can cause an inconsistent state." MemberStatus = INVITED | ACTIVE | LEFT | REMOVED.

## Steps to Reproduce
Boot the emulator and run the cleanrooms behavioral checks (suite simparity-behavioral-aws-v1), check cleanrooms.delete-member-creator-rejected.

1. GetCallerIdentity to learn the calling account id (this account becomes the collaboration creator).
2. CreateCollaboration(name=<any valid name>, members=[], creatorMemberAbilities=[CAN_QUERY, CAN_RECEIVE_RESULTS], creatorDisplayName="creator", queryLogStatus=DISABLED); keep collaboration.id. Then CreateMembership(collaborationIdentifier=<id>, queryLogStatus=DISABLED).
3. Positive control: CreateCollaborationChangeRequest on the collaboration with one MEMBER change (accountId 111122223333, memberAbilities=[], displayName="invited-member"); APPROVE then COMMIT it if it is returned PENDING. ListMembers now shows 111122223333. DELETE /collaborations/{id}/member/111122223333 succeeds and ListMembers shows that account with status REMOVED.
4. Issue the same call against the CREATOR's account id: DELETE /collaborations/{id}/member/{creatorAccountId}. Observe HTTP 409 Code 'ConflictException' with Message "Cannot delete member for the creator of the collaboration." instead of AWS's "Cannot delete member for the creator of the collaboration until the collaboration is deleted."
5. (Unaffected, passes either way: GetMembership still reports ACTIVE, ListMembers still lists the creator, GetCollaboration still reports creatorAccountId.)
