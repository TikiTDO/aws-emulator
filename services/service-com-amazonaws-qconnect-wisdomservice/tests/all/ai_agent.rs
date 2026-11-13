// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Q in Connect AI-agent slice (`CreateAIAgent` +
//! `GetAIAgent`), driven over HTTP through the same router the emulator serves with.
//!
//! The rule under test, asserted over a TABLE of inputs rather than the one recorded
//! request: a create of a resource nested in an assistant PERSISTS every member the
//! request carried and answers out of the stored record, so a later read — by id, by ARN
//! or by the `$LATEST` qualifier that names the working copy — reports exactly the same
//! values, while the members the SERVICE owns (the minted id, the ARN, the origin and the
//! status) are the service's own.
//!
//! The rows differ along every axis the rule generalises over:
//!
//! * every `AIAgentType` the model declares, each with the `AIAgentConfiguration` union
//!   member that type is configured through — including a configuration carrying no member
//!   at all, one carrying every member of its shape, nested association configurations
//!   with each `TagFilter` form, and a version-qualified prompt id;
//! * both values the modelled `VisibilityStatus` declares;
//! * the published length bounds of `name` and `description`, and whether the optional
//!   members are sent at all;
//! * the tag count (none, one, several).
//!
//! Beside the round trip it pins the refusals: a configuration whose union member is not
//! the one the requested `type` is configured through, a parent assistant this account
//! does not hold, an agent its assistant does not hold, an agent of ANOTHER assistant, a
//! numbered version qualifier (nothing publishes a version yet), and a replayed
//! `clientToken` — answered with the agent it already created, or refused when the request
//! behind the token differs.
//!
//! Every assertion goes over the wire — only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected — so a response that is not computed from
//! persisted state cannot pass. The harness (the router, the reply reader, the assistant
//! create and the tagging read) is the one `resource_tags.rs` already drives this service
//! with.

use serde_json::{json, Value};

use service_com_amazonaws_qconnect_wisdomservice::state::MAX_AI_AGENT_CONFIGURATION_ENTRIES;

use super::resource_tags::{
    label, tag_map, wisdom_arn, Emulator, Reply, ABSENT_UUID, OTHER_ACCOUNT, REGION,
};

/// Every value the modelled `AIAgentType` declares, paired with the
/// `AIAgentConfiguration` union member the model documents as "the configuration for AI
/// Agents of type `<TYPE>`" (`smithy_query(query="shape",shape="AIAgentType")` and
/// `…shape="AIAgentConfiguration"`), written out here rather than imported from the
/// implementation so the table spans the whole catalog.
pub(crate) const TYPE_CONFIGURATIONS: [(&str, &str); 9] = [
    ("MANUAL_SEARCH", "manualSearchAIAgentConfiguration"),
    (
        "ANSWER_RECOMMENDATION",
        "answerRecommendationAIAgentConfiguration",
    ),
    ("SELF_SERVICE", "selfServiceAIAgentConfiguration"),
    ("EMAIL_RESPONSE", "emailResponseAIAgentConfiguration"),
    ("EMAIL_OVERVIEW", "emailOverviewAIAgentConfiguration"),
    (
        "EMAIL_GENERATIVE_ANSWER",
        "emailGenerativeAnswerAIAgentConfiguration",
    ),
    ("ORCHESTRATION", "orchestrationAIAgentConfiguration"),
    ("NOTE_TAKING", "noteTakingAIAgentConfiguration"),
    ("CASE_SUMMARIZATION", "caseSummarizationAIAgentConfiguration"),
];

/// Both values the modelled `VisibilityStatus` declares
/// (`smithy_query(query="shape",shape="VisibilityStatus")`).
pub(crate) const VISIBILITY_SAVED: &str = "SAVED";
pub(crate) const VISIBILITY_PUBLISHED: &str = "PUBLISHED";

/// Both values the modelled `Origin` declares
/// (`smithy_query(query="shape",shape="Origin")`): what the service provisioned, and what
/// a caller created.
const ORIGIN_SYSTEM: &str = "SYSTEM";
pub(crate) const ORIGIN_CUSTOMER: &str = "CUSTOMER";

/// The `Status` an AI agent this service holds reports
/// (`smithy_query(query="shape",shape="Status")` declares it beside the transient
/// `CREATE_IN_PROGRESS`, `DELETE_IN_PROGRESS` and the rest).
pub(crate) const STATUS_ACTIVE: &str = "ACTIVE";

/// The published bounds of the members this suite varies in length
/// (`smithy_query(query="operation",operation="CreateAIAgent")`: `name` and `description`
/// are both `len 1..255`).
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 255;
pub(crate) const DESCRIPTION_MIN: usize = 1;
pub(crate) const DESCRIPTION_MAX: usize = 255;

/// The only value the modelled `AIAgentAssociationConfigurationType` declares, and both
/// values of `KnowledgeBaseSearchType`
/// (`smithy_query(query="shape",shape="AIAgentAssociationConfigurationType")`,
/// `…shape="KnowledgeBaseSearchType"`).
const ASSOCIATION_TYPE_KNOWLEDGE_BASE: &str = "KNOWLEDGE_BASE";
pub(crate) const SEARCH_TYPES: [&str; 2] = ["HYBRID", "SEMANTIC"];

/// The published maximum of `KnowledgeBaseAssociationConfigurationData$maxResults`
/// (`smithy_query(query="shape",shape="KnowledgeBaseAssociationConfigurationData")`:
/// `range 1..100`), and its minimum.
pub(crate) const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The qualifier AWS addresses the unversioned WORKING COPY of a versioned resource by,
/// which the modelled label pattern admits
/// (`smithy_query(query="shape",shape="UuidOrArnOrEitherWithQualifier")`: a uuid or an
/// ARN, each optionally followed by `:[A-Z0-9_$]+`).
pub(crate) const QUALIFIER_LATEST: &str = "$LATEST";

/// The fixed sentences this endpoint answers a resource it does not hold with, and the one
/// it refuses a replayed token carrying a different request with. Written out here rather
/// than imported from the implementation, so the assertions pin the answer a client sees.
pub(crate) const MESSAGE_AI_AGENT_NOT_FOUND: &str = "AI Agent does not exist.";
pub(crate) const MESSAGE_ASSISTANT_NOT_FOUND: &str = "Assistant does not exist.";
pub(crate) const MESSAGE_IDEMPOTENCY_CONFLICT: &str = "Idempotency conflict";

/// A `UuidWithQualifier`-shaped id for a configuration member to name, built from `seed`
/// so two members of one row never name the same resource
/// (`smithy_query(query="shape",shape="UuidWithQualifier")`: a lowercase-hex uuid plus an
/// optional `:<QUALIFIER>`).
pub(crate) fn configured_id(seed: usize) -> String {
    format!("{seed:08x}-1a2b-3c4d-5e6f-0123456789ab")
}

/// `CreateAIAgent` on one assistant, as the model's URI binds it
/// (`POST /assistants/{assistantId}/aiagents`).
pub(crate) async fn create_agent(emulator: &Emulator, assistant: &str, body: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/assistants/{}/aiagents", label(assistant)),
            Some(body),
        )
        .await
}

/// `GetAIAgent` (`GET /assistants/{assistantId}/aiagents/{aiAgentId}`).
pub(crate) async fn get_agent(emulator: &Emulator, assistant: &str, agent: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!(
                "/assistants/{}/aiagents/{}",
                label(assistant),
                label(agent)
            ),
            None,
        )
        .await
}

/// A fresh assistant to nest agents in, answering with the id its create minted.
pub(crate) async fn assistant_id(emulator: &Emulator, name: &str) -> String {
    let reply = emulator.create_assistant(name, &tag_map(0)).await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: the assistant this suite nests under must be created, got HTTP {} \
         with body {}",
        reply.status, reply.body
    );
    let id = reply.string("/assistant/assistantId");
    assert!(
        !id.is_empty(),
        "case {name:?}: the assistant create answered no assistantId, body {}",
        reply.body
    );
    id
}

/// One association configuration a row's configuration carries, with the `TagFilter` form
/// `filter` selects: the union's three members
/// (`smithy_query(query="shape",shape="TagFilter")`) spread across the rows that use one.
pub(crate) fn association_configuration(
    seed: usize,
    search_type: &str,
    max_results: i64,
    filter: Value,
) -> Value {
    json!({
        "associationId": configured_id(seed),
        "associationType": ASSOCIATION_TYPE_KNOWLEDGE_BASE,
        "associationConfigurationData": {
            "knowledgeBaseAssociationConfigurationData": {
                "contentTagFilter": filter,
                "maxResults": max_results,
                "overrideKnowledgeBaseSearchType": search_type,
            },
        },
    })
}

/// The configuration body each `AIAgentType` is created with, one per entry of
/// [`TYPE_CONFIGURATIONS`]: every member of that type's configuration shape, so the row
/// spans the shape rather than the one member a single recording used.
pub(crate) fn full_configuration(agent_type: &str) -> Value {
    match agent_type {
        "MANUAL_SEARCH" => json!({ "manualSearchAIAgentConfiguration": {
            "answerGenerationAIPromptId": configured_id(1),
            "answerGenerationAIGuardrailId": configured_id(2),
            "locale": "en_US",
            "associationConfigurations": [association_configuration(
                3,
                SEARCH_TYPES[0],
                MAX_RESULTS_MAX,
                json!({ "tagCondition": { "key": "team", "value": "support" } }),
            )],
        } }),
        "ANSWER_RECOMMENDATION" => json!({ "answerRecommendationAIAgentConfiguration": {
            "intentLabelingGenerationAIPromptId": configured_id(4),
            "queryReformulationAIPromptId": configured_id(5),
            // A version-qualified id: the member's pattern admits a qualifier, and what a
            // caller pinned is stored and reported exactly as given.
            "answerGenerationAIPromptId": format!("{}:{QUALIFIER_LATEST}", configured_id(6)),
            "answerGenerationAIGuardrailId": configured_id(7),
            "locale": "es_ES",
            "suggestedMessages": ["How can I help?", "Anything else?"],
            "associationConfigurations": [association_configuration(
                8,
                SEARCH_TYPES[1],
                MAX_RESULTS_MIN,
                json!({ "andConditions": [
                    { "key": "team", "value": "support" },
                    { "key": "tier" },
                ] }),
            )],
        } }),
        "SELF_SERVICE" => json!({ "selfServiceAIAgentConfiguration": {
            "selfServicePreProcessingAIPromptId": configured_id(9),
            "selfServiceAnswerGenerationAIPromptId": configured_id(10),
            "selfServiceAIGuardrailId": configured_id(11),
            "associationConfigurations": [association_configuration(
                12,
                SEARCH_TYPES[0],
                MAX_RESULTS_MAX,
                json!({ "orConditions": [
                    { "andConditions": [{ "key": "team", "value": "support" }] },
                    { "tagCondition": { "key": "tier", "value": "gold" } },
                ] }),
            )],
        } }),
        "EMAIL_RESPONSE" => json!({ "emailResponseAIAgentConfiguration": {
            "emailResponseAIPromptId": configured_id(13),
            "emailQueryReformulationAIPromptId": configured_id(14),
            "locale": "en_GB",
        } }),
        "EMAIL_OVERVIEW" => json!({ "emailOverviewAIAgentConfiguration": {
            "emailOverviewAIPromptId": configured_id(15),
            "locale": "fr_FR",
        } }),
        "EMAIL_GENERATIVE_ANSWER" => json!({ "emailGenerativeAnswerAIAgentConfiguration": {
            "emailGenerativeAnswerAIPromptId": configured_id(16),
            "emailQueryReformulationAIPromptId": configured_id(17),
            "locale": "de_DE",
            "associationConfigurations": [association_configuration(
                18,
                SEARCH_TYPES[1],
                MAX_RESULTS_MAX,
                json!({ "tagCondition": { "key": "tier" } }),
            )],
        } }),
        "ORCHESTRATION" => json!({ "orchestrationAIAgentConfiguration": {
            "orchestrationAIPromptId": configured_id(19),
            "orchestrationAIGuardrailId": configured_id(20),
            "connectInstanceArn": format!(
                "arn:aws:connect:{REGION}:{}:instance/{ABSENT_UUID}",
                engine::EMULATOR_ACCOUNT_ID,
            ),
            "locale": "it_IT",
        } }),
        "NOTE_TAKING" => json!({ "noteTakingAIAgentConfiguration": {
            "noteTakingAIPromptId": configured_id(21),
            "noteTakingAIGuardrailId": configured_id(22),
            "locale": "pt_BR",
        } }),
        "CASE_SUMMARIZATION" => json!({ "caseSummarizationAIAgentConfiguration": {
            "caseSummarizationAIPromptId": configured_id(23),
            "caseSummarizationAIGuardrailId": configured_id(24),
            "locale": "ja_JP",
        } }),
        other => panic!("case {other:?}: the suite names no configuration for this AIAgentType"),
    }
}

/// One row of the create table: what it varies, and the request body it sends.
#[derive(Debug, Clone)]
struct Case {
    label: String,
    body: Value,
}

/// The create bodies the round-trip rule is asserted over. Each row differs from the
/// others along one axis of the rule — the AI agent type and the configuration it is
/// configured through, the visibility status, a published length bound, an optional member,
/// the tag count.
fn cases() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();

    // One row per `AIAgentType`, each with every member of that type's configuration, and
    // the two visibility statuses alternating across them so both are exercised with a
    // configuration of their own.
    for (index, (agent_type, _)) in TYPE_CONFIGURATIONS.iter().enumerate() {
        let visibility = if index % 2 == 0 {
            VISIBILITY_SAVED
        } else {
            VISIBILITY_PUBLISHED
        };
        cases.push(Case {
            label: format!("type {agent_type} with every configuration member, {visibility}"),
            body: json!({
                "name": format!("agent-{agent_type}"),
                "type": agent_type,
                "visibilityStatus": visibility,
                "configuration": full_configuration(agent_type),
            }),
        });
    }

    // A configuration carrying NO member of its own: the union member alone says which
    // type it configures, which is all this request claims.
    for (agent_type, member) in [TYPE_CONFIGURATIONS[4], TYPE_CONFIGURATIONS[6]] {
        cases.push(Case {
            label: format!("type {agent_type} with an empty {member}"),
            body: json!({
                "name": format!("bare-{agent_type}"),
                "type": agent_type,
                "visibilityStatus": VISIBILITY_PUBLISHED,
                "configuration": json!({ member: {} }),
            }),
        });
    }

    let (agent_type, _) = TYPE_CONFIGURATIONS[0];
    let mut with = |label: String, name: String, visibility: &str, members: Vec<(&str, Value)>| {
        let mut body = json!({
            "name": name,
            "type": agent_type,
            "visibilityStatus": visibility,
            "configuration": full_configuration(agent_type),
        });
        for (member, value) in members {
            body[member] = value;
        }
        cases.push(Case { label, body });
    };

    with(
        format!("name at its published minimum length ({NAME_MIN})"),
        "n".repeat(NAME_MIN),
        VISIBILITY_SAVED,
        Vec::new(),
    );
    with(
        format!("name at its published maximum length ({NAME_MAX})"),
        "n".repeat(NAME_MAX),
        VISIBILITY_PUBLISHED,
        Vec::new(),
    );
    with(
        format!("description at its published minimum length ({DESCRIPTION_MIN})"),
        "description at minimum".to_string(),
        VISIBILITY_SAVED,
        vec![("description", json!("d".repeat(DESCRIPTION_MIN)))],
    );
    with(
        format!("description at its published maximum length ({DESCRIPTION_MAX})"),
        "description at maximum".to_string(),
        VISIBILITY_PUBLISHED,
        vec![("description", json!("d".repeat(DESCRIPTION_MAX)))],
    );
    with(
        "one tag".to_string(),
        "one tag".to_string(),
        VISIBILITY_SAVED,
        vec![("tags", json!(tag_map(1)))],
    );
    with(
        "several tags and a description".to_string(),
        "several tags".to_string(),
        VISIBILITY_PUBLISHED,
        vec![
            ("tags", json!(tag_map(3))),
            ("description", json!("Everything this row carries.")),
        ],
    );

    cases
}

#[tokio::test]
async fn a_configuration_is_stored_up_to_the_stores_entry_ceiling_and_refused_past_it() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "configuration ceiling").await;

    // A configuration whose one open-ended list carries `entries` association
    // configurations: the list the model puts no maximum on, so it is what the store's
    // ceiling is about.
    let body = |name: &str, entries: usize| {
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
            "name": name,
            "type": "MANUAL_SEARCH",
            "visibilityStatus": VISIBILITY_SAVED,
            "configuration": { "manualSearchAIAgentConfiguration": {
                "associationConfigurations": associations,
            } },
        })
    };

    // Exactly at the ceiling: stored, and read back out of the record.
    let at_ceiling = create_agent(
        &emulator,
        &assistant,
        body("configuration at the ceiling", MAX_AI_AGENT_CONFIGURATION_ENTRIES),
    )
    .await;
    assert_eq!(
        at_ceiling.status, 200,
        "a configuration of exactly {MAX_AI_AGENT_CONFIGURATION_ENTRIES} entries must be \
         stored, got HTTP {} with body {}",
        at_ceiling.status, at_ceiling.body
    );
    let agent_id = at_ceiling.string("/aiAgent/aiAgentId");
    let read = get_agent(&emulator, &assistant, &agent_id).await;
    assert_reads_the_record(
        "a configuration at the ceiling",
        "a read of the stored configuration",
        at_ceiling
            .field("/aiAgent")
            .unwrap_or_else(|| panic!("the create answered no aiAgent: {}", at_ceiling.body)),
        &read,
    );

    // One entry past it: refused, and nothing stored.
    let refused = create_agent(
        &emulator,
        &assistant,
        body(
            "configuration past the ceiling",
            MAX_AI_AGENT_CONFIGURATION_ENTRIES + 1,
        ),
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
    assert!(
        refused.field("/aiAgent").is_none(),
        "the refused create must have answered with no agent, body {}",
        refused.body
    );
}

/// Every member the request carried is reported back unchanged, whichever answer is being
/// read — the create's own or a later read's.
fn assert_reports_request(case: &str, what: &str, body: &Value, reply: &Reply) {
    let members = body
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the request body must be an object"));
    for (member, sent) in members {
        if member == "clientToken" {
            continue;
        }
        let got = reply.field(&format!("/aiAgent/{member}"));
        assert_eq!(
            got,
            Some(sent),
            "case {case:?}: {what} must report the {member:?} the request carried, expected \
             {sent}, got {got:?}"
        );
    }
}

/// Whether `value` has the form the modelled `Uuid` shape declares
/// (`smithy_query(query="shape",shape="Uuid")`:
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`).
fn is_uuid_shaped(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, expected)| {
                group.len() == expected
                    && group.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && group.bytes().all(|byte| !byte.is_ascii_uppercase())
            })
}

/// The ARN AWS documents for an AI agent of this service, written out here rather than
/// taken from the handler's helper so the test pins the format the modelled `Arn` pattern
/// describes: the agent's own id LAST, behind its parent assistant's.
pub(crate) fn expected_agent_arn(assistant_id: &str, agent_id: &str) -> String {
    wisdom_arn(
        "aws",
        REGION,
        engine::EMULATOR_ACCOUNT_ID,
        &format!("ai-agent/{assistant_id}/{agent_id}"),
    )
}

/// The ARN a read of one VERSION of an agent reports: the agent's own ARN with the
/// qualifier of the version read appended. An operation addressed through a label that
/// may carry a version qualifier names the version it answered for; the create, which
/// addresses no version, reports the bare resource ARN.
pub(crate) fn expected_read_arn(assistant_id: &str, agent_id: &str, qualifier: &str) -> String {
    format!("{}:{qualifier}", expected_agent_arn(assistant_id, agent_id))
}

/// A read answers with the `AIAgentData` the last write stored, member for member — with
/// the one member a read renders differently: the ARN it reports names the version it
/// answered for, which for the agent's working copy is the floating alias.
pub(crate) fn assert_reads_the_record(case: &str, what: &str, record: &Value, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: {what} must be served, got HTTP {} with body {}",
        reply.status, reply.body
    );
    let mut expected = record.clone();
    let stored_arn = expected
        .get("aiAgentArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    expected["aiAgentArn"] = json!(format!("{stored_arn}:{QUALIFIER_LATEST}"));
    let read = reply.field("/aiAgent");
    assert_eq!(
        read,
        Some(&expected),
        "case {case:?}: {what} must answer with the record the write stored, expected \
         {expected}, got {read:?}"
    );
}

/// A reply refusing a resource this account does not hold: HTTP 404, the modelled error
/// and the sentence this endpoint words that class with, naming the resource addressed.
pub(crate) fn assert_not_found(case: &str, message: &str, resource: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {}",
        reply.status, reply.body
    );
    assert!(
        reply.error_type.contains("ResourceNotFoundException"),
        "case {case:?}: expected the modelled ResourceNotFoundException, got x-amzn-errortype \
         {:?} with body {}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.string("/message"),
        message,
        "case {case:?}: expected the sentence this endpoint words a missing resource with, \
         body {}",
        reply.body
    );
    assert_eq!(
        reply.string("/resourceName"),
        resource,
        "case {case:?}: the refusal must name the resource the request addressed, body {}",
        reply.body
    );
}

#[tokio::test]
async fn a_created_ai_agent_is_reported_out_of_the_stored_record_by_every_later_read() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent round trip").await;
    let assistant_arn = wisdom_arn(
        "aws",
        REGION,
        engine::EMULATOR_ACCOUNT_ID,
        &format!("assistant/{assistant}"),
    );

    // Everything each row's create answered, so the reads below can be compared against
    // it rather than against the request alone.
    let mut created: Vec<(Case, String, Value)> = Vec::new();
    for case in cases() {
        let reply = create_agent(&emulator, &assistant, case.body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: the create must be served, got HTTP {} with body {}",
            case.label, reply.status, reply.body
        );
        assert_reports_request(&case.label, "the create", &case.body, &reply);

        let agent_id = reply.string("/aiAgent/aiAgentId");
        assert!(
            is_uuid_shaped(&agent_id),
            "case {:?}: the minted aiAgentId must have the modelled Uuid form, got {agent_id:?}",
            case.label
        );
        let agent_arn = reply.string("/aiAgent/aiAgentArn");
        assert_eq!(
            agent_arn,
            expected_agent_arn(&assistant, &agent_id),
            "case {:?}: the agent must report the ARN of this account's agent in {REGION}, got \
             {agent_arn:?}",
            case.label
        );
        assert_eq!(
            reply.string("/aiAgent/assistantId"),
            assistant,
            "case {:?}: the agent must report the assistant the route named, body {}",
            case.label,
            reply.body
        );
        assert_eq!(
            reply.string("/aiAgent/assistantArn"),
            assistant_arn,
            "case {:?}: the agent must report its parent assistant's own ARN, body {}",
            case.label,
            reply.body
        );
        assert_eq!(
            reply.string("/aiAgent/origin"),
            ORIGIN_CUSTOMER,
            "case {:?}: an agent a caller created is CUSTOMER-origin, body {}",
            case.label,
            reply.body
        );
        assert_eq!(
            reply.string("/aiAgent/status"),
            STATUS_ACTIVE,
            "case {:?}: a created agent is in service, body {}",
            case.label,
            reply.body
        );
        let modified = reply.field("/aiAgent/modifiedTime").cloned();
        assert!(
            modified.as_ref().is_some_and(Value::is_number),
            "case {:?}: the agent must report when it was last modified, got {modified:?}",
            case.label
        );
        // The `tags` member travels for every resource this service reports, as an empty
        // object when nothing tagged it.
        let tags = reply.field("/aiAgent/tags").cloned();
        assert!(
            tags.as_ref().is_some_and(Value::is_object),
            "case {:?}: the agent must report a tags map, got {tags:?}",
            case.label
        );
        // A create that named no description reports none rather than an empty string.
        if case.body.get("description").is_none() {
            assert!(
                reply.field("/aiAgent/description").is_none(),
                "case {:?}: an agent created without a description reports none, body {}",
                case.label,
                reply.body
            );
        }

        let data = reply
            .field("/aiAgent")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: the create answered no aiAgent", case.label));
        created.push((case, agent_id, data));
    }

    // A read by id, the same read by ARN, and the same read with the qualifier that names
    // the working copy all answer with the record the create stored.
    for (case, agent_id, data) in &created {
        for addressed_by in [
            agent_id.clone(),
            expected_agent_arn(&assistant, agent_id),
            format!("{agent_id}:{QUALIFIER_LATEST}"),
            expected_read_arn(&assistant, agent_id, QUALIFIER_LATEST),
        ] {
            let reply = get_agent(&emulator, &assistant, &addressed_by).await;
            assert_reads_the_record(
                &case.label,
                &format!("a read addressed by {addressed_by:?}"),
                data,
                &reply,
            );
            assert_reports_request(&case.label, "a later read", &case.body, &reply);
            // The model: the version number is reported only when the request's
            // `aiAgentId` named a VERSION, which none of these did.
            assert!(
                reply.field("/versionNumber").is_none(),
                "case {:?}: a read of the agent itself reports no versionNumber, body {}",
                case.label,
                reply.body
            );
        }
    }

    // A second read of an unchanged agent answers alike: reading one changes nothing, and
    // one row's agent is never another's.
    for (case, agent_id, data) in &created {
        let reply = get_agent(&emulator, &assistant, agent_id).await;
        assert_reads_the_record(&case.label, "a repeated read", data, &reply);
    }

    // The agent the create answered with is the one the tagging surface reports tags for,
    // since there is only ever one copy of a resource's tags.
    for (case, agent_id, data) in &created {
        let reply = emulator
            .list_tags(&expected_agent_arn(&assistant, agent_id))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: the created agent's ARN must name a resource the tagging surface \
             holds, got HTTP {} with body {}",
            case.label, reply.status, reply.body
        );
        assert_eq!(
            reply.field("/tags"),
            data.get("tags"),
            "case {:?}: the tagging surface must report the tags the create stored, body {}",
            case.label,
            reply.body
        );
    }
}

#[tokio::test]
async fn the_built_in_agents_an_assistant_is_provisioned_with_read_back_as_system_agents() {
    let emulator = Emulator::new();
    let reply = emulator
        .create_assistant("built-in agents", &tag_map(0))
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
        .unwrap_or_else(|| panic!("the assistant reported no aiAgentConfiguration: {}", reply.body));
    assert!(
        !bindings.is_empty(),
        "a fresh assistant is provisioned with built-in AI agents, body {}",
        reply.body
    );

    // Every binding the assistant reports names an agent this store holds: the id the
    // binding carries, with the version qualifier stripped off, reads back as the
    // SYSTEM-origin agent for that type.
    for (agent_type, binding) in &bindings {
        let bound = binding
            .pointer("/aiAgentId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let agent_id = bound.split(':').next().unwrap_or_default().to_string();
        let reply = get_agent(&emulator, &assistant, &agent_id).await;
        assert_eq!(
            reply.status, 200,
            "case {agent_type:?}: the bound built-in agent must be readable, got HTTP {} with \
             body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.string("/aiAgent/aiAgentId"),
            agent_id,
            "case {agent_type:?}: the read must answer for the agent the binding named, body {}",
            reply.body
        );
        assert_eq!(
            reply.string("/aiAgent/type"),
            *agent_type,
            "case {agent_type:?}: the bound agent must be the agent for that type, body {}",
            reply.body
        );
        assert_eq!(
            reply.string("/aiAgent/origin"),
            ORIGIN_SYSTEM,
            "case {agent_type:?}: an agent the service provisioned is SYSTEM-origin, body {}",
            reply.body
        );
        assert_eq!(
            reply.string("/aiAgent/status"),
            STATUS_ACTIVE,
            "case {agent_type:?}: a provisioned agent is in service, body {}",
            reply.body
        );
        assert_eq!(
            reply.string("/aiAgent/visibilityStatus"),
            VISIBILITY_PUBLISHED,
            "case {agent_type:?}: a provisioned agent is in service for the assistant, body {}",
            reply.body
        );
        // The configuration a built-in agent reports is the union member its type is
        // configured through, whichever type the binding names.
        let expected_member = TYPE_CONFIGURATIONS
            .iter()
            .find(|(declared, _)| declared == agent_type)
            .map(|(_, member)| *member)
            .unwrap_or_else(|| panic!("case {agent_type:?}: no AIAgentType declares this label"));
        let configuration = reply
            .field("/aiAgent/configuration")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {agent_type:?}: the agent reported no configuration: {}",
                    reply.body
                )
            });
        assert!(
            configuration.contains_key(expected_member),
            "case {agent_type:?}: expected the {expected_member:?} configuration, got {:?}",
            configuration.keys().collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn a_configuration_for_another_type_than_the_request_names_is_refused() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "configuration mismatch").await;

    // Every type, each paired with the configuration of the NEXT type in the catalog, so
    // every member of the union is tried against a type it does not configure.
    for (index, (agent_type, _)) in TYPE_CONFIGURATIONS.iter().enumerate() {
        let (other_type, other_member) = TYPE_CONFIGURATIONS[(index + 1) % TYPE_CONFIGURATIONS.len()];
        let case = format!("type {agent_type} carrying the {other_member} of {other_type}");
        let reply = create_agent(
            &emulator,
            &assistant,
            json!({
                "name": format!("mismatch-{agent_type}"),
                "type": agent_type,
                "visibilityStatus": VISIBILITY_SAVED,
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
            "case {case:?}: the refusal must state the rule for the type the request named, \
             body {}",
            reply.body
        );
    }

    // And the matching pair, which the same table otherwise refuses, is served: the
    // refusal is about the PAIR, not about the configuration.
    for (agent_type, _) in TYPE_CONFIGURATIONS {
        let reply = create_agent(
            &emulator,
            &assistant,
            json!({
                "name": format!("matched-{agent_type}"),
                "type": agent_type,
                "visibilityStatus": VISIBILITY_SAVED,
                "configuration": full_configuration(agent_type),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {agent_type:?}: the matching type and configuration must be served, got HTTP \
             {} with body {}",
            reply.status, reply.body
        );
    }
}

#[tokio::test]
async fn an_agent_this_account_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent misses").await;
    let other_assistant = assistant_id(&emulator, "ai agent misses, other parent").await;
    let (agent_type, _) = TYPE_CONFIGURATIONS[0];
    let created = create_agent(
        &emulator,
        &assistant,
        json!({
            "name": "held by one assistant",
            "type": agent_type,
            "visibilityStatus": VISIBILITY_SAVED,
            "configuration": full_configuration(agent_type),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the agent this test reads must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    let agent_id = created.string("/aiAgent/aiAgentId");

    // A create whose parent assistant is not there.
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
        let reply = create_agent(
            &emulator,
            &parent,
            json!({
                "name": "orphan",
                "type": agent_type,
                "visibilityStatus": VISIBILITY_SAVED,
                "configuration": full_configuration(agent_type),
            }),
        )
        .await;
        assert_not_found(case, MESSAGE_ASSISTANT_NOT_FOUND, &parent, &reply);
    }

    // A read whose parent assistant is not there.
    let reply = get_agent(&emulator, ABSENT_UUID, &agent_id).await;
    assert_not_found(
        "a read under an assistant id nothing holds",
        MESSAGE_ASSISTANT_NOT_FOUND,
        ABSENT_UUID,
        &reply,
    );

    // A read whose agent is not there, however it is addressed: an id nothing holds, the
    // agent of ANOTHER assistant, and a NUMBERED version qualifier — nothing publishes a
    // version of an agent yet, so a number names nothing here.
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
        let reply = get_agent(&emulator, &parent, &addressed_by).await;
        assert_not_found(case, MESSAGE_AI_AGENT_NOT_FOUND, &addressed_by, &reply);
    }

    // The agent itself is still there: a refusal changes nothing.
    let reply = get_agent(&emulator, &assistant, &agent_id).await;
    assert_reads_the_record(
        "after every refusal",
        "a read of the agent that was held all along",
        created
            .field("/aiAgent")
            .unwrap_or_else(|| panic!("the create answered no aiAgent: {}", created.body)),
        &reply,
    );
}

#[tokio::test]
async fn a_replayed_client_token_answers_with_the_agent_it_already_created() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "ai agent idempotency").await;

    for (index, (agent_type, _)) in TYPE_CONFIGURATIONS.iter().enumerate() {
        let token = format!("token-{index}");
        let body = json!({
            "name": format!("idempotent-{agent_type}"),
            "type": agent_type,
            "visibilityStatus": VISIBILITY_SAVED,
            "configuration": full_configuration(agent_type),
            "clientToken": token,
        });
        let first = create_agent(&emulator, &assistant, body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {agent_type:?}: the first create must be served, got HTTP {} with body {}",
            first.status, first.body
        );
        let replay = create_agent(&emulator, &assistant, body.clone()).await;
        assert_eq!(
            replay.field("/aiAgent"),
            first.field("/aiAgent"),
            "case {agent_type:?}: a replayed clientToken must answer with the agent it already \
             created, expected {:?}, got {:?}",
            first.field("/aiAgent"),
            replay.field("/aiAgent")
        );

        // The same token behind a DIFFERENT request is the modelled idempotency conflict
        // rather than a record the caller did not ask for.
        let mut differing = body.clone();
        differing["visibilityStatus"] = json!(VISIBILITY_PUBLISHED);
        let reply = create_agent(&emulator, &assistant, differing).await;
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
    }
}
