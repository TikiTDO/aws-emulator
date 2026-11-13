// SPDX-License-Identifier: CC0-1.0

//! @op DeleteOpsItem
//! Deletes the OpsItem an id names: the record [`crate::state::ops_items`] holds for it is
//! marked as owed removal and is removed once the delete settles, together with the
//! related-item associations held against it — so a read made straight after this call is
//! still served the OpsItem (the delete is eventually consistent) and one made after the
//! window takes the not-found path. A delete of an id the Region holds no OpsItem under
//! marks nothing and still succeeds, which is what makes the operation idempotent.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `DeleteOpsItem::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteOpsItem`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Delete an OpsItem. You must have permission in Identity and Access Management (IAM) to
//! delete an OpsItem. Note the following important information about this operation.
//! Deleting an OpsItem is irreversible. You can't restore a deleted OpsItem. This operation
//! uses an eventual consistency model, which means the system can take a few minutes to
//! complete this operation. If you delete an OpsItem and immediately call, for example,
//! GetOpsItem, the deleted OpsItem might still appear in the response. This operation is
//! idempotent. The system doesn't throw an exception if you repeatedly call this operation
//! for the same OpsItem. If the first call is successful, all additional calls return the
//! same successful response as the first call. This operation doesn't support cross-account
//! calls. A delegated administrator or management account can't delete OpsItems in other
//! accounts, even if OpsCenter has been set up for cross-account administration. For more
//! information about cross-account administration, see Setting up OpsCenter to centrally
//! manage OpsItems across accounts in the Systems Manager User Guide.
//!
//! ## Input
//!
//! * `OpsItemId` (OpsItemId, pattern ^(oi)-[0-9a-f]{12}$) **required** — The ID of the
//!   OpsItem that you want to delete.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `OpsItemInvalidParameterException` (HTTP 400) — A specified parameter argument isn't
//!   valid. Verify the available arguments and try again.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteOpsItemError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteOpsItemError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ssm#DeleteOpsItem")]
#[derive(Debug)]
pub enum DeleteOpsItemErrorModeled {
    InternalServerError(InternalServerError),
    OpsItemInvalidParameterException(OpsItemInvalidParameterException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_ops_item::DeleteOpsItemError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteOpsItemError = crate::scaffolded::Scaffolded<DeleteOpsItemErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteOpsItemErrorModeled> for DeleteOpsItemError {
    fn from(modeled: DeleteOpsItemErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteOpsItem` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteOpsItem<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteOpsItem<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteOpsItem<Eng>
{
    type Input = DeleteOpsItemInput;
    type Output = DeleteOpsItemOutput;
    type Error = DeleteOpsItemError;

    const NAME: &'static str = "DeleteOpsItem";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_OPS_ITEM_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_OPS_ITEM_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `OpsItemId`'s `^(oi)-[0-9a-f]{12}$` pattern is MODELLED, so the centralized walk
        // has already refused anything outside it: reaching here means the id is well
        // formed and names either an OpsItem this Region holds or none at all.
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteOpsItemErrorModeled::InternalServerError(InternalServerError {
                message: Some("the OpsItem store is unavailable".to_string()),
            })
        })?;

        // An OpsItem is reached through the endpoint of the Region it was created in, so a
        // delete asks for what that Region holds under the id and nothing of any other
        // Region.
        //
        // "This operation uses an eventual consistency model, which means the system can
        // take a few minutes to complete this operation. If you delete an OpsItem and
        // immediately call, for example, GetOpsItem, the deleted OpsItem might still appear
        // in the response": the record is marked as owed removal and is removed at the
        // instant that delete settles
        // (`crate::state::ops_items::OPS_ITEM_DELETE_SETTLES_AFTER`), so a read made
        // straight after this call is served the OpsItem and a read made after the window
        // is refused.
        //
        // This operation models no not-found error, and "the system doesn't throw an
        // exception if you repeatedly call this operation for the same OpsItem. If the first
        // call is successful, all additional calls return the same successful response as
        // the first call": a delete of an id this Region holds no OpsItem under marks
        // nothing and succeeds, so whether a record was there is not reported.
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        store.materialize_due(now);
        store.delete_ops_item(&region, &input.ops_item_id, now);

        Ok(DeleteOpsItemOutput {})
    }
}
