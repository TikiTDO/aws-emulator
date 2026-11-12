// SPDX-License-Identifier: CC0-1.0

//! `AWS::ApiGateway::VpcLink` end to end: the VPC link a stack declares exists in Amazon API
//! Gateway itself, under the identifier the stack publishes.
//!
//! RULE: for a resource type whose owning service MINTS the resource's identifier, the stack
//! creates the resource through that service's own create call and publishes the identifier
//! THAT service answered as the resource's physical id — so the value a client reads out of
//! `Ref`, of the type's own identifier attribute, of `DescribeStackResource(s)` or of a stack
//! output is one the owning service answers for, the resource is discoverable through that
//! service's own collection read, and a create that service reports it cannot bring up FAILS
//! the stack's resource. A VPC link is such a resource: `CreateVpcLink` takes its `name` and
//! answers the `id` it assigned, which is what `GetVpcLink`, `GetVpcLinks` and a private
//! integration's `connectionId` take, and that create is "an asynchronous operation" whose
//! link settles `AVAILABLE` or `FAILED`.
//!
//! The rows differ along what the rule generalises over: how many load balancers the link
//! targets, whether the declaration states a description and tags, how many links one stack
//! declares — and, for the failure half, HOW a declared target misses being a network load
//! balancer (an application load balancer, a target group, a resource of another service
//! entirely, a value that is no ARN at all).
//!
//! The target ARNs are written by the rows rather than taken from a load balancer this stack
//! created: an `AWS::ElasticLoadBalancingV2::LoadBalancer` is DECLARED-ONLY in this emulator
//! — no Elastic Load Balancing call is made for one yet, which is its own issue — so what a
//! row states is an ARN of the form that service writes for a network load balancer, which is
//! what API Gateway reads a `targetArns` entry as.
//!
//! Everything goes over the wire each service speaks through the one router `stack_lifecycle.rs`
//! builds — the CloudFormation calls over its awsQuery wire, the VPC-link reads over API
//! Gateway's own `restJson1` wire — so what is asserted is what a client that deploys the
//! template and then calls API Gateway observes.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, resource_event_reason, resource_event_statuses,
    router, wait_for_the_transition, xml_blocks, xml_text, ACCOUNT_ID, REGION,
};

/// How many characters a minted VPC link id carries, and which they may be: API Gateway
/// assigns a record of the ACCOUNT ten lowercase alphanumeric characters, the width the
/// `RestApi`, `ApiKey` and `UsagePlan` ids of that service carry too.
const VPC_LINK_ID_CHARACTERS: usize = 10;

/// The `status` a VPC link that finished being provisioned reports
/// (`VpcLinkStatus`: `AVAILABLE | PENDING | DELETING | FAILED`).
const AVAILABLE: &str = "AVAILABLE";

/// How many reads a row gives a stack to reach its terminal status in: a stack is owed that
/// status once the services that own its resources report them ready, which only a read asks
/// them about, so a client polls — which is what this is.
const POLL_ATTEMPTS: usize = 20;

/// The `ResourceStatus` a resource whose create did not go through carries, which is what the
/// stack's event stream records for a link API Gateway could not provision.
const CREATE_FAILED: &str = "CREATE_FAILED";

/// How long the delete of a VPC link is given to come through.
const REMOVAL_POLLS: usize = 20;
const REMOVAL_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The ARN Elastic Load Balancing writes for the NETWORK load balancer `name`:
/// `loadbalancer/net/<name>/<id>`, the resource path that service's own ARNs carry, under the
/// `elasticloadbalancing` namespace. This is what `VpcLink$targetArns` names — "the ARN of the
/// network load balancer of the VPC targeted by the VPC link".
fn network_load_balancer_arn(name: &str) -> String {
    elb_arn(&format!("loadbalancer/net/{name}/0123456789abcdef"))
}

/// The ARN of an APPLICATION load balancer of the same account: the same ARN form with the
/// `app` type segment, which is a load balancer no VPC link can carry traffic through.
fn application_load_balancer_arn(name: &str) -> String {
    elb_arn(&format!("loadbalancer/app/{name}/0123456789abcdef"))
}

/// The ARN of a target GROUP: an Elastic Load Balancing resource that is not a load balancer
/// at all.
fn target_group_arn(name: &str) -> String {
    elb_arn(&format!("targetgroup/{name}/0123456789abcdef"))
}

/// One Elastic Load Balancing ARN over this emulator's own account and Region.
fn elb_arn(resource: &str) -> String {
    format!("arn:aws:elasticloadbalancing:{REGION}:{ACCOUNT_ID}:{resource}")
}

/// One Amazon API Gateway call over the `restJson1` wire its model declares, on the method and
/// URI that operation's own `@http` binding names, signed for `apigateway` in the Region the
/// stack's resources live in.
///
/// `pub(crate)` because `sam_api_stages.rs` reads a deployed REST API and its stages through
/// the same wire: the calling convention for this service is spelled once.
pub(crate) async fn apigateway(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://apigateway.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("apigateway.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/apigateway/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed Amazon API Gateway request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `GetVpcLinks` (`GET /vpclinks`): the VPC links the account holds, as the collection's own
/// `item` member carries them.
async fn listed_vpc_links(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let (status, document) = apigateway(router, "GET", "/vpclinks").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetVpcLinks to answer 200, got {status} with {document}"
    );
    document
        .get("item")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// `GetVpcLink` (`GET /vpclinks/{vpcLinkId}`): the status and the link one id names.
async fn get_vpc_link(router: &MultiServiceRouter, vpc_link_id: &str) -> (u16, Value) {
    apigateway(router, "GET", &format!("/vpclinks/{vpc_link_id}")).await
}

/// The text of one member of a record as API Gateway reports it.
///
/// `pub(crate)` because `sam_api_stages.rs` reads the members of an api and of its stages the
/// same way: one reading of a `restJson1` answer's member, shared with the wire helper above.
pub(crate) fn member_text(record: &Value, member: &str) -> Option<String> {
    record
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `targetArns` a reported VPC link carries, in the order it reports them.
fn reported_targets(link: &Value) -> Vec<String> {
    link.get("targetArns")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `value` is an identifier of the form API Gateway mints: exactly `width` lowercase
/// alphanumeric characters.
///
/// `pub(crate)` over an explicit width because every record API Gateway assigns an identifier
/// to carries that shape — a VPC link, a REST API (`sam_api_stages.rs`) — and the width is the
/// only thing that differs.
pub(crate) fn is_api_gateway_id(value: &str, width: usize) -> bool {
    value.chars().count() == width
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Whether `value` is an identifier of the form API Gateway mints for a VPC link: exactly
/// [`VPC_LINK_ID_CHARACTERS`] of them.
fn is_vpc_link_id(value: &str) -> bool {
    is_api_gateway_id(value, VPC_LINK_ID_CHARACTERS)
}

/// One `AWS::ApiGateway::VpcLink` declaration, as a row writes it.
fn vpc_link_resource(
    name: &str,
    targets: &[String],
    description: Option<&str>,
    tags: Option<Value>,
) -> Value {
    let mut properties = json!({ "Name": name, "TargetArns": targets });
    if let Some(description) = description {
        properties["Description"] = json!(description);
    }
    if let Some(tags) = tags {
        properties["Tags"] = tags;
    }
    json!({ "Type": "AWS::ApiGateway::VpcLink", "Properties": properties })
}

/// The outputs every row publishes per link: what `Ref` resolves to, and the identity
/// attribute the type publishes.
fn published_outputs(logical_ids: &[&str]) -> Value {
    let mut outputs = serde_json::Map::new();
    for logical_id in logical_ids {
        outputs.insert(
            format!("{logical_id}Ref"),
            json!({ "Value": { "Ref": logical_id } }),
        );
        outputs.insert(
            format!("{logical_id}VpcLinkId"),
            json!({ "Value": { "Fn::GetAtt": [logical_id, "VpcLinkId"] } }),
        );
    }
    Value::Object(outputs)
}

/// One link a row declares, and what the rule says API Gateway must then hold for it.
struct DeclaredLink {
    logical_id: &'static str,
    name: &'static str,
    targets: Vec<String>,
    description: Option<&'static str>,
}

/// One row: a template, and the links it declares.
struct LinkCase {
    case: &'static str,
    stack: &'static str,
    template: String,
    links: Vec<DeclaredLink>,
}

fn link_cases() -> Vec<LinkCase> {
    let one_target = vec![network_load_balancer_arn("vpclinkcfn-one")];
    let two_targets = vec![
        network_load_balancer_arn("vpclinkcfn-two-a"),
        network_load_balancer_arn("vpclinkcfn-two-b"),
    ];
    let described = vec![network_load_balancer_arn("vpclinkcfn-described")];
    let first = vec![network_load_balancer_arn("vpclinkcfn-pair-a")];
    let second = vec![network_load_balancer_arn("vpclinkcfn-pair-b")];
    vec![
        LinkCase {
            case: "a link over one network load balancer, with nothing else declared",
            stack: "vpclinkcfn-one",
            template: json!({
                "Resources": {
                    "Private": vpc_link_resource("vpclinkcfn-one-link", &one_target, None, None)
                },
                "Outputs": published_outputs(&["Private"])
            })
            .to_string(),
            links: vec![DeclaredLink {
                logical_id: "Private",
                name: "vpclinkcfn-one-link",
                targets: one_target.clone(),
                description: None,
            }],
        },
        LinkCase {
            case: "a link with a description and tags",
            stack: "vpclinkcfn-described",
            template: json!({
                "Resources": {
                    "Private": vpc_link_resource(
                        "vpclinkcfn-described-link",
                        &described,
                        Some("the private path to the orders VPC"),
                        Some(json!([{ "Key": "estate", "Value": "vpclinkcfn" }])),
                    )
                },
                "Outputs": published_outputs(&["Private"])
            })
            .to_string(),
            links: vec![DeclaredLink {
                logical_id: "Private",
                name: "vpclinkcfn-described-link",
                targets: described.clone(),
                description: Some("the private path to the orders VPC"),
            }],
        },
        LinkCase {
            case: "a link over two network load balancers",
            stack: "vpclinkcfn-two",
            template: json!({
                "Resources": {
                    "Private": vpc_link_resource("vpclinkcfn-two-link", &two_targets, None, None)
                },
                "Outputs": published_outputs(&["Private"])
            })
            .to_string(),
            links: vec![DeclaredLink {
                logical_id: "Private",
                name: "vpclinkcfn-two-link",
                targets: two_targets.clone(),
                description: None,
            }],
        },
        LinkCase {
            case: "two links in one stack",
            stack: "vpclinkcfn-pair",
            template: json!({
                "Resources": {
                    "Orders": vpc_link_resource("vpclinkcfn-pair-orders", &first, None, None),
                    "Billing": vpc_link_resource(
                        "vpclinkcfn-pair-billing",
                        &second,
                        Some("the billing VPC"),
                        None,
                    )
                },
                "Outputs": published_outputs(&["Orders", "Billing"])
            })
            .to_string(),
            links: vec![
                DeclaredLink {
                    logical_id: "Orders",
                    name: "vpclinkcfn-pair-orders",
                    targets: first.clone(),
                    description: None,
                },
                DeclaredLink {
                    logical_id: "Billing",
                    name: "vpclinkcfn-pair-billing",
                    targets: second.clone(),
                    description: Some("the billing VPC"),
                },
            ],
        },
    ]
}

/// Create a stack and poll it to a terminal status the way a client does, answering that
/// read's `DescribeStacks` body.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = create_stack(router, stack_name, template, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack must answer 200, got {status} {body}"
    );
    poll_to_terminal(router, case, stack_name).await
}

/// Read a stack until it reports a status no longer in progress, and answer that read's body.
async fn poll_to_terminal(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let mut described = String::new();
    for _ in 0..POLL_ATTEMPTS {
        wait_for_the_transition().await;
        let (status, body) = describe_stack(router, stack_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks must answer 200, got {status} {body}"
        );
        described = body;
        if !xml_text(&described, "StackStatus")
            .unwrap_or_default()
            .ends_with("_IN_PROGRESS")
        {
            break;
        }
    }
    described
}

/// The `PhysicalResourceId` `DescribeStackResource` reports for one logical id.
async fn described_physical_id(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackResource({stack}, {logical_id}) answered {status}: {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: expected a physical resource id for {logical_id}, got {body}")
    })
}

/// The value the stack reports for one declared output.
async fn output_value(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    key: &str,
) -> Option<String> {
    let (status, body) = describe_stack(router, stack).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStacks({stack}) answered {status}: {body}"
    );
    let outputs = xml_blocks(&body, "Outputs").into_iter().next()?;
    xml_blocks(&outputs, "member").into_iter().find_map(|entry| {
        (xml_text(&entry, "OutputKey").as_deref() == Some(key))
            .then(|| xml_text(&entry, "OutputValue"))
            .flatten()
    })
}

/// Every row: the identifier the stack publishes for a declared VPC link is the one API
/// Gateway minted, that service lists the link under it, and every reader of the stack reports
/// the same value.
#[tokio::test]
async fn a_cloudformation_vpc_link_is_created_under_the_id_the_stack_publishes() {
    let router = router();
    for case in link_cases() {
        let name = case.case;
        let described = deploy(&router, name, case.stack, &case.template).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {name:?}: expected the create to complete, got {described}"
        );

        let listed = listed_vpc_links(&router, name).await;
        let mut published: Vec<String> = Vec::new();
        for declared in &case.links {
            let logical_id = declared.logical_id;
            let id = described_physical_id(&router, name, case.stack, logical_id).await;
            assert!(
                is_vpc_link_id(&id),
                "case {name:?}: expected {logical_id} to report an id of the form API Gateway \
                 mints, got {id:?}"
            );
            assert_ne!(
                id, declared.name,
                "case {name:?}: expected the published id to be the minted identifier rather \
                 than the link's name, got {id:?}"
            );
            published.push(id.clone());

            // Every reader of the stack reports the SAME identifier.
            for (output, expected) in [
                (format!("{logical_id}Ref"), id.clone()),
                (format!("{logical_id}VpcLinkId"), id.clone()),
            ] {
                let reported = output_value(&router, name, case.stack, &output).await;
                assert_eq!(
                    reported.as_deref(),
                    Some(expected.as_str()),
                    "case {name:?}: expected the output {output:?} to report {expected:?}, got \
                     {reported:?}"
                );
            }

            // The owning service LISTS the link under that id, with the configuration the
            // declaration asked for — which is what a client filtering `GetVpcLinks` by name
            // finds.
            let by_name: Vec<&Value> = listed
                .iter()
                .filter(|link| member_text(link, "name").as_deref() == Some(declared.name))
                .collect();
            assert_eq!(
                by_name.len(),
                1,
                "case {name:?}: expected GetVpcLinks to list exactly one link named {:?}, got \
                 {listed:?}",
                declared.name
            );
            let link = by_name[0];
            assert_eq!(
                member_text(link, "id").as_deref(),
                Some(id.as_str()),
                "case {name:?}: expected the listed link to carry the published id {id:?}, got \
                 {link:?}"
            );
            let targets = reported_targets(link);
            assert_eq!(
                targets, declared.targets,
                "case {name:?}: expected the listed link to target {:?}, got {targets:?}",
                declared.targets
            );
            assert_eq!(
                member_text(link, "description").as_deref(),
                declared.description,
                "case {name:?}: expected the listed link to report the declared description, \
                 got {link:?}"
            );

            // ...and answers its own read for that id, reporting the same record.
            let (status, read) = get_vpc_link(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: expected GetVpcLink({id}) to answer 200, got {status} with \
                 {read}"
            );
            for (reported_member, expected) in [
                ("id", Some(id.clone())),
                ("name", Some(declared.name.to_string())),
                ("status", Some(AVAILABLE.to_string())),
                ("description", declared.description.map(str::to_string)),
            ] {
                let held = member_text(&read, reported_member);
                assert_eq!(
                    held, expected,
                    "case {name:?}: expected GetVpcLink to report {reported_member:?} as \
                     {expected:?}, got {held:?} in {read}"
                );
            }
            assert_eq!(
                reported_targets(&read),
                declared.targets,
                "case {name:?}: expected GetVpcLink to report the declared targets, got {read}"
            );
        }

        // Two links of one stack are two links: each carries an identifier of its own.
        let mut unique = published.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            published.len(),
            "case {name:?}: expected one minted identifier per link, got {published:?}"
        );
    }
}

/// One row of the failure half: a target the owning service cannot provision a link over, and
/// how it misses being a network load balancer.
struct FailureCase {
    case: &'static str,
    stack: &'static str,
    link_name: &'static str,
    target: String,
}

fn failure_cases() -> Vec<FailureCase> {
    vec![
        FailureCase {
            case: "an application load balancer",
            stack: "vpclinkcfn-alb",
            link_name: "vpclinkcfn-alb-link",
            target: application_load_balancer_arn("vpclinkcfn-alb"),
        },
        FailureCase {
            case: "a target group",
            stack: "vpclinkcfn-tg",
            link_name: "vpclinkcfn-tg-link",
            target: target_group_arn("vpclinkcfn-tg"),
        },
        FailureCase {
            case: "a resource of another service entirely",
            stack: "vpclinkcfn-queue",
            link_name: "vpclinkcfn-queue-link",
            target: format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:vpclinkcfn-orders"),
        },
        FailureCase {
            case: "a value that is no ARN at all",
            stack: "vpclinkcfn-garbage",
            link_name: "vpclinkcfn-garbage-link",
            target: "the-orders-load-balancer".to_string(),
        },
    ]
}

/// A link whose declared target is no network load balancer is a link API Gateway cannot
/// provision: the stack's resource FAILS and the stack rolls back, and the link is left
/// nowhere for a client to find.
#[tokio::test]
async fn a_link_over_no_network_load_balancer_fails_the_resource_and_rolls_the_stack_back() {
    let router = router();
    for case in failure_cases() {
        let name = case.case;
        let template = json!({
            "Resources": {
                "Private": vpc_link_resource(
                    case.link_name,
                    std::slice::from_ref(&case.target),
                    None,
                    None,
                )
            },
            "Outputs": published_outputs(&["Private"])
        })
        .to_string();
        let described = deploy(&router, name, case.stack, &template).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {name:?}: expected the stack to roll back, got {described}"
        );

        let statuses = resource_event_statuses(&router, name, case.stack, "Private").await;
        assert!(
            statuses.iter().any(|status| status == CREATE_FAILED),
            "case {name:?}: expected a {CREATE_FAILED} event for the link, got {statuses:?}"
        );
        let reason = resource_event_reason(&router, case.stack, "Private", CREATE_FAILED).await;
        // The reason is the OWNING service's own account of why the link cannot carry
        // traffic, relayed unchanged: whether API Gateway refused the create outright or
        // settled the link `FAILED`, the sentence names the target the caller has to change.
        assert!(
            reason.contains(&case.target),
            "case {name:?}: expected the failure to name the target {:?} as API Gateway\
             reported it, got {reason:?}",
            case.target
        );

        let listed = listed_vpc_links(&router, name).await;
        let left_behind: Vec<&Value> = listed
            .iter()
            .filter(|link| member_text(link, "name").as_deref() == Some(case.link_name))
            .collect();
        assert!(
            left_behind.is_empty(),
            "case {name:?}: expected the rolled-back stack to leave no link named {:?}, got \
             {left_behind:?}",
            case.link_name
        );
    }
}

/// Deleting the stack deletes the VPC link from the service that holds it: the identifier the
/// stack published stops resolving, which is what says the stack's resource really was that
/// service's.
#[tokio::test]
async fn deleting_the_stack_removes_the_vpc_link_from_api_gateway() {
    let case = "a deleted vpc link stack";
    let router = router();
    let stack = "vpclinkcfn-deleted";
    let targets = vec![network_load_balancer_arn("vpclinkcfn-deleted")];
    let template = json!({
        "Resources": {
            "Private": vpc_link_resource("vpclinkcfn-deleted-link", &targets, None, None)
        },
        "Outputs": published_outputs(&["Private"])
    })
    .to_string();
    let described = deploy(&router, case, stack, &template).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the create to complete, got {described}"
    );
    let id = described_physical_id(&router, case, stack, "Private").await;
    let (status, read) = get_vpc_link(&router, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the published id to be one GetVpcLink accepts, got {status} \
         with {read}"
    );

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteStack({stack}) answered {status}: {body}"
    );
    let mut gone = false;
    for _ in 0..REMOVAL_POLLS {
        let (status, _) = get_vpc_link(&router, &id).await;
        if status == 404 {
            gone = true;
            break;
        }
        tokio::time::sleep(REMOVAL_POLL_INTERVAL).await;
        // Every read of the stack is what asks the owning service whether its delete has come
        // through.
        let _ = call(&router, "ListStacks", &[]).await;
    }
    assert!(
        gone,
        "case {case:?}: expected the VPC link {id:?} to be removed from the service that held it"
    );
}
