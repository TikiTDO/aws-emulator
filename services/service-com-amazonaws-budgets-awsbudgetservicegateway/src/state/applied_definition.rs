// SPDX-License-Identifier: CC0-1.0

//! The work an executed budget action's `Definition` DESCRIBES, performed in the service
//! that OWNS the resources it names — and taken back off again when the action is reversed.
//!
//! A budget action exists to apply something outside Budgets: `Action$ActionType` "defines
//! the type of tasks that can be carried out by this action", and `Action$Definition` is
//! "where you specify all of the type-specific parameters" — the policy to attach
//! (`IamActionDefinition$PolicyArn`, "the ARN of the policy to be attached"), the roles,
//! groups and users to attach it to, the service control policy and the targets it goes on
//! (`ScpActionDefinition$PolicyId` / `$TargetIds`), or the instances a document is run
//! against (`SsmActionDefinition`). Executing the action is what performs that work, and
//! reverting it is what takes it off again (`Action$ExecutionRoleArn`: "the role passed for
//! action execution and reversion") — so an action's settled `ActionStatus` is a report
//! about work the owning service did, not a status this control plane hands itself.
//!
//! Which service owns the work is the `ActionType`, and each is asked for it through its own
//! API: a managed policy is attached and detached by IAM
//! ([`::service_common::iam_client`]), a service control policy by AWS Organizations
//! ([`::service_common::organizations_client`]), and a document is run by Systems Manager
//! ([`::service_common::ssm_client`]) — this crate's one construction of each of those hops
//! rather than a client built here. Every one of those calls is made the way every emulated
//! service calls another — over the IN-PROCESS callback into this same emulator
//! ([`smithy_server_runtime::OperationContext::callback`]). Nothing here opens a socket or
//! leaves the host: each client's endpoint URL is an ADDRESS the in-process router reads,
//! never a host any resolver looks up, and the only part of it a request contributes is read
//! through the shared Region reading ([`::service_common::arn::region_label`]), so no
//! caller-supplied text reaches that authority.
//!
//! What the owning service ANSWERS is what the action's status reports ([`Outcome`]):
//!
//! * the work is in place — the call was SERVED, or the service answered that the end state
//!   this work aims at already holds (a policy already attached, an attachment that is not
//!   there to take off);
//! * the work did NOT happen — the owning service answered anything else: a refusal of the
//!   request, a fault of its own, or that it does not implement the call. Each is an answer
//!   that the policy was not attached or the document not run, so the action settles to the
//!   matching `*_FAILURE` status ([`super::failed_settle`]) rather than reporting a success
//!   nothing backs — and `RETRY_BUDGET_ACTION` is the execution type that runs the work
//!   again;
//! * nothing was learned — the call reached NO service: the in-process transport refused it
//!   because no mounted service claims it or more than one does
//!   ([`::service_common::in_process_refusal`]), or there was no answer to read at all. That
//!   is not evidence about the work, so the status the state machine already owes stands.
//!   This is the same line the one other reading in this service that asks another service
//!   about a caller-supplied value draws ([`super::execution_role`], over
//!   [`::service_common::iam_role`]): a report has to be grounded in what the owning service
//!   said.
//!
//! The fan-out is bounded at the calls themselves ([`MAX_NAMED_TARGETS`]) and by what an
//! action can hold: `IamActionDefinition`'s three attach lists and
//! `ScpActionDefinition$TargetIds` are each `@length(max: 100)`, and one action carries one
//! `Definition`, so a request cannot grow the number of calls one execution makes.
//!
//! Two of the three legs reach something this emulator does not serve yet, and the rule above
//! is what reports that: AWS Organizations answers `AttachPolicy` and `DetachPolicy` with the
//! 501 of an operation nobody has implemented, and Systems Manager publishes no `AWS-Stop…` /
//! `AWS-Start…` Automation runbook for either `ActionSubType`, so an `APPLY_SCP_POLICY` or
//! `RUN_SSM_DOCUMENTS` action settles at the `*_FAILURE` status — no policy was attached and
//! no instance was stopped, and a success for either would be a status for work nothing did.
//! Each of those is a gap in the OWNING service; when it closes, such an action settles at
//! the success status with nothing here changing.

use crate::ops::types::{Definition, IamActionDefinition, ScpActionDefinition, SsmActionDefinition};

use ::aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use ::aws_smithy_runtime_api::client::result::SdkError;
use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

/// Which way one execution takes the work the definition describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    /// Put the definition in place — what the approval of a triggered action performs.
    Apply,
    /// Take it back off — what a reversion performs, and what re-arming an action that has
    /// already run performs too, since an armed action is one whose definition is not in
    /// place ([`super::ACTION_STATUS_STANDBY`]).
    Reverse,
}

/// What the service that owns the work answered about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The work the definition describes is in place (or, for a reversal, off again).
    Done,
    /// The owning service answered that it did NOT do the work, so the definition is not in
    /// place and the execution did not complete.
    Refused,
    /// Nothing was learned about the work — the call reached no service at all (see the
    /// module docs).
    Unknown,
}

impl Outcome {
    /// The outcome of two pieces of one execution's work: an answer that a piece was not done
    /// is the execution's outcome, since the definition is then only partly in place, and a
    /// piece nothing was learned about leaves the whole no better known than that — so a
    /// success is reported only where every piece of the work was answered for.
    fn with(self, other: Self) -> Self {
        match (self, other) {
            (Self::Refused, _) | (_, Self::Refused) => Self::Refused,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            _ => Self::Done,
        }
    }
}

/// The work `execution` performs on the action's definition, or `None` for a move that
/// leaves the definition where it is.
///
/// Read off the status the execution SETTLES to, which is what that status means: the
/// definition is in place at [`super::ACTION_STATUS_EXECUTION_SUCCESS`], off again at
/// [`super::ACTION_STATUS_REVERSE_SUCCESS`], and not in place at
/// [`super::ACTION_STATUS_STANDBY`] — the status a completed reset leaves an action armed
/// at. So an approval and the retry of a failed execution apply it, and a reversion, the
/// retry of a failed reversion and a reset take it off.
pub fn work_of(execution: super::ActionExecution) -> Option<Work> {
    match execution.settles_to {
        super::ACTION_STATUS_EXECUTION_SUCCESS => Some(Work::Apply),
        super::ACTION_STATUS_REVERSE_SUCCESS | super::ACTION_STATUS_STANDBY => Some(Work::Reverse),
        _ => None,
    }
}

/// Perform `work` on the definition of an `action_type` action, and answer what the service
/// that owns it said.
///
/// Which member of `Definition` describes the work is the action type — "this field also
/// determines the format for definition" (`Action$ActionType`) — and a stored action carries
/// exactly that member ([`super::validate_action_definition`] holds every write to it), so a
/// definition that describes nothing for its type asks nothing of any service.
pub async fn perform(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action_type: &str,
    definition: &Definition,
    work: Work,
) -> Outcome {
    match action_type {
        super::ACTION_TYPE_APPLY_IAM_POLICY => match &definition.iam_action_definition {
            Some(iam) => apply_iam_policy(ctx, iam, work).await,
            None => Outcome::Done,
        },
        super::ACTION_TYPE_APPLY_SCP_POLICY => match &definition.scp_action_definition {
            Some(scp) => apply_scp_policy(ctx, scp, work).await,
            None => Outcome::Done,
        },
        super::ACTION_TYPE_RUN_SSM_DOCUMENTS => match &definition.ssm_action_definition {
            Some(ssm) => run_ssm_document(ctx, ssm, work).await,
            None => Outcome::Done,
        },
        // An `ActionType` this emulator does not know, which the modelled enum already
        // refuses at the boundary: no service owns work for it.
        _ => Outcome::Done,
    }
}

/// Attach `definition.policy_arn` to every role, group and user the definition names — or
/// detach it from each of them.
///
/// "A list of roles to be attached", "a list of groups to be attached", "a list of users to
/// be attached": each list names IAM identities by the friendly name IAM holds them under,
/// and the policy goes on each of them, so an action naming identities of more than one kind
/// attaches to all of them. An identity list the definition omits names nothing to attach
/// to.
///
/// The three lists are walked by ONE loop, and the six calls the kinds and the two directions
/// make sit side by side in its one match, so no kind can drift from the others.
async fn apply_iam_policy(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    definition: &IamActionDefinition,
    work: Work,
) -> Outcome {
    /// Which of a definition's three identity lists a name came out of, which is what says
    /// through which of IAM's attach and detach calls the policy goes on or comes off it.
    enum Kind {
        Role,
        Group,
        User,
    }

    let iam = ::service_common::iam_client::iam_client(ctx, super::PARTITION_GLOBAL_REGION);
    let policy_arn = definition.policy_arn.as_str();
    let lists = [
        (Kind::Role, definition.roles.as_deref()),
        (Kind::Group, definition.groups.as_deref()),
        (Kind::User, definition.users.as_deref()),
    ];
    let mut outcome = Outcome::Done;
    for (kind, named) in lists {
        for name in named.unwrap_or_default().iter().take(MAX_NAMED_TARGETS) {
            let answered = match (&kind, work) {
                (Kind::Role, Work::Apply) => answer(
                    iam.attach_role_policy()
                        .role_name(name)
                        .policy_arn(policy_arn)
                        .send()
                        .await
                        .map(|_| ()),
                    &[],
                ),
                (Kind::Role, Work::Reverse) => answer(
                    iam.detach_role_policy()
                        .role_name(name)
                        .policy_arn(policy_arn)
                        .send()
                        .await
                        .map(|_| ()),
                    NOTHING_TO_TAKE_OFF,
                ),
                (Kind::Group, Work::Apply) => answer(
                    iam.attach_group_policy()
                        .group_name(name)
                        .policy_arn(policy_arn)
                        .send()
                        .await
                        .map(|_| ()),
                    &[],
                ),
                (Kind::Group, Work::Reverse) => answer(
                    iam.detach_group_policy()
                        .group_name(name)
                        .policy_arn(policy_arn)
                        .send()
                        .await
                        .map(|_| ()),
                    NOTHING_TO_TAKE_OFF,
                ),
                (Kind::User, Work::Apply) => answer(
                    iam.attach_user_policy()
                        .user_name(name)
                        .policy_arn(policy_arn)
                        .send()
                        .await
                        .map(|_| ()),
                    &[],
                ),
                (Kind::User, Work::Reverse) => answer(
                    iam.detach_user_policy()
                        .user_name(name)
                        .policy_arn(policy_arn)
                        .send()
                        .await
                        .map(|_| ()),
                    NOTHING_TO_TAKE_OFF,
                ),
            };
            outcome = outcome.with(answered);
        }
    }
    outcome
}

/// Attach the service control policy `definition.policy_id` names to every target it names
/// — or detach it from each of them.
///
/// `ScpActionDefinition$TargetIds` is "a list of target IDs", which is what Organizations
/// attaches a policy to one at a time (`AttachPolicy`'s `TargetId`), so one call is made per
/// target.
async fn apply_scp_policy(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    definition: &ScpActionDefinition,
    work: Work,
) -> Outcome {
    let organizations = ::service_common::organizations_client::organizations_client(
        ctx.callback(),
        super::PARTITION_GLOBAL_REGION,
    );
    let policy_id = definition.policy_id.as_str();
    let mut outcome = Outcome::Done;
    for target in definition.target_ids.iter().take(MAX_NAMED_TARGETS) {
        let answered = match work {
            Work::Apply => answer(
                organizations
                    .attach_policy()
                    .policy_id(policy_id)
                    .target_id(target)
                    .send()
                    .await
                    .map(|_| ()),
                ALREADY_IN_PLACE,
            ),
            Work::Reverse => answer(
                organizations
                    .detach_policy()
                    .policy_id(policy_id)
                    .target_id(target)
                    .send()
                    .await
                    .map(|_| ()),
                NOTHING_TO_TAKE_OFF,
            ),
        };
        outcome = outcome.with(answered);
    }
    outcome
}

/// Run the Systems Manager document the definition's `ActionSubType` names against the
/// instances it names, in the Region it names.
///
/// `SsmActionDefinition` carries the sub type ("the action subType"), "the Region to run the
/// SSM document" and "the EC2 and RDS instance IDs" — so the document is the one that
/// performs that sub type ([`ssm_runbook`]), the instances are its target, and the call is
/// addressed in that Region and no other.
async fn run_ssm_document(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    definition: &SsmActionDefinition,
    work: Work,
) -> Outcome {
    // The Region ends up in the hop's endpoint authority, so it is read through the shared
    // Region reading rather than used as supplied: a value that is not a Region label names
    // no Systems Manager endpoint to address at all, so no document was run — and the same
    // holds for an `ActionSubType` outside the modelled catalog, which names no runbook. In
    // both cases the work did not happen, which is what the action must report.
    let Some(region) = ::service_common::arn::region_label(&definition.region) else {
        return Outcome::Refused;
    };
    let Some(document) = ssm_runbook(&definition.action_sub_type, work) else {
        return Outcome::Refused;
    };
    answer(
        ::service_common::ssm_client::ssm_client(ctx.callback(), region)
            .start_automation_execution()
            .document_name(document)
            .parameters(
                SSM_INSTANCE_PARAMETER.to_string(),
                definition.instance_ids.clone(),
            )
            .send()
            .await
            .map(|_| ()),
        &[],
    )
}

/// The Systems Manager Automation runbook that performs `action_sub_type`, in the direction
/// `work` takes it.
///
/// SOURCE: the Systems Manager Automation runbook reference — `AWS-StopEC2Instance` and
/// `AWS-StopRdsInstance` stop an EC2 instance and an RDS DB instance, and
/// `AWS-StartEC2Instance` and `AWS-StartRdsInstance` start them again, which is what
/// reverting an action that stopped them performs. `None` for an `ActionSubType` outside the
/// modelled catalog (`STOP_EC2_INSTANCES`, `STOP_RDS_INSTANCES`), which the modelled enum
/// already refuses at the boundary.
fn ssm_runbook(action_sub_type: &str, work: Work) -> Option<&'static str> {
    match (action_sub_type, work) {
        (ACTION_SUB_TYPE_STOP_EC2_INSTANCES, Work::Apply) => Some("AWS-StopEC2Instance"),
        (ACTION_SUB_TYPE_STOP_EC2_INSTANCES, Work::Reverse) => Some("AWS-StartEC2Instance"),
        (ACTION_SUB_TYPE_STOP_RDS_INSTANCES, Work::Apply) => Some("AWS-StopRdsInstance"),
        (ACTION_SUB_TYPE_STOP_RDS_INSTANCES, Work::Reverse) => Some("AWS-StartRdsInstance"),
        _ => None,
    }
}

/// `ActionSubType$STOP_EC2_INSTANCES` — the sub type whose document stops the EC2 instances
/// the definition names.
const ACTION_SUB_TYPE_STOP_EC2_INSTANCES: &str = "STOP_EC2_INSTANCES";

/// `ActionSubType$STOP_RDS_INSTANCES` — the sub type whose document stops the RDS instances
/// the definition names.
const ACTION_SUB_TYPE_STOP_RDS_INSTANCES: &str = "STOP_RDS_INSTANCES";

/// The parameter each runbook above takes the instances to act on as, which is the one
/// parameter the definition states a value for (`SsmActionDefinition$InstanceIds`).
const SSM_INSTANCE_PARAMETER: &str = "InstanceId";

/// How many targets of one list this performs the work on, which is the ceiling on the
/// number of calls one execution makes per list.
///
/// The model already bounds each list — `IamActionDefinition$Roles`, `$Groups` and `$Users`
/// and `ScpActionDefinition$TargetIds` are each `@length(min: 1, max: 100)`, which request
/// validation holds every write that establishes a definition to — so a stored definition
/// cannot name more than this and nothing of one is skipped. It is stated again HERE, where
/// the calls are made, so the fan-out of one execution is capped at the growth site itself
/// rather than only upstream of it.
const MAX_NAMED_TARGETS: usize = 100;

/// The answers that say the end state an APPLY aims at already holds, so there is nothing
/// left to put in place: a policy a target already carries. (IAM answers an attach of a
/// policy an identity already carries with success, so no code of its own is listed.)
const ALREADY_IN_PLACE: &[&str] = &["DuplicatePolicyAttachmentException"];

/// The answers that say there is nothing to TAKE OFF, so the end state a reversal aims at
/// already holds: an attachment the owning service does not hold, and an identity or policy
/// it holds nothing under — in each case the definition is not in place on anything the
/// action named.
const NOTHING_TO_TAKE_OFF: &[&str] = &[
    "NoSuchEntity",
    "NoSuchEntityException",
    "PolicyNotAttachedException",
    "PolicyNotFoundException",
    "TargetNotFoundException",
];

/// What one call to the owning service answered about the work it was asked for.
///
/// A call the owning service SERVED put the work in place, and so did one it answered with a
/// code in `satisfied_by`: the end state this work aims at already holds.
///
/// Every other ANSWER says the work did not happen — a refusal of the request, a fault of
/// the service's own, and the 501 with which this emulator answers a call it does not
/// implement alike: in each case the policy is not attached and the document was not run, so
/// the action must not report that it was. The one case nothing is learned from is a call
/// that reached NO service: the in-process transport's own refusal
/// ([`::service_common::in_process_refusal`]) — no mounted service claims the request, or
/// more than one does — and a failure that carried no answer at all.
fn answer<E: ProvideErrorMetadata + ::std::error::Error + 'static>(
    answered: Result<(), SdkError<E, HttpResponse>>,
    satisfied_by: &[&str],
) -> Outcome {
    let Err(error) = answered else {
        return Outcome::Done;
    };
    if satisfied_by.contains(&error.code().unwrap_or_default()) {
        return Outcome::Done;
    }
    let reached_nothing = error.raw_response().is_none_or(|response| {
        response
            .body()
            .bytes()
            .is_some_and(::service_common::in_process_refusal::is_transport_refusal)
    });
    // What the answer means for the action is reported through its status, which is what a
    // client reads; this record is for the developer running the emulator, and carries the
    // answer rather than any member of the definition.
    ::tracing::info!(
        reached_nothing,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "the work a budget action's definition describes was not performed by the service \
         that owns it"
    );
    if reached_nothing {
        Outcome::Unknown
    } else {
        Outcome::Refused
    }
}
