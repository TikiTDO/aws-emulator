// SPDX-License-Identifier: CC0-1.0

//! @op CreateOpsItem
//! Creates an OpsItem in OpsCenter: it mints the OpsItem's id, persists it in
//! [`crate::state::ops_items`] and answers with the id and the ARN a read resolves it by.
//! The rules it is held to that the model cannot state — the forbidden `Source` strings,
//! the reserved operational-data keys, the value size, the supported OpsItem types,
//! severities and categories, and the `/aws/dedup` collision — live beside the store.
//!
//! ---
//!
//! # Modeled contract for `CreateOpsItem`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Creates a new OpsItem. You must have permission in Identity and Access Management (IAM)
//! to create a new OpsItem. For more information, see Set up OpsCenter in the Amazon Web
//! Services Systems Manager User Guide. Operations engineers and IT professionals use
//! Amazon Web Services Systems Manager OpsCenter to view, investigate, and remediate
//! operational issues impacting the performance and health of their Amazon Web Services
//! resources. For more information, see Amazon Web Services Systems Manager OpsCenter in
//! the Amazon Web Services Systems Manager User Guide.
//!
//! ## Input
//!
//! * `AccountId` (OpsItemAccountId, pattern ^[0-9]{12}$) — The target Amazon Web Services
//!   account where you want to create an OpsItem. To make this call, your account must be
//!   configured to work with OpsItems across accounts. For more information, see Set up
//!   OpsCenter in the Amazon Web Services Systems Manager User Guide.
//! * `ActualEndTime` (DateTime) — The time a runbook workflow ended. Currently reported only
//!   for the OpsItem type /aws/changerequest.
//! * `ActualStartTime` (DateTime) — The time a runbook workflow started. Currently reported
//!   only for the OpsItem type /aws/changerequest.
//! * `Category` (OpsItemCategory, len 1..64, pattern ^(?!\s*$).+$) — Specify a category to
//!   assign to an OpsItem.
//! * `Description` (OpsItemDescription, len 1..2048, pattern ^[\s\S]*\S[\s\S]*$) **required**
//!   — User-defined text that contains information about the OpsItem, in Markdown format.
//!   Provide enough information so that users viewing this OpsItem for the first time
//!   understand the issue.
//! * `Notifications` (OpsItemNotifications) — The Amazon Resource Name (ARN) of an SNS topic
//!   where notifications are sent when this OpsItem is edited or changed.
//! * `OperationalData` (OpsItemOperationalData) — Operational data is custom data that
//!   provides useful reference details about the OpsItem. For example, you can specify log
//!   files, error strings, license keys, troubleshooting tips, or other relevant data. You
//!   enter operational data as key-value pairs. The key has a maximum length of 128
//!   characters. The value has a maximum size of 20 KB. Operational data keys can't begin
//!   with the following: amazon, aws, amzn, ssm, /amazon, /aws, /amzn, /ssm. You can choose
//!   to make the data searchable by other users in the account or you can restrict search
//!   access. Searchable data means that all users with access to the OpsItem Overview page
//!   (as provided by the DescribeOpsItems API operation) can view and …
//! * `OpsItemType` (OpsItemType) — The type of OpsItem to create. Systems Manager supports
//!   the following types of OpsItems: /aws/issue This type of OpsItem is used for default
//!   OpsItems created by OpsCenter. /aws/insight This type of OpsItem is used by OpsCenter
//!   for aggregating and reporting on duplicate OpsItems. /aws/changerequest This type of
//!   OpsItem is used by Change Manager for reviewing and approving or rejecting change
//!   requests. Amazon Web Services Systems Manager Change Manager is no longer open to new
//!   customers. Existing customers can continue to use the service as normal. For more
//!   information, see Amazon Web Services Systems Manager Change Manager availability change.
//! * `PlannedEndTime` (DateTime) — The time specified in a change request for a runbook
//!   workflow to end. Currently supported only for the OpsItem type /aws/changerequest.
//! * `PlannedStartTime` (DateTime) — The time specified in a change request for a runbook
//!   workflow to start. Currently supported only for the OpsItem type /aws/changerequest.
//! * `Priority` (OpsItemPriority, range 1..5) — The importance of this OpsItem in relation to
//!   other OpsItems in the system.
//! * `RelatedOpsItems` (RelatedOpsItems) — One or more OpsItems that share something in
//!   common with the current OpsItems. For example, related OpsItems can include OpsItems
//!   with similar error messages, impacted resources, or statuses for the impacted resource.
//! * `Severity` (OpsItemSeverity, len 1..64, pattern ^(?!\s*$).+$) — Specify a severity to
//!   assign to an OpsItem.
//! * `Source` (OpsItemSource, len 1..128, pattern ^(?!\s*$).+$) **required** — The origin of
//!   the OpsItem, such as Amazon EC2 or Systems Manager. The source name can't contain the
//!   following strings: aws, amazon, and amzn.
//! * `Tags` (TagList, len 0..1000) — Optional metadata that you assign to a resource. Tags
//!   use a key-value pair. For example: Key=Department,Value=Finance To add tags to a new
//!   OpsItem, a user must have IAM permissions for both the ssm:CreateOpsItems operation and
//!   the ssm:AddTagsToResource operation. To add tags to an existing OpsItem, use the
//!   AddTagsToResource operation.
//! * `Title` (OpsItemTitle, len 1..1024, pattern ^(?!\s*$).+$) **required** — A short heading
//!   that describes the nature of the OpsItem and the impacted resource.
//!
//! ## Output
//!
//! * `OpsItemArn` (OpsItemArn, len 20..2048, pattern
//!   ^arn:(aws[a-zA-Z-]*)?:ssm:[a-z0-9-\.]{0,63}:[0-9]{12}:opsitem) — The OpsItem Amazon
//!   Resource Name (ARN).
//! * `OpsItemId` (String) — The ID of the OpsItem.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `OpsItemAccessDeniedException` (HTTP 400) — You don't have permission to view OpsItems
//!   in the specified account. Verify that your account is configured either as a Systems
//!   Manager delegated administrator or that you are logged into the Organizations management
//!   account.
//! * `OpsItemAlreadyExistsException` (HTTP 400) — The OpsItem already exists.
//! * `OpsItemInvalidParameterException` (HTTP 400) — A specified parameter argument isn't
//!   valid. Verify the available arguments and try again.
//! * `OpsItemLimitExceededException` (HTTP 400) — The request caused OpsItems to exceed one
//!   or more quotas.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateOpsItemError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateOpsItemError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ssm#CreateOpsItem")]
#[derive(Debug)]
pub enum CreateOpsItemErrorModeled {
    InternalServerError(InternalServerError),
    OpsItemAccessDeniedException(OpsItemAccessDeniedException),
    OpsItemAlreadyExistsException(OpsItemAlreadyExistsException),
    OpsItemInvalidParameterException(OpsItemInvalidParameterException),
    OpsItemLimitExceededException(OpsItemLimitExceededException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_ops_item::CreateOpsItemError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateOpsItemError = crate::scaffolded::Scaffolded<CreateOpsItemErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateOpsItemErrorModeled> for CreateOpsItemError {
    fn from(modeled: CreateOpsItemErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateOpsItem` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateOpsItem<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateOpsItem<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateOpsItem<Eng>
{
    type Input = CreateOpsItemInput;
    type Output = CreateOpsItemOutput;
    type Error = CreateOpsItemError;

    const NAME: &'static str = "CreateOpsItem";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_OPS_ITEM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_OPS_ITEM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::ops_items;

        // `Title`, `Description` and `Source`'s lengths and patterns, `Priority`'s 1..5
        // range, the operational-data key's 1..128 length and `AccountId`'s 12-digit
        // pattern are MODELLED constraints, so the centralized walk has already refused
        // anything outside them. What is checked here is what the model cannot say: the
        // documented value sets and name rules, the operational-data value size, the
        // account this emulator serves, and the store's own bound.
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateOpsItemErrorModeled::InternalServerError(InternalServerError {
                message: Some("the OpsItem store is unavailable".to_string()),
            })
        })?;

        // "The target Amazon Web Services account where you want to create an OpsItem. To
        // make this call, your account must be configured to work with OpsItems across
        // accounts." This emulator serves exactly one account and is configured with no
        // other, so an OpsItem may be created in that account alone.
        if input
            .account_id
            .as_deref()
            .is_some_and(|account| account != ::engine::emulator_account_id())
        {
            return Err(CreateOpsItemErrorModeled::OpsItemAccessDeniedException(
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

        // A member the request OMITS gets the documented default; one it sends is held to
        // the documented value set, so a value Systems Manager does not support is never
        // served as if it were supported.
        let ops_item_type = input
            .ops_item_type
            .clone()
            .unwrap_or_else(|| ops_items::DEFAULT_OPS_ITEM_TYPE.to_string());
        let operational_data: ::std::collections::BTreeMap<String, OpsItemDataValue> = input
            .operational_data
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let rejection = ops_items::ops_item_type_rejection(&ops_item_type)
            .or_else(|| ops_items::source_rejection(&input.source))
            .or_else(|| {
                input
                    .severity
                    .as_deref()
                    .and_then(ops_items::severity_rejection)
            })
            .or_else(|| {
                input
                    .category
                    .as_deref()
                    .and_then(ops_items::category_rejection)
            })
            .or_else(|| ops_items::operational_data_rejection(&operational_data));
        if let Some(rejection) = rejection {
            return Err(CreateOpsItemErrorModeled::OpsItemInvalidParameterException(
                OpsItemInvalidParameterException {
                    parameter_names: Some(vec![rejection.parameter]),
                    message: Some(rejection.message),
                },
            )
            .into());
        }

        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // A delete that had settled by now was owed whether or not anybody called, so an
        // OpsItem it removed does not collide with this create's dedup string.
        store.materialize_due(now);

        // The `/aws/dedup` key is how a caller says "this is the same work item": an
        // OpsItem already OPEN in this Region under the same dedup string is that work
        // item, so the create reports it rather than opening a second one. An OpsItem that
        // has been resolved or closed is finished work and no longer collides.
        let dedup = ops_items::dedup_string(&operational_data);
        if let Some(dedup) = dedup.as_deref() {
            if let Some(held) = store.open_ops_item_with_dedup(&region, dedup) {
                return Err(CreateOpsItemErrorModeled::OpsItemAlreadyExistsException(
                    OpsItemAlreadyExistsException {
                        message: Some("The OpsItem already exists.".to_string()),
                        ops_item_id: Some(held.ops_item_id.clone()),
                    },
                )
                .into());
            }
        }

        // The emulator's own bound on the store, named as the emulator's: the published
        // OpsCenter quota is not restated here. The store's one write path enforces it, so
        // the refusal below is reported for whichever of the two reaches it first: the
        // check before the id mint, which keeps a refused create from advancing the mint
        // counter, or the insert itself.
        let limit_exceeded = |limit: usize| {
            CreateOpsItemError::from(CreateOpsItemErrorModeled::OpsItemLimitExceededException(
                OpsItemLimitExceededException {
                    resource_types: Some(vec!["OpsItem".to_string()]),
                    limit: i32::try_from(limit).unwrap_or(i32::MAX),
                    limit_type: Some("OpsItem".to_string()),
                    message: Some(format!(
                        "This emulator holds at most {limit} OpsItems across every Region"
                    )),
                },
            ))
        };
        if store.ops_items_at_capacity() {
            return Err(limit_exceeded(ops_items::MAX_OPS_ITEMS));
        }

        // "The ARN of the Amazon Web Services account that created the OpsItem": this
        // create ran for the one principal this emulator's credential speaks for.
        let created_by = ::service_common::caller_principal::ambient_user_arn();
        let record = ops_items::OpsItemRecord {
            ops_item_id: store.next_ops_item_id(now),
            region: region.clone(),
            title: input.title.clone(),
            description: input.description.clone(),
            source: input.source.clone(),
            ops_item_type,
            // "An OpsItem is a work item" nobody has worked yet, so a created one is open.
            status: ops_items::OPS_ITEM_STATUS_OPEN.to_string(),
            priority: input.priority,
            category: input.category.clone(),
            severity: input.severity.clone(),
            notifications: input.notifications.clone().unwrap_or_default(),
            related_ops_items: input.related_ops_items.clone().unwrap_or_default(),
            operational_data,
            dedup,
            actual_start_time: input.actual_start_time,
            actual_end_time: input.actual_end_time,
            planned_start_time: input.planned_start_time,
            planned_end_time: input.planned_end_time,
            tags: crate::state::to_btree_map(input.tags.clone()),
            created_by: created_by.clone(),
            created_time: now,
            // Nothing has edited this OpsItem yet, so its last edit IS its create, and the
            // version token this create mints is the one a later edit advances past.
            last_modified_by: created_by.clone(),
            last_modified_time: now,
            version: ops_items::minted_ops_item_version(now, None),
            // Nothing has asked for this OpsItem to be deleted.
            delete_due: None,
        };
        let (ops_item_id, ops_item_arn) = (record.ops_item_id.clone(), record.arn());
        store.insert_ops_item(record).map_err(limit_exceeded)?;

        // Opening an OpsItem records no entry in its event trail: what
        // `ListOpsItemEvents` reports is the events OpsCenter itself recorded about an
        // OpsItem, and the trail of one just created holds none — so nothing is appended
        // here, and that listing reports an empty page until an event is recorded.

        Ok(CreateOpsItemOutput {
            ops_item_id: Some(ops_item_id),
            ops_item_arn: Some(ops_item_arn),
        })
    }
}
