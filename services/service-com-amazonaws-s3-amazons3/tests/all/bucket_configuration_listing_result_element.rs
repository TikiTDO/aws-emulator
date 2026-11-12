// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DOCUMENT a bucket-configuration LISTING answers with: the
//! element its result is rooted in, and the configurations that element carries.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — the response
//! document a client parses — rather than any handler's internals.
//!
//! The rule under test: a client reads a listing's result out of the element the service
//! SENDS, so each of the four id-addressed configuration listings answers a document rooted
//! in its own `List…Result` element, carrying one entry per configuration the bucket holds
//! — before any put (none), after two puts (both, in ascending id order) and after one of
//! them is deleted (the survivor). A root element the model resolves to something else (the
//! analytics listing's output shape declares the singular
//! `ListBucketAnalyticsConfigurationResult` and the S3 Intelligent-Tiering listing's shape
//! declares no `xmlName` at all, so the generic resolution is the shape's own name) is no
//! element S3 sends, and a client walking the document for its result finds nothing under
//! it.
//!
//! The expected names are the four the service serves:
//! `ListBucketAnalyticsConfigurationsResult`, `ListMetricsConfigurationsResult`,
//! `ListInventoryConfigurationsResult` and `ListIntelligentTieringConfigurationsResult` —
//! the last two as their own output shapes' `xmlName` declares them
//! (`smithy_query(query="shape",shape="ListBucketMetricsConfigurationsOutput")` and
//! `…="ListBucketInventoryConfigurationsOutput"`).
//!
//! Each entry's element is the family's flattened list member name —
//! `AnalyticsConfigurationList` is `xmlFlattened` with `xmlName` `AnalyticsConfiguration`
//! and its three siblings likewise
//! (`smithy_query(query="operation",operation="ListBucketAnalyticsConfigurations")`), so a
//! page carrying no configuration carries no entry element at all.

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::ops::put_bucket_analytics_configuration::PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::ops::put_bucket_intelligent_tiering_configuration::PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::ops::put_bucket_inventory_configuration::PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::ops::put_bucket_metrics_configuration::PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_analytics_configuration::{bucket, bucket_arn, configuration, DataExport, Filter};
use super::bucket_lifecycle::{router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The bucket an analytics or S3 Inventory export names as its destination.
const EXPORT_DESTINATION: &str = "configuration-listing-destination";

/// The two ids every family's bucket carries, in the order a listing reports them: ascending.
const IDS: [&str; 2] = ["alpha", "beta"];

/// One id-addressed configuration family, as this suite's table names it.
struct Family {
    /// The subresource query parameter that names the family.
    subresource: &'static str,
    /// The element the family's listing result is rooted in, as the service sends it.
    result_element: &'static str,
    /// The element one reported configuration occupies: the family's flattened list member.
    entry_element: &'static str,
    /// A document the family's `PUT` accepts for the id given.
    document: fn(&str) -> String,
    /// The status a successful put of one of this family's configurations answers.
    put_status: u16,
}

/// The four families a bucket holds per-id configurations of.
///
/// The rows differ along what the rule generalises over — which family the listing belongs
/// to, and so which element its result is rooted in and which element one entry occupies.
/// The recorded call that filed this is the S3 Intelligent-Tiering row.
fn families() -> Vec<Family> {
    vec![
        Family {
            subresource: "analytics",
            result_element: "ListBucketAnalyticsConfigurationsResult",
            entry_element: "AnalyticsConfiguration",
            document: analytics_document,
            put_status: PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
        },
        Family {
            subresource: "metrics",
            result_element: "ListMetricsConfigurationsResult",
            entry_element: "MetricsConfiguration",
            document: metrics_document,
            put_status: PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS,
        },
        Family {
            subresource: "inventory",
            result_element: "ListInventoryConfigurationsResult",
            entry_element: "InventoryConfiguration",
            document: inventory_document,
            put_status: PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
        },
        Family {
            subresource: "intelligent-tiering",
            result_element: "ListIntelligentTieringConfigurationsResult",
            entry_element: "IntelligentTieringConfiguration",
            document: intelligent_tiering_document,
            put_status: PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
        },
    ]
}

/// An `AnalyticsConfiguration` for `id`, spelled by the writer the analytics suite owns.
fn analytics_document(id: &str) -> String {
    configuration(
        id,
        Some(Filter::Prefix(format!("{id}/"))),
        Some(DataExport::new(EXPORT_DESTINATION, None, None)),
    )
    .xml()
}

/// A `MetricsConfiguration` for `id`: the id alone, which is every member the shape requires
/// (`smithy_query(query="shape",shape="MetricsConfiguration")`).
fn metrics_document(id: &str) -> String {
    format!("<MetricsConfiguration xmlns=\"{XMLNS}\"><Id>{id}</Id></MetricsConfiguration>")
}

/// An `InventoryConfiguration` for `id`, carrying every member the shape requires
/// (`smithy_query(query="shape",shape="InventoryConfiguration")`).
fn inventory_document(id: &str) -> String {
    let destination = bucket_arn(EXPORT_DESTINATION);
    format!(
        "<InventoryConfiguration xmlns=\"{XMLNS}\"><Destination><S3BucketDestination>\
         <Bucket>{destination}</Bucket><Format>CSV</Format></S3BucketDestination></Destination>\
         <IsEnabled>true</IsEnabled><Id>{id}</Id>\
         <IncludedObjectVersions>All</IncludedObjectVersions><Schedule>\
         <Frequency>Daily</Frequency></Schedule></InventoryConfiguration>"
    )
}

/// An `IntelligentTieringConfiguration` for `id`, carrying every member the shape requires
/// (`smithy_query(query="shape",shape="IntelligentTieringConfiguration")`); 90 days is the
/// fewest `Tiering$Days` documents for the Archive Access tier.
fn intelligent_tiering_document(id: &str) -> String {
    format!(
        "<IntelligentTieringConfiguration xmlns=\"{XMLNS}\"><Id>{id}</Id>\
         <Status>Enabled</Status><Tiering><Days>90</Days>\
         <AccessTier>ARCHIVE_ACCESS</AccessTier></Tiering></IntelligentTieringConfiguration>"
    )
}

/// Every id-addressed configuration listing answers a document rooted in the element the
/// service sends, and that element carries exactly the configurations the bucket holds —
/// through a put, a second put and a delete.
#[tokio::test]
async fn every_configuration_listing_is_rooted_in_the_element_the_service_sends() {
    let router = router_for_listings().await;

    for family in families() {
        let name = bucket_name(family.subresource);
        let case = format!("?{} on {name:?}", family.subresource);

        // Nothing put yet: the result element is still the one the service sends, and it
        // carries no entry at all.
        let empty = listing(&router, &name, family.subresource).await;
        assert_listing(&empty, &format!("{case} before any put"), &family, &[]);

        for id in IDS {
            let put = send(
                &router,
                "PUT",
                &format!("/{name}?{}&id={id}", family.subresource),
                &[],
                &(family.document)(id),
            )
            .await;
            assert_eq!(
                put.status,
                family.put_status,
                "case {case:?} ({id}): expected the put to be recorded with {}, got {} ({})",
                family.put_status,
                put.status,
                String::from_utf8_lossy(&put.body)
            );
        }

        // Both puts are visible, in ascending id order.
        let both = listing(&router, &name, family.subresource).await;
        assert_listing(&both, &format!("{case} after two puts"), &family, &IDS);

        // And the delete of one leaves the other listable.
        let deleted = send(
            &router,
            "DELETE",
            &format!("/{name}?{}&id={}", family.subresource, IDS[0]),
            &[],
            "",
        )
        .await;
        assert_eq!(
            deleted.status,
            204,
            "case {case:?}: expected the delete to answer 204, got {} ({})",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        let survivor = listing(&router, &name, family.subresource).await;
        assert_listing(
            &survivor,
            &format!("{case} after one delete"),
            &family,
            &[IDS[1]],
        );
    }
}

/// A listing REPORTS: a second identical listing of an unchanged bucket answers the same
/// document, root element included.
#[tokio::test]
async fn a_second_listing_of_an_unchanged_bucket_answers_the_same_document() {
    let router = router_for_listings().await;

    for family in families() {
        let name = bucket_name(family.subresource);
        let case = format!("?{} on {name:?}", family.subresource);
        let put = send(
            &router,
            "PUT",
            &format!("/{name}?{}&id={}", family.subresource, IDS[0]),
            &[],
            &(family.document)(IDS[0]),
        )
        .await;
        assert_eq!(
            put.status,
            family.put_status,
            "case {case:?}: expected the put to be recorded with {}, got {} ({})",
            family.put_status,
            put.status,
            String::from_utf8_lossy(&put.body)
        );

        let first = listing(&router, &name, family.subresource).await;
        let again = listing(&router, &name, family.subresource).await;
        assert_listing(&first, &format!("{case} read once"), &family, &[IDS[0]]);
        assert_eq!(
            document_of(&again),
            document_of(&first),
            "case {case:?}: a listing reports, so a second one must answer the same document, \
             got {:?} after {:?}",
            document_of(&again),
            document_of(&first)
        );
    }
}

/// One router with a bucket per family already standing.
async fn router_for_listings() -> MultiServiceRouter {
    let router = router();
    for family in families() {
        bucket(&router, &bucket_name(family.subresource)).await;
    }
    router
}

/// The bucket a family's configurations are put on: one per family, so a listing of one
/// family can never report another's.
fn bucket_name(subresource: &str) -> String {
    format!("configuration-listing-{subresource}")
}

/// `GET /<bucket>?<subresource>` with no id — the family's LISTING.
async fn listing(router: &MultiServiceRouter, bucket: &str, subresource: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?{subresource}"), &[], "").await
}

/// What `answer` carried, as text.
fn document_of(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).into_owned()
}

/// `answer` is a 200 whose document is rooted in `family`'s result element and carries one
/// entry per id in `expected`, in that order, with `IsTruncated` false.
fn assert_listing(answer: &Answer, case: &str, family: &Family, expected: &[&str]) {
    let document = document_of(answer);
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the listing to answer 200, got {} ({document})",
        answer.status
    );
    assert_eq!(
        root_element(&document).as_deref(),
        Some(family.result_element),
        "case {case:?}: expected the result element <{}>, got {:?} ({document})",
        family.result_element,
        root_element(&document)
    );
    assert_eq!(
        answer.element("IsTruncated").as_deref(),
        Some("false"),
        "case {case:?}: a page reporting every configuration is not truncated, got {:?}",
        answer.element("IsTruncated")
    );
    let entries = document.matches(&format!("<{}>", family.entry_element)).count();
    assert_eq!(
        entries,
        expected.len(),
        "case {case:?}: expected {} <{}> entr(y/ies), got {entries} ({document})",
        expected.len(),
        family.entry_element
    );
    let reported = answer.elements("Id");
    assert_eq!(
        reported,
        expected.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "case {case:?}: expected the listing to report {expected:?}, got {reported:?}"
    );
}

/// The name of the FIRST element of `document` — the root a client walks the result out of —
/// or `None` for a body that carries no element at all.
fn root_element(document: &str) -> Option<String> {
    let mut reader = Reader::from_str(document);
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) | Ok(Event::Empty(start)) => {
                return Some(String::from_utf8_lossy(start.name().as_ref()).into_owned())
            }
            Ok(Event::Eof) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}
