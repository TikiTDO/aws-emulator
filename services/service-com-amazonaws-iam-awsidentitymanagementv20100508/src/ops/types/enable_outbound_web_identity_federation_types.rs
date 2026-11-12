// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `enable_outbound_web_identity_federation`
//! operation.
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for
//! `EnableOutboundWebIdentityFederation` because its input is `smithy.api#Unit`, which it
//! does not type yet (see `src/ops/SKIPPED_STUBS.md`), so these shapes are written out here
//! in the same shape as a rendered sibling — exactly as
//! `get_account_password_policy_types.rs` was written for another operation of this service
//! whose input is a unit.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

/// `EnableOutboundWebIdentityFederation` takes no input: the operation's modelled input is
/// `smithy.api#Unit`. The empty shape still carries a schema, a serializer and a
/// deserializer, because the framework's centralized request walk deserializes and
/// validates every input against `INPUT_SCHEMA` before `handle` runs.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct EnableOutboundWebIdentityFederationInput;

#[smithy_macros::smithy(shape = "com.amazonaws.iam#EnableOutboundWebIdentityFederationResponse")]
#[derive(Debug, Clone, Default)]
pub struct EnableOutboundWebIdentityFederationOutput {
    pub issuer_identifier: Option<String>,
}

/// The refusal only the ENABLE models, which is why it is declared here rather than in
/// `shared_types`: the account already has the feature on, and "you cannot enable the
/// feature multiple times".
#[smithy_macros::smithy(shape = "com.amazonaws.iam#FeatureEnabledException")]
#[derive(Debug, Clone, Default)]
pub struct FeatureEnabledException {
    pub message: Option<String>,
}
