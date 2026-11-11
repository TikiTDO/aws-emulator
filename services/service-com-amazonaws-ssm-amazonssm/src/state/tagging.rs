// SPDX-License-Identifier: CC0-1.0

//! The tag plane: the one tag set each resource this store holds carries, and how a
//! request addresses it.
//!
//! Tagging is a single rule that spans resource types. A request names a resource by
//! `(ResourceType, ResourceId)` — "Use the ID of the resource", which for `Document` and
//! `Parameter` is "the name of the resource", for `ManagedInstance` is the `mi-ID_number`
//! node id (`AddTagsToResourceRequest$ResourceId`) and for an `OpsItem`, a
//! `CloudConnector`, an `Association` or a `MaintenanceWindow` is the id its create minted
//! — and that pair
//! resolves to the ONE record the Region holds for it. Each such record owns its tag
//! set, so a tag written through `AddTagsToResource` is the tag `ListTagsForResource`
//! reports and the tag `RemoveTagsFromResource` drops, and a resource's tags live with
//! the resource rather than in a second store beside it. A record is reached through
//! [`ComAmazonawsSsmAmazonssmState::resource_tags`] and
//! [`ComAmazonawsSsmAmazonssmState::resource_tags_mut`]; a pair that resolves to none is
//! what the tag operations refuse as an invalid resource id.
//!
//! "Adds or overwrites one or more tags" (`AddTagsToResource`): a key the resource
//! already carries takes the submitted value rather than appearing twice, which is what
//! makes the set a map from key to value ([`merge_tags`]).

use std::collections::BTreeMap;

use crate::ops::types::Tag;

use super::ComAmazonawsSsmAmazonssmState;

/// The `ResourceTypeForTagging` value whose id is a Parameter Store parameter's name
/// (from the modelled enum, `crate::ops::types::RESOURCE_TYPE_FOR_TAGGING_VALUES`).
pub const TAGGING_TYPE_PARAMETER: &str = "Parameter";

/// The `ResourceTypeForTagging` value whose id is an SSM document's name (same enum).
pub const TAGGING_TYPE_DOCUMENT: &str = "Document";

/// The `ResourceTypeForTagging` value whose id is a managed node's id (same enum).
pub const TAGGING_TYPE_MANAGED_INSTANCE: &str = "ManagedInstance";

/// The `ResourceTypeForTagging` value whose id is an OpsItem's own id (same enum).
pub const TAGGING_TYPE_OPS_ITEM: &str = "OpsItem";

/// The `ResourceTypeForTagging` value whose id is a cloud connector's minted id
/// (`super::cloud_connectors::CLOUD_CONNECTOR_RESOURCE_TYPE`, from the same enum).
pub const TAGGING_TYPE_CLOUD_CONNECTOR: &str = super::cloud_connectors::CLOUD_CONNECTOR_RESOURCE_TYPE;

/// The `ResourceTypeForTagging` value whose id is an automation execution's own id —
/// "Automation: example-c160-4567-8519-012345abcde" (`AddTagsToResourceRequest$ResourceId`),
/// which is the id `StartAutomationExecution` minted (same enum).
pub const TAGGING_TYPE_AUTOMATION: &str = "Automation";

/// The `ResourceTypeForTagging` value whose id is a State Manager association's minted id
/// (`super::associations::TAGGING_TYPE_ASSOCIATION`, from the same enum): "Adds or
/// overwrites one or more tags for a State Manager association"
/// (`CreateAssociationRequest$Tags`), so the tag set a create wrote is the one the tag
/// operations read and rewrite.
pub const TAGGING_TYPE_ASSOCIATION: &str = super::associations::TAGGING_TYPE_ASSOCIATION;

/// The `ResourceTypeForTagging` value whose id is a maintenance window's minted id
/// (`super::maintenance_windows::TAGGING_TYPE_MAINTENANCE_WINDOW`, from the same enum):
/// "MaintenanceWindow: mw-012345abcde" (`AddTagsToResourceRequest$ResourceId`), so the
/// tag set a create wrote is the one the tag operations read and rewrite.
pub const TAGGING_TYPE_MAINTENANCE_WINDOW: &str =
    super::maintenance_windows::TAGGING_TYPE_MAINTENANCE_WINDOW;

/// How many tags one resource may carry: "Most resources can have a maximum of 50 tags"
/// (`AddTagsToResource`'s own doc). The same doc states one exception — "Automations can
/// have a maximum of 5 tags" — which [`max_tags_for`] answers for.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many tags the resources of `resource_type` may carry.
///
/// "Most resources can have a maximum of 50 tags… Automations can have a maximum of 5
/// tags" (`AddTagsToResource`'s own doc), so the maximum is the resource type's rather
/// than one number for every type — and it is the SAME maximum `StartAutomationExecution`
/// holds the tags it carries to (`super::automation::MAX_TAGS_PER_AUTOMATION`), so the tag
/// plane and the start cannot disagree about how many tags an automation holds.
pub fn max_tags_for(resource_type: &str) -> usize {
    match resource_type {
        TAGGING_TYPE_AUTOMATION => super::automation::MAX_TAGS_PER_AUTOMATION,
        _ => MAX_TAGS_PER_RESOURCE,
    }
}

/// A tag set as the modelled `TagList` reports it, in key order — the order the store
/// holds it in, so two reads of one resource report its tags alike.
pub fn to_tag_list(held: &BTreeMap<String, String>) -> Vec<Tag> {
    held.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// Merge `adding` into `held` — "Adds or overwrites one or more tags", so a key already
/// held takes the submitted value and the set grows only by the keys it did not hold.
///
/// Answers `Err` with the number of tags the resource WOULD then carry when that is more
/// than `maximum` ([`max_tags_for`]), leaving `held` untouched: the merge is all or
/// nothing, so a refused request writes no part of its tags.
pub fn merge_tags(
    held: &mut BTreeMap<String, String>,
    adding: &[Tag],
    maximum: usize,
) -> Result<(), usize> {
    let mut merged = held.clone();
    for tag in adding {
        merged.insert(tag.key.clone(), tag.value.clone());
    }
    if merged.len() > maximum {
        return Err(merged.len());
    }
    *held = merged;
    Ok(())
}

/// Drop every key in `keys` from `held`, which is what a remove does to the resource's
/// tag set. A key the resource does not carry leaves the set as it was: the request
/// asks for those keys to be absent, and they are.
pub fn remove_tags(held: &mut BTreeMap<String, String>, keys: &[String]) {
    for key in keys {
        held.remove(key);
    }
}

impl ComAmazonawsSsmAmazonssmState {
    /// The tag set the resource `(resource_type, resource_id)` names in `region`
    /// carries, or `None` when the Region holds no such resource — which is what the tag
    /// operations refuse as an invalid resource id.
    ///
    /// A resource type this store holds no records for resolves to nothing for the same
    /// reason: nothing has created a resource of that type, so no id names one.
    ///
    /// Documents resolve through the records a caller CREATED here. The documents
    /// Systems Manager publishes are owned by Amazon rather than by the account reading
    /// them (`super::PUBLISHED_DOCUMENT_OWNER`), so the account carries no tag set on
    /// one and a published name addresses no resource of its own to tag.
    pub fn resource_tags(
        &self,
        region: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Option<&BTreeMap<String, String>> {
        match resource_type {
            TAGGING_TYPE_PARAMETER => self.parameter(region, resource_id).map(|held| &held.tags),
            TAGGING_TYPE_DOCUMENT => self.document(region, resource_id).map(|held| &held.tags),
            TAGGING_TYPE_MANAGED_INSTANCE => self
                .managed_node(region, resource_id)
                .map(|held| &held.tags),
            TAGGING_TYPE_OPS_ITEM => self.ops_item(region, resource_id).map(|held| &held.tags),
            TAGGING_TYPE_CLOUD_CONNECTOR => self
                .cloud_connector(region, resource_id)
                .map(|held| &held.tags),
            TAGGING_TYPE_AUTOMATION => self
                .automation_execution(region, resource_id)
                .map(|held| &held.tags),
            TAGGING_TYPE_ASSOCIATION => {
                self.association(region, resource_id).map(|held| &held.tags)
            }
            TAGGING_TYPE_MAINTENANCE_WINDOW => self
                .maintenance_window(region, resource_id)
                .map(|held| &held.tags),
            _ => None,
        }
    }

    /// The same tag set, to write it. See [`ComAmazonawsSsmAmazonssmState::resource_tags`]
    /// for how a request addresses a resource.
    pub fn resource_tags_mut(
        &mut self,
        region: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        match resource_type {
            TAGGING_TYPE_PARAMETER => self
                .parameter_mut(region, resource_id)
                .map(|held| &mut held.tags),
            TAGGING_TYPE_DOCUMENT => self
                .document_mut(region, resource_id)
                .map(|held| &mut held.tags),
            TAGGING_TYPE_MANAGED_INSTANCE => self
                .managed_node_mut(region, resource_id)
                .map(|held| &mut held.tags),
            TAGGING_TYPE_OPS_ITEM => self
                .ops_item_mut(region, resource_id)
                .map(|held| &mut held.tags),
            TAGGING_TYPE_CLOUD_CONNECTOR => self
                .cloud_connector_mut(region, resource_id)
                .map(|held| &mut held.tags),
            TAGGING_TYPE_AUTOMATION => self
                .automation_execution_mut(region, resource_id)
                .map(|held| &mut held.tags),
            TAGGING_TYPE_ASSOCIATION => self
                .association_mut(region, resource_id)
                .map(|held| &mut held.tags),
            TAGGING_TYPE_MAINTENANCE_WINDOW => self
                .maintenance_window_mut(region, resource_id)
                .map(|held| &mut held.tags),
            _ => None,
        }
    }
}
