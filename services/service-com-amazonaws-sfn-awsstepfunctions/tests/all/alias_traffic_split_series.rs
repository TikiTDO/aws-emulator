// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `AWS/States` execution series an alias TRAFFIC SPLIT is
//! readable from, driven over each service's own wire protocol through the production
//! `MultiServiceRouter` with Step Functions and CloudWatch mounted on ONE engine — the
//! shape `core/router/src/lib.rs` mounts them in — and read back through CloudWatch's own
//! `ListMetrics` and `GetMetricStatistics`, so what is asserted is what a client
//! monitoring a canary or a linear rollout sees.
//!
//! RULE: an execution started through a state machine alias ARN is associated with that
//! alias and with the version the alias routed it to, so each of its execution
//! measurements is published once per reference it is associated with — under the state
//! machine, under the alias and under the routed version — which makes the per-version
//! series of one alias a PARTITION of that alias' own series: the share a version served
//! is readable from the series alone, and a version the routing configuration leaves at no
//! weight, or does not name at all, carries nothing.
//!
//! The sibling suite `states_metric_families` pins the reference families with one alias
//! per version at the whole weight; the rows here differ along what the rule generalises
//! over for a SPLIT: how many versions the routing configuration names (one and two), the
//! weights it gives them (the whole weight, no weight, an even split, a one-tenth canary),
//! the ORDER the entries are written in, and how many executions are started through the
//! alias.
//!
//! Every series name, dimension name and namespace below is written as a LITERAL rather
//! than read from the crate under test — they are the names the AWS Step Functions metric
//! reference publishes — so a series published under a name no AWS reader would ask for
//! fails here.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::state_machine_lifecycle::{string, REGION, ROLE_ARN};

/// The namespace Step Functions publishes its own series to.
const STATES_NAMESPACE: &str = "AWS/States";

/// The dimension those series name the state machine with.
const STATE_MACHINE_ARN_DIMENSION: &str = "StateMachineArn";

/// The dimension naming the ALIAS that routed the executions, beside the state machine's.
const ALIAS_DIMENSION: &str = "Alias";

/// The dimension naming the VERSION that served them, beside the state machine's.
const VERSION_DIMENSION: &str = "Version";

/// The series counting the executions that were started.
const EXECUTIONS_STARTED_SERIES: &str = "ExecutionsStarted";

/// The series counting the executions that completed successfully.
const EXECUTIONS_SUCCEEDED_SERIES: &str = "ExecutionsSucceeded";

/// The series measuring how long an execution took.
const EXECUTION_TIME_SERIES: &str = "ExecutionTime";

/// How long one poll of a status the clock owes sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a row waits for an execution to leave `RUNNING`: generous, so a loaded host
/// cannot fail a row that is not about how fast anything runs.
const PATIENCE: Duration = Duration::from_secs(90);

/// The window every series is read over, in seconds either side of now.
const WINDOW_SECS: f64 = 600.0;

/// The period the datapoints of that window are folded into, which is the one-minute
/// granularity these series are stored at.
const PERIOD_SECS: i64 = 60;

/// Step Functions and CloudWatch over ONE engine.
struct Rollout {
    router: MultiServiceRouter,
}

impl Rollout {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` header whose credential scope names `service`, which is
    /// what routes the request. The signature is never checked: this router runs with
    /// request authentication off.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        )
    }

    /// One `awsJson` call to the service `signing_name` routes to, requiring a 200.
    async fn rpc(
        &self,
        case: &str,
        signing_name: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", Self::authorization(signing_name))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let answered = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: {operation} must answer JSON, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200, got {status} with {answered}"
        );
        answered
    }

    /// One Step Functions call, requiring a 200.
    async fn sfn(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.rpc(case, "states", "AWSStepFunctions", operation, body)
            .await
    }

    /// One CloudWatch call, requiring a 200.
    async fn cloudwatch(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.rpc(
            case,
            "monitoring",
            "GraniteServiceVersion20100801",
            operation,
            body,
        )
        .await
    }

    /// Create a state machine, answering its ARN.
    async fn machine(&self, case: &str, name: &str, definition: &str) -> String {
        let created = self
            .sfn(
                case,
                "CreateStateMachine",
                &json!({ "name": name, "definition": definition, "roleArn": ROLE_ARN }),
            )
            .await;
        string(&created, "stateMachineArn")
    }

    /// Replace the state machine's definition without publishing a version.
    async fn update(&self, case: &str, machine: &str, definition: &str) {
        self.sfn(
            case,
            "UpdateStateMachine",
            &json!({ "stateMachineArn": machine, "definition": definition }),
        )
        .await;
    }

    /// Publish the state machine's current configuration as its next version, answering
    /// that version's ARN.
    async fn publish(&self, case: &str, machine: &str) -> String {
        let published = self
            .sfn(
                case,
                "PublishStateMachineVersion",
                &json!({ "stateMachineArn": machine }),
            )
            .await;
        string(&published, "stateMachineVersionArn")
    }

    /// Create an alias over `routing`, answering its ARN.
    async fn alias(&self, case: &str, name: &str, routing: &[Value]) -> String {
        let created = self
            .sfn(
                case,
                "CreateStateMachineAlias",
                &json!({ "name": name, "routingConfiguration": routing }),
            )
            .await;
        string(&created, "stateMachineAliasArn")
    }

    /// Start one execution of the reference `arn` names, answering its ARN.
    async fn start(&self, case: &str, arn: &str, name: &str) -> String {
        let started = self
            .sfn(
                case,
                "StartExecution",
                &json!({ "stateMachineArn": arn, "name": name }),
            )
            .await;
        string(&started, "executionArn")
    }

    /// The execution once it has left `RUNNING`, looked at again until it has or
    /// [`PATIENCE`] has passed.
    async fn settled(&self, case: &str, execution: &str) -> Value {
        let began = Instant::now();
        loop {
            let described = self
                .sfn(
                    case,
                    "DescribeExecution",
                    &json!({ "executionArn": execution }),
                )
                .await;
            if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
                return described;
            }
            assert!(
                began.elapsed() < PATIENCE,
                "case {case:?}: expected a terminal status within {PATIENCE:?}, last saw \
                 {described}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// The datapoints the series `metric` of [`STATES_NAMESPACE`] holds for `dimensions`,
    /// as CloudWatch's own reader answers them.
    async fn datapoints(&self, case: &str, metric: &str, dimensions: &Value) -> Vec<Value> {
        let now = now_secs();
        let stats = self
            .cloudwatch(
                case,
                "GetMetricStatistics",
                &json!({
                    "Namespace": STATES_NAMESPACE,
                    "MetricName": metric,
                    "Dimensions": dimensions,
                    "StartTime": now - WINDOW_SECS,
                    "EndTime": now + WINDOW_SECS,
                    "Period": PERIOD_SECS,
                    "Statistics": ["Sum", "SampleCount"],
                }),
            )
            .await;
        stats["Datapoints"].as_array().cloned().unwrap_or_default()
    }

    /// The SUM of that series over the window — the magnitude a client charts. A series
    /// with no datapoint sums to zero, which is what "the series carries nothing" reads
    /// as.
    async fn summed(&self, case: &str, metric: &str, dimensions: &Value) -> f64 {
        self.datapoints(case, metric, dimensions)
            .await
            .iter()
            .filter_map(|point| point["Sum"].as_f64())
            .sum()
    }

    /// How many SAMPLES that series holds over the window, which is how many measurements
    /// were published into it.
    async fn samples(&self, case: &str, metric: &str, dimensions: &Value) -> f64 {
        self.datapoints(case, metric, dimensions)
            .await
            .iter()
            .filter_map(|point| point["SampleCount"].as_f64())
            .sum()
    }

    /// The names of the [`STATES_NAMESPACE`] series CloudWatch LISTS for the dimension
    /// filters `filters`, sorted — what a client discovering a rollout's series reads
    /// before charting any of them.
    ///
    /// "If you specify one dimension name and a metric has that dimension and also other
    /// dimensions, it will be returned." (`ListMetricsInput$Dimensions`)
    async fn listed(&self, case: &str, filters: &Value) -> Vec<String> {
        let mut named: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "Namespace": STATES_NAMESPACE, "Dimensions": filters });
            if let Some(next) = &token {
                request["NextToken"] = json!(next);
            }
            let listed = self.cloudwatch(case, "ListMetrics", &request).await;
            for metric in listed["Metrics"].as_array().cloned().unwrap_or_default() {
                assert_eq!(
                    metric["Namespace"].as_str(),
                    Some(STATES_NAMESPACE),
                    "case {case:?}: a listed series carries the namespace asked for, got {metric}"
                );
                named.push(
                    metric["MetricName"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: a listed series is named"))
                        .to_string(),
                );
            }
            token = listed["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        named.sort();
        named.dedup();
        named
    }
}

/// Now, in whole epoch seconds, which is how an `awsJson` request carries an instant.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("now is after the epoch")
        .as_secs_f64()
        .trunc()
}

/// Definitions that differ only in the value they compute, so a version really holds a
/// configuration of its own.
fn definition(version: usize) -> String {
    format!(
        r#"{{"StartAt":"Only","States":{{"Only":{{"Type":"Pass","Result":{{"version":{version}}},"End":true}}}}}}"#
    )
}

/// One entry of a routing configuration.
fn route(version_arn: &str, weight: i64) -> Value {
    json!({ "stateMachineVersionArn": version_arn, "weight": weight })
}

/// The dimension set of a series reported per STATE MACHINE.
fn machine_dimensions(machine_arn: &str) -> Value {
    json!([{ "Name": STATE_MACHINE_ARN_DIMENSION, "Value": machine_arn }])
}

/// The dimension set of a series reported per the ALIAS that routed the executions: the
/// state machine's own dimension and the alias' NAME, which is what names an alias inside
/// its state machine.
fn alias_dimensions(machine_arn: &str, alias_name: &str) -> Value {
    json!([
        { "Name": ALIAS_DIMENSION, "Value": alias_name },
        { "Name": STATE_MACHINE_ARN_DIMENSION, "Value": machine_arn },
    ])
}

/// The dimension set of a series reported per the VERSION that served them: the state
/// machine's own dimension and the version's NUMBER.
fn version_dimensions(machine_arn: &str, version: &str) -> Value {
    json!([
        { "Name": STATE_MACHINE_ARN_DIMENSION, "Value": machine_arn },
        { "Name": VERSION_DIMENSION, "Value": version },
    ])
}

/// What a QUALIFIED state machine ARN names inside its state machine: the alias name or
/// the version number its qualifier carries, which is how the series of that reference
/// name it.
fn qualifier_of<'a>(machine_arn: &str, qualified_arn: &'a str) -> &'a str {
    qualified_arn
        .strip_prefix(machine_arn)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or_else(|| panic!("expected {qualified_arn:?} to qualify {machine_arn:?}"))
}

/// One row of the split table: the routing configuration an alias is created over, and how
/// many executions are started through it.
struct Row {
    case: &'static str,
    /// The weight each of the row's first versions is given, in the order the routing
    /// configuration writes the entries — so a row can write the newer version first.
    /// Each weight names the version at that index of [`Row::order`].
    order: &'static [usize],
    /// The weights, one per entry of [`Row::order`]; they sum to 100, as
    /// `RoutingConfigurationListItem$weight` requires.
    weights: &'static [i64],
    /// How many executions the row starts through the alias.
    runs: usize,
}

/// The rows: a routing configuration naming one version and ones naming two, differing in
/// the weights, in the order the entries are written and in the traffic put through them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one version at the whole weight",
            order: &[0],
            weights: &[100],
            runs: 4,
        },
        Row {
            case: "the whole weight on the older of two",
            order: &[0, 1],
            weights: &[100, 0],
            runs: 6,
        },
        Row {
            case: "the whole weight on the newer of two",
            order: &[0, 1],
            weights: &[0, 100],
            runs: 6,
        },
        Row {
            case: "an even split over two versions",
            order: &[0, 1],
            weights: &[50, 50],
            runs: 8,
        },
        Row {
            case: "a canary taking a tenth of the traffic",
            order: &[0, 1],
            weights: &[90, 10],
            runs: 20,
        },
        Row {
            case: "the newer version written first",
            order: &[1, 0],
            weights: &[80, 20],
            runs: 10,
        },
    ]
}

/// Every execution an alias routed is counted under that alias AND under the version it
/// ran, so the per-version series partition the alias' own series: the share each version
/// served is the share the service really routed to it, a version at no weight carries
/// nothing, and a published version the routing configuration never names carries nothing
/// either.
#[tokio::test]
async fn an_alias_split_is_readable_per_alias_and_per_routed_version() {
    let rollout = Rollout::new();

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            order,
            weights,
            runs,
        } = row;
        assert_eq!(
            order.len(),
            weights.len(),
            "case {case:?}: every routing entry carries a weight, got {order:?} and {weights:?}"
        );
        assert_eq!(
            weights.iter().sum::<i64>(),
            100,
            "case {case:?}: \"the sum of the weights in the routing configuration must be equal \
             to 100\", got {weights:?}"
        );

        // Three published versions, each holding a configuration of its own. The routing
        // configuration names the ones the row's `order` selects, so the rest are the
        // control: a published version no alias routes to.
        let machine = rollout
            .machine(case, &format!("split-{index}"), &definition(1))
            .await;
        let mut versions: Vec<String> = vec![rollout.publish(case, &machine).await];
        for next in 2..=3 {
            rollout.update(case, &machine, &definition(next)).await;
            versions.push(rollout.publish(case, &machine).await);
        }

        let routing: Vec<Value> = order
            .iter()
            .zip(weights)
            .map(|(version, weight)| route(&versions[*version], *weight))
            .collect();
        let alias_name = format!("rollout-{index}");
        let alias = rollout.alias(case, &alias_name, &routing).await;

        // The traffic the service really routed, read off the control plane: the version
        // each run was associated with. It is the ground truth the series are judged
        // against, so a series that counted the wrong reference cannot pass.
        let mut started: Vec<String> = Vec::with_capacity(runs);
        for run in 0..runs {
            started.push(rollout.start(case, &alias, &format!("run-{run}")).await);
        }
        let mut routed: BTreeMap<String, usize> = BTreeMap::new();
        for execution in &started {
            let settled = rollout.settled(case, execution).await;
            assert_eq!(
                settled.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: expected every run to succeed, got {settled}"
            );
            assert_eq!(
                settled.get("stateMachineAliasArn").and_then(Value::as_str),
                Some(alias.as_str()),
                "case {case:?}: a run started through an alias is associated with it, got \
                 {settled}"
            );
            let version = settled
                .get("stateMachineVersionArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an alias routes to a version, got {settled}")
                })
                .to_string();
            *routed.entry(version).or_default() += 1;
        }

        #[allow(clippy::cast_precision_loss)]
        let all_runs = runs as f64;

        // The ALIAS carries the whole of its own traffic, on each of the three series an
        // execution publishes.
        let dimensions = alias_dimensions(&machine, &alias_name);
        for series in [EXECUTIONS_STARTED_SERIES, EXECUTIONS_SUCCEEDED_SERIES] {
            let counted = rollout.summed(case, series, &dimensions).await;
            assert_eq!(
                counted, all_runs,
                "case {case:?}: expected the alias' {series} to count {all_runs}, got {counted}"
            );
        }
        let timed = rollout
            .samples(case, EXECUTION_TIME_SERIES, &dimensions)
            .await;
        assert_eq!(
            timed, all_runs,
            "case {case:?}: expected {all_runs} ExecutionTime samples under the alias, got {timed}"
        );

        // And each VERSION carries exactly the share it served, so the shares add back up
        // to the alias' own traffic: that sum is what makes the split readable.
        let mut split = 0.0;
        for (position, version) in versions.iter().enumerate() {
            let qualifier = qualifier_of(&machine, version);
            let dimensions = version_dimensions(&machine, qualifier);
            #[allow(clippy::cast_precision_loss)]
            let served = routed.get(version).copied().unwrap_or_default() as f64;
            let weight = order
                .iter()
                .position(|routed_version| *routed_version == position)
                .map_or(0, |entry| weights[entry]);
            if weight == 0 {
                assert_eq!(
                    served, 0.0,
                    "case {case:?}: a version at no weight is never routed to, got {served} runs \
                     on version {qualifier}"
                );
            }
            for series in [EXECUTIONS_STARTED_SERIES, EXECUTIONS_SUCCEEDED_SERIES] {
                let counted = rollout.summed(case, series, &dimensions).await;
                assert_eq!(
                    counted, served,
                    "case {case:?}: expected version {qualifier} at weight {weight} to count \
                     {served} in {series}, got {counted}"
                );
            }
            let timed = rollout
                .samples(case, EXECUTION_TIME_SERIES, &dimensions)
                .await;
            assert_eq!(
                timed, served,
                "case {case:?}: expected {served} ExecutionTime samples under version \
                 {qualifier}, got {timed}"
            );
            split += served;
        }
        assert_eq!(
            split, all_runs,
            "case {case:?}: the versions' shares partition the alias' traffic, got {split} of \
             {all_runs}"
        );

        // The state machine's own series carry the same runs, undivided: the reference
        // series are a split of it rather than a second count beside it.
        let counted = rollout
            .summed(
                case,
                EXECUTIONS_STARTED_SERIES,
                &machine_dimensions(&machine),
            )
            .await;
        assert_eq!(
            counted, all_runs,
            "case {case:?}: expected the state machine to count {all_runs}, got {counted}"
        );

        // A client finds those series before it charts them: `ListMetrics` lists the
        // alias' three series, and the routed versions' — and nothing at all for a version
        // the routing configuration left at no weight or never named.
        let mut expected = vec![
            EXECUTIONS_STARTED_SERIES.to_string(),
            EXECUTIONS_SUCCEEDED_SERIES.to_string(),
            EXECUTION_TIME_SERIES.to_string(),
        ];
        expected.sort();
        let filters = dimensions_filter(&machine, ALIAS_DIMENSION, &alias_name);
        let listed = rollout.listed(case, &filters).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the alias' series to be listed, got {listed:?}"
        );
        for version in &versions {
            let qualifier = qualifier_of(&machine, version);
            let filters = dimensions_filter(&machine, VERSION_DIMENSION, qualifier);
            let listed = rollout.listed(case, &filters).await;
            let served = routed.get(version).copied().unwrap_or_default();
            let wanted: Vec<String> = if served == 0 {
                Vec::new()
            } else {
                expected.clone()
            };
            assert_eq!(
                listed, wanted,
                "case {case:?}: version {qualifier} served {served} runs, so expected \
                 {wanted:?} listed, got {listed:?}"
            );
        }
    }
}

/// The `ListMetrics` dimension filters selecting the series of ONE reference of ONE state
/// machine: the state machine's own dimension and the reference's.
fn dimensions_filter(machine_arn: &str, dimension: &str, value: &str) -> Value {
    json!([
        { "Name": STATE_MACHINE_ARN_DIMENSION, "Value": machine_arn },
        { "Name": dimension, "Value": value },
    ])
}
