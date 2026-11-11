Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# A bandwidth change on a connection still awaiting acceptance settles it into the accepted state `pending`, not back into the state it came from

_Filed: 2026-10-29T09:00:20Z_
_Operation: UpdateConnection | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateConnection` is permitted from `requested` as well as from `available` (`UPDATABLE_CONNECTION_STATES`). When a bandwidth change is applied, the handler moves the record to `updating` and arms a transition back to the value `settled_state()` returned — and `settled_state()` returns the state the connection's PENDING transition is owed, not the state the connection is in. A connection in `requested` always carries the owed activation leg `requested -> pending` (armed by every create), so `settled` is `"pending"`, and the update's settle target is the NEXT leg of the acceptance progression rather than the state the record came from. The comment directly above the write states the opposite rule ("the clock settles it back into the state it came from"), so the code contradicts the rule written over it: the connection never returns to `requested`.

The consequence is not cosmetic, because this crate derives "has the proposal been accepted?" from the `ConnectionState` alone. `is_accepted_state` classifies both `updating` and `pending` as states "a connection whose proposal HAS been accepted" reports, while `AWAITING_ACCEPTANCE_CONNECTION_STATE` (`requested`) is documented as "the one state an acceptance can be made from". So the instant a bandwidth change is applied to a connection whose proposal nobody has accepted, `AcceptConnectionProposal` on that connection's `activationKey` stops being possible: it answers HTTP 400 `InterconnectClientException` "The connection proposal for the supplied activationKey has already been accepted as connection <id>, whose attach point cannot be changed" (or, when the request happens to ask for exactly what the record already holds, a 200 claiming the acceptance was already made). Nothing accepted anything — an unrelated bandwidth edit did.

This is NOT the open issue 20261028T035322Z ("AcceptConnectionProposal races the clock"), which is about the clock-owed `requested -> pending` leg firing on its own after ACTIVATION_SPAN, and it is not the fixed issue "A retried UpdateConnection bandwidth change is refused with 400" (idempotent retry handling). This is a SECOND, independent writer of the same `requested -> pending` advance that lives in `UpdateConnection`: it fires immediately, inside the activation span, with no clock involvement, and a fix to the clock path would leave it in place. A fix here only has to pass the state the record is actually in as the settle target (the clock then re-arms whatever that state owes via `owed_progression`), which leaves every existing assertion intact — the suite only asserts the settled state is live and no longer `updating`.

## Evidence

The settle target the update arms is the OWED state, not the current one — services/service-com-amazonaws-interconnect-interconnect/src/state.rs:408-415:
```
    /// The state a connection settles into once its pending transition fires —
    /// its current state when nothing is pending.
    pub fn settled_state(stored: &StoredConnection) -> String {
        stored
            .pending_transition
            .as_ref()
            .map_or_else(|| stored.connection.state.clone(), |p| p.state.clone())
    }
```
read by the update — src/ops/update_connection.rs:212-217:
```
        // The state the connection settles into: the one a pending transition is
        // already owed, or its current state. A bandwidth change below moves it to
        // `updating` and hands it back to the clock, so re-reading the settled state
        // (rather than the reported one) keeps a second change from pinning it in
        // `updating` for good.
        let settled = crate::state::ComAmazonawsInterconnectInterconnectState::settled_state(stored);
```
and armed as the state the connection comes back to, under a comment stating the rule the code breaks — src/ops/update_connection.rs:252-263:
```
        if let Some(bandwidth) = input.bandwidth.clone() {
            if bandwidth != stored.connection.bandwidth {
                stored.connection.bandwidth = bandwidth.clone();
                // "updating: the connection is being updated" — re-provisioning the
                // circuit is not instantaneous, so the connection reports `updating`
                // and the clock settles it back into the state it came from.
                stored.connection.state = crate::state::UPDATING_CONNECTION_STATE.to_string();
                stored.pending_transition = Some(crate::state::PendingTransition {
                    due: now + crate::state::PROVISIONING_SPAN,
                    state: settled,
                });
```
A `requested` connection always carries the owed leg that makes `settled` == "pending" — src/ops/create_connection.rs:475 and :518:
```
            state: "requested".to_string(),
...
                pending_transition: crate::state::owed_progression(&connection.state, now),
```
with src/state.rs:137-147:
```
pub fn owed_progression(state: &str, entered: Instant) -> Option<PendingTransition> {
    let next = match state {
        "requested" => "pending",
```
and the update is permitted from that very state — src/state.rs:1390-1395:
```
pub const UPDATABLE_CONNECTION_STATES: [&str; 2] = ["requested", "available"];

/// Whether a connection in `state` may be updated at all.
pub fn is_updatable_state(state: &str) -> bool {
```
The other side of the inconsistency — acceptance is derived from the state alone. src/state.rs:1296-1302:
```
/// The `ConnectionState` a connection whose proposal has not been accepted yet
/// reports ("requested: The initial state of a connection. The state will remain here
/// until the Connection is accepted on the Partner portal").
///
/// It is the one state an acceptance can be made from: before it there is no
/// connection to accept, and after it the acceptance has already happened.
pub const AWAITING_ACCEPTANCE_CONNECTION_STATE: &str = "requested";
```
src/state.rs:1314-1328:
```
/// The `ConnectionState` values a connection whose proposal HAS been accepted, and
/// whose circuit has not since been torn down or failed, reports: ...
pub const ACCEPTED_CONNECTION_STATES: [&str; 4] = [
    ACCEPTED_CONNECTION_STATE,
    "available",
    "down",
    UPDATING_CONNECTION_STATE,
];

/// Whether `state` is one a connection reports once its proposal has been accepted.
pub fn is_accepted_state(state: &str) -> bool {
    ACCEPTED_CONNECTION_STATES.contains(&state)
}
```
and the refusal that derivation produces — src/ops/accept_connection_proposal.rs:244-262:
```
    if connection.state != crate::state::AWAITING_ACCEPTANCE_CONNECTION_STATE {
        if crate::state::is_accepted_state(&connection.state) {
            if crate::state::acceptance_already_applied(...) {
                return Ok(StoreVerdict::AlreadyAccepted(Box::new(connection.clone())));
            }
            return Err(
                AcceptConnectionProposalErrorModeled::InterconnectClientException(
                    InterconnectClientException {
                        message: crate::state::proposal_already_accepted_message(&connection.id),
```
worded by src/state.rs:1362-1367:
```
pub fn proposal_already_accepted_message(connection_id: &str) -> String {
    format!(
        "{REFUSAL_ENVELOPE}The connection proposal for the supplied activationKey has already \
         been accepted as connection {connection_id}, whose attach point cannot be changed"
```
Test coverage: the `requested`-state bandwidth change IS exercised (tests/all/update_connection_idempotent_retry.rs:199-273 creates a connection and updates its bandwidth as the next call) but the settle TARGET is left open — the only assertions are `assert_ne!(settled_state, "updating")` (line 263-266) and `LIVE_STATES.contains(&state)` (line 239-242). tests/all/connection_state_progression.rs:287-292 pins the settle target only for the `available` case ("RULE: ... a bandwidth change runs it through `updating` back to `available` — not back to the state it was created in"). No test pins what an update does to a proposal still awaiting acceptance, and no test issues an AcceptConnectionProposal after an update.

## Steps to Reproduce

1. Boot the emulator and create a ready Direct Connect gateway G1 and a second one G2, so two distinct attach points exist.
2. Call ListEnvironments and pick an environment that offers two available bandwidths (e.g. env-csp-azure-us-east-1 with 1Gbps and 10Gbps).
3. Call CreateConnection with attachPoint={"directConnectGateway": G1} and bandwidth=1Gbps. Note connection.state == "requested" and connection.activationKey.
4. IMMEDIATELY (well inside the 250 ms ACTIVATION_SPAN, so the clock has advanced nothing) call UpdateConnection with that identifier and bandwidth=10Gbps. Observed: connection.state == "updating".
5. IMMEDIATELY call AcceptConnectionProposal with that activationKey and attachPoint={"directConnectGateway": G2}. Observed: HTTP 400 InterconnectClientException "Unable to process request: The connection proposal for the supplied activationKey has already been accepted as connection <id>, whose attach point cannot be changed" — although no acceptance was ever made and the clock has not fired.
6. Repeat steps 3 and 5 WITHOUT the intervening update: the acceptance is answered 200 and the attach point is applied. The bandwidth change alone is what ended the acceptance window.
7. Sleep 700 ms and call GetConnection: the record reports "pending"/"available" — the update's settle target was the next leg of the acceptance progression (src/state.rs:137-147), so the connection never returned to the "requested" state it came from, contrary to src/ops/update_connection.rs:256-257.
