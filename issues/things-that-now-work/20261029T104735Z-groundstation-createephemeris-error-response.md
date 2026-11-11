Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 6270af2faafa317884be9e4627608e6b01112bc2

# A full ephemeris store answers DependencyException (531) — the exact answer the sibling create's own comment says misdescribes that condition

_Filed: 2026-10-29T10:47:35Z_
_Operation: CreateEphemeris | Class: error-response_

> _Note: this issue was identified against the code base at commit `6270af2faafa317884be9e4627608e6b01112bc2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Six creates in this crate guard the same emulator-internal store bound (`crate::state::MAX_RECORDS_PER_STORE`) and answer the identical condition in two contradictory ways, each under a comment asserting its way is the correct one.

`RegisterAgent` models exactly three errors — `DependencyException`, `InvalidParameterException`, `ResourceNotFoundException` — and DELIBERATELY widens its error enum with `extra_errors = ["ResourceLimitExceededException"]`, on the stated ground that answering through any of those three "would misdescribe it — the request is valid, it names resources that exist and nothing downstream failed".

`CreateEphemeris` models the SAME three errors and does the opposite: it answers `DependencyException` (HTTP 531, a SERVER error whose own model doc is "Dependency encountered an error"), on the stated ground that "its client errors are `InvalidParameterException` and `ResourceNotFoundException`, neither of which describes a full store" — a reasoning that simply omits the option its sibling took, and lands on precisely the answer its sibling says is wrong.

So a caller that fills the ephemeris store is told this service's dependency failed (a retryable 5xx-class fault that did not happen), where the same caller filling the config, mission-profile, dataflow-endpoint-group or agent store is told a client-side account limit was reached (429), and the V2 group create answers `ServiceQuotaExceededException` (402). One of the two readings has to go; the crate cannot hold both.

## Evidence

Both sides, in this crate, for the identical error set and the identical condition:

`services/service-com-amazonaws-groundstation-groundstation/src/ops/register_agent.rs:80-92`:
"// `ResourceLimitExceededException` is widened in with `extra_errors`, DELIBERATELY off
// this operation's own modelled error set, for the same reason `CreateConfig` and
// `CreateMissionProfile` answer with it: the SERVICE models the shape and it is what this
// service answers when a create runs past an account limit, which is how the bounded
// store below refuses one. Reporting an emulator store bound through one of this
// operation's three declared errors would misdescribe it — the request is valid, it names
// resources that exist and nothing downstream failed (see `state::MAX_RECORDS_PER_STORE`,
// reachable only after 1,000 registrations)."
and `register_agent.rs:210-221`: `if guard.agents.len() >= crate::state::MAX_RECORDS_PER_STORE { return Err(RegisterAgentErrorModeled::ResourceLimitExceededException(...)) }`

`services/service-com-amazonaws-groundstation-groundstation/src/ops/create_ephemeris.rs:213-229`:
"// The store this emulator keeps is bounded, so a caller cannot grow it without
// limit. This operation models no account-limit error (its client errors are
// `InvalidParameterException` and `ResourceNotFoundException`, neither of which
// describes a full store), so the bound is reported as what it is: a failure of
// the store this service depends on, not of the request."
and the body: `if guard.ephemerides.len() >= crate::state::MAX_RECORDS_PER_STORE { return Err(CreateEphemerisErrorModeled::DependencyException(DependencyException { message: Some(format!("this account already holds the maximum of {} ephemerides", crate::state::MAX_RECORDS_PER_STORE)), parameter_name: None }).into()); }`

The two operations have the SAME modelled error set — `smithy_query(query="operation", operation="RegisterAgent,CreateEphemeris")` gives each exactly `DependencyException` (531), `InvalidParameterException` (431), `ResourceNotFoundException` (434) — so the "this operation models no account-limit error" premise is equally true of `RegisterAgent`, which nonetheless widens rather than answering 531.

The crate's own statement of what the condition IS, at `src/state.rs:1190-1196`: "NOT an AWS quota … It is a bound on the shared in-process store, so a caller cannot grow the emulator's memory without limit; a create that would pass it is refused with the service's modelled `ResourceLimitExceededException` (HTTP 429, \"Account limits for this resource have been exceeded\"), which is what the service answers when a create runs past an account limit." — the constant's own doc names the answer, and `CreateEphemeris` is the only create that does not give it.

The other four creates agree with the constant's doc: `create_config.rs:233-244` and `create_mission_profile.rs:244-257` answer `ResourceLimitExceededException`; `create_dataflow_endpoint_group.rs:281-294` answers `ResourceLimitExceededException`; `create_dataflow_endpoint_group_v2.rs:216-230` answers the `ServiceQuotaExceededException` that operation models.

Test coverage: only `CreateConfig`'s bound is exercised — `tests/all/scheduling_loop.rs:1878-1918`: "// A store at its bound refuses the next create with the service's own // account-limit error rather than growing without end." asserting `status == 429` and `error_code() == "ResourceLimitExceededException"`. No test pins `CreateEphemeris`'s 531, so nothing in the crate justifies the divergence beyond the comment its sibling explicitly rejects.

## Steps to Reproduce

1. Start the emulator.
2. POST /ephemeris 1,000 times with a valid body (name + ephemeris with inline tle data), filling the ephemeris store to crate::state::MAX_RECORDS_PER_STORE.
3. POST /ephemeris once more: the reply is HTTP 531 with __type DependencyException and message "this account already holds the maximum of 1000 ephemerides" — a server/dependency fault for a request that is valid and a dependency that did not fail.
4. POST /agent 1,000 times to fill the agent store, then once more: the reply is HTTP 429 ResourceLimitExceededException, although RegisterAgent declares the identical three errors as CreateEphemeris.
5. Do the same against /config (tests/all/scheduling_loop.rs:1878-1918 already does): HTTP 429 ResourceLimitExceededException.
