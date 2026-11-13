// SPDX-License-Identifier: CC0-1.0

//! Which Verified Access trust providers each Verified Access instance has attached: the one
//! writer of that attachment, the one reading of it, and the idempotency tokens the two calls
//! that change it were given.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every other
//! primitive there.

use super::*;

/// One `AttachVerifiedAccessTrustProvider` or `DetachVerifiedAccessTrustProvider` request, as
/// the token it was given with recorded it: the pair it named and which way it moved it.
///
/// "A unique, case-sensitive token that you provide to ensure idempotency of your modification
/// request" (`AttachVerifiedAccessTrustProviderRequest$ClientToken`) — so a repeat of one
/// request changes nothing further, while the SAME token carrying a DIFFERENT request is the
/// mismatch EC2 refuses ([`idempotent_parameter_mismatch`]) rather than a second change.
/// Telling those two apart needs the REQUEST, which is why a recorded change carries the pair
/// and the direction and not just the instance it named.
#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct VerifiedAccessTrustProviderAttachmentRequest {
    /// The Verified Access instance the request named.
    pub verified_access_instance_id: String,
    /// The Verified Access trust provider the request named.
    pub verified_access_trust_provider_id: String,
    /// `true` for the attach of that pair, `false` for the detach of it: the two calls are
    /// each other's mirror, so one record describes either.
    pub attach: bool,
}

/// `provider` as an instance reports it among the trust providers attached to it:
/// "Condensed information about a trust provider"
/// (`smithy_query(query="shape",shape="VerifiedAccessTrustProviderCondensed")`) — its id, its
/// description and its kinds, and nothing else that shape carries no member for.
///
/// Derived from the STORED trust provider rather than from a request, so what
/// `DescribeVerifiedAccessInstances` reports on an instance and what
/// `DescribeVerifiedAccessTrustProviders` reports for the same trust provider cannot
/// disagree.
pub fn verified_access_trust_provider_condensed(
    provider: &crate::ops::types::VerifiedAccessTrustProvider,
) -> crate::ops::types::VerifiedAccessTrustProviderCondensed {
    crate::ops::types::VerifiedAccessTrustProviderCondensed {
        verified_access_trust_provider_id: provider.verified_access_trust_provider_id.clone(),
        description: provider.description.clone(),
        trust_provider_type: provider.trust_provider_type.clone(),
        user_trust_provider_type: provider.user_trust_provider_type.clone(),
        device_trust_provider_type: provider.device_trust_provider_type.clone(),
    }
}

/// The caller-chosen bytes one change of an attachment retains for the process's lifetime: the
/// trust provider's description, which the instance's own record carries a copy of while the
/// pair is attached, plus the client token the token map keeps. A DETACH retains neither copy,
/// so it charges the token alone.
///
/// ONE definition of that charge, computed identically by each handler's pre-check of the
/// store's byte budget and by
/// [`ComAmazonawsEc2Amazonec2State::set_verified_access_trust_provider_attachment`], so a
/// writer cannot store bytes the budget never saw. The ids, the kinds and the timestamps are
/// the service's own or come from a modelled enum, so they are bounded already and not counted
/// here.
pub fn verified_access_trust_provider_attachment_payload_bytes(
    request: &VerifiedAccessTrustProviderAttachmentRequest,
    provider: &crate::ops::types::VerifiedAccessTrustProvider,
    client_token: Option<&str>,
) -> usize {
    let condensed = if request.attach {
        provider.description.as_ref().map_or(0, String::len)
    } else {
        0
    };
    condensed.saturating_add(client_token.map_or(0, str::len))
}

/// Refuse a detach of a pair that is not attached: HTTP 400 `InvalidParameterValue`, the code
/// EC2's error list gives for a value that "is not valid, is unsupported, or cannot be used".
///
/// The mirror of the duplicate attach, which is refused with
/// [`resource_already_associated`]: a detach of a trust provider the named instance does not
/// have cannot answer as though it had removed an attachment, and the message names the pair
/// the request sent in the sentence form this resource family words its refusals with.
pub fn verified_access_trust_provider_not_attached<E>(
    trust_provider_id: &str,
    instance_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::invalid_parameter_value(format!(
        "VerifiedAccessTrustProvider {trust_provider_id} is not attached to \
         VerifiedAccessInstance {instance_id}"
    ))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether the Verified Access instance `instance_id` has the trust provider
    /// `trust_provider_id` attached.
    ///
    /// The one reading of an attachment, so the attach that refuses a pair it already holds,
    /// the detach that refuses a pair it does not, and the set
    /// `DescribeVerifiedAccessInstances` reports all answer from the same place: the trust
    /// providers the instance's own record names
    /// (`VerifiedAccessInstance$VerifiedAccessTrustProviders`).
    pub fn verified_access_trust_provider_is_attached(
        &self,
        instance_id: &str,
        trust_provider_id: &str,
    ) -> bool {
        self.verified_access_instances
            .get(instance_id)
            .into_iter()
            .flat_map(|instance| instance.verified_access_trust_providers.iter().flatten())
            .any(|attached| {
                attached.verified_access_trust_provider_id.as_deref() == Some(trust_provider_id)
            })
    }

    /// The attach or detach the `ClientToken` `token` performed, or `None` when no such call
    /// has been made under it.
    pub fn verified_access_trust_provider_attachment_for_token(
        &self,
        token: &str,
    ) -> Option<&VerifiedAccessTrustProviderAttachmentRequest> {
        self.verified_access_trust_provider_attachment_client_tokens
            .get(token)
    }

    /// Attach or detach the pair `request` names as of `at`, record it under `client_token`
    /// when the request carried one, and answer the instance as it NOW stands beside the trust
    /// provider as it stands — `None` when this store holds no instance or no trust provider
    /// under the ids it names.
    ///
    /// The ONE way an attachment changes, so what an attach or a detach answers and what a
    /// later `DescribeVerifiedAccessInstances` reports cannot differ: the set it hands back is
    /// the stored record's own. An instance names a trust provider at most once, so an attach
    /// of a pair that is already attached leaves one entry rather than two, and a detach
    /// removes the entry whole. The entry is [`verified_access_trust_provider_condensed`] of
    /// the STORED trust provider, never of a request.
    ///
    /// The instance's `LastUpdatedTime` moves, because the set the instance reports is what
    /// this call changed; the trust provider's does not, because the modelled trust provider
    /// shape carries no member for the attachment and so nothing about that record changed.
    /// The caller-chosen bytes are re-charged across the write — the record this replaces is
    /// refunded and the record it becomes is charged — so a caller cannot grow the store past
    /// [`MAX_STORED_BYTES`] by attaching and detaching repeatedly; a handler asks about that
    /// net change first
    /// ([`verified_access_trust_provider_attachment_payload_bytes`]).
    pub fn set_verified_access_trust_provider_attachment(
        &mut self,
        request: &VerifiedAccessTrustProviderAttachmentRequest,
        client_token: Option<&str>,
        at: SystemTime,
    ) -> Option<(
        crate::ops::types::VerifiedAccessInstance,
        crate::ops::types::VerifiedAccessTrustProvider,
    )> {
        let provider = self
            .verified_access_trust_providers
            .get(&request.verified_access_trust_provider_id)?
            .clone();
        let held = self
            .verified_access_instances
            .get(&request.verified_access_instance_id)?;
        let refund = verified_access_instance_payload_bytes(held, None);
        let mut view = held.clone();
        let mut attached = view.verified_access_trust_providers.take().unwrap_or_default();
        attached.retain(|entry| {
            entry.verified_access_trust_provider_id.as_deref()
                != Some(request.verified_access_trust_provider_id.as_str())
        });
        if request.attach {
            attached.push(verified_access_trust_provider_condensed(&provider));
        }
        view.verified_access_trust_providers = Some(attached);
        // "The last updated time" (`VerifiedAccessInstance$LastUpdatedTime`): the instant this
        // change was applied to the instance.
        view.last_updated_time = Some(iso8601_utc(at));
        let charge = verified_access_instance_payload_bytes(&view, None);
        self.verified_access_instances
            .insert(request.verified_access_instance_id.clone(), view.clone());
        self.release_stored_bytes(refund);
        self.add_stored_bytes(charge);
        if let Some(token) = client_token {
            self.add_stored_bytes(verified_access_trust_provider_attachment_payload_bytes(
                request,
                &provider,
                Some(token),
            ));
            self.verified_access_trust_provider_attachment_client_tokens
                .insert(token.to_string(), request.clone());
        }
        Some((view, provider))
    }

    /// Bring every instance that names the trust provider `trust_provider_id` back in step
    /// with the trust provider store as of `at`: an entry whose trust provider is still stored
    /// is re-derived from it, and one whose trust provider is GONE is dropped.
    ///
    /// The condensed entry an instance carries is a copy of facts the trust provider owns, so
    /// a writer that CHANGES or REMOVES a trust provider lands here rather than leaving an
    /// instance reporting a description the trust provider no longer has, or an id the account
    /// no longer holds a trust provider for.
    /// [`Self::remove_verified_access_trust_provider`] calls it with the record already gone,
    /// which is what ends the attachments a deleted trust provider had.
    ///
    /// An instance whose set this leaves unchanged is NOT rewritten and its `LastUpdatedTime`
    /// does not move: nothing about that instance changed.
    pub fn resync_verified_access_trust_provider_attachments(
        &mut self,
        trust_provider_id: &str,
        at: SystemTime,
    ) {
        let rederived = self
            .verified_access_trust_providers
            .get(trust_provider_id)
            .map(verified_access_trust_provider_condensed);
        let naming: Vec<String> = self
            .verified_access_instances
            .iter()
            .filter(|(_, instance)| {
                instance
                    .verified_access_trust_providers
                    .iter()
                    .flatten()
                    .any(|entry| {
                        entry.verified_access_trust_provider_id.as_deref()
                            == Some(trust_provider_id)
                    })
            })
            .map(|(instance_id, _)| instance_id.clone())
            .collect();
        for instance_id in naming {
            let Some(held) = self.verified_access_instances.get(&instance_id) else {
                continue;
            };
            let refund = verified_access_instance_payload_bytes(held, None);
            let mut view = held.clone();
            let mut attached = view.verified_access_trust_providers.take().unwrap_or_default();
            let mut changed = false;
            match &rederived {
                // The trust provider is still stored, so the entry is replaced IN PLACE: the
                // order an instance reports its trust providers in is the order they were
                // attached in, and re-deriving an entry is not an attachment.
                Some(entry) => {
                    for held_entry in &mut attached {
                        if held_entry.verified_access_trust_provider_id.as_deref()
                            == Some(trust_provider_id)
                            && !same_condensed_trust_provider(held_entry, entry)
                        {
                            *held_entry = entry.clone();
                            changed = true;
                        }
                    }
                }
                None => {
                    let before = attached.len();
                    attached.retain(|held_entry| {
                        held_entry.verified_access_trust_provider_id.as_deref()
                            != Some(trust_provider_id)
                    });
                    changed = attached.len() != before;
                }
            }
            // An instance this leaves exactly as it was is not rewritten at all, so nothing
            // here moves a `LastUpdatedTime` for a change that did not happen.
            if !changed {
                continue;
            }
            view.verified_access_trust_providers = Some(attached);
            view.last_updated_time = Some(iso8601_utc(at));
            let charge = verified_access_instance_payload_bytes(&view, None);
            self.verified_access_instances
                .insert(instance_id.clone(), view);
            self.release_stored_bytes(refund);
            self.add_stored_bytes(charge);
        }
    }
}

/// Whether two condensed trust provider entries report the same trust provider identically,
/// member for member.
///
/// The modelled [`crate::ops::types::VerifiedAccessTrustProviderCondensed`] shape derives no
/// equality of its own, and
/// [`ComAmazonawsEc2Amazonec2State::resync_verified_access_trust_provider_attachments`] has to
/// know whether re-deriving an entry CHANGED it before it rewrites an instance for it.
fn same_condensed_trust_provider(
    held: &crate::ops::types::VerifiedAccessTrustProviderCondensed,
    rederived: &crate::ops::types::VerifiedAccessTrustProviderCondensed,
) -> bool {
    held.verified_access_trust_provider_id == rederived.verified_access_trust_provider_id
        && held.description == rederived.description
        && held.trust_provider_type == rederived.trust_provider_type
        && held.user_trust_provider_type == rederived.user_trust_provider_type
        && held.device_trust_provider_type == rederived.device_trust_provider_type
}
