// SPDX-License-Identifier: CC0-1.0

//! Which Athena resource a caller-supplied ARN names, and the tags that resource holds.
//!
//! Athena's tagging calls address a resource by ARN rather than by name, and `Tag` says
//! which resources those are: "Athena resources include workgroups, data catalogs, and
//! capacity reservations." Each of those already has a record on this service's store
//! carrying the tags its create call persisted ([`WorkGroupRecord::tags`],
//! [`DataCatalogRecord::tags`], [`CapacityReservationRecord::tags`]), so a tag write goes
//! into THAT list and a tag read answers out of it: one copy of a resource's tags, which
//! is what makes a tag a create carried one a listing reports and a tag a listing reports
//! one the resource really holds.
//!
//! The ARN is read through [`::service_common::arn`], the one place this tree parses a
//! caller-supplied ARN, so "not an ARN of this service at all" stays a bad REQUEST while
//! "an Athena resource this account does not hold" stays a miss — the distinction is
//! client-visible, the first being permanent and the second not.
//!
//! The ARN's Region and account fields are not part of the lookup: this emulator serves
//! one account through one endpoint, so the resource an Athena ARN names here is the one
//! its type and name name. What the ARN must carry is the vendor
//! ([`authorization::ACTION_PREFIX`], the namespace every Athena ARN carries) and one of
//! the resource types below.

use std::collections::HashSet;
use std::time::SystemTime;

use super::*;

/// The resource type an Athena WORKGROUP is addressed by in an ARN, from the "Resource
/// types defined by Amazon Athena" table of the IAM Service Authorization Reference:
/// `arn:${Partition}:athena:${Region}:${Account}:workgroup/${WorkGroupName}`.
pub const WORK_GROUP_RESOURCE_TYPE: &str = "workgroup";

/// The resource type an Athena DATA CATALOG is addressed by, from that same table:
/// `arn:${Partition}:athena:${Region}:${Account}:datacatalog/${DataCatalogName}`.
pub const DATA_CATALOG_RESOURCE_TYPE: &str = "datacatalog";

/// The resource type an Athena CAPACITY RESERVATION is addressed by, from that same
/// table:
/// `arn:${Partition}:athena:${Region}:${Account}:capacity-reservation/${CapacityReservationName}`.
pub const CAPACITY_RESERVATION_RESOURCE_TYPE: &str = "capacity-reservation";

/// The tag-key prefix Amazon Web Services keeps for the tags it generates itself.
///
/// Athena's tag restrictions state the reservation: "Do not use the `aws:` prefix in tag
/// keys or values; it is reserved for AWS use. You can't edit or delete tag keys or values
/// with this prefix." So such a key is one no caller may write and none may remove.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies inside the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves.
///
/// Matched without regard to ASCII case: what is reserved is the NAMESPACE, not one
/// spelling of it, so a key differing only in case is the same reserved key.
#[must_use]
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The refusal a request naming a tag key inside that namespace answers with.
///
/// Wholly FIXED text: it names neither the key the request carried nor the resource it
/// addressed, and states the rule — the tags inside that namespace are the system's own,
/// which no caller may modify.
#[must_use]
pub fn reserved_tag_key_message() -> String {
    format!(
        "System Tags (such as those with the prefix '{RESERVED_TAG_KEY_PREFIX}') are not \
         allowed to be modified."
    )
}

/// Why `keys` is not a list of tag keys a caller may remove, if it is not.
///
/// The one rule a `TagKeyList` carries beyond the lengths the model states: a key inside
/// the reserved namespace is not a caller's to delete (see [`RESERVED_TAG_KEY_PREFIX`]).
/// Stated once, beside the rules [`tag_list_error`] states for a `TagList`, so a removal
/// and a write refuse the same key with the same sentence.
#[must_use]
pub fn tag_keys_error(keys: &[String]) -> Option<String> {
    if keys.iter().any(|key| is_reserved_tag_key(key)) {
        return Some(reserved_tag_key_message());
    }
    keys.iter()
        .find(|key| !is_taggable_text(key))
        .map(|key| tag_text_message("Tag key", key))
}

/// The characters a tag key or value may carry BESIDE letters and numbers.
///
/// `Tag$Key` and `Tag$Value` both state the set: "You can use letters and numbers
/// representable in UTF-8, and the following characters: + - = . _ : / @." Letters and
/// numbers are read as Unicode alphanumerics, the documentation saying "representable in
/// UTF-8" rather than naming an ASCII range.
pub const TAG_TEXT_PUNCTUATION: [char; 8] = ['+', '-', '=', '.', '_', ':', '/', '@'];

/// Whether `text` is a tag key or value composed only of what [`TAG_TEXT_PUNCTUATION`]
/// quotes: letters, numbers and those eight characters.
#[must_use]
pub fn is_taggable_text(text: &str) -> bool {
    text.chars()
        .all(|character| character.is_alphanumeric() || TAG_TEXT_PUNCTUATION.contains(&character))
}

/// The refusal a key or value carrying a character outside that set answers with, where
/// `member` names which of the two it was.
#[must_use]
pub fn tag_text_message(member: &str, text: &str) -> String {
    format!(
        "{member} {text} carries a character tags cannot use; a tag key or value holds \
         letters, numbers and the characters {}.",
        TAG_TEXT_PUNCTUATION.iter().collect::<String>()
    )
}

/// The resource one of Athena's tagging calls addresses, and the name it addressed it by.
///
/// Borrowed from the ARN that was read, so resolving one allocates nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource<'arn> {
    /// A workgroup, by name — the key [`ComAmazonawsAthenaAmazonathenaState::work_groups`]
    /// holds it under.
    WorkGroup(&'arn str),
    /// A data catalog, by name — the key
    /// [`ComAmazonawsAthenaAmazonathenaState::data_catalogs`] holds it under.
    DataCatalog(&'arn str),
    /// A capacity reservation, by name — the key the
    /// [`CapacityReservationRegistry`] holds it under.
    CapacityReservation(&'arn str),
}

/// What a tagging call's `ResourceARN` addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressedResource<'arn> {
    /// A resource of a type Athena holds tags on, in this account and the Region the call
    /// was addressed to. Whether this account HOLDS it is the next question, answered by
    /// [`ComAmazonawsAthenaAmazonathenaState::tags_of`].
    Taggable(TaggedResource<'arn>),
    /// An ARN naming ANOTHER account's resource, which is a bad request rather than a
    /// resource that happens to be missing (see [`ARN_ACCOUNT_MISMATCH`]).
    AnotherAccount,
    /// Anything else: a value that is not an Athena resource ARN, one naming another
    /// Region's resource, one of a type Athena holds no tags on, or one carrying no
    /// resource name. Nothing a resource resolves from (see [`could_not_find_resource`]).
    Unresolvable,
}

/// What the ARN `arn` addresses, where `region` is the Region the CALL was made in (see
/// [`authorization::region_from_request`]).
///
/// A tagging call reaches the resources of the account it was signed for, in the Region it
/// was addressed to: an ARN naming another account's resource is refused ON THE REQUEST,
/// which is why that refusal says nothing about whether the named resource exists, while an
/// ARN naming another Region's resource, a type Athena holds no tags on, or nothing at all
/// is simply an ARN no resource of this endpoint resolves from.
///
/// The partition is not read: one endpoint serves one partition, and the account and Region
/// above already say whether the ARN addresses what this endpoint holds. An ARN with an
/// EMPTY account field names no account to mismatch — AWS writes an account-less resource
/// that way — so it falls to [`AddressedResource::Unresolvable`] like any other ARN this
/// service resolves nothing from.
pub fn addressed_resource<'arn>(arn: &'arn str, region: &str) -> AddressedResource<'arn> {
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        return AddressedResource::Unresolvable;
    };
    if !parsed.account.is_empty() && parsed.account != ::engine::emulator_account_id() {
        return AddressedResource::AnotherAccount;
    }
    if parsed.vendor != authorization::ACTION_PREFIX || parsed.region != region {
        return AddressedResource::Unresolvable;
    }
    let name = ::service_common::arn::resource_id_of_type(arn, parsed.resource_type())
        .unwrap_or_default();
    if name.is_empty() {
        return AddressedResource::Unresolvable;
    }
    match parsed.resource_type() {
        WORK_GROUP_RESOURCE_TYPE => AddressedResource::Taggable(TaggedResource::WorkGroup(name)),
        DATA_CATALOG_RESOURCE_TYPE => {
            AddressedResource::Taggable(TaggedResource::DataCatalog(name))
        }
        CAPACITY_RESERVATION_RESOURCE_TYPE => {
            AddressedResource::Taggable(TaggedResource::CapacityReservation(name))
        }
        _ => AddressedResource::Unresolvable,
    }
}

/// The refusal a `ResourceARN` naming another account's resource answers with.
///
/// Wholly FIXED text: it quotes neither the ARN the request carried nor the account either
/// side of the comparison, so it discloses nothing about the caller's account or about
/// whether the named resource exists.
pub const ARN_ACCOUNT_MISMATCH: &str = "Resource ARN account does not match caller account";

/// The message a tagging call answers an ARN it could resolve no held resource from with.
///
/// ONE sentence for the whole class, naming the ARN the request carried and nothing of the
/// resource it would have named: a value that is not an Athena resource ARN, one naming a
/// type Athena holds no tags on, one naming another Region's resource and one naming a
/// resource of a taggable type this account does not hold are all answered alike, because
/// what the service reports is that it could not resolve a resource from the ARN it was
/// given.
#[must_use]
pub fn could_not_find_resource(arn: &str) -> String {
    format!("Could not find valid resource from ARN: {arn}")
}

/// Why writing `tags` onto a resource already holding `held` is not a change one Athena
/// resource's tags can take, if it is not.
///
/// The documented cap is on what the RESOURCE holds — "You can have up to 50 tags per
/// resource" ([`MAX_TAGS_PER_RESOURCE`]) — so it is the resulting set that is measured: the
/// keys the resource already carries that this write only revalues take no further room,
/// and the keys it does not carry each take one.
#[must_use]
pub fn written_tags_error(held: &[Tag], tags: &[Tag]) -> Option<String> {
    let key_of = |tag: &Tag| tag.key.clone().unwrap_or_default();
    let mut keys: HashSet<String> = held.iter().map(key_of).collect();
    keys.extend(tags.iter().map(key_of));
    (keys.len() > MAX_TAGS_PER_RESOURCE).then(|| {
        format!(
            "Tags would take this resource to {} tags, more than the \
             {MAX_TAGS_PER_RESOURCE} tags one resource can hold.",
            keys.len()
        )
    })
}

/// Write `tags` onto the list `held`, as a tag write does.
///
/// `Tag$Key` documents "Tag keys ... must be unique per resource", so a key the resource
/// already carries takes the submitted value in place rather than appearing twice; a key
/// it does not carry is appended, in the order the request listed it.
pub fn write_tags(held: &mut Vec<Tag>, tags: Vec<Tag>) {
    for tag in tags {
        let key = tag.key.clone().unwrap_or_default();
        match held
            .iter_mut()
            .find(|existing| existing.key.as_deref().unwrap_or_default() == key)
        {
            Some(existing) => *existing = tag,
            None => held.push(tag),
        }
    }
}

/// Remove from `held` every tag whose key is one of `keys`, as a tag removal does.
///
/// A key the resource does not carry removes nothing: the request asks for those keys not
/// to be held, which is already so.
pub fn remove_tag_keys(held: &mut Vec<Tag>, keys: &[String]) {
    held.retain(|tag| {
        let key = tag.key.as_deref().unwrap_or_default();
        !keys.iter().any(|removed| removed == key)
    });
}

/// The cursor a `ListTagsForResource` page hands back: the resource it is paging — by the
/// ARN the request addressed — and the position to resume at.
fn tags_cursor(arn: &str, position: usize) -> String {
    format!("tags{CURSOR_SEPARATOR}{arn}{CURSOR_SEPARATOR}{position}")
}

impl ComAmazonawsAthenaAmazonathenaState {
    /// The tags the resource `resource` holds at `now`, or `None` when this account holds
    /// no such resource.
    ///
    /// A READ: nothing here writes the store, so what the next caller sees is what it saw
    /// before. The account's default workgroup ([`DEFAULT_WORK_GROUP`]) exists without a
    /// record of its own, and holds whatever that record would carry — which is no tags
    /// until a write materialises it.
    pub fn tags_of(&self, resource: &TaggedResource<'_>, now: SystemTime) -> Option<Vec<Tag>> {
        match *resource {
            TaggedResource::WorkGroup(name) => match self.work_groups.get(name) {
                Some(record) => Some(record.tags.clone()),
                None => (name == DEFAULT_WORK_GROUP).then(Vec::new),
            },
            TaggedResource::DataCatalog(name) => {
                self.data_catalogs.get(name).map(|record| record.tags.clone())
            }
            TaggedResource::CapacityReservation(name) => self
                .capacity_reservations
                .reservation(name, now)
                .map(|record| record.tags.clone()),
        }
    }

    /// The same tags, for a caller that is going to CHANGE them (a tag write or a tag
    /// removal). `None` for a resource this account does not hold, which such a call
    /// refuses rather than creating.
    pub fn tags_of_mut(
        &mut self,
        resource: &TaggedResource<'_>,
        now: SystemTime,
    ) -> Option<&mut Vec<Tag>> {
        match *resource {
            TaggedResource::WorkGroup(name) => self
                .work_group_record_mut(name)
                .map(|record| &mut record.tags),
            TaggedResource::DataCatalog(name) => self
                .data_catalogs
                .get_mut(name)
                .map(|record| &mut record.tags),
            TaggedResource::CapacityReservation(name) => self
                .capacity_reservations
                .reservation_mut(name, now)
                .map(|record| &mut record.tags),
        }
    }

    /// The record of the workgroup `name`, for a call that is going to change it.
    ///
    /// The account's default workgroup has no record until something writes one (see
    /// [`default_work_group_record`]): a change to it materialises that record, dated from
    /// the instant this account's registries were opened, so the workgroup a listing
    /// already reports is the one the change is held on and no second copy of it appears.
    fn work_group_record_mut(&mut self, name: &str) -> Option<&mut WorkGroupRecord> {
        if name == DEFAULT_WORK_GROUP && !self.work_groups.contains_key(name) {
            let record = default_work_group_record(self.opened_at);
            self.work_groups.insert(name.to_string(), record);
        }
        self.work_groups.get_mut(name)
    }

    /// The `NextToken` a `ListTagsForResource` page of the resource `arn` resumed at
    /// `position` hands back.
    pub fn tags_page_token(&self, arn: &str, position: usize) -> String {
        ::hex::encode(self.page_token_key.mint(&tags_cursor(arn, position)))
    }

    /// The position `token` resumes a tag listing of `arn` at, or `None` for a token this
    /// store did not mint for THAT resource.
    pub fn tags_page_position(&self, arn: &str, token: &str) -> Option<usize> {
        let cursor = self.page_cursor(token)?;
        cursor_position(&cursor, "tags", arn)
    }
}
