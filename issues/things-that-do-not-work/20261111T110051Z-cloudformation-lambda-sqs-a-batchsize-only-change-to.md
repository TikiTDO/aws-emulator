Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs.cfn-lambda-sqs-event-source-mapping-in-place-batch-retained-replacement-both-deliver-then-not-found
Issue-Key: cloudformation__lambda__sqs.cfn-lambda-sqs-event-source-mapping-in-place-batch-retained-replacement-both-deliver-then-not-found|_expect#21|AssertionError|raise AssertionError(
Issue-Commit: 56af4a38801370870cefc18c3ef22e7084182017

# cloudformation__lambda__sqs: a BatchSize-only change to AWS::Lambda::EventSourceMapping is executed as a replacement instead of an in-place update, so the stack update fails ResourceConflict and rolls back

> _Note: this issue was identified against the code base at commit `56af4a38801370870cefc18c3ef22e7084182017`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
One net-new root cause, grounded in a failing behavioral check the replay report left unmarked (no ALREADY-FILED marker, no OWNED marker).

Cause 1 of 1 — CloudFormation classifies a change to AWS::Lambda::EventSourceMapping.BatchSize as requiring a new physical resource. It creates a second mapping for the same (EventSourceArn, FunctionName) pair instead of calling Lambda's UpdateEventSourceMapping on the existing UUID, Lambda correctly refuses the duplicate with 409, and the whole stack update rolls back.

AWS: per the AWS::Lambda::EventSourceMapping resource reference, BatchSize has Update requires: No interruption — it is an in-place-updatable property, unlike the create-only EventSourceArn. AWS's resource handler therefore calls lambda:UpdateEventSourceMapping(UUID=<existing>, BatchSize=<new>), and the stack emits exactly `UPDATE_IN_PROGRESS` then `UPDATE_COMPLETE` for the logical resource, both carrying the SAME physical id (the mapping's original UUID); the stack settles UPDATE_COMPLETE and GetEventSourceMapping reports the new BatchSize on the unchanged UUID. AWS's own 409 for a genuinely duplicate create is modeled as ResourceConflictException with the message "An event source mapping with SQS arn (\" <queue-arn> \") and function (\" <function-name> \") already exists. Please update or delete the existing mapping with UUID <uuid>" — the emulator reproduces that message correctly; the defect is that the request which provokes it is made at all.

Emulator: the BatchSize-only UpdateStack is planned as a replacement. The logical resource `Mapping` goes CREATE_FAILED with `Resource handler returned message: "An event source mapping with SQS arn (\" <source-queue-arn> \") and function (\" <function-name> \") already exists. Please update or delete the existing mapping with UUID <uuid> (Service: Lambda, Status Code: 409, Request ID: <id>) (SDK Attempt Count: 1)" (HandlerErrorCode: ResourceConflict)`, and the stack ends UPDATE_ROLLBACK_COMPLETE with StackStatusReason "The following resource(s) failed to create: [Mapping]." instead of UPDATE_COMPLETE.

Why: a client cannot tune an SQS event source mapping through its stack at all — the single most common operational change to an ESM (raise or lower BatchSize) fails the stack and rolls it back. Worse, the failure mode is a self-collision the client cannot act on: the "already exists" message points at the stack's own mapping, so the suggested remedy (delete the existing mapping) means deleting the resource the stack owns. Any property CloudFormation mis-classifies as create-only on a resource whose service enforces uniqueness fails this way, so pipelines that update a stack get a rollback where AWS performs a no-interruption update.

Fix: the CloudFormation change-set/update planner's per-property replacement classification for AWS::Lambda::EventSourceMapping (the resource handler's update path in the cloudformation service crate) — BatchSize (and the other no-interruption members: Enabled, MaximumBatchingWindowInSeconds, FilterCriteria, FunctionResponseTypes, ScalingConfig, MaximumConcurrency) must be applied with lambda:UpdateEventSourceMapping on the existing physical id and reported as UPDATE_IN_PROGRESS/UPDATE_COMPLETE on that same id; only the create-only members (EventSourceArn, StartingPosition, FunctionName change to a different function) may trigger a replacement.

Traces: cloudformation__lambda__sqs.cfn-lambda-sqs-event-source-mapping-in-place-batch-retained-replacement-both-deliver-then-not-found. The later legs of that check — the UpdateReplacePolicy: Retain replacement onto a second queue, both mappings delivering, and the post-delete not-found — were never reached and are not claimed here; they must be re-asserted after this fix.

## Evidence
cloudformation__lambda__sqs.cfn-lambda-sqs-event-source-mapping-in-place-batch-retained-replacement-both-deliver-then-not-found — verdict mismatch.

Leg 1 (create) passed: a stack with an AWS::SQS::Queue `SourceA`, a python3.12 function and an AWS::Lambda::EventSourceMapping {EventSourceArn: GetAtt SourceA.Arn, FunctionName: Ref Fn, BatchSize: 1, Enabled: true, UpdateReplacePolicy: Retain} reached CREATE_COMPLETE, GetEventSourceMapping reported the mapping Enabled with BatchSize 1, and a probe sent to SourceA was forwarded to the sink.

Leg 2 (in-place) diverges. The same template is re-submitted with BatchSize 5 and nothing else changed.
  Expected: the stack settles UPDATE_COMPLETE; DescribeStackResources still reports the original mapping UUID for logical id `Mapping`; the resource's new events are exactly [UPDATE_IN_PROGRESS, UPDATE_COMPLETE] with the last one carrying that same UUID; GetEventSourceMapping reports BatchSize 5 on it.
  Actual (AssertionError from the check's _expect):
    stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to create: [Mapping]. '), expected 'UPDATE_COMPLETE'; failed events: [('Mapping', 'CREATE_FAILED', 'Resource handler returned message: "An event source mapping with SQS arn (" <SourceA arn> ") and function (" <function-name> ") already exists. Please update or delete the existing mapping with UUID <uuid> (Service: Lambda, Status Code: 409, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: ResourceConflict)')]

The CREATE_FAILED status is itself the evidence of the mis-classification: an in-place update never produces a CREATE_* event for the logical resource. The queue ARN, function name, UUID, request id and request token above are per-run values; the requirement is the UPDATE_IN_PROGRESS/UPDATE_COMPLETE event pair on the unchanged physical id and the UPDATE_COMPLETE stack status.

Reference: https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-lambda-eventsourcemapping.html (BatchSize — Update requires: No interruption).

## Steps to Reproduce
1. cloudformation:CreateStack with a template containing: an AWS::SQS::Queue `SourceA`, an AWS::SQS::Queue `Sink`, an execution role, an AWS::Lambda::Function forwarding its SQS records to `Sink`, and an AWS::Lambda::EventSourceMapping `Mapping` with {EventSourceArn: !GetAtt SourceA.Arn, FunctionName: !Ref Fn, BatchSize: 1, Enabled: true} and `UpdateReplacePolicy: Retain`. Wait for CREATE_COMPLETE and note the mapping's physical id (its UUID).
2. cloudformation:UpdateStack with the identical template except BatchSize: 5.
3. Poll DescribeStacks to a terminal status. Expected UPDATE_COMPLETE; observed UPDATE_ROLLBACK_COMPLETE with StackStatusReason "The following resource(s) failed to create: [Mapping]."
4. DescribeStackEvents for the stack: expected the logical resource `Mapping` to show only UPDATE_IN_PROGRESS then UPDATE_COMPLETE, both with the original UUID as PhysicalResourceId; observed a CREATE_FAILED whose reason is Lambda's 409 ResourceConflict naming the stack's own existing mapping UUID.
5. After the fix, confirm GetEventSourceMapping(UUID=<original>) reports BatchSize 5, State Enabled, and the same EventSourceArn/FunctionArn.
