Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 35244fd3792ee15123773e1aba2707f16463d58b

# Replace the hand-rolled page-token mint/verify with service_common::page_tokens

_Filed: 2026-11-13T05:49:27Z_
_Operation: ListLinuxSubscriptionInstances | Class: maintainability_

> _Note: this issue was identified against the code base at commit `35244fd3792ee15123773e1aba2707f16463d58b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paged list of this service (`ListLinuxSubscriptionInstances`, `ListLinuxSubscriptions`, `ListRegisteredSubscriptionProviders`) mints and verifies its `NextToken` through a cursor scheme hand-rolled in `src/state.rs`: `encode_page_token` / `decode_page_token` / `list_identity`, built on `hex` + `sha2` dependencies added to this crate for exactly that purpose. The workspace already has ONE implementation of that job — `service_common::page_tokens::PageTokenKey`, reached through the `service-common` dependency this crate already uses in four files — and 14 other service crates bind their cursors with it.

The hand-rolled span is not just a second copy, it is a WEAKER one: the only thing `decode_page_token` checks is that the token carries `list_identity(kind)`, and `list_identity` is a plain UNKEYED SHA-256 over `kind`, while `kind` is derived entirely from the request the caller sent (`format!("linux-subscription-instances/{}", filter_signature(&filters))`). A caller can therefore compute the digest itself, hex-encode `[identity, ["i-0zzz"]]`, and hand the service a cursor it never minted — which the handler then serves a page for, even though `paginate`'s own doc says such a value is refused. `PageTokenKey` is the module that exists to make that distinction: it authenticates the cursor under a secret the store draws from system randomness and never reveals.

The fix is a swap inside `src/state.rs`: hold a `PageTokenKey` on the state struct, mint `key.mint(<list identity + cursor key>)` and read it back with `key.cursor_of(..)`, keeping this service's existing hex spelling of the bytes and the same `StateError::Validation` refusal. `NextToken` stays an opaque, fixed-size-per-list string, so every response member a client reads and every existing assertion about cursors keeps holding; the `hex`/`sha2` additions in Cargo.toml made only for this span can go with it.

## Evidence

The hand-rolled span, `services/.../src/state.rs:1075-1084` and `:1100-1103`:
```
fn decode_page_token(kind: &str, token: &str) -> Result<Vec<String>, StateError> {
    let invalid = || StateError::Validation("NextToken is not a valid pagination token".to_string());
    let decoded = ::hex::decode(token).map_err(|_| invalid())?;
    let (token_identity, key): (String, Vec<String>) =
        ::serde_json::from_slice(&decoded).map_err(|_| invalid())?;
    if token_identity != list_identity(kind) {
        return Err(invalid());
    }
    Ok(key)
}
...
fn list_identity(kind: &str) -> String {
    use ::sha2::Digest as _;
    ::hex::encode(::sha2::Sha256::digest(kind.as_bytes()))
}
```
The rule that span states for itself, `src/state.rs:987-991`:
```
/// `MaxResults` is the page size; a `NextToken` is this service's opaque cursor, minted
/// by the previous page of the SAME list (`kind` names that list: the operation and
/// whatever selected it) and rejected otherwise — a token is not a client-chosen value,
/// so an unparseable or foreign one is a `ValidationException` rather than a silent
/// restart from the beginning.
```
The crate API that replaces it, `services/common/src/page_tokens.rs:3` and `:8-17`:
```
//! The ONE way a service tells a continuation token it ISSUED from one a caller built.
...
//! that shape, and a caller can name a position the service never offered: the first
//! page of a report it was paging past, a position far in the future, a position under
//! another filter. What the services' APIs document for such a value is a refusal (an
//! invalid-token error), not a page.
//!
//! So a token carries an AUTHENTICATOR over its own cursor, computed under a secret the
//! issuing store draws from the system's randomness and never reveals ([`PageTokenKey`]):
//! the cursor stays readable by the service that minted it, and no caller can produce a
//! cursor the key authenticates.
```
That crate is already a dependency of THIS crate — `services/.../Cargo.toml:35` `service-common = { path = "../common" }` — while the two dependencies carrying the hand-rolled span were added only for it, `Cargo.toml:24-28`:
```
# Encodes/decodes this service's opaque pagination cursors.
hex = { workspace = true }
# SHA-256 over the description of the list a pagination cursor belongs to, so the cursor
# binds to that list at a fixed size instead of carrying the request's whole selection.
sha2 = { workspace = true }
```
The three call sites of the span: `src/ops/list_linux_subscription_instances.rs:191-205`, `src/ops/list_linux_subscriptions.rs:194-213`, `src/ops/list_registered_subscription_providers.rs:174-190` (all via `crate::state::paginate`).

Test coverage that must keep passing unchanged, and does: `tests/all/page_token_length_bound.rs:265-291` asserts only that a minted cursor is `1..=MAX_NEXT_TOKEN` characters and that its length does not vary with the request, and `:294-326` that a cursor of one selection is refused (400 `ValidationException`) for another selection and for another list; `tests/all/paged_walk_under_writes.rs` and `tests/all/walk_under_a_discovery_pass.rs` walk pages end to end without asserting any token's bytes.

## Steps to Reproduce

1. Read `services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/src/state.rs:1060-1103` — `encode_page_token`, `decode_page_token` and `list_identity` are this crate's own mint/verify of a continuation token.
2. Read `services/common/src/page_tokens.rs:1-95` — `PageTokenKey::mint` / `PageTokenKey::cursor_of` is the workspace's one implementation of that job, and `services/.../Cargo.toml:35` shows `service-common` is already a dependency of this crate (grep `service_common::` in `src/` for the four existing uses).
3. Grep `page_tokens::PageTokenKey` under `services/` — 14 other service crates bind their cursors with it; this crate instead carries `hex` + `sha2` (Cargo.toml:24-28) for the hand-rolled span.
4. Confirm the hand-rolled check is unkeyed and therefore reproducible by a caller: for `POST /subscription/ListLinuxSubscriptionInstances` with no `Filters`, `kind` is `"linux-subscription-instances/"`; compute `hex(json([hex(sha256(kind)), ["i-0000"]]))` and send it as `NextToken` — the list serves a page for a cursor it never minted, where `src/state.rs:987-991` says a token "is not a client-chosen value" and a foreign one is a `ValidationException`.
5. Swap the three functions onto `PageTokenKey` (key held on the state struct, same hex spelling on the wire, same `StateError::Validation` refusal) and re-run `tests/all/page_token_length_bound.rs`, `tests/all/paged_walk_under_writes.rs` and `tests/all/walk_under_a_discovery_pass.rs`: the length bound, the fixed size per list and the cross-list refusal all still hold.
