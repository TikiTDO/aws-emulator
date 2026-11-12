// SPDX-License-Identifier: CC0-1.0

use super::*;

/// One resource type a Redshift tag can be attached to: the segment its ARN
/// carries and the name the API publishes for it.
#[derive(Debug, Clone, Copy)]
pub struct TaggableResourceType {
    /// The resource-type segment of this type's ARN, which is also the value
    /// `TaggedResource$ResourceType` reports and the value a
    /// `DescribeTags$ResourceType` filter names.
    pub arn_segment: &'static str,
    /// The human-readable name the model lists for the type, accepted on a
    /// `DescribeTags$ResourceType` filter beside [`Self::arn_segment`].
    pub documented_name: &'static str,
}

/// Every resource type a Redshift tag can be attached to, in the order the model
/// lists them.
///
/// Source for the names and the order: `smithy_query(query="doc",
/// shape="TaggedResource", member="ResourceType")` — "Valid resource types are:
/// Cluster, CIDR/IP, EC2 security group, Snapshot, Cluster security group,
/// Subnet group, HSM connection, HSM certificate, Parameter group" — plus the
/// two `smithy_query(query="doc", shape="DescribeTagsMessage",
/// member="ResourceType")` adds at the end of the same list ("Snapshot copy
/// grant", "Integration"). Source for each ARN segment: the Amazon Redshift
/// Cluster Management Guide page that same doc links to, "Constructing an Amazon
/// Redshift Amazon Resource Name (ARN)", which gives one ARN form per resource
/// type.
///
/// Only four of these types name a resource THIS store holds (see
/// [`ComAmazonawsRedshiftRedshiftserviceversion20121201State::set_resource_tags`]);
/// the rest are listed because they are valid `ResourceType` filters, and an
/// account that cannot create such a resource yet simply has none of them to
/// report.
pub const TAGGABLE_RESOURCE_TYPES: [TaggableResourceType; 11] = [
    TaggableResourceType {
        arn_segment: ARN_SEGMENT_CLUSTER,
        documented_name: "Cluster",
    },
    TaggableResourceType {
        arn_segment: "securitygroupingress-cidr",
        documented_name: "CIDR/IP",
    },
    TaggableResourceType {
        arn_segment: "securitygroupingress-ec2securitygroup",
        documented_name: "EC2 security group",
    },
    TaggableResourceType {
        arn_segment: ARN_SEGMENT_SNAPSHOT,
        documented_name: "Snapshot",
    },
    TaggableResourceType {
        arn_segment: "securitygroup",
        documented_name: "Cluster security group",
    },
    TaggableResourceType {
        arn_segment: ARN_SEGMENT_SUBNET_GROUP,
        documented_name: "Subnet group",
    },
    TaggableResourceType {
        arn_segment: ARN_SEGMENT_HSM_CONFIGURATION,
        documented_name: "HSM connection",
    },
    TaggableResourceType {
        arn_segment: "hsmclientcertificate",
        documented_name: "HSM certificate",
    },
    TaggableResourceType {
        arn_segment: ARN_SEGMENT_PARAMETER_GROUP,
        documented_name: "Parameter group",
    },
    TaggableResourceType {
        arn_segment: "snapshotcopygrant",
        documented_name: "Snapshot copy grant",
    },
    TaggableResourceType {
        arn_segment: "integration",
        documented_name: "Integration",
    },
];

/// A `ResourceType` or ARN segment folded for comparison: lower-cased with every
/// separator dropped, so one spelling of a type matches every other the API
/// publishes for it (`Subnet group`, `subnet-group`, `subnetgroup`).
pub(super) fn folded_resource_type(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The taggable resource type a request's `ResourceType` names, by either the
/// ARN segment or the name the model publishes, in any case or separator
/// spelling; `None` when it names no published type.
pub fn resource_type_named(requested: &str) -> Option<&'static TaggableResourceType> {
    let wanted = folded_resource_type(requested);
    TAGGABLE_RESOURCE_TYPES.iter().find(|candidate| {
        folded_resource_type(candidate.arn_segment) == wanted
            || folded_resource_type(candidate.documented_name) == wanted
    })
}

/// Every published `ResourceType` value, for the message that refuses one this
/// service does not publish.
pub fn published_resource_types() -> Vec<&'static str> {
    TAGGABLE_RESOURCE_TYPES
        .iter()
        .map(|candidate| candidate.arn_segment)
        .collect()
}

/// The service field of an Amazon Redshift ARN: the only ARNs whose resource a
/// tagging operation of this service resolves (see [`cluster_arn`] for the form
/// the Cluster Management Guide publishes).
const ARN_SERVICE: &str = "redshift";

/// The published resource type and the bare identifier the resource portion of
/// `resource_name` carries, or `None` when the value is not an Amazon Redshift
/// ARN of a type this service publishes.
///
/// A Redshift ARN's resource portion is `<type>:<identifier>`, with the type one
/// of [`TAGGABLE_RESOURCE_TYPES`]'s segments and the identifier everything after
/// it — for a snapshot the `<cluster>/<snapshot>` pair its ARN carries (see
/// [`snapshot_arn`]).
fn arn_resource(resource_name: &str) -> Option<(&'static str, &str)> {
    if resource_name.len() > MAX_ARN_LENGTH {
        return None;
    }
    let mut fields = resource_name.splitn(7, ':');
    if fields.next()? != "arn" {
        return None;
    }
    // The partition, the service, the Region and the account: only the service
    // decides whether the value can name a resource of THIS control plane.
    fields.next()?;
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    fields.next()?;
    fields.next()?;
    let resource_type = fields.next()?;
    let identifier = fields.next().filter(|value| !value.is_empty())?;
    let published = TAGGABLE_RESOURCE_TYPES
        .iter()
        .find(|candidate| candidate.arn_segment == resource_type)?;
    Some((published.arn_segment, identifier))
}

/// Why a request naming a resource this account holds none of is refused.
///
/// Amazon Redshift words it from what the ARN NAMES rather than echoing the ARN:
/// the resource type the ARN carries, then the bare identifier that follows it,
/// as `<type> (<identifier>) was not found.` The type is the ARN's own
/// resource-type segment, which is the same value `TaggedResource$ResourceType`
/// reports for a resource of that type, so a missing cluster, snapshot and
/// parameter group each read as what they are rather than as one sentence with a
/// different ARN in it. A value that is no Amazon Redshift ARN of a published
/// type names no type to report ([`arn_resource`]), so that one is refused
/// naming the value the request carried.
///
/// One function for every operation of this service that resolves an ARN to
/// nothing (see `create_tags.rs`, `delete_tags.rs`, `describe_tags.rs`), so one
/// condition reads the same whichever operation reports it.
pub fn resource_not_found_message(resource_name: &str) -> String {
    match arn_resource(resource_name) {
        Some((resource_type, identifier)) => {
            format!("{resource_type} ({identifier}) was not found.")
        }
        None => format!("The resource could not be found: {resource_name}"),
    }
}

/// How many tags one resource may carry.
///
/// Source: `smithy_query(query="doc", operation="CreateTags")` — "A resource can
/// have up to 50 tags. If you try to create more than 50 tags for a resource,
/// you will receive an error and the attempt will fail."
pub const TAGS_PER_RESOURCE_QUOTA: usize = 50;

/// The longest a tag key may be, in characters.
///
/// Source: the Amazon Redshift Cluster Management Guide, "Tagging resources in
/// Amazon Redshift" — its tag restrictions give a key a maximum of 128 Unicode
/// characters and a value a maximum of 256 ([`TAG_VALUE_MAX_LEN`]), and reserve
/// the [`RESERVED_TAG_PREFIX`] prefix. The model itself bounds neither
/// (`smithy_query(query="shape", shape="Tag")` targets plain `String` for both),
/// so the guide is the only published bound.
pub const TAG_KEY_MAX_LEN: usize = 128;

/// The longest a tag value may be, in characters. See [`TAG_KEY_MAX_LEN`].
pub const TAG_VALUE_MAX_LEN: usize = 256;

/// The key prefix AWS reserves for its own tags. See [`TAG_KEY_MAX_LEN`].
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// Check one request's tags against the published restrictions, naming the tag
/// and the restriction it breaks. An `Err` is an `InvalidTagFault`.
///
/// Length is counted in CHARACTERS, not bytes: the guide bounds a key at 128 and
/// a value at 256 Unicode characters, so a multi-byte value is bounded by what
/// it spells and not by how UTF-8 encodes it.
pub fn check_tags(tags: &[crate::ops::types::Tag]) -> Result<(), String> {
    for tag in tags {
        let key = tag.key.as_deref().unwrap_or_default();
        if key.is_empty() {
            return Err("Invalid tag: a tag key cannot be null, empty, or blank.".to_string());
        }
        if key.chars().count() > TAG_KEY_MAX_LEN {
            return Err(format!(
                "Invalid tag key '{key}': a tag key cannot be longer than {TAG_KEY_MAX_LEN} characters."
            ));
        }
        if key.to_lowercase().starts_with(RESERVED_TAG_PREFIX) {
            return Err(format!(
                "Invalid tag key '{key}': the prefix '{RESERVED_TAG_PREFIX}' is reserved for Amazon Web Services."
            ));
        }
        let value = tag.value.as_deref().unwrap_or_default();
        if value.chars().count() > TAG_VALUE_MAX_LEN {
            return Err(format!(
                "Invalid tag value for key '{key}': a tag value cannot be longer than {TAG_VALUE_MAX_LEN} characters."
            ));
        }
        if value.to_lowercase().starts_with(RESERVED_TAG_PREFIX) {
            return Err(format!(
                "Invalid tag value for key '{key}': the prefix '{RESERVED_TAG_PREFIX}' is reserved for Amazon Web Services."
            ));
        }
    }
    Ok(())
}

/// The tag list a resource carrying `existing` has after `incoming` is added,
/// or the message of the `TagLimitExceededFault` the request exceeds.
///
/// A key already on the resource keeps its position and takes the new value —
/// `smithy_query(query="doc", operation="CreateTags")`: "If you specify a key
/// that already exists for the resource, the value for that key will be updated
/// with the new value" — which is also what a key repeated WITHIN one request
/// does, the later pair winning. A key the resource does not carry is appended.
/// Computed into a new list rather than applied in place because the attempt
/// must "fail" as a whole when it would exceed [`TAGS_PER_RESOURCE_QUOTA`]:
/// nothing is persisted on the error path.
///
/// A tag whose `Value` the request omits is stored as the EMPTY value, which is
/// the value such a tag reports: the member is modelled optional but a stored
/// tag always has one.
pub fn tags_with(
    existing: &[crate::ops::types::Tag],
    incoming: &[crate::ops::types::Tag],
) -> Result<Vec<crate::ops::types::Tag>, String> {
    let mut merged = existing.to_vec();
    // The keys already in `merged`, by position: ONE lookup per incoming tag
    // rather than a scan of the merged list, so the merge cost grows with the
    // request rather than with its square.
    let mut positions: BTreeMap<String, usize> = merged
        .iter()
        .enumerate()
        .filter_map(|(position, tag)| tag.key.clone().map(|key| (key, position)))
        .collect();
    for tag in incoming {
        let key = tag.key.clone().unwrap_or_default();
        let value = Some(tag.value.clone().unwrap_or_default());
        match positions.get(&key) {
            Some(&position) => merged[position].value = value,
            None => {
                // Checked as the list GROWS, before the entry past the quota is
                // allocated, so a request that must fail cannot first do the
                // work of one that would have succeeded.
                if merged.len() == TAGS_PER_RESOURCE_QUOTA {
                    return Err(tag_quota_message(merged.len() + 1));
                }
                positions.insert(key.clone(), merged.len());
                merged.push(crate::ops::types::Tag {
                    key: Some(key),
                    value,
                });
            }
        }
    }
    Ok(merged)
}

/// Why a request past [`TAGS_PER_RESOURCE_QUOTA`] is refused, naming the count
/// the resource would have carried.
pub fn tag_quota_message(would_carry: usize) -> String {
    format!(
        "The request would exceed the allowed number of tags for one resource ({TAGS_PER_RESOURCE_QUOTA}); the resource would carry {would_carry}."
    )
}

/// A request's `TagKeys` / `TagValues` filter, indexed once for matching.
///
/// Built from the request's own lists BEFORE the store is walked, so testing a
/// resource against the filter costs one set lookup per tag instead of a scan of
/// the caller's lists: the work is the SUM of the request and the store rather
/// than their product, whatever lengths a caller sends.
#[derive(Debug, Default)]
pub struct TagFilter<'a> {
    /// The keys a resource must carry one of, or `None` when the filter does not
    /// constrain keys.
    pub(super) keys: Option<BTreeSet<&'a str>>,
    /// The values a resource must carry one of, likewise.
    pub(super) values: Option<BTreeSet<&'a str>>,
}

impl<'a> TagFilter<'a> {
    /// Index one request's `TagKeys` / `TagValues`.
    ///
    /// An absent list — and an EMPTY one, which names nothing to match —
    /// constrains nothing.
    pub fn new(tag_keys: Option<&'a [String]>, tag_values: Option<&'a [String]>) -> Self {
        let index = |list: Option<&'a [String]>| {
            list.filter(|values| !values.is_empty())
                .map(|values| values.iter().map(String::as_str).collect())
        };
        Self {
            keys: index(tag_keys),
            values: index(tag_values),
        }
    }

    /// Whether `tags` satisfy this filter.
    ///
    /// Each filter is independent and inclusive: a resource passes the keys when
    /// it carries a tag whose key is one of them, and the values when it carries
    /// a tag whose value is one of them.
    pub fn matches(&self, tags: &[crate::ops::types::Tag]) -> bool {
        if let Some(keys) = self.keys.as_ref() {
            if !tags
                .iter()
                .any(|tag| tag.key.as_deref().is_some_and(|key| keys.contains(key)))
            {
                return false;
            }
        }
        if let Some(values) = self.values.as_ref() {
            if !tags.iter().any(|tag| {
                tag.value
                    .as_deref()
                    .is_some_and(|value| values.contains(value))
            }) {
                return false;
            }
        }
        true
    }
}

/// Whether a resource's tags satisfy a request's `TagKeys` / `TagValues` filter.
///
/// For the one-resource-at-a-time callers; a caller that walks the store builds
/// the [`TagFilter`] once and matches through it instead.
pub fn tags_match(
    tags: &[crate::ops::types::Tag],
    tag_keys: Option<&[String]>,
    tag_values: Option<&[String]>,
) -> bool {
    TagFilter::new(tag_keys, tag_values).matches(tags)
}
