// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `ValidationException` this service answers a value
//! outside an enumerated type's set with.
//!
//! The rule these pin, over a TABLE of operations and members rather than one recorded
//! request: a value the request BODY carries for a member whose type enumerates its values
//! is refused 400 `ValidationException` whose whole message is the schema finding that
//! front door reports — `[Validation] [instance value ("<the value sent>") not found in
//! enum (possible values: ["<v1>","<v2>",…])]`, the values double-quoted, comma-separated
//! with no spaces and in the order the MODEL declares them — and nothing else: no `Reason`
//! and no `Fields`. The variable parts come from the request and from the model, so a
//! client that matches the text, or parses the permitted set out of it, always can.
//!
//! A value bound to the URI rather than to the body does not travel through that layer, so
//! its refusal is the service's own field validation and names the enumerated TYPE it was
//! parsed into; the last suites pin that split.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{created_id, encode_path_segment, minimal_create, Answer, Client};

/// Identifiers that are well-formed (`^[0-9a-f]{32}$`) and name nothing: the boundary
/// refuses the request before any lookup, so a row that is about the REFUSAL need not own
/// a resource.
const ABSENT_HEX_ID: &str = "0123456789abcdef0123456789abcdef";

/// A custom lens ARN that is well-formed and names no imported lens, for the same reason.
const ABSENT_LENS_ARN: &str =
    "arn:aws:wellarchitected:us-west-2:000000000000:lens/0123456789abcdef0123456789abcdef";

/// The alias of the AWS Well-Architected Framework lens.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// The value sets the model declares, IN ITS DECLARED ORDER — which is the order the
/// finding lists them in. From `smithy_query(query="shape", shape="PermissionType")` /
/// `"AnswerReason"` / `"ChoiceStatus"` / `"WorkloadEnvironment"` /
/// `"ShareInvitationAction"` / `"LensStatusType"`; the service's own
/// `PERMISSION_TYPE_VALUES` and its siblings are sorted, so they are NOT what a finding
/// lists.
const PERMISSION_TYPE: &[&str] = &["READONLY", "CONTRIBUTOR"];
const ANSWER_REASON: &[&str] = &[
    "OUT_OF_SCOPE",
    "BUSINESS_PRIORITIES",
    "ARCHITECTURE_CONSTRAINTS",
    "OTHER",
    "NONE",
];
const CHOICE_STATUS: &[&str] = &["SELECTED", "NOT_APPLICABLE", "UNSELECTED"];
const WORKLOAD_ENVIRONMENT: &[&str] = &["PRODUCTION", "PREPRODUCTION"];
const SHARE_INVITATION_ACTION: &[&str] = &["ACCEPT", "REJECT"];
const LENS_STATUS_TYPE: &[&str] = &["ALL", "DRAFT", "PUBLISHED"];

/// The whole `Message` the refusal of `value` against `permitted` must carry.
fn expected_message(value: &str, permitted: &[&str]) -> String {
    let quoted: Vec<String> = permitted
        .iter()
        .map(|admitted| format!("\"{admitted}\""))
        .collect();
    format!(
        "[Validation] [instance value (\"{value}\") not found in enum (possible values: [{}])]",
        quoted.join(",")
    )
}

/// One row: the request to send and the member's declared value set.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Value,
    /// The value the request sends for the enumerated member.
    sent: String,
    permitted: &'static [&'static str],
}

fn assert_schema_finding(row: &Row, answer: &Answer) {
    let Row {
        case,
        sent,
        permitted,
        ..
    } = row;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Message").and_then(Value::as_str),
        Some(expected_message(sent, permitted).as_str()),
        "case {case:?}: the refusal must state the schema finding, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Reason"),
        None,
        "case {case:?}: a schema finding is not classified, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Fields"),
        None,
        "case {case:?}: a schema finding reports no per-member finding, got {answer:?}"
    );
}

/// Every body member of this service whose type enumerates its values states the same
/// finding. The rows span the operation, the member's depth in the body (a top-level
/// member, a member of a map's value) and the value sent (a member of another enum, a
/// lowercased member of this one, a value no enum publishes, and the empty string).
#[tokio::test]
async fn an_out_of_enum_body_value_is_refused_with_the_schema_finding() {
    let client = Client::new();
    // One real workload, so the rows that carry a valid body except for the enum value are
    // refused on that value rather than on a lookup.
    let workload_id = created_id(
        &client
            .create(minimal_create("enum-wording-host", "tok-enum-host"))
            .await,
        "host",
    );

    let mut rows: Vec<Row> = Vec::new();
    for sent in ["READONLY_EXTRA", "readonly", "PENDING", ""] {
        rows.push(Row {
            case: "CreateWorkloadShare$PermissionType",
            method: "POST",
            path: format!("/workloads/{workload_id}/shares"),
            body: json!({
                "SharedWith": "111122223333",
                "PermissionType": sent,
                "ClientRequestToken": format!("tok-share-{sent}"),
            }),
            sent: sent.to_string(),
            permitted: PERMISSION_TYPE,
        });
        rows.push(Row {
            case: "UpdateWorkloadShare$PermissionType",
            method: "PATCH",
            path: format!("/workloads/{workload_id}/shares/{ABSENT_HEX_ID}"),
            body: json!({ "PermissionType": sent }),
            sent: sent.to_string(),
            permitted: PERMISSION_TYPE,
        });
    }
    for sent in ["SIM2REAL_PROBE", "out_of_scope", ""] {
        rows.push(Row {
            case: "UpdateAnswer$Reason",
            method: "PATCH",
            path: format!(
                "/workloads/{}/lensReviews/{}/answers/organization-priorities",
                encode_path_segment(&workload_id),
                encode_path_segment(FRAMEWORK_LENS)
            ),
            body: json!({ "Reason": sent }),
            sent: sent.to_string(),
            permitted: ANSWER_REASON,
        });
        rows.push(Row {
            // A member of a MAP's value: the finding is the same at any depth of the body.
            case: "UpdateAnswer$ChoiceUpdates.Status",
            method: "PATCH",
            path: format!(
                "/workloads/{}/lensReviews/{}/answers/organization-priorities",
                encode_path_segment(&workload_id),
                encode_path_segment(FRAMEWORK_LENS)
            ),
            body: json!({
                "ChoiceUpdates": {
                    "ops_priorities_ext_cust_needs": { "Status": sent }
                }
            }),
            sent: sent.to_string(),
            permitted: CHOICE_STATUS,
        });
    }
    for sent in ["STAGING", "production", ""] {
        let mut body = minimal_create(&format!("enum-env-{sent}"), &format!("tok-env-{sent}"));
        body["Environment"] = json!(sent);
        rows.push(Row {
            case: "CreateWorkload$Environment",
            method: "POST",
            path: "/workloads".to_string(),
            body,
            sent: sent.to_string(),
            permitted: WORKLOAD_ENVIRONMENT,
        });
    }
    for sent in ["IGNORE", "accept", ""] {
        rows.push(Row {
            case: "UpdateShareInvitation$ShareInvitationAction",
            method: "PATCH",
            path: format!("/shareInvitations/{ABSENT_HEX_ID}"),
            body: json!({ "ShareInvitationAction": sent }),
            sent: sent.to_string(),
            permitted: SHARE_INVITATION_ACTION,
        });
    }

    for row in &rows {
        let answer = client
            .send(row.method, &row.path, Some(row.body.clone()))
            .await;
        assert_schema_finding(row, &answer);
    }
}

/// A member of the same enum that IS admitted is not refused by that layer at all: the
/// value set the finding lists is the one the service accepts. Every published member of
/// `PermissionType` is accepted on a share of a real workload.
#[tokio::test]
async fn every_published_member_of_the_enum_is_accepted() {
    let client = Client::new();
    let workload_id = created_id(
        &client
            .create(minimal_create("enum-accepted-host", "tok-enum-accepted"))
            .await,
        "host",
    );

    for (index, permission_type) in PERMISSION_TYPE.iter().enumerate() {
        let answer = client
            .send(
                "POST",
                &format!("/workloads/{workload_id}/shares"),
                Some(json!({
                    "SharedWith": format!("1111222233{index:02}"),
                    "PermissionType": permission_type,
                    "ClientRequestToken": format!("tok-accepted-{permission_type}"),
                })),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {permission_type:?}: a published member must be accepted, got {answer:?}"
        );
    }
}

/// The other side of the split: a value bound to the URI's query string does not travel
/// through the body layer, so its refusal is the service's own field validation — it names
/// the enumerated TYPE the value was parsed into and is NOT the schema finding. Rows differ
/// along the value sent.
#[tokio::test]
async fn an_out_of_enum_query_value_is_refused_naming_its_type() {
    let client = Client::new();
    for sent in ["SIM2REAL", "draft", "EVERYTHING"] {
        let answer = client
            .send(
                "DELETE",
                &format!(
                    "/lenses/{}?LensStatus={sent}&ClientRequestToken=tok-delete-{sent}",
                    encode_path_segment(ABSENT_LENS_ARN)
                ),
                None,
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {sent:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {sent:?}: expected ValidationException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Message").and_then(Value::as_str),
            Some("[Validation] The LensStatusType that was passed in is invalid."),
            "case {sent:?}: a refusal of a URI-bound value must name its type, got {answer:?}"
        );
        assert_ne!(
            answer.body.get("Message").and_then(Value::as_str),
            Some(expected_message(sent, LENS_STATUS_TYPE).as_str()),
            "case {sent:?}: a URI-bound value is not reported as a body schema finding, got \
             {answer:?}"
        );
    }
}

/// A request whose body breaks an enumerated set AND a member's `@length` is reported
/// wholly in the finding layer's voice: that layer measures both classes, so it lists both
/// findings inside the one pair of brackets, in the order the validating walk found them.
/// Pinned so a message can never come back half in this layer's voice and half in the
/// framework's.
#[tokio::test]
async fn a_body_breaking_two_classes_at_once_lists_both_findings() {
    let client = Client::new();
    let mut body = minimal_create("ab", "tok-mixed");
    body["Environment"] = json!("STAGING");
    let answer = client.send("POST", "/workloads", Some(body)).await;
    assert_eq!(
        answer.status, 400,
        "a request breaking two constraint classes is still a 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        answer.body.get("Message").and_then(Value::as_str),
        Some(
            format!(
                "[Validation] [string \"ab\" is too short (length: 2, required minimum: 3), \
                 instance value (\"STAGING\") not found in enum (possible values: [{}])]",
                WORKLOAD_ENVIRONMENT
                    .iter()
                    .map(|admitted| format!("\"{admitted}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .as_str()
        ),
        "both findings must be listed once, got {answer:?}"
    );
    // Nothing was recorded: the refusal is about the request, not about the store.
    let listed = client.list(json!({})).await;
    assert_eq!(
        listed
            .body
            .get("WorkloadSummaries")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused create must record nothing, got {listed:?}"
    );
}
