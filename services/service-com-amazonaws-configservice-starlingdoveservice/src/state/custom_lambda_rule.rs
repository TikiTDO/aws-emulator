// SPDX-License-Identifier: CC0-1.0

//! What Config does for a rule it does NOT evaluate itself: it invokes the evaluator the
//! account pointed the rule at, and that evaluator delivers the verdict back.
//!
//! A Config Custom Lambda rule names a Lambda function as its `SourceIdentifier`, and
//! that function "is required in every Lambda function that is invoked by an Config rule"
//! to report what it found with `PutEvaluations` (`PutEvaluations`). So the invocation is
//! the half Config owes: it hands the function the event describing what to evaluate and
//! the `ResultToken` the delivery comes back on
//! ([`super::ConfigRuleEntry::mint_evaluation_result_token`]), and the verdict itself is
//! the function's own — nothing here computes one.
//!
//! An invocation is OWED for the evaluation pass Config invoked — the pass a write of the
//! rule starts ("when you create a rule, Config evaluates your resources against the rule
//! automatically", `PutConfigRule`) and the pass `StartConfigRulesEvaluation` asks for
//! ("re-runs an evaluation against the last known state of your resources") — and it is
//! owed ONCE per pass: the pass's own token is the record of whether its evaluator has
//! been invoked yet ([`super::RuleEvaluationEntry::dispatched_token`]), so a pump that
//! runs every few hundred milliseconds does not invoke the function again and again for
//! one pass.
//!
//! WHICH event the function is handed is the rule's own trigger
//! (`SourceDetail$MessageType`): a periodic rule is invoked with the scheduled
//! notification, and a change-triggered one once per configuration item in its scope with
//! that item's own change notification — the same notification body a delivery channel
//! streams ([`super::change_notification`]), so there is one statement of what a
//! configuration-item event looks like.
//!
//! The function is invoked over the in-process callback, exactly as every other
//! cross-service call of this crate is made, so this crate names no other service crate.
//! It is the function's own run that writes anything: an invocation that could not be made
//! is logged and leaves the rule unevaluated rather than recording a verdict nothing
//! produced.

use std::time::SystemTime;

use super::{
    service_state, ComAmazonawsConfigserviceStarlingdoveserviceState, ConfigRuleEntry,
    CONFIG_RULE_STATE_ACTIVE,
};

/// The `messageType` of the event Config hands a PERIODIC rule's function (the AWS Config
/// Developer Guide's *Example event for periodic rules*), which is also the
/// `SourceDetail$MessageType` such a rule declares.
pub const SCHEDULED_NOTIFICATION_MESSAGE_TYPE: &str = "ScheduledNotification";

/// The `version` of the invocation event Config hands a rule's function (the same guide's
/// example events).
const INVOCATION_EVENT_VERSION: &str = "1.0";

/// The `recordVersion` the scheduled notification inside that event carries.
const SCHEDULED_NOTIFICATION_RECORD_VERSION: &str = "1.0";

/// The most functions ONE dispatch invokes for a single change-triggered rule.
///
/// A HOST bound: a change-triggered rule is invoked per configuration item in its scope,
/// and the inventory is bounded, so one owed pass cannot turn into an unbounded number of
/// in-process invocations.
const MAX_INVOCATIONS_PER_PASS: usize = 50;

impl ConfigRuleEntry {
    /// Whether Config still owes the invocation of THIS rule's own evaluator: the rule
    /// delivers its own verdicts ([`ConfigRuleEntry::delivers_own_evaluations`]), Config
    /// has invoked an evaluation pass for it (so the pass has a `ResultToken`), and that
    /// pass's evaluator has not been invoked yet.
    pub fn owes_evaluator_invocation(&self) -> bool {
        self.delivers_own_evaluations()
            && self.evaluation.result_token.is_some()
            && self.evaluation.result_token != self.evaluation.dispatched_token
    }

    /// Whether this rule is triggered by a CONFIGURATION CHANGE rather than periodically:
    /// one of its `SourceDetails` asks for the configuration-item notification
    /// (`SourceDetail$MessageType`: "ConfigurationItemChangeNotification - Triggers an
    /// evaluation when Config delivers a configuration item as a result of a resource
    /// change").
    ///
    /// A rule that declares no source details at all is PERIODIC here: this emulator has
    /// no configuration-item delivery to trigger off for it, so it is invoked with the
    /// scheduled notification rather than not at all.
    pub(super) fn is_change_triggered(&self) -> bool {
        self.source
            .source_details
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|detail| {
                detail.message_type.as_deref()
                    == Some(super::CONFIGURATION_ITEM_CHANGE_MESSAGE_TYPE)
            })
    }
}

/// One invocation of one rule's evaluator that Config owes.
#[derive(Debug, Clone)]
pub struct EvaluatorInvocation {
    /// The function the rule names as its `SourceIdentifier`, which is what the
    /// invocation addresses.
    pub function: String,
    /// The rule the invocation is about, for the log of an invocation that failed.
    pub config_rule_name: String,
    /// The event the function is handed, as the function's own handler reads it.
    pub event: ::serde_json::Value,
}

/// One recorded resource a change-triggered rule may be invoked for: what identifies it,
/// the tags its item was recorded with (which the rule's scope can narrow by) and the
/// configuration-item change notification its evaluator is handed.
struct RecordedSubject {
    resource_type: String,
    resource_id: String,
    tags: ::std::collections::BTreeMap<String, String>,
    notification: ::serde_json::Value,
}

impl ComAmazonawsConfigserviceStarlingdoveserviceState {
    /// Whether any rule that owes its evaluator an invocation is a CHANGE-TRIGGERED one,
    /// i.e. whether the recorded inventory has to be read to build those invocations at
    /// all.
    fn owes_change_triggered_invocation(&self) -> bool {
        self.config_rules.values().any(|rule| {
            rule.state == CONFIG_RULE_STATE_ACTIVE
                && rule.owes_evaluator_invocation()
                && rule.is_change_triggered()
        })
    }

    /// Note that the configuration items `recorded` TRIGGER the rules they are in the
    /// scope of, so Config owes those rules' evaluators an invocation.
    ///
    /// A change-triggered rule is evaluated "when Config delivers a configuration item as
    /// a result of a resource change" (`SourceDetail$MessageType`), so what invokes such a
    /// rule's evaluator is the recorded item, not a pass of the clock: a rule written
    /// before the account held anything in its scope is invoked when the first item in
    /// that scope is recorded, and a rule nothing in the pass touched is not invoked again.
    pub fn owe_change_triggered_invocations(
        &mut self,
        recorded: &[super::RecordedItemChange],
        now: SystemTime,
    ) {
        if recorded.is_empty() {
            return;
        }
        // What identifies each recorded resource, and the tags its item was recorded
        // with, read out of the inventory before the rules are walked mutably.
        let subjects: Vec<(String, String, ::std::collections::BTreeMap<String, String>)> = recorded
            .iter()
            .filter_map(|change| {
                let entry = self
                    .discovered_resources
                    .get(&(change.resource_type.clone(), change.resource_id.clone()))?;
                Some((
                    entry.resource_type.clone(),
                    entry.resource_id.clone(),
                    entry.tags.clone(),
                ))
            })
            .collect();
        for rule in self.config_rules.values_mut() {
            if rule.state != CONFIG_RULE_STATE_ACTIVE
                || !rule.delivers_own_evaluations()
                || !rule.is_change_triggered()
            {
                continue;
            }
            let triggered = subjects.iter().any(|(resource_type, resource_id, tags)| {
                super::scope_covers(rule.scope.as_ref(), resource_type, resource_id, tags)
            });
            if triggered {
                // The same invocation a write of the rule owes: Config has an evaluation
                // to run, and the token it mints is the one the delivery comes back on.
                rule.evaluation.invoke(now);
                rule.mint_evaluation_result_token(now);
            }
        }
    }

    /// Take the evaluator invocations this account owes as of `now`, marking each rule's
    /// pass as dispatched so one pass is invoked once.
    ///
    /// Read under the store's guard and performed with it released, because the function
    /// an invocation runs calls back into this same emulator — which is also why the
    /// marking happens HERE rather than after the call: a second pump that takes the
    /// invocations finds nothing left to make.
    pub fn take_due_evaluator_invocations(
        &mut self,
        region: &str,
        now: SystemTime,
    ) -> Vec<EvaluatorInvocation> {
        // The configuration items a change-triggered rule is invoked for, copied out so
        // the rules below can be walked mutably. A resource whose deletion is recorded is
        // no longer there to evaluate. Rendering one item's notification is the cost of
        // one invocation, so the inventory is read only when a rule actually owes a
        // change-triggered invocation: a pass that owes none — which is every pass of an
        // account holding no such rule — reads nothing at all.
        let recorded: Vec<RecordedSubject> = if self.owes_change_triggered_invocation() {
            self.discovered_resources
                .values()
                .filter(|entry| entry.deleted_at.is_none())
                .map(|entry| {
                    let item = entry.to_configuration_item(
                        region,
                        self.recording_frequency_of(&entry.resource_type),
                    );
                    RecordedSubject {
                        resource_type: entry.resource_type.clone(),
                        resource_id: entry.resource_id.clone(),
                        tags: entry.tags.clone(),
                        // The event a change-triggered rule is handed IS the notification
                        // the recorded item produced, built through the one place such a
                        // notification is rendered. The item's own status says which change
                        // it records, so the diff names the change the item is rather than
                        // one chosen here; which PROPERTIES changed is reported by the
                        // delivery channel's stream, and Config's invocation carries the
                        // item.
                        notification: super::configuration_item_change_notification(
                            &super::delivered_configuration_item(&item),
                            recorded_change_type(entry.configuration_item_status()),
                            ::serde_json::Map::new(),
                            now,
                        ),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        let account_id = ::emulator_server::emulator_account_id();
        let mut invocations = Vec::new();
        for rule in self.config_rules.values_mut() {
            if rule.state != CONFIG_RULE_STATE_ACTIVE || !rule.owes_evaluator_invocation() {
                continue;
            }
            // The function the rule points at: a Custom Lambda rule's `SourceIdentifier`
            // is "the Amazon Resource Name (ARN) of the Lambda function". A rule naming
            // none names no evaluator, so there is nothing to invoke.
            let Some(function) = rule
                .source
                .source_identifier
                .as_deref()
                .map(str::trim)
                .filter(|identifier| !identifier.is_empty())
            else {
                continue;
            };
            let Some(result_token) = rule.evaluation.result_token.clone() else {
                continue;
            };
            let events: Vec<::serde_json::Value> = if rule.is_change_triggered() {
                recorded
                    .iter()
                    .filter(|subject| {
                        super::scope_covers(
                            rule.scope.as_ref(),
                            &subject.resource_type,
                            &subject.resource_id,
                            &subject.tags,
                        )
                    })
                    .take(MAX_INVOCATIONS_PER_PASS)
                    .map(|subject| subject.notification.clone())
                    .collect()
            } else {
                vec![scheduled_notification(account_id, now)]
            };
            for invoking_event in events {
                invocations.push(EvaluatorInvocation {
                    function: function.to_string(),
                    config_rule_name: rule.name.clone(),
                    event: invocation_event(rule, &invoking_event, &result_token, account_id),
                });
            }
            // The pass's evaluator has now been invoked, whatever the function answers:
            // the verdict is the function's to deliver, and Config does not invoke it
            // twice for one pass.
            rule.evaluation.dispatched_token = Some(result_token);
        }
        invocations
    }
}

/// The `configurationItemDiff$changeType` of a recorded item, read off the
/// `configurationItemStatus` the item itself reports: the resource's discovery, its
/// deletion, or a configuration that has been updated.
fn recorded_change_type(configuration_item_status: &str) -> &'static str {
    match configuration_item_status {
        super::CONFIGURATION_ITEM_STATUS_RESOURCE_DISCOVERED => super::CHANGE_TYPE_CREATE,
        super::CONFIGURATION_ITEM_STATUS_RESOURCE_DELETED => super::CHANGE_TYPE_DELETE,
        _ => super::CHANGE_TYPE_UPDATE,
    }
}

/// The `invokingEvent` a PERIODIC rule's function is handed: "the notification that
/// triggered the evaluation", which for such a rule is the scheduled one.
fn scheduled_notification(account_id: &str, now: SystemTime) -> ::serde_json::Value {
    ::serde_json::json!({
        "awsAccountId": account_id,
        "notificationCreationTime": super::event_instant(now),
        "messageType": SCHEDULED_NOTIFICATION_MESSAGE_TYPE,
        "recordVersion": SCHEDULED_NOTIFICATION_RECORD_VERSION,
    })
}

/// The event Config hands a rule's function, with the members and the spellings the AWS
/// Config Developer Guide's example events carry.
///
/// `invokingEvent` and `ruleParameters` travel as JSON-encoded STRINGS, which is why a
/// function's handler parses them (`json.loads(event["invokingEvent"])`), and
/// `resultToken` is the token the delivery comes back on. `eventLeftScope` is "whether the
/// Amazon Web Services resource to be evaluated has been removed from the rule's scope",
/// which is false for every invocation Config makes here: the items an invocation carries
/// are the ones the inventory holds.
fn invocation_event(
    rule: &ConfigRuleEntry,
    invoking_event: &::serde_json::Value,
    result_token: &str,
    account_id: &str,
) -> ::serde_json::Value {
    ::serde_json::json!({
        "invokingEvent": invoking_event.to_string(),
        "ruleParameters": rule.input_parameters.clone(),
        "resultToken": result_token,
        "eventLeftScope": false,
        "configRuleArn": rule.arn.clone(),
        "configRuleName": rule.name.clone(),
        "configRuleId": rule.id.clone(),
        "accountId": account_id,
        "version": INVOCATION_EVENT_VERSION,
    })
}

/// Invoke the evaluators this account owes an invocation of, as of now.
///
/// Takes the callback rather than an `OperationContext`, because the pump that calls it
/// outlives the request that started it ([`super::drive_recorder_clock`]). No lock is held
/// across an await: the invocations are decided under the store's guard and made with it
/// released, because the function they run calls back into this same emulator — including
/// into this service, which is how its verdict is delivered.
pub async fn invoke_due_rule_evaluators<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
    region: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let invocations = state
        .lock()
        .take_due_evaluator_invocations(region, SystemTime::now());
    if invocations.is_empty() {
        return;
    }
    let client = lambda_client(callback, region);
    for invocation in invocations {
        // "RequestResponse": Config waits for the function, because the verdict is
        // delivered by the function's own run.
        let answered = client
            .invoke()
            .function_name(&invocation.function)
            .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
            .payload(::aws_sdk_lambda::primitives::Blob::new(
                invocation.event.to_string().into_bytes(),
            ))
            .send()
            .await;
        match answered {
            Ok(answer) => {
                // "FunctionError: if present, indicates that an error occurred during
                // function execution": the evaluator ran and failed, so it delivered no
                // verdict and Config records none of its own.
                if let Some(function_error) = answer.function_error() {
                    ::tracing::warn!(
                        config_rule_name = %invocation.config_rule_name,
                        function_error = %function_error,
                        "the evaluator of a Config rule failed"
                    );
                }
            }
            Err(error) => ::tracing::warn!(
                config_rule_name = %invocation.config_rule_name,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the evaluator of a Config rule could not be invoked"
            ),
        }
    }
}

/// An AWS Lambda client over the in-process callback, built the way this crate's other
/// cross-service clients are: the emulator supplies the transport, so there is nothing to
/// retry, time out, cache an identity for or watch for a stalled stream.
fn lambda_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}
