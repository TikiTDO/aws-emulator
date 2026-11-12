// SPDX-License-Identifier: CC0-1.0

//! What a CodeDeploy read reports for a collection member of a resource that
//! always has one, driven over the wire.
//!
//! RULE under test: a read reports every collection member the resource ALWAYS
//! holds as the list it holds — the empty list included — rather than omitting
//! the member, so a client can index it instead of distinguishing absent from
//! empty. The members are the ones the model declares on the projections a read
//! answers with: `GenericRevisionInfo$deploymentGroups`
//! (`smithy_query(query="shape",shape="GenericRevisionInfo")`, "The deployment
//! groups for which this is the current target revision"), `InstanceInfo$tags`
//! (`smithy_query(query="shape",shape="InstanceInfo")`, "The tags currently
//! associated with the on-premises instance") and the deployment group's own
//! selector and trigger lists (`smithy_query(query="shape",shape="DeploymentGroupInfo")`:
//! ec2TagFilters, onPremisesInstanceTagFilters, autoScalingGroups,
//! triggerConfigurations).
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle`.

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{deployment_on_free_group, s3_revision, Emulator};

/// The service role every deployment group here is created with.
const ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The account id the IAM user ARN of a registered instance carries.
const ACCOUNT: &str = "123456789012";

/// The four revision forms the model holds, one location each
/// (`smithy_query(query="shape",shape="RevisionLocation")`: s3Location,
/// gitHubLocation, string, appSpecContent; `revisionType` only says which to
/// read). A row per form, so the rule is asserted over the whole class of
/// revisions rather than the S3 one alone.
fn revision_forms() -> Vec<(&'static str, Value)> {
    vec![
        ("an S3 revision", s3_revision("artifacts", "app/one.zip")),
        (
            "a GitHub revision",
            json!({
                "revisionType": "GitHub",
                "gitHubLocation": {
                    "repository": "octocorp/app",
                    "commitId": "0123456789abcdef0123456789abcdef01234567",
                },
            }),
        ),
        (
            "a string revision",
            json!({"revisionType": "String", "string": {"content": "version: 0.0", "sha256": ""}}),
        ),
        (
            "an AppSpecContent revision",
            json!({
                "revisionType": "AppSpecContent",
                "appSpecContent": {"content": "version: 0.0\nresources: []", "sha256": ""},
            }),
        ),
    ]
}

/// One row of the targeting table: the case name, the deployment to make before
/// the read (a group name and the revision deployed to it, or nothing), and the
/// deployment groups the tracked revision must then report.
type TargetCase<'a> = (&'a str, Option<(&'a str, &'a Value)>, &'a [&'a str]);

/// What one row of the tagging table does to the instance before the read.
enum Step {
    /// Nothing: the instance as it was registered.
    None,
    /// `AddTagsToOnPremisesInstances` with these key/value pairs.
    Add(&'static [(&'static str, &'static str)]),
    /// `RemoveTagsFromOnPremisesInstances` with these key/value pairs.
    Remove(&'static [(&'static str, &'static str)]),
}

/// One row of the tagging table: the case name, what it does to the instance's
/// tags, and the tags the instance must then report.
type TagCase<'a> = (&'a str, Step, &'a [(&'a str, &'a str)]);

/// Create an application, answering its name.
async fn application(emulator: &Emulator, name: &str) -> String {
    emulator
        .ok("CreateApplication", json!({"applicationName": name}))
        .await;
    name.to_string()
}

/// Create a deployment group named `group` on `application`.
async fn deployment_group(emulator: &Emulator, application: &str, group: &str) {
    emulator
        .ok(
            "CreateDeploymentGroup",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "serviceRoleArn": ROLE,
            }),
        )
        .await;
}

/// The `revisionInfo` `GetApplicationRevision` answers for `revision`.
async fn revision_info(emulator: &Emulator, application: &str, revision: &Value) -> Value {
    emulator
        .ok(
            "GetApplicationRevision",
            json!({"applicationName": application, "revision": revision}),
        )
        .await["revisionInfo"]
        .clone()
}

/// The `deploymentGroups` a `revisionInfo` reports, as names.
fn reported_groups(info: &Value, case: &str) -> Vec<String> {
    let groups = info["deploymentGroups"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: expected deploymentGroups to be reported as a list, got {info:?}")
    });
    groups
        .iter()
        .filter_map(|group| group.as_str().map(str::to_string))
        .collect()
}

/// RULE: `revisionInfo.deploymentGroups` is reported for every registered
/// revision, whatever form it takes — the empty list for one no group targets.
///
/// Rows differ along the revision FORM, which is what the rule generalises over:
/// a registered revision is a registered revision however it names its artifact.
#[tokio::test]
async fn a_registered_revision_no_group_targets_reports_an_empty_group_list() {
    let emulator = Emulator::new();
    let name = application(&emulator, "revision-collections").await;
    // A group exists, and targets nothing: the empty list is about what targets
    // the revision, not about whether the application has any groups at all.
    deployment_group(&emulator, &name, "revision-collections-dg").await;

    for (case, revision) in revision_forms() {
        emulator
            .ok(
                "RegisterApplicationRevision",
                json!({"applicationName": name, "revision": revision}),
            )
            .await;
        let info = revision_info(&emulator, &name, &revision).await;
        assert_eq!(
            info["deploymentGroups"],
            json!([]),
            "case {case:?}: expected deploymentGroups [], got {info:?}"
        );
        // The member a read reports is the list, not a null: a client indexes it.
        assert!(
            info["deploymentGroups"].is_array(),
            "case {case:?}: expected deploymentGroups to be a list, got {info:?}"
        );
        // Registration dates the revision without making it anybody's target.
        assert!(
            info["registerTime"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: expected a registerTime, got {info:?}"
        );
    }
}

/// RULE: the same member reports exactly the groups whose current target
/// revision it is — so it grows, shrinks and empties with the deployments, and is
/// present at every count.
///
/// Rows differ along the COUNT the rule generalises over: nothing targeting the
/// revision, one group, two groups, and back to one and to none as later
/// deployments supersede it.
#[tokio::test]
async fn a_revision_reports_every_group_targeting_it_at_any_count() {
    let emulator = Emulator::new();
    let name = application(&emulator, "revision-targets").await;
    let groups = ["alpha-dg", "beta-dg", "gamma-dg"];
    for group in groups {
        deployment_group(&emulator, &name, group).await;
    }
    let revision = s3_revision("artifacts", "app/tracked.zip");
    let newer = s3_revision("artifacts", "app/newer.zip");
    emulator
        .ok(
            "RegisterApplicationRevision",
            json!({"applicationName": name, "revision": revision}),
        )
        .await;

    /// Deploy `revision` to `group`, waiting out a deployment of that group that
    /// is still active (a group runs one deployment at a time).
    async fn deploy(emulator: &Emulator, application: &str, group: &str, revision: &Value) {
        deployment_on_free_group(
            emulator,
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": revision,
            }),
        )
        .await;
    }

    let cases: &[TargetCase<'_>] = &[
        ("no deployment yet", None, &[]),
        (
            "one group targets it",
            Some(("alpha-dg", &revision)),
            &["alpha-dg"],
        ),
        (
            "two groups target it",
            Some(("beta-dg", &revision)),
            &["alpha-dg", "beta-dg"],
        ),
        (
            "three groups target it",
            Some(("gamma-dg", &revision)),
            &["alpha-dg", "beta-dg", "gamma-dg"],
        ),
        (
            "one group moved on",
            Some(("beta-dg", &newer)),
            &["alpha-dg", "gamma-dg"],
        ),
        (
            "another group moved on",
            Some(("alpha-dg", &newer)),
            &["gamma-dg"],
        ),
        ("every group moved on", Some(("gamma-dg", &newer)), &[]),
    ];

    for (case, deployment, expected) in cases {
        if let Some((group, deployed)) = deployment {
            deploy(&emulator, &name, group, deployed).await;
        }
        let info = revision_info(&emulator, &name, &revision).await;
        let reported = reported_groups(&info, case);
        assert_eq!(
            reported,
            expected
                .iter()
                .map(|group| (*group).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected deploymentGroups {expected:?}, got {reported:?}"
        );
    }
}

/// The `instanceInfo` the two readers of an on-premises instance answer for
/// `name`: the single-instance read and the batch read, which must agree.
async fn instance_infos(emulator: &Emulator, name: &str, case: &str) -> Value {
    let single = emulator
        .ok("GetOnPremisesInstance", json!({"instanceName": name}))
        .await["instanceInfo"]
        .clone();
    let batched = emulator
        .ok(
            "BatchGetOnPremisesInstances",
            json!({"instanceNames": [name]}),
        )
        .await["instanceInfos"][0]
        .clone();
    assert_eq!(
        single, batched,
        "case {case:?}: the two readers report the same record, got {single:?} and {batched:?}"
    );
    single
}

/// RULE: `instanceInfo.tags` is reported as the tag list the instance holds, the
/// empty list included — on registration, after tags are added, and after they
/// are all removed again.
///
/// Rows differ along the COUNT and the HISTORY the rule generalises over: never
/// tagged, one tag, several tags, and emptied by removal (which is the same
/// empty list a never-tagged instance reports).
#[tokio::test]
async fn an_on_premises_instance_reports_the_tags_it_holds() {
    let emulator = Emulator::new();
    let name = "collections-host";
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({
                "instanceName": name,
                "iamUserArn": format!("arn:aws:iam::{ACCOUNT}:user/{name}"),
            }),
        )
        .await;

    let cases: &[TagCase<'_>] = &[
        ("registered with no tags", Step::None, &[]),
        (
            "one tag added",
            Step::Add(&[("Env", "prod")]),
            &[("Env", "prod")],
        ),
        (
            "two more tags added",
            Step::Add(&[("Fleet", "blue"), ("Owner", "team")]),
            &[("Env", "prod"), ("Fleet", "blue"), ("Owner", "team")],
        ),
        (
            "one tag removed",
            Step::Remove(&[("Fleet", "blue")]),
            &[("Env", "prod"), ("Owner", "team")],
        ),
        (
            "every remaining tag removed",
            Step::Remove(&[("Env", "prod"), ("Owner", "team")]),
            &[],
        ),
    ];

    let tag_list = |tags: &[(&str, &str)]| {
        tags.iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect::<Vec<_>>()
    };

    for (case, step, expected) in cases {
        match step {
            Step::None => {}
            Step::Add(tags) => {
                emulator
                    .ok(
                        "AddTagsToOnPremisesInstances",
                        json!({"instanceNames": [name], "tags": tag_list(tags)}),
                    )
                    .await;
            }
            Step::Remove(tags) => {
                emulator
                    .ok(
                        "RemoveTagsFromOnPremisesInstances",
                        json!({"instanceNames": [name], "tags": tag_list(tags)}),
                    )
                    .await;
            }
        }

        let info = instance_infos(&emulator, name, case).await;
        let reported = info["tags"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: expected tags to be reported as a list, got {info:?}")
        });
        let mut reported: Vec<(String, String)> = reported
            .iter()
            .map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or_default().to_string(),
                    tag["Value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        reported.sort();
        let mut wanted: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected tags {wanted:?}, got {reported:?} in {info:?}"
        );
    }
}

/// RULE: a deployment group's selector and trigger lists are reported as the
/// lists it holds, the empty list included, whichever of them the create request
/// named.
///
/// Rows differ along WHICH member the request names and the compute platform of
/// the application, which is what the rule generalises over: the member is
/// reported either way, carrying what was stored.
#[tokio::test]
async fn a_deployment_group_reports_every_selector_list_it_holds() {
    let emulator = Emulator::new();
    /// The members of `DeploymentGroupInfo` a group of any platform always holds
    /// a list for (`smithy_query(query="shape",shape="DeploymentGroupInfo")`).
    const ALWAYS_LISTED: &[&str] = &[
        "ec2TagFilters",
        "onPremisesInstanceTagFilters",
        "autoScalingGroups",
        "triggerConfigurations",
    ];

    let cases: &[(&str, &str, Value)] = &[
        ("nothing named, Server", "Server", json!({})),
        ("nothing named, Lambda", "Lambda", json!({})),
        ("nothing named, ECS", "ECS", json!({})),
        (
            "only the EC2 filters named",
            "Server",
            json!({"ec2TagFilters": [{"Key": "Stage", "Value": "prod", "Type": "KEY_AND_VALUE"}]}),
        ),
        (
            "only the on-premises filters named",
            "Server",
            json!({"onPremisesInstanceTagFilters": [{"Key": "Fleet", "Type": "KEY_ONLY"}]}),
        ),
        (
            "only the auto-scaling groups named",
            "Server",
            json!({"autoScalingGroups": ["asg-one", "asg-two"]}),
        ),
        (
            "every list named, one of them empty",
            "Server",
            json!({
                "ec2TagFilters": [{"Value": "prod", "Type": "VALUE_ONLY"}],
                "onPremisesInstanceTagFilters": [],
                "autoScalingGroups": ["asg-three"],
            }),
        ),
    ];

    for (index, (case, platform, extra)) in cases.iter().enumerate() {
        let name = format!("selector-app-{index:02}");
        emulator
            .ok(
                "CreateApplication",
                json!({"applicationName": name, "computePlatform": platform}),
            )
            .await;
        let group = format!("{name}-dg");
        let mut request = json!({
            "applicationName": name,
            "deploymentGroupName": group,
            "serviceRoleArn": ROLE,
        });
        for (member, value) in extra.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        emulator.ok("CreateDeploymentGroup", request).await;

        let info = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": name, "deploymentGroupName": group}),
            )
            .await["deploymentGroupInfo"]
            .clone();

        for member in ALWAYS_LISTED {
            assert!(
                info[member].is_array(),
                "case {case:?}: expected {member} to be reported as a list, got {info:?}"
            );
        }
        // What the request named is what the group reports; what it did not name
        // is the empty list.
        for member in ALWAYS_LISTED {
            let expected = extra
                .get(member)
                .cloned()
                .unwrap_or_else(|| json!([]));
            if *member == "autoScalingGroups" {
                // The group reports an auto-scaling group as a structure, so the
                // names are what a row states.
                let reported: Vec<String> = info[member]
                    .as_array()
                    .map(|groups| {
                        groups
                            .iter()
                            .filter_map(|asg| asg["name"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let wanted: Vec<String> = expected
                    .as_array()
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(|name| name.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    reported, wanted,
                    "case {case:?}: expected {member} {wanted:?}, got {reported:?} in {info:?}"
                );
            } else {
                assert_eq!(
                    info[member], expected,
                    "case {case:?}: expected {member} {expected:?}, got {info:?}"
                );
            }
        }
    }
}
