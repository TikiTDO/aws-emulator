// SPDX-License-Identifier: CC0-1.0

//! The account-level `PublicAccessBlock` configuration, kept in the store that ENFORCES it.
//!
//! S3 Block Public Access is configured at two levels, and this service owns the API over
//! the ACCOUNT level: `PutPublicAccessBlock` "creates or modifies the `PublicAccessBlock`
//! configuration for an Amazon Web Services account", `GetPublicAccessBlock` reports it and
//! `DeletePublicAccessBlock` removes it. What the configuration DOES, though, happens in
//! Amazon S3: "when Amazon S3 receives a request to access a bucket or an object, it
//! determines whether the account or the bucket has a block public access setting applied …
//! if there is an existing block public access setting that prohibits the requested access,
//! Amazon S3 rejects the request" (Amazon S3 User Guide, *Blocking public access to your
//! Amazon S3 storage*) — a public bucket policy, a public ACL and a public read are all
//! judged in that service's own data plane, which does not read this store.
//!
//! So the configuration lives ONCE, in the store that serves and judges those requests
//! (`service_com_amazonaws_s3_amazons3::state::account_public_access_block`), exactly as a
//! bucket's tag set lives in the store that owns the bucket
//! ([`crate::state::change_resource_tags`]) and as an access point's data path does
//! ([`crate::state::access_point_routes`]). A copy of its own here would mean the
//! configuration a client set and the configuration Amazon S3 enforces were different
//! records.
//!
//! This service still applies its OWN bound on how many accounts' configurations it will
//! record ([`crate::state::PUBLIC_ACCESS_BLOCK_QUOTA`]), under the one guard the write takes,
//! so the count a put is judged against cannot move between the check and the insert.
//!
//! LOCK ORDER: every function here takes the object store's lock and holds no other while it
//! does, so it must be called with NO guard of this service's store held — the same
//! discipline, and for the same reason, as this service's other reaches into that store.

use ::service_com_amazonaws_s3_amazons3::state::BucketPublicAccessBlock;

use crate::ops::types::PublicAccessBlockConfiguration;

/// What a read of an account's configuration found.
#[derive(Debug, Clone)]
pub enum Configured {
    /// The configuration that account carries.
    Held(PublicAccessBlockConfiguration),
    /// That account carries none: the state it is in before any put, and the one the delete
    /// returns it to.
    None,
    /// The store that holds these configurations could not be reached at all, which is not an
    /// account holding none — a reader answers a server failure rather than a not-found.
    Unavailable,
}

/// What a write of an account's configuration did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// The configuration is the account's, as the request named it.
    Stored,
    /// As many accounts' configurations are held as will be ([`crate::state::
    /// PUBLIC_ACCESS_BLOCK_QUOTA`], or the object store's own ceiling), and this request
    /// would have added a further one, so nothing was stored.
    StoreFull,
    /// The store could not be reached at all, so nothing was stored.
    Unavailable,
}

/// The configuration the account `account_id` carries.
pub fn configuration<Eng: Send + Sync + 'static>(engine: &Eng, account_id: &str) -> Configured {
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return Configured::Unavailable;
    };
    let held = state.lock().account_public_access_block(account_id);
    match held {
        Some(settings) => Configured::Held(configuration_of(settings)),
        None => Configured::None,
    }
}

/// Whether `BlockPublicPolicy` is on in the configuration the account `account_id` carries.
///
/// The one setting this service's own guardrails ask about: a public ACCESS POINT policy is
/// refused while it is in force, exactly as a public BUCKET policy is in the data plane.
pub fn blocks_public_policy<Eng: Send + Sync + 'static>(engine: &Eng, account_id: &str) -> bool {
    matches!(
        configuration(engine, account_id),
        Configured::Held(held) if held.block_public_policy
    )
}

/// Record `configuration` as the whole configuration of the account `account_id`.
///
/// Create or modify: an account has at most one configuration, so a second put replaces the
/// first rather than adding to it — and an account already recorded stays writable at the
/// bound, because replacing its configuration adds no record. The bound and the insert are
/// applied under ONE guard of the store, so two concurrent puts cannot both pass a bound only
/// one of them fits in.
pub fn record<Eng: Send + Sync + 'static>(
    engine: &Eng,
    account_id: &str,
    configuration: &PublicAccessBlockConfiguration,
) -> Recorded {
    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return Recorded::Unavailable;
    };
    let mut store = state.lock();
    let fresh = store.account_public_access_block(account_id).is_none();
    if fresh && store.account_public_access_blocks() >= crate::state::PUBLIC_ACCESS_BLOCK_QUOTA {
        return Recorded::StoreFull;
    }
    match store.record_account_public_access_block(account_id, settings_of(configuration)) {
        // The object store keeps a ceiling of its own, above this service's quota; past it a
        // further account is not recorded either, and a put that recorded nothing is not
        // reported as stored.
        true => Recorded::Stored,
        false => Recorded::StoreFull,
    }
}

/// Remove the configuration of the account `account_id`, answering whether the store was
/// reached at all.
///
/// Idempotent: whether that account carried one or not, it carries none afterwards — so a
/// caller branches on being able to reach the store and on nothing else.
pub fn forget<Eng: Send + Sync + 'static>(engine: &Eng, account_id: &str) -> bool {
    ::service_com_amazonaws_s3_amazons3::state::forget_account_public_access_block_in(
        engine, account_id,
    )
    .is_some()
}

/// The four settings as this service's model declares them, from the record the object store
/// holds.
///
/// Every member of `PublicAccessBlockConfiguration` is a `Setting` (a boolean) carrying
/// `@default: false`, so the two shapes carry exactly the same four facts and the mapping is
/// member for member.
fn configuration_of(settings: BucketPublicAccessBlock) -> PublicAccessBlockConfiguration {
    PublicAccessBlockConfiguration {
        block_public_acls: settings.block_public_acls,
        ignore_public_acls: settings.ignore_public_acls,
        block_public_policy: settings.block_public_policy,
        restrict_public_buckets: settings.restrict_public_buckets,
    }
}

/// [`configuration_of`] the other way round: the record the object store holds, from the
/// configuration a request carried.
fn settings_of(configuration: &PublicAccessBlockConfiguration) -> BucketPublicAccessBlock {
    BucketPublicAccessBlock {
        block_public_acls: configuration.block_public_acls,
        ignore_public_acls: configuration.ignore_public_acls,
        block_public_policy: configuration.block_public_policy,
        restrict_public_buckets: configuration.restrict_public_buckets,
    }
}
