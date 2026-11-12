// SPDX-License-Identifier: CC0-1.0

//! The SCHEDULE rule of a target group's health check, over the wire, on both operations
//! that write one.
//!
//! `HealthCheckIntervalSeconds` is "the approximate amount of time, in seconds, between
//! health checks of an individual target" and `HealthCheckTimeoutSeconds` is "the amount
//! of time, in seconds, during which no response from a target means a failed health
//! check" (`smithy_query(query="operation",operation="CreateTargetGroup,ModifyTargetGroup")`
//! gives each member its own range — 5..300 and 2..120 — and says nothing about the
//! pair). A check whose timeout OUTLASTS the interval is still waiting for an answer
//! after the next check was due, so the rule these tables pin is the pair: the EFFECTIVE
//! timeout — what the request carries, else what the call leaves in place (the documented
//! default on a create, the stored value on a modify) — may be as long as the effective
//! interval and no longer, and a call that would leave a target group outside the rule is
//! refused with HTTP 400 `ValidationError` and writes nothing.
//!
//! The rows differ along what the rule generalises over: which of the two members the
//! request names (both, only one, neither), where the pair sits (far apart, one second
//! apart, equal, inverted by one second, inverted by a wide margin), which published
//! bound each value is at, and which target group the check belongs to (HTTP, TCP and
//! Lambda, whose documented defaults differ), on `CreateTargetGroup` and on
//! `ModifyTargetGroup` alike.
//!
//! The wire harness (`emulator`, `call`, `action`, `p`) is `provisioning_loop`'s and the
//! target-group helpers are `target_group_health_check_port`'s; nothing here restates
//! either.

use crate::provisioning_loop::{action, call, emulator, p, Reply};
use crate::target_group_attributes_and_modification::unescaped;
use crate::target_group_health_check_port::{create_group, describe, health_check_of};
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::HEALTH_CHECK_SCHEDULE_MESSAGE;
use smithy_server_runtime::MultiServiceRouter;

/// The target groups the tables below start from, each with the health-check schedule its
/// own documented defaults give it: an HTTP group (interval 30, timeout 5), a TCP group
/// (interval 30, timeout 10) and a Lambda group (interval 35, timeout 30).
const START_GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "http",
        &[("Protocol", "HTTP"), ("Port", "80"), ("VpcId", "vpc-1")],
    ),
    (
        "tcp",
        &[("Protocol", "TCP"), ("Port", "1234"), ("VpcId", "vpc-1")],
    ),
    ("lambda", &[("TargetType", "lambda")]),
];

/// One row: the case name, the starting group, the schedule members the call names, and
/// whether the service accepts it.
type ScheduleCase = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
    bool,
);

/// Every row the rule is tabled over on the CREATE path, where an unnamed member takes
/// the default its own documentation gives it.
const CREATE_CASES: &[ScheduleCase] = &[
    // Neither member named: the documented defaults of every group shape are a schedule.
    ("an HTTP group taking both defaults", "http", &[], true),
    ("a TCP group taking both defaults", "tcp", &[], true),
    ("a Lambda group taking both defaults", "lambda", &[], true),
    // Both members named.
    (
        "both members one second apart",
        "http",
        &[
            ("HealthCheckIntervalSeconds", "6"),
            ("HealthCheckTimeoutSeconds", "5"),
        ],
        true,
    ),
    (
        "both members at their low bounds",
        "http",
        &[
            ("HealthCheckIntervalSeconds", "5"),
            ("HealthCheckTimeoutSeconds", "2"),
        ],
        true,
    ),
    (
        "both members at their high bounds",
        "http",
        &[
            ("HealthCheckIntervalSeconds", "300"),
            ("HealthCheckTimeoutSeconds", "120"),
        ],
        true,
    ),
    (
        "both members equal at the interval's low bound",
        "http",
        &[
            ("HealthCheckIntervalSeconds", "5"),
            ("HealthCheckTimeoutSeconds", "5"),
        ],
        true,
    ),
    (
        "both members equal at the timeout's high bound",
        "tcp",
        &[
            ("HealthCheckIntervalSeconds", "120"),
            ("HealthCheckTimeoutSeconds", "120"),
        ],
        true,
    ),
    (
        "the timeout one second past the interval",
        "http",
        &[
            ("HealthCheckIntervalSeconds", "10"),
            ("HealthCheckTimeoutSeconds", "11"),
        ],
        false,
    ),
    (
        "the timeout far past the interval",
        "lambda",
        &[
            ("HealthCheckIntervalSeconds", "5"),
            ("HealthCheckTimeoutSeconds", "120"),
        ],
        false,
    ),
    // Only one member named, so the other is the default the group's documentation gives
    // it — which is what the effective reading of the rule is about.
    (
        "only the interval, one second above the HTTP default timeout of 5",
        "http",
        &[("HealthCheckIntervalSeconds", "6")],
        true,
    ),
    (
        "only the interval, down to the HTTP default timeout of 5",
        "http",
        &[("HealthCheckIntervalSeconds", "5")],
        true,
    ),
    (
        "only the interval, down to the TCP default timeout of 10",
        "tcp",
        &[("HealthCheckIntervalSeconds", "10")],
        true,
    ),
    (
        "only the interval, under the TCP default timeout of 10",
        "tcp",
        &[("HealthCheckIntervalSeconds", "5")],
        false,
    ),
    (
        "only the interval, down to the Lambda default timeout of 30",
        "lambda",
        &[("HealthCheckIntervalSeconds", "30")],
        true,
    ),
    (
        "only the interval, one second under the Lambda default timeout of 30",
        "lambda",
        &[("HealthCheckIntervalSeconds", "29")],
        false,
    ),
    (
        "only the timeout, one second under the default interval of 30",
        "http",
        &[("HealthCheckTimeoutSeconds", "29")],
        true,
    ),
    (
        "only the timeout, up to the default interval of 30",
        "http",
        &[("HealthCheckTimeoutSeconds", "30")],
        true,
    ),
    (
        "only the timeout, one second past the default interval of 30",
        "http",
        &[("HealthCheckTimeoutSeconds", "31")],
        false,
    ),
    (
        "only the timeout, past the Lambda default interval of 35",
        "lambda",
        &[("HealthCheckTimeoutSeconds", "36")],
        false,
    ),
];

/// The schedule the starting group of each MODIFY row is created with, so every row is
/// judged against a KNOWN stored pair rather than against a default.
const STORED_INTERVAL: &str = "30";
const STORED_TIMEOUT: &str = "10";

/// Every row the rule is tabled over on the MODIFY path, against the stored pair above.
/// A member the request does not name keeps the value the group already has, which is
/// what each single-member row is about.
const MODIFY_CASES: &[ScheduleCase] = &[
    (
        "only the interval, above the stored timeout",
        "http",
        &[("HealthCheckIntervalSeconds", "11")],
        true,
    ),
    (
        "only the interval, at the stored timeout",
        "http",
        &[("HealthCheckIntervalSeconds", "10")],
        true,
    ),
    (
        "only the interval, one second under the stored timeout",
        "http",
        &[("HealthCheckIntervalSeconds", "9")],
        false,
    ),
    (
        "only the interval, far under the stored timeout",
        "http",
        &[("HealthCheckIntervalSeconds", "5")],
        false,
    ),
    (
        "only the interval, at its high bound",
        "tcp",
        &[("HealthCheckIntervalSeconds", "300")],
        true,
    ),
    (
        "only the timeout, under the stored interval",
        "http",
        &[("HealthCheckTimeoutSeconds", "29")],
        true,
    ),
    (
        "only the timeout, at the stored interval",
        "http",
        &[("HealthCheckTimeoutSeconds", "30")],
        true,
    ),
    (
        "only the timeout, one second past the stored interval",
        "http",
        &[("HealthCheckTimeoutSeconds", "31")],
        false,
    ),
    (
        "only the timeout, far past the stored interval",
        "lambda",
        &[("HealthCheckTimeoutSeconds", "120")],
        false,
    ),
    (
        "only the timeout, at its low bound",
        "lambda",
        &[("HealthCheckTimeoutSeconds", "2")],
        true,
    ),
    (
        // Both members at once: the pair the call LEAVES BEHIND is what the rule reads,
        // so an interval the stored timeout would refuse is served when the same call
        // lowers the timeout with it.
        "both members, the interval under the stored timeout",
        "http",
        &[
            ("HealthCheckIntervalSeconds", "5"),
            ("HealthCheckTimeoutSeconds", "2"),
        ],
        true,
    ),
    (
        "both members, inverted",
        "tcp",
        &[
            ("HealthCheckIntervalSeconds", "20"),
            ("HealthCheckTimeoutSeconds", "21"),
        ],
        false,
    ),
    (
        "both members at their high bounds",
        "lambda",
        &[
            ("HealthCheckIntervalSeconds", "300"),
            ("HealthCheckTimeoutSeconds", "120"),
        ],
        true,
    ),
    (
        // Neither schedule member: a modify of another member is outside the rule and
        // leaves the stored schedule as it is.
        "neither schedule member",
        "http",
        &[("HealthyThresholdCount", "3")],
        true,
    ),
];

/// The create fields of the starting group named `kind`.
fn start_fields(kind: &str) -> Vec<(String, String)> {
    let (_, fields) = START_GROUPS
        .iter()
        .find(|(name, _)| *name == kind)
        .unwrap_or_else(|| panic!("case setup: no starting group named {kind}"));
    fields.iter().map(|(key, value)| p(key, value)).collect()
}

/// The schedule one reply reports, as the two elements a target group carries.
fn schedule_of(reply: &Reply) -> (Option<String>, Option<String>) {
    (
        reply.text("HealthCheckIntervalSeconds"),
        reply.text("HealthCheckTimeoutSeconds"),
    )
}

/// Assert that `reply` is the refusal the rule states: HTTP 400 `ValidationError`
/// carrying the service's wording for a schedule whose timeout outlasts its interval.
fn assert_schedule_refusal(case: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "ValidationError"),
        "case {case:?}: a schedule whose timeout outlasts its interval must be refused \
         with 400 ValidationError, got {} body {}",
        reply.status,
        reply.body
    );
    let message = unescaped(&reply.text("Message").unwrap_or_default());
    assert_eq!(
        message, HEALTH_CHECK_SCHEDULE_MESSAGE,
        "case {case:?}: expected the refusal to state the rule, got {message:?}"
    );
}

/// Read one target group back by NAME, which is how a refused create is checked to have
/// stored nothing.
async fn describe_by_name(router: &MultiServiceRouter, name: &str) -> Reply {
    let mut form = action("DescribeTargetGroups");
    form.push(p("Names.member.1", name));
    call(router, &form).await
}

#[tokio::test]
async fn a_create_persists_only_a_health_check_whose_timeout_fits_its_interval() {
    let (router, _engine) = emulator();

    for (index, (case, kind, fields, accepted)) in CREATE_CASES.iter().enumerate() {
        let name = format!("hcs-create-{index}");
        let mut form = action("CreateTargetGroup");
        form.push(p("Name", &name));
        form.extend(start_fields(kind));
        for (key, value) in fields.iter() {
            form.push(p(key, value));
        }
        let reply = call(&router, &form).await;

        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a schedule inside the rule must be accepted, got {} body {}",
                reply.status, reply.body
            );
            let described = describe_by_name(&router, &name).await;
            // Every member the request named is the stored one, and the stored pair is a
            // schedule — which is the invariant the rule exists for.
            for (key, value) in fields.iter() {
                assert_eq!(
                    described.text(key).as_deref(),
                    Some(*value),
                    "case {case:?}: expected {key} to read back as {value}, got {}",
                    described.body
                );
            }
            let (interval, timeout) = schedule_of(&described);
            let interval: i32 = interval
                .unwrap_or_else(|| panic!("case {case:?}: no interval in {}", described.body))
                .parse()
                .unwrap_or_else(|_| panic!("case {case:?}: a non-numeric interval"));
            let timeout: i32 = timeout
                .unwrap_or_else(|| panic!("case {case:?}: no timeout in {}", described.body))
                .parse()
                .unwrap_or_else(|_| panic!("case {case:?}: a non-numeric timeout"));
            assert!(
                timeout <= interval,
                "case {case:?}: a stored health check's timeout may not outlast its \
                 interval, got interval {interval} and timeout {timeout}"
            );
        } else {
            assert_schedule_refusal(case, &reply);
            // Nothing may be persisted by a refused create.
            let described = describe_by_name(&router, &name).await;
            assert_eq!(
                (described.status, described.error_code().as_str()),
                (400, "TargetGroupNotFound"),
                "case {case:?}: a refused create must store nothing, got {} body {}",
                described.status,
                described.body
            );
        }
    }
}

#[tokio::test]
async fn a_modify_is_refused_when_the_schedule_it_would_leave_cannot_be_run() {
    let (router, _engine) = emulator();

    for (index, (case, kind, fields, accepted)) in MODIFY_CASES.iter().enumerate() {
        let name = format!("hcs-modify-{index}");
        let mut created = start_fields(kind);
        created.push(p("HealthCheckIntervalSeconds", STORED_INTERVAL));
        created.push(p("HealthCheckTimeoutSeconds", STORED_TIMEOUT));
        let arn = create_group(&router, case, &name, &created).await;
        let before = health_check_of(&describe(&router, &arn).await);

        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &arn));
        for (key, value) in fields.iter() {
            form.push(p(key, value));
        }
        let reply = call(&router, &form).await;
        let described = describe(&router, &arn).await;

        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a schedule inside the rule must be accepted, got {} body {}",
                reply.status, reply.body
            );
            for (key, value) in fields.iter() {
                assert_eq!(
                    described.text(key).as_deref(),
                    Some(*value),
                    "case {case:?}: expected {key} to read back as {value}, got {}",
                    described.body
                );
            }
        } else {
            assert_schedule_refusal(case, &reply);
            assert_eq!(
                health_check_of(&described),
                before,
                "case {case:?}: a refused modify must change nothing, got {}",
                described.body
            );
        }
    }
}

#[tokio::test]
async fn the_schedule_a_series_of_modifies_leaves_is_always_one_the_check_can_run() {
    // The rule holds across a SEQUENCE, not just one call: whatever a client lowers or
    // raises, every modify the service serves leaves a target group whose timeout fits
    // inside its interval, and every one it refuses leaves the pair the modify before it
    // stored.
    let (router, _engine) = emulator();
    let mut fields = start_fields("http");
    fields.push(p("HealthCheckIntervalSeconds", "300"));
    fields.push(p("HealthCheckTimeoutSeconds", "120"));
    let arn = create_group(&router, "a walk down the schedule", "hcs-walk", &fields).await;
    let mut previous = (300, 120);

    // Each step names ONE member, so each is judged against what the step before stored.
    let walk = [
        ("the timeout down to 60", "HealthCheckTimeoutSeconds", "60"),
        ("the interval down to 61", "HealthCheckIntervalSeconds", "61"),
        ("the interval down to 60", "HealthCheckIntervalSeconds", "60"),
        ("the interval down to 5", "HealthCheckIntervalSeconds", "5"),
        ("the timeout down to 2", "HealthCheckTimeoutSeconds", "2"),
        ("the interval down to 5", "HealthCheckIntervalSeconds", "5"),
        ("the timeout up to 120", "HealthCheckTimeoutSeconds", "120"),
    ];

    for (step, member, value) in walk {
        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &arn));
        form.push(p(member, value));
        let reply = call(&router, &form).await;
        let described = describe(&router, &arn).await;
        let (interval, timeout) = schedule_of(&described);
        let interval: i32 = interval
            .unwrap_or_else(|| panic!("case {step}: no interval in {}", described.body))
            .parse()
            .unwrap_or_else(|_| panic!("case {step}: a non-numeric interval"));
        let timeout: i32 = timeout
            .unwrap_or_else(|| panic!("case {step}: no timeout in {}", described.body))
            .parse()
            .unwrap_or_else(|_| panic!("case {step}: a non-numeric timeout"));
        assert!(
            timeout <= interval,
            "case {step}: after a {} the stored schedule must still be runnable, got \
             interval {interval} and timeout {timeout}",
            if reply.status == 200 {
                "served modify"
            } else {
                "refused modify"
            }
        );
        if reply.status == 200 {
            previous = (interval, timeout);
        } else {
            assert_eq!(
                reply.error_code(),
                "ValidationError",
                "case {step}: expected the refusal of the rule, got {} body {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                (interval, timeout),
                previous,
                "case {step}: a refused modify must leave the stored schedule as it was"
            );
        }
    }
}
