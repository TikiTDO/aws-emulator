// SPDX-License-Identifier: CC0-1.0

//! The Amazon SNS half of the SDK seam a RUNNING handler calls another service through: the
//! topics it publishes to, the subscriptions it manages and the attributes and tags it reads,
//! built the way Amazon SNS's own protocol binds them.
//!
//! "Lambda as glue" is the shape AWS documents most: a function reacts to an event and tells
//! the rest of the system about it by publishing to a topic, and it subscribes a queue, a
//! function or an endpoint to that topic so the fan-out exists at all. Those calls are only
//! reachable from a handler if this seam binds them.
//!
//! Most services a handler reaches bind their requests with an AWS JSON protocol, where the
//! request body IS the operation's input document, so one code path serves every operation of
//! every one of them ([`super::aws_api`]). Amazon SNS does not: its model declares `awsQuery`
//! (`smithy_query(query="service_info")`), so a request to it is a form-encoded `Action` with
//! the input flattened into its parameters, and the answer is an XML document. That binding is
//! not written out here: the request is built, signed, dispatched and parsed by the Amazon SNS
//! CLIENT this crate holds (`aws_sdk_sns`), over the same in-process callback transport every
//! other call of this seam uses, so the wire details are the client's and the answer is the
//! emulated Amazon SNS service's own.
//!
//! What a guest can and cannot do here:
//!
//! * The destination is ALWAYS this emulator: the client is given the callback's own transport
//!   and Amazon SNS's own regional endpoint, so no host name is resolved and no socket is
//!   opened.
//! * The call is signed as the identity the invocation was given — the session of the
//!   function's execution role ([`super::execution_role_session`]) — so Amazon SNS judges it
//!   against the policies in force on that role exactly as it judges any other signed request,
//!   and a role without `sns:Publish` is answered that service's own authorization refusal
//!   rather than a success.
//! * Only the operations in [`BOUND_OPERATIONS`] are bound, and only the input members each
//!   entry names. An operation or a member this seam does not bind is reported as a call that
//!   was NOT MADE, naming it, rather than sent with a value dropped — a dropped member would
//!   make a call that did something other than what the handler asked for.
//! * The answer's own status and document are what Amazon SNS answered: a success is that
//!   operation's output members, and a refusal is its own error code, message and status, which
//!   the SDK the guest calls raises as `ClientError` ([`super::aws_sdk_guest`]).
//!
//! How one call's input is read, what the answer carried and how a refusal is handed back are
//! the same for every service reached through an SDK client, so they are [`super::sdk_call`]'s
//! rather than this module's.

use ::aws_sdk_sns::Client;
use ::serde_json::{Map, Value};
use ::smithy_server_runtime::EmbeddedEmulator;

use super::aws_api::{Answer, CallerIdentity, Refusal};
use super::sdk_call::{insert_text, insert_text_map, Input, Observed, PROVIDER_NAME};

/// The service a refusal of one of these calls names, as the AWS SDKs name it.
const SERVICE: &str = "Amazon SNS";

/// The operations of Amazon SNS this seam binds, in the order a refusal lists them.
///
/// One entry per operation [`call`] builds a request for: the topic surface a handler that
/// notifies the rest of a system uses — publishing one message or a batch, managing the topics
/// and subscriptions the fan-out stands on, reading and setting their attributes, and the tags
/// on either. A handler naming any other operation of the service is told the call was not made
/// and why, because a request this module cannot bind is one it would otherwise be guessing at.
pub const BOUND_OPERATIONS: &[&str] = &[
    "Publish",
    "PublishBatch",
    "CreateTopic",
    "DeleteTopic",
    "ListTopics",
    "GetTopicAttributes",
    "SetTopicAttributes",
    "Subscribe",
    "Unsubscribe",
    "ConfirmSubscription",
    "ListSubscriptions",
    "ListSubscriptionsByTopic",
    "GetSubscriptionAttributes",
    "SetSubscriptionAttributes",
    "AddPermission",
    "RemovePermission",
    "TagResource",
    "UntagResource",
    "ListTagsForResource",
];

/// Make ONE Amazon SNS call: `operation`, with `body` as the operation's input document (the
/// keyword arguments the handler passed its client method), in `region`.
pub async fn call(
    callback: &EmbeddedEmulator,
    identity: Option<&CallerIdentity>,
    region: &str,
    operation: &str,
    body: &str,
) -> Result<Answer, Refusal> {
    let input = Input::read(SERVICE, body)?;
    let observed = Observed::of(SERVICE);
    let client = client(callback, identity, region, observed.clone());
    match operation {
        "Publish" => publish(&client, &input, &observed).await,
        "PublishBatch" => publish_batch(&client, &input, &observed).await,
        "CreateTopic" => create_topic(&client, &input, &observed).await,
        "DeleteTopic" => delete_topic(&client, &input, &observed).await,
        "ListTopics" => list_topics(&client, &input, &observed).await,
        "GetTopicAttributes" => get_topic_attributes(&client, &input, &observed).await,
        "SetTopicAttributes" => set_topic_attributes(&client, &input, &observed).await,
        "Subscribe" => subscribe(&client, &input, &observed).await,
        "Unsubscribe" => unsubscribe(&client, &input, &observed).await,
        "ConfirmSubscription" => confirm_subscription(&client, &input, &observed).await,
        "ListSubscriptions" => list_subscriptions(&client, &input, &observed).await,
        "ListSubscriptionsByTopic" => {
            list_subscriptions_by_topic(&client, &input, &observed).await
        }
        "GetSubscriptionAttributes" => {
            get_subscription_attributes(&client, &input, &observed).await
        }
        "SetSubscriptionAttributes" => {
            set_subscription_attributes(&client, &input, &observed).await
        }
        "AddPermission" => add_permission(&client, &input, &observed).await,
        "RemovePermission" => remove_permission(&client, &input, &observed).await,
        "TagResource" => tag_resource(&client, &input, &observed).await,
        "UntagResource" => untag_resource(&client, &input, &observed).await,
        "ListTagsForResource" => list_tags_for_resource(&client, &input, &observed).await,
        other => Err(Refusal::NotMade(format!(
            "the AWS SDK this runtime supplies binds no {other} call on {SERVICE}; the \
             operations it binds are {}",
            BOUND_OPERATIONS.join(", ")
        ))),
    }
}

/// An Amazon SNS client over the in-process callback into this same emulator, signing as the
/// identity this invocation was given.
///
/// The transport is in-process, so everything a network client needs and this one cannot have —
/// retries, timeouts, identity caching and stalled-stream protection, each of which needs a
/// sleep implementation the callback transport is not given — is switched off.
fn client(
    callback: &EmbeddedEmulator,
    identity: Option<&CallerIdentity>,
    region: &str,
    observed: Observed,
) -> Client {
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://sns.{region}.amazonaws.com"))
        .http_client(callback.http_client())
        .interceptor(observed)
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled(),
        );
    let config = match identity {
        Some(identity) => {
            config.credentials_provider(::aws_sdk_sns::config::Credentials::new(
                identity.access_key_id.clone(),
                identity.secret_access_key.clone(),
                Some(identity.session_token.clone()),
                None,
                PROVIDER_NAME,
            ))
        }
        None => config.credentials_provider(callback.aws_credentials_provider()),
    };
    Client::from_conf(config.build())
}

/// `Publish`: one message sent to a topic, a mobile endpoint or a phone number.
async fn publish(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "TopicArn",
        "TargetArn",
        "PhoneNumber",
        "Message",
        "Subject",
        "MessageStructure",
        "MessageAttributes",
        "MessageDeduplicationId",
        "MessageGroupId",
    ])?;
    let mut sending = client
        .publish()
        .set_topic_arn(input.text("TopicArn")?)
        .set_target_arn(input.text("TargetArn")?)
        .set_phone_number(input.text("PhoneNumber")?)
        .message(input.required_text("Message")?)
        .set_subject(input.text("Subject")?)
        .set_message_structure(input.text("MessageStructure")?)
        .set_message_deduplication_id(input.text("MessageDeduplicationId")?)
        .set_message_group_id(input.text("MessageGroupId")?);
    for (name, value) in input.structure_map("MessageAttributes")? {
        sending = sending.message_attributes(name, message_attribute(&value)?);
    }
    let published = match sending.send().await {
        Ok(published) => published,
        Err(error) => return observed.refused(&error),
    };
    let mut document = Map::new();
    insert_text(&mut document, "MessageId", published.message_id());
    insert_text(&mut document, "SequenceNumber", published.sequence_number());
    observed.answer(document)
}

/// `PublishBatch`: up to ten messages sent to one topic in a single call.
async fn publish_batch(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["TopicArn", "PublishBatchRequestEntries"])?;
    let mut entries = Vec::new();
    for entry in input.required_structures("PublishBatchRequestEntries")? {
        entry.only(&[
            "Id",
            "Message",
            "Subject",
            "MessageStructure",
            "MessageAttributes",
            "MessageDeduplicationId",
            "MessageGroupId",
        ])?;
        let mut built = ::aws_sdk_sns::types::PublishBatchRequestEntry::builder()
            .id(entry.required_text("Id")?)
            .message(entry.required_text("Message")?)
            .set_subject(entry.text("Subject")?)
            .set_message_structure(entry.text("MessageStructure")?)
            .set_message_deduplication_id(entry.text("MessageDeduplicationId")?)
            .set_message_group_id(entry.text("MessageGroupId")?);
        for (name, value) in entry.structure_map("MessageAttributes")? {
            built = built.message_attributes(name, message_attribute(&value)?);
        }
        entries.push(built.build().map_err(|error| {
            Refusal::NotMade(format!(
                "an entry of the call's batch is not one {SERVICE} accepts: {error}"
            ))
        })?);
    }
    let sent = client
        .publish_batch()
        .topic_arn(input.required_text("TopicArn")?)
        .set_publish_batch_request_entries(Some(entries))
        .send()
        .await;
    let published = match sent {
        Ok(published) => published,
        Err(error) => return observed.refused(&error),
    };
    let successful: Vec<Value> = published
        .successful()
        .iter()
        .map(|entry| {
            let mut held = Map::new();
            insert_text(&mut held, "Id", entry.id());
            insert_text(&mut held, "MessageId", entry.message_id());
            insert_text(&mut held, "SequenceNumber", entry.sequence_number());
            Value::Object(held)
        })
        .collect();
    let failed: Vec<Value> = published
        .failed()
        .iter()
        .map(|entry| {
            let mut held = Map::new();
            insert_text(&mut held, "Id", Some(entry.id()));
            insert_text(&mut held, "Code", Some(entry.code()));
            insert_text(&mut held, "Message", entry.message());
            held.insert(
                "SenderFault".to_string(),
                Value::Bool(entry.sender_fault()),
            );
            Value::Object(held)
        })
        .collect();
    let mut document = Map::new();
    document.insert("Successful".to_string(), Value::Array(successful));
    document.insert("Failed".to_string(), Value::Array(failed));
    observed.answer(document)
}

/// One `MessageAttributeValue` as the call carries it: its type, its text and its bytes.
fn message_attribute(
    value: &Input,
) -> Result<::aws_sdk_sns::types::MessageAttributeValue, Refusal> {
    value.only(&["DataType", "StringValue", "BinaryValue"])?;
    ::aws_sdk_sns::types::MessageAttributeValue::builder()
        .data_type(value.required_text("DataType")?)
        .set_string_value(value.text("StringValue")?)
        .set_binary_value(
            value
                .blob("BinaryValue")?
                .map(::aws_sdk_sns::primitives::Blob::new),
        )
        .build()
        .map_err(|error| {
            Refusal::NotMade(format!(
                "a message attribute of the call is not one {SERVICE} accepts: {error}"
            ))
        })
}

/// `CreateTopic`: the topic a handler publishes to, with the attributes and tags it names.
async fn create_topic(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Name", "Attributes", "Tags", "DataProtectionPolicy"])?;
    let sent = client
        .create_topic()
        .name(input.required_text("Name")?)
        .set_attributes(input.text_map("Attributes")?)
        .set_tags(tags(input, "Tags")?)
        .set_data_protection_policy(input.text("DataProtectionPolicy")?)
        .send()
        .await;
    let created = match sent {
        Ok(created) => created,
        Err(error) => return observed.refused(&error),
    };
    let mut document = Map::new();
    insert_text(&mut document, "TopicArn", created.topic_arn());
    observed.answer(document)
}

/// `DeleteTopic`: the topic and every subscription to it, removed.
async fn delete_topic(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["TopicArn"])?;
    let sent = client
        .delete_topic()
        .topic_arn(input.required_text("TopicArn")?)
        .send()
        .await;
    match sent {
        Ok(_deleted) => observed.answer(Map::new()),
        Err(error) => observed.refused(&error),
    }
}

/// `ListTopics`: one page of the account's topics in this Region.
async fn list_topics(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["NextToken"])?;
    let sent = client
        .list_topics()
        .set_next_token(input.text("NextToken")?)
        .send()
        .await;
    let listed = match sent {
        Ok(listed) => listed,
        Err(error) => return observed.refused(&error),
    };
    let topics: Vec<Value> = listed
        .topics()
        .iter()
        .map(|topic| {
            let mut held = Map::new();
            insert_text(&mut held, "TopicArn", topic.topic_arn());
            Value::Object(held)
        })
        .collect();
    let mut document = Map::new();
    document.insert("Topics".to_string(), Value::Array(topics));
    insert_text(&mut document, "NextToken", listed.next_token());
    observed.answer(document)
}

/// `GetTopicAttributes`: every property the topic reports.
async fn get_topic_attributes(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["TopicArn"])?;
    let sent = client
        .get_topic_attributes()
        .topic_arn(input.required_text("TopicArn")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return observed.refused(&error),
    };
    let mut document = Map::new();
    insert_text_map(&mut document, "Attributes", read.attributes());
    observed.answer(document)
}

/// `SetTopicAttributes`: one property of the topic set to a new value.
async fn set_topic_attributes(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["TopicArn", "AttributeName", "AttributeValue"])?;
    let sent = client
        .set_topic_attributes()
        .topic_arn(input.required_text("TopicArn")?)
        .attribute_name(input.required_text("AttributeName")?)
        .set_attribute_value(input.text("AttributeValue")?)
        .send()
        .await;
    match sent {
        Ok(_set) => observed.answer(Map::new()),
        Err(error) => observed.refused(&error),
    }
}

/// `Subscribe`: an endpoint subscribed to a topic, which is what a fan-out stands on.
async fn subscribe(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "TopicArn",
        "Protocol",
        "Endpoint",
        "Attributes",
        "ReturnSubscriptionArn",
    ])?;
    let sent = client
        .subscribe()
        .topic_arn(input.required_text("TopicArn")?)
        .protocol(input.required_text("Protocol")?)
        .set_endpoint(input.text("Endpoint")?)
        .set_attributes(input.text_map("Attributes")?)
        .set_return_subscription_arn(input.boolean("ReturnSubscriptionArn")?)
        .send()
        .await;
    let subscribed = match sent {
        Ok(subscribed) => subscribed,
        Err(error) => return observed.refused(&error),
    };
    let mut document = Map::new();
    insert_text(
        &mut document,
        "SubscriptionArn",
        subscribed.subscription_arn(),
    );
    observed.answer(document)
}

/// `Unsubscribe`: the subscription removed, which stops its deliveries.
async fn unsubscribe(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["SubscriptionArn"])?;
    let sent = client
        .unsubscribe()
        .subscription_arn(input.required_text("SubscriptionArn")?)
        .send()
        .await;
    match sent {
        Ok(_removed) => observed.answer(Map::new()),
        Err(error) => observed.refused(&error),
    }
}

/// `ConfirmSubscription`: the token an endpoint was sent, exchanged for the subscription.
async fn confirm_subscription(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["TopicArn", "Token", "AuthenticateOnUnsubscribe"])?;
    let sent = client
        .confirm_subscription()
        .topic_arn(input.required_text("TopicArn")?)
        .token(input.required_text("Token")?)
        .set_authenticate_on_unsubscribe(input.text("AuthenticateOnUnsubscribe")?)
        .send()
        .await;
    let confirmed = match sent {
        Ok(confirmed) => confirmed,
        Err(error) => return observed.refused(&error),
    };
    let mut document = Map::new();
    insert_text(
        &mut document,
        "SubscriptionArn",
        confirmed.subscription_arn(),
    );
    observed.answer(document)
}

/// `ListSubscriptions`: one page of the account's subscriptions in this Region.
async fn list_subscriptions(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["NextToken"])?;
    let sent = client
        .list_subscriptions()
        .set_next_token(input.text("NextToken")?)
        .send()
        .await;
    let listed = match sent {
        Ok(listed) => listed,
        Err(error) => return observed.refused(&error),
    };
    let mut document = Map::new();
    document.insert(
        "Subscriptions".to_string(),
        Value::Array(subscriptions(listed.subscriptions())),
    );
    insert_text(&mut document, "NextToken", listed.next_token());
    observed.answer(document)
}

/// `ListSubscriptionsByTopic`: one page of the subscriptions to one topic.
async fn list_subscriptions_by_topic(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["TopicArn", "NextToken"])?;
    let sent = client
        .list_subscriptions_by_topic()
        .topic_arn(input.required_text("TopicArn")?)
        .set_next_token(input.text("NextToken")?)
        .send()
        .await;
    let listed = match sent {
        Ok(listed) => listed,
        Err(error) => return observed.refused(&error),
    };
    let mut document = Map::new();
    document.insert(
        "Subscriptions".to_string(),
        Value::Array(subscriptions(listed.subscriptions())),
    );
    insert_text(&mut document, "NextToken", listed.next_token());
    observed.answer(document)
}

/// `GetSubscriptionAttributes`: every property the subscription reports.
async fn get_subscription_attributes(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["SubscriptionArn"])?;
    let sent = client
        .get_subscription_attributes()
        .subscription_arn(input.required_text("SubscriptionArn")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return observed.refused(&error),
    };
    let mut document = Map::new();
    insert_text_map(&mut document, "Attributes", read.attributes());
    observed.answer(document)
}

/// `SetSubscriptionAttributes`: one property of the subscription set to a new value.
async fn set_subscription_attributes(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["SubscriptionArn", "AttributeName", "AttributeValue"])?;
    let sent = client
        .set_subscription_attributes()
        .subscription_arn(input.required_text("SubscriptionArn")?)
        .attribute_name(input.required_text("AttributeName")?)
        .set_attribute_value(input.text("AttributeValue")?)
        .send()
        .await;
    match sent {
        Ok(_set) => observed.answer(Map::new()),
        Err(error) => observed.refused(&error),
    }
}

/// `AddPermission`: a statement added to the topic's own access policy.
async fn add_permission(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["TopicArn", "Label", "AWSAccountId", "ActionName"])?;
    let sent = client
        .add_permission()
        .topic_arn(input.required_text("TopicArn")?)
        .label(input.required_text("Label")?)
        .set_aws_account_id(Some(input.required_text_list("AWSAccountId")?))
        .set_action_name(Some(input.required_text_list("ActionName")?))
        .send()
        .await;
    match sent {
        Ok(_added) => observed.answer(Map::new()),
        Err(error) => observed.refused(&error),
    }
}

/// `RemovePermission`: that statement removed again.
async fn remove_permission(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["TopicArn", "Label"])?;
    let sent = client
        .remove_permission()
        .topic_arn(input.required_text("TopicArn")?)
        .label(input.required_text("Label")?)
        .send()
        .await;
    match sent {
        Ok(_removed) => observed.answer(Map::new()),
        Err(error) => observed.refused(&error),
    }
}

/// `TagResource`: the tags added to a topic.
async fn tag_resource(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["ResourceArn", "Tags"])?;
    let added = tags(input, "Tags")?.ok_or_else(|| {
        Refusal::NotMade(format!(
            "the call carries no Tags, which this {SERVICE} operation requires"
        ))
    })?;
    let sent = client
        .tag_resource()
        .resource_arn(input.required_text("ResourceArn")?)
        .set_tags(Some(added))
        .send()
        .await;
    match sent {
        Ok(_tagged) => observed.answer(Map::new()),
        Err(error) => observed.refused(&error),
    }
}

/// `UntagResource`: the tags removed from it again.
async fn untag_resource(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["ResourceArn", "TagKeys"])?;
    let sent = client
        .untag_resource()
        .resource_arn(input.required_text("ResourceArn")?)
        .set_tag_keys(Some(input.required_text_list("TagKeys")?))
        .send()
        .await;
    match sent {
        Ok(_untagged) => observed.answer(Map::new()),
        Err(error) => observed.refused(&error),
    }
}

/// `ListTagsForResource`: the tags a topic carries now.
async fn list_tags_for_resource(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["ResourceArn"])?;
    let sent = client
        .list_tags_for_resource()
        .resource_arn(input.required_text("ResourceArn")?)
        .send()
        .await;
    let listed = match sent {
        Ok(listed) => listed,
        Err(error) => return observed.refused(&error),
    };
    let held: Vec<Value> = listed
        .tags()
        .iter()
        .map(|tag| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Key", Some(tag.key()));
            insert_text(&mut entry, "Value", Some(tag.value()));
            Value::Object(entry)
        })
        .collect();
    let mut document = Map::new();
    document.insert("Tags".to_string(), Value::Array(held));
    observed.answer(document)
}

/// The `Tag` list one call carries, or `None` where it carries the member not at all.
fn tags(input: &Input, name: &str) -> Result<Option<Vec<::aws_sdk_sns::types::Tag>>, Refusal> {
    let entries = input.structures(name)?;
    if entries.is_empty() {
        return Ok(None);
    }
    let mut built = Vec::with_capacity(entries.len());
    for entry in entries {
        entry.only(&["Key", "Value"])?;
        built.push(
            ::aws_sdk_sns::types::Tag::builder()
                .key(entry.required_text("Key")?)
                .value(entry.required_text("Value")?)
                .build()
                .map_err(|error| {
                    Refusal::NotMade(format!(
                        "a tag of the call is not one {SERVICE} accepts: {error}"
                    ))
                })?,
        );
    }
    Ok(Some(built))
}

/// The subscriptions one page of a listing carries, as the answer's document holds them.
fn subscriptions(listed: &[::aws_sdk_sns::types::Subscription]) -> Vec<Value> {
    listed
        .iter()
        .map(|subscription| {
            let mut held = Map::new();
            insert_text(
                &mut held,
                "SubscriptionArn",
                subscription.subscription_arn(),
            );
            insert_text(&mut held, "Owner", subscription.owner());
            insert_text(&mut held, "Protocol", subscription.protocol());
            insert_text(&mut held, "Endpoint", subscription.endpoint());
            insert_text(&mut held, "TopicArn", subscription.topic_arn());
            Value::Object(held)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::BOUND_OPERATIONS;

    /// Every operation this seam binds is named once, so the catalog a refusal lists is a list
    /// of operations rather than of spellings.
    #[test]
    fn every_bound_operation_is_named_once() {
        let mut names = BOUND_OPERATIONS.to_vec();
        names.sort_unstable();
        let total = names.len();
        names.dedup();
        assert_eq!(
            names.len(),
            total,
            "case \"the bound operations\": expected each operation to be named once, got \
             {BOUND_OPERATIONS:?}"
        );
    }
}
