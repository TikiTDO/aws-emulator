// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's S3 Inventory configurations:
//! `PutBucketInventoryConfiguration` records one under the inventory id that identifies it
//! and `GetBucketInventoryConfiguration` reports that one back.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutBucketInventoryConfiguration` "adds an S3 Inventory configuration (identified by
//!   the inventory ID) to the bucket" (the operation's own documentation): the
//!   `InventoryConfiguration` payload becomes the configuration stored under THAT id in
//!   full, every other id is left alone, and the call answers
//!   [`PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS`] with an empty body. A second put under one id replaces that configuration rather than merging into
//!   it, so a member the replacement omits is gone — which is also why repeating a put of
//!   one id adds nothing to what the bucket carries.
//! * `GetBucketInventoryConfiguration` reports exactly that container, with every member the
//!   request named — the destination and its optional members, the filter prefix, the
//!   schedule and each optional field asked for — and no member it did not. A bucket
//!   carrying none under the id answers `NoSuchConfiguration` (HTTP 404).
//! * "You can have up to 1,000 inventory configurations per bucket" and the operation's
//!   special error "TooManyConfigurations … HTTP 400 Bad Request": a put that would create
//!   one past [`MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET`] is refused, while replacing an id
//!   already stored creates nothing and is not.
//! * The request's `Id` and the payload's `Id` identify ONE configuration, and an id is
//!   bounded by [`MAX_INVENTORY_ID_CHARS`]: a value outside that bound, or two values that
//!   disagree, is refused with `InvalidArgument` (HTTP 400). A member whose value is outside
//!   its enum's published set is refused with HTTP 400 as well.
//! * A call naming a bucket this emulator does not hold answers `NoSuchBucket` (HTTP 404);
//!   one naming an `ExpectedBucketOwner` that is not the account's answers HTTP 403
//!   `AccessDenied`. Every refusal leaves the stored configurations exactly as they were.

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::ops::put_bucket_inventory_configuration::PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::ops::types::{
    INVENTORY_FORMAT_VALUES, INVENTORY_FREQUENCY_VALUES,
    INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES, INVENTORY_OPTIONAL_FIELD_VALUES,
};
use service_com_amazonaws_s3_amazons3::state::{
    INVALID_ARGUMENT_CODE, MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET, MAX_INVENTORY_ID_CHARS,
    NO_SUCH_BUCKET_CODE, NO_SUCH_CONFIGURATION_CODE, TOO_MANY_CONFIGURATIONS_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_analytics_configuration::FOREIGN_ACCOUNT_ID;
use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// "The Amazon Resource Name (ARN) of the bucket where inventory results will be published"
/// (`InventoryS3BucketDestination$Bucket`) — a value of the request, so it comes back
/// verbatim.
const DESTINATION_ARN: &str = "arn:aws:s3:::inventory-destination";

/// A KMS key ARN, as `SSEKMS$KeyId` carries one: "the ID of the customer managed KMS key" —
/// again a value of the request, reported back as it was named.
const KMS_KEY_ARN: &str = "arn:aws:kms:us-east-1:123456789012:key/inventory-key";

/// How "where the inventory results are published" is encrypted, as
/// `smithy_query(query="shape",shape="InventoryEncryption")` declares it: either of the two
/// members, each with its own `xmlName`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Encryption {
    /// `InventoryEncryption$SSES3` — "Specifies the use of SSE-S3 to encrypt delivered
    /// inventory reports", a container with no members at all.
    Sses3,
    /// `InventoryEncryption$SSEKMS`, which carries the key id as its only member.
    SseKms(String),
}

impl Encryption {
    /// The `<Encryption>` element, with the member names the shape's `xmlName` traits give
    /// (`SSE-S3` and `SSE-KMS`, which are not the Rust member names).
    fn xml(&self) -> String {
        let inner = match self {
            Self::Sses3 => "<SSE-S3></SSE-S3>".to_string(),
            Self::SseKms(key_id) => format!("<SSE-KMS><KeyId>{key_id}</KeyId></SSE-KMS>"),
        };
        format!("<Encryption>{inner}</Encryption>")
    }
}

/// The `InventoryDestination$S3BucketDestination` of a configuration — "contains the bucket
/// name, file format, bucket owner (optional), and prefix (optional) where inventory results
/// are published" — flattened to the members a request names and a report brings back.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Destination {
    bucket: String,
    format: String,
    account_id: Option<String>,
    prefix: Option<String>,
    encryption: Option<Encryption>,
}

impl Destination {
    /// A destination publishing in `format` to [`DESTINATION_ARN`], with none of the
    /// optional members.
    pub(crate) fn new(format: &str) -> Self {
        Self {
            bucket: DESTINATION_ARN.to_string(),
            format: format.to_string(),
            account_id: None,
            prefix: None,
            encryption: None,
        }
    }

    /// The same destination, owned by `account_id` — "the destination bucket account ID".
    pub(crate) fn owned_by(mut self, account_id: &str) -> Self {
        self.account_id = Some(account_id.to_string());
        self
    }

    /// The same destination, publishing under `prefix` — "the prefix that is prepended to
    /// all inventory results".
    pub(crate) fn under(mut self, prefix: &str) -> Self {
        self.prefix = Some(prefix.to_string());
        self
    }

    /// The same destination, with the reports encrypted the named way.
    pub(crate) fn encrypted(mut self, encryption: Encryption) -> Self {
        self.encryption = Some(encryption);
        self
    }

    /// The `<Destination>` element, in the member order
    /// `smithy_query(query="shape",shape="InventoryS3BucketDestination")` declares.
    fn xml(&self) -> String {
        let mut body = String::new();
        if let Some(account_id) = &self.account_id {
            body.push_str(&format!("<AccountId>{account_id}</AccountId>"));
        }
        body.push_str(&format!("<Bucket>{}</Bucket>", self.bucket));
        body.push_str(&format!("<Format>{}</Format>", self.format));
        if let Some(prefix) = &self.prefix {
            body.push_str(&format!("<Prefix>{prefix}</Prefix>"));
        }
        if let Some(encryption) = &self.encryption {
            body.push_str(&encryption.xml());
        }
        format!("<Destination><S3BucketDestination>{body}</S3BucketDestination></Destination>")
    }
}

/// One whole `InventoryConfiguration`, as a put carries it and as a read reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Configuration {
    pub(crate) id: String,
    is_enabled: bool,
    included_object_versions: String,
    frequency: String,
    /// `InventoryFilter$Prefix` when the configuration names a filter at all — "the prefix
    /// that an object must have to be included in the inventory results".
    filter_prefix: Option<String>,
    /// `InventoryConfiguration$OptionalFields` when the configuration names the list.
    optional_fields: Option<Vec<String>>,
    destination: Destination,
}

impl Configuration {
    /// An enabled configuration of `id`, inventorying the named object versions on the named
    /// schedule into `destination`, with no filter and no optional fields.
    pub(crate) fn new(
        id: &str,
        included_object_versions: &str,
        frequency: &str,
        destination: Destination,
    ) -> Self {
        Self {
            id: id.to_string(),
            is_enabled: true,
            included_object_versions: included_object_versions.to_string(),
            frequency: frequency.to_string(),
            filter_prefix: None,
            optional_fields: None,
            destination,
        }
    }

    /// The same configuration, not in force — `InventoryConfiguration$IsEnabled` "set to
    /// False, no inventory list is generated".
    pub(crate) fn disabled(mut self) -> Self {
        self.is_enabled = false;
        self
    }

    /// The same configuration, inventorying only the objects under `prefix`.
    pub(crate) fn filtered(mut self, prefix: &str) -> Self {
        self.filter_prefix = Some(prefix.to_string());
        self
    }

    /// The same configuration, asking for exactly `fields` in the report.
    pub(crate) fn with_fields(mut self, fields: &[&str]) -> Self {
        self.optional_fields = Some(fields.iter().map(|field| (*field).to_string()).collect());
        self
    }

    /// The payload a client's XML writer spells for this configuration, in the member order
    /// `smithy_query(query="shape",shape="InventoryConfiguration")` declares.
    fn xml(&self) -> String {
        let mut body = format!("<InventoryConfiguration xmlns=\"{XMLNS}\">");
        body.push_str(&self.destination.xml());
        body.push_str(&format!("<IsEnabled>{}</IsEnabled>", self.is_enabled));
        if let Some(prefix) = &self.filter_prefix {
            body.push_str(&format!("<Filter><Prefix>{prefix}</Prefix></Filter>"));
        }
        body.push_str(&format!("<Id>{}</Id>", self.id));
        body.push_str(&format!(
            "<IncludedObjectVersions>{}</IncludedObjectVersions>",
            self.included_object_versions
        ));
        if let Some(fields) = &self.optional_fields {
            body.push_str("<OptionalFields>");
            for field in fields {
                body.push_str(&format!("<Field>{field}</Field>"));
            }
            body.push_str("</OptionalFields>");
        }
        body.push_str(&format!(
            "<Schedule><Frequency>{}</Frequency></Schedule>",
            self.frequency
        ));
        body.push_str("</InventoryConfiguration>");
        body
    }
}

/// `PUT /<bucket>?inventory&id=<id>` — `PutBucketInventoryConfiguration`.
pub(crate) async fn put_inventory(
    router: &MultiServiceRouter,
    bucket: &str,
    wanted: &Configuration,
) -> Answer {
    put_inventory_payload(router, bucket, &wanted.id, &wanted.xml(), &[]).await
}

/// `PutBucketInventoryConfiguration` with the payload spelled out — for a container no
/// [`Configuration`] can express — and with any extra bound headers.
async fn put_inventory_payload(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    payload: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?inventory&id={id}"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>?inventory&id=<id>` — `GetBucketInventoryConfiguration`.
pub(crate) async fn get_inventory(router: &MultiServiceRouter, bucket: &str, id: &str) -> Answer {
    get_inventory_with_headers(router, bucket, id, &[]).await
}

/// `GetBucketInventoryConfiguration` with extra bound headers.
async fn get_inventory_with_headers(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}?inventory&id={id}"),
        headers,
        "",
    )
    .await
}

/// The one configuration a `GetBucketInventoryConfiguration` body reports.
fn reported_configuration(answer: &Answer) -> Configuration {
    reported_configurations(answer)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// EVERY `InventoryConfiguration` a body reports, in document order: one for a
/// `GetBucketInventoryConfiguration` body, as many as the page carries for a
/// `ListBucketInventoryConfigurations` body (which the listing suite beside this one reads
/// through this same parser rather than a second one).
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the analytics
/// suite beside this one reads a container, rather than by slicing the body. The member a
/// value belongs to is its PATH, not its tag: `Prefix` sits on the filter and on the
/// destination alike, and `Bucket` names the destination bucket.
pub(crate) fn reported_configurations(answer: &Answer) -> Vec<Configuration> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut all: Vec<Configuration> = Vec::new();
    // The configuration being read: an `InventoryConfiguration` opening starts a new one, so
    // the previous one is complete and is pushed then.
    let mut reported = Configuration::default();
    let mut reading = false;
    let mut path: Vec<String> = Vec::new();
    // An element OPENING is what declares which member is present, so a member whose value
    // is the empty string (`<Prefix></Prefix>`, which carries no text event at all) and a
    // container with no members at all (`<SSE-S3>`) are reported as present rather than
    // absent.
    let begin = |name: &str, path: &[String], reported: &mut Configuration| {
        let parent = path.last().map(String::as_str).unwrap_or_default();
        match (parent, name) {
            ("Filter", "Prefix") => reported.filter_prefix = Some(String::new()),
            ("InventoryConfiguration", "OptionalFields") => {
                reported.optional_fields = Some(Vec::new());
            }
            ("OptionalFields", "Field") => {
                if let Some(fields) = reported.optional_fields.as_mut() {
                    fields.push(String::new());
                }
            }
            ("S3BucketDestination", "AccountId") => {
                reported.destination.account_id = Some(String::new());
            }
            ("S3BucketDestination", "Prefix") => {
                reported.destination.prefix = Some(String::new());
            }
            ("Encryption", "SSE-S3") => {
                reported.destination.encryption = Some(Encryption::Sses3);
            }
            ("Encryption", "SSE-KMS") => {
                reported.destination.encryption = Some(Encryption::SseKms(String::new()));
            }
            _ => {}
        }
    };
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Empty(empty) => {
                let name = String::from_utf8_lossy(empty.name().as_ref()).into_owned();
                begin(&name, &path, &mut reported);
            }
            Event::Start(start) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                if name == "InventoryConfiguration" {
                    if reading {
                        all.push(std::mem::take(&mut reported));
                    }
                    reading = true;
                }
                begin(&name, &path, &mut reported);
                path.push(name);
            }
            Event::Text(text) => {
                let value = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"))
                    .into_owned();
                let here: Vec<&str> = path.iter().map(String::as_str).collect();
                match here[here.len().saturating_sub(2)..] {
                    ["InventoryConfiguration", "Id"] => reported.id = value,
                    ["InventoryConfiguration", "IsEnabled"] => reported.is_enabled = value == "true",
                    ["InventoryConfiguration", "IncludedObjectVersions"] => {
                        reported.included_object_versions = value;
                    }
                    ["Schedule", "Frequency"] => reported.frequency = value,
                    ["Filter", "Prefix"] => reported.filter_prefix = Some(value),
                    ["OptionalFields", "Field"] => {
                        if let Some(fields) = reported.optional_fields.as_mut() {
                            if let Some(last) = fields.last_mut() {
                                *last = value;
                            }
                        }
                    }
                    ["S3BucketDestination", member] => match member {
                        "Bucket" => reported.destination.bucket = value,
                        "Format" => reported.destination.format = value,
                        "AccountId" => reported.destination.account_id = Some(value),
                        "Prefix" => reported.destination.prefix = Some(value),
                        _ => {}
                    },
                    ["SSE-KMS", "KeyId"] => {
                        reported.destination.encryption = Some(Encryption::SseKms(value));
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if reading {
        all.push(reported);
    }
    all
}

/// Stand up a bucket named `name`, which is what an inventory configuration is put on.
pub(crate) async fn bucket(router: &MultiServiceRouter, name: &str) {
    let created = create_bucket(router, name, None, None).await;
    assert_eq!(
        created.status, 200,
        "case setup: the bucket {name:?} must be created, got {}",
        created.status
    );
}

/// An id exactly [`MAX_INVENTORY_ID_CHARS`] characters long — the ceiling every bucket
/// configuration id shares, which a put must still take.
fn id_at_the_ceiling() -> String {
    "i".repeat(MAX_INVENTORY_ID_CHARS)
}

/// The table of configurations every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: WHICH value each
/// of the three enum members takes — the published catalogs
/// `INVENTORY_FORMAT_VALUES`, `INVENTORY_FREQUENCY_VALUES` and
/// `INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES`, every combination of them, so a value this
/// issue never cited is covered too — whether the configuration is in force, whether it
/// names a filter and what the prefix is (including the empty one), HOW MANY optional fields
/// it asks for (none, the list named but empty, one, every published field), WHICH of the
/// destination's optional members it carries (a foreign owning account, a prefix, either
/// encryption, neither), and how the id is SPELLED — up to the 64-character ceiling.
fn round_trip_rows() -> Vec<(String, Configuration)> {
    let mut rows: Vec<(String, Configuration)> = Vec::new();

    // Every combination of the three enum catalogs, which is where a value outside the
    // handful a recording happens to carry gets exercised.
    for format in INVENTORY_FORMAT_VALUES {
        for frequency in INVENTORY_FREQUENCY_VALUES {
            for versions in INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES {
                let id = format!("{format}-{frequency}-{versions}");
                let configuration =
                    Configuration::new(&id, versions, frequency, Destination::new(format));
                rows.push((
                    format!("{format} reports {frequency}, {versions} object versions"),
                    configuration,
                ));
            }
        }
    }

    rows.push((
        "a filter prefix and a destination under a prefix".to_string(),
        Configuration::new(
            "documents",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[0]).under("reports/documents/"),
        )
        .filtered("documents/"),
    ));
    rows.push((
        "an empty filter prefix, which selects every object".to_string(),
        Configuration::new(
            "empty-prefix",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[1],
            INVENTORY_FREQUENCY_VALUES[1],
            Destination::new(INVENTORY_FORMAT_VALUES[0]),
        )
        .filtered(""),
    ));
    rows.push((
        "a disabled configuration with one optional field".to_string(),
        Configuration::new(
            "not_in_force",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[1]),
        )
        .disabled()
        .with_fields(&[INVENTORY_OPTIONAL_FIELD_VALUES[0]]),
    ));
    rows.push((
        "every optional field the model publishes".to_string(),
        Configuration::new(
            "all.fields",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[1],
            Destination::new(INVENTORY_FORMAT_VALUES[2]),
        )
        .with_fields(INVENTORY_OPTIONAL_FIELD_VALUES),
    ));
    rows.push((
        "a destination in another account, encrypted with SSE-S3".to_string(),
        Configuration::new(
            "cross-account",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[1],
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[0])
                .owned_by(FOREIGN_ACCOUNT_ID)
                .encrypted(Encryption::Sses3),
        )
        .filtered("logs/"),
    ));
    rows.push((
        "a destination encrypted with a KMS key, under a prefix".to_string(),
        Configuration::new(
            "kms-encrypted",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[1],
            Destination::new(INVENTORY_FORMAT_VALUES[1])
                .owned_by(FOREIGN_ACCOUNT_ID)
                .under("exports/")
                .encrypted(Encryption::SseKms(KMS_KEY_ARN.to_string())),
        )
        .with_fields(&[
            INVENTORY_OPTIONAL_FIELD_VALUES[2],
            INVENTORY_OPTIONAL_FIELD_VALUES[5],
        ]),
    ));
    rows.push((
        "an id at the 64-character ceiling".to_string(),
        Configuration::new(
            &id_at_the_ceiling(),
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[0]),
        ),
    ));

    rows
}

#[tokio::test]
async fn an_inventory_configuration_round_trips_under_its_own_id() {
    let router = router();

    for (index, (case, wanted)) in round_trip_rows().into_iter().enumerate() {
        let name = format!("inventory-round-trip-{index}");
        bucket(&router, &name).await;

        let put = put_inventory(&router, &name, &wanted).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected the put to answer \
             {PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: a successful put carries no body, got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_inventory(&router, &name, &wanted.id).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the read to answer 200, got {} ({})",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let reported = reported_configuration(&read);
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the configuration that was put ({wanted:?}), got \
             {reported:?}"
        );

        // The payload is the whole configuration: a second put under the same id REPLACES
        // it, so the members this one omits — the filter, the optional fields, the
        // destination's owning account and encryption among them — are gone rather than
        // merged forward.
        let replacement = Configuration::new(
            &wanted.id,
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[1],
            INVENTORY_FREQUENCY_VALUES[1],
            Destination::new(INVENTORY_FORMAT_VALUES[0]),
        )
        .disabled();
        let answer = put_inventory(&router, &name, &replacement).await;
        assert_eq!(
            answer.status, PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected the replacing put to answer \
             {PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS}, got {}",
            answer.status
        );
        let after = reported_configuration(&get_inventory(&router, &name, &wanted.id).await);
        assert_eq!(
            after, replacement,
            "case {case:?}: expected the replacement ({replacement:?}), got {after:?}"
        );
    }
}

/// A configuration is addressable on its own: a put under one id neither changes nor hides
/// what another id carries, a put REPEATED under one id changes nothing at all, and a read of
/// an id the bucket carries nothing under reports that rather than somebody else's
/// configuration.
#[tokio::test]
async fn each_id_carries_its_own_configuration() {
    let router = router();
    let name = "inventory-per-id";
    bucket(&router, name).await;

    let stored: Vec<Configuration> = vec![
        Configuration::new(
            "logs",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[0]),
        )
        .filtered("logs/"),
        Configuration::new(
            "weekly-parquet",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[1],
            INVENTORY_FREQUENCY_VALUES[1],
            Destination::new(INVENTORY_FORMAT_VALUES[2]).under("weekly/"),
        )
        .with_fields(&[INVENTORY_OPTIONAL_FIELD_VALUES[1]]),
        Configuration::new(
            "foreign",
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[1])
                .owned_by(FOREIGN_ACCOUNT_ID)
                .encrypted(Encryption::Sses3),
        )
        .disabled(),
    ];
    for wanted in &stored {
        let put = put_inventory(&router, name, wanted).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
            "case {:?}: expected the put to answer \
             {PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            wanted.id,
            put.status,
            String::from_utf8_lossy(&put.body)
        );
    }

    // The SAME put again: an id already carrying this configuration keeps exactly it, so
    // repeating a put adds nothing to what the bucket carries.
    for wanted in &stored {
        let repeated = put_inventory(&router, name, wanted).await;
        assert_eq!(
            repeated.status,
            PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
            "case {:?}: expected a repeated put to answer \
             {PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            wanted.id,
            repeated.status,
            String::from_utf8_lossy(&repeated.body)
        );
    }

    for wanted in &stored {
        let read = get_inventory(&router, name, &wanted.id).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected the read to answer 200, got {} ({})",
            wanted.id,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let reported = reported_configuration(&read);
        assert_eq!(
            &reported, wanted,
            "case {:?}: expected each id to report its own configuration, got {reported:?}",
            wanted.id
        );
    }

    // Replacing ONE id leaves its neighbours exactly as they were.
    let replaced = Configuration::new(
        "weekly-parquet",
        INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
        INVENTORY_FREQUENCY_VALUES[0],
        Destination::new(INVENTORY_FORMAT_VALUES[0]),
    )
    .filtered("replaced/");
    let answer = put_inventory(&router, name, &replaced).await;
    assert_eq!(
        answer.status, PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
        "case \"replacing one of several ids\": expected \
         {PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS}, got {}",
        answer.status
    );
    for wanted in &stored {
        let expected = if wanted.id == replaced.id {
            &replaced
        } else {
            wanted
        };
        let reported = reported_configuration(&get_inventory(&router, name, &wanted.id).await);
        assert_eq!(
            &reported, expected,
            "case {:?}: expected a put under one id to leave the others alone, got \
             {reported:?}",
            wanted.id
        );
    }

    let absent = get_inventory(&router, name, "never-put").await;
    assert_eq!(
        absent.status, 404,
        "case \"an id the bucket carries nothing under\": expected 404, got {}",
        absent.status
    );
    assert_eq!(
        absent.error_code(),
        NO_SUCH_CONFIGURATION_CODE,
        "case \"an id the bucket carries nothing under\": expected \
         {NO_SUCH_CONFIGURATION_CODE}, got {}",
        absent.error_code()
    );
}

/// One row of the refusal table: the case name, the id in the query, the payload, the status
/// expected and the code expected, when the code is this handler's own rather than the
/// boundary validator's.
type RefusalRow = (&'static str, String, String, u16, Option<&'static str>);

/// A configuration whose `member` carries `value` — a value outside the enum's published
/// set, which describes no configuration S3 can apply.
fn with_member(id: &str, member: &str, value: &str) -> String {
    let mut body = Configuration::new(
        id,
        INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
        INVENTORY_FREQUENCY_VALUES[0],
        Destination::new(INVENTORY_FORMAT_VALUES[0]),
    )
    .xml();
    // Replace the published value this member carries with the one the row names.
    let (published, replacement) = match member {
        "IncludedObjectVersions" => (
            format!(
                "<IncludedObjectVersions>{}</IncludedObjectVersions>",
                INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0]
            ),
            format!("<IncludedObjectVersions>{value}</IncludedObjectVersions>"),
        ),
        "Frequency" => (
            format!("<Frequency>{}</Frequency>", INVENTORY_FREQUENCY_VALUES[0]),
            format!("<Frequency>{value}</Frequency>"),
        ),
        "Format" => (
            format!("<Format>{}</Format>", INVENTORY_FORMAT_VALUES[0]),
            format!("<Format>{value}</Format>"),
        ),
        "OptionalFields" => (
            "<Schedule>".to_string(),
            format!("<OptionalFields><Field>{value}</Field></OptionalFields><Schedule>"),
        ),
        other => panic!("the table names no member {other:?}"),
    };
    assert!(
        body.contains(&published),
        "case setup: the payload must carry {published:?}, got {body}"
    );
    body = body.replace(&published, &replacement);
    body
}

/// Every request the service cannot record is refused with the code AWS documents for it,
/// and leaves the configuration the bucket already carried in place.
#[tokio::test]
async fn a_configuration_s3_cannot_record_is_refused_and_changes_nothing() {
    let router = router();
    let name = "inventory-refusals";
    bucket(&router, name).await;

    let stored = Configuration::new(
        "already-here",
        INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
        INVENTORY_FREQUENCY_VALUES[0],
        Destination::new(INVENTORY_FORMAT_VALUES[0]).owned_by(FOREIGN_ACCOUNT_ID),
    )
    .filtered("kept/");
    let put = put_inventory(&router, name, &stored).await;
    assert_eq!(
        put.status,
        PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
        "case setup: expected the put to answer \
         {PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    // One character past the ceiling every bucket configuration id shares.
    let too_long = "i".repeat(MAX_INVENTORY_ID_CHARS + 1);
    let configuration_of = |id: &str| {
        Configuration::new(
            id,
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[0]),
        )
    };

    let rows: Vec<RefusalRow> = vec![
        (
            "an id one character past the 64-character ceiling",
            too_long.clone(),
            configuration_of(&too_long).xml(),
            400,
            Some(INVALID_ARGUMENT_CODE),
        ),
        (
            "an empty id, which identifies no configuration",
            String::new(),
            configuration_of("").xml(),
            400,
            Some(INVALID_ARGUMENT_CODE),
        ),
        (
            "a payload Id that is not the configuration the request names",
            "query-id".to_string(),
            configuration_of("payload-id").xml(),
            400,
            Some("IdMismatch"),
        ),
        (
            "an IncludedObjectVersions outside its enum",
            "bad-versions".to_string(),
            with_member("bad-versions", "IncludedObjectVersions", "Some"),
            400,
            None,
        ),
        (
            "a Frequency outside its enum",
            "bad-frequency".to_string(),
            with_member("bad-frequency", "Frequency", "Hourly"),
            400,
            None,
        ),
        (
            "a destination Format outside its enum",
            "bad-format".to_string(),
            with_member("bad-format", "Format", "JSON"),
            400,
            None,
        ),
        (
            "an optional field outside its enum",
            "bad-field".to_string(),
            with_member("bad-field", "OptionalFields", "ObjectTemperature"),
            400,
            None,
        ),
    ];

    for (case, id, payload, status, code) in rows {
        let answer = put_inventory_payload(&router, name, &id, &payload, &[]).await;
        assert_eq!(
            answer.status,
            status,
            "case {case:?}: expected HTTP {status}, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if let Some(code) = code {
            assert_eq!(
                answer.error_code(),
                code,
                "case {case:?}: expected the code {code}, got {}",
                answer.error_code()
            );
        }

        // The refusal recorded nothing: the configuration that WAS stored is unchanged.
        let reported = reported_configuration(&get_inventory(&router, name, &stored.id).await);
        assert_eq!(
            reported, stored,
            "case {case:?}: expected the stored configuration to survive the refusal, got \
             {reported:?}"
        );
    }

    // A bucket this emulator does not hold, on both sides of the round trip.
    for (case, answer) in [
        (
            "a put naming a bucket that does not exist",
            put_inventory(
                &router,
                "inventory-no-such-bucket",
                &configuration_of("any"),
            )
            .await,
        ),
        (
            "a read naming a bucket that does not exist",
            get_inventory(&router, "inventory-no-such-bucket", "any").await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {}",
            answer.status
        );
        assert_eq!(
            answer.error_code(),
            NO_SUCH_BUCKET_CODE,
            "case {case:?}: expected the code {NO_SUCH_BUCKET_CODE}, got {}",
            answer.error_code()
        );
    }

    // An expected bucket owner that is not this account's, on both sides.
    let not_the_owner = [("x-amz-expected-bucket-owner", FOREIGN_ACCOUNT_ID)];
    for (case, answer) in [
        (
            "a put naming another account as the expected bucket owner",
            put_inventory_payload(
                &router,
                name,
                "owner-mismatch",
                &configuration_of("owner-mismatch").xml(),
                &not_the_owner,
            )
            .await,
        ),
        (
            "a read naming another account as the expected bucket owner",
            get_inventory_with_headers(&router, name, &stored.id, &not_the_owner).await,
        ),
    ] {
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected HTTP 403, got {}",
            answer.status
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {case:?}: expected the code AccessDenied, got {}",
            answer.error_code()
        );
    }

    // Nothing any refusal above named reached the store.
    for id in [
        "query-id",
        "payload-id",
        "bad-versions",
        "bad-frequency",
        "bad-format",
        "bad-field",
        "owner-mismatch",
    ] {
        let read = get_inventory(&router, name, id).await;
        assert_eq!(
            read.error_code(),
            NO_SUCH_CONFIGURATION_CODE,
            "case {id:?}: a refused put records nothing, so the read must answer \
             {NO_SUCH_CONFIGURATION_CODE}, got {} ({})",
            read.error_code(),
            read.status
        );
    }
}

/// "You can have up to 1,000 inventory configurations per bucket": the 1,001st NEW
/// configuration is refused with the operation's own `TooManyConfigurations`, while replacing
/// one of the 1,000 creates none and still answers
/// [`PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS`].
#[tokio::test]
async fn one_configuration_past_the_per_bucket_limit_is_refused() {
    let router = router();
    let name = "inventory-at-the-limit";
    bucket(&router, name).await;

    let configuration_of = |id: &str| {
        Configuration::new(
            id,
            INVENTORY_INCLUDED_OBJECT_VERSIONS_VALUES[0],
            INVENTORY_FREQUENCY_VALUES[0],
            Destination::new(INVENTORY_FORMAT_VALUES[0]),
        )
    };

    for index in 0..MAX_INVENTORY_CONFIGURATIONS_PER_BUCKET {
        let id = format!("configuration-{index}");
        let answer = put_inventory(&router, name, &configuration_of(&id)).await;
        assert_eq!(
            answer.status,
            PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
            "case {id:?}: a configuration within the limit must be recorded, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    let past = put_inventory(&router, name, &configuration_of("one-too-many")).await;
    assert_eq!(
        past.status, 400,
        "case \"one configuration past the limit\": expected HTTP 400, got {}",
        past.status
    );
    assert_eq!(
        past.error_code(),
        TOO_MANY_CONFIGURATIONS_CODE,
        "case \"one configuration past the limit\": expected the code \
         {TOO_MANY_CONFIGURATIONS_CODE}, got {}",
        past.error_code()
    );

    // A REPLACEMENT at the ceiling creates no new configuration, so the limit does not
    // refuse it.
    let replacement = configuration_of("configuration-0").filtered("replaced/");
    let answer = put_inventory(&router, name, &replacement).await;
    assert_eq!(
        answer.status,
        PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
        "case \"replacing a configuration at the limit\": expected HTTP \
         {PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let reported = reported_configuration(&get_inventory(&router, name, &replacement.id).await);
    assert_eq!(
        reported, replacement,
        "case \"replacing a configuration at the limit\": expected the replacing \
         configuration, got {reported:?}"
    );
}
