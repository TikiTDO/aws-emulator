// SPDX-License-Identifier: CC0-1.0

//! The resource a tag operation's `ResourceArn` names, and the tag map every tag
//! operation reads and writes on it.
//!
//! Kept beside [`super`] rather than in one operation's file because `TagResource`,
//! `UntagResource` and `ListTagsForResource` resolve an ARN the same way and all reach
//! the SAME per-resource tag map the creates fill and the describes report: there is one
//! tag map per resource, not a second tag store beside it, so a tag written through the
//! tag plane is what that resource's own read reports.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::{
    ComAmazonawsLocationLocationserviceState, GEOFENCE_COLLECTION_RESOURCE, MAP_RESOURCE,
    ROUTE_CALCULATOR_RESOURCE, TRACKER_RESOURCE,
};

/// How a refusal names a resource whose ARN carries a `resourcetype` this service
/// holds no resources of: there is no resource kind to name, so the sentence names the
/// ARN the request sent instead.
pub const ANY_RESOURCE: &str = "Resource";

/// The member a tag write's refusal names, as the model spells it.
pub const TAGS_MEMBER: &str = "Tags";

/// Tags one resource holds at once.
///
/// The `TagResource` doc: "You can associate up to 50 tags with a resource." The
/// modelled `TagMap` is `len ..50`, so no single request can carry more than this, but a
/// merge into a resource that already carries tags can — which is what this bound is
/// checked against, and what keeps a tag map a caller grows one key per request from
/// growing without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The tag-key prefix this service reserves.
///
/// The `Tags` member doc of `TagResource` and of every create that takes tags: "Cannot
/// use "aws:" as a prefix for a key." The modelled `TagKey` pattern admits `:`, so the
/// request boundary accepts such a key and the reservation is this service's own check.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// One resource kind an Amazon Location ARN can name, for the kinds this service holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedKind {
    Tracker,
    GeofenceCollection,
    Map,
    RouteCalculator,
}

impl TaggedKind {
    /// Every kind an ARN resolves to, so a caller can walk the resolvable set.
    pub const ALL: [Self; 4] = [
        Self::Tracker,
        Self::GeofenceCollection,
        Self::Map,
        Self::RouteCalculator,
    ];

    /// The `resourcetype` segment an ARN of this kind carries, spelled as the
    /// resource's own `*Arn` member doc spells it
    /// (`arn:aws:geo:region:account-id:tracker/ExampleTracker`) and as
    /// [`super::tracker_arn`] and its siblings mint it.
    pub const fn arn_resource_type(self) -> &'static str {
        match self {
            Self::Tracker => "tracker",
            Self::GeofenceCollection => "geofence-collection",
            Self::Map => "map",
            Self::RouteCalculator => "route-calculator",
        }
    }

    /// How a refusal about this kind names it — the same label every other refusal
    /// about that resource uses.
    pub const fn resource_label(self) -> &'static str {
        match self {
            Self::Tracker => TRACKER_RESOURCE,
            Self::GeofenceCollection => GEOFENCE_COLLECTION_RESOURCE,
            Self::Map => MAP_RESOURCE,
            Self::RouteCalculator => ROUTE_CALCULATOR_RESOURCE,
        }
    }

    /// The kind an ARN's `resourcetype` segment names, or `None` when this service
    /// holds no resources of that type.
    fn of_arn_resource_type(resource_type: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.arn_resource_type() == resource_type)
    }
}

/// The member a refusal about the addressed resource names, as the model spells it.
pub const RESOURCE_ARN_MEMBER: &str = "ResourceArn";

/// This service's ARN vendor field: the `arnNamespace` its model publishes, and the
/// namespace every ARN this service mints carries.
pub const ARN_VENDOR: &str = "geo";

/// The sentence a value that is not a resource ARN of this service is refused with.
///
/// The vendor field of an ARN is what says whose API can address the resource at all, so
/// an ARN of another vendor — or a value that is no ARN — names nothing in this
/// service's namespace and is a bad REQUEST, refused on the input before any lookup,
/// rather than a resource that happens to be missing. The sentence follows the form this
/// service states a member requirement in: the member name as the model spells it,
/// quoted, then what the member has to satisfy.
pub fn not_an_arn_of_this_service() -> String {
    format!("'{RESOURCE_ARN_MEMBER}' must be the ARN of an Amazon Location resource.")
}

/// Why a tag operation's `ResourceArn` resolved to no resource of this service.
///
/// The two cases are different refusals, and the difference is client-visible: a
/// validation refusal is permanent and caller-side, a not-found is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnresolvedArn {
    /// The value is not a resource ARN of this service's vendor: a 400
    /// `ValidationException` naming `ResourceArn`, carrying this sentence.
    NotAnArnOfThisService(String),
    /// A resource ARN of this service's vendor naming a resource this service does not
    /// hold: a 404 `ResourceNotFoundException` carrying this sentence.
    NotFound(String),
}

/// The resource a tag operation addresses, resolved from its `ResourceArn` once so the
/// read path and the write path agree on what the ARN names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagTarget {
    kind: TaggedKind,
    name: String,
}

impl TagTarget {
    /// What `arn` names, or why it names no resource of this service.
    ///
    /// The six ARN fields come from the workspace's one caller-supplied-ARN parse
    /// (`service_common::arn`), read against this service's vendor, so a value that is
    /// not an ARN and one of another vendor are both refused on the input. The region and
    /// account fields take no part in the lookup: this service stores a resource under
    /// its name alone, which is also what every name-addressed read in the crate
    /// resolves. A resource type of this vendor that the store holds no resources of —
    /// one whose create is still a scaffolded operation — resolves to nothing and is the
    /// not-found case.
    pub fn of_arn(arn: &str) -> Result<Self, UnresolvedArn> {
        let parsed = ::service_common::arn::parse_of(arn, ARN_VENDOR)
            .ok_or_else(|| UnresolvedArn::NotAnArnOfThisService(not_an_arn_of_this_service()))?;
        let kind = TaggedKind::of_arn_resource_type(parsed.resource_type())
            .ok_or_else(|| UnresolvedArn::NotFound(unresolvable_arn_not_found(arn)))?;
        let name = ::service_common::arn::resource_id_of_type(arn, kind.arn_resource_type())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| UnresolvedArn::NotFound(unresolvable_arn_not_found(arn)))?;
        Ok(Self {
            kind,
            name: name.to_string(),
        })
    }

    /// The resource kind the ARN named.
    pub fn kind(&self) -> TaggedKind {
        self.kind
    }

    /// The resource name the ARN carried.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The `ResourceNotFoundException` sentence stated when this service holds no
    /// resource of that kind under that name.
    pub fn not_found(&self) -> String {
        super::resource_not_found(self.kind.resource_label(), &self.name)
    }
}

/// The `ResourceNotFoundException` sentence stated for an ARN that names no resource
/// kind this service holds: the ARN is all the request said about the resource, so the
/// sentence names the ARN.
pub fn unresolvable_arn_not_found(arn: &str) -> String {
    super::resource_not_found(ANY_RESOURCE, arn)
}

impl ComAmazonawsLocationLocationserviceState {
    /// The stored tag map of the resource `target` names, or `None` when this service
    /// holds no such resource.
    pub fn resource_tags(&self, target: &TagTarget) -> Option<&BTreeMap<String, String>> {
        match target.kind {
            TaggedKind::Tracker => self.trackers.get(&target.name).map(|held| &held.tags),
            TaggedKind::GeofenceCollection => self
                .geofence_collections
                .get(&target.name)
                .map(|held| &held.tags),
            TaggedKind::Map => self.maps.get(&target.name).map(|held| &held.tags),
            TaggedKind::RouteCalculator => self
                .route_calculators
                .get(&target.name)
                .map(|held| &held.tags),
        }
    }

    /// The same map for writing — the one map that resource's own read reports, so a
    /// tag written here is a tag its describe reports.
    pub fn resource_tags_mut(
        &mut self,
        target: &TagTarget,
    ) -> Option<&mut BTreeMap<String, String>> {
        match target.kind {
            TaggedKind::Tracker => self.trackers.get_mut(&target.name).map(|held| &mut held.tags),
            TaggedKind::GeofenceCollection => self
                .geofence_collections
                .get_mut(&target.name)
                .map(|held| &mut held.tags),
            TaggedKind::Map => self.maps.get_mut(&target.name).map(|held| &mut held.tags),
            TaggedKind::RouteCalculator => self
                .route_calculators
                .get_mut(&target.name)
                .map(|held| &mut held.tags),
        }
    }
}

/// `Ok` when every key is one a caller may write, otherwise the sentence the refusal
/// states.
///
/// The sentence follows the form this service states a member requirement in — the
/// member name as the model spells it, quoted, then what the member has to satisfy. It
/// carries no key from the request, so a request with several reserved keys is refused
/// with one fixed sentence however its keys happen to be ordered.
pub fn validate_tag_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    for key in keys {
        if key.starts_with(RESERVED_TAG_KEY_PREFIX) {
            return Err(format!(
                "'{TAGS_MEMBER}' must not use the reserved prefix '{RESERVED_TAG_KEY_PREFIX}' in a tag key."
            ));
        }
    }
    Ok(())
}

/// Merge `incoming` into `tags`, or the sentence the refusal states.
///
/// The `TagResource` doc: "If you specify a new tag key for the resource, this tag is
/// appended to the tags already associated with the resource. If you specify a tag key
/// that's already associated with the resource, the new tag value that you specify
/// replaces the previous value for that tag." A refused merge writes NOTHING, so the
/// resource keeps exactly the tags it had.
pub fn merge_tags(
    tags: &mut BTreeMap<String, String>,
    incoming: HashMap<String, String>,
) -> Result<(), String> {
    validate_tag_keys(incoming.keys().map(String::as_str))?;
    let merged: BTreeSet<&str> = tags
        .keys()
        .chain(incoming.keys())
        .map(String::as_str)
        .collect();
    if merged.len() > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "'{TAGS_MEMBER}' must not take the resource past {MAX_TAGS_PER_RESOURCE} tags."
        ));
    }
    tags.extend(incoming);
    Ok(())
}

/// Remove `keys` from `tags`.
///
/// `UntagResource` is `@idempotent` in the model: a key the resource does not hold is
/// removed by doing nothing, so the second identical call answers exactly like the
/// first and never fails on a key the first one took away.
pub fn remove_tags(tags: &mut BTreeMap<String, String>, keys: &[String]) {
    for key in keys {
        tags.remove(key);
    }
}
