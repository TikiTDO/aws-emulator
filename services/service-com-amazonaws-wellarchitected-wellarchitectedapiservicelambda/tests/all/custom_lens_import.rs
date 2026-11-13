// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the custom lens a request imports: `ImportLens` and the
//! read that proves what it recorded, `ExportLens`.
//!
//! The rules pinned here, each over a table of inputs rather than one recorded request:
//!
//! 1. `ImportLens` RECORDS the lens document it is given, under a minted lens ARN, and
//!    answers the import's `ImportLensStatus`; `ExportLens` answers that same document
//!    back from the moment the import is ACCEPTED. Rows differ along the SHAPE of the
//!    document — how many pillars, questions, choices and risk rules it defines, and
//!    whether it carries a description — because the document is the resource, so every
//!    shape of it must round-trip.
//! 2. A payload that is not a lens imports NOTHING and is refused 400
//!    `ValidationException`, with `CANNOT_PARSE` when it is not JSON at all and
//!    `FIELD_VALIDATION_FAILED` when it is JSON that breaks a rule of the lens JSON
//!    format specification.
//! 3. A `LensAlias` that is an ARN updates THAT lens; a request without one creates
//!    another lens. An unknown custom ARN is a 404 and an Amazon Web Services official
//!    lens is refused, on both the import and the export side.
//! 4. The import is idempotent per `ClientRequestToken`: the same token with the same
//!    parameters reports the original import, and with different parameters is a 409.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client is the shared `super::workload_harness`.
//!
//! The wire helpers here (`lens_document`, `import`, `export`, `imported_arn`,
//! `wait_for_the_import_to_land`) are `pub(crate)` because the version suite,
//! `super::custom_lens_versions`, publishes versions of the lenses THIS suite's import
//! helper records — one spelling of each lens request for both.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use super::workload_harness::{encode_path_segment, Answer, Client};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// A custom lens ARN that is well-formed but names no imported lens.
pub(crate) const ABSENT_LENS_ARN: &str =
    "arn:aws:wellarchitected:us-west-2:000000000000:lens/0123456789abcdef0123456789abcdef";

/// A lens document, as the lens JSON format specification the `ImportLens` doc points at
/// defines one: a `schemaVersion`, a `name`, an optional `description` and the pillars,
/// each pillar's questions, each question's choices and the risk rules that map the
/// selected choices to a risk, the last of which carries the `default` condition.
///
/// `pillars` is `(pillar id, questions, choices per question)`, so a row can vary the
/// document's shape without spelling the whole document out.
pub(crate) fn lens_document(
    name: &str,
    description: Option<&str>,
    pillars: &[(&str, usize, usize)],
) -> Value {
    let pillars_json: Vec<Value> = pillars
        .iter()
        .enumerate()
        .map(|(index, (pillar_id, questions, choices))| {
            let questions_json: Vec<Value> = (0..*questions)
                .map(|question| {
                    let choice_ids: Vec<String> = (0..*choices)
                        .map(|choice| format!("{pillar_id}_q{question}_c{choice}"))
                        .collect();
                    let choices_json: Vec<Value> = choice_ids
                        .iter()
                        .map(|choice_id| {
                            json!({ "id": choice_id, "title": format!("Choice {choice_id}") })
                        })
                        .collect();
                    let mut rules: Vec<Value> = choice_ids
                        .iter()
                        .map(|choice_id| json!({ "condition": choice_id, "risk": "NO_RISK" }))
                        .collect();
                    rules.push(json!({ "condition": "default", "risk": "HIGH_RISK" }));
                    json!({
                        "id": format!("{pillar_id}_q{question}"),
                        "title": format!("Question {question} of {pillar_id}?"),
                        "description": "what this question asks about",
                        "choices": choices_json,
                        "riskRules": rules,
                    })
                })
                .collect();
            json!({
                "id": pillar_id,
                "name": format!("Pillar {index}: {pillar_id}"),
                "questions": questions_json,
            })
        })
        .collect();
    let mut document = json!({
        "schemaVersion": "2021-11-01",
        "name": name,
        "pillars": pillars_json,
    });
    if let Some(description) = description {
        document["description"] = json!(description);
    }
    document
}

/// `PUT /importLens`, the wire form of `ImportLens`.
pub(crate) async fn import(
    client: &Client,
    document: &str,
    alias: Option<&str>,
    token: &str,
    tags: Option<Value>,
) -> Answer {
    let mut body = json!({ "JSONString": document, "ClientRequestToken": token });
    if let Some(alias) = alias {
        body["LensAlias"] = json!(alias);
    }
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    client.send("PUT", "/importLens", Some(body)).await
}

/// `GET /lenses/{LensAlias}/export`. The alias is percent-encoded because a custom lens
/// is named by its ARN, which carries `:` and `/` inside ONE path segment.
pub(crate) async fn export(client: &Client, alias: &str, version: Option<&str>) -> Answer {
    let path = format!("/lenses/{}/export", encode_path_segment(alias));
    let uri = match version {
        Some(version) => format!("{path}?LensVersion={version}"),
        None => path,
    };
    client.send("GET", &uri, None).await
}

pub(crate) fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// The `LensArn` a successful import answered with, checked against the form the
/// `LensAlias` doc gives for a custom lens: `…:lens/<32 hex>` under this emulator's
/// account and the request's Region.
pub(crate) fn imported_arn(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ImportLens 200, got {answer:?}"
    );
    let arn = member(answer, "LensArn").to_string();
    let expected_prefix = format!(
        "arn:aws:wellarchitected:us-west-2:{}:lens/",
        engine::emulator_account_id()
    );
    let id = arn.strip_prefix(&expected_prefix).unwrap_or_default();
    assert!(
        id.len() == 32 && id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "case {case:?}: LensArn must be {expected_prefix}<32 hex>, got {arn:?}"
    );
    arn
}

/// Wait for an accepted import's job to report `COMPLETE`. The span is the service's own
/// ([`state::LENS_IMPORT_DURATION`]), plus a margin, and it is non-zero on purpose: that
/// is what makes the `IN_PROGRESS` status observable to a caller. The lens's CONTENT does
/// not wait for it — the accepting call recorded the document — so this is only ever
/// needed before a read of the import's status.
pub(crate) fn wait_for_the_import_to_land() {
    assert!(
        !state::LENS_IMPORT_DURATION.is_zero(),
        "an import that lands instantly would never report IN_PROGRESS, got {:?}",
        state::LENS_IMPORT_DURATION
    );
    // A test, not a handler: the handler never sleeps for this instant, it compares the
    // stored instant with the clock on every read.
    std::thread::sleep(state::LENS_IMPORT_DURATION + Duration::from_millis(250));
}

/// Rule 1. Every shape of lens document is recorded under its own ARN and exported back
/// VERBATIM from the moment the import is accepted, while the import's own job reports
/// `IN_PROGRESS` until its span has passed.
#[tokio::test]
async fn an_imported_lens_document_is_recorded_and_exported_back_verbatim() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "one-pillar-one-question-two-choices",
            lens_document("Minimal lens", None, &[("governance", 1, 2)]),
        ),
        (
            "described",
            lens_document(
                "Described lens",
                Some("the questions company policy adds to a review"),
                &[("security", 2, 3)],
            ),
        ),
        (
            "several-pillars",
            lens_document(
                "Broad lens",
                None,
                &[("operations", 1, 1), ("security", 3, 2), ("cost", 2, 4)],
            ),
        ),
        (
            "many-questions-one-pillar",
            lens_document("Deep lens", None, &[("reliability", 12, 2)]),
        ),
        (
            "non-ascii-name",
            lens_document("Lentille personnalisée", Some("accents dans le nom"), &[
                ("durabilité", 1, 2),
            ]),
        ),
    ];

    let client = Client::new();
    let mut imported: Vec<(&str, String, String)> = Vec::new();
    for (case, document) in &cases {
        let document = document.to_string();
        let answer = import(&client, &document, None, &format!("tok-{case}"), None).await;
        let arn = imported_arn(&answer, case);
        assert_eq!(
            member(&answer, "Status"),
            "IN_PROGRESS",
            "case {case:?}: an accepted import is still in progress, got {answer:?}"
        );

        // The import's job is still in progress, and the document it carried is already
        // the lens's content: "Import a new custom lens" is what the accepting call did.
        let early = export(&client, &arn, None).await;
        assert_eq!(
            early.status, 200,
            "case {case:?}: an accepted import is exportable at once, got {early:?}"
        );
        assert_eq!(
            member(&early, "LensJSON"),
            document.as_str(),
            "case {case:?}: ExportLens must answer the document as imported, got {early:?}"
        );
        imported.push((case, arn, document));
    }

    let arns: BTreeSet<&str> = imported.iter().map(|(_, arn, _)| arn.as_str()).collect();
    assert_eq!(
        arns.len(),
        cases.len(),
        "each import without a LensAlias must mint its own lens, got {arns:?}"
    );

    wait_for_the_import_to_land();

    for (case, arn, document) in &imported {
        let exported = export(&client, arn, None).await;
        assert_eq!(
            exported.status, 200,
            "case {case:?}: ExportLens must return the imported lens, got {exported:?}"
        );
        assert_eq!(
            member(&exported, "LensJSON"),
            document.as_str(),
            "case {case:?}: ExportLens must answer the document as imported, got {exported:?}"
        );

        // The same token with the same parameters replays the original import, whose job
        // has now finished, so the status it reports has moved on.
        let replay = import(&client, document, None, &format!("tok-{case}"), None).await;
        assert_eq!(
            member(&replay, "LensArn"),
            arn.as_str(),
            "case {case:?}: a replayed import must name the original lens, got {replay:?}"
        );
        assert_eq!(
            member(&replay, "Status"),
            "COMPLETE",
            "case {case:?}: a landed import reports COMPLETE, got {replay:?}"
        );

        // Reading it twice answers the same thing: ExportLens writes nothing.
        let again = export(&client, arn, None).await;
        assert_eq!(
            again.body, exported.body,
            "case {case:?}: a second ExportLens must answer the same lens, got {again:?}"
        );

        // A version of the lens exists only once one is published from the draft, which
        // no request here did — `super::custom_lens_versions` covers the publish.
        let versioned = export(&client, arn, Some("2024-01-01")).await;
        assert_eq!(
            versioned.status, 404,
            "case {case:?}: an unpublished LensVersion must not resolve, got {versioned:?}"
        );
        assert_eq!(
            versioned.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {versioned:?}"
        );
    }
}

/// One row of the definition table: a case name, the lens name, and the pillars as
/// `(pillar id, questions, choices per question)`.
type DefinitionCase<'a> = (&'a str, &'a str, Vec<(&'a str, usize, usize)>);

/// Rule 1, continued: the lens document the import recorded is the one the service reads
/// its name, pillars and questions from, so the parsed definition matches the document
/// the request sent — checked over the same table of shapes.
#[tokio::test]
async fn the_recorded_definition_matches_the_document_that_was_imported() {
    let cases: Vec<DefinitionCase<'_>> = vec![
        ("one-pillar", "One pillar lens", vec![("governance", 1, 2)]),
        (
            "three-pillars",
            "Three pillar lens",
            vec![("operations", 2, 2), ("security", 5, 3), ("cost", 1, 4)],
        ),
        (
            "fifty-questions",
            "Fifty question lens",
            vec![("reliability", 25, 2), ("sustainability", 25, 3)],
        ),
    ];

    for (case, name, pillars) in cases {
        let client = Client::new();
        let document = lens_document(name, Some("a described lens"), &pillars).to_string();
        let answer = import(&client, &document, None, "tok-definition", None).await;
        let arn = imported_arn(&answer, case);

        let expected_questions: usize = pillars.iter().map(|(_, questions, _)| questions).sum();
        let expected_pillars: Vec<String> = pillars
            .iter()
            .map(|(pillar_id, _, _)| (*pillar_id).to_string())
            .collect();

        let definition = state::parse_lens_json(&document)
            .unwrap_or_else(|rejection| panic!("case {case:?}: {rejection:?}"));
        assert_eq!(
            definition.name, name,
            "case {case:?}: the lens takes its name from the document, got {definition:?}"
        );
        assert_eq!(
            definition.schema_version,
            state::LENS_SCHEMA_VERSION,
            "case {case:?}: the document declares the format version, got {definition:?}"
        );
        assert_eq!(
            definition.question_count(),
            expected_questions,
            "case {case:?}: expected {expected_questions} questions, got {definition:?}"
        );
        assert_eq!(
            definition.pillar_ids(),
            expected_pillars,
            "case {case:?}: expected pillars {expected_pillars:?}, got {definition:?}"
        );

        // And the document that was stored under the ARN is the one that definition came
        // from.
        wait_for_the_import_to_land();
        let exported = export(&client, &arn, None).await;
        assert_eq!(
            member(&exported, "LensJSON"),
            document.as_str(),
            "case {case:?}: the stored document must be the one imported, got {exported:?}"
        );
    }
}

/// Rule 2. A payload that is not a lens is refused with the modelled
/// `ValidationException` — and it imports nothing, which the reused `ClientRequestToken`
/// below proves: a refused request recorded no token, so the same token still imports a
/// valid document.
///
/// Either way the request carried a document that is not a lens, so it never became what it
/// had to be: `Reason` is carried unclassified and no field finding is reported. A document
/// the service cannot read as JSON at all is answered by naming `JSONString` alone; one it
/// DID read is answered with what the lens format's schema found in it, in the same voice a
/// finding about the request's own body is stated in — the property that is missing, the
/// type the instance has against the types admitted, the string that was measured with its
/// character count and the published bound, the value against the set the format declares.
/// The rows below state each of those findings in full.
#[tokio::test]
async fn a_payload_that_is_not_a_lens_is_refused_and_imports_nothing() {
    /// The whole `Message` a row expects for a document the service could not read as JSON
    /// at all: the member named as invalid, with nothing about the document in it.
    const UNPARSABLE: &str = "[Validation] Invalid JSONString";

    /// The whole `Message` for a document the service read: the finding, listed the way
    /// every finding of this service is listed, behind the failure-class tag.
    fn finding(found: &str) -> String {
        format!("[Validation] [{found}]")
    }

    /// The finding for a property the document does not carry.
    fn missing(property: &str) -> String {
        finding(&format!(
            "object has missing required properties ([\"{property}\"])"
        ))
    }

    /// The finding for an instance of another type than the format admits there.
    fn wrong_type(found: &str, allowed: &str) -> String {
        finding(&format!(
            "instance type ({found}) does not match any allowed primitive type (allowed: \
             [\"{allowed}\"])"
        ))
    }

    /// The finding for a value outside the set the format declares, which lists that set in
    /// the order the format declares it.
    fn not_in_set(value: &str, permitted: &[&str]) -> String {
        let quoted: Vec<String> = permitted
            .iter()
            .map(|admitted| format!("\"{admitted}\""))
            .collect();
        finding(&format!(
            "instance value (\"{value}\") not found in enum (possible values: [{}])",
            quoted.join(",")
        ))
    }

    /// The two findings for a string outside the bounds the format publishes for it.
    fn too_short(value: &str, min: usize) -> String {
        finding(&format!(
            "string \"{value}\" is too short (length: {}, required minimum: {min})",
            value.chars().count()
        ))
    }

    fn too_long(value: &str, max: usize) -> String {
        finding(&format!(
            "string \"{value}\" is too long (length: {}, maximum allowed: {max})",
            value.chars().count()
        ))
    }

    /// The finding for an array the format requires an element of.
    fn empty_array() -> String {
        finding("array is too short: must have at least 1 elements but instance has 0 elements")
    }

    let valid = lens_document("Valid lens", None, &[("governance", 1, 2)]);
    // Rows: each a document breaking ONE rule of the lens JSON format specification,
    // plus the two payloads that are not lens documents at all.
    let mut broken_default = valid.clone();
    broken_default["pillars"][0]["questions"][0]["riskRules"] =
        json!([{ "condition": "default", "risk": "HIGH_RISK" }, { "condition": "governance_q0_c0", "risk": "NO_RISK" }]);
    let mut unmodelled_risk = valid.clone();
    unmodelled_risk["pillars"][0]["questions"][0]["riskRules"][0]["risk"] = json!("LOW_RISK");
    let mut duplicate_choice = valid.clone();
    duplicate_choice["pillars"][0]["questions"][0]["choices"][1]["id"] = json!("governance_q0_c0");
    let mut duplicate_question = lens_document("Duplicate questions", None, &[("governance", 2, 2)]);
    duplicate_question["pillars"][0]["questions"][1]["id"] = json!("governance_q0");
    let mut no_choices = valid.clone();
    no_choices["pillars"][0]["questions"][0]["choices"] = json!([]);
    let mut no_questions = valid.clone();
    no_questions["pillars"][0]["questions"] = json!([]);
    let mut wrong_schema = valid.clone();
    wrong_schema["schemaVersion"] = json!("2020-07-02");
    let mut empty_name = valid.clone();
    empty_name["name"] = json!("");
    let mut long_name = valid.clone();
    long_name["name"] = json!("n".repeat(129));
    let mut long_description = valid.clone();
    long_description["description"] = json!("d".repeat(1025));
    let mut no_pillars = valid.clone();
    no_pillars["pillars"] = json!([]);
    let mut name_not_a_string = valid.clone();
    name_not_a_string["name"] = json!(7);

    let cases: Vec<(&str, String, String)> = vec![
        // The two documents the service cannot read as JSON at all: no rule of the format
        // was reached, so nothing about the document is reported.
        (
            "not-json-at-all",
            "this is not a lens document".to_string(),
            UNPARSABLE.to_string(),
        ),
        (
            "truncated-json",
            "{\"schemaVersion\": ".to_string(),
            UNPARSABLE.to_string(),
        ),
        (
            "json-but-not-an-object",
            json!([valid.clone()]).to_string(),
            wrong_type("array", "object"),
        ),
        (
            "missing-schema-version",
            json!({ "name": "No version", "pillars": valid["pillars"].clone() }).to_string(),
            missing("schemaVersion"),
        ),
        (
            "wrong-schema-version",
            wrong_schema.to_string(),
            not_in_set("2020-07-02", &[state::LENS_SCHEMA_VERSION]),
        ),
        (
            "missing-name",
            json!({ "schemaVersion": "2021-11-01", "pillars": valid["pillars"].clone() })
                .to_string(),
            missing("name"),
        ),
        (
            "name-not-a-string",
            name_not_a_string.to_string(),
            wrong_type("integer", "string"),
        ),
        // `LensName` is @length(1, 128) and `LensDescription` @length(1, 1024)
        // (smithy_query: shape Lens), so one character past each is refused.
        (
            "name-below-the-minimum",
            empty_name.to_string(),
            too_short("", 1),
        ),
        (
            "name-above-the-maximum",
            long_name.to_string(),
            too_long(&"n".repeat(129), 128),
        ),
        (
            "description-above-the-maximum",
            long_description.to_string(),
            too_long(&"d".repeat(1025), 1024),
        ),
        ("no-pillars", no_pillars.to_string(), empty_array()),
        (
            "missing-pillars",
            json!({ "schemaVersion": "2021-11-01", "name": "No pillars" }).to_string(),
            missing("pillars"),
        ),
        (
            "pillar-without-questions",
            no_questions.to_string(),
            empty_array(),
        ),
        (
            "question-without-choices",
            no_choices.to_string(),
            empty_array(),
        ),
        (
            // A rule the format states that no schema can: the condition that applies when
            // no other matched belongs to the last rule and to no other.
            "default-risk-rule-not-last",
            broken_default.to_string(),
            finding(
                "array must carry the \"default\" condition on its last risk rule and on no \
                 other",
            ),
        ),
        (
            "unmodelled-risk",
            unmodelled_risk.to_string(),
            not_in_set("LOW_RISK", state::LENS_JSON_RISK_VALUES),
        ),
        (
            // The other rule of that kind: an id names one choice of a question and one
            // question of a lens, so two of them cannot share it.
            "duplicate-choice-id",
            duplicate_choice.to_string(),
            finding(
                "array must not carry a duplicate id (\"governance_q0_c0\") within one question",
            ),
        ),
        (
            "duplicate-question-id",
            duplicate_question.to_string(),
            finding("array must not carry a duplicate id (\"governance_q0\") within one lens"),
        ),
    ];

    for (case, document, expected_message) in cases {
        let client = Client::new();
        let answer = import(&client, &document, None, "tok-refused", None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        // The request carried a document and the document is not a lens, so the request
        // never became what it had to be: `Reason` is carried unclassified and there is no
        // field finding, whichever rule the document broke.
        assert_eq!(
            answer.body.get("Reason"),
            Some(&json!("")),
            "case {case:?}: Reason must be carried unclassified, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Fields"),
            None,
            "case {case:?}: no field rule ran, so no field finding is reported, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "Message"),
            expected_message,
            "case {case:?}: unexpected Message, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("LensArn"),
            None,
            "case {case:?}: a refused import mints no lens, got {answer:?}"
        );

        // Nothing was recorded for that token, so it is still free for a real import.
        let accepted = import(
            &client,
            &lens_document("Lens after a refusal", None, &[("governance", 1, 2)]).to_string(),
            None,
            "tok-refused",
            None,
        )
        .await;
        imported_arn(&accepted, case);
    }
}

/// Rule 3. An ARN in `LensAlias` updates THAT lens — the lens keeps its ARN and the
/// document the update carried becomes its content, while the import's own job reports
/// `IN_PROGRESS` for its span. A request with no ARN mints another lens instead. Rows
/// differ along the shape of the replacement document.
#[tokio::test]
async fn importing_with_a_lens_arn_updates_that_lens_and_without_one_mints_another() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "same-shape-new-content",
            lens_document("Updated lens", None, &[("governance", 1, 2)]),
        ),
        (
            "more-pillars",
            lens_document("Grown lens", Some("now with more pillars"), &[
                ("governance", 2, 2),
                ("security", 1, 3),
            ]),
        ),
        (
            "fewer-questions",
            lens_document("Shrunk lens", None, &[("governance", 1, 1)]),
        ),
    ];

    for (case, replacement) in cases {
        let client = Client::new();
        let first = lens_document("Original lens", None, &[("governance", 2, 3)]).to_string();
        let created = import(&client, &first, None, "tok-create", None).await;
        let arn = imported_arn(&created, case);
        wait_for_the_import_to_land();

        let replacement = replacement.to_string();
        let updated = import(
            &client,
            &replacement,
            Some(&arn),
            "tok-update",
            Some(json!({ "team": "platform" })),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: importing over a lens ARN must succeed, got {updated:?}"
        );
        assert_eq!(
            member(&updated, "LensArn"),
            arn.as_str(),
            "case {case:?}: an update keeps the lens's ARN, got {updated:?}"
        );
        assert_eq!(
            member(&updated, "Status"),
            "IN_PROGRESS",
            "case {case:?}: the replacement import is still in progress, got {updated:?}"
        );

        // The replacement is the lens's content from the moment it was accepted, while
        // the import that carried it is still reporting IN_PROGRESS.
        let during = export(&client, &arn, None).await;
        assert_eq!(
            member(&during, "LensJSON"),
            replacement.as_str(),
            "case {case:?}: an accepted update must replace the readable lens, got {during:?}"
        );

        wait_for_the_import_to_land();
        let after = export(&client, &arn, None).await;
        assert_eq!(
            member(&after, "LensJSON"),
            replacement.as_str(),
            "case {case:?}: the lens keeps exporting the new document, got {after:?}"
        );

        // No ARN: another lens, with its own ARN, leaving the first one alone.
        let another = import(&client, &first, None, "tok-another", None).await;
        let other_arn = imported_arn(&another, case);
        assert_ne!(
            other_arn, arn,
            "case {case:?}: an import without a LensAlias must mint a new lens, got {another:?}"
        );
        wait_for_the_import_to_land();
        let unchanged = export(&client, &arn, None).await;
        assert_eq!(
            member(&unchanged, "LensJSON"),
            replacement.as_str(),
            "case {case:?}: minting another lens must not touch this one, got {unchanged:?}"
        );
    }
}

/// Rule 3, continued: an ARN that names no imported lens is a 404, and an Amazon Web
/// Services official lens is refused — "Note that some operations (such as ExportLens and
/// CreateLensShare) are not permitted on Amazon Web Services official lenses"
/// (`LensAlias` doc), and an official lens ARN names no owner account. Rows differ along
/// how the lens is named.
#[tokio::test]
async fn an_official_lens_is_refused_and_an_unknown_custom_lens_is_not_found() {
    let client = Client::new();
    let document = lens_document("A real lens", None, &[("governance", 1, 2)]).to_string();
    let arn = imported_arn(
        &import(&client, &document, None, "tok-present", None).await,
        "present",
    );
    wait_for_the_import_to_land();

    // Official: a bare alias (only an official lens is named by one) and an ARN with an
    // empty account id, which is the form the `LensAlias` doc gives for an official lens.
    for (case, alias) in [
        ("framework-alias", "wellarchitected"),
        ("specialised-alias", "serverless"),
        (
            "official-arn",
            "arn:aws:wellarchitected:us-east-1::lens/serverless",
        ),
        (
            "official-framework-arn",
            "arn:aws:wellarchitected:us-east-1::lens/wellarchitected",
        ),
    ] {
        let exported = export(&client, alias, None).await;
        assert_eq!(
            exported.status, 400,
            "case {case:?}: an official lens cannot be exported, got {exported:?}"
        );
        assert_eq!(
            exported.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {exported:?}"
        );
        assert_eq!(
            member(&exported, "Reason"),
            "FIELD_VALIDATION_FAILED",
            "case {case:?}: expected Reason FIELD_VALIDATION_FAILED, got {exported:?}"
        );
    }

    // An official lens ARN cannot be imported over either; a bare alias is not an ARN, so
    // it imports a NEW lens instead ("If no ARN is specified, a new custom lens is
    // created").
    for (case, alias) in [
        (
            "official-arn",
            "arn:aws:wellarchitected:us-east-1::lens/serverless",
        ),
        (
            "official-framework-arn",
            "arn:aws:wellarchitected:us-east-1::lens/wellarchitected",
        ),
    ] {
        let answer = import(
            &client,
            &document,
            Some(alias),
            &format!("tok-official-{case}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an official lens cannot be imported over, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer
                .body
                .get("Fields")
                .and_then(Value::as_array)
                .and_then(|fields| fields.first())
                .and_then(|field| field.get("Name"))
                .and_then(Value::as_str),
            Some("LensAlias"),
            "case {case:?}: the refusal must name LensAlias, got {answer:?}"
        );
    }

    // A custom lens ARN that names no imported lens: the lookup answers 404 on both
    // sides, carrying the alias as sent. A lens belongs to the account and Region its ARN
    // carries, so an ARN that agrees on the id and disagrees on either names a different
    // lens and must not resolve to this one.
    let lens_id = arn
        .rsplit_once("lens/")
        .map(|(_, id)| id.to_string())
        .unwrap_or_default();
    for (case, alias) in [
        ("absent-custom-arn", ABSENT_LENS_ARN.to_string()),
        (
            "absent-custom-arn-other-region",
            "arn:aws:wellarchitected:eu-west-1:000000000000:lens/ffffffffffffffffffffffffffffffff"
                .to_string(),
        ),
        (
            "stored-id-other-region",
            format!("arn:aws:wellarchitected:eu-west-1:000000000000:lens/{lens_id}"),
        ),
        (
            "stored-id-other-account",
            format!("arn:aws:wellarchitected:us-west-2:123456789012:lens/{lens_id}"),
        ),
    ] {
        let alias = alias.as_str();
        let exported = export(&client, alias, None).await;
        assert_eq!(
            exported.status, 404,
            "case {case:?}: an unknown custom lens must be a 404, got {exported:?}"
        );
        assert_eq!(
            member(&exported, "ResourceId"),
            alias,
            "case {case:?}: the 404 must carry the alias as sent, got {exported:?}"
        );
        assert_eq!(
            member(&exported, "ResourceType"),
            "LENS",
            "case {case:?}: expected ResourceType LENS, got {exported:?}"
        );

        let answer = import(
            &client,
            &document,
            Some(alias),
            &format!("tok-absent-{case}"),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: importing over an unknown lens must be a 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    // The lens that DOES exist still exports, so the rows above fail on resolution and
    // not on a broken route.
    let present = export(&client, &arn, None).await;
    assert_eq!(
        present.status, 200,
        "case \"present\": the imported lens must still export, got {present:?}"
    );
}

/// Rule 4. The same `ClientRequestToken` with the same parameters reports the original
/// import; with different parameters it is a 409 naming the lens the token already
/// imported. Rows differ along WHICH parameter the second request changes.
#[tokio::test]
async fn an_import_is_idempotent_per_client_request_token() {
    let first = lens_document("Token lens", None, &[("governance", 1, 2)]).to_string();
    let other = lens_document("Other lens", None, &[("governance", 2, 2)]).to_string();

    // (case, document, tags) for the SECOND request under the same token.
    let cases: Vec<(&str, &str, Option<Value>, bool)> = vec![
        ("same-parameters", first.as_str(), None, true),
        ("different-document", other.as_str(), None, false),
        (
            "different-tags",
            first.as_str(),
            Some(json!({ "team": "platform" })),
            false,
        ),
    ];

    for (case, document, tags, expect_replay) in cases {
        let client = Client::new();
        let created = import(&client, &first, None, "tok-idempotent", None).await;
        let arn = imported_arn(&created, case);

        let again = import(&client, document, None, "tok-idempotent", tags).await;
        if expect_replay {
            assert_eq!(
                again.status, 200,
                "case {case:?}: a replay must succeed, got {again:?}"
            );
            assert_eq!(
                member(&again, "LensArn"),
                arn.as_str(),
                "case {case:?}: a replay must name the original lens, got {again:?}"
            );
            // The replay imported nothing new, so the lens still exports the original.
            wait_for_the_import_to_land();
            let exported = export(&client, &arn, None).await;
            assert_eq!(
                member(&exported, "LensJSON"),
                first.as_str(),
                "case {case:?}: a replay must not change the lens, got {exported:?}"
            );
        } else {
            assert_eq!(
                again.status, 409,
                "case {case:?}: the token with different parameters must be a 409, got {again:?}"
            );
            assert_eq!(
                again.error_code, "ConflictException",
                "case {case:?}: expected ConflictException, got {again:?}"
            );
            assert_eq!(
                member(&again, "ResourceType"),
                "LENS",
                "case {case:?}: expected ResourceType LENS, got {again:?}"
            );
            assert_eq!(
                member(&again, "Message"),
                "[Conflict] A request with ClientRequestToken tok-idempotent was already \
                 submitted with different parameters.",
                "case {case:?}: the 409 must name the token, got {again:?}"
            );
            // And the lens the token imported is untouched.
            wait_for_the_import_to_land();
            let exported = export(&client, &arn, None).await;
            assert_eq!(
                member(&exported, "LensJSON"),
                first.as_str(),
                "case {case:?}: a refused replay must not change the lens, got {exported:?}"
            );
        }
    }
}

/// Rule 4, continued: the maps a CALLER keys stay bounded. The idempotency ledger holds
/// at most `MAX_REMEMBERED_REQUEST_TOKENS` tokens, evicting the one remembered longest
/// ago — so a stream of fresh tokens cannot grow it — and a lens holds at most
/// `MAX_TAGS_PER_RESOURCE` tags, so repeated imports over one lens cannot grow its tag
/// map. A token the ledger has forgotten is not a replay: the next request carrying it
/// imports afresh. (`super::token_ledgers` pins the same ceiling over every other
/// caller-keyed ledger; this row is the import one, end to end over HTTP.)
#[tokio::test]
async fn the_token_ledger_and_a_lens_tag_map_stay_within_their_ceilings() {
    let client = Client::new();
    let document = lens_document("Bounded lens", None, &[("governance", 1, 2)]).to_string();
    let created = import(&client, &document, None, "tok-bounded", None).await;
    let arn = imported_arn(&created, "bounded");

    // Driven on the state directly: reaching the ledger's ceiling over HTTP would take
    // tens of thousands of requests. The eviction rule is the state's, so this is where
    // it is observed.
    let overflow = 25usize;
    {
        let store = client.store();
        let mut store = store.lock();
        let lens_id = arn
            .rsplit_once("lens/")
            .map(|(_, id)| id.to_string())
            .unwrap_or_default();
        for index in 0..state::MAX_REMEMBERED_REQUEST_TOKENS + overflow {
            store.import_lens_tokens.remember(
                format!("bulk-token-{index:08}"),
                state::ImportLensToken {
                    lens_id: lens_id.clone(),
                    fingerprint: format!("fingerprint-{index}"),
                },
            );
        }
        assert_eq!(
            store.import_lens_tokens.len(),
            state::MAX_REMEMBERED_REQUEST_TOKENS,
            "case \"ledger\": the ledger must hold exactly {} tokens after {} inserts, got {}",
            state::MAX_REMEMBERED_REQUEST_TOKENS,
            state::MAX_REMEMBERED_REQUEST_TOKENS + overflow,
            store.import_lens_tokens.len()
        );
        let newest = format!(
            "bulk-token-{:08}",
            state::MAX_REMEMBERED_REQUEST_TOKENS + overflow - 1
        );
        assert!(
            store.import_lens_tokens.contains_key(&newest),
            "case \"ledger\": the newest token must be remembered, got {:?}",
            store.import_lens_tokens.len()
        );
        assert!(
            !store.import_lens_tokens.contains_key("bulk-token-00000000"),
            "case \"ledger\": the oldest token must have been evicted"
        );
        // The real token was remembered before the flood, so it is one of the evicted.
        assert!(
            !store.import_lens_tokens.contains_key("tok-bounded"),
            "case \"ledger\": a token older than the ceiling must be forgotten"
        );
    }

    // Forgotten, so the same token with the same parameters is a fresh import rather than
    // a replay: it mints another lens.
    let afresh = import(&client, &document, None, "tok-bounded", None).await;
    let fresh_arn = imported_arn(&afresh, "forgotten-token");
    assert_ne!(
        fresh_arn, arn,
        "case \"forgotten-token\": a forgotten token must import afresh, got {afresh:?}"
    );

    // The tag ceiling. `TagMap` is @length(1, 50), so one request can carry at most 50
    // tags; repeated updates must not take the stored map past that either.
    let full: Value = (0..state::MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:02}"), Value::String(format!("v{index}"))))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let tagged = import(&client, &document, None, "tok-tagged", Some(full.clone())).await;
    let tagged_arn = imported_arn(&tagged, "tags-at-the-cap");
    wait_for_the_import_to_land();

    for (case, tags, expected_status) in [
        // Tags already on the lens raise nothing, so the update is accepted.
        ("tags-already-held", full.clone(), 200),
        // One more distinct key would take the lens past the cap.
        ("one-key-past-the-cap", json!({ "key-50": "v50" }), 400),
    ] {
        let answer = import(
            &client,
            &document,
            Some(&tagged_arn),
            &format!("tok-tags-{case}"),
            Some(tags),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {answer:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}: expected ValidationException, got {answer:?}"
            );
            assert_eq!(
                answer
                    .body
                    .get("Fields")
                    .and_then(Value::as_array)
                    .and_then(|fields| fields.first())
                    .and_then(|field| field.get("Name"))
                    .and_then(Value::as_str),
                Some("Tags"),
                "case {case:?}: the refusal must name Tags, got {answer:?}"
            );
        }
        let store = client.store();
        let store = store.lock();
        let held = store
            .lens_by_alias(&tagged_arn)
            .map(|record| record.tags.len())
            .unwrap_or_default();
        assert!(
            held <= state::MAX_TAGS_PER_RESOURCE,
            "case {case:?}: a lens must hold at most {} tags, got {held}",
            state::MAX_TAGS_PER_RESOURCE
        );
    }
}
