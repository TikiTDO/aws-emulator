// SPDX-License-Identifier: CC0-1.0

//! Maintenance windows end to end: `CreateMaintenanceWindow` mints one and
//! `GetMaintenanceWindow` reports it.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call_in_region`], [`error_code`],
//! [`error_message`]) dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>`
//! through the multi-service router. Nothing reaches into the state struct, so a response
//! that is not computed from persisted state cannot pass: every read follows a write.
//!
//! The rules under test are that a read of a window reports, for the `WindowId` the
//! request names, exactly the members the create of that window persisted — with the
//! documented default of every member the create left out — that a `Schedule` stating none
//! of the forms this service documents is refused before any window exists, that
//! `NextExecutionTime` is the first instant the stored schedule names, and that an
//! idempotency token makes a retried create the SAME window. The suites are tables whose
//! rows differ along what each rule generalises over: which optional members a create
//! supplies, the duration and cutoff figures, the form of the schedule, the zone it is
//! read in, the days it waits, the Region the endpoint is, and the way an id or an
//! expression misses.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call_in_region, error_code, error_message, router, REGION};

/// A second Region, so a window is shown to live at the endpoint of ONE Region.
const OTHER_REGION: &str = "eu-central-1";

/// A `WindowId` of the form the modelled `MaintenanceWindowId` pattern requires
/// (`^mw-[0-9a-f]{17}$`) that no create has minted.
const UNKNOWN_WINDOW_ID: &str = "mw-00000000000000000";

/// `CreateMaintenanceWindow` at `region`'s endpoint with `body`.
async fn create(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, region, "CreateMaintenanceWindow", body).await
}

/// The same write, asserting it succeeded, and returning the `WindowId` it minted.
async fn create_ok(router: &MultiServiceRouter, case: &str, region: &str, body: &Value) -> String {
    let (status, response) = create(router, region, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateMaintenanceWindow {body} must succeed, got {status} {response}"
    );
    response["WindowId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a create reports a WindowId, got {response}"))
        .to_string()
}

/// `GetMaintenanceWindow` at `region`'s endpoint for `window_id`.
async fn get(router: &MultiServiceRouter, region: &str, window_id: &str) -> (u16, Value) {
    call_in_region(
        router,
        region,
        "GetMaintenanceWindow",
        &json!({ "WindowId": window_id }),
    )
    .await
}

/// The same read, asserting it succeeded.
async fn get_ok(router: &MultiServiceRouter, case: &str, region: &str, window_id: &str) -> Value {
    let (status, response) = get(router, region, window_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMaintenanceWindow {window_id:?} must succeed, got {status} {response}"
    );
    response
}

/// A create body carrying only the members the model makes required.
fn minimal(name: &str, schedule: &str) -> Value {
    json!({
        "Name": name,
        "Schedule": schedule,
        "Duration": 2,
        "Cutoff": 1,
        "AllowUnassociatedTargets": false,
    })
}

/// A read of a window reports the members its create persisted, and the documented default
/// of every member the create left out. The rows differ along which optional members the
/// create supplies and along the figures it states.
#[tokio::test]
async fn a_read_reports_what_the_create_persisted() {
    let router = router();
    // (case, the create body, the members the read must report)
    let rows: [(&str, Value, Value); 5] = [
        (
            "only the required members",
            minimal("only-required", "cron(0 8 ? * MON *)"),
            json!({
                "Name": "only-required",
                "Schedule": "cron(0 8 ? * MON *)",
                "Duration": 2,
                "Cutoff": 1,
                "AllowUnassociatedTargets": false,
                // A create carries no `Enabled` member, so a window it mints is enabled.
                "Enabled": true,
            }),
        ),
        (
            "a description and a zone",
            json!({
                "Name": "described-window",
                "Description": "Patch the fleet",
                "Schedule": "cron(0 16 ? * TUE *)",
                "ScheduleTimezone": "America/Los_Angeles",
                "Duration": 4,
                "Cutoff": 0,
                "AllowUnassociatedTargets": true,
            }),
            json!({
                "Name": "described-window",
                "Description": "Patch the fleet",
                "Schedule": "cron(0 16 ? * TUE *)",
                "ScheduleTimezone": "America/Los_Angeles",
                "Duration": 4,
                "Cutoff": 0,
                "AllowUnassociatedTargets": true,
                "Enabled": true,
            }),
        ),
        (
            "the activation interval, echoed exactly",
            json!({
                "Name": "dated-window",
                "Schedule": "rate(7 days)",
                "StartDate": "2030-01-01T00:00:00Z",
                "EndDate": "2031-01-01T00:00:00-05:00",
                "Duration": 1,
                "Cutoff": 0,
                "AllowUnassociatedTargets": false,
            }),
            json!({
                "Name": "dated-window",
                "Schedule": "rate(7 days)",
                "StartDate": "2030-01-01T00:00:00Z",
                "EndDate": "2031-01-01T00:00:00-05:00",
                "Duration": 1,
                "Cutoff": 0,
            }),
        ),
        (
            "a cron that waits whole days",
            json!({
                "Name": "offset-window",
                "Schedule": "cron(30 23 ? * TUE *)",
                "ScheduleOffset": 2,
                "Duration": 24,
                "Cutoff": 23,
                "AllowUnassociatedTargets": false,
            }),
            json!({
                "Name": "offset-window",
                "Schedule": "cron(30 23 ? * TUE *)",
                "ScheduleOffset": 2,
                "Duration": 24,
                "Cutoff": 23,
            }),
        ),
        (
            "a one-time window",
            json!({
                "Name": "one.time-window_1",
                "Schedule": "at(2030-06-01T12:00:00)",
                "Duration": 3,
                "Cutoff": 1,
                "AllowUnassociatedTargets": false,
            }),
            json!({
                "Name": "one.time-window_1",
                "Schedule": "at(2030-06-01T12:00:00)",
                "Duration": 3,
                "Cutoff": 1,
            }),
        ),
    ];

    for (case, body, expected) in rows {
        let window_id = create_ok(&router, case, REGION, &body).await;
        assert!(
            window_id.starts_with("mw-") && window_id.len() == 20,
            "case {case:?}: a minted WindowId has the modelled form, got {window_id:?}"
        );
        let read = get_ok(&router, case, REGION, &window_id).await;
        assert_eq!(
            read["WindowId"].as_str(),
            Some(window_id.as_str()),
            "case {case:?}: the read reports the id it was asked for, got {read}"
        );
        for (member, value) in expected.as_object().expect("the expectation is an object") {
            assert_eq!(
                &read[member], value,
                "case {case:?}: expected {member}={value}, got {}",
                read[member]
            );
        }
        // A member the create left out is reported as absent rather than invented.
        if body.get("Description").is_none() {
            assert!(
                read.get("Description").is_none(),
                "case {case:?}: a window with no description reports none, got {}",
                read["Description"]
            );
        }
        if body.get("ScheduleOffset").is_none() {
            assert!(
                read.get("ScheduleOffset").is_none(),
                "case {case:?}: a window with no offset reports none, got {}",
                read["ScheduleOffset"]
            );
        }
        // Nothing has updated the window, so it was last written when it was created.
        assert_eq!(
            read["CreatedDate"], read["ModifiedDate"],
            "case {case:?}: a window nothing has updated reports one instant for both, \
             got {read}"
        );
    }
}

/// `NextExecutionTime` is the first instant the stored schedule names: the rows differ
/// along the form of the schedule, the zone it is read in and the days it waits.
#[tokio::test]
async fn the_next_execution_time_is_read_from_the_schedule() {
    let router = router();
    // (case, the create body, how the reported instant ends)
    let rows: [(&str, Value, &str); 5] = [
        (
            "a cron at an hour of the day, in UTC",
            minimal("next-utc", "cron(0 8 * * ? *)"),
            "T08:00Z",
        ),
        (
            "the same cron read in a western zone",
            json!({
                "Name": "next-western",
                "Schedule": "cron(0 8 * * ? *)",
                "ScheduleTimezone": "America/Los_Angeles",
                "Duration": 2,
                "Cutoff": 1,
                "AllowUnassociatedTargets": false,
            }),
            "T08:00-08:00",
        ),
        (
            "the same cron read in an eastern zone",
            json!({
                "Name": "next-eastern",
                "Schedule": "cron(0 8 * * ? *)",
                "ScheduleTimezone": "Asia/Seoul",
                "Duration": 2,
                "Cutoff": 1,
                "AllowUnassociatedTargets": false,
            }),
            "T08:00+09:00",
        ),
        (
            "a cron on the half hour",
            minimal("next-half-hour", "cron(30 23 * * ? *)"),
            "T23:30Z",
        ),
        (
            "a one-time window still ahead",
            minimal("next-one-time", "at(2030-06-01T12:00:00)"),
            "2030-06-01T12:00Z",
        ),
    ];

    for (case, body, ends_with) in rows {
        let window_id = create_ok(&router, case, REGION, &body).await;
        let read = get_ok(&router, case, REGION, &window_id).await;
        let next = read["NextExecutionTime"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: a window that will run reports a next execution, got {read}")
        });
        assert!(
            next.ends_with(ends_with),
            "case {case:?}: expected a next execution ending {ends_with:?}, got {next:?}"
        );
    }

    // A window whose activation interval has already closed will not run again, so it
    // reports no next execution rather than an instant outside that interval.
    let closed = json!({
        "Name": "already-closed",
        "Schedule": "cron(0 8 * * ? *)",
        "StartDate": "2020-01-01T00:00:00Z",
        "EndDate": "2020-02-01T00:00:00Z",
        "Duration": 2,
        "Cutoff": 1,
        "AllowUnassociatedTargets": false,
    });
    let window_id = create_ok(&router, "an interval already closed", REGION, &closed).await;
    let read = get_ok(&router, "an interval already closed", REGION, &window_id).await;
    assert!(
        read.get("NextExecutionTime").is_none(),
        "case \"an interval already closed\": a window that will not run reports no next \
         execution, got {read}"
    );

    // A bound the window STATES that names no instant leaves the interval it would run in
    // unknown: the date is still echoed exactly as the create stated it, and no next
    // execution is reported for it rather than one computed as though it had no bound.
    let unreadable = json!({
        "Name": "unreadable-bound",
        "Schedule": "cron(0 8 * * ? *)",
        "StartDate": "not-a-date",
        "Duration": 2,
        "Cutoff": 1,
        "AllowUnassociatedTargets": false,
    });
    let case = "an activation stated in no readable form";
    let window_id = create_ok(&router, case, REGION, &unreadable).await;
    let read = get_ok(&router, case, REGION, &window_id).await;
    assert_eq!(
        read["StartDate"].as_str(),
        Some("not-a-date"),
        "case {case:?}: the stated activation is echoed exactly, got {read}"
    );
    assert!(
        read.get("NextExecutionTime").is_none(),
        "case {case:?}: a window whose stated bound names no instant reports no next \
         execution, got {read}"
    );
}

/// A `Schedule` that states none of the documented forms is refused, and nothing is
/// written for it; one that states a form is accepted. The rows differ along the form and
/// along the way a malformed one is malformed.
#[tokio::test]
async fn a_schedule_states_a_cron_or_rate_expression() {
    let router = router();
    // (case, schedule, states a schedule this service can read)
    let rows: [(&str, &str, bool); 21] = [
        ("a six-field cron", "cron(0 8 ? * MON *)", true),
        ("a cron naming a range of days", "cron(0 18 ? * MON-FRI *)", true),
        ("a cron over a step", "cron(0/15 * * * ? *)", true),
        // The wildcards this service's own cron dialect documents.
        ("a cron on the third Tuesday", "cron(30 23 ? * TUE#3 *)", true),
        ("a cron on the last day of the month", "cron(0 8 L * ? *)", true),
        ("a cron on the nearest weekday", "cron(0 8 15W * ? *)", true),
        ("a rate of minutes", "rate(30 minutes)", true),
        ("a rate of one day", "rate(1 day)", true),
        ("a one-time instant", "at(2030-06-01T12:00:00)", true),
        ("a cron of five fields", "cron(0 8 1 * ?)", false),
        ("a cron of prose", "cron(not a cron)", false),
        // A token that merely CARRIES a dialect character is still read by the shared
        // grammar rather than waved through as this service's dialect.
        ("a cron naming a weekday that is not one", "cron(0 8 ? * WEDX *)", false),
        ("a cron naming a day of the month that is not one", "cron(0 8 32W * ? *)", false),
        ("a cron on a weekday of no such ordinal", "cron(0 8 ? * TUE#9 *)", false),
        // A dialect form states something only in the field it is documented for.
        ("a cron naming the last day as a minute", "cron(L 8 ? * MON *)", false),
        ("a cron naming the last day of the week", "cron(0 8 ? * L *)", true),
        ("a rate of zero", "rate(0 minutes)", false),
        ("a rate in a unit nothing measures", "rate(5 fortnights)", false),
        // A period whose first occurrence falls off the calendar names no instant to open
        // at, so it is no schedule either.
        ("a rate past the calendar", "rate(99999999999 days)", false),
        ("a one-time instant that is not one", "at(tomorrow)", false),
        ("no documented form at all", "every 30 minutes", false),
    ];

    for (index, (case, schedule, readable)) in rows.into_iter().enumerate() {
        let body = minimal(&format!("schedule-row-{index}"), schedule);
        let (status, response) = create(&router, REGION, &body).await;
        if readable {
            assert_eq!(
                status, 200,
                "case {case:?}: {schedule:?} is a schedule this service reads, got {status} \
                 {response}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: {schedule:?} is no schedule, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a validation refusal, got {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message,
            "The schedule is invalid. Verify your cron or rate expression and try again.",
            "case {case:?}: the refusal is worded as this error class is, got {message:?}"
        );
    }
}

/// A `ScheduleTimezone` naming no IANA zone is refused: the instants the window would open
/// at are undefined without one.
#[tokio::test]
async fn a_schedule_timezone_names_an_iana_zone() {
    let router = router();
    // (case, timezone, names a zone the IANA database carries)
    let rows: [(&str, &str, bool); 5] = [
        ("UTC", "UTC", true),
        ("an American zone", "America/Los_Angeles", true),
        ("an Asian zone", "Asia/Seoul", true),
        ("a zone nothing names", "Not/AZone", false),
        ("an offset rather than a zone", "+01:00", false),
    ];

    for (index, (case, timezone, names_a_zone)) in rows.into_iter().enumerate() {
        let mut body = minimal(&format!("zone-row-{index}"), "cron(0 8 ? * MON *)");
        body["ScheduleTimezone"] = json!(timezone);
        let (status, response) = create(&router, REGION, &body).await;
        if names_a_zone {
            assert_eq!(
                status, 200,
                "case {case:?}: {timezone:?} names a zone, got {status} {response}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: {timezone:?} names no zone, got {status} {response}"
        );
        assert!(
            error_message(&response).contains(timezone),
            "case {case:?}: the refusal states the zone the request carried, got {response}"
        );
    }
}

/// A `WindowId` that names no window reachable through the endpoint is refused through the
/// modelled `DoesNotExistException`. The rows differ along the way the id misses.
#[tokio::test]
async fn an_id_that_names_no_window_is_refused() {
    let router = router();
    let body = minimal("a-window-of-one-region", "cron(0 8 ? * MON *)");
    let window_id = create_ok(&router, "a window of one Region", REGION, &body).await;

    // (case, the Region read at, the id read for)
    let rows: [(&str, &str, &str); 3] = [
        ("an id nothing created", REGION, UNKNOWN_WINDOW_ID),
        ("a window of another Region", OTHER_REGION, &window_id),
        ("an id nothing created, at another Region", OTHER_REGION, UNKNOWN_WINDOW_ID),
    ];

    for (case, region, asked_for) in rows {
        let (status, response) = get(&router, region, asked_for).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {asked_for:?} names no window at {region}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "DoesNotExistException",
            "case {case:?}: expected the modelled not-found refusal, got {response}"
        );
        assert!(
            error_message(&response).contains(asked_for),
            "case {case:?}: the refusal states the window the request asked for, got \
             {response}"
        );
    }

    // The window the first Region holds is still readable there, so the refusals above
    // are about the id and the endpoint rather than about the window being gone.
    let read = get_ok(&router, "the window of its own Region", REGION, &window_id).await;
    assert_eq!(
        read["Name"].as_str(),
        Some("a-window-of-one-region"),
        "the window of its own Region is still read there, got {read}"
    );
}

/// An idempotency token makes a retried create the SAME window, and the same token
/// carrying different members is refused. The rows differ along which member differs.
#[tokio::test]
async fn an_idempotency_token_makes_a_retry_the_same_window() {
    let router = router();
    let mut first = minimal("token-window", "cron(0 8 ? * MON *)");
    first["ClientToken"] = json!("a-token-of-one-create");
    first["Tags"] = json!([{ "Key": "Env", "Value": "prod" }]);
    let window_id = create_ok(&router, "the first create", REGION, &first).await;

    // The same call again reports the window the first one minted rather than minting a
    // second, whatever order its tags arrive in.
    let mut retry = first.clone();
    retry["Tags"] = json!([{ "Key": "Env", "Value": "prod" }]);
    let retried = create_ok(&router, "a retry of that create", REGION, &retry).await;
    assert_eq!(
        retried, window_id,
        "a retry under one token reports the window that token already minted, got \
         {retried:?} for {window_id:?}"
    );

    // (case, how the second create differs under the same token)
    let rows: [(&str, &str, Value); 6] = [
        ("the name", "Name", json!("another-window")),
        ("the schedule", "Schedule", json!("rate(1 day)")),
        ("the duration", "Duration", json!(3)),
        ("the cutoff", "Cutoff", json!(0)),
        ("a description it did not carry", "Description", json!("new")),
        ("a tag", "Tags", json!([{ "Key": "Env", "Value": "dev" }])),
    ];

    for (case, member, value) in rows {
        let mut body = first.clone();
        body[member] = value;
        let (status, response) = create(&router, REGION, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a create differing in {member} under one token is another \
             call, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "IdempotentParameterMismatch",
            "case {case:?}: expected the modelled idempotency refusal, got {response}"
        );
        // The error code is the whole of what this refusal says: no sentence is invented
        // for it.
        assert_eq!(
            error_message(&response),
            "",
            "case {case:?}: the idempotency refusal carries no message, got {response}"
        );
    }

    // A token of another Region's endpoint has run no create there, so it mints its own
    // window rather than resolving the first one.
    let elsewhere = create_ok(&router, "the same token at another Region", OTHER_REGION, &first)
        .await;
    assert_ne!(
        elsewhere, window_id,
        "a token is a token of ONE Region's endpoint, got {elsewhere:?} for {window_id:?}"
    );
}

/// The tags a create supplied are the window's own: the tag plane reports them for the
/// `MaintenanceWindow` resource type under the id the create minted, and reports nothing
/// for an id nothing created.
#[tokio::test]
async fn the_tags_a_create_supplied_belong_to_the_window() {
    let router = router();
    // (case, the tags the create supplies)
    let rows: [(&str, Value); 3] = [
        ("no tags at all", json!([])),
        ("one tag", json!([{ "Key": "Env", "Value": "prod" }])),
        (
            "several tags",
            json!([
                { "Key": "Env", "Value": "prod" },
                { "Key": "OS", "Value": "Windows" },
                { "Key": "TaskType", "Value": "AgentUpdate" },
            ]),
        ),
    ];

    for (index, (case, tags)) in rows.into_iter().enumerate() {
        let mut body = minimal(&format!("tagged-window-{index}"), "cron(0 8 ? * MON *)");
        body["Tags"] = tags.clone();
        let window_id = create_ok(&router, case, REGION, &body).await;
        let (status, response) = call_in_region(
            &router,
            REGION,
            "ListTagsForResource",
            &json!({ "ResourceType": "MaintenanceWindow", "ResourceId": window_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the window the create minted carries a tag set, got {status} \
             {response}"
        );
        let reported = response["TagList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a tag listing reports a TagList, got {response}"));
        assert_eq!(
            reported.len(),
            tags.as_array().expect("the row's tags are a list").len(),
            "case {case:?}: expected the tags the create supplied, got {response}"
        );
        for tag in tags.as_array().expect("the row's tags are a list") {
            assert!(
                reported.contains(tag),
                "case {case:?}: expected {tag} among the reported tags, got {response}"
            );
        }
    }

    // An id nothing created addresses no window, so it carries no tag set to report.
    let (status, response) = call_in_region(
        &router,
        REGION,
        "ListTagsForResource",
        &json!({ "ResourceType": "MaintenanceWindow", "ResourceId": UNKNOWN_WINDOW_ID }),
    )
    .await;
    assert_eq!(
        status, 400,
        "an id nothing created addresses no window to tag, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidResourceId",
        "expected the modelled invalid-id refusal, got {response}"
    );
}
