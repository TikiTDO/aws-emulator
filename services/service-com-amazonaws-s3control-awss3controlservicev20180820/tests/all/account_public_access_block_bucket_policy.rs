// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT-level `PublicAccessBlock` as Amazon S3's data plane sees it: S3 Control
//! and Amazon S3 mounted on ONE shared engine, driven over the wire.
//!
//! The rule under test: a Block Public Access setting is in force on a bucket when it is
//! on in the BUCKET's own configuration or in the one the owning ACCOUNT carries — "when
//! Amazon S3 receives a request to access a bucket or an object, it determines whether the
//! account or the bucket has a block public access setting applied … if there is an
//! existing block public access setting that prohibits the requested access, Amazon S3
//! rejects the request" (Amazon S3 User Guide, *Blocking public access to your Amazon S3
//! storage*). For `BlockPublicPolicy` that means `PutBucketPolicy` of a document that
//! grants public access is refused `AccessDenied` (403) while the setting is on at either
//! level, and nothing is stored.
//!
//! The table below spans that class in both dimensions the rule generalises over: WHICH
//! account-level configuration is in force (none at all, one that blocks nothing, one that
//! blocks only public policies, one that blocks everything else, all four settings on) and
//! WHICH document is being written (four forms that grant to every principal and three that
//! do not). Each row is a bucket of its own, with its own configuration deleted first, so
//! what refuses the write can only be the account-level record.
//!
//! The evaluation is LIVE, not a flag the operation remembers: the suite below also walks
//! the whole sequence — accepted with no account configuration, refused once it is put,
//! accepted again once it is deleted — and pins that a refusal leaves the policy the bucket
//! already carried exactly as it was.

use crate::access_point_lifecycle::{xml_unescape, ACCOUNT, NO_CONTENT, REGION, XMLNS};
use crate::object_store::{elements, ObjectStore};

/// The status Amazon S3 answers a successful `PutBucketPolicy` with
/// ([`::service_com_amazonaws_s3_amazons3::ops::put_bucket_policy::PUT_BUCKET_POLICY_SUCCESS_STATUS`]),
/// read from that service rather than spelled as a number here.
const POLICY_PUT_ACCEPTED: u16 =
    ::service_com_amazonaws_s3_amazons3::ops::put_bucket_policy::PUT_BUCKET_POLICY_SUCCESS_STATUS;

/// The answer Amazon S3 gives a call its Block Public Access configuration denies.
const BLOCKED_STATUS: u16 = 403;
/// See [`BLOCKED_STATUS`].
const BLOCKED_CODE: &str = "AccessDenied";

/// The answer `GetBucketPolicy` gives for a bucket carrying no policy.
const NO_POLICY_STATUS: u16 = 404;
/// See [`NO_POLICY_STATUS`].
const NO_POLICY_CODE: &str = "NoSuchBucketPolicy";

/// The four members of `PublicAccessBlockConfiguration`, in the order the shape declares
/// them. SOURCE: the model's `com.amazonaws.s3control#PublicAccessBlockConfiguration` —
/// each member targets `Setting` (boolean) and carries `@default: false`.
const SETTINGS: [&str; 4] = [
    "BlockPublicAcls",
    "IgnorePublicAcls",
    "BlockPublicPolicy",
    "RestrictPublicBuckets",
];

/// Which of [`SETTINGS`] is the one this suite is about.
const BLOCK_PUBLIC_POLICY: usize = 2;

/// The account-level configuration URI
/// (`smithy_query(query="operation",operation="PutPublicAccessBlock")`).
const ACCOUNT_CONFIGURATION_URI: &str = "/v20180820/configuration/publicAccessBlock";

/// One document a client may write as a bucket policy, and whether it grants access to
/// everyone — the classification `GetBucketPolicyStatus` reports as `IsPublic`, and the one
/// `BlockPublicPolicy` refuses a write of.
struct Document {
    case: &'static str,
    body: String,
    is_public: bool,
}

/// The documents every row below is run over: four that reach every principal and three
/// that do not, differing in the form the principal takes, in the action and resource they
/// name, and in the condition that holds them back.
///
/// SOURCE for which of them is public: Amazon S3 User Guide, *The Meaning of "Public"* — a
/// statement that allows and reaches every principal is public unless a `Condition`
/// restricts it to fixed values of one of the keys that name identities or sources, and
/// `aws:SourceIp` over the whole internet restricts nothing.
fn documents(bucket: &str) -> Vec<Document> {
    let objects = format!("arn:aws:s3:::{bucket}/*");
    let itself = format!("arn:aws:s3:::{bucket}");
    vec![
        Document {
            case: "Allow to the wildcard principal on the bucket's objects",
            body: statement(&format!(
                "\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"s3:GetObject\",\
                 \"Resource\":\"{objects}\""
            )),
            is_public: true,
        },
        Document {
            case: "Allow to a principal map whose AWS value is the wildcard",
            body: statement(&format!(
                "\"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"*\"}},\
                 \"Action\":[\"s3:GetObject\",\"s3:GetObjectVersion\"],\"Resource\":\"{objects}\""
            )),
            is_public: true,
        },
        Document {
            case: "Allow to every principal on the bucket itself",
            body: statement(&format!(
                "\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"s3:ListBucket\",\
                 \"Resource\":\"{itself}\""
            )),
            is_public: true,
        },
        Document {
            case: "Allow to the wildcard principal from the whole internet",
            body: statement(&format!(
                "\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"s3:GetObject\",\
                 \"Resource\":\"{objects}\",\
                 \"Condition\":{{\"IpAddress\":{{\"aws:SourceIp\":\"0.0.0.0/0\"}}}}"
            )),
            is_public: true,
        },
        Document {
            case: "Allow to the account's own root principal",
            body: statement(&format!(
                "\"Effect\":\"Allow\",\"Principal\":{{\"AWS\":\"arn:aws:iam::{ACCOUNT}:root\"}},\
                 \"Action\":\"s3:GetObject\",\"Resource\":\"{objects}\""
            )),
            is_public: false,
        },
        Document {
            case: "Allow to the wildcard principal, restricted to one organization",
            body: statement(&format!(
                "\"Effect\":\"Allow\",\"Principal\":\"*\",\"Action\":\"s3:GetObject\",\
                 \"Resource\":\"{objects}\",\
                 \"Condition\":{{\"StringEquals\":{{\"aws:PrincipalOrgID\":\"o-abcde12345\"}}}}"
            )),
            is_public: false,
        },
        Document {
            case: "Deny to the wildcard principal",
            body: statement(&format!(
                "\"Effect\":\"Deny\",\"Principal\":\"*\",\"Action\":\"s3:GetObject\",\
                 \"Resource\":\"{objects}\""
            )),
            is_public: false,
        },
    ]
}

/// One policy document carrying exactly one statement, written as `members`.
fn statement(members: &str) -> String {
    format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{{{members}}}]}}")
}

/// One account-level configuration a row is run under: the four settings, or `None` for an
/// account carrying no configuration at all.
struct AccountConfiguration {
    case: &'static str,
    settings: Option<[bool; 4]>,
}

/// The configurations every document above is written under. They differ in whether a
/// configuration exists at all, in whether `BlockPublicPolicy` is among the settings that
/// are on, and in how many of the others are — so a row cannot pass by treating "a
/// configuration exists" or "some setting is on" as the block.
fn account_configurations() -> Vec<AccountConfiguration> {
    vec![
        AccountConfiguration {
            case: "no account-level configuration at all",
            settings: None,
        },
        AccountConfiguration {
            case: "a configuration whose four settings are all false",
            settings: Some([false; 4]),
        },
        AccountConfiguration {
            case: "only BlockPublicPolicy on",
            settings: Some([false, false, true, false]),
        },
        AccountConfiguration {
            case: "every setting but BlockPublicPolicy on",
            settings: Some([true, true, false, true]),
        },
        AccountConfiguration {
            case: "all four settings on",
            settings: Some([true; 4]),
        },
    ]
}

/// The message Amazon S3 words this refusal in: who was refused, the action, the resource
/// and the setting that prevented it.
///
/// The fixed text is the service's; the principal is the one the request signed for and the
/// resource is the bucket it addressed, both read from the request rather than from a
/// recording.
fn blocked_message(bucket: &str) -> String {
    format!(
        "User: {} is not authorized to perform: s3:PutBucketPolicy on resource: \
         \"arn:aws:s3:::{bucket}\" because public policies are prevented by the \
         BlockPublicPolicy setting in S3 Block Public Access.",
        ::service_common::caller_principal::ambient_user_arn()
    )
}

/// The text of the first `<element>` of `body`, unescaped as XML, or a note naming what was
/// answered.
fn element(body: &[u8], name: &str) -> String {
    let document = String::from_utf8_lossy(body).into_owned();
    elements(&document, name)
        .into_iter()
        .next()
        .map(|text| xml_unescape(&text))
        .unwrap_or_else(|| format!("<no {name} element> in {document}"))
}

impl ObjectStore {
    /// Put the account-level configuration `settings`, or remove it altogether for `None`,
    /// through S3 Control's own operations — the API a client configures it with.
    async fn set_account_configuration(&self, case: &str, settings: Option<[bool; 4]>) {
        let reply = match settings {
            Some(settings) => {
                let members: String = SETTINGS
                    .iter()
                    .zip(settings)
                    .map(|(member, value)| format!("<{member}>{value}</{member}>"))
                    .collect();
                self.s3control(
                    REGION,
                    "PUT",
                    ACCOUNT_CONFIGURATION_URI,
                    &format!(
                        "<PublicAccessBlockConfiguration xmlns=\"{XMLNS}\">{members}\
                         </PublicAccessBlockConfiguration>"
                    ),
                )
                .await
            }
            None => {
                self.s3control(REGION, "DELETE", ACCOUNT_CONFIGURATION_URI, "")
                    .await
            }
        };
        assert_eq!(
            reply.status, NO_CONTENT,
            "case {case:?}: the account-level configuration must be written first, got {} with \
             body {}",
            reply.status, reply.body
        );
    }

    /// The four settings `GetPublicAccessBlock` reports for the account, as the text of each
    /// element in [`SETTINGS`] order, or `None` when the account carries no configuration.
    async fn account_configuration(&self) -> Option<Vec<String>> {
        let reply = self
            .s3control(REGION, "GET", ACCOUNT_CONFIGURATION_URI, "")
            .await;
        if reply.status != 200 {
            return None;
        }
        Some(
            SETTINGS
                .iter()
                .map(|member| reply.text(member).unwrap_or_default())
                .collect(),
        )
    }

    /// A bucket of this suite's own, created in [`REGION`] with its OWN Block Public Access
    /// configuration deleted — so every refusal below can only come from the account level.
    async fn bucket_without_its_own_configuration(&self, case: &str, bucket: &str) {
        self.create_bucket(case, REGION, bucket).await;
        let (status, body) = self
            .s3(REGION, "DELETE", &format!("/{bucket}?publicAccessBlock"), b"")
            .await;
        assert_eq!(
            status,
            204,
            "case {case:?}: the bucket's own configuration must be deleted first, got {status} \
             with body {:?}",
            String::from_utf8_lossy(&body)
        );
    }

    /// `PutBucketPolicy` of `document` on `bucket`: the status and the body answered.
    async fn put_bucket_policy(&self, bucket: &str, document: &str) -> (u16, Vec<u8>) {
        self.s3(
            REGION,
            "PUT",
            &format!("/{bucket}?policy"),
            document.as_bytes(),
        )
        .await
    }

    /// `GetBucketPolicy` of `bucket`: the status and the body answered.
    async fn bucket_policy(&self, bucket: &str) -> (u16, Vec<u8>) {
        self.s3(REGION, "GET", &format!("/{bucket}?policy"), b"").await
    }

    /// The `IsPublic` classification `GetBucketPolicyStatus` reports for `bucket`.
    async fn policy_status(&self, case: &str, bucket: &str) -> String {
        let (status, body) = self
            .s3(REGION, "GET", &format!("/{bucket}?policyStatus"), b"")
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the policy status of {bucket} must be reported, got {status} with \
             body {:?}",
            String::from_utf8_lossy(&body)
        );
        element(&body, "IsPublic")
    }

    /// `DeleteBucketPolicy` on `bucket`, so the next row starts from a bucket carrying none.
    async fn delete_bucket_policy(&self, case: &str, bucket: &str) {
        let (status, body) = self
            .s3(REGION, "DELETE", &format!("/{bucket}?policy"), b"")
            .await;
        assert_eq!(
            status,
            204,
            "case {case:?}: the policy of {bucket} must be removed, got {status} with body {:?}",
            String::from_utf8_lossy(&body)
        );
    }
}

#[tokio::test]
async fn an_account_level_block_refuses_exactly_the_public_documents() {
    let store = ObjectStore::start();

    for (configuration_index, configuration) in account_configurations().into_iter().enumerate() {
        store
            .set_account_configuration(configuration.case, configuration.settings)
            .await;
        // The configuration the service really holds is what the rows below are judged
        // against, so it is read back through its own operation first.
        let reported = store.account_configuration().await;
        let expected = configuration
            .settings
            .map(|settings| settings.iter().map(bool::to_string).collect::<Vec<_>>());
        assert_eq!(
            reported, expected,
            "case {:?}: expected GetPublicAccessBlock to report the configuration just put",
            configuration.case
        );

        let blocks_public_policy = configuration
            .settings
            .is_some_and(|settings| settings[BLOCK_PUBLIC_POLICY]);
        for (document_index, document) in documents("placeholder").into_iter().enumerate() {
            let bucket = format!("bpp-{configuration_index}-{document_index}-bucket");
            let case = format!(
                "{} with {}, on {bucket}",
                configuration.case, document.case
            );
            // The document names the bucket it is written on, so it is rebuilt for this
            // row's bucket rather than for the placeholder above.
            let body = documents(&bucket)
                .into_iter()
                .nth(document_index)
                .expect("the same row")
                .body;
            store
                .bucket_without_its_own_configuration(&case, &bucket)
                .await;

            let (status, answered) = store.put_bucket_policy(&bucket, &body).await;
            if blocks_public_policy && document.is_public {
                assert_eq!(
                    (status, element(&answered, "Code")),
                    (BLOCKED_STATUS, BLOCKED_CODE.to_string()),
                    "case {case:?}: expected the public document to be refused \
                     {BLOCKED_STATUS} {BLOCKED_CODE}, got {status} with body {:?}",
                    String::from_utf8_lossy(&answered)
                );
                assert_eq!(
                    element(&answered, "Message"),
                    blocked_message(&bucket),
                    "case {case:?}: expected the guardrail's own message, got body {:?}",
                    String::from_utf8_lossy(&answered)
                );
                // Nothing was stored: the bucket carries no policy at all.
                let (read, read_body) = store.bucket_policy(&bucket).await;
                assert_eq!(
                    (read, element(&read_body, "Code")),
                    (NO_POLICY_STATUS, NO_POLICY_CODE.to_string()),
                    "case {case:?}: expected the refused put to store nothing, got {read} with \
                     body {:?}",
                    String::from_utf8_lossy(&read_body)
                );
                continue;
            }

            assert_eq!(
                status,
                POLICY_PUT_ACCEPTED,
                "case {case:?}: expected the write to be accepted {POLICY_PUT_ACCEPTED}, got \
                 {status} with body {:?}",
                String::from_utf8_lossy(&answered)
            );
            let (read, read_body) = store.bucket_policy(&bucket).await;
            assert_eq!(
                (read, String::from_utf8_lossy(&read_body).into_owned()),
                (200, body.clone()),
                "case {case:?}: expected the stored document to be reported byte for byte"
            );
            assert_eq!(
                store.policy_status(&case, &bucket).await,
                document.is_public.to_string(),
                "case {case:?}: expected IsPublic {} for the stored document",
                document.is_public
            );
            store.delete_bucket_policy(&case, &bucket).await;
        }
        // Each configuration row leaves the account with none, so the next row's
        // configuration is the only one in force.
        store.set_account_configuration(configuration.case, None).await;
    }
}

#[tokio::test]
async fn the_account_level_block_is_evaluated_on_every_write_and_released_by_its_delete() {
    let store = ObjectStore::start();
    let bucket = "bpp-live-evaluation-bucket";
    let case = "the whole sequence";
    store
        .bucket_without_its_own_configuration(case, bucket)
        .await;
    let public = documents(bucket)
        .into_iter()
        .next()
        .expect("the first row is a public document")
        .body;
    let scoped = documents(bucket)
        .into_iter()
        .find(|document| !document.is_public)
        .expect("a document that reaches no public principal")
        .body;

    // With no account-level configuration the public document is accepted, and classified
    // public.
    assert_eq!(
        store.put_bucket_policy(bucket, &public).await.0,
        POLICY_PUT_ACCEPTED,
        "case {case:?}: expected the public document to be accepted with no account \
         configuration"
    );
    assert_eq!(
        store.policy_status(case, bucket).await,
        "true",
        "case {case:?}: expected the accepted public document to be classified public"
    );

    // A document that reaches no public principal stays acceptable under the block, and is
    // what the bucket carries when the block goes on.
    store.delete_bucket_policy(case, bucket).await;
    assert_eq!(
        store.put_bucket_policy(bucket, &scoped).await.0,
        POLICY_PUT_ACCEPTED,
        "case {case:?}: expected the scoped document to be accepted"
    );
    store
        .set_account_configuration(case, Some([true; 4]))
        .await;
    assert_eq!(
        store
            .account_configuration()
            .await
            .as_ref()
            .map(|reported| reported[BLOCK_PUBLIC_POLICY].clone()),
        Some("true".to_string()),
        "case {case:?}: expected the account to report BlockPublicPolicy on"
    );
    assert_eq!(
        store.put_bucket_policy(bucket, &public).await.0,
        BLOCKED_STATUS,
        "case {case:?}: expected the identical public document to be refused once the \
         account-level block is in force"
    );
    // The refused write left the document the bucket already carried exactly as it was.
    let (read, read_body) = store.bucket_policy(bucket).await;
    assert_eq!(
        (read, String::from_utf8_lossy(&read_body).into_owned()),
        (200, scoped.clone()),
        "case {case:?}: expected the refused write to leave the stored document untouched"
    );
    assert_eq!(
        store.policy_status(case, bucket).await,
        "false",
        "case {case:?}: expected the stored scoped document to be classified not public"
    );
    // A scoped document is still writable while the block is in force: the guardrail is
    // about the document, not about the operation.
    assert_eq!(
        store.put_bucket_policy(bucket, &scoped).await.0,
        POLICY_PUT_ACCEPTED,
        "case {case:?}: expected a scoped document to be writable under the block"
    );

    // And the block is released the moment the configuration is deleted: the identical
    // public document is acceptable again.
    store.set_account_configuration(case, None).await;
    assert_eq!(
        store.put_bucket_policy(bucket, &public).await.0,
        POLICY_PUT_ACCEPTED,
        "case {case:?}: expected the public document to be accepted again once the \
         account-level configuration is deleted"
    );
    assert_eq!(
        store.policy_status(case, bucket).await,
        "true",
        "case {case:?}: expected the re-accepted document to be classified public"
    );
}

#[tokio::test]
async fn a_bucket_that_keeps_its_own_block_is_refused_with_no_account_configuration() {
    let store = ObjectStore::start();
    // A bucket comes into being with all four settings on, so the bucket-level half of the
    // rule governs on its own — with no account-level configuration anywhere.
    let bucket = "bpp-own-configuration-bucket";
    let case = "the bucket's own configuration, no account configuration";
    store.create_bucket(case, REGION, bucket).await;
    let public = documents(bucket)
        .into_iter()
        .next()
        .expect("the first row is a public document")
        .body;

    let (status, answered) = store.put_bucket_policy(bucket, &public).await;
    assert_eq!(
        (status, element(&answered, "Code")),
        (BLOCKED_STATUS, BLOCKED_CODE.to_string()),
        "case {case:?}: expected the bucket's own block to refuse the public document, got \
         {status} with body {:?}",
        String::from_utf8_lossy(&answered)
    );
    assert_eq!(
        element(&answered, "Message"),
        blocked_message(bucket),
        "case {case:?}: expected the same guardrail message whichever level blocks"
    );
    // And it stays refused when the account's configuration blocks nothing: the levels are
    // joined, not replaced.
    store
        .set_account_configuration(case, Some([false; 4]))
        .await;
    assert_eq!(
        store.put_bucket_policy(bucket, &public).await.0,
        BLOCKED_STATUS,
        "case {case:?}: expected an all-false account configuration not to lift the bucket's \
         own block"
    );
}
