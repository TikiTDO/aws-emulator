// SPDX-License-Identifier: CC0-1.0

//! Cloud Control API over the buckets Amazon S3 holds.
//!
//! The rule every case here holds the service to: **a resource of a type another service of
//! this emulator owns is provisioned, resolved, patched and removed in THAT service's own
//! state — "you can use this action to return information about an existing resource in
//! your account and Amazon Web Services Region, whether those resources were provisioned
//! using Cloud Control API" (`GetResource`) — so a `CreateResource` of an
//! `AWS::S3::Bucket` that reaches `SUCCESS` has created the bucket Amazon S3's own API then
//! serves objects in, a bucket that service no longer holds resolves to nothing, and work
//! that service refuses leaves the request `FAILED` carrying its handler's verdict instead
//! of a success nothing performed.**
//!
//! The rows differ along what the rule generalises over: which properties the desired state
//! carries (the identity alone, a tag set, a versioning configuration, both), which API the
//! bucket came into being through, what the owning service then serves on it (a head, an
//! object write and read back, its own subresource readers), and which verdict that service
//! returns for work it will not do (a name it holds no bucket under, a name already taken, a
//! bucket that is not empty, a property its handler does not apply).
//!
//! `sqs_queue_read_through` and `native_resource_resolution` hold the same rule for the
//! `AWS::SQS::Queue` and `AWS::Logs::*` types, so what is asserted across the suites is
//! where a resource LIVES rather than anything about one owning service.

use serde_json::{json, Value};

use super::emulator::{progress, text, Emulator, STATUS_FAILED, STATUS_SUCCESS};

/// The type whose resources live in the Amazon S3 service's own store.
const BUCKET: &str = "AWS::S3::Bucket";

/// The ARN of the bucket called `name`, built the way the OWNING service's own reader
/// builds it — a general purpose bucket is named by its globally unique name alone — so a
/// case pins the bucket Cloud Control API resolved to the one Amazon S3 holds rather than to
/// a string this suite spells.
fn bucket_arn(name: &str) -> String {
    service_com_amazonaws_s3_amazons3::state::bucket_arn(name)
}

/// One `Tags` entry, as the type's schema carries a tag: a `{ Key, Value }` pair.
fn tag(key: &str, value: &str) -> Value {
    json!({ "Key": key, "Value": value })
}

/// Create a bucket through the OWNING service's own API — `s3:CreateBucket`. Nothing of
/// such a bucket is ever named to Cloud Control API.
async fn create_native(emulator: &Emulator, name: &str) {
    let (status, body) = emulator.s3_bucket_call("PUT", &format!("/{name}")).await;
    assert_eq!(
        status, 200,
        "s3:CreateBucket {name:?}: expected HTTP 200, got {status} with body {body:?}"
    );
}

/// Whether the OWNING service holds a bucket called `name`, as its own `s3:HeadBucket`
/// answers.
async fn native_holds(emulator: &Emulator, name: &str) -> bool {
    let (status, _) = emulator.s3_bucket_call("HEAD", &format!("/{name}")).await;
    assert!(
        status == 200 || status == 404,
        "s3:HeadBucket {name:?}: expected 200 or 404, got {status}"
    );
    status == 200
}

/// Whether THIS service resolves a bucket called `name` — which, for a type whose
/// resources live in another service's store, is whether that service holds one. A name
/// that service's own front door refuses outright is one no bucket of it answers to, so it
/// is asked this way rather than through a reader of its own.
async fn resolves(emulator: &Emulator, name: &str) -> bool {
    let (status, body) = emulator.get(BUCKET, name).await;
    assert!(
        status == 200 || status == 404,
        "GetResource {name:?}: expected 200 or 404, got {status} with body {body:?}"
    );
    status == 200
}

#[tokio::test]
async fn a_created_bucket_is_the_bucket_amazon_s3_holds() {
    let emulator = Emulator::with_s3();

    // The rows differ along which of the properties this type's schema declares the desired
    // state carries.
    let rows: [(&str, &str, Value, Value); 4] = [
        (
            "the identity alone",
            "plain-bucket",
            json!({ "BucketName": "plain-bucket" }),
            json!({ "BucketName": "plain-bucket", "Arn": bucket_arn("plain-bucket") }),
        ),
        (
            "a tag set",
            "tagged-bucket",
            json!({
                "BucketName": "tagged-bucket",
                "Tags": [tag("team", "web"), tag("env", "test")],
            }),
            json!({
                "BucketName": "tagged-bucket",
                "Arn": bucket_arn("tagged-bucket"),
                "Tags": [tag("team", "web"), tag("env", "test")],
            }),
        ),
        (
            "a versioning configuration",
            "versioned-bucket",
            json!({
                "BucketName": "versioned-bucket",
                "VersioningConfiguration": { "Status": "Enabled" },
            }),
            json!({
                "BucketName": "versioned-bucket",
                "Arn": bucket_arn("versioned-bucket"),
                "VersioningConfiguration": { "Status": "Enabled" },
            }),
        ),
        (
            "a tag set and a versioning configuration",
            "both-bucket",
            json!({
                "BucketName": "both-bucket",
                "Tags": [tag("owner", "platform")],
                "VersioningConfiguration": { "Status": "Suspended" },
            }),
            json!({
                "BucketName": "both-bucket",
                "Arn": bucket_arn("both-bucket"),
                "Tags": [tag("owner", "platform")],
                "VersioningConfiguration": { "Status": "Suspended" },
            }),
        ),
    ];

    // Started together, then waited for: the creates are independent, so one span covers
    // them all.
    let mut accepted = Vec::new();
    for (_, _, desired, _) in &rows {
        let event = emulator.create_ok(BUCKET, desired, None).await;
        accepted.push(text(&event, "RequestToken"));
    }
    for ((case, name, _, expected), request_token) in rows.iter().zip(&accepted) {
        let settled = emulator.settled(request_token).await;
        let identifier = text(&settled, "Identifier");
        assert_eq!(
            &identifier, name,
            "case {case:?}: the create must report the name the desired state carries"
        );
        let reported = emulator.properties(BUCKET, &identifier).await;
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected the resource model {expected:?}, got {reported:?}"
        );
        // The bucket EXISTS in the owning service: its own reader answers for it.
        assert!(
            native_holds(&emulator, name).await,
            "case {case:?}: Amazon S3 must hold the bucket Cloud Control API created"
        );
    }

    // And each property the create asked for is the one the OWNING service's own
    // subresource readers report, so nothing was recorded in Cloud Control API's terms
    // alone.
    let (status, tagging) = emulator
        .s3_bucket_call("GET", "/tagged-bucket?tagging")
        .await;
    assert_eq!(
        status, 200,
        "s3:GetBucketTagging: expected HTTP 200, got {status} with body {tagging:?}"
    );
    for fragment in [
        "<Key>team</Key>",
        "<Value>web</Value>",
        "<Key>env</Key>",
        "<Value>test</Value>",
    ] {
        assert!(
            tagging.contains(fragment),
            "s3:GetBucketTagging must report {fragment:?}, got {tagging:?}"
        );
    }
    let (status, versioning) = emulator
        .s3_bucket_call("GET", "/versioned-bucket?versioning")
        .await;
    assert_eq!(
        status, 200,
        "s3:GetBucketVersioning: expected HTTP 200, got {status} with body {versioning:?}"
    );
    assert!(
        versioning.contains("<Status>Enabled</Status>"),
        "s3:GetBucketVersioning must report the status the create asked for, got \
         {versioning:?}"
    );

    // Every bucket a create provisioned is reported by this service's own listing of the
    // type, which reads the owning store too.
    let mut listed = emulator.walk(BUCKET, None).await;
    listed.sort();
    let mut expected: Vec<String> = rows.iter().map(|(_, name, _, _)| name.to_string()).collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "the listing must report the buckets Amazon S3 holds, expected {expected:?}, got \
         {listed:?}"
    );
}

#[tokio::test]
async fn objects_can_be_written_into_a_bucket_cloud_control_provisioned() {
    let emulator = Emulator::with_s3();
    emulator
        .provision(BUCKET, &json!({ "BucketName": "object-bucket" }))
        .await;

    // The rows differ along the payload written: text, bytes with no text in them, and an
    // empty object.
    let rows: [(&str, &str, &[u8]); 3] = [
        ("text", "notes/blocker.txt", b"blocking-content"),
        ("binary", "data/blob.bin", &[0_u8, 1, 2, 250, 255]),
        ("an empty payload", "data/empty", b""),
    ];
    for (case, key, payload) in rows {
        let (status, body) = emulator
            .s3(
                "PUT",
                &format!("/object-bucket/{key}"),
                "application/octet-stream",
                payload,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: writing {key} into the provisioned bucket must answer 200, got \
             {status} with body {:?}",
            String::from_utf8_lossy(&body)
        );
        let (status, read) = emulator
            .s3("GET", &format!("/object-bucket/{key}"), "application/octet-stream", b"")
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the object written at {key} must be readable, got {status}"
        );
        assert_eq!(
            read.as_slice(),
            payload,
            "case {case:?}: the read must answer exactly the stored bytes"
        );
    }

    // A bucket nothing created is still no bucket: the same write into a name no create
    // named is refused by the owning service.
    let (status, body) = emulator
        .s3(
            "PUT",
            "/never-provisioned-bucket/blocker.txt",
            "application/octet-stream",
            b"blocking-content",
        )
        .await;
    let body = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 404,
        "a write into a bucket nothing created must be refused, got {status} with body \
         {body:?}"
    );
    assert!(
        body.contains("NoSuchBucket"),
        "that refusal must be the owning service's own NoSuchBucket, got {body:?}"
    );
}

#[tokio::test]
async fn a_delete_waits_for_the_owning_service_to_allow_it() {
    let emulator = Emulator::with_s3();
    emulator
        .provision(BUCKET, &json!({ "BucketName": "occupied-bucket" }))
        .await;
    let (status, _) = emulator
        .s3(
            "PUT",
            "/occupied-bucket/blocker.txt",
            "application/octet-stream",
            b"blocking-content",
        )
        .await;
    assert_eq!(status, 200, "the object write must succeed, got {status}");

    // "All objects (including all object versions and delete markers) in the bucket must be
    // deleted before the bucket itself can be deleted": the owning service refuses the
    // delete, so the request FAILS with that handler's verdict and the bucket stays.
    let (status, body) = emulator.delete(BUCKET, "occupied-bucket").await;
    assert_eq!(
        status, 200,
        "the delete must be accepted, got {status} with body {body:?}"
    );
    let blocked = emulator
        .terminal(&text(&progress(&body), "RequestToken"))
        .await;
    assert_eq!(
        text(&blocked, "OperationStatus"),
        STATUS_FAILED,
        "the delete of a bucket that is not empty must fail, got {blocked:?}"
    );
    assert_eq!(
        text(&blocked, "ErrorCode"),
        "GeneralServiceException",
        "the delete of a bucket that is not empty: unexpected handler error code"
    );
    assert_eq!(
        text(&blocked, "StatusMessage"),
        "The bucket you tried to delete is not empty",
        "the delete of a bucket that is not empty: unexpected wording"
    );
    assert!(
        native_holds(&emulator, "occupied-bucket").await,
        "the refused delete must leave the bucket where it was"
    );

    // Emptied through the owning service's own API, the same delete goes through — and the
    // bucket stops existing in BOTH views.
    let (status, _) = emulator
        .s3(
            "DELETE",
            "/occupied-bucket/blocker.txt",
            "application/octet-stream",
            b"",
        )
        .await;
    assert_eq!(status, 204, "the object delete must succeed, got {status}");
    let removed = emulator.delete_and_settle(BUCKET, "occupied-bucket").await;
    assert_eq!(
        text(&removed, "OperationStatus"),
        STATUS_SUCCESS,
        "the delete of an emptied bucket must succeed, got {removed:?}"
    );
    assert!(
        !native_holds(&emulator, "occupied-bucket").await,
        "Amazon S3 must no longer hold a bucket Cloud Control API deleted"
    );
    let (status, body) = emulator.get(BUCKET, "occupied-bucket").await;
    assert_eq!(
        status, 404,
        "the deleted bucket must resolve to nothing, got {status} with body {body:?}"
    );
}

#[tokio::test]
async fn a_bucket_the_owning_api_created_is_the_same_resource() {
    let emulator = Emulator::with_s3();
    create_native(&emulator, "native-bucket").await;

    // Cloud Control API resolves a resource whichever API created it.
    let reported = emulator.properties(BUCKET, "native-bucket").await;
    assert_eq!(
        reported,
        json!({ "BucketName": "native-bucket", "Arn": bucket_arn("native-bucket") }),
        "a bucket the owning API created must resolve, got {reported:?}"
    );
    let listed = emulator.walk(BUCKET, None).await;
    assert_eq!(
        listed,
        vec!["native-bucket".to_string()],
        "a bucket the owning API created must be listed, got {listed:?}"
    );

    // A patch through this service is a write to THAT store: the tag set it leaves is the
    // one the owning service's own reader reports.
    let patch = json!([{ "op": "add", "path": "/Tags", "value": [tag("team", "web")] }]);
    emulator
        .update_and_settle(BUCKET, "native-bucket", &patch)
        .await;
    let (status, tagging) = emulator
        .s3_bucket_call("GET", "/native-bucket?tagging")
        .await;
    assert_eq!(
        status, 200,
        "s3:GetBucketTagging: expected HTTP 200, got {status} with body {tagging:?}"
    );
    assert!(
        tagging.contains("<Key>team</Key>") && tagging.contains("<Value>web</Value>"),
        "the patch must be a write to the owning store, got {tagging:?}"
    );

    // And a bucket that service no longer holds resolves to nothing here either.
    let (status, body) = emulator.s3_bucket_call("DELETE", "/native-bucket").await;
    assert_eq!(
        status, 204,
        "s3:DeleteBucket: expected HTTP 204, got {status} with body {body:?}"
    );
    let (status, body) = emulator.get(BUCKET, "native-bucket").await;
    assert_eq!(
        status, 404,
        "a bucket the owning API deleted must resolve to nothing, got {status} with body \
         {body:?}"
    );
}

#[tokio::test]
async fn work_the_owning_service_refuses_leaves_the_request_failed() {
    let emulator = Emulator::with_s3();
    emulator
        .provision(BUCKET, &json!({ "BucketName": "taken-bucket" }))
        .await;

    // The rows differ along the verdict the owning service returns: a name it holds no
    // bucket under (each published naming rule the name breaks is one row), a name already
    // taken, and a property of the type its handler here does not apply.
    //
    // Catalog source for the names: the *General purpose bucket naming rules* of the Amazon
    // S3 User Guide, as the one shared reading of them in this workspace states them
    // (`service_common::s3_bucket_name`) — the length bounds, the character class, the
    // first and last character, adjacent periods, an IP address, and the reserved prefixes
    // and suffixes.
    let refused_names = [
        ("one character short of the minimum", "ab"),
        ("one character past the maximum", &"b".repeat(64)),
        ("an upper case letter", "Upper-Bucket"),
        ("an underscore", "under_score"),
        ("a leading hyphen", "-leading-hyphen"),
        ("a trailing dot", "trailing-dot."),
        ("two adjacent periods", "two..dots"),
        ("an IP address", "192.168.5.4"),
        ("a reserved prefix", "xn--bucket"),
        ("a reserved suffix", "bucket-s3alias"),
    ];
    // Started together, then waited for: the creates are independent, so one span covers
    // them all.
    let mut accepted = Vec::new();
    for (_, name) in &refused_names {
        let event = emulator
            .create_ok(BUCKET, &json!({ "BucketName": name }), None)
            .await;
        accepted.push(text(&event, "RequestToken"));
    }
    for ((case, name), request_token) in refused_names.iter().zip(&accepted) {
        let failed = emulator.terminal(request_token).await;
        assert_eq!(
            text(&failed, "OperationStatus"),
            STATUS_FAILED,
            "case {case:?}: a name Amazon S3 holds no bucket under must fail, got {failed:?}"
        );
        assert_eq!(
            text(&failed, "ErrorCode"),
            "InvalidRequest",
            "case {case:?}: unexpected handler error code"
        );
        assert_eq!(
            text(&failed, "StatusMessage"),
            "The specified bucket is not valid.",
            "case {case:?}: unexpected wording"
        );
        assert!(
            !resolves(&emulator, name).await,
            "case {case:?}: a failed create must have provisioned nothing"
        );
    }

    // Each published length bound is admitted, so the rule is a bound rather than a
    // rejection of anything unusual: three characters and sixty-three. Started together
    // too, for the same reason.
    let lengths = [3_usize, 63];
    let mut accepted = Vec::new();
    for length in lengths {
        let event = emulator
            .create_ok(BUCKET, &json!({ "BucketName": "b".repeat(length) }), None)
            .await;
        accepted.push(text(&event, "RequestToken"));
    }
    for (length, request_token) in lengths.iter().zip(&accepted) {
        let name = "b".repeat(*length);
        let settled = emulator.settled(request_token).await;
        assert_eq!(
            text(&settled, "Identifier"),
            name,
            "length {length}: a name at a published bound must be accepted"
        );
        assert!(
            native_holds(&emulator, &name).await,
            "length {length}: Amazon S3 must hold the bucket that create provisioned"
        );
    }

    // A name the owning store already holds is that service's own already-exists verdict,
    // and a property this handler does not apply is refused rather than accepted and
    // dropped: a property a caller sets has to be one a later read reports. Every row below
    // is started together with the duplicate create, for the same reason as above.
    let duplicate_token = text(
        &emulator
            .create_ok(BUCKET, &json!({ "BucketName": "taken-bucket" }), None)
            .await,
        "RequestToken",
    );
    let rows = [
        (
            "a property the type declares that this handler does not apply",
            json!({
                "BucketName": "unapplied-bucket",
                "PublicAccessBlockConfiguration": { "BlockPublicAcls": true },
            }),
            "PublicAccessBlockConfiguration is a property of AWS::S3::Bucket this \
             emulator's handler does not apply",
        ),
        (
            "a property the type does not declare at all",
            json!({ "BucketName": "unknown-bucket", "Secret": "s3cret" }),
            "Secret is not a property of AWS::S3::Bucket",
        ),
        (
            "a versioning status outside the pair the owning service admits",
            json!({
                "BucketName": "misversioned-bucket",
                "VersioningConfiguration": { "Status": "Paused" },
            }),
            "VersioningConfiguration.Status must be one of [\"Enabled\", \"Suspended\"]",
        ),
        (
            "a tag key in the namespace reserved for Amazon Web Services",
            json!({
                "BucketName": "reserved-tag-bucket",
                "Tags": [tag("aws:created-by", "me")],
            }),
            "System tags cannot be added/updated by requester",
        ),
    ];
    let mut accepted = Vec::new();
    for (_, desired, _) in &rows {
        let event = emulator.create_ok(BUCKET, desired, None).await;
        accepted.push(text(&event, "RequestToken"));
    }
    let duplicate = emulator.terminal(&duplicate_token).await;
    assert_eq!(
        text(&duplicate, "OperationStatus"),
        STATUS_FAILED,
        "a second create of the same bucket must fail, got {duplicate:?}"
    );
    assert_eq!(
        text(&duplicate, "ErrorCode"),
        "AlreadyExists",
        "a second create of the same bucket: unexpected handler error code"
    );
    for ((case, desired, detail), request_token) in rows.iter().zip(&accepted) {
        let name = text(desired, "BucketName");
        let failed = emulator.terminal(request_token).await;
        assert_eq!(
            text(&failed, "OperationStatus"),
            STATUS_FAILED,
            "case {case:?}: expected the request to fail, got {failed:?}"
        );
        assert_eq!(
            text(&failed, "ErrorCode"),
            "InvalidRequest",
            "case {case:?}: unexpected handler error code"
        );
        assert_eq!(
            text(&failed, "StatusMessage"),
            *detail,
            "case {case:?}: unexpected wording"
        );
        assert!(
            !native_holds(&emulator, &name).await,
            "case {case:?}: a failed create must have provisioned nothing"
        );
    }
}
