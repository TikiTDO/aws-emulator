// SPDX-License-Identifier: CC0-1.0

//! The organization the account this emulator presents belongs to, read from the
//! control plane that owns it.
//!
//! Membership of an organization, and which of its accounts is the management
//! account, are established in AWS Organizations: the account a `CreateOrganization`
//! is made from becomes that organization's management account, and no TaxSettings
//! request puts an account anywhere. So this service does not keep its own copy of
//! that fact and is not told it out of band — it READS it from the Organizations
//! store in the one shared engine, which is where every client that creates an
//! organization writes it. That is what makes the standing an organization-level
//! write is reserved to reachable for a client that speaks only HTTP.
//!
//! A one-directional, guard-free read: it takes the Organizations store's lock,
//! holds no other while it does, hands back owned values and writes nothing — so
//! call it with NO guard of this service's own store held.
//!
//! It fails CLOSED. An engine the Organizations control plane is not mounted on,
//! and one whose store holds no organization, record no management account at all,
//! and a standing that cannot be established must not admit a reserved write.

use ::service_com_amazonaws_organizations_awsorganizationsv20161128::state as organizations;

/// The management account of the organization the presented account belongs to, or
/// `None` while that account is in no organization.
///
/// The organization record's `masterAccountId` — "the account whose user is calling
/// the `CreateOrganization` operation automatically becomes the management account
/// of the new organization" — is the one answer to that question, so every gate in
/// this service judges a caller against it.
pub fn management_account_id<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<String> {
    let organizations = organizations::service_state(engine)?;
    let organizations = organizations.lock();
    let organization = organizations.organization.as_ref()?;
    Some(organization.master_account_id.clone())
}

/// Whether the account this emulator presents — the caller of every request it
/// serves — is the management account of the organization it belongs to, the
/// standing an organization-level write is reserved to.
///
/// False while no organization is recorded, because an account that is in no
/// organization is the management account of none.
pub fn caller_is_management_account<Eng: Send + Sync + 'static>(engine: &Eng) -> bool {
    management_account_id(engine).as_deref() == Some(::engine::emulator_account_id())
}
