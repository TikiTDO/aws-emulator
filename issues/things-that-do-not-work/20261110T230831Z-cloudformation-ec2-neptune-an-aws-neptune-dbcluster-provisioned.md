Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__neptune.deploy-neptune-cluster-invalid-engine-version-rejected
Issue-Key: cloudformation__ec2__neptune|DescribeDBClusters|DBClusterNotFoundFault
Issue-Commit: 76d01eabdbc8750ace4af5212f225bb5f29fbdd8

# cloudformation__ec2__neptune: an AWS::Neptune::DBCluster provisioned by a CREATE_COMPLETE stack does not exist in Neptune — DescribeDBClusters on the Ref'd identifier answers DBClusterNotFoundFault

> _Note: this issue was identified against the code base at commit `76d01eabdbc8750ace4af5212f225bb5f29fbdd8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation AWS::Neptune::DBCluster resource provisioner never reaches the Neptune control plane. The stack is reported CREATE_COMPLETE and the resource's Ref resolves to the declared DBClusterIdentifier, but no DB cluster exists in Neptune's state, so the very next Neptune read of that identifier is refused as not found. Failing check: cloudformation__ec2__neptune.deploy-neptune-cluster-invalid-engine-version-rejected.

AWS: CloudFormation's AWS::Neptune::DBCluster handler calls Neptune CreateDBCluster (Smithy, com-amazonaws-neptune-amazonrdsv19: CreateDBCluster takes required DBClusterIdentifier and Engine plus DBSubnetGroupName, EngineVersion, DeletionProtection, Tags, ... and returns CreateDBClusterResult{DBCluster}). On success the cluster is a real Neptune resource and the resource's Ref is the DBClusterIdentifier. DescribeDBClusters(DBClusterIdentifier=<that identifier>) then answers HTTP 200 with exactly one DBCluster whose DBClusterIdentifier equals the declared id, whose Status settles to "available", and whose EngineVersion equals the EngineVersion the template declared (the check reads it back as the version DescribeDBEngineVersions(Engine="neptune", DefaultOnly=true) reported as the engine default). DescribeDBClusters models exactly one error, DBClusterNotFoundFault (HTTP 404), and AWS raises it only for an identifier that does not exist — e.g. after the stack is deleted, or for the cluster of a stack that rolled back; the message's fixed text is "DBCluster " ... " not found." around the requested identifier, which is a value taken from the request.

Emulator: the positive-control stack (Vpc + SubnetA + SubnetB + AWS::Neptune::DBSubnetGroup + AWS::Neptune::DBCluster, EngineVersion = the engine default) reaches CREATE_COMPLETE and DescribeStacks reports Outputs.ClusterRef equal to the declared DBClusterIdentifier — both of those assertions pass. The next call, neptune.DescribeDBClusters(DBClusterIdentifier=<that same Ref value>), is refused: "botocore.errorfactory.DBClusterNotFoundFault: An error occurred (DBClusterNotFoundFault) when calling the DescribeDBClusters operation: DBCluster <declared identifier> not found." So the provisioner marks the resource created and publishes a Ref for it without writing any Neptune state. This is the DBCluster analogue of the AWS::Neptune::DBSubnetGroup no-op already reported as Cause 1 of open issue 20261109T134315Z-cloudformation-ec2-neptune-3-net-new-causes-aws.md; that issue's causes cover DBSubnetGroup provisioning, key-only stack Outputs and the DescribeDBEngineVersions(DefaultOnly) refusal, and none of them covers the DBCluster resource or this DescribeDBClusters refusal (its recorded keys name DescribeDBSubnetGroups and DescribeDBEngineVersions, not DescribeDBClusters).

Why: a client that provisions Neptune through IaC gets a green stack and an unusable cluster — every later Neptune call that names the cluster (DescribeDBClusters, ModifyDBCluster, CreateDBInstance into it, tagging, snapshotting) fails as if the cluster had never been declared, and the data-plane endpoint the stack is supposed to hand downstream does not exist. Because the create never reaches Neptune, Neptune's own create-time validation never runs either: the rejection arm of this scenario — a template whose EngineVersion is not a real Neptune version, which AWS refuses so the resource goes CREATE_FAILED with the registry-handler reason 'Resource handler returned message: "Cannot find version <version> for neptune (Service: Neptune, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)' and the stack ends ROLLBACK_COMPLETE — is unreachable behind this cause (the fixed text is "Cannot find version " ... " for neptune (Service: Neptune, Status Code: 400, ...)"; the version is a value from the request, the request id and attempt count are per-run).

Fix: the cloudformation service's AWS::Neptune::DBCluster resource provisioner — its create/update/delete path must dispatch to the Neptune CreateDBCluster / ModifyDBCluster / DeleteDBCluster handlers and write Neptune state (so DescribeDBClusters finds the cluster with the declared identifier, engine version and subnet group, Status settling to available), propagate the resource Tags plus the aws:cloudformation:stack-name / logical-id / stack-id tags, resolve Ref to the DBClusterIdentifier from the created resource, and translate a Neptune refusal into a CREATE_FAILED resource carrying the handler-error envelope with HandlerErrorCode InvalidRequest so the stack rolls back and leaves no cluster behind.

Traces: cloudformation__ec2__neptune.deploy-neptune-cluster-invalid-engine-version-rejected (errors on the first neptune.describe_db_clusters after the control stack reports CREATE_COMPLETE and ClusterRef == the declared identifier).

## Evidence
Check cloudformation__ec2__neptune.deploy-neptune-cluster-invalid-engine-version-rejected

Sequence observed in the replay:
1. neptune.describe_db_engine_versions(Engine="neptune", DefaultOnly=True) -> succeeded, yielding the default engine version.
2. cfn.create_stack(<Vpc + SubnetA + SubnetB + AWS::Neptune::DBSubnetGroup + AWS::Neptune::DBCluster with EngineVersion = that default>) -> stack reached CREATE_COMPLETE (assertion passed).
3. cfn.describe_stacks(...)["Stacks"][0]["Outputs"] -> ClusterRef == the declared DBClusterIdentifier (assertion passed).
4. neptune.describe_db_clusters(DBClusterIdentifier=<that identifier>)

Expected (AWS): HTTP 200 with DBClusters = [ { DBClusterIdentifier: <declared identifier>, Status: "available", EngineVersion: <the declared/default engine version>, ... } ].
Actual (emulator): botocore.errorfactory.DBClusterNotFoundFault: An error occurred (DBClusterNotFoundFault) when calling the DescribeDBClusters operation: DBCluster <declared identifier> not found.

Message anatomy: fixed text "DBCluster " + <the requested DBClusterIdentifier, a value from the request> + " not found." — correct wording for a genuinely absent cluster, but AWS does not reach this path for a cluster a CREATE_COMPLETE stack just provisioned; the 404 itself is the divergence.

Smithy (com-amazonaws-neptune-amazonrdsv19): DescribeDBClusters(DBClusterIdentifier, Filters, MaxRecords, Marker) -> DBClusterMessage{Marker, DBClusters}, errors: DBClusterNotFoundFault (httpError 404). CreateDBCluster(DBClusterIdentifier required, Engine required, DBSubnetGroupName, EngineVersion, DeletionProtection, Tags, ...) -> CreateDBClusterResult{DBCluster}.

Consequence inside the same check: because the control cluster is absent, the rejection arm (bad EngineVersion "99.99.99" -> CREATE_FAILED resource with HandlerErrorCode InvalidRequest and message "Cannot find version 99.99.99 for neptune (Service: Neptune, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)", stack ROLLBACK_COMPLETE, then DescribeDBClusters on the bad id answering DBClusterNotFoundFault) is never exercised.

## Steps to Reproduce
1. Boot the emulator and point botocore at it; ec2 = client("ec2"), nep = client("neptune"), cfn = client("cloudformation").
2. good_version = nep.describe_db_engine_versions(Engine="neptune", DefaultOnly=True)["DBEngineVersions"][0]["EngineVersion"].
3. cfn.create_stack(StackName="<stack>", TemplateBody=<template below>, Parameters=[AzA/AzB = two AZs that offer Neptune db.t3.medium, ClusterId="<cluster-id>", SubnetGroupName="<sng>", EngineVersion=good_version]) where the template declares:
   - Vpc: AWS::EC2::VPC, CidrBlock 10.62.0.0/16
   - SubnetA/SubnetB: AWS::EC2::Subnet, 10.62.1.0/24 in AzA and 10.62.2.0/24 in AzB
   - SubnetGroup: AWS::Neptune::DBSubnetGroup {DBSubnetGroupName: !Ref SubnetGroupName, DBSubnetGroupDescription: ..., SubnetIds: [!Ref SubnetA, !Ref SubnetB]}
   - Cluster: AWS::Neptune::DBCluster {DBClusterIdentifier: !Ref ClusterId, DBSubnetGroupName: !Ref SubnetGroup, DeletionProtection: false, EngineVersion: !Ref EngineVersion}
   - Outputs: ClusterRef: {Value: !Ref Cluster}
4. Poll cfn.describe_stacks(StackName=<stack id>) until terminal -> observe CREATE_COMPLETE, and Outputs ClusterRef == "<cluster-id>".
5. nep.describe_db_clusters(DBClusterIdentifier="<cluster-id>") -> observe (DBClusterNotFoundFault) "DBCluster <cluster-id> not found." instead of HTTP 200 with the one available cluster at engine version good_version.
6. Follow-on (blocked today): repeat step 3 with EngineVersion="99.99.99" -> AWS accepts the stack, the Cluster resource goes CREATE_FAILED with the handler reason 'Resource handler returned message: "Cannot find version 99.99.99 for neptune (Service: Neptune, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)' and the stack ends ROLLBACK_COMPLETE.
