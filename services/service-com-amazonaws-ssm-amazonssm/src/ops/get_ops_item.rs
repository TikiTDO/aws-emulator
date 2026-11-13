// SPDX-License-Identifier: CC0-1.0

//! @op GetOpsItem
//! Reports the OpsItem an id names, projected from the record the last write of it
//! persisted in [`crate::state::ops_items`]. A reporting operation: the only write it makes
//! is materialising what the clock already owed — the removal of an OpsItem whose delete
//! had settled by the time this read ran.
//!
//! ---
//!
//! # Modeled contract for `GetOpsItem`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Get information about an OpsItem by using the ID. You must have permission in Identity
//! and Access Management (IAM) to view information about an OpsItem. For more information,
//! see Set up OpsCenter in the Amazon Web Services Systems Manager User Guide. Operations
//! engineers and IT professionals use Amazon Web Services Systems Manager OpsCenter to
//! view, investigate, and remediate operational issues impacting the performance and health
//! of their Amazon Web Services resources. For more information, see Amazon Web Services
//! Systems Manager OpsCenter in the Amazon Web Services Systems Manager User Guide.
//!
//! ## Input
//!
//! * `OpsItemArn` (OpsItemArn, len 20..2048, pattern
//!   ^arn:(aws[a-zA-Z-]*)?:ssm:[a-z0-9-\.]{0,63}:[0-9]{12}:opsitem) — The OpsItem Amazon
//!   Resource Name (ARN).
//! * `OpsItemId` (OpsItemId, pattern ^(oi)-[0-9a-f]{12}$) **required** — The ID of the
//!   OpsItem that you want to get.
//!
//! ## Output
//!
//! * `OpsItem` (OpsItem) — The OpsItem.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `OpsItemAccessDeniedException` (HTTP 400) — You don't have permission to view OpsItems
//!   in the specified account. Verify that your account is configured either as a Systems
//!   Manager delegated administrator or that you are logged into the Organizations management
//!   account.
//! * `OpsItemNotFoundException` (HTTP 400) — The specified OpsItem ID doesn't exist. Verify
//!   the ID and try again.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetOpsItemError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetOpsItemError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ssm#GetOpsItem")]
#[derive(Debug)]
pub enum GetOpsItemErrorModeled {
    InternalServerError(InternalServerError),
    OpsItemAccessDeniedException(OpsItemAccessDeniedException),
    OpsItemNotFoundException(OpsItemNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_ops_item::GetOpsItemError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetOpsItemError = crate::scaffolded::Scaffolded<GetOpsItemErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetOpsItemErrorModeled> for GetOpsItemError {
    fn from(modeled: GetOpsItemErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetOpsItem` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetOpsItem<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetOpsItem<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetOpsItem<Eng>
{
    type Input = GetOpsItemInput;
    type Output = GetOpsItemOutput;
    type Error = GetOpsItemError;

    const NAME: &'static str = "GetOpsItem";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_OPS_ITEM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_OPS_ITEM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `OpsItemId`'s `^(oi)-[0-9a-f]{12}$` pattern and `OpsItemArn`'s pattern and
        // length are MODELLED, so the centralized walk has already refused anything
        // outside them: reaching here means the id is well formed and names either an
        // OpsItem this Region holds or none at all.
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetOpsItemErrorModeled::InternalServerError(InternalServerError {
                message: Some("the OpsItem store is unavailable".to_string()),
            })
        })?;

        // An `OpsItemArn` names the account the OpsItem belongs to, and this emulator
        // serves exactly one: an ARN naming any other account names an OpsItem no read
        // here may report.
        if let Some(arn) = input.ops_item_arn.as_deref() {
            let names_this_account =
                ::service_common::arn::parse_of(arn, crate::state::ARN_SERVICE)
                    .is_some_and(|parsed| parsed.account == ::engine::emulator_account_id());
            if !names_this_account {
                return Err(GetOpsItemErrorModeled::OpsItemAccessDeniedException(
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
        // A delete that had settled by now was owed whether or not anybody called, so the
        // OpsItem it removed is gone before this read resolves the id.
        store.materialize_due(now);
        // How Systems Manager words a refusal of an id this Region holds no OpsItem under,
        // stated once beside the store
        // (`crate::state::ops_items::ops_item_not_found_message`).
        let not_found = || {
            GetOpsItemErrorModeled::OpsItemNotFoundException(OpsItemNotFoundException {
                message: Some(crate::state::ops_items::ops_item_not_found_message()),
            })
        };
        let Some(record) = store.ops_item(&region, &input.ops_item_id) else {
            return Err(not_found().into());
        };
        // When the request names BOTH, they must name the one OpsItem: an ARN that is not
        // this OpsItem's names another, which this id does not resolve to.
        if input
            .ops_item_arn
            .as_deref()
            .is_some_and(|arn| arn != record.arn())
        {
            return Err(not_found().into());
        }

        Ok(GetOpsItemOutput {
            ops_item: Some(record.to_ops_item()),
        })
    }
}
