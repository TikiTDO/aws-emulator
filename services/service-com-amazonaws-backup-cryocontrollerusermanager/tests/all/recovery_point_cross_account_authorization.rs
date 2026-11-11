// SPDX-License-Identifier: CC0-1.0

//! A call that addresses a RECOVERY POINT by ARN is authorized on that ARN
//! before the point is resolved.
//!
//! The rule under test: when the account field of the `RecoveryPointArn` a
//! request carries is not the account serving the request, the call is refused
//! HTTP 403 with the error type `AccessDeniedException` and this service's own
//! insufficient-privileges sentence, and the refusal discloses nothing about
//! whether that point exists — not even the ARN or the action it was asked for. A
//! recovery point belongs to the account its ARN names, and only that account's
//! own policies could grant a caller anything on it, so there is nothing here to
//! resolve it against and nothing to report as absent.
//!
//! The table spans every implemented operation that addresses a recovery point
//! by the ARN the request supplies (the delete and the read, which bind it into
//! the URI, and the two jobs that take one in the body) crossed with several
//! foreign-account ARNs: different account ids, different regions, different
//! point ids, and an ARN with no account field at all. Each row carries its own
//! controls: the same call on the account's OWN stored point still answers, an
//! own-account ARN that names nothing is still the operation's not-found
//! refusal, and a refused delete leaves the stored point where it was.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `backup` SDK client sees. The request harness is the shared
//! one in `vault_and_backup_job.rs` (`call_reported`, which also reports the
//! `x-amzn-errortype` header a client dispatches on).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::recovery_point_deletion::{listed_points, start_point};
use crate::vault_and_backup_job::{
    call_reported, create_vault, error_code, string_at, ACCOUNT, PAST_SETTLE, REGION,
};

/// The vault whose stored point the controls use.
const VAULT: &str = "authz-source";

/// The vault a copy would be written into, so the copy rows are refused for the
/// SOURCE point rather than for a destination that does not exist.
const DESTINATION_VAULT: &str = "authz-destination";

/// The principal every request to this emulator is authenticated as: the one IAM
/// user of the single account it serves, the identity `sts:GetCallerIdentity`
/// answers with (`service_common::caller_principal::ambient_user_arn`).
fn caller_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/emulator")
}

/// The sentence AWS Backup refuses a request for lack of privileges with: the
/// service's own fixed text, built from nothing in the request.
const DENIAL: &str = "Insufficient privileges to perform this action.";

/// The role every request that needs one names.
fn role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole")
}

/// One operation that addresses a recovery point by the ARN the request carries:
/// its API name (which is the action the decision is taken on) and how a request
/// naming `arn` reaches it.
///
/// From the model (`smithy_query(query="operation",
/// operation="DeleteRecoveryPoint,DescribeRecoveryPoint,StartCopyJob,StartRestoreJob")`):
/// the delete and the read bind `RecoveryPointArn` into
/// `/backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}`, while
/// `StartCopyJob` (`PUT /copy-jobs`) and `StartRestoreJob` (`PUT /restore-jobs`)
/// carry it in the body. Every other operation that takes one is still a 501
/// stub.
struct Addressed {
    operation: &'static str,
    method: &'static str,
    path: fn(&str) -> String,
    body: Option<fn(&str) -> String>,
}

/// The four of them, each addressing the recovery point `arn`.
fn addressed_operations() -> Vec<Addressed> {
    vec![
        Addressed {
            operation: "DeleteRecoveryPoint",
            method: "DELETE",
            path: |arn| format!("/backup-vaults/{VAULT}/recovery-points/{arn}"),
            body: None,
        },
        Addressed {
            operation: "DescribeRecoveryPoint",
            method: "GET",
            path: |arn| format!("/backup-vaults/{VAULT}/recovery-points/{arn}"),
            body: None,
        },
        Addressed {
            operation: "StartCopyJob",
            method: "PUT",
            path: |_| "/copy-jobs".to_string(),
            body: Some(|arn| {
                json!({
                    "RecoveryPointArn": arn,
                    "SourceBackupVaultName": VAULT,
                    "DestinationBackupVaultArn":
                        format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:{DESTINATION_VAULT}"),
                    "IamRoleArn": role(),
                })
                .to_string()
            }),
        },
        Addressed {
            operation: "StartRestoreJob",
            method: "PUT",
            path: |_| "/restore-jobs".to_string(),
            body: Some(|arn| {
                json!({
                    "RecoveryPointArn": arn,
                    "Metadata": {"targetTableName": "restored"},
                    "ResourceType": "DynamoDB",
                    "IamRoleArn": role(),
                })
                .to_string()
            }),
        },
    ]
}

/// Address `arn` through `operation` and report the status, the
/// `x-amzn-errortype` header and the parsed body.
async fn address(
    router: &MultiServiceRouter,
    operation: &Addressed,
    arn: &str,
) -> (u16, String, Value) {
    let path = (operation.path)(arn);
    let body = operation.body.map(|render| render(arn));
    call_reported(router, operation.method, &path, body.as_deref()).await
}

/// The vaults and the one settled recovery point every case runs against.
async fn stored_point(router: &MultiServiceRouter) -> String {
    for vault in [VAULT, DESTINATION_VAULT] {
        let (status, created) = create_vault(router, vault, "{}").await;
        assert_eq!(
            status, 200,
            "setup {vault:?}: CreateBackupVault should succeed, got {status} {created:?}"
        );
    }
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let stored = start_point(router, VAULT, &resource, None).await;
    std::thread::sleep(PAST_SETTLE);
    stored
}

/// A recovery-point ARN of an account other than the one serving the request is
/// refused by the authorization decision — 403 `AccessDeniedException` with
/// AWS's denial sentence — on every operation that addresses a point by ARN.
#[tokio::test]
async fn a_recovery_point_arn_of_another_account_is_denied() {
    // The rows differ along what the rule generalises over: the account id, the
    // Region the ARN names, the point id, and the account field's presence. None
    // of them is the account this emulator serves, so none can be resolved here.
    let foreign: Vec<(&str, String)> = vec![
        (
            "another account, this Region",
            format!("arn:aws:backup:{REGION}:111122223333:recovery-point:1EB3B5E7"),
        ),
        (
            "a different other account",
            format!("arn:aws:backup:{REGION}:210987654321:recovery-point:9fd3b1c4"),
        ),
        (
            "another account in another Region",
            "arn:aws:backup:eu-central-1:999988887777:recovery-point:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
        ),
        (
            "an account id one digit off this account's",
            format!("arn:aws:backup:{REGION}:000000000001:recovery-point:1EB3B5E7"),
        ),
        (
            "no account field at all",
            format!("arn:aws:backup:{REGION}::recovery-point:1EB3B5E7"),
        ),
    ];

    for operation in addressed_operations() {
        let router = crate::vault_and_backup_job::router();
        let stored = stored_point(&router).await;

        for (case, arn) in &foreign {
            let (status, error_type, refused) = address(&router, &operation, arn).await;
            assert_eq!(
                (
                    status,
                    error_type.as_str(),
                    error_code(&refused).as_str(),
                    string_at(&refused, "Message").as_str()
                ),
                (403, "AccessDeniedException", "AccessDeniedException", DENIAL),
                "case {case:?} on {}: expected the authorization refusal, got {status} \
                 {error_type} {refused:?}",
                operation.operation
            );
            let rendered = refused.to_string();
            assert!(
                !rendered.contains("NotFound") && !rendered.contains("Cannot find"),
                "case {case:?} on {}: the refusal must disclose nothing about existence, \
                 got {rendered}",
                operation.operation
            );
            // The refusal is taken before anything is resolved, so it discloses
            // neither what was asked for nor who asked.
            for disclosed in [arn.as_str(), operation.operation, &caller_principal()] {
                assert!(
                    !rendered.contains(disclosed),
                    "case {case:?} on {}: the refusal must not disclose {disclosed:?}, \
                     got {rendered}",
                    operation.operation
                );
            }
        }

        // The control: the account's OWN stored point is still addressable
        // through the same call, so the refusals above are about the ARN's
        // account and not about the operation.
        let (status, _, answered) = address(&router, &operation, &stored).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (200, "<none>"),
            "case {}: the account's own stored point must still be addressable, got {status} \
             {answered:?}",
            operation.operation
        );
    }
}

/// The decision turns on the ARN's ACCOUNT, not on whether anything is stored:
/// an own-account ARN that names no stored point is still the operation's
/// not-found refusal, with its own status and code.
#[tokio::test]
async fn an_own_account_arn_that_names_nothing_is_still_not_found() {
    // What each operation answers for a `RecoveryPointArn` of this account that
    // resolves to no stored point: the delete and the read report the resource
    // absent, while the two jobs take the point as an INPUT and refuse its value
    // (`smithy_query(query="operation", operation="StartRestoreJob")` — the
    // modelled errors each one declares).
    let expected: Vec<(&str, u16, &str)> = vec![
        ("DeleteRecoveryPoint", 400, "ResourceNotFoundException"),
        ("DescribeRecoveryPoint", 400, "ResourceNotFoundException"),
        ("StartCopyJob", 400, "ResourceNotFoundException"),
        ("StartRestoreJob", 400, "InvalidParameterValueException"),
    ];

    let absent = format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:does-not-exist");
    for operation in addressed_operations() {
        let router = crate::vault_and_backup_job::router();
        let _stored = stored_point(&router).await;
        let (status, error_type, refused) = address(&router, &operation, &absent).await;
        let (_, want_status, want_code) = expected
            .iter()
            .find(|(name, _, _)| *name == operation.operation)
            .copied()
            .expect("every addressed operation has a not-found answer");
        assert_eq!(
            (status, error_type.as_str(), error_code(&refused).as_str()),
            (want_status, want_code, want_code),
            "case {}: an own-account ARN naming nothing must be the not-found refusal, got \
             {status} {error_type} {refused:?}",
            operation.operation
        );
    }
}

/// A delete the authorization decision refused deletes nothing: the point the
/// account holds is still listed afterwards, and the own-account delete that
/// follows still removes it.
#[tokio::test]
async fn a_denied_delete_leaves_the_stored_point_in_place() {
    let router = crate::vault_and_backup_job::router();
    let stored = stored_point(&router).await;

    let foreign = format!("arn:aws:backup:{REGION}:111122223333:recovery-point:1EB3B5E7");
    let delete = Addressed {
        operation: "DeleteRecoveryPoint",
        method: "DELETE",
        path: |arn| format!("/backup-vaults/{VAULT}/recovery-points/{arn}"),
        body: None,
    };

    let (status, _, refused) = address(&router, &delete, &foreign).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (403, "AccessDeniedException"),
        "case a foreign-account delete: expected the authorization refusal, got {status} \
         {refused:?}"
    );
    let arns: Vec<String> = listed_points(&router, VAULT)
        .await
        .iter()
        .map(|point| string_at(point, "RecoveryPointArn"))
        .collect();
    assert_eq!(
        arns,
        vec![stored.clone()],
        "case a foreign-account delete: the stored point must survive it, got {arns:?}"
    );

    let (status, _, answered) = address(&router, &delete, &stored).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (200, "<none>"),
        "case the account's own delete: expected it to remove the point, got {status} \
         {answered:?}"
    );
    let arns: Vec<String> = listed_points(&router, VAULT)
        .await
        .iter()
        .map(|point| string_at(point, "RecoveryPointArn"))
        .collect();
    assert!(
        arns.is_empty(),
        "case the account's own delete: the vault must report the point gone, got {arns:?}"
    );
}
