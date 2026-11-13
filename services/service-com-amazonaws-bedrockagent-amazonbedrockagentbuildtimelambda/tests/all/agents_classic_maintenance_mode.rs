// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT condition on minting a new agent, end to end over HTTP.
//!
//! THE RULE UNDER TEST: this plane's agent surface is no longer open to new customers, so
//! a `CreateAgent` from an account with no prior usage of that surface is refused with the
//! operation's modelled `AccessDeniedException` (403) stating the maintenance-mode
//! condition and writes nothing, while every other operation of the plane — the agent read
//! and list plane, and the other resource families it holds — is served whatever the
//! account's history is. The condition belongs to the ACCOUNT and not to the request:
//! recording the account's prior usage serves the very request that was refused.
//!
//! The tables below span what that rule generalises over: which optional members a create
//! carries (none, a role and a model, an idempotency token, all of them), the published
//! bounds of `idleSessionTTLInSeconds`, and which operation of the plane the ineligible
//! account calls.

use serde_json::json;
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::agents_classic::MAINTENANCE_MODE_MESSAGE;

use crate::agent_lifecycle::{instruction, Emulator, ACCOUNT};

/// An `agentResourceRoleArn` matching the member's modelled pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const AGENT_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForAgents_probe";

/// A `clientToken` inside the member's modelled `length 33..256` and pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`.
const CLIENT_TOKEN: &str = "maintenance-mode-probe-token-00001";

/// A `flowExecutionRoleArn` matching the modelled `FlowExecutionRoleArn` pattern
/// `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/(service-role/)?.+$`, for the control that
/// another create family of this plane is untouched.
const FLOW_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForFlows";

/// A well-formed `agentId` (`^[0-9a-zA-Z]{10}$`) this plane never minted.
const UNKNOWN_AGENT: &str = "ABCDEFGHIJ";

/// The `CreateAgent` bodies the rule is asserted over, each named by what it varies and
/// carrying the `agentName` it is given: the modelled minimum, the request a client
/// building an agent sends, one carrying an idempotency token, one carrying every optional
/// member a create takes, and each published bound of `idleSessionTTLInSeconds`
/// (`range 60..5400`).
fn create_requests(suffix: &str) -> Vec<(&'static str, serde_json::Value)> {
    vec![
        (
            "the modelled minimum: an agentName alone",
            json!({ "agentName": format!("minimal-{suffix}") }),
        ),
        (
            "a role and a foundation model",
            json!({
                "agentName": format!("modelled-{suffix}"),
                "agentResourceRoleArn": AGENT_ROLE,
                "foundationModel": "amazon.titan-text-express-v1",
            }),
        ),
        (
            "an idempotency token, which a refusal must not consume",
            json!({
                "agentName": format!("tokened-{suffix}"),
                "clientToken": CLIENT_TOKEN,
            }),
        ),
        (
            "every optional member a create takes",
            json!({
                "agentName": format!("furnished-{suffix}"),
                "description": "an agent a new customer tries to create",
                "instruction": instruction("booking"),
                "foundationModel": "anthropic.claude-v2",
                "idleSessionTTLInSeconds": 60,
                "agentCollaboration": "DISABLED",
                "orchestrationType": "DEFAULT",
                "tags": { "probe": "maintenance-mode" },
            }),
        ),
        (
            "the upper published bound of idleSessionTTLInSeconds",
            json!({
                "agentName": format!("long-lived-{suffix}"),
                "idleSessionTTLInSeconds": 5400,
            }),
        ),
    ]
}

/// Assert `answer` is the maintenance-mode refusal: the modelled status and error shape,
/// and the service's own sentence for the condition.
fn assert_refused(case: &str, answer: &crate::agent_lifecycle::Answer) {
    assert_eq!(
        answer.status, 403,
        "case {case}: expected CreateAgent -> 403 for an account with no prior usage, got \
         {answer:?}"
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some("AccessDeniedException"),
        "case {case}: expected AccessDeniedException, got {answer:?}"
    );
    assert_eq!(
        answer.string(&["message"]),
        MAINTENANCE_MODE_MESSAGE,
        "case {case}: expected the maintenance-mode sentence, got {answer:?}"
    );
}

/// Every `CreateAgent` an account with no recorded prior usage makes is refused, and the
/// store is left exactly as it was: the listing still holds nothing.
#[tokio::test]
async fn an_account_with_no_prior_usage_is_refused_every_create_agent_and_writes_nothing() {
    let emulator = Emulator::without_prior_agent_usage();

    for (case, request) in create_requests("new-customer") {
        let refused = emulator.create_agent(request.clone()).await;
        assert_refused(case, &refused);
        assert!(
            refused.member(&["agent"]).is_null(),
            "case {case}: a refusal carries no created agent, got {refused:?}"
        );

        // Nor does a retry of the same request: the refusal is a standing condition of
        // the account, not a transient one.
        let again = emulator.create_agent(request).await;
        assert_refused(case, &again);
    }

    let listed = emulator.list_agents(json!({})).await;
    assert_eq!(
        listed.status, 200,
        "case \"the listing after the refusals\": expected 200, got {listed:?}"
    );
    assert_eq!(
        listed.member(&["agentSummaries"]).as_array().map(Vec::len),
        Some(0),
        "case \"the listing after the refusals\": a refused create must have written \
         nothing, got {listed:?}"
    );
}

/// The condition is the ACCOUNT's and not the request's: recording the account's prior
/// usage of the agent surface serves the very bodies that were refused, each of which then
/// round-trips through `GetAgent`.
#[tokio::test]
async fn recording_the_accounts_prior_usage_serves_the_requests_it_refused() {
    let emulator = Emulator::without_prior_agent_usage();

    let requests = create_requests("existing-customer");
    for (case, request) in &requests {
        let refused = emulator.create_agent(request.clone()).await;
        assert_refused(case, &refused);
    }

    emulator.record_prior_agent_usage();

    for (case, request) in &requests {
        let created = emulator.create_agent(request.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case}: once the account's prior usage is recorded, expected \
             CreateAgent -> 202, got {created:?}"
        );
        let agent_id = created.string(&["agent", "agentId"]);
        let expected_name = request["agentName"]
            .as_str()
            .expect("every row names the agent it creates");
        assert_eq!(
            created.string(&["agent", "agentName"]),
            expected_name,
            "case {case}: expected the agent the request named, got {created:?}"
        );

        // The create is BACKED: the plane reports the same agent back, and its ARN
        // belongs to the account whose usage the create turned on.
        let fetched = emulator.get_agent(&agent_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetAgent -> 200 for the created agent, got {fetched:?}"
        );
        assert_eq!(
            fetched.string(&["agent", "agentName"]),
            expected_name,
            "case {case}: GetAgent must report the agent the create persisted, got \
             {fetched:?}"
        );
        assert!(
            fetched
                .string(&["agent", "agentArn"])
                .contains(&format!(":{ACCOUNT}:agent/")),
            "case {case}: expected the agent ARN to carry this account, got {fetched:?}"
        );
    }

    let listed = emulator.list_agents(json!({ "maxResults": 10 })).await;
    assert_eq!(
        listed.member(&["agentSummaries"]).as_array().map(Vec::len),
        Some(requests.len()),
        "case \"the listing after the accepted creates\": expected one summary per \
         request, got {listed:?}"
    );
}

/// Only the minting of a new agent turns on the account's history: everything else the
/// plane serves answers its own resource's status for an account with no prior usage —
/// which is what lets a caller tell this refusal from an authorization failure.
#[tokio::test]
async fn an_account_with_no_prior_usage_keeps_the_rest_of_the_plane() {
    let emulator = Emulator::without_prior_agent_usage();

    // Rows: an operation of the plane an ineligible account calls, with the status and
    // error shape it owes — the agent read and list plane answers its own resource's
    // status, and another create family of the same plane is untouched by a condition
    // that belongs to agents.
    for (case, method, uri, body, status, error_type) in [
        (
            "ListAgents, the positive control",
            "POST",
            "/agents/".to_string(),
            Some(json!({})),
            200,
            None,
        ),
        (
            "GetAgent on an agent no account here holds",
            "GET",
            format!("/agents/{UNKNOWN_AGENT}/"),
            None,
            404,
            Some("ResourceNotFoundException"),
        ),
        (
            "DeleteAgent on an agent no account here holds",
            "DELETE",
            format!("/agents/{UNKNOWN_AGENT}/"),
            None,
            404,
            Some("ResourceNotFoundException"),
        ),
        (
            "PrepareAgent on an agent no account here holds",
            "POST",
            format!("/agents/{UNKNOWN_AGENT}/"),
            Some(json!({})),
            404,
            Some("ResourceNotFoundException"),
        ),
        (
            "CreateFlow, another create family of the same plane",
            "POST",
            "/flows/".to_string(),
            Some(json!({ "name": "new-customer-flow", "executionRoleArn": FLOW_ROLE })),
            201,
            None,
        ),
    ] {
        let answer = emulator.call(method, &uri, body).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            error_type,
            "case {case}: expected the error shape {error_type:?}, got {answer:?}"
        );
    }

    // And the boundary still decides a malformed request before the account does: a
    // name outside the modelled `agentName` pattern is a validation failure, not a
    // maintenance-mode refusal.
    let malformed = emulator
        .create_agent(json!({ "agentName": "not a legal agent name!" }))
        .await;
    assert_eq!(
        malformed.status, 400,
        "case \"a malformed agentName\": expected 400 from the boundary, got {malformed:?}"
    );
    assert_eq!(
        malformed.error_type.as_deref(),
        Some("ValidationException"),
        "case \"a malformed agentName\": expected ValidationException, got {malformed:?}"
    );
}
