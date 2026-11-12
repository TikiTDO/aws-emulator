Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-cluster-and-instance-endpoint-discovery
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-cluster-and-instance-endpoint-discovery
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.AllocatedStorage
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.DbInstancePort
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.Endpoint.Address
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.HttpEndpointEnabled
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.LicenseModel
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.MasterUsername
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.PendingModifiedValues
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.PromotionTier
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.PubliclyAccessible
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.StorageThroughput
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.StorageType
Issue-Key: docdb|CreateDBInstance|2xx->2xx|$.CreateDBInstanceResponse.CreateDBInstanceResult.DBInstance.VPCNetworkingEnabled
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.AllocatedStorage
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.DbInstancePort
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.Endpoint.Address
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.HttpEndpointEnabled
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.LicenseModel
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.MasterUsername
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.PendingModifiedValues
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.PromotionTier
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.PubliclyAccessible
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.StorageThroughput
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.StorageType
Issue-Key: docdb|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.VPCNetworkingEnabled
Issue-Commit: 4f8baca82116a343bf4a764bcd60c9c8f261ed5e

# DocDB DBInstance projection omits modeled instance attributes and misreports storage, port and PubliclyAccessible

> _Note: this issue was identified against the code base at commit `4f8baca82116a343bf4a764bcd60c9c8f261ed5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
Two net-new causes, one shared fault: the docdb DBInstance projection is built from a different attribute set than the one AWS returns for a DocumentDB instance. The same twelve members diverge on the create response and on the subsequent read, so the defect is in the shared DBInstance state/serializer rather than in one handler.

Cause 1 of 2: CreateDBInstance response (fixture docdb-cluster-and-instance-endpoint-discovery, step task#8).
AWS: Smithy com.amazonaws.docdb#AmazonRDSv19 models DBInstance.PubliclyAccessible with the explicit statement "Not supported. Amazon DocumentDB does not currently support public endpoints. The value of PubliclyAccessible is always false"; DBInstance.PromotionTier ("the order in which an Amazon DocumentDB replica is promoted to the primary instance") and DBInstance.PendingModifiedValues are modeled members; DBInstance.Endpoint is the Endpoint structure whose Address must be the address the engine answers on. The remaining diverging members (AllocatedStorage, VPCNetworkingEnabled, HttpEndpointEnabled, LicenseModel, MasterUsername, DbInstancePort, StorageType, StorageThroughput) are not in the docdb Smithy shape but are served by the shared RDS wire API that DocumentDB is fronted by — recorded behavior only: the recording shows a DocumentDB instance answering with AllocatedStorage, VPCNetworkingEnabled true, HttpEndpointEnabled false, LicenseModel 'na', MasterUsername echoed from the CreateDBCluster request's master user, DbInstancePort 0, StorageType 'standard' and StorageThroughput 0 — i.e. DocumentDB reports cluster-managed storage (standard, no provisioned throughput), no instance-level port, and the cluster's master username.
Emulator: omits AllocatedStorage, VPCNetworkingEnabled, HttpEndpointEnabled, LicenseModel, PendingModifiedValues, MasterUsername and PromotionTier from the DBInstance element entirely, and reports PubliclyAccessible true (model says always false), StorageType 'gp3' instead of the cluster-managed 'standard', StorageThroughput 125 instead of 0, and DbInstancePort 27017 instead of 0. It also fills Endpoint.Address with a *.docdb.amazonaws.com host it does not serve (host_not_own) where the recording has no Address on the create response.
Why: a client that reads the created instance back cannot see its promotion tier, master username, license model or pending modifications at all, and is told the instance is publicly accessible and backed by provisioned gp3 storage with instance-level port 27017 — none of which is true of DocumentDB. Tooling that keys off PubliclyAccessible (security posture checks), StorageType/StorageThroughput (cost and capacity logic) or DbInstancePort (connection construction) makes the wrong decision, and code reading MasterUsername or PromotionTier gets a missing field instead of a value.
Fix: the docdb CreateDBInstance handler and the DBInstance state record it builds, plus the DBInstance serializer: it must project the full DocumentDB instance attribute set, pin PubliclyAccessible to false and the storage/port attributes to the cluster-managed values, and carry the master username and promotion tier through from the cluster/request. Endpoint.Address belongs to the shared endpoint construction tracked in the companion endpoint issue.
Traces: docdb-cluster-and-instance-endpoint-discovery#task#8.

Cause 2 of 2: DescribeDBInstances response for the same instance (fixture docdb-cluster-and-instance-endpoint-discovery, step task#9).
AWS: as in cause 1; the read-back projection of DBInstance is the same shape as the create response, and here Endpoint.Address must additionally be a served address (the fixture asserts the host is the server's own, not its text).
Emulator: the identical twelve divergences — the same seven members absent, the same four values wrong (PubliclyAccessible true, StorageType 'gp3', StorageThroughput 125, DbInstancePort 27017) and an Endpoint.Address in the unserved *.docdb.amazonaws.com namespace.
Why: the wrong attribute set is not a create-path artifact; it is what every subsequent describe of the instance reports, so a client polling for instance readiness or inventorying instances never recovers the correct values.
Fix: same component — the shared docdb DBInstance state/serializer that both CreateDBInstance and DescribeDBInstances project through.
Traces: docdb-cluster-and-instance-endpoint-discovery#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 89.2%, 13 mismatched steps of 120).

Cause 1 — docdb-cluster-and-instance-endpoint-discovery#task#8, CreateDBInstance, expected 200 / actual 200:
  $...DBInstance.AllocatedStorage: missing (expected '1', got None)
  $...DBInstance.VPCNetworkingEnabled: missing (expected 'true', got None)
  $...DBInstance.StorageThroughput: value_mismatch (expected '0', got '125')
  $...DBInstance.HttpEndpointEnabled: missing (expected 'false', got None)
  $...DBInstance.PubliclyAccessible: value_mismatch (expected 'false', got 'true')
  $...DBInstance.LicenseModel: missing (expected 'na', got None)
  $...DBInstance.PendingModifiedValues: missing (expected '', got None)
  $...DBInstance.MasterUsername: missing (expected 'probeadmin', got None)
  $...DBInstance.PromotionTier: missing (expected '1', got None)
  $...DBInstance.DbInstancePort: value_mismatch (expected '0', got '27017')
  $...DBInstance.StorageType: value_mismatch (expected 'standard', got 'gp3')
  $...DBInstance.Endpoint.Address: host_not_own (expected None, got 'sim2real-probe-docdbep-i-2634d42f.000000000002.us-west-2.docdb.amazonaws.com')

Cause 2 — docdb-cluster-and-instance-endpoint-discovery#task#9, DescribeDBInstances, expected 200 / actual 200: the same twelve keys under $.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance, with the same expected/actual pairs; Endpoint.Address expected '<shaped-not-asserted>' (a served host), got 'sim2real-probe-docdbep-i-2634d42f.000000000002.us-west-2.docdb.amazonaws.com'.

Expected (AWS): a DocumentDB DBInstance element carrying AllocatedStorage, VPCNetworkingEnabled, HttpEndpointEnabled, LicenseModel 'na', PendingModifiedValues, the cluster's MasterUsername, PromotionTier, PubliclyAccessible false, StorageType 'standard', StorageThroughput 0 and DbInstancePort 0.
Actual (emulator): seven of those members absent and four carrying RDS-style provisioned-storage/public-endpoint values.
Value notes: 'probeadmin' is the master username taken from the request that created the cluster, '1' the promotion tier the instance was created with, and the instance identifier/account/region inside the endpoint host are per-run values — none of those literals is the requirement. The requirements are the presence of each member, PubliclyAccessible being false (the model states it is always false for DocumentDB), the cluster-managed storage values ('standard', throughput 0), DbInstancePort 0, and an Endpoint.Address the server actually serves.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. ec2 CreateVpc + CreateSubnet in two AZs; docdb CreateDBSubnetGroup over them; ec2 CreateSecurityGroup.
3. docdb CreateDBCluster(DBClusterIdentifier=<unique>, Engine='docdb', MasterUsername=<user>, MasterUserPassword=<pw>, DBSubnetGroupName=<group>, VpcSecurityGroupIds=[<sg>]).
4. docdb CreateDBInstance(DBInstanceIdentifier=<unique>, DBInstanceClass='db.t3.medium', Engine='docdb', DBClusterIdentifier=<cluster>, PromotionTier=1) — fixture docdb-cluster-and-instance-endpoint-discovery step task#8. Inspect the DBInstance element: observed no AllocatedStorage, VPCNetworkingEnabled, HttpEndpointEnabled, LicenseModel, PendingModifiedValues, MasterUsername or PromotionTier, and PubliclyAccessible 'true', StorageType 'gp3', StorageThroughput '125', DbInstancePort '27017'.
5. docdb DescribeDBInstances(DBInstanceIdentifier=<instance>) — step task#9. Observed the identical twelve divergences, so the read path reports the same wrong attribute set.
6. Compare against AWS: PubliclyAccessible must be false for DocumentDB, storage must read as cluster-managed ('standard', throughput 0), DbInstancePort 0, and the omitted members must be present with the instance's/cluster's values.
