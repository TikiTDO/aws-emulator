Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3.cfn-deploy-changeset-references-s3-template-positive
Issue-Key: cloudformation__dynamodb__s3.cfn-deploy-changeset-references-s3-template-positive|<genexpr>#26|KeyError|nested_bucket_name = next(o["OutputValue"] for o in outputs if o["OutputKey"] == "NestedBucketName")
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# cloudformation__dynamodb__s3: DescribeStacks returns stack Outputs entries with no OutputValue, so a change-set deploy that references an S3 template cannot read back its nested-stack outputs (1 cause)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: DescribeStacks reports the stack's Outputs as entries that carry an OutputKey but no OutputValue, so no consumer can resolve an output.

AWS: CloudFormation's Output member of the Stack shape carries OutputKey and OutputValue (Smithy: Output.OutputKey, Output.OutputValue, plus optional Description/ExportName). For a stack that reached CREATE_COMPLETE and declares Outputs — here a nested bucket name resolved through !Ref / !GetAtt on a template referenced from S3 via a change set — DescribeStacks returns each declared output with its resolved OutputValue. The value is the resolved physical attribute (the real bucket name), not the logical id.

Emulator: the Outputs list contains entries without the OutputValue key at all. The check reads `next(o["OutputValue"] for o in outputs if o["OutputKey"] == "NestedBucketName")` and gets KeyError: 'OutputValue'. The member is absent from the serialized response rather than empty, so even a defensive client cannot distinguish "no value yet" from "unresolvable".

Why: stack outputs are the documented hand-off between a template and everything downstream (cross-stack Fn::ImportValue, CLI/CDK consumers, scripts that address created resources). With OutputValue missing, every output-consuming workflow fails at the first read — a client cannot learn the name/ARN/id of anything a stack created, and a change-set deploy that references a template in S3 looks successful while being unusable.

Fix: the CloudFormation DescribeStacks response serializer and the stack-output resolution in the stack state/provisioning component — outputs must be resolved to the created resources' physical values at stack completion and emitted as OutputValue. A family of open issues describes the same missing member reached through other services (20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md, 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md, 20261024T024248Z-cloudwatch-redshift-cloudformation-describestacks-returns-stack-outputs-entries-without-outputvalue.md); none carries this check's replay key, and none covers the change-set/S3-template path.

Traces: cloudformation__dynamodb__s3.cfn-deploy-changeset-references-s3-template-positive

## Evidence
Expected (AWS): DescribeStacks -> Stacks[0].Outputs contains {"OutputKey": "NestedBucketName", "OutputValue": "<the real bucket name created by the nested template>"} (the key is fixed template text; the value is the resolved physical name from stack state).

Actual (emulator): the matching entry has an OutputKey but no OutputValue member:
  File "check_cloudformation__dynamodb__s3_cfn_deploy_changeset_references_s3_template_positive.py", line 110, in run
    nested_bucket_name = next(o["OutputValue"] for o in outputs if o["OutputKey"] == "NestedBucketName")
  KeyError: 'OutputValue'

## Steps to Reproduce
1. PutObject a nested template (creating an S3 bucket) into a bucket; note its URL.
2. Author a parent template whose resource references the S3-hosted template and whose Outputs export the nested bucket name via !GetAtt/!Ref.
3. CreateChangeSet(TemplateURL=<s3 url>) then ExecuteChangeSet; wait for CREATE_COMPLETE.
4. DescribeStacks(StackName=<stack>) and read Stacks[0].Outputs.
Observed: output entries have OutputKey but no OutputValue. Expected: every declared output carries its resolved OutputValue.
