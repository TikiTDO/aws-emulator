// SPDX-License-Identifier: CC0-1.0

//! A workspace's alert manager definition, end to end over HTTP: create, describe,
//! update, delete, and the refusals the model declares for them — each asserted against
//! what the service PERSISTED rather than against what one request carried.
//!
//! The rule under test: a workspace holds at most ONE alert manager definition, whose
//! document is stored byte for byte and reported from that record; it is `CREATING`
//! until the clock owes the move to `ACTIVE`, `UPDATING` while an update is pending and
//! `DELETING` until the removal is due; a blob that is not an alert manager definition is
//! refused at the member, a second create conflicts, and a `clientToken` replay answers
//! with the call it already made.
//!
//! Every row drives real requests through `MultiServiceRouter::dispatch` over this
//! service on one shared engine, so routing, the `restJson1` wire shapes (a blob rides as
//! base64), the modelled statuses and the modelled error envelopes are all exercised. The
//! rows differ along what the rule generalises over: the document's YAML form, character
//! set, size and nesting, the form of the idempotency token, and the state each call
//! finds the workspace and its definition in.

use service_com_amazonaws_amp_amazonprometheusservice::ops::types::{
    ALERT_MANAGER_DEFINITION_STATUS_CODE_VALUES, VALIDATION_EXCEPTION_REASON_VALUES,
};
use service_com_amazonaws_amp_amazonprometheusservice::state::{
    ALERT_MANAGER_DEFINITION_EXISTS_MESSAGE, ALERT_MANAGER_DEFINITION_MAX_BYTES,
    ALERT_MANAGER_DEFINITION_RESOURCE_TYPE, DEFINITION_STATUS_ACTIVE, DEFINITION_STATUS_CREATING,
    DEFINITION_STATUS_DELETING, DEFINITION_STATUS_UPDATING, WORKSPACE_RESOURCE_TYPE,
};

use crate::workspace_lifecycle::{text, wait_for_due_transition, Amp, Reply};

/// How deeply a definition row nests: past what an Alertmanager configuration needs and
/// within `yaml-rust2`'s own recursion limit, so a document the real service accepts is
/// not refused for its shape.
const DEEP_NESTING: usize = 200;
/// Past that parser's limit, which is what reports such a document unreadable — the
/// emulator adds no nesting bound of its own.
const NESTING_PAST_PARSER_LIMIT: usize = 400;

/// The documents here follow the form the `AlertManagerDefinitionData` reference gives —
/// a YAML mapping whose `alertmanager_config` entry holds the Alertmanager configuration,
/// with an optional `template_files` beside it:
/// <https://docs.aws.amazon.com/prometheus/latest/APIReference/yaml-AlertManagerDefinitionData.html>
pub(crate) fn block_scalar() -> Vec<u8> {
    b"alertmanager_config: |\n  route:\n    receiver: default\n  receivers:\n    - name: default\n"
        .to_vec()
}

pub(crate) fn with_template_files() -> Vec<u8> {
    b"alertmanager_config: |\n  route:\n    receiver: sns\n\
      template_files:\n  default_template: |\n    {{ define \"alert\" }}fired{{ end }}\n"
        .to_vec()
}

pub(crate) fn flow_style() -> Vec<u8> {
    b"{alertmanager_config: \"route:\\n  receiver: default\\n\"}".to_vec()
}

pub(crate) fn non_ascii() -> Vec<u8> {
    "alertmanager_config: |\n  route:\n    receiver: 通知-équipe\n"
        .as_bytes()
        .to_vec()
}

/// A valid definition padded with one YAML comment line to EXACTLY `bytes` bytes, so a
/// case can sit on the emulator's size bound or one byte past it.
fn padded(bytes: usize) -> Vec<u8> {
    let mut document = b"alertmanager_config: |\n  route:\n    receiver: default\n#".to_vec();
    document.resize(bytes - 1, b'x');
    document.push(b'\n');
    document
}

/// `data` is a blob, so it rides the wire base64-encoded the way an SDK encodes it —
/// written with `aws_smithy_types::base64`, the codec the wire serializer itself uses and
/// the one this crate's rules-file suites already encode blobs with.
fn body(data: &[u8], token: Option<&str>) -> String {
    let mut body = serde_json::Map::new();
    body.insert(
        "data".to_string(),
        aws_smithy_types::base64::encode(data).into(),
    );
    if let Some(token) = token {
        body.insert("clientToken".to_string(), token.into());
    }
    serde_json::Value::Object(body).to_string()
}

fn uri(ws: &str) -> String {
    format!("/workspaces/{ws}/alertmanager/definition")
}

pub(crate) async fn create(
    amp: &Amp,
    region: &str,
    ws: &str,
    data: &[u8],
    token: Option<&str>,
) -> Reply {
    amp.call("POST", &uri(ws), region, &body(data, token)).await
}

pub(crate) async fn put(
    amp: &Amp,
    region: &str,
    ws: &str,
    data: &[u8],
    token: Option<&str>,
) -> Reply {
    amp.call("PUT", &uri(ws), region, &body(data, token)).await
}

pub(crate) async fn delete(amp: &Amp, region: &str, ws: &str, token: Option<&str>) -> Reply {
    let path = match token {
        Some(token) => format!("{}?clientToken={token}", uri(ws)),
        None => uri(ws),
    };
    amp.call("DELETE", &path, region, "").await
}

pub(crate) async fn describe(amp: &Amp, region: &str, ws: &str) -> Reply {
    amp.call("GET", &uri(ws), region, "").await
}

/// The `statusCode` a create or an update reports.
fn reported_status(reply: &Reply) -> String {
    text(&reply.body["status"]["statusCode"])
}

/// The `statusCode` a describe reports for the stored definition.
fn stored_status(reply: &Reply) -> String {
    text(&reply.body["alertManagerDefinition"]["status"]["statusCode"])
}

/// The document a describe reports, decoded from the wire's base64.
fn stored_data(case: &str, reply: &Reply) -> Vec<u8> {
    let encoded = text(&reply.body["alertManagerDefinition"]["data"]);
    aws_smithy_types::base64::decode(&encoded).unwrap_or_else(|error| {
        panic!("case {case:?}: data must be base64 on the wire ({error}): {reply:?}")
    })
}

/// `createdAt` and `modifiedAt`, as the epoch-seconds timestamps the wire carries.
fn instants(case: &str, reply: &Reply) -> (f64, f64) {
    let definition = &reply.body["alertManagerDefinition"];
    match (
        definition["createdAt"].as_f64(),
        definition["modifiedAt"].as_f64(),
    ) {
        (Some(created), Some(modified)) => (created, modified),
        _ => panic!("case {case:?}: createdAt and modifiedAt must be timestamps, got {reply:?}"),
    }
}

/// The document a workspace currently stores, read back through the describe.
async fn read_back(case: &str, amp: &Amp, region: &str, ws: &str) -> Vec<u8> {
    stored_data(case, &describe(amp, region, ws).await)
}

/// The modelled 202 every create, update and delete answers with.
fn assert_accepted(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 202,
        "case {case:?}: expected the modelled 202, got {reply:?}"
    );
}

/// A modelled error envelope: the status, the `__type` code and the resource it names.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str, id: &str, kind: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {reply:?}"
    );
    assert!(
        reply.error_code().ends_with(code),
        "case {case:?}: expected error code {code}, got {:?}",
        reply.error_code()
    );
    assert_eq!(
        (
            text(&reply.body["resourceId"]),
            text(&reply.body["resourceType"])
        ),
        (id.to_string(), kind.to_string()),
        "case {case:?}: the error must name the resource it refused, got {reply:?}"
    );
}

/// A `ConflictException` (409) naming the definition's resource type.
fn assert_conflict(case: &str, reply: &Reply, ws: &str) {
    assert_refused(
        case,
        reply,
        409,
        "ConflictException",
        ws,
        ALERT_MANAGER_DEFINITION_RESOURCE_TYPE,
    );
}

/// Every status this crate reports for a definition is a value the modelled
/// `AlertManagerDefinitionStatusCode` enum declares — the catalog comes from the model.
#[test]
fn reported_statuses_are_modelled_status_codes() {
    for status in [
        DEFINITION_STATUS_CREATING,
        DEFINITION_STATUS_ACTIVE,
        DEFINITION_STATUS_UPDATING,
        DEFINITION_STATUS_DELETING,
    ] {
        assert!(
            ALERT_MANAGER_DEFINITION_STATUS_CODE_VALUES.contains(&status),
            "case {status:?}: not a modelled AlertManagerDefinitionStatusCode \
             ({ALERT_MANAGER_DEFINITION_STATUS_CODE_VALUES:?})"
        );
    }
}

/// A create persists the document it was given and a describe reads that same record
/// back, `CREATING` until the clock owes the move to `ACTIVE`. One row per thing the rule
/// generalises over: the document's YAML form, its optional `template_files` entry, its
/// character set, its size (up to the emulator's bound), its nesting, its use of an
/// anchor and alias, and the region the request is signed for.
#[tokio::test]
async fn create_persists_the_document_and_describe_reads_it_back() {
    // The reader never recurses over the document, so neither its nesting nor the number
    // of collections it opens is judged: both of these are accepted.
    let deep = format!(
        "alertmanager_config: {}{}",
        "[".repeat(DEEP_NESTING),
        "]".repeat(DEEP_NESTING)
    );
    let wide = format!(
        "alertmanager_config: r\nflow: [{}]\n",
        (0..DEEP_NESTING)
            .map(|index| format!("{{receiver: r{index}}}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let cases: Vec<(&str, &str, Vec<u8>)> = vec![
        ("block scalar config", "us-west-2", block_scalar()),
        (
            "config and template_files",
            "eu-west-1",
            with_template_files(),
        ),
        ("flow-style mapping", "ap-southeast-2", flow_style()),
        (
            "comments and blank lines",
            "us-east-1",
            b"# routing\n\nalertmanager_config: |\n  route:\n    receiver: ops\n# end\n".to_vec(),
        ),
        ("non-ASCII receiver name", "eu-central-1", non_ascii()),
        (
            "exactly at the size bound",
            "us-west-2",
            padded(ALERT_MANAGER_DEFINITION_MAX_BYTES),
        ),
        ("deeply nested flow collections", "us-east-1", deep.into_bytes()),
        ("many sibling flow collections", "eu-west-1", wide.into_bytes()),
        (
            // An alias refers to an anchored node; the reader never expands one, and a
            // document that uses one is valid YAML the real service takes.
            "an anchor and an alias",
            "us-west-2",
            b"anchors:\n  route: &route\n    receiver: default\nalertmanager_config:\n  route: *route\n"
                .to_vec(),
        ),
    ];

    let amp = Amp::new();
    for (case, region, data) in &cases {
        let ws = amp.active_workspace(region, "amd-round-trip").await;

        let created = create(&amp, region, &ws, data, None).await;
        assert_accepted(case, &created);
        assert_eq!(
            reported_status(&created),
            DEFINITION_STATUS_CREATING,
            "case {case:?}: a new definition starts CREATING, got {created:?}"
        );

        let described = describe(&amp, region, &ws).await;
        assert_eq!(
            (described.status, stored_status(&described)),
            (200, DEFINITION_STATUS_CREATING.to_string()),
            "case {case:?}: describe answers the modelled 200 and a caller reading \
             straight after the create still sees the transient status, got {described:?}"
        );
        assert_eq!(
            &stored_data(case, &described),
            data,
            "case {case:?}: describe must report the stored document byte for byte"
        );
        let (created_at, modified_at) = instants(case, &described);
        assert!(
            created_at > 0.0 && created_at == modified_at,
            "case {case:?}: a definition nothing has updated was modified when it was \
             created, got ({created_at}, {modified_at})"
        );

        // A read never moves the record: a second describe reports the same instants.
        let again = describe(&amp, region, &ws).await;
        assert_eq!(
            instants(case, &again),
            (created_at, modified_at),
            "case {case:?}: the instants must be stable across reads, got {again:?}"
        );

        // Once the clock owes the transition the definition is ACTIVE, and nothing else
        // about the record moved.
        wait_for_due_transition();
        let settled = describe(&amp, region, &ws).await;
        assert_eq!(
            (stored_status(&settled), instants(case, &settled)),
            (
                DEFINITION_STATUS_ACTIVE.to_string(),
                (created_at, modified_at)
            ),
            "case {case:?}: the due transition makes the definition ACTIVE and moves \
             nothing else, got {settled:?}"
        );
        assert_eq!(
            &stored_data(case, &settled),
            data,
            "case {case:?}: the transition must not move the document"
        );
    }
}

/// A blob that is not an alert manager definition document is refused with the modelled
/// `ValidationException` and nothing is stored — by the create AND by the update, which
/// share one rule. One row per way a blob fails it, including the emulator's one bound on
/// an untrusted document, its size: ON the bound in the table above, one byte past it
/// here.
#[tokio::test]
async fn blobs_that_are_not_an_alert_manager_definition_are_refused() {
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("empty blob", Vec::new(), "FIELD_VALIDATION_FAILED"),
        ("not valid UTF-8", vec![0xff, 0xfe, 0xfd], "CANNOT_PARSE"),
        (
            "unterminated flow sequence",
            b"alertmanager_config: [unclosed".to_vec(),
            "CANNOT_PARSE",
        ),
        (
            "two YAML documents",
            b"alertmanager_config: a\n---\nalertmanager_config: b\n".to_vec(),
            "CANNOT_PARSE",
        ),
        (
            "a plain scalar, not a mapping",
            b"just some text".to_vec(),
            "FIELD_VALIDATION_FAILED",
        ),
        (
            "a sequence at the top level",
            b"- alertmanager_config: a\n".to_vec(),
            "FIELD_VALIDATION_FAILED",
        ),
        (
            "alertmanager_config nested instead of top-level",
            b"route:\n  alertmanager_config: a\n".to_vec(),
            "FIELD_VALIDATION_FAILED",
        ),
        (
            "alertmanager_config as a VALUE, not a key",
            b"key: alertmanager_config\n".to_vec(),
            "FIELD_VALIDATION_FAILED",
        ),
        (
            // `yaml-rust2` enforces its own recursion limit while parsing, so a document
            // nesting past it is reported unreadable rather than left to the stack.
            "nested past the parser's recursion limit",
            format!(
                "alertmanager_config: {}{}",
                "[".repeat(NESTING_PAST_PARSER_LIMIT),
                "]".repeat(NESTING_PAST_PARSER_LIMIT)
            )
            .into_bytes(),
            "CANNOT_PARSE",
        ),
        (
            "one byte past the size bound",
            padded(ALERT_MANAGER_DEFINITION_MAX_BYTES + 1),
            "FIELD_VALIDATION_FAILED",
        ),
    ];

    let amp = Amp::new();
    let region = "us-east-1";
    // A refusal stores nothing, so every create row runs against one workspace that never
    // acquires a definition; every update row runs against a workspace whose stored
    // definition must survive each refusal untouched.
    let empty = amp.active_workspace(region, "amd-invalid-create").await;
    let holder = amp.active_workspace(region, "amd-invalid-put").await;
    let stored = block_scalar();
    create(&amp, region, &holder, &stored, None).await;
    wait_for_due_transition();

    for (case, data, reason) in &cases {
        assert!(
            VALIDATION_EXCEPTION_REASON_VALUES.contains(reason),
            "case {case:?}: {reason:?} is not a modelled ValidationExceptionReason \
             ({VALIDATION_EXCEPTION_REASON_VALUES:?})"
        );

        for (operation, reply) in [
            ("Create", create(&amp, region, &empty, data, None).await),
            ("Put", put(&amp, region, &holder, data, None).await),
        ] {
            assert_eq!(
                reply.status, 400,
                "case {case:?} on {operation}: expected the modelled 400, got {reply:?}"
            );
            assert!(
                reply.error_code().ends_with("ValidationException"),
                "case {case:?} on {operation}: expected ValidationException, got {:?}",
                reply.error_code()
            );
            assert_eq!(
                (
                    text(&reply.body["reason"]),
                    text(&reply.body["fieldList"][0]["name"])
                ),
                (reason.to_string(), "data".to_string()),
                "case {case:?} on {operation}: the refusal must carry the modelled reason \
                 and name the member, got {reply:?}"
            );
        }

        let absent = describe(&amp, region, &empty).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: a refused create must store nothing, got {absent:?}"
        );
        let described = describe(&amp, region, &holder).await;
        assert_eq!(
            (stored_data(case, &described), stored_status(&described)),
            (stored.clone(), DEFINITION_STATUS_ACTIVE.to_string()),
            "case {case:?}: a refused update must leave the stored record alone"
        );
    }
}

/// A workspace holds at most ONE definition, so a second create conflicts; and
/// `clientToken` carries `@idempotencyToken`, so replaying the create that minted the
/// definition — the same token AND the same document — answers with THAT definition,
/// before or after the clock settles it. One row per form the token takes
/// (`IdempotencyToken`: length 1..64, pattern `^[!-~]+$`).
#[tokio::test]
async fn a_second_create_conflicts_and_a_replayed_token_answers_with_its_definition() {
    let cases = [
        ("one character", "t".to_string()),
        ("printable punctuation", "~!#$%&*+-.token".to_string()),
        ("at the 64-character bound", "z".repeat(64)),
    ];

    let amp = Amp::new();
    let region = "eu-west-1";
    let first = block_scalar();
    let second = non_ascii();

    for (case, token) in &cases {
        let ws = amp.active_workspace(region, "amd-idempotent").await;
        assert_accepted(case, &create(&amp, region, &ws, &first, Some(token)).await);

        let replayed = create(&amp, region, &ws, &first, Some(token)).await;
        assert_accepted(case, &replayed);
        assert_eq!(
            reported_status(&replayed),
            DEFINITION_STATUS_CREATING,
            "case {case:?}: the replay reports the definition it created, got {replayed:?}"
        );

        // A different document under that token, a token no earlier call used, and no
        // token at all are all refused: the workspace already has a definition.
        for (which, reply) in [
            (
                "same token, other document",
                create(&amp, region, &ws, &second, Some(token)).await,
            ),
            (
                "a fresh token",
                create(&amp, region, &ws, &second, Some("fresh-token")).await,
            ),
            ("no token", create(&amp, region, &ws, &second, None).await),
        ] {
            let which = format!("{case}: {which}");
            assert_conflict(&which, &reply, &ws);
            // The refusal reports the one constant sentence, carrying nothing of the
            // request: the workspace it refused rides in `resourceId` instead.
            assert_eq!(
                text(&reply.body["message"]),
                ALERT_MANAGER_DEFINITION_EXISTS_MESSAGE,
                "case {which:?}: a second create must report the existing-definition \
                 sentence, got {reply:?}"
            );
        }

        wait_for_due_transition();
        let late = create(&amp, region, &ws, &first, Some(token)).await;
        assert_accepted(case, &late);
        assert_eq!(
            reported_status(&late),
            DEFINITION_STATUS_ACTIVE,
            "case {case:?}: the late replay reports the settled definition, got {late:?}"
        );
        assert_eq!(
            read_back(case, &amp, region, &ws).await,
            first,
            "case {case:?}: no replay or refused create stores the second document"
        );
    }
}

/// The record's whole life: a transient definition refuses an update and a delete
/// ("Update/Deletion is disallowed until definition is ACTIVE and workspace status is
/// ACTIVE"); an update replaces the document, keeps `createdAt` and settles back to
/// `ACTIVE`, replayable on its own token but never on one already spent; a delete leaves
/// the definition `DELETING` until the removal is due and then gone, after which the
/// workspace takes a fresh one; and deleting the workspace releases it. The stored
/// document reaches no debug rendering of the store at any point.
#[tokio::test]
async fn the_definition_lifecycle_is_answered_from_the_stored_record() {
    let amp = Amp::new();
    let region = "us-west-2";
    // A receiver credential, which an Alertmanager configuration routinely carries.
    let secret = "s3cr3t-smtp-password";
    let first =
        format!("alertmanager_config: |\n  route:\n    receiver: paged\n  credentials: {secret}\n")
            .into_bytes();
    let second = with_template_files();

    // A definition the clock has not finished creating refuses both mutations.
    let transient = amp.active_workspace(region, "amd-creating").await;
    create(&amp, region, &transient, &first, None).await;
    assert_conflict(
        "definition CREATING, update",
        &put(&amp, region, &transient, &second, None).await,
        &transient,
    );
    assert_conflict(
        "definition CREATING, delete",
        &delete(&amp, region, &transient, None).await,
        &transient,
    );

    let case = "lifecycle";
    let ws = amp.active_workspace(region, "amd-lifecycle").await;
    assert_accepted(case, &create(&amp, region, &ws, &first, Some("token-1")).await);

    // Nothing renders the document: a debug rendering of the store reports its size.
    let rendered = format!("{:?}", amp.store().lock());
    assert!(
        !rendered.contains(secret) && !rendered.contains("alertmanager_config"),
        "case {case:?}: a debug rendering must reproduce neither the document nor its \
         secret, got {rendered:?}"
    );
    assert!(
        rendered.contains(&format!("data_bytes: {}", first.len())),
        "case {case:?}: the rendering must report the document's size instead, got {rendered:?}"
    );
    wait_for_due_transition();
    let (created_at, modified_at) = instants(case, &describe(&amp, region, &ws).await);

    // The create's token on an UPDATE is a different call, so the update applies.
    let updated = put(&amp, region, &ws, &second, Some("token-1")).await;
    assert_accepted(case, &updated);
    assert_eq!(
        reported_status(&updated),
        DEFINITION_STATUS_UPDATING,
        "case {case:?}: an update moves the definition to UPDATING and a create's token \
         does not make it a replay, got {updated:?}"
    );
    let during = describe(&amp, region, &ws).await;
    let (created_after, modified_after) = instants(case, &during);
    assert_eq!(
        (stored_data(case, &during), created_after),
        (second.clone(), created_at),
        "case {case:?}: the update stores the new document and leaves createdAt, got {during:?}"
    );
    assert!(
        modified_after >= modified_at,
        "case {case:?}: modifiedAt must not move backwards: {modified_after} < {modified_at}"
    );

    // Replaying that exact update applies nothing further; the same token carrying another
    // document is not that call, before the transition is due and after it, where the
    // status rule would otherwise allow an update.
    assert_accepted(case, &put(&amp, region, &ws, &second, Some("token-1")).await);
    assert_conflict(
        "pending update, token with another document",
        &put(&amp, region, &ws, &first, Some("token-1")).await,
        &ws,
    );
    wait_for_due_transition();
    let settled = describe(&amp, region, &ws).await;
    assert_eq!(
        (stored_status(&settled), stored_data(case, &settled)),
        (DEFINITION_STATUS_ACTIVE.to_string(), second.clone()),
        "case {case:?}: the due transition settles the update, got {settled:?}"
    );
    assert_conflict(
        "spent token with another document",
        &put(&amp, region, &ws, &first, Some("token-1")).await,
        &ws,
    );

    // A FRESH token with that document is a different call, and is applied.
    assert_accepted(case, &put(&amp, region, &ws, &first, Some("token-2")).await);
    wait_for_due_transition();
    assert_eq!(
        read_back(case, &amp, region, &ws).await,
        first,
        "case {case:?}: the accepted update must be what later reads report"
    );

    // The delete starts a removal the clock finishes, and is replayable on its token.
    assert_accepted(case, &delete(&amp, region, &ws, Some("delete-1")).await);
    let deleting = describe(&amp, region, &ws).await;
    assert_eq!(
        stored_status(&deleting),
        DEFINITION_STATUS_DELETING,
        "case {case:?}: before the removal is due the definition is DELETING, got {deleting:?}"
    );
    assert_accepted(case, &delete(&amp, region, &ws, Some("delete-1")).await);
    wait_for_due_transition();
    assert_refused(
        "after the removal is due",
        &describe(&amp, region, &ws).await,
        404,
        "ResourceNotFoundException",
        &ws,
        ALERT_MANAGER_DEFINITION_RESOURCE_TYPE,
    );

    // The workspace is free to take a fresh definition, which is a NEW record.
    assert_accepted(case, &create(&amp, region, &ws, &second, None).await);
    assert_eq!(
        read_back(case, &amp, region, &ws).await,
        second,
        "case {case:?}: the fresh definition holds the new document"
    );
    wait_for_due_transition();

    // The definition belongs to its workspace: deleting the workspace releases it.
    assert_accepted(
        "after the workspace is gone",
        &amp
            .call("DELETE", &format!("/workspaces/{ws}"), region, "")
            .await,
    );
    wait_for_due_transition();
    assert_refused(
        "after the workspace is gone",
        &describe(&amp, region, &ws).await,
        404,
        "ResourceNotFoundException",
        &ws,
        WORKSPACE_RESOURCE_TYPE,
    );
}

/// Every definition operation refuses a workspace that does not exist, and refuses a
/// workspace that holds no definition — each naming the resource that is missing. A
/// member that fails its own rule is refused before either resource is resolved, and a
/// workspace the clock has not finished creating cannot take a definition.
#[tokio::test]
async fn operations_report_not_found_for_an_absent_workspace_or_definition() {
    let amp = Amp::new();
    let region = "us-east-1";
    let absent = "ws-00000000-0000-0000-0000-000000000000";
    let empty = amp.active_workspace(region, "amd-not-found").await;
    let data = block_scalar();

    for (case, reply) in [
        ("create", create(&amp, region, absent, &data, None).await),
        ("describe", describe(&amp, region, absent).await),
        ("update", put(&amp, region, absent, &data, None).await),
        ("delete", delete(&amp, region, absent, None).await),
    ] {
        assert_refused(
            &format!("{case} on an absent workspace"),
            &reply,
            404,
            "ResourceNotFoundException",
            absent,
            WORKSPACE_RESOURCE_TYPE,
        );
    }

    for (case, reply) in [
        ("describe", describe(&amp, region, &empty).await),
        ("update", put(&amp, region, &empty, &data, None).await),
        ("delete", delete(&amp, region, &empty, None).await),
    ] {
        assert_refused(
            &format!("{case} with no definition stored"),
            &reply,
            404,
            "ResourceNotFoundException",
            &empty,
            ALERT_MANAGER_DEFINITION_RESOURCE_TYPE,
        );
    }

    for (case, reply) in [
        (
            "create on an absent workspace",
            create(&amp, region, absent, b"not: a definition", None).await,
        ),
        (
            "update on a workspace holding no definition",
            put(&amp, region, &empty, b"not: a definition", None).await,
        ),
    ] {
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "ValidationException".to_string()),
            "case {case:?}: a member that fails its own rule is refused before the \
             resource is resolved, got {reply:?}"
        );
        assert_eq!(
            text(&reply.body["fieldList"][0]["name"]),
            "data",
            "case {case:?}: the refusal must name the member it refused, got {reply:?}"
        );
    }

    // A workspace the clock has not finished creating holds no definition, so the refusal
    // names the workspace's own state.
    let fresh = amp
        .call(
            "POST",
            "/workspaces",
            region,
            &serde_json::json!({ "alias": "amd-ws-creating" }).to_string(),
        )
        .await;
    assert_accepted("workspace CREATING, create", &fresh);
    let creating_ws = text(&fresh.body["workspaceId"]);
    assert_refused(
        "workspace CREATING, create",
        &create(&amp, region, &creating_ws, &data, None).await,
        409,
        "ConflictException",
        &creating_ws,
        WORKSPACE_RESOURCE_TYPE,
    );
}
