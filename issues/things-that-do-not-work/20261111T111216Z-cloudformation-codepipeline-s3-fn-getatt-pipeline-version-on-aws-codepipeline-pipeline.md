Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__codepipeline__s3.cfn-codepipeline-pipeline-in-place-update-deploys-retained-replacement-then-not-found
Issue-Key: cloudformation__codepipeline__s3.cfn-codepipeline-pipeline-in-place-update-deploys-retained-replacement-then-not-found|run#22|AssertionError|assert outputs["Version"] == "1", f"GetAtt Pipeline.Version of a new pipeline: {outputs!r}"
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# Fn::GetAtt Pipeline.Version on AWS::CodePipeline::Pipeline resolves to null

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: the Version attribute of a CloudFormation-created pipeline is not populated.

AWS: AWS::CodePipeline::Pipeline exposes a Version attribute; for a newly created pipeline Fn::GetAtt <Pipeline>.Version resolves to the string "1" (CodePipeline's pipeline version, incremented on each structure update), and it appears in the stack output as that value. A stack output never carries a null value — an output that cannot be resolved fails the stack.

Emulator: the stack output for GetAtt Pipeline.Version comes back as None while the sibling RoleArn output resolves: outputs were {'RoleArn': 'arn:aws:iam::<account>:role/<stack>-PipelineRole-...', 'Version': None}.

Why: a client that wires the pipeline version into another resource, or that asserts the pipeline was created at version 1 and incremented by a structure update, reads a null instead of a number; any string operation on it fails, and update-detection built on the version is impossible.

Fix: the AWS::CodePipeline::Pipeline resource handler's GetAtt attribute resolution (and the pipeline-version field of CodePipeline state it must read).

Traces: the failing check in trace_ids.

## Evidence
Expected: outputs['Version'] == '1' for a newly created pipeline.
Actual: AssertionError: GetAtt Pipeline.Version of a new pipeline: {'RoleArn': 'arn:aws:iam::000000000000:role/...-PipelineRole-CC693FD1C881', 'Version': None}

## Steps to Reproduce
1. CreateStack a template with an AWS::CodePipeline::Pipeline (S3 source/deploy) and Outputs exporting Fn::GetAtt Pipeline.Version and Pipeline.RoleArn (or the role).
2. DescribeStacks: the Version output value is null.
