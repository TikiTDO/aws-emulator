// SPDX-License-Identifier: CC0-1.0

//! The resource-signal waits the signalled resource's OWN INSTANCES release, end to end over
//! the awsQuery wire.
//!
//! RULE: **a `CreationPolicy`'s `ResourceSignal` on a resource whose signals are sent by the
//! Amazon EC2 instances it consists of — an Auto Scaling group's launched instances, or an
//! `AWS::EC2::Instance` itself — receives one SUCCESS signal per such instance, under that
//! instance's own id as the signal's `UniqueId`, once the service that owns the resource
//! reports the instance serving and that instance's boot configuration asks it to signal its
//! stack with a success; so the create reaches a terminal status with no call from the client,
//! while a resource whose instances are not told to signal — or are told to report the failure
//! of their own bootstrap — receives nothing and is held to its `Timeout`.** CloudFormation "doesn't proceed with a stack creation or update until
//! resources receive the required number of signals or the timeout period is exceeded", those
//! signals come from the instances (`SignalResource` "is useful in cases where you want to
//! send signals from anywhere other than an Amazon EC2 instance", and `CreationPolicy`
//! documents the `cfn-signal` helper script as how an instance sends one), and "when you
//! signal Amazon EC2 instances or Auto Scaling groups, specify the instance ID that you are
//! signaling as the unique ID".
//!
//! Why a client cares: the standard signal-based deploy — a group with a `CreationPolicy`
//! whose instances signal from their user data — is what `aws cloudformation deploy`, `cdk
//! deploy` and every `wait stack-create-complete` gate sits on, and a stack that never leaves
//! `CREATE_IN_PROGRESS` makes all of them hang for ever.
//!
//! The rows span what the rule generalises over: which type the gate is declared on (a group
//! and an instance), how many instances send the signals (two, three and one), how the boot
//! configuration asks for them (the API's own command and the helper script), which resource
//! it names (this one, another one, and one the instance builds at boot), how many commands
//! the bootstrap runs before it (none, and the `cfn-init` AWS documents before it), which
//! status it asks for (a success, the failure of the bootstrap each way states it), whether it
//! asks at all, and whether the group's capacity is stated outright or left to its `MinSize`.
//! A further case has the group's instances come into service across SEVERAL passes, which is
//! what a gate reaching its `Count` over time depends on — and `resource_signal_waits.rs` is
//! the suite for the wait itself, which these rows do not re-pin.

use std::time::{Duration, Instant};

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    autoscaling, call, create_stack, describe_stack, ec2, member, router, xml_blocks, xml_text,
    xml_texts,
};

/// The AMI the rows' instances launch from, in Amazon EC2's own id form.
const IMAGE_ID: &str = "ami-00000000000000001";

/// A published instance type, which is what Amazon EC2 launches from.
const INSTANCE_TYPE: &str = "t3.micro";

/// The boot configuration of an instance that signals its stack through the API's own
/// command, as `SignalResource` documents it.
fn signals_through_the_api(logical_id: &str) -> String {
    format!(
        "#!/bin/bash\nINSTANCE_ID=$(curl -s \
         http://169.254.169.254/latest/meta-data/instance-id)\naws cloudformation \
         signal-resource --stack-name my-stack --logical-resource-id {logical_id} --unique-id \
         $INSTANCE_ID --status SUCCESS\n"
    )
}

/// The boot configuration of an instance that signals its stack through the helper script
/// `CreationPolicy` documents.
fn signals_through_the_helper(logical_id: &str) -> String {
    format!(
        "#!/bin/bash\n/opt/aws/bin/cfn-signal -e 0 --stack my-stack --resource {logical_id} \
         --region us-east-1\n"
    )
}

/// The boot configuration of an instance that runs the bootstrap AWS documents: `cfn-init`
/// for another resource's metadata first, then `cfn-signal` for this one. The two invocations
/// carry their own options, and only the second is a signal of this resource.
fn signals_after_initialising(logical_id: &str) -> String {
    format!(
        "#!/bin/bash\n/opt/aws/bin/cfn-init -v --stack my-stack --resource LaunchConfig --region \
         us-east-1\n/opt/aws/bin/cfn-signal -e 0 --stack my-stack --resource {logical_id} \
         --region us-east-1\n"
    )
}

/// A boot configuration that asks the instance for something OTHER than a signal, so nothing
/// signals the stack.
fn asks_for_no_signal() -> String {
    "#!/bin/bash\nyum install -y httpd\nsystemctl start httpd\n".to_string()
}

/// The boot configuration of an instance that signals the FAILURE of its own bootstrap, which
/// is the status the API's own command states: a success signal is not what such an instance
/// sends, so the gate is not released by it.
fn signals_failure_through_the_api(logical_id: &str) -> String {
    format!(
        "#!/bin/bash\naws cloudformation signal-resource --stack-name my-stack \
         --logical-resource-id {logical_id} --unique-id $INSTANCE_ID --status FAILURE\n"
    )
}

/// The same through the helper script, which states it as the exit code it reports: "a
/// non-zero exit code" is the failure of the bootstrap.
fn signals_failure_through_the_helper(logical_id: &str) -> String {
    format!(
        "#!/bin/bash\n/opt/aws/bin/cfn-signal -e 1 --stack my-stack --resource {logical_id} \
         --region us-east-1\n"
    )
}

/// A template whose Auto Scaling group carries the `CreationPolicy` its instances signal,
/// with the launch template they boot from and a topic that waits behind the gate.
fn group_template(
    capacity: Option<&str>,
    count: usize,
    user_data: Option<String>,
    behind: &str,
) -> String {
    let mut launch_data = json!({"ImageId": IMAGE_ID, "InstanceType": INSTANCE_TYPE});
    if let Some(user_data) = user_data {
        launch_data["UserData"] = json!({"Fn::Base64": user_data});
    }
    let mut properties = json!({
        "MinSize": "2",
        "MaxSize": "3",
        "AvailabilityZones": ["us-east-1a"],
        "LaunchTemplate": {
            "LaunchTemplateId": {"Ref": "Launch"},
            "Version": {"Fn::GetAtt": ["Launch", "LatestVersionNumber"]},
        },
    });
    if let Some(capacity) = capacity {
        properties["DesiredCapacity"] = json!(capacity);
    }
    json!({"Resources": {
        "Launch": {
            "Type": "AWS::EC2::LaunchTemplate",
            "Properties": {"LaunchTemplateData": launch_data},
        },
        "Group": {
            "Type": "AWS::AutoScaling::AutoScalingGroup",
            "Properties": properties,
            "CreationPolicy": {"ResourceSignal": {"Count": count, "Timeout": "PT15M"}},
            // The rolling update the same declaration carries in the deploys this rule is
            // about: it governs a later update, and says nothing about the create's gate.
            "UpdatePolicy": {"AutoScalingRollingUpdate": {
                "MaxBatchSize": 1,
                "MinInstancesInService": 1,
                "MinSuccessfulInstancesPercent": 100,
                "PauseTime": "PT15M",
                "WaitOnResourceSignals": true,
            }},
        },
        "Behind": {
            "Type": "AWS::SNS::Topic",
            "DependsOn": "Group",
            "Properties": {"TopicName": behind},
        },
    }})
    .to_string()
}

/// A template whose `AWS::EC2::Instance` carries the `CreationPolicy` it signals itself.
fn instance_template(user_data: Option<String>, behind: &str) -> String {
    let mut properties = json!({"ImageId": IMAGE_ID, "InstanceType": INSTANCE_TYPE});
    if let Some(user_data) = user_data {
        properties["UserData"] = json!({"Fn::Base64": user_data});
    }
    json!({"Resources": {
        "Node": {
            "Type": "AWS::EC2::Instance",
            "Properties": properties,
            "CreationPolicy": {"ResourceSignal": {"Count": 1, "Timeout": "PT15M"}},
        },
        "Behind": {
            "Type": "AWS::SNS::Topic",
            "DependsOn": "Node",
            "Properties": {"TopicName": behind},
        },
    }})
    .to_string()
}

/// How a row reads the instances the gate's resource consists of back from the service that
/// OWNS them, which is what the signals must be accounted against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// Amazon EC2 Auto Scaling, which reports the group's instances.
    AutoScalingGroup,
    /// The resource is the instance itself, so its physical id is the one instance.
    Instance,
}

/// One row: a template whose gate waits for the signals its own instances send, and how many
/// of them send one.
struct GateRow {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The logical id the gate is declared on, which the signals name.
    gate: &'static str,
    owner: Owner,
    /// How many instances the resource brings up, each of which signals once where its boot
    /// configuration asks it to.
    instances: usize,
    /// Whether that boot configuration asks for a signal at all.
    signalling: bool,
}

fn gate_rows() -> Vec<GateRow> {
    vec![
        GateRow {
            case: "a group of two whose instances signal through the API's own command",
            stack: "instance-signals-api",
            template: group_template(
                Some("2"),
                2,
                Some(signals_through_the_api("Group")),
                "instance-signals-api-behind",
            ),
            gate: "Group",
            owner: Owner::AutoScalingGroup,
            instances: 2,
            signalling: true,
        },
        GateRow {
            case: "a group of three whose instances signal through the helper script",
            stack: "instance-signals-helper",
            template: group_template(
                Some("3"),
                3,
                Some(signals_through_the_helper("Group")),
                "instance-signals-helper-behind",
            ),
            gate: "Group",
            owner: Owner::AutoScalingGroup,
            instances: 3,
            signalling: true,
        },
        GateRow {
            case: "a group whose capacity is left to its MinSize, signalling through the API",
            stack: "instance-signals-minsize",
            template: group_template(
                None,
                2,
                Some(signals_through_the_api("Group")),
                "instance-signals-minsize-behind",
            ),
            gate: "Group",
            owner: Owner::AutoScalingGroup,
            instances: 2,
            signalling: true,
        },
        GateRow {
            case: "an instance that signals for itself through the helper script",
            stack: "instance-signals-node",
            template: instance_template(
                Some(signals_through_the_helper("Node")),
                "instance-signals-node-behind",
            ),
            gate: "Node",
            owner: Owner::Instance,
            instances: 1,
            signalling: true,
        },
        GateRow {
            case: "a group whose instances are told to do something other than signal",
            stack: "instance-signals-silent",
            template: group_template(
                Some("2"),
                2,
                Some(asks_for_no_signal()),
                "instance-signals-silent-behind",
            ),
            gate: "Group",
            owner: Owner::AutoScalingGroup,
            instances: 2,
            signalling: false,
        },
        GateRow {
            case: "a group whose instances are given no boot configuration at all",
            stack: "instance-signals-bare",
            template: group_template(Some("2"), 2, None, "instance-signals-bare-behind"),
            gate: "Group",
            owner: Owner::AutoScalingGroup,
            instances: 2,
            signalling: false,
        },
        GateRow {
            case: "an instance given no boot configuration at all",
            stack: "instance-signals-bare-node",
            template: instance_template(None, "instance-signals-bare-node-behind"),
            gate: "Node",
            owner: Owner::Instance,
            instances: 1,
            signalling: false,
        },
        GateRow {
            case: "a group whose instances report the FAILURE of their own bootstrap through \
                   the API",
            stack: "instance-signals-failure",
            template: group_template(
                Some("2"),
                2,
                Some(signals_failure_through_the_api("Group")),
                "instance-signals-failure-behind",
            ),
            gate: "Group",
            owner: Owner::AutoScalingGroup,
            instances: 2,
            signalling: false,
        },
        GateRow {
            case: "an instance that reports the FAILURE of its own bootstrap through the \
                   helper script",
            stack: "instance-signals-failure-node",
            template: instance_template(
                Some(signals_failure_through_the_helper("Node")),
                "instance-signals-failure-node-behind",
            ),
            gate: "Node",
            owner: Owner::Instance,
            instances: 1,
            signalling: false,
        },
        GateRow {
            case: "a group whose instances signal ANOTHER resource of the same stack",
            stack: "instance-signals-elsewhere",
            template: group_template(
                Some("2"),
                2,
                Some(signals_through_the_api("SomeOtherResource")),
                "instance-signals-elsewhere-behind",
            ),
            gate: "Group",
            owner: Owner::AutoScalingGroup,
            instances: 2,
            signalling: false,
        },
        GateRow {
            case: "a group whose instances build the resource they signal at boot",
            stack: "instance-signals-substituted",
            template: group_template(
                Some("2"),
                2,
                Some(signals_through_the_api("$RESOURCE")),
                "instance-signals-substituted-behind",
            ),
            gate: "Group",
            owner: Owner::AutoScalingGroup,
            instances: 2,
            signalling: true,
        },
        GateRow {
            case: "a group whose instances initialise another resource before signalling this \
                   one",
            stack: "instance-signals-after-init",
            template: group_template(
                Some("2"),
                2,
                Some(signals_after_initialising("Group")),
                "instance-signals-after-init-behind",
            ),
            gate: "Group",
            owner: Owner::AutoScalingGroup,
            instances: 2,
            signalling: true,
        },
    ]
}

/// How long a row gives a create to reach a terminal status — far longer than the spans this
/// emulator's own clock owes, and far short of the `PT15M` every row's declaration asks the
/// wait to run for: what the rule is about is that the signals come without a client call, not
/// how fast.
const SETTLING_WINDOW: Duration = Duration::from_secs(30);

/// How long a row watches an UNSIGNALLED create for: past the spans the instances take to come
/// up and the stack's own transition, so a create still in flight is one the wait is holding.
const HOLDING_WINDOW: Duration = Duration::from_secs(6);

/// The `StackStatus` a stack reports right now.
async fn stack_status(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStacks({stack}) to answer, got {code} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The status the stack settles into, polled exactly as a client waiting on a deploy polls.
async fn settled_status(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let deadline = Instant::now() + SETTLING_WINDOW;
    loop {
        let status = stack_status(case, router, stack).await;
        if !status.ends_with("_IN_PROGRESS") {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the create to reach a terminal status within \
             {SETTLING_WINDOW:?}, got {status:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The physical id the stack reports for one of its resources, or the empty string where it
/// reports none.
async fn physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (_, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// The ids of the instances the SERVICE THAT OWNS the gate's resource reports for it, sorted:
/// the set the signals must be accounted against, read through that service's own wire.
async fn owned_instance_ids(
    case: &str,
    router: &MultiServiceRouter,
    row: &GateRow,
    physical_id: &str,
) -> Vec<String> {
    match row.owner {
        Owner::AutoScalingGroup => {
            let (code, body) = autoscaling(
                router,
                "DescribeAutoScalingGroups",
                &[("AutoScalingGroupNames.member.1", physical_id)],
            )
            .await;
            assert_eq!(
                code, 200,
                "case {case:?}: expected Amazon EC2 Auto Scaling to describe {physical_id}, got \
                 {code} {body}"
            );
            let in_service: Vec<String> = xml_texts(&body, "InstanceId")
                .into_iter()
                .zip(xml_texts(&body, "LifecycleState"))
                .filter(|(_, state)| state == "InService")
                .map(|(instance_id, _)| instance_id)
                .collect();
            let mut sorted = in_service;
            sorted.sort();
            sorted
        }
        Owner::Instance => vec![physical_id.to_string()],
    }
}

/// The `UniqueId`s of the signals the stack's event stream reports for one resource, sorted:
/// every event whose reason is a received signal, which is how this service reports one.
async fn signalled_unique_ids(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> Vec<String> {
    let (code, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStackEvents({stack}) to answer, got {code} {body}"
    );
    let mut unique_ids: Vec<String> = xml_blocks(&body, "member")
        .iter()
        .filter(|event| xml_text(event, "LogicalResourceId").as_deref() == Some(logical_id))
        .filter_map(|event| xml_text(event, "ResourceStatusReason"))
        .filter_map(|reason| {
            reason
                .strip_prefix("Received SUCCESS signal with UniqueId ")
                .map(str::to_string)
        })
        .collect();
    unique_ids.sort();
    unique_ids.dedup();
    unique_ids
}

#[tokio::test]
async fn a_creation_policy_is_signalled_by_the_instances_of_the_resource_it_gates() {
    for row in gate_rows() {
        let case = row.case;
        let stack = row.stack;
        let router = router();
        let (code, created) = create_stack(&router, stack, &row.template, &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected CreateStack to succeed, got {code} {created}"
        );

        if !row.signalling {
            // Nothing told these instances to signal, so the gate receives nothing and the
            // create is still in flight long after the instances are up.
            tokio::time::sleep(HOLDING_WINDOW).await;
            let status = stack_status(case, &router, stack).await;
            assert_eq!(
                status, "CREATE_IN_PROGRESS",
                "case {case:?}: expected the unsignalled create to still be in flight after \
                 {HOLDING_WINDOW:?}, got {status:?}"
            );
            // ...and the reason is the signals, not the instances: the owning service reports
            // every one of them serving already.
            let physical = physical_id(&router, stack, row.gate).await;
            let instances = owned_instance_ids(case, &router, &row, &physical).await;
            assert_eq!(
                instances.len(),
                row.instances,
                "case {case:?}: expected the owning service to report {} serving instance(s) \
                 for {physical:?}, got {instances:?}",
                row.instances
            );
            let signalled = signalled_unique_ids(case, &router, stack, row.gate).await;
            assert!(
                signalled.is_empty(),
                "case {case:?}: expected no signal to be reported for {}, got {signalled:?}",
                row.gate
            );
            continue;
        }

        // The signals the instances send are what completes the create: no client call is made
        // for this stack at all.
        let status = settled_status(case, &router, stack).await;
        assert_eq!(
            status, "CREATE_COMPLETE",
            "case {case:?}: expected the create to complete on the signals its own instances \
             sent, got {status:?}"
        );

        // One signal per instance, each under the id the SERVICE THAT OWNS the instances
        // assigned it: the signals are backed by instances that really exist there.
        let physical = physical_id(&router, stack, row.gate).await;
        let instances = owned_instance_ids(case, &router, &row, &physical).await;
        assert_eq!(
            instances.len(),
            row.instances,
            "case {case:?}: expected the owning service to report {} instance(s) for \
             {physical:?}, got {instances:?}",
            row.instances
        );
        let signalled = signalled_unique_ids(case, &router, stack, row.gate).await;
        assert_eq!(
            signalled, instances,
            "case {case:?}: expected one signal per instance the owning service reports, got \
             {signalled:?} against {instances:?}"
        );

        // ...and the resource behind the gate was created once it opened, which is what says
        // the walk went on rather than the status alone moving.
        let behind = physical_id(&router, stack, "Behind").await;
        assert!(
            !behind.is_empty(),
            "case {case:?}: expected the resource behind the gate to be created, got no \
             physical id"
        );
    }
}

/// Every `logical:status:reason` triple the stack's event stream carries, newest first: the
/// diagnosis a failing row reports, so a failure names which resource failed and why.
async fn event_trail(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    xml_blocks(&body, "member")
        .iter()
        .map(|event| {
            format!(
                "{}:{}:{}",
                xml_text(event, "LogicalResourceId").unwrap_or_default(),
                xml_text(event, "ResourceStatus").unwrap_or_default(),
                xml_text(event, "ResourceStatusReason").unwrap_or_default()
            )
        })
        .collect()
}

/// The VPC estate a signal-based Auto Scaling deploy carries: the network its instances are
/// placed in, the launch template they boot from, and the group whose `CreationPolicy` their
/// signals release.
fn estate_template(user_data: &str) -> String {
    json!({"Resources": {
        "Vpc": {"Type": "AWS::EC2::VPC", "Properties": {"CidrBlock": "10.0.0.0/16"}},
        "Igw": {"Type": "AWS::EC2::InternetGateway"},
        "AttachGateway": {
            "Type": "AWS::EC2::VPCGatewayAttachment",
            "Properties": {"VpcId": {"Ref": "Vpc"}, "InternetGatewayId": {"Ref": "Igw"}},
        },
        "RouteTable": {
            "Type": "AWS::EC2::RouteTable",
            "Properties": {"VpcId": {"Ref": "Vpc"}},
        },
        "SubnetA": {
            "Type": "AWS::EC2::Subnet",
            "Properties": {
                "VpcId": {"Ref": "Vpc"},
                "CidrBlock": "10.0.1.0/24",
                "AvailabilityZone": "us-east-1a",
            },
        },
        "SubnetARouteAssoc": {
            "Type": "AWS::EC2::SubnetRouteTableAssociation",
            "Properties": {
                "SubnetId": {"Ref": "SubnetA"},
                "RouteTableId": {"Ref": "RouteTable"},
            },
        },
        // The rest of the network whose identifiers Amazon EC2 assigns, so the carry-forward
        // the update is read for spans every one of them rather than the route table alone.
        "Address": {"Type": "AWS::EC2::EIP", "Properties": {"Domain": "vpc"}},
        "Nat": {
            "Type": "AWS::EC2::NatGateway",
            "Properties": {
                "AllocationId": {"Fn::GetAtt": ["Address", "AllocationId"]},
                "SubnetId": {"Ref": "SubnetA"},
            },
        },
        "Vgw": {"Type": "AWS::EC2::VPNGateway", "Properties": {"Type": "ipsec.1"}},
        "Endpoint": {
            "Type": "AWS::EC2::VPCEndpoint",
            "Properties": {
                "VpcId": {"Ref": "Vpc"},
                "ServiceName": "com.amazonaws.us-east-1.s3",
                "RouteTableIds": [{"Ref": "RouteTable"}],
            },
        },
        "Launch": {
            "Type": "AWS::EC2::LaunchTemplate",
            "Properties": {"LaunchTemplateData": {
                "ImageId": IMAGE_ID,
                "InstanceType": INSTANCE_TYPE,
                "UserData": {"Fn::Base64": user_data},
            }},
        },
        "Group": {
            "Type": "AWS::AutoScaling::AutoScalingGroup",
            "Properties": {
                "MinSize": "2",
                "MaxSize": "4",
                "DesiredCapacity": "2",
                "VPCZoneIdentifier": [{"Ref": "SubnetA"}],
                "LaunchTemplate": {
                    "LaunchTemplateId": {"Ref": "Launch"},
                    "Version": {"Fn::GetAtt": ["Launch", "LatestVersionNumber"]},
                },
            },
            "CreationPolicy": {"ResourceSignal": {"Count": 2, "Timeout": "PT15M"}},
            "UpdatePolicy": {"AutoScalingRollingUpdate": {
                "MaxBatchSize": 1,
                "MinInstancesInService": 1,
                "MinSuccessfulInstancesPercent": 100,
                "PauseTime": "PT15M",
                "WaitOnResourceSignals": true,
            }},
        },
    },
    "Outputs": {
        // The launch template's own version numbers, which are Amazon EC2's: what a group
        // reading `LatestVersionNumber` launches from.
        "LatestVersion": {"Value": {"Fn::GetAtt": ["Launch", "LatestVersionNumber"]}},
        "TemplateId": {"Value": {"Ref": "Launch"}},
    }})
    .to_string()
}

#[tokio::test]
async fn a_signalled_estate_deploys_and_takes_a_new_launch_template_version() {
    const CASE: &str = "a signalled Auto Scaling estate whose launch template changes";
    let stack = "instance-signals-estate";
    let router = router();
    let created = estate_template(&signals_through_the_api("Group"));
    let (code, body) = create_stack(&router, stack, &created, &[]).await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected CreateStack to succeed, got {code} {body}"
    );
    let status = settled_status(CASE, &router, stack).await;
    assert_eq!(
        status, "CREATE_COMPLETE",
        "case {CASE:?}: expected the signalled create to complete, got {status:?}; events \
         {:?}",
        event_trail(&router, stack).await
    );

    // The same estate with a changed boot configuration: the launch template takes a new
    // version, and every other resource of the estate is carried forward untouched.
    //
    // Every logical id below is a resource whose identifier its OWNING SERVICE assigned, which
    // no deployment can recompute: what the update must do with each is keep it.
    const CARRIED_FORWARD: &[&str] = &[
        "Vpc",
        "SubnetA",
        "Igw",
        "RouteTable",
        "SubnetARouteAssoc",
        "Address",
        "Nat",
        "Vgw",
        "Endpoint",
        "Launch",
        "Group",
    ];
    let mut before: Vec<(&str, String)> = Vec::new();
    for logical_id in CARRIED_FORWARD {
        let identifier = physical_id(&router, stack, logical_id).await;
        assert!(
            !identifier.is_empty(),
            "case {CASE:?}: expected {logical_id} to report a physical id after the create"
        );
        before.push((logical_id, identifier));
    }
    let created_version = output_value(CASE, &router, stack, "LatestVersion").await;

    let changed = estate_template(&format!(
        "{}echo redeployed\n",
        signals_through_the_api("Group")
    ));
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[member("StackName", stack), member("TemplateBody", &changed)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected UpdateStack to be accepted, got {code} {body}"
    );
    let status = settled_status(CASE, &router, stack).await;
    assert_eq!(
        status, "UPDATE_COMPLETE",
        "case {CASE:?}: expected the update to complete, got {status:?}; events {:?}",
        event_trail(&router, stack).await
    );

    // Nothing was created a second time: each resource the update did not change keeps the
    // identifier its owning service assigned, and the stack reports ONE creation of it.
    let created_events = creations_per_resource(&router, stack).await;
    for (logical_id, identifier) in &before {
        let now = physical_id(&router, stack, logical_id).await;
        assert_eq!(
            &now, identifier,

            "case {CASE:?}: expected {logical_id} to keep the identifier its owning service \
             assigned, got {now:?} against {identifier:?}"
        );
        let creations = created_events
            .iter()
            .filter(|created| created == logical_id)
            .count();
        assert_eq!(
            creations, 1,
            "case {CASE:?}: expected {logical_id} to be created exactly once, got {creations} \
             creations; events {:?}",
            event_trail(&router, stack).await
        );
    }

    // ...and the one resource the update DID change took a new VERSION rather than a new
    // identity: a launch template is a versioned document, and the group launches from the
    // version the stack now reports.
    let updated_version = output_value(CASE, &router, stack, "LatestVersion").await;
    assert_ne!(
        updated_version, created_version,
        "case {CASE:?}: expected the changed launch template to report a new \
         LatestVersionNumber, got {updated_version:?} against {created_version:?}"
    );
    let template_id = output_value(CASE, &router, stack, "TemplateId").await;
    let (code, described) = ec2(
        &router,
        "DescribeLaunchTemplates",
        &[("LaunchTemplateId.1", &template_id)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected Amazon EC2 to describe the launch template, got {code} \
         {described}"
    );
    assert_eq!(
        xml_text(&described, "latestVersionNumber").as_deref(),
        Some(updated_version.as_str()),
        "case {CASE:?}: expected Amazon EC2 to hold the version the stack reports, got \
         {described}"
    );
    assert_eq!(
        xml_text(&described, "defaultVersionNumber").as_deref(),
        Some(updated_version.as_str()),
        "case {CASE:?}: expected the new version to be the template's default, got {described}"
    );
}

/// One `OutputValue` the stack reports, which is how a resource's `Fn::GetAtt` attribute is
/// read back through the wire: the stack's outputs are evaluated against the resources the
/// deployment that finished left.
async fn output_value(case: &str, router: &MultiServiceRouter, stack: &str, key: &str) -> String {
    let (code, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected DescribeStacks({stack}) to answer, got {code} {body}"
    );
    let value = xml_blocks(&body, "member")
        .iter()
        .find(|output| xml_text(output, "OutputKey").as_deref() == Some(key))
        .and_then(|output| xml_text(output, "OutputValue"));
    value.unwrap_or_else(|| panic!("case {case:?}: the stack reports no output {key:?}: {body}"))
}

/// Every logical id the stack's event stream reports a CREATE_COMPLETE for, one entry per
/// such event: a resource created twice appears twice.
async fn creations_per_resource(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    xml_blocks(&body, "member")
        .iter()
        .filter(|event| xml_text(event, "ResourceStatus").as_deref() == Some("CREATE_COMPLETE"))
        .filter_map(|event| xml_text(event, "LogicalResourceId"))
        .collect()
}

#[tokio::test]
async fn a_gate_reaches_its_count_from_instances_that_come_into_service_over_time() {
    const CASE: &str = "a group whose second instance only comes into service later";
    let stack = "instance-signals-staggered";
    let router = router();
    // The gate waits for THREE signals while the group starts with two instances: its `Count`
    // is not met by the instances that are already serving, so the gate holds until the
    // capacity the group is given later brings up the third.
    let template = group_template(
        Some("2"),
        3,
        Some(signals_through_the_api("Group")),
        "instance-signals-staggered-behind",
    );
    let (code, created) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected CreateStack to succeed, got {code} {created}"
    );

    // The signals of the instances already serving are recorded and counted, and two signals
    // are short of the gate's `Count`: the create is still in flight.
    tokio::time::sleep(HOLDING_WINDOW).await;
    let status = stack_status(CASE, &router, stack).await;
    assert_eq!(
        status, "CREATE_IN_PROGRESS",
        "case {CASE:?}: expected the create to still be in flight on two signals of three, got \
         {status:?}"
    );
    let group = physical_id(&router, stack, "Group").await;
    let signalled = signalled_unique_ids(CASE, &router, stack, "Group").await;
    assert_eq!(
        signalled.len(),
        2,
        "case {CASE:?}: expected the serving instances to have signalled, got {signalled:?}"
    );

    // A third instance, brought up through the OWNING service's own call: its signal is the
    // one that meets the `Count`, which the pass must read even though signals of this gate's
    // are already recorded.
    let (code, scaled) = autoscaling(
        &router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", group.as_str()),
            ("DesiredCapacity", "3"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected the capacity change to be accepted, got {code} {scaled}"
    );
    let status = settled_status(CASE, &router, stack).await;
    assert_eq!(
        status, "CREATE_COMPLETE",
        "case {CASE:?}: expected the later instance's signal to complete the create, got \
         {status:?}; events {:?}",
        event_trail(&router, stack).await
    );
    let signalled = signalled_unique_ids(CASE, &router, stack, "Group").await;
    assert_eq!(
        signalled.len(),
        3,
        "case {CASE:?}: expected one signal per instance that came into service, got \
         {signalled:?}"
    );
}

#[tokio::test]
async fn a_gate_is_released_by_whichever_of_a_group_s_instances_is_told_to_signal() {
    const CASE: &str = "a group whose instances are told different things";
    let stack = "instance-signals-mixed";
    let router = router();
    // The group launches from whatever version its launch template has at the time
    // (`$Latest`), and the version it starts on tells its instances NOT to signal: the gate
    // waits for one signal that none of the instances now serving will send.
    let template = json!({"Resources": {
        "Launch": {
            "Type": "AWS::EC2::LaunchTemplate",
            "Properties": {"LaunchTemplateData": {
                "ImageId": IMAGE_ID,
                "InstanceType": INSTANCE_TYPE,
                "UserData": {"Fn::Base64": asks_for_no_signal()},
            }},
        },
        "Group": {
            "Type": "AWS::AutoScaling::AutoScalingGroup",
            "Properties": {
                "MinSize": "2",
                "MaxSize": "4",
                "DesiredCapacity": "2",
                "AvailabilityZones": ["us-east-1a"],
                "LaunchTemplate": {
                    "LaunchTemplateId": {"Ref": "Launch"},
                    "Version": "$Latest",
                },
            },
            "CreationPolicy": {"ResourceSignal": {"Count": 1, "Timeout": "PT15M"}},
        },
    }})
    .to_string();
    let (code, created) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected CreateStack to succeed, got {code} {created}"
    );
    tokio::time::sleep(HOLDING_WINDOW).await;
    let status = stack_status(CASE, &router, stack).await;
    assert_eq!(
        status, "CREATE_IN_PROGRESS",
        "case {CASE:?}: expected the create to still be in flight while no instance is told \
         to signal, got {status:?}"
    );

    // A version of the launch template that DOES tell an instance to signal, and one more
    // unit of capacity to launch from it: that instance is not the first the owning service
    // reports for the group, and its signal is the one the gate waits for.
    let launch = physical_id(&router, stack, "Launch").await;
    let group = physical_id(&router, stack, "Group").await;
    let signalling = ::aws_smithy_types::base64::encode(signals_through_the_api("Group"));
    let (code, versioned) = ec2(
        &router,
        "CreateLaunchTemplateVersion",
        &[
            ("LaunchTemplateId", launch.as_str()),
            ("SourceVersion", "1"),
            ("LaunchTemplateData.UserData", signalling.as_str()),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected Amazon EC2 to take the new version, got {code} {versioned}"
    );
    let (code, scaled) = autoscaling(
        &router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", group.as_str()),
            ("DesiredCapacity", "3"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected the capacity change to be accepted, got {code} {scaled}"
    );

    let status = settled_status(CASE, &router, stack).await;
    assert_eq!(
        status, "CREATE_COMPLETE",
        "case {CASE:?}: expected the one instance told to signal to complete the create, got \
         {status:?}; events {:?}",
        event_trail(&router, stack).await
    );
    let signalled = signalled_unique_ids(CASE, &router, stack, "Group").await;
    assert_eq!(
        signalled.len(),
        1,
        "case {CASE:?}: expected exactly the signal of the instance that was told to send \
         one, got {signalled:?}"
    );
}

#[tokio::test]
async fn a_client_signal_still_releases_a_gate_the_instances_do_not_signal() {
    const CASE: &str = "a group whose instances are told to do something other than signal";
    let stack = "instance-signals-client";
    let router = router();
    let template = group_template(
        Some("2"),
        2,
        Some(asks_for_no_signal()),
        "instance-signals-client-behind",
    );
    let (code, created) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(
        code, 200,
        "case {CASE:?}: expected CreateStack to succeed, got {code} {created}"
    );

    // The instances send nothing, so the client's own `SignalResource` calls are what the
    // gate's `Count` is met by — one per host, exactly as a client outside Amazon EC2 signals.
    for unique_id in ["host-one", "host-two"] {
        let (code, signalled) = call(
            &router,
            "SignalResource",
            &[
                member("StackName", stack),
                member("LogicalResourceId", "Group"),
                member("UniqueId", unique_id),
                member("Status", "SUCCESS"),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {CASE:?}: expected the signal {unique_id} to be accepted, got {code} \
             {signalled}"
        );
    }
    let status = settled_status(CASE, &router, stack).await;
    assert_eq!(
        status, "CREATE_COMPLETE",
        "case {CASE:?}: expected the client-signalled create to complete, got {status:?}"
    );
}
