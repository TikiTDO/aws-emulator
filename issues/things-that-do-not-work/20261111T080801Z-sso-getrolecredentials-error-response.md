Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 34aa023e13a037c836015cae341f7a7e68727fe7

# The pre-handler session gate resolves a token against un-swept state, skipping the expiry every handler applies first — so it can refuse a token the handler would have served

_Filed: 2026-11-11T08:08:01Z_
_Operation: GetRoleCredentials | Class: error-response_

> _Note: this issue was identified against the code base at commit `34aa023e13a037c836015cae341f7a7e68727fe7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of this service's four handlers takes the store lock and runs `crate::state::end_expired_credentials(&mut state, …, now)` BEFORE it asks `state.signed_in(..)` what the presented token names. `end_expired_credentials` is not only a credential sweep: its tail calls `ComAmazonawsSsoSwbportalserviceState::expire_sessions`, which REMOVES sign-in records from `self.sessions`.

That removal is observable by `signed_in`, because `signed_in` branches on whether this store holds a record for the token at all: a record that is present and unusable short-circuits to `None` (refuse), while NO record falls through to the sign-in resolved from the emulator's own token mint. So sweeping first and sweeping later are not the same question.

The pre-handler gate — `SessionGate::presents_a_usable_session`, the seam `EmulatedService::override_operation` and `override_routing_error` both consult, and the one that decides the request's fate before any handler runs — locks the same store and calls the same `signed_in`, but does NOT run `end_expired_credentials` first. It is the only caller of `signed_in` in the crate that skips the sweep (the gate's own `ErasedOperation::run` runs it, and says it does so "exactly as each handler ends it" — but only AFTER `presents_a_usable_session` has already decided).

Consequence: for a token this store holds a spent record of (expiry passed, no credential outstanding) while the emulator's token mint still holds it as valid, the gate answers 401 `UnauthorizedException` and the handler never runs — although the handler's own rule (sweep, then resolve) would have reclaimed the spent record and served the request from the mint. That is exactly what the gate's own documentation says cannot happen, and what `expire_sessions`'s documentation says the sweep never does.

Scope, stated honestly: reaching the divergence needs one token string registered in BOTH stores (a portal-local `put_session` record that has gone spent, plus a live record in the OIDC mint), which is reachable through the two crates' public `state` seeding APIs rather than from an ordinary client — the same scope the already-filed cursor-scope issue for this service was accepted under. The defect being filed is the skipped normalisation itself: the gate answers from state the handlers normalise before they read it, which is a check/expiry the siblings apply and this one omits.

## Evidence

The gate's own claim that it cannot disagree with a handler — services/service-com-amazonaws-sso-swbportalservice/src/service/session_gate.rs:53-65:
```rust
    /// Whether `req` presents a sign-in session this service still serves at the moment
    /// it is asked.
    ///
    /// `false` covers every way a request can fail to present one — no access-token
    /// header at all, or one naming a session that is unknown, logged out or expired —
    /// because the portal authenticates a caller by that session and distinguishes none
    /// of them. It is the same question
    /// [`crate::state::ComAmazonawsSsoSwbportalserviceState::signed_in`] answers for a
    /// handler, asked of a request that has not reached one — resolved against the same
    /// two sources, so the gate cannot refuse a token a handler would have served.
```
and the body, which locks the store and resolves WITHOUT the sweep — session_gate.rs:80-85:
```rust
        let now = SystemTime::now();
        // Resolved before this store is locked, exactly as a handler does it.
        let federated = crate::state::federated_sign_in(&*self.engine, token, now);
        let state = state.lock();
        state.signed_in(token, federated.as_ref(), now).is_some()
```

What every sibling does instead — the sweep runs BEFORE `signed_in`:
* src/ops/list_accounts.rs:174-181:
```rust
        let mut state = state.lock();
        ...
        crate::state::end_expired_credentials(&mut state, ctx.credential_store(), now);
        let Some(signed_in) = state.signed_in(&input.access_token, federated.as_ref(), now) else {
```
* src/ops/get_role_credentials.rs:171-177 (`crate::state::end_expired_credentials(&mut state, ctx.credential_store(), now);` then `let Some(signed_in) = state.signed_in(...)`)
* src/ops/list_account_roles.rs:167-171 (same two lines, in the same order)
* src/ops/logout.rs:190-192 (same two lines, in the same order)

The rule the state module states for that call — src/state.rs:402-403:
```rust
/// Every handler calls this before it serves, because the expiration is due at its
/// instant and not at the next request for the same role: a credential nobody asks about
```
and its tail, which is the part that moves what `signed_in` answers — src/state.rs:425-427:
```rust
    // After the credentials, so a record still speaking for a usable credential is kept
    // and every other expired record is reclaimed.
    state.expire_sessions(now);
```
with `expire_sessions` dropping records and claiming no response depends on it — src/state.rs:345-348 and :366-368:
```rust
    /// A sign-in's expiry is the instant the CLOCK makes the record spent — past it the
    /// record answers nothing, because an expired sign-in is refused whether it is held
    /// here or resolved from the mint — so dropping it is owed then and no response changes
    /// because this ran.
```
```rust
        self.sessions.retain(|access_token, session| {
            session.expires_at > now || issued_to.contains(access_token.as_str())
        });
```
Why the drop DOES change the answer — src/state/sign_in.rs:253-262, where a present-but-unusable record short-circuits to `None` while an ABSENT record falls through to the mint:
```rust
        let recorded = self.sessions.get(access_token);
        let resolved = match recorded {
            Some(session) if !session.is_usable_at(now) => None,
            _ => match federated.filter(|sign_in| sign_in.is_usable_at(now)) {
                Some(sign_in) => Some(sign_in.clone()),
                None => recorded
                    .filter(|session| !session.federated)
                    .map(|session| self.seeded_sign_in(session)),
            },
        };
```
The gate's decision is final for the request — src/service.rs:326-335:
```rust
        self.entry.routing_table().get(routing_key)?;
        (!self.session_gate.presents_a_usable_session(req))
            .then_some(&self.session_gate as &dyn ErasedOperation)
```
and the gate's refusal path sweeps only AFTER that decision — session_gate.rs:104-116 ("The one write every portal request makes … it is ended here exactly as each handler ends it").

Test coverage: no suite asks `signed_in` across the sweep boundary. `tests/all/credential_expiry.rs:157-204` drives the sweep through every operation but never puts a token in both stores; `tests/all/bearer_token_gate.rs:184-230` seeds `tok-expired`/`tok-logged-out` in the portal store only; `tests/all/minted_sign_in.rs:1076` (`a_spent_sign_in_record_is_reclaimed`) exercises the reclamation for mint-only tokens. So the gate-versus-handler ordering is unpinned.

## Steps to Reproduce

1. Stand up a router mounting the portal and the IAM Identity Center OIDC service on one shared `engine::InMemoryEngine`, as tests/all/minted_sign_in.rs:101-130 does.
2. Make the emulator's own mint hold a live access token `T` (an OIDC `CreateToken` flow as in tests/all/minted_sign_in.rs, or seed `oidc::state::service_state(&engine).lock().access_tokens` with an unexpired `IssuedToken` for `T`), and make at least one IAM Identity Center account assignment so `federated_sign_in` resolves an account for it.
3. ALSO give the portal store a spent record of the same token string and no credential: `portal::state::service_state(&engine).unwrap().lock().put_session(T, "x@example.com", ["111111111111"], Duration::ZERO)` — `put_session` dates the expiry from now (src/state.rs:288), so the record is already past its expiry.
4. Dispatch `GET /assignment/accounts` with `x-amz-sso_bearer_token: T`.
5. Observed: 401 `UnauthorizedException` with the gate's constant message — `override_operation` (src/service.rs:333) asked `presents_a_usable_session`, which locked the store without running `end_expired_credentials`, so the spent record was still there and `signed_in` short-circuited to `None` (src/state/sign_in.rs:255).
6. Expected per the handler's own order (src/ops/list_accounts.rs:180-181) and the gate's own doc (src/service/session_gate.rs:60-62): the sweep reclaims the spent record (src/state.rs:366-368, no credential keeps it alive), `signed_in` then resolves `T` from the mint, and the accounts are served — the gate must not refuse what the handler would serve.
7. Verify without HTTP, on one store: call `state.signed_in(T, federated.as_ref(), now)` (answers `None`), then `crate::state::end_expired_credentials(&mut state, credential_store, now)`, then call `state.signed_in(T, federated.as_ref(), now)` again (answers `Some`) — one sweep, two different answers to the question src/state.rs:345-348 says no response depends on.
