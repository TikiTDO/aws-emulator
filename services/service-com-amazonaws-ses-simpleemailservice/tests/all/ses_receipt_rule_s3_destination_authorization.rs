// SPDX-License-Identifier: CC0-1.0

//! The Amazon S3 DESTINATION of a receipt rule's `S3Action`, authorized before the rule is
//! stored — end to end over the wire, with Amazon S3 and Amazon Web Services KMS mounted
//! beside this service on ONE engine exactly as the assembled router mounts them.
//!
//! `S3Action` documents the requirement: "To enable Amazon SES to write emails to your
//! Amazon S3 bucket, use an Amazon Web Services KMS key to encrypt your emails, or publish
//! to an Amazon SNS topic of another account, Amazon SES must have permission to access
//! those resources." So the rule this suite pins is: a stored `S3Action` must name a
//! destination Amazon SES may really write into — a bucket whose own policy grants the
//! service principal the object write, and (when the action names one) a customer managed
//! key whose key policy grants it the encryption — and a rule that does not is refused
//! `InvalidS3Configuration` (HTTP 400) with NOTHING persisted, the absent-bucket fault
//! worded apart from the unauthorized-destination one.
//!
//! Every bucket, bucket policy and KMS key here is made through Amazon S3's and KMS's OWN
//! operations over the in-process callback, so what the pre-flight reads is what those
//! services really hold. The table spans what the rule generalises over: whether the bucket
//! exists, what its policy says (nothing, a grant to this principal, a grant under one
//! object-key prefix only, a grant to another principal, an explicit deny), whether the
//! action names a key, and which key it names (the Amazon Web Services managed key, a
//! customer managed key that grants the service nothing, one that grants it the encryption,
//! and an identifier KMS holds no key under).

use crate::harness::{call, element, error_envelope, field, Field, REGION};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The rule set every rule below is added to.
const RULE_SET: &str = "inbound-rules";

/// The account this emulator serves, which owns every bucket and key here.
fn account() -> String {
    engine::EMULATOR_ACCOUNT_ID.to_string()
}

/// A router carrying this service plus the two the `S3Action` pre-flight resolves its
/// destination against — Amazon S3 (the bucket and its policy) and KMS (the key and its key
/// policy) — all over ONE shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; the awsQuery calls
    // below sign nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ses_simpleemailservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// An Amazon S3 client over this emulator's own in-process callback.
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// A KMS client over the same callback.
fn kms(router: &MultiServiceRouter) -> aws_sdk_kms::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_kms::Config::builder()
        .behavior_version(aws_sdk_kms::config::BehaviorVersion::latest())
        .region(aws_sdk_kms::config::Region::new(REGION))
        .endpoint_url(format!("https://kms.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_kms::Client::from_conf(config)
}

/// The statement every key policy below carries so KMS accepts it at all: "the key policy
/// must allow the calling principal to make a subsequent PutKeyPolicy request on the KMS
/// key" (`PutKeyPolicyRequest$Policy`), which the account root keeps open.
fn root_statement() -> String {
    format!(
        r#"{{"Sid":"Enable IAM User Permissions","Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{}:root"}},"Action":"kms:*","Resource":"*"}}"#,
        account()
    )
}

/// A key policy naming nobody but the account root: the policy a freshly created customer
/// managed key carries, which grants Amazon SES nothing.
fn root_only_key_policy() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
        root_statement()
    )
}

/// The same policy plus the statement `S3Action$KmsKeyArn` asks for — "ensure that you add a
/// statement to your key's policy to give Amazon SES permission to use it" — granting the
/// service principal the encrypt operations, scoped to this account.
fn key_policy_granting_ses() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{},{{"Sid":"AllowSESEncryption","Effect":"Allow","Principal":{{"Service":"ses.amazonaws.com"}},"Action":["kms:Encrypt","kms:GenerateDataKey*"],"Resource":"*","Condition":{{"StringEquals":{{"AWS:SourceAccount":"{}"}}}}}}]}}"#,
        root_statement(),
        account()
    )
}

/// The same policy granting the service principal ONE of the two encrypt operations only:
/// encrypting a message under a KMS key is generating a data key under it AND encrypting with
/// it, so a policy naming just one of them does not let Amazon SES encrypt the message.
fn key_policy_granting_one_operation() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{},{{"Sid":"AllowSESEncryptOnly","Effect":"Allow","Principal":{{"Service":"ses.amazonaws.com"}},"Action":"kms:Encrypt","Resource":"*"}}]}}"#,
        root_statement()
    )
}

/// A bucket policy granting `principal` the object write on `resource` of `bucket`, scoped to
/// this account — the shape the Amazon SES Developer Guide's inbound-mail bucket policy has.
fn bucket_policy(bucket: &str, principal: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"AllowSESPuts","Effect":"Allow","Principal":{{"Service":"{principal}"}},"Action":"s3:PutObject","Resource":"arn:aws:s3:::{bucket}/{resource}","Condition":{{"StringEquals":{{"AWS:SourceAccount":"{}"}}}}}}]}}"#,
        account()
    )
}

/// The same grant to Amazon SES plus an explicit `Deny` of it: "an explicit deny in any
/// policy overrides any allow".
fn bucket_policy_denying_ses(bucket: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"AllowSESPuts","Effect":"Allow","Principal":{{"Service":"ses.amazonaws.com"}},"Action":"s3:PutObject","Resource":"arn:aws:s3:::{bucket}/*"}},{{"Sid":"DenySESPuts","Effect":"Deny","Principal":{{"Service":"ses.amazonaws.com"}},"Action":"s3:PutObject","Resource":"arn:aws:s3:::{bucket}/*"}}]}}"#
    )
}

/// The grant to Amazon SES under ONE condition clause of the caller's choosing, so a row can
/// state what a clause this evaluation can and cannot decide means.
fn bucket_policy_conditioned(bucket: &str, condition: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"AllowSESPuts","Effect":"Allow","Principal":{{"Service":"ses.amazonaws.com"}},"Action":"s3:PutObject","Resource":"arn:aws:s3:::{bucket}/*","Condition":{condition}}}]}}"#
    )
}

/// What the object store holds for the bucket one row names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BucketSetup {
    /// No bucket at all: the write has nowhere to go.
    Absent,
    /// The bucket exists and carries no policy, so nothing grants Amazon SES the write.
    NoPolicy,
    /// The bucket's policy grants Amazon SES the write on every object.
    GrantsSes,
    /// The grant covers only the objects under `inbound/`.
    GrantsSesUnderPrefix,
    /// The policy grants the write to another service principal.
    GrantsAnotherPrincipal,
    /// The policy grants Amazon SES the write and then denies it.
    DeniesSes,
    /// The grant is scoped by the condition key the Amazon SES Developer Guide's own
    /// inbound-mail bucket policy writes beside the source account and ARN: the account the
    /// write is made for.
    GrantsSesUnderReferer,
    /// The grant is scoped to ANOTHER account, a clause whose key the write does carry a
    /// value for — so the clause is decidably false and the statement is not about this
    /// write.
    GrantsSesForAnotherAccount,
    /// The grant is scoped by a condition key the pre-flight supplies no value for, so its
    /// clause can be decided neither way.
    GrantsSesUnderUndecidableCondition,
}

/// Which key, if any, the row's `S3Action` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeySetup {
    /// No `KmsKeyArn` member at all: "If you do not specify an Amazon Web Services KMS key,
    /// Amazon SES does not encrypt your emails."
    None,
    /// The Amazon Web Services managed key, named as `S3Action$KmsKeyArn` documents it:
    /// "If you use the Amazon Web Services managed key, you don't need to perform any extra
    /// steps to give Amazon SES permission to use the key."
    AwsManaged,
    /// A customer managed key whose policy names only the account root.
    CustomerRootOnly,
    /// A customer managed key whose policy grants Amazon SES the encryption.
    CustomerGrantsSes,
    /// A customer managed key whose policy grants Amazon SES ONE of the two encrypt
    /// operations only.
    CustomerGrantsOneOperation,
    /// A well-formed key ARN KMS holds no key under.
    Unheld,
}

/// One row of the table: the destination, the prefix the action writes under, and whether the
/// rule is stored or the sentence it is refused with.
#[derive(Debug, Clone)]
struct Row {
    /// What this row is called in every assertion it makes.
    case: &'static str,
    /// The rule (and bucket) name this row uses, so no two rows share state.
    name: &'static str,
    bucket: BucketSetup,
    key: KeySetup,
    /// `S3Action$ObjectKeyPrefix`, or `None` when the action leaves it out.
    prefix: Option<&'static str>,
    /// `None` when the rule must be stored; otherwise the refusal's message, with `{bucket}`
    /// standing for the bucket the request named.
    refused_with: Option<&'static str>,
}

/// `CreateReceiptRuleSet`.
async fn create_rule_set(router: &MultiServiceRouter) {
    let (status, body) = call(
        router,
        "CreateReceiptRuleSet",
        &[field("RuleSetName", RULE_SET)],
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the rule set: expected 200, got {status} with {body}"
    );
}

/// The `Rule.*` form fields of a request carrying one `S3Action`.
fn rule_fields(rule_name: &str, bucket: &str, prefix: Option<&str>, key: Option<&str>) -> Vec<Field> {
    let mut params = vec![
        field("Rule.Name", rule_name),
        field("Rule.Enabled", "true"),
        field("Rule.ScanEnabled", "false"),
        field("Rule.Actions.member.1.S3Action.BucketName", bucket),
    ];
    if let Some(prefix) = prefix {
        params.push(field(
            "Rule.Actions.member.1.S3Action.ObjectKeyPrefix",
            prefix,
        ));
    }
    if let Some(key) = key {
        params.push(field("Rule.Actions.member.1.S3Action.KmsKeyArn", key));
    }
    params
}

/// One `CreateReceiptRule` carrying that rule.
async fn create_rule(
    router: &MultiServiceRouter,
    rule_name: &str,
    bucket: &str,
    prefix: Option<&str>,
    key: Option<&str>,
) -> (u16, String) {
    let mut params = vec![field("RuleSetName", RULE_SET)];
    params.extend(rule_fields(rule_name, bucket, prefix, key));
    call(router, "CreateReceiptRule", &params).await
}

/// `DescribeReceiptRule`.
async fn describe_rule(router: &MultiServiceRouter, rule_name: &str) -> (u16, String) {
    call(
        router,
        "DescribeReceiptRule",
        &[field("RuleSetName", RULE_SET), field("RuleName", rule_name)],
    )
    .await
}

/// Put the destination this row names in place, and answer the bucket name its `S3Action`
/// carries together with the `KmsKeyArn` it carries.
async fn stage(router: &MultiServiceRouter, row: &Row) -> (String, Option<String>) {
    let bucket = format!("{}-bucket", row.name);
    if row.bucket != BucketSetup::Absent {
        s3(router)
            .create_bucket()
            .bucket(&bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("case {}: creating {bucket}: {error}", row.case));
    }
    let policy = match row.bucket {
        BucketSetup::Absent | BucketSetup::NoPolicy => None,
        BucketSetup::GrantsSes => Some(bucket_policy(&bucket, "ses.amazonaws.com", "*")),
        BucketSetup::GrantsSesUnderPrefix => {
            Some(bucket_policy(&bucket, "ses.amazonaws.com", "inbound/*"))
        }
        BucketSetup::GrantsAnotherPrincipal => {
            Some(bucket_policy(&bucket, "sns.amazonaws.com", "*"))
        }
        BucketSetup::DeniesSes => Some(bucket_policy_denying_ses(&bucket)),
        BucketSetup::GrantsSesUnderReferer => Some(bucket_policy_conditioned(
            &bucket,
            &format!(r#"{{"StringEquals":{{"aws:Referer":"{}"}}}}"#, account()),
        )),
        BucketSetup::GrantsSesForAnotherAccount => Some(bucket_policy_conditioned(
            &bucket,
            r#"{"StringEquals":{"AWS:SourceAccount":"999999999999"}}"#,
        )),
        BucketSetup::GrantsSesUnderUndecidableCondition => Some(bucket_policy_conditioned(
            &bucket,
            r#"{"Bool":{"aws:SecureTransport":"true"}}"#,
        )),
    };
    if let Some(policy) = policy {
        s3(router)
            .put_bucket_policy()
            .bucket(&bucket)
            .policy(policy)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("case {}: putting {bucket}'s policy: {error}", row.case)
            });
    }
    let key = match row.key {
        KeySetup::None => None,
        KeySetup::AwsManaged => Some(format!(
            "arn:aws:kms:{REGION}:{}:alias/aws/ses",
            account()
        )),
        KeySetup::Unheld => Some(format!(
            "arn:aws:kms:{REGION}:{}:key/9e8d7c6b-5a49-4382-91f0-1234567890ab",
            account()
        )),
        KeySetup::CustomerRootOnly
        | KeySetup::CustomerGrantsSes
        | KeySetup::CustomerGrantsOneOperation => {
            let policy = match row.key {
                KeySetup::CustomerRootOnly => root_only_key_policy(),
                KeySetup::CustomerGrantsOneOperation => key_policy_granting_one_operation(),
                _ => key_policy_granting_ses(),
            };
            let created = kms(router)
                .create_key()
                .policy(policy)
                .send()
                .await
                .unwrap_or_else(|error| panic!("case {}: creating the key: {error}", row.case));
            Some(
                created
                    .key_metadata()
                    .and_then(|metadata| metadata.arn())
                    .unwrap_or_else(|| panic!("case {}: the key has an ARN", row.case))
                    .to_string(),
            )
        }
    };
    (bucket, key)
}

/// A rule is stored only when the destination its `S3Action` names is one Amazon SES may
/// write into, and a rule that is refused leaves the set holding nothing for it.
#[tokio::test]
async fn an_s3_action_is_authorized_against_the_bucket_and_the_key_before_the_rule_is_stored() {
    let rows = [
        Row {
            case: "a bucket whose policy grants the service the write",
            name: "granted",
            bucket: BucketSetup::GrantsSes,
            key: KeySetup::None,
            prefix: None,
            refused_with: None,
        },
        Row {
            case: "a grant under the prefix the action writes under",
            name: "prefix-granted",
            bucket: BucketSetup::GrantsSesUnderPrefix,
            key: KeySetup::None,
            prefix: Some("inbound/"),
            refused_with: None,
        },
        Row {
            case: "a grant under another prefix than the action writes under",
            name: "prefix-elsewhere",
            bucket: BucketSetup::GrantsSesUnderPrefix,
            key: KeySetup::None,
            prefix: Some("archive/"),
            refused_with: Some("Could not write to bucket: {bucket}"),
        },
        Row {
            case: "a bucket the object store does not hold",
            name: "ghost",
            bucket: BucketSetup::Absent,
            key: KeySetup::None,
            prefix: None,
            refused_with: Some("No such bucket: {bucket}"),
        },
        Row {
            case: "a bucket carrying no policy at all",
            name: "unpolicied",
            bucket: BucketSetup::NoPolicy,
            key: KeySetup::None,
            prefix: None,
            refused_with: Some("Could not write to bucket: {bucket}"),
        },
        Row {
            case: "a policy granting the write to another principal",
            name: "granted-elsewhere",
            bucket: BucketSetup::GrantsAnotherPrincipal,
            key: KeySetup::None,
            prefix: None,
            refused_with: Some("Could not write to bucket: {bucket}"),
        },
        Row {
            case: "a policy that denies the service the write",
            name: "denied",
            bucket: BucketSetup::DeniesSes,
            key: KeySetup::None,
            prefix: None,
            refused_with: Some("Could not write to bucket: {bucket}"),
        },
        Row {
            case: "a grant scoped by the account the write is made for",
            name: "referer-granted",
            bucket: BucketSetup::GrantsSesUnderReferer,
            key: KeySetup::None,
            prefix: None,
            refused_with: None,
        },
        Row {
            case: "a grant scoped to another account, which this write is not made for",
            name: "account-elsewhere",
            bucket: BucketSetup::GrantsSesForAnotherAccount,
            key: KeySetup::None,
            prefix: None,
            refused_with: Some("Could not write to bucket: {bucket}"),
        },
        Row {
            // A refusal has to be grounded in what the object store's own policy says, and a
            // clause this evaluation cannot decide says nothing either way: the grant is left
            // undecided rather than read as the bucket withholding the write.
            case: "a grant under a condition key the pre-flight carries no value for",
            name: "undecidable-condition",
            bucket: BucketSetup::GrantsSesUnderUndecidableCondition,
            key: KeySetup::None,
            prefix: None,
            refused_with: None,
        },
        Row {
            case: "the Amazon Web Services managed key, which needs no statement",
            name: "aws-managed-key",
            bucket: BucketSetup::GrantsSes,
            key: KeySetup::AwsManaged,
            prefix: None,
            refused_with: None,
        },
        Row {
            case: "a customer managed key whose policy grants the service nothing",
            name: "key-root-only",
            bucket: BucketSetup::GrantsSes,
            key: KeySetup::CustomerRootOnly,
            prefix: None,
            refused_with: Some("Could not write to bucket: {bucket}"),
        },
        Row {
            case: "a customer managed key whose policy grants the service the encryption",
            name: "key-granted",
            bucket: BucketSetup::GrantsSes,
            key: KeySetup::CustomerGrantsSes,
            prefix: Some("inbound/"),
            refused_with: None,
        },
        Row {
            // Encrypting a message under a KMS key is generating a data key under it and
            // encrypting with it, so a key policy granting one of the two operations only
            // does not let Amazon SES encrypt the message.
            case: "a customer managed key whose policy grants one encrypt operation only",
            name: "key-one-operation",
            bucket: BucketSetup::GrantsSes,
            key: KeySetup::CustomerGrantsOneOperation,
            prefix: None,
            refused_with: Some("Could not write to bucket: {bucket}"),
        },
        Row {
            case: "a key ARN KMS holds no key under",
            name: "key-unheld",
            bucket: BucketSetup::GrantsSes,
            key: KeySetup::Unheld,
            prefix: None,
            refused_with: Some("Could not write to bucket: {bucket}"),
        },
    ];

    let router = emulator();
    create_rule_set(&router).await;

    for row in &rows {
        let (bucket, key) = stage(&router, row).await;
        let (status, body) =
            create_rule(&router, row.name, &bucket, row.prefix, key.as_deref()).await;
        match row.refused_with {
            None => {
                assert_eq!(
                    status, 200,
                    "case {}: expected the rule to be stored (200), got {status} with {body}",
                    row.case
                );
                // Read the stored rule back through the service's own reader: what the rule
                // carries is what the request carried, the key included.
                let (status, described) = describe_rule(&router, row.name).await;
                assert_eq!(
                    status, 200,
                    "case {}: expected the stored rule to be described (200), got {status} \
                     with {described}",
                    row.case
                );
                assert_eq!(
                    element(&described, "BucketName").as_deref(),
                    Some(bucket.as_str()),
                    "case {}: expected the bucket the request named, got {:?}",
                    row.case,
                    element(&described, "BucketName")
                );
                assert_eq!(
                    element(&described, "KmsKeyArn"),
                    key,
                    "case {}: expected the key the request named, got {:?}",
                    row.case,
                    element(&described, "KmsKeyArn")
                );
            }
            Some(message) => {
                let expected = message.replace("{bucket}", &bucket);
                assert_eq!(
                    status, 400,
                    "case {}: expected HTTP 400, got {status} with {body}",
                    row.case
                );
                let (code, refusal) = error_envelope(&body);
                assert_eq!(
                    code.as_deref(),
                    Some("InvalidS3Configuration"),
                    "case {}: expected InvalidS3Configuration, got {code:?} with {body}",
                    row.case
                );
                assert_eq!(
                    refusal.as_deref(),
                    Some(expected.as_str()),
                    "case {}: expected {expected:?}, got {refusal:?}",
                    row.case
                );
                // Nothing is persisted for a refused rule: the set holds no rule of that
                // name at all.
                let (status, described) = describe_rule(&router, row.name).await;
                assert_eq!(
                    status, 400,
                    "case {}: expected the refused rule not to be stored, got {status} with \
                     {described}",
                    row.case
                );
                assert_eq!(
                    error_envelope(&described).0.as_deref(),
                    Some("RuleDoesNotExist"),
                    "case {}: expected RuleDoesNotExist, got {described}",
                    row.case
                );
            }
        }
    }
}

/// The two refusals are told APART: the bucket that is not there and the destination the
/// service may not write into carry different sentences, so a client catching this one error
/// class can tell which fault to go and fix.
#[tokio::test]
async fn the_absent_bucket_and_the_unauthorized_destination_are_worded_apart() {
    let router = emulator();
    create_rule_set(&router).await;
    s3(&router)
        .create_bucket()
        .bucket("told-apart")
        .send()
        .await
        .expect("the bucket is created");

    let (_, absent) = create_rule(&router, "absent-bucket", "no-such-bucket-here", None, None).await;
    let (_, unwritable) = create_rule(&router, "unwritable-bucket", "told-apart", None, None).await;
    let absent = error_envelope(&absent);
    let unwritable = error_envelope(&unwritable);
    assert_eq!(
        absent.0, unwritable.0,
        "case the error class: expected one class for both faults, got {:?} and {:?}",
        absent.0, unwritable.0
    );
    assert_ne!(
        absent.1, unwritable.1,
        "case the sentences: expected the two faults to be worded apart, got {:?} for both",
        absent.1
    );
}

/// EVERY `S3Action` of a rule is authorized, not only the first: a rule whose second action
/// names a destination the service may not write into is refused whole, and nothing of it is
/// stored.
#[tokio::test]
async fn every_action_of_a_rule_is_authorized() {
    let router = emulator();
    create_rule_set(&router).await;
    for bucket in ["first-granted", "second-unpolicied"] {
        s3(&router)
            .create_bucket()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {bucket}: {error}"));
    }
    s3(&router)
        .put_bucket_policy()
        .bucket("first-granted")
        .policy(bucket_policy("first-granted", "ses.amazonaws.com", "*"))
        .send()
        .await
        .expect("the grant is published");

    let params = vec![
        field("RuleSetName", RULE_SET),
        field("Rule.Name", "two-actions"),
        field("Rule.Enabled", "true"),
        field("Rule.ScanEnabled", "false"),
        field(
            "Rule.Actions.member.1.S3Action.BucketName",
            "first-granted",
        ),
        field(
            "Rule.Actions.member.2.S3Action.BucketName",
            "second-unpolicied",
        ),
    ];
    let (status, body) = call(&router, "CreateReceiptRule", &params).await;
    assert_eq!(
        status, 400,
        "case the second action's bucket carries no grant: expected HTTP 400, got {status} \
         with {body}"
    );
    let (code, message) = error_envelope(&body);
    assert_eq!(
        code.as_deref(),
        Some("InvalidS3Configuration"),
        "case the second action: expected InvalidS3Configuration, got {code:?} with {body}"
    );
    assert_eq!(
        message.as_deref(),
        Some("Could not write to bucket: second-unpolicied"),
        "case the second action: expected the SECOND action's bucket to be named, got \
         {message:?}"
    );
    let (status, described) = describe_rule(&router, "two-actions").await;
    assert_eq!(
        status, 400,
        "case the second action: expected nothing to be stored, got {status} with {described}"
    );
}

/// An update is held to the same authorization as a create — `UpdateReceiptRule` models the
/// same `InvalidS3ConfigurationException` — and a refused update leaves the stored rule
/// exactly as it was. — `UpdateReceiptRule` models the
#[tokio::test]
async fn an_update_is_held_to_the_same_destination_authorization() {
    let router = emulator();
    create_rule_set(&router).await;
    for bucket in ["update-granted", "update-unpolicied"] {
        s3(&router)
            .create_bucket()
            .bucket(bucket)
            .send()
            .await
            .unwrap_or_else(|error| panic!("creating {bucket}: {error}"));
    }
    s3(&router)
        .put_bucket_policy()
        .bucket("update-granted")
        .policy(bucket_policy("update-granted", "ses.amazonaws.com", "*"))
        .send()
        .await
        .expect("the grant is published");

    let (status, body) = create_rule(&router, "s3-delivery", "update-granted", None, None).await;
    assert_eq!(
        status, 200,
        "seeding the rule: expected 200, got {status} with {body}"
    );

    // Each row updates the SAME rule to a destination the service may not write into: the
    // bucket that is not there, the bucket carrying no grant, and a key granting nothing.
    let key = kms(&router)
        .create_key()
        .policy(root_only_key_policy())
        .send()
        .await
        .expect("the key is created");
    let key_arn = key
        .key_metadata()
        .and_then(|metadata| metadata.arn())
        .expect("the key has an ARN")
        .to_string();
    let rows: [(&str, &str, Option<&str>, String); 3] = [
        (
            "a bucket the object store does not hold",
            "update-ghost",
            None,
            "No such bucket: update-ghost".to_string(),
        ),
        (
            "a bucket carrying no grant",
            "update-unpolicied",
            None,
            "Could not write to bucket: update-unpolicied".to_string(),
        ),
        (
            "a key whose policy grants the service nothing",
            "update-granted",
            Some(key_arn.as_str()),
            "Could not write to bucket: update-granted".to_string(),
        ),
    ];
    for (case, bucket, key, expected) in &rows {
        let mut params = vec![field("RuleSetName", RULE_SET)];
        params.extend(rule_fields("s3-delivery", bucket, None, *key));
        let (status, body) = call(&router, "UpdateReceiptRule", &params).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error_envelope(&body);
        assert_eq!(
            code.as_deref(),
            Some("InvalidS3Configuration"),
            "case {case}: expected InvalidS3Configuration, got {code:?} with {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(expected.as_str()),
            "case {case}: expected {expected:?}, got {message:?}"
        );
        // The stored rule is left at its last-good state: the destination it was created
        // with, and no key.
        let (status, described) = describe_rule(&router, "s3-delivery").await;
        assert_eq!(
            status, 200,
            "case {case}: expected the stored rule to survive, got {status} with {described}"
        );
        assert_eq!(
            element(&described, "BucketName").as_deref(),
            Some("update-granted"),
            "case {case}: expected the rule's own bucket, got {:?}",
            element(&described, "BucketName")
        );
        assert_eq!(
            element(&described, "KmsKeyArn"),
            None,
            "case {case}: expected no key on the stored rule, got {:?}",
            element(&described, "KmsKeyArn")
        );
    }
}
