// SPDX-License-Identifier: CC0-1.0

//! The rollback deployment CodeDeploy creates for a deployment that was stopped,
//! and the `RollbackInfo` that links the two records.
//!
//! ONE place, because a rollback is the same record whatever stopped the
//! deployment: a caller's `StopDeployment` asking for one, or an alarm the
//! deployment group polls ([`super::alarm_gate`]). Both go through
//! [`record_rollback`], so the two can never report a rollback differently.
//!
//! What the rollback record is: a deployment of the revision this deployment
//! group was on BEFORE the stopped deployment — "the ID of the deployment
//! rollback" (`smithy_query(query="shape",shape="RollbackInfo")`) — created by the
//! service rather than by a client, so it carries the `codeDeployRollback`
//! creator (`smithy_query(query="shape",shape="DeploymentCreator")`) and is then
//! run by the clock like any other deployment. Both records carry the same
//! `RollbackInfo`, whose `rollbackTriggeringDeploymentId` is "the deployment ID of
//! the deployment that was underway and triggered a rollback deployment because it
//! failed or was stopped".
//!
//! A deployment group whose FIRST deployment is the one being stopped has no
//! revision to go back to, so no rollback deployment is created for it; the
//! `RollbackInfo` then carries no `rollbackDeploymentId` and says why.
//!
//! What the rollback DOES, for a deployment that had already shifted traffic: it
//! re-points the Lambda alias at the revision it redeploys — the version the
//! stopped deployment started from — in ONE shift rather than in the increments
//! that deployment was walking. A rollback is CodeDeploy's own remediation for a
//! deployment that was stopped, so it puts the alias back rather than walking a
//! window again (which is also why [`super::alarm_gate`] leaves it ungated).

use crate::ops::types::RollbackInfo;

use super::{
    deployment_completion_due, deployment_start_due, lambda_traffic_shift, revision_key,
    termination_wait, traffic_shift, ComAmazonawsCodedeployCodedeploy20141006State, Deployment,
    DEPLOYMENT_CREATOR_ROLLBACK, DEPLOYMENT_STATUS_CREATED,
};

/// Roll `deployment_id` back: mint the rollback deployment this group has a
/// revision for, write the `RollbackInfo` onto BOTH records, and answer that
/// information so the caller can report it.
///
/// Call it for a deployment that is being stopped — the status and the completion
/// of the stopped deployment itself belong to whoever stops it, so nothing here
/// touches them. `None` means the store holds no such deployment.
pub fn record_rollback(
    state: &mut ComAmazonawsCodedeployCodedeploy20141006State,
    deployment_id: &str,
    now_ms: i64,
) -> Option<RollbackInfo> {
    // What a rollback of this deployment redeploys, and what the rollback
    // deployment inherits from it: a rollback goes to the same deployment group
    // with the same configuration, so all of it comes from the stored record.
    let deployment = state.deployments.get(deployment_id)?;
    let rollback_revision = deployment.previous_revision.clone();
    let rolled_back_revision = deployment.revision.clone();
    let application_name = deployment.application_name.clone();
    let deployment_group_name = deployment.deployment_group_name.clone();
    let deployment_config_name = deployment.deployment_config_name.clone();
    let compute_platform = deployment.compute_platform.clone();
    let deployment_style = deployment.deployment_style.clone();
    let auto_rollback_configuration = deployment.auto_rollback_configuration.clone();
    // A rollback runs in the Region the deployment it rolls back ran in.
    let region = deployment.region.clone();
    // A rollback goes to the SAME deployment group, so it resolves the same
    // deployment targets as the deployment it rolls back, and its terminal status
    // follows from them the same way.
    let targets = deployment.targets.clone();

    // The id of the rollback deployment, minted only when this deployment group
    // has a revision to go back to. A group whose first deployment is being
    // stopped has none, which is what the rollback message then reports.
    let rollback_deployment_id = rollback_revision
        .is_some()
        .then(|| state.mint_deployment_id());
    let rollback_info = match &rollback_deployment_id {
        Some(rollback_id) => RollbackInfo {
            rollback_deployment_id: Some(rollback_id.clone()),
            rollback_triggering_deployment_id: Some(deployment_id.to_string()),
            rollback_message: Some(format!(
                "Deployment {rollback_id} was created to roll deployment {deployment_id} back \
                 to the previously deployed revision"
            )),
        },
        None => RollbackInfo {
            rollback_deployment_id: None,
            rollback_triggering_deployment_id: Some(deployment_id.to_string()),
            rollback_message: Some(format!(
                "Deployment {deployment_id} cannot be rolled back: no revision was deployed to \
                 this deployment group before it"
            )),
        },
    };
    if let Some(deployment) = state.deployments.get_mut(deployment_id) {
        deployment.rollback_info = Some(rollback_info.clone());
    }

    // The rollback is a DEPLOYMENT of its own, created by the service rather than
    // by a client, which the clock then runs like any other.
    if let (Some(rollback_id), Some(revision)) = (rollback_deployment_id, rollback_revision) {
        // A rollback is a deployment to the SAME deployment group, so it owes the
        // original environment whatever that group configured, exactly as a
        // client's own deployment to it does.
        let (terminates_original_instances, termination_wait_millis) = deployment_group_name
            .as_deref()
            .and_then(|group_name| {
                state
                    .applications
                    .get(&application_name)
                    .and_then(|application| application.deployment_groups.get(group_name))
            })
            .map_or((false, 0), termination_wait);
        // The traffic shift the rollback performs is the one the revision it
        // redeploys describes, read exactly as a client's own deployment of that
        // revision reads it: a rollback moves the alias back by re-pointing it,
        // rather than by undoing anything.
        let declared_shift = lambda_traffic_shift::declared_shift(&compute_platform, &revision);
        state.record_deployment(Deployment {
            deployment_id: rollback_id,
            application_name,
            deployment_group_name,
            deployment_config_name,
            // The revision's identity, computed once here so no later request
            // re-derives it from the revision's content.
            revision_key: revision_key(&revision),
            revision: Some(revision),
            // The revision this group was on when the rollback started: the one
            // the stopped deployment was deploying.
            previous_revision: rolled_back_revision,
            status: DEPLOYMENT_STATUS_CREATED.to_string(),
            creator: DEPLOYMENT_CREATOR_ROLLBACK.to_string(),
            description: None,
            ignore_application_stop_failures: false,
            update_outdated_instances_only: false,
            file_exists_behavior: None,
            deployment_mode: None,
            auto_rollback_configuration,
            // Nothing a client sent to the stopped deployment carries over: a
            // rollback targets the group's own instances with the group's own
            // settings.
            target_instances: None,
            override_alarm_configuration: None,
            ec2_deployment_options: None,
            deployment_style,
            compute_platform,
            external_id: None,
            rollback_info: Some(rollback_info.clone()),
            create_time_ms: now_ms,
            start_time_ms: None,
            complete_time_ms: None,
            starts_at_ms: deployment_start_due(now_ms),
            completes_at_ms: deployment_completion_due(now_ms) + termination_wait_millis,
            // Nothing has failed, and no lifecycle event hook has reported
            // against the rollback yet.
            error_information: None,
            lifecycle_hook_executions: Vec::new(),
            targets,
            terminates_original_instances,
            termination_wait_millis,
            instance_termination_wait_time_started: false,
            lambda_traffic_shift: declared_shift,
            traffic_shift: lambda_traffic_shift::TrafficShiftProgress::Declared,
            // In ONE shift: a rollback undoes, so it re-points the alias at the
            // revision it redeploys instead of walking the increments the
            // deployment it rolls back was walking.
            traffic_increments: traffic_shift::single_shift(),
            traffic_increments_applied: 0,
            region,
            // The rollback is a deployment of its own, so its own lifecycle
            // events are published as it reaches them.
            published_trigger_events: Vec::new(),
        });
    }

    Some(rollback_info)
}
