// SPDX-License-Identifier: CC0-1.0

//! The DELIVERY a subscriber owes its target: the call that puts an accepted
//! event where the subscriber's `InvokeConfiguration` names.
//!
//! `CreateSubscriber` creates "a subscriber on an event bus, which delivers
//! matching events to the configured target", so a bus that accepts an event
//! owes that event to each of its subscribers. Nothing a client calls makes that
//! happen: a client publishes and then watches the TARGET, which is why the
//! publish itself performs the deliveries it has just made due.
//!
//! Which subscriber is owed an event, and what its target is handed, is
//! [`super::subscribers`]; this module is the call alone. It is made over the
//! IN-PROCESS callback into this same emulator
//! (`OperationContext::callback`) — no socket is opened and nothing leaves the
//! host — so what the target holds afterwards is what that service's own
//! handlers wrote.
//!
//! The target kinds this slice does not call are recorded as NOT ATTEMPTED and
//! never as delivered: a delivery nobody made must not be reported as one.

use super::subscribers::{delivered_body, delivered_event, filters_match, DueDelivery, Subscriber};

/// The `arnNamespace` of Amazon SQS, the vendor field of a queue's ARN.
pub const SQS_ARN_NAMESPACE: &str = "sqs";

/// How many messages one `SendMessageBatch` call carries at most.
///
/// "For Amazon SQS, the maximum number of entries you can include in a single
/// `SendMessageBatch` … request is 10"
/// (`com.amazonaws.sqs#TooManyEntriesInBatchRequest`), so a fan-out of more
/// events than that is sent as several calls rather than one the queue refuses.
pub const MAX_QUEUE_BATCH_ENTRIES: usize = 10;

/// The `DataType` a message attribute carrying text declares
/// (`com.amazonaws.sqs#MessageAttributeValue$DataType`: "String").
const STRING_ATTRIBUTE_TYPE: &str = "String";

/// The one message system attribute Amazon SQS accepts on a send
/// (`com.amazonaws.sqs#MessageSystemAttributeNameForSends`).
const TRACE_HEADER_ATTRIBUTE: &str = "AWSTraceHeader";

/// Deliver what each subscriber of a bus is owed — the events taken for it by
/// [`super::ComAmazonawsEventbridgev2Awseventsv2State::take_due_deliveries`], in
/// publication order.
///
/// Each subscriber is handed the events its own filters select
/// ([`filters_match`]); the rest of its due window is skipped, because a filter
/// is what the subscriber asked to be delivered. The matching of one call is
/// charged to ONE budget across every subscriber and event, so a request can
/// never buy more matching work than that budget sells.
pub async fn deliver_due(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    due: &[DueDelivery],
) {
    let mut budget = ::service_common::event_pattern::MatchBudget::new(
        ::service_common::event_pattern::MAX_MATCH_STEPS_PER_REQUEST,
    );
    for owed in due {
        let subscriber = &owed.subscriber;
        let mut bodies = Vec::new();
        for stored in &owed.events {
            let event = delivered_event(&subscriber.event_bus_arn, stored);
            if !filters_match(subscriber.filter_configuration.as_ref(), &event, &mut budget) {
                continue;
            }
            match delivered_body(subscriber, &event) {
                Some(body) => bodies.push(body),
                None => ::tracing::warn!(
                    subscriber = %subscriber.arn,
                    event = %event.event_id,
                    "an EventBridge subscriber was not delivered an event: its transformer names \
                     a transformation this emulator does not perform"
                ),
            }
        }
        if bodies.is_empty() {
            continue;
        }
        deliver_to_target(callback, subscriber, &bodies).await;
    }
}

/// Hand `bodies` to the resource this subscriber's `InvokeConfiguration` names,
/// through the one API that resource's service is invoked by.
async fn deliver_to_target(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    subscriber: &Subscriber,
    bodies: &[String],
) {
    let arn = subscriber.target_arn();
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        ::tracing::warn!(
            subscriber = %subscriber.arn,
            target = %arn,
            "an EventBridge subscriber's delivery was not attempted: its TargetArn names no \
             resource"
        );
        return;
    };
    // The target's OWN region — the one its ARN names — which is not
    // necessarily the region its subscriber's bus lives in; a target ARN
    // carrying no region label addresses the bus's own region.
    let region = ::service_common::arn::region_of(arn, parsed.vendor).unwrap_or(&subscriber.region);
    match parsed.vendor {
        SQS_ARN_NAMESPACE => {
            let Some(queue_url) = queue_url(region, parsed.account, parsed.resource) else {
                ::tracing::warn!(
                    subscriber = %subscriber.arn,
                    target = %arn,
                    "an EventBridge subscriber's delivery was not attempted: its TargetArn names \
                     no queue"
                );
                return;
            };
            send_to_queue(callback, subscriber, region, &queue_url, bodies).await;
        }
        _ => ::tracing::warn!(
            subscriber = %subscriber.arn,
            target = %arn,
            "an EventBridge subscriber's delivery was not attempted: this emulator makes no call \
             for a target of that service yet"
        ),
    }
}

/// Send one message per event to the queue at `queue_url`, in batches the queue
/// accepts.
///
/// The values the subscriber's `SqsParameters` carries travel with the send:
/// "Values are forwarded to the SQS `SendMessageBatch` API" (`SqsParameters`),
/// which is the API this delivery makes.
async fn send_to_queue(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    subscriber: &Subscriber,
    region: &str,
    queue_url: &str,
    bodies: &[String],
) {
    let client = sqs_client(callback, region);
    let parameters = subscriber.invoke_configuration.sqs_parameters.as_ref();
    for (batch, chunk) in bodies.chunks(MAX_QUEUE_BATCH_ENTRIES).enumerate() {
        let mut request = client.send_message_batch().queue_url(queue_url);
        for (position, body) in chunk.iter().enumerate() {
            let mut entry = ::aws_sdk_sqs::types::SendMessageBatchRequestEntry::builder()
                // An entry id identifies one message WITHIN its batch
                // ("an identifier for a message in this batch used to
                // communicate the result"), and the character set it is held to
                // is alphanumerics, hyphens and underscores.
                .id(format!(
                    "{}-{position}",
                    batch * MAX_QUEUE_BATCH_ENTRIES
                ))
                .message_body(body.as_str());
            if let Some(parameters) = parameters {
                // A FIFO queue takes no message without a group, so the group
                // the subscriber names travels with every delivery to it.
                if let Some(group) = parameters.message_group_id.as_deref() {
                    entry = entry.message_group_id(group);
                }
                if let Some(deduplication_id) = parameters.message_deduplication_id.as_deref() {
                    entry = entry.message_deduplication_id(deduplication_id);
                }
                // `DelaySeconds` is text in this model, and the queue takes a
                // number: a value that is not one carries no delay rather than
                // one nobody asked for. `SqsParameters` is a @sensitive shape,
                // so what a refusal reports is the member and not its value.
                if let Some(delay) = parameters.delay_seconds.as_deref() {
                    match delay.parse::<i32>() {
                        Ok(seconds) => entry = entry.delay_seconds(seconds),
                        Err(error) => ::tracing::warn!(
                            subscriber = %subscriber.arn,
                            %error,
                            "an EventBridge subscriber's DelaySeconds is not a number of seconds \
                             and was not carried to its queue target"
                        ),
                    }
                }
                for (name, value) in parameters.message_attributes.iter().flatten() {
                    if let Some(attribute) = message_attribute(subscriber, value) {
                        entry = entry.message_attributes(name, attribute);
                    }
                }
                for (name, value) in parameters.message_system_attributes.iter().flatten() {
                    if let Some(attribute) = system_attribute(subscriber, name, value) {
                        entry = entry.message_system_attributes(
                            ::aws_sdk_sqs::types::MessageSystemAttributeNameForSends::AwsTraceHeader,
                            attribute,
                        );
                    }
                }
            }
            match entry.build() {
                Ok(entry) => request = request.entries(entry),
                Err(error) => ::tracing::warn!(
                    subscriber = %subscriber.arn,
                    %error,
                    "an EventBridge subscriber's delivery could not be built as a queue message"
                ),
            }
        }
        match request.send().await {
            Ok(answer) => {
                for failed in answer.failed() {
                    ::tracing::warn!(
                        subscriber = %subscriber.arn,
                        queue = %queue_url,
                        code = %failed.code(),
                        "an EventBridge subscriber's delivery was refused by its queue target"
                    );
                }
            }
            Err(error) => ::tracing::warn!(
                subscriber = %subscriber.arn,
                queue = %queue_url,
                error = ?error,
                "an EventBridge subscriber's delivery did not reach its queue target"
            ),
        }
    }
}

/// One `MessageAttributes` entry of the subscriber's `SqsParameters` as the
/// queue takes it, or `None` for one this delivery cannot carry.
///
/// A text value travels with its declared `DataType`, defaulting to `String`
/// when the subscriber named none, since that is the type of the value it
/// carries. A `BinaryValue` is NOT carried: the queue takes binary as bytes and
/// the encoding of this member's text is not stated, so guessing one would put
/// an attribute on the message the subscriber did not configure.
///
/// `SqsParameters` is a @sensitive shape, so no value of it reaches a log: what
/// is reported is the subscriber and what could not be carried.
fn message_attribute(
    subscriber: &Subscriber,
    value: &crate::ops::types::SqsMessageAttributeValue,
) -> Option<::aws_sdk_sqs::types::MessageAttributeValue> {
    let Some(text) = value.string_value.as_deref() else {
        ::tracing::warn!(
            subscriber = %subscriber.arn,
            "an EventBridge subscriber's message attribute carries no StringValue and was not \
             carried to its queue target"
        );
        return None;
    };
    let data_type = value
        .data_type
        .as_deref()
        .filter(|declared| !declared.is_empty())
        .unwrap_or(STRING_ATTRIBUTE_TYPE);
    match ::aws_sdk_sqs::types::MessageAttributeValue::builder()
        .data_type(data_type)
        .string_value(text)
        .build()
    {
        Ok(attribute) => Some(attribute),
        Err(error) => {
            ::tracing::warn!(
                subscriber = %subscriber.arn,
                %error,
                "an EventBridge subscriber's message attribute could not be built"
            );
            None
        }
    }
}

/// One `MessageSystemAttributes` entry as the queue takes it, or `None` for a
/// name the queue accepts no system attribute under.
///
/// Amazon SQS accepts exactly one system attribute on a send,
/// `AWSTraceHeader`, so an entry named anything else is not carried: the queue
/// would refuse the whole send for it.
fn system_attribute(
    subscriber: &Subscriber,
    name: &str,
    value: &crate::ops::types::SqsMessageAttributeValue,
) -> Option<::aws_sdk_sqs::types::MessageSystemAttributeValue> {
    if name != TRACE_HEADER_ATTRIBUTE {
        ::tracing::warn!(
            subscriber = %subscriber.arn,
            "an EventBridge subscriber's message system attribute is not one Amazon SQS accepts \
             on a send and was not carried to its queue target"
        );
        return None;
    }
    let text = value.string_value.as_deref()?;
    let data_type = value
        .data_type
        .as_deref()
        .filter(|declared| !declared.is_empty())
        .unwrap_or(STRING_ATTRIBUTE_TYPE);
    match ::aws_sdk_sqs::types::MessageSystemAttributeValue::builder()
        .data_type(data_type)
        .string_value(text)
        .build()
    {
        Ok(attribute) => Some(attribute),
        Err(error) => {
            ::tracing::warn!(
                subscriber = %subscriber.arn,
                attribute = %name,
                %error,
                "an EventBridge subscriber's message system attribute could not be built"
            );
            None
        }
    }
}

/// The `QueueUrl` Amazon SQS addresses the queue of `name` in `account` by, or
/// `None` when the ARN's fields name no queue.
///
/// A queue's ARN names the queue DIRECTLY
/// (`arn:<partition>:sqs:<region>:<account>:<QueueName>`), and the url every SQS
/// operation takes is `<endpoint>/<account>/<QueueName>`. A resource field
/// carrying a type separator addresses no queue, and an account field that is
/// not an account id names none either — so neither reaches the url, which is
/// what keeps a caller-written ARN from reshaping the address this delivery is
/// made to.
fn queue_url(region: &str, account: &str, name: &str) -> Option<String> {
    /// How many digits an account id carries.
    const ACCOUNT_ID_DIGITS: usize = 12;

    let account_names_an_account =
        account.len() == ACCOUNT_ID_DIGITS && account.bytes().all(|byte| byte.is_ascii_digit());
    if !account_names_an_account || name.contains('/') || name.contains(':') {
        return None;
    }
    Some(format!("https://sqs.{region}.amazonaws.com/{account}/{name}"))
}

/// Build the Amazon SQS client over the IN-PROCESS callback into this same
/// emulator, against that service's endpoint in `region`.
///
/// The transport is in-process, so the client disables what a network client
/// needs and this one cannot have: retries, timeouts, stalled-stream protection
/// and identity caching each need a sleep implementation the callback transport
/// is not given, and there is nothing to retry behind it anyway.
fn sqs_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_sqs::Client {
    let config = ::aws_sdk_sqs::Config::builder()
        .behavior_version(::aws_sdk_sqs::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_sqs::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://sqs.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_sqs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sqs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sqs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_sqs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_sqs::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::queue_url;

    /// A queue url is built only from fields that really name a queue of an
    /// account: the rows differ along the resource field and the account field.
    #[test]
    fn a_queue_url_is_built_only_from_fields_that_name_a_queue() {
        // (case, account, resource, the url the pair addresses)
        let rows: [(&str, &str, &str, Option<&str>); 6] = [
            (
                "a queue name",
                "000000000000",
                "orders",
                Some("https://sqs.eu-west-2.amazonaws.com/000000000000/orders"),
            ),
            (
                "a FIFO queue name, whose name carries a suffix",
                "000000000000",
                "orders.fifo",
                Some("https://sqs.eu-west-2.amazonaws.com/000000000000/orders.fifo"),
            ),
            ("a resource carrying a path", "000000000000", "../other", None),
            ("a resource carrying a type", "000000000000", "queue:orders", None),
            ("an account that is no account id", "my-account", "orders", None),
            ("an account of the wrong length", "0000", "orders", None),
        ];
        for (case, account, resource, expected) in rows {
            let built = queue_url("eu-west-2", account, resource);
            assert_eq!(
                built.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {built:?}"
            );
        }
    }
}
