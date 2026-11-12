// SPDX-License-Identifier: CC0-1.0

//! `PutRawMessageContent` resolves its `S3Reference` against AMAZON S3's own object
//! store.
//!
//! RULE: the content an update installs is the bytes Amazon S3 holds for the object
//! the request's `s3Reference` names — the model documents `S3Reference` as "Amazon
//! S3 object representing the updated message content, in MIME format"
//! (`smithy_query(query="shape", shape="S3Reference")`) — so an object any S3 client
//! wrote is resolvable, a reference naming no `objectVersion` addresses the key's
//! current version while one naming a version addresses exactly that version, and
//! what S3 does to the object afterwards is what the next update sees. The bucket's
//! own Region and owning account are what the two documented
//! `InvalidContentLocation` reasons are decided from, since they are properties of
//! the bucket S3 holds (`crate::raw_message_content` is where those rows are).
//!
//! The objects here are staged through S3's own `CreateBucket`/`PutObject`/
//! `DeleteObject` seams on the one shared engine — never through a copy of them kept
//! in this service's state, which no S3 client could write — so every row asks the
//! question an SDK client asks: "I uploaded this; does the update install it?" The
//! rows differ along what the rule generalises over: the bytes, the key, the bucket,
//! whether the bucket is versioning-enabled, which version the reference names, and
//! what S3 did to the object between two updates. The same path over the wire with
//! both services mounted is in the router's own suite
//! (`core/router/tests/all/workmailmessageflow_resolves_an_s3_object.rs`).

use service_com_amazonaws_workmailmessageflow_giraffemessageintransitservice::state::S3Ref;

use crate::raw_message_content::{
    mime, reference, versioned_reference, Emulator, StagedObject, DATE, REGION,
};

/// Rule, first half: the bytes S3 holds for the referenced object are the bytes the
/// update installs. The rows differ along the content, the key and the bucket.
#[tokio::test]
async fn an_update_installs_the_bytes_amazon_s3_holds() {
    // (case, bucket, key, the bytes a client uploaded)
    let rows: Vec<(&str, &str, &str, Vec<u8>)> = vec![
        (
            "the issue's recorded reference",
            "mailbucket",
            "updated/message-mime",
            mime("rewritten through S3", "body"),
        ),
        (
            "a bucket name at the modeled minimum length",
            "abc",
            "k",
            mime("tiny bucket", "body"),
        ),
        (
            "a nested key",
            "flow-objects",
            "mail/2026/10/updated-mime",
            mime("nested key", "body"),
        ),
        (
            "headers only, no body",
            "flow-objects",
            "headers-only",
            format!("Date: {DATE}\r\nFrom: a@b\r\n").into_bytes(),
        ),
        ("content of a few kilobytes", "flow-objects", "large", {
            let mut content = mime("large", "body");
            for index in 0..512 {
                content.extend(format!("paragraph {index}\r\n").as_bytes());
            }
            content
        }),
    ];

    for (case, bucket, key, uploaded) in rows {
        let emulator = Emulator::new();
        let message_id = "00000000-0000-0000-0000-000000000001";
        let original = mime("original", "body");
        emulator.seed_message(message_id, &original);
        let s3_reference = emulator.seed_object(
            &S3Ref::new(bucket, key, None),
            StagedObject::new(uploaded.clone()),
        );

        let put = emulator
            .put_raw_message_content(REGION, message_id, &s3_reference)
            .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected HTTP 200 from an update naming an object S3 holds, got {} \
             ({})",
            put.status,
            put.summary()
        );
        let after = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            after.payload.as_ref(),
            uploaded.as_slice(),
            "case {case:?}: expected the {} octet(s) S3 holds, got {}",
            uploaded.len(),
            after.summary()
        );
    }
}

/// What the client does to the object between the two updates of
/// [`what_s3_does_to_the_object_is_what_the_next_update_resolves`].
#[derive(Debug, Clone, Copy)]
enum Then {
    /// A second `PutObject` at the same key, with different bytes.
    Overwrite,
    /// A `DeleteObject`, which leaves the key with a delete marker over it.
    Delete,
}

/// Rule, second half: the object belongs to S3, so what S3 does to it between two
/// updates is what the second update resolves — there is no copy of it here to go
/// stale.
#[tokio::test]
async fn what_s3_does_to_the_object_is_what_the_next_update_resolves() {
    let rows: [(&str, Then); 2] = [
        ("S3 overwrites the object", Then::Overwrite),
        ("S3 deletes the object", Then::Delete),
    ];

    for (case, then) in rows {
        let emulator = Emulator::new();
        let message_id = "s3-lifecycle-0";
        emulator.seed_message(message_id, &mime("original", "body"));
        let first = mime("the first upload", "body");
        let second = mime("the second upload", "body");
        // A versioning-enabled bucket, so a delete lays a marker over the key the
        // way it does for a client that has versioning on.
        let staged = versioned_reference("lifecycle/object");
        let uploaded = emulator.seed_object(&staged, StagedObject::new(first.clone()));
        // The reference the requests below carry names no version, so each one
        // addresses whatever the key's CURRENT version is when it is made.
        let current = S3Ref::new(uploaded.bucket.clone(), uploaded.key.clone(), None);

        let before = emulator
            .put_raw_message_content(REGION, message_id, &current)
            .await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: expected the first update to install the first upload, got {} ({})",
            before.status,
            before.summary()
        );
        let installed = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            installed.payload.as_ref(),
            first.as_slice(),
            "case {case:?}: expected the first upload's bytes, got {}",
            installed.summary()
        );

        match then {
            Then::Overwrite => {
                emulator.seed_object(&staged, StagedObject::new(second.clone()));
            }
            Then::Delete => emulator.delete_object(&uploaded),
        }

        let again = emulator
            .put_raw_message_content(REGION, message_id, &current)
            .await;
        let after = emulator.get_raw_message_content(REGION, message_id).await;
        match then {
            Then::Overwrite => {
                assert_eq!(
                    again.status, 200,
                    "case {case:?}: expected HTTP 200 after the overwrite, got {} ({})",
                    again.status,
                    again.summary()
                );
                assert_eq!(
                    after.payload.as_ref(),
                    second.as_slice(),
                    "case {case:?}: expected the second upload's bytes, got {}",
                    after.summary()
                );
            }
            Then::Delete => {
                assert_eq!(
                    again.status, 400,
                    "case {case:?}: expected HTTP 400 once S3 holds no current object, got {} \
                     ({})",
                    again.status,
                    again.summary()
                );
                assert_eq!(
                    again.error_type, "InvalidContentLocation",
                    "case {case:?}: expected InvalidContentLocation, got {:?}",
                    again.error_type
                );
                assert_eq!(
                    after.payload.as_ref(),
                    first.as_slice(),
                    "case {case:?}: the refused update changed the installed content: {}",
                    after.summary()
                );
            }
        }
    }
}

/// `objectVersion` selects ONE version of the object: a reference naming a version
/// S3 minted installs that version's bytes however many versions came after it,
/// while a reference naming none installs the key's current version.
#[tokio::test]
async fn a_versioned_reference_installs_the_version_it_names() {
    let uploads: [(&str, &str); 3] = [
        ("the first version", "first"),
        ("the second version", "second"),
        ("the third version", "third"),
    ];

    let emulator = Emulator::new();
    let staged = versioned_reference("versioned/message-mime");
    // Each upload is a write to the SAME key in a versioning-enabled bucket, so S3
    // mints an id per write and keeps the versions behind it.
    let mut versions: Vec<(&str, Vec<u8>, Option<String>)> = Vec::new();
    for (case, label) in uploads {
        let content = mime(label, "body");
        let resolved = emulator.seed_object(&staged, StagedObject::new(content.clone()));
        assert!(
            resolved.object_version.is_some(),
            "case {case:?}: expected S3 to mint a version id for a write into a \
             versioning-enabled bucket, got none"
        );
        assert!(
            !versions
                .iter()
                .any(|(_, _, earlier)| *earlier == resolved.object_version),
            "case {case:?}: expected a fresh version id per write, got {:?} again",
            resolved.object_version
        );
        versions.push((case, content, resolved.object_version.clone()));
    }

    // Every version still resolves to its own bytes, the oldest included.
    for (case, content, version) in &versions {
        let message_id = "versioned-0";
        emulator.seed_message(message_id, &mime("original", "body"));
        let s3_reference = S3Ref::new(staged.bucket.clone(), staged.key.clone(), version.clone());
        let put = emulator
            .put_raw_message_content(REGION, message_id, &s3_reference)
            .await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected HTTP 200 for the version S3 minted, got {} ({})",
            put.status,
            put.summary()
        );
        let after = emulator.get_raw_message_content(REGION, message_id).await;
        assert_eq!(
            after.payload.as_ref(),
            content.as_slice(),
            "case {case:?}: expected that version's own bytes, got {}",
            after.summary()
        );
    }

    // And a reference naming no version installs the CURRENT version: the last
    // upload's bytes.
    let message_id = "versioned-current";
    emulator.seed_message(message_id, &mime("original", "body"));
    let unversioned = S3Ref::new(staged.bucket.clone(), staged.key.clone(), None);
    let put = emulator
        .put_raw_message_content(REGION, message_id, &unversioned)
        .await;
    assert_eq!(
        put.status, 200,
        "case \"the current version\": expected HTTP 200, got {} ({})",
        put.status,
        put.summary()
    );
    let after = emulator.get_raw_message_content(REGION, message_id).await;
    let (_, newest, _) = versions.last().expect("three versions were uploaded");
    assert_eq!(
        after.payload.as_ref(),
        newest.as_slice(),
        "case \"the current version\": expected the newest upload's bytes, got {}",
        after.summary()
    );
}

/// A bucket this service never saw is resolvable the moment S3 holds the object:
/// nothing has to register the bucket or the object with WorkMail first, which is
/// what makes the operation usable by an out-of-process client.
#[tokio::test]
async fn an_object_s3_holds_needs_no_registration_with_this_service() {
    let emulator = Emulator::new();
    let message_id = "unregistered-0";
    let original = mime("original", "body");
    emulator.seed_message(message_id, &original);
    let s3_reference = reference("never/registered");

    // Before S3 holds anything, the reference resolves to nothing.
    let refused = emulator
        .put_raw_message_content(REGION, message_id, &s3_reference)
        .await;
    assert_eq!(
        refused.error_type, "InvalidContentLocation",
        "case \"before the upload\": expected InvalidContentLocation, got {:?} ({})",
        refused.error_type,
        refused.summary()
    );
    let unchanged = emulator.get_raw_message_content(REGION, message_id).await;
    assert_eq!(
        unchanged.payload.as_ref(),
        original.as_slice(),
        "case \"before the upload\": the refused update changed the stored content: {}",
        unchanged.summary()
    );

    // The only thing that changes is an S3 write.
    let uploaded = mime("uploaded afterwards", "body");
    let s3_reference = emulator.seed_object(&s3_reference, StagedObject::new(uploaded.clone()));
    let accepted = emulator
        .put_raw_message_content(REGION, message_id, &s3_reference)
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"after the upload\": expected HTTP 200, got {} ({})",
        accepted.status,
        accepted.summary()
    );
    let after = emulator.get_raw_message_content(REGION, message_id).await;
    assert_eq!(
        after.payload.as_ref(),
        uploaded.as_slice(),
        "case \"after the upload\": expected the uploaded bytes, got {}",
        after.summary()
    );
}
