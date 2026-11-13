Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.delegation-request-workflow-lifecycle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.delegation-request-workflow-lifecycle|run#23|AssertionError|assert code_reject == "NoSuchEntity", (
Issue-Commit: 2294e811bd9f401401d7efd231d6d04e23ec8f30

# iam: RejectDelegationRequest answers 501 NotImplemented instead of the modelled NoSuchEntity (404) for an unknown DelegationRequestId

> _Note: this issue was identified against the code base at commit `2294e811bd9f401401d7efd231d6d04e23ec8f30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RejectDelegationRequest

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: iam:RejectDelegationRequest refuses every call with the scaffold's NotImplemented, so it never reaches the modelled not-found refusal for a DelegationRequestId that names no delegation request.

AWS: RejectDelegationRequest is a bound IAM Query operation. Its input is DelegationRequestId (required, delegationRequestIdType, @length 16..128, pattern ^[\w-]+$) plus optional Notes (notesType, @length 0..500). Smithy: RejectDelegationRequest models ConcurrentModificationException (ConcurrentModification, 409), InvalidInputException (InvalidInput, 400), NoSuchEntityException (awsQueryError code NoSuchEntity, HTTP 404) and ServiceFailureException (ServiceFailure, 500); its output shape is Unit. Smithy doc for NoSuchEntityException: "The request was rejected because it referenced a resource entity that does not exist. The error message describes the resource." Given a syntactically valid id that was never created, the refusal is HTTP 404, code NoSuchEntity, with the message AWS words for the request-scoped delegation ops: the fixed text "Delegation request not found: " followed by the DelegationRequestId taken verbatim from the request (recorded behavior: the sibling ops GetDelegationRequest/AcceptDelegationRequest use the compact spelling "DelegationRequest not found: <id>", while Update/Reject/Associate/SendDelegationToken use the spaced spelling "Delegation request not found: <id>"). The id in that message is a request value, never a fixed part of the text.

Emulator: the call is refused with code NotImplemented and message "operation RejectDelegationRequest is scaffolded but not implemented in this emulator" (HTTP 501) — a code the IAM model declares for no operation. The handler services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/reject_delegation_request.rs still returns crate::scaffolded::Scaffolded::not_implemented("RejectDelegationRequest") and carries the SCAFFOLDED marker, so neither the success path nor the NoSuchEntity path exists.

Why: a client branching on IAM's modelled error surface gets an error code no AWS SDK's IAM error set knows, so a rejected/unknown delegation request is indistinguishable from a transport or service outage; the whole delegation-request lifecycle (reject, then re-read to confirm nothing was fabricated) cannot be driven, and error handling written against NoSuchEntity/404 never fires.

Fix: the RejectDelegationRequest handler and its delegation-request state lookup — resolve DelegationRequestId against IAM's delegation-request store, return the modelled NoSuchEntityException (404 / NoSuchEntity) with the spaced not-found message when it resolves to nothing, and the Unit 200 success (marking the request rejected, with Notes recorded) when it does. Note the open issue 20261029T192356Z-iam-acceptdelegationrequest-acquirerole-associatedelegationrequest-answer-notimplemented-and-generatecredentialreport.md describes the same symptom for AcceptDelegationRequest / AcquireRole / AssociateDelegationRequest but does NOT name RejectDelegationRequest or carry this check's replay key, so it does not cover this.

Traces: iam.delegation-request-workflow-lifecycle (the check's earlier steps — CreateDelegationRequest's out-of-range SessionDuration ValidationError, GetDelegationRequest, ListDelegationRequests, UpdateDelegationRequest, AcceptDelegationRequest — got past their assertions in this run; the first divergence is at RejectDelegationRequest).

## Evidence
Replay of iam.delegation-request-workflow-lifecycle:
  File "check_iam_delegation_request_workflow_lifecycle.py", line 91, in run
    assert code_reject == "NoSuchEntity", (
  AssertionError: expected NoSuchEntity for RejectDelegationRequest, got NotImplemented

Expected (AWS): HTTP 404, Error.Code "NoSuchEntity", Error.Message "Delegation request not found: <DelegationRequestId>" — fixed text "Delegation request not found: ", variable part the DelegationRequestId from the request.
Actual (emulator): HTTP 501, Error.Code "NotImplemented", Error.Message "operation RejectDelegationRequest is scaffolded but not implemented in this emulator".
Source: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/reject_delegation_request.rs line 4 (SCAFFOLDED marker) and line 182 (Err(crate::scaffolded::Scaffolded::not_implemented("RejectDelegationRequest"))).

## Steps to Reproduce
1. Point an IAM client at the emulator.
2. Call iam:RejectDelegationRequest with a syntactically valid DelegationRequestId that was never created (matches ^[\w-]+$, 16..128 chars, e.g. "sim2real-behav-" + 32 hex chars) and no Notes.
3. Read the HTTP status and Error.Code/Error.Message. Observed: 501 / NotImplemented / "operation RejectDelegationRequest is scaffolded but not implemented in this emulator". Expected: 404 / NoSuchEntity / "Delegation request not found: <that id>".
4. Re-replay iam.delegation-request-workflow-lifecycle to confirm the fix; the check continues on to AssociateDelegationRequest and SendDelegationToken after Reject.
