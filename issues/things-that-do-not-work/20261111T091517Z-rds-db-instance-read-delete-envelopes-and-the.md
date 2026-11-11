Issue-Source: trace-sweep-divergence
Issue-Trace: rds-create-db-instance-catalog-triple
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-create-db-instance-catalog-triple rds-db-instance-core-lifecycle rds-describe-instance-case-insensitive rds-describe-db-engine-versions-marker-continues-without-overlap
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBInstances|2xx->2xx|$.DescribeDBInstancesResponse.DescribeDBInstancesResult.DBInstances.DBInstance.Endpoint.Port
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.Endpoint.Port
Issue-Key: rds|DeleteDBInstance|2xx->2xx|$.DeleteDBInstanceResponse.DeleteDBInstanceResult.DBInstance.PendingModifiedValues
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.Marker
Issue-Related: 20261022T162132Z-rds-describedbinstances-omits-modeled-members-and-reports-wrong.md
Issue-Related: 20261030T001542Z-rds-db-subnet-group-and-db-instance-projections.md
Issue-Related: 20261108T233943Z-rds-describedbengineversions-serves-a-synthetic-engine-version-catalog-for.md
Issue-Commit: 34a59b28db3b6549e829977bc135282939da6978

# rds: DB instance read/delete envelopes and the engine-version page serve emulator-authored values (4 causes)

> _Note: this issue was identified against the code base at commit `34a59b28db3b6549e829977bc135282939da6978`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
Four net-new wire causes in the rds DB-instance and engine-version catalog surface. Each is a separate cluster in the replay and must be closed on its own; the fix is not complete until all four re-replay clean.

Cause 1 of 4: DescribeDBInstances publishes the emulator's listener port as DBInstance.Endpoint.Port.
AWS: Endpoint.Port is the port the DB instance's engine listens on — the value supplied as CreateDBInstance.Port, or, when that is omitted, the engine's default port (Smithy: CreateDBInstance input member Port -> IntegerOptional; DBInstance.Endpoint.Port -> Integer). For a MySQL instance created with no Port, AWS reports 3306; the recorded response for the fixture's instance reports 3306.
Emulator: the same member carries a per-process, randomly chosen host listener port (the replay observed 42827 where 3306 was recorded); the number changes on every boot, so it is not a stable value at all.
Why: a client reads Endpoint.Address/Endpoint.Port to build a connection string, and anything that asserts or records the engine port (a generated JDBC URL, a security-group rule, a stored connection profile, a config template) gets a value that is neither the engine default nor the port it requested.
Fix: the DescribeDBInstances handler / the DBInstance endpoint projection in the instance state — the stored instance must keep the requested-or-engine-default port as its reported Endpoint.Port, with any host-side listener port mapped behind it rather than echoed into the envelope.
Traces: rds-create-db-instance-catalog-triple#task#14.

Cause 2 of 4: the DeleteDBInstance response's DBInstance element omits modeled members and reports configuration the request never asked for.
AWS: DeleteDBInstance returns the full DBInstance record of the instance as it enters deleting (Smithy: DeleteDBInstance -> DeleteDBInstanceResult.DBInstance -> DBInstance). The recorded response carries VPCNetworkingEnabled=true, HttpEndpointEnabled=false and an (empty) PendingModifiedValues element, reports StorageType as the gp2 the instance was created with, StorageThroughput 0 for that storage type, an empty CACertificateIdentifier, and Endpoint.Port 3306.
Emulator: VPCNetworkingEnabled, HttpEndpointEnabled and PendingModifiedValues are absent from the envelope entirely; StorageType comes back 'gp3' instead of the created 'gp2'; StorageThroughput comes back 125 instead of 0; CACertificateIdentifier comes back 'rds-ca-rsa2048-g1' instead of empty; Endpoint.Port is again the random listener port (42827 observed).
Why: a client that reads back the delete response to confirm the instance's configuration sees storage settings it never requested (a gp2 instance reported as gp3 with provisioned throughput), and code that branches on the presence of PendingModifiedValues or on the VPC/HTTP-endpoint flags takes the absent-member path on every delete.
Fix: the DeleteDBInstance handler's DBInstance serializer and the instance state it reads — emit the modeled members AWS emits and project the instance's own stored storage/CA/port configuration instead of defaults chosen by the emulator.
Traces: rds-create-db-instance-catalog-triple#task#15.

Cause 3 of 4: the same omissions and substitutions appear on the DescribeDBInstances read path, so they are not delete-specific.
AWS: DescribeDBInstances returns the instance's own configuration (Smithy: DescribeDBInstances -> DBInstanceMessage.DBInstances -> DBInstance). The recorded response carries VPCNetworkingEnabled=true, HttpEndpointEnabled=false, an empty PendingModifiedValues, StorageType gp2, StorageThroughput 0 and Endpoint.Port 3306.
Emulator: VPCNetworkingEnabled, HttpEndpointEnabled and PendingModifiedValues are missing; StorageType is gp3, StorageThroughput is 125, and Endpoint.Port is the random listener port (45599 observed).
Why: every describe-based reconciliation (drift detection, a Terraform/CDK read, a human audit) concludes the instance is configured differently from how it was created, and never sees the pending-modification envelope AWS always includes.
Fix: the DescribeDBInstances handler's DBInstance projection — the same serializer/state gap as cause 2, fixed once for both paths.
Traces: rds-db-instance-core-lifecycle#task#2 (same cluster also covers rds-describe-instance-case-insensitive#task#2).

Cause 4 of 4: DescribeDBEngineVersions ignores the requested page size and never issues a Marker, so the catalog cannot be paged.
AWS: DescribeDBEngineVersions is paginated on Marker/MaxRecords (Smithy: paginated inputToken=Marker, outputToken=Marker, items=DBEngineVersions, pageSize=MaxRecords). A request for a page returns up to MaxRecords DBEngineVersion entries and, when more remain, a Marker that the next call continues from; the recorded first page carries 20 entries plus a Marker.
Emulator: the response carries only 8 DBEngineVersion entries (indices 8..19 are absent) and no Marker at all, so the caller is told the catalog ends after the first short page.
Why: a client that pages the engine-version catalog — to pick an engine version, to validate one, or to enumerate families — sees a truncated catalog and has no continuation token, so the versions beyond the short page are invisible and a 'version not supported' decision is made on incomplete data. This is distinct from the open issue '20261108T233943Z-rds-describedbengineversions-serves-a-synthetic-engine-version-catalog-for.md' (catalog CONTENT per engine); this cause is the page size and the missing Marker.
Fix: the DescribeDBEngineVersions handler's pagination — honour MaxRecords for the slice length and emit the output Marker whenever the filtered catalog has more entries than the page returned.
Traces: rds-describe-db-engine-versions-marker-continues-without-overlap#setup#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — rds-create-db-instance-catalog-triple#task#14, DescribeDBInstances, status 200 vs 200:
  $...DBInstances.DBInstance.Endpoint.Port: value_mismatch (expected '3306', got '42827')

Cause 2 — rds-create-db-instance-catalog-triple#task#15, DeleteDBInstance, status 200 vs 200:
  $...DBInstance.VPCNetworkingEnabled: missing (expected 'true', got None)
  $...DBInstance.HttpEndpointEnabled: missing (expected 'false', got None)
  $...DBInstance.PendingModifiedValues: missing (expected '', got None)
  $...DBInstance.StorageThroughput: value_mismatch (expected '0', got '125')
  $...DBInstance.StorageType: value_mismatch (expected 'gp2', got 'gp3')
  $...DBInstance.CACertificateIdentifier: value_mismatch (expected '', got 'rds-ca-rsa2048-g1')
  $...DBInstance.Endpoint.Port: value_mismatch (expected '3306', got '42827')

Cause 3 — rds-db-instance-core-lifecycle#task#2, DescribeDBInstances, status 200 vs 200 (cluster of 2, also rds-describe-instance-case-insensitive#task#2):
  $...DBInstance.VPCNetworkingEnabled: missing (expected 'true', got None)
  $...DBInstance.HttpEndpointEnabled: missing (expected 'false', got None)
  $...DBInstance.PendingModifiedValues: missing (expected '', got None)
  $...DBInstance.StorageThroughput: value_mismatch (expected '0', got '125')
  $...DBInstance.StorageType: value_mismatch (expected 'gp2', got 'gp3')
  $...DBInstance.Endpoint.Port: value_mismatch (expected '3306', got '45599')

Cause 4 — rds-describe-db-engine-versions-marker-continues-without-overlap#setup#1, DescribeDBEngineVersions, status 200 vs 200:
  $...DBEngineVersions.DBEngineVersion[8] .. [19]: missing (expected a DBEngineVersion element, got None)
  $.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.Marker: diverged (no Marker served)

The observed ports (42827, 45599) are per-run host values and are not themselves requirements; the requirement is that the member reports the instance's engine port (3306 for the fixtures' MySQL instances), not a listener port.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture rds-create-db-instance-catalog-triple step task#14 and read DescribeDBInstancesResult.DBInstances.DBInstance.Endpoint.Port — it is the emulator's listener port, not 3306.
4. Cause 2: replay rds-create-db-instance-catalog-triple step task#15 and diff the DeleteDBInstanceResult.DBInstance element against the recording — VPCNetworkingEnabled, HttpEndpointEnabled and PendingModifiedValues are absent and StorageType/StorageThroughput/CACertificateIdentifier/Endpoint.Port differ.
5. Cause 3: replay rds-db-instance-core-lifecycle step task#2 and diff the DescribeDBInstancesResult DBInstance element — same omissions and substitutions.
6. Cause 4: replay rds-describe-db-engine-versions-marker-continues-without-overlap step setup#1 — only 8 DBEngineVersion entries come back and DescribeDBEngineVersionsResult.Marker is absent, so the recorded second page cannot be fetched.
