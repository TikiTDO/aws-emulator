// SPDX-License-Identifier: CC0-1.0

//! The FEATURE GROUP DIRECTORY: which feature groups the SageMaker control plane holds,
//! and the two feature names each one's definition fixes.
//!
//! A SageMaker Feature Store feature group is DEFINED by the control plane
//! (`CreateFeatureGroup`: its schema, its `RecordIdentifierFeatureName`, its
//! `EventTimeFeatureName`, whether it has an `OnlineStore`) and USED by the FeatureStore
//! Runtime data plane (`PutRecord`, `GetRecord`, `UpdateRecord`, `DeleteRecord`,
//! `BatchGetRecord`, `ListRecords`), whose own API carries none of that definition: a
//! runtime request names a feature group and nothing else. So the data plane can only
//! resolve the name it is given — and can only know which feature of a record is its
//! identifier and which carries its event time — because the control plane RECORDED it.
//!
//! This is that record: ONE directory in the shared engine, written by the control plane
//! when it creates a feature group (and cleared when the group is deleted), read by the
//! data plane on every call. Nothing here names a wire shape: the directory holds the
//! group's name, its ARN, the two feature names its definition fixes, whether its
//! `OnlineStore` is on and where its `OfflineStore` replicates to
//! ([`crate::feature_store_offline_store`]), and a reader projects those onto the shapes
//! ITS model declares.
//!
//! Unlike [`crate::db_data_sources`], a name this directory does not hold is a FACT and
//! not a guess: a feature group exists only because `CreateFeatureGroup` minted it, so
//! the data plane refuses a name with no registration rather than treating it as
//! undecided. The directory is BOUNDED all the same — see
//! [`MAX_REGISTERED_FEATURE_GROUPS`] — so a caller looping on create cannot grow it
//! without end; the oldest registration is dropped first, which keeps the groups a live
//! workflow just created resolvable.

use std::collections::{HashMap, VecDeque};

/// How many feature groups the directory holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: this directory lives in memory for the
/// lifetime of the process, so it needs a maximum count. It is an order of magnitude
/// above the count the SageMaker control plane's own `CreateFeatureGroup` admits (its
/// `FEATURE_GROUP_QUOTA`, 1 000 today, which that handler refuses past with
/// `ResourceLimitExceeded`), so while that bound stands no group the control plane still
/// holds can be evicted from here. Were it ever to admit more, eviction drops the OLDEST
/// registration first — never a newer one — so the groups a live workflow just created
/// stay resolvable and only the longest-standing ones could become unresolvable to the
/// data plane.
pub const MAX_REGISTERED_FEATURE_GROUPS: usize = 10_000;

/// One feature group the control plane created, as the directory records it.
#[derive(Debug, Clone)]
pub struct FeatureGroupRecord {
    /// The `FeatureGroupName` the group was created with, which is also this record's
    /// key in the directory.
    pub name: String,
    /// The `FeatureGroupArn` the control plane minted for it.
    pub arn: String,
    /// `RecordIdentifierFeatureName`: "The name of the Feature whose value uniquely
    /// identifies a Record defined in the FeatureStore." A record's identifier is the
    /// value this feature carries.
    pub record_identifier_feature_name: String,
    /// `EventTimeFeatureName`: the name of the feature carrying "the event time of a
    /// Record", which is what the data plane orders two writes of one record by.
    pub event_time_feature_name: String,
    /// Whether the group has an `OnlineStore` —
    /// `OnlineStoreConfig.EnableOnlineStore`, whose documented default is `False`.
    /// The runtime data plane serves the OnlineStore, so a group without one holds no
    /// record it can read or write.
    pub online_store_enabled: bool,
    /// Where the group's records are replicated to, for a group created with an
    /// `OfflineStoreConfig`, and `None` for a group that has no offline store at all.
    ///
    /// "If a new record's EventTime is greater, the new record is written to both the
    /// OnlineStore and OfflineStore. Otherwise, the record is a historic record and it is
    /// written only to the OfflineStore" (`PutRecord`), so the data plane needs the
    /// destination and the role of EVERY ingestion — and the runtime API carries neither.
    pub offline_store: Option<crate::feature_store_offline_store::OfflineStoreDelivery>,
    /// The DEFAULT time-to-live the group's `OnlineStoreConfig.TtlDuration` states, in
    /// seconds ([`ttl_duration_seconds`]), and `None` for a group configured with none.
    ///
    /// "Amazon SageMaker Feature Store deletes the record when EventTime + TtlDuration
    /// elapses" (`TtlDuration`), so a default TTL makes every record of the group carry an
    /// expiry derived from its own event time — which is why the data plane cannot update
    /// such a record without being told an `EventTime`.
    pub default_ttl_seconds: Option<f64>,
}

/// `value` of the `TtlDurationUnit` `unit`, in seconds, or `None` when `unit` is not one
/// of the units the `TtlDurationUnit` enum declares.
///
/// The ONE reading of a `TtlDuration` the Feature Store API carries, so the control plane
/// that records a group's default duration and the data plane that applies a request's own
/// cannot read the same unit two ways. The units are the enum's own values: Seconds,
/// Minutes, Hours, Days, Weeks.
#[must_use]
pub fn ttl_duration_seconds(unit: &str, value: i32) -> Option<f64> {
    let unit_secs = match unit {
        "Seconds" => 1.0,
        "Minutes" => 60.0,
        "Hours" => 3_600.0,
        "Days" => 86_400.0,
        "Weeks" => 604_800.0,
        _ => return None,
    };
    Some(f64::from(value) * unit_secs)
}

/// The directory itself: feature group name -> its definition.
///
/// Registered in the shared engine under THIS type, so the control plane that defines a
/// feature group and the data plane that serves its records read one directory instead
/// of each keeping its own copy.
#[derive(Debug, Default)]
pub struct FeatureGroupDirectory {
    /// The records, by feature group name.
    groups: HashMap<String, FeatureGroupRecord>,
    /// Registration order, so the ceiling drops the oldest entry first.
    order: VecDeque<String>,
}

impl FeatureGroupDirectory {
    /// Record `group`'s definition under its own name.
    ///
    /// Re-registering a name REPLACES its record (the control plane correcting what it
    /// told the directory, as an update to the group's definition does) and does not move
    /// its place in the eviction order.
    pub fn register(&mut self, group: FeatureGroupRecord) {
        let name = group.name.clone();
        if self.groups.insert(name.clone(), group).is_none() {
            self.order.push_back(name);
            while self.order.len() > MAX_REGISTERED_FEATURE_GROUPS {
                if let Some(oldest) = self.order.pop_front() {
                    self.groups.remove(&oldest);
                }
            }
        }
    }

    /// Drop the record of the feature group `name_or_arn` names: the group is gone, so
    /// the directory no longer claims it exists.
    pub fn forget(&mut self, name_or_arn: &str) {
        let name = group_name(name_or_arn);
        if self.groups.remove(name).is_some() {
            self.order.retain(|held| held != name);
        }
    }

    /// The definition of the feature group `name_or_arn` names, or `None` when no
    /// feature group of that name was ever created.
    ///
    /// Both spellings the runtime's `FeatureGroupNameOrArn` admits reach one record (see
    /// [`group_name`]).
    pub fn get(&self, name_or_arn: &str) -> Option<&FeatureGroupRecord> {
        self.groups.get(group_name(name_or_arn))
    }

    /// How many feature groups the directory holds.
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    /// Whether the directory holds no feature group at all.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

/// The feature group NAME a `FeatureGroupNameOrArn` carries.
///
/// The runtime member's pattern is
/// `^(arn:aws…:sagemaker:…:feature-group/)?(<name>)$`: the ARN prefix is OPTIONAL, so the
/// ARN and the bare name denote the same feature group and must reach the same record.
/// The ARN form is read through [`crate::arn::resource_id_of_type`] — this crate's ONE
/// parse of a caller-supplied resource ARN — so only a well-formed ARN naming a
/// `feature-group` contributes its id; anything else is taken as the bare name it is, and
/// a value carrying a `/` for some other reason cannot be mistaken for an ARN's tail. Two
/// ARNs that differ only in region or account therefore collapse onto one record, which
/// is correct for an emulator serving a single account and Region.
pub fn group_name(name_or_arn: &str) -> &str {
    crate::arn::resource_id_of_type(name_or_arn, FEATURE_GROUP_RESOURCE_TYPE)
        .unwrap_or(name_or_arn)
}

/// The `<type>` component of a feature group's ARN, from `FeatureGroupArn`'s own pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/`).
const FEATURE_GROUP_RESOURCE_TYPE: &str = "feature-group";

#[cfg(test)]
mod tests {
    use super::*;

    /// One registration, named.
    fn group(name: &str) -> FeatureGroupRecord {
        FeatureGroupRecord {
            name: name.to_owned(),
            arn: format!("arn:aws:sagemaker:us-east-1:000000000000:feature-group/{name}"),
            record_identifier_feature_name: "record_id".to_owned(),
            event_time_feature_name: "event_time".to_owned(),
            online_store_enabled: true,
            offline_store: None,
            default_ttl_seconds: None,
        }
    }

    /// Every unit the `TtlDurationUnit` enum declares converts to its own span in seconds,
    /// and a value outside that set converts to nothing at all.
    #[test]
    fn a_ttl_duration_converts_by_the_unit_the_model_declares() {
        // (case, unit, value, expected seconds)
        let rows: [(&str, &str, i32, Option<f64>); 8] = [
            ("seconds", "Seconds", 30, Some(30.0)),
            ("minutes", "Minutes", 2, Some(120.0)),
            ("hours", "Hours", 1, Some(3_600.0)),
            ("days", "Days", 3, Some(259_200.0)),
            ("weeks", "Weeks", 1, Some(604_800.0)),
            ("a zero value", "Hours", 0, Some(0.0)),
            ("a unit the enum does not declare", "Fortnights", 1, None),
            ("a unit in the wrong case", "hours", 1, None),
        ];
        for (case, unit, value, expected) in rows {
            let actual = ttl_duration_seconds(unit, value);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?} seconds, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_registered_group_resolves_by_name_and_by_arn() {
        let mut directory = FeatureGroupDirectory::default();
        directory.register(group("customers"));

        // (case, the spelling a data-plane request carries, whether it resolves)
        let rows = [
            ("the bare name", "customers".to_owned(), true),
            (
                "the ARN the control plane minted",
                "arn:aws:sagemaker:us-east-1:000000000000:feature-group/customers".to_owned(),
                true,
            ),
            (
                "an ARN of another Region and account",
                "arn:aws:sagemaker:eu-west-1:123456789012:feature-group/customers".to_owned(),
                true,
            ),
            ("a name never created", "nosuchgroup".to_owned(), false),
            (
                "an ARN of a name never created",
                "arn:aws:sagemaker:us-east-1:000000000000:feature-group/nosuchgroup".to_owned(),
                false,
            ),
            // Only a well-formed ARN naming a `feature-group` contributes its id: an ARN
            // of another resource type whose id happens to be a created group's name
            // names no feature group.
            (
                "an ARN of another resource type",
                "arn:aws:sagemaker:us-east-1:000000000000:endpoint/customers".to_owned(),
                false,
            ),
            (
                "a value that merely carries a slash",
                "nosuchgroup/customers".to_owned(),
                false,
            ),
        ];
        for (case, spelling, expected) in rows {
            let actual = directory.get(&spelling).is_some();
            assert_eq!(
                actual, expected,
                "case {case:?} ({spelling}): expected resolves={expected}, got {actual}"
            );
        }
        let held = directory.get("customers").expect("the registered group");
        assert_eq!(
            (
                held.record_identifier_feature_name.as_str(),
                held.event_time_feature_name.as_str(),
                held.online_store_enabled
            ),
            ("record_id", "event_time", true),
            "case the registered group: expected the definition it was registered with, got \
             {held:?}"
        );
    }

    #[test]
    fn re_registering_a_name_replaces_its_definition_without_growing_the_directory() {
        let mut directory = FeatureGroupDirectory::default();
        directory.register(group("customers"));
        let mut updated = group("customers");
        updated.online_store_enabled = false;
        directory.register(updated);

        assert_eq!(
            directory.len(),
            1,
            "case a re-registration: expected one record, got {}",
            directory.len()
        );
        assert_eq!(
            directory
                .get("customers")
                .map(|held| held.online_store_enabled),
            Some(false),
            "case a re-registration: expected the newest definition to stand"
        );
    }

    #[test]
    fn forgetting_a_group_drops_it_and_leaves_the_others() {
        let mut directory = FeatureGroupDirectory::default();
        directory.register(group("kept"));
        directory.register(group("dropped"));

        directory.forget("arn:aws:sagemaker:us-east-1:000000000000:feature-group/dropped");
        assert!(
            directory.get("dropped").is_none() && directory.get("kept").is_some(),
            "case one group forgotten by ARN: expected only it to go, got {} record(s)",
            directory.len()
        );

        // Forgetting a name the directory does not hold changes nothing, so a repeated
        // delete is harmless.
        directory.forget("dropped");
        directory.forget("nosuchgroup");
        assert_eq!(
            directory.len(),
            1,
            "case forgetting an absent name: expected the remaining record untouched, got {}",
            directory.len()
        );

        directory.forget("kept");
        assert!(
            directory.is_empty(),
            "case every group forgotten: expected an empty directory, got {} record(s)",
            directory.len()
        );
    }

    #[test]
    fn the_ceiling_drops_the_oldest_registration_first() {
        let mut directory = FeatureGroupDirectory::default();
        for index in 0..MAX_REGISTERED_FEATURE_GROUPS {
            directory.register(group(&format!("fg{index}")));
        }
        assert_eq!(
            directory.len(),
            MAX_REGISTERED_FEATURE_GROUPS,
            "case a directory filled to the ceiling: expected \
             {MAX_REGISTERED_FEATURE_GROUPS} records, got {}",
            directory.len()
        );

        // A re-registration is not a new entry, so it moves nothing out.
        directory.register(group("fg0"));
        assert!(
            directory.get("fg0").is_some() && directory.len() == MAX_REGISTERED_FEATURE_GROUPS,
            "case a re-registration at the ceiling: expected the record kept, got {} record(s)",
            directory.len()
        );

        // One more NEW name evicts the oldest and only the oldest.
        directory.register(group(&format!("fg{MAX_REGISTERED_FEATURE_GROUPS}")));
        assert_eq!(
            directory.len(),
            MAX_REGISTERED_FEATURE_GROUPS,
            "case one past the ceiling: expected the ceiling held, got {}",
            directory.len()
        );
        assert!(
            directory.get("fg0").is_none(),
            "case one past the ceiling: expected the oldest registration dropped"
        );
        assert!(
            directory.get("fg1").is_some()
                && directory
                    .get(&format!("fg{MAX_REGISTERED_FEATURE_GROUPS}"))
                    .is_some(),
            "case one past the ceiling: expected the newest registrations kept"
        );
    }
}
