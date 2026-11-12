Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore-control__logs.orphaned-runtime-log-group-names-runtime-not-found bedrock-agentcore-control__logs.runtime-delivery-source-duplicate-resource-log-type-conflict bedrock-agentcore-control__logs.runtime-delivery-source-unsupported-log-type-rejected
Issue-Key: bedrock-agentcore-control__logs.orphaned-runtime-log-group-names-runtime-not-found|run#7|AssertionError|assert [g["logGroupName"] for g in found] == [rt["group"]], f"orphaned group lookup {found!r}"
Issue-Key: bedrock-agentcore-control__logs.runtime-delivery-source-duplicate-resource-log-type-conflict|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, it succeeded: {result!r}")
Issue-Key: bedrock-agentcore-control__logs.runtime-delivery-source-unsupported-log-type-rejected|run#8|AssertionError|assert (code, message) == expected, f"unsupported logType returned {(code, message)!r}"
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# AgentCore runtime vended log group is never created and PutDeliverySource's AgentCore refusals diverge (3 causes)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliverySource

## Description
Three net-new root causes. Each must be closed before submitting.

Cause 1 of 3 — CreateAgentRuntime creates no vended CloudWatch Logs group, so the runtime's log group cannot be found or outlive the runtime.
AWS: recorded behavior only. Creating a Bedrock AgentCore runtime provisions the service's vended log group for that runtime, and DescribeLogGroups lists it by its name; the group survives DeleteAgentRuntime (it is an orphaned group whose runtime no longer resolves).
Emulator: the group is never created — the check's DescribeLogGroups lookup for the runtime's group name returns an empty list.
Why: nothing can read an AgentCore runtime's logs, and the orphaned-group lifecycle (group outliving its runtime) is unobservable; a client that resolves logs by the vended group name gets ResourceNotFoundException on every read.
Fix: the vended-log-group provisioning performed by CreateAgentRuntime in the bedrock-agentcore-control crate (and the logs-side group registration it drives). Note: open issue 20261101T213737Z-bedrock-agentcore-control-createagentruntime-creates-no-vended-cloudwatch-logs-group-for.md records this root cause but carries only the bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events key, not this check's.
Traces: bedrock-agentcore-control__logs.orphaned-runtime-log-group-names-runtime-not-found

Cause 2 of 3 — PutDeliverySource accepts a second delivery source for a resourceArn/logType pair that is already represented.
AWS: Smithy (com.amazonaws.cloudwatchlogs) PutDeliverySource, required members name, resourceArn, logType; models ConflictException with httpError 400. The model carries no message text, so for the text this is recorded behavior only: a second PutDeliverySource under a NEW name for a resourceArn that another delivery source already represents with the same logType is refused with ConflictException and the message `This ResourceId has already been used in another Delivery Source in this account.` — entirely fixed text, no variable part — and the second source is not created (GetDeliverySource on it answers ResourceNotFoundException `Requested Delivery Source does not exist in this account.`). A DIFFERENT logType for the same ARN is accepted.
Emulator: answers HTTP 200 and creates the duplicate source, returning its name, arn, resourceArns, service `bedrock-agentcore` and logType `APPLICATION_LOGS`.
Why: the uniqueness of (resource, logType) is what makes a delivery source addressable; two sources for one pair make the delivery wiring ambiguous, and a client that relies on the conflict to detect an existing wiring instead creates a second one.
Fix: the (resourceArn, logType) uniqueness check in PutDeliverySource's handler/state in the logs crate.
Traces: bedrock-agentcore-control__logs.runtime-delivery-source-duplicate-resource-log-type-conflict

Cause 3 of 3 — PutDeliverySource's unsupported-logType refusal uses the emulator's own enumerating sentence instead of AWS's fixed one.
AWS: PutDeliverySource models ValidationException with httpError 400. Model text only; the recorded refusal for a logType the named resource's service does not vend (here `ERROR_LOGS` on a bedrock-agentcore runtime ARN) is ValidationException with the message `This service is not allowed for this LogSource.` — entirely fixed text, no variable part, and no enumeration of the supported types.
Emulator: answers ValidationException (code correct) with `Provided LogType is not valid. Supported options are [APPLICATION_LOGS, TRACES, USAGE_LOGS].` — a different fixed sentence that also leaks a supported-type list AWS does not emit here.
Why: a client matching the documented sentence does not recognise the refusal; the divergence is message-only.
Fix: the logType validation message for PutDeliverySource in the logs crate's validation layer.
Traces: bedrock-agentcore-control__logs.runtime-delivery-source-unsupported-log-type-rejected

## Evidence
All from the simparity-behavioral-aws-v1 replay against this checkout.

Cause 1 — orphaned-runtime-log-group-names-runtime-not-found:
  AssertionError: orphaned group lookup [] (DescribeLogGroups returned no group for the runtime; one group named after the runtime was expected)

Cause 2 — runtime-delivery-source-duplicate-resource-log-type-conflict:
  AssertionError: expected the call to be rejected, it succeeded: {'deliverySource': {'name': '<name>-b', 'arn': 'arn:aws:logs:<region>:<account>:delivery-source:<name>-b', 'resourceArns': ['arn:aws:bedrock-agentcore:<region>:<account>:runtime/<runtimeId>'], 'service': 'bedrock-agentcore', 'logType': 'APPLICATION_LOGS', ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  expected ('ConflictException', 'This ResourceId has already been used in another Delivery Source in this account.')

Cause 3 — runtime-delivery-source-unsupported-log-type-rejected:
  expected ('ValidationException', 'This service is not allowed for this LogSource.')
  actual   ('ValidationException', 'Provided LogType is not valid. Supported options are [APPLICATION_LOGS, TRACES, USAGE_LOGS].')
  The resource ARNs, names and ids above are this run's values filling the request; they are not part of the requirement.

## Steps to Reproduce
Boot the emulator from this checkout.

Cause 1: aws bedrock-agentcore-control create-agent-runtime ... ; then aws logs describe-log-groups --log-group-name-prefix <the runtime's vended group name> -> empty. AWS lists the group, and it remains after delete-agent-runtime.

Cause 2: aws logs put-delivery-source --name s-a --resource-arn arn:aws:bedrock-agentcore:<region>:<account>:runtime/<id> --log-type APPLICATION_LOGS ; then the same with --name s-b and the same ARN/log-type -> 200 and a second source is created; AWS answers ConflictException 'This ResourceId has already been used in another Delivery Source in this account.' and creates nothing. --log-type USAGE_LOGS for the same ARN must still be accepted.

Cause 3: aws logs put-delivery-source --name s-bad --resource-arn arn:aws:bedrock-agentcore:<region>:<account>:runtime/<id> --log-type ERROR_LOGS -> ValidationException 'Provided LogType is not valid. Supported options are [...]'; AWS answers 'This service is not allowed for this LogSource.'

Then re-replay: bedrock-agentcore-control__logs.orphaned-runtime-log-group-names-runtime-not-found, .runtime-delivery-source-duplicate-resource-log-type-conflict, .runtime-delivery-source-unsupported-log-type-rejected
