// SPDX-License-Identifier: CC0-1.0

//! WHICH IAM actions permit an operation of this service, driven over the real wire path (an
//! awsJson1_0 request through the multi-service router into this service mounted beside IAM
//! and STS on one shared engine), so what is asserted is what a client sees.
//!
//! RULE. AWS decides a call of this service over the SET of actions that permit the
//! operation — the action it is performed under, `aws-marketplace:<OperationName>`, plus the
//! coarse `aws-marketplace:ViewSubscriptions` for an operation that only reports what an
//! account's agreements are. An `Allow` of ANY of them serves the call; an explicit `Deny`
//! of ANY of them refuses it; with neither, the call falls to IAM's default deny. A refusal
//! is the modelled `AccessDeniedException` at `@httpError(403)`, taken BEFORE the store is
//! read, and its sentence names the action asked for, the resource the action takes and —
//! because a decision over several actions is attributable to no single one of them — every
//! action of the set in one clause that states neither of IAM's two rules. An operation one
//! action alone permits keeps IAM's single-action wording, which does name the rule.
//!
//! The rows span what the rule generalises over: the OPERATION (each of the four operations
//! the coarse action also permits, two reads it does not, a write and the create), the
//! DOCUMENT in force on the principal (none, the operation's own action alone, the coarse
//! action alone, another operation's action alone, an allow of everything beside an explicit
//! deny of the operation's own action, and the same beside an explicit deny of the coarse
//! action) and, in the second test, whether the agreement the call names EXISTS — which the
//! decision never depends on.
//!
//! The harness, the principals and the refusal clause are the ones
//! [`crate::identity_authorization`] states this service's authorization rule with: the same
//! IAM and STS operations write the documents this service reads.

use serde_json::{json, Value};

use crate::identity_authorization::{
    allow, allow_all_but, code_of, evaluated_actions, message_of, Emulator, Verdict,
    ACCESS_DENIED, ANY_RESOURCE, SIGNING_NAME, VIEW_SUBSCRIPTIONS,
};

/// The code and message this service refuses a read of an agreement it does not hold with.
/// The message is entirely fixed text; the STATUS such a refusal carries is this service's
/// own and is pinned by the suites that own it (`crate::state::NOT_FOUND_STATUS`), so it is
/// not restated here.
const NOT_FOUND: (&str, &str) = (
    "ResourceNotFoundException",
    "Provided agreement does not exist.",
);

/// One operation the rule is asserted over: the call that reaches it, and whether the coarse
/// view action also permits it.
struct Operation {
    case: &'static str,
    name: &'static str,
    /// Built from the fixture agreement's id, so a served row really answers from the store.
    body: fn(&str) -> Value,
    /// Whether `aws-marketplace:ViewSubscriptions` also permits this operation.
    viewable: bool,
}

/// Every class of operation the rule spans: the four that only report what an account's
/// agreements are (which the coarse view action also permits), two reads of the request
/// workflows it does not, a write and the create.
fn operations() -> Vec<Operation> {
    vec![
        Operation {
            case: "the report of one agreement",
            name: "DescribeAgreement",
            body: |agreement| json!({"agreementId": agreement}),
            viewable: true,
        },
        Operation {
            case: "the terms an agreement was accepted under",
            name: "GetAgreementTerms",
            body: |agreement| json!({"agreementId": agreement}),
            viewable: true,
        },
        Operation {
            case: "the entitlements an agreement grants",
            name: "GetAgreementEntitlements",
            body: |agreement| json!({"agreementId": agreement}),
            viewable: true,
        },
        Operation {
            case: "the search across agreements",
            name: "SearchAgreements",
            body: |_| {
                json!({"filters": [{"name": "AgreementType", "values": ["PurchaseAgreement"]}]})
            },
            viewable: true,
        },
        Operation {
            case: "a read of a payment request",
            name: "GetAgreementPaymentRequest",
            body: |agreement| {
                json!({"paymentRequestId": "pr-0000000000000001", "agreementId": agreement})
            },
            viewable: false,
        },
        Operation {
            case: "a listing of an agreement's charges",
            name: "ListAgreementCharges",
            body: |agreement| json!({"agreementId": agreement}),
            viewable: false,
        },
        Operation {
            case: "a write",
            name: "CancelAgreement",
            body: |agreement| json!({"agreementId": agreement}),
            viewable: false,
        },
        Operation {
            case: "the create that mints a quote",
            name: "CreateAgreementRequest",
            body: |_| {
                json!({
                    "intent": "NEW",
                    "agreementProposalIdentifier": "ap-permitting",
                    "requestedTerms": [{"id": "LegalTerm"}],
                })
            },
            viewable: false,
        },
    ]
}

/// One document in force on the calling role, and the verdict it reaches for an operation the
/// coarse action also permits and for one only its own action permits.
struct Document {
    case: &'static str,
    /// Built from the action the operation under test is performed under.
    policy: fn(&str) -> Option<Value>,
    /// The verdict for an operation the coarse view action also permits.
    viewable: Verdict,
    /// The verdict for an operation only its own action permits.
    own_only: Verdict,
}

/// Every document class the rule spans.
fn documents() -> Vec<Document> {
    vec![
        Document {
            case: "no policy at all",
            policy: |_| None,
            viewable: Verdict::DefaultDeny,
            own_only: Verdict::DefaultDeny,
        },
        Document {
            case: "the operation's own action alone",
            policy: |own| Some(allow(json!(own))),
            viewable: Verdict::Served,
            own_only: Verdict::Served,
        },
        Document {
            case: "the coarse view action alone",
            policy: |_| Some(allow(json!(coarse_action()))),
            viewable: Verdict::Served,
            own_only: Verdict::DefaultDeny,
        },
        Document {
            case: "another operation's action alone",
            policy: |_| Some(allow(json!(format!("{SIGNING_NAME}:GetBillingAdjustmentRequest")))),
            viewable: Verdict::DefaultDeny,
            own_only: Verdict::DefaultDeny,
        },
        Document {
            case: "everything but an explicit deny of the operation's own action",
            policy: |own| Some(allow_all_but(json!(own))),
            viewable: Verdict::ExplicitDeny,
            own_only: Verdict::ExplicitDeny,
        },
        Document {
            case: "everything but an explicit deny of the coarse view action",
            policy: |_| Some(allow_all_but(json!(coarse_action()))),
            viewable: Verdict::ExplicitDeny,
            own_only: Verdict::Served,
        },
    ]
}

/// The coarse action in full: this service's prefix and the action's own name.
fn coarse_action() -> String {
    format!("{SIGNING_NAME}:{VIEW_SUBSCRIPTIONS}")
}

/// RULE: a call is served where the policies in force allow ANY action that permits the
/// operation, refused where they explicitly deny any of them, and refused by default where
/// they allow none — and the refusal's sentence names every action the decision was taken
/// over.
#[tokio::test]
async fn every_action_that_permits_an_operation_decides_it() {
    for (index, document) in documents().into_iter().enumerate() {
        for operation in operations() {
            let case = format!("{} / {}", document.case, operation.case);
            let emulator = Emulator::new();
            let agreement = emulator
                .given_agreement(&format!("ap-permitting{index}"))
                .await;
            let own = format!("{SIGNING_NAME}:{}", operation.name);
            let role = format!("permitting-{index}-{}", operation.name);
            let session = emulator
                .assumed_role(&role, (document.policy)(&own).as_ref())
                .await;

            let body = (operation.body)(&agreement);
            let (status, answered) = emulator
                .call(&session.access_key_id, operation.name, &body)
                .await;
            let expected = if operation.viewable {
                document.viewable
            } else {
                document.own_only
            };
            if expected == Verdict::Served {
                // A served call reaches the operation, so neither the 403 nor its code is
                // the answer — and neither is a refusal of the body, which every row sends
                // well-formed: what a served row gets is the operation's OWN answer (its
                // report, or the not-found refusal for a request workflow the fixture holds
                // nothing of).
                assert_ne!(
                    (status, code_of(&answered)),
                    ACCESS_DENIED,
                    "case {case:?}: a permitted action must not be refused by the \
                     authorization layer, got {status} with {answered}"
                );
                assert_ne!(
                    code_of(&answered),
                    "ValidationException",
                    "case {case:?}: the row's body is well-formed, so a served call must \
                     reach the operation, got {status} with {answered}"
                );
                continue;
            }
            assert_eq!(
                status, ACCESS_DENIED.0,
                "case {case:?}: expected {}, got {status} with {answered}",
                ACCESS_DENIED.0
            );
            assert_eq!(
                code_of(&answered),
                ACCESS_DENIED.1,
                "case {case:?}: expected {}, got {answered}",
                ACCESS_DENIED.1
            );
            let sentence = format!(
                "User: {} is not authorized to perform: {own} on resource: {ANY_RESOURCE} {}",
                session.arn,
                expected.reason(&evaluated_actions(operation.name))
            );
            assert_eq!(
                message_of(&answered),
                sentence,
                "case {case:?}: expected IAM's wording, got {answered}"
            );
        }
    }
}

/// RULE: the decision precedes the store, so an explicit deny of an action that permits the
/// read refuses the identical call 403 whether the agreement it names exists or not — and
/// taking the deny off returns both calls to the answers the store gives, the report for the
/// agreement it holds and the not-found refusal for an id nothing answers to.
#[tokio::test]
async fn a_denied_read_is_refused_whether_the_agreement_exists_or_not() {
    let emulator = Emulator::new();
    let agreement = emulator.given_agreement("ap-deniedread").await;
    let role = "permitting-denied-read";
    let own = format!("{SIGNING_NAME}:DescribeAgreement");
    // The caller starts out allowed exactly the action the read is performed under.
    let session = emulator
        .assumed_role(role, Some(&allow(json!(own))))
        .await;

    // (case, the agreementId the identical call names, the code the store answers with once
    // the caller is allowed, the message with it)
    let rows: Vec<(&str, String, &str, &str)> = vec![
        ("an agreement the store holds", agreement.clone(), "", ""),
        (
            "an id nothing answers to",
            "no-such-agreement".to_string(),
            NOT_FOUND.0,
            NOT_FOUND.1,
        ),
        (
            "an id of another well-formed shape",
            "agmt-0000000000009999".to_string(),
            NOT_FOUND.0,
            NOT_FOUND.1,
        ),
    ];

    for (case, named, code, message) in &rows {
        let (status, answered) = emulator
            .call(
                &session.access_key_id,
                "DescribeAgreement",
                &json!({"agreementId": named}),
            )
            .await;
        assert_eq!(
            code_of(&answered),
            *code,
            "case {case:?} (allowed): expected {code:?}, got {status} with {answered}"
        );
        if !message.is_empty() {
            assert_eq!(
                message_of(&answered),
                *message,
                "case {case:?} (allowed): expected the not-found refusal, got {status} with \
                 {answered}"
            );
        }
    }

    // A `Deny` of that action is put on the role beside the `Allow`: every identical call is
    // now refused with the same body, whichever id it names.
    emulator
        .put_role_policy(
            role,
            &json!({
                "Version": "2012-10-17",
                "Statement": [
                    {"Effect": "Allow", "Action": own, "Resource": "*"},
                    {"Effect": "Deny", "Action": own, "Resource": "*"},
                ],
            }),
        )
        .await;
    let refusal = format!(
        "User: {} is not authorized to perform: {own} on resource: {ANY_RESOURCE} {}",
        session.arn,
        Verdict::ExplicitDeny.reason(&evaluated_actions("DescribeAgreement"))
    );
    for (case, named, _, _) in &rows {
        let (status, answered) = emulator
            .call(
                &session.access_key_id,
                "DescribeAgreement",
                &json!({"agreementId": named}),
            )
            .await;
        assert_eq!(
            (status, code_of(&answered)),
            ACCESS_DENIED,
            "case {case:?} (denied): expected the modelled refusal, got {status} with \
             {answered}"
        );
        assert_eq!(
            message_of(&answered),
            refusal,
            "case {case:?} (denied): expected IAM's wording, got {answered}"
        );
    }

    // The deny is taken off again, leaving the coarse view action as the only grant: the
    // identical calls answer from the store once more, which is also what shows the refusals
    // changed nothing.
    emulator
        .put_role_policy(role, &allow(json!(coarse_action())))
        .await;
    for (case, named, code, _) in &rows {
        let (status, answered) = emulator
            .call(
                &session.access_key_id,
                "DescribeAgreement",
                &json!({"agreementId": named}),
            )
            .await;
        assert_eq!(
            code_of(&answered),
            *code,
            "case {case:?} (allowed again): expected {code:?}, got {status} with {answered}"
        );
    }

    // And the agreement the coarse grant reads back is the one the fixture accepted.
    let (status, described) = emulator
        .call(
            &session.access_key_id,
            "DescribeAgreement",
            &json!({"agreementId": agreement}),
        )
        .await;
    assert_eq!(
        (status, described["agreementId"].as_str().unwrap_or_default()),
        (200, agreement.as_str()),
        "case \"the coarse grant reads the agreement back\": expected 200 with the fixture \
         agreement, got {status} with {described}"
    );
}
