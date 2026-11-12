// SPDX-License-Identifier: CC0-1.0

//! What the operations of ONE managed prefix list share beyond the store itself: how the
//! entries a request names are read, how an id no prefix list the caller can see carries is
//! refused, the state words a revision moves the list through, and where the entry set of a
//! version a revision supersedes is kept.
//!
//! Re-exported from this operation, so a handler reaches these as
//! `crate::ops::modify_managed_prefix_list::managed_prefix_list_entries::<item>`.
//!
//! ## Reading the entries a request names
//!
//! `CreateManagedPrefixList` names the entries a list starts with and
//! `ModifyManagedPrefixList` names the ones it adds and removes, and all three describe one
//! entry the same way — "a CIDR block and an optional description" (`AddPrefixListEntry`) —
//! so [`read_entry_block`] is the ONE reader of such a block and [`read_entries`] the ONE
//! reader of a list of them. A block is read against the list's own address family and stored
//! in canonical form, so one range has one spelling however a request names it.
//!
//! ## The entry set of a superseded version
//!
//! "Adding or removing entries in a prefix list creates a new version of the prefix list"
//! (`ModifyManagedPrefixList`), and "the version of the prefix list for which to return the
//! entries" (`GetManagedPrefixListEntriesRequest$TargetVersion`) is a version a caller may
//! still ask about once a later one has replaced it — so the entry set a revision REPLACES is
//! kept here, under the version it was the entry set of, rather than dropped. The CURRENT
//! version's entries are not here: they live on the list's own record
//! ([`crate::state::ManagedPrefixListRecord::entries`]), which is what every report of the
//! list reads, so one version's entries are held in exactly one place.
//!
//! ## Where it is stored
//!
//! In the shared engine's own typed state slot, reached through
//! [`require_prefix_list_versions`] from the engine every operation of this service is
//! constructed with — never from an engine a handler builds, which would drop what a request
//! wrote. It is a slot of its own rather than another field of
//! [`crate::state::ComAmazonawsEc2Amazonec2State`] because `src/state.rs`, the module that
//! declares that struct, is far past the per-file length ceiling a change to this crate is
//! held to and so cannot take another field.
//!
//! That slot has exactly the SCOPE of the records whose superseded versions it holds, neither
//! wider nor narrower: the service's own store is reached the same way
//! ([`crate::state::service_state`] resolves `InMemoryEngine::state::<…Ec2State>()`), so both
//! are one per engine, and this emulator serves one account
//! (`engine::emulator_account_id()`) through one engine per process. The key is the
//! `PrefixListId` the main store minted with its own monotonic counter
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::next_id`]) beside the version it was that
//! list's entry set at, so one key names one list's one version, and an id the caller sends
//! reaches this store only after that same store has resolved it to a list the caller can see
//! — an unresolvable id is refused ([`prefix_list_not_found`]) before anything is read here.
//!
//! The slot is never locked while the service's main store is: each handler finishes with one
//! before it takes the other, so the two have no lock order to invert.
//!
//! The not-found refusal below replaces what this crate's prefix list surface used to answer,
//! so `crate::state::invalid_prefix_list_id_not_found` has no caller left; it stays where it is
//! until `src/state.rs` is split under the per-file ceiling and can be edited again.

use std::collections::BTreeMap;

use crate::ops::types::{AddPrefixListEntry, PrefixListEntry};
use crate::scaffolded::Scaffolded;
use crate::state::{
    INVALID_PREFIX_LIST_ID_NOT_FOUND, MAX_PREFIX_LIST_ENTRY_DESCRIPTION_CHARS,
    MAX_STORED_RECORDS_PER_STORE, PREFIX_LIST_ADDRESS_FAMILY_IPV6,
    PREFIX_LIST_STATE_CREATE_COMPLETE, canonical_cidr_block, invalid_parameter_value,
    managed_prefix_list_payload_bytes,
};

/// Refuse a request that named an id no prefix list the caller can see carries: HTTP 400
/// [`INVALID_PREFIX_LIST_ID_NOT_FOUND`], worded the way EC2 words THIS class — `Prefix List
/// <id> not found.`
///
/// The wording of that refusal every operation that REPORTS or REVISES a prefix list answers
/// with, so two reports of one unresolvable id cannot say different things. It is a wording of
/// its own rather than the generic `The <kind> ID '<id>' does not exist`
/// ([`crate::state::id_not_found`]) because EC2 words a prefix list's not-found that way and
/// not the generic way — the same reason the Client VPN, IPAM and transit gateway surfaces of
/// this crate each keep their own.
///
/// `DeleteManagedPrefixList` is NOT one of its callers: EC2 words the same code differently for
/// that operation (`The prefix list (<id>) does not exist.`), which is the recorded contract of
/// the delete, so that wording lives with the handler that answers it.
pub fn prefix_list_not_found<E>(prefix_list_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_PREFIX_LIST_ID_NOT_FOUND,
        400,
        format!("Prefix List {prefix_list_id} not found."),
    )
}

/// The state a prefix list is in while the revision a `ModifyManagedPrefixList` started is
/// still owed, spelled as the modelled catalog spells it
/// (`crate::ops::types::PREFIX_LIST_STATE_VALUES`).
pub const PREFIX_LIST_STATE_MODIFY_IN_PROGRESS: &str = "modify-in-progress";

/// The state that revision reaches, from the same catalog: the clock owes a
/// [`PREFIX_LIST_STATE_MODIFY_IN_PROGRESS`] list this one [`crate::state::TRANSITION_DELAY`]
/// after the call that started it, whether or not anybody calls again.
pub const PREFIX_LIST_STATE_MODIFY_COMPLETE: &str = "modify-complete";

/// The states a prefix list of this emulator may be MODIFIED or DELETED in: the settled
/// states, the ones no clock-owed move is still due from.
///
/// A list whose creation or whose last revision is still in progress is not one a further
/// revision has a defined entry set to start from, so it is refused with the state it is in
/// rather than revised. The catalog holds the two settled states this emulator can leave a
/// list in — the one `CreateManagedPrefixList` settles into and the one a revision settles
/// into; the `restore-*` states of the modelled catalog land with
/// `RestoreManagedPrefixListVersion`, which is still a scaffold stub.
pub const PREFIX_LIST_STATES_ADMITTING_A_REVISION: [&str; 2] = [
    PREFIX_LIST_STATE_CREATE_COMPLETE,
    PREFIX_LIST_STATE_MODIFY_COMPLETE,
];

/// One CIDR block a request named for a prefix list of address family `family`, in the
/// canonical form the list stores it in — or the refusal it earns.
///
/// "The CIDR block" (`AddPrefixListEntry$Cidr`, `RemovePrefixListEntry$Cidr`) is read against
/// the list's own family ("The IP address version", `ManagedPrefixList$AddressFamily`): a
/// block of the other family, or a value that is no CIDR block at all, is refused rather than
/// read, an entry of the wrong family having no address the list could ever be consulted for.
/// The canonical form has the block's host bits clear, the way every other caller-supplied
/// range of this service is stored ([`canonical_cidr_block`]), so two spellings of one range
/// name one entry.
pub fn read_entry_block<E>(cidr: &str, family: &str) -> Result<String, Scaffolded<E>> {
    let wants_ipv6 = family == PREFIX_LIST_ADDRESS_FAMILY_IPV6;
    match canonical_cidr_block(cidr) {
        Some(canonical) if cidr.contains(':') == wants_ipv6 => Ok(canonical),
        _ => Err(invalid_parameter_value(format!(
            "Value ({cidr}) for parameter cidr is invalid. \
             This is not a valid {family} CIDR block.",
        ))),
    }
}

/// The entries a request named as the stored, reported [`PrefixListEntry`] shape, or the
/// refusal one of them earns.
///
/// Each entry is "a CIDR block and an optional description" (`AddPrefixListEntry`): the block
/// is read by [`read_entry_block`], and the description is held to the length the model
/// documents for it ("Constraints: Up to 255 characters in length",
/// `AddPrefixListEntry$Description`). One range is one entry, so a second entry for a range
/// the same request already named has no defined winner and is refused rather than read
/// twice. `family` is one of [`crate::state::PREFIX_LIST_ADDRESS_FAMILIES`].
pub fn read_entries<E>(
    entries: &[AddPrefixListEntry],
    family: &str,
) -> Result<Vec<PrefixListEntry>, Scaffolded<E>> {
    let mut read: Vec<PrefixListEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let canonical = read_entry_block(&entry.cidr, family)?;
        if entry.description.as_deref().is_some_and(|description| {
            description.chars().count() > MAX_PREFIX_LIST_ENTRY_DESCRIPTION_CHARS
        }) {
            return Err(invalid_parameter_value(format!(
                "The description of entry {cidr} accepts at most {max} characters",
                cidr = entry.cidr,
                max = MAX_PREFIX_LIST_ENTRY_DESCRIPTION_CHARS,
            )));
        }
        if read
            .iter()
            .any(|stored| stored.cidr.as_deref() == Some(canonical.as_str()))
        {
            return Err(invalid_parameter_value(format!(
                "Value ({cidr}) for parameter cidr is invalid. \
                 The prefix list cannot carry one CIDR block twice.",
                cidr = entry.cidr,
            )));
        }
        read.push(PrefixListEntry {
            cidr: Some(canonical),
            description: entry.description.clone(),
        });
    }
    Ok(read)
}

/// The entry sets of the prefix list versions a revision has SUPERSEDED, keyed by the list the
/// version belongs to and the version number it was.
///
/// A [`BTreeMap`] for the reason every other store of this crate is one: one stable order over
/// the pairs it holds, so what a reader sees does not depend on insertion order.
#[derive(Debug, Default)]
pub struct PrefixListVersions {
    entry_sets: BTreeMap<(String, i64), Vec<PrefixListEntry>>,
}

impl PrefixListVersions {
    /// Whether another superseded entry set fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`], the ceiling every other store of this crate holds).
    ///
    /// A revision asks this BEFORE it writes, so it can refuse with the quota code instead of
    /// overshooting: nothing reclaims a superseded version, and a caller may revise one list
    /// as often as it likes. The caller-chosen bytes those entries hold are charged against
    /// the main store's byte budget by the revision that retains them, so this is a ceiling on
    /// the COUNT alone.
    pub fn has_capacity(&self) -> bool {
        self.entry_sets.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Keep `entries` as the entry set version `version` of the prefix list `prefix_list_id`
    /// had.
    ///
    /// The ONE writer of a superseded entry set. Called with the entry set a revision just
    /// replaced, so the version a report may still ask about keeps its own entries rather than
    /// being answered with the ones that replaced them. `prefix_list_id` is an id the main
    /// store MINTED and the revision has just resolved against it under its lock, not a value
    /// read off the request: a request naming an id that store holds no list for is refused
    /// before anything reaches here.
    pub fn record(&mut self, prefix_list_id: &str, version: i64, entries: Vec<PrefixListEntry>) {
        self.entry_sets
            .insert((prefix_list_id.to_string(), version), entries);
    }

    /// Forget every superseded entry set of the prefix list `prefix_list_id`, and answer the
    /// caller-supplied bytes they held.
    ///
    /// Called by the delete of a prefix list: the versions a revision superseded belong to that
    /// list, so a list that is on its way out of the main store takes them with it rather than
    /// leaving entry sets nothing can ever be asked for. The bytes come back out because the
    /// revision that retained them charged them against the MAIN store's budget, which is where
    /// the caller of this gives them back — this store's own ceiling is on the count alone
    /// ([`Self::has_capacity`]).
    ///
    /// The entries of the version the list STANDS AT are not here — they live on the list's own
    /// record — so a report of the current version still answers while the removal is owed.
    pub fn purge(&mut self, prefix_list_id: &str) -> usize {
        let mut released = 0usize;
        self.entry_sets.retain(|(id, _), entries| {
            if id != prefix_list_id {
                return true;
            }
            released = released
                .saturating_add(managed_prefix_list_payload_bytes("", entries, &[], None));
            false
        });
        released
    }

    /// The entry set version `version` of the prefix list `prefix_list_id` had, or `None` when
    /// this store holds none for that pair — either because that version is the one the list
    /// stands at, whose entries live on the list's own record, or because the list never had
    /// it.
    ///
    /// Asked only for a list the main store has already resolved for the reader, the same way
    /// [`Self::record`] is: the pair reaches this store after the list it names has been found,
    /// so an id nothing holds a list for is refused rather than looked up here.
    pub fn entries_of(&self, prefix_list_id: &str, version: i64) -> Option<&Vec<PrefixListEntry>> {
        self.entry_sets.get(&(prefix_list_id.to_string(), version))
    }
}

/// This store's slot in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answers the server error every handler answers a service mounted on an engine that holds no
/// state with — never a fresh engine, whose writes nothing else would ever see. The same
/// resolution [`crate::state::service_state`] performs for this service's main store, against
/// the same engine, so the two stores have one scope between them.
pub fn require_prefix_list_versions<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
) -> Result<::engine::ServiceState<PrefixListVersions>, Scaffolded<E>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<PrefixListVersions>)
        .ok_or_else(|| {
            Scaffolded::internal_error("the EC2 emulator is mounted on an engine that holds no state")
        })
}
