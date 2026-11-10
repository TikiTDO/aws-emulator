Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 14df30a096a54aecb31fd12ed18f88f92038b3f7

# Accepting a proposal produces a connection at the record's current bandwidth, not the bandwidth the activation key captured, so an UpdateConnection before the acceptance modifies the proposal

_Filed: 2026-11-10T21:46:26Z_
_Operation: AcceptConnectionProposal | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `14df30a096a54aecb31fd12ed18f88f92038b3f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate holds a proposal's parameters in a dedicated, write-once capture (`StoredConnection::proposal` / `CapturedProposal`) precisely because the model says the proposal's Environment and bandwidth "cannot be modified", and `DescribeConnectionProposal` reads that capture rather than the live record. `AcceptConnectionProposal`'s own handler doc restates the same rule for the acceptance: the Environment and the bandwidth "are read from the proposal".

But the one writer of an acceptance, `crate::state::apply_acceptance`, never reads `stored.proposal` and never touches `stored.connection.bandwidth` — it writes only the attach point, the description, the tags, `proposal_accepted` and the state. And nothing stops the record's bandwidth from having moved in the meantime: `UpdateConnection` permits a bandwidth change from `requested` and `available` (`UPDATABLE_CONNECTION_STATES`) and never consults `proposal_accepted`, so a connection whose proposal nobody has accepted yet can be re-banded freely.

The result is that the two halves of the proposal contract disagree. For a connection created at 1Gbps and then updated to 2Gbps before anyone accepts it:

* `DescribeConnectionProposal(activationKey)` reports `bandwidth: "1Gbps"` — the capture, which the crate protects on purpose;
* `AcceptConnectionProposal(activationKey, attachPoint)` accepts THAT proposal and answers with a `Connection` whose `bandwidth` is `"2Gbps"`, and the record keeps that value for the rest of its life.

So the bandwidth the proposal offers was in fact modified between the describe and the acceptance, which is the exact thing the model sentence and this crate's own capture machinery exist to prevent. `grep` confirms `stored.proposal` has exactly one reader in the crate (`describe_connection_proposal.rs:199`) — the acceptance path never consults it — and the acceptance also never re-checks the record's bandwidth against the Environment's currently available set, unlike both of the crate's actual bandwidth writers (`bandwidth_availability_violation`, applied by create and update).

## Evidence

The Smithy sentence the whole capture exists for — smithy_query(query="doc",shape="AcceptConnectionProposal"):
"Accepts a connection proposal which was generated at a supported partner's portal. The proposal contains the Environment and bandwidth that were chosen on the partner's portal and cannot be modified."

The acceptance handler's own doc, claiming the bandwidth is read from the proposal — services/service-com-amazonaws-interconnect-interconnect/src/ops/accept_connection_proposal.rs:406-411:
```
    /// which the clock owes it. The Environment and the bandwidth were chosen on the
    /// partner's portal and "cannot be modified", so they are read from the proposal
    /// and nothing the request carries can change them; the optional `description` and
    /// `tags` are the caller's to set on the resulting Connection.
```

The one writer of an acceptance, which never reads the capture and never writes a bandwidth — src/state.rs:1665-1683:
```
pub fn apply_acceptance(
    stored: &mut StoredConnection,
    attach_point: &AttachPoint,
    description: Option<&String>,
    tags: Option<&std::collections::HashMap<String, String>>,
    now: Instant,
) {
    stored.connection.attach_point = attach_point.clone();
    if let Some(description) = description {
        stored.connection.description = description.clone();
    }
    ...
    stored.proposal_accepted = true;
    stored.connection.state = ACCEPTED_CONNECTION_STATE.to_string();
    stored.pending_transition = owed_progression(ACCEPTED_CONNECTION_STATE, now);
}
```

The crate's own statement that the record's bandwidth and the proposal's are different things, and that the record's is reassigned by UpdateConnection — src/state.rs:159-170:
```
/// Kept beside the record rather than read back off it, because the two are
/// different things once the connection has been modified: `Connection$bandwidth` is
/// "the specific selected bandwidth of this connection", which `UpdateConnection`
/// ("we support modifications to the connection's description and/or bandwidth")
/// reassigns, while the proposal's is "the bandwidth of the proposed Connection" —
/// the one the key "captures … from the initial creation request", chosen before
/// there was a connection to modify.
```

The update path that re-bands an unaccepted connection — src/state.rs:1745 and src/ops/update_connection.rs:257-259 (no `proposal_accepted` guard anywhere in that file; `grep proposal_accepted` matches only state.rs, create_connection.rs:578 and accept_connection_proposal.rs:328/337):
```
pub const UPDATABLE_CONNECTION_STATES: [&str; 2] = ["requested", "available"];
```
```
        if let Some(bandwidth) = input.bandwidth.clone() {
            if bandwidth != stored.connection.bandwidth {
                stored.connection.bandwidth = bandwidth.clone();
```

The acceptance is reachable from the state an update leaves — src/state.rs:1642-1644:
```
pub fn acceptance_possible_in(state: &str) -> bool {
    !is_deleted_or_deleting(state) && state != FAILED_CONNECTION_STATE
}
```

Test coverage: the capture suite only ever updates AFTER the acceptance, so this ordering is unpinned — tests/all/proposal_parameters_are_captured.rs:436-458 ("The acceptance, as the very next call after the create" … then "Then the connection the acceptance made is updated", asserting at :449-454 `"expected the acceptance to leave the proposal's bandwidth"`), and the update-before-accept row at tests/all/proposal_parameters_are_captured.rs:246-320 never calls AcceptConnectionProposal at all. No test accepts a proposal whose connection was re-banded first.

## Steps to Reproduce

1. Boot the emulator and point an Interconnect client at it; create a ready Direct Connect gateway so there is a valid attach point.
2. ListEnvironments and pick an Environment offering at least two bandwidths (the seeded catalog offers 500Mbps/1Gbps/2Gbps/...).
3. CreateConnection(environmentId=<that id>, bandwidth="1Gbps", attachPoint={directConnectGateway:<gateway>}, remoteAccount=...). Note `activationKey` and `id`. Do NOT accept yet.
4. DescribeConnectionProposal(activationKey=<key>) and observe bandwidth "1Gbps" — the proposal's captured parameters.
5. Poll GetConnection until the connection reports `available` (the clock carries it there without any acceptance), then UpdateConnection(identifier=<id>, bandwidth="2Gbps"). Observe HTTP 200 with bandwidth "2Gbps" — an unaccepted proposal's connection was re-banded.
6. DescribeConnectionProposal(activationKey=<key>) again: still bandwidth "1Gbps", the capture the crate protects.
7. AcceptConnectionProposal(activationKey=<key>, attachPoint={directConnectGateway:<gateway>}). Observe HTTP 200 whose `connection.bandwidth` is "2Gbps", not the "1Gbps" the proposal just reported — the acceptance produced a connection at a bandwidth the proposal does not offer, even though the handler doc says the bandwidth is "read from the proposal" and the model says the proposal's bandwidth "cannot be modified".
8. GetConnection(identifier=<id>) confirms the accepted connection keeps "2Gbps" permanently while its activation key keeps describing a "1Gbps" proposal.
