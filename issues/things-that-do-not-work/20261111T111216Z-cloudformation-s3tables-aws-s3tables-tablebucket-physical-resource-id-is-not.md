Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3tables.cfn-s3tables-table-bucket-in-place-update-retained-replacement-hosts-table-then-not-found
Issue-Key: cloudformation__s3tables.cfn-s3tables-table-bucket-in-place-update-retained-replacement-hosts-table-then-not-found|run#19|AssertionError|assert _physical_ids(cfn, ref["stack"])["Bucket"] == first_arn, (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# AWS::S3Tables::TableBucket physical resource id is not the table bucket ARN

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResources

## Description
Cause 1 of 1: the table bucket resource is tracked by something other than its ARN.

AWS: recorded behavior only. The physical resource id of AWS::S3Tables::TableBucket is the table bucket ARN, 'arn:aws:s3tables:<region>:<account>:bucket/<name>' — that is what Fn::Ref resolves to and what every S3 Tables call (GetTableBucket, CreateNamespace, CreateTable, PutTableBucketPolicy) takes as tableBucketARN.

Emulator: the physical resource id the stack tracks for the bucket is not that ARN ('the table bucket\'s physical id must be its ARN arn:aws:s3tables:us-west-2:<account>:bucket/<name>').

Why: the Ref value a template feeds to AWS::S3Tables::Namespace / ::Table / ::TableBucketPolicy, and anything an operator passes to the S3 Tables API, is not a tableBucketARN AWS would accept — every dependent resource and lookup is addressed to an identifier that names nothing.

Fix: the AWS::S3Tables::TableBucket resource handler in the CloudFormation provisioner — return the table bucket ARN as the physical resource id.

Traces: the failing check in trace_ids.

## Evidence
Expected: the bucket's PhysicalResourceId == 'arn:aws:s3tables:us-west-2:000000000000:bucket/sim2real-cfntb-a-64192d981d7e'.
Actual: AssertionError: the table bucket's physical id must be its ARN 'arn:aws:s3tables:us-west-2:000000000000:bucket/sim2real-cfntb-a-64192d981d7e' — DescribeStackResources reported a different value.

## Steps to Reproduce
1. CreateStack a template with an AWS::S3Tables::TableBucket; wait for CREATE_COMPLETE.
2. DescribeStackResources: the bucket's PhysicalResourceId is not the arn:aws:s3tables:...:bucket/<name> ARN.
