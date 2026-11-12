Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3.cfn-deploy-changeset-references-s3-template-positive
Issue-Key: cloudformation__dynamodb__s3.cfn-deploy-changeset-references-s3-template-positive|<genexpr>#24|KeyError|nested_bucket_name = next(o["OutputValue"] for o in outputs if o["OutputKey"] == "NestedBucketName")
Issue-Commit: 72e40d23ae86c919f91b622f812baa35119cc1ff

# cloudformation__dynamodb__s3: a nested-stack Fn::GetAtt Output is returned with no OutputValue, so an `aws cloudformation package`/`deploy` change-set workflow cannot read back the resource the nested stack created (1 cause)

> _Note: this issue was identified against the code base at commit `72e40d23ae86c919f91b622f812baa35119cc1ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: DescribeStacks returns the root stack's Outputs list with the OutputKey present but OutputValue absent for an output whose template Value is {"Fn::GetAtt": "Nested.Outputs.BucketName"}, so the nested stack's output cannot be read back at all.

AWS: Smithy (com.amazonaws.cloudformation) — DescribeStacks returns Stacks: list<Stack>; Stack.Outputs: list<Output>; Output has members OutputKey ("The key associated with the output"), OutputValue ("The value associated with the output"), Description and ExportName. For a stack in CREATE_COMPLETE, every entry of Stack.Outputs carries BOTH OutputKey and OutputValue, and an output declared as {"Fn::GetAtt": "<NestedLogicalId>.Outputs.<NestedOutputKey>"} resolves to the value the nested stack published for that key — here the physical name of the S3 bucket the nested template created, so that s3:HeadBucket on it answers 200. Recorded behavior for the surrounding workflow: `aws cloudformation package` uploads the nested template to the S3 bucket and rewrites TemplateURL to the S3 object; `aws cloudformation deploy` then creates and executes a change set (observable in cfn:ListChangeSets while it is still active) and the root stack settles at CREATE_COMPLETE with its nested AWS::CloudFormation::Stack child actually provisioned.

Emulator: the stack does reach CREATE_COMPLETE, but the Outputs entry for OutputKey "NestedBucketName" has no OutputValue member at all, so a client reading `next(o["OutputValue"] for o in outputs if o["OutputKey"] == "NestedBucketName")` raises KeyError: 'OutputValue'. Because no value is produced, it cannot be shown that the nested template's content was ever fetched from S3 and its AWS::S3::Bucket created.

Why: every consumer of stack outputs breaks on this. Stack outputs are the documented hand-off between a stack and its caller (CDK/SAM --outputs-file, `aws cloudformation describe-stacks --query Stacks[0].Outputs`, cross-stack wiring). A client that reads Outputs gets a dict with only OutputKey and must either crash or treat the output as empty, and nested-stack GetAtt outputs in particular are how a parent stack exposes a child's resources.

Fix: belongs in the CloudFormation DescribeStacks output serializer together with the stack's output-resolution step — Output.OutputValue must be populated from the resolved template Output Value, including resolution of Fn::GetAtt "<NestedLogicalId>.Outputs.<Key>" against the nested stack's own resolved Outputs. Related OPEN issues describe the same symptom family but none carries this check's replay key, so they do not cover it: 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md, 20261023T114649Z-cloudformation-outputs-carry-no-outputvalue-failed-resources-never.md, 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md, 20261027T101632Z-cloudformation-2-net-new-behavioral-root-causes-an.md.

Traces: cloudformation__dynamodb__s3.cfn-deploy-changeset-references-s3-template-positive.

## Evidence
Check cloudformation__dynamodb__s3.cfn-deploy-changeset-references-s3-template-positive [error]
  Expected (real CloudFormation): after `aws cloudformation deploy` of a packaged root template whose Nested resource is AWS::CloudFormation::Stack with TemplateURL pointing at the packaged S3 object, DescribeStacks(StackName=<root>)["Stacks"][0]["Outputs"] contains {"OutputKey": "NestedBucketName", "OutputValue": "<the bucket name the nested stack created>"}, and s3:HeadBucket(Bucket=<that value>) answers 200.
  Actual: the Outputs entry for "NestedBucketName" has no OutputValue member; the check fails with
    KeyError: 'OutputValue'
  at `nested_bucket_name = next(o["OutputValue"] for o in outputs if o["OutputKey"] == "NestedBucketName")`.
  Root fact: the stack settles at CREATE_COMPLETE and the output KEY is reported, but no value is ever attached to it, so the nested stack's resource is unreachable. (The bucket/stack names are per-run values; the requirement is that OutputValue is present and resolves to the nested stack's published output.)

## Steps to Reproduce
1. s3:CreateBucket a staging bucket.
2. Write nested.json = {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Bucket":{"Type":"AWS::S3::Bucket"}},"Outputs":{"BucketName":{"Value":{"Ref":"Bucket"}}}} and root.json = {"Resources":{"Nested":{"Type":"AWS::CloudFormation::Stack","Properties":{"TemplateURL":"./nested.json"}}},"Outputs":{"NestedBucketName":{"Value":{"Fn::GetAtt":"Nested.Outputs.BucketName"}}}}.
3. Run `aws cloudformation package --template-file root.json --s3-bucket <bucket> --use-json --output-template-file packaged.json`.
4. Run `aws cloudformation deploy --template-file packaged.json --stack-name <stack> --s3-bucket <bucket> --tags sim2real:managed=true`.
5. Poll cfn:DescribeStacks(StackName=<stack>) until CREATE_COMPLETE.
6. Read Stacks[0].Outputs: the entry with OutputKey "NestedBucketName" has no OutputValue key (AWS returns the nested stack's bucket name there).
7. With AWS's value, s3:HeadBucket(Bucket=<OutputValue>) answers 200; here there is no value to probe with.
