// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_outbound_web_identity_federation_info`
//! operation.
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for
//! `GetOutboundWebIdentityFederationInfo` because its input is `smithy.api#Unit`, which it
//! does not type yet (see `src/ops/SKIPPED_STUBS.md`), so these shapes are written out here
//! in the same shape as a rendered sibling.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

/// `GetOutboundWebIdentityFederationInfo` takes no input: the operation's modelled input is
/// `smithy.api#Unit`. The empty shape still carries a schema, a serializer and a
/// deserializer, because the framework's centralized request walk deserializes and
/// validates every input against `INPUT_SCHEMA` before `handle` runs.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetOutboundWebIdentityFederationInfoInput;

#[smithy_macros::smithy(
    shape = "com.amazonaws.iam#GetOutboundWebIdentityFederationInfoResponse"
)]
#[derive(Debug, Clone, Default)]
pub struct GetOutboundWebIdentityFederationInfoOutput {
    pub issuer_identifier: Option<String>,
    pub jwt_vending_enabled: bool,
}
