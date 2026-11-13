// SPDX-License-Identifier: CC0-1.0

//! What a client does with a deployed Elastic Load Balancing v2 estate AFTER the create: it
//! drives it on that service's own API, through every stack operation the deployment's life
//! takes.
//!
//! The rule under test: **the Elastic Load Balancing resources a stack declares live in
//! Elastic Load Balancing's own state for the whole life of the stack — so every identifier
//! the stack publishes resolves there on the first read, an update to a declared property is
//! applied to that resource through the owning service's own modify call and keeps its
//! identity, a change to a property that service cannot change replaces the resource and
//! re-points the resources that reference it, and a delete that service refuses fails the
//! stack instead of reporting a deployment that removed nothing.**
//!
//! That is the whole of the flow an IaC-plus-verification pipeline runs, and the reason it is
//! pinned here rather than in a single read: the identifiers a `CREATE_COMPLETE` stack exports
//! are the handles for everything that follows, so a listener ARN `DescribeRules` answers
//! `ListenerNotFound` for leaves not only the first read broken but every later assertion
//! unreachable — the rule ordering, the in-place rule update, the group replacement and the
//! negative paths deletion protection guards.
//!
//! `elbv2_resource_provisioning.rs` beside this suite pins the CREATE half of the rule over
//! the load-balancer families; this one picks up where it stops, at the stack's second
//! operation.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, elbv2, list_members, member, output_value,
    resource_event_reason, router, xml_text, xml_texts,
};

/// How long a case waits for a stack operation's terminal status before it gives up.
///
/// Generous on purpose: what the case needs is the CONDITION (a status that is no longer
/// `*_IN_PROGRESS`), which it polls for; the deadline is only the backstop that turns a stack
/// nothing will ever settle into a readable failure instead of a hang.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long the poll loop waits between two reads of a stack's status.
const POLL_SPACING: Duration = Duration::from_millis(50);

/// The values a template DECLARES for the estate, so a case can deploy one template and then
/// update the stack with the same template carrying one changed value.
#[derive(Clone, Copy, Debug)]
struct Declared {
    /// `Priority` of the first listener rule.
    first_priority: &'static str,
    /// `MessageBody` of the first rule's fixed-response action.
    first_body: &'static str,
    /// The listener's `Port`.
    listener_port: &'static str,
    /// The target group's `HealthCheckPath`.
    health_check_path: &'static str,
    /// The target group's `Name` — a property Elastic Load Balancing cannot change on the
    /// group that exists, so a new one asks for a new group.
    target_group_name: &'static str,
    /// The load balancer's `deletion_protection.enabled` attribute.
    deletion_protection: &'static str,
}

/// What every case deploys first: the estate of the flow the issue filed — a network, an
/// internet-facing Application Load Balancer, a target group, an HTTP listener forwarding to
/// it and two rules on that listener at priorities 10 and 50.
const BASE: Declared = Declared {
    first_priority: "10",
    first_body: "v1",
    listener_port: "80",
    health_check_path: "/",
    target_group_name: "flow-targets",
    deletion_protection: "false",
};

/// The whole template for `declared`.
fn template(declared: &Declared) -> String {
    let Declared {
        first_priority,
        first_body,
        listener_port,
        health_check_path,
        target_group_name,
        deletion_protection,
    } = declared;
    format!(
        "Resources:
  Vpc:
    Type: AWS::EC2::VPC
    Properties:
      CidrBlock: 10.9.0.0/16
  SubnetA:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.9.1.0/24
      AvailabilityZone: us-east-1a
  SubnetB:
    Type: AWS::EC2::Subnet
    Properties:
      VpcId: !Ref Vpc
      CidrBlock: 10.9.2.0/24
      AvailabilityZone: us-east-1b
  Igw:
    Type: AWS::EC2::InternetGateway
  Attachment:
    Type: AWS::EC2::VPCGatewayAttachment
    Properties:
      VpcId: !Ref Vpc
      InternetGatewayId: !Ref Igw
  Sg:
    Type: AWS::EC2::SecurityGroup
    Properties:
      GroupDescription: the group the load balancer is placed in
      VpcId: !Ref Vpc
  Alb:
    Type: AWS::ElasticLoadBalancingV2::LoadBalancer
    Properties:
      Type: application
      Scheme: internet-facing
      Subnets:
        - !Ref SubnetA
        - !Ref SubnetB
      SecurityGroups:
        - !Ref Sg
      LoadBalancerAttributes:
        - Key: deletion_protection.enabled
          Value: \"{deletion_protection}\"
  Tg:
    Type: AWS::ElasticLoadBalancingV2::TargetGroup
    Properties:
      Name: {target_group_name}
      Port: 80
      Protocol: HTTP
      TargetType: ip
      VpcId: !Ref Vpc
      HealthCheckPath: {health_check_path}
  Listener:
    Type: AWS::ElasticLoadBalancingV2::Listener
    Properties:
      LoadBalancerArn: !Ref Alb
      Port: {listener_port}
      Protocol: HTTP
      DefaultActions:
        - Type: forward
          TargetGroupArn: !Ref Tg
  RuleFirst:
    Type: AWS::ElasticLoadBalancingV2::ListenerRule
    Properties:
      ListenerArn: !Ref Listener
      Priority: {first_priority}
      Conditions:
        - Field: path-pattern
          Values:
            - /a
      Actions:
        - Type: fixed-response
          FixedResponseConfig:
            StatusCode: \"200\"
            ContentType: text/plain
            MessageBody: {first_body}
  RuleSecond:
    Type: AWS::ElasticLoadBalancingV2::ListenerRule
    Properties:
      ListenerArn: !Ref Listener
      Priority: 50
      Conditions:
        - Field: path-pattern
          Values:
            - /a
      Actions:
        - Type: fixed-response
          FixedResponseConfig:
            StatusCode: \"200\"
            ContentType: text/plain
            MessageBody: v2
Outputs:
  AlbArn:
    Value: !Ref Alb
  AlbDns:
    Value: !GetAtt Alb.DNSName
  ListenerArn:
    Value: !Ref Listener
  TgArn:
    Value: !Ref Tg
  RuleFirstArn:
    Value: !Ref RuleFirst
  RuleSecondArn:
    Value: !Ref RuleSecond
  VpcId:
    Value: !Ref Vpc
"
    )
}

/// The outputs every template above declares, which is the map a client reads a deployment's
/// identifiers out of.
const OUTPUT_KEYS: &[&str] = &[
    "AlbArn",
    "AlbDns",
    "ListenerArn",
    "TgArn",
    "RuleFirstArn",
    "RuleSecondArn",
    "VpcId",
];

/// Read a stack until it reports a status that is no longer in progress, and answer that
/// read's body.
///
/// The WAIT IS FOR THE CONDITION: the loop looks again until the status has settled, and
/// [`SETTLE_DEADLINE`] is only the point at which a status that never settles is reported as
/// such.
async fn settle(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let started = std::time::Instant::now();
    loop {
        let (status_code, body) = describe_stack(router, stack).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: DescribeStacks({stack}) must answer 200, got {status_code} {body}"
        );
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") {
            return body;
        }
        assert!(
            started.elapsed() < SETTLE_DEADLINE,
            "case {case:?}: the stack operation never settled; last status {reported:?}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Deploy `declared` as a new stack and answer the stack id and the described stack, having
/// asserted it reached `CREATE_COMPLETE` with every output resolved.
async fn deploy(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    declared: &Declared,
) -> (String, String) {
    let (status_code, body) = create_stack(router, stack, &template(declared), &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: CreateStack must answer 200, got {status_code} {body}"
    );
    let stack_id = xml_text(&body, "StackId").unwrap_or_default();
    assert!(
        !stack_id.is_empty(),
        "case {case:?}: CreateStack must answer the stack's id, got {body}"
    );
    let described = settle(router, case, &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: the stack must reach CREATE_COMPLETE, got {described}"
    );
    for key in OUTPUT_KEYS {
        assert!(
            output_value(&described, key).is_some(),
            "case {case:?}: every declared output of a CREATE_COMPLETE stack carries an \
             OutputValue; {key:?} carries none, got {described}"
        );
    }
    (stack_id, described)
}

/// The identifiers a described stack publishes, as the map a client reads them into.
fn identifiers(described: &str) -> Vec<(&'static str, String)> {
    OUTPUT_KEYS
        .iter()
        .map(|key| (*key, output_value(described, key).unwrap_or_default()))
        .collect()
}

/// The value one output carries.
fn identifier(case: &str, described: &str, key: &str) -> String {
    output_value(described, key)
        .unwrap_or_else(|| panic!("case {case:?}: no {key:?} output, got {described}"))
}

/// Every rule a listener holds, as the pairs (`RuleArn`, `Priority`) `DescribeRules` reports,
/// in the order it reports them.
///
/// Each pair is read INSIDE the `Rules` entry that carries it, so a `RuleArn` and a `Priority`
/// belonging to two different rules cannot be paired — the depth-aware scan of the list is
/// what makes that sound for an entry carrying nested `member` lists of its own (a rule's
/// conditions and actions).
async fn rules_of(
    router: &MultiServiceRouter,
    case: &str,
    listener_arn: &str,
) -> Vec<(String, String)> {
    let (status_code, described) =
        elbv2(router, "DescribeRules", &[member("ListenerArn", listener_arn)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeRules on the listener the stack published ({listener_arn:?}) \
         must answer 200, got {status_code} {described}"
    );
    list_members(&described, "Rules")
        .iter()
        .map(|rule| {
            (
                xml_text(rule, "RuleArn").unwrap_or_default(),
                xml_text(rule, "Priority").unwrap_or_default(),
            )
        })
        .collect()
}

/// What a case asserts the owning service reports once the stack's update has been applied.
#[derive(Clone, Copy, Debug)]
enum Reported {
    /// The answer carries `element` with this text.
    Member {
        element: &'static str,
        value: &'static str,
    },
    /// The answer's `Key`/`Value` attribute pairs carry this pair.
    Attribute {
        key: &'static str,
        value: &'static str,
    },
}

/// One row: a declared value changed on a deployed stack, and the read through which the
/// owning service must report the change.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The template the stack is UPDATED with — `BASE` with one value changed.
    updated: Declared,
    /// The Elastic Load Balancing read that must report the change: the action, the request
    /// member naming the resource, and the stack output whose value that member takes.
    read: (&'static str, &'static str, &'static str),
    /// What that read must then report.
    reported: Reported,
}

/// The rows: one per Elastic Load Balancing resource type a stack declares, each changing a
/// property of a DIFFERENT kind — a rule's position among its siblings, the body a rule's own
/// action renders, a scalar of the listener itself, the target group's health check, and an
/// attribute that is a second call of the owning service rather than a member of its update.
const ROWS: &[Row] = &[
    Row {
        case: "a listener rule's priority",
        stack: "elbv2-flow-priority",
        updated: Declared {
            first_priority: "5",
            ..BASE
        },
        read: ("DescribeRules", "ListenerArn", "ListenerArn"),
        reported: Reported::Member {
            element: "Priority",
            value: "5",
        },
    },
    Row {
        case: "a listener rule's fixed-response body",
        stack: "elbv2-flow-body",
        updated: Declared {
            first_body: "v1b",
            ..BASE
        },
        read: ("DescribeRules", "ListenerArn", "ListenerArn"),
        reported: Reported::Member {
            element: "MessageBody",
            value: "v1b",
        },
    },
    Row {
        case: "the listener's port",
        stack: "elbv2-flow-port",
        updated: Declared {
            listener_port: "8080",
            ..BASE
        },
        read: ("DescribeListeners", "ListenerArns.member.1", "ListenerArn"),
        reported: Reported::Member {
            element: "Port",
            value: "8080",
        },
    },
    Row {
        case: "the target group's health check path",
        stack: "elbv2-flow-health",
        updated: Declared {
            health_check_path: "/healthz",
            ..BASE
        },
        read: (
            "DescribeTargetGroups",
            "TargetGroupArns.member.1",
            "TgArn",
        ),
        reported: Reported::Member {
            element: "HealthCheckPath",
            value: "/healthz",
        },
    },
    Row {
        case: "the load balancer's deletion-protection attribute",
        stack: "elbv2-flow-attribute",
        updated: Declared {
            deletion_protection: "true",
            ..BASE
        },
        read: (
            "DescribeLoadBalancerAttributes",
            "LoadBalancerArn",
            "AlbArn",
        ),
        reported: Reported::Attribute {
            key: "deletion_protection.enabled",
            value: "true",
        },
    },
];

/// RULE: the first Elastic Load Balancing read of an identifier a `CREATE_COMPLETE` stack
/// exports resolves there, and an in-place stack update is applied to that very resource
/// through the owning service's own call without changing any resource's identity.
#[tokio::test]
async fn a_stack_update_is_applied_to_the_load_balancing_resources_it_declared() {
    for row in ROWS {
        let case = row.case;
        // One router — so one engine — per row: a row's estate is its own, and the names its
        // template declares are account-unique in the service that holds them.
        let router = router();
        let (stack_id, described) = deploy(&router, case, row.stack, &BASE).await;

        // The read the issue filed, on every row: the listener ARN the stack exported carries
        // the stack's two rules, at the priorities the template declared, and the default rule
        // the listener was created with.
        let listener_arn = identifier(case, &described, "ListenerArn");
        let first_arn = identifier(case, &described, "RuleFirstArn");
        let second_arn = identifier(case, &described, "RuleSecondArn");
        let rules = rules_of(&router, case, &listener_arn).await;
        for (rule_arn, priority) in [(&first_arn, "10"), (&second_arn, "50")] {
            assert!(
                rules
                    .iter()
                    .any(|(arn, held)| arn == rule_arn && held == priority),
                "case {case:?}: expected the listener's rules to carry {rule_arn:?} at priority \
                 {priority:?}, got {rules:?}"
            );
        }
        let before = identifiers(&described);

        let (status_code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", &stack_id),
                member("TemplateBody", &template(&row.updated)),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: UpdateStack must answer 200, got {status_code} {body}"
        );
        let described = settle(&router, case, &stack_id).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::UPDATE_COMPLETE),
            "case {case:?}: the stack must reach UPDATE_COMPLETE, got {described}"
        );

        // None of these changes is a property the owning service cannot change, so every
        // resource the stack published keeps the identity a client already holds.
        assert_eq!(
            identifiers(&described),
            before,
            "case {case:?}: an in-place update must keep every identifier the stack published"
        );

        let (action, request_member, naming_output) = row.read;
        let addressed = identifier(case, &described, naming_output);
        let (status_code, answered) =
            elbv2(&router, action, &[member(request_member, &addressed)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: {action} on {addressed:?} must answer 200, got {status_code} \
             {answered}"
        );
        match row.reported {
            Reported::Member { element, value } => {
                let held = xml_texts(&answered, element);
                assert!(
                    held.iter().any(|reported| reported == value),
                    "case {case:?}: expected {action} to report {element} {value:?}, got \
                     {held:?} in {answered}"
                );
            }
            Reported::Attribute { key, value } => {
                // Each pair is read inside the `Attributes` entry that carries it, so one
                // attribute's key is never paired with another's value.
                let held: Vec<(String, String)> = list_members(&answered, "Attributes")
                    .iter()
                    .map(|attribute| {
                        (
                            xml_text(attribute, "Key").unwrap_or_default(),
                            xml_text(attribute, "Value").unwrap_or_default(),
                        )
                    })
                    .collect();
                assert!(
                    held.iter()
                        .any(|(held_key, held_value)| held_key == key && held_value == value),
                    "case {case:?}: expected {action} to report {key:?} as {value:?}, got \
                     {held:?}"
                );
            }
        }
    }
}

/// RULE, the other half of an update: a change to a property the owning service cannot change
/// on the resource that exists REPLACES that resource there — the new one exists, the old one
/// is gone, and the resources that reference it are re-pointed at the new one while keeping
/// their own identity.
#[tokio::test]
async fn a_change_the_owning_service_cannot_apply_replaces_the_resource_it_names() {
    let case = "the target group's name, which Elastic Load Balancing cannot change";
    let router = router();
    let (stack_id, described) = deploy(&router, case, "elbv2-flow-replace", &BASE).await;
    let listener_arn = identifier(case, &described, "ListenerArn");
    let first_arn = identifier(case, &described, "RuleFirstArn");
    let replaced_group = identifier(case, &described, "TgArn");

    let (status_code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", &stack_id),
            member(
                "TemplateBody",
                &template(&Declared {
                    target_group_name: "flow-targets-next",
                    ..BASE
                }),
            ),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: UpdateStack must answer 200, got {status_code} {body}"
    );
    let described = settle(&router, case, &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::UPDATE_COMPLETE),
        "case {case:?}: the stack must reach UPDATE_COMPLETE, got {described}"
    );

    // The group is a NEW one, and the resources that merely reference it are the same.
    let group = identifier(case, &described, "TgArn");
    assert_ne!(
        group, replaced_group,
        "case {case:?}: expected the replaced group to be published under a new ARN"
    );
    assert_eq!(
        identifier(case, &described, "ListenerArn"),
        listener_arn,
        "case {case:?}: expected the listener to be updated in place, not replaced with it"
    );
    assert_eq!(
        identifier(case, &described, "RuleFirstArn"),
        first_arn,
        "case {case:?}: expected the rule on that listener to keep its own ARN"
    );

    // The new group exists in Elastic Load Balancing under the name the template now declares…
    let (status_code, answered) = elbv2(
        &router,
        "DescribeTargetGroups",
        &[member("TargetGroupArns.member.1", &group)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeTargetGroups on the new group must answer 200, got \
         {status_code} {answered}"
    );
    assert_eq!(
        xml_text(&answered, "TargetGroupName").as_deref(),
        Some("flow-targets-next"),
        "case {case:?}: expected the new group to carry the declared name, got {answered}"
    );

    // …the replaced one is gone from it…
    let (status_code, answered) = elbv2(
        &router,
        "DescribeTargetGroups",
        &[member("TargetGroupArns.member.1", &replaced_group)],
    )
    .await;
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the replaced group to be gone, got {status_code} {answered}"
    );
    assert_eq!(
        xml_text(&answered, "Code").as_deref(),
        Some("TargetGroupNotFound"),
        "case {case:?}: expected TargetGroupNotFound for the replaced group, got {answered}"
    );

    // …and the listener the stack kept forwards to the group that now exists, so nothing it
    // reports names a resource the service no longer holds.
    let (status_code, answered) = elbv2(
        &router,
        "DescribeListeners",
        &[member("ListenerArns.member.1", &listener_arn)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DescribeListeners must answer 200, got {status_code} {answered}"
    );
    let forwarded: BTreeSet<String> = xml_texts(&answered, "TargetGroupArn").into_iter().collect();
    assert_eq!(
        forwarded,
        BTreeSet::from([group.clone()]),
        "case {case:?}: expected the listener's default action to forward to the new group \
         alone, got {answered}"
    );

    // The rules on that listener came through the replacement untouched.
    let rules = rules_of(&router, case, &listener_arn).await;
    assert!(
        rules.iter().any(|(arn, priority)| arn == &first_arn && priority == "10"),
        "case {case:?}: expected the listener's rules to survive the replacement, got {rules:?}"
    );
}

/// RULE: a delete the owning service REFUSES fails the stack's delete — the stack reports
/// `DELETE_FAILED` with that service's own refusal on the resource, and the resource is still
/// there; the delete retried once the refusal no longer holds completes, and the resource is
/// gone from that service.
#[tokio::test]
async fn a_delete_the_owning_service_refuses_fails_the_stack_and_the_retry_completes() {
    let case = "a load balancer the stack declared deletion protection for";
    let router = router();
    let (stack_id, described) = deploy(
        &router,
        case,
        "elbv2-flow-protected",
        &Declared {
            deletion_protection: "true",
            ..BASE
        },
    )
    .await;
    let load_balancer_arn = identifier(case, &described, "AlbArn");

    let (status_code, body) = call(&router, "DeleteStack", &[member("StackName", &stack_id)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: DeleteStack must answer 200, got {status_code} {body}"
    );
    let described = settle(&router, case, &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::DELETE_FAILED),
        "case {case:?}: the protected delete must end DELETE_FAILED, got {described}"
    );
    let reason = resource_event_reason(
        &router,
        &stack_id,
        "Alb",
        resource_status::DELETE_FAILED,
    )
    .await;
    assert!(
        reason.contains("deletion protection is enabled"),
        "case {case:?}: expected the stack to report the owning service's own refusal, got \
         {reason:?}"
    );

    // The load balancer is still there: a refused delete removed nothing.
    let (status_code, answered) = elbv2(
        &router,
        "DescribeLoadBalancers",
        &[member("LoadBalancerArns.member.1", &load_balancer_arn)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the refused delete must leave the load balancer in place, got \
         {status_code} {answered}"
    );

    // Clear the protection through the service that owns the attribute, and retry the delete.
    let (status_code, answered) = elbv2(
        &router,
        "ModifyLoadBalancerAttributes",
        &[
            member("LoadBalancerArn", &load_balancer_arn),
            member("Attributes.member.1.Key", "deletion_protection.enabled"),
            member("Attributes.member.1.Value", "false"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: ModifyLoadBalancerAttributes must answer 200, got {status_code} \
         {answered}"
    );

    let (status_code, body) = call(&router, "DeleteStack", &[member("StackName", &stack_id)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the retried DeleteStack must answer 200, got {status_code} {body}"
    );
    let described = settle(&router, case, &stack_id).await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::DELETE_COMPLETE),
        "case {case:?}: the retried delete must reach DELETE_COMPLETE, got {described}"
    );

    let (status_code, answered) = elbv2(
        &router,
        "DescribeLoadBalancers",
        &[member("LoadBalancerArns.member.1", &load_balancer_arn)],
    )
    .await;
    assert_eq!(
        status_code, 400,
        "case {case:?}: expected the load balancer to be gone with the stack, got \
         {status_code} {answered}"
    );
    assert_eq!(
        xml_text(&answered, "Code").as_deref(),
        Some("LoadBalancerNotFound"),
        "case {case:?}: expected LoadBalancerNotFound once the stack deleted it, got {answered}"
    );
}
