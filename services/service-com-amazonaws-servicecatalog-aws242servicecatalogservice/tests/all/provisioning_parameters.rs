// SPDX-License-Identifier: CC0-1.0

//! `DescribeProvisioningParameters` over the wire, driven as awsJson1_1 requests through a
//! `MultiServiceRouter` carrying Service Catalog beside the services a launch's
//! configuration is read out of.
//!
//! The rule under test: the operation reports the configuration of ONE launch — a product,
//! the path it is launched through, and the version to launch — computed from stored state:
//! the parameters the version's CloudFormation template declares (the template the version
//! NAMES, read by the service that owns it, whether the version names it by URL or by the
//! stack that contains it), the constraints stored on the portfolio-product pair that path
//! is, the TagOptions associated with those two resources grouped by key, and the accounts
//! and Regions a STACKSET constraint governs the launch with.
//!
//! The rows differ along what the rule generalises over: the SPELLING of the staged template
//! (JSON and YAML), the ADDRESSING of it (path-style and virtual-hosted, and a stack rather
//! than an object), the SHAPE of a parameter declaration (a default, allowed values, a
//! no-echo value, a numeric type, none at all), which resource an offered TagOption is
//! associated with, whether the two resources agree about a key, and how each resource of
//! the launch is addressed (by id and by name).

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    associate_product_with_portfolio, call, constraint_parameters, create_constraint,
    create_portfolio, create_product_with_info, create_template_bucket, emulator_with_template_services,
    error_code, message, stage_template, DEFAULT_REGION, TEMPLATE_BUCKET,
};

/// The product type every product here is created as: the one whose versions are
/// CloudFormation templates, which is what a provisioning parameter is declared in.
const CFN_PRODUCT: &str = "CLOUD_FORMATION_TEMPLATE";

/// The Amazon S3 addressing styles a template URL may spell, each naming the SAME object.
///
/// Source: Amazon S3's documented request styles — virtual-hosted
/// (`https://<bucket>.s3.<region>.amazonaws.com/<key>`) and path-style
/// (`https://s3.<region>.amazonaws.com/<bucket>/<key>`) — which is what
/// `ProvisioningArtifactProperties$Info` takes as "the URL of the CloudFormation template
/// in Amazon S3".
#[derive(Clone, Copy, Debug)]
enum Style {
    VirtualHosted,
    PathStyle,
}

impl Style {
    /// The URL this style spells for one object of the staging bucket.
    fn url(self, key: &str) -> String {
        match self {
            Style::VirtualHosted => {
                format!("https://{TEMPLATE_BUCKET}.s3.{DEFAULT_REGION}.amazonaws.com/{key}")
            }
            Style::PathStyle => {
                format!("https://s3.{DEFAULT_REGION}.amazonaws.com/{TEMPLATE_BUCKET}/{key}")
            }
        }
    }
}

/// What one `ProvisioningArtifactParameter` of the answer must carry.
///
/// Every member is the one the parameter's own DECLARATION states, so a row cannot pass
/// against a handler that reports a parameter it did not read: a key, a type, a default, a
/// description and the allowed values come from the staged template and from nowhere else.
#[derive(Debug)]
struct ExpectedParameter {
    key: &'static str,
    default_value: Option<&'static str>,
    parameter_type: &'static str,
    is_no_echo: bool,
    description: Option<&'static str>,
    /// The values the declaration permits — the one constraint a template summary reports.
    allowed_values: &'static [&'static str],
}

/// One row of the staged-template table: a template, how it is staged and addressed, and
/// the parameters the launch must report for the product created from it.
struct StagedTemplate {
    case: &'static str,
    key: &'static str,
    template: String,
    /// `None` for the row whose template is never staged, so the URL names no object.
    style: Option<Style>,
    product: &'static str,
    expected: &'static [ExpectedParameter],
}

/// A JSON template declaring four parameters whose declarations differ in shape, and one
/// resource so the body is a deployable template.
fn json_template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "A staged product template",
        "Parameters": {
            "BucketName": {
                "Type": "String",
                "Default": "launch-data",
                "Description": "The bucket the launch creates."
            },
            "Environment": {
                "Type": "String",
                "AllowedValues": ["dev", "prod"]
            },
            "AdminPassword": {
                "Type": "String",
                "NoEcho": true,
                "Description": "Not echoed back."
            },
            "RetentionDays": {
                "Type": "Number",
                "Default": "7"
            }
        },
        "Resources": {
            "Data": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": {"Ref": "BucketName"}}
            }
        }
    })
    .to_string()
}

/// The parameters [`json_template`] declares, as the launch must report them.
const JSON_PARAMETERS: &[ExpectedParameter] = &[
    ExpectedParameter {
        key: "BucketName",
        default_value: Some("launch-data"),
        parameter_type: "String",
        is_no_echo: false,
        description: Some("The bucket the launch creates."),
        allowed_values: &[],
    },
    ExpectedParameter {
        key: "Environment",
        default_value: None,
        parameter_type: "String",
        is_no_echo: false,
        description: None,
        allowed_values: &["dev", "prod"],
    },
    ExpectedParameter {
        key: "AdminPassword",
        default_value: None,
        parameter_type: "String",
        is_no_echo: true,
        description: Some("Not echoed back."),
        allowed_values: &[],
    },
    ExpectedParameter {
        key: "RetentionDays",
        default_value: Some("7"),
        parameter_type: "Number",
        is_no_echo: false,
        description: None,
        allowed_values: &[],
    },
];

/// The same kind of template in the YAML spelling CloudFormation also reads, declaring one
/// parameter — so the rows differ in the SPELLING of the body as well as in its content.
fn yaml_template() -> String {
    "AWSTemplateFormatVersion: '2010-09-09'\nParameters:\n  Tier:\n    Type: String\n    \
     Default: standard\n    Description: Which tier to launch.\n    AllowedValues:\n      - \
     standard\n      - premium\nResources:\n  Data:\n    Type: AWS::S3::Bucket\n"
        .to_string()
}

/// The parameter [`yaml_template`] declares.
const YAML_PARAMETERS: &[ExpectedParameter] = &[ExpectedParameter {
    key: "Tier",
    default_value: Some("standard"),
    parameter_type: "String",
    is_no_echo: false,
    description: Some("Which tier to launch."),
    allowed_values: &["standard", "premium"],
}];

/// A template declaring NO `Parameters` section: a launch of it takes no parameter, which
/// is itself what an end user reads before provisioning.
fn parameterless_template() -> String {
    json!({
        "Resources": {"Data": {"Type": "AWS::S3::Bucket"}}
    })
    .to_string()
}

fn staged_templates() -> Vec<StagedTemplate> {
    vec![
        StagedTemplate {
            case: "a JSON template addressed path-style",
            key: "products/json/template.json",
            template: json_template(),
            style: Some(Style::PathStyle),
            product: "staged-json-path",
            expected: JSON_PARAMETERS,
        },
        StagedTemplate {
            case: "the same JSON template addressed virtual-hosted",
            key: "flat.json",
            template: json_template(),
            style: Some(Style::VirtualHosted),
            product: "staged-json-virtual",
            expected: JSON_PARAMETERS,
        },
        StagedTemplate {
            case: "a YAML template addressed virtual-hosted",
            key: "products/yaml/template.yaml",
            template: yaml_template(),
            style: Some(Style::VirtualHosted),
            product: "staged-yaml",
            expected: YAML_PARAMETERS,
        },
        StagedTemplate {
            case: "a template declaring no Parameters section",
            key: "parameterless.json",
            template: parameterless_template(),
            style: Some(Style::PathStyle),
            product: "staged-parameterless",
            expected: &[],
        },
        StagedTemplate {
            case: "a URL naming no staged object",
            key: "never-staged.json",
            template: json_template(),
            style: None,
            product: "unstaged",
            expected: &[],
        },
    ]
}

/// `case` as an `IdempotencyToken`: the modelled pattern is
/// `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`, which a prose case name is not.
fn token(case: &str) -> String {
    case.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The one provisioning artifact a `CreateProduct` minted for `product_id`.
async fn only_version(router: &MultiServiceRouter, product_id: &str) -> String {
    let (status, listed) = call(
        router,
        "ListProvisioningArtifacts",
        &json!({"ProductId": product_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListProvisioningArtifacts {product_id}: expected 200, got {status} with {listed}"
    );
    listed["ProvisioningArtifactDetails"][0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("ListProvisioningArtifacts {product_id}: no version in {listed}"))
        .to_string()
}

/// `DescribeProvisioningParameters`, answered with the status and the body.
async fn describe_parameters(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "DescribeProvisioningParameters", request).await
}

/// The answer's `ProvisioningArtifactParameters`, by `ParameterKey`.
fn parameters_by_key(case: &str, body: &Value) -> BTreeMap<String, Value> {
    let listed = body["ProvisioningArtifactParameters"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a ProvisioningArtifactParameters list, got {body}")
        });
    listed
        .iter()
        .map(|parameter| {
            let key = parameter["ParameterKey"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a parameter has no key: {parameter}"));
            (key.to_string(), parameter.clone())
        })
        .collect()
}

/// A string list of the answer, as a `Vec<String>`.
fn string_list(case: &str, member: &str, value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} to be a list, got {value}"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: {member} carries {entry}, not a string"))
                .to_string()
        })
        .collect()
}

/// Assert that one launch reports exactly the parameters `expected` states.
fn assert_parameters(case: &str, body: &Value, expected: &[ExpectedParameter]) {
    let reported = parameters_by_key(case, body);
    let reported_keys: Vec<&String> = reported.keys().collect();
    let mut expected_keys: Vec<&str> = expected.iter().map(|parameter| parameter.key).collect();
    expected_keys.sort_unstable();
    assert_eq!(
        reported_keys
            .iter()
            .map(|key| key.as_str())
            .collect::<Vec<&str>>(),
        expected_keys,
        "case {case:?}: expected the declared parameters {expected_keys:?}, got {reported_keys:?}"
    );
    for parameter in expected {
        let actual = &reported[parameter.key];
        assert_eq!(
            actual["DefaultValue"].as_str(),
            parameter.default_value,
            "case {case:?}: parameter {}: expected default {:?}, got {actual}",
            parameter.key,
            parameter.default_value
        );
        assert_eq!(
            actual["ParameterType"].as_str(),
            Some(parameter.parameter_type),
            "case {case:?}: parameter {}: expected type {:?}, got {actual}",
            parameter.key,
            parameter.parameter_type
        );
        assert_eq!(
            actual["IsNoEcho"].as_bool(),
            Some(parameter.is_no_echo),
            "case {case:?}: parameter {}: expected IsNoEcho {}, got {actual}",
            parameter.key,
            parameter.is_no_echo
        );
        assert_eq!(
            actual["Description"].as_str(),
            parameter.description,
            "case {case:?}: parameter {}: expected description {:?}, got {actual}",
            parameter.key,
            parameter.description
        );
        let allowed = string_list(
            case,
            "AllowedValues",
            &actual["ParameterConstraints"]["AllowedValues"],
        );
        assert_eq!(
            allowed,
            parameter
                .allowed_values
                .iter()
                .map(|value| (*value).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: parameter {}: expected allowed values {:?}, got {allowed:?}",
            parameter.key,
            parameter.allowed_values
        );
    }
}

/// The parameters a launch reports are the ones the version's own template declares,
/// whichever spelling the body is written in and whichever style the version addresses it
/// by — and a version whose URL names no object declares none.
#[tokio::test]
async fn the_parameters_reported_are_the_ones_the_versions_template_declares() {
    let router = emulator_with_template_services();
    create_template_bucket(&router).await;
    let portfolio = create_portfolio(&router, "staged templates", "acme", &[], "staged").await;

    for row in staged_templates() {
        let case = row.case;
        let url = match row.style {
            Some(style) => {
                stage_template(&router, row.key, &row.template).await;
                style.url(row.key)
            }
            // Never staged: the URL is well formed and the object is not there.
            None => Style::PathStyle.url(row.key),
        };
        let product = create_product_with_info(
            &router,
            row.product,
            CFN_PRODUCT,
            &json!({"LoadTemplateFromURL": url}),
            &token(row.product),
        )
        .await;
        associate_product_with_portfolio(&router, &product, &portfolio).await;
        let version = only_version(&router, &product).await;

        let (status, body) = describe_parameters(
            &router,
            &json!({"ProductId": product, "ProvisioningArtifactId": version}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_parameters(case, &body, row.expected);
        // Whatever the template says, the rest of the launch's configuration is this
        // service's own: a product with no constraint on its path reports none, and the
        // metadata nothing stores is an empty list rather than an absent member.
        assert_eq!(
            body["ConstraintSummaries"],
            json!([]),
            "case {case:?}: expected no constraint on an unconstrained launch, got {body}"
        );
        assert_eq!(
            body["UsageInstructions"],
            json!([]),
            "case {case:?}: expected no usage instruction, got {body}"
        );
        assert!(
            body.get("ProvisioningArtifactPreferences").is_none(),
            "case {case:?}: expected no preferences without a STACKSET constraint, got {body}"
        );
    }
}

/// A version may name its template by the stack that CONTAINS it
/// (`ImportFromPhysicalId`) rather than by a URL, and the launch then reports the
/// parameters THAT stack's template declares.
#[tokio::test]
async fn a_version_naming_a_stack_reports_the_parameters_that_stacks_template_declares() {
    let router = emulator_with_template_services();
    let portfolio = create_portfolio(&router, "imported templates", "acme", &[], "imported").await;

    // The stack whose template the version is: created through CloudFormation's own wire,
    // so the template the launch reports on is one that service holds.
    let mut form = ::form_urlencoded::Serializer::new(String::new());
    form.append_pair("Action", "CreateStack");
    form.append_pair("Version", "2010-05-15");
    form.append_pair("StackName", "product-source-stack");
    form.append_pair("TemplateBody", &yaml_template());
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://cloudformation.{DEFAULT_REGION}.amazonaws.com/"
        ))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTHARNESS/20260101/{DEFAULT_REGION}/\
                 cloudformation/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(form.finish()))
        .expect("a well-formed CloudFormation request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let created = String::from_utf8_lossy(&response.into_body()).to_string();
    assert_eq!(
        status, 200,
        "the source stack must be created first, got {status} with {created}"
    );
    let stack_id = created
        .split("<StackId>")
        .nth(1)
        .and_then(|rest| rest.split("</StackId>").next())
        .unwrap_or_else(|| panic!("no StackId in {created}"))
        .to_string();

    let product = create_product_with_info(
        &router,
        "imported-from-stack",
        CFN_PRODUCT,
        &json!({"ImportFromPhysicalId": stack_id}),
        "imported-from-stack",
    )
    .await;
    associate_product_with_portfolio(&router, &product, &portfolio).await;
    let version = only_version(&router, &product).await;

    let (status, body) = describe_parameters(
        &router,
        &json!({"ProductId": product, "ProvisioningArtifactId": version}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a version naming a stack\": expected 200, got {status} with {body}"
    );
    assert_parameters("a version naming a stack", &body, YAML_PARAMETERS);
}

/// One TagOption seed: its key, its value, which resources of the launch carry it, and
/// whether it is deactivated before it is associated.
struct TagOptionSeed {
    case: &'static str,
    key: &'static str,
    value: &'static str,
    on_portfolio: bool,
    on_product: bool,
    active: bool,
}

/// The seeded TagOptions: a key only the portfolio carries, a key only the product
/// carries, a key both carry with the SAME value, a key each carries with a DIFFERENT
/// value (the conflict this operation's documentation describes), and a deactivated one.
const TAG_OPTION_SEEDS: &[TagOptionSeed] = &[
    TagOptionSeed {
        case: "a key only the portfolio carries",
        key: "portfolio.only",
        value: "shared-services",
        on_portfolio: true,
        on_product: false,
        active: true,
    },
    TagOptionSeed {
        case: "a key only the product carries",
        key: "product.only",
        value: "web",
        on_portfolio: false,
        on_product: true,
        active: true,
    },
    TagOptionSeed {
        case: "a value both resources carry",
        key: "env",
        value: "prod",
        on_portfolio: true,
        on_product: true,
        active: true,
    },
    TagOptionSeed {
        case: "a value only the portfolio carries under a shared key",
        key: "costcenter",
        value: "platform",
        on_portfolio: true,
        on_product: false,
        active: true,
    },
    TagOptionSeed {
        case: "a value only the product carries under that shared key",
        key: "costcenter",
        value: "web",
        on_portfolio: false,
        on_product: true,
        active: true,
    },
    TagOptionSeed {
        case: "a deactivated TagOption",
        key: "retired",
        value: "yes",
        on_portfolio: true,
        on_product: true,
        active: false,
    },
];

/// Create one TagOption and answer the id the service minted.
async fn create_tag_option(router: &MultiServiceRouter, key: &str, value: &str) -> String {
    let (status, created) =
        call(router, "CreateTagOption", &json!({"Key": key, "Value": value})).await;
    assert_eq!(
        status, 200,
        "seed CreateTagOption({key:?}, {value:?}): expected 200, got {status} with {created}"
    );
    created["TagOptionDetail"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("seed CreateTagOption({key:?}): no Id in {created}"))
        .to_string()
}

/// Associate one TagOption with one resource.
async fn associate_tag_option(router: &MultiServiceRouter, resource_id: &str, tag_option: &str) {
    let (status, body) = call(
        router,
        "AssociateTagOptionWithResource",
        &json!({"ResourceId": resource_id, "TagOptionId": tag_option}),
    )
    .await;
    assert_eq!(
        status, 200,
        "seed AssociateTagOptionWithResource({resource_id}, {tag_option}): expected 200, got \
         {status} with {body}"
    );
}

/// The answer's `TagOptions`, as key -> values.
fn tag_options_by_key(case: &str, body: &Value) -> BTreeMap<String, Vec<String>> {
    body["TagOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a TagOptions list, got {body}"))
        .iter()
        .map(|summary| {
            let key = summary["Key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a TagOption summary has no key: {body}"));
            (
                key.to_string(),
                string_list(case, "Values", &summary["Values"]),
            )
        })
        .collect()
}

/// The TagOptions a launch offers are the ACTIVE ones associated with the product and with
/// the portfolio the path is, one entry per key — and a key the two resources carry
/// different values for offers none, which is the conflict this operation documents.
#[tokio::test]
async fn the_tag_options_offered_are_the_ones_the_launchs_resources_carry() {
    let router = emulator_with_template_services();
    let portfolio = create_portfolio(&router, "tagged portfolio", "acme", &[], "tagged").await;
    let product = create_product_with_info(
        &router,
        "tagged-product",
        CFN_PRODUCT,
        &json!({"LoadTemplateFromURL": "https://example.invalid/template.json"}),
        "tagged-product",
    )
    .await;
    associate_product_with_portfolio(&router, &product, &portfolio).await;
    let version = only_version(&router, &product).await;

    for seed in TAG_OPTION_SEEDS {
        let id = create_tag_option(&router, seed.key, seed.value).await;
        if seed.on_portfolio {
            associate_tag_option(&router, &portfolio, &id).await;
        }
        if seed.on_product {
            associate_tag_option(&router, &product, &id).await;
        }
        if !seed.active {
            let (status, updated) =
                call(&router, "UpdateTagOption", &json!({"Id": id, "Active": false})).await;
            assert_eq!(
                status, 200,
                "seed UpdateTagOption({id}) for {:?}: expected 200, got {status} with {updated}",
                seed.case
            );
        }
    }

    let (status, body) = describe_parameters(
        &router,
        &json!({"ProductId": product, "ProvisioningArtifactId": version}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the launch to be described, got {status} with {body}"
    );
    let offered = tag_options_by_key("the seeded TagOptions", &body);
    let expected: BTreeMap<String, Vec<String>> = [
        ("portfolio.only", vec!["shared-services"]),
        ("product.only", vec!["web"]),
        // Both resources carry this key with the one value, which they therefore agree on.
        ("env", vec!["prod"]),
        // Each carries a DIFFERENT value under this key, so they agree on none: the
        // TagOption conflict the operation's documentation describes.
        ("costcenter", vec![]),
    ]
    .into_iter()
    .map(|(key, values)| {
        (
            key.to_string(),
            values.into_iter().map(str::to_string).collect(),
        )
    })
    .collect();
    assert_eq!(
        offered, expected,
        "expected the launch to offer {expected:?}, got {offered:?}"
    );
    for seed in TAG_OPTION_SEEDS.iter().filter(|seed| !seed.active) {
        assert!(
            !offered.contains_key(seed.key),
            "case {:?}: expected a deactivated TagOption not to be offered, got {offered:?}",
            seed.case
        );
    }
}

/// The constraints a launch reports are the ones stored on the portfolio-product pair its
/// path is, and a STACKSET constraint among them is where the launch's accounts and Regions
/// come from.
#[tokio::test]
async fn the_constraints_reported_are_the_ones_stored_on_the_launchs_path() {
    let router = emulator_with_template_services();
    let portfolio = create_portfolio(&router, "constrained", "acme", &[], "constrained").await;
    let other_portfolio = create_portfolio(&router, "unrelated", "acme", &[], "unrelated").await;
    let product = create_product_with_info(
        &router,
        "constrained-product",
        CFN_PRODUCT,
        &json!({"LoadTemplateFromURL": "https://example.invalid/template.json"}),
        "constrained-product",
    )
    .await;
    let other_product = create_product_with_info(
        &router,
        "unconstrained-product",
        CFN_PRODUCT,
        &json!({"LoadTemplateFromURL": "https://example.invalid/template.json"}),
        "unconstrained-product",
    )
    .await;
    associate_product_with_portfolio(&router, &product, &portfolio).await;
    associate_product_with_portfolio(&router, &other_product, &other_portfolio).await;
    let version = only_version(&router, &product).await;
    let other_version = only_version(&router, &other_product).await;

    // Three constraints on the launch's own pair, and one on another pair entirely, which
    // governs no launch of this product.
    for (constraint_type, description) in [
        ("LAUNCH", "launch with the catalog role"),
        ("STACKSET", "deploy as a stack set"),
        ("TEMPLATE", "the template rules"),
    ] {
        create_constraint(
            &router,
            &portfolio,
            &product,
            constraint_type,
            description,
            &token(&format!("{constraint_type}-on-launch")),
        )
        .await;
    }
    create_constraint(
        &router,
        &other_portfolio,
        &other_product,
        "NOTIFICATION",
        "another pair's constraint",
        "notification-elsewhere",
    )
    .await;

    let (status, body) = describe_parameters(
        &router,
        &json!({"ProductId": product, "ProvisioningArtifactId": version}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the constrained launch to be described, got {status} with {body}"
    );
    let mut reported: Vec<(String, String)> = body["ConstraintSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ConstraintSummaries list, got {body}"))
        .iter()
        .map(|summary| {
            (
                summary["Type"].as_str().unwrap_or_default().to_string(),
                summary["Description"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    reported.sort();
    let expected = vec![
        ("LAUNCH".to_string(), "launch with the catalog role".to_string()),
        ("STACKSET".to_string(), "deploy as a stack set".to_string()),
        ("TEMPLATE".to_string(), "the template rules".to_string()),
    ];
    assert_eq!(
        reported, expected,
        "case \"three constraints on the launch's pair\": expected {expected:?}, got {reported:?}"
    );

    // The STACKSET constraint's own `Parameters` are where the launch's accounts and
    // Regions come from: the same document `create_constraint` stored.
    let stored: Value = serde_json::from_str(&constraint_parameters("STACKSET"))
        .expect("the seeded STACKSET parameters are JSON");
    let preferences = &body["ProvisioningArtifactPreferences"];
    assert_eq!(
        preferences["StackSetAccounts"], stored["AccountList"],
        "case \"a STACKSET constraint\": expected the constraint's accounts, got {body}"
    );
    assert_eq!(
        preferences["StackSetRegions"], stored["RegionList"],
        "case \"a STACKSET constraint\": expected the constraint's Regions, got {body}"
    );

    // A launch of another product, in another portfolio, reports neither that pair's
    // constraints nor any preference: the constraints are the ones on ITS path.
    let (status, body) = describe_parameters(
        &router,
        &json!({"ProductId": other_product, "ProvisioningArtifactId": other_version}),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the other launch to be described, got {status} with {body}"
    );
    let reported = body["ConstraintSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a ConstraintSummaries list, got {body}"))
        .len();
    assert_eq!(
        reported, 1,
        "case \"another pair's own constraint\": expected only that pair's constraint, got {body}"
    );
    assert_eq!(
        body["ConstraintSummaries"][0]["Type"].as_str(),
        Some("NOTIFICATION"),
        "case \"another pair's own constraint\": expected its NOTIFICATION constraint, got {body}"
    );
    assert!(
        body.get("ProvisioningArtifactPreferences").is_none(),
        "case \"a launch no STACKSET constraint governs\": expected no preferences, got {body}"
    );
}

/// One refusal row: the request members, and the error the service must answer.
struct Refusal {
    case: &'static str,
    /// Built from the seeded ids, so no row carries a literal identifier.
    request: Box<dyn Fn(&Seed) -> Value>,
    code: &'static str,
    message: Box<dyn Fn(&Seed) -> String>,
}

/// What every refusal row is addressed against.
struct Seed {
    product: String,
    product_name: String,
    version: String,
    version_name: String,
    path: String,
    path_name: String,
    /// A product this account holds that sits in no portfolio, so it has no launch path.
    unreachable_product: String,
    /// A second portfolio the two-path product is in, so that product has no default path.
    second_path: String,
    two_path_product: String,
    two_path_version: String,
}

/// Addressing a launch by NAME reads the same launch as addressing it by id, and every
/// request that addresses one of its resources by neither, or by both, is refused with the
/// sentence that rule is stated in.
#[tokio::test]
async fn a_launch_is_addressed_by_id_or_by_name_and_refused_otherwise() {
    let router = emulator_with_template_services();
    create_template_bucket(&router).await;
    stage_template(&router, "refusals.json", &json_template()).await;
    let url = Style::PathStyle.url("refusals.json");

    let path = create_portfolio(&router, "addressed portfolio", "acme", &[], "addressed").await;
    let product = create_product_with_info(
        &router,
        "addressed-product",
        CFN_PRODUCT,
        &json!({"LoadTemplateFromURL": url}),
        "addressed-product",
    )
    .await;
    associate_product_with_portfolio(&router, &product, &path).await;
    let version = only_version(&router, &product).await;

    let unreachable_product = create_product_with_info(
        &router,
        "unreachable-product",
        CFN_PRODUCT,
        &json!({"LoadTemplateFromURL": url}),
        "unreachable-product",
    )
    .await;

    let second_path = create_portfolio(&router, "second portfolio", "acme", &[], "second").await;
    let two_path_product = create_product_with_info(
        &router,
        "two-path-product",
        CFN_PRODUCT,
        &json!({"LoadTemplateFromURL": url}),
        "two-path-product",
    )
    .await;
    associate_product_with_portfolio(&router, &two_path_product, &path).await;
    associate_product_with_portfolio(&router, &two_path_product, &second_path).await;
    let two_path_version = only_version(&router, &two_path_product).await;

    let seed = Seed {
        product,
        product_name: "addressed-product".to_string(),
        version,
        version_name: "v1".to_string(),
        path,
        path_name: "addressed portfolio".to_string(),
        unreachable_product,
        second_path,
        two_path_product,
        two_path_version,
    };

    // Every way of addressing the SAME launch answers the same parameters.
    let addressings: Vec<(&str, Value)> = vec![
        (
            "by every id",
            json!({
                "ProductId": seed.product,
                "ProvisioningArtifactId": seed.version,
                "PathId": seed.path,
            }),
        ),
        (
            "by every name",
            json!({
                "ProductName": seed.product_name,
                "ProvisioningArtifactName": seed.version_name,
                "PathName": seed.path_name,
            }),
        ),
        (
            "by the product's name and the version's id, with no path named",
            json!({
                "ProductName": seed.product_name,
                "ProvisioningArtifactId": seed.version,
            }),
        ),
    ];
    for (case, request) in addressings {
        let (status, body) = describe_parameters(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        assert_parameters(case, &body, JSON_PARAMETERS);
    }

    for row in refusals() {
        let case = row.case;
        let request = (row.request)(&seed);
        let (status, body) = describe_parameters(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request refused, got {status} with {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, row.code,
            "case {case:?}: expected {}, got {code} with {body}",
            row.code
        );
        let reported = message(&body);
        let expected = (row.message)(&seed);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// The refusal rows: one per way a request can name a launch that cannot be resolved.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            case: "the product addressed by both id and name",
            request: Box::new(|seed| {
                json!({
                    "ProductId": seed.product,
                    "ProductName": seed.product_name,
                    "ProvisioningArtifactId": seed.version,
                })
            }),
            code: "InvalidParametersException",
            message: Box::new(|_| {
                "ProductId and ProductName cannot both be provided: you must provide the name or \
                 ID, but not both."
                    .to_string()
            }),
        },
        Refusal {
            case: "the product addressed by neither id nor name",
            request: Box::new(|seed| json!({"ProvisioningArtifactId": seed.version})),
            code: "InvalidParametersException",
            message: Box::new(|_| "Either ProductId or ProductName must be provided.".to_string()),
        },
        Refusal {
            case: "the version addressed by neither id nor name",
            request: Box::new(|seed| json!({"ProductId": seed.product})),
            code: "InvalidParametersException",
            message: Box::new(|_| {
                "Either ProvisioningArtifactId or ProvisioningArtifactName must be provided."
                    .to_string()
            }),
        },
        Refusal {
            case: "the version addressed by both id and name",
            request: Box::new(|seed| {
                json!({
                    "ProductId": seed.product,
                    "ProvisioningArtifactId": seed.version,
                    "ProvisioningArtifactName": seed.version_name,
                })
            }),
            code: "InvalidParametersException",
            message: Box::new(|_| {
                "ProvisioningArtifactId and ProvisioningArtifactName cannot both be provided: you \
                 must provide the name or ID, but not both."
                    .to_string()
            }),
        },
        Refusal {
            case: "a path named for a product that sits in no portfolio",
            request: Box::new(|seed| {
                json!({
                    "ProductId": seed.unreachable_product,
                    "ProvisioningArtifactId": seed.version,
                    // A path is NAMED, and a product in no portfolio has no launch path
                    // for it to identify.
                    "PathId": seed.path,
                })
            }),
            code: "InvalidParametersException",
            message: Box::new(|seed| format!("Invalid PathId: {}", seed.path)),
        },
        Refusal {
            case: "a path named for a product id nothing stored carries",
            request: Box::new(|seed| {
                json!({
                    "ProductId": "prod-000000000000",
                    "ProvisioningArtifactId": seed.version,
                    "PathId": seed.path,
                })
            }),
            code: "InvalidParametersException",
            message: Box::new(|seed| format!("Invalid PathId: {}", seed.path)),
        },
        Refusal {
            case: "no path named for a product that sits in no portfolio",
            request: Box::new(|seed| {
                json!({
                    "ProductId": seed.unreachable_product,
                    "ProvisioningArtifactId": seed.version,
                })
            }),
            code: "InvalidParametersException",
            message: Box::new(|_| "Either Path Id or Path Name needs to be specified".to_string()),
        },
        Refusal {
            case: "no path named for a product id nothing stored carries",
            request: Box::new(|seed| {
                json!({
                    "ProductId": "prod-000000000000",
                    "ProvisioningArtifactId": seed.version,
                })
            }),
            code: "InvalidParametersException",
            message: Box::new(|_| "Either Path Id or Path Name needs to be specified".to_string()),
        },
        Refusal {
            case: "a path that is not one of the product's",
            request: Box::new(|seed| {
                json!({
                    "ProductId": seed.product,
                    "ProvisioningArtifactId": seed.version,
                    "PathId": seed.second_path,
                })
            }),
            code: "InvalidParametersException",
            message: Box::new(|seed| format!("Invalid PathId: {}", seed.second_path)),
        },
        Refusal {
            case: "a path NAME that is not one of the product's",
            request: Box::new(|seed| {
                json!({
                    "ProductId": seed.product,
                    "ProvisioningArtifactId": seed.version,
                    "PathName": "second portfolio",
                })
            }),
            code: "InvalidParametersException",
            message: Box::new(|_| "Invalid PathName: second portfolio".to_string()),
        },
        Refusal {
            case: "a version of another product",
            request: Box::new(|seed| {
                json!({
                    "ProductId": seed.product,
                    "ProvisioningArtifactId": seed.two_path_version,
                })
            }),
            code: "ResourceNotFoundException",
            message: Box::new(|seed| {
                format!("Provisioning artifact not found: {}", seed.two_path_version)
            }),
        },
        Refusal {
            case: "no path named for a product that has more than one",
            request: Box::new(|seed| {
                json!({
                    "ProductId": seed.two_path_product,
                    "ProvisioningArtifactId": seed.two_path_version,
                })
            }),
            code: "InvalidParametersException",
            message: Box::new(|_| "Either Path Id or Path Name needs to be specified".to_string()),
        },
    ]
}
