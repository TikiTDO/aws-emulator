// SPDX-License-Identifier: CC0-1.0

//! A bucket's DEFAULT encryption as a stack declares it, read back through Amazon S3 and
//! applied to the writes that inherit it.
//!
//! RULE: a resource CloudFormation provisions carries, in the service that OWNS it, every
//! sub-resource configuration its declaration states — each applied by the one API call that
//! service publishes for it, since none of them is a member of the create. For an
//! `AWS::S3::Bucket` the whole `BucketEncryption` is ONE `PutBucketEncryption`, whose API
//! takes the rule set as a single container, so a bucket a template declares a default
//! encryption for behaves exactly like one configured through that call: the configuration
//! reads back as declared, and a write that names no encryption of its own is stored under it
//! ("Amazon S3 encrypts data with server-side encryption by using Amazon S3 managed keys
//! (SSE-S3) by default. You can optionally tell Amazon S3 to encrypt data at rest by using
//! server-side encryption with other key options",
//! `smithy_query(query="doc",shape="PutObjectRequest",member="ServerSideEncryption")` — the
//! other key options being the ones `PutBucketEncryption` configures).
//!
//! The template nests the rules one level differently from the API and renames nothing else:
//! `BucketEncryption.ServerSideEncryptionConfiguration` is the LIST the API carries as
//! `ServerSideEncryptionConfiguration.Rules`, and each entry states the same two things a
//! `ServerSideEncryptionRule` does — a `ServerSideEncryptionByDefault` naming the
//! `SSEAlgorithm` and, for the SSE-KMS algorithms, the `KMSMasterKeyID`, beside the rule's own
//! `BucketKeyEnabled`.
//!
//! The rows differ along what the rule generalises over: WHICH algorithm the declaration
//! names (SSE-KMS with no key, SSE-KMS naming a key, dual-layer SSE-KMS, SSE-S3, and nothing
//! at all), whether an S3 Bucket Key is declared and in which position, and the template
//! SPELLING (JSON and YAML).
//!
//! Every expected value is computed from what the row declares, never read back out of the
//! answer under test.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    create_stack, describe_stack, router, wait_for_the_transition, xml_blocks, xml_text,
};
use crate::stack_resource_provisioning::{output_value, s3};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The header an object's server-side encryption is reported in.
const SSE_HEADER: &str = "x-amz-server-side-encryption";

/// The `ServerSideEncryption` value a bucket with no default encryption of its own stores an
/// object under: "by default, all buckets have a default encryption configuration that uses
/// server-side encryption with Amazon S3 managed keys (SSE-S3)" (`GetBucketEncryption`).
const AES256: &str = "AES256";

/// One default-encryption rule as `GetBucketEncryption` reports it.
#[derive(Debug, PartialEq, Eq)]
struct ReportedRule {
    algorithm: Option<String>,
    kms_key: Option<String>,
    bucket_key: Option<String>,
}

/// The rules one `GetBucketEncryption` body reports, in document order.
fn reported_rules(body: &str) -> Vec<ReportedRule> {
    xml_blocks(body, "Rule")
        .iter()
        .map(|rule| ReportedRule {
            algorithm: xml_text(rule, "SSEAlgorithm"),
            kms_key: xml_text(rule, "KMSMasterKeyID"),
            bucket_key: xml_text(rule, "BucketKeyEnabled"),
        })
        .collect()
}

/// A rule a row EXPECTS, in the same shape [`reported_rules`] reads one into.
fn rule(algorithm: &str, kms_key: Option<&str>, bucket_key: bool) -> ReportedRule {
    ReportedRule {
        algorithm: Some(algorithm.to_string()),
        kms_key: kms_key.map(str::to_string),
        // A `ServerSideEncryptionRule`'s own `BucketKeyEnabled` is a SETTING with a
        // documented off state ("General purpose buckets - By default, S3 Bucket Key is not
        // enabled"), so a rule stored without it reads back with it explicitly off.
        bucket_key: Some(bucket_key.to_string()),
    }
}

/// `GET /<bucket>?encryption` — `GetBucketEncryption`.
async fn get_encryption(router: &MultiServiceRouter, bucket: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?encryption"), b"").await;
    (answer.status, answer.body)
}

/// Deploy a template and answer with the `DescribeStacks` body it completed with.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (http_status, created) = create_stack(router, stack_name, template, &[]).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: CreateStack must answer 200, got {http_status} {created}"
    );
    wait_for_the_transition().await;

    let (http_status, described) = describe_stack(router, stack_name).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: DescribeStacks must answer 200, got {http_status} {described}"
    );
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the stack must reach CREATE_COMPLETE, got {described}"
    );
    described
}

/// The key the one write of each bucket below is made under, and its bytes.
const SEEDED_KEY: &str = "sample.csv";
const SEEDED_BODY: &[u8] = b"region,total\nus-west-2,41\n";

/// The KMS key one row's rule names: the predefined Amazon Web Services alias of this
/// account's managed key for Amazon S3 — a reference in the alias form `KMSMasterKeyID`
/// documents ("you can specify the key ID, key alias, or the Amazon Resource Name (ARN) of
/// the KMS key"), which the bucket's rule stores and echoes verbatim and which KMS resolves
/// for the writes that inherit the rule.
const DECLARED_KEY_REFERENCE: &str = "alias/aws/s3";

/// The stack whose four buckets declare every form of default encryption, in JSON.
const JSON_TEMPLATE: &str = r#"{"Resources":{
     "ProdBucket":{"Type":"AWS::S3::Bucket","Properties":{
       "BucketName":"stack-sse-prod",
       "VersioningConfiguration":{"Status":"Enabled"},
       "BucketEncryption":{"ServerSideEncryptionConfiguration":[
         {"ServerSideEncryptionByDefault":{"SSEAlgorithm":"aws:kms"},
          "BucketKeyEnabled":true}]}}},
     "StagingBucket":{"Type":"AWS::S3::Bucket","Properties":{
       "BucketName":"stack-sse-staging",
       "BucketEncryption":{"ServerSideEncryptionConfiguration":[
         {"BucketKeyEnabled":false,
          "ServerSideEncryptionByDefault":{"SSEAlgorithm":"aws:kms",
            "KMSMasterKeyID":"alias/aws/s3"}}]}}},
     "LogsBucket":{"Type":"AWS::S3::Bucket","Properties":{
       "BucketName":"stack-sse-logs",
       "BucketEncryption":{"ServerSideEncryptionConfiguration":[
         {"ServerSideEncryptionByDefault":{"SSEAlgorithm":"AES256"}}]}}},
     "RawBucket":{"Type":"AWS::S3::Bucket","Properties":{
       "BucketName":"stack-sse-raw"}}},
   "Outputs":{
     "ProdBucketName":{"Value":{"Ref":"ProdBucket"}},
     "StagingBucketName":{"Value":{"Ref":"StagingBucket"}},
     "LogsBucketName":{"Value":{"Ref":"LogsBucket"}},
     "RawBucketName":{"Value":{"Ref":"RawBucket"}}}}"#;

/// The stack whose bucket declares a dual-layer SSE-KMS default, in YAML.
const YAML_TEMPLATE: &str = concat!(
    "Resources:\n",
    "  ArchiveBucket:\n",
    "    Type: AWS::S3::Bucket\n",
    "    Properties:\n",
    "      BucketName: stack-sse-archive\n",
    "      BucketEncryption:\n",
    "        ServerSideEncryptionConfiguration:\n",
    "          - ServerSideEncryptionByDefault:\n",
    "              SSEAlgorithm: aws:kms:dsse\n",
    "            BucketKeyEnabled: true\n",
    "Outputs:\n",
    "  ArchiveBucketName:\n",
    "    Value: !Ref ArchiveBucket\n",
);

/// One row of the table: the case, the stack output naming the bucket, the rules its
/// `GetBucketEncryption` must report, and the `ServerSideEncryption` a write that names none
/// of its own must then be stored under.
struct Row {
    case: &'static str,
    output: &'static str,
    rules: Vec<ReportedRule>,
    stored_under: &'static str,
}

#[tokio::test]
async fn a_bucket_a_stack_declares_carries_the_default_encryption_it_declared() {
    let router = router();
    let json = deploy(
        &router,
        "four buckets, in JSON: every form of declared default encryption",
        "stack-sse-json",
        JSON_TEMPLATE,
    )
    .await;
    let yaml = deploy(
        &router,
        "one bucket, in YAML: a dual-layer SSE-KMS default",
        "stack-sse-yaml",
        YAML_TEMPLATE,
    )
    .await;

    let rows: Vec<(Row, &str)> = vec![
        (
            Row {
                case: "an SSE-KMS default naming no key, with a Bucket Key",
                output: "ProdBucketName",
                rules: vec![rule("aws:kms", None, true)],
                stored_under: "aws:kms",
            },
            &json,
        ),
        (
            Row {
                case: "an SSE-KMS default naming a key, with the Bucket Key off",
                output: "StagingBucketName",
                rules: vec![rule("aws:kms", Some(DECLARED_KEY_REFERENCE), false)],
                stored_under: "aws:kms",
            },
            &json,
        ),
        (
            Row {
                case: "an SSE-S3 default, named explicitly",
                output: "LogsBucketName",
                rules: vec![rule(AES256, None, false)],
                stored_under: AES256,
            },
            &json,
        ),
        (
            Row {
                case: "no BucketEncryption at all, so the base default stands",
                output: "RawBucketName",
                rules: vec![rule(AES256, None, false)],
                stored_under: AES256,
            },
            &json,
        ),
        (
            Row {
                case: "a dual-layer SSE-KMS default declared in YAML",
                output: "ArchiveBucketName",
                rules: vec![rule("aws:kms:dsse", None, true)],
                stored_under: "aws:kms:dsse",
            },
            &yaml,
        ),
    ];

    for (row, described) in &rows {
        let Row {
            case,
            output,
            rules,
            stored_under,
        } = row;
        let name = output_value(described, output);
        assert!(
            !name.is_empty(),
            "case {case:?}: the stack must publish {output}, got {described}"
        );

        // The configuration the stack installed, read through the call S3 publishes for it.
        let (http_status, body) = get_encryption(&router, &name).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: s3:GetBucketEncryption({name:?}) must answer 200, got \
             {http_status} {body}"
        );
        let reported = reported_rules(&body);
        assert_eq!(
            reported, *rules,
            "case {case:?}: {name:?} must report {rules:?}, got {reported:?} ({body})"
        );

        // And a write that names no encryption of its own is stored under it — which is what
        // makes the declared default the one in force rather than a record nothing applies.
        let written = s3(
            &router,
            "PUT",
            &format!("/{name}/{SEEDED_KEY}"),
            SEEDED_BODY,
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: the seed write into {name:?} must answer 200, got {} {}",
            written.status, written.body
        );
        assert_eq!(
            written.header(SSE_HEADER).as_deref(),
            Some(*stored_under),
            "case {case:?}: the seed into {name:?} must report {stored_under:?}, got {:?}",
            written.header(SSE_HEADER)
        );
        let read = s3(&router, "HEAD", &format!("/{name}/{SEEDED_KEY}"), b"").await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the seeded object must read back, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.header(SSE_HEADER).as_deref(),
            Some(*stored_under),
            "case {case:?}: a read of the seed must report the same, got {:?}",
            read.header(SSE_HEADER)
        );
    }
}
