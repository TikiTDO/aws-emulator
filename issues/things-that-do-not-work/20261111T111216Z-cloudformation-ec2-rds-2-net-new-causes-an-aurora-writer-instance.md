Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__rds.cfn-rds-db-cluster-in-place-tag-update-retained-replacement-keeps-cluster-then-not-found cloudformation__ec2__rds.cfn-rds-db-instance-in-place-tag-update-retained-replacement-keeps-rows-then-not-found
Issue-Key: cloudformation__ec2__rds.cfn-rds-db-cluster-in-place-tag-update-retained-replacement-keeps-cluster-then-not-found|_expect#18|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ec2__rds.cfn-rds-db-instance-in-place-tag-update-retained-replacement-keeps-rows-then-not-found|_expect#27|AssertionError|raise AssertionError(
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# 2 net-new causes: an Aurora writer instance cannot find the cluster the same stack created, and AWS::RDS::DBInstance in-place updates are refused

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two independent root causes in the RDS CloudFormation resource handlers.

Cause 1 of 2: an AWS::RDS::DBInstance declared as a member of an AWS::RDS::DBCluster in the same stack fails to create because the cluster is not found.
AWS: a template with an AWS::RDS::DBCluster (aurora-postgresql) plus an AWS::RDS::DBInstance whose DBClusterIdentifier Refs that cluster provisions the writer through CreateDBInstance; RDS resolves the cluster identifier the stack just created and the stack reaches CREATE_COMPLETE.
Emulator: the writer fails at create. Stack 'ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to create: [Writer]. ' and resource reason 'Resource handler returned message: "The source cluster could not be found or cannot be accessed: <cluster-identifier>" (RequestToken: <token>, HandlerErrorCode: NotFound)'. The fixed text is 'The source cluster could not be found or cannot be accessed: ' and the variable part is the DBClusterIdentifier taken from the request — but the cluster does exist, so the refusal is wrong.
Why: no Aurora cluster with an instance can be provisioned through CloudFormation at all; every such stack rolls back, and nothing that needs a reachable database (connections, tag updates, retained replacement) can run.
Fix: the AWS::RDS::DBInstance resource handler's cluster-membership resolution (and RDS's DescribeDBClusters lookup it uses) — it must find a cluster created earlier in the same stack.

Cause 2 of 2: in-place update of AWS::RDS::DBInstance is refused with an emulator-authored reason.
AWS: changing mutable DBInstance properties (tags, allocated storage, parameter group) is an in-place update applied through ModifyDBInstance; the DBInstanceIdentifier is unchanged, the resource goes UPDATE_IN_PROGRESS -> UPDATE_COMPLETE and the stack reaches UPDATE_COMPLETE.
Emulator: the resource is UPDATE_FAILED and the stack ends 'UPDATE_ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to update: [Database]. ' and reason 'NotImplemented: this emulator does not change the properties of an existing AWS::RDS::DBInstance in place; the resource was left as it is.'
Why: a client cannot retag or resize a CloudFormation-managed database; every such update rolls back.
Fix: the AWS::RDS::DBInstance handler's update path in the CloudFormation provisioner.

## Evidence
Cause 1: AssertionError: stack ended 'ROLLBACK_COMPLETE' ('The following resource(s) failed to create: [Writer]. '), expected 'CREATE_COMPLETE'; failed events: [('Writer', 'CREATE_FAILED', 'Resource handler returned message: "The source cluster could not be found or cannot be accessed: sim2real-cfnaur-a-8aa49fec7729" (RequestToken: ..., HandlerErrorCode: NotFound)')]
Cause 2: AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Database]. '), expected 'UPDATE_COMPLETE'; failed events: [('Database', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::RDS::DBInstance in place; the resource was left as it is.')]

## Steps to Reproduce
Cause 1: CreateStack a template with an AWS::RDS::DBCluster plus an AWS::RDS::DBInstance whose DBClusterIdentifier Refs the cluster; the Writer resource ends CREATE_FAILED with the NotFound reason.
Cause 2: CreateStack an AWS::RDS::DBInstance, then UpdateStack changing only its tags; the resource ends UPDATE_FAILED with the NotImplemented reason.
