// SPDX-License-Identifier: CC0-1.0

//! `TerminateInstanceInAutoScalingGroup` over the awsQuery wire.
//!
//! The rule under test: a termination request names the instances to take out of service
//! — one through `InstanceId`, or up to the modelled hundred through `InstanceIds`
//! together with the `AutoScalingGroupName` that holds them — and every id it names must
//! be an instance of that Auto Scaling group, so a call naming an instance of another
//! group, or of no group at all, terminates NONE of them and is refused `ValidationError`
//! naming the instance and the group it is not part of. A call whose every id belongs to
//! the group reports each named instance `Terminating`, answers with the scaling activity
//! each termination recorded (`Activity` for the `InstanceId` form, `Activities` for the
//! `InstanceIds` form, as the model documents), and treats
//! `ShouldDecrementDesiredCapacity` as what the request says about the group's size: with
//! it the desired capacity goes down by the number of instances terminated — and a
//! request that would take it below the group's own `MinSize`, which no group's desired
//! capacity goes under, is refused instead — while without it the capacity stands, so the
//! group launches a replacement for each instance it lost.
//!
//! The tables below span what that rule generalises over — how many instances the group
//! holds, which of them the request names and where in the request it names them, whether
//! the request names one instance or a list (and a list of one), whether an id repeats,
//! whether the instance is protected from scale-in, whether the request asks for the
//! decrement, where the group's bounds sit relative to its capacity, and whether a
//! missing id belongs to another group or to no group — and every row reads the group back
//! through `DescribeAutoScalingGroups` (the implemented reader of a group's instances and
//! of its desired capacity), so no row can pass on an accepted-but-unstored change.
//!
//! The activity each call answers with is also read BACK through
//! `DescribeScalingActivities`, the reader of the recorded activity log, so the record the
//! response carries is the record the service stored and not a second copy of it.
//!
//! A termination already under way owes nothing further: naming the same instance again,
//! in either form, is served and records no second activity, which is its own row below.

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{borrowed, new_engine, p, query, router_with, tag_value, xml_unescape};

/// The launch source every group here is created from — a launch TEMPLATE, because no
/// account can create a launch configuration (see
/// `launch_configurations_are_unavailable.rs`).
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// The zone every group here places into, so each group has somewhere to launch the
/// instances this suite terminates and replaces.
const ZONE: (&str, &str) = ("AvailabilityZones.member.1", "us-west-2a");

/// The `LifecycleState` of an instance on its way out of a group, spelled as the model's
/// `com.amazonaws.autoscaling#LifecycleState` enum spells it.
const TERMINATING: &str = "Terminating";

/// The most instance ids ONE call may name, from the model's own `TerminationInstanceIds`
/// (`@length(min: 1, max: 100)`) and the documentation of
/// `TerminateInstanceInAutoScalingGroupType$InstanceIds`: "You can specify up to 100
/// instances."
const MODELLED_MAX_INSTANCE_IDS: i32 = 100;

/// What a group reports for one of its instances.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedInstance {
    instance_id: String,
    lifecycle_state: String,
}

/// What the group reports about itself after a call: its desired capacity and the
/// instances it holds, in the order it holds them.
#[derive(Debug, Clone)]
struct ReportedGroup {
    desired_capacity: i32,
    instances: Vec<ReportedInstance>,
}

impl ReportedGroup {
    /// The instances that count towards the group's capacity: everything not on its way
    /// out.
    fn active(&self) -> Vec<&ReportedInstance> {
        self.instances
            .iter()
            .filter(|instance| instance.lifecycle_state != TERMINATING)
            .collect()
    }

    /// Whether the group reports `instance_id` as `Terminating`.
    fn is_terminating(&self, instance_id: &str) -> bool {
        self.instances.iter().any(|instance| {
            instance.instance_id == instance_id && instance.lifecycle_state == TERMINATING
        })
    }
}

/// Create a group with the bounds and capacity given and answer the instance ids it
/// reports, in the order the group holds them (oldest first).
async fn create_group(
    router: &MultiServiceRouter,
    name: &str,
    min_size: i32,
    max_size: i32,
    desired_capacity: i32,
) -> Vec<String> {
    let params = vec![
        p("AutoScalingGroupName", name),
        p(LAUNCH_SOURCE.0, LAUNCH_SOURCE.1),
        p("MinSize", min_size.to_string()),
        p("MaxSize", max_size.to_string()),
        p("DesiredCapacity", desired_capacity.to_string()),
        p(ZONE.0, ZONE.1),
    ];
    let (status, body) = query(router, "CreateAutoScalingGroup", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "creating {name:?} with capacity {desired_capacity} must succeed, got {status} with \
         {body:?}"
    );
    let reported = reported_group(router, name).await;
    assert_eq!(
        reported.instances.len(),
        usize::try_from(desired_capacity).expect("a capacity is a count"),
        "the group {name:?} must hold the capacity it was created with, got {reported:?}"
    );
    reported
        .instances
        .into_iter()
        .map(|instance| instance.instance_id)
        .collect()
}

/// The inner text of the first `<tag …>…</tag>` element, whose open tag may carry
/// attributes — the envelope stamps an `xmlns` on the elements a modelled structure
/// renders, so a reader that looks only for `<tag>` would miss them.
fn element_body<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut rest = xml;
    loop {
        let start = rest.find(&open)?;
        let after = &rest[start + open.len()..];
        // `<Activities>` also starts with `<Activity`: the character after the name is
        // what tells the element apart from one whose name merely begins with it.
        let boundary = after.chars().next()?;
        if boundary == '>' || boundary.is_whitespace() {
            let inner_start = after.find('>')? + 1;
            let end = after.find(&close)?;
            return Some(&after[inner_start..end]);
        }
        rest = after;
    }
}

/// The inner text of every `<tag …>…</tag>` element, in document order. The elements
/// this suite reads never nest, so each one ends at the next closing tag.
fn element_bodies<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(inner) = element_body(rest, tag) {
        out.push(inner);
        let consumed = rest
            .find(close.as_str())
            .map_or(rest.len(), |at| at + close.len());
        rest = &rest[consumed..];
    }
    out
}

/// The group as its implemented reader reports it.
async fn reported_group(router: &MultiServiceRouter, name: &str) -> ReportedGroup {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the read-back of {name:?} must succeed, got {status} with {body:?}"
    );
    let desired_capacity = tag_value(&body, "DesiredCapacity")
        .parse::<i32>()
        .unwrap_or_else(|_| panic!("the group {name:?} must report a desired capacity: {body:?}"));
    // The instance members live inside the group's own `Instances` element; the response
    // carries other `<member>` lists (the zones, the termination policies), so the slice
    // is narrowed to that element first.
    let instances = element_body(&body, "Instances")
        .map(|inner| {
            element_bodies(inner, "member")
                .iter()
                .map(|member| ReportedInstance {
                    instance_id: tag_value(member, "InstanceId"),
                    lifecycle_state: tag_value(member, "LifecycleState"),
                })
                .collect()
        })
        .unwrap_or_default();
    ReportedGroup {
        desired_capacity,
        instances,
    }
}

/// One answered termination: its status, the wire error code and message of a refusal,
/// and the body for the activities a success carries.
struct Answer {
    status: u16,
    code: String,
    message: String,
    body: String,
}

/// Post one `TerminateInstanceInAutoScalingGroup`, naming the instances in the form the
/// caller chose: `InstanceId` for a single id, `InstanceIds.member.N` for a list.
async fn terminate(
    router: &MultiServiceRouter,
    group_name: Option<&str>,
    single: Option<&str>,
    list: &[String],
    should_decrement: bool,
) -> Answer {
    let mut params = vec![p(
        "ShouldDecrementDesiredCapacity",
        if should_decrement { "true" } else { "false" },
    )];
    if let Some(group_name) = group_name {
        params.push(p("AutoScalingGroupName", group_name));
    }
    if let Some(single) = single {
        params.push(p("InstanceId", single));
    }
    for (index, instance_id) in list.iter().enumerate() {
        params.push(p(
            format!("InstanceIds.member.{}", index + 1).as_str(),
            instance_id.clone(),
        ));
    }
    let (status, body) = query(
        router,
        "TerminateInstanceInAutoScalingGroup",
        &borrowed(&params),
    )
    .await;
    Answer {
        status,
        code: tag_value(&body, "Code"),
        message: xml_unescape(&tag_value(&body, "Message")),
        body,
    }
}

/// The members of the one `Activity` a single-instance request answers with.
fn single_activity(body: &str) -> Vec<(String, String)> {
    element_body(body, "Activity")
        .map(|inner| {
            vec![(
                tag_value(inner, "ActivityId"),
                tag_value(inner, "Description"),
            )]
        })
        .unwrap_or_default()
}

/// The `<member>` elements of the `Activities` a list request answers with.
fn listed_activities(body: &str) -> Vec<(String, String)> {
    element_body(body, "Activities")
        .map(|inner| {
            element_bodies(inner, "member")
                .iter()
                .map(|member| {
                    (
                        tag_value(member, "ActivityId"),
                        tag_value(member, "Description"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The message a termination naming an instance that is not part of the group is refused
/// with: fixed text, with the instance id and the group name from the request, and the
/// unfilled token where the request named no group.
fn not_part_of_group_message(instance_id: &str, group_name: Option<&str>) -> String {
    let group_name = group_name.unwrap_or("null");
    format!("Instance {instance_id} is not part of Auto Scaling group {group_name}.")
}

/// One accepted single-instance row: the case, the group's bounds and capacity, the
/// 1-based position of the instance the request names, whether that instance is protected
/// from scale-in first, whether the request asks for the decrement, and the desired
/// capacity the group must report afterwards.
struct SingleRow {
    case: &'static str,
    min_size: i32,
    max_size: i32,
    held: i32,
    named: usize,
    protect_first: bool,
    should_decrement: bool,
    names_the_group: bool,
    expected_capacity: i32,
}

/// Every row: the named instance is the one taken out of service, the group's desired
/// capacity is what the request asked for, and the group holds exactly that capacity in
/// instances that are not leaving — so a non-decrementing call is seen to replace what it
/// terminated.
#[tokio::test]
async fn a_termination_takes_the_named_instance_out_of_service() {
    let rows = [
        SingleRow {
            case: "the only instance of the group, decrementing",
            min_size: 0,
            max_size: 3,
            held: 1,
            named: 1,
            protect_first: false,
            should_decrement: true,
            names_the_group: false,
            expected_capacity: 0,
        },
        SingleRow {
            case: "the oldest of three, decrementing — not the one a scale-in would pick",
            min_size: 0,
            max_size: 3,
            held: 3,
            named: 1,
            protect_first: false,
            should_decrement: true,
            names_the_group: false,
            expected_capacity: 2,
        },
        SingleRow {
            case: "the newest of three, decrementing",
            min_size: 0,
            max_size: 3,
            held: 3,
            named: 3,
            protect_first: false,
            should_decrement: true,
            names_the_group: false,
            expected_capacity: 2,
        },
        SingleRow {
            case: "the middle of three, NOT decrementing, so the group replaces it",
            min_size: 0,
            max_size: 3,
            held: 3,
            named: 2,
            protect_first: false,
            should_decrement: false,
            names_the_group: false,
            expected_capacity: 3,
        },
        SingleRow {
            case: "the only instance, NOT decrementing, so the group replaces it",
            min_size: 0,
            max_size: 2,
            held: 1,
            named: 1,
            protect_first: false,
            should_decrement: false,
            names_the_group: false,
            expected_capacity: 1,
        },
        SingleRow {
            case: "an instance protected from scale-in, which the request names anyway",
            min_size: 0,
            max_size: 2,
            held: 2,
            named: 2,
            protect_first: true,
            should_decrement: true,
            names_the_group: false,
            expected_capacity: 1,
        },
        SingleRow {
            case: "the group named beside the instance, decrementing",
            min_size: 0,
            max_size: 2,
            held: 2,
            named: 1,
            protect_first: false,
            should_decrement: true,
            names_the_group: true,
            expected_capacity: 1,
        },
        SingleRow {
            case: "a decrement down TO the group's MinSize, which is not under it",
            min_size: 1,
            max_size: 3,
            held: 2,
            named: 1,
            protect_first: false,
            should_decrement: true,
            names_the_group: false,
            expected_capacity: 1,
        },
        SingleRow {
            case: "a group at its MinSize, NOT decrementing, which replaces what it lost",
            min_size: 2,
            max_size: 3,
            held: 2,
            named: 2,
            protect_first: false,
            should_decrement: false,
            names_the_group: false,
            expected_capacity: 2,
        },
    ];

    for (row_index, row) in rows.iter().enumerate() {
        // One emulator per row: the instance ids of this row alone, so a single-instance
        // request that names no group resolves the group from the instance unambiguously.
        let router = router_with(new_engine());
        let name = format!("asg-single-{row_index}");
        let held = create_group(&router, &name, row.min_size, row.max_size, row.held).await;
        let named = held[row.named - 1].clone();

        if row.protect_first {
            let params = vec![
                p("AutoScalingGroupName", name.as_str()),
                p("InstanceIds.member.1", named.clone()),
                p("ProtectedFromScaleIn", "true"),
            ];
            let (status, body) = query(&router, "SetInstanceProtection", &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {:?}: protecting {named} first must succeed, got {status} with {body:?}",
                row.case
            );
        }

        let group_name = row.names_the_group.then_some(name.as_str());
        let answer = terminate(
            &router,
            group_name,
            Some(&named),
            &[],
            row.should_decrement,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200, got {} with {:?} {:?}",
            row.case, answer.status, answer.code, answer.message
        );

        // The `InstanceId` form answers with `Activity`, never with `Activities`.
        let activity = single_activity(&answer.body);
        assert_eq!(
            activity.len(),
            1,
            "case {:?}: expected one Activity, got {:?}",
            row.case,
            answer.body
        );
        assert!(
            listed_activities(&answer.body).is_empty(),
            "case {:?}: the InstanceId form carries no Activities member, got {:?}",
            row.case,
            answer.body
        );
        let (activity_id, description) = activity[0].clone();
        assert!(
            !activity_id.is_empty(),
            "case {:?}: the activity must carry an id, got {:?}",
            row.case,
            answer.body
        );
        assert!(
            description.contains(&named),
            "case {:?}: expected the description to name {named}, got {description:?}",
            row.case
        );

        let reported = reported_group(&router, &name).await;
        assert!(
            reported.is_terminating(&named),
            "case {:?}: expected {named} reported Terminating, got {reported:?}",
            row.case
        );
        assert_eq!(
            reported.desired_capacity, row.expected_capacity,
            "case {:?}: expected the desired capacity {}, got {reported:?}",
            row.case, row.expected_capacity
        );
        assert_eq!(
            i32::try_from(reported.active().len()).expect("a held instance count fits in an i32"),
            row.expected_capacity,
            "case {:?}: the group must hold its desired capacity in instances that are not \
             leaving, got {reported:?}",
            row.case
        );
        // Nothing but the named instance left: every other instance the group held is
        // still there and is not leaving.
        for other in held.iter().filter(|instance_id| **instance_id != named) {
            let state = reported
                .instances
                .iter()
                .find(|instance| instance.instance_id == *other)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the group must still report {other}, got {reported:?}",
                        row.case
                    )
                });
            assert_ne!(
                state.lifecycle_state, TERMINATING,
                "case {:?}: only the named instance is taken out of service, got {state:?}",
                row.case
            );
        }
    }
}

/// One accepted list row: the case, how many instances the group holds, the 1-based
/// positions the request names (in request order, repeats included), whether the request
/// asks for the decrement, how many distinct instances it should take out of service, and
/// the desired capacity the group must report afterwards.
struct ListRow {
    case: &'static str,
    held: i32,
    named: &'static [usize],
    should_decrement: bool,
    terminated: usize,
    expected_capacity: i32,
}

/// Every row: the `InstanceIds` form answers with `Activities` — one per instance it took
/// out of service, whatever the list's length — and the capacity change counts each
/// instance once.
#[tokio::test]
async fn a_list_request_answers_with_the_activities_of_the_instances_it_names() {
    let rows = [
        ListRow {
            case: "a list of one, decrementing",
            held: 2,
            named: &[1],
            should_decrement: true,
            terminated: 1,
            expected_capacity: 1,
        },
        ListRow {
            case: "two of three, decrementing",
            held: 3,
            named: &[1, 3],
            should_decrement: true,
            terminated: 2,
            expected_capacity: 1,
        },
        ListRow {
            case: "every instance of three, decrementing",
            held: 3,
            named: &[1, 2, 3],
            should_decrement: true,
            terminated: 3,
            expected_capacity: 0,
        },
        ListRow {
            case: "two of three in reverse launch order, decrementing",
            held: 3,
            named: &[3, 2],
            should_decrement: true,
            terminated: 2,
            expected_capacity: 1,
        },
        ListRow {
            case: "the same instance named twice, which counts once",
            held: 2,
            named: &[1, 1],
            should_decrement: true,
            terminated: 1,
            expected_capacity: 1,
        },
        ListRow {
            case: "two of three, NOT decrementing, so the group replaces both",
            held: 3,
            named: &[1, 2],
            should_decrement: false,
            terminated: 2,
            expected_capacity: 3,
        },
    ];

    let router = router_with(new_engine());

    for (row_index, row) in rows.iter().enumerate() {
        let name = format!("asg-list-{row_index}");
        let held = create_group(&router, &name, 0, row.held, row.held).await;
        let requested: Vec<String> = row
            .named
            .iter()
            .map(|position| held[*position - 1].clone())
            .collect();

        let answer = terminate(
            &router,
            Some(&name),
            None,
            &requested,
            row.should_decrement,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200, got {} with {:?} {:?}",
            row.case, answer.status, answer.code, answer.message
        );

        let activities = listed_activities(&answer.body);
        assert_eq!(
            activities.len(),
            row.terminated,
            "case {:?}: expected {} activities, got {activities:?}",
            row.case,
            row.terminated
        );
        assert!(
            single_activity(&answer.body).is_empty(),
            "case {:?}: the InstanceIds form carries no Activity member, got {:?}",
            row.case,
            answer.body
        );
        for instance_id in requested.iter() {
            assert!(
                activities
                    .iter()
                    .any(|(_, description)| description.contains(instance_id)),
                "case {:?}: expected an activity naming {instance_id}, got {activities:?}",
                row.case
            );
        }

        let reported = reported_group(&router, &name).await;
        assert_eq!(
            reported.desired_capacity, row.expected_capacity,
            "case {:?}: expected the desired capacity {}, got {reported:?}",
            row.case, row.expected_capacity
        );
        for instance_id in requested.iter() {
            assert!(
                reported.is_terminating(instance_id),
                "case {:?}: expected {instance_id} reported Terminating, got {reported:?}",
                row.case
            );
        }
        assert_eq!(
            i32::try_from(reported.active().len()).expect("a held instance count fits in an i32"),
            row.expected_capacity,
            "case {:?}: the group must hold its desired capacity in instances that are not \
             leaving, got {reported:?}",
            row.case
        );
    }
}

/// Where an id the group does not hold comes from.
#[derive(Debug, Clone, Copy)]
enum Foreign {
    /// An id of the same account's OTHER group.
    OtherGroup(usize),
    /// A well-formed instance id no instance of the account has.
    Unknown(&'static str),
}

/// One refused row: the case, the request's ids (an `Ok` position names an instance of
/// the group, an `Err` one an id it does not hold), whether the request names the group,
/// whether it names them as a list, and the id the refusal must report.
struct RefusedRow {
    case: &'static str,
    named: &'static [Result<usize, Foreign>],
    names_the_group: bool,
    as_list: bool,
    reported: Foreign,
}

/// Every row: HTTP 400, `ValidationError`, the refusal naming the instance and the group
/// it is not part of, and NOTHING terminated — every instance of either group is still in
/// service and both groups still report the capacity they held.
#[tokio::test]
async fn a_termination_naming_an_instance_the_group_does_not_hold_is_refused_whole() {
    let rows = [
        RefusedRow {
            case: "an id no instance of the account has, with the group named",
            named: &[Err(Foreign::Unknown("i-0fff0000000000001"))],
            names_the_group: true,
            as_list: false,
            reported: Foreign::Unknown("i-0fff0000000000001"),
        },
        RefusedRow {
            case: "an id no instance of the account has, with no group named",
            named: &[Err(Foreign::Unknown("i-0fff0000000000002"))],
            names_the_group: false,
            as_list: false,
            reported: Foreign::Unknown("i-0fff0000000000002"),
        },
        RefusedRow {
            case: "an instance of another group of the account, with the group named",
            named: &[Err(Foreign::OtherGroup(1))],
            names_the_group: true,
            as_list: false,
            reported: Foreign::OtherGroup(1),
        },
        RefusedRow {
            case: "a list mixing an instance of the group with one of another group",
            named: &[Ok(1), Err(Foreign::OtherGroup(2))],
            names_the_group: true,
            as_list: true,
            reported: Foreign::OtherGroup(2),
        },
        RefusedRow {
            case: "a list whose foreign id comes first",
            named: &[Err(Foreign::Unknown("i-0fff0000000000003")), Ok(2)],
            names_the_group: true,
            as_list: true,
            reported: Foreign::Unknown("i-0fff0000000000003"),
        },
    ];

    for (row_index, row) in rows.iter().enumerate() {
        // One emulator per row: the other group's instances are the foreign ids of this
        // row alone.
        let router = router_with(new_engine());
        let other = create_group(&router, "asg-other", 0, 2, 2).await;
        let name = format!("asg-refused-{row_index}");
        let held = create_group(&router, &name, 0, 2, 2).await;

        let id_of = |foreign: &Foreign| match foreign {
            Foreign::OtherGroup(position) => other[*position - 1].clone(),
            Foreign::Unknown(instance_id) => (*instance_id).to_string(),
        };
        let requested: Vec<String> = row
            .named
            .iter()
            .map(|entry| match entry {
                Ok(position) => held[*position - 1].clone(),
                Err(foreign) => id_of(foreign),
            })
            .collect();

        let group_name = row.names_the_group.then_some(name.as_str());
        let answer = if row.as_list {
            terminate(&router, group_name, None, &requested, true).await
        } else {
            terminate(&router, group_name, Some(&requested[0]), &[], true).await
        };
        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400, got {} with {:?} {:?}",
            row.case, answer.status, answer.code, answer.message
        );
        assert_eq!(
            answer.code, "ValidationError",
            "case {:?}: expected the ValidationError code, got {:?} {:?}",
            row.case, answer.code, answer.message
        );
        let expected = not_part_of_group_message(&id_of(&row.reported), group_name);
        assert_eq!(
            answer.message, expected,
            "case {:?}: expected the message {expected:?}, got {:?}",
            row.case, answer.message
        );

        for (group, instances) in [(name.as_str(), &held), ("asg-other", &other)] {
            let reported = reported_group(&router, group).await;
            assert_eq!(
                reported.active().len(),
                instances.len(),
                "case {:?}: a refused call takes no instance of {group:?} out of service, got \
                 {reported:?}",
                row.case
            );
            assert_eq!(
                reported.desired_capacity,
                i32::try_from(instances.len()).expect("a held instance count fits in an i32"),
                "case {:?}: a refused call leaves the desired capacity of {group:?} alone, got \
                 {reported:?}",
                row.case
            );
        }
    }
}

/// The request-shape refusals: a call that names no instance at all, a list that names no
/// group to resolve the ids against, and a group the account does not hold — each refused
/// `ValidationError` before anything is terminated.
#[tokio::test]
async fn a_termination_the_service_cannot_resolve_is_refused() {
    let router = router_with(new_engine());
    let held = create_group(&router, "asg-shape", 0, 2, 2).await;

    // Neither member that names the instances to terminate.
    let answer = terminate(&router, Some("asg-shape"), None, &[], true).await;
    assert_eq!(
        answer.status, 400,
        "a call naming no instance must be refused, got {} with {:?} {:?}",
        answer.status, answer.code, answer.message
    );
    assert_eq!(
        answer.code, "ValidationError",
        "expected the ValidationError code, got {:?} {:?}",
        answer.code, answer.message
    );
    assert_eq!(
        answer.message,
        "Valid requests must contain either the InstanceId or the InstanceIds parameter.",
        "expected the no-instance sentence, got {:?}",
        answer.message
    );

    // `InstanceIds` without the group that holds them: the model documents that member as
    // requiring `AutoScalingGroupName`, so the group is not resolved from the instances.
    let answer = terminate(&router, None, None, &held[..1], true).await;
    assert_eq!(
        answer.status, 400,
        "a list naming no group must be refused, got {} with {:?} {:?}",
        answer.status, answer.code, answer.message
    );
    assert_eq!(
        answer.code, "ValidationError",
        "expected the ValidationError code, got {:?} {:?}",
        answer.code, answer.message
    );
    assert_eq!(
        answer.message,
        "Valid requests must contain the AutoScalingGroupName parameter when the InstanceIds \
         parameter is specified.",
        "expected the missing-group-name sentence, got {:?}",
        answer.message
    );

    // A group the account does not hold, refused like every other group-scoped write of
    // this service: the fixed sentence whose detail field is unfilled.
    for absent in ["asg-absent", "ASG-Shape", "asg-shape-but-longer"] {
        let answer = terminate(&router, Some(absent), Some(&held[0]), &[], true).await;
        assert_eq!(
            answer.status, 400,
            "case {absent:?}: expected HTTP 400, got {} with {:?} {:?}",
            answer.status, answer.code, answer.message
        );
        assert_eq!(
            answer.code, "ValidationError",
            "case {absent:?}: expected the ValidationError code, got {:?} {:?}",
            answer.code, answer.message
        );
        assert_eq!(
            answer.message, "AutoScalingGroup name not found - null",
            "case {absent:?}: expected the no-detail absent-group sentence, got {:?}",
            answer.message
        );
    }

    // None of the refusals above touched the group that does exist.
    let reported = reported_group(&router, "asg-shape").await;
    assert_eq!(
        reported.desired_capacity, 2,
        "the refusals above left the desired capacity alone, got {reported:?}"
    );
    assert_eq!(
        reported.active().len(),
        held.len(),
        "the refusals above took no instance out of service, got {reported:?}"
    );
}

/// One capacity-bound row: the case, the group's minimum, how many instances it holds
/// (which is its desired capacity too), how many of them the request names, and whether
/// the decrement it asks for must be refused.
struct BoundRow {
    case: &'static str,
    min_size: i32,
    held: i32,
    named: usize,
    refused: bool,
}

/// The group's own minimum is the bound on the decrement a termination may ask for: a
/// request that takes the desired capacity down TO the minimum is served, and one that
/// would take it under is refused `ValidationError` with the sentence the service states
/// for that class — which says what to change — and with nothing terminated, while the
/// same request WITHOUT the decrement is served, because then the capacity does not move.
#[tokio::test]
async fn a_decrement_below_the_groups_minimum_size_is_refused() {
    let rows = [
        BoundRow {
            case: "one instance down to a minimum of nought",
            min_size: 0,
            held: 1,
            named: 1,
            refused: false,
        },
        BoundRow {
            case: "one instance of two down to a minimum of one",
            min_size: 1,
            held: 2,
            named: 1,
            refused: false,
        },
        BoundRow {
            case: "both instances down to a minimum of nought",
            min_size: 0,
            held: 2,
            named: 2,
            refused: false,
        },
        BoundRow {
            case: "one instance under a minimum of one",
            min_size: 1,
            held: 1,
            named: 1,
            refused: true,
        },
        BoundRow {
            case: "two instances under a minimum of one",
            min_size: 1,
            held: 2,
            named: 2,
            refused: true,
        },
        BoundRow {
            case: "one instance under a minimum of three",
            min_size: 3,
            held: 3,
            named: 1,
            refused: true,
        },
    ];

    for (row_index, row) in rows.iter().enumerate() {
        let router = router_with(new_engine());
        let name = format!("asg-bound-{row_index}");
        let held = create_group(&router, &name, row.min_size, row.held, row.held).await;
        let requested: Vec<String> = held[..row.named].to_vec();

        let answer = terminate(&router, Some(&name), None, &requested, true).await;
        if !row.refused {
            assert_eq!(
                answer.status, 200,
                "case {:?}: expected HTTP 200, got {} with {:?} {:?}",
                row.case, answer.status, answer.code, answer.message
            );
            let reported = reported_group(&router, &name).await;
            let expected = row.held - i32::try_from(row.named).expect("a count fits in an i32");
            assert_eq!(
                reported.desired_capacity, expected,
                "case {:?}: expected the desired capacity {expected}, got {reported:?}",
                row.case
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400, got {} with {:?} {:?}",
            row.case, answer.status, answer.code, answer.message
        );
        assert_eq!(
            answer.code, "ValidationError",
            "case {:?}: expected the ValidationError code, got {:?} {:?}",
            row.case, answer.code, answer.message
        );
        let expected = if row.held == row.min_size {
            format!(
                "Currently, desiredSize equals minSize ({}). Terminating instance without \
                 replacement will violate group's min size constraint. Either set \
                 shouldDecrementDesiredCapacity flag to false or lower group's min size.",
                row.min_size
            )
        } else {
            format!(
                "Currently, desiredSize is {} and minSize is {}. Terminating {} instances \
                 without replacement will violate group's min size constraint. Either set \
                 shouldDecrementDesiredCapacity flag to false or lower group's min size.",
                row.held, row.min_size, row.named
            )
        };
        assert_eq!(
            answer.message, expected,
            "case {:?}: expected the message {expected:?}, got {:?}",
            row.case, answer.message
        );

        // The refusal terminated nothing: the group still holds every instance in
        // service, at the capacity it had.
        let reported = reported_group(&router, &name).await;
        assert_eq!(
            reported.desired_capacity, row.held,
            "case {:?}: a refused decrement leaves the desired capacity alone, got {reported:?}",
            row.case
        );
        assert_eq!(
            reported.active().len(),
            held.len(),
            "case {:?}: a refused decrement takes no instance out of service, got {reported:?}",
            row.case
        );

        // The SAME request without the decrement is served: the capacity does not move,
        // so the group's minimum is not in the way — the group replaces what it loses.
        let answer = terminate(&router, Some(&name), None, &requested, false).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: the same request without the decrement must succeed, got {} with {:?} \
             {:?}",
            row.case, answer.status, answer.code, answer.message
        );
        let reported = reported_group(&router, &name).await;
        assert_eq!(
            reported.desired_capacity, row.held,
            "case {:?}: a termination that does not decrement leaves the capacity alone, got \
             {reported:?}",
            row.case
        );
        for instance_id in requested.iter() {
            assert!(
                reported.is_terminating(instance_id),
                "case {:?}: expected {instance_id} reported Terminating, got {reported:?}",
                row.case
            );
        }
    }
}

/// The model's own bound on how many instances ONE call may name: `TerminationInstanceIds`
/// is `@length(min: 1, max: 100)` and the member documents "You can specify up to 100
/// instances", so a list AT the hundred is served and one past it is refused — by the
/// validation of that modelled length, which runs before the handler and is what bounds
/// the per-id work one call can ask for.
#[tokio::test]
async fn a_list_at_the_modelled_maximum_is_served_and_one_past_it_is_refused() {
    let router = router_with(new_engine());
    let at_bound = create_group(
        &router,
        "asg-max",
        0,
        MODELLED_MAX_INSTANCE_IDS,
        MODELLED_MAX_INSTANCE_IDS,
    )
    .await;

    // One past the modelled maximum: every instance of the group plus one well-formed id
    // of no instance, so only the COUNT can be what the refusal is about.
    let mut past_bound: Vec<String> = at_bound.clone();
    past_bound.push("i-0fff0000000000001".to_string());
    let answer = terminate(&router, Some("asg-max"), None, &past_bound, true).await;
    assert_eq!(
        answer.status, 400,
        "a call naming {} instance ids must be refused, got {} with {:?} {:?}",
        past_bound.len(),
        answer.status,
        answer.code,
        answer.message
    );
    assert_eq!(
        answer.code, "ValidationError",
        "expected the ValidationError code past the bound, got {:?} {:?}",
        answer.code, answer.message
    );
    let expected = format!(
        "1 validation error detected: Value '{} items' at 'instanceIds' failed to satisfy \
         constraint: Member must have length less than or equal to {MODELLED_MAX_INSTANCE_IDS}",
        past_bound.len()
    );
    assert_eq!(
        answer.message, expected,
        "expected the message {expected:?} past the bound, got {:?}",
        answer.message
    );
    let reported = reported_group(&router, "asg-max").await;
    assert_eq!(
        reported.active().len(),
        at_bound.len(),
        "a call refused past the modelled maximum terminates nothing, got {} instances in \
         service",
        reported.active().len()
    );
    assert_eq!(
        reported.desired_capacity, MODELLED_MAX_INSTANCE_IDS,
        "a call refused past the modelled maximum leaves the capacity alone, got {}",
        reported.desired_capacity
    );

    // AT the bound: served, with one activity per instance named, and the capacity down by
    // all of them.
    let answer = terminate(&router, Some("asg-max"), None, &at_bound, true).await;
    assert_eq!(
        answer.status, 200,
        "a call naming {MODELLED_MAX_INSTANCE_IDS} instance ids must succeed, got {} with \
         {:?} {:?}",
        answer.status, answer.code, answer.message
    );
    assert_eq!(
        listed_activities(&answer.body).len(),
        at_bound.len(),
        "expected one activity per instance named at the bound, got {}",
        listed_activities(&answer.body).len()
    );
    let reported = reported_group(&router, "asg-max").await;
    assert_eq!(
        reported.desired_capacity, 0,
        "expected the capacity decremented by every instance terminated, got {}",
        reported.desired_capacity
    );
    assert!(
        reported.active().is_empty(),
        "expected every instance of the group leaving, got {} in service",
        reported.active().len()
    );
}

/// What a second termination of the SAME instance answers: the instance is already on its
/// way out, so nothing further is owed — the call is served, records no second activity,
/// and moves the capacity no further, while the group still reports the instance leaving.
#[tokio::test]
async fn an_instance_named_again_while_already_leaving_owes_nothing_further() {
    let router = router_with(new_engine());
    let held = create_group(&router, "asg-again", 0, 3, 3).await;

    let answer = terminate(&router, Some("asg-again"), Some(&held[0]), &[], true).await;
    assert_eq!(
        answer.status, 200,
        "the first termination must succeed, got {} with {:?} {:?}",
        answer.status, answer.code, answer.message
    );
    let reported = reported_group(&router, "asg-again").await;
    assert!(
        reported.is_terminating(&held[0]),
        "expected {} reported Terminating after the first call, got {reported:?}",
        held[0]
    );
    assert_eq!(
        reported.desired_capacity, 2,
        "expected the capacity decremented once, got {reported:?}"
    );

    // Named again, in both forms: served, with no activity for a termination that is
    // already under way and no second decrement.
    for (case, single, list) in [
        ("named again through InstanceId", Some(held[0].clone()), Vec::new()),
        (
            "named again through InstanceIds",
            None,
            vec![held[0].clone()],
        ),
    ] {
        let answer = terminate(
            &router,
            Some("asg-again"),
            single.as_deref(),
            &list,
            true,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?} {:?}",
            answer.status, answer.code, answer.message
        );
        assert!(
            single_activity(&answer.body).is_empty() && listed_activities(&answer.body).is_empty(),
            "case {case:?}: a termination already under way records no second activity, got {:?}",
            answer.body
        );
        let reported = reported_group(&router, "asg-again").await;
        assert_eq!(
            reported.desired_capacity, 2,
            "case {case:?}: the capacity is decremented once per instance taken out of \
             service, got {reported:?}"
        );
        assert!(
            reported.is_terminating(&held[0]),
            "case {case:?}: expected {} still reported Terminating, got {reported:?}",
            held[0]
        );
        assert_eq!(
            reported.active().len(),
            2,
            "case {case:?}: the group still holds its other instances, got {reported:?}"
        );
    }
}

/// The activity a termination answers with describes THAT termination: it belongs to the
/// group, names the instance in its description, is in progress with a start time and a
/// progress reading, and reports the capacity change the request asked for in its cause.
#[tokio::test]
async fn the_activity_a_termination_answers_with_describes_that_termination() {
    let router = router_with(new_engine());
    let held = create_group(&router, "asg-activity", 0, 3, 3).await;

    let answer = terminate(&router, Some("asg-activity"), Some(&held[0]), &[], true).await;
    assert_eq!(
        answer.status, 200,
        "the termination must succeed, got {} with {:?} {:?}",
        answer.status, answer.code, answer.message
    );

    for (member, expected) in [
        ("AutoScalingGroupName", "asg-activity"),
        ("StatusCode", "InProgress"),
        ("AutoScalingGroupState", "InService"),
        ("Description", &format!("Terminating EC2 instance: {}", held[0])[..]),
    ] {
        let actual = tag_value(&answer.body, member);
        assert_eq!(
            actual, expected,
            "expected the activity's {member} to be {expected:?}, got {actual:?} in {:?}",
            answer.body
        );
    }
    for member in ["ActivityId", "StartTime", "Progress", "AutoScalingGroupARN"] {
        assert!(
            !tag_value(&answer.body, member).is_empty(),
            "expected the activity to carry a {member}, got {:?}",
            answer.body
        );
    }
    // The cause says what the request asked for: the capacities the group moved between.
    let cause = xml_unescape(&tag_value(&answer.body, "Cause"));
    assert!(
        cause.contains("from 3 to 2"),
        "expected the cause to report the capacity change the request asked for, got {cause:?}"
    );

    // An activity that is in progress has not ended yet: the group's own transition is
    // what finishes it, so the response reports no end time.
    assert!(
        tag_value(&answer.body, "EndTime").is_empty(),
        "an activity still in progress reports no EndTime, got {:?}",
        answer.body
    );

    // The same activity, read back through the reader of the activity log: the response
    // reported the record the service STORED, so the reader answers the same id with the
    // same description and cause.
    let activity_id = tag_value(&answer.body, "ActivityId");
    let (status, body) = query(
        &router,
        "DescribeScalingActivities",
        &[("ActivityIds.member.1", activity_id.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "the read-back of {activity_id:?} must succeed, got {status} with {body:?}"
    );
    let read_back = listed_activities(&body);
    assert_eq!(
        read_back.len(),
        1,
        "expected the log to report exactly the activity the termination recorded, got \
         {read_back:?}"
    );
    assert_eq!(
        read_back[0],
        (
            activity_id.clone(),
            format!("Terminating EC2 instance: {}", held[0])
        ),
        "expected the stored activity to carry the id and description the response did, got \
         {read_back:?}"
    );
    assert_eq!(
        xml_unescape(&tag_value(&body, "Cause")),
        cause,
        "expected the stored activity to carry the cause the response did, got {body:?}"
    );
}
