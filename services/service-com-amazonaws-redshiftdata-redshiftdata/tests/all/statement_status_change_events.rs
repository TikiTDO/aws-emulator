// SPDX-License-Identifier: CC0-1.0

//! A submission that asked for an event PUBLISHES its statement's status change,
//! and the EventBridge rule that matches it delivers it to its SQS target.
//!
//! The RULE asserted here: a submit carrying `WithEvent` — "a value that indicates
//! whether to send an event to the Amazon EventBridge event bus after the SQL
//! statement runs"
//! (`smithy_query(query="doc",shape="ExecuteStatementInput",member="WithEvent")`) —
//! has the status change its statement makes when it STOPS published once onto the
//! account's default event bus under `source` `aws.redshift-data` and `detail-type`
//! `Redshift Data Statement Status Change`, naming the warehouse the statement ran
//! on in `resources` and carrying, in `detail`, the `Id` the submit answered with,
//! the `StatementName` it was given, the terminal status the record reports and the
//! principal that submitted it. A submit that asked for NO event publishes nothing
//! at all: accepting the member and emitting nothing is the defect this pins, and
//! so is emitting for a statement nobody asked about.
//!
//! The rows span what the rule generalises over rather than one recorded request:
//! whether the submission asked for an event (absent, false, true), WHICH submit
//! operation made it (one statement, a batch), whether it carried a
//! `StatementName`, WHICH terminal status the statement reaches (a statement that
//! runs FINISHES, one the engine refuses FAILS), and WHICH kind of warehouse it ran
//! on (a provisioned cluster, a serverless workgroup) — because the ARN an event
//! names the warehouse by is the owning service's, not this one's. A statement a
//! caller STOPS is the third terminal status, driven by its own test below.
//!
//! EventBridge and Amazon SQS are mounted BESIDE this service on the SAME engine,
//! the way the assembled emulator runs them, so the publication travels the real
//! in-process callback, through the real bus, to a real queue: nothing here reads
//! the publisher's own queue to decide whether delivery happened.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::emulator::{Warehouses, REGION};

/// The `source`, `detail-type` and bus the Data API's statement events travel on,
/// spelled out here rather than imported from the service crate, so a wrong
/// constant in the implementation cannot make this suite pass.
const EVENT_SOURCE: &str = "aws.redshift-data";
const DETAIL_TYPE: &str = "Redshift Data Statement Status Change";
const DEFAULT_BUS: &str = "default";

/// The warehouse every row that names a cluster runs on, and the database both
/// warehouses are created with.
const CLUSTER: &str = "statement-event-cluster";
const WORKGROUP: &str = "statement-event-workgroup";
const DATABASE: &str = "devdb";
const DB_USER: &str = "adminuser";

/// A statement the engine runs to completion, and one it refuses.
const FINISHING_SQL: &str = "SELECT 1";
const FAILING_SQL: &str = "SELECT * FROM a_table_no_statement_ever_created";

/// The statuses a statement's lifecycle ends at.
const TERMINAL_STATUSES: [&str; 3] = ["FINISHED", "FAILED", "ABORTED"];

/// How long a poll in this suite keeps looking before it gives up.
///
/// Generous beside the engine's own execution bound, which is what decides when a
/// statement stops: a poll here gives up because the emulator never produced what
/// it waited for, and never because the machine running the test was busy.
const POLL_BUDGET: Duration = Duration::from_secs(180);

/// How long to wait between two polls.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Which kind of warehouse a row's statement runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Warehouse {
    /// A provisioned cluster, named by its `ClusterIdentifier`.
    Cluster,
    /// A serverless workgroup, named by its `WorkgroupName`.
    Workgroup,
}

/// One row: the submit made, and what the bus must carry for it.
struct Row {
    case: &'static str,
    /// The submit operation: `ExecuteStatement` or `BatchExecuteStatement`.
    operation: &'static str,
    /// The statements submitted: exactly one for `ExecuteStatement`.
    sqls: &'static [&'static str],
    /// The `WithEvent` the request carries, or `None` for a request that omits the
    /// member altogether.
    with_event: Option<bool>,
    /// The `StatementName` the request carries, when it carries one.
    statement_name: Option<&'static str>,
    warehouse: Warehouse,
    /// Whether this submission must be announced on the bus.
    announced: bool,
    /// The terminal status the statement must reach.
    status: &'static str,
}

/// The table. The rows that must announce NOTHING come first, so that every later
/// row's announcement is a drain of the same queue: a quiet row that had queued an
/// event would have been published by one of them.
const ROWS: &[Row] = &[
    Row {
        case: "a statement that omits WithEvent",
        operation: "ExecuteStatement",
        sqls: &[FINISHING_SQL],
        with_event: None,
        statement_name: Some("omitted-the-member"),
        warehouse: Warehouse::Cluster,
        announced: false,
        status: "FINISHED",
    },
    Row {
        case: "a statement that asked for no event",
        operation: "ExecuteStatement",
        sqls: &[FINISHING_SQL],
        with_event: Some(false),
        statement_name: Some("asked-for-none"),
        warehouse: Warehouse::Cluster,
        announced: false,
        status: "FINISHED",
    },
    Row {
        case: "a batch that asked for no event",
        operation: "BatchExecuteStatement",
        sqls: &[FINISHING_SQL, "SELECT 2"],
        with_event: Some(false),
        statement_name: Some("quiet-batch"),
        warehouse: Warehouse::Cluster,
        announced: false,
        status: "FINISHED",
    },
    Row {
        case: "a named statement that ran",
        operation: "ExecuteStatement",
        sqls: &[FINISHING_SQL],
        with_event: Some(true),
        statement_name: Some("sim2real.completion.check"),
        warehouse: Warehouse::Cluster,
        announced: true,
        status: "FINISHED",
    },
    Row {
        case: "an unnamed statement that ran",
        operation: "ExecuteStatement",
        sqls: &[FINISHING_SQL],
        with_event: Some(true),
        statement_name: None,
        warehouse: Warehouse::Cluster,
        announced: true,
        status: "FINISHED",
    },
    Row {
        case: "a statement the engine refused",
        operation: "ExecuteStatement",
        sqls: &[FAILING_SQL],
        with_event: Some(true),
        statement_name: Some("failing-statement"),
        warehouse: Warehouse::Cluster,
        announced: true,
        status: "FAILED",
    },
    Row {
        case: "a batch that ran",
        operation: "BatchExecuteStatement",
        sqls: &[FINISHING_SQL, "SELECT 2"],
        with_event: Some(true),
        statement_name: Some("loud-batch"),
        warehouse: Warehouse::Cluster,
        announced: true,
        status: "FINISHED",
    },
    Row {
        case: "a statement on a serverless workgroup",
        operation: "ExecuteStatement",
        sqls: &[FINISHING_SQL],
        with_event: Some(true),
        statement_name: Some("workgroup-statement"),
        warehouse: Warehouse::Workgroup,
        announced: true,
        status: "FINISHED",
    },
];

/// The submit request one row makes.
fn submit_request(row: &Row) -> Value {
    let mut request = match row.warehouse {
        // A provisioned cluster, authenticated as a database user with temporary
        // credentials.
        Warehouse::Cluster => json!({
            "ClusterIdentifier": CLUSTER,
            "Database": DATABASE,
            "DbUser": DB_USER,
        }),
        // A serverless workgroup: "the database user name is derived from the IAM
        // identity", so the request names no user.
        Warehouse::Workgroup => json!({
            "WorkgroupName": WORKGROUP,
            "Database": DATABASE,
        }),
    };
    if row.operation == "BatchExecuteStatement" {
        request["Sqls"] = json!(row.sqls);
    } else {
        request["Sql"] = json!(row.sqls[0]);
    }
    if let Some(with_event) = row.with_event {
        request["WithEvent"] = json!(with_event);
    }
    if let Some(name) = row.statement_name {
        request["StatementName"] = json!(name);
    }
    request
}

/// Create the queue `name`, grant `events.amazonaws.com` the `sqs:SendMessage` a
/// rule's target needs on it, and answer its url and the ARN a rule names it by.
///
/// EventBridge makes a target's send as its own service principal and "relies on
/// resource-based policies" for a queue, so a queue that granted it nothing
/// receives nothing.
async fn given_queue(emulator: &Warehouses, name: &str) -> (String, String) {
    let created = emulator
        .sqs_ok("CreateQueue", &json!({"QueueName": name}))
        .await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("a created queue carries its url: {created}"))
        .to_string();
    let attributes = emulator
        .sqs_ok(
            "GetQueueAttributes",
            &json!({"QueueUrl": &url, "AttributeNames": ["QueueArn"]}),
        )
        .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a queue carries its ARN: {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "events.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Resource": &arn,
        }],
    })
    .to_string();
    emulator
        .sqs_ok(
            "SetQueueAttributes",
            &json!({"QueueUrl": &url, "Attributes": {"Policy": policy}}),
        )
        .await;
    (url, arn)
}

/// An ENABLED rule on the default bus carrying `pattern`, with `queue_arn` as its
/// one target.
async fn given_rule(emulator: &Warehouses, name: &str, pattern: &Value, queue_arn: &str) {
    emulator
        .events_ok(
            "PutRule",
            &json!({
                "Name": name,
                "EventBusName": DEFAULT_BUS,
                "EventPattern": pattern.to_string(),
                "State": "ENABLED",
            }),
        )
        .await;
    let targeted = emulator
        .events_ok(
            "PutTargets",
            &json!({
                "Rule": name,
                "EventBusName": DEFAULT_BUS,
                "Targets": [{"Id": "statement-events", "Arn": queue_arn}],
            }),
        )
        .await;
    assert_eq!(
        targeted["FailedEntryCount"],
        json!(0),
        "every target of {name:?} must be accepted, got {targeted}"
    );
}

/// The rule every statement event of this service matches.
fn statement_event_pattern() -> Value {
    json!({"source": [EVENT_SOURCE], "detail-type": [DETAIL_TYPE]})
}

/// Every envelope now waiting on the queue at `url`.
async fn drain(emulator: &Warehouses, url: &str) -> Vec<Value> {
    let received = emulator
        .sqs_ok(
            "ReceiveMessage",
            &json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
        )
        .await;
    received["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            serde_json::from_str::<Value>(message["Body"].as_str().unwrap_or_default())
                .unwrap_or(Value::Null)
        })
        .collect()
}

/// Collect envelopes off the queue at `url` until `wanted` have arrived or
/// [`POLL_BUDGET`] passes.
async fn collect(emulator: &Warehouses, url: &str, wanted: usize) -> Vec<Value> {
    let started = Instant::now();
    let mut arrived: Vec<Value> = Vec::new();
    while arrived.len() < wanted && started.elapsed() < POLL_BUDGET {
        arrived.extend(drain(emulator, url).await);
        if arrived.len() < wanted {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    arrived
}

/// Poll `id` until it reports a terminal status, and answer the record that
/// reported it.
async fn wait_for_terminal(emulator: &Warehouses, case: &str, id: &str) -> Value {
    let started = Instant::now();
    loop {
        let described = emulator
            .data_ok("DescribeStatement", &json!({"Id": id}))
            .await;
        let status = described["Status"].as_str().unwrap_or_default().to_string();
        if TERMINAL_STATUSES.contains(&status.as_str()) {
            return described;
        }
        assert!(
            started.elapsed() < POLL_BUDGET,
            "case {case:?}: {id} reported {status:?} and never stopped: {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The one envelope of `delivered` that is about `statement_id`.
fn envelope_for<'a>(case: &str, delivered: &'a [Value], statement_id: &str) -> &'a Value {
    let mine: Vec<&Value> = delivered
        .iter()
        .filter(|envelope| envelope.pointer("/detail/statementId") == Some(&json!(statement_id)))
        .collect();
    assert_eq!(
        mine.len(),
        1,
        "case {case:?}: expected exactly one announcement of statement {statement_id}, got \
         {mine:?} among {delivered:?}"
    );
    mine[0]
}

/// Assert the whole envelope of one announced row against the record the Data API
/// reports for the same statement.
fn assert_envelope(
    row: &Row,
    envelope: &Value,
    described: &Value,
    statement_id: &str,
    warehouse_arn: &str,
) {
    let case = row.case;
    for (pointer, expected) in [
        ("/source", json!(EVENT_SOURCE)),
        ("/detail-type", json!(DETAIL_TYPE)),
        ("/account", json!(engine::EMULATOR_ACCOUNT_ID)),
        ("/region", json!(REGION)),
        ("/resources", json!([warehouse_arn])),
        ("/detail/statementId", json!(statement_id)),
        // The announced state is the state the API reports for the same
        // statement: one record, read two ways.
        ("/detail/state", described["Status"].clone()),
    ] {
        let actual = envelope.pointer(pointer).cloned().unwrap_or(Value::Null);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {pointer} to be {expected}, got {actual} in {envelope}"
        );
    }
    assert_eq!(
        envelope.pointer("/detail/state"),
        Some(&json!(row.status)),
        "case {case:?}: expected the announced state {:?}, got {envelope}",
        row.status
    );

    // The name the submission gave the statement, and nothing at all for a
    // submission that gave it none.
    let announced_name = envelope.pointer("/detail/statementName");
    match row.statement_name {
        Some(name) => assert_eq!(
            announced_name,
            Some(&json!(name)),
            "case {case:?}: expected the submitted statement name {name:?}, got {envelope}"
        ),
        None => assert!(
            announced_name.is_none(),
            "case {case:?}: a submission that named nothing must announce no name, got {envelope}"
        ),
    }

    // The principal that submitted it: a non-empty string naming an identity.
    let principal = envelope
        .pointer("/detail/principal")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !principal.is_empty() && principal.starts_with("arn:"),
        "case {case:?}: expected a non-empty principal ARN, got {principal:?} in {envelope}"
    );

    // The reason it stopped, for a statement that stopped without finishing —
    // exactly what the record reports, and nothing for one that finished.
    let announced_error = envelope
        .pointer("/detail/errorMessage")
        .and_then(Value::as_str);
    match described["Error"].as_str() {
        Some(error) => {
            assert_eq!(
                announced_error,
                Some(error),
                "case {case:?}: expected the record's own reason {error:?}, got {envelope}"
            );
            assert!(
                !error.is_empty(),
                "case {case:?}: a statement that failed reports why: {described}"
            );
        }
        None => assert!(
            announced_error.is_none(),
            "case {case:?}: a statement that finished announces no reason, got {envelope}"
        ),
    }
}

/// RULE: every submission that asked for an event is announced once on the default
/// bus, with the envelope and `detail` above, and delivered to the rule that
/// matches it — and a submission that asked for none is announced not at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_with_event_submission_is_announced_and_delivered_to_its_sqs_target() {
    let emulator = Warehouses::new();
    emulator
        .create_cluster_with_database(CLUSTER, Some(DATABASE))
        .await;
    let workgroup_arn = emulator
        .create_workgroup_with_database(WORKGROUP, Some(DATABASE))
        .await;
    let cluster_arn = format!(
        "arn:aws:redshift:{REGION}:{}:cluster:{CLUSTER}",
        engine::EMULATOR_ACCOUNT_ID
    );

    // One queue behind a rule matching every statement event of this service, and
    // one behind a rule that cannot match anything it publishes.
    let (matching_url, matching_arn) = given_queue(&emulator, "statement-events").await;
    let (other_url, other_arn) = given_queue(&emulator, "unrelated-events").await;
    given_rule(
        &emulator,
        "statement-status-changes",
        &statement_event_pattern(),
        &matching_arn,
    )
    .await;
    given_rule(
        &emulator,
        "another-services-events",
        &json!({"source": ["aws.ecs"], "detail-type": ["ECS Task State Change"]}),
        &other_arn,
    )
    .await;

    // Every row is submitted with NO wait at all, which is the path a client that
    // relies on the event takes: nothing here waits for the statement inside its
    // own submit request.
    let mut submitted: Vec<(&Row, String)> = Vec::new();
    for row in ROWS {
        let answered = emulator
            .data_ok(row.operation, &submit_request(row))
            .await;
        let id = answered["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no Id in {answered}", row.case))
            .to_string();
        submitted.push((row, id));
    }

    // Each statement reaches the terminal status its row states, read back through
    // the reader a polling client uses.
    let mut described: Vec<Value> = Vec::new();
    for (row, id) in &submitted {
        let record = wait_for_terminal(&emulator, row.case, id).await;
        assert_eq!(
            record["Status"],
            json!(row.status),
            "case {:?}: expected the statement to end {}, got {record}",
            row.case,
            row.status
        );
        described.push(record);
    }

    let wanted = ROWS.iter().filter(|row| row.announced).count();
    let delivered = collect(&emulator, &matching_url, wanted).await;
    assert_eq!(
        delivered.len(),
        wanted,
        "expected {wanted} announcements on the matching rule's queue, got {delivered:?}"
    );

    for ((row, id), record) in submitted.iter().zip(&described) {
        if !row.announced {
            let quiet: Vec<&Value> = delivered
                .iter()
                .filter(|envelope| envelope.pointer("/detail/statementId") == Some(&json!(id)))
                .collect();
            assert!(
                quiet.is_empty(),
                "case {:?}: a submission that asked for no event must announce nothing, got \
                 {quiet:?}",
                row.case
            );
            continue;
        }
        let warehouse_arn = match row.warehouse {
            Warehouse::Cluster => cluster_arn.as_str(),
            Warehouse::Workgroup => workgroup_arn.as_str(),
        };
        let envelope = envelope_for(row.case, &delivered, id);
        assert_envelope(row, envelope, record, id, warehouse_arn);
    }

    // The rule that cannot match this service's events received none of them.
    let withheld = drain(&emulator, &other_url).await;
    assert!(
        withheld.is_empty(),
        "a rule for another service's events must receive none of these, got {withheld:?}"
    );

    // And nothing is left waiting: every queued status change was published by the
    // execution that produced it, so the quiet rows above queued nothing at all.
    let queued = emulator.store().lock().pending_statement_event_count();
    assert_eq!(
        queued, 0,
        "every queued status change must have been published, got {queued} waiting"
    );
}

/// RULE, third terminal status: a statement a caller STOPS is a status change too,
/// so a submission that asked for an event is told that its statement was aborted —
/// with the backend's own reason, exactly as the record reports it.
///
/// The statement is submitted while the turn on its database is HELD, so it is
/// still waiting for its turn when the cancel reaches it: the window in which it
/// can be stopped is closed by this test dropping the guard and never by how fast
/// the host ran the queue.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_with_event_statement_announces_that_it_was_aborted() {
    let case = "a statement a caller stopped";
    let emulator = Warehouses::new();
    emulator
        .create_cluster_with_database(CLUSTER, Some(DATABASE))
        .await;
    let cluster_arn = format!(
        "arn:aws:redshift:{REGION}:{}:cluster:{CLUSTER}",
        engine::EMULATOR_ACCOUNT_ID
    );

    let (url, queue_arn) = given_queue(&emulator, "aborted-statement-events").await;
    given_rule(
        &emulator,
        "statement-status-changes",
        &statement_event_pattern(),
        &queue_arn,
    )
    .await;

    // Nothing this statement submits can run while this guard lives.
    let held = emulator.hold_turn(CLUSTER, DATABASE);
    let answered = emulator
        .data_ok(
            "ExecuteStatement",
            &json!({
                "ClusterIdentifier": CLUSTER,
                "Database": DATABASE,
                "DbUser": DB_USER,
                "Sql": FINISHING_SQL,
                "WithEvent": true,
                "StatementName": "cancelled-statement",
            }),
        )
        .await;
    let id = answered["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no Id in {answered}"))
        .to_string();
    assert_eq!(
        answered["Status"],
        json!("SUBMITTED"),
        "case {case:?}: a statement waiting for its turn is SUBMITTED: {answered}"
    );

    let cancelled = emulator
        .data_ok("CancelStatement", &json!({"Id": &id}))
        .await;
    assert_eq!(
        cancelled["Status"],
        json!(true),
        "case {case:?}: cancelling a statement that has not stopped answers that it was stopped: \
         {cancelled}"
    );
    drop(held);

    let record = wait_for_terminal(&emulator, case, &id).await;
    assert_eq!(
        record["Status"],
        json!("ABORTED"),
        "case {case:?}: a cancelled statement lands ABORTED: {record}"
    );

    let delivered = collect(&emulator, &url, 1).await;
    let envelope = envelope_for(case, &delivered, &id);
    let row = Row {
        case,
        operation: "ExecuteStatement",
        sqls: &[],
        with_event: Some(true),
        statement_name: Some("cancelled-statement"),
        warehouse: Warehouse::Cluster,
        announced: true,
        status: "ABORTED",
    };
    assert_envelope(&row, envelope, &record, &id, &cluster_arn);
}
