// SPDX-License-Identifier: CC0-1.0

//! The prefix list a security group rule names as its source or destination: whether the
//! account can reference it at all.
//!
//! "The prefix list IDs" (`IpPermission$PrefixListIds`) is one of the grants a rule may
//! carry, and a rule granting a prefix list grants exactly the address ranges that list
//! holds. A value that names no prefix list the account can see therefore grants nothing
//! and can never grant anything, so the authorize is refused with the prefix list's own
//! not-found class ([`crate::state::invalid_prefix_list_id_not_found`]) rather than storing a
//! rule pointing at an id that resolves to nothing — the same reading every other operation
//! of this crate that resolves a `PrefixListId` makes
//! ([`crate::ops::create_route`] for a route's destination,
//! [`crate::ops::describe_managed_prefix_lists`] for a report of one).
//!
//! Which lists the account can see is the SAME derivation
//! `DescribeManagedPrefixLists` reports — "describes your managed prefix lists and any
//! Amazon Web Services-managed prefix lists": the lists this account created
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::managed_prefix_lists`]) and the ones the
//! Region publishes for its services ([`crate::state::service_prefix_lists_in_region`]) —
//! so one id is visible to a rule exactly when it is visible to a report.
//!
//! This reading is shared by both DIRECTIONS of an authorize, inbound and outbound: a rule's
//! grant is read the same way whichever side of the group it is added to, the way every
//! other part of reading one permission is
//! ([`crate::state::prepared_ip_permission`]). It lives here, beside the outbound
//! authorize, because `src/state.rs` is over this project's per-file ceiling and cannot be
//! edited.

use crate::ops::types::IpPermission;
use crate::scaffolded::Scaffolded;
use crate::state::{
    invalid_prefix_list_id_not_found, managed_prefix_lists_in_region,
    ComAmazonawsEc2Amazonec2State,
};

/// Refuse `permissions` if any of them names a prefix list `region` and this account hold
/// none of, naming the id the request sent.
///
/// Asked about EVERY permission of the request before anything is written, the way every
/// other bound an authorize enforces is, so a request that references one unknown list
/// stores no rule for the grants beside it. The Region's published catalog is derived ONCE
/// for the whole request rather than per id, because its size is a property of the Region
/// and not of what the caller sent.
pub fn refuse_unknown_prefix_lists<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    region: &str,
    permissions: &[IpPermission],
) -> Result<(), Scaffolded<E>> {
    let published = managed_prefix_lists_in_region(region);
    for permission in permissions {
        for entry in permission.prefix_list_ids.iter().flatten() {
            let prefix_list_id = entry.prefix_list_id.as_deref().unwrap_or_default();
            let visible = store.managed_prefix_lists.contains_key(prefix_list_id)
                || published
                    .iter()
                    .any(|list| list.prefix_list_id == prefix_list_id);
            if !visible {
                return Err(invalid_prefix_list_id_not_found(prefix_list_id));
            }
        }
    }
    Ok(())
}
