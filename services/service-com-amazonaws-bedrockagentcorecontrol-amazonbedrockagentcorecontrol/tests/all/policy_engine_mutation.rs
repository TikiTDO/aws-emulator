// SPDX-License-Identifier: CC0-1.0

//! `UpdatePolicyEngine` end to end over HTTP: the change an accepted request applies to a
//! stored policy engine, and the requests this control plane refuses.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint walk and the handler are all exercised against the one shared engine.
//!
//! The rule under test, read off the Smithy model rather than off any one recorded request
//! (`smithy_query(query="operation",operation="UpdatePolicyEngine,GetPolicyEngine")` and
//! `smithy_query(query="shape",shape="UpdatedDescription")`): an accepted
//! `UpdatePolicyEngine` modifies only the description of the engine the path names, with the
//! PATCH semantics the `UpdatedDescription` wrapper states — "When present in an update
//! request, the description is replaced with optionalValue. When absent, the description is
//! left unchanged. To unset the description, include the wrapper with optionalValue not
//! specified" — leaves every other member the create stored (the id, the name, the ARN, the
//! key, `createdAt`) as it was, reports the engine `UPDATING` with the transition back to
//! `ACTIVE` owed by the clock and not by the call ("This is an asynchronous operation. Use
//! the GetPolicyEngine operation to poll the status field to track completion"), and reads
//! back through `GetPolicyEngine` exactly as the update reported it; an engine whose own
//! asynchronous work is still under way — so one that is not `ACTIVE` — refuses a further
//! edit the modelled `ConflictException` (HTTP 409) and keeps the description it had; and an
//! id naming no engine of this account is the modelled `ResourceNotFoundException`
//! (HTTP 404) and changes nothing.
//!
//! The rows differ along what the rule generalises over: whether the engine had a
//! description before the call, which form of the wrapper the request carries (a value, an
//! empty wrapper, no wrapper at all), the length of the replacement value at both ends of
//! `Description`'s 1..4096 `@length`, and — for the refusals — the status the engine is in
//! and the shape of the id the path carries.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The Region the rows sign for.
const REGION: &str = "us-east-1";

/// The account every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// How long a row waits for a clock-owed transition before it gives up. The span itself is
/// the policy plane's own; this only bounds a hang, so it is generous.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long the poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// `Description`'s `@length` maximum, which `UpdatedDescription$optionalValue` targets
/// (`smithy_query(query="shape",shape="UpdatedDescription")`).
const MAX_DESCRIPTION_LENGTH: usize = 4096;

/// A `KmsKeyArn` inside the member's `@pattern`, so a row can prove an update leaves the
/// engine's key alone.
const AWS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// The members `UpdatePolicyEngineResponse` and `GetPolicyEngineResponse` both declare, so a
/// round trip compares the whole envelope rather than the members one row happens to set.
const ENGINE_MEMBERS: [&str; 9] = [
    "policyEngineId",
    "name",
    "policyEngineArn",
    "status",
    "statusReasons",
    "description",
    "encryptionKeyArn",
    "createdAt",
    "updatedAt",
];

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

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Answer {
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

async fn create_policy_engine(router: &MultiServiceRouter, body: &Value) -> Answer {
    send(router, "POST", "/policy-engines", Some(body)).await
}

async fn get_policy_engine(router: &MultiServiceRouter, policy_engine_id: &str) -> Answer {
    send(router, "GET", &format!("/policy-engines/{policy_engine_id}"), None).await
}

async fn update_policy_engine(
    router: &MultiServiceRouter,
    policy_engine_id: &str,
    body: &Value,
) -> Answer {
    send(
        router,
        "PATCH",
        &format!("/policy-engines/{policy_engine_id}"),
        Some(body),
    )
    .await
}

/// The sentence this plane refuses a change with when the engine's own asynchronous work is
/// still under way. The text around the id is the service's; the only variable part is the
/// id the request's path carried.
fn concurrent_modification_message(policy_engine_id: &str) -> String {
    format!(
        "Concurrent modification on policy engine with ID {policy_engine_id} detected, \
         please try again once the policy engine status is ACTIVE"
    )
}

/// The sentence this plane words a missing engine with.
fn not_found_message(policy_engine_id: &str) -> String {
    format!("Policy engine with ID {policy_engine_id} not found")
}

/// A `DateTimestamp` is an RFC 3339 string on this wire (`@timestampFormat("date-time")`),
/// which is what a caller parses: a date, a `T`, a time and a `Z` offset.
fn is_date_time(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|rendered| {
        rendered.len() >= "2026-01-01T00:00:00Z".len()
            && rendered.as_bytes()[4] == b'-'
            && rendered.as_bytes()[10] == b'T'
            && rendered.ends_with('Z')
    })
}

/// Look again until the engine reports `ACTIVE`, which is what a caller polling the status
/// field does. The deadline only catches an engine that never settles at all.
async fn poll_until_active(
    router: &MultiServiceRouter,
    policy_engine_id: &str,
    case: &str,
) -> Answer {
    let started = Instant::now();
    loop {
        let read = get_policy_engine(router, policy_engine_id).await;
        if read.status == 200 && text(&read.body, "status") == "ACTIVE" {
            return read;
        }
        assert!(
            started.elapsed() < SETTLE_DEADLINE,
            "case {case:?}: the engine never reached ACTIVE, last answer {} ({})",
            read.status,
            read.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create an engine and wait out the create's own clock-owed transition, so a row starts
/// from an engine that is `ACTIVE` and can therefore take a change.
async fn active_engine(router: &MultiServiceRouter, body: &Value, case: &str) -> String {
    let created = create_policy_engine(router, body).await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: the engine must be created, got {} ({})",
        created.status, created.body
    );
    let policy_engine_id = text(&created.body, "policyEngineId");
    poll_until_active(router, &policy_engine_id, case).await;
    policy_engine_id
}

/// Which form of the `UpdatedDescription` wrapper a row sends.
#[derive(Debug, Clone, Copy)]
enum Patch {
    /// No wrapper at all, so the description is left unchanged.
    NoWrapper,
    /// The wrapper with no `optionalValue`, so the description is unset.
    EmptyWrapper,
    /// The wrapper carrying a replacement value.
    Value(&'static str),
}

impl Patch {
    /// The `UpdatePolicyEngine` body this form sends.
    fn body(self, longest: &str) -> Value {
        match self {
            Patch::NoWrapper => json!({}),
            Patch::EmptyWrapper => json!({"description": {}}),
            // `"<max>"` stands for the longest value `Description` admits, built at run
            // time rather than written out here.
            Patch::Value("<max>") => json!({"description": {"optionalValue": longest}}),
            Patch::Value(value) => json!({"description": {"optionalValue": value}}),
        }
    }

    /// The description the engine holds after this form was applied to an engine holding
    /// `before`.
    fn expected(self, before: Option<&str>, longest: &str) -> Option<String> {
        match self {
            Patch::NoWrapper => before.map(str::to_string),
            Patch::EmptyWrapper => None,
            Patch::Value("<max>") => Some(longest.to_string()),
            Patch::Value(value) => Some(value.to_string()),
        }
    }
}

/// One row of the description table.
struct Row {
    case: &'static str,
    /// The engine's name (`PolicyEngineName` is `^[A-Za-z][A-Za-z0-9_]*$`, 1..48).
    name: &'static str,
    /// The description the create stored, if any.
    before: Option<&'static str>,
    /// Whether the create also named a KMS key, so a row proves the key survives the update.
    with_key: bool,
    patch: Patch,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an engine with no description takes one",
            name: "Describe_me",
            before: None,
            with_key: false,
            patch: Patch::Value("governs the orders gateway"),
        },
        Row {
            case: "a description is replaced by another",
            name: "Replace_me",
            before: Some("the first description"),
            with_key: true,
            patch: Patch::Value("the second description"),
        },
        Row {
            case: "the wrapper with no optionalValue unsets the description",
            name: "Clear_me",
            before: Some("about to be cleared"),
            with_key: false,
            patch: Patch::EmptyWrapper,
        },
        Row {
            case: "an engine with no description is left without one by the empty wrapper",
            name: "A",
            before: None,
            with_key: false,
            patch: Patch::EmptyWrapper,
        },
        Row {
            case: "no wrapper at all leaves the description it had",
            name: "Leave_me_alone",
            before: Some("left exactly as it was"),
            with_key: true,
            patch: Patch::NoWrapper,
        },
        Row {
            case: "no wrapper at all leaves an engine without a description",
            name: "Engine9_Mixed_Case",
            before: None,
            with_key: false,
            patch: Patch::NoWrapper,
        },
        Row {
            case: "the shortest Description the model admits",
            name: "Shortest_description",
            before: Some("longer than one character"),
            with_key: false,
            patch: Patch::Value("x"),
        },
        Row {
            case: "the longest Description the model admits",
            name: "Longest_description",
            before: Some("x"),
            with_key: false,
            patch: Patch::Value("<max>"),
        },
    ]
}

impl Row {
    /// The `CreatePolicyEngine` body this row starts from.
    fn create_body(&self) -> Value {
        let mut body = json!({"name": self.name});
        if let Some(description) = self.before {
            body["description"] = json!(description);
        }
        if self.with_key {
            body["encryptionKeyArn"] = json!(AWS_KEY_ARN);
        }
        body
    }
}

/// The wrapper decides what happens to the description, every other member survives the
/// call, the engine reports `UPDATING`, and the change reads back and outlives the
/// clock-owed transition to `ACTIVE`.
#[tokio::test]
async fn the_wrapper_decides_what_happens_to_the_description() {
    let longest = "d".repeat(MAX_DESCRIPTION_LENGTH);
    let longest = longest.as_str();
    for row in rows() {
        let router = router();
        let policy_engine_id = active_engine(&router, &row.create_body(), row.case).await;
        let before = get_policy_engine(&router, &policy_engine_id).await;

        let updated =
            update_policy_engine(&router, &policy_engine_id, &row.patch.body(longest)).await;
        assert_eq!(
            updated.status, 202,
            "case {:?}: expected HTTP 202 from UpdatePolicyEngine, got {} ({})",
            row.case, updated.status, updated.body
        );
        let expected = row.patch.expected(row.before, longest);
        assert_eq!(
            updated.body.get("description").and_then(Value::as_str).map(str::to_string),
            expected,
            "case {:?}: expected the description {expected:?}, got {}",
            row.case,
            updated.body
        );
        assert_eq!(
            text(&updated.body, "status"),
            "UPDATING",
            "case {:?}: an accepted change reports UPDATING, got {}",
            row.case,
            updated.body
        );
        assert_eq!(
            updated.body.get("statusReasons").cloned(),
            Some(json!([])),
            "case {:?}: nothing failed, so the status carries no reason, got {}",
            row.case,
            updated.body
        );
        for member in ["createdAt", "updatedAt"] {
            assert!(
                is_date_time(updated.body.get(member)),
                "case {:?}: {member} is an RFC 3339 string on this wire, got {}",
                row.case,
                updated.body
            );
        }
        // Only the description is modifiable by this operation: the identity the create
        // minted, the name, the key and the creation timestamp are reported unchanged.
        for member in ["policyEngineId", "name", "policyEngineArn", "createdAt", "encryptionKeyArn"] {
            assert_eq!(
                updated.body.get(member),
                before.body.get(member),
                "case {:?}: the update leaves the {member} the create stored, got {}",
                row.case,
                updated.body
            );
        }
        assert_eq!(
            text(&updated.body, "policyEngineArn"),
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:policy-engine/{policy_engine_id}"),
            "case {:?}: the ARN is still the one derived from the id, the Region and the \
             account, got {}",
            row.case,
            updated.body
        );
        // The engine was last modified by this call, so the stored `updatedAt` moved while
        // `createdAt` did not.
        assert_ne!(
            updated.body.get("updatedAt"),
            before.body.get("updatedAt"),
            "case {:?}: the update records when the engine was last modified, got {}",
            row.case,
            updated.body
        );

        // The round trip: the read reports the record the update wrote.
        let read = get_policy_engine(&router, &policy_engine_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the engine must read back, got {} ({})",
            row.case, read.status, read.body
        );
        for member in ENGINE_MEMBERS {
            // `status` is read at the instant of the answer, so it is compared through the
            // settled read below rather than between the two envelopes.
            if member == "status" {
                continue;
            }
            assert_eq!(
                read.body.get(member),
                updated.body.get(member),
                "case {:?}: the read reports the {member} the update did, got {}",
                row.case,
                read.body
            );
        }

        // The clock, not the call, moves the engine back to `ACTIVE`, and the change the
        // call applied is still there when it does.
        let settled = poll_until_active(&router, &policy_engine_id, row.case).await;
        assert_eq!(
            settled.body.get("description").and_then(Value::as_str).map(str::to_string),
            expected,
            "case {:?}: the settled engine still holds the description the update applied, \
             got {}",
            row.case,
            settled.body
        );
        assert_eq!(
            settled.body.get("updatedAt"),
            updated.body.get("updatedAt"),
            "case {:?}: settling is not a modification, so updatedAt is the update's, got {}",
            row.case,
            settled.body
        );
    }
}

/// `UpdatePolicyEngine` is `@idempotent`: the same change applied again to the settled
/// engine leaves it holding exactly what the first call stored.
#[tokio::test]
async fn the_same_change_applied_again_stores_the_same_engine() {
    let patches = [
        ("a replacement value", json!({"description": {"optionalValue": "repeated"}}), Some("repeated")),
        ("the empty wrapper", json!({"description": {}}), None),
        ("no wrapper at all", json!({}), Some("as created")),
    ];
    for (case, body, expected) in patches {
        let router = router();
        let policy_engine_id = active_engine(
            &router,
            &json!({"name": "Repeated_change", "description": "as created"}),
            case,
        )
        .await;

        let first = update_policy_engine(&router, &policy_engine_id, &body).await;
        assert_eq!(
            first.status, 202,
            "case {case:?}: the first change is accepted, got {} ({})",
            first.status, first.body
        );
        poll_until_active(&router, &policy_engine_id, case).await;

        let second = update_policy_engine(&router, &policy_engine_id, &body).await;
        assert_eq!(
            second.status, 202,
            "case {case:?}: the repeat is accepted once the engine is ACTIVE again, got {} \
             ({})",
            second.status, second.body
        );
        assert_eq!(
            second.body.get("description").and_then(Value::as_str),
            expected,
            "case {case:?}: the repeat stores the same description, got {}",
            second.body
        );
        for member in ["policyEngineId", "name", "policyEngineArn", "createdAt"] {
            assert_eq!(
                second.body.get(member),
                first.body.get(member),
                "case {case:?}: the repeat reports the {member} the first call did, got {}",
                second.body
            );
        }
    }
}

/// An engine whose own asynchronous work is still under way refuses a further edit the
/// modelled conflict, in this plane's own sentence, and keeps the description it had — so
/// the refused call changed nothing.
#[tokio::test]
async fn an_engine_that_is_not_active_refuses_a_further_edit() {
    // (case, the body of the edit that is refused)
    let edits = [
        ("a replacement value", json!({"description": {"optionalValue": "never applied"}})),
        ("the empty wrapper", json!({"description": {}})),
        ("no wrapper at all", json!({})),
    ];

    for (case, edit) in &edits {
        // While the CREATE is still under way: the engine is `CREATING`, so a caller acting
        // straight after the create meets the conflict.
        let router = router();
        let created = create_policy_engine(
            &router,
            &json!({"name": "Still_creating", "description": "as created"}),
        )
        .await;
        let policy_engine_id = text(&created.body, "policyEngineId");
        assert_eq!(
            text(&created.body, "status"),
            "CREATING",
            "case {case:?}: the create reports CREATING, got {}",
            created.body
        );
        let refused = update_policy_engine(&router, &policy_engine_id, edit).await;
        assert_eq!(
            (refused.status, error_code(&refused.body)),
            (409, "ConflictException".to_string()),
            "case {case:?}/CREATING: expected HTTP 409 ConflictException, got {} {} ({})",
            refused.status,
            error_code(&refused.body),
            refused.body
        );
        assert_eq!(
            text(&refused.body, "message"),
            concurrent_modification_message(&policy_engine_id),
            "case {case:?}/CREATING: the refusal names the engine the path carried and what \
             to wait for, got {}",
            text(&refused.body, "message")
        );
        let unchanged = get_policy_engine(&router, &policy_engine_id).await;
        assert_eq!(
            (unchanged.status, text(&unchanged.body, "description")),
            (200, "as created".to_string()),
            "case {case:?}/CREATING: the refused edit changed nothing, got {} ({})",
            unchanged.status,
            unchanged.body
        );

        // And while an accepted UPDATE is still under way: the engine is `UPDATING`, which
        // is the concurrent edit the sentence is about.
        let settled_id = active_engine(
            &router,
            &json!({"name": "Still_updating", "description": "as created"}),
            case,
        )
        .await;
        let accepted = update_policy_engine(
            &router,
            &settled_id,
            &json!({"description": {"optionalValue": "the edit that won"}}),
        )
        .await;
        assert_eq!(
            (accepted.status, text(&accepted.body, "status")),
            (202, "UPDATING".to_string()),
            "case {case:?}: the first edit is accepted and leaves the engine UPDATING, got \
             {} ({})",
            accepted.status,
            accepted.body
        );
        let second = update_policy_engine(&router, &settled_id, edit).await;
        assert_eq!(
            (
                second.status,
                error_code(&second.body),
                text(&second.body, "message")
            ),
            (
                409,
                "ConflictException".to_string(),
                concurrent_modification_message(&settled_id)
            ),
            "case {case:?}/UPDATING: a concurrent edit is the modelled conflict, got {} ({})",
            second.status,
            second.body
        );
        let won = poll_until_active(&router, &settled_id, case).await;
        assert_eq!(
            text(&won.body, "description"),
            "the edit that won",
            "case {case:?}: the refused edit left the accepted one's description in place, \
             got {}",
            won.body
        );
    }
}

/// An id naming no engine of this account refuses the change with the modelled 404 and the
/// plane's own sentence — and brings no engine into being by being written to.
#[tokio::test]
async fn an_unknown_policy_engine_is_refused() {
    let router = router();
    // Every id satisfies `ResourceId`'s `^[A-Za-z][A-Za-z0-9_]*-[a-z0-9_]{10}$`, so the
    // refusal is the handler's lookup and not the boundary constraint walk: the shortest id
    // the pattern admits, one carrying underscores, one at the top of the 12..59 length
    // range, and one whose suffix is all underscores.
    let unknown = [
        "A-0123456789",
        "Missing_engine-abcdefghij",
        "Engine_with_a_long_but_valid_name_for_account-zz00zz00zz",
        "Underscores-__________",
    ];
    for policy_engine_id in unknown {
        for (case, edit) in [
            ("a replacement value", json!({"description": {"optionalValue": "anything"}})),
            ("the empty wrapper", json!({"description": {}})),
            ("no wrapper at all", json!({})),
        ] {
            let refused = update_policy_engine(&router, policy_engine_id, &edit).await;
            assert_eq!(
                (
                    refused.status,
                    error_code(&refused.body),
                    text(&refused.body, "message")
                ),
                (
                    404,
                    "ResourceNotFoundException".to_string(),
                    not_found_message(policy_engine_id)
                ),
                "case {policy_engine_id:?}/{case}: expected HTTP 404 \
                 ResourceNotFoundException naming the engine the path carried, got {} ({})",
                refused.status,
                refused.body
            );
            assert!(
                refused.body.get("policyEngineArn").is_none(),
                "case {policy_engine_id:?}/{case}: a refused change answers no success \
                 envelope, got {}",
                refused.body
            );
        }
        let read = get_policy_engine(&router, policy_engine_id).await;
        assert_eq!(
            (read.status, text(&read.body, "message")),
            (404, not_found_message(policy_engine_id)),
            "case {policy_engine_id:?}: the refused change left no engine behind, got {} ({})",
            read.status,
            read.body
        );
    }
}
