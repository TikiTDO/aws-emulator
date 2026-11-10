// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `CreationDate` a `ListBuckets` reports, which depends on the
//! endpoint the listing was served by.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`] owns), addressed to one Region's endpoint and signed for it —
//! which is how the emulator reads WHICH endpoint a client reached.
//!
//! The rules under test:
//!
//! * Recorded behavior: the endpoint of `us-east-1` — the Region a request that names no
//!   location constraint is signed for ("the signature calculations in Signature Version 4
//!   must use us-east-1 as the Region", `CreateBucket`) — reports the instant the bucket was
//!   created, and that instant never moves.
//! * Every OTHER Region's endpoint reports the instant that Region's record of the bucket was
//!   last written, so writing ANY bucket configuration advances the date the regional view
//!   reports while leaving the global one where it is.
//! * A bucket nothing has been configured on has only the one instant to report, whichever
//!   endpoint asks — the subresources a create installs come into being with the bucket.

use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_s3_amazons3::state::DEFAULT_BUCKET_REGION;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send_in_region};
use super::object_and_tagging_roundtrip::text;

/// A Region whose endpoint is not the one every creation date is recorded against.
const OTHER_REGION: &str = "us-west-2";

/// The `CreationDate` the endpoint of `region` reports for the bucket named `bucket`.
///
/// Asked with a `prefix` that names the one bucket, so the listing carries exactly its entry
/// and the first `CreationDate` in the document is that bucket's.
async fn creation_date(router: &MultiServiceRouter, region: &str, bucket: &str) -> String {
    let listing = send_in_region(
        router,
        region,
        "GET",
        &format!("/?prefix={bucket}"),
        &[],
        "",
    )
    .await;
    assert_eq!(
        listing.status,
        200,
        "the {region} view of {bucket:?} must list it, got {} with body {:?}",
        listing.status,
        text(&listing)
    );
    let document = text(&listing);
    assert!(
        document.contains(&format!("<Name>{bucket}</Name>")),
        "the {region} view must report the bucket {bucket:?}, got {document:?}"
    );
    listing
        .element("CreationDate")
        .unwrap_or_else(|| panic!("the {region} view must report a CreationDate, got {document:?}"))
}

/// The instant a reported `CreationDate` names, as `(seconds, nanoseconds)` — compared
/// rather than the rendered text, because a renderer may trim a fractional zero and two
/// instants an instant apart would then not order by their spelling.
///
/// `pub(crate)`: every suite that orders two reported instants reads them through this one
/// parser (the multipart initiation-order suite,
/// [`super::multipart_upload_initiation_order`], among them) rather than taking the
/// timestamp apart itself.
pub(crate) fn instant_of(reported: &str) -> (i64, u32) {
    let date = DateTime::from_str(reported, Format::DateTime)
        .unwrap_or_else(|_| panic!("a reported instant must be a date-time, got {reported:?}"));
    (date.secs(), date.subsec_nanos())
}

/// One row of the configuration-write table: the case, and the request that writes it.
struct WriteRow {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    payload: &'static str,
}

/// The bucket-configuration writes the rule spans.
///
/// The rows differ along WHICH subresource is written (a tag set, a versioning state, an
/// encryption configuration, a CORS configuration, a lifecycle configuration, an accelerate
/// state and a request-payment configuration) and along the kind of write it is (associating
/// a configuration, replacing one, and removing one) — the rule is about a configuration
/// write of any kind, not about one operation.
fn write_rows() -> Vec<WriteRow> {
    vec![
        WriteRow {
            case: "a tag set",
            method: "PUT",
            path: "?tagging",
            payload: "<Tagging xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><TagSet>\
                      <Tag><Key>team</Key><Value>data</Value></Tag></TagSet></Tagging>",
        },
        WriteRow {
            case: "a replacement tag set",
            method: "PUT",
            path: "?tagging",
            payload: "<Tagging xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><TagSet>\
                      <Tag><Key>team</Key><Value>platform</Value></Tag></TagSet></Tagging>",
        },
        WriteRow {
            case: "a versioning state",
            method: "PUT",
            path: "?versioning",
            payload: "<VersioningConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                      <Status>Enabled</Status></VersioningConfiguration>",
        },
        WriteRow {
            case: "an encryption configuration",
            method: "PUT",
            path: "?encryption",
            payload: "<ServerSideEncryptionConfiguration \
                      xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Rule>\
                      <ApplyServerSideEncryptionByDefault><SSEAlgorithm>AES256</SSEAlgorithm>\
                      </ApplyServerSideEncryptionByDefault></Rule>\
                      </ServerSideEncryptionConfiguration>",
        },
        WriteRow {
            case: "a CORS configuration",
            method: "PUT",
            path: "?cors",
            payload: "<CORSConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                      <CORSRule><AllowedMethod>GET</AllowedMethod>\
                      <AllowedOrigin>*</AllowedOrigin></CORSRule></CORSConfiguration>",
        },
        WriteRow {
            case: "a lifecycle configuration",
            method: "PUT",
            path: "?lifecycle",
            payload: "<LifecycleConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                      <Rule><ID>expire</ID><Filter><Prefix>logs/</Prefix></Filter>\
                      <Status>Enabled</Status><Expiration><Days>30</Days></Expiration></Rule>\
                      </LifecycleConfiguration>",
        },
        WriteRow {
            case: "a transfer-acceleration state",
            method: "PUT",
            path: "?accelerate",
            payload: "<AccelerateConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                      <Status>Enabled</Status></AccelerateConfiguration>",
        },
        WriteRow {
            case: "a request-payment configuration",
            method: "PUT",
            path: "?requestPayment",
            payload: "<RequestPaymentConfiguration \
                      xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Payer>Requester</Payer>\
                      </RequestPaymentConfiguration>",
        },
        WriteRow {
            case: "the removal of a CORS configuration",
            method: "DELETE",
            path: "?cors",
            payload: "",
        },
        WriteRow {
            case: "the removal of a lifecycle configuration",
            method: "DELETE",
            path: "?lifecycle",
            payload: "",
        },
        WriteRow {
            case: "the removal of a tag set",
            method: "DELETE",
            path: "?tagging",
            payload: "",
        },
    ]
}

#[tokio::test]
async fn a_configuration_write_advances_only_the_regional_creation_date() {
    let router = router();
    let bucket = "creation-date-views";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status,
        200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );

    // Before anything configures it the two views agree: the create's own subresources came
    // into being with the bucket.
    let original = creation_date(&router, DEFAULT_BUCKET_REGION, bucket).await;
    let regional_before = creation_date(&router, OTHER_REGION, bucket).await;
    assert_eq!(
        regional_before, original,
        "before any configuration write both views must agree, got {DEFAULT_BUCKET_REGION} \
         {original:?} and {OTHER_REGION} {regional_before:?}"
    );

    let mut previous = regional_before;
    for row in write_rows() {
        let case = row.case;
        // A reported `CreationDate` is rendered at the resolution S3 reports it in, which is
        // coarser than the clock: so each row waits for the clock to cross into the next
        // reported instant before writing, and the advance it asserts is one a client could
        // actually observe.
        std::thread::sleep(Duration::from_millis(1_100));
        let written = send_in_region(
            &router,
            DEFAULT_BUCKET_REGION,
            row.method,
            &format!("/{bucket}{}", row.path),
            &[],
            row.payload,
        )
        .await;
        assert!(
            (200..300).contains(&written.status),
            "case {case:?}: the configuration write must be taken, got {} with body {:?}",
            written.status,
            text(&written)
        );

        let global = creation_date(&router, DEFAULT_BUCKET_REGION, bucket).await;
        assert_eq!(
            global, original,
            "case {case:?}: the {DEFAULT_BUCKET_REGION} view must still report the instant \
             the bucket was created ({original:?}), got {global:?}"
        );
        let regional = creation_date(&router, OTHER_REGION, bucket).await;
        assert!(
            instant_of(&regional) > instant_of(&previous),
            "case {case:?}: the {OTHER_REGION} view must advance past {previous:?}, got \
             {regional:?}"
        );
        previous = regional;
    }
}

#[tokio::test]
async fn a_never_configured_bucket_reports_one_instant_to_every_endpoint() {
    let router = router();

    // The rows differ along the Region the bucket itself was created in, which is not what
    // the rule turns on: it is the endpoint the LISTING was served by.
    for (case, bucket, constraint) in [
        ("a bucket in the global Region", "views-global", None),
        (
            "a bucket in another Region",
            "views-elsewhere",
            Some(OTHER_REGION),
        ),
    ] {
        let created = create_bucket(&router, bucket, constraint, None).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: the bucket must be created, got {} with body {:?}",
            created.status,
            text(&created)
        );
        let global = creation_date(&router, DEFAULT_BUCKET_REGION, bucket).await;
        let regional = creation_date(&router, OTHER_REGION, bucket).await;
        assert_eq!(
            regional, global,
            "case {case:?}: a bucket nothing has configured reports one instant to every \
             endpoint, got {DEFAULT_BUCKET_REGION} {global:?} and {OTHER_REGION} {regional:?}"
        );
    }
}
