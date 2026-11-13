// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE rule every absolute URL this service hands a caller back
//! follows: it is rooted on the ENDPOINT serving the request, never on a host under
//! `amazonaws.com`.
//!
//! Three answers of this service carry such a URL — an address the client dials next:
//!
//! * `CreateBucketOutput$Location`, "the global endpoint URL" of the bucket that was just
//!   created (the member also has a path form, `/<bucket>`, for an account regional
//!   namespace bucket and for a global bucket in `us-east-1`; a path is already relative to
//!   whatever endpoint the caller is talking to, and [`super::bucket_lifecycle`] pins which
//!   create reports which form);
//! * `CompleteMultipartUploadOutput$Location`, "the URI that identifies the newly created
//!   object";
//! * the `Location` of the `PostResponse` document a browser-form upload is answered with.
//!
//! A URL naming one of AWS's own hosts resolves away from this process: a client that
//! follows it talks to public AWS, where the bucket and the object do not exist, or — with
//! no egress — fails to connect at all, so the create looks successful while the handle it
//! returned is unusable.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire value a client reads. The rows differ along what the rule generalises over: WHICH
//! endpoint this emulator is served at (served on no port at all, a loopback address with a
//! port, a bracketed IPv6 address with a port, a name with a port), which host the request
//! addressed (the served endpoint itself, another address of it, one of AWS's own regional
//! hosts, and the bucket's own virtual-hosted address), and the shape of the key the URL
//! carries.

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter, UNSERVED_URL_BASE};

use super::bucket_lifecycle::{send_bytes_signed, Answer};
use super::post_object_form_upload::{form_document, form_media_type};

/// The Region every request in this suite is signed for, and the one its buckets are created
/// in. Deliberately NOT `us-east-1`: a create in the default Region reports the path form of
/// `CreateBucketOutput$Location`, which names no host at all, so it would not exercise the
/// rule under test.
const REGION: &str = "us-west-2";

/// The regional endpoint host of AWS's own S3 in [`REGION`] — the host an SDK configured
/// against real S3 addresses, and one row's request is addressed to.
const AWS_REGIONAL_HOST: &str = "s3.us-west-2.amazonaws.com";

/// The credential scope every request carries: neither the key id nor the signature is a
/// credential, and the harness below turns request authentication off, so nothing here is
/// verified. The scope's Region is what tells the emulator which endpoint was reached.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/s3/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One emulator, served at `base` — the URL the data port publishes, which is what
/// `OperationContext::endpoint` reports — or served on no port at all when `base` is `None`.
fn router_served_at(base: Option<&str>) -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    if let Some(base) = base {
        router.set_url_base(
            EndpointUrl::parse(base).unwrap_or_else(|| panic!("{base:?} is an endpoint base")),
        );
    }
    router.register_service(EmulatedService::new(engine));
    router
}

/// One request to `host`, which is the endpoint the caller reached this emulator at.
async fn send_to(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Answer {
    send_bytes_signed(router, &authorization(), method, host, uri, headers, body).await
}

/// `PUT /<bucket>` carrying the location constraint that places the bucket in [`REGION`] —
/// `CreateBucket`, which answers the new bucket's `Location` header.
async fn create_bucket(router: &MultiServiceRouter, host: &str, bucket: &str) -> Answer {
    let body = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint>\
         </CreateBucketConfiguration>"
    );
    let created = send_to(
        router,
        host,
        "PUT",
        &format!("/{bucket}"),
        &[("content-type", "application/xml")],
        body.as_bytes(),
    )
    .await;
    assert_eq!(
        created.status,
        200,
        "creating {bucket:?} must answer HTTP 200, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
}

/// A whole multipart upload of one part at `key`, completed — `CreateMultipartUpload`,
/// `UploadPart` and `CompleteMultipartUpload`, answering the completion. `host` addresses
/// every leg, and `path` is the request path the bucket is addressed by (the bucket and the
/// key for a path-style request, the key alone for a virtual-hosted-style one).
async fn complete_upload(
    router: &MultiServiceRouter,
    host: &str,
    path: &str,
    key: &str,
) -> Answer {
    let initiated = send_to(router, host, "POST", &format!("{path}?uploads"), &[], b"").await;
    assert_eq!(
        initiated.status,
        200,
        "initiating an upload of {key:?} must answer HTTP 200, got {} with body {:?}",
        initiated.status,
        String::from_utf8_lossy(&initiated.body)
    );
    let upload_id = initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId");
    let uploaded = send_to(
        router,
        host,
        "PUT",
        &format!("{path}?partNumber=1&uploadId={upload_id}&x-id=UploadPart"),
        &[],
        b"assembled",
    )
    .await;
    assert_eq!(
        uploaded.status,
        200,
        "uploading the part of {key:?} must answer HTTP 200, got {} with body {:?}",
        uploaded.status,
        String::from_utf8_lossy(&uploaded.body)
    );
    let etag = uploaded
        .header("etag")
        .expect("a part upload must report an ETag header");
    let payload = format!(
        "<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>{etag}</ETag></Part>\
         </CompleteMultipartUpload>"
    );
    let completed = send_to(
        router,
        host,
        "POST",
        &format!("{path}?uploadId={upload_id}"),
        &[("content-type", "application/xml")],
        payload.as_bytes(),
    )
    .await;
    assert_eq!(
        completed.status,
        200,
        "completing the upload of {key:?} must answer HTTP 200, got {} with body {:?}",
        completed.status,
        String::from_utf8_lossy(&completed.body)
    );
    completed
}

/// The browser-form upload of one object at `key`, asking for the `201` that carries the
/// `PostResponse` document — the third answer of this service that names a URL.
async fn post_form(router: &MultiServiceRouter, host: &str, bucket: &str, key: &str) -> Answer {
    let media_type = form_media_type();
    let answered = send_to(
        router,
        host,
        "POST",
        &format!("/{bucket}/"),
        &[("content-type", media_type.as_str())],
        &form_document(
            &[("key", key), ("success_action_status", "201")],
            Some(("quarterly.csv", b"browser,form", None)),
        ),
    )
    .await;
    assert_eq!(
        answered.status,
        201,
        "a form naming the 201 status must answer it, got {} with body {:?}",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
    answered
}

/// One row of the endpoint table: the case, the base this emulator is served at (`None` for
/// one served on no port), and the root every URL it announces must carry.
type EndpointRow<'a> = (&'a str, Option<&'a str>, &'a str);

/// RULE: every absolute URL this service announces for a resource is rooted on the endpoint
/// serving the request, with the bucket and the key beneath it — never on a host under
/// `amazonaws.com`, which resolves away from this emulator.
///
/// The rows differ along the endpoint this emulator is served at, which is what the rule
/// generalises over, and the keys span a plain key, a key prefix (whose `/` separators stay
/// path separators) and a key carrying characters a URI path cannot hold verbatim.
#[tokio::test]
async fn every_announced_url_names_the_endpoint_serving_the_request() {
    let rows: [EndpointRow<'_>; 4] = [
        ("served on no port at all", None, UNSERVED_URL_BASE),
        (
            "a loopback address with a port",
            Some("https://127.0.0.1:39355"),
            "https://127.0.0.1:39355",
        ),
        (
            "a bracketed IPv6 address with a port",
            Some("https://[::1]:9297"),
            "https://[::1]:9297",
        ),
        (
            "a name with a port",
            Some("https://emulator.example:8443"),
            "https://emulator.example:8443",
        ),
    ];

    // (the key the object is written at, the path a request addresses it by, the path the
    // announced URL must carry it as).
    let keys: [(&str, &str, &str); 3] = [
        ("assembled.txt", "assembled.txt", "assembled.txt"),
        (
            "reports/2026/q1.csv",
            "reports/2026/q1.csv",
            "reports/2026/q1.csv",
        ),
        (
            "quarterly report (final).csv",
            "quarterly%20report%20(final).csv",
            "quarterly%20report%20%28final%29.csv",
        ),
    ];

    for (case, base, root) in rows {
        let router = router_served_at(base);

        let bucket = "announced-bucket";
        let created = create_bucket(&router, AWS_REGIONAL_HOST, bucket).await;
        let expected = format!("{root}/{bucket}/");
        assert_eq!(
            created.header("location").as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the created bucket's Location {expected:?}, got {:?}",
            created.header("location")
        );

        for (key, request_path, encoded) in keys {
            let completed = complete_upload(
                &router,
                AWS_REGIONAL_HOST,
                &format!("/{bucket}/{request_path}"),
                key,
            )
            .await;
            let expected = format!("{root}/{bucket}/{encoded}");
            assert_eq!(
                completed.element("Location").as_deref(),
                Some(expected.as_str()),
                "case {case:?}, key {key:?}: expected the completed object's Location \
                 {expected:?}, got {:?}",
                completed.element("Location")
            );

            let posted = post_form(&router, AWS_REGIONAL_HOST, bucket, key).await;
            let expected = format!("{root}/{bucket}/{encoded}");
            assert_eq!(
                posted.element("Location").as_deref(),
                Some(expected.as_str()),
                "case {case:?}, key {key:?}: expected the posted object's Location \
                 {expected:?}, got {:?}",
                posted.element("Location")
            );
        }
    }
}

/// RULE: no URL this service announces names a host under `amazonaws.com` — the half of the
/// rule a client in a sandbox with no egress reads, asserted over every announcing answer
/// and both addressing styles.
#[tokio::test]
async fn no_announced_url_names_a_public_aws_host() {
    let router = router_served_at(Some("https://127.0.0.1:39355"));
    let bucket = "no-aws-host-bucket";
    let key = "assembled.txt";

    let created = create_bucket(&router, AWS_REGIONAL_HOST, bucket).await;
    let completed =
        complete_upload(&router, AWS_REGIONAL_HOST, &format!("/{bucket}/{key}"), key).await;
    let posted = post_form(&router, AWS_REGIONAL_HOST, bucket, key).await;

    let announced: [(&str, Option<String>); 3] = [
        ("CreateBucket's Location header", created.header("location")),
        (
            "CompleteMultipartUpload's Location element",
            completed.element("Location"),
        ),
        ("the PostResponse Location element", posted.element("Location")),
    ];
    for (case, url) in announced {
        let url = url.unwrap_or_else(|| panic!("case {case:?}: no URL was announced"));
        assert!(
            !url.contains("amazonaws.com"),
            "case {case:?}: an announced URL must not name a public AWS host, got {url:?}"
        );
    }
}

/// One row of the addressing table: the case, the host the request is addressed to, the
/// path the bucket is addressed by, and the root the announced URL must carry.
type AddressingRow<'a> = (&'a str, &'a str, &'a str, &'a str);

/// RULE: the announced root is the emulator's own endpoint the request REACHED when the
/// request reached one, and the endpoint this process is served at otherwise — so a client
/// configured with one local endpoint URL is handed back the very address it is already
/// talking to, and a caller that addressed one of AWS's own hosts, or a bucket's own
/// published hostname, is handed the address this process answers on.
///
/// The rows differ along the host the request addressed: the served endpoint itself, another
/// address of the same emulator, one of AWS's own regional hosts, and the bucket's own
/// virtual-hosted address — and the announced URL addresses the bucket path-style under that
/// root either way.
#[tokio::test]
async fn the_announced_root_is_the_endpoint_the_request_reached() {
    const SERVED: &str = "https://127.0.0.1:39355";
    let bucket = "reached-endpoint-bucket";
    let key = "assembled.txt";

    let rows: [AddressingRow<'_>; 4] = [
        (
            "the served endpoint itself",
            "127.0.0.1:39355",
            "path",
            "https://127.0.0.1:39355",
        ),
        (
            "another address of the same emulator",
            "localhost:39355",
            "path",
            "https://localhost:39355",
        ),
        (
            "one of AWS's own regional hosts falls back to the served endpoint",
            AWS_REGIONAL_HOST,
            "path",
            SERVED,
        ),
        (
            "the bucket's own virtual-hosted address falls back to the served endpoint",
            "reached-endpoint-bucket.localhost:39355",
            "host",
            SERVED,
        ),
    ];

    for (case, host, style, root) in rows {
        // One emulator per row: each row creates the same bucket, so the create under test
        // is a fresh one rather than a re-create.
        let router = router_served_at(Some(SERVED));
        // The create is always path-style: a virtual-hosted address resolves against the
        // buckets this emulator HOLDS, and the bucket does not exist until the create runs.
        let created = create_bucket(&router, AWS_REGIONAL_HOST, bucket).await;
        assert_eq!(
            created.header("location").as_deref(),
            Some(format!("{SERVED}/{bucket}/").as_str()),
            "case {case:?}: the create was addressed to the served endpoint, so its \
             Location must name it, got {:?}",
            created.header("location")
        );

        // A virtual-hosted-style request carries the bucket in its HOST and the key alone in
        // its path; a path-style one carries both in the path.
        let path = match style {
            "host" => format!("/{key}"),
            _ => format!("/{bucket}/{key}"),
        };
        let completed = complete_upload(&router, host, &path, key).await;
        let expected = format!("{root}/{bucket}/{key}");
        assert_eq!(
            completed.element("Location").as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected the completed object's Location {expected:?}, got {:?}",
            completed.element("Location")
        );
    }
}
