// SPDX-License-Identifier: CC0-1.0

//! The asset-model, asset, time-series and capture records of
//! [`super::ComAmazonawsIotsitewiseAwsiotsitewiseState`].

use super::*;

/// A time series (data stream) that no asset property owns.
///
/// IoT SiteWise models a time series as a resource in its own right, separate
/// from the asset property it may be attached to: `ListTimeSeries`'s
/// `timeSeriesType` is `ASSOCIATED` or `DISASSOCIATED`
/// (`smithy_query(query="shape", shape="ListTimeSeriesType")`), a
/// `TimeSeriesSummary` carries `assetId` and `propertyId` only when it is
/// associated, and `DeleteTimeSeries` — not the disassociate call — is what
/// forgets one ("If the time series isn't associated with an asset property,
/// specify the alias of the time series"). So detaching a series from its asset
/// property leaves THIS record behind: the stream keeps the identity, alias, data
/// type and creation date it had while associated, and its last-update instant
/// moves to the call that detached it. Alias-addressed ingestion creates one the
/// same way, with the data type of the value that created it.
#[derive(Debug, Clone)]
pub struct DisassociatedTimeSeriesRecord {
    /// The alias that identifies the stream, and the only way to address it once
    /// no `(assetId, propertyId)` pair names it.
    pub alias: String,
    /// The stream's own `TimeSeriesId`: a fresh `ID`-shaped value, inside the
    /// modelled `TimeSeriesId` bound of 36..73 characters.
    ///
    /// Minted rather than carried over from the pair that owned it, because the
    /// asset property the stream left still has a series of its own that its ids
    /// name ([`time_series_id`]): two series are two ids, so a detached stream
    /// cannot keep reporting the pair-derived one. Stored, so it is stable across
    /// every read of this stream.
    pub time_series_id: String,
    /// A `PropertyDataType` value: the data type of the stream's samples.
    pub data_type: String,
    /// Set only for a `STRUCT` stream, as `TimeSeriesSummary$dataTypeSpec` states.
    pub data_type_spec: Option<String>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The stream's samples, kept sorted ascending by timestamp exactly as the
    /// property's own series is. A time series IS the data stream, so the values
    /// written while it was associated travel with it rather than staying on the
    /// asset property, and come back to a property the alias is bound to again.
    pub values: Vec<AssetPropertyValue>,
}

/// Why [`ComAmazonawsIotsitewiseAwsiotsitewiseState::put_capture_segment`]
/// refused to store a segment. Each variant is ONE rule, so a producer handler
/// maps a refusal to one modelled error rather than re-deriving the checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureWriteRefusal {
    /// The store holds no workspace of that name — a capture source lives in a
    /// workspace.
    NoSuchWorkspace,
    /// The `TimeSeriesId` is outside [`MIN_TIME_SERIES_ID_LENGTH`]..=[`MAX_TIME_SERIES_ID_LENGTH`].
    TimeSeriesIdLength,
    /// The alias is outside [`MIN_PROPERTY_ALIAS_LENGTH`]..=[`MAX_PROPERTY_ALIAS_LENGTH`].
    AliasLength,
    /// The data type is not a value the modelled `VideoDataType` declares.
    UnmodelledDataType,
    /// The source already holds media of ANOTHER `VideoDataType`. The modelled
    /// enum declares exactly one value today, so this is what keeps the rule
    /// true if it ever grows.
    DataTypeMismatch,
    /// A bound of the span is not an instant the modelled `TimeInNanos` can
    /// express; see [`MIN_TIME_IN_SECONDS`].
    UnmodelledInstant,
    /// The span covers no time: its start is not before its end.
    EmptySpan,
    /// The source does not exist yet and the store already holds
    /// [`MAX_CAPTURE_SOURCES`].
    SourceLimit,
}

/// One persisted span of a capture source's video: the bytes a producer ingested
/// and the span of time they cover.
///
/// A segment is ATOMIC — this emulator stores the media exactly as it was
/// ingested and decodes nothing, so a read that overlaps part of a segment is
/// answered with the whole segment and reports the span the segment really
/// covers, which is what `GetCaptureData`'s own output members are for ("The
/// actual start time of the returned data").
#[derive(Debug, Clone)]
pub struct CaptureSegment {
    /// When the video in [`Self::data`] begins.
    pub start_time: TimeInNanos,
    /// When it ends. Always later than [`Self::start_time`]: the writer refuses a
    /// segment covering no time at all.
    pub end_time: TimeInNanos,
    /// The media bytes, exactly as the producer ingested them.
    pub data: Vec<u8>,
}

/// A capture source inside a workspace: the video stream `GetCaptureData` reads.
///
/// A capture source is a time series of a workspace — `GetCaptureData` names one
/// by its `timeSeriesId` or by the `propertyAlias` bound to it, the same two
/// identifying forms every other time-series operation of this service accepts —
/// so the record carries both identities plus the `VideoDataType` of the media it
/// holds, which a read reports rather than assuming.
///
/// The PRODUCER side (video an edge gateway publishes for the source, and the
/// bulk import of `mp4` files a `CreateBulkImportJob` asks for) is not
/// implemented in this emulator yet, so today the only writer of this log is
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::put_capture_segment`] — the one
/// entry point a producer path (or a test seeding a capture source) calls.
/// `GetCaptureData` itself never appends a segment and never creates a source.
///
/// A capture source IS one of its workspace's time series, so it is what the
/// workspace-scoped forms of `ListTimeSeries`, `DescribeTimeSeries` and
/// `DeleteTimeSeries` enumerate, report and remove
/// ([`Self::time_series_summary`],
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::workspace_time_series`]).
///
/// WHY this is its own registry rather than a field on the records
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::property_alias_index`] and
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::disassociated_time_series`]
/// resolve: a capture source is addressed INSIDE a workspace, and those streams
/// belong to no workspace at all — nothing that creates or ingests one records
/// one for it. Keying a source by `(workspaceName, timeSeriesId)` is what makes
/// one identifier free in every other workspace, and it stays correct when the
/// operations that ingest a workspace's streams arrive: a stream that gains a
/// workspace gains a capture source under that key, rather than a read having to
/// guess which workspace an account-wide stream belongs to.
#[derive(Debug, Clone)]
pub struct CaptureSourceRecord {
    /// The workspace that contains the source. A source is addressable only
    /// inside it, which is what makes one identifier free in every other
    /// workspace.
    pub workspace_name: String,
    /// The source's `TimeSeriesId`.
    pub time_series_id: String,
    /// The `propertyAlias` bound to the source, when one is.
    pub alias: Option<String>,
    /// A `VideoDataType` value: the type of the media the segments hold.
    pub data_type: String,
    /// When the write that created the source landed — the instant this time
    /// series came into being, which `timeSeriesCreationDate` reports.
    pub creation_date: SystemTime,
    /// When the last write this source accepted landed, which
    /// `timeSeriesLastUpdateDate` reports.
    pub last_update_date: SystemTime,
    /// The source's video, kept sorted ascending by start time and
    /// single-valued at a start time, exactly as a time series' samples are.
    pub segments: Vec<CaptureSegment>,
}

impl CaptureSourceRecord {
    /// This source as the time series it is: the one summary every time-series
    /// operation of this service reports a workspace's stream through, so a
    /// listing and a describe of one stream cannot disagree about it.
    ///
    /// No asset property owns a workspace's video stream — nothing binds one to
    /// an `(assetId, propertyId)` pair — so the summary carries neither id,
    /// which is what makes it `DISASSOCIATED` in a listing. Its
    /// `PropertyDataType` is [`DATA_TYPE_VIDEO`]: the media's own
    /// `VideoDataType` is finer-grained than that member can express and is
    /// what `GetCaptureData` reports instead. `dataTypeSpec` stays absent —
    /// `TimeSeriesSummary$dataTypeSpec` is "required for time series that have
    /// the `STRUCT` data type".
    pub fn time_series_summary(&self, region: &str) -> TimeSeriesSummary {
        TimeSeriesSummary {
            asset_id: None,
            property_id: None,
            alias: self.alias.clone(),
            data_type: DATA_TYPE_VIDEO.to_string(),
            data_type_spec: None,
            time_series_creation_date: to_date_time(self.creation_date),
            time_series_last_update_date: to_date_time(self.last_update_date),
            time_series_arn: time_series_arn(region, &self.time_series_id),
            time_series_id: self.time_series_id.clone(),
        }
    }
}

/// Which identifying form a time-series lookup took.
///
/// `DescribeTimeSeries` and `DeleteTimeSeries` accept the same two —
/// "specify the alias of the time series" or "the assetId and propertyId that
/// identifies the asset property" — whatever scope they name, and a lookup
/// scoped to a workspace resolves both through
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::workspace_series`], so neither
/// handler re-derives it.
#[derive(Debug, Clone, Copy)]
pub enum SeriesLookup<'a> {
    /// The series the `propertyAlias` bound to it names.
    Alias(&'a str),
    /// The series of the asset property the pair of `CustomID` labels names.
    Pair(&'a str, &'a str),
}

/// Why [`ComAmazonawsIotsitewiseAwsiotsitewiseState::workspace_series`]
/// resolved no time series, carrying the identifier the request supplied for the
/// resource that is missing. Each variant is ONE absence, so a handler maps it
/// to one modelled error and one message
/// ([`workspace_series_miss_message`]) rather than re-deriving the lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSeriesMiss {
    /// The store holds no workspace of that name.
    NoSuchWorkspace,
    /// No stored asset answers to the `assetId` label the variant carries.
    NoSuchAsset(String),
    /// The asset the first label names carries no property the second names.
    NoSuchProperty(String, String),
    /// The workspace holds no time series under the identifier the request
    /// supplied, which the variant carries: the alias as sent, or the
    /// [`time_series_id`] of the pair the request named.
    NoSuchSeries(String),
}

/// How a workspace-scoped time-series lookup states the absence it stopped at:
/// the missing resource named as the kind it is, echoing the identifier the
/// REQUEST supplied for it, through the one template every not-found path of
/// this service uses ([`not_found_message`]).
///
/// Shared by every operation that takes a `workspaceName`, so two of them cannot
/// state one store state differently.
pub fn workspace_series_miss_message(workspace_name: &str, miss: &WorkspaceSeriesMiss) -> String {
    match miss {
        WorkspaceSeriesMiss::NoSuchWorkspace => {
            not_found_message(RESOURCE_WORKSPACE, workspace_name)
        }
        WorkspaceSeriesMiss::NoSuchAsset(asset_label) => asset_not_found_message(asset_label),
        WorkspaceSeriesMiss::NoSuchProperty(asset_label, property_label) => {
            format!("The asset {asset_label} has no property {property_label}.")
        }
        WorkspaceSeriesMiss::NoSuchSeries(requested) => {
            not_found_message(RESOURCE_TIME_SERIES, requested)
        }
    }
}

/// A stored asset model: everything `DescribeAssetModel` reports, plus the
/// bookkeeping (`version`, `tags`, `active_at`) the other operations read.
///
/// The members below carry the model's LATEST version, and
/// [`Self::superseded_active`] the definition of its ACTIVE one for as long as
/// those are two different definitions — which is exactly while a write the
/// model already accepted is still settling.
#[derive(Debug, Clone)]
pub struct AssetModelRecord {
    pub id: String,
    pub external_id: Option<String>,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// An `AssetModelType` value: `ASSET_MODEL`, `COMPONENT_MODEL` or `INTERFACE`.
    pub model_type: String,
    pub properties: Vec<AssetModelProperty>,
    pub hierarchies: Vec<AssetModelHierarchy>,
    pub composite_models: Vec<AssetModelCompositeModel>,
    /// `assetModelCompositeModelId` -> the links the modelled
    /// `AssetModelCompositeModel` shape has no member for: the composite model
    /// it is nested inside, and the component model it reuses. Absent for a
    /// top-level, inline composite model, which is what every composite model
    /// defined by `CreateAssetModel` is.
    pub composite_model_links: HashMap<String, CompositeModelLink>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `AssetModelState` this model reports until [`Self::active_at`] falls
    /// due: [`STATE_CREATING`] for a freshly created model, [`STATE_UPDATING`]
    /// while a mutating call it already accepted settles.
    pub settling_state: String,
    pub version: u32,
    /// The definition of the model's ACTIVE version while the members above are
    /// NOT it: the one the write that is still settling superseded.
    ///
    /// Recorded by [`Self::begin_update`] and consulted only while the
    /// transition that write owes is still pending — from then on the stored
    /// definition IS the active one. `None` for a model no version of which has
    /// ever been active, which is one still being created.
    ///
    /// Exactly ONE definition, replaced rather than appended to, so no number
    /// of writes grows the record: a model carries its latest definition and at
    /// most its active one. Past the due instant the value kept here is stale
    /// and INERT — [`Self::version_view`] consults it only while the stored
    /// definition is not the active one — so nothing clears it on a read.
    pub superseded_active: Option<AssetModelDefinition>,
    pub tags: HashMap<String, String>,
}

/// One version of an asset model's definition: the members a write to the model
/// replaces, as of the write that replaced them.
///
/// An asset model is VERSIONED — the reads carry an `assetModelVersion`
/// selecting `LATEST` or `ACTIVE`, and the writes a `matchForVersionType`
/// targeting one of the same two — so an accepted write KEEPS the definition it
/// superseded for as long as that definition is still the active version,
/// rather than leaving the two selectors naming one definition.
#[derive(Debug, Clone)]
pub struct AssetModelDefinition {
    /// This version's number, as `assetModelVersion` reports it.
    pub version: u32,
    pub external_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub properties: Vec<AssetModelProperty>,
    pub hierarchies: Vec<AssetModelHierarchy>,
    pub composite_models: Vec<AssetModelCompositeModel>,
    pub composite_model_links: HashMap<String, CompositeModelLink>,
    /// When the call that wrote this version landed: what
    /// `assetModelLastUpdateDate` reports for it.
    pub last_update_date: SystemTime,
}

/// What a composite model is attached to, beside the asset model that holds it.
#[derive(Debug, Clone, Default)]
pub struct CompositeModelLink {
    /// The `assetModelCompositeModelId` of the composite model this one is
    /// nested inside, when it is not top-level.
    pub parent_id: Option<String>,
    /// The `assetModelId` of the `COMPONENT_MODEL` this composite model reuses,
    /// for a component-model-based composite model. An inline one has none.
    pub composed_asset_model_id: Option<String>,
}

impl AssetModelRecord {
    /// The model's status as of `now`: its [`Self::settling_state`] until the
    /// owed transition falls due, `ACTIVE` from then on. Derived, so a read
    /// never has to write.
    pub fn status(&self, now: SystemTime) -> AssetModelStatus {
        AssetModelStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// The model's `AssetModelState` as of `now`.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The definition this record carries as it stands — its LATEST version.
    ///
    /// Read BEFORE a mutating call applies its writes, so the definition that
    /// call supersedes can be handed to [`Self::begin_update`].
    pub fn definition(&self) -> AssetModelDefinition {
        AssetModelDefinition {
            version: self.version,
            external_id: self.external_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            properties: self.properties.clone(),
            hierarchies: self.hierarchies.clone(),
            composite_models: self.composite_models.clone(),
            composite_model_links: self.composite_model_links.clone(),
            last_update_date: self.last_update_date,
        }
    }

    /// This model as it read while it carried `definition`: the same resource —
    /// its id, ARN, type, creation date and tags — reporting that version's
    /// definition, number and last update date, in the `ACTIVE` state that
    /// version had reached.
    ///
    /// A superseded definition is only ever kept while it is the ACTIVE version
    /// (see [`Self::begin_update`]), so the view's own owed transition has long
    /// fallen due: its `active_at` is the instant the call that wrote it landed.
    ///
    /// Every member is listed, so only the members the version does not carry
    /// are cloned off `self` and the definition's own lists are cloned once.
    pub(super) fn as_of(&self, definition: &AssetModelDefinition) -> Self {
        Self {
            version: definition.version,
            external_id: definition.external_id.clone(),
            name: definition.name.clone(),
            description: definition.description.clone(),
            properties: definition.properties.clone(),
            hierarchies: definition.hierarchies.clone(),
            composite_models: definition.composite_models.clone(),
            composite_model_links: definition.composite_model_links.clone(),
            last_update_date: definition.last_update_date,
            settling_state: STATE_ACTIVE.to_string(),
            active_at: definition.last_update_date,
            superseded_active: None,
            // The members a version of the definition does not carry: this is
            // the same resource, read as an earlier version of itself.
            id: self.id.clone(),
            arn: self.arn.clone(),
            model_type: self.model_type.clone(),
            creation_date: self.creation_date,
            tags: self.tags.clone(),
        }
    }

    /// This model as the version the selector `version_type` names it as of
    /// `now` (`LATEST | ACTIVE`, defaulting to `LATEST`), or `None` when the
    /// model has no such version.
    ///
    /// `LATEST` is the stored definition. `ACTIVE` is that same definition once
    /// the transition the clock owes has fallen due, and the definition the
    /// still-settling write superseded until then; a model no version of which
    /// has ever been active — one still being created — has no active version
    /// at all. ONE place, so every operation carrying the selector, whether as
    /// `assetModelVersion` on a read or as `matchForVersionType` on a write's
    /// precondition, resolves it identically.
    ///
    /// Anything that is not `ACTIVE` is the latest version: the modelled
    /// `^(LATEST|ACTIVE)$` pattern, which the runtime refuses a request against
    /// before a handler runs, leaves `LATEST` and an absent selector as the
    /// only other values that reach here.
    pub fn version_view(
        &self,
        version_type: Option<&str>,
        now: SystemTime,
    ) -> Option<Cow<'_, Self>> {
        match version_type.unwrap_or(VERSION_TYPE_LATEST) {
            VERSION_TYPE_ACTIVE if self.state(now) != STATE_ACTIVE => self
                .superseded_active
                .as_ref()
                .map(|definition| Cow::Owned(self.as_of(definition))),
            _ => Some(Cow::Borrowed(self)),
        }
    }

    /// Record that a mutating call — a composite model added to this model, say
    /// — landed at `now`, superseding the definition `superseded`: the
    /// definition now stored carries a new version, and the model reports
    /// `UPDATING` until the transition that call owes falls due.
    ///
    /// `superseded` is the definition as it stood BEFORE this call's writes,
    /// taken with [`Self::definition`]. It becomes what a read scoped to the
    /// ACTIVE version is answered from until the new definition settles, since
    /// the version this call wrote is not active yet. A call that lands while an
    /// earlier one is STILL settling supersedes that pending version, so the
    /// active version is left where it is — and a model that has never been
    /// active still has none.
    ///
    /// The due instant is STORED, never slept on, exactly as
    /// [`AssetRecord::begin_update`] does it for an asset. A model that had not
    /// finished being created keeps reporting `CREATING`: the pending transition
    /// it already owed is the one that settles, and it is the same one.
    pub fn begin_update(&mut self, superseded: AssetModelDefinition, now: SystemTime) {
        if self.state(now) == STATE_ACTIVE {
            self.superseded_active = Some(superseded);
        }
        self.last_update_date = now;
        self.version += 1;
        if self.state(now) != STATE_CREATING {
            self.settling_state = STATE_UPDATING.to_string();
        }
        self.active_at = now + ACTIVATION_DELAY;
    }

    /// The composite model stored under `id`, if this model holds one.
    pub fn composite_model(&self, id: &str) -> Option<&AssetModelCompositeModel> {
        self.composite_models
            .iter()
            .find(|composite| composite.id.as_deref() == Some(id))
    }

    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to
    /// the `assetModelCompositeModelId` of a composite model of THIS model.
    pub fn resolve_composite_model_id(&self, label: &str) -> Option<String> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self
                .composite_models
                .iter()
                .find(|composite| composite.external_id.as_deref() == Some(external))
                .and_then(|composite| composite.id.clone()),
            None => self.composite_model(label).and_then(|composite| composite.id.clone()),
        }
    }

    /// The id of the composite model `id` is nested inside, if any.
    pub fn composite_model_parent(&self, id: &str) -> Option<&str> {
        self.composite_model_links
            .get(id)?
            .parent_id
            .as_deref()
    }

    /// The `COMPONENT_MODEL` asset model composite model `id` reuses, if it is
    /// component-model-based.
    pub fn composed_asset_model_id(&self, id: &str) -> Option<&str> {
        self.composite_model_links
            .get(id)?
            .composed_asset_model_id
            .as_deref()
    }

    /// The ids of the composite models nested DIRECTLY inside `id`.
    pub fn composite_model_children(&self, id: &str) -> Vec<String> {
        self.composite_models
            .iter()
            .filter_map(|composite| {
                let child = composite.id.as_deref()?;
                (self.composite_model_parent(child) == Some(id)).then(|| child.to_string())
            })
            .collect()
    }

    /// `id` and every composite model nested inside it, at any depth.
    ///
    /// Exactly the composite models that stop being reachable when `id` goes
    /// away: a nested composite model is addressed only through the chain of
    /// parents that contains it, so `DeleteAssetModelCompositeModel` removes the
    /// whole subtree rather than leaving records no path reaches. Each id is
    /// listed once, so a link that somehow pointed into a cycle cannot spin here.
    pub fn composite_model_subtree(&self, id: &str) -> Vec<String> {
        let mut subtree = vec![id.to_string()];
        let mut at = 0;
        while at < subtree.len() {
            for child in self.composite_model_children(&subtree[at]) {
                if !subtree.contains(&child) {
                    subtree.push(child);
                }
            }
            at += 1;
        }
        subtree
    }

    /// The ids of the composite models this model holds directly: the ones that
    /// are nested inside no other composite model.
    pub fn top_level_composite_model_ids(&self) -> Vec<String> {
        self.composite_models
            .iter()
            .filter_map(|composite| {
                let id = composite.id.as_deref()?;
                self.composite_model_parent(id)
                    .is_none()
                    .then(|| id.to_string())
            })
            .collect()
    }

    /// Resolve a `CustomID` label — a raw id or `externalId:<externalId>` — to
    /// the id of a property THIS model declares.
    ///
    /// The property-list counterpart of [`Self::resolve_composite_model_id`], so
    /// every operation that addresses a model property by label resolves it the
    /// same way.
    pub fn resolve_property_id(&self, label: &str) -> Option<String> {
        self.properties
            .iter()
            .find(|property| {
                matches_id_label(
                    property.id.as_deref(),
                    property.external_id.as_deref(),
                    label,
                )
            })
            .and_then(|property| property.id.clone())
    }

    /// The id of the property this model declares under `name`, if any.
    ///
    /// A property's name is the only identity two asset models can share — ids
    /// are minted per model — so this is how an interface's property is paired
    /// with the property of the model the interface is applied to.
    pub fn property_id_named(&self, name: &str) -> Option<String> {
        self.properties
            .iter()
            .find(|property| property.name == name)
            .and_then(|property| property.id.clone())
    }

    /// The id of the hierarchy this model declares under `name`, if any. The
    /// hierarchy counterpart of [`Self::property_id_named`].
    pub fn hierarchy_id_named(&self, name: &str) -> Option<String> {
        self.hierarchies
            .iter()
            .find(|hierarchy| hierarchy.name == name)
            .and_then(|hierarchy| hierarchy.id.clone())
    }

    /// Composite model `id` as the summary both `DescribeAssetModel` and
    /// `DescribeAssetModelCompositeModel` report it as, so the two views of one
    /// stored composite model cannot disagree. The path it carries is the one
    /// rooted at this asset model ([`Self::composite_model_path`]).
    pub fn composite_model_summary(&self, id: &str) -> Option<AssetModelCompositeModelSummary> {
        let composite = self.composite_model(id)?;
        Some(AssetModelCompositeModelSummary {
            id: id.to_string(),
            external_id: composite.external_id.clone(),
            name: composite.name.clone(),
            r#type: composite.r#type.clone(),
            description: composite.description.clone(),
            path: Some(self.composite_model_path(id)),
        })
    }
}

/// A stored interface relationship: an interface asset model applied to an
/// asset model, and the mappings that application resolved to.
///
/// The mappings are RESOLVED ONCE, by the call that applies the interface, and
/// stored: they pair ids of two different models, which nothing else in the
/// store relates, so a later read reports the pairing the application made
/// rather than re-deriving one from definitions that may have moved since. What
/// a later write to either definition does take away is a mapping whose half
/// that model no longer declares — see
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::prune_interface_relationship_mappings`]
/// — because such a mapping names nothing.
#[derive(Debug, Clone)]
pub struct InterfaceRelationshipRecord {
    /// The stored `assetModelId` of the model the interface is applied to.
    pub asset_model_id: String,
    /// The stored `assetModelId` of the interface asset model.
    pub interface_asset_model_id: String,
    /// Each property of the interface asset model, paired with the property of
    /// [`Self::asset_model_id`] it maps to.
    pub property_mappings: Vec<PropertyMapping>,
    /// Each hierarchy of the interface asset model that the applied-to model
    /// also declares, paired with that model's own hierarchy.
    pub hierarchy_mappings: Vec<HierarchyMapping>,
}

/// One property of an interface asset model that a property of ANOTHER asset
/// model answers for, under an interface relationship this store holds.
///
/// The pairing is the stored relationship's own, read rather than re-derived, so
/// every view of an applied interface reports the same two ids. Ordered, so a
/// caller reporting several of these reports them in one order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MappedInterfaceProperty {
    /// The stored `assetModelId` of the interface asset model that declares the
    /// property.
    pub interface_asset_model_id: String,
    /// The stored id of that interface asset model's property.
    pub interface_asset_model_property_id: String,
}

/// A stored asset. `properties` is materialised from the referenced model's
/// property definitions at create time, so property writes and reads resolve.
/// It is ONE flat list holding the asset's own (base) properties and the ones
/// each composite model of that model contributes, the latter carrying the
/// `path` WITHIN the asset that addresses them — the composite models they sit
/// inside, then the property; see
/// [`ComAmazonawsIotsitewiseAwsiotsitewiseState::materialise_asset_properties`].
/// The asset's own hop, which a read puts at the head of that path, is added by
/// [`AssetRecord::property_path`] rather than stored, because it differs per
/// asset and moves when the asset is renamed.
#[derive(Debug, Clone)]
pub struct AssetRecord {
    pub id: String,
    pub external_id: Option<String>,
    pub arn: String,
    pub name: String,
    pub asset_model_id: String,
    pub description: Option<String>,
    pub properties: Vec<AssetProperty>,
    pub hierarchies: Vec<AssetHierarchy>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `AssetState` this asset reports until [`Self::active_at`] falls due:
    /// [`STATE_CREATING`] for a freshly created asset, [`STATE_UPDATING`] while a
    /// mutating call it already accepted settles.
    pub settling_state: String,
    pub tags: HashMap<String, String>,
}

impl AssetRecord {
    /// Where in `properties` the property a `CustomID` label names sits — a raw
    /// property id, or `externalId:<externalId>` for one that carries an external
    /// id — if this asset holds it.
    pub fn property_position(&self, label: &str) -> Option<usize> {
        match label.strip_prefix(EXTERNAL_ID_PREFIX) {
            Some(external) => self
                .properties
                .iter()
                .position(|property| property.external_id.as_deref() == Some(external)),
            None => self.properties.iter().position(|property| property.id == label),
        }
    }

    /// EVERY place in `properties` the property a `CustomID` label names sits,
    /// in list order, empty when this asset holds no such property.
    ///
    /// A property id names ONE property of an asset — one definition, one time
    /// series ([`AssetRecord::time_series_properties`]) and one alias binding —
    /// but the flat list carries one INSTANCE of it per composite model that
    /// contributes it, because two composite models may reuse the same component
    /// model and each instance sits under its own path. A write to the property
    /// therefore lands on every instance of it, so no two views of one asset can
    /// report different aliases, notification states or units for one property
    /// id; a read answers from the first ([`AssetRecord::property`]).
    pub fn property_positions(&self, label: &str) -> Vec<usize> {
        let Some(first) = self.property_position(label) else {
            return Vec::new();
        };
        let id = self.properties[first].id.as_str();
        self.properties
            .iter()
            .enumerate()
            .filter(|(_, property)| property.id == id)
            .map(|(at, _)| at)
            .collect()
    }

    /// The asset property `label` names, resolved the way every `CustomID` member
    /// is — the same lookup as [`AssetRecord::property_position`], for a caller
    /// that wants the property rather than where it sits.
    ///
    /// `DescribeTimeSeries` models its `propertyId` as `CustomID`, so both forms
    /// reach a handler and both must name the same stored property.
    pub fn property(&self, label: &str) -> Option<&AssetProperty> {
        self.properties.get(self.property_position(label)?)
    }

    /// The asset's own (BASE) properties, as a read reports them.
    ///
    /// The ones materialised from its model's top-level property definitions:
    /// what `DescribeAsset` reports in `assetProperties`, which "doesn't include
    /// properties that you define in composite models", and the ones
    /// `ListAssetProperties` enumerates under its default `BASE` filter. Each is
    /// projected through [`Self::reported_property`], so it carries the path that
    /// addresses it from the root of the asset and its notification block.
    pub fn base_properties(&self) -> Vec<AssetProperty> {
        self.properties
            .iter()
            .filter(|property| composite_model_of_property(property).is_none())
            .map(|property| self.reported_property(property))
            .collect()
    }

    /// The properties this asset's instance of composite model `composite_id`
    /// carries, in the order they were materialised, as a read reports them (see
    /// [`Self::base_properties`]).
    pub fn composite_properties(&self, composite_id: &str) -> Vec<AssetProperty> {
        self.properties
            .iter()
            .filter(|property| composite_model_of_property(property) == Some(composite_id))
            .map(|property| self.reported_property(property))
            .collect()
    }

    /// This asset's properties, ONE per distinct property id: what addresses a
    /// time series of this asset.
    ///
    /// Two composite models of one asset model may reuse the SAME component
    /// model, so the asset carries that component's definition once per
    /// composite model that contributes it; the instances share the definition's
    /// id, hence the one series that id addresses. Every enumeration of an
    /// asset's time series goes through here, so a shared definition is not
    /// listed as two streams.
    pub fn time_series_properties(&self) -> Vec<&AssetProperty> {
        let mut seen: HashSet<&str> = HashSet::new();
        self.properties
            .iter()
            .filter(|property| seen.insert(property.id.as_str()))
            .collect()
    }

    /// The asset's status as of `now`; see [`AssetModelRecord::status`].
    pub fn status(&self, now: SystemTime) -> AssetStatus {
        AssetStatus {
            state: self.state(now).to_string(),
            error: None,
        }
    }

    /// The hierarchy of this asset a `CustomID` label names: a raw hierarchy id,
    /// or `externalId:<externalId>` resolved against the hierarchy's external id.
    ///
    /// The same two label forms every other id member of this service accepts, so
    /// a hierarchy is addressable the way the model says ids are.
    pub fn hierarchy(&self, label: &str) -> Option<&AssetHierarchy> {
        self.hierarchies.iter().find(|hierarchy| {
            matches_id_label(hierarchy.id.as_deref(), hierarchy.external_id.as_deref(), label)
        })
    }

    /// The asset's `AssetState` as of `now`: its [`Self::settling_state`] until
    /// the owed transition falls due, `ACTIVE` from then on. Derived, so a read
    /// never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// Record that a mutating call landed at `now`: the asset reports `UPDATING`
    /// until the transition that call owes falls due, and `ACTIVE` after it.
    ///
    /// The due instant is STORED, never slept on, so the caller's own call
    /// returns immediately and the transient state is still what a read taken
    /// straight afterwards observes.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.last_update_date = now;
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
    }

    /// This asset as a `ListAssets` summary, as of `now`.
    pub fn summary(&self, now: SystemTime) -> AssetSummary {
        AssetSummary {
            id: self.id.clone(),
            external_id: self.external_id.clone(),
            arn: self.arn.clone(),
            name: self.name.clone(),
            asset_model_id: self.asset_model_id.clone(),
            creation_date: to_date_time(self.creation_date),
            last_update_date: to_date_time(self.last_update_date),
            status: self.status(now),
            hierarchies: self.hierarchies.clone(),
            description: self.description.clone(),
        }
    }
}
