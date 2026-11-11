Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__rds.cfn-rds-db-subnet-group-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__rds.cfn-rds-db-subnet-group-create-update-in-place-replace-retained-then-delete|run#18|AssertionError|assert outputs == {"Arn": group["DBSubnetGroupArn"]}, (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# Fn::GetAtt DBSubnetGroupArn is built with the wrong RDS ARN resource type (db-subnet-group/<name> instead of subgrp:<name>)

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Cause 1 of 1: the ARN CloudFormation reports for a DB subnet group does not match the ARN RDS reports for the same group.

AWS: Smithy documents RDS DBSubnetGroup.DBSubnetGroupArn as 'The Amazon Resource Name (ARN) for the DB subnet group'; RDS's own value for a DB subnet group uses the 'subgrp' resource type with a colon separator — arn:aws:rds:<region>:<account>:subgrp:<name>. Fn::GetAtt <DBSubnetGroup>.DBSubnetGroupArn resolves to exactly the ARN DescribeDBSubnetGroups returns, so the two always agree.

Emulator: the stack output from GetAtt is 'arn:aws:rds:<region>:<account>:db-subnet-group/<name>' while DescribeDBSubnetGroups for the same group returns 'arn:aws:rds:<region>:<account>:subgrp:<name>'. The resource-type token and the separator differ; everything else (region, account, name) matches.

Why: an ARN is an identity. A client that takes the CloudFormation output and uses it in an IAM policy resource, a tag-resource call, a cross-service reference or a comparison against the RDS-reported ARN is using an ARN that names nothing — authorization and lookups silently fail or over/under-match.

Fix: the AWS::RDS::DBSubnetGroup resource handler's GetAtt attribute construction (and any shared RDS ARN builder it uses) — emit the 'subgrp:<name>' form RDS itself reports.

Traces: the failing check in trace_ids.

## Evidence
Expected: GetAtt DBSubnetGroupArn == RDS's DBSubnetGroupArn 'arn:aws:rds:us-west-2:000000000000:subgrp:sim2real-cfnreal-dbsg-a-737a955da650'.
Actual: the stack output was {'Arn': 'arn:aws:rds:us-west-2:000000000000:db-subnet-group/sim2real-cfnreal-dbsg-a-737a955da650'} while DescribeDBSubnetGroups reported 'DBSubnetGroupArn': 'arn:aws:rds:us-west-2:000000000000:subgrp:sim2real-cfnreal-dbsg-a-737a955da650'.

## Steps to Reproduce
1. CreateStack a template with an AWS::RDS::DBSubnetGroup over two subnets and an Output of Fn::GetAtt <group>.DBSubnetGroupArn.
2. DescribeStacks for the output and rds DescribeDBSubnetGroups for the same group; the two ARNs differ in resource type and separator.
