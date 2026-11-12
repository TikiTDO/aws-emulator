// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediapackagevod-mediapackagevod service.
//!
//! Three resource maps, each keyed by a [`ResourceKey`] — the AWS Region the
//! resource was created in, then the resource's own `Id` (the identifier the REST
//! routes address it by) — plus ONE tag map keyed by resource ARN. Tags live only
//! in `tags_by_arn`: a resource record carries its ARN, and an ARN carries its
//! Region, so every operation — the creates here and the later `TagResource` /
//! `ListTagsForResource` slice — reads and writes the one copy rather than two that
//! can drift.
//!
//! MediaPackage VOD is a REGIONAL control plane (`aws.api#service` gives it the
//! `mediapackage-vod` endpoint prefix, arn namespace and SigV4 signing name), so
//! the Region a request is served in is part of the identity of every resource it
//! touches: the same `Id` in two Regions is two independent resources, and a
//! request served in one Region resolves, lists, mutates and deletes only the
//! records created in it. Every accessor on this struct therefore takes the
//! request's Region ([`request_region`]) beside the `Id`.
//!
//! `ApproximateAssetCount` and `EgressEndpoints` are never stored: they are
//! derived from these maps on every read ([`Self::asset_count`],
//! [`Self::egress_endpoints`]), so adding a packaging configuration or an asset
//! shows up in the next describe without rewriting any stored record. What an
//! endpoint's `Status` reports is the state of the packaging job that (asset,
//! configuration) pair started: the clock says whether that job is waiting, running or
//! done, and the asset's own [`AssetRecord::source_readability`] says how a job that is
//! done came out.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{RangeTrait, Violation, ViolationKind};

use crate::ops::types::{
    Authorization, CmafPackage, DashManifest, DashPackage, EgressAccessLogs, EgressEndpoint,
    HlsManifest, HlsPackage, MssPackage,
};

// Whether the SOURCE an asset was ingested with is one this service could read — the fact
// the TERMINAL packaging status of its egress endpoints turns on — lives in
// `state/ingest_source.rs`.
pub mod ingest_source;

// WHERE a packaging group's egress is announced — the address its `DomainName` names, and
// the root of every playback URL beneath it — lives in `state/egress_addressing.rs`.
pub mod egress_addressing;

pub use egress_addressing::{
    announced_egress_origin, packaging_group_domain_name, EGRESS_ORIGIN_PREFIX,
};
pub use ingest_source::SourceReadability;

/// The region an unsigned request is served in. A signed request names its own
/// region in the SigV4 credential scope; see [`request_region`].
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's SigV4 / ARN namespace, from the model's `aws.api#service`
/// (`arnNamespace: mediapackage-vod`).
pub const ARN_NAMESPACE: &str = "mediapackage-vod";

/// How long after both the asset and the packaging configuration exist the packaging
/// job the service owes has RUN, so the endpoint reports its terminal status —
/// [`EGRESS_STATUS_PLAYABLE`] for an asset whose source the ingest read, and
/// [`EGRESS_STATUS_FAILED`] for one it could not ([`SourceReadability`]).
///
/// AWS documents no duration for VOD ingest, so this span is CHOSEN: it is long
/// enough that a caller acting straight after `CreateAsset` still observes the
/// non-terminal statuses, and short enough not to slow a test down.
pub const INGEST_PLAYABLE_AFTER: Duration = Duration::from_millis(500);

/// How long after that same instant the packaging job has been PICKED UP, so the
/// endpoint reports [`EGRESS_STATUS_PROCESSING`] rather than [`EGRESS_STATUS_QUEUED`]:
/// the job is no longer waiting to run and has not finished running.
///
/// Chosen, like [`INGEST_PLAYABLE_AFTER`], and strictly inside it: a job is queued, then
/// processing, then done, so the two spans divide one ingest into its three states.
pub const INGEST_PROCESSING_AFTER: Duration = Duration::from_millis(250);

/// The egress-endpoint status of a packaging job that has not been picked up yet.
///
/// From `EgressEndpoint$Status`: "The current processing status of the asset used
/// for the packaging configuration. The status can be either QUEUED, PROCESSING,
/// PLAYABLE, or FAILED."
pub const EGRESS_STATUS_QUEUED: &str = "QUEUED";

/// The egress-endpoint status of a packaging job that is RUNNING: picked up, not
/// finished. Same `EgressEndpoint$Status` value list as [`EGRESS_STATUS_QUEUED`].
pub const EGRESS_STATUS_PROCESSING: &str = "PROCESSING";

/// The terminal egress-endpoint status for a successfully packaged asset. Same
/// `EgressEndpoint$Status` value list as [`EGRESS_STATUS_QUEUED`].
pub const EGRESS_STATUS_PLAYABLE: &str = "PLAYABLE";

/// The terminal egress-endpoint status of a packaging job that could not be carried
/// out: the asset's source was not readable ([`SourceReadability::Unreadable`]), so
/// nothing was packaged for this configuration and nothing ever will be. Same
/// `EgressEndpoint$Status` value list as [`EGRESS_STATUS_QUEUED`].
pub const EGRESS_STATUS_FAILED: &str = "FAILED";

/// The manifest file name a packaging configuration uses when its manifest block
/// names none. `HlsManifest$ManifestName` (and the Dash/Mss equivalents) is "An
/// optional string to include in the name of the manifest".
pub const DEFAULT_MANIFEST_NAME: &str = "index";

/// The ad-marker treatment an `HlsManifest` is packaged with when the request names
/// none: `AdMarkers` value `NONE`, which "will omit all SCTE-35 ad markers from the
/// output" (`HlsManifest$AdMarkers`) — the treatment of content carrying no markers.
pub const DEFAULT_AD_MARKERS: &str = "NONE";

/// The `EXT-X-PROGRAM-DATE-TIME` interval an `HlsManifest` is packaged with when the
/// request names none. `HlsManifest$ProgramDateTimeIntervalSeconds` is "The interval
/// (in seconds) between each EXT-X-PROGRAM-DATE-TIME tag inserted into manifests";
/// zero is the interval that inserts none.
pub const DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS: i32 = 0;

/// The segment-template form a `DashPackage` is packaged with when the request names
/// none, from `SegmentTemplateFormat`: "When set to NUMBER_WITH_TIMELINE, a full
/// timeline is presented in each SegmentTemplate, with $Number$ media URLs".
pub const DEFAULT_SEGMENT_TEMPLATE_FORMAT: &str = "NUMBER_WITH_TIMELINE";

/// The DASH profile a `DashManifest` is packaged with when the request names none,
/// from `Profile`: the value that enables no profile-specific output (the other,
/// `HBBTV_1_5`, "enables HbbTV 1.5 compliant output").
pub const DEFAULT_DASH_PROFILE: &str = "NONE";

/// The SCTE-marker source a `DashManifest` is packaged with when the request names
/// none, from `ScteMarkersSource`: "When set to SEGMENTS, the scte markers are sourced
/// from the segments of the ingested content."
pub const DEFAULT_SCTE_MARKERS_SOURCE: &str = "SEGMENTS";

/// The buffer a `DashManifest` advertises when the request names none —
/// `DashManifest$MinBufferTimeSeconds`, "Minimum duration (in seconds) that a player
/// will buffer media before starting the presentation".
///
/// The value is the one the service materialises, as the recorded create responses of
/// this service show; the model declares no `@default`.
pub const DEFAULT_MIN_BUFFER_TIME_SECONDS: i32 = 30;

/// The most PackagingGroups this emulator holds.
///
/// A CAPACITY BOUND, not an AWS-published quota: every create grows the in-process
/// store and the only removal path is an explicit delete, so without a ceiling a
/// caller looping creates with fresh ids grows this process without limit. Past the
/// bound a create answers the modelled `TooManyRequestsException` (HTTP 429), whose
/// own documentation is "The client has exceeded their resource or throttling
/// limits."
pub const MAX_PACKAGING_GROUPS: usize = 1_000;

/// The most PackagingConfigurations this emulator holds in total. Same kind of
/// capacity bound as [`MAX_PACKAGING_GROUPS`].
pub const MAX_PACKAGING_CONFIGURATIONS: usize = 1_000;

/// The most PackagingConfigurations this emulator holds in ONE PackagingGroup.
///
/// This also bounds the derived fan-out: `EgressEndpoints` is one entry per
/// configuration in the asset's group, so it caps both the work a single read does
/// and the size of the response it returns, however many assets are stored.
pub const MAX_PACKAGING_CONFIGURATIONS_PER_GROUP: usize = 100;

/// The most Assets this emulator holds. Same kind of capacity bound as
/// [`MAX_PACKAGING_GROUPS`].
pub const MAX_ASSETS: usize = 10_000;

/// The most tags one resource may carry, from AWS's universal resource-tagging
/// restrictions ("Maximum number of tags per resource - 50").
///
/// NOT a modelled constraint: `Tags` is a plain `map<__string,__string>` on every
/// request that carries one
/// (`smithy_query(query="shape",shape="CreatePackagingGroupRequest",model_path="services/service-com-amazonaws-mediapackagevod-mediapackagevod/smithy-model/com-amazonaws-mediapackagevod-mediapackagevod.json")`),
/// so the boundary validator admits any map at all and the SERVICE decides this.
pub const MAX_TAGS: usize = 50;

/// The shortest tag key AWS accepts, from the same restrictions ("Minimum key length
/// - 1 Unicode character in UTF-8"), counted in characters rather than bytes.
///
/// The empty string is therefore not a tag key at all: naming it — on a create or to
/// remove it — is a parameter violation rather than a request that tags or untags
/// nothing. The key's VALUE has no such minimum ("Values can be empty"), so only the
/// key is bounded below.
pub const MIN_TAG_KEY_CHARS: usize = 1;

/// The longest tag key AWS accepts, from the same restrictions ("Maximum key length
/// - 128 Unicode characters in UTF-8"), counted in characters rather than bytes.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The longest tag value AWS accepts, from the same restrictions ("Maximum value
/// length - 256 Unicode characters in UTF-8").
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The tag-key prefix AWS reserves for its own tags: "Do not use `aws:` ... as a
/// prefix for keys, as it is reserved for AWS use. You cannot edit or delete tag keys
/// with this prefix." A caller may therefore neither name such a key on a create, nor
/// remove one.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// How many characters of a refused tag key the refusal quotes, so a message is bounded
/// by this service rather than by the length of the key the caller sent.
const TAG_KEY_LABEL_CHARS: usize = 32;

/// The wire name of the member a caller's tag map arrives on — `tags` on every request
/// that carries one, the creates and `TagResource` alike — which is what a refusal about
/// that map is attributed to.
const TAGS_MEMBER: &str = "tags";

/// Length of the checksum prefix a pagination cursor carries, so a cursor this
/// service did not issue is detectable rather than merely malformed.
const PAGE_TOKEN_CHECK_LEN: usize = 8;

/// The listing name [`encode_page_token`] binds a `ListAssets` cursor to.
pub const LISTING_ASSETS: &str = "assets";

/// The listing name [`encode_page_token`] binds a `ListPackagingConfigurations`
/// cursor to.
pub const LISTING_PACKAGING_CONFIGURATIONS: &str = "packaging-configurations";

/// The listing name [`encode_page_token`] binds a `ListPackagingGroups` cursor to.
pub const LISTING_PACKAGING_GROUPS: &str = "packaging-groups";

/// The resource-type segment an Asset's ARN carries — the `assets` of
/// `arn:aws:mediapackage-vod:<region>:<account>:assets/<Id>`, which is the form
/// `CreateAsset` mints and every read reports.
pub const ARN_KIND_ASSETS: &str = "assets";

/// The resource-type segment a PackagingConfiguration's ARN carries; see
/// [`ARN_KIND_ASSETS`].
pub const ARN_KIND_PACKAGING_CONFIGURATIONS: &str = "packaging-configurations";

/// The resource-type segment a PackagingGroup's ARN carries; see
/// [`ARN_KIND_ASSETS`].
pub const ARN_KIND_PACKAGING_GROUPS: &str = "packaging-groups";

/// Every resource type of this service an ARN can name, as `(the resource-type
/// segment of that resource's ARN, the label a message about that resource names it
/// by)`.
///
/// The segments are the ones the creates mint an ARN with ([`resource_arn`]), and the
/// labels the per-type constants every message of this crate words a resource type
/// with — [`crate::scaffolded::ASSET_LABEL`],
/// [`crate::scaffolded::PACKAGING_CONFIGURATION_LABEL`] and
/// [`crate::scaffolded::PACKAGING_GROUP_LABEL`] — so an operation that resolves a
/// caller-supplied ARN words its refusal exactly as the operation that resolves the
/// same resource by `Id` does.
pub const ARN_RESOURCE_KINDS: [(&str, &str); 3] = [
    (ARN_KIND_ASSETS, crate::scaffolded::ASSET_LABEL),
    (
        ARN_KIND_PACKAGING_CONFIGURATIONS,
        crate::scaffolded::PACKAGING_CONFIGURATION_LABEL,
    ),
    (
        ARN_KIND_PACKAGING_GROUPS,
        crate::scaffolded::PACKAGING_GROUP_LABEL,
    ),
];

/// The smallest page a listing returns: the minimum of the `MaxResults` `range`
/// trait every paginated operation in this model shares (`min: 1, max: 1000`).
pub const MIN_PAGE_SIZE: usize = 1;

/// The largest page a listing returns, and the page size a request that names no
/// `MaxResults` gets: the maximum of that same `range` trait.
pub const MAX_PAGE_SIZE: usize = 1000;

/// How every resource map in this state is keyed: the AWS Region the resource was
/// created in, then the caller-chosen `Id` the REST routes address it by.
///
/// The Region comes FIRST so one Region's records are a contiguous range of the
/// `BTreeMap`, read in `Id` order ([`region_slice`]) \u2014 which is what lets a listing
/// page one Region's resources in one stable order. Two Regions holding the same
/// `Id` are two distinct keys, so the same `Id` can be created independently in
/// each, exactly as a regional AWS control plane allows.
pub type ResourceKey = (String, String);

/// The key `id` is stored under when the request that created it was served in
/// `region`.
pub fn resource_key(region: &str, id: &str) -> ResourceKey {
    (region.to_string(), id.to_string())
}

/// The records of ONE Region, in `Id` order.
///
/// A `BTreeMap` keyed by [`ResourceKey`] orders by Region first, so a Region's
/// records are the contiguous range that starts at its smallest possible key; the
/// scan stops at the first key naming another Region.
fn region_slice<'map, V>(
    map: &'map BTreeMap<ResourceKey, V>,
    region: &str,
) -> impl Iterator<Item = &'map V> {
    let region = region.to_string();
    map.range(resource_key(&region, "")..)
        .take_while(move |((stored_region, _), _)| *stored_region == region)
        .map(|(_, record)| record)
}

/// A stored PackagingGroup. `ApproximateAssetCount` is NOT a field: it is counted
/// from [`ComAmazonawsMediapackagevodMediapackagevodState::assets`] on each read.
#[derive(Debug, Clone)]
pub struct PackagingGroupRecord {
    /// The caller-chosen `Id` this group is addressed by.
    pub id: String,
    /// The Region this group was created in — the one its ARN carries, and the only
    /// one it resolves in. See [`ResourceKey`].
    pub region: String,
    /// `arn:aws:mediapackage-vod:<region>:<account>:packaging-groups/<Id>`.
    pub arn: String,
    /// The egress origin assets in this group are played back from, resolved ONCE when
    /// the group was created ([`announced_egress_origin`]) so every projection of the
    /// group announces the same address.
    pub domain_name: String,
    /// ISO-8601 instant this group was created, as returned by every read.
    pub created_at: String,
    /// CDN authorization credentials, exactly as supplied.
    pub authorization: Option<Authorization>,
    /// Egress access logging configuration, exactly as supplied.
    pub egress_access_logs: Option<EgressAccessLogs>,
}

/// A stored PackagingConfiguration. The package block is kept verbatim so a read
/// returns what was written.
#[derive(Debug, Clone)]
pub struct PackagingConfigurationRecord {
    /// The caller-chosen `Id` this configuration is addressed by.
    pub id: String,
    /// The Region this configuration was created in; see [`ResourceKey`].
    pub region: String,
    /// `arn:aws:mediapackage-vod:<region>:<account>:packaging-configurations/<Id>`.
    pub arn: String,
    /// ISO-8601 instant this configuration was created.
    pub created_at: String,
    /// The `Id` of the PackagingGroup this configuration packages for.
    pub packaging_group_id: String,
    /// Exactly one of these four is `Some`; whichever it is decides the playback
    /// manifest the egress endpoints expose.
    pub hls_package: Option<HlsPackage>,
    /// See [`Self::hls_package`].
    pub dash_package: Option<DashPackage>,
    /// See [`Self::hls_package`].
    pub cmaf_package: Option<CmafPackage>,
    /// See [`Self::hls_package`].
    pub mss_package: Option<MssPackage>,
    /// Monotonic instant this configuration was created, for the packaging job the
    /// clock owes each asset in the group.
    pub created_instant: Instant,
}

/// A stored Asset. `EgressEndpoints` is NOT a field: it is derived from the
/// packaging configurations attached to this asset's group on each read.
#[derive(Debug, Clone)]
pub struct AssetRecord {
    /// The caller-chosen `Id` this asset is addressed by.
    pub id: String,
    /// The Region this asset was ingested in; see [`ResourceKey`].
    pub region: String,
    /// `arn:aws:mediapackage-vod:<region>:<account>:assets/<Id>`.
    pub arn: String,
    /// ISO-8601 instant this asset was submitted for ingest.
    pub created_at: String,
    /// The `Id` of the PackagingGroup this asset was ingested into.
    pub packaging_group_id: String,
    /// The resource id to include in SPEKE key requests, exactly as supplied — and
    /// `None` when the caller supplied none, because the service reports this member
    /// only where the caller stated it.
    pub resource_id: Option<String>,
    /// ARN of the source object in S3.
    pub source_arn: String,
    /// The IAM role ARN used to read the source object.
    pub source_role_arn: String,
    /// What the ingest ESTABLISHED about this asset's source when it was submitted:
    /// whether the object `source_arn` names was one this service could read as the
    /// role `source_role_arn` names ([`ingest_source::source_readability`]).
    ///
    /// Asked once, by the submit, and kept here — the packaging job reads the source at
    /// the instant it runs, so every later read of this asset reports the outcome of
    /// THAT read rather than asking again. It is what decides the TERMINAL status of
    /// each of this asset's egress endpoints.
    pub source_readability: SourceReadability,
    /// Monotonic instant ingest started, for the packaging job the clock owes.
    pub created_instant: Instant,
}

/// Per-service state for com-amazonaws-mediapackagevod-mediapackagevod. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMediapackagevodMediapackagevodState {
    /// PackagingGroups by (Region, `Id`).
    pub packaging_groups: BTreeMap<ResourceKey, PackagingGroupRecord>,
    /// PackagingConfigurations by (Region, `Id`).
    pub packaging_configurations: BTreeMap<ResourceKey, PackagingConfigurationRecord>,
    /// Assets by (Region, `Id`). A `BTreeMap` so `ListAssets` pages one Region's
    /// assets in one stable `Id` order.
    pub assets: BTreeMap<ResourceKey, AssetRecord>,
    /// Every resource's tags, keyed by that resource's ARN — the single tag store.
    ///
    /// It holds at most one entry per stored resource (a create inserts one, a
    /// delete removes it), so the three capacity bounds above bound this map too.
    pub tags_by_arn: BTreeMap<String, BTreeMap<String, String>>,
}

impl ComAmazonawsMediapackagevodMediapackagevodState {
    /// The PackagingGroup `id` names IN `region`, for a reporting operation. `None`
    /// for an `Id` only another Region holds: that group does not exist here.
    pub fn packaging_group(&self, region: &str, id: &str) -> Option<&PackagingGroupRecord> {
        self.packaging_groups.get(&resource_key(region, id))
    }

    /// The PackagingGroup `id` names in `region`, for an operation the spec says
    /// mutates it (`UpdatePackagingGroup`, `ConfigureLogs`).
    pub fn packaging_group_mut(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<&mut PackagingGroupRecord> {
        self.packaging_groups.get_mut(&resource_key(region, id))
    }

    /// Whether `region` holds a PackagingGroup under `id`.
    pub fn has_packaging_group(&self, region: &str, id: &str) -> bool {
        self.packaging_groups.contains_key(&resource_key(region, id))
    }

    /// Store `record` as the PackagingGroup `id` of `region`.
    pub fn insert_packaging_group(&mut self, record: PackagingGroupRecord) {
        let key = resource_key(&record.region, &record.id);
        self.packaging_groups.insert(key, record);
    }

    /// Remove the PackagingGroup `id` names in `region`, returning what was stored.
    pub fn remove_packaging_group(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<PackagingGroupRecord> {
        self.packaging_groups.remove(&resource_key(region, id))
    }

    /// Every PackagingGroup stored in `region`, in `Id` order.
    pub fn packaging_groups_in(&self, region: &str) -> impl Iterator<Item = &PackagingGroupRecord> {
        region_slice(&self.packaging_groups, region)
    }

    /// The PackagingConfiguration `id` names in `region`.
    pub fn packaging_configuration(
        &self,
        region: &str,
        id: &str,
    ) -> Option<&PackagingConfigurationRecord> {
        self.packaging_configurations.get(&resource_key(region, id))
    }

    /// Whether `region` holds a PackagingConfiguration under `id`.
    pub fn has_packaging_configuration(&self, region: &str, id: &str) -> bool {
        self.packaging_configurations
            .contains_key(&resource_key(region, id))
    }

    /// Store `record` as the PackagingConfiguration `id` of `region`.
    pub fn insert_packaging_configuration(&mut self, record: PackagingConfigurationRecord) {
        let key = resource_key(&record.region, &record.id);
        self.packaging_configurations.insert(key, record);
    }

    /// Remove the PackagingConfiguration `id` names in `region`.
    pub fn remove_packaging_configuration(
        &mut self,
        region: &str,
        id: &str,
    ) -> Option<PackagingConfigurationRecord> {
        self.packaging_configurations.remove(&resource_key(region, id))
    }

    /// Every PackagingConfiguration stored in `region`, in `Id` order.
    pub fn packaging_configurations_in(
        &self,
        region: &str,
    ) -> impl Iterator<Item = &PackagingConfigurationRecord> {
        region_slice(&self.packaging_configurations, region)
    }

    /// The Asset `id` names in `region`.
    pub fn asset(&self, region: &str, id: &str) -> Option<&AssetRecord> {
        self.assets.get(&resource_key(region, id))
    }

    /// Whether `region` holds an Asset under `id`.
    pub fn has_asset(&self, region: &str, id: &str) -> bool {
        self.assets.contains_key(&resource_key(region, id))
    }

    /// Store `record` as the Asset `id` of `region`.
    pub fn insert_asset(&mut self, record: AssetRecord) {
        let key = resource_key(&record.region, &record.id);
        self.assets.insert(key, record);
    }

    /// Remove the Asset `id` names in `region`.
    pub fn remove_asset(&mut self, region: &str, id: &str) -> Option<AssetRecord> {
        self.assets.remove(&resource_key(region, id))
    }

    /// Every Asset stored in `region`, in `Id` order.
    pub fn assets_in(&self, region: &str) -> impl Iterator<Item = &AssetRecord> {
        region_slice(&self.assets, region)
    }

    /// Replace the tags stored for `arn`. An absent `Tags` member and an empty map
    /// are the same request: the resource carries no tags.
    ///
    /// Stores what it is given: the caller-supplied map is judged against AWS's
    /// tagging restrictions by [`check_tags`] before a handler reaches this, so a map
    /// that breaks them never gets here.
    pub fn set_tags(&mut self, arn: &str, tags: Option<&HashMap<String, String>>) {
        let stored: BTreeMap<String, String> = tags
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.tags_by_arn.insert(arn.to_string(), stored);
    }

    /// ADD `tags` to the map stored for `arn`, leaving every tag of that resource the
    /// map does not name in place.
    ///
    /// A key the resource does not carry yet becomes a new tag; a key it already
    /// carries takes the new value, since a tag key is unique per resource. Keys are
    /// compared verbatim — a key differing in case or in surrounding whitespace is a
    /// different key — and an empty map adds nothing, so the resource is left as it was.
    ///
    /// Stores what it is given: the caller-supplied map is judged against AWS's tagging
    /// restrictions by [`check_tags`] and [`check_tag_total`] before a handler reaches
    /// this, so a map that breaks them never gets here.
    pub fn merge_tags(&mut self, arn: &str, tags: &HashMap<String, String>) {
        if tags.is_empty() {
            // Nothing to add, so no row is created for a resource that had none.
            return;
        }
        let stored = self.tags_by_arn.entry(arn.to_string()).or_default();
        for (key, value) in tags {
            stored.insert(key.clone(), value.clone());
        }
    }

    /// How many tags the resource `arn` would carry once `added` is merged into its
    /// stored map — what [`check_tag_total`] judges against [`MAX_TAGS`].
    ///
    /// An added key the resource already carries REPLACES that tag rather than adding
    /// one, so it counts once; a resource with no stored map counts only the added keys.
    pub fn tag_count_after_adding(&self, arn: &str, added: &HashMap<String, String>) -> usize {
        let stored = self.tags_by_arn.get(arn);
        let fresh = added
            .keys()
            .filter(|key| stored.is_none_or(|stored| !stored.contains_key(key.as_str())))
            .count();
        stored.map_or(0, BTreeMap::len) + fresh
    }

    /// Forget the tags stored for `arn`, so a deleted resource leaves none behind.
    pub fn forget_tags(&mut self, arn: &str) {
        self.tags_by_arn.remove(arn);
    }

    /// Remove from the map stored for `arn` exactly the tags `keys` names, leaving
    /// every other tag of that resource in place.
    ///
    /// A key the resource does not carry removes nothing, so the request is
    /// idempotent: repeating it leaves the same tag map, and the resource itself is
    /// untouched either way. Keys are compared verbatim — a tag key differing in
    /// case or in surrounding whitespace is a different key.
    pub fn remove_tag_keys(&mut self, arn: &str, keys: &[String]) {
        let Some(stored) = self.tags_by_arn.get_mut(arn) else {
            return;
        };
        for key in keys {
            stored.remove(key);
        }
    }

    /// The tags stored for `arn`, in the modelled `Tags` map shape. A resource with
    /// no tags reports an empty map, which is what it was created with.
    pub fn tags_of(&self, arn: &str) -> HashMap<String, String> {
        self.tags_by_arn
            .get(arn)
            .map(|tags| {
                tags.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<HashMap<String, String>>()
            })
            .unwrap_or_default()
    }

    /// Whether this service holds, IN `region`, the resource the caller-supplied ARN
    /// `arn` names — the resource whose tags are stored under that very ARN.
    ///
    /// An ARN resolves here only when it is EXACTLY the ARN this service issued for a
    /// resource it still holds in `region`: the comparison is against
    /// [`resource_arn`], so an ARN differing in partition, account, Region or id, one
    /// naming a resource type this service has none of, and a value that is not an ARN
    /// at all all resolve to nothing. That is what makes a tag read Region-scoped like
    /// every other read of this regional control plane.
    ///
    /// The equality is also what fixes the resource form: [`arn_resource`] reads both
    /// ARN forms AWS writes (`<type>/<id>` and `<type>:<id>`), while this service
    /// issues only the slash form, so a colon-form value names no resource it ever
    /// minted and resolves to nothing here \u2014 deliberately, since the `ResourceArn`
    /// documentation is "You can get this from the response to any request to the
    /// resource".
    pub fn holds_arn_resource(&self, region: &str, arn: &str) -> bool {
        let Some((kind, _, id)) = arn_resource(arn) else {
            return false;
        };
        if arn != resource_arn(kind, region, id) {
            return false;
        }
        match kind {
            ARN_KIND_ASSETS => self.has_asset(region, id),
            ARN_KIND_PACKAGING_CONFIGURATIONS => self.has_packaging_configuration(region, id),
            ARN_KIND_PACKAGING_GROUPS => self.has_packaging_group(region, id),
            // `arn_resource` reports only the three kinds above.
            _ => false,
        }
    }

    /// How many assets are currently stored in `group_id` IN `region` —
    /// `ApproximateAssetCount`, counted rather than stored. An asset of the same
    /// group `Id` in another Region belongs to another group, so it is not counted.
    pub fn asset_count(&self, region: &str, group_id: &str) -> i32 {
        let count = self
            .assets_in(region)
            .filter(|asset| asset.packaging_group_id == group_id)
            .count();
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    /// How many packaging configurations are currently stored in `group_id` in
    /// `region`, which is also the length of the `EgressEndpoints` list every asset
    /// in that group derives. Bounded by
    /// [`MAX_PACKAGING_CONFIGURATIONS_PER_GROUP`].
    pub fn configuration_count(&self, region: &str, group_id: &str) -> usize {
        self.packaging_configurations_in(region)
            .filter(|cfg| cfg.packaging_group_id == group_id)
            .count()
    }

    /// One egress endpoint per packaging configuration attached to this asset's
    /// group, in configuration-`Id` order.
    ///
    /// Derived, never stored: a configuration added after the asset shows up here on
    /// the next read, which is how the real service behaves. Only the group and the
    /// configurations of the asset's OWN Region take part, since those are the only
    /// ones that exist where the asset does. `now` is the reading instant, so the
    /// packaging status the clock owes is materialised without writing anything — the
    /// TERMINAL one of which is read off the asset's own record
    /// ([`AssetRecord::source_readability`]) rather than from the clock.
    pub fn egress_endpoints(&self, asset: &AssetRecord, now: Instant) -> Vec<EgressEndpoint> {
        let Some(group) = self.packaging_group(&asset.region, &asset.packaging_group_id) else {
            return Vec::new();
        };
        self.packaging_configurations_in(&asset.region)
            .filter(|cfg| cfg.packaging_group_id == asset.packaging_group_id)
            .map(|cfg| EgressEndpoint {
                packaging_configuration_id: Some(cfg.id.clone()),
                status: Some(egress_status(asset, cfg, now).to_string()),
                url: Some(playback_url(&group.domain_name, asset, cfg)),
            })
            .collect()
    }
}

/// The processing status of one asset packaged by one configuration.
///
/// Packaging cannot start before BOTH the asset and the configuration exist, so the job
/// is queued from the later of the two, picked up [`INGEST_PROCESSING_AFTER`] past it and
/// finished [`INGEST_PLAYABLE_AFTER`] past it.
///
/// The TERMINAL status is the outcome of the job rather than another instant of the
/// clock: a job whose source the ingest could not read packaged nothing and never will,
/// so it ends [`EGRESS_STATUS_FAILED`]; only one that read its source ends
/// [`EGRESS_STATUS_PLAYABLE`]. See [`AssetRecord::source_readability`].
fn egress_status(
    asset: &AssetRecord,
    cfg: &PackagingConfigurationRecord,
    now: Instant,
) -> &'static str {
    let started = asset.created_instant.max(cfg.created_instant);
    let elapsed = now.saturating_duration_since(started);
    if elapsed < INGEST_PROCESSING_AFTER {
        EGRESS_STATUS_QUEUED
    } else if elapsed < INGEST_PLAYABLE_AFTER {
        EGRESS_STATUS_PROCESSING
    } else {
        match asset.source_readability {
            SourceReadability::Readable => EGRESS_STATUS_PLAYABLE,
            SourceReadability::Unreadable => EGRESS_STATUS_FAILED,
        }
    }
}

/// The parent-manifest URL one packaging configuration exposes one asset at.
///
/// It is rooted at the group's `DomainName` (which carries its own scheme — see
/// [`announced_egress_origin`]), the path segment is derived from the (group,
/// asset, configuration) triple so it is stable across reads and distinct per
/// endpoint, and the file name is the configuration's manifest name plus the
/// extension that configuration's packaging format uses.
pub fn playback_url(
    domain_name: &str,
    asset: &AssetRecord,
    cfg: &PackagingConfigurationRecord,
) -> String {
    let path = digest_hex(
        &format!(
            "{}/{}/{}",
            asset.packaging_group_id, asset.id, cfg.id
        ),
        32,
    );
    let (manifest_name, suffix) = manifest_name_and_suffix(cfg);
    format!("{domain_name}/out/v1/{path}/{manifest_name}.{suffix}")
}

/// The manifest name and file-name suffix a packaging configuration's format uses.
///
/// The suffix follows the packaging format, which is the package block the
/// configuration was created with: HLS and CMAF expose an HLS parent manifest
/// (`.m3u8`), DASH an MPD (`.mpd`), and Microsoft Smooth Streaming a server
/// manifest (`.ism/Manifest`). The name is the manifest block's own
/// `ManifestName`, or [`DEFAULT_MANIFEST_NAME`] when it names none.
///
/// A STORED configuration always carries at least one manifest configuration —
/// [`check_manifest_lists`] refuses a create whose package block carries none — so the
/// name read here is the first manifest's own, and the default stands in only for a
/// manifest that named none.
fn manifest_name_and_suffix(cfg: &PackagingConfigurationRecord) -> (String, &'static str) {
    if let Some(pkg) = &cfg.hls_package {
        let name = pkg.hls_manifests.first().and_then(|m| m.manifest_name.as_deref());
        return (manifest_name(name), "m3u8");
    }
    if let Some(pkg) = &cfg.cmaf_package {
        let name = pkg.hls_manifests.first().and_then(|m| m.manifest_name.as_deref());
        return (manifest_name(name), "m3u8");
    }
    if let Some(pkg) = &cfg.dash_package {
        let name = pkg.dash_manifests.first().and_then(|m| m.manifest_name.as_deref());
        return (manifest_name(name), "mpd");
    }
    if let Some(pkg) = &cfg.mss_package {
        let name = pkg.mss_manifests.first().and_then(|m| m.manifest_name.as_deref());
        return (manifest_name(name), "ism/Manifest");
    }
    // Unreachable through the API: `CreatePackagingConfiguration` refuses a request
    // that carries no package block. Kept total rather than panicking in a handler.
    (DEFAULT_MANIFEST_NAME.to_string(), "m3u8")
}

/// A manifest block's `ManifestName`, or [`DEFAULT_MANIFEST_NAME`].
fn manifest_name(name: Option<&str>) -> String {
    name.filter(|n| !n.is_empty())
        .unwrap_or(DEFAULT_MANIFEST_NAME)
        .to_string()
}

/// The package block a `CreatePackagingConfiguration` request stores: the one the
/// caller supplied, with every member the service materialises a default for filled
/// in.
///
/// The service does not store a packaging configuration "as sent": it resolves the
/// optional settings of the chosen packaging format to the values it will package
/// with, and every later read of the configuration reports those resolved values. The
/// defaults are therefore applied ONCE, here, on the way into state — so the create
/// response, a describe and a list all project the identical block.
///
/// `SegmentDurationSeconds`, the encryption blocks, `StreamSelection`,
/// `ManifestName` and `DashManifest$ManifestLayout` are deliberately left as sent:
/// the recorded responses of this service report them only when the request
/// supplied them, so materialising a value for them would invent one the service
/// does not report.
pub fn hls_package_with_defaults(mut package: HlsPackage) -> HlsPackage {
    package.include_dvb_subtitles.get_or_insert(false);
    package.use_audio_rendition_group.get_or_insert(false);
    for manifest in &mut package.hls_manifests {
        hls_manifest_apply_defaults(manifest);
    }
    package
}

/// See [`hls_package_with_defaults`]: the same resolution for a CMAF package, whose
/// manifests are the same `HlsManifest` shape.
pub fn cmaf_package_with_defaults(mut package: CmafPackage) -> CmafPackage {
    package
        .include_encoder_configuration_in_segments
        .get_or_insert(false);
    for manifest in &mut package.hls_manifests {
        hls_manifest_apply_defaults(manifest);
    }
    package
}

/// See [`hls_package_with_defaults`]: the same resolution for a DASH package.
pub fn dash_package_with_defaults(mut package: DashPackage) -> DashPackage {
    package
        .include_encoder_configuration_in_segments
        .get_or_insert(false);
    package.include_iframe_only_stream.get_or_insert(false);
    // `PeriodTriggers` is "a list of triggers that controls when the outgoing
    // [MPD] will be partitioned into multiple periods. If empty, the content will
    // not be partitioned" — so the resolved value of an unset list is the empty one,
    // reported rather than omitted.
    package.period_triggers.get_or_insert_with(Vec::new);
    package
        .segment_template_format
        .get_or_insert_with(|| DEFAULT_SEGMENT_TEMPLATE_FORMAT.to_string());
    for manifest in &mut package.dash_manifests {
        dash_manifest_apply_defaults(manifest);
    }
    package
}

/// See [`hls_package_with_defaults`]: a Microsoft Smooth package carries no member
/// the service materialises a default for — `MssPackage` and `MssManifest` declare
/// only `SegmentDurationSeconds`, `Encryption`, `ManifestName` and
/// `StreamSelection`, each reported only as sent — so the stored block is the block
/// supplied. The function exists so every packaging format is resolved through one
/// named seam rather than three plus a silent case.
pub fn mss_package_with_defaults(package: MssPackage) -> MssPackage {
    package
}

/// The defaults the service materialises on one HLS manifest configuration (used by
/// both the HLS and the CMAF packaging formats).
fn hls_manifest_apply_defaults(manifest: &mut HlsManifest) {
    manifest
        .ad_markers
        .get_or_insert_with(|| DEFAULT_AD_MARKERS.to_string());
    manifest.include_iframe_only_stream.get_or_insert(false);
    manifest
        .program_date_time_interval_seconds
        .get_or_insert(DEFAULT_PROGRAM_DATE_TIME_INTERVAL_SECONDS);
    manifest.repeat_ext_x_key.get_or_insert(false);
}

/// The defaults the service materialises on one DASH manifest configuration.
fn dash_manifest_apply_defaults(manifest: &mut DashManifest) {
    manifest
        .min_buffer_time_seconds
        .get_or_insert(DEFAULT_MIN_BUFFER_TIME_SECONDS);
    manifest
        .profile
        .get_or_insert_with(|| DEFAULT_DASH_PROFILE.to_string());
    manifest
        .scte_markers_source
        .get_or_insert_with(|| DEFAULT_SCTE_MARKERS_SOURCE.to_string());
}

/// The first `len` hex characters of the SHA-256 of `input`.
fn digest_hex(input: &str, len: usize) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(input.as_bytes());
    let mut hex = hex::encode(digest);
    hex.truncate(len);
    hex
}

/// The ARN of a resource of `kind` (the ARN's resource-type segment) named `id`.
pub fn resource_arn(kind: &str, region: &str, id: &str) -> String {
    engine::emulator_arn(ARN_NAMESPACE, region, &format!("{kind}/{id}"))
}

/// What a caller-supplied ARN of this service names: the [`ARN_RESOURCE_KINDS`] row
/// its resource-type segment matches — that segment and the label a message about
/// that resource type names it by — and the non-empty id after it.
///
/// `None` for a value that is not a well-formed resource ARN of this service
/// ([`::service_common::arn::parse_of`], this repository's one caller-supplied-ARN
/// parse), one naming a resource type this service has none of, and one carrying no
/// id — none of which names a resource whose tags could be stored.
pub fn arn_resource(arn: &str) -> Option<(&'static str, &'static str, &str)> {
    let parsed = ::service_common::arn::parse_of(arn, ARN_NAMESPACE)?;
    let (kind, label) = ARN_RESOURCE_KINDS
        .into_iter()
        .find(|(kind, _)| *kind == parsed.resource_type())?;
    let id = ::service_common::arn::resource_id_of_type(arn, kind)?;
    (!id.is_empty()).then_some((kind, label, id))
}

/// The message a `NotFoundException` about a caller-supplied `ResourceArn` carries
/// when this service holds no such resource.
///
/// An ARN that names one of this service's resource types is reported in the very
/// form [`crate::scaffolded::not_found_message`] words every other missing resource
/// of that type in — the resource label and the id the ARN asked for — so a client
/// parses one form whether it addressed the resource by `Id` or by ARN. One that
/// names no resource type of this service has no id to report, so it is named by the
/// ARN the request sent.
pub fn arn_resource_not_found_message(arn: &str) -> String {
    match arn_resource(arn) {
        Some((_, label, id)) => crate::scaffolded::not_found_message(label, id),
        None => format!("Resource with arn={arn} not found"),
    }
}

/// The region this request is served in: the SigV4 credential scope's region, or
/// [`DEFAULT_REGION`] when the request carries no scope.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The current wall-clock instant as an ISO-8601 `YYYY-MM-DDThh:mm:ssZ` string, the
/// form this model's `CreatedAt` members (plain `__string`) carry.
pub fn now_iso8601() -> String {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = i64::try_from(unix.as_secs()).unwrap_or(i64::MAX);
    aws_smithy_types::DateTime::from_secs(secs)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Encode a pagination cursor for `listing`: the id the page ended on, which the
/// next page of THAT listing resumes strictly after.
///
/// Opaque to the caller, and carrying a checksum over both the listing and the id it
/// names, so a cursor that is merely malformed — or one another listing of this
/// service issued — is REFUSED rather than silently resumed from. The checksum is an
/// unkeyed digest over public text, so it detects those two cases rather than
/// resisting a deliberately forged cursor; a resume point only picks where in this
/// same store the listing starts, so there is nothing a forged one reaches that the
/// unpaged listing does not already report. Decoded only by [`decode_page_token`].
pub fn encode_page_token(listing: &str, last_id: &str) -> String {
    format!(
        "{}{}",
        page_token_check(listing, last_id),
        hex::encode(last_id.as_bytes())
    )
}

/// Decode a cursor produced by [`encode_page_token`] for `listing`. `None` for
/// anything that is not a cursor this service issued for that listing — a malformed
/// one, one whose checksum does not match the id it carries, or one issued by a
/// different listing — which the caller answers as an invalid parameter.
pub fn decode_page_token(listing: &str, token: &str) -> Option<String> {
    // Both halves are ASCII hex when this service issued the token; the ASCII check
    // is also what makes the byte split below a safe char boundary.
    if !token.is_ascii() || token.len() < PAGE_TOKEN_CHECK_LEN {
        return None;
    }
    let (check, payload) = token.split_at(PAGE_TOKEN_CHECK_LEN);
    let last_id = String::from_utf8(hex::decode(payload).ok()?).ok()?;
    (page_token_check(listing, &last_id) == check).then_some(last_id)
}

/// The checksum prefix a cursor over `last_id` in `listing` carries.
fn page_token_check(listing: &str, last_id: &str) -> String {
    digest_hex(&format!("{listing}-cursor/{last_id}"), PAGE_TOKEN_CHECK_LEN)
}

/// The message the refusal of an unusable `NextToken` carries, for every listing of
/// this service.
///
/// FIXED text: the recorded refusal of a `nextToken` this service never issued
/// interpolates nothing from the request — it neither names the input member nor
/// echoes the offending token — so it is NOT composed by the per-member
/// `<member_name> <phrase>` form the other parameter refusals of this service use.
/// The per-attribute `errors` map still attributes it to `nextToken`.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "Invalid NextToken";

/// The id a `NextToken` names as the resume point of `listing`, or the refusal a
/// cursor this listing did not issue gets.
///
/// `None` for a request that carries no `NextToken`: the listing starts at its
/// first record.
pub fn resume_after(
    listing: &str,
    next_token: Option<&str>,
) -> Result<Option<String>, crate::scaffolded::UnprocessableEntity> {
    match next_token {
        None => Ok(None),
        Some(token) => decode_page_token(listing, token).map(Some).ok_or_else(|| {
            crate::scaffolded::UnprocessableEntity::member(
                "nextToken",
                "is not a cursor this service issued",
            )
            .with_message(INVALID_NEXT_TOKEN_MESSAGE)
        }),
    }
}

/// How many records one page of a listing carries, from the request's `MaxResults`.
///
/// `MaxResults` is modelled `range(min: 1, max: 1000)`, and a request outside that bound
/// is refused before any handler runs — in this same envelope, with this same phrase
/// ([`modelled_constraint_refusal`]). The bound is still enforced here so the handler
/// answers the same page size whoever calls it, and a request that names no `MaxResults`
/// gets the largest page the model allows.
pub fn page_size(
    max_results: Option<i32>,
) -> Result<usize, crate::scaffolded::UnprocessableEntity> {
    match max_results {
        None => Ok(MAX_PAGE_SIZE),
        Some(max) => match usize::try_from(max) {
            Ok(max) if (MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&max) => Ok(max),
            _ => Err(crate::scaffolded::UnprocessableEntity::member(
                PAGE_SIZE_QUERY,
                between_phrase(MIN_PAGE_SIZE, MAX_PAGE_SIZE),
            )),
        },
    }
}

/// The query parameter every listing of this service binds its page size to
/// (`smithy_query(query="operation",operation="ListAssets")`: `MaxResults` is
/// `@httpQuery("maxResults")`, as it is on both other listings).
pub const PAGE_SIZE_QUERY: &str = "maxResults";

/// The phrase a value outside a bound stated at BOTH ends is refused with.
///
/// ONE sentence whichever end was exceeded: a caller that sent too small a value and one
/// that sent too large a value broke the same published bound, and the bound is what they
/// have to act on.
pub fn between_phrase(min: impl std::fmt::Display, max: impl std::fmt::Display) -> String {
    format!("must be between {min} and {max}")
}

/// The phrase a member that carries NO value is refused with.
///
/// A member the request left out and one it sent empty fail the same presence rule, so
/// they are refused with the one phrase: this is the phrase the creates already state for
/// an `Id` sent as the empty string, and an absent `@required` member is the same claim
/// about the same member.
pub const NO_VALUE_PHRASE: &str = "must not be empty";

/// This service's refusal of a request that broke a rule the MODEL declares, built from
/// the violations the framework's own boundary walk found, or `None` when it found none.
///
/// The envelope is the one every other parameter violation of this service travels in —
/// HTTP 422 `UnprocessableEntityException` with `message` and the per-attribute `errors`
/// map — so a declared rule and a rule a handler checks itself are answered alike: the
/// caller reads one body whichever found the violation. Every violation of the request is
/// carried, keyed by the member's WIRE name, because a request's parameters are validated
/// together.
pub fn modelled_constraint_refusal(
    violations: &[Violation],
    input_schema: &Schema,
) -> Option<crate::scaffolded::UnprocessableEntity> {
    crate::scaffolded::UnprocessableEntity::members(
        violations
            .iter()
            .map(|violation| {
                (
                    wire_member_path(&violation.path, input_schema),
                    constraint_phrase(violation, input_schema),
                )
            })
            .collect(),
    )
}

/// The phrase one violated modelled constraint is stated with, worded as every other
/// refused member of this service is worded: the rule alone, with the member supplied by
/// the `errors` key the envelope states it under.
///
/// A numeric bound is stated as the member's whole published range when the model declares
/// both of its ends ([`between_phrase`]), rather than as only the end this value fell
/// outside, so the caller is told the bound to satisfy instead of the half it broke.
fn constraint_phrase(violation: &Violation, input_schema: &Schema) -> String {
    match &violation.kind {
        ViolationKind::MissingRequiredMember => NO_VALUE_PHRASE.to_string(),
        ViolationKind::MinValue(min) => declared_range(&violation.path, input_schema)
            .unwrap_or_else(|| format!("must be greater than or equal to {min}")),
        ViolationKind::MaxValue(max) => declared_range(&violation.path, input_schema)
            .unwrap_or_else(|| format!("must be less than or equal to {max}")),
        ViolationKind::MinLength(min) => {
            format!("must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => format!("must have length less than or equal to {max}"),
        ViolationKind::Pattern(pattern) => format!("must satisfy the pattern {pattern}"),
        ViolationKind::EnumValue(values) => format!("must be one of {}", values.join(", ")),
        ViolationKind::UniqueItems => "must not carry a repeated entry".to_string(),
        // `ViolationKind` is `#[non_exhaustive]`: a class the framework adds later says
        // what little can be said about it rather than going unreported.
        _ => "is not valid".to_string(),
    }
}

/// The whole bound the member at `path` publishes, when the model states a `@range` with
/// BOTH ends on it; `None` for a one-ended range and for a member whose bound this reader
/// cannot place (one inside a nested shape).
fn declared_range(path: &str, input_schema: &Schema) -> Option<String> {
    let range = input_member(path, input_schema)?
        .traits()?
        .get(&RangeTrait::TRAIT_ID)?
        .as_any()
        .downcast_ref::<RangeTrait>()
        .copied()?;
    match (range.min, range.max) {
        (Some(min), Some(max)) => Some(between_phrase(bound(min), bound(max))),
        _ => None,
    }
}

/// A `@range` bound as a phrase states it: an integral bound without a trailing `.0`, a
/// fractional one with its decimals — the rendering the framework gives the same bound
/// when it reports the violation itself.
fn bound(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// A violation path as the WIRE names its members: the top-level member under the name it
/// travels on the wire (its `@jsonName` for a body member, its `@httpQuery` for a query
/// parameter), with any path below it kept as the walk spelled it.
///
/// The walk names a member by the lowerCamel of its MODEL name, which for every member of
/// this model is also its wire name; reading the name off the model rather than assuming
/// that is what keeps the `errors` key the one a client keys its per-field rendering on.
fn wire_member_path(path: &str, input_schema: &Schema) -> String {
    let (head, below) = match path.split_once('.') {
        Some((head, below)) => (head, Some(below)),
        None => (path, None),
    };
    let head = input_member(path, input_schema)
        .map_or_else(|| head.to_string(), wire_member_name);
    match below {
        Some(below) => format!("{head}.{below}"),
        None => head,
    }
}

/// The top-level input member a violation path starts at, matched by the lowerCamel
/// spelling the constraint walk gives a path segment. `None` when no member of the input
/// shape has that name.
fn input_member<'a>(path: &str, input_schema: &'a Schema) -> Option<&'a Schema> {
    let segment = path.split('.').next()?;
    input_schema.members().iter().copied().find(|member| {
        member
            .member_name()
            .is_some_and(|name| lower_camel(name) == segment)
    })
}

/// The name `member` travels under on the wire: its `@jsonName` when the model declares
/// one, else the `@httpQuery` parameter it is bound to, else the lowerCamel of its member
/// name.
fn wire_member_name(member: &Schema) -> String {
    if let Some(json) = member.json_name() {
        return json.value().to_string();
    }
    if let Some(query) = member.http_query() {
        return query.value().to_string();
    }
    member.member_name().map(lower_camel).unwrap_or_default()
}

/// `name` with its first character lowercased — the spelling the constraint walk gives a
/// member's path segment (`MaxResults` -> `maxResults`).
fn lower_camel(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// What an input member this model DOCUMENTS as an ARN has to name.
///
/// Every such member is a plain `__string` with no `@pattern`
/// (`smithy_query(query="shape",shape="CreateAssetRequest")`,
/// `shape="Authorization"`, `shape="SpekeKeyProvider"`), so the boundary validator
/// admits any text at all and reading the ARN form is the handler's job. It has to be
/// read: the service ASSUMES the role and READS the object those members name, and a
/// value that is not an ARN of the documented service names neither — so the request
/// is a parameter violation (422, attributed to the member) rather than a resource
/// that happens to be missing.
///
/// The type, the rule and the constants below are the SHARED ones
/// ([`::service_common::arn::Requirement`]), so this service and every other that binds a
/// caller-supplied resource ARN read it against one requirement rather than against a copy
/// each. What stays here is which of THIS model's members are ARNs
/// ([`check_authorization_arns`], [`check_speke_role_arns`]) and the envelope the refusal
/// travels in.
pub type ArnRequirement = ::service_common::arn::Requirement;

/// An object in Amazon S3: `CreateAssetRequest$SourceArn` is "ARN of the source
/// object in S3".
pub const S3_OBJECT_ARN: ArnRequirement = ::service_common::arn::S3_OBJECT;

/// An IAM role: `CreateAssetRequest$SourceRoleArn` is "The IAM role ARN used to
/// access the source S3 bucket", `Authorization$SecretsRoleArn` "the IAM role that
/// allows MediaPackage to communicate with AWS Secrets Manager", and
/// `SpekeKeyProvider$RoleArn` "an IAM role that AWS Elemental MediaPackage will
/// assume when accessing the key provider service".
pub const IAM_ROLE_ARN: ArnRequirement = ::service_common::arn::IAM_ROLE;

/// A secret in AWS Secrets Manager: `Authorization$CdnIdentifierSecret` is "The
/// Amazon Resource Name (ARN) for the secret in AWS Secrets Manager that is used for
/// CDN authorization".
pub const SECRETS_MANAGER_SECRET_ARN: ArnRequirement =
    ::service_common::arn::SECRETS_MANAGER_SECRET;

/// Read every listed `(wire member path, value, requirement)` as an ARN, refusing the
/// request when any of them does not satisfy its requirement.
///
/// One refusal covers ALL of them — AWS validates a request's parameters together, and
/// the `errors` map is per-attribute — so a caller that sent two junk ARNs is told
/// about both rather than fixing one to discover the other. `Ok(())` when nothing is
/// violated; the member path is the WIRE spelling, dotted for a nested member
/// (`authorization.cdnIdentifierSecret`), which is what a client keys its per-field
/// rendering on.
pub fn check_arn_members(
    members: &[(&str, &str, ArnRequirement)],
) -> Result<(), crate::scaffolded::UnprocessableEntity> {
    let violations: Vec<(String, String)> = members
        .iter()
        .filter(|(_, value, requirement)| !requirement.admits(value))
        .map(|(member, _, requirement)| ((*member).to_string(), requirement.phrase()))
        .collect();
    match crate::scaffolded::UnprocessableEntity::members(violations) {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

/// A tag key as a refusal quotes it: quoted, and clipped to [`TAG_KEY_LABEL_CHARS`]
/// characters so the refusal names the key the caller sent without echoing a key of
/// any length back at them.
fn tag_key_label(key: &str) -> String {
    let clipped: String = key.chars().take(TAG_KEY_LABEL_CHARS).collect();
    if clipped.chars().count() < key.chars().count() {
        format!("{clipped:?}…")
    } else {
        format!("{clipped:?}")
    }
}

/// Every way the tag key `key` breaks AWS's tagging restrictions, as the phrases a
/// refusal attributes to the member the key arrived on.
///
/// Only the rules that hold for a key WHATEVER the caller does with it: its length —
/// bounded at both ends, [`MIN_TAG_KEY_CHARS`] to [`MAX_TAG_KEY_CHARS`] characters, so
/// the empty string is not a key — and the reserved [`RESERVED_TAG_KEY_PREFIX`] AWS
/// owns, which is why the same check covers naming a key on a create and naming one to
/// remove.
fn tag_key_violations(key: &str) -> Vec<String> {
    let mut phrases = Vec::new();
    let length = key.chars().count();
    if length < MIN_TAG_KEY_CHARS {
        phrases.push(format!(
            "key {} must be at least {MIN_TAG_KEY_CHARS} character, not {length}",
            tag_key_label(key)
        ));
    }
    if length > MAX_TAG_KEY_CHARS {
        phrases.push(format!(
            "key {} must be at most {MAX_TAG_KEY_CHARS} characters, not {length}",
            tag_key_label(key)
        ));
    }
    if key.starts_with(RESERVED_TAG_KEY_PREFIX) {
        phrases.push(format!(
            "key {} must not use the reserved `{RESERVED_TAG_KEY_PREFIX}` prefix, which \
             only Amazon Web Services may set",
            tag_key_label(key)
        ));
    }
    phrases
}

/// Refuse a list of tag keys a caller may not name — the `TagKeys` of `UntagResource`.
///
/// A reserved key is refused on NAMING it, not on writing it: a caller may no more
/// DELETE an `aws:` tag than create one, so the removal is refused rather than reported
/// as done. The same holds for a key no tag can have at all — one outside the
/// [`MIN_TAG_KEY_CHARS`]-to-[`MAX_TAG_KEY_CHARS`] bound, the empty string among them:
/// the request names something that is not a tag key, so it is refused rather than
/// answered as a removal that removed nothing. A WELL-FORMED key the resource does not
/// carry is NOT refused here — removing it is simply idempotent.
pub fn check_tag_keys(
    member: &str,
    keys: &[String],
) -> Result<(), crate::scaffolded::UnprocessableEntity> {
    let violations: Vec<(String, String)> = keys
        .iter()
        .flat_map(|key| {
            tag_key_violations(key)
                .into_iter()
                .map(|phrase| (member.to_string(), phrase))
        })
        .collect();
    match crate::scaffolded::UnprocessableEntity::members(violations) {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

/// Refuse a tag map that would leave a resource carrying more than [`MAX_TAGS`] tags.
///
/// The restriction is on the RESOURCE, not on one request, so `total` is how many tags
/// the resource ends up with: the whole map for a create, and the stored map merged with
/// the added one for `TagResource` ([`ComAmazonawsMediapackagevodMediapackagevodState::tag_count_after_adding`]).
/// One place decides it, so both operations refuse the same request the same way.
pub fn check_tag_total(total: usize) -> Result<(), crate::scaffolded::UnprocessableEntity> {
    if total > MAX_TAGS {
        return Err(crate::scaffolded::UnprocessableEntity::member(
            TAGS_MEMBER,
            format!("must carry at most {MAX_TAGS} tags, not {total}"),
        ));
    }
    Ok(())
}

/// Refuse a caller-supplied `Tags` map that breaks AWS's universal resource-tagging
/// restrictions: more than [`MAX_TAGS`] entries, a key shorter than
/// [`MIN_TAG_KEY_CHARS`] or longer than [`MAX_TAG_KEY_CHARS`], a value longer than
/// [`MAX_TAG_VALUE_CHARS`], or a key carrying the reserved
/// [`RESERVED_TAG_KEY_PREFIX`].
///
/// This is the ONE place the rules are decided, so every operation that accepts a tag
/// map judges one the same way, and it is called BEFORE anything is stored — a refused
/// request creates no resource and leaves no tag behind. An absent map carries no tag
/// and so violates nothing. Entries are judged in key order, so the refusal does not
/// depend on how the request's map happened to be hashed.
pub fn check_tags(
    tags: Option<&HashMap<String, String>>,
) -> Result<(), crate::scaffolded::UnprocessableEntity> {
    let Some(tags) = tags else { return Ok(()) };
    check_tag_total(tags.len())?;
    let mut entries: Vec<(&str, &str)> = tags
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    entries.sort_unstable();
    let mut violations: Vec<(String, String)> = Vec::new();
    for (key, value) in entries {
        for phrase in tag_key_violations(key) {
            violations.push((TAGS_MEMBER.to_string(), phrase));
        }
        let length = value.chars().count();
        if length > MAX_TAG_VALUE_CHARS {
            violations.push((
                TAGS_MEMBER.to_string(),
                format!(
                    "the value of key {} must be at most {MAX_TAG_VALUE_CHARS} characters, \
                     not {length}",
                    tag_key_label(key)
                ),
            ));
        }
    }
    match crate::scaffolded::UnprocessableEntity::members(violations) {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

/// Read the two ARN members of a CDN `Authorization` block, for the operations that
/// accept one (`CreatePackagingGroup`, `UpdatePackagingGroup`).
///
/// Both are required members of the block, so a request that carries the block at all
/// carries both; a request that carries no block has no ARN to read.
pub fn check_authorization_arns(
    authorization: &Authorization,
) -> Result<(), crate::scaffolded::UnprocessableEntity> {
    check_arn_members(&[
        (
            "authorization.cdnIdentifierSecret",
            authorization.cdn_identifier_secret.as_str(),
            SECRETS_MANAGER_SECRET_ARN,
        ),
        (
            "authorization.secretsRoleArn",
            authorization.secrets_role_arn.as_str(),
            IAM_ROLE_ARN,
        ),
    ])
}

// ---------------------------------------------------------------------------
// What the log-subscription member has to NAME
//
// `EgressAccessLogs$LogGroupName` ("Customize the log group name.") is an instruction to
// WRITE a packaging group's egress access logs INTO that CloudWatch Logs log group, so the
// value has to be a name CloudWatch Logs can actually hold: the empty string, a name past
// CloudWatch's ceiling and a name carrying a character outside its set each name no group
// this service could ever write to. Such a request is refused with
// `UnprocessableEntityException` (422, keyed by the member) before anything is stored,
// instead of being kept and reported by every later read as a live log subscription that
// collects nothing.
//
// The model targets the member at the bare `__string` and publishes no `@length` or
// `@pattern` (`smithy_query(query="shape",shape="EgressAccessLogs")` reports the single
// `LogGroupName` member, `jsonName: logGroupName`), so the framework's constraint walk
// cannot refuse any of it. What CloudWatch Logs can hold is the SHARED reading
// ([`::service_common::cloudwatch_logs`]), so this service and every other that binds a
// log group name judge a name against one rule; what lives here is which of this model's
// members name a log group, and the envelope the refusal travels in.
// ---------------------------------------------------------------------------

/// What CloudWatch Logs can hold, read from the SHARED rule rather than copied here: the
/// two published bounds, the predicate over a whole name, and the one sentence a refusal
/// states. Re-exported rather than wrapped, so this service's own suites read the bounds
/// off `state` while the rule itself has exactly one definition.
pub use ::service_common::cloudwatch_logs::{
    is_log_group_name, log_group_name_requirement, MAX_LOG_GROUP_NAME_CHARS,
    MIN_LOG_GROUP_NAME_CHARS,
};

/// The wire path of the egress access-log group member, which its refusal is keyed by
/// (`EgressAccessLogs` arrives as `@jsonName("egressAccessLogs")` and carries
/// `LogGroupName` as `@jsonName("logGroupName")`).
pub const EGRESS_LOG_GROUP_MEMBER: &str = "egressAccessLogs.logGroupName";

/// Refuse a log-subscription block that names a log group CloudWatch Logs could not hold.
///
/// Every operation that ACCEPTS the block judges it the same way — `CreatePackagingGroup`
/// and `ConfigureLogs` are the two this model binds it on — and the judgement happens
/// before anything is written, so a refused request leaves the addressed group's
/// subscription exactly as it was. A block that carries no `LogGroupName` names no group
/// and so violates nothing: it leaves the group unsubscribed.
pub fn check_egress_access_logs(
    logs: Option<&EgressAccessLogs>,
) -> Result<(), crate::scaffolded::UnprocessableEntity> {
    let Some(name) = logs.and_then(|logs| logs.log_group_name.as_deref()) else {
        return Ok(());
    };
    if is_log_group_name(name) {
        return Ok(());
    }
    Err(crate::scaffolded::UnprocessableEntity::member(
        EGRESS_LOG_GROUP_MEMBER,
        log_group_name_requirement(),
    ))
}

/// Read the `SpekeKeyProvider$RoleArn` of whichever package block a
/// `CreatePackagingConfiguration` request carries an `Encryption` block in.
///
/// The role lives at the same place under all four packaging formats, so the same
/// requirement is read under each one's own wire path; a format whose block names no
/// encryption names no role.
pub fn check_speke_role_arns(
    hls: Option<&HlsPackage>,
    dash: Option<&DashPackage>,
    cmaf: Option<&CmafPackage>,
    mss: Option<&MssPackage>,
) -> Result<(), crate::scaffolded::UnprocessableEntity> {
    let role_arns = [
        hls.and_then(|package| package.encryption.as_ref())
            .map(|encryption| ("hlsPackage", &encryption.speke_key_provider.role_arn)),
        dash.and_then(|package| package.encryption.as_ref())
            .map(|encryption| ("dashPackage", &encryption.speke_key_provider.role_arn)),
        cmaf.and_then(|package| package.encryption.as_ref())
            .map(|encryption| ("cmafPackage", &encryption.speke_key_provider.role_arn)),
        mss.and_then(|package| package.encryption.as_ref())
            .map(|encryption| ("mssPackage", &encryption.speke_key_provider.role_arn)),
    ];
    let paths: Vec<(String, &str)> = role_arns
        .into_iter()
        .flatten()
        .map(|(block, role_arn)| {
            (
                format!("{block}.encryption.spekeKeyProvider.roleArn"),
                role_arn.as_str(),
            )
        })
        .collect();
    let members: Vec<(&str, &str, ArnRequirement)> = paths
        .iter()
        .map(|(path, role_arn)| (path.as_str(), *role_arn, IAM_ROLE_ARN))
        .collect();
    check_arn_members(&members)
}

/// The constraint phrase a manifest list that carries no manifest configuration
/// violated, worded as a verb phrase about the member like every other phrase this
/// service attributes (see [`crate::scaffolded::UnprocessableEntity::member`]).
///
/// Published so a test asserts the phrase the handler enforces rather than a copy of
/// it.
pub const MANIFEST_LIST_REQUIREMENT: &str = "must carry at least one manifest configuration";

/// Read the manifest list of whichever package block a `CreatePackagingConfiguration`
/// request carries, and refuse one that carries no manifest configuration.
///
/// A packaging configuration IS its manifests: every packaging format's manifest list
/// (`HlsPackage$HlsManifests`, `CmafPackage$HlsManifests`, `DashPackage$DashManifests`,
/// `MssPackage$MssManifests`) is a `required` list of manifest configurations, so a
/// block carrying none describes no output manifest at all — there would be no parent
/// manifest for the `EgressEndpoint$Url` of an asset in the group ("The URL of the
/// parent manifest for the repackaged Asset") to point at. Each list is read under its
/// OWN block's wire path, the same way [`check_speke_role_arns`] reads the SPEKE role
/// under each format's path, so the refusal names the member the caller sent.
///
/// The list is a required member, so a request that OMITS it never reaches here: it is
/// refused in front of the operation, in this same envelope and attributed to this same
/// member, with the phrase this service states for a member carrying no value at all
/// ([`NO_VALUE_PHRASE`]). What reaches here is a request that sent the list and sent it
/// empty.
pub fn check_manifest_lists(
    hls: Option<&HlsPackage>,
    dash: Option<&DashPackage>,
    cmaf: Option<&CmafPackage>,
    mss: Option<&MssPackage>,
) -> Result<(), crate::scaffolded::UnprocessableEntity> {
    let lists = [
        hls.map(|package| ("hlsPackage.hlsManifests", package.hls_manifests.len())),
        dash.map(|package| ("dashPackage.dashManifests", package.dash_manifests.len())),
        cmaf.map(|package| ("cmafPackage.hlsManifests", package.hls_manifests.len())),
        mss.map(|package| ("mssPackage.mssManifests", package.mss_manifests.len())),
    ];
    let violations: Vec<(String, String)> = lists
        .into_iter()
        .flatten()
        .filter(|(_, manifests)| *manifests == 0)
        .map(|(member, _)| (member.to_string(), MANIFEST_LIST_REQUIREMENT.to_string()))
        .collect();
    match crate::scaffolded::UnprocessableEntity::members(violations) {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediapackagevodMediapackagevodState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediapackagevodMediapackagevodState>)
}

#[cfg(test)]
mod tests {
    //! The two halves of a refusal built from a violation the framework found: the phrase
    //! each constraint CLASS is stated with, and the member name the `errors` map keys it
    //! under. Both are driven over a table here because only two of the classes are
    //! reachable through this model's wire surface (an absent `@required` member and the
    //! `MaxResults` range) — the rest are what this service would say about a constraint
    //! the model does not declare today. The end-to-end envelope is pinned on the wire in
    //! `tests/all/modelled_constraint_refusals.rs`.

    use super::*;

    /// One violation as the framework's walk reports it: the member path it names, and
    /// what was violated. The value echo is left unset, because no phrase this service
    /// states quotes it.
    fn violation(path: &str, kind: ViolationKind) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: None,
            sensitive: false,
        }
    }

    /// Every constraint class is stated as one verb phrase about the member, and a numeric
    /// bound the model states at both ends is stated whole.
    #[test]
    fn every_constraint_class_is_stated_as_one_phrase_about_the_member() {
        let listing = crate::ops::types::LIST_ASSETS_INPUT_SCHEMA;
        // (case, the violation the framework found, the phrase this service states)
        let rows: [(&str, Violation, String); 9] = [
            (
                "an absent required member",
                violation("packagingGroupId", ViolationKind::MissingRequiredMember),
                NO_VALUE_PHRASE.to_string(),
            ),
            (
                "a value under the member's published range",
                violation("maxResults", ViolationKind::MinValue("1".to_string())),
                between_phrase(MIN_PAGE_SIZE, MAX_PAGE_SIZE),
            ),
            (
                "a value over the member's published range",
                violation("maxResults", ViolationKind::MaxValue("1000".to_string())),
                between_phrase(MIN_PAGE_SIZE, MAX_PAGE_SIZE),
            ),
            (
                "a minimum the model states alone",
                violation("nextToken", ViolationKind::MinValue("3".to_string())),
                "must be greater than or equal to 3".to_string(),
            ),
            (
                "a maximum the model states alone",
                violation("nextToken", ViolationKind::MaxValue("7".to_string())),
                "must be less than or equal to 7".to_string(),
            ),
            (
                "a length minimum",
                violation("nextToken", ViolationKind::MinLength(1)),
                "must have length greater than or equal to 1".to_string(),
            ),
            (
                "a length maximum",
                violation("nextToken", ViolationKind::MaxLength(256)),
                "must have length less than or equal to 256".to_string(),
            ),
            (
                "a pattern",
                violation("nextToken", ViolationKind::Pattern("^[a-z]+$".to_string())),
                "must satisfy the pattern ^[a-z]+$".to_string(),
            ),
            (
                "an enumerated value set",
                violation(
                    "nextToken",
                    ViolationKind::EnumValue(vec!["NONE".to_string(), "SEGMENTS".to_string()]),
                ),
                "must be one of NONE, SEGMENTS".to_string(),
            ),
        ];

        for (case, found, expected) in rows {
            let actual = constraint_phrase(&found, listing);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            // And the whole refusal states that one phrase twice: as the member's entry in
            // the `errors` map, and as the message's own attribution.
            let refusal = modelled_constraint_refusal(std::slice::from_ref(&found), listing)
                .unwrap_or_else(|| panic!("case {case:?}: a violation is a refusal"));
            assert_eq!(
                refusal.errors.get(&found.path),
                Some(&vec![expected.clone()]),
                "case {case:?}: expected the phrase attributed to {:?}, got {:?}",
                found.path,
                refusal.errors
            );
        }

        // Nothing violated is no refusal: a request that satisfies the model is not
        // refused by this path at all.
        let nothing = modelled_constraint_refusal(&[], listing);
        assert!(
            nothing.is_none(),
            "case no violations: expected None, got {nothing:?}"
        );
    }

    /// A violation path is keyed by the name its member travels under on the wire, at the
    /// top level and below it, and an unplaceable path is keyed as the walk spelled it.
    #[test]
    fn a_violation_is_keyed_by_the_members_wire_name() {
        let listing = crate::ops::types::LIST_ASSETS_INPUT_SCHEMA;
        let create = crate::ops::types::CREATE_PACKAGING_CONFIGURATION_INPUT_SCHEMA;
        // (case, the input shape the violation was found on, the walk's path, the key)
        let rows: [(&str, &Schema, &str, &str); 6] = [
            (
                "a query parameter",
                listing,
                PAGE_SIZE_QUERY,
                PAGE_SIZE_QUERY,
            ),
            ("another query parameter", listing, "nextToken", "nextToken"),
            ("a body member", create, "packagingGroupId", "packagingGroupId"),
            (
                "a member of a nested structure",
                create,
                "hlsPackage.hlsManifests",
                "hlsPackage.hlsManifests",
            ),
            (
                "an element of a nested list",
                create,
                "hlsPackage.hlsManifests.1.member.manifestName",
                "hlsPackage.hlsManifests.1.member.manifestName",
            ),
            (
                "a path naming no member of the shape",
                listing,
                "notAMember",
                "notAMember",
            ),
        ];

        for (case, schema, path, expected) in rows {
            let actual = wire_member_path(path, schema);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the key {expected:?}, got {actual:?}"
            );
        }
    }
}
