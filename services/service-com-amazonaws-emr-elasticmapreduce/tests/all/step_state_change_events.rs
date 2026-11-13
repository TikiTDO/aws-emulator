// SPDX-License-Identifier: CC0-1.0

//! What a `command-runner.jar` step SETTLES in, and the `EMR Step Status Change` event
//! every state it moves into is published as.
//!
//! TWO RULES are under test, both end to end over the wire.
//!
//! The step's: "Amazon EMR executes each step in the order listed. For a step to be
//! considered complete, the main function must exit with a zero exit code"
//! (`AddJobFlowSteps` doc), so the terminal `StepState` of a step IS the status the command
//! it submitted exited with — zero is `COMPLETED`, any other status is `FAILED` — with the
//! `StepTimeline` of the run. A command this emulator performs nothing of is no outcome at
//! all: such a step keeps the failure that says its work was not run, never a `COMPLETED`
//! for work nothing performed.
//!
//! The event's: every state a step moves into is published onto the account's DEFAULT
//! EventBridge bus as `source` `aws.emr` / `detail-type` `EMR Step Status Change`, carrying
//! that step's id, name, cluster, state and the severity that state rates in the event
//! `detail`. EventBridge then routes it: a rule whose `EventPattern` names the state the
//! step reached delivers it to that rule's SQS target, and the rule patterned on the other
//! state receives nothing.
//!
//! The rows span what the first rule generalises over: the FORM the command arrives in (a
//! shell program after `-c`, the step's own argument list), how the shell was spelled (by
//! name, by path), which construct the program ends with (`exit`, a utility that answers a
//! status, an and-or list, a sequence), the status itself (zero, non-zero, the boundary a
//! shell takes an exit status modulo) and a command this emulator performs nothing of.
//! Every row is read back through `DescribeStep` and `ListSteps`, and its event off the
//! queue a rule delivered it to, so nothing here is evidence of anything but the persisted
//! state and the real publication.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_flow_lifecycle::{at, INSTANCE_PROFILE, SERVICE_ROLE};

/// The `source` and `detail-type` Amazon EMR publishes a step's state change under, spelled
/// out here rather than imported, so a wrong constant in the implementation cannot make a
/// test pass.
const EMR_EVENT_SOURCE: &str = "aws.emr";
const STEP_STATE_CHANGE: &str = "EMR Step Status Change";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// How long a poll keeps asking before it gives up. The cluster takes two provisioning
/// spans and each step one run span, so this is several times the whole table's work and
/// still lets a broken case fail rather than hang.
const SETTLE_BUDGET: Duration = Duration::from_secs(60);

/// How long a poll waits between asks.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The step states a step never leaves (`com.amazonaws.emr#StepState`).
const TERMINAL_STATES: [&str; 4] = ["COMPLETED", "CANCELLED", "FAILED", "INTERRUPTED"];

/// How many `ReceiveMessage` rounds one drain makes: a receive answers at most ten
/// messages, and this is well above the rounds the whole table's events need.
const MAX_RECEIVE_ROUNDS: usize = 16;

/// One emulator carrying Amazon EMR, EventBridge and Amazon SQS over one shared engine:
/// the bus a step's events are published onto and the queue a rule delivers them to are
/// those services' own.
struct Pairing {
    router: MultiServiceRouter,
}

impl Pairing {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// One `awsJson` RPC call against the service whose signing name is `signing_name`.
    async fn call(
        &self,
        signing_name: &str,
        target_prefix: &str,
        json_version: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header(
                "content-type",
                format!("application/x-amz-json-{json_version}"),
            )
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDSTEPEVENTS/20261105/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    async fn emr(&self, operation: &str, body: Value) -> Value {
        let (status, payload) = self
            .call("elasticmapreduce", "ElasticMapReduce", "1.1", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "{operation} must answer 200, got {status} with {payload}"
        );
        payload
    }

    async fn events(&self, operation: &str, body: Value) -> Value {
        let (status, payload) = self
            .call("events", "AWSEvents", "1.1", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "{operation} must answer 200, got {status} with {payload}"
        );
        payload
    }

    async fn sqs(&self, operation: &str, body: Value) -> Value {
        let (status, payload) = self
            .call("sqs", "AmazonSQS", "1.0", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "{operation} must answer 200, got {status} with {payload}"
        );
        payload
    }

    /// A queue called `name`, created through Amazon SQS itself and carrying the grant a
    /// rule's target needs — EventBridge makes the target's `sqs:SendMessage` as its own
    /// service principal and relies on the queue's resource-based policy — answering its
    /// url and the ARN a rule names it by.
    async fn queue(&self, name: &str) -> (String, String) {
        let created = self.sqs("CreateQueue", json!({ "QueueName": name })).await;
        let url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateQueue must answer a QueueUrl, got {created}"))
            .to_string();
        let attributes = self
            .sqs(
                "GetQueueAttributes",
                json!({ "QueueUrl": &url, "AttributeNames": ["QueueArn"] }),
            )
            .await;
        let arn = at(&attributes, &["Attributes", "QueueArn"])
            .as_str()
            .unwrap_or_else(|| panic!("GetQueueAttributes must answer a QueueArn, got {attributes}"))
            .to_string();
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": "events.amazonaws.com" },
                "Action": "sqs:SendMessage",
                "Resource": &arn,
            }],
        })
        .to_string();
        self.sqs(
            "SetQueueAttributes",
            json!({ "QueueUrl": &url, "Attributes": { "Policy": policy } }),
        )
        .await;
        (url, arn)
    }

    /// A default-bus rule matching the `aws.emr` step state changes whose `detail.state` is
    /// `state`, delivering to the queue `queue_arn`.
    async fn rule_on_state(&self, name: &str, state: &str, queue_arn: &str) {
        self.events(
            "PutRule",
            json!({
                "Name": name,
                "State": "ENABLED",
                "EventPattern": json!({
                    "source": [EMR_EVENT_SOURCE],
                    "detail-type": [STEP_STATE_CHANGE],
                    "detail": { "state": [state] },
                })
                .to_string(),
            }),
        )
        .await;
        self.events(
            "PutTargets",
            json!({ "Rule": name, "Targets": [{ "Id": "t1", "Arn": queue_arn }] }),
        )
        .await;
    }

    /// A cluster that stays up when it has no steps to run, with `concurrency` steps
    /// running at a time, waited for until it is up the way a client waits.
    async fn live_cluster(&self, name: &str, concurrency: i32) -> String {
        let out = self
            .emr(
                "RunJobFlow",
                json!({
                    "Name": name,
                    "ReleaseLabel": "emr-6.15.0",
                    "JobFlowRole": INSTANCE_PROFILE,
                    "ServiceRole": SERVICE_ROLE,
                    "StepConcurrencyLevel": concurrency,
                    "Instances": {
                        "InstanceCount": 2,
                        "MasterInstanceType": "m5.xlarge",
                        "SlaveInstanceType": "m5.xlarge",
                        "KeepJobFlowAliveWhenNoSteps": true,
                    },
                }),
            )
            .await;
        let cluster_id = out["JobFlowId"]
            .as_str()
            .unwrap_or_else(|| panic!("RunJobFlow must answer a JobFlowId, got {out}"))
            .to_string();
        let deadline = Instant::now() + SETTLE_BUDGET;
        loop {
            let described = self
                .emr("DescribeCluster", json!({ "ClusterId": &cluster_id }))
                .await;
            let state = at(&described, &["Cluster", "Status", "State"])
                .as_str()
                .unwrap_or_default()
                .to_string();
            if state == "RUNNING" || state == "WAITING" {
                return cluster_id;
            }
            assert!(
                Instant::now() < deadline,
                "the cluster must reach a state that runs steps, last saw {state:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// `AddJobFlowSteps`, answering the minted step ids in submission order.
    async fn add_steps(&self, cluster_id: &str, steps: Vec<Value>) -> Vec<String> {
        let out = self
            .emr(
                "AddJobFlowSteps",
                json!({ "JobFlowId": cluster_id, "Steps": steps }),
            )
            .await;
        out["StepIds"]
            .as_array()
            .unwrap_or_else(|| panic!("AddJobFlowSteps must answer StepIds, got {out}"))
            .iter()
            .map(|id| {
                id.as_str()
                    .unwrap_or_else(|| panic!("a StepId is a string, got {out}"))
                    .to_string()
            })
            .collect()
    }

    async fn describe_step(&self, cluster_id: &str, step_id: &str) -> Value {
        self.emr(
            "DescribeStep",
            json!({ "ClusterId": cluster_id, "StepId": step_id }),
        )
        .await["Step"]
            .clone()
    }

    /// Poll `DescribeStep` until the step is terminal, answering the step as it settled.
    async fn await_terminal_step(&self, case: &str, cluster_id: &str, step_id: &str) -> Value {
        let deadline = Instant::now() + SETTLE_BUDGET;
        loop {
            let step = self.describe_step(cluster_id, step_id).await;
            let state = at(&step, &["Status", "State"])
                .as_str()
                .unwrap_or_default()
                .to_string();
            if TERMINAL_STATES.contains(&state.as_str()) {
                return step;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the step must reach a terminal state, last saw {state:?} in \
                 {step}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Every event waiting on the queue at `url`, as parsed bodies.
    ///
    /// Nothing is waited for: a publication is made inside the Amazon EMR call that
    /// materialised the transition and EventBridge delivers to a target inside that same
    /// call, so what the queue holds once a step has been read back is settled. Each
    /// message is DELETED as it is read, so no message is reported twice, and the receives
    /// stop at the first one that answers none.
    async fn drain(&self, url: &str) -> Vec<Value> {
        let mut collected: Vec<Value> = Vec::new();
        for _ in 0..MAX_RECEIVE_ROUNDS {
            let received = self
                .sqs(
                    "ReceiveMessage",
                    json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
                )
                .await;
            let messages = received["Messages"].as_array().cloned().unwrap_or_default();
            if messages.is_empty() {
                break;
            }
            for message in messages {
                if let Some(handle) = message["ReceiptHandle"].as_str() {
                    self.sqs(
                        "DeleteMessage",
                        json!({ "QueueUrl": url, "ReceiptHandle": handle }),
                    )
                    .await;
                }
                collected.push(
                    message["Body"]
                        .as_str()
                        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                        .unwrap_or(Value::Null),
                );
            }
        }
        collected
    }
}

/// A `command-runner.jar` step whose command line is `args`.
fn command_step(name: &str, args: Value) -> Value {
    json!({
        "Name": name,
        "ActionOnFailure": "CONTINUE",
        "HadoopJarStep": { "Jar": "command-runner.jar", "Args": args },
    })
}

/// One row of the table: a command, the state the step must settle in, and the status the
/// command exited with — `None` for a command this emulator performs nothing of, which is
/// no status at all.
struct Row {
    case: &'static str,
    name: &'static str,
    args: Value,
    state: &'static str,
    status: Option<i32>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a shell program that exits zero",
            name: "exit-zero",
            args: json!(["bash", "-c", "exit 0"]),
            state: "COMPLETED",
            status: Some(0),
        },
        Row {
            case: "a shell program that exits non-zero",
            name: "exit-seven",
            args: json!(["bash", "-c", "exit 7"]),
            state: "FAILED",
            status: Some(7),
        },
        Row {
            case: "a shell named by its path on the node",
            name: "path-qualified-shell",
            args: json!(["/bin/sh", "-c", "exit 0"]),
            state: "COMPLETED",
            status: Some(0),
        },
        Row {
            case: "a program ending in a utility that answers failure",
            name: "false-utility",
            args: json!(["sh", "-c", "false"]),
            state: "FAILED",
            status: Some(1),
        },
        Row {
            case: "a program whose and-or list recovers from the failure before it",
            name: "or-exit-zero",
            args: json!(["bash", "-c", "false || exit 0"]),
            state: "COMPLETED",
            status: Some(0),
        },
        Row {
            case: "a sequence that writes a line and then exits non-zero",
            name: "echo-then-exit",
            args: json!(["bash", "-c", "echo the step ran; exit 3"]),
            state: "FAILED",
            status: Some(3),
        },
        Row {
            case: "a utility that answers success, as the step's own argument list",
            name: "true-argv",
            args: json!(["true"]),
            state: "COMPLETED",
            status: Some(0),
        },
        Row {
            case: "a utility that answers failure, as the step's own argument list",
            name: "false-argv",
            args: json!(["false"]),
            state: "FAILED",
            status: Some(1),
        },
        Row {
            case: "the highest status a shell reports",
            name: "exit-255",
            args: json!(["bash", "-c", "exit 255"]),
            state: "FAILED",
            status: Some(255),
        },
        Row {
            // A shell takes an exit status modulo 256, so the first status past the byte it
            // reports is zero again — the boundary of the status range.
            case: "one status past the byte a shell reports",
            name: "exit-256",
            args: json!(["bash", "-c", "exit 256"]),
            state: "COMPLETED",
            status: Some(0),
        },
        Row {
            case: "a command this emulator performs nothing of",
            name: "unperformed-jar",
            args: json!(["hadoop", "jar", "s3://example/analytics.jar"]),
            state: "FAILED",
            status: None,
        },
    ]
}

/// The event detail of the message `message`, or a panic naming the case.
fn detail(case: &str, message: &Value) -> Value {
    assert_eq!(
        message["source"].as_str(),
        Some(EMR_EVENT_SOURCE),
        "case {case:?}: expected source {EMR_EVENT_SOURCE}, got {message}"
    );
    assert_eq!(
        message["detail-type"].as_str(),
        Some(STEP_STATE_CHANGE),
        "case {case:?}: expected detail-type {STEP_STATE_CHANGE}, got {message}"
    );
    message["detail"].clone()
}

/// THE TWO RULES, over the table: each step settles in the status its command exited with,
/// and the state it reached is delivered to the rule patterned on that state and to no
/// other.
#[tokio::test]
async fn a_step_settles_on_its_commands_status_and_publishes_that_state() {
    let pairing = Pairing::new();
    let (completed_url, completed_arn) = pairing.queue("emr-completed-steps").await;
    let (failed_url, failed_arn) = pairing.queue("emr-failed-steps").await;
    pairing
        .rule_on_state("emr-step-completed", "COMPLETED", &completed_arn)
        .await;
    pairing
        .rule_on_state("emr-step-failed", "FAILED", &failed_arn)
        .await;

    let table = rows();
    // Concurrency enough for every row to run at once: the sequencing rule is the step
    // runner's own suite's subject.
    let cluster_id = pairing.live_cluster("step events", 25).await;
    let step_ids = pairing
        .add_steps(
            &cluster_id,
            table.iter().map(|row| command_step(row.name, row.args.clone())).collect(),
        )
        .await;
    assert_eq!(
        step_ids.len(),
        table.len(),
        "AddJobFlowSteps must answer one id per submitted step, got {step_ids:?}"
    );

    // Every row settles in the state its own command's status says, with the timeline of a
    // run, and says why when it failed.
    let mut unperformed_reasons: Vec<String> = Vec::new();
    let mut performed_reasons: Vec<String> = Vec::new();
    for (row, step_id) in table.iter().zip(&step_ids) {
        let case = row.case;
        let step = pairing
            .await_terminal_step(case, &cluster_id, step_id)
            .await;
        let state = at(&step, &["Status", "State"]).as_str();
        assert_eq!(
            state,
            Some(row.state),
            "case {case:?}: expected {}, got {state:?} in {step}",
            row.state
        );
        for member in ["CreationDateTime", "StartDateTime", "EndDateTime"] {
            assert!(
                at(&step, &["Status", "Timeline", member]).as_f64().is_some(),
                "case {case:?}: a step that ran reports its {member}, got {step}"
            );
        }
        if row.state == "FAILED" {
            let reason = at(&step, &["Status", "FailureDetails", "Reason"])
                .as_str()
                .unwrap_or_default()
                .to_string();
            let message = at(&step, &["Status", "FailureDetails", "Message"])
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !reason.is_empty() && !message.is_empty(),
                "case {case:?}: a failed step says why it failed, got {step}"
            );
            match row.status {
                Some(status) => {
                    assert!(
                        message.contains(&status.to_string()),
                        "case {case:?}: the failure names the status {status} the command \
                         exited with, got {message:?}"
                    );
                    performed_reasons.push(reason);
                }
                None => unperformed_reasons.push(reason),
            }
        }
    }
    // A command nothing performed is NOT reported as a command that exited non-zero: the two
    // classes of failure carry different reasons.
    for unperformed in &unperformed_reasons {
        assert!(
            !performed_reasons.contains(unperformed),
            "case {:?}: a command this emulator performed nothing of must not report the \
             failure of a command that ran, got {unperformed:?}",
            "a command this emulator performs nothing of"
        );
    }

    // `ListSteps` projects the same records: the settled state of every row is the one the
    // listing reports for it.
    let listed = pairing
        .emr("ListSteps", json!({ "ClusterId": &cluster_id }))
        .await;
    for (row, step_id) in table.iter().zip(&step_ids) {
        let summary = at(&listed, &["Steps"])
            .as_array()
            .and_then(|steps| {
                steps
                    .iter()
                    .find(|step| step["Id"].as_str() == Some(step_id.as_str()))
                    .cloned()
            })
            .unwrap_or_else(|| panic!("case {:?}: {step_id} must be listed, got {listed}", row.case));
        assert_eq!(
            at(&summary, &["Status", "State"]).as_str(),
            Some(row.state),
            "case {:?}: the listing reports the same state, got {summary}",
            row.case
        );
    }

    // THE EVENTS: one per step on the queue of the rule patterned on the state that step
    // reached, and nothing for it on the other rule's queue.
    let expected_completed = table.iter().filter(|row| row.state == "COMPLETED").count();
    let expected_failed = table.iter().filter(|row| row.state == "FAILED").count();
    let completed = pairing.drain(&completed_url).await;
    let failed = pairing.drain(&failed_url).await;
    assert_eq!(
        completed.len(),
        expected_completed,
        "expected one event per completed step, got {completed:?}"
    );
    assert_eq!(
        failed.len(),
        expected_failed,
        "expected one event per failed step, got {failed:?}"
    );

    for (row, step_id) in table.iter().zip(&step_ids) {
        let case = row.case;
        let (matching, other) = if row.state == "COMPLETED" {
            (&completed, &failed)
        } else {
            (&failed, &completed)
        };
        let delivered: Vec<Value> = matching
            .iter()
            .filter(|message| detail(case, message)["stepId"] == json!(step_id))
            .cloned()
            .collect();
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected exactly one {} event for {step_id}, got {delivered:?}",
            row.state
        );
        let event_detail = detail(case, &delivered[0]);
        assert_eq!(
            event_detail["state"], json!(row.state),
            "case {case:?}: the event carries the state the step reached, got {event_detail}"
        );
        assert_eq!(
            event_detail["stepId"], json!(step_id),
            "case {case:?}: the event names the step, got {event_detail}"
        );
        assert_eq!(
            event_detail["clusterId"], json!(&cluster_id),
            "case {case:?}: the event names the cluster, got {event_detail}"
        );
        assert_eq!(
            event_detail["name"], json!(row.name),
            "case {case:?}: the event names the step's own name, got {event_detail}"
        );
        assert_eq!(
            event_detail["actionOnFailure"], json!("CONTINUE"),
            "case {case:?}: the event reports what the cluster does on failure, got \
             {event_detail}"
        );
        // How the state RATES: a completed step is the ordinary course of a cluster's work
        // and a failed one is the error.
        let expected_severity = if row.state == "COMPLETED" { "INFO" } else { "ERROR" };
        assert_eq!(
            event_detail["severity"], json!(expected_severity),
            "case {case:?}: expected severity {expected_severity}, got {event_detail}"
        );
        // The event's own sentence names the step, the cluster and what became of the step.
        let message = event_detail["message"].as_str().unwrap_or_default().to_string();
        let became = if row.state == "COMPLETED" { "completed" } else { "failed" };
        for part in [step_id.as_str(), row.name, cluster_id.as_str(), became] {
            assert!(
                message.contains(part),
                "case {case:?}: the event's message must name {part:?}, got {message:?}"
            );
        }
        assert!(
            !other
                .iter()
                .any(|message| detail(case, message)["stepId"] == json!(step_id)),
            "case {case:?}: the rule patterned on the other state must receive nothing for \
             {step_id}, got {other:?}"
        );
    }
}

/// A step's transition is published ONCE however often it is read: the states a step moved
/// through are each delivered a single time, and polling a settled step publishes nothing
/// more.
#[tokio::test]
async fn a_step_transition_is_published_once_however_often_it_is_read() {
    let case = "a settled step read again";
    let pairing = Pairing::new();
    let (completed_url, completed_arn) = pairing.queue("emr-completed-once").await;
    pairing
        .rule_on_state("emr-step-completed-once", "COMPLETED", &completed_arn)
        .await;
    let cluster_id = pairing.live_cluster("one event per transition", 1).await;
    let step_ids = pairing
        .add_steps(
            &cluster_id,
            vec![command_step("single", json!(["bash", "-c", "exit 0"]))],
        )
        .await;
    let step_id = step_ids
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: expected a step id"))
        .clone();

    let step = pairing
        .await_terminal_step(case, &cluster_id, &step_id)
        .await;
    assert_eq!(
        at(&step, &["Status", "State"]).as_str(),
        Some("COMPLETED"),
        "case {case:?}: the step must complete, got {step}"
    );

    // Read the settled step several more times, each of which materialises nothing new.
    for _ in 0..3 {
        pairing.describe_step(&cluster_id, &step_id).await;
        pairing
            .emr("ListSteps", json!({ "ClusterId": &cluster_id }))
            .await;
        pairing
            .emr("DescribeCluster", json!({ "ClusterId": &cluster_id }))
            .await;
    }

    // One delivery for the one COMPLETED transition, however many times the step was read.
    let delivered = pairing.drain(&completed_url).await;
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: expected one COMPLETED event however often the step is read, got \
         {delivered:?}"
    );
    assert_eq!(
        detail(case, &delivered[0])["stepId"],
        json!(&step_id),
        "case {case:?}: the event names the step, got {:?}",
        delivered[0]
    );
}
