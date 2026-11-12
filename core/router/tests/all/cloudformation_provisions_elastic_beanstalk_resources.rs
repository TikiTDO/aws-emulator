// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares AWS Elastic Beanstalk resources really creates
//! them in Elastic Beanstalk, and the stack reaches `CREATE_COMPLETE` only once it has.
//!
//! The rule under test: **a resource of a type this emulator provisions is created in the
//! service that OWNS it, through that service's own public API, before the stack reports
//! the resource as created — so a client that provisions through CloudFormation can go on
//! to call the owning service about it — and a create that owning service REFUSES fails
//! the stack instead of completing it.** Three crates have to agree for that to hold
//! (CloudFormation makes the calls, Elastic Beanstalk serves them, Amazon S3 holds the
//! source bundle one of them downloads), so the suite lives here rather than in any one
//! of them.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! application, the application version and the environment or leaves CloudFormation to
//! generate each name, whether the environment names its platform by `SolutionStackName`
//! or by `PlatformArn`, which environment tier it runs, whether it declares
//! `OptionSettings`, `Tags` and a source bundle at all, whether the template is written
//! in JSON or YAML, and which Region it deploys in.
//!
//! Every expected value is computed here from what the row declares, never read back out
//! of the answer being checked — except the GENERATED names, which are CloudFormation's
//! to compute: what is checked about those is that they are inside the owning service's
//! own modelled bounds and that Elastic Beanstalk resolves them to the resources the
//! template declared.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{element, elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) AND for
/// the Elastic Beanstalk transitions each stack operation starts there to come due: an
/// environment the stack launches reaches `Ready`, and the deletion of the stack terminates
/// that environment and THEN deletes the application it belonged to — two of that service's
/// own transition spans, one after the other. Derived from the service's own constant, so
/// the two cannot drift apart.
const PAST_STACK_SPAN: Duration = eb::state::STATUS_TRANSITION_SPAN
    .saturating_mul(2)
    .saturating_add(Duration::from_millis(900));

/// Elastic Beanstalk's API version, as its model declares it (`version: 2010-12-01`).
const EB_API_VERSION: &str = "2010-12-01";

/// `EnvironmentName`'s modelled bounds (`@length(min: 4, max: 40)` in Elastic Beanstalk's
/// own model), restated here: a name CloudFormation GENERATES has to be inside them, or
/// the create could not have been accepted at all.
const ENVIRONMENT_NAME_BOUNDS: (usize, usize) = (4, 40);

/// `VersionLabel`'s and `ApplicationName`'s modelled bounds
/// (`@length(min: 1, max: 100)`).
const HUNDRED_CHARACTER_BOUNDS: (usize, usize) = (1, 100);

/// The emulator with the three services this suite exercises, mounted on ONE engine
/// exactly as `core/router/src/lib.rs` mounts them. Request authentication is off, as in
/// every other suite here; the in-process callback CloudFormation makes signs its request
/// either way (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(eb::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`,
/// which is what routes the request and what each handler reads its Region from. The
/// signature is never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one awsQuery operation of `service` in `region`, returning its status and body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    region: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service, region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", region, "2010-05-15", action, members).await
}

/// Call one Elastic Beanstalk operation over its awsQuery wire, signed for `region`.
async fn elasticbeanstalk(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(
        router,
        "elasticbeanstalk",
        region,
        EB_API_VERSION,
        action,
        members,
    )
    .await
}

/// Store one object in the emulator's own object storage, creating its bucket first, so
/// the source bundle an `AWS::ElasticBeanstalk::ApplicationVersion` names is an archive
/// Amazon S3 really holds — Elastic Beanstalk DOWNLOADS it while serving the create.
///
/// The create carries a `CreateBucketConfiguration` naming the Region for every Region but
/// `us-east-1`, which is Amazon S3's own rule: "If you don't specify a Region, the bucket
/// is created in the US East (N. Virginia) Region (us-east-1) by default"
/// (`CreateBucketConfiguration$LocationConstraint`), so a create signed for another Region
/// that names none is refused.
async fn put_object(router: &MultiServiceRouter, region: &str, bucket: &str, key: &str) {
    let created = match region {
        "us-east-1" => Bytes::new(),
        region => Bytes::from(format!(
            "<CreateBucketConfiguration><LocationConstraint>{region}\
             </LocationConstraint></CreateBucketConfiguration>"
        )),
    };
    for (method, path, body) in [
        ("PUT", format!("/{bucket}"), created),
        (
            "PUT",
            format!("/{bucket}/{key}"),
            Bytes::from_static(b"PK\x03\x04 a source bundle"),
        ),
    ] {
        let request = http::Request::builder()
            .method(method)
            .uri(&path)
            .header("host", format!("s3.{region}.amazonaws.com"))
            .header("authorization", authorization("s3", region))
            .body(body)
            .expect("a well-formed request");
        let response = router.dispatch(request).await;
        assert!(
            response.status().is_success(),
            "seeding s3://{bucket}/{key}: expected a success from {method} {path}, got {} and \
             body {}",
            response.status(),
            String::from_utf8_lossy(response.body())
        );
    }
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// How many environments an Elastic Beanstalk `DescribeEnvironments` answer reports: one
/// `EnvironmentId` per environment, and nothing else in the answer carries that element.
fn environment_count(xml: &str) -> usize {
    elements(xml, "EnvironmentId").len()
}

/// Each `<member>` of a FLAT awsQuery list, as its own inner XML.
///
/// The first member of such a list repeats the envelope's `xmlns` attribute
/// (`<member xmlns="…">`), which the shared reader's literal `<member>` would skip, so the
/// split here is on the opening tag's NAME. Only for a list whose members hold no list of
/// their own, which is what a tag set and a configuration option list are.
fn members(xml: &str) -> Vec<&str> {
    xml.split("<member")
        .skip(1)
        .filter_map(|rest| {
            let start = rest.find('>')? + 1;
            let end = rest.find("</member>")?;
            (start <= end).then_some(&rest[start..end])
        })
        .collect()
}

/// One row of the table: a template, and what the rule says the resources it declares must
/// then be in Elastic Beanstalk.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region its resources are created in.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML), built from the
    /// platform the row runs on.
    template: fn(&str) -> String,
    /// The bucket and key the row's source bundle must be seeded at, or `None` for a row
    /// that declares no application version at all.
    bundle: Option<(&'static str, &'static str)>,
    /// The name the template declares for the environment, or `None` for a row that names
    /// nothing — whose environment carries the name CloudFormation generated.
    declared_environment: Option<&'static str>,
    /// The name the template declares for the application, or `None` as above.
    declared_application: Option<&'static str>,
    /// The `Tier` members `DescribeEnvironments` must report for the environment.
    expected_tier: &'static [(&'static str, &'static str)],
    /// The `(Namespace, OptionName, Value)` the environment's configuration must carry
    /// because the template declared it as an `OptionSettings` entry.
    expected_option: Option<(&'static str, &'static str, &'static str)>,
    /// The tags Elastic Beanstalk must report for the environment's ARN, beside the three
    /// CloudFormation records on every resource it provisions.
    expected_tags: &'static [(&'static str, &'static str)],
    /// Whether the row's environment names its platform by `PlatformArn` (rather than by
    /// `SolutionStackName`), which is the form the stack must then report back.
    by_platform_arn: bool,
}

/// The platform the first row runs on: the current version of the first branch of the
/// service's OWN published catalog (`eb::state::SOLUTION_STACKS`, what
/// `ListAvailableSolutionStacks` reports), named by its solution stack name.
fn first_solution_stack() -> String {
    eb::state::SOLUTION_STACKS
        .first()
        .expect("the service publishes at least one solution stack")
        .name()
}

/// The platform the other rows run on, named by its ARN in `region`: a DIFFERENT branch
/// of the same catalog, so no row is the only platform exercised.
fn second_platform_arn(region: &str) -> String {
    eb::state::SOLUTION_STACKS
        .get(1)
        .expect("the service publishes several solution stacks")
        .platform_arn(region)
}

/// A worker-tier platform ARN: the catalog's third row, so the worker row names a third
/// branch.
fn worker_platform_arn(region: &str) -> String {
    eb::state::SOLUTION_STACKS
        .get(2)
        .expect("the service publishes several solution stacks")
        .platform_arn(region)
}

const ROWS: &[Row] = &[
    Row {
        case: "a named application and web-server environment on a solution stack, in JSON",
        region: "us-west-2",
        template: |_platform| {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "an Elastic Beanstalk application and environment",
                "Resources": {
                    "EbApplication": {
                        "Type": "AWS::ElasticBeanstalk::Application",
                        "Properties": {
                            "ApplicationName": "orders-app",
                            "Description": "the orders application"
                        }
                    },
                    "EbEnvironment": {
                        "Type": "AWS::ElasticBeanstalk::Environment",
                        "Properties": {
                            "ApplicationName": { "Ref": "EbApplication" },
                            "EnvironmentName": "orders-env",
                            "Description": "the orders environment",
                            "SolutionStackName": first_solution_stack(),
                            "CNAMEPrefix": "orders-env-cfn",
                            "Tier": { "Name": "WebServer", "Type": "Standard" },
                            "OptionSettings": [{
                                "Namespace": "aws:autoscaling:asg",
                                "OptionName": "MinSize",
                                "Value": "2"
                            }],
                            "Tags": [
                                { "Key": "Owner", "Value": "orders" },
                                { "Key": "Tier", "Value": "prod" }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "AppRef": { "Value": { "Ref": "EbApplication" } },
                    "EnvRef": { "Value": { "Ref": "EbEnvironment" } }
                }
            })
            .to_string()
        },
        bundle: None,
        declared_environment: Some("orders-env"),
        declared_application: Some("orders-app"),
        expected_tier: &[("Name", "WebServer"), ("Type", "Standard")],
        expected_option: Some(("aws:autoscaling:asg", "MinSize", "2")),
        expected_tags: &[("Owner", "orders"), ("Tier", "prod")],
        by_platform_arn: false,
    },
    Row {
        case: "an unnamed application, version and environment on a platform ARN, in YAML",
        region: "eu-west-1",
        template: |platform| {
            [
                "Resources:",
                "  EbApplication:",
                "    Type: AWS::ElasticBeanstalk::Application",
                "  EbVersion:",
                "    Type: AWS::ElasticBeanstalk::ApplicationVersion",
                "    Properties:",
                "      ApplicationName: !Ref EbApplication",
                "      Description: the bundle this stack deploys",
                "      SourceBundle:",
                "        S3Bucket: eb-bundles",
                "        S3Key: app.zip",
                "  EbEnvironment:",
                "    Type: AWS::ElasticBeanstalk::Environment",
                "    Properties:",
                "      ApplicationName: !Ref EbApplication",
                "      VersionLabel: !Ref EbVersion",
                &format!("      PlatformArn: {platform}"),
                "      CNAMEPrefix: cfn-yaml-env",
                "Outputs:",
                "  AppRef:",
                "    Value: !Ref EbApplication",
                "  EnvRef:",
                "    Value: !Ref EbEnvironment",
                "  VersionRef:",
                "    Value: !Ref EbVersion",
                "",
            ]
            .join("\n")
        },
        bundle: Some(("eb-bundles", "app.zip")),
        declared_environment: None,
        declared_application: None,
        // A create that names no tier gets the documented default: a Standard-mode
        // EC2-based web server.
        expected_tier: &[("Name", "WebServer"), ("Type", "Standard")],
        expected_option: None,
        expected_tags: &[],
        by_platform_arn: true,
    },
    Row {
        case: "a worker-tier environment, named, on a third platform, in JSON",
        region: "us-east-1",
        template: |platform| {
            json!({
                "Resources": {
                    "EbApplication": {
                        "Type": "AWS::ElasticBeanstalk::Application",
                        "Properties": { "ApplicationName": "batch-app" }
                    },
                    "EbEnvironment": {
                        "Type": "AWS::ElasticBeanstalk::Environment",
                        "Properties": {
                            "ApplicationName": { "Ref": "EbApplication" },
                            "EnvironmentName": "batch-worker-env",
                            "PlatformArn": platform,
                            "Tier": { "Name": "Worker", "Type": "SQS/HTTP" },
                            "OptionSettings": [{
                                "Namespace": "aws:elasticbeanstalk:sqsd",
                                "OptionName": "HttpPath",
                                "Value": "/work"
                            }],
                            "Tags": [{ "Key": "Owner", "Value": "batch" }]
                        }
                    }
                },
                "Outputs": {
                    "AppRef": { "Value": { "Ref": "EbApplication" } },
                    "EnvRef": { "Value": { "Ref": "EbEnvironment" } }
                }
            })
            .to_string()
        },
        bundle: None,
        declared_environment: Some("batch-worker-env"),
        declared_application: Some("batch-app"),
        expected_tier: &[("Name", "Worker"), ("Type", "SQS/HTTP")],
        expected_option: Some(("aws:elasticbeanstalk:sqsd", "HttpPath", "/work")),
        expected_tags: &[("Owner", "batch")],
        by_platform_arn: true,
    },
];

/// Every row: the stack creates the application and the environment its template declares
/// in Elastic Beanstalk, `DescribeEnvironments` for the declared name then reports exactly
/// one environment, that environment carries the configuration and the tags the
/// declaration asked for, and deleting the stack removes both from Elastic Beanstalk.
#[tokio::test]
async fn a_stack_provisions_the_elastic_beanstalk_resources_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("eb-stack-{index}");
        let platform = match index {
            2 => worker_platform_arn(region),
            _ => second_platform_arn(region),
        };
        let template = (row.template)(&platform);
        if let Some((bucket, key)) = row.bundle {
            put_object(&router, region, bucket, key).await;
        }

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let outputs = outputs_of(&described);
        let application = outputs
            .get("AppRef")
            .unwrap_or_else(|| panic!("case {case:?}: no AppRef output in {described}"))
            .clone();
        let environment = outputs
            .get("EnvRef")
            .unwrap_or_else(|| panic!("case {case:?}: no EnvRef output in {described}"))
            .clone();
        match row.declared_application {
            Some(declared) => assert_eq!(
                application, declared,
                "case {case:?}: expected the application the template named"
            ),
            None => assert!(
                (HUNDRED_CHARACTER_BOUNDS.0..=HUNDRED_CHARACTER_BOUNDS.1)
                    .contains(&application.chars().count()),
                "case {case:?}: a generated application name must be inside the modelled \
                 bounds, got {application:?}"
            ),
        }
        match row.declared_environment {
            Some(declared) => assert_eq!(
                environment, declared,
                "case {case:?}: expected the environment the template named"
            ),
            None => assert!(
                (ENVIRONMENT_NAME_BOUNDS.0..=ENVIRONMENT_NAME_BOUNDS.1)
                    .contains(&environment.chars().count()),
                "case {case:?}: a generated environment name must be inside the modelled \
                 bounds, got {environment:?}"
            ),
        }

        // THE defect this suite is about: the environment the stack reported as created
        // is one Elastic Beanstalk really holds, and exactly one.
        let (status, environments) = elasticbeanstalk(
            &router,
            region,
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", &environment)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeEnvironments, got {status} with body \
             {environments}"
        );
        assert_eq!(
            environment_count(&environments),
            1,
            "case {case:?}: expected exactly one environment named {environment}, got \
             {environments}"
        );
        assert_eq!(
            opt_element(&environments, "ApplicationName").as_deref(),
            Some(application.as_str()),
            "case {case:?}: the environment must belong to the application the stack created, \
             got {environments}"
        );
        for (member, expected) in row.expected_tier {
            let tier = element(&environments, "Tier");
            assert_eq!(
                opt_element(&tier, member).as_deref(),
                Some(*expected),
                "case {case:?}: expected Tier.{member} to be {expected:?}, got {tier}"
            );
        }
        if row.by_platform_arn {
            assert_eq!(
                opt_element(&environments, "PlatformArn").as_deref(),
                Some(platform.as_str()),
                "case {case:?}: the environment must run the platform version the template \
                 named, got {environments}"
            );
        } else {
            assert_eq!(
                opt_element(&environments, "SolutionStackName"),
                Some(first_solution_stack()),
                "case {case:?}: the environment must run the solution stack the template \
                 named, got {environments}"
            );
        }
        let environment_arn = element(&environments, "EnvironmentArn");

        // The option setting the declaration carried reached the service's own
        // configuration for the environment.
        if let Some((namespace, option, value)) = row.expected_option {
            let (status, settings) = elasticbeanstalk(
                &router,
                region,
                "DescribeConfigurationSettings",
                &[
                    ("ApplicationName", &application),
                    ("EnvironmentName", &environment),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeConfigurationSettings, got \
                 {status} with body {settings}"
            );
            let carried = members(&settings).iter().any(|setting| {
                opt_element(setting, "Namespace").as_deref() == Some(namespace)
                    && opt_element(setting, "OptionName").as_deref() == Some(option)
                    && opt_element(setting, "Value").as_deref() == Some(value)
            });
            assert!(
                carried,
                "case {case:?}: expected the declared option {namespace}/{option} = {value:?} in \
                 the environment's configuration, got {settings}"
            );
        }

        // The tags the declaration asked for, plus the three CloudFormation records on
        // every resource it provisions, read through Elastic Beanstalk's own tagging call.
        let (status, listed) = elasticbeanstalk(
            &router,
            region,
            "ListTagsForResource",
            &[("ResourceArn", &environment_arn)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with body \
             {listed}"
        );
        let reported: BTreeMap<String, String> = members(&listed)
            .into_iter()
            .filter_map(|tag| Some((opt_element(tag, "Key")?, opt_element(tag, "Value")?)))
            .collect();
        let stack_id = element(&described, "StackId");
        for (key, value) in row
            .expected_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .chain([
                (
                    "aws:cloudformation:stack-name".to_string(),
                    stack_name.clone(),
                ),
                ("aws:cloudformation:stack-id".to_string(), stack_id),
                (
                    "aws:cloudformation:logical-id".to_string(),
                    "EbEnvironment".to_string(),
                ),
            ])
        {
            assert_eq!(
                reported.get(&key),
                Some(&value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported:?}"
            );
        }

        // An application version the row declared is one Elastic Beanstalk holds too, under
        // the label CloudFormation generated for it.
        if let Some(version) = outputs.get("VersionRef") {
            assert!(
                (HUNDRED_CHARACTER_BOUNDS.0..=HUNDRED_CHARACTER_BOUNDS.1)
                    .contains(&version.chars().count()),
                "case {case:?}: a generated version label must be inside the modelled bounds, \
                 got {version:?}"
            );
            let (status, versions) = elasticbeanstalk(
                &router,
                region,
                "DescribeApplicationVersions",
                &[
                    ("ApplicationName", &application),
                    ("VersionLabels.member.1", version),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeApplicationVersions, got {status} \
                 with body {versions}"
            );
            assert_eq!(
                opt_element(&versions, "VersionLabel").as_deref(),
                Some(version.as_str()),
                "case {case:?}: Elastic Beanstalk must hold the version the stack created, got \
                 {versions}"
            );
            assert_eq!(
                opt_element(&versions, "S3Key").as_deref(),
                row.bundle.map(|(_, key)| key),
                "case {case:?}: the version must carry the source bundle the template declared, \
                 got {versions}"
            );
            // And the environment runs it, because the declaration pointed at it.
            assert_eq!(
                opt_element(&environments, "VersionLabel").as_deref(),
                Some(version.as_str()),
                "case {case:?}: the environment must run the declared version, got \
                 {environments}"
            );
        }

        // Deleting the stack removes from Elastic Beanstalk what the stack created there.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        // Elastic Beanstalk deletes an application asynchronously ("the call returns at
        // once and the application is still described while the deletion runs"), so the
        // span that deletion is owed in has to come due before it is gone.
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (_, gone) = elasticbeanstalk(
            &router,
            region,
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", &environment)],
        )
        .await;
        assert_eq!(
            environment_count(&gone),
            0,
            "case {case:?}: the stack's deletion must terminate the environment, got {gone}"
        );
        let (_, applications) = elasticbeanstalk(
            &router,
            region,
            "DescribeApplications",
            &[("ApplicationNames.member.1", &application)],
        )
        .await;
        assert_eq!(
            opt_element(&applications, "ApplicationName"),
            None,
            "case {case:?}: the stack's deletion must delete the application, got {applications}"
        );
    }
}

/// A stack whose environment Elastic Beanstalk REFUSES does not report a resource nothing
/// created: the stack FAILS with that refusal, and no environment exists under the name the
/// template declared.
///
/// The rows are two refusals of the owning service's own validation — a `PlatformArn` the
/// catalog does not publish, and an environment whose declaration names no platform at all
/// — asked for with `OnFailure=DO_NOTHING`, which "if the stack creation fails ... the
/// stack status becomes CREATE_FAILED" and leaves what the create had made for the
/// assertion below.
#[tokio::test]
async fn an_environment_elastic_beanstalk_refuses_fails_the_stack() {
    let router = build_router();
    let region = "us-east-2";

    // (case, the environment's own properties, the environment name declared)
    let rows: &[(&str, Value, &str)] = &[
        (
            "a platform ARN the catalog does not publish",
            json!({
                "ApplicationName": { "Ref": "EbApplication" },
                "EnvironmentName": "ghost-platform-env",
                "PlatformArn": "arn:aws:elasticbeanstalk:us-east-2::platform/No Such Branch/9.9.9"
            }),
            "ghost-platform-env",
        ),
        (
            "an environment naming no platform at all",
            json!({
                "ApplicationName": { "Ref": "EbApplication" },
                "EnvironmentName": "platformless-env"
            }),
            "platformless-env",
        ),
    ];

    for (index, (case, properties, environment)) in rows.iter().enumerate() {
        let stack_name = format!("eb-refused-{index}");
        let template = json!({
            "Resources": {
                "EbApplication": {
                    "Type": "AWS::ElasticBeanstalk::Application",
                    "Properties": { "ApplicationName": format!("refused-app-{index}") }
                },
                "EbEnvironment": {
                    "Type": "AWS::ElasticBeanstalk::Environment",
                    "Properties": properties
                }
            }
        })
        .to_string();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                ("OnFailure", "DO_NOTHING"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;

        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_FAILED"),
            "case {case:?}: a create the owning service refused must fail the stack, got \
             {described}"
        );
        let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            reason.contains("EbEnvironment"),
            "case {case:?}: expected the failure to name the resource, got {reason:?}"
        );

        let (_, environments) = elasticbeanstalk(
            &router,
            region,
            "DescribeEnvironments",
            &[("EnvironmentNames.member.1", environment)],
        )
        .await;
        assert_eq!(
            environment_count(&environments),
            0,
            "case {case:?}: no environment may exist under {environment}, got {environments}"
        );
    }
}
