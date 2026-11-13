// SPDX-License-Identifier: CC0-1.0

//! A bucket's REPLICATION configuration as a stack declares it, read back through Amazon S3.
//!
//! RULE: a resource CloudFormation provisions carries, in the service that OWNS it, every
//! sub-resource configuration its declaration states — each applied by the one API call that
//! service publishes for it, since none of them is a member of the create — so a read of
//! that sub-resource straight after the stack reports the resource created answers with what
//! the template declared. For an `AWS::S3::Bucket`'s `ReplicationConfiguration` that is ONE
//! `PutBucketReplication` ("creates a replication configuration or replaces an existing
//! one"), whose API takes the whole container, so `s3:GetBucketReplication` on the bucket the
//! stack published reports the declared role and every member of every declared rule.
//!
//! The template's spelling is the API's with two renames, and the rule covers them: a rule's
//! object scope spells the API's `Filter.Tag` as `TagFilter` and its `Filter.And.Tags` as
//! `TagFilters` — the spelling every id-keyed configuration list of this type uses for the tag
//! half of a scope — so a declaration written either way reads back in the API's own
//! vocabulary and in no other.
//!
//! The rows differ along what the rule generalises over: WHICH schema the rule is written in
//! (the earlier one, whose scope is the rule's own `Prefix`, and the V2 one carrying a
//! `Filter`), which form the scope takes (`Prefix`, a single `TagFilter`, an `And`
//! conjunction of a prefix and tags), which optional containers the rule and its destination
//! carry (`DeleteMarkerReplication`, `SourceSelectionCriteria`'s two, `Account`,
//! `StorageClass`, `AccessControlTranslation`, `EncryptionConfiguration`, `Metrics` with its
//! `EventThreshold`, `ReplicationTime` with its `Time`), and the template SPELLING (JSON and
//! YAML). The second test is the other half of the same rule: a declaration missing a member
//! the API REQUIRES fails that resource with a reason naming the member, rather than
//! completing the stack with a configuration the bucket never carried — the rows there span
//! every required member of the property.
//!
//! Every expected value is computed from what the row declares — the account id the emulator
//! mints and the `arn:aws:s3:::<name>` form an `AWS::S3::Bucket`'s `Arn` attribute publishes
//! — never read back out of the answer under test.

use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, create_stack, describe_stack, resource_event_reason, router, xml_blocks,
    xml_text, ACCOUNT_ID,
};
use crate::stack_resource_provisioning::{output_value, s3};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// How often to look while a stack settles, and how many looks to take: the span is the
/// stack's own, so nothing here sleeps for a fixed one expecting the work to be done, and the
/// deadline is generous so a loaded host cannot decide the outcome.
const LOOK: Duration = Duration::from_millis(100);
const LOOKS: usize = 900;

/// The destination every row replicates to, declared by the same stack and versioned, since
/// replication copies object versions.
const DESTINATION: &str = "stack-replication-destination";

/// Whether a stack status is one a deployment STOPS in.
fn is_terminal(reported: &str) -> bool {
    reported.ends_with("_COMPLETE") || reported.ends_with("_FAILED")
}

/// Poll `DescribeStacks` until the stack settles in a terminal status, and answer that status
/// with the body it was last described with.
async fn settled(router: &MultiServiceRouter, case: &str, stack: &str) -> (String, String) {
    let mut described = String::new();
    for _ in 0..LOOKS {
        let (http_status, body) = describe_stack(router, stack).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: DescribeStacks({stack:?}) must answer 200, got {http_status} {body}"
        );
        described = body;
        let reported = xml_text(&described, "StackStatus").unwrap_or_default();
        if is_terminal(&reported) {
            return (reported, described);
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!("case {case:?}: the deployment must settle in a terminal status, got {described}");
}

/// `GET /<bucket>?replication` — `GetBucketReplication`.
async fn get_replication(router: &MultiServiceRouter, bucket: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?replication"), b"").await;
    (answer.status, answer.body)
}

/// The role ARN the declared configurations name, which IAM mints for the declared role: the
/// physical id of an `AWS::IAM::Role` is its name, and its `Arn` attribute the ARN below.
fn role_arn(role_name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/{role_name}")
}

/// The stack whose three source buckets declare every form of replication rule, in JSON.
const JSON_TEMPLATE: &str = r#"{"Resources":{
     "ReplicationRole":{"Type":"AWS::IAM::Role","Properties":{
       "AssumeRolePolicyDocument":{"Version":"2012-10-17","Statement":[
         {"Effect":"Allow","Principal":{"Service":"s3.amazonaws.com"},
          "Action":"sts:AssumeRole"}]}}},
     "DestinationBucket":{"Type":"AWS::S3::Bucket","Properties":{
       "BucketName":"stack-replication-destination",
       "VersioningConfiguration":{"Status":"Enabled"}}},
     "PlainBucket":{"Type":"AWS::S3::Bucket",
       "DependsOn":["DestinationBucket","ReplicationRole"],
       "Properties":{
       "BucketName":"stack-replication-plain",
       "VersioningConfiguration":{"Status":"Enabled"},
       "ReplicationConfiguration":{
         "Role":{"Fn::GetAtt":["ReplicationRole","Arn"]},
         "Rules":[{"Status":"Enabled","Prefix":"journal/",
           "Destination":{"Bucket":{"Fn::GetAtt":["DestinationBucket","Arn"]}}}]}}},
     "FullBucket":{"Type":"AWS::S3::Bucket",
       "DependsOn":["DestinationBucket","ReplicationRole"],
       "Properties":{
       "BucketName":"stack-replication-full",
       "VersioningConfiguration":{"Status":"Enabled"},
       "ReplicationConfiguration":{
         "Role":{"Fn::GetAtt":["ReplicationRole","Arn"]},
         "Rules":[{
           "Id":"every-tagged-report","Status":"Enabled","Priority":7,
           "DeleteMarkerReplication":{"Status":"Enabled"},
           "Filter":{"And":{"Prefix":"reports/","TagFilters":[
             {"Key":"tier","Value":"gold"},{"Key":"team","Value":"search"}]}},
           "SourceSelectionCriteria":{
             "SseKmsEncryptedObjects":{"Status":"Enabled"},
             "ReplicaModifications":{"Status":"Enabled"}},
           "Destination":{
             "Bucket":{"Fn::GetAtt":["DestinationBucket","Arn"]},
             "Account":"000000000000",
             "StorageClass":"STANDARD_IA",
             "AccessControlTranslation":{"Owner":"Destination"},
             "EncryptionConfiguration":{"ReplicaKmsKeyID":"alias/aws/s3"},
             "Metrics":{"Status":"Enabled","EventThreshold":{"Minutes":15}},
             "ReplicationTime":{"Status":"Enabled","Time":{"Minutes":15}}}}]}}},
     "TaggedBucket":{"Type":"AWS::S3::Bucket",
       "DependsOn":["DestinationBucket","ReplicationRole"],
       "Properties":{
       "BucketName":"stack-replication-tagged",
       "VersioningConfiguration":{"Status":"Enabled"},
       "ReplicationConfiguration":{
         "Role":{"Fn::GetAtt":["ReplicationRole","Arn"]},
         "Rules":[{
           "Id":"one-tag-only","Status":"Enabled","Priority":1,
           "DeleteMarkerReplication":{"Status":"Disabled"},
           "Filter":{"TagFilter":{"Key":"archive","Value":"true"}},
           "Destination":{"Bucket":{"Fn::GetAtt":["DestinationBucket","Arn"]}}}]}}}},
   "Outputs":{
     "RoleName":{"Value":{"Ref":"ReplicationRole"}},
     "PlainBucketName":{"Value":{"Ref":"PlainBucket"}},
     "FullBucketName":{"Value":{"Ref":"FullBucket"}},
     "TaggedBucketName":{"Value":{"Ref":"TaggedBucket"}}}}"#;

/// The same property declared in YAML, whose rule narrows by a `Filter.Prefix`.
const YAML_TEMPLATE: &str = concat!(
    "Resources:\n",
    "  ReplicationRole:\n",
    "    Type: AWS::IAM::Role\n",
    "    Properties:\n",
    "      AssumeRolePolicyDocument:\n",
    "        Version: '2012-10-17'\n",
    "        Statement:\n",
    "          - Effect: Allow\n",
    "            Principal:\n",
    "              Service: s3.amazonaws.com\n",
    "            Action: sts:AssumeRole\n",
    "  DestinationBucket:\n",
    "    Type: AWS::S3::Bucket\n",
    "    Properties:\n",
    "      BucketName: stack-replication-yaml-destination\n",
    "      VersioningConfiguration:\n",
    "        Status: Enabled\n",
    "  SourceBucket:\n",
    "    Type: AWS::S3::Bucket\n",
    "    DependsOn:\n",
    "      - DestinationBucket\n",
    "      - ReplicationRole\n",
    "    Properties:\n",
    "      BucketName: stack-replication-yaml-source\n",
    "      VersioningConfiguration:\n",
    "        Status: Enabled\n",
    "      ReplicationConfiguration:\n",
    "        Role: !GetAtt ReplicationRole.Arn\n",
    "        Rules:\n",
    "          - Id: filtered-by-prefix\n",
    "            Status: Enabled\n",
    "            Priority: 2\n",
    "            DeleteMarkerReplication:\n",
    "              Status: Disabled\n",
    "            Filter:\n",
    "              Prefix: incoming/\n",
    "            Destination:\n",
    "              Bucket: !GetAtt DestinationBucket.Arn\n",
    "              StorageClass: STANDARD\n",
    "Outputs:\n",
    "  RoleName:\n",
    "    Value: !Ref ReplicationRole\n",
    "  SourceBucketName:\n",
    "    Value: !Ref SourceBucket\n",
);

/// One row of the round-trip table: the output naming the bucket, the texts its reported
/// configuration must carry, and the element names it must NOT carry.
struct Row {
    case: &'static str,
    output: &'static str,
    /// The declared values and API element names the report must carry.
    carries: &'static [&'static str],
    /// The template's own spellings, which the API's vocabulary does not use.
    withholds: &'static [&'static str],
}

/// The template's two tag spellings: a report carrying either would mean the declaration
/// reached Amazon S3 unreconciled.
const TEMPLATE_TAG_SPELLINGS: &[&str] = &["TagFilter", "TagFilters"];

const ROWS: &[Row] = &[
    Row {
        case: "a rule of the earlier schema, whose scope is the rule's own Prefix",
        output: "PlainBucketName",
        carries: &["<Prefix>journal/</Prefix>", "<Status>Enabled</Status>"],
        withholds: &["<Filter>"],
    },
    Row {
        case: "a V2 rule carrying every optional container the property admits",
        output: "FullBucketName",
        carries: &[
            "every-tagged-report",
            "<Priority>7</Priority>",
            "<And>",
            "<Prefix>reports/</Prefix>",
            "<Key>tier</Key>",
            "<Value>gold</Value>",
            "<Key>team</Key>",
            "<Value>search</Value>",
            "<StorageClass>STANDARD_IA</StorageClass>",
            "<Account>000000000000</Account>",
            "<Owner>Destination</Owner>",
            "<ReplicaKmsKeyID>alias/aws/s3</ReplicaKmsKeyID>",
            "<Minutes>15</Minutes>",
        ],
        withholds: TEMPLATE_TAG_SPELLINGS,
    },
    Row {
        case: "a V2 rule narrowing by a single tag",
        output: "TaggedBucketName",
        carries: &["one-tag-only", "<Key>archive</Key>", "<Value>true</Value>"],
        withholds: TEMPLATE_TAG_SPELLINGS,
    },
];

/// Deploy a template, wait for it to settle and assert it completed.
async fn deployed(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
) -> String {
    let (http_status, created) =
        create_stack(router, stack, template, &acknowledging_iam(&[])).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: CreateStack must answer 200, got {http_status} {created}"
    );
    let (reported, described) = settled(router, case, stack).await;
    assert_eq!(
        reported,
        status::CREATE_COMPLETE,
        "case {case:?}: the stack must reach CREATE_COMPLETE, got {described}"
    );
    described
}

/// RULE: the bucket a stack declares a replication configuration for carries that
/// configuration in Amazon S3 — the whole container, in the API's own vocabulary.
#[tokio::test]
async fn a_bucket_a_stack_declares_carries_the_replication_configuration_it_declared() {
    let router = router();
    let json = deployed(
        &router,
        "three source buckets, in JSON: every form of declared rule",
        "stack-replication-json",
        JSON_TEMPLATE,
    )
    .await;
    let yaml = deployed(
        &router,
        "one source bucket, in YAML: a rule narrowing by a Filter.Prefix",
        "stack-replication-yaml",
        YAML_TEMPLATE,
    )
    .await;

    let expected_role = role_arn(&output_value(&json, "RoleName"));
    for row in ROWS {
        let case = row.case;
        let name = output_value(&json, row.output);
        assert!(
            !name.is_empty(),
            "case {case:?}: the stack must publish {}, got {json}",
            row.output
        );
        let (http_status, body) = get_replication(&router, &name).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: s3:GetBucketReplication({name:?}) must report the declared \
             configuration, got {http_status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Role").as_deref(),
            Some(expected_role.as_str()),
            "case {case:?}: the reported role must be the one the declaration named, got {body}"
        );
        // The destination is reported as the ARN of the bucket the same stack declared, which
        // is where every row replicates to.
        assert_eq!(
            xml_blocks(&body, "Destination")
                .first()
                .and_then(|block| xml_text(block, "Bucket")),
            Some(format!("arn:aws:s3:::{DESTINATION}")),
            "case {case:?}: expected the declared destination, got {body}"
        );
        for carried in row.carries {
            assert!(
                body.contains(carried),
                "case {case:?}: expected the report to carry {carried:?}, got {body}"
            );
        }
        for withheld in row.withholds {
            assert!(
                !body.contains(withheld),
                "case {case:?}: the API's vocabulary carries no {withheld:?}, got {body}"
            );
        }
    }

    // The YAML row, read the same way: one spelling of a template is not a different rule.
    let case = "a rule narrowing by a Filter.Prefix, declared in YAML";
    let name = output_value(&yaml, "SourceBucketName");
    let (http_status, body) = get_replication(&router, &name).await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: s3:GetBucketReplication({name:?}) must answer 200, got {http_status} \
         {body}"
    );
    for carried in [
        "filtered-by-prefix",
        "<Prefix>incoming/</Prefix>",
        "<StorageClass>STANDARD</StorageClass>",
        "<Priority>2</Priority>",
    ] {
        assert!(
            body.contains(carried),
            "case {case:?}: expected the report to carry {carried:?}, got {body}"
        );
    }
    assert_eq!(
        xml_text(&body, "Role").as_deref(),
        Some(role_arn(&output_value(&yaml, "RoleName")).as_str()),
        "case {case:?}: the reported role must be the one the declaration named, got {body}"
    );
}

/// One row of the refusal table: the rule the declaration writes, and the member whose
/// absence must fail the resource.
struct RefusalRow {
    case: &'static str,
    stack: &'static str,
    bucket: &'static str,
    /// The `ReplicationConfiguration` the template declares, as JSON text.
    declared: &'static str,
    /// The member the reason must name.
    missing: &'static str,
}

const REFUSAL_ROWS: &[RefusalRow] = &[
    RefusalRow {
        case: "a configuration naming no Role",
        stack: "stack-replication-no-role",
        bucket: "stack-replication-no-role-source",
        declared: r#"{"Rules":[{"Status":"Enabled","Prefix":"",
            "Destination":{"Bucket":{"Fn::GetAtt":["DestinationBucket","Arn"]}}}]}"#,
        missing: "Role",
    },
    RefusalRow {
        case: "a rule naming no Status",
        stack: "stack-replication-no-status",
        bucket: "stack-replication-no-status-source",
        declared: r#"{"Role":{"Fn::GetAtt":["ReplicationRole","Arn"]},
            "Rules":[{"Prefix":"",
              "Destination":{"Bucket":{"Fn::GetAtt":["DestinationBucket","Arn"]}}}]}"#,
        missing: "Status",
    },
    RefusalRow {
        case: "a rule naming no Destination",
        stack: "stack-replication-no-destination",
        bucket: "stack-replication-no-destination-source",
        declared: r#"{"Role":{"Fn::GetAtt":["ReplicationRole","Arn"]},
            "Rules":[{"Status":"Enabled","Prefix":""}]}"#,
        missing: "Destination",
    },
    RefusalRow {
        case: "a destination naming no Bucket",
        stack: "stack-replication-no-bucket",
        bucket: "stack-replication-no-bucket-source",
        declared: r#"{"Role":{"Fn::GetAtt":["ReplicationRole","Arn"]},
            "Rules":[{"Status":"Enabled","Prefix":"","Destination":{"StorageClass":"STANDARD"}}]}"#,
        missing: "Destination.Bucket",
    },
    RefusalRow {
        case: "a Metrics container naming no Status",
        stack: "stack-replication-no-metrics-status",
        bucket: "stack-replication-no-metrics-status-source",
        declared: r#"{"Role":{"Fn::GetAtt":["ReplicationRole","Arn"]},
            "Rules":[{"Status":"Enabled","Prefix":"","Destination":{
              "Bucket":{"Fn::GetAtt":["DestinationBucket","Arn"]},
              "Metrics":{"EventThreshold":{"Minutes":15}}}}]}"#,
        missing: "Destination.Metrics.Status",
    },
    RefusalRow {
        case: "a ReplicationTime container naming no Time",
        stack: "stack-replication-no-time",
        bucket: "stack-replication-no-time-source",
        declared: r#"{"Role":{"Fn::GetAtt":["ReplicationRole","Arn"]},
            "Rules":[{"Status":"Enabled","Prefix":"","Destination":{
              "Bucket":{"Fn::GetAtt":["DestinationBucket","Arn"]},
              "ReplicationTime":{"Status":"Enabled"}}}]}"#,
        missing: "Destination.ReplicationTime.Time",
    },
    RefusalRow {
        case: "an AccessControlTranslation naming no Owner",
        stack: "stack-replication-no-owner",
        bucket: "stack-replication-no-owner-source",
        declared: r#"{"Role":{"Fn::GetAtt":["ReplicationRole","Arn"]},
            "Rules":[{"Status":"Enabled","Prefix":"","Destination":{
              "Bucket":{"Fn::GetAtt":["DestinationBucket","Arn"]},
              "Account":"000000000000","AccessControlTranslation":{}}}]}"#,
        missing: "Destination.AccessControlTranslation.Owner",
    },
    RefusalRow {
        case: "a single tag filter naming no Value",
        stack: "stack-replication-no-tag-value",
        bucket: "stack-replication-no-tag-value-source",
        declared: r#"{"Role":{"Fn::GetAtt":["ReplicationRole","Arn"]},
            "Rules":[{"Status":"Enabled","Priority":1,
              "DeleteMarkerReplication":{"Status":"Disabled"},
              "Filter":{"TagFilter":{"Key":"archive"}},
              "Destination":{"Bucket":{"Fn::GetAtt":["DestinationBucket","Arn"]}}}]}"#,
        missing: "Filter.TagFilter.Value",
    },
];

/// The template one refusal row deploys: the role and the destination it names, and the
/// source bucket whose declaration is the row's own.
fn refusal_template(row: &RefusalRow) -> String {
    format!(
        r#"{{"Resources":{{
             "ReplicationRole":{{"Type":"AWS::IAM::Role","Properties":{{
               "AssumeRolePolicyDocument":{{"Version":"2012-10-17","Statement":[
                 {{"Effect":"Allow","Principal":{{"Service":"s3.amazonaws.com"}},
                   "Action":"sts:AssumeRole"}}]}}}}}},
             "DestinationBucket":{{"Type":"AWS::S3::Bucket","Properties":{{
               "BucketName":"{bucket}-destination",
               "VersioningConfiguration":{{"Status":"Enabled"}}}}}},
             "SourceBucket":{{"Type":"AWS::S3::Bucket",
               "DependsOn":["DestinationBucket","ReplicationRole"],
               "Properties":{{
               "BucketName":"{bucket}",
               "VersioningConfiguration":{{"Status":"Enabled"}},
               "ReplicationConfiguration":{declared}}}}}}}}}"#,
        bucket = row.bucket,
        declared = row.declared,
    )
}

/// RULE (the other half): a declaration missing a member Amazon S3 REQUIRES fails that
/// resource with a reason naming the member, so the stack never completes carrying a
/// configuration the bucket does not hold.
#[tokio::test]
async fn a_declaration_missing_a_required_member_fails_the_resource() {
    let router = router();

    for row in REFUSAL_ROWS {
        let case = row.case;
        let (http_status, created) = create_stack(
            &router,
            row.stack,
            &refusal_template(row),
            &acknowledging_iam(&[]),
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: CreateStack must answer 200, got {http_status} {created}"
        );
        let (reported, described) = settled(&router, case, row.stack).await;
        assert_eq!(
            reported,
            status::ROLLBACK_COMPLETE,
            "case {case:?}: a resource the provisioner refused must roll the stack back, got \
             {described}"
        );
        let reason =
            resource_event_reason(&router, row.stack, "SourceBucket", "CREATE_FAILED").await;
        for expected in [
            "ReplicationConfiguration",
            &format!("declares no {}", row.missing),
        ] {
            assert!(
                reason.contains(expected),
                "case {case:?}: expected the reason to carry {expected:?}, got {reason:?}"
            );
        }
        // Nothing of the refused configuration reached Amazon S3: the rolled-back stack keeps
        // no bucket at all, so the read of its replication answers no bucket.
        let (http_status, body) = get_replication(&router, row.bucket).await;
        assert_ne!(
            http_status, 200,
            "case {case:?}: the rolled-back bucket must hold nothing, got {http_status} {body}"
        );
    }
}
