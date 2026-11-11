// SPDX-License-Identifier: CC0-1.0

//! An instance refresh is a stored resource whose progress is the work it has done.
//!
//! The rule under test: `StartInstanceRefresh` stores one refresh for the group it
//! names, answers the unique id it is tracked by, and the clock then REPLACES the
//! group's instances one at a time — so the progress `DescribeInstanceRefreshes`
//! reports (its status, the percentage complete and the instances left to update) is
//! what those replacements have actually done to the group, the group's instance ids
//! are the ids of the replacements once it has succeeded, and a group has at most one
//! active refresh at a time.
//!
//! The tables below span what that rule generalises over: how many instances the group
//! holds (none, one, two, three), which strategy the request names (none, so the
//! model's default, or either value the enum admits), whether the refresh carries a
//! desired configuration — which the group's own settings reflect once the refresh
//! succeeds — and whether `SkipMatching` leaves instances that already run the target
//! configuration alone. A second table spans the requests the operation refuses.

use std::time::Duration;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

use crate::support::{new_engine, query, router_with, tag_value, tag_values, xml_unescape};

/// The launch template the groups in this suite launch from.
const LAUNCH_TEMPLATE: &str = "lt-refresh";

/// The launch template a desired configuration moves a group to.
const DESIRED_TEMPLATE: &str = "lt-refresh-next";

/// The zone the groups in this suite place their instances in.
const ZONE: &str = "us-west-2a";

/// Wait until the clock has certainly brought `steps` of a refresh's own steps due,
/// each of which starts a lifecycle transition of its own. Derived from the service's
/// own spans, so this and the implementation cannot drift apart.
///
/// `pub(crate)` so a sibling suite that waits for a refresh's steps waits the same span
/// rather than growing a copy of this.
pub(crate) async fn past_refresh_steps(steps: u32) {
    let span = autoscaling::state::INSTANCE_REFRESH_REPLACEMENT
        .max(autoscaling::state::LIFECYCLE_TRANSITION);
    tokio::time::sleep(span * (steps + 2) + Duration::from_millis(250)).await;
}

/// One row of the refresh table.
struct Row {
    case: &'static str,
    /// How many instances the group holds when the refresh starts.
    capacity: i32,
    /// The `Strategy` the request names, or `None` to leave it to the model's default.
    strategy: Option<&'static str>,
    /// Whether the refresh carries a `DesiredConfiguration` naming another launch
    /// template.
    desired_configuration: bool,
    /// Whether the request asks for `SkipMatching`.
    skip_matching: bool,
    /// How many instances the refresh must set out to replace.
    expected_to_update: i32,
}

/// Create one group holding `capacity` instances and wait until they are placed.
async fn create_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    capacity: i32,
) {
    let capacity = capacity.to_string();
    let (status, body) = query(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", name),
            ("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE),
            ("MinSize", "0"),
            ("MaxSize", capacity.as_str()),
            ("DesiredCapacity", capacity.as_str()),
            ("AvailabilityZones.member.1", ZONE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateAutoScalingGroup for {name} expected 200, got {status}: {body}"
    );
    past_refresh_steps(0).await;
}

/// The instance ids one group reports.
async fn group_instance_ids(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> Vec<String> {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups for {name} expected 200, got {status}: {body}"
    );
    tag_values(&body, "InstanceId")
}

/// What `DescribeInstanceRefreshes` reports for the newest refresh of a group.
#[derive(Debug, Clone, Default)]
struct ReportedRefresh {
    instance_refresh_id: String,
    auto_scaling_group_name: String,
    status: String,
    strategy: String,
    percentage_complete: String,
    instances_to_update: String,
    end_time: String,
    /// The launch template name the refresh's desired configuration names, when it
    /// carries one.
    desired_template: String,
}

/// Describe one group's refreshes and read the newest one out of the answer.
async fn newest_refresh(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> (Vec<String>, ReportedRefresh) {
    let (status, body) = query(
        router,
        "DescribeInstanceRefreshes",
        &[("AutoScalingGroupName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstanceRefreshes for {name} expected 200, got {status}: {body}"
    );
    let ids = tag_values(&body, "InstanceRefreshId");
    let reported = ReportedRefresh {
        instance_refresh_id: tag_value(&body, "InstanceRefreshId"),
        auto_scaling_group_name: tag_value(&body, "AutoScalingGroupName"),
        status: tag_value(&body, "Status"),
        strategy: tag_value(&body, "Strategy"),
        percentage_complete: tag_value(&body, "PercentageComplete"),
        instances_to_update: tag_value(&body, "InstancesToUpdate"),
        end_time: tag_value(&body, "EndTime"),
        desired_template: tag_value(&body, "LaunchTemplateName"),
    };
    (ids, reported)
}

/// Start one refresh as the row asks for it, and answer the status, body and the id
/// the answer carries.
async fn start_refresh(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    row: &Row,
) -> (u16, String) {
    let mut params: Vec<(&str, &str)> = vec![("AutoScalingGroupName", name)];
    if let Some(strategy) = row.strategy {
        params.push(("Strategy", strategy));
    }
    if row.desired_configuration {
        params.push((
            "DesiredConfiguration.LaunchTemplate.LaunchTemplateName",
            DESIRED_TEMPLATE,
        ));
    }
    if row.skip_matching {
        params.push(("Preferences.SkipMatching", "true"));
    }
    let (status, body) = query(router, "StartInstanceRefresh", &params).await;
    (status, body)
}

/// Every row: the refresh is stored, reported while it runs, and once it has succeeded
/// the group holds the instances it launched.
#[tokio::test]
async fn an_instance_refresh_replaces_the_groups_instances() {
    let rows = [
        Row {
            case: "a group holding no instance: the refresh has nothing to replace",
            capacity: 0,
            strategy: None,
            desired_configuration: false,
            skip_matching: false,
            expected_to_update: 0,
        },
        Row {
            case: "one instance, the strategy left to the model's default",
            capacity: 1,
            strategy: None,
            desired_configuration: false,
            skip_matching: false,
            expected_to_update: 1,
        },
        Row {
            case: "two instances, the Rolling strategy named explicitly",
            capacity: 2,
            strategy: Some("Rolling"),
            desired_configuration: false,
            skip_matching: false,
            expected_to_update: 2,
        },
        Row {
            case: "three instances, the ReplaceRootVolume strategy",
            capacity: 3,
            strategy: Some("ReplaceRootVolume"),
            desired_configuration: false,
            skip_matching: false,
            expected_to_update: 3,
        },
        Row {
            case: "two instances and a desired configuration the group moves to",
            capacity: 2,
            strategy: None,
            desired_configuration: true,
            skip_matching: false,
            expected_to_update: 2,
        },
        Row {
            case: "SkipMatching on instances that already run the group's own template",
            capacity: 2,
            strategy: None,
            desired_configuration: false,
            skip_matching: true,
            expected_to_update: 0,
        },
        Row {
            case: "SkipMatching with a desired configuration none of them runs",
            capacity: 2,
            strategy: Some("Rolling"),
            desired_configuration: true,
            skip_matching: true,
            expected_to_update: 2,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One emulator per row: each row is its own account, so the ids of one row
        // never explain another's.
        let router = router_with(new_engine());
        let name = format!("asg-refresh-{index}");
        create_group(&router, &name, row.capacity).await;
        let before = group_instance_ids(&router, &name).await;
        assert_eq!(
            before.len(),
            usize::try_from(row.capacity).expect("a capacity is a count"),
            "case {:?}: the group must hold the capacity it was created with, got {before:?}",
            row.case
        );

        let (status, body) = start_refresh(&router, &name, row).await;
        assert_eq!(
            status, 200,
            "case {:?}: StartInstanceRefresh expected 200, got {status}: {body}",
            row.case
        );
        let instance_refresh_id = tag_value(&body, "InstanceRefreshId");
        assert!(
            !instance_refresh_id.is_empty(),
            "case {:?}: the answer must carry the refresh's id, got {body}",
            row.case
        );

        // Asked for FIRST, while the refresh it was started beside is certainly still
        // running: a group has only one active refresh at a time.
        let (status, body) = start_refresh(&router, &name, row).await;
        assert_eq!(
            status, 400,
            "case {:?}: a second refresh must be refused, got {status}: {body}",
            row.case
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "InstanceRefreshInProgress",
            "case {:?}: expected the InstanceRefreshInProgress code, got {body}",
            row.case
        );
        assert_eq!(
            xml_unescape(&tag_value(&body, "Message")),
            IN_PROGRESS_MESSAGE,
            "case {:?}: expected the fixed in-progress sentence, got {body}",
            row.case
        );

        // While it runs it is the group's refresh, under that same id.
        let (ids, running) = newest_refresh(&router, &name).await;
        assert_eq!(
            ids,
            vec![instance_refresh_id.clone()],
            "case {:?}: the group reports exactly the refresh it was given, got {ids:?}",
            row.case
        );
        assert_eq!(
            running.auto_scaling_group_name, name,
            "case {:?}: the refresh belongs to the group it was started on, got {running:?}",
            row.case
        );
        assert!(
            running.status == "Pending" || running.status == "InProgress",
            "case {:?}: a refresh that has just started is still running, got {running:?}",
            row.case
        );
        assert_eq!(
            running.strategy,
            row.strategy.unwrap_or("Rolling"),
            "case {:?}: expected the strategy the request asked for, got {running:?}",
            row.case
        );
        assert_eq!(
            running.instances_to_update,
            row.expected_to_update.to_string(),
            "case {:?}: expected {} instances to update, got {running:?}",
            row.case,
            row.expected_to_update
        );
        if row.desired_configuration {
            assert_eq!(
                running.desired_template, DESIRED_TEMPLATE,
                "case {:?}: the refresh reports the desired configuration it carries, \
                 got {running:?}",
                row.case
            );
        }

        let (status, body) = start_refresh(&router, &name, row).await;
        assert_eq!(
            status, 400,
            "case {:?}: a second refresh must still be refused, got {status}: {body}",
            row.case
        );

        // Once every replacement the clock owed has come due, the refresh has
        // succeeded and reports no instance left to update.
        let steps = u32::try_from(row.expected_to_update).expect("a count of instances") + 1;
        past_refresh_steps(steps).await;
        let (_, finished) = newest_refresh(&router, &name).await;
        assert_eq!(
            finished.status, "Successful",
            "case {:?}: expected the refresh to have succeeded, got {finished:?}",
            row.case
        );
        assert_eq!(
            finished.percentage_complete, "100",
            "case {:?}: a succeeded refresh is complete, got {finished:?}",
            row.case
        );
        assert_eq!(
            finished.instances_to_update, "0",
            "case {:?}: a succeeded refresh has nothing left to update, got {finished:?}",
            row.case
        );
        assert!(
            !finished.end_time.is_empty(),
            "case {:?}: a succeeded refresh reports when it ended, got {finished:?}",
            row.case
        );
        assert_eq!(
            finished.instance_refresh_id, instance_refresh_id,
            "case {:?}: the refresh keeps the id it was given, got {finished:?}",
            row.case
        );

        // The group holds its capacity again, and every instance the refresh replaced
        // is gone: the ids are the ids of the instances it launched.
        let after = group_instance_ids(&router, &name).await;
        assert_eq!(
            after.len(),
            usize::try_from(row.capacity).expect("a capacity is a count"),
            "case {:?}: the group holds its capacity after the refresh, got {after:?}",
            row.case
        );
        let kept = after.iter().filter(|id| before.contains(id)).count();
        let expected_kept = usize::try_from(row.capacity - row.expected_to_update)
            .expect("the instances the refresh skipped are a count");
        assert_eq!(
            kept, expected_kept,
            "case {:?}: expected {expected_kept} of the instances {before:?} to be kept, \
             got {after:?}",
            row.case
        );

        // "Once the instance refresh succeeds, Amazon EC2 Auto Scaling updates the
        // settings of the Auto Scaling group to reflect the new desired configuration"
        // (smithy_query(query="operation",operation="StartInstanceRefresh")).
        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the read-back expected 200, got {status}: {body}",
            row.case
        );
        let expected_template = if row.desired_configuration {
            DESIRED_TEMPLATE
        } else {
            LAUNCH_TEMPLATE
        };
        assert_eq!(
            tag_value(&body, "LaunchTemplateName"),
            expected_template,
            "case {:?}: expected the group to launch from {expected_template}, got {body}",
            row.case
        );
    }
}

/// The sentence a second refresh on one group is refused with, which has no variable
/// part.
const IN_PROGRESS_MESSAGE: &str =
    "An Instance Refresh is already in progress and blocks the execution of this Instance Refresh.";

/// One row of the refusal table: the request's members beyond the group name, and
/// whether the service accepts it.
struct Refusal {
    case: &'static str,
    members: &'static [(&'static str, &'static str)],
    /// The wire error code expected, or `None` for a request the service accepts.
    code: Option<&'static str>,
}

/// The preference combinations the service refuses, and the neighbouring ones it
/// accepts: the rules the model documents on the members themselves.
#[tokio::test]
async fn the_preferences_a_refresh_refuses() {
    let rows = [
        Refusal {
            case: "no preferences at all",
            members: &[],
            code: None,
        },
        Refusal {
            case: "MinHealthyPercentage alone",
            members: &[("Preferences.MinHealthyPercentage", "50")],
            code: None,
        },
        Refusal {
            case: "MaxHealthyPercentage without MinHealthyPercentage",
            members: &[("Preferences.MaxHealthyPercentage", "150")],
            code: Some("ValidationError"),
        },
        Refusal {
            case: "a healthy range exactly 100 wide",
            members: &[
                ("Preferences.MinHealthyPercentage", "50"),
                ("Preferences.MaxHealthyPercentage", "150"),
            ],
            code: None,
        },
        Refusal {
            case: "a healthy range wider than 100",
            members: &[
                ("Preferences.MinHealthyPercentage", "20"),
                ("Preferences.MaxHealthyPercentage", "200"),
            ],
            code: Some("ValidationError"),
        },
        Refusal {
            case: "checkpoint thresholds in ascending order",
            members: &[
                ("Preferences.CheckpointPercentages.member.1", "20"),
                ("Preferences.CheckpointPercentages.member.2", "60"),
                ("Preferences.CheckpointPercentages.member.3", "100"),
            ],
            code: None,
        },
        Refusal {
            case: "checkpoint thresholds out of order",
            members: &[
                ("Preferences.CheckpointPercentages.member.1", "60"),
                ("Preferences.CheckpointPercentages.member.2", "20"),
            ],
            code: Some("ValidationError"),
        },
        Refusal {
            case: "a checkpoint threshold repeated",
            members: &[
                ("Preferences.CheckpointPercentages.member.1", "50"),
                ("Preferences.CheckpointPercentages.member.2", "50"),
            ],
            code: Some("ValidationError"),
        },
        Refusal {
            case: "every other preference the model carries",
            members: &[
                ("Preferences.InstanceWarmup", "120"),
                ("Preferences.AutoRollback", "true"),
                ("Preferences.ScaleInProtectedInstances", "Ignore"),
                ("Preferences.StandbyInstances", "Terminate"),
                ("Preferences.BakeTime", "60"),
            ],
            code: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let router = router_with(new_engine());
        let name = format!("asg-preferences-{index}");
        create_group(&router, &name, 0).await;

        let mut params: Vec<(&str, &str)> = vec![("AutoScalingGroupName", name.as_str())];
        params.extend_from_slice(row.members);
        let (status, body) = query(&router, "StartInstanceRefresh", &params).await;
        match row.code {
            None => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected the refresh to be accepted, got {status}: {body}",
                    row.case
                );
                assert!(
                    !tag_value(&body, "InstanceRefreshId").is_empty(),
                    "case {:?}: an accepted refresh carries an id, got {body}",
                    row.case
                );
            }
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {:?}: expected the refresh to be refused, got {status}: {body}",
                    row.case
                );
                assert_eq!(
                    tag_value(&body, "Code"),
                    code,
                    "case {:?}: expected the {code} code, got {body}",
                    row.case
                );
            }
        }
    }
}

/// A refresh of a group the account does not hold is refused by its name, and so is a
/// read of its refreshes: neither invents a group.
#[tokio::test]
async fn an_absent_group_is_refused_by_both_operations() {
    let router = router_with(new_engine());

    // Names differing in length, case and character set, each of which the boundary
    // validator admits, so only the handler can refuse them.
    let names = [
        "a",
        "asg-does-not-exist",
        "ASG-UPPER-CASE",
        "asg.name_with-separators 1",
        "グループ-ünïcode",
    ];

    // Each operation answers the absent-group refusal in the form IT serves: the read
    // fills the detail field after the dash with the group it was asked for, while the
    // refresh leaves that field unfilled.
    for action in ["StartInstanceRefresh", "DescribeInstanceRefreshes"] {
        for name in names {
            let (status, body) = query(&router, action, &[("AutoScalingGroupName", name)]).await;
            assert_eq!(
                status, 400,
                "case {action} {name:?}: expected HTTP 400, got {status}: {body}"
            );
            assert_eq!(
                tag_value(&body, "Code"),
                "ValidationError",
                "case {action} {name:?}: expected the ValidationError code, got {body}"
            );
            let expected = if action == "DescribeInstanceRefreshes" {
                format!("AutoScalingGroup name not found - AutoScalingGroup {name} not found")
            } else {
                "AutoScalingGroup name not found - null".to_string()
            };
            assert_eq!(
                xml_unescape(&tag_value(&body, "Message")),
                expected,
                "case {action} {name:?}: expected the message {expected:?}, got {body}"
            );
        }
    }
}

/// A group's refreshes are reported newest first, one page at a time, and a token this
/// service never issued is refused.
#[tokio::test]
async fn the_refreshes_of_a_group_are_paged_newest_first() {
    let router = router_with(new_engine());
    let name = "asg-refresh-history";
    create_group(&router, name, 0).await;

    // Three refreshes, each started once the one before it has finished: a group has
    // only one active refresh at a time, so they are serialized.
    let mut ids = Vec::new();
    for _ in 0..3 {
        let (status, body) = query(
            &router,
            "StartInstanceRefresh",
            &[("AutoScalingGroupName", name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "starting a refresh expected 200, got {status}: {body}"
        );
        ids.push(tag_value(&body, "InstanceRefreshId"));
        past_refresh_steps(1).await;
    }
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        ids.len(),
        "each refresh is tracked by its own id, got {ids:?}"
    );

    let (status, body) = query(
        &router,
        "DescribeInstanceRefreshes",
        &[("AutoScalingGroupName", name)],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    let reported = tag_values(&body, "InstanceRefreshId");
    let newest_first: Vec<String> = ids.iter().rev().cloned().collect();
    assert_eq!(
        reported, newest_first,
        "expected the refreshes newest first, got {reported:?} for {ids:?}"
    );

    // One page at a time: the first page holds one refresh and names where the next
    // one starts.
    let (status, body) = query(
        &router,
        "DescribeInstanceRefreshes",
        &[("AutoScalingGroupName", name), ("MaxRecords", "1")],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        tag_values(&body, "InstanceRefreshId"),
        vec![newest_first[0].clone()],
        "the first page holds the newest refresh alone, got {body}"
    );
    let next_token = tag_value(&body, "NextToken");
    assert!(
        !next_token.is_empty(),
        "a page that does not hold everything names the next one, got {body}"
    );

    let (status, body) = query(
        &router,
        "DescribeInstanceRefreshes",
        &[
            ("AutoScalingGroupName", name),
            ("MaxRecords", "1"),
            ("NextToken", next_token.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        tag_values(&body, "InstanceRefreshId"),
        vec![newest_first[1].clone()],
        "the second page resumes where the first ended, got {body}"
    );

    // A token this service never issued names no page to resume from.
    let (status, body) = query(
        &router,
        "DescribeInstanceRefreshes",
        &[
            ("AutoScalingGroupName", name),
            ("NextToken", "not-a-token-this-service-issued"),
        ],
    )
    .await;
    assert_eq!(status, 400, "expected 400, got {status}: {body}");
    assert_eq!(
        tag_value(&body, "Code"),
        "InvalidNextToken",
        "expected the InvalidNextToken code, got {body}"
    );

    // A group that has never been refreshed reports no refresh rather than refusing.
    create_group(&router, "asg-never-refreshed", 0).await;
    let (status, body) = query(
        &router,
        "DescribeInstanceRefreshes",
        &[("AutoScalingGroupName", "asg-never-refreshed")],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert!(
        tag_values(&body, "InstanceRefreshId").is_empty(),
        "a group nobody has refreshed reports no refresh, got {body}"
    );
}
