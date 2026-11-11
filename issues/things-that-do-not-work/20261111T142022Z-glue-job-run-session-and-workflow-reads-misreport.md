Issue-Source: trace-sweep-divergence
Issue-Trace: glue-job-run-arguments-propagated
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-job-run-arguments-propagated glue-session-command-args-round-trip glue-session-description-and-tags-round-trip glue-workflow-definition-and-batch-get-not-found glue-gjwr-traj-start-the-copied-job-fails-before-provisioning
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetJobRun|2xx->2xx|$.JobRun.ExecutionArguments
Issue-Key: glue|GetSession|2xx->2xx|$.Session.Timeout
Issue-Key: glue|BatchGetWorkflows|2xx->2xx|$.Workflows
Issue-Related: 20261102T132146Z-glue-job-and-job-run-reads-omit-modeled.md
Issue-Commit: 18f142fa95962db3f809978042802814a1944299

# Glue job-run, session and workflow reads misreport response members

> _Note: this issue was identified against the code base at commit `18f142fa95962db3f809978042802814a1944299`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobRun

## Description
This bundle carries 4 net-new root causes, all in Glue read responses for job runs, sessions and workflows. Each cause is numbered below with its own fixture and step; all four must be closed before this issue is resolved.

Cause 1 of 4: GetJobRun drops JobRun.ExecutionArguments, so the arguments a run was started with are not observable.
AWS: recorded behavior only — the on-disk Smithy model's com.amazonaws.glue#JobRun structure carries Arguments (GenericMap) but declares no ExecutionArguments member, so the model states nothing about it; the recording shows AWS's GetJobRun answering 200 with JobRun.ExecutionArguments present as a map echoing the per-run argument overrides supplied to StartJobRun (recorded as a single '--<name>' -> '<value>' entry). The key is a map of the run's effective execution arguments, keyed by argument name; its contents come entirely from the request that started the run.
Emulator: answers 200 but omits JobRun.ExecutionArguments entirely (replay: '$.JobRun.ExecutionArguments: missing').
Why: a client that starts a run with argument overrides and then reads the run back to confirm them sees no ExecutionArguments at all. Any caller that diffs requested against effective arguments, or that reconstructs a re-run from the recorded run, concludes the overrides were dropped and either re-submits or fails its own validation.
Fix: the GetJobRun response serializer plus the job-run state that records a run's effective arguments — the overrides passed to StartJobRun must be persisted on the run and emitted as JobRun.ExecutionArguments.
Traces: glue-job-run-arguments-propagated#task#5 (GetJobRun, expected 200 / actual 200).

Cause 2 of 4: GetSession drops Session.Timeout, so a session's configured timeout is not observable.
AWS: recorded behavior only — the on-disk model's com.amazonaws.glue#Session structure carries IdleTimeout but declares no Timeout member, so the model states nothing about it; the recording shows AWS's GetSession answering 200 with Session.Timeout present as an integer equal to the timeout (in minutes) the session was created with. The value is taken from the CreateSession request, not from a fixed constant.
Emulator: answers 200 but omits Session.Timeout (replay: '$.Session.Timeout: missing'). Every other asserted Session member matches, so this is a dropped member and not a wrong session.
Why: a client that creates a session with an explicit timeout and reads it back cannot confirm it, and a client that reconciles session configuration (or renders it) treats the session as having no timeout configured.
Fix: the GetSession response serializer plus the session state that records the timeout given at CreateSession time.
Traces: glue-session-command-args-round-trip#task#4 and glue-session-description-and-tags-round-trip#task#4 (GetSession, expected 200 / actual 200).

Cause 3 of 4: BatchGetWorkflows omits the Workflows member instead of returning it empty when every requested name is missing.
AWS: Smithy: BatchGetWorkflows output members Workflows (list Workflows, @length min 1 max 25) and MissingWorkflows (WorkflowNames); errors InternalServiceException, InvalidInputException, OperationTimeoutException. The recording shows AWS answering 200 with Workflows present as an empty list and the unresolved names reported under MissingWorkflows — i.e. the member is always serialized, and the empty case is an empty list, not an absent key.
Emulator: answers 200 with Workflows absent from the body (replay: '$.Workflows: missing (expected [], got None)').
Why: a client that iterates response.Workflows unconditionally — which is safe against AWS, since the key is always present — gets a null/None dereference or a missing-key error instead of an empty iteration, so a batch lookup that legitimately resolves nothing becomes a client-side crash rather than an empty result plus MissingWorkflows.
Fix: the BatchGetWorkflows response serializer — emit Workflows as an empty list rather than skipping the member when no requested name resolves.
Traces: glue-workflow-definition-and-batch-get-not-found#task#15 (BatchGetWorkflows, expected 200 / actual 200).

Cause 4 of 4: a job run whose execution role is not trusted by Glue fails for the wrong reason, and JobRun.ErrorMessage carries an internal Rust/SDK debug dump instead of AWS's failure text.
AWS: Smithy: GetJobRun output JobRun; JobRun.ErrorMessage targets ErrorString and is documented as 'An error message associated with this job run.' The model does not specify the text, so: recorded behavior only for the wording. The recording shows that when the role named for the run has not been given assume-role permissions for the Glue service, the run fails BEFORE provisioning and ErrorMessage is a single AWS-authored line of the form 'AccountId:<accountId> and JobName:<jobName> and JobRunId:<jobRunId> failed to execute with exception Role <roleArn> should be given assume role permissions for Glue Service.\n (Service: AWSGlueJobExecutor; Status Code: 400; Error Code: InvalidInputException; Request ID: <requestId>; Proxy: null)'. Fixed text: 'AccountId:', ' and JobName:', ' and JobRunId:', ' failed to execute with exception Role ', ' should be given assume role permissions for Glue Service.', and the trailing '(Service: AWSGlueJobExecutor; Status Code: 400; Error Code: InvalidInputException; Request ID: <id>; Proxy: null)' envelope. Variable parts taken from the request and state: the account id, the job name, the job run id, the role ARN and the request id — the recorded account/job/run/role/request values are one run's fill-in, not part of the requirement.
Emulator: answers 200, but reports a completely different failure: it attempts to fetch the job script from S3 and serializes the raw Rust Debug formatting of the S3 SDK error into ErrorMessage — 'Unable to download the job script from s3://<bucket>/<key>: the object store answered Some("NoSuchBucket"): service error: unhandled error (NoSuchBucket): Error { code: ..., message: ..., extras: ... } (ServiceError(ServiceError { source: Unhandled(Unhandled { ... }), raw: Response { status: StatusCode(404), headers: {...}, body: SdkBody { inner: Once(Some(b"<Error><Code>NoSuchBucket</Code>...")) } } }))', including struct literals, header maps and a raw XML body. The same step also shows JobRun.ExecutionArguments missing (see Cause 1).
Why: the run fails for a reason that is not the real one — a role-trust misconfiguration is reported as a missing script bucket — so a client diagnosing the failure, or an operator asserting on the Error Code / Service segment of AWS's envelope, is pointed at the wrong resource entirely. The leaked Rust Debug text is also unparseable by any consumer that reads AWS's single-line format, and it exposes emulator internals (SDK error structs, request ids, raw XML) through a modeled customer-facing field.
Fix: the StartJobRun / job-run execution path and the state it records on failure — validate that the execution role grants Glue assume-role permission before any script fetch, and have the job-run failure writer author AWS's single-line envelope rather than formatting an internal SDK error with Rust Debug.
Traces: glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#4 (GetJobRun, expected 200 / actual 200).

Related open issues (neither owns these replay keys, so these causes are net-new): '20261102T132146Z-glue-job-and-job-run-reads-omit-modeled.md' covers other omitted job/job-run members, '20261109T085123Z-athena-glue-s3-3-net-new-causes-glue.md' and '20261109T085123Z-emr-glue-s3-3-net-new-causes-no.md' cover other JobRun.ErrorMessage re-wordings and script-accessibility validation.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 83.5%, 66 divergence clusters) — these four clusters have empty owned_by and empty stub_path.

Cause 1 — glue-job-run-arguments-propagated#task#5, GetJobRun, verdict mismatch, expected_status 200, actual_status 200:
  $.JobRun.ExecutionArguments: missing (expected {'--my-custom-arg': 'hello-world'}, got None)

Cause 2 — glue-session-command-args-round-trip#task#4, GetSession, verdict mismatch, expected_status 200, actual_status 200 (affected_count 2; also glue-session-description-and-tags-round-trip#task#4):
  $.Session.Timeout: missing (expected 5, got None)

Cause 3 — glue-workflow-definition-and-batch-get-not-found#task#15, BatchGetWorkflows, verdict mismatch, expected_status 200, actual_status 200:
  $.Workflows: missing (expected [], got None)

Cause 4 — glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#4, GetJobRun, verdict mismatch, expected_status 200, actual_status 200, divergence_key keys $.JobRun.ErrorMessage,$.JobRun.ExecutionArguments:
  $.JobRun.ErrorMessage: value_mismatch
    expected: 'AccountId:000000000000 and JobName:<job> and JobRunId:<runId> failed to execute with exception Role <roleArn> should be given assume role permissions for Glue Service.\n (Service: AWSGlueJobExecutor; Status Code: 400; Error Code: InvalidInputException; Request ID: <requestId>; Proxy: null)'
    actual:   'Unable to download the job script from s3://<bucket>/scripts/<script>.py: the object store answered Some("NoSuchBucket"): service error: unhandled error (NoSuchBucket): Error { code: Some("NoSuchBucket"), message: Some("The specified bucket does not exist"), extras: Some({...}) } (ServiceError(ServiceError { source: Unhandled(Unhandled { ... }), raw: Response { status: StatusCode(404), headers: {...}, body: SdkBody { inner: Once(Some(b"<Error><Code>NoSuchBucket</Code>...")) } } }))'

Model evidence (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): JobRun has ErrorMessage (ErrorString) and Arguments (GenericMap) but no ExecutionArguments member; Session has IdleTimeout but no Timeout member — so ExecutionArguments and Session.Timeout are recorded behavior only. BatchGetWorkflows output declares Workflows and MissingWorkflows.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture glue-job-run-arguments-propagated and inspect step task#5 — StartJobRun supplies a per-run argument override, then GetJobRun is read back; assert JobRun.ExecutionArguments is present and equals the override map.
3. Cause 2: replay fixture glue-session-command-args-round-trip step task#4 (and glue-session-description-and-tags-round-trip step task#4) — CreateSession with an explicit timeout, then GetSession; assert Session.Timeout is present and equals the created value.
4. Cause 3: replay fixture glue-workflow-definition-and-batch-get-not-found step task#15 — call BatchGetWorkflows with names that do not exist; assert the 200 body contains Workflows as an empty list alongside MissingWorkflows.
5. Cause 4: replay fixture glue-gjwr-traj-start-the-copied-job-fails-before-provisioning step task#4 — start a job whose execution role is not trusted by Glue, then GetJobRun; assert JobRun.ErrorMessage is AWS's single-line 'AccountId:... and JobName:... and JobRunId:... failed to execute with exception Role ... should be given assume role permissions for Glue Service.\n (Service: AWSGlueJobExecutor; Status Code: 400; Error Code: InvalidInputException; Request ID: ...; Proxy: null)' envelope, and that no Rust Debug/SDK struct text appears in the field.
