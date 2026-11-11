Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__firehose__s3.cfn-firehose-delivery-stream-in-place-prefix-update-retained-replacement-then-not-found
Issue-Key: cloudformation__firehose__s3.cfn-firehose-delivery-stream-in-place-prefix-update-retained-replacement-then-not-found|_expect#28|AssertionError|raise AssertionError(
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# In-place update of AWS::KinesisFirehose::DeliveryStream is refused with an emulator-authored NotImplemented reason

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: changing a mutable property of a Firehose delivery stream through CloudFormation is refused instead of applied.

AWS: recorded behavior only. Changing the S3 destination Prefix (or buffering hints, compression, error prefix) of AWS::KinesisFirehose::DeliveryStream is an in-place update applied through Firehose UpdateDestination; the delivery stream name (physical id) is unchanged, the resource goes UPDATE_IN_PROGRESS -> UPDATE_COMPLETE and the stack reaches UPDATE_COMPLETE. Only a DeliveryStreamName change forces replacement.

Emulator: the resource is UPDATE_FAILED and the stack ends 'UPDATE_ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to update: [Stream]. '; the reason is 'NotImplemented: this emulator does not change the properties of an existing AWS::KinesisFirehose::DeliveryStream in place; the resource was left as it is.' (fixed text 'NotImplemented: this emulator does not change the properties of an existing ' ... ' in place; the resource was left as it is.'; variable part = resource type). AWS has no NotImplemented error for a supported property change.

Why: a client cannot repoint or retune a CloudFormation-managed delivery stream — every prefix or buffering change rolls the stack back, so the delivered object layout can never be altered.

Fix: the AWS::KinesisFirehose::DeliveryStream resource handler's update path in the CloudFormation provisioner — classify destination changes as in-place and call UpdateDestination.

Traces: the failing check in trace_ids.

## Evidence
Expected: UPDATE_COMPLETE with the delivery stream retained.
Actual: AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Stream]. '), expected 'UPDATE_COMPLETE'; failed events: [('Stream', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::KinesisFirehose::DeliveryStream in place; the resource was left as it is.')]

## Steps to Reproduce
1. CreateStack a template with an AWS::KinesisFirehose::DeliveryStream writing to an S3 bucket; wait for CREATE_COMPLETE.
2. UpdateStack changing only the destination Prefix.
3. DescribeStackEvents: Stream is UPDATE_FAILED with the NotImplemented reason; the stack is UPDATE_ROLLBACK_COMPLETE.
