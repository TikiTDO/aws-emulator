// SPDX-License-Identifier: CC0-1.0

//! Every tag this store holds, as the modelled `TagDescription` the tag report answers with,
//! and the position one of those descriptions is paged from.
//!
//! `DescribeTags` reports the tags of the account's resources without being told which kind
//! of resource holds any of them, so it reads them off the SAME list of stores
//! `CreateTags` and `DeleteTags` resolve an id against
//! ([`ComAmazonawsEc2Amazonec2State::resource_tags_mut`]): a tag written through one call is
//! reported by this one, and a store the crate gains becomes taggable and reportable in the
//! same edit.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

use crate::ops::types::TagDescription;

/// What separates the resource id from the tag key in the position
/// [`tag_description_cursor`] names.
///
/// A byte no resource id this service MINTS carries, which is what makes the position
/// unambiguous: the separator is the first occurrence of it, so the id ahead of it and the key
/// behind it are recovered exactly even for a tag key that carries one — two different
/// `(resource id, tag key)` pairs cannot share a position. The ORDER of the positions is
/// whatever this one function yields, and the report's sort and its cursor both read it from
/// here, so neither can disagree with the other about which description follows which.
const TAG_POSITION_SEPARATOR: char = '\0';

/// The position a `TagDescription` is paged from: its resource id and its tag key.
///
/// One resource carries many tags and a report is a list of TAGS, so neither half alone
/// names a position in it; together they are unique ("Tag keys must be unique per resource",
/// `CreateTags`), which is what lets a cursor always advance.
pub fn tag_description_cursor(described: &TagDescription) -> String {
    format!(
        "{}{TAG_POSITION_SEPARATOR}{}",
        described.resource_id.as_deref().unwrap_or_default(),
        described.key.as_deref().unwrap_or_default(),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Every tag this store holds, each with the resource that carries it and the
    /// `ResourceType` that resource is reported under, in [`tag_description_cursor`] order.
    ///
    /// The stores walked are the ones [`taggable_tag_list_stores`] names, plus the two whose
    /// records keep their tags in a shape of their own — an instance's tag MAP and a network
    /// interface's `TagSet` — which is the same set
    /// [`ComAmazonawsEc2Amazonec2State::resource_tags_mut`] resolves an id against. A tag
    /// list entry carrying no key names no tag, so it is not reported; an entry with no value
    /// reads as the empty value, which is the value a tag given without one is stored with.
    pub fn tag_descriptions(&self) -> Vec<TagDescription> {
        let mut described: Vec<TagDescription> = Vec::new();
        {
            let mut report = |resource_id: &str, resource_type: &str, tags: &[Tag]| {
                for tag in tags {
                    let Some(key) = tag.key.as_deref() else {
                        continue;
                    };
                    described.push(TagDescription {
                        key: Some(key.to_string()),
                        resource_id: Some(resource_id.to_string()),
                        resource_type: Some(resource_type.to_string()),
                        value: Some(tag.value.clone().unwrap_or_default()),
                    });
                }
            };

            let store = self;
            // The instance record keeps its tags as a key -> value map rather than as the
            // modelled list, so it is read on its own and projected onto that list here.
            for (resource_id, instance) in &store.instances {
                let tags: Vec<Tag> = instance
                    .tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some(key.clone()),
                        value: Some(value.clone()),
                    })
                    .collect();
                report(resource_id, RESOURCE_TYPE_INSTANCE, &tags);
            }
            // A network interface's modelled tag member is `TagSet`
            // (`NetworkInterface$TagSet`) rather than `Tags`, so it is read on its own too —
            // the tags are the same modelled list either way.
            for (resource_id, interface) in &store.network_interfaces {
                report(
                    resource_id,
                    RESOURCE_TYPE_NETWORK_INTERFACE,
                    interface.tag_set.as_deref().unwrap_or_default(),
                );
            }
            taggable_tag_list_stores!(resource_tag_descriptions[store, report]);
        }

        // The report's own stable order, which the page cursor names a position in. "The
        // order of the elements in the response … might vary" (`DescribeTags`), so no caller
        // may depend on it; a cursor must, and a total order is what makes one advance.
        described.sort_by_key(tag_description_cursor);
        described
    }
}
