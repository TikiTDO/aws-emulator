Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iottwinmaker.create-sync-job-unassumable-role-rejected
Issue-Shape-Id: com.amazonaws.iottwinmaker#AWSIoTTwinMaker
Issue-Key: iottwinmaker.create-sync-job-unassumable-role-rejected|run#12|AssertionError|assert refusal["Message"] == expected, f"expected message {expected!r}, got {refusal!r}"
Issue-Commit: 2bd233cca01cafdaf79cdc4364a8c85763a42b7f

# CreateSyncJob's unassumable-syncRole ValidationException carries an emulator-invented message instead of AWS's fixed "Could not assume the role provided, verify permissions"

> _Note: this issue was identified against the code base at commit `2bd233cca01cafdaf79cdc4364a8c85763a42b7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSyncJob

## Description
Cause 1 of 1: the message of the ValidationException that refuses a CreateSyncJob whose syncRole cannot be assumed by iottwinmaker.amazonaws.com is the emulator's own wording, built by interpolating the member name and the caller's role ARN, rather than AWS's fixed sentence.

AWS: Smithy (services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json) models CreateSyncJob with syncRole (RoleArn, @required, @length 20..2048, @pattern ^arn:((aws)|(aws-cn)|(aws-us-gov)):iam::[0-9]{12}:role/) and error ValidationException (HTTP 400); the model does not state the message text, so this is recorded behavior only. The recording shows AWS refusing the call with HTTP 400, Code 'ValidationException' and Message exactly 'Could not assume the role provided, verify permissions'. That message is ENTIRELY fixed text: it has no variable parts — it names neither the member, nor the role ARN, nor the service principal, nor the action. The same recording also shows the refusal has no effect: GetSyncJob for that workspace/syncSource then fails ResourceNotFoundException with message 'Provided Sync Job does not exist!' (the emulator already matches this part), and a syncRole whose trust policy does grant sts:AssumeRole to iottwinmaker.amazonaws.com is accepted.

Emulator: the status (400) and the code ('ValidationException') are right, but the message is a different sentence of the emulator's own composition. Replay output: Message = 'syncRole arn:aws:iam::000000000000:role/sim2real-tm-untrusted-role-<suffix> cannot be assumed by iottwinmaker.amazonaws.com: its trust policy must grant sts:AssumeRole to that service principal'. Its fixed parts are ' cannot be assumed by ', ': its trust policy must grant ' and ' to that service principal'; its variable parts are the member name ('syncRole'), the role ARN copied from the request, the service principal and the action constant. (The account id and role-name suffix in the quoted string are per-run values from the check's own CreateRole, not part of any template.) No part of this wording is AWS's.

Why: a client that keys off the refusal text — AWS's documented sentence 'Could not assume the role provided, verify permissions' is the string surfaced to operators and matched by provisioning/retry logic and by test assertions for this exact trust-policy failure — sees an unrecognised message and treats a correctly-diagnosed trust-policy problem as an unknown validation failure, so the role-trust remediation path is never taken. The emulator also leaks the caller's full role ARN (up to the 2048-character RoleArn length) into a message AWS keeps constant, which is the same unclamped-interpolation shape already filed for other members of this service (see open issues 20261031T151130Z-iottwinmaker-listmetadatatransferjobs-error-response.md and 20261101T203427Z-iottwinmaker-createmetadatatransferjob-error-response.md — neither covers this refusal).

Fix: the refusal message belongs to the shared trust-policy rule the CreateSyncJob handler's validation calls — crate::state::assume_role_refusal, which formats the sentence, and its caller in the CreateSyncJob handler, which lifts it into ValidationException.message. The rule that decides WHETHER to refuse (trust policy withholds sts:AssumeRole from iottwinmaker.amazonaws.com) is correct and should be left alone; only the message the refusal carries is wrong, and the correct message is a constant with no interpolated request or state values. Note the same helper serves the workspace `role` member (WORKSPACE_ROLE_MEMBER), so whichever component is changed must keep the sync-job refusal's wording the fixed AWS sentence.

Traces: iottwinmaker.create-sync-job-unassumable-role-rejected (mismatch, assertion on refusal['Message']).

## Evidence
Expected (AWS, from the recording): HTTP 400, Error = {'Code': 'ValidationException', 'Message': 'Could not assume the role provided, verify permissions'} — message is wholly fixed text with no request- or state-derived parts.

Actual (emulator): HTTP 400, Error = {'Code': 'ValidationException', 'Message': 'syncRole arn:aws:iam::000000000000:role/sim2real-tm-untrusted-role-72aa71473e89 cannot be assumed by iottwinmaker.amazonaws.com: its trust policy must grant sts:AssumeRole to that service principal'}.

Assertion from the suite: check_iottwinmaker_create_sync_job_unassumable_role_rejected.py line 48 — assert refusal["Message"] == expected — AssertionError: expected message 'Could not assume the role provided, verify permissions'.

The divergence is stable across runs: two replays produced the same wording with only the per-run role-name suffix differing (…-e1982aa1779c, …-72aa71473e89), confirming the template rather than a per-run value is wrong. The code and status match; only the message diverges. Source of the wording: services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs lines 762-775 (assume_role_refusal), reached from services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_sync_job.rs lines 225-235.

## Steps to Reproduce
1. CreateRole in IAM with a trust policy whose only statement allows sts:AssumeRole to a principal OTHER than iottwinmaker.amazonaws.com (e.g. Service: ec2.amazonaws.com); note the returned role ARN.
2. Create an iottwinmaker workspace (any valid s3Location and an execution role iottwinmaker.amazonaws.com can assume).
3. Call CreateSyncJob(workspaceId=<that workspace>, syncSource='SITEWISE', syncRole=<the untrusted role ARN>).
4. Observe the refusal: AWS returns HTTP 400 ValidationException with Message exactly 'Could not assume the role provided, verify permissions'; the emulator returns HTTP 400 ValidationException with the '<member> <roleArn> cannot be assumed by iottwinmaker.amazonaws.com: its trust policy must grant sts:AssumeRole to that service principal' sentence instead.
5. (Unchanged, for context) GetSyncJob(workspaceId=<that workspace>, syncSource='SITEWISE') then fails ResourceNotFoundException 'Provided Sync Job does not exist!', and repeating step 3 with a role trusting iottwinmaker.amazonaws.com is accepted.
