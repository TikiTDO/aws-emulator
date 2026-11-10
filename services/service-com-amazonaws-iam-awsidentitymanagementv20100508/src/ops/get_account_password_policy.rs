// SPDX-License-Identifier: CC0-1.0

//! @op GetAccountPasswordPolicy
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for this
//! operation because its input is `smithy.api#Unit`, which it does not type yet (see
//! `src/ops/SKIPPED_STUBS.md`), so the operation was left modeled-but-unwired and the
//! framework answered it from the unroutable-operation path — an awsQuery `InvalidAction`,
//! which tells a client the API does not exist rather than naming one of the conditions
//! this operation models. The input, output, error enum and schemas a generated stub would
//! have carried are declared in `src/ops/types/get_account_password_policy_types.rs` (the
//! layout every other operation of this service follows) and reached through
//! `super::types::*`; `build.rs` picks the operation up from the `@op` line above and wires
//! it into the service's routing table, exactly as it does for the hand-authored
//! `src/ops/get_credential_report.rs`.
//!
//! ## What this operation answers on the wire
//!
//! A READ: it "retrieves the password policy for the Amazon Web Services account" — the
//! policy `UpdateAccountPasswordPolicy` last wrote
//! ([`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::account_password_policy`]).
//! An account has ONE password policy, so what a call answers is decided by that record
//! alone and nothing is written: two reads of one policy answer the same members.
//!
//! * an account that has never set a policy has none to read, which is the modelled
//!   `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`) — "the request was
//!   rejected because it referenced a resource entity that does not exist", worded in the
//!   `<Entity> … cannot be found.` form this service words its reads with and naming the
//!   account the policy would belong to;
//! * a policy the account holds: served, `200`, with every member the account configured.
//!   `ExpirePasswords` is DERIVED rather than stored, because the member itself says so:
//!   "Indicates whether passwords in the account expire. Returns true if `MaxPasswordAge`
//!   contains a value greater than 0."
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
//! # Modeled contract for `GetAccountPasswordPolicy`
//!
//! Retrieves the password policy for the Amazon Web Services account. This tells you the
//! complexity requirements and mandatory rotation periods for the IAM user passwords in your
//! account. For more information about using a password policy, see Managing an IAM password
//! policy.
//!
//! ## Input
//!
//! None (`smithy.api#Unit`).
//!
//! ## Output
//!
//! * `PasswordPolicy` (PasswordPolicy) **required** — A structure that contains details about
//!   the account's password policy.
//!
//! ## Errors
//!
//! * `NoSuchEntityException` (HTTP 404) — The request was rejected because it referenced a
//!   resource entity that does not exist. The error message describes the resource.
//! * `ServiceFailureException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception or failure.
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
// `GetAccountPasswordPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.iam#GetAccountPasswordPolicy")]
#[derive(Debug)]
pub enum GetAccountPasswordPolicyErrorModeled {
    NoSuchEntityException(NoSuchEntityException),
    ServiceFailureException(ServiceFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_account_password_policy::GetAccountPasswordPolicyError`).
pub type GetAccountPasswordPolicyError =
    crate::scaffolded::Scaffolded<GetAccountPasswordPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<GetAccountPasswordPolicyErrorModeled> for GetAccountPasswordPolicyError {
    fn from(modeled: GetAccountPasswordPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAccountPasswordPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetAccountPasswordPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAccountPasswordPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAccountPasswordPolicy<Eng>
{
    type Input = GetAccountPasswordPolicyInput;
    type Output = GetAccountPasswordPolicyOutput;
    type Error = GetAccountPasswordPolicyError;

    const NAME: &'static str = "GetAccountPasswordPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_ACCOUNT_PASSWORD_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_ACCOUNT_PASSWORD_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The operation takes no input: what it answers comes from the account's own
        // password policy record.
        let _ = &input;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                GetAccountPasswordPolicyErrorModeled::ServiceFailureException(
                    ServiceFailureException {
                        message: Some("the IAM store is unavailable".to_string()),
                    },
                )
                .into(),
            );
        };

        // A READ: the policy is reported as the last write left it, so this operation only
        // consults the record and writes nothing.
        let store = state.lock();

        // IAM authorizes its own API: the signing principal's identity-based policies are
        // evaluated before anything is reported. The policy is the ACCOUNT's, not one
        // entity's, so the decision is made against the account-wide resource — exactly as
        // `UpdateAccountPasswordPolicy`, the write of this same record, decides.
        crate::state::authorization::authorize(
            &store,
            ctx,
            crate::state::authorization::GET_ACCOUNT_PASSWORD_POLICY,
            crate::state::authorization::ACCOUNT_WIDE_RESOURCE,
        )?;

        // An account that has never set a password policy holds none to report.
        let Some(held) = store.account_password_policy.as_ref() else {
            return Err(
                GetAccountPasswordPolicyErrorModeled::NoSuchEntityException(
                    NoSuchEntityException {
                        message: Some(no_password_policy_message()),
                    },
                )
                .into(),
            );
        };

        Ok(GetAccountPasswordPolicyOutput {
            password_policy: PasswordPolicy {
                minimum_password_length: Some(held.minimum_password_length),
                require_symbols: held.require_symbols,
                require_numbers: held.require_numbers,
                require_uppercase_characters: held.require_uppercase_characters,
                require_lowercase_characters: held.require_lowercase_characters,
                allow_users_to_change_password: held.allow_users_to_change_password,
                // "Returns true if MaxPasswordAge contains a value greater than 0. Returns
                // false if MaxPasswordAge is 0 or not present"
                // (`PasswordPolicy$ExpirePasswords`), so the member is DERIVED from the
                // age the account configured rather than stored beside it.
                expire_passwords: held.max_password_age.is_some_and(|age| age > 0),
                max_password_age: held.max_password_age,
                password_reuse_prevention: held.password_reuse_prevention,
                hard_expiry: held.hard_expiry,
            },
        })
    }
}

/// How this service words `NoSuchEntityException` for an account that holds no password
/// policy.
///
/// The `<Entity> … cannot be found.` form this service words every absent-entity read with
/// (`crate::state::no_such_role_message` and the sentences beside it), whose one variable
/// part here is the ACCOUNT the policy would belong to — the account this store stands for,
/// never a value a caller sent, because the request carries nothing at all.
fn no_password_policy_message() -> String {
    format!(
        "The Password Policy with domain name {} cannot be found.",
        ::emulator_server::emulator_account_id()
    )
}
