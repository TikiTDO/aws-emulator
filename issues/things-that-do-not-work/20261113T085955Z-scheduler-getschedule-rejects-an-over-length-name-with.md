Issue-Source: trace-sweep-divergence
Issue-Trace: scheduler-get-schedule-name-too-long-rejected
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: scheduler-get-schedule-name-too-long-rejected
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler|GetSchedule|4xx->4xx|$.Message
Issue-Commit: 140723b65076e66e2cb90e34db509407cbbeb018

# scheduler GetSchedule rejects an over-length Name with an empty ValidationException body

> _Note: this issue was identified against the code base at commit `140723b65076e66e2cb90e34db509407cbbeb018`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSchedule

## Description
One root cause in the EventBridge Scheduler wire surface, found by replaying simparity-wire-aws-v1 against the emulator.

Cause 1 of 1: GetSchedule's @length validation of the Name path label produces the right HTTP status but no error message at all, so the required Message member is absent from the ValidationException body.

AWS: GetSchedule binds Name as a required httpLabel whose target is constrained (Smithy, services/service-com-amazonaws-scheduler-awschronosservice: GetSchedule.Name -> Name, required, httpLabel, @length min 1 max 64, @pattern ^[0-9a-zA-Z-_.]+$; GetSchedule errors include ValidationException with httpError 400, and ValidationException.Message is required). A Name longer than the modeled maximum is refused with HTTP 400 ValidationException whose Message is rendered in this service's standard constraint form. Its FIXED text is "1 validation error detected: Value '" + "' at '" + "' failed to satisfy constraint: Member must have length less than or equal to "; its VARIABLE parts are the value quoted straight from the request (the submitted Name), the member's wire path, and the modeled bound. For this member the path is the fixed token 'name' and the bound is the fixed number 64, because both come from the model rather than from the request; only the quoted Name varies per call. The recorded response carries exactly that body.

Emulator: answers the correct status (HTTP 400) but the JSON body has no $.Message key at all (absent, not null) — the constraint is detected and the response is otherwise shaped correctly, only the diagnostic payload is dropped.

Why: every AWS SDK surfaces ValidationException with an empty message, so a caller is told only that its request was invalid and cannot learn WHICH member violated WHICH bound. Code that logs or matches on the validation text (the normal way to distinguish a client-side name-length bug from a server-side rejection) sees nothing, and a client that retries on unparseable errors retries a request that can never succeed.

Fix: the ValidationException construction for @length violations on GetSchedule's path label — the scheduler constraint validator that builds the "1 validation error detected: ..." text (src/ops/types.rs) as reached from the input validation in src/ops/get_schedule.rs, and/or the error serializer that writes the exception body. The fix must emit the Message for the length class of constraint on this operation, not only the status.

Related but NOT covering: the open issue 20261030T010351Z-scheduler-wire-responses-drop-validationexception-messages-modeled-getschedule.md files the same empty-Message fault for CreateSchedule's @range check, ListSchedules' MaxResults @range check and TagResource's ResourceArn @pattern check, and carries none of this fixture step's replay key; the @length path on GetSchedule is unowned, and this divergence is reported with no owning issue by the replay.

Traces: scheduler-get-schedule-name-too-long-rejected#task#1.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up true, /__admin/health 200): 122 steps, 106 passed, 16 mismatched, score 86.9%, 10 divergence clusters; nine of them are owned by existing open issues and are excluded. This cluster:

GetSchedule, expected status 400 / actual status 400, 1 affected step.
  $.Message: missing (expected "1 validation error detected: Value '<submitted Name>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 64", got None)

The recorded step submitted a 66-character schedule name; in the expected message the quoted value is that request's Name, while 'name' and '64' are fixed text taken from the model (Name @length max 64). No other key in the response diverged, and no header diverged — the status and error class are already right.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a scheduler client at its data listener.
2. Call GetSchedule with a Name longer than 64 characters, e.g. a 66-character name built from the allowed character class ([0-9a-zA-Z-_.]), with no GroupName.
3. Observe the response: HTTP 400 with a body carrying no Message member.
   Expected (AWS): HTTP 400 ValidationException whose body is {"Message": "1 validation error detected: Value '<the Name you sent>' at 'name' failed to satisfy constraint: Member must have length less than or equal to 64"}.
4. Equivalent replay: replay fixture scheduler-get-schedule-name-too-long-rejected, step task#1.
