// SPDX-License-Identifier: CC0-1.0

//! The agent lifecycle, end to end over HTTP: `CreateAgent`, `GetAgent`,
//! `ListAgents`, `PrepareAgent` and `DeleteAgent` against one shared engine.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::{
    service_state, AGENT_QUOTA, DEFAULT_IDLE_SESSION_TTL_SECONDS, MAX_AGENTS_PER_PAGE, PREPARE_SPAN,
};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on, and the source of the region its ARNs
/// carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The region of [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    pub(crate) error_type: Option<String>,
    pub(crate) body: Value,
}

impl Answer {
    pub(crate) fn member(&self, path: &[&str]) -> &Value {
        let mut at = &self.body;
        for key in path {
            at = &at[*key];
        }
        at
    }

    pub(crate) fn string(&self, path: &[&str]) -> String {
        self.member(path)
            .as_str()
            .unwrap_or_else(|| panic!("{path:?} is not a string in {:?}", self.body))
            .to_string()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The SAME engine the mounted service reads and writes, so a suite can record
    /// what no modelled operation writes — see [`Emulator::record_prior_agent_usage`].
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// The emulator as an EXISTING customer's account sees it: the account's prior
    /// usage of the agent surface is recorded, because that surface is closed to an
    /// account with none and every suite here drives agents that exist.
    pub(crate) fn new() -> Self {
        let emulator = Self::without_prior_agent_usage();
        emulator.record_prior_agent_usage();
        emulator
    }

    /// The emulator as a NEW customer's account sees it: nothing has recorded any
    /// prior usage of the agent surface, which is what `CreateAgent` refuses on —
    /// `tests/all/agents_classic_maintenance_mode.rs` is that rule's suite.
    pub(crate) fn without_prior_agent_usage() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Record this account's prior usage of the agent surface in the plane's own
    /// store. This is the producer the API does not model; it is NOT an AWS
    /// operation.
    pub(crate) fn record_prior_agent_usage(&self) {
        service_state(&*self.engine)
            .expect("the bedrock-agent store is reachable through the shared engine")
            .lock()
            .agents_classic_usage
            .record(ACCOUNT);
    }

    pub(crate) async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bedrock-agent.eu-west-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|e| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({e}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let error_type = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            body,
        }
    }

    pub(crate) async fn create_agent(&self, request: Value) -> Answer {
        self.call("PUT", "/agents/", Some(request)).await
    }

    pub(crate) async fn get_agent(&self, agent_id: &str) -> Answer {
        self.call("GET", &format!("/agents/{agent_id}/"), None).await
    }

    pub(crate) async fn list_agents(&self, request: Value) -> Answer {
        self.call("POST", "/agents/", Some(request)).await
    }

    async fn prepare_agent(&self, agent_id: &str) -> Answer {
        self.call("POST", &format!("/agents/{agent_id}/"), Some(json!({})))
            .await
    }

    async fn delete_agent(&self, agent_id: &str) -> Answer {
        self.call("DELETE", &format!("/agents/{agent_id}/"), None)
            .await
    }
}

/// An `instruction` long enough for the member's modelled `length 40..4000`.
pub(crate) fn instruction(flavour: &str) -> String {
    format!("You are a {flavour} assistant. Answer the customer precisely and cite your sources.")
}

/// Assert `agent_id` is shaped as the model's `Id` demands (`^[0-9a-zA-Z]{10}$`).
fn assert_well_formed_id(case: &str, agent_id: &str) {
    assert_eq!(
        agent_id.len(),
        10,
        "case {case}: expected a 10-character agentId, got {agent_id:?}"
    );
    assert!(
        agent_id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case}: expected agentId to match ^[0-9a-zA-Z]{{10}}$, got {agent_id:?}"
    );
}

/// `CreateAgent` persists every member the request supplied, and `GetAgent`
/// reports that record back — over a table of requests that differ in which
/// optional members they carry, the TTL they ask for, and the model identifier
/// form they use.
#[tokio::test]
async fn created_agents_round_trip_through_get_agent() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: which optional members
    // are present, the `idleSessionTTLInSeconds` asked for (both published bounds
    // and one in between, plus absent -> the default), and the shape of the
    // foundation-model identifier.
    let cases: Vec<(&str, Value, i64)> = vec![
        (
            "minimal: only the required agentName",
            json!({ "agentName": "minimal-agent" }),
            i64::from(DEFAULT_IDLE_SESSION_TTL_SECONDS),
        ),
        (
            "ttl at the lower published bound",
            json!({
                "agentName": "ttl-low-agent",
                "foundationModel": "amazon.titan-text-express-v1",
                "idleSessionTTLInSeconds": 60,
            }),
            60,
        ),
        (
            "ttl at the upper published bound",
            json!({
                "agentName": "ttl-high-agent",
                "foundationModel": "anthropic.claude-3-sonnet-20240229-v1:0",
                "idleSessionTTLInSeconds": 5400,
            }),
            5400,
        ),
        (
            "every lifecycle member supplied",
            json!({
                "agentName": "full-agent",
                "description": "an agent carrying every member of this slice",
                "instruction": instruction("support"),
                "foundationModel": "anthropic.claude-v2",
                "idleSessionTTLInSeconds": 900,
                "agentResourceRoleArn": "arn:aws:iam::000000000000:role/AgentRole",
                "customerEncryptionKeyArn":
                    "arn:aws:kms:eu-west-1:000000000000:key/12345678-1234-1234-1234-123456789012",
                "agentCollaboration": "DISABLED",
                "orchestrationType": "DEFAULT",
                "tags": { "team": "platform" },
            }),
            900,
        ),
        (
            "underscore-separated name, instruction only",
            json!({
                "agentName": "report_builder_agent",
                "instruction": instruction("reporting"),
            }),
            i64::from(DEFAULT_IDLE_SESSION_TTL_SECONDS),
        ),
    ];

    for (case, request, expected_ttl) in cases {
        let created = emulator.create_agent(request.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {case}: expected CreateAgent -> 202, got {created:?}"
        );

        let agent_id = created.string(&["agent", "agentId"]);
        assert_well_formed_id(case, &agent_id);
        assert_eq!(
            created.string(&["agent", "agentArn"]),
            format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:agent/{agent_id}"),
            "case {case}: agentArn must be built from the request's region, account and the minted id"
        );
        assert_eq!(
            created.string(&["agent", "agentVersion"]),
            "DRAFT",
            "case {case}: a newly created agent has only the DRAFT version"
        );
        assert_eq!(
            created.string(&["agent", "agentStatus"]),
            "NOT_PREPARED",
            "case {case}: a newly created agent has not been prepared"
        );
        assert!(
            created.member(&["agent", "createdAt"]).is_number()
                && created.member(&["agent", "updatedAt"]).is_number(),
            "case {case}: createdAt/updatedAt must be populated, got {:?}",
            created.body["agent"]
        );
        assert!(
            created.member(&["agent", "preparedAt"]).is_null(),
            "case {case}: an unprepared agent has no preparedAt, got {:?}",
            created.member(&["agent", "preparedAt"])
        );

        // The round trip: the stored record, read back through GetAgent.
        let fetched = emulator.get_agent(&agent_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case}: expected GetAgent -> 200, got {fetched:?}"
        );
        assert_eq!(
            fetched.body["agent"], created.body["agent"],
            "case {case}: GetAgent must report exactly the record CreateAgent persisted"
        );
        assert_eq!(
            fetched.member(&["agent", "idleSessionTTLInSeconds"]).as_i64(),
            Some(expected_ttl),
            "case {case}: expected idleSessionTTLInSeconds {expected_ttl}, got {:?}",
            fetched.member(&["agent", "idleSessionTTLInSeconds"])
        );

        // Every member the request supplied survives the round trip verbatim.
        for member in [
            "agentName",
            "description",
            "instruction",
            "foundationModel",
            "agentResourceRoleArn",
            "customerEncryptionKeyArn",
            "agentCollaboration",
            "orchestrationType",
        ] {
            if let Some(supplied) = request.get(member) {
                assert_eq!(
                    fetched.member(&["agent", member]),
                    supplied,
                    "case {case}: expected {member} to round-trip as {supplied:?}, got {:?}",
                    fetched.member(&["agent", member])
                );
            }
        }

        // An agentResourceRoleArn the request omitted is still populated: the
        // member is required on `Agent`.
        if request.get("agentResourceRoleArn").is_none() {
            let role = fetched.string(&["agent", "agentResourceRoleArn"]);
            assert!(
                role.starts_with("arn:aws:iam::") && role.contains(":role/"),
                "case {case}: expected a well-formed default role ARN, got {role:?}"
            );
        }
    }
}

/// A name already in use is a conflict; a repeated `clientToken` is the same
/// agent, not a second one.
#[tokio::test]
async fn create_agent_refuses_a_duplicate_name_and_honours_its_idempotency_token() {
    let emulator = Emulator::new();

    let first = emulator
        .create_agent(json!({ "agentName": "conflict-agent" }))
        .await;
    assert_eq!(
        first.status, 202,
        "case first create: expected 202, got {first:?}"
    );

    let duplicate = emulator
        .create_agent(json!({ "agentName": "conflict-agent", "description": "a different body" }))
        .await;
    assert_eq!(
        duplicate.status, 409,
        "case duplicate name: expected 409, got {duplicate:?}"
    );
    assert_eq!(
        duplicate.error_type.as_deref(),
        Some("ConflictException"),
        "case duplicate name: expected ConflictException, got {duplicate:?}"
    );

    // Two @idempotencyToken values, each replayed: a replay returns the agent the
    // first call created and mints nothing new.
    for token in [
        "idempotency-token-for-the-first-agent",
        "IdempotencyTokenForTheSecondAgent00",
    ] {
        let request = json!({ "agentName": format!("idem-{}", &token[..8]), "clientToken": token });
        let once = emulator.create_agent(request.clone()).await;
        let again = emulator.create_agent(request).await;
        assert_eq!(
            once.status, 202,
            "case token {token}: expected 202 on the first call, got {once:?}"
        );
        assert_eq!(
            again.status, 202,
            "case token {token}: expected 202 on the replay, got {again:?}"
        );
        assert_eq!(
            again.string(&["agent", "agentId"]),
            once.string(&["agent", "agentId"]),
            "case token {token}: a replayed idempotency token must return the same agent"
        );
    }

    // Three agents exist: the conflict one and one per idempotency token.
    let listed = emulator.list_agents(json!({})).await;
    assert_eq!(
        listed.member(&["agentSummaries"]).as_array().map(Vec::len),
        Some(3),
        "expected exactly 3 agents after two replayed creates, got {listed:?}"
    );
}

/// `ListAgents` returns exactly the stored agents, in creation order, truncated
/// to `maxResults`, with a `nextToken` only while more remain — over a table of
/// page sizes.
#[tokio::test]
async fn list_agents_pages_over_every_stored_agent() {
    let emulator = Emulator::new();

    // Created in a known order, so the expected listing order is known.
    let mut expected_ids = Vec::new();
    for index in 0..7 {
        let created = emulator
            .create_agent(json!({ "agentName": format!("paged-agent-{index}") }))
            .await;
        assert_eq!(
            created.status, 202,
            "case agent {index}: expected 202, got {created:?}"
        );
        expected_ids.push(created.string(&["agent", "agentId"]));
    }

    // Rows differ in the page size asked for: one, a few, exactly the number of
    // agents, more than exist, above the modelled page cap, and absent.
    for requested in [Some(1), Some(2), Some(3), Some(7), Some(8), Some(1000), None] {
        let page_size = requested
            .map(|value: usize| value.min(MAX_AGENTS_PER_PAGE))
            .unwrap_or(MAX_AGENTS_PER_PAGE);
        let case = format!("maxResults {requested:?}");

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(value) = requested {
                request["maxResults"] = json!(value);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let answer = emulator.list_agents(request).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected ListAgents -> 200, got {answer:?}"
            );
            let summaries = answer
                .member(&["agentSummaries"])
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: agentSummaries is not a list: {answer:?}"))
                .clone();
            let remaining = expected_ids.len() - seen.len();
            assert_eq!(
                summaries.len(),
                remaining.min(page_size),
                "case {case}: expected a page of {} summaries, got {}",
                remaining.min(page_size),
                summaries.len()
            );
            for summary in &summaries {
                // Every required AgentSummary member is projected from the record.
                for member in ["agentId", "agentName", "agentStatus"] {
                    assert!(
                        summary[member].is_string(),
                        "case {case}: summary is missing {member}: {summary:?}"
                    );
                }
                assert!(
                    summary["updatedAt"].is_number(),
                    "case {case}: summary is missing updatedAt: {summary:?}"
                );
                assert_eq!(
                    summary["agentStatus"], "NOT_PREPARED",
                    "case {case}: expected the stored status, got {summary:?}"
                );
                seen.push(
                    summary["agentId"]
                        .as_str()
                        .expect("agentId is a string")
                        .to_string(),
                );
            }

            pages += 1;
            next_token = answer
                .member(&["nextToken"])
                .as_str()
                .map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= expected_ids.len(),
                "case {case}: pagination did not terminate after {pages} pages"
            );
        }

        assert_eq!(
            seen, expected_ids,
            "case {case}: paging must visit every agent exactly once, in creation order"
        );
        let expected_pages = expected_ids.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} pages of {page_size}, got {pages}"
        );
    }
}

/// A `nextToken` this service never issued names no position in the listing.
#[tokio::test]
async fn list_agents_refuses_a_token_it_did_not_issue() {
    let emulator = Emulator::new();
    emulator
        .create_agent(json!({ "agentName": "token-agent" }))
        .await;

    // Rows: strings no page of this listing handed out. A token names a
    // position as the one spelling `format!` renders — the creation instant in
    // nanoseconds, then the 10-character id of the agent the page stopped at —
    // so another spelling of the same number, or an id no agent could be
    // minted, names no position.
    for token in [
        "not-a-cursor",
        "0",
        "123456789",
        "-AAAAAAAAAA",
        "1700000000000000000-",
        "+1-AAAAAAAAAA",
        "0001-AAAAAAAAAA",
        "0x10-AAAAAAAAAA",
        "1.5-AAAAAAAAAA",
        "1700000000000000000-AAAA",
        "1700000000000000000-AAAA:AAAAA",
    ] {
        let answer = emulator.list_agents(json!({ "nextToken": token })).await;
        assert_eq!(
            answer.status, 400,
            "case token {token:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ValidationException"),
            "case token {token:?}: expected ValidationException, got {answer:?}"
        );
    }
}

/// `PrepareAgent` mutates the stored agent: it reports `PREPARING` straight
/// away, and the clock carries it to `PREPARED`, which a later `GetAgent` sees.
#[tokio::test]
async fn prepare_agent_mutates_the_stored_agent() {
    let emulator = Emulator::new();
    let created = emulator
        .create_agent(json!({
            "agentName": "prepared-agent",
            "instruction": instruction("booking"),
            "foundationModel": "anthropic.claude-v2",
        }))
        .await;
    let agent_id = created.string(&["agent", "agentId"]);

    let prepared = emulator.prepare_agent(&agent_id).await;
    assert_eq!(
        prepared.status, 202,
        "case prepare: expected 202, got {prepared:?}"
    );
    assert_eq!(
        prepared.string(&["agentId"]),
        agent_id,
        "case prepare: expected the prepared agent's id, got {prepared:?}"
    );
    assert_eq!(
        prepared.string(&["agentVersion"]),
        "DRAFT",
        "case prepare: expected the DRAFT version, got {prepared:?}"
    );
    assert_eq!(
        prepared.string(&["agentStatus"]),
        "PREPARING",
        "case prepare: expected PREPARING, got {prepared:?}"
    );
    assert!(
        prepared.member(&["preparedAt"]).is_number(),
        "case prepare: expected preparedAt to be set, got {prepared:?}"
    );

    // The transient status is observable: a caller reading back immediately sees
    // the preparation still in flight.
    let while_preparing = emulator.get_agent(&agent_id).await;
    assert_eq!(
        while_preparing.string(&["agent", "agentStatus"]),
        "PREPARING",
        "case read during preparation: expected PREPARING, got {while_preparing:?}"
    );

    // Once the span the clock owes has elapsed, the agent is PREPARED — and stays
    // PREPARED across reads.
    tokio::time::sleep(PREPARE_SPAN + Duration::from_millis(100)).await;
    for read in ["first", "second"] {
        let fetched = emulator.get_agent(&agent_id).await;
        assert_eq!(
            fetched.string(&["agent", "agentStatus"]),
            "PREPARED",
            "case read {read} after preparation: expected PREPARED, got {fetched:?}"
        );
        assert_eq!(
            fetched.member(&["agent", "preparedAt"]),
            prepared.member(&["preparedAt"]),
            "case read {read} after preparation: preparedAt must be the instant PrepareAgent recorded"
        );
    }

    // ListAgents projects the same converged status.
    let listed = emulator.list_agents(json!({})).await;
    assert_eq!(
        listed.member(&["agentSummaries"])[0]["agentStatus"],
        "PREPARED",
        "expected the listing to report the stored status, got {listed:?}"
    );

    // Preparing an agent that does not exist is a 404, not a fabricated agent.
    let missing = emulator.prepare_agent("AAAAAAAAAA").await;
    assert_eq!(
        missing.status, 404,
        "case prepare unknown id: expected 404, got {missing:?}"
    );
    assert_eq!(
        missing.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case prepare unknown id: expected ResourceNotFoundException, got {missing:?}"
    );
}

/// `DeleteAgent` removes the agent: the id stops resolving and stops being
/// listed, and the name it held is free again.
#[tokio::test]
async fn delete_agent_removes_it_from_the_store() {
    let emulator = Emulator::new();

    let mut agent_ids = Vec::new();
    for index in 0..3 {
        let created = emulator
            .create_agent(json!({ "agentName": format!("doomed-agent-{index}") }))
            .await;
        agent_ids.push(created.string(&["agent", "agentId"]));
    }

    for (index, agent_id) in agent_ids.iter().enumerate() {
        let deleted = emulator.delete_agent(agent_id).await;
        assert_eq!(
            deleted.status, 202,
            "case delete {index}: expected 202, got {deleted:?}"
        );
        assert_eq!(
            deleted.string(&["agentId"]),
            *agent_id,
            "case delete {index}: expected the deleted agent's id, got {deleted:?}"
        );
        assert_eq!(
            deleted.string(&["agentStatus"]),
            "DELETING",
            "case delete {index}: expected DELETING, got {deleted:?}"
        );

        let fetched = emulator.get_agent(agent_id).await;
        assert_eq!(
            fetched.status, 404,
            "case delete {index}: a deleted agent must not resolve, got {fetched:?}"
        );
        assert_eq!(
            fetched.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case delete {index}: expected ResourceNotFoundException, got {fetched:?}"
        );

        let again = emulator.delete_agent(agent_id).await;
        assert_eq!(
            again.status, 404,
            "case delete {index}: deleting it twice must not succeed, got {again:?}"
        );

        let listed = emulator.list_agents(json!({})).await;
        let remaining: Vec<&str> = listed
            .member(&["agentSummaries"])
            .as_array()
            .expect("agentSummaries is a list")
            .iter()
            .map(|summary| summary["agentId"].as_str().expect("agentId is a string"))
            .collect();
        assert!(
            !remaining.contains(&agent_id.as_str()),
            "case delete {index}: the listing still carries {agent_id}: {remaining:?}"
        );
        assert_eq!(
            remaining.len(),
            agent_ids.len() - index - 1,
            "case delete {index}: expected {} agents left, got {remaining:?}",
            agent_ids.len() - index - 1
        );
    }

    // The freed name can be used again.
    let reused = emulator
        .create_agent(json!({ "agentName": "doomed-agent-0" }))
        .await;
    assert_eq!(
        reused.status, 202,
        "case reuse a deleted agent's name: expected 202, got {reused:?}"
    );
}

/// The store is bounded: once the quota is reached a create is refused with the
/// modelled `ServiceQuotaExceededException` rather than growing, while a replay
/// of an already-accepted create is still served.
#[tokio::test]
async fn create_agent_refuses_to_grow_past_the_agent_quota() {
    let emulator = Emulator::new();

    let token = "quota-idempotency-token-for-the-last-agent";
    for index in 0..AGENT_QUOTA {
        let mut request = json!({ "agentName": format!("quota-agent-{index}") });
        // The last accepted create carries an idempotency token, so the replay
        // below is of a create that succeeded.
        if index + 1 == AGENT_QUOTA {
            request["clientToken"] = json!(token);
        }
        let created = emulator.create_agent(request).await;
        assert_eq!(
            created.status, 202,
            "case agent {index} of the quota: expected 202, got {created:?}"
        );
    }

    // One past the quota, with and without a token: refused, not stored.
    for case in ["without a client token", "with a fresh client token"] {
        let mut request = json!({ "agentName": "one-agent-too-many" });
        if case.contains("fresh") {
            request["clientToken"] = json!("fresh-idempotency-token-for-the-refused-agent");
        }
        let refused = emulator.create_agent(request).await;
        assert_eq!(
            refused.status, 402,
            "case {case}: expected ServiceQuotaExceededException's 402, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ServiceQuotaExceededException"),
            "case {case}: expected ServiceQuotaExceededException, got {refused:?}"
        );
    }

    // The quota does not break idempotency: replaying an accepted create still
    // answers with the agent it created.
    let replayed = emulator
        .create_agent(json!({
            "agentName": format!("quota-agent-{}", AGENT_QUOTA - 1),
            "clientToken": token,
        }))
        .await;
    assert_eq!(
        replayed.status, 202,
        "case replay at the quota: expected 202, got {replayed:?}"
    );

    // Releasing one record makes room for exactly one more create.
    let freed = replayed.string(&["agent", "agentId"]);
    let deleted = emulator.delete_agent(&freed).await;
    assert_eq!(
        deleted.status, 202,
        "case free one slot: expected 202, got {deleted:?}"
    );
    let after = emulator
        .create_agent(json!({ "agentName": "agent-in-the-freed-slot" }))
        .await;
    assert_eq!(
        after.status, 202,
        "case create into the freed slot: expected 202, got {after:?}"
    );
}

/// A well-formed id that names no agent is a 404 — never a 501 and never an
/// invented agent.
#[tokio::test]
async fn unknown_agent_ids_are_not_found() {
    let emulator = Emulator::new();
    emulator
        .create_agent(json!({ "agentName": "only-agent" }))
        .await;

    // Rows differ along the shape of a well-formed but unused `Id`: all letters,
    // all digits, mixed case, and a boundary-looking value.
    for agent_id in ["AAAAAAAAAA", "0123456789", "aA0bB1cC2d", "zzzzzzzzz9"] {
        let answer = emulator.get_agent(agent_id).await;
        assert_eq!(
            answer.status, 404,
            "case id {agent_id}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case id {agent_id}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert!(
            answer.member(&["agent"]).is_null(),
            "case id {agent_id}: a not-found answer must carry no agent, got {answer:?}"
        );
    }
}
