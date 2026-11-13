// SPDX-License-Identifier: CC0-1.0

//! @op UpdateEksAnywhereSubscription
//! IMPLEMENTED. Writes the auto-renewal setting of the
//! [`crate::state::EksAnywhereSubscriptionRecord`] the request addresses — "only auto
//! renewal and tags can be updated after subscription creation" — and answers with
//! that record.
//!
//! The rules it enforces:
//!
//! * a subscription is addressed by the id the create minted and belongs to the
//!   Region it was created in, so an id this Region carries no subscription for is
//!   the `ResourceNotFoundException` the subscription reads answer
//!   ([`crate::state::subscription_not_found`]);
//! * `clientRequestToken` is `@idempotencyToken`, so an SDK that RESENDS the token of
//!   an update this record already took is answered with that subscription rather
//!   than starting a second update;
//! * a subscription whose term has run out, or whose removal has been accepted, has
//!   no renewal left to set: that is `InvalidRequestException` naming the
//!   subscription the service resolved ([`crate::state::subscription_update_refusal`]).
//!
//! An accepted update reports `UPDATING` for the span the clock owes it, after which
//! the subscription is `ACTIVE` again; nothing sleeps for it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateEksAnywhereSubscription`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Update an EKS Anywhere Subscription. Only auto renewal and tags can be updated after
//! subscription creation.
//!
//! ## Input
//!
//! * `autoRenew` (Boolean, default false) **required** — A boolean indicating whether or not
//!   to automatically renew the subscription.
//! * `clientRequestToken` (String, @idempotencyToken) — Unique, case-sensitive identifier to
//!   ensure the idempotency of the request.
//! * `id` (String) **required** — The ID of the subscription.
//!
//! ## Output
//!
//! * `subscription` (EksAnywhereSubscription) — The full description of the updated
//!   subscription.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
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
// `UpdateEksAnywhereSubscriptionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateEksAnywhereSubscriptionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#UpdateEksAnywhereSubscription")]
#[derive(Debug)]
pub enum UpdateEksAnywhereSubscriptionErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_eks_anywhere_subscription::UpdateEksAnywhereSubscriptionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateEksAnywhereSubscriptionError = crate::scaffolded::Scaffolded<UpdateEksAnywhereSubscriptionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateEksAnywhereSubscriptionErrorModeled> for UpdateEksAnywhereSubscriptionError {
    fn from(modeled: UpdateEksAnywhereSubscriptionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateEksAnywhereSubscription` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateEksAnywhereSubscription<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateEksAnywhereSubscription<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateEksAnywhereSubscription<Eng>
{
    type Input = UpdateEksAnywhereSubscriptionInput;
    type Output = UpdateEksAnywhereSubscriptionOutput;
    type Error = UpdateEksAnywhereSubscriptionError;

    const NAME: &'static str = "UpdateEksAnywhereSubscription";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_EKS_ANYWHERE_SUBSCRIPTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_EKS_ANYWHERE_SUBSCRIPTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();
        let store = state::service_state(&*self.engine).ok_or_else(|| {
            UpdateEksAnywhereSubscriptionError::from(
                UpdateEksAnywhereSubscriptionErrorModeled::ServerException(ServerException {
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
                UpdateEksAnywhereSubscriptionErrorModeled::ResourceNotFoundException(
                    state::subscription_not_found(),
                )
                .into(),
            );
        };

        // `clientRequestToken` is `@idempotencyToken`: an SDK generates it once per
        // call and RESENDS the same value when it retries, so the retry of an update
        // this record already took is answered with that subscription rather than
        // starting a second one. A blank token identifies nothing, so it never matches.
        let token = input
            .client_request_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        if token.is_some() && record.update_token.as_deref() == token {
            return Ok(UpdateEksAnywhereSubscriptionOutput {
                subscription: Some(record.to_subscription()),
            });
        }

        // WHAT THE SUBSCRIPTION'S STATE ALLOWS: `InvalidRequestException` is "the
        // request is invalid given the state" — the class for a subscription there is
        // no renewal left to set on. `subscriptionId` IS set here: the service
        // resolved the subscription it is refusing for.
        if let Some(why) = state::subscription_update_refusal(&record.status) {
            return Err(
                UpdateEksAnywhereSubscriptionErrorModeled::InvalidRequestException(
                    InvalidRequestException {
                        subscription_id: Some(record.id.clone()),
                        message: Some(why),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        // "Only auto renewal and tags can be updated after subscription creation", and
        // the tags of a subscription travel through `TagResource`: this request carries
        // the renewal flag and nothing else, so that is the one member written. The
        // subscription takes the update's own status for the span the clock owes it,
        // after which it is `ACTIVE` again — the move is stored, never slept for.
        record.auto_renew = input.auto_renew;
        record.update_token = token.map(str::to_owned);
        record.status = state::SUBSCRIPTION_STATUS_UPDATING.to_string();
        record.active_at = Some(now + state::SUBSCRIPTION_ACTIVATION_DELAY);

        Ok(UpdateEksAnywhereSubscriptionOutput {
            subscription: Some(record.to_subscription()),
        })
    }
}
