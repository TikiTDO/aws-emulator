// SPDX-License-Identifier: CC0-1.0

//! The `StepFunctions-StartExecution` and `StepFunctions-StartSyncExecution` actions: the
//! workflow run a route starts.
//!
//! The parameters of each are the members of that operation's own input
//! (`smithy_query(query="operation", operation="StartExecution,StartSyncExecution")` on the
//! Step Functions model), which is what the action takes: `stateMachineArn` is the one member
//! either marks required ([`START_EXECUTION_REQUIRED_PARAMETERS`]), so an integration mapping
//! nothing for it names no run at all and the request is answered as an integration that
//! produced no response — never as an execution nothing started — while a request that carries
//! no value for the expression the integration maps onto it is a bad request, which is the
//! parent module's own rule for every action's required parameters.
//!
//! `IncludedData`, which only the synchronous input declares, is NOT among them: an
//! integration mapping it is reported rather than started without it ([`Mapped::beyond`]).
//!
//! The state machine ARN is a parameter of the in-process call and NOT a destination this
//! process connects to: the client is the emulator's own transport, addressed at the Step
//! Functions endpoint of the Region the parent module resolved for this call, so the run is
//! started on the state machine this emulator holds.
//!
//! What the start then does is Step Functions' own: whether the calling role may start an
//! execution of that state machine, whether the state machine's type admits the call
//! ("StartSyncExecution is not available for STANDARD workflows"), which states the definition
//! walks and what the run comes to. Nothing of that is decided here; this module maps the
//! input and makes the call.
//!
//! # The form a start's answer is reported in
//!
//! Both actions are performed over Step Functions' own api, which states an answer as a JSON
//! document (`awsJson1_0`), so that is the form the caller is answered in ([`Performed::json`])
//! — the members the operation's own output shape declares, under their own names, carrying
//! what that service answered and nothing of this gateway's. A client fronting a workflow with
//! an HTTP api reads the `executionArn` of the run it just started, or the synchronous run's
//! `status` and `output`, off this document exactly as it would reading the action's own
//! answer.
//!
//! The two instants an answer carries are written the way this wire writes a timestamp
//! ([`epoch_seconds`]): an `awsJson` protocol states one as epoch SECONDS with a fractional
//! part, which is the single rendering `vendor/smithy-schema-server/src/json.rs` gives a
//! timestamp carrying no `@timestampFormat`, so the number a client reads here is the number
//! it would have read calling the action itself.

use ::aws_smithy_types::date_time::Format;
use ::aws_smithy_types::DateTime;
use serde_json::{json, Map, Value};
use smithy_server_runtime::OperationContext;

use super::credentials::SigningIdentity;
use super::parameters::Mapped;
use super::{Performed, Unperformed};

/// The `IntegrationSubtype` that names each of these actions.
pub const START_EXECUTION_SUBTYPE: &str = "StepFunctions-StartExecution";
pub const START_SYNC_EXECUTION_SUBTYPE: &str = "StepFunctions-StartSyncExecution";

/// Every parameter an integration may map onto a start: the members of `StartExecutionInput`,
/// which `StartSyncExecutionInput` declares as well.
pub const START_EXECUTION_PARAMETERS: &[&str] = &["Input", "Name", "StateMachineArn", "TraceHeader"];

/// The same set for the synchronous start (see the module docs for `IncludedData`, which is
/// left out).
pub const START_SYNC_EXECUTION_PARAMETERS: &[&str] = START_EXECUTION_PARAMETERS;

/// The parameters a start must carry a value for: the one member either input marks required.
pub const START_EXECUTION_REQUIRED_PARAMETERS: &[&str] = &["StateMachineArn"];

/// The same for the synchronous start.
pub const START_SYNC_EXECUTION_REQUIRED_PARAMETERS: &[&str] = START_EXECUTION_REQUIRED_PARAMETERS;

/// The `arnNamespace` Step Functions publishes its endpoint and signs under, which is the
/// endpoint label the in-process client is addressed at.
const STATES_ENDPOINT: &str = "states";

/// The members of the mapping each action reads, spelled as the integration's
/// `RequestParameters` spell them.
const STATE_MACHINE_ARN: &str = "StateMachineArn";
const NAME: &str = "Name";
const INPUT: &str = "Input";
const TRACE_HEADER: &str = "TraceHeader";

/// The state machine the mapping names, or the reason there is no start to make.
///
/// An integration mapping nothing at all for the one required member names no run, which is
/// the "integration produced no response" class rather than a start of some other workflow.
fn state_machine_arn<'m>(mapped: &'m Mapped, subtype: &str) -> Result<&'m str, Unperformed> {
    mapped.get(STATE_MACHINE_ARN).ok_or_else(|| {
        Unperformed::NotPerformed(format!(
            "the integration maps no {STATE_MACHINE_ARN}, which {subtype} requires"
        ))
    })
}

/// One instant as this wire states a timestamp: epoch seconds, with the fractional part the
/// instant carries (see the module docs).
fn epoch_seconds(instant: &DateTime) -> Option<Value> {
    let rendered = instant.fmt(Format::EpochSeconds).ok()?;
    ::serde_json::from_str::<Value>(&rendered)
        .ok()
        .filter(Value::is_number)
}

/// Write the timestamp member `member` of an answer, which every output shape here marks
/// REQUIRED: such a member is never left off, because a document missing one is not an answer
/// that service produces, so an instant this endpoint cannot state is reported as an action
/// whose answer could not be stated at all.
fn required_instant(
    answer: &mut Map<String, Value>,
    member: &str,
    instant: &DateTime,
    subtype: &str,
) -> Result<(), Unperformed> {
    let stated = epoch_seconds(instant).ok_or_else(|| {
        Unperformed::NotPerformed(format!(
            "{subtype} answered a {member} this endpoint cannot state as an instant"
        ))
    })?;
    answer.insert(member.to_string(), stated);
    Ok(())
}

/// Write the string member `member` of an answer, when the service answered one for it.
fn insert_text(answer: &mut Map<String, Value>, member: &str, value: Option<&str>) {
    if let Some(value) = value {
        answer.insert(member.to_string(), json!(value));
    }
}

/// Start the mapped execution, and answer the document Step Functions returned.
pub async fn start_execution(
    ctx: &OperationContext<'_>,
    region: &str,
    identity: Option<&SigningIdentity>,
    mapped: &Mapped,
) -> Result<Performed, Unperformed> {
    let state_machine_arn = state_machine_arn(mapped, START_EXECUTION_SUBTYPE)?;
    let client = action_client!(
        aws_sdk_sfn,
        ctx.callback(),
        STATES_ENDPOINT,
        region,
        identity
    );
    let mut call = client
        .start_execution()
        .state_machine_arn(state_machine_arn);
    if let Some(name) = mapped.get(NAME) {
        call = call.name(name);
    }
    if let Some(input) = mapped.get(INPUT) {
        call = call.input(input);
    }
    if let Some(trace_header) = mapped.get(TRACE_HEADER) {
        call = call.trace_header(trace_header);
    }
    let started = call
        .send()
        .await
        .map_err(|error| super::unperformed(START_EXECUTION_SUBTYPE, &error))?;
    // The members `StartExecutionOutput` declares, both of them required.
    let mut answer = Map::new();
    answer.insert("executionArn".to_string(), json!(started.execution_arn()));
    required_instant(
        &mut answer,
        "startDate",
        started.start_date(),
        START_EXECUTION_SUBTYPE,
    )?;
    Ok(Performed::json(&Value::Object(answer)))
}

/// Start the mapped execution SYNCHRONOUSLY, and answer the document Step Functions returned.
pub async fn start_sync_execution(
    ctx: &OperationContext<'_>,
    region: &str,
    identity: Option<&SigningIdentity>,
    mapped: &Mapped,
) -> Result<Performed, Unperformed> {
    let state_machine_arn = state_machine_arn(mapped, START_SYNC_EXECUTION_SUBTYPE)?;
    let client = action_client!(
        aws_sdk_sfn,
        ctx.callback(),
        STATES_ENDPOINT,
        region,
        identity
    );
    let mut call = client
        .start_sync_execution()
        .state_machine_arn(state_machine_arn);
    if let Some(name) = mapped.get(NAME) {
        call = call.name(name);
    }
    if let Some(input) = mapped.get(INPUT) {
        call = call.input(input);
    }
    if let Some(trace_header) = mapped.get(TRACE_HEADER) {
        call = call.trace_header(trace_header);
    }
    let ran = call
        .send()
        .await
        .map_err(|error| super::unperformed(START_SYNC_EXECUTION_SUBTYPE, &error))?;
    // The members `StartSyncExecutionOutput` declares, in the order it declares them; each
    // one the run answered nothing for is left off, and each one it marks required is stated.
    let mut answer = Map::new();
    answer.insert("executionArn".to_string(), json!(ran.execution_arn()));
    insert_text(&mut answer, "stateMachineArn", ran.state_machine_arn());
    insert_text(&mut answer, "name", ran.name());
    required_instant(
        &mut answer,
        "startDate",
        ran.start_date(),
        START_SYNC_EXECUTION_SUBTYPE,
    )?;
    required_instant(
        &mut answer,
        "stopDate",
        ran.stop_date(),
        START_SYNC_EXECUTION_SUBTYPE,
    )?;
    answer.insert("status".to_string(), json!(ran.status().as_str()));
    insert_text(&mut answer, "error", ran.error());
    insert_text(&mut answer, "cause", ran.cause());
    insert_text(&mut answer, "input", ran.input());
    if let Some(details) = ran.input_details() {
        answer.insert(
            "inputDetails".to_string(),
            json!({ "included": details.included() }),
        );
    }
    insert_text(&mut answer, "output", ran.output());
    if let Some(details) = ran.output_details() {
        answer.insert(
            "outputDetails".to_string(),
            json!({ "included": details.included() }),
        );
    }
    insert_text(&mut answer, "traceHeader", ran.trace_header());
    if let Some(billing) = ran.billing_details() {
        answer.insert(
            "billingDetails".to_string(),
            json!({
                "billedMemoryUsedInMB": billing.billed_memory_used_in_mb(),
                "billedDurationInMilliseconds": billing.billed_duration_in_milliseconds(),
            }),
        );
    }
    Ok(Performed::json(&Value::Object(answer)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an instant is stated as epoch seconds carrying the fractional part it has,
    /// which is how this wire states a timestamp.
    #[test]
    fn an_instant_is_stated_as_the_epoch_seconds_this_wire_states() {
        // Rows differ along the fractional part: none, milliseconds, the epoch itself, and
        // an instant before it.
        let rows: &[(&str, DateTime, Value)] = &[
            ("whole seconds", DateTime::from_secs(1_793_241_840), json!(1_793_241_840_i64)),
            (
                "milliseconds",
                DateTime::from_secs_f64(1_793_241_840.725),
                json!(1_793_241_840.725),
            ),
            ("the epoch itself", DateTime::from_secs(0), json!(0_i64)),
            (
                "an instant before the epoch",
                DateTime::from_secs(-1),
                json!(-1_i64),
            ),
        ];
        for (case, instant, expected) in rows {
            let stated = epoch_seconds(instant);
            assert_eq!(
                stated.as_ref(),
                Some(expected),
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: a mapping that names no state machine names no run at all, and is reported as
    /// an integration that produced no response rather than started elsewhere.
    #[test]
    fn a_mapping_naming_no_state_machine_names_no_run() {
        let unmapped = Mapped::default();
        let reported = state_machine_arn(&unmapped, START_EXECUTION_SUBTYPE);
        let case = "nothing mapped for the required member";
        assert!(
            matches!(reported, Err(Unperformed::NotPerformed(_))),
            "case {case:?}: expected no run to be named, got {reported:?}"
        );
    }
}
