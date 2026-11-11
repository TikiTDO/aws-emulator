Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly
Issue-Key: cloudformation__lambda__ssm|DescribeStacks|ValidationError
Issue-Commit: 5b7d66e77a060af6cd000fd4d7e09192c88c4502

# A CDK custom resource whose onEvent handler raises leaves no stack at all: DescribeStacks answers ValidationError "Stack with id <name> does not exist"

> _Note: this issue was identified against the code base at commit `5b7d66e77a060af6cd000fd4d7e09192c88c4502`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: when a custom resource's onEvent provider fails during CREATE, the emulator removes every trace of the stack instead of settling it in a terminal failed state that is still describable.
AWS: a CREATE whose custom resource signals FAILED moves the stack to CREATE_FAILED and then ROLLBACK_COMPLETE; a ROLLBACK_COMPLETE stack is retained and DescribeStacks(StackName=<name>) keeps answering 200 with that status (only after DeleteStack does DescribeStacks by name raise ValidationError "Stack with id <name> does not exist"), and the stack events name the failed custom resource.
Emulator: DescribeStacks on the stack the check just created raises ClientError (ValidationError) "Stack with id <name> does not exist" - HTTP 400 - so the failed create is unobservable. The fixed text is AWS's ValidationError wording; the stack name is the caller's.
Why: a client cannot tell a rollback from a never-created stack, cannot read the failure reason or events, and cannot drive the documented clean-up path.
Fix: the CloudFormation create/rollback state machine for custom-resource failures (retain the stack in ROLLBACK_COMPLETE) and the DescribeStacks handler's lookup of terminal-state stacks.
Traces: cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly.

## Evidence
Expected (AWS): after the onEvent handler raises, DescribeStacks(StackName=<stack>) returns the stack with StackStatus ROLLBACK_COMPLETE (preceded by CREATE_FAILED) and events naming the custom resource; the stack stays describable until it is deleted.
Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the DescribeStacks operation: Stack with id <stack-name> does not exist.

## Steps to Reproduce
1. Replay the behavioral suite and re-run cloudformation__lambda__ssm.cdk-custom-resource-onevent-raise-create-failed-rolls-back-cleanly.
2. Deploy a stack with an AWS::CloudFormation::CustomResource backed by a Lambda onEvent handler that raises on Create, then call DescribeStacks with the stack name.
