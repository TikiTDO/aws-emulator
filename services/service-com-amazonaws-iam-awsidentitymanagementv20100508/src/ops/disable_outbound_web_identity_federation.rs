// SPDX-License-Identifier: CC0-1.0

//! @op DisableOutboundWebIdentityFederation
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for this
//! operation because its input is `smithy.api#Unit`, which it does not type yet (see
//! `src/ops/SKIPPED_STUBS.md`), so the operation was left modeled-but-unwired and the
//! framework answered it from the unroutable-operation path — an awsQuery `InvalidAction`,
//! which tells a client the API does not exist rather than naming the condition this
//! operation models. The input and error enum a generated stub would have carried are
//! declared in `src/ops/types/disable_outbound_web_identity_federation_types.rs` and
//! reached through `super::types::*`, and the unit OUTPUT is declared below the way every
//! rendered sibling of this service with a unit output declares its own; `build.rs` picks
//! the operation up from the `@op` line above and wires it into the service's routing
//! table.
//!
//! ## What this operation answers on the wire
//!
//! A MUTATION: it "disables the outbound identity federation feature for your Amazon Web
//! Services account", which is the flag on the account's own record of the feature
//! ([`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::outbound_web_identity_federation`]):
//!
//! * the feature on: it is turned off and the call answers `200` with an empty result. The
//!   issuer the account was given stays on the record — the operation "does not affect
//!   tokens that were issued before the feature was disabled", and the issuer belongs to
//!   the account — so a later enable reports that same issuer;
//! * the feature off already (never enabled, or disabled before): refused with the modelled
//!   `FeatureDisabledException` (HTTP 404, awsQuery code `FeatureDisabled`) — "you cannot
//!   disable the feature multiple times".
//!
//! A caller whose identity-based policies do not permit the call is refused before anything
//! is written, `403` `<Code>AccessDenied</Code>`
//! ([`crate::state::authorization::authorize`]), so a request that was not allowed disables
//! nothing.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `DisableOutboundWebIdentityFederation`
//!
//! Disables the outbound identity federation feature for your Amazon Web Services account.
//! When disabled, IAM principals in the account cannot use the `GetWebIdentityToken` API to
//! obtain JSON Web Tokens (JWTs) for authentication with external services. This operation
//! does not affect tokens that were issued before the feature was disabled.
//!
//! ## Input
//!
//! None (`smithy.api#Unit`).
//!
//! ## Output
//!
//! None (`smithy.api#Unit`).
//!
//! ## Errors
//!
//! * `FeatureDisabledException` (HTTP 404, awsQuery code `FeatureDisabled`) — The request
//!   failed because outbound identity federation is already disabled for your Amazon Web
//!   Services account. You cannot disable the feature multiple times.
//!
//! For the full text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json")
//! ```

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DisableOutboundWebIdentityFederationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

/// The operation's output: the modelled output is `smithy.api#Unit`, so the response
/// carries the envelope and no members at all.
#[derive(Debug, Clone, Default)]
pub struct DisableOutboundWebIdentityFederationOutput;

impl DisableOutboundWebIdentityFederationOutput {
    /// The unit output's schema, declared the way every rendered sibling of this service
    /// with a unit output declares its own (`crate::ops::add_user_to_group`).
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static DISABLE_OUTBOUND_WEB_IDENTITY_FEDERATION_OUTPUT_SCHEMA:
    &::aws_smithy_schema::Schema = &DisableOutboundWebIdentityFederationOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DisableOutboundWebIdentityFederationOutput {
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.iam#DisableOutboundWebIdentityFederation")]
#[derive(Debug)]
pub enum DisableOutboundWebIdentityFederationErrorModeled {
    FeatureDisabledException(FeatureDisabledException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::disable_outbound_web_identity_federation::DisableOutboundWebIdentityFederationError`).
pub type DisableOutboundWebIdentityFederationError =
    crate::scaffolded::Scaffolded<DisableOutboundWebIdentityFederationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<DisableOutboundWebIdentityFederationErrorModeled>
    for DisableOutboundWebIdentityFederationError
{
    fn from(modeled: DisableOutboundWebIdentityFederationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DisableOutboundWebIdentityFederation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct DisableOutboundWebIdentityFederation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DisableOutboundWebIdentityFederation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DisableOutboundWebIdentityFederation<Eng>
{
    type Input = DisableOutboundWebIdentityFederationInput;
    type Output = DisableOutboundWebIdentityFederationOutput;
    type Error = DisableOutboundWebIdentityFederationError;

    const NAME: &'static str = "DisableOutboundWebIdentityFederation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DISABLE_OUTBOUND_WEB_IDENTITY_FEDERATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        DISABLE_OUTBOUND_WEB_IDENTITY_FEDERATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The operation takes no input: what it does is decided by the account's own
        // record of the feature.
        let _ = &input;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // This operation models no server error, so an unreachable store is reported
            // through the crate's generic refusal channel.
            return Err(crate::scaffolded::Scaffolded::validation_error(
                "the IAM store is unavailable",
            ));
        };

        // A MUTATION: the lock is held for write across the decision and the write, so two
        // calls cannot both disable the feature for one account.
        let mut store = state.lock();

        // IAM authorizes its own API: the signing principal's identity-based policies are
        // evaluated before anything is written. The feature is the ACCOUNT's, not one
        // entity's, so the decision is made against the account-wide resource — exactly as
        // the enable and the read of this same record decide.
        crate::state::authorization::authorize(
            &store,
            ctx,
            crate::state::authorization::DISABLE_OUTBOUND_WEB_IDENTITY_FEDERATION,
            crate::state::authorization::ACCOUNT_WIDE_RESOURCE,
        )?;

        let caller_akid = ::engine::extract_akid(&ctx.parts().headers);

        // The feature off already is the one refusal this operation models, and it is
        // answered before anything is written.
        if store.served_outbound_web_identity_federation().is_none() {
            ::tracing::info!(
                audit = true,
                operation = "DisableOutboundWebIdentityFederation",
                outcome = "refused_feature_disabled",
                caller_akid = %caller_akid,
                "outbound identity federation was already disabled"
            );
            return Err(
                DisableOutboundWebIdentityFederationErrorModeled::FeatureDisabledException(
                    FeatureDisabledException {
                        message: Some(
                            crate::state::outbound_federation_already_disabled_message(),
                        ),
                    },
                )
                .into(),
            );
        }

        store.disable_outbound_web_identity_federation();

        ::tracing::info!(
            audit = true,
            operation = "DisableOutboundWebIdentityFederation",
            outcome = "disabled",
            caller_akid = %caller_akid,
            "outbound identity federation was disabled for the account"
        );

        Ok(DisableOutboundWebIdentityFederationOutput)
    }
}
