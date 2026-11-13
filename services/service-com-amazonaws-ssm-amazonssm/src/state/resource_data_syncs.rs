// SPDX-License-Identifier: CC0-1.0

//! The resource data sync configurations this service holds: "A resource data sync helps
//! you view data from multiple sources in a single location"
//! (`smithy_query(query="doc",shape="CreateResourceDataSync")`).
//!
//! `CreateResourceDataSync` persists one, `ListResourceDataSync` reports what is held,
//! `UpdateResourceDataSync` replaces the source of one and `DeleteResourceDataSync`
//! removes it — all four reach the one store below
//! ([`super::ComAmazonawsSsmAmazonssmState::resource_data_syncs`]), so whatever a write
//! persisted is what a later read reports.
//!
//! A sync is created through the endpoint of ONE Region and the data it aggregates is
//! aggregated per Region ("After you create a resource data sync for a Region, you can't
//! change the account options for that sync", `UpdateResourceDataSync`'s doc), so the
//! store is keyed by `(region, sync name)`: the name is the caller's own
//! (`ResourceDataSyncName`, `len 1..64`) and is unique within the Region, which is what
//! `ResourceDataSyncAlreadyExistsException` ("A sync configuration with the same name
//! already exists") reports a collision on — that shape carries the `SyncName` alone, so
//! the name is the identity and the sync type is a property of the sync rather than part
//! of its key.
//!
//! A request that names a sync carries the name and, OPTIONALLY, the type
//! ([`addressed_sync_type`]): where the type is omitted the documented default applies —
//! "The default value is SyncToDestination" (`CreateResourceDataSync`'s `SyncType` doc) —
//! and a type that is not the one the stored sync carries addresses no sync, which is
//! what `ResourceDataSyncNotFoundException` reports, naming the name and the type it
//! looked for ([`sync_not_found_message`]).
//!
//! Everything a read reports is what the create persisted or what a later update
//! replaced ([`ResourceDataSyncRecord::apply_update`]). What a read does NOT report is
//! the outcome of a sync run: nothing here copies Inventory or OpsData anywhere, so a
//! sync has never run, and `LastStatus`, `LastSyncTime`, `LastSuccessfulSyncTime` and
//! `LastSyncStatusMessage` are left unset rather than claiming a run nobody made.

use std::time::SystemTime;

use crate::ops::types::{
    ResourceDataSyncItem, ResourceDataSyncS3Destination, ResourceDataSyncSource,
    ResourceDataSyncSourceWithState,
};

/// The sync type that "synchronizes data to an S3 bucket for Inventory", and the type a
/// create that names none gets: "The default value is SyncToDestination"
/// (`CreateResourceDataSync`'s `SyncType` doc).
pub const SYNC_TYPE_TO_DESTINATION: &str = "SyncToDestination";

/// The sync type that synchronizes "data from a single account and multiple Regions, or
/// multiple Amazon Web Services accounts and Amazon Web Services Regions, as listed in
/// Organizations for Explorer" (same doc).
pub const SYNC_TYPE_FROM_SOURCE: &str = "SyncFromSource";

/// The sync types a request may name. `ResourceDataSyncType` is a plain string in the
/// model (`len 1..64`) rather than an enum, so the pair the docs name is checked here.
pub const SYNC_TYPES: &[&str] = &[SYNC_TYPE_TO_DESTINATION, SYNC_TYPE_FROM_SOURCE];

/// A source that aggregates "data from Organizations"
/// (`ResourceDataSyncSource$AwsOrganizationsSource`'s doc).
pub const SOURCE_TYPE_AWS_ORGANIZATIONS: &str = "AwsOrganizations";

/// A source that aggregates data from one account's Regions: "SourceType is either
/// AwsOrganizations (if an organization is present in Organizations) or
/// SingleAccountMultiRegions" (`ResourceDataSyncSource$SourceType`'s doc).
pub const SOURCE_TYPE_SINGLE_ACCOUNT_MULTI_REGIONS: &str = "SingleAccountMultiRegions";

/// The source types a sync source may name, from the `SourceType` doc quoted on
/// [`SOURCE_TYPE_SINGLE_ACCOUNT_MULTI_REGIONS`]. `ResourceDataSyncSourceType` is a plain
/// string in the model, so the pair is checked here.
pub const SOURCE_TYPES: &[&str] = &[
    SOURCE_TYPE_AWS_ORGANIZATIONS,
    SOURCE_TYPE_SINGLE_ACCOUNT_MULTI_REGIONS,
];

/// An Organizations source aggregating "from a set of organization units", which is the
/// one organization source type that names them
/// (`ResourceDataSyncAwsOrganizationsSource$OrganizationSourceType`'s doc).
pub const ORGANIZATION_SOURCE_TYPE_ORGANIZATIONAL_UNITS: &str = "OrganizationalUnits";

/// An Organizations source aggregating "from the entire Amazon Web Services
/// organization" (same doc).
pub const ORGANIZATION_SOURCE_TYPE_ENTIRE_ORGANIZATION: &str = "EntireOrganization";

/// The organization source types an Organizations source may name, from the doc quoted
/// on [`ORGANIZATION_SOURCE_TYPE_ORGANIZATIONAL_UNITS`].
pub const ORGANIZATION_SOURCE_TYPES: &[&str] = &[
    ORGANIZATION_SOURCE_TYPE_ORGANIZATIONAL_UNITS,
    ORGANIZATION_SOURCE_TYPE_ENTIRE_ORGANIZATION,
];

/// The lower bound of the modelled `ResourceDataSyncName` (`len 1..64`), which is also
/// the shape a `NextToken` this service issued has: the token is the name of the sync the
/// next page starts at.
pub const MIN_SYNC_NAME_LENGTH: usize = 1;

/// The upper bound of the same shape.
pub const MAX_SYNC_NAME_LENGTH: usize = 64;

/// The lower bound of the modelled `MaxResults` of a sync listing (`range 1..50`).
pub const MIN_PAGE_SIZE: i32 = 1;

/// The upper bound of the same member, and the page a request that names no `MaxResults`
/// is served up to (see [`super::page_size`]).
pub const MAX_PAGE_SIZE: i32 = 50;

/// How many sync configurations this store holds at once, across every Region.
///
/// This is the EMULATOR's own ceiling rather than a quota Systems Manager publishes: a
/// create is the only thing that admits a sync, callers may create as many as they like,
/// and the store is held in memory for as long as the process runs — so without a ceiling
/// the one map would grow for as long as creates keep arriving. A create past it is
/// refused through the modelled `ResourceDataSyncCountExceededException` ("You have
/// exceeded the allowed maximum sync configurations") and writes nothing.
pub const MAX_RESOURCE_DATA_SYNCS: usize = 100;

/// What a create refused by [`MAX_RESOURCE_DATA_SYNCS`] reports: the emulator's own
/// ceiling, named as the emulator's, with the figure it stands at.
pub fn sync_ceiling_message() -> String {
    format!(
        "This emulator holds at most {MAX_RESOURCE_DATA_SYNCS} resource data sync \
         configurations across every Region"
    )
}

/// How this service reports a sync a request addresses that the Region holds none of,
/// through `ResourceDataSyncNotFoundException`.
///
/// The sentence is this error class's own, and its variable parts are the name and the
/// type the request addressed the sync by ([`addressed_sync_type`]) — which the shape's
/// `SyncName` and `SyncType` members carry beside it.
pub fn sync_not_found_message(sync_name: &str, sync_type: &str) -> String {
    format!("ResourceDataSync with name {sync_name} synctype {sync_type} does not exists")
}

/// The page size a sync listing serves (see [`super::page_size`]).
pub fn page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(max_results, MIN_PAGE_SIZE, MAX_PAGE_SIZE)
}

/// Whether `candidate` could be a `ResourceDataSyncName`, and so whether a `NextToken`
/// is one this service issued: the token a page hands out is the name of the sync the
/// next page starts at, so a value no name could take was not issued here.
pub fn is_sync_name(candidate: &str) -> bool {
    (MIN_SYNC_NAME_LENGTH..=MAX_SYNC_NAME_LENGTH).contains(&candidate.chars().count())
}

/// The sync type a request addresses a sync by: the one it names, or the documented
/// default for a request that names none ("The default value is SyncToDestination",
/// `CreateResourceDataSync`'s `SyncType` doc).
///
/// Every operation that addresses a sync goes through here, so a create, a listing, an
/// update and a delete all read an omitted `SyncType` the same way.
pub fn addressed_sync_type(requested: Option<&str>) -> &str {
    requested.unwrap_or(SYNC_TYPE_TO_DESTINATION)
}

/// Why this sync type cannot be served, or `None` when it can: the two values the docs
/// name are the only types a sync carries, so a request naming any other addresses a
/// kind of sync that cannot exist.
///
/// `ResourceDataSyncType` is a plain string in the model (`len 1..64`) rather than an
/// enum, so the walk admits any text of that length and every operation of this slice
/// that reads a `SyncType` refuses an unknown one through here, with one wording.
pub fn sync_type_rejection(sync_type: &str) -> Option<String> {
    (!SYNC_TYPES.contains(&sync_type)).then(|| {
        format!(
            "The sync type {sync_type} is not valid. Specify either \
             {SYNC_TYPE_TO_DESTINATION} or {SYNC_TYPE_FROM_SOURCE}."
        )
    })
}

/// Why this sync configuration cannot be served, or `None` when it can: the rules the
/// docs state over the three members a create carries together, which is what
/// `ResourceDataSyncInvalidConfigurationException` ("The specified sync configuration is
/// invalid") reports.
///
/// The members' own lengths and patterns, and the members each structure requires, are
/// MODELLED constraints the centralized request walk has already refused anything
/// outside of; what is left is which member has to be present for which sync type, and
/// the value sets the docs name for the three string members the model does not model as
/// enums.
///
/// A member the sync type does not USE is not refused here: `S3Destination` is required
/// "if the SyncType value is SyncToDestination" and `SyncSource` "if the SyncType value
/// is SyncFromSource", and the docs state no rule against carrying the other one, so what
/// a create supplied is persisted and reported back rather than silently dropped.
pub fn configuration_rejection(
    sync_type: &str,
    s3_destination: Option<&ResourceDataSyncS3Destination>,
    sync_source: Option<&ResourceDataSyncSource>,
) -> Option<String> {
    if let Some(why) = sync_type_rejection(sync_type) {
        return Some(why);
    }
    // "This parameter is required if the SyncType value is SyncToDestination"
    // (`CreateResourceDataSync`'s `S3Destination` doc), and the same for `SyncSource` and
    // `SyncFromSource`.
    if sync_type == SYNC_TYPE_TO_DESTINATION && s3_destination.is_none() {
        return Some(format!(
            "S3Destination is required if the SyncType value is {SYNC_TYPE_TO_DESTINATION}."
        ));
    }
    if sync_type == SYNC_TYPE_FROM_SOURCE && sync_source.is_none() {
        return Some(format!(
            "SyncSource is required if the SyncType value is {SYNC_TYPE_FROM_SOURCE}."
        ));
    }
    sync_source.and_then(source_rejection)
}

/// Why this sync source cannot be served, or `None` when it can: the value sets the
/// `SourceType` and `OrganizationSourceType` docs name, and the member each of them
/// requires.
pub fn source_rejection(source: &ResourceDataSyncSource) -> Option<String> {
    if !SOURCE_TYPES.contains(&source.source_type.as_str()) {
        return Some(format!(
            "The sync source type {} is not valid. Specify either \
             {SOURCE_TYPE_AWS_ORGANIZATIONS} or {SOURCE_TYPE_SINGLE_ACCOUNT_MULTI_REGIONS}.",
            source.source_type
        ));
    }
    if source.source_type != SOURCE_TYPE_AWS_ORGANIZATIONS {
        // Nothing beyond the source type is required of a source that aggregates one
        // account's Regions: `SourceRegions` is `@required` in the model, so the walk has
        // already refused a source without it.
        return None;
    }
    let Some(organizations) = source.aws_organizations_source.as_ref() else {
        return Some(format!(
            "AwsOrganizationsSource is required if the SourceType value is \
             {SOURCE_TYPE_AWS_ORGANIZATIONS}."
        ));
    };
    let organization_source_type = organizations.organization_source_type.as_str();
    if !ORGANIZATION_SOURCE_TYPES.contains(&organization_source_type) {
        return Some(format!(
            "The organization source type {organization_source_type} is not valid. Specify \
             either {ORGANIZATION_SOURCE_TYPE_ORGANIZATIONAL_UNITS} or \
             {ORGANIZATION_SOURCE_TYPE_ENTIRE_ORGANIZATION}."
        ));
    }
    // "For OrganizationalUnits, the data is aggregated from a set of organization units"
    // — so that type names them, and a source of that type without them aggregates
    // nothing.
    if organization_source_type == ORGANIZATION_SOURCE_TYPE_ORGANIZATIONAL_UNITS
        && organizations
            .organizational_units
            .as_ref()
            .is_none_or(|units| units.is_empty())
    {
        return Some(format!(
            "OrganizationalUnits is required if the OrganizationSourceType value is \
             {ORGANIZATION_SOURCE_TYPE_ORGANIZATIONAL_UNITS}."
        ));
    }
    None
}

/// One resource data sync configuration, as the sync listing reports it.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ResourceDataSyncRecord {
    /// "A name for the configuration", the key this record is held under within its
    /// Region.
    pub name: String,
    /// The Region whose endpoint created this sync, and whose data it aggregates.
    pub region: String,
    /// The sync's type: one of [`SYNC_TYPES`], defaulted by the create to
    /// [`SYNC_TYPE_TO_DESTINATION`].
    pub sync_type: String,
    /// "Amazon S3 configuration details for the sync", as the create supplied it;
    /// `None` for a sync that carries no destination.
    pub s3_destination: Option<ResourceDataSyncS3Destination>,
    /// "Information about the data sources to synchronize", as the create supplied it or
    /// an update replaced it; `None` for a sync that carries no source.
    pub sync_source: Option<ResourceDataSyncSource>,
    /// "The date and time the configuration was created (UTC)."
    pub created_time: SystemTime,
    /// "The date and time the resource data sync was changed." Equal to
    /// [`created_time`](Self::created_time) until an update changes the sync.
    pub last_modified_time: SystemTime,
}

impl ResourceDataSyncRecord {
    /// This sync, as `ListResourceDataSync` reports it.
    ///
    /// `LastStatus`, `LastSyncTime`, `LastSuccessfulSyncTime` and `LastSyncStatusMessage`
    /// are the outcome of a sync RUN — "The status reported by the last sync" — and
    /// nothing in this emulator aggregates Inventory or OpsData anywhere, so no sync has
    /// run and none of them is reported.
    pub fn to_item(&self) -> ResourceDataSyncItem {
        ResourceDataSyncItem {
            sync_name: Some(self.name.clone()),
            sync_type: Some(self.sync_type.clone()),
            sync_source: self.sync_source.as_ref().map(source_with_state),
            s3_destination: self.s3_destination.clone(),
            last_sync_time: None,
            last_successful_sync_time: None,
            sync_last_modified_time: Some(::aws_smithy_types::DateTime::from(
                self.last_modified_time,
            )),
            last_status: None,
            sync_created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            last_sync_status_message: None,
        }
    }

    /// Replace this sync's source with the one an update carries and stamp `now` as when
    /// the sync was last changed.
    ///
    /// `SyncSource` is the whole of what an update carries beyond the name and the type,
    /// and it is `@required` there, so an update replaces the source outright rather than
    /// merging members into it.
    pub fn apply_update(&mut self, sync_source: ResourceDataSyncSource, now: SystemTime) {
        self.sync_source = Some(sync_source);
        self.last_modified_time = now;
    }
}

/// A persisted sync source, as a read reports it.
///
/// `ResourceDataSyncSourceWithState` is the create's `ResourceDataSyncSource` plus a
/// `State`, and the four states the model names — `OrganizationNotExists`,
/// `NoPermissions`, `InvalidOrganizationalUnit`, `TrustedAccessDisabled` — are each a
/// reason the source cannot be read. A source this store holds is in none of them, so no
/// state is reported.
fn source_with_state(source: &ResourceDataSyncSource) -> ResourceDataSyncSourceWithState {
    ResourceDataSyncSourceWithState {
        source_type: Some(source.source_type.clone()),
        aws_organizations_source: source.aws_organizations_source.clone(),
        source_regions: Some(source.source_regions.clone()),
        include_future_regions: source.include_future_regions,
        state: None,
        enable_all_ops_data_sources: source.enable_all_ops_data_sources,
    }
}

impl super::ComAmazonawsSsmAmazonssmState {
    /// The sync `name` names in `region`, or `None` when that Region holds none — which
    /// is what the not-found refusal reports.
    pub fn resource_data_sync(&self, region: &str, name: &str) -> Option<&ResourceDataSyncRecord> {
        self.resource_data_syncs
            .get(&(region.to_string(), name.to_string()))
    }

    /// The same record, to apply an update to it.
    pub fn resource_data_sync_mut(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<&mut ResourceDataSyncRecord> {
        self.resource_data_syncs
            .get_mut(&(region.to_string(), name.to_string()))
    }

    /// Store `record` as a sync of its own Region, keyed by its name.
    pub fn insert_resource_data_sync(&mut self, record: ResourceDataSyncRecord) {
        let key = (record.region.clone(), record.name.clone());
        self.resource_data_syncs.insert(key, record);
    }

    /// Drop the sync `name` names in `region`, answering what was held.
    pub fn remove_resource_data_sync(
        &mut self,
        region: &str,
        name: &str,
    ) -> Option<ResourceDataSyncRecord> {
        self.resource_data_syncs
            .remove(&(region.to_string(), name.to_string()))
    }

    /// Every sync `region` holds, in ascending sync-name order, as `(name, record)`. The
    /// order is what makes a listing's resume cursor total: it is a sync name, not an
    /// offset into a list that may have changed between two pages.
    pub fn resource_data_syncs_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a ResourceDataSyncRecord)> + 'a {
        self.resource_data_syncs
            .iter()
            .filter(move |((held, _), _)| held == region)
            .map(|((_, name), record)| (name.as_str(), record))
    }

    /// Whether this store can take another sync: the emulator's own total guard
    /// ([`MAX_RESOURCE_DATA_SYNCS`]), measured across EVERY Region because the Region a
    /// sync is stored under comes from the request's own credential scope.
    pub fn can_hold_another_resource_data_sync(&self) -> bool {
        self.resource_data_syncs.len() < MAX_RESOURCE_DATA_SYNCS
    }
}
