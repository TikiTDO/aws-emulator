// SPDX-License-Identifier: CC0-1.0

//! The `EventBridge-PutEvents` action: the event a route publishes onto a bus.
//!
//! The parameters are the members of one `PutEventsRequestEntry` (`smithy_query(query=
//! "shape", shape="PutEventsRequestEntry")` on the EventBridge model), because that is what
//! the action takes: a first-class integration publishes ONE event per request — the request
//! is the event — so the mapped parameters are that entry's members and the call carries the
//! single entry they state.
//!
//! What the publication then does is EventBridge's own: whether the calling role may publish
//! onto the named bus, which rules of that bus match the event, and what each of their
//! targets receives. Nothing of that is decided here; this module maps the entry and makes
//! the call.

use serde_json::{json, Map, Value};
use smithy_server_runtime::OperationContext;

use super::credentials::SigningIdentity;
use super::parameters::Mapped;
use super::{Performed, Unperformed};

/// The `IntegrationSubtype` that names this action.
pub const PUT_EVENTS_SUBTYPE: &str = "EventBridge-PutEvents";

/// Every parameter an integration may map onto the event: the members of
/// `PutEventsRequestEntry`. A member the integration maps nothing for is left off the entry,
/// the way the model leaves an absent member off.
pub const PUT_EVENTS_PARAMETERS: &[&str] = &[
    "Detail",
    "DetailType",
    "EventBusName",
    "Resources",
    "Source",
    "Time",
    "TraceHeader",
];

/// The parameters the publication must carry a value for: NONE, because
/// `PutEventsRequestEntry` marks no member required — an entry stating only some of them is
/// an entry EventBridge itself decides about.
pub const PUT_EVENTS_REQUIRED_PARAMETERS: &[&str] = &[];

/// The `arnNamespace` EventBridge publishes its endpoint and signs under, which is the
/// endpoint label the in-process client is addressed at.
const EVENTS_ENDPOINT: &str = "events";

/// How a mapped `Resources` states more than one ARN: as the JSON list the member is
/// (`EventResourceList`). A value that is no list names ONE resource, which is the
/// single-entry case of the same member.
fn resources(mapped: Option<&str>) -> Vec<String> {
    let Some(value) = mapped else {
        return Vec::new();
    };
    match ::serde_json::from_str::<Value>(value) {
        Ok(Value::Array(entries)) => entries
            .iter()
            .map(|entry| match entry {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect(),
        _ => vec![value.to_string()],
    }
}

/// Publish the mapped event, and answer the document EventBridge returned.
///
/// The action is performed over EventBridge's own api, which states an answer as a JSON
/// document (`awsJson1_1`), so that is the form the caller is answered it in.
pub async fn put_events(
    ctx: &OperationContext<'_>,
    region: &str,
    identity: Option<&SigningIdentity>,
    mapped: &Mapped,
) -> Result<Performed, Unperformed> {
    let mut entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder();
    if let Some(source) = mapped.get("Source") {
        entry = entry.source(source);
    }
    if let Some(detail_type) = mapped.get("DetailType") {
        entry = entry.detail_type(detail_type);
    }
    if let Some(detail) = mapped.get("Detail") {
        entry = entry.detail(detail);
    }
    if let Some(event_bus_name) = mapped.get("EventBusName") {
        entry = entry.event_bus_name(event_bus_name);
    }
    if let Some(trace_header) = mapped.get("TraceHeader") {
        entry = entry.trace_header(trace_header);
    }
    for resource in resources(mapped.get("Resources")) {
        entry = entry.resources(resource);
    }
    if let Some(time) = mapped.get("Time") {
        // "The time stamp of the event, per RFC3339" (`PutEventsRequestEntry$Time`): a
        // mapped value that is not that instant is not a time the action can be called
        // with, so the integration is not performed rather than published at another
        // instant.
        let parsed = ::aws_smithy_types::DateTime::from_str(
            time,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .map_err(|error| {
            Unperformed::NotPerformed(format!(
                "the integration maps a Time this action cannot be called with: {error}"
            ))
        })?;
        entry = entry.time(parsed);
    }
    let client = action_client!(
        aws_sdk_eventbridge,
        ctx.callback(),
        EVENTS_ENDPOINT,
        region,
        identity
    );
    let published = client
        .put_events()
        .entries(entry.build())
        .send()
        .await
        .map_err(|error| super::unperformed(PUT_EVENTS_SUBTYPE, &error))?;
    let mut answer = Map::new();
    answer.insert(
        "Entries".to_string(),
        Value::Array(
            published
                .entries()
                .iter()
                .map(|entry| {
                    let mut reported = Map::new();
                    if let Some(event_id) = entry.event_id() {
                        reported.insert("EventId".to_string(), json!(event_id));
                    }
                    if let Some(code) = entry.error_code() {
                        reported.insert("ErrorCode".to_string(), json!(code));
                    }
                    if let Some(message) = entry.error_message() {
                        reported.insert("ErrorMessage".to_string(), json!(message));
                    }
                    Value::Object(reported)
                })
                .collect(),
        ),
    );
    answer.insert(
        "FailedEntryCount".to_string(),
        json!(published.failed_entry_count()),
    );
    Ok(Performed::json(&Value::Object(answer)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: `Resources` states the event's resources as the list the member is, and a
    /// value that is no list names one resource.
    #[test]
    fn the_resources_of_an_event_are_the_list_the_member_is() {
        let rows: &[(&str, Option<&str>, &[&str])] = &[
            ("nothing mapped", None, &[]),
            (
                "one ARN",
                Some("arn:aws:s3:::orders"),
                &["arn:aws:s3:::orders"],
            ),
            (
                "a JSON list of two",
                Some(r#"["arn:aws:s3:::orders","arn:aws:s3:::invoices"]"#),
                &["arn:aws:s3:::orders", "arn:aws:s3:::invoices"],
            ),
            ("an empty JSON list", Some("[]"), &[]),
        ];
        for (case, mapped, expected) in rows {
            let actual = resources(*mapped);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
