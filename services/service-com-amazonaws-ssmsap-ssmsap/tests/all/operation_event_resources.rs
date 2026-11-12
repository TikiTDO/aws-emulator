// SPDX-License-Identifier: CC0-1.0

//! The milestones an operation reports for the resources it acted on BESIDES the one the
//! call named.
//!
//! RULE under test: an operation's event list holds one milestone per resource the
//! operation acted on, and a milestone names that resource's OWN identity — its
//! `Resource$ResourceArn`, its `Resource$ResourceType` (`AWS::<Service>::<Resource>`, the
//! form `smithy_query(query="doc", shape="com.amazonaws.ssmsap#Resource",
//! member="ResourceType")` documents) and a `Description` saying what the operation did to
//! it — carrying the outcome that work actually had. A resource nothing was asked of has
//! no milestone.
//!
//! The one place this service reaches past the application the call named is the Amazon
//! EC2 instance shutdown `StopApplication$IncludeEc2InstanceShutdown` asks for, so the
//! rows drive that: the instance is EC2's resource, which is why this suite mounts EC2
//! beside this service on one engine (the harness
//! `crate::ec2_instance_shutdown::Emulator`, which the sibling suite about what the
//! shutdown DOES to an instance owns) and asserts everything about the milestones over
//! the wire.
//!
//! The rows span what the rule generalises over: no resource reached (the flag false and
//! the flag omitted), one reached, several reached, the same instance named twice by one
//! registration (one resource, so one milestone), and a resource whose owner REFUSED the
//! work beside one that accepted it — which is the row that pins a milestone to the
//! outcome the work had rather than to the fact it was asked for.

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::{
    OPERATION_EVENT_RESOURCE_TYPE_APPLICATION, OPERATION_EVENT_RESOURCE_TYPE_EC2_INSTANCE,
    OPERATION_EVENT_STATUS_COMPLETED, OPERATION_EVENT_STATUS_FAILED,
    OPERATION_EVENT_STATUS_IN_PROGRESS,
};

use crate::application_lifecycle::REGION;
use crate::ec2_instance_shutdown::{
    register_activated, settle_wait, stop, Emulator, UNHELD_INSTANCE,
};

/// The `OperationEvents` `ListOperationEvents` reports for `operation_id`, in the order
/// the listing reported them.
async fn events(emulator: &Emulator, operation_id: &str) -> Vec<Value> {
    let reply = emulator
        .ssm_sap(
            "/list-operation-events",
            json!({"OperationId": operation_id}),
        )
        .await;
    reply
        .get("OperationEvents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {operation_id}: expected OperationEvents, got {reply}"))
}

/// A string member of one event, treating JSON `null` as absent.
fn field(event: &Value, name: &str) -> String {
    event
        .get(name)
        .filter(|value| !value.is_null())
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A member of one event's `Resource`, treating JSON `null` as absent.
fn resource_field(event: &Value, name: &str) -> String {
    event
        .get("Resource")
        .and_then(|resource| resource.get(name))
        .filter(|value| !value.is_null())
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The events whose `Resource` is an Amazon EC2 instance, in listing order.
fn instance_events(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .filter(|event| {
            resource_field(event, "ResourceType") == OPERATION_EVENT_RESOURCE_TYPE_EC2_INSTANCE
        })
        .cloned()
        .collect()
}

/// The ARN an Amazon EC2 instance milestone must name: the form
/// `Resource$ResourceArn`'s own documentation gives for an instance
/// (`arn:aws:ec2:<region>:<account>:instance/<id>`), over the account every ARN of this
/// emulator carries.
fn expected_instance_arn(instance_id: &str) -> String {
    format!(
        "arn:aws:ec2:{REGION}:{}:instance/{instance_id}",
        ::engine::emulator_account_id()
    )
}

/// One row of the table: a registration, the stop it is asked to make, and the instances
/// the stop therefore reaches.
#[derive(Debug)]
struct Row {
    /// The application id the row registers.
    id: &'static str,
    /// How many components the registration declares, each on an instance of its own.
    components: &'static [&'static str],
    /// Whether a component is declared on the SAME instance the SAP system runs on, so
    /// one registration names that instance twice.
    repeats_instance: bool,
    /// Whether the SAP system itself is registered on an instance EC2 does not hold, so
    /// the owner of that resource refuses the work.
    system_instance_unheld: bool,
    /// The `IncludeEc2InstanceShutdown` the stop carries; `None` omits the member.
    flag: Option<bool>,
    /// How many instance milestones the operation must report.
    expected_milestones: usize,
}

/// Every row's registration names ONE SAP system instance plus one instance per declared
/// component, so this is how many instances the row launches.
fn launch_count(row: &Row) -> i32 {
    let components = i32::try_from(row.components.len()).expect("a small component count");
    if row.system_instance_unheld {
        // The SAP system's own instance is not launched: it is an id EC2 holds no record
        // of.
        components
    } else {
        1 + components
    }
}

/// An operation reports one milestone per resource it acted on, naming that resource
/// itself, and none for a resource nothing was asked of.
#[tokio::test]
async fn reports_one_milestone_per_resource_the_operation_acted_on() {
    let rows: &[Row] = &[
        Row {
            id: "one-instance",
            components: &[],
            repeats_instance: false,
            system_instance_unheld: false,
            flag: Some(true),
            expected_milestones: 1,
        },
        Row {
            id: "three-instances",
            components: &["WD", "ASCS"],
            repeats_instance: false,
            system_instance_unheld: false,
            flag: Some(true),
            expected_milestones: 3,
        },
        Row {
            id: "instance-named-twice",
            components: &[],
            repeats_instance: true,
            system_instance_unheld: false,
            flag: Some(true),
            expected_milestones: 1,
        },
        Row {
            id: "flag-false",
            components: &["ERS"],
            repeats_instance: false,
            system_instance_unheld: false,
            flag: Some(false),
            expected_milestones: 0,
        },
        Row {
            id: "flag-omitted",
            components: &[],
            repeats_instance: false,
            system_instance_unheld: false,
            flag: None,
            expected_milestones: 0,
        },
    ];

    for row in rows {
        let case = format!("{row:?}");
        let emulator = Emulator::new();
        let launched = emulator.launch_running(launch_count(row)).await;
        let system_instance = launched[0].clone();
        let mut components: Vec<(&str, &str)> = row
            .components
            .iter()
            .zip(launched.iter().skip(1))
            .map(|(component_type, instance)| (*component_type, instance.as_str()))
            .collect();
        if row.repeats_instance {
            components.push(("WD", system_instance.as_str()));
        }
        register_activated(&emulator, row.id, &system_instance, &components).await;

        let operation_id = stop(&emulator, row.id, row.flag).await;

        // While the stop is still running: the work asked of each instance is under way,
        // and the instances the stop reached are exactly the ones the registration
        // associated with the application — each named once.
        let running = events(&emulator, &operation_id).await;
        let instances = instance_events(&running);
        assert_eq!(
            instances.len(),
            row.expected_milestones,
            "case {case}: expected one milestone per instance reached, got {running:?}"
        );
        let named: Vec<String> = instances
            .iter()
            .map(|event| resource_field(event, "ResourceArn"))
            .collect();
        let expected_named: Vec<String> = if row.expected_milestones == 0 {
            Vec::new()
        } else {
            launched.iter().map(|id| expected_instance_arn(id)).collect()
        };
        assert_eq!(
            named, expected_named,
            "case {case}: a milestone must name the instance's own ARN, got {instances:?}"
        );
        for (event, instance_id) in instances.iter().zip(launched.iter()) {
            assert_eq!(
                field(event, "Description"),
                format!("Stop the EC2 instance {instance_id}"),
                "case {case}: the description must say what was done to the instance, got {event:?}"
            );
            assert_eq!(
                field(event, "Status"),
                OPERATION_EVENT_STATUS_IN_PROGRESS,
                "case {case}: work EC2 accepted and has not finished is IN_PROGRESS, got {event:?}"
            );
            assert!(
                event.get("StatusMessage").is_none_or(Value::is_null),
                "case {case}: work that has not failed has no status to explain, got {event:?}"
            );
            assert!(
                event.get("Timestamp").is_some(),
                "case {case}: a milestone is timestamped, got {event:?}"
            );
        }

        // The milestones of the resource the CALL named are still the first and the last
        // of the list, so the whole list reads oldest first.
        assert_eq!(
            resource_field(&running[0], "ResourceType"),
            OPERATION_EVENT_RESOURCE_TYPE_APPLICATION,
            "case {case}: the first milestone is the start of the operation's own work, got {running:?}"
        );
        assert_eq!(
            running.len(),
            1 + row.expected_milestones,
            "case {case}: a running operation has reached no terminal milestone yet, got {running:?}"
        );

        tokio::time::sleep(settle_wait()).await;

        // Once the clock has brought the stop to its terminal status, the work asked of
        // each instance is done — and the operation's own terminal milestone is last.
        let settled = events(&emulator, &operation_id).await;
        assert_eq!(
            settled.len(),
            2 + row.expected_milestones,
            "case {case}: a settled operation reports its terminal milestone too, got {settled:?}"
        );
        for event in instance_events(&settled) {
            assert_eq!(
                field(&event, "Status"),
                OPERATION_EVENT_STATUS_COMPLETED,
                "case {case}: work EC2 carried out is COMPLETED once the stop is, got {event:?}"
            );
        }
        let last = settled.last().expect("a settled operation reports events");
        assert_eq!(
            resource_field(last, "ResourceType"),
            OPERATION_EVENT_RESOURCE_TYPE_APPLICATION,
            "case {case}: the last milestone is the operation's own terminal one, got {settled:?}"
        );
        assert_eq!(
            field(last, "Status"),
            OPERATION_EVENT_STATUS_COMPLETED,
            "case {case}: the stop completed, got {settled:?}"
        );

        // The milestone is the work's: every instance it reports COMPLETED for really is
        // stopped where that instance's state lives.
        if row.expected_milestones > 0 {
            let states = emulator.instance_states(&launched).await;
            assert!(
                states.values().all(|state| state == "stopped"),
                "case {case}: every instance a milestone completed for must be stopped, got {states:?}"
            );
        }
    }
}

/// A milestone carries the outcome the work HAD: the instance whose owner refused the
/// stop is reported as the failure it was, and it does not change what the instance
/// beside it reports.
#[tokio::test]
async fn a_refused_resource_reports_a_failed_milestone() {
    let case = "an application whose component instance EC2 does not hold";
    let emulator = Emulator::new();
    let held = emulator.launch_running(1).await;
    let system_instance = held[0].clone();
    let id = "partly-held";
    // The SAP system runs on an instance EC2 holds, so the registration settles as
    // discovered; the declared component is the one on an instance EC2 refuses.
    register_activated(
        &emulator,
        id,
        &system_instance,
        &[("WD", UNHELD_INSTANCE)],
    )
    .await;

    let operation_id = stop(&emulator, id, Some(true)).await;
    tokio::time::sleep(settle_wait()).await;
    let settled = events(&emulator, &operation_id).await;
    let instances = instance_events(&settled);
    assert_eq!(
        instances.len(),
        2,
        "case {case}: both instances the registration named were asked for, got {settled:?}"
    );

    // The instances are asked for in the order the registration named them: the SAP
    // system's own first, then the declared component's.
    let carried_out = &instances[0];
    assert_eq!(
        resource_field(carried_out, "ResourceArn"),
        expected_instance_arn(&system_instance),
        "case {case}: the first milestone names the instance EC2 does hold, got {carried_out:?}"
    );
    assert_eq!(
        field(carried_out, "Status"),
        OPERATION_EVENT_STATUS_COMPLETED,
        "case {case}: work the owner carried out is COMPLETED, got {carried_out:?}"
    );

    let refused = &instances[1];
    assert_eq!(
        resource_field(refused, "ResourceArn"),
        expected_instance_arn(UNHELD_INSTANCE),
        "case {case}: the refused milestone names the instance that was asked for, got {refused:?}"
    );
    assert_eq!(
        field(refused, "Status"),
        OPERATION_EVENT_STATUS_FAILED,
        "case {case}: work the owner refused is FAILED, got {refused:?}"
    );
    assert!(
        !field(refused, "StatusMessage").is_empty(),
        "case {case}: a failed milestone explains its status, got {refused:?}"
    );

    let states = emulator.instance_states(&held).await;
    assert!(
        states.values().all(|state| state == "stopped"),
        "case {case}: the instance a milestone completed for must be stopped, got {states:?}"
    );
}

/// The filters this listing documents reach the milestones of the other resources too:
/// `resourceType` narrows to a kind of resource and `resourceID` to one resource, over
/// the very values each milestone reports.
#[tokio::test]
async fn filters_narrow_to_a_resource_kind_and_to_one_resource() {
    let emulator = Emulator::new();
    let launched = emulator.launch_running(2).await;
    let id = "filtered-milestones";
    register_activated(&emulator, id, &launched[0], &[("WD", launched[1].as_str())]).await;
    let operation_id = stop(&emulator, id, Some(true)).await;
    tokio::time::sleep(settle_wait()).await;

    // Rows: each resource type a milestone of this operation reports — the two instances
    // the stop reached, and the application's own start and completion — and one instance
    // id of the two, which selects that instance's milestone alone.
    let rows: &[(&str, &str, usize, Vec<String>)] = &[
        (
            "resourceType",
            OPERATION_EVENT_RESOURCE_TYPE_EC2_INSTANCE,
            2,
            launched.iter().map(|id| expected_instance_arn(id)).collect(),
        ),
        (
            "resourceType",
            OPERATION_EVENT_RESOURCE_TYPE_APPLICATION,
            2,
            Vec::new(),
        ),
        (
            "resourceID",
            launched[1].as_str(),
            1,
            vec![expected_instance_arn(&launched[1])],
        ),
    ];

    for (name, value, expected_count, expected_instance_arns) in rows {
        let case = format!("filter {name} = {value}");
        let reply = emulator
            .ssm_sap(
                "/list-operation-events",
                json!({
                    "OperationId": operation_id,
                    "Filters": [{"Name": name, "Value": value, "Operator": "Equals"}],
                }),
            )
            .await;
        let selected: Vec<Value> = reply
            .get("OperationEvents")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected OperationEvents, got {reply}"));
        assert_eq!(
            selected.len(),
            *expected_count,
            "case {case}: expected the milestones the filter selects, got {selected:?}"
        );
        let instance_arns: Vec<String> = instance_events(&selected)
            .iter()
            .map(|event| resource_field(event, "ResourceArn"))
            .collect();
        assert_eq!(
            &instance_arns, expected_instance_arns,
            "case {case}: expected these instance milestones, got {selected:?}"
        );
    }
}
