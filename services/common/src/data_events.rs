// SPDX-License-Identifier: CC0-1.0

//! The DATA-event leg of CloudTrail delivery: a call on the CONTENTS of a resource,
//! delivered as a log file into the bucket of every trail whose event selectors admit it.
//!
//! An account's audit trail has two classes of API-call event, and they are delivered
//! differently. A MANAGEMENT event is the control-plane call of a service: it is recorded in
//! event history, where `LookupEvents` reports it, published onto the default event bus, and
//! written into every logging trail's bucket ([`crate::management_events`],
//! [`crate::api_call_events`], [`crate::trail_log_files`]). A DATA event is a call on the
//! contents of one resource — an object read, a token exchanged against an identity pool —
//! and it is:
//!
//! * NOT event history: "data events are not logged by default when you create a trail", and
//!   `LookupEvents` serves management and Insights events, so no data event is recorded in
//!   the log at all;
//! * NOT on the `AWS API Call via CloudTrail` channel, which carries management events; and
//! * delivered to a trail's BUCKET only while one of that trail's event selectors admits it,
//!   which is stated per resource type rather than by a flag — the very thing an advanced
//!   event selector's `resources.type` field names.
//!
//! So this module is two things: the SELECTOR — the projection of one trail's advanced event
//! selector, and the one match of a rendered event record against it ([`DataEventSelector`])
//! — and the SEAM a serving service records a data event through
//! ([`hold_data_event`] and [`DeliveringDataEvents`]).
//!
//! # Why the seam is a wrapper
//!
//! A record names the request its caller was answered under (`requestID`), and that id
//! exists only once the response has been rendered — after the handler has returned. So the
//! handler BUILDS the record and holds it ([`hold_data_event`]), and the wrapper the service
//! serves the operation behind takes it, asks the account's trails what they deliver for it
//! ([`DataEventDelivery`]) and writes the log files, outside every lock and after the
//! caller's own answer has been rendered. Delivery is BEST EFFORT, as every other delivery
//! in this tree: a caller is never failed because a log file could not be written.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use crate::management_events::ServedApiCall;
use crate::query_error_envelope::named_request;
use crate::trail_log_files::DeliveredLogFile;

/// One field selector of an advanced event selector, as `AdvancedFieldSelector` carries it:
/// the event field it is about, and the operands of each operator it states.
///
/// The operators are the model's own (`AdvancedFieldSelector`: `Equals`, `StartsWith`,
/// `EndsWith`, `NotEquals`, `NotStartsWith`, `NotEndsWith`), each a list of values. An
/// operator the selector does not state carries no operand and requires nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataEventFieldSelector {
    /// The field of the event record this selector is about, as the model spells it
    /// (`eventCategory`, `resources.type`, `readOnly`, `eventName`, `eventSource`,
    /// `resources.ARN`).
    pub field: String,
    /// `Equals`: a value of the field must be one of these.
    pub equals: Vec<String>,
    /// `StartsWith`: a value of the field must begin with one of these.
    pub starts_with: Vec<String>,
    /// `EndsWith`: a value of the field must end with one of these.
    pub ends_with: Vec<String>,
    /// `NotEquals`: no value of the field may be one of these.
    pub not_equals: Vec<String>,
    /// `NotStartsWith`: no value of the field may begin with one of these.
    pub not_starts_with: Vec<String>,
    /// `NotEndsWith`: no value of the field may end with one of these.
    pub not_ends_with: Vec<String>,
}

/// The fields of an event record a selector can be matched on, as `AdvancedFieldSelector`
/// documents them for a data event: the category, the serving service, the operation, the
/// read/write kind, the type and the ARN of a resource the call acted on, and the error code
/// a failed call answered with.
///
/// A selector naming a field outside this set admits NOTHING, which is the fail-closed
/// direction: a trail delivers a record only where this emulator can establish that the
/// selector the client wrote really admits it.
const MATCHABLE_FIELDS: &[&str] = &[
    EVENT_CATEGORY_FIELD,
    "eventSource",
    "eventName",
    READ_ONLY_FIELD,
    RESOURCES_TYPE_FIELD,
    RESOURCES_ARN_FIELD,
    "errorCode",
];

/// The field naming an event's category, which every advanced event selector pins.
pub const EVENT_CATEGORY_FIELD: &str = "eventCategory";

/// The field naming the TYPE of a resource the call acted on, which is what a data-event
/// selector names the class of data events by.
pub const RESOURCES_TYPE_FIELD: &str = "resources.type";

/// The field naming the ARN of a resource the call acted on.
pub const RESOURCES_ARN_FIELD: &str = "resources.ARN";

/// The field naming whether the call only READ state.
pub const READ_ONLY_FIELD: &str = "readOnly";

/// One advanced event selector of a trail, as the delivery of a data event reads it.
///
/// "Contains all selector statements in an advanced event selector"
/// (`AdvancedEventSelector$FieldSelectors`): a selector admits an event when EVERY one of its
/// field selectors does, so a selector that pins the `Data` category and one resource type
/// admits the data events of that type and nothing else.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DataEventSelector {
    /// The selector's own statements.
    pub field_selectors: Vec<DataEventFieldSelector>,
}

impl DataEventSelector {
    /// Whether this selector admits the event `record` — a rendered CloudTrail event
    /// document.
    ///
    /// A selector with no statement at all admits nothing: the model requires at least one
    /// field selector, so an empty one is a selector this emulator could not read rather than
    /// a selector that matches everything.
    #[must_use]
    pub fn admits(&self, record: &serde_json::Value) -> bool {
        !self.field_selectors.is_empty()
            && self
                .field_selectors
                .iter()
                .all(|field| field.matches(record))
    }
}

impl DataEventFieldSelector {
    /// Whether the event `record` satisfies this one statement.
    ///
    /// Each operator the statement carries must hold, and an operator holds when one of the
    /// field's values satisfies one of its operands — or, for an excluding operator, when
    /// none of them does. A field the record holds no value for therefore fails every
    /// including operator and passes every excluding one.
    fn matches(&self, record: &serde_json::Value) -> bool {
        if !MATCHABLE_FIELDS.contains(&self.field.as_str()) {
            return false;
        }
        let values = field_values(record, &self.field);
        let included = |operands: &[String], hold: fn(&str, &str) -> bool| {
            operands.is_empty()
                || values
                    .iter()
                    .any(|value| operands.iter().any(|operand| hold(value, operand)))
        };
        let excluded = |operands: &[String], hold: fn(&str, &str) -> bool| {
            !values
                .iter()
                .any(|value| operands.iter().any(|operand| hold(value, operand)))
        };
        included(&self.equals, |value, operand| value == operand)
            && included(&self.starts_with, |value, operand| {
                value.starts_with(operand)
            })
            && included(&self.ends_with, |value, operand| value.ends_with(operand))
            && excluded(&self.not_equals, |value, operand| value == operand)
            && excluded(&self.not_starts_with, |value, operand| {
                value.starts_with(operand)
            })
            && excluded(&self.not_ends_with, |value, operand| value.ends_with(operand))
    }
}

/// The values the event `record` holds at the selector field `field`.
///
/// A scalar member holds one value; `resources.type` and `resources.ARN` hold one per
/// resource the call acted on, because a selector about them is satisfied by ANY of the
/// resources. `readOnly` is a boolean in the record and a string in a selector ("This is the
/// only valid operator that you can use with the `readOnly` … field" — `Equals` over
/// `"true"` / `"false"`), so it is compared in the text form the selector states it in.
fn field_values<'a>(record: &'a serde_json::Value, field: &str) -> Vec<&'a str> {
    match field {
        READ_ONLY_FIELD => record
            .get(READ_ONLY_FIELD)
            .and_then(serde_json::Value::as_bool)
            .map(|read_only| if read_only { "true" } else { "false" })
            .into_iter()
            .collect(),
        RESOURCES_TYPE_FIELD => resource_members(record, "type"),
        RESOURCES_ARN_FIELD => resource_members(record, "ARN"),
        scalar => record
            .get(scalar)
            .and_then(serde_json::Value::as_str)
            .into_iter()
            .collect(),
    }
}

/// The text each entry of the record's `resources` holds at `member`.
fn resource_members<'a>(record: &'a serde_json::Value, member: &str) -> Vec<&'a str> {
    record
        .get("resources")
        .and_then(serde_json::Value::as_array)
        .map(|resources| {
            resources
                .iter()
                .filter_map(|resource| resource.get(member).and_then(serde_json::Value::as_str))
                .collect()
        })
        .unwrap_or_default()
}

thread_local! {
    /// The data event this thread is holding: built as the last thing the handler that served
    /// the call does, and taken by [`DeliveringDataEvents`] once the response that answers it
    /// exists — which is the only moment the id the caller was answered under is known.
    ///
    /// The two sides are ONE poll apart, the same invariant the management recorder rests on
    /// ([`crate::audited_operation`]): the framework's sequence serializes the handler's
    /// result with nothing awaited after `Operation::handle`, and the layer that takes the
    /// record is the one that awaited that sequence. The record is TAKEN rather than read and
    /// the slot is cleared before a run uses it, so a record can never be attributed to
    /// another call: a record naming a request no caller was answered under is the very
    /// defect `requestID` exists to prevent, so it is dropped instead.
    static PENDING_DATA_EVENT: RefCell<Option<(ServedApiCall, SystemTime)>> =
        const { RefCell::new(None) };
}

/// The data event this thread is holding, taken out of the slot so it is read once.
fn taken_data_event() -> Option<(ServedApiCall, SystemTime)> {
    PENDING_DATA_EVENT.with(|slot| slot.borrow_mut().take())
}

/// Drop whatever data event this thread may still hold.
fn forget_any_undelivered() {
    drop(taken_data_event());
}

/// Hold the data event `call`, served at `served_at`, until the response that answers it
/// exists.
///
/// Called by the handler that served the call, as the LAST thing it does. A call nothing
/// holds a record for delivers nothing, which is what an operation that is not a data event
/// does. `served_at` is the instant the call was served, which is the record's `eventTime`
/// and the minute its log file is named for — not the instant the write is made.
pub fn hold_data_event(call: ServedApiCall, served_at: SystemTime) {
    forget_any_undelivered();
    PENDING_DATA_EVENT.with(|slot| *slot.borrow_mut() = Some((call, served_at)));
}

/// What the serving SERVICE contributes to the delivery of one data event: the log files the
/// account's trails deliver it as.
///
/// Implemented once per service, on a value holding that service's engine handle — the
/// account's trail projection lives in the shared engine
/// ([`crate::management_events::ManagementEventLog::data_event_log_files`]) and a service
/// reaches engine state through its own accessor — so this module names no engine type.
pub trait DataEventDelivery: Send + Sync + 'static {
    /// The log files the account's trails deliver `call` as — served at `served_at` and
    /// answered under the request id `answered_as`.
    ///
    /// Empty is a data event nothing is delivered for: no trail of the account logs it, or
    /// this engine carries no log at all.
    fn delivered(
        &self,
        call: ServedApiCall,
        served_at: SystemTime,
        answered_as: &str,
    ) -> Vec<DeliveredLogFile>;
}

/// One operation served so that the data event its handler held is DELIVERED.
///
/// The OUTERMOST wrapper of such an operation: it is handed the rendered response, which is
/// the only place the id the caller was answered under can be read, and it writes the log
/// files after that response exists. Nothing about the response changes — the id is read, and
/// reported only where the renderer named none ([`named_request`]).
pub struct DeliveringDataEvents<W, S> {
    /// What serves the call.
    inner: W,
    /// The service that answers what is delivered for it.
    service: Arc<S>,
}

impl<W, S> DeliveringDataEvents<W, S> {
    /// Serve `inner` with the data events its handler holds delivered through `service`.
    pub fn new(inner: W, service: Arc<S>) -> Self {
        Self { inner, service }
    }
}

impl<W: ErasedOperation, S: DataEventDelivery> ErasedOperation for DeliveringDataEvents<W, S> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Whatever this thread may still hold belongs to no call this run serves: cleared
            // before the slot is used, so a record cannot be named on another call's response.
            forget_any_undelivered();
            let resp = self.inner.run(svc, req, ctx).await;
            let pending = taken_data_event();
            let (resp, answered_as) = named_request(resp);
            if let Some((call, served_at)) = pending {
                // Every write is made OUTSIDE the log's lock, after the response the caller
                // gets has been rendered, so it changes nothing about what this call answers.
                for log_file in self.service.delivered(call, served_at, &answered_as) {
                    log_file.deliver(ctx.callback()).await;
                }
            }
            resp
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The served operation's modelled schemes: delivering a record must not change which
        // credentials sign a call — an anonymous data-plane call stays anonymous.
        self.inner.auth_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record of one identity-pool data call, as the document writer renders it.
    fn record(resource_type: &str, event_name: &str, read_only: bool) -> serde_json::Value {
        serde_json::json!({
            "eventCategory": "Data",
            "eventSource": "cognito-identity.amazonaws.com",
            "eventName": event_name,
            "readOnly": read_only,
            "managementEvent": false,
            "resources": [{
                "accountId": "000000000000",
                "type": resource_type,
                "ARN": "arn:aws:cognito-identity:us-east-1:000000000000:identitypool/us-east-1:pool",
            }],
        })
    }

    /// One field selector stating `Equals` over `values`, which is the only operator the
    /// category, read/write and resource-type fields take.
    fn equals(field: &str, values: &[&str]) -> DataEventFieldSelector {
        DataEventFieldSelector {
            field: field.to_string(),
            equals: values.iter().map(ToString::to_string).collect(),
            ..DataEventFieldSelector::default()
        }
    }

    /// RULE: a selector admits an event when EVERY one of its field selectors does.
    #[test]
    fn a_selector_admits_an_event_only_when_every_field_selector_does() {
        let event = record("AWS::Cognito::IdentityPool", "GetId", false);
        // The rows differ along which fields the selector pins, along the values it pins them
        // to and along the operator it pins them with.
        let rows: Vec<(&str, DataEventSelector, bool)> = vec![
            (
                "the published identity-pool data selector",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        equals(RESOURCES_TYPE_FIELD, &["AWS::Cognito::IdentityPool"]),
                    ],
                },
                true,
            ),
            (
                "the category alone",
                DataEventSelector {
                    field_selectors: vec![equals(EVENT_CATEGORY_FIELD, &["Data"])],
                },
                true,
            ),
            (
                "the management category",
                DataEventSelector {
                    field_selectors: vec![equals(EVENT_CATEGORY_FIELD, &["Management"])],
                },
                false,
            ),
            (
                "a data selector over ANOTHER resource type",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        equals(RESOURCES_TYPE_FIELD, &["AWS::S3::Object"]),
                    ],
                },
                false,
            ),
            (
                "one of several types named by the same field",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        equals(
                            RESOURCES_TYPE_FIELD,
                            &["AWS::S3::Object", "AWS::Cognito::IdentityPool"],
                        ),
                    ],
                },
                true,
            ),
            (
                "a read-only narrowing the call does not fall in",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        equals(READ_ONLY_FIELD, &["true"]),
                    ],
                },
                false,
            ),
            (
                "a read-only narrowing the call falls in",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        equals(READ_ONLY_FIELD, &["false"]),
                    ],
                },
                true,
            ),
            (
                "an event name the call carries",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        equals("eventName", &["GetId"]),
                    ],
                },
                true,
            ),
            (
                "an event name excluded",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        DataEventFieldSelector {
                            field: "eventName".to_string(),
                            not_equals: vec!["GetId".to_string()],
                            ..DataEventFieldSelector::default()
                        },
                    ],
                },
                false,
            ),
            (
                "an ARN prefix the resource starts with",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        DataEventFieldSelector {
                            field: RESOURCES_ARN_FIELD.to_string(),
                            starts_with: vec!["arn:aws:cognito-identity:".to_string()],
                            ..DataEventFieldSelector::default()
                        },
                    ],
                },
                true,
            ),
            (
                "an ARN prefix the resource does not start with",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        DataEventFieldSelector {
                            field: RESOURCES_ARN_FIELD.to_string(),
                            starts_with: vec!["arn:aws:s3:::".to_string()],
                            ..DataEventFieldSelector::default()
                        },
                    ],
                },
                false,
            ),
            (
                "a field this emulator cannot match on",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        equals("vpcEndpointId", &["vpce-0123"]),
                    ],
                },
                false,
            ),
            (
                "no statement at all",
                DataEventSelector::default(),
                false,
            ),
        ];
        for (case, selector, expected) in rows {
            let actual = selector.admits(&event);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected admits == {expected}, got {actual} for {selector:?}"
            );
        }
    }

    /// RULE: a field the record holds no value for fails every INCLUDING operator and passes
    /// every excluding one — so a selector about a resource type admits no event that acted
    /// on no resource.
    #[test]
    fn a_field_the_record_does_not_carry_admits_nothing_that_requires_it() {
        let without_resources = serde_json::json!({
            "eventCategory": "Data",
            "eventName": "GetId",
            "readOnly": false,
        });
        for (case, selector, expected) in [
            (
                "a required resource type",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        equals(RESOURCES_TYPE_FIELD, &["AWS::Cognito::IdentityPool"]),
                    ],
                },
                false,
            ),
            (
                "an excluded resource type",
                DataEventSelector {
                    field_selectors: vec![
                        equals(EVENT_CATEGORY_FIELD, &["Data"]),
                        DataEventFieldSelector {
                            field: RESOURCES_TYPE_FIELD.to_string(),
                            not_equals: vec!["AWS::S3::Object".to_string()],
                            ..DataEventFieldSelector::default()
                        },
                    ],
                },
                true,
            ),
        ] {
            let actual = selector.admits(&without_resources);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected admits == {expected}, got {actual}"
            );
        }
    }
}
