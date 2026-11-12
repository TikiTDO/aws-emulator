// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule a scheduled query's `scheduleExpression` is held to, on both
//! operations that state one — `CreateScheduledQuery` and `UpdateScheduledQuery`.
//!
//! The member is "A cron expression that defines when the scheduled query runs", so the value has
//! to PARSE as a schedule before anything is written: either a `cron(...)` carrying its six
//! fields (minutes hours day_of_month month day_of_week year), each a legal cron field in the
//! place it sits, or a `rate(<value> <unit>)`. A value that parses as neither schedules nothing
//! and is refused HTTP 400 `ValidationException` with the sentence naming the expected format,
//! with no record stored and no stored query rewritten; a value that parses is accepted and read
//! back byte for byte.
//!
//! Both halves are asserted over a SET of expressions — the field counts either side of six, a
//! field whose value, name or spelling is not cron, the forms a cron field takes, and each
//! published rate unit in both numbers — rather than over one request. Every case drives the
//! service the way a client does, `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it, and every write is
//! proved through `GetScheduledQuery`, so a response that is not computed from the shared store
//! cannot pass.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::ComAmazonawsCloudwatchlogsLogs20140328State;

use crate::harness::Logs;

/// The operations under test, and the reader every write is proved through.
const CREATE: &str = "CreateScheduledQuery";
const UPDATE: &str = "UpdateScheduledQuery";
const GET: &str = "GetScheduledQuery";

/// The codes the two refusals travel under.
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// An IAM role ARN every case executes under, so no case is refused about its role.
const ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";

/// The sentence a schedule expression that does not parse is refused with, pinned LITERALLY here
/// rather than built with the helper the handlers call, so a wording that is wrong but consistent
/// with the implementation cannot pass. The only variable part is the leading count of violations
/// the request carries, which for a request breaking this one rule is one.
fn schedule_refusal() -> String {
    "1 validation errors detected: Invalid cron expression format. Expected format: \
     cron(minutes hours day_of_month month day_of_week year)"
        .to_string()
}

/// The sentence a scheduled query the account does not hold is refused with; the variable part is
/// the identifier the request asked for.
fn not_found(identifier: &str) -> String {
    format!("Scheduled query with identifier {identifier} not found")
}

/// Every schedule expression that does not parse, as the class the rule generalises over: a cron
/// body carrying fewer or more than the six fields the format states, a cron field that is not a
/// legal cron field in the place it sits — a value past the field's range, a name no month or
/// weekday has, a word that is not cron at all — each way a `rate(...)` fails to state a positive
/// count of a published unit, and a value that is neither form at all.
fn refused() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a cron body of no fields", "cron()"),
        ("a cron body of one field", "cron(0)"),
        ("a cron body of four fields", "cron(0 12 * *)"),
        ("a cron body of five fields", "cron(0 12 * * ?)"),
        ("a cron body of seven fields", "cron(0 12 * * ? * *)"),
        ("a cron field outside cron's alphabet", "cron(0 12 * * ? ev€ry)"),
        ("a minute spelled as a word", "cron(sixty 10 * * ? *)"),
        ("a minute past its range", "cron(99 10 * * ? *)"),
        ("an hour past its range", "cron(0 99 * * ? *)"),
        ("a day of the month past its range", "cron(0 12 32 * ? *)"),
        ("a month no calendar has", "cron(0 12 * SMARCH ? *)"),
        ("a weekday no week has", "cron(0 10 ? * FUNDAY *)"),
        ("a nth-weekday marker past the fifth", "cron(0 12 ? * 6#9 *)"),
        ("a last-weekday marker over a wildcard", "cron(0 12 ? * *L *)"),
        ("a weekday marker outside the field's days", "cron(0 12 ? * 0L *)"),
        ("a last-weekday marker over a range", "cron(0 12 ? * 1-5L *)"),
        ("a nth-weekday marker over a list", "cron(0 12 ? * 1,2#3 *)"),
        ("six fields of markers and words", "cron(sixty 99 L L L L)"),
        ("a rate stating no unit", "rate(5)"),
        ("a rate stating a value of none", "rate(0 minutes)"),
        ("a rate whose unit disagrees in number", "rate(1 minutes)"),
        ("a rate whose value is not a number", "rate(five minutes)"),
        ("a rate of an unpublished unit", "rate(5 fortnights)"),
        ("a rate stating a unit alone", "rate(minutes)"),
        ("a schedule of neither form", "every 5 minutes"),
        ("a bare cron body", "0 12 * * ? *"),
        ("an unterminated cron expression", "cron(0 12 * * ? *"),
        ("an empty expression", ""),
    ]
}

/// Every schedule expression that parses, over the forms the two published shapes take: the
/// wildcards, lists, ranges, steps, names, last-day and nth-weekday markers a cron field is
/// written with, and each rate unit in the singular and the plural.
///
/// Source: the six-field cron format the refusal above names — "minutes hours day_of_month month
/// day_of_week year" — and the rate form `rate(<value> <unit>)` over the units a schedule
/// expression publishes (minute(s), hour(s), day(s)).
fn accepted() -> Vec<(&'static str, &'static str)> {
    vec![
        ("every hour on the hour", "cron(0 * * * ? *)"),
        ("every five minutes", "cron(*/5 * * * ? *)"),
        ("every minute of every day", "cron(* * * * ? *)"),
        ("a list and a range of hours", "cron(0,30 9-17 ? * MON-FRI *)"),
        ("a stepped range of hours", "cron(0 9-17/2 ? * MON-FRI *)"),
        ("the last day of the month", "cron(15 10 L * ? *)"),
        ("the last weekday of the month", "cron(15 10 LW * ? *)"),
        ("the weekday nearest the 15th", "cron(15 10 15W * ? *)"),
        ("the third Friday of the month", "cron(0 12 ? * 6#3 *)"),
        ("the last Friday of the month", "cron(0 12 ? * 6L *)"),
        ("a single named month", "cron(0 12 1 JAN ? *)"),
        ("a range of years", "cron(0 12 1 * ? 2026-2030)"),
        ("a rate of one minute", "rate(1 minute)"),
        ("a rate of several minutes", "rate(5 minutes)"),
        ("a rate of one hour", "rate(1 hour)"),
        ("a rate of several hours", "rate(12 hours)"),
        ("a rate of one day", "rate(1 day)"),
        ("a rate of several days", "rate(7 days)"),
    ]
}

/// A create body for `name` stating `expression` as its schedule, and the members a create must
/// state besides.
fn create_body(name: &str, expression: &str) -> Value {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("queryLanguage".to_string(), json!("CWLI"));
    body.insert("queryString".to_string(), json!("fields @timestamp | limit 1"));
    body.insert("scheduleExpression".to_string(), json!(expression));
    body.insert("executionRoleArn".to_string(), json!(ROLE));
    Value::Object(body)
}

/// An update body addressing `identifier` and stating `expression` as its schedule.
fn update_body(identifier: &str, expression: &str) -> Value {
    let mut body = Map::new();
    body.insert("identifier".to_string(), json!(identifier));
    body.insert("queryLanguage".to_string(), json!("CWLI"));
    body.insert("queryString".to_string(), json!("fields @message"));
    body.insert("scheduleExpression".to_string(), json!(expression));
    body.insert("executionRoleArn".to_string(), json!(ROLE));
    Value::Object(body)
}

/// The names the store itself holds, which is what a later create's uniqueness check reads: a
/// refused create must leave the name free rather than occupied by a query no reader reports.
fn held(logs: &Logs) -> Vec<String> {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let names: Vec<String> = state.lock().scheduled_queries.keys().cloned().collect();
    names
}

/// The schedule expression one reader reports for a query, or `None` when it reports none.
fn reported_schedule(read: &Value) -> Option<&str> {
    read.get("scheduleExpression").and_then(Value::as_str)
}

/// A create whose schedule expression does not parse is refused with the format sentence and
/// stores NOTHING: the query it names is reported by no reader and the store holds no record for
/// it, so the name is free for a corrected retry.
#[tokio::test]
async fn a_create_whose_schedule_does_not_parse_is_refused_and_stores_nothing() {
    let logs = Logs::new();
    let expected = schedule_refusal();

    for (case, expression) in refused() {
        let name = "orders-errors-hourly";
        let (code, message) = logs.refusal(CREATE, &create_body(name, expression)).await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?} ({expression:?}) on {CREATE}: expected {VALIDATION}, got {code} \
             {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?} ({expression:?}) on {CREATE}: expected {expected:?}, got {message:?}"
        );

        // Nothing came into being, so the reader refuses the identifier the create asked for.
        let (code, message) = logs.refusal(GET, &json!({"identifier": name})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?} after the refused {CREATE}: expected {NOT_FOUND}, got {code} \
             {message:?}"
        );
        let missing = not_found(name);
        assert_eq!(
            message, missing,
            "case {case:?} after the refused {CREATE}: expected {missing:?}, got {message:?}"
        );

        let stored = held(&logs);
        assert!(
            stored.is_empty(),
            "case {case:?}: the refused create stored no query, got {stored:?}"
        );

        // The name is still free: the corrected retry under it is accepted and stores the
        // schedule it states.
        logs.ok(CREATE, &create_body(name, "cron(0 12 * * ? *)"))
            .await;
        let read = logs.ok(GET, &json!({"identifier": name})).await;
        assert_eq!(
            reported_schedule(&read),
            Some("cron(0 12 * * ? *)"),
            "case {case:?}: the corrected retry under the same name is accepted, got {read}"
        );
        logs.ok("DeleteScheduledQuery", &json!({"identifier": name}))
            .await;
    }
}

/// An update whose schedule expression does not parse is refused with the same sentence and
/// writes NOTHING: the query it addresses keeps the whole configuration it had, including the
/// schedule it runs on.
#[tokio::test]
async fn an_update_whose_schedule_does_not_parse_is_refused_and_writes_nothing() {
    let logs = Logs::new();
    const STORED: &str = "orders-errors-stored";
    logs.ok(CREATE, &create_body(STORED, "cron(0 * * * ? *)"))
        .await;
    let before = logs.ok(GET, &json!({"identifier": STORED})).await;

    let expected = schedule_refusal();
    for (case, expression) in refused() {
        let (code, message) = logs.refusal(UPDATE, &update_body(STORED, expression)).await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?} ({expression:?}) on {UPDATE}: expected {VALIDATION}, got {code} \
             {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case:?} ({expression:?}) on {UPDATE}: expected {expected:?}, got {message:?}"
        );

        let after = logs.ok(GET, &json!({"identifier": STORED})).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused update wrote nothing, expected {before}, got {after}"
        );
    }

    // The member is judged before the identifier is resolved against state, so an update that
    // states an unparsable schedule for a query the account does not hold is refused about the
    // schedule.
    let (code, message) = logs
        .refusal(UPDATE, &update_body("orders-absent", "cron(0 12 * *)"))
        .await;
    assert_eq!(
        code, VALIDATION,
        "case \"an unparsable schedule for a query that is not there\": expected {VALIDATION}, \
         got {code} {message:?}"
    );
    assert_eq!(
        message, expected,
        "case \"an unparsable schedule for a query that is not there\": expected {expected:?}, \
         got {message:?}"
    );
}

/// Every schedule expression that parses is accepted by both writes and read back byte for byte:
/// the service stores the expression the request stated rather than a normalised form of it.
#[tokio::test]
async fn a_schedule_that_parses_is_accepted_and_read_back_byte_for_byte() {
    for (index, (case, expression)) in accepted().into_iter().enumerate() {
        let logs = Logs::new();
        let name = format!("orders-schedule-{index}");

        let created = logs.ok(CREATE, &create_body(&name, expression)).await;
        assert_eq!(
            created.get("state").and_then(Value::as_str),
            Some("ENABLED"),
            "case {case:?} ({expression:?}) on {CREATE}: the create is accepted, got {created}"
        );
        let read = logs.ok(GET, &json!({"identifier": &name})).await;
        assert_eq!(
            reported_schedule(&read),
            Some(expression),
            "case {case:?}: the stored query runs on the schedule the create stated, got {read}"
        );

        // The write that re-states the whole configuration accepts the same value and stores it
        // the same way, over whatever schedule the query already ran on.
        let updated = logs.ok(UPDATE, &update_body(&name, expression)).await;
        assert_eq!(
            reported_schedule(&updated),
            Some(expression),
            "case {case:?} in the {UPDATE} response: expected {expression:?}, got {updated}"
        );
        let read = logs.ok(GET, &json!({"identifier": &name})).await;
        assert_eq!(
            reported_schedule(&read),
            Some(expression),
            "case {case:?} after {UPDATE}: expected {expression:?}, got {read}"
        );
    }
}
