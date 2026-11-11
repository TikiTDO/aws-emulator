// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Q in Connect message-template VERSION plane
//! (`CreateMessageTemplateVersion`, `ActivateMessageTemplate`,
//! `DeactivateMessageTemplate`, `ListMessageTemplateVersions`, `UpdateMessageTemplate` and
//! the version-addressed read of `GetMessageTemplate`).
//!
//! Every assertion goes over the wire, through the same [`Emulator`] harness the rest of
//! this crate's suites drive the production router with: only the HTTP status, the
//! `x-amzn-errortype` header and the JSON body are inspected, so a response that is not
//! computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one recorded
//! request:
//!
//! * a request addressed to ONE version of a versioned resource resolves that version
//!   against the stored version records, and a version the resource never published names
//!   nothing: the request is refused with the modelled `ResourceNotFoundException` (HTTP
//!   404) and writes nothing, so the listing after the refusal reports exactly what it
//!   reported before;
//! * `CreateMessageTemplateVersion` publishes the working copy as the next version number,
//!   starting at 1 and increasing by one, and a publish made "while the message template
//!   remains the same" answers with the version it already published rather than numbering
//!   a second identical one; a supplied `messageTemplateContentSha256` that is not the
//!   working copy's is the modelled `ConflictException` (HTTP 409) and publishes nothing;
//! * activating a version makes it the activated one and deactivates the version that was
//!   active, so at most one version of a template reports `isActive: true` and
//!   `$ACTIVE_VERSION` resolves to it; deactivating the activated version leaves none
//!   activated, so `$ACTIVE_VERSION` then names nothing;
//! * a published version is IMMUTABLE: a later `UpdateMessageTemplate` applies to the
//!   working copy (`$LATEST`) alone and leaves every published version's content, language,
//!   attributes and checksum as they were;
//! * `ListMessageTemplateVersions` reports the published versions in ascending
//!   `versionNumber` order, one page at a time, and a token it did not mint is refused;
//! * every one of these operations documents its `messageTemplateId` as a label that
//!   "cannot contain any qualifier", so a label that carries one is the modelled
//!   `ValidationException` (HTTP 400), and a label that names no template of this knowledge
//!   base — a bare id, another knowledge base's template, an ARN of another account — is
//!   HTTP 404;
//! * one message template holds at most the store's version ceiling, past which a publish
//!   is the modelled `ServiceQuotaExceededException` (HTTP 402).

use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::state::MAX_MESSAGE_TEMPLATE_VERSIONS_PER_TEMPLATE;

use super::message_template::{content_for, CHANNEL_SUBTYPES};
use super::resource_tags::{label, wisdom_arn, Emulator, Reply, ABSENT_UUID, OTHER_ACCOUNT, REGION};

/// The sentence a request addressed to a message-template version that was never published
/// is refused with. Written out here rather than imported from the implementation, so the
/// assertion pins the answer a client sees.
const MESSAGE_VERSION_NOT_FOUND: &str = "Message template version does not exist.";

/// The sentences the other two not-found resources of this family are refused with.
const MESSAGE_TEMPLATE_NOT_FOUND: &str = "MessageTemplate does not exist.";
const MESSAGE_KNOWLEDGE_BASE_NOT_FOUND: &str = "Knowledge Base does not exist.";

/// The two managed version qualifiers the read's own documentation names
/// (`smithy_query(query="doc",shape="GetMessageTemplate")`: the qualifier is "either an
/// actual version number or an Amazon Q Connect managed qualifier $ACTIVE_VERSION |
/// $LATEST").
const QUALIFIER_ACTIVE: &str = "$ACTIVE_VERSION";
const QUALIFIER_LATEST: &str = "$LATEST";

/// The low bound of the modelled `Version` shape
/// (`smithy_query(query="operation",operation="ActivateMessageTemplate")`: `versionNumber`
/// is `range min 1`), and the value one past it.
const VERSION_MIN: i64 = 1;
const VERSION_BELOW_MIN: i64 = 0;

/// The published bounds of `maxResults`
/// (`smithy_query(query="operation",operation="ListMessageTemplateVersions")`: `range
/// 1..100`), and the values one past each of them.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;
const MAX_RESULTS_BELOW_MIN: i64 = 0;
const MAX_RESULTS_ABOVE_MAX: i64 = 101;

/// The message-template ARN this service mints, with the qualifier of the version
/// addressed appended. Written out here rather than taken from the handler's helper, so
/// the test pins the format the model's `ArnWithQualifier` pattern describes.
fn expected_version_arn(knowledge_base_id: &str, message_template_id: &str, qualifier: &str) -> String {
    format!(
        "arn:aws:wisdom:{REGION}:{account}:message-template/{knowledge_base_id}/\
         {message_template_id}:{qualifier}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// `CreateKnowledgeBase` of the type that holds message templates, answering with the id
/// its create minted.
async fn knowledge_base(emulator: &Emulator, name: &str) -> String {
    let reply = emulator
        .send(
            "POST",
            "/knowledgeBases",
            Some(json!({ "name": name, "knowledgeBaseType": "MESSAGE_TEMPLATES" })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: the knowledge base this suite nests under must be created, got HTTP {} \
         with body {}",
        reply.status, reply.body
    );
    reply.string("/knowledgeBase/knowledgeBaseId")
}

/// `CreateMessageTemplate`, answering with the `messageTemplateId` its create minted.
async fn message_template(
    emulator: &Emulator,
    knowledge_base_id: &str,
    name: &str,
    channel_subtype: &str,
    content: Value,
) -> String {
    let reply = emulator
        .send(
            "POST",
            &format!(
                "/knowledgeBases/{}/messageTemplates",
                label(knowledge_base_id)
            ),
            Some(json!({
                "name": name,
                "channelSubtype": channel_subtype,
                "content": content,
            })),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: the message template this row publishes versions of must be created, \
         got HTTP {} with body {}",
        reply.status, reply.body
    );
    reply.string("/messageTemplate/messageTemplateId")
}

/// `CreateMessageTemplateVersion`, optionally carrying the caller's
/// `messageTemplateContentSha256` precondition.
async fn publish(
    emulator: &Emulator,
    knowledge_base_id: &str,
    message_template: &str,
    content_sha256: Option<&str>,
) -> Reply {
    let body = match content_sha256 {
        Some(sha256) => json!({ "messageTemplateContentSha256": sha256 }),
        None => json!({}),
    };
    emulator
        .send(
            "POST",
            &format!(
                "/knowledgeBases/{}/messageTemplates/{}/versions",
                label(knowledge_base_id),
                label(message_template)
            ),
            Some(body),
        )
        .await
}

/// `ActivateMessageTemplate` for one `versionNumber`.
async fn activate(
    emulator: &Emulator,
    knowledge_base_id: &str,
    message_template: &str,
    version_number: i64,
) -> Reply {
    emulator
        .send(
            "POST",
            &format!(
                "/knowledgeBases/{}/messageTemplates/{}/activate",
                label(knowledge_base_id),
                label(message_template)
            ),
            Some(json!({ "versionNumber": version_number })),
        )
        .await
}

/// `DeactivateMessageTemplate` for one `versionNumber`.
async fn deactivate(
    emulator: &Emulator,
    knowledge_base_id: &str,
    message_template: &str,
    version_number: i64,
) -> Reply {
    emulator
        .send(
            "POST",
            &format!(
                "/knowledgeBases/{}/messageTemplates/{}/deactivate",
                label(knowledge_base_id),
                label(message_template)
            ),
            Some(json!({ "versionNumber": version_number })),
        )
        .await
}

/// `ListMessageTemplateVersions`, with whatever query string the row pages with.
async fn list_versions(
    emulator: &Emulator,
    knowledge_base_id: &str,
    message_template: &str,
    query: &str,
) -> Reply {
    emulator
        .send(
            "GET",
            &format!(
                "/knowledgeBases/{}/messageTemplates/{}/versions{query}",
                label(knowledge_base_id),
                label(message_template)
            ),
            None,
        )
        .await
}

/// `GetMessageTemplate`, whose label may carry a version qualifier.
async fn read(emulator: &Emulator, knowledge_base_id: &str, message_template: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!(
                "/knowledgeBases/{}/messageTemplates/{}",
                label(knowledge_base_id),
                label(message_template)
            ),
            None,
        )
        .await
}

/// `UpdateMessageTemplate` with whatever members the row changes.
async fn update(
    emulator: &Emulator,
    knowledge_base_id: &str,
    message_template: &str,
    body: Value,
) -> Reply {
    emulator
        .send(
            "POST",
            &format!(
                "/knowledgeBases/{}/messageTemplates/{}",
                label(knowledge_base_id),
                label(message_template)
            ),
            Some(body),
        )
        .await
}

/// An `sms` content union member carrying `body`, so two rows differ in exactly the
/// content the checksum is calculated over.
fn sms_content(body: &str) -> Value {
    json!({ "sms": { "body": { "plainText": { "content": body } } } })
}

/// The integer at `pointer`, or `None` when the response does not carry it.
fn integer(reply: &Reply, pointer: &str) -> Option<i64> {
    reply.field(pointer).and_then(Value::as_i64)
}

/// The boolean at `pointer`, or `None` when the response does not carry it.
fn boolean(reply: &Reply, pointer: &str) -> Option<bool> {
    reply.field(pointer).and_then(Value::as_bool)
}

/// The `versionNumber`s one listing reports, in the order it reported them.
fn listed_versions(reply: &Reply) -> Vec<i64> {
    reply
        .field("/messageTemplateVersionSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    summary
                        .get("versionNumber")
                        .and_then(Value::as_i64)
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `versionNumber`s one listing reports as ACTIVATED.
fn listed_active_versions(reply: &Reply) -> Vec<i64> {
    reply
        .field("/messageTemplateVersionSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter(|summary| summary.get("isActive").and_then(Value::as_bool) == Some(true))
                .map(|summary| {
                    summary
                        .get("versionNumber")
                        .and_then(Value::as_i64)
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Publish `count` versions of one template, each from a working copy the previous version
/// does not hold, answering with the version numbers that were published.
async fn publish_versions(
    emulator: &Emulator,
    knowledge_base_id: &str,
    message_template: &str,
    count: usize,
) -> Vec<i64> {
    let mut published = Vec::new();
    for round in 0..count {
        if round > 0 {
            let changed = update(
                emulator,
                knowledge_base_id,
                message_template,
                json!({ "content": sms_content(&format!("revision {round}")) }),
            )
            .await;
            assert_eq!(
                changed.status, 200,
                "case \"revision {round}\": the working copy must be updated before it is \
                 published again, got HTTP {} with body {}",
                changed.status, changed.body
            );
        }
        let reply = publish(emulator, knowledge_base_id, message_template, None).await;
        assert_eq!(
            reply.status, 200,
            "case \"publish {round}\": expected HTTP 200, got HTTP {} with body {}",
            reply.status, reply.body
        );
        published.push(
            integer(&reply, "/messageTemplate/versionNumber").unwrap_or_default(),
        );
    }
    published
}

/// One row of the refusal table: how many versions the template has published, and the
/// `versionNumber` the activation names.
#[derive(Debug, Clone, Copy)]
struct MissingVersionCase {
    label: &'static str,
    published: usize,
    asked: i64,
}

/// The rule the recorded refusal shows: a `versionNumber` the template never published
/// names no resource, so the activation is refused with HTTP 404
/// `ResourceNotFoundException` and the fixed sentence of that class — and it creates
/// nothing, so the listing after it reports exactly what it reported before.
#[tokio::test]
async fn activating_a_version_that_was_never_published_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "activate-missing").await;

    // The rows differ along what the rule generalises over: how many versions exist (none,
    // one, several) and which number past them the request names (the first, the next, a
    // far one).
    let cases = [
        MissingVersionCase {
            label: "no version published, the lowest version number",
            published: 0,
            asked: VERSION_MIN,
        },
        MissingVersionCase {
            label: "no version published, a later number",
            published: 0,
            asked: 2,
        },
        MissingVersionCase {
            label: "no version published, a far number",
            published: 0,
            asked: 4_096,
        },
        MissingVersionCase {
            label: "one version published, the number past it",
            published: 1,
            asked: 2,
        },
        MissingVersionCase {
            label: "three versions published, the number past them",
            published: 3,
            asked: 4,
        },
        MissingVersionCase {
            label: "three versions published, a far number",
            published: 3,
            asked: i64::from(u32::MAX),
        },
    ];

    for case in cases {
        let message_template_id = message_template(
            &emulator,
            &knowledge_base_id,
            case.label,
            "SMS",
            sms_content("No version created yet"),
        )
        .await;
        let published = publish_versions(
            &emulator,
            &knowledge_base_id,
            &message_template_id,
            case.published,
        )
        .await;
        let before = list_versions(&emulator, &knowledge_base_id, &message_template_id, "").await;

        let reply = activate(
            &emulator,
            &knowledge_base_id,
            &message_template_id,
            case.asked,
        )
        .await;
        assert_eq!(
            reply.status, 404,
            "case {:?}: expected HTTP 404 for a version that was never published, got HTTP {} \
             with body {}",
            case.label, reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, "ResourceNotFoundException",
            "case {:?}: expected error type ResourceNotFoundException, got {:?}",
            case.label, reply.error_type
        );
        assert_eq!(
            reply.string("/message"),
            MESSAGE_VERSION_NOT_FOUND,
            "case {:?}: expected the message {MESSAGE_VERSION_NOT_FOUND:?}, got {:?}",
            case.label,
            reply.string("/message")
        );

        // The refusal created nothing: the listing is what it was, and no version of this
        // template is activated.
        let after = list_versions(&emulator, &knowledge_base_id, &message_template_id, "").await;
        assert_eq!(
            listed_versions(&after),
            published,
            "case {:?}: the refused activation must create no version; expected {published:?}, \
             got {:?} (before the refusal: {:?})",
            case.label,
            listed_versions(&after),
            listed_versions(&before)
        );
        assert_eq!(
            listed_active_versions(&after),
            Vec::<i64>::new(),
            "case {:?}: no version was activated, so none may report isActive, got {:?}",
            case.label,
            listed_active_versions(&after)
        );

        // The same version is not readable either, nor deactivatable: one rule, every
        // operation that addresses a version.
        let read_back = read(
            &emulator,
            &knowledge_base_id,
            &format!("{message_template_id}:{}", case.asked),
        )
        .await;
        assert_eq!(
            read_back.status, 404,
            "case {:?}: a read of the same version expects HTTP 404, got HTTP {} with body {}",
            case.label, read_back.status, read_back.body
        );
        assert_eq!(
            read_back.string("/message"),
            MESSAGE_VERSION_NOT_FOUND,
            "case {:?}: a read of the same version expects the message \
             {MESSAGE_VERSION_NOT_FOUND:?}, got {:?}",
            case.label,
            read_back.string("/message")
        );
        let deactivated = deactivate(
            &emulator,
            &knowledge_base_id,
            &message_template_id,
            case.asked,
        )
        .await;
        assert_eq!(
            deactivated.status, 404,
            "case {:?}: a deactivation of the same version expects HTTP 404, got HTTP {} with \
             body {}",
            case.label, deactivated.status, deactivated.body
        );
        assert_eq!(
            deactivated.string("/message"),
            MESSAGE_VERSION_NOT_FOUND,
            "case {:?}: a deactivation of the same version expects the message \
             {MESSAGE_VERSION_NOT_FOUND:?}, got {:?}",
            case.label,
            deactivated.string("/message")
        );
    }

    // The modelled `Version` range (`min 1`) refuses a number below its bound before the
    // handler runs: that is a bad request, not a missing resource.
    let message_template_id = message_template(
        &emulator,
        &knowledge_base_id,
        "below the version bound",
        "SMS",
        sms_content("No version created yet"),
    )
    .await;
    let reply = activate(
        &emulator,
        &knowledge_base_id,
        &message_template_id,
        VERSION_BELOW_MIN,
    )
    .await;
    assert_eq!(
        reply.status, 400,
        "case {VERSION_BELOW_MIN:?}: a versionNumber below the modelled range expects HTTP 400, \
         got HTTP {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type, "ValidationException",
        "case {VERSION_BELOW_MIN:?}: expected error type ValidationException, got {:?}",
        reply.error_type
    );
}

/// The rule: a publish numbers the working copy as the template's next version, and
/// activating that version makes it the activated one — reported by every read of it, by
/// the `$ACTIVE_VERSION` qualifier and by the listing, while the working copy itself is no
/// version and reports neither member.
#[tokio::test]
async fn a_published_version_is_activated_and_read_back_as_active() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "activate-published").await;

    // One row per channel subtype the model declares, so the rule is asserted over the
    // whole value set rather than the one subtype a recording used.
    for (channel_subtype, content_member) in CHANNEL_SUBTYPES {
        let message_template_id = message_template(
            &emulator,
            &knowledge_base_id,
            &format!("published {channel_subtype}"),
            channel_subtype,
            content_for(channel_subtype),
        )
        .await;
        let working_copy = read(&emulator, &knowledge_base_id, &message_template_id).await;
        let working_copy_sha256 = working_copy.string("/messageTemplate/messageTemplateContentSha256");

        let published = publish(&emulator, &knowledge_base_id, &message_template_id, None).await;
        assert_eq!(
            published.status, 200,
            "case {channel_subtype:?}: expected HTTP 200 from the publish, got HTTP {} with body {}",
            published.status, published.body
        );
        assert_eq!(
            integer(&published, "/messageTemplate/versionNumber"),
            Some(VERSION_MIN),
            "case {channel_subtype:?}: a template's first version is numbered {VERSION_MIN}, got {:?}",
            integer(&published, "/messageTemplate/versionNumber")
        );
        assert_eq!(
            boolean(&published, "/messageTemplate/isActive"),
            Some(false),
            "case {channel_subtype:?}: a version is not activated by being published, got {:?}",
            boolean(&published, "/messageTemplate/isActive")
        );
        assert_eq!(
            published.string("/messageTemplate/messageTemplateArn"),
            expected_version_arn(&knowledge_base_id, &message_template_id, &VERSION_MIN.to_string()),
            "case {channel_subtype:?}: the published version reports the template's ARN with its \
             own qualifier, got {:?}",
            published.string("/messageTemplate/messageTemplateArn")
        );
        assert_eq!(
            published.string("/messageTemplate/messageTemplateContentSha256"),
            working_copy_sha256,
            "case {channel_subtype:?}: the version was published from the working copy, so it \
             carries the working copy's checksum {working_copy_sha256:?}, got {:?}",
            published.string("/messageTemplate/messageTemplateContentSha256")
        );
        assert!(
            published
                .field(&format!("/messageTemplate/content/{content_member}"))
                .is_some(),
            "case {channel_subtype:?}: the published version carries the content it was published \
             from under {content_member:?}, got {}",
            published.body
        );

        // Nothing is activated yet, so the managed qualifier names nothing.
        let before = read(
            &emulator,
            &knowledge_base_id,
            &format!("{message_template_id}:{QUALIFIER_ACTIVE}"),
        )
        .await;
        assert_eq!(
            before.status, 404,
            "case {channel_subtype:?}: {QUALIFIER_ACTIVE} names nothing while no version is \
             activated, got HTTP {} with body {}",
            before.status, before.body
        );

        let activated = activate(&emulator, &knowledge_base_id, &message_template_id, VERSION_MIN).await;
        assert_eq!(
            activated.status, 200,
            "case {channel_subtype:?}: expected HTTP 200 from the activation, got HTTP {} with \
             body {}",
            activated.status, activated.body
        );
        assert_eq!(
            activated.string("/messageTemplateArn"),
            expected_version_arn(&knowledge_base_id, &message_template_id, &VERSION_MIN.to_string()),
            "case {channel_subtype:?}: the activation reports the activated version's ARN, got {:?}",
            activated.string("/messageTemplateArn")
        );
        assert_eq!(
            activated.string("/messageTemplateId"),
            message_template_id,
            "case {channel_subtype:?}: the activation reports the template's id, got {:?}",
            activated.string("/messageTemplateId")
        );
        assert_eq!(
            integer(&activated, "/versionNumber"),
            Some(VERSION_MIN),
            "case {channel_subtype:?}: the activation reports the version it activated, got {:?}",
            integer(&activated, "/versionNumber")
        );

        // The write is observable through every read of that version.
        for qualifier in [VERSION_MIN.to_string(), QUALIFIER_ACTIVE.to_string()] {
            let read_back = read(
                &emulator,
                &knowledge_base_id,
                &format!("{message_template_id}:{qualifier}"),
            )
            .await;
            assert_eq!(
                read_back.status, 200,
                "case {channel_subtype:?}/{qualifier:?}: expected HTTP 200, got HTTP {} with body {}",
                read_back.status, read_back.body
            );
            assert_eq!(
                boolean(&read_back, "/messageTemplate/isActive"),
                Some(true),
                "case {channel_subtype:?}/{qualifier:?}: the activated version reports isActive \
                 true, got {:?}",
                boolean(&read_back, "/messageTemplate/isActive")
            );
            assert_eq!(
                integer(&read_back, "/messageTemplate/versionNumber"),
                Some(VERSION_MIN),
                "case {channel_subtype:?}/{qualifier:?}: expected versionNumber {VERSION_MIN}, got {:?}",
                integer(&read_back, "/messageTemplate/versionNumber")
            );
            assert_eq!(
                read_back.string("/messageTemplate/messageTemplateArn"),
                expected_version_arn(&knowledge_base_id, &message_template_id, &qualifier),
                "case {channel_subtype:?}/{qualifier:?}: the read reports the ARN with the \
                 qualifier it addressed, got {:?}",
                read_back.string("/messageTemplate/messageTemplateArn")
            );
        }

        // The listing answers out of the same records.
        let listing = list_versions(&emulator, &knowledge_base_id, &message_template_id, "").await;
        assert_eq!(
            listed_versions(&listing),
            vec![VERSION_MIN],
            "case {channel_subtype:?}: the listing reports the published version, got {:?}",
            listed_versions(&listing)
        );
        assert_eq!(
            listed_active_versions(&listing),
            vec![VERSION_MIN],
            "case {channel_subtype:?}: the listing reports the activated version, got {:?}",
            listed_active_versions(&listing)
        );
        assert_eq!(
            listing.string("/messageTemplateVersionSummaries/0/messageTemplateArn"),
            expected_version_arn(&knowledge_base_id, &message_template_id, &VERSION_MIN.to_string()),
            "case {channel_subtype:?}: the summary reports the version's ARN, got {:?}",
            listing.string("/messageTemplateVersionSummaries/0/messageTemplateArn")
        );
        assert_eq!(
            listing.string("/messageTemplateVersionSummaries/0/channelSubtype"),
            channel_subtype,
            "case {channel_subtype:?}: the summary reports the template's channel subtype, got {:?}",
            listing.string("/messageTemplateVersionSummaries/0/channelSubtype")
        );

        // The working copy is not a version: activating one changes neither of the two
        // members that only a version has.
        for working_copy_label in [
            message_template_id.clone(),
            format!("{message_template_id}:{QUALIFIER_LATEST}"),
        ] {
            let latest = read(&emulator, &knowledge_base_id, &working_copy_label).await;
            assert_eq!(
                boolean(&latest, "/messageTemplate/isActive"),
                None,
                "case {channel_subtype:?}/{working_copy_label:?}: the working copy is no version, \
                 so it reports no isActive, got {:?}",
                boolean(&latest, "/messageTemplate/isActive")
            );
            assert_eq!(
                integer(&latest, "/messageTemplate/versionNumber"),
                None,
                "case {channel_subtype:?}/{working_copy_label:?}: the working copy reports no \
                 versionNumber, got {:?}",
                integer(&latest, "/messageTemplate/versionNumber")
            );
        }
    }
}

/// The rule the operation documents: "After the version is activated, the previous active
/// version will be deactivated automatically" — so at most one version of a template is
/// activated, whichever order the activations come in, and re-activating the version that
/// is already active leaves it active.
#[tokio::test]
async fn activating_a_version_deactivates_the_one_that_was_active() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "activate-sequence").await;
    let message_template_id = message_template(
        &emulator,
        &knowledge_base_id,
        "sequenced",
        "SMS",
        sms_content("first"),
    )
    .await;
    let published = publish_versions(&emulator, &knowledge_base_id, &message_template_id, 4).await;
    assert_eq!(
        published,
        vec![1, 2, 3, 4],
        "versions are monotonically increasing from 1, got {published:?}"
    );

    // The rows differ along the order the activations come in: forward, backward, the one
    // already active, and back to the first.
    for asked in [2_i64, 4, 1, 1, 3] {
        let reply = activate(&emulator, &knowledge_base_id, &message_template_id, asked).await;
        assert_eq!(
            reply.status, 200,
            "case {asked:?}: expected HTTP 200, got HTTP {} with body {}",
            reply.status, reply.body
        );

        let listing = list_versions(&emulator, &knowledge_base_id, &message_template_id, "").await;
        assert_eq!(
            listed_active_versions(&listing),
            vec![asked],
            "case {asked:?}: exactly the activated version reports isActive, got {:?}",
            listed_active_versions(&listing)
        );
        assert_eq!(
            listed_versions(&listing),
            published,
            "case {asked:?}: an activation publishes no version, got {:?}",
            listed_versions(&listing)
        );

        let active = read(
            &emulator,
            &knowledge_base_id,
            &format!("{message_template_id}:{QUALIFIER_ACTIVE}"),
        )
        .await;
        assert_eq!(
            integer(&active, "/messageTemplate/versionNumber"),
            Some(asked),
            "case {asked:?}: {QUALIFIER_ACTIVE} resolves to the activated version, got {:?}",
            integer(&active, "/messageTemplate/versionNumber")
        );
    }

    // The other half of the pair: "After the version is deactivated, you can no longer use
    // the $ACTIVE_VERSION qualifier to reference the version in active status." The rows
    // differ along whether the version deactivated is the activated one.
    let active_version = 3_i64;
    for asked in [1_i64, active_version, active_version] {
        let still_active = (asked != active_version).then_some(active_version);
        let reply = deactivate(&emulator, &knowledge_base_id, &message_template_id, asked).await;
        assert_eq!(
            reply.status, 200,
            "case \"deactivate {asked}\": expected HTTP 200, got HTTP {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            integer(&reply, "/versionNumber"),
            Some(asked),
            "case \"deactivate {asked}\": the deactivation reports the version it deactivated, \
             got {:?}",
            integer(&reply, "/versionNumber")
        );

        let listing = list_versions(&emulator, &knowledge_base_id, &message_template_id, "").await;
        assert_eq!(
            listed_active_versions(&listing),
            still_active.into_iter().collect::<Vec<i64>>(),
            "case \"deactivate {asked}\": deactivating a version that was not the activated one \
             leaves that one active, got {:?}",
            listed_active_versions(&listing)
        );

        let active = read(
            &emulator,
            &knowledge_base_id,
            &format!("{message_template_id}:{QUALIFIER_ACTIVE}"),
        )
        .await;
        let expected_status = if still_active.is_some() { 200 } else { 404 };
        assert_eq!(
            active.status, expected_status,
            "case \"deactivate {asked}\": {QUALIFIER_ACTIVE} names the activated version while \
             there is one and nothing once there is none; expected HTTP {expected_status}, got \
             HTTP {} with body {}",
            active.status, active.body
        );
    }

    // And the versions are all still there: a deactivation publishes and forgets nothing.
    let listing = list_versions(&emulator, &knowledge_base_id, &message_template_id, "").await;
    assert_eq!(
        listed_versions(&listing),
        published,
        "case \"after the deactivations\": every published version is still listed, got {:?}",
        listed_versions(&listing)
    );
}

/// The rule: a published version is immutable — "Any modification will only apply to the
/// message template that is referenced by the $LATEST qualifier. The fields for all
/// available versions will remain unchanged" — so a later update changes the working copy
/// alone, and the next publish numbers what the working copy holds then.
#[tokio::test]
async fn an_update_applies_to_the_working_copy_and_leaves_published_versions_unchanged() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "immutable-versions").await;
    let message_template_id = message_template(
        &emulator,
        &knowledge_base_id,
        "immutable",
        "SMS",
        sms_content("as published"),
    )
    .await;

    let first = publish(&emulator, &knowledge_base_id, &message_template_id, None).await;
    let published_sha256 = first.string("/messageTemplate/messageTemplateContentSha256");

    // Each row changes one of the members the checksum is calculated over, plus one it is
    // not, so the rule is asserted over the members the update accepts rather than one.
    let changed = update(
        &emulator,
        &knowledge_base_id,
        &message_template_id,
        json!({
            "content": sms_content("after the update"),
            "language": "ja_JP",
            "defaultAttributes": { "customAttributes": { "orderId": "4711" } },
        }),
    )
    .await;
    assert_eq!(
        changed.status, 200,
        "case \"update the working copy\": expected HTTP 200, got HTTP {} with body {}",
        changed.status, changed.body
    );
    assert_eq!(
        changed.string("/messageTemplate/language"),
        "ja_JP",
        "case \"update the working copy\": the update reports what it wrote, got {:?}",
        changed.string("/messageTemplate/language")
    );

    let version = read(
        &emulator,
        &knowledge_base_id,
        &format!("{message_template_id}:{VERSION_MIN}"),
    )
    .await;
    assert_eq!(
        version.string("/messageTemplate/content/sms/body/plainText/content"),
        "as published",
        "case \"read the published version\": a version keeps the content it was published \
         from, got {:?}",
        version.string("/messageTemplate/content/sms/body/plainText/content")
    );
    assert_eq!(
        version.string("/messageTemplate/messageTemplateContentSha256"),
        published_sha256,
        "case \"read the published version\": a version keeps the checksum it was published \
         with, got {:?}",
        version.string("/messageTemplate/messageTemplateContentSha256")
    );
    assert!(
        version.field("/messageTemplate/language").is_none(),
        "case \"read the published version\": the published version carried no language, and the \
         update that set one applies to {QUALIFIER_LATEST} alone, got {}",
        version.body
    );

    let latest = read(&emulator, &knowledge_base_id, &message_template_id).await;
    assert_eq!(
        latest.string("/messageTemplate/content/sms/body/plainText/content"),
        "after the update",
        "case \"read the working copy\": the update applies to {QUALIFIER_LATEST}, got {:?}",
        latest.string("/messageTemplate/content/sms/body/plainText/content")
    );
    assert_ne!(
        latest.string("/messageTemplate/messageTemplateContentSha256"),
        published_sha256,
        "case \"read the working copy\": a change to the content changes the checksum, got {:?}",
        latest.string("/messageTemplate/messageTemplateContentSha256")
    );

    // The next publish numbers what the working copy holds NOW.
    let second = publish(&emulator, &knowledge_base_id, &message_template_id, None).await;
    assert_eq!(
        integer(&second, "/messageTemplate/versionNumber"),
        Some(2),
        "case \"publish the changed working copy\": expected version 2, got {:?}",
        integer(&second, "/messageTemplate/versionNumber")
    );
    assert_eq!(
        second.string("/messageTemplate/content/sms/body/plainText/content"),
        "after the update",
        "case \"publish the changed working copy\": the new version carries the changed content, \
         got {:?}",
        second.string("/messageTemplate/content/sms/body/plainText/content")
    );
    assert_eq!(
        second.string("/messageTemplate/language"),
        "ja_JP",
        "case \"publish the changed working copy\": the new version carries the changed language, \
         got {:?}",
        second.string("/messageTemplate/language")
    );
}

/// The rule the operation documents: "If multiple CreateMessageTemplateVersion requests are
/// made while the message template remains the same, only the first invocation creates a
/// new version and the succeeding requests will return the same response as the first
/// invocation", and a supplied `messageTemplateContentSha256` that is not the working
/// copy's is an error that publishes nothing.
#[tokio::test]
async fn a_publish_of_an_unchanged_working_copy_answers_with_the_version_it_already_published() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "publish-idempotence").await;
    let message_template_id = message_template(
        &emulator,
        &knowledge_base_id,
        "repeated",
        "SMS",
        sms_content("unchanged"),
    )
    .await;

    let first = publish(&emulator, &knowledge_base_id, &message_template_id, None).await;
    let content_sha256 = first.string("/messageTemplate/messageTemplateContentSha256");

    // The rows differ along what the repeated publish supplies: nothing, and the checksum
    // the working copy carries.
    for supplied in [None, Some(content_sha256.as_str())] {
        let repeated = publish(
            &emulator,
            &knowledge_base_id,
            &message_template_id,
            supplied,
        )
        .await;
        assert_eq!(
            repeated.status, 200,
            "case {supplied:?}: expected HTTP 200, got HTTP {} with body {}",
            repeated.status, repeated.body
        );
        assert_eq!(
            integer(&repeated, "/messageTemplate/versionNumber"),
            Some(VERSION_MIN),
            "case {supplied:?}: a repeated publish of the same working copy answers with the \
             version it already published, got {:?}",
            integer(&repeated, "/messageTemplate/versionNumber")
        );
        let listing = list_versions(&emulator, &knowledge_base_id, &message_template_id, "").await;
        assert_eq!(
            listed_versions(&listing),
            vec![VERSION_MIN],
            "case {supplied:?}: no second version was numbered, got {:?}",
            listed_versions(&listing)
        );
    }

    // A checksum that is not the working copy's: the row forms are a checksum of another
    // content, and one of the modelled form that names nothing.
    let other_sha256 = read(&emulator, &knowledge_base_id, &message_template_id)
        .await
        .string("/messageTemplate/messageTemplateContentSha256");
    assert_eq!(
        other_sha256, content_sha256,
        "the working copy is unchanged, so its checksum is the published one"
    );
    for stale in ["0".repeat(64), "deadBEEF".to_string()] {
        let refused = publish(
            &emulator,
            &knowledge_base_id,
            &message_template_id,
            Some(&stale),
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case {stale:?}: a checksum that is not the working copy's expects HTTP 409, got HTTP \
             {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ConflictException",
            "case {stale:?}: expected error type ConflictException, got {:?}",
            refused.error_type
        );
        assert!(
            refused.string("/message").contains(&content_sha256),
            "case {stale:?}: the refusal states the checksum the working copy carries, got {:?}",
            refused.string("/message")
        );
        let listing = list_versions(&emulator, &knowledge_base_id, &message_template_id, "").await;
        assert_eq!(
            listed_versions(&listing),
            vec![VERSION_MIN],
            "case {stale:?}: the refused publish must number no version, got {:?}",
            listed_versions(&listing)
        );
    }

    // Once the working copy changes, the checksum of what WAS published is stale too, and
    // the new one publishes the next version.
    update(
        &emulator,
        &knowledge_base_id,
        &message_template_id,
        json!({ "content": sms_content("changed") }),
    )
    .await;
    let stale = publish(
        &emulator,
        &knowledge_base_id,
        &message_template_id,
        Some(&content_sha256),
    )
    .await;
    assert_eq!(
        stale.status, 409,
        "case \"the checksum of the previous working copy\": expected HTTP 409, got HTTP {} with \
         body {}",
        stale.status, stale.body
    );
    let changed_sha256 = read(&emulator, &knowledge_base_id, &message_template_id)
        .await
        .string("/messageTemplate/messageTemplateContentSha256");
    let next = publish(
        &emulator,
        &knowledge_base_id,
        &message_template_id,
        Some(&changed_sha256),
    )
    .await;
    assert_eq!(
        integer(&next, "/messageTemplate/versionNumber"),
        Some(2),
        "case \"the checksum of the changed working copy\": expected version 2, got {:?}",
        integer(&next, "/messageTemplate/versionNumber")
    );
}

/// The rule: the listing reports the published versions in ascending `versionNumber`
/// order, one page of at most `maxResults` at a time, and a token this service did not mint
/// names no position.
#[tokio::test]
async fn the_version_listing_pages_in_ascending_version_order() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "version-paging").await;
    let message_template_id = message_template(
        &emulator,
        &knowledge_base_id,
        "paged",
        "SMS",
        sms_content("page one"),
    )
    .await;
    let published = publish_versions(&emulator, &knowledge_base_id, &message_template_id, 5).await;

    // The rows differ along the page size: the published bound, a size that splits the set,
    // the exact count, one past it and the other published bound.
    for page_size in [MAX_RESULTS_MIN, 2, 5, 6, MAX_RESULTS_MAX] {
        let mut seen: Vec<i64> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let page = list_versions(&emulator, &knowledge_base_id, &message_template_id, &query).await;
            assert_eq!(
                page.status, 200,
                "case {page_size:?}: expected HTTP 200, got HTTP {} with body {}",
                page.status, page.body
            );
            let numbers = listed_versions(&page);
            assert!(
                numbers.len() as i64 <= page_size,
                "case {page_size:?}: a page carries at most maxResults entries, got {numbers:?}"
            );
            seen.extend(numbers);
            let next_token = page.string("/nextToken");
            if next_token.is_empty() {
                break;
            }
            query = format!("?maxResults={page_size}&nextToken={next_token}");
        }
        assert_eq!(
            seen, published,
            "case {page_size:?}: paging reports every version once, in ascending order; expected \
             {published:?}, got {seen:?}"
        );
    }

    // A token this service did not mint, and the page sizes outside the modelled range.
    let refused = list_versions(
        &emulator,
        &knowledge_base_id,
        &message_template_id,
        "?nextToken=not-a-token-this-service-minted",
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case \"a token this service did not mint\": expected HTTP 400, got HTTP {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type, "ValidationException",
        "case \"a token this service did not mint\": expected error type ValidationException, got \
         {:?}",
        refused.error_type
    );
    for page_size in [MAX_RESULTS_BELOW_MIN, MAX_RESULTS_ABOVE_MAX] {
        let reply = list_versions(
            &emulator,
            &knowledge_base_id,
            &message_template_id,
            &format!("?maxResults={page_size}"),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {page_size:?}: a maxResults outside the modelled range expects HTTP 400, got \
             HTTP {} with body {}",
            reply.status, reply.body
        );
    }
}

/// Every operation of this plane, with the route it is reached through and whether its
/// label may carry a version qualifier.
#[derive(Debug, Clone, Copy)]
struct PlaneOperation {
    label: &'static str,
    method: &'static str,
    /// The path suffix after the message-template label, if any.
    suffix: &'static str,
    body: bool,
    /// Whether the operation's own documentation admits a version qualifier on the label.
    admits_qualifier: bool,
}

/// The operations the version plane is reached through.
const PLANE_OPERATIONS: [PlaneOperation; 6] = [
    PlaneOperation {
        label: "ActivateMessageTemplate",
        method: "POST",
        suffix: "/activate",
        body: true,
        admits_qualifier: false,
    },
    PlaneOperation {
        label: "DeactivateMessageTemplate",
        method: "POST",
        suffix: "/deactivate",
        body: true,
        admits_qualifier: false,
    },
    PlaneOperation {
        label: "CreateMessageTemplateVersion",
        method: "POST",
        suffix: "/versions",
        body: true,
        admits_qualifier: false,
    },
    PlaneOperation {
        label: "ListMessageTemplateVersions",
        method: "GET",
        suffix: "/versions",
        body: false,
        admits_qualifier: false,
    },
    PlaneOperation {
        label: "UpdateMessageTemplate",
        method: "POST",
        suffix: "",
        body: true,
        admits_qualifier: false,
    },
    PlaneOperation {
        label: "GetMessageTemplate",
        method: "GET",
        suffix: "",
        body: false,
        admits_qualifier: true,
    },
];

/// Send one operation of the plane against a (knowledge base, message template) label pair.
async fn send_plane_operation(
    emulator: &Emulator,
    operation: PlaneOperation,
    knowledge_base: &str,
    message_template: &str,
) -> Reply {
    let uri = format!(
        "/knowledgeBases/{}/messageTemplates/{}{}",
        label(knowledge_base),
        label(message_template),
        operation.suffix
    );
    let body = operation
        .body
        .then(|| json!({ "versionNumber": VERSION_MIN }));
    emulator.send(operation.method, &uri, body).await
}

/// The rule: every operation of this plane resolves the knowledge base and then the
/// template inside it, so a label that names no resource of this account is refused with
/// HTTP 404 naming the resource that is missing — and the operations whose documentation
/// says their label "cannot contain any qualifier" refuse one that carries any.
#[tokio::test]
async fn every_version_operation_resolves_its_label_the_same_way() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "label-resolution").await;
    let elsewhere_id = knowledge_base(&emulator, "label-resolution-elsewhere").await;
    let message_template_id = message_template(
        &emulator,
        &knowledge_base_id,
        "resolved",
        "SMS",
        sms_content("resolved"),
    )
    .await;
    let elsewhere_template_id = message_template(
        &emulator,
        &elsewhere_id,
        "resolved elsewhere",
        "SMS",
        sms_content("resolved elsewhere"),
    )
    .await;

    for operation in PLANE_OPERATIONS {
        // The knowledge base is resolved first, so a label pair whose knowledge base this
        // account does not hold reports the knowledge base as missing.
        for (case, knowledge_base_label) in [
            ("a knowledge base id this account does not hold", ABSENT_UUID.to_string()),
            (
                "a knowledge base ARN of another account",
                wisdom_arn(
                    "aws",
                    REGION,
                    OTHER_ACCOUNT,
                    &format!("knowledge-base/{knowledge_base_id}"),
                ),
            ),
        ] {
            let reply = send_plane_operation(
                &emulator,
                operation,
                &knowledge_base_label,
                &message_template_id,
            )
            .await;
            assert_eq!(
                reply.status, 404,
                "case {}/{case:?}: expected HTTP 404, got HTTP {} with body {}",
                operation.label, reply.status, reply.body
            );
            assert_eq!(
                reply.string("/message"),
                MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
                "case {}/{case:?}: expected the message {MESSAGE_KNOWLEDGE_BASE_NOT_FOUND:?}, got \
                 {:?}",
                operation.label,
                reply.string("/message")
            );
        }

        // Then the template inside it: an id this knowledge base holds no template under,
        // another knowledge base's template, and an ARN of another account.
        for (case, template_label) in [
            ("a template id this knowledge base does not hold", ABSENT_UUID.to_string()),
            ("another knowledge base's template", elsewhere_template_id.clone()),
            (
                "a template ARN of another account",
                wisdom_arn(
                    "aws",
                    REGION,
                    OTHER_ACCOUNT,
                    &format!("message-template/{knowledge_base_id}/{message_template_id}"),
                ),
            ),
        ] {
            let reply =
                send_plane_operation(&emulator, operation, &knowledge_base_id, &template_label).await;
            assert_eq!(
                reply.status, 404,
                "case {}/{case:?}: expected HTTP 404, got HTTP {} with body {}",
                operation.label, reply.status, reply.body
            );
            assert_eq!(
                reply.string("/message"),
                MESSAGE_TEMPLATE_NOT_FOUND,
                "case {}/{case:?}: expected the message {MESSAGE_TEMPLATE_NOT_FOUND:?}, got {:?}",
                operation.label,
                reply.string("/message")
            );
        }

        // A label carrying a qualifier: refused by the operations that document it as a
        // label that cannot carry one, and resolved by the read that documents that it can.
        for qualifier in [QUALIFIER_LATEST, QUALIFIER_ACTIVE, "1"] {
            let template_label = format!("{message_template_id}:{qualifier}");
            let reply =
                send_plane_operation(&emulator, operation, &knowledge_base_id, &template_label).await;
            if operation.admits_qualifier {
                assert_ne!(
                    reply.error_type, "ValidationException",
                    "case {}/{qualifier:?}: this operation's label may carry a qualifier, got {}",
                    operation.label, reply.body
                );
                continue;
            }
            assert_eq!(
                reply.status, 400,
                "case {}/{qualifier:?}: this operation's label cannot carry a qualifier, so \
                 expected HTTP 400, got HTTP {} with body {}",
                operation.label, reply.status, reply.body
            );
            assert_eq!(
                reply.error_type, "ValidationException",
                "case {}/{qualifier:?}: expected error type ValidationException, got {:?}",
                operation.label, reply.error_type
            );
            assert!(
                reply.string("/message").contains("messageTemplateId"),
                "case {}/{qualifier:?}: the refusal names the member that carried the qualifier, \
                 got {:?}",
                operation.label,
                reply.string("/message")
            );
        }
    }

    // None of those refusals published or activated anything.
    let listing = list_versions(&emulator, &knowledge_base_id, &message_template_id, "").await;
    assert_eq!(
        listed_versions(&listing),
        Vec::<i64>::new(),
        "case \"after every refusal\": the refused requests must publish no version, got {:?}",
        listed_versions(&listing)
    );
}

/// The rule: one message template holds at most the store's version ceiling, past which a
/// publish is the modelled `ServiceQuotaExceededException` (HTTP 402) and numbers nothing.
#[tokio::test]
async fn a_publish_past_the_version_ceiling_is_refused() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "version-ceiling").await;
    let message_template_id = message_template(
        &emulator,
        &knowledge_base_id,
        "ceiling",
        "SMS",
        sms_content("version 1"),
    )
    .await;
    let published = publish_versions(
        &emulator,
        &knowledge_base_id,
        &message_template_id,
        MAX_MESSAGE_TEMPLATE_VERSIONS_PER_TEMPLATE,
    )
    .await;
    assert_eq!(
        published.len(),
        MAX_MESSAGE_TEMPLATE_VERSIONS_PER_TEMPLATE,
        "case \"up to the ceiling\": every publish up to the ceiling must be numbered, got \
         {published:?}"
    );

    let changed = update(
        &emulator,
        &knowledge_base_id,
        &message_template_id,
        json!({ "content": sms_content("past the ceiling") }),
    )
    .await;
    assert_eq!(
        changed.status, 200,
        "case \"past the ceiling\": the working copy must change before the publish, got HTTP {} \
         with body {}",
        changed.status, changed.body
    );
    let refused = publish(&emulator, &knowledge_base_id, &message_template_id, None).await;
    assert_eq!(
        refused.status, 402,
        "case \"past the ceiling\": expected HTTP 402, got HTTP {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type, "ServiceQuotaExceededException",
        "case \"past the ceiling\": expected error type ServiceQuotaExceededException, got {:?}",
        refused.error_type
    );
    let listing = list_versions(
        &emulator,
        &knowledge_base_id,
        &message_template_id,
        &format!("?maxResults={MAX_RESULTS_MAX}"),
    )
    .await;
    assert_eq!(
        listed_versions(&listing),
        published,
        "case \"past the ceiling\": the refused publish numbers nothing, got {:?}",
        listed_versions(&listing)
    );
}

/// The rule: a version exists only inside the template that published it, which exists only
/// inside its knowledge base — so deleting the knowledge base takes the versions with it.
#[tokio::test]
async fn deleting_the_knowledge_base_forgets_the_versions_of_its_templates() {
    let emulator = Emulator::new();
    let knowledge_base_id = knowledge_base(&emulator, "doomed-versions").await;
    let message_template_id = message_template(
        &emulator,
        &knowledge_base_id,
        "doomed",
        "SMS",
        sms_content("doomed"),
    )
    .await;
    let published = publish_versions(&emulator, &knowledge_base_id, &message_template_id, 2).await;
    let activated = activate(&emulator, &knowledge_base_id, &message_template_id, published[1]).await;
    assert_eq!(
        activated.status, 200,
        "case \"activate before the delete\": expected HTTP 200, got HTTP {} with body {}",
        activated.status, activated.body
    );

    let deleted = emulator
        .send(
            "DELETE",
            &format!("/knowledgeBases/{}", label(&knowledge_base_id)),
            None,
        )
        .await;
    assert!(
        (200..300).contains(&deleted.status),
        "case \"delete the knowledge base\": expected a success status, got HTTP {} with body {}",
        deleted.status,
        deleted.body
    );

    for case in ["the listing", "the activation"] {
        let reply = if case == "the listing" {
            list_versions(&emulator, &knowledge_base_id, &message_template_id, "").await
        } else {
            activate(&emulator, &knowledge_base_id, &message_template_id, published[0]).await
        };
        assert_eq!(
            reply.status, 404,
            "case {case:?}: the knowledge base is gone, so expected HTTP 404, got HTTP {} with \
             body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.string("/message"),
            MESSAGE_KNOWLEDGE_BASE_NOT_FOUND,
            "case {case:?}: expected the message {MESSAGE_KNOWLEDGE_BASE_NOT_FOUND:?}, got {:?}",
            reply.string("/message")
        );
    }
}
