// SPDX-License-Identifier: CC0-1.0

//! `CreateProvisionedProductPlan` and `DescribeProvisionedProductPlan` over the wire,
//! driven as awsJson1_1 requests through a `MultiServiceRouter` carrying Service Catalog
//! beside the services a plan is computed out of.
//!
//! The rules under test:
//!
//! * creating a plan resolves the SAME launch triple a provisioning request resolves —
//!   the product, the launch path it is provisioned through and the product version —
//!   and refuses a request naming one that does not resolve before it stores anything;
//! * a plan is stored for exactly ONE provisioned product: for a name this account does
//!   not hold the plan brings that provisioned product into existence in
//!   `PLAN_IN_PROGRESS`, for one it holds the plan is refused unless that provisioned
//!   product is in a stable status, and a second plan for the same provisioned product
//!   is refused;
//! * a plan "includes the list of resources to be created ... when the plan is
//!   executed": that list is the change set CloudFormation computes over the product
//!   version's own template, and the plan's status is that change set's;
//! * every other member the create answers and the describe reports is read from the
//!   stored plan, and reading a plan changes nothing a later caller sees.
//!
//! The rows differ along what the rules generalise over: the resources the staged
//! template declares, the product type planned, how many launch paths the product has
//! and whether the request names one, the tags, the SNS topic ARNs and the provisioning
//! parameters.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    associate_product_with_portfolio, call, call_in, create_portfolio, create_product_with_info,
    create_template_bucket, emulator_with_template_services, error_code,
    first_provisioning_artifact_id, is_catalog_id, message, stage_template, staged_template_url,
    tag_list, tag_map,
};

/// The only `ProvisionedProductPlanType` the model declares
/// (`smithy_query query="shape" shape="ProvisionedProductPlanType"`).
const PLAN_TYPE: &str = "CLOUDFORMATION";

/// `ProvisionedProductPlanStatus`'s non-terminal creation value
/// (`smithy_query query="shape" shape="ProvisionedProductPlanStatus"`:
/// CREATE_IN_PROGRESS, CREATE_SUCCESS, CREATE_FAILED, EXECUTE_IN_PROGRESS,
/// EXECUTE_SUCCESS, EXECUTE_FAILED).
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// `ProvisionedProductPlanStatus`'s terminal value for a plan that was created.
const CREATE_SUCCESS: &str = "CREATE_SUCCESS";

/// The `ChangeAction` of a resource a plan would create.
///
/// A plan relays the change set CloudFormation computed, so the action carries
/// CloudFormation's own spelling of the value (`Add`) rather than the upper-case one
/// this service's `ChangeAction` enum names.
const ADD: &str = "Add";

/// The `ProvisionedProductStatus` of a provisioned product a plan created and nothing
/// has executed (`smithy_query query="shape" shape="ProvisionedProductStatus"`:
/// AVAILABLE, UNDER_CHANGE, TAINTED, ERROR, PLAN_IN_PROGRESS).
const PLAN_IN_PROGRESS: &str = "PLAN_IN_PROGRESS";

/// `ProvisionedProductStatus`'s stable value.
const AVAILABLE: &str = "AVAILABLE";

/// AWS's sentence for a request that names neither half of the launch-path pair when the
/// path cannot be inferred. All fixed text: it carries no value from the request.
const PATH_REQUIRED: &str = "Either Path Id or Path Name needs to be specified";

/// How long a poll keeps looking for a condition the emulator's clock owes before it
/// gives up. Far longer than any span this emulator settles on, so a loaded or slow host
/// cannot fail a case a fast one passes: the loop waits for the CONDITION, never for a
/// fixed time.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits between two looks.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A staged product template declaring ONE resource and no parameters: the simplest
/// launch a plan can plan.
fn one_resource_template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "A staged product template",
        "Resources": {"Data": {"Type": "AWS::S3::Bucket"}}
    })
    .to_string()
}

/// The resources [`one_resource_template`] declares, as the plan over it must list
/// them: (logical resource id, resource type).
const ONE_RESOURCE: &[(&str, &str)] = &[("Data", "AWS::S3::Bucket")];

/// A staged template declaring TWO resources and a parameter with a default, so the
/// plan over it lists more than one change and the launch takes a parameter.
fn two_resource_template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Parameters": {"BucketName": {"Type": "String", "Default": "planned-data"}},
        "Resources": {
            "Data": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": {"Ref": "BucketName"}}
            },
            "Events": {"Type": "AWS::SQS::Queue"}
        }
    })
    .to_string()
}

/// The resources [`two_resource_template`] declares, in logical-id order.
const TWO_RESOURCES: &[(&str, &str)] = &[("Data", "AWS::S3::Bucket"), ("Events", "AWS::SQS::Queue")];

/// One provisioning parameter of a request: key, value (absent when the row keeps the
/// previous one) and `UsePreviousValue`.
type Parameter = (&'static str, Option<&'static str>, bool);

/// One row of the plan table.
struct PlanCase {
    case: &'static str,
    /// The template the row's product version names, and the object key it is staged
    /// under.
    template: fn() -> String,
    template_key: &'static str,
    /// The resources that template declares, which the plan must list as changes.
    declared: &'static [(&'static str, &'static str)],
    /// The `ProductType` of the product the row plans a provisioning of.
    product_type: &'static str,
    /// How many portfolios the product sits in: one leaves a default launch path, two
    /// make `PathId` the only way to name one.
    portfolios: usize,
    /// Whether the request names `PathId` explicitly.
    names_path: bool,
    tags: &'static [(&'static str, &'static str)],
    notification_arns: &'static [&'static str],
    parameters: &'static [Parameter],
}

/// The launch one row plans a provisioning of: the product, its version, and the
/// portfolios it can be launched through, in the order they were associated.
struct Launch {
    product_id: String,
    artifact_id: String,
    portfolio_ids: Vec<String>,
}

/// Stage `template`, create one product of `product_type` whose only version names it,
/// put that product in `portfolios` portfolios and answer the launch that makes.
async fn seed_launch(
    router: &MultiServiceRouter,
    slug: &str,
    product_type: &str,
    template: &str,
    template_key: &str,
    portfolios: usize,
) -> Launch {
    stage_template(router, template_key, template).await;
    let product_id = create_product_with_info(
        router,
        &format!("{slug}-product"),
        product_type,
        // The accepted `Info` keys are the model's own:
        // `ProvisioningArtifactProperties$Info` documents
        // "Keys accepted: [ LoadTemplateFromURL, ImportFromPhysicalId ]".
        &json!({"LoadTemplateFromURL": staged_template_url(template_key)}),
        slug,
    )
    .await;
    let artifact_id = first_provisioning_artifact_id(router, &product_id).await;
    let mut portfolio_ids = Vec::new();
    for index in 0..portfolios {
        let portfolio_id = create_portfolio(
            router,
            &format!("{slug}-portfolio-{index}"),
            "acme",
            &[],
            &format!("{slug}-portfolio-{index}"),
        )
        .await;
        associate_product_with_portfolio(router, &product_id, &portfolio_id).await;
        portfolio_ids.push(portfolio_id);
    }
    Launch {
        product_id,
        artifact_id,
        portfolio_ids,
    }
}

/// Seed the one launch a suite that does not vary the template needs.
async fn seed_simple_launch(router: &MultiServiceRouter, slug: &str, portfolios: usize) -> Launch {
    seed_launch(
        router,
        slug,
        "CLOUD_FORMATION_TEMPLATE",
        &one_resource_template(),
        &format!("{slug}/template.json"),
        portfolios,
    )
    .await
}

/// `parameters` as the modelled `UpdateProvisioningParameters` list.
fn parameter_list(parameters: &[Parameter]) -> Value {
    Value::Array(
        parameters
            .iter()
            .map(|(key, value, use_previous)| {
                let mut parameter = json!({"Key": key, "UsePreviousValue": use_previous});
                if let Some(value) = value {
                    parameter["Value"] = json!(value);
                }
                parameter
            })
            .collect(),
    )
}

/// The `ProvisioningParameters` of a response as (key, value, `UsePreviousValue`)
/// triples, in the order they are reported.
fn reported_parameters(case: &str, reported: &Value) -> Vec<(String, Option<String>, bool)> {
    reported
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a ProvisioningParameters list, got {reported}")
        })
        .iter()
        .map(|parameter| {
            (
                parameter["Key"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a parameter has no Key: {parameter}"))
                    .to_string(),
                parameter
                    .get("Value")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                // The wire omits a `false` default, so an absent member reads as false.
                parameter
                    .get("UsePreviousValue")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
        })
        .collect()
}

/// The `ResourceChanges` of a response as (action, logical resource id, resource type)
/// triples, in logical-id order.
fn reported_changes(case: &str, body: &Value) -> Vec<(String, String, String)> {
    let changes = body["ResourceChanges"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceChanges list, got {body}"));
    let mut read: Vec<(String, String, String)> = changes
        .iter()
        .map(|change| {
            let member = |name: &str| {
                change[name]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a change has no {name}: {change}"))
                    .to_string()
            };
            (member("Action"), member("LogicalResourceId"), member("ResourceType"))
        })
        .collect();
    read.sort();
    read
}

/// A plan request, with only the members a row asks for.
fn plan_request(
    case: &PlanCase,
    launch: &Launch,
    plan_name: &str,
    provisioned_name: &str,
    token: &str,
) -> Value {
    let mut request = json!({
        "PlanName": plan_name,
        "PlanType": PLAN_TYPE,
        "ProductId": launch.product_id,
        "ProvisionedProductName": provisioned_name,
        "ProvisioningArtifactId": launch.artifact_id,
        "IdempotencyToken": token,
    });
    if case.names_path {
        request["PathId"] = json!(expected_path(case, launch));
    }
    if !case.tags.is_empty() {
        request["Tags"] = tag_list(case.tags);
    }
    if !case.notification_arns.is_empty() {
        request["NotificationArns"] = json!(case.notification_arns);
    }
    if !case.parameters.is_empty() {
        request["ProvisioningParameters"] = parameter_list(case.parameters);
    }
    request
}

/// The launch path a row's plan is provisioned through: the one it names, or the
/// product's only path.
fn expected_path(case: &PlanCase, launch: &Launch) -> String {
    let index = if case.names_path {
        launch.portfolio_ids.len() - 1
    } else {
        0
    };
    launch.portfolio_ids[index].clone()
}

/// Describe one plan.
async fn describe_plan(router: &MultiServiceRouter, plan_id: &str) -> (u16, Value) {
    call(
        router,
        "DescribeProvisionedProductPlan",
        &json!({"PlanId": plan_id}),
    )
    .await
}

/// The plan status a describe reports.
fn plan_status(case: &str, body: &Value) -> String {
    body["ProvisionedProductPlanDetails"]["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no plan Status in {body}"))
        .to_string()
}

/// Look at the plan until it reports `wanted`, or the generous deadline passes: the
/// change set behind it settles on the emulator's clock, so the loop waits for the
/// CONDITION and never for a fixed time.
async fn plan_settles_to(
    router: &MultiServiceRouter,
    case: &str,
    plan_id: &str,
    wanted: &str,
) -> Value {
    let started = std::time::Instant::now();
    loop {
        let (status, body) = describe_plan(router, plan_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected plan {plan_id} to be described, got {status} with {body}"
        );
        let reported = plan_status(case, &body);
        if reported == wanted {
            return body;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: expected plan {plan_id} to reach {wanted}, last reported \
             {reported:?} with {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look at the provisioned product `name` until it reports `wanted`, or the deadline
/// passes.
async fn provisioned_product_settles_to(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    wanted: &str,
) {
    let started = std::time::Instant::now();
    loop {
        let (status, body) = call(router, "DescribeProvisionedProduct", &json!({"Name": name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected provisioned product {name} to be described, got {status} \
             with {body}"
        );
        let reported = body["ProvisionedProductDetail"]["Status"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no provisioned product Status in {body}"))
            .to_string();
        if reported == wanted {
            return;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: expected provisioned product {name} to reach {wanted}, last \
             reported {reported:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every shape of plan request stores a plan whose members the create answers and the
/// describe reports identically, lists the resources the product version's template
/// declares, and brings the planned provisioned product into existence in
/// `PLAN_IN_PROGRESS`.
#[tokio::test]
async fn a_plan_lists_the_resources_the_versions_template_declares() {
    let cases = [
        PlanCase {
            case: "one declared resource, default path, no tags, no notifications",
            template: one_resource_template,
            template_key: "plans/one/template.json",
            declared: ONE_RESOURCE,
            product_type: "CLOUD_FORMATION_TEMPLATE",
            portfolios: 1,
            names_path: false,
            tags: &[],
            notification_arns: &[],
            parameters: &[],
        },
        PlanCase {
            case: "two declared resources, a parameter, a named path, one tag",
            template: two_resource_template,
            template_key: "plans/two/template.json",
            declared: TWO_RESOURCES,
            product_type: "CLOUD_FORMATION_TEMPLATE",
            portfolios: 1,
            names_path: true,
            tags: &[("env", "prod")],
            notification_arns: &["arn:aws:sns:us-east-1:000000000000:plan-events"],
            parameters: &[("BucketName", Some("planned-bucket"), false)],
        },
        PlanCase {
            case: "two launch paths, the second named, three tags, two notifications",
            template: two_resource_template,
            template_key: "plans/paths/template.json",
            declared: TWO_RESOURCES,
            product_type: "CLOUD_FORMATION_TEMPLATE",
            portfolios: 2,
            names_path: true,
            tags: &[("env", "stage"), ("owner", "platform"), ("tier", "2")],
            notification_arns: &[
                "arn:aws:sns:us-east-1:000000000000:one",
                "arn:aws:sns:us-east-1:000000000000:two",
            ],
            parameters: &[("BucketName", Some("staged-bucket"), false)],
        },
        PlanCase {
            case: "a Marketplace product, five notification ARNs (the modelled maximum)",
            template: one_resource_template,
            template_key: "plans/marketplace/template.json",
            declared: ONE_RESOURCE,
            product_type: "MARKETPLACE",
            portfolios: 1,
            names_path: false,
            tags: &[("env", "dev")],
            notification_arns: &[
                "arn:aws:sns:us-east-1:000000000000:a",
                "arn:aws:sns:us-east-1:000000000000:b",
                "arn:aws:sns:us-east-1:000000000000:c",
                "arn:aws:sns:us-east-1:000000000000:d",
                "arn:aws:sns:us-east-1:000000000000:e",
            ],
            parameters: &[],
        },
    ];

    let router = emulator_with_template_services();
    create_template_bucket(&router).await;
    for (row, case) in cases.iter().enumerate() {
        let slug = format!("plan-{row}");
        let launch = seed_launch(
            &router,
            &slug,
            case.product_type,
            &(case.template)(),
            case.template_key,
            case.portfolios,
        )
        .await;
        let plan_name = format!("{slug}-plan");
        let provisioned_name = format!("{slug}-pp");
        let token = format!("{slug}-token");
        let request = plan_request(case, &launch, &plan_name, &provisioned_name, &token);

        let (status, created) = call(&router, "CreateProvisionedProductPlan", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the plan to be created, got {status} with {created}",
            case.case
        );

        let plan_id = created["PlanId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no PlanId in {created}", case.case))
            .to_string();
        assert!(
            is_catalog_id("plan", &plan_id),
            "case {:?}: expected a minted plan id, got {plan_id:?}",
            case.case
        );
        let provision_product_id = created["ProvisionProductId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no ProvisionProductId in {created}", case.case))
            .to_string();
        assert!(
            is_catalog_id("pp", &provision_product_id),
            "case {:?}: expected a minted provisioned product id, got {provision_product_id:?}",
            case.case
        );
        assert_eq!(
            created["PlanName"].as_str(),
            Some(plan_name.as_str()),
            "case {:?}: expected the plan name the request asked for, got {created}",
            case.case
        );
        assert_eq!(
            created["ProvisionedProductName"].as_str(),
            Some(provisioned_name.as_str()),
            "case {:?}: expected the provisioned product name the request asked for, got \
             {created}",
            case.case
        );
        assert_eq!(
            created["ProvisioningArtifactId"].as_str(),
            Some(launch.artifact_id.as_str()),
            "case {:?}: expected the product version the request named, got {created}",
            case.case
        );

        // The round trip: every member of the stored plan, read back through the
        // describe once the change set behind it is computed.
        let described = plan_settles_to(&router, case.case, &plan_id, CREATE_SUCCESS).await;
        let details = &described["ProvisionedProductPlanDetails"];
        assert_eq!(
            details["PlanId"].as_str(),
            Some(plan_id.as_str()),
            "case {:?}: expected the created plan's id, got {described}",
            case.case
        );
        assert_eq!(
            details["PlanName"].as_str(),
            Some(plan_name.as_str()),
            "case {:?}: expected the created plan's name, got {described}",
            case.case
        );
        assert_eq!(
            details["PlanType"].as_str(),
            Some(PLAN_TYPE),
            "case {:?}: expected the plan type the request asked for, got {described}",
            case.case
        );
        assert_eq!(
            details["ProductId"].as_str(),
            Some(launch.product_id.as_str()),
            "case {:?}: expected the planned product, got {described}",
            case.case
        );
        assert_eq!(
            details["ProvisioningArtifactId"].as_str(),
            Some(launch.artifact_id.as_str()),
            "case {:?}: expected the planned product version, got {described}",
            case.case
        );
        assert_eq!(
            details["PathId"].as_str(),
            Some(expected_path(case, &launch).as_str()),
            "case {:?}: expected the launch path the plan provisions through, got {described}",
            case.case
        );
        assert_eq!(
            details["ProvisionProductId"].as_str(),
            Some(provision_product_id.as_str()),
            "case {:?}: expected the planned provisioned product, got {described}",
            case.case
        );
        assert_eq!(
            details["ProvisionProductName"].as_str(),
            Some(provisioned_name.as_str()),
            "case {:?}: expected the planned provisioned product's name, got {described}",
            case.case
        );
        assert_eq!(
            details.get("StatusMessage"),
            None,
            "case {:?}: expected no status message on a plan that was created, got {described}",
            case.case
        );
        let expected_arns: Vec<Value> = case
            .notification_arns
            .iter()
            .map(|arn| json!(arn))
            .collect();
        assert_eq!(
            details["NotificationArns"],
            Value::Array(expected_arns),
            "case {:?}: expected the notification ARNs the request passed, got {described}",
            case.case
        );
        assert_eq!(
            reported_parameters(case.case, &details["ProvisioningParameters"]),
            case.parameters
                .iter()
                .map(|(key, value, use_previous)| (
                    key.to_string(),
                    value.map(str::to_string),
                    *use_previous
                ))
                .collect::<Vec<_>>(),
            "case {:?}: expected the provisioning parameters the request carried, got {described}",
            case.case
        );
        let expected_tags: BTreeMap<String, String> = case
            .tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            tag_map(case.case, &details["Tags"]),
            expected_tags,
            "case {:?}: expected the tags the request asked for, got {described}",
            case.case
        );
        let created_time = details["CreatedTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {:?}: no CreatedTime in {described}", case.case));
        assert_eq!(
            details["UpdatedTime"].as_f64(),
            Some(created_time),
            "case {:?}: expected the stored plan's last update to be its creation, got \
             {described}",
            case.case
        );

        // The plan's own content: the resources the version's template declares, each
        // one the plan would ADD, and no further page of them.
        assert_eq!(
            reported_changes(case.case, &described),
            case.declared
                .iter()
                .map(|(logical_id, resource_type)| (
                    ADD.to_string(),
                    logical_id.to_string(),
                    resource_type.to_string()
                ))
                .collect::<Vec<_>>(),
            "case {:?}: expected the resources the template declares, got {described}",
            case.case
        );
        assert_eq!(
            described.get("NextPageToken"),
            None,
            "case {:?}: expected no further page of resource changes, got {described}",
            case.case
        );

        // The provisioned product the plan planned EXISTS, in the status the model
        // documents for a plan nothing has executed yet.
        let (status, provisioned) = call(
            &router,
            "DescribeProvisionedProduct",
            &json!({"Name": provisioned_name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the planned provisioned product to exist, got {status} with \
             {provisioned}",
            case.case
        );
        let detail = &provisioned["ProvisionedProductDetail"];
        assert_eq!(
            detail["Id"].as_str(),
            Some(provision_product_id.as_str()),
            "case {:?}: expected the provisioned product the plan answered, got {provisioned}",
            case.case
        );
        assert_eq!(
            detail["Status"].as_str(),
            Some(PLAN_IN_PROGRESS),
            "case {:?}: expected the planned provisioned product in {PLAN_IN_PROGRESS}, got \
             {provisioned}",
            case.case
        );
        assert_eq!(
            detail["ProductId"].as_str(),
            Some(launch.product_id.as_str()),
            "case {:?}: expected the planned product, got {provisioned}",
            case.case
        );
        assert_eq!(
            detail["IdempotencyToken"].as_str(),
            Some(token.as_str()),
            "case {:?}: expected the token the plan carried, got {provisioned}",
            case.case
        );
        assert_eq!(
            detail.get("LastRecordId"),
            None,
            "case {:?}: expected no record for a provisioned product nothing has provisioned, \
             got {provisioned}",
            case.case
        );

        // Reading the plan changes nothing a later caller sees.
        let (_, again) = describe_plan(&router, &plan_id).await;
        assert_eq!(
            again, described,
            "case {:?}: expected a second read to report the same plan, got {again}",
            case.case
        );
    }
}

/// A plan is answered while the change set behind it is still being computed, and
/// reaches its terminal creation status on the clock.
#[tokio::test]
async fn a_plan_is_in_flight_until_its_change_set_is_computed() {
    let case = "plan creation settles on the clock";
    let router = emulator_with_template_services();
    create_template_bucket(&router).await;
    let launch = seed_simple_launch(&router, "settling", 1).await;
    let (status, created) = call(
        &router,
        "CreateProvisionedProductPlan",
        &json!({
            "PlanName": "settling-plan",
            "PlanType": PLAN_TYPE,
            "ProductId": launch.product_id,
            "ProvisionedProductName": "settling-pp",
            "ProvisioningArtifactId": launch.artifact_id,
            "IdempotencyToken": "settling-token",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the plan to be created, got {status} with {created}"
    );
    let plan_id = created["PlanId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no PlanId in {created}"))
        .to_string();

    // The plan is answered in flight: the span its change set takes to be computed is
    // the limit this case tests, and a caller acting straight after the create still
    // sees it.
    let (_, in_flight) = describe_plan(&router, &plan_id).await;
    assert_eq!(
        plan_status(case, &in_flight),
        CREATE_IN_PROGRESS,
        "case {case:?}: expected the plan in flight straight after the create, got {in_flight}"
    );

    // The clock owes the transition; nothing sleeps for it.
    let settled = plan_settles_to(&router, case, &plan_id, CREATE_SUCCESS).await;
    assert_eq!(
        reported_changes(case, &settled),
        vec![(
            ADD.to_string(),
            "Data".to_string(),
            "AWS::S3::Bucket".to_string()
        )],
        "case {case:?}: expected the settled plan to list the template's resource, got {settled}"
    );
}

/// A plan for a provisioned product this account already holds plans a change to THAT
/// provisioned product, and only one plan can exist for it.
#[tokio::test]
async fn a_plan_for_an_existing_provisioned_product_plans_a_change_to_it() {
    let case = "a plan for an existing provisioned product";
    let router = emulator_with_template_services();
    create_template_bucket(&router).await;
    let launch = seed_simple_launch(&router, "existing", 1).await;

    let (status, provisioned) = call(
        &router,
        "ProvisionProduct",
        &json!({
            "ProductId": launch.product_id,
            "ProvisioningArtifactId": launch.artifact_id,
            "ProvisionedProductName": "existing-pp",
            "ProvisionToken": "existing-provision",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the provisioning to be served, got {status} with {provisioned}"
    );
    let provisioned_id = provisioned["RecordDetail"]["ProvisionedProductId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no ProvisionedProductId in {provisioned}"))
        .to_string();

    // A provisioning is in flight for a while, and "to create a plan for an existing
    // provisioned product, the product status must be AVAILABLE or TAINTED": the plan is
    // refused while the provisioned product is being changed.
    let (status, refused) = call(
        &router,
        "CreateProvisionedProductPlan",
        &json!({
            "PlanName": "too-early-plan",
            "PlanType": PLAN_TYPE,
            "ProductId": launch.product_id,
            "ProvisionedProductName": "existing-pp",
            "ProvisioningArtifactId": launch.artifact_id,
            "IdempotencyToken": "too-early",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a plan for a provisioned product under change to be refused, \
         got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidStateException",
        "case {case:?}: expected the state refusal, got {refused}"
    );
    assert!(
        message(&refused).contains("UNDER_CHANGE"),
        "case {case:?}: expected the refusal to name the status that blocked it, got {:?}",
        message(&refused)
    );

    // Once the provisioning settles, the plan is served and plans a change to the
    // provisioned product that already exists — no second one is minted.
    provisioned_product_settles_to(&router, case, "existing-pp", AVAILABLE).await;
    let (status, created) = call(
        &router,
        "CreateProvisionedProductPlan",
        &json!({
            "PlanName": "update-plan",
            "PlanType": PLAN_TYPE,
            "ProductId": launch.product_id,
            "ProvisionedProductName": "existing-pp",
            "ProvisioningArtifactId": launch.artifact_id,
            "IdempotencyToken": "update-plan-token",
            "ProvisioningParameters": [{"Key": "BucketName", "UsePreviousValue": true}],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the plan to be created, got {status} with {created}"
    );
    assert_eq!(
        created["ProvisionProductId"].as_str(),
        Some(provisioned_id.as_str()),
        "case {case:?}: expected the plan to name the provisioned product that already exists, \
         got {created}"
    );
    let plan_id = created["PlanId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no PlanId in {created}"))
        .to_string();

    // The parameters the request carried are stored and read back verbatim, including
    // the one that keeps the previous value.
    let (status, described) = describe_plan(&router, &plan_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the plan to be described, got {status} with {described}"
    );
    assert_eq!(
        reported_parameters(
            case,
            &described["ProvisionedProductPlanDetails"]["ProvisioningParameters"]
        ),
        vec![("BucketName".to_string(), None, true)],
        "case {case:?}: expected the parameter the request carried, got {described}"
    );

    // Planning a change leaves the provisioned product in its stable status: nothing
    // has been executed yet.
    let (_, still) = call(
        &router,
        "DescribeProvisionedProduct",
        &json!({"Id": provisioned_id}),
    )
    .await;
    assert_eq!(
        still["ProvisionedProductDetail"]["Status"].as_str(),
        Some(AVAILABLE),
        "case {case:?}: expected the provisioned product to stay stable while a plan exists, \
         got {still}"
    );

    // "You can create one plan for each provisioned product": a second plan for it is
    // refused, and the first one is untouched.
    let (status, refused) = call(
        &router,
        "CreateProvisionedProductPlan",
        &json!({
            "PlanName": "second-plan",
            "PlanType": PLAN_TYPE,
            "ProductId": launch.product_id,
            "ProvisionedProductName": "existing-pp",
            "ProvisioningArtifactId": launch.artifact_id,
            "IdempotencyToken": "second-plan-token",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a second plan to be refused, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidStateException",
        "case {case:?}: expected the state refusal, got {refused}"
    );
    assert!(
        message(&refused).contains(&plan_id),
        "case {case:?}: expected the refusal to name the plan that already exists ({plan_id}), \
         got {:?}",
        message(&refused)
    );
    let (status, described) = describe_plan(&router, &plan_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first plan to survive the refusal, got {status} with \
         {described}"
    );
    assert_eq!(
        described["ProvisionedProductPlanDetails"]["PlanName"].as_str(),
        Some("update-plan"),
        "case {case:?}: expected the first plan unchanged, got {described}"
    );
}

/// A repeated `IdempotencyToken` replays the plan the first request created rather than
/// planning a second change.
#[tokio::test]
async fn a_repeated_idempotency_token_replays_the_plan_it_created() {
    let case = "a repeated idempotency token";
    let router = emulator_with_template_services();
    create_template_bucket(&router).await;
    let launch = seed_simple_launch(&router, "replayed", 1).await;
    let request = json!({
        "PlanName": "replayed-plan",
        "PlanType": PLAN_TYPE,
        "ProductId": launch.product_id,
        "ProvisionedProductName": "replayed-pp",
        "ProvisioningArtifactId": launch.artifact_id,
        "IdempotencyToken": "replayed-token",
        "Tags": tag_list(&[("env", "prod")]),
    });

    let (status, first) = call(&router, "CreateProvisionedProductPlan", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the plan to be created, got {status} with {first}"
    );
    // The same token, with a plan name the first request did not use: the token alone
    // decides the replay, so the stored plan is answered unchanged.
    let mut repeated = request.clone();
    repeated["PlanName"] = json!("a-different-name");
    let (status, second) = call(&router, "CreateProvisionedProductPlan", &repeated).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the repeat to be served, got {status} with {second}"
    );
    assert_eq!(
        second, first,
        "case {case:?}: expected the repeat to replay the first plan, got {second}"
    );

    // Exactly one provisioned product was planned, and it is the one both answers name.
    let (status, searched) = call(&router, "SearchProvisionedProducts", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the search to be served, got {status} with {searched}"
    );
    let found = searched["ProvisionedProducts"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ProvisionedProducts in {searched}"));
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected the one planned provisioned product, got {searched}"
    );
    assert_eq!(
        found[0]["Id"].as_str(),
        first["ProvisionProductId"].as_str(),
        "case {case:?}: expected the planned provisioned product, got {searched}"
    );
}

/// One row of the refusal table: a request that names a resource that does not resolve,
/// or a plan that does not exist.
struct RefusalCase {
    case: &'static str,
    /// The operation the row calls.
    operation: &'static str,
    /// The request the row sends, filled in from the seeded launch.
    request: fn(&Launch) -> Value,
    code: &'static str,
    message: &'static str,
}

/// Every resource a plan request names is resolved before anything is stored, and the
/// refusal names what did not resolve.
#[tokio::test]
async fn a_plan_request_naming_an_unresolved_resource_is_refused() {
    let cases = [
        RefusalCase {
            case: "a product id nothing stored carries",
            operation: "CreateProvisionedProductPlan",
            request: |launch| {
                json!({
                    "PlanName": "p",
                    "PlanType": PLAN_TYPE,
                    "ProductId": "prod-00000000nope",
                    "ProvisionedProductName": "refused-pp",
                    "ProvisioningArtifactId": launch.artifact_id,
                    "IdempotencyToken": "refusal-unknown-product",
                })
            },
            code: "ResourceNotFoundException",
            message: "No launch paths found for resource: prod-00000000nope",
        },
        RefusalCase {
            case: "a path id that is none of the product's launch paths",
            operation: "CreateProvisionedProductPlan",
            request: |launch| {
                json!({
                    "PlanName": "p",
                    "PlanType": PLAN_TYPE,
                    "ProductId": launch.product_id,
                    "PathId": "port-00000000nope",
                    "ProvisionedProductName": "refused-pp",
                    "ProvisioningArtifactId": launch.artifact_id,
                    "IdempotencyToken": "refusal-unknown-path",
                })
            },
            code: "ResourceNotFoundException",
            message: "Launch path not found: port-00000000nope",
        },
        RefusalCase {
            case: "a provisioning artifact that is no version of the product",
            operation: "CreateProvisionedProductPlan",
            request: |launch| {
                json!({
                    "PlanName": "p",
                    "PlanType": PLAN_TYPE,
                    "ProductId": launch.product_id,
                    "ProvisionedProductName": "refused-pp",
                    "ProvisioningArtifactId": "pa-00000000nope",
                    "IdempotencyToken": "refusal-unknown-version",
                })
            },
            code: "ResourceNotFoundException",
            message: "Provisioning artifact not found: pa-00000000nope",
        },
        RefusalCase {
            case: "a plan id nothing stored carries",
            operation: "DescribeProvisionedProductPlan",
            request: |_| json!({"PlanId": "plan-00000000nope"}),
            code: "ResourceNotFoundException",
            message: "Plan not found: plan-00000000nope",
        },
    ];

    let router = emulator_with_template_services();
    create_template_bucket(&router).await;
    let launch = seed_simple_launch(&router, "refusals", 1).await;
    for case in &cases {
        let request = (case.request)(&launch);
        let (status, body) = call(&router, case.operation, &request).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected the request to be refused, got {status} with {body}",
            case.case
        );
        assert_eq!(
            error_code(&body),
            case.code,
            "case {:?}: expected {}, got {body}",
            case.case,
            case.code
        );
        assert_eq!(
            message(&body),
            case.message,
            "case {:?}: expected the refusal to name what did not resolve, got {body}",
            case.case
        );
    }

    // Nothing was stored by any refused request: no provisioned product was planned.
    let (status, searched) = call(&router, "SearchProvisionedProducts", &json!({})).await;
    assert_eq!(
        status, 200,
        "expected the search to be served, got {status} with {searched}"
    );
    assert_eq!(
        searched["ProvisionedProducts"],
        json!([]),
        "expected a refused plan to store nothing, got {searched}"
    );
}

/// The resource changes a plan lists are paged by `PageSize`, and only the cursors this
/// service mints resume a page.
#[tokio::test]
async fn the_resource_changes_of_a_plan_are_paged() {
    let case = "paging over a plan's resource changes";
    let router = emulator_with_template_services();
    create_template_bucket(&router).await;
    let launch = seed_launch(
        &router,
        "paged",
        "CLOUD_FORMATION_TEMPLATE",
        &two_resource_template(),
        "plans/paged/template.json",
        1,
    )
    .await;
    let (status, created) = call(
        &router,
        "CreateProvisionedProductPlan",
        &json!({
            "PlanName": "paged-plan",
            "PlanType": PLAN_TYPE,
            "ProductId": launch.product_id,
            "ProvisionedProductName": "paged-pp",
            "ProvisioningArtifactId": launch.artifact_id,
            "IdempotencyToken": "paged-token",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the plan to be created, got {status} with {created}"
    );
    let plan_id = created["PlanId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no PlanId in {created}"))
        .to_string();
    let whole = plan_settles_to(&router, case, &plan_id, CREATE_SUCCESS).await;
    let all_changes = reported_changes(case, &whole);
    assert_eq!(
        all_changes.len(),
        TWO_RESOURCES.len(),
        "case {case:?}: expected one change per declared resource, got {whole}"
    );

    // One change per page: each page carries a cursor onto the next, and the last one
    // carries none.
    let mut paged: Vec<(String, String, String)> = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut request = json!({"PlanId": plan_id, "PageSize": 1});
        if let Some(token) = &page_token {
            request["PageToken"] = json!(token);
        }
        let (status, page) = call(&router, "DescribeProvisionedProductPlan", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the page to be served, got {status} with {page}"
        );
        let changes = reported_changes(case, &page);
        assert_eq!(
            changes.len(),
            1,
            "case {case:?}: expected one change on a page of one, got {page}"
        );
        paged.extend(changes);
        match page["NextPageToken"].as_str() {
            Some(token) => page_token = Some(token.to_string()),
            None => break,
        }
        assert!(
            paged.len() <= all_changes.len(),
            "case {case:?}: expected the pages to end after {} changes, got {paged:?}",
            all_changes.len()
        );
    }
    paged.sort();
    assert_eq!(
        paged, all_changes,
        "case {case:?}: expected the pages to carry every change exactly once"
    );

    // A token this service did not mint is a bad parameter.
    let (status, refused) = call(
        &router,
        "DescribeProvisionedProductPlan",
        &json!({"PlanId": plan_id, "PageToken": "not-a-cursor"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the request to be refused, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParametersException",
        "case {case:?}: expected the parameter refusal, got {refused}"
    );
    assert_eq!(
        message(&refused),
        "PageToken is not a valid page token.",
        "case {case:?}: expected the page-token refusal, got {refused}"
    );
}

/// The Region a request is signed for is addressing only: a plan is created in that
/// Region and read back there whatever Region the read is signed for, and a value that
/// is not a well-formed Region token still plans the same change.
#[tokio::test]
async fn a_region_that_is_not_a_region_token_still_plans_the_change() {
    let router = emulator_with_template_services();
    create_template_bucket(&router).await;
    let launch = seed_simple_launch(&router, "regions", 1).await;
    // Rows differ along the signed Region: the emulator's own, another real one, and
    // two values no Region name can be (an upper-case one, and one far longer than
    // AWS's longest Region name).
    let cases = [
        "us-east-1",
        "eu-west-2",
        "US-East-1",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ];
    for (row, region) in cases.iter().enumerate() {
        let (status, created) = call_in(
            &router,
            region,
            "CreateProvisionedProductPlan",
            &json!({
                "PlanName": format!("region-{row}-plan"),
                "PlanType": PLAN_TYPE,
                "ProductId": launch.product_id,
                "ProvisionedProductName": format!("region-{row}-pp"),
                "ProvisioningArtifactId": launch.artifact_id,
                "IdempotencyToken": format!("region-{row}-token"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected the plan to be created, got {status} with {created}"
        );
        let plan_id = created["PlanId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {region:?}: no PlanId in {created}"))
            .to_string();
        // The read is signed for the harness's own Region, not the one the plan was
        // created in: the plan is still read back from where its change set was made.
        let settled = plan_settles_to(&router, region, &plan_id, CREATE_SUCCESS).await;
        assert_eq!(
            reported_changes(region, &settled),
            vec![(
                ADD.to_string(),
                "Data".to_string(),
                "AWS::S3::Bucket".to_string()
            )],
            "case {region:?}: expected the template's resource to be planned, got {settled}"
        );
    }
}

/// A product with several launch paths leaves `PathId` required: the plan cannot infer
/// which path it provisions through.
#[tokio::test]
async fn a_plan_for_a_product_with_several_paths_requires_the_path() {
    let case = "two launch paths, no PathId";
    let router = emulator_with_template_services();
    create_template_bucket(&router).await;
    let launch = seed_simple_launch(&router, "ambiguous", 2).await;
    let (status, refused) = call(
        &router,
        "CreateProvisionedProductPlan",
        &json!({
            "PlanName": "ambiguous-plan",
            "PlanType": PLAN_TYPE,
            "ProductId": launch.product_id,
            "ProvisionedProductName": "ambiguous-pp",
            "ProvisioningArtifactId": launch.artifact_id,
            "IdempotencyToken": "ambiguous-token",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the request to be refused, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidParametersException",
        "case {case:?}: expected the parameter refusal, got {refused}"
    );
    assert_eq!(
        message(&refused),
        PATH_REQUIRED,
        "case {case:?}: expected AWS's sentence for the missing path, got {refused}"
    );

    // Each of the product's two paths is a path a plan CAN name.
    for (index, portfolio_id) in launch.portfolio_ids.iter().enumerate() {
        let (status, created) = call(
            &router,
            "CreateProvisionedProductPlan",
            &json!({
                "PlanName": format!("path-{index}-plan"),
                "PlanType": PLAN_TYPE,
                "ProductId": launch.product_id,
                "PathId": portfolio_id,
                "ProvisionedProductName": format!("ambiguous-pp-{index}"),
                "ProvisioningArtifactId": launch.artifact_id,
                "IdempotencyToken": format!("ambiguous-token-{index}"),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the plan through path {portfolio_id} to be created, got \
             {status} with {created}"
        );
        let plan_id = created["PlanId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no PlanId in {created}"));
        let (_, described) = describe_plan(&router, plan_id).await;
        assert_eq!(
            described["ProvisionedProductPlanDetails"]["PathId"].as_str(),
            Some(portfolio_id.as_str()),
            "case {case:?}: expected the plan to be provisioned through the path it named, got \
             {described}"
        );
    }
}
