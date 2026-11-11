// SPDX-License-Identifier: CC0-1.0

//! The report of the automation executions one Region holds: which of the executions this
//! store persisted a request's filters select, and the metadata each selected execution is
//! reported as.
//!
//! "Provides details about all active and terminated Automation executions"
//! (`DescribeAutomationExecutions`), so every execution the Region's endpoint holds is
//! reported whatever status the walk has left it in, narrowed by the criteria the request
//! carries. A filter names a PROPERTY of an execution ([`is_automation_execution_filter_key`])
//! and carries values of that property, so what each key tests is read off the stored
//! execution ([`AutomationExecutionRecord::matches_filter`]) and nothing is reported that
//! the start, the walk or the runbook did not produce.

use crate::ops::types::{AutomationExecutionMetadata, ResolvedTargets};
use crate::state::commands::{
    parse_filter_instant, TARGET_KEY_RESOURCE_GROUP, TARGET_KEY_RESOURCE_GROUP_NAME,
};
use crate::state::ComAmazonawsSsmAmazonssmState;

use super::{invalid_filter_value_message, AutomationExecutionRecord, StepExecutionRecord};

/// The `AutomationExecutionFilterKey` that selects the executions of the runbooks whose
/// name starts with the value.
pub const FILTER_KEY_DOCUMENT_NAME_PREFIX: &str = "DocumentNamePrefix";

/// The `AutomationExecutionFilterKey` that selects the executions an
/// `AutomationExecutionStatus` holds for.
pub const FILTER_KEY_EXECUTION_STATUS: &str = "ExecutionStatus";

/// The `AutomationExecutionFilterKey` that selects ONE execution by its own id.
pub const FILTER_KEY_EXECUTION_ID: &str = "ExecutionId";

/// The `AutomationExecutionFilterKey` that selects the children of one rate-controlled
/// execution — `AutomationExecutionMetadata$ParentAutomationExecutionId`.
pub const FILTER_KEY_PARENT_EXECUTION_ID: &str = "ParentExecutionId";

/// The `AutomationExecutionFilterKey` that selects the executions whose running step runs
/// an action — `AutomationExecutionMetadata$CurrentAction`.
pub const FILTER_KEY_CURRENT_ACTION: &str = "CurrentAction";

/// The `AutomationExecutionFilterKey` that selects the executions that started before an
/// instant.
pub const FILTER_KEY_START_TIME_BEFORE: &str = "StartTimeBefore";

/// The `AutomationExecutionFilterKey` that selects the executions that started after an
/// instant.
pub const FILTER_KEY_START_TIME_AFTER: &str = "StartTimeAfter";

/// The `AutomationExecutionFilterKey` that selects the executions of one `AutomationType`.
pub const FILTER_KEY_AUTOMATION_TYPE: &str = "AutomationType";

/// The `AutomationExecutionFilterKey` that selects the executions carrying a tag of that
/// key, whatever value the tag holds.
pub const FILTER_KEY_TAG_KEY: &str = "TagKey";

/// The `AutomationExecutionFilterKey` that selects the executions targeting a resource
/// group by name.
pub const FILTER_KEY_TARGET_RESOURCE_GROUP: &str = "TargetResourceGroup";

/// The `AutomationExecutionFilterKey` that selects the executions of one
/// `AutomationSubtype` — a Change Manager change request or an access request.
pub const FILTER_KEY_AUTOMATION_SUBTYPE: &str = "AutomationSubtype";

/// The `AutomationExecutionFilterKey` that selects the executions of the Change Manager
/// change request that OpsItem represents.
pub const FILTER_KEY_OPS_ITEM_ID: &str = "OpsItemId";

/// The `AutomationType` of an execution that runs in the one account and Region it was
/// started in: "Specify either Local or CrossAccount" (`AutomationExecutionMetadata$AutomationType`).
/// From the modelled `AutomationType` enum (`crate::ops::types::AUTOMATION_TYPE_VALUES`).
pub const AUTOMATION_TYPE_LOCAL: &str = "Local";

/// The `AutomationType` of an execution that spans Regions and accounts: "CrossAccount is
/// an Automation that runs in multiple Amazon Web Services Regions and Amazon Web Services
/// accounts" (same member, same enum).
pub const AUTOMATION_TYPE_CROSS_ACCOUNT: &str = "CrossAccount";

/// The bounds the modelled `MaxResults` of an execution report carries (range 1..50).
pub const MIN_AUTOMATION_EXECUTION_PAGE_SIZE: i32 = 1;

/// See [`MIN_AUTOMATION_EXECUTION_PAGE_SIZE`]. It is also the page size a request naming no
/// `MaxResults` is served, the way every other read of this service serves one.
pub const MAX_AUTOMATION_EXECUTION_PAGE_SIZE: i32 = 50;

/// The page size an execution report serves, or the refusal message for a `MaxResults`
/// outside the modelled range — which the protocol's boundary validator already refuses, so
/// this is a backstop rather than the first line of defence.
pub fn automation_execution_page_size(max_results: Option<i32>) -> Result<usize, String> {
    crate::state::page_size(
        max_results,
        MIN_AUTOMATION_EXECUTION_PAGE_SIZE,
        MAX_AUTOMATION_EXECUTION_PAGE_SIZE,
    )
}

/// Whether `key` is a filter key an execution report accepts: the modelled
/// `AutomationExecutionFilterKey` enum is that whole set, so nothing else names a property
/// of an automation execution to narrow by.
pub fn is_automation_execution_filter_key(key: &str) -> bool {
    crate::ops::types::AUTOMATION_EXECUTION_FILTER_KEY_VALUES.contains(&key)
}

/// Why `value` is not a value the execution filter key `key` may carry, or `None` when it
/// is one.
///
/// A filter value has to be a legal value of what it filters: a timestamp for the two
/// instant keys, an `AutomationExecutionStatus` for the status key, an `AutomationType` for
/// the type key and an `AutomationSubtype` for the subtype key — each the modelled enum of
/// the member that key filters. A document-name prefix, an id, an action, a tag key and a
/// resource group constrain nothing here: a value no execution carries selects no execution
/// rather than refusing the read.
pub fn automation_execution_filter_value_rejection(key: &str, value: &str) -> Option<String> {
    let takes = |what: String| Some(invalid_filter_value_message(key, value, &what));
    let one_of = |values: &[&str]| format!("one of [{}]", values.join(", "));
    match key {
        FILTER_KEY_START_TIME_BEFORE | FILTER_KEY_START_TIME_AFTER
            if parse_filter_instant(value).is_none() =>
        {
            takes("a timestamp, for example 2024-07-07T00:00:00Z".to_string())
        }
        FILTER_KEY_EXECUTION_STATUS
            if !crate::ops::types::AUTOMATION_EXECUTION_STATUS_VALUES.contains(&value) =>
        {
            takes(one_of(
                crate::ops::types::AUTOMATION_EXECUTION_STATUS_VALUES,
            ))
        }
        FILTER_KEY_AUTOMATION_TYPE
            if !crate::ops::types::AUTOMATION_TYPE_VALUES.contains(&value) =>
        {
            takes(one_of(crate::ops::types::AUTOMATION_TYPE_VALUES))
        }
        FILTER_KEY_AUTOMATION_SUBTYPE
            if !crate::ops::types::AUTOMATION_SUBTYPE_VALUES.contains(&value) =>
        {
            takes(one_of(crate::ops::types::AUTOMATION_SUBTYPE_VALUES))
        }
        _ => None,
    }
}

impl AutomationExecutionRecord {
    /// This execution's `AutomationType`: `CrossAccount` for one the start pointed at other
    /// Regions and accounts — by naming `TargetLocations`, or the URL of a file carrying
    /// them — and `Local` for one that runs where it was started.
    pub fn automation_type(&self) -> &'static str {
        if self.target_locations.is_empty() && self.target_locations_url.is_none() {
            AUTOMATION_TYPE_LOCAL
        } else {
            AUTOMATION_TYPE_CROSS_ACCOUNT
        }
    }

    /// Whether this execution meets the filter `key` = `values`.
    ///
    /// A filter carries one or more values of its key ("The values used to limit the
    /// execution information associated with the filter's key",
    /// `AutomationExecutionFilter$Values`), so the execution meets the filter when it meets
    /// the key's test for ANY of them; the report then keeps the executions that meet EVERY
    /// filter.
    pub fn matches_filter(&self, key: &str, values: &[String]) -> bool {
        values
            .iter()
            .any(|value| self.matches_filter_value(key, value))
    }

    /// Whether this execution meets the filter key `key` at the single value `value`.
    ///
    /// The Change Manager keys filter members only a change request carries — its subtype
    /// and the OpsItem that represents it — and an automation started through
    /// `StartAutomationExecution` is not one, so no execution here carries them and such a
    /// filter selects no execution.
    fn matches_filter_value(&self, key: &str, value: &str) -> bool {
        let started_at_satisfies = |ordering: ::std::cmp::Ordering| {
            parse_filter_instant(value).is_some_and(|bound| self.started_at.cmp(&bound) == ordering)
        };
        // "Key=ResourceGroup,Values=resource-group-name" and the Run Command spelling of
        // the same selection (`Target`'s own doc) are the two forms a start names a
        // resource group by, so a filter on one selects an execution that named either.
        let targets_resource_group = |group: &str| {
            self.targets.iter().any(|target| {
                target.key.as_deref().is_some_and(|key| {
                    key == TARGET_KEY_RESOURCE_GROUP || key == TARGET_KEY_RESOURCE_GROUP_NAME
                }) && target
                    .values
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .any(|named| named == group)
            })
        };
        match key {
            FILTER_KEY_DOCUMENT_NAME_PREFIX => self.document_name.starts_with(value),
            FILTER_KEY_EXECUTION_STATUS => self.status == value,
            FILTER_KEY_EXECUTION_ID => self.execution_id == value,
            FILTER_KEY_PARENT_EXECUTION_ID => self.parent_execution_id.as_deref() == Some(value),
            FILTER_KEY_CURRENT_ACTION => self
                .running_step()
                .is_some_and(|step| step.definition.action == value),
            FILTER_KEY_START_TIME_BEFORE => started_at_satisfies(::std::cmp::Ordering::Less),
            FILTER_KEY_START_TIME_AFTER => started_at_satisfies(::std::cmp::Ordering::Greater),
            FILTER_KEY_AUTOMATION_TYPE => self.automation_type() == value,
            FILTER_KEY_TAG_KEY => self.tags.contains_key(value),
            FILTER_KEY_TARGET_RESOURCE_GROUP => targets_resource_group(value),
            _ => false,
        }
    }

    /// The modelled `AutomationExecutionMetadata` for this execution: "Details about a
    /// specific Automation execution", every member either echoed from the start as it was
    /// received, resolved by the start, or written by the walk. A member nothing has
    /// produced is left unset rather than invented, so this report and
    /// `GetAutomationExecution` agree member for member about the one stored execution.
    pub fn to_automation_execution_metadata(&self) -> AutomationExecutionMetadata {
        let running: Option<&StepExecutionRecord> = self.running_step();
        AutomationExecutionMetadata {
            automation_execution_id: Some(self.execution_id.clone()),
            document_name: Some(self.document_name.clone()),
            document_version: Some(self.document_version.clone()),
            automation_execution_status: Some(self.status.clone()),
            execution_start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            execution_end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
            executed_by: Some(self.executed_by.clone()),
            // "An S3 bucket where execution information is stored": nothing has been
            // written to a bucket for this execution, so there is no file to name.
            log_file: None,
            // "The list of execution outputs as defined in the Automation runbook": an
            // output is a value a step produced, and the actions this emulator runs produce
            // none — the same reading `GetAutomationExecution` reports.
            outputs: None,
            mode: Some(self.mode.clone()),
            parent_automation_execution_id: self.parent_execution_id.clone(),
            current_step_name: running.map(|step| step.definition.name.clone()),
            current_action: running.map(|step| step.definition.action.clone()),
            failure_message: self.failure_message.clone(),
            warning_message: None,
            target_parameter_name: self.target_parameter_name.clone(),
            targets: (!self.targets.is_empty()).then(|| self.targets.clone()),
            target_maps: (!self.target_maps.is_empty()).then(|| self.target_maps.clone()),
            resolved_targets: (!self.resolved_target_values.is_empty()).then(|| ResolvedTargets {
                parameter_values: Some(self.resolved_target_values.clone()),
                truncated: false,
            }),
            max_concurrency: Some(self.max_concurrency.clone()),
            max_errors: Some(self.max_errors.clone()),
            target: self.target.clone(),
            automation_type: Some(self.automation_type().to_string()),
            alarm_configuration: self.alarm_configuration.clone(),
            // No alarm has been evaluated for this execution, so none was invoked.
            triggered_alarms: None,
            target_locations_url: self.target_locations_url.clone(),
            // The Change Manager members: an automation started through
            // `StartAutomationExecution` is not a change request, so it carries no subtype,
            // schedule, runbook list, OpsItem, association or change request name.
            automation_subtype: None,
            scheduled_time: None,
            runbooks: None,
            ops_item_id: None,
            association_id: None,
            change_request_name: None,
        }
    }
}

impl ComAmazonawsSsmAmazonssmState {
    /// Every automation execution `region` holds, in the order a report reads them: the
    /// most recently started first, which is the order Automation reports its history in.
    ///
    /// Two executions started at the one instant — a rate-controlled parent and the
    /// children it released, say — are ordered by their ids, which is what makes the order
    /// total and a page of it resumable.
    pub fn automation_executions_in_order(
        &self,
        region: &str,
    ) -> Vec<&AutomationExecutionRecord> {
        let mut ordered: Vec<&AutomationExecutionRecord> = self
            .automation_executions
            .iter()
            .filter(|((held, _), _)| held == region)
            .map(|(_, execution)| execution)
            .collect();
        ordered.sort_by(|left, right| {
            right
                .started_at
                .cmp(&left.started_at)
                .then_with(|| left.execution_id.cmp(&right.execution_id))
        });
        ordered
    }
}
