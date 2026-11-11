// SPDX-License-Identifier: CC0-1.0

//! The outputs of a stack whose deployment has COMPLETED — the create, and the revision that
//! swaps which identity its deployed resources name — end to end over the awsQuery wire.
//!
//! RULE under test: **a stack whose deployment has completed reports, for EVERY output the
//! template it now carries declares, an `Output` with both its `OutputKey` and the resolved
//! `OutputValue` — `Ref` answering the resource's physical id, `Fn::GetAtt` the named
//! attribute of it, and `Fn::Sub` the string it composes out of them — and a deployment that
//! revises the template re-resolves every one of them against the resources then in force.**
//! The rule holds whichever operation completed the deployment, so an updated stack
//! (`UPDATE_COMPLETE`, reached through `UpdateStack` or through an executed change set) is
//! held to it exactly as a created one (`CREATE_COMPLETE`) is: the output map is the hand-off
//! a client reads the identifiers of what it just deployed from, and a key reported without a
//! value makes `{o["OutputKey"]: o["OutputValue"] for o in stack["Outputs"]}` — the read every
//! client writes — raise instead of answering.
//!
//! The estate is the reported case: two IAM roles and an Amazon OpenSearch Service domain
//! whose access policy ALLOWS one of them and DENIES the other, re-deployed with the two
//! swapped in place. Its outputs are the ARNs, the names and the ids of all three resources,
//! which is what the client that deployed it goes on to use.
//!
//! The rows differ along what the rule generalises over rather than restating one template:
//!
//! * the SPELLING of the body — JSON with the long-form intrinsic functions, and YAML with
//!   the short-form tags;
//! * which operation COMPLETES the revision — an executed change set, and `UpdateStack`;
//! * where each role's IDENTITY comes from — a row that names both roles and one that names
//!   neither, so CloudFormation generates them;
//! * which published name the domain's ARN is read by — the uniform `Arn` and the type's own
//!   `DomainArn`;
//! * whether the revision declares the SAME outputs or one MORE than the create did.
//!
//! No expected value is a literal: each is computed from what the OWNING service reports for
//! the resource — the domain through `DescribeDomain` on its own `restJson1` wire, each role
//! through IAM's `GetRole` — or from the physical id the stack itself reports for the logical
//! id, so a row cannot drift from the service that holds the resource.
//!
//! Three services have to agree for the rule to hold (CloudFormation resolves the outputs,
//! Amazon OpenSearch Service holds the domain and IAM holds the roles), so the suite lives
//! here. The awsQuery caller, the domain read, the stack poller and the change-set poller are
//! the test crate's own ([`crate::cloudformation_provisions_opensearch_domains`],
//! [`crate::cloudformation_domain_access_policy_revision`],
//! [`crate::iam_role_id_agrees_with_sts`], [`crate::query_wire`]) rather than a second copy.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_domain_access_policy_revision::settled_change_set;
use crate::cloudformation_provisions_opensearch_domains::{
    cloudformation, describe_domain, resource_reasons, settle_stack, xml_text, REGION,
};
use crate::iam_role_id_agrees_with_sts::iam;
use crate::query_wire::{element, outputs_of, physical_ids_of};

/// The engine release every row deploys, from the owning service's own catalogue.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// The logical id of the domain every template declares.
const DOMAIN: &str = "Domain";

/// The logical ids of the two roles every template declares. The create allows the first and
/// denies the second; the revision swaps them.
const ROLE_ONE: &str = "RoleOne";
const ROLE_TWO: &str = "RoleTwo";

/// The outputs every template declares, whichever spelling it is in.
const BASE_OUTPUTS: [&str; 10] = [
    "AllowedRoleArn",
    "AllowedRoleId",
    "AllowedRoleName",
    "DashboardsUrl",
    "DeniedRoleArn",
    "DomainArn",
    "DomainEndpoint",
    "DomainId",
    "DomainName",
    "StackRegion",
];

/// The one output a revision may declare BESIDE those: an output set that GREW is still one
/// where every entry carries its value.
const ADDED_OUTPUT: &str = "DeniedRoleName";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them: the stack really creates its domain in Amazon
/// OpenSearch Service and its roles in IAM, so every value asserted here is one of those
/// services' own.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_opensearch_amazonopensearchservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// How a row's revision is deployed: the two operations that complete an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deploy {
    UpdateStack,
    ExecuteChangeSet,
}

/// How a row's template body is written: the two spellings CloudFormation accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// JSON, with the long-form intrinsic functions (`{"Fn::GetAtt": ["Domain", "Arn"]}`).
    Json,
    /// YAML, with the short-form intrinsic tags (`!GetAtt Domain.Arn`).
    Yaml,
}

/// One row of the table.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The domain the template names, inside `DomainName`'s own `^[a-z][a-z0-9\-]+$` and
    /// 3..28 characters.
    domain: &'static str,
    spelling: Spelling,
    deploy: Deploy,
    /// The names the template gives the two roles, or `None` to leave both to CloudFormation.
    role_names: Option<(&'static str, &'static str)>,
    /// Which published attribute the template reads the domain's ARN by: the uniform `Arn`
    /// every type carries, and the type's own `DomainArn`.
    domain_arn_attribute: &'static str,
    /// Whether the revision declares one output MORE than the create did.
    revision_adds_an_output: bool,
}

/// The table: both spellings, both ways an update completes, both sources of a role's name,
/// both published names of the domain's ARN, and an output set that stays and one that grows.
const ROWS: &[Row] = &[
    Row {
        case: "a JSON template whose named roles are swapped by an executed change set",
        stack: "sim-cfnout-changeset-stack",
        domain: "sim-cfnout-changeset",
        spelling: Spelling::Json,
        deploy: Deploy::ExecuteChangeSet,
        role_names: Some(("sim-cfnout-role-one", "sim-cfnout-role-two")),
        domain_arn_attribute: "Arn",
        revision_adds_an_output: true,
    },
    Row {
        case: "a YAML template whose generated roles are swapped by UpdateStack",
        stack: "sim-cfnout-update-stack",
        domain: "sim-cfnout-update",
        spelling: Spelling::Yaml,
        deploy: Deploy::UpdateStack,
        role_names: None,
        domain_arn_attribute: "DomainArn",
        revision_adds_an_output: false,
    },
];

/// The trust policy both roles carry: Amazon OpenSearch Service may assume them.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "es.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// The template a row deploys at one stage: the two roles, the domain whose access policy
/// names `allowed` in its `Allow` statement and `denied` in its `Deny` one, and the outputs
/// that publish the identifiers of all three.
fn template(row: &Row, allowed: &str, denied: &str, with_added_output: bool) -> String {
    match row.spelling {
        Spelling::Json => json_template(row, allowed, denied, with_added_output),
        Spelling::Yaml => yaml_template(row, allowed, denied, with_added_output),
    }
}

/// [`template`] in JSON, with the long-form intrinsic functions.
fn json_template(row: &Row, allowed: &str, denied: &str, with_added_output: bool) -> String {
    let resource = json!({
        "Fn::Sub":
            format!(
                "arn:${{AWS::Partition}}:es:${{AWS::Region}}:${{AWS::AccountId}}:domain/{}/*",
                row.domain
            ),
    });
    let statement = |effect: &str, role: &str| {
        json!({
            "Effect": effect,
            "Principal": { "AWS": { "Fn::GetAtt": [role, "Arn"] } },
            "Action": "es:*",
            "Resource": resource,
        })
    };
    let role = |name: Option<&str>| {
        let mut properties = json!({ "AssumeRolePolicyDocument": trust_policy() });
        if let (Some(name), Some(object)) = (name, properties.as_object_mut()) {
            object.insert("RoleName".to_string(), json!(name));
        }
        json!({ "Type": "AWS::IAM::Role", "Properties": properties })
    };
    let mut outputs = json!({
        "DomainArn": { "Value": { "Fn::GetAtt": [DOMAIN, row.domain_arn_attribute] } },
        "DomainEndpoint": { "Value": { "Fn::GetAtt": [DOMAIN, "DomainEndpoint"] } },
        "DomainName": { "Value": { "Ref": DOMAIN } },
        "DomainId": { "Value": { "Fn::GetAtt": [DOMAIN, "Id"] } },
        "AllowedRoleArn": { "Value": { "Fn::GetAtt": [allowed, "Arn"] } },
        "AllowedRoleName": { "Value": { "Ref": allowed } },
        "AllowedRoleId": { "Value": { "Fn::GetAtt": [allowed, "RoleId"] } },
        "DeniedRoleArn": { "Value": { "Fn::GetAtt": [denied, "Arn"] } },
        "DashboardsUrl": {
            "Value": { "Fn::Sub": format!("https://${{{DOMAIN}.DomainEndpoint}}/_dashboards") },
        },
        "StackRegion": { "Value": { "Ref": "AWS::Region" } },
    });
    if with_added_output {
        if let Some(object) = outputs.as_object_mut() {
            object.insert(
                ADDED_OUTPUT.to_string(),
                json!({ "Value": { "Ref": denied } }),
            );
        }
    }
    let (one, two) = match row.role_names {
        Some((one, two)) => (Some(one), Some(two)),
        None => (None, None),
    };
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            ROLE_ONE: role(one),
            ROLE_TWO: role(two),
            DOMAIN: {
                "Type": "AWS::OpenSearchService::Domain",
                "Properties": {
                    "DomainName": row.domain,
                    "EngineVersion": ENGINE_VERSION,
                    "ClusterConfig": {
                        "InstanceType": "t3.small.search",
                        "InstanceCount": 1,
                        "DedicatedMasterEnabled": false,
                        "ZoneAwarenessEnabled": false,
                    },
                    "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
                    "DomainEndpointOptions": { "EnforceHTTPS": true },
                    "AccessPolicies": {
                        "Version": "2012-10-17",
                        "Statement": [
                            statement("Allow", allowed),
                            statement("Deny", denied),
                        ],
                    },
                },
            },
        },
        "Outputs": outputs,
    })
    .to_string()
}

/// [`template`] in YAML, with the short-form intrinsic tags — the same estate, written the
/// other way a client writes it.
fn yaml_template(row: &Row, allowed: &str, denied: &str, with_added_output: bool) -> String {
    let role = |logical: &str, name: Option<&str>| {
        let named = match name {
            Some(name) => format!("      RoleName: {name}\n"),
            None => String::new(),
        };
        format!(
            "  {logical}:\n\
             \x20   Type: AWS::IAM::Role\n\
             \x20   Properties:\n\
             {named}\
             \x20     AssumeRolePolicyDocument:\n\
             \x20       Version: \"2012-10-17\"\n\
             \x20       Statement:\n\
             \x20         - Effect: Allow\n\
             \x20           Principal:\n\
             \x20             Service: es.amazonaws.com\n\
             \x20           Action: \"sts:AssumeRole\"\n"
        )
    };
    let statement = |effect: &str, logical: &str| {
        format!(
            "          - Effect: {effect}\n\
             \x20           Principal:\n\
             \x20             AWS: !GetAtt {logical}.Arn\n\
             \x20           Action: \"es:*\"\n\
             \x20           Resource: !Sub \"arn:${{AWS::Partition}}:es:${{AWS::Region}}:${{AWS::AccountId}}:domain/{domain}/*\"\n",
            domain = row.domain,
        )
    };
    let (one, two) = match row.role_names {
        Some((one, two)) => (Some(one), Some(two)),
        None => (None, None),
    };
    let added = if with_added_output {
        format!("  {ADDED_OUTPUT}:\n    Value: !Ref {denied}\n")
    } else {
        String::new()
    };
    format!(
        "AWSTemplateFormatVersion: \"2010-09-09\"\n\
         Resources:\n\
         {role_one}{role_two}\
         \x20 {DOMAIN}:\n\
         \x20   Type: AWS::OpenSearchService::Domain\n\
         \x20   Properties:\n\
         \x20     DomainName: {domain}\n\
         \x20     EngineVersion: {ENGINE_VERSION}\n\
         \x20     ClusterConfig:\n\
         \x20       InstanceType: t3.small.search\n\
         \x20       InstanceCount: 1\n\
         \x20       DedicatedMasterEnabled: false\n\
         \x20       ZoneAwarenessEnabled: false\n\
         \x20     EBSOptions:\n\
         \x20       EBSEnabled: true\n\
         \x20       VolumeType: gp3\n\
         \x20       VolumeSize: 10\n\
         \x20     DomainEndpointOptions:\n\
         \x20       EnforceHTTPS: true\n\
         \x20     AccessPolicies:\n\
         \x20       Version: \"2012-10-17\"\n\
         \x20       Statement:\n\
         {allow_statement}{deny_statement}\
         Outputs:\n\
         \x20 DomainArn:\n\
         \x20   Value: !GetAtt {DOMAIN}.{arn_attribute}\n\
         \x20 DomainEndpoint:\n\
         \x20   Value: !GetAtt {DOMAIN}.DomainEndpoint\n\
         \x20 DomainName:\n\
         \x20   Value: !Ref {DOMAIN}\n\
         \x20 DomainId:\n\
         \x20   Value: !GetAtt {DOMAIN}.Id\n\
         \x20 AllowedRoleArn:\n\
         \x20   Value: !GetAtt {allowed}.Arn\n\
         \x20 AllowedRoleName:\n\
         \x20   Value: !Ref {allowed}\n\
         \x20 AllowedRoleId:\n\
         \x20   Value: !GetAtt {allowed}.RoleId\n\
         \x20 DeniedRoleArn:\n\
         \x20   Value: !GetAtt {denied}.Arn\n\
         \x20 DashboardsUrl:\n\
         \x20   Value: !Sub \"https://${{{DOMAIN}.DomainEndpoint}}/_dashboards\"\n\
         \x20 StackRegion:\n\
         \x20   Value: !Ref AWS::Region\n\
         {added}",
        role_one = role(ROLE_ONE, one),
        role_two = role(ROLE_TWO, two),
        domain = row.domain,
        allow_statement = statement("Allow", allowed),
        deny_statement = statement("Deny", denied),
        arn_attribute = row.domain_arn_attribute,
    )
}

/// The members a stack operation carries: the template, and the acknowledgement a template
/// declaring named IAM resources needs.
fn deployment_members<'a>(stack: &'a str, body: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("StackName", stack),
        ("TemplateBody", body),
        ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
    ]
}

/// One CloudFormation call that must be accepted.
async fn accepted(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = cloudformation(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {action}, got {status} with {body}"
    );
    body
}

/// What IAM reports for one role, as the three values a template reads off it: `Ref` answers
/// the role's name, `Fn::GetAtt` its `Arn` and its `RoleId`.
async fn reported_role(router: &MultiServiceRouter, case: &str, role_name: &str) -> RoleReport {
    let (status, body) = iam(
        router,
        "GetRole",
        &[("RoleName", role_name.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from IAM GetRole({role_name}), got {status} with {body}"
    );
    RoleReport {
        name: element(&body, "RoleName"),
        arn: element(&body, "Arn"),
        role_id: element(&body, "RoleId"),
    }
}

/// One role as IAM reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RoleReport {
    name: String,
    arn: String,
    role_id: String,
}

/// One domain as Amazon OpenSearch Service reports it: the identifiers and the address a
/// template's `Ref` and `Fn::GetAtt` read off it, and the access policy in force on it.
struct DomainReport {
    name: String,
    arn: String,
    domain_id: String,
    endpoint: String,
    access_policies: Value,
}

/// Read the domain back through the owning service's own reader.
async fn reported_domain(router: &MultiServiceRouter, case: &str, domain: &str) -> DomainReport {
    let (status, reported) = describe_domain(router, domain).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from DescribeDomain({domain}), got {status} with {reported}"
    );
    let text = |member: &str| {
        reported
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    DomainReport {
        name: text("DomainName"),
        arn: text("ARN"),
        domain_id: text("DomainId"),
        endpoint: text("Endpoint"),
        access_policies: serde_json::from_str(&text("AccessPolicies")).unwrap_or(Value::Null),
    }
}

/// The `Principal.AWS` of the one statement of `policy` carrying `effect`.
fn principal_of(policy: &Value, effect: &str) -> Option<String> {
    policy
        .get("Statement")?
        .as_array()?
        .iter()
        .find(|statement| statement.get("Effect").and_then(Value::as_str) == Some(effect))?
        .get("Principal")?
        .get("AWS")?
        .as_str()
        .map(str::to_string)
}

/// Hold one settled deployment to the rule, and answer the output map it reported.
///
/// `allowed`/`denied` are the logical ids the template at this stage allows and denies, and
/// every expected value is computed from the owning service's own report of the resource or
/// from the physical id the stack reports for the logical id — never read out of the output
/// being checked.
async fn outputs_hold(
    router: &MultiServiceRouter,
    row: &Row,
    stage: &str,
    described: &str,
    allowed: &str,
    denied: &str,
    with_added_output: bool,
) -> BTreeMap<String, String> {
    let case = format!("{} ({stage})", row.case);
    let case = case.as_str();
    let outputs = outputs_of(described);

    // Every declared output is reported, and each one carries a value: `outputs_of` pairs a
    // key with the value inside its OWN member, so an entry reported without an `OutputValue`
    // is missing from the map rather than borrowing its neighbour's.
    let mut expected: BTreeSet<String> = BASE_OUTPUTS.iter().map(|key| key.to_string()).collect();
    if with_added_output {
        expected.insert(ADDED_OUTPUT.to_string());
    }
    let reported: BTreeSet<String> = outputs.keys().cloned().collect();
    assert_eq!(
        reported, expected,
        "case {case:?}: every declared output must be reported with its OutputValue, got \
         {outputs:?} in {described}"
    );
    for (key, value) in &outputs {
        assert!(
            !value.is_empty(),
            "case {case:?}: output {key:?} must carry a resolved value, got {outputs:?}"
        );
    }

    // The physical ids the stack reports for its logical ids, which is what a `Ref` answers.
    let resources = accepted(
        router,
        case,
        "DescribeStackResources",
        &[("StackName", row.stack)],
    )
    .await;
    let physical = physical_ids_of(&resources);
    let physical_of = |logical: &str| {
        physical
            .get(logical)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no physical id for {logical} in {resources}"))
    };

    let domain = reported_domain(router, case, row.domain).await;
    let allowed_role = reported_role(router, case, &physical_of(allowed)).await;
    let denied_role = reported_role(router, case, &physical_of(denied)).await;

    // (the output, the value the service that owns the resource reports for it)
    let mut owed: Vec<(&str, String)> = vec![
        ("DomainArn", domain.arn.clone()),
        ("DomainEndpoint", domain.endpoint.clone()),
        ("DomainName", domain.name.clone()),
        ("DomainId", domain.domain_id.clone()),
        ("AllowedRoleArn", allowed_role.arn.clone()),
        ("AllowedRoleName", allowed_role.name.clone()),
        ("AllowedRoleId", allowed_role.role_id.clone()),
        ("DeniedRoleArn", denied_role.arn.clone()),
        (
            "DashboardsUrl",
            format!("https://{}/_dashboards", domain.endpoint),
        ),
        ("StackRegion", REGION.to_string()),
    ];
    if with_added_output {
        owed.push((ADDED_OUTPUT, denied_role.name.clone()));
    }
    for (key, value) in owed {
        assert_eq!(
            outputs.get(key).map(String::as_str),
            Some(value.as_str()),
            "case {case:?}: output {key:?} must resolve to {value:?}, got {outputs:?}"
        );
    }

    // A `Ref` answers the physical id the stack itself reports, and the two roles are two
    // different roles — so a swap is observable rather than two readings of one value.
    assert_eq!(
        outputs.get("AllowedRoleName").map(String::as_str),
        Some(physical_of(allowed).as_str()),
        "case {case:?}: Ref must answer the role's physical id, got {outputs:?}"
    );
    assert_ne!(
        allowed_role.arn, denied_role.arn,
        "case {case:?}: the two roles must be two different roles, got {allowed_role:?} and \
         {denied_role:?}"
    );

    // And the policy the domain really carries names those same ARNs, so the outputs describe
    // an estate that exists rather than a template that was read.
    assert_eq!(
        principal_of(&domain.access_policies, "Allow"),
        Some(allowed_role.arn.clone()),
        "case {case:?}: the domain's Allow statement must name the allowed role, got {}",
        domain.access_policies
    );
    assert_eq!(
        principal_of(&domain.access_policies, "Deny"),
        Some(denied_role.arn.clone()),
        "case {case:?}: the domain's Deny statement must name the denied role, got {}",
        domain.access_policies
    );

    outputs
}

/// Deploy each row, revise it the row's way, and hold both completed deployments to the rule.
#[tokio::test]
async fn a_completed_deployment_reports_every_declared_output_with_its_value() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;

        // The create: the first role is allowed, the second denied.
        let created = template(row, ROLE_ONE, ROLE_TWO, false);
        accepted(
            &router,
            case,
            "CreateStack",
            &deployment_members(row.stack, &created),
        )
        .await;
        let described = settle_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus"),
            "CREATE_COMPLETE",
            "case {case:?}: expected the stack to deploy, got {described} with reasons {:?}",
            resource_reasons(&router, row.stack).await
        );
        let after_create = outputs_hold(
            &router,
            row,
            "the completed create",
            &described,
            ROLE_ONE,
            ROLE_TWO,
            false,
        )
        .await;

        // The revision: the same estate with the two roles swapped, deployed the row's way.
        let revised = template(row, ROLE_TWO, ROLE_ONE, row.revision_adds_an_output);
        match row.deploy {
            Deploy::UpdateStack => {
                accepted(
                    &router,
                    case,
                    "UpdateStack",
                    &deployment_members(row.stack, &revised),
                )
                .await;
            }
            Deploy::ExecuteChangeSet => {
                let change_set = format!("{}-revision", row.stack);
                let mut members = deployment_members(row.stack, &revised);
                members.push(("ChangeSetName", &change_set));
                members.push(("ChangeSetType", "UPDATE"));
                accepted(&router, case, "CreateChangeSet", &members).await;
                settled_change_set(&router, case, row.stack, &change_set).await;
                accepted(
                    &router,
                    case,
                    "ExecuteChangeSet",
                    &[("StackName", row.stack), ("ChangeSetName", &change_set)],
                )
                .await;
            }
        }
        let updated = settle_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&updated, "StackStatus"),
            "UPDATE_COMPLETE",
            "case {case:?}: expected the revision to deploy, got {updated} with reasons {:?}",
            resource_reasons(&router, row.stack).await
        );
        let after_update = outputs_hold(
            &router,
            row,
            "the completed update",
            &updated,
            ROLE_TWO,
            ROLE_ONE,
            row.revision_adds_an_output,
        )
        .await;

        // The update RE-RESOLVED the outputs that moved: the roles swapped places, so the two
        // role ARNs traded outputs, while the domain's own identifiers are untouched.
        assert_eq!(
            (
                after_update.get("AllowedRoleArn"),
                after_update.get("DeniedRoleArn"),
            ),
            (
                after_create.get("DeniedRoleArn"),
                after_create.get("AllowedRoleArn"),
            ),
            "case {case:?}: the revision must re-resolve the outputs the swap moved, got \
             {after_create:?} then {after_update:?}"
        );
        for key in ["DomainArn", "DomainEndpoint", "DomainName", "DomainId"] {
            assert_eq!(
                after_update.get(key),
                after_create.get(key),
                "case {case:?}: an in-place revision must not move the domain's {key}, got \
                 {after_create:?} then {after_update:?}"
            );
        }
    }
}
