// SPDX-License-Identifier: CC0-1.0

//! The BODY of the authorization refusal a call that addresses a RECOVERY POINT
//! by ARN answers.
//!
//! The rule under test: when such a call is refused because the recovery point
//! its ARN names belongs to another account, the refusal's `Message` is AWS
//! Backup's own fixed sentence — the SAME one every refusal this service does not
//! grant carries (`vault_read_visibility::DENIED_MESSAGE`) — built from nothing in
//! the request, so it names neither the principal that signed, nor the action
//! asked for, nor the `RecoveryPointArn` the request carried, and discloses
//! nothing about what the other account holds.
//!
//! The table spans every implemented operation that addresses a recovery point by
//! the ARN the request supplies — so the action varies — crossed with ARNs that
//! differ in every part a refusal could have quoted back: the account id, the
//! Region, the partition, the spelling of the point id, and an ARN with no account
//! field at all. The controls are the other half of the rule: a refusal this
//! decision did not take (an own-account ARN that names no stored point) is NOT
//! this sentence, and the account's own stored point is still answered.
//!
//! Every case drives real HTTP through the router, so what is asserted is the wire
//! behaviour a `backup` SDK client sees. The request harness and the operation
//! table are the shared ones in `recovery_point_cross_account_authorization.rs`,
//! so this suite drives the same wire path rather than growing a second copy of it.

use std::collections::BTreeSet;

use crate::recovery_point_cross_account_authorization::{
    address, addressed_operations, caller_principal, denial, stored_point, ACTION_NAMESPACE,
};
use crate::vault_and_backup_job::{error_code, string_at, ACCOUNT, REGION};

/// Recovery-point ARNs of accounts other than the one serving the request, each
/// differing in a part a refusal could have quoted back.
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

/// The refusal's `Message` is the service's own sentence, for every operation that
/// addresses a recovery point by ARN and every foreign-account ARN form.
#[tokio::test]
async fn the_refusal_states_the_services_own_sentence() {
    for operation in addressed_operations() {
        let router = crate::vault_and_backup_job::router();
        let _stored = stored_point(&router).await;

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
                message,
                denial(),
                "case {case:?} on {}: expected this service's refusal sentence, got \
                 {message:?}",
                operation.operation
            );
        }
    }
}

/// The sentence is built from NOTHING in the request: it quotes back neither the
/// signing principal, nor the action, nor the ARN — so every case of the table
/// answers the one same message.
#[tokio::test]
async fn the_sentence_quotes_nothing_the_request_carried() {
    let mut messages: BTreeSet<String> = BTreeSet::new();

    for operation in addressed_operations() {
        let router = crate::vault_and_backup_job::router();
        let _stored = stored_point(&router).await;
        let action = format!("{ACTION_NAMESPACE}:{}", operation.operation);

        for (case, arn) in foreign_arns() {
            let (status, _, refused) = address(&router, &operation, &arn).await;
            let message = string_at(&refused, "Message");
            assert_eq!(
                status, 403,
                "case {case:?} on {}: expected the authorization refusal, got {status} \
                 {refused:?}",
                operation.operation
            );
            for quoted in [caller_principal(), action.clone(), arn.clone()] {
                assert!(
                    !message.contains(&quoted),
                    "case {case:?} on {}: the refusal must not quote {quoted:?} back, got \
                     {message:?}",
                    operation.operation
                );
            }
            messages.insert(message);
        }
    }

    let distinct: Vec<&String> = messages.iter().collect();
    assert_eq!(
        distinct,
        vec![&denial()],
        "case the whole table: expected the one fixed sentence for every case, got \
         {distinct:?}"
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
            (status, message == denial()),
            (400, false),
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
