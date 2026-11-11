// SPDX-License-Identifier: CC0-1.0

//! The message-template VERSION family of this service's state: the record one published
//! version of a message template is persisted as, the projections the operations that
//! report one answer out of, and the lookups that reach a version inside its template.
//!
//! A version is an IMMUTABLE snapshot of the template's working copy, taken when
//! `CreateMessageTemplateVersion` published it — "Creates a new Amazon Q in Connect
//! message template version from the current content and configuration of a message
//! template. Versions are immutable and monotonically increasing"
//! (`smithy_query(query="doc",shape="CreateMessageTemplateVersion")`) — so a later write
//! to the working copy leaves every published version as it was. The snapshot is a whole
//! [`MessageTemplate`] record, so every member a version reports is rendered by the SAME
//! projection a read of the working copy renders, and the two cannot disagree about what
//! was published.
//!
//! At most one version of a template is activated at a time: activating one deactivates
//! the one that was active, which is what the `$ACTIVE_VERSION` qualifier
//! ([`VERSION_QUALIFIER_ACTIVE`]) resolves to.

use std::collections::HashMap;

use super::*;
use crate::ops::types::{ExtendedMessageTemplateData, MessageTemplateVersionSummary};

/// The message a request addressed to a version of a message template this store holds no
/// such version of is refused with, worded as this endpoint words the other not-found
/// resources of this family ([`MESSAGE_MESSAGE_TEMPLATE_NOT_FOUND`]), with the resource
/// token spelled as the API itself names the resource.
pub const MESSAGE_MESSAGE_TEMPLATE_VERSION_NOT_FOUND: &str =
    "Message template version does not exist.";

/// The most versions one message template may hold at a time; a publish beyond it is
/// refused with the modelled `ServiceQuotaExceededException` (HTTP 402), which
/// `CreateMessageTemplateVersion` declares.
///
/// CHOSEN: AWS quotas message-template versions but does not publish the figure. The bound
/// is also what keeps this store finite — a version is released only by the delete of the
/// template that holds it, or of the knowledge base that holds the template.
pub const MAX_MESSAGE_TEMPLATE_VERSIONS_PER_TEMPLATE: usize = 100;

/// The sentence a publish past [`MAX_MESSAGE_TEMPLATE_VERSIONS_PER_TEMPLATE`] is refused
/// with, stating the RULE — the ceiling and what to do about it — and carrying nothing
/// from the request or from what the template already holds.
pub fn message_message_template_version_quota() -> String {
    format!(
        "The maximum number of message template versions \
         ({MAX_MESSAGE_TEMPLATE_VERSIONS_PER_TEMPLATE}) for this message template has been \
         reached. Delete a message template version before creating another."
    )
}

/// The sentence a publish whose `messageTemplateContentSha256` is not the checksum of the
/// working copy is refused with.
///
/// The operation's own documentation states the rule — "An error is displayed if the
/// supplied messageTemplateContentSha256 is different from the
/// messageTemplateContentSha256 of the message template with $LATEST qualifier"
/// (`smithy_query(query="doc",shape="CreateMessageTemplateVersion")`) — so the sentence
/// states that, and its one variable part is the checksum the working copy actually
/// carries, read off the stored record.
pub fn message_content_checksum_mismatch(content_sha256: &str) -> String {
    format!(
        "The supplied messageTemplateContentSha256 does not match the \
         messageTemplateContentSha256 {content_sha256} of the message template referenced by \
         the {VERSION_QUALIFIER_LATEST} qualifier."
    )
}

/// The sentence a request whose resource label carries a version qualifier, on an
/// operation whose own documentation says it may not, is refused with.
///
/// Those operations address the working copy (or name the version in a member of their
/// own), and each documents the label as one that "cannot contain any qualifier"
/// (`smithy_query(query="operation",operation="ActivateMessageTemplate")` and the other
/// message-template writes), so a label that carries one is refused rather than read as if
/// it named something. Its one variable part is the member the request carried it in.
pub fn message_qualifier_not_accepted(member: &str) -> String {
    format!("The {member} cannot contain a qualifier.")
}

/// One published version of a message template: the snapshot that was taken of the working
/// copy, the number it was published under, and whether it is the activated one.
#[derive(Debug, Clone)]
pub struct MessageTemplateVersion {
    /// The `versionNumber` this version was published under: 1 for a template's first
    /// version and the previous one plus 1 for each later publish, which is what
    /// "monotonically increasing" states and what the modelled `Version` range (`min 1`)
    /// admits.
    pub version_number: i64,
    /// Whether this version is the ACTIVATED one. At most one version of a template
    /// carries it: `ActivateMessageTemplate` sets it here and clears it on the version
    /// that held it.
    pub is_active: bool,
    /// The working copy as it was when this version was published. Immutable: a later
    /// write to the template leaves this record alone.
    pub snapshot: MessageTemplate,
}

impl MessageTemplateVersion {
    /// The version qualifier that names this version: its number, which is what a label
    /// addresses it by — "you can reference a specific version of the message template by
    /// passing in <message-template-id>:<versionNumber>".
    pub fn qualifier(&self) -> String {
        self.version_number.to_string()
    }

    /// The ARN of this version: the template's own ARN with this version's qualifier
    /// appended ([`version_qualified_arn`]), which is the `messageTemplateArn` every
    /// operation that answers about a version reports.
    pub fn message_template_arn(&self) -> String {
        version_qualified_arn(&self.snapshot.message_template_arn, &self.qualifier())
    }

    /// This version as the `ExtendedMessageTemplateData` shape a read of it answers with:
    /// the snapshot's own projection, carrying this version's number and activation state,
    /// so a version reports exactly what was published under it.
    pub fn to_extended_message_template_data(&self) -> ExtendedMessageTemplateData {
        self.snapshot.to_extended_message_template_data(
            &self.qualifier(),
            Some(self.version_number),
            Some(self.is_active),
        )
    }

    /// This version as the `MessageTemplateVersionSummary` shape
    /// `ListMessageTemplateVersions` answers with, rendered out of the snapshot.
    pub fn to_summary(&self) -> MessageTemplateVersionSummary {
        MessageTemplateVersionSummary {
            message_template_arn: self.message_template_arn(),
            message_template_id: self.snapshot.message_template_id.clone(),
            knowledge_base_arn: self.snapshot.knowledge_base_arn.clone(),
            knowledge_base_id: self.snapshot.knowledge_base_id.clone(),
            name: self.snapshot.name.clone(),
            // `channel` is the Amazon Connect channel of the template, which no member of
            // any message-template request carries and this store therefore holds no value
            // for: the member is optional, so nothing is claimed under it.
            channel: None,
            channel_subtype: self.snapshot.channel_subtype.clone(),
            is_active: self.is_active,
            version_number: self.version_number,
        }
    }
}

/// Every published version of one message template, keyed by `messageTemplateId` and held
/// in ascending `versionNumber` order — the order a publish appends in, so what a listing
/// reports does not depend on map iteration order.
pub type MessageTemplateVersions = HashMap<String, Vec<MessageTemplateVersion>>;

impl ComAmazonawsQconnectWisdomserviceState {
    /// Every published version of one message template, in ascending `versionNumber`
    /// order. Empty while the template has published none, which is the correct "no
    /// versions" answer for a template that exists.
    pub fn message_template_versions_of(&self, message_template_id: &str) -> &[MessageTemplateVersion] {
        self.message_template_versions
            .get(message_template_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The version of this template published under `version_number`, or `None` when it
    /// published none under that number — the lookup a request that names a version
    /// resolves through.
    pub fn message_template_version(
        &self,
        message_template_id: &str,
        version_number: i64,
    ) -> Option<&MessageTemplateVersion> {
        self.message_template_versions_of(message_template_id)
            .iter()
            .find(|version| version.version_number == version_number)
    }

    /// The ACTIVATED version of this template, which is what the `$ACTIVE_VERSION`
    /// qualifier names, or `None` while no version of it is activated.
    pub fn active_message_template_version(
        &self,
        message_template_id: &str,
    ) -> Option<&MessageTemplateVersion> {
        self.message_template_versions_of(message_template_id)
            .iter()
            .find(|version| version.is_active)
    }

    /// The most recently published version of this template, or `None` while it has
    /// published none.
    pub fn latest_message_template_version(
        &self,
        message_template_id: &str,
    ) -> Option<&MessageTemplateVersion> {
        self.message_template_versions_of(message_template_id).last()
    }

    /// Publish `snapshot` as the template's next version, answering with the
    /// `versionNumber` it was published under: the previous one plus 1, and 1 for a
    /// template's first version.
    ///
    /// The published version is NOT activated: activation is `ActivateMessageTemplate`'s
    /// own call.
    pub fn publish_message_template_version(&mut self, snapshot: MessageTemplate) -> i64 {
        let versions = self
            .message_template_versions
            .entry(snapshot.message_template_id.clone())
            .or_default();
        let version_number = versions
            .last()
            .map_or(1, |previous| previous.version_number + 1);
        versions.push(MessageTemplateVersion {
            version_number,
            is_active: false,
            snapshot,
        });
        version_number
    }

    /// Activate one published version of a template, deactivating the version that was
    /// active — "After the version is activated, the previous active version will be
    /// deactivated automatically" — and answering whether the template published a version
    /// under that number at all. A template that published none is left untouched.
    pub fn activate_message_template_version(
        &mut self,
        message_template_id: &str,
        version_number: i64,
    ) -> bool {
        let Some(versions) = self.message_template_versions.get_mut(message_template_id) else {
            return false;
        };
        if !versions
            .iter()
            .any(|version| version.version_number == version_number)
        {
            return false;
        }
        for version in versions.iter_mut() {
            version.is_active = version.version_number == version_number;
        }
        true
    }

    /// Deactivate one published version of a template, answering whether the template
    /// published a version under that number — what `DeactivateMessageTemplate` calls, so
    /// that after it no version of the template is the activated one. A version that was
    /// not activated stays inactive, and the other versions are left as they were.
    pub fn deactivate_message_template_version(
        &mut self,
        message_template_id: &str,
        version_number: i64,
    ) -> bool {
        let Some(versions) = self.message_template_versions.get_mut(message_template_id) else {
            return false;
        };
        let Some(version) = versions
            .iter_mut()
            .find(|version| version.version_number == version_number)
        else {
            return false;
        };
        version.is_active = false;
        true
    }

    /// Forget every published version of one message template — what the delete of the
    /// template, and of the knowledge base that holds it, calls so a template that is gone
    /// takes its versions with it.
    pub fn forget_message_template_versions_of(&mut self, message_template_id: &str) {
        self.message_template_versions.remove(message_template_id);
    }
}
