// SPDX-License-Identifier: CC0-1.0

//! The EventBridge event a plan execution PUBLISHES when it starts.
//!
//! Region switch does not only record an execution for a reader to poll: starting one is
//! published onto the account's default event bus, under the `source` this service owns
//! ([`EVENT_SOURCE`]) and the `detail-type` of a plan execution
//! ([`PLAN_EXECUTION_DETAIL_TYPE`]), naming the plan in the envelope's `resources` and the
//! execution in the event's `detail`. A rule on that bus whose `EventPattern` matches then
//! delivers it to that rule's targets, so a client that drives automation off a Region
//! switch — an alarm, a runbook, an audit pipeline — observes the switch starting without
//! calling Region switch again.
//!
//! The event is built from the STORED execution record ([`PlanExecutionEvent::of`]), so
//! every value in it is one a later `GetPlanExecution` reports, and it is published with no
//! store lock held ([`PlanExecutionEvent::publish`]).
//!
//! EventBridge is reached the way every emulated service reaches another: the AWS SDK over
//! the framework's in-process callback, with the ONE client construction
//! [`::service_common::eventbridge_client`] holds, so nothing here opens a socket or names
//! another service's crate. The only thing the address is built from is the Region the
//! execution runs in — one of the two Regions the plan covers, each a value of the
//! modelled `Region` shape — so no part of a request is a destination. Publication is BEST
//! EFFORT — an emulator with no EventBridge mounted has nowhere to publish — so a failure
//! is traced and the operation's own answer stands.

use super::StoredExecution;

/// The `source` of every event Region switch publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// this service's namespace is the one its ARNs carry
/// (`smithy_query(query="service_info", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`:
/// `arnNamespace` `arc-region-switch`).
pub const EVENT_SOURCE: &str = "aws.arc-region-switch";

/// The `detail-type` of the event published for a PLAN EXECUTION.
pub const PLAN_EXECUTION_DETAIL_TYPE: &str = "ARC Region Switch Plan Execution";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// One plan-execution event, rendered from the stored execution and waiting to be
/// published.
#[derive(Debug, Clone)]
pub struct PlanExecutionEvent {
    /// The plan's ARN, which the envelope carries in `resources`: the event names the
    /// resource it is about, which is what a rule filtering on `resources` reads.
    pub plan_arn: String,
    /// The Region the event is published in: the Region the execution runs in.
    pub region: String,
    /// WHEN the execution moved, in epoch milliseconds — the instant the record carries,
    /// not the instant the publication was made.
    pub at: i64,
    /// The rendered `detail` of the event.
    pub detail: ::serde_json::Value,
}

impl PlanExecutionEvent {
    /// The event for `execution` as it now stands.
    ///
    /// Every member is read off the record, so the event reports what the API reports:
    /// which execution, of which plan and which version of it, the action and mode it
    /// runs, the Regions it shifts traffic between, and the state it stands in.
    #[must_use]
    pub fn of(execution: &StoredExecution) -> Self {
        let mut detail = ::serde_json::Map::new();
        detail.insert(
            "executionId".to_string(),
            ::serde_json::Value::String(execution.execution_id.clone()),
        );
        detail.insert(
            "plan".to_string(),
            ::serde_json::Value::String(execution.plan_arn.clone()),
        );
        detail.insert(
            "planVersion".to_string(),
            ::serde_json::Value::String(execution.plan_version.clone()),
        );
        detail.insert(
            "action".to_string(),
            ::serde_json::Value::String(execution.action.clone()),
        );
        detail.insert(
            "mode".to_string(),
            ::serde_json::Value::String(execution.mode.clone()),
        );
        detail.insert(
            "executionState".to_string(),
            ::serde_json::Value::String(execution.execution_state.clone()),
        );
        detail.insert(
            "executionRegion".to_string(),
            ::serde_json::Value::String(execution.execution_region.clone()),
        );
        // A `postRecovery` execution shifts no traffic, so it activates and deactivates
        // nothing: the members the record leaves empty are left out rather than carried
        // as a null standing for a Region.
        for (member, value) in [
            ("activateRegion", execution.activate_region.as_deref()),
            ("deactivateRegion", execution.deactivate_region.as_deref()),
        ] {
            if let Some(value) = value {
                detail.insert(
                    member.to_string(),
                    ::serde_json::Value::String(value.to_string()),
                );
            }
        }
        Self {
            plan_arn: execution.plan_arn.clone(),
            region: execution.execution_region.clone(),
            at: execution.updated_at.to_millis().unwrap_or_default(),
            detail: ::serde_json::Value::Object(detail),
        }
    }

    /// Publish this event onto the account's default event bus in the execution's Region.
    ///
    /// Call it with NO store lock held. Nothing about the answer changes what the caller
    /// of the operation is told, for the reason the module docs give.
    pub async fn publish(self, callback: &::smithy_server_runtime::EmbeddedEmulator) {
        let client = ::service_common::eventbridge_client::eventbridge_client(callback, &self.region);
        let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
            .event_bus_name(DEFAULT_EVENT_BUS)
            .source(EVENT_SOURCE)
            .detail_type(PLAN_EXECUTION_DETAIL_TYPE)
            // The event's `time` is WHEN the execution moved, not when it was published:
            // `PutEventsRequestEntry$Time` is "the time stamp of the event, per RFC3339.
            // If no time stamp is provided, the time stamp of the PutEvents call is used."
            .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                self.at,
            ))
            .resources(&self.plan_arn)
            .detail(self.detail.to_string())
            .build();
        match client.put_events().entries(entry).send().await {
            Ok(answer) if answer.failed_entry_count() > 0 => {
                ::tracing::warn!(
                    plan = %self.plan_arn,
                    region = %self.region,
                    "a Region switch plan-execution event was not accepted by the default \
                     event bus"
                );
            }
            Ok(_) => {}
            Err(error) => {
                ::tracing::warn!(
                    plan = %self.plan_arn,
                    region = %self.region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a Region switch plan-execution event could not be published"
                );
            }
        }
    }
}
