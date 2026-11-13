// SPDX-License-Identifier: CC0-1.0

//! The EventBridge event a statement publishes when its submission asked to be
//! told how it ended.
//!
//! # The rule this module implements
//!
//! A submit may ask for an event instead of being polled for: `WithEvent` is "a
//! value that indicates whether to send an event to the Amazon EventBridge event
//! bus after the SQL statement runs"
//! (`smithy_query(query="doc",shape="ExecuteStatementInput",member="WithEvent")`).
//! So a submission that asked for one has the status change it makes when it stops
//! published onto the account's default event bus ([`DEFAULT_EVENT_BUS`]) under the
//! `source` this service owns ([`EVENT_SOURCE`]) and the `detail-type` of a
//! statement's status change ([`DETAIL_TYPE`]), naming the warehouse the statement
//! ran on in `resources` and carrying the statement's own identity and outcome in
//! `detail`. A rule on that bus whose `EventPattern` matches then delivers it to
//! the rule's targets, so a client driven by the event observes the statement
//! without ever calling this service again.
//!
//! Accepting the member and publishing nothing is the one thing that must not
//! happen: it leaves such a client waiting for ever on an event that was asked for
//! and never sent. A submission that asked for NOTHING publishes nothing, so the
//! member is read off the stored record and nowhere else
//! ([`crate::state::Statement::with_event`]).
//!
//! The envelope and the `detail` member names are not in any Smithy model —
//! EventBridge events never are — so they are stated once here, and every value in
//! them is read off the statement's own record.
//!
//! # How a status change becomes an event
//!
//! Nothing here decides WHEN a statement stops: its own execution does, and the
//! two places that record a terminal status queue the event for it
//! ([`crate::state::ComAmazonawsRedshiftdataRedshiftdataState::complete`] for the
//! outcome an execution reported, and that store's `cancel` for a statement the
//! caller stopped), so one status change is queued exactly once and the queue is
//! bounded ([`MAX_PENDING_EVENTS`], oldest dropped first).
//!
//! A queued event is published with NO store lock held ([`publish_pending`]):
//! EventBridge is reached the way every emulated service reaches another, through
//! the framework's in-process callback, so this crate names no other service's
//! crate. Publication is BEST EFFORT — an emulator with no EventBridge mounted has
//! nowhere to publish — so a failure is logged and the statement's own record
//! stands.
//!
//! WHICH call publishes: the task that runs the statement, as soon as it has
//! recorded the outcome, so the event arrives whether or not anybody polls; and
//! the cancel that stopped a running statement, for the same reason.
//!
//! A BATCH is one submission holding one `Id` — the `Id` its caller was answered
//! with and correlates by — so its status change is announced once under that
//! `Id`, rather than once per statement of it.

use std::time::{SystemTime, UNIX_EPOCH};

use ::smithy_server_runtime::EmbeddedEmulator;

use super::Statement;

/// The `source` of every event this service publishes.
///
/// EventBridge names the publishing service in an event's `source` as
/// `aws.<service>`, and this service's own name on the wire is `redshift-data`
/// (`smithy_query(query="service_info")`: its `sdkId` and `endpointPrefix`).
pub const EVENT_SOURCE: &str = "aws.redshift-data";

/// The `detail-type` a statement's status change is published under — the value a
/// rule's `EventPattern` matches on.
pub const DETAIL_TYPE: &str = "Redshift Data Statement Status Change";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus,
/// which is the bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// How many statement events the queue holds at once.
///
/// This module's OWN ceiling, not an AWS quota: the queue lives in memory and
/// every status change of every statement that asked for an event appends to it,
/// so without a bound a caller submitting without end could grow it without end.
/// The oldest event is dropped first, so the status changes the newest statements
/// just made are the ones that survive. It is far above what a realistic burst
/// produces, because a submission contributes one event and this service bounds
/// how many submissions are in flight at once
/// ([`crate::state::MAX_ACTIVE_STATEMENTS`]).
pub const MAX_PENDING_EVENTS: usize = 2_000;

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList`
/// is `@length(min: 1, max: 10)`, so a drain holding more events than that is
/// published in several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many events ONE drain publishes.
///
/// The queue itself is bounded ([`MAX_PENDING_EVENTS`]), but one call draining all
/// of it would fan out to a rule's targets that many times: this caps one drain,
/// and [`publish_pending`] repeats a drain until the queue is empty, so nothing
/// waits for a later call.
const MAX_PUBLISHED_PER_DRAIN: usize = 50;

/// How many drains ONE call makes.
///
/// A call publishes at most a FULL QUEUE's worth and then returns, so the work one
/// call does is bounded even while other statements are still stopping and
/// appending: what arrives after it started is left to the next drain, which every
/// terminal transition makes. Without this the call-level work would be unbounded
/// however tightly one drain is capped.
const MAX_DRAINS_PER_CALL: usize = MAX_PENDING_EVENTS.div_ceil(MAX_PUBLISHED_PER_DRAIN);

/// The IAM principal an event names as the identity that submitted the statement.
///
/// This emulator publishes exactly one long-term credential, which speaks for the
/// account's single IAM user — the identity STS reports as the caller and IAM holds
/// among the account's users — so that user's ARN is who a submitted statement
/// belongs to. Stated through the one definition of it
/// ([`service_common::caller_principal`]) rather than built here, so the principal
/// an event names is the principal every other service reports for the same call.
#[must_use]
pub fn submitting_principal() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// One statement event waiting to be published: WHICH statement, WHAT it ended as,
/// WHEN that happened, and the Region and warehouse it happened on.
///
/// Self-contained on purpose — everything the event carries is taken when the
/// status change is queued, so a drain does not depend on the statement still
/// being retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementEvent {
    /// The `Id` the submit answered with, which is what a client correlates the
    /// event with the statement it submitted by.
    pub statement_id: String,
    /// The `StatementName` the submission carried, when it carried one.
    pub statement_name: Option<String>,
    /// The status the statement stopped at.
    pub state: String,
    /// Why it stopped, for a statement that stopped WITHOUT finishing: the
    /// backend's own reason, exactly as the record reports it.
    pub error: Option<String>,
    /// The IAM principal that submitted the statement ([`submitting_principal`]).
    pub principal: String,
    /// The ARN of the warehouse the statement ran on, when this service could
    /// name it — the envelope's `resources` entry.
    pub resource_arn: Option<String>,
    /// WHEN the statement stopped, which is the instant the event reports.
    pub at: SystemTime,
    /// The Region the statement was submitted in: an event is published in the
    /// Region the work happened in.
    pub region: String,
}

impl StatementEvent {
    /// The event reporting the status change `statement` has just made, taken off
    /// the record itself so the event and the statement cannot disagree.
    pub(super) fn of(statement: &Statement) -> Self {
        Self {
            statement_id: statement.id.clone(),
            statement_name: statement.statement_name.clone(),
            state: statement.status.as_str().to_string(),
            error: statement.error(),
            principal: statement.principal.clone(),
            resource_arn: statement.resource_arn.clone(),
            at: statement.completed_at,
            region: statement.region.clone(),
        }
    }

    /// The event's `detail`: which statement this is about, what the submission
    /// called it, what it ended as, who submitted it — and, for a statement that
    /// stopped without finishing, the reason the record carries.
    ///
    /// A member the record cannot ground is LEFT OUT rather than written as null:
    /// a statement submitted without a name has no name, and one that finished has
    /// no error.
    fn detail(&self) -> ::serde_json::Value {
        let mut detail = ::serde_json::Map::new();
        detail.insert(
            "statementId".to_string(),
            ::serde_json::Value::String(self.statement_id.clone()),
        );
        if let Some(name) = &self.statement_name {
            detail.insert(
                "statementName".to_string(),
                ::serde_json::Value::String(name.clone()),
            );
        }
        detail.insert(
            "state".to_string(),
            ::serde_json::Value::String(self.state.clone()),
        );
        detail.insert(
            "principal".to_string(),
            ::serde_json::Value::String(self.principal.clone()),
        );
        if let Some(error) = &self.error {
            detail.insert(
                "errorMessage".to_string(),
                ::serde_json::Value::String(error.clone()),
            );
        }
        ::serde_json::Value::Object(detail)
    }
}

/// `at` as the epoch milliseconds the event's `time` carries.
fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or_default()
}

/// Publish every statement event this store has queued onto the account's default
/// EventBridge bus, and leave the queue empty.
///
/// Called with NO store lock held: each drain takes its events out in one critical
/// section, so nothing is published twice, and the in-process `PutEvents` runs
/// outside it. The queue is drained until it is EMPTY — one drain is capped
/// ([`MAX_PUBLISHED_PER_DRAIN`]) and the number of drains is capped
/// ([`MAX_DRAINS_PER_CALL`]), so no single call is unbounded, and repeating the
/// drain is what guarantees the tail of a burst is published by the call that
/// produced it rather than waiting for a later one. Nothing about the answer an
/// operation gives changes either way.
pub async fn publish_pending<Eng: Send + Sync + 'static>(
    callback: &EmbeddedEmulator,
    engine: &Eng,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    for _drain in 0..MAX_DRAINS_PER_CALL {
        let events = state
            .lock()
            .take_statement_events(MAX_PUBLISHED_PER_DRAIN);
        if events.is_empty() {
            return;
        }
        publish_batch(callback, &events).await;
    }
}

/// Publish one drain, in `PutEvents` requests of at most [`MAX_PUT_EVENTS_ENTRIES`]
/// entries, one request per Region the drained events belong to.
async fn publish_batch(callback: &EmbeddedEmulator, events: &[StatementEvent]) {
    for window in events.chunks(MAX_PUT_EVENTS_ENTRIES) {
        for region in regions_of(window) {
            let client = ::service_common::eventbridge_client::eventbridge_client(callback, &region);
            let mut request = client.put_events();
            for event in window.iter().filter(|event| event.region == region) {
                let mut entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                    .event_bus_name(DEFAULT_EVENT_BUS)
                    .source(EVENT_SOURCE)
                    .detail_type(DETAIL_TYPE)
                    // The event's `time` is WHEN the statement stopped, not when
                    // the queue was drained: `PutEventsRequestEntry$Time` is "the
                    // time stamp of the event, per RFC3339. If no time stamp is
                    // provided, the time stamp of the PutEvents call is used."
                    .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                        epoch_millis(event.at),
                    ))
                    .detail(event.detail().to_string());
                if let Some(resource_arn) = &event.resource_arn {
                    entry = entry.resources(resource_arn);
                }
                request = request.entries(entry.build());
            }
            match request.send().await {
                Err(error) => ::tracing::warn!(
                    region = %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a statement status change could not be published to EventBridge"
                ),
                // The bus answers one outcome PER ENTRY, so a request it
                // accepted can still have refused some of the events in it. An
                // event nobody will ever receive is exactly what this module
                // exists to prevent, so a refused entry is said out loud —
                // naming the statements whose status change did not land —
                // rather than passing as a published one.
                Ok(answered) if answered.failed_entry_count() > 0 => {
                    ::tracing::warn!(
                        region = %region,
                        refused = answered.failed_entry_count(),
                        reasons = ?answered
                            .entries()
                            .iter()
                            .filter_map(|result| {
                                result.error_code().map(|code| {
                                    (code.to_string(), result.error_message().map(str::to_string))
                                })
                            })
                            .collect::<Vec<_>>(),
                        statements = ?window
                            .iter()
                            .filter(|event| event.region == region)
                            .map(|event| event.statement_id.clone())
                            .collect::<Vec<_>>(),
                        "EventBridge refused a statement status change, so nothing was published \
                         for it"
                    );
                }
                Ok(_published) => {}
            }
        }
    }
}

/// The Regions the events of one window belong to, each once and in the order they
/// first appear: an event is published in the Region the statement ran in, so a
/// window holding two Regions' statements is published to both buses.
fn regions_of(window: &[StatementEvent]) -> Vec<String> {
    let mut regions: Vec<String> = Vec::new();
    for event in window {
        if !regions.iter().any(|region| region == &event.region) {
            regions.push(event.region.clone());
        }
    }
    regions
}
