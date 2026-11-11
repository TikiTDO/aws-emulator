// SPDX-License-Identifier: CC0-1.0

//! The ELBv2 trust-store control plane, end to end over the wire.
//!
//! Every request goes through the router exactly as a client's would — awsQuery form body
//! in, `<{Operation}Response>` XML out — so each case exercises routing, the awsQuery
//! (de)serialization, the handler, the persisted state and the serializer. Amazon S3 is
//! MOUNTED beside this service on the same engine, because a trust store is built from an
//! object that store holds: the CA certificates bundle `CreateTrustStore` and
//! `ModifyTrustStore` read is one this suite really staged with `PutObject`, and the
//! certificates in it are real self-signed X.509 certificates this suite generated.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. a create PERSISTS a trust store whose `NumberOfCaCertificates` is the number of
//!    certificates read out of the bundle the request named, at the `CREATING` status the
//!    CLOCK — not a later call — owes its move to `ACTIVE`, with nothing revoked in it yet;
//!    a later describe answers from that record, by ARN and by name;
//! 2. a call whose bundle could not be READ is refused and changes nothing:
//!    `CaCertificatesBundleNotFound` where nothing can be fetched at the address,
//!    `InvalidCaCertificatesBundle` where what is there is no CA certificates bundle;
//! 3. a name identifies one trust store per Region, so a repeated name is
//!    `DuplicateTrustStoreName` and the trust store holding it is untouched;
//! 4. a modification rotates the bundle: the stored certificate count becomes the new
//!    bundle's, and the trust store's identity does not move;
//! 5. a trust store is ASSOCIATED with the listeners that authenticate client certificates
//!    against it, which is what `DescribeTrustStoreAssociations` reports (paginated) and
//!    what makes a delete `TrustStoreInUse`;
//! 6. every call that addresses a trust store by an ARN this account holds none under is
//!    `TrustStoreNotFound` naming that ARN, and a read by a name it holds none under names
//!    the NAME;
//! 7. a shared association is one with a resource of ANOTHER account: a resource of the
//!    trust store's own account is `DeleteAssociationSameAccount`, and one of an account
//!    this emulator holds no resource for is `AssociationNotFound`.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::{
    CA_CERTIFICATES_BUNDLE_NOT_FOUND_MESSAGE, INVALID_CA_CERTIFICATES_BUNDLE_MESSAGE, SETTLE_AFTER,
    TRUST_STORE_ACTIVE, TRUST_STORE_CREATING,
};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::listener_rules::create_load_balancer;
use crate::provisioning_loop::{account, action, call, first_text, members, p, Reply, REGION};
use crate::target_group_attributes_and_modification::unescaped;

/// The bucket every bundle below is staged in.
const BUNDLE_BUCKET: &str = "elbv2-trust-store-bundles";

/// A certificate ARN an HTTPS listener's default certificate resolves to, so a case that
/// needs an mTLS listener reaches the association assertions rather than the certificate
/// refusal pinned in `all/listener_tls_certificates.rs`.
const CERTIFICATE: &str = "arn:aws:acm:us-west-2:000000000000:certificate/mtls-listener";

/// The awsQuery error codes the model declares for the trust-store operations
/// (`smithy_query(query="operation",operation="CreateTrustStore,ModifyTrustStore,\
/// DeleteTrustStore,DescribeTrustStoreAssociations,DeleteSharedTrustStoreAssociation")`:
/// each error's `awsQueryError` code).
const TRUST_STORE_NOT_FOUND: &str = "TrustStoreNotFound";
const CA_BUNDLE_NOT_FOUND: &str = "CaCertificatesBundleNotFound";
const INVALID_CA_BUNDLE: &str = "InvalidCaCertificatesBundle";
const DUPLICATE_TRUST_STORE_NAME: &str = "DuplicateTrustStoreName";
const DUPLICATE_TAG_KEYS: &str = "DuplicateTagKeys";
const TRUST_STORE_IN_USE: &str = "TrustStoreInUse";
const SAME_ACCOUNT: &str = "DeleteAssociationSameAccount";
const ASSOCIATION_NOT_FOUND: &str = "AssociationNotFound";

// ---------------------------------------------------------------------------
// The emulator: this service plus the object store its trust stores are built from
// ---------------------------------------------------------------------------

/// This service and Amazon S3 over ONE shared engine, with a callback into the same router
/// so this suite stages bundles through Amazon S3's own API — the very seam the handler
/// reads them over.
struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// The Amazon S3 client, built by the one shared constructor, so this suite's writes
    /// land in the store the handler reads.
    fn s3(&self) -> aws_sdk_s3::Client {
        service_common::s3_client::s3_client(&self.callback, REGION)
    }

    /// Create [`BUNDLE_BUCKET`], the bucket every staged bundle lives in.
    ///
    /// The bucket is created IN [`REGION`] — the Region every request of this suite is
    /// signed for, and so the Region the handler reads the bundle from — which outside
    /// `us-east-1` is what the location constraint states.
    async fn bundle_bucket(&self) {
        self.s3()
            .create_bucket()
            .bucket(BUNDLE_BUCKET)
            .create_bucket_configuration(
                aws_sdk_s3::types::CreateBucketConfiguration::builder()
                    .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                    .build(),
            )
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {BUNDLE_BUCKET} must succeed, got {error:?}"));
    }

    /// Put `body` at `key` of [`BUNDLE_BUCKET`].
    async fn stage(&self, key: &str, body: &str) {
        self.s3()
            .put_object()
            .bucket(BUNDLE_BUCKET)
            .key(key)
            .body(aws_sdk_s3::primitives::ByteStream::from(
                body.as_bytes().to_vec(),
            ))
            .send()
            .await
            .unwrap_or_else(|error| panic!("staging {key} must succeed, got {error:?}"));
    }
}

/// An emulator whose bundle bucket exists.
async fn emulator_with_bundles() -> Emulator {
    let emulator = Emulator::new();
    emulator.bundle_bucket().await;
    emulator
}

// ---------------------------------------------------------------------------
// Real CA certificates, and the requests that name them
// ---------------------------------------------------------------------------

/// A bundle of `count` real self-signed CA certificates, PEM encoded one after another —
/// what `openssl` produces for a CA bundle, and what a caller stages in Amazon S3.
///
/// Each certificate is GENERATED here (`rcgen`), so every count asserted below is the
/// number of certificates a parser produced rather than a number this suite and the
/// handler both guessed.
fn ca_bundle(count: usize) -> String {
    (0..count)
        .map(|index| {
            let key = rcgen::KeyPair::generate().expect("a key pair");
            let mut params = rcgen::CertificateParams::new(vec![format!("ca{index}.example.com")])
                .expect("certificate parameters");
            params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            params
                .self_signed(&key)
                .expect("a self-signed certificate")
                .pem()
        })
        .collect()
}

/// One `CreateTrustStore` a client submits: the name, the bundle's key under
/// [`BUNDLE_BUCKET`], how many certificates that bundle holds, whether the request names an
/// object version, and the tags it carries.
struct Create {
    case: &'static str,
    name: &'static str,
    key: &'static str,
    certificates: usize,
    tags: &'static [(&'static str, &'static str)],
}

/// `CreateTrustStore` for `bucket`/`key`, with the extra fields `extra`.
async fn create_trust_store(
    emulator: &Emulator,
    name: &str,
    bucket: &str,
    key: &str,
    extra: &[(String, String)],
) -> Reply {
    let mut form = action("CreateTrustStore");
    form.push(p("Name", name));
    form.push(p("CaCertificatesBundleS3Bucket", bucket));
    form.push(p("CaCertificatesBundleS3Key", key));
    form.extend_from_slice(extra);
    call(&emulator.router, &form).await
}

/// `ModifyTrustStore` pointing `trust_store` at `bucket`/`key`.
async fn modify_trust_store(
    emulator: &Emulator,
    trust_store: &str,
    bucket: &str,
    key: &str,
) -> Reply {
    let mut form = action("ModifyTrustStore");
    form.push(p("TrustStoreArn", trust_store));
    form.push(p("CaCertificatesBundleS3Bucket", bucket));
    form.push(p("CaCertificatesBundleS3Key", key));
    call(&emulator.router, &form).await
}

/// `DescribeTrustStores` with the extra form fields `fields`.
async fn describe_trust_stores(emulator: &Emulator, fields: &[(String, String)]) -> Reply {
    let mut form = action("DescribeTrustStores");
    form.extend_from_slice(fields);
    call(&emulator.router, &form).await
}

/// `DescribeTrustStoreAssociations` for `trust_store`, with the extra fields `fields`.
async fn describe_associations(
    emulator: &Emulator,
    trust_store: &str,
    fields: &[(String, String)],
) -> Reply {
    let mut form = action("DescribeTrustStoreAssociations");
    form.push(p("TrustStoreArn", trust_store));
    form.extend_from_slice(fields);
    call(&emulator.router, &form).await
}

/// `DeleteTrustStore` for `trust_store`.
async fn delete_trust_store(emulator: &Emulator, trust_store: &str) -> Reply {
    let mut form = action("DeleteTrustStore");
    form.push(p("TrustStoreArn", trust_store));
    call(&emulator.router, &form).await
}

/// The one trust store the reply reports, or a panic naming the case.
fn only_trust_store(reply: &Reply, case: &str) -> String {
    let found = members(&reply.body, "TrustStores");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly 1 trust store, got {} in {}",
        found.len(),
        reply.body
    );
    found[0].clone()
}

/// The `ResourceArn` of every association the reply reports, in the order it reported them.
fn reported_associations(reply: &Reply) -> Vec<String> {
    members(&reply.body, "TrustStoreAssociations")
        .iter()
        .filter_map(|association| first_text(association, "ResourceArn"))
        .collect()
}

/// The `<Message>` of an error envelope, with the XML entities the serializer writes
/// resolved, so an assertion compares the VALUE rather than its encoding (the ONE reader
/// the crate's suites share).
fn message(reply: &Reply) -> String {
    unescaped(&reply.text("Message").unwrap_or_default())
}

/// An ARN of a trust store this account holds none under, named after `label`.
fn absent_trust_store_arn(label: &str) -> String {
    format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:truststore/{label}/0123456789abcdef",
        account()
    )
}

// ---------------------------------------------------------------------------
// RULE 1: the create persists what it read, and a describe answers from it
// ---------------------------------------------------------------------------

/// The rows differ along everything the rule generalises over: how many certificates the
/// bundle holds, the shape of the object key, the shape of the name within the modelled
/// pattern, and whether the create carries tags.
fn create_rows() -> Vec<Create> {
    vec![
        Create {
            case: "one certificate, a flat key, no tags",
            name: "ts-one",
            key: "one.pem",
            certificates: 1,
            tags: &[],
        },
        Create {
            case: "two certificates under a nested key",
            name: "ts-two-bundle",
            key: "bundles/nested/two.pem",
            certificates: 2,
            tags: &[("team", "platform")],
        },
        Create {
            case: "five certificates, a key with dots, two tags",
            name: "ts5",
            key: "ca.bundle.v5.pem",
            certificates: 5,
            tags: &[("team", "platform"), ("env", "test")],
        },
        Create {
            case: "a name at the modelled maximum length",
            name: "abcdefghij-abcdefghij-abcdefgh",
            key: "max-name.pem",
            certificates: 3,
            tags: &[],
        },
    ]
}

#[tokio::test]
async fn a_created_trust_store_reports_the_bundle_it_was_built_from() {
    let emulator = emulator_with_bundles().await;

    for row in create_rows() {
        emulator.stage(row.key, &ca_bundle(row.certificates)).await;
        let mut extra: Vec<(String, String)> = Vec::new();
        for (index, (key, value)) in row.tags.iter().enumerate() {
            extra.push(p(&format!("Tags.member.{}.Key", index + 1), key));
            extra.push(p(&format!("Tags.member.{}.Value", index + 1), value));
        }
        let created = create_trust_store(&emulator, row.name, BUNDLE_BUCKET, row.key, &extra).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateTrustStore, got {} body {}",
            row.case, created.status, created.body
        );

        let answered = only_trust_store(&created, row.case);
        let arn = first_text(&answered, "TrustStoreArn")
            .unwrap_or_else(|| panic!("case {:?}: no TrustStoreArn in {}", row.case, created.body));
        let expected_prefix = format!(
            "arn:aws:elasticloadbalancing:{REGION}:{}:truststore/{}/",
            account(),
            row.name
        );
        assert!(
            arn.starts_with(&expected_prefix),
            "case {:?}: expected an ARN under {expected_prefix:?}, got {arn:?}",
            row.case
        );
        let id = arn[expected_prefix.len()..].to_string();
        assert!(
            id.len() == 16 && id.chars().all(|char| char.is_ascii_hexdigit()),
            "case {:?}: expected a 16-hex resource id, got {id:?}",
            row.case
        );

        // What the create ANSWERED, and then what a read answers from the record: the
        // certificate count is the bundle's, nothing is revoked yet, and the status is the
        // one the clock still owes its move from.
        for (source, store) in [
            ("CreateTrustStore", answered.clone()),
            (
                "DescribeTrustStores by ARN",
                only_trust_store(
                    &describe_trust_stores(&emulator, &[p("TrustStoreArns.member.1", &arn)]).await,
                    row.case,
                ),
            ),
            (
                "DescribeTrustStores by name",
                only_trust_store(
                    &describe_trust_stores(&emulator, &[p("Names.member.1", row.name)]).await,
                    row.case,
                ),
            ),
        ] {
            for (member, expected) in [
                ("Name", row.name.to_string()),
                ("TrustStoreArn", arn.clone()),
                ("Status", TRUST_STORE_CREATING.to_string()),
                (
                    "NumberOfCaCertificates",
                    row.certificates.to_string(),
                ),
                ("TotalRevokedEntries", "0".to_string()),
            ] {
                assert_eq!(
                    first_text(&store, member),
                    Some(expected.clone()),
                    "case {:?} ({source}): expected {member} {expected:?}, got {:?}",
                    row.case,
                    first_text(&store, member)
                );
            }
        }

        // The tags the create carried are the tags the tagging surface reports for the ARN
        // it minted.
        let mut form = action("DescribeTags");
        form.push(p("ResourceArns.member.1", &arn));
        let tagged = call(&emulator.router, &form).await;
        for (key, value) in row.tags {
            assert!(
                tagged.body.contains(&format!("<Key>{key}</Key>"))
                    && tagged.body.contains(&format!("<Value>{value}</Value>")),
                "case {:?}: expected the tag {key}={value} in {}",
                row.case,
                tagged.body
            );
        }
    }
}

#[tokio::test]
async fn the_clock_moves_a_created_trust_store_to_active() {
    let emulator = emulator_with_bundles().await;
    emulator.stage("settling.pem", &ca_bundle(1)).await;
    let created = create_trust_store(
        &emulator,
        "ts-settling",
        BUNDLE_BUCKET,
        "settling.pem",
        &[],
    )
    .await;
    let arn = first_text(&only_trust_store(&created, "settling"), "TrustStoreArn")
        .unwrap_or_else(|| panic!("no TrustStoreArn in {}", created.body));

    // A caller acting straight after the create still sees the transient status, so the
    // span is never zero.
    let straight_after =
        describe_trust_stores(&emulator, &[p("TrustStoreArns.member.1", &arn)]).await;
    assert_eq!(
        first_text(&only_trust_store(&straight_after, "settling"), "Status").as_deref(),
        Some(TRUST_STORE_CREATING),
        "case {:?}: expected {TRUST_STORE_CREATING} straight after the create, got {}",
        "the span has not passed",
        straight_after.body
    );

    // The move is the CLOCK's: nothing but time passes here.
    tokio::time::sleep(SETTLE_AFTER + Duration::from_millis(150)).await;
    for read in ["first read", "second read"] {
        let settled = describe_trust_stores(&emulator, &[p("TrustStoreArns.member.1", &arn)]).await;
        assert_eq!(
            first_text(&only_trust_store(&settled, "settling"), "Status").as_deref(),
            Some(TRUST_STORE_ACTIVE),
            "case {:?} ({read}): expected {TRUST_STORE_ACTIVE}, got {}",
            "the span has passed",
            settled.body
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: a bundle that could not be read refuses the call and stores nothing
// ---------------------------------------------------------------------------

/// One bundle a call cannot be served from: how the case addresses it, and the refusal it
/// earns.
struct Unreadable {
    case: &'static str,
    /// The bucket the request names.
    bucket: String,
    /// The key the request names.
    key: String,
    /// The object staged at that key, if any.
    staged: Option<String>,
    code: &'static str,
    message: &'static str,
}

/// The rows span both halves of the split the model declares: nothing can be FETCHED at
/// the address, or what is there is no CA certificates BUNDLE.
fn unreadable_rows() -> Vec<Unreadable> {
    vec![
        Unreadable {
            case: "a bucket the object store holds no bucket under",
            bucket: "elbv2-trust-store-no-such-bucket".to_string(),
            key: "absent.pem".to_string(),
            staged: None,
            code: CA_BUNDLE_NOT_FOUND,
            message: CA_CERTIFICATES_BUNDLE_NOT_FOUND_MESSAGE,
        },
        Unreadable {
            case: "a key the bundle bucket holds nothing at",
            bucket: BUNDLE_BUCKET.to_string(),
            key: "nothing/here.pem".to_string(),
            staged: None,
            code: CA_BUNDLE_NOT_FOUND,
            message: CA_CERTIFICATES_BUNDLE_NOT_FOUND_MESSAGE,
        },
        Unreadable {
            case: "a bucket name Amazon S3 holds no bucket under at all",
            bucket: "Not A Bucket Name".to_string(),
            key: "absent.pem".to_string(),
            staged: None,
            code: CA_BUNDLE_NOT_FOUND,
            message: CA_CERTIFICATES_BUNDLE_NOT_FOUND_MESSAGE,
        },
        Unreadable {
            case: "an object holding prose rather than PEM",
            bucket: BUNDLE_BUCKET.to_string(),
            key: "prose.txt".to_string(),
            staged: Some("these are not certificates\n".to_string()),
            code: INVALID_CA_BUNDLE,
            message: INVALID_CA_CERTIFICATES_BUNDLE_MESSAGE,
        },
        Unreadable {
            case: "an object holding PEM that carries no certificate",
            bucket: BUNDLE_BUCKET.to_string(),
            key: "public-key.pem".to_string(),
            staged: Some(
                "-----BEGIN PUBLIC KEY-----\nMTIzNDU2Nzg5MA==\n-----END PUBLIC KEY-----\n"
                    .to_string(),
            ),
            code: INVALID_CA_BUNDLE,
            message: INVALID_CA_CERTIFICATES_BUNDLE_MESSAGE,
        },
        Unreadable {
            case: "an object holding a certificate block with no X.509 structure",
            bucket: BUNDLE_BUCKET.to_string(),
            key: "corrupt.pem".to_string(),
            staged: Some(
                "-----BEGIN CERTIFICATE-----\nMTIzNDU2Nzg5MA==\n-----END CERTIFICATE-----\n"
                    .to_string(),
            ),
            code: INVALID_CA_BUNDLE,
            message: INVALID_CA_CERTIFICATES_BUNDLE_MESSAGE,
        },
        Unreadable {
            case: "an object holding no bytes at all",
            bucket: BUNDLE_BUCKET.to_string(),
            key: "empty.pem".to_string(),
            staged: Some(String::new()),
            code: INVALID_CA_BUNDLE,
            message: INVALID_CA_CERTIFICATES_BUNDLE_MESSAGE,
        },
    ]
}

#[tokio::test]
async fn a_create_whose_bundle_cannot_be_read_is_refused_and_stores_nothing() {
    let emulator = emulator_with_bundles().await;

    for row in unreadable_rows() {
        if let Some(body) = row.staged.as_deref() {
            emulator.stage(&row.key, body).await;
        }
        let refused =
            create_trust_store(&emulator, "ts-unreadable", &row.bucket, &row.key, &[]).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, row.code),
            "case {:?}: expected 400 {}, got {} body {}",
            row.case,
            row.code,
            refused.status,
            refused.body
        );
        assert_eq!(
            message(&refused),
            row.message,
            "case {:?}: expected the modelled message, got {}",
            row.case,
            refused.body
        );

        // Nothing was stored, so the account still holds no trust store and the name the
        // refused create asked for is still free.
        let all = describe_trust_stores(&emulator, &[]).await;
        assert_eq!(
            members(&all.body, "TrustStores").len(),
            0,
            "case {:?}: a refused create must store nothing, got {}",
            row.case,
            all.body
        );
    }

    // The name every refusal above asked for is free, which is what proves none of them
    // took it.
    emulator.stage("after.pem", &ca_bundle(1)).await;
    let created =
        create_trust_store(&emulator, "ts-unreadable", BUNDLE_BUCKET, "after.pem", &[]).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected 200, got {} body {}",
        "the name a refused create asked for is still free", created.status, created.body
    );
}

// ---------------------------------------------------------------------------
// RULE 3: one name per Region
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_name_a_trust_store_already_holds_is_refused() {
    let emulator = emulator_with_bundles().await;
    emulator.stage("first.pem", &ca_bundle(2)).await;
    emulator.stage("second.pem", &ca_bundle(4)).await;
    let created =
        create_trust_store(&emulator, "ts-unique", BUNDLE_BUCKET, "first.pem", &[]).await;
    let arn = first_text(&only_trust_store(&created, "the first create"), "TrustStoreArn")
        .unwrap_or_else(|| panic!("no TrustStoreArn in {}", created.body));

    // The rule is about the NAME, so a repeat naming another bundle is refused too.
    for (case, key) in [
        ("the same name and the same bundle", "first.pem"),
        ("the same name and another bundle", "second.pem"),
    ] {
        let refused = create_trust_store(&emulator, "ts-unique", BUNDLE_BUCKET, key, &[]).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, DUPLICATE_TRUST_STORE_NAME),
            "case {case:?}: expected 400 {DUPLICATE_TRUST_STORE_NAME}, got {} body {}",
            refused.status,
            refused.body
        );
        assert!(
            message(&refused).contains("ts-unique"),
            "case {case:?}: expected the name in the message, got {}",
            refused.body
        );
    }

    // The trust store holding the name is untouched: same ARN, same certificate count.
    let still = only_trust_store(
        &describe_trust_stores(&emulator, &[p("Names.member.1", "ts-unique")]).await,
        "the trust store holding the name",
    );
    assert_eq!(
        (
            first_text(&still, "TrustStoreArn"),
            first_text(&still, "NumberOfCaCertificates")
        ),
        (Some(arn), Some("2".to_string())),
        "case {:?}: a refused repeat must change nothing, got {still}",
        "the trust store holding the name"
    );

    // A `Tags` list naming one key twice says nothing about that key's value, so it is
    // refused for the tags rather than stored.
    let refused = create_trust_store(
        &emulator,
        "ts-tag-keys",
        BUNDLE_BUCKET,
        "first.pem",
        &[
            p("Tags.member.1.Key", "team"),
            p("Tags.member.1.Value", "platform"),
            p("Tags.member.2.Key", "team"),
            p("Tags.member.2.Value", "network"),
        ],
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, DUPLICATE_TAG_KEYS),
        "case {:?}: expected 400 {DUPLICATE_TAG_KEYS}, got {} body {}",
        "one tag key with two values",
        refused.status,
        refused.body
    );
}

// ---------------------------------------------------------------------------
// RULE 4: a modification rotates the bundle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_modification_rotates_the_bundle_and_keeps_the_trust_store() {
    let emulator = emulator_with_bundles().await;
    emulator.stage("rotate-1.pem", &ca_bundle(1)).await;
    let created =
        create_trust_store(&emulator, "ts-rotate", BUNDLE_BUCKET, "rotate-1.pem", &[]).await;
    let arn = first_text(&only_trust_store(&created, "the create"), "TrustStoreArn")
        .unwrap_or_else(|| panic!("no TrustStoreArn in {}", created.body));

    // The rule generalises over the SIZE of the new bundle, in both directions, and over
    // repeating a rotation.
    for (case, key, certificates) in [
        ("one certificate to three", "rotate-3.pem", 3),
        ("three certificates back to one", "rotate-1.pem", 1),
        ("one certificate to seven", "rotate-7.pem", 7),
    ] {
        emulator.stage(key, &ca_bundle(certificates)).await;
        let modified = modify_trust_store(&emulator, &arn, BUNDLE_BUCKET, key).await;
        assert_eq!(
            modified.status, 200,
            "case {case:?}: expected 200 from ModifyTrustStore, got {} body {}",
            modified.status, modified.body
        );
        let answered = only_trust_store(&modified, case);
        assert_eq!(
            (
                first_text(&answered, "TrustStoreArn"),
                first_text(&answered, "Name"),
                first_text(&answered, "NumberOfCaCertificates"),
            ),
            (
                Some(arn.clone()),
                Some("ts-rotate".to_string()),
                Some(certificates.to_string()),
            ),
            "case {case:?}: expected the same trust store carrying {certificates} \
             certificates, got {answered}"
        );
        // Read back: the count is the stored one, not just what the write answered.
        let read = only_trust_store(
            &describe_trust_stores(&emulator, &[p("TrustStoreArns.member.1", &arn)]).await,
            case,
        );
        assert_eq!(
            first_text(&read, "NumberOfCaCertificates"),
            Some(certificates.to_string()),
            "case {case:?}: expected the stored count to be the new bundle's, got {read}"
        );
    }

    // A bundle the modification cannot read refuses it and leaves the trust store on the
    // bundle it had.
    for row in unreadable_rows() {
        if let Some(body) = row.staged.as_deref() {
            emulator.stage(&row.key, body).await;
        }
        let refused = modify_trust_store(&emulator, &arn, &row.bucket, &row.key).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, row.code),
            "case {:?} (ModifyTrustStore): expected 400 {}, got {} body {}",
            row.case,
            row.code,
            refused.status,
            refused.body
        );
        let read = only_trust_store(
            &describe_trust_stores(&emulator, &[p("TrustStoreArns.member.1", &arn)]).await,
            row.case,
        );
        assert_eq!(
            first_text(&read, "NumberOfCaCertificates"),
            Some("7".to_string()),
            "case {:?}: a refused rotation must change nothing, got {read}",
            row.case
        );
    }

    // A trust store this account holds none of is the modelled refusal, naming the ARN.
    let absent = absent_trust_store_arn("rotate-absent");
    let refused = modify_trust_store(&emulator, &absent, BUNDLE_BUCKET, "rotate-1.pem").await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, TRUST_STORE_NOT_FOUND),
        "case {:?}: expected 400 {TRUST_STORE_NOT_FOUND}, got {} body {}",
        "a trust store that does not exist",
        refused.status,
        refused.body
    );
    assert!(
        message(&refused).contains(&absent),
        "case {:?}: expected the ARN in the message, got {}",
        "a trust store that does not exist",
        refused.body
    );
}

// ---------------------------------------------------------------------------
// RULE 5: the associations a trust store has, and what they block
// ---------------------------------------------------------------------------

/// Create an HTTPS listener on `load_balancer` that authenticates client certificates
/// against `trust_store`, and answer its ARN.
async fn create_mtls_listener(
    emulator: &Emulator,
    load_balancer: &str,
    trust_store: &str,
    port: i32,
) -> String {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", load_balancer));
    form.push(p("Protocol", "HTTPS"));
    form.push(p("Port", &port.to_string()));
    form.push(p("Certificates.member.1.CertificateArn", CERTIFICATE));
    form.push(p("MutualAuthentication.Mode", "verify"));
    form.push(p("MutualAuthentication.TrustStoreArn", trust_store));
    form.push(p("DefaultActions.member.1.Type", "fixed-response"));
    form.push(p(
        "DefaultActions.member.1.FixedResponseConfig.StatusCode",
        "200",
    ));
    let created = call(&emulator.router, &form).await;
    assert_eq!(
        created.status, 200,
        "case create mTLS listener on port {port}: expected 200, got {} body {}",
        created.status, created.body
    );
    first_text(&created.body, "ListenerArn")
        .unwrap_or_else(|| panic!("no ListenerArn in {}", created.body))
}

#[tokio::test]
async fn the_associations_of_a_trust_store_are_the_listeners_that_authenticate_against_it() {
    let emulator = emulator_with_bundles().await;
    emulator.stage("associated.pem", &ca_bundle(1)).await;
    let created = create_trust_store(
        &emulator,
        "ts-associated",
        BUNDLE_BUCKET,
        "associated.pem",
        &[],
    )
    .await;
    let arn = first_text(&only_trust_store(&created, "the create"), "TrustStoreArn")
        .unwrap_or_else(|| panic!("no TrustStoreArn in {}", created.body));

    // A trust store nothing authenticates against is an empty collection, not a refusal.
    let none = describe_associations(&emulator, &arn, &[]).await;
    assert_eq!(
        (none.status, reported_associations(&none)),
        (200, Vec::new()),
        "case {:?}: expected 200 and no association, got {} body {}",
        "a trust store nothing is associated with",
        none.status,
        none.body
    );

    // The rule generalises over the NUMBER of associated resources, so listeners are added
    // one at a time and every count is asserted.
    let load_balancer = create_load_balancer(&emulator.router, "lb-mtls").await;
    let mut expected: Vec<String> = Vec::new();
    for (index, port) in [40_001, 40_002, 40_003].into_iter().enumerate() {
        expected.push(create_mtls_listener(&emulator, &load_balancer, &arn, port).await);
        expected.sort();
        let case = format!("{} associated listener(s)", index + 1);
        let reported = describe_associations(&emulator, &arn, &[]).await;
        assert_eq!(
            (reported.status, reported_associations(&reported)),
            (200, expected.clone()),
            "case {case:?}: expected the associated listeners, got {}",
            reported.body
        );

        // Paging is the SAME sequence in any page size.
        for page_size in [1usize, 2, 4] {
            let case = format!("{case} at page size {page_size}");
            let mut walked: Vec<String> = Vec::new();
            let mut marker: Option<String> = None;
            for _ in 0..expected.len() + 2 {
                let mut fields = vec![p("PageSize", &page_size.to_string())];
                if let Some(marker) = marker.as_deref() {
                    fields.push(p("Marker", marker));
                }
                let page = describe_associations(&emulator, &arn, &fields).await;
                assert_eq!(
                    page.status, 200,
                    "case {case:?}: expected 200, got {} body {}",
                    page.status, page.body
                );
                let reported = reported_associations(&page);
                assert!(
                    reported.len() <= page_size,
                    "case {case:?}: a page holds at most {page_size}, got {reported:?}"
                );
                walked.extend(reported);
                marker = page.text("NextMarker");
                if marker.is_none() {
                    break;
                }
            }
            assert_eq!(
                (walked, marker),
                (expected.clone(), None),
                "case {case:?}: expected the unpaged order and no marker at the end"
            );
        }

        // While a resource is associated with it, the trust store is not released.
        let refused = delete_trust_store(&emulator, &arn).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, TRUST_STORE_IN_USE),
            "case {case:?}: expected 400 {TRUST_STORE_IN_USE}, got {} body {}",
            refused.status,
            refused.body
        );
    }

    // A marker this collection did not issue names no position in it.
    for (case, marker) in [
        ("a marker that is not a number", "not-a-marker"),
        ("a marker past the end", "500"),
    ] {
        let refused = describe_associations(&emulator, &arn, &[p("Marker", marker)]).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} body {}",
            refused.status,
            refused.body
        );
    }

    // Taking the listeners away takes the associations with them, and then the trust store
    // is released.
    for listener in &expected {
        let mut form = action("DeleteListener");
        form.push(p("ListenerArn", listener));
        let deleted = call(&emulator.router, &form).await;
        assert_eq!(
            deleted.status, 200,
            "case delete listener: expected 200, got {} body {}",
            deleted.status, deleted.body
        );
    }
    let emptied = describe_associations(&emulator, &arn, &[]).await;
    assert_eq!(
        reported_associations(&emptied),
        Vec::<String>::new(),
        "case {:?}: expected no association left, got {}",
        "every associated listener deleted",
        emptied.body
    );
    let deleted = delete_trust_store(&emulator, &arn).await;
    assert_eq!(
        deleted.status, 200,
        "case {:?}: expected 200 from DeleteTrustStore, got {} body {}",
        "the last association gone", deleted.status, deleted.body
    );

    // Released: the ARN and the name resolve to nothing, and a repeat of the delete says
    // so rather than answering success twice.
    for (case, fields) in [
        (
            "the released ARN",
            vec![p("TrustStoreArns.member.1", &arn)],
        ),
        (
            "the released name",
            vec![p("Names.member.1", "ts-associated")],
        ),
    ] {
        let refused = describe_trust_stores(&emulator, &fields).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, TRUST_STORE_NOT_FOUND),
            "case {case:?}: expected 400 {TRUST_STORE_NOT_FOUND}, got {} body {}",
            refused.status,
            refused.body
        );
    }
    let again = delete_trust_store(&emulator, &arn).await;
    assert_eq!(
        (again.status, again.error_code().as_str()),
        (400, TRUST_STORE_NOT_FOUND),
        "case {:?}: expected 400 {TRUST_STORE_NOT_FOUND}, got {} body {}",
        "a repeated delete",
        again.status,
        again.body
    );
    // The name is free again, which is what makes the release a real one.
    emulator.stage("reused.pem", &ca_bundle(1)).await;
    let reused = create_trust_store(
        &emulator,
        "ts-associated",
        BUNDLE_BUCKET,
        "reused.pem",
        &[],
    )
    .await;
    assert_eq!(
        reused.status, 200,
        "case {:?}: expected 200, got {} body {}",
        "the name of a released trust store", reused.status, reused.body
    );
}

// ---------------------------------------------------------------------------
// RULES 6 and 7: every ARN-addressing call refuses an ARN this account holds none under,
// and a shared association is one with another account's resource
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_trust_store_call_refuses_an_identifier_this_account_holds_none_under() {
    let emulator = emulator_with_bundles().await;
    emulator.stage("held.pem", &ca_bundle(1)).await;
    let created = create_trust_store(&emulator, "ts-held", BUNDLE_BUCKET, "held.pem", &[]).await;
    let held = first_text(&only_trust_store(&created, "the create"), "TrustStoreArn")
        .unwrap_or_else(|| panic!("no TrustStoreArn in {}", created.body));

    // The rule is about the IDENTIFIER, so the rows span the operations that address a
    // trust store by one and the forms an identifier can take.
    let absent = absent_trust_store_arn("absent");
    let other_service = format!("arn:aws:s3:::bucket-{}", account());
    // Each row is the case, the form fields the call carries, and the identifier its
    // refusal must name.
    type Addressed = (&'static str, Vec<(String, String)>, String);
    let rows: Vec<Addressed> = vec![
        (
            "ModifyTrustStore",
            vec![
                p("Action", "ModifyTrustStore"),
                p("TrustStoreArn", &absent),
                p("CaCertificatesBundleS3Bucket", BUNDLE_BUCKET),
                p("CaCertificatesBundleS3Key", "held.pem"),
            ],
            absent.clone(),
        ),
        (
            "DeleteTrustStore",
            vec![
                p("Action", "DeleteTrustStore"),
                p("TrustStoreArn", &absent),
            ],
            absent.clone(),
        ),
        (
            "DescribeTrustStoreAssociations",
            vec![
                p("Action", "DescribeTrustStoreAssociations"),
                p("TrustStoreArn", &absent),
            ],
            absent.clone(),
        ),
        (
            "DeleteSharedTrustStoreAssociation",
            vec![
                p("Action", "DeleteSharedTrustStoreAssociation"),
                p("TrustStoreArn", &absent),
                p("ResourceArn", &held),
            ],
            absent.clone(),
        ),
        (
            "DescribeTrustStores by ARN",
            vec![
                p("Action", "DescribeTrustStores"),
                p("TrustStoreArns.member.1", &absent),
            ],
            absent.clone(),
        ),
        (
            "DescribeTrustStores by an ARN of another service",
            vec![
                p("Action", "DescribeTrustStores"),
                p("TrustStoreArns.member.1", &other_service),
            ],
            other_service.clone(),
        ),
        (
            "DeleteTrustStore on a value that is no ARN at all",
            vec![
                p("Action", "DeleteTrustStore"),
                p("TrustStoreArn", "not-an-arn"),
            ],
            "not-an-arn".to_string(),
        ),
    ];
    for (case, fields, named) in rows {
        let mut form = vec![p("Version", crate::provisioning_loop::VERSION)];
        form.extend(fields);
        let refused = call(&emulator.router, &form).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, TRUST_STORE_NOT_FOUND),
            "case {case:?}: expected 400 {TRUST_STORE_NOT_FOUND}, got {} body {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            message(&refused),
            format!("Trust store ARN '{named}' not found"),
            "case {case:?}: expected the refusal to name the ARN, got {}",
            refused.body
        );
    }

    // A read by NAME names the NAME, which is the other identifier a caller corrects.
    let by_name = describe_trust_stores(&emulator, &[p("Names.member.1", "no-such-store")]).await;
    assert_eq!(
        (
            by_name.status,
            by_name.error_code().as_str(),
            message(&by_name)
        ),
        (
            400,
            TRUST_STORE_NOT_FOUND,
            "Trust store name 'no-such-store' not found".to_string()
        ),
        "case {:?}: expected the refusal to name the name, got {}",
        "DescribeTrustStores by name",
        by_name.body
    );

    // The trust store the account DOES hold is still there, so no refusal changed anything.
    let still = describe_trust_stores(&emulator, &[p("TrustStoreArns.member.1", &held)]).await;
    assert_eq!(
        still.status, 200,
        "case {:?}: expected 200, got {} body {}",
        "the held trust store survives every refusal", still.status, still.body
    );
}

#[tokio::test]
async fn a_shared_association_is_one_with_another_accounts_resource() {
    let emulator = emulator_with_bundles().await;
    emulator.stage("shared.pem", &ca_bundle(1)).await;
    let created =
        create_trust_store(&emulator, "ts-shared", BUNDLE_BUCKET, "shared.pem", &[]).await;
    let trust_store = first_text(&only_trust_store(&created, "the create"), "TrustStoreArn")
        .unwrap_or_else(|| panic!("no TrustStoreArn in {}", created.body));
    let load_balancer = create_load_balancer(&emulator.router, "lb-shared").await;
    let listener = create_mtls_listener(&emulator, &load_balancer, &trust_store, 41_001).await;

    // A resource of the trust store's OWN account is refused for being in that account —
    // including the listener that really does authenticate against it, whose association is
    // taken off through `ModifyListener` rather than here.
    for (case, resource) in [
        ("the listener that authenticates against it", listener),
        ("the load balancer that carries that listener", load_balancer),
        ("the trust store itself", trust_store.clone()),
    ] {
        let mut form = action("DeleteSharedTrustStoreAssociation");
        form.push(p("TrustStoreArn", &trust_store));
        form.push(p("ResourceArn", &resource));
        let refused = call(&emulator.router, &form).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, SAME_ACCOUNT),
            "case {case:?}: expected 400 {SAME_ACCOUNT}, got {} body {}",
            refused.status,
            refused.body
        );
        assert!(
            message(&refused).contains(&resource),
            "case {case:?}: expected the resource in the message, got {}",
            refused.body
        );
    }

    // A resource of ANOTHER account: this emulator holds no resource of another account, so
    // there is no such association to delete.
    for (case, resource) in [
        (
            "a listener of another account",
            format!(
                "arn:aws:elasticloadbalancing:{REGION}:210987654321:listener/app/their-lb/\
                 0123456789abcdef/0123456789abcdef"
            ),
        ),
        (
            "a load balancer of another account",
            format!(
                "arn:aws:elasticloadbalancing:{REGION}:210987654321:loadbalancer/app/their-lb/\
                 0123456789abcdef"
            ),
        ),
    ] {
        let mut form = action("DeleteSharedTrustStoreAssociation");
        form.push(p("TrustStoreArn", &trust_store));
        form.push(p("ResourceArn", &resource));
        let refused = call(&emulator.router, &form).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, ASSOCIATION_NOT_FOUND),
            "case {case:?}: expected 400 {ASSOCIATION_NOT_FOUND}, got {} body {}",
            refused.status,
            refused.body
        );
        assert!(
            message(&refused).contains(&resource) && message(&refused).contains(&trust_store),
            "case {case:?}: expected the pair in the message, got {}",
            refused.body
        );
    }
}
