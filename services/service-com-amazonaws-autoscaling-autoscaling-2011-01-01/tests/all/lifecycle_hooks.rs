// SPDX-License-Identifier: CC0-1.0

//! The lifecycle hooks of an Auto Scaling group over the awsQuery wire:
//! `PutLifecycleHook`, `DescribeLifecycleHooks`, `DeleteLifecycleHook` and the hook
//! specifications a group can be created with.
//!
//! The rule under test: a lifecycle hook is a NAMED CHILD of one Auto Scaling group, so
//! `PutLifecycleHook` is an upsert over that group's own collection — a name the group
//! does not hold is CREATED with the documented defaults for every member the request
//! omits (`HeartbeatTimeout` 3600, `DefaultResult` ABANDON) and requires a
//! `LifecycleTransition`, while a name it holds is UPDATED per member: an omitted member
//! keeps the stored value, a supplied one overwrites it, and an empty
//! `NotificationTargetARN` clears the stored target rather than counting as omitted. The
//! group's reader reports exactly what is stored (with `GlobalTimeout` derived from the
//! hook's own heartbeat timeout), the delete removes exactly the hook it names, and
//! deleting the group takes its hooks with it.
//!
//! The tables below span what that rule generalises over — which members a request
//! carries, whether the call creates or updates, the value written and whether it
//! replaces an earlier one, each published transition and default result, both published
//! heartbeat bounds and one past each, how many hooks the group holds, which hooks a read
//! filters for, and which of the two writers (the upsert or the group create) stored the
//! hook. Every row reads the hooks back through `DescribeLifecycleHooks`, so no row can
//! pass on an accepted-but-unstored change.
//!
//! This suite OWNS the hook harness for this crate's tests — the writers, the reader and
//! the projection they are asserted through are defined here once, and
//! `describe_lifecycle_hooks_projection.rs`, which covers what the reader projects out of
//! a group's collection, drives the same helpers rather than keeping a second copy.

use smithy_server_runtime::MultiServiceRouter;

use crate::support::{
    borrowed, new_engine, p, query, router_with, tag_value, tag_values, xml_unescape,
};

/// The launch source every group here is created from — a launch TEMPLATE, because no
/// account can create a launch configuration (see
/// `launch_configurations_are_unavailable.rs`).
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// The zone every group here places into.
const ZONE: (&str, &str) = ("AvailabilityZones.member.1", "us-west-2a");

/// The transitions a lifecycle hook can be created for: the catalog
/// `DescribeLifecycleHookTypes` publishes, from the model's own documentation for
/// `PutLifecycleHookType$LifecycleTransition` ("To create a lifecycle hook for scale-out
/// events, specify autoscaling:EC2_INSTANCE_LAUNCHING. To create a lifecycle hook for
/// scale-in events, specify autoscaling:EC2_INSTANCE_TERMINATING").
pub(crate) const TRANSITIONS: [&str; 2] = [
    "autoscaling:EC2_INSTANCE_LAUNCHING",
    "autoscaling:EC2_INSTANCE_TERMINATING",
];

/// The results a hook may default to, from the model's own documentation for
/// `PutLifecycleHookType$DefaultResult` ("Valid values: CONTINUE | ABANDON").
pub(crate) const RESULTS: [&str; 2] = ["CONTINUE", "ABANDON"];

/// `DefaultResult` for a hook created without one, from the same documentation ("The
/// default value is ABANDON").
const DEFAULT_RESULT: &str = "ABANDON";

/// `HeartbeatTimeout` for a hook created without one, from the model's own documentation
/// for `PutLifecycleHookType$HeartbeatTimeout` ("The default value is 3600 seconds (1
/// hour)").
const DEFAULT_HEARTBEAT_TIMEOUT: i32 = 3600;

/// The published bounds of `HeartbeatTimeout`, from the same documentation ("The range is
/// from 30 to 7200 seconds").
pub(crate) const MIN_HEARTBEAT_TIMEOUT: i32 = 30;
pub(crate) const MAX_HEARTBEAT_TIMEOUT: i32 = 7200;

/// The ceiling on a hook's `GlobalTimeout`, from the model's own documentation for
/// `LifecycleHook$GlobalTimeout` ("The maximum is 172800 seconds (48 hours) or 100 times
/// HeartbeatTimeout, whichever is smaller").
const MAX_GLOBAL_TIMEOUT: i32 = 172_800;

/// The most hooks one group may hold, from the model's own documentation for
/// `PutLifecycleHook` ("your maximum limit of lifecycle hooks, which by default is 50 per
/// Auto Scaling group").
const MAX_HOOKS_PER_GROUP: usize = 50;

/// What `GlobalTimeout` a hook with this heartbeat timeout must report: the documented
/// derivation, written out here so the assertion does not take the handler's own word.
fn expected_global_timeout(heartbeat_timeout: i32) -> i32 {
    (heartbeat_timeout * 100).min(MAX_GLOBAL_TIMEOUT)
}

/// A catalog as a refusal states it: the entries comma-separated inside brackets, the shape
/// an admissible value set is stated in on the wire.
fn stated(catalog: &[&str]) -> String {
    format!("[{}]", catalog.join(", "))
}

/// One hook as `DescribeLifecycleHooks` reports it. Every member is kept as the wire
/// spelled it, `""` for one the answer omits, so an assertion can tell a cleared member
/// from a stored one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReportedHook {
    pub(crate) name: String,
    pub(crate) group: String,
    pub(crate) transition: String,
    pub(crate) notification_target_arn: String,
    pub(crate) role_arn: String,
    pub(crate) notification_metadata: String,
    pub(crate) heartbeat_timeout: String,
    pub(crate) global_timeout: String,
    pub(crate) default_result: String,
}

/// Create an empty group (nothing to launch, so nothing settles on the clock while the
/// hooks are written and read).
pub(crate) async fn create_group(router: &MultiServiceRouter, name: &str) {
    create_group_with(router, name, &[]).await;
}

/// Create an empty group carrying `extra` request members, and assert the create was
/// served.
async fn create_group_with(router: &MultiServiceRouter, name: &str, extra: &[(String, String)]) {
    let (status, body) = create_group_result(router, name, extra).await;
    assert_eq!(
        status, 200,
        "creating {name:?} must succeed, got {status} with {body:?}"
    );
}

/// Post one `CreateAutoScalingGroup` for an empty group carrying `extra`, and answer its
/// status and body.
async fn create_group_result(
    router: &MultiServiceRouter,
    name: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![
        p("AutoScalingGroupName", name),
        p(LAUNCH_SOURCE.0, LAUNCH_SOURCE.1),
        p("MinSize", "0"),
        p("MaxSize", "1"),
        p("DesiredCapacity", "0"),
        p(ZONE.0, ZONE.1),
    ];
    params.extend_from_slice(extra);
    query(router, "CreateAutoScalingGroup", &borrowed(&params)).await
}

/// Post one `PutLifecycleHook` naming `members` beyond the group and hook names, and
/// answer its status, wire error code and message.
pub(crate) async fn put_hook(
    router: &MultiServiceRouter,
    group: &str,
    hook: &str,
    members: &[(String, String)],
) -> (u16, String, String) {
    let mut params = vec![
        p("AutoScalingGroupName", group),
        p("LifecycleHookName", hook),
    ];
    params.extend_from_slice(members);
    let (status, body) = query(router, "PutLifecycleHook", &borrowed(&params)).await;
    (
        status,
        tag_value(&body, "Code"),
        xml_unescape(&tag_value(&body, "Message")),
    )
}

/// Post one `DeleteLifecycleHook`, and answer its status, wire error code and message.
async fn delete_hook(
    router: &MultiServiceRouter,
    group: &str,
    hook: &str,
) -> (u16, String, String) {
    let (status, body) = query(
        router,
        "DeleteLifecycleHook",
        &[("AutoScalingGroupName", group), ("LifecycleHookName", hook)],
    )
    .await;
    (
        status,
        tag_value(&body, "Code"),
        xml_unescape(&tag_value(&body, "Message")),
    )
}

/// Post one `DescribeLifecycleHooks`, filtering for `names` when any are given, and
/// answer its status, wire error code and message.
async fn describe_result(
    router: &MultiServiceRouter,
    group: &str,
    names: &[&str],
) -> (u16, String, String) {
    let mut params = vec![p("AutoScalingGroupName", group)];
    for (index, name) in names.iter().enumerate() {
        params.push(p(
            format!("LifecycleHookNames.member.{}", index + 1).as_str(),
            *name,
        ));
    }
    let (status, body) = query(router, "DescribeLifecycleHooks", &borrowed(&params)).await;
    (
        status,
        tag_value(&body, "Code"),
        xml_unescape(&tag_value(&body, "Message")),
    )
}

/// The hooks the group reports, in the order the answer carries them.
pub(crate) async fn described_hooks(
    router: &MultiServiceRouter,
    group: &str,
    names: &[&str],
) -> Vec<ReportedHook> {
    let mut params = vec![p("AutoScalingGroupName", group)];
    for (index, name) in names.iter().enumerate() {
        params.push(p(
            format!("LifecycleHookNames.member.{}", index + 1).as_str(),
            *name,
        ));
    }
    let (status, body) = query(router, "DescribeLifecycleHooks", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "the read-back of {group:?} must succeed, got {status} with {body:?}"
    );
    // The hook members live inside the answer's own `LifecycleHooks` element; the
    // envelope carries other elements, so the slice is narrowed to that one first.
    let Some((_, rest)) = body.split_once("<LifecycleHooks>") else {
        return Vec::new();
    };
    let inner = rest
        .split_once("</LifecycleHooks>")
        .map_or(rest, |(inner, _)| inner);
    tag_values(inner, "member")
        .iter()
        // Every value is decoded: a member can carry the entities the envelope escapes
        // its text with (the notification metadata is free text), so the assertion
        // compares what a client parses rather than its wire spelling.
        .map(|member| {
            let text = |tag: &str| xml_unescape(&tag_value(member, tag));
            ReportedHook {
                name: text("LifecycleHookName"),
                group: text("AutoScalingGroupName"),
                transition: text("LifecycleTransition"),
                notification_target_arn: text("NotificationTargetARN"),
                role_arn: text("RoleARN"),
                notification_metadata: text("NotificationMetadata"),
                heartbeat_timeout: text("HeartbeatTimeout"),
                global_timeout: text("GlobalTimeout"),
                default_result: text("DefaultResult"),
            }
        })
        .collect()
}

/// The one hook the group reports under `hook`.
pub(crate) async fn described_hook(
    router: &MultiServiceRouter,
    group: &str,
    hook: &str,
) -> ReportedHook {
    let reported = described_hooks(router, group, &[hook]).await;
    assert_eq!(
        reported.len(),
        1,
        "the group {group:?} must report exactly the hook {hook:?}, got {reported:?}"
    );
    reported.into_iter().next().expect("one reported hook")
}

/// One create row: the case, the members the request carries beyond the names, and what
/// the group must then report for the hook.
struct CreateRow {
    case: &'static str,
    members: Vec<(String, String)>,
    expected: ReportedHook,
}

/// What a created hook must look like, filled in per row.
pub(crate) fn expect(
    group: &str,
    hook: &str,
    transition: &str,
    heartbeat_timeout: i32,
    default_result: &str,
) -> ReportedHook {
    ReportedHook {
        name: hook.to_string(),
        group: group.to_string(),
        transition: transition.to_string(),
        notification_target_arn: String::new(),
        role_arn: String::new(),
        notification_metadata: String::new(),
        heartbeat_timeout: heartbeat_timeout.to_string(),
        global_timeout: expected_global_timeout(heartbeat_timeout).to_string(),
        default_result: default_result.to_string(),
    }
}

/// A notification target of the shape a hook names: an Amazon SNS topic ARN of the
/// account, which the model documents as one of the two kinds of target.
fn topic_arn(suffix: &str) -> String {
    format!("arn:aws:sns:us-west-2:000000000000:asg-hook-{suffix}")
}

/// A role of the shape a hook names.
fn role_arn(suffix: &str) -> String {
    format!("arn:aws:iam::000000000000:role/asg-hook-{suffix}")
}

/// Every create row: the hook is stored with exactly the members the request carried and
/// the documented default for each one it omitted, read back off the group itself.
#[tokio::test]
async fn a_put_creates_the_hook_with_the_documented_defaults() {
    let group = "asg-create";
    let mut rows = vec![
        CreateRow {
            case: "the transition alone, so every other member defaults",
            members: vec![p("LifecycleTransition", TRANSITIONS[0])],
            expected: expect(
                group,
                "hook-1",
                TRANSITIONS[0],
                DEFAULT_HEARTBEAT_TIMEOUT,
                DEFAULT_RESULT,
            ),
        },
        CreateRow {
            case: "the scale-in transition, which the other rows do not use",
            members: vec![p("LifecycleTransition", TRANSITIONS[1])],
            expected: expect(
                group,
                "hook-2",
                TRANSITIONS[1],
                DEFAULT_HEARTBEAT_TIMEOUT,
                DEFAULT_RESULT,
            ),
        },
        CreateRow {
            case: "a notification target and the role that publishes to it",
            members: vec![
                p("LifecycleTransition", TRANSITIONS[0]),
                p("NotificationTargetARN", topic_arn("a")),
                p("RoleARN", role_arn("a")),
            ],
            expected: ReportedHook {
                notification_target_arn: topic_arn("a"),
                role_arn: role_arn("a"),
                ..expect(
                    group,
                    "hook-3",
                    TRANSITIONS[0],
                    DEFAULT_HEARTBEAT_TIMEOUT,
                    DEFAULT_RESULT,
                )
            },
        },
        CreateRow {
            case: "notification metadata and a heartbeat timeout",
            members: vec![
                p("LifecycleTransition", TRANSITIONS[1]),
                p("NotificationMetadata", "{\"stage\":\"drain\"}"),
                p("HeartbeatTimeout", "120"),
            ],
            expected: ReportedHook {
                notification_metadata: "{\"stage\":\"drain\"}".to_string(),
                ..expect(group, "hook-4", TRANSITIONS[1], 120, DEFAULT_RESULT)
            },
        },
        CreateRow {
            case: "every member at once",
            members: vec![
                p("LifecycleTransition", TRANSITIONS[0]),
                p("NotificationTargetARN", topic_arn("b")),
                p("RoleARN", role_arn("b")),
                p("NotificationMetadata", "drain-then-continue"),
                p("HeartbeatTimeout", "600"),
                p("DefaultResult", RESULTS[0]),
            ],
            expected: ReportedHook {
                notification_target_arn: topic_arn("b"),
                role_arn: role_arn("b"),
                notification_metadata: "drain-then-continue".to_string(),
                ..expect(group, "hook-5", TRANSITIONS[0], 600, RESULTS[0])
            },
        },
    ];
    // Each published `DefaultResult`, so the row that writes one is not the only
    // evidence that the member is stored rather than defaulted.
    for (index, result) in RESULTS.iter().enumerate() {
        rows.push(CreateRow {
            case: "an explicit default result",
            members: vec![
                p("LifecycleTransition", TRANSITIONS[index % TRANSITIONS.len()]),
                p("DefaultResult", *result),
            ],
            expected: expect(
                group,
                &format!("hook-result-{result}"),
                TRANSITIONS[index % TRANSITIONS.len()],
                DEFAULT_HEARTBEAT_TIMEOUT,
                result,
            ),
        });
    }
    // Each published heartbeat bound, and the derived `GlobalTimeout` that follows from
    // it: the smaller bound derives a timeout below the ceiling, the larger one is capped
    // by it.
    for timeout in [MIN_HEARTBEAT_TIMEOUT, MAX_HEARTBEAT_TIMEOUT] {
        rows.push(CreateRow {
            case: "a published heartbeat bound",
            members: vec![
                p("LifecycleTransition", TRANSITIONS[0]),
                p("HeartbeatTimeout", timeout.to_string()),
            ],
            expected: expect(
                group,
                &format!("hook-heartbeat-{timeout}"),
                TRANSITIONS[0],
                timeout,
                DEFAULT_RESULT,
            ),
        });
    }

    let router = router_with(new_engine());
    create_group(&router, group).await;

    for row in &rows {
        let hook = row.expected.name.as_str();
        let (status, code, message) = put_hook(&router, group, hook, &row.members).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 for {hook:?}, got {status} with {code:?} {message:?}",
            row.case
        );
        let actual = described_hook(&router, group, hook).await;
        assert_eq!(
            actual, row.expected,
            "case {:?}: expected {:?}, got {actual:?}",
            row.case, row.expected
        );
    }

    // Every hook of the table is still there, and an unfiltered read reports all of
    // them: the collection is a collection, not one slot.
    let all = described_hooks(&router, group, &[]).await;
    assert_eq!(
        all.len(),
        rows.len(),
        "the group must report every hook the table created, got {all:?}"
    );
}

/// One update row: the case, the members the second call carries, and what the hook must
/// report afterwards given what the first call stored.
struct UpdateRow {
    case: &'static str,
    members: Vec<(String, String)>,
    expect_after: fn(ReportedHook) -> ReportedHook,
}

/// Every update row: the members the second call carries overwrite, the members it omits
/// keep what the first call stored, and an empty `NotificationTargetARN` clears the
/// stored target.
#[tokio::test]
async fn a_put_on_an_existing_hook_updates_only_the_members_it_carries() {
    let rows = [
        UpdateRow {
            case: "the heartbeat timeout alone, leaving the target and the role",
            members: vec![p("HeartbeatTimeout", "120")],
            expect_after: |before| ReportedHook {
                heartbeat_timeout: "120".to_string(),
                global_timeout: expected_global_timeout(120).to_string(),
                ..before
            },
        },
        UpdateRow {
            case: "an empty notification target, which overrides the stored one",
            members: vec![p("NotificationTargetARN", "")],
            expect_after: |before| ReportedHook {
                notification_target_arn: String::new(),
                ..before
            },
        },
        UpdateRow {
            case: "a different notification target",
            members: vec![p("NotificationTargetARN", topic_arn("moved"))],
            expect_after: |before| ReportedHook {
                notification_target_arn: topic_arn("moved"),
                ..before
            },
        },
        UpdateRow {
            case: "the other transition, which an update may change",
            members: vec![p("LifecycleTransition", TRANSITIONS[1])],
            expect_after: |before| ReportedHook {
                transition: TRANSITIONS[1].to_string(),
                ..before
            },
        },
        UpdateRow {
            case: "a new default result",
            members: vec![p("DefaultResult", RESULTS[0])],
            expect_after: |before| ReportedHook {
                default_result: RESULTS[0].to_string(),
                ..before
            },
        },
        UpdateRow {
            case: "new metadata and a new role at once",
            members: vec![
                p("NotificationMetadata", "second"),
                p("RoleARN", role_arn("second")),
            ],
            expect_after: |before| ReportedHook {
                notification_metadata: "second".to_string(),
                role_arn: role_arn("second"),
                ..before
            },
        },
        UpdateRow {
            case: "nothing beyond the names, which changes nothing",
            members: vec![],
            expect_after: |before| before,
        },
        UpdateRow {
            case: "the same values again, which changes nothing either",
            members: vec![
                p("LifecycleTransition", TRANSITIONS[0]),
                p("NotificationTargetARN", topic_arn("first")),
                p("RoleARN", role_arn("first")),
                p("NotificationMetadata", "first"),
                p("HeartbeatTimeout", "300"),
                p("DefaultResult", RESULTS[1]),
            ],
            expect_after: |before| before,
        },
    ];

    for (row_index, row) in rows.iter().enumerate() {
        // One emulator per row, so each row's update acts on the same starting hook.
        let router = router_with(new_engine());
        let group = format!("asg-update-{row_index}");
        create_group(&router, &group).await;

        let created = vec![
            p("LifecycleTransition", TRANSITIONS[0]),
            p("NotificationTargetARN", topic_arn("first")),
            p("RoleARN", role_arn("first")),
            p("NotificationMetadata", "first"),
            p("HeartbeatTimeout", "300"),
            p("DefaultResult", RESULTS[1]),
        ];
        let (status, code, message) = put_hook(&router, &group, "hook", &created).await;
        assert_eq!(
            status, 200,
            "case {:?}: the create must succeed, got {status} with {code:?} {message:?}",
            row.case
        );
        // A second hook of the same group, left untouched by every row: an update of one
        // hook is an update of ONE hook.
        let (status, code, message) = put_hook(
            &router,
            &group,
            "other-hook",
            &[p("LifecycleTransition", TRANSITIONS[1])],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the second hook must be created, got {status} with {code:?} {message:?}",
            row.case
        );

        let before = described_hook(&router, &group, "hook").await;
        let (status, code, message) = put_hook(&router, &group, "hook", &row.members).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {code:?} {message:?}",
            row.case
        );

        let expected = (row.expect_after)(before);
        let actual = described_hook(&router, &group, "hook").await;
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected:?}, got {actual:?}",
            row.case
        );

        let untouched = described_hook(&router, &group, "other-hook").await;
        assert_eq!(
            untouched,
            expect(
                &group,
                "other-hook",
                TRANSITIONS[1],
                DEFAULT_HEARTBEAT_TIMEOUT,
                DEFAULT_RESULT,
            ),
            "case {:?}: the other hook of the group must be untouched, got {untouched:?}",
            row.case
        );
    }
}

/// A create needs a transition and an update does not: the model documents
/// `LifecycleTransition` as "Required for new lifecycle hooks, but optional when updating
/// existing hooks", and nothing is stored for the refused create.
#[tokio::test]
async fn a_transition_is_required_only_to_create_a_hook() {
    let router = router_with(new_engine());
    create_group(&router, "asg-transition").await;

    let (status, code, message) = put_hook(&router, "asg-transition", "hook", &[]).await;
    assert_eq!(
        status, 400,
        "a create without a transition must be refused, got {status} with {code:?} {message:?}"
    );
    assert_eq!(
        code, "ValidationError",
        "expected the ValidationError code, got {code:?} {message:?}"
    );
    let expected = "The LifecycleTransition is required to create the lifecycle hook hook, and \
                    may be omitted only when updating one that already exists";
    assert_eq!(
        message, expected,
        "expected the message {expected:?}, got {message:?}"
    );
    let hooks = described_hooks(&router, "asg-transition", &[]).await;
    assert!(
        hooks.is_empty(),
        "the refused create stored nothing, got {hooks:?}"
    );

    let (status, code, message) = put_hook(
        &router,
        "asg-transition",
        "hook",
        &[p("LifecycleTransition", TRANSITIONS[0])],
    )
    .await;
    assert_eq!(
        status, 200,
        "the create naming a transition must succeed, got {status} with {code:?} {message:?}"
    );
    let (status, code, message) =
        put_hook(&router, "asg-transition", "hook", &[p("HeartbeatTimeout", "60")]).await;
    assert_eq!(
        status, 200,
        "the update may omit the transition, got {status} with {code:?} {message:?}"
    );
    let actual = described_hook(&router, "asg-transition", "hook").await;
    assert_eq!(
        actual,
        expect(
            "asg-transition",
            "hook",
            TRANSITIONS[0],
            60,
            DEFAULT_RESULT
        ),
        "the update keeps the transition the create stored, got {actual:?}"
    );
}

/// One refused row: the case, the members the request carries, and the whole message it
/// must be refused with.
struct RefusedRow {
    case: &'static str,
    members: Vec<(String, String)>,
    message: String,
}

/// The value rules: a transition outside the published catalog, a default result outside
/// the published pair and a heartbeat timeout one past either published bound are each
/// refused `ValidationError`, and nothing is written — the hook the group already holds
/// is unchanged.
#[tokio::test]
async fn a_put_carrying_a_value_outside_its_rule_is_refused_and_writes_nothing() {
    let constraint_sentence = |value: i32, constraint: &str| {
        format!(
            "1 validation error detected: Value '{value}' at 'heartbeatTimeout' failed to \
             satisfy constraint: {constraint}"
        )
    };
    let rows = [
        RefusedRow {
            case: "a transition of neither published kind",
            members: vec![p("LifecycleTransition", "autoscaling:EC2_INSTANCE_REBOOTING")],
            message: format!(
                "The LifecycleTransition value autoscaling:EC2_INSTANCE_REBOOTING is not valid: \
                 it must be one of {}",
                stated(&TRANSITIONS)
            ),
        },
        RefusedRow {
            case: "a transition spelled without its namespace",
            members: vec![p("LifecycleTransition", "EC2_INSTANCE_LAUNCHING")],
            message: format!(
                "The LifecycleTransition value EC2_INSTANCE_LAUNCHING is not valid: it must be \
                 one of {}",
                stated(&TRANSITIONS)
            ),
        },
        RefusedRow {
            case: "a default result of neither published value",
            members: vec![
                p("LifecycleTransition", TRANSITIONS[0]),
                p("DefaultResult", "PROCEED"),
            ],
            message: format!(
                "The DefaultResult value PROCEED is not valid: it must be one of {}",
                stated(&RESULTS)
            ),
        },
        RefusedRow {
            case: "a default result in the wrong case",
            members: vec![
                p("LifecycleTransition", TRANSITIONS[0]),
                p("DefaultResult", "continue"),
            ],
            message: format!(
                "The DefaultResult value continue is not valid: it must be one of {}",
                stated(&RESULTS)
            ),
        },
        RefusedRow {
            case: "a heartbeat timeout one second below the published minimum",
            members: vec![
                p("LifecycleTransition", TRANSITIONS[0]),
                p("HeartbeatTimeout", (MIN_HEARTBEAT_TIMEOUT - 1).to_string()),
            ],
            message: constraint_sentence(
                MIN_HEARTBEAT_TIMEOUT - 1,
                &format!(
                    "Member must have value greater than or equal to {MIN_HEARTBEAT_TIMEOUT}"
                ),
            ),
        },
        RefusedRow {
            case: "a heartbeat timeout one second above the published maximum",
            members: vec![
                p("LifecycleTransition", TRANSITIONS[0]),
                p("HeartbeatTimeout", (MAX_HEARTBEAT_TIMEOUT + 1).to_string()),
            ],
            message: constraint_sentence(
                MAX_HEARTBEAT_TIMEOUT + 1,
                &format!("Member must have value less than or equal to {MAX_HEARTBEAT_TIMEOUT}"),
            ),
        },
        RefusedRow {
            case: "a heartbeat timeout of zero",
            members: vec![
                p("LifecycleTransition", TRANSITIONS[0]),
                p("HeartbeatTimeout", "0"),
            ],
            message: constraint_sentence(
                0,
                &format!(
                    "Member must have value greater than or equal to {MIN_HEARTBEAT_TIMEOUT}"
                ),
            ),
        },
    ];

    let router = router_with(new_engine());
    create_group(&router, "asg-values").await;
    let stored = vec![
        p("LifecycleTransition", TRANSITIONS[0]),
        p("HeartbeatTimeout", "300"),
    ];
    let (status, code, message) = put_hook(&router, "asg-values", "hook", &stored).await;
    assert_eq!(
        status, 200,
        "the hook the rows must leave alone has to be created, got {status} with {code:?} \
         {message:?}"
    );
    let before = described_hook(&router, "asg-values", "hook").await;

    for row in &rows {
        // The same name as the stored hook, so a row that wrote anything would be caught
        // by the read-back below — a refused UPDATE writes nothing either.
        let (status, code, message) = put_hook(&router, "asg-values", "hook", &row.members).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {code:?} {message:?}",
            row.case
        );
        assert_eq!(
            code, "ValidationError",
            "case {:?}: expected the ValidationError code, got {code:?} {message:?}",
            row.case
        );
        assert_eq!(
            message, row.message,
            "case {:?}: expected the message {:?}, got {message:?}",
            row.case, row.message
        );
        let actual = described_hook(&router, "asg-values", "hook").await;
        assert_eq!(
            actual, before,
            "case {:?}: a refused call leaves the stored hook as it was, got {actual:?}",
            row.case
        );
    }
}

/// The documented per-group ceiling: the group holds the maximum, and one hook more is
/// refused with the operation's modelled `LimitExceededFault` while an UPDATE of a hook
/// it already holds still succeeds at the ceiling.
#[tokio::test]
async fn a_put_past_the_documented_hook_limit_is_refused() {
    let router = router_with(new_engine());
    create_group(&router, "asg-limit").await;

    for index in 0..MAX_HOOKS_PER_GROUP {
        let hook = format!("hook-{index}");
        let (status, code, message) = put_hook(
            &router,
            "asg-limit",
            &hook,
            &[p("LifecycleTransition", TRANSITIONS[index % TRANSITIONS.len()])],
        )
        .await;
        assert_eq!(
            status, 200,
            "the hook {hook:?} is inside the limit and must be stored, got {status} with \
             {code:?} {message:?}"
        );
    }
    let held = described_hooks(&router, "asg-limit", &[]).await;
    assert_eq!(
        held.len(),
        MAX_HOOKS_PER_GROUP,
        "the group must hold the documented maximum, got {} hooks",
        held.len()
    );

    let (status, code, message) = put_hook(
        &router,
        "asg-limit",
        "hook-past-the-limit",
        &[p("LifecycleTransition", TRANSITIONS[0])],
    )
    .await;
    assert_eq!(
        status, 400,
        "one hook past the limit must be refused, got {status} with {code:?} {message:?}"
    );
    // `LimitExceededFault` carries the awsQuery error code `LimitExceeded`
    // (smithy_query(query="operation",operation="PutLifecycleHook")).
    assert_eq!(
        code, "LimitExceeded",
        "expected the LimitExceeded code past the limit, got {code:?} {message:?}"
    );
    let expected = format!(
        "The lifecycle hook hook-past-the-limit exceeds the limit of {MAX_HOOKS_PER_GROUP} \
         lifecycle hooks per Auto Scaling group"
    );
    assert_eq!(
        message, expected,
        "expected the message {expected:?}, got {message:?}"
    );
    assert_eq!(
        described_hooks(&router, "asg-limit", &[]).await.len(),
        MAX_HOOKS_PER_GROUP,
        "the refused create stored nothing"
    );

    // An UPDATE at the ceiling creates no hook, so the limit does not apply to it.
    let (status, code, message) =
        put_hook(&router, "asg-limit", "hook-0", &[p("HeartbeatTimeout", "90")]).await;
    assert_eq!(
        status, 200,
        "an update at the ceiling must succeed, got {status} with {code:?} {message:?}"
    );
    assert_eq!(
        described_hook(&router, "asg-limit", "hook-0").await.heartbeat_timeout,
        "90",
        "the update at the ceiling must be stored"
    );
}

/// What a read filters for: no names is every hook of the group, a subset is exactly
/// those hooks, and a name the group does not hold contributes nothing rather than
/// refusing the read.
#[tokio::test]
async fn a_read_reports_the_hooks_it_filters_for() {
    let router = router_with(new_engine());
    create_group(&router, "asg-read").await;
    create_group(&router, "asg-read-other").await;

    for hook in ["hook-a", "hook-b", "hook-c"] {
        let (status, code, message) = put_hook(
            &router,
            "asg-read",
            hook,
            &[p("LifecycleTransition", TRANSITIONS[0])],
        )
        .await;
        assert_eq!(
            status, 200,
            "the hook {hook:?} must be created, got {status} with {code:?} {message:?}"
        );
    }
    // A hook of ANOTHER group, to show the read is scoped to the group it names.
    let (status, code, message) = put_hook(
        &router,
        "asg-read-other",
        "hook-elsewhere",
        &[p("LifecycleTransition", TRANSITIONS[1])],
    )
    .await;
    assert_eq!(
        status, 200,
        "the other group's hook must be created, got {status} with {code:?} {message:?}"
    );

    let cases: [(&str, &[&str], Vec<&str>); 6] = [
        ("no names, so every hook", &[], vec!["hook-a", "hook-b", "hook-c"]),
        ("one name", &["hook-b"], vec!["hook-b"]),
        ("two names", &["hook-a", "hook-c"], vec!["hook-a", "hook-c"]),
        (
            "the names in the reverse of the order they were created",
            &["hook-c", "hook-a"],
            vec!["hook-a", "hook-c"],
        ),
        ("a name the group does not hold", &["hook-z"], vec![]),
        (
            "a name of ANOTHER group's hook",
            &["hook-elsewhere"],
            vec![],
        ),
    ];
    for (case, names, expected) in cases {
        let reported = described_hooks(&router, "asg-read", names).await;
        let actual: Vec<String> = reported.iter().map(|hook| hook.name.clone()).collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the hooks {expected:?}, got {reported:?}"
        );
        for hook in &reported {
            assert_eq!(
                hook.group, "asg-read",
                "case {case:?}: every reported hook belongs to the group read, got {hook:?}"
            );
        }
    }
}

/// The delete removes exactly the hook it names, is a success for a name the group does
/// not hold, and frees the name for a later create.
#[tokio::test]
async fn a_delete_removes_exactly_the_hook_it_names() {
    let router = router_with(new_engine());
    create_group(&router, "asg-delete").await;

    for hook in ["hook-a", "hook-b"] {
        let (status, code, message) = put_hook(
            &router,
            "asg-delete",
            hook,
            &[
                p("LifecycleTransition", TRANSITIONS[0]),
                p("HeartbeatTimeout", "300"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "the hook {hook:?} must be created, got {status} with {code:?} {message:?}"
        );
    }

    let (status, code, message) = delete_hook(&router, "asg-delete", "hook-a").await;
    assert_eq!(
        status, 200,
        "the delete must succeed, got {status} with {code:?} {message:?}"
    );
    let remaining: Vec<String> = described_hooks(&router, "asg-delete", &[])
        .await
        .iter()
        .map(|hook| hook.name.clone())
        .collect();
    assert_eq!(
        remaining,
        vec!["hook-b".to_string()],
        "only the hook the delete named is gone, got {remaining:?}"
    );

    // Deleting it again, and deleting a name the group never held, are successes that
    // remove nothing.
    for hook in ["hook-a", "hook-never-stored"] {
        let (status, code, message) = delete_hook(&router, "asg-delete", hook).await;
        assert_eq!(
            status, 200,
            "case {hook:?}: deleting a hook the group does not hold is a success, got {status} \
             with {code:?} {message:?}"
        );
    }
    assert_eq!(
        described_hooks(&router, "asg-delete", &[]).await.len(),
        1,
        "those deletes removed nothing"
    );

    // The name is free again: a put under it CREATES a hook, with the defaults rather
    // than anything the deleted hook carried.
    let (status, code, message) = put_hook(
        &router,
        "asg-delete",
        "hook-a",
        &[p("LifecycleTransition", TRANSITIONS[1])],
    )
    .await;
    assert_eq!(
        status, 200,
        "the name must be free to create again, got {status} with {code:?} {message:?}"
    );
    let actual = described_hook(&router, "asg-delete", "hook-a").await;
    assert_eq!(
        actual,
        expect(
            "asg-delete",
            "hook-a",
            TRANSITIONS[1],
            DEFAULT_HEARTBEAT_TIMEOUT,
            DEFAULT_RESULT,
        ),
        "the recreated hook carries the documented defaults, got {actual:?}"
    );
}

/// A hook belongs to its group: deleting the group takes its hooks with it, so a group
/// created again under the same name holds none.
#[tokio::test]
async fn a_deleted_group_takes_its_hooks_with_it() {
    let router = router_with(new_engine());
    create_group(&router, "asg-owned").await;
    let (status, code, message) = put_hook(
        &router,
        "asg-owned",
        "hook",
        &[p("LifecycleTransition", TRANSITIONS[0])],
    )
    .await;
    assert_eq!(
        status, 200,
        "the hook must be created, got {status} with {code:?} {message:?}"
    );

    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[("AutoScalingGroupName", "asg-owned")],
    )
    .await;
    assert_eq!(status, 200, "the group delete must succeed, got {status} with {body:?}");

    create_group(&router, "asg-owned").await;
    let hooks = described_hooks(&router, "asg-owned", &[]).await;
    assert!(
        hooks.is_empty(),
        "a group created again under the same name holds no hook of the deleted one, got \
         {hooks:?}"
    );
}

/// The other writer of the same collection: a group created WITH lifecycle hook
/// specifications holds exactly those hooks, under the same defaults, and a specification
/// the service cannot accept refuses the create whole — no group and no hook.
#[tokio::test]
async fn a_group_created_with_hook_specifications_reports_them() {
    let router = router_with(new_engine());
    let specifications = vec![
        p("LifecycleHookSpecificationList.member.1.LifecycleHookName", "spec-a"),
        p(
            "LifecycleHookSpecificationList.member.1.LifecycleTransition",
            TRANSITIONS[0],
        ),
        p(
            "LifecycleHookSpecificationList.member.1.HeartbeatTimeout",
            "120",
        ),
        p("LifecycleHookSpecificationList.member.2.LifecycleHookName", "spec-b"),
        p(
            "LifecycleHookSpecificationList.member.2.LifecycleTransition",
            TRANSITIONS[1],
        ),
        p(
            "LifecycleHookSpecificationList.member.2.NotificationTargetARN",
            topic_arn("spec"),
        ),
        p(
            "LifecycleHookSpecificationList.member.2.RoleARN",
            role_arn("spec"),
        ),
        p(
            "LifecycleHookSpecificationList.member.2.DefaultResult",
            RESULTS[0],
        ),
    ];
    create_group_with(&router, "asg-spec", &specifications).await;

    let actual = described_hooks(&router, "asg-spec", &[]).await;
    let expected = vec![
        expect("asg-spec", "spec-a", TRANSITIONS[0], 120, DEFAULT_RESULT),
        ReportedHook {
            notification_target_arn: topic_arn("spec"),
            role_arn: role_arn("spec"),
            ..expect(
                "asg-spec",
                "spec-b",
                TRANSITIONS[1],
                DEFAULT_HEARTBEAT_TIMEOUT,
                RESULTS[0],
            )
        },
    ];
    assert_eq!(
        actual, expected,
        "expected the specified hooks {expected:?}, got {actual:?}"
    );

    // A specification carrying a value outside its rule refuses the create: no group, so
    // no hook either.
    let refused = vec![
        p("LifecycleHookSpecificationList.member.1.LifecycleHookName", "spec-bad"),
        p(
            "LifecycleHookSpecificationList.member.1.LifecycleTransition",
            "autoscaling:EC2_INSTANCE_REBOOTING",
        ),
    ];
    let (status, body) = create_group_result(&router, "asg-spec-bad", &refused).await;
    assert_eq!(
        status, 400,
        "a create carrying an unknown transition must be refused, got {status} with {body:?}"
    );
    assert_eq!(
        tag_value(&body, "Code"),
        "ValidationError",
        "expected the ValidationError code, got {body:?}"
    );
    let expected = format!(
        "The LifecycleTransition value autoscaling:EC2_INSTANCE_REBOOTING is not valid: it must \
         be one of {}",
        stated(&TRANSITIONS)
    );
    assert_eq!(
        xml_unescape(&tag_value(&body, "Message")),
        expected,
        "expected the message {expected:?}, got {body:?}"
    );
    let (status, _, message) = describe_result(&router, "asg-spec-bad", &[]).await;
    assert_eq!(
        status, 400,
        "the refused create stored no group, so the read of it is refused too, got {status} \
         with {message:?}"
    );
}

/// A hook-scoped call naming a group the account does not hold: the WRITES answer the
/// fixed sentence whose detail field is unfilled, like every other group-scoped write of
/// this service, and the READ names the group it could not find.
#[tokio::test]
async fn a_call_against_an_absent_group_is_refused() {
    let router = router_with(new_engine());
    create_group(&router, "asg-present").await;

    for absent in ["asg-absent", "ASG-Present", "asg-present-but-longer"] {
        let (status, code, message) = put_hook(
            &router,
            absent,
            "hook",
            &[p("LifecycleTransition", TRANSITIONS[0])],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {absent:?}: the put must be refused, got {status} with {code:?} {message:?}"
        );
        assert_eq!(
            code, "ValidationError",
            "case {absent:?}: expected the ValidationError code, got {code:?} {message:?}"
        );
        assert_eq!(
            message, "AutoScalingGroup name not found - null",
            "case {absent:?}: expected the no-detail absent-group sentence, got {message:?}"
        );

        let (status, code, message) = delete_hook(&router, absent, "hook").await;
        assert_eq!(
            status, 400,
            "case {absent:?}: the delete must be refused, got {status} with {code:?} {message:?}"
        );
        assert_eq!(
            message, "AutoScalingGroup name not found - null",
            "case {absent:?}: expected the no-detail absent-group sentence, got {message:?}"
        );

        let (status, code, message) = describe_result(&router, absent, &[]).await;
        assert_eq!(
            status, 400,
            "case {absent:?}: the read must be refused, got {status} with {code:?} {message:?}"
        );
        assert_eq!(
            code, "ValidationError",
            "case {absent:?}: expected the ValidationError code, got {code:?} {message:?}"
        );
        let expected = format!("AutoScalingGroup name not found - AutoScalingGroup {absent} not found");
        assert_eq!(
            message, expected,
            "case {absent:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    let hooks = described_hooks(&router, "asg-present", &[]).await;
    assert!(
        hooks.is_empty(),
        "the refusals above wrote nothing on the group that does exist, got {hooks:?}"
    );
}
