Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sagemaker.cfn-sagemaker-model-in-place-tags-retained-replacement-then-not-found
Issue-Key: cloudformation__sagemaker.cfn-sagemaker-model-in-place-tags-retained-replacement-then-not-found|_assert_tracks#15|AssertionError|assert got == expected, f"the stack must track {logical_id} as {expected!r}, got {got!r}"
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# AWS::SageMaker::Model physical resource id is the model name instead of the model ARN

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResources

## Description
Cause 1 of 1: the stack tracks the model by name where AWS tracks it by ARN.

AWS: recorded behavior only. The physical resource id of AWS::SageMaker::Model is the model ARN, 'arn:aws:sagemaker:<region>:<account>:model/<model-name>'; that is what Fn::Ref resolves to and what SageMaker's tagging and resource-reference APIs take.

Emulator: the physical resource id is the bare model name — the stack tracks 'sim2real-cfnreal-sm-a-c309f2373eaf' where 'arn:aws:sagemaker:us-west-2:<account>:model/sim2real-cfnreal-sm-a-c309f2373eaf' was required.

Why: an ARN is an identity. A template that feeds the Ref into an endpoint configuration, an IAM policy resource, ListTagsForResource or any cross-service reference passes a bare name where an ARN is expected, so authorization and lookups fail or match the wrong thing; a client comparing the stack's physical id against SageMaker's reported ARN sees a mismatch.

Fix: the AWS::SageMaker::Model resource handler in the CloudFormation provisioner — return the model ARN as the physical resource id.

Traces: the failing check in trace_ids.

## Evidence
Expected: the stack tracks Model as 'arn:aws:sagemaker:us-west-2:000000000000:model/sim2real-cfnreal-sm-a-c309f2373eaf'.
Actual: AssertionError: the stack must track Model as 'arn:aws:sagemaker:us-west-2:000000000000:model/sim2real-cfnreal-sm-a-c309f2373eaf', got 'sim2real-cfnreal-sm-a-c309f2373eaf'

## Steps to Reproduce
1. CreateStack a template with an AWS::SageMaker::Model (ExecutionRoleArn plus a primary container); wait for CREATE_COMPLETE.
2. DescribeStackResources: the model's PhysicalResourceId is the model name, not its ARN.
