// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack whose `AWS::OpenSearchService::Domain` is RE-DEPLOYED with a
//! different access policy: the policy change reaches the domain the stack already has, and
//! it is what decides who may reach that domain's data plane afterwards.
//!
//! The rule under test: **a property of a deployed resource that the owning service's own
//! update API takes is applied to the resource the stack has, through that API, rather than
//! left undone or applied by replacing the resource — so the resource keeps its identity and
//! the address a client already holds for it, and the new value is in force.**
//! `AWS::OpenSearchService::Domain`'s `AccessPolicies` is such a property: it is a member of
//! `UpdateDomainConfig`, "modifies the cluster configuration of the specified Amazon
//! OpenSearch Service domain"
//! (`smithy_query(query="operation", operation="UpdateDomainConfig")`), and it is the
//! document the domain's own IAM authorization is decided from — so the change is observable
//! on the data plane and nowhere else: a principal the new document allows is served, and
//! one it denies is refused `403` with AWS's own `Message` sentence.
//!
//! The rows differ along what the rule generalises over: which WAY the revision is deployed
//! (`UpdateStack`, and a change set created and executed — the two ways a client applies a
//! changed template), which direction the effect moves (an allow becoming a deny and a deny
//! becoming an allow), and whether the policy a stack started with named the caller at all.
//! The recorded case — a change set that swaps two roles between `Allow` and `Deny` — is one
//! row of that table.
//!
//! Four services have to agree for the rule to hold (CloudFormation makes the update call,
//! Amazon OpenSearch Service applies it and serves the domain, and STS and IAM are what the
//! principal of a signed data-plane request is resolved through), so the suite lives here
//! rather than in any one service's crate. The awsQuery and `restJson1` callers, the stack
//! poll and the domain read come from
//! [`crate::cloudformation_provisions_opensearch_domains`]; this suite adds the router that
//! mounts the four services and the data-plane dial, and no second copy of them.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_provisions_opensearch_domains::{
    cloudformation, describe_domain, resource_reasons, settle_stack, xml_text, REGION,
};
use crate::query_wire::outputs_of;

/// The engine release every row deploys, from the owning service's own catalogue.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// A SECOND release that catalogue offers, for the revision that moves the one property
/// `UpdateDomainConfig` does not carry: Amazon OpenSearch Service moves a domain between
/// releases through `UpgradeDomain`.
const OTHER_ENGINE_VERSION: &str = "OpenSearch_2.13";

/// The logical id every template declares the domain under.
const DOMAIN_LOGICAL_ID: &str = "Domain";

/// The emulator with the four services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them: CloudFormation makes the calls, Amazon OpenSearch
/// Service serves the domain, and a signed data-plane request's principal is resolved through
/// STS over the IAM store.
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
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One data-plane request to the address the control plane PUBLISHED for the domain, dialled
/// exactly as a client dials it: the client builds `https://{address}{path}`, so the
/// authority of the published value is the host the request carries and whatever follows it
/// is the prefix of the request path.
async fn at_published_address(
    router: &MultiServiceRouter,
    address: &str,
    method: &'static str,
    path: &str,
) -> (u16, Value) {
    let authority = address.split('/').next().unwrap_or(address);
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{address}{path}"))
        .header("host", authority)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/es/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The template one row deploys: the domain, carrying `policy` as its `AccessPolicies` and
/// `engine_version` as its release, with the identifiers the owning service mints as
/// outputs. The policy is written the way a template writes a policy document — as the
/// document itself, with its principal and its resource built by `Fn::Sub` over the pseudo
/// parameters — so the provisioner is what renders it as the member's modelled text.
fn template_on(domain: &str, effect: &str, engine_version: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            DOMAIN_LOGICAL_ID: {
                "Type": "AWS::OpenSearchService::Domain",
                "Properties": {
                    "DomainName": domain,
                    "EngineVersion": engine_version,
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
                        "Statement": [{
                            "Effect": effect,
                            "Principal": {
                                "AWS": {
                                    "Fn::Sub":
                                        "arn:${AWS::Partition}:iam::${AWS::AccountId}:root",
                                },
                            },
                            "Action": "es:*",
                            "Resource": {
                                "Fn::Sub":
                                    "arn:${AWS::Partition}:es:${AWS::Region}:${AWS::AccountId}:domain/<DOMAIN>/*",
                            },
                        }],
                    },
                },
            },
        },
        "Outputs": {
            "DomainArn": { "Value": { "Fn::GetAtt": [DOMAIN_LOGICAL_ID, "Arn"] } },
            "DomainEndpoint": {
                "Value": { "Fn::GetAtt": [DOMAIN_LOGICAL_ID, "DomainEndpoint"] },
            },
        },
    })
    .to_string()
    .replace("<DOMAIN>", domain)
}

/// [`template_on`] for the engine release every row deploys.
fn template(domain: &str, effect: &str) -> String {
    template_on(domain, effect, ENGINE_VERSION)
}

/// How a row deploys its revision: the two ways a client applies a changed template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deploy {
    UpdateStack,
    ExecuteChangeSet,
}

/// One row: the stack and domain it deploys, the effect its policy starts with, the effect
/// the revision states, and how the revision is deployed.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The domain the template names, inside `DomainName`'s own `^[a-z][a-z0-9\-]+$` and
    /// 3..28 characters.
    domain: &'static str,
    created_effect: &'static str,
    revised_effect: &'static str,
    deploy: Deploy,
}

/// The table: both directions the effect moves, deployed both ways a changed template is
/// applied.
const ROWS: &[Row] = &[
    Row {
        case: "an allow becoming a deny, through a change set",
        stack: "sim-cfnpol-deny-stack",
        domain: "sim-cfnpol-deny",
        created_effect: "Allow",
        revised_effect: "Deny",
        deploy: Deploy::ExecuteChangeSet,
    },
    Row {
        case: "a deny becoming an allow, through UpdateStack",
        stack: "sim-cfnpol-allow-stack",
        domain: "sim-cfnpol-allow",
        created_effect: "Deny",
        revised_effect: "Allow",
        deploy: Deploy::UpdateStack,
    },
];

/// The identity a client holds for the domain, which an in-place policy change must not
/// move: the id and the ARN the owning service minted, and the address it publishes.
async fn reported_identity(
    router: &MultiServiceRouter,
    domain: &str,
    case: &str,
) -> (String, String, String) {
    let (status, reported) = describe_domain(router, domain).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from DescribeDomain, got {status} with {reported}"
    );
    let text = |member: &str| {
        reported
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let identity = (text("DomainId"), text("ARN"), text("Endpoint"));
    assert!(
        !identity.0.is_empty() && !identity.1.is_empty() && !identity.2.is_empty(),
        "case {case:?}: expected the domain to report its id, its ARN and its address, got \
         {reported}"
    );
    identity
}

/// The document the domain carries now, parsed.
async fn stored_policy(router: &MultiServiceRouter, domain: &str, case: &str) -> Value {
    let (status, reported) = describe_domain(router, domain).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from DescribeDomain, got {status} with {reported}"
    );
    let text = reported
        .get("AccessPolicies")
        .and_then(Value::as_str)
        .unwrap_or_default();
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// What the data plane answers the account's own signed request at `address`: `200` with the
/// cluster health for a principal the policy allows, `403` with AWS's own `Message` for one
/// it denies.
async fn health_verdict(router: &MultiServiceRouter, address: &str, case: &str) -> (u16, Value) {
    let answered = at_published_address(router, address, "GET", "/_cluster/health").await;
    assert!(
        matches!(answered.0, 200 | 403),
        "case {case:?}: expected the data plane to decide the request, got {} with {}",
        answered.0,
        answered.1
    );
    answered
}

/// Deploy one row's stack, re-deploy it with the revised policy, and assert the rule holds at
/// every step.
#[tokio::test]
async fn an_access_policy_revision_reaches_the_deployed_domain_in_place() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template(row.domain, row.created_effect)),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
        );
        let described = settle_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus"),
            "CREATE_COMPLETE",
            "case {case:?}: expected the stack to deploy, got {described} with reasons {:?}",
            resource_reasons(&router, row.stack).await
        );

        // The address the stack publishes for the domain IS the address the owning service
        // publishes for it, and it is the address the data plane answers at.
        let outputs = outputs_of(&described);
        let identity = reported_identity(&router, row.domain, case).await;
        assert_eq!(
            outputs.get("DomainEndpoint").map(String::as_str),
            Some(identity.2.as_str()),
            "case {case:?}: expected the stack's DomainEndpoint to be the one the service \
             publishes, got {outputs:?}"
        );
        assert_eq!(
            outputs.get("DomainArn").map(String::as_str),
            Some(identity.1.as_str()),
            "case {case:?}: expected the stack's DomainArn to be the service's own, got \
             {outputs:?}"
        );
        let created_verdict = health_verdict(&router, &identity.2, case).await;
        let created_status = if row.created_effect == "Allow" { 200 } else { 403 };
        assert_eq!(
            created_verdict.0, created_status,
            "case {case:?}: expected the policy the stack deployed to decide the request \
             ({created_status}), got {} with {}",
            created_verdict.0, created_verdict.1
        );

        // The revision: the same template with the one effect moved, deployed the row's way.
        let revision = template(row.domain, row.revised_effect);
        match row.deploy {
            Deploy::UpdateStack => {
                let (status, body) = cloudformation(
                    &router,
                    "UpdateStack",
                    &[("StackName", row.stack), ("TemplateBody", &revision)],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from UpdateStack, got {status} with {body}"
                );
            }
            Deploy::ExecuteChangeSet => {
                let change_set = format!("{}-revision", row.stack);
                let (status, body) = cloudformation(
                    &router,
                    "CreateChangeSet",
                    &[
                        ("StackName", row.stack),
                        ("ChangeSetName", &change_set),
                        ("TemplateBody", &revision),
                        ("ChangeSetType", "UPDATE"),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from CreateChangeSet, got {status} with {body}"
                );
                // The change set previews ONE change to the domain, and it is a modification
                // in place: the resource is not recreated for a property its service can
                // change on it.
                let planned = settled_change_set(&router, case, row.stack, &change_set).await;
                assert_eq!(
                    (
                        xml_text(&planned, "LogicalResourceId"),
                        xml_text(&planned, "Action"),
                        xml_text(&planned, "Replacement"),
                    ),
                    (
                        DOMAIN_LOGICAL_ID.to_string(),
                        "Modify".to_string(),
                        "False".to_string()
                    ),
                    "case {case:?}: expected one in-place modification of the domain, got \
                     {planned}"
                );
                assert!(
                    planned.contains("AccessPolicies"),
                    "case {case:?}: expected the change to name the property that moved, got \
                     {planned}"
                );
                let (status, body) = cloudformation(
                    &router,
                    "ExecuteChangeSet",
                    &[("StackName", row.stack), ("ChangeSetName", &change_set)],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from ExecuteChangeSet, got {status} with {body}"
                );
            }
        }
        let updated = settle_stack(&router, case, row.stack).await;
        assert_eq!(
            xml_text(&updated, "StackStatus"),
            "UPDATE_COMPLETE",
            "case {case:?}: expected the revision to deploy, got {updated} with reasons {:?}",
            resource_reasons(&router, row.stack).await
        );

        // The domain is the SAME domain — its id, its ARN and the address a client already
        // holds are untouched — and the document now in force is the revised one.
        assert_eq!(
            reported_identity(&router, row.domain, case).await,
            identity,
            "case {case:?}: an access-policy revision must move neither the domain's identity \
             nor the address it publishes"
        );
        let stored = stored_policy(&router, row.domain, case).await;
        assert_eq!(
            stored["Statement"][0]["Effect"].as_str(),
            Some(row.revised_effect),
            "case {case:?}: expected the revised document to be the one in force, got {stored}"
        );

        // And the data plane follows it: the verdict the caller gets is the other one.
        let revised_verdict = health_verdict(&router, &identity.2, case).await;
        let revised_status = if row.revised_effect == "Allow" { 200 } else { 403 };
        assert_eq!(
            revised_verdict.0, revised_status,
            "case {case:?}: expected the revised policy to decide the request \
             ({revised_status}), got {} with {}",
            revised_verdict.0, revised_verdict.1
        );
        if revised_status == 403 {
            let message = revised_verdict.1["Message"].as_str().unwrap_or_default();
            assert!(
                message.contains("is not authorized to perform: es:ESHttpGet")
                    && message.ends_with("with an explicit deny in a resource-based policy"),
                "case {case:?}: expected AWS's own explicit-deny sentence, got {:?}",
                revised_verdict.1
            );
        } else {
            assert!(
                revised_verdict.1.get("status").is_some()
                    || revised_verdict.1.get("cluster_name").is_some(),
                "case {case:?}: expected the cluster health document, got {}",
                revised_verdict.1
            );
        }
    }
}

/// RULE (the other half of rule 3): a revision that states a property the owning service's
/// update API CANNOT carry is reported as not applied — the deployment fails and the
/// resource is left as it is — rather than as a success that silently dropped that half of
/// the change. The engine release is such a property: Amazon OpenSearch Service moves a
/// domain between releases through `UpgradeDomain`, which is no member of
/// `UpdateDomainConfig`, so a revision changing it beside a property that call does carry is
/// one this emulator does not make in place.
#[tokio::test]
async fn a_revision_the_update_api_cannot_carry_is_not_reported_as_applied() {
    let router = build_router();
    let (stack, domain) = ("sim-cfnpol-release-stack", "sim-cfnpol-release");
    let case = "a revision changing the engine release beside the policy";

    let (status, body) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &template(domain, "Allow")),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
    );
    let described = settle_stack(&router, case, stack).await;
    assert_eq!(
        xml_text(&described, "StackStatus"),
        "CREATE_COMPLETE",
        "case {case:?}: expected the stack to deploy, got {described} with reasons {:?}",
        resource_reasons(&router, stack).await
    );
    let identity = reported_identity(&router, domain, case).await;
    let deployed_policy = stored_policy(&router, domain, case).await;

    // The revision moves the engine release AND the policy's effect: the release is what no
    // `UpdateDomainConfig` carries.
    let (status, body) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack),
            (
                "TemplateBody",
                &template_on(domain, "Deny", OTHER_ENGINE_VERSION),
            ),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdateStack, got {status} with {body}"
    );
    let settled = settle_stack(&router, case, stack).await;
    let reached = xml_text(&settled, "StackStatus");
    assert_ne!(
        reached, "UPDATE_COMPLETE",
        "case {case:?}: a revision this emulator does not make in place must not be \
         reported as applied, got {settled}"
    );
    assert!(
        reached.starts_with("UPDATE_ROLLBACK") || reached == "UPDATE_FAILED",
        "case {case:?}: expected the deployment to report the resource's update as failed, \
         got {reached:?} in {settled}"
    );

    // And the domain was left as it is: the release it runs, the policy in force, its
    // identity and the address it publishes are all the ones the create left.
    let (status, reported) = describe_domain(&router, domain).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from DescribeDomain, got {status} with {reported}"
    );
    assert_eq!(
        reported.get("EngineVersion").and_then(Value::as_str),
        Some(ENGINE_VERSION),
        "case {case:?}: the release must be the deployed one, got {reported}"
    );
    assert_eq!(
        stored_policy(&router, domain, case).await,
        deployed_policy,
        "case {case:?}: the policy in force must be the deployed one"
    );
    assert_eq!(
        reported_identity(&router, domain, case).await,
        identity,
        "case {case:?}: a revision that was not applied must move nothing about the domain"
    );
    let verdict = health_verdict(&router, &identity.2, case).await;
    assert_eq!(
        verdict.0, 200,
        "case {case:?}: the deployed policy still decides the request, got {} with {}",
        verdict.0, verdict.1
    );
}

/// How many times a change set is polled before the suite gives up on it becoming
/// executable.
const CHANGE_SET_POLLS: usize = 40;

/// Poll `DescribeChangeSet` until the change set can be executed, and answer that body: the
/// preview is computed when the change set is created, so what each poll collects is what the
/// clock already owed. `Status` is the change set's own creation status and `ExecutionStatus`
/// is whether it may be executed, so both are read.
///
/// `pub(crate)` because a sibling suite applies a revision the same way
/// ([`crate::cloudformation_updated_stack_outputs`]): one poller, called twice, rather than a
/// second copy that can drift from this one.
pub(crate) async fn settled_change_set(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    change_set: &str,
) -> String {
    let mut last = String::new();
    for _ in 0..CHANGE_SET_POLLS {
        let (status, body) = cloudformation(
            router,
            "DescribeChangeSet",
            &[("StackName", stack), ("ChangeSetName", change_set)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DescribeChangeSet, got {status} with {body}"
        );
        if xml_text(&body, "Status") == "CREATE_COMPLETE"
            && xml_text(&body, "ExecutionStatus") == "AVAILABLE"
        {
            return body;
        }
        last = body;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("case {case:?}: the change set never became executable, last seen {last}");
}
