// SPDX-License-Identifier: CC0-1.0

//! The FEATURE STORE control-plane slice of this service's state: the feature group
//! `CreateFeatureGroup` writes, `DescribeFeatureGroup` replays and
//! `DeleteFeatureGroup` releases, with its quota, its statuses, the clock spans it
//! moves on and the sentences its refusals are worded with.
//!
//! Moved out of [`super`] unchanged, so the parent module stays inside the project's
//! file-length limit. Every name is re-exported from [`super`], so a handler keeps
//! naming `crate::state::…`.

use super::*;

/// How long after `CreateFeatureGroup` the clock owes the `Creating` -> `Created`
/// transition.
///
/// Same contract as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the feature
/// group straight after creating it still observes `Creating`, and one that polls (as
/// `aws sagemaker wait` and the Feature Store SDK's own wait loop do) observes
/// `Created`. The model's doc puts the real service at "approximately 10-15 minutes to
/// provision an OnlineStore FeatureGroup with the InMemory StorageType", so the span is
/// compressed, never zero.
pub const FEATURE_GROUP_CREATED_AFTER: Duration = Duration::from_millis(300);

/// How long after `DeleteFeatureGroup` the clock owes the `Deleting` -> gone transition
/// of a feature group.
///
/// The same compressed, never-zero span as [`FEATURE_GROUP_CREATED_AFTER`]: a caller
/// that describes the group straight after asking for its deletion still observes
/// `Deleting`, and one that polls observes the group gone. The model's doc puts the real
/// service at "approximately 10-15 minutes to delete an OnlineStore FeatureGroup with the
/// InMemory StorageType", so the span is compressed, never zero.
pub const FEATURE_GROUP_RELEASED_AFTER: Duration = FEATURE_GROUP_CREATED_AFTER;

/// See [`MODEL_QUOTA`]. `CreateFeatureGroup`'s doc points at the published quota
/// ("Check Amazon Web Services service quotas to see the FeatureGroups quota for your
/// Amazon Web Services account"), which is per-account and adjustable, so this is the
/// emulator's own bound rather than a claim about any account's limit.
pub const FEATURE_GROUP_QUOTA: usize = 1_000;

/// `FeatureGroupStatus`'s members this emulator moves a feature group through (the
/// modelled enum also carries `CreateFailed` and `DeleteFailed`, which this slice does
/// not reach: nothing here fails a feature group). Named once here so no handler compares
/// a bare string literal. Deliberately separate from [`STATUS_CREATING`] even though the
/// two spell the same value today: they are members of two different modelled enums, and
/// either may be re-spelled without the other.
pub const FEATURE_GROUP_STATUS_CREATING: &str = "Creating";
/// See [`FEATURE_GROUP_STATUS_CREATING`].
pub const FEATURE_GROUP_STATUS_CREATED: &str = "Created";
/// See [`FEATURE_GROUP_STATUS_CREATING`]. What a group reports from the
/// `DeleteFeatureGroup` that asked for its deletion until the clock completes it.
pub const FEATURE_GROUP_STATUS_DELETING: &str = "Deleting";

/// The `OfflineStoreStatusValue` an offline store reports while the replication it
/// describes is being made: "An `OfflineStore` status."
pub const OFFLINE_STORE_STATUS_ACTIVE: &str = "Active";

/// The `OfflineStoreStatusValue` an offline store reports while the replication CANNOT be
/// made: the role it runs under may not write the objects at the group's offline location,
/// so nothing a client ingests is retained there ([`offline_store_status`]).
pub const OFFLINE_STORE_STATUS_BLOCKED: &str = "Blocked";

/// The status of a feature group's offline store, as every report of that group carries it:
/// `None` for a group created with no `OfflineStoreConfig` (there is no replication to
/// report on), `Blocked` with a `BlockedReason` while the role the replication runs under
/// may not write the objects at the group's resolved output path, and `Active` otherwise.
///
/// "The status of the OfflineStore. Notifies you if replicating data into the OfflineStore
/// has failed" (`DescribeFeatureGroup$OfflineStoreStatus`): what can fail is the WRITE, made
/// under the group's own `RoleArn` — "the IAM execution role used to persist data into the
/// OfflineStore" — so the status is a function of the permission that role holds at the
/// moment the group is reported, which is why a client that repairs the role's policy sees
/// the status return to `Active` and one that revokes it sees `Blocked`. The permission
/// question is the shared one the data plane authorizes each of its own writes with
/// ([`::service_common::feature_store_offline_store::write_is_withheld`]), so a status this
/// control plane reports and a record that plane retains cannot disagree.
///
/// Reporting only: nothing is written here, and a role this emulator's IAM holds nothing
/// under withholds nothing (see the shared question's own docs).
pub async fn offline_store_status(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    offline_store_config: Option<&crate::ops::types::OfflineStoreConfig>,
    role_arn: Option<&str>,
) -> Option<crate::ops::types::OfflineStoreStatus> {
    let config = offline_store_config?;
    let active = crate::ops::types::OfflineStoreStatus {
        status: OFFLINE_STORE_STATUS_ACTIVE.to_string(),
        blocked_reason: None,
    };
    let storage = &config.s3_storage_config;
    // The resolved output path is what the records are written under, so it is the location
    // the permission is asked about; a group whose create could not resolve one falls back
    // to the `S3Uri` the create named, which is the only location it has.
    let resolved = storage
        .resolved_output_s3_uri
        .as_deref()
        .unwrap_or(storage.s3_uri.as_str());
    let (Some(location), Some(role_arn)) = (
        ::service_common::feature_store_offline_store::OfflineStoreLocation::parse(resolved),
        role_arn,
    ) else {
        return Some(active);
    };
    let key = ::service_common::feature_store_offline_store::status_key(&location);
    if ::service_common::feature_store_offline_store::write_is_withheld(
        callback,
        region,
        role_arn,
        &location,
        &key,
    )
    .await
    {
        return Some(crate::ops::types::OfflineStoreStatus {
            status: OFFLINE_STORE_STATUS_BLOCKED.to_string(),
            blocked_reason: Some(
                ::service_common::feature_store_offline_store::blocked_reason(
                    role_arn,
                    &location.uri(),
                ),
            ),
        });
    }
    Some(active)
}

/// The `ThroughputMode` a feature group runs under when its create stated no
/// `ThroughputConfig`: the mode that bills per read and write and needs no stated
/// capacity ("With on-demand mode, you are charged for data reads and writes that your
/// application performs on your feature group. You do not need to specify read and write
/// throughput" — `ThroughputConfig`), spelled as the model's `ThroughputMode` enum spells
/// it.
pub const THROUGHPUT_MODE_ON_DEMAND: &str = "OnDemand";

/// The throughput configuration a feature group whose create stated none runs under.
pub fn default_throughput_config() -> crate::ops::types::ThroughputConfig {
    crate::ops::types::ThroughputConfig {
        throughput_mode: THROUGHPUT_MODE_ON_DEMAND.to_string(),
        // "This field is not applicable for on-demand feature groups."
        provisioned_read_capacity_units: None,
        provisioned_write_capacity_units: None,
    }
}

/// The `<kind>` component of a feature group's ARN, i.e. the `<kind>` in
/// [`resource_arn`], from `FeatureGroupArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/`). Named once here
/// so the create mints, the readers resolve and the refusals match the one kind.
pub const FEATURE_GROUP_ARN_KIND: &str = "feature-group";

/// How SageMaker words a feature group this account does not hold: the `Resource Not
/// Found:` prefix, then the resource type and the name the request resolved to — a
/// sentence that ends with NO period.
///
/// Stated once here so the feature-group readers and the delete cannot word the same fact
/// differently. The name is the one the request addressed the group by, reduced from the
/// ARN form where the member accepts one ([`name_from_arn_or_name`]).
pub fn feature_group_not_found(name: &str) -> String {
    format!("Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name {name}")
}

/// How SageMaker words a refusal to delete a feature group that is not provisioned yet:
/// the `Validation Error:` prefix, the group's name, and the status that stands in the
/// way — reported in the upper case this message spells a `FeatureGroupStatus` in.
pub fn feature_group_not_deletable(name: &str, status: &str) -> String {
    format!(
        "Validation Error: FeatureGroup with name {name} cannot be deleted, because \
         FeatureGroupStatus is '{}'.",
        status.to_uppercase()
    )
}

/// The feature names a `FeatureGroup` may not declare, from `FeatureDefinitions`' own
/// doc: "FeatureNames cannot be any of the following: is_deleted, write_time,
/// api_invocation_time" — Feature Store writes those three columns itself.
pub const RESERVED_FEATURE_NAMES: &[&str] = &["is_deleted", "write_time", "api_invocation_time"];

/// A feature group created by `CreateFeatureGroup`, as `DescribeFeatureGroup` replays
/// it.
///
/// `created_due_at` is the instant the clock owes the `Creating` -> `Created` transition;
/// like [`Endpoint::in_service_at`] it is stored, never slept on, and materialised by
/// whichever reporting call first runs at or after it.
#[derive(Debug, Clone)]
pub struct FeatureGroup {
    pub name: String,
    pub arn: String,
    pub record_identifier_feature_name: String,
    pub event_time_feature_name: String,
    pub feature_definitions: Vec<crate::ops::types::FeatureDefinition>,
    pub online_store_config: Option<crate::ops::types::OnlineStoreConfig>,
    pub offline_store_config: Option<crate::ops::types::OfflineStoreConfig>,
    pub throughput_config: Option<crate::ops::types::ThroughputConfig>,
    pub role_arn: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<Tag>,
    pub status: String,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// WHEN the `Created` status falls due, not when the group was created — that is
    /// `creation_time`. Named the way [`Endpoint::in_service_at`] names the same thing.
    pub created_due_at: SystemTime,
    /// The instant the clock owes this group's removal, set by
    /// [`FeatureGroup::begin_deletion`]; `None` while no `DeleteFeatureGroup` has asked
    /// for one. Past it the group is gone for every caller
    /// ([`FeatureGroup::is_released`]), whether or not a sweep has dropped the record
    /// yet — the way [`Cluster::removal_due_at`] carries the same fact.
    pub removal_due_at: Option<SystemTime>,
}

impl FeatureGroup {
    /// Materialise the `Creating` -> `Created` transition the clock already owes.
    ///
    /// The one write a reporting operation may make, for the reason spelled out on
    /// [`Endpoint::settle_status`]: the transition was due at `created_due_at` whether or
    /// not anybody described the group.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == FEATURE_GROUP_STATUS_CREATING && now >= self.created_due_at {
            self.status = FEATURE_GROUP_STATUS_CREATED.to_string();
            self.last_modified_time = self.created_due_at;
            return true;
        }
        false
    }

    /// Start the deletion `DeleteFeatureGroup` asked for: the group reports `Deleting`,
    /// and the clock owes its removal [`FEATURE_GROUP_RELEASED_AFTER`] later.
    ///
    /// The record is NOT dropped here: `FeatureGroupStatus` carries `Deleting` and
    /// SageMaker answers it until the deletion finishes, so the transient status is
    /// observable and the removal is the clock's — stored as an instant and never slept
    /// on. The status change is an update to the group, so it moves
    /// `last_modified_time` too.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = FEATURE_GROUP_STATUS_DELETING.to_string();
        self.last_modified_time = now;
        self.removal_due_at = Some(now + FEATURE_GROUP_RELEASED_AFTER);
    }

    /// Whether a `DeleteFeatureGroup` has asked for this group's deletion, i.e. it is
    /// `Deleting` and on its way out.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the deletion the clock owed has completed, i.e. the group is gone.
    ///
    /// A lookup answers `None` for such a group and a listing leaves it out, so no caller
    /// sees a group whose removal was already due — the record itself is dropped by
    /// [`ComAmazonawsSagemakerSagemakerState::release_deleted_feature_groups`], which
    /// frees the name and the quota slot it held.
    pub fn is_released(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due| now >= due)
    }

    /// This group's throughput configuration as `DescribeFeatureGroup` reports it.
    ///
    /// `ThroughputConfig` (what a create states) and `ThroughputConfigDescription`
    /// (what a describe reports) are two shapes with the same three members, so the
    /// active configuration is the one stored for the group, re-shaped. Every group has
    /// one: a create that stated none is stored with
    /// [`default_throughput_config`], the mode the group then runs under.
    pub fn throughput_config_description(
        &self,
    ) -> Option<crate::ops::types::ThroughputConfigDescription> {
        self.throughput_config.as_ref().map(|config| {
            crate::ops::types::ThroughputConfigDescription {
                throughput_mode: config.throughput_mode.clone(),
                provisioned_read_capacity_units: config.provisioned_read_capacity_units,
                provisioned_write_capacity_units: config.provisioned_write_capacity_units,
            }
        })
    }

    /// Whether this group keeps an `OnlineStore`: "Turn OnlineStore off by specifying
    /// False for the EnableOnlineStore flag. … The default value is False."
    /// (`OnlineStoreConfig$EnableOnlineStore`.)
    pub fn online_store_enabled(&self) -> bool {
        self.online_store_config
            .as_ref()
            .and_then(|config| config.enable_online_store)
            .unwrap_or(false)
    }
}

/// The feature-group half of the store's API, in its own `impl` block for the same
/// reason as the section: a resource's state reads as one block.
impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another feature group fits, i.e. the store is below
    /// [`FEATURE_GROUP_QUOTA`].
    pub fn feature_groups_below_quota(&self) -> bool {
        self.feature_groups.len() < FEATURE_GROUP_QUOTA
    }

    /// The feature group a `FeatureGroupName` or `FeatureGroupNameOrArn` names, or `None`
    /// when this account holds none under it.
    ///
    /// The one lookup every feature-group operation resolves that member with, so no
    /// handler re-implements it: the ARN form is reduced to the name the store is keyed by
    /// ([`name_from_arn_or_name`]), and a group whose removal the clock already completed
    /// resolves to nothing — past that instant the group is gone whether or not the record
    /// has been dropped yet ([`FeatureGroup::is_released`]).
    pub fn feature_group_mut(&mut self, name_or_arn: &str) -> Option<&mut FeatureGroup> {
        let now = SystemTime::now();
        self.feature_groups
            .get_mut(name_from_arn_or_name(name_or_arn))
            .filter(|group| !group.is_released(now))
    }

    /// Drop every feature group whose removal the clock already completed, answering
    /// their ARNs in store order.
    ///
    /// Materialises only what was already due: the removal was owed at the stored instant
    /// whether or not anybody called, and a lookup already answers `None` for such a group
    /// ([`FeatureGroup::is_released`]), so this changes nothing a later caller sees — it
    /// frees the name and the quota slot the gone group still held.
    pub fn release_deleted_feature_groups(&mut self, now: SystemTime) -> Vec<String> {
        let released: Vec<(String, String)> = self
            .feature_groups
            .values()
            .filter(|group| group.is_released(now))
            .map(|group| (group.name.clone(), group.arn.clone()))
            .collect();
        for (name, _) in &released {
            self.feature_groups.remove(name);
        }
        released.into_iter().map(|(_, arn)| arn).collect()
    }
}
