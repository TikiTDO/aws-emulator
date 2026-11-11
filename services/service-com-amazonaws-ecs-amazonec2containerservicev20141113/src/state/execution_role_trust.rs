// SPDX-License-Identifier: CC0-1.0

//! The TRUST POLICY of a role an Amazon ECS task runs under, and the one reading of it this
//! service takes.
//!
//! A task's execution role (`TaskDefinition$executionRoleArn`, "the task execution role that
//! grants the Amazon ECS container agent permission to make Amazon Web Services API calls on
//! your behalf") and its task role (`TaskDefinition$taskRoleArn`, "the IAM role that
//! containers in this task can assume") are both reached by ASSUMING them, and a role is
//! assumable only by the principals its own trust policy admits
//! (`CreateRole$AssumeRolePolicyDocument`: "The trust relationship policy document that grants
//! an entity permission to assume the role"). The principal Amazon ECS assumes either of them
//! as is [`ECS_TASKS_SERVICE_PRINCIPAL`], so ONE question decides both: does the trust policy
//! the role carries let that principal perform
//! [`::service_common::iam_trust_policy::ASSUME_ROLE_ACTION`]?
//!
//! That question is [`role_withholds_assumption`], and the sentence a task stopped by its
//! answer carries is [`unable_to_assume_role_message`]. WHICH roles a launch has Amazon ECS
//! assume, and what a launch it cannot assume one of does, is [`super::launch_role_trust`].
//!
//! The decision is taken against the account's OWN IAM records — the ones `CreateRole` and
//! `UpdateAssumeRolePolicy` wrote, reached through the shared engine — and through the tree's
//! one reading of a trust document
//! ([`::service_common::iam_trust_policy::admits_service_principal`]), so no two services
//! handed the same document answer differently.
//!
//! Only what IAM ANSWERED is refused: a role this account's IAM holds no record for (it may
//! have been provisioned out of band, which is the ordinary way these roles exist), a role
//! carrying no trust document, and a document this cannot read as a policy are each evidence
//! of nothing.

/// The service principal Amazon ECS assumes a task's roles as, which is therefore the
/// principal the trust policy of a task's execution role and task role has to admit (the
/// `ecs-tasks` service principal of the Amazon ECS task roles, Amazon Elastic Container
/// Service Developer Guide).
pub const ECS_TASKS_SERVICE_PRINCIPAL: &str = "ecs-tasks.amazonaws.com";

/// The IAM store a launch's roles are resolved against.
///
/// It is the IAM service's OWN state struct, reached through the SAME shared engine this
/// service's store lives in, so Amazon ECS reads the very records `CreateRole` and
/// `UpdateAssumeRolePolicy` wrote rather than a second copy of them.
pub type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The `stoppedReason` of a task whose role Amazon ECS could not assume: the fixed sentence
/// Amazon ECS words such a stop with, carrying the role the launch resolved.
pub fn unable_to_assume_role_message(role_arn: &str) -> String {
    format!(
        "ECS was unable to assume the configured role '{role_arn}'. Please verify that the \
         ECS service principal is allowed to assume this role."
    )
}

/// Whether the account's IAM ANSWERED that `role_arn` is a role Amazon ECS cannot assume for
/// a task: IAM holds that role, and the trust policy it carries is a policy that does not let
/// [`ECS_TASKS_SERVICE_PRINCIPAL`] assume it.
///
/// `false` for everything that is not such an answer — a role this store holds no record for,
/// a role carrying no trust document, and a document that cannot be read as a policy — for
/// the reason the module docs give.
pub fn role_withholds_assumption(iam: &IamState, role_arn: &str) -> bool {
    let Some(role) = iam.roles.values().find(|role| role.arn == role_arn) else {
        return false;
    };
    role.assume_role_policy_document
        .as_deref()
        .and_then(|document| {
            ::service_common::iam_trust_policy::admits_service_principal(
                document,
                ECS_TASKS_SERVICE_PRINCIPAL,
            )
        })
        == Some(false)
}
