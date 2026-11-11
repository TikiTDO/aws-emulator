Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.create-cluster-available-fields redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.create-cluster-available-fields|run#27|AssertionError|assert cluster["VpcId"] == vpc_id, f"expected the cluster in VPC {vpc_id}, got {cluster.get('VpcId')}"
Issue-Key: redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected|run#26|AssertionError|assert gone == want_gone, (
Issue-Commit: 74fb40980dbbe3b3bd7d1c6ccbad3ad407443092

# Redshift cluster networking identity is synthesized, not resolved: ClusterSubnetGroup/Cluster VpcId is a hash of the subnet-group name instead of the VPC of the named subnets, and the DescribeClusterParameterGroups not-found message uses the wrong wording (2 root causes)

> _Note: this issue was identified against the code base at commit `74fb40980dbbe3b3bd7d1c6ccbad3ad407443092`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateClusterSubnetGroup

## Description
Two net-new root causes in the Redshift control plane, both grounded in failing behavioral checks. Neither is covered by an open issue; the closest open items (20261024T024248Z-redshift-cluster-scoped-credential-and-notification-operations-answer.md, 20261024T024248Z-redshift-createcluster-skips-request-validation-and-publishes-an.md) concern NotImplemented operations and CreateCluster request validation, not the networking identity or these messages, and the fixed 20261023T081912Z-redshift-cluster-parameter-group-state-and-metadata-diverge.md / 20261023T081912Z-redshift-control-plane-skips-existence-preconditions-createcluster-describeclusters.md made DescribeClusterParameterGroups refuse an unknown name at all but left its message text wrong. Both calls below SUCCEED or refuse with the right code; the divergence is in the value/wording, so this is not a reopen of a fixed refusal.

Cause 1 of 2: ClusterSubnetGroup.VpcId (and therefore Cluster.VpcId) is invented from the group name instead of resolved from the subnets the request named.
AWS: Smithy: CreateClusterSubnetGroup.SubnetIds is a required SubnetIdentifierList, and the operation models InvalidSubnet and ClusterSubnetGroupNotFoundFault. ClusterSubnetGroup.VpcId is documented as "The VPC ID of the cluster subnet group" and Cluster.VpcId as "The identifier of the VPC the cluster is in, if the cluster is in a VPC." In AWS the VPC is a DERIVED fact: every subnet named in SubnetIds belongs to exactly one VPC, Redshift resolves those subnet ids against EC2, and the group — and any cluster launched into it — report THAT VPC's id, the same id EC2's CreateVpc/DescribeSubnets report. AWS additionally refuses SubnetIds that do not resolve, or that span more than one VPC, with InvalidSubnet.
Emulator: CreateClusterSubnetGroup never looks at the SubnetIds at all beyond counting them; it stores vpc_id = derived_vpc_id(<normalized group name>), i.e. a value hashed from the group's own name (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/create_cluster_subnet_group.rs:278). CreateCluster then reads the cluster's VPC off that stored value, with a second synthesis of the same kind as its fallback (.../src/ops/create_cluster.rs:638-648). In the replay the check created a VPC through EC2, created one subnet in it, created a cluster subnet group over that subnet and a cluster in that group; DescribeClusters reported a VpcId that is not the EC2 VPC the subnet lives in (run showed EC2's vpc-0000000000000001e versus the cluster's hashed vpc-6a7dd2c9cf9c140b3 — those literal ids are per-run values, the requirement is that the two are the SAME id). Note the shape difference as a symptom: the emulated Redshift id is a 17-hex-digit hash while EC2 mints sequential ids, so the two services can never agree.
Why: a client cannot join a Redshift cluster to its own network. Anything that reads Cluster.VpcId or ClusterSubnetGroup.VpcId to place security groups, VPC endpoints, route tables, peering or an IaC cross-reference gets a VPC id that does not exist in EC2; DescribeVpcs on it returns InvalidVpcID.NotFound. CloudFormation/Terraform resources that wire a cluster's VpcId into a security-group or endpoint resource break, and no network-scoped assertion about a cluster can ever hold. Because the id is a pure function of the group name, two different VPCs with same-named groups also collide.
Fix: CreateClusterSubnetGroup's handler and the Redshift subnet-group state — resolve the request's SubnetIds against the EC2 subnet state to obtain their VPC (and the subnets' Availability Zones), store that real VPC id on the ClusterSubnetGroup record, and refuse a SubnetIds list that does not resolve or spans more than one VPC with the modeled InvalidSubnet. CreateCluster's VPC derivation must then read only the group's stored VPC and must not fall back to a name-hash; the same applies to the account's default subnet group materialized by ensure_default_subnet_group.
Traces: redshift.create-cluster-available-fields — assertion `cluster["VpcId"] == vpc_id` at check line 303/1537. Source: services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/create_cluster_subnet_group.rs:275-279, services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/create_cluster.rs:638-648.

Cause 2 of 2: DescribeClusterParameterGroups ClusterParameterGroupNotFound message uses an invented wording instead of AWS's template.
AWS: Smithy: DescribeClusterParameterGroups models ClusterParameterGroupNotFoundFault with awsQueryError code `ClusterParameterGroupNotFound`, HTTP 404 ("The parameter group name does not refer to an existing parameter group"). Recorded behavior: for a name that names no group, AWS answers 404 / ClusterParameterGroupNotFound with the message `ClusterParameterGroup <name> not found.` — fixed text `ClusterParameterGroup ` + the requested ParameterGroupName taken verbatim from the request + ` not found.`; the only variable part is that name. (This is a different template from CreateCluster's own `ClusterParameterGroup not found: <name>`, which the emulator already gets right and which this check also asserts and passes — so the two operations must keep their two distinct wordings.)
Emulator: the status (404) and code (ClusterParameterGroupNotFound) are right, but the message is `Cluster parameter group not found: <name>` — spaced-out prose plus a colon-suffixed name, matching neither AWS template. The replay recorded `('ClusterParameterGroupNotFound', 'Cluster parameter group not found: <group>')` where `('ClusterParameterGroupNotFound', 'ClusterParameterGroup <group> not found.')` was required.
Why: callers that surface or match on the service's error message — log scrapers, IaC providers and test suites that assert the documented text, and users reading the failure — see text AWS never emits, so a not-found on a parameter group is not recognizable as the same condition AWS reports. It also makes the service self-inconsistent: the same fault reads two different ways depending on which operation raised it, and neither read matches AWS's pairing.
Fix: the DescribeClusterParameterGroups handler's not-found branch — emit the operation's own documented template with the requested name interpolated, leaving the status and awsQueryError code as they are.
Traces: redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected — assertion `gone == want_gone` at check line 79/1655. Source: services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/describe_cluster_parameter_groups.rs:199-212 (message built at line 204-206).

## Evidence
Cause 1 — redshift.create-cluster-available-fields (mismatch):
  File "check_redshift_create_cluster_available_fields.py", line 303, in run
    assert cluster["VpcId"] == vpc_id, f"expected the cluster in VPC {vpc_id}, got {cluster.get('VpcId')}"
  AssertionError: expected the cluster in VPC vpc-0000000000000001e, got vpc-6a7dd2c9cf9c140b3
Expected: DescribeClusters.Clusters[0].VpcId == the VPC id EC2 returned from CreateVpc, i.e. the VPC that owns the subnet named in the cluster subnet group's SubnetIds.
Actual: a VPC id hashed from the subnet group's name (create_cluster_subnet_group.rs:278 `vpc_id: Some(st::derived_vpc_id(&name))`; create_cluster.rs:638-648 reads it, falling back to another derived_vpc_id). The id does not exist in the EC2 state. (The two literal ids are per-run values; the requirement is their equality.)

Cause 2 — redshift.create-cluster-invalid-shape-and-in-use-parameter-group-delete-rejected (mismatch):
  File "check_redshift_create_cluster_invalid_shape_and_in_use_parameter_group_delete_rejected.py", line 79, in run
    assert gone == want_gone
  AssertionError: once the cluster is gone the group deletes, then describes as not found: expected ('ClusterParameterGroupNotFound', 'ClusterParameterGroup sim2real-rshealth-6b8c0d0b16f0-pg not found.'), got ('ClusterParameterGroupNotFound', 'Cluster parameter group not found: sim2real-rshealth-6b8c0d0b16f0-pg')
Expected: HTTP 404, Code ClusterParameterGroupNotFound, Message = "ClusterParameterGroup " + <requested ParameterGroupName> + " not found."
Actual: HTTP 404, Code ClusterParameterGroupNotFound, Message = "Cluster parameter group not found: " + <requested ParameterGroupName>.
Fixed text vs variables: the only variable in AWS's message is the ParameterGroupName copied from the request; everything else is fixed text. The group name in the transcript above is a per-run minted name, not part of the template. The same check asserts CreateCluster's distinct template "ClusterParameterGroup not found: <name>" and that assertion passes, so the divergence is specific to DescribeClusterParameterGroups.

## Steps to Reproduce
Cause 1 (VpcId not derived from the named subnets):
1. ec2.create_vpc(CidrBlock="10.0.0.0/16") -> note VpcId V.
2. ec2.create_subnet(VpcId=V, CidrBlock="10.0.0.0/24", AvailabilityZone=<an available AZ>) -> note SubnetId S.
3. redshift.create_cluster_subnet_group(ClusterSubnetGroupName=G, Description="...", SubnetIds=[S]).
4. redshift.describe_cluster_subnet_groups(ClusterSubnetGroupName=G) -> ClusterSubnetGroups[0].VpcId is NOT V (it is a hash of G).
5. redshift.create_cluster(ClusterIdentifier=C, NodeType=<orderable single-node type>, ClusterType="single-node", MasterUsername="adminuser", MasterUserPassword="TestPassw0rd123", DBName="devdb", Encrypted=True, PubliclyAccessible=False, AutomatedSnapshotRetentionPeriod=1, ClusterSubnetGroupName=G); wait for "available".
6. redshift.describe_clusters(ClusterIdentifier=C) -> Clusters[0].VpcId != V. ec2.describe_vpcs(VpcIds=[that id]) does not resolve it.

Cause 2 (wrong not-found message on DescribeClusterParameterGroups):
1. redshift.describe_cluster_parameter_groups(ParameterGroupName="no-such-group-<unique>").
2. Observe the ClientError: Code is ClusterParameterGroupNotFound (correct) but Message is "Cluster parameter group not found: no-such-group-<unique>" instead of "ClusterParameterGroup no-such-group-<unique> not found.".
3. For contrast, redshift.create_cluster(..., ClusterParameterGroupName="no-such-group-<unique>") returns the correct CreateCluster wording "ClusterParameterGroup not found: no-such-group-<unique>", showing only the describe path diverges.
