// SPDX-License-Identifier: CC0-1.0

//! What `SkipMatching` MATCHES, end to end over HTTP.
//!
//! RULE under test: `RefreshPreferences$SkipMatching` — "if enabled (true), then Amazon EC2
//! Auto Scaling skips replacing instances that match the desired configuration. If no desired
//! configuration is specified, then it skips replacing instances that have the same launch
//! template and instance types that the Auto Scaling group was using before the start of the
//! instance refresh"
//! (`smithy_query(query="doc",shape="RefreshPreferences",member="SkipMatching")`). An
//! instance MATCHES when the launch-template version it was launched from is the version the
//! refresh's configuration names — and a launch template specification is a WAY OF NAMING one
//! version: the template by `LaunchTemplateId` or by `LaunchTemplateName`, the version by
//! number or by `$Latest`/`$Default` (which is also what an omitted `Version` means,
//! `smithy_query(query="doc",shape="LaunchTemplateSpecification",member="Version")`). So the
//! two are compared by the version they RESOLVE to in the account's own Amazon EC2 store, not
//! by how they were spelled: a refresh that names the template the group is already running,
//! however it names it, replaces nothing, while one that names another template — or another
//! version of the same template — replaces every instance.
//!
//! The rows span what the rule generalises over: which half of the specification the desired
//! configuration states (name, id), how it states the version ($Latest, $Default, the version
//! number, omitted), where it carries the specification (its own `LaunchTemplate` or the one
//! of a `MixedInstancesPolicy`), whether the template is the group's own or another, whether
//! the version is the one the group runs or a newer one, how many instances the group holds,
//! and a refresh that names no desired configuration at all.
//!
//! Both services are mounted on ONE engine and driven over their own wire protocols — awsQuery
//! for Auto Scaling, ec2Query for Amazon EC2 — so the launch templates these refreshes resolve
//! against are the ones a client created.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;

use crate::instance_refresh_lifecycle::past_refresh_steps;
use crate::support::{borrowed, ec2_query, p, query, tag_value, tag_values};

/// The image and instance type every launch template in this suite carries, beside a second
/// instance type no template of this suite launches.
const IMAGE_ID: &str = "ami-00000000000000031";
const INSTANCE_TYPE: &str = "t3.micro";
const OTHER_INSTANCE_TYPE: &str = "m5.large";

/// The template the groups in this suite are created on, and a second one no group runs.
const TEMPLATE_NAME: &str = "asg-skip-matching";
const OTHER_TEMPLATE_NAME: &str = "asg-skip-matching-other";

/// The zone the groups in this suite place their instances in.
const ZONE: &str = "us-west-2a";

/// The lifecycle state an instance that is serving carries.
const IN_SERVICE: &str = "InService";

/// One emulator with both services on one engine: the group and the launch templates belong
/// to ONE account, and one engine is what makes the two views that account.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// Create one launch template over Amazon EC2's own wire and answer the id EC2 assigned it.
async fn create_launch_template(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", IMAGE_ID),
            p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate {name} must succeed, got {status}: {body}"
    );
    let id = tag_value(&body, "launchTemplateId");
    assert!(
        !id.is_empty(),
        "CreateLaunchTemplate {name} must report the template's id, got {body}"
    );
    id
}

/// Add one more version to the template `name` and answer the version number EC2 minted,
/// which is the template's LATEST version from then on.
async fn add_launch_template_version(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplateVersion",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", IMAGE_ID),
            p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplateVersion for {name} must succeed, got {status}: {body}"
    );
    let version = tag_value(&body, "versionNumber");
    assert!(
        !version.is_empty(),
        "CreateLaunchTemplateVersion must report the version it minted, got {body}"
    );
    version
}

/// Create one group of `capacity` instances on [`TEMPLATE_NAME`], naming the template by
/// NAME and leaving the version to the model's default, and wait until its instances are
/// placed.
async fn create_group(router: &MultiServiceRouter, name: &str, capacity: i32) {
    let capacity = capacity.to_string();
    let (status, body) = query(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", name),
            ("LaunchTemplate.LaunchTemplateName", TEMPLATE_NAME),
            ("MinSize", capacity.as_str()),
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

/// The ids of the instances one group reports as `InService`, in the order it reports them.
async fn in_service_ids(router: &MultiServiceRouter, name: &str) -> Vec<String> {
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
    let ids = tag_values(&body, "InstanceId");
    let states = tag_values(&body, "LifecycleState");
    assert_eq!(
        ids.len(),
        states.len(),
        "every instance reports one lifecycle state, got {body}"
    );
    ids.into_iter()
        .zip(states)
        .filter(|(_, state)| state == IN_SERVICE)
        .map(|(id, _)| id)
        .collect()
}

/// Where the desired configuration carries its launch template specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    /// `DesiredConfiguration.LaunchTemplate`.
    Own,
    /// The launch template of `DesiredConfiguration.MixedInstancesPolicy`.
    MixedPolicy,
}

impl Carrier {
    /// The request-parameter prefix the specification's members are carried under.
    fn prefix(self) -> &'static str {
        match self {
            Self::Own => "DesiredConfiguration.LaunchTemplate",
            Self::MixedPolicy => "DesiredConfiguration.MixedInstancesPolicy.LaunchTemplate.\
                                  LaunchTemplateSpecification",
        }
    }
}

/// Which half of the specification the desired configuration states the template by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// `LaunchTemplateName`, naming the template the group was created on.
    OwnName,
    /// `LaunchTemplateId`, naming that same template by the id EC2 assigned it.
    OwnId,
    /// `LaunchTemplateName`, naming the OTHER template of the account.
    OtherName,
}

/// How the desired configuration states the version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    /// No `Version` member at all, which the model documents as `$Default`.
    Omitted,
    Default,
    Latest,
    /// The version number the group's own instances run.
    Running,
    /// A version minted AFTER the group's instances were launched, named by its number.
    Newer,
}

/// One row: how the refresh names the configuration it would give the group's instances,
/// and whether those instances already match it.
struct Row {
    case: &'static str,
    /// How many instances the group holds when the refresh starts.
    capacity: i32,
    /// The desired configuration the refresh carries, or `None` for a refresh that names
    /// none — which matches against the group's own launch source.
    desired: Option<(Carrier, Named, Version)>,
    /// The `InstanceType` the desired configuration's mixed instances policy OVERRIDES name,
    /// when it names any: a configuration that states its own instance types is matched on
    /// them too. Only a [`Carrier::MixedPolicy`] row carries one.
    override_instance_type: Option<&'static str>,
    /// Whether a second version of the group's own template is minted before the refresh,
    /// so `$Latest` and the newer version number name something the instances do not run.
    mint_newer_version: bool,
    /// How many of the group's instances the refresh must set out to replace.
    expected_to_update: i32,
}

const ROWS: &[Row] = &[
    Row {
        case: "the group's own template by name, at $Latest",
        capacity: 1,
        desired: Some((Carrier::Own, Named::OwnName, Version::Latest)),
        override_instance_type: None,
        mint_newer_version: false,
        expected_to_update: 0,
    },
    Row {
        case: "the group's own template by name, at $Default",
        capacity: 1,
        desired: Some((Carrier::Own, Named::OwnName, Version::Default)),
        override_instance_type: None,
        mint_newer_version: false,
        expected_to_update: 0,
    },
    Row {
        case: "the group's own template by name, with no version stated",
        capacity: 2,
        desired: Some((Carrier::Own, Named::OwnName, Version::Omitted)),
        override_instance_type: None,
        mint_newer_version: false,
        expected_to_update: 0,
    },
    Row {
        case: "the group's own template by id, at $Latest",
        capacity: 1,
        desired: Some((Carrier::Own, Named::OwnId, Version::Latest)),
        override_instance_type: None,
        mint_newer_version: false,
        expected_to_update: 0,
    },
    Row {
        case: "the group's own template by id, at the version number its instances run",
        capacity: 2,
        desired: Some((Carrier::Own, Named::OwnId, Version::Running)),
        override_instance_type: None,
        mint_newer_version: false,
        expected_to_update: 0,
    },
    Row {
        case: "the group's own template through a mixed instances policy, at $Latest",
        capacity: 1,
        desired: Some((Carrier::MixedPolicy, Named::OwnName, Version::Latest)),
        override_instance_type: None,
        mint_newer_version: false,
        expected_to_update: 0,
    },
    Row {
        case: "no desired configuration at all: the group's own launch source",
        capacity: 2,
        desired: None,
        override_instance_type: None,
        mint_newer_version: false,
        expected_to_update: 0,
    },
    Row {
        case: "another template of the account, by name",
        capacity: 1,
        desired: Some((Carrier::Own, Named::OtherName, Version::Latest)),
        override_instance_type: None,
        mint_newer_version: false,
        expected_to_update: 1,
    },
    Row {
        case: "a NEWER version of the group's own template, by its number",
        capacity: 1,
        desired: Some((Carrier::Own, Named::OwnName, Version::Newer)),
        override_instance_type: None,
        mint_newer_version: true,
        expected_to_update: 1,
    },
    Row {
        case: "the group's own template at $Latest once a newer version exists",
        capacity: 2,
        desired: Some((Carrier::Own, Named::OwnName, Version::Latest)),
        override_instance_type: None,
        mint_newer_version: true,
        expected_to_update: 2,
    },
    Row {
        // The "and instance types" half of the rule: the policy's overrides name the type the
        // group's instances were launched as, so they match on the type as well as on the
        // launch-template version.
        case: "a mixed instances policy whose overrides name the type the instances run",
        capacity: 2,
        desired: Some((Carrier::MixedPolicy, Named::OwnName, Version::Latest)),
        override_instance_type: Some(INSTANCE_TYPE),
        mint_newer_version: false,
        expected_to_update: 0,
    },
    Row {
        // The same version of the same template, under a configuration that names ANOTHER
        // instance type: an instance of a type the configuration does not name is not running
        // that configuration.
        case: "a mixed instances policy whose overrides name another instance type",
        capacity: 1,
        desired: Some((Carrier::MixedPolicy, Named::OwnName, Version::Latest)),
        override_instance_type: Some(OTHER_INSTANCE_TYPE),
        mint_newer_version: false,
        expected_to_update: 1,
    },
];

/// The refresh's own report for the newest refresh of a group: `(status, percentage
/// complete, instances to update, end time)`.
async fn newest_refresh(
    router: &MultiServiceRouter,
    name: &str,
) -> (String, String, String, String) {
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
    (
        tag_value(&body, "Status"),
        tag_value(&body, "PercentageComplete"),
        tag_value(&body, "InstancesToUpdate"),
        tag_value(&body, "EndTime"),
    )
}

#[tokio::test]
async fn skip_matching_leaves_every_instance_that_already_runs_the_target_version() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        // One emulator per row: each row is its own account, so the ids of one row never
        // explain another's.
        let router = emulator();
        let own_template_id = create_launch_template(&router, TEMPLATE_NAME).await;
        create_launch_template(&router, OTHER_TEMPLATE_NAME).await;

        let group = format!("asg-skip-matching-{index}");
        create_group(&router, &group, row.capacity).await;
        let before = in_service_ids(&router, &group).await;
        assert_eq!(
            before.len(),
            usize::try_from(row.capacity).expect("a capacity is a count"),
            "case {case:?}: the group must serve the capacity it was created with, got {before:?}"
        );

        // The version the group's instances run is the one that was its template's default
        // when they launched; a newer version is minted only where the row asks for one, and
        // only AFTER those instances exist.
        let running_version = "1";
        let newer_version = if row.mint_newer_version {
            add_launch_template_version(&router, TEMPLATE_NAME).await
        } else {
            String::new()
        };

        let mut params = vec![
            p("AutoScalingGroupName", group.as_str()),
            p("Strategy", "Rolling"),
            p("Preferences.MinHealthyPercentage", "100"),
            p("Preferences.InstanceWarmup", "0"),
            p("Preferences.SkipMatching", "true"),
        ];
        if let Some((carrier, named, version)) = row.desired {
            let prefix = carrier.prefix();
            let (member, value) = match named {
                Named::OwnName => ("LaunchTemplateName", TEMPLATE_NAME.to_string()),
                Named::OwnId => ("LaunchTemplateId", own_template_id.clone()),
                Named::OtherName => ("LaunchTemplateName", OTHER_TEMPLATE_NAME.to_string()),
            };
            params.push(p(&format!("{prefix}.{member}"), value));
            let stated = match version {
                Version::Omitted => None,
                Version::Default => Some("$Default".to_string()),
                Version::Latest => Some("$Latest".to_string()),
                Version::Running => Some(running_version.to_string()),
                Version::Newer => Some(newer_version.clone()),
            };
            if let Some(stated) = stated {
                assert!(
                    !stated.is_empty(),
                    "case {case:?}: the row must state a version it minted"
                );
                params.push(p(&format!("{prefix}.Version"), stated));
            }
            if let Some(instance_type) = row.override_instance_type {
                assert_eq!(
                    carrier,
                    Carrier::MixedPolicy,
                    "case {case:?}: only a mixed instances policy carries overrides"
                );
                params.push(p(
                    "DesiredConfiguration.MixedInstancesPolicy.LaunchTemplate.Overrides.\
                     member.1.InstanceType",
                    instance_type,
                ));
            }
        }

        let (status, body) = query(&router, "StartInstanceRefresh", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartInstanceRefresh expected 200, got {status}: {body}"
        );
        assert!(
            !tag_value(&body, "InstanceRefreshId").is_empty(),
            "case {case:?}: the answer must carry the refresh's id, got {body}"
        );

        // What the refresh set out to replace is what SkipMatching left for it.
        let (_, _, to_update, _) = newest_refresh(&router, &group).await;
        assert_eq!(
            to_update,
            row.expected_to_update.to_string(),
            "case {case:?}: expected {} instance(s) left to update, got {to_update:?}",
            row.expected_to_update
        );

        // Once every replacement the clock owed has come due, the refresh has succeeded
        // whether it replaced anything or not.
        let steps = u32::try_from(row.expected_to_update).expect("a count of instances") + 1;
        past_refresh_steps(steps).await;
        let (status, percentage, to_update, end_time) = newest_refresh(&router, &group).await;
        assert_eq!(
            status, "Successful",
            "case {case:?}: expected the refresh to have succeeded, got {status:?}"
        );
        assert_eq!(
            percentage, "100",
            "case {case:?}: a succeeded refresh is complete, got {percentage:?}"
        );
        assert_eq!(
            to_update, "0",
            "case {case:?}: a succeeded refresh has nothing left to update, got {to_update:?}"
        );
        assert!(
            !end_time.is_empty(),
            "case {case:?}: a succeeded refresh reports when it ended"
        );

        // The group's membership is the requirement: a fully matching refresh costs no
        // instance churn, and one that matches nothing replaces every instance.
        let after = in_service_ids(&router, &group).await;
        assert_eq!(
            after.len(),
            before.len(),
            "case {case:?}: the group must serve its capacity after the refresh, got {after:?}"
        );
        if row.expected_to_update == 0 {
            assert_eq!(
                after, before,
                "case {case:?}: a fully matching refresh must not replace any instance, \
                 got {after:?} where {before:?} was serving"
            );
        } else {
            assert!(
                after.iter().all(|id| !before.contains(id)),
                "case {case:?}: every instance of {before:?} had to be replaced, got {after:?}"
            );
        }
    }
}
