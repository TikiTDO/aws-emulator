Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.emr-launch-time-auto-termination-policy-echoed-and-out-of-range-refused emr.get-session-endpoint-returns-endpoint-for-started-session emr.terminate-session-twice-rejected emr.start-session-on-non-running-cluster-rejected emr.session-idle-timeout-marks-session-stopped emr.session-idle-ttl-expires-without-explicit-terminate emr.e2e-notebook-execution-run-to-completion emr.list-notebook-executions-ordering-and-status-filter emr.e2e-notebook-execution-persistent-app-ui-flow
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.emr-launch-time-auto-termination-policy-echoed-and-out-of-range-refused|_refusal#8|AssertionError|raise AssertionError(
Issue-Key: emr.get-session-endpoint-returns-endpoint-for-started-session|run#11|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: emr.terminate-session-twice-rejected|run#13|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: emr.start-session-on-non-running-cluster-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: emr.session-idle-timeout-marks-session-stopped|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: emr.session-idle-ttl-expires-without-explicit-terminate|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: emr.e2e-notebook-execution-run-to-completion|run#22|KeyError|output_key = final["OutputNotebookURI"].split(f"s3://{bucket}/", 1)[-1]
Issue-Key: emr.list-notebook-executions-ordering-and-status-filter|_poll_until#8|Timeout|return last
Issue-Key: emr.e2e-notebook-execution-persistent-app-ui-flow|_poll_until#20|Timeout|return last
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# EMR: 8 net-new root causes — RunJobFlow skips the AutoTerminationPolicy IdleTimeout bounds, GetSessionEndpoint/TerminateSession answer NotImplemented, the StartSession state refusal is emulator prose, the session idle-timeout sweep never fires, and notebook executions expose no output notebook

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow/GetSessionEndpoint/TerminateSession/StartSession/GetSession/DescribeNotebookExecution/ListNotebookExecutions

## Description
Nine EMR behavioral checks fail for eight distinct root causes in the EMR emulator. None of the nine is marked ALREADY FILED or OWNED by an open issue in this suite report. Related open issues are named per cause; none of them carries these checks' replay keys.

Cause 1 of 8: RunJobFlow performs no range validation on AutoTerminationPolicy.IdleTimeout, so a cluster is LAUNCHED where AWS refuses the request.
AWS: Smithy AutoTerminationPolicy.IdleTimeout documents the fixed bounds — "Specifies the amount of idle time in seconds after which the cluster automatically terminates. You can specify a minimum of 60 seconds and a maximum of 604800 seconds (seven days)". RunJobFlow with IdleTimeout below 60 or above 604800 is refused (recorded behavior: HTTP 400, error code ValidationException) with the message "Auto-termination policy IdleTimeout must be within a minimum of 60 seconds, and a maximum of 604800 seconds." — that whole sentence is fixed text, with no value interpolated from the request, and the two limits in it are the API constants 60 and 604800, not values of one run. A launch at exactly the minimum (60) is accepted and GetAutoTerminationPolicy then reports exactly {IdleTimeout: 60}.
Emulator: RunJobFlow with AutoTerminationPolicy={IdleTimeout: 59} is ACCEPTED and returns a JobFlowId (the replay shows a cluster launched where a refusal was required); the out-of-range high value (604801) is therefore never graded either.
Why: a client that mistypes or computes an out-of-bounds idle timeout gets a running, billable cluster whose auto-termination policy AWS would never have stored, instead of a 400 ValidationException naming the bounds; error-handling code around cluster launch is never exercised and the client cannot distinguish an accepted from a rejected policy.
Fix: the RunJobFlow handler's input validation for the AutoTerminationPolicy member (the same bounds check that the PutAutoTerminationPolicy path needs; the open issue 20261108T042656Z-emr-auto-termination-policy-writes-skip-the-idletimeout.md reports the missing bounds check on the PutAutoTerminationPolicy write surface only, and does not cover the launch-time path).
Traces: emr.emr-launch-time-auto-termination-policy-echoed-and-out-of-range-refused (AssertionError: "RunJobFlow with IdleTimeout=59 launched ... where a refusal was required").

Cause 2 of 8: GetSessionEndpoint is not served — it answers error code NotImplemented, so neither its refusal contract nor its success contract exists.
AWS: Smithy GetSessionEndpoint takes required ClusterId and SessionId and models exactly two errors, InvalidRequestException (client, HTTP 400) and InternalServerException (500). For a session that has not yet reached a ready state (SessionState SUBMITTED or STARTING) the call is refused InvalidRequestException with the message "GetSessionEndpoint is not supported because Session <sessionId> is in <state> state." — fixed text with two variable parts taken from the request and the emulator's own state (the session id, and the SessionState the session is in at the call). The refusal has no side effect on the session. Once the session is ready (STARTED/IDLE/BUSY) the call returns the required Endpoint member (an HTTPS URL) plus AuthToken/AuthTokenExpirationTime/Credentials.
Emulator: the call is answered with error code NotImplemented (replay: "expected InvalidRequestException for a not-yet-ready session, got 'NotImplemented'"), which is not a modeled error of the operation, so the ready-session success path is unreachable as well.
Why: a client cannot distinguish "session not ready yet, retry" from a permanent failure, and can never obtain the Spark Connect endpoint/auth token that is the only documented way to use a session's data plane.
Fix: the GetSessionEndpoint handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/get_session_endpoint.rs) — its not-ready state gate and refusal message, and the endpoint/auth-token projection from the session state. The scaffold-stub issue auto-filed for GetSessionEndpoint covers only "implement the operation"; this cause records the error class and exact message that the implementation owes.
Traces: emr.get-session-endpoint-returns-endpoint-for-started-session.

Cause 3 of 8: TerminateSession is not served — it answers error code NotImplemented instead of the modeled refusal, and a live session can never be ended.
AWS: Smithy TerminateSession takes required ClusterId and SessionId, models InvalidRequestException (400) and InternalServerException (500), and returns required ClusterId/SessionId/State on success. A SessionId that never existed on the cluster is refused (recorded behavior: error code ValidationException) with the message "sessionId '<clusterId>/session/<sessionId>' is not valid." — fixed text around one variable part composed from the request's ClusterId and SessionId. The rejected call leaves the live session untouched; terminating a real live session settles it to TERMINATED, observable through GetSession.
Emulator: TerminateSession answers NotImplemented (replay: "expected ValidationException, got NotImplemented"), so neither the unknown-session refusal nor the terminate-then-TERMINATED transition is observable.
Why: a client cannot tear down a session it created — the session stays live forever — and an unknown session id is reported as a server-side "not implemented" rather than a client validation error.
Fix: the TerminateSession handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/terminate_session.rs): the sessionId validity refusal and the state transition to TERMINATED in the EMR session state. The scaffold-stub issue auto-filed for TerminateSession covers only "implement the operation"; this cause records the error class, message template and post-state it owes.
Traces: emr.terminate-session-twice-rejected.

Cause 4 of 8: StartSession's cluster-state refusal carries an emulator-authored sentence instead of AWS's.
AWS: StartSession on a cluster that is not RUNNING or WAITING is refused with the message "StartSession is not supported for cluster '<clusterId>' in state <state>. The cluster must be in RUNNING or WAITING state." (recorded behavior). The fixed text is the whole sentence pair; the variable parts are the cluster id from the request and the ClusterState the cluster is in at the call (here STARTING).
Emulator: the refusal message is re-worded as "Cluster '<clusterId>' is in state <state>; a session can only be started on a cluster in the RUNNING or WAITING state."
Why: a client (or a CLI/SDK user) matching on AWS's sentence — the only machine-readable detail beyond the error code for this refusal class — does not recognise the emulator's prose, so retry/diagnostic logic keyed on it never triggers.
Fix: the StartSession handler's cluster-state gate and its refusal message template (not the operation's existence; the open stub issue 20261104T102001Z-emr-startsession-stub.md is about StartSession being unimplemented, and the message is served today).
Traces: emr.start-session-on-non-running-cluster-rejected.

Cause 5 of 8: a session's SessionIdleTimeoutInMinutes sweep never fires, so an idle session is never auto-terminated — two checks are cut off at the 600 s harness cap while polling GetSession.
AWS: StartSession.SessionIdleTimeoutInMinutes is documented as "The idle timeout, in minutes. If the session is idle for this duration, Amazon EMR automatically terminates it" (mirrored on Session.SessionIdleTimeoutInMinutes), and Session.IdleSince is "the date and time that the session last entered the IDLE state". So a session started with the minimum timeout first reaches a ready state (SessionState STARTED/IDLE/BUSY) and then, left untouched with no TerminateSession call, reaches TERMINATED on its own, observable through GetSession.
Emulator: neither polled condition is ever satisfied — both checks are killed at 600 s with the session never reported active and/or never reported TERMINATED. Hypothesis: the session record has no automatic state advancement at all, so neither the start-up transition nor the idle sweep exists (the open issue 20261110T142808Z-emr-3-net-new-root-causes-the-spark.md reports the same missing session progression for four other checks at the 60 s cap; it does not carry these two).
Why: a client that relies on EMR reclaiming idle sessions keeps paying for a session that is never reclaimed, and a poll for TERMINATED after the configured timeout never returns.
Fix: the session state machine behind StartSession/GetSession (session records and their timed state advancement in the EMR service state), specifically the idle-timeout sweep that moves an idle session to TERMINATED and sets EndedAt/IdleSince.
Traces: emr.session-idle-timeout-marks-session-stopped, emr.session-idle-ttl-expires-without-explicit-terminate (both "Check timed out after 600.0s").

Cause 6 of 8: a FINISHED notebook execution exposes no output notebook — DescribeNotebookExecution omits OutputNotebookURI and no output object exists in S3.
AWS: Smithy NotebookExecution.OutputNotebookURI is "The location of the notebook execution's output file in Amazon S3"; for an execution started with OutputNotebookS3Location={Bucket,Key} and run to NotebookExecutionStatus FINISHED, DescribeNotebookExecution reports that s3:// URI and the object it names exists with a non-zero length (the executed notebook, default ipynb format).
Emulator: the execution is reported FINISHED and is returned by ListNotebookExecutions(Status="FINISHED"), but the NotebookExecution structure has no OutputNotebookURI member at all (replay: KeyError: 'OutputNotebookURI'), so nothing identifies or contains the output artifact.
Why: a client that runs a notebook to completion and then fetches its executed output — the entire point of a notebook execution — has no URI to fetch and no object to read; the success response is unusable.
Fix: the StartNotebookExecution/DescribeNotebookExecution handlers and the notebook-execution state (populate OutputNotebookURI and OutputNotebookS3Location from the submitted OutputNotebookS3Location) plus the execution's S3 output side effect.
Traces: emr.e2e-notebook-execution-run-to-completion.

Cause 7 of 8: the ListNotebookExecutions Status partition / ordering scenario reaches no verdict — it is cut off at the 60 s harness cap while polling DescribeNotebookExecution.
AWS: an execution walks START_PENDING -> STARTING -> RUNNING -> FINISHING -> FINISHED for a healthy notebook and -> FAILING -> FAILED for one whose cell raises (Smithy NotebookExecutionStatus), and ListNotebookExecutions(Status=...) returns exactly the executions in that status, with two consecutive unfiltered listings in the same order.
Emulator: the check never reaches its Status-filter assertions; it is killed at the 60 s cap still polling for the two executions to settle. Hypothesis: the failing notebook never reaches FAILED (the healthy one does finish — see cause 6), so the FINISHED/FAILED partition never forms. The open issue 20261110T142808Z-emr-3-net-new-root-causes-the-spark.md describes this check under a different emulator state (nothing finishing at all) and no longer carries it; 20261107T182201Z-emr-notebook-executions-never-run-a-healthy-notebook.md describes the opposite symptom (a healthy notebook ending FAILED).
Why: a client cannot triage executions by status — the status filter can never return both classes — and a wait for a failing execution's terminal status never ends.
Fix: the notebook-execution status progression behind StartNotebookExecution/DescribeNotebookExecution (a notebook whose cell raises must settle FAILED with a LastStateChangeReason) and the Status filtering/ordering in the ListNotebookExecutions handler.
Traces: emr.list-notebook-executions-ordering-and-status-filter ("Check timed out after 60.0s").

Cause 8 of 8: the notebook-execution -> persistent-app-UI end-to-end scenario reaches no verdict — cut off at the 60 s harness cap.
AWS: after a notebook execution finishes on a cluster, CreatePersistentAppUI(TargetResourceArn=<cluster ARN>) returns a PersistentAppUIId whose GetPersistentAppUIPresignedURL reports PresignedURLReady=false immediately after creation, and DescribePersistentAppUI then reaches PersistentAppUIStatus ATTACHED, after which the presigned URL becomes ready and non-empty.
Emulator: the check is killed at the 60 s cap before any of those assertions, so at least one polled transition (the execution settling, or the persistent app UI reaching ATTACHED) never happens. Hypothesis: the persistent app UI never leaves its initial status — the open issue 20261108T151103Z-emr-4-net-new-root-causes-every-job.md reports persistent-app-UI progression failures for other checks and does not carry this one.
Why: a client that creates a persistent app UI to inspect a finished job's Spark History Server waits forever for ATTACHED and never gets a usable presigned URL.
Fix: the CreatePersistentAppUI/DescribePersistentAppUI/GetPersistentAppUIPresignedURL handlers and the persistent-app-UI status advancement in the EMR state, together with the notebook-execution progression of cause 6/7.
Traces: emr.e2e-notebook-execution-persistent-app-ui-flow ("Check timed out after 60.0s").

## Evidence
Suite report simparity-behavioral-aws-v1, service heading '### emr' (emr: 113/219 passed). The nine failing checks clustered here, with the detail the report gives:

[mismatch] emr.emr-launch-time-auto-termination-policy-echoed-and-out-of-range-refused
  AssertionError: RunJobFlow with IdleTimeout=59 launched 'j-0000000000002' where a refusal was required
  Expected (AWS): HTTP 400, code ValidationException, message "Auto-termination policy IdleTimeout must be within a minimum of 60 seconds, and a maximum of 604800 seconds." (whole sentence fixed; 60/604800 are the API constants from the Smithy doc of AutoTerminationPolicy.IdleTimeout). Actual: 200 with a JobFlowId (the generated cluster id in the message is this run's value, not part of the requirement).

[mismatch] emr.get-session-endpoint-returns-endpoint-for-started-session
  AssertionError: expected InvalidRequestException for a not-yet-ready session, got 'NotImplemented'
  Expected (AWS): 400 InvalidRequestException, message "GetSessionEndpoint is not supported because Session <sessionId> is in <state> state." then, once the session is ready, Endpoint (HTTPS URL) + non-empty AuthToken. Actual: error code NotImplemented, which GetSessionEndpoint does not model (modeled errors: InvalidRequestException, InternalServerException).

[mismatch] emr.terminate-session-twice-rejected
  AssertionError: expected ValidationException, got NotImplemented
  Expected (AWS): ValidationException, message "sessionId '<clusterId>/session/<sessionId>' is not valid." for an unknown session; the live session then terminates to TERMINATED. Actual: NotImplemented for every TerminateSession call.

[mismatch] emr.start-session-on-non-running-cluster-rejected
  AssertionError: expected message "StartSession is not supported for cluster 'j-...' in state STARTING. The cluster must be in RUNNING or WAITING state.", got "Cluster 'j-...' is in state STARTING; a session can only be started on a cluster in the RUNNING or WAITING state."
  (The cluster id in both strings is this run's value; the requirement is the sentence around it and the state name read from the emulator's own cluster record.)

[mismatch] emr.session-idle-timeout-marks-session-stopped — "Error: Check timed out after 600.0s"
[mismatch] emr.session-idle-ttl-expires-without-explicit-terminate — "Error: Check timed out after 600.0s"
  Both poll GetSession after StartSession(SessionIdleTimeoutInMinutes=1): expected a ready SessionState (STARTED/IDLE/BUSY) and then TERMINATED with no TerminateSession call. Neither condition is ever observed in 600 s; the emulator serves these reads from memory, so the budget is ample (113 checks of this suite complete).

[error] emr.e2e-notebook-execution-run-to-completion
  KeyError: 'OutputNotebookURI' at final["OutputNotebookURI"], after the execution was reported FINISHED and listed by ListNotebookExecutions(Status="FINISHED").
  Expected (AWS): NotebookExecution.OutputNotebookURI = the s3:// location of the executed notebook, and HeadObject on it returns ContentLength > 0. Actual: the member is absent from the response.

[mismatch] emr.list-notebook-executions-ordering-and-status-filter — "Error: Check timed out after 60.0s" (killed while polling DescribeNotebookExecution for FINISHED/FAILED on two executions).
[mismatch] emr.e2e-notebook-execution-persistent-app-ui-flow — "Error: Check timed out after 60.0s" (killed before the PresignedURLReady=false / ATTACHED / PresignedURLReady=true assertions).

None of these nine is marked ALREADY FILED or OWNED in the report.

## Steps to Reproduce
Cause 1 (RunJobFlow IdleTimeout bounds):
1. Create an EMR service role and EC2 instance profile; pick a subnet.
2. RunJobFlow(Name=..., ReleaseLabel=emr-6.15.0, Instances={one MASTER m5.xlarge group, KeepJobFlowAliveWhenNoSteps=true, Ec2SubnetId=<subnet>}, AutoTerminationPolicy={IdleTimeout: 60}) -> accepted; GetAutoTerminationPolicy must report exactly {IdleTimeout: 60}.
3. Repeat with AutoTerminationPolicy={IdleTimeout: 59} and with {IdleTimeout: 604801}: both must be refused ValidationException with "Auto-termination policy IdleTimeout must be within a minimum of 60 seconds, and a maximum of 604800 seconds." The emulator accepts 59 and returns a JobFlowId.

Causes 2-5 (sessions):
1. RunJobFlow with ReleaseLabel=emr-spark-8.0.0, Applications=[{Name: Spark}], SessionEnabled=true into a subnet; wait for DescribeCluster WAITING.
2. While the cluster is still STARTING, call StartSession: the refusal message must be "StartSession is not supported for cluster '<id>' in state STARTING. The cluster must be in RUNNING or WAITING state." (cause 4).
3. On the WAITING cluster, StartSession, then immediately GetSessionEndpoint(ClusterId, SessionId) while GetSession still reports SUBMITTED/STARTING: expected 400 InvalidRequestException "GetSessionEndpoint is not supported because Session <id> is in <state> state."; the emulator answers NotImplemented (cause 2).
4. Poll GetSession to a ready state, then GetSessionEndpoint again: expected Endpoint starting with https:// and a non-empty AuthToken (cause 2).
5. TerminateSession with a same-shaped but never-issued SessionId: expected ValidationException "sessionId '<clusterId>/session/<sessionId>' is not valid."; then TerminateSession on the real session and poll GetSession to TERMINATED. The emulator answers NotImplemented to both (cause 3).
6. StartSession(..., SessionIdleTimeoutInMinutes=1), poll GetSession to a ready state, then stop touching it and poll GetSession: it must reach TERMINATED without any TerminateSession call (cause 5).

Causes 6-8 (notebook executions / persistent app UI):
1. Launch a cluster with Hadoop, Spark, Livy and JupyterEnterpriseGateway; wait for WAITING. Create a bucket and put a one-cell notebook at notebooks/input.ipynb.
2. StartNotebookExecution(ExecutionEngine={Id:<cluster>,Type:EMR}, ServiceRole=<notebook role>, NotebookS3Location={Bucket,Key}, OutputNotebookS3Location={Bucket, Key: notebooks/output}); poll DescribeNotebookExecution to FINISHED.
3. Read NotebookExecution.OutputNotebookURI — it is absent — and HeadObject the key it should name (cause 6).
4. Start a second execution of a notebook whose cell raises and poll DescribeNotebookExecution: it must settle FAILED, and ListNotebookExecutions(Status="FINISHED"/"FAILED") must partition the two (cause 7).
5. CreatePersistentAppUI(TargetResourceArn=<cluster ARN>); GetPersistentAppUIPresignedURL(PersistentAppUIType="SHS") must report PresignedURLReady=false, then DescribePersistentAppUI must reach ATTACHED and the presigned URL become ready and non-empty (cause 8).
