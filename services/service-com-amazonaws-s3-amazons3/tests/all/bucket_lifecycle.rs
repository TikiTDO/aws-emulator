// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for S3's bucket lifecycle: create a general purpose bucket and read
//! it back with `HeadBucket`.
//!
//! Every assertion here drives a real HTTP request through the router, so what is pinned
//! is the wire behaviour (status, bound headers, error code) rather than any handler's
//! internals.
//!
//! The rules under test come from the model's own documentation, quoted beside each
//! table:
//!
//! * `CreateBucketConfiguration$LocationConstraint` decides the bucket's Region — absent
//!   means `us-east-1`, the legacy value `EU` means `eu-west-1`, any other value IS the
//!   Region code.
//! * `CreateBucketOutput$Location` is `/<bucket>` for an account regional namespace
//!   bucket and for a global bucket in `us-east-1`, and the new bucket's own endpoint URL
//!   for a global bucket in any other Region — rooted on the endpoint serving the request
//!   (pinned over a table of endpoint bases by [`super::announced_response_urls`]).
//! * Re-creating a bucket you already own answers `BucketAlreadyOwnedByYou`, except in
//!   `us-east-1` where, "for legacy compatibility", it answers 200 OK.

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::state::{
    BucketCreation, ComAmazonawsS3Amazons3State, MAX_BUCKETS_PER_ACCOUNT,
};
use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `s3`. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off below, exactly as the emulator runs it
/// unless started with authentication on, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// [`AUTHORIZATION`] with `region` in its credential scope — which is how the emulator reads
/// WHICH Region's endpoint a client addressed it as.
fn authorization_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{region}/s3/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it, together with the ENGINE it was mounted on — so a suite can seed the same
/// store the service reads for a state no sequence of requests can reach.
///
/// `pub(crate)`, like the helpers below it: a suite that needs the store reaches it through
/// this one harness instead of standing up a second one.
pub(crate) fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
///
/// `pub(crate)`, like the three helpers below it: the tagging suite in this same test
/// crate drives its requests through the same harness instead of standing up a second one.
pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// One row of the create-and-read-back table: the case name, the location constraint the
/// request carries (when any), the bucket namespace it carries (when any), the bucket the
/// create names, the `Location` the create must report, and the Region the bucket must then
/// read back with.
type CreateRow<'a> = (
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    &'a str,
    String,
    &'a str,
);

/// The URL a create reports for a bucket whose `Location` is its own endpoint URL, on an
/// emulator served on NO port — the base `OperationContext::endpoint` reports for one
/// (`smithy_server_runtime::UNSERVED_URL_BASE`), which is what this harness mounts.
///
/// What the rule itself generalises over — WHICH endpoint is serving the request — is pinned
/// over a table of endpoint bases by [`super::announced_response_urls`]; here it fixes the
/// one expectation the rest of this table is written against.
fn endpoint_url_of(bucket: &str) -> String {
    format!("{}/{bucket}/", smithy_server_runtime::UNSERVED_URL_BASE)
}

/// What a dispatched request answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: bytes::Bytes,
}

impl Answer {
    /// A bound response header's value.
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The wire error code: the `<Code>` element of S3's restXml error document (the
    /// model sets `noErrorWrapping`, so it is the document's own child), or the
    /// `x-amzn-errortype` header when the protocol carried it there instead.
    pub(crate) fn error_code(&self) -> String {
        self.element("Code")
            .or_else(|| self.header("x-amzn-errortype"))
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The `<Message>` of that same error document, or the empty string when the body
    /// carries none — which is exactly what a client renders as "Unknown".
    pub(crate) fn error_message(&self) -> String {
        self.element("Message").unwrap_or_default()
    }

    /// The text of the first `<name>` element of the body, with the XML escaping the
    /// serializer applies undone, when it carries one.
    ///
    /// `pub(crate)`: the copy suite ([`super::object_copy`]) reads the `CopyObjectResult`
    /// container's members out of a SUCCESS body through this one reader rather than
    /// slicing the body a second way.
    pub(crate) fn element(&self, name: &str) -> Option<String> {
        let body = String::from_utf8_lossy(&self.body).to_string();
        body.split_once(&format!("<{name}>"))
            .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
            .map(|(text, _)| super::bucket_tagging::unescaped(text))
    }

    /// The text of EVERY `<name>` element of the body, in the order the document carries
    /// them, with the XML escaping the serializer applies undone — [`Self::element`] for a
    /// member a document repeats, such as the `<Name>` of each bucket a listing reports.
    ///
    /// `pub(crate)`, beside the single-element reader: a suite that reads a repeated member
    /// goes through this one rather than slicing the body a second way.
    pub(crate) fn elements(&self, name: &str) -> Vec<String> {
        elements_of(&String::from_utf8_lossy(&self.body), name)
            .iter()
            .map(|text| super::bucket_tagging::unescaped(text))
            .collect()
    }
}

/// The text of every `<name>` element of `xml`, in document order, exactly as the document
/// carries it.
///
/// `pub(crate)`: the ONE reading of a repeated element in this test crate — [`Answer::elements`]
/// reads a response body through it (and undoes the escaping its own single-element reader
/// undoes), and a suite holding the document as text
/// ([`super::notification_destination_validation`]) reads that text through it.
pub(crate) fn elements_of(xml: &str, name: &str) -> Vec<String> {
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    xml.split(&open)
        .skip(1)
        .filter_map(|rest| rest.split_once(&close))
        .map(|(text, _)| text.to_string())
        .collect()
}

/// Dispatch one request with the given headers and an XML (or empty) body.
///
/// The XML content type every request in this suite and the tagging suite carries is
/// added here; a suite whose payload is arbitrary bytes (an object's) calls
/// [`send_bytes`] and names its own content type.
pub(crate) async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("content-type", "application/xml")];
    all.extend_from_slice(headers);
    send_bytes(router, method, uri, &all, body.as_bytes()).await
}

/// The endpoint host every request in this crate's suites is addressed to unless it names
/// another: the Regional one, which carries no bucket, so the request is PATH-STYLE.
const REGIONAL_ENDPOINT_HOST: &str = "s3.us-east-1.amazonaws.com";

/// Dispatch one request whose body is arbitrary BYTES and whose headers are exactly the
/// ones given — what an object payload needs.
pub(crate) async fn send_bytes(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    send_bytes_to_host(router, method, REGIONAL_ENDPOINT_HOST, uri, headers, body).await
}

/// [`send_bytes`] addressed to `host` instead of to the Regional endpoint — which is what
/// makes a request VIRTUAL-HOSTED-STYLE: the bucket is the first label of the host and the
/// path is the key alone.
pub(crate) async fn send_bytes_to_host(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    send_bytes_signed(router, AUTHORIZATION, method, host, uri, headers, body).await
}

/// [`send_bytes_to_host`] signed for `region` and addressed to that Region's own endpoint —
/// which is what tells the emulator WHICH endpoint a client reached, and so which Region a
/// `CreateBucket` sent there may place its bucket in.
///
/// `pub(crate)`: every suite that drives a request as a client of another Region's endpoint
/// goes through this one helper rather than spelling a credential scope of its own.
pub(crate) async fn send_in_region(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("content-type", "application/xml")];
    all.extend_from_slice(headers);
    send_bytes_signed(
        router,
        &authorization_for(region),
        method,
        &format!("s3.{region}.amazonaws.com"),
        uri,
        &all,
        body.as_bytes(),
    )
    .await
}

/// The one place this harness puts a request on the wire: `authorization` carries the
/// credential scope the emulator reads the endpoint's Region out of, and `host` the endpoint
/// the request is addressed to.
///
/// `pub(crate)`: a suite whose case varies the credential SCOPE — the signing name of the
/// endpoint a client reached, or the key id a session vended ([`super::directory_bucket_listings`],
/// [`super::express_session_tokens`]) — drives its requests through this one helper rather
/// than rebuilding the dispatch.
pub(crate) async fn send_bytes_signed(
    router: &MultiServiceRouter,
    authorization: &str,
    method: &str,
    host: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host)
        .header("authorization", authorization);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `PUT /<bucket>` — `CreateBucket`, with the location constraint (when any) in the XML
/// payload and the bucket namespace (when any) in its bound header.
///
/// The request is addressed to the endpoint of the Region its constraint names, the way a
/// client that creates a bucket in that Region addresses S3: an endpoint for one Region
/// cannot place a bucket in another, which S3 refuses with
/// `IllegalLocationConstraintException`. A create naming no constraint goes to the endpoint
/// of the Region the unspecified constraint means ([`constraint_region`]).
pub(crate) async fn create_bucket(
    router: &MultiServiceRouter,
    bucket: &str,
    location_constraint: Option<&str>,
    bucket_namespace: Option<&str>,
) -> Answer {
    let body = match location_constraint {
        None => String::new(),
        Some(constraint) => format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{constraint}</LocationConstraint>\
             </CreateBucketConfiguration>"
        ),
    };
    let headers: Vec<(&str, &str)> = bucket_namespace
        .map(|namespace| vec![("x-amz-bucket-namespace", namespace)])
        .unwrap_or_default();
    send_in_region(
        router,
        &constraint_region(location_constraint),
        "PUT",
        &format!("/{bucket}"),
        &headers,
        &body,
    )
    .await
}

/// The Region a location constraint names, through the one reading the service itself uses
/// (`state::bucket_region`): absent means `us-east-1` — the Region the unspecified constraint
/// stands for, and the one [`AUTHORIZATION`] is scoped to — the legacy `EU` means
/// `eu-west-1`, and every other `BucketLocationConstraint` value IS the Region code.
pub(crate) fn constraint_region(location_constraint: Option<&str>) -> String {
    service_com_amazonaws_s3_amazons3::state::bucket_region(location_constraint)
}

/// `HEAD /<bucket>` — `HeadBucket`.
///
/// `pub(crate)`, like the harness above: the object listing and deletion suite reads a
/// bucket back through it rather than standing up its own request.
pub(crate) async fn head_bucket(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "HEAD", &format!("/{bucket}"), &[], "").await
}

/// `PUT /<bucket>?policy` — `PutBucketPolicy`, with the document as the body.
///
/// `pub(crate)` for the same reason as the helpers above: both the policy-status suite
/// and the object-and-policy suite drive the bucket policy subresource, and one helper
/// keeps them from drifting apart.
pub(crate) async fn put_bucket_policy(
    router: &MultiServiceRouter,
    bucket: &str,
    policy: &str,
) -> Answer {
    send(router, "PUT", &format!("/{bucket}?policy"), &[], policy).await
}

/// `GET /<bucket>?policy` — `GetBucketPolicy`.
pub(crate) async fn get_bucket_policy(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?policy"), &[], "").await
}

/// A valid policy document padded with a `Sid` so that it is exactly `bytes` long — what a
/// size-limit row needs, since the limit is on the document's BYTES.
///
/// `pub(crate)` so the two suites that drive the bucket policy subresource cannot drift on
/// what "exactly at the ceiling" means. The padded document keeps the skeleton's member
/// ORDER (`Sid` first), so measuring the skeleton and then padding by the difference lands
/// on exactly `bytes`; each call site asserts the resulting length, which is what would
/// catch a change in `serde_json`'s key ordering.
pub(crate) fn policy_of_size(bytes: usize) -> String {
    let skeleton = serde_json::json!({
        "Statement": [{"Sid": "", "Effect": "Allow", "Principal": "*", "Action": "s3:*"}],
    })
    .to_string();
    let padding = bytes.saturating_sub(skeleton.len());
    serde_json::json!({
        "Statement": [{
            "Sid": "s".repeat(padding),
            "Effect": "Allow",
            "Principal": "*",
            "Action": "s3:*",
        }],
    })
    .to_string()
}

#[tokio::test]
async fn creating_a_bucket_reports_its_location_and_region_and_reads_back() {
    let router = router();

    // (case, location constraint, bucket namespace, expected Location, expected Region).
    //
    // The rows differ along what the two rules generalise over: whether a constraint is
    // present, which Region it names (including the legacy `EU` alias and Regions in
    // three partitions' worth of prefixes, none of them the one the issue cited), and
    // which namespace the bucket is created in. Every constraint value below is one of
    // `BucketLocationConstraint`'s enum values
    // (`smithy_query(query="shape",shape="BucketLocationConstraint")`); `us-east-1` is
    // deliberately NOT among them, which is why the absent-constraint row is the only
    // way to ask for it.
    let rows: [CreateRow<'_>; 7] = [
        (
            "no constraint defaults to us-east-1, so Location is the bucket path",
            None,
            None,
            "bucket-default",
            "/bucket-default".to_string(),
            "us-east-1",
        ),
        (
            "a named Region makes Location the bucket's own endpoint URL",
            Some("us-west-2"),
            None,
            "bucket-us-west-2",
            endpoint_url_of("bucket-us-west-2"),
            "us-west-2",
        ),
        (
            "the legacy EU constraint creates the bucket in eu-west-1",
            Some("EU"),
            None,
            "bucket-eu",
            endpoint_url_of("bucket-eu"),
            "eu-west-1",
        ),
        (
            "a Region the issue never cited behaves like any other",
            Some("ap-southeast-7"),
            None,
            "bucket-ap-southeast-7",
            endpoint_url_of("bucket-ap-southeast-7"),
            "ap-southeast-7",
        ),
        (
            "a gov-cloud Region is still just a Region code",
            Some("us-gov-east-1"),
            None,
            "bucket-us-gov-east-1",
            endpoint_url_of("bucket-us-gov-east-1"),
            "us-gov-east-1",
        ),
        (
            "an account regional namespace bucket reports the bucket path",
            Some("us-west-2"),
            Some("account-regional"),
            "bucket-account-regional",
            "/bucket-account-regional".to_string(),
            "us-west-2",
        ),
        (
            "a global namespace bucket in us-east-1 reports the bucket path",
            None,
            Some("global"),
            "bucket-global-default",
            "/bucket-global-default".to_string(),
            "us-east-1",
        ),
    ];

    for (case, constraint, namespace, bucket, expected_location, expected_region) in rows {
        let created = create_bucket(&router, bucket, constraint, namespace).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from CreateBucket on the fresh name \
             {bucket:?}, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        assert_eq!(
            created.header("location").as_deref(),
            Some(expected_location.as_str()),
            "case {case:?}: expected Location {expected_location:?}, got {:?}",
            created.header("location")
        );
        // The new bucket's own ARN, which a create reports for a general purpose bucket as
        // well as for a directory bucket (recorded behaviour, pinned over a table of its own
        // by [`super::bucket_create_list_and_tagging_contracts`]): a general purpose
        // bucket's carries its globally unique name alone.
        assert_eq!(
            created.header("x-amz-bucket-arn").as_deref(),
            Some(format!("arn:aws:s3:::{bucket}").as_str()),
            "case {case:?}: expected the created bucket's ARN, got {:?}",
            created.header("x-amz-bucket-arn")
        );

        // Read the created bucket back: the Region the constraint asked for is the one
        // persisted state answers with.
        let head = head_bucket(&router, bucket).await;
        assert_eq!(
            head.status, 200,
            "case {case:?}: expected HTTP 200 from HeadBucket on the bucket just \
             created, got {}",
            head.status
        );
        assert_eq!(
            head.header("x-amz-bucket-region").as_deref(),
            Some(expected_region),
            "case {case:?}: expected HeadBucket to report Region {expected_region:?}, \
             got {:?}",
            head.header("x-amz-bucket-region")
        );
        assert_eq!(
            head.header("x-amz-access-point-alias").as_deref(),
            Some("false"),
            "case {case:?}: the request named a bucket, not an access point alias, \
             got {:?}",
            head.header("x-amz-access-point-alias")
        );
    }
}

#[tokio::test]
async fn recreating_a_bucket_you_own_answers_by_the_bucket_region() {
    let router = router();

    // (case, location constraint, expected status of the SECOND create, expected error
    // code of the second create).
    //
    // The rule: re-creating a bucket you already own is `BucketAlreadyOwnedByYou`
    // (HTTP 409 per the model) — except in us-east-1, where "for legacy compatibility
    // … Amazon S3 returns 200 OK" (`BucketAlreadyOwnedByYou`'s own documentation). The
    // rows span both sides of that split, in three Regions.
    let rows: [(&str, Option<&str>, u16, Option<&str>); 4] = [
        (
            "us-east-1 re-create is the documented legacy 200",
            None,
            200,
            None,
        ),
        (
            "us-west-2 re-create is refused",
            Some("us-west-2"),
            409,
            Some("BucketAlreadyOwnedByYou"),
        ),
        (
            "the EU alias Region is refused like any other non-us-east-1 Region",
            Some("EU"),
            409,
            Some("BucketAlreadyOwnedByYou"),
        ),
        (
            "ap-northeast-3 re-create is refused",
            Some("ap-northeast-3"),
            409,
            Some("BucketAlreadyOwnedByYou"),
        ),
    ];

    for (index, (case, constraint, expected_status, expected_code)) in rows.iter().enumerate() {
        let bucket = format!("recreate-bucket-{index}");

        let first = create_bucket(&router, &bucket, *constraint, None).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first create of {bucket:?} must succeed, got {}",
            first.status
        );

        let second = create_bucket(&router, &bucket, *constraint, None).await;
        assert_eq!(
            second.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status} from the second create, \
             got {} with body {:?}",
            second.status,
            String::from_utf8_lossy(&second.body)
        );
        match expected_code {
            Some(code) => assert_eq!(
                second.error_code(),
                *code,
                "case {case:?}: expected error code {code:?}, got {:?}",
                second.error_code()
            ),
            None => assert_eq!(
                second.header("location").as_deref(),
                Some(format!("/{bucket}").as_str()),
                "case {case:?}: the legacy 200 still reports the bucket's Location, \
                 got {:?}",
                second.header("location")
            ),
        }

        // Either way the re-create changed nothing: the bucket still reports the Region
        // it was created in.
        let head = head_bucket(&router, &bucket).await;
        let expected_region = match constraint {
            None => "us-east-1",
            Some("EU") => "eu-west-1",
            Some(region) => region,
        };
        assert_eq!(
            head.header("x-amz-bucket-region").as_deref(),
            Some(expected_region),
            "case {case:?}: expected the stored Region {expected_region:?} to survive \
             the re-create, got {:?}",
            head.header("x-amz-bucket-region")
        );
    }
}

#[tokio::test]
async fn creating_past_the_account_bucket_quota_is_refused() {
    // This suite needs the store itself, to stand the account at its quota without
    // dispatching ten thousand requests. The engine is the SAME one the router hands the
    // service, so what is seeded here is what the handler sees.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    {
        let state = engine.state::<ComAmazonawsS3Amazons3State>();
        let mut store = state.lock();
        for index in 0..MAX_BUCKETS_PER_ACCOUNT {
            assert_eq!(
                store.create_bucket(&format!("quota-filler-{index}"), "us-east-1"),
                BucketCreation::Created,
                "case {index}: seeding the store up to the quota must record every bucket"
            );
        }
        // The bound itself: one more, by any name and in any Region, is refused by the
        // store rather than recorded.
        for (name, region) in [
            ("quota-filler-extra", "us-east-1"),
            ("another-name", "us-west-2"),
        ] {
            assert_eq!(
                store.create_bucket(name, region),
                BucketCreation::QuotaReached,
                "case {name:?}: a create past {MAX_BUCKETS_PER_ACCOUNT} buckets must be \
                 refused by the store"
            );
        }
    }

    // And over the wire: a fresh name is refused with a 400 rather than growing the
    // store, in every Region form the create table covers.
    for (case, bucket, constraint) in [
        ("default Region", "past-quota-default", None),
        ("named Region", "past-quota-us-west-2", Some("us-west-2")),
        ("legacy EU alias", "past-quota-eu", Some("EU")),
    ] {
        let refused = create_bucket(&router, bucket, constraint, None).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the create past the quota to be refused with HTTP \
             400, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected the framework's refusal code, got {:?}",
            refused.error_code()
        );
        // Nothing was written: the refused name is still not a bucket.
        let head = head_bucket(&router, bucket).await;
        assert_eq!(
            head.status, 404,
            "case {case:?}: the refused name must not have been recorded, got HeadBucket \
             {}",
            head.status
        );
    }

    // A bucket the account already holds is NOT growth, so the documented us-east-1
    // legacy re-create still answers 200 even at the quota.
    let recreated = create_bucket(&router, "quota-filler-0", None, None).await;
    assert_eq!(
        recreated.status, 200,
        "re-creating a held us-east-1 bucket adds no entry, so it must still answer 200, \
         got {} with body {:?}",
        recreated.status,
        String::from_utf8_lossy(&recreated.body)
    );
}

#[tokio::test]
async fn heading_a_bucket_this_emulator_does_not_hold_is_not_found() {
    let router = router();

    create_bucket(&router, "held-bucket", None, None).await;

    // Names that are NOT the one bucket created above: a different name, a prefix of it,
    // an extension of it, and one differing only in case (bucket names are
    // case-sensitive). Of the three statuses `HeadBucket` documents for a HEAD it cannot
    // answer 200 — "a generic 400 Bad Request, 403 Forbidden, or 404 Not Found" — 404 is
    // the one that means the bucket is not there, and a HEAD carries no body, so the
    // status is the whole of what tells a caller that. The modelled `NotFound` shape
    // carries no `@httpError` for the framework to render it with
    // (`smithy_query(query="operation",operation="HeadBucket")` reports `httpError 400,
    // httpErrorDefaulted true`), so the code and status go out directly.
    for bucket in ["absent-bucket", "held", "held-bucket-2", "Held-Bucket"] {
        let answer = head_bucket(&router, bucket).await;
        assert_eq!(
            answer.status, 404,
            "case {bucket:?}: expected 404 for a bucket this emulator does not hold, got {}",
            answer.status
        );
        assert_eq!(
            answer.error_code(),
            "NotFound",
            "case {bucket:?}: expected the NotFound code, got {:?}",
            answer.error_code()
        );
    }

    // And the bucket that IS held still reads back, so the rows above failed on the
    // lookup rather than on the operation being unreachable.
    let held = head_bucket(&router, "held-bucket").await;
    assert_eq!(
        held.status, 200,
        "the held bucket must still read back, got {}",
        held.status
    );
}
