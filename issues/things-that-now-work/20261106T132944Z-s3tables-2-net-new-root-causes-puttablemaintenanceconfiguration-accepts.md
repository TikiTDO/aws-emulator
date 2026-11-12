Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.s3t-sort-compaction-rejected-until-the-metadata-carries-a-sort-order s3tables.s3t-rebuilding-the-estate-converges-to-the-same-table-state
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.s3t-sort-compaction-rejected-until-the-metadata-carries-a-sort-order|_error#10|AssertionError|raise AssertionError(
Issue-Key: s3tables.s3t-rebuilding-the-estate-converges-to-the-same-table-state|_build#10|AssertionError|assert status == 200, (
Issue-Commit: 664a72bba61e2c00bac218cc260309c5c90c944d

# s3tables: 2 net-new root causes — PutTableMaintenanceConfiguration accepts a sort compaction strategy on a table with no sort order, and the Iceberg REST LoadTable route is unserved (501) on the rebuild path

> _Note: this issue was identified against the code base at commit `664a72bba61e2c00bac218cc260309c5c90c944d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTableMaintenanceConfiguration

## Description
Two distinct net-new root causes in the s3tables emulator, each grounded in a failing behavioral check that the suite report marks neither ALREADY FILED nor OWNED. NOTE: this issue carries 2 SEPARATE causes — both must be closed before submitting.

=== Cause 1 of 2: PutTableMaintenanceConfiguration does not enforce the sort-order precondition for the `sort` compaction strategy ===
AWS: Smithy: PutTableMaintenanceConfiguration PUT /tables/{tableBucketARN}/{namespace}/{name}/maintenance/{type}, success code 204, required input member `value` -> TableMaintenanceConfigurationValue{status: MaintenanceStatus, settings: TableMaintenanceSettings}; IcebergCompactionSettings carries `strategy` (IcebergCompactionStrategy enum) and `targetFileSizeMB`; modeled errors include BadRequestException (httpError 400, "The request is invalid or malformed."). The precondition itself is recorded behavior only: enabling the icebergCompaction strategy `sort` on an Iceberg table whose metadata carries no sort order is refused with code BadRequestException and the message "The specified compaction strategy requires sort order in the table metadata. Please refer to the S3 Tables documentation for steps on configuring sort order." — that message is entirely fixed text with no variable parts. The refusal has no effect: a following GetTableMaintenanceConfiguration still reads the untouched default strategy `auto`. The same put is accepted (204) once the table metadata carries a default sort order (added over the Iceberg REST commit endpoint), and the strategy then reads back as `sort`. A strategy that needs no sort order (`binpack`, with targetFileSizeMB) is accepted on the same table, so only the precondition is refused, not the operation.
Emulator: the sort-strategy put SUCCEEDS — HTTP 204, empty body ({'ResponseMetadata': {'HTTPStatusCode': 204, ...}}) — so the precondition is never evaluated and the configuration is stored as if valid.
Why: a client that relies on AWS refusing an unsupported maintenance configuration gets a silent accept: the table is left configured for a compaction strategy the table metadata cannot support, the caller never learns it must add a sort order first, and negative-path tests written against AWS (expecting BadRequestException with that message) pass against the emulator and fail in production. The read-back after the refusal also diverges, because the emulator's stored strategy is no longer the default `auto`.
Fix: the PutTableMaintenanceConfiguration handler and its validation in the s3tables crate (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_maintenance_configuration.rs) — gate an icebergCompaction value whose strategy is `sort` on the target table's metadata carrying a sort order, refusing with BadRequestException before any state change. Open issue 20261101T233620Z-s3tables-puttablemaintenanceconfiguration-stub.md covers standing the operation up at all; the operation now answers 204, so it does not cover this missing precondition.
Traces: s3tables.s3t-sort-compaction-rejected-until-the-metadata-carries-a-sort-order.

=== Cause 2 of 2: the Iceberg REST LoadTable route answers 501 NotImplemented, so the rebuild/convergence path cannot read a table's metadata ===
AWS: recorded behavior only — the Iceberg REST catalog is not bound in the s3tables Smithy model (no operation maps /iceberg/v1/*). Real S3 Tables serves it alongside the control plane: GET /iceberg/v1/{url-encoded table bucket ARN}/namespaces/{namespace}/tables/{table} (LoadTable) answers HTTP 200 with the table's Iceberg metadata document, including `metadata.location` (the warehouse location), `metadata.schemas` with field-ids assigned in declaration order, and `metadata.table-uuid`.
Emulator: the route answers HTTP 501 with body {"__type": "NotImplemented", "message": "operation GET /iceberg/v1/<encoded bucket ARN>/namespaces/<namespace>/tables/<table> is not implemented in this emulator"}. The fixed text is "operation <METHOD> <path> is not implemented in this emulator"; the encoded bucket ARN, namespace and table name are values taken from the request, not part of the requirement.
Why: every data-plane consumer of S3 Tables (Spark, Trino, PyIceberg, Athena) loads a table through this route, so no table's schema, metadata location or UUID can be read; here it also makes the control-plane property under test unobservable — after deleting and rebuilding a namespace and its tables in the same bucket, a client cannot confirm the rebuilt tables converge on the same schema while minting a fresh warehouse location and table ARN.
Fix: the s3tables crate's non-Smithy Iceberg REST catalog router and its LoadTable handler (the /iceberg/v1 surface of services/service-com-amazonaws-s3tables-s3tablebuckets), projecting the same namespace/table state the control-plane handlers use. Open issue 20261102T104943Z-s3tables-8-net-new-root-causes-the-iceberg.md describes the same unserved-catalog root cause (its Cause 1) but does not carry this check's replay key, so this check is still unrun by any filed issue; closing that cause should be re-verified against the check listed below.
Traces: s3tables.s3t-rebuilding-the-estate-converges-to-the-same-table-state.

## Evidence
Behavioral suite: 48/149 passed; s3tables 42/139. The two failing checks below are the only s3tables failures the report marks neither "ALREADY FILED" nor "OWNED by".

Cause 1 — [mismatch] s3tables.s3t-sort-compaction-rejected-until-the-metadata-carries-a-sort-order:
  AssertionError: the call was expected to be rejected but succeeded, returning {'ResponseMetadata': {'HTTPStatusCode': 204, 'HTTPHeaders': {'content-type': 'application/json', ...}, 'RetryAttempts': 0}}
  The call: put_table_maintenance_configuration(tableBucketARN=<bucket ARN>, namespace=<ns>, name='user_events_with_sort_compaction', type='icebergCompaction', value={'status': 'enabled', 'settings': {'icebergCompaction': {'strategy': 'sort'}}}) on a table created with an Iceberg schema and no sort order.
  Expected: ('BadRequestException', 'The specified compaction strategy requires sort order in the table metadata. Please refer to the S3 Tables documentation for steps on configuring sort order.') at HTTP 400, with GetTableMaintenanceConfiguration still reporting strategy 'auto' afterwards.

Cause 2 — [mismatch] s3tables.s3t-rebuilding-the-estate-converges-to-the-same-table-state:
  AssertionError: loading 'user_events_with_sort_compaction' over the catalog returned HTTP 501: {'__type': 'NotImplemented', 'message': 'operation GET /iceberg/v1/arn%3Aaws%3As3tables%3A<region>%3A<account>%3Abucket%2F<bucket>/namespaces/<ns>/tables/user_events_with_sort_compaction is not implemented in this emulator'}
  Expected: HTTP 200 with the Iceberg metadata document (metadata.location, metadata.schemas, metadata.table-uuid). The region, account, bucket and namespace shown are per-run values.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, or reproduce by hand against the s3tables endpoint:

Cause 1:
1. create_table_bucket(name=<bucket>) -> arn; create_namespace(tableBucketARN=arn, namespace=[<ns>]).
2. create_table(tableBucketARN=arn, namespace=<ns>, name='user_events_with_sort_compaction', format='ICEBERG', metadata={'iceberg': {'schema': {'fields': [{'name':'user_id','type':'string','required':True}, {'name':'event_type','type':'string','required':True}, {'name':'event_timestamp','type':'long','required':True}, {'name':'session_id','type':'string','required':False}]}}}).
3. put_table_maintenance_configuration(tableBucketARN=arn, namespace=<ns>, name=<table>, type='icebergCompaction', value={'status':'enabled','settings':{'icebergCompaction':{'strategy':'sort'}}}). Emulator: HTTP 204 success. AWS: 400 BadRequestException "The specified compaction strategy requires sort order in the table metadata. Please refer to the S3 Tables documentation for steps on configuring sort order."
4. get_table_maintenance_configuration(...) -> AWS still reports configuration.icebergCompaction.settings.icebergCompaction.strategy == 'auto'.
5. Control: the same put with {'strategy':'binpack','targetFileSizeMB':256} is accepted and reads back unchanged; after adding a default sort order over the Iceberg REST commit endpoint, the identical sort put is accepted and reads back status 'enabled', strategy 'sort'.

Cause 2:
1. create_table_bucket -> arn; create_namespace; create_table(format='ICEBERG', metadata=<the schema above>).
2. Issue a SigV4-signed GET to <s3tables endpoint>/iceberg/v1/<url-encoded arn>/namespaces/<ns>/tables/<table>. Emulator: 501 NotImplemented. AWS: 200 with the table's Iceberg metadata document.
3. The check then deletes the tables and namespace, rebuilds them in the same bucket and compares: the schemas and maintenance configurations must match the first build while namespaceId, tableARN and metadata.location must all be newly minted — none of which can be observed while LoadTable answers 501.
