Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 3ee3ad8755f7cbca1472e043fe3407ec62e0eff1

# The aliasName prefix refusal runs before the policy store is resolved, so a request that is wrong in both ways answers 400 instead of the 404 the crate's rule states

_Filed: 2026-10-29T11:39:25Z_
_Operation: CreatePolicyStoreAlias | Class: error-response_

> _Note: this issue was identified against the code base at commit `3ee3ad8755f7cbca1472e043fe3407ec62e0eff1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreatePolicyStoreAlias` refuses an `aliasName` that does not carry the documented `policy-store-alias/` prefix, and it does so before it ever reaches the state store: the check sits above `crate::state::service_state(...)`/`state.lock()` and therefore above the `resolve_store_id_only` lookup that produces the operation's modelled `ResourceNotFoundException`.

The result is that a request naming BOTH an unprefixed alias name and a `policyStoreId` no store carries is answered 400 `ValidationException`, while the crate's own rule — resolve every resource the request REFERENCES before reading the request's own payload — says the missing store outranks the payload refusal and the answer is 404 `ResourceNotFoundException`. The alias name is this request's payload; the policy store is the resource it references.

This is the same precedence inversion already recognised and fixed for `IsAuthorized` (fixed issue 20261029T005600Z-verifiedpermissions-isauthorized-error-response), and unlike `BatchIsAuthorized`'s deliberately hoisted batch-size check, nothing in this handler states a reason for running the prefix check ahead of the lookup — a prefix comparison is no cheaper to do before the lock than after it, since the handler takes the lock unconditionally on every other path.

## Evidence

The rule, stated by the suite that pins it — services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/tests/all/not_found_precedes_payload_refusal.rs:7-13:
```
//! The rule under test: an operation resolves every resource the request
//! REFERENCES before it reads the request's own payload, so a reference that
//! names nothing answers the 404 `ResourceNotFoundException` whatever else the
//! request got wrong — the same answer whichever operation the reference arrived
//! on.
```
The same rule, stated and implemented by a sibling — src/ops/put_schema.rs:144-149:
```
        // The store is resolved FIRST: a schema is placed in a policy store, so a
        // reference that names no store is a missing resource whatever else the
        // request carries.
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
```
The path that breaks it — src/ops/create_policy_store_alias.rs:161-194, payload refusal first:
```
        // The documented prefix is a rule the model's `pattern` cannot express —
        // it admits any `[a-zA-Z0-9-/_]*` — and a name without it could never be
        // referenced through a `policyStoreId` member, so the request is refused
        // rather than persisted as unreachable.
        if !input.alias_name.starts_with(crate::state::ALIAS_PREFIX) {
            return Err(CreatePolicyStoreAliasErrorModeled::ValidationException(
                crate::ops::validation_exception([crate::ops::InvalidInput::new(
                    crate::ops::paths::ALIAS_NAME,
```
and only then the reference it should have resolved first — same file, lines 178-194:
```
        let state = crate::state::service_state(&*self.engine).ok_or_else(unavailable)?;
        let mut state = state.lock();
        ...
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
```
The operation models the 404 that is being skipped: smithy_query(query="operation", operation="CreatePolicyStoreAlias") lists `ResourceNotFoundException (HTTP 404)`, and `CreatePolicyStoreAliasInput$policyStoreId` is documented as "The associated policy store must be specified using its ID".

Test coverage: tests/all/not_found_precedes_payload_refusal.rs's `FAULTS` table (lines 96-261) has no `CreatePolicyStoreAlias` row, and tests/all/policy_store_alias.rs exercises the prefix refusal only against a store that exists, so nothing pins or justifies the divergence.

## Steps to Reproduce

1. Boot the emulator and point a Verified Permissions client at its data listener.
2. Call CreatePolicyStoreAlias { aliasName: "no-prefix", policyStoreId: "PSdoesnotexist11111111" }.
3. Observed: HTTP 400 ValidationException with fieldList naming `/aliasName`.
4. Expected per the crate's stated rule: HTTP 404 ResourceNotFoundException with resourceType `POLICY_STORE` and resourceId `PSdoesnotexist11111111`, the answer that same reference earns from PutSchema, CreateIdentitySource, GetPolicyStore and ListPolicies.
5. Contrast: CreatePolicyStoreAlias { aliasName: "policy-store-alias/ok", policyStoreId: "PSdoesnotexist11111111" } does answer 404 — so whether an unknown store is reported depends on whether the alias name also happens to be faulty.
