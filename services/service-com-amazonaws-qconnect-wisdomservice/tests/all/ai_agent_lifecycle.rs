// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Q in Connect AI-agent LIFECYCLE as `GetAIAgent`
//! reports it: what a read answers after the record has been written, and after it has
//! been removed.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than the one
//! recorded request:
//!
//! 1. A read of a resource of a VERSIONED family, reached through a label that may carry
//!    a version qualifier, reports that resource's ARN with the qualifier of the version
//!    addressed appended — the working copy's floating alias where the label named no
//!    version — while an operation whose label addresses no version at all (the create,
//!    the update) reports the bare resource ARN.
//! 2. A read reports every member as LAST WRITTEN, so what a mutation stored is what the
//!    next read answers with, not the record as the create left it.
//! 3. A delete forgets the record, so every later read of it — by any label form that
//!    named it — is the modelled `ResourceNotFoundException` (HTTP 404), while a sibling
//!    the delete did not name reads back unchanged.
//!
//! The rows differ along the axes each rule generalises over: the `AIAgentType` and the
//! `AIAgentConfiguration` union member it is configured through (the whole catalog), both
//! values of `VisibilityStatus` and both directions of the move between them, which
//! optional members a mutation carries, the published length bounds of `description`, the
//! label form the request addresses the agent by, and how many agents the assistant
//! holds. The refusals are pinned too: an assistant this account does not hold, an agent
//! of ANOTHER assistant, a numbered version qualifier (nothing publishes a version yet),
//! and one of the built-in agents the service provisions the assistant with.
//!
//! Every assertion goes over the wire — only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected — so a response that is not computed from
//! persisted state cannot pass.

use serde_json::{json, Value};

use super::ai_agent::{
    assert_not_found, assistant_id, create_agent, expected_agent_arn, expected_read_arn,
    full_configuration, get_agent, DESCRIPTION_MAX, DESCRIPTION_MIN, MESSAGE_AI_AGENT_NOT_FOUND,
    MESSAGE_ASSISTANT_NOT_FOUND, ORIGIN_CUSTOMER, QUALIFIER_LATEST, STATUS_ACTIVE,
    TYPE_CONFIGURATIONS, VISIBILITY_PUBLISHED, VISIBILITY_SAVED,
};
use super::ai_agent_update::update_agent;
use super::resource_tags::{label, tag_map, wisdom_arn, Emulator, Reply, ABSENT_UUID, REGION};

/// The sentence this endpoint refuses a delete addressed at one of the built-in AI agents
/// the service provisions with the assistant with. Written out here rather than imported
/// from the implementation, so the assertion pins the answer a client sees.
const MESSAGE_SYSTEM_AGENT_NOT_DELETABLE: &str =
    "A default AI Agent created by Amazon Q in Connect cannot be deleted.";

/// Two of the built-in agents the service provisions every assistant with: the id under
/// which the `ANSWER_RECOMMENDATION` agent and the `MANUAL_SEARCH` agent are reachable
/// (`GetAIAgent` on a fresh assistant reports them with `origin` `SYSTEM`). One is bound
/// by the floating alias and one by a pinned version, so the refusal is asserted on both
/// kinds of service-owned record.
const BUILT_IN_AGENT_IDS: [(&str, &str); 2] = [
    ("ANSWER_RECOMMENDATION", "3025a3fa-5419-4c9c-a040-877213b93f70"),
    ("MANUAL_SEARCH", "652f0287-1610-4fee-bbe3-c687c13f5ec3"),
];

/// One string member of a request body, so a row can state what a read must report
/// without unwrapping a `Value` at every assertion.
fn member_string(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `DeleteAIAgent` on one agent, as the model's URI binds it
/// (`DELETE /assistants/{assistantId}/aiagents/{aiAgentId}`).
async fn delete_agent(emulator: &Emulator, assistant: &str, agent: &str) -> Reply {
    emulator
        .send(
            "DELETE",
            &format!("/assistants/{}/aiagents/{}", label(assistant), label(agent)),
            None,
        )
        .await
}

/// Every label form the modelled `UuidOrArnOrEitherWithQualifier` admits for the WORKING
/// COPY of one agent: the bare id, the resource ARN, and either carrying the qualifier
/// that names the working copy.
fn label_forms(assistant: &str, agent_id: &str) -> Vec<String> {
    vec![
        agent_id.to_string(),
        expected_agent_arn(assistant, agent_id),
        format!("{agent_id}:{QUALIFIER_LATEST}"),
        expected_read_arn(assistant, agent_id, QUALIFIER_LATEST),
    ]
}

/// A create body for `agent_type` carrying a description of `description_len` characters.
fn create_body(name: &str, agent_type: &str, visibility: &str, description_len: usize) -> Value {
    json!({
        "name": name,
        "type": agent_type,
        "visibilityStatus": visibility,
        "description": "d".repeat(description_len),
        "configuration": full_configuration(agent_type),
        "tags": tag_map(1),
    })
}

/// One row of the lifecycle table: an agent of one type, created at one visibility status
/// with a description of one published bound length, then mutated by one update body.
#[derive(Debug, Clone)]
struct Row {
    label: String,
    create: Value,
    update: Value,
}

/// The rows the rules are asserted over: one per `AIAgentType` the model declares, each
/// differing from its neighbours along the visibility status it is created at and moves
/// to, the `description` length it carries (the published minimum and maximum), and which
/// optional members its update carries.
fn rows() -> Vec<Row> {
    TYPE_CONFIGURATIONS
        .iter()
        .enumerate()
        .map(|(index, (agent_type, _))| {
            let (created_with, updated_to) = if index % 2 == 0 {
                (VISIBILITY_SAVED, VISIBILITY_PUBLISHED)
            } else {
                (VISIBILITY_PUBLISHED, VISIBILITY_SAVED)
            };
            // The published length bounds of `description`
            // (`smithy_query(query="operation",operation="CreateAIAgent")`: `len 1..255`),
            // alternating so both ends of the range are created and written.
            let (created_len, updated_len) = if index % 2 == 0 {
                (DESCRIPTION_MIN, DESCRIPTION_MAX)
            } else {
                (DESCRIPTION_MAX, DESCRIPTION_MIN)
            };
            // Which optional members the update carries: the required status alone on
            // every third row, the status plus a description otherwise.
            let mut update = json!({ "visibilityStatus": updated_to });
            if index % 3 != 0 {
                update["description"] = json!("u".repeat(updated_len));
            }
            Row {
                label: format!(
                    "type {agent_type}: created {created_with} with a {created_len}-character \
                     description, updated to {updated_to}"
                ),
                create: create_body(
                    &format!("lifecycle-{agent_type}"),
                    agent_type,
                    created_with,
                    created_len,
                ),
                update,
            }
        })
        .collect()
}

#[tokio::test]
async fn a_read_reports_the_arn_of_the_version_it_addressed() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent read arn").await;

    for row in rows() {
        let created = create_agent(&emulator, &assistant, row.create.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the create must be served, got HTTP {} with body {}",
            row.label, created.status, created.body
        );
        let agent_id = created.string("/aiAgent/aiAgentId");
        // The create addresses no version, so it reports the bare resource ARN.
        let bare = expected_agent_arn(&assistant, &agent_id);
        let created_arn = created.string("/aiAgent/aiAgentArn");
        assert_eq!(
            created_arn, bare,
            "case {:?}: a create addresses no version, so it reports the bare agent ARN, \
             expected {bare}, got {created_arn:?}",
            row.label
        );

        // Every label form that names the working copy answers for the same version, so
        // every one reports the ARN with that version's qualifier.
        let qualified = expected_read_arn(&assistant, &agent_id, QUALIFIER_LATEST);
        for addressed_by in label_forms(&assistant, &agent_id) {
            let reply = get_agent(&emulator, &assistant, &addressed_by).await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: a read addressed by {addressed_by:?} must be served, got HTTP {} \
                 with body {}",
                row.label, reply.status, reply.body
            );
            let got = reply.string("/aiAgent/aiAgentArn");
            assert_eq!(
                got, qualified,
                "case {:?}: a read addressed by {addressed_by:?} must report the ARN of the \
                 version it answered for, expected {qualified}, got {got:?}",
                row.label
            );
            // The qualifier is the only member the read renders differently: the id, the
            // parentage and the members the service owns are the ones the create stored.
            for member in ["aiAgentId", "assistantId", "assistantArn", "name", "type"] {
                let expected = created.field(&format!("/aiAgent/{member}"));
                let actual = reply.field(&format!("/aiAgent/{member}"));
                assert_eq!(
                    actual, expected,
                    "case {:?}: a read addressed by {addressed_by:?} must report the \
                     {member:?} the create stored, expected {expected:?}, got {actual:?}",
                    row.label
                );
            }
            assert_eq!(
                reply.string("/aiAgent/origin"),
                ORIGIN_CUSTOMER,
                "case {:?}: an agent a caller created is CUSTOMER-origin, body {}",
                row.label,
                reply.body
            );
            assert_eq!(
                reply.string("/aiAgent/status"),
                STATUS_ACTIVE,
                "case {:?}: a created agent is in service, body {}",
                row.label,
                reply.body
            );
            // The model: the version number is reported only when the request's
            // `aiAgentId` named a published VERSION, which none of these did.
            assert!(
                reply.field("/versionNumber").is_none(),
                "case {:?}: a read of the working copy reports no versionNumber, body {}",
                row.label,
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_read_reports_every_member_as_last_written() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent read last written").await;

    for row in rows() {
        let created = create_agent(&emulator, &assistant, row.create.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the create must be served, got HTTP {} with body {}",
            row.label, created.status, created.body
        );
        let agent_id = created.string("/aiAgent/aiAgentId");

        let updated = update_agent(&emulator, &assistant, &agent_id, row.update.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: the update must be served, got HTTP {} with body {}",
            row.label, updated.status, updated.body
        );
        // What the mutation stored, member by member: the value it carried where it
        // carried one, and the created value where it carried none.
        let expected_visibility = member_string(&row.update, "visibilityStatus");
        let expected_description = row
            .update
            .get("description")
            .or_else(|| row.create.get("description"))
            .cloned();

        for addressed_by in label_forms(&assistant, &agent_id) {
            let reply = get_agent(&emulator, &assistant, &addressed_by).await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: a read addressed by {addressed_by:?} must be served, got HTTP {} \
                 with body {}",
                row.label, reply.status, reply.body
            );
            let visibility = reply.string("/aiAgent/visibilityStatus");
            assert_eq!(
                visibility, expected_visibility,
                "case {:?}: a read addressed by {addressed_by:?} must report the \
                 visibilityStatus the mutation wrote, expected {expected_visibility:?}, got \
                 {visibility:?}",
                row.label
            );
            let description = reply.field("/aiAgent/description").cloned();
            assert_eq!(
                description, expected_description,
                "case {:?}: a read addressed by {addressed_by:?} must report the description \
                 as last written, expected {expected_description:?}, got {description:?}",
                row.label
            );
            // The whole record the mutation answered with is what the read answers with —
            // save the ARN, which names the version the read addressed.
            let mut whole = updated
                .field("/aiAgent")
                .cloned()
                .unwrap_or_else(|| panic!("case {:?}: the update answered no aiAgent", row.label));
            whole["aiAgentArn"] = json!(expected_read_arn(
                &assistant,
                &agent_id,
                QUALIFIER_LATEST
            ));
            let read = reply.field("/aiAgent");
            assert_eq!(
                read,
                Some(&whole),
                "case {:?}: a read addressed by {addressed_by:?} must answer with the record \
                 the mutation left, expected {whole}, got {read:?}",
                row.label
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_ai_agent_is_no_longer_found_by_any_read() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent delete").await;

    // Every row's agent is created first, so each delete below runs against an assistant
    // that still holds the others: a delete forgets the one record it named and no more.
    let mut held: Vec<(String, String)> = Vec::new();
    for row in rows() {
        let created = create_agent(&emulator, &assistant, row.create.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the create must be served, got HTTP {} with body {}",
            row.label, created.status, created.body
        );
        held.push((row.label.clone(), created.string("/aiAgent/aiAgentId")));
    }

    // Each row's delete addresses the agent by a different label form, so the rule holds
    // however the request names it.
    let mut index = 0usize;
    while let Some((case, agent_id)) = held.pop() {
        let forms = label_forms(&assistant, &agent_id);
        let addressed_by = forms[index % forms.len()].clone();
        index += 1;

        let reply = delete_agent(&emulator, &assistant, &addressed_by).await;
        assert_eq!(
            reply.status, 204,
            "case {case:?}: a delete addressed by {addressed_by:?} must be served, got HTTP {} \
             with body {}",
            reply.status, reply.body
        );

        // Every label form that named the agent now names nothing: the read misses, with
        // the modelled error and the sentence this endpoint words that class with.
        for form in label_forms(&assistant, &agent_id) {
            let reply = get_agent(&emulator, &assistant, &form).await;
            assert_not_found(&case, MESSAGE_AI_AGENT_NOT_FOUND, &form, &reply);
            assert!(
                reply.field("/aiAgent").is_none(),
                "case {case:?}: a read of the deleted agent addressed by {form:?} must answer \
                 with no aiAgent, body {}",
                reply.body
            );
        }

        // A repeated delete of a record that is gone is the same miss, not a second
        // success.
        let reply = delete_agent(&emulator, &assistant, &agent_id).await;
        assert_not_found(&case, MESSAGE_AI_AGENT_NOT_FOUND, &agent_id, &reply);

        // An update of a record that is gone misses too: the delete removed the record
        // every operation of this family resolves against.
        let reply = update_agent(
            &emulator,
            &assistant,
            &agent_id,
            json!({ "visibilityStatus": VISIBILITY_PUBLISHED }),
        )
        .await;
        assert_not_found(&case, MESSAGE_AI_AGENT_NOT_FOUND, &agent_id, &reply);

        // Every agent still held reads back unchanged.
        for (other, other_id) in &held {
            let reply = get_agent(&emulator, &assistant, other_id).await;
            assert_eq!(
                reply.status, 200,
                "case {other:?}: an agent another delete never named must still read back, got \
                 HTTP {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.string("/aiAgent/aiAgentId"),
                *other_id,
                "case {other:?}: the read must answer with the agent it addressed, body {}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_delete_never_reaches_another_parent_a_version_or_a_built_in_agent() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent delete misses").await;
    let other_assistant = assistant_id(&emulator, "ai agent delete misses, other parent").await;
    let (agent_type, _) = TYPE_CONFIGURATIONS[0];
    let created = create_agent(
        &emulator,
        &assistant,
        create_body("held by one assistant", agent_type, VISIBILITY_SAVED, DESCRIPTION_MIN),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the agent this test addresses must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    let agent_id = created.string("/aiAgent/aiAgentId");

    // A parent assistant this account does not hold: the parent is resolved first, so the
    // refusal names the assistant rather than the agent.
    for (case, parent) in [
        ("an assistant id nothing holds", ABSENT_UUID.to_string()),
        (
            "an assistant ARN naming an id nothing holds",
            wisdom_arn(
                "aws",
                REGION,
                engine::EMULATOR_ACCOUNT_ID,
                &format!("assistant/{ABSENT_UUID}"),
            ),
        ),
    ] {
        let reply = delete_agent(&emulator, &parent, &agent_id).await;
        assert_not_found(case, MESSAGE_ASSISTANT_NOT_FOUND, &parent, &reply);
    }

    // An agent of ANOTHER assistant, and a numbered version qualifier — nothing publishes
    // a version yet, so such a label names no record to remove.
    for (case, addressed_by) in [
        ("an agent of another assistant", agent_id.clone()),
        ("an agent id nothing holds", ABSENT_UUID.to_string()),
    ] {
        let reply = delete_agent(&emulator, &other_assistant, &addressed_by).await;
        assert_not_found(case, MESSAGE_AI_AGENT_NOT_FOUND, &addressed_by, &reply);
    }
    for numbered in ["1", "2", "99"] {
        let addressed_by = format!("{agent_id}:{numbered}");
        let reply = delete_agent(&emulator, &assistant, &addressed_by).await;
        assert_not_found(
            "a numbered version qualifier",
            MESSAGE_AI_AGENT_NOT_FOUND,
            &addressed_by,
            &reply,
        );
    }

    // The built-in agents the service provisions the assistant with are the SERVICE's
    // records, so they are not a caller's to remove — and they are still readable after
    // the refusal.
    for (built_in_type, built_in_id) in BUILT_IN_AGENT_IDS {
        let reply = delete_agent(&emulator, &assistant, built_in_id).await;
        assert_eq!(
            reply.status, 400,
            "case {built_in_type:?}: deleting a built-in agent must be refused, got HTTP {} \
             with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("ValidationException"),
            "case {built_in_type:?}: expected the modelled ValidationException, got \
             x-amzn-errortype {:?} with body {}",
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message"),
            MESSAGE_SYSTEM_AGENT_NOT_DELETABLE,
            "case {built_in_type:?}: expected the sentence this endpoint refuses a \
             service-owned agent with, body {}",
            reply.body
        );
        let read = get_agent(&emulator, &assistant, built_in_id).await;
        assert_eq!(
            read.status, 200,
            "case {built_in_type:?}: the refused delete must have removed nothing, got HTTP {} \
             with body {}",
            read.status, read.body
        );
    }

    // Nothing above removed the agent the test created: it still reads back.
    let read = get_agent(&emulator, &assistant, &agent_id).await;
    assert_eq!(
        read.status, 200,
        "the agent none of the refused deletes named must still read back, got HTTP {} with \
         body {}",
        read.status, read.body
    );
}

