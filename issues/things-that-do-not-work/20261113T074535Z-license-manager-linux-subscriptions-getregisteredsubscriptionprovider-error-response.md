Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 92fe44509b790ef44b44d785952419008ad12946

# Cross-account ARN gate is applied by the tag handlers and skipped by the provider-plane handlers that address the same resource

_Filed: 2026-11-13T07:45:35Z_
_Operation: GetRegisteredSubscriptionProvider | Class: error-response_

> _Note: this issue was identified against the code base at commit `92fe44509b790ef44b44d785952419008ad12946`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::service::authorization::authorize_resource_arn` denies a request whose ARN names an account other than the emulator's, with HTTP 403 `AccessDeniedException`, BEFORE the store is consulted. All three tag handlers call it. `GetRegisteredSubscriptionProvider` and `DeregisterSubscriptionProvider` do not: they hand the caller-supplied `SubscriptionProviderArn` straight to `resolve_provider`, so an ARN carrying a foreign account id is answered `ResourceNotFoundException` instead.

The authorization module justifies the split with a premise the crate's own state module contradicts: it claims the tag APIs are "the only operations of this service that take an ARN ... from the caller rather than a `SubscriptionProviderArn` this account minted", while `state.rs` documents that the tag APIs' `resourceArn` IS a `SubscriptionProviderArn` and that a registered subscription provider is the only resource they can address — the very same resource, reached through the very same `providers` lookup, as the two operations that skip the gate. The two members also carry the identical modelled pattern, so the account segment is caller-chosen in both.

The result is that one resource kind answers two different things for the same foreign-account ARN: 403 through `/tags/{resourceArn}` and 400 `ResourceNotFoundException` through `/subscription/GetRegisteredSubscriptionProvider` (and `/subscription/DeregisterSubscriptionProvider`) — exactly the confusion the gate was introduced to prevent, per the comment above its call site.

## Evidence

The premise, `src/service/authorization.rs:6-16`:
```
//! The tag APIs (`/tags/{resourceArn}`) are the only operations of this service that take an
//! ARN of "the Amazon Web Services resource" from the caller rather than a `SubscriptionProviderArn`
//! this account minted, so they are the only ones whose request can name a resource of
//! ANOTHER account. ... That is why the gate is stated here once and applied by all three handlers
//! before they touch the store: a denial must not depend on whether this account happens to
//! hold a resource under the same id, and a refusal of absence must never stand in for one
//! of authorization
```

The crate's own contradiction of that premise, `src/state.rs:726-733`:
```
    /// The metadata tags of the resource `arn` addresses, or `None` when this account
    /// holds no such resource.
    ///
    /// The tag APIs accept a `SubscriptionProviderArn`, so a registered subscription
    /// provider is the only resource of this service they can address: every tag
    /// operation resolves its `resourceArn` through this one lookup
```

Gate applied — `src/ops/list_tags_for_resource.rs:166-170`:
```
        // AUTHORIZATION first, from the `resourceArn` alone and ahead of the lookup: a read
        // of a resource in another account is denied rather than reported absent, so a
        // caller enumerating ARNs across accounts learns that it lacks access instead of
        // concluding the resources do not exist.
        crate::service::authorization::authorize_resource_arn(&input.resource_arn)?;
```
(same call at `src/ops/tag_resource.rs:172` and `src/ops/untag_resource.rs:171`).

Gate skipped — `src/ops/get_registered_subscription_provider.rs:176-180`:
```
        let mut state = state.lock();
        state.settle_providers(::std::time::SystemTime::now());
        let record = state
            .resolve_provider(&input.subscription_provider_arn)
            .map_err(into_error)?;
```
and `src/ops/deregister_subscription_provider.rs:154-157`:
```
        state
            .resolve_provider(&input.subscription_provider_arn)
            .map_err(into_error)?;
        state.remove_provider(&input.subscription_provider_arn);
```
`resolve_provider` answers `StateError::NotFound` (`src/state.rs:498-502`), which `into_error` renders as `ResourceNotFoundException`.

The two members are identically patterned — `OPERATIONS.md:37` (`SubscriptionProviderArn` ... pattern `^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,510}/[a-z0-9-\.]{1,510}$`) and `OPERATIONS.md:93` (`resourceArn` ... the same pattern) — so neither member constrains the account segment.

Test coverage: `tests/all/tag_plane_authorization.rs:10-17` pins the 403 for the tag plane only ("on every operation of the surface"); no suite exercises a foreign-account `SubscriptionProviderArn` against the provider plane.

## Steps to Reproduce

1. Register a provider so the emulator holds one, and note the account id the emulator mints ARNs under.
2. GET /tags/arn%3Aaws%3Alicense-manager-linux-subscriptions%3Aus-east-1%3A999999999999%3Asubscription-provider%2F00000000-0000-4000-8000-000000000001 — observe HTTP 403 AccessDeniedException (the store is never consulted).
3. POST /subscription/GetRegisteredSubscriptionProvider with {"SubscriptionProviderArn":"arn:aws:license-manager-linux-subscriptions:us-east-1:999999999999:subscription-provider/00000000-0000-4000-8000-000000000001"} — the identical ARN, the identical resource kind — and observe HTTP 400 ResourceNotFoundException instead.
4. Repeat step 3 against /subscription/DeregisterSubscriptionProvider for the same divergence.
