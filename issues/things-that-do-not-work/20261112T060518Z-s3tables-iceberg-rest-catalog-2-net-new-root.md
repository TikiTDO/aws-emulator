Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3tables.s3t-a-sort-order-commit-advances-the-metadata-location-and-version-token s3tables.s3t-iceberg-rest-load-table-assigns-field-ids-in-declaration-order
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables.s3t-a-sort-order-commit-advances-the-metadata-location-and-version-token|run#21|IndexError|"timestamp-ms": committed["metadata"]["metadata-log"][0]["timestamp-ms"],
Issue-Key: s3tables.s3t-iceberg-rest-load-table-assigns-field-ids-in-declaration-order|run#30|AssertionError|assert metadata["properties"] == {"write.parquet.compression-codec": "zstd"}, (
Issue-Commit: 848ff7798cc0e070898f50ccf51befabeb3eafe0

# s3tables Iceberg REST catalog: 2 net-new root causes — a commit returns an empty metadata-log instead of retaining the pre-commit metadata file, and LoadTable returns no table properties instead of the zstd Parquet codec

> _Note: this issue was identified against the code base at commit `848ff7798cc0e070898f50ccf51befabeb3eafe0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LoadTable

## Description
Two distinct net-new root causes in the s3tables emulator's Iceberg REST catalog surface (the non-Smithy /iceberg/v1 routes), each grounded in a failing behavioral check that is NOT marked ALREADY FILED or OWNED. NOTE: this issue carries 2 SEPARATE causes — both must be closed before submitting.

Context: the open issue 20261102T104943Z-s3tables-8-net-new-root-causes-the-iceberg.md (Cause 1) covered the catalog answering a blanket HTTP 501 for every /iceberg/v1 route, and its replay keys for these two checks are the `assert status == 200` keys. The routes now answer 200, and these two checks fail further down on the CONTENT of the metadata document — different keys, not covered by that issue; it is named here only for lineage.

=== Cause 1 of 2: an Iceberg REST commit returns `metadata-log` empty, dropping the pre-commit metadata file from the metadata chain ===
AWS: recorded behavior only — the Iceberg REST catalog is not part of the s3tables Smithy model (no operation in services/service-com-amazonaws-s3tables-s3tablebuckets/smithy-model/com-amazonaws-s3tables-s3tablebuckets.json binds /iceberg/v1/*), so this is the recorded behavior of the catalog plus the Iceberg table-metadata specification it serves. A successful commit (POST /iceberg/v1/{prefix}/namespaces/{namespace}/tables/{table} with requirements + updates) returns HTTP 200 and a metadata document in which `metadata-log` holds one entry per SUPERSEDED metadata file, in commit order: after the first commit on a freshly created table it holds exactly one entry whose `metadata-file` is the table's pre-commit `metadataLocation` (the value GetTable reported before the commit) and whose `timestamp-ms` is the millisecond timestamp at which that file was written. `metadata-location` advances to the next numbered metadata file (the 00001- sequence, the pre-commit one being 00000-), and the s3tables control plane's GetTable/GetTableMetadataLocation (Smithy: output members metadataLocation, versionToken, warehouseLocation) then report that new location and a rotated versionToken.
Emulator: the commit is accepted (HTTP 200) but the returned metadata document's `metadata-log` is an EMPTY list, so indexing its first entry raises `IndexError: list index out of range` in the check. The emulator therefore publishes a new current metadata document with no record of the metadata file it superseded.
Why: `metadata-log` is how an Iceberg reader walks a table's metadata history — time travel to a previous metadata version, `rollback`/`set-current-snapshot` style recovery, and any tool that audits or garbage-collects superseded metadata files all read it. With an always-empty log a client sees every commit as the table's first, cannot reach any earlier metadata version, and a metadata-file GC that trusts the log treats the superseded file as unreferenced.
Fix: the commit (UpdateTable) handler of the s3tables crate's Iceberg REST catalog router — when a commit writes a new metadata document it must append the previous metadata file and its write timestamp to `metadata-log` in the document it persists and returns, rather than emitting a fresh empty log.
Traces: s3tables.s3t-a-sort-order-commit-advances-the-metadata-location-and-version-token.

=== Cause 2 of 2: Iceberg REST LoadTable returns `properties` as {} instead of the zstd Parquet codec S3 Tables sets ===
AWS: recorded behavior only (the catalog is unmodeled, as above). A table created through CreateTable (Smithy: CreateTable PUT /tables/{tableBucketARN}/{namespace}, format ICEBERG, metadata.iceberg.schema.fields) and then loaded over the catalog (GET /iceberg/v1/{prefix}/namespaces/{namespace}/tables/{table}) comes back with a format-version-2 metadata document whose `properties` map holds exactly the one table property S3 Tables sets on every managed table: {"write.parquet.compression-codec": "zstd"}. The key and the value are both fixed text, and no other property is present on a freshly created table.
Emulator: the same LoadTable returns `properties` as an empty map {} — AssertionError: "the table properties came back {}; S3 Tables sets exactly the zstd Parquet codec, which decides how a writer encodes the data files". Every other assertion in the check (format-version, table-uuid, location, schema field ids 1..N in declaration order, last-column-id, sort-orders, default-sort-order-id, current-snapshot-id, snapshots, snapshot-log) passes, so the divergence is confined to the properties map.
Why: `properties` is what an Iceberg writer consults to decide how it encodes data files. With the codec property absent, Spark/Trino/PyIceberg write with their own default codec (gzip or snappy for Parquet) instead of the zstd S3 Tables mandates, so files written against the emulator do not match what the same job produces against AWS; a client that reads the property to report or enforce the table's compression sees it as unset.
Fix: the table-metadata projection used by the Iceberg REST LoadTable handler (and by whatever materialises a table's initial metadata document in the s3tables crate's CreateTable state) — seed and serialize the default property map rather than an empty one.
Traces: s3tables.s3t-iceberg-rest-load-table-assigns-field-ids-in-declaration-order.

## Evidence
Behavioral suite: 62/156 passed; s3tables 56/139. Both checks below are s3tables failures carrying NO ALREADY FILED / OWNED marker.

Cause 1 — [error] s3tables.s3t-a-sort-order-commit-advances-the-metadata-location-and-version-token:
  File "check_s3tables_s3t_a_sort_order_commit_advances_the_metadata_location_and_version_token.py", line 181, in run
    "timestamp-ms": committed["metadata"]["metadata-log"][0]["timestamp-ms"],
  IndexError: list index out of range
The commit itself returned HTTP 200 and satisfied the preceding assertions (default-sort-order-id == 1 and sort-orders == [{order-id 0, fields []}, {order-id 1, fields [the identity/asc/nulls-first field on the user_id column]}]). Expected: committed["metadata"]["metadata-log"] == [{"metadata-file": <the metadataLocation GetTable reported before the commit>, "timestamp-ms": <ms timestamp of that file>}] — one entry, the pre-commit metadata file. Observed: []. The specific location string and timestamp are per-run values that fill the template; the single retained entry is the requirement.

Cause 2 — [mismatch] s3tables.s3t-iceberg-rest-load-table-assigns-field-ids-in-declaration-order:
  File "check_s3tables_s3t_iceberg_rest_load_table_assigns_field_ids_in_declaration_order.py", line 195, in run
    assert metadata["properties"] == {"write.parquet.compression-codec": "zstd"}, (
  AssertionError: the table properties came back {}; S3 Tables sets exactly the zstd Parquet codec, which decides how a writer encodes the data files
Expected: properties == {"write.parquet.compression-codec": "zstd"} (both key and value fixed text). Observed: {}.

## Steps to Reproduce
Boot the emulator and replay the two checks, or reproduce by hand against the s3tables endpoint (region/account/bucket names below are per-run values):

Setup (both causes): arn = create_table_bucket(name=<bucket>)["arn"]; create_namespace(tableBucketARN=arn, namespace=[<ns>]); create_table(tableBucketARN=arn, namespace=<ns>, name="user_events_with_sort_compaction", format="ICEBERG", metadata={"iceberg": {"schema": {"fields": [{name user_id, string, required}, {name event_type, string, required}, {name event_timestamp, long, required}, {name session_id, string, not required}]}}}). prefix = urlencode(arn).

Cause 2: SigV4-signed GET <s3tables endpoint>/iceberg/v1/{prefix}/namespaces/{ns}/tables/user_events_with_sort_compaction. The response is HTTP 200; inspect metadata["properties"] — the emulator returns {}, AWS returns {"write.parquet.compression-codec": "zstd"}.

Cause 1: record before = get_table(tableBucketARN=arn, namespace=<ns>, name=<table>)["metadataLocation"]. Load the table over the catalog as above and read the id of the "user_id" column from metadata["schemas"][0]["fields"]. Then SigV4-signed POST to the same path with body {"identifier": {"namespace": [<ns>], "name": <table>}, "requirements": [{"type": "assert-table-uuid", "uuid": <loaded metadata table-uuid>}], "updates": [{"action": "add-sort-order", "sort-order": {"order-id": 1, "fields": [{"source-id": <user_id id>, "transform": "identity", "direction": "asc", "null-order": "nulls-first"}]}}, {"action": "set-default-sort-order", "sort-order-id": -1}]}. The commit returns 200; inspect metadata["metadata-log"] — the emulator returns [], AWS returns one entry whose "metadata-file" is `before`. Then check that get_table / get_table_metadata_location report the advanced 00001- metadataLocation and a rotated versionToken.
