// SPDX-License-Identifier: CC0-1.0

//! `DeletePolicy` end to end over HTTP, with the reads that observe what it did.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised against the one shared engine.
//!
//! The rule under test, read off the Smithy model rather than off any one recorded request
//! (`smithy_query(query="operation",operation="DeletePolicy")`): a policy is deleted only
//! through the engine that evaluates it, so a policy the addressed engine holds has its
//! deletion ACCEPTED (HTTP 202) and is reported with the members it was stored with and
//! the `DELETING` status an accepted-but-unfinished deletion carries ("This is an
//! asynchronous operation. Use the GetPolicy operation to poll the status field to track
//! completion"); it stays readable in `DELETING` until the span the clock owes passes and
//! is gone afterwards, freeing the name it held, while an engine or a policy the path does
//! not resolve is refused the modelled `ResourceNotFoundException` (HTTP 404) and deletes
//! nothing.
//!
//! The rows differ along what the rule generalises over: the member of the `definition`
//! union, the enforcement mode, whether a description is supplied, the shape of the engine
//! and policy names, and whether the policy is still `CREATING` or already `ACTIVE` when
//! the deletion arrives.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// The settle span `src/state/policies.rs` gives the policy plane's clock-owed
/// transitions, plus slack, so a row can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(3_200);

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the HTTP status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<&Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

/// The error code `restJson1` puts in the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

async fn create_policy_engine(router: &MultiServiceRouter, name: &str) -> Answer {
    send(router, "POST", "/policy-engines", Some(&json!({"name": name}))).await
}

async fn create_policy(router: &MultiServiceRouter, policy_engine: &str, body: &Value) -> Answer {
    send(
        router,
        "POST",
        &format!("/policy-engines/{policy_engine}/policies"),
        Some(body),
    )
    .await
}

async fn get_policy(router: &MultiServiceRouter, policy_engine: &str, policy: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/policy-engines/{policy_engine}/policies/{policy}"),
        None,
    )
    .await
}

async fn delete_policy(router: &MultiServiceRouter, policy_engine: &str, policy: &str) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/policy-engines/{policy_engine}/policies/{policy}"),
        None,
    )
    .await
}

async fn get_policy_engine(router: &MultiServiceRouter, policy_engine: &str) -> Answer {
    send(router, "GET", &format!("/policy-engines/{policy_engine}"), None).await
}

/// A Cedar definition: `CedarPolicy$statement` is 35..10000 characters
/// (`smithy_query(query="shape",shape="CedarPolicy")`).
fn cedar_definition(action: &str) -> Value {
    json!({"cedar": {
        "statement": format!("permit(principal, action == Action::\"{action}\", resource);"),
    }})
}

/// A Dogwood definition, the other statement-carrying member of the union
/// (`smithy_query(query="shape",shape="PolicyDefinition")`).
fn dogwood_definition(tool: &str) -> Value {
    json!({"policy": {
        "statement": format!("forbid(principal, action, resource) when {{ tool == \"{tool}\" }};"),
    }})
}

/// The sentence this service refuses with when the path names no policy engine of this
/// account. The fixed text is the service's; the only variable part is the id the path
/// carried.
fn engine_not_found_message(policy_engine_id: &str) -> String {
    format!("Policy engine with ID {policy_engine_id} not found")
}

/// The sentence this service refuses with when the addressed engine holds no such policy.
fn policy_not_found_message(policy_id: &str) -> String {
    format!("Policy with ID {policy_id} not found")
}

/// The members `DeletePolicyResponse` and `GetPolicyResponse` both declare about the
/// policy itself, apart from the status the deletion moves
/// (`smithy_query(query="operation",operation="DeletePolicy,GetPolicy")`).
const POLICY_MEMBERS: &[&str] = &[
    "policyId",
    "name",
    "policyEngineId",
    "policyArn",
    "definition",
    "description",
    "enforcementMode",
    "createdAt",
];

/// One row of the deletion table.
struct Row {
    case: &'static str,
    /// The engine's name (`PolicyEngineName` is `^[A-Za-z][A-Za-z0-9_]*$`, 1..48).
    engine_name: &'static str,
    /// The policy's name (`PolicyName` carries the same pattern).
    policy_name: &'static str,
    definition: Value,
    enforcement_mode: Option<&'static str>,
    description: Option<&'static str>,
    /// Whether the clock has already settled the policy to `ACTIVE` before the deletion
    /// arrives; `false` deletes it while it is still `CREATING`.
    settled_before_delete: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "required members only, a Cedar definition, deleted while CREATING",
            engine_name: "Gateway_engine",
            policy_name: "AllowOrders",
            definition: cedar_definition("invokeTool"),
            enforcement_mode: None,
            description: None,
            settled_before_delete: false,
        },
        Row {
            case: "a Dogwood definition, LOG_ONLY, with a description, deleted once ACTIVE",
            engine_name: "A",
            policy_name: "LogOnlySearch",
            definition: dogwood_definition("search"),
            enforcement_mode: Some("LOG_ONLY"),
            description: Some("collect data before enforcing"),
            settled_before_delete: true,
        },
        Row {
            case: "the longest engine and policy names this suite uses, ACTIVE named \
                   explicitly",
            engine_name: "Engine_with_a_long_but_valid_name_for_account",
            policy_name: "Policy_with_a_long_but_valid_name_for_account",
            definition: cedar_definition("listTools"),
            enforcement_mode: Some("ACTIVE"),
            description: Some("x"),
            settled_before_delete: false,
        },
    ]
}

impl Row {
    /// The `CreatePolicy` body this row sends.
    fn policy_body(&self) -> Value {
        let mut body = json!({"name": self.policy_name, "definition": self.definition});
        if let Some(enforcement_mode) = self.enforcement_mode {
            body["enforcementMode"] = json!(enforcement_mode);
        }
        if let Some(description) = self.description {
            body["description"] = json!(description);
        }
        body
    }
}

/// An accepted deletion reports the stored policy in `DELETING`, the policy stays readable
/// in that status until the clock-owed removal comes due, and is gone afterwards — which
/// frees the name it held while the engine that evaluated it stays.
#[tokio::test]
async fn an_accepted_policy_deletion_reports_deleting_and_then_the_policy_is_gone() {
    for row in rows() {
        let router = router();
        let engine = create_policy_engine(&router, row.engine_name).await;
        assert_eq!(
            engine.status, 202,
            "case {:?}: expected HTTP 202 from CreatePolicyEngine, got {} ({})",
            row.case, engine.status, engine.body
        );
        let policy_engine_id = text(&engine.body, "policyEngineId");
        let created = create_policy(&router, &policy_engine_id, &row.policy_body()).await;
        assert_eq!(
            created.status, 202,
            "case {:?}: expected HTTP 202 from CreatePolicy, got {} ({})",
            row.case, created.status, created.body
        );
        let policy_id = text(&created.body, "policyId");

        if row.settled_before_delete {
            tokio::time::sleep(PAST_SETTLE).await;
            let active = get_policy(&router, &policy_engine_id, &policy_id).await;
            assert_eq!(
                (active.status, text(&active.body, "status").as_str()),
                (200, "ACTIVE"),
                "case {:?}: the policy settles to ACTIVE before the deletion, got {} {} ({})",
                row.case,
                active.status,
                text(&active.body, "status"),
                active.body
            );
        }

        let deleted = delete_policy(&router, &policy_engine_id, &policy_id).await;
        assert_eq!(
            deleted.status, 202,
            "case {:?}: expected HTTP 202 from DeletePolicy, got {} ({})",
            row.case, deleted.status, deleted.body
        );
        assert_eq!(
            (
                text(&deleted.body, "status"),
                deleted.body.get("statusReasons").cloned()
            ),
            ("DELETING".to_string(), Some(json!([]))),
            "case {:?}: an accepted deletion reports the policy DELETING with no status \
             reason, got {}",
            row.case,
            deleted.body
        );
        for member in POLICY_MEMBERS {
            assert_eq!(
                deleted.body.get(member),
                created.body.get(member),
                "case {:?}: the deletion reports the {member} the create did, got {}",
                row.case,
                deleted.body
            );
        }
        assert!(
            !text(&deleted.body, "updatedAt").is_empty(),
            "case {:?}: the deletion reports when the policy was last modified, got {}",
            row.case,
            deleted.body
        );

        // The deletion is asynchronous, so the policy is still readable — in `DELETING`,
        // which is what a caller polling the status field tracks completion by.
        let polled = get_policy(&router, &policy_engine_id, &policy_id).await;
        assert_eq!(
            (polled.status, text(&polled.body, "status").as_str()),
            (200, "DELETING"),
            "case {:?}: the policy polls DELETING while the deletion is under way, got {} \
             {} ({})",
            row.case,
            polled.status,
            text(&polled.body, "status"),
            polled.body
        );
        for member in POLICY_MEMBERS {
            assert_eq!(
                polled.body.get(member),
                created.body.get(member),
                "case {:?}: a policy on its way out still reports its {member}, got {}",
                row.case,
                polled.body
            );
        }

        // Once the span the clock owes passes, the policy is gone.
        tokio::time::sleep(PAST_SETTLE).await;
        let gone = get_policy(&router, &policy_engine_id, &policy_id).await;
        assert_eq!(
            (
                gone.status,
                error_code(&gone.body),
                text(&gone.body, "message")
            ),
            (
                404,
                "ResourceNotFoundException".to_string(),
                policy_not_found_message(&policy_id)
            ),
            "case {:?}: once the deletion completes the policy is the modelled 404, got {} \
             ({})",
            row.case,
            gone.status,
            gone.body
        );

        // The engine that evaluated it is untouched by the policy's removal.
        let engine_after = get_policy_engine(&router, &policy_engine_id).await;
        assert_eq!(
            engine_after.status, 200,
            "case {:?}: deleting a policy does not delete the engine that held it, got {} \
             ({})",
            row.case, engine_after.status, engine_after.body
        );

        // "Must be unique within the account" held the name while the policy existed; a
        // deleted policy holds it no longer.
        let recreated = create_policy(&router, &policy_engine_id, &row.policy_body()).await;
        assert_eq!(
            recreated.status, 202,
            "case {:?}: the name a deleted policy held is free again, got {} ({})",
            row.case, recreated.status, recreated.body
        );
        assert_ne!(
            text(&recreated.body, "policyId"),
            policy_id,
            "case {:?}: the re-created policy is a new resource, got {}",
            row.case,
            recreated.body
        );
    }
}

/// `DeletePolicy` is `@idempotent`: a repeat while the deletion is still under way reports
/// the same policy and does not re-arm the clock that owes its removal.
#[tokio::test]
async fn repeating_a_policy_deletion_under_way_reports_the_same_policy() {
    let router = router();
    let engine = create_policy_engine(&router, "Repeat_engine").await;
    let policy_engine_id = text(&engine.body, "policyEngineId");
    let created = create_policy(
        &router,
        &policy_engine_id,
        &json!({"name": "RepeatedDelete", "definition": cedar_definition("invokeTool")}),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: the policy must be created, got {} ({})",
        created.status, created.body
    );
    let policy_id = text(&created.body, "policyId");

    let first = delete_policy(&router, &policy_engine_id, &policy_id).await;
    assert_eq!(
        first.status, 202,
        "the first delete is accepted, got {} ({})",
        first.status, first.body
    );
    // Three repeats, spread across the span the first delete armed, so a re-arming
    // implementation would push the removal past the wait below.
    for attempt in 1..=3u32 {
        tokio::time::sleep(PAST_SETTLE / 4).await;
        let repeated = delete_policy(&router, &policy_engine_id, &policy_id).await;
        assert_eq!(
            (repeated.status, text(&repeated.body, "status").as_str()),
            (202, "DELETING"),
            "case repeat {attempt}: a repeat while the deletion is under way is accepted \
             and still DELETING, got {} {} ({})",
            repeated.status,
            text(&repeated.body, "status"),
            repeated.body
        );
        for member in POLICY_MEMBERS {
            assert_eq!(
                repeated.body.get(member),
                first.body.get(member),
                "case repeat {attempt}: the repeat reports the {member} the first delete \
                 did, got {}",
                repeated.body
            );
        }
        assert_eq!(
            repeated.body.get("updatedAt"),
            first.body.get("updatedAt"),
            "case repeat {attempt}: a repeat changes nothing, so the policy was not \
             modified again, got {}",
            repeated.body
        );
    }

    // The clock the FIRST delete armed is the one that removes the record.
    tokio::time::sleep(PAST_SETTLE).await;
    let gone = delete_policy(&router, &policy_engine_id, &policy_id).await;
    assert_eq!(
        (gone.status, text(&gone.body, "message")),
        (404, policy_not_found_message(&policy_id)),
        "a delete of a policy whose removal has come due is the modelled 404, got {} ({})",
        gone.status,
        gone.body
    );
}

/// A delete the path cannot resolve — to a policy engine this account does not hold, to a
/// policy that engine does not hold, or to another engine's policy — is refused the
/// modelled 404 and deletes nothing.
#[tokio::test]
async fn a_delete_the_path_does_not_resolve_is_refused_and_deletes_nothing() {
    let router = router();
    // Every id below satisfies `ResourceId`'s pattern
    // `^[A-Za-z][A-Za-z0-9_]*-[a-z0-9_]{10}$`, so the refusal is the handler's lookup and
    // not the boundary constraint walk: the shortest id the pattern admits, one carrying
    // underscores, one at the top of the length range, and one whose suffix is all
    // underscores.
    let unknown = [
        "A-0123456789",
        "Missing_engine-abcdefghij",
        "Engine_with_a_long_but_valid_name_for_account-zz00zz00zz",
        "Underscores-__________",
    ];

    let first = create_policy_engine(&router, "First_engine").await;
    let second = create_policy_engine(&router, "Second_engine").await;
    let first_id = text(&first.body, "policyEngineId");
    let second_id = text(&second.body, "policyEngineId");
    let created = create_policy(
        &router,
        &first_id,
        &json!({"name": "OnlyInFirst", "definition": dogwood_definition("search")}),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: the policy must be created, got {} ({})",
        created.status, created.body
    );
    let policy_id = text(&created.body, "policyId");

    for policy_engine_id in unknown {
        // An engine the path names and this account does not hold is reported in its own
        // sentence, whatever the policy id alongside it.
        for addressed_policy in [policy_id.as_str(), "NoSuchPolicy-0123456789"] {
            let refused = delete_policy(&router, policy_engine_id, addressed_policy).await;
            assert_eq!(
                (
                    refused.status,
                    error_code(&refused.body),
                    text(&refused.body, "message")
                ),
                (
                    404,
                    "ResourceNotFoundException".to_string(),
                    engine_not_found_message(policy_engine_id)
                ),
                "case {policy_engine_id:?}/{addressed_policy:?}: the refusal names the \
                 engine the path carried, got {} ({})",
                refused.status,
                refused.body
            );
        }
        // And a policy the EXISTING engine does not hold is reported as the missing
        // policy, not as a missing engine.
        let refused = delete_policy(&router, &first_id, policy_engine_id).await;
        assert_eq!(
            (
                refused.status,
                error_code(&refused.body),
                text(&refused.body, "message")
            ),
            (
                404,
                "ResourceNotFoundException".to_string(),
                policy_not_found_message(policy_engine_id)
            ),
            "case {policy_engine_id:?}: an id no policy of the engine carries is the \
             modelled 404, got {} ({})",
            refused.status,
            refused.body
        );
    }

    // A policy is deleted only through the engine that evaluates it.
    let through_other = delete_policy(&router, &second_id, &policy_id).await;
    assert_eq!(
        (
            through_other.status,
            text(&through_other.body, "message").as_str()
        ),
        (404, policy_not_found_message(&policy_id).as_str()),
        "a policy of one engine is not deletable through another, got {} ({})",
        through_other.status,
        through_other.body
    );

    // None of the refusals touched the policy: it is still there, and still not DELETING.
    let intact = get_policy(&router, &first_id, &policy_id).await;
    assert_eq!(
        intact.status, 200,
        "a refused delete leaves the policy readable, got {} ({})",
        intact.status, intact.body
    );
    assert_ne!(
        text(&intact.body, "status"),
        "DELETING",
        "a refused delete does not start the policy on its way out, got {}",
        intact.body
    );
    for member in POLICY_MEMBERS {
        assert_eq!(
            intact.body.get(member),
            created.body.get(member),
            "a refused delete leaves the {member} the create reported, got {}",
            intact.body
        );
    }
}
