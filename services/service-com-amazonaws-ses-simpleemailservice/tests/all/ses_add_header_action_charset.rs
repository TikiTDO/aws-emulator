// SPDX-License-Identifier: CC0-1.0

//! The CHARSET an `AddHeaderAction` header name and value are held to, read before anything
//! is stored.
//!
//! `AddHeaderAction$HeaderName` states the form its value takes and the model declares no
//! pattern for it: "The name must contain at least one character, and can contain up to 50
//! characters. It consists of alphanumeric (a–z, A–Z, 0–9) characters and dashes."
//! `AddHeaderAction$HeaderValue` states its own: "This value can contain up to 2048
//! characters. It can't contain newline (\n) or carriage return (\r) characters." So the rule
//! this suite pins is: a member whose documentation states the form its value takes is held to
//! that form where the rule is stored — a value outside it is refused `InvalidParameterValue`
//! (HTTP 400) naming the value the request carried, and nothing is persisted for it — and this
//! holds on `CreateReceiptRule` and `UpdateReceiptRule` alike, which store the same shape.
//!
//! The tables below span what the two forms generalise over: the charset (letters, digits,
//! dashes, and each class outside it), the published bounds and one character past each, and
//! for the value the two control characters it may not carry.
//!
//! The rules here carry an `AddHeaderAction` alone, which addresses no other service, so what
//! is being read is the member's own form rather than any destination's state.

use crate::harness::{call, element, error_envelope, emulator, field, Field};
use smithy_server_runtime::MultiServiceRouter;

/// The rule set every rule below is added to.
const RULE_SET: &str = "header-rules";

/// The error code every structural refusal of a member's own form carries here.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// `CreateReceiptRuleSet`.
async fn create_rule_set(router: &MultiServiceRouter) {
    let (status, body) = call(
        router,
        "CreateReceiptRuleSet",
        &[field("RuleSetName", RULE_SET)],
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the rule set: expected 200, got {status} with {body}"
    );
}

/// The form fields of a rule named `rule_name` whose only action adds the header
/// `(name, value)`.
fn rule_fields(rule_name: &str, name: &str, value: &str) -> Vec<Field> {
    vec![
        field("RuleSetName", RULE_SET),
        field("Rule.Name", rule_name),
        field("Rule.Enabled", "true"),
        field("Rule.ScanEnabled", "false"),
        field("Rule.Actions.member.1.AddHeaderAction.HeaderName", name),
        field("Rule.Actions.member.1.AddHeaderAction.HeaderValue", value),
    ]
}

/// The `<Rule>` element `DescribeReceiptRule` reports for `rule_name`, or `None` when the set
/// holds no rule of that name.
async fn described_rule(router: &MultiServiceRouter, rule_name: &str) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeReceiptRule",
        &[field("RuleSetName", RULE_SET), field("RuleName", rule_name)],
    )
    .await;
    (status == 200).then(|| element(&body, "Rule")).flatten()
}

/// Drive one `(header name, header value)` through both operations that store a rule, and
/// assert the rule is stored exactly when the two members are in the documented form.
async fn assert_header(case: &str, name: &str, value: &str, refused_with: Option<String>) {
    let router = emulator();
    create_rule_set(&router).await;
    let rule_name = "header-rule";
    for action in ["CreateReceiptRule", "UpdateReceiptRule"] {
        if action == "UpdateReceiptRule" && refused_with.is_some() {
            // An update replaces a rule the set HOLDS, so a stored rule has to exist for the
            // refusal to be about the header rather than about the missing rule.
            let (status, body) = call(
                &router,
                "CreateReceiptRule",
                &rule_fields(rule_name, "X-Kept", "kept"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: seeding the last-good rule: expected 200, got {status} with {body}"
            );
        }
        let (status, body) = call(&router, action, &rule_fields(rule_name, name, value)).await;
        match &refused_with {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?} ({action}): expected the rule to be stored, got {status} with \
                     {body}"
                );
                let reported = described_rule(&router, rule_name).await.unwrap_or_default();
                assert_eq!(
                    element(&reported, "HeaderName").as_deref(),
                    Some(name),
                    "case {case:?} ({action}): expected the stored rule to carry the header \
                     name, got {reported}"
                );
            }
            Some(message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?} ({action}): expected HTTP 400, got {status} with {body}"
                );
                let envelope = error_envelope(&body);
                assert_eq!(
                    envelope,
                    (
                        Some(INVALID_PARAMETER_VALUE.to_string()),
                        Some(message.clone())
                    ),
                    "case {case:?} ({action}): expected ({INVALID_PARAMETER_VALUE:?}, \
                     {message:?}), got {envelope:?}"
                );
                let reported = described_rule(&router, rule_name).await;
                let stored_header = reported
                    .as_deref()
                    .and_then(|rule| element(rule, "HeaderName"));
                assert_ne!(
                    stored_header.as_deref(),
                    Some(name),
                    "case {case:?} ({action}): expected the refused header not to be stored, got \
                     {reported:?}"
                );
            }
        }
    }
}

/// A header name is accepted exactly when it is inside the charset and the length
/// `AddHeaderAction$HeaderName` documents.
#[tokio::test]
async fn an_add_header_action_header_name_is_held_to_its_documented_charset() {
    // The accepted set is the member's own: "alphanumeric (a–z, A–Z, 0–9) characters and
    // dashes", at least one and at most 50 of them.
    let fifty = "X".repeat(50);
    let fifty_one = "X".repeat(51);
    let rows: [(&str, String, bool); 11] = [
        ("letters, digits and dashes", "X-Sim-Probe-7".to_string(), true),
        ("a single character", "a".to_string(), true),
        ("digits alone", "2026".to_string(), true),
        ("mixed case", "X-mIxEd-Case".to_string(), true),
        ("the longest name the member admits", fifty, true),
        ("one character past that length", fifty_one, false),
        ("no character at all", String::new(), false),
        ("a space", "X Sim2Real Probe".to_string(), false),
        ("an exclamation mark", "X-Probe!".to_string(), false),
        ("an underscore", "X_Probe".to_string(), false),
        ("a non-ASCII letter", "X-Pröbe".to_string(), false),
    ];
    for (case, name, accepted) in rows {
        let refused_with = (!accepted).then(|| format!("Invalid header name: {name}"));
        assert_header(case, &name, "probe", refused_with).await;
    }
}

/// A header value is accepted exactly when it is inside the length and free of the two
/// control characters `AddHeaderAction$HeaderValue` documents.
#[tokio::test]
async fn an_add_header_action_header_value_is_held_to_its_documented_form() {
    let longest = "v".repeat(2048);
    let one_past = "v".repeat(2049);
    let rows: [(&str, String, bool); 7] = [
        ("plain text", "spam=high; probe".to_string(), true),
        ("punctuation and spaces", "a value, with punctuation!".to_string(), true),
        ("the longest value the member admits", longest, true),
        ("one character past that length", one_past, false),
        ("a newline", "two\nlines".to_string(), false),
        ("a carriage return", "two\rlines".to_string(), false),
        ("both control characters", "two\r\nlines".to_string(), false),
    ];
    for (case, value, accepted) in rows {
        let refused_with = (!accepted).then(|| format!("Invalid header value: {value}"));
        assert_header(case, "X-Probe", &value, refused_with).await;
    }
}
