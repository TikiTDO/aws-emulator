Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.submit-task-state-change-invalid-status-rejected ecs.task-state-change-ordering-enforced-rejected ecs.task-state-change-server-enforced ecs.task-state-changes-are-monotonic-terminal
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.submit-task-state-change-invalid-status-rejected|run#13|AssertionError|assert control_error == "AccessDeniedException", (
Issue-Key: ecs.task-state-change-ordering-enforced-rejected|_error_code#12|AssertionError|raise AssertionError("expected SubmitTaskStateChange to raise for a non-agent caller, it succeeded")
Issue-Key: ecs.task-state-change-server-enforced|_error#17|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: ecs.task-state-changes-are-monotonic-terminal|run#30|AssertionError|assert code == "AccessDeniedException", (
Issue-Commit: 7be3c4c4f98e3bb4fe12b293a9b02ae8c9c46178

# ecs: SubmitTaskStateChange is reachable from a non-agent caller — a Fargate task state change answers 200 {"acknowledgment":"ACK"} instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `7be3c4c4f98e3bb4fe12b293a9b02ae8c9c46178`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitTaskStateChange

## Description
Cause 1 of 1: the agent-only caller gate in front of SubmitTaskStateChange is not enforced for Fargate tasks, so an ordinary signed client can report task state and the call is acknowledged.

AWS: Smithy (com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113) SubmitTaskStateChange takes {cluster, task, status, reason, containers, attachments, managedAgents, pullStartedAt, pullStoppedAt, executionStoppedAt}, returns {acknowledgment}, and models AccessDeniedException (httpError 403), ClientException, ClusterNotFoundException, InvalidParameterException and ServerException. Recorded behavior: the operation belongs to the container-agent plane. For a FARGATE task there is no container instance whose identity the service can verify, so AWS refuses the call to any caller but the verified agent with HTTP 403 AccessDeniedException and the message `Fargate task state changes from the agent must have the instance verified` — that message is entirely fixed text for this error class; it carries no value from the request or from state. The refusal is unconditional on the status value: the recordings show a *well-formed* enum status (e.g. the task's own current lastStatus, or RUNNING against an already-STOPPED task) refused with AccessDeniedException, and the task's lastStatus left untouched afterwards.

Emulator: every one of those calls succeeds. The replay shows `{'acknowledgment': 'ACK', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}` for `submit_task_state_change(cluster=<c>, task=<fargate task arn>, status='RUNNING')`, and the harness reports the diverging codes as `expected the well-formed control status 'PROVISIONING' to be refused with AccessDeniedException (the Fargate-agent-only gate), got None`, `expected SubmitTaskStateChange to raise for a non-agent caller, it succeeded`, `expected the call to be rejected, but it succeeded: {'acknowledgment': 'ACK', ...'HTTPStatusCode': 200...}` and `expected SubmitTaskStateChange against a Fargate task from a non-agent caller to be rejected with AccessDeniedException; got None instead`. So the status/code/message triple diverges: 200 + no error code + no message where AWS answers 403 + `AccessDeniedException` + `Fargate task state changes from the agent must have the instance verified`.

Why: SubmitTaskStateChange is the write path for Task.lastStatus. With the gate open, any client can drive a task's state — including reversing a terminal STOPPED back to RUNNING — so lastStatus/desiredStatus stop being monotonic and stop being authoritative. Clients and tests that rely on AWS denying this privileged call (and on the task staying in its terminal state) instead observe a successful ACK, and nothing distinguishes 'the agent reported this' from 'an unrelated caller rewrote it'. It also makes two downstream rules unobservable, because the checks cannot get past the control assertion: the per-status validation that refuses a non-enum status with InvalidParameterException and the message `The status <status> is not a valid status.` (fixed text `The status ` + the request's status value + ` is not a valid status.`), and the terminal-state ordering assertion. Those are not claimed to diverge here — they were never reached — but they sit behind the same handler.

Fix: the caller/instance-verification gate in the SubmitTaskStateChange handler of the ECS crate (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113, submit_task_state_change op), together with the transition validation behind it. It must refuse a non-agent caller for a task with no verifiable container-instance identity (i.e. a FARGATE task) with AccessDeniedException/403 and AWS's fixed wording, and must leave the task's lastStatus unchanged.

Related filings, named so a fixer does not ping-pong: the FIXED issue 20261104T062603Z-ecs-resource-scoping-and-agent-only-apis-are.md carried exactly these four check ids under its Cause 7 and described the identical symptom (`the calls succeed, returning {'acknowledgment': 'ACK'} with HTTP 200`); that fix did not land the gate, and all four checks still fail the same way. Separately, the OPEN issue 20261109T144605Z-ecs-10-net-new-causes-container-health-checks.md Cause 2 asks for the opposite direction for the *other* two ops (SubmitAttachmentStateChanges / SubmitContainerStateChange, which the emulator refuses with this same message) and labels the wording emulator-authored; these recordings show the wording IS AWS's for the SubmitTaskStateChange/Fargate class, so the gate must be made conditional per operation and per task identity rather than removed outright.

Traces: ecs.submit-task-state-change-invalid-status-rejected, ecs.task-state-change-ordering-enforced-rejected, ecs.task-state-change-server-enforced, ecs.task-state-changes-are-monotonic-terminal.

## Evidence
Expected (AWS): HTTP 403, Error.Code `AccessDeniedException`, Error.Message `Fargate task state changes from the agent must have the instance verified` (fixed text, no request/state values), and DescribeTasks afterwards still reporting the task's prior lastStatus (e.g. STOPPED stays STOPPED).

Actual (emulator), from the replay:
- ecs.task-state-change-server-enforced: `AssertionError: expected the call to be rejected, but it succeeded: {'acknowledgment': 'ACK', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '24', ...}}}` for `submit_task_state_change(cluster=<cluster>, task=<fargate task arn>, status='RUNNING')`.
- ecs.task-state-change-ordering-enforced-rejected: `AssertionError: expected SubmitTaskStateChange to raise for a non-agent caller, it succeeded` (status='RUNNING' submitted after the task had reached STOPPED).
- ecs.task-state-changes-are-monotonic-terminal: `AssertionError: expected SubmitTaskStateChange against a Fargate task from a non-agent caller to be rejected with AccessDeniedException; got None instead -- SubmitTaskStateChange may now be reachable from a non-agent caller`.
- ecs.submit-task-state-change-invalid-status-rejected: `AssertionError: expected the well-formed control status 'PROVISIONING' to be refused with AccessDeniedException (the Fargate-agent-only gate), got None`.

Smithy citation: ecs SubmitTaskStateChange errors = AccessDeniedException(403), ClientException, ClusterNotFoundException, InvalidParameterException, ServerException; output member `acknowledgment`. Cluster/task ARNs, ids and timestamps in the quoted payloads are per-run values, not requirements.

## Steps to Reproduce
1. CreateCluster, then RegisterTaskDefinition with requiresCompatibilities=[FARGATE], networkMode=awsvpc, cpu=256, memory=512 and one sleeping container.
2. RunTask with launchType=FARGATE, count=1 and an awsvpcConfiguration naming a subnet and security group; capture tasks[0].taskArn.
3. As an ordinary signed caller (not the container agent), call ecs SubmitTaskStateChange(cluster=<cluster>, task=<taskArn>, status=<the task's current lastStatus from DescribeTasks>).
   Expected: HTTP 403 AccessDeniedException `Fargate task state changes from the agent must have the instance verified`. Observed: HTTP 200 {'acknowledgment': 'ACK'}.
4. StopTask the same task and poll DescribeTasks until lastStatus=STOPPED, then call SubmitTaskStateChange(cluster=<cluster>, task=<taskArn>, status='RUNNING').
   Expected: the same 403 AccessDeniedException, and DescribeTasks still reporting lastStatus=STOPPED. Observed: HTTP 200 {'acknowledgment': 'ACK'}.
5. (Behind the gate, for the same handler) call SubmitTaskStateChange(..., status='NOT_A_REAL_ECS_TASK_STATUS'): AWS refuses with InvalidParameterException and `The status NOT_A_REAL_ECS_TASK_STATUS is not a valid status.`
