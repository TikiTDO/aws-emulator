// SPDX-License-Identifier: CC0-1.0

//! The VPC ORIGIN slice of this service's state.
//!
//! "An Amazon CloudFront VPC origin" is its own resource kind — addressed by a minted `Id`,
//! carrying the endpoint configuration it was created with — so everything about how one is
//! persisted and projected lives here rather than in the parent module: the record, the id
//! and ARN one is addressed by, the lookup every read goes through, and the modelled errors
//! the VPC origin operations answer with. The parent
//! [`ComAmazonawsCloudfrontCloudfront20200531State`] holds the one map (`vpc_origins`) and
//! this module holds everything that reads or writes it.
//!
//! One thing is owed by the CLOCK: a VPC origin CloudFront has just created reports
//! [`VPC_ORIGIN_STATUS_DEPLOYING`] until its endpoint has been deployed to the edge
//! locations, and [`STATUS_DEPLOYED`] after ([`super::PROPAGATION_DELAY`], the span every
//! other propagation of this service is owed on). [`StoredVpcOrigin::settle`] applies it,
//! and nothing ever sleeps for it.

use super::*;
use crate::ops::types::{VpcOrigin, VpcOriginEndpointConfig};

/// The prefix every VPC origin id this service mints carries.
///
/// The model states no pattern for `VpcOrigin$Id`, but its own examples spell a VPC origin
/// id `vo_` followed by a token body, so a minted id carries that namespace label exactly as
/// a distribution tenant's ([`DISTRIBUTION_TENANT_ID_PREFIX`]) and a trust store's
/// ([`TRUST_STORE_ID_PREFIX`]) carry theirs. The body is this mint's
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::allocate_vpc_origin_id`]).
pub const VPC_ORIGIN_ID_PREFIX: &str = "vo_";

/// `VpcOrigin$Status` while the VPC origin's endpoint is still being deployed to the edge
/// locations — the status the model's own `CreateVpcOrigin` example answers with, before the
/// terminal [`STATUS_DEPLOYED`] its read examples report.
///
/// A VPC origin CloudFront has just accepted is not serving origin requests yet, so the
/// create answers this non-terminal status and the clock — not a later call — owes it
/// [`STATUS_DEPLOYED`].
pub const VPC_ORIGIN_STATUS_DEPLOYING: &str = "Deploying";

/// The most VPC origins this emulator holds.
///
/// `CreateVpcOrigin` models `EntityLimitExceeded` ("The entity limit has been exceeded"), so
/// the store is bounded; the bound is this emulator's, CHOSEN like [`MAX_TRUST_STORES`]
/// rather than read off a published quota, and it is an entry count refused before anything
/// is minted.
pub const MAX_VPC_ORIGINS: usize = 1_000;

/// The sentence CloudFront answers an id no VPC origin answers to with.
///
/// It states the CONDITION and echoes no part of the request, as every other not-found
/// sentence of this service states its own ([`TRUST_STORE_NOT_FOUND_MESSAGE`]), so the same
/// sentence answers an unknown id and an id of no form this service issues.
pub const VPC_ORIGIN_NOT_FOUND_MESSAGE: &str = "The specified VPC origin does not exist.";

/// The modelled `EntityNotFound` (HTTP 404) for an `Id` no stored VPC origin answers to.
///
/// Takes no id: the sentence is fixed ([`VPC_ORIGIN_NOT_FOUND_MESSAGE`]), so every operation
/// that resolves a VPC origin refuses this condition the same way.
pub fn vpc_origin_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(VPC_ORIGIN_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The modelled `InconsistentQuantities` message for the one list wrapper a
/// `VpcOriginEndpointConfig` reaches — its `OriginSslProtocols` — or `None` when the two
/// agree.
///
/// The SAME rule [`config_quantity_mismatch`] applies to a distribution configuration: every
/// CloudFront list travels in a wrapper stating its own length (`Quantity`) beside the list
/// itself (`Items`), and the service refuses a request where the two disagree — through the
/// parent module's one `quantity_disagreement` sentence, so no two of these walks can word
/// the refusal differently.
pub fn vpc_origin_endpoint_config_quantity_mismatch(
    config: &VpcOriginEndpointConfig,
) -> Option<String> {
    let protocols = config.origin_ssl_protocols.as_ref()?;
    quantity_disagreement(
        "OriginSslProtocols",
        protocols.quantity,
        protocols.items.len(),
    )
}

/// One CloudFront VPC origin, as this emulator persists it.
///
/// The record is the single source of truth for the VPC origin's id, ARN, status, the
/// endpoint configuration it was created with, the instants it was created and last written,
/// its version token and its tags — so the create and every later read answer from it rather
/// than from the request being served.
#[derive(Debug, Clone)]
pub struct StoredVpcOrigin {
    /// `VpcOrigin$Id` — "The VPC origin ID": the token minted at creation, which is also the
    /// key this record is held under and the last segment of its ARN.
    pub id: String,
    /// `VpcOrigin$Arn` — "The VPC origin ARN", built from the minted id once
    /// ([`vpc_origin_arn`]) and stored, so every read reports the same ARN.
    pub arn: String,
    /// [`VPC_ORIGIN_STATUS_DEPLOYING`] until [`StoredVpcOrigin::deployed_due`], then
    /// [`STATUS_DEPLOYED`].
    pub status: String,
    /// The instant this VPC origin is owed [`STATUS_DEPLOYED`]. Stored rather than slept for;
    /// every stored VPC origin has one, because every one of them was created by a call.
    pub deployed_due: SystemTime,
    /// `VpcOrigin$CreatedTime` — "The VPC origin created time": the instant the call that
    /// minted this record ran, written once so every later read reports the same one.
    pub created_time: SystemTime,
    /// `VpcOrigin$LastModifiedTime` — "The VPC origin last modified time", written by the
    /// call that last wrote this record.
    pub last_modified_time: SystemTime,
    /// The endpoint configuration as submitted, stored whole: a later read answers from this
    /// record, down to the members the request left out.
    pub endpoint_config: VpcOriginEndpointConfig,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this VPC origin.
    pub etag: String,
    /// The tags submitted with the create, held in the one representation every tagged
    /// resource of this service uses, so the tagging surface reads the very set that was
    /// stored.
    pub tags: TagSet,
}

impl StoredVpcOrigin {
    /// Apply the deployment transition the clock owes at `now`.
    ///
    /// Idempotent and monotonic, like every other settlement in this store: it only ever
    /// moves the non-terminal status to the terminal one, only once the deadline has passed,
    /// so a read that calls it cannot change what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == VPC_ORIGIN_STATUS_DEPLOYING && now >= self.deployed_due {
            self.status = STATUS_DEPLOYED.to_string();
        }
    }

    /// The `VpcOrigin` projection every VPC origin operation returns, read from the one
    /// stored record.
    ///
    /// `AccountId` is "The account ID of the Amazon Web Services account that owns the VPC
    /// origin", which is this emulator's own account ([`::engine::emulator_account_id`]) for
    /// every VPC origin it holds: each was created through this control plane.
    pub fn to_vpc_origin(&self) -> VpcOrigin {
        VpcOrigin {
            id: self.id.clone(),
            arn: self.arn.clone(),
            account_id: Some(::engine::emulator_account_id().to_string()),
            status: self.status.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_time),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            vpc_origin_endpoint_config: self.endpoint_config.clone(),
        }
    }
}

impl ComAmazonawsCloudfrontCloudfront20200531State {
    /// Allocate a VPC origin id no stored VPC origin holds: [`VPC_ORIGIN_ID_PREFIX`] followed
    /// by a minted token, drawn from the same mint as every other id this service generates
    /// so no two are minted the same.
    pub fn allocate_vpc_origin_id(&mut self) -> String {
        loop {
            let candidate = format!("{VPC_ORIGIN_ID_PREFIX}{}", self.mint(ID_BODY_LEN));
            if !self.vpc_origins.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// The VPC origin `id` names, or `None` when the store holds none.
    ///
    /// Every VPC origin operation addresses one by "The VPC origin ID", which is the key this
    /// record is held under, so the lookup is that one map read rather than a scan.
    pub fn vpc_origin(&self, id: &str) -> Option<&StoredVpcOrigin> {
        self.vpc_origins.get(id)
    }
}
