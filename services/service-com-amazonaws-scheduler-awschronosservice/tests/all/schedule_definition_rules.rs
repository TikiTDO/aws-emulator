// SPDX-License-Identifier: CC0-1.0

//! The rules a schedule's own definition is held to, driven over the real
//! `restJson1` front door.
//!
//! Each suite below is one rule over a TABLE of inputs, not one recorded request:
//!
//! * `ScheduleExpression` states one of the three documented forms —
//!   `at(yyyy-mm-ddThh:mm:ss)`, `rate(value unit)`, `cron(fields)`
//!   (`CreateSchedule$ScheduleExpression`) — and anything else is refused 400
//!   `ValidationException` with nothing persisted;
//! * `ScheduleExpressionTimezone` names a zone of the IANA database;
//! * `StartDate`/`EndDate` bound a window holding at least one instant;
//! * both instants are held, and reported, at millisecond resolution;
//! * a schedule group whose deletion is under way takes no write;
//! * `UpdateSchedule` resolves the GROUP before the schedule inside it;
//! * a constraint refusal quotes the value the caller submitted IN FULL and lists
//!   an enum's value set the way the service declares it.
//!
//! Every rule is asserted on BOTH operations that write a schedule
//! (`CreateSchedule` and `UpdateSchedule`), because both carry the same members.

use smithy_server_runtime::MultiServiceRouter;

use crate::wire_harness::{call, error_code, error_message, scheduler_router};

/// A target every row shares: the suite is about the schedule's own members, and
/// this router has no IAM to resolve a role against, so the role is only shaped
/// like one.
fn target() -> serde_json::Value {
    serde_json::json!({
        "Arn": "arn:aws:sqs:eu-west-2:000000000000:q",
        "RoleArn": "arn:aws:iam::000000000000:role/r"
    })
}

/// A `CreateSchedule`/`UpdateSchedule` body carrying `members` on top of the two
/// required ones.
fn schedule_body(members: serde_json::Value) -> serde_json::Value {
    let mut body = serde_json::json!({
        "ScheduleExpression": "rate(5 minutes)",
        "FlexibleTimeWindow": { "Mode": "OFF" },
        "Target": target()
    });
    let object = body
        .as_object_mut()
        .expect("the body under construction is an object");
    for (member, value) in members
        .as_object()
        .expect("the members to set are an object")
    {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// Create one schedule that later rows update, and answer its name.
async fn seeded_schedule(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/schedules/{name}"),
        Some(schedule_body(serde_json::json!({}))),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateSchedule {name} must succeed, got {status} {body}"
    );
    name.to_string()
}

/// `GET /schedules/{Name}` as the parsed body it answers with.
async fn read_schedule(router: &MultiServiceRouter, name: &str) -> (u16, serde_json::Value) {
    call(router, "GET", &format!("/schedules/{name}"), None).await
}

/// One row of a members table: the members a write carries, and whether the rule
/// accepts them.
struct MemberCase {
    case: &'static str,
    members: serde_json::Value,
    accepted: bool,
}

/// Drive `rows` through `CreateSchedule` and `UpdateSchedule`: an accepted row is
/// written and reads back, a refused one is answered 400 `ValidationException`
/// and leaves nothing behind (create) or leaves the stored schedule as it was
/// (update).
async fn assert_members_rule(rows: Vec<MemberCase>, member_in_message: fn(&MemberCase) -> String) {
    let router = scheduler_router();
    let seeded = seeded_schedule(&router, "updated.schedule_1").await;

    for (index, row) in rows.iter().enumerate() {
        let name = format!("created-{index}");
        let body = schedule_body(row.members.clone());
        let (status, answer) = call(
            &router,
            "POST",
            &format!("/schedules/{name}"),
            Some(body.clone()),
        )
        .await;
        let (read_status, read) = read_schedule(&router, &name).await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {:?}: CreateSchedule must accept it, got {status} {answer}",
                row.case
            );
            assert_eq!(
                read_status, 200,
                "case {:?}: the schedule must be persisted, got {read_status} {read}",
                row.case
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {:?}: CreateSchedule must refuse it 400 ValidationException, got \
                 {status} {answer}",
                row.case
            );
            let expected = member_in_message(row);
            assert!(
                error_message(&answer).contains(&expected),
                "case {:?}: expected a message built from the request, naming {expected:?}, \
                 got {:?}",
                row.case,
                error_message(&answer)
            );
            assert_eq!(
                (read_status, error_code(&read).as_str()),
                (404, "ResourceNotFoundException"),
                "case {:?}: a refused create persists nothing, got {read_status} {read}",
                row.case
            );
        }

        let (status, answer) = call(
            &router,
            "PUT",
            &format!("/schedules/{seeded}"),
            Some(body.clone()),
        )
        .await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {:?}: UpdateSchedule must accept it, got {status} {answer}",
                row.case
            );
            // Put the baseline back, so the next row's refused update is checked
            // against a schedule this row did not move.
            let (status, answer) = call(
                &router,
                "PUT",
                &format!("/schedules/{seeded}"),
                Some(schedule_body(serde_json::json!({}))),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: restoring the baseline must succeed, got {status} {answer}",
                row.case
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {:?}: UpdateSchedule must refuse it 400 ValidationException, got \
                 {status} {answer}",
                row.case
            );
            let (status, read) = read_schedule(&router, &seeded).await;
            assert_eq!(
                (
                    status,
                    read.get("ScheduleExpression")
                        .and_then(serde_json::Value::as_str)
                ),
                (200, Some("rate(5 minutes)")),
                "case {:?}: a refused update leaves the stored schedule as it was, got \
                 {status} {read}",
                row.case
            );
        }
    }
}

/// The three documented forms, well-formed and malformed.
///
/// Source for the forms, their fields and the units: the `ScheduleExpression`
/// member's own documentation
/// (`smithy_query(query="doc", shape="com.amazonaws.scheduler#CreateScheduleInput",
/// member="ScheduleExpression")`) and the wildcards table of the service's
/// schedule-types reference.
fn expression_cases() -> Vec<MemberCase> {
    let rows: [(&str, &str, bool); 20] = [
        ("a rate of minutes", "rate(5 minutes)", true),
        ("a rate of one minute", "rate(1 minute)", true),
        ("a rate of hours", "rate(12 hours)", true),
        ("a rate of days", "rate(2 days)", true),
        ("a rate of zero", "rate(0 minutes)", false),
        ("a rate of prose", "rate(bogus)", false),
        ("a rate with no unit", "rate(5)", false),
        ("a rate in a unit nothing measures", "rate(5 fortnights)", false),
        ("a negative rate", "rate(-1 hours)", false),
        ("a six-field cron", "cron(0 8 1 * ? *)", true),
        ("a cron naming weekdays", "cron(15 10 ? * MON-FRI *)", true),
        ("a cron over a step", "cron(0/30 * * * ? *)", true),
        ("a cron on the last day of the month", "cron(0 8 L * ? *)", true),
        ("a cron of prose", "cron(not a cron)", false),
        ("a cron of five fields", "cron(0 8 1 * ?)", false),
        ("a cron with a field nothing parses", "cron(0 8 1 * QQ *)", false),
        ("an empty cron", "cron()", false),
        ("an at instant", "at(2030-06-15T12:30:45)", true),
        ("an at instant that is not a date", "at(tomorrow)", false),
        ("no documented form at all", "every 5 minutes", false),
    ];
    rows.into_iter()
        .map(|(case, expression, accepted)| MemberCase {
            case,
            members: serde_json::json!({ "ScheduleExpression": expression }),
            accepted,
        })
        .collect()
}

#[tokio::test]
async fn an_expression_states_one_of_the_three_documented_forms() {
    assert_members_rule(expression_cases(), |row| {
        row.members["ScheduleExpression"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    })
    .await;
}

/// Time zones, named and not. Source for the member: `ScheduleExpressionTimezone`
/// ("The timezone in which the scheduling expression is evaluated", len 1..50).
fn timezone_cases() -> Vec<MemberCase> {
    let rows: [(&str, &str, bool); 7] = [
        ("UTC", "UTC", true),
        ("a European zone", "Europe/London", true),
        ("an American zone", "America/Los_Angeles", true),
        ("an Etc zone", "Etc/GMT+5", true),
        ("a zone nothing names", "Not/AZone", false),
        ("an offset rather than a zone", "+01:00", false),
        ("a region with no city", "Europe", false),
    ];
    rows.into_iter()
        .map(|(case, timezone, accepted)| MemberCase {
            case,
            members: serde_json::json!({ "ScheduleExpressionTimezone": timezone }),
            accepted,
        })
        .collect()
}

#[tokio::test]
async fn a_timezone_names_a_zone_of_the_iana_database() {
    assert_members_rule(timezone_cases(), |row| {
        row.members["ScheduleExpressionTimezone"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    })
    .await;
}

/// Epoch seconds well ahead of any clock a test runs on, so an "ahead of the
/// clock" row stays ahead: 2286-11-20T17:46:40Z.
const FAR_AHEAD: i64 = 10_000_000_000;

/// Epoch seconds behind any clock a test runs on: 2001-09-09T01:46:40Z.
const BEHIND: i64 = 1_000_000_000;

/// Windows a schedule may fire in, and windows holding no instant at all.
fn window_cases() -> Vec<MemberCase> {
    let rows: [(&str, serde_json::Value, bool); 7] = [
        (
            "an ordered pair ahead of the clock",
            serde_json::json!({ "StartDate": FAR_AHEAD, "EndDate": FAR_AHEAD + 86_400 }),
            true,
        ),
        (
            "an ordered pair behind the clock",
            serde_json::json!({ "StartDate": BEHIND, "EndDate": BEHIND + 86_400 }),
            true,
        ),
        (
            "a start with no end",
            serde_json::json!({ "StartDate": BEHIND }),
            true,
        ),
        (
            "an end ahead of the clock with no start",
            serde_json::json!({ "EndDate": FAR_AHEAD }),
            true,
        ),
        (
            "both boundaries at one instant",
            serde_json::json!({ "StartDate": FAR_AHEAD, "EndDate": FAR_AHEAD }),
            true,
        ),
        (
            "a start after its end",
            serde_json::json!({ "StartDate": FAR_AHEAD + 86_400, "EndDate": FAR_AHEAD }),
            false,
        ),
        (
            "an end behind the clock with no start",
            serde_json::json!({ "EndDate": BEHIND }),
            false,
        ),
    ];
    rows.into_iter()
        .map(|(case, members, accepted)| MemberCase {
            case,
            members,
            accepted,
        })
        .collect()
}

#[tokio::test]
async fn a_window_holds_at_least_one_instant() {
    // The refusal names the member that bounds the empty window, which is the one
    // the request has to change.
    assert_members_rule(window_cases(), |row| {
        if row.members.get("StartDate").is_some() {
            "StartDate".to_string()
        } else {
            "EndDate".to_string()
        }
    })
    .await;
}

#[tokio::test]
async fn an_instant_round_trips_at_the_resolution_the_service_holds_it() {
    let router = scheduler_router();
    // (case, submitted seconds, the seconds the service reports). A JSON
    // timestamp travels as epoch seconds with a fraction, so the fraction is
    // where the resolution shows.
    let rows: [(&str, f64, f64); 6] = [
        ("a whole second", 4_082_400_000.0, 4_082_400_000.0),
        ("whole milliseconds", 4_082_400_000.123, 4_082_400_000.123),
        ("microseconds", 4_082_400_000.123_456, 4_082_400_000.123),
        ("nanoseconds", 4_082_400_000.123_456_7, 4_082_400_000.123),
        ("under a millisecond", 4_082_400_000.000_4, 4_082_400_000.0),
        ("half a second", 4_082_400_000.5, 4_082_400_000.5),
    ];

    for (index, (case, submitted, reported)) in rows.into_iter().enumerate() {
        let name = format!("precision-{index}");
        let body = schedule_body(serde_json::json!({
            "StartDate": submitted,
            "EndDate": submitted + 86_400.0,
        }));
        let (status, answer) = call(
            &router,
            "POST",
            &format!("/schedules/{name}"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSchedule must succeed, got {status} {answer}"
        );
        let (status, read) = read_schedule(&router, &name).await;
        assert_eq!(
            (
                status,
                read.get("StartDate").and_then(serde_json::Value::as_f64)
            ),
            (200, Some(reported)),
            "case {case:?}: expected StartDate reported as {reported}, got {status} {read}"
        );
        assert_eq!(
            read.get("EndDate").and_then(serde_json::Value::as_f64),
            Some(reported + 86_400.0),
            "case {case:?}: expected EndDate held to the same resolution, got {read}"
        );

        // The same instant through `UpdateSchedule`, which carries the same
        // members and so is held to the same resolution.
        let (status, answer) = call(
            &router,
            "PUT",
            &format!("/schedules/{name}"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateSchedule must succeed, got {status} {answer}"
        );
        let (status, read) = read_schedule(&router, &name).await;
        assert_eq!(
            (
                status,
                read.get("StartDate").and_then(serde_json::Value::as_f64)
            ),
            (200, Some(reported)),
            "case {case:?}: an updated instant is held the same way, got {status} {read}"
        );
    }
}

#[tokio::test]
async fn a_group_whose_deletion_is_under_way_takes_no_write() {
    let router = scheduler_router();
    let group = "doomed-group";
    let (status, body) = call(&router, "POST", &format!("/schedule-groups/{group}"), None).await;
    assert_eq!(
        status, 200,
        "setup: CreateScheduleGroup must succeed, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "POST",
        "/schedules/member",
        Some(schedule_body(serde_json::json!({ "GroupName": group }))),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateSchedule into the group must succeed, got {status} {body}"
    );
    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/schedule-groups/{group}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: DeleteScheduleGroup must succeed, got {status} {body}"
    );

    // The group is DELETING: a create addressed into it is refused as the
    // request-validation failure it is (400), not as a conflict a client may
    // retry.
    let (status, body) = call(
        &router,
        "POST",
        "/schedules/late",
        Some(schedule_body(serde_json::json!({ "GroupName": group }))),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "CreateSchedule into a DELETING group must be refused 400, got {status} {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("Schedule group {group} is being deleted."),
        "expected the refusal to name the group being deleted, got {body}"
    );

    // An update addressed into it reports the GROUP as gone: deleting a group
    // deletes the schedules it held, so there is nothing in it to update.
    let (status, body) = call(
        &router,
        "PUT",
        "/schedules/member",
        Some(schedule_body(serde_json::json!({ "GroupName": group }))),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "UpdateSchedule into a DELETING group must report the group gone, got {status} {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("Schedule group {group} does not exist."),
        "expected the refusal to name the group, got {body}"
    );

    // Past the due instant the group and the schedules it held are gone, and every
    // operation addressed into it reports the GROUP as missing: the group a request
    // names is resolved before the schedule inside it, so a request into a group that
    // no longer exists never names the schedule that group would have held.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    for (case, method, expected) in [
        (
            "GetSchedule",
            "GET",
            format!("Schedule group {group} does not exist."),
        ),
        (
            "DeleteSchedule",
            "DELETE",
            format!("Schedule group {group} does not exist."),
        ),
    ] {
        let (status, body) = call(
            &router,
            method,
            &format!("/schedules/member?groupName={group}"),
            None,
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body)),
            (404, "ResourceNotFoundException", expected.clone()),
            "case {case:?}: expected {expected:?}, got {status} {body}"
        );
    }
    let (status, body) = call(
        &router,
        "PUT",
        "/schedules/member",
        Some(schedule_body(serde_json::json!({ "GroupName": group }))),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str(), error_message(&body)),
        (
            404,
            "ResourceNotFoundException",
            format!("Schedule group {group} does not exist.")
        ),
        "expected UpdateSchedule to name the group that is gone, got {status} {body}"
    );
}

#[tokio::test]
async fn an_update_resolves_the_group_before_the_schedule_in_it() {
    let router = scheduler_router();
    let present = seeded_schedule(&router, "present").await;

    // (case, the group the request names, the resource the refusal must name)
    let rows: [(&str, Option<&str>, String); 3] = [
        (
            "a group this account does not hold",
            Some("never-created-group"),
            "Schedule group never-created-group does not exist.".to_string(),
        ),
        (
            "the implicit default group, which exists, and a schedule it does not hold",
            None,
            "Schedule absent does not exist.".to_string(),
        ),
        (
            "a group that exists and a schedule it does not hold",
            Some("sibling-group"),
            "Schedule absent does not exist.".to_string(),
        ),
    ];
    let (status, body) = call(&router, "POST", "/schedule-groups/sibling-group", None).await;
    assert_eq!(
        status, 200,
        "setup: CreateScheduleGroup must succeed, got {status} {body}"
    );

    for (case, group, expected) in rows {
        let members = match group {
            Some(group) => serde_json::json!({ "GroupName": group }),
            None => serde_json::json!({}),
        };
        let (status, body) = call(
            &router,
            "PUT",
            "/schedules/absent",
            Some(schedule_body(members)),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // The same reading for the two other operations that address a schedule by
    // name: the message names the schedule the request named, and nothing else.
    for (case, method) in [("GetSchedule", "GET"), ("DeleteSchedule", "DELETE")] {
        let (status, body) = call(&router, method, "/schedules/absent", None).await;
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body)),
            (
                404,
                "ResourceNotFoundException",
                "Schedule absent does not exist.".to_string()
            ),
            "case {case:?}: expected the schedule named and nothing else, got {status} {body}"
        );
    }

    // And the schedule that IS there still updates.
    let (status, body) = call(
        &router,
        "PUT",
        &format!("/schedules/{present}"),
        Some(schedule_body(serde_json::json!({ "Description": "updated" }))),
    )
    .await;
    assert_eq!(
        status, 200,
        "an existing schedule must still update, got {status} {body}"
    );
}

#[tokio::test]
async fn a_constraint_refusal_quotes_the_submitted_value_and_the_declared_value_set() {
    let router = scheduler_router();

    // `Description` is `len 0..512` (`smithy_query(query="shape",
    // shape="com.amazonaws.scheduler#Description")`), so the published bound and
    // every size past it are refused with the WHOLE value quoted — the message is
    // the only place the service reports the offending input back.
    for (case, length) in [
        ("one past the bound", 513),
        ("twice the bound", 1_024),
        ("far past the bound", 4_000),
    ] {
        let value = "d".repeat(length);
        let (status, body) = call(
            &router,
            "POST",
            "/schedules/long-description",
            Some(schedule_body(serde_json::json!({ "Description": value }))),
        )
        .await;
        let expected = format!(
            "1 validation error detected: Value '{value}' at 'description' failed to satisfy \
             constraint: Member must have length less than or equal to 512"
        );
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body)),
            (400, "ValidationException", expected.clone()),
            "case {case:?}: expected the submitted value quoted in full, got {status} {body}"
        );
    }

    // The published bound itself is accepted, so the refusal is the bound's and
    // not a shorter one.
    let (status, body) = call(
        &router,
        "POST",
        "/schedules/at-the-bound",
        Some(schedule_body(
            serde_json::json!({ "Description": "d".repeat(512) }),
        )),
    )
    .await;
    assert_eq!(
        status, 200,
        "a description at the published bound must be accepted, got {status} {body}"
    );

    // An out-of-range enum value is refused with the value set the service
    // declares, in that order. `ActionAfterCompletion` is the one enum whose
    // reported order is not the public model's (see `src/service/validation.rs`).
    for (case, value) in [
        ("a value of another service", "ARCHIVE"),
        ("the empty string", ""),
        ("a declared value in the wrong case", "delete"),
    ] {
        let (status, body) = call(
            &router,
            "POST",
            "/schedules/bad-action",
            Some(schedule_body(
                serde_json::json!({ "ActionAfterCompletion": value }),
            )),
        )
        .await;
        let expected = format!(
            "1 validation error detected: Value '{value}' at 'actionAfterCompletion' failed \
             to satisfy constraint: Member must satisfy enum value set: [DELETE, NONE]"
        );
        assert_eq!(
            (status, error_code(&body).as_str(), error_message(&body)),
            (400, "ValidationException", expected.clone()),
            "case {case:?}: expected the declared value set, got {status} {body}"
        );
    }

    // An enum whose reported set IS the model's keeps the model's order, so the
    // restatement is one enum's and not every enum's.
    let (status, body) = call(
        &router,
        "POST",
        "/schedules/bad-state",
        Some(schedule_body(serde_json::json!({ "State": "PAUSED" }))),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str(), error_message(&body)),
        (
            400,
            "ValidationException",
            "1 validation error detected: Value 'PAUSED' at 'state' failed to satisfy \
             constraint: Member must satisfy enum value set: [ENABLED, DISABLED]"
                .to_string()
        ),
        "expected ScheduleState reported as the model declares it, got {status} {body}"
    );
}
