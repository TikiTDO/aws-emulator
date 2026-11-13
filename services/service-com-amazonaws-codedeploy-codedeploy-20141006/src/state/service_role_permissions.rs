// SPDX-License-Identifier: CC0-1.0

//! Whether the SERVICE ROLE a deployment group runs as lets its deployments do
//! what they are about to do.
//!
//! A deployment does not act as whoever called `CreateDeployment`: it acts as its
//! deployment group's `serviceRoleArn`, "a service role Amazon Resource Name (ARN)
//! that allows CodeDeploy to act on the user's behalf when interacting with Amazon
//! Web Services services"
//! (`smithy_query(query="doc",shape="CreateDeploymentGroupInput",member="serviceRoleArn")`).
//! So what that role may do decides what the deployment can do, and a deployment
//! whose role does not allow the call it has to make is one that FAILS rather than
//! one that happens: the model carries `IAM_ROLE_PERMISSIONS` as an `ErrorCode`
//! (`smithy_query(query="shape",shape="ErrorCode")`) for exactly that, and
//! CodeDeploy's "Error codes" page gives it as the service role not granting
//! enough permissions.
//!
//! The verdict is IN BAND, not a refusal of a call: both `CreateDeploymentGroup`
//! and `CreateDeployment` ACCEPT an under-privileged role — nothing about a role's
//! permissions is a question about the shape of those requests — and the deployment
//! then reaches the terminal `Failed` status carrying the error information. That is
//! why this runs where [`super::alarm_gate`] runs, from [`super::with_state`],
//! rather than in a handler.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **which calls** — the calls a deployment makes on ANOTHER service, which
//!    follow from the deployment itself: the alias write a Lambda-platform
//!    deployment performs ([`super::lambda_alias`]), and the alarm poll a
//!    deployment whose effective `alarmConfiguration` is enabled performs
//!    ([`super::alarm_gate`]). One call per service reached is enough to decide
//!    the verdict, because the sentence CodeDeploy answers names a SERVICE rather
//!    than an action;
//! 2. **what the role may do** — the identity-based documents the IAM store holds
//!    for that role (its inline policies and the default version of each managed
//!    policy attached to it), read through IAM's own `principal_policies` accessor
//!    so this service sees exactly what `CreateRole`/`PutRolePolicy` wrote, plus
//!    the permissions boundary that caps them;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of policy documents against `(action, resource)`, so this service
//!    and every other one handed the same documents decide alike.
//!
//! The order matters: the role is consulted BEFORE the alias is written and before
//! the alarms are polled, so a deployment the role does not authorize changes
//! nothing — the alias stays on the version it was on, with the routing it had —
//! and `ignorePollAlarmFailure`, which is about alarm state that could not be
//! retrieved, does not excuse a role that may not retrieve it at all.
//!
//! Nothing is refused on evidence this emulator does not have. Each of these shows
//! nothing about the role and lets the deployment go ahead: an engine carrying no
//! IAM store, a `serviceRoleArn` the store holds no role for (it may belong to
//! another account), a role carrying no policy document this emulator can read, and
//! a role carrying an attachment whose document this account does not hold — an
//! AWS-managed policy might be the very one that carries the grant. What the
//! documents DO state is decided in IAM's own order: an explicit `Deny` refuses, a
//! grant allows, and documents that state nothing about the call withhold it,
//! because the documents in force on a role are the authority on what it may do.
//!
//! Like [`super::alarm_gate`], it runs with THIS store unlocked while the IAM store
//! is locked: the two are never held at once.

use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::ErrorInformation;

use super::service_role::{iam_state, IamState};
use super::{
    alarm_gate, deployment_is_terminal, now_ms, rollback, service_state,
    ComAmazonawsCodedeployCodedeploy20141006State, Deployment, DEPLOYMENT_STATUS_FAILED,
    DEPLOYMENT_STATUS_IN_PROGRESS,
};

/// The `ErrorCode` CodeDeploy reports on a deployment its deployment group's
/// service role did not authorize
/// (`smithy_query(query="shape",shape="ErrorCode")` carries it).
pub const ERROR_CODE_IAM_ROLE_PERMISSIONS: &str = "IAM_ROLE_PERMISSIONS";

/// The name CodeDeploy gives AWS Lambda in [`iam_role_permissions_message`], the
/// service a Lambda-platform deployment reaches to move its alias.
const AWS_SERVICE_LAMBDA: &str = "AWSLambda";

/// The name CodeDeploy gives Amazon CloudWatch in [`iam_role_permissions_message`],
/// the service a deployment with an enabled alarm configuration reaches to poll
/// those alarms.
const AWS_SERVICE_CLOUDWATCH: &str = "AmazonCloudWatch";

/// The IAM action the alias write of a Lambda traffic shift needs: a shift moves
/// the alias its AppSpec names, which is the call `UpdateAlias` is.
const ACTION_UPDATE_ALIAS: &str = "lambda:UpdateAlias";

/// The IAM action the alarm poll needs: the gate reads the state of the alarms the
/// configuration names, which is the call `DescribeAlarms` is.
const ACTION_DESCRIBE_ALARMS: &str = "cloudwatch:DescribeAlarms";

/// One call a deployment makes on another AWS service, as the authorization
/// question is asked about it.
#[derive(Debug, Clone)]
struct RequiredCall {
    /// The AWS service the call reaches, as CodeDeploy NAMES it in its refusal.
    service: &'static str,
    /// The IAM action the call needs.
    action: &'static str,
    /// The ARN of the resource the call acts on, so a role granted the action on
    /// that resource alone is allowed it exactly as one granted it on everything.
    resource: String,
}

/// What one deployment's pre-flight asks about: the role it runs as, and the calls
/// it makes.
#[derive(Debug, Clone)]
struct Preflight {
    /// The deployment the verdict is about.
    deployment_id: String,
    /// The `serviceRoleArn` of its deployment group, read off the group at this
    /// instant so a group whose role was replaced is judged by the role it carries
    /// now.
    role_arn: String,
    /// The calls [`RequiredCall`] describes, in the order they are asked about.
    calls: Vec<RequiredCall>,
}

/// Which AWS service a deployment's role did not authorize, which is the one
/// variable part of the sentence besides the role itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Denial {
    /// See [`RequiredCall::service`].
    service: &'static str,
}

/// Fail every deployment in flight whose deployment group's service role does not
/// authorize a call that deployment makes.
///
/// Runs before the alarm gate and before any traffic moves ([`super::with_state`]),
/// so a deployment the role does not authorize has no effect at all. The failure is
/// a transition the CLOCK owed — the role withheld the permission at that instant
/// whether or not anybody called — so materialising it from a reporting operation
/// shows a caller nothing a later caller would not also see.
///
/// A store holding no deployment that reaches another service costs one read of this
/// store and no IAM lock at all.
pub fn refuse_unauthorized_deployments<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let now = now_ms();
    let pending = {
        let store = state.lock();
        preflights(&store, now)
    };
    if pending.is_empty() {
        return;
    }
    // No IAM beside this service is no evidence about any role, so there is
    // nothing to decide.
    let Some(iam) = iam_state(engine) else {
        return;
    };
    let refused: Vec<(Preflight, Denial)> = {
        let held = iam.lock();
        pending
            .into_iter()
            .filter_map(|preflight| {
                denial(&preflight, &held).map(|denial| (preflight, denial))
            })
            .collect()
    };
    if refused.is_empty() {
        return;
    }
    let mut store = state.lock();
    for (preflight, denial) in &refused {
        fail_deployment(&mut store, preflight, denial, now);
    }
}

/// The pre-flight of every deployment that is in flight at `now_ms` and makes at
/// least one call on another service.
fn preflights(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    now_ms: i64,
) -> Vec<Preflight> {
    state
        .pending_deployments
        .iter()
        .filter_map(|id| state.deployments.get(id))
        .filter(|deployment| is_under_way(deployment, now_ms))
        .filter_map(|deployment| preflight_of(state, deployment))
        .collect()
}

/// Whether `deployment` is one the role is consulted for at `now_ms`.
///
/// A deployment that has reached an outcome is owed nothing further, and one the
/// clock has not yet started has made no call to be refused — which is also why
/// `CreateDeployment` is answered with the acceptance it would be answered with
/// anyway and the failure follows out of band.
fn is_under_way(deployment: &Deployment, now_ms: i64) -> bool {
    deployment.complete_time_ms.is_none()
        && !deployment_is_terminal(&deployment.status)
        && deployment.starts_at_ms <= now_ms
}

/// The pre-flight `deployment` runs under, or `None` when its group is gone from
/// the store or it makes no call this emulator asks about.
fn preflight_of(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    deployment: &Deployment,
) -> Option<Preflight> {
    let group = rollback::group_of(state, deployment)?;
    let calls = required_calls(state, deployment);
    (!calls.is_empty()).then(|| Preflight {
        deployment_id: deployment.deployment_id.clone(),
        role_arn: group.service_role_arn.clone(),
        calls,
    })
}

/// The calls `deployment` makes on another AWS service.
///
/// One per service it reaches: the alias write its declared traffic shift performs
/// in AWS Lambda, and the alarm poll its effective alarm configuration performs in
/// Amazon CloudWatch — the configuration the gate that polls them resolves, so the
/// deployment's own override takes precedence over its group's exactly as it does
/// there.
fn required_calls(
    state: &ComAmazonawsCodedeployCodedeploy20141006State,
    deployment: &Deployment,
) -> Vec<RequiredCall> {
    let mut calls = Vec::new();
    if let Some(shift) = deployment.lambda_traffic_shift.as_ref() {
        calls.push(RequiredCall {
            service: AWS_SERVICE_LAMBDA,
            action: ACTION_UPDATE_ALIAS,
            resource: alias_arn(&deployment.region, &shift.function_name, &shift.alias_name),
        });
    }
    if let Some(configuration) = alarm_gate::effective_alarm_configuration(state, deployment) {
        if configuration.enabled {
            calls.extend(
                alarm_gate::polled_alarms(configuration)
                    .into_iter()
                    .map(|alarm| RequiredCall {
                        service: AWS_SERVICE_CLOUDWATCH,
                        action: ACTION_DESCRIBE_ALARMS,
                        resource: alarm_arn(&deployment.region, &alarm),
                    }),
            );
        }
    }
    calls
}

/// The ARN of the Lambda alias a traffic shift moves:
/// `arn:<partition>:lambda:<region>:<account>:function:<function-name>:<alias>`,
/// the form the AWS Lambda Developer Guide gives for an alias. The account segment
/// comes from the engine, never from a literal.
fn alias_arn(region: &str, function_name: &str, alias_name: &str) -> String {
    ::engine::emulator_arn(
        "lambda",
        region,
        &format!("function:{function_name}:{alias_name}"),
    )
}

/// The ARN of a CloudWatch alarm the gate polls:
/// `arn:<partition>:cloudwatch:<region>:<account>:alarm:<alarm-name>`, the form the
/// Amazon CloudWatch User Guide gives for an alarm.
fn alarm_arn(region: &str, alarm_name: &str) -> String {
    ::engine::emulator_arn("cloudwatch", region, &format!("alarm:{alarm_name}"))
}

/// Which service the role of `preflight` does not authorize a call on, or `None`
/// when nothing of the kind can be shown (see the module docs for the cases that
/// show nothing).
fn denial(preflight: &Preflight, iam: &IamState) -> Option<Denial> {
    let role_arn = preflight.role_arn.as_str();
    // A role this account does not hold shows nothing: it may belong to another
    // account, or to a client that never told this emulator about it.
    iam.roles.values().find(|role| role.arn == role_arn)?;
    // The documents in force on the role. The collection is bounded by IAM's own
    // quotas on how many policies one role carries, so this walk is bounded by the
    // store rather than by a number of its own.
    let policies = iam.principal_policies(role_arn)?;
    // An attachment whose document this emulator does not hold could be the very
    // one that carries the grant, and a role carrying no readable document at all
    // says nothing this emulator can evaluate.
    if !policies.uncarried.is_empty() || policies.documents.is_empty() {
        return None;
    }
    // Read once, asked for each call: n calls cost n statement walks rather than n
    // parses.
    let prepared = iam::prepare_documents(&policies.documents);
    let boundary = iam
        .permissions_boundary_document(role_arn)
        .map(|document| iam::PreparedDocument::read(&document));
    preflight.calls.iter().find_map(|call| {
        let request = iam::Request::new(call.action, &call.resource, &[]);
        let granted = iam::identity_decision_prepared(&prepared, &request) == Decision::Allowed
            // The ceiling attached to the role caps what its own policies grant, so
            // a call the boundary does not allow is withheld however widely the
            // role is granted it.
            && boundary.as_ref().is_none_or(|boundary| {
                iam::identity_decision_prepared(::std::slice::from_ref(boundary), &request)
                    == Decision::Allowed
            });
        (!granted).then_some(Denial {
            service: call.service,
        })
    })
}

/// `errorInformation.message` as CodeDeploy words a deployment its service role did
/// not authorize.
///
/// The sentence is fixed; its variable parts are the role the deployment group
/// carries and the AWS service the deployment could not reach under it — what has
/// to change, and where. It names no action: CodeDeploy reports the SERVICE the
/// role gives no permission to operate in.
#[must_use]
pub fn iam_role_permissions_message(role_arn: &str, service_name: &str) -> String {
    format!(
        "The IAM role {role_arn} does not give you permission to perform operations in the \
         following AWS service: {service_name}. Contact your AWS administrator if you need \
         help. If you are an AWS administrator, you can grant permissions to your users or \
         groups by creating IAM policies."
    )
}

/// Fail the deployment `preflight` is about at `now_ms`, with the error information
/// CodeDeploy reports for a service role that does not authorize it, and roll it
/// back when its configuration asks for that on a deployment failure.
///
/// This store was unlocked while IAM was read, so what the pre-flight was resolved
/// from is checked again: a deployment another caller stopped, or the clock
/// completed, in between keeps the outcome it already has. A deployment the clock
/// had not yet started also gets the start it was owed, so one that failed before
/// anybody read it still reports the instant it began.
fn fail_deployment(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    preflight: &Preflight,
    denial: &Denial,
    now_ms: i64,
) {
    let deployment_id = preflight.deployment_id.as_str();
    if !state
        .deployments
        .get(deployment_id)
        .is_some_and(|deployment| is_under_way(deployment, now_ms))
    {
        return;
    }
    if let Some(deployment) = state.deployments.get_mut(deployment_id) {
        if deployment.start_time_ms.is_none() {
            deployment.start_time_ms = Some(deployment.starts_at_ms);
            deployment.status = DEPLOYMENT_STATUS_IN_PROGRESS.to_string();
        }
    }
    state.settle_deployment(
        deployment_id,
        DEPLOYMENT_STATUS_FAILED,
        Some(ErrorInformation {
            code: Some(ERROR_CODE_IAM_ROLE_PERMISSIONS.to_string()),
            message: Some(iam_role_permissions_message(
                &preflight.role_arn,
                denial.service,
            )),
        }),
        now_ms,
    );
    // A deployment that failed is rolled back when its configuration asks for that
    // on a deployment failure, from the one place that is decided.
    rollback::roll_back_on_failure(state, deployment_id, now_ms);
}
