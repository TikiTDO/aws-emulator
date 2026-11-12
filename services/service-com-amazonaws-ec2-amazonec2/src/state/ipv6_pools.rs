// SPDX-License-Identifier: CC0-1.0

//! The IPv6 address pools the account holds: what one is stored as, the insert that mints
//! its id and charges its bytes, the removal, and the projection `DescribeIpv6Pools` reports
//! it through.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there. The public IPv4 pool store this one mirrors lives in
//! `byoip_and_capacity_reservations`, beside the BYOIP provision that creates a pool of
//! either family.

use std::collections::BTreeSet;

use super::{
    ComAmazonawsEc2Amazonec2State, MAX_STORED_RECORDS_PER_STORE, STORED_RECORD_OVERHEAD_BYTES,
    canonical_ipv6_cidr, split_ipv6_cidr, tag_list_bytes,
};
use crate::ops::types::{Ipv6Pool, PoolCidrBlock, Tag};
use crate::scaffolded::Scaffolded;

/// The `ResourceType` and id prefix of an IPv6 address pool — `ipv6pool-ec2`, which is also
/// what [`super::AddressFamily::pool_resource_type`] answers for an IPv6 range, defined here
/// so the pool store and the family read one spelling of it.
pub const IPV6_POOL_RESOURCE_TYPE: &str = "ipv6pool-ec2";

/// EC2's error code for an IPv6 pool id a READ named that the account holds no pool for.
///
/// The read-path spelling of the convention EC2's resource-id refusals follow —
/// `Invalid<Resource>ID.NotFound`, as `InvalidVpcID.NotFound` and `InvalidSubnetID.NotFound`
/// do — rather than the `Invalid<Resource>.NotFound` the mutation path of a pool uses
/// ([`super::INVALID_PUBLIC_IPV4_POOL_NOT_FOUND`] is the other family's), because a client
/// branches on the code and the two paths are different questions.
pub const INVALID_IPV6_POOL_ID_NOT_FOUND: &str = "InvalidIpv6PoolID.NotFound";

/// Refuse a read that named an IPv6 address pool this account does not hold: HTTP 400
/// [`INVALID_IPV6_POOL_ID_NOT_FOUND`].
///
/// The message names the pool id the REQUEST sent, after the SHAPE whose id it is
/// (`Ipv6Pool`) rather than after "the pool ID" the public IPv4 pool surface names its own
/// with, so a caller can tell both which kind of pool and which of several ids was not found
/// from the text alone.
pub fn invalid_ipv6_pool_id_not_found<E>(pool_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_IPV6_POOL_ID_NOT_FOUND,
        400,
        format!("Ipv6Pool '{pool_id}' does not exist."),
    )
}

/// EC2's error code for a value that is no IPv6 address pool id AT ALL, answered BEFORE the
/// id is looked up — the `.Malformed` counterpart of [`INVALID_IPV6_POOL_ID_NOT_FOUND`],
/// keyed to the same resource, so a caller can tell "not an id of this kind" from "no such
/// pool" by the code alone.
pub const INVALID_IPV6_POOL_ID_MALFORMED: &str = "InvalidIpv6PoolID.Malformed";

/// Whether `pool_id` is an IPv6 address pool id EC2 could have ISSUED.
///
/// The pool id member carries no `@pattern` (`smithy_query(query="shape",shape=
/// "Ipv6PoolEc2Id")` is a plain string), so the FORM an id of this kind has is read here:
/// [`IPV6_POOL_RESOURCE_TYPE`] followed by one of EC2's two published id forms, excluding
/// the all-zero placeholder a mint counting from one can never produce — the same reading
/// [`super::issued_id_is_well_formed`] gives every other kind recorded refusing a
/// placeholder on its form.
pub fn ipv6_pool_id_is_well_formed(pool_id: &str) -> bool {
    super::issued_id_is_well_formed(IPV6_POOL_RESOURCE_TYPE, pool_id)
}

/// Refuse a value that is not an IPv6 address pool id at all: HTTP 400
/// [`INVALID_IPV6_POOL_ID_MALFORMED`], naming the resource by the EC2 resource token of an
/// IPv6 address pool ([`IPV6_POOL_RESOURCE_TYPE`]) around the value the REQUEST sent.
///
/// `The ipv6pool-ec2 ID '<id>' is malformed` — the sentence EC2 answers a pool id of this
/// kind it cannot read, as `The coip-pool ID '<id>' is malformed` is for the customer-owned
/// family, so the resource is named by its wire token rather than by prose. Only the first
/// [`super::MAX_ECHOED_RESOURCE_ID_CHARS`] characters of the value are named, the way every
/// refusal of this crate that quotes a caller-chosen id bounds its echo.
pub fn invalid_ipv6_pool_id_malformed<E>(pool_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_IPV6_POOL_ID_MALFORMED,
        400,
        format!(
            "The {IPV6_POOL_RESOURCE_TYPE} ID '{echoed}' is malformed",
            echoed = super::echoed_resource_id(pool_id),
        ),
    )
}

/// One IPv6 address pool the account holds: the pool's own facts, beside the address ranges
/// provisioned to it.
///
/// NOT the modelled [`Ipv6Pool`] itself, for the reason
/// [`super::PublicIpv4PoolRecord`] is not the modelled pool of its family: what the shape
/// reports about the pool's ranges is a FUNCTION of the ranges the pool holds, so storing it
/// would be storing a cache that can drift. The ranges are the one source of truth and
/// [`ComAmazonawsEc2Amazonec2State::ipv6_pool_view`] derives the rest on read.
#[derive(Debug, Clone)]
pub struct Ipv6PoolRecord {
    /// The pool's own id (`ipv6pool-ec2-…`), minted by the store.
    pub pool_id: String,
    /// "The description for the address pool" (`Ipv6Pool$Description`) — for a pool a BYOIP
    /// provision created, the description that provision gave the range and the pool ("A
    /// description for the address range and the address pool",
    /// `ProvisionByoipCidrRequest$Description`).
    pub description: Option<String>,
    /// "Any tags for the address pool" (`Ipv6Pool$Tags`) — the tags
    /// `ProvisionByoipCidrRequest$PoolTagSpecifications` asked for.
    pub tags: Vec<Tag>,
    /// The address ranges provisioned to the pool, each a canonical IPv6 CIDR block
    /// ([`canonical_ipv6_cidr`]). A set, so one block cannot be provisioned to one pool
    /// twice.
    pub cidrs: BTreeSet<String>,
}

/// What a provision stores for the IPv6 pool it creates, read from the request before the
/// store is touched — the IPv6 counterpart of [`super::PublicIpv4PoolSpec`].
#[derive(Debug, Clone)]
pub struct Ipv6PoolSpec {
    /// "The description for the address pool" (`Ipv6Pool$Description`).
    pub description: Option<String>,
    /// The tags the provision asked to put on the pool.
    pub tags: Vec<Tag>,
    /// The address ranges the pool starts with.
    pub cidrs: Vec<String>,
}

/// The bytes one stored IPv6 pool holds.
///
/// The ONE definition of that charge, the way [`super::public_ipv4_pool_payload_bytes`] is
/// for a pool of the other family: the fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored
/// record costs, plus the caller-chosen text the record carries — its description, its tags
/// and its address ranges. Every path that stores a pool charges exactly this and every path
/// that drops one gives back exactly this.
pub fn ipv6_pool_payload_bytes(record: &Ipv6PoolRecord) -> usize {
    let cidr_bytes: usize = record
        .cidrs
        .iter()
        .map(String::len)
        .fold(0usize, usize::saturating_add);
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(record.description.as_deref().map_or(0, str::len))
        .saturating_add(tag_list_bytes(&record.tags))
        .saturating_add(cidr_bytes)
}

/// The address-order key of the IPv6 CIDR block `cidr`: its base address and its prefix
/// length, so blocks compare by the addresses they cover rather than by how they are spelled
/// (`2001:…` sorts before `::1` as text).
///
/// The one reading of that order outside this module, so an operation that reports IPv6
/// blocks in address order — the order [`ComAmazonawsEc2Amazonec2State::ipv6_pool_view`]
/// reports a pool's own blocks in — orders them the same way the store does. A value that is
/// no IPv6 CIDR block at all has no address to place, so it sorts after every block that has
/// one rather than being dropped from a report.
pub fn ipv6_cidr_address_order_key(cidr: &str) -> (u128, u8) {
    split_ipv6_cidr(cidr).unwrap_or((u128::MAX, u8::MAX))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another IPv6 address pool fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]).
    pub fn has_ipv6_pool_capacity(&self) -> bool {
        self.ipv6_pools.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store `spec` as a new IPv6 address pool and answer the id minted for it.
    ///
    /// The id is minted HERE from the store's own counter, so no caller can choose a pool id,
    /// exactly as [`Self::insert_public_ipv4_pool`] does for a pool of the other family. The
    /// ranges are canonicalised on the way in, so a writer cannot create a range no lookup
    /// can name.
    ///
    /// `None`, with NOTHING written, when a range of the spec is not an IPv6 CIDR block at
    /// all — a pool created without the range it was asked to hold is not the pool the caller
    /// asked for — when the store is at [`MAX_STORED_RECORDS_PER_STORE`], or when the bytes
    /// the pool would hold would cross the store's byte budget. All three are asked before
    /// the first write, so a refused create leaves the store exactly as it was.
    pub fn insert_ipv6_pool(&mut self, spec: Ipv6PoolSpec) -> Option<String> {
        let mut cidrs = BTreeSet::new();
        for cidr in &spec.cidrs {
            cidrs.insert(canonical_ipv6_cidr(cidr).ok()?);
        }
        let record = Ipv6PoolRecord {
            // Minted after the ceilings below would be asked, so a refused create does not
            // burn an id; `next_id` is the only source of one.
            pool_id: String::new(),
            description: spec.description,
            tags: spec.tags,
            cidrs,
        };
        let charge = ipv6_pool_payload_bytes(&record);
        if !self.has_ipv6_pool_capacity() || self.would_exceed_stored_bytes(charge) {
            return None;
        }
        let pool_id = self.next_id(IPV6_POOL_RESOURCE_TYPE);
        self.add_stored_bytes(charge);
        self.ipv6_pools.insert(
            pool_id.clone(),
            Ipv6PoolRecord {
                pool_id: pool_id.clone(),
                ..record
            },
        );
        Some(pool_id)
    }

    /// Drop the IPv6 address pool `pool_id`, giving its bytes back to the budget, and answer
    /// whether the store held one.
    pub fn remove_ipv6_pool(&mut self, pool_id: &str) -> bool {
        match self.ipv6_pools.remove(pool_id) {
            Some(record) => {
                self.release_stored_bytes(ipv6_pool_payload_bytes(&record));
                true
            }
            None => false,
        }
    }

    /// The pool `record` as the API reports it: the modelled [`Ipv6Pool`], whose
    /// `PoolCidrBlocks` are the ranges the record holds.
    ///
    /// "The CIDR blocks for the address pool" (`Ipv6Pool$PoolCidrBlocks`) are reported in
    /// ADDRESS order — which the record's key order (the block's text) is not, `2001:…`
    /// sorting before `::1` as text — so what a describe reports does not depend on how the
    /// blocks happen to be spelled. Reads NOTHING into the store: every value here is
    /// computed from what is already held.
    pub fn ipv6_pool_view(&self, record: &Ipv6PoolRecord) -> Ipv6Pool {
        let mut blocks: Vec<(u128, u8, &String)> = record
            .cidrs
            .iter()
            .filter_map(|cidr| {
                split_ipv6_cidr(cidr).map(|(base, prefix_length)| (base, prefix_length, cidr))
            })
            .collect();
        blocks.sort_unstable();
        Ipv6Pool {
            pool_id: Some(record.pool_id.clone()),
            description: record.description.clone(),
            pool_cidr_blocks: Some(
                blocks
                    .into_iter()
                    .map(|(_, _, cidr)| PoolCidrBlock {
                        cidr: Some(cidr.clone()),
                    })
                    .collect(),
            ),
            tags: Some(record.tags.clone()),
        }
    }
}
