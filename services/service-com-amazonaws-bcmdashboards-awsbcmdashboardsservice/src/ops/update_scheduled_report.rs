// SPDX-License-Identifier: CC0-1.0

//! @op UpdateScheduledReport
//!
//! Updates the stored scheduled report in place, PARTIALLY: "Only the parameters
//! included in the request are updated; all other properties remain unchanged"
//! (`smithy_query(query="doc",shape="UpdateScheduledReport")`). So every optional member
//! the request carries REPLACES the stored one and every member it omits leaves the
//! stored value alone — `name`, `description`, `dashboardArn`,
//! `scheduledReportExecutionRoleArn`, `scheduleConfig`, `widgetIds` and
//! `widgetDateRangeOverride`. A property of a submitted STRUCTURE is named only when that
//! structure carries it, so a submitted `scheduleConfig` is MERGED into the stored one
//! property by property ([`crate::state::updated_schedule_config`]): the expression, the
//! time zone, the active period and the state each move only when the submitted config
//! names them, and nothing on this request can clear or re-default any of them, so an
//! update that only re-crons a report keeps the time zone and the period it was configured
//! with and leaves a paused report paused. `updatedAt` is set to the moment of the call;
//! `createdAt`, the tags, the execution history and the health the last refresh
//! established are properties this request does not include, so they are left untouched
//! (health is established by a refresh, which is `ExecuteScheduledReport`).
//!
//! The two `clear…` flags are the only way to UNSET an optional member — "Set to true to
//! clear existing widgetIds" / "… widgetDateRangeOverride" — since an omitted member
//! means "unchanged". A request that both clears a member and supplies a new value for it
//! cannot hold together, so it is refused with the modelled `ValidationException` (400)
//! and changes nothing.
//!
//! A submitted `scheduleConfig` carries a `schedulePeriod` whose bounds are each held to
//! the rule its own member states: "The start time cannot be more than 5 minutes before the
//! time of the request" and "The maximum allowed value is 3 years from the current time.
//! Setting an end time beyond this limit returns a `ValidationException`"
//! (`smithy_query(query="shape",shape="SchedulePeriod")`), sentences the members write for
//! the create and the update request alike, measured from the instant of THIS call
//! ([`crate::state::schedule_period_refusal`]). Refused that way, the stored record keeps
//! the period it had. A period this call stores carries both its bounds: the two members
//! document their own service-side defaults for the create and the update request alike, so
//! a bound the submitted period omits is resolved — to the time of this request, and to 3
//! years on from it ([`crate::state::resolved_schedule_period`]) — rather than stored
//! absent.
//!
//! The refusals that come before the write:
//!
//! * an `arn` no stored scheduled report answers to is refused with the service's
//!   not-found answer — HTTP 400 with the error code `ResourceNotFoundException` and a
//!   `resourceId` member carrying the requested ARN, as
//!   [`crate::scaffolded::Scaffolded::resource_not_found`] renders it (the model's
//!   `@httpError` for that shape is 404, which is not what the recorded service answers);
//! * a `dashboardArn` no stored dashboard answers to is refused the same way, naming that
//!   ARN: a report can only be scheduled on a dashboard that exists, and this operation
//!   DOES declare the not-found error the model documents for "the specified resource
//!   (dashboard, policy, or widget)" — which is why the refusal here is a not-found while
//!   `CreateScheduledReport`, whose model declares no `ResourceNotFoundException`, words
//!   that same rule as its `ValidationException`.
//!
//! A `widgetIds` entry that names no widget of the dashboard is NOT refused: the
//! configuration is stored and that mismatch surfaces as the report's health
//! (`WIDGET_ID_NOT_FOUND`) at its next refresh, exactly as it does for a report created
//! that way.
//!
//! Being state-changing, it emits the audit record [`crate::state::record_state_change`]
//! writes on the success and on each refusal above.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `UpdateScheduledReport::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateScheduledReport`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bcmdashboards-awsbcmdashboardsservice/smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json")
//! ```
//!
//! Updates an existing scheduled report's properties, including its name, description,
//! schedule configuration, and widget settings. Only the parameters included in the request
//! are updated; all other properties remain unchanged.
//!
//! ## Input
//!
//! * `arn` (ScheduledReportArn, len 20..2048, pattern
//!   ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/(\*|[-a-z0-9]+)$)
//!   **required** — The ARN of the scheduled report to update.
//! * `clearWidgetDateRangeOverride` (Boolean, default false) **required** — Set to true to
//!   clear existing widgetDateRangeOverride.
//! * `clearWidgetIds` (Boolean, default false) **required** — Set to true to clear existing
//!   widgetIds.
//! * `dashboardArn` (DashboardArn, len 20..2048, pattern
//!   ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$) — The ARN of
//!   the dashboard to associate with the scheduled report.
//! * `description` (Description, len 1..200, pattern ^(?!.* {2})[
//!   a-zA-Z0-9.,!?;:@#$%&\-_/\\]*$) — The new description for the scheduled report.
//! * `name` (ScheduledReportName, len 2..50, pattern ^(?!.* {2})[a-zA-Z][a-zA-Z0-9
//!   _-]{0,48}[a-zA-Z0-9_-]$) — The new name for the scheduled report.
//! * `scheduleConfig` (ScheduleConfig) — The updated schedule configuration for the report.
//! * `scheduledReportExecutionRoleArn` (ServiceRoleArn, len 20..2048, pattern
//!   ^arn:aws[-a-z0-9]*:iam::[0-9]{12}:role/[a-zA-Z0-9+=,.@_/-]+$) — The ARN of the IAM role
//!   that the scheduled report uses to execute. Amazon Web Services Billing and Cost
//!   Management Dashboards will assume this IAM role while executing the scheduled report.
//! * `widgetDateRangeOverride` (DateTimeRange) — The date range override to apply to widgets
//!   in the scheduled report.
//! * `widgetIds` (WidgetIdList, len ..1) — The list of widget identifiers to include in the
//!   scheduled report. If not specified, all widgets in the dashboard are included.
//!
//! ## Output
//!
//! * `arn` (ScheduledReportArn, len 20..2048, pattern
//!   ^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:scheduled-report/(\*|[-a-z0-9]+)$)
//!   **required** — The ARN of the updated scheduled report.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permissions to perform
//!   this action. Verify your IAM permissions and any resource policies.
//! * `ConflictException` (HTTP 409) — The request could not be completed due to a conflict
//!   with the current state of the resource. For example, attempting to create a resource
//!   that already exists or is being created.
//! * `InternalServerException` (HTTP 500) — An internal error occurred while processing the
//!   request. Retry your request. If the problem persists, contact Amazon Web Services
//!   Support.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource (dashboard, policy, or
//!   widget) was not found. Verify the ARN and try again.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.
//!   Reduce the frequency of requests and use exponential backoff.
//! * `ValidationException` (HTTP 400) — The input parameters do not satisfy the requirements.
//!   Check the error message for specific validation details.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateScheduledReportError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bcmdashboards-awsbcmdashboardsservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateScheduledReportError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bcmdashboards#UpdateScheduledReport")]
#[derive(Debug)]
pub enum UpdateScheduledReportErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_scheduled_report::UpdateScheduledReportError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateScheduledReportError = crate::scaffolded::Scaffolded<UpdateScheduledReportErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateScheduledReportErrorModeled> for UpdateScheduledReportError {
    fn from(modeled: UpdateScheduledReportErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateScheduledReport` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateScheduledReport<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateScheduledReport<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateScheduledReport<Eng>
{
    type Input = UpdateScheduledReportInput;
    type Output = UpdateScheduledReportOutput;
    type Error = UpdateScheduledReportError;

    const NAME: &'static str = "UpdateScheduledReport";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_SCHEDULED_REPORT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_SCHEDULED_REPORT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateScheduledReportErrorModeled::InternalServerException(InternalServerException {
                message: "the dashboard store is unavailable".to_string(),
            })
        })?;
        let mut state = state.lock();

        let arn = input.arn;

        // The modelled `@pattern` of the report's name and description is applied BEFORE
        // the resource is resolved, which is where the boundary validator would have
        // applied it had its regex engine compiled the negative lookahead both
        // expressions open with (`crate::state::ConstrainedText`): a malformed request is
        // refused as malformed whether or not the ARN it names resolves, and changes
        // nothing on any stored record.
        if let Some(message) = crate::state::text_pattern_refusal(
            crate::state::report_text_members(None, input.name.as_deref(), input.description.as_deref()),
        ) {
            crate::state::record_state_change(ctx, Self::NAME, &arn, "ValidationException");
            return Err(UpdateScheduledReportErrorModeled::ValidationException(
                ValidationException { message },
            )
            .into());
        }

        // A `widgetDateRangeOverride` is a submitted `DateTimeRange`, so it satisfies the
        // same two rules a widget's own time range does — bounds in the form their `type`
        // names, and a period that runs forwards (`crate::state::date_time_range_refusal`)
        // — and, like a malformed name, it is refused whether or not the ARN it names
        // resolves, leaving every stored record as it was.
        if let Some(message) = input.widget_date_range_override.as_ref().and_then(|range| {
            crate::state::date_time_range_refusal("widgetDateRangeOverride", range)
        }) {
            crate::state::record_state_change(ctx, Self::NAME, &arn, "ValidationException");
            return Err(UpdateScheduledReportErrorModeled::ValidationException(
                ValidationException { message },
            )
            .into());
        }

        let now = ::std::time::SystemTime::now();

        // The schedule this request submits is held to the two bounds the model states on
        // the period's own members — a start time at most 5 minutes before the request and
        // an end time at most 3 years past it, sentences written for the create request and
        // this one alike (`crate::state::schedule_period_refusal`). Measured from the
        // instant of THIS call, so what the rules admit is decided by the request that
        // submits the period and not by the one that first created the report; refused
        // before the ARN is resolved, like the other rules on the submitted members, so the
        // stored record keeps the period it had.
        if let Some(message) = input
            .schedule_config
            .as_ref()
            .and_then(|config| crate::state::schedule_period_refusal("scheduleConfig", config, now))
        {
            crate::state::record_state_change(ctx, Self::NAME, &arn, "ValidationException");
            return Err(UpdateScheduledReportErrorModeled::ValidationException(
                ValidationException { message },
            )
            .into());
        }

        // The rules above are about the submitted members alone, so they are applied
        // whether or not the ARN resolves. The resource this call addresses is resolved
        // next, before any rule that reads the stored record or any member is written: an
        // ARN no report is stored under is refused here, and nothing is written on that
        // path.
        if !state.scheduled_reports.contains_key(&arn) {
            crate::state::record_state_change(
                ctx,
                Self::NAME,
                &arn,
                crate::scaffolded::RESOURCE_NOT_FOUND_CODE,
            );
            return Err(crate::scaffolded::Scaffolded::resource_not_found(
                &arn,
                // The request-derived template this operation words its refusal with, the
                // same one every other operation on a scheduled report uses.
                format!("Scheduled report not found: {arn}"),
            ));
        }

        // Clearing a member and supplying a new value for that same member are
        // contradictory instructions, a combination the model cannot express — so it is
        // refused here, with the members named, and the stored record is left as it was.
        for (member, flag, clear, replaced) in [
            (
                "widgetIds",
                "clearWidgetIds",
                input.clear_widget_ids,
                input.widget_ids.is_some(),
            ),
            (
                "widgetDateRangeOverride",
                "clearWidgetDateRangeOverride",
                input.clear_widget_date_range_override,
                input.widget_date_range_override.is_some(),
            ),
        ] {
            if clear && replaced {
                crate::state::record_state_change(ctx, Self::NAME, &arn, "ValidationException");
                return Err(UpdateScheduledReportErrorModeled::ValidationException(
                    ValidationException {
                        message: format!(
                            "{flag}: cannot be true in a request that also specifies {member}"
                        ),
                    },
                )
                .into());
            }
        }

        // A scheduled report reports on a dashboard, so a request that re-points it has
        // to name one this account stores. Checked before the record is borrowed, so a
        // request naming a missing dashboard changes nothing at all — not even the
        // members it got right.
        if let Some(dashboard_arn) = input.dashboard_arn.as_deref() {
            if !state.dashboards.contains_key(dashboard_arn) {
                crate::state::record_state_change(
                    ctx,
                    Self::NAME,
                    &arn,
                    crate::scaffolded::RESOURCE_NOT_FOUND_CODE,
                );
                return Err(crate::scaffolded::Scaffolded::resource_not_found(
                    dashboard_arn,
                    format!("Dashboard not found: {dashboard_arn}"),
                ));
            }
        }

        let Some(report) = state.scheduled_reports.get_mut(&arn) else {
            // Unreachable: the key was present a moment ago under this same lock.
            return Err(crate::scaffolded::Scaffolded::resource_not_found(
                &arn,
                format!("Scheduled report not found: {arn}"),
            ));
        };

        // Present replaces, absent leaves alone — the partial update this operation
        // documents. `description` has no `clear` flag of its own, so an absent one keeps
        // the stored description rather than dropping it.
        if let Some(name) = input.name {
            report.name = name;
        }
        if let Some(description) = input.description {
            report.description = Some(description);
        }
        if let Some(dashboard_arn) = input.dashboard_arn {
            report.dashboard_arn = dashboard_arn;
        }
        if let Some(role_arn) = input.scheduled_report_execution_role_arn {
            report.execution_role_arn = role_arn;
        }
        if let Some(schedule_config) = input.schedule_config {
            // The config the request carries is merged into the stored one property by
            // property: each property it names replaces the stored one and each property
            // it omits is left alone, since this request models no flag that clears or
            // re-defaults any of them (`crate::state::updated_schedule_config`).
            report.schedule_config = crate::state::updated_schedule_config(
                &report.schedule_config,
                schedule_config,
                now,
            );
        }
        if let Some(widget_ids) = input.widget_ids {
            report.widget_ids = Some(widget_ids);
        } else if input.clear_widget_ids {
            // Back to absent, which is "all widgets in the dashboard are included" —
            // distinct on the wire from the empty list a request may also send.
            report.widget_ids = None;
        }
        if let Some(range) = input.widget_date_range_override {
            report.widget_date_range_override = Some(range);
        } else if input.clear_widget_date_range_override {
            report.widget_date_range_override = None;
        }
        report.updated_at = now;
        let changed = report.arn.clone();

        // The record this call changed state, for the audit stream.
        crate::state::record_state_change(ctx, Self::NAME, &changed, "SUCCESS");

        Ok(UpdateScheduledReportOutput { arn: changed })
    }
}
