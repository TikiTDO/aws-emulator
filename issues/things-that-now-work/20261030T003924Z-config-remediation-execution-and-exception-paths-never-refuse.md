Issue-Source: trace-sweep-divergence
Issue-Trace: config-remediation-configuration-workflow
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-remediation-configuration-workflow
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|StartRemediationExecution|2xx->2xx|$.FailedItems[*]
Issue-Key: config|StartRemediationExecution|2xx->2xx|$.FailureMessage
Issue-Key: config|DescribeRemediationExecutionStatus|2xx->2xx|$.RemediationExecutionStatuses[*]
Issue-Key: config|PutRemediationExceptions|4xx->2xx|$.__type
Issue-Key: config|PutRemediationExceptions|4xx->2xx|$.message
Issue-Key: config|PutRemediationExceptions|4xx->2xx|status
Issue-Key: config|DescribeRemediationExceptions|2xx->2xx|$.RemediationExceptions[*]
Issue-Key: config|DeleteRemediationExceptions|4xx->2xx|$.__type
Issue-Key: config|DeleteRemediationExceptions|4xx->2xx|$.message
Issue-Key: config|DeleteRemediationExceptions|4xx->2xx|status
Issue-Key: config|DeleteRemediationConfiguration|4xx->4xx|$.message
Issue-Commit: a6cd77094d82d955b2186a9b9b1faf688af70a8f

# Config remediation execution and exception paths never refuse, fabricate state

> _Note: this issue was identified against the code base at commit `a6cd77094d82d955b2186a9b9b1faf688af70a8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartRemediationExecution

## Description
Six distinct causes in this service's remediation surface. The emulator accepts remediation requests AWS refuses because the target resource has no non-compliant evaluation, then reports the fabricated executions and exceptions back on the read paths, and words one not-found message differently. SIX CAUSES must be closed.

Cause 1 of 6 - StartRemediationExecution silently queues a remediation for a resource with no non-compliant evaluation (fixture config-remediation-configuration-workflow, step task#5).
AWS: StartRemediationExecution returns FailureMessage and FailedItems (ResourceKeys) in a 200 response (Smithy: StartRemediationExecution output; errors InsufficientPermissionsException / InvalidParameterValueException / NoSuchRemediationConfigurationException). The recording shows that when the named resource has no non-compliant EvaluationResult, AWS answers 200 and reports the resource under FailedItems (resourceId + resourceType from the request) together with the fixed FailureMessage "There are no non-compliant EvaluationResult for the resource(s) that are trying to be remediated. Please wait several minutes and try again when the resource(s) are non-compliant."
Emulator: answers 200 with neither FailedItems nor FailureMessage - the request is treated as fully accepted.
Why: a client that inspects FailedItems/FailureMessage to decide whether remediation actually started concludes it started; every subsequent assertion is made against an execution AWS never began.
Fix: StartRemediationExecution's handler - check each ResourceKey against recorded non-compliant evaluations and report unremediable keys under FailedItems with AWS's fixed FailureMessage instead of enqueueing them.

Cause 2 of 6 - DescribeRemediationExecutionStatus reports a fabricated QUEUED execution (fixture config-remediation-configuration-workflow, step task#6).
AWS: the recording shows an empty RemediationExecutionStatuses list, because no execution was started (see cause 1).
Emulator: returns one entry with State 'QUEUED', a StepDetails list ([{Name, State 'PENDING'}]) and InvocationTime/LastUpdatedTime.
Why: callers polling execution status see work in flight that does not exist on AWS, so state machines built on this read path take a branch AWS never takes.
Fix: DescribeRemediationExecutionStatus' handler/state - derive entries from executions that were actually started.

Cause 3 of 6 - PutRemediationExceptions accepts an exception for a resource that is not an evaluation target (fixture config-remediation-configuration-workflow, step task#7).
AWS: PutRemediationExceptions models InvalidParameterValueException (client, httpError 400, defaulted) (Smithy: PutRemediationExceptions.errors). The recording shows a 400 with that code when the ResourceKeys cannot be resolved for the rule, message fixed text "Could not find resources: " + the rule name + ": (" + resourceType + ", " + resourceId + ")", all variable parts taken from the request.
Emulator: answers 200 and persists the exception.
Why: the exception-validation gate is absent, so clients believe an exception is in force for a resource AWS refused to except; remediation suppression then differs between environments.
Fix: PutRemediationExceptions' handler validation - resolve each ResourceKey against the rule's evaluated resources and raise the modeled 400 InvalidParameterValueException instead of persisting.

Cause 4 of 6 - DescribeRemediationExceptions reports the exception AWS never accepted (fixture config-remediation-configuration-workflow, step task#8).
AWS: the recording shows an empty RemediationExceptions list (the put in cause 3 was refused).
Emulator: returns the persisted exception (ConfigRuleName, ResourceType, ResourceId, Message, ExpirationTime).
Why: a read-after-write check passes against the emulator and fails against AWS, so clients cannot detect that their exception was rejected.
Fix: follows cause 3 - once the put is refused nothing is stored; the read path must not synthesise entries.

Cause 5 of 6 - DeleteRemediationExceptions is wrongly idempotent (fixture config-remediation-configuration-workflow, step teardown#9).
AWS: DeleteRemediationExceptions models exactly one error, NoSuchRemediationExceptionException (client, 400) (Smithy: DeleteRemediationExceptions.errors). The recording shows 400 with that code and the fixed message "Specified RemediationException could not be found." when no matching exception exists.
Emulator: answers 200 with no FailedBatches.
Why: teardown and reconciliation code that relies on the delete failing for an absent exception cannot distinguish "removed" from "never there".
Fix: DeleteRemediationExceptions' handler - look the exception up first and raise the modeled 400 when absent.

Cause 6 of 6 - DeleteRemediationConfiguration's not-found message is reworded (fixture config-remediation-configuration-workflow, step teardown#11).
AWS: status 400 and the error class match; the recording shows the message is fixed text "No RemediationConfiguration for rule " + the rule name + " exists" - the rule name is NOT quoted and there is no trailing period.
Emulator: "No RemediationConfiguration for rule '<ruleName>' exists." - adds single quotes around the name and a trailing period.
Why: assertions and log scrapers that match AWS's exact sentence do not match.
Fix: DeleteRemediationConfiguration's handler error construction - emit the unquoted, period-free sentence.

No existing open issue covers the config remediation surface.

Traces: config-remediation-configuration-workflow#task#5 (StartRemediationExecution), #task#6 (DescribeRemediationExecutionStatus), #task#7 (PutRemediationExceptions), #task#8 (DescribeRemediationExceptions), #teardown#9 (DeleteRemediationExceptions), #teardown#11 (DeleteRemediationConfiguration).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false).

Cause 1 - StartRemediationExecution, config-remediation-configuration-workflow#task#5 (200 on both sides):
  $.FailedItems[0]: missing (expected {'resourceId': '<bucket>', 'resourceType': 'AWS::S3::Bucket'}, got None)
  $.FailureMessage: missing (expected 'There are no non-compliant EvaluationResult for the resource(s) that are trying to be remediated. Please wait several minutes and try again when the resource(s) are non-compliant.', got None)

Cause 2 - DescribeRemediationExecutionStatus, config-remediation-configuration-workflow#task#6 (200 on both sides):
  $.RemediationExecutionStatuses[0]: extra (expected None, got {'ResourceKey': {...}, 'State': 'QUEUED', 'StepDetails': [{'Name': 'AWS-PublishSNSNotification', 'State': 'PENDING'}], 'InvocationTime': ..., 'LastUpdatedTime': ...})

Cause 3 - PutRemediationExceptions, config-remediation-configuration-workflow#task#7:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParameterValueException'); $.message: missing (expected 'Could not find resources: <ruleName>: (AWS::S3::Bucket, <bucket>)')

Cause 4 - DescribeRemediationExceptions, config-remediation-configuration-workflow#task#8 (200 on both sides):
  $.RemediationExceptions[0]: extra (expected None, got {'ConfigRuleName': '<ruleName>', 'ResourceType': 'AWS::S3::Bucket', 'ResourceId': '<bucket>', 'Message': 'temporary exception', 'ExpirationTime': ...})

Cause 5 - DeleteRemediationExceptions, config-remediation-configuration-workflow#teardown#9:
  status: expected 400, got 200
  $.__type: missing (expected 'NoSuchRemediationExceptionException'); $.message: missing (expected 'Specified RemediationException could not be found.')

Cause 6 - DeleteRemediationConfiguration, config-remediation-configuration-workflow#teardown#11 (400 on both sides):
  $.message expected 'No RemediationConfiguration for rule <ruleName> exists'
  got "No RemediationConfiguration for rule '<ruleName>' exists."

(Rule names, bucket names and timestamps are per-run fixture values; the requirements are the statuses, codes and message templates.)

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): StartRemediationExecution output has FailureMessage + FailedItems; PutRemediationExceptions models InvalidParameterValueException (400); DeleteRemediationExceptions models NoSuchRemediationExceptionException (400).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: config-remediation-configuration-workflow#task#5 - StartRemediationExecution for a resource with no non-compliant evaluation; FailedItems/FailureMessage absent.
4. Cause 2: same fixture #task#6 - DescribeRemediationExecutionStatus returns a QUEUED entry AWS does not have.
5. Cause 3: same fixture #task#7 - PutRemediationExceptions for that resource; 200 instead of 400 InvalidParameterValueException.
6. Cause 4: same fixture #task#8 - DescribeRemediationExceptions returns the wrongly accepted exception.
7. Cause 5: same fixture #teardown#9 - DeleteRemediationExceptions with no matching exception; 200 instead of 400 NoSuchRemediationExceptionException.
8. Cause 6: same fixture #teardown#11 - DeleteRemediationConfiguration for an absent configuration; compare the message wording.
9. Re-replay all six fixture#step bindings to confirm the fix.
