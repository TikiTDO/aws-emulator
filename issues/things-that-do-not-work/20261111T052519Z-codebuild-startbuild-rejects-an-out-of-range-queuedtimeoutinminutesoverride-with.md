Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.build-queued-timeout-override-reported-on-started-build
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.build-queued-timeout-override-reported-on-started-build|run#10|AssertionError|assert message == expected, (
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# StartBuild rejects an out-of-range queuedTimeoutInMinutesOverride with the generic constraint-validation message instead of AWS's queued-timeout wording

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBuild

## Description
ONE net-new root cause in the codebuild crate (services/service-com-amazonaws-codebuild-codebuild-20161006). The emulator returns the right HTTP status (400) and the right error code (InvalidInputException) when StartBuild carries a queuedTimeoutInMinutesOverride below the modeled minimum, but it emits a GENERIC Smithy range-constraint validation string instead of CodeBuild's own queued-timeout message, so a client that branches on or logs AWS's wording misreads the refusal.

Cause 1 of 1: StartBuild's out-of-range queuedTimeoutInMinutesOverride refusal carries the generic constraint-violation message.

AWS: Smithy StartBuild(projectName @required NonEmptyString @length min 1, ... queuedTimeoutInMinutesOverride -> TimeOut @range min 5 max 480, timeoutInMinutesOverride -> BuildTimeOut @range min 5 max 2160) -> {build}; modeled errors AccountLimitExceededException, InvalidInputException, ResourceNotFoundException (all errorKind client, httpError 400). The model states the admissible range (5..480 minutes, i.e. 5 minutes to 8 hours) but not the message text, so the message is recorded behavior only: HTTP 400, __type InvalidInputException, message exactly 'Invalid queued timeout. Should be between 5 minutes and 8 hours'. That string is WHOLLY FIXED TEXT for this error class — AWS interpolates nothing into it: not the rejected value, not the member name, not the project name or ARN, and there is no trailing period. The two limits it names are the model's own range bounds for TimeOut (min 5, max 480 rendered as '8 hours'), not values from any one request.

Emulator: HTTP 400 with the correct __type InvalidInputException, but message '1 validation error detected: Value '<rejected value>' at 'queuedTimeoutInMinutesOverride' failed to satisfy constraint: Member must have value greater than or equal to <min>'. This is the generic protocol-level constraint-violation template: its variable parts are the request's rejected value, the wire member name and the model's range bound, and it names no queued timeout at all. Hypothesis: the handler relies on the shared @range enforcement of the generated input validator rather than performing CodeBuild's own queued-timeout precondition check, so the generic validator's message escapes to the client before any service-specific wording is applied.

Why: a caller that distinguishes the queued-timeout refusal from CodeBuild's other InvalidInputException paths (unparseable buildspec, bad compute type, missing source location) can only do so by message, because all of them share one code and status. Against AWS it exact-matches or greps 'Invalid queued timeout'; against the emulator it sees an unrelated validator string and treats a well-understood, retriable misconfiguration as an unknown failure. The emulator also leaks the wire member name and the rejected value into a message AWS keeps constant, so message comparisons differ per request where AWS's are identical, and the human-readable upper bound ('8 hours') that tells an operator the legal range is never shown.

Fix: the StartBuild handler's queued-timeout validation — the operation's own precondition on queuedTimeoutInMinutesOverride must produce CodeBuild's InvalidInputException message, rather than letting the shared generated @range input validator answer the call. The sibling member timeoutInMinutesOverride (BuildTimeOut, @range 5..2160) is validated on the same path and should be checked for the same defect while this is fixed.

Related open issue that does NOT cover this check: 20261029T204615Z-codebuild-error-message-wording-diverges-from-aws-on.md describes the same wording family across six other CodeBuild refusal paths (DeleteWebhook, UpdateProject, ListBuildBatchesForProject, GetReportGroupTrend, ListBuilds, CreateWebhook) but carries no replay key for StartBuild or for the check below, so this cause must be replayed and closed separately.

Traces: codebuild.build-queued-timeout-override-reported-on-started-build.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout: 117/196 passed (59.7%); codebuild 105/145 (72%). This check is the only codebuild failure not already owned by an open issue or blocked by an unimplemented stub.

Failing check codebuild.build-queued-timeout-override-reported-on-started-build, verdict mismatch. The check creates an IAM service role and a minimal NO_SOURCE / NO_ARTIFACTS project with queuedTimeoutInMinutes=7, confirms BatchGetProjects echoes 7 (this passed), then calls StartBuild with queuedTimeoutInMinutesOverride=4 through a botocore client configured with parameter_validation=False so the request reaches the emulator on the wire.

The error-code assertion passed — the emulator did return InvalidInputException — so status and code are correct and the message is the sole divergence. The message assertion then failed:

  AssertionError: queuedTimeoutInMinutesOverride=4: expected message 'Invalid queued timeout. Should be between 5 minutes and 8 hours', got "1 validation error detected: Value '4' at 'queuedTimeoutInMinutesOverride' failed to satisfy constraint: Member must have value greater than or equal to 5"

  Expected (recorded AWS): 400 / InvalidInputException / 'Invalid queued timeout. Should be between 5 minutes and 8 hours'
  Actual   (emulator):     400 / InvalidInputException / "1 validation error detected: Value '4' at 'queuedTimeoutInMinutesOverride' failed to satisfy constraint: Member must have value greater than or equal to 5"

In the expected message every character is fixed text; '4' and the literal '5' in the actual message are the request's rejected value and the model's range bound respectively, i.e. per-request fillers of the emulator's generic template and not part of any requirement. Because StartBuild was refused here, the check never reached its later assertions (that an in-progress build started with queuedTimeoutInMinutesOverride=5 reports 5 and one started without an override reports the project's 7); those remain unverified and are not claimed by this issue.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — StartBuild input member queuedTimeoutInMinutesOverride targets TimeOut with range {min: 5, max: 480}; modeled errors AccountLimitExceededException / InvalidInputException / ResourceNotFoundException, each errorKind client with httpError 400.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. CreateRole an IAM role trusted by codebuild.amazonaws.com and capture its ARN.
3. CreateProject with source {type: NO_SOURCE, buildspec: a trivial version 0.2 buildspec}, artifacts {type: NO_ARTIFACTS}, environment {type: LINUX_CONTAINER, image: aws/codebuild/standard:7.0, computeType: BUILD_GENERAL1_SMALL}, that serviceRole, and queuedTimeoutInMinutes=7.
4. Confirm BatchGetProjects(names=[<project>]) echoes queuedTimeoutInMinutes=7 (this already works).
5. Call StartBuild(projectName=<project>, queuedTimeoutInMinutesOverride=4) with client-side parameter validation DISABLED (botocore Config(parameter_validation=False)) so the out-of-range value is sent on the wire.
6. Expect HTTP 400, __type InvalidInputException, message exactly 'Invalid queued timeout. Should be between 5 minutes and 8 hours'.
7. Observe HTTP 400 and InvalidInputException with the generic message "1 validation error detected: Value '4' at 'queuedTimeoutInMinutesOverride' failed to satisfy constraint: Member must have value greater than or equal to 5".
8. Repeat with queuedTimeoutInMinutesOverride=481 (above the modeled max of 480) — AWS returns the same constant message; and with timeoutInMinutesOverride=4 to check whether the sibling BuildTimeOut member (@range 5..2160) shares the defect.
9. Re-replay codebuild.build-queued-timeout-override-reported-on-started-build to confirm the fix; note the check continues past the refusal to assert that an in-progress build reports its overridden and inherited queuedTimeoutInMinutes.
