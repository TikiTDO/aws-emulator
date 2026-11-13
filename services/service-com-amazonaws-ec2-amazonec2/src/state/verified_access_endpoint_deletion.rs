// SPDX-License-Identifier: CC0-1.0

//! Deleting a Verified Access endpoint: the move a delete leaves the endpoint in, the removal
//! the clock owes it afterwards, and the answered record a retry of that delete is served
//! from.
//!
//! "Delete an Amazon Web Services Verified Access endpoint" (`DeleteVerifiedAccessEndpoint`)
//! is work the service reports as UNDER WAY rather than done: the modelled
//! `VerifiedAccessEndpointStatusCode` catalog carries `deleting` beside `active`
//! (`smithy_query(query="shape",shape="VerifiedAccessEndpointStatusCode")`), so the endpoint
//! is left in that non-terminal status and the clock owes it the REMOVAL one
//! [`TRANSITION_DELAY`] later ([`ComAmazonawsEc2Amazonec2State::verified_access_endpoint_delete_at`],
//! applied by [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`]) — the same
//! arrangement, on the same clock, as the `pending` -> `active` move a create leaves the
//! endpoint in, and the one [`ComAmazonawsEc2Amazonec2State::begin_transit_gateway_deletion`]
//! and [`ComAmazonawsEc2Amazonec2State::begin_ipam_deletion`] give those resources. So a
//! caller that reads straight back still sees the delete under way, and a later one finds the
//! endpoint gone, which is what `DescribeVerifiedAccessEndpoints` no longer carrying it means.
//!
//! A removal rather than a further stored status: nothing in this emulator reclaims a record
//! that keeps reporting a terminal state, the endpoint's slot is charged against
//! [`MAX_VERIFIED_ACCESS_ENDPOINTS`] and its caller-chosen bytes against
//! [`MAX_STORED_BYTES`] for as long as it is stored, and an endpoint on its way out is one the
//! Verified Access INSTANCE it belongs to no longer has to wait for
//! ([`ComAmazonawsEc2Amazonec2State::verified_access_instance_dependencies`]).
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every other
//! primitive there.

use super::*;

/// A Verified Access endpoint whose deletion has been asked for and has not settled yet: the
/// `VerifiedAccessEndpointStatusCode` enum's `deleting` member
/// (`smithy_query(query="shape",shape="VerifiedAccessEndpointStatusCode")`), which is what
/// `DeleteVerifiedAccessEndpoint` answers with.
pub const VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING: &str = "deleting";

/// How many answered `DeleteVerifiedAccessEndpoint` calls this store remembers for their
/// idempotency tokens.
///
/// The same terms as [`MAX_REMEMBERED_VERIFIED_ACCESS_GROUP_DELETIONS`], and for the same
/// reason: a delete FREES a live record's slot once the removal falls due, so
/// [`MAX_VERIFIED_ACCESS_ENDPOINTS`] does not bound a history that outlives the endpoint. The
/// newest deletes are kept and the oldest dropped, so a create-and-delete loop cannot grow the
/// history without end and a delete is never refused because it is full.
pub const MAX_REMEMBERED_VERIFIED_ACCESS_ENDPOINT_DELETIONS: usize = 200;

/// Whether the endpoint `endpoint` has had its deletion asked for already — the one definition
/// of "this endpoint is on its way out", so no path restarts the clock on one and no reader
/// has to spell the status itself.
pub fn verified_access_endpoint_is_deleting(
    endpoint: &crate::ops::types::VerifiedAccessEndpoint,
) -> bool {
    endpoint
        .status
        .as_ref()
        .and_then(|status| status.code.as_deref())
        == Some(VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING)
}

/// "A custom identifier that is prepended to the DNS name that is generated for the endpoint"
/// (`CreateVerifiedAccessEndpointRequest$EndpointDomainPrefix`), read back off the generated
/// name of the endpoint `endpoint_id` — `None` for a name that carries no prefix.
///
/// The prefix is the one part of [`verified_access_endpoint_domain`] the CALLER chose the
/// length of, so it is the part [`verified_access_endpoint_payload_bytes`] charges; the stored
/// endpoint keeps the whole name rather than the prefix on its own, so this is how a removal
/// gives back exactly the bytes the create took.
pub fn verified_access_endpoint_domain_prefix<'a>(
    endpoint_id: &str,
    endpoint_domain: Option<&'a str>,
) -> Option<&'a str> {
    let domain = endpoint_domain?;
    let position = domain.find(&format!("{endpoint_id}."))?;
    domain[..position].strip_suffix('.')
}

impl ComAmazonawsEc2Amazonec2State {
    /// Start deleting the Verified Access endpoint `endpoint_id` names, and answer it as the
    /// store then holds it — `None` when the account holds no such endpoint.
    ///
    /// The endpoint is left in the non-terminal [`VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING`]
    /// status with its REMOVAL owed to the clock one [`TRANSITION_DELAY`] later (see the module
    /// docs), and any move the clock still owed it is superseded: an endpoint on its way out is
    /// not going to land in the `active` state its create was moving it to.
    ///
    /// The timestamps are left as the store holds them: "The last updated time"
    /// (`VerifiedAccessEndpoint$LastUpdatedTime`) is when the endpoint's own configuration last
    /// changed, which a delete does not change, and "The deletion time"
    /// (`VerifiedAccessEndpoint$DeletionTime`) is a fact of a deletion that has FINISHED — and
    /// an endpoint whose deletion has finished is one no read of this store reports, because
    /// the removal took it away.
    ///
    /// Asking twice is not asking again: an endpoint whose deletion has already been asked for
    /// is not one a further delete acts on, so this answers `None` for it rather than restarting
    /// its clock or reporting a second deletion — the caller is refused the way an id that names
    /// no endpoint at all is (see `delete_verified_access_endpoint`).
    ///
    /// `client_token` is the DELETE's own token: when the caller gave one, the answered record
    /// is kept under it ([`Self::verified_access_endpoint_deletion`]) so a retry of that delete
    /// answers what the first one answered even after the removal has taken the endpoint away,
    /// and the bytes it keeps are charged against the store's byte budget on the terms
    /// [`verified_access_endpoint_payload_bytes`] took them on. That history is bounded by
    /// [`MAX_REMEMBERED_VERIFIED_ACCESS_ENDPOINT_DELETIONS`] entries, the oldest dropped (and
    /// its bytes given back) to make room.
    pub fn begin_verified_access_endpoint_deletion(
        &mut self,
        endpoint_id: &str,
        client_token: Option<&str>,
    ) -> Option<crate::ops::types::VerifiedAccessEndpoint> {
        let record = self.verified_access_endpoints.get_mut(endpoint_id)?;
        if verified_access_endpoint_is_deleting(&record.view) {
            return None;
        }
        record.view.status = Some(crate::ops::types::VerifiedAccessEndpointStatus {
            code: Some(VERIFIED_ACCESS_ENDPOINT_STATUS_DELETING.to_string()),
            message: None,
        });
        record.pending = None;
        // A report of the STORED endpoint reports what was HELD, in which "A description for the
        // Amazon Web Services Verified Access endpoint"
        // (`VerifiedAccessEndpoint$Description`) a caller never set is empty text rather than an
        // absent member — the create is what echoes the request it was given. The same reading
        // [`Self::remove_verified_access_group`] answers a deleted group with.
        let mut answered = record.clone();
        if answered.view.description.is_none() {
            answered.view.description = Some(String::new());
        }
        self.verified_access_endpoint_delete_at
            .insert(endpoint_id.to_string(), Instant::now() + TRANSITION_DELAY);
        if let Some(token) = client_token {
            self.remember_verified_access_endpoint_deletion(token, endpoint_id, answered.clone());
        }
        Some(answered.view)
    }

    /// Keep `record` — the endpoint as this delete answered it — under the delete's own
    /// `client_token`.
    ///
    /// One token names ONE delete, so a token used again replaces what it named rather than
    /// leaving the history two entries to answer with, and the history is held to
    /// [`MAX_REMEMBERED_VERIFIED_ACCESS_ENDPOINT_DELETIONS`] entries with the oldest dropped.
    fn remember_verified_access_endpoint_deletion(
        &mut self,
        client_token: &str,
        endpoint_id: &str,
        record: VerifiedAccessEndpointRecord,
    ) {
        if let Some(position) = self
            .verified_access_endpoint_deletions
            .iter()
            .position(|(remembered, _)| remembered == client_token)
        {
            self.forget_verified_access_endpoint_deletion(position);
        }
        let domain_prefix = verified_access_endpoint_domain_prefix(
            endpoint_id,
            record.view.endpoint_domain.as_deref(),
        );
        self.add_stored_bytes(verified_access_endpoint_payload_bytes(
            &record,
            domain_prefix,
            Some(client_token),
        ));
        self.verified_access_endpoint_deletions
            .push_back((client_token.to_string(), record));
        while self.verified_access_endpoint_deletions.len()
            > MAX_REMEMBERED_VERIFIED_ACCESS_ENDPOINT_DELETIONS
        {
            self.forget_verified_access_endpoint_deletion(0);
        }
    }

    /// The endpoint the delete that carried `client_token` answered, while this store still
    /// remembers that delete.
    ///
    /// The history is bounded ([`MAX_REMEMBERED_VERIFIED_ACCESS_ENDPOINT_DELETIONS`]), so a
    /// token whose delete has been dropped answers `None` — the same answer as a token no
    /// delete of this store ever carried.
    pub fn verified_access_endpoint_deletion(
        &self,
        client_token: &str,
    ) -> Option<&crate::ops::types::VerifiedAccessEndpoint> {
        self.verified_access_endpoint_deletions
            .iter()
            .find(|(remembered, _)| remembered == client_token)
            .map(|(_, record)| &record.view)
    }

    /// Drop the remembered deletion at `position`, giving the bytes it held back to the store's
    /// budget: the record leaves the history on the same terms it entered it on.
    fn forget_verified_access_endpoint_deletion(&mut self, position: usize) {
        if let Some((token, record)) = self.verified_access_endpoint_deletions.remove(position) {
            let endpoint_id = record
                .view
                .verified_access_endpoint_id
                .as_deref()
                .unwrap_or_default();
            let domain_prefix = verified_access_endpoint_domain_prefix(
                endpoint_id,
                record.view.endpoint_domain.as_deref(),
            );
            self.release_stored_bytes(verified_access_endpoint_payload_bytes(
                &record,
                domain_prefix,
                Some(&token),
            ));
        }
    }

    /// Apply the removal the clock owes every endpoint a delete left `deleting`, as of `now`.
    ///
    /// An endpoint a `DeleteVerifiedAccessEndpoint` left `deleting` was GONE at the instant that
    /// call recorded for it, whether or not anybody called again — so the removal is applied
    /// from [`Self::materialize_due_transitions`], its one caller, before anything reads the
    /// store.
    pub(super) fn materialize_due_verified_access_endpoint_deletions(&mut self, now: Instant) {
        let due: Vec<String> = self
            .verified_access_endpoint_delete_at
            .iter()
            .filter(|(_, due_at)| now >= **due_at)
            .map(|(endpoint_id, _)| endpoint_id.clone())
            .collect();
        for endpoint_id in due {
            self.verified_access_endpoint_delete_at.remove(&endpoint_id);
            self.remove_verified_access_endpoint(&endpoint_id);
        }
    }

    /// Remove the Verified Access endpoint `endpoint_id` names and answer the record it was
    /// held as, or `None` when the store holds no such endpoint.
    ///
    /// The one way an endpoint LEAVES the store, so what a read of the account reports and what
    /// the store charges cannot disagree with each other. The `CreateVerifiedAccessEndpoint`
    /// token that minted it goes with it — a token must never answer with an endpoint that is no
    /// longer stored — and the caller-chosen bytes the record held go back to the store's budget
    /// on exactly the terms [`verified_access_endpoint_payload_bytes`] took them on, the
    /// `EndpointDomainPrefix` the generated domain carries included
    /// ([`verified_access_endpoint_domain_prefix`]).
    pub fn remove_verified_access_endpoint(
        &mut self,
        endpoint_id: &str,
    ) -> Option<VerifiedAccessEndpointRecord> {
        let record = self.verified_access_endpoints.remove(endpoint_id)?;
        let minting_token = self
            .verified_access_endpoint_client_tokens
            .iter()
            .find(|(_, minted)| minted.as_str() == endpoint_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = &minting_token {
            self.verified_access_endpoint_client_tokens.remove(token);
        }
        let domain_prefix = verified_access_endpoint_domain_prefix(
            endpoint_id,
            record.view.endpoint_domain.as_deref(),
        );
        self.release_stored_bytes(verified_access_endpoint_payload_bytes(
            &record,
            domain_prefix,
            minting_token.as_deref(),
        ));
        self.verified_access_endpoint_delete_at.remove(endpoint_id);
        Some(record)
    }
}
