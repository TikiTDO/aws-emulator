Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__stepfunctions.events-putevents-task-without-permission-is-caught-as-access-denied
Issue-Key: events__stepfunctions.events-putevents-task-without-permission-is-caught-as-access-denied|run#10|AssertionError|assert out.get("handled") == "denied", f"the Denied state did not run: {out!r}"
Issue-Commit: b088d29fc6cbae0dbe15d6c5c29ce12fa9f3ea19

# events:putEvents Task integration ignores the execution role's missing events:PutEvents permission and succeeds

> _Note: this issue was identified against the code base at commit `b088d29fc6cbae0dbe15d6c5c29ce12fa9f3ea19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: an EventBridge PutEvents Task integration is not authorized against the execution role, so a denial that AWS surfaces as a catchable task failure is served as a success.

AWS: a Task state whose Resource is the EventBridge PutEvents integration calls EventBridge with credentials obtained by assuming the state machine's execution role. When that role carries no identity-based statement allowing events:PutEvents on the target bus, EventBridge refuses the call: HTTP 400, error code AccessDeniedException (Smithy: com.amazonaws.sfn DescribeExecutionOutput.error/.cause are the fields that carry the task failure; the EventBridge refusal itself is recorded behavior only — the sfn model does not enumerate integration error names). Step Functions relays the refusal as a task failure whose ASL error name is 'EventBridge.AmazonEventBridgeException' and whose Cause is the relayed SDK exception message. The fixed text of that cause is: "User: <assumed-role principal ARN> is not authorized to perform: events:PutEvents on resource: <bus ARN> because no identity-based policy allows the events:PutEvents action (Service: AmazonEventBridge; Status Code: 400; Error Code: AccessDeniedException; Request ID: <id>; Proxy: null)". Its variable parts are taken from the request and from state: the partition and account of the caller, the execution role's name and its session name in the assumed-role ARN, the event bus ARN named by the entry, and the per-call request id. A Catch on 'EventBridge.AmazonEventBridgeException' therefore fires and the execution can end SUCCEEDED on the handler branch.

Emulator: the same Task under a role with an EMPTY policy document completes successfully. The task result is the ordinary PutEvents success envelope — {'FailedEntryCount': 0, 'Entries': [{'EventId': ...}], 'SdkHttpMetadata': {... 'HttpStatusCode': 200}} — so no error is raised, the Catch never matches, and the 'Denied' handler state is never entered (the execution output has no 'handled' member). No AccessDeniedException, no 400, and no 'EventBridge.AmazonEventBridgeException' error name is produced at all.

Why: a client that relies on AWS's behavior gets the opposite verdict from the emulator. Least-privilege tests pass vacuously: a workflow that must fail (or must take its documented error-handling branch) instead publishes the event and continues down the happy path. Any test asserting that an unauthorized PutEvents task is caught, that the denial names the role and the action, or that no event reaches the bus, cannot be written against the emulator, and a policy regression in the customer's own template is reported as working.

Fix: the EventBridge PutEvents task integration's authorization step in the Step Functions task-dispatch path — the integration must assume the execution role and have EventBridge's PutEvents handler evaluate the resulting principal's identity-based policy before accepting entries, and the task-failure serializer must then render the 'EventBridge.<Exception>' error name plus the relayed AccessDeniedException cause rather than a 200 envelope.

Traces: events__stepfunctions.events-putevents-task-without-permission-is-caught-as-access-denied (the positive-control sibling role, which DOES hold events:PutEvents on the default bus, is expected to succeed and is not what diverges).

## Evidence
Check: events__stepfunctions.events-putevents-task-without-permission-is-caught-as-access-denied

Expected (AWS): execution SUCCEEDED via the Catch branch, output containing handled == "denied" and err.Error == "EventBridge.AmazonEventBridgeException", with err.Cause matching
  "User: arn:{partition}:sts::{account}:assumed-role/{role}/<session> is not authorized to perform: events:PutEvents on resource: {bus} because no identity-based policy allows the events:PutEvents action (Service: AmazonEventBridge; Status Code: 400; Error Code: AccessDeniedException; Request ID: <id>; Proxy: null)"

Actual (emulator): the task succeeded and the execution output was the raw PutEvents response:
  AssertionError: the Denied state did not run: {'FailedEntryCount': 0, 'Entries': [{'EventId': '...'}], 'SdkHttpMetadata': {'AllHttpHeaders': {'content-type': ['application/x-amz-json-1.1'], 'content-length': ['85']}, 'HttpHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '85'}, 'HttpStatusCode': 200}, 'SdkResponseMetadata': {}}

Status/code divergence: expected a relayed 400 / AccessDeniedException inside the task cause; got HTTP 200 and no error at all.

## Steps to Reproduce
1. CreateRole with a trust policy allowing states.amazonaws.com to sts:AssumeRole, and attach NO permission policy at all (empty statement list).
2. CreateStateMachine (STANDARD) with a Task state whose Resource is the EventBridge PutEvents integration, Parameters naming one entry on the default event bus, and a Catch on ErrorEquals ["EventBridge.AmazonEventBridgeException"] routing to a Pass state that emits {"handled": "denied", "err": $$...}.
3. StartExecution and poll DescribeExecution to a terminal status.
4. Observe: the emulator ends SUCCEEDED with the PutEvents success envelope as output and never enters the Catch branch. AWS ends SUCCEEDED via the Catch branch with handled == "denied" and the AccessDeniedException cause above.
5. Positive control: repeat with a sibling role that allows events:PutEvents on arn:<partition>:events:<region>:<account>:event-bus/default — both AWS and the emulator succeed, confirming the only difference is that the denial is not enforced.
