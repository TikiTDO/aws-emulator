// SPDX-License-Identifier: CC0-1.0

//! The BODY of the authorization refusal a call that addresses a RECOVERY POINT
//! by ARN answers.
//!
//! The rule under test: when such a call is refused because the recovery point
//! its ARN names belongs to another account, the refusal's `Message` is AWS's
//! authorization sentence, whose fixed text is "User: ", " is not authorized to
//! perform: ", " on resource: ", " because no resource-based policy allows the "
//! and " action", filled in with THREE values the request itself decides: the
//! principal the request was signed by, the action asked for (this service's
//! action namespace plus the API name of the operation) and the `RecoveryPointArn`
//! exactly as the request carried it. So the sentence is not one fixed string: it
//! changes with the operation and with the ARN, and a client that parses it learns
//! which principal lacked which permission on which resource.
//!
//! The table spans every implemented operation that addresses a recovery point by
//! the ARN the request supplies — so the action name varies — crossed with ARNs
//! that differ in each part the sentence quotes back: the account id, the Region,
//! the partition, the spelling of the point id, and an ARN with no account field
//! at all. The controls are the other half of the rule: a refusal this decision
//! did not take (an own-account ARN that names no stored point) is NOT this
//! sentence, and the account's own stored point is still answered.
//!
//! Every case drives real HTTP through the router, so what is asserted is the wire
//! behaviour a `backup` SDK client sees. The request harness, the operation table
//! and the sentence itself are the shared ones in
//! `recovery_point_cross_account_authorization.rs`, so this suite drives the same
//! wire path rather than growing a second copy of it.

use std::collections::BTreeSet;

use crate::recovery_point_cross_account_authorization::{
    address, addressed_operations, caller_principal, denial, stored_point, ACTION_NAMESPACE,
};
use crate::vault_and_backup_job::{error_code, string_at, ACCOUNT, REGION};

/// The FIXED text of AWS's authorization sentence, in order, as the recorded
/// refusal of a cross-account recovery-point call words it. Everything between
/// these segments is a value the request decided.
const FIXED: [&str; 5] = [
    "User: ",
    " is not authorized to perform: ",
    " on resource: ",
    " because no resource-based policy allows the ",
    " action",
];

/// The three variable parts of `message` — the principal, the action and the
/// resource — or `None` when `message` is not that sentence at all.
///
/// The sentence names ONE action, in both of the places it appears, so a message
/// whose two action slots disagree is not the template either.
fn filled(message: &str) -> Option<(String, String, String)> {
    let rest = message.strip_prefix(FIXED[0])?;
    let (principal, rest) = rest.split_once(FIXED[1])?;
    let (action, rest) = rest.split_once(FIXED[2])?;
    let (resource, rest) = rest.split_once(FIXED[3])?;
    let repeated = rest.strip_suffix(FIXED[4])?;
    if repeated != action {
        return None;
    }
    Some((
        principal.to_string(),
        action.to_string(),
        resource.to_string(),
    ))
}

/// Recovery-point ARNs of accounts other than the one serving the request, each
/// differing in a part the sentence quotes back.
///
/// The id spellings are the ones the model's own `RecoveryPointArn` examples use
/// (`smithy_query(query="shape", shape="DescribeRecoveryPointInput")`:
/// "arn:aws:backup:us-east-1:123456789012:recovery-point:1EB3B5E7-9EB0-435A-A80B-108B488B0D45"),
/// plus the lowercase uuid form a started backup mints.
fn foreign_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "another account, a lowercase uuid",
            format!("arn:aws:backup:{REGION}:111122223333:recovery-point:fb698c18-a985-4704-b4fb-0c14fa8cfaa3"),
        ),
        (
            "another account, the model's uppercase id",
            "arn:aws:backup:us-east-1:123456789012:recovery-point:1EB3B5E7-9EB0-435A-A80B-108B488B0D45"
                .to_string(),
        ),
        (
            "another Region and another account",
            "arn:aws:backup:eu-central-1:999988887777:recovery-point:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
                .to_string(),
        ),
        (
            "another partition",
            "arn:aws-us-gov:backup:us-gov-west-1:210987654321:recovery-point:7c1b0f42"
                .to_string(),
        ),
        (
            "an account id one digit off this account's",
            format!("arn:aws:backup:{REGION}:000000000001:recovery-point:1EB3B5E7"),
        ),
        (
            "no account field at all",
            format!("arn:aws:backup:{REGION}::recovery-point:1EB3B5E7"),
        ),
    ]
}

/// The refusal's `Message` is AWS's sentence with the request's own principal,
/// action and resource ARN in it — for every operation that addresses a recovery
/// point by ARN and every foreign-account ARN form.
#[tokio::test]
async fn the_refusal_states_the_principal_the_action_and_the_arn_the_request_carried() {
    for operation in addressed_operations() {
        let router = crate::vault_and_backup_job::router();
        let _stored = stored_point(&router).await;
        let action = format!("{ACTION_NAMESPACE}:{}", operation.operation);

        for (case, arn) in foreign_arns() {
            let (status, error_type, refused) = address(&router, &operation, &arn).await;
            assert_eq!(
                (status, error_type.as_str(), error_code(&refused).as_str()),
                (403, "AccessDeniedException", "AccessDeniedException"),
                "case {case:?} on {}: expected the authorization refusal, got {status} \
                 {error_type} {refused:?}",
                operation.operation
            );

            let message = string_at(&refused, "Message");
            assert_eq!(
                filled(&message),
                Some((caller_principal(), action.clone(), arn.clone())),
                "case {case:?} on {}: expected AWS's authorization sentence over the \
                 caller, {action:?} and the ARN the request carried, got {message:?}",
                operation.operation
            );
            assert_eq!(
                message,
                denial(&action, &arn),
                "case {case:?} on {}: expected the sentence rendered whole, got {message:?}",
                operation.operation
            );
        }
    }
}

/// The sentence is built from the request, so it is a DIFFERENT sentence for a
/// different resource and for a different action: no two of the table's cases
/// share a message.
#[tokio::test]
async fn the_sentence_varies_with_the_resource_and_with_the_action() {
    let mut messages: BTreeSet<String> = BTreeSet::new();
    let mut cases = 0usize;

    for operation in addressed_operations() {
        let router = crate::vault_and_backup_job::router();
        let _stored = stored_point(&router).await;

        for (case, arn) in foreign_arns() {
            let (status, _, refused) = address(&router, &operation, &arn).await;
            let message = string_at(&refused, "Message");
            assert_eq!(
                status, 403,
                "case {case:?} on {}: expected the authorization refusal, got {status} \
                 {refused:?}",
                operation.operation
            );
            assert!(
                messages.insert(message.clone()),
                "case {case:?} on {}: the sentence must name this request's own action and \
                 resource, so no earlier case may have produced {message:?}",
                operation.operation
            );
            cases += 1;
        }
    }

    assert_eq!(
        messages.len(),
        cases,
        "case the whole table: expected one distinct sentence per case, got \
         {} for {cases} cases",
        messages.len()
    );
}

/// The other half of the rule: a refusal this decision did NOT take is not this
/// sentence. An own-account ARN that names no stored point is still each
/// operation's own 400 refusal, and the account's own stored point is answered.
#[tokio::test]
async fn an_own_account_refusal_is_not_the_authorization_sentence() {
    for operation in addressed_operations() {
        let router = crate::vault_and_backup_job::router();
        let stored = stored_point(&router).await;

        let absent = format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:nothing-here");
        let (status, _, refused) = address(&router, &operation, &absent).await;
        let message = string_at(&refused, "Message");
        assert_eq!(
            (status, filled(&message)),
            (400, None),
            "case an own-account ARN naming nothing on {}: expected a 400 refusal that is \
             not the authorization sentence, got {status} {refused:?}",
            operation.operation
        );

        let (status, _, answered) = address(&router, &operation, &stored).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (200, "<none>"),
            "case the account's own stored point on {}: expected it to be answered, got \
             {status} {answered:?}",
            operation.operation
        );
    }
}
