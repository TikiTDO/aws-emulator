// SPDX-License-Identifier: CC0-1.0

//! A refusal that is ABOUT a resource names that resource in its error document.
//!
//! The rule under test: where S3 refuses a request because the resource it addresses — a
//! bucket, a configuration of a bucket, an annotation of an object — is not there, or is
//! already there, the `<Error>` document carries, beside the fixed `<Code>` and `<Message>`
//! of the error class, the member that says WHICH resource the refusal is about:
//! `<BucketName>` for a bucket-scoped refusal and `<AnnotationName>` for an
//! annotation-scoped one, copied from the request. The envelope is the only place such a
//! refusal says which resource it concerns, so a client driving several buckets or
//! annotations through one code path cannot otherwise attribute the failure, and an SDK
//! error shape exposing those members hands back nothing.
//!
//! The tables span what the rule generalises over:
//!
//! * the bucket-scoped table over the REFUSAL CLASS (a configuration that is not there, a
//!   configuration that already is, a bucket that is not there), over the CALL that answers
//!   it (a read, a create, the three table updates, a delete) and over the FORM of the
//!   bucket name the request carries (short, hyphenated with digits, dotted, and one at the
//!   long end of what a general purpose bucket name may be);
//! * the annotation-scoped table over the FORM and SIZE of the annotation name (plain,
//!   dotted, mixed case, one at the published 512-byte ceiling of
//!   `PutObjectAnnotationRequest$AnnotationName`, one spelled with a character that has to
//!   be percent-encoded in the query) and over the CALL that answers it (the read and the
//!   delete), including a name that another object DOES carry — so what the envelope names
//!   can only have come from the request. A name carrying XML metacharacters is not a name
//!   of this namespace at all (it is refused for its characters before any annotation is
//!   resolved — see [`super::object_annotations`]), so the envelope's escaping of a
//!   caller-influenced value is pinned on the upload id instead
//!   ([`super::refusal_detail_members`]);
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite in this crate owns), so what is pinned is the wire document.

use service_com_amazonaws_s3_amazons3::state::MAX_ANNOTATION_NAME_BYTES;

use super::bucket_lifecycle::{create_bucket, router, send};
use super::bucket_metadata_configuration::{create_metadata_configuration, XMLNS};
use super::object_and_tagging_roundtrip::{bucket, put_object, text};
use super::object_annotation_removal::delete_annotation;
use super::object_annotations::{get_annotation, put_annotation};

/// How the bucket a row addresses is set up before the refused call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// Never created, so the call is refused on the bucket itself.
    NotHeld,
    /// Created and carrying none of the subresources the rows below read.
    Bare,
    /// Created and already carrying a metadata configuration, so a second create collides
    /// with it.
    WithMetadataConfiguration,
}

/// One row of the bucket-scoped table: which call on which bucket, and the refusal it must
/// answer with.
#[derive(Debug, Clone)]
struct BucketRow {
    case: &'static str,
    /// The bucket the request addresses — its NAME is what the envelope must carry.
    bucket: String,
    setup: Setup,
    method: &'static str,
    /// The subresource query string, as it appears after `?` in the request URI.
    subresource: &'static str,
    /// The request payload, empty for a call that carries none.
    body: String,
    status: u16,
    code: &'static str,
}

/// One row of the annotation-scoped table: which annotation name is addressed on which
/// object, and whether any object in the store carries that name at all.
#[derive(Debug, Clone)]
struct AnnotationRow {
    case: &'static str,
    key: &'static str,
    /// The annotation name the request addresses — what the envelope must carry.
    annotation: String,
}

/// The `MetadataConfiguration` payload a create carries: the journal table the model makes
/// required, and nothing else.
fn metadata_configuration() -> String {
    format!(
        "<MetadataConfiguration xmlns=\"{XMLNS}\"><JournalTableConfiguration>\
         <RecordExpiration><Expiration>DISABLED</Expiration></RecordExpiration>\
         </JournalTableConfiguration></MetadataConfiguration>"
    )
}

/// The `JournalTableConfiguration` payload the journal-table update carries.
fn journal_update() -> String {
    format!(
        "<JournalTableConfiguration xmlns=\"{XMLNS}\"><RecordExpiration>\
         <Expiration>ENABLED</Expiration><Days>7</Days></RecordExpiration>\
         </JournalTableConfiguration>"
    )
}

/// The `InventoryTableConfiguration` payload the inventory-table update carries.
fn inventory_update() -> String {
    format!(
        "<InventoryTableConfiguration xmlns=\"{XMLNS}\">\
         <ConfigurationState>ENABLED</ConfigurationState></InventoryTableConfiguration>"
    )
}

/// The `AnnotationTableConfiguration` payload the annotation-table update carries.
fn annotation_update() -> String {
    format!(
        "<AnnotationTableConfiguration xmlns=\"{XMLNS}\">\
         <ConfigurationState>DISABLED</ConfigurationState></AnnotationTableConfiguration>"
    )
}

#[tokio::test]
async fn a_bucket_scoped_refusal_names_the_bucket_in_its_error_document() {
    let router = router();

    // A neighbour that holds a metadata configuration and a tag set throughout, so every
    // refusal below is about the bucket the request named rather than about an empty store —
    // and so a row's envelope naming the wrong bucket would be caught.
    let neighbour = "resource-naming-neighbour";
    bucket(&router, neighbour).await;
    let configured =
        create_metadata_configuration(&router, neighbour, &[], &metadata_configuration()).await;
    assert_eq!(
        configured.status,
        200,
        "the neighbour's metadata configuration must be recorded, got {} with body {:?}",
        configured.status,
        text(&configured)
    );

    let rows = vec![
        BucketRow {
            case: "a read of a metadata configuration a bucket does not carry",
            bucket: "metadata-config-absent".to_string(),
            setup: Setup::Bare,
            method: "GET",
            subresource: "metadataConfiguration",
            body: String::new(),
            status: 404,
            code: "MetadataConfigurationNotFound",
        },
        BucketRow {
            case: "a read of a replication configuration a bucket does not carry",
            bucket: "replication-absent-2026".to_string(),
            setup: Setup::Bare,
            method: "GET",
            subresource: "replication",
            body: String::new(),
            status: 404,
            code: "ReplicationConfigurationNotFoundError",
        },
        BucketRow {
            case: "a read of a tag set a bucket does not carry, under a long name",
            bucket: "a-rather-long-bucket-name-whose-text-the-envelope-must-carry".to_string(),
            setup: Setup::Bare,
            method: "GET",
            subresource: "tagging",
            body: String::new(),
            status: 404,
            code: "NoSuchTagSet",
        },
        BucketRow {
            case: "a read of an Object Lock configuration a dotted-name bucket does not carry",
            bucket: "object.lock.absent".to_string(),
            setup: Setup::Bare,
            method: "GET",
            subresource: "object-lock",
            body: String::new(),
            status: 404,
            code: "ObjectLockConfigurationNotFoundError",
        },
        BucketRow {
            case: "a create of a metadata configuration a bucket already carries",
            bucket: "metadata-config-duplicate".to_string(),
            setup: Setup::WithMetadataConfiguration,
            method: "POST",
            subresource: "metadataConfiguration",
            body: metadata_configuration(),
            status: 409,
            code: "MetadataConfigurationAlreadyExists",
        },
        BucketRow {
            case: "a journal-table update on a bucket carrying no configuration",
            bucket: "journal-update-no-config".to_string(),
            setup: Setup::Bare,
            method: "PUT",
            subresource: "metadataJournalTable",
            body: journal_update(),
            status: 404,
            code: "MetadataConfigurationNotFound",
        },
        BucketRow {
            case: "an inventory-table update on a bucket carrying no configuration",
            bucket: "inventory-update-no-config".to_string(),
            setup: Setup::Bare,
            method: "PUT",
            subresource: "metadataInventoryTable",
            body: inventory_update(),
            status: 404,
            code: "MetadataConfigurationNotFound",
        },
        BucketRow {
            case: "an annotation-table update on a bucket carrying no configuration",
            bucket: "annotation-update-no-config".to_string(),
            setup: Setup::Bare,
            method: "PUT",
            subresource: "metadataAnnotationTable",
            body: annotation_update(),
            status: 404,
            code: "MetadataConfigurationNotFound",
        },
        BucketRow {
            case: "a metadata configuration read on a bucket this emulator does not hold",
            bucket: "never.created.bucket".to_string(),
            setup: Setup::NotHeld,
            method: "GET",
            subresource: "metadataConfiguration",
            body: String::new(),
            status: 404,
            code: "NoSuchBucket",
        },
        BucketRow {
            case: "a tag set read on a bucket this emulator does not hold",
            bucket: "tagging-never-created".to_string(),
            setup: Setup::NotHeld,
            method: "GET",
            subresource: "tagging",
            body: String::new(),
            status: 404,
            code: "NoSuchBucket",
        },
        BucketRow {
            case: "a journal-table update on a bucket this emulator does not hold",
            bucket: "journal-update-never-created".to_string(),
            setup: Setup::NotHeld,
            method: "PUT",
            subresource: "metadataJournalTable",
            body: journal_update(),
            status: 404,
            code: "NoSuchBucket",
        },
        BucketRow {
            case: "a metadata configuration delete on a bucket this emulator does not hold",
            bucket: "delete-config-never-created".to_string(),
            setup: Setup::NotHeld,
            method: "DELETE",
            subresource: "metadataConfiguration",
            body: String::new(),
            status: 404,
            code: "NoSuchBucket",
        },
    ];

    for row in &rows {
        let BucketRow {
            case,
            bucket: name,
            setup,
            method,
            subresource,
            body,
            status,
            code,
        } = row;

        if *setup != Setup::NotHeld {
            let created = create_bucket(&router, name, None, None).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the bucket must be created first, got {} with body {:?}",
                created.status,
                text(&created)
            );
        }
        if *setup == Setup::WithMetadataConfiguration {
            let recorded =
                create_metadata_configuration(&router, name, &[], &metadata_configuration()).await;
            assert_eq!(
                recorded.status, 200,
                "case {case:?}: the first create must be served, got {} with body {:?}",
                recorded.status,
                text(&recorded)
            );
        }

        let refused = send(
            &router,
            method,
            &format!("/{name}?{subresource}"),
            &[],
            body,
        )
        .await;
        assert_eq!(
            refused.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected the error code {code:?}, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.element("BucketName").as_deref(),
            Some(name.as_str()),
            "case {case:?}: the error document must name the bucket the request addressed, \
             got {:?} in {:?}",
            refused.element("BucketName"),
            text(&refused)
        );
        // The naming member belongs to the error document, beside its code and message —
        // not to some other element of the body.
        let document = text(&refused);
        assert!(
            document.contains("</Message>")
                && document
                    .find("<BucketName>")
                    .is_some_and(|at| document.find("</Message>").is_some_and(|before| before < at)),
            "case {case:?}: the naming member must follow the message the way S3's error \
             document carries it, got {document:?}"
        );
    }

    // The neighbour still reports the configuration it was given, so no row's refusal
    // touched another bucket's state.
    let held = send(
        &router,
        "GET",
        &format!("/{neighbour}?metadataConfiguration"),
        &[],
        "",
    )
    .await;
    assert_eq!(
        held.status,
        200,
        "the neighbour must still report its configuration, got {} with body {:?}",
        held.status,
        text(&held)
    );
}

#[tokio::test]
async fn an_annotation_scoped_refusal_names_the_annotation_in_its_error_document() {
    let router = router();
    let bucket_name = "annotation-naming";
    bucket(&router, bucket_name).await;

    // Two objects: one carrying an annotation, one carrying none. The name the first one
    // holds is addressed on the SECOND, so what the envelope names can only have come from
    // the request.
    let held_name = "held-annotation";
    for key in ["annotated.txt", "bare.txt"] {
        let written = put_object(&router, bucket_name, key, b"annotation body", &[]).await;
        assert_eq!(
            written.status, 200,
            "the object {key:?} must be stored first, got {} with body {:?}",
            written.status,
            text(&written)
        );
    }
    let attached = put_annotation(
        &router,
        bucket_name,
        "annotated.txt",
        held_name,
        b"the payload kept throughout",
        &[],
    )
    .await;
    assert_eq!(
        attached.status, 200,
        "the annotation must be stored first, got {} with body {:?}",
        attached.status,
        text(&attached)
    );

    let rows = vec![
        AnnotationRow {
            case: "a plain name no object carries",
            key: "annotated.txt",
            annotation: "absent-annotation".to_string(),
        },
        AnnotationRow {
            case: "a dotted name no object carries",
            key: "annotated.txt",
            annotation: "team.review.notes".to_string(),
        },
        AnnotationRow {
            case: "a mixed-case name no object carries",
            key: "bare.txt",
            annotation: "Mixed-Case-Annotation".to_string(),
        },
        AnnotationRow {
            case: "a name at the published 512-byte ceiling",
            key: "bare.txt",
            // "Minimum length of 1. Maximum length of 512 bytes"
            // (`PutObjectAnnotationRequest$AnnotationName`), so a name of exactly that many
            // bytes is a name a write could have recorded and the refusal is about its
            // absence rather than its size.
            annotation: "c".repeat(MAX_ANNOTATION_NAME_BYTES),
        },
        AnnotationRow {
            case: "a name spelled with a character the query has to percent-encode",
            key: "bare.txt",
            annotation: "quarterly/report-2026".to_string(),
        },
        AnnotationRow {
            case: "a name ANOTHER object carries, addressed on an object that does not",
            key: "bare.txt",
            annotation: held_name.to_string(),
        },
    ];

    for row in &rows {
        let AnnotationRow {
            case,
            key,
            annotation,
        } = row;

        // The read and the delete of the same absent name: both are refused about the
        // annotation, so both name it.
        let read = get_annotation(&router, bucket_name, key, annotation).await;
        let removed = delete_annotation(&router, bucket_name, key, annotation, "", &[]).await;
        for (call, refused) in [("GetObjectAnnotation", &read), ("DeleteObjectAnnotation", &removed)]
        {
            assert_eq!(
                refused.status, 404,
                "case {case:?}/{call}: expected HTTP 404, got {} with body {:?}",
                refused.status,
                text(refused)
            );
            assert_eq!(
                refused.error_code(),
                "NoSuchAnnotation",
                "case {case:?}/{call}: expected the error code \"NoSuchAnnotation\", got {:?}",
                refused.error_code()
            );
            assert_eq!(
                refused.element("AnnotationName").as_deref(),
                Some(annotation.as_str()),
                "case {case:?}/{call}: the error document must name the annotation the \
                 request addressed, got {:?} in {:?}",
                refused.element("AnnotationName"),
                text(refused)
            );
        }
    }

    // The annotation that WAS stored still reads back, so every refusal above reported an
    // absent name instead of removing anything.
    let kept = get_annotation(&router, bucket_name, "annotated.txt", held_name).await;
    assert_eq!(
        kept.status, 200,
        "the stored annotation must still read back, got {} with body {:?}",
        kept.status,
        text(&kept)
    );
    assert_eq!(
        kept.body.as_ref(),
        b"the payload kept throughout",
        "the stored annotation must read back unchanged, got {:?}",
        String::from_utf8_lossy(&kept.body)
    );

    // A bucket-scoped refusal on the same annotation route still names the BUCKET: the
    // member is the one for the resource the refusal is about, not one per operation.
    let elsewhere = get_annotation(&router, "annotation-naming-never-created", "bare.txt", "any")
        .await;
    assert_eq!(
        elsewhere.status, 404,
        "a read addressed at a bucket this emulator does not hold must be refused, got {} \
         with body {:?}",
        elsewhere.status,
        text(&elsewhere)
    );
    assert_eq!(
        elsewhere.error_code(),
        "NoSuchBucket",
        "expected the error code \"NoSuchBucket\", got {:?}",
        elsewhere.error_code()
    );
    assert_eq!(
        elsewhere.element("BucketName").as_deref(),
        Some("annotation-naming-never-created"),
        "the error document must name the bucket the request addressed, got {:?} in {:?}",
        elsewhere.element("BucketName"),
        text(&elsewhere)
    );
}
