Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: partnercentral-account.list-connection-invitations-invalid-status-filter-rejected
Issue-Shape-Id: com.amazonaws.partnercentralaccount#PartnerCentralAccount
Issue-Key: partnercentral-account.list-connection-invitations-invalid-status-filter-rejected|run#5|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: a36b120d1ec2ecc9379e4b38fe0d35161c55eb57

# ListConnectionInvitations enum-constraint ValidationException message echoes the rejected value and lists the enum value set in model order, not AWS's wording

> _Note: this issue was identified against the code base at commit `a36b120d1ec2ecc9379e4b38fe0d35161c55eb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListConnectionInvitations

## Description
Cause 1 of 1: the ValidationException message the emulator produces when an input member constrained to an enum receives an out-of-set value does not match the message AWS produces for that error class. Two parts of the template diverge: the value clause and the ordering of the enum value set.

AWS: `ListConnectionInvitations$Status` targets `com.amazonaws.partnercentralaccount#InvitationStatus`, an enum whose members are `PENDING, ACCEPTED, REJECTED, CANCELED, EXPIRED` (Smithy: ListConnectionInvitations input member `Status` -> `InvitationStatus`, type enum; operation models `ValidationException` HTTP 400). The model fixes the error code and status but says nothing about message text, so the message is recorded behavior only. The recording shows AWS answering HTTP 400 `ValidationException` with exactly:

    1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [EXPIRED, ACCEPTED, PENDING, CANCELED, REJECTED]

In that template the fixed text is `1 validation error detected: Value at '` + `' failed to satisfy constraint: Member must satisfy enum value set: [` + `]`. The variable parts are the member's wire name, lowercased as AWS reports it for this operation (`status`), and the comma-separated enum value set. AWS does NOT interpolate the rejected value into this message for this member — the clause is `Value at 'status'`, with no `'<value>'` between `Value` and `at`. AWS also does not emit the value set in the model's declaration order; the recorded order for `InvitationStatus` is `EXPIRED, ACCEPTED, PENDING, CANCELED, REJECTED`. Hypothesis: the service's validation framework enumerates the enum's values from a hash-ordered set and suppresses echoing a caller-supplied value for this member; only the recorded order and the absence of the value clause are facts.

Emulator: the same call, `ListConnectionInvitations(Catalog="AWS", Status="NOT_A_REAL_STATUS")`, is refused with the right status (400) and the right code (`ValidationException`), but the message is:

    1 validation error detected: Value 'NOT_A_REAL_STATUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [PENDING, ACCEPTED, REJECTED, CANCELED, EXPIRED]

i.e. it (a) inserts the rejected value as `Value 'NOT_A_REAL_STATUS' at 'status'` where AWS writes `Value at 'status'`, and (b) renders the value set in Smithy declaration order `[PENDING, ACCEPTED, REJECTED, CANCELED, EXPIRED]` instead of AWS's `[EXPIRED, ACCEPTED, PENDING, CANCELED, REJECTED]`.

Why: clients and test suites that assert on AWS's validation text — error-classification middleware, SDK retry/branching wrappers, user-facing surfacing of `Error.Message`, and contract tests pinned to recorded AWS strings — see a message that never occurs on AWS. Any consumer matching the `Value at '<member>'` form fails to classify the error, and any consumer comparing the enum value set verbatim mismatches. The echoed value is additionally a behavioral difference in kind, not just in text: AWS deliberately does not reflect the rejected enum value back in this message, so the emulator discloses request content in a field AWS leaves out.

Fix: the shared request-validation layer that formats enum-constraint failures into `ValidationException.message` for this crate (the framework-driven deserialize -> validate step that runs before `handle`; the handler in services/service-com-amazonaws-partnercentralaccount-partnercentralaccount/src/ops/list_connection_invitations.rs is never reached for this input, and the hand-written ValidationExceptions in it are a different, unaffected path). The enum-constraint message builder must drop the `'<value>' ` clause for enum-set violations and emit the value set in AWS's recorded order for `InvitationStatus` rather than Smithy member order. The correction belongs in that one formatter, so every enum-constrained member of the service (`ConnectionType`, `ParticipantType`, `InvitationStatus`) is worded the same way.

Traces: partnercentral-account.list-connection-invitations-invalid-status-filter-rejected (the check's positive control, `Status="PENDING"`, succeeded and returned a well-formed list; only the refusal's message diverged).

## Evidence
Check: partnercentral-account.list-connection-invitations-invalid-status-filter-rejected [mismatch]

Request: ListConnectionInvitations(Catalog="AWS", Status="NOT_A_REAL_STATUS") against us-east-1.

Status/code: both sides answer HTTP 400 with Error.Code == "ValidationException" — the divergence is only Error.Message.

Expected (AWS):
  "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [EXPIRED, ACCEPTED, PENDING, CANCELED, REJECTED]"

Actual (emulator):
  "1 validation error detected: Value 'NOT_A_REAL_STATUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [PENDING, ACCEPTED, REJECTED, CANCELED, EXPIRED]"

AssertionError raised at check_partnercentral_account_list_connection_invitations_invalid_status_filter_rejected.py line 47: `assert message == expected`.

Template breakdown — fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]". Variable from the request: the member name as AWS reports it, `status`. Variable from the model/state: the enum value set, recorded as `EXPIRED, ACCEPTED, PENDING, CANCELED, EXPIRED`-ordered list `[EXPIRED, ACCEPTED, PENDING, CANCELED, REJECTED]`. The rejected value `NOT_A_REAL_STATUS` is NOT part of the template; it appears only in the emulator's output and is specific to this one request.

Smithy (services/service-com-amazonaws-partnercentralaccount-partnercentralaccount/smithy-model/com-amazonaws-partnercentralaccount-partnercentralaccount.json):
  ListConnectionInvitations input member Status -> InvitationStatus (enum, values PENDING, ACCEPTED, REJECTED, CANCELED, EXPIRED); operation errors include ValidationException (client, httpError 400).

Positive control in the same check passed: ListConnectionInvitations(Catalog="AWS", Status="PENDING") returned a list, so the operation itself is implemented and only the enum-rejection wording is wrong.

## Steps to Reproduce
1. Boot the emulator and point a botocore client at it: client = session.client("partnercentral-account", region_name="us-east-1", endpoint_url=<emulator>).
2. Call client.list_connection_invitations(Catalog="AWS", Status="NOT_A_REAL_STATUS").
3. Catch botocore.exceptions.ClientError and read e.response["Error"]["Code"] and e.response["Error"]["Message"].
4. Observe Code == "ValidationException" (correct) but Message == "1 validation error detected: Value 'NOT_A_REAL_STATUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [PENDING, ACCEPTED, REJECTED, CANCELED, EXPIRED]", where AWS answers "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [EXPIRED, ACCEPTED, PENDING, CANCELED, REJECTED]" — the rejected value must not be echoed, and the enum value set must be in AWS's recorded order.
5. Re-replay partnercentral-account.list-connection-invitations-invalid-status-filter-rejected to confirm the fix; its positive control (Status="PENDING") must keep succeeding.
