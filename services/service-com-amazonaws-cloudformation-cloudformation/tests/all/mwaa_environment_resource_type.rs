// SPDX-License-Identifier: CC0-1.0

//! `AWS::MWAA::Environment` is a resource type a template may DECLARE: every operation that
//! submits a template admits a body declaring one, and the catalogue entries behind that say
//! what a resource of the type reports.
//!
//! The rule under test: **a resource type Amazon Web Services publishes belongs to the
//! catalogue a submitted template is held to, so a body declaring it is admitted and the type
//! reports the identity and the return values its own "Return values" section publishes** —
//! the half of the rule this crate owns on its own. It is the mirror of
//! `unrecognized_resource_types.rs`, which pins the other direction: a type CloudFormation
//! does NOT know is refused `Template format error: Unrecognized resource types: [<types>]`,
//! and that refusal must never be the answer for a type AWS publishes.
//!
//! What the stack then DOES with the declaration — the call to Amazon MWAA's own
//! `CreateEnvironment`, the values that service reports for the environment, the change an
//! update applies to it, its removal with the stack and the refusals it answers — needs that
//! service mounted beside this one, so it lives in the router's own cross-service suite
//! (`core/router/tests/all/cloudformation_provisions_mwaa_environments.rs`). This suite
//! asserts admission alone, which is what a client sees before any resource is attempted.
//!
//! The rows differ along what the rule generalises over: the declared properties (the required
//! five alone, those plus the class, the fleet and the access mode, those plus the type's own
//! `Tags` and a logging configuration), whether the declaration names the environment at all,
//! the template spelling (JSON and YAML), how many environments one body declares, and whether
//! the declaration stands beside a resource of another type.

use service_com_amazonaws_cloudformation_cloudformation::state::resource_types::{
    is_published, is_recognized, AWS_RESOURCE_TYPES,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::{
    identity_is_service_assigned, name_property_candidates, published_attribute_names,
    replacement_properties, MWAA_ENVIRONMENT,
};

use crate::stack_lifecycle::{call, create_stack, member, router, xml_text};

/// The fixed form of the refusal a type CloudFormation does not know is answered with, which
/// no row here may ever see.
const UNRECOGNIZED_PREFIX: &str = "Template format error: Unrecognized resource types: [";

/// The placement, the execution role and the DAG bucket the bodies below declare, each of the
/// form the member's own modelled pattern requires (`^subnet-[a-zA-Z0-9\-._]+$`,
/// `^sg-[a-zA-Z0-9\-._]+$`, the IAM role and Amazon S3 bucket ARN patterns).
const SUBNET_A: &str = "subnet-0a1b2c3d4e5f60001";
const SUBNET_B: &str = "subnet-0a1b2c3d4e5f60002";
const SECURITY_GROUP: &str = "sg-0a1b2c3d4e5f60001";
const EXECUTION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/mwaa-execution";
const SOURCE_BUCKET_ARN: &str = "arn:aws:s3:::mwaa-dags-bucket";

/// Every `Fn::GetAtt` attribute the type's "Return values" section publishes, in the sorted
/// order a resource's own attribute map holds them in.
const PUBLISHED_ATTRIBUTES: &[&str] = &[
    "Arn",
    "CeleryExecutorQueue",
    "DatabaseVpcEndpointService",
    "LoggingConfiguration.DagProcessingLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.SchedulerLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.TaskLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.WebserverLogs.CloudWatchLogGroupArn",
    "LoggingConfiguration.WorkerLogs.CloudWatchLogGroupArn",
    "WebserverUrl",
    "WebserverVpcEndpointService",
];

/// One row: a template body declaring the type, and the name of the stack it is submitted
/// under.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
}

fn rows() -> Vec<Row> {
    vec![
        // The reported case: the five `@required` members of the create, with every endpoint
        // return value read back through `Outputs`.
        Row {
            case: "the required members alone, with the endpoint return values read back",
            stack_name: "mwaa-environment-admitted",
            template: format!(
                r#"{{"AWSTemplateFormatVersion": "2010-09-09",
                     "Resources": {{"MwaaEnvironment": {{"Type": "{MWAA_ENVIRONMENT}",
                       "Properties": {{"Name": "airflow-admitted",
                         "ExecutionRoleArn": "{EXECUTION_ROLE_ARN}",
                         "SourceBucketArn": "{SOURCE_BUCKET_ARN}",
                         "DagS3Path": "dags",
                         "NetworkConfiguration": {{
                           "SubnetIds": ["{SUBNET_A}", "{SUBNET_B}"],
                           "SecurityGroupIds": ["{SECURITY_GROUP}"]}}}}}}}},
                     "Outputs": {{"EnvironmentRef": {{"Value": {{"Ref": "MwaaEnvironment"}}}},
                       "Arn": {{"Value": {{"Fn::GetAtt": ["MwaaEnvironment", "Arn"]}}}},
                       "CeleryQueue": {{"Value": {{"Fn::GetAtt": ["MwaaEnvironment", "CeleryExecutorQueue"]}}}},
                       "DatabaseService": {{"Value": {{"Fn::GetAtt": ["MwaaEnvironment", "DatabaseVpcEndpointService"]}}}},
                       "WebserverService": {{"Value": {{"Fn::GetAtt": ["MwaaEnvironment", "WebserverVpcEndpointService"]}}}},
                       "WebserverUrl": {{"Value": {{"Fn::GetAtt": ["MwaaEnvironment", "WebserverUrl"]}}}}}}}}"#
            ),
        },
        // The same type carrying the class, the worker fleet and the access mode, and NO name
        // at all — so the environment carries the one CloudFormation generates.
        Row {
            case: "the class, the fleet and the access mode, with no declared name",
            stack_name: "mwaa-environment-unnamed",
            template: format!(
                r#"{{"Resources": {{"Airflow": {{"Type": "{MWAA_ENVIRONMENT}",
                     "Properties": {{"ExecutionRoleArn": "{EXECUTION_ROLE_ARN}",
                       "SourceBucketArn": "{SOURCE_BUCKET_ARN}",
                       "DagS3Path": "workflows/dags",
                       "NetworkConfiguration": {{
                         "SubnetIds": ["{SUBNET_A}", "{SUBNET_B}"],
                         "SecurityGroupIds": ["{SECURITY_GROUP}"]}},
                       "EnvironmentClass": "mw1.micro",
                       "MinWorkers": 1, "MaxWorkers": 1,
                       "WebserverAccessMode": "PRIVATE_ONLY"}}}}}}}}"#
            ),
        },
        // The YAML spelling, with the type's own `Tags`, a logging configuration and a
        // short-form intrinsic reading one log type's published log-group ARN.
        Row {
            case: "the YAML spelling, with the type's own Tags and a logging configuration",
            stack_name: "mwaa-environment-yaml",
            template: format!(
                "\
Resources:
  Airflow:
    Type: {MWAA_ENVIRONMENT}
    Properties:
      Name: airflow-yaml
      ExecutionRoleArn: {EXECUTION_ROLE_ARN}
      SourceBucketArn: {SOURCE_BUCKET_ARN}
      DagS3Path: dags
      NetworkConfiguration:
        SubnetIds:
          - {SUBNET_A}
          - {SUBNET_B}
        SecurityGroupIds:
          - {SECURITY_GROUP}
      LoggingConfiguration:
        SchedulerLogs:
          Enabled: true
          LogLevel: INFO
      Tags:
        team: data-platform
Outputs:
  SchedulerLogGroup:
    Value: !GetAtt Airflow.LoggingConfiguration.SchedulerLogs.CloudWatchLogGroupArn
"
            ),
        },
        // Two environments in one body, beside a resource of another type: the catalogue
        // admits every type a body declares or none of them.
        Row {
            case: "two environments beside a resource of another type",
            stack_name: "mwaa-environment-mixed",
            template: format!(
                r#"{{"Resources": {{
                     "First": {{"Type": "{MWAA_ENVIRONMENT}",
                       "Properties": {{"Name": "airflow-mixed-first",
                         "ExecutionRoleArn": "{EXECUTION_ROLE_ARN}",
                         "SourceBucketArn": "{SOURCE_BUCKET_ARN}",
                         "DagS3Path": "dags",
                         "NetworkConfiguration": {{
                           "SubnetIds": ["{SUBNET_A}", "{SUBNET_B}"],
                           "SecurityGroupIds": ["{SECURITY_GROUP}"]}}}}}},
                     "Second": {{"Type": "{MWAA_ENVIRONMENT}",
                       "Properties": {{"Name": "airflow-mixed-second",
                         "ExecutionRoleArn": "{EXECUTION_ROLE_ARN}",
                         "SourceBucketArn": "{SOURCE_BUCKET_ARN}",
                         "DagS3Path": "dags",
                         "NetworkConfiguration": {{
                           "SubnetIds": ["{SUBNET_A}", "{SUBNET_B}"],
                           "SecurityGroupIds": ["{SECURITY_GROUP}"]}}}}}},
                     "Topic": {{"Type": "AWS::SNS::Topic",
                       "Properties": {{"TopicName": "mwaa-environment-topic"}}}}}}}}"#
            ),
        },
    ]
}

/// Assert one answer admitted the body rather than refusing its resource types.
fn assert_admitted(case: &str, status: u16, body: &str) {
    let message = xml_text(body, "Message").unwrap_or_default();
    assert!(
        !message.starts_with(UNRECOGNIZED_PREFIX),
        "case {case:?}: expected a type Amazon Web Services publishes to be recognised, got \
         {message:?}"
    );
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 for a body declaring a published type, got {status} \
         with body {body}"
    );
}

/// Every operation that submits a template for deployment admits a body declaring the type,
/// and so does the operation whose whole answer is a verdict on the body.
#[tokio::test]
async fn every_operation_that_takes_a_template_admits_an_mwaa_environment() {
    for row in rows() {
        let router = router();
        let (status, body) = create_stack(&router, row.stack_name, &row.template, &[]).await;
        assert_admitted(row.case, status, &body);
        let stack_id = xml_text(&body, "StackId").unwrap_or_default();
        assert!(
            stack_id.contains(":stack/"),
            "case {:?}: expected CreateStack to answer a stack id, got {body}",
            row.case
        );

        let (status, body) = call(
            &router,
            "ValidateTemplate",
            &[member("TemplateBody", &row.template)],
        )
        .await;
        assert_admitted(row.case, status, &body);

        let (status, body) = call(
            &router,
            "CreateChangeSet",
            &[
                member("StackName", &format!("{}-change-set", row.stack_name)),
                member("ChangeSetName", "a-change-set"),
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", &row.template),
            ],
        )
        .await;
        assert_admitted(row.case, status, &body);
    }
}

/// The catalogue entries the rule rests on: the type is published and recognised, a near-miss
/// spelling of it still is not, and the type reports the identity and the return values its
/// own "Return values" section publishes.
#[test]
fn the_catalogue_publishes_the_mwaa_environment_type_and_what_it_returns() {
    assert!(
        AWS_RESOURCE_TYPES.contains(&MWAA_ENVIRONMENT),
        "expected {MWAA_ENVIRONMENT:?} in the published catalogue, it is absent"
    );
    assert!(
        is_published(MWAA_ENVIRONMENT) && is_recognized(MWAA_ENVIRONMENT),
        "expected {MWAA_ENVIRONMENT:?} to be a type a template may declare, it is not"
    );
    // The catalogue is a list of names, not a shape test: a name it does not carry stays
    // unrecognised however close it is to one it does.
    for misspelled in [
        "AWS::MWAA::Environments",
        "AWS::MWAA::Env",
        "AWS::MWAA::Workflow",
        "AWS::Mwaa::Environment",
    ] {
        assert!(
            !is_recognized(misspelled),
            "expected {misspelled:?} to stay unrecognised, it is recognised"
        );
    }
    // "Ref returns the name of the Amazon MWAA environment", which is the name the template
    // states in the type's own `Name` property — so the identity is not one Amazon MWAA
    // assigns, and the property the name is read from is a candidate of the type.
    assert!(
        !identity_is_service_assigned(MWAA_ENVIRONMENT),
        "expected the type's identity to be the declared name, it is service-assigned"
    );
    assert!(
        name_property_candidates(MWAA_ENVIRONMENT).contains(&"Name".to_string()),
        "expected the type to take its name from its own Name property, got {:?}",
        name_property_candidates(MWAA_ENVIRONMENT)
    );
    assert_eq!(
        published_attribute_names(MWAA_ENVIRONMENT),
        PUBLISHED_ATTRIBUTES,
        "expected the type's published return values, got {:?}",
        published_attribute_names(MWAA_ENVIRONMENT)
    );
    // `UpdateEnvironment` takes every member of the create but the encryption key and who
    // manages the environment's VPC endpoints, so a declaration restating either of those two
    // asks for a different environment.
    assert_eq!(
        replacement_properties(MWAA_ENVIRONMENT),
        &["EndpointManagement", "KmsKey"],
        "expected the type's create-only properties, got {:?}",
        replacement_properties(MWAA_ENVIRONMENT)
    );
}
