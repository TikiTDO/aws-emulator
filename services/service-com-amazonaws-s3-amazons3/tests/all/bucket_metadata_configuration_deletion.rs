// SPDX-License-Identifier: CC0-1.0

//! Taking a bucket's S3 METADATA configuration away, and what a create finds afterwards.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate already owns), so what is pinned is the wire
//! behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rules under test:
//!
//! 1. `DeleteBucketMetadataConfiguration` "deletes an S3 Metadata configuration from a
//!    general purpose bucket" (that operation's own documentation): it answers the modelled
//!    204 with an empty body, a later `GetBucketMetadataConfiguration` reports
//!    `MetadataConfigurationNotFound` (404), and a SECOND delete answers that same condition
//!    — a bucket carrying no configuration has none to delete, so the delete is not
//!    idempotent. A bucket this emulator does not hold
//!    is `NoSuchBucket` (404) and an `x-amz-expected-bucket-owner` that is not the account's
//!    is `AccessDenied` (403).
//! 2. The delete takes ONLY the configuration: the tables it provisioned stay in the managed
//!    table bucket's namespace, which the published already-exists failure asks the caller to
//!    delete separately, so an identical create afterwards is accepted and reports its
//!    journal table `FAILED` with `JournalTableAlreadyExists`. The whole class is pinned by
//!    [`super::bucket_metadata_namespace_retention`]; this suite keeps the one row that
//!    follows the delete it is about.
//! 3. A create whose table name is one the namespace already holds — the namespace is one per
//!    Region, and two bucket names that differ only in the characters a table name cannot
//!    carry name the same table — is ACCEPTED with that table reporting `TableStatus`
//!    `FAILED` and the published code and sentence for the name being taken
//!    (`JournalTableAlreadyExists`, `InventoryTableAlreadyExists`), or
//!    `JournalTableNotAvailable` for an inventory table whose journal table could not be
//!    created ("an inventory table requires a journal table with an `ACTIVE` status", same
//!    published list).
//! 4. A delete releases NO name at all: deleting a configuration whose own table failed to be
//!    created leaves the name with the table that holds it, so the bucket whose table really
//!    holds that name still reports it in service and a create colliding with it is still
//!    answered the same failure.

use service_com_amazonaws_s3_amazons3::state::{
    METADATA_INVENTORY_TABLE_EXISTS_CODE, METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
    METADATA_JOURNAL_TABLE_EXISTS_CODE, METADATA_JOURNAL_TABLE_EXISTS_MESSAGE,
    METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE, METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
    METADATA_TABLE_STATUS_ACTIVE, METADATA_TABLE_STATUS_FAILED,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, inside, wait_for_the_tables,
    ANNOTATION_ROLE, NOT_THE_OWNER, XMLNS,
};
use super::object_and_tagging_roundtrip::text;

/// `DELETE /<bucket>?metadataConfiguration` — `DeleteBucketMetadataConfiguration`.
///
/// `pub(crate)`, like the two readers below it: the namespace-retention suite
/// ([`super::bucket_metadata_namespace_retention`]) drives the same subresource through the
/// same harness instead of standing up a second copy of it.
pub(crate) async fn delete_metadata_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/{bucket}?metadataConfiguration"),
        headers,
        "",
    )
    .await
}

/// A `MetadataConfiguration` container: the required journal table, and the optional
/// inventory and annotation tables when the case asks for them.
pub(crate) fn payload(inventory: bool, annotation: bool) -> String {
    let mut body = format!("<MetadataConfiguration xmlns=\"{XMLNS}\">");
    body.push_str(
        "<JournalTableConfiguration><RecordExpiration>\
         <Expiration>ENABLED</Expiration><Days>7</Days>\
         </RecordExpiration></JournalTableConfiguration>",
    );
    if inventory {
        body.push_str(
            "<InventoryTableConfiguration>\
             <ConfigurationState>ENABLED</ConfigurationState>\
             </InventoryTableConfiguration>",
        );
    }
    if annotation {
        body.push_str(&format!(
            "<AnnotationTableConfiguration>\
             <ConfigurationState>ENABLED</ConfigurationState><Role>{ANNOTATION_ROLE}</Role>\
             </AnnotationTableConfiguration>"
        ));
    }
    body.push_str("</MetadataConfiguration>");
    body
}

/// The `TableStatus`, `ErrorCode` and `ErrorMessage` a read reports for one table container
/// of the configuration.
pub(crate) fn reported(
    body: &str,
    container: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let Some(fragment) = inside(body, container) else {
        return (None, None, None);
    };
    // The reader unescapes the text, so a sentence carrying an apostrophe is compared as a
    // client's parser hands it over rather than as the XML spells it.
    let element = |name: &str| inside(fragment, name).map(super::bucket_tagging::unescaped);
    (
        element("TableStatus"),
        element("ErrorCode"),
        element("ErrorMessage"),
    )
}

/// A bucket carrying a metadata configuration with the tables the row asks for, whose tables
/// may still be being created.
///
/// The create step [`bucket_with_an_active_configuration`] waits on, on its own: a suite that
/// acts on the CONFIGURATION rather than on the tables it provisioned needs no wait, and
/// drives this rather than standing up a second copy of the create
/// ([`super::bucket_metadata_configuration_delete_scope`]).
pub(crate) async fn bucket_with_a_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    inventory: bool,
    annotation: bool,
) {
    let created = create_bucket(router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {bucket:?} must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let configured =
        create_metadata_configuration(router, bucket, &[], &payload(inventory, annotation)).await;
    assert_eq!(
        configured.status, 200,
        "the configuration of {bucket:?} must be created first, got {} with body {:?}",
        configured.status,
        text(&configured)
    );
}

/// A bucket carrying a metadata configuration whose tables are in service, so a row acts on
/// the state the API asks a caller to wait for.
pub(crate) async fn bucket_with_an_active_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    inventory: bool,
    annotation: bool,
) {
    bucket_with_a_configuration(router, bucket, inventory, annotation).await;
    wait_for_the_tables();
    let read = get_metadata_configuration(router, bucket, &[]).await;
    let body = text(&read);
    assert_eq!(
        reported(&body, "JournalTableConfigurationResult").0.as_deref(),
        Some(METADATA_TABLE_STATUS_ACTIVE),
        "the journal table must reach {METADATA_TABLE_STATUS_ACTIVE} before a row acts on it, \
         got {body:?}"
    );
}

#[tokio::test]
async fn a_deleted_configuration_is_gone_and_a_second_delete_says_there_is_none() {
    let router = router();
    bucket_with_an_active_configuration(&router, "metadata-delete", false, false).await;

    // While the configuration is there, a second create is refused rather than replacing it.
    let duplicate =
        create_metadata_configuration(&router, "metadata-delete", &[], &payload(false, false))
            .await;
    assert_eq!(
        duplicate.status, 409,
        "a second create while a configuration is active must be refused, got {} with body {:?}",
        duplicate.status,
        text(&duplicate)
    );
    assert_eq!(
        duplicate.error_code(),
        "MetadataConfigurationAlreadyExists",
        "expected the MetadataConfigurationAlreadyExists code, got {:?}",
        duplicate.error_code()
    );

    // The delete answers the modelled 204 with an empty body, and the configuration is gone.
    let deleted = delete_metadata_configuration(&router, "metadata-delete", &[]).await;
    assert_eq!(
        deleted.status, 204,
        "the delete must answer the modelled 204, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
    assert!(
        deleted.body.is_empty(),
        "a 204 carries no body, got {:?}",
        text(&deleted)
    );
    let absent = get_metadata_configuration(&router, "metadata-delete", &[]).await;
    assert_eq!(
        absent.status, 404,
        "a read of the deleted configuration must answer 404, got {} with body {:?}",
        absent.status,
        text(&absent)
    );
    assert_eq!(
        absent.error_code(),
        "MetadataConfigurationNotFound",
        "expected the MetadataConfigurationNotFound code, got {:?}",
        absent.error_code()
    );

    // NOT idempotent: there is no configuration left to delete, so a second delete answers
    // the metadata configuration's own not-there condition, the one a read answers.
    let again = delete_metadata_configuration(&router, "metadata-delete", &[]).await;
    assert_eq!(
        again.status, 404,
        "a second delete must answer 404, got {} with body {:?}",
        again.status,
        text(&again)
    );
    assert_eq!(
        again.error_code(),
        "MetadataConfigurationNotFound",
        "a second delete must name the configuration that is not there, got {:?}",
        again.error_code()
    );
    assert_eq!(
        again.error_message(),
        "The metadata configuration was not found",
        "the refusal carries the text S3 sends for that code"
    );
    assert_eq!(
        again.element("BucketName").as_deref(),
        Some("metadata-delete"),
        "the refusal names the bucket whose configuration is not there, got {:?}",
        again.element("BucketName")
    );
    assert_eq!(
        get_metadata_configuration(&router, "metadata-delete", &[])
            .await
            .status,
        404,
        "the bucket must still carry no configuration"
    );

    // The addresses the delete refuses: a bucket this emulator does not hold, and an
    // expected owner that is not the account's.
    for bucket in ["metadata-absent", "metadata", "metadata-delete-2"] {
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
            "NoSuchBucket",
            "case {bucket:?}: expected the NoSuchBucket code, got {:?}",
            refused.error_code()
        );
    }
    let denied = delete_metadata_configuration(
        &router,
        "metadata-delete",
        &[("x-amz-expected-bucket-owner", NOT_THE_OWNER)],
    )
    .await;
    assert_eq!(
        denied.status, 403,
        "an owner that is not the account's must answer 403, got {} with body {:?}",
        denied.status,
        text(&denied)
    );
    assert_eq!(
        denied.error_code(),
        "AccessDenied",
        "expected the AccessDenied code, got {:?}",
        denied.error_code()
    );
}

/// One row of the taken-name table: the case name, the bucket whose LIVE configuration holds
/// the folded table name, whether that configuration has an inventory table, the bucket whose
/// create then collides with it (a name that folds to the same table name), whether its create
/// asks for an inventory table, and the error code and sentence that inventory table must then
/// report (`None` for a create that names no inventory table).
type TakenName = (
    &'static str,
    &'static str,
    bool,
    &'static str,
    bool,
    Option<(&'static str, &'static str)>,
);

#[tokio::test]
async fn an_identical_create_after_a_delete_still_finds_its_table_names_taken() {
    let router = router();

    // Both optional tables as well as the journal one, so the names the delete left behind are
    // read back for every table the configuration provisioned rather than only the required
    // one.
    for (case, bucket, inventory) in [
        ("a journal table alone", "metadata-recreate-journal", false),
        (
            "a journal and an inventory table",
            "metadata-recreate-inventory",
            true,
        ),
    ] {
        bucket_with_an_active_configuration(&router, bucket, inventory, false).await;
        let deleted = delete_metadata_configuration(&router, bucket, &[]).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: the delete must succeed, got {} with body {:?}",
            deleted.status,
            text(&deleted)
        );

        // The identical create is ACCEPTED — the bucket carries a configuration again — but
        // the tables it would provision are still in the managed namespace, which the delete
        // did not take them out of.
        let recreated =
            create_metadata_configuration(&router, bucket, &[], &payload(inventory, false)).await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: a create after a delete must be accepted, got {} with body {:?}",
            recreated.status,
            text(&recreated)
        );

        let expected: Vec<(&str, &str, &str)> = match inventory {
            false => vec![(
                "JournalTableConfigurationResult",
                METADATA_JOURNAL_TABLE_EXISTS_CODE,
                METADATA_JOURNAL_TABLE_EXISTS_MESSAGE,
            )],
            true => vec![
                (
                    "JournalTableConfigurationResult",
                    METADATA_JOURNAL_TABLE_EXISTS_CODE,
                    METADATA_JOURNAL_TABLE_EXISTS_MESSAGE,
                ),
                (
                    "InventoryTableConfigurationResult",
                    METADATA_INVENTORY_TABLE_EXISTS_CODE,
                    METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
                ),
            ],
        };
        // Provisioning failed, so each table reports the published code and sentence for its
        // name being taken — before and after the span a successful creation would take, since
        // nothing is going to create it.
        for when in ["straight after the create", "after the activation span"] {
            if when == "after the activation span" {
                wait_for_the_tables();
            }
            let read = get_metadata_configuration(&router, bucket, &[]).await;
            let body = text(&read);
            for (container, expected_code, expected_message) in &expected {
                let (status, code, message) = reported(&body, container);
                assert_eq!(
                    status.as_deref(),
                    Some(METADATA_TABLE_STATUS_FAILED),
                    "case {case:?} ({when}): {container} must report \
                     {METADATA_TABLE_STATUS_FAILED}, got {body:?}"
                );
                assert_eq!(
                    code.as_deref(),
                    Some(*expected_code),
                    "case {case:?} ({when}): expected the {expected_code} error code, got \
                     {code:?}"
                );
                assert_eq!(
                    message.as_deref(),
                    Some(*expected_message),
                    "case {case:?} ({when}): expected the published sentence, got {message:?}"
                );
            }
        }

        // And the configuration is one the bucket carries again, so a second create is
        // refused for that.
        let duplicate =
            create_metadata_configuration(&router, bucket, &[], &payload(inventory, false)).await;
        assert_eq!(
            duplicate.error_code(),
            "MetadataConfigurationAlreadyExists",
            "case {case:?}: expected the MetadataConfigurationAlreadyExists code, got {:?}",
            duplicate.error_code()
        );
    }
}

#[tokio::test]
async fn a_create_whose_table_name_is_taken_reports_the_table_it_could_not_create() {
    let router = router();

    // Three pairs of buckets whose names fold to the same table names, so the rows differ
    // along which tables the LIVE configuration holds — which is what decides the error code
    // each table of the colliding create reports:
    //
    // * the journal table alone, and the colliding create names only a journal table: the
    //   journal name is taken, so it alone fails;
    // * the journal table alone, and the colliding create also enables an inventory table:
    //   the inventory table's own name is free, so it reports the journal table it relies on;
    // * the journal AND inventory tables, and the colliding create names both: each reports
    //   its own name being taken.
    let rows: [TakenName; 3] = [
        (
            "the journal table alone",
            "metadata-taken-journal",
            false,
            "metadata.taken.journal",
            false,
            None,
        ),
        (
            "an inventory table whose journal table failed",
            "metadata-taken-dependent",
            false,
            "metadata.taken.dependent",
            true,
            Some((
                METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE,
                METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
            )),
        ),
        (
            "an inventory table whose own name is taken",
            "metadata-taken-inventory",
            true,
            "metadata.taken.inventory",
            true,
            Some((
                METADATA_INVENTORY_TABLE_EXISTS_CODE,
                METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
            )),
        ),
    ];

    for (case, holder, holder_inventory, bucket, second_inventory, expected_inventory) in rows {
        bucket_with_an_active_configuration(&router, holder, holder_inventory, false).await;
        let created = create_bucket(&router, bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the colliding bucket must be created, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        // The create is ACCEPTED: the bucket carries a configuration, whatever its tables
        // could do.
        let colliding =
            create_metadata_configuration(&router, bucket, &[], &payload(second_inventory, false))
                .await;
        assert_eq!(
            colliding.status, 200,
            "case {case:?}: a create whose table name is taken must be accepted, got {} with \
             body {:?}",
            colliding.status,
            text(&colliding)
        );

        // Its journal table never reaches ACTIVE: another configuration's table holds that
        // name, so provisioning failed with the published code and sentence — before and
        // after the span a successful creation would take.
        for when in ["straight after the create", "after the activation span"] {
            if when == "after the activation span" {
                wait_for_the_tables();
            }
            let read = get_metadata_configuration(&router, bucket, &[]).await;
            let body = text(&read);
            let (status, code, message) = reported(&body, "JournalTableConfigurationResult");
            assert_eq!(
                status.as_deref(),
                Some(METADATA_TABLE_STATUS_FAILED),
                "case {case:?} ({when}): the journal table must report {METADATA_TABLE_STATUS_FAILED}, got {body:?}"
            );
            assert_eq!(
                code.as_deref(),
                Some(METADATA_JOURNAL_TABLE_EXISTS_CODE),
                "case {case:?} ({when}): expected the {METADATA_JOURNAL_TABLE_EXISTS_CODE} error code, got {code:?}"
            );
            assert_eq!(
                message.as_deref(),
                Some(METADATA_JOURNAL_TABLE_EXISTS_MESSAGE),
                "case {case:?} ({when}): expected the published sentence, got {message:?}"
            );
        }

        // What the inventory table reports, when the create asked for one.
        let read = get_metadata_configuration(&router, bucket, &[]).await;
        let body = text(&read);
        let (status, code, message) = reported(&body, "InventoryTableConfigurationResult");
        match expected_inventory {
            None => assert_eq!(
                status, None,
                "case {case:?}: a create naming no inventory table reports none, got {body:?}"
            ),
            Some((expected_code, expected_message)) => {
                assert_eq!(
                    status.as_deref(),
                    Some(METADATA_TABLE_STATUS_FAILED),
                    "case {case:?}: the inventory table must report {METADATA_TABLE_STATUS_FAILED}, got {body:?}"
                );
                assert_eq!(
                    code.as_deref(),
                    Some(expected_code),
                    "case {case:?}: expected the {expected_code} error code, got {code:?}"
                );
                assert_eq!(
                    message.as_deref(),
                    Some(expected_message),
                    "case {case:?}: expected the published sentence, got {message:?}"
                );
            }
        }

        // And the configuration that failed to provision is still a configuration the bucket
        // carries, so a second create is refused for that rather than retried.
        let duplicate =
            create_metadata_configuration(&router, bucket, &[], &payload(second_inventory, false))
                .await;
        assert_eq!(
            duplicate.error_code(),
            "MetadataConfigurationAlreadyExists",
            "case {case:?}: expected the MetadataConfigurationAlreadyExists code, got {:?}",
            duplicate.error_code()
        );

        // And deleting the configuration that FAILED to provision releases nothing: the name
        // belongs to the table the HOLDER created, so the holder still reports its journal
        // table in service and a third colliding create is still refused the name.
        let deleted = delete_metadata_configuration(&router, bucket, &[]).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: the delete of the colliding configuration must succeed, got {} \
             with body {:?}",
            deleted.status,
            text(&deleted)
        );
        let holder_body = text(&get_metadata_configuration(&router, holder, &[]).await);
        assert_eq!(
            reported(&holder_body, "JournalTableConfigurationResult")
                .0
                .as_deref(),
            Some(METADATA_TABLE_STATUS_ACTIVE),
            "case {case:?}: the holder's journal table must still be in service, got \
             {holder_body:?}"
        );
        let colliding_again =
            create_metadata_configuration(&router, bucket, &[], &payload(second_inventory, false))
                .await;
        assert_eq!(
            colliding_again.status, 200,
            "case {case:?}: the colliding create must be accepted again, got {} with body {:?}",
            colliding_again.status,
            text(&colliding_again)
        );
        let again = text(&get_metadata_configuration(&router, bucket, &[]).await);
        let (status, code, _) = reported(&again, "JournalTableConfigurationResult");
        assert_eq!(
            status.as_deref(),
            Some(METADATA_TABLE_STATUS_FAILED),
            "case {case:?}: the name is still held, so the journal table must report \
             {METADATA_TABLE_STATUS_FAILED}, got {again:?}"
        );
        assert_eq!(
            code.as_deref(),
            Some(METADATA_JOURNAL_TABLE_EXISTS_CODE),
            "case {case:?}: expected the {METADATA_JOURNAL_TABLE_EXISTS_CODE} error code, got \
             {code:?}"
        );
    }
}
