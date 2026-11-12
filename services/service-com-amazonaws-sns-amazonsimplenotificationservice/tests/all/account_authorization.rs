// SPDX-License-Identifier: CC0-1.0

//! An SNS call that addresses no resource of its own is served only where the CALLER's own
//! identity-based policies allow it, end to end over the wire through the real
//! `MultiServiceRouter` with IAM and STS mounted on the same engine — so the refusal is
//! observed where a client observes it: in the answer to the signed call.
//!
//! The RULE asserted here: such a call is authorized against the ARN that names every SNS
//! resource of the account and Region the request reached
//! (`arn:<partition>:sns:<region>:<account>:*`), because the actions these operations perform
//! support no resource-level permissions, and no resource-based policy is in force on a call
//! that names no resource. IAM's order decides it: an explicit `Deny` refuses whatever `Allow`
//! sits beside it, otherwise an `Allow` covering the action AND that whole resource grants it,
//! and with neither the call falls to IAM's default deny. A refused call is answered with the
//! modelled `AuthorizationErrorException` — HTTP 403, `awsQueryError` code
//! `AuthorizationError` (`smithy_query(query="shape",shape="AuthorizationErrorException")`) —
//! carrying IAM's refusal sentence, and reports nothing about what the account holds.
//!
//! The rows differ along what the rule generalises over: which OPERATION is called (so the
//! action is read per operation rather than for one of them), and what the caller's documents
//! say — nothing at all, a grant naming each action, a grant for every action of the service,
//! an explicit deny beside a grant, a grant for another action, and a grant scoped to one
//! topic's ARN rather than to the account's whole SNS resource. The recorded case — an
//! explicit `Deny` beside an `Allow` for `sns:GetSMSSandboxAccountStatus` — is one row of
//! one operation.

use serde_json::{json, Value};

use super::publish_authorization::{
    emulator, given_user, iam, policy, sns_as, Emulator, AMBIENT_KEY, REGION,
};
use super::topics_subscriptions_publish::text_of;

/// The operations whose request addresses no resource of its own, each with the parameters it
/// requires. Source: the operations of
/// `smithy_query(query="digest",model_path="services/service-com-amazonaws-sns-amazonsimple\
/// notificationservice/smithy-model/com-amazonaws-sns-amazonsimplenotificationservice.json")`
/// whose input carries no ARN member and that create no resource — the account's own
/// listings, its SMS settings and its SMS-sandbox membership. Each is served with HTTP 200
/// once the caller is authorized, so the grant and the refusal are told apart on the status
/// alone.
const ACCOUNT_WIDE_OPERATIONS: &[&str] = &[
    "GetSMSSandboxAccountStatus",
    "GetSMSAttributes",
    "ListTopics",
    "ListSubscriptions",
    "ListPlatformApplications",
    "ListSMSSandboxPhoneNumbers",
    "ListOriginationNumbers",
    "ListPhoneNumbersOptedOut",
];

/// The IAM action an SNS operation performs: `SNS:<OperationName>` ("Actions defined by
/// Amazon Simple Notification Service", IAM Service Authorization Reference).
fn action_of(operation: &str) -> String {
    format!("SNS:{operation}")
}

/// The resource such a call is judged on, and which its refusal names: every SNS resource of
/// this account in the Region the request addressed.
fn account_wide_resource() -> String {
    format!("arn:aws:sns:{REGION}:{}:*", engine::emulator_account_id())
}

/// A topic of this account — a resource a grant can be scoped to, which is NOT the resource an
/// account-wide call is judged on.
fn one_topic() -> String {
    format!(
        "arn:aws:sns:{REGION}:{}:one-topic",
        engine::emulator_account_id()
    )
}

/// One document around several statements.
fn document(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// The clause a refusal states when one of the principal's own documents explicitly denies the
/// action.
const IDENTITY_DENY: &str = "with an explicit deny in an identity-based policy";

/// The clause a refusal states when no readable identity-based document allows the action —
/// IAM's default deny, which names the action nothing granted.
fn no_identity_grant(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// What the policies in force must decide for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The call is served.
    Served,
    /// The call is refused, with this clause closing IAM's sentence. `None` is the default-deny
    /// clause, which names the action and so is built per operation.
    Refused(Option<&'static str>),
}

/// The document a row installs on the caller, built over the actions of the operations it is
/// asserted for.
type DocumentOf = fn(&[String]) -> Option<String>;

#[tokio::test]
async fn an_account_wide_call_is_served_only_where_the_callers_policies_allow_it() {
    let emulator = emulator();
    let actions: Vec<String> = ACCOUNT_WIDE_OPERATIONS
        .iter()
        .map(|operation| action_of(operation))
        .collect();
    let resource = account_wide_resource();

    // (case, the caller's inline identity policy, the verdict)
    let rows: &[(&str, DocumentOf, Verdict)] = &[
        ("no identity-based document at all", |_| None, Verdict::Refused(None)),
        (
            "a grant naming each of these actions on every resource",
            |actions| {
                Some(document(vec![json!({
                    "Effect": "Allow",
                    "Action": actions,
                    "Resource": "*",
                })]))
            },
            Verdict::Served,
        ),
        (
            "a grant for every action of the service",
            |_| {
                Some(document(vec![json!({
                    "Effect": "Allow",
                    "Action": "sns:*",
                    "Resource": "*",
                })]))
            },
            Verdict::Served,
        ),
        (
            "an explicit deny beside that grant",
            |actions| {
                Some(document(vec![
                    json!({"Effect": "Allow", "Action": "sns:*", "Resource": "*"}),
                    json!({"Effect": "Deny", "Action": actions, "Resource": "*"}),
                ]))
            },
            Verdict::Refused(Some(IDENTITY_DENY)),
        ),
        (
            "a grant for another action of the service only",
            |_| {
                Some(document(vec![json!({
                    "Effect": "Allow",
                    "Action": "sns:Publish",
                    "Resource": "*",
                })]))
            },
            Verdict::Refused(None),
        ),
        (
            "a grant scoped to one topic's ARN rather than the account's SNS resource",
            |actions| {
                Some(document(vec![json!({
                    "Effect": "Allow",
                    "Action": actions,
                    "Resource": one_topic(),
                })]))
            },
            Verdict::Refused(None),
        ),
    ];

    for (row, (case, document_of, verdict)) in rows.iter().enumerate() {
        let name = format!("account-caller-{row}");
        let (user, access_key_id) = given_user(&emulator, &name).await;
        if let Some(text) = document_of(&actions) {
            iam(
                &emulator,
                "PutUserPolicy",
                &[
                    ("UserName", &name),
                    ("PolicyName", "account-wide"),
                    ("PolicyDocument", &text),
                ],
            )
            .await;
        }
        for operation in ACCOUNT_WIDE_OPERATIONS {
            let action = action_of(operation);
            let (status, body) = sns_as(&emulator, &access_key_id, operation, &[]).await;
            match verdict {
                Verdict::Served => assert_eq!(
                    status, 200,
                    "case {case:?} calling {operation}: expected HTTP 200, got {status} / {body}"
                ),
                Verdict::Refused(reason) => {
                    assert_eq!(
                        status, 403,
                        "case {case:?} calling {operation}: expected HTTP 403, got {status} / \
                         {body}"
                    );
                    let code = text_of(&body, "Code");
                    assert_eq!(
                        code, "AuthorizationError",
                        "case {case:?} calling {operation}: expected the AuthorizationError \
                         code, got {code:?} in {body}"
                    );
                    let clause = reason
                        .map_or_else(|| no_identity_grant(&action), ToString::to_string);
                    let message = text_of(&body, "Message");
                    assert_eq!(
                        message,
                        format!(
                            "User: {user} is not authorized to perform: {action} on resource: \
                             {resource} {clause}"
                        ),
                        "case {case:?} calling {operation}: expected IAM's refusal wording, got \
                         {message:?}"
                    );
                }
            }
        }
    }
}

/// RULE, on the operations that carry required input: the gate decides the call before the
/// handler reads its own preconditions, so a caller the policies refuse is answered 403 with
/// IAM's sentence whatever it sent, and one they allow is not refused at all — whatever that
/// operation's own preconditions then answer.
///
/// These are the remaining operations of [`ACCOUNT_WIDE_OPERATIONS`]'s class: their request
/// addresses no resource of this service either, but it carries members (a phone number, an
/// attribute map, a one-time password), so the served answer is the operation's own rather
/// than a plain 200 and only the authorization half is asserted here.
#[tokio::test]
async fn an_account_wide_call_carrying_input_is_decided_before_its_own_preconditions() {
    let number = "+12065550100";
    // (operation, the members it requires beside none)
    let rows: &[(&str, &[(&str, &str)])] = &[
        ("CheckIfPhoneNumberIsOptedOut", &[("phoneNumber", number)]),
        ("OptInPhoneNumber", &[("phoneNumber", number)]),
        (
            "SetSMSAttributes",
            &[
                ("attributes.entry.1.key", "DefaultSenderID"),
                ("attributes.entry.1.value", "Sender"),
            ],
        ),
        ("CreateSMSSandboxPhoneNumber", &[("PhoneNumber", number)]),
        ("DeleteSMSSandboxPhoneNumber", &[("PhoneNumber", number)]),
        (
            "VerifySMSSandboxPhoneNumber",
            &[("PhoneNumber", number), ("OneTimePassword", "123456")],
        ),
    ];

    let emulator = emulator();
    let resource = account_wide_resource();
    let (refused_user, refused_key) = given_user(&emulator, "input-refused").await;
    iam(
        &emulator,
        "PutUserPolicy",
        &[
            ("UserName", "input-refused"),
            ("PolicyName", "deny-beside-allow"),
            (
                "PolicyDocument",
                &document(vec![
                    json!({"Effect": "Allow", "Action": "sns:*", "Resource": "*"}),
                    json!({"Effect": "Deny", "Action": "sns:*", "Resource": "*"}),
                ]),
            ),
        ],
    )
    .await;
    let (_, allowed_key) = given_user(&emulator, "input-allowed").await;
    iam(
        &emulator,
        "PutUserPolicy",
        &[
            ("UserName", "input-allowed"),
            ("PolicyName", "allow-everything"),
            (
                "PolicyDocument",
                &policy(json!({"Effect": "Allow", "Action": "sns:*", "Resource": "*"})),
            ),
        ],
    )
    .await;

    for (operation, params) in rows {
        let action = action_of(operation);
        let (status, body) = sns_as(&emulator, &refused_key, operation, params).await;
        assert_eq!(
            status, 403,
            "case {operation:?} refused by an explicit deny: expected HTTP 403, got {status} \
             / {body}"
        );
        let code = text_of(&body, "Code");
        assert_eq!(
            code, "AuthorizationError",
            "case {operation:?} refused by an explicit deny: expected the AuthorizationError \
             code, got {code:?} in {body}"
        );
        let message = text_of(&body, "Message");
        assert_eq!(
            message,
            format!(
                "User: {refused_user} is not authorized to perform: {action} on resource: \
                 {resource} {IDENTITY_DENY}"
            ),
            "case {operation:?} refused by an explicit deny: expected IAM's refusal wording, \
             got {message:?}"
        );

        let (status, body) = sns_as(&emulator, &allowed_key, operation, params).await;
        assert_ne!(
            status, 403,
            "case {operation:?} allowed by a grant for every action: expected no \
             authorization refusal, got {status} / {body}"
        );
    }
}

/// RULE: a grant is read for the ACTION it names, so a document allowing one account-wide
/// operation grants that one and no sibling of it — the decision is per action, not per
/// service.
#[tokio::test]
async fn a_grant_for_one_account_wide_action_grants_that_action_alone() {
    let emulator = emulator();
    let granted = "GetSMSSandboxAccountStatus";
    let (user, access_key_id) = given_user(&emulator, "one-action-caller").await;
    iam(
        &emulator,
        "PutUserPolicy",
        &[
            ("UserName", "one-action-caller"),
            ("PolicyName", "one-action"),
            (
                "PolicyDocument",
                &policy(json!({
                    "Effect": "Allow",
                    // SNS's own spelling of an action and IAM's differ only in case, which
                    // IAM compares case-insensitively.
                    "Action": format!("sns:{granted}"),
                    "Resource": "*",
                })),
            ),
        ],
    )
    .await;

    let (status, body) = sns_as(&emulator, &access_key_id, granted, &[]).await;
    assert_eq!(
        status, 200,
        "case \"the granted action\": expected HTTP 200, got {status} / {body}"
    );

    for operation in ACCOUNT_WIDE_OPERATIONS
        .iter()
        .filter(|operation| **operation != granted)
    {
        let action = action_of(operation);
        let (status, body) = sns_as(&emulator, &access_key_id, operation, &[]).await;
        assert_eq!(
            status, 403,
            "case \"a sibling action\" calling {operation}: expected HTTP 403, got {status} / \
             {body}"
        );
        let message = text_of(&body, "Message");
        assert_eq!(
            message,
            format!(
                "User: {user} is not authorized to perform: {action} on resource: {} {}",
                account_wide_resource(),
                no_identity_grant(&action)
            ),
            "case \"a sibling action\" calling {operation}: expected IAM's refusal wording, got \
             {message:?}"
        );
    }
}

/// RULE: the account's own published credential — the one an emulated AWS service signs its
/// in-process calls into this one with — holds no identity-based policies, so no account-wide
/// call of it is refused.
#[tokio::test]
async fn the_accounts_own_credential_is_not_bounded_by_this_gate() {
    let emulator: Emulator = emulator();
    for operation in ACCOUNT_WIDE_OPERATIONS {
        let (status, body) = sns_as(&emulator, AMBIENT_KEY, operation, &[]).await;
        assert_eq!(
            status, 200,
            "calling {operation} with the account's own credential: expected HTTP 200, got \
             {status} / {body}"
        );
    }
}
