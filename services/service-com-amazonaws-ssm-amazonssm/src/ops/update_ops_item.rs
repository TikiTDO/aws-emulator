// SPDX-License-Identifier: CC0-1.0

//! @op UpdateOpsItem
//! Edits the OpsItem an id names: every member the request carries is written onto the
//! record [`crate::state::ops_items`] holds, every member it omits is left as it was, and
//! the edit advances the OpsItem's version, its last-modified identity and instant, and
//! appends one entry to the trail `ListOpsItemEvents` reports
//! ([`crate::state::ops_item_events`]). A mutating operation, so a later `GetOpsItem`
//! reports what this write persisted.
//!
//! ---
//!
//! # Modeled contract for `UpdateOpsItem`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Edit or change an OpsItem. You must have permission in Identity and Access Management
//! (IAM) to update an OpsItem. For more information, see Set up OpsCenter in the Amazon Web
//! Services Systems Manager User Guide. Operations engineers and IT professionals use
//! Amazon Web Services Systems Manager OpsCenter to view, investigate, and remediate
//! operational issues impacting the performance and health of their Amazon Web Services
//! resources. For more information, see Amazon Web Services Systems Manager OpsCenter in
//! the Amazon Web Services Systems Manager User Guide.
//!
//! ## Input
//!
//! * `ActualEndTime` (DateTime) — The time a runbook workflow ended. Currently reported only
//!   for the OpsItem type /aws/changerequest.
//! * `ActualStartTime` (DateTime) — The time a runbook workflow started. Currently reported
//!   only for the OpsItem type /aws/changerequest.
//! * `Category` (OpsItemCategory, len 1..64, pattern ^(?!\s*$).+$) — Specify a new category
//!   for an OpsItem.
//! * `Description` (OpsItemDescription, len 1..2048, pattern ^[\s\S]*\S[\s\S]*$) —
//!   User-defined text that contains information about the OpsItem, in Markdown format.
//! * `Notifications` (OpsItemNotifications) — The Amazon Resource Name (ARN) of an SNS topic
//!   where notifications are sent when this OpsItem is edited or changed.
//! * `OperationalData` (OpsItemOperationalData) — Add new keys or edit existing key-value
//!   pairs of the OperationalData map in the OpsItem object. Operational data is custom data
//!   that provides useful reference details about the OpsItem. For example, you can specify
//!   log files, error strings, license keys, troubleshooting tips, or other relevant data.
//!   You enter operational data as key-value pairs. The key has a maximum length of 128
//!   characters. The value has a maximum size of 20 KB. Operational data keys can't begin
//!   with the following: amazon, aws, amzn, ssm, /amazon, /aws, /amzn, /ssm. You can choose
//!   to make the data searchable by other users in the account or you can restrict search
//!   access. Searchable data means that all users with access …
//! * `OperationalDataToDelete` (OpsItemOpsDataKeysList) — Keys that you want to remove from
//!   the OperationalData map.
//! * `OpsItemArn` (OpsItemArn, len 20..2048, pattern
//!   ^arn:(aws[a-zA-Z-]*)?:ssm:[a-z0-9-\.]{0,63}:[0-9]{12}:opsitem) — The OpsItem Amazon
//!   Resource Name (ARN).
//! * `OpsItemId` (OpsItemId, pattern ^(oi)-[0-9a-f]{12}$) **required** — The ID of the
//!   OpsItem.
//! * `PlannedEndTime` (DateTime) — The time specified in a change request for a runbook
//!   workflow to end. Currently supported only for the OpsItem type /aws/changerequest.
//! * `PlannedStartTime` (DateTime) — The time specified in a change request for a runbook
//!   workflow to start. Currently supported only for the OpsItem type /aws/changerequest.
//! * `Priority` (OpsItemPriority, range 1..5) — The importance of this OpsItem in relation to
//!   other OpsItems in the system.
//! * `RelatedOpsItems` (RelatedOpsItems) — One or more OpsItems that share something in
//!   common with the current OpsItems. For example, related OpsItems can include OpsItems
//!   with similar error messages, impacted resources, or statuses for the impacted resource.
//! * `Severity` (OpsItemSeverity, len 1..64, pattern ^(?!\s*$).+$) — Specify a new severity
//!   for an OpsItem.
//! * `Status` (OpsItemStatus, one of: Approved | Cancelled | Cancelling |
//!   ChangeCalendarOverrideApproved | ChangeCalendarOverrideRejected | Closed |
//!   CompletedWithFailure | CompletedWithSuccess | Failed | InProgress | Open | Pending |
//!   PendingApproval | …) — The OpsItem status. For more information, see Editing OpsItem
//!   details in the Amazon Web Services Systems Manager User Guide.
//! * `Title` (OpsItemTitle, len 1..1024, pattern ^(?!\s*$).+$) — A short heading that
//!   describes the nature of the OpsItem and the impacted resource.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `OpsItemAccessDeniedException` (HTTP 400) — You don't have permission to view OpsItems
//!   in the specified account. Verify that your account is configured either as a Systems
//!   Manager delegated administrator or that you are logged into the Organizations management
//!   account.
//! * `OpsItemAlreadyExistsException` (HTTP 400) — The OpsItem already exists.
//! * `OpsItemConflictException` (HTTP 400) — The specified OpsItem is in the process of being
//!   deleted.
//! * `OpsItemInvalidParameterException` (HTTP 400) — A specified parameter argument isn't
//!   valid. Verify the available arguments and try again.
//! * `OpsItemLimitExceededException` (HTTP 400) — The request caused OpsItems to exceed one
//!   or more quotas.
//! * `OpsItemNotFoundException` (HTTP 400) — The specified OpsItem ID doesn't exist. Verify
//!   the ID and try again.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateOpsItemError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateOpsItemError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ssm#UpdateOpsItem")]
#[derive(Debug)]
pub enum UpdateOpsItemErrorModeled {
    InternalServerError(InternalServerError),
    OpsItemAccessDeniedException(OpsItemAccessDeniedException),
    OpsItemAlreadyExistsException(OpsItemAlreadyExistsException),
    OpsItemConflictException(OpsItemConflictException),
    OpsItemInvalidParameterException(OpsItemInvalidParameterException),
    OpsItemLimitExceededException(OpsItemLimitExceededException),
    OpsItemNotFoundException(OpsItemNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_ops_item::UpdateOpsItemError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateOpsItemError = crate::scaffolded::Scaffolded<UpdateOpsItemErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateOpsItemErrorModeled> for UpdateOpsItemError {
    fn from(modeled: UpdateOpsItemErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateOpsItem` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateOpsItem<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateOpsItem<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateOpsItem<Eng>
{
    type Input = UpdateOpsItemInput;
    type Output = UpdateOpsItemOutput;
    type Error = UpdateOpsItemError;

    const NAME: &'static str = "UpdateOpsItem";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_OPS_ITEM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_OPS_ITEM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::{ops_item_events, ops_items};

        // `OpsItemId`'s `^(oi)-[0-9a-f]{12}$` pattern, `Title`'s, `Description`'s,
        // `Category`'s and `Severity`'s lengths and patterns, `Priority`'s 1..5 range,
        // `Status`'s enum values and the operational-data key's 1..128 length are MODELLED,
        // so the centralized walk has already refused anything outside them. What is
        // checked here is what the model cannot say: the documented value sets, the
        // operational-data key and value rules, and the account this emulator serves.
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateOpsItemErrorModeled::InternalServerError(InternalServerError {
                message: Some("the OpsItem store is unavailable".to_string()),
            })
        })?;

        // An `OpsItemArn` names the account the OpsItem belongs to, and this emulator
        // serves exactly one: an ARN naming any other account names an OpsItem no edit
        // here may write, the same way `GetOpsItem` reports none of it.
        if let Some(arn) = input.ops_item_arn.as_deref() {
            let names_this_account =
                ::service_common::arn::parse_of(arn, crate::state::ARN_SERVICE)
                    .is_some_and(|parsed| parsed.account == ::engine::emulator_account_id());
            if !names_this_account {
                return Err(UpdateOpsItemErrorModeled::OpsItemAccessDeniedException(
                    OpsItemAccessDeniedException {
                        message: Some(
                            "You don't have permission to view OpsItems in the specified account. \
                             Verify that your account is configured either as a Systems Manager \
                             delegated administrator or that you are logged into the Organizations \
                             management account."
                                .to_string(),
                        ),
                    },
                )
                .into());
            }
        }

        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // A delete that had settled by now was owed whether or not anybody called, so an
        // OpsItem it removed is gone before this edit resolves the id.
        store.materialize_due(now);

        // How Systems Manager words a refusal of an id this Region holds no OpsItem under,
        // stated once beside the store
        // (`crate::state::ops_items::ops_item_not_found_message`).
        let not_found = || {
            UpdateOpsItemError::from(UpdateOpsItemErrorModeled::OpsItemNotFoundException(
                OpsItemNotFoundException {
                    message: Some(ops_items::ops_item_not_found_message()),
                },
            ))
        };
        let Some(held) = store.ops_item(&region, &input.ops_item_id) else {
            return Err(not_found());
        };
        // When the request names BOTH, they must name the one OpsItem: an ARN that is not
        // this OpsItem's names another, which this id does not resolve to.
        if input
            .ops_item_arn
            .as_deref()
            .is_some_and(|arn| arn != held.arn())
        {
            return Err(not_found());
        }
        // An OpsItem whose delete has been asked for and has not settled yet is still
        // reported, but there is nothing left to edit about it: "the specified OpsItem is in
        // the process of being deleted", which is what `OpsItemConflictException` reports.
        if held.is_deleting() {
            return Err(UpdateOpsItemErrorModeled::OpsItemConflictException(
                OpsItemConflictException {
                    message: Some(
                        "The specified OpsItem is in the process of being deleted.".to_string(),
                    ),
                },
            )
            .into());
        }
        let mut record = held.clone();

        // "Add new keys or edit existing key-value pairs of the OperationalData map"
        // against "Keys that you want to remove from the OperationalData map": an edit of
        // the map is a patch of what the OpsItem already holds, never a replacement of it,
        // so the merged map is what the documented key and value rules are held against.
        let additions: ::std::collections::BTreeMap<String, OpsItemDataValue> = input
            .operational_data
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let operational_data = ops_items::merged_operational_data(
            &record.operational_data,
            &additions,
            input.operational_data_to_delete.as_deref().unwrap_or(&[]),
        );
        let rejection = input
            .severity
            .as_deref()
            .and_then(ops_items::severity_rejection)
            .or_else(|| {
                input
                    .category
                    .as_deref()
                    .and_then(ops_items::category_rejection)
            })
            .or_else(|| ops_items::operational_data_rejection(&operational_data));
        if let Some(rejection) = rejection {
            return Err(UpdateOpsItemErrorModeled::OpsItemInvalidParameterException(
                OpsItemInvalidParameterException {
                    parameter_names: Some(vec![rejection.parameter]),
                    message: Some(rejection.message),
                },
            )
            .into());
        }

        // The `/aws/dedup` key is how a caller says "this is the same work item", so an
        // edit may no more point this OpsItem at a work item another OPEN OpsItem already
        // is than a create may open a second one for it — which is what
        // `OpsItemAlreadyExistsException` reports, naming the OpsItem that holds it.
        let dedup = ops_items::dedup_string(&operational_data);
        if let Some(dedup) = dedup.as_deref() {
            if let Some(other) = store
                .open_ops_item_with_dedup(&region, dedup)
                .filter(|other| other.ops_item_id != record.ops_item_id)
            {
                return Err(UpdateOpsItemErrorModeled::OpsItemAlreadyExistsException(
                    OpsItemAlreadyExistsException {
                        message: Some("The OpsItem already exists.".to_string()),
                        ops_item_id: Some(other.ops_item_id.clone()),
                    },
                )
                .into());
            }
        }

        // Every member the request carries is written; a member it omits is left as the
        // OpsItem already holds it, because an edit says what to change and says nothing
        // about the rest.
        record.operational_data = operational_data;
        record.dedup = dedup;
        if let Some(title) = input.title.clone() {
            record.title = title;
        }
        if let Some(description) = input.description.clone() {
            record.description = description;
        }
        if let Some(status) = input.status.clone() {
            record.status = status;
        }
        if let Some(priority) = input.priority {
            record.priority = Some(priority);
        }
        if let Some(category) = input.category.clone() {
            record.category = Some(category);
        }
        if let Some(severity) = input.severity.clone() {
            record.severity = Some(severity);
        }
        if let Some(notifications) = input.notifications.clone() {
            record.notifications = notifications;
        }
        if let Some(related) = input.related_ops_items.clone() {
            record.related_ops_items = related;
        }
        if let Some(at) = input.actual_start_time {
            record.actual_start_time = Some(at);
        }
        if let Some(at) = input.actual_end_time {
            record.actual_end_time = Some(at);
        }
        if let Some(at) = input.planned_start_time {
            record.planned_start_time = Some(at);
        }
        if let Some(at) = input.planned_end_time {
            record.planned_end_time = Some(at);
        }

        // An edit re-derives the OpsItem's version token, so a read either side of it
        // reports two versions and the later one is the greater
        // (`crate::state::ops_items::minted_ops_item_version`); the edit is also what "the
        // ARN of the Amazon Web Services account that last updated the OpsItem" and "the
        // date and time the OpsItem was last updated" then report.
        let edited_by = ::service_common::caller_principal::ambient_user_arn();
        record.version = ops_items::minted_ops_item_version(now, Some(record.version));
        record.last_modified_by = edited_by.clone();
        record.last_modified_time = now;
        let (version, status) = (record.version, record.status.clone());

        // Through the store's ONE write path, so an edit is held to the same bound a create
        // is; replacing a record the store already holds is not growth, so an edit is
        // served at capacity.
        store.insert_ops_item(record).map_err(|limit| {
            UpdateOpsItemError::from(UpdateOpsItemErrorModeled::OpsItemLimitExceededException(
                OpsItemLimitExceededException {
                    resource_types: Some(vec!["OpsItem".to_string()]),
                    limit: i32::try_from(limit).unwrap_or(i32::MAX),
                    limit_type: Some("OpsItem".to_string()),
                    message: Some(format!(
                        "This emulator holds at most {limit} OpsItems across every Region"
                    )),
                },
            ))
        })?;

        // An edit is the write OpsCenter records an OpsItem event about, and the event's
        // detail is read off what this write left persisted rather than off the request.
        store.record_ops_item_event(
            &region,
            &input.ops_item_id,
            ops_item_events::DETAIL_TYPE_UPDATE,
            ops_item_events::written_detail(version, &status),
            &edited_by,
            now,
        );

        Ok(UpdateOpsItemOutput {})
    }
}
