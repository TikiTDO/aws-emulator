// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Q in Connect AI-prompt slice (`CreateAIPrompt` +
//! `GetAIPrompt`), driven over HTTP through the same router the emulator serves with.
//!
//! The rule under test, asserted over a TABLE of inputs rather than the one recorded
//! request: a create of a resource nested in an assistant PERSISTS every member the
//! request carried under an id the service mints, and answers out of the stored record, so
//! a later read — by id, by ARN, or by either carrying the working copy's version
//! qualifier — reports exactly the same values, while a label naming no such resource of
//! the assistant the route named is refused as not found.
//!
//! The rows differ along every axis the rule generalises over:
//!
//! * every value of the modelled `AIPromptType` catalog, and every value of
//!   `AIPromptAPIFormat` (the two recommended ones and the two the model documents as
//!   being deprecated);
//! * both values the modelled `VisibilityStatus` declares;
//! * the published length bounds of `name`, `description`, `modelId` and the template's
//!   own text, and whether the optional members are sent at all;
//! * the inference configuration: absent, partial, and every parameter at each published
//!   bound of its range;
//! * the tag count (none, one, several).
//!
//! Every assertion goes over the wire — only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected — so a response that is not computed from
//! persisted state cannot pass. The harness (the router, the reply reader and the
//! assistant create) is the one `resource_tags.rs` and `ai_guardrail.rs` already drive
//! this service with.

use serde_json::{json, Value};

use service_com_amazonaws_qconnect_wisdomservice::state::MAX_AI_PROMPTS_PER_ASSISTANT;

use super::ai_guardrail::{
    assert_not_found, assistant_id, QUALIFIER_LATEST, STATUS_ACTIVE, VISIBILITY_PUBLISHED,
    VISIBILITY_SAVED,
};
use super::resource_tags::{
    label, tag_map, wisdom_arn, Emulator, Reply, ABSENT_UUID, OTHER_ACCOUNT, OTHER_REGION, REGION,
};

/// The fixed sentence this endpoint answers a label naming no AI prompt of the assistant
/// the route named with, written out here rather than imported from the implementation so
/// the assertion pins the answer a client sees.
const MESSAGE_AI_PROMPT_NOT_FOUND: &str = "AI Prompt does not exist.";

/// The sentence a create whose `clientToken` replays a DIFFERENT request is refused with.
const MESSAGE_IDEMPOTENCY_CONFLICT: &str = "Idempotency conflict";

/// The `Origin` a prompt a caller created reports
/// (`smithy_query(query="shape",shape="Origin")` declares `SYSTEM` beside it).
const ORIGIN_CUSTOMER: &str = "CUSTOMER";

/// Every value the modelled `AIPromptType` enumeration declares
/// (`smithy_query(query="shape",shape="AIPromptType")`), written out here rather than
/// imported from the implementation so the table spans the whole catalog — including the
/// types no recording of this surface used.
const PROMPT_TYPES: [&str; 12] = [
    "ANSWER_GENERATION",
    "INTENT_LABELING_GENERATION",
    "QUERY_REFORMULATION",
    "SELF_SERVICE_PRE_PROCESSING",
    "SELF_SERVICE_ANSWER_GENERATION",
    "EMAIL_RESPONSE",
    "EMAIL_OVERVIEW",
    "EMAIL_GENERATIVE_ANSWER",
    "EMAIL_QUERY_REFORMULATION",
    "ORCHESTRATION",
    "NOTE_TAKING",
    "CASE_SUMMARIZATION",
];

/// Every value the modelled `AIPromptAPIFormat` enumeration declares
/// (`smithy_query(query="shape",shape="AIPromptAPIFormat")`): the two the model recommends
/// and the two it documents as being deprecated, which the service still accepts.
const API_FORMATS: [&str; 4] = [
    "MESSAGES",
    "TEXT_COMPLETIONS",
    "ANTHROPIC_CLAUDE_MESSAGES",
    "ANTHROPIC_CLAUDE_TEXT_COMPLETIONS",
];

/// The only value the modelled `AIPromptTemplateType` enumeration declares
/// (`smithy_query(query="shape",shape="AIPromptTemplateType")`).
const TEMPLATE_TYPE_TEXT: &str = "TEXT";

/// Model identifiers from the catalog `AIPromptData$modelId` documents
/// (`smithy_query(query="doc",shape="AIPromptData",member="modelId")` lists seventeen),
/// including entries no recording of this surface used.
const MODEL_IDS: [&str; 4] = [
    "anthropic.claude-3-haiku--v1:0",
    "us.amazon.nova-pro-v1:0",
    "eu.anthropic.claude-3-7-sonnet-20250219-v1:0",
    "apac.anthropic.claude-3-5-sonnet--v2:0",
];

/// The published bounds of the members this suite varies in length
/// (`smithy_query(query="operation",operation="CreateAIPrompt")`: `name` is `len 1..255`,
/// `description` `len 1..255` and `modelId` `len 1..2048`;
/// `smithy_query(query="shape",shape="TextFullAIPromptEditTemplateConfiguration")`: the
/// template's `text` is `len 1..1000000`).
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 255;
const DESCRIPTION_MIN: usize = 1;
const DESCRIPTION_MAX: usize = 255;
const MODEL_ID_MIN: usize = 1;
const MODEL_ID_MAX: usize = 2048;
const TEMPLATE_TEXT_MIN: usize = 1;
const TEMPLATE_TEXT_MAX: usize = 1_000_000;

/// The published bounds of every `AIPromptInferenceConfiguration` parameter's range
/// (`smithy_query(query="shape",shape="AIPromptInferenceConfiguration")`: `temperature`
/// and `topP` are `range 0..1`, `topK` `range 0..200` and `maxTokensToSample`
/// `range 1..4096`).
const PROBABILITY_MIN: f64 = 0.0;
const PROBABILITY_MAX: f64 = 1.0;
const TOP_K_MIN: i64 = 0;
const TOP_K_MAX: i64 = 200;
const MAX_TOKENS_MIN: i64 = 1;
const MAX_TOKENS_MAX: i64 = 4096;

/// `CreateAIPrompt` on one assistant, as the model's URI binds it
/// (`POST /assistants/{assistantId}/aiprompts`).
async fn create_prompt(emulator: &Emulator, assistant: &str, body: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/assistants/{}/aiprompts", label(assistant)),
            Some(body),
        )
        .await
}

/// `GetAIPrompt` (`GET /assistants/{assistantId}/aiprompts/{aiPromptId}`).
async fn get_prompt(emulator: &Emulator, assistant: &str, prompt: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!(
                "/assistants/{}/aiprompts/{}",
                label(assistant),
                label(prompt)
            ),
            None,
        )
        .await
}

/// A YAML prompt template, as the member's own documentation describes it ("The YAML text
/// for the AI Prompt template").
fn template(text: &str) -> Value {
    json!({ "textFullAIPromptEditTemplateConfiguration": { "text": text } })
}

/// The required members of every create in this suite, so a row states only what it
/// varies (`smithy_query(query="operation",operation="CreateAIPrompt")` marks `name`,
/// `type`, `templateConfiguration`, `visibilityStatus`, `templateType`, `modelId` and
/// `apiFormat` required).
fn required(name: &str, prompt_type: &str, api_format: &str, visibility: &str) -> Value {
    json!({
        "name": name,
        "type": prompt_type,
        "apiFormat": api_format,
        "visibilityStatus": visibility,
        "templateType": TEMPLATE_TYPE_TEXT,
        "modelId": MODEL_IDS[0],
        "templateConfiguration": template("system: You are a helpful assistant.\nmessages: []\n"),
    })
}

/// One row of the create table: what it varies, and the request body it sends.
#[derive(Debug, Clone)]
struct Case {
    label: String,
    body: Value,
}

/// The create bodies the round-trip rule is asserted over. Each row differs from the
/// others along one axis of the rule — the prompt type, the API format, the visibility
/// status, a published length bound, an optional member, the inference configuration, the
/// tag count.
fn cases() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    let mut with = |label: String, mut body: Value, members: Vec<(&str, Value)>| {
        for (member, value) in members {
            body[member] = value;
        }
        cases.push(Case { label, body });
    };

    // One row per value of the `AIPromptType` catalog, each paired with a different API
    // format, model id and visibility status, so the table spans all four catalogs.
    for (index, prompt_type) in PROMPT_TYPES.iter().enumerate() {
        let visibility = if index % 2 == 0 {
            VISIBILITY_SAVED
        } else {
            VISIBILITY_PUBLISHED
        };
        with(
            format!("type {prompt_type}, only the required members"),
            required(
                &format!("prompt of type {prompt_type}"),
                prompt_type,
                API_FORMATS[index % API_FORMATS.len()],
                visibility,
            ),
            vec![("modelId", json!(MODEL_IDS[index % MODEL_IDS.len()]))],
        );
    }

    // One row per value of the `AIPromptAPIFormat` catalog, against one type, so a format
    // is covered on its own axis as well as paired above.
    for api_format in API_FORMATS {
        with(
            format!("apiFormat {api_format}"),
            required(
                &format!("prompt in format {api_format}"),
                PROMPT_TYPES[0],
                api_format,
                VISIBILITY_PUBLISHED,
            ),
            Vec::new(),
        );
    }

    // The published length bounds of the members a create carries by value.
    for (what, length) in [
        ("name at its published minimum length", NAME_MIN),
        ("name at its published maximum length", NAME_MAX),
    ] {
        with(
            format!("{what} ({length})"),
            required(
                &"n".repeat(length),
                PROMPT_TYPES[1],
                API_FORMATS[0],
                VISIBILITY_SAVED,
            ),
            Vec::new(),
        );
    }
    for (what, length) in [
        ("description at its published minimum length", DESCRIPTION_MIN),
        ("description at its published maximum length", DESCRIPTION_MAX),
    ] {
        with(
            format!("{what} ({length})"),
            required(
                &format!("described in {length}"),
                PROMPT_TYPES[2],
                API_FORMATS[1],
                VISIBILITY_SAVED,
            ),
            vec![("description", json!("d".repeat(length)))],
        );
    }
    for (what, length) in [
        ("modelId at its published minimum length", MODEL_ID_MIN),
        ("modelId at its published maximum length", MODEL_ID_MAX),
    ] {
        with(
            format!("{what} ({length})"),
            required(
                &format!("modelled in {length}"),
                PROMPT_TYPES[3],
                API_FORMATS[2],
                VISIBILITY_PUBLISHED,
            ),
            vec![("modelId", json!("m".repeat(length)))],
        );
    }

    // The inference configuration: every parameter at each published bound of its range,
    // and a configuration that carries only one of them.
    with(
        "an inference configuration with every parameter at its published minimum"
            .to_string(),
        required(
            "inference at minimum",
            PROMPT_TYPES[4],
            API_FORMATS[0],
            VISIBILITY_SAVED,
        ),
        vec![(
            "inferenceConfiguration",
            json!({
                "temperature": PROBABILITY_MIN,
                "topP": PROBABILITY_MIN,
                "topK": TOP_K_MIN,
                "maxTokensToSample": MAX_TOKENS_MIN,
            }),
        )],
    );
    with(
        "an inference configuration with every parameter at its published maximum"
            .to_string(),
        required(
            "inference at maximum",
            PROMPT_TYPES[5],
            API_FORMATS[1],
            VISIBILITY_PUBLISHED,
        ),
        vec![(
            "inferenceConfiguration",
            json!({
                "temperature": PROBABILITY_MAX,
                "topP": PROBABILITY_MAX,
                "topK": TOP_K_MAX,
                "maxTokensToSample": MAX_TOKENS_MAX,
            }),
        )],
    );
    with(
        "an inference configuration carrying one parameter only".to_string(),
        required(
            "inference partial",
            PROMPT_TYPES[6],
            API_FORMATS[2],
            VISIBILITY_SAVED,
        ),
        vec![("inferenceConfiguration", json!({ "temperature": 0.25 }))],
    );

    // The tag count, and one row carrying every optional member at once.
    for count in [1_usize, 3] {
        with(
            format!("{count} tag(s)"),
            required(
                &format!("tagged with {count}"),
                PROMPT_TYPES[7],
                API_FORMATS[3],
                VISIBILITY_SAVED,
            ),
            vec![("tags", json!(tag_map(count)))],
        );
    }
    with(
        "every optional member at once".to_string(),
        required(
            "everything at once",
            PROMPT_TYPES[8],
            API_FORMATS[0],
            VISIBILITY_PUBLISHED,
        ),
        vec![
            ("description", json!("Answers questions, politely.")),
            ("tags", json!(tag_map(2))),
            (
                "inferenceConfiguration",
                json!({ "topK": 50, "maxTokensToSample": 512 }),
            ),
            (
                "templateConfiguration",
                template(
                    "system: |\n  You answer questions.\nmessages:\n  - role: user\n    \
                     content: '{{question}}'\n",
                ),
            ),
        ],
    );

    cases
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
        let got = reply.field(&format!("/aiPrompt/{member}"));
        assert_eq!(
            got,
            Some(sent),
            "case {case:?}: {what} must report the {member:?} the request carried, expected \
             {sent}, got {got:?}"
        );
    }
}

/// A read answers with the `AIPromptData` the create stored, member for member — with the
/// one member a read renders differently: the ARN it reports names the version it answered
/// for, which for the prompt's working copy is the floating alias.
fn assert_reads_the_create(case: &str, what: &str, created: &Value, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: {what} must be served, got HTTP {} with body {}",
        reply.status, reply.body
    );
    let mut expected = created.clone();
    let stored_arn = expected
        .get("aiPromptArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    expected["aiPromptArn"] = json!(format!("{stored_arn}:{QUALIFIER_LATEST}"));
    let read = reply.field("/aiPrompt");
    assert_eq!(
        read,
        Some(&expected),
        "case {case:?}: {what} must answer with the record the create stored, expected \
         {expected}, got {read:?}"
    );
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

/// The ARN AWS documents for an AI prompt of this service, written out here rather than
/// taken from the handler's helper so the test pins the format the modelled `Arn` pattern
/// describes: the prompt's own id LAST, behind its parent assistant's.
fn expected_prompt_arn(assistant_id: &str, prompt_id: &str) -> String {
    wisdom_arn(
        "aws",
        REGION,
        engine::EMULATOR_ACCOUNT_ID,
        &format!("ai-prompt/{assistant_id}/{prompt_id}"),
    )
}

/// The ARN a read of one VERSION of a prompt reports: the prompt's own ARN with the
/// qualifier of the version read appended.
fn expected_read_arn(assistant_id: &str, prompt_id: &str, qualifier: &str) -> String {
    format!(
        "{}:{qualifier}",
        expected_prompt_arn(assistant_id, prompt_id)
    )
}

#[tokio::test]
async fn a_created_prompt_is_reported_out_of_the_stored_record_by_every_later_read() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "prompt round trip").await;

    // Everything each row's create answered, so the reads below can be compared against
    // it rather than against the request alone.
    let mut created: Vec<(Case, String, Value)> = Vec::new();
    for case in cases() {
        let reply = create_prompt(&emulator, &assistant, case.body.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: the create must be served, got HTTP {} with body {}",
            case.label, reply.status, reply.body
        );
        assert_reports_request(&case.label, "the create", &case.body, &reply);

        let prompt_id = reply.string("/aiPrompt/aiPromptId");
        assert!(
            is_uuid_shaped(&prompt_id),
            "case {:?}: the minted aiPromptId must have the modelled Uuid form, got \
             {prompt_id:?}",
            case.label
        );
        let prompt_arn = reply.string("/aiPrompt/aiPromptArn");
        assert_eq!(
            prompt_arn,
            expected_prompt_arn(&assistant, &prompt_id),
            "case {:?}: the prompt must report the ARN of this account's prompt in {REGION}, \
             got {prompt_arn:?}",
            case.label
        );
        assert_eq!(
            reply.string("/aiPrompt/assistantId"),
            assistant,
            "case {:?}: the prompt must report the assistant the route named, body {}",
            case.label,
            reply.body
        );
        assert_eq!(
            reply.string("/aiPrompt/assistantArn"),
            wisdom_arn(
                "aws",
                REGION,
                engine::EMULATOR_ACCOUNT_ID,
                &format!("assistant/{assistant}"),
            ),
            "case {:?}: the prompt must report its parent assistant's own ARN, body {}",
            case.label,
            reply.body
        );
        assert_eq!(
            reply.string("/aiPrompt/status"),
            STATUS_ACTIVE,
            "case {:?}: a created prompt is in service, body {}",
            case.label,
            reply.body
        );
        assert_eq!(
            reply.string("/aiPrompt/origin"),
            ORIGIN_CUSTOMER,
            "case {:?}: a prompt a caller created is of CUSTOMER origin, body {}",
            case.label,
            reply.body
        );
        let modified = reply.field("/aiPrompt/modifiedTime").cloned();
        assert!(
            modified.as_ref().is_some_and(Value::is_number),
            "case {:?}: the prompt must report when it was last modified, got {modified:?}",
            case.label
        );
        // The `tags` member travels for every resource this service reports, as an empty
        // object when nothing tagged it.
        let tags = reply.field("/aiPrompt/tags").cloned();
        assert!(
            tags.as_ref().is_some_and(Value::is_object),
            "case {:?}: the prompt must report a tags map, got {tags:?}",
            case.label
        );
        // A prompt created without inference parameters reports none, rather than a
        // structure the caller never sent.
        if case.body.get("inferenceConfiguration").is_none() {
            assert!(
                reply.field("/aiPrompt/inferenceConfiguration").is_none(),
                "case {:?}: a prompt created with no inferenceConfiguration reports none, \
                 body {}",
                case.label,
                reply.body
            );
        }

        let data = reply
            .field("/aiPrompt")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: the create answered no aiPrompt", case.label));
        created.push((case, prompt_id, data));
    }

    // Every id the creates minted is distinct, so one row's record is never another's.
    for (index, (case, prompt_id, _)) in created.iter().enumerate() {
        for (other, other_id, _) in created.iter().skip(index + 1) {
            assert_ne!(
                prompt_id, other_id,
                "case {:?}: two creates must be two records, but {:?} got the same id \
                 {prompt_id:?}",
                case.label, other.label
            );
        }
    }

    // A read by id, by ARN, and by either carrying the working copy's version qualifier,
    // answer with the record the create stored.
    for (case, prompt_id, data) in &created {
        for addressed_by in [
            prompt_id.clone(),
            expected_prompt_arn(&assistant, prompt_id),
            format!("{prompt_id}:{QUALIFIER_LATEST}"),
            expected_read_arn(&assistant, prompt_id, QUALIFIER_LATEST),
        ] {
            let reply = get_prompt(&emulator, &assistant, &addressed_by).await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: a read addressed by {addressed_by:?} must be served, got HTTP {} \
                 with body {}",
                case.label, reply.status, reply.body
            );
            assert_reads_the_create(&case.label, &addressed_by, data, &reply);
            assert_reports_request(&case.label, "a later read", &case.body, &reply);
            // The model: the version number is reported only when the request's
            // `aiPromptId` carried a version qualifier naming a published version, which
            // none of these did.
            assert!(
                reply.field("/versionNumber").is_none(),
                "case {:?}: a read of the prompt itself reports no versionNumber, body {}",
                case.label,
                reply.body
            );
        }
    }

    // A second read of an unchanged prompt answers alike: reading one changes nothing.
    for (case, prompt_id, data) in &created {
        let reply = get_prompt(&emulator, &assistant, prompt_id).await;
        assert_reads_the_create(&case.label, "a repeated read", data, &reply);
    }

    // The assistant the route names is also read through its ARN, which names the same
    // parent: the records are reachable either way.
    let assistant_arn = wisdom_arn(
        "aws",
        REGION,
        engine::EMULATOR_ACCOUNT_ID,
        &format!("assistant/{assistant}"),
    );
    for (case, prompt_id, data) in &created {
        let reply = get_prompt(&emulator, &assistant_arn, prompt_id).await;
        assert_reads_the_create(&case.label, "a read through the parent's ARN", data, &reply);
    }
}

#[tokio::test]
async fn a_prompt_template_round_trips_at_both_published_text_lengths() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "prompt template lengths").await;

    // Rows: the published bounds of the template text, and a multi-line YAML body between
    // them.
    for (what, text) in [
        (
            format!("template text at its published minimum length {TEMPLATE_TEXT_MIN}"),
            "y".repeat(TEMPLATE_TEXT_MIN),
        ),
        (
            "a multi-line YAML template".to_string(),
            "system: |\n  Answer in one sentence.\nmessages:\n  - role: user\n".to_string(),
        ),
        (
            format!("template text at its published maximum length {TEMPLATE_TEXT_MAX}"),
            "y".repeat(TEMPLATE_TEXT_MAX),
        ),
    ] {
        let mut body = required(&what, PROMPT_TYPES[0], API_FORMATS[0], VISIBILITY_SAVED);
        body["templateConfiguration"] = template(&text);
        let created = create_prompt(&emulator, &assistant, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {what:?}: the create must be served, got HTTP {}",
            created.status
        );
        let prompt_id = created.string("/aiPrompt/aiPromptId");
        let read = get_prompt(&emulator, &assistant, &prompt_id).await;
        assert_eq!(
            read.status, 200,
            "case {what:?}: the read must be served, got HTTP {}",
            read.status
        );
        let got = read
            .field("/aiPrompt/templateConfiguration")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            got,
            template(&text),
            "case {what:?}: the read must report the template the create persisted, got a \
             template of {} bytes",
            got.to_string().len()
        );
    }
}

#[tokio::test]
async fn a_label_naming_no_prompt_of_this_assistant_is_not_found() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "prompt not found").await;
    let other = assistant_id(&emulator, "prompt not found elsewhere").await;

    let held = create_prompt(
        &emulator,
        &assistant,
        required("held", PROMPT_TYPES[0], API_FORMATS[0], VISIBILITY_SAVED),
    )
    .await;
    assert_eq!(
        held.status, 200,
        "the prompt this suite reads around must be created, got HTTP {} with body {}",
        held.status, held.body
    );
    let held_id = held.string("/aiPrompt/aiPromptId");

    // Rows: the prompt label that names nothing this assistant holds — an absent uuid, a
    // prompt of ANOTHER assistant, an ARN of another account, another region or another
    // resource family, and a version qualifier naming a version nothing published.
    for prompt_label in [
        ABSENT_UUID.to_string(),
        held_id.clone(),
        expected_prompt_arn(&assistant, &held_id),
        wisdom_arn(
            "aws",
            REGION,
            OTHER_ACCOUNT,
            &format!("ai-prompt/{assistant}/{held_id}"),
        ),
        wisdom_arn(
            "aws",
            OTHER_REGION,
            engine::EMULATOR_ACCOUNT_ID,
            &format!("ai-prompt/{assistant}/{held_id}"),
        ),
        format!("{held_id}:1"),
    ] {
        // The prompt is held by `assistant`, so the rows that name it correctly are read
        // against the OTHER assistant, which holds nothing.
        let reply = get_prompt(&emulator, &other, &prompt_label).await;
        assert_not_found(
            &format!("read of {prompt_label:?} under another assistant"),
            MESSAGE_AI_PROMPT_NOT_FOUND,
            &prompt_label,
            &reply,
        );
    }

    // A numbered version qualifier names no version of a prompt that published none, even
    // under the assistant that holds it.
    let reply = get_prompt(&emulator, &assistant, &format!("{held_id}:1")).await;
    assert_not_found(
        "a numbered version qualifier",
        MESSAGE_AI_PROMPT_NOT_FOUND,
        &format!("{held_id}:1"),
        &reply,
    );

    // A route whose ASSISTANT names nothing this account holds is not found for the
    // assistant, whatever the rest of the route names.
    for assistant_label in [
        ABSENT_UUID.to_string(),
        wisdom_arn(
            "aws",
            REGION,
            OTHER_ACCOUNT,
            &format!("assistant/{assistant}"),
        ),
    ] {
        for prompt_label in [held_id.clone(), ABSENT_UUID.to_string()] {
            let reply = get_prompt(&emulator, &assistant_label, &prompt_label).await;
            assert_not_found(
                &format!("read under assistant {assistant_label:?}"),
                "Assistant does not exist.",
                &assistant_label,
                &reply,
            );
        }
        let reply = create_prompt(
            &emulator,
            &assistant_label,
            required(
                "orphan",
                PROMPT_TYPES[0],
                API_FORMATS[0],
                VISIBILITY_PUBLISHED,
            ),
        )
        .await;
        assert_not_found(
            &format!("create under assistant {assistant_label:?}"),
            "Assistant does not exist.",
            &assistant_label,
            &reply,
        );
    }

    // The prompt that WAS created is still readable: a refusal persisted nothing.
    let reply = get_prompt(&emulator, &assistant, &held_id).await;
    assert_eq!(
        reply.status, 200,
        "the held prompt must still be served, got HTTP {} with body {}",
        reply.status, reply.body
    );
}

#[tokio::test]
async fn a_replayed_client_token_answers_with_the_prompt_it_already_created() {
    let emulator = Emulator::new();
    let first = assistant_id(&emulator, "prompt idempotent first").await;
    let second = assistant_id(&emulator, "prompt idempotent second").await;

    // Rows: the token's form, spanning the published length bounds of `ClientToken`
    // (`len 1..4096`).
    for token in ["a-client-token", &"t".repeat(4096), "1"] {
        let mut body = required(
            "idempotent prompt",
            PROMPT_TYPES[0],
            API_FORMATS[0],
            VISIBILITY_SAVED,
        );
        body["clientToken"] = json!(token);

        let created = create_prompt(&emulator, &first, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {token:?}: the create must be served, got HTTP {} with body {}",
            created.status, created.body
        );
        let prompt_id = created.string("/aiPrompt/aiPromptId");

        let replayed = create_prompt(&emulator, &first, body.clone()).await;
        assert_eq!(
            replayed.status, 200,
            "case {token:?}: the replay must be served, got HTTP {} with body {}",
            replayed.status, replayed.body
        );
        assert_eq!(
            replayed.field("/aiPrompt"),
            created.field("/aiPrompt"),
            "case {token:?}: a replayed token answers with the prompt it already created"
        );

        // The same token carrying a DIFFERENT request is the idempotency conflict.
        for (member, value) in [
            ("modelId", json!(MODEL_IDS[1])),
            ("apiFormat", json!(API_FORMATS[1])),
            (
                "templateConfiguration",
                template("system: Something else entirely.\n"),
            ),
            ("tags", json!(tag_map(1))),
        ] {
            let mut changed = body.clone();
            changed[member] = value;
            let conflict = create_prompt(&emulator, &first, changed).await;
            assert_eq!(
                conflict.status, 409,
                "case {token:?}/{member}: one token identifies one request, got HTTP {} with \
                 body {}",
                conflict.status, conflict.body
            );
            assert_eq!(
                conflict.string("/message"),
                MESSAGE_IDEMPOTENCY_CONFLICT,
                "case {token:?}/{member}: the refusal must carry this endpoint's sentence \
                 for the condition, body {}",
                conflict.body
            );
        }

        // The token is scoped to the assistant it created under, so the same token on
        // another assistant creates another prompt.
        let elsewhere = create_prompt(&emulator, &second, body).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {token:?}: the same token on another parent creates another prompt, got \
             HTTP {} with body {}",
            elsewhere.status, elsewhere.body
        );
        let elsewhere_id = elsewhere.string("/aiPrompt/aiPromptId");
        assert_ne!(
            elsewhere_id, prompt_id,
            "case {token:?}: the two creates must be two records, got one id {prompt_id:?}"
        );
        // Each parent reports only its own record.
        let reply = get_prompt(&emulator, &second, &prompt_id).await;
        assert_not_found(
            &format!("case {token:?}: the first parent's prompt under the second"),
            MESSAGE_AI_PROMPT_NOT_FOUND,
            &prompt_id,
            &reply,
        );
    }
}

#[tokio::test]
async fn the_tags_a_create_persisted_on_a_prompt_are_what_list_tags_reports() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "prompt tags").await;

    // Rows: the tag count — none, one, several.
    for count in [0_usize, 1, 7] {
        let mut body = required(
            &format!("tagged with {count}"),
            PROMPT_TYPES[0],
            API_FORMATS[0],
            VISIBILITY_SAVED,
        );
        let tags = tag_map(count);
        if count > 0 {
            body["tags"] = json!(tags);
        }
        let created = create_prompt(&emulator, &assistant, body).await;
        assert_eq!(
            created.status, 200,
            "case {count}: the create must be served, got HTTP {} with body {}",
            created.status, created.body
        );
        let prompt_arn = created.string("/aiPrompt/aiPromptArn");

        let reported = emulator.list_tags(&prompt_arn).await;
        assert_eq!(
            reported.status, 200,
            "case {count}: the prompt's ARN must name a resource the tagging surface holds, \
             got HTTP {} with body {}",
            reported.status, reported.body
        );
        assert_eq!(
            reported.field("/tags"),
            created.field("/aiPrompt/tags"),
            "case {count}: the reported map must be the one the create persisted"
        );
        assert_eq!(
            reported.field("/tags").cloned().unwrap_or(Value::Null),
            json!(tags),
            "case {count}: the reported map must be the one the request carried"
        );
    }
}

#[tokio::test]
async fn an_assistant_holds_at_most_the_stores_prompt_ceiling() {
    let emulator = Emulator::new();
    let assistant = assistant_id(&emulator, "prompt ceiling").await;

    for index in 0..MAX_AI_PROMPTS_PER_ASSISTANT {
        let reply = create_prompt(
            &emulator,
            &assistant,
            required(
                &format!("prompt {index}"),
                PROMPT_TYPES[index % PROMPT_TYPES.len()],
                API_FORMATS[index % API_FORMATS.len()],
                VISIBILITY_SAVED,
            ),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {index}: a create inside the ceiling must be served, got HTTP {} with body \
             {}",
            reply.status, reply.body
        );
    }

    let refused = create_prompt(
        &emulator,
        &assistant,
        required(
            "one too many",
            PROMPT_TYPES[0],
            API_FORMATS[0],
            VISIBILITY_SAVED,
        ),
    )
    .await;
    assert_eq!(
        refused.status, 402,
        "a create past the ceiling is the modelled quota refusal, got HTTP {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type, "ServiceQuotaExceededException",
        "the refusal must carry the modelled error code, got {:?} with body {}",
        refused.error_type, refused.body
    );
    assert!(
        refused
            .string("/message")
            .contains(&MAX_AI_PROMPTS_PER_ASSISTANT.to_string()),
        "the refusal must state the ceiling it enforced, body {}",
        refused.body
    );
}
