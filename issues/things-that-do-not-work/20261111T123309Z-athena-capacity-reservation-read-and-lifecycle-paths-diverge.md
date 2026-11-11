Issue-Source: trace-sweep-divergence
Issue-Trace: athena-capacity-reservation-validation-and-lookup
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-capacity-reservation-validation-and-lookup
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetCapacityReservation|2xx->2xx|$.CapacityReservation.CapacityLockTime
Issue-Key: athena|GetCapacityReservation|2xx->2xx|$.CapacityReservation.LastAllocation.LastAllocationStatus
Issue-Key: athena|GetCapacityReservation|2xx->2xx|$.CapacityReservation.LastAllocation.SubmissionTime
Issue-Key: athena|GetCapacityReservation|2xx->2xx|$.CapacityReservation.LastCapacityAllocation
Issue-Key: athena|GetCapacityReservation|4xx->4xx|$.Message
Issue-Key: athena|UpdateCapacityReservation|4xx->4xx|$.Message
Issue-Key: athena|CancelCapacityReservation|4xx->4xx|$.Message
Issue-Key: athena|DeleteCapacityReservation|4xx->4xx|$.Message
Issue-Key: athena|GetCapacityReservation|2xx->2xx|$.CapacityReservation.TargetDpus
Issue-Commit: 3f07c9f1d60993a38a36a4f215d4651495d2c52f

# Athena capacity-reservation read and lifecycle paths diverge: not-found refusal is reworded across four operations and GetCapacityReservation drops allocation state

> _Note: this issue was identified against the code base at commit `3f07c9f1d60993a38a36a4f215d4651495d2c52f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCapacityReservation

## Description
Six net-new divergences on the Athena capacity-reservation surface. They share one root area — the capacity-reservation state record and the refusal the four name-keyed capacity-reservation operations raise when that record is absent. This bundle carries 6 causes; all six must be closed before submitting.

Cause 1 of 6: GetCapacityReservation omits the reservation's allocation state.
  AWS: Smithy: GetCapacityReservation output member CapacityReservation @required, target shape CapacityReservation, which models member LastAllocation -> CapacityAllocation {Status @required (enum CapacityAllocationStatus: PENDING|SUCCEEDED|FAILED), StatusMessage, RequestTime @required, RequestCompletionTime}. The recording additionally shows members the public model does not declare — CapacityReservation.CapacityLockTime, CapacityReservation.LastCapacityAllocation {LastAllocationStatus, Status, SubmissionTime, RequestTime, RequestCompletionTime} and CapacityReservation.LastAllocation.{LastAllocationStatus, SubmissionTime}: recorded behavior only. Real Athena, once a reservation's DPUs have been allocated, reports a completed allocation (allocation status SUCCEEDED) plus the time the capacity was locked, and it populates both the modeled LastAllocation and the wire-only LastCapacityAllocation view of the same allocation.
  Emulator: returns 200 with CapacityReservation present but LastAllocation.LastAllocationStatus, LastAllocation.SubmissionTime, LastCapacityAllocation and CapacityLockTime all absent (null). The emulator never records an allocation attempt against the reservation at all, so there is no allocation to report.
  Why: a client that polls GetCapacityReservation to learn whether its requested DPUs were actually allocated — reading LastAllocation.Status / LastCapacityAllocation.LastAllocationStatus and waiting for SUCCEEDED, or reading CapacityLockTime to learn when the capacity became usable — never observes a terminal allocation and waits forever, or dereferences a null allocation struct.
  Fix: the capacity-reservation state (the reservation record must carry an allocation history entry created when the reservation is provisioned) and the GetCapacityReservation serializer, which must project that entry into LastAllocation, LastCapacityAllocation and CapacityLockTime.
  Traces: athena-capacity-reservation-validation-and-lookup#task#3.

Cause 2 of 6: GetCapacityReservation's unknown-reservation refusal message is emulator-authored.
  AWS: Smithy: GetCapacityReservation.Name @required, errors InvalidRequestException (client, HTTP 400). Recorded response: 400 with Message 'Reservation with name <Name> not found.' The fixed text is 'Reservation with name ' ... ' not found.'; the only variable part is the reservation name taken verbatim from the request's Name member.
  Emulator: 400 (status correct) with Message 'CapacityReservation <Name> was not found.' — different fixed text, different word order, and a different noun for the resource.
  Why: a client or test that matches Athena's refusal text to distinguish a missing reservation from another InvalidRequestException cause (this error shape is shared by validation failures too, so the message is the only discriminator) fails to classify the error and surfaces it as an unknown fault.
  Fix: the GetCapacityReservation handler's not-found refusal path, where the message is composed.
  Traces: athena-capacity-reservation-validation-and-lookup#task#4 (and task#13).

Cause 3 of 6: UpdateCapacityReservation's unknown-reservation refusal message is emulator-authored.
  AWS: Smithy: UpdateCapacityReservation.Name @required (@length 1..128, @pattern ^[a-zA-Z0-9._-]+$), TargetDpus @required, errors InvalidRequestException (client, HTTP 400). Recorded response: 400 with Message 'Reservation with name <Name> not found.' — same fixed text as cause 2, variable part is the request's Name.
  Emulator: 400 with Message 'CapacityReservation <Name> was not found.'
  Why: same as cause 2 — the refusal is indistinguishable from a validation refusal because its text does not match Athena's, so retry/abort logic keyed on the message misroutes.
  Fix: the UpdateCapacityReservation handler's not-found refusal path.
  Traces: athena-capacity-reservation-validation-and-lookup#task#5.

Cause 4 of 6: CancelCapacityReservation's unknown-reservation refusal message is emulator-authored.
  AWS: Smithy: CancelCapacityReservation.Name @required, @idempotent, errors InvalidRequestException (client, HTTP 400). Recorded response: 400 with Message 'Reservation with name <Name> not found.'
  Emulator: 400 with Message 'CapacityReservation <Name> was not found.'
  Why: as cause 2. A cleanup path that treats Athena's 'not found' wording as already-cancelled (benign) instead aborts on an unrecognized message.
  Fix: the CancelCapacityReservation handler's not-found refusal path.
  Traces: athena-capacity-reservation-validation-and-lookup#task#8.

Cause 5 of 6: DeleteCapacityReservation's unknown-reservation refusal message is emulator-authored.
  AWS: Smithy: DeleteCapacityReservation.Name @required, @idempotent, errors InvalidRequestException (client, HTTP 400). Recorded response: 400 with Message 'Reservation with name <Name> not found.'
  Emulator: 400 with Message 'CapacityReservation <Name> was not found.'
  Why: as cause 2; an idempotent teardown that swallows Athena's not-found wording fails instead.
  Fix: the DeleteCapacityReservation handler's not-found refusal path. Causes 2-5 share one message template, so the fix belongs in one shared refusal constructor used by all four name-keyed capacity-reservation operations.
  Traces: athena-capacity-reservation-validation-and-lookup#task#9.

Cause 6 of 6: a cancelled reservation keeps its requested DPU count instead of dropping to zero, and still reports no allocation.
  AWS: Smithy: CapacityReservation.TargetDpus @required, target TargetDpusInteger @range min 4; LastAllocation -> CapacityAllocation. Recorded behavior: after CancelCapacityReservation the reservation is read back with TargetDpus 0 — Athena releases the requested capacity on cancellation, so the post-cancel value falls outside the request-time @range min of 4 — while LastAllocation / LastCapacityAllocation still report the last completed allocation (status SUCCEEDED) and CapacityLockTime, so the allocation history survives cancellation.
  Emulator: reads back TargetDpus unchanged at its requested value (the recording's run requested 4, so the emulator answered 4 where AWS answered 0) and, as in cause 1, omits LastAllocation.LastAllocationStatus, LastAllocation.SubmissionTime, LastCapacityAllocation and CapacityLockTime entirely.
  Why: a client that reads TargetDpus to confirm a cancel actually released capacity, or that bills/accounts against reported target DPUs, sees capacity that is still reserved after a successful cancellation; and it cannot see the retained allocation history.
  Fix: the CancelCapacityReservation handler's state transition (it must zero the reservation's target DPU count while retaining the allocation entry) plus the same GetCapacityReservation projection named in cause 1.
  Traces: athena-capacity-reservation-validation-and-lookup#task#11.

No open issue covers this surface: the only athena capacity-reservation issue on file is 20261104T163600Z-athena-listcapacityreservations-stub.md, which covers the unimplemented ListCapacityReservations operation and not the read/lifecycle behavior above.

## Evidence
Replay of trace set simparity-wire-aws-v1 (fixture athena-capacity-reservation-validation-and-lookup), all six clusters with no owning issue and no blocking stub:

Cause 1 — GetCapacityReservation, step task#3, expected 200 / actual 200:
  $.CapacityReservation.CapacityLockTime: missing (expected a timestamp, got None)
  $.CapacityReservation.LastAllocation.LastAllocationStatus: missing (expected 'SUCCEEDED', got None)
  $.CapacityReservation.LastAllocation.SubmissionTime: missing (expected a timestamp, got None)
  $.CapacityReservation.LastCapacityAllocation: missing (expected {'LastAllocationStatus': 'SUCCEEDED', 'Status': 'SUCCEEDED', 'RequestTime': <ts>, 'RequestCompletionTime': <ts>, 'SubmissionTime': <ts>}, got None)

Cause 2 — GetCapacityReservation, step task#4 (cluster also covers task#13), expected 400 / actual 400:
  $.Message: expected 'Reservation with name <name> not found.'
                  got 'CapacityReservation <name> was not found.'

Cause 3 — UpdateCapacityReservation, step task#5, expected 400 / actual 400:
  $.Message: expected 'Reservation with name <name> not found.'
                  got 'CapacityReservation <name> was not found.'

Cause 4 — CancelCapacityReservation, step task#8, expected 400 / actual 400:
  $.Message: expected 'Reservation with name <name> not found.'
                  got 'CapacityReservation <name> was not found.'

Cause 5 — DeleteCapacityReservation, step task#9, expected 400 / actual 400:
  $.Message: expected 'Reservation with name <name> not found.'
                  got 'CapacityReservation <name> was not found.'

Cause 6 — GetCapacityReservation, step task#11, expected 200 / actual 200:
  $.CapacityReservation.TargetDpus: value_mismatch (expected 0, got the requested DPU count)
  $.CapacityReservation.CapacityLockTime: missing (got None)
  $.CapacityReservation.LastAllocation.LastAllocationStatus: missing (expected 'SUCCEEDED', got None)
  $.CapacityReservation.LastAllocation.SubmissionTime: missing (got None)
  $.CapacityReservation.LastCapacityAllocation: missing (got None)

In every refusal above the HTTP status (400) and the error shape match; only the message text diverges. In the 200 responses the status matches and the payload is short modeled and wire-recorded members. (Recorded reservation names, timestamps and request ids are per-run values and are not part of the requirement.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1, fixture athena-capacity-reservation-validation-and-lookup.

Or by hand against the emulator's Athena endpoint:

A. Causes 1 and 6 (allocation state and post-cancel TargetDpus)
   1. CreateCapacityReservation with Name=<res>, TargetDpus=4.
   2. GetCapacityReservation Name=<res> -> observe 200 whose CapacityReservation has no LastCapacityAllocation, no CapacityLockTime, and a LastAllocation with no LastAllocationStatus/SubmissionTime. AWS reports a SUCCEEDED allocation and a capacity-lock time (cause 1, step task#3).
   3. CancelCapacityReservation Name=<res>, then GetCapacityReservation Name=<res> -> observe TargetDpus still 4 where AWS reports 0, and the allocation members still absent (cause 6, step task#11).

B. Causes 2-5 (not-found refusal wording). Pick a name that satisfies the modeled pattern ^[a-zA-Z0-9._-]+$ and length 1..128 but names no existing reservation, then call each of:
   - GetCapacityReservation    (cause 2, step task#4)
   - UpdateCapacityReservation (with any TargetDpus >= 4) (cause 3, step task#5)
   - CancelCapacityReservation (cause 4, step task#8)
   - DeleteCapacityReservation (cause 5, step task#9)
   Each returns 400 InvalidRequestException with Message 'CapacityReservation <name> was not found.'; AWS returns 'Reservation with name <name> not found.'
