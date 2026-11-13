// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the words this service refuses an INPUT VALUE with.
//!
//! The rule these pin, over tables of operations, members and values rather than one
//! recorded request:
//!
//! 1. A value the request BODY carried that breaks a rule the layer reading the body
//!    measures — a string's `@length`, its `@pattern`, an enumerated type's value set — is
//!    refused 400 `ValidationException` whose WHOLE message is that layer's findings, listed
//!    inside one pair of brackets behind this service's `[Validation]` tag, with nothing
//!    classified: no `Reason` and no `Fields`. A finding states what was MEASURED (the
//!    string, its character count, the published bound, the modelled expression, the
//!    declared value set) and never the member path.
//! 2. A property the shape requires that the request does not carry is reported by the same
//!    layer as the property it is, with `Reason` carried unclassified — the request never
//!    became the shape, so no field rule ran and there is no field finding.
//! 3. A value bound OUTSIDE the body is parsed by the service into the type its member
//!    targets, so a value outside an enumerated type's set is a field validation naming
//!    that TYPE: `Reason` `FIELD_VALIDATION_FAILED` and one `Fields` entry whose sentence
//!    `Message` repeats.
//! 4. What that layer states no finding for keeps the framework's own clause: a `@length`
//!    broken by a LIST (there is no string to quote) and a value past the framework's echo
//!    ceiling (the quoted value would contradict the length a finding states).
//!
//! Every variable part of a message comes from the request or from the model, so the rows
//! below differ along the member, the bound, the constraint class, the operation and the
//! value (its length in CHARACTERS rather than bytes included) — the recorded request is
//! one row of each table.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, the boundary walk and the handlers all run as they do in production.
//! The HTTP client and the request builders are the shared `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{
    encode_path_segment, member, minimal_create, minimal_profile, minimal_template, Client,
};

/// A `WorkloadId`-shaped value (`^[0-9a-f]{32}$`) that names nothing: every row here is
/// refused before any lookup, so a row about a REFUSAL need not own a workload.
const ABSENT_HEX_ID: &str = "0123456789abcdef0123456789abcdef";

/// A custom lens ARN that is well formed and names no imported lens, for the same reason.
const ABSENT_LENS_ARN: &str =
    "arn:aws:wellarchitected:us-west-2:000000000000:lens/0123456789abcdef0123456789abcdef";

/// The expression `CreateReviewTemplate$TemplateName` and `$Description` declare
/// (`smithy_query(query="operation", operation="CreateReviewTemplate")`), as the model
/// writes it — which is how a finding quotes it.
const TEMPLATE_PATTERN: &str = r"^[A-Za-z0-9-_.,:/()@!&?#+'’\s]+$";

/// The expression `CreateWorkload$ArchitecturalDesign` declares
/// (`smithy_query(query="operation", operation="CreateWorkload")`).
const DESIGN_PATTERN: &str = r"^(|(https?|ftp):\/\/[^\s/$.?#].[^\s]*)$";

/// The expression every `ClientRequestToken` of this service declares — printable and
/// control ASCII only.
const TOKEN_PATTERN: &str = r"^[\x00-\x7F]*$";

/// One row: a request to send and the finding its whole message has to be.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The finding, without the brackets the layer lists findings inside.
    finding: String,
}

/// The sentences the layer that reads a body states, built here exactly as the service's
/// shared renderer states them so a row names the message it expects in full.
fn too_short(value: &str, min: usize) -> String {
    format!(
        "string \"{value}\" is too short (length: {}, required minimum: {min})",
        value.chars().count()
    )
}

fn too_long(value: &str, max: usize) -> String {
    format!(
        "string \"{value}\" is too long (length: {}, maximum allowed: {max})",
        value.chars().count()
    )
}

fn pattern_mismatch(pattern: &str, value: &str) -> String {
    format!("ECMA 262 regex \"{pattern}\" does not match input string \"{value}\"")
}

/// Assert rule 1 of the module docs over one row.
async fn assert_schema_finding(client: &Client, row: &Row) {
    let Row {
        case,
        method,
        path,
        body,
        finding,
    } = row;
    let answer = client.send(method, path, body.clone()).await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "Message"),
        format!("[Validation] [{finding}]"),
        "case {case:?}: the whole message must be the finding, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Reason"),
        None,
        "case {case:?}: a measured value is not classified, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Fields"),
        None,
        "case {case:?}: the layer reports no per-member finding, got {answer:?}"
    );
}

/// Rule 1 for `@length`: the string the layer measured, its length in CHARACTERS and the
/// bound the model publishes.
///
/// The bounds are the modelled ones (`smithy_query(query="operation",
/// operation="CreateWorkload,CreateMilestone,CreateReviewTemplate,CreateProfile")`):
/// `WorkloadName` 3..100, `Description` 3..250, `ReviewOwner` 3..255,
/// `MilestoneName` 3..100, `TemplateName` 3..100, `ProfileName` 3..100 and
/// `ProfileDescription` 3..100. Rows cover each published bound from the outside — one
/// character short of the minimum and one past the maximum — over members and operations
/// the issue did not cite, and a value whose characters are multi-byte, which is what tells
/// a character count from a byte count.
#[tokio::test]
async fn a_body_length_violation_states_the_measured_string() {
    let client = Client::new();
    let long_name = "n".repeat(101);
    let long_description = "d".repeat(251);
    let long_owner = "o".repeat(256);
    let long_template = "t".repeat(101);
    let long_profile = "p".repeat(101);
    // 101 characters of a two-byte character: 202 bytes, so the value is quoted whole and
    // the length the finding states is the CHARACTER count.
    let long_unicode_name: String = "é".repeat(101);

    let rows: Vec<Row> = vec![
        Row {
            case: "CreateWorkload$WorkloadName below the minimum",
            method: "POST",
            path: "/workloads".to_string(),
            body: Some(minimal_create("ab", "tok-name-short")),
            finding: too_short("ab", 3),
        },
        Row {
            case: "CreateWorkload$WorkloadName above the maximum",
            method: "POST",
            path: "/workloads".to_string(),
            body: Some(minimal_create(&long_name, "tok-name-long")),
            finding: too_long(&long_name, 100),
        },
        Row {
            case: "CreateWorkload$WorkloadName multi-byte characters",
            method: "POST",
            path: "/workloads".to_string(),
            body: Some(minimal_create("éé", "tok-name-unicode-short")),
            finding: too_short("éé", 3),
        },
        Row {
            case: "CreateWorkload$WorkloadName multi-byte characters above the maximum",
            method: "POST",
            path: "/workloads".to_string(),
            body: Some(minimal_create(&long_unicode_name, "tok-name-unicode-long")),
            finding: too_long(&long_unicode_name, 100),
        },
        Row {
            case: "CreateWorkload$Description below the minimum",
            method: "POST",
            path: "/workloads".to_string(),
            body: Some({
                let mut request = minimal_create("desc-short", "tok-desc-short");
                request["Description"] = json!("ab");
                request
            }),
            finding: too_short("ab", 3),
        },
        Row {
            case: "CreateWorkload$Description above the maximum",
            method: "POST",
            path: "/workloads".to_string(),
            body: Some({
                let mut request = minimal_create("desc-long", "tok-desc-long");
                request["Description"] = json!(long_description);
                request
            }),
            finding: too_long(&long_description, 250),
        },
        Row {
            case: "CreateWorkload$ReviewOwner above the maximum",
            method: "POST",
            path: "/workloads".to_string(),
            body: Some({
                let mut request = minimal_create("owner-long", "tok-owner-long");
                request["ReviewOwner"] = json!(long_owner);
                request
            }),
            finding: too_long(&long_owner, 255),
        },
        Row {
            case: "CreateMilestone$MilestoneName below the minimum",
            method: "POST",
            path: format!("/workloads/{ABSENT_HEX_ID}/milestones"),
            body: Some(json!({ "MilestoneName": "ab", "ClientRequestToken": "tok-ms-short" })),
            finding: too_short("ab", 3),
        },
        Row {
            case: "CreateMilestone$MilestoneName above the maximum",
            method: "POST",
            path: format!("/workloads/{ABSENT_HEX_ID}/milestones"),
            body: Some(
                json!({ "MilestoneName": long_name, "ClientRequestToken": "tok-ms-long" }),
            ),
            finding: too_long(&long_name, 100),
        },
        Row {
            case: "CreateReviewTemplate$TemplateName above the maximum",
            method: "POST",
            path: "/reviewTemplates".to_string(),
            body: Some(minimal_template(&long_template, "tok-tpl-long")),
            finding: too_long(&long_template, 100),
        },
        Row {
            case: "CreateProfile$ProfileName below the minimum",
            method: "POST",
            path: "/profiles".to_string(),
            body: Some({
                let mut request = minimal_profile("profile-short");
                request["ProfileName"] = json!("ab");
                request
            }),
            finding: too_short("ab", 3),
        },
        Row {
            case: "CreateProfile$ProfileDescription above the maximum",
            method: "POST",
            path: "/profiles".to_string(),
            body: Some({
                let mut request = minimal_profile("profile-desc-long");
                request["ProfileDescription"] = json!(long_profile);
                request
            }),
            finding: too_long(&long_profile, 100),
        },
    ];

    for row in &rows {
        assert_schema_finding(&client, row).await;
    }
}

/// ON each published bound the value is accepted, so the refusals above are about the
/// length and not about the request: a value at the minimum and one at the maximum of
/// `WorkloadName` and `Description` are both served, multi-byte characters included.
#[tokio::test]
async fn a_value_on_a_published_length_bound_is_accepted() {
    let client = Client::new();
    for (case, name, description) in [
        ("at-the-minimum", "abc".to_string(), "abc".to_string()),
        ("at-the-maximum", "n".repeat(100), "d".repeat(250)),
        ("multi-byte-at-the-maximum", "é".repeat(100), "é".repeat(250)),
    ] {
        let mut request = minimal_create(&name, &format!("tok-bound-{case}"));
        request["Description"] = json!(description);
        let answer = client.create(request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a value on the published bound must be served, got {answer:?}"
        );
    }
}

/// Rule 1 for `@pattern`: the expression the MODEL declares, as it writes it, and the
/// string the caller submitted. Rows span three members whose expressions differ, two
/// operations, and values that break the expression in different ways.
#[tokio::test]
async fn a_body_pattern_violation_states_the_modelled_expression() {
    let client = Client::new();
    let rows: Vec<Row> = vec![
        Row {
            case: "CreateReviewTemplate$TemplateName outside its expression",
            method: "POST",
            path: "/reviewTemplates".to_string(),
            body: Some(minimal_template("sim2real*probe*tpl", "tok-tpl-pattern")),
            finding: pattern_mismatch(TEMPLATE_PATTERN, "sim2real*probe*tpl"),
        },
        Row {
            case: "CreateReviewTemplate$Description outside its expression",
            method: "POST",
            path: "/reviewTemplates".to_string(),
            body: Some({
                let mut request = minimal_template("pattern-description", "tok-tpl-desc");
                request["Description"] = json!("a description with a | in it");
                request
            }),
            finding: pattern_mismatch(TEMPLATE_PATTERN, "a description with a | in it"),
        },
        Row {
            case: "CreateWorkload$ArchitecturalDesign outside its expression",
            method: "POST",
            path: "/workloads".to_string(),
            body: Some({
                let mut request = minimal_create("design-pattern", "tok-design");
                request["ArchitecturalDesign"] = json!("not a url");
                request
            }),
            finding: pattern_mismatch(DESIGN_PATTERN, "not a url"),
        },
        Row {
            case: "CreateWorkload$ClientRequestToken outside its expression",
            method: "POST",
            path: "/workloads".to_string(),
            body: Some(minimal_create("token-pattern", "tok-é")),
            finding: pattern_mismatch(TOKEN_PATTERN, "tok-é"),
        },
    ];

    for row in &rows {
        assert_schema_finding(&client, row).await;
    }

    // A value the expression DOES admit is not refused by that rule, so the findings above
    // are about the value and not about the member.
    for (case, member_name, value) in [
        ("TemplateName", "TemplateName", "probe-template (1).".to_string()),
        (
            "Description",
            "Description",
            "a description: punctuation, parentheses (and more)".to_string(),
        ),
    ] {
        let mut request = minimal_template(
            &format!("pattern-accepted-{case}"),
            &format!("tok-pattern-accepted-{case}"),
        );
        request[member_name] = json!(value);
        let answer = client.send("POST", "/reviewTemplates", Some(request)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a value the expression admits must be served, got {answer:?}"
        );
    }
}

/// Rule 2: the property the request does not carry, named as the schema declares it, with
/// `Reason` carried unclassified and no field finding.
///
/// Rows span the operation and the property, and whether the MODEL requires it or the
/// operation's own documentation does (`CreateWorkload$ReviewOwner`: "You also must specify
/// ReviewOwner, even though the parameter is listed as not being required").
#[tokio::test]
async fn an_absent_required_property_is_named_as_the_schema_declares_it() {
    let client = Client::new();
    let rows: Vec<(&str, &str, String, Value, &str)> = vec![
        (
            "CreateWorkload$WorkloadName",
            "POST",
            "/workloads".to_string(),
            minimal_create("absent", "tok-absent-name"),
            "WorkloadName",
        ),
        (
            "CreateWorkload$Environment",
            "POST",
            "/workloads".to_string(),
            minimal_create("absent-env", "tok-absent-env"),
            "Environment",
        ),
        (
            "CreateWorkload$Lenses",
            "POST",
            "/workloads".to_string(),
            minimal_create("absent-lenses", "tok-absent-lenses"),
            "Lenses",
        ),
        (
            "CreateWorkload$ClientRequestToken",
            "POST",
            "/workloads".to_string(),
            minimal_create("absent-token", "tok-absent-token"),
            "ClientRequestToken",
        ),
        (
            // Required by the operation's documentation rather than by the model, and
            // reported exactly as a modelled one is.
            "CreateWorkload$ReviewOwner",
            "POST",
            "/workloads".to_string(),
            minimal_create("absent-owner", "tok-absent-owner"),
            "ReviewOwner",
        ),
        (
            "CreateMilestone$MilestoneName",
            "POST",
            format!("/workloads/{ABSENT_HEX_ID}/milestones"),
            json!({ "MilestoneName": "a milestone", "ClientRequestToken": "tok-absent-ms" }),
            "MilestoneName",
        ),
        (
            "CreateReviewTemplate$TemplateName",
            "POST",
            "/reviewTemplates".to_string(),
            minimal_template("absent-template", "tok-absent-tpl"),
            "TemplateName",
        ),
        (
            "CreateReviewTemplate$Lenses",
            "POST",
            "/reviewTemplates".to_string(),
            minimal_template("absent-template-lenses", "tok-absent-tpl-lenses"),
            "Lenses",
        ),
        (
            "CreateProfile$ProfileQuestions",
            "POST",
            "/profiles".to_string(),
            minimal_profile("absent-questions"),
            "ProfileQuestions",
        ),
    ];

    for (case, method, path, request, absent) in rows {
        let mut request = request;
        request
            .as_object_mut()
            .expect("the request is a JSON object")
            .remove(absent);
        let answer = client.send(method, &path, Some(request)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "Message"),
            format!("[Validation] [object has missing required properties ([\"{absent}\"])]"),
            "case {case:?}: the message must name the absent property, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason"),
            Some(&json!("")),
            "case {case:?}: the request was never read, so Reason is unclassified, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Fields"),
            None,
            "case {case:?}: no field rule ran, so no field finding is reported, got {answer:?}"
        );
    }
}

/// Rule 3: a value bound outside the body that is outside its enumerated type's set is a
/// field validation naming that TYPE.
///
/// The types are the ones this service binds outside its body
/// (`smithy_query(query="operation",
/// operation="DeleteLens,ListLenses,ListProfiles,ListShareInvitations,ListWorkloadShares,ListAnswers,GetConsolidatedReport")`):
/// `LensStatus` -> `LensStatusType`, `LensType` -> `LensType`, `ProfileOwnerType` ->
/// `ProfileOwnerType`, `ShareResourceType` -> `ShareResourceType`, `Status` ->
/// `ShareStatus`, `QuestionPriority` -> `QuestionPriority`, `Format` -> `ReportFormat`.
/// Rows span the operation, the member, the type and the value sent — a member of another
/// enum, a lowercased member of this one and a value no enum publishes — and include the
/// last operation, which is still scaffolded, so the envelope cannot come from a handler.
#[tokio::test]
async fn an_out_of_set_query_enum_names_the_type_it_was_parsed_into() {
    let client = Client::new();
    let rows: Vec<(&str, &str, String, &str)> = vec![
        (
            "DeleteLens$LensStatus",
            "DELETE",
            format!(
                "/lenses/{}?LensStatus=SIM2REAL&ClientRequestToken=tok-delete-lens",
                encode_path_segment(ABSENT_LENS_ARN)
            ),
            "LensStatusType",
        ),
        (
            "ListLenses$LensStatus lowercased",
            "GET",
            "/lenses?LensStatus=draft".to_string(),
            "LensStatusType",
        ),
        (
            "ListLenses$LensType",
            "GET",
            "/lenses?LensType=CUSTOM".to_string(),
            "LensType",
        ),
        (
            "ListProfiles$ProfileOwnerType",
            "GET",
            "/profileSummaries?ProfileOwnerType=NEITHER".to_string(),
            "ProfileOwnerType",
        ),
        (
            "ListShareInvitations$ShareResourceType",
            "GET",
            "/shareInvitations?ShareResourceType=MILESTONE".to_string(),
            "ShareResourceType",
        ),
        (
            "ListWorkloadShares$Status",
            "GET",
            format!("/workloads/{ABSENT_HEX_ID}/shares?Status=WITHDRAWN"),
            "ShareStatus",
        ),
        (
            "ListAnswers$QuestionPriority",
            "GET",
            format!(
                "/workloads/{ABSENT_HEX_ID}/lensReviews/wellarchitected/answers?\
                 QuestionPriority=URGENT"
            ),
            "QuestionPriority",
        ),
        (
            "GetConsolidatedReport$Format",
            "GET",
            "/consolidatedReport?Format=CSV".to_string(),
            "ReportFormat",
        ),
    ];

    for (case, method, path, declared) in rows {
        let answer = client.send(method, &path, None).await;
        let sentence = format!("The {declared} that was passed in is invalid.");
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "Message"),
            format!("[Validation] {sentence}"),
            "case {case:?}: the message must name the type, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "Reason"),
            "FIELD_VALIDATION_FAILED",
            "case {case:?}: a parsed parameter value is a field validation, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Fields").cloned().unwrap_or(Value::Null),
            json!([{ "Name": declared, "Message": sentence }]),
            "case {case:?}: Fields must name the type, got {answer:?}"
        );
    }

    // Every value the type publishes is accepted on the same path, so the refusals above
    // are about the value: `LensType`'s whole value set (the model's own, through the
    // service's generated table) pages the lens catalog.
    for declared in service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::ops::types::LENS_TYPE_VALUES
    {
        let answer = client
            .send("GET", &format!("/lenses?LensType={declared}"), None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {declared:?}: a published member of LensType must be served, got {answer:?}"
        );
    }
}

/// Rule 4: a `@length` broken by a LIST keeps the framework's own clause.
///
/// `DisassociateLenses$LensAliases` is `@length(min: 1)` on a list, and a list has no
/// string rendering to measure: the layer that words a string's length states nothing about
/// it, so the refusal is not reported as a string finding.
#[tokio::test]
async fn a_collection_length_bound_is_not_reported_as_a_string() {
    let client = Client::new();
    let answer = client
        .send(
            "PATCH",
            &format!("/workloads/{ABSENT_HEX_ID}/disassociateLenses"),
            Some(json!({ "LensAliases": [] })),
        )
        .await;
    assert_eq!(
        answer.status, 400,
        "an empty LensAliases must be refused, got {answer:?}"
    );
    let message = member(&answer, "Message");
    assert!(
        message.contains("failed to satisfy constraint"),
        "a collection keeps the framework's clause, got {answer:?}"
    );
    assert!(
        !message.contains("is too short (length"),
        "a collection has no string to measure, got {answer:?}"
    );
}

/// Rule 4 again: a value the framework's echo ceiling elides keeps the framework's clause.
///
/// `CreateWorkload$Notes` is `@length(max: 2084)`, past the ceiling the framework quotes a
/// refused value up to, so a finding built from the quoted text would state a length its own
/// text contradicts. No finding is stated for it and the whole message stays the
/// framework's, rather than being half in one voice and half in the other.
#[tokio::test]
async fn a_value_past_the_echo_ceiling_is_not_reported_as_a_finding() {
    let client = Client::new();
    let mut request = minimal_create("notes-too-long", "tok-notes-long");
    request["Notes"] = json!("n".repeat(2085));
    let answer = client.create(request).await;
    assert_eq!(
        answer.status, 400,
        "Notes past its maximum must be refused, got {answer:?}"
    );
    let message = member(&answer, "Message");
    assert!(
        message.contains("failed to satisfy constraint"),
        "an elided value keeps the framework's clause, got {answer:?}"
    );
    assert!(
        !message.contains("maximum allowed"),
        "no finding may state a length the message's own text contradicts, got {answer:?}"
    );
}
