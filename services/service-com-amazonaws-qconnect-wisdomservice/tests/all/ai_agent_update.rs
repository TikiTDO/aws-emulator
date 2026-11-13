// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateAIAgent`, driven over HTTP through the same router the
//! emulator serves with.
//!
//! The rule under test, asserted over a TABLE of inputs rather than the one recorded
//! request: an update WRITES the working copy of the AI agent the route names — the
//! required member and every optional member the request carries replace the stored
//! values, a member the request carries none of keeps the value it had, and the members
//! the SERVICE owns (the id, the ARN, the name, the type, the origin, the status and the
//! tags) are untouched — and the answer is rendered out of the written record, so a later
//! `GetAIAgent` reports exactly the same values.
//!
//! The rows differ along every axis the rule generalises over:
//!
//! * every `AIAgentType` the model declares, updated with the `AIAgentConfiguration`
//!   union member that type is configured through — a configuration carrying every member
//!   of its shape, and one carrying no member at all, so a replacement is a replacement
//!   rather than a merge;
//! * both values the modelled `VisibilityStatus` declares, in both directions;
//! * `description` at both published length bounds, and omitted — which keeps the stored
//!   one;
//! * `configuration` omitted, which keeps the stored one;
//! * one update, and a second update layered over the first.
//!
//! Beside the write it pins the refusals: a parent assistant this account does not hold,
//! an agent its assistant does not hold, an agent of ANOTHER assistant, a numbered version
//! qualifier (a published snapshot is not the working copy an update writes), a
//! configuration whose union member is not the one the STORED agent's type is configured
//! through, a configuration past the store's entry ceiling, a write to one of the built-in
//! agents the service provisions, and a replayed `clientToken` — answered with the record
//! that token's own write left, or refused when the request behind the token differs.
//! After every refusal the stored record is read back unchanged.
//!
//! Every assertion goes over the wire — only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected — so a response that is not computed from
//! persisted state cannot pass. The harness (the router, the reply reader, the assistant
//! create, the agent create and read, and the configuration bodies) is the one
//! `resource_tags.rs` and `ai_agent.rs` already drive this service with.

use serde_json::{json, Value};

use service_com_amazonaws_qconnect_wisdomservice::state::{
    MAX_AI_AGENT_CONFIGURATION_ENTRIES, MAX_IDEMPOTENCY_TOKENS,
};

use super::ai_agent::{
    assert_not_found, assert_reads_the_record, assistant_id, association_configuration,
    create_agent, expected_agent_arn, expected_read_arn, full_configuration, get_agent,
    DESCRIPTION_MAX, DESCRIPTION_MIN, MAX_RESULTS_MIN, MESSAGE_AI_AGENT_NOT_FOUND,
    MESSAGE_ASSISTANT_NOT_FOUND, MESSAGE_IDEMPOTENCY_CONFLICT, ORIGIN_CUSTOMER, QUALIFIER_LATEST,
    SEARCH_TYPES, STATUS_ACTIVE, TYPE_CONFIGURATIONS, VISIBILITY_PUBLISHED, VISIBILITY_SAVED,
};
use super::resource_tags::{label, tag_map, wisdom_arn, Emulator, Reply, ABSENT_UUID, OTHER_ACCOUNT, REGION};

/// The sentence this endpoint refuses a write to one of the built-in AI agents the service
/// provisions with the assistant with. Written out here rather than imported from the
/// implementation, so the assertion pins the answer a client sees.
const MESSAGE_SYSTEM_AGENT_NOT_MODIFIABLE: &str =
    "A default AI Agent created by Amazon Q in Connect cannot be modified. Create an AI \
     Agent of your own to carry this configuration.";

/// The members of `AIAgentData` an update may write, in the order the rows name them
/// (`smithy_query(query="operation",operation="UpdateAIAgent")`: `visibilityStatus` is
/// required, `configuration` and `description` optional).
const WRITABLE_MEMBERS: [&str; 3] = ["visibilityStatus", "configuration", "description"];

/// The members of `AIAgentData` no update may change: the agent's identity and parentage,
/// the name and type it was created with, and the members the SERVICE owns
/// (`smithy_query(query="shape",shape="AIAgentData")`).
const PRESERVED_MEMBERS: [&str; 9] = [
    "aiAgentId",
    "aiAgentArn",
    "assistantId",
    "assistantArn",
    "name",
    "type",
    "origin",
    "status",
    "tags",
];

/// `UpdateAIAgent` on one agent, as the model's URI binds it
/// (`POST /assistants/{assistantId}/aiagents/{aiAgentId}`).
pub(crate) async fn update_agent(
    emulator: &Emulator,
    assistant: &str,
    agent: &str,
    body: Value,
) -> Reply {
    emulator
        .send(
            "POST",
            &format!(
                "/assistants/{}/aiagents/{}",
                label(assistant),
                label(agent)
            ),
            Some(body),
        )
        .await
}

/// One row of the update table: the agent it creates, and the update bodies it then sends
/// in order (one row may layer a second update over the first).
#[derive(Debug, Clone)]
struct Row {
    label: String,
    create: Value,
    updates: Vec<Value>,
}

/// A create body for `agent_type`, carrying the configuration given plus a description, so
/// a row can assert both what an update replaces and what it keeps.
fn create_body_with(name: &str, agent_type: &str, visibility: &str, configuration: Value) -> Value {
    json!({
        "name": name,
        "type": agent_type,
        "visibilityStatus": visibility,
        "description": "The description the create stored.",
        "configuration": configuration,
        "tags": tag_map(2),
    })
}

/// A create body carrying every member of that type's configuration shape.
fn create_body(name: &str, agent_type: &str, visibility: &str) -> Value {
    create_body_with(name, agent_type, visibility, full_configuration(agent_type))
}

/// The configuration of `agent_type` carrying NO member of its own: the union member alone
/// says which type it configures, which is all such a request claims. Every member of the
/// union is a structure whose own members are optional
/// (`smithy_query(query="shape",shape="AIAgentConfiguration"`), so every type has one.
fn bare_configuration(member: &str) -> Value {
    json!({ member: {} })
}

/// The update bodies the write rule is asserted over. Each row differs from the others
/// along one axis of the rule — the agent type and the configuration member it is
/// configured through, the visibility status it moves to, which optional members the
/// request carries, and how many updates are layered on one record.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // One row per `AIAgentType`: the whole configuration of that type replaced, and the
    // visibility status moved to the other value the model declares — so both directions
    // of the move, and both directions of the configuration replacement (a full
    // configuration replacing a bare one and a bare one replacing a full one, which is
    // what says a replacement is not a merge), are exercised across the catalog.
    for (index, (agent_type, member)) in TYPE_CONFIGURATIONS.iter().enumerate() {
        let (created_with, updated_to) = if index % 2 == 0 {
            (VISIBILITY_SAVED, VISIBILITY_PUBLISHED)
        } else {
            (VISIBILITY_PUBLISHED, VISIBILITY_SAVED)
        };
        let (created_configuration, replacement) = if index % 2 == 0 {
            (full_configuration(agent_type), bare_configuration(member))
        } else {
            (bare_configuration(member), full_configuration(agent_type))
        };
        rows.push(Row {
            label: format!(
                "type {agent_type}: configuration replaced, {created_with} -> {updated_to}"
            ),
            create: create_body_with(
                &format!("update-{agent_type}"),
                agent_type,
                created_with,
                created_configuration,
            ),
            updates: vec![json!({
                "visibilityStatus": updated_to,
                "configuration": replacement,
                "description": "The description the update wrote.",
            })],
        });
    }

    let (agent_type, _) = TYPE_CONFIGURATIONS[0];

    // Only the required member: the stored configuration and description are what the
    // record keeps.
    rows.push(Row {
        label: "the required member alone".to_string(),
        create: create_body("required member alone", agent_type, VISIBILITY_SAVED),
        updates: vec![json!({ "visibilityStatus": VISIBILITY_PUBLISHED })],
    });

    // The description at each published bound of its length.
    for (bound, length) in [
        ("minimum", DESCRIPTION_MIN),
        ("maximum", DESCRIPTION_MAX),
    ] {
        rows.push(Row {
            label: format!("description at its published {bound} length ({length})"),
            create: create_body(
                &format!("description {bound}"),
                agent_type,
                VISIBILITY_PUBLISHED,
            ),
            updates: vec![json!({
                "visibilityStatus": VISIBILITY_SAVED,
                "description": "d".repeat(length),
            })],
        });
    }

    // A description alone, with the configuration left as it was.
    rows.push(Row {
        label: "description only".to_string(),
        create: create_body("description only", agent_type, VISIBILITY_SAVED),
        updates: vec![json!({
            "visibilityStatus": VISIBILITY_SAVED,
            "description": "Only the description moves.",
        })],
    });

    // An update that restates exactly what is stored: the record is written, and reports
    // the same values.
    rows.push(Row {
        label: "an update restating the stored values".to_string(),
        create: create_body("restated", agent_type, VISIBILITY_SAVED),
        updates: vec![json!({
            "visibilityStatus": VISIBILITY_SAVED,
            "configuration": full_configuration(agent_type),
            "description": "The description the create stored.",
        })],
    });

    // Three updates layered on one record: the last write is what the record holds, and
    // each one is applied to the record the previous left.
    rows.push(Row {
        label: "three updates layered on one record".to_string(),
        create: create_body("layered", agent_type, VISIBILITY_SAVED),
        updates: vec![
            json!({
                "visibilityStatus": VISIBILITY_PUBLISHED,
                "description": "The first update.",
            }),
            json!({
                "visibilityStatus": VISIBILITY_SAVED,
                "configuration": bare_configuration(TYPE_CONFIGURATIONS[0].1),
            }),
            json!({
                "visibilityStatus": VISIBILITY_PUBLISHED,
                "description": "The third update.",
            }),
        ],
    });

    rows
}

/// The `AIAgentData` an update must answer with: every member it carried replaces the
/// stored value, and every other member of the record is the one the previous answer
/// reported.
fn assert_applies_update(case: &str, what: &str, stored: &Value, update: &Value, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: {what} must be served, got HTTP {} with body {}",
        reply.status, reply.body
    );
    let got = reply
        .field("/aiAgent")
        .unwrap_or_else(|| panic!("case {case:?}: {what} answered no aiAgent: {}", reply.body));

    for member in WRITABLE_MEMBERS {
        let expected = update.get(member).or_else(|| stored.get(member));
        let actual = got.get(member);
        assert_eq!(
            actual, expected,
            "case {case:?}: {what} must report the {member:?} the update carried, or the \
             stored one when it carried none, expected {expected:?}, got {actual:?}"
        );
    }
    for member in PRESERVED_MEMBERS {
        let expected = stored.get(member);
        let actual = got.get(member);
        assert_eq!(
            actual, expected,
            "case {case:?}: {what} must leave {member:?} as it was, expected {expected:?}, \
             got {actual:?}"
        );
    }

    // The write is what `modifiedTime` reports, so it never moves backwards.
    let was = stored
        .get("modifiedTime")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let now = got.get("modifiedTime").and_then(Value::as_f64);
    assert!(
        now.is_some_and(|now| now >= was),
        "case {case:?}: {what} must report when the record was last written, expected at \
         least {was}, got {now:?}"
    );
}

/// The record the update left, read back through `GetAIAgent` however it is addressed.
/// A read answers with that record, with the one member a read renders differently: the
/// ARN it reports names the version it answered for.
async fn assert_reads_back(
    emulator: &Emulator,
    case: &str,
    assistant: &str,
    agent_id: &str,
    written: &Value,
) {
    let mut expected = written.clone();
    expected["aiAgentArn"] = json!(expected_read_arn(assistant, agent_id, QUALIFIER_LATEST));
    for addressed_by in [
        agent_id.to_string(),
        expected_agent_arn(assistant, agent_id),
        format!("{agent_id}:{QUALIFIER_LATEST}"),
        expected_read_arn(assistant, agent_id, QUALIFIER_LATEST),
    ] {
        let reply = get_agent(emulator, assistant, &addressed_by).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a read addressed by {addressed_by:?} must be served, got HTTP {} \
             with body {}",
            reply.status, reply.body
        );
        let read = reply.field("/aiAgent");
        assert_eq!(
            read,
            Some(&expected),
            "case {case:?}: a read addressed by {addressed_by:?} must answer with the record \
             the update wrote, expected {expected}, got {read:?}"
        );
        // The model: a version number is reported only when the request's `aiAgentId`
        // named a VERSION. An update writes the working copy and publishes nothing, so a
        // read of it still names no version.
        assert!(
            reply.field("/versionNumber").is_none(),
            "case {case:?}: a read of the working copy reports no versionNumber, body {}",
            reply.body
        );
    }

    // An update publishes no version, so a numbered qualifier still names nothing.
    let reply = get_agent(emulator, assistant, &format!("{agent_id}:1")).await;
    assert_not_found(
        case,
        MESSAGE_AI_AGENT_NOT_FOUND,
        &format!("{agent_id}:1"),
        &reply,
    );
}

#[tokio::test]
async fn an_update_writes_the_working_copy_and_every_later_read_reports_it() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent update").await;

    for row in rows() {
        let created = create_agent(&emulator, &assistant, row.create.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the agent this row updates must be created, got HTTP {} with body {}",
            row.label, created.status, created.body
        );
        let agent_id = created.string("/aiAgent/aiAgentId");
        let mut stored = created
            .field("/aiAgent")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: the create answered no aiAgent", row.label));
        assert_eq!(
            stored.get("origin").and_then(Value::as_str),
            Some(ORIGIN_CUSTOMER),
            "case {:?}: this row updates an agent the caller created, body {}",
            row.label,
            created.body
        );

        for (index, update) in row.updates.iter().enumerate() {
            let case = format!("{} (update {})", row.label, index + 1);
            let reply = update_agent(&emulator, &assistant, &agent_id, update.clone()).await;
            assert_applies_update(&case, "the update", &stored, update, &reply);
            let written = reply
                .field("/aiAgent")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: the update answered no aiAgent"));
            assert_reads_back(&emulator, &case, &assistant, &agent_id, &written).await;
            // The next update of this row is applied to the record this one left.
            stored = written;
        }

        // An agent this row never touched is unaffected: one row's write is never
        // another's. The agent created first in the loop is re-read through the record the
        // row itself holds, so this is asserted by the read-back above; what is asserted
        // here is that the agent still reports the status and the type it was created
        // with, whatever the updates carried.
        assert_eq!(
            stored.get("status").and_then(Value::as_str),
            Some(STATUS_ACTIVE),
            "case {:?}: an updated agent is still in service, record {stored}",
            row.label
        );
    }
}

#[tokio::test]
async fn an_update_never_reaches_another_assistants_agent_or_a_version() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent update misses").await;
    let other_assistant = assistant_id(&emulator, "ai agent update misses, other parent").await;
    let (agent_type, _) = TYPE_CONFIGURATIONS[0];
    let created = create_agent(
        &emulator,
        &assistant,
        create_body("held by one assistant", agent_type, VISIBILITY_SAVED),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the agent this test updates must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    let agent_id = created.string("/aiAgent/aiAgentId");
    let stored = created
        .field("/aiAgent")
        .cloned()
        .unwrap_or_else(|| panic!("the create answered no aiAgent: {}", created.body));
    let body = json!({
        "visibilityStatus": VISIBILITY_PUBLISHED,
        "description": "This write must not land.",
    });

    // A parent assistant that is not there.
    for (case, parent) in [
        ("an assistant id nothing holds", ABSENT_UUID.to_string()),
        (
            "an assistant ARN of another account",
            wisdom_arn(
                "aws",
                REGION,
                OTHER_ACCOUNT,
                &format!("assistant/{assistant}"),
            ),
        ),
    ] {
        let reply = update_agent(&emulator, &parent, &agent_id, body.clone()).await;
        assert_not_found(case, MESSAGE_ASSISTANT_NOT_FOUND, &parent, &reply);
    }

    // An agent that is not there, however it is addressed — including a NUMBERED version
    // qualifier, which names a published snapshot rather than the working copy an update
    // writes.
    for (case, addressed_by, parent) in [
        (
            "an agent id nothing holds",
            ABSENT_UUID.to_string(),
            assistant.clone(),
        ),
        (
            "an agent of another assistant",
            agent_id.clone(),
            other_assistant.clone(),
        ),
        (
            "an agent ARN under another assistant as its parent",
            expected_agent_arn(&assistant, &agent_id),
            other_assistant.clone(),
        ),
        (
            "a numbered version qualifier",
            format!("{agent_id}:1"),
            assistant.clone(),
        ),
    ] {
        let reply = update_agent(&emulator, &parent, &addressed_by, body.clone()).await;
        assert_not_found(case, MESSAGE_AI_AGENT_NOT_FOUND, &addressed_by, &reply);
    }

    // Nothing was written: the record is the one the create stored.
    let reply = get_agent(&emulator, &assistant, &agent_id).await;
    assert_reads_the_record(
        "after every refusal",
        "a read of the record the create stored",
        &stored,
        &reply,
    );
}

#[tokio::test]
async fn a_configuration_for_another_type_than_the_stored_agent_is_refused() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent update mismatch").await;

    // Every type, each updated with the configuration of the NEXT type in the catalog, so
    // every member of the union is tried against an agent it does not configure. The
    // request carries no `type`, so the pair the refusal is about is the configuration and
    // the type the STORED agent was created with.
    for (index, (agent_type, _)) in TYPE_CONFIGURATIONS.iter().enumerate() {
        let (other_type, other_member) =
            TYPE_CONFIGURATIONS[(index + 1) % TYPE_CONFIGURATIONS.len()];
        let created = create_agent(
            &emulator,
            &assistant,
            create_body(
                &format!("mismatch-{agent_type}"),
                agent_type,
                VISIBILITY_SAVED,
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {agent_type:?}: the agent must be created, got HTTP {} with body {}",
            created.status, created.body
        );
        let agent_id = created.string("/aiAgent/aiAgentId");
        let stored = created
            .field("/aiAgent")
            .cloned()
            .unwrap_or_else(|| panic!("case {agent_type:?}: the create answered no aiAgent"));

        let case = format!("an agent of type {agent_type} updated with the {other_member} of {other_type}");
        let reply = update_agent(
            &emulator,
            &assistant,
            &agent_id,
            json!({
                "visibilityStatus": VISIBILITY_PUBLISHED,
                "configuration": full_configuration(other_type),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("ValidationException"),
            "case {case:?}: expected the modelled ValidationException, got x-amzn-errortype \
             {:?} with body {}",
            reply.error_type,
            reply.body
        );
        assert!(
            reply.string("/message").contains(agent_type),
            "case {case:?}: the refusal must state the rule for the type the stored agent \
             has, body {}",
            reply.body
        );

        // The refusal wrote nothing: neither the configuration it carried nor the
        // visibility status it asked for.
        let read = get_agent(&emulator, &assistant, &agent_id).await;
        assert_reads_the_record(&case, "a read after a refused update", &stored, &read);

        // The configuration of the agent's OWN type, which the same table otherwise
        // refuses, is written: the refusal is about the PAIR, not about the configuration.
        let reply = update_agent(
            &emulator,
            &assistant,
            &agent_id,
            json!({
                "visibilityStatus": VISIBILITY_PUBLISHED,
                "configuration": full_configuration(agent_type),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {agent_type:?}: a configuration of the agent's own type must be written, \
             got HTTP {} with body {}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn an_update_is_bounded_by_the_stores_configuration_entry_ceiling() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent update ceiling").await;
    let (agent_type, member) = TYPE_CONFIGURATIONS[0];
    let created = create_agent(
        &emulator,
        &assistant,
        create_body("ceiling", agent_type, VISIBILITY_SAVED),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the agent this test updates must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    let agent_id = created.string("/aiAgent/aiAgentId");

    // A configuration whose one open-ended list carries `entries` association
    // configurations: the list the model puts no maximum on, so it is what the store's
    // ceiling is about.
    let body = |entries: usize| {
        let associations: Vec<Value> = (0..entries)
            .map(|index| {
                association_configuration(
                    index,
                    SEARCH_TYPES[index % SEARCH_TYPES.len()],
                    MAX_RESULTS_MIN,
                    json!({ "tagCondition": { "key": format!("key-{index}") } }),
                )
            })
            .collect();
        json!({
            "visibilityStatus": VISIBILITY_SAVED,
            "configuration": { member: { "associationConfigurations": associations } },
        })
    };

    // Exactly at the ceiling: written, and read back out of the record.
    let at_ceiling = update_agent(
        &emulator,
        &assistant,
        &agent_id,
        body(MAX_AI_AGENT_CONFIGURATION_ENTRIES),
    )
    .await;
    assert_eq!(
        at_ceiling.status, 200,
        "a configuration of exactly {MAX_AI_AGENT_CONFIGURATION_ENTRIES} entries must be \
         written, got HTTP {} with body {}",
        at_ceiling.status, at_ceiling.body
    );
    let written = at_ceiling
        .field("/aiAgent")
        .cloned()
        .unwrap_or_else(|| panic!("the update answered no aiAgent: {}", at_ceiling.body));
    let read = get_agent(&emulator, &assistant, &agent_id).await;
    assert_reads_the_record(
        "a configuration at the ceiling",
        "a read of the configuration the update wrote",
        &written,
        &read,
    );

    // One entry past it: refused, and nothing written.
    let refused = update_agent(
        &emulator,
        &assistant,
        &agent_id,
        body(MAX_AI_AGENT_CONFIGURATION_ENTRIES + 1),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "a configuration past the ceiling is refused, got HTTP {} with body {}",
        refused.status, refused.body
    );
    assert!(
        refused.error_type.contains("ValidationException"),
        "the refusal must carry the modelled error code, got {:?}",
        refused.error_type
    );
    assert!(
        refused
            .string("/message")
            .contains(&MAX_AI_AGENT_CONFIGURATION_ENTRIES.to_string()),
        "the refusal must state the ceiling it enforces, body {}",
        refused.body
    );
    let read = get_agent(&emulator, &assistant, &agent_id).await;
    assert_reads_the_record(
        "a configuration past the ceiling",
        "a read after the refused update",
        &written,
        &read,
    );
}

#[tokio::test]
async fn a_built_in_agent_the_service_provisioned_is_not_writable() {
    let emulator = Emulator::new();
    let reply = emulator
        .create_assistant("built-in agents are not writable", &tag_map(0))
        .await;
    assert_eq!(
        reply.status, 200,
        "the assistant must be created, got HTTP {} with body {}",
        reply.status, reply.body
    );
    let assistant = reply.string("/assistant/assistantId");
    let bindings = reply
        .field("/assistant/aiAgentConfiguration")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "the assistant reported no aiAgentConfiguration: {}",
                reply.body
            )
        });
    assert!(
        !bindings.is_empty(),
        "a fresh assistant is provisioned with built-in AI agents, body {}",
        reply.body
    );

    // Every built-in agent the assistant was provisioned with, whichever type it serves.
    for (agent_type, binding) in &bindings {
        let bound = binding
            .pointer("/aiAgentId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let agent_id = bound.split(':').next().unwrap_or_default().to_string();
        let before = get_agent(&emulator, &assistant, &agent_id).await;
        let stored = before.field("/aiAgent").cloned().unwrap_or_else(|| {
            panic!("case {agent_type:?}: the built-in agent must be readable: {}", before.body)
        });

        let reply = update_agent(
            &emulator,
            &assistant,
            &agent_id,
            json!({
                "visibilityStatus": VISIBILITY_SAVED,
                "description": "A write a caller does not own.",
            }),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {agent_type:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("ValidationException"),
            "case {agent_type:?}: expected the modelled ValidationException, got \
             x-amzn-errortype {:?} with body {}",
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message"),
            MESSAGE_SYSTEM_AGENT_NOT_MODIFIABLE,
            "case {agent_type:?}: expected the sentence this endpoint words that class with, \
             body {}",
            reply.body
        );

        let after = get_agent(&emulator, &assistant, &agent_id).await;
        assert_eq!(
            after.field("/aiAgent"),
            Some(&stored),
            "case {agent_type:?}: the refused write must leave the built-in agent as it was, \
             expected {stored}, got body {}",
            after.body
        );
    }
}

#[tokio::test]
async fn a_replayed_client_token_answers_with_the_record_it_already_wrote() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent update idempotency").await;

    for (index, (agent_type, _)) in TYPE_CONFIGURATIONS.iter().enumerate() {
        let created = create_agent(
            &emulator,
            &assistant,
            create_body(
                &format!("idempotent-{agent_type}"),
                agent_type,
                VISIBILITY_SAVED,
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {agent_type:?}: the agent must be created, got HTTP {} with body {}",
            created.status, created.body
        );
        let agent_id = created.string("/aiAgent/aiAgentId");

        let token = format!("token-{index}");
        let body = json!({
            "visibilityStatus": VISIBILITY_PUBLISHED,
            "description": "The update behind the token.",
            "clientToken": token,
        });
        let first = update_agent(&emulator, &assistant, &agent_id, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {agent_type:?}: the first update must be served, got HTTP {} with body {}",
            first.status, first.body
        );
        let replay = update_agent(&emulator, &assistant, &agent_id, body.clone()).await;
        assert_eq!(
            replay.field("/aiAgent"),
            first.field("/aiAgent"),
            "case {agent_type:?}: a replayed clientToken must answer with the record it \
             already wrote, expected {:?}, got {:?}",
            first.field("/aiAgent"),
            replay.field("/aiAgent")
        );

        // The same token behind a DIFFERENT request is the modelled idempotency conflict
        // rather than a write the caller did not ask for.
        let mut differing = body.clone();
        differing["visibilityStatus"] = json!(VISIBILITY_SAVED);
        let reply = update_agent(&emulator, &assistant, &agent_id, differing.clone()).await;
        assert_eq!(
            reply.status, 409,
            "case {agent_type:?}: expected HTTP 409 for a token carrying another request, got \
             {} with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("ConflictException"),
            "case {agent_type:?}: expected the modelled ConflictException, got x-amzn-errortype \
             {:?} with body {}",
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message"),
            MESSAGE_IDEMPOTENCY_CONFLICT,
            "case {agent_type:?}: expected the sentence this endpoint words that class with, \
             body {}",
            reply.body
        );
        let read = get_agent(&emulator, &assistant, &agent_id).await;
        assert_reads_the_record(
            agent_type,
            "a read after the refused replay",
            first
                .field("/aiAgent")
                .unwrap_or_else(|| panic!("case {agent_type:?}: the first update answered no \
                     aiAgent: {}", first.body)),
            &read,
        );

        // The same request under a token of its own is written.
        differing["clientToken"] = json!(format!("{token}-other"));
        let reply = update_agent(&emulator, &assistant, &agent_id, differing).await;
        assert_eq!(
            reply.status, 200,
            "case {agent_type:?}: a request under a token of its own must be served, got HTTP \
             {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.string("/aiAgent/visibilityStatus"),
            VISIBILITY_SAVED,
            "case {agent_type:?}: that write must be the one the record holds, body {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn the_tokens_the_store_remembers_are_bounded_to_the_most_recent_ones() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent update token ceiling").await;
    let (agent_type, _) = TYPE_CONFIGURATIONS[0];
    let created = create_agent(
        &emulator,
        &assistant,
        create_body("token ceiling", agent_type, VISIBILITY_SAVED),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the agent this test updates must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    let agent_id = created.string("/aiAgent/aiAgentId");

    // One update per token, each carrying the SAME request, so the record is the same
    // whichever token wrote it and the only thing the loop varies is how many tokens the
    // store has been asked to remember: past its ceiling it keeps the most recent ones.
    let token = |index: usize| format!("ceiling-token-{index}");
    let sent = MAX_IDEMPOTENCY_TOKENS + 8;
    for index in 0..sent {
        let reply = update_agent(
            &emulator,
            &assistant,
            &agent_id,
            json!({
                "visibilityStatus": VISIBILITY_SAVED,
                "description": "The request every token behind this loop carries.",
                "clientToken": token(index),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: the update must be served, got HTTP {} with body {}",
            token(index),
            reply.status,
            reply.body
        );
    }

    // A DIFFERENT request under a token the store still holds — one of the most recent —
    // is the modelled idempotency conflict, and writes nothing.
    let differing = |carrying: String| {
        json!({
            "visibilityStatus": VISIBILITY_PUBLISHED,
            "description": "A different request under an old token.",
            "clientToken": carrying,
        })
    };
    let recent = token(sent - 1);
    let reply = update_agent(&emulator, &assistant, &agent_id, differing(recent.clone())).await;
    assert_eq!(
        reply.status, 409,
        "case {recent:?}: a token among the most recent {MAX_IDEMPOTENCY_TOKENS} is still \
         remembered, so another request under it conflicts, got HTTP {} with body {}",
        reply.status, reply.body
    );
    assert!(
        reply.error_type.contains("ConflictException"),
        "case {recent:?}: expected the modelled ConflictException, got x-amzn-errortype {:?} \
         with body {}",
        reply.error_type,
        reply.body
    );

    // The same request under the FIRST token, which the ceiling forgot long ago, is the
    // fresh request it now is: the store's memory is bounded, not its service.
    let forgotten = token(0);
    let reply = update_agent(
        &emulator,
        &assistant,
        &agent_id,
        differing(forgotten.clone()),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {forgotten:?}: a token evicted by the ceiling is no longer remembered, so the \
         request under it is served, got HTTP {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("/aiAgent/visibilityStatus"),
        VISIBILITY_PUBLISHED,
        "case {forgotten:?}: that write must be the one the record holds, body {}",
        reply.body
    );
    let read = get_agent(&emulator, &assistant, &agent_id).await;
    assert_reads_the_record(
        &forgotten,
        "a read after the write under the evicted token",
        reply
            .field("/aiAgent")
            .unwrap_or_else(|| panic!("case {forgotten:?}: that write answered no aiAgent: {}", reply.body)),
        &read,
    );
}
