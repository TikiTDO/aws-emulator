// SPDX-License-Identifier: CC0-1.0

//! @op DeleteEksAnywhereSubscription
//! IMPLEMENTED. Removes the [`crate::state::EksAnywhereSubscriptionRecord`] the
//! request addresses — "deletes an expired or inactive subscription... removes them
//! from list/describe API responses" — and answers with the record it removed.
//!
//! The rules it enforces:
//!
//! * a subscription is addressed by the id the create minted and belongs to the
//!   Region it was created in, so an id this Region carries no subscription for is
//!   the `ResourceNotFoundException` the subscription reads answer
//!   ([`crate::state::subscription_not_found`]);
//! * only an expired or inactive subscription is removed by this API — a live one
//!   "can only be cancelled within 7 days of creation... by creating a ticket in the
//!   Amazon Web Services Support Center" — so a subscription in any other status is
//!   `InvalidRequestException` naming the subscription the service resolved, and
//!   stays exactly as it was ([`crate::state::subscription_delete_refusal`]);
//! * a delete this record has already taken is answered with that record again,
//!   rather than reported as a subscription that was never there.
//!
//! An accepted delete reports `DELETING` for the span the clock owes it, after which
//! the store no longer holds the subscription; nothing sleeps for it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteEksAnywhereSubscription`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Deletes an expired or inactive subscription. Deleting inactive subscriptions removes
//! them from the Amazon Web Services Management Console view and from list/describe API
//! responses. Subscriptions can only be cancelled within 7 days of creation and are
//! cancelled by creating a ticket in the Amazon Web Services Support Center.
//!
//! ## Input
//!
//! * `id` (String) **required** — The ID of the subscription.
//!
//! ## Output
//!
//! * `subscription` (EksAnywhereSubscription) — The full description of the subscription to
//!   be deleted.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteEksAnywhereSubscriptionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteEksAnywhereSubscriptionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DeleteEksAnywhereSubscription")]
#[derive(Debug)]
pub enum DeleteEksAnywhereSubscriptionErrorModeled {
    ClientException(ClientException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_eks_anywhere_subscription::DeleteEksAnywhereSubscriptionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteEksAnywhereSubscriptionError = crate::scaffolded::Scaffolded<DeleteEksAnywhereSubscriptionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteEksAnywhereSubscriptionErrorModeled> for DeleteEksAnywhereSubscriptionError {
    fn from(modeled: DeleteEksAnywhereSubscriptionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteEksAnywhereSubscription` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteEksAnywhereSubscription<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteEksAnywhereSubscription<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteEksAnywhereSubscription<Eng>
{
    type Input = DeleteEksAnywhereSubscriptionInput;
    type Output = DeleteEksAnywhereSubscriptionOutput;
    type Error = DeleteEksAnywhereSubscriptionError;

    const NAME: &'static str = "DeleteEksAnywhereSubscription";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_EKS_ANYWHERE_SUBSCRIPTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_EKS_ANYWHERE_SUBSCRIPTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();
        let store = state::service_state(&*self.engine).ok_or_else(|| {
            DeleteEksAnywhereSubscriptionError::from(
                DeleteEksAnywhereSubscriptionErrorModeled::ServerException(ServerException {
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // The subscription this request addresses, in the Region the request is signed
        // for. An id this Region carries no subscription for is the 404 this operation
        // models, worded for the resource it addresses and naming nothing from the
        // request ([`state::subscription_not_found`]).
        let Some(record) = guard
            .eks_anywhere_subscriptions
            .get_mut(&(region, input.id.clone()))
        else {
            return Err(
                DeleteEksAnywhereSubscriptionErrorModeled::ResourceNotFoundException(
                    state::subscription_not_found(),
                )
                .into(),
            );
        };

        // The removal this record has ALREADY taken: the subscription is on its way
        // out of the store, so a repeat of the call that accepted it is answered with
        // the same record rather than refused for the status that call put it in.
        if record.removed_at.is_some() {
            return Ok(DeleteEksAnywhereSubscriptionOutput {
                subscription: Some(record.to_subscription()),
            });
        }

        // WHAT THIS API REMOVES: "an expired or inactive subscription". A live one is
        // cancelled through Amazon Web Services Support, not here, so it is refused as
        // the request's being invalid given the state and NOTHING of it is torn down.
        // `subscriptionId` IS set: the service resolved the subscription it refuses for
        // ([`state::subscription_delete_refusal`]).
        if let Some(why) = state::subscription_delete_refusal(&record.status) {
            return Err(
                DeleteEksAnywhereSubscriptionErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        subscription_id: Some(record.id.clone()),
                        message: Some(why),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        // The subscription takes the status of the removal it is in for the span the
        // clock owes it, after which the store no longer holds it and the listing and
        // the reads report it no more. The due instant is stored; nothing sleeps for
        // it. No other move is owed on a record on its way out, so the activation
        // instant goes with it.
        record.status = state::SUBSCRIPTION_STATUS_DELETING.to_string();
        record.active_at = None;
        record.removed_at = Some(now + state::SUBSCRIPTION_DELETION_DELAY);

        Ok(DeleteEksAnywhereSubscriptionOutput {
            subscription: Some(record.to_subscription()),
        })
    }
}
