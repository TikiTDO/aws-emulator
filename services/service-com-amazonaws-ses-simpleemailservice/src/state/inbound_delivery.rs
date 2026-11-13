// SPDX-License-Identifier: CC0-1.0

//! The RECEIVING half of Amazon SES: what the account's ACTIVE receipt rule set does with a
//! message this account has just accepted for one of its own recipients.
//!
//! A receipt rule set is not inert configuration — it is the program Amazon SES runs on every
//! message it receives. `SetActiveReceiptRuleSet` "sets the specified receipt rule set as the
//! active receipt rule set", `DescribeActiveReceiptRuleSet` reports "the metadata and receipt
//! rules for the receipt rule set that is currently active", and `ReceiptRule$Recipients` is
//! "the recipient domains and email addresses that the receipt rule applies to. If this field
//! is not specified, this rule matches all recipients on all verified domains". So a message
//! accepted for a recipient this account holds a verified identity for is also RECEIVED by
//! this account: the active set's rules are evaluated in the order they are held, and each
//! ENABLED rule whose recipient conditions match carries out its actions, until a `StopAction`
//! "terminates the evaluation of the receipt rule set".
//!
//! Of those actions this module carries out the two that act on the message itself:
//!
//! * `S3Action` "saves the received message to an Amazon Simple Storage Service (Amazon S3)
//!   bucket" — one object per matching rule, written at the action's `ObjectKeyPrefix`
//!   followed by the message's own id, carrying the message's RFC 5322 bytes UNALTERED, so
//!   the MIME structure, the part media types, the `Content-Disposition` filenames and the
//!   transfer encodings a client submitted are the ones a client reading the object back
//!   parses. `S3Action$KmsKeyArn` is "the customer managed key that Amazon SES should use to
//!   encrypt your emails before saving them to the Amazon S3 bucket", so a write for an
//!   action naming one asks the object store for that key.
//! * `AddHeaderAction` "adds a header to the received email", which the actions after it
//!   therefore act on.
//!
//! Every other action addresses a resource this module does not act on (a bounce sent back to
//! the sender, a Lambda function, an Amazon SNS notification, a WorkMail organization, an
//! Amazon Connect instance); nothing here reports one of them as carried out.
//!
//! The write is made the way the framework documents a handler reaching another emulated
//! service ([`smithy_server_runtime::OperationContext::callback`]): the AWS SDK driven over
//! the emulator's own in-process transport ([`::service_common::s3_client`]), so nothing here
//! opens a socket or leaves the host. The destination is a bucket NAME and an object key, both
//! resolved by this same emulator's own object store, and the only part of the client's
//! addressing a request contributes is the signing Region, which is a Region LABEL by the time
//! it reaches here ([`super::region_from_request`]) — so no text of a request can steer where
//! a received message goes. A delivery that cannot be made leaves a line for an operator and
//! nothing else: the message was already ACCEPTED, and Amazon SES does not un-accept a message
//! because an action of a receipt rule failed.

/// How many actions of the active rule set ONE received message is carried out through.
///
/// An EMULATOR ceiling, not an AWS quota: a rule's `Actions` is a list a caller writes and the
/// active set holds up to [`super::MAX_RECEIPT_RULES_PER_RULE_SET`] rules, so the work one
/// accepted message can drive is bounded here. A received message is processed by the handful
/// of actions its recipient's rules name, so no rule set a client really writes reaches it.
pub const MAX_RECEIPT_ACTIONS_PER_MESSAGE: usize = 20;

/// One thing the active receipt rule set does with a received message, in evaluation order.
///
/// Read off the rule set while this service's store is held, and carried out afterwards
/// ([`deliver`]): the decision is about state, the delivery is a call to another service, and
/// holding a store across that call would make one message's delivery block every other
/// request this service serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptStep {
    /// `AddHeaderAction`: the header to add to the received message, which every step after
    /// this one acts on.
    AddHeader {
        /// `AddHeaderAction$HeaderName`.
        name: String,
        /// `AddHeaderAction$HeaderValue`.
        value: String,
    },
    /// `S3Action`: save the received message into the object store.
    Save {
        /// `S3Action$BucketName`.
        bucket: String,
        /// `S3Action$ObjectKeyPrefix`, empty when the action names none.
        object_key_prefix: String,
        /// `S3Action$KmsKeyArn`, when the action names a key to encrypt the message under.
        kms_key_arn: Option<String>,
    },
}

/// What the account's active receipt rule set does with a message accepted for `recipients` —
/// the steps [`deliver`] then carries out, in evaluation order, and empty when this account
/// receives no mail for any of them, when no rule set is active, or when no rule of the active
/// set matches.
///
/// `recipients` are the message's ENVELOPE recipients, which is what a receipt rule's
/// conditions are compared against.
#[must_use]
pub fn receipt_steps(
    state: &super::ComAmazonawsSesSimpleemailserviceState,
    recipients: &[String],
) -> Vec<ReceiptStep> {
    let Some(rule_set) = state.active_rule_set() else {
        // "If you leave this parameter empty, the account's active rule set is disabled" —
        // an account with no active set receives nothing.
        return Vec::new();
    };
    // Only the recipients this account receives mail FOR are evaluated: a rule's conditions
    // match "all recipients on all verified domains", so a recipient on a domain this account
    // holds no verified identity for is not a recipient of this account at all.
    let received: Vec<&String> = recipients
        .iter()
        .filter(|recipient| receives_mail_for(state, recipient))
        .collect();
    if received.is_empty() {
        return Vec::new();
    }
    let mut steps: Vec<ReceiptStep> = Vec::new();
    for rule in &rule_set.rules {
        // "If false, the receipt rule is inactive."
        if !rule.enabled {
            continue;
        }
        if !received
            .iter()
            .any(|recipient| matches_recipients(rule, recipient))
        {
            continue;
        }
        for action in rule.actions.as_deref().unwrap_or_default() {
            if steps.len() >= MAX_RECEIPT_ACTIONS_PER_MESSAGE {
                ::tracing::warn!(
                    rule = ?rule.name,
                    ceiling = MAX_RECEIPT_ACTIONS_PER_MESSAGE,
                    "the active receipt rule set names more actions for one received message \
                     than this emulator carries out"
                );
                return steps;
            }
            if let Some(add_header) = &action.add_header_action {
                steps.push(ReceiptStep::AddHeader {
                    name: add_header.header_name.clone(),
                    value: add_header.header_value.clone(),
                });
            }
            if let Some(s3_action) = &action.s3_action {
                steps.push(ReceiptStep::Save {
                    bucket: s3_action.bucket_name.clone(),
                    object_key_prefix: s3_action
                        .object_key_prefix
                        .clone()
                        .unwrap_or_default(),
                    kms_key_arn: s3_action.kms_key_arn.clone(),
                });
            }
            // "This action terminates the evaluation of the receipt rule set": the actions
            // before it in this rule have been taken, and no later rule is evaluated.
            if action.stop_action.is_some() {
                return steps;
            }
        }
    }
    steps
}

/// Whether this account RECEIVES mail for `recipient`: the address itself is one of its
/// verified identities, or it sits in a domain one of them covers.
///
/// The same reading of a verified domain every other gate of this service applies — a
/// verified domain covers the mailboxes in it and in its subdomains
/// ([`super::ComAmazonawsSesSimpleemailserviceState::covers_verified_domain`]) — so a rule
/// matching "all recipients on all verified domains" matches exactly the recipients this
/// account holds a verified identity for.
fn receives_mail_for(
    state: &super::ComAmazonawsSesSimpleemailserviceState,
    recipient: &str,
) -> bool {
    let address = super::addr_spec(recipient);
    state.is_verified(address) || state.covers_verified_domain(super::identity_domain(address))
}

/// Whether `rule`'s recipient conditions apply to `recipient`.
///
/// `ReceiptRule$Recipients` holds "the recipient domains and email addresses that the receipt
/// rule applies to. If this field is not specified, this rule matches all recipients on all
/// verified domains" — so a rule stating no condition applies to every recipient of the
/// account, and a condition applies when it names the recipient's own address, the recipient's
/// domain, or a domain the recipient's domain sits under. Compared without regard to case,
/// which is how a domain name is compared and how a mailbox is addressed in practice.
fn matches_recipients(rule: &crate::ops::types::ReceiptRule, recipient: &str) -> bool {
    let conditions = rule.recipients.as_deref().unwrap_or_default();
    if conditions.is_empty() {
        return true;
    }
    let address = super::addr_spec(recipient);
    let domain = super::identity_domain(address);
    conditions.iter().any(|condition| {
        let condition = condition.trim();
        condition.eq_ignore_ascii_case(address)
            || condition.eq_ignore_ascii_case(domain)
            || super::is_subdomain_of(domain, condition)
    })
}

/// How many received-message delivery tasks this process may have in flight at once.
///
/// An EMULATOR ceiling, not an AWS quota: a caller looping on sends must not be able to start
/// delivery tasks without end, and each task holds a copy of the message it is saving, which
/// `super::MAX_MESSAGE_BYTES` bounds. A message that finds every permit taken is delivered
/// nowhere and says so in the log, which is the same outcome as a destination that cannot be
/// reached — the delivery is best effort either way, and what the sender is told about its send
/// does not depend on it.
pub const MAX_IN_FLIGHT_DELIVERY_TASKS: usize = 16;

/// How many delivery tasks are in flight right now; see [`MAX_IN_FLIGHT_DELIVERY_TASKS`].
static IN_FLIGHT_DELIVERY_TASKS: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// One task's claim on an in-flight permit, released when the task ends — including when it
/// ends by panicking or by being dropped at runtime shutdown, because the release is a `Drop`.
struct DeliveryPermit;

impl DeliveryPermit {
    /// Claim a permit, or `None` when [`MAX_IN_FLIGHT_DELIVERY_TASKS`] are already in flight.
    fn claim() -> Option<Self> {
        IN_FLIGHT_DELIVERY_TASKS
            .try_update(
                ::std::sync::atomic::Ordering::SeqCst,
                ::std::sync::atomic::Ordering::SeqCst,
                |in_flight| (in_flight < MAX_IN_FLIGHT_DELIVERY_TASKS).then_some(in_flight + 1),
            )
            .ok()
            .map(|_| Self)
    }
}

impl Drop for DeliveryPermit {
    fn drop(&mut self) {
        IN_FLIGHT_DELIVERY_TASKS.fetch_sub(1, ::std::sync::atomic::Ordering::SeqCst);
    }
}

/// Carry out `steps` for the message `message_id` identifies, whose RFC 5322 bytes are `raw`,
/// on the runtime's own time.
///
/// Called with this service's store HELD, because nothing it does touches that store: a
/// message is RECEIVED after it was accepted — the acceptance is answered first, and the
/// actions of the matching rules are carried out behind it — so the steps run on a task of
/// their own rather than on the accepting request's path. With no async runtime to spawn onto
/// there is nothing to run them on, and nothing a caller is told depends on them.
///
/// Nothing about the call that accepted the message changes: the message was accepted before
/// this runs, so a step that could not be carried out leaves a line for an operator and the
/// accepted send stands (see the module docs).
///
/// What bounds the task: one accepted message spawns at most one task, at most
/// [`MAX_IN_FLIGHT_DELIVERY_TASKS`] of them run at once, each carries out at most
/// [`MAX_RECEIPT_ACTIONS_PER_MESSAGE`] actions with no retry loop behind any of them, and the
/// whole delivery is abandoned after [`DELIVERY_DEADLINE`].
///
/// `region` is the Region the accepting request was signed for, which is where this account's
/// receipt rule set and the destinations its actions name live; no text of a request chooses
/// the endpoint a write is made to.
pub fn deliver(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    message_id: &str,
    raw: &[u8],
    steps: &[ReceiptStep],
) {
    if steps.is_empty() {
        return;
    }
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        ::tracing::warn!(
            message_id,
            "no async runtime is available to carry out the receipt rules of this received \
             message on"
        );
        return;
    };
    let Some(permit) = DeliveryPermit::claim() else {
        ::tracing::warn!(
            message_id,
            in_flight = MAX_IN_FLIGHT_DELIVERY_TASKS,
            "this emulator is already delivering as many received messages as it delivers at \
             once; this message is delivered nowhere"
        );
        return;
    };
    // The callback is an owned handle into this same emulator, so the task that carries the
    // actions out outlives the request that accepted the message.
    let callback = ctx.callback().clone();
    let region = region.to_string();
    let identifier = message_id.to_string();
    let message = raw.to_vec();
    let steps = steps.to_vec();
    runtime.spawn(async move {
        // Moved into the task, so the permit is held for exactly as long as the task runs and
        // is released however it ends.
        let _permit = permit;
        if ::tokio::time::timeout(
            DELIVERY_DEADLINE,
            carry_out(&callback, &region, &identifier, message, &steps),
        )
        .await
        .is_err()
        {
            ::tracing::warn!(
                message_id = identifier,
                deadline_secs = DELIVERY_DEADLINE.as_secs(),
                "the receipt rules of this received message were not carried out inside the \
                 deadline and were abandoned"
            );
        }
    });
}

/// How long the actions of ONE received message are carried out for before the delivery is
/// abandoned.
///
/// An EMULATOR deadline, not an AWS one: every call behind a step is in-process, with retries
/// and the SDK's own timeouts disabled, so this is what keeps a task waiting on a slow handler
/// from holding its permit for the life of the process.
pub const DELIVERY_DEADLINE: ::std::time::Duration = ::std::time::Duration::from_secs(30);

/// Carry out `steps` for one received message, in order, against the services their actions
/// address.
async fn carry_out(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    message_id: &str,
    raw: Vec<u8>,
    steps: &[ReceiptStep],
) {
    let client = ::service_common::s3_client::s3_client(callback, region);
    // The message as the steps taken so far leave it: the bytes a client submitted, plus the
    // headers an `AddHeaderAction` has added to it.
    let mut message = raw;
    for step in steps {
        match step {
            ReceiptStep::AddHeader { name, value } => with_header(&mut message, name, value),
            ReceiptStep::Save {
                bucket,
                object_key_prefix,
                kms_key_arn,
            } => {
                let key = format!("{object_key_prefix}{message_id}");
                let mut write = client
                    .put_object()
                    .bucket(bucket)
                    .key(&key)
                    .body(::aws_sdk_s3::primitives::ByteStream::from(message.clone()));
                if let Some(kms_key_arn) = kms_key_arn
                    .as_deref()
                    .map(str::trim)
                    .filter(|kms_key_arn| !kms_key_arn.is_empty())
                {
                    // "The customer managed key that Amazon SES should use to encrypt your
                    // emails before saving them to the Amazon S3 bucket": the object store
                    // owns the key's use, so the write names the key and lets it encrypt
                    // under it rather than keeping a second account of the encryption here.
                    write = write
                        .server_side_encryption(::aws_sdk_s3::types::ServerSideEncryption::AwsKms)
                        .ssekms_key_id(kms_key_arn);
                }
                if let Err(failure) = write.send().await {
                    ::tracing::warn!(
                        bucket = ?bucket,
                        key = ?key,
                        error = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                        "a received message could not be saved into the receipt rule's bucket"
                    );
                }
            }
        }
    }
}

/// Add the header `name: value` to the received message `message`.
///
/// Added at the TOP of the header block, which is where a message-transfer agent adds a header
/// of its own: the fields already in the block keep their order and their bytes, and the body
/// is untouched. The line ends `CRLF`, the line ending RFC 5322 states for a message's fields;
/// a name and a value outside the charsets `AddHeaderAction` documents for them were refused
/// when the rule was stored ([`super::is_header_name`], [`super::is_header_value`]), so no
/// value reaching here can carry a line ending of its own.
fn with_header(message: &mut Vec<u8>, name: &str, value: &str) {
    let field = format!("{name}: {value}\r\n");
    message.splice(..0, field.into_bytes());
}
