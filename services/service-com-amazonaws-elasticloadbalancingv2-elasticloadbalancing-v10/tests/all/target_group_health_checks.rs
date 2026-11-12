// SPDX-License-Identifier: CC0-1.0

//! `ModifyTargetGroup`'s numeric health-check members, over the wire: each one is
//! accepted on both published bounds, read back EXACTLY as it was written, and refused
//! outside them with nothing stored.
//!
//! The ranges are the model's own
//! (`smithy_query(query="operation",operation="ModifyTargetGroup")`):
//! `HealthCheckIntervalSeconds` 5..300, `HealthCheckTimeoutSeconds` 2..120,
//! `HealthyThresholdCount` and `UnhealthyThresholdCount` 2..10. They are modelled
//! constraints, so the boundary validator refuses a value outside them before the
//! handler runs — which is what makes the pair of tables below the rule: inside the
//! range the value is PERSISTED and reported unchanged, and outside it the call is
//! refused (HTTP 400 `ValidationError`) and the stored health check is untouched.
//!
//! The wire harness is `provisioning_loop`'s, and the target-group helpers are the ones
//! the sibling `target_group_health_check_port` suite owns; nothing here restates
//! either.
//!
//! Each row starts from a group whose stored interval and timeout sit at OPPOSITE ends of
//! their ranges, so the value under test is the only thing that decides the call: the
//! schedule rule the two members answer together (the interval has to exceed the timeout)
//! is the subject of `target_group_health_check_schedule`, not of this table.

use crate::provisioning_loop::{action, call, emulator, p};
use crate::target_group_health_check_port::{
    create_group, describe, health_check_of, http_group_fields,
};

/// An HTTP target group whose stored health-check schedule leaves room for every value in
/// [`NUMERIC_CASES`]: the interval at the high bound of its range and the timeout at the
/// low bound of its, so no row is refused for the schedule it would leave behind rather
/// than for the bound it is testing.
fn widest_schedule_group_fields() -> Vec<(String, String)> {
    let mut fields = http_group_fields();
    fields.push(p("HealthCheckIntervalSeconds", "300"));
    fields.push(p("HealthCheckTimeoutSeconds", "2"));
    fields
}

/// One row: the member, the value, and whether the model's range admits it.
type NumericCase = (&'static str, &'static str, bool);

/// Every numeric health-check member `ModifyTargetGroup` declares, on each published
/// bound, one past each, and — for the two that span a wide range — a value in between
/// that no recorded call cites.
const NUMERIC_CASES: &[NumericCase] = &[
    ("HealthCheckIntervalSeconds", "5", true),
    ("HealthCheckIntervalSeconds", "42", true),
    ("HealthCheckIntervalSeconds", "300", true),
    ("HealthCheckIntervalSeconds", "4", false),
    ("HealthCheckIntervalSeconds", "301", false),
    ("HealthCheckTimeoutSeconds", "2", true),
    ("HealthCheckTimeoutSeconds", "37", true),
    ("HealthCheckTimeoutSeconds", "120", true),
    ("HealthCheckTimeoutSeconds", "1", false),
    ("HealthCheckTimeoutSeconds", "121", false),
    ("HealthyThresholdCount", "2", true),
    ("HealthyThresholdCount", "10", true),
    ("HealthyThresholdCount", "1", false),
    ("HealthyThresholdCount", "11", false),
    ("UnhealthyThresholdCount", "2", true),
    ("UnhealthyThresholdCount", "10", true),
    ("UnhealthyThresholdCount", "1", false),
    ("UnhealthyThresholdCount", "11", false),
];

#[tokio::test]
async fn a_numeric_health_check_member_round_trips_inside_its_range_and_is_refused_outside() {
    let (router, _engine) = emulator();

    for (index, (member, value, accepted)) in NUMERIC_CASES.iter().enumerate() {
        let case = format!("{member}={value}");
        let name = format!("mtg-num-{index}");
        let arn = create_group(&router, &case, &name, &widest_schedule_group_fields()).await;
        let before = health_check_of(&describe(&router, &arn).await);

        let mut form = action("ModifyTargetGroup");
        form.push(p("TargetGroupArn", &arn));
        form.push(p(member, value));
        let reply = call(&router, &form).await;
        let described = describe(&router, &arn).await;

        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case}: inside the published range, expected 200, got {} body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.text(member).as_deref(),
                Some(*value),
                "case {case}: the modify must answer the value it stored, got {}",
                reply.body
            );
            assert_eq!(
                described.text(member).as_deref(),
                Some(*value),
                "case {case}: expected the stored value to read back exactly, got {}",
                described.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.error_code().as_str()),
                (400, "ValidationError"),
                "case {case}: outside the published range, expected 400 ValidationError, got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                health_check_of(&described),
                before,
                "case {case}: a refused modify must change nothing, got {}",
                described.body
            );
        }
    }
}

#[tokio::test]
async fn every_numeric_member_written_in_one_request_is_stored_as_sent() {
    // The members do not interfere: one request naming all four stores all four, each
    // exactly as sent.
    let written = [
        ("HealthCheckIntervalSeconds", "11"),
        ("HealthCheckTimeoutSeconds", "7"),
        ("HealthyThresholdCount", "3"),
        ("UnhealthyThresholdCount", "9"),
    ];

    let (router, _engine) = emulator();
    let arn = create_group(
        &router,
        "every numeric member at once",
        "mtg-num-all",
        &http_group_fields(),
    )
    .await;

    let mut form = action("ModifyTargetGroup");
    form.push(p("TargetGroupArn", &arn));
    for (member, value) in written {
        form.push(p(member, value));
    }
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case every numeric member at once: expected 200, got {} body {}",
        reply.status, reply.body
    );

    let described = describe(&router, &arn).await;
    for (member, value) in written {
        assert_eq!(
            described.text(member).as_deref(),
            Some(value),
            "case every numeric member at once: expected {member} to read back as {value}, got {}",
            described.body
        );
    }
}
