Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: e635fc8db7925c597029a243d97ecda6419e6d1a

# No portal session or account assignment is ever created outside this crate's own tests, so all four operations answer 401 to every client of the running emulator

_Filed: 2026-10-30T07:17:06Z_
_Operation: ListAccounts | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `e635fc8db7925c597029a243d97ecda6419e6d1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the four portal operations reports out of `ComAmazonawsSsoSwbportalserviceState` and refuses a request whose presented access token resolves to no session (`state.session(...) -> None` => `UnauthorizedException`). The only code that can put a session or an account into that store is `put_session` / `put_account`, and `state.rs` itself names where a session is supposed to come from: "a sign-in session is minted elsewhere (`CreateToken` in the OIDC service)". But nothing outside this crate's own test fixtures ever calls either seeder: in the emulator the router actually mounts (`core/router/src/lib.rs:944`), `sessions` and `accounts` are permanently empty.

The consequence is that the whole service is unusable end to end in the running emulator, not just at an edge: a client that obtains an access token from the emulator's own `sso-oidc` `CreateToken` (which does persist it, `access_tokens: HashMap<String, IssuedToken>`) and then calls `ListAccounts`, `ListAccountRoles`, `GetRoleCredentials` or `Logout` with that token gets 401 `UnauthorizedException` "Session token not found or invalid" every time, forever — the portal never learns that the token names a signed-in user, and `ListAccounts` can never report an account because no account record exists either. The handlers are correct in isolation; what is missing is the wiring from the token mint (and from the IAM Identity Center assignment records held in the same engine) into this service's store, which is the pattern the SSO family already uses elsewhere in this repo (`services/service-com-amazonaws-ssooidc-awsssooidcservice/src/state.rs:888` resolves an access-portal URL by reading IAM Identity Center's instance records out of the SAME shared engine).

Filed against `ListAccounts`, the first operation an SSO client calls and the first handler that reads `state.session`; the same single cause makes the other three unusable.

## Evidence

The crate states where a session comes from — `services/service-com-amazonaws-sso-swbportalservice/src/state.rs:5-10`:
```
//! The IAM Identity Center portal has no create API of its own: a sign-in session is
//! minted elsewhere (`CreateToken` in the OIDC service) and the account/role
//! assignments are made by an administrator. So everything the four portal operations
//! report is held here and seeded through [`ComAmazonawsSsoSwbportalserviceState::put_session`]
//! / [`ComAmazonawsSsoSwbportalserviceState::put_account`]; no handler ever synthesizes
//! an account, a role or a session.
```
and `src/state.rs:185-188`:
```
    /// The portal models no create API, so this is how the assignments an
    /// administrator would have made get into the emulator: a test, or whatever stands
    /// an emulator up, calls it through [`service_state`].
```
Nothing stands it up. A repo-wide grep for the two seeders finds ONLY this crate's tests:
- `services/service-com-amazonaws-sso-swbportalservice/tests/all/portal_session.rs:94` (`store.put_account(...)`), `:108` (`put_session`)
- `tests/all/endpoint_agnostic_routing.rs:127` (`put_account`), `:144` (`put_session`)
- `tests/all/bearer_token_gate.rs:224` (`put_session`)
- `tests/all/unauthorized_rejection.rs:223` (`put_session`)
and the only production mention of the crate is the registration `core/router/src/lib.rs:944`:
```rust
    router.register_service(service_com_amazonaws_sso_swbportalservice::EmulatedService::new(engine.clone()));
```

With the store empty, the gate is unconditional — `src/state.rs:237-241`:
```rust
    pub fn session(&self, access_token: &str, now: SystemTime) -> Option<&Session> {
        let resolved = self
            .sessions
            .get(access_token)
            .filter(|session| session.is_usable_at(now));
```
and each handler turns that `None` into 401, e.g. `src/ops/list_accounts.rs:173-180`:
```rust
        let Some(session) = state.session(&input.access_token, now) else {
            return Err(ListAccountsErrorModeled::UnauthorizedException(
                crate::state::unauthorized_session(),
            )
            .into());
```
(identically `src/ops/list_account_roles.rs:166-172`, `src/ops/get_role_credentials.rs:170-181`, `src/ops/logout.rs:185-191`, plus the pre-handler gate `src/service.rs:371-373`).

The token the model points at IS persisted by the sibling SSO OIDC emulator in the same engine — `services/service-com-amazonaws-ssooidc-awsssooidcservice/src/state.rs:538`:
```rust
    pub access_tokens: HashMap<String, IssuedToken>,
```
and that crate already reads ANOTHER service's records out of the shared engine (`.../ssooidc.../src/state.rs:888` `pub fn names_account_access_portal<Eng: Send + Sync + 'static>(`), so the seam a fix needs exists and is already used in this family.

Test coverage: every portal test seeds the store by hand before asserting (`tests/all/portal_session.rs:85-114`, `tests/all/endpoint_agnostic_routing.rs:120-150`), so the suites pass while the deployed emulator has nothing in the store; no test dispatches a portal request against a store populated the way a running emulator populates it (i.e. not at all).

## Steps to Reproduce

1. Start the emulator that `core/router/src/lib.rs:944` builds (all services on one shared `InMemoryEngine`), with no test fixture code involved.
2. Obtain an access token from the emulator's own SSO OIDC service: `RegisterClient` -> `StartDeviceAuthorization` -> `CreateToken`, which persists it in `access_tokens` (`services/service-com-amazonaws-ssooidc-awsssooidcservice/src/state.rs:538`).
3. Call `GET /assignment/accounts` with header `x-amz-sso_bearer_token: <that access token>` (i.e. `aws sso list-accounts --access-token <tok>`).
4. Observed: 401 `UnauthorizedException` with message "Session token not found or invalid" — the portal's `sessions` map is empty because no code outside this crate's tests calls `put_session`. The same holds for `GET /assignment/roles`, `GET /federation/credentials` and `POST /logout`, on every token and forever.
5. Expected per `src/state.rs:5-10` ("a sign-in session is minted elsewhere (`CreateToken` in the OIDC service)"): the token minted by the emulator's own `CreateToken` names a usable portal sign-in session, so `ListAccounts` reports the accounts assigned to it (and `ListAccountRoles` / `GetRoleCredentials` serve that assignment), instead of every portal call being unauthorized.
