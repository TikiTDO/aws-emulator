// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Iceberg REST catalog this service serves
//! beside its control plane, at `/iceberg/v1`.
//!
//! The rule under test: the catalog's table endpoint projects the SAME table
//! records the control plane reads and writes. A load answers the table's
//! Iceberg metadata document — the schema the create declared, with a field id
//! for every column, the table's warehouse location, the UUID that identifies
//! the table and the sort order it writes in — beside the location of the
//! metadata file it is written to; a commit applies its updates to that
//! document, under the requirements it states, and writes a new metadata file
//! and a new version token for the table; and a table, namespace or warehouse
//! the store does not hold is answered as absent rather than as unimplemented.
//!
//! The table spans what the rule generalises over: both schema forms the model
//! declares (`schema` with and without stated field ids, `schemaV2` with a
//! nested column type), a table with and without a sort order, a table with and
//! without a partition spec, properties, the region, and each coordinate of the
//! endpoint being absent.

use serde_json::{json, Value};
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;

use s3tables::state::OpenTableFormat;

use super::harness::{catalog_table_path, Emulator, Reply};

/// The `(id, name, required)` of one column, as a load reports it.
type Column = (i64, String, bool);

/// One row of the load table: the case, the region, the `metadata` the create
/// carries, the columns the load must report and whether the table writes in a
/// sort order.
type LoadCase = (&'static str, &'static str, Value, Vec<Column>, bool);

/// One table created for the catalog to serve.
struct Created {
    bucket_arn: String,
    table_arn: String,
    warehouse_location: String,
    metadata_location: String,
    version_token: String,
}

/// Create a table bucket, a namespace and one table whose create carries
/// `metadata`, and read its control-plane record back.
async fn create_table(
    emulator: &Emulator,
    region: &str,
    bucket: &str,
    namespace: &str,
    table: &str,
    metadata: Option<Value>,
) -> Created {
    let created = emulator
        .create_table_bucket(region, &json!({ "name": bucket }))
        .await;
    assert_eq!(
        created.status, 200,
        "creating the bucket {bucket:?} must succeed, got {status} {body}",
        status = created.status,
        body = created.body
    );
    let bucket_arn = created.text("arn");
    let namespace_created = emulator
        .create_namespace(region, &bucket_arn, &json!({ "namespace": [namespace] }))
        .await;
    assert_eq!(
        namespace_created.status, 200,
        "creating the namespace {namespace:?} must succeed, got {status} {body}",
        status = namespace_created.status,
        body = namespace_created.body
    );
    let mut request = json!({ "name": table, "format": OpenTableFormat::Iceberg.as_wire() });
    if let Some(metadata) = metadata {
        request
            .as_object_mut()
            .expect("the create request is an object")
            .insert("metadata".to_string(), metadata);
    }
    let table_created = emulator
        .create_table(region, &bucket_arn, namespace, &request)
        .await;
    assert_eq!(
        table_created.status, 200,
        "creating the table {table:?} must succeed, got {status} {body}",
        status = table_created.status,
        body = table_created.body
    );
    let read = emulator
        .get_table(region, &bucket_arn, namespace, table)
        .await;
    assert_eq!(
        read.status, 200,
        "reading the table {table:?} must succeed, got {status} {body}",
        status = read.status,
        body = read.body
    );
    Created {
        bucket_arn,
        table_arn: read.text("tableARN"),
        warehouse_location: read.text("warehouseLocation"),
        metadata_location: read
            .optional_text("metadataLocation")
            .unwrap_or_else(|| panic!("the table must carry a metadata location: {}", read.body)),
        version_token: read.text("versionToken"),
    }
}

/// The `metadata` document a load answered with.
fn metadata(reply: &Reply, case: &str) -> Value {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the load must succeed, got {status} {body}",
        status = reply.status,
        body = reply.body
    );
    reply
        .body
        .get("metadata")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the load carries no metadata: {}", reply.body))
}

/// The `(id, name, required)` of every column of the document's current schema,
/// in the order the schema declares them.
fn columns(document: &Value, case: &str) -> Vec<Column> {
    let current = document
        .get("current-schema-id")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: the document names no current schema"));
    let schemas = document
        .get("schemas")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: the document carries no schemas"));
    let schema = schemas
        .iter()
        .find(|schema| schema.get("schema-id").and_then(Value::as_i64) == Some(current))
        .unwrap_or_else(|| panic!("case {case:?}: no schema carries the current schema id"));
    schema
        .get("fields")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: the current schema carries no fields"))
        .iter()
        .map(|field| {
            (
                field.get("id").and_then(Value::as_i64).unwrap_or_default(),
                field
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                field
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// The table's UUID, as its ARN carries it: the last segment of the ARN is the
/// table's server-minted id, which is what identifies the table for the life of
/// the record.
fn table_uuid(table_arn: &str) -> String {
    table_arn
        .rsplit('/')
        .next()
        .expect("a table ARN ends in the table's id")
        .to_string()
}

#[tokio::test]
async fn a_load_reports_the_table_the_control_plane_created() {
    let emulator = Emulator::new();
    let namespace = "events";
    let table = "user_events";

    // (the case, the region, the `metadata` the create carries, the columns the
    // load must report, whether the table writes in a sort order)
    let rows: Vec<LoadCase> = vec![
        (
            "a schema whose columns state no field id",
            "us-west-2",
            json!({ "iceberg": { "schema": { "fields": [
                { "name": "user_id", "type": "string", "required": true },
                { "name": "event_type", "type": "string", "required": true },
                { "name": "event_timestamp", "type": "long", "required": true },
                { "name": "session_id", "type": "string", "required": false },
            ]}}}),
            vec![
                (1, "user_id".to_string(), true),
                (2, "event_type".to_string(), true),
                (3, "event_timestamp".to_string(), true),
                (4, "session_id".to_string(), false),
            ],
            false,
        ),
        (
            "a schema whose columns state their own field ids",
            "eu-west-1",
            json!({ "iceberg": { "schema": { "fields": [
                { "id": 11, "name": "a", "type": "int", "required": true },
                { "id": 12, "name": "b", "type": "decimal(10, 2)", "required": false },
            ]}}}),
            vec![
                (11, "a".to_string(), true),
                (12, "b".to_string(), false),
            ],
            false,
        ),
        (
            "a schema that numbers some columns and leaves the rest to the service",
            "ap-south-1",
            json!({ "iceberg": { "schema": { "fields": [
                { "id": 5, "name": "first", "type": "string", "required": false },
                { "name": "second", "type": "boolean", "required": false },
                { "name": "third", "type": "uuid", "required": true },
            ]}}}),
            vec![
                (5, "first".to_string(), false),
                (1, "second".to_string(), false),
                (2, "third".to_string(), true),
            ],
            false,
        ),
        (
            "a version 2 schema with a nested column type",
            "us-east-2",
            json!({ "iceberg": { "schemaV2": {
                "type": "struct",
                "schema-id": 0,
                "fields": [
                    { "id": 1, "name": "id", "type": "long", "required": true },
                    {
                        "id": 2,
                        "name": "payload",
                        "required": false,
                        "type": {
                            "type": "list",
                            "element-id": 3,
                            "element": "string",
                            "element-required": false,
                        },
                    },
                ],
            }}}),
            vec![(1, "id".to_string(), true), (2, "payload".to_string(), false)],
            false,
        ),
        (
            "a table the create gave a sort order and a partition spec",
            "ca-central-1",
            json!({ "iceberg": {
                "schema": { "fields": [
                    { "name": "id", "type": "long", "required": true },
                    { "name": "day", "type": "date", "required": true },
                ]},
                "partitionSpec": { "spec-id": 0, "fields": [
                    { "source-id": 2, "transform": "day", "name": "day_partition" },
                ]},
                "writeOrder": { "order-id": 1, "fields": [
                    {
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                ]},
                "properties": { "write.format.default": "parquet" },
            }}),
            vec![(1, "id".to_string(), true), (2, "day".to_string(), true)],
            true,
        ),
    ];

    for (index, (case, region, metadata_member, expected_columns, sorted)) in
        rows.into_iter().enumerate()
    {
        let bucket = format!("catalog-load-{index}");
        let created = create_table(
            &emulator,
            region,
            &bucket,
            namespace,
            table,
            Some(metadata_member),
        )
        .await;

        let loaded = emulator
            .load_table(region, &created.bucket_arn, namespace, table)
            .await;
        let document = metadata(&loaded, case);

        assert_eq!(
            loaded.body.get("metadata-location").and_then(Value::as_str),
            Some(created.metadata_location.as_str()),
            "case {case:?}: expected the metadata location the control plane reports \
             ({expected:?}), got {actual:?}",
            expected = created.metadata_location,
            actual = loaded.body.get("metadata-location")
        );
        assert_eq!(
            document.get("location").and_then(Value::as_str),
            Some(created.warehouse_location.as_str()),
            "case {case:?}: expected the table's warehouse location {expected:?}, got {actual:?}",
            expected = created.warehouse_location,
            actual = document.get("location")
        );
        assert_eq!(
            document.get("table-uuid").and_then(Value::as_str),
            Some(table_uuid(&created.table_arn).as_str()),
            "case {case:?}: expected the table's own id {expected:?}, got {actual:?}",
            expected = table_uuid(&created.table_arn),
            actual = document.get("table-uuid")
        );
        assert_eq!(
            document.get("format-version").and_then(Value::as_i64),
            Some(2),
            "case {case:?}: expected an Iceberg v2 table, got {actual:?}",
            actual = document.get("format-version")
        );
        let actual_columns = columns(&document, case);
        assert_eq!(
            actual_columns, expected_columns,
            "case {case:?}: expected the columns {expected_columns:?}, got {actual_columns:?}"
        );
        let default_order = document
            .get("default-sort-order-id")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let orders = document
            .get("sort-orders")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let sorted_fields = orders
            .iter()
            .find(|order| order.get("order-id").and_then(Value::as_i64) == Some(default_order))
            .and_then(|order| order.get("fields"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            sorted_fields > 0,
            sorted,
            "case {case:?}: expected the table to write in a sort order: {sorted}, got \
             {sorted_fields} sort field(s) under order {default_order}"
        );
        // A load writes nothing, so a second load answers the same document.
        let again = emulator
            .load_table(region, &created.bucket_arn, namespace, table)
            .await;
        assert_eq!(
            metadata(&again, case),
            document,
            "case {case:?}: a second load must report the same document"
        );
    }
}

#[tokio::test]
async fn a_partition_field_that_states_no_id_is_numbered_by_the_service() {
    let emulator = Emulator::new();
    let region = "eu-central-1";
    let namespace = "events";
    let table = "user_events";

    // (the case, the partition spec the create declares, the field ids the load
    // must report for it). `IcebergPartitionField.fieldId` is optional and
    // documented as assigned by the service when the request states none
    // (`smithy_query(query="shape", shape="IcebergPartitionField")`), and the
    // ids it assigns start where the Iceberg table specification numbers
    // partition fields from.
    let rows: Vec<(&str, Value, Vec<i64>)> = vec![
        (
            "no field states an id",
            json!({ "fields": [
                { "source-id": 1, "transform": "identity", "name": "by_id" },
                { "source-id": 2, "transform": "identity", "name": "by_day" },
            ]}),
            vec![1000, 1001],
        ),
        (
            "one field states the first id and the other states none",
            json!({ "fields": [
                { "source-id": 1, "transform": "identity", "name": "by_id", "field-id": 1000 },
                { "source-id": 2, "transform": "identity", "name": "by_day" },
            ]}),
            vec![1000, 1001],
        ),
        (
            "one field states an id past the start and the other states none",
            json!({ "fields": [
                { "source-id": 1, "transform": "identity", "name": "by_id", "field-id": 1005 },
                { "source-id": 2, "transform": "identity", "name": "by_day" },
            ]}),
            vec![1005, 1000],
        ),
    ];

    for (index, (case, spec, expected)) in rows.into_iter().enumerate() {
        let bucket = format!("catalog-partition-{index}");
        let created = create_table(
            &emulator,
            region,
            &bucket,
            namespace,
            table,
            Some(json!({ "iceberg": {
                "schema": { "fields": [
                    { "name": "id", "type": "long", "required": true },
                    { "name": "day", "type": "date", "required": true },
                ]},
                "partitionSpec": spec,
            }})),
        )
        .await;

        let document = metadata(
            &emulator
                .load_table(region, &created.bucket_arn, namespace, table)
                .await,
            case,
        );
        let default_spec = document
            .get("default-spec-id")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let actual: Vec<i64> = document
            .get("partition-specs")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: the document carries no partition specs"))
            .iter()
            .find(|spec| spec.get("spec-id").and_then(Value::as_i64) == Some(default_spec))
            .and_then(|spec| spec.get("fields"))
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no spec carries the default spec id"))
            .iter()
            .map(|field| {
                field
                    .get("field-id")
                    .and_then(Value::as_i64)
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the partition field ids {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_table_the_catalog_does_not_hold_is_answered_as_absent() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let namespace = "events";
    let table = "user_events";
    let created = create_table(
        &emulator,
        region,
        "catalog-absent",
        namespace,
        table,
        Some(json!({ "iceberg": { "schema": { "fields": [
            { "name": "id", "type": "long", "required": true },
        ]}}})),
    )
    .await;

    // (the case, the warehouse prefix, the namespace, the table, the exception
    // the catalog names the class by)
    let rows: Vec<(&str, String, &str, &str, &str)> = vec![
        (
            "a table the namespace does not hold",
            created.bucket_arn.clone(),
            namespace,
            "absent_table",
            "NoSuchTableException",
        ),
        (
            "a namespace the warehouse does not hold",
            created.bucket_arn.clone(),
            "absent_ns",
            table,
            "NoSuchNamespaceException",
        ),
        (
            "a warehouse that was never created",
            "arn:aws:s3tables:us-east-1:000000000000:bucket/catalog-absent-warehouse".to_string(),
            namespace,
            table,
            "NoSuchNamespaceException",
        ),
        (
            "the resident warehouse in another account",
            created.bucket_arn.replace("000000000000", "123456789012"),
            namespace,
            table,
            "NoSuchNamespaceException",
        ),
    ];

    for (case, prefix, requested_namespace, requested_table, kind) in rows {
        let reply = emulator
            .load_table(region, &prefix, requested_namespace, requested_table)
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {status} {body}",
            status = reply.status,
            body = reply.body
        );
        let error = reply.body.get("error").cloned().unwrap_or(Value::Null);
        assert_eq!(
            error.get("type").and_then(Value::as_str),
            Some(kind),
            "case {case:?}: expected {kind}, got {error}"
        );
        assert_eq!(
            error.get("code").and_then(Value::as_i64),
            Some(404),
            "case {case:?}: expected the code to be the status, got {error}"
        );

        let exists = emulator
            .table_exists(region, &prefix, requested_namespace, requested_table)
            .await;
        assert_eq!(
            exists.status, 404,
            "case {case:?}: the existence check must answer 404, got {status}",
            status = exists.status
        );
    }

    // The resident table still answers, so the refusals above are about what
    // each named and not about an empty catalog.
    let exists = emulator
        .table_exists(region, &created.bucket_arn, namespace, table)
        .await;
    assert_eq!(
        exists.status, 204,
        "the resident table must answer the existence check, got {status}",
        status = exists.status
    );
    let loaded = emulator
        .load_table(region, &created.bucket_arn, namespace, table)
        .await;
    assert_eq!(
        loaded.status, 200,
        "the resident table must still load, got {status} {body}",
        status = loaded.status,
        body = loaded.body
    );
}

#[tokio::test]
async fn a_commit_advances_the_metadata_and_the_version_token() {
    let emulator = Emulator::new();
    let region = "us-west-2";
    let namespace = "events";
    let table = "user_events";
    let created = create_table(
        &emulator,
        region,
        "catalog-commit",
        namespace,
        table,
        Some(json!({ "iceberg": { "schema": { "fields": [
            { "name": "user_id", "type": "string", "required": true },
            { "name": "event_timestamp", "type": "long", "required": true },
        ]}}})),
    )
    .await;
    let uuid = table_uuid(&created.table_arn);

    // A commit whose table UUID is not the table's is refused, and nothing it
    // asked for is applied.
    let mismatched = emulator
        .commit_table(
            region,
            &created.bucket_arn,
            namespace,
            table,
            &json!({
                "requirements": [
                    { "type": "assert-table-uuid", "uuid": "00000000-0000-4000-8000-000000000000" },
                ],
                "updates": [
                    { "action": "set-properties", "updates": { "owner": "analytics" } },
                ],
            }),
        )
        .await;
    assert_eq!(
        mismatched.status, 409,
        "a commit under another table's UUID must be refused, got {status} {body}",
        status = mismatched.status,
        body = mismatched.body
    );
    assert_eq!(
        mismatched
            .body
            .get("error")
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str),
        Some("CommitFailedException"),
        "expected CommitFailedException, got {body}",
        body = mismatched.body
    );
    let unchanged = metadata(
        &emulator
            .load_table(region, &created.bucket_arn, namespace, table)
            .await,
        "after the refused commit",
    );
    assert_eq!(
        unchanged.get("properties"),
        Some(&json!({})),
        "a refused commit must apply none of its updates, got {actual:?}",
        actual = unchanged.get("properties")
    );

    // The same updates under the table's own UUID are applied whole.
    let committed = emulator
        .commit_table(
            region,
            &created.bucket_arn,
            namespace,
            table,
            &json!({
                "requirements": [
                    { "type": "assert-table-uuid", "uuid": uuid },
                    { "type": "assert-current-schema-id", "current-schema-id": 0 },
                ],
                "updates": [
                    { "action": "set-properties", "updates": { "owner": "analytics" } },
                    { "action": "add-sort-order", "sort-order": { "fields": [
                        {
                            "source-id": 2,
                            "transform": "identity",
                            "direction": "asc",
                            "null-order": "nulls-first",
                        },
                    ]}},
                    { "action": "set-default-sort-order", "sort-order-id": -1 },
                ],
            }),
        )
        .await;
    assert_eq!(
        committed.status, 200,
        "the commit must succeed, got {status} {body}",
        status = committed.status,
        body = committed.body
    );
    let location = committed
        .body
        .get("metadata-location")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        location, created.metadata_location,
        "the commit must write a new metadata file, got {location:?}"
    );
    let document = metadata(&committed, "the commit");
    assert_eq!(
        document.get("properties"),
        Some(&json!({ "owner": "analytics" })),
        "expected the committed properties, got {actual:?}",
        actual = document.get("properties")
    );
    let default_order = document
        .get("default-sort-order-id")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    assert_ne!(
        default_order, 0,
        "the commit must make the added order the table's default, got {default_order}"
    );

    // The control plane reports the same new metadata file, under a new version
    // token, and a load answers the committed document.
    let read = emulator
        .get_table(region, &created.bucket_arn, namespace, table)
        .await;
    assert_eq!(
        read.optional_text("metadataLocation").as_deref(),
        Some(location.as_str()),
        "expected the control plane to report the committed metadata location {location:?}, got \
         {actual:?}",
        actual = read.optional_text("metadataLocation")
    );
    assert_ne!(
        read.text("versionToken"),
        created.version_token,
        "the commit must mint a new version token"
    );
    let loaded = metadata(
        &emulator
            .load_table(region, &created.bucket_arn, namespace, table)
            .await,
        "after the commit",
    );
    assert_eq!(
        loaded, document,
        "a load after the commit must answer the committed document"
    );
}

#[tokio::test]
async fn a_commit_this_catalog_cannot_apply_is_refused_as_what_it_is() {
    let emulator = Emulator::new();
    let region = "eu-west-1";
    let namespace = "events";
    let table = "user_events";
    let created = create_table(
        &emulator,
        region,
        "catalog-commit-refusals",
        namespace,
        table,
        Some(json!({ "iceberg": { "schema": { "fields": [
            { "name": "id", "type": "long", "required": true },
        ]}}})),
    )
    .await;

    // (the case, the commit body, the status, the exception the catalog names
    // the class by)
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a requirement this catalog does not know",
            json!({ "requirements": [{ "type": "assert-everything" }], "updates": [] }),
            400,
            "BadRequestException",
        ),
        (
            "an update action this catalog does not know",
            json!({ "updates": [{ "action": "rewrite-everything" }] }),
            400,
            "BadRequestException",
        ),
        (
            "an update that leaves out the member it is defined by",
            json!({ "updates": [{ "action": "set-default-sort-order" }] }),
            400,
            "BadRequestException",
        ),
        (
            "a default sort order no order of the table carries",
            json!({ "updates": [
                { "action": "set-default-sort-order", "sort-order-id": 97 },
            ]}),
            400,
            "BadRequestException",
        ),
        (
            "a commit of a snapshot, which no data plane of this emulator wrote",
            json!({ "updates": [
                { "action": "add-snapshot", "snapshot": { "snapshot-id": 1 } },
            ]}),
            501,
            "NotImplementedException",
        ),
        (
            "a commit that moves the table out of the location the service gave it",
            json!({ "updates": [
                { "action": "set-location", "location": "s3://somewhere-else" },
            ]}),
            400,
            "BadRequestException",
        ),
        (
            "a commit that requires the table not to exist yet",
            json!({ "requirements": [{ "type": "assert-create" }], "updates": [] }),
            409,
            "CommitFailedException",
        ),
    ];

    for (case, body, status, kind) in rows {
        let reply = emulator
            .commit_table(region, &created.bucket_arn, namespace, table, &body)
            .await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected {status}, got {actual} {body}",
            actual = reply.status,
            body = reply.body
        );
        assert_eq!(
            reply
                .body
                .get("error")
                .and_then(|error| error.get("type"))
                .and_then(Value::as_str),
            Some(kind),
            "case {case:?}: expected {kind}, got {body}",
            body = reply.body
        );
        // None of them changed the table: its metadata file is the one the
        // create wrote.
        let read = emulator
            .get_table(region, &created.bucket_arn, namespace, table)
            .await;
        assert_eq!(
            read.optional_text("metadataLocation").as_deref(),
            Some(created.metadata_location.as_str()),
            "case {case:?}: a refused commit must write no metadata file, got {actual:?}",
            actual = read.optional_text("metadataLocation")
        );
    }
}

#[tokio::test]
async fn a_commit_past_the_bounds_one_commit_is_read_under_is_refused() {
    let emulator = Emulator::new();
    let region = "us-east-2";
    let namespace = "events";
    let table = "user_events";
    let created = create_table(
        &emulator,
        region,
        "catalog-commit-bounds",
        namespace,
        table,
        Some(json!({ "iceberg": { "schema": { "fields": [
            { "name": "id", "type": "long", "required": true },
        ]}}})),
    )
    .await;

    // Past the bound the catalog reads one commit under: the count is the
    // service's own (a commit is a caller-sized document, so what one can ask
    // for is bounded), and the row only has to be past it.
    let past_the_bound = 1_001;
    let sort_order = json!({ "fields": [
        { "source-id": 1, "transform": "identity", "direction": "asc", "null-order": "nulls-first" },
    ]});

    let rows: Vec<(&str, Value)> = vec![
        (
            "more updates than one commit states",
            json!({ "updates": (0..past_the_bound)
                .map(|_| json!({ "action": "add-sort-order", "sort-order": sort_order }))
                .collect::<Vec<Value>>() }),
        ),
        (
            "more requirements than one commit states",
            json!({ "requirements": (0..past_the_bound)
                .map(|_| json!({ "type": "assert-table-uuid", "uuid": table_uuid(&created.table_arn) }))
                .collect::<Vec<Value>>() }),
        ),
        (
            "a sort order with more fields than one commit states",
            json!({ "updates": [
                { "action": "add-sort-order", "sort-order": { "fields": (0..past_the_bound)
                    .map(|_| json!({
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    }))
                    .collect::<Vec<Value>>() }},
            ]}),
        ),
        (
            "more properties than one commit sets",
            json!({ "updates": [
                { "action": "set-properties", "updates": (0..past_the_bound)
                    .map(|nth| (format!("key-{nth}"), Value::from("value")))
                    .collect::<serde_json::Map<String, Value>>() },
            ]}),
        ),
        (
            "more property removals than one commit states",
            json!({ "updates": [
                { "action": "remove-properties", "removals": (0..past_the_bound)
                    .map(|nth| Value::from(format!("key-{nth}")))
                    .collect::<Vec<Value>>() },
            ]}),
        ),
        (
            "a body that is not a JSON document at all",
            Value::from("not a commit"),
        ),
    ];

    for (case, body) in rows {
        let reply = emulator
            .commit_table(region, &created.bucket_arn, namespace, table, &body)
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected the refusal 400, got {status} {reply_body}",
            status = reply.status,
            reply_body = reply.body
        );
        assert_eq!(
            reply
                .body
                .get("error")
                .and_then(|error| error.get("type"))
                .and_then(Value::as_str),
            Some("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {reply_body}",
            reply_body = reply.body
        );
        // Nothing was stored: the table still carries the metadata file the
        // create wrote, and the document it had.
        let read = emulator
            .get_table(region, &created.bucket_arn, namespace, table)
            .await;
        assert_eq!(
            read.optional_text("metadataLocation").as_deref(),
            Some(created.metadata_location.as_str()),
            "case {case:?}: a refused commit must write no metadata file, got {actual:?}",
            actual = read.optional_text("metadataLocation")
        );
        let document = metadata(
            &emulator
                .load_table(region, &created.bucket_arn, namespace, table)
                .await,
            case,
        );
        assert_eq!(
            document.get("properties"),
            Some(&json!({})),
            "case {case:?}: a refused commit must set no property, got {actual:?}",
            actual = document.get("properties")
        );
        assert_eq!(
            document.get("default-sort-order-id").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: a refused commit must add no sort order, got {document}"
        );
    }
}

#[tokio::test]
async fn a_catalog_path_the_service_does_not_serve_is_left_to_the_model() {
    let emulator = Emulator::new();
    let region = "us-east-1";

    // The catalog endpoint this service serves is the TABLE endpoint; a path
    // beneath `/iceberg/v1` that is not one is not claimed here, so it is
    // answered the way the emulator answers a route no operation serves.
    for case in [
        "/iceberg/v1/config",
        "/iceberg/v1/arn%3Aaws%3As3tables%3Aus-east-1%3A000000000000%3Abucket%2Fb/namespaces",
    ] {
        let reply = emulator.call("GET", case, region, None).await;
        assert_eq!(
            reply.status, 501,
            "case {case:?}: this suite's rule does not cover {case}, so the emulator's \
             unserved-route answer stands, got {status} {body}",
            status = reply.status,
            body = reply.body
        );
    }

    // The table endpoint's own path is the one that is served, whatever the
    // table it names.
    let path = catalog_table_path(
        "arn:aws:s3tables:us-east-1:000000000000:bucket/b",
        "ns",
        "t",
    );
    let reply = emulator.call("GET", &path, region, None).await;
    assert_eq!(
        reply.status, 404,
        "case {path:?}: the table endpoint is served, so an absent table is a 404, got {status} \
         {body}",
        status = reply.status,
        body = reply.body
    );

    // A method of that endpoint this catalog does not serve says so, rather
    // than reporting the table it names as absent.
    let unserved = emulator.call("DELETE", &path, region, None).await;
    assert_eq!(
        unserved.status, 501,
        "case {path:?}: an unserved method of the table endpoint must say so, got {status} {body}",
        status = unserved.status,
        body = unserved.body
    );
    assert_eq!(
        unserved
            .body
            .get("error")
            .and_then(|error| error.get("type"))
            .and_then(Value::as_str),
        Some("NotImplementedException"),
        "case {path:?}: expected NotImplementedException, got {body}",
        body = unserved.body
    );
}
