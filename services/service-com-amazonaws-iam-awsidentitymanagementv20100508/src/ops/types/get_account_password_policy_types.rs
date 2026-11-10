// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_account_password_policy` operation.
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for
//! `GetAccountPasswordPolicy` because its input is `smithy.api#Unit`, which it does not type
//! yet (see `src/ops/SKIPPED_STUBS.md`), so these shapes are written out here in the same
//! shape as a rendered sibling — exactly as `get_credential_report_types.rs` was written for
//! the other operation of this service whose input is a unit.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

/// `GetAccountPasswordPolicy` takes no input: the operation's modelled input is
/// `smithy.api#Unit`. The empty shape still carries a schema, a serializer and a
/// deserializer, because the framework's centralized request walk deserializes and
/// validates every input against `INPUT_SCHEMA` before `handle` runs.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetAccountPasswordPolicyInput;

#[smithy_macros::smithy(shape = "com.amazonaws.iam#GetAccountPasswordPolicyResponse")]
#[derive(Debug, Clone)]
pub struct GetAccountPasswordPolicyOutput {
    pub password_policy: PasswordPolicy,
}

#[smithy_macros::smithy(shape = "com.amazonaws.iam#PasswordPolicy")]
#[derive(Debug, Clone, Default)]
pub struct PasswordPolicy {
    pub minimum_password_length: Option<i32>,
    pub require_symbols: bool,
    pub require_numbers: bool,
    pub require_uppercase_characters: bool,
    pub require_lowercase_characters: bool,
    pub allow_users_to_change_password: bool,
    pub expire_passwords: bool,
    pub max_password_age: Option<i32>,
    pub password_reuse_prevention: Option<i32>,
    pub hard_expiry: Option<bool>,
}
