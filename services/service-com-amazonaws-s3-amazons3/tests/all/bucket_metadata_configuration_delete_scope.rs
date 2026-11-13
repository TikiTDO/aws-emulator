// SPDX-License-Identifier: CC0-1.0

//! What `DeleteBucketMetadataConfiguration` reaches, and what it leaves alone.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate already owns), so what is pinned is the wire
//! behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rule under test: "Deletes an S3 Metadata configuration from a general purpose bucket"
//! (`DeleteBucketMetadataConfiguration`'s own documentation) — the call takes away the whole
//! metadata configuration subresource of the ONE bucket its `Bucket` label names, whichever
//! tables that configuration asked for, and nothing else: it answers the modelled 204 with an
//! empty body, a later `GetBucketMetadataConfiguration` on that bucket reports
//! `MetadataConfigurationNotFound` (404), every other bucket's configuration still reads back,
//! and the bucket and its objects are untouched. A bucket carrying no configuration — one that
//! never had one as much as one whose configuration was already taken away — has none to
//! delete and answers that same `MetadataConfigurationNotFound` (404), and a bucket this
//! emulator does not hold `NoSuchBucket` (404).
//!
//! [`super::bucket_metadata_configuration_deletion`] pins what a create finds afterwards (the
//! table names a delete does and does not release); this suite spans the configurations a
//! delete is asked for — which optional tables they carry, and whether the request names the
//! bucket's owner — and the state around them that must survive it.

use service_com_amazonaws_s3_amazons3::state::{
    METADATA_CONFIGURATION_NOT_FOUND_CODE, METADATA_CONFIGURATION_NOT_FOUND_MESSAGE,
    METADATA_CONFIGURATION_NOT_FOUND_STATUS, NO_SUCH_BUCKET_CODE,
};
use super::bucket_lifecycle::{create_bucket, head_bucket, router};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, inside, table_arn,
    text as element_of, JOURNAL_TABLE,
};
use super::bucket_metadata_configuration_deletion::{
    bucket_with_a_configuration, delete_metadata_configuration, payload,
};
use super::object_and_tagging_roundtrip::{get_object, put_object, text};

/// One row of the delete table: the case name, the bucket whose configuration the row
/// deletes, the bucket whose configuration must survive that delete together with that
/// bucket's own table namespace and the id its tables are named under, whether the deleted
/// configuration asked for an inventory table, whether it asked for an annotation table, and
/// whether the delete carries an `x-amz-expected-bucket-owner` naming the account that owns
/// the bucket.
///
/// The namespace and the id are spelled out rather than folded from the bucket name here, the
/// way [`super::bucket_metadata_configuration`] spells them out: a table of a general purpose
/// bucket's own namespace of the managed table bucket is `b_` and the bucket's name, and the
/// id its ARN carries is that written in the character set an S3 Tables name is written in.
struct Row {
    case: &'static str,
    bucket: &'static str,
    neighbour: &'static str,
    neighbour_namespace: &'static str,
    neighbour_namespace_id: &'static str,
    inventory: bool,
    annotation: bool,
    as_owner: bool,
}

/// The object a row writes into the bucket it then takes the configuration away from, so the
/// row reads back that the delete reached the subresource alone.
const KEY: &str = "ledger/entry.txt";

/// See [`KEY`] — the bytes that object carries.
const BODY: &[u8] = b"an object the metadata configuration knows nothing about";

/// What a read must report for the bucket whose configuration the delete did NOT name: its
/// own namespace of the managed table bucket, and its own journal table inside it.
///
/// The coordinates rather than the table's status, because the status a table reports depends
/// on whether its creation has fallen due, which is not what this suite is about: a
/// configuration still naming the tables of the bucket it belongs to is what shows the delete
/// did not reach past the bucket it named.
fn assert_the_neighbours_tables(case: &str, body: &str, row: &Row) {
    let destination = inside(body, "DestinationResult").unwrap_or_else(|| {
        panic!("case {case:?}: the read must report a DestinationResult, got {body:?}")
    });
    assert_eq!(
        element_of(destination, "TableNamespace").as_deref(),
        Some(row.neighbour_namespace),
        "case {case:?}: expected the other bucket's own table namespace {:?}, got {:?}",
        row.neighbour_namespace,
        element_of(destination, "TableNamespace")
    );
    let journal = inside(body, "JournalTableConfigurationResult").unwrap_or_else(|| {
        panic!("case {case:?}: the read must report a journal table, got {body:?}")
    });
    assert_eq!(
        element_of(journal, "TableName").as_deref(),
        Some(JOURNAL_TABLE),
        "case {case:?}: expected the journal TableName {JOURNAL_TABLE:?}, got {:?}",
        element_of(journal, "TableName")
    );
    assert_eq!(
        element_of(journal, "TableArn"),
        Some(table_arn(row.neighbour_namespace_id, JOURNAL_TABLE)),
        "case {case:?}: expected the journal TableArn {:?}, got {:?}",
        table_arn(row.neighbour_namespace_id, JOURNAL_TABLE),
        element_of(journal, "TableArn")
    );
}

#[tokio::test]
async fn a_delete_takes_the_named_buckets_configuration_and_leaves_everything_else() {
    // The code and the sentence the rows compare against are the service's published ones,
    // pinned here as text so the table is not comparing a constant with itself.
    assert_eq!(
        METADATA_CONFIGURATION_NOT_FOUND_CODE, "MetadataConfigurationNotFound",
        "the code a bucket carrying no metadata configuration is answered with"
    );
    assert_eq!(
        METADATA_CONFIGURATION_NOT_FOUND_MESSAGE, "The metadata configuration was not found",
        "the sentence that code carries"
    );

    // The rows differ along the configuration the delete is asked for — the required journal
    // table alone, and each combination of the two optional tables beside it — and along
    // whether the request names the bucket's owner, which an owner that IS the account's does
    // not change.
    let rows = [
        Row {
            case: "a journal table alone",
            bucket: "metadata-scope-journal",
            neighbour: "metadata-scope-journal-keep",
            neighbour_namespace: "b_metadata-scope-journal-keep",
            neighbour_namespace_id: "b_metadata_scope_journal_keep",
            inventory: false,
            annotation: false,
            as_owner: false,
        },
        Row {
            case: "a journal and an inventory table, deleted as the owner",
            bucket: "metadata-scope-inventory",
            neighbour: "metadata-scope-inventory-keep",
            neighbour_namespace: "b_metadata-scope-inventory-keep",
            neighbour_namespace_id: "b_metadata_scope_inventory_keep",
            inventory: true,
            annotation: false,
            as_owner: true,
        },
        Row {
            case: "a journal and an annotation table",
            bucket: "metadata-scope-annotation",
            neighbour: "metadata-scope-annotation-keep",
            neighbour_namespace: "b_metadata-scope-annotation-keep",
            neighbour_namespace_id: "b_metadata_scope_annotation_keep",
            inventory: false,
            annotation: true,
            as_owner: false,
        },
        Row {
            case: "all three tables, deleted as the owner",
            bucket: "metadata-scope-every-table",
            neighbour: "metadata-scope-every-table-keep",
            neighbour_namespace: "b_metadata-scope-every-table-keep",
            neighbour_namespace_id: "b_metadata_scope_every_table_keep",
            inventory: true,
            annotation: true,
            as_owner: true,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        // No wait for the tables to fall due: a configuration is the bucket's from the instant
        // the create records it, and this suite is about the configuration being taken away
        // rather than about the tables it provisioned — which
        // [`super::bucket_metadata_configuration_deletion`] and
        // [`super::bucket_metadata_unprovisioned_table_release`] pin.
        bucket_with_a_configuration(&router, row.bucket, row.inventory, row.annotation).await;
        bucket_with_a_configuration(&router, row.neighbour, row.inventory, row.annotation).await;
        let written = put_object(&router, row.bucket, KEY, BODY, &[]).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the object must be written first, got {} with body {:?}",
            written.status,
            text(&written)
        );

        // The configuration is there to be deleted.
        let before = get_metadata_configuration(&router, row.bucket, &[]).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the configuration must read back before the delete, got {} with \
             body {:?}",
            before.status,
            text(&before)
        );

        let owner = engine::emulator_account_id();
        let headers: Vec<(&str, &str)> = match row.as_owner {
            false => Vec::new(),
            true => vec![("x-amz-expected-bucket-owner", owner)],
        };
        let deleted = delete_metadata_configuration(&router, row.bucket, &headers).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: the delete must answer the modelled 204, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );
        assert!(
            deleted.body.is_empty(),
            "case {case:?}: a 204 carries no body, got {:?}",
            text(&deleted)
        );

        // The whole subresource went, whichever tables it carried.
        let after = get_metadata_configuration(&router, row.bucket, &[]).await;
        assert_eq!(
            after.status, METADATA_CONFIGURATION_NOT_FOUND_STATUS,
            "case {case:?}: a read after the delete must answer \
             {METADATA_CONFIGURATION_NOT_FOUND_STATUS}, got {} with body {:?}",
            after.status,
            text(&after)
        );
        assert_eq!(
            after.error_code(),
            METADATA_CONFIGURATION_NOT_FOUND_CODE,
            "case {case:?}: expected the {METADATA_CONFIGURATION_NOT_FOUND_CODE} code, got {:?}",
            after.error_code()
        );

        // And a second delete has none to take: the delete is not idempotent.
        let again = delete_metadata_configuration(&router, row.bucket, &headers).await;
        assert_eq!(
            again.status, METADATA_CONFIGURATION_NOT_FOUND_STATUS,
            "case {case:?}: a second delete must answer \
             {METADATA_CONFIGURATION_NOT_FOUND_STATUS}, got {} with body {:?}",
            again.status,
            text(&again)
        );
        assert_eq!(
            again.error_code(),
            METADATA_CONFIGURATION_NOT_FOUND_CODE,
            "case {case:?}: expected the {METADATA_CONFIGURATION_NOT_FOUND_CODE} code, got {:?}",
            again.error_code()
        );
        assert_eq!(
            again.error_message(),
            METADATA_CONFIGURATION_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the published sentence, got {:?}",
            again.error_message()
        );
        assert_eq!(
            again.element("BucketName").as_deref(),
            Some(row.bucket),
            "case {case:?}: the refusal names the bucket whose configuration is not there, got \
             {:?}",
            again.element("BucketName")
        );

        // The delete reached the subresource ALONE: the bucket is still there, so is its
        // object, and so is the other bucket's configuration.
        let head = head_bucket(&router, row.bucket).await;
        assert_eq!(
            head.status, 200,
            "case {case:?}: the bucket must survive the delete, got {}",
            head.status
        );
        let read = get_object(&router, row.bucket, KEY, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the object must survive the delete, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body, BODY,
            "case {case:?}: the object's bytes must be unchanged, got {:?}",
            text(&read)
        );
        let neighbour = get_metadata_configuration(&router, row.neighbour, &[]).await;
        assert_eq!(
            neighbour.status, 200,
            "case {case:?}: the other bucket's configuration must survive, got {} with body {:?}",
            neighbour.status,
            text(&neighbour)
        );
        assert_the_neighbours_tables(case, &text(&neighbour), &row);

        // A create is accepted again, and the bucket carries a configuration again: the delete
        // really changed the state a later call reads, rather than only answering 204.
        let recreated =
            create_metadata_configuration(&router, row.bucket, &[], &payload(false, false)).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: a create after the delete must be accepted, got {} with body {:?}",
            recreated.status,
            text(&recreated)
        );
        let reread = get_metadata_configuration(&router, row.bucket, &[]).await;
        assert_eq!(
            reread.status, 200,
            "case {case:?}: the bucket must carry a configuration again, got {} with body {:?}",
            reread.status,
            text(&reread)
        );
    }
}

#[tokio::test]
async fn a_delete_addressed_where_there_is_no_configuration_to_take_says_so() {
    let router = router();

    // Buckets this emulator HOLDS that never carried a configuration: the delete has none to
    // take, so it answers the configuration's own not-there condition and names the bucket.
    // The names differ along length and along the separators a bucket name may carry, so the
    // row is not one bucket name's behaviour.
    for bucket in [
        "metadata-none",
        "metadata.none.dotted",
        // Three characters: the shortest name a bucket may carry.
        "mnn",
        "metadata-none-with-a-rather-longer-name-123",
    ] {
        let created = create_bucket(&router, bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {bucket:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let refused = delete_metadata_configuration(&router, bucket, &[]).await;
        assert_eq!(
            refused.status, METADATA_CONFIGURATION_NOT_FOUND_STATUS,
            "case {bucket:?}: a bucket carrying no configuration must answer \
             {METADATA_CONFIGURATION_NOT_FOUND_STATUS}, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            METADATA_CONFIGURATION_NOT_FOUND_CODE,
            "case {bucket:?}: expected the {METADATA_CONFIGURATION_NOT_FOUND_CODE} code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            METADATA_CONFIGURATION_NOT_FOUND_MESSAGE,
            "case {bucket:?}: expected the published sentence, got {:?}",
            refused.error_message()
        );
        assert_eq!(
            refused.element("BucketName").as_deref(),
            Some(bucket),
            "case {bucket:?}: the refusal names the bucket, got {:?}",
            refused.element("BucketName")
        );
        // And nothing was created on the way: the bucket still carries no configuration.
        assert_eq!(
            get_metadata_configuration(&router, bucket, &[]).await.status,
            METADATA_CONFIGURATION_NOT_FOUND_STATUS,
            "case {bucket:?}: the bucket must still carry no configuration"
        );
    }

    // A bucket this emulator does not hold is the bucket's own not-there condition instead,
    // whatever the name looks like.
    for bucket in ["metadata-absent", "metadata.absent.dotted", "maa"] {
        let refused = delete_metadata_configuration(&router, bucket, &[]).await;
        assert_eq!(
            refused.status, 404,
            "case {bucket:?}: a bucket this emulator does not hold must answer 404, got {} with \
             body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            NO_SUCH_BUCKET_CODE,
            "case {bucket:?}: expected the {NO_SUCH_BUCKET_CODE} code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.element("BucketName").as_deref(),
            Some(bucket),
            "case {bucket:?}: the refusal names the bucket, got {:?}",
            refused.element("BucketName")
        );
    }
}
