Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3__ssm.cfn-nested-stack-in-place-parameter-update-reaches-child-resource-then-not-found
Issue-Key: cloudformation__s3__ssm.cfn-nested-stack-in-place-parameter-update-reaches-child-resource-then-not-found|run#21|AssertionError|assert out == {"ChildId": child_id, "ChildParam": param_name}, (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# Fn::GetAtt Outputs.<name> on a nested AWS::CloudFormation::Stack resolves to null in the parent stack

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: a nested stack's outputs are not readable from its parent.

AWS: Fn::GetAtt <NestedStackLogicalId>.Outputs.<OutputName> resolves to the value of that output in the child stack, so a parent output built from it carries the child's resolved value (here the SSM parameter name the child created). Ref on the nested stack resolves to the child stack id, and both are available once the parent reaches CREATE_COMPLETE.

Emulator: Ref resolves (the child stack id is returned) but the GetAtt-on-Outputs value is null: parent outputs were {'ChildId': 'arn:aws:cloudformation:<region>:<account>:stack/<parent>-Child-<suffix>/<uuid>', 'ChildParam': None} where ChildParam had to be the child's ParamName. A stack output never carries a null on AWS — an output that cannot be resolved fails the stack.

Why: GetAtt-on-nested-outputs is the only supported way to pass a value out of a nested stack. With a null, every parent/child wiring in nested templates, CDK nested stacks and SAM nested applications silently loses the value, and anything that then uses it addresses an empty name.

Fix: the AWS::CloudFormation::Stack resource handler's GetAtt resolution for Outputs.<name> (it must read the child stack's resolved outputs) and the parent stack-output serializer.

Traces: the failing check in trace_ids.

## Evidence
Expected: parent outputs == {'ChildId': '<child stack id>', 'ChildParam': '<the child's ParamName>'}.
Actual: AssertionError: parent outputs {'ChildId': 'arn:aws:cloudformation:us-west-2:000000000000:stack/sim2real-cfnnest-stack-...-Child-.../321c20ae-...', 'ChildParam': None}

## Steps to Reproduce
1. CreateStack a parent template with an AWS::CloudFormation::Stack child (TemplateURL in S3) whose child template creates an AWS::SSM::Parameter and exports its name as an Output.
2. In the parent, Output Fn::GetAtt Child.Outputs.ParamName.
3. DescribeStacks on the parent: that output's value is null.
