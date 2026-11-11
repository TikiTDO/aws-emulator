// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a bucket's ID-ADDRESSED configuration subresources are
//! ADDRESSED: which call a request on one of them is served as, and which ids identify a
//! configuration at all.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code and message — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * A request that names a bucket SUBRESOURCE is a call on that subresource, never the
//!   bucket-level operation bound to the bare path. The model binds each of the four
//!   id-addressed configuration families at `/{Bucket}?<subresource>` with `Id` as a required
//!   `@httpQuery("id")` member (`smithy_query(query="operation",
//!   operation="PutBucketAnalyticsConfiguration")`; `OPERATIONS.md` restates the same binding
//!   for the metrics, S3 Inventory and S3 Intelligent-Tiering families), and `PUT`/`DELETE`
//!   `/{Bucket}` with no subresource is `CreateBucket`/`DeleteBucket` — so a subresource call
//!   that states no id must be refused for the id it did not state and must neither create
//!   nor delete the bucket it addressed. A `GET` with no id, or with an empty one, is that
//!   subresource's LISTING, which is the operation the model binds at the same method and
//!   path with no id.
//! * An id identifies the configuration, so a value that is not one identifies nothing: an
//!   id the request states empty (or does not state at all) is refused with
//!   `InvalidConfigurationId` / "The specified configuration id is invalid." on the write,
//!   the read and the delete alike, since none of the three identifies a configuration at
//!   all. The PATTERN and the LENGTH bound are a different rule: they bound what a WRITE
//!   may record, so an id spelled with a character outside [`CONFIGURATION_ID_PATTERN`] is
//!   refused `InvalidConfigId` quoting that pattern and one longer than
//!   [`MAX_CONFIGURATION_ID_CHARS`] `InvalidConfigId` stating that bound — on the PUT only,
//!   while the read and the delete report what the bucket holds under that id, which for an
//!   id no put could have recorded is `NoSuchConfiguration`.
//! * An id the rule admits round-trips: the put records the configuration under it
//!   ([`PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS`] with an empty body), the read
//!   reports it back, and the listing names it in its own result element,
//!   `<ListBucketAnalyticsConfigurationsResult>`.

use service_com_amazonaws_s3_amazons3::ops::put_bucket_analytics_configuration::PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::state::{
    CONFIGURATION_ID_PATTERN, INVALID_CONFIGURATION_ID_CODE, INVALID_CONFIGURATION_ID_MESSAGE,
    INVALID_CONFIG_ID_CODE, INVALID_CONFIG_ID_MESSAGE, INVALID_CONFIG_ID_PATTERN_MESSAGE,
    MAX_CONFIGURATION_ID_CHARS, NO_SUCH_CONFIGURATION_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_analytics_configuration::{
    bucket, bucket_arn, configuration, get_analytics, put_analytics, reported_configurations,
    Configuration, DataExport, Filter,
};
use super::bucket_lifecycle::{head_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The element a `ListBucketAnalyticsConfigurations` response is rooted in, as the service
/// answers it.
const ANALYTICS_LISTING_RESULT: &str = "ListBucketAnalyticsConfigurationsResult";

/// One id-addressed configuration family: the subresource query parameter that names it and
/// a well-formed document for its `PUT`, so a refusal of such a call is the ID's and not the
/// document's.
struct Family {
    subresource: &'static str,
    document: String,
}

/// The four families a bucket holds per-id configurations of, each with a document its own
/// `PUT` accepts when the request states an id.
///
/// The documents are the minimal ones each shape requires
/// (`smithy_query(query="shape",shape="MetricsConfiguration")` and its siblings); the id
/// inside each is `ADDRESSED_ID`, which is the id the stating rows name.
fn families() -> Vec<Family> {
    let destination = bucket_arn("configuration-destination");
    vec![
        Family {
            subresource: "analytics",
            document: configuration(
                ADDRESSED_ID,
                Some(Filter::Prefix("logs/".to_string())),
                Some(DataExport::new("configuration-destination", None, None)),
            )
            .xml(),
        },
        Family {
            subresource: "metrics",
            document: format!(
                "<MetricsConfiguration xmlns=\"{XMLNS}\"><Id>{ADDRESSED_ID}</Id>\
                 </MetricsConfiguration>"
            ),
        },
        Family {
            subresource: "inventory",
            document: format!(
                "<InventoryConfiguration xmlns=\"{XMLNS}\"><Destination><S3BucketDestination>\
                 <Bucket>{destination}</Bucket><Format>CSV</Format></S3BucketDestination>\
                 </Destination><IsEnabled>true</IsEnabled><Id>{ADDRESSED_ID}</Id>\
                 <IncludedObjectVersions>All</IncludedObjectVersions><Schedule>\
                 <Frequency>Daily</Frequency></Schedule></InventoryConfiguration>"
            ),
        },
        Family {
            subresource: "intelligent-tiering",
            document: format!(
                "<IntelligentTieringConfiguration xmlns=\"{XMLNS}\"><Id>{ADDRESSED_ID}</Id>\
                 <Status>Enabled</Status><Tiering><Days>90</Days>\
                 <AccessTier>ARCHIVE_ACCESS</AccessTier></Tiering>\
                 </IntelligentTieringConfiguration>"
            ),
        },
    ]
}

/// The id every family's document names, and the one a request that DOES state an id states.
const ADDRESSED_ID: &str = "addressed";

/// `<METHOD> /<bucket>?<subresource>` with no `id` parameter at all.
async fn without_id(
    router: &MultiServiceRouter,
    method: &str,
    bucket: &str,
    subresource: &str,
    body: &str,
) -> Answer {
    send(
        router,
        method,
        &format!("/{bucket}?{subresource}"),
        &[],
        body,
    )
    .await
}

/// `<METHOD> /<bucket>?<subresource>&id=<id>`.
async fn with_id(
    router: &MultiServiceRouter,
    method: &str,
    bucket: &str,
    subresource: &str,
    id: &str,
    body: &str,
) -> Answer {
    send(
        router,
        method,
        &format!("/{bucket}?{subresource}&id={id}"),
        &[],
        body,
    )
    .await
}

/// A write or a delete that names a configuration subresource but no id is that
/// subresource's call with an id missing: it is refused for the id, and it neither creates
/// the bucket it addressed nor deletes it.
#[tokio::test]
async fn a_subresource_call_without_an_id_is_not_a_call_on_the_bucket() {
    let router = router();
    let held = "configuration-addressing-held";
    bucket(&router, held).await;
    // One configuration the bucket really carries, so a delete that reached the BUCKET
    // instead of the subresource would be visible as the loss of this record too.
    let stored = configuration(
        "kept",
        Some(Filter::Prefix("kept/".to_string())),
        Some(DataExport::new("configuration-destination", None, None)),
    );
    let setup = put_analytics(&router, held, &stored).await;
    assert_eq!(
        setup.status,
        PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
        "case setup: the stored configuration must be recorded, got {} ({})",
        setup.status,
        String::from_utf8_lossy(&setup.body)
    );

    // A name this emulator holds NO bucket under, so a `PUT` that reached `CreateBucket`
    // would be visible as a bucket coming into being.
    let unheld = "configuration-addressing-unheld";

    for family in families() {
        for (method, body, named) in [
            ("PUT", family.document.clone(), held),
            ("PUT", family.document.clone(), unheld),
            ("DELETE", String::new(), held),
            ("DELETE", String::new(), unheld),
        ] {
            let case = format!("{method} ?{} on {named:?}", family.subresource);
            let answer = without_id(&router, method, named, family.subresource, &body).await;
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: expected HTTP 400, got {} ({})",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                INVALID_CONFIGURATION_ID_CODE,
                "case {case:?}: expected the code {INVALID_CONFIGURATION_ID_CODE}, got {}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                INVALID_CONFIGURATION_ID_MESSAGE,
                "case {case:?}: expected the message {INVALID_CONFIGURATION_ID_MESSAGE:?}, got \
                 {:?}",
                answer.error_message()
            );
        }
    }

    // The bucket that was addressed is still there, and still carries what it carried.
    let head = head_bucket(&router, held).await;
    assert_eq!(
        head.status, 200,
        "case \"the addressed bucket\": a subresource call must not delete it, got {}",
        head.status
    );
    let read = get_analytics(&router, held, &stored.id).await;
    assert_eq!(
        reported_configurations(&read),
        vec![stored.clone()],
        "case \"the stored configuration\": a subresource call that stated no id must leave it \
         alone, got {:?}",
        reported_configurations(&read)
    );

    // And no bucket came into being behind the refused writes.
    let absent = head_bucket(&router, unheld).await;
    assert_eq!(
        absent.status, 404,
        "case \"the unheld bucket\": a subresource call must not create it, got {}",
        absent.status
    );
}

/// A READ of a configuration subresource that identifies no configuration — no `id` at all,
/// or an empty one — is that subresource's listing, which reports what the puts recorded.
#[tokio::test]
async fn a_read_without_an_id_is_the_subresource_listing() {
    let router = router();
    let name = "configuration-addressing-listing";
    bucket(&router, name).await;
    let stored: Vec<Configuration> = vec![
        configuration("alpha", Some(Filter::Prefix("alpha/".to_string())), None),
        configuration(
            "beta",
            Some(Filter::Tag("team".to_string(), "beta".to_string())),
            Some(DataExport::new("configuration-destination", None, None)),
        ),
    ];
    for wanted in &stored {
        let put = put_analytics(&router, name, wanted).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
            "case setup {:?}: expected the put to be recorded, got {} ({})",
            wanted.id,
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case setup {:?}: a recorded put carries no body, got {:?}",
            wanted.id,
            String::from_utf8_lossy(&put.body)
        );
    }

    // The two ways a read can identify no configuration: no `id` parameter, and one stated
    // empty. Both are the listing.
    for (case, uri) in [
        ("no id parameter", format!("/{name}?analytics")),
        ("an id stated empty", format!("/{name}?analytics&id=")),
        ("a bare id flag", format!("/{name}?analytics&id")),
    ] {
        let answer = send(&router, "GET", &uri, &[], "").await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected the listing to answer 200, got {} ({})",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let body = String::from_utf8_lossy(&answer.body).into_owned();
        assert!(
            body.contains(&format!("<{ANALYTICS_LISTING_RESULT}")),
            "case {case:?}: expected the result element <{ANALYTICS_LISTING_RESULT}>, got {body:?}"
        );
        let reported: Vec<String> = reported_configurations(&answer)
            .into_iter()
            .map(|configuration| configuration.id)
            .collect();
        assert_eq!(
            reported,
            vec!["alpha".to_string(), "beta".to_string()],
            "case {case:?}: expected the listing to report every stored configuration, got \
             {reported:?}"
        );
    }
}

/// One row of the id table: the case, the id the request states, and what the three calls
/// that address a configuration by it must answer — `None` for an id the rule admits.
///
/// The last field says whether a READ of that id is the subresource's LISTING rather than a
/// refusal: an id stated EMPTY identifies no configuration, so the read that names it is the
/// bucket's analytics listing (the write and the delete, which have no listing bound at that
/// method, are refused).
type IdRow = (
    &'static str,
    String,
    Option<(&'static str, &'static str)>,
    bool,
);

/// The ids every call that addresses an analytics configuration is judged by.
///
/// The rows differ along what the id rule generalises over: which characters the id holds
/// (letters, digits and each of the three punctuation marks
/// [`CONFIGURATION_ID_PATTERN`]'s class admits, and one character outside it per kind —
/// a space, a tilde, a slash, a colon and a non-ASCII letter), and how LONG it is (one
/// character, exactly [`MAX_CONFIGURATION_ID_CHARS`], one past it, and empty). The recorded
/// call that filed this is one row among them.
fn id_rows() -> Vec<IdRow> {
    let pattern = Some((INVALID_CONFIG_ID_CODE, INVALID_CONFIG_ID_PATTERN_MESSAGE));
    let length = Some((INVALID_CONFIG_ID_CODE, INVALID_CONFIG_ID_MESSAGE));
    let unstated = Some((
        INVALID_CONFIGURATION_ID_CODE,
        INVALID_CONFIGURATION_ID_MESSAGE,
    ));
    vec![
        ("letters and digits", "export7".to_string(), None, false),
        ("a dash", "daily-export".to_string(), None, false),
        ("an underscore", "daily_export".to_string(), None, false),
        ("a period", "daily.export".to_string(), None, false),
        ("one character", "x".to_string(), None, false),
        (
            "exactly the published maximum",
            "x".repeat(MAX_CONFIGURATION_ID_CHARS),
            None,
            false,
        ),
        (
            "one character past the published maximum",
            "x".repeat(MAX_CONFIGURATION_ID_CHARS + 1),
            length,
            false,
        ),
        (
            "a percent-encoded space",
            "has%20space".to_string(),
            pattern,
            false,
        ),
        ("a tilde", "has~tilde".to_string(), pattern, false),
        (
            "a percent-encoded slash",
            "has%2Fslash".to_string(),
            pattern,
            false,
        ),
        ("a colon", "has:colon".to_string(), pattern, false),
        (
            "a percent-encoded non-ASCII letter",
            "caf%C3%A9".to_string(),
            pattern,
            false,
        ),
        ("an empty id", String::new(), unstated, true),
    ]
}

/// Every call that addresses an analytics configuration judges the id by ONE rule: an id the
/// rule admits round-trips through the write, the read and the listing, and one it does not
/// is refused by all three with the code and the published sentence for the rule it broke —
/// having recorded, reported and removed nothing.
#[tokio::test]
async fn one_id_rule_governs_the_write_the_read_and_the_delete() {
    let router = router();
    let name = "configuration-addressing-ids";
    bucket(&router, name).await;

    for (case, id, refusal, reads_as_listing) in id_rows() {
        // The payload names the id the request states, so a refusal is about the id itself
        // and never about the two disagreeing. A percent-encoded id is spelled decoded in
        // the document, which is where the id a client sent actually reads.
        let in_document = percent_decoded(&id);
        let document = configuration(
            &in_document,
            None,
            Some(DataExport::new("configuration-destination", None, None)),
        );
        let put = with_id(&router, "PUT", name, "analytics", &id, &document.xml()).await;
        let read = with_id(&router, "GET", name, "analytics", &id, "").await;
        let deleted = with_id(&router, "DELETE", name, "analytics", &id, "").await;

        match refusal {
            None => {
                assert_eq!(
                    put.status,
                    PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
                    "case {case:?}: expected the put to answer \
                     {PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
                    put.status,
                    String::from_utf8_lossy(&put.body)
                );
                assert_eq!(
                    read.status,
                    200,
                    "case {case:?}: expected the read to answer 200, got {} ({})",
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
                assert_eq!(
                    reported_configurations(&read),
                    vec![document.clone()],
                    "case {case:?}: expected the configuration that was put ({document:?}), got \
                     {:?}",
                    reported_configurations(&read)
                );
                assert_eq!(
                    deleted.status,
                    204,
                    "case {case:?}: expected the delete to answer 204, got {} ({})",
                    deleted.status,
                    String::from_utf8_lossy(&deleted.body)
                );
                // And the id is gone: the read of it now reports no such configuration.
                let after = with_id(&router, "GET", name, "analytics", &id, "").await;
                assert_eq!(
                    after.error_code(),
                    NO_SUCH_CONFIGURATION_CODE,
                    "case {case:?}: expected the deleted id to answer \
                     {NO_SUCH_CONFIGURATION_CODE}, got {} ({})",
                    after.error_code(),
                    after.status
                );
            }
            Some((code, message)) => {
                let refused: Vec<(&str, &Answer)> = match reads_as_listing {
                    // The read of an id that identifies nothing is the LISTING, so only the
                    // write and the delete are refused — and the read answers the bucket's
                    // own listing, which the suite above pins in full.
                    true => {
                        assert_eq!(
                            read.status,
                            200,
                            "case {case:?}: a read naming no configuration is the listing, got \
                             {} ({})",
                            read.status,
                            String::from_utf8_lossy(&read.body)
                        );
                        vec![("put", &put), ("delete", &deleted)]
                    }
                    false => {
                        // The id rule bounds what a PUT may RECORD. A read and a delete name
                        // a configuration the bucket either carries or does not, so they
                        // report the not-there refusal rather than judging the id.
                        for (call, answer) in [("read", &read), ("delete", &deleted)] {
                            assert_eq!(
                                answer.error_code(),
                                NO_SUCH_CONFIGURATION_CODE,
                                "case {case:?} ({call}): a read and a delete report only what \
                                 the bucket holds; expected \
                                 {NO_SUCH_CONFIGURATION_CODE}, got {} ({})",
                                answer.error_code(),
                                answer.status
                            );
                        }
                        vec![("put", &put)]
                    }
                };
                for (call, answer) in refused {
                    assert_eq!(
                        answer.status,
                        400,
                        "case {case:?} ({call}): expected HTTP 400, got {} ({})",
                        answer.status,
                        String::from_utf8_lossy(&answer.body)
                    );
                    assert_eq!(
                        answer.error_code(),
                        code,
                        "case {case:?} ({call}): expected the code {code}, got {}",
                        answer.error_code()
                    );
                    assert_eq!(
                        answer.error_message(),
                        message,
                        "case {case:?} ({call}): expected the message {message:?}, got {:?}",
                        answer.error_message()
                    );
                }
            }
        }
    }

    // Nothing any refused id left behind: the bucket's listing is empty again.
    let listing = send(&router, "GET", &format!("/{name}?analytics"), &[], "").await;
    let reported: Vec<String> = reported_configurations(&listing)
        .into_iter()
        .map(|configuration| configuration.id)
        .collect();
    assert!(
        reported.is_empty(),
        "case \"after the table\": every id was either deleted or refused, so the listing must \
         be empty, got {reported:?}"
    );
}

/// The sentence the pattern refusal carries quotes the pattern an id has to match, so the
/// rule a client reads and the rule the service enforces cannot drift apart.
#[tokio::test]
async fn the_pattern_refusal_quotes_the_pattern() {
    assert!(
        INVALID_CONFIG_ID_PATTERN_MESSAGE.ends_with(CONFIGURATION_ID_PATTERN),
        "the pattern sentence must quote {CONFIGURATION_ID_PATTERN:?}, got {:?}",
        INVALID_CONFIG_ID_PATTERN_MESSAGE
    );
}

/// `value` with the percent-escapes these rows use decoded, which is the value a client
/// spelled and the one the request document carries.
///
/// Only the escapes the table above names are decoded: the rows are written by hand, so this
/// is a reader for them rather than a general percent-decoder.
fn percent_decoded(value: &str) -> String {
    value
        .replace("%20", " ")
        .replace("%2F", "/")
        .replace("%C3%A9", "é")
}
