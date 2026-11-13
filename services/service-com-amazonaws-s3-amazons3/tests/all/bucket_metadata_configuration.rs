// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's S3 METADATA configuration:
//! `CreateBucketMetadataConfiguration` records it and creates the tables it asks for, and
//! `GetBucketMetadataConfiguration` reports them.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate already owns), so what is pinned is the wire
//! behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `CreateBucketMetadataConfiguration` "creates an S3 Metadata V2 metadata configuration
//!   for a general purpose bucket" (the operation's own documentation): what the
//!   `MetadataConfiguration` payload names becomes the bucket's configuration, the call
//!   answers the modelled 200 with an empty body, and a later
//!   `GetBucketMetadataConfiguration` reports it.
//! * A TABLE exists exactly when the configuration asks for one: the journal table always
//!   (`MetadataConfiguration$JournalTableConfiguration` is required), the inventory and
//!   annotation tables only while their `ConfigurationState` is `ENABLED` — which is why
//!   `InventoryTableConfigurationResult` requires its `ConfigurationState` and leaves its
//!   `TableName`, `TableStatus` and `TableArn` optional. All three table containers are
//!   REPORTED whatever state their tables are in: a table the configuration never named is
//!   reported as the `DISABLED` state it is in, with no table beside it.
//! * The tables are S3-managed, so their coordinates are the service's: each table is named
//!   for which of the three it is (`journal`, `inventory`, `annotation`) inside the general
//!   purpose bucket's OWN namespace of the managed table bucket, `b_` and the bucket's name;
//!   a table's ARN carries the id that identifies that one table of the table bucket, its
//!   namespace and name written in the character set an S3 Tables name is written in.
//! * `TableStatus` is "`CREATING` - the journal table is in the process of being created"
//!   until the table falls due to be in service, and "`ACTIVE` - the journal table has
//!   been created successfully" from then on
//!   (`JournalTableConfigurationResult$TableStatus`).
//! * "Journal table records must be retained for a minimum of 7 days. To set this value,
//!   specify any whole number from `7` to `2147483647`" (`RecordExpiration$Days`): a day
//!   count inside that range round-trips, one outside it is refused with `InvalidRequest`
//!   (HTTP 400) and nothing is recorded.
//! * "If server-side encryption with Key Management Service (KMS) keys (SSE-KMS) is
//!   specified, you must also specify the KMS key Amazon Resource Name (ARN)"
//!   (`MetadataTableEncryptionConfiguration$KmsKeyArn`): the same refusal for whichever of
//!   the three tables carries that container.
//! * A bucket carries at most one metadata configuration, so a second create is refused
//!   with `MetadataConfigurationAlreadyExists` (HTTP 409) and leaves the stored one
//!   untouched; a bucket carrying none answers `MetadataConfigurationNotFound` (HTTP 404),
//!   one this
//!   emulator does not hold `NoSuchBucket` (HTTP 404), and an
//!   `x-amz-expected-bucket-owner` that is not the account's HTTP 403 `AccessDenied`.

use std::time::Duration;

use service_com_amazonaws_s3_amazons3::state::{
    INVALID_RECORD_EXPIRATION_MESSAGE, METADATA_TABLE_ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
///
/// `pub(crate)`, like the constants and helpers below it: the inventory-table update suite
/// ([`super::bucket_metadata_inventory_table_update`]) drives the same subresource through
/// the same harness instead of standing up a second copy of it.
pub(crate) const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// An account id that is NOT the one account this emulator serves, so an
/// `x-amz-expected-bucket-owner` carrying it names an owner no bucket here has.
pub(crate) const NOT_THE_OWNER: &str = "123456789012";

/// An IAM role ARN of the shape `AnnotationTableConfiguration$Role` documents ("the ARN of
/// the IAM role used to manage the annotation table").
pub(crate) const ANNOTATION_ROLE: &str = "arn:aws:iam::000000000000:role/s3-metadata-annotations";

/// A customer-managed KMS key ARN, as `MetadataTableEncryptionConfiguration$KmsKeyArn`
/// takes one.
pub(crate) const KMS_KEY_ARN: &str = "arn:aws:kms:us-east-1:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The name S3 Metadata gives the JOURNAL table of a V2 configuration, spelled out here
/// rather than read from the code under test.
pub(crate) const JOURNAL_TABLE: &str = "journal";

/// See [`JOURNAL_TABLE`] — the INVENTORY table's name.
pub(crate) const INVENTORY_TABLE: &str = "inventory";

/// See [`JOURNAL_TABLE`] — the ANNOTATION table's name.
pub(crate) const ANNOTATION_TABLE: &str = "annotation";

/// How long a row waits for a table to fall due: the span the store records plus slack for
/// a loaded machine.
///
/// A blocking sleep rather than `tokio::time::sleep(..).await`, for the reason the restore
/// suite in this crate gives: the workspace's tokio carries only the `macros`/`rt`
/// features, so its timer driver is not built.
pub(crate) fn wait_for_the_tables() {
    std::thread::sleep(METADATA_TABLE_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// `POST /<bucket>?metadataConfiguration` — `CreateBucketMetadataConfiguration`, with
/// `payload` as the `MetadataConfiguration` container and whatever headers the case
/// carries.
pub(crate) async fn create_metadata_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
    payload: &str,
) -> Answer {
    send(
        router,
        "POST",
        &format!("/{bucket}?metadataConfiguration"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>?metadataConfiguration` — `GetBucketMetadataConfiguration`.
pub(crate) async fn get_metadata_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}?metadataConfiguration"),
        headers,
        "",
    )
    .await
}

/// The inner XML of the first `<name>…</name>` container of `fragment`, or `None` when it
/// carries none — the one reader this suite slices a response with, used both for a
/// container and for a leaf element.
///
/// Scoping a leaf read to its container is what keeps the three tables' identically named
/// members apart: `TableName` appears once per table, so the journal table's is read out of
/// the journal container rather than out of the whole body.
pub(crate) fn inside<'a>(fragment: &'a str, name: &str) -> Option<&'a str> {
    let (_, rest) = fragment.split_once(&format!("<{name}>"))?;
    let (inner, _) = rest.split_once(&format!("</{name}>"))?;
    Some(inner)
}

/// That same read, as an owned value — for a leaf element whose text is the value.
pub(crate) fn text(fragment: &str, name: &str) -> Option<String> {
    inside(fragment, name).map(str::to_string)
}

/// What one row asks for in a table's `ConfigurationState`, or that it names no such table
/// at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableAsked {
    /// The container is absent from the payload, so the configuration has no such table
    /// configuration and the read reports none.
    Unnamed,
    /// `<ConfigurationState>ENABLED</ConfigurationState>`: the table is created.
    Enabled,
    /// `<ConfigurationState>DISABLED</ConfigurationState>`: the state is recorded and no
    /// table is created behind it.
    Disabled,
}

impl TableAsked {
    /// The `ConfigurationState` value this names, when it names one.
    fn state(self) -> Option<&'static str> {
        match self {
            Self::Unnamed => None,
            Self::Enabled => Some("ENABLED"),
            Self::Disabled => Some("DISABLED"),
        }
    }

    /// Whether a table is created for it.
    fn creates_a_table(self) -> bool {
        self == Self::Enabled
    }
}

/// One row of the create-and-read-back table.
#[derive(Debug, Clone)]
struct Case {
    /// The case name every assertion carries.
    name: &'static str,
    /// The general purpose bucket the configuration is created for.
    bucket: &'static str,
    /// The `RecordExpiration$Expiration` the journal table configuration names.
    expiration: &'static str,
    /// The `RecordExpiration$Days` it names, when it names one.
    days: Option<i32>,
    /// What the inventory table configuration asks for.
    inventory: TableAsked,
    /// What the annotation table configuration asks for.
    annotation: TableAsked,
    /// The IAM role the annotation table configuration names, when it names one.
    role: Option<&'static str>,
    /// The `SseAlgorithm` the journal table's encryption configuration names, when the row
    /// carries one; a KMS algorithm is named beside [`KMS_KEY_ARN`].
    journal_encryption: Option<&'static str>,
    /// The namespace this row's tables must read back as living in — spelled out, so what the
    /// namespace is made of is pinned by hand rather than by the code under test.
    namespace: &'static str,
    /// That namespace as a table ARN's id carries it: written in the character set an S3
    /// Tables name is written in, so each character of the bucket's name outside it is an
    /// underscore — spelled out here too.
    namespace_id: &'static str,
}

/// The `MetadataConfiguration` container this case carries, naming the members it names and
/// nothing else.
fn payload(case: &Case) -> String {
    let mut body = format!("<MetadataConfiguration xmlns=\"{XMLNS}\">");
    body.push_str("<JournalTableConfiguration><RecordExpiration>");
    body.push_str(&format!("<Expiration>{}</Expiration>", case.expiration));
    if let Some(days) = case.days {
        body.push_str(&format!("<Days>{days}</Days>"));
    }
    body.push_str("</RecordExpiration>");
    if let Some(algorithm) = case.journal_encryption {
        body.push_str(&encryption(algorithm));
    }
    body.push_str("</JournalTableConfiguration>");
    if let Some(state) = case.inventory.state() {
        body.push_str(&format!(
            "<InventoryTableConfiguration>\
             <ConfigurationState>{state}</ConfigurationState>\
             </InventoryTableConfiguration>"
        ));
    }
    if let Some(state) = case.annotation.state() {
        body.push_str("<AnnotationTableConfiguration>");
        body.push_str(&format!(
            "<ConfigurationState>{state}</ConfigurationState>"
        ));
        if let Some(role) = case.role {
            body.push_str(&format!("<Role>{role}</Role>"));
        }
        body.push_str("</AnnotationTableConfiguration>");
    }
    body.push_str("</MetadataConfiguration>");
    body
}

/// A `MetadataTableEncryptionConfiguration` naming `algorithm`, with the KMS key ARN beside
/// it exactly when that algorithm is the KMS one.
fn encryption(algorithm: &str) -> String {
    let key = if algorithm == "aws:kms" {
        format!("<KmsKeyArn>{KMS_KEY_ARN}</KmsKeyArn>")
    } else {
        String::new()
    };
    format!(
        "<EncryptionConfiguration>\
         <SseAlgorithm>{algorithm}</SseAlgorithm>{key}\
         </EncryptionConfiguration>"
    )
}

/// The table bucket ARN every configuration's destination reports, and the table ARN of
/// `table` inside it: one managed table bucket per Region, in the Region the general
/// purpose bucket lives in (`us-east-1` for every bucket this suite creates).
pub(crate) fn table_bucket_arn() -> String {
    String::from("arn:aws:s3tables:us-east-1:000000000000:bucket/aws-s3")
}

/// See [`table_bucket_arn`] — the ARN of the table `table` of the namespace whose folded
/// form is `namespace_id`, whose last segment identifies that one table of the table bucket.
pub(crate) fn table_arn(namespace_id: &str, table: &str) -> String {
    format!("{}/table/{namespace_id}_{table}", table_bucket_arn())
}

/// Every row of the create-and-read-back table: the rows differ along the expiration state,
/// the day count (including both published bounds), which of the optional tables the
/// configuration names and in which state, whether an IAM role is named, whether an
/// encryption configuration is named, and the FORM of the bucket's name the namespace and the
/// table ids are built from.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "journal only, at the published minimum day count",
            bucket: "metadata-journal-minimum",
            expiration: "ENABLED",
            days: Some(7),
            inventory: TableAsked::Unnamed,
            annotation: TableAsked::Unnamed,
            role: None,
            journal_encryption: None,
            namespace: "b_metadata-journal-minimum",
            namespace_id: "b_metadata_journal_minimum",
        },
        Case {
            name: "journal and inventory, a year of records",
            bucket: "metadata-year-of-records",
            expiration: "ENABLED",
            days: Some(365),
            inventory: TableAsked::Enabled,
            annotation: TableAsked::Unnamed,
            role: None,
            journal_encryption: Some("AES256"),
            namespace: "b_metadata-year-of-records",
            namespace_id: "b_metadata_year_of_records",
        },
        Case {
            name: "every table, at the published maximum day count",
            bucket: "metadata.every-table",
            expiration: "ENABLED",
            days: Some(2_147_483_647),
            inventory: TableAsked::Disabled,
            annotation: TableAsked::Enabled,
            role: Some(ANNOTATION_ROLE),
            journal_encryption: Some("aws:kms"),
            namespace: "b_metadata.every-table",
            namespace_id: "b_metadata_every_table",
        },
        Case {
            name: "record expiration disabled, so no day count",
            bucket: "metadata-no-expiration",
            expiration: "DISABLED",
            days: None,
            inventory: TableAsked::Enabled,
            annotation: TableAsked::Disabled,
            role: None,
            journal_encryption: None,
            namespace: "b_metadata-no-expiration",
            namespace_id: "b_metadata_no_expiration",
        },
        Case {
            name: "a bucket name carrying dots and digits",
            bucket: "metadata.tables.2026",
            expiration: "ENABLED",
            days: Some(30),
            inventory: TableAsked::Enabled,
            annotation: TableAsked::Enabled,
            role: Some(ANNOTATION_ROLE),
            journal_encryption: None,
            namespace: "b_metadata.tables.2026",
            namespace_id: "b_metadata_tables_2026",
        },
    ]
}

/// What a read reports for one table container, as a row expects it.
///
/// The container is there WHATEVER the row asked for, because `ConfigurationState` is a
/// required member of both optional results: a row that named no such table configuration
/// reads back the `DISABLED` state it is in, with no table name, status or ARN beside it.
fn assert_table(
    case: &Case,
    container: Option<&str>,
    asked: TableAsked,
    table: &str,
    expected_status: &str,
) {
    let container = container.unwrap_or_else(|| {
        panic!(
            "case {:?}: every optional table result is reported whatever state its table is \
             in, so the read must report this one",
            case.name
        )
    });
    let state = asked.state().unwrap_or("DISABLED");
    assert_eq!(
        text(container, "ConfigurationState").as_deref(),
        Some(state),
        "case {:?}: expected ConfigurationState {state}, got {:?}",
        case.name,
        text(container, "ConfigurationState")
    );
    let expected_name = asked.creates_a_table().then_some(table);
    assert_eq!(
        text(container, "TableName").as_deref(),
        expected_name,
        "case {:?}: expected TableName {expected_name:?}, got {:?}",
        case.name,
        text(container, "TableName")
    );
    let expected_arn = expected_name.map(|table| table_arn(case.namespace_id, table));
    assert_eq!(
        text(container, "TableArn"),
        expected_arn,
        "case {:?}: expected TableArn {:?}, got {:?}",
        case.name,
        expected_name.map(|table| table_arn(case.namespace_id, table)),
        text(container, "TableArn")
    );
    let status = asked.creates_a_table().then_some(expected_status);
    assert_eq!(
        text(container, "TableStatus").as_deref(),
        status,
        "case {:?}: expected TableStatus {status:?}, got {:?}",
        case.name,
        text(container, "TableStatus")
    );
}

/// Everything a read of `case`'s configuration must report, with each table in
/// `expected_status`.
fn assert_reported(case: &Case, body: &str, expected_status: &str) {
    let destination = inside(body, "DestinationResult").unwrap_or_else(|| {
        panic!(
            "case {:?}: the read must report a DestinationResult, got {body:?}",
            case.name
        )
    });
    assert_eq!(
        text(destination, "TableBucketType").as_deref(),
        Some("aws"),
        "case {:?}: expected the managed table bucket type, got {:?}",
        case.name,
        text(destination, "TableBucketType")
    );
    assert_eq!(
        text(destination, "TableBucketArn"),
        Some(table_bucket_arn()),
        "case {:?}: expected TableBucketArn {:?}, got {:?}",
        case.name,
        table_bucket_arn(),
        text(destination, "TableBucketArn")
    );
    assert_eq!(
        text(destination, "TableNamespace").as_deref(),
        Some(case.namespace),
        "case {:?}: expected the bucket's own table namespace {:?}, got {:?}",
        case.name,
        case.namespace,
        text(destination, "TableNamespace")
    );

    let journal = inside(body, "JournalTableConfigurationResult").unwrap_or_else(|| {
        panic!(
            "case {:?}: every configuration has a journal table, so the read must report \
             one, got {body:?}",
            case.name
        )
    });
    assert_eq!(
        text(journal, "TableName").as_deref(),
        Some(JOURNAL_TABLE),
        "case {:?}: expected journal TableName {JOURNAL_TABLE:?}, got {:?}",
        case.name,
        text(journal, "TableName")
    );
    assert_eq!(
        text(journal, "TableArn"),
        Some(table_arn(case.namespace_id, JOURNAL_TABLE)),
        "case {:?}: expected journal TableArn {:?}, got {:?}",
        case.name,
        table_arn(case.namespace_id, JOURNAL_TABLE),
        text(journal, "TableArn")
    );
    assert_eq!(
        text(journal, "TableStatus").as_deref(),
        Some(expected_status),
        "case {:?}: expected journal TableStatus {expected_status}, got {:?}",
        case.name,
        text(journal, "TableStatus")
    );
    let expiration = inside(journal, "RecordExpiration").unwrap_or_else(|| {
        panic!(
            "case {:?}: RecordExpiration is a required member of the journal result, got \
             {journal:?}",
            case.name
        )
    });
    assert_eq!(
        text(expiration, "Expiration").as_deref(),
        Some(case.expiration),
        "case {:?}: expected Expiration {:?}, got {:?}",
        case.name,
        case.expiration,
        text(expiration, "Expiration")
    );
    assert_eq!(
        text(expiration, "Days"),
        case.days.map(|days| days.to_string()),
        "case {:?}: expected Days {:?}, got {:?}",
        case.name,
        case.days,
        text(expiration, "Days")
    );

    assert_table(
        case,
        inside(body, "InventoryTableConfigurationResult"),
        case.inventory,
        INVENTORY_TABLE,
        expected_status,
    );
    assert_table(
        case,
        inside(body, "AnnotationTableConfigurationResult"),
        case.annotation,
        ANNOTATION_TABLE,
        expected_status,
    );
    let annotation = inside(body, "AnnotationTableConfigurationResult");
    assert_eq!(
        annotation.and_then(|container| text(container, "Role")),
        case.role.map(str::to_string),
        "case {:?}: expected the annotation Role {:?}, got {:?}",
        case.name,
        case.role,
        annotation.and_then(|container| text(container, "Role"))
    );
}

#[tokio::test]
async fn a_created_metadata_configuration_reads_back_with_the_tables_it_asked_for() {
    let router = router();

    for case in cases() {
        create_bucket(&router, case.bucket, None, None).await;

        let created = create_metadata_configuration(&router, case.bucket, &[], &payload(&case)).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected the modelled 200, got {} with body {:?}",
            case.name,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        assert!(
            created.body.is_empty(),
            "case {:?}: the create models a Unit output, so its body must be empty, got {:?}",
            case.name,
            String::from_utf8_lossy(&created.body)
        );

        // Straight after the create, every table it created is still being created: the
        // status member publishes a transient `CREATING` value for exactly that window.
        //
        // Each row's window starts at its OWN create — the store records the instant that
        // create's tables fall due — so what this assertion depends on is the time between
        // this row's create and this row's read, one in-process dispatch each, and NOT how
        // many rows the loop has already run.
        let reading = get_metadata_configuration(&router, case.bucket, &[]).await;
        assert_eq!(
            reading.status, 200,
            "case {:?}: expected the read to answer 200, got {} with body {:?}",
            case.name,
            reading.status,
            String::from_utf8_lossy(&reading.body)
        );
        let body = String::from_utf8_lossy(&reading.body).to_string();
        assert_reported(&case, &body, "CREATING");
    }

    // Once the span the store recorded has passed, the same configurations report the same
    // tables in service — and nothing else about them moved, which is what re-running the
    // whole table against `ACTIVE` pins.
    wait_for_the_tables();
    for case in cases() {
        let reading = get_metadata_configuration(&router, case.bucket, &[]).await;
        let body = String::from_utf8_lossy(&reading.body).to_string();
        assert_reported(&case, &body, "ACTIVE");

        // A read changes nothing a later caller sees: the second read reports exactly what
        // the first one did.
        let again = get_metadata_configuration(&router, case.bucket, &[]).await;
        assert_eq!(
            String::from_utf8_lossy(&again.body).to_string(),
            body,
            "case {:?}: a read must leave the configuration exactly as it was",
            case.name
        );
    }
}

#[tokio::test]
async fn a_second_create_is_refused_and_leaves_the_stored_configuration_untouched() {
    let router = router();
    let case = &cases()[1];
    create_bucket(&router, case.bucket, None, None).await;
    create_metadata_configuration(&router, case.bucket, &[], &payload(case)).await;

    // The tables are allowed to reach their terminal status FIRST, so what the two reads
    // below report cannot differ because of the clock: the whole comparison is then about
    // what the refused create did or did not write.
    wait_for_the_tables();
    let before = get_metadata_configuration(&router, case.bucket, &[]).await;
    let before = String::from_utf8_lossy(&before.body).to_string();

    // A different configuration, so a create that overwrote rather than refused would be
    // visible in the read below.
    let overwrite = Case {
        expiration: "DISABLED",
        days: None,
        inventory: TableAsked::Unnamed,
        ..case.clone()
    };
    let refused =
        create_metadata_configuration(&router, case.bucket, &[], &payload(&overwrite)).await;
    assert_eq!(
        refused.status, 409,
        "a second create must be refused with 409, got {} with body {:?}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(
        refused.error_code(),
        "MetadataConfigurationAlreadyExists",
        "a second create must name the configuration that is already there"
    );
    assert_eq!(
        refused.error_message(),
        "The metadata configuration already exists",
        "a refusal carries the text S3 sends for that code"
    );

    let reading = get_metadata_configuration(&router, case.bucket, &[]).await;
    let after = String::from_utf8_lossy(&reading.body).to_string();
    assert_eq!(
        after, before,
        "a refused create must leave the stored configuration exactly as it was"
    );
    assert_reported(case, &after, "ACTIVE");
}

#[tokio::test]
async fn a_day_count_outside_the_published_range_is_refused_and_records_nothing() {
    let router = router();

    // (case, the Days the journal table configuration names, whether it is inside the
    // published range `7..=2147483647`).
    let rows: [(&str, i32, bool); 6] = [
        ("one day short of the minimum", 6, false),
        ("the published minimum", 7, true),
        ("no retention at all", 0, false),
        ("a negative count", -1, false),
        ("a year", 365, true),
        ("the published maximum", 2_147_483_647, true),
    ];

    for (name, days, inside_the_range) in rows {
        let bucket = format!("metadata-days-{}", days.unsigned_abs());
        create_bucket(&router, &bucket, None, None).await;
        let body = format!(
            "<MetadataConfiguration xmlns=\"{XMLNS}\">\
             <JournalTableConfiguration><RecordExpiration>\
             <Expiration>ENABLED</Expiration><Days>{days}</Days>\
             </RecordExpiration></JournalTableConfiguration>\
             </MetadataConfiguration>"
        );
        let answer = create_metadata_configuration(&router, &bucket, &[], &body).await;

        if inside_the_range {
            assert_eq!(
                answer.status, 200,
                "case {name:?}: {days} days is inside the published range, got {} with body \
                 {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {name:?}: {days} days is outside the published range, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequest",
            "case {name:?}: a record expiration that states no usable retention period is an \
             InvalidRequest"
        );
        assert_eq!(
            answer.error_message(),
            INVALID_RECORD_EXPIRATION_MESSAGE,
            "case {name:?}: the refusal carries that class's fixed sentence, got {:?}",
            answer.error_message()
        );
        // Refused BEFORE anything is written, so the bucket still carries no configuration.
        let reading = get_metadata_configuration(&router, &bucket, &[]).await;
        assert_eq!(
            reading.error_code(),
            "MetadataConfigurationNotFound",
            "case {name:?}: a refused create must record nothing, got {:?}",
            String::from_utf8_lossy(&reading.body)
        );
    }
}

#[tokio::test]
async fn a_kms_algorithm_without_a_key_arn_is_refused_for_every_table() {
    let router = router();

    // (the table container whose encryption configuration the row names, the rest of the
    // configuration that container sits in).
    let rows: [(&str, &str); 3] = [
        ("JournalTableConfiguration", ""),
        (
            "InventoryTableConfiguration",
            "<ConfigurationState>ENABLED</ConfigurationState>",
        ),
        (
            "AnnotationTableConfiguration",
            "<ConfigurationState>ENABLED</ConfigurationState>",
        ),
    ];

    for (index, (member, state)) in rows.iter().enumerate() {
        let bucket = format!("metadata-kms-{index}");
        create_bucket(&router, &bucket, None, None).await;

        // The journal table configuration is required, so a row naming another table's
        // encryption still carries one — with no encryption configuration of its own.
        let journal = if *member == "JournalTableConfiguration" {
            String::from(
                "<JournalTableConfiguration><RecordExpiration>\
                 <Expiration>DISABLED</Expiration></RecordExpiration>\
                 <EncryptionConfiguration><SseAlgorithm>aws:kms</SseAlgorithm>\
                 </EncryptionConfiguration></JournalTableConfiguration>",
            )
        } else {
            String::from(
                "<JournalTableConfiguration><RecordExpiration>\
                 <Expiration>DISABLED</Expiration></RecordExpiration>\
                 </JournalTableConfiguration>",
            )
        };
        let other = if *member == "JournalTableConfiguration" {
            String::new()
        } else {
            format!(
                "<{member}>{state}<EncryptionConfiguration>\
                 <SseAlgorithm>aws:kms</SseAlgorithm></EncryptionConfiguration></{member}>"
            )
        };
        let keyless = format!(
            "<MetadataConfiguration xmlns=\"{XMLNS}\">{journal}{other}</MetadataConfiguration>"
        );

        let refused = create_metadata_configuration(&router, &bucket, &[], &keyless).await;
        assert_eq!(
            refused.status, 400,
            "case {member:?}: SSE-KMS without a key ARN must be refused, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {member:?}: a KMS algorithm with no key ARN beside it is an InvalidArgument"
        );

        // The same configuration with the key ARN named is accepted, so what the row pins is
        // the missing key and not the algorithm.
        let keyed = keyless.replace(
            "<SseAlgorithm>aws:kms</SseAlgorithm>",
            &format!("<SseAlgorithm>aws:kms</SseAlgorithm><KmsKeyArn>{KMS_KEY_ARN}</KmsKeyArn>"),
        );
        let accepted = create_metadata_configuration(&router, &bucket, &[], &keyed).await;
        assert_eq!(
            accepted.status, 200,
            "case {member:?}: the same configuration naming a key ARN must be accepted, got {} \
             with body {:?}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
    }
}

#[tokio::test]
async fn a_call_naming_an_address_or_an_owner_this_emulator_does_not_hold_says_which() {
    let router = router();
    let case = &cases()[0];
    let configuration = payload(case);

    // A bucket this emulator does not hold: both calls answer the shape they widen in.
    let created =
        create_metadata_configuration(&router, "metadata-no-such-bucket", &[], &configuration)
            .await;
    assert_eq!(
        created.error_code(),
        "NoSuchBucket",
        "a create naming no bucket of this emulator must say so, got {:?}",
        String::from_utf8_lossy(&created.body)
    );
    assert_eq!(created.status, 404, "NoSuchBucket is a 404");
    let read = get_metadata_configuration(&router, "metadata-no-such-bucket", &[]).await;
    assert_eq!(
        read.error_code(),
        "NoSuchBucket",
        "a read naming no bucket of this emulator must say so, got {:?}",
        String::from_utf8_lossy(&read.body)
    );

    // A bucket that carries no configuration: the read reports that, and reports it again
    // after a refused create.
    create_bucket(&router, case.bucket, None, None).await;
    let absent = get_metadata_configuration(&router, case.bucket, &[]).await;
    assert_eq!(
        absent.status, 404,
        "a bucket carrying no metadata configuration answers 404, got {}",
        absent.status
    );
    assert_eq!(
        absent.error_code(),
        "MetadataConfigurationNotFound",
        "a bucket carrying no metadata configuration must say the metadata configuration is \
         not there"
    );
    assert_eq!(
        absent.error_message(),
        "The metadata configuration was not found",
        "the text S3 publishes for that code"
    );

    // An expected bucket owner that is not this account's: "403 Forbidden (access denied)"
    // on both calls, and nothing recorded by the create.
    let owner = [("x-amz-expected-bucket-owner", NOT_THE_OWNER)];
    let denied_create =
        create_metadata_configuration(&router, case.bucket, &owner, &configuration).await;
    assert_eq!(
        denied_create.status, 403,
        "an owner mismatch is a 403, got {} with body {:?}",
        denied_create.status,
        String::from_utf8_lossy(&denied_create.body)
    );
    assert_eq!(
        denied_create.error_code(),
        "AccessDenied",
        "an owner mismatch is reported as AccessDenied"
    );
    let still_absent = get_metadata_configuration(&router, case.bucket, &[]).await;
    assert_eq!(
        still_absent.error_code(),
        "MetadataConfigurationNotFound",
        "a denied create must record nothing"
    );

    create_metadata_configuration(&router, case.bucket, &[], &configuration).await;
    let denied_read = get_metadata_configuration(&router, case.bucket, &owner).await;
    assert_eq!(
        denied_read.status, 403,
        "an owner mismatch on the read is a 403, got {} with body {:?}",
        denied_read.status,
        String::from_utf8_lossy(&denied_read.body)
    );
    assert_eq!(
        denied_read.error_code(),
        "AccessDenied",
        "an owner mismatch on the read is reported as AccessDenied"
    );
}
