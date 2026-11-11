// SPDX-License-Identifier: CC0-1.0

//! A stack whose `AWS::EC2::Instance` names an instance type Amazon EC2 will not launch FAILS
//! that resource and rolls back, while the same stack naming a published type settles at
//! `CREATE_COMPLETE` with an instance that service really holds.
//!
//! RULE: a stack's resource is created by the OWNING SERVICE's own create call, so that call's
//! refusal is the RESOURCE's failure — CloudFormation records a `CREATE_FAILED` event for the
//! logical resource carrying the owning service's own message, asks nothing of any service for
//! the resources that depend on it (so they get no event at all), and rolls the stack back to
//! `ROLLBACK_COMPLETE` with nothing left behind. Driven over `AWS::EC2::Instance`, whose create
//! is `RunInstances`: an instance type outside the catalog Amazon EC2 publishes is a launch
//! that service refuses, so a template that cannot work in Amazon EC2 cannot deploy here
//! either.
//!
//! The rows differ along what the rule generalises over — whether the instance type the
//! template declares is one Amazon EC2 publishes, and HOW an unpublished one misses the catalog
//! (a family that does not exist, a size the catalog does not carry, a family stated with no
//! size at all). WHICH instance types exist is Amazon EC2's own catalog, which that crate's own
//! suite covers value by value; what this suite is for is the consequence for the STACK.
//!
//! Three services have to agree for any of it, all mounted on the one engine this crate's
//! harness builds: CloudFormation makes the calls, Amazon EC2 serves the launch and the reads,
//! and AWS Systems Manager answers the public parameter the template resolves its AMI from.
//!
//! The stack's `AWS::SSM::Association` is created in AWS Systems Manager itself, like every
//! other resource here: it is in the template because it is what DEPENDS on the instance —
//! through a `Ref` to it — and the rule's second consequence is about the dependent, so what
//! this suite reads about it is whether the stack recorded an event for it at all. Its
//! parameters are the ones the document it applies requires, which is what Systems Manager
//! holds any association to; `ssm_association_resources.rs` is the suite that pins what the
//! association itself then reports.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    create_stack, describe_stack, ec2, is_service_assigned_id, member, router_with_engine,
    wait_for_the_transition, xml_blocks, xml_text, xml_texts,
};

/// The Availability Zone the stack's subnet is chosen in: the Region's first, which offers
/// every instance type the catalog publishes (Amazon EC2 does not offer every type in every
/// zone, and which zone lacks which is that service's own answer).
const ZONE: &str = "us-east-1a";

/// The public Systems Manager parameter the template resolves its AMI from — a name in AWS's
/// own `/aws/service/` hierarchy, which no account's store holds and the published catalog
/// answers for.
const IMAGE_PARAMETER: &str =
    "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64";

/// The logical ids the template declares: the instance, and the resource that depends on it.
const INSTANCE: &str = "TheInstance";
const DEPENDENT: &str = "TheAssociation";

/// How many reads a row gives a stack to reach its terminal status in: a stack is owed that
/// status once the services that own its resources report them ready, which only a read asks
/// them about, so a client polls — which is what this is.
const POLL_ATTEMPTS: usize = 20;

/// A subnet of the account's default VPC in [`ZONE`], read out of Amazon EC2 itself: the
/// template cannot invent a subnet id, and the zone is CHOSEN rather than taken by position so
/// a published instance type is one that zone offers.
async fn subnet_in_the_first_zone(router: &MultiServiceRouter) -> String {
    let (status, body) = ec2(
        router,
        "DescribeSubnets",
        &[
            ("Filter.1.Name", "availability-zone"),
            ("Filter.1.Value.1", ZONE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeSubnets must report the default VPC's subnets, got {status} with body {body}"
    );
    xml_text(&body, "subnetId")
        .unwrap_or_else(|| panic!("a fresh account must hold a subnet in {ZONE}: {body}"))
}

/// The template every row deploys: an instance whose AMI comes from the public Systems Manager
/// parameter above, and the resource that DEPENDS on it through a `Ref`.
fn template(instance_type: &str, subnet_id: &str) -> String {
    let declared: Value = json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Parameters": {
            "ImageId": {
                "Type": "AWS::SSM::Parameter::Value<AWS::EC2::Image::Id>",
                "Default": IMAGE_PARAMETER,
            }
        },
        "Resources": {
            INSTANCE: {
                "Type": "AWS::EC2::Instance",
                "Properties": {
                    "ImageId": { "Ref": "ImageId" },
                    "InstanceType": instance_type,
                    "SubnetId": subnet_id,
                }
            },
            DEPENDENT: {
                "Type": "AWS::SSM::Association",
                "Properties": {
                    "Name": "AWS-RunShellScript",
                    // The one parameter that document declares without a default, which
                    // Systems Manager requires of every association applying it.
                    "Parameters": { "commands": ["echo deployed"] },
                    "Targets": [
                        { "Key": "InstanceIds", "Values": [{ "Ref": INSTANCE }] }
                    ]
                }
            }
        },
        "Outputs": {
            "InstanceId": { "Value": { "Ref": INSTANCE } }
        }
    });
    declared.to_string()
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
/// The last read stands where the stack is still in progress after the bound, so the caller's
/// own assertion reports what it was waiting for.
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

/// Every event the stack recorded, as `(logical id, resource status, status reason)`.
async fn stack_events(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
) -> Vec<(String, String, String)> {
    let (status, body) = crate::stack_lifecycle::call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackEvents must answer 200, got {status} {body}"
    );
    xml_blocks(&body, "member")
        .into_iter()
        .map(|event| {
            let read = |tag: &str| xml_texts(&event, tag).into_iter().next().unwrap_or_default();
            (
                read("LogicalResourceId"),
                read("ResourceStatus"),
                read("ResourceStatusReason"),
            )
        })
        .collect()
}

/// The instance ids Amazon EC2 reports for the instances one stack created, found the way a
/// client finds them: by the tag CloudFormation records the stack's name under.
async fn instances_of_the_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
) -> Vec<String> {
    let (status, body) = ec2(
        router,
        "DescribeInstances",
        &[
            ("Filter.1.Name", "tag:aws:cloudformation:stack-name"),
            ("Filter.1.Value.1", stack_name),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstances must answer 200, got {status} {body}"
    );
    xml_texts(&body, "instanceId")
}

/// A declared instance type Amazon EC2 will not launch fails THAT resource, skips the resource
/// that depends on it and rolls the stack back with no instance left behind; the same stack
/// naming a published type reaches `CREATE_COMPLETE`, publishes the id Amazon EC2 assigned and
/// is readable there.
#[tokio::test]
async fn a_declared_instance_type_amazon_ec2_refuses_rolls_the_stack_back() {
    // (case, the InstanceType the template declares, whether Amazon EC2 launches it)
    let rows: &[(&str, &str, bool)] = &[
        ("a published current-generation type", "t3.micro", true),
        ("a published previous-generation type", "m1.small", true),
        (
            "a family Amazon EC2 does not have",
            "sim2real.invalid-type",
            false,
        ),
        ("a size the family does not offer", "t3.enormous", false),
        ("a family stated with no size", "m5", false),
    ];

    let (router, _engine) = router_with_engine();
    let subnet_id = subnet_in_the_first_zone(&router).await;
    for (index, (case, instance_type, launches)) in rows.iter().enumerate() {
        let stack_name = format!("instance-type-row-{index}");
        let described = deploy(
            &router,
            case,
            &stack_name,
            &template(instance_type, &subnet_id),
        )
        .await;
        let status = xml_text(&described, "StackStatus").unwrap_or_default();
        let events = stack_events(&router, case, &stack_name).await;
        let found = instances_of_the_stack(&router, case, &stack_name).await;

        if *launches {
            assert_eq!(
                status, "CREATE_COMPLETE",
                "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {status:?}; \
                 its events: {events:?}"
            );
            // The id the stack publishes is the one Amazon EC2 assigned, not a name
            // CloudFormation computed — which is what makes it an id that service answers for.
            let published = xml_texts(&described, "OutputValue")
                .into_iter()
                .next()
                .unwrap_or_default();
            assert!(
                is_service_assigned_id(&published, "i"),
                "case {case:?}: expected the stack to publish an Amazon EC2 instance id, got \
                 {published:?} in {described}"
            );
            assert_eq!(
                found,
                vec![published.clone()],
                "case {case:?}: expected Amazon EC2 to report exactly the instance the stack \
                 published ({published}), got {found:?}"
            );
            // And it is the instance the template DECLARED: the launch carried the declared
            // type, the declared subnet and the AMI the Systems Manager parameter resolved to.
            let (read_status, reported) =
                ec2(&router, "DescribeInstances", &[("InstanceId.1", &published)]).await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: expected Amazon EC2 to hold {published}, got {read_status} \
                 with body {reported}"
            );
            let reported_type = xml_text(&reported, "instanceType").unwrap_or_default();
            assert_eq!(
                reported_type, *instance_type,
                "case {case:?}: expected the instance to report {instance_type:?}, got \
                 {reported_type:?} in {reported}"
            );
            let reported_subnet = xml_text(&reported, "subnetId").unwrap_or_default();
            assert_eq!(
                reported_subnet, subnet_id,
                "case {case:?}: expected the instance in the declared subnet ({subnet_id}), \
                 got {reported_subnet:?}"
            );
            let reported_image = xml_text(&reported, "imageId").unwrap_or_default();
            assert!(
                is_service_assigned_id(&reported_image, "ami"),
                "case {case:?}: expected the launch to resolve the parameter to an AMI id, got \
                 {reported_image:?}"
            );
            // The resource that depends on the instance WAS asked for once the instance came
            // up, which is the other half of the skip below.
            assert!(
                events.iter().any(|(logical, _, _)| logical == DEPENDENT),
                "case {case:?}: expected the dependent {DEPENDENT} to be recorded once the \
                 instance came up, got {events:?}"
            );
            continue;
        }

        assert_eq!(
            status, "ROLLBACK_COMPLETE",
            "case {case:?}: expected the stack to reach ROLLBACK_COMPLETE, got {status:?}; its \
             events: {events:?}"
        );
        // The failure is attributable to the instance type: the event is the INSTANCE's, and
        // the reason carries the message Amazon EC2 refused the launch with.
        let failed: Vec<&(String, String, String)> = events
            .iter()
            .filter(|(logical, resource_status, _)| {
                logical == INSTANCE && resource_status == "CREATE_FAILED"
            })
            .collect();
        assert!(
            !failed.is_empty(),
            "case {case:?}: expected a CREATE_FAILED event for {INSTANCE}, got {events:?}"
        );
        let expected = format!("Invalid value '{instance_type}' for InstanceType.");
        assert!(
            failed
                .iter()
                .any(|(_, _, reason)| reason.contains(&expected)),
            "case {case:?}: expected the failure reason to carry {expected:?}, got {failed:?}"
        );
        // The dependent was never asked for, so the stack recorded NO event for it at all.
        let dependent: Vec<&(String, String, String)> = events
            .iter()
            .filter(|(logical, _, _)| logical == DEPENDENT)
            .collect();
        assert!(
            dependent.is_empty(),
            "case {case:?}: expected no event for the dependent {DEPENDENT}, got {dependent:?}"
        );
        // And nothing was left behind: no instance of this stack exists in Amazon EC2.
        assert!(
            found.is_empty(),
            "case {case:?}: expected Amazon EC2 to hold no instance of the rolled-back stack, \
             got {found:?}"
        );
    }
}

/// Deleting a stack that DID launch its instance takes that instance away in Amazon EC2: the
/// resource the stack created is the one its teardown terminates, so the account is left
/// holding nothing the deleted stack made.
#[tokio::test]
async fn deleting_the_stack_terminates_the_instance_it_launched() {
    let case = "a stack whose instance launched";
    let stack_name = "instance-teardown";
    let (router, _engine) = router_with_engine();
    let subnet_id = subnet_in_the_first_zone(&router).await;

    let described = deploy(&router, case, stack_name, &template("t3.micro", &subnet_id)).await;
    let status = xml_text(&described, "StackStatus").unwrap_or_default();
    let events = stack_events(&router, case, stack_name).await;
    assert_eq!(
        status, "CREATE_COMPLETE",
        "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {status:?}; its \
         events: {events:?}"
    );
    let instance_id = xml_texts(&described, "OutputValue")
        .into_iter()
        .next()
        .unwrap_or_default();

    let (delete_status, body) = crate::stack_lifecycle::call(
        &router,
        "DeleteStack",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        delete_status, 200,
        "case {case:?}: DeleteStack must answer 200, got {delete_status} {body}"
    );
    // The deletion is polled the way a client does, and a deleted stack stops resolving by
    // NAME — "Stack with id … does not exist" — so either answer ends the wait.
    for _ in 0..POLL_ATTEMPTS {
        wait_for_the_transition().await;
        let (poll_status, polled) = describe_stack(&router, stack_name).await;
        if poll_status != 200
            || !xml_text(&polled, "StackStatus")
                .unwrap_or_default()
                .ends_with("_IN_PROGRESS")
        {
            break;
        }
    }

    let (read_status, reported) =
        ec2(&router, "DescribeInstances", &[("InstanceId.1", &instance_id)]).await;
    assert_eq!(
        read_status, 200,
        "case {case:?}: DescribeInstances must answer 200, got {read_status} {reported}"
    );
    // Amazon EC2 keeps a terminated instance rather than removing it, so what says the
    // teardown terminated the instance is the state it reports it in — read inside the
    // instance's own `instanceState`, since `name` is an element many members carry.
    let state = xml_blocks(&reported, "instanceState")
        .into_iter()
        .next()
        .and_then(|reported_state| xml_text(&reported_state, "name"))
        .unwrap_or_default();
    assert!(
        ["shutting-down", "terminated"].contains(&state.as_str()),
        "case {case:?}: expected Amazon EC2 to report {instance_id} on its way out, got \
         {state:?} in {reported}"
    );
}
