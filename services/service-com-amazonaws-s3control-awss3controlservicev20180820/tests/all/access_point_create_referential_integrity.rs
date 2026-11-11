// SPDX-License-Identifier: CC0-1.0

//! What `CreateAccessPoint` will and will not create, and how it says so, driven
//! end to end over HTTP against Amazon S3 beside it on one shared engine.
//!
//! The rules under test:
//!
//! * an access point is a resource OF a bucket, so a create is carried out only
//!   when the bucket it names EXISTS in the object store that owns buckets; a
//!   create naming any other bucket is refused `InvalidRequest` and records
//!   nothing;
//! * a create that names NO bucket at all is refused in this service's own
//!   words, under the code its refusals carry, rather than in the request
//!   boundary's validation vocabulary — while a bucket the request DID name and
//!   that breaks one of the member's own rules keeps the cause its caller has to
//!   fix;
//! * a refusal that carries a `URI` member names what the stage that refused
//!   can name: a path segment longer than the wire can carry as an access point
//!   name is not read as a resource name at all, so the refusal echoes the
//!   request target it was handed; every other name refusal names the RESOURCE it
//!   did read — the path under the service root, without the API-version segment
//!   and without a leading separator.
//!
//! Each rule is driven over a TABLE whose rows differ along what it generalises
//! over: whether the bucket exists, how it came to exist or stop existing, the
//! Region it lives in, the length of its name, which other member the body
//! carries instead of the bucket, which rule the access point name breaks, and
//! which route and which resource the refusal is about. No row is a renaming of
//! another.
//!
//! The harness is the sibling suites': [`crate::object_store::ObjectStore`]
//! mounts Amazon S3 and S3 Control on ONE engine — the assembly `core/router`
//! performs — so every bucket a row needs is created through Amazon S3's own
//! front door, and every access point through S3 Control's.

use crate::access_point_lifecycle::{xml_escape, Reply, ACCOUNT, REGION, XMLNS};
use crate::object_store::{ObjectStore, DEFAULT_BUCKET_REGION};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    ACCESS_POINT_BUCKET_ABSENT_MESSAGE, ACCESS_POINT_NAME_LEN, INVALID_ACCESS_POINT_NAME_MESSAGE,
    INVALID_URI_CODE, INVALID_URI_MESSAGE, MISSING_ACCESS_POINT_BUCKET_MESSAGE,
    REQUEST_INVALID_CODE, REQUEST_INVALID_STATUS,
};

/// Create the access point `name` over the bucket `bucket`, naming nothing else.
async fn create_over(store: &ObjectStore, name: &str, bucket: &str) -> Reply {
    create_with(
        store,
        name,
        &format!("<Bucket>{}</Bucket>", xml_escape(bucket)),
    )
    .await
}

/// Create the access point `name` with `members` for the whole request document.
async fn create_with(store: &ObjectStore, name: &str, members: &str) -> Reply {
    store
        .s3control(
            REGION,
            "PUT",
            &format!("/v20180820/accesspoint/{name}"),
            &format!("<CreateAccessPointRequest xmlns=\"{XMLNS}\">{members}</CreateAccessPointRequest>"),
        )
        .await
}

/// Read the access point `name`.
async fn read(store: &ObjectStore, name: &str) -> Reply {
    store
        .s3control(REGION, "GET", &format!("/v20180820/accesspoint/{name}"), "")
        .await
}

/// Assert that `refused` is the refusal of a create whose bucket does not exist,
/// and that `name` names no access point afterwards.
async fn assert_bucket_absent(store: &ObjectStore, case: &str, name: &str, refused: &Reply) {
    assert_eq!(
        (
            refused.status,
            refused.code(),
            refused.text("Message").unwrap_or_default()
        ),
        (
            REQUEST_INVALID_STATUS,
            REQUEST_INVALID_CODE.to_string(),
            ACCESS_POINT_BUCKET_ABSENT_MESSAGE.to_string()
        ),
        "case {case:?}: expected {REQUEST_INVALID_STATUS} {REQUEST_INVALID_CODE} \
         {ACCESS_POINT_BUCKET_ABSENT_MESSAGE:?}, got {} with body {}",
        refused.status,
        refused.body
    );
    let read_back = read(store, name).await;
    assert_eq!(
        (read_back.status, read_back.code().as_str()),
        (404, "NoSuchAccessPoint"),
        "case {case:?}: expected the refused create to record nothing, got {} with body {}",
        read_back.status,
        read_back.body
    );
}

/// A bucket name of exactly `length` characters, built from one letter and a
/// digit tail so it stays inside the general-purpose bucket naming rules
/// whatever the length asked for. `tag` makes two names of one length differ.
fn bucket_name_of(tag: &str, length: usize) -> String {
    let mut name = format!("b{tag}");
    name.truncate(length);
    while name.len() < length {
        name.push(char::from(b'0' + (name.len() % 10) as u8));
    }
    name
}

/// The general-purpose bucket name length bounds Amazon S3 publishes: "Bucket
/// names must be between 3 (min) and 63 (max) characters long" (Amazon S3 User
/// Guide, *Bucket naming rules*). The rows below name a bucket at each end, so
/// what decides the answer is whether the bucket EXISTS rather than how long its
/// name is.
const SHORTEST_BUCKET_NAME: usize = 3;
const LONGEST_BUCKET_NAME: usize = 63;

#[tokio::test]
async fn a_create_is_carried_out_exactly_when_the_bucket_it_names_exists() {
    let store = ObjectStore::start();

    // The rows that MUST be served: the bucket exists, however it came to exist
    // and whatever its name and Region.
    let served: Vec<(&str, String, &str)> = vec![
        (
            "a bucket in the Region the create is signed for",
            "reports".to_string(),
            REGION,
        ),
        (
            "a bucket in another Region",
            "archive".to_string(),
            DEFAULT_BUCKET_REGION,
        ),
        (
            "a bucket at the shortest name length",
            bucket_name_of("held", SHORTEST_BUCKET_NAME),
            REGION,
        ),
        (
            "a bucket at the longest name length",
            bucket_name_of("held", LONGEST_BUCKET_NAME),
            REGION,
        ),
    ];
    for (index, (case, bucket, region)) in served.iter().enumerate() {
        store.create_bucket(case, region, bucket).await;
        let name = format!("served-ap-{index}");
        let created = create_over(&store, &name, bucket).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create over an existing bucket to be served, got {} \
             with body {}",
            created.status, created.body
        );
        // Round trip: the access point the create reported is the one a read
        // reports, fronting the bucket the create named.
        let read_back = read(&store, &name).await;
        assert_eq!(
            (read_back.status, read_back.text("Bucket")),
            (200, Some(bucket.clone())),
            "case {case:?}: expected the read to report the access point over {bucket:?}, got \
             {} with body {}",
            read_back.status,
            read_back.body
        );
    }

    // The rows that must be REFUSED: there is no such bucket when the create
    // arrives.
    let absent = "never-created";
    let typo = "reportz";
    let deleted = "transient";
    store.create_bucket("a bucket to delete", REGION, deleted).await;
    let (status, body) = store.s3(REGION, "DELETE", &format!("/{deleted}"), b"").await;
    assert_eq!(
        status,
        204,
        "the bucket {deleted} must be deleted first, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
    for (index, (case, bucket)) in [
        ("a bucket nothing ever created", absent.to_string()),
        (
            "a name one character off an existing bucket",
            typo.to_string(),
        ),
        (
            "a bucket that was created and then deleted",
            deleted.to_string(),
        ),
        (
            "an absent name at the shortest bucket length",
            bucket_name_of("gone", SHORTEST_BUCKET_NAME),
        ),
        (
            "an absent name at the longest bucket length",
            bucket_name_of("gone", LONGEST_BUCKET_NAME),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("absent-ap-{index}");
        let refused = create_over(&store, &name, &bucket).await;
        assert_bucket_absent(&store, case, &name, &refused).await;
    }

    // Nothing the refusals asked for is in the listing: every entry is one of
    // the creates that were served.
    let listed = store
        .s3control(REGION, "GET", "/v20180820/accesspoint", "")
        .await;
    assert!(
        listed
            .texts("Name")
            .iter()
            .all(|name| name.starts_with("served-ap-")),
        "expected only the served creates to be listed, got body {}",
        listed.body
    );
}

#[tokio::test]
async fn a_create_that_names_no_bucket_is_refused_in_this_services_own_words() {
    let store = ObjectStore::start();
    store.create_bucket("the bucket", REGION, "reports").await;

    // One row per document that names no bucket: the member is absent whatever
    // else the body carries.
    let rows: Vec<(&str, String)> = vec![
        ("an empty document", String::new()),
        (
            "a document carrying only the VPC configuration",
            "<VpcConfiguration><VpcId>vpc-0a1b2c3d</VpcId></VpcConfiguration>".to_string(),
        ),
        (
            "a document carrying only the public-access block",
            "<PublicAccessBlockConfiguration><BlockPublicAcls>true</BlockPublicAcls>\
             </PublicAccessBlockConfiguration>"
                .to_string(),
        ),
        (
            "a document carrying only the bucket's owning account",
            format!("<BucketAccountId>{ACCOUNT}</BucketAccountId>"),
        ),
        (
            "a document carrying only a tag",
            "<Tags><Tag><Key>team</Key><Value>finance</Value></Tag></Tags>".to_string(),
        ),
    ];
    for (index, (case, members)) in rows.iter().enumerate() {
        let name = format!("no-bucket-ap-{index}");
        let refused = create_with(&store, &name, members).await;
        assert_eq!(
            (
                refused.status,
                refused.code(),
                refused.text("Message").unwrap_or_default()
            ),
            (
                REQUEST_INVALID_STATUS,
                REQUEST_INVALID_CODE.to_string(),
                MISSING_ACCESS_POINT_BUCKET_MESSAGE.to_string()
            ),
            "case {case:?}: expected {REQUEST_INVALID_STATUS} {REQUEST_INVALID_CODE} \
             {MISSING_ACCESS_POINT_BUCKET_MESSAGE:?}, got {} with body {}",
            refused.status,
            refused.body
        );
        let read_back = read(&store, &name).await;
        assert_eq!(
            read_back.status, 404,
            "case {case:?}: expected the refused create to record nothing, got {} with body {}",
            read_back.status, read_back.body
        );
    }

    // A bucket the request DID name and that breaks one of the member's own
    // rules is a refusal about that value: `BucketName` is `@length 3..255`
    // (`smithy_query(query="operation", operation="CreateAccessPoint")`), so a
    // shorter one is refused — but not as a request that named no bucket.
    for (case, bucket) in [
        ("a bucket name one character under the modelled minimum", "ab"),
        ("a bucket name of one character", "a"),
    ] {
        let name = "short-bucket-ap";
        let refused = create_over(&store, name, bucket).await;
        assert_eq!(
            refused.status, REQUEST_INVALID_STATUS,
            "case {case:?}: expected the value to be refused, got {} with body {}",
            refused.status, refused.body
        );
        assert_ne!(
            refused.text("Message").as_deref(),
            Some(MISSING_ACCESS_POINT_BUCKET_MESSAGE),
            "case {case:?}: expected a refusal about the value, not about an absent member, \
             got body {}",
            refused.body
        );
        let read_back = read(&store, name).await;
        assert_eq!(
            read_back.status, 404,
            "case {case:?}: expected the refused create to record nothing, got {} with body {}",
            read_back.status, read_back.body
        );
    }
}

#[tokio::test]
async fn a_name_refusal_names_what_the_stage_that_refused_it_can_name() {
    let store = ObjectStore::start();
    let longest = *ACCESS_POINT_NAME_LEN.end();

    // One row per refusal that carries a `URI` member, per stage it comes from:
    // a segment LONGER than the wire can carry as a name, which this service
    // cannot read as a resource name at all; and a name it did read but will not
    // accept — one the wire cannot address, one an access point may not be given,
    // on the create route and on a sub-resource route.
    //
    // The expected member is spelled per row rather than derived, so the two
    // forms are stated here rather than recomputed from the code under test.
    let rows: Vec<(&str, &str, String, &str, &str, String)> = vec![
        (
            "a name one past the addressable length, on the create",
            "PUT",
            format!("/v20180820/accesspoint/{}", "a".repeat(longest + 1)),
            INVALID_URI_CODE,
            INVALID_URI_MESSAGE,
            format!("/v20180820/accesspoint/{}", "a".repeat(longest + 1)),
        ),
        (
            "a name far past the addressable length, on the read",
            "GET",
            format!("/v20180820/accesspoint/{}", "b".repeat(longest * 3)),
            INVALID_URI_CODE,
            INVALID_URI_MESSAGE,
            format!("/v20180820/accesspoint/{}", "b".repeat(longest * 3)),
        ),
        (
            "an upper-case name, on the create",
            "PUT",
            "/v20180820/accesspoint/Reports-AP".to_string(),
            INVALID_URI_CODE,
            INVALID_URI_MESSAGE,
            "accesspoint/Reports-AP".to_string(),
        ),
        (
            "an underscore in the name, on the read",
            "GET",
            "/v20180820/accesspoint/reports_ap".to_string(),
            INVALID_URI_CODE,
            INVALID_URI_MESSAGE,
            "accesspoint/reports_ap".to_string(),
        ),
        (
            "a name an access point may not be given, on the create",
            "PUT",
            "/v20180820/accesspoint/-reports-ap".to_string(),
            REQUEST_INVALID_CODE,
            INVALID_ACCESS_POINT_NAME_MESSAGE,
            "accesspoint/-reports-ap".to_string(),
        ),
        (
            "a name ending in the alias suffix, on the read",
            "GET",
            "/v20180820/accesspoint/reports-s3alias".to_string(),
            REQUEST_INVALID_CODE,
            INVALID_ACCESS_POINT_NAME_MESSAGE,
            "accesspoint/reports-s3alias".to_string(),
        ),
        (
            "a name an access point may not be given, on a sub-resource",
            "GET",
            "/v20180820/accesspoint/reports.ap/policy".to_string(),
            REQUEST_INVALID_CODE,
            INVALID_ACCESS_POINT_NAME_MESSAGE,
            "accesspoint/reports.ap/policy".to_string(),
        ),
        (
            "a name one character under the shortest, on the create",
            "PUT",
            "/v20180820/accesspoint/ap".to_string(),
            REQUEST_INVALID_CODE,
            INVALID_ACCESS_POINT_NAME_MESSAGE,
            "accesspoint/ap".to_string(),
        ),
    ];

    for (case, method, uri, code, message, named) in rows {
        let body = match method {
            "PUT" => format!(
                "<CreateAccessPointRequest xmlns=\"{XMLNS}\"><Bucket>reports</Bucket>\
                 </CreateAccessPointRequest>"
            ),
            _ => String::new(),
        };
        let refused = store.s3control(REGION, method, &uri, &body).await;
        assert_eq!(
            (refused.status, refused.code(), refused.text("Message")),
            (400, code.to_string(), Some(message.to_string())),
            "case {case:?}: expected 400 {code} {message:?}, got {} with body {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.text("URI"),
            Some(named.clone()),
            "case {case:?}: expected the refusal to name {named:?}, got body {}",
            refused.body
        );
    }

    // The refusal that names a RESOURCE names it under the service root: no
    // leading separator and no API-version segment, so a caller can read the
    // resource identity out of it.
    let resource_named = store
        .s3control(
            REGION,
            "GET",
            "/v20180820/accesspoint/reports.ap/policy",
            "",
        )
        .await
        .text("URI")
        .unwrap_or_default();
    assert!(
        !resource_named.starts_with('/') && !resource_named.contains("v20180820"),
        "expected a resource-relative URI member, got {resource_named:?}"
    );
}
