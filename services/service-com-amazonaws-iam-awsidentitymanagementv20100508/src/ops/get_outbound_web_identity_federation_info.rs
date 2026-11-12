// SPDX-License-Identifier: CC0-1.0

//! @op GetOutboundWebIdentityFederationInfo
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for this
//! operation because its input is `smithy.api#Unit`, which it does not type yet (see
//! `src/ops/SKIPPED_STUBS.md`), so the operation was left modeled-but-unwired and the
//! framework answered it from the unroutable-operation path — an awsQuery `InvalidAction`,
//! which tells a client the API does not exist rather than naming one of the conditions
//! this operation models. The input, output, error enum and schemas a generated stub would
//! have carried are declared in
//! `src/ops/types/get_outbound_web_identity_federation_info_types.rs` and reached through
//! `super::types::*`; `build.rs` picks the operation up from the `@op` line above and wires
//! it into the service's routing table.
//!
//! ## What this operation answers on the wire
//!
//! A READ: it "retrieves the configuration information for the outbound identity
//! federation feature in your Amazon Web Services account", which is the record
//! `EnableOutboundWebIdentityFederation` wrote and
//! `DisableOutboundWebIdentityFederation` cleared
//! ([`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::outbound_web_identity_federation`]).
//! Nothing is written, so two reads of one account answer the same members:
//!
//! * the feature on: served `200` with the account's `IssuerIdentifier` and
//!   `JwtVendingEnabled`, both read off that record;
//! * the feature off — an account that never enabled it and one that disabled it again are
//!   the same state — refused with the modelled `FeatureDisabledException` (HTTP 404,
//!   awsQuery code `FeatureDisabled`), naming the account the feature is off for.
//!
//! A caller whose identity-based policies do not permit the call is refused before anything
//! is reported, `403` `<Code>AccessDenied</Code>`
//! ([`crate::state::authorization::authorize`]).
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GetOutboundWebIdentityFederationInfo`
//!
//! Retrieves the configuration information for the outbound identity federation feature in
//! your Amazon Web Services account. The response includes the unique issuer URL for your
//! Amazon Web Services account and the current enabled/disabled status of the feature. Use
//! this operation to obtain the issuer URL that you need to configure trust relationships
//! with external services.
//!
//! ## Input
//!
//! None (`smithy.api#Unit`).
//!
//! ## Output
//!
//! * `IssuerIdentifier` (stringType) — A unique issuer URL for your Amazon Web Services
//!   account that hosts the OpenID Connect (OIDC) discovery endpoints at
//!   `/.well-known/openid-configuration and /.well-known/jwks.json`.
//! * `JwtVendingEnabled` (booleanType, default false) — Indicates whether outbound identity
//!   federation is currently enabled for your Amazon Web Services account. When true, IAM
//!   principals in the account can call the `GetWebIdentityToken` API to obtain JSON Web
//!   Tokens (JWTs) for authentication with external services.
//!
//! ## Errors
//!
//! * `FeatureDisabledException` (HTTP 404, awsQuery code `FeatureDisabled`) — The request
//!   failed because outbound identity federation is already disabled for your Amazon Web
//!   Services account.
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
// `GetOutboundWebIdentityFederationInfoError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.iam#GetOutboundWebIdentityFederationInfo")]
#[derive(Debug)]
pub enum GetOutboundWebIdentityFederationInfoErrorModeled {
    FeatureDisabledException(FeatureDisabledException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_outbound_web_identity_federation_info::GetOutboundWebIdentityFederationInfoError`).
pub type GetOutboundWebIdentityFederationInfoError =
    crate::scaffolded::Scaffolded<GetOutboundWebIdentityFederationInfoErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<GetOutboundWebIdentityFederationInfoErrorModeled>
    for GetOutboundWebIdentityFederationInfoError
{
    fn from(modeled: GetOutboundWebIdentityFederationInfoErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetOutboundWebIdentityFederationInfo` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetOutboundWebIdentityFederationInfo<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetOutboundWebIdentityFederationInfo<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetOutboundWebIdentityFederationInfo<Eng>
{
    type Input = GetOutboundWebIdentityFederationInfoInput;
    type Output = GetOutboundWebIdentityFederationInfoOutput;
    type Error = GetOutboundWebIdentityFederationInfoError;

    const NAME: &'static str = "GetOutboundWebIdentityFederationInfo";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_OUTBOUND_WEB_IDENTITY_FEDERATION_INFO_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_OUTBOUND_WEB_IDENTITY_FEDERATION_INFO_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The operation takes no input: what it reports comes from the account's own
        // record of the feature.
        let _ = &input;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // This operation models no server error, so an unreachable store is reported
            // through the crate's generic refusal channel.
            return Err(crate::scaffolded::Scaffolded::validation_error(
                "the IAM store is unavailable",
            ));
        };

        // A READ: the configuration is reported as the last write left it, so this
        // operation only consults the record and writes nothing.
        let store = state.lock();

        // IAM authorizes its own API: the signing principal's identity-based policies are
        // evaluated before anything is reported. The feature is the ACCOUNT's, not one
        // entity's, so the decision is made against the account-wide resource — exactly as
        // the enable and the disable of this same record decide.
        crate::state::authorization::authorize(
            &store,
            ctx,
            crate::state::authorization::GET_OUTBOUND_WEB_IDENTITY_FEDERATION_INFO,
            crate::state::authorization::ACCOUNT_WIDE_RESOURCE,
        )?;

        // An account with the feature off has no configuration to report, which is the one
        // refusal this operation models.
        let Some(held) = store.served_outbound_web_identity_federation() else {
            return Err(
                GetOutboundWebIdentityFederationInfoErrorModeled::FeatureDisabledException(
                    FeatureDisabledException {
                        message: Some(crate::state::outbound_federation_disabled_message()),
                    },
                )
                .into(),
            );
        };

        Ok(GetOutboundWebIdentityFederationInfoOutput {
            issuer_identifier: Some(held.issuer_identifier.clone()),
            // Read off the record rather than stated as a literal: the member reports
            // "whether outbound identity federation is currently enabled", which is the
            // very flag the enable set and the disable clears.
            jwt_vending_enabled: held.enabled,
        })
    }
}
