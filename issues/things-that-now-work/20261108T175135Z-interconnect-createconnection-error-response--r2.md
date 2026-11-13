Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: cf75889d0041250f9d4c4a6a69f11963f546e0f7

# Four client-facing refusals are worded without the service refusal envelope the crate says every rejection carries

_Filed: 2026-11-08T17:51:35Z_
_Operation: CreateConnection | Class: error-response_

> _Note: this issue was identified against the code base at commit `cf75889d0041250f9d4c4a6a69f11963f546e0f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate words its own client-facing refusals through one constant, `crate::state::REFUSAL_ENVELOPE` ("Unable to process request: "), whose doc states the rule categorically: it is "The envelope this service puts on the client-facing refusals it words itself" and "Every recorded Interconnect rejection message starts with it ... so the refusals below are built from this one constant rather than each spelling the prefix out".

Every refusal builder in `state.rs` honours it — `environment_not_found_message`, `connection_not_retrievable_message`, `arn_resource_absent_message`, `proposal_not_retrievable_message`, `activation_key_not_base64_message`, `connection_absent_message`, `not_a_connection_arn_message`, `not_a_gateway_identifier_message`, `attach_point_not_valid_message`, `provider_violation`, `remote_identifier_violation`, `remote_account_violation`, `reserved_tag_key_violation`, `tag_length_violation`, `tag_write_violation`, `proposal_already_accepted_message`, `proposal_not_acceptable_message`, `connection_not_updatable_message` — and the ONE deliberate exception says so in its own doc: `arn_scope_violation` records that "It carries neither this service's [`REFUSAL_ENVELOPE`] nor the ARN text".

Four client-fault refusals (all HTTP 400, all worded by this service itself) silently skip the envelope and give no reason for doing so:

1. `crate::state::bandwidth_availability_violation` — the shared rule both bandwidth writers use, reached from `CreateConnection` (`create_connection.rs:315-324`) and `UpdateConnection` (`update_connection.rs:230-241`);
2. the environment-unavailable refusal built inline in `CreateConnection` (`create_connection.rs:299-308`);
3. the `clientToken`-reused-with-different-parameters refusal built inline in `CreateConnection` (`create_connection.rs:260-268`);
4. the connection-quota refusal built inline in `CreateConnection` (`create_connection.rs:472-481`).

So one and the same operation answers two of its refusals with the envelope (an unresolvable `environmentId`, a malformed `attachPoint`) and three without it, and the crate's own constant asserts that cannot happen. A client that strips or matches on the documented prefix sees two message shapes from one operation.

## Evidence

The rule, stated by the constant itself — services/service-com-amazonaws-interconnect-interconnect/src/state.rs:1106-1112:
```
/// The envelope this service puts on the client-facing refusals it words itself.
///
/// Every recorded Interconnect rejection message starts with it ("Unable to process
/// request: This environment model with id … could not be found", "Unable to process
/// request: Connection does not exist for …"), so the refusals below are built from
/// this one constant rather than each spelling the prefix out.
pub const REFUSAL_ENVELOPE: &str = "Unable to process request: ";
```
The one exception DOCUMENTS itself as one — src/state.rs:1325-1328:
```
/// request carried the offending ARN. It carries neither this service's
/// [`REFUSAL_ENVELOPE`] nor the ARN text, so a refusal reproduces nothing of the ARN
/// the caller supplied.
```
The four sites that skip it without a word. The shared bandwidth rule — src/state.rs:1023-1037:
```
pub fn bandwidth_availability_violation(
    environment: Option<&Environment>,
    environment_id: &str,
    bandwidth: &str,
) -> Option<String> {
    let available = environment
        .and_then(|environment| environment.bandwidths.available.as_deref())
        .unwrap_or_default();
    (!available.iter().any(|offered| offered == bandwidth)).then(|| {
        format!(
            "bandwidth '{bandwidth}' is not available on environment '{environment_id}'; available bandwidths are [{}]",
            available.join(", ")
        )
    })
}
```
The environment-unavailable refusal — src/ops/create_connection.rs:298-308:
```
    if environment.state == crate::state::UNAVAILABLE_ENVIRONMENT_STATE {
        return Err(CreateConnectionErrorModeled::InterconnectClientException(
            InterconnectClientException {
                message: format!(
                    "environment '{}' is {} and cannot take new connections",
                    environment.environment_id, environment.state
                ),
            },
        )
        .into());
    }
```
The clientToken refusal — src/ops/create_connection.rs:259-268:
```
            if !created.matches(input) {
                return Err(CreateConnectionErrorModeled::InterconnectValidationException(
                    InterconnectValidationException {
                        message: format!(
                            "clientToken '{token}' was already used to create connection '{}' with different parameters",
                            created.connection_id
                        ),
                    },
                )
                .into());
            }
```
The quota refusal — src/ops/create_connection.rs:471-481:
```
        if live_connections >= crate::state::MAX_CONNECTIONS {
            return Err(CreateConnectionErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(format!(
                        "the account already holds the maximum of {} connections",
                        crate::state::MAX_CONNECTIONS
                    )),
                },
            )
            .into());
        }
```
versus the same handler's envelope-carrying refusal for the sibling failure mode — src/ops/create_connection.rs:284-292 via `crate::state::environment_not_found_message`, src/state.rs:1121-1123:
```
pub fn environment_not_found_message(environment_id: &str) -> String {
    format!("{REFUSAL_ENVELOPE}This environment model with id {environment_id} could not be found")
}
```
Test coverage: tests/all/bandwidth_availability.rs:38-44 reproduces the bandwidth sentence verbatim (`fn not_available`) and tests/all/request_faults_precede_resource_verdicts.rs:701 the clientToken sentence; neither states a reason for the missing prefix, while four other suites import or restate the envelope as the service's refusal form (tests/all/provider_member_constraints.rs:41, tests/all/arn_scope_mismatch_wording.rs:53, tests/all/refusals_and_gateway_attach_points.rs:20-22, tests/all/update_connection_idempotent_retry.rs:27). No test pins the absence of the envelope on these four messages as intended.

## Steps to Reproduce

1. Boot the emulator and point an Interconnect client at it.
2. Call ListEnvironments to read the called Region's environmentId and its bandwidths.available list.
3. Call CreateConnection with that environmentId and a well-formed bandwidth the environment does not offer (e.g. "40Gbps"). Observe InterconnectValidationException with the message "bandwidth '40Gbps' is not available on environment 'mce-aws-gcp-iad'; available bandwidths are [...]" — no "Unable to process request: " prefix.
4. Call CreateConnection again with an environmentId no catalog holds (e.g. "mce-aws-nobody-xxx"). Observe InterconnectClientException with "Unable to process request: This environment model with id mce-aws-nobody-xxx could not be found" — the prefix is there.
5. Call CreateConnection twice with the same clientToken but a different description. Observe "clientToken '...' was already used to create connection '...' with different parameters" — again no prefix.
6. Compare the three messages against src/state.rs:1106-1112, which says every rejection this service words starts with the envelope, and against src/state.rs:1325-1328, the only site that documents omitting it.
