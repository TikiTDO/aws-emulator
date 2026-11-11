// SPDX-License-Identifier: CC0-1.0

//! The AWS IoT RULES ENGINE: what a message published to a topic DOES to the topic rules
//! this account holds.
//!
//! A rule's `sql` selects the messages it is about and its `actions` are what AWS IoT then
//! does with each of them, so a message published to a topic a stored rule's `FROM` clause
//! covers is evaluated against that rule and each of its actions is performed. Driven from
//! the data plane's `Publish` ([`dispatch_published_message`]); none of it is reported to the
//! publisher, so what a client observes is the effect on the action's TARGET — the record a
//! `kinesis` action wrote onto its stream.
//!
//! Three gates, in order: the rule's topic filter covers the topic
//! ([`super::topic_filter_matches`], the reading `ListTopicRules` also uses) and the rule is
//! not disabled; the statement SELECTS the message ([`super::rule_sql`], which reads the
//! statement, and [`super::rule_expression`], which evaluates it over the message); and the
//! action's role lets this service make the action's API call
//! ([`super::rule_action_role`]) — a role that does not admit this service principal, or
//! whose documents do not grant that call on that target, is an action DROPPED with nothing
//! written, which is the only observable the permission boundary has.
//!
//! What this engine cannot compute faithfully it does NOT perform: a statement it cannot
//! read, a condition it cannot decide, and an action parameter carrying a substitution
//! template are each left unperformed rather than performed on a guessed value.
//!
//! The action performed today is `kinesis`, "write data to an Amazon Kinesis stream", over
//! the emulator's own in-process transport
//! ([`::service_common::kinesis_client`] — no socket, and no part of the action is a
//! destination), and `http`, "send data to an HTTPS endpoint", whose endpoint is a TOPIC RULE
//! DESTINATION: the message is POSTed to the action's `url` only while a destination that
//! admits that URL is `ENABLED` ([`super::topic_rule_destinations::http_action_is_allowed`]),
//! and nothing is sent otherwise — "the destination must be confirmed prior to use"
//! (`CreateTopicRuleDestination`), which is the gate the whole confirm/enable lifecycle
//! exists for. Every other member of `Action` is a target service with its own semantics (a
//! DynamoDB column per message attribute, a rendered S3 object key, a Lambda invocation) and
//! its own open issue; an action this engine does not perform is left alone rather than
//! approximated.
//!
//! Every call an action makes goes over the emulator's own IN-PROCESS transport — the
//! Kinesis client's and the `http` action's alike — so a stream name or a URL a caller
//! configured reaches only what this emulator serves itself: no socket is opened and no name
//! is resolved.

use super::rule_action_role::{role_bars_action, IamState};
use super::rule_sql::{selected_message, Selected};
use super::topic_rule_destinations::{http_action_is_allowed, http_action_unperformable};
use crate::ops::types::{Action, HttpAction, KinesisAction};

/// The service principal the rules engine acts as: the one an action's role must trust.
pub const SERVICE_PRINCIPAL: &str = "iot.amazonaws.com";

/// The IAM action a `kinesis` rule action performs on the stream it names.
pub const KINESIS_PUT_RECORD_ACTION: &str = "kinesis:PutRecord";

/// Amazon Kinesis's ARN namespace and the resource type of a data stream:
/// `arn:aws:kinesis:<region>:<account>:stream/<streamName>`, the resource an action's role is
/// granted [`KINESIS_PUT_RECORD_ACTION`] on.
const KINESIS_ARN_NAMESPACE: &str = "kinesis";
const STREAM_RESOURCE_TYPE: &str = "stream";

/// How many actions ONE published message performs — the EMULATOR's own ceiling on the
/// fan-out of a single request, which is otherwise the (rules x actions per rule) PRODUCT of
/// a store the caller grows ([`super::MAX_TOPIC_RULES`] rules of at most ten actions each).
/// Far above any rule set a client under test writes; past it the remaining actions of the
/// message are not performed and the engine says so in its log.
const MAX_ACTIONS_PER_MESSAGE: usize = 100;

/// The opening of a substitution template in an action parameter. AWS IoT renders `${…}` from
/// the message; this engine does not compute those expressions, so a parameter carrying one is
/// an action it cannot perform faithfully rather than one to perform on the literal text.
const SUBSTITUTION_TEMPLATE_OPENING: &str = "${";

/// Evaluate the message `payload` published to `topic` against every topic rule of this
/// account, and perform the actions of each rule that selects it.
///
/// Called from the data plane's `Publish` with the SAME shared engine that handler holds, so
/// the rules are the ones `CreateTopicRule` wrote and the enablement the one
/// `EnableTopicRule` / `DisableTopicRule` last left. An engine with no IoT store evaluates
/// nothing.
///
/// The work one call does is bounded: the rules are the ones the store holds (at most
/// [`super::MAX_TOPIC_RULES`]), the actions performed at most [`MAX_ACTIONS_PER_MESSAGE`],
/// the statement read under the bounds [`super::rule_sql`] documents, and the record written
/// is the published payload, which the data plane has already weighed against its own
/// quota.
pub async fn dispatch_published_message<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    topic: &str,
    payload: &[u8],
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    // The IAM store of this same engine, which decides what an action's role may do. `None`
    // is a deployment with no IAM to ask, which bars nothing.
    let iam = super::rule_action_role::iam_state(engine);
    // An action's target is reached in the Region the publish was signed for.
    let region = super::request_region(ctx.parts());
    // PASS 1 — under the guard, the rules this message is about.
    let selecting: Vec<(String, String, Vec<Action>)> = {
        let store = state.lock();
        store
            .topic_rules
            .values()
            .filter(|rule| !rule.rule_disabled)
            .filter(|rule| {
                rule.topic_pattern
                    .as_deref()
                    .is_some_and(|pattern| super::topic_filter_matches(pattern, topic))
            })
            .map(|rule| {
                (
                    rule.rule_name.clone(),
                    rule.sql.clone(),
                    rule.actions.clone(),
                )
            })
            .collect()
    };

    // PASS 2 — with NO guard held: the statement is read and the action's calls are made
    // outside the critical section, so no caller's statement is evaluated under a lock over
    // shared state and no lock is held across an await.
    let mut performed = 0_usize;
    for (rule_name, sql, actions) in selecting {
        // Reading a statement and evaluating it is CPU work on this worker, and a message may
        // select many rules: yielding first gives a cancellation or shutdown signal somewhere
        // to land between rules.
        ::tokio::task::yield_now().await;
        let message = match selected_message(&sql, payload) {
            Selected::Message(message) => message,
            Selected::NotSelected => continue,
            Selected::Undecided => {
                ::tracing::debug!(
                    rule = %rule_name,
                    "a topic rule's statement is not one this emulator's rules engine \
                     evaluates, so its actions were not performed"
                );
                continue;
            }
        };
        for action in &actions {
            let Some(performable) = Performed::of(action) else {
                continue;
            };
            if performed >= MAX_ACTIONS_PER_MESSAGE {
                ::tracing::warn!(
                    rule = %rule_name,
                    performed,
                    "a published message matched more rule actions than this emulator \
                     performs for one message; the rest were not performed"
                );
                return;
            }
            performed += 1;
            performable
                .perform(ctx, &state, &region, iam.as_ref(), &rule_name, &message)
                .await;
        }
    }
}

/// One action of a rule this engine performs, with the parameters it performs it from.
enum Performed<'a> {
    /// `Action$kinesis`: one [`KINESIS_PUT_RECORD_ACTION`] of the selected message onto the
    /// stream the action names, under its partition key.
    Kinesis(&'a KinesisAction),
    /// `Action$http`: one POST of the selected message to the endpoint URL the action names,
    /// made only while a topic rule destination admits that URL.
    Http(&'a HttpAction),
}

impl<'a> Performed<'a> {
    /// The action `action` carries, or `None` when it carries none this engine performs.
    /// `Action` is a structure of optional members, one per target service, of which a rule
    /// sets the one it performs; the members this engine leaves alone are named in the module
    /// documentation.
    fn of(action: &'a Action) -> Option<Self> {
        action
            .kinesis
            .as_ref()
            .map(Performed::Kinesis)
            .or_else(|| action.http.as_ref().map(Performed::Http))
    }

    /// Perform the action for one selected message, or drop it with nothing written.
    async fn perform(
        self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        state: &::engine::ServiceState<super::ComAmazonawsIotAwsiotserviceState>,
        region: &str,
        iam: Option<&::engine::ServiceState<IamState>>,
        rule_name: &str,
        message: &[u8],
    ) {
        match self {
            Self::Kinesis(action) => {
                put_record(ctx, region, iam, rule_name, action, message).await
            }
            Self::Http(action) => post_to_endpoint(ctx, state, rule_name, action, message).await,
        }
    }
}

/// POST one selected message to the endpoint an `http` action names, if a topic rule
/// destination admits that URL.
///
/// The GATE is the whole point of a topic rule destination: "the destination must be
/// confirmed prior to use" (`CreateTopicRuleDestination`), and traffic is allowed only while
/// a destination that admits the action's URL is `ENABLED`
/// ([`super::topic_rule_destinations::http_action_is_allowed`]) — so an action whose endpoint
/// has not been confirmed and enabled delivers NOTHING, which is the only observable that
/// guarantee has. The verdict is read under the store's guard and the guard is released
/// before the POST: no lock is held across an await.
///
/// Nothing is delivered either for an action this engine does not perform at all
/// ([`super::topic_rule_destinations::http_action_unperformable`]), and an endpoint that
/// refuses the request or is not there is a message dropped — which AWS does not surface to
/// the publisher either.
///
/// The request is made over the emulator this publish is being served by and nothing else
/// ([`super::topic_rule_destinations::InProcessTransport`]): the action's URL is matched
/// against the services this emulator serves ITSELF, so no socket is opened, no host name is
/// resolved and no address off this process is reachable.
async fn post_to_endpoint(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    state: &::engine::ServiceState<super::ComAmazonawsIotAwsiotserviceState>,
    rule_name: &str,
    action: &HttpAction,
    message: &[u8],
) {
    if let Some(reason) = http_action_unperformable(action) {
        ::tracing::debug!(
            rule = %rule_name,
            reason,
            "an http rule action is not one this emulator's rules engine performs, so \
             nothing was delivered to its endpoint"
        );
        return;
    }
    let allowed = {
        let mut store = state.lock();
        // A destination the clock has already carried out of `IN_PROGRESS` is read at the
        // status it rests at, which is the status the gate is about.
        super::settle_destinations(
            &mut store.topic_rule_destinations,
            ::std::time::SystemTime::now(),
        );
        http_action_is_allowed(&store.topic_rule_destinations, action)
    };
    if !allowed {
        ::tracing::debug!(
            rule = %rule_name,
            "an http rule action's endpoint has no enabled topic rule destination, so \
             nothing was delivered to it"
        );
        return;
    }
    let delivered = super::topic_rule_destinations::deliver_to_endpoint(
        &super::topic_rule_destinations::InProcessTransport::of(ctx.callback()),
        action,
        message,
    )
    .await;
    if let Err(reason) = delivered {
        ::tracing::warn!(
            rule = %rule_name,
            reason = %reason,
            "an http rule action's message was not delivered to its endpoint"
        );
    }
}

/// Write one Kinesis record for the selected message, under the action's role: the record's
/// data is the message the statement selected, its key is [`partition_key`], and the stream is
/// the `streamName` the action names in this publish's Region — reached through the shared
/// in-process client, so the write never leaves this process.
///
/// Nothing is written when the role bars the call, and nothing when Kinesis refuses the record
/// (a stream the account does not hold, one not yet active) — that refusal is the target
/// service's answer about its own state, which AWS does not surface to the publisher either.
async fn put_record(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    iam: Option<&::engine::ServiceState<IamState>>,
    rule_name: &str,
    action: &KinesisAction,
    message: &[u8],
) {
    let stream_arn = ::emulator_server::emulator_arn(
        KINESIS_ARN_NAMESPACE,
        region,
        &format!("{STREAM_RESOURCE_TYPE}/{}", action.stream_name),
    );
    // The verdict is read under the IAM guard and the guard is released before the write: no
    // lock is held across an await.
    let barred = iam.is_some_and(|iam| {
        role_bars_action(
            &iam.lock(),
            &action.role_arn,
            SERVICE_PRINCIPAL,
            KINESIS_PUT_RECORD_ACTION,
            &stream_arn,
        )
    });
    if barred {
        ::tracing::debug!(
            rule = %rule_name,
            "a kinesis rule action's role does not grant the stream write, so no record \
             was written"
        );
        return;
    }
    let Some(partition_key) = partition_key(action) else {
        ::tracing::debug!(
            rule = %rule_name,
            "a kinesis rule action's partitionKey is not one this emulator's rules engine \
             renders as the value the action means, so no record was written"
        );
        return;
    };
    let written = ::service_common::kinesis_client::kinesis_client(ctx, region)
        .put_record()
        .stream_name(&action.stream_name)
        .data(::aws_sdk_kinesis::primitives::Blob::new(message))
        .partition_key(partition_key)
        .send()
        .await;
    if let Err(error) = written {
        ::tracing::warn!(
            rule = %rule_name,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "a kinesis rule action's record was not written"
        );
    }
}

/// The partition key one record is written under: "the partition key" the action names, or a
/// key unique to this record when the action names NONE — the key decides the shard and
/// Kinesis takes no record without one, so each record of an action naming none gets its own,
/// which spreads them over the stream's shards.
///
/// `None` — an action this engine does not perform — for a key it cannot render as the value
/// the action means: one carrying a substitution template
/// ([`SUBSTITUTION_TEMPLATE_OPENING`]), whose value is computed from the message, and an EMPTY
/// one, which `PartitionKey` (`length 1..256`) is not a value of, so Kinesis would refuse the
/// record rather than take it under a key this engine chose.
fn partition_key(action: &KinesisAction) -> Option<String> {
    match action.partition_key.as_deref() {
        Some(key) if key.contains(SUBSTITUTION_TEMPLATE_OPENING) || key.is_empty() => None,
        Some(key) => Some(key.to_string()),
        None => Some(super::fresh_uuid()),
    }
}

