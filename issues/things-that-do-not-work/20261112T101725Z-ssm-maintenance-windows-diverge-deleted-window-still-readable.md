Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-get-execution-task-and-invocation-not-found
Issue-Step: teardown#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-get-execution-task-and-invocation-not-found ssm-maintenance-window-execution-monitoring ssm-maintenance-window-lifecycle ssm-maintenance-window-lifecycle-and-schedule ssm-maintenance-window-schedule-workflow ssm-maintenance-window-invalid-cutoff-error
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|GetMaintenanceWindow|4xx->2xx|$.Message
Issue-Key: ssm|GetMaintenanceWindow|4xx->2xx|$.__type
Issue-Key: ssm|GetMaintenanceWindow|4xx->2xx|status
Issue-Key: ssm|CreateMaintenanceWindow|4xx->2xx|$.__type
Issue-Key: ssm|CreateMaintenanceWindow|4xx->2xx|$.message
Issue-Key: ssm|CreateMaintenanceWindow|4xx->2xx|status
Issue-Key: ssm|GetMaintenanceWindow|2xx->2xx|$.NextExecutionTime
Issue-Commit: 09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77

# ssm maintenance windows diverge: deleted window still readable, Cutoff not validated, NextExecutionTime truncated

> _Note: this issue was identified against the code base at commit `09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMaintenanceWindow

## Description
The emulator's SSM Maintenance Windows surface diverges from AWS on three independent counts: a window that no longer exists is still readable, CreateMaintenanceWindow performs no Cutoff-vs-Duration validation, and the window's NextExecutionTime is serialized at a coarser precision than AWS uses. This bundle carries 3 causes; a fixer must close all 3 before submitting. Smithy model used throughout: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json.

Cause 1 of 3 - GetMaintenanceWindow answers 200 with the stale window after it has been deleted, instead of DoesNotExistException (fixture ssm-get-execution-task-and-invocation-not-found, step teardown#5).
AWS: Smithy GetMaintenanceWindow takes a required WindowId (MaintenanceWindowId, length 20, pattern ^mw-[0-9a-f]{17}$) and declares error DoesNotExistException (errorKind client, httpError 400, awsQueryError code DoesNotExistException, single member Message), whose shape doc is "Error returned when the ID specified for a resource, such as a maintenance window or patch baseline, doesn't exist." Recorded behavior after the window is deleted: HTTP 400, __type "DoesNotExistException", Message = "Maintenance window <WindowId> does not exist" - fixed text "Maintenance window " ... " does not exist" (no trailing period), with the window id taken verbatim from the request's WindowId.
Emulator: returns HTTP 200 carrying the window record, with no __type and no Message - the read path performs no existence check for a removed window id.
Why: a deleted maintenance window stays visible forever, so teardown and idempotency logic that polls GetMaintenanceWindow until it refuses never terminates, and any client using the read to confirm deletion concludes the window still exists and skips recreating it.
Fix: the GetMaintenanceWindow handler must raise DoesNotExistException (400, code DoesNotExistException, Message composed from the requested WindowId) for a WindowId absent from the maintenance-window store, and DeleteMaintenanceWindow must actually remove the record from that store. Note the companion gap: open issue 20261111T143427Z-ssm-deletemaintenancewindow-stub.md tracks DeleteMaintenanceWindow being an unimplemented stub; that issue does not cover the read path, which must refuse an unknown WindowId regardless of how it became unknown.
Traces: ssm-get-execution-task-and-invocation-not-found#teardown#5 (cluster also covers ssm-maintenance-window-execution-monitoring#teardown#6, ssm-maintenance-window-lifecycle#teardown#18, ssm-maintenance-window-lifecycle-and-schedule#teardown#4, ssm-maintenance-window-schedule-workflow#teardown#9).

Cause 2 of 3 - CreateMaintenanceWindow accepts a Cutoff that is not smaller than Duration, returning 200 and a WindowId instead of ValidationException (fixture ssm-maintenance-window-invalid-cutoff-error, step task#1).
AWS: Smithy CreateMaintenanceWindow requires Duration (MaintenanceWindowDurationHours, range 1..24) and Cutoff (MaintenanceWindowCutoff, range 0..23, default 0). The per-member ranges are satisfied by the request in the recording, so this is a cross-member rule the service enforces on top of them: Cutoff must be strictly less than Duration. Recorded behavior when it is not: HTTP 400, __type "ValidationException" (Smithy ValidationException: errorKind client, httpError 400, awsQueryError code ValidationException, members Message and ReasonCode), message = "Cutoff was <Cutoff> but must be smaller than <Duration - 1> (Duration  - 1 )". The fixed text is "Cutoff was ", " but must be smaller than " and the trailing literal " (Duration  - 1 )" (note the AWS message's doubled space before the hyphen); the variable parts are the request's Cutoff value and the computed Duration - 1. In the recording the probe sent Cutoff 2 with Duration 1, so AWS emitted "Cutoff was 2 but must be smaller than 1 (Duration  - 1 )".
Emulator: returns HTTP 200 and mints a WindowId, with no __type and no message - the cross-member Cutoff/Duration rule is never evaluated.
Why: a window is created whose cutoff exceeds its duration, a state AWS will never produce, so a client's invalid configuration is silently accepted and a negative test that asserts AWS refuses the combination passes against the emulator while failing against AWS.
Fix: CreateMaintenanceWindow's input validation - add the Cutoff < Duration cross-member check after the per-member range checks and raise ValidationException with the message template above. UpdateMaintenanceWindow's equivalent path should be checked for the same gap.
Traces: ssm-maintenance-window-invalid-cutoff-error#task#1.

Cause 3 of 3 - GetMaintenanceWindow serializes NextExecutionTime at minute precision instead of AWS's full ISO-8601 instant (fixture ssm-maintenance-window-lifecycle, step task#2).
AWS: Smithy GetMaintenanceWindow output declares NextExecutionTime targeting MaintenanceWindowStringDateTime, a plain string shape with no timestampFormat trait - so the wire format is whatever the service writes into that string. Recorded behavior: a full ISO-8601 UTC instant with seconds and milliseconds and a 'Z' suffix, of the form YYYY-MM-DDTHH:MM:SS.sssZ. The instant itself is derived from the window's Schedule and ScheduleTimezone and differs per run; the requirement here is the FORMAT, not the recorded value.
Emulator: emits the string at minute precision with no seconds and no fractional part, of the form YYYY-MM-DDTHH:MMZ.
Why: NextExecutionTime is a string the client must parse. A consumer using a strict ISO-8601-with-seconds or AWS-datetime parser (the shape is a string, so SDKs hand it to application code verbatim) rejects or mis-parses the emulator's value, and schedulers that compare it to other SSM timestamps compare two different precisions.
Fix: the GetMaintenanceWindow serializer (and the shared maintenance-window next-execution-time formatter feeding DescribeMaintenanceWindows and the schedule APIs) - format the computed instant as a full UTC ISO-8601 timestamp with seconds and milliseconds, YYYY-MM-DDTHH:MM:SS.sssZ.
Traces: ssm-maintenance-window-lifecycle#task#2.

## Evidence
Cause 1 - ssm-get-execution-task-and-invocation-not-found#teardown#5, GetMaintenanceWindow, expected 400 -> actual 200 (5 affected steps): status expected 400, got 200; $.__type missing (expected 'DoesNotExistException', got None); $.Message missing (expected 'Maintenance window <WindowId> does not exist', recorded with the run's minted id mw-c3576fd088e039361, got None).

Cause 2 - ssm-maintenance-window-invalid-cutoff-error#task#1, CreateMaintenanceWindow, expected 400 -> actual 200 (1 affected step): status expected 400, got 200; $.__type missing (expected 'ValidationException', got None); $.message missing (expected 'Cutoff was 2 but must be smaller than 1 (Duration  - 1 )' for the recorded Cutoff=2 / Duration=1 request, got None).

Cause 3 - ssm-maintenance-window-lifecycle#task#2, GetMaintenanceWindow, 200 -> 200 (1 affected step): $.NextExecutionTime value_mismatch - expected a full ISO-8601 instant (recording: '2026-08-08T17:02:13.040Z'), got a minute-precision string ('2026-11-13T10:07Z'). The instant differs per run; the divergence is the missing seconds and milliseconds components.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture ssm-get-execution-task-and-invocation-not-found through teardown#5 - CreateMaintenanceWindow, DeleteMaintenanceWindow, then GetMaintenanceWindow with the same WindowId; expect 400 DoesNotExistException with Message 'Maintenance window <WindowId> does not exist', observe 200 with the window record.
3. Cause 2: replay fixture ssm-maintenance-window-invalid-cutoff-error step task#1 - CreateMaintenanceWindow with Duration=1 and Cutoff=2 (both inside their modelled ranges); expect 400 ValidationException with message 'Cutoff was 2 but must be smaller than 1 (Duration  - 1 )', observe 200 and a minted WindowId.
4. Cause 3: replay fixture ssm-maintenance-window-lifecycle step task#2 - CreateMaintenanceWindow with a cron Schedule, then GetMaintenanceWindow; inspect the NextExecutionTime string and check it carries seconds and milliseconds (YYYY-MM-DDTHH:MM:SS.sssZ).
