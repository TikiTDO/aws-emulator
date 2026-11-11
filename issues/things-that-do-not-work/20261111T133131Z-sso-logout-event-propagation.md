Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: dcee3c715dd1f76e7da2147c1b920fea18ed74e4

# Logout ends only the presented access token, not the sign-in session: spending the refresh token the same sign-in holds re-animates portal access and mints NEW role credentials

_Filed: 2026-11-11T13:31:31Z_
_Operation: Logout | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `dcee3c715dd1f76e7da2147c1b920fea18ed74e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Logout` is documented to invalidate the server-side IAM Identity Center *sign-in session*, with exactly one carve-out: the IAM role sessions that ALREADY exist continue for the duration configured in the permission set. The handler implements something narrower: it deactivates the one record keyed by the presented access-token STRING (`invalidate_session(&input.access_token, now)`), and writes nothing to the OIDC token mint where that sign-in's tokens actually live.

The emulator's own OIDC service hands a client a refresh token beside the access token, and `CreateToken` with `grantType=refresh_token` rotates it into a BRAND-NEW access token for the same `clientId` (`rotate_refresh_token` → `issue_token(client_id, …)`, inserted into `access_tokens`). The portal store holds no record keyed by that new string, so `ComAmazonawsSsoSwbportalserviceState::signed_in` falls through to `federated_sign_in`, which resolves the new token straight from the mint and serves it. The logged-out sign-in is therefore fully usable again, with no further user authentication: `ListAccounts`/`ListAccountRoles` answer 200 again and `GetRoleCredentials` mints *new* role credentials — not the "existing IAM role sessions" the model says continue after logout.

The crate itself states that a refresh does not change which sign-in a token belongs to (`minted_sign_in`: "rotating a refresh token re-issues against the same `clientId` — so a credential minted before a refresh speaks for the same subject as one minted after"), and that `Session::active` is "Cleared by `Logout`, which invalidates the server-side sign-in session". Both sentences describe a sign-in that outlives any single token, while the only thing the code can end is a token string. The Logout event is never propagated to the mint (nor is the spent sign-in's refresh grant consulted), so the one state change this operation exists to make is undone by an ordinary client call to another emulated service.

This is NOT a request to refuse a rotated token in general: the suite's rule that "the token the rotation issued is served" (tests/all/minted_sign_in.rs:981-985) is about a sign-in nobody logged out of, and stays true. The defect is that a rotation performed AFTER a `Logout` resurrects the sign-in that `Logout` reported it had ended.

## Evidence

The model sentence the operation is named for — smithy_query(query="doc", shape="Logout"):
```
<p>Removes the locally stored SSO tokens from the client-side cache and sends an API call to
      the IAM Identity Center service to invalidate the corresponding server-side IAM Identity Center sign in
      session.</p>
…
<p>After user logout, any existing IAM role sessions that were created by using IAM Identity Center
        permission sets continue based on the duration configured in the permission set.</p>
```
The handler quotes that same note to justify what it leaves alone, so the carve-out is the crate's own reading — services/service-com-amazonaws-sso-swbportalservice/src/ops/logout.rs:183-189:
```rust
        // Invalidating the sign-in session is what this operation DOES. The role
        // credentials already minted for it are deliberately left alone: the model's own
        // documentation says role sessions created through a permission set "continue
        // based on the duration configured in the permission set" after logout.
```
What it actually ends — logout.rs:203-204:
```rust
        state.remember_sign_in(&input.access_token, &signed_in);
        if !state.invalidate_session(&input.access_token, now) {
```
keyed by the token STRING — src/state.rs:308-311:
```rust
    pub fn invalidate_session(&mut self, access_token: &str, now: SystemTime) -> bool {
        match self.sessions.get_mut(access_token) {
            Some(session) if session.is_usable_at(now) => {
                session.active = false;
```
while the field it clears is documented as the SESSION's end — src/state.rs:54:
```rust
    /// Cleared by `Logout`, which invalidates the server-side sign-in session.
```
Any OTHER token of the same sign-in is resolved from the mint with nothing consulted about the logout — src/state/sign_in.rs:14-15 and :121-130:
```
//! * an access token names a sign-in exactly when the emulator's own OIDC service issued
//!   it and it is still inside its own lifetime;
```
```rust
fn minted_sign_in<Eng: Send + Sync + 'static>(…) -> Option<(String, SystemTime)> {
    let tokens = oidc::state::service_state(engine)?;
    let tokens = tokens.lock();
    let issued = tokens.access_tokens.get(access_token)?;
    (!issued.is_expired(now)).then(|| (issued.client_id.clone(), issued.expires_at()))
}
```
and `signed_in` serves it, because this store holds no record under the new string — src/state/sign_in.rs:253-262:
```rust
        let recorded = self.sessions.get(access_token);
        let resolved = match recorded {
            Some(session) if !session.is_usable_at(now) => None,
            _ => match federated.filter(|sign_in| sign_in.is_usable_at(now)) {
                Some(sign_in) => Some(sign_in.clone()),
```
The crate's own statement that a refresh keeps the SAME sign-in — src/state/sign_in.rs:113-116:
```
/// is the identity the emulator records for it. It is stable for the life of that
/// registration — rotating a refresh token re-issues against the same `clientId` — so a
/// credential minted before a refresh speaks for the same subject as one minted after.
```
The wire-reachable rotation that produces the new token — services/service-com-amazonaws-ssooidc-awsssooidcservice/src/state.rs:772-774 (reached from create_token.rs:464-471 on `grantType=refresh_token`):
```rust
        self.refresh_tokens.remove(refresh_token);
        self.access_tokens.remove(&grant.bound_access_token);
        Ok(self.issue_token(client_id, region, scopes, true, now))
```
and `issue_token` persisting it for the same client — oidc src/state.rs:697-724 (`let access_token = mint_opaque_value(); … self.access_tokens.insert(access_token, token.clone());` with `client_id: client_id.to_string()`).

Test coverage: the two halves are pinned separately and never composed. `tests/all/minted_sign_in.rs:913-979` (`logout_ends_a_minted_sign_in_for_good`) logs out and re-presents ONLY the same token; `tests/all/minted_sign_in.rs:981-1059` spends the refresh token WITHOUT any preceding logout and asserts the rotated token is served (`:1009-1012`). No test logs out and then refreshes, which is why the resurrection fails no assertion.

## Steps to Reproduce

1. Stand up the four-service emulator the way tests/all/minted_sign_in.rs:101-130 does: the portal, sso-oidc, sso-admin (IAM Identity Center) and Organizations on one shared `engine::InMemoryEngine` behind a `MultiServiceRouter`, with at least one completed account assignment so a sign-in has an account to report.
2. Walk the device flow as `Emulator::minted_sign_in` does (RegisterClient with the `refresh_token` grant → StartDeviceAuthorization → approve the user code → CreateToken with `grantType=urn:ietf:params:oauth:grant-type:device_code`), keeping both `accessToken` A and `refreshToken` R.
3. `GET /assignment/accounts` with `x-amz-sso_bearer_token: A` → 200 with the assigned accounts (the sign-in works).
4. `POST /logout` with `x-amz-sso_bearer_token: A` → 200. Re-present A on all four routes → 401 UnauthorizedException "Session token not found or invalid" (as tests/all/minted_sign_in.rs:938-969 pins).
5. Call the OIDC service: `POST /token` with `{clientId, clientSecret, grantType: "refresh_token", refreshToken: R}` → 200 with a new `accessToken` A2 (services/service-com-amazonaws-ssooidc-awsssooidcservice/src/state.rs:772-774 mints it for the same `clientId`).
6. `GET /assignment/accounts` with `x-amz-sso_bearer_token: A2`. Observed: 200 with the very same account list — the sign-in the previous step's `Logout` reported it had invalidated is serving again. Expected per the Logout documentation quoted above: the server-side sign-in session was invalidated, and only the role sessions that already existed continue.
7. `GET /federation/credentials?account_id=<assigned>&role_name=<assigned>` with A2. Observed: 200 with a FRESH accessKeyId/secretAccessKey/sessionToken — a NEW role session created after the logout, which the model's carve-out ("any existing IAM role sessions … continue") does not cover.
8. Inspect the portal store: `sessions` holds a record only for A (`active: false`); nothing exists for A2, and the OIDC store's `access_tokens` holds A2 as a live token — the logout was never propagated to the mint that issued the sign-in's tokens.
