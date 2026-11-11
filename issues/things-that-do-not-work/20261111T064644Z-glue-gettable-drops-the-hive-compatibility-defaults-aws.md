Issue-Source: trace-sweep-divergence
Issue-Trace: athena-cats-traj-table-before-database-and-ddl-retries
Issue-Step: task#19
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-cats-traj-table-before-database-and-ddl-retries athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: glue|GetTable|2xx->2xx|$.Table.LastAccessTime
Issue-Key: glue|GetTable|2xx->2xx|$.Table.Parameters.transient_lastDdlTime
Issue-Key: glue|GetTable|2xx->2xx|$.Table.PartitionKeys
Issue-Key: glue|GetTable|2xx->2xx|$.Table.StorageDescriptor.BucketColumns
Issue-Key: glue|GetTable|2xx->2xx|$.Table.StorageDescriptor.SkewedInfo
Issue-Key: glue|GetTable|2xx->2xx|$.Table.StorageDescriptor.Location
Issue-Related: 20261103T171935Z-glue-gettable-omits-the-empty-parameters-maps-aws.md
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# Glue GetTable drops the Hive-compatibility defaults AWS always returns for Athena-created tables (1 cause)

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
One net-new root cause, seen in two fixtures.

Cause 1 of 1 — a Glue GetTable 200 for a table created through Athena DDL omits the Hive-compatibility default members AWS always materializes on such a table (fixtures athena-cats-traj-table-before-database-and-ddl-retries step task#19 and athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket step task#12).
AWS: a table registered in the Data Catalog by Athena/Hive DDL is returned with the Hive defaults filled in rather than left unset — Table.LastAccessTime = 0 (epoch), Table.PartitionKeys = [] (the model explicitly says so: 'When you create a table used by Amazon Athena, and you do not specify any partitionKeys, you must at least set the value of partitionKeys to an empty list. For example: "PartitionKeys": []'), Table.Parameters carrying the Hive bookkeeping key transient_lastDdlTime (a state-derived epoch-seconds string written at DDL time), StorageDescriptor.BucketColumns = [], StorageDescriptor.Parameters = {} and StorageDescriptor.SkewedInfo = {SkewedColumnNames: [], SkewedColumnValues: [], SkewedColumnValueLocationMaps: {}}. Smithy: Table members LastAccessTime (Timestamp), PartitionKeys (ColumnList), Parameters (ParametersMap), StorageDescriptor; StorageDescriptor members BucketColumns (NameStringList), Parameters (ParametersMap), SkewedInfo (SkewedInfo with the three listed members). The empty-collection/zero defaults are recorded behavior for Athena-created tables.
Emulator: all of those members are absent from the GetTable response (each 'missing ... got None'), so the table reads as if partitioning, bucketing, skew and Hive parameters were unknown rather than empty.
Why: Hive/Spark/Athena clients and SDK round-trips treat these as always-present: code that iterates Table.PartitionKeys or reads StorageDescriptor.SkewedInfo.SkewedColumnNames gets a null and raises instead of seeing an empty list, a GetTable -> UpdateTable round-trip silently drops the Hive parameters (including transient_lastDdlTime), and tools that compare catalog state before/after a DDL see fields appear and disappear.
Fix: the Glue table projection/serializer for tables created through the Athena DDL path (the state the catalog stores for such a table, and the GetTable response builder), which must populate these Hive-compatibility defaults.
Traces: athena-cats-traj-table-before-database-and-ddl-retries#task#19, athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket#task#12. The second step additionally shows StorageDescriptor.Location naming the absent bucket from the DDL that AWS failed — that part is downstream of the Athena DDL LOCATION defect filed separately against the athena service and is not a Glue requirement being asserted here.

Related open issues that do NOT cover this cause: '20261108T041854Z-glue-gettables-omits-hive-compatibility-defaults-on-tables.md' records the same omission on the plural GetTables operation (and the replay report does attribute the GetTables cluster to it), but both GetTable clusters above come back unowned; '20261103T171935Z-glue-gettable-omits-the-empty-parameters-maps-aws.md' and '20261102T132146Z-glue-gettables-omits-catalog-and-database-identity-members.md' cover other missing members. A fix that only touches the GetTables path leaves GetTable diverging.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Fixture athena-cats-traj-table-before-database-and-ddl-retries, step task#19, GetTable, 200 -> 200:
  $.Table.LastAccessTime: missing (expected 0.0, got None)
  $.Table.Parameters.transient_lastDdlTime: missing (expected a Hive epoch-seconds string, shaped-not-asserted in the recording, got None)
  $.Table.PartitionKeys: missing (expected [], got None)
  $.Table.StorageDescriptor.BucketColumns: missing (expected [], got None)
  $.Table.StorageDescriptor.Parameters: missing (expected {}, got None)
  $.Table.StorageDescriptor.SkewedInfo: missing (expected {'SkewedColumnNames': [], 'SkewedColumnValueLocationMaps': {}, 'SkewedColumnValues': []}, got None)

Fixture athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket, step task#12, GetTable, 200 -> 200: the same six members missing, plus $.Table.StorageDescriptor.Location expected 's3://<ddl-bucket>/' and got 's3://<absent-bucket>' (downstream of the Athena DDL LOCATION defect, not asserted as a Glue requirement here).

The expected values are the defaults themselves — empty list, empty map, zero timestamp and the presence of transient_lastDdlTime; the table/database/bucket names and the epoch value in the recordings are per-run.

## Steps to Reproduce
1. git_clone() the project and boot the emulator built from the checkout.
2. replay_wire_corpus() over trace set simparity-wire-aws-v1.
3. Replay fixture athena-cats-traj-table-before-database-and-ddl-retries to step task#19 — create a database and a table through Athena DDL (StartQueryExecution with CREATE DATABASE then CREATE EXTERNAL TABLE), then call Glue GetTable for that database/table.
4. Observe the 200 response: Table.LastAccessTime, Table.PartitionKeys, Table.Parameters.transient_lastDdlTime, StorageDescriptor.BucketColumns, StorageDescriptor.Parameters and StorageDescriptor.SkewedInfo are all absent, where AWS returns 0, [], the Hive DDL timestamp, [], {} and the empty SkewedInfo triple.
5. Repeat with fixture athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket up to step task#12 for the same six omissions.
