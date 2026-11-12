// SPDX-License-Identifier: CC0-1.0

//! @op CreatePolicyStoreAlias
//!
//! Creates a policy store alias — an alternative identifier for a policy store —
//! and INSERTS it into [`crate::state`]: the ARN, name, target store and
//! timestamp this returns are the ones `GetPolicyStoreAlias` reads back, and the
//! ones that make `policy-store-alias/<name>` resolve in every operation taking a
//! `policyStoreId`.
//!
//! The model documents three rules this handler implements:
//!
//! * **Idempotent on `(aliasName, policyStoreId)`** — "If multiple
//!   `CreatePolicyStoreAlias` requests are made where the `aliasName` and
//!   `policyStoreId` fields are the same between the requests, subsequent
//!   requests will be ignored. For each duplicate … a Success response will be
//!   returned and a new policy store alias will not be created." The duplicate
//!   therefore answers with the PERSISTED alias, timestamp included, rather than
//!   minting a second one.
//! * **The name is unique in the account and Region** — a request naming a name
//!   another store already holds IN THE REGION IT IS SIGNED FOR conflicts (HTTP
//!   409), which is what makes concurrent creations of one name have exactly one
//!   winner; the same name in another Region is a free name there, and the alias
//!   it creates resolves only from that Region.
//! * **A name in `PendingDeletion` cannot be taken** —
//!   `DeletePolicyStoreAlias` documents that "when a policy store alias is in the
//!   `PendingDeletion` state, new policy store aliases cannot be created with the
//!   same name", so that name conflicts until it is hard-deleted.
//!
//! ## Input
//!
//! * `aliasName` (Alias, len 0..150, pattern ^[a-zA-Z0-9-/_]*$) **required** — the
//!   alias name, which "must always be prefixed with `policy-store-alias/`"
//!   ([`crate::state::ALIAS_PREFIX`]).
//! * `policyStoreId` (PolicyStoreId, len 1..200) **required** — "The associated
//!   policy store must be specified using its ID. The alias name cannot be used",
//!   so this member is looked up as a store ID only.
//!
//! ## Output
//!
//! * `aliasArn`, `aliasName`, `createdAt`, `policyStoreId` — all **required**.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — the name is already held by an alias of
//!   another store, or by an alias awaiting deletion.
//! * `ResourceNotFoundException` (HTTP 404) — no policy store has that ID. The
//!   store is the resource this request REFERENCES, so it is resolved before any
//!   of the request's own payload is read and this answer outranks the refusals
//!   below.
//! * `ServiceQuotaExceededException` (HTTP 402) — the account is at
//!   [`crate::state::MAX_POLICY_STORE_ALIASES`] aliases.
//! * `ValidationException` (HTTP 400) — the name does not carry the documented
//!   prefix, so no `policyStoreId` member could ever reference it.

#[allow(unused_imports)]
use super::types::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

/// The operation's MODELED errors. `InternalServerException` is a service-level
/// error this operation's own `errors` list does not repeat, so it comes in
/// through `extra_errors`; it is the honest answer when the service's state store
/// cannot be reached, which is a server fault and not a bad request.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#CreatePolicyStoreAlias",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum CreatePolicyStoreAliasErrorModeled {
    ConflictException(ConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_policy_store_alias::CreatePolicyStoreAliasError`).
pub type CreatePolicyStoreAliasError = crate::scaffolded::Scaffolded<CreatePolicyStoreAliasErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CreatePolicyStoreAliasErrorModeled> for CreatePolicyStoreAliasError {
    fn from(modeled: CreatePolicyStoreAliasErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreatePolicyStoreAlias` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct CreatePolicyStoreAlias<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreatePolicyStoreAlias<Eng> {
    /// Construct the operation from the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The conflict answered when the requested name is already taken — whether by
/// a live alias of another store or by one awaiting deletion.
///
/// Both causes answer `ConflictException`'s one fixed message
/// ([`crate::state::CONFLICT_MESSAGE`], through [`crate::ops::conflict`]) and
/// report the collision in the required `resources` list: the ALIAS that holds
/// the name, and the POLICY STORE it is associated with, which is what a caller
/// racing for the name needs to learn. Nothing of the request or of the stored
/// state is spliced into the text.
///
/// The alias is reported by its OWN id — its name without the documented
/// `policy-store-alias/` resource path, through
/// [`crate::state::alias_reference`] — which is the same identity this service's
/// `ResourceNotFoundException` reports for an alias.
fn already_taken(held_by: &crate::state::PolicyStoreAlias) -> CreatePolicyStoreAliasErrorModeled {
    let alias = crate::state::alias_reference(&held_by.alias_name);
    CreatePolicyStoreAliasErrorModeled::ConflictException(crate::ops::conflict([
        crate::ops::resource_conflict(&alias.resource_id, &alias.resource_type),
        crate::ops::resource_conflict(
            &held_by.policy_store_id,
            crate::state::RESOURCE_TYPE_POLICY_STORE,
        ),
    ]))
}

/// Render a persisted alias as this operation's output, so a value that was never
/// stored cannot appear on the wire.
fn output_for(alias: &crate::state::PolicyStoreAlias) -> CreatePolicyStoreAliasOutput {
    CreatePolicyStoreAliasOutput {
        alias_name: alias.alias_name.clone(),
        policy_store_id: alias.policy_store_id.clone(),
        alias_arn: alias.arn.clone(),
        created_at: alias.created_at,
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreatePolicyStoreAlias<Eng>
{
    type Input = CreatePolicyStoreAliasInput;
    type Output = CreatePolicyStoreAliasOutput;
    type Error = CreatePolicyStoreAliasError;

    const NAME: &'static str = "CreatePolicyStoreAlias";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CREATE_POLICY_STORE_ALIAS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CREATE_POLICY_STORE_ALIAS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let unavailable = || {
            CreatePolicyStoreAliasErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        };

        // An alias name is scoped to the account AND Region, so the Region the
        // request was signed for is part of the alias's identity: it keys the
        // record, decides which names collide, and names the Region in the ARN.
        let region = crate::state::request_region(ctx.parts());

        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let mut state = state.lock();

        // The policy store is the resource this request REFERENCES, so it is
        // resolved BEFORE any of the request's own payload is read: a reference
        // that names no store answers `ResourceNotFoundException` whatever else
        // the request got wrong. Resolving is a READ, so a request refused below
        // still changes nothing.
        //
        // `policyStoreId` is documented as an ID here too — "The alias name
        // cannot be used" — so it goes through the ID-only lookup instead of
        // `resolve_store_id`, which would also accept an alias reference.
        if state
            .resolve_store_id_only(&input.policy_store_id)
            .is_none()
        {
            return Err(
                CreatePolicyStoreAliasErrorModeled::ResourceNotFoundException(
                    crate::ops::resource_not_found(&input.policy_store_id, "POLICY_STORE"),
                )
                .into(),
            );
        }

        // Only once the referenced store has resolved is the request's own
        // payload read. The documented prefix is a rule the model's `pattern`
        // cannot express — it admits any `[a-zA-Z0-9-/_]*` — and a name without
        // it could never be referenced through a `policyStoreId` member, so the
        // request is refused rather than persisted as unreachable. The check is
        // the one every operation taking this member shares.
        if let Some(invalid) = crate::ops::invalid_alias_name(&input.alias_name) {
            return Err(CreatePolicyStoreAliasErrorModeled::ValidationException(invalid).into());
        }

        // The name is unique in the account and Region: a duplicate of the SAME
        // association is the documented no-op, and anything else holding the name
        // IN THIS REGION conflicts. A name held only in another Region is free.
        if let Some(existing) = state.alias(&region, &input.alias_name) {
            if existing.state == crate::state::ALIAS_STATE_ACTIVE
                && existing.policy_store_id == input.policy_store_id
            {
                let output = output_for(existing);
                crate::ops::audit(
                    ctx,
                    "CreatePolicyStoreAlias",
                    &output.alias_name,
                    "already created",
                );
                return Ok(output);
            }
            return Err(already_taken(existing).into());
        }

        // The alias table is the one structure this request grows, so the bound is
        // checked before anything is persisted.
        if state.aliases.len() >= crate::state::MAX_POLICY_STORE_ALIASES {
            return Err(
                CreatePolicyStoreAliasErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: format!(
                            "this account already holds the maximum of {} policy store aliases",
                            crate::state::MAX_POLICY_STORE_ALIASES
                        ),
                        resource_type: "POLICY_STORE_ALIAS".to_string(),
                        ..Default::default()
                    },
                )
                .into(),
            );
        }

        let alias = crate::state::PolicyStoreAlias {
            alias_name: input.alias_name.clone(),
            // The alias ARN names the region the request was signed for: an
            // alias name is unique in the account and Region, so the ARN that
            // identifies it has to say which Region — the same Region the record
            // is keyed under, so the ARN and the key cannot disagree.
            arn: crate::state::alias_arn(&region, &input.alias_name),
            policy_store_id: input.policy_store_id.clone(),
            created_at: crate::state::now(),
            state: crate::state::ALIAS_STATE_ACTIVE.to_string(),
        };
        state.insert_alias(&region, alias);

        // Read the response back out of what was just persisted.
        let stored = state
            .alias(&region, &input.alias_name)
            .ok_or_else(unavailable)?;
        let output = output_for(stored);
        crate::ops::audit(ctx, "CreatePolicyStoreAlias", &output.alias_name, "created");
        Ok(output)
    }
}
