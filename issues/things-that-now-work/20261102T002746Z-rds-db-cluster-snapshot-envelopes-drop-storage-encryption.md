Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-cluster-lifecycle-with-snapshot
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-cluster-lifecycle-with-snapshot
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.CustomDataGroupPresent
Issue-Key: rds|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.EngineVersion
Issue-Key: rds|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.LicenseModel
Issue-Key: rds|CreateDBClusterSnapshot|2xx->2xx|$.CreateDBClusterSnapshotResponse.CreateDBClusterSnapshotResult.DBClusterSnapshot.StorageEncryptionType
Issue-Key: rds|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.EngineVersion
Issue-Key: rds|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.FullSnapshotSizeInBytes
Issue-Key: rds|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.LicenseModel
Issue-Key: rds|DescribeDBClusterSnapshots|2xx->2xx|$.DescribeDBClusterSnapshotsResponse.DescribeDBClusterSnapshotsResult.DBClusterSnapshots.DBClusterSnapshot.StorageEncryptionType
Issue-Key: rds|DeleteDBClusterSnapshot|2xx->2xx|$.DeleteDBClusterSnapshotResponse.DeleteDBClusterSnapshotResult.DBClusterSnapshot.CustomDataGroupPresent
Issue-Key: rds|DeleteDBClusterSnapshot|2xx->2xx|$.DeleteDBClusterSnapshotResponse.DeleteDBClusterSnapshotResult.DBClusterSnapshot.EngineVersion
Issue-Key: rds|DeleteDBClusterSnapshot|2xx->2xx|$.DeleteDBClusterSnapshotResponse.DeleteDBClusterSnapshotResult.DBClusterSnapshot.LicenseModel
Issue-Key: rds|DeleteDBClusterSnapshot|2xx->2xx|$.DeleteDBClusterSnapshotResponse.DeleteDBClusterSnapshotResult.DBClusterSnapshot.StorageEncryptionType
Issue-Commit: 67befbd38f0c0c0cf22099d8dd1549ab186d2816

# rds DB cluster snapshot envelopes drop storage-encryption and license attributes and report a stale engine version

> _Note: this issue was identified against the code base at commit `67befbd38f0c0c0cf22099d8dd1549ab186d2816`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBClusterSnapshot

## Description
Every operation that returns a `DBClusterSnapshot` element builds it from a snapshot record that never carries the source cluster's engine version, never derives the license model from the engine, and never emits the data-at-rest encryption or group attributes AWS includes. All three causes are the same projection (`DbClusterSnapshotRecord::from_cluster`, named by the CreateDBClusterSnapshot handler doc) surfacing on three operations, and each must be closed and re-replayed separately. 3 causes.

Cause 1 of 3 — CreateDBClusterSnapshot (fixture rds-db-cluster-lifecycle-with-snapshot, step task#4).
AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json) models DBClusterSnapshot.StorageEncryptionType as the StorageEncryptionType enum, documented "sse-rds - The DB cluster snapshot is encrypted using an Amazon Web Services owned KMS key"; for a snapshot of an Aurora cluster protected by the AWS-owned key AWS returns StorageEncryptionType=sse-rds. DBClusterSnapshot.LicenseModel is "The license model information for this DB cluster snapshot", and for an Aurora MySQL cluster AWS returns the Aurora engine's license model `aurora-mysql`, not a MySQL community license string. DBClusterSnapshot.EngineVersion is "The version of the database engine for this DB cluster snapshot" and must be the engine version of the cluster the snapshot was taken from (the recorded cluster is Aurora MySQL 8.0.mysql_aurora.3.10.3). The recorded response also carries CustomDataGroupPresent=false, a member this checkout's model does not declare - recorded behavior only.
Emulator: answers 200 but omits StorageEncryptionType and CustomDataGroupPresent entirely, returns LicenseModel=general-public-license, and returns EngineVersion=8.0.mysql_aurora.3.08.0 (the crate's stale Aurora MySQL default, not the stored cluster's version).
Why: a client that reads a snapshot back to decide whether the data at rest is encrypted sees no StorageEncryptionType at all and cannot distinguish none/sse-rds/sse-kms; a client that reads LicenseModel or EngineVersion to pick a restore target gets a non-Aurora license string and an engine version that does not exist on the cluster, so a restore built from the snapshot's own attributes is rejected.
Fix: the snapshot projection in the service's state - `DbClusterSnapshotRecord::from_cluster` - must copy the source cluster's EngineVersion, derive LicenseModel from the cluster's engine, and populate StorageEncryptionType (and the recorded CustomDataGroupPresent flag) rather than defaulting them. The stale Aurora MySQL default engine version is already filed for the DBCluster envelopes in open issue 20261029T214632Z-rds-dbcluster-response-envelope-omits-modeled-members-and.md, which does NOT cover the three snapshot operations below.
Traces: rds-db-cluster-lifecycle-with-snapshot#task#4.

Cause 2 of 3 — DescribeDBClusterSnapshots (fixture rds-db-cluster-lifecycle-with-snapshot, step task#5).
AWS: same modeled members as Cause 1 (StorageEncryptionType enum, LicenseModel, EngineVersion) read back on the stored snapshot; the recording additionally carries FullSnapshotSizeInBytes=0, which this checkout's model does not declare - recorded behavior only.
Emulator: answers 200 with EngineVersion=8.0.mysql_aurora.3.08.0 instead of the cluster's version, LicenseModel=general-public-license instead of aurora-mysql, and StorageEncryptionType and FullSnapshotSizeInBytes absent.
Why: the read-back path is what a waiter polls after CreateDBClusterSnapshot, so the wrong attributes persist for the whole lifetime of the snapshot, not just on the create response.
Fix: the DescribeDBClusterSnapshots serializer must project the same corrected record as Cause 1; it must not recompute engine or license defaults of its own.
Traces: rds-db-cluster-lifecycle-with-snapshot#task#5.

Cause 3 of 3 — DeleteDBClusterSnapshot (fixture rds-db-cluster-lifecycle-with-snapshot, step teardown#8).
AWS: DeleteDBClusterSnapshot returns the deleted snapshot's `DBClusterSnapshot` element with the attributes it had, so StorageEncryptionType=sse-rds, LicenseModel=aurora-mysql, EngineVersion = the source cluster's engine version, plus the recorded CustomDataGroupPresent=false (recorded behavior only).
Emulator: answers 200 with EngineVersion=8.0.mysql_aurora.3.08.0, LicenseModel=general-public-license, and StorageEncryptionType and CustomDataGroupPresent missing.
Why: the delete response is the last record a client can capture for audit; it reports a license model and engine version the cluster never had.
Fix: the DeleteDBClusterSnapshot handler must serialize the stored record (corrected per Cause 1) on the way out instead of re-deriving defaults.
Traces: rds-db-cluster-lifecycle-with-snapshot#teardown#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200).

Cause 1 — rds-db-cluster-lifecycle-with-snapshot#task#4, CreateDBClusterSnapshot, expected 200 / actual 200:
  $...DBClusterSnapshot.EngineVersion: expected '8.0.mysql_aurora.3.10.3', got '8.0.mysql_aurora.3.08.0'
  $...DBClusterSnapshot.StorageEncryptionType: expected 'sse-rds', got None (member absent)
  $...DBClusterSnapshot.CustomDataGroupPresent: expected 'false', got None (member absent)
  $...DBClusterSnapshot.LicenseModel: expected 'aurora-mysql', got 'general-public-license'

Cause 2 — rds-db-cluster-lifecycle-with-snapshot#task#5, DescribeDBClusterSnapshots, expected 200 / actual 200:
  $...DBClusterSnapshots.DBClusterSnapshot.EngineVersion: expected '8.0.mysql_aurora.3.10.3', got '8.0.mysql_aurora.3.08.0'
  $...DBClusterSnapshots.DBClusterSnapshot.FullSnapshotSizeInBytes: expected '0', got None
  $...DBClusterSnapshots.DBClusterSnapshot.StorageEncryptionType: expected 'sse-rds', got None
  $...DBClusterSnapshots.DBClusterSnapshot.LicenseModel: expected 'aurora-mysql', got 'general-public-license'

Cause 3 — rds-db-cluster-lifecycle-with-snapshot#teardown#8, DeleteDBClusterSnapshot, expected 200 / actual 200:
  $...DBClusterSnapshot.EngineVersion: expected '8.0.mysql_aurora.3.10.3', got '8.0.mysql_aurora.3.08.0'
  $...DBClusterSnapshot.StorageEncryptionType: expected 'sse-rds', got None
  $...DBClusterSnapshot.CustomDataGroupPresent: expected 'false', got None
  $...DBClusterSnapshot.LicenseModel: expected 'aurora-mysql', got 'general-public-license'

Model citation: Smithy shape com.amazonaws.rds#DBClusterSnapshot — StorageEncryptionType (enum StorageEncryptionType: none | sse-rds | sse-kms), LicenseModel (String), EngineVersion (String). FullSnapshotSizeInBytes and CustomDataGroupPresent are not declared in this checkout's model (recorded behavior only).

The values '8.0.mysql_aurora.3.10.3' and the snapshot/cluster identifiers above are this recording's instance of the rule; the requirement is that EngineVersion equals the source cluster's engine version and LicenseModel equals the engine's license model, whatever those are in the request/state.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay fixture rds-db-cluster-lifecycle-with-snapshot (trace set simparity-wire-aws-v1).
3. Cause 1: at step task#4 the CreateDBClusterSnapshot response for the Aurora MySQL cluster created in task#1 omits StorageEncryptionType/CustomDataGroupPresent and reports LicenseModel=general-public-license and the crate's default EngineVersion.
4. Cause 2: at step task#5 DescribeDBClusterSnapshots reads the same snapshot back with the same four divergences plus a missing FullSnapshotSizeInBytes.
5. Cause 3: at step teardown#8 DeleteDBClusterSnapshot returns the same mis-projected element.
6. Equivalent by hand: aws rds create-db-cluster --engine aurora-mysql ... ; aws rds create-db-cluster-snapshot --db-cluster-identifier <id> --db-cluster-snapshot-identifier <snap> ; aws rds describe-db-cluster-snapshots --db-cluster-snapshot-identifier <snap> ; aws rds delete-db-cluster-snapshot --db-cluster-snapshot-identifier <snap> and compare StorageEncryptionType, LicenseModel and EngineVersion against the cluster's.
