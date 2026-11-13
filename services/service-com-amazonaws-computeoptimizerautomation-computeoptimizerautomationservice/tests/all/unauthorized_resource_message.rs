// SPDX-License-Identifier: CC0-1.0

//! What the BODY of this service's authorization refusal says.
//!
//! A resource string that names no automation rule of this service is refused
//! `AccessDeniedException` (403) before the request is parsed — which operation
//! answers that way, and with which code and status, is
//! [`crate::automation_event_and_tagging`]'s
//! `a_string_naming_no_rule_of_this_service_is_unauthorized`. This suite asserts
//! the SENTENCE it carries.
//!
//! The rule: an authorization decision is taken from three things the request
//! itself states — the identity it is signed with, the action its operation asks
//! for and the resource string it carries — so the refusal names all three, as
//! AWS words an IAM denial: `User: <principal> is not authorized to perform:
//! <service prefix>:<Operation> on resource: <the string as sent>`.
//!
//! The rows vary what the rule generalises over: the OPERATION refusing (every
//! operation of this service whose input names a resource by ARN, so the action
//! in the sentence is the one the request named) and the FORM the resource string
//! takes (each way the modelled `RuleArn` can fail to name a rule of this
//! service, including the ARN of a rule that does not exist in another account's
//! namespace). The CREDENTIAL the sentence names the principal of is varied in
//! [`crate::denial_principal_agrees_with_sts`].
//!
//! Every case drives the service through the awsJson1_0 router a real caller
//! reaches, using [`crate::automation_rule_lifecycle`]'s harness.

use serde_json::{json, Value};
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

use crate::automation_rule_lifecycle::{
    assert_refused, call, call_ok, emulator, member, minimal_rule, string_member,
};
use crate::error_payloads::error_message;

/// The IAM service prefix every action of this service carries: the signing name
/// the model vends (`sigv4Name`, equal to its `endpointPrefix`, from
/// `smithy_query(query="service_info")`).
const IAM_SERVICE_PREFIX: &str = "aco-automation";

/// The sentence an authorization refusal carries, as the rule under test states
/// it.
///
/// `pub(crate)` because the sibling suites in this binary assert the SAME sentence
/// about other inputs — the credential it names the principal of
/// ([`crate::denial_principal_agrees_with_sts`]) — rather than each restating the
/// rule's wording.
pub(crate) fn expected_message(principal: &str, operation: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: \
         {IAM_SERVICE_PREFIX}:{operation} on resource: {resource}"
    )
}

/// Every operation of this service whose input names a resource by ARN:
/// `ruleArn` on the three rule operations and `resourceArn` on the three tagging
/// ones (`smithy_query(query="operation",operation="GetAutomationRule,UpdateAutomationRule,DeleteAutomationRule,TagResource,UntagResource,ListTagsForResource")`).
///
/// `pub(crate)` for the sibling suites that assert the SAME sentence about other
/// inputs, so there is ONE list of the operations the rule covers.
pub(crate) const RESOURCE_OPERATIONS: [&str; 6] = [
    "GetAutomationRule",
    "UpdateAutomationRule",
    "DeleteAutomationRule",
    "TagResource",
    "UntagResource",
    "ListTagsForResource",
];

/// The request `operation` takes for `resource`, with every other required
/// member of that operation filled in, so no case is refused for a reason other
/// than the resource string it carries. `pub(crate)` for the sibling suite, as
/// [`expected_message`] is.
pub(crate) fn request_for(operation: &str, resource: &str) -> Value {
    match operation {
        "GetAutomationRule" => json!({"ruleArn": resource}),
        "UpdateAutomationRule" => {
            json!({"ruleArn": resource, "ruleRevision": 1, "status": "Inactive"})
        }
        "DeleteAutomationRule" => json!({"ruleArn": resource, "ruleRevision": 1}),
        "ListTagsForResource" => json!({"resourceArn": resource}),
        "TagResource" => json!({
            "resourceArn": resource,
            "ruleRevision": 1,
            "tags": [{"key": "team", "value": "sim2real"}]
        }),
        "UntagResource" => json!({
            "resourceArn": resource,
            "ruleRevision": 1,
            "tagKeys": ["team"]
        }),
        other => panic!("no request is defined for {other}"),
    }
}

/// The resource strings under test: one row per way a value can fail to name an
/// automation rule of this service, whose form is the model's `RuleArn`
/// (`^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`,
/// from `smithy_query(query="shape",shape="RuleArn")`).
///
/// `pub(crate)` for the same reason [`RESOURCE_OPERATIONS`] is: the sibling
/// suites judge the same forms rather than each listing their own.
pub(crate) fn unauthorized_resources() -> Vec<(&'static str, String)> {
    let account = engine::EMULATOR_ACCOUNT_ID;
    vec![
        ("not-an-arn", "not-an-arn".to_string()),
        ("empty", String::new()),
        ("a bare rule id", "42d39f82e52fa35c".to_string()),
        (
            "a sentence with spaces",
            "the rule I meant".to_string(),
        ),
        (
            "another service's namespace",
            format!("arn:aws:compute-optimiser::{account}:automation-rule/42d39f82e52fa35c"),
        ),
        (
            "another partition",
            format!("arn:aws-cn:compute-optimizer::{account}:automation-rule/42d39f82e52fa35c"),
        ),
        (
            "a filled-in region segment",
            format!("arn:aws:compute-optimizer:us-east-1:{account}:automation-rule/42d39f82e52fa35c"),
        ),
        (
            "an account that is not 12 digits",
            "arn:aws:compute-optimizer::1234:automation-rule/42d39f82e52fa35c".to_string(),
        ),
        (
            "another resource type",
            format!("arn:aws:compute-optimizer::{account}:rule/42d39f82e52fa35c"),
        ),
        (
            "no rule id at all",
            format!("arn:aws:compute-optimizer::{account}:automation-rule/"),
        ),
        (
            "uppercased",
            format!("ARN:AWS:COMPUTE-OPTIMIZER::{account}:AUTOMATION-RULE/42D39F82E52FA35C"),
        ),
    ]
}

/// The refusal names the caller, the action the request asked for and the
/// resource string exactly as sent — for every operation that takes a resource
/// and every form such a string can take.
#[tokio::test]
async fn the_denial_names_the_caller_the_action_and_the_resource_as_sent() {
    let router = emulator();
    // The key the harness presents names no session, so it speaks for the
    // identity this emulator publishes — the one `sts:GetCallerIdentity` answers
    // with for it.
    let principal = sts_state::ambient_identity().arn;

    for operation in RESOURCE_OPERATIONS {
        for (form, resource) in unauthorized_resources() {
            let case = format!("{operation} on {form}");
            let answer = call(&router, operation, &request_for(operation, &resource)).await;
            assert_refused(answer.clone(), 403, "AccessDeniedException", &case);
            let stated = error_message(&answer.1);
            assert_eq!(
                stated,
                expected_message(&principal, operation, &resource),
                "case {case:?}: expected the IAM denial built from the request, got {stated:?}"
            );
        }
    }
}

/// The boundary: a string that DOES name an automation rule of this service is
/// never answered this way — the request reaches the operation, which serves it.
#[tokio::test]
async fn a_string_that_names_a_rule_is_not_refused_in_these_words() {
    let router = emulator();
    let created = call_ok(&router, "CreateAutomationRule", &minimal_rule("authorized"), "setup").await;
    let rule_arn = string_member(&created, "ruleArn");
    let revision = member(&created, "ruleRevision").as_i64().unwrap_or_default();

    let read_back = call_ok(
        &router,
        "GetAutomationRule",
        &json!({"ruleArn": rule_arn}),
        "the rule's own ARN",
    )
    .await;
    assert_eq!(
        string_member(&read_back, "ruleArn"),
        rule_arn,
        "case \"the rule's own ARN\": the read must answer the rule that was created, \
         got {read_back}"
    );

    // A well-formed ARN of this service that names no stored rule is the 404 the
    // operation itself answers, not an authorization refusal: the gate judges the
    // FORM of the string, never whether the resource exists.
    let absent = format!(
        "arn:aws:compute-optimizer::{}:automation-rule/0000000000000000",
        engine::EMULATOR_ACCOUNT_ID
    );
    let answer = call(&router, "GetAutomationRule", &json!({"ruleArn": absent})).await;
    assert_refused(
        answer.clone(),
        404,
        "ResourceNotFoundException",
        "a well-formed ARN of no rule",
    );
    let stated = error_message(&answer.1);
    assert!(
        !stated.contains("is not authorized to perform"),
        "case \"a well-formed ARN of no rule\": expected the operation's own not-found, \
         got {stated:?}"
    );

    // And the rule the refused call did not address is still there, at the
    // revision the create reported: nothing the gate refused ran.
    let unchanged = call_ok(
        &router,
        "GetAutomationRule",
        &json!({"ruleArn": rule_arn}),
        "nothing ran",
    )
    .await;
    assert_eq!(
        member(&unchanged, "ruleRevision").as_i64(),
        Some(revision),
        "case \"nothing ran\": a refused request must change nothing, got {unchanged}"
    );
}
