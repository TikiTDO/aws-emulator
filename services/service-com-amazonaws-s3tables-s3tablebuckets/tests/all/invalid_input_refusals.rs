// SPDX-License-Identifier: CC0-1.0

//! How S3 Tables refuses a request it will not act on, and what it leaves
//! behind when it does.
//!
//! Three rules are under test, each asserted over a table that spans the class
//! it generalises over rather than one recorded request:
//!
//! * a value outside the modelled `OpenTableFormat` enum is refused with the
//!   service's own fixed sentence for that member class, whatever the spelling
//!   that missed the set;
//! * a request body this service cannot read into the operation's modelled
//!   input — a member carrying a value of a JSON type the model does not declare
//!   for it — is refused with the service's fixed invalid-input sentence under
//!   `BadRequestException`, the one client-input error this model declares, for
//!   every operation that takes a body. A member that was READ and then broke a
//!   modelled rule is the OTHER class and keeps its own wording, which the
//!   control rows pin;
//! * `metadata` that does not describe a table this service can create is
//!   refused with the service's fixed metadata sentence BEFORE anything is
//!   written, so the create reserves no name: the same request repeated is
//!   refused the same way, and the name is still free for a create that carries
//!   a document the service can act on.
//!
//! Every refusal is asserted as the WHOLE sentence, which is what proves nothing
//! from the request — and nothing of this emulator's codec — is echoed back.

use serde_json::{json, Value};

use crate::harness::{encode, expected_bucket_arn, Emulator};

/// The region every case works in.
const REGION: &str = "us-east-1";

/// The one client-input error this service's model declares.
const BAD_REQUEST: &str = "BadRequestException";

/// The fixed sentences, spelled out here rather than read from the crate, so
/// this suite pins the wording itself.
const INVALID_OPEN_TABLE_FORMAT: &str = "The specified open table format is not valid";
const INVALID_METADATA: &str = "The specified metadata is not valid";
const INVALID_INPUT: &str = "Invalid input. Consult the S3 Tables and/or Iceberg REST Catalog API \
                             reference for constraints on request parameters.";
const NO_TABLE: &str = "The specified table does not exist.";

/// A table bucket with one namespace in it, for the cases that get as far as a
/// create. Answers the bucket's ARN and the namespace's name.
async fn bucket_with_namespace(emulator: &Emulator, bucket: &str, namespace: &str) -> String {
    let created = emulator
        .create_table_bucket(REGION, &json!({ "name": bucket }))
        .await;
    assert_eq!(
        created.status, 200,
        "the table bucket must be created, got {status} {body}",
        status = created.status,
        body = created.body
    );
    let arn = created.text("arn");
    let namespace = emulator
        .create_namespace(REGION, &arn, &json!({ "namespace": [namespace] }))
        .await;
    assert_eq!(
        namespace.status, 200,
        "the namespace must be created, got {status} {body}",
        status = namespace.status,
        body = namespace.body
    );
    arn
}

#[tokio::test]
async fn an_open_table_format_outside_the_modelled_enum_is_refused_with_the_services_sentence() {
    let emulator = Emulator::new();
    let arn = bucket_with_namespace(&emulator, "format-refusals", "resident").await;

    // The spellings that miss the modelled value set: the one member's own
    // casing variants, the name of another open table format, a value with
    // trailing space, an empty value, and a comma-joined pair. `OpenTableFormat`
    // declares exactly one value (`smithy_query(query="shape",
    // shape="OpenTableFormat")`), so each of these is outside it.
    let outside = [
        "iceberg",
        "Iceberg",
        "ICEBERG ",
        " ICEBERG",
        "PARQUET",
        "DELTA",
        "HUDI",
        "",
        "ICEBERG,ICEBERG",
    ];

    for (index, format) in outside.iter().enumerate() {
        let refused = emulator
            .create_table(
                REGION,
                &arn,
                "resident",
                &json!({ "name": format!("refused_{index}"), "format": format }),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {format:?}: expected status 400, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some(BAD_REQUEST),
            "case {format:?}: expected error type {BAD_REQUEST}, got {actual:?}",
            actual = refused.error_code
        );
        assert_eq!(
            refused.message(),
            INVALID_OPEN_TABLE_FORMAT,
            "case {format:?}: expected the service's fixed sentence, got {actual:?}",
            actual = refused.message()
        );
        // A refused format creates nothing.
        let read = emulator
            .get_table(REGION, &arn, "resident", &format!("refused_{index}"))
            .await;
        assert_eq!(
            read.status, 404,
            "case {format:?}: a refused create must leave no table, got {status} {body}",
            status = read.status,
            body = read.body
        );
    }

    // The one value the enum declares is served, so the refusals above are of
    // the values outside the set and not of the member.
    let created = emulator
        .create_table(
            REGION,
            &arn,
            "resident",
            &json!({ "name": "accepted", "format": "ICEBERG" }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"ICEBERG\": the modelled value must be served, got {status} {body}",
        status = created.status,
        body = created.body
    );

    // A create that names NO format names no format the service has either, and
    // the model makes the member `@required`, so it is the same refusal: the
    // sentence states the class rather than which way the request missed it.
    let refused = emulator
        .create_table(REGION, &arn, "resident", &json!({ "name": "absent_format" }))
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"no format member at all\": expected status 400, got {status} {body}",
        status = refused.status,
        body = refused.body
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some(BAD_REQUEST),
        "case \"no format member at all\": expected error type {BAD_REQUEST}, got {actual:?}",
        actual = refused.error_code
    );
    assert_eq!(
        refused.message(),
        INVALID_OPEN_TABLE_FORMAT,
        "case \"no format member at all\": expected the service's fixed sentence, got {actual:?}",
        actual = refused.message()
    );
}

/// One request whose body the service cannot read: how it is addressed, and
/// which member carries the value of the wrong JSON type.
struct UnreadableCase {
    label: &'static str,
    method: &'static str,
    path: String,
    body: Value,
}

/// A body of the wrong JSON type for every member class the model declares — a
/// list, a map, a string, a structure, a nested structure — reached through
/// every shape of operation that takes a body.
fn unreadable_cases(arn: &str) -> Vec<UnreadableCase> {
    let encoded = encode(arn);
    vec![
        UnreadableCase {
            label: "CreateNamespace with the namespace list sent as a string",
            method: "PUT",
            path: format!("/namespaces/{encoded}"),
            body: json!({ "namespace": "resident" }),
        },
        UnreadableCase {
            label: "CreateNamespace with the namespace list sent as a number",
            method: "PUT",
            path: format!("/namespaces/{encoded}"),
            body: json!({ "namespace": 7 }),
        },
        UnreadableCase {
            label: "CreateNamespace with the namespace list sent as an object",
            method: "PUT",
            path: format!("/namespaces/{encoded}"),
            body: json!({ "namespace": { "name": "resident" } }),
        },
        UnreadableCase {
            label: "CreateNamespace with a namespace element sent as a list",
            method: "PUT",
            path: format!("/namespaces/{encoded}"),
            body: json!({ "namespace": [["resident"]] }),
        },
        UnreadableCase {
            label: "CreateTableBucket with the name sent as a number",
            method: "PUT",
            path: "/buckets".to_string(),
            body: json!({ "name": 7 }),
        },
        UnreadableCase {
            label: "CreateTableBucket with the encryption configuration sent as a string",
            method: "PUT",
            path: "/buckets".to_string(),
            body: json!({ "name": "unreadable-body", "encryptionConfiguration": "AES256" }),
        },
        UnreadableCase {
            label: "CreateTable with the tag map sent as a list",
            method: "PUT",
            path: format!("/tables/{encoded}/resident"),
            body: json!({ "name": "unreadable", "format": "ICEBERG", "tags": ["owner"] }),
        },
        UnreadableCase {
            label: "CreateTable with the metadata union sent as a string",
            method: "PUT",
            path: format!("/tables/{encoded}/resident"),
            body: json!({ "name": "unreadable", "format": "ICEBERG", "metadata": "iceberg" }),
        },
        UnreadableCase {
            label: "CreateTable with a nested schema field list sent as a string",
            method: "PUT",
            path: format!("/tables/{encoded}/resident"),
            body: json!({
                "name": "unreadable",
                "format": "ICEBERG",
                "metadata": { "iceberg": { "schema": { "fields": "id" } } },
            }),
        },
        UnreadableCase {
            label: "CreateTable with a nested schema field id sent as a string",
            method: "PUT",
            path: format!("/tables/{encoded}/resident"),
            body: json!({
                "name": "unreadable",
                "format": "ICEBERG",
                "metadata": {
                    "iceberg": {
                        "schema": { "fields": [{ "id": "one", "name": "id", "type": "long" }] },
                    },
                },
            }),
        },
        UnreadableCase {
            label: "TagResource with the tag map sent as a string",
            method: "POST",
            path: format!("/tag/{encoded}"),
            body: json!({ "tags": "owner" }),
        },
        UnreadableCase {
            label: "PutTableBucketReplication with the configuration sent as a string",
            method: "PUT",
            path: format!("/table-bucket-replication?tableBucketARN={encoded}"),
            body: json!({ "configuration": "enabled" }),
        },
        UnreadableCase {
            label: "PutTableBucketReplication with the rule list sent as an object",
            method: "PUT",
            path: format!("/table-bucket-replication?tableBucketARN={encoded}"),
            body: json!({
                "configuration": {
                    "role": "arn:aws:iam::000000000000:role/replication",
                    "rules": { "destinations": [] },
                },
            }),
        },
        UnreadableCase {
            label: "PutTableBucketStorageClass with the configuration sent as a string",
            method: "PUT",
            path: format!("/buckets/{encoded}/storage-class"),
            body: json!({ "storageClassConfiguration": "STANDARD" }),
        },
        UnreadableCase {
            label: "PutTableMaintenanceConfiguration with the value sent as a string",
            method: "PUT",
            path: format!("/tables/{encoded}/resident/resident/maintenance/icebergCompaction"),
            body: json!({ "value": "enabled" }),
        },
        UnreadableCase {
            label: "PutTableBucketPolicy with the policy sent as a number",
            method: "PUT",
            path: format!("/buckets/{encoded}/policy"),
            body: json!({ "resourcePolicy": 7 }),
        },
        UnreadableCase {
            label: "RenameTable with the new name sent as a number",
            method: "PUT",
            path: format!("/tables/{encoded}/resident/resident/rename"),
            body: json!({ "newName": 7 }),
        },
        UnreadableCase {
            label: "UpdateTableMetadataLocation with the location sent as a list",
            method: "PUT",
            path: format!("/tables/{encoded}/resident/resident/metadata-location"),
            body: json!({ "metadataLocation": ["s3://warehouse/metadata.json"], "versionToken": "0" }),
        },
    ]
}

#[tokio::test]
async fn a_body_this_service_cannot_read_is_refused_with_the_invalid_input_sentence() {
    let emulator = Emulator::new();
    // A well-formed ARN is enough: the body is read before anything is looked
    // up, so the refusal cannot depend on what the store holds.
    let arn = expected_bucket_arn(REGION, "unreadable-body");

    for case in unreadable_cases(&arn) {
        let refused = emulator
            .call(case.method, &case.path, REGION, Some(&case.body))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: expected status 400, got {status} {body}",
            label = case.label,
            status = refused.status,
            body = refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some(BAD_REQUEST),
            "case {label:?}: expected error type {BAD_REQUEST}, got {actual:?}",
            label = case.label,
            actual = refused.error_code
        );
        assert_eq!(
            refused.message(),
            INVALID_INPUT,
            "case {label:?}: expected the service's fixed invalid-input sentence, got {actual:?}",
            label = case.label,
            actual = refused.message()
        );
    }
}

#[tokio::test]
async fn a_body_that_reads_keeps_its_own_class_of_answer() {
    let emulator = Emulator::new();

    // The control rows: a request whose body READS is not this refusal, whether
    // it is then served or refused for a reason of its own. An absent
    // `@required` member is found while reading and is described per member, so
    // it keeps the clause that names the member rather than the invalid-input
    // sentence.
    let absent = emulator
        .create_table_bucket(REGION, &json!({}))
        .await;
    assert_eq!(
        absent.status, 400,
        "case \"an absent required member\": expected status 400, got {status} {body}",
        status = absent.status,
        body = absent.body
    );
    assert_ne!(
        absent.message(),
        INVALID_INPUT,
        "case \"an absent required member\": a member found absent while reading is described \
         per member, got {actual:?}",
        actual = absent.message()
    );
    assert!(
        absent.message().contains("name"),
        "case \"an absent required member\": the refusal must name the member, got {actual:?}",
        actual = absent.message()
    );

    // And a body of the right shape is served end to end.
    let arn = bucket_with_namespace(&emulator, "readable-body", "resident").await;
    let created = emulator
        .create_table(
            REGION,
            &arn,
            "resident",
            &json!({ "name": "readable", "format": "ICEBERG", "tags": { "owner": "platform" } }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"a body that reads\": the create must be served, got {status} {body}",
        status = created.status,
        body = created.body
    );
}

/// A version 2 column type nested `depth` `list` constructors deep: one
/// document of a shape the version 2 schema admits, at a depth the case chooses.
fn nested_type(depth: usize) -> Value {
    let mut document = json!("string");
    for level in 0..depth {
        document = json!({
            "type": "list",
            "element-id": level + 2,
            "element": document,
            "element-required": false,
        });
    }
    document
}

/// The metadata documents that do not describe a table this service can create.
///
/// The rules they break are Iceberg's own (the Apache Iceberg table
/// specification's type and transform sets, and the requirement that a
/// partition or sort field name a column the schema declares); the member names
/// are the model's wire names, which are hyphenated for the ones the model gives
/// a `jsonName` (`smithy_query(query="shape",
/// shape="IcebergSortOrder,IcebergSortField,IcebergPartitionSpec,IcebergPartitionField,IcebergSchemaV2")`).
fn metadata_that_describes_no_table() -> Vec<(&'static str, Value)> {
    vec![
        (
            // A version 2 column's type is a free-form document, so its depth
            // is the caller's; a document nested past what the service reads
            // one under describes no table it will create.
            "a version 2 column type nested deeper than the service reads one",
            json!({ "iceberg": { "schemaV2": {
                "type": "struct",
                "fields": [
                    { "id": 1, "name": "deep", "type": nested_type(64), "required": false },
                ],
            } } }),
        ),
        (
            "a column of a type no open table format has",
            json!({ "iceberg": { "schema": { "fields": [
                { "name": "id", "type": "not_a_type" },
            ] } } }),
        ),
        (
            "two columns of the same name",
            json!({ "iceberg": { "schema": { "fields": [
                { "name": "id", "type": "long" },
                { "name": "id", "type": "string" },
            ] } } }),
        ),
        (
            "two columns of the same stated id",
            json!({ "iceberg": { "schema": { "fields": [
                { "id": 1, "name": "id", "type": "long" },
                { "id": 1, "name": "label", "type": "string" },
            ] } } }),
        ),
        (
            "a column with no name",
            json!({ "iceberg": { "schema": { "fields": [
                { "name": "", "type": "long" },
            ] } } }),
        ),
        (
            "a decimal column wider than Iceberg's widest precision",
            json!({ "iceberg": { "schema": { "fields": [
                { "name": "amount", "type": "decimal(39, 2)" },
            ] } } }),
        ),
        (
            "a decimal column scaled past its own precision",
            json!({ "iceberg": { "schema": { "fields": [
                { "name": "amount", "type": "decimal(4, 9)" },
            ] } } }),
        ),
        (
            "a fixed column of no length",
            json!({ "iceberg": { "schema": { "fields": [
                { "name": "digest", "type": "fixed[0]" },
            ] } } }),
        ),
        (
            "a sort order naming a column the schema does not declare",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "id", "type": "long" }] },
                "writeOrder": { "order-id": 1, "fields": [
                    {
                        "source-id": 99,
                        "transform": "identity",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                ] },
            } }),
        ),
        (
            "a sort order naming a transform no open table format applies",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "id", "type": "long" }] },
                "writeOrder": { "order-id": 1, "fields": [
                    {
                        "source-id": 1,
                        "transform": "sorted",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                ] },
            } }),
        ),
        (
            "a sort order naming a bucket transform of no width",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "id", "type": "long" }] },
                "writeOrder": { "order-id": 1, "fields": [
                    {
                        "source-id": 1,
                        "transform": "bucket[0]",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                ] },
            } }),
        ),
        (
            "a sort order on a document that declares no schema",
            json!({ "iceberg": {
                "writeOrder": { "order-id": 1, "fields": [
                    {
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                ] },
            } }),
        ),
        (
            "a sort direction outside the modelled set",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "id", "type": "long" }] },
                "writeOrder": { "order-id": 1, "fields": [
                    {
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "ascending",
                        "null-order": "nulls-first",
                    },
                ] },
            } }),
        ),
        (
            "a partition spec naming a column the schema does not declare",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "at", "type": "timestamp" }] },
                "partitionSpec": { "fields": [
                    { "source-id": 42, "transform": "day", "name": "at_day" },
                ] },
            } }),
        ),
        (
            "a partition spec naming a transform no open table format applies",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "at", "type": "timestamp" }] },
                "partitionSpec": { "fields": [
                    { "source-id": 1, "transform": "bucketize", "name": "at_bucket" },
                ] },
            } }),
        ),
        (
            "a partition spec with two fields of the same name",
            json!({ "iceberg": {
                "schema": { "fields": [
                    { "name": "at", "type": "timestamp" },
                    { "name": "id", "type": "long" },
                ] },
                "partitionSpec": { "fields": [
                    { "source-id": 1, "transform": "day", "name": "part" },
                    { "source-id": 2, "transform": "identity", "name": "part" },
                ] },
            } }),
        ),
        (
            "a sort order sorting on one column twice the same way",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "id", "type": "long" }] },
                "writeOrder": { "order-id": 1, "fields": [
                    {
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                    {
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                ] },
            } }),
        ),
        (
            // Iceberg reserves order id zero for the unsorted order, so an
            // order that names a field cannot carry it.
            "a sort order naming a field under Iceberg's unsorted order id",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "id", "type": "long" }] },
                "writeOrder": { "order-id": 0, "fields": [
                    {
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                ] },
            } }),
        ),
        (
            "a partition spec partitioning one column twice the same way",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "at", "type": "timestamp" }] },
                "partitionSpec": { "fields": [
                    { "source-id": 1, "transform": "day", "name": "at_day" },
                    { "source-id": 1, "transform": "day", "name": "at_day_again" },
                ] },
            } }),
        ),
        (
            "both forms of the schema in one document",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "id", "type": "long" }] },
                "schemaV2": {
                    "type": "struct",
                    "fields": [{ "id": 1, "name": "id", "type": "long", "required": true }],
                },
            } }),
        ),
        (
            "a version 2 schema naming an identifier column it does not declare",
            json!({ "iceberg": { "schemaV2": {
                "type": "struct",
                "fields": [{ "id": 1, "name": "id", "type": "long", "required": true }],
                "identifier-field-ids": [4],
            } } }),
        ),
        (
            "a version 2 schema column of a type no open table format has",
            json!({ "iceberg": { "schemaV2": {
                "type": "struct",
                "fields": [{ "id": 1, "name": "id", "type": "quantity", "required": true }],
            } } }),
        ),
    ]
}

/// The metadata documents that DO describe a table, so the refusals above are
/// of the documents and not of the member.
fn metadata_that_describes_a_table() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one column",
            json!({ "iceberg": { "schema": { "fields": [
                { "name": "id", "type": "long", "required": true },
            ] } } }),
        ),
        (
            // A table created with no columns at all is one the recorded
            // service serves: the columns are added later by whatever writes
            // to the table.
            "a schema declaring no column",
            json!({ "iceberg": { "schema": { "fields": [] } } }),
        ),
        (
            "a sort order that names no field, on a schema with no column",
            json!({ "iceberg": {
                "schema": { "fields": [] },
                "writeOrder": { "order-id": 0, "fields": [] },
            } }),
        ),
        (
            "a partition spec that partitions nothing",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "at", "type": "timestamp" }] },
                "partitionSpec": { "fields": [] },
            } }),
        ),
        (
            "the parameterized and nested column types",
            json!({ "iceberg": { "schema": { "fields": [
                { "name": "amount", "type": "decimal(10, 2)" },
                { "name": "digest", "type": "fixed[16]" },
                { "name": "at", "type": "timestamptz" },
                { "name": "labels", "type": "list<string>" },
                { "name": "flag", "type": "boolean" },
            ] } } }),
        ),
        (
            "a sort order on a column the schema declares",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "id", "type": "long" }] },
                "writeOrder": { "order-id": 1, "fields": [
                    {
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                ] },
            } }),
        ),
        (
            "a sort order on an explicitly numbered column, bucketed",
            json!({ "iceberg": {
                "schema": { "fields": [{ "id": 7, "name": "key", "type": "string" }] },
                "writeOrder": { "order-id": 2, "fields": [
                    {
                        "source-id": 7,
                        "transform": "bucket[16]",
                        "direction": "desc",
                        "null-order": "nulls-last",
                    },
                ] },
            } }),
        ),
        (
            "an order that names no field, whatever its order id",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "id", "type": "long" }] },
                "writeOrder": { "order-id": 5, "fields": [] },
            } }),
        ),
        (
            "one column sorted on two ways",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "key", "type": "string" }] },
                "writeOrder": { "order-id": 3, "fields": [
                    {
                        "source-id": 1,
                        "transform": "truncate[2]",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                    {
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "desc",
                        "null-order": "nulls-last",
                    },
                ] },
            } }),
        ),
        (
            // The same column and transform, differing in direction and null
            // order: two sort keys, not one stated twice.
            "one column sorted on in both directions",
            json!({ "iceberg": {
                "schema": { "fields": [{ "name": "key", "type": "string" }] },
                "writeOrder": { "order-id": 4, "fields": [
                    {
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                    {
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "desc",
                        "null-order": "nulls-last",
                    },
                ] },
            } }),
        ),
        (
            // Some columns numbered by the request and the rest left to the
            // service: a sort order may name either, and the numbered column
            // does not collide with the position of the unnumbered one.
            "a schema numbering only some of its columns",
            json!({ "iceberg": {
                "schema": { "fields": [
                    { "name": "first", "type": "long" },
                    { "id": 1, "name": "second", "type": "string" },
                ] },
                "writeOrder": { "order-id": 1, "fields": [
                    {
                        "source-id": 1,
                        "transform": "identity",
                        "direction": "asc",
                        "null-order": "nulls-first",
                    },
                ] },
            } }),
        ),
        (
            "a partition spec and a sort order together",
            json!({ "iceberg": {
                "schema": { "fields": [
                    { "name": "at", "type": "timestamp" },
                    { "name": "id", "type": "long" },
                ] },
                "partitionSpec": {
                    "spec-id": 0,
                    "fields": [
                        { "source-id": 1, "transform": "day", "name": "at_day", "field-id": 1000 },
                    ],
                },
                "writeOrder": { "order-id": 1, "fields": [
                    {
                        "source-id": 2,
                        "transform": "truncate[4]",
                        "direction": "asc",
                        "null-order": "nulls-last",
                    },
                ] },
            } }),
        ),
        (
            "a version 2 schema with its identifier column",
            json!({ "iceberg": { "schemaV2": {
                "type": "struct",
                "schema-id": 0,
                "fields": [
                    { "id": 1, "name": "id", "type": "long", "required": true },
                    { "id": 2, "name": "label", "type": "string", "required": false },
                ],
                "identifier-field-ids": [1],
            } } }),
        ),
        (
            // The nesting a real Iceberg type reaches is well inside the depth
            // the service reads a type document under.
            "a version 2 column type nested a few levels deep",
            json!({ "iceberg": { "schemaV2": {
                "type": "struct",
                "fields": [
                    { "id": 1, "name": "nested", "type": nested_type(3), "required": false },
                ],
            } } }),
        ),
        (
            "table properties and no schema at all",
            json!({ "iceberg": { "properties": { "write.format.default": "parquet" } } }),
        ),
    ]
}

#[tokio::test]
async fn metadata_that_describes_no_table_is_refused_and_reserves_no_name() {
    let emulator = Emulator::new();
    let arn = bucket_with_namespace(&emulator, "metadata-refusals", "resident").await;

    for (index, (label, metadata)) in metadata_that_describes_no_table()
        .into_iter()
        .enumerate()
    {
        // Every case uses its OWN name, so the second create of that name is the
        // repeat and nothing else can have taken it.
        let name = format!("metadata_{index}");
        let request = json!({ "name": name, "format": "ICEBERG", "metadata": metadata });

        let refused = emulator.create_table(REGION, &arn, "resident", &request).await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: expected status 400, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some(BAD_REQUEST),
            "case {label:?}: expected error type {BAD_REQUEST}, got {actual:?}",
            actual = refused.error_code
        );
        assert_eq!(
            refused.message(),
            INVALID_METADATA,
            "case {label:?}: expected the service's fixed metadata sentence, got {actual:?}",
            actual = refused.message()
        );

        // Nothing was written: the name is not taken, so the identical request
        // is refused the same way rather than conflicting with a table the
        // first attempt should never have created.
        let repeat = emulator.create_table(REGION, &arn, "resident", &request).await;
        assert_eq!(
            repeat.status, 400,
            "case {label:?}: the repeat must be refused the same way, got {status} {body}",
            status = repeat.status,
            body = repeat.body
        );
        assert_eq!(
            repeat.message(),
            INVALID_METADATA,
            "case {label:?}: the repeat must carry the same sentence, got {actual:?}",
            actual = repeat.message()
        );
        let read = emulator.get_table(REGION, &arn, "resident", &name).await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: the refused create must leave no table, got {status} {body}",
            status = read.status,
            body = read.body
        );
        assert_eq!(
            read.message(),
            NO_TABLE,
            "case {label:?}: expected the not-found sentence, got {actual:?}",
            actual = read.message()
        );

        // And the name the refused creates asked for is still free: the same
        // name with a document the service can act on is served.
        let accepted = emulator
            .create_table(
                REGION,
                &arn,
                "resident",
                &json!({
                    "name": name,
                    "format": "ICEBERG",
                    "metadata": { "iceberg": { "schema": { "fields": [
                        { "name": "id", "type": "long", "required": true },
                    ] } } },
                }),
            )
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {label:?}: the name must still be free, got {status} {body}",
            status = accepted.status,
            body = accepted.body
        );
        let read = emulator.get_table(REGION, &arn, "resident", &name).await;
        assert_eq!(
            read.text("tableARN"),
            accepted.text("tableARN"),
            "case {label:?}: the read must report the table the create minted"
        );
    }
}

#[tokio::test]
async fn metadata_that_describes_a_table_is_served_and_the_table_reads_back() {
    let emulator = Emulator::new();
    let arn = bucket_with_namespace(&emulator, "metadata-accepted", "resident").await;

    for (index, (label, metadata)) in metadata_that_describes_a_table().into_iter().enumerate() {
        let name = format!("accepted_{index}");
        let created = emulator
            .create_table(
                REGION,
                &arn,
                "resident",
                &json!({ "name": name, "format": "ICEBERG", "metadata": metadata }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the create must be served, got {status} {body}",
            status = created.status,
            body = created.body
        );
        let read = emulator.get_table(REGION, &arn, "resident", &name).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the table must read back, got {status} {body}",
            status = read.status,
            body = read.body
        );
        assert_eq!(
            read.text("tableARN"),
            created.text("tableARN"),
            "case {label:?}: the read must report the table the create minted"
        );
        // A create that carries metadata writes the table's first metadata
        // file, so the stored table has a location to report.
        assert!(
            read.optional_text("metadataLocation").is_some(),
            "case {label:?}: a table created with metadata must report its metadata location, \
             got {body}",
            body = read.body
        );
    }
}
