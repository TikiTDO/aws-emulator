// SPDX-License-Identifier: CC0-1.0

//! What this service PUBLISHES about its own work: the `AWS/States` metric series its
//! executions, its workers' tasks, its state machines and its APIs are counted in, and
//! the history events a state machine's `loggingConfiguration` delivers to CloudWatch
//! Logs.
//!
//! An execution runs with no client action at all once it has been started, so the
//! only way a client can see that it started, how long it ran and whether it succeeded
//! — and the only way an alarm, a dashboard or a log-based assertion over Step
//! Functions can see anything — is what the service publishes about its own work:
//!
//! * the METRICS go to the [`METRIC_NAMESPACE`] namespace, each series dimensioned on
//!   WHAT it is about:
//!   * the EXECUTION series are reported per REFERENCE the execution is associated with:
//!     one datapoint per execution started, one per execution that reached a terminal
//!     status under the metric for that status, the span it ran for, and the redrive
//!     series of a re-run. A run started with an unqualified ARN is associated with the
//!     state machine alone ([`STATE_MACHINE_ARN_DIMENSION`]); one started through a
//!     QUALIFIED ARN is associated with the alias that routed it ([`ALIAS_DIMENSION`])
//!     and the version it ran ([`VERSION_DIMENSION`]) as well, so it is counted under
//!     each of them ([`execution_series`]) — which is what makes a weighted alias's
//!     traffic split, and the traffic one version served, visible. An EXPRESS workflow is
//!     billed per run, so it also publishes the two billed series, whose values are the
//!     very figures `StartSyncExecution` reports as `billingDetails` — summing a window
//!     of either series therefore gives the summed billing details of the runs in it, and
//!     a STANDARD workflow, which is not billed that way, publishes neither;
//!   * the series of a `Task` state's own WORK are dimensioned on the RESOURCE that work
//!     was taken on, in the family that kind of resource is counted in ([`WorkSeries`]):
//!     a state whose `Resource` is the ARN of a Lambda FUNCTION counts each invocation it
//!     takes under [`LAMBDA_FUNCTION_ARN_DIMENSION`], and one whose `Resource` is the ARN
//!     of an ACTIVITY counts each task it hands to that activity's workers under
//!     [`ACTIVITY_ARN_DIMENSION`] — scheduled, started, and closed under the series of
//!     the outcome it reached, beside the spans it stayed scheduled and ran for — so a
//!     client can tell which function's or which worker fleet's work is failing from a
//!     series the execution counters cannot show it. A `Resource` that names one of this
//!     service's integration tokens names neither resource, so it is counted in neither
//!     family, and the dimensions never cross;
//!   * the series counting what a state machine HOLDS are reported per state machine
//!     ([`RESOURCE_ARN_DIMENSION`]): how many versions and how many aliases it carries,
//!     reported as of every accepted mutation of them, so a refused publish is visible as
//!     a count that did not move;
//!   * the series of an API of this service are reported per operation
//!     ([`API_NAME_DIMENSION`]): one unit of consumed capacity per call served
//!     ([`meter_api_call`]), which is what a client watching its own approach to an API's
//!     quota reads;
//! * the LOG EVENTS go to the log group the configuration names, with the events
//!   selected by its `level` ([`delivered_at_level`]) and the execution data included
//!   only where `includeExecutionData` asks for it.
//!
//! Both are written into the stores the OWNING service's own readers answer from —
//! CloudWatch's for the metrics, CloudWatch Logs' for the events — through those
//! services' own record types, so a published metric and `GetMetricData`, and a
//! delivered event and `FilterLogEvents`, cannot disagree. Neither goes out over
//! `PutMetricData` or `PutLogEvents`: CloudWatch reserves the `AWS/` namespaces for
//! the services that own them ("you should not specify a namespace that begins with
//! `AWS/`", `PutMetricData$Namespace`), so a service's own metric never travels over
//! that API.
//!
//! ## When it is published
//!
//! An observation is QUEUED on this service's own store at the instant the thing it
//! measures happens — an execution admitted, an outcome the clock made due, a history
//! event of a task's work becoming reportable
//! ([`ComAmazonawsSfnAwsstepfunctionsState::observe_task_work`]) — and
//! PUBLISHED once the call that queued it has released the store ([`publish`]). An
//! outcome the CLOCK made due is queued by no call at all, so the clock that
//! materialised it hands it over itself ([`super::events`]): nothing an execution
//! publishes waits for a client to call this service again, which for a workflow whose
//! own reporting APIs do not serve it would be never. The queue is what keeps the two
//! stores from being locked at once and what lets the work that produces an observation
//! stay a pure store transition; it is bounded ([`MAX_PENDING_OBSERVATIONS`]), so a
//! deployment with no CloudWatch mounted cannot grow it without end.
//!
//! Every call this service SERVES hands the queue over once it has answered
//! ([`meter_api_call`], called in front of every operation by
//! [`crate::service::identity_authorization`]), so a mutation's own series — the counts
//! of what a state machine holds, which no clock ever makes due — reach CloudWatch's
//! store as soon as the call that made it has answered.

use ::std::collections::BTreeMap;
use ::std::time::{SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;
use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

use super::ComAmazonawsSfnAwsstepfunctionsState;

/// The namespace Step Functions publishes its own metrics to.
pub const METRIC_NAMESPACE: &str = "AWS/States";

/// The dimension those series are reported per: the state machine the execution
/// belongs to, named by its ARN.
pub const STATE_MACHINE_ARN_DIMENSION: &str = "StateMachineArn";

/// The dimension the series of the executions one ALIAS of a state machine routed are
/// reported per, beside that state machine's own dimension: the alias, named by its NAME
/// — the qualifier of its ARN, which is what names an alias inside the state machine it
/// belongs to.
///
/// An alias is a reference of its own rather than another name for the state machine, so
/// its traffic is its own series: that is what makes a weighted alias's split, and a
/// canary's own failures, visible beside the state machine's totals. The state machine's
/// dimension stays on the series, so a reader asking for the state machine is not
/// answered one alias' traffic.
pub const ALIAS_DIMENSION: &str = "Alias";

/// The dimension the series of the executions one VERSION of a state machine served are
/// reported per, beside that state machine's own: the version, named by its NUMBER — the
/// qualifier of its ARN — for the same reason an alias is named by its name.
pub const VERSION_DIMENSION: &str = "Version";

/// The series counting the executions that were STARTED.
pub const EXECUTIONS_STARTED_METRIC: &str = "ExecutionsStarted";

/// The series counting the executions that completed successfully.
pub const EXECUTIONS_SUCCEEDED_METRIC: &str = "ExecutionsSucceeded";

/// The series counting the executions that failed.
pub const EXECUTIONS_FAILED_METRIC: &str = "ExecutionsFailed";

/// The series counting the executions that were aborted.
pub const EXECUTIONS_ABORTED_METRIC: &str = "ExecutionsAborted";

/// The series counting the executions that timed out.
pub const EXECUTIONS_TIMED_OUT_METRIC: &str = "ExecutionsTimedOut";

/// The series measuring how long an execution took, from start to terminal status.
pub const EXECUTION_TIME_METRIC: &str = "ExecutionTime";

/// The series measuring the duration an EXPRESS run is billed for, which is the
/// `billedDurationInMilliseconds` of its `billingDetails`.
pub const EXPRESS_BILLED_DURATION_METRIC: &str = "ExpressExecutionBilledDuration";

/// The series measuring the memory an EXPRESS run is billed for, which is the
/// `billedMemoryUsedInMB` of its `billingDetails`.
pub const EXPRESS_BILLED_MEMORY_METRIC: &str = "ExpressExecutionBilledMemory";

/// The dimension the series of a `Task` state's own work are reported per, for a state
/// whose `Resource` is the ARN of a LAMBDA FUNCTION: the function that work was taken
/// on, named by the ARN the `Resource` carried.
///
/// A `Resource` that names one of this service's integration tokens
/// (`arn:aws:states:::lambda:invoke`) names no function: the request it builds does,
/// and the work is an integration call rather than an invocation of a function the
/// definition named — so a task of that form is counted in none of the series below,
/// and this dimension and [`STATE_MACHINE_ARN_DIMENSION`] never cross.
pub const LAMBDA_FUNCTION_ARN_DIMENSION: &str = "LambdaFunctionArn";

/// The series counting the invocations a `Task` state SCHEDULED for its function.
pub const LAMBDA_FUNCTIONS_SCHEDULED_METRIC: &str = "LambdaFunctionsScheduled";

/// The series counting the invocations that were STARTED.
pub const LAMBDA_FUNCTIONS_STARTED_METRIC: &str = "LambdaFunctionsStarted";

/// The series counting the invocations that succeeded.
pub const LAMBDA_FUNCTIONS_SUCCEEDED_METRIC: &str = "LambdaFunctionsSucceeded";

/// The series counting the invocations that failed.
pub const LAMBDA_FUNCTIONS_FAILED_METRIC: &str = "LambdaFunctionsFailed";

/// The series counting the invocations that ran out of the time the state allowed
/// them.
pub const LAMBDA_FUNCTIONS_TIMED_OUT_METRIC: &str = "LambdaFunctionsTimedOut";

/// The series measuring the span one invocation stayed SCHEDULED for: from the instant
/// the task scheduled it to the instant it started.
pub const LAMBDA_FUNCTION_SCHEDULE_TIME_METRIC: &str = "LambdaFunctionScheduleTime";

/// The series measuring the span one invocation RAN for: from the instant it started to
/// the instant it closed.
pub const LAMBDA_FUNCTION_RUN_TIME_METRIC: &str = "LambdaFunctionRunTime";

/// The series measuring the whole span one invocation took: from the instant the task
/// scheduled it to the instant it closed.
pub const LAMBDA_FUNCTION_TIME_METRIC: &str = "LambdaFunctionTime";

/// The dimension the series of a `Task` state's own work are reported per, for a state
/// whose `Resource` is the ARN of an ACTIVITY: the activity the task was handed to a
/// worker under, named by the ARN the `Resource` carried.
///
/// An activity's work is taken by a worker that polls for it rather than by a call this
/// service makes, so these series are the only ones a client can see a worker fleet
/// through — the backlog it has to take, the outcomes it reported and the tasks it let
/// run out of time.
pub const ACTIVITY_ARN_DIMENSION: &str = "ActivityArn";

/// The series counting the tasks a `Task` state SCHEDULED for its activity.
pub const ACTIVITIES_SCHEDULED_METRIC: &str = "ActivitiesScheduled";

/// The series counting the tasks a worker's poll took.
pub const ACTIVITIES_STARTED_METRIC: &str = "ActivitiesStarted";

/// The series counting the tasks a worker reported succeeded.
pub const ACTIVITIES_SUCCEEDED_METRIC: &str = "ActivitiesSucceeded";

/// The series counting the tasks a worker reported failed.
pub const ACTIVITIES_FAILED_METRIC: &str = "ActivitiesFailed";

/// The series counting the tasks that ran out of the time the scheduling state allowed
/// them.
pub const ACTIVITIES_TIMED_OUT_METRIC: &str = "ActivitiesTimedOut";

/// The series measuring the span one task stayed SCHEDULED for: from the instant the
/// state scheduled it to the instant a worker took it.
pub const ACTIVITY_SCHEDULE_TIME_METRIC: &str = "ActivityScheduleTime";

/// The series measuring the span one task RAN for: from the instant a worker took it to
/// the instant it closed.
pub const ACTIVITY_RUN_TIME_METRIC: &str = "ActivityRunTime";

/// The series measuring the whole span one task took: from the instant the state
/// scheduled it to the instant it closed.
pub const ACTIVITY_TIME_METRIC: &str = "ActivityTime";

/// The dimension the series of an API of this service are reported per: the operation
/// the call named.
pub const API_NAME_DIMENSION: &str = "APIName";

/// The series counting the capacity the calls of one API consumed — one unit per call
/// served, so the sum over a window is the number of calls made in it, which is what a
/// client watching its own approach to an API's quota reads.
pub const CONSUMED_CAPACITY_METRIC: &str = "ConsumedCapacity";

/// The dimension the series counting what a state machine HOLDS are reported per: the
/// state machine itself, named by its unqualified ARN.
pub const RESOURCE_ARN_DIMENSION: &str = "ResourceArn";

/// The series reporting how many versions a state machine holds.
pub const VERSION_COUNT_METRIC: &str = "VersionCount";

/// The series reporting how many aliases a state machine holds.
pub const ALIAS_COUNT_METRIC: &str = "AliasCount";

/// The series counting the executions a redrive re-ran.
pub const EXECUTIONS_REDRIVEN_METRIC: &str = "ExecutionsRedriven";

/// The series counting the redriven executions that then completed successfully.
pub const REDRIVEN_EXECUTIONS_SUCCEEDED_METRIC: &str = "RedrivenExecutionsSucceeded";

/// The series counting the redriven executions that then failed.
pub const REDRIVEN_EXECUTIONS_FAILED_METRIC: &str = "RedrivenExecutionsFailed";

/// The series counting the redriven executions that were aborted.
pub const REDRIVEN_EXECUTIONS_ABORTED_METRIC: &str = "RedrivenExecutionsAborted";

/// The series counting the redriven executions that timed out.
pub const REDRIVEN_EXECUTIONS_TIMED_OUT_METRIC: &str = "RedrivenExecutionsTimedOut";

/// The `StandardUnit` a bare count of things carries.
pub const COUNT_UNIT: &str = "Count";

/// The `StandardUnit` a span of milliseconds carries.
pub const MILLISECONDS_UNIT: &str = "Milliseconds";

/// The `StandardUnit` an amount of memory in megabytes carries.
pub const MEGABYTES_UNIT: &str = "Megabytes";

/// The `StorageResolution` these datapoints are published at, in seconds: regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// `LogLevel`'s value that logs every history event ("ALL – Log all events").
pub const LOG_LEVEL_ALL: &str = "ALL";

/// `LogLevel`'s value that logs the failures ("ERROR – Log all events of type ERROR
/// and FATAL").
pub const LOG_LEVEL_ERROR: &str = "ERROR";

/// `LogLevel`'s value that logs the execution-level failures alone ("FATAL – Log all
/// events of type FATAL").
pub const LOG_LEVEL_FATAL: &str = "FATAL";

/// How many observations this service's store queues before it drops them.
///
/// The queue is drained by the call that filled it, so it normally holds the few
/// observations of one call; the bound is what keeps a deployment with no CloudWatch
/// mounted — where nothing is ever published — from growing it with every execution.
pub const MAX_PENDING_OBSERVATIONS: usize = 4096;

/// ONE measurement an execution owes a series, waiting to be published.
#[derive(Debug, Clone)]
pub struct Datapoint {
    /// The dimension SET that identifies the series — the state machine an execution
    /// belongs to ([`state_machine_dimensions`]), the alias that routed it
    /// ([`state_machine_alias_dimensions`]), the function or activity a task's work was
    /// taken on ([`lambda_function_dimensions`], [`activity_dimensions`]) — and the
    /// resource each of its dimensions names.
    ///
    /// A SET rather than one dimension, because "a metric is uniquely defined by a name,
    /// a namespace, and zero or more dimensions" (`Metric`): a series scoped to a
    /// REFERENCE of a resource carries the resource's own dimension beside the one
    /// naming that reference, so a reader asking for the resource and a reader asking
    /// for one reference of it are asking for two different series.
    pub dimensions: BTreeMap<String, String>,
    /// The metric the measurement belongs to.
    pub metric: &'static str,
    /// What was measured.
    pub value: f64,
    /// The unit it was measured in.
    pub unit: &'static str,
    /// The instant it was taken at.
    pub at: SystemTime,
}

/// The history events of ONE execution a logging configuration owes a log group.
#[derive(Debug, Clone)]
pub struct Delivery {
    /// The log group the configuration names, by the NAME CloudWatch Logs holds it
    /// under.
    pub log_group: String,
    /// The Region that group lives in, read off the ARN the configuration carried, so
    /// a group this delivery has to create carries the ARN its own reader reports.
    pub region: String,
    /// The stream the events are written to: one per execution.
    pub log_stream: String,
    /// Each event as `(timestamp in epoch milliseconds, message)`.
    pub events: Vec<(i64, String)>,
}

/// The observations this service's store has queued and not published yet.
#[derive(Debug, Default)]
pub struct Pending {
    /// The metric measurements.
    pub datapoints: Vec<Datapoint>,
    /// The log deliveries.
    pub deliveries: Vec<Delivery>,
}

impl Pending {
    /// Whether there is nothing to publish.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.datapoints.is_empty() && self.deliveries.is_empty()
    }

    /// Queue `datapoint`, or drop it when the queue is full (see
    /// [`MAX_PENDING_OBSERVATIONS`]).
    fn queue_datapoint(&mut self, datapoint: Datapoint) {
        if self.datapoints.len() >= MAX_PENDING_OBSERVATIONS {
            return;
        }
        self.datapoints.push(datapoint);
    }

    /// Queue `delivery`, or drop it when the queue is full.
    fn queue_delivery(&mut self, delivery: Delivery) {
        if self.deliveries.len() >= MAX_PENDING_OBSERVATIONS {
            return;
        }
        self.deliveries.push(delivery);
    }
}

/// `at` in milliseconds since the Unix epoch, which is what every CloudWatch Logs
/// timestamp is expressed in (`OutputLogEvent$timestamp`).
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// `at` in seconds since the Unix epoch, which is how a CloudWatch datapoint carries
/// its timestamp.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The metric an execution that reached `status` is counted under, or `None` for a
/// status that is not terminal: a running execution has not completed, so it is
/// counted in no completion series yet.
fn completion_metric(status: &str) -> Option<&'static str> {
    match status {
        super::EXECUTION_STATUS_SUCCEEDED => Some(EXECUTIONS_SUCCEEDED_METRIC),
        super::EXECUTION_STATUS_FAILED => Some(EXECUTIONS_FAILED_METRIC),
        super::EXECUTION_STATUS_ABORTED => Some(EXECUTIONS_ABORTED_METRIC),
        super::EXECUTION_STATUS_TIMED_OUT => Some(EXECUTIONS_TIMED_OUT_METRIC),
        _ => None,
    }
}

/// The series a REDRIVEN execution that reached `status` is counted in BESIDE the
/// completion series every execution is counted in, and `None` for a status that is not
/// terminal.
///
/// A redrive re-runs an execution that had already ended, so the outcome of the re-run
/// is what tells a client whether re-running helped: the base series alone cannot, since
/// the first attempt's own outcome is already counted in it.
fn redriven_completion_metric(status: &str) -> Option<&'static str> {
    match status {
        super::EXECUTION_STATUS_SUCCEEDED => Some(REDRIVEN_EXECUTIONS_SUCCEEDED_METRIC),
        super::EXECUTION_STATUS_FAILED => Some(REDRIVEN_EXECUTIONS_FAILED_METRIC),
        super::EXECUTION_STATUS_ABORTED => Some(REDRIVEN_EXECUTIONS_ABORTED_METRIC),
        super::EXECUTION_STATUS_TIMED_OUT => Some(REDRIVEN_EXECUTIONS_TIMED_OUT_METRIC),
        _ => None,
    }
}

/// Whether an event of type `event_type` is delivered under the logging `level`.
///
/// `LogLevel`'s own documentation: "ALL – Log all events. ERROR – Log all events of
/// type ERROR and FATAL. FATAL – Log all events of type FATAL. OFF – No logging." So
/// the decision is which CLASS the event type belongs to:
///
/// * FATAL is the end of the execution itself, which is what a workflow that failed,
///   was aborted or timed out reports;
/// * ERROR is every other event reporting that something did not succeed — a task, an
///   activity, an iteration, a branch or a map run that failed, timed out or was
///   aborted, whose type names end the same way the execution-level ones do, and
///   entering a `Fail` state, which is the definition declaring the run failed;
/// * everything else is the ordinary progress of the run, which only `ALL` logs.
#[must_use]
pub fn delivered_at_level(level: &str, event_type: &str) -> bool {
    let fatal = matches!(
        event_type,
        super::EVENT_EXECUTION_FAILED
            | super::EVENT_EXECUTION_ABORTED
            | super::EVENT_EXECUTION_TIMED_OUT
    );
    let error = fatal
        || event_type == super::EVENT_FAIL_STATE_ENTERED
        || event_type.ends_with("Failed")
        || event_type.ends_with("TimedOut")
        || event_type.ends_with("Aborted");
    match level {
        LOG_LEVEL_ALL => true,
        LOG_LEVEL_ERROR => error,
        LOG_LEVEL_FATAL => fatal,
        // `OFF`, and any value the enum does not name, deliver nothing.
        _ => false,
    }
}

/// The log group a `loggingConfiguration` delivers to, as `(region, name)`, or `None`
/// when it names none.
///
/// The ARN the configuration carries is the qualified log group ARN
/// `arn:<partition>:logs:<region>:<account>:log-group:<name>:*` — the form
/// `logGroupArn` documents and `crate::state::invalid_logging_reason` admits — so the
/// Region and the name CloudWatch Logs holds the group under are read out of it rather
/// than assumed.
#[must_use]
pub fn log_group_of(configuration: &crate::ops::types::LoggingConfiguration) -> Option<(String, String)> {
    let arn = configuration
        .destinations
        .as_deref()?
        .first()?
        .cloud_watch_logs_log_group
        .as_ref()?
        .log_group_arn
        .as_deref()?;
    let mut fields = arn.splitn(6, ':');
    let _scheme = fields.next()?;
    let _partition = fields.next()?;
    let _service = fields.next()?;
    let region = fields.next()?.to_string();
    let _account = fields.next()?;
    let resource = fields.next()?;
    let name = resource
        .strip_prefix("log-group:")?
        .strip_suffix(":*")?
        .to_string();
    (!name.is_empty()).then_some((region, name))
}

/// The IAM actions that give a state machine's own role ACCESS to the log destination
/// its logging configuration names.
///
/// Step Functions does not write a vended log as the caller: it sets a DELIVERY up and
/// writes the events as the state machine's role, so the role is what needs the
/// CloudWatch Logs permissions the service's logging guidance publishes — creating the
/// delivery, and putting the events. A role whose documents allow it NONE of them has
/// no access to that destination at all and cannot deliver one event, which is what the
/// control plane refuses when the configuration is given rather than letting a state
/// machine that can never log be reported as healthy.
///
/// A role allowed ANY of them HAS access, and is served: the refused call's own sentence
/// is about the role's access to the destination, the recorded creates show one whose
/// role's documents carry the event-writing grant alone being served, and this service
/// reads only the identity-based documents it can see — so a create the service serves
/// is not refused here on a document that grants part of a published policy.
pub const LOG_DELIVERY_ACTIONS: &[&str] = &["logs:CreateLogDelivery", "logs:PutLogEvents"];

/// The resource those actions are taken on: the log destination the configuration
/// names, which is what the refusal is about.
///
/// It is the qualified log group ARN the destination carries, so a policy that grants a
/// delivery action on every resource and one that grants it on the group the
/// configuration names both give the role access to the destination, while a policy
/// that grants neither gives it none.
fn log_delivery_resource(
    configuration: &crate::ops::types::LoggingConfiguration,
) -> Option<String> {
    configuration
        .destinations
        .as_deref()?
        .first()?
        .cloud_watch_logs_log_group
        .as_ref()?
        .log_group_arn
        .clone()
}

/// What a control-plane call answers when the role it names cannot deliver to the log
/// destination the configuration carries.
///
/// Fixed service text: the sentence states the relationship that does not hold — the
/// state machine's role against the log destination — and carries no value from the
/// request, so neither the role nor the group is quoted back.
pub const LOG_DESTINATION_ACCESS_DENIED: &str =
    "The state machine IAM Role is not authorized to access the Log Destination";

/// Whether the role `role_arn` has NO access to the log destination `configuration`
/// names, so the call that named them both is refused
/// ([`LOG_DESTINATION_ACCESS_DENIED`]).
///
/// A configuration that delivers nothing — one whose level is `OFF`, or that names no
/// log group — needs no permission at all, so nothing is decided for it. Everything
/// else is the one reading of what a state machine's role may do
/// ([`super::task_authorization::role_is_denied`]), asked for EACH delivery action in
/// turn: the refusal stands only where that reading withholds every one of them, which
/// is the role having no access to the destination at all ([`LOG_DELIVERY_ACTIONS`]).
/// That reading shows a refusal only where the account really holds the role and the
/// documents it holds for it withhold the action, so a role the account does not hold —
/// and an engine with no IAM store — refuses nothing.
///
/// Call it with no store of this service locked: it takes IAM's own.
#[must_use]
pub fn log_destination_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    configuration: &crate::ops::types::LoggingConfiguration,
    role_arn: &str,
) -> bool {
    let level = configuration.level.as_deref().unwrap_or(super::LOG_LEVEL_OFF);
    if level == super::LOG_LEVEL_OFF {
        return false;
    }
    let Some(resource) = log_delivery_resource(configuration) else {
        return false;
    };
    LOG_DELIVERY_ACTIONS.iter().all(|action| {
        super::task_authorization::role_is_denied(
            engine,
            role_arn,
            ::std::slice::from_ref(action),
            &resource,
        )
    })
}

/// The stream one execution's events are delivered to.
///
/// Step Functions writes a stream per execution under the `states/` prefix every
/// vended Step Functions log stream carries, so two executions of one state machine
/// never interleave in a stream and a reader can tell which run an event belongs to
/// from the stream alone — while a reader of the GROUP (`FilterLogEvents`) sees them
/// all.
fn log_stream_of(state_machine_name: &str, execution_name: &str) -> String {
    format!("states/{state_machine_name}/{execution_name}")
}

/// One history event as a delivered log message.
///
/// The envelope members are the event's own identity — the id, the type, the event it
/// follows and the instant it happened — beside the execution it belongs to and how
/// many times that execution has been redriven, which is what makes a delivered event
/// traceable back to the history it came from and tells a redriven run from a first
/// one. Every one of them is carried as TEXT, which is the form a vended event's
/// numeric members are delivered in.
///
/// `details` is the event's own detail object ([`log_details`]), the same detail
/// `GetExecutionHistory` reports for that event type, so a consumer can join a log
/// event to the history event it mirrors member for member. The execution DATA inside
/// it (the document a step was handed or produced) is delivered only when the
/// configuration's `includeExecutionData` asks for it: "determines whether execution
/// data is included in your log" — the rest of the detail, which is the transition
/// rather than the payload, is delivered either way.
fn log_message(
    event: &super::HistoryEvent,
    execution: &super::Execution,
    include_execution_data: bool,
) -> String {
    let mut message = ::serde_json::Map::new();
    message.insert("id".to_string(), event.id.to_string().into());
    message.insert("type".to_string(), event.event_type.clone().into());
    let mut details = log_details(event, execution);
    if let Some(details) = details.as_mut() {
        if !include_execution_data {
            for member in PAYLOAD_DETAIL_MEMBERS {
                details.remove(*member);
            }
        }
    }
    if let Some(details) = details {
        message.insert("details".to_string(), ::serde_json::Value::Object(details));
    }
    message.insert(
        "previous_event_id".to_string(),
        event.previous_event_id.to_string().into(),
    );
    message.insert(
        "event_timestamp".to_string(),
        epoch_millis(event.timestamp).to_string().into(),
    );
    message.insert(
        "execution_arn".to_string(),
        execution.arn.to_string().into(),
    );
    message.insert(
        "redrive_count".to_string(),
        execution.redrive_count.to_string().into(),
    );
    ::serde_json::Value::Object(message).to_string()
}

/// The members of a vended event's `details` that carry the execution's own PAYLOAD.
///
/// "Determines whether execution data is included in your log": the data is the
/// document a step was handed or produced — the input, the output, and the request a
/// task's payload template built — together with the `*Details` member that states
/// whether that document was truncated, which the event carries only for the members
/// the model gives one and which says nothing once the document itself is not
/// delivered. Everything else an event reports (the state it happened in, the work it
/// named, the error and the cause of a failure) is the TRANSITION rather than the
/// payload, so a configuration that excludes execution data still delivers it.
const PAYLOAD_DETAIL_MEMBERS: &[&str] = &[
    "input",
    "inputDetails",
    "output",
    "outputDetails",
    "parameters",
];

/// The detail object one history event carries in the log, under the member names the
/// model gives that event type's own detail structure — or `None` for an event type the
/// model gives no detail structure at all.
///
/// It is the SAME detail `GetExecutionHistory` reports for the event, read off the same
/// stored record, because the vended log "mirrors" the execution history: the event
/// type decides which members there are, and a member the record does not carry is left
/// out rather than delivered empty. The two differ only in form — this one is the JSON
/// object a log message carries, that one the modelled structure an API response does.
///
/// The object is reported even when it ends up EMPTY, because the event type having a
/// detail object is a fact of the type rather than of what that event carried: a
/// configuration that excludes the execution data leaves the detail of an event whose
/// only members are the payload with nothing in it, and that is still the event's
/// detail.
fn log_details(
    event: &super::HistoryEvent,
    execution: &super::Execution,
) -> Option<::serde_json::Map<String, ::serde_json::Value>> {
    let mut details = ::serde_json::Map::new();
    let stored = &event.details;
    let event_type = event.event_type.as_str();
    if let Some(name) = event.state_name.as_deref() {
        if event_type.ends_with(super::STATE_ENTERED_SUFFIX) {
            text(&mut details, "name", Some(name));
            payload(&mut details, "input", stored.input.as_deref());
            return Some(details);
        }
        if event_type.ends_with(super::STATE_EXITED_SUFFIX) {
            text(&mut details, "name", Some(name));
            payload(&mut details, "output", stored.output.as_deref());
            // "Map of variable name and value as a serialized JSON representation":
            // the variables in scope as the state left, for a state whose own `Assign`
            // wrote any.
            if !stored.assigned_variables.is_empty() {
                let assigned: ::serde_json::Map<String, ::serde_json::Value> = stored
                    .assigned_variables
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone().into()))
                    .collect();
                details.insert(
                    "assignedVariables".to_string(),
                    ::serde_json::Value::Object(assigned),
                );
                details.insert("assignedVariablesDetails".to_string(), not_truncated());
            }
            return Some(details);
        }
    }
    match event_type {
        super::EVENT_EXECUTION_STARTED => {
            // The input is the one the execution was STARTED with and the role the one
            // it RUNS under, both read off the record for the same reason the history
            // API reads them there: they belong to the execution for its whole life.
            payload(&mut details, "input", Some(&execution.input));
            text(
                &mut details,
                "roleArn",
                Some(execution.ran.role_arn.as_str()).filter(|role| !role.is_empty()),
            );
            text(
                &mut details,
                "stateMachineAliasArn",
                execution.state_machine_alias_arn.as_deref(),
            );
            text(
                &mut details,
                "stateMachineVersionArn",
                execution.state_machine_version_arn.as_deref(),
            );
        }
        super::EVENT_EXECUTION_SUCCEEDED => {
            payload(&mut details, "output", stored.output.as_deref());
        }
        super::EVENT_EXECUTION_FAILED
        | super::EVENT_EXECUTION_ABORTED
        | super::EVENT_EXECUTION_TIMED_OUT => {
            failure(&mut details, stored);
        }
        super::EVENT_EXECUTION_REDRIVEN => {
            number(
                &mut details,
                "redriveCount",
                stored.redrive_count.map(i64::from),
            );
        }
        super::EVENT_ACTIVITY_SCHEDULED => {
            // `resource` is the activity the task was scheduled for and is required, so
            // an event whose record does not name it carries no detail at all — exactly
            // as the history API reports it.
            stored.resource.as_deref()?;
            text(&mut details, "resource", stored.resource.as_deref());
            payload(&mut details, "input", stored.input.as_deref());
            bounds(&mut details, stored);
        }
        super::EVENT_ACTIVITY_STARTED => {
            text(&mut details, "workerName", stored.worker_name.as_deref());
        }
        super::EVENT_ACTIVITY_SUCCEEDED => {
            payload(&mut details, "output", stored.output.as_deref());
        }
        super::EVENT_ACTIVITY_FAILED | super::EVENT_ACTIVITY_TIMED_OUT => {
            failure(&mut details, stored);
        }
        // A `Task` state's own work: every one of its events names the work — both
        // members are required, so an event whose record names neither carries no
        // detail — and the request its payload template built is reported under
        // `parameters` rather than under `input`, exactly as the history API reports it.
        super::EVENT_TASK_SCHEDULED => {
            work(&mut details, stored)?;
            details.insert(
                "region".to_string(),
                super::arn_region(&execution.arn).into(),
            );
            // `parameters` is the request the state's payload template built. The model
            // gives it no `*Details` companion — unlike `input` and `output` — so none
            // is delivered beside it.
            text(&mut details, "parameters", stored.input.as_deref());
            bounds(&mut details, stored);
        }
        super::EVENT_TASK_STARTED => {
            work(&mut details, stored)?;
        }
        super::EVENT_TASK_SUCCEEDED | super::EVENT_TASK_SUBMITTED => {
            work(&mut details, stored)?;
            payload(&mut details, "output", stored.output.as_deref());
        }
        super::EVENT_TASK_FAILED
        | super::EVENT_TASK_TIMED_OUT
        | super::EVENT_TASK_SUBMIT_FAILED => {
            work(&mut details, stored)?;
            failure(&mut details, stored);
        }
        // A `Task` state whose `Resource` is the ARN of a Lambda FUNCTION records the
        // same lifecycle under the `LambdaFunction*` family, whose events name the work
        // by the function's own ARN alone and carry no `resourceType`
        // (`super::asl`'s work-event families). `LambdaFunctionStarted` is the one
        // event of either family the model gives no detail structure at all.
        super::EVENT_LAMBDA_FUNCTION_SCHEDULED => {
            // `resource` is the function the invocation was scheduled on and is
            // required, so an event whose record does not name it carries no detail at
            // all. `LambdaFunctionScheduledEventDetails` declares `timeoutInSeconds`
            // and no heartbeat bound, so only the one the structure has is reported.
            let resource = stored.resource.as_deref()?;
            text(&mut details, "resource", Some(resource));
            payload(&mut details, "input", stored.input.as_deref());
            number(
                &mut details,
                "timeoutInSeconds",
                stored
                    .timeout_seconds
                    .map(|bound| i64::try_from(bound).unwrap_or(i64::MAX)),
            );
        }
        super::EVENT_LAMBDA_FUNCTION_SUCCEEDED => {
            payload(&mut details, "output", stored.output.as_deref());
        }
        super::EVENT_LAMBDA_FUNCTION_FAILED | super::EVENT_LAMBDA_FUNCTION_TIMED_OUT => {
            failure(&mut details, stored);
        }
        // "The size of the array for Map state iterations", which the state knew when
        // it started iterating. It is required, so a record carrying none reports the
        // member's own default.
        super::EVENT_MAP_STATE_STARTED => {
            number(
                &mut details,
                "length",
                Some(stored.length.map_or(0, i64::from)),
            );
        }
        // One `Map` iteration reports its parent state and the index it was handed, and
        // the index is required in the same way.
        super::EVENT_MAP_ITERATION_STARTED
        | super::EVENT_MAP_ITERATION_SUCCEEDED
        | super::EVENT_MAP_ITERATION_FAILED
        | super::EVENT_MAP_ITERATION_ABORTED => {
            text(&mut details, "name", event.state_name.as_deref());
            number(
                &mut details,
                "index",
                Some(stored.index.map_or(0, i64::from)),
            );
        }
        // An expression of a state that could not be evaluated: the error and the
        // cause the state reports, the field of the definition the diagnosis named,
        // and the state it happened in — which is required, so an event whose record
        // names none carries no detail, exactly as the history API reports it.
        super::EVENT_EVALUATION_FAILED => {
            event.state_name.as_deref()?;
            failure(&mut details, stored);
            text(&mut details, "location", stored.location.as_deref());
            text(&mut details, "state", event.state_name.as_deref());
        }
        // Every other event type the model gives no detail structure at all.
        _ => return None,
    }
    Some(details)
}

/// `member` as the detail carries it, for a value the record holds; nothing for one it
/// does not.
fn text(
    into: &mut ::serde_json::Map<String, ::serde_json::Value>,
    member: &str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        into.insert(member.to_string(), value.to_string().into());
    }
}

/// The same for a member that carries a figure.
fn number(
    into: &mut ::serde_json::Map<String, ::serde_json::Value>,
    member: &str,
    value: Option<i64>,
) {
    if let Some(value) = value {
        into.insert(member.to_string(), value.into());
    }
}

/// A PAYLOAD member and the `*Details` member that accompanies it: a document the
/// record holds is delivered whole, so the accompanying member states that it was not
/// truncated.
fn payload(
    into: &mut ::serde_json::Map<String, ::serde_json::Value>,
    member: &str,
    value: Option<&str>,
) {
    let Some(value) = value else {
        return;
    };
    into.insert(member.to_string(), value.to_string().into());
    into.insert(format!("{member}Details"), not_truncated());
}

/// What a `*Details` member states about a document delivered whole: "indicates whether
/// input or output was included in the response", which it was, since nothing here is
/// truncated.
fn not_truncated() -> ::serde_json::Value {
    let mut details = ::serde_json::Map::new();
    details.insert("truncated".to_string(), false.into());
    ::serde_json::Value::Object(details)
}

/// The error and the cause a failed event reports.
fn failure(
    into: &mut ::serde_json::Map<String, ::serde_json::Value>,
    stored: &super::EventDetails,
) {
    text(into, "error", stored.error.as_deref());
    text(into, "cause", stored.cause.as_deref());
}

/// The work a `Task` state's event names: "the service name of the resource in a task
/// state" and "the action of the resource called by a task state". Both are required,
/// so `None` — which ends the detail — for an event whose record names neither.
fn work(
    into: &mut ::serde_json::Map<String, ::serde_json::Value>,
    stored: &super::EventDetails,
) -> Option<()> {
    let resource_type = stored.resource_type.as_deref()?;
    let resource = stored.resource.as_deref()?;
    text(into, "resourceType", Some(resource_type));
    text(into, "resource", Some(resource));
    Some(())
}

/// The bounds the scheduling state declared for the work.
fn bounds(
    into: &mut ::serde_json::Map<String, ::serde_json::Value>,
    stored: &super::EventDetails,
) {
    let seconds = |bound: Option<u64>| bound.map(|bound| i64::try_from(bound).unwrap_or(i64::MAX));
    number(into, "timeoutInSeconds", seconds(stored.timeout_seconds));
    number(
        into,
        "heartbeatInSeconds",
        seconds(stored.heartbeat_seconds),
    );
}

impl ComAmazonawsSfnAwsstepfunctionsState {
    /// Queue what the execution `execution_arn` publishes about having been STARTED,
    /// and — for a record that is already terminal, which a synchronous Express run
    /// is — everything it publishes about having ENDED.
    ///
    /// Called by the one place a started execution enters the store, so an execution
    /// nothing admitted publishes nothing.
    pub(super) fn observe_execution_started(&mut self, execution_arn: &str) {
        let Some(execution) = self.executions.get(execution_arn) else {
            return;
        };
        let queued = execution_datapoints(
            execution,
            &[(EXECUTIONS_STARTED_METRIC, 1.0, COUNT_UNIT)],
            execution.start_date,
        );
        let terminal = execution.is_terminal();
        for datapoint in queued {
            self.pending_observations.queue_datapoint(datapoint);
        }
        if terminal {
            self.observe_execution_settled(execution_arn);
        }
        // The work the execution's own states have already reported is counted here
        // too, because a record that arrives with its history complete — a synchronous
        // Express run — reports every task it ran at the instant it is stored.
        self.observe_task_work();
    }

    /// Queue what the execution `execution_arn` publishes about having been REDRIVEN at
    /// `at`: the series counting the executions a redrive re-ran.
    ///
    /// Called by the one place a redrive is recorded
    /// ([`ComAmazonawsSfnAwsstepfunctionsState::redrive_execution`]), so a redrive the
    /// store refused — one of an execution that is not redrivable, or a repeat of a
    /// client token that already redrove it — is counted in nothing, and an accepted one
    /// exactly once however many times the execution is read afterwards.
    pub(super) fn observe_execution_redriven(&mut self, execution_arn: &str, at: SystemTime) {
        let Some(execution) = self.executions.get(execution_arn) else {
            return;
        };
        let queued = execution_datapoints(
            execution,
            &[(EXECUTIONS_REDRIVEN_METRIC, 1.0, COUNT_UNIT)],
            at,
        );
        for datapoint in queued {
            self.pending_observations.queue_datapoint(datapoint);
        }
    }

    /// Queue what the state machine `state_machine_arn` publishes about how many
    /// VERSIONS it holds, as of this instant.
    ///
    /// The series is a COUNT of what the state machine holds rather than a count of
    /// mutations, so every accepted mutation of its versions reports the count that
    /// mutation left — which is what makes a refused publish observable: it leaves the
    /// count where the last accepted one put it.
    pub(super) fn observe_version_count(&mut self, state_machine_arn: &str, at: SystemTime) {
        #[allow(clippy::cast_precision_loss)]
        let held = self.version_count(state_machine_arn) as f64;
        self.pending_observations.queue_datapoint(Datapoint {
            dimensions: resource_dimensions(state_machine_arn),
            metric: VERSION_COUNT_METRIC,
            value: held,
            unit: COUNT_UNIT,
            at,
        });
    }

    /// Queue what the state machine `state_machine_arn` publishes about how many
    /// ALIASES it holds; see [`Self::observe_version_count`].
    pub(super) fn observe_alias_count(&mut self, state_machine_arn: &str, at: SystemTime) {
        #[allow(clippy::cast_precision_loss)]
        let held = self.aliases_of(state_machine_arn).len() as f64;
        self.pending_observations.queue_datapoint(Datapoint {
            dimensions: resource_dimensions(state_machine_arn),
            metric: ALIAS_COUNT_METRIC,
            value: held,
            unit: COUNT_UNIT,
            at,
        });
    }

    /// Queue what the execution `execution_arn` publishes about having reached a
    /// TERMINAL status: the series for that status, the span it ran for, the billed
    /// series of an Express run, and the history events its state machine's logging
    /// configuration delivers.
    ///
    /// Called by the one place an owed outcome is materialised (`settle`) and by
    /// [`Self::observe_execution_started`] for a record that arrives terminal, so
    /// every execution that ends is counted exactly once.
    pub(super) fn observe_execution_settled(&mut self, execution_arn: &str) {
        let Some(execution) = self.executions.get(execution_arn) else {
            return;
        };
        let Some(metric) = completion_metric(&execution.status) else {
            return;
        };
        let machine_arn = execution.state_machine_arn.clone();
        let stopped = execution.stop_date.unwrap_or(execution.start_date);
        let ran_millis = stopped
            .duration_since(execution.start_date)
            .map(|span| span.as_millis())
            .unwrap_or_default();
        #[allow(clippy::cast_precision_loss)]
        let mut measured: Vec<(&'static str, f64, &'static str)> = vec![
            (metric, 1.0, COUNT_UNIT),
            (EXECUTION_TIME_METRIC, ran_millis as f64, MILLISECONDS_UNIT),
        ];
        // An execution a redrive re-ran is counted in the redriven series of the outcome
        // it reached as well: "The number of times you've redriven an execution" is what
        // makes this attempt one of those.
        if execution.redrive_count > 0 {
            if let Some(redriven) = redriven_completion_metric(&execution.status) {
                measured.push((redriven, 1.0, COUNT_UNIT));
            }
        }
        let machine = self.state_machines.get(&machine_arn);
        let express =
            machine.is_some_and(|machine| machine.sm_type == super::STATE_MACHINE_TYPE_EXPRESS);
        if express {
            // The figures `StartSyncExecution` reports as `billingDetails`, computed
            // from the same place it computes them, so a window summed over either
            // series equals the billing details of the runs in it.
            let billed_duration = super::billed_duration_millis(execution.start_date, stopped);
            let billed_memory = super::billed_memory_mb(execution.measure());
            #[allow(clippy::cast_precision_loss)]
            measured.push((
                EXPRESS_BILLED_DURATION_METRIC,
                billed_duration as f64,
                MILLISECONDS_UNIT,
            ));
            #[allow(clippy::cast_precision_loss)]
            measured.push((
                EXPRESS_BILLED_MEMORY_METRIC,
                billed_memory as f64,
                MEGABYTES_UNIT,
            ));
        }
        let queued = execution_datapoints(execution, &measured, stopped);
        // The events the state machine's logging configuration delivers, selected by
        // its own `level` and carrying the execution data only where it asks for it.
        let delivery = machine.and_then(|machine| {
            let configuration = machine.logging_configuration.as_ref()?;
            let level = configuration.level.as_deref().unwrap_or(super::LOG_LEVEL_OFF);
            let (region, log_group) = log_group_of(configuration)?;
            let events: Vec<(i64, String)> = execution
                .history
                .iter()
                .filter(|event| delivered_at_level(level, &event.event_type))
                .map(|event| {
                    (
                        epoch_millis(event.timestamp),
                        log_message(event, execution, configuration.include_execution_data),
                    )
                })
                .collect();
            (!events.is_empty()).then(|| Delivery {
                log_group,
                region,
                log_stream: log_stream_of(&machine.name, &execution.name),
                events,
            })
        });
        for datapoint in queued {
            self.pending_observations.queue_datapoint(datapoint);
        }
        if let Some(delivery) = delivery {
            self.pending_observations.queue_delivery(delivery);
        }
    }

    /// Queue what the `Task` states of every stored execution publish about the work
    /// they took on the resource their `Resource` named — a LAMBDA FUNCTION or an
    /// ACTIVITY — for the history events that have become reportable since this last
    /// ran.
    ///
    /// An execution's history is what the work really did: an event of it is written as
    /// the walk takes the step it reports and becomes reportable at its own instant
    /// ([`super::Execution::advance_history`]), so the series are counted off those
    /// events rather than off anything a caller said. Each execution remembers how far
    /// it has been counted ([`super::Execution::observed_events`]), so an event is
    /// counted exactly once however many times this runs — the same way a status change
    /// owes exactly one event ([`super::events::record_status_changes`]) — and a
    /// REDRIVEN execution's second attempt is counted beside the first rather than
    /// instead of it.
    ///
    /// Called by [`ComAmazonawsSfnAwsstepfunctionsState::settle`] and by the admission
    /// of a started execution, so no site that advances a history has to announce
    /// itself.
    pub(super) fn observe_task_work(&mut self) {
        // The store is READ first and written afterwards: the scan reads each
        // execution's history and the definition it runs, and the queue and the cursors
        // are written once the scan is done.
        let mut queued: Vec<Datapoint> = Vec::new();
        let mut counted: Vec<(String, usize)> = Vec::new();
        for (arn, execution) in &self.executions {
            if execution.observed_events >= execution.history.len() {
                continue;
            }
            queued.extend(work_datapoints(execution));
            counted.push((arn.clone(), execution.history.len()));
        }
        for (arn, through) in counted {
            if let Some(execution) = self.executions.get_mut(&arn) {
                execution.observed_events = through;
            }
        }
        for datapoint in queued {
            self.pending_observations.queue_datapoint(datapoint);
        }
    }

    /// Take the observations queued so far, leaving the queue empty: what [`publish`]
    /// writes into the owning services' stores once this one's guard is released.
    pub fn take_observations(&mut self) -> Pending {
        ::std::mem::take(&mut self.pending_observations)
    }
}

/// The series ONE execution's own measurements are published in, each identified by the
/// dimension SET of a reference that execution is associated with: the state machine it
/// belongs to, and — for a run started through a QUALIFIED ARN — the alias that routed
/// it and the version it ran.
///
/// "Step Functions associates executions that you start with an alias ARN with that
/// alias and the state machine version used for that execution"
/// ([`super::Execution::state_machine_alias_arn`]), so the run is counted once under
/// each reference it is associated with: the series of a weighted alias are its own
/// traffic, and the series of a version are the traffic that version served, neither of
/// which the state machine's own series can show.
///
/// A reference's series carries the STATE MACHINE's dimension beside the one naming the
/// reference inside it ([`ALIAS_DIMENSION`], [`VERSION_DIMENSION`]) — an alias and a
/// version are named within their state machine, as their own ARNs are — so a reader
/// asking for the state machine is answered all of its traffic and never one alias'.
fn execution_series(execution: &super::Execution) -> Vec<BTreeMap<String, String>> {
    let machine_arn = execution.state_machine_arn.as_str();
    let mut series = vec![state_machine_dimensions(machine_arn)];
    let qualifier = |arn: &Option<String>| {
        arn.as_deref()
            .and_then(|arn| qualifier_of(machine_arn, arn))
            .map(str::to_string)
    };
    if let Some(alias_name) = qualifier(&execution.state_machine_alias_arn) {
        series.push(state_machine_alias_dimensions(machine_arn, &alias_name));
    }
    if let Some(version) = qualifier(&execution.state_machine_version_arn) {
        series.push(state_machine_version_dimensions(machine_arn, &version));
    }
    series
}

/// What `arn` names INSIDE the state machine `state_machine_arn`: the alias name or the
/// version number its qualifier carries, and `None` for an ARN that qualifies another
/// state machine or qualifies nothing.
///
/// A qualified state machine ARN is the state machine's own ARN, a colon and the
/// qualifier ([`super::state_machine_alias_arn`],
/// [`super::state_machine_version_arn`]), so the qualifier is read off that prefix
/// rather than guessed at from the colons.
fn qualifier_of<'a>(state_machine_arn: &str, arn: &'a str) -> Option<&'a str> {
    arn.strip_prefix(state_machine_arn)?
        .strip_prefix(':')
        .filter(|qualifier| !qualifier.is_empty())
}

/// The datapoints `measured` — each a `(series, value, unit)` — owes at `at`, one per
/// series the execution is counted in ([`execution_series`]).
fn execution_datapoints(
    execution: &super::Execution,
    measured: &[(&'static str, f64, &'static str)],
    at: SystemTime,
) -> Vec<Datapoint> {
    let mut queued = Vec::with_capacity(measured.len());
    for dimensions in execution_series(execution) {
        for (metric, value, unit) in measured {
            queued.push(Datapoint {
                dimensions: dimensions.clone(),
                metric,
                value: *value,
                unit,
                at,
            });
        }
    }
    queued
}

/// What one history event of a `Task` state's own work reports about the attempt it is
/// about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The work was handed over.
    Scheduled,
    /// It began.
    Started,
    /// It ended, as reported here.
    Closed(Outcome),
}

/// How one attempt of a `Task` state's work ENDED, which is what decides the series it
/// is counted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Succeeded,
    Failed,
    TimedOut,
}

/// The series ONE KIND of resource a `Task` state takes its work on is counted in: the
/// dimension those series are reported per, and the series of each phase and outcome an
/// attempt of that work reports.
///
/// There is one of these per kind of resource because the series are per kind — a client
/// watching a function fleet and a client watching a worker fleet read different metric
/// names — while what is counted, and when, is the same for both: an attempt is
/// scheduled, starts, and closes as one outcome, and the two spans either side of it are
/// measured between those instants.
#[derive(Debug, Clone, Copy)]
struct WorkSeries {
    /// The dimension the series are reported per, naming the resource the work was taken
    /// on.
    dimension: &'static str,
    /// The series counting the attempts scheduled, started, and closed as each outcome.
    scheduled: &'static str,
    started: &'static str,
    succeeded: &'static str,
    failed: &'static str,
    timed_out: &'static str,
    /// The series measuring the span one attempt stayed scheduled for, the span it ran
    /// for, and the whole span it took.
    schedule_time: &'static str,
    run_time: &'static str,
    time: &'static str,
}

impl WorkSeries {
    /// The series an attempt that ended as `outcome` is counted in.
    fn closed_in(self, outcome: Outcome) -> &'static str {
        match outcome {
            Outcome::Succeeded => self.succeeded,
            Outcome::Failed => self.failed,
            Outcome::TimedOut => self.timed_out,
        }
    }
}

/// The series the work a `Task` state takes on a LAMBDA FUNCTION is counted in.
const LAMBDA_FUNCTION_SERIES: WorkSeries = WorkSeries {
    dimension: LAMBDA_FUNCTION_ARN_DIMENSION,
    scheduled: LAMBDA_FUNCTIONS_SCHEDULED_METRIC,
    started: LAMBDA_FUNCTIONS_STARTED_METRIC,
    succeeded: LAMBDA_FUNCTIONS_SUCCEEDED_METRIC,
    failed: LAMBDA_FUNCTIONS_FAILED_METRIC,
    timed_out: LAMBDA_FUNCTIONS_TIMED_OUT_METRIC,
    schedule_time: LAMBDA_FUNCTION_SCHEDULE_TIME_METRIC,
    run_time: LAMBDA_FUNCTION_RUN_TIME_METRIC,
    time: LAMBDA_FUNCTION_TIME_METRIC,
};

/// The series the work a `Task` state hands to an ACTIVITY's workers is counted in.
const ACTIVITY_SERIES: WorkSeries = WorkSeries {
    dimension: ACTIVITY_ARN_DIMENSION,
    scheduled: ACTIVITIES_SCHEDULED_METRIC,
    started: ACTIVITIES_STARTED_METRIC,
    succeeded: ACTIVITIES_SUCCEEDED_METRIC,
    failed: ACTIVITIES_FAILED_METRIC,
    timed_out: ACTIVITIES_TIMED_OUT_METRIC,
    schedule_time: ACTIVITY_SCHEDULE_TIME_METRIC,
    run_time: ACTIVITY_RUN_TIME_METRIC,
    time: ACTIVITY_TIME_METRIC,
};

/// The phase an event of type `event_type` reports, and `None` for every event that is
/// not a `Task` state reporting its own work.
///
/// Every FAMILY a `Task` state records its work under reports the same phases — the
/// `Task*` events of a service integration's call, the `LambdaFunction*` events of a
/// direct invocation and the `Activity*` events of a task a worker polls for
/// (`super::asl`'s work-event families) — so each phase is read off any of those
/// families' event for it, and which SERIES the phase is counted in is decided by the
/// resource the work was taken on ([`work_on`]) rather than by the event's name.
fn work_phase(event_type: &str) -> Option<Phase> {
    task_phase(event_type).or_else(|| activity_phase(event_type))
}

/// The phase an event of the `Task*` and `LambdaFunction*` families reports.
///
/// The failure of the work and the failure to SUBMIT it are both the attempt ending
/// without doing its work, so both are counted as a failure; running out of the time
/// the state allowed is its own series, exactly as it is its own event.
fn task_phase(event_type: &str) -> Option<Phase> {
    match event_type {
        super::EVENT_TASK_SCHEDULED | super::EVENT_LAMBDA_FUNCTION_SCHEDULED => {
            Some(Phase::Scheduled)
        }
        super::EVENT_TASK_STARTED | super::EVENT_LAMBDA_FUNCTION_STARTED => Some(Phase::Started),
        super::EVENT_TASK_SUCCEEDED | super::EVENT_LAMBDA_FUNCTION_SUCCEEDED => {
            Some(Phase::Closed(Outcome::Succeeded))
        }
        super::EVENT_TASK_FAILED
        | super::EVENT_TASK_SUBMIT_FAILED
        | super::EVENT_LAMBDA_FUNCTION_FAILED => Some(Phase::Closed(Outcome::Failed)),
        super::EVENT_TASK_TIMED_OUT | super::EVENT_LAMBDA_FUNCTION_TIMED_OUT => {
            Some(Phase::Closed(Outcome::TimedOut))
        }
        _ => None,
    }
}

/// The phase an event of the `Activity*` family reports: the life of the task a `Task`
/// state whose `Resource` is an activity ARN queued for a worker to poll for.
fn activity_phase(event_type: &str) -> Option<Phase> {
    match event_type {
        super::EVENT_ACTIVITY_SCHEDULED => Some(Phase::Scheduled),
        super::EVENT_ACTIVITY_STARTED => Some(Phase::Started),
        super::EVENT_ACTIVITY_SUCCEEDED => Some(Phase::Closed(Outcome::Succeeded)),
        super::EVENT_ACTIVITY_FAILED => Some(Phase::Closed(Outcome::Failed)),
        super::EVENT_ACTIVITY_TIMED_OUT => Some(Phase::Closed(Outcome::TimedOut)),
        _ => None,
    }
}

/// Whether an event of type `event_type` CLOSES a `Task` state's CALL: the work that
/// attempt handed over to another service has ended, whatever it ended as.
///
/// The one place a task's call is made asks this to decide which event its measured span
/// belongs on ([`super::EventDetails::work_millis`]), so it asks about the families whose
/// work is a call this service makes; an activity's task is answered by a worker rather
/// than by a call, and the span it ran for is measured between its own events.
pub(super) fn closes_task_work(event_type: &str) -> bool {
    matches!(task_phase(event_type), Some(Phase::Closed(_)))
}

/// The resource a `Task` state's work is taken ON, and the series that work is counted
/// in, for the state of `definition` named `state_name`; `None` for a state whose work
/// is counted in neither family.
///
/// The `Resource` is read out of the very definition the execution interprets
/// ([`super::RanConfiguration::definition`]), and what it names is decided by the one
/// reader of each kind: an ACTIVITY ARN is work handed to the workers that poll that
/// activity ([`super::names_an_activity`]), and a LAMBDA FUNCTION's ARN is the one
/// `Resource` whose work is an invocation of a function the definition itself named
/// ([`super::integration::Call::invokes`]). Everything else — the integration tokens
/// whose work is a call this service makes on the caller's behalf — names neither
/// resource, so it is counted in neither family and the dimensions never cross.
fn work_on(definition: &::serde_json::Value, state_name: &str) -> Option<(WorkSeries, String)> {
    let resource = state_named(definition, state_name)?
        .get("Resource")?
        .as_str()?;
    if super::names_an_activity(resource) {
        return Some((ACTIVITY_SERIES, resource.to_string()));
    }
    let invoked = super::integration::read(resource).ok()?.invokes?;
    Some((LAMBDA_FUNCTION_SERIES, invoked))
}

/// The state named `state_name` of the state machine `definition`, searched through the
/// nested state lists a state declaring a machine of its own carries
/// ([`super::asl::NESTED_MACHINE_MEMBERS`]).
///
/// "States names must be unique within the scope of the whole state machine", so one
/// name names at most one state however deeply the definition nests it.
///
/// The definition is a caller's document, so how deep this descends is bounded by how
/// deep the document is: the walk itself refuses one nested past
/// [`super::asl::MAX_NESTING_DEPTH`], and the document is read by the maintained JSON
/// parser, whose own nesting limit decides what a `Value` can be parsed to at all.
fn state_named<'a>(
    definition: &'a ::serde_json::Value,
    state_name: &str,
) -> Option<&'a ::serde_json::Map<String, ::serde_json::Value>> {
    let states = definition.get("States")?.as_object()?;
    if let Some(state) = states.get(state_name).and_then(::serde_json::Value::as_object) {
        return Some(state);
    }
    for state in states.values() {
        // A member naming ONE machine carries it as an object, and `Branches` carries a
        // list of them: each is a state list of its own to look through.
        let nested = super::asl::NESTED_MACHINE_MEMBERS
            .iter()
            .filter_map(|member| state.get(*member))
            .flat_map(|value| match value.as_array() {
                Some(machines) => machines.iter().collect::<Vec<_>>(),
                None => vec![value],
            });
        for list in nested {
            if let Some(found) = state_named(list, state_name) {
                return Some(found);
            }
        }
    }
    None
}

/// What one attempt at a `Task` state's work has reported so far, for the spans the
/// series below measure.
#[derive(Debug, Default)]
struct Attempt {
    /// The instant the task scheduled it.
    scheduled: Option<SystemTime>,
    /// The instant it started.
    started: Option<SystemTime>,
    /// How long it stayed scheduled, in milliseconds, once it has started.
    waited_millis: Option<f64>,
}

/// How far back of the events already counted one more pass reads, to pick an
/// attempt's spans back up: an attempt that STRADDLES the cursor — scheduled and
/// perhaps started before it, closed after it — reported those two events immediately
/// before it, because one attempt's events are consecutive. Reading that much and no
/// more is what keeps the work of a pass proportional to the events it counts, however
/// long a history grows.
const STRADDLING_EVENTS: usize = 4;

/// Where one pass over the history of `execution` begins: the events already counted,
/// backed up over the ones an attempt straddling the cursor reported
/// ([`STRADDLING_EVENTS`]), stopping at the first event that is no `Task` state's own —
/// which is where the attempt before it ended.
fn scan_from(execution: &super::Execution) -> usize {
    let mut at = execution.observed_events.min(execution.history.len());
    let floor = at.saturating_sub(STRADDLING_EVENTS);
    while at > floor {
        let previous = execution
            .history
            .get(at - 1)
            .is_some_and(|event| work_phase(&event.event_type).is_some());
        if !previous {
            break;
        }
        at -= 1;
    }
    at
}

/// The datapoints the per-resource series owe for the events of `execution` that have
/// become reportable since it was last counted.
///
/// The pass begins a little BEFORE the cursor ([`scan_from`]), because an attempt's
/// span is measured between events that can become reportable at different instants, and
/// counts only the events past the cursor: each one is counted exactly once, at its own
/// instant, with the spans of the attempt it belongs to.
fn work_datapoints(execution: &super::Execution) -> Vec<Datapoint> {
    let from = execution.observed_events;
    let mut queued: Vec<Datapoint> = Vec::new();
    let mut document: Option<::serde_json::Value> = None;
    // The resource each `Task` state's work is taken on, and the series it is counted
    // in, read once per state name.
    let mut taken_on: BTreeMap<String, Option<(WorkSeries, String)>> = BTreeMap::new();
    // What each state's attempt has reported so far. A state is entered again by a
    // `Retry`, a loop and every `Map` iteration, and each of those is one more
    // attempt at the work, so the entry is replaced as the next one is scheduled.
    let mut attempts: BTreeMap<String, Attempt> = BTreeMap::new();
    let scanned = scan_from(execution);
    for (index, event) in execution.history.iter().enumerate().skip(scanned) {
        let Some(phase) = work_phase(&event.event_type) else {
            continue;
        };
        let Some(state_name) = event.state_name.as_deref() else {
            continue;
        };
        let document = document.get_or_insert_with(|| {
            ::serde_json::from_str(&execution.ran.definition).unwrap_or(::serde_json::Value::Null)
        });
        let work = taken_on
            .entry(state_name.to_string())
            .or_insert_with(|| work_on(document, state_name));
        let Some((series, target)) = work.clone() else {
            continue;
        };
        let attempt = attempts.entry(state_name.to_string()).or_default();
        let at = event.timestamp;
        let mut count = |metric: &'static str, value: f64, unit: &'static str| {
            if index >= from {
                queued.push(Datapoint {
                    dimensions: one_dimension(series.dimension, &target),
                    metric,
                    value,
                    unit,
                    at,
                });
            }
        };
        // The span between two instants of one attempt, in milliseconds, which is
        // the unit every one of these series is measured in.
        #[allow(clippy::cast_precision_loss)]
        let span = |from: Option<SystemTime>| {
            from.and_then(|from| at.duration_since(from).ok())
                .map(|span| span.as_millis() as f64)
        };
        match phase {
            Phase::Scheduled => {
                count(series.scheduled, 1.0, COUNT_UNIT);
                *attempt = Attempt {
                    scheduled: Some(at),
                    started: None,
                    waited_millis: None,
                };
            }
            Phase::Started => {
                count(series.started, 1.0, COUNT_UNIT);
                attempt.waited_millis = span(attempt.scheduled);
                if let Some(waited) = attempt.waited_millis {
                    count(series.schedule_time, waited, MILLISECONDS_UNIT);
                }
                attempt.started = Some(at);
            }
            Phase::Closed(outcome) => {
                count(series.closed_in(outcome), 1.0, COUNT_UNIT);
                // How long the work RAN: the span the call it made really took, as the
                // event that closed it measured it, and the span between the two events
                // for work no call of this walk closed — which is every task a worker
                // answered, since no call of this service ran it.
                #[allow(clippy::cast_precision_loss)]
                let ran = event
                    .details
                    .work_millis
                    .map(|millis| millis as f64)
                    .or_else(|| span(attempt.started));
                if let Some(ran) = ran {
                    count(series.run_time, ran, MILLISECONDS_UNIT);
                    // The whole span the attempt took: what it stayed scheduled for,
                    // and what it then ran for.
                    count(
                        series.time,
                        attempt.waited_millis.unwrap_or_default() + ran,
                        MILLISECONDS_UNIT,
                    );
                }
                attempts.remove(state_name);
            }
        }
    }
    queued
}

/// Publish everything this service's store has queued: the metric datapoints into
/// CloudWatch's own store, and the history events into CloudWatch Logs'. Answers how
/// many observations were handed over, which is what tells the clock that drives this
/// store's owed transitions whether it is still moving
/// ([`super::events`]).
///
/// Call it with NO store of this service locked — it takes this service's guard to
/// drain the queue, and then the owning service's to write — so the two are never held
/// at once. An engine carrying neither store has nowhere to publish to, so the
/// observations are dropped rather than kept for a reader that cannot exist.
pub fn publish<Eng: Send + Sync + 'static>(engine: &Eng) -> usize {
    let Some(state) = super::service_state(engine) else {
        return 0;
    };
    let pending = {
        let mut store = state.lock();
        store.take_observations()
    };
    if pending.is_empty() {
        return 0;
    }
    let handed = pending.datapoints.len() + pending.deliveries.len();
    publish_datapoints(engine, &pending.datapoints);
    deliver_events(engine, &pending.deliveries);
    handed
}

/// Append every datapoint to the CloudWatch store of the SAME engine — the store
/// `GetMetricData`, `GetMetricStatistics` and `ListMetrics` answer from — and make the
/// alarms watching those series due, exactly as an ingested `PutMetricData` datapoint
/// does, so an alarm cannot tell a service's own metric from a client's.
fn publish_datapoints<Eng: Send + Sync + 'static>(engine: &Eng, datapoints: &[Datapoint]) {
    if datapoints.is_empty() {
        return;
    }
    let Some(metrics) = cloudwatch::service_state(engine) else {
        return;
    };
    let mut store = metrics.lock();
    for datapoint in datapoints {
        let key = cloudwatch::MetricKey {
            namespace: METRIC_NAMESPACE.to_string(),
            metric_name: datapoint.metric.to_string(),
            dimensions: datapoint.dimensions.clone(),
        };
        let at_secs = epoch_secs(datapoint.at);
        let datum = cloudwatch::StoredDatum {
            timestamp_secs: at_secs,
            sample_count: 1.0,
            sum: datapoint.value,
            minimum: datapoint.value,
            maximum: datapoint.value,
            unit: Some(datapoint.unit.to_string()),
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        let watched = cloudwatch::WrittenSeries {
            key: key.clone(),
            units: vec![datapoint.unit.to_string()],
        };
        // A store already holding its full complement of series drops the observation
        // rather than failing the work it is about: a metric is published off the
        // response path.
        if store.append_datapoints(key, vec![datum]).is_err() {
            ::tracing::warn!(
                metric = datapoint.metric,
                "the CloudWatch store is at capacity; a Step Functions metric was not published"
            );
            continue;
        }
        store.make_alarms_due(&[watched], at_secs);
    }
}

/// Write every delivery into the CloudWatch Logs store of the SAME engine — the store
/// `FilterLogEvents`, `GetLogEvents` and `DescribeLogStreams` answer from.
///
/// The log group a configuration names is CREATED if the account does not hold it yet:
/// the configuration says where the executions' logs go, and a group a reader cannot
/// find is a destination nothing reaches. A group the account ALREADY holds is left
/// exactly as it is — its retention, its tags and the events already in it belong to
/// the account — and so is a group this emulator has no room for, which takes no
/// delivery rather than displacing another one.
///
/// Every factor a caller can grow is admitted against the OWNING service's own
/// ceiling before anything is written: how many groups the store holds
/// ([`logs::MAX_LOG_GROUPS`]), how many streams one group holds
/// ([`logs::MAX_STREAMS_PER_GROUP`], the ceiling that service's own `CreateLogStream`
/// admits one against) and how much one stream retains (its own
/// `enforce_storage_ceiling`), so the product of the executions started and the events
/// each delivers is bounded by that store rather than by the requests made.
fn deliver_events<Eng: Send + Sync + 'static>(engine: &Eng, deliveries: &[Delivery]) {
    if deliveries.is_empty() {
        return;
    }
    let Some(state) = logs::service_state(engine) else {
        return;
    };
    let mut store = state.lock();
    for delivery in deliveries {
        let now_ms = logs::now_ms();
        if !store.log_groups.contains_key(&delivery.log_group)
            && store.log_groups.len() >= logs::MAX_LOG_GROUPS
        {
            ::tracing::warn!(
                log_group = %delivery.log_group,
                "the CloudWatch Logs store is at capacity; a Step Functions log delivery was \
                 not made"
            );
            continue;
        }
        // The data identifiers the data protection policies in force for the group name: such
        // a policy masks what is ingested into the group, by whichever path the events arrive
        // ("Sensitive data is detected and masked when it is ingested into the log group"). A
        // group this delivery is about to create is covered by no group-level policy.
        let data_identifiers = store
            .log_groups
            .get(&delivery.log_group)
            .map(|group| store.data_protection_identifiers_for(group))
            .unwrap_or_default();
        let events: Vec<logs::StoredEvent> = delivery
            .events
            .iter()
            .map(|(timestamp, message)| logs::StoredEvent {
                timestamp: *timestamp,
                masked_message: logs::data_protection::masked(message, &data_identifiers),
                message: message.clone(),
                ingestion_time: now_ms,
                ingestion_seq: store.take_ingestion_seq(),
            })
            .collect();
        let added: i64 = events
            .iter()
            .map(logs::StoredEvent::stored_bytes)
            .fold(0i64, i64::saturating_add);
        let group = store
            .log_groups
            .entry(delivery.log_group.clone())
            .or_insert_with(|| {
                logs::LogGroupRecord::new(&delivery.region, &delivery.log_group, now_ms)
            });
        // A stream is opened per EXECUTION, so how many of them one group holds grows
        // with the executions a caller starts: the delivery is admitted against the
        // same per-group ceiling the owning service's own `CreateLogStream` admits one
        // against ([`logs::MAX_STREAMS_PER_GROUP`]), so no write path can take a group
        // past it. A group already holding its full complement takes no further
        // delivery rather than displacing a stream the account holds.
        if !group.streams.contains_key(&delivery.log_stream)
            && group.streams.len() >= logs::MAX_STREAMS_PER_GROUP
        {
            ::tracing::warn!(
                log_group = %delivery.log_group,
                log_stream = %delivery.log_stream,
                "the log group already holds the maximum number of streams; a Step Functions \
                 log delivery was not made"
            );
            continue;
        }
        let stream = group
            .streams
            .entry(delivery.log_stream.clone())
            .or_insert_with(|| {
                logs::LogStreamRecord::new(
                    &delivery.region,
                    &delivery.log_group,
                    &delivery.log_stream,
                    now_ms,
                )
            });
        stream.events.extend(events);
        // The stream is kept in `(timestamp, ingestion_seq)` order, which is the order
        // its reader pages it in; a run's events are already in that order, so only a
        // delivery that predates events already there needs the sort.
        stream.events.sort_by_key(logs::StoredEvent::key);
        let discarded = stream.enforce_storage_ceiling();
        stream.refresh_event_times();
        stream.last_ingestion_time = Some(now_ms);
        group.stored_bytes = group
            .stored_bytes
            .saturating_add(added)
            .saturating_sub(discarded)
            .max(0);
    }
}

/// The dimension set a series of this service is identified by, for a reader to ask
/// for the same series the publisher wrote.
///
/// `pub` so this crate's tests read the series through the same construction the
/// publisher writes them under.
#[must_use]
pub fn state_machine_dimensions(state_machine_arn: &str) -> BTreeMap<String, String> {
    one_dimension(STATE_MACHINE_ARN_DIMENSION, state_machine_arn)
}

/// The dimension set the series of the executions the ALIAS `alias_name` of
/// `state_machine_arn` routed are identified by: the state machine's own dimension and
/// the alias' ([`ALIAS_DIMENSION`]).
///
/// `pub` for the same reason [`state_machine_dimensions`] is.
#[must_use]
pub fn state_machine_alias_dimensions(
    state_machine_arn: &str,
    alias_name: &str,
) -> BTreeMap<String, String> {
    let mut dimensions = state_machine_dimensions(state_machine_arn);
    dimensions.insert(ALIAS_DIMENSION.to_string(), alias_name.to_string());
    dimensions
}

/// The dimension set the series of the executions the VERSION `version` of
/// `state_machine_arn` served are identified by, built the same way and for the same
/// reason ([`VERSION_DIMENSION`]).
#[must_use]
pub fn state_machine_version_dimensions(
    state_machine_arn: &str,
    version: &str,
) -> BTreeMap<String, String> {
    let mut dimensions = state_machine_dimensions(state_machine_arn);
    dimensions.insert(VERSION_DIMENSION.to_string(), version.to_string());
    dimensions
}

/// The dimension set the series of a `Task` state's work on a LAMBDA FUNCTION are
/// identified by, for the same reason [`state_machine_dimensions`] is `pub`.
#[must_use]
pub fn lambda_function_dimensions(function_arn: &str) -> BTreeMap<String, String> {
    one_dimension(LAMBDA_FUNCTION_ARN_DIMENSION, function_arn)
}

/// The dimension set the series of a `Task` state's work on an ACTIVITY are identified
/// by, for the same reason [`state_machine_dimensions`] is `pub`.
#[must_use]
pub fn activity_dimensions(activity_arn: &str) -> BTreeMap<String, String> {
    one_dimension(ACTIVITY_ARN_DIMENSION, activity_arn)
}

/// The dimension set the series of ONE API of this service are identified by, for the
/// same reason [`state_machine_dimensions`] is `pub`.
#[must_use]
pub fn api_name_dimensions(api_name: &str) -> BTreeMap<String, String> {
    one_dimension(API_NAME_DIMENSION, api_name)
}

/// The dimension set the series counting what a state machine HOLDS are identified by,
/// and the series of the executions one ALIAS routed or one VERSION served
/// ([`execution_series`]), for the same reason [`state_machine_dimensions`] is `pub`.
#[must_use]
pub fn resource_dimensions(resource_arn: &str) -> BTreeMap<String, String> {
    one_dimension(RESOURCE_ARN_DIMENSION, resource_arn)
}

/// The dimension set of a series identified by ONE dimension: `name`, naming `value`.
fn one_dimension(name: &str, value: &str) -> BTreeMap<String, String> {
    [(name.to_string(), value.to_string())]
        .into_iter()
        .collect()
}

/// Count ONE served call of the API `api_name` into [`CONSUMED_CAPACITY_METRIC`], and
/// hand over everything that call left queued.
///
/// A call consumes one unit of the capacity of the API it named, so the sum over a
/// window is the number of calls made in it: that is what lets a client see how close
/// its own traffic is to the quota of an API before the throttling starts, which no
/// series about the executions can show — the capacity is consumed by the REQUEST,
/// whatever it then did.
///
/// Call it with NO store of this service locked, once the call has been served: it is
/// also where the observations that call queued are handed over ([`publish`]), so a
/// mutation's own series reach CloudWatch's store as soon as the call that made it has
/// answered rather than waiting for the next caller.
pub fn meter_api_call<Eng: Send + Sync + 'static>(engine: &Eng, api_name: &str) {
    if let Some(state) = super::service_state(engine) {
        state.lock().pending_observations.queue_datapoint(Datapoint {
            dimensions: api_name_dimensions(api_name),
            metric: CONSUMED_CAPACITY_METRIC,
            value: 1.0,
            unit: COUNT_UNIT,
            at: super::now(),
        });
    }
    publish(engine);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a logging `level` selects the CLASS of history event delivered — `ALL`
    /// every event, `ERROR` the failures, `FATAL` the execution-level ones alone, and
    /// `OFF` none.
    ///
    /// The rows span the three classes of event type (ordinary progress, a failure of
    /// one state's work, the end of the execution) against every value of the enum.
    #[test]
    fn a_level_selects_the_class_of_event_it_delivers() {
        // (event type, delivered at ALL, at ERROR, at FATAL, at OFF)
        let rows = [
            (super::super::EVENT_EXECUTION_STARTED, true, false, false),
            (super::super::EVENT_EXECUTION_SUCCEEDED, true, false, false),
            ("TaskStateEntered", true, false, false),
            ("TaskFailed", true, true, false),
            ("ActivityTimedOut", true, true, false),
            ("MapRunAborted", true, true, false),
            (super::super::EVENT_EXECUTION_FAILED, true, true, true),
            (super::super::EVENT_EXECUTION_ABORTED, true, true, true),
            (super::super::EVENT_EXECUTION_TIMED_OUT, true, true, true),
        ];
        for (event_type, at_all, at_error, at_fatal) in rows {
            for (level, expected) in [
                (LOG_LEVEL_ALL, at_all),
                (LOG_LEVEL_ERROR, at_error),
                (LOG_LEVEL_FATAL, at_fatal),
                (super::super::LOG_LEVEL_OFF, false),
            ] {
                let delivered = delivered_at_level(level, event_type);
                assert_eq!(
                    delivered, expected,
                    "case {event_type:?} at level {level:?}: expected delivered={expected}, got \
                     {delivered}"
                );
            }
        }
    }

    /// RULE: the log group a configuration delivers to is the one its qualified
    /// `logGroupArn` names, in that ARN's own Region; a value that is not such an ARN
    /// names no group.
    #[test]
    fn the_destination_arn_names_the_group_and_its_region() {
        let destination = |arn: &str| crate::ops::types::LoggingConfiguration {
            level: Some(LOG_LEVEL_ALL.to_string()),
            include_execution_data: true,
            destinations: Some(vec![crate::ops::types::LogDestination {
                cloud_watch_logs_log_group: Some(crate::ops::types::CloudWatchLogsLogGroup {
                    log_group_arn: Some(arn.to_string()),
                }),
            }]),
        };
        // One row: the ARN a configuration carries, and the group it names.
        struct Row {
            case: &'static str,
            arn: &'static str,
            /// The Region and the name the ARN names, and `None` for a value that
            /// names no log group at all.
            names: Option<(&'static str, &'static str)>,
        }
        let rows = [
            Row {
                case: "a qualified log group ARN",
                arn: "arn:aws:logs:us-east-1:000000000000:log-group:/aws/vendedlogs/states/one:*",
                names: Some(("us-east-1", "/aws/vendedlogs/states/one")),
            },
            Row {
                case: "another Region's group",
                arn: "arn:aws:logs:eu-west-2:000000000000:log-group:sfn-logs:*",
                names: Some(("eu-west-2", "sfn-logs")),
            },
            Row {
                case: "an ARN with no qualifier",
                arn: "arn:aws:logs:us-east-1:000000000000:log-group:sfn-logs",
                names: None,
            },
            Row {
                case: "an ARN of another resource type",
                arn: "arn:aws:logs:us-east-1:000000000000:destination:sfn-logs:*",
                names: None,
            },
            Row {
                case: "a value that is no ARN",
                arn: "sfn-logs",
                names: None,
            },
        ];
        for Row { case, arn, names } in rows {
            let read = log_group_of(&destination(arn));
            let expected = names.map(|(region, name)| (region.to_string(), name.to_string()));
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: an execution is counted in the completion series of the status it reached,
    /// and a status that is not terminal is counted in none.
    #[test]
    fn a_terminal_status_selects_its_completion_series() {
        for (status, expected) in [
            (super::super::EXECUTION_STATUS_SUCCEEDED, Some(EXECUTIONS_SUCCEEDED_METRIC)),
            (super::super::EXECUTION_STATUS_FAILED, Some(EXECUTIONS_FAILED_METRIC)),
            (super::super::EXECUTION_STATUS_ABORTED, Some(EXECUTIONS_ABORTED_METRIC)),
            (super::super::EXECUTION_STATUS_TIMED_OUT, Some(EXECUTIONS_TIMED_OUT_METRIC)),
            (super::super::EXECUTION_STATUS_RUNNING, None),
            ("PENDING_REDRIVE", None),
        ] {
            let metric = completion_metric(status);
            assert_eq!(
                metric, expected,
                "case {status:?}: expected {expected:?}, got {metric:?}"
            );
        }
    }

    /// RULE: the resource a `Task` state's work is taken on is the one its own
    /// `Resource` names, wherever the definition nests that state, and the series it is
    /// counted in are the ones of that KIND of resource — a function's ARN per function,
    /// an activity's ARN per activity, and a `Resource` naming neither counted in
    /// neither.
    ///
    /// The rows span where a state can live (the top-level list, a `Map` state's
    /// `ItemProcessor`, the legacy `Iterator`, a `Parallel` state's branch) against the
    /// forms a `Resource` takes (a function's ARN, an activity's ARN, an optimized
    /// integration token, an ARN whose region field names no region — so it is the ARN
    /// of no activity this emulator holds — and a state that names no work at all).
    #[test]
    fn a_nested_task_state_names_the_resource_its_work_is_taken_on() {
        const FUNCTION: &str = "arn:aws:lambda:us-east-1:000000000000:function:counted";
        const ACTIVITY: &str = "arn:aws:states:us-east-1:000000000000:activity:counted";
        const BARE_ACTIVITY: &str = "arn:aws:states:::activity:worker";
        let definition = ::serde_json::json!({
            "StartAt": "Top",
            "States": {
                "Top": { "Type": "Task", "Resource": FUNCTION, "Next": "Fan" },
                "Polled": { "Type": "Task", "Resource": ACTIVITY, "End": true },
                "Optimized": {
                    "Type": "Task",
                    "Resource": "arn:aws:states:::lambda:invoke",
                    "End": true,
                },
                "Fan": {
                    "Type": "Map",
                    "ItemProcessor": {
                        "StartAt": "Item",
                        "States": {
                            "Item": { "Type": "Task", "Resource": FUNCTION, "End": true },
                        },
                    },
                    "Next": "Legacy",
                },
                "Legacy": {
                    "Type": "Map",
                    "Iterator": {
                        "StartAt": "Iterated",
                        "States": {
                            "Iterated": { "Type": "Task", "Resource": FUNCTION, "End": true },
                        },
                    },
                    "Next": "Split",
                },
                "Split": {
                    "Type": "Parallel",
                    "Branches": [
                        {
                            "StartAt": "Left",
                            "States": {
                                "Left": { "Type": "Task", "Resource": FUNCTION, "End": true },
                            },
                        },
                        {
                            "StartAt": "Right",
                            "States": {
                                "Right": {
                                    "Type": "Task",
                                    "Resource": BARE_ACTIVITY,
                                    "End": true,
                                },
                            },
                        },
                    ],
                    "Next": "Done",
                },
                "Done": { "Type": "Succeed" },
            },
        });
        // (the state, the dimension its work is counted per and the resource it names)
        let rows = [
            ("Top", Some((LAMBDA_FUNCTION_ARN_DIMENSION, FUNCTION))),
            ("Item", Some((LAMBDA_FUNCTION_ARN_DIMENSION, FUNCTION))),
            ("Iterated", Some((LAMBDA_FUNCTION_ARN_DIMENSION, FUNCTION))),
            ("Left", Some((LAMBDA_FUNCTION_ARN_DIMENSION, FUNCTION))),
            ("Polled", Some((ACTIVITY_ARN_DIMENSION, ACTIVITY))),
            ("Right", None),
            ("Optimized", None),
            ("Done", None),
            ("NoSuchState", None),
        ];
        for (state_name, expected) in rows {
            let read = work_on(&definition, state_name)
                .map(|(series, target)| (series.dimension, target));
            let expected =
                expected.map(|(dimension, target)| (dimension, target.to_string()));
            assert_eq!(
                read, expected,
                "case {state_name:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: a `Task` state's own events report the phase of the attempt they are about
    /// — handed over, begun, or closed as the outcome it reached — in every family such
    /// a state records its work under, and no other event type reports one; and only the
    /// families whose work is a CALL this service makes close that call's measured span.
    #[test]
    fn a_task_event_reports_the_phase_of_its_attempt() {
        // (event type, the phase it reports, whether it closes a task's own call)
        let rows = [
            (
                super::super::EVENT_TASK_SCHEDULED,
                Some(Phase::Scheduled),
                false,
            ),
            (super::super::EVENT_TASK_STARTED, Some(Phase::Started), false),
            (
                super::super::EVENT_TASK_SUCCEEDED,
                Some(Phase::Closed(Outcome::Succeeded)),
                true,
            ),
            (
                super::super::EVENT_TASK_FAILED,
                Some(Phase::Closed(Outcome::Failed)),
                true,
            ),
            (
                super::super::EVENT_TASK_SUBMIT_FAILED,
                Some(Phase::Closed(Outcome::Failed)),
                true,
            ),
            (
                super::super::EVENT_TASK_TIMED_OUT,
                Some(Phase::Closed(Outcome::TimedOut)),
                true,
            ),
            (
                super::super::EVENT_LAMBDA_FUNCTION_SCHEDULED,
                Some(Phase::Scheduled),
                false,
            ),
            (
                super::super::EVENT_LAMBDA_FUNCTION_SUCCEEDED,
                Some(Phase::Closed(Outcome::Succeeded)),
                true,
            ),
            (
                super::super::EVENT_ACTIVITY_SCHEDULED,
                Some(Phase::Scheduled),
                false,
            ),
            (
                super::super::EVENT_ACTIVITY_STARTED,
                Some(Phase::Started),
                false,
            ),
            (
                super::super::EVENT_ACTIVITY_SUCCEEDED,
                Some(Phase::Closed(Outcome::Succeeded)),
                false,
            ),
            (
                super::super::EVENT_ACTIVITY_FAILED,
                Some(Phase::Closed(Outcome::Failed)),
                false,
            ),
            (
                super::super::EVENT_ACTIVITY_TIMED_OUT,
                Some(Phase::Closed(Outcome::TimedOut)),
                false,
            ),
            (super::super::EVENT_TASK_SUBMITTED, None, false),
            (super::super::EVENT_EXECUTION_SUCCEEDED, None, false),
            ("TaskStateEntered", None, false),
        ];
        for (event_type, expected, closes) in rows {
            let phase = work_phase(event_type);
            assert_eq!(
                phase, expected,
                "case {event_type:?}: expected {expected:?}, got {phase:?}"
            );
            let closed = closes_task_work(event_type);
            assert_eq!(
                closed, closes,
                "case {event_type:?}: expected closes_task_work={closes}, got {closed}"
            );
        }
    }

    /// RULE: a terminal status a REDRIVEN execution reached is counted in the redriven
    /// series of that status as well, and a status that is not terminal in none.
    #[test]
    fn a_redriven_execution_is_counted_in_the_redriven_series_of_its_status() {
        for (status, expected) in [
            (
                super::super::EXECUTION_STATUS_SUCCEEDED,
                Some(REDRIVEN_EXECUTIONS_SUCCEEDED_METRIC),
            ),
            (
                super::super::EXECUTION_STATUS_FAILED,
                Some(REDRIVEN_EXECUTIONS_FAILED_METRIC),
            ),
            (
                super::super::EXECUTION_STATUS_ABORTED,
                Some(REDRIVEN_EXECUTIONS_ABORTED_METRIC),
            ),
            (
                super::super::EXECUTION_STATUS_TIMED_OUT,
                Some(REDRIVEN_EXECUTIONS_TIMED_OUT_METRIC),
            ),
            (super::super::EXECUTION_STATUS_RUNNING, None),
            ("PENDING_REDRIVE", None),
        ] {
            let metric = redriven_completion_metric(status);
            assert_eq!(
                metric, expected,
                "case {status:?}: expected {expected:?}, got {metric:?}"
            );
        }
    }
}
