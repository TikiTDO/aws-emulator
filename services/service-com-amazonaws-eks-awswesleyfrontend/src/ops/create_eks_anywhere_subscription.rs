// SPDX-License-Identifier: CC0-1.0

//! @op CreateEksAnywhereSubscription
//! IMPLEMENTED. Writes an [`crate::state::EksAnywhereSubscriptionRecord`] for one EKS
//! Anywhere support contract and answers with it, reporting `CREATING` until the clock
//! owes it `ACTIVE` ([`crate::state::SUBSCRIPTION_ACTIVATION_DELAY`]) and `EXPIRED`
//! once its term has run out on a subscription that does not auto renew.
//! `DescribeEksAnywhereSubscription` computes its whole answer from that record;
//! nothing here keeps a second copy.
//!
//! A subscription hangs off no cluster: it is "a contract agreement for the length of
//! the term specified in the request", so the record lives in the store's own
//! subscription map, keyed by the Region the request is signed for and the id this
//! call mints.
//!
//! The rules it enforces, each from the model's own documentation:
//!
//! * "Valid values are MONTHS for term unit and 12 or 36 for term duration"
//!   (`EksAnywhereSubscriptionTerm`) — a term outside that is
//!   `InvalidParameterException` and NOTHING is written
//!   ([`crate::state::subscription_term_refusal`]);
//! * "The number of licenses to purchase with the subscription. Valid values are
//!   between 1 and 100" (`$licenseQuantity`), a member carrying a `@default` of 0 and
//!   no modelled range, so a count outside the published one is refused here;
//! * the name "must be unique in your Amazon Web Services account in the Amazon Web
//!   Services Region you're creating the subscription in" (`$name`): a name the
//!   Region already carries is refused, and nothing is written;
//! * `clientRequestToken` is `@idempotencyToken`, so a retry carrying the token of the
//!   call that created the subscription is answered with that subscription instead of
//!   a refusal.
//!
//! ORDER: every rule above is about the REQUEST, so each is checked before the store
//! is consulted — a malformed term is refused whether or not the name is taken.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateEksAnywhereSubscription`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Creates an EKS Anywhere subscription. When a subscription is created, it is a contract
//! agreement for the length of the term specified in the request. Licenses that are used to
//! validate support are provisioned in Amazon Web Services License Manager and the caller
//! account is granted access to EKS Anywhere Curated Packages.
//!
//! ## Input
//!
//! * `autoRenew` (Boolean, default false) **required** — A boolean indicating whether the
//!   subscription auto renews at the end of the term.
//! * `clientRequestToken` (String, @idempotencyToken) — A unique, case-sensitive identifier
//!   that you provide to ensure the idempotency of the request.
//! * `licenseQuantity` (Integer, default 0) **required** — The number of licenses to purchase
//!   with the subscription. Valid values are between 1 and 100. This value can't be changed
//!   after creating the subscription.
//! * `licenseType` (EksAnywhereSubscriptionLicenseType, one of: Cluster) — The license type
//!   for all licenses in the subscription. Valid value is CLUSTER. With the CLUSTER license
//!   type, each license covers support for a single EKS Anywhere cluster.
//! * `name` (EksAnywhereSubscriptionName, len 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$)
//!   **required** — The unique name for your subscription. It must be unique in your Amazon
//!   Web Services account in the Amazon Web Services Region you're creating the subscription
//!   in. The name can contain only alphanumeric characters (case-sensitive), hyphens, and
//!   underscores. It must start with an alphabetic character and can't be longer than 100
//!   characters.
//! * `tags` (TagMap, len 1..50) — The metadata for a subscription to assist with
//!   categorization and organization. Each tag consists of a key and an optional value.
//!   Subscription tags don't propagate to any other resources associated with the
//!   subscription.
//! * `term` (EksAnywhereSubscriptionTerm) **required** — An object representing the term
//!   duration and term unit type of your subscription. This determines the term length of
//!   your subscription. Valid values are MONTHS for term unit and 12 or 36 for term duration,
//!   indicating a 12 month or 36 month subscription. This value cannot be changed after
//!   creating the subscription.
//!
//! ## Output
//!
//! * `subscription` (EksAnywhereSubscription) — The full description of the subscription.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceLimitExceededException` (HTTP 400) — You have encountered a service limit on
//!   the specified resource.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{
    self, EksAnywhereSubscriptionRecord, MAX_STORED_SUBSCRIPTIONS,
    MAX_SUBSCRIPTION_LICENSE_QUANTITY, MIN_SUBSCRIPTION_LICENSE_QUANTITY, SUBSCRIPTION_MONTH,
    SUBSCRIPTION_ACTIVATION_DELAY, SUBSCRIPTION_LICENSE_TYPE_CLUSTER, SUBSCRIPTION_STATUS_CREATING,
};
use std::collections::BTreeMap;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateEksAnywhereSubscriptionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateEksAnywhereSubscriptionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#CreateEksAnywhereSubscription")]
#[derive(Debug)]
pub enum CreateEksAnywhereSubscriptionErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceLimitExceededException(ResourceLimitExceededException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_eks_anywhere_subscription::CreateEksAnywhereSubscriptionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateEksAnywhereSubscriptionError = crate::scaffolded::Scaffolded<CreateEksAnywhereSubscriptionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateEksAnywhereSubscriptionErrorModeled> for CreateEksAnywhereSubscriptionError {
    fn from(modeled: CreateEksAnywhereSubscriptionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateEksAnywhereSubscription` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateEksAnywhereSubscription<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateEksAnywhereSubscription<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateEksAnywhereSubscription<Eng>
{
    type Input = CreateEksAnywhereSubscriptionInput;
    type Output = CreateEksAnywhereSubscriptionOutput;
    type Error = CreateEksAnywhereSubscriptionError;

    const NAME: &'static str = "CreateEksAnywhereSubscription";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_EKS_ANYWHERE_SUBSCRIPTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_EKS_ANYWHERE_SUBSCRIPTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let now = SystemTime::now();

        let store = state::service_state(&*self.engine).ok_or_else(|| {
            CreateEksAnywhereSubscriptionError::from(
                CreateEksAnywhereSubscriptionErrorModeled::ServerException(ServerException {
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        // `InvalidParameterException` carries a `subscriptionId` member, which a
        // request that has not created a subscription yet has no value for: the
        // refusals below name the subscription by nothing but the sentence, which is
        // what the recorded service answers too.
        let invalid_parameter = |message: String| {
            CreateEksAnywhereSubscriptionError::from(
                CreateEksAnywhereSubscriptionErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };

        // THE REQUEST'S OWN MEMBERS FIRST, before the store is consulted: a term or a
        // license count outside the published values could not be served against any
        // account, so it is refused as the bad parameter it is whatever the store
        // holds — and nothing is written for a request that is refused.
        //
        // "Valid values are MONTHS for term unit and 12 or 36 for term duration"
        // (`EksAnywhereSubscriptionTerm`); neither half is modelled as a range, so
        // this is where it is caught.
        if let Some(why) = state::subscription_term_refusal(&input.term) {
            return Err(invalid_parameter(why));
        }

        // "The number of licenses to purchase with the subscription. Valid values are
        // between 1 and 100" (`$licenseQuantity`). The member carries a `@default` of
        // 0 and no modelled range, so a request that names none arrives as 0 — which
        // is below the published minimum and so is not a subscription that can be
        // created.
        if !(MIN_SUBSCRIPTION_LICENSE_QUANTITY..=MAX_SUBSCRIPTION_LICENSE_QUANTITY)
            .contains(&input.license_quantity)
        {
            return Err(invalid_parameter(format!(
                "licenseQuantity must be between {MIN_SUBSCRIPTION_LICENSE_QUANTITY} and \
                 {MAX_SUBSCRIPTION_LICENSE_QUANTITY}, got {}",
                input.license_quantity
            )));
        }

        // "The license type for all licenses in the subscription. Valid value is
        // CLUSTER" (`$licenseType`). The member targets an enum, so a value outside it
        // is already refused before this handler runs and a request that names none
        // takes the one type the API defines.
        let license_type = input
            .license_type
            .clone()
            .unwrap_or_else(|| SUBSCRIPTION_LICENSE_TYPE_CLUSTER.to_string());

        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // `clientRequestToken` is `@idempotencyToken`: an SDK generates it once per
        // call and RESENDS the same value when it retries, so a retry of the call that
        // created a subscription is answered with THAT subscription rather than
        // creating a second one. A blank token identifies nothing, so it never
        // matches.
        let replayed_token = input
            .client_request_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        if let Some(existing) = replayed_token.and_then(|token| {
            guard
                .eks_anywhere_subscriptions
                .values()
                .find(|record| record.client_request_token.as_deref() == Some(token))
        }) {
            return Ok(CreateEksAnywhereSubscriptionOutput {
                subscription: Some(existing.to_subscription()),
            });
        }

        // The name "must be unique in your Amazon Web Services account in the Amazon
        // Web Services Region you're creating the subscription in" (`$name`). A second
        // subscription under a name the Region already carries is refused as the bad
        // parameter it is: this operation does not model `ResourceInUseException`, so
        // the conflict is reported in the one client-input error it does model, naming
        // the subscription the name is taken by.
        if let Some(existing) = guard
            .eks_anywhere_subscriptions_in(&region)
            .find(|record| record.name == input.name)
        {
            return Err(invalid_parameter(format!(
                "Subscription already exists with name: {} and id: {}",
                input.name, existing.id
            )));
        }

        // What BOUNDS THE STORE: the caller decides how many Regions exist, so without
        // a ceiling repeated creates would retain an unbounded number of records.
        // Exceeding a limit is what `ResourceLimitExceededException` reports.
        if guard.eks_anywhere_subscriptions.len() >= MAX_STORED_SUBSCRIPTIONS {
            return Err(
                CreateEksAnywhereSubscriptionErrorModeled::ResourceLimitExceededException(
                    ResourceLimitExceededException {
                        message: Some(format!(
                            "You have reached the limit of {MAX_STORED_SUBSCRIPTIONS} EKS \
                             Anywhere subscriptions for this account"
                        )),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        // "UUID identifying a subscription" (`EksAnywhereSubscription$id`), drawn from
        // the store's own counter so two creates in the same nanosecond cannot collide,
        // and the ARN that carries it.
        let id = state::new_update_id(guard.next_update_sequence());
        let arn = state::eks_anywhere_subscription_arn(&region, &id);
        // The term is a count of MONTHS, so the expiration date is the effective date
        // plus that many months ([`crate::state::SUBSCRIPTION_MONTH`]). The duration
        // is one of the published ones — the refusal above admitted it — so the
        // multiplication is in range; a clock so far past the representable range that
        // the term cannot be placed reports the effective date itself, which is a
        // subscription with no term left rather than a wrapped instant.
        let term = u32::try_from(input.term.duration.max(0))
            .ok()
            .and_then(|months| SUBSCRIPTION_MONTH.checked_mul(months))
            .unwrap_or_default();
        let record = EksAnywhereSubscriptionRecord {
            id: id.clone(),
            name: input.name.clone(),
            arn,
            created_at: now,
            // "The Unix timestamp in seconds for when the subscription is effective":
            // the contract starts when it is bought.
            effective_date: now,
            expiration_date: now.checked_add(term).unwrap_or(now),
            license_quantity: input.license_quantity,
            license_type,
            term_duration: input.term.duration,
            // The unit is the one the term refusal above admitted.
            term_unit: input
                .term
                .unit
                .clone()
                .unwrap_or_else(|| state::SUBSCRIPTION_TERM_UNIT_MONTHS.to_string()),
            status: SUBSCRIPTION_STATUS_CREATING.to_string(),
            active_at: Some(now + SUBSCRIPTION_ACTIVATION_DELAY),
            // A subscription this call creates is on its way IN: nothing owes its
            // removal, and no update has been taken on it yet.
            removed_at: None,
            update_token: None,
            auto_renew: input.auto_renew,
            tags: input
                .tags
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
            client_request_token: replayed_token.map(str::to_owned),
        };

        let subscription = record.to_subscription();
        guard
            .eks_anywhere_subscriptions
            .insert((region, id), record);

        Ok(CreateEksAnywhereSubscriptionOutput {
            subscription: Some(subscription),
        })
    }
}
