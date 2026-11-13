// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateBucketMetadataJournalTableConfiguration`: enabling and
//! disabling journal table record expiration on the S3 Metadata configuration a general
//! purpose bucket carries, read back through `GetBucketMetadataConfiguration`.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns, with the metadata-configuration suite's own
//! helpers for the create, the read and for slicing a response), so what is pinned is the
//! wire behaviour — status, body, error code, message — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * The operation "enables or disables journal table record expiration for an S3 Metadata
//!   configuration on a general purpose bucket" (its own documentation), and its payload
//!   `JournalTableConfigurationUpdates` carries `RecordExpiration` as its one required member
//!   (`smithy_query(query="shape",shape="JournalTableConfigurationUpdates")`): the settings
//!   the request carries REPLACE the ones the configuration carried, member for member, so a
//!   request naming no `Days` leaves the configuration naming none. The call answers the
//!   modelled 200 with an empty body.
//! * The update is scoped to that one setting: the journal TABLE itself — its name, status
//!   and ARN — and the inventory and annotation tables and the destination are reported
//!   exactly as they were before the call. No table is created or dropped by it, because
//!   `MetadataConfiguration$JournalTableConfiguration` is required and so every configuration
//!   already has a journal table.
//! * "Journal table records must be retained for a minimum of 7 days. To set this value,
//!   specify any whole number from `7` to `2147483647`" (`RecordExpiration$Days`): a day
//!   count outside that published range is refused with `InvalidArgument` (HTTP 400) and
//!   records nothing, and each published bound is accepted.
//! * A bucket carrying no metadata configuration has no journal table to expire records of,
//!   so the call answers `MetadataConfigurationNotFound` (HTTP 404); a bucket this emulator
//!   does not hold `NoSuchBucket` (HTTP 404), and an `x-amz-expected-bucket-owner` that is
//!   not the account's HTTP 403 `AccessDenied` — each with nothing recorded.

use service_com_amazonaws_s3_amazons3::state::INVALID_RECORD_EXPIRATION_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, inside, text, wait_for_the_tables,
    ANNOTATION_ROLE, NOT_THE_OWNER, XMLNS,
};

/// The fewest days journal table records may be retained for, and the most: the bounds
/// `RecordExpiration$Days` publishes ("any whole number from `7` to `2147483647`"), written
/// out here so the rows pin them by hand rather than by the code under test.
const MIN_DAYS: i64 = 7;
/// See [`MIN_DAYS`].
const MAX_DAYS: i64 = 2_147_483_647;

/// `PUT /<bucket>?metadataJournalTable` —
/// `UpdateBucketMetadataJournalTableConfiguration`, with `payload` as the
/// `JournalTableConfiguration` container and whatever headers the case carries.
async fn update_journal_table(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
    payload: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?metadataJournalTable"),
        headers,
        payload,
    )
    .await
}

/// The `JournalTableConfigurationUpdates` document naming `expiration`, with a `Days` beside
/// it exactly when the case names one.
fn updates(expiration: &str, days: Option<i64>) -> String {
    let days = match days {
        None => String::new(),
        Some(days) => format!("<Days>{days}</Days>"),
    };
    format!(
        "<JournalTableConfiguration xmlns=\"{XMLNS}\">\
         <RecordExpiration><Expiration>{expiration}</Expiration>{days}</RecordExpiration>\
         </JournalTableConfiguration>"
    )
}

/// The `MetadataConfiguration` every case starts from: a journal table retaining records for
/// `days` under `expiration`, an enabled inventory table and an annotation table managed by
/// an IAM role — so each row can show that the update moves the journal record expiration and
/// leaves the other two tables alone.
fn starting_configuration(expiration: &str, days: Option<i64>) -> String {
    let days = match days {
        None => String::new(),
        Some(days) => format!("<Days>{days}</Days>"),
    };
    format!(
        "<MetadataConfiguration xmlns=\"{XMLNS}\">\
         <JournalTableConfiguration><RecordExpiration>\
         <Expiration>{expiration}</Expiration>{days}\
         </RecordExpiration></JournalTableConfiguration>\
         <InventoryTableConfiguration>\
         <ConfigurationState>ENABLED</ConfigurationState>\
         </InventoryTableConfiguration>\
         <AnnotationTableConfiguration>\
         <ConfigurationState>ENABLED</ConfigurationState>\
         <Role>{ANNOTATION_ROLE}</Role>\
         </AnnotationTableConfiguration>\
         </MetadataConfiguration>"
    )
}

/// One row of the enable-and-disable table.
#[derive(Debug, Clone)]
struct Case {
    /// The case name every assertion carries.
    name: &'static str,
    /// The general purpose bucket the configuration lives on.
    bucket: &'static str,
    /// The `RecordExpiration$Expiration` the configuration was CREATED with.
    start_expiration: &'static str,
    /// The `RecordExpiration$Days` the create named, when it named one.
    start_days: Option<i64>,
    /// The `Expiration` the update names.
    expiration: &'static str,
    /// The `Days` the update names, when it names one.
    days: Option<i64>,
}

/// Every row: they differ along the settings the configuration started from, the state the
/// update names (enabled, disabled), whether either of them names a `Days` at all, and the
/// day count itself — each published bound of the range and a value well inside it.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a week's retention turned off, leaving no day count",
            bucket: "journal-update-disable",
            start_expiration: "ENABLED",
            start_days: Some(MIN_DAYS),
            expiration: "DISABLED",
            days: None,
        },
        Case {
            name: "expiration turned on for a year",
            bucket: "journal-update-enable-a-year",
            start_expiration: "DISABLED",
            start_days: None,
            expiration: "ENABLED",
            days: Some(365),
        },
        Case {
            name: "a year's retention shortened to the published minimum",
            bucket: "journal-update-shorten",
            start_expiration: "ENABLED",
            start_days: Some(365),
            expiration: "ENABLED",
            days: Some(MIN_DAYS),
        },
        Case {
            name: "a week's retention lengthened to the published maximum",
            bucket: "journal-update-lengthen",
            start_expiration: "ENABLED",
            start_days: Some(MIN_DAYS),
            expiration: "ENABLED",
            days: Some(MAX_DAYS),
        },
        Case {
            name: "expiration turned on with no day count named",
            bucket: "journal.update.2026",
            start_expiration: "DISABLED",
            start_days: None,
            expiration: "ENABLED",
            days: None,
        },
        Case {
            name: "a day count dropped while expiration stays on",
            bucket: "journal-update-drop-days",
            start_expiration: "ENABLED",
            start_days: Some(90),
            expiration: "ENABLED",
            days: None,
        },
        Case {
            name: "disabled expiration named again, unchanged",
            bucket: "journal-update-disabled-again",
            start_expiration: "DISABLED",
            start_days: None,
            expiration: "DISABLED",
            days: None,
        },
    ]
}

/// The `RecordExpiration` the read reports for the journal table of `body`.
fn reported_expiration<'a>(case_name: &str, body: &'a str) -> &'a str {
    let journal = inside(body, "JournalTableConfigurationResult").unwrap_or_else(|| {
        panic!(
            "case {case_name:?}: every configuration has a journal table, so the read must \
             report one, got {body:?}"
        )
    });
    inside(journal, "RecordExpiration").unwrap_or_else(|| {
        panic!(
            "case {case_name:?}: RecordExpiration is a required member of the journal result, \
             got {journal:?}"
        )
    })
}

/// That the read reports exactly the record expiration `expiration`/`days` name.
fn assert_expiration(case_name: &str, body: &str, expiration: &str, days: Option<i64>) {
    let reported = reported_expiration(case_name, body);
    assert_eq!(
        text(reported, "Expiration").as_deref(),
        Some(expiration),
        "case {case_name:?}: expected Expiration {expiration:?}, got {:?}",
        text(reported, "Expiration")
    );
    assert_eq!(
        text(reported, "Days"),
        days.map(|days| days.to_string()),
        "case {case_name:?}: expected Days {days:?}, got {:?}",
        text(reported, "Days")
    );
}

/// That everything the update is NOT about reads back exactly as it did before: the journal
/// table itself, the two optional tables and the destination.
fn assert_the_rest_is_untouched(case_name: &str, before: &str, after: &str) {
    for container in [
        "InventoryTableConfigurationResult",
        "AnnotationTableConfigurationResult",
        "DestinationResult",
    ] {
        assert_eq!(
            inside(after, container),
            inside(before, container),
            "case {case_name:?}: the update must leave {container} exactly as it was, got {:?} \
             where the read before it reported {:?}",
            inside(after, container),
            inside(before, container)
        );
    }
    let journal = |body: &str| {
        let journal = inside(body, "JournalTableConfigurationResult")
            .expect("the read reports a journal table");
        [
            text(journal, "TableName"),
            text(journal, "TableArn"),
            text(journal, "TableStatus"),
        ]
    };
    assert_eq!(
        journal(after),
        journal(before),
        "case {case_name:?}: the update changes the record expiration only, so the journal \
         table's own name, ARN and status must be reported as they were, got {:?} where the \
         read before it reported {:?}",
        journal(after),
        journal(before)
    );
}

#[tokio::test]
async fn an_update_replaces_the_journal_record_expiration_and_nothing_else() {
    let router = router();

    for case in cases() {
        create_bucket(&router, case.bucket, None, None).await;
        let created = create_metadata_configuration(
            &router,
            case.bucket,
            &[],
            &starting_configuration(case.start_expiration, case.start_days),
        )
        .await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: the configuration this row starts from must be created, got {} with \
             body {:?}",
            case.name,
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        // The tables the CREATE asked for are allowed to reach their terminal status first,
        // so the status the read reports after the update is stable and this row pins the
        // update leaving it alone rather than the create's window still being open.
        wait_for_the_tables();
        let before = get_metadata_configuration(&router, case.bucket, &[]).await;
        let before = String::from_utf8_lossy(&before.body).to_string();
        assert_expiration(case.name, &before, case.start_expiration, case.start_days);

        let answer =
            update_journal_table(&router, case.bucket, &[], &updates(case.expiration, case.days))
                .await;
        assert_eq!(
            answer.status,
            200,
            "case {:?}: expected the modelled 200, got {} with body {:?}",
            case.name,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.body.is_empty(),
            "case {:?}: the update models a Unit output, so its body must be empty, got {:?}",
            case.name,
            String::from_utf8_lossy(&answer.body)
        );

        let reading = get_metadata_configuration(&router, case.bucket, &[]).await;
        assert_eq!(
            reading.status,
            200,
            "case {:?}: expected the read to answer 200, got {} with body {:?}",
            case.name,
            reading.status,
            String::from_utf8_lossy(&reading.body)
        );
        let after = String::from_utf8_lossy(&reading.body).to_string();
        assert_expiration(case.name, &after, case.expiration, case.days);
        assert_the_rest_is_untouched(case.name, &before, &after);

        // The settings the update recorded are what every later read reports, so the update
        // is a write to the store rather than an answer computed for the one call.
        let again = get_metadata_configuration(&router, case.bucket, &[]).await;
        let again = String::from_utf8_lossy(&again.body).to_string();
        assert_expiration(case.name, &again, case.expiration, case.days);
    }
}

#[tokio::test]
async fn a_day_count_outside_the_published_range_is_refused_and_records_nothing() {
    let router = router();
    let bucket = "journal-update-day-range";
    create_bucket(&router, bucket, None, None).await;
    create_metadata_configuration(&router, bucket, &[], &starting_configuration("ENABLED", Some(30)))
        .await;
    wait_for_the_tables();

    // One past the published minimum, and the two values below it a caller can send. One past
    // the published MAXIMUM is not expressible: `Days` targets `RecordExpirationDays`, a
    // Smithy `integer` (`smithy_query(query="shape",shape="RecordExpirationDays")`), and
    // 2147483647 is the largest value that type carries — so the bound itself is what the
    // accepted rows below pin at that end.
    for days in [MIN_DAYS - 1, 0, -1] {
        let before = get_metadata_configuration(&router, bucket, &[]).await;
        let before = String::from_utf8_lossy(&before.body).to_string();
        let refused = update_journal_table(&router, bucket, &[], &updates("ENABLED", Some(days)))
            .await;
        assert_eq!(
            refused.status,
            400,
            "case {days:?}: a day count outside the published range is a 400, got {} with body \
             {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "InvalidRequest",
            "case {days:?}: a record expiration that states no usable retention period is an \
             InvalidRequest, got {:?}",
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_message(),
            INVALID_RECORD_EXPIRATION_MESSAGE,
            "case {days:?}: the refusal carries that class's fixed sentence, got {:?}",
            String::from_utf8_lossy(&refused.body)
        );
        let unchanged = get_metadata_configuration(&router, bucket, &[]).await;
        assert_eq!(
            String::from_utf8_lossy(&unchanged.body).to_string(),
            before,
            "case {days:?}: a refused update must leave the stored configuration exactly as it \
             was"
        );
    }

    // Each published bound of the same range is accepted, so what is pinned is the range and
    // not the member.
    for days in [MIN_DAYS, MAX_DAYS] {
        let accepted = update_journal_table(&router, bucket, &[], &updates("ENABLED", Some(days)))
            .await;
        assert_eq!(
            accepted.status,
            200,
            "case {days:?}: a published bound of the range must be accepted, got {} with body \
             {:?}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
        let reading = get_metadata_configuration(&router, bucket, &[]).await;
        let body = String::from_utf8_lossy(&reading.body).to_string();
        assert_expiration(&format!("{days} days"), &body, "ENABLED", Some(days));
    }
}

#[tokio::test]
async fn an_update_naming_an_address_or_an_owner_this_emulator_does_not_hold_says_which() {
    let router = router();
    let enable = updates("ENABLED", Some(MIN_DAYS));

    // A bucket this emulator does not hold.
    let absent_bucket =
        update_journal_table(&router, "journal-update-no-such-bucket", &[], &enable).await;
    assert_eq!(
        absent_bucket.status,
        404,
        "NoSuchBucket is a 404, got {} with body {:?}",
        absent_bucket.status,
        String::from_utf8_lossy(&absent_bucket.body)
    );
    assert_eq!(
        absent_bucket.error_code(),
        "NoSuchBucket",
        "an update naming no bucket of this emulator must say so, got {:?}",
        String::from_utf8_lossy(&absent_bucket.body)
    );

    // A bucket carrying no metadata configuration: there is no configuration whose journal
    // records could expire, and the refused update records none.
    let bucket = "journal-update-no-configuration";
    create_bucket(&router, bucket, None, None).await;
    let absent_configuration = update_journal_table(&router, bucket, &[], &enable).await;
    assert_eq!(
        absent_configuration.status,
        404,
        "a bucket carrying no metadata configuration answers 404, got {} with body {:?}",
        absent_configuration.status,
        String::from_utf8_lossy(&absent_configuration.body)
    );
    assert_eq!(
        absent_configuration.error_code(),
        "MetadataConfigurationNotFound",
        "an update of a configuration the bucket does not carry must say the metadata \
         configuration is not there, got {:?}",
        String::from_utf8_lossy(&absent_configuration.body)
    );
    assert_eq!(
        absent_configuration.error_message(),
        "The metadata configuration was not found",
        "a refusal carries the text S3 publishes for that code"
    );
    let still_absent = get_metadata_configuration(&router, bucket, &[]).await;
    assert_eq!(
        still_absent.error_code(),
        "MetadataConfigurationNotFound",
        "a refused update must record nothing, got {:?}",
        String::from_utf8_lossy(&still_absent.body)
    );

    // An expected bucket owner that is not this account's: "403 Forbidden (access denied)",
    // with the configuration left exactly as it was.
    create_metadata_configuration(&router, bucket, &[], &starting_configuration("DISABLED", None))
        .await;
    wait_for_the_tables();
    let before = get_metadata_configuration(&router, bucket, &[]).await;
    let before = String::from_utf8_lossy(&before.body).to_string();
    let denied = update_journal_table(
        &router,
        bucket,
        &[("x-amz-expected-bucket-owner", NOT_THE_OWNER)],
        &enable,
    )
    .await;
    assert_eq!(
        denied.status,
        403,
        "an owner mismatch is a 403, got {} with body {:?}",
        denied.status,
        String::from_utf8_lossy(&denied.body)
    );
    assert_eq!(
        denied.error_code(),
        "AccessDenied",
        "an owner mismatch is reported as AccessDenied, got {:?}",
        String::from_utf8_lossy(&denied.body)
    );
    let unchanged = get_metadata_configuration(&router, bucket, &[]).await;
    assert_eq!(
        String::from_utf8_lossy(&unchanged.body).to_string(),
        before,
        "a denied update must leave the stored configuration exactly as it was"
    );
}
