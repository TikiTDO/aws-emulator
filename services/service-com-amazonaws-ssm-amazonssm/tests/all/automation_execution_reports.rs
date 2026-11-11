// SPDX-License-Identifier: CC0-1.0

//! `DescribeAutomationExecutions` end to end: the automation executions
//! `StartAutomationExecution` persisted in one Region, reported with what the start and the
//! walk made of each of them, narrowed by the request's filters and served a page at a
//! time.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol").
//!
//! An automation is `Pending` until the service picks it up, so the cases that look past
//! that instant move the clock through the service's own state ([`pick_up`]) rather than
//! sleeping for it.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — which filter key narrows the report, how many values one filter
//! carries, how many filters narrow together, the page size, the Region the report is read
//! in — rather than the single report the recorded unit happened to make.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ssm_amazonssm::ops::types::Target;
use service_com_amazonaws_ssm_amazonssm::state::ComAmazonawsSsmAmazonssmState;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise. Deliberately not the
/// emulator's default, so a report served here proves the Region came from the request's
/// SigV4 credential scope.
const REGION: &str = "us-west-2";

/// A second Region, to pin that an execution is reported only through the Region its start
/// was sent to.
const OTHER_REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// A token shaped like one of this service's minted ids that names no execution.
const UNKNOWN_TOKEN: &str = "99999999-8888-7777-6666-555555555555";

/// Every `AutomationExecutionFilterKey` the model declares, from
/// `smithy_query(query="shape",shape="AutomationExecutionFilterKey")`. The filter table
/// below must exercise every one of them.
const FILTER_KEYS: [&str; 12] = [
    "DocumentNamePrefix",
    "ExecutionStatus",
    "ExecutionId",
    "ParentExecutionId",
    "CurrentAction",
    "StartTimeBefore",
    "StartTimeAfter",
    "AutomationType",
    "TagKey",
    "TargetResourceGroup",
    "AutomationSubtype",
    "OpsItemId",
];

/// The bounds the modelled `MaxResults` of this report carries (range 1..50), from
/// `smithy_query(query="operation",operation="DescribeAutomationExecutions")`.
const MIN_MAX_RESULTS: i64 = 1;

/// See [`MIN_MAX_RESULTS`].
const MAX_MAX_RESULTS: i64 = 50;

/// A router with just this service mounted, and the engine it was mounted on — the handle
/// a case reads the store through and moves the clock with.
fn service() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where the Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Call `operation` signed for `region`, returning `(status, body)`.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Call `operation` signed for [`REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// The refusal message of a failure body.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// A string member of a response body.
fn text(body: &Value, member: &str) -> String {
    body[member].as_str().unwrap_or("<unset>").to_string()
}

/// Create an Automation runbook of `name` with `content` in `region`, asserting it
/// succeeded.
async fn runbook_in_region(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    content: &Value,
) {
    let (status, created) = call_in_region(
        router,
        region,
        "CreateDocument",
        &json!({
            "Name": name,
            "DocumentType": "Automation",
            "Content": content.to_string(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the runbook {name} must be created first in {region}, got {status} with {created:?}"
    );
}

/// Create an Automation runbook in [`REGION`].
async fn runbook(router: &MultiServiceRouter, name: &str, content: &Value) {
    runbook_in_region(router, REGION, name, content).await
}

/// Start an automation in `region`, asserting it succeeded, and answering the id it minted.
async fn started_in_region(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    body: &Value,
) -> String {
    let (status, response) = call_in_region(router, region, "StartAutomationExecution", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from StartAutomationExecution, got {status} with \
         {response:?}"
    );
    text(&response, "AutomationExecutionId")
}

/// Start an automation in [`REGION`].
async fn started(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    started_in_region(router, REGION, case, body).await
}

/// An instant well in the past, which is what the clock is moved to below.
fn long_ago() -> SystemTime {
    SystemTime::now() - Duration::from_secs(3_600)
}

/// Let the clock reach the pickup every execution of `region` is waiting for.
///
/// The service owes a started automation ONE transition, a span after the start
/// (`automation::PICKUP_DELAY`); a case that asserts what the walk reported moves the clock
/// to it through the store rather than sleeping for it.
fn pick_up(engine: &Arc<engine::InMemoryEngine>, region: &str) {
    let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
    let mut store = state.lock();
    for ((held, _), execution) in store.automation_executions.iter_mut() {
        if held == region && execution.runnable_at.is_some() {
            execution.runnable_at = Some(long_ago());
        }
    }
}

/// Move the instant `execution_id` started at to `at`, so a case can rank two executions
/// that this test started within the one instant.
fn back_date(engine: &Arc<engine::InMemoryEngine>, execution_id: &str, at: SystemTime) {
    let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
    let mut store = state.lock();
    let execution = store
        .automation_execution_mut(REGION, execution_id)
        .unwrap_or_else(|| panic!("the execution {execution_id} must be held"));
    execution.started_at = at;
}

/// The ids of the executions the rate-controlled start `execution_id` released, read out of
/// the store so a case can name them without depending on the report under test.
fn children_of(engine: &Arc<engine::InMemoryEngine>, execution_id: &str) -> Vec<String> {
    let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
    let store = state.lock();
    store
        .automation_execution(REGION, execution_id)
        .map(|execution| execution.children.clone())
        .unwrap_or_default()
}

/// Record a resource-group target on `execution_id`, spelled with the target `key` the case
/// picks, the way a start that named one would have recorded it.
///
/// A resource group is a resource of another service and nothing in this emulator registers
/// one, so a START naming one resolves no resource and is refused (`InvalidTarget`). The
/// stored target the `TargetResourceGroup` filter reads is therefore written through the
/// store — the same place the handler reads it from.
fn target_resource_group(
    engine: &Arc<engine::InMemoryEngine>,
    execution_id: &str,
    key: &str,
    group: &str,
) {
    let state = engine.state::<ComAmazonawsSsmAmazonssmState>();
    let mut store = state.lock();
    let execution = store
        .automation_execution_mut(REGION, execution_id)
        .unwrap_or_else(|| panic!("the execution {execution_id} must be held"));
    execution.targets = vec![Target {
        key: Some(key.to_string()),
        values: Some(vec![group.to_string()]),
    }];
}

/// `at` in the ISO 8601 spelling a timestamp filter value carries.
fn iso8601(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant renders")
}

/// Report the Region's automation executions with whatever `body` carries.
async fn describe(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "DescribeAutomationExecutions", body).await
}

/// Report them, asserting the report succeeded.
async fn reported(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = describe(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeAutomationExecutions, got {status} with \
         {response:?}"
    );
    response
}

/// The metadata entries a report carries, in the order it carried them.
fn entries(report: &Value) -> Vec<Value> {
    report["AutomationExecutionMetadataList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The execution ids a report carries, in the order it carried them.
fn ids(report: &Value) -> Vec<String> {
    entries(report)
        .iter()
        .map(|entry| text(entry, "AutomationExecutionId"))
        .collect()
}

/// The execution ids a report carries, as a set — what a case asserting WHICH executions a
/// filter selects compares, since the filter is not what orders the report.
fn id_set(report: &Value) -> BTreeSet<String> {
    ids(report).into_iter().collect()
}

/// A runbook that pauses, so the walk leaves the execution waiting on a step that has
/// started and not ended.
fn pausing_runbook() -> Value {
    json!({
        "schemaVersion": "0.3",
        "description": "Pauses and waits for a signal.",
        "mainSteps": [{"name": "hold", "action": "aws:pause", "inputs": {}}],
    })
}

/// A runbook of one step that ends at once, with the parameter a rate-controlled start
/// binds each of its target's values to.
fn sleeping_runbook() -> Value {
    json!({
        "schemaVersion": "0.3",
        "description": "Ends at once.",
        "parameters": {
            "Value": {"type": "String", "default": "none"},
        },
        "mainSteps": [
            {"name": "only", "action": "aws:sleep", "inputs": {"Duration": "PT0S"}, "isEnd": true},
        ],
    })
}

/// The world the filter table narrows: five starts of two runbooks, one of them
/// rate-controlled over two target values and one of them spanning other Regions and
/// accounts, picked up so the walk has run — plus one start made after the pickup, which is
/// therefore still waiting to be picked up.
struct World {
    /// The paused execution: `Waiting`, on the `aws:pause` step, carrying one tag.
    paused: String,
    /// A plain execution of the other runbook, which ran to the end.
    plain: String,
    /// The rate-controlled parent, and the children it released.
    parent: String,
    children: Vec<String>,
    /// An execution the start pointed at other Regions and accounts.
    cross_account: String,
    /// A start made after the pickup, so nothing has picked it up yet.
    pending: String,
}

impl World {
    /// Every execution id this world holds.
    fn every_id(&self) -> BTreeSet<String> {
        let mut held: BTreeSet<String> = [
            self.paused.clone(),
            self.plain.clone(),
            self.parent.clone(),
            self.cross_account.clone(),
            self.pending.clone(),
        ]
        .into_iter()
        .collect();
        held.extend(self.children.iter().cloned());
        held
    }

    /// Every execution of the sleeping runbook: the plain start, the rate-controlled parent
    /// and its children, the cross-account start and the pending one.
    fn sleeping_ids(&self) -> BTreeSet<String> {
        let mut held = self.every_id();
        held.remove(&self.paused);
        held
    }
}

/// Build [`World`] on a fresh service.
async fn world(router: &MultiServiceRouter, engine: &Arc<engine::InMemoryEngine>) -> World {
    runbook(router, "Alpha-Pause", &pausing_runbook()).await;
    runbook(router, "Beta-Sleep", &sleeping_runbook()).await;
    let paused = started(
        router,
        "a paused start",
        &json!({
            "DocumentName": "Alpha-Pause",
            "Tags": [{"Key": "environment", "Value": "test"}],
        }),
    )
    .await;
    let plain = started(router, "a plain start", &json!({"DocumentName": "Beta-Sleep"})).await;
    let parent = started(
        router,
        "a rate-controlled start",
        &json!({
            "DocumentName": "Beta-Sleep",
            "TargetParameterName": "Value",
            "Targets": [{"Key": "ParameterValues", "Values": ["left", "right"]}],
        }),
    )
    .await;
    let cross_account = started(
        router,
        "a cross-account start",
        &json!({
            "DocumentName": "Beta-Sleep",
            "TargetLocations": [{"Accounts": ["123456789012"], "Regions": ["eu-west-1"]}],
        }),
    )
    .await;
    pick_up(engine, REGION);
    // Started AFTER the pickup, so the service still owes it one: this is the execution the
    // `Pending` rows below select.
    let pending = started(router, "a start nothing picked up", &json!({"DocumentName": "Beta-Sleep"}))
        .await;
    let children = children_of(engine, &parent);
    assert_eq!(
        children.len(),
        2,
        "the rate-controlled start must have released one execution per target value, got \
         {children:?}"
    );
    World {
        paused,
        plain,
        parent,
        children,
        cross_account,
        pending,
    }
}

/// Every execution the Region holds is reported with what the start and the walk made of
/// it, and every member this report shares with `GetAutomationExecution` agrees with it —
/// one stored execution, two readers.
#[tokio::test]
async fn every_execution_is_reported_and_agrees_with_the_get() {
    let (router, engine) = service();
    let held = world(&router, &engine).await;

    let report = reported(&router, "every execution", &json!({})).await;
    assert_eq!(
        id_set(&report),
        held.every_id(),
        "case \"every execution\": expected every execution the Region holds, got {report}"
    );
    assert!(
        report["NextToken"].is_null(),
        "case \"every execution\": expected no token once the whole result fits one page, got \
         {report}"
    );

    // The members both readers carry, so a client that polls either sees the one execution.
    let shared = [
        "AutomationExecutionId",
        "DocumentName",
        "DocumentVersion",
        "AutomationExecutionStatus",
        "Mode",
        "ExecutedBy",
        "MaxConcurrency",
        "MaxErrors",
        "CurrentStepName",
        "CurrentAction",
        "ParentAutomationExecutionId",
        "Target",
        "TargetParameterName",
        "ExecutionStartTime",
        "ExecutionEndTime",
    ];
    for entry in entries(&report) {
        let id = text(&entry, "AutomationExecutionId");
        let (status, got) = call(
            &router,
            "GetAutomationExecution",
            &json!({"AutomationExecutionId": id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {id:?}: expected HTTP 200 from GetAutomationExecution, got {status} with {got:?}"
        );
        let execution = got["AutomationExecution"].clone();
        for member in shared {
            assert_eq!(
                entry[member], execution[member],
                "case {id:?}: expected the report's {member} to agree with \
                 GetAutomationExecution's, got {:?} and {:?}",
                entry[member], execution[member]
            );
        }
        // Every execution of this world runs where it was started, except the one whose
        // start named other Regions and accounts.
        let expected_type = if id == held.cross_account {
            "CrossAccount"
        } else {
            "Local"
        };
        assert_eq!(
            text(&entry, "AutomationType"),
            expected_type,
            "case {id:?}: expected the AutomationType the start's locations imply, got {entry}"
        );
    }

    let paused = entries(&report)
        .into_iter()
        .find(|entry| text(entry, "AutomationExecutionId") == held.paused)
        .expect("the paused execution is reported");
    assert_eq!(
        (
            text(&paused, "AutomationExecutionStatus"),
            text(&paused, "CurrentStepName"),
            text(&paused, "CurrentAction"),
            text(&paused, "DocumentName"),
        ),
        (
            "Waiting".to_string(),
            "hold".to_string(),
            "aws:pause".to_string(),
            "Alpha-Pause".to_string(),
        ),
        "case \"the paused execution\": expected the step the walk is waiting on, got {paused}"
    );
}

/// The report ranks the most recently started execution first, whatever order the store
/// holds them in.
#[tokio::test]
async fn the_report_ranks_the_most_recently_started_first() {
    let (router, engine) = service();
    runbook(&router, "Beta-Sleep", &sleeping_runbook()).await;
    let mut started_ids = Vec::new();
    for case in ["first", "second", "third"] {
        started_ids.push(started(&router, case, &json!({"DocumentName": "Beta-Sleep"})).await);
    }
    // Three starts made within one instant cannot be ranked by their start times, so each
    // is moved to an instant of its own, newest last.
    let now = SystemTime::now();
    for (position, id) in started_ids.iter().enumerate() {
        back_date(
            &engine,
            id,
            now - Duration::from_secs(30 - 10 * position as u64),
        );
    }

    let report = reported(&router, "ranked", &json!({})).await;
    let expected: Vec<String> = started_ids.iter().rev().cloned().collect();
    assert_eq!(
        ids(&report),
        expected,
        "case \"ranked\": expected the most recently started first, got {report}"
    );
}

/// A filter selects the executions its key names, at any of the values it carries, and two
/// filters narrow together.
#[tokio::test]
async fn a_filter_selects_the_executions_its_key_names() {
    let (router, engine) = service();
    let held = world(&router, &engine).await;
    let every = held.every_id();
    let children: BTreeSet<String> = held.children.iter().cloned().collect();
    let nothing: BTreeSet<String> = BTreeSet::new();
    let one = |id: &String| -> BTreeSet<String> { [id.clone()].into_iter().collect() };
    let ahead = iso8601(SystemTime::now() + Duration::from_secs(3_600));
    let behind = iso8601(long_ago());

    let rows: Vec<(&str, Value, BTreeSet<String>)> = vec![
        // DocumentNamePrefix: the executions of the runbooks whose name starts with it.
        (
            "a prefix of one runbook's name",
            json!([{"Key": "DocumentNamePrefix", "Values": ["Alpha"]}]),
            one(&held.paused),
        ),
        (
            "a prefix of the other runbook's name",
            json!([{"Key": "DocumentNamePrefix", "Values": ["Beta"]}]),
            held.sleeping_ids(),
        ),
        (
            "a whole runbook name as the prefix",
            json!([{"Key": "DocumentNamePrefix", "Values": ["Alpha-Pause"]}]),
            one(&held.paused),
        ),
        (
            "a prefix no runbook starts with",
            json!([{"Key": "DocumentNamePrefix", "Values": ["Gamma"]}]),
            nothing.clone(),
        ),
        (
            "either runbook's prefix in one filter",
            json!([{"Key": "DocumentNamePrefix", "Values": ["Alpha", "Beta"]}]),
            every.clone(),
        ),
        // ExecutionStatus: the executions an AutomationExecutionStatus holds for.
        (
            "the status of the paused execution",
            json!([{"Key": "ExecutionStatus", "Values": ["Waiting"]}]),
            one(&held.paused),
        ),
        (
            "the status of an execution nothing picked up",
            json!([{"Key": "ExecutionStatus", "Values": ["Pending"]}]),
            one(&held.pending),
        ),
        (
            "two statuses in one filter",
            json!([{"Key": "ExecutionStatus", "Values": ["Waiting", "Pending"]}]),
            [held.paused.clone(), held.pending.clone()]
                .into_iter()
                .collect(),
        ),
        (
            "a status no execution of this Region is in",
            json!([{"Key": "ExecutionStatus", "Values": ["TimedOut"]}]),
            nothing.clone(),
        ),
        // ExecutionId: one execution by its own id.
        (
            "one execution by its id",
            json!([{"Key": "ExecutionId", "Values": [held.plain]}]),
            one(&held.plain),
        ),
        (
            "two executions by their ids",
            json!([{"Key": "ExecutionId", "Values": [held.plain, held.paused]}]),
            [held.plain.clone(), held.paused.clone()]
                .into_iter()
                .collect(),
        ),
        (
            "an id no execution was minted under",
            json!([{"Key": "ExecutionId", "Values": [UNKNOWN_TOKEN]}]),
            nothing.clone(),
        ),
        // ParentExecutionId: the children one rate-controlled start released.
        (
            "the children of the rate-controlled start",
            json!([{"Key": "ParentExecutionId", "Values": [held.parent]}]),
            children.clone(),
        ),
        (
            "the children of an execution that released none",
            json!([{"Key": "ParentExecutionId", "Values": [held.plain]}]),
            nothing.clone(),
        ),
        // CurrentAction: the executions whose running step runs that action.
        (
            "the action the paused execution is waiting on",
            json!([{"Key": "CurrentAction", "Values": ["aws:pause"]}]),
            one(&held.paused),
        ),
        (
            "an action no execution is running",
            json!([{"Key": "CurrentAction", "Values": ["aws:sleep"]}]),
            nothing.clone(),
        ),
        // The two instant keys, read against bounds either side of every start.
        (
            "started before an instant every start is behind",
            json!([{"Key": "StartTimeBefore", "Values": [ahead]}]),
            every.clone(),
        ),
        (
            "started before an instant every start is ahead of",
            json!([{"Key": "StartTimeBefore", "Values": [behind]}]),
            nothing.clone(),
        ),
        (
            "started after an instant every start is ahead of",
            json!([{"Key": "StartTimeAfter", "Values": [behind]}]),
            every.clone(),
        ),
        (
            "started after an instant every start is behind",
            json!([{"Key": "StartTimeAfter", "Values": [ahead]}]),
            nothing.clone(),
        ),
        // AutomationType: whether the start pointed at other Regions and accounts.
        (
            "the executions that run where they were started",
            json!([{"Key": "AutomationType", "Values": ["Local"]}]),
            every
                .iter()
                .filter(|id| *id != &held.cross_account)
                .cloned()
                .collect(),
        ),
        (
            "the execution that spans Regions and accounts",
            json!([{"Key": "AutomationType", "Values": ["CrossAccount"]}]),
            one(&held.cross_account),
        ),
        // TagKey: the executions carrying a tag of that key, whatever value it holds.
        (
            "the key of a tag one start carried",
            json!([{"Key": "TagKey", "Values": ["environment"]}]),
            one(&held.paused),
        ),
        (
            "a tag key no start carried",
            json!([{"Key": "TagKey", "Values": ["owner"]}]),
            nothing.clone(),
        ),
        // A resource group is a resource of another service and nothing registers one here,
        // so no execution of this Region targets one.
        (
            "a resource group no start targeted",
            json!([{"Key": "TargetResourceGroup", "Values": ["production"]}]),
            nothing.clone(),
        ),
        // The Change Manager keys filter members only a change request carries, and a start
        // through this operation is not one.
        (
            "the subtype of a change request",
            json!([{"Key": "AutomationSubtype", "Values": ["ChangeRequest"]}]),
            nothing.clone(),
        ),
        (
            "the OpsItem of a change request",
            json!([{"Key": "OpsItemId", "Values": ["oi-0123456789ab"]}]),
            nothing.clone(),
        ),
        // Two filters narrow TOGETHER: each is a criterion the execution has to meet.
        (
            "a runbook prefix and a status together",
            json!([
                {"Key": "DocumentNamePrefix", "Values": ["Beta"]},
                {"Key": "ExecutionStatus", "Values": ["Pending"]},
            ]),
            one(&held.pending),
        ),
        (
            "a runbook prefix and a status that cannot hold together",
            json!([
                {"Key": "DocumentNamePrefix", "Values": ["Alpha"]},
                {"Key": "ExecutionStatus", "Values": ["Pending"]},
            ]),
            nothing.clone(),
        ),
        (
            "a type, a prefix and an id together",
            json!([
                {"Key": "AutomationType", "Values": ["Local"]},
                {"Key": "DocumentNamePrefix", "Values": ["Beta"]},
                {"Key": "ExecutionId", "Values": [held.plain]},
            ]),
            one(&held.plain),
        ),
    ];

    let mut exercised: BTreeSet<String> = BTreeSet::new();
    for (case, filters, expected) in rows {
        for filter in filters.as_array().unwrap_or(&Vec::new()) {
            exercised.insert(text(filter, "Key"));
        }
        let report = reported(&router, case, &json!({"Filters": filters})).await;
        assert_eq!(
            id_set(&report),
            expected,
            "case {case:?}: expected the executions {expected:?}, got {report}"
        );
    }
    let every_key: BTreeSet<String> = FILTER_KEYS.iter().map(|key| key.to_string()).collect();
    assert_eq!(
        exercised, every_key,
        "the table must narrow by every modelled filter key, missing {:?}",
        every_key.difference(&exercised).collect::<Vec<_>>()
    );
}

/// RULE: the `TargetResourceGroup` key selects the executions whose stored target names that
/// resource group, in either spelling the `Target` doc gives for one, and no execution whose
/// target names another group or no group at all.
#[tokio::test]
async fn the_resource_group_filter_selects_the_execution_whose_target_names_it() {
    // The two spellings of one selection, from `Target`'s own doc:
    // `Key=ResourceGroup,Values=resource-group-name` and the Run Command form
    // `Key=resource-groups:Name,Values=resource-group-name`.
    let spellings = [
        ("the Automation spelling", "ResourceGroup"),
        ("the Run Command spelling", "resource-groups:Name"),
    ];
    for (case, key) in spellings {
        let (router, engine) = service();
        runbook(&router, "Beta-Sleep", &sleeping_runbook()).await;
        let targeted = started(&router, case, &json!({"DocumentName": "Beta-Sleep"})).await;
        let untargeted = started(&router, case, &json!({"DocumentName": "Beta-Sleep"})).await;
        target_resource_group(&engine, &targeted, key, "production");

        let group_filter = |group: &str| {
            json!({"Filters": [{"Key": "TargetResourceGroup", "Values": [group]}]})
        };
        let selected = reported(&router, case, &group_filter("production")).await;
        assert_eq!(
            id_set(&selected),
            [targeted.clone()].into_iter().collect::<BTreeSet<String>>(),
            "case {case:?}: expected only the execution whose target names that group, got \
             {selected}"
        );
        let other = reported(&router, case, &group_filter("staging")).await;
        assert!(
            ids(&other).is_empty(),
            "case {case:?}: expected no execution for a group none targets, got {other}"
        );
        // The execution that targets nothing is still held: the filter narrowed the report
        // rather than the store.
        let every = reported(&router, case, &json!({})).await;
        assert_eq!(
            id_set(&every),
            [targeted, untargeted].into_iter().collect::<BTreeSet<String>>(),
            "case {case:?}: expected both executions when nothing narrows the report, got \
             {every}"
        );
    }
}

/// `MaxResults` bounds one page and the token resumes the next, so paging through a report
/// reads every execution exactly once however small the page.
#[tokio::test]
async fn a_page_is_bounded_and_the_token_resumes_the_rest() {
    let (router, engine) = service();
    let held = world(&router, &engine).await;
    let every = held.every_id();

    for page_size in [1usize, 2, 3, every.len(), every.len() + 1] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({"MaxResults": page_size});
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let report = reported(&router, &case, &body).await;
            let page = ids(&report);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} entries in a page, got {report}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= every.len() + 1,
                "case {case:?}: expected the paging to end, got {pages} pages and {seen:?}"
            );
            match report["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            (unique.clone(), seen.len()),
            (every.clone(), every.len()),
            "case {case:?}: expected every execution exactly once across the pages, got {seen:?}"
        );
    }
}

/// The modelled bounds of `MaxResults` are the ones this report serves: a page size inside
/// the range is served, one outside it is refused.
#[tokio::test]
async fn max_results_is_held_to_its_modelled_range() {
    let (router, _engine) = service();
    runbook(&router, "Beta-Sleep", &sleeping_runbook()).await;
    started(&router, "one start", &json!({"DocumentName": "Beta-Sleep"})).await;

    for requested in [MIN_MAX_RESULTS, MAX_MAX_RESULTS] {
        let (status, response) = describe(&router, &json!({"MaxResults": requested})).await;
        assert_eq!(
            status, 200,
            "case {requested:?}: expected a page size inside the modelled range to be served, \
             got {status} with {response:?}"
        );
    }
    for requested in [MIN_MAX_RESULTS - 1, MAX_MAX_RESULTS + 1] {
        let (status, response) = describe(&router, &json!({"MaxResults": requested})).await;
        assert_eq!(
            status, 400,
            "case {requested:?}: expected a page size outside the modelled range to be refused, \
             got {status} with {response:?}"
        );
    }
}

/// A token this report never handed out names no page of it, so it is refused as invalid
/// rather than served from the start.
#[tokio::test]
async fn a_token_this_report_did_not_hand_out_is_refused() {
    let (router, engine) = service();
    let held = world(&router, &engine).await;

    // A token shaped like one of this store's minted ids, and one that is not shaped like
    // one at all: neither names a page of this result.
    for token in [UNKNOWN_TOKEN, "not-a-token"] {
        let (status, response) = describe(&router, &json!({"NextToken": token})).await;
        assert_eq!(
            (status, error_code(&response)),
            (400, "InvalidNextToken".to_string()),
            "case {token:?}: expected the token to be refused, got {status} with {response:?}"
        );
    }
    // The empty token is no token at all: the report starts at the beginning.
    let report = reported(&router, "an empty token", &json!({"NextToken": ""})).await;
    assert_eq!(
        id_set(&report),
        held.every_id(),
        "case \"an empty token\": expected the whole result, got {report}"
    );
}

/// A filter value has to be a legal value of the member its key filters: one that is not is
/// refused with `InvalidFilterValue`, and one that is — even a value no execution carries —
/// is served.
#[tokio::test]
async fn a_filter_value_outside_its_key_is_refused() {
    let (router, engine) = service();
    world(&router, &engine).await;

    // The keys that carry a value set: the two instant keys take a timestamp, and the three
    // enum keys take the modelled enum of the member they filter.
    let refused: [(&str, &str, &str); 6] = [
        ("ExecutionStatus", "Done", "one of ["),
        ("AutomationType", "Global", "one of ["),
        ("AutomationSubtype", "ServiceRequest", "one of ["),
        ("StartTimeBefore", "yesterday", "a timestamp"),
        ("StartTimeAfter", "1717200000", "a timestamp"),
        ("ExecutionStatus", "success", "one of ["),
    ];
    for (key, value, takes) in refused {
        let (status, response) =
            describe(&router, &json!({"Filters": [{"Key": key, "Values": [value]}]})).await;
        let message = error_message(&response);
        assert_eq!(
            (status, error_code(&response)),
            (400, "InvalidFilterValue".to_string()),
            "case {key:?}={value:?}: expected the value to be refused, got {status} with \
             {response:?}"
        );
        let names_both = format!(
            "The following filter value is not valid: {value}. The {key} filter key takes "
        );
        assert!(
            message.starts_with(&names_both),
            "case {key:?}={value:?}: expected the refusal to open with {names_both:?}, got \
             {message:?}"
        );
        assert!(
            message.contains(takes),
            "case {key:?}={value:?}: expected the refusal to say the key takes {takes:?}, got \
             {message:?}"
        );
    }

    // A legal value of the member a key filters is served, whether or not any execution
    // carries it; the keys that constrain nothing take whatever the request sends.
    let served: [(&str, &str); 6] = [
        ("ExecutionStatus", "TimedOut"),
        ("AutomationType", "CrossAccount"),
        ("AutomationSubtype", "ChangeRequest"),
        ("StartTimeAfter", "2024-07-07T00:00:00Z"),
        ("DocumentNamePrefix", "anything-at-all"),
        ("TagKey", "owner"),
    ];
    for (key, value) in served {
        let (status, response) =
            describe(&router, &json!({"Filters": [{"Key": key, "Values": [value]}]})).await;
        assert_eq!(
            status, 200,
            "case {key:?}={value:?}: expected a legal value of the filtered member to be served, \
             got {status} with {response:?}"
        );
    }

    // A key outside the modelled set names no property of an execution to narrow by.
    let (status, response) = describe(
        &router,
        &json!({"Filters": [{"Key": "RunbookName", "Values": ["Beta-Sleep"]}]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a key outside the modelled set\": expected the filter to be refused, got {status} \
         with {response:?}"
    );
}

/// An automation is reported through the Region its start was sent to and nowhere else, and
/// reporting it changes nothing a later caller sees.
#[tokio::test]
async fn a_report_is_regional_and_changes_nothing() {
    let (router, engine) = service();
    runbook(&router, "Alpha-Pause", &pausing_runbook()).await;
    // A document is regional too, so the other Region's start resolves its own copy.
    runbook_in_region(&router, OTHER_REGION, "Alpha-Pause", &pausing_runbook()).await;
    let here = started(
        &router,
        "a start in this Region",
        &json!({"DocumentName": "Alpha-Pause"}),
    )
    .await;
    let there = started_in_region(
        &router,
        OTHER_REGION,
        "a start in the other Region",
        &json!({"DocumentName": "Alpha-Pause"}),
    )
    .await;
    pick_up(&engine, REGION);
    pick_up(&engine, OTHER_REGION);

    for (region, expected) in [(REGION, &here), (OTHER_REGION, &there)] {
        let (status, report) =
            call_in_region(&router, region, "DescribeAutomationExecutions", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with {report:?}"
        );
        assert_eq!(
            ids(&report),
            vec![expected.clone()],
            "case {region:?}: expected only the automation started in that Region, got {report}"
        );
    }

    // The report is a read: the statuses it reported stand, so a second report of the same
    // executions says exactly what the first did.
    let first = reported(&router, "the first report", &json!({})).await;
    let again = reported(&router, "the second report", &json!({})).await;
    assert_eq!(
        first, again,
        "case \"two reports\": expected a read to leave the executions as they were, got {first} \
         then {again}"
    );
}
