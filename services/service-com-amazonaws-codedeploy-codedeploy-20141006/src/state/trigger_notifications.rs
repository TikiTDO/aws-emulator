// SPDX-License-Identifier: CC0-1.0

//! The Amazon SNS notifications a deployment group's triggers publish.
//!
//! # The rule this module implements
//!
//! A trigger configuration exists to be ACTED on: `TriggerConfig.triggerTargetArn`
//! is "the Amazon Resource Name (ARN) of the Amazon Simple Notification Service
//! topic through which notifications about deployment or instance events are sent"
//! and `triggerEvents` is "the event type or types for which notifications are
//! triggered"
//! (`smithy_query(query="shape",shape="TriggerConfig",model_path="services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json")`).
//! So every lifecycle event a deployment REACHES is published, once, to each
//! trigger of its deployment group whose `triggerEvents` name that event, and to no
//! other topic; and storing a trigger list publishes the one-time verification
//! notification that tells each named topic the trigger was configured on it.
//! Storing the list and only reading it back leaves both sentences unimplemented:
//! a subscriber of the topic receives nothing at all, and a topic configured for
//! another event cannot be told apart from one nothing was ever published to.
//!
//! # Which events a deployment of this emulator reaches
//!
//! `TriggerEventType` carries ten values
//! (`smithy_query(query="shape",shape="TriggerEventType")`: DeploymentStart |
//! DeploymentSuccess | DeploymentFailure | DeploymentStop | DeploymentRollback |
//! DeploymentReady | InstanceStart | InstanceSuccess | InstanceFailure |
//! InstanceReady). The five DEPLOYMENT-level events this module publishes are the
//! lifecycle facts this service records about a deployment: the start the clock
//! owes it, each of the three outcomes it can hold ([`super::deployment_is_terminal`])
//! and the rollback deployment a stop gives it ([`super::rollback`]). The other
//! five are not published, because nothing here establishes them: no deployment of
//! this emulator is reported in the `Ready` status that `DeploymentReady`
//! describes, and a deployment's targets carry a status PROJECTED from the
//! deployment's own ([`super::target_status`]) rather than a per-instance lifecycle
//! this service measures, so an instance-level notification would report a fact
//! nothing here holds.
//!
//! # How a notification is published
//!
//! Nothing here decides WHEN a deployment moves and nothing here reads the store:
//! the events one pass of the store owes are selected from the deployment's own
//! record and its group's stored trigger list
//! ([`super::ComAmazonawsCodedeployCodedeploy20141006State::queue_due_trigger_notifications`]),
//! queued on the store, and published by [`deliver`] with the store UNLOCKED — by
//! the operation that produced them, and by the deployment clock
//! ([`super::deployment_clock`]) for the transitions that fall due while nobody is
//! calling this service. Each event is marked on the deployment as it is queued, so
//! one event is published once however many passes run over it.
//!
//! What a client can observe is one `sns:Publish` onto the topic, made the way every
//! emulated service calls another: over the IN-PROCESS callback into this same
//! emulator ([`smithy_server_runtime::EmbeddedEmulator`]). Nothing here opens a
//! socket or leaves the host — the ARN a caller supplied is not a URL this code
//! dials, it is a topic the request is routed to INSIDE this process, so the topic
//! has to be one this emulator itself holds or Amazon SNS answers that it does not
//! exist. The only thing read out of the caller's ARN is which Region of this
//! emulator the publish is addressed in, under the workspace's one Region-label
//! grammar (`service_common::arn::region_of`), so a value carrying anything else
//! names no topic at all and is not published to.
//!
//! A publish the topic's own service refuses — most often a topic that does not
//! exist, or one no Amazon SNS is mounted to hold — is not the CodeDeploy
//! operation's failure: the group is stored and the deployment keeps the outcome it
//! reached, and a notification that could not be delivered is logged while the rest
//! are still published.

use super::{
    timestamp, Deployment, DeploymentGroup, DEPLOYMENT_STATUS_CREATED,
    DEPLOYMENT_STATUS_FAILED, DEPLOYMENT_STATUS_IN_PROGRESS, DEPLOYMENT_STATUS_STOPPED,
    DEPLOYMENT_STATUS_SUCCEEDED,
};

/// The vendor field of an Amazon SNS ARN: the service namespace Amazon SNS
/// publishes, which is what says a `triggerTargetArn` names a topic at all.
const SNS_ARN_VENDOR: &str = "sns";

/// `TriggerEventType$DeploymentStart`: the deployment started.
pub const EVENT_DEPLOYMENT_START: &str = "DeploymentStart";
/// `TriggerEventType$DeploymentSuccess`: the deployment reached
/// [`DEPLOYMENT_STATUS_SUCCEEDED`].
pub const EVENT_DEPLOYMENT_SUCCESS: &str = "DeploymentSuccess";
/// `TriggerEventType$DeploymentFailure`: the deployment reached
/// [`DEPLOYMENT_STATUS_FAILED`].
pub const EVENT_DEPLOYMENT_FAILURE: &str = "DeploymentFailure";
/// `TriggerEventType$DeploymentStop`: the deployment reached
/// [`DEPLOYMENT_STATUS_STOPPED`].
pub const EVENT_DEPLOYMENT_STOP: &str = "DeploymentStop";
/// `TriggerEventType$DeploymentRollback`: the deployment was given a rollback
/// deployment ([`super::rollback`]).
pub const EVENT_DEPLOYMENT_ROLLBACK: &str = "DeploymentRollback";

/// One notification waiting to be published: the topic one trigger configuration
/// named, and the body a subscriber of that topic receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingNotification {
    /// The `triggerTargetArn` of the trigger this notification is published for.
    pub topic_arn: String,
    /// The notification, as this module renders it.
    pub message: String,
}

/// The lifecycle events `deployment` has REACHED, in the order it reached them.
///
/// Read off the stored record rather than off whatever moved it, so a deployment
/// the clock settled, one a caller stopped and one a failed validation function
/// failed all reach their events the same way.
#[must_use]
pub fn events_reached(deployment: &Deployment) -> Vec<&'static str> {
    let mut reached = Vec::new();
    if deployment.start_time_ms.is_some() {
        reached.push(EVENT_DEPLOYMENT_START);
    }
    // The rollback deployment a stop gave this one: the rollback IS a record of
    // its own ("the ID of the deployment rollback"), and BOTH records carry the
    // same `RollbackInfo`, so what says this deployment was rolled back is that
    // it is the one that TRIGGERED the rollback — "the deployment ID of the
    // deployment that was underway and triggered a rollback deployment because it
    // failed or was stopped".
    if deployment.rollback_info.as_ref().is_some_and(|info| {
        info.rollback_deployment_id.is_some()
            && info.rollback_triggering_deployment_id.as_deref()
                == Some(deployment.deployment_id.as_str())
    }) {
        reached.push(EVENT_DEPLOYMENT_ROLLBACK);
    }
    match deployment.status.as_str() {
        DEPLOYMENT_STATUS_SUCCEEDED => reached.push(EVENT_DEPLOYMENT_SUCCESS),
        DEPLOYMENT_STATUS_FAILED => reached.push(EVENT_DEPLOYMENT_FAILURE),
        DEPLOYMENT_STATUS_STOPPED => reached.push(EVENT_DEPLOYMENT_STOP),
        _ => {}
    }
    reached
}

/// Every notification one event of `deployment` owes: one per trigger of `group`
/// whose `triggerEvents` name `event` and whose `triggerTargetArn` names an Amazon
/// SNS topic in a Region of this emulator.
///
/// A trigger that selects another event owes nothing, which is what makes the
/// negative half of the contract observable: a topic configured for one event
/// receives no notification of another.
#[must_use]
pub fn owed_for_event(
    group: &DeploymentGroup,
    deployment: &Deployment,
    event: &str,
) -> Vec<PendingNotification> {
    group
        .trigger_configurations
        .iter()
        .filter(|trigger| {
            trigger
                .trigger_events
                .as_ref()
                .is_some_and(|events| events.iter().any(|selected| selected == event))
        })
        .filter_map(|trigger| {
            let topic_arn = addressable_topic(trigger.trigger_target_arn.as_deref(), group)?;
            Some(PendingNotification {
                topic_arn,
                message: deployment_document(
                    deployment,
                    trigger.trigger_name.as_deref().unwrap_or_default(),
                ),
            })
        })
        .collect()
}

/// The one-time verification notification each trigger of `group` owes its topic,
/// at the instant the trigger list is stored.
///
/// It is the only thing there is to tell a topic at that instant — the trigger is
/// configured on it and nothing has happened yet — so, unlike every other
/// notification here, it carries no deployment and is not a JSON document.
#[must_use]
pub fn owed_for_stored_triggers(group: &DeploymentGroup) -> Vec<PendingNotification> {
    group
        .trigger_configurations
        .iter()
        .filter_map(|trigger| {
            let topic_arn = addressable_topic(trigger.trigger_target_arn.as_deref(), group)?;
            Some(PendingNotification {
                topic_arn,
                message: verification_text(
                    trigger.trigger_name.as_deref().unwrap_or_default(),
                    group,
                ),
            })
        })
        .collect()
}

/// The topic a `triggerTargetArn` names, when it names an Amazon SNS topic in a
/// Region of this emulator; `None` for anything else, which is published to
/// nothing.
fn addressable_topic(trigger_target_arn: Option<&str>, group: &DeploymentGroup) -> Option<String> {
    let candidate = trigger_target_arn?;
    if ::service_common::arn::region_of(candidate, SNS_ARN_VENDOR).is_none() {
        ::tracing::debug!(
            application = %group.application_name,
            deployment_group = %group.deployment_group_name,
            "this trigger configuration names no Amazon SNS topic in a Region of this \
             emulator; nothing is published to it"
        );
        return None;
    }
    Some(candidate.to_string())
}

/// The document a subscriber of a configured topic receives for one lifecycle
/// event of one deployment.
///
/// `eventTriggerName` is the trigger CONFIGURATION's own `triggerName` — "the name
/// of the notification trigger" — not the event that was reached, so a subscriber
/// can tell which of its triggers produced the message. Everything else is read
/// off the deployment's stored record, so a notification and a `GetDeployment` of
/// the same instant report one fact.
fn deployment_document(deployment: &Deployment, trigger_name: &str) -> String {
    let mut document = ::serde_json::Map::new();
    insert_text(&mut document, "region", &deployment.region);
    insert_text(&mut document, "accountId", ::engine::emulator_account_id());
    insert_text(&mut document, "eventTriggerName", trigger_name);
    insert_text(&mut document, "applicationName", &deployment.application_name);
    insert_text(&mut document, "deploymentId", &deployment.deployment_id);
    insert_text(
        &mut document,
        "deploymentGroupName",
        deployment.deployment_group_name.as_deref().unwrap_or_default(),
    );
    insert_text(
        &mut document,
        "deploymentConfigName",
        &deployment.deployment_config_name,
    );
    insert_text(&mut document, "createTime", &date_time_text(deployment.create_time_ms));
    insert_text(&mut document, "status", notification_status(&deployment.status));
    // A deployment still in flight has not completed, so there is no instant to
    // name and the member is left out rather than rendered as a null.
    if let Some(complete_time_ms) = deployment.complete_time_ms {
        insert_text(&mut document, "completeTime", &date_time_text(complete_time_ms));
    }
    // Why it failed, when it did: the same `ErrorInformation` the deployment
    // reports to a reader, from the one place it is derived.
    if let Some(error) = super::deployment_info(deployment).error_information {
        let mut rendered = ::serde_json::Map::new();
        insert_text(&mut rendered, "code", error.code.as_deref().unwrap_or_default());
        insert_text(
            &mut rendered,
            "message",
            error.message.as_deref().unwrap_or_default(),
        );
        document.insert(
            "errorInformation".to_string(),
            ::serde_json::Value::Object(rendered),
        );
    }
    ::serde_json::Value::Object(document).to_string()
}

/// The verification notification's body: not a JSON document, because there is no
/// deployment to describe — it names the trigger that was configured, the
/// deployment group and application it was configured on, and the Region they are
/// in.
fn verification_text(trigger_name: &str, group: &DeploymentGroup) -> String {
    format!(
        "This notification was sent to verify the trigger {trigger_name} of the deployment \
         group {} in the application {} in region {}.",
        group.deployment_group_name, group.application_name, group.region
    )
}

/// The `status` a notification reports a deployment in: its own
/// `DeploymentStatus`, in the lower-case form a CodeDeploy notification carries
/// it in.
fn notification_status(status: &str) -> &str {
    match status {
        DEPLOYMENT_STATUS_CREATED => "created",
        DEPLOYMENT_STATUS_IN_PROGRESS => "in_progress",
        DEPLOYMENT_STATUS_SUCCEEDED => "succeeded",
        DEPLOYMENT_STATUS_FAILED => "failed",
        DEPLOYMENT_STATUS_STOPPED => "stopped",
        // Every status this service puts a deployment in is named above; a value
        // from the rest of the modelled enum is reported as it stands rather
        // than mapped to something it is not.
        other => other,
    }
}

/// Write one text member into a rendered document.
fn insert_text(
    document: &mut ::serde_json::Map<String, ::serde_json::Value>,
    key: &str,
    value: &str,
) {
    document.insert(
        key.to_string(),
        ::serde_json::Value::String(value.to_string()),
    );
}

/// The date-time text a notification carries an instant in: RFC 3339 in UTC, the
/// one form this emulator writes into a document it renders itself.
fn date_time_text(epoch_millis: i64) -> String {
    timestamp(epoch_millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// Publish every notification this store has queued, to the topic each one names.
///
/// Call it from an operation that may have queued one — any operation at all, since
/// every pass through the store materialises what the clock owed — and from the
/// deployment clock, with the store UNLOCKED: a publish reaches Amazon SNS's own
/// handler, which takes that service's lock.
pub async fn deliver<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let notifications = state.lock().take_trigger_notifications();
    if notifications.is_empty() {
        return;
    }
    publish(callback, &notifications).await;
}

/// Publish every notification in `notifications` onto the topic it names.
///
/// The client is built once per Region and reused, so a set of notifications
/// spanning one Region builds one client.
async fn publish(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    notifications: &[PendingNotification],
) {
    let mut client: Option<(String, ::aws_sdk_sns::Client)> = None;
    for notification in notifications {
        // Re-read from the ARN rather than carried alongside it, so the Region a
        // publish is addressed in is always one that came out of the ARN under the
        // Region grammar.
        let Some(region) =
            ::service_common::arn::region_of(&notification.topic_arn, SNS_ARN_VENDOR)
        else {
            continue;
        };
        if client
            .as_ref()
            .is_none_or(|(held, _)| held.as_str() != region)
        {
            client = Some((region.to_string(), sns_client(callback, region)));
        }
        let Some((_, sns)) = client.as_ref() else {
            continue;
        };
        let published = sns
            .publish()
            .topic_arn(&notification.topic_arn)
            .message(&notification.message)
            .send()
            .await;
        if let Err(error) = published {
            ::tracing::warn!(
                topic = %notification.topic_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a CodeDeploy trigger notification could not be published to the configured \
                 topic"
            );
        }
    }
}

/// The Amazon SNS client a notification is published through: the IN-PROCESS
/// callback into this same emulator.
///
/// The `endpoint_url` below is an ADDRESS the in-process router reads, never a host
/// any resolver looks up, and `region` is the label read out of the topic's own
/// ARN, so a caller cannot steer that authority. There is nothing to retry, time
/// out, cache an identity for or watch for a stalled stream over that transport —
/// each would need a sleep implementation this client is not given — and the
/// credentials the callback hands over are the emulator's own, which only sign this
/// operation's in-process calls and are never put in a response or a log.
fn sns_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_sns::Client {
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{SNS_ARN_VENDOR}.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}
