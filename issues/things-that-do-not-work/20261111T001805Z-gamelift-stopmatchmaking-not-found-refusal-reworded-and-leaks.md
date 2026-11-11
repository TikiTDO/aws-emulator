Issue-Source: trace-sweep-divergence
Issue-Trace: gamelift-matchmaking-configuration-and-ticket-error-paths
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gamelift-matchmaking-configuration-and-ticket-error-paths
Issue-Shape-Id: com.amazonaws.gamelift#GameLift
Issue-Key: gamelift|StopMatchmaking|4xx->4xx|$.Message
Issue-Commit: 971bdc7c017da8bc0b64d07fdab7095383b15275

# GameLift StopMatchmaking not-found refusal reworded and leaks the requested ticket id

> _Note: this issue was identified against the code base at commit `971bdc7c017da8bc0b64d07fdab7095383b15275`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopMatchmaking

## Description
One cause.

Cause 1 of 1: StopMatchmaking called with a TicketId that no stored matchmaking ticket carries is refused with the correct HTTP status and the correct modeled error shape, but with an invented Message that is not AWS's text and that interpolates a value AWS's message does not contain.

AWS: StopMatchmaking refuses an unknown ticket id with HTTP 400 and the modeled NotFoundException, whose Message is exactly "Specified matchmaking ticket does not exist." The ENTIRE string is fixed text for this error class — there is no variable part: AWS does not echo the TicketId from the request, nor any name, id or count from service state, and the sentence ends in a period. Smithy (services/service-com-amazonaws-gamelift-gamelift/smithy-model/com-amazonaws-gamelift-gamelift.json): StopMatchmaking input member TicketId -> MatchmakingIdStringModel, required, @length 0..128, @pattern ^[a-zA-Z0-9-\.]*$; modeled errors InternalServiceException (server, httpError 500), InvalidRequestException, NotFoundException and UnsupportedRegionException (all client, httpError 400 defaulted). NotFoundException carries a single Message member (NonEmptyString, @length min 1) and is documented "The requested resource was not found. The resource was either not created yet or deleted." The exact Message text for this path is recorded behavior (the fixture's recorded 400 response), since the model does not carry per-operation message templates.

Emulator: answers HTTP 400 with NotFoundException (correct status and correct error shape) but Message "Matchmaking ticket not found: <ticketId>" — different fixed text ("Matchmaking ticket not found: "), no trailing period, and an extra variable part, the TicketId copied from the request, which AWS's template does not have at all.

Why: a client or operator that matches, asserts on, or surfaces AWS's not-found text for a cancelled/expired/unknown matchmaking ticket never sees it — the emulator's sentence shares no substring with AWS's. Because the emulator's text is parameterised by the request's ticket id, it is also not even a stable constant across runs, so log-based and message-based handling that works against AWS silently fails here. The status (400) and the error code (NotFoundException) are already right, so code-based branching is unaffected; only the message is wrong.

Fix: the StopMatchmaking handler's ticket-resolution failure path (services/service-com-amazonaws-gamelift-gamelift/src/ops/stop_matchmaking.rs, the `state.matchmaking_tickets.get_mut` else-branch at lines 170-179) and the shared message helper it calls, crate::state::matchmaking_ticket_not_found_message in services/service-com-amazonaws-gamelift-gamelift/src/state.rs. The helper must emit the constant AWS sentence for this operation with no interpolation; if other GameLift ticket operations word the same missing resource differently (as GameLift does elsewhere — see open issue 20261103T083127Z-gamelift-refusal-fidelity-3-error-class-message-divergences.md, where DescribeMatchmakingConfigurations and UpdateMatchmakingConfiguration need different not-found texts), the helper must be split per operation rather than shared.

Related but NOT covering this: open issues 20261103T083127Z-gamelift-refusal-fidelity-3-error-class-message-divergences.md (UpdateGameSessionQueue, DeleteMatchmakingRuleSet, UpdateMatchmakingConfiguration), 20261103T145425Z-gamelift-wire-fidelity-listlocations-serves-one-entry-catalog.md (ListLocations, DeleteMatchmakingRuleSet) and 20261104T000529Z-gamelift-not-found-refusals-reword-the-aws-notfoundexception.md (ListTagsForResource) each reword other GameLift not-found paths; none of them names StopMatchmaking or this ticket-not-found helper, and the replay reports this cluster as unowned.

Traces: gamelift-matchmaking-configuration-and-ticket-error-paths#task#3.

## Evidence
Replay of fixture gamelift-matchmaking-configuration-and-ticket-error-paths, step task#3 (operation StopMatchmaking, verdict mismatch, expected_status 400, actual_status 400, affected_count 1):

  $.Message: value_mismatch
    expected 'Specified matchmaking ticket does not exist.'
    got      'Matchmaking ticket not found: sim2real-probe-gamelift-missing-ticket-ebd77c6e'

Status and error shape matched (400 / NotFoundException); only the Message member diverged. In the expected value the whole sentence is this error class's fixed text. In the emulator's value, "Matchmaking ticket not found: " is the emulator's invented fixed text and the trailing 'sim2real-probe-gamelift-missing-ticket-ebd77c6e' is a variable part taken from the request's TicketId member — it is one run's probe name, not part of any requirement. Emulator source: services/service-com-amazonaws-gamelift-gamelift/src/ops/stop_matchmaking.rs:170-179 returns NotFoundException with crate::state::matchmaking_ticket_not_found_message(&input.ticket_id).

## Steps to Reproduce
1. Boot the emulator and point a GameLift client at it.
2. Call StopMatchmaking with a TicketId that matches ^[a-zA-Z0-9-\.]*$ and that no stored matchmaking ticket carries (never created, or already deleted) — e.g. TicketId="no-such-ticket".
3. Read the HTTP status, the error code and the Message member: status 400 and code NotFoundException are correct; the Message must be exactly "Specified matchmaking ticket does not exist." with no part of the request interpolated.
4. Observed instead: Message "Matchmaking ticket not found: no-such-ticket".
5. Re-replay fixture gamelift-matchmaking-configuration-and-ticket-error-paths step task#3 to confirm the fix.
