Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.capacity-reservation-delete-before-cancel-rejected athena.delete-capacity-reservation-while-active-fails athena.delete-capacity-reservation-without-cancel-fails athena.update-capacity-reservation-unknown-name-rejected athena.capacity-reservation-lifecycle-create-update-delete athena.update-capacity-reservation-below-minimum-rejected athena.update-capacity-reservation-negative-target-dpus-rejected athena.capacity-reservation-update-below-minimum-rejected athena.put-capacity-assignment-configuration-invalid-workgroup-fails
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.capacity-reservation-delete-before-cancel-rejected|run#10|AssertionError|assert message == (
Issue-Key: athena.delete-capacity-reservation-while-active-fails|run#11|AssertionError|assert message == (
Issue-Key: athena.delete-capacity-reservation-without-cancel-fails|run#8|AssertionError|assert message == (
Issue-Key: athena.update-capacity-reservation-unknown-name-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: athena.capacity-reservation-lifecycle-create-update-delete|run#22|AssertionError|assert message == not_found, repr(message)
Issue-Key: athena.update-capacity-reservation-below-minimum-rejected|run#10|AssertionError|assert err == expected, f"expected {expected!r}, got {err!r}"
Issue-Key: athena.update-capacity-reservation-negative-target-dpus-rejected|run#10|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: athena.capacity-reservation-update-below-minimum-rejected|run#12|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: athena.put-capacity-assignment-configuration-invalid-workgroup-fails|run#10|AssertionError|assert code == "InvalidRequestException", (
Issue-Commit: 115da0938d5a480a5bdac5ab944971a99ada7e73

# Athena capacity-reservation lifecycle refusals diverge: delete-state and not-found messages are emulator-authored, TargetDpus constraint refusals raise ValidationException, and PutCapacityAssignmentConfiguration is NotImplemented (4 causes)

> _Note: this issue was identified against the code base at commit `115da0938d5a480a5bdac5ab944971a99ada7e73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCapacityReservation

## Description
Four net-new root causes on Athena's capacity-reservation control plane, all reached through the behavioral suite. This bundle carries 4 causes; ALL FOUR must be closed before submitting.

Cause 1 of 4: DeleteCapacityReservation's "not cancelled yet" refusal does not name the state it met.
  AWS: Smithy (services/service-com-amazonaws-athena-amazonathena/smithy-model/com-amazonaws-athena-amazonathena.json): DeleteCapacityReservation.Name @required (@length 1..128, @pattern ^[a-zA-Z0-9._-]+$), @idempotent, errors InvalidRequestException (client, HTTP 400) and InternalServerException (500); the operation doc states "Deletes a cancelled capacity reservation. A reservation must be cancelled before it can be deleted." Recorded behavior: 400 InvalidRequestException whose Message is "Reservation state is <STATUS>. Reservation can only be deleted when state is CANCELLED". Fixed text: "Reservation state is " … ". Reservation can only be deleted when state is CANCELLED"; the single variable part is the reservation's CURRENT status read from the emulator's own state (the checks exercise it at ACTIVE, but PENDING/UPDATE_PENDING/CANCELLING must each report themselves).
  Emulator: refuses with the right status (400) and the right code (InvalidRequestException) but Message "Reservation must be cancelled before it can be deleted" — a constant that borrows the operation's documentation sentence and names no state at all.
  Why: a client that reads the refusal to decide what to do next (wait out PENDING/CANCELLING, or issue CancelCapacityReservation first) cannot tell which state blocked the delete, and teardown logic keyed on AWS's wording never matches, so an orderly cancel-then-delete loop aborts on an unrecognized message.
  Fix: the DeleteCapacityReservation handler's not-cancelled refusal path and the shared message constructor it calls (crate::state::capacity_reservation_not_cancelled, which takes no status argument today and therefore cannot render the state).
  Traces: athena.capacity-reservation-delete-before-cancel-rejected, athena.delete-capacity-reservation-while-active-fails, athena.delete-capacity-reservation-without-cancel-fails. Code: services/service-com-amazonaws-athena-amazonathena/src/ops/delete_capacity_reservation.rs lines 156-161 (status check) and 178-180 (refusal constructor).

Cause 2 of 4: the unknown-reservation refusal message is emulator-authored on the name-keyed capacity-reservation operations.
  AWS: Smithy: GetCapacityReservation.Name / UpdateCapacityReservation.Name @required, each modeling InvalidRequestException (client, HTTP 400) as its only client error. Recorded behavior: 400 InvalidRequestException with Message "Reservation with name <Name> not found." Fixed text: "Reservation with name " … " not found."; the only variable part is the Name member copied verbatim from the request. A reservation that was created and then deleted is indistinguishable from one that never existed, so the same message must be served after a successful delete.
  Emulator: 400 InvalidRequestException with Message "CapacityReservation <Name> was not found." — different fixed text, different word order, different noun for the resource.
  Why: InvalidRequestException is the ONLY client error these operations model, so the message is the sole discriminator between "no such reservation" and a validation refusal. A client that classifies on it (an idempotent teardown that swallows not-found, a poller that stops when the reservation is gone) misroutes the error and surfaces it as an unknown fault.
  Fix: the shared not-found refusal constructor used by the name-keyed capacity-reservation operations (crate::state::capacity_reservation_not_found) — one template feeds all of them.
  NOTE for the fixer: this is the same root cause as causes 2-5 of the already-open WIRE issue 20261111T123309Z-athena-capacity-reservation-read-and-lifecycle-paths-diverge.md, which carries it for GetCapacityReservation/UpdateCapacityReservation/CancelCapacityReservation/DeleteCapacityReservation. That issue did not carry these behavioral check ids; one fix closes both, but re-replay the checks listed here as well.
  Traces: athena.update-capacity-reservation-unknown-name-rejected (UpdateCapacityReservation and GetCapacityReservation on a never-created Name), athena.capacity-reservation-lifecycle-create-update-delete (GetCapacityReservation and UpdateCapacityReservation after the reservation was deleted). Code: services/service-com-amazonaws-athena-amazonathena/src/ops/update_capacity_reservation.rs lines 147-154.

Cause 3 of 4: UpdateCapacityReservation's TargetDpus constraint refusals are raised as an unmodeled ValidationException instead of InvalidRequestException.
  AWS: Smithy: UpdateCapacityReservation input TargetDpus @required, target TargetDpusInteger with @range min 4; the operation models exactly two errors — InvalidRequestException (client, HTTP 400) and InternalServerException (500). ValidationException is NOT in this operation's error list and is not a shape of this service's error set. Recorded behavior: a below-minimum, non-multiple-of-4 TargetDpus (e.g. 2) is refused 400 InvalidRequestException with Message "Capacity must be requested in increments of 4." (entirely fixed text, no request values); TargetDpus=0 is refused 400 InvalidRequestException with Message "1 validation error detected: Value at 'targetDpus' failed to satisfy constraint: Member must have value greater than or equal to 1" — fixed text with the member name 'targetDpus' and the limit as AWS's frontend states it (1), and with NO echoed value.
  Emulator: refuses with error code "ValidationException" and Message "1 validation error detected: Value '<N>' at 'targetDpus' failed to satisfy constraint: Member must have value greater than or equal to 4" — an error code the operation does not model, plus a message that echoes the submitted value and quotes the modeled minimum (4) rather than AWS's wording for either class. The refusal is produced by the centralized deserialize→validate step from the schema's @range trait, before the handler runs, so the handler never gets to classify it.
  Why: a client with a generated Athena model cannot deserialize an error shape the operation does not declare; botocore surfaces it as a generic ClientError with Code "ValidationException", so every except-InvalidRequestException branch (retry, user-facing "adjust your DPU request" handling) is skipped and the call looks like an unknown fault. It also hides the increments-of-4 rule, which is a different refusal from the minimum-value one.
  Fix: the operation's validation layer — the schema-driven @range refusal for Athena must be emitted as the operation's modeled InvalidRequestException (HTTP 400) with Athena's wording, and UpdateCapacityReservation's handler/validation must distinguish the increments-of-4 rule from the minimum-value rule.
  NOTE for the fixer: the already-open issue 20261104T163600Z-athena-constraint-refusals-raise-an-unmodeled-validationexception-instead.md carries this class for other Athena operations (including the CreateCapacityReservation side). It does not carry UpdateCapacityReservation or these check ids; if the shared validation seam is fixed there, re-replay the checks listed here to confirm.
  Traces: athena.update-capacity-reservation-below-minimum-rejected (TargetDpus=2: expected ('InvalidRequestException','Capacity must be requested in increments of 4.'), got ('ValidationException', "1 validation error detected: Value '2' at 'targetDpus' ... greater than or equal to 4")), athena.update-capacity-reservation-negative-target-dpus-rejected (TargetDpus=0), athena.capacity-reservation-update-below-minimum-rejected (TargetDpus=1). Code: services/service-com-amazonaws-athena-amazonathena/src/ops/update_capacity_reservation.rs — the handler at lines 130-173 performs no TargetDpus validation of its own.

Cause 4 of 4: PutCapacityAssignmentConfiguration answers NotImplemented, so its workgroup-existence refusal and its persistence contract never happen.
  AWS: Smithy: PutCapacityAssignmentConfiguration.CapacityReservationName @required (@length 1..128, @pattern ^[a-zA-Z0-9._-]+$), CapacityAssignments @required (list), @idempotent, errors InvalidRequestException (client, HTTP 400) and InternalServerException (500); empty output. Recorded behavior: an assignment naming a workgroup that does not exist is refused 400 InvalidRequestException with Message "One or more WorkGroups are not found." — entirely fixed text, no request values interpolated — and nothing is stored, so a subsequent GetCapacityAssignmentConfiguration for that reservation succeeds (200) and reports no assignment containing that name. With an existing workgroup the call succeeds and GetCapacityAssignmentConfiguration reads the assignment back.
  Emulator: every call is refused with error code "NotImplemented" ("operation PutCapacityAssignmentConfiguration is scaffolded but not implemented in this emulator"), so neither the refusal nor the accepted write exists.
  Why: a client cannot assign a workgroup to a capacity reservation at all; the negative path it relies on (a typo'd workgroup name being rejected before anything is persisted) returns an error code the operation does not model, and the positive path never persists, so capacity-assignment configuration is unobservable.
  Fix: implement the PutCapacityAssignmentConfiguration handler (services/service-com-amazonaws-athena-amazonathena/src/ops/put_capacity_assignment_configuration.rs) — resolve the reservation, validate every WorkGroupNames entry against the workgroup registry before any mutation, refuse with the modeled InvalidRequestException and AWS's text otherwise, and store the assignments in the capacity-reservation state so GetCapacityAssignmentConfiguration projects them.
  NOTE for the fixer: the suite also reports this stub as blocking athena.capacity-assignment-configuration-governs-workgroup-execution, for which a bare "implement the stub" issue is filed automatically; this cause adds the behavioral contract (refusal code, exact message, no partial write) that the stub issue does not state.
  Traces: athena.put-capacity-assignment-configuration-invalid-workgroup-fails (expected InvalidRequestException, got NotImplemented).

## Evidence
Behavioral suite simparity-behavioral-aws-v1, all failures listed under the report heading `### athena`. Nine failing checks, none marked ALREADY FILED and none marked OWNED, clustered into the four causes above.

Cause 1 — DeleteCapacityReservation while the reservation is ACTIVE (status and code both correct, message diverges):
  expected Message: 'Reservation state is ACTIVE. Reservation can only be deleted when state is CANCELLED'
       got Message: 'Reservation must be cancelled before it can be deleted'
  athena.capacity-reservation-delete-before-cancel-rejected — AssertionError at check line 48 on the message assertion.
  athena.delete-capacity-reservation-while-active-fails — AssertionError at check line 58: 'Reservation must be cancelled before it can be deleted'.
  athena.delete-capacity-reservation-without-cancel-fails — AssertionError at check line 35: 'Reservation must be cancelled before it can be deleted'.

Cause 2 — unknown / deleted reservation name (status 400 and code InvalidRequestException correct, message diverges):
  expected Message: 'Reservation with name <name> not found.'
       got Message: 'CapacityReservation <name> was not found.'
  athena.update-capacity-reservation-unknown-name-rejected — AssertionError at check line 35: expected message 'Reservation with name <name> not found.', got 'CapacityReservation <name> was not found.' (UpdateCapacityReservation on a never-created Name).
  athena.capacity-reservation-lifecycle-create-update-delete — AssertionError at check line 63: 'CapacityReservation <name> was not found.' where GetCapacityReservation on a just-deleted reservation must answer 'Reservation with name <name> not found.'
  (The reservation names in these runs are per-run unique_name values and are not part of the requirement; only the template is.)

Cause 3 — UpdateCapacityReservation TargetDpus constraint refusals carry an unmodeled error code:
  athena.update-capacity-reservation-below-minimum-rejected — AssertionError at check line 64: expected ('InvalidRequestException', 'Capacity must be requested in increments of 4.'), got ('ValidationException', "1 validation error detected: Value '2' at 'targetDpus' failed to satisfy constraint: Member must have value greater than or equal to 4").
  athena.update-capacity-reservation-negative-target-dpus-rejected — AssertionError at check line 59: expected TargetDpus=0 update to be rejected with InvalidRequestException, got ValidationException (AWS message: "1 validation error detected: Value at 'targetDpus' failed to satisfy constraint: Member must have value greater than or equal to 1").
  athena.capacity-reservation-update-below-minimum-rejected — AssertionError at check line 58: expected TargetDpus=1 to be rejected with InvalidRequestException, got 'ValidationException': "1 validation error detected: Value '1' at 'targetDpus' failed to satisfy constraint: Member must have value greater than or equal to 4".
  All three calls were issued with botocore parameter_validation disabled, so the refusal graded is the emulator's, not the SDK's.

Cause 4 — PutCapacityAssignmentConfiguration:
  athena.put-capacity-assignment-configuration-invalid-workgroup-fails — AssertionError at check line 44: PutCapacityAssignmentConfiguration with nonexistent workgroup: expected InvalidRequestException, got NotImplemented. AWS answers 400 InvalidRequestException 'One or more WorkGroups are not found.' and stores nothing.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator, then replay the behavioral suite simparity-behavioral-aws-v1 and look at the nine checks named under `trace_ids`.

Or by hand against the emulator's Athena endpoint (any region/credentials; use a client with parameter validation disabled for the TargetDpus cases, because botocore's generated model carries the @range min 4 and would refuse locally):

A. Cause 1 — CreateCapacityReservation Name=<res>, TargetDpus=4; poll GetCapacityReservation until Status=ACTIVE; call DeleteCapacityReservation Name=<res>. Observe 400 InvalidRequestException 'Reservation must be cancelled before it can be deleted'; AWS answers 'Reservation state is ACTIVE. Reservation can only be deleted when state is CANCELLED'. The reservation must still be readable and unchanged afterwards (it is).

B. Cause 2 — pick a name matching ^[a-zA-Z0-9._-]+$ (length 1..128) that names no reservation and call UpdateCapacityReservation Name=<bogus>, TargetDpus=8, then GetCapacityReservation Name=<bogus>. Observe 400 InvalidRequestException 'CapacityReservation <bogus> was not found.'; AWS answers 'Reservation with name <bogus> not found.' Repeat after cancelling and deleting a real reservation: the deleted name must answer the same AWS text.

C. Cause 3 — with an ACTIVE reservation <res> (wait out the post-ACTIVE update cooldown), call UpdateCapacityReservation Name=<res> with TargetDpus=0, 1 and 2 using an unvalidating client. Each returns error code ValidationException; AWS returns InvalidRequestException — 'Capacity must be requested in increments of 4.' for 2, and "1 validation error detected: Value at 'targetDpus' failed to satisfy constraint: Member must have value greater than or equal to 1" for 0. TargetDpus must be left unchanged either way.

D. Cause 4 — CreateWorkGroup Name=<wg>; CreateCapacityReservation Name=<res>, TargetDpus=4; call PutCapacityAssignmentConfiguration CapacityReservationName=<res>, CapacityAssignments=[{WorkGroupNames:[<nonexistent-wg>]}]. Observe error code NotImplemented; AWS answers 400 InvalidRequestException 'One or more WorkGroups are not found.' and GetCapacityAssignmentConfiguration for <res> then returns 200 with no assignment naming it. Repeating the Put with <wg> must succeed and be readable back through GetCapacityAssignmentConfiguration.
