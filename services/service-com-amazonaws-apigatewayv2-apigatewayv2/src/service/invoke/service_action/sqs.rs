// SPDX-License-Identifier: CC0-1.0

//! The `SQS-SendMessage` action: the message a route sends to a queue.
//!
//! The parameters are the members of `SendMessage`'s own input
//! (`smithy_query(query="operation", operation="SendMessage")` on the Amazon SQS model),
//! which is what the action takes. `QueueUrl` and `MessageBody` are the members the model
//! marks required ([`SEND_MESSAGE_REQUIRED_PARAMETERS`]), so an integration mapping neither
//! names no send at all and the request is answered as an integration that produced no
//! response — never as a message nothing enqueued — while a request that carries no value for
//! the expression the integration maps onto one of them is a bad request, which is the parent
//! module's own rule for every action's required parameters.
//!
//! The queue URL is a parameter of the in-process call and NOT a destination this process
//! connects to: the client is the emulator's own transport, addressed at the Amazon SQS
//! endpoint of the api's Region, so the send is served by the queue this emulator holds.
//!
//! What the send then does is Amazon SQS's own: whether the calling role may send to that
//! queue, what the queue's own configuration does with the message, and what a consumer
//! receives.
//!
//! # The form the send's answer is reported in
//!
//! The caller of this route is answered the queue api's own `{Action}Response` envelope —
//! `SendMessageResponse` wrapping the `SendMessageResult` members beside the
//! `ResponseMetadata` that names the request — as `text/xml` ([`query_envelope`]).
//!
//! That is the form RECORDED for this subtype, and relaying it is what the route exists for:
//! a client fronting a queue with an HTTP api reads the id of the message it just sent off
//! this document, and the behavioral recording this emulator is measured against reads that
//! id out of `SendMessageResponse/SendMessageResult/MessageId` and compares it with the id
//! its consumer then takes off the queue — a JSON object of the same members answers it
//! nothing, which is the divergence this module's issue records.
//!
//! What decides the form is therefore NOT the protocol an SDK client of Amazon SQS speaks:
//! the model in this tree declares `awsJson1_0` (and its `awsQueryCompatible` covers that
//! service's error CODE surface, not its documents), so the in-process call this module makes
//! with the SDK is itself answered a JSON document. It is which INTERFACE of the queue api
//! API Gateway performs a first-class send over, and for this subtype the recording shows the
//! query one. Nothing of the envelope is a shape this gateway invented for itself: the
//! element names are the action's own, the members are the ones its output shape declares,
//! and every value in it — the digests, the id, the sequence number, the request named — is
//! what Amazon SQS answered the call with.

use aws_sdk_sqs::operation::RequestId as _;
use aws_smithy_schema_server::xml_escape;
use smithy_server_runtime::OperationContext;

use super::credentials::SigningIdentity;
use super::parameters::Mapped;
use super::{Performed, Unperformed};

/// The `IntegrationSubtype` that names this action.
pub const SEND_MESSAGE_SUBTYPE: &str = "SQS-SendMessage";

/// Every parameter an integration may map onto the send: the members of
/// `SendMessageRequest` this emulator carries into the call.
///
/// The two attribute MAPS the operation also declares (`MessageAttributes`,
/// `MessageSystemAttributes`) are not among them, so an integration mapping one is reported
/// rather than sent without it ([`Mapped::beyond`]).
pub const SEND_MESSAGE_PARAMETERS: &[&str] = &[
    "DelaySeconds",
    "MessageBody",
    "MessageDeduplicationId",
    "MessageGroupId",
    "QueueUrl",
];

/// The parameters the send must carry a value for: the members `SendMessageRequest` marks
/// required.
pub const SEND_MESSAGE_REQUIRED_PARAMETERS: &[&str] = &["MessageBody", "QueueUrl"];

/// The `arnNamespace` Amazon SQS publishes its endpoint and signs under, which is the
/// endpoint label the in-process client is addressed at.
const SQS_ENDPOINT: &str = "sqs";

/// The action of the Amazon SQS api this subtype performs, which is what the envelope its
/// answer is reported in is named after (`{Action}Response` / `{Action}Result`).
const SEND_MESSAGE_ACTION: &str = "SendMessage";

/// The namespace the query interface of the queue api states its documents in: the
/// `doc/{api version}` namespace it publishes, for the api version the model declares
/// (`smithy_query(query="service_info")` reports `"version": "2012-11-05"`).
const SQS_QUERY_NAMESPACE: &str = "http://queue.amazonaws.com/doc/2012-11-05/";

/// Send the mapped message, and answer the document Amazon SQS returned.
pub async fn send_message(
    ctx: &OperationContext<'_>,
    region: &str,
    identity: Option<&SigningIdentity>,
    mapped: &Mapped,
) -> Result<Performed, Unperformed> {
    let (Some(queue_url), Some(body)) = (mapped.get("QueueUrl"), mapped.get("MessageBody")) else {
        return Err(Unperformed::NotPerformed(format!(
            "the integration maps no QueueUrl and MessageBody, which {SEND_MESSAGE_SUBTYPE} \
             requires"
        )));
    };
    let client = action_client!(aws_sdk_sqs, ctx.callback(), SQS_ENDPOINT, region, identity);
    let mut call = client.send_message().queue_url(queue_url).message_body(body);
    if let Some(delay) = mapped.get("DelaySeconds") {
        // `DelaySeconds` is an integer member, so a mapped value that is not one is not a
        // delay the action can be called with.
        let seconds = delay.parse::<i32>().map_err(|error| {
            Unperformed::NotPerformed(format!(
                "the integration maps a DelaySeconds this action cannot be called with: \
                 {error}"
            ))
        })?;
        call = call.delay_seconds(seconds);
    }
    if let Some(deduplication_id) = mapped.get("MessageDeduplicationId") {
        call = call.message_deduplication_id(deduplication_id);
    }
    if let Some(group_id) = mapped.get("MessageGroupId") {
        call = call.message_group_id(group_id);
    }
    let sent = call
        .send()
        .await
        .map_err(|error| super::unperformed(SEND_MESSAGE_SUBTYPE, &error))?;
    // The members of the action's result, in the order its output shape declares them.
    let answered: [(&str, Option<&str>); 5] = [
        ("MD5OfMessageBody", sent.md5_of_message_body()),
        ("MD5OfMessageAttributes", sent.md5_of_message_attributes()),
        (
            "MD5OfMessageSystemAttributes",
            sent.md5_of_message_system_attributes(),
        ),
        ("MessageId", sent.message_id()),
        ("SequenceNumber", sent.sequence_number()),
    ];
    Ok(Performed::query_envelope(query_envelope(
        SEND_MESSAGE_ACTION,
        &answered,
        sent.request_id(),
    )))
}

/// The document the QUERY INTERFACE of the queue api answers `action` with: the members the
/// service answered under `{action}Result`, and the `ResponseMetadata` that names the
/// request, both inside the `{action}Response` element that interface wraps them in (see the
/// module docs for why this is the form a first-class send is relayed in).
///
/// Written the way this emulator's own query serializer writes an answer
/// (`vendor/smithy-schema-server/src/query.rs`), with every value escaped through the one
/// XML escaper the framework exposes, so the envelope is well formed whatever the service
/// answered. A member the service answered nothing for is LEFT OFF, which is how an absent
/// member is written on this wire.
///
/// The request named is the one the SERVICE named for the call it served, since this envelope
/// is that call's answer; a call whose answer named none is named by a freshly minted id, the
/// way every other `ResponseMetadata/RequestId` this tree reports is.
fn query_envelope(action: &str, answered: &[(&str, Option<&str>)], request: Option<&str>) -> String {
    let mut envelope = format!(
        "<{action}Response xmlns=\"{}\"><{action}Result>",
        xml_escape(SQS_QUERY_NAMESPACE)
    );
    for (member, value) in answered {
        if let Some(value) = value {
            envelope.push_str(&format!("<{member}>{}</{member}>", xml_escape(value)));
        }
    }
    let request_id = request
        .map(str::to_string)
        .unwrap_or_else(::service_common::query_error_envelope::new_request_id);
    envelope.push_str(&format!(
        "</{action}Result><ResponseMetadata><RequestId>{}</RequestId></ResponseMetadata>\
         </{action}Response>",
        xml_escape(&request_id)
    ));
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the envelope carries the members the service answered, under the result element
    /// the action names, and leaves off every member it answered nothing for.
    #[test]
    fn the_envelope_carries_the_members_the_service_answered() {
        /// One case: what the service answered, the request it named for the call it served,
        /// the fragments the envelope must then carry, and the ones it must not.
        struct Row {
            case: &'static str,
            answered: &'static [(&'static str, Option<&'static str>)],
            request: Option<&'static str>,
            expected: &'static [&'static str],
            absent: &'static [&'static str],
        }
        // Rows differ along WHAT the service answered: every member, the two a standard
        // queue's send answers, nothing at all, and a value carrying XML metacharacters.
        let rows: &[Row] = &[
            Row {
                case: "every member of the result",
                answered: &[
                    ("MD5OfMessageBody", Some("7299ee64a9c302c7c74ebf3dc3dcc03f")),
                    ("MessageId", Some("m-1")),
                    ("SequenceNumber", Some("18849496460467696128")),
                ],
                request: Some("27daac76-34dd-47df-bd01-1f6e873584a0"),
                expected: &[
                    "<MD5OfMessageBody>7299ee64a9c302c7c74ebf3dc3dcc03f</MD5OfMessageBody>",
                    "<MessageId>m-1</MessageId>",
                    "<SequenceNumber>18849496460467696128</SequenceNumber>",
                    "<RequestId>27daac76-34dd-47df-bd01-1f6e873584a0</RequestId>",
                ],
                absent: &[],
            },
            Row {
                case: "the members a standard queue's send answers",
                answered: &[
                    ("MD5OfMessageBody", Some("d41d8cd98f00b204e9800998ecf8427e")),
                    ("MessageId", Some("m-2")),
                    ("SequenceNumber", None),
                ],
                request: Some("9f0a1b2c-3d4e-5f60-7182-93a4b5c6d7e8"),
                expected: &["<MessageId>m-2</MessageId>"],
                absent: &["SequenceNumber"],
            },
            Row {
                case: "nothing at all, and no request named",
                answered: &[("MessageId", None)],
                request: None,
                expected: &["<SendMessageResult></SendMessageResult>"],
                absent: &["MessageId"],
            },
            Row {
                case: "a value carrying XML metacharacters",
                answered: &[("MessageId", Some("a<b&c"))],
                request: None,
                expected: &["<MessageId>a&lt;b&amp;c</MessageId>"],
                absent: &["a<b&c"],
            },
        ];
        for row in rows {
            let case = row.case;
            let envelope = query_envelope(SEND_MESSAGE_ACTION, row.answered, row.request);
            assert!(
                envelope.starts_with(&format!(
                    "<SendMessageResponse xmlns=\"{SQS_QUERY_NAMESPACE}\"><SendMessageResult>"
                )),
                "case {case:?}: expected the action's own envelope, got {envelope:?}"
            );
            assert!(
                envelope.ends_with("</SendMessageResponse>"),
                "case {case:?}: expected a closed envelope, got {envelope:?}"
            );
            for fragment in row.expected {
                assert!(
                    envelope.contains(fragment),
                    "case {case:?}: expected {fragment:?} in {envelope:?}"
                );
            }
            for fragment in row.absent {
                assert!(
                    !envelope.contains(fragment),
                    "case {case:?}: expected no {fragment:?} in {envelope:?}"
                );
            }
        }
    }

    /// RULE: the `ResponseMetadata` names the request the SERVICE named for the call it
    /// served, and a call whose answer named none is named by a fresh id of its own.
    #[test]
    fn the_envelope_names_the_request_the_service_named() {
        let case = "the request the service named";
        let named = query_envelope(
            SEND_MESSAGE_ACTION,
            &[("MessageId", Some("m-1"))],
            Some("27daac76-34dd-47df-bd01-1f6e873584a0"),
        );
        assert_eq!(
            request_id_of(&named).as_deref(),
            Some("27daac76-34dd-47df-bd01-1f6e873584a0"),
            "case {case:?}: expected the service's own request id, got {named:?}"
        );
        let case = "three answers naming no request";
        let envelopes: Vec<String> = (0..3)
            .map(|_| query_envelope(SEND_MESSAGE_ACTION, &[("MessageId", Some("m-1"))], None))
            .collect();
        let mut ids = Vec::new();
        for envelope in &envelopes {
            let id = request_id_of(envelope)
                .unwrap_or_else(|| panic!("case {case:?}: no request id in {envelope:?}"));
            assert!(
                ::service_common::query_error_envelope::is_request_id(&id),
                "case {case:?}: expected the form every AWS request id carries, got {id:?}"
            );
            ids.push(id);
        }
        ids.sort();
        ids.dedup();
        assert_eq!(
            ids.len(),
            envelopes.len(),
            "case {case:?}: expected each answer to name its own request, got {ids:?}"
        );
    }

    /// The id one envelope's `ResponseMetadata` names the request by.
    fn request_id_of(envelope: &str) -> Option<String> {
        let opened = "<ResponseMetadata><RequestId>";
        let start = envelope.find(opened)? + opened.len();
        let rest = envelope.get(start..)?;
        let end = rest.find("</RequestId>")?;
        rest.get(..end).map(str::to_string)
    }
}
