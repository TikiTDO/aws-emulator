// SPDX-License-Identifier: CC0-1.0

//! DELIVERY-STATUS LOGGING: the record one attempted delivery leaves in CloudWatch Logs
//! when the topic is configured to log it.
//!
//! "The following attributes apply only to message delivery status: `<Protocol>`
//! SuccessFeedbackRoleArn – Indicates successful message delivery status for an Amazon SNS
//! topic that is subscribed to an endpoint of that kind; `<Protocol>`FailureFeedbackRoleArn
//! – Indicates failed message delivery status; `<Protocol>`SuccessFeedbackSampleRate –
//! Indicates percentage of successful messages to sample"
//! (`SetTopicAttributesInput$AttributeName`).
//!
//! The rule those attributes express: a topic that names a feedback role for a protocol has
//! Amazon SNS ASSUME that role and write one record per delivery over that protocol into the
//! CloudWatch Logs log group [`log_group_name`] names — a successful delivery through the
//! success role, at the sampled percentage, and a failed one through the failure role,
//! always. A topic that names no role for the protocol a delivery went over logs nothing,
//! which is why the feedback attributes are a per-protocol pair rather than one switch.
//!
//! What makes this the role being EXERCISED rather than merely stored: the role is resolved
//! in the account's own IAM records, its trust policy has to admit this service's principal
//! ([`crate::state::SERVICE_PRINCIPAL`]) for `sts:AssumeRole` — through the tree's one
//! reading of a trust document (`service_common::iam_trust_policy`) — and the permissions in
//! force ON that role have to allow the CloudWatch Logs calls the write makes
//! ([`LOG_WRITE_ACTIONS`]), which is the question `service_common::iam_role` puts to IAM
//! itself. A role Amazon SNS cannot act as, and one it can act as but that grants the write
//! nothing, are both configurations under which the real service writes nothing and creates
//! no log group at all, while the DELIVERY itself is unaffected: the record reports a
//! delivery that has already been attempted.
//!
//! The write goes through the emulator's OWN CloudWatch Logs service over the in-process
//! callback, so the record lands in the very store `logs:GetLogEvents` reads. It is BEST
//! EFFORT and never fatal: the publish that attempted the delivery is answered either way,
//! because the record reports a delivery that already happened.
//!
//! The REQUIREMENT this module reproduces is the log group's name template — the base group
//! for a success and that group's `/Failure` child for a failure — and a record per delivery
//! naming the endpoint, the delivery's status, the HTTP status the endpoint's own service
//! answered with and that service's own answer; the member set around them is the form AWS
//! publishes for a delivery-status record in the Amazon SNS Developer Guide (a `notification`
//! block naming the published message by id, topic, publish time and CONTENT DIGEST, a
//! `delivery` block and a `status`), and the log STREAM's name is this emulator's, pinned by
//! no recording.

use ::std::time::SystemTime;

use super::{rfc3339, PublishedMessage, TopicRecord, SERVICE_PRINCIPAL};

/// The protocol-independent half of a feedback attribute name, for the success role.
const SUCCESS_FEEDBACK_ROLE_SUFFIX: &str = "SuccessFeedbackRoleArn";

/// The same for the role a FAILED delivery is logged under.
const FAILURE_FEEDBACK_ROLE_SUFFIX: &str = "FailureFeedbackRoleArn";

/// The same for the percentage of SUCCESSFUL deliveries that are logged.
const SUCCESS_FEEDBACK_SAMPLE_RATE_SUFFIX: &str = "SuccessFeedbackSampleRate";

/// The attribute-name prefix each subscription protocol's feedback attributes carry, from
/// the names [`crate::state::TOPIC_ATTRIBUTE_NAMES`] holds: `Application`, `Firehose`,
/// `HTTP`, `Lambda` and `SQS`.
///
/// `http` and `https` share one prefix — the topic carries `HTTPSuccessFeedbackRoleArn` and
/// no `HTTPS…` variant — and a protocol with no feedback attributes at all (`email`,
/// `email-json`, `sms`) has no prefix, so nothing is logged for it.
fn feedback_attribute_prefix(protocol: &str) -> Option<&'static str> {
    match protocol {
        "sqs" => Some("SQS"),
        "lambda" => Some("Lambda"),
        "http" | "https" => Some("HTTP"),
        "firehose" => Some("Firehose"),
        "application" => Some("Application"),
        _ => None,
    }
}

/// The percentage of successful deliveries a topic that names no
/// `<Protocol>SuccessFeedbackSampleRate` logs.
///
/// The sample rate is an attribute of its own, so a topic that names a success role and no
/// rate has asked for no sample of its successes: the rate decides how many are logged, and
/// an absent one samples none. A FAILED delivery has no sample rate and is always logged.
const DEFAULT_SUCCESS_SAMPLE_RATE: u32 = 0;

/// What one topic asks to have logged about deliveries over ONE protocol.
///
/// Read off the topic record while the store is locked, and carried with the delivery, so
/// the write that follows needs nothing from the store.
#[derive(Debug, Clone)]
pub struct Feedback {
    /// `<Protocol>SuccessFeedbackRoleArn`: the role a SUCCESSFUL delivery's record is
    /// written as, and `None` when the topic names none.
    pub success_role_arn: Option<String>,
    /// `<Protocol>FailureFeedbackRoleArn`: the role a FAILED delivery's record is written
    /// as.
    pub failure_role_arn: Option<String>,
    /// `<Protocol>SuccessFeedbackSampleRate`, as a percentage.
    pub success_sample_rate: u32,
    /// The log group the records go into: [`log_group_name`] for this topic.
    pub log_group: String,
}

/// What `topic` asks to have logged about its deliveries over `protocol`, or `None` when it
/// asks for nothing — the protocol has no feedback attributes, or the topic names no role
/// for it, in which case there is no role to assume and nothing to write.
pub fn feedback_of(topic: &TopicRecord, protocol: &str) -> Option<Feedback> {
    let prefix = feedback_attribute_prefix(protocol)?;
    let attribute = |suffix: &str| topic.attributes.get(&format!("{prefix}{suffix}")).cloned();
    let success_role_arn = attribute(SUCCESS_FEEDBACK_ROLE_SUFFIX);
    let failure_role_arn = attribute(FAILURE_FEEDBACK_ROLE_SUFFIX);
    if success_role_arn.is_none() && failure_role_arn.is_none() {
        return None;
    }
    Some(Feedback {
        success_role_arn,
        failure_role_arn,
        success_sample_rate: attribute(SUCCESS_FEEDBACK_SAMPLE_RATE_SUFFIX)
            .and_then(|rate| rate.trim().parse::<u32>().ok())
            .unwrap_or(DEFAULT_SUCCESS_SAMPLE_RATE),
        log_group: log_group_name(topic),
    })
}

/// The CloudWatch Logs log group a topic's delivery-status records go into:
/// `sns/<region>/<account-id>/<topic-name>`.
///
/// The Region is the one the topic's own ARN names, so a topic's records are logged in the
/// Region the topic is in; the account is the topic's owner.
pub fn log_group_name(topic: &TopicRecord) -> String {
    let region = super::arn_region(&topic.arn).unwrap_or(super::DEFAULT_REGION);
    format!("sns/{region}/{}/{}", topic.owner, topic.name)
}

/// What a FAILED delivery's log group carries beyond the topic's own
/// ([`log_group_name`]): the records of successes and of failures are read apart, so a
/// failure is written to that group's `/Failure` child rather than into the group a success
/// goes to.
const FAILURE_LOG_GROUP_SUFFIX: &str = "/Failure";

/// What came of one delivery, as the record reports it under `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The endpoint took the message.
    Success,
    /// It did not: the endpoint refused it, or could not be reached at all.
    Failure,
}

impl Status {
    /// The `status` member's value, as a delivery-status record carries it.
    fn reported(self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
        }
    }

    /// The log group a record of this outcome goes into, given the topic's own group
    /// `base`: that group for a success, and its [`FAILURE_LOG_GROUP_SUFFIX`] child for a
    /// failure.
    fn log_group(self, base: &str) -> String {
        match self {
            Self::Success => base.to_string(),
            Self::Failure => format!("{base}{FAILURE_LOG_GROUP_SUFFIX}"),
        }
    }
}

/// What the endpoint's OWN service answered about one attempted delivery, which the record
/// reports beside the outcome.
///
/// The two members are that answer and nothing else: a status no service answered and a
/// sentence no service gave are not reported, so each is left out of the record when the
/// attempt reached no answer carrying it.
#[derive(Debug, Clone, Default)]
pub struct Answered {
    /// The HTTP status the endpoint's service answered with, as the record's `statusCode`.
    pub status_code: Option<u16>,
    /// That service's own answer about the delivery, as the record's `providerResponse`:
    /// the identifiers it reported for a message it took, or the error document it refused
    /// with.
    pub provider_response: Option<String>,
}

/// One attempted delivery, as its delivery-status record reports it.
#[derive(Debug, Clone)]
struct Attempt<'a> {
    /// The published message the delivery carried.
    message_id: &'a str,
    /// The topic it was published to.
    topic_arn: &'a str,
    /// The subscription that owed the delivery: what the record's delivery id is derived
    /// from, so one message's records are told apart per subscriber.
    subscription_arn: &'a str,
    /// The digest of the published message's body, which the record reports beside the
    /// message's id so a reader can tell WHICH payload the attempt carried.
    message_md5: &'a str,
    /// The endpoint the message was delivered to, as the subscription names it — the
    /// `destination` the record reports.
    endpoint: &'a str,
    /// When the message was published, which is what the dwell time is measured from.
    published_at: SystemTime,
    /// What came of the attempt.
    status: Status,
    /// What the endpoint's own service answered about it.
    answered: &'a Answered,
}

/// The record `attempt` writes, as one JSON log event.
///
/// The form AWS publishes for a delivery-status record: a `notification` block naming the
/// published message, a `delivery` block naming the endpoint it was delivered to and what
/// came of the attempt, and the `status` of the delivery. `dwellTimeMs` is the real span
/// between the publish and this attempt, and `deliveryId` is derived from the subscription
/// and the message, so one published message's records are distinct per subscriber and
/// stable across reads.
///
/// `statusCode` and `providerResponse` are the endpoint service's OWN answer, carried here
/// from the attempt ([`Answered`]); an attempt that reached no answer carrying one leaves
/// that member out rather than reporting a status or a sentence no service gave.
fn rendered(attempt: &Attempt<'_>, at: SystemTime) -> String {
    let mut notification = ::serde_json::Map::new();
    notification.insert(
        "messageId".to_string(),
        ::serde_json::Value::from(attempt.message_id),
    );
    notification.insert(
        "topicArn".to_string(),
        ::serde_json::Value::from(attempt.topic_arn),
    );
    notification.insert(
        "messageMD5Sum".to_string(),
        ::serde_json::Value::from(attempt.message_md5),
    );
    if let Some(timestamp) = rfc3339(attempt.published_at) {
        notification.insert("timestamp".to_string(), ::serde_json::Value::from(timestamp));
    }

    let mut delivery = ::serde_json::Map::new();
    delivery.insert(
        "deliveryId".to_string(),
        ::serde_json::Value::from(delivery_id(attempt)),
    );
    delivery.insert(
        "destination".to_string(),
        ::serde_json::Value::from(attempt.endpoint),
    );
    delivery.insert(
        "dwellTimeMs".to_string(),
        ::serde_json::Value::from(dwell_time_ms(attempt.published_at, at)),
    );
    // One attempt is what this emulator makes of a delivery, so that is what the record
    // reports rather than a retry count nothing performed.
    delivery.insert("attempts".to_string(), ::serde_json::Value::from(1));
    if let Some(status_code) = attempt.answered.status_code {
        delivery.insert(
            "statusCode".to_string(),
            ::serde_json::Value::from(status_code),
        );
    }
    if let Some(response) = attempt.answered.provider_response.as_deref() {
        delivery.insert(
            "providerResponse".to_string(),
            ::serde_json::Value::from(response),
        );
    }

    let mut record = ::serde_json::Map::new();
    record.insert(
        "notification".to_string(),
        ::serde_json::Value::Object(notification),
    );
    record.insert("delivery".to_string(), ::serde_json::Value::Object(delivery));
    record.insert(
        "status".to_string(),
        ::serde_json::Value::from(attempt.status.reported()),
    );
    ::serde_json::Value::Object(record).to_string()
}

/// The `messageMD5Sum` of a published message: the MD5 digest of its body, in lowercase
/// hexadecimal.
///
/// A digest of the CONTENT, so every record of one published message — one per subscriber,
/// over every protocol — reports the same value, two publishes of the same body report the
/// same value, and the value changes only when the body does. The body is the one `Publish`
/// received, which is what the record's `notification` block names.
///
/// The digest function is the one the member's own name states, and the value is a checksum a
/// reader compares against its own copy of the body: nothing is authenticated or protected by
/// it, here or anywhere a client reads it.
fn message_md5(message: &str) -> String {
    use ::md5::{Digest, Md5};

    ::hex::encode(Md5::digest(message.as_bytes()))
}

/// The digest of ONE published message's body, taken at most once however many subscriptions
/// that message fans out to.
///
/// The digest is a property of the PUBLISH rather than of a delivery: one publish is
/// delivered to every subscription of the topic and each delivery's record reports the same
/// value, so one pass over the body serves the whole fan-out. Held per publish and read on
/// first use, so a publish no subscription logs takes no pass at all, and one that logs N
/// deliveries still takes one — what a publish costs here does not grow with the number of
/// subscribers.
#[derive(Debug)]
pub struct MessageDigest<'a> {
    /// The published body the digest is taken of.
    message: &'a str,
    /// That digest, once something has asked for it.
    digest: ::std::cell::OnceCell<String>,
}

impl<'a> MessageDigest<'a> {
    /// The digest of `published`'s body, not yet taken.
    pub fn of(published: &'a PublishedMessage) -> Self {
        Self {
            message: &published.message,
            digest: ::std::cell::OnceCell::new(),
        }
    }

    /// That digest, taken now if this is the first ask.
    fn get(&self) -> &str {
        self.digest.get_or_init(|| message_md5(self.message))
    }
}

/// How many hexadecimal characters a record's `deliveryId` carries. Half a SHA-256 digest
/// is far more than enough for one account's deliveries to stay distinct, and the id is
/// opaque: nothing is read off it.
const DELIVERY_ID_HEX_CHARS: usize = 32;

/// The `deliveryId` of one attempt: a digest of the subscription that owed the delivery and
/// the message it carried.
///
/// Derived rather than minted, so the same delivery reports the same id however many times
/// it is rendered, and two subscribers of one published message report different ones.
fn delivery_id(attempt: &Attempt<'_>) -> String {
    use ::sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    digest.update(attempt.subscription_arn.as_bytes());
    digest.update([0]);
    digest.update(attempt.message_id.as_bytes());
    let hex = ::hex::encode(digest.finalize());
    hex[..DELIVERY_ID_HEX_CHARS.min(hex.len())].to_string()
}

/// How long the message waited between being published and this delivery attempt, in
/// milliseconds — an attempt stamped before the publish dwelled for no time at all.
fn dwell_time_ms(published_at: SystemTime, at: SystemTime) -> u64 {
    at.duration_since(published_at)
        .map(|span| u64::try_from(span.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// Whether a SUCCESSFUL delivery of `message_id` is one of the sampled percentage
/// `sample_rate` names.
///
/// "Indicates percentage of successful messages to sample" — a rate of 0 samples none and a
/// rate of 100 samples every one. In between, the decision is taken from a digest of the
/// message's own id, so it is spread over the published messages without a random source and
/// one message's records do not change between reads.
fn is_sampled(message_id: &str, sample_rate: u32) -> bool {
    use ::sha2::{Digest, Sha256};

    let rate = sample_rate.min(100);
    if rate == 0 {
        return false;
    }
    if rate >= 100 {
        return true;
    }
    let digest = Sha256::digest(message_id.as_bytes());
    let bucket = u32::from(digest[0]) % 100;
    bucket < rate
}

/// The role `feedback` asks a record of `status` to be written as, when it asks for one at
/// all: the success role at the sampled percentage, or the failure role, which has no sample
/// rate because every failure is logged.
fn feedback_role<'a>(
    feedback: &'a Feedback,
    status: Status,
    message_id: &str,
) -> Option<&'a String> {
    match status {
        Status::Success => feedback
            .success_role_arn
            .as_ref()
            .filter(|_| is_sampled(message_id, feedback.success_sample_rate)),
        Status::Failure => feedback.failure_role_arn.as_ref(),
    }
}

/// Everything a delivery carries so that its record can be written once the attempt has been
/// made: what the topic asks to have logged about deliveries over that subscription's
/// protocol, and the published message the record names.
///
/// Read off the topic record while the store is locked and carried with the delivery, so the
/// write that follows the attempt needs nothing from the store.
#[derive(Debug, Clone)]
pub struct Logged {
    /// What the topic asks to have logged about this protocol's deliveries.
    feedback: Feedback,
    /// The published message the delivery carries.
    message_id: String,
    /// The digest of that message's body, as the record reports it.
    message_md5: String,
    /// When it was published: what the record's dwell time is measured from.
    published_at: SystemTime,
}

impl Logged {
    /// What `topic` asks to have logged about `published`'s delivery over `protocol`, or
    /// `None` when it asks for nothing — there is no topic (a replay of one that is no longer
    /// there), the protocol has no feedback attributes, or the topic names no role for it.
    ///
    /// `digest` is the published body's digest, shared across the whole fan-out of one publish
    /// ([`MessageDigest`]), and is read only for a delivery that IS logged.
    pub fn of(
        topic: Option<&TopicRecord>,
        protocol: &str,
        published: &PublishedMessage,
        digest: &MessageDigest<'_>,
    ) -> Option<Self> {
        let feedback = feedback_of(topic?, protocol)?;
        Some(Self {
            feedback,
            message_id: published.message_id.clone(),
            message_md5: digest.get().to_string(),
            published_at: published.published_at,
        })
    }

    /// Write the record of one attempted delivery to `endpoint`, made on behalf of
    /// `subscription_arn` for a message published to `topic_arn`, with `answered` the answer
    /// the endpoint's own service gave.
    ///
    /// Nothing is written when the topic asks for no record of this outcome (no role for it,
    /// or a success outside the sampled percentage), when the role it names is not one of this
    /// account's, when that role's trust policy does not let this service assume it, or when
    /// the permissions in force on that role do not allow the CloudWatch Logs write: each of
    /// those is a configuration under which the real service writes nothing either, and
    /// creates no log group.
    ///
    /// Best effort: a CloudWatch Logs write that fails is traced and changes nothing about
    /// what the publishing call answers. Call it with NO store guard held — it awaits.
    pub async fn record(
        &self,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        subscription_arn: &str,
        topic_arn: &str,
        endpoint: &str,
        delivered: bool,
        answered: &Answered,
    ) {
        let status = if delivered {
            Status::Success
        } else {
            Status::Failure
        };
        let Some(role_arn) = feedback_role(&self.feedback, status, &self.message_id) else {
            return;
        };
        let region = super::arn_region(topic_arn)
            .unwrap_or(super::DEFAULT_REGION)
            .to_string();
        let log_group = status.log_group(&self.feedback.log_group);
        if !assumable(ctx, &region, role_arn).await {
            return;
        }
        if !writes_logs(ctx, &region, role_arn, &log_group).await {
            return;
        }
        let attempt = Attempt {
            message_id: &self.message_id,
            topic_arn,
            subscription_arn,
            message_md5: &self.message_md5,
            endpoint,
            published_at: self.published_at,
            status,
            answered,
        };
        let event = rendered(&attempt, SystemTime::now());
        write_log_event(ctx, &region, &log_group, &log_stream_name(), &event).await;
    }
}

/// Whether this service can act as the role `role_arn` names: the role is one of this
/// account's IAM roles and its trust policy admits [`SERVICE_PRINCIPAL`] for
/// `sts:AssumeRole`.
///
/// The role is resolved in the IAM service's own store over the in-process callback, so what
/// decides this is the document a client's `CreateRole` or `UpdateAssumeRolePolicy` left on
/// it, and the reading of that document is the tree's one reading of a trust policy. A role
/// IAM does not hold, and a trust document that cannot be read as a policy at all, are both
/// roles nothing can be assumed under.
async fn assumable(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    let Some(name) = ::service_common::iam_role::role_name(role_arn) else {
        ::tracing::warn!(
            role = %role_arn,
            "a topic's delivery-status feedback role names no IAM role of this account, so no \
             delivery-status record is written"
        );
        return false;
    };
    let client = ::service_common::iam_client::iam_client(ctx, region);
    let answered = client.get_role().role_name(name).send().await;
    let document = match answered {
        Ok(answer) => answer
            .role()
            .and_then(|role| role.assume_role_policy_document())
            .map(ToString::to_string),
        Err(error) => {
            ::tracing::warn!(
                role = %role_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a topic's delivery-status feedback role could not be read, so no \
                 delivery-status record is written"
            );
            return false;
        }
    };
    let admitted = document.as_deref().and_then(|document| {
        ::service_common::iam_trust_policy::admits_service_principal(document, SERVICE_PRINCIPAL)
    });
    if admitted != Some(true) {
        ::tracing::warn!(
            role = %role_arn,
            "a topic's delivery-status feedback role does not trust this service, so it cannot \
             be assumed and no delivery-status record is written"
        );
        return false;
    }
    true
}

/// The CloudWatch Logs actions Amazon SNS performs AS the feedback role to leave one
/// delivery-status record: the group and the stream are created on first write and then the
/// record is put, which is what "give Amazon SNS write access to use CloudWatch Logs on your
/// behalf" (`SetTopicAttributesInput$AttributeName`) asks the role to grant.
const LOG_WRITE_ACTIONS: [&str; 3] = [
    "logs:CreateLogGroup",
    "logs:CreateLogStream",
    "logs:PutLogEvents",
];

/// Whether the permissions in force on the role `role_arn` names allow the CloudWatch Logs
/// write a record costs ([`LOG_WRITE_ACTIONS`]) on the group it goes into.
///
/// The record is written AS that role, so a role whose policies grant the write nothing
/// cannot leave one — and because the group is created by the same role on first write,
/// such a role leaves no group behind either, which is why this is asked BEFORE anything is
/// created. The question is put to IAM itself through the tree's one reading of a principal's
/// permissions (`service_common::iam_role::role_withholds_action`), so what decides it is the
/// documents a client's `PutRolePolicy` or `AttachRolePolicy` left on the role; an emulator
/// with no IAM to ask, and a role it holds nothing under, withhold nothing — a feedback role
/// is routinely provisioned out of band.
async fn writes_logs(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    log_group: &str,
) -> bool {
    // The log group as an IAM resource: CloudWatch Logs names a group's own streams under
    // it, so a policy written for the group covers `log-group:<name>:*`.
    let resource =
        ::emulator_server::emulator_arn("logs", region, &format!("log-group:{log_group}:*"));
    for action in LOG_WRITE_ACTIONS {
        if ::service_common::iam_role::role_withholds_action(ctx, region, role_arn, action, &resource)
            .await
        {
            ::tracing::warn!(
                role = %role_arn,
                action = %action,
                log_group = %log_group,
                "a topic's delivery-status feedback role is not allowed the CloudWatch Logs \
                 write a record costs, so no delivery-status record is written"
            );
            return false;
        }
    }
    true
}

/// The log stream one day's delivery-status records of a topic are written to:
/// `<YYYY>/<MM>/<DD>`, the date-partitioned form CloudWatch Logs streams conventionally
/// carry.
///
/// This emulator's own choice — no recording pins a delivery-status stream name — so nothing
/// is read off it; what a client reads is the group ([`log_group_name`]) and the records in
/// it.
fn log_stream_name() -> String {
    let today = rfc3339(SystemTime::now()).unwrap_or_default();
    today
        .split('T')
        .next()
        .unwrap_or_default()
        .replace('-', "/")
}

/// The most bytes one delivery-status record may carry: "The maximum size of a log event is
/// 256 KB", so a longer one is a record CloudWatch Logs refuses. A record is built from a
/// handful of ids and one provider response, so this only bounds a pathological refusal
/// sentence.
const MAX_LOG_EVENT_BYTES: usize = 256 * 1024;

/// Write one record into `log_group`/`log_stream`, creating both when they are not there
/// yet — which is what the real service does as the assumed role, and why that role needs
/// `logs:CreateLogGroup` and `logs:CreateLogStream` beside `logs:PutLogEvents`.
async fn write_log_event(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    log_group: &str,
    log_stream: &str,
    event: &str,
) {
    if event.len() > MAX_LOG_EVENT_BYTES {
        ::tracing::warn!(
            log_group = %log_group,
            bytes = event.len(),
            limit = MAX_LOG_EVENT_BYTES,
            "a delivery-status record is larger than one log event may be, so it is not written"
        );
        return;
    }
    let client = logs_client(ctx, region);
    // A group or stream that is already there is the one this topic has been writing to, so
    // the `ResourceAlreadyExists` both answer is the ordinary case rather than a failure.
    if let Err(error) = client
        .create_log_group()
        .log_group_name(log_group)
        .send()
        .await
    {
        if !error
            .as_service_error()
            .is_some_and(|service| service.is_resource_already_exists_exception())
        {
            ::tracing::warn!(
                log_group = %log_group,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a topic's delivery-status log group could not be created, so no record is \
                 written"
            );
            return;
        }
    }
    if let Err(error) = client
        .create_log_stream()
        .log_group_name(log_group)
        .log_stream_name(log_stream)
        .send()
        .await
    {
        if !error
            .as_service_error()
            .is_some_and(|service| service.is_resource_already_exists_exception())
        {
            ::tracing::warn!(
                log_group = %log_group,
                log_stream = %log_stream,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a topic's delivery-status log stream could not be created, so no record is \
                 written"
            );
            return;
        }
    }
    let Ok(input) = ::aws_sdk_cloudwatchlogs::types::InputLogEvent::builder()
        .timestamp(epoch_millis(SystemTime::now()))
        .message(event)
        .build()
    else {
        return;
    };
    if let Err(error) = client
        .put_log_events()
        .log_group_name(log_group)
        .log_stream_name(log_stream)
        .log_events(input)
        .send()
        .await
    {
        ::tracing::warn!(
            log_group = %log_group,
            log_stream = %log_stream,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "a delivery-status record could not be written to CloudWatch Logs"
        );
    }
}

/// `at` in milliseconds since the Unix epoch, which is the stamp a log event takes.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// A CloudWatch Logs client over the in-process callback into this same emulator, so a
/// record lands in the store `logs:GetLogEvents` reads.
///
/// The transport is in-process: there is nothing to retry, time out or watch for a stalled
/// stream, and each of those would need a sleep implementation this client is not given. The
/// same goes for caching the identity: the credentials are handed over directly.
fn logs_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_cloudwatchlogs::Client {
    let config = ::aws_sdk_cloudwatchlogs::Config::builder()
        .behavior_version(::aws_sdk_cloudwatchlogs::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_cloudwatchlogs::config::Region::new(
            region.to_string(),
        ))
        .endpoint_url(format!("https://logs.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_cloudwatchlogs::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_cloudwatchlogs::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_cloudwatchlogs::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_cloudwatchlogs::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_cloudwatchlogs::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A topic record carrying `attributes`, as `CreateTopic` would have left it.
    fn topic(attributes: &[(&str, &str)]) -> TopicRecord {
        TopicRecord {
            arn: "arn:aws:sns:us-west-2:000000000000:alerts".to_string(),
            name: "alerts".to_string(),
            owner: "000000000000".to_string(),
            attributes: attributes
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
            tags: ::std::collections::HashMap::new(),
            data_protection_policy: None,
            is_fifo: false,
            created_at: SystemTime::now(),
            subscriptions_deleted: 0,
            subscriptions_deleting: Vec::new(),
            archive_enabled_at: None,
        }
    }

    /// One row of the attribute table: the case, the attributes the topic carries, the protocol
    /// the delivery went over, and whether a feedback configuration is read for it.
    type AttributeCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str, bool);

    /// One row of the sampling table: the case, the success sample rate the topic carries (`None`
    /// for one that names no rate), the outcome of the delivery, and whether a record is written.
    type SamplingCase<'a> = (&'a str, Option<&'a str>, Status, bool);

    /// RULE: the feedback configuration of a delivery is the pair of attributes named for
    /// the DELIVERY'S OWN protocol, so a role set for one protocol logs nothing about
    /// another's deliveries. The rows differ along what the rule generalises over: which
    /// protocol the attributes were set for, which protocol the delivery went over, and
    /// which of the pair is set.
    #[test]
    fn a_protocols_feedback_comes_from_that_protocols_attributes() {
        const ROLE: &str = "arn:aws:iam::000000000000:role/feedback";
        let rows: [AttributeCase<'_>; 7] = [
            (
                "the lambda success role, read for a lambda delivery",
                &[("LambdaSuccessFeedbackRoleArn", ROLE)],
                "lambda",
                true,
            ),
            (
                "the lambda success role, read for an sqs delivery",
                &[("LambdaSuccessFeedbackRoleArn", ROLE)],
                "sqs",
                false,
            ),
            (
                "the sqs success role, read for an sqs delivery",
                &[("SQSSuccessFeedbackRoleArn", ROLE)],
                "sqs",
                true,
            ),
            (
                "the HTTP roles, read for an https delivery",
                &[("HTTPFailureFeedbackRoleArn", ROLE)],
                "https",
                true,
            ),
            (
                "the failure role alone",
                &[("LambdaFailureFeedbackRoleArn", ROLE)],
                "lambda",
                true,
            ),
            ("no feedback attribute at all", &[], "lambda", false),
            (
                "a protocol with no feedback attributes",
                &[("LambdaSuccessFeedbackRoleArn", ROLE)],
                "email",
                false,
            ),
        ];
        for (case, attributes, protocol, expected) in rows {
            let configured = feedback_of(&topic(attributes), protocol).is_some();
            assert_eq!(
                configured, expected,
                "case {case:?}: expected configured={expected}, got {configured}"
            );
        }
    }

    /// RULE: the log group a topic's delivery-status records go into is
    /// `sns/<region>/<account-id>/<topic-name>`, built from the topic itself.
    #[test]
    fn the_log_group_is_named_for_the_topic() {
        let named = log_group_name(&topic(&[]));
        assert_eq!(
            named, "sns/us-west-2/000000000000/alerts",
            "case \"a topic of one Region\": expected the published template, got {named:?}"
        );
    }

    /// RULE: the success sample rate decides how many successful deliveries are logged — 0
    /// none, 100 every one — while a FAILED delivery carries no sample rate and is always
    /// logged. The rows differ along the rate, the outcome and which role is set.
    #[test]
    fn the_sample_rate_decides_which_successes_are_logged() {
        const ROLE: &str = "arn:aws:iam::000000000000:role/feedback";
        let rows: [SamplingCase<'_>; 6] = [
            ("a full sample of a success", Some("100"), Status::Success, true),
            ("no sample of a success", Some("0"), Status::Success, false),
            ("an unset rate", None, Status::Success, false),
            ("a full sample of a failure", Some("100"), Status::Failure, true),
            ("no success sample, but a failure", Some("0"), Status::Failure, true),
            ("an unset rate, and a failure", None, Status::Failure, true),
        ];
        for (case, rate, status, expected) in rows {
            let mut attributes: Vec<(&str, &str)> = vec![
                ("LambdaSuccessFeedbackRoleArn", ROLE),
                ("LambdaFailureFeedbackRoleArn", ROLE),
            ];
            if let Some(rate) = rate {
                attributes.push(("LambdaSuccessFeedbackSampleRate", rate));
            }
            let feedback = feedback_of(&topic(&attributes), "lambda")
                .unwrap_or_else(|| panic!("case {case:?}: the topic names a feedback role"));
            let logged = feedback_role(&feedback, status, "a-message-id").is_some();
            assert_eq!(
                logged, expected,
                "case {case:?}: expected logged={expected}, got {logged}"
            );
        }
    }

    /// RULE: the record names the delivery's endpoint, the status of the attempt and the
    /// answer the endpoint's own service gave — and a record of a FAILURE goes into the
    /// topic's `/Failure` group while a success goes into the group itself. The rows differ
    /// along the outcome, which is what the rule generalises over.
    #[test]
    fn the_record_names_the_endpoint_and_the_status() {
        let published = SystemTime::now();
        for (case, status, reported, code, group) in [
            (
                "a delivery the endpoint took",
                Status::Success,
                "SUCCESS",
                200,
                "sns/us-west-2/000000000000/alerts",
            ),
            (
                "one it did not",
                Status::Failure,
                "FAILURE",
                403,
                "sns/us-west-2/000000000000/alerts/Failure",
            ),
        ] {
            let answered = Answered {
                status_code: Some(code),
                provider_response: Some("{\"ErrorCode\":\"AccessDenied\"}".to_string()),
            };
            let attempt = Attempt {
                message_id: "0123",
                topic_arn: "arn:aws:sns:us-west-2:000000000000:alerts",
                subscription_arn: "arn:aws:sns:us-west-2:000000000000:alerts:abcd",
                message_md5: &message_md5("a notification"),
                endpoint: "arn:aws:lambda:us-west-2:000000000000:function:notify",
                published_at: published,
                status,
                answered: &answered,
            };
            let record: ::serde_json::Value =
                ::serde_json::from_str(&rendered(&attempt, published))
                    .unwrap_or_else(|error| panic!("case {case:?}: not JSON ({error})"));
            assert_eq!(
                record["status"].as_str(),
                Some(reported),
                "case {case:?}: expected status {reported:?}, got {record}"
            );
            assert_eq!(
                record["delivery"]["destination"].as_str(),
                Some(attempt.endpoint),
                "case {case:?}: the record must name the endpoint, got {record}"
            );
            assert_eq!(
                record["notification"]["messageId"].as_str(),
                Some(attempt.message_id),
                "case {case:?}: the record must name the published message, got {record}"
            );
            assert_eq!(
                record["delivery"]["statusCode"].as_u64(),
                Some(u64::from(code)),
                "case {case:?}: expected statusCode {code}, got {record}"
            );
            assert_eq!(
                record["delivery"]["providerResponse"].as_str(),
                answered.provider_response.as_deref(),
                "case {case:?}: the record must carry the provider's answer, got {record}"
            );
            let named = status.log_group(&log_group_name(&topic(&[])));
            assert_eq!(
                named, group,
                "case {case:?}: expected the record in {group:?}, got {named:?}"
            );
        }
    }

    /// RULE: a record whose attempt reached no answer from the endpoint's service carries
    /// neither a status code nor a provider response, rather than a value no service gave.
    #[test]
    fn an_unanswered_attempt_reports_no_status_code() {
        let published = SystemTime::now();
        let answered = Answered::default();
        let attempt = Attempt {
            message_id: "0123",
            topic_arn: "arn:aws:sns:us-west-2:000000000000:alerts",
            subscription_arn: "arn:aws:sns:us-west-2:000000000000:alerts:abcd",
            message_md5: &message_md5("a notification"),
            endpoint: "arn:aws:sqs:us-west-2:000000000000:orders",
            published_at: published,
            status: Status::Failure,
            answered: &answered,
        };
        let record: ::serde_json::Value = ::serde_json::from_str(&rendered(&attempt, published))
            .unwrap_or_else(|error| panic!("case \"no answer\": not JSON ({error})"));
        for member in ["statusCode", "providerResponse"] {
            assert!(
                record["delivery"].get(member).is_none(),
                "case \"no answer\": {member} must be left out, got {record}"
            );
        }
    }

    /// RULE: the `notification` block reports the digest of the published message's BODY, so
    /// one body always digests to the same value and two different bodies to different ones.
    /// The rows differ along the body the digest is taken of: its length, its character set
    /// and its shape. The first two are RFC 1321's own published test values for MD5, which
    /// is what pins the digest to that function rather than to this code.
    #[test]
    fn the_record_reports_the_digest_of_the_published_body() {
        for (case, body, expected) in [
            ("the empty body", "", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a three-byte body", "abc", "900150983cd24fb0d6963f7d28e17f72"),
            (
                "the same three-byte body again",
                "abc",
                "900150983cd24fb0d6963f7d28e17f72",
            ),
        ] {
            let digest = message_md5(body);
            assert_eq!(
                digest, expected,
                "case {case:?}: expected the MD5 of the body, got {digest:?}"
            );
        }

        let published = SystemTime::now();
        let answered = Answered::default();
        let digest = message_md5("a notification");
        let attempt = Attempt {
            message_id: "0123",
            topic_arn: "arn:aws:sns:us-west-2:000000000000:alerts",
            subscription_arn: "arn:aws:sns:us-west-2:000000000000:alerts:abcd",
            message_md5: &digest,
            endpoint: "arn:aws:sqs:us-west-2:000000000000:orders",
            published_at: published,
            status: Status::Success,
            answered: &answered,
        };
        let record: ::serde_json::Value = ::serde_json::from_str(&rendered(&attempt, published))
            .unwrap_or_else(|error| panic!("case \"a rendered record\": not JSON ({error})"));
        assert_eq!(
            record["notification"]["messageMD5Sum"].as_str(),
            Some(digest.as_str()),
            "case \"a rendered record\": the record must carry the body's digest, got {record}"
        );
    }
}
