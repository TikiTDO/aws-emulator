// SPDX-License-Identifier: CC0-1.0

//! The members of a stored receipt rule whose DEFAULT the service supplies, read back through
//! `DescribeReceiptRule`.
//!
//! Three members of the receipt-rule shapes document the value they hold when the request
//! omits them — `ReceiptRule$TlsPolicy` ("The default is `Optional`"), `SNSAction$Encoding`
//! ("The default value is UTF-8") and `LambdaAction$InvocationType` ("The default value is
//! `Event`") — and `ReceiptRule$Enabled` and `ReceiptRule$ScanEnabled` carry a modelled
//! `@default false`. So the rule this suite pins is: a member documented with a default is
//! optional in the REQUEST and not in the resource — the default is materialised where the
//! rule is stored, so every read of the rule reports it and a caller that reads a rule and
//! writes it back loses nothing. A value the request DOES carry is kept as given.
//!
//! `UpdateReceiptRule` stores the rule WHOLE, so an update naming only the rule's name resets
//! every other member to its default — which is the same reading, applied to the operation
//! that replaces a rule.
//!
//! The rules here name destinations of other services, and no Lambda or Amazon SNS is mounted
//! beside this service, so the destination pre-flight is told nothing about them and refuses
//! nothing (`ses_receipt_rule_destination_preflight` is the suite about that half). What this
//! suite reads is what the STORE holds for the rule.

use crate::harness::{call, element, emulator, field, topic_arn, Field, REGION};
use smithy_server_runtime::MultiServiceRouter;

/// The rule set every rule below is added to.
const RULE_SET: &str = "default-rules";

/// The account this emulator serves, which owns the function the `LambdaAction` names.
fn function_arn(name: &str) -> String {
    format!(
        "arn:aws:lambda:{REGION}:{}:function:{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

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

/// One `CreateReceiptRule` carrying `fields` beside the rule's name.
async fn create_rule(
    router: &MultiServiceRouter,
    case: &str,
    rule_name: &str,
    fields: &[Field],
) -> String {
    let mut params = vec![field("RuleSetName", RULE_SET), field("Rule.Name", rule_name)];
    params.extend(fields.iter().cloned());
    let (status, body) = call(router, "CreateReceiptRule", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the rule to be stored, got {status} with {body}"
    );
    described_rule(router, case, rule_name).await
}

/// The `<Rule>` element `DescribeReceiptRule` reports for `rule_name`.
async fn described_rule(router: &MultiServiceRouter, case: &str, rule_name: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeReceiptRule",
        &[field("RuleSetName", RULE_SET), field("RuleName", rule_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: describing {rule_name}: expected 200, got {status} with {body}"
    );
    element(&body, "Rule")
        .unwrap_or_else(|| panic!("case {case:?}: expected a reported Rule, got {body}"))
}

/// Assert that `reported` carries `expected` as its `element_name`.
fn assert_member(case: &str, reported: &str, element_name: &str, expected: Option<&str>) {
    let held = element(reported, element_name);
    assert_eq!(
        held.as_deref(),
        expected,
        "case {case:?}: expected {element_name}={expected:?}, got {reported}"
    );
}

/// `ReceiptRule$TlsPolicy` reads back as the value the request carried, and as the documented
/// default when the request carried none.
#[tokio::test]
async fn a_stored_rule_carries_the_documented_tls_policy_default() {
    // The admitted set is the model's (`smithy_query(query="shape",shape="TlsPolicy")`:
    // Require, Optional), and the default is the member's documentation.
    let rows = [
        ("no TlsPolicy at all", None, "Optional"),
        ("TlsPolicy Optional", Some("Optional"), "Optional"),
        ("TlsPolicy Require", Some("Require"), "Require"),
    ];
    let router = emulator();
    create_rule_set(&router).await;
    for (at, (case, carried, expected)) in rows.into_iter().enumerate() {
        let rule_name = format!("tls-rule-{at}");
        let mut fields = vec![
            field("Rule.Enabled", "true"),
            field("Rule.ScanEnabled", "false"),
        ];
        if let Some(carried) = carried {
            fields.push(field("Rule.TlsPolicy", carried));
        }
        let reported = create_rule(&router, case, &rule_name, &fields).await;
        assert_member(case, &reported, "TlsPolicy", Some(expected));
    }
}

/// An action member documented with a default reads back carrying it: `SNSAction$Encoding`
/// and `LambdaAction$InvocationType`.
#[tokio::test]
async fn a_stored_action_carries_its_documented_member_defaults() {
    let router = emulator();
    create_rule_set(&router).await;
    let topic = topic_arn("default-probe-topic");
    let function = function_arn("default-probe-function");

    // `SNSActionEncoding` admits UTF-8 and Base64
    // (`smithy_query(query="shape",shape="SNSActionEncoding")`); the default is the member's
    // documentation.
    let sns_rows = [
        ("an SNSAction with no Encoding", None, "UTF-8"),
        ("an SNSAction encoded UTF-8", Some("UTF-8"), "UTF-8"),
        ("an SNSAction encoded Base64", Some("Base64"), "Base64"),
    ];
    for (at, (case, carried, expected)) in sns_rows.into_iter().enumerate() {
        let rule_name = format!("sns-rule-{at}");
        let mut fields = vec![
            field("Rule.Enabled", "true"),
            field("Rule.ScanEnabled", "false"),
            field("Rule.Actions.member.1.SNSAction.TopicArn", &topic),
        ];
        if let Some(carried) = carried {
            fields.push(field("Rule.Actions.member.1.SNSAction.Encoding", carried));
        }
        let reported = create_rule(&router, case, &rule_name, &fields).await;
        assert_member(case, &reported, "Encoding", Some(expected));
        assert_member(case, &reported, "TopicArn", Some(topic.as_str()));
    }

    // `InvocationType` admits Event and RequestResponse
    // (`smithy_query(query="shape",shape="InvocationType")`); the default is the member's
    // documentation.
    let lambda_rows = [
        ("a LambdaAction with no InvocationType", None, "Event"),
        ("a LambdaAction invoked as an Event", Some("Event"), "Event"),
        (
            "a LambdaAction invoked as a RequestResponse",
            Some("RequestResponse"),
            "RequestResponse",
        ),
    ];
    for (at, (case, carried, expected)) in lambda_rows.into_iter().enumerate() {
        let rule_name = format!("lambda-rule-{at}");
        let mut fields = vec![
            field("Rule.Enabled", "true"),
            field("Rule.ScanEnabled", "false"),
            field("Rule.Actions.member.1.LambdaAction.FunctionArn", &function),
        ];
        if let Some(carried) = carried {
            fields.push(field(
                "Rule.Actions.member.1.LambdaAction.InvocationType",
                carried,
            ));
        }
        let reported = create_rule(&router, case, &rule_name, &fields).await;
        assert_member(case, &reported, "InvocationType", Some(expected));
        assert_member(case, &reported, "FunctionArn", Some(function.as_str()));
    }
}

/// An update stores the rule WHOLE, so a replacement naming only the rule's name reads back
/// with every other member at its default — the TLS policy included.
#[tokio::test]
async fn a_full_replace_update_resets_every_member_to_its_default() {
    let case = "a replacement carrying only the rule's name";
    let rule_name = "replaced-rule";
    let router = emulator();
    create_rule_set(&router).await;
    let topic = topic_arn("replaced-probe-topic");
    let before = create_rule(
        &router,
        case,
        rule_name,
        &[
            field("Rule.Enabled", "true"),
            field("Rule.ScanEnabled", "true"),
            field("Rule.TlsPolicy", "Require"),
            field("Rule.Recipients.member.1", "inbound@example.com"),
            field("Rule.Actions.member.1.SNSAction.TopicArn", &topic),
        ],
    )
    .await;
    assert_member(case, &before, "TlsPolicy", Some("Require"));
    assert_member(case, &before, "Encoding", Some("UTF-8"));

    let (status, body) = call(
        &router,
        "UpdateReceiptRule",
        &[
            field("RuleSetName", RULE_SET),
            field("Rule.Name", rule_name),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the update to be stored, got {status} with {body}"
    );
    let after = described_rule(&router, case, rule_name).await;
    assert_member(case, &after, "TlsPolicy", Some("Optional"));
    assert_member(case, &after, "Enabled", Some("false"));
    assert_member(case, &after, "ScanEnabled", Some("false"));
    assert_member(case, &after, "Recipients", None);
    assert_member(case, &after, "Actions", None);
}
