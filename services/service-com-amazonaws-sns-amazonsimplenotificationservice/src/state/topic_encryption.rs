// SPDX-License-Identifier: CC0-1.0

//! The customer managed key a TOPIC is encrypted with, and whether a message may be sealed
//! under it right now — the step every operation that writes a message to a topic takes
//! before it accepts one.
//!
//! The rule: a topic carrying [`KMS_MASTER_KEY_ID_ATTRIBUTE`] has every message published to
//! it encrypted under that key, and Amazon SNS performs that encryption by asking KMS for a
//! data key as the message is published. So the KEY's own condition is in force over the
//! publish: a key KMS cannot use for a cryptographic operation at that moment seals nothing,
//! and the publish is refused rather than answered with a message id — a client is never told
//! a message is enveloped under a key that could not have encrypted it. The key's condition
//! is read when the message is published rather than when the topic was configured, so
//! disabling the key stops the topic from taking traffic and enabling it again lets the topic
//! take traffic once more, with nothing written to the topic in between.
//!
//! The key is resolved in the KMS service's OWN store, through KMS's own resolution of a
//! `KeyId` — so a key id, a key ARN, an alias and an alias ARN all name the key their own
//! service says they name, and the two services cannot disagree about a key's state.
//!
//! Amazon SNS does not word these refusals itself: the call that failed is its own call to
//! KMS, so the message it answers with is KMS's own sentence followed by the metadata block an
//! AWS error carries — the service that refused, the status, the error code and the id of the
//! refused call ([`relayed`]). Which modelled error each refusal is answered in is the
//! publishing operation's own mapping, since each models the `KMS*` family itself.
//!
//! Only what the KEY ITSELF says about a cryptographic use is decided here, and only where a
//! key record says it: the state the key is in. An identifier that resolves to no key record
//! attributes no condition to a key and so refuses nothing, and whether the key's POLICY admits
//! the caller is a decision of its own, which this service does not make on the data plane yet.
//!
//! When no KMS service is mounted beside this one, nothing answers for the key and the publish
//! proceeds: a condition nobody attributed to the key never refuses a publish.

use std::collections::HashMap;

use ::service_com_amazonaws_kms_trentservice::state as kms_store;

use super::KMS_MASTER_KEY_ID_ATTRIBUTE;

/// The key the topic's attribute set encrypts its messages under, or `None` for a topic on no
/// customer managed key.
///
/// An empty value is no key: an empty `KmsMasterKeyId` is how a caller takes a topic off its
/// key, so nothing downstream reads the empty string as a key identifier.
pub fn configured_master_key(attributes: &HashMap<String, String>) -> Option<&str> {
    attributes
        .get(KMS_MASTER_KEY_ID_ATTRIBUTE)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// Why the topic's key cannot seal this message, in the KMS error class the publishing
/// operation answers it in. The payload is the message, as KMS answered it.
#[derive(Debug, Clone)]
pub enum KeyRefusal {
    /// The key is disabled — KMS's `DisabledException`.
    Disabled(String),
    /// The key is in a state no cryptographic operation can use — KMS's
    /// `KMSInvalidStateException`.
    InvalidState(String),
}

/// The service name KMS's own error metadata carries.
const KMS_SERVICE_NAME: &str = "AWSKMS";

/// The status KMS answers each of these refusals with.
const KMS_REFUSAL_STATUS: u16 = 400;

/// KMS's own error code for a key that is disabled.
const KMS_DISABLED_CODE: &str = "DisabledException";

/// KMS's own error code for a key whose state admits no cryptographic operation.
const KMS_INVALID_STATE_CODE: &str = "KMSInvalidStateException";

/// A refusal KMS answered, as the publishing caller sees it: this service does not word these
/// refusals itself, it relays the call that failed — so the message is KMS's own sentence
/// followed by the metadata block an AWS error carries.
///
/// `code` is KMS's own error code for the class, which is not the `KMS*` code this service
/// answers with: the first names the shape KMS returned, the second the shape SNS relays it
/// as. The request id identifies the ONE refused call, so it is minted per refusal.
fn relayed(message: &str, code: &str) -> String {
    format!(
        "{message} (Service: {KMS_SERVICE_NAME}; Status Code: {KMS_REFUSAL_STATUS}; \
         Error Code: {code}; Request ID: {}; Proxy: null)",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// Whether the key `master_key_id` names — the one the topic encrypts its messages with — can
/// seal a message being published right now, judged at `region`'s endpoint.
///
/// `master_key_id` is the topic's `KmsMasterKeyId` in any form KMS accepts: a key id, a key
/// ARN, an alias or an alias ARN.
///
/// `Ok(())` for a key KMS can use, and wherever nothing of the kind can be shown, so a publish
/// is never refused on a guess: an empty identifier (no key), an engine carrying no KMS store,
/// and an identifier that resolves to no key record this endpoint holds — which says nothing
/// about a key, and whose resolution is a concern of its own. See the module docs for the rule.
///
/// Call it with no store of this service locked: the KMS guard is taken here and released
/// before it answers.
pub fn authorize_topic_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    master_key_id: &str,
) -> Result<(), KeyRefusal> {
    let master_key_id = master_key_id.trim();
    if master_key_id.is_empty() {
        return Ok(());
    }
    let Some(kms) = kms_store::service_state(engine) else {
        // Visible to an operator: a lookup nothing answered lets the publish through, so
        // without this line it would be indistinguishable from a key KMS said it can use.
        ::tracing::warn!(
            master_key_id,
            "no KMS service is mounted beside Amazon SNS; an encrypted topic's key is accepted"
        );
        return Ok(());
    };
    let found = {
        let mut store = kms.lock();
        let resolved = match store.resolve(region, master_key_id) {
            kms_store::KeyRef::Found(key) => Some(KeyCondition::of(key)),
            // An identifier that resolves to no key record attributes no condition to a key,
            // so it refuses nothing here: an identifier KMS cannot parse, one naming a key
            // this endpoint does not hold, and one whose account segment names no account are
            // all a question about the IDENTIFIER rather than about the key's state.
            kms_store::KeyRef::NotFound
            | kms_store::KeyRef::MalformedArn
            | kms_store::KeyRef::UnauthorizedAccount => None,
        };
        match resolved {
            Some(key) => Some(key),
            // "Amazon Web Services services use DescribeKey to create Amazon Web Services
            // managed keys from a predefined Amazon Web Services alias with no key ID": a
            // topic on such an alias is on the Amazon Web Services managed key it names, and
            // the first use of it is what associates the two — so the key whose condition
            // governs the publish exists from this publish on.
            None => kms_store::alias_named_by(region, master_key_id).and_then(|alias| {
                store
                    .associate_predefined_aws_alias(region, &alias)
                    .map(KeyCondition::of)
            }),
        }
    };
    let Some(key) = found else {
        return Ok(());
    };

    // A key that is merely DISABLED is its own refusal class, told apart from every other
    // state no cryptographic operation can use: a disabled key is one a client enables again,
    // and the two are modelled separately for exactly that reason.
    if key.state == kms_store::KEY_STATE_DISABLED {
        return Err(KeyRefusal::Disabled(relayed(
            &key.state_message,
            KMS_DISABLED_CODE,
        )));
    }
    if !key.usable {
        return Err(KeyRefusal::InvalidState(relayed(
            &key.state_message,
            KMS_INVALID_STATE_CODE,
        )));
    }
    Ok(())
}

/// What the key itself says about sealing a message under it, taken out of the KMS store so
/// the refusal is worded with its lock released.
#[derive(Debug, Clone)]
struct KeyCondition {
    /// The `KeyState` a reader of the key reports, which is the state the refusal is decided
    /// on — so the class the caller is refused in and the sentence it carries are the same
    /// state.
    state: String,
    /// How KMS words a refusal caused by that key state.
    state_message: String,
    /// Whether the key can be used in a cryptographic operation at all right now.
    usable: bool,
}

impl KeyCondition {
    fn of(key: &kms_store::Cmk) -> Self {
        Self {
            state: key.reported_key_state().to_string(),
            state_message: kms_store::invalid_state_message(key),
            usable: key.is_usable(),
        }
    }
}
