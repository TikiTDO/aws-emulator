// SPDX-License-Identifier: CC0-1.0

//! What a started backup job COMES TO: whether the role it runs under may perform the
//! backup it was started for, which is what decides the job's terminal state.
//!
//! # The rule this module implements
//!
//! `StartBackupJob` takes the `IamRoleArn` "used to create the target recovery point", and
//! the backup is performed UNDER that role rather than under the caller's own identity. So
//! the call is accepted on what it can be judged on synchronously — the vault, the
//! parameters — and the job's TERMINAL state is decided by what that role is permitted to
//! do: a role whose permissions do not allow the action that creates the recovery point of
//! the named resource cannot produce one, so the job is stored with no recovery point at
//! all, ends in the terminal state `FAILED` carrying the denial as its `StatusMessage`, and
//! a role that is allowed it ends `COMPLETED` with the point it wrote.
//!
//! The question is answered BEFORE the job is stored, so the stored job already carries the
//! outcome it is owed: there is no instant at which the clock could settle a job whose
//! outcome is still being decided.
//!
//! The question is put to IAM ITSELF, over the emulator's own in-process callback, through
//! the tree's one reading of what a principal's policies decide
//! ([`::service_common::iam_role::role_withholds_action`]): what settles it is the documents
//! a client's `PutRolePolicy` or `AttachRolePolicy` left on the role, and the sentence the
//! denial is worded with is the tree's one wording of an identity-based denial
//! ([`::service_common::iam_authorization`]), so two services that reach this decision state
//! it the same way.
//!
//! Nothing is decided on an answer this emulator could not get: an emulator with no IAM
//! mounted beside this service, a value naming no role of this account, and a role IAM holds
//! nothing under all leave the job's outcome alone, for the reason the shared reading gives —
//! a backup role is routinely provisioned out of band, so its absence from this store is not
//! evidence about its permissions. A resource type this emulator knows no creating action for
//! is the same kind of silence ([`backup_action_for`]).

/// The action AWS Backup performs UNDER a job's role to create the recovery point of a
/// protected resource, per Backup resource type ([`super::resource_type_of`]).
///
/// SOURCE: the per-resource-type grants AWS publishes for the backup role (AWS Backup
/// *Managed policies*, `AWSBackupServiceRolePolicyForBackup`) — of each type's grants, the
/// one action that CREATES the recovery point, which is the call a role with no permissions
/// is denied. The snapshot of a cluster the `rds` API serves is created by the same action
/// whichever engine fronts it, so Aurora, Amazon DocumentDB and Amazon Neptune share it.
///
/// A type this table does not name is one this emulator knows no creating action for, so it
/// grounds no decision — see the module docs.
const BACKUP_ROLE_ACTIONS: &[(&str, &str)] = &[
    ("Aurora", "rds:CreateDBClusterSnapshot"),
    ("DocumentDB", "rds:CreateDBClusterSnapshot"),
    ("DynamoDB", "dynamodb:CreateBackup"),
    ("EBS", "ec2:CreateSnapshot"),
    ("EC2", "ec2:CreateImage"),
    ("EFS", "elasticfilesystem:Backup"),
    ("FSx", "fsx:CreateBackup"),
    ("Neptune", "rds:CreateDBClusterSnapshot"),
    ("RDS", "rds:CreateDBSnapshot"),
    ("Redshift", "redshift:CreateClusterSnapshot"),
    ("Timestream", "timestream:StartAwsBackupJob"),
];

/// The action a backup of a resource of `resource_type` performs under the job's role, or
/// `None` for a type [`BACKUP_ROLE_ACTIONS`] names none for — including a resource whose ARN
/// resolved to no Backup resource type at all.
#[must_use]
pub fn backup_action_for(resource_type: Option<&str>) -> Option<&'static str> {
    let resource_type = resource_type?;
    BACKUP_ROLE_ACTIONS
        .iter()
        .find(|(named, _)| *named == resource_type)
        .map(|(_, action)| *action)
}

/// Why the backup of `resource_arn` under `role_arn` can create no recovery point — the
/// `StatusMessage` the job's terminal `FAILED` state carries — or `None` when nothing shows
/// it cannot.
///
/// The action asked about is selected by the Backup resource type of the protected resource,
/// derived from its ARN the way every other report of that type is
/// ([`super::resource_type_of`]), so one resource is judged on one action however many
/// callers ask.
///
/// Asked BEFORE the job is stored, with NO store guard held: it makes one in-process IAM
/// call and opens no socket.
pub async fn withheld_backup_denial(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    resource_arn: &str,
) -> Option<String> {
    let resource_type = super::resource_type_of(resource_arn);
    let action = backup_action_for(resource_type.as_deref())?;
    if !::service_common::iam_role::role_withholds_action(
        ctx, region, role_arn, action, resource_arn,
    )
    .await
    {
        return None;
    }
    Some(
        ::service_common::iam_authorization::not_authorized_by_identity_policy(
            role_arn,
            action,
            resource_arn,
            &::service_common::iam_authorization::no_identity_policy_allows(action),
        ),
    )
}
