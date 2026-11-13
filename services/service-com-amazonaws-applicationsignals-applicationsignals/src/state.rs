// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-applicationsignals-applicationsignals service.
//!
//! The service level objective (SLO) is this service's primary resource, so the store is
//! the set of SLOs this emulator holds plus the tags attached to each one by ARN. The
//! derivation helpers below live here, beside the store, because the SLO lifecycle
//! operations (`CreateServiceLevelObjective`, `GetServiceLevelObjective`,
//! `ListServiceLevelObjectives`, `UpdateServiceLevelObjective` and
//! `DeleteServiceLevelObjective`) must all agree on identity, on the ARN they mint and on
//! the server-derived members of a stored record — two copies of that logic would let a
//! Get disagree with the Create that wrote the record.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::Duration;

use ::aws_smithy_types::DateTime;
// The calendar arithmetic a calendar-month evaluation interval is stepped by.
use ::chrono::Datelike;

use crate::ops::types::{
    ConflictException, DependencyConfig, ExclusionWindow, Goal, GroupingAttributeDefinition,
    GroupingConfiguration, Interval, Metric, MetricDataQuery, MetricSource, MetricStat,
    MonitoredRequestCountMetricDataQueries,
    RequestBasedServiceLevelIndicator, RequestBasedServiceLevelIndicatorConfig,
    RequestBasedServiceLevelIndicatorMetric, ResourceNotFoundException, RollingInterval,
    ServiceLevelIndicator, ServiceLevelIndicatorConfig, ServiceLevelIndicatorMetric,
    ServiceLevelIndicatorMetricConfig, ServiceLevelObjective, ServiceLevelObjectiveSummary, Tag,
};
// The shapes the time-ranged topology reads project, on their own `use` so this set and the
// SLO lifecycle's above are edited on separate lines.
use crate::ops::types::{
    AttributeFilter, ChangeEvent, Dimension, MetricReference, Service, ServiceDependency,
    ServiceDependent, ServiceGroup, ServiceOperation, ServiceState, ServiceSummary,
};
// The dynamic instrumentation shapes, on their own `use` for the same reason.
use crate::ops::types::{
    CaptureConfiguration, CaptureLimitsConfig, CodeCaptureConfiguration, CodeLocation,
    InstrumentationConfiguration, InstrumentationConfigurationWithoutServiceEnv,
    InstrumentationStatusEvent, Location, LocationIdentifier,
};
// The shapes the audit read projects, on their own `use` for the same reason.
use crate::ops::types::{
    AuditFinding, AuditTarget, AuditTargetEntity, AuditorResult, MetricGraph, ServiceEntity,
};
// `ResourceNotFoundException` as the instrumentation-configuration operations serialize it:
// the same shape, with the members those refusals do not put on the wire declared
// unsupported (`crate::ops::types::instrumentation_not_found_types`). Imported under a
// second name because the shape's own name is already taken here by the copy the SLO and
// tagging refusals serialize, which DOES carry those members.
use crate::ops::types::instrumentation_not_found_types::ResourceNotFoundException as InstrumentationNotFound;

/// This service's SigV4 signing name, which is also its ARN namespace
/// (`OPERATIONS.md`: `sigv4 signing name: application-signals`, `ARN namespace:
/// application-signals`). The modelled `ServiceLevelObjectiveArn` pattern
/// (`^arn:(aws|aws-us-gov):application-signals:[^:]*:[^:]*:slo/...$`) carries the same
/// namespace.
pub const SIGNING_NAME: &str = "application-signals";

/// The resource-type segment of an SLO ARN, from the modelled `ServiceLevelObjectiveArn`
/// pattern (`:slo/<name>`), which spells that segment in lower case.
pub const SLO_ARN_RESOURCE: &str = "slo";

/// The `ResourceType` an SLO failure reports, and the type token its message names: the
/// resource's TYPE NAME, which is capitalised — not the lower-case segment an ARN spells
/// (`SLO_ARN_RESOURCE`). The two are different strings for one resource kind, and a client
/// comparing `ResourceType` is comparing against the type name.
pub const SLO_RESOURCE_TYPE: &str = "Slo";

/// Region for a minted ARN when the request carries no SigV4 credential scope to read one
/// from; the emulator's documented default region (`AGENTS.md`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `EvaluationType` for an SLO configured with `SliConfig`
/// (`smithy_query(shape="EvaluationType")` -> `["PeriodBased", "RequestBased"]`).
pub const EVALUATION_TYPE_PERIOD_BASED: &str = "PeriodBased";

/// `EvaluationType` for an SLO configured with `RequestBasedSliConfig`.
pub const EVALUATION_TYPE_REQUEST_BASED: &str = "RequestBased";

/// The largest page `ListServiceLevelObjectives` returns when the caller names no
/// `MaxResults`: the modelled maximum of `ListServiceLevelObjectivesMaxResults`
/// (`range 1..50`), since the member declares no default.
pub const MAX_SLO_PAGE: i32 = 50;

/// The largest page `ListServiceStates` serves: the modelled maximum of
/// `ListServiceStatesMaxResults` (`smithy_query(query="shape",
/// shape="ListServiceStatesMaxResults")` -> `range ..250`), which is also the modelled
/// maximum length of the `ServiceStates` list a page is reported in.
pub const MAX_SERVICE_STATES_PAGE: i32 = 250;

/// The smallest page a caller of any list of this service may ask for.
///
/// A page bound is a COUNT of items, so one is the smallest that names a page at all; it is
/// also the minimum every `MaxResults` member of this service that states one states
/// (`range 1..N`), so enforcing it where a member leaves it unstated keeps one bound across
/// the service's lists rather than one per operation.
const MIN_PAGE_BOUND: i32 = 1;

/// The most SLOs this service holds, after which a create is refused with the modelled
/// `ServiceQuotaExceededException`.
///
/// The real service enforces a per-account, per-Region quota on service level objectives
/// (CloudWatch Application Signals, "Service level objectives" — a soft quota of 2,000),
/// and the create operation models `ServiceQuotaExceededException` precisely for it. The
/// cap is also what keeps the store bounded: the record count is otherwise chosen by the
/// caller, since nothing else in the create path evicts or expires a record.
pub const MAX_SLOS: usize = 2_000;

/// The most tags a resource whose API documents a tag maximum may carry.
///
/// The maximum is documented per resource, not by the list shape: an SLO's `Tags` member
/// reads "You can associate as many as 50 tags with an SLO", and `TagResource` states the
/// same for the CloudWatch resource it addresses — "You can associate as many as 50 tags
/// with a CloudWatch resource". The modelled `TagList` bound (`length 0..200`) is wider than
/// that, so the boundary validator admits a list the service itself refuses and the count is
/// checked where the tags are attached ([`tag_count_refusal`] for a create, which starts from
/// no tags, and [`max_tags_of`] where tags are added to a resource that already carries
/// some). A resource whose own API
/// documents no such maximum — the dynamic instrumentation configuration, whose `Tags` doc
/// states only that the list is optional — is bounded by the modelled `TagList` instead
/// ([`MAX_TAGS_PER_LIST`]).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most tags a resource whose API documents NO tag maximum may carry: the modelled
/// `TagList` maximum (`smithy_query(query="shape",shape="TagList")` -> `length 0..200`),
/// which is the only bound stated for such a resource.
///
/// The boundary validator already holds one REQUEST's list to it; this is the same bound on
/// the resource's resulting set, which an attachment grows beyond one request's list.
pub const MAX_TAGS_PER_LIST: usize = 200;

/// The largest page `ListAuditFindings` returns when the caller names no `MaxResults`.
/// Model doc for the member: "The maximum number of audit findings to return in one
/// operation. If you omit this parameter, the default of 10 is used."
pub const DEFAULT_AUDIT_FINDING_PAGE: i32 = 10;

/// The page `ListServices` serves a caller who names no `MaxResults`: the `@default` the
/// member's own model carries (`smithy_query(query="operation",operation="ListServices")` ->
/// `MaxResults {default: 50, range 1..100}`), which its documentation states too ("If you
/// omit this parameter, the default of 50 is used").
pub const DEFAULT_SERVICE_PAGE: i32 = 50;

/// The page `ListServiceOperations` serves a caller who names no `MaxResults`: the `@default`
/// the member's own model carries (`smithy_query(query="operation",
/// operation="ListServiceOperations")` -> `MaxResults {default: 20, range 1..100}`), which is
/// the value the request walk fills an absent member in with.
pub const DEFAULT_SERVICE_OPERATION_PAGE: i32 = 20;

/// The page `ListServiceDependencies` serves a caller who names no `MaxResults`: the
/// `@default` the member's own model carries (`smithy_query(query="operation",
/// operation="ListServiceDependencies")` -> `MaxResults {default: 20, range 1..100}`).
pub const DEFAULT_SERVICE_DEPENDENCY_PAGE: i32 = 20;

/// The page `ListServiceDependents` serves a caller who names no `MaxResults`: the `@default`
/// the member's own model carries (`smithy_query(query="operation",
/// operation="ListServiceDependents")` -> `MaxResults {default: 20, range 1..100}`).
pub const DEFAULT_SERVICE_DEPENDENT_PAGE: i32 = 20;

/// The page `ListEntityEvents` serves a caller who names no `MaxResults`: the `@default` the
/// member's own model carries (`smithy_query(query="operation",operation="ListEntityEvents")`
/// -> `MaxResults {default: 20, range 1..250}`).
pub const DEFAULT_ENTITY_EVENT_PAGE: i32 = 20;

/// `Goal.Interval` when omitted. Model doc: "If you omit this parameter, a rolling
/// interval of 7 days is used."
const DEFAULT_ROLLING_INTERVAL_DURATION: i32 = 7;

/// The `DurationUnit` of that default rolling interval
/// (`smithy_query(shape="DurationUnit")` -> `["MINUTE", "HOUR", "DAY", "MONTH"]`).
const DEFAULT_ROLLING_INTERVAL_UNIT: &str = "DAY";

/// `Goal.AttainmentGoal` when omitted. Model doc: "If you omit this parameter, 99 is used
/// to represent 99% as the attainment goal."
const DEFAULT_ATTAINMENT_GOAL: f64 = 99.0;

/// `Goal.WarningThreshold` when omitted. Model doc: "If you omit this parameter, the
/// default of 50.0 is used."
const DEFAULT_WARNING_THRESHOLD: f64 = 50.0;

/// `ServiceLevelIndicatorConfig.MetricThreshold`'s modelled `@default` of `0`.
const DEFAULT_SLI_METRIC_THRESHOLD: f64 = 0.0;

/// `ServiceLevelIndicatorConfig.ComparisonOperator`'s modelled `@default` of `"LessThan"`.
const DEFAULT_SLI_COMPARISON_OPERATOR: &str = "LessThan";

/// The `Id` of the one `MetricDataQuery` an SLI config's own metric description is
/// materialised into ([`materialised_sli_metric_data_queries`]).
///
/// `MetricDataQuery.Id` is `@required` and its doc states the rules an id must meet: it
/// "must be unique within a `MetricDataQueries` array" and "the valid characters are
/// letters, numbers, and underscore. The first character must be a lowercase letter." A
/// materialised list holds exactly ONE query, so a single fixed id that starts with a
/// lowercase letter satisfies both.
const MATERIALISED_SLI_QUERY_ID: &str = "m1";

/// The `Period` of a materialised query when the config names a statistic but no
/// `PeriodSeconds`.
///
/// `MetricStat.Period` is `@required` while the config's `PeriodSeconds` is optional, so one
/// period has to be reported. It is the minimum of the modelled `SLIPeriodSeconds`
/// (`smithy_query(shape="ServiceLevelIndicatorMetricConfig")`, member `PeriodSeconds`:
/// `range 60..900`) — the shortest period an SLO evaluation may use — which is also the
/// shortest period `MetricStat.Period` documents for a regular-resolution CloudWatch metric
/// ("a period can be as short as one minute (60 seconds) and must be a multiple of 60").
const MIN_SLI_PERIOD_SECONDS: i32 = 60;

/// The marker every `NextToken` the objective listing issues carries, so a well-formed token
/// this service did not issue is refused instead of being read as a cursor. The `/` cannot
/// occur in an SLO name (the modelled `ServiceLevelObjectiveName` pattern admits only
/// `[-._0-9A-Za-z ]`), so the prefix can never be mistaken for part of the cursor.
///
/// The listing reports the stored objectives the REQUEST selects, so its tokens are issued
/// under this marker plus the fingerprint of that selection ([`slo_selection_token_tag`]).
const NEXT_TOKEN_TAG: &str = "slos/1/";

/// The [`NEXT_TOKEN_TAG`] of each time-ranged topology read. One tag per operation, so a
/// token one of them issued is not read as a cursor by another: the offsets they count are
/// into different projections, and honouring a foreign one would silently skip entries.
pub const SERVICE_OPERATIONS_TOKEN_TAG: &str = "service-operations/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServiceDependencies`.
pub const SERVICE_DEPENDENCIES_TOKEN_TAG: &str = "service-dependencies/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServiceDependents`.
pub const SERVICE_DEPENDENTS_TOKEN_TAG: &str = "service-dependents/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListAuditFindings`.
pub const AUDIT_FINDINGS_TOKEN_TAG: &str = "audit-findings/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServices`.
pub const SERVICES_TOKEN_TAG: &str = "services/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListEntityEvents`.
pub const ENTITY_EVENTS_TOKEN_TAG: &str = "entity-events/1/";
/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServiceStates`.
pub const SERVICE_STATES_TOKEN_TAG: &str = "service-states/1/";

/// Per-service state for com-amazonaws-applicationsignals-applicationsignals. A handler reaches it through [`service_state`].
///
/// `slos` is keyed by SLO NAME: a name is unique within an account and region (the
/// `ConflictException` a repeated create answers is exactly that uniqueness), and the
/// ordered map gives `ListServiceLevelObjectives` a deterministic order to page over.
///
/// The key is the name ALONE, not `(region, name)`, because this store is one account's and
/// one Region's: a process holds a single engine state, every ARN it mints carries
/// `engine::emulator_account_id()`, and `ListServiceLevelObjectives` reports nothing for any
/// other account. The region in a minted ARN is therefore a rendering of the caller's SigV4
/// scope rather than part of an SLO's identity, which is also why [`slo_name_of`] resolves
/// an ARN by its name segment and ignores the region and partition around it. The ACCOUNT
/// field is NOT such a rendering: it says which account's resource an ARN names, so an ARN
/// naming any other account names no record here ([`names_other_account`]).
#[derive(Debug, Default)]
pub struct ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// Every SLO created in this emulator, keyed by `ServiceLevelObjective::name`. Bounded
    /// at [`MAX_SLOS`] entries by the create path, which is the only writer.
    pub slos: BTreeMap<String, ServiceLevelObjective>,
    /// Tags per resource ARN, the key the service's tagging operations address a resource
    /// by (`TagResource`/`UntagResource`/`ListTagsForResource` take a `ResourceArn`). One
    /// entry per tagged resource of either kind this service holds, so
    /// [`MAX_SLOS`] + [`MAX_INSTRUMENTATION_CONFIGURATIONS`] bounds this map too; the entry
    /// is dropped once the resource holds no tags (see [`Self::remove_tags`]) or the resource
    /// itself is gone.
    pub tags: BTreeMap<String, BTreeMap<String, String>>,
    /// Every dynamic instrumentation configuration this emulator holds, grouped by the
    /// [`InstrumentationScope`] a `ListInstrumentationConfigurations` call names.
    ///
    /// The grouping is the one the API itself imposes: every instrumentation operation takes
    /// `Service`, `Environment` and `InstrumentationType` together (the type "determines
    /// which backing store to query"), and a list reports exactly one scope's set. Within a
    /// scope the records are keyed by [`instrumentation_key`] — signal type and location
    /// hash — and the ordered map gives the list a deterministic order to page over.
    ///
    /// A configuration's IDENTITY is narrower than its place in this map: the service, the
    /// environment, the signal type and the location, WITHOUT the instrumentation type (see
    /// [`Self::instrumentation_configuration_at_location`]) — so at most one scope of a
    /// given service and environment holds a record under any one [`instrumentation_key`].
    pub instrumentation_configurations: BTreeMap<InstrumentationScope, InstrumentationConfigurationSet>,
    /// When a configuration was last removed from a location the store has removed one from,
    /// keyed by the [`InstrumentationScope`] it lived in and the [`instrumentation_key`] it
    /// was stored under.
    ///
    /// A READ of a location this store once held a configuration for answers differently from
    /// a read of one it never held ([`read_instrumentation_configuration_not_found`]), so the
    /// removal has to be remembered after the record and even after the scope's set are gone —
    /// which is why this is its own map rather than a field of
    /// [`InstrumentationConfigurationSet`], whose entry is dropped once it has emptied and
    /// stopped being news ([`EMPTY_SCOPE_RETENTION`]). Both removal paths write it, the
    /// explicit delete and the lapse the clock makes due, because both leave the location
    /// holding a configuration it once held.
    ///
    /// A create at the location clears the entry: the record it writes is a NEW resource, not
    /// a removed one. Bounded at [`MAX_REMEMBERED_INSTRUMENTATION_REMOVALS`] entries, past
    /// which the OLDEST removal is forgotten, so a caller that creates and deletes for ever
    /// does not grow the store for ever.
    removed_instrumentation_configurations:
        BTreeMap<InstrumentationScope, BTreeMap<String, DateTime>>,
    /// The next instant [`ComAmazonawsApplicationsignalsApplicationsignalsState::expire_instrumentation_configurations`]
    /// has anything to do, or `None` while it owes nothing — so the pass every create and
    /// list makes walks the store only when the clock has actually made a lapse or a scope
    /// drop due.
    instrumentation_due_at: Option<DateTime>,
    /// The exclusion windows configured on each SLO, keyed by the same SLO NAME `slos` is
    /// keyed by, so the two maps name one resource and a window set cannot outlive the SLO
    /// it belongs to (see [`Self::remove`]).
    ///
    /// The windows of one SLO are an ORDERED SET identified by configuration: a removal
    /// names a window by its configuration (`RemoveExclusionWindows`: "The window
    /// configuration must match an existing exclusion window"), so two windows with the same
    /// configuration would be one window named twice. Bounded at
    /// [`MAX_EXCLUSION_WINDOWS_PER_SLO`] per SLO by the only writer, and the entry is dropped
    /// once an SLO holds no windows — holding none and holding an empty set are the same
    /// state, exactly as for `tags`.
    ///
    /// Within one SLO the windows are keyed by the ORDINAL they were added under
    /// ([`next_exclusion_window_ordinal`]), so the ordered map iterates them in the order they
    /// were added and each window keeps its key for as long as it is stored: an ordinal is
    /// never reused and never moves when another window is removed, which is what makes a
    /// `ListServiceLevelObjectiveExclusionWindows` page cursor a stable boundary over a set the
    /// batch writer can change between two pages
    /// ([`Self::page_exclusion_windows`]) — the same property the
    /// instrumentation list gets from keying its own stored set
    /// ([`Self::page_instrumentation_configurations`]).
    ///
    /// Every STORED window carries a concrete `StartTime`, whether or not the addition stated
    /// one ([`stored_exclusion_window`]), so the set a read reports is a set of windows that
    /// each start at a known instant.
    pub exclusion_windows: BTreeMap<String, BTreeMap<u64, ExclusionWindow>>,
    /// This account's grouping configuration, or `None` while none is configured.
    ///
    /// At most ONE configuration exists: `PutGroupingConfiguration` "creates or updates the
    /// grouping configuration for this account", so a second put REPLACES the first rather
    /// than adding a record, and `DeleteGroupingConfiguration` "deletes the grouping
    /// configuration for this account", which returns the store to `None`. The single slot
    /// is also why this is an `Option` rather than a map: like `slos`, this store is one
    /// account's and one Region's.
    pub grouping_configuration: Option<GroupingConfiguration>,
    /// This account's Application Signals enablement, or `None` while discovery has never
    /// been started.
    ///
    /// At most ONE record exists, for the same reason `grouping_configuration` holds one:
    /// this store is one account's. `StartDiscovery` "enables this Amazon Web Services
    /// account to be able to use CloudWatch Application Signals by creating the
    /// AWSServiceRoleForCloudWatchApplicationSignals service-linked role", and an account
    /// that already holds that role needs no second one — so the record is written by the
    /// FIRST call and left exactly as it is by every later one
    /// ([`Self::start_discovery`]).
    discovery: Option<Discovery>,
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// The stored SLO an `Id` names, accepting either form the modelled
    /// `ServiceLevelObjectiveId` pattern allows: a bare name or a full `slo/<name>` ARN.
    /// Which record each form names is [`slo_name_of`]'s rule, so an `Id` naming another
    /// account names nothing here.
    pub fn resolve(&self, id: &str) -> Option<&ServiceLevelObjective> {
        self.slos.get(slo_name_of(id)?)
    }

    /// [`Self::resolve`] for a mutating caller.
    pub fn resolve_mut(&mut self, id: &str) -> Option<&mut ServiceLevelObjective> {
        self.slos.get_mut(slo_name_of(id)?)
    }

    /// Remove the SLO an `Id` names, along with its tags, and hand back the removed
    /// record. `None` when the `Id` names no SLO this store holds.
    pub fn remove(&mut self, id: &str) -> Option<ServiceLevelObjective> {
        let removed = self.slos.remove(slo_name_of(id)?);
        if let Some(slo) = removed.as_ref() {
            self.tags.remove(&slo.arn);
            // The window set belongs to the SLO, so it goes with it: a later SLO created
            // under the same name is a NEW resource and starts with no exclusion windows.
            self.exclusion_windows.remove(&slo.name);
        }
        removed
    }

    /// The ARN this resource's tag set is stored under, for a tagging operation's
    /// `ResourceArn`: the ARN of the STORED resource the request names, read off the record
    /// rather than taken from the request. `None` when the ARN names no resource this
    /// service holds, which is what the tagging operations answer
    /// `ResourceNotFoundException` for.
    ///
    /// `ResourceArn` is an ARN (`AmazonResourceName`), so only the ARN form resolves: a
    /// bare SLO name is not a resource ARN and names no resource here. Which ARNs name an
    /// SLO of this service and account is [`tagged_slo_name_in_arn`]'s rule.
    ///
    /// An instrumentation configuration is tagged too (`CreateInstrumentationConfiguration`
    /// takes a `TagList`), and its ARN carries the service and environment names, which may
    /// themselves contain `/` and `:` — so it is resolved by comparing against the ARN each
    /// stored record was minted with ([`names_stored_resource`], which compares the identity
    /// an ARN spells rather than taking it apart again) instead of re-deriving the identity.
    /// The scan is bounded by [`MAX_INSTRUMENTATION_CONFIGURATIONS`].
    pub fn tagged_resource_arn(&self, resource_arn: &str) -> Option<&str> {
        if let Some(arn) = tagged_slo_name_in_arn(resource_arn)
            .and_then(|name| self.slos.get(name))
            .map(|slo| slo.arn.as_str())
        {
            return Some(arn);
        }
        self.instrumentation_configurations
            .values()
            .flat_map(|set| set.configurations.values())
            .find(|configuration| names_stored_resource(resource_arn, &configuration.arn))
            .map(|configuration| configuration.arn.as_str())
    }

    /// The tags attached to the resource stored under `arn`, as the modelled `TagList`.
    ///
    /// Key order: the store is ordered by tag key, so two reads of one resource report the
    /// tags in the same order. A resource with no tags has no entry in the map and reports
    /// the empty list — holding no tags and holding an empty tag set are the same state.
    pub fn tag_list(&self, arn: &str) -> Vec<Tag> {
        self.tags
            .get(arn)
            .into_iter()
            .flatten()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }

    /// Attach `tags` to the tag set of the resource stored under `arn`, leaving every other
    /// tag of that resource in place, and hand back nothing: the stored set is what a later
    /// read reports.
    ///
    /// A tag set is a map from key to value, so attaching is per KEY: a key the resource does
    /// not carry yet is added, a key it already carries takes the new value, and a key the
    /// request does not name keeps the value it had. Attaching the same set twice therefore
    /// leaves the resource exactly as the first call left it. A key named more than once in one
    /// request resolves to the last value given for it, which is what a map of the request's
    /// pairs holds. An empty list attaches nothing and leaves no entry behind, so a resource
    /// that was never tagged stays stored exactly as one holding an empty tag set.
    pub fn put_tags(&mut self, arn: &str, tags: &[Tag]) {
        if tags.is_empty() {
            return;
        }
        let stored = self.tags.entry(arn.to_string()).or_default();
        for tag in tags {
            stored.insert(tag.key.clone(), tag.value.clone());
        }
    }

    /// How many tags the resource stored under `arn` would carry once `tags` were attached by
    /// [`Self::put_tags`]: [`tag_count_after_attach`] over that resource's stored set.
    pub fn tag_count_after_put(&self, arn: &str, tags: &[Tag]) -> usize {
        tag_count_after_attach(self.tags.get(arn), tags)
    }

    /// Detach `keys` from the tag set of the resource stored under `arn`, leaving every
    /// other tag of that resource in place.
    ///
    /// A key the resource does not carry removes nothing: removal is per key, so the result
    /// of a call is "none of these keys is attached any more" whether or not each one was
    /// attached to begin with, and repeating a call changes nothing further. The resource's
    /// entry is dropped once its last tag is gone, so an untagged resource is stored exactly
    /// as one that was never tagged.
    pub fn remove_tags<K: AsRef<str>>(&mut self, arn: &str, keys: &[K]) {
        let Some(tags) = self.tags.get_mut(arn) else {
            return;
        };
        for key in keys {
            tags.remove(key.as_ref());
        }
        if tags.is_empty() {
            self.tags.remove(arn);
        }
    }

    /// Replace this account's grouping configuration with `definitions`, stamping
    /// `updated_at` as the moment it was last written, and hand back the record that was
    /// stored. The stored record — never the request — is what the writer answers with and
    /// what a later read reports, so the two can never disagree.
    pub fn put_grouping_configuration(
        &mut self,
        definitions: Vec<GroupingAttributeDefinition>,
        updated_at: ::aws_smithy_types::DateTime,
    ) -> GroupingConfiguration {
        let configuration = GroupingConfiguration {
            grouping_attribute_definitions: definitions,
            updated_at,
        };
        self.grouping_configuration = Some(configuration.clone());
        configuration
    }

    /// Remove this account's grouping configuration, answering whether one was configured.
    ///
    /// Idempotent: clearing an account that has no configuration leaves the store `None`
    /// and is not an error, which is what the operation's `@idempotent` trait and its empty
    /// modelled error set (no not-found error) require.
    pub fn clear_grouping_configuration(&mut self) -> bool {
        self.grouping_configuration.take().is_some()
    }

    /// Enable Application Signals for this account, and hand back the record the account's
    /// enablement is held under.
    ///
    /// The enablement IS the service-linked role the account gains
    /// ([`discovery_service_linked_role_arn`]), stamped with the instant it was created, so
    /// `started_at` comes from the clock at the write rather than from a request.
    ///
    /// Idempotent: a role is created only while the account holds none, so an account that
    /// is already enabled keeps the record of the call that enabled it — neither the role
    /// nor the instant it was created moves — and every later call is the same success.
    pub fn start_discovery(&mut self, started_at: ::aws_smithy_types::DateTime) -> Discovery {
        self.discovery
            .get_or_insert_with(|| Discovery {
                service_linked_role_arn: discovery_service_linked_role_arn(),
                started_at,
            })
            .clone()
    }

    /// This account's Application Signals enablement, or `None` while discovery has never
    /// been started for it.
    pub fn discovery(&self) -> Option<&Discovery> {
        self.discovery.as_ref()
    }

    /// Whether another SLO would put this service over its [`MAX_SLOS`] quota.
    pub fn at_slo_quota(&self) -> bool {
        self.slos.len() >= MAX_SLOS
    }

    /// One page of the SLOs that pass `matches`, in name order, starting strictly after
    /// `cursor`, together with the cursor for the next page (`None` once the page is the
    /// last one).
    ///
    /// The next page's token is issued under `token_tag`, the tag of the very selection
    /// `matches` picks out ([`slo_selection_token_tag`]), so the key it carries is resolved
    /// only against the ordered set it was taken from.
    ///
    /// At most `page_size + 1` records are COLLECTED, so the memory the page holds and the
    /// response it produces are bounded by the page rather than by the number of stored
    /// records. The range scan itself still steps over any record the filter rejects, so a
    /// selector that matches few SLOs walks the stored set once.
    ///
    /// The token is opaque but not authenticated, so the KEY inside the tag is a value a
    /// caller may have written: it is honoured only when it is a boundary this paging can have
    /// minted over the set being read ([`issued_key`]), and `Err` otherwise carries the
    /// service's ONE wording for a token it did not issue ([`UNISSUED_NEXT_TOKEN`]).
    pub fn page<F>(
        &self,
        token_tag: &str,
        cursor: Option<&str>,
        page_size: usize,
        matches: F,
    ) -> Result<(Vec<ServiceLevelObjectiveSummary>, Option<String>), String>
    where
        F: Fn(&ServiceLevelObjective) -> bool,
    {
        let lower = match cursor {
            None => Bound::Unbounded,
            Some(cursor) => {
                // The name of the LAST objective this selection reports, and so the one
                // position past which this paging can have minted no boundary over the set
                // being read. A selection that reports nothing has minted none at all.
                let last_name = self
                    .slos
                    .values()
                    .rev()
                    .find(|slo| matches(slo))
                    .map(|slo| slo.name.as_str());
                Bound::Excluded(
                    issued_key(cursor, last_name, well_formed_slo_name)
                        .ok_or_else(|| UNISSUED_NEXT_TOKEN.to_string())?,
                )
            }
        };
        let mut window: Vec<&ServiceLevelObjective> = self
            .slos
            .range::<str, _>((lower, Bound::Unbounded))
            .map(|(_, slo)| slo)
            .filter(|slo| matches(slo))
            .take(page_size.saturating_add(1))
            .collect();

        let next_cursor = if window.len() > page_size {
            window.truncate(page_size);
            window.last().map(|slo| encode_cursor(token_tag, &slo.name))
        } else {
            None
        };
        Ok((window.into_iter().map(summary_of).collect(), next_cursor))
    }
}

/// What this account's Application Signals enablement is: the service-linked role the
/// account gained when discovery was started, and the instant it gained it.
///
/// Held beside the account's other singleton records rather than derived, because the
/// instant is the clock reading of the call that enabled the account and nothing else in
/// the store carries it.
#[derive(Debug, Clone)]
pub struct Discovery {
    /// The ARN of the service-linked role the account holds, as
    /// [`discovery_service_linked_role_arn`] renders it.
    pub service_linked_role_arn: String,
    /// The instant the account was enabled: the clock at the first `StartDiscovery` call,
    /// which is when the role was created.
    pub started_at: DateTime,
}

/// The service principal the CloudWatch Application Signals service-linked role is linked
/// to — the service that assumes it, and the segment IAM files the role under.
///
/// It is this service's own endpoint prefix under `amazonaws.com` (`OPERATIONS.md`:
/// `Endpoint prefix: application-signals`), which is also the principal the emulated IAM
/// service's published catalog of service-linked role names lists this role under, so the
/// ARN this service renders and the one IAM reports for that role are the same string.
pub const SERVICE_PRINCIPAL: &str = "application-signals.amazonaws.com";

/// The service-linked role `StartDiscovery` creates in the account.
///
/// Model doc for the operation: "Enables this Amazon Web Services account to be able to use
/// CloudWatch Application Signals by creating the
/// AWSServiceRoleForCloudWatchApplicationSignals service-linked role."
pub const DISCOVERY_SERVICE_LINKED_ROLE_NAME: &str =
    "AWSServiceRoleForCloudWatchApplicationSignals";

/// The ARN of [`DISCOVERY_SERVICE_LINKED_ROLE_NAME`]:
/// `arn:aws:iam::<account>:role/aws-service-role/application-signals.amazonaws.com/AWSServiceRoleForCloudWatchApplicationSignals`
/// — a role under the path IAM gives a service-linked role.
///
/// The role is an IAM resource, so the ARN names IAM rather than this service, and IAM is
/// global: the ARN carries NO Region, whichever Region the caller signed for. One role is
/// created per account and reused, so every call reads the same ARN.
pub fn discovery_service_linked_role_arn() -> String {
    // `iam` is the IAM ARN namespace the role belongs to, not this service's.
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("role/aws-service-role/{SERVICE_PRINCIPAL}/{DISCOVERY_SERVICE_LINKED_ROLE_NAME}"),
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApplicationsignalsApplicationsignalsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApplicationsignalsApplicationsignalsState>)
}

/// Whether an account id a request supplies names an account OTHER than the one this store
/// holds — the single place every account-scoped read of this service decides that.
///
/// This store is one account's and one Region's (see the store doc): every ARN it mints
/// carries `engine::emulator_account_id()`, so a request that names any other account names
/// one this emulator holds no records for, and the selection it asks for is empty. An ABSENT
/// account id names no account at all and therefore selects this store's own records, which
/// is why the answer for `None` is `false`.
///
/// Every reader that takes an account id — a list's own `AwsAccountId`, an SLO list's
/// `SloOwnerAwsAccountId`, the account an audited entity names, the account an entity MAP
/// states as one of its attributes ([`entity_account_id`]), and the account field of an ARN
/// a request supplies, whether the member is a `ResourceArn` or the ARN form of a
/// `ServiceLevelObjectiveId` (always present, so `Some`) — reads it through here, so the
/// reads that must agree about which account a request names cannot drift apart.
pub fn names_other_account(account: Option<&str>) -> bool {
    account.is_some_and(|account| account != ::engine::emulator_account_id())
}

/// The SLO name an `Id` NAMES A RECORD UNDER, or `None` when it names no resource of this
/// account at all. The modelled `ServiceLevelObjectiveId` pattern is an alternation of the
/// bare `ServiceLevelObjectiveName` and the full `ServiceLevelObjectiveArn`, so both forms
/// name one record and every operation that takes an `Id` must resolve them identically.
///
/// An `Id` given as an ARN is a caller-supplied ARN, so it is held to the same narrowing as
/// every other one this service resolves ([`tagged_resource_field`]): the VENDOR field must
/// be this service's namespace and the ACCOUNT field the account this store holds
/// ([`names_other_account`]). The modelled pattern does not make that check redundant — it
/// fixes the namespace, but spells the account field `[^:]*`, so nothing before a handler
/// constrains which account an `Id` names — and the account is part of what an ARN
/// identifies here, which is why an `Id` naming another account resolves to nothing exactly
/// as a `ResourceArn` naming another account does.
///
/// The REGION and PARTITION fields are not compared, for the reason
/// [`tagged_resource_field`] gives.
pub fn slo_name_of(id: &str) -> Option<&str> {
    match slo_name_in_arn(id) {
        Some(name) => tagged_resource_field(id).map(|_| name),
        None => Some(id),
    }
}

/// The SLO name an `Id` SPELLS, whichever form it takes: the name an ARN's resource field
/// carries, else the bare name itself.
///
/// This is what a refusal reports an `Id` by — the identity the REQUEST named, which exists
/// whether or not it names a record this store holds — while [`slo_name_of`] is the
/// resolution, which also decides whose account an ARN names.
pub fn slo_name_in_id(id: &str) -> &str {
    slo_name_in_arn(id).unwrap_or(id)
}

/// The SLO name an ARN names, or `None` when the string is not an SLO ARN at all.
///
/// The ARN form of the modelled `ServiceLevelObjectiveArn`: a well-formed resource ARN
/// whose resource field is `<SLO_ARN_RESOURCE>/<name>`. This is [`slo_name_in_id`] WITHOUT
/// its bare-name fallback, and [`slo_name_of`] without its account narrowing: it reads what
/// an ARN SPELLS, for a member the model types as an ARN rather than as a
/// `ServiceLevelObjectiveId`.
///
/// Read through `service_common::arn` — the workspace's ONE resource-ARN parse — so the name
/// comes out of the ARN's RESOURCE field, the last of the six, rather than out of the first
/// `:slo/` the string happens to contain: no earlier field (a region read off the caller's
/// credential scope, say) can be mistaken for the resource one, and this service agrees with
/// every other about what an ARN is.
pub fn slo_name_in_arn(arn: &str) -> Option<&str> {
    ::service_common::arn::resource_id_of_type(arn, SLO_ARN_RESOURCE)
}

/// The shortest and longest SLO name the modelled `ServiceLevelObjectiveName` pattern
/// admits (`^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$`): the two anchored alphanumeric
/// characters, plus up to 126 between them.
const SLO_NAME_LENGTH: ::std::ops::RangeInclusive<usize> = 2..=128;

/// Whether `name` is a `ServiceLevelObjectiveName`: an alphanumeric first and last
/// character, and `-`, `.`, `_`, a space or another alphanumeric between them.
///
/// This is the KEY SPACE of the SLO store — a record is keyed by its name, and a create that
/// names anything else is refused at the boundary — so it is also what says whether a value a
/// request carried could be a key of that store at all: the budget report reads an `Id` by it
/// (a member the model types as a plain string), and the objective listing reads a page
/// boundary by it ([`issued_key`]).
///
/// Compared byte for byte, which is exact because every character the pattern admits is
/// ASCII: any byte of a multi-byte character fails every class below.
pub fn well_formed_slo_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if !SLO_NAME_LENGTH.contains(&bytes.len()) {
        return false;
    }
    let Some((first, rest)) = bytes.split_first() else {
        return false;
    };
    let Some((last, middle)) = rest.split_last() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && last.is_ascii_alphanumeric()
        && middle
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b' '))
}

/// The SLO name a tagging operation's `ResourceArn` names, or `None` when the ARN names no
/// SLO of this service and account.
///
/// Stricter than [`slo_name_in_arn`], because `ResourceArn` is a bare `AmazonResourceName`:
/// the model puts no pattern on it, so nothing before the handler rejects an ARN of another
/// service or another account, and an ARN that merely CARRIES an SLO-shaped resource field
/// (`arn:aws:s3:::slo/x`) names no resource here. That narrowing — the VENDOR field is this
/// service's namespace and the ACCOUNT field the one this store holds — is
/// [`tagged_resource_field`]'s, the one every caller-supplied `ResourceArn` of this service
/// passes through. An `Id` in ARN form passes through it too ([`slo_name_of`]): its modelled
/// pattern fixes the namespace before a handler runs, but leaves the account field `[^:]*`,
/// so the account an `Id` names is no more constrained than a `ResourceArn`'s.
///
/// The REGION field is not compared, for the reason [`tagged_resource_field`] gives: this
/// process holds one account's and one Region's records, and the region in a minted ARN is a
/// rendering of the writer's SigV4 scope rather than part of an SLO's identity.
pub fn tagged_slo_name_in_arn(resource_arn: &str) -> Option<&str> {
    // The narrowing to this service's namespace and this account, which every ARN this
    // service resolves is held to; the name itself then comes out of the RESOURCE field.
    tagged_resource_field(resource_arn)?;
    slo_name_in_arn(resource_arn)
}

/// The RESOURCE field of `resource_arn` when that ARN addresses a resource of THIS service
/// and account, and `None` for any other value — the one narrowing every caller-supplied
/// `ResourceArn` of this service passes through, whichever KIND of resource it names.
///
/// The VENDOR field must be this service's namespace (`SIGNING_NAME`) and the ACCOUNT field
/// the account this store holds ([`names_other_account`]), because a bare
/// `AmazonResourceName` carries no modelled pattern: nothing before a handler rejects an ARN
/// of another service or another account.
///
/// The REGION and PARTITION fields are NOT part of what an ARN identifies here: this process
/// holds one account's and one Region's records, so those fields render the writer's SigV4
/// credential scope rather than the resource's identity, and both the modelled
/// `InstrumentationConfigurationArn` pattern (`^arn:[^:]+:application-signals:[^:]+:…`) and
/// the `ServiceLevelObjectiveArn` one leave the Region unconstrained. What is left is the
/// resource field, which is the whole identity an ARN of this service spells.
pub fn tagged_resource_field(resource_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse_of(resource_arn, SIGNING_NAME)?;
    (!names_other_account(Some(arn.account))).then_some(arn.resource)
}

/// Whether the caller-supplied `resource_arn` names the resource a stored record was minted
/// under `stored_arn` for.
///
/// The comparison is on the RESOURCE field each ARN carries ([`tagged_resource_field`]): the
/// field is compared WHOLE, so a service or environment name that itself contains `/` or `:`
/// (which the modelled `InstrumentationConfigurationArn` doc calls out) needs no taking the
/// identity apart again, while the region and partition a caller spelled do not narrow the
/// match — the same rule [`tagged_slo_name_in_arn`] applies to the service's other kind, so
/// one ARN resolves one stored resource whatever credential scope minted the spelling.
pub fn names_stored_resource(resource_arn: &str, stored_arn: &str) -> bool {
    tagged_resource_field(resource_arn)
        .is_some_and(|resource| ::service_common::arn::resource(stored_arn) == Some(resource))
}

/// The ARN of the SLO called `name` in `region`, in the shape the modelled
/// `ServiceLevelObjectiveArn` pattern requires.
pub fn slo_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(SIGNING_NAME, region, &format!("{SLO_ARN_RESOURCE}/{name}"))
}

/// The ARN of the SLO an `Id` denotes, for either form the modelled
/// `ServiceLevelObjectiveId` pattern admits: an `Id` already in ARN form IS that ARN, and a
/// bare name is rendered into the ARN it would carry in `region` (this account's, since the
/// store is one account's — see [`slo_name_of`]).
///
/// This is how an error identifies an SLO that is not stored: there is no record to read an
/// ARN off, so the ARN is derived from the identity the request named — including an `Id`
/// whose ARN form names another account, which is reported back as the request spelled it.
///
/// Which ids are already in ARN form is [`slo_name_in_arn`]'s rule — the one ARN reader in
/// this module, so the ARN this renders and the name [`slo_name_in_id`] reports cannot
/// disagree about the same id.
pub fn slo_arn_of(region: &str, id: &str) -> String {
    if slo_name_in_arn(id).is_some() {
        id.to_string()
    } else {
        slo_arn(region, id)
    }
}

/// The region an ARN minted for this request names: the one in the caller's SigV4
/// credential scope, else the emulator's default. An empty region in the scope falls back
/// too, so a minted ARN never carries an empty region segment.
pub fn region_of(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The modelled `ResourceNotFoundException` for an `Id` that names no stored SLO.
///
/// The failure identifies the SLO by its ARN — the full resource identifier, which is what
/// `ResourceId`'s doc ("Can't find the resource id") names and what a client parses to
/// resolve the resource — and reports the capitalised type name in `ResourceType`. Both are
/// `@required` members a client branches on, so a bare name there would hand it an
/// un-namespaced id and a type token that no equality test against the type name matches.
/// The message states the same two facts, so a caller reading only the prose sees the ARN
/// too.
pub fn slo_not_found(region: &str, id: &str) -> ResourceNotFoundException {
    let arn = slo_arn_of(region, id);
    ResourceNotFoundException {
        message: format!("{SLO_RESOURCE_TYPE} with arn '{arn}' does not exist."),
        resource_type: SLO_RESOURCE_TYPE.to_string(),
        resource_id: arn,
    }
}

/// The namespace the work-item layer that serves `UpdateServiceLevelObjective` addresses an
/// SLO in, as its refusal names it: the same token the modelled `ServiceLevelObjectiveArn`
/// pattern spells the resource segment with ([`SLO_ARN_RESOURCE`]).
const SLO_WORK_ITEM_NAMESPACE: &str = SLO_ARN_RESOURCE;

/// How many digits a work-item discriminator carries.
const WORK_ITEM_DISCRIMINATOR_DIGITS: u32 = 12;

/// The modelled `ResourceNotFoundException` `UpdateServiceLevelObjective` refuses an `Id`
/// that names no stored SLO with.
///
/// The update resolves the SLO through the WORK ITEM that holds it, so its message identifies
/// that work item rather than the resource: the account the item belongs to, the SLO NAME the
/// request named (the name half of either form the modelled `ServiceLevelObjectiveId` admits,
/// so an `Id` given as an ARN is reported by its name all the same), the item's own
/// discriminator and the namespace it lives in — and it states no ARN, which is not how the
/// work-item layer addresses anything.
///
/// `ResourceType` and `ResourceId` stay the RESOURCE's, as every other refusal about an SLO
/// reports them ([`slo_not_found`]): they are what a client branches on to learn which
/// resource it asked about, and that is the SLO either way.
pub fn slo_work_item_not_found(region: &str, id: &str) -> ResourceNotFoundException {
    let name = slo_name_in_id(id);
    let account = ::engine::emulator_account_id();
    ResourceNotFoundException {
        message: format!(
            "Work item with id: {account}:{name}:{} and namespace: {SLO_WORK_ITEM_NAMESPACE} \
             does not exist",
            work_item_discriminator(account, name)
        ),
        resource_type: SLO_RESOURCE_TYPE.to_string(),
        resource_id: slo_arn_of(region, id),
    }
}

/// The discriminator of the work item that would hold the SLO called `name` in `account`:
/// [`WORK_ITEM_DISCRIMINATOR_DIGITS`] decimal digits, server-minted.
///
/// It is the server's own id for the item, so it comes from nothing the caller sent beyond
/// the item's identity — and from that identity alone, which is what makes it STABLE: two
/// calls about one work item report one discriminator, and two work items get two. A value
/// drawn from the clock or a counter would change under a client that retries, and there is
/// no stored record to read one off, since the whole refusal is that the item is not there.
///
/// [`stable_hash`] renders exactly 16 hex characters, which is what a `u64` holds, so the
/// parse cannot fail; it falls back to a value rather than panicking all the same, because
/// this runs on a request path, where an unwind would turn a modelled 404 into a 500.
fn work_item_discriminator(account: &str, name: &str) -> String {
    let digest = stable_hash(&[Some(account), Some(name)]);
    let value = u64::from_str_radix(&digest, 16).unwrap_or_default();
    let digits = u64::pow(10, WORK_ITEM_DISCRIMINATOR_DIGITS);
    format!(
        "{:0>width$}",
        value % digits,
        width = WORK_ITEM_DISCRIMINATOR_DIGITS as usize
    )
}

/// How the MESSAGE of a tagging refusal names the resource the request addressed: the
/// generic noun, never the concrete type the ARN's resource segment spells.
///
/// The tagging operations take any resource of this service by `ResourceArn` alone
/// (`smithy_query(query="operation",operation="TagResource")`: `ResourceArn`,
/// `AmazonResourceName`), so the plane is resource-kind agnostic and its prose is one
/// sentence whatever kind an ARN names — a message that varied by kind would differ between
/// two calls the operation treats identically.
const TAGGED_RESOURCE_NOUN: &str = "Resource";

/// The modelled `ResourceNotFoundException` for a tagging operation's `ResourceArn` that
/// names no resource this service holds.
///
/// `ResourceId` is the ARN the request named, reported verbatim: the member is already a
/// full resource identifier, so there is nothing to mint from it and nothing of the
/// caller's identity to re-derive — unlike [`slo_not_found`], which renders an ARN for a
/// request that may have named a bare SLO name. `ResourceType` is the capitalised type name
/// of the resource KIND the ARN addresses ([`tagged_resource_type`]), so a client comparing
/// it sees the type it asked about rather than the type of this service's other resource.
///
/// The MESSAGE, in contrast, is the tagging plane's own ([`TAGGED_RESOURCE_NOUN`]): the
/// generic noun and the ARN the request carried, so one sentence serves every kind the plane
/// addresses.
pub fn tagged_resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    let resource_type = tagged_resource_type(resource_arn);
    ResourceNotFoundException {
        message: format!("{TAGGED_RESOURCE_NOUN} with arn '{resource_arn}' does not exist."),
        resource_type: resource_type.to_string(),
        resource_id: resource_arn.to_string(),
    }
}

/// The capitalised type name of the resource kind a tagging operation's `ResourceArn`
/// addresses: this service tags two kinds, and which one an ARN names is the resource TYPE
/// segment of an ARN of THIS service — read through `service_common::arn::parse_of`, the
/// workspace's ONE resource-ARN parse narrowed to this service's namespace, so the segment
/// comes out of the ARN's resource field rather than out of a substring match, and an ARN of
/// another vendor cannot be reported as a resource type of this one.
///
/// Anything else is reported as the SLO type, which is also what an ARN of another service or
/// another account earns: the tagging operations' primary resource.
pub fn tagged_resource_type(resource_arn: &str) -> &'static str {
    let names_configuration = ::service_common::arn::parse_of(resource_arn, SIGNING_NAME)
        .is_some_and(|arn| arn.resource_type() == INSTRUMENTATION_CONFIG_ARN_RESOURCE);
    if names_configuration {
        INSTRUMENTATION_CONFIG_RESOURCE_TYPE
    } else {
        SLO_RESOURCE_TYPE
    }
}

/// The sentence a create that names an SLO already stored is refused with: the identifier
/// the request sent is taken, stated over the rule and no value of it.
///
/// Entirely fixed text, with no trailing period. The identifier at fault is the `Name` the
/// caller itself sent (`smithy_query(query="operation",
/// operation="CreateServiceLevelObjective")`: `Name`, `@required`), so the sentence adds
/// nothing a client does not already hold, and the ARN of the stored record is an identifier
/// the caller never named.
const SLO_IDENTIFIER_TAKEN: &str = "A resource with the same identifier already exists";

/// The modelled `ConflictException` for a create that names an SLO already stored.
///
/// `ConflictException` carries only `Message`, so the sentence
/// ([`SLO_IDENTIFIER_TAKEN`]) is the whole body, and it is the same for every conflicting
/// name: what the caller has to change is the identifier it sent, which the request already
/// states.
pub fn slo_already_exists() -> ConflictException {
    ConflictException {
        message: SLO_IDENTIFIER_TAKEN.to_string(),
    }
}

/// One clause of a `ValidationException` message: the member whose value violated a
/// constraint, and the requirement it violated.
///
/// The ONE place this service words a constraint violation, reached both by the
/// framework-level refusal renderer (`crate::service`) and by a handler that enforces a
/// constraint itself, so the two cannot drift.
///
/// The clause names the member path and the requirement and NOTHING of the value that was
/// rejected: `ValidationException` carries only `message`, so this text is the whole
/// machine-readable detail a client gets, and the rejected value is caller input that would
/// otherwise be copied into every client log recording the failure.
pub fn constraint_clause(member_path: &str, requirement: &str) -> String {
    format!("Value at '{member_path}' failed to satisfy constraint: {requirement}")
}

/// [`constraint_clause`] for a member that was ABSENT, whose requirement is that it be
/// present.
///
/// Worded through the same builder as every other clause, with NO rendering of the absent
/// value: the recorded refusal of a call that omits a `@required` `@httpQuery` member reads
/// `1 validation error detected: Value at 'endTime' failed to satisfy constraint: Member
/// must not be null` — the clause states the member and the requirement, and says nothing
/// where a value would go. Reporting the absence as a `null` VALUE would describe a value
/// the request never carried.
pub fn absent_member_clause(member_path: &str) -> String {
    constraint_clause(member_path, "Member must not be null")
}

/// The `ValidationException` message over one or more clauses: the count of violated
/// constraints, then the clauses.
pub fn validation_envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        ),
    }
}

/// The refusal for a request that attaches more tags to one resource than that resource's
/// API documents, or `None` when the tag set the new resource will carry is within
/// [`MAX_TAGS_PER_RESOURCE`].
///
/// One helper for every CREATE that attaches a `TagList` to a resource with a documented tag
/// maximum, so the bound one create enforces is the bound they all enforce. Worded through
/// the same clause builder the boundary validator uses for a `@length` maximum, because for a
/// create that is what this is: a maximum on the member's length that the model states too
/// wide to enforce, checked against the whole tag set the new resource will carry.
///
/// That SET is what is counted, through the one counting rule every tag attachment in this
/// service is held to (`tag_count_after_attach`) — a create starts from no tags, so the
/// count is the number of DISTINCT keys the request names. A key named twice in one request
/// is one stored tag
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::put_tags`]), so counting the
/// member's entries instead would
/// refuse a request whose resulting set is within the maximum, and refuse it where an
/// attachment of the same list to the same resource is admitted.
///
/// An operation that attaches tags to an EXISTING resource ends up over the maximum through
/// the tags the resource already carries as much as through the ones the request names, so the
/// count there is a property of the resource rather than the length of a member — see
/// [`tag_quota_message`].
pub fn tag_count_refusal(tags: Option<&[Tag]>) -> Option<String> {
    let count = tags.map_or(0, |tags| tag_count_after_attach(None, tags));
    (count > MAX_TAGS_PER_RESOURCE).then(|| {
        validation_envelope(&[constraint_clause(
            "tags",
            &format!(
                "Member must have length less than or equal to {MAX_TAGS_PER_RESOURCE}"
            ),
        )])
    })
}

/// How many tags a resource whose stored tag set is `stored` would carry once `tags` were
/// attached to it by [`ComAmazonawsApplicationsignalsApplicationsignalsState::put_tags`].
///
/// The ONE counting rule every tag maximum in this service is enforced with, so a create and
/// a later attachment of one list to one resource cannot disagree about how many tags that
/// list leaves the resource carrying. A stored tag set is a map from key to value, so the
/// count is over the UNION of the stored keys and the request's KEYS: a request that only
/// re-values keys the resource already carries adds nothing to the count, and a key named more
/// than once in one request counts once, because it is one stored tag. `None` is the set of a
/// resource carrying no tags — what a create starts from.
fn tag_count_after_attach(stored: Option<&BTreeMap<String, String>>, tags: &[Tag]) -> usize {
    let added = tags
        .iter()
        .filter(|tag| stored.is_none_or(|stored| !stored.contains_key(&tag.key)))
        .map(|tag| tag.key.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    stored.map_or(0, BTreeMap::len) + added
}

/// The most tags the resource an ARN addresses may carry: the maximum that resource's own API
/// documents where it documents one ([`MAX_TAGS_PER_RESOURCE`]), and the modelled `TagList`
/// maximum ([`MAX_TAGS_PER_LIST`]) for a kind whose API documents none.
///
/// Which KIND an ARN addresses is [`tagged_resource_type`]'s rule, so the bound an attachment
/// enforces is the bound the create of that same kind enforces — a resource cannot be stored
/// carrying more tags than a later attachment to it would admit.
pub fn max_tags_of(resource_arn: &str) -> usize {
    if tagged_resource_type(resource_arn) == INSTRUMENTATION_CONFIG_RESOURCE_TYPE {
        MAX_TAGS_PER_LIST
    } else {
        MAX_TAGS_PER_RESOURCE
    }
}

/// The message of the quota failure for an attachment that would leave one resource carrying
/// more tags than [`max_tags_of`] admits for its kind.
///
/// The limit is on the resource, so the message states it as such: how many tags the
/// resulting set would hold and the maximum that resource may carry.
pub fn tag_quota_message(count_after: usize, maximum: usize) -> String {
    format!(
        "Attaching these tags would leave the resource with {count_after} tags, \
         over the quota of {maximum} tags per resource"
    )
}

/// A `@pattern` expression without the anchors a model may spell it with: the requirement
/// stated is the expression a value must satisfy, not the anchoring the model uses to make
/// the match total. A `$` escaped by a backslash is a literal dollar sign inside the
/// expression, not the end anchor, so it stays.
///
/// Only the expression's OUTER anchors are stripped: a `@pattern` that anchors each branch
/// of an alternation separately (`^a$|^b$`, as `ServiceLevelObjectiveId` does) keeps the
/// inner ones, because they are part of the expression's own structure rather than a wrapper
/// around it — dropping them would change which strings the reported expression describes.
pub fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The default `Goal.Interval`: a rolling interval of 7 days.
fn default_interval() -> Interval {
    Interval::RollingInterval(RollingInterval {
        duration_unit: DEFAULT_ROLLING_INTERVAL_UNIT.to_string(),
        duration: DEFAULT_ROLLING_INTERVAL_DURATION,
    })
}

/// The `Goal` stored for a request, with each member the model documents a default for
/// filled in when the request omitted it.
pub fn goal_with_defaults(requested: Option<&Goal>) -> Goal {
    Goal {
        interval: Some(
            requested
                .and_then(|goal| goal.interval.clone())
                .unwrap_or_else(default_interval),
        ),
        attainment_goal: Some(
            requested
                .and_then(|goal| goal.attainment_goal)
                .unwrap_or(DEFAULT_ATTAINMENT_GOAL),
        ),
        warning_threshold: Some(
            requested
                .and_then(|goal| goal.warning_threshold)
                .unwrap_or(DEFAULT_WARNING_THRESHOLD),
        ),
    }
}

/// The `Goal` an UPDATE stores, given the goal the record already carries and the goal the
/// request supplied.
///
/// `Goal`'s members are parameters in their own right — each documents its own behaviour
/// for being left out (`smithy_query(query="shape",shape="Goal")`: `Interval` "If you omit
/// this parameter, a rolling interval of 7 days is used", `WarningThreshold` "If you omit
/// this parameter, the default of 50.0 is used") — so an update that supplies the structure
/// changes only the members it NAMES and retains the stored value of each one it omits. The
/// documented default is what an omitted parameter falls back to when there is nothing
/// stored to retain, which is the create case, so [`goal_with_defaults`] still fills any
/// member neither side carries.
pub fn goal_merged(stored: &Goal, requested: &Goal) -> Goal {
    let retained = Goal {
        interval: requested
            .interval
            .clone()
            .or_else(|| stored.interval.clone()),
        attainment_goal: requested.attainment_goal.or(stored.attainment_goal),
        warning_threshold: requested.warning_threshold.or(stored.warning_threshold),
    };
    goal_with_defaults(Some(&retained))
}

/// The `MetricType` the indicator of a stored SLO reports, from the config that built it.
///
/// Both indicator shapes document the member identically
/// (`smithy_query(shape="ServiceLevelIndicatorMetric")`, member `MetricType`): "If the SLO
/// monitors either the `LATENCY` or `AVAILABILITY` metric that Application Signals
/// collects, this field displays which of those metrics is used." The member therefore
/// reports the standard metric Application Signals SELECTED for this SLO, not whatever the
/// request happened to carry, so it is reported only when the config actually made that
/// selection:
///
/// * the config must name the Application Signals entity whose standard metrics are
///   collected — its `KeyAttributes`, which the member's config-side doc requires for an
///   SLO "related to a metric collected by Application Signals"; and
/// * the config must not specify a metric of its OWN, since then that metric, not a
///   standard one, is what the SLO monitors: CloudWatch metric or metric-math queries
///   (`MetricDataQueries`, or a request-based config's request-count queries), a
///   `MetricName` which the model defines as "the name of the CloudWatch metric to use for
///   the SLO, when using a custom metric rather than Application Signals standard metrics",
///   or a `MetricSource`, "the metric source for SLOs on resources other than Application
///   Signals services".
///
/// Otherwise the member stays ABSENT: reporting it would tell a client the SLO tracks an
/// Application Signals standard metric when the SLO's metric is the one the request spelled
/// out, and would send that classification back on a Get-into-Update round trip.
fn reported_metric_type(
    requested: Option<&String>,
    key_attributes: Option<&HashMap<String, String>>,
    metric_name: Option<&str>,
    metric_source: Option<&MetricSource>,
    config_names_its_own_metric: bool,
) -> Option<String> {
    let selects_standard_metric = key_attributes.is_some()
        && metric_name.is_none()
        && metric_source.is_none()
        && !config_names_its_own_metric;
    requested.filter(|_| selects_standard_metric).cloned()
}

/// The queries a `MonitoredRequestCountMetric` holds, whichever of the union's two variants
/// names them: both target the same `MetricDataQueries` list, and which variant a request
/// chose says whether the queries count good or bad requests, not whether there are any.
fn monitored_request_count_queries(
    monitored: &MonitoredRequestCountMetricDataQueries,
) -> &[MetricDataQuery] {
    match monitored {
        MonitoredRequestCountMetricDataQueries::GoodCountMetric(queries) => queries,
        MonitoredRequestCountMetricDataQueries::BadCountMetric(queries) => queries,
    }
}

/// Whether a request-based SLI — a config or the stored indicator it built — spells
/// CloudWatch metric queries of its OWN.
///
/// A request-based SLI states its metric through TWO members, and the model leaves both
/// optional on the config: `TotalRequestCountMetric` ("the metric that you want to use as the
/// \"total requests\" number") and `MonitoredRequestCountMetric` ("the metric that you want to
/// use as the \"good request\" or \"bad request\" value"). Either one carrying at least one
/// query is the SLI naming its own metric, so this is the single place both derivations over
/// such an SLI — the standard-metric test and the metric source kind — ask the question, and
/// neither can count one member while ignoring the other. An EMPTY query list names no
/// metric, exactly as an empty `MetricDataQueries` does on the period-based side.
fn names_own_request_count_queries(
    total_request_count_metric: Option<&[MetricDataQuery]>,
    monitored_request_count_metric: Option<&MonitoredRequestCountMetricDataQueries>,
) -> bool {
    total_request_count_metric.is_some_and(|queries| !queries.is_empty())
        || monitored_request_count_metric
            .is_some_and(|monitored| !monitored_request_count_queries(monitored).is_empty())
}

/// The `MetricDataQueries` the indicator of a period-based SLI config reports.
///
/// An SLI config describes the metric the SLO evaluates in one of two ways, and the
/// indicator has ONE member for both — its `@required` `MetricDataQueries`:
///
/// * a config that spells the queries out is reported with those queries verbatim, each
///   already carrying its own `Period` and `Stat`; or
/// * a config that instead names the metric's parts — `MetricName` ("the name of the
///   CloudWatch metric to use for the SLO, when using a custom metric rather than
///   Application Signals standard metrics"), `Statistic` ("the statistic to use for
///   comparison to the threshold") and `PeriodSeconds` ("the number of seconds to use as the
///   period for SLO evaluation") — names exactly the three parts of a `MetricStat` (its
///   `Metric`, its `Stat` and its `Period`), so they are MATERIALISED into the single query
///   they describe. The output shape declares no member of its own for any of the three
///   (`smithy_query(shape="ServiceLevelIndicatorMetric")`), so the query list is the only
///   place they can surface: without this a read could not tell a client which metric, which
///   statistic or which evaluation period the SLO it returns is measured by.
///
/// A materialised query reports only what the request stated. The metric's `Namespace` and
/// `Dimensions` stay ABSENT: nothing in the request names them, the real service takes them
/// from its own standard-metric catalogue, and a namespace or dimension set invented here
/// would name a metric nothing selected.
///
/// A config that names NO `Statistic` materialises nothing and the required list is reported
/// EMPTY, whichever other members it names: `MetricStat.Stat` is `@required` with no default,
/// nothing else in the request states a statistic, and a query carrying an invented one would
/// report an evaluation the caller never asked for. That is the standard-metric case —
/// `KeyAttributes` with a `MetricType` — where the whole query comes from the catalogue this
/// emulator cannot compute.
fn materialised_sli_metric_data_queries(
    metric: &ServiceLevelIndicatorMetricConfig,
) -> Vec<MetricDataQuery> {
    if let Some(queries) = metric
        .metric_data_queries
        .as_ref()
        .filter(|queries| !queries.is_empty())
    {
        return queries.clone();
    }
    let Some(stat) = metric.statistic.clone() else {
        return Vec::new();
    };
    vec![MetricDataQuery {
        id: MATERIALISED_SLI_QUERY_ID.to_string(),
        metric_stat: Some(MetricStat {
            metric: Metric {
                namespace: None,
                metric_name: metric.metric_name.clone(),
                dimensions: None,
            },
            period: metric.period_seconds.unwrap_or(MIN_SLI_PERIOD_SECONDS),
            stat,
            unit: None,
        }),
        // `ReturnData` is documented for metric MATH ("use this only if you are using a
        // metric math expression for the SLO"), and this query is a single metric rather
        // than an expression, so the member stays absent. `Expression` is mutually exclusive
        // with `MetricStat`, and nothing in the config names a `Label` or an `AccountId`.
        expression: None,
        label: None,
        return_data: None,
        period: None,
        account_id: None,
    }]
}

/// The `ServiceLevelIndicator` a period-based `SliConfig` describes.
///
/// Every member of the indicator is the corresponding member of the config, with the
/// config's own modelled defaults applied to `MetricThreshold` and `ComparisonOperator`,
/// except `MetricType`, which is reported only when the config selected an Application
/// Signals standard metric (see [`reported_metric_type`]), and
/// `SliMetric.MetricDataQueries`, which is where the metric the config describes surfaces
/// (see [`materialised_sli_metric_data_queries`]).
pub fn sli_from_config(config: &ServiceLevelIndicatorConfig) -> ServiceLevelIndicator {
    let metric = &config.sli_metric_config;
    ServiceLevelIndicator {
        sli_metric: ServiceLevelIndicatorMetric {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            // Judged from the members the CONFIG carries, not from the query materialised
            // below: a config that states a statistic and a period for the standard metric
            // it selected has still selected that standard metric.
            metric_type: reported_metric_type(
                metric.metric_type.as_ref(),
                metric.key_attributes.as_ref(),
                metric.metric_name.as_deref(),
                metric.metric_source.as_ref(),
                metric
                    .metric_data_queries
                    .as_ref()
                    .is_some_and(|queries| !queries.is_empty()),
            ),
            metric_data_queries: materialised_sli_metric_data_queries(metric),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        },
        metric_threshold: config
            .metric_threshold
            .unwrap_or(DEFAULT_SLI_METRIC_THRESHOLD),
        comparison_operator: config
            .comparison_operator
            .clone()
            .unwrap_or_else(|| DEFAULT_SLI_COMPARISON_OPERATOR.to_string()),
    }
}

/// The `RequestBasedServiceLevelIndicator` a `RequestBasedSliConfig` describes.
///
/// `MetricThreshold`/`ComparisonOperator` carry no default on this config (the model
/// declares them only "required if this SLO is tracking the Latency metric"), so they are
/// stored exactly as supplied, and `MetricType` follows the same standard-metric rule as
/// the period-based indicator (see [`reported_metric_type`]).
///
/// `TotalRequestCountMetric` and `MonitoredRequestCountMetric` are `@required` on the
/// indicator but optional on the config, and this config states NO statistic and NO period
/// to evaluate a metric with (unlike the period-based one, it declares neither `Statistic`
/// nor `PeriodSeconds`), so a config that supplies neither list describes no query that
/// could be materialised: `MetricStat` needs a `Stat` and a `Period` that nothing in the
/// request names. Both required members are therefore reported with what the config
/// supplied, and the union keeps the good-count variant when the config named neither — the
/// member admits no absent value, and a query filled in from the service's own
/// standard-metric catalogue is not something this emulator can compute.
pub fn request_based_sli_from_config(
    config: &RequestBasedServiceLevelIndicatorConfig,
) -> RequestBasedServiceLevelIndicator {
    let metric = &config.request_based_sli_metric_config;
    RequestBasedServiceLevelIndicator {
        request_based_sli_metric: RequestBasedServiceLevelIndicatorMetric {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            metric_type: reported_metric_type(
                metric.metric_type.as_ref(),
                metric.key_attributes.as_ref(),
                metric.metric_name.as_deref(),
                metric.metric_source.as_ref(),
                names_own_request_count_queries(
                    metric.total_request_count_metric.as_deref(),
                    metric.monitored_request_count_metric.as_ref(),
                ),
            ),
            total_request_count_metric: metric
                .total_request_count_metric
                .clone()
                .unwrap_or_default(),
            monitored_request_count_metric: metric
                .monitored_request_count_metric
                .clone()
                .unwrap_or(MonitoredRequestCountMetricDataQueries::GoodCountMetric(
                    Vec::new(),
                )),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        },
        metric_threshold: config.metric_threshold,
        comparison_operator: config.comparison_operator.clone(),
    }
}

/// The `EvaluationType` the SLI config a request supplies describes, or `None` when the
/// request supplies neither config.
///
/// The two configs are mutually exclusive, and which one a request carries is the ONLY
/// thing that decides the kind: `SliConfig` is documented as "if this SLO is a period-based
/// SLO, this structure defines..." and `RequestBasedSliConfig` as "if this SLO is a
/// request-based SLO, ...". Derived in one place so every operation that accepts the pair
/// classifies it identically.
pub fn requested_evaluation_type(
    sli_config: Option<&ServiceLevelIndicatorConfig>,
    request_based_sli_config: Option<&RequestBasedServiceLevelIndicatorConfig>,
) -> Option<&'static str> {
    if sli_config.is_some() {
        Some(EVALUATION_TYPE_PERIOD_BASED)
    } else if request_based_sli_config.is_some() {
        Some(EVALUATION_TYPE_REQUEST_BASED)
    } else {
        None
    }
}

/// The member path a refusal about the period-based SLI config names, spelled the way the
/// boundary validator spells a member path for this service ([`START_TIME_MEMBER_PATH`]):
/// lower camel case, as the refusal of a modelled constraint on a member of the same
/// operation spells it.
const SLI_CONFIG_MEMBER_PATH: &str = "sliConfig";

/// [`SLI_CONFIG_MEMBER_PATH`], for the member that carries the request-based config.
const REQUEST_BASED_SLI_CONFIG_MEMBER_PATH: &str = "requestBasedSliConfig";

/// The modelled name of the member a period-based SLI config travels in
/// (`smithy_query(query="operation",operation="CreateServiceLevelObjective")`: `SliConfig`),
/// which is how a refusal that names the member in PROSE spells it — the shape's own
/// spelling, not the lower-camel path a constraint clause states
/// ([`SLI_CONFIG_MEMBER_PATH`]).
const SLI_CONFIG_MEMBER: &str = "SliConfig";

/// [`SLI_CONFIG_MEMBER`], for the member that carries the request-based config.
const REQUEST_BASED_SLI_CONFIG_MEMBER: &str = "RequestBasedSliConfig";

/// How prose NAMES an SLO's evaluation kind: the hyphenated words the SLO operations'
/// own docs use for it ("a period-based SLO", "a request-based SLO"), which is not the
/// `EvaluationType` token a response reports ([`EVALUATION_TYPE_PERIOD_BASED`]).
fn evaluation_kind_prose(evaluation_kind: &str) -> &'static str {
    if evaluation_kind == EVALUATION_TYPE_REQUEST_BASED {
        "request-based"
    } else {
        "period-based"
    }
}

/// The refusal for a request that names BOTH mutually exclusive SLI configs, or `None`
/// when it names at most one.
///
/// Documented on both members — "You can't specify both RequestBasedSliConfig and SliConfig
/// in the same operation" — and inexpressible in the model, so the boundary validator admits
/// the pair and every operation that accepts it refuses here, through one wording.
///
/// Worded through the service's one constraint-violation builder ([`constraint_clause`],
/// under [`validation_envelope`]), like every other constraint this crate enforces in a
/// handler: the clause names the member the request must DROP and the sibling whose presence
/// forbids it, so the member at fault reaches a client exactly as it does from a modelled
/// constraint refused at the boundary. The pair is symmetric, and the member named is the one
/// the input shapes declare SECOND (`smithy_query(query="operation",
/// operation="CreateServiceLevelObjective")`: `SliConfig`, then `RequestBasedSliConfig`) —
/// the same tie-break the inverted-window refusal makes by naming the member that CLOSES a
/// pair ([`status_event_window`]).
pub fn exclusive_sli_configs_refusal(
    sli_config: Option<&ServiceLevelIndicatorConfig>,
    request_based_sli_config: Option<&RequestBasedServiceLevelIndicatorConfig>,
) -> Option<String> {
    (sli_config.is_some() && request_based_sli_config.is_some()).then(|| {
        validation_envelope(&[constraint_clause(
            REQUEST_BASED_SLI_CONFIG_MEMBER_PATH,
            &format!("Member must be null when {SLI_CONFIG_MEMBER_PATH} is provided"),
        )])
    })
}

/// The refusal for a request that must DEFINE an SLO's indicator and names neither config,
/// or `None` when it names one.
///
/// An SLO is its service level indicator: the operation doc states SLOs "use service level
/// indicators (SLIs) to calculate whether the application is performing at the level that
/// you want" and that creating one is where "you specify whether it is a period-based SLO or
/// a request-based SLO", a classification [`requested_evaluation_type`] can only read off
/// one of the two configs. So a definition that names neither describes no indicator, no
/// evaluation kind and no metric source, and is refused instead of stored.
///
/// Worded as the rule itself reads, not through the constraint-clause builder
/// ([`constraint_clause`]): the requirement is satisfied by EITHER member, so the sentence
/// states the pair and what it must do — a clause naming one member and its sibling as a
/// condition would report a constraint on a member the request need not supply.
pub fn absent_sli_config_refusal(
    sli_config: Option<&ServiceLevelIndicatorConfig>,
    request_based_sli_config: Option<&RequestBasedServiceLevelIndicatorConfig>,
) -> Option<String> {
    requested_evaluation_type(sli_config, request_based_sli_config)
        .is_none()
        .then(|| {
            format!(
                "Either {SLI_CONFIG_MEMBER} or {REQUEST_BASED_SLI_CONFIG_MEMBER} must be \
                 populated"
            )
        })
}

/// The `EvaluationType` a STORED SLO is of, or `None` when the record carries no indicator
/// at all — which no stored record does, since a create names one indicator or is refused
/// ([`absent_sli_config_refusal`]).
///
/// Judged from the record's own declared `EvaluationType`, falling back to whichever
/// indicator it holds, so the kind of a record is read off the record rather than recomputed
/// from the request that is trying to change it. Only the two values the modelled
/// `EvaluationType` enum names are ever written, so any other string is treated as no
/// declared kind and the indicator decides.
pub fn stored_evaluation_type(slo: &ServiceLevelObjective) -> Option<&'static str> {
    match slo.evaluation_type.as_deref() {
        Some(EVALUATION_TYPE_PERIOD_BASED) => Some(EVALUATION_TYPE_PERIOD_BASED),
        Some(EVALUATION_TYPE_REQUEST_BASED) => Some(EVALUATION_TYPE_REQUEST_BASED),
        _ if slo.sli.is_some() => Some(EVALUATION_TYPE_PERIOD_BASED),
        _ if slo.request_based_sli.is_some() => Some(EVALUATION_TYPE_REQUEST_BASED),
        _ => None,
    }
}

/// The refusal a request that would change a STORED SLO's evaluation kind earns, or `None`
/// when the two kinds agree, when the request names no config, or when the record carries no
/// kind for a config to disagree with.
///
/// An SLO's evaluation kind is settled when it is created:
/// `UpdateServiceLevelObjective`'s doc states "You cannot change from a period-based SLO to a
/// request-based SLO, or change from a request-based SLO to a period-based SLO", so a config
/// of the other kind than the record's own is refused rather than written. A DIFFERENT rule
/// from [`exclusive_sli_configs_refusal`]: that one is about one request naming two configs,
/// this one about a request whose single config does not belong to the stored resource's kind.
///
/// Worded as the rule itself reads, naming the DIRECTION the request attempted: the kind the
/// stored record is of and the kind the supplied config would make it, each in the prose the
/// docs name a kind with ([`evaluation_kind_prose`]). A client is told which half of its
/// request was wrong, which a direction-agnostic restatement of the rule — or a constraint
/// clause about one member ([`constraint_clause`]) — does not say.
pub fn evaluation_kind_change_refusal(
    stored_kind: Option<&str>,
    requested_kind: Option<&str>,
) -> Option<String> {
    let (stored, requested) = (stored_kind?, requested_kind?);
    (stored != requested).then(|| {
        format!(
            "Updating from {} SLO to {} SLO is not allowed",
            evaluation_kind_prose(stored),
            evaluation_kind_prose(requested)
        )
    })
}

/// The `MetricSourceType` an SLI describes.
///
/// `smithy_query(shape="MetricSourceType")` lists exactly six values — `AppMonitor`,
/// `Canary`, `CloudWatchMetric`, `Service`, `ServiceDependency`, `ServiceOperation` — and
/// the member's doc calls them "the SLI metric source type for this SLO". They name the
/// six kinds of source an SLI config can describe, so the value follows from which source
/// members the request supplied, most specific first: a `MetricSource` names a resource
/// outside Application Signals and carries its own `Type` key attribute; otherwise a
/// dependency, then an operation, then a service narrows the Application Signals entity;
/// and an SLI that names no entity at all is reading plain CloudWatch metrics.
///
/// A `MetricSource` whose `Type` names neither of the two specific resource kinds the enum
/// has values for falls back to `CloudWatchMetric` rather than to `None`: the modelled
/// `MetricSource` doc defines the member as "the metric source for SLOs on resources other
/// than Application Signals services", so such an SLO IS reading that resource's CloudWatch
/// metrics, and `CloudWatchMetric` is the enum's only value for a non-Application-Signals
/// source. Reporting `None` would drop a source kind the request did state, and reporting
/// `Service` would claim an Application Signals entity the request did not name.
///
/// `names_own_metric_queries` is that last test: whether the SLI spells CloudWatch metric
/// queries of its own, in whichever member(s) its kind of indicator states them — so the
/// caller answers it over EVERY such member rather than over one of them.
fn metric_source_type(
    key_attributes: Option<&HashMap<String, String>>,
    operation_name: Option<&str>,
    dependency_config: Option<&DependencyConfig>,
    metric_source: Option<&MetricSource>,
    names_own_metric_queries: bool,
) -> Option<String> {
    if let Some(source) = metric_source {
        // A key-attribute `Type` is a CloudFormation-style name such as
        // `AWS::RUM::AppMonitor`; its last segment is the enum value.
        let kind = source
            .metric_source_key_attributes
            .get("Type")
            .and_then(|value| value.rsplit("::").next())
            .unwrap_or_default();
        return Some(
            match kind {
                "AppMonitor" => "AppMonitor",
                "Canary" => "Canary",
                _ => "CloudWatchMetric",
            }
            .to_string(),
        );
    }
    if dependency_config.is_some() {
        return Some("ServiceDependency".to_string());
    }
    if operation_name.is_some() {
        return Some("ServiceOperation".to_string());
    }
    if key_attributes.is_some() {
        return Some("Service".to_string());
    }
    if names_own_metric_queries {
        return Some("CloudWatchMetric".to_string());
    }
    None
}

/// The `MetricSourceType` of a stored SLO, from whichever indicator it carries.
pub fn derive_metric_source_type(
    sli: Option<&ServiceLevelIndicator>,
    request_based_sli: Option<&RequestBasedServiceLevelIndicator>,
) -> Option<String> {
    if let Some(sli) = sli {
        let metric = &sli.sli_metric;
        return metric_source_type(
            metric.key_attributes.as_ref(),
            metric.operation_name.as_deref(),
            metric.dependency_config.as_ref(),
            metric.metric_source.as_ref(),
            !metric.metric_data_queries.is_empty(),
        );
    }
    if let Some(request_based_sli) = request_based_sli {
        let metric = &request_based_sli.request_based_sli_metric;
        return metric_source_type(
            metric.key_attributes.as_ref(),
            metric.operation_name.as_deref(),
            metric.dependency_config.as_ref(),
            metric.metric_source.as_ref(),
            names_own_request_count_queries(
                Some(&metric.total_request_count_metric),
                Some(&metric.monitored_request_count_metric),
            ),
        );
    }
    None
}

/// The entity members a stored SLO's indicator identifies: key attributes, operation name,
/// dependency, metric source and composite-SLI configuration. `ListServiceLevelObjectives`
/// both projects and filters on them, so they are read out of the record in ONE place.
pub struct SloEntity {
    pub key_attributes: Option<HashMap<String, String>>,
    pub operation_name: Option<String>,
    /// The kind of metric the indicator measures (`MetricType`,
    /// `smithy_query(shape="ServiceLevelIndicatorMetric")`), which is how a read that
    /// selects on the measured type — an audit target naming a service operation and its
    /// metric type — narrows to the objectives that measure it.
    pub metric_type: Option<String>,
    pub dependency_config: Option<DependencyConfig>,
    pub metric_source: Option<MetricSource>,
    pub composite_sli_config: Option<crate::ops::types::CompositeSliConfig>,
}

/// Read the entity an SLO's indicator identifies.
pub fn entity_of(slo: &ServiceLevelObjective) -> SloEntity {
    if let Some(sli) = slo.sli.as_ref() {
        let metric = &sli.sli_metric;
        return SloEntity {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            metric_type: metric.metric_type.clone(),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        };
    }
    if let Some(request_based_sli) = slo.request_based_sli.as_ref() {
        let metric = &request_based_sli.request_based_sli_metric;
        return SloEntity {
            key_attributes: metric.key_attributes.clone(),
            operation_name: metric.operation_name.clone(),
            metric_type: metric.metric_type.clone(),
            dependency_config: metric.dependency_config.clone(),
            metric_source: metric.metric_source.clone(),
            composite_sli_config: metric.composite_sli_config.clone(),
        };
    }
    SloEntity {
        key_attributes: None,
        operation_name: None,
        metric_type: None,
        dependency_config: None,
        metric_source: None,
        composite_sli_config: None,
    }
}

/// The `ServiceLevelObjectiveSummary` projection of a stored SLO, as
/// `ListServiceLevelObjectives` returns it.
pub fn summary_of(slo: &ServiceLevelObjective) -> ServiceLevelObjectiveSummary {
    let entity = entity_of(slo);
    ServiceLevelObjectiveSummary {
        arn: slo.arn.clone(),
        name: slo.name.clone(),
        key_attributes: entity.key_attributes,
        operation_name: entity.operation_name,
        dependency_config: entity.dependency_config,
        created_time: Some(slo.created_time),
        evaluation_type: slo.evaluation_type.clone(),
        metric_source_type: slo.metric_source_type.clone(),
        metric_source: entity.metric_source,
        composite_sli_config: entity.composite_sli_config,
    }
}

/// The message a handler reports when [`service_state`] answers `None`.
///
/// `None` means this service was mounted on an engine type it does not know — an assembly
/// mistake no request can fix, and one a correctly built router never produces. None of the
/// SLO operations models a server-fault error, so each reports it as the modelled
/// `ThrottlingException`: of the errors they do declare, it is the only one that does not
/// mislead the caller about its own request.
pub const STATE_UNAVAILABLE: &str = "Application Signals state is unavailable";

/// A `NextToken` carrying `cursor` under `tag`: hex, so the caller reads it as the opaque
/// value the model documents rather than as a store key it could edit.
pub fn encode_cursor(tag: &str, cursor: &str) -> String {
    ::hex::encode(format!("{tag}{cursor}"))
}

/// The cursor `token` carries under `tag`, or `None` when it is not a token this service
/// issued under that tag.
///
/// Only the ONE spelling [`encode_cursor`] writes for a position names that position, so the
/// token is accepted only when it is byte-for-byte the token this service would mint for the
/// cursor it decodes to. `hex::decode` reads more than `hex::encode` writes — it accepts
/// `A-F` as well as the `a-f` the minter emits — so without that comparison a re-cased copy
/// of an issued token, a string no response of this service ever carried, would resume a
/// page. Re-encoding the decoded cursor is what compares the two, so the accepted spelling
/// is the ENCODER's rather than a second grammar maintained beside it, exactly as the
/// POSITION inside the tag is held to the spelling its encoder writes
/// ([`canonical_number`]).
pub fn decode_cursor(tag: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let cursor = decoded.strip_prefix(tag)?;
    (encode_cursor(tag, cursor) == token).then(|| cursor.to_string())
}

/// The refusal every read of this service reports for a `NextToken` it did not issue.
///
/// ONE wording for ONE failure, whichever read resolved the cursor — the token is opaque to
/// the caller ([`encode_cursor`]), so "this service did not issue it" is the whole
/// requirement a client can act on, and a client comparing the message gets the same text
/// from a read whose cursor is a store key and from one whose cursor is an offset
/// ([`page_of`]).
///
/// It names the member and the requirement and NOTHING of the rejected token, the rule
/// [`constraint_clause`] states for every constraint violation this service words: the
/// token is caller input, and a refusal that rendered it would copy it into every client log
/// recording the failure.
pub const UNISSUED_NEXT_TOKEN: &str = "NextToken is not a token this service issued";

/// [`UNISSUED_NEXT_TOKEN`] narrowed to the SCOPE a read resolves its cursor within, for a
/// read whose token is only a cursor within the one scope that issued it: same member, same
/// requirement, plus the scope the token had to have been issued for.
///
/// `scope` names the request members that pick that scope out — never their values, so the
/// narrowing adds no echo of caller input either.
pub fn unissued_next_token_in(scope: &str) -> String {
    format!("{UNISSUED_NEXT_TOKEN} for {scope}")
}

/// The SCOPE a `ListInstrumentationConfigurations` cursor is only a cursor within, as
/// [`unissued_next_token_in`] narrows that read's refusal by: the request members that pick
/// the one ordered set its tokens are issued over
/// ([`instrumentation_token_tag`]), and never their values.
///
/// Stated once so the refusal for a token carrying no tag of that scope and the refusal for a
/// payload inside such a tag that names no boundary of it
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::page_instrumentation_configurations`])
/// are the SAME text: both say the token is not one this service issued for that scope.
pub const INSTRUMENTATION_CURSOR_SCOPE: &str =
    "this service, environment and instrumentation type";

/// The page size a list read serves for the `MaxResults` a request states, or the refusal
/// for a bound outside the range that member admits: at least [`MIN_PAGE_BOUND`] and at
/// most `maximum`.
///
/// Where a `MaxResults` member's own model declares both bounds, the centralized walk
/// enforces them before any handler runs, and the handler reads the value straight off the
/// input. This is the same rule for a member whose model declares a maximum ONLY: a bound is
/// never coerced, because a page smaller than the bound means "the list ends here" and a page
/// clamped up to one entry plus a `NextToken` would answer a request nobody made. The minimum
/// is the same for every list of this service ([`MIN_PAGE_BOUND`]), so a member that leaves it
/// unstated is held to it here and the service holds ONE position on the rule.
///
/// The refusal is worded by the service's one constraint-violation builder — member path
/// and requirement, with no echo of the rejected value ([`constraint_clause`]) — so a bound
/// refused here reads exactly like the same bound refused at the boundary for the lists
/// whose model states its minimum.
pub fn page_bound(member_path: &str, max_results: i32, maximum: i32) -> Result<usize, String> {
    if max_results < MIN_PAGE_BOUND {
        return Err(validation_envelope(&[constraint_clause(
            member_path,
            &format!("Member must have value greater than or equal to {MIN_PAGE_BOUND}"),
        )]));
    }
    if max_results > maximum {
        return Err(validation_envelope(&[constraint_clause(
            member_path,
            &format!("Member must have value less than or equal to {maximum}"),
        )]));
    }
    Ok(max_results as usize)
}

/// The page size a read serves for a `MaxResults` whose own model declares the WHOLE range it
/// admits together with the `@default` its documentation names, where `default` is the page
/// that operation documents for a caller who states no bound.
///
/// The centralized walk enforces both ends of such a member's range before any handler runs,
/// so the only case left for a handler to decide is the member that carries no bound at all: a
/// non-positive value can therefore only mean the member was ABSENT, and is read as the
/// documented default rather than as a page of nothing, which could not advance its own cursor
/// ([`page_of`]).
///
/// [`page_bound`] is the same rule for a member whose model states a maximum ONLY: there the
/// bound is refused rather than defaulted, because such a member documents no default to fall
/// back on, and [`optional_page_bound`] is the same rule for a member that declares its whole
/// range but no usable default, which a handler therefore receives as an `Option`. All live
/// here so every read of this service that holds to one of them holds to ONE copy of it, and
/// the page sizes they default to are the constants beside them.
pub fn defaulted_page_bound(max_results: i32, default: i32) -> usize {
    if max_results < MIN_PAGE_BOUND {
        default as usize
    } else {
        max_results as usize
    }
}

/// The page size a read serves for a `MaxResults` whose own model declares the WHOLE range it
/// admits but NO usable `@default`, so the handler receives the member as an `Option`, where
/// `default` is the page that operation documents for a caller who states no bound.
///
/// The centralized walk enforces both ends of such a member's range before any handler runs,
/// so a bound that reaches a handler at all is inside the range and is read straight off the
/// input; the only case left to decide is the member that is ABSENT, which takes that
/// operation's documented default page rather than a page of nothing, which could not advance
/// its own cursor ([`page_of`]).
///
/// [`defaulted_page_bound`] is the same rule for a member whose model carries that default
/// itself (so the handler receives an `i32` already defaulted by the walk), and [`page_bound`]
/// for one whose model states a maximum ONLY, where the bound is refused rather than
/// defaulted. All three live here so every read of this service that holds to one of them
/// holds to ONE copy of it, and the page sizes they default to are the constants beside them.
pub fn optional_page_bound(max_results: Option<i32>, default: i32) -> usize {
    max_results.unwrap_or(default).max(0) as usize
}

/// A decimal field of a cursor payload, read as the number it spells and only when it spells
/// it the way this service WROTE it.
///
/// A payload is rendered by `Display` ([`encode_cursor`]), which spells one number one way: no
/// sign on a value that is not negative, and no zero before its most significant digit.
/// `FromStr` reads more than that — a leading `+`, and any number of leading zeros — so a
/// field it accepts but `Display` would never have produced is not a field this service wrote,
/// and the token carrying it is not a token this service issued. Rendering the parsed value
/// back is what compares the two, so the accepted spelling is the ENCODER's rather than a
/// second grammar maintained beside it.
fn canonical_number<T>(field: &str) -> Option<T>
where
    T: ::std::str::FromStr + ::std::fmt::Display,
{
    let value = field.parse::<T>().ok()?;
    (value.to_string() == field).then_some(value)
}

/// The offset `cursor` names into a projection of `len` entries, or `None` when it is not an
/// offset [`page_of`] can have minted over that projection.
///
/// A page token is minted by counting PAST the page just served (`offset + page_size`, with
/// `page_size` at least [`MIN_PAGE_BOUND`]) and only while the projection still holds entries
/// beyond it, so every offset this service hands out is a whole number of at least one entry
/// and strictly inside the projection: the FIRST entry is served by the request that states no
/// cursor, and an offset at or past the end bounds no page at all. An offset outside that
/// range, or one spelled a way the encoder does not spell it ([`canonical_number`]), is
/// therefore a boundary no read of this service issued — and honouring it would re-serve the
/// first page under a cursor, or report the projection as exhausted to a caller whose cursor is
/// not one it was given.
fn issued_offset(cursor: &str, len: usize) -> Option<usize> {
    let offset = canonical_number::<usize>(cursor)?;
    (MIN_PAGE_BOUND as usize..len)
        .contains(&offset)
        .then_some(offset)
}

/// The ordinal `cursor` names as the boundary of a page over a stored exclusion-window set
/// whose highest ordinal is `last_ordinal`, or `None` when it is not a boundary
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::page_exclusion_windows`] can have
/// minted over that set.
///
/// A page token is minted by keying on the LAST window the page served, and only while the set
/// still holds a window beyond it, so every ordinal this service hands out is the ordinal of a
/// stored window that at least one other window follows: an ordinal below
/// [`FIRST_EXCLUSION_WINDOW_ORDINAL`] names no window at all — the first window is served by
/// the request that states no cursor — and one at or past the highest stored ordinal bounds no
/// page. An ordinal outside that range, or one spelled a way the encoder does not spell it
/// ([`canonical_number`]), is therefore a boundary no read of this service issued over the set
/// being read, exactly as for the offset cursors ([`issued_offset`]). An ordinal INSIDE it is
/// honoured whether or not the set still holds the window it keys on, which is what makes the
/// boundary survive the removal of that very window.
///
/// The range is the set as it stands NOW, because that is the only set this read can place a
/// boundary in: a caller whose boundary and every window after it have since been removed is
/// therefore refused rather than served an empty last page. The refusal loses no window — the
/// survivors are all before the boundary and a cursorless read reports them — and it is the
/// same answer the offset cursors give a position past the end of what they report.
fn issued_window_ordinal(cursor: &str, last_ordinal: u64) -> Option<u64> {
    let ordinal = canonical_number::<u64>(cursor)?;
    (FIRST_EXCLUSION_WINDOW_ORDINAL..last_ordinal)
        .contains(&ordinal)
        .then_some(ordinal)
}

/// The key `cursor` names as the boundary of a page over a stored, key-ordered collection
/// whose last key is `last_key`, or `None` when it is not a boundary [`page_keyed`] can have
/// minted over the set being read — where `spelled` answers whether a value is spelled the way
/// that collection's keys are.
///
/// A page token is minted by keying on the LAST record the page served, and only while the set
/// still holds a record beyond it, so every key this paging hands out is a key of the
/// collection's own key space with at least one stored key after it: a value `spelled` rejects
/// is no key of that collection at all — the empty payload a bare tag carries is one such — and
/// a key at or past the last stored key bounds no page, the first record being served by the
/// request that states no cursor. A payload outside that range, or spelled a way the
/// collection's keys never are, is therefore a boundary no read of this service issued over
/// the set being read, exactly as for the ordinal and offset cursors
/// ([`issued_window_ordinal`], [`issued_offset`]) — and honouring it would re-serve the first
/// page under a cursor, or report the set as exhausted to a caller whose cursor is not one it
/// was given.
///
/// A key INSIDE that range is honoured whether or not the set still holds the record it keys
/// on, which is what makes a boundary survive the removal of that very record — the property
/// a key cursor is chosen for ([`page_keyed`]). That is also why the LOW end of the range is
/// the key space rather than the set's own first key: these keys are not dense, so a floor
/// read off the stored set would turn a boundary this service really issued into a refusal as
/// soon as the records before it were removed, while the ordinal cursors take their floor from
/// the one position their dense key space starts at.
///
/// The HIGH end is the set as it stands NOW, because that is the only set this read can place
/// a boundary in, exactly as for the ordinal cursor ([`issued_window_ordinal`]): a caller whose
/// boundary and every record after it have since been removed is therefore refused rather than
/// served an empty last page. That refusal loses no record — the survivors are all before the
/// boundary and a cursorless read reports them — while admitting a key past the last one would
/// report any set as exhausted to a caller whose cursor is not one it was given, which is the
/// failure this check exists for. The two ends are asymmetric because the harms are: below the
/// set, honouring costs a re-served first page and refusing costs a resumable listing; at or
/// above it, honouring costs the listing's own termination signal and refusing costs nothing
/// the caller cannot read without a cursor.
fn issued_key<'a>(
    cursor: &'a str,
    last_key: Option<&str>,
    spelled: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    let last = last_key?;
    (cursor < last && spelled(cursor)).then_some(cursor)
}

/// One page of a derived projection, plus the token for the page after it.
///
/// The projections the time-ranged topology reads page over are DERIVED (they are
/// recomputed from the stored SLOs on every call) rather than stored, so a cursor into them
/// is an offset into their deterministic order, not a key of a stored record. Each read
/// passes its own [`SERVICE_OPERATIONS_TOKEN_TAG`]-style tag, so a token is only ever
/// resolved against the projection that issued it; `Err` carries the refusal message for a
/// token that is not one this service issued under that tag — the service's ONE wording for
/// that failure ([`UNISSUED_NEXT_TOKEN`]), so a cursor refused here reads exactly like the
/// same cursor refused by a read whose cursor is a stored key. A read whose projection the
/// REQUEST selects passes that tag with the whole selection bound into it
/// ([`subject_token_tag`]), since the operation alone does not name the projection there.
///
/// The token is opaque but not authenticated, so the POSITION inside the tag is a value a
/// caller may have written: it is honoured only when it is an offset this paging can have
/// minted over the projection being read ([`issued_offset`]), and refused with that same ONE
/// wording otherwise.
pub fn page_of<T>(
    items: Vec<T>,
    tag: &str,
    token: Option<&str>,
    page_size: usize,
) -> Result<(Vec<T>, Option<String>), String> {
    // A page of nothing could not advance its own cursor, so a caller following `NextToken`
    // would loop for ever. A non-positive bound is therefore REFUSED before any read gets
    // here — by the boundary validator for a `MaxResults` whose model states the minimum,
    // and by [`page_bound`] for one whose model states a maximum only — so this floor is
    // only what keeps the invariant local to this function, never a bound a request can
    // reach.
    let page_size = page_size.max(1);
    let offset = match token {
        None => 0,
        Some(token) => decode_cursor(tag, token)
            .and_then(|cursor| issued_offset(&cursor, items.len()))
            .ok_or_else(|| UNISSUED_NEXT_TOKEN.to_string())?,
    };
    let mut page: Vec<T> = items.into_iter().skip(offset).collect();
    let next_token = if page.len() > page_size {
        page.truncate(page_size);
        Some(encode_cursor(tag, &offset.saturating_add(page_size).to_string()))
    } else {
        None
    };
    Ok((page, next_token))
}

/// One page of a STORED, key-ordered collection: its records from `lower` onwards, bounded by
/// `page_size`, together with the KEY of the last record on the page while the collection holds
/// records beyond it (`None` once the page is the last one).
///
/// The page boundary is a KEY of the collection rather than a count of the records already
/// served, which is what makes it survive a write landing between two pages: creating or
/// removing a record moves no other record's key, so the next page can neither skip nor repeat
/// a record the collection still holds. Every read of this service whose page is STORED records
/// takes its page from here — one SLO's exclusion windows
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::page_exclusion_windows`]) and one
/// scope's instrumentation configurations
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::page_instrumentation_configurations`])
/// — so that property is implemented ONCE and a further such read inherits it instead of
/// copying it. [`page_of`] is the offset cursor of the reads whose page is a projection DERIVED
/// on every call instead.
///
/// At most `page_size + 1` records are visited, so both the response and the memory it takes to
/// build are bounded by the page rather than by the size of the collection. `lower` is stated on
/// the type the keys are LOOKED UP by, so a cursor a request carried resolves without being
/// rebuilt as a key.
fn page_keyed<'a, K, Q, V>(
    records: &'a BTreeMap<K, V>,
    lower: Bound<&Q>,
    page_size: usize,
) -> (Vec<&'a V>, Option<&'a K>)
where
    K: Ord + ::std::borrow::Borrow<Q>,
    Q: Ord + ?Sized,
{
    // A page of nothing could not advance its own cursor, so a caller following `NextToken`
    // would loop for ever. Every bound that reaches a read of this service is at least
    // [`MIN_PAGE_BOUND`], so this floor only keeps the invariant local to this function.
    let page_size = page_size.max(1);
    let mut window: Vec<(&K, &V)> = records
        .range::<Q, _>((lower, Bound::Unbounded))
        .take(page_size.saturating_add(1))
        .collect();
    let boundary = if window.len() > page_size {
        window.truncate(page_size);
        window.last().map(|(key, _)| *key)
    } else {
        None
    };
    (
        window.into_iter().map(|(_, record)| record).collect(),
        boundary,
    )
}

/// The `NextToken` tag a read whose projection the REQUEST selects issues its page tokens
/// under: the read's own [`SERVICE_OPERATIONS_TOKEN_TAG`]-style tag, plus a fingerprint of the
/// selection the page was taken over.
///
/// A [`page_of`] cursor is an offset within ONE ordered set, so a token issued over another
/// selection would silently skip that selection's first entries if it were honoured. Binding
/// the selection into the tag makes such a token fail to decode, so it is refused like any
/// other token this service did not issue — the rule the scoped instrumentation reads already
/// hold ([`instrumentation_token_tag`]), stated once here for every read whose set the request
/// picks out. Every request member the projection is a function of belongs in `subject`: the
/// subject the request names, the filters that narrow it ([`filtered_window_token_tag`]) and
/// the period it covers ([`window_fields`]) — and nothing else, so a member that changes only
/// how a reported entry is rendered leaves a token honoured.
///
/// `subject` is the selection's RESOLVED identity — what the store holds it under, not how the
/// request spelled it — so a token handed back for a read that named the subject one way
/// still resumes a read naming the SAME subject another way. A field list is injectively
/// rendered ([`stable_hash`]), so no two different selections share a tag; a list of variable
/// length is preceded by its length, so where one list ends and the next begins is readable
/// off the fields themselves rather than being a property of the values in them.
pub fn subject_token_tag(tag: &str, subject: &[Option<&str>]) -> String {
    format!("{tag}{}/", stable_hash(subject))
}

/// `fields` as the borrowed field list [`subject_token_tag`] fingerprints, for a selection
/// whose fields are assembled as owned values.
fn borrowed_fields(fields: &[Option<String>]) -> Vec<Option<&str>> {
    fields.iter().map(|field| field.as_deref()).collect()
}

/// The fields one time period contributes to the tag of a cursor paged over it: the period's
/// start and its end, each on the one key instants are compared on ([`instant_nanos`]).
///
/// A read whose projection is what the store holds OVER A PERIOD derives a different ordered
/// set for a different period, so the period is part of what picks that set out: an offset
/// counted in one period's set would skip or repeat entries if it were honoured in another's,
/// which is the rule [`subject_token_tag`] states. The chronological read binds its own window
/// for the same reason, inside the cursor rather than in the tag ([`StatusEventCursor`]),
/// because a window it was never given is one it derived.
fn window_fields(window_start: &DateTime, window_end: &DateTime) -> [String; 2] {
    [window_start, window_end].map(|instant| instant_nanos(instant).to_string())
}

/// [`subject_token_tag`] for a read whose projection is a PERIOD of what this store holds and
/// which names no subject of its own: the period is the whole selection, so it alone is bound
/// into the tag ([`window_fields`]).
pub fn window_token_tag(tag: &str, window_start: &DateTime, window_end: &DateTime) -> String {
    let window = window_fields(window_start, window_end);
    subject_token_tag(tag, &[Some(window[0].as_str()), Some(window[1].as_str())])
}

/// [`subject_token_tag`] for a read whose subject is the entity a key-attribute map selects,
/// over the period the request names.
///
/// The whole map is the subject's identity, because the projection is a function of all of it:
/// a stored entity is selected when it carries every entry the map states
/// ([`attributes_contain`]), so two maps that differ in one entry select different sets. The
/// entries are fingerprinted in key order, since a map carries no order of its own while one
/// subject must yield the same tag on every call, and the count comes first so that the
/// attributes cannot run into the period's fields.
pub fn entity_token_tag(
    tag: &str,
    attributes: &HashMap<String, String>,
    window_start: &DateTime,
    window_end: &DateTime,
) -> String {
    let mut fields = attribute_fields(attributes);
    fields.extend(window_fields(window_start, window_end).into_iter().map(Some));
    subject_token_tag(tag, &borrowed_fields(&fields))
}

/// The fields one `Attributes` map contributes to the tag of a cursor paged over the set it
/// selects: how many entries it holds, then those entries in key order.
///
/// The entries are fingerprinted in KEY ORDER, since a map carries no order of its own while
/// one selection must yield the same tag on every call, and the count comes first so that
/// where the map's entries end and the fields after them begin is readable off the field list
/// itself rather than being a property of the values in it ([`subject_token_tag`]).
fn attribute_fields(attributes: &HashMap<String, String>) -> Vec<Option<String>> {
    let mut entries: Vec<(&str, &str)> = attributes
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    entries.sort_unstable();
    let mut fields: Vec<Option<String>> = vec![Some(entries.len().to_string())];
    for (key, value) in entries {
        fields.push(Some(key.to_string()));
        fields.push(Some(value.to_string()));
    }
    fields
}

/// [`attribute_fields`] for a selector the request may leave out: ONE absent field where no
/// map is stated, so a selection that states no map renders a different field list from one
/// that states any map at all.
fn optional_attribute_fields(attributes: Option<&HashMap<String, String>>) -> Vec<Option<String>> {
    attributes.map_or_else(|| vec![None], attribute_fields)
}

/// [`subject_token_tag`] for a read whose selection is a period plus the attribute filters that
/// narrow it.
///
/// The filters are part of the selection, since each one is a further condition the reported
/// services pass ([`passes_attribute_filter`]), so an offset counted in a filtered set is not
/// an offset into an unfiltered one. They are fingerprinted in a CANONICAL form — each
/// filter's accepted values sorted and deduplicated, then the filters themselves — because the
/// set they select does not depend on the order they were written in, nor on a value or a
/// filter named twice: two requests stating ONE selection resume each other.
pub fn filtered_window_token_tag(
    tag: &str,
    window_start: &DateTime,
    window_end: &DateTime,
    filters: &[AttributeFilter],
) -> String {
    let mut canonical: Vec<(&str, Vec<&str>)> = filters
        .iter()
        .map(|filter| {
            let mut values: Vec<&str> = filter
                .attribute_filter_values
                .iter()
                .map(String::as_str)
                .collect();
            values.sort_unstable();
            values.dedup();
            (filter.attribute_filter_name.as_str(), values)
        })
        .collect();
    canonical.sort_unstable();
    canonical.dedup();

    let mut fields: Vec<Option<String>> = vec![Some(canonical.len().to_string())];
    for (name, values) in canonical {
        fields.push(Some(name.to_string()));
        fields.push(Some(values.len().to_string()));
        fields.extend(values.into_iter().map(|value| Some(value.to_string())));
    }
    fields.extend(
        window_fields(window_start, window_end)
            .into_iter()
            .map(Some),
    );
    subject_token_tag(tag, &borrowed_fields(&fields))
}

/// [`subject_token_tag`] for the objective listing, whose ordered set is every stored
/// objective that passes the selectors the request supplied.
///
/// Each selector is a further condition a reported objective meets, so two requests stating
/// different selectors page different sets and a cursor counted in one of them is not a
/// boundary in the other — the rule [`subject_token_tag`] states, which here binds the whole
/// selection rather than a period. A selector LEFT OUT narrows nothing, which is a selection
/// of its own and renders an absent field ([`optional_attribute_fields`]) rather than the
/// fields of an empty one.
///
/// Each selector is fingerprinted on the identity the matcher resolves it to: the entries of
/// an `Attributes` map in key order, a dependency on both of its members, a metric source on
/// both of its members too — its key attributes and, as a half the request may leave out, the
/// additional attributes beside them ([`same_metric_source`]) — and the metric source types
/// sorted and deduplicated, since an objective is reported when its own type is ANY of them
/// and so neither their order nor a type named twice picks out another set.
///
/// The account the request names is NOT part of the tag: an id naming another account selects
/// no stored objective at all ([`names_other_account`]), so that listing reports an empty set
/// in full and holds no entry a resumed cursor could skip — and an id naming THIS account
/// selects the same set as no id at all, as the sibling account-scoped reads also resolve it.
pub fn slo_selection_token_tag(
    key_attributes: Option<&HashMap<String, String>>,
    operation_name: Option<&str>,
    dependency: Option<&DependencyConfig>,
    metric_source: Option<&MetricSource>,
    metric_source_types: Option<&[String]>,
) -> String {
    let mut fields = optional_attribute_fields(key_attributes);
    fields.push(operation_name.map(ToString::to_string));
    match dependency {
        None => fields.push(None),
        Some(dependency) => {
            fields.extend(attribute_fields(&dependency.dependency_key_attributes));
            fields.push(Some(dependency.dependency_operation_name.clone()));
        }
    }
    fields.extend(optional_attribute_fields(
        metric_source.map(|source| &source.metric_source_key_attributes),
    ));
    fields.extend(optional_attribute_fields(
        metric_source.and_then(|source| source.metric_source_attributes.as_ref()),
    ));
    match metric_source_types {
        None => fields.push(None),
        Some(kinds) => {
            let mut kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
            kinds.sort_unstable();
            kinds.dedup();
            fields.push(Some(kinds.len().to_string()));
            fields.extend(kinds.into_iter().map(|kind| Some(kind.to_string())));
        }
    }
    subject_token_tag(NEXT_TOKEN_TAG, &borrowed_fields(&fields))
}

/// Whether `candidate` holds every entry of `filter` with the same value.
///
/// Attribute filters on this API name the entity to match, and an entity is identified by
/// as few as one of its up to four `Attributes` entries (`length 1..4`), so a filter
/// matches when it is a subset of the stored attributes — exact equality is the case where
/// the caller names them all.
pub fn attributes_contain(
    candidate: Option<&HashMap<String, String>>,
    filter: &HashMap<String, String>,
) -> bool {
    let Some(candidate) = candidate else {
        return filter.is_empty();
    };
    filter
        .iter()
        .all(|(key, value)| candidate.get(key) == Some(value))
}

/// The fullest of `candidates` that names the entity `named` names, or `None` when none of
/// them does.
///
/// An entity is identified by as few as one of its up to four `Attributes` entries
/// ([`attributes_contain`]), so a candidate holding every entry of `named` names the SAME
/// entity as `named`, and the LONGEST of those candidates is the fullest identity the
/// candidates hold for it. A `named` map that states NO attribute identifies no entity —
/// every candidate holds its entries — so there is no identity to resolve.
///
/// Equal-length candidates are two identities neither of which contains the other, i.e. two
/// distinct entities both answering to `named`; the first of them is kept, which is the order
/// the store holds its records in.
///
/// Single-homed so that every resolution of an entity's ONE identity applies one rule: the
/// identity of a DISCOVERED entity
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::observed_identity`]) and the
/// identity of an entity the store knows as the far end of a recorded edge
/// ([`far_end_identity`]) differ only in WHICH records are evidence of the entity, never in
/// how the fullest of them is picked.
fn fullest_identity<'a>(
    candidates: &'a [HashMap<String, String>],
    named: &HashMap<String, String>,
) -> Option<&'a HashMap<String, String>> {
    if named.is_empty() {
        return None;
    }
    candidates
        .iter()
        .filter(|candidate| attributes_contain(Some(candidate), named))
        .fold(None, |fullest: Option<&HashMap<String, String>>, candidate| {
            match fullest {
                Some(current) if current.len() >= candidate.len() => Some(current),
                _ => Some(candidate),
            }
        })
}

/// The ONE identity this store holds for the entity a recorded edge's FAR end names — the
/// entity that was INVOKED — which is `named` itself when none of the identities the stored
/// edges record (`candidates`,
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::recorded_far_identities`]) holds a
/// fuller one.
///
/// The far end of an edge is not an entity this emulator discovered: "the edge is evidence
/// that the monitored entity calls it, not that Application Signals discovered and
/// instrumented the callee"
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::observed_services`], which is why
/// no such entity is a reported service). The records that are evidence OF IT are therefore
/// the recorded edges that name it, and the identity it is reported and dimensioned by is the
/// fullest identity any of them holds for it ([`fullest_identity`]) — exactly as a discovered
/// entity is reported by the fullest identity the SLOs that monitor it hold
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::observed_identity`]).
///
/// Resolving it is what keeps the two views of one edge from disagreeing: an edge is recorded
/// by each SLO that measures it, and two such records may name the far end by identities of
/// different fullness, which are the SAME entity ([`attributes_contain`]). Reporting each
/// record's own identity would list that one entity twice — once under a strict subset of the
/// identity the other entry carries, which is the reporting
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::observed_services`] refuses for a
/// service, and which need not even carry the minimum identity a read of that entity must name
/// it by ([`entity_identity_refusal`]) — and would make the two views disagree about how many
/// edges there are.
fn far_end_identity(
    candidates: &[HashMap<String, String>],
    named: &HashMap<String, String>,
) -> HashMap<String, String> {
    fullest_identity(candidates, named)
        .cloned()
        .unwrap_or_else(|| named.clone())
}

/// The `Attributes` entry that says WHICH KIND of object a key-attribute map identifies:
/// "`Type` designates the type of object this is"
/// (`smithy_query(query="doc", shape="GetServiceInput", member="KeyAttributes")`).
pub const TYPE_ATTRIBUTE: &str = "Type";

/// The `Attributes` entry that NAMES the object: "`Name` specifies the name of the object"
/// (`smithy_query(query="doc", shape="GetServiceInput", member="KeyAttributes")`).
const NAME_ATTRIBUTE: &str = "Name";

/// The `Attributes` entry that says where the object runs: "`Environment` specifies the
/// location where this object is hosted" (same member doc as [`NAME_ATTRIBUTE`]).
const ENVIRONMENT_ATTRIBUTE: &str = "Environment";

/// The `Attributes` entry that says what KIND of resource an object is: "`ResourceType`
/// specifies the type of the resource" (same member doc as [`NAME_ATTRIBUTE`]).
const RESOURCE_TYPE_ATTRIBUTE: &str = "ResourceType";

/// The `Attributes` entry that picks out WHICH resource of that kind: "`Identifier`
/// identifies the resource objects of this resource" (same member doc as
/// [`NAME_ATTRIBUTE`]).
const IDENTIFIER_ATTRIBUTE: &str = "Identifier";

/// The `Attributes` entry that says which ACCOUNT the entity is in: "`AwsAccountId` specifies
/// the account where this object is in" (`smithy_query(query="doc",
/// shape="ListEntityEventsInput", member="Entity")`, the one entity map whose doc lists it).
///
/// An account id, not an identifying attribute: it says WHERE the entity lives rather than
/// WHICH entity it is, and no identity this store holds a record under carries such an entry
/// ([`service_entity_attributes`] and an objective's own `KeyAttributes` are built from the
/// identifying attributes alone). It is therefore read as an account
/// ([`entity_account_id`], [`names_other_account`]) and left out of the identity records are
/// matched on ([`entity_identity_attributes`]).
const ACCOUNT_ID_ATTRIBUTE: &str = "AwsAccountId";

/// The entity types whose identity NAMES the object: `Name` "is used only if the value of
/// the `Type` field is `Service`, `RemoteService`, or `AWS::Service`"
/// (`smithy_query(query="doc", shape="GetServiceInput", member="KeyAttributes")`).
const NAMED_ENTITY_TYPES: [&str; 3] = [SERVICE_ENTITY_TYPE, "RemoteService", "AWS::Service"];

/// The entity types whose identity picks out a RESOURCE: `ResourceType` and `Identifier` are
/// used "only when the value of the `Type` field is `Resource` or `AWS::Resource`" (same
/// member doc as [`NAMED_ENTITY_TYPES`]).
const RESOURCE_ENTITY_TYPES: [&str; 2] = ["Resource", "AWS::Resource"];

/// The entity types `Type` designates, in sorted order: the two groups above together, so
/// the set of types and the attributes each type makes applicable cannot disagree.
///
/// The `KeyAttributes` doc names them as the values the rest of the map is interpreted
/// against ([`NAMED_ENTITY_TYPES`], [`RESOURCE_ENTITY_TYPES`]; the same text on
/// `ListServiceOperationsInput`, `ListServiceDependenciesInput` and
/// `ListServiceDependentsInput`). A value outside this set designates no kind of object, so
/// none of the other attributes has a meaning.
pub const ENTITY_TYPES: [&str; 5] = [
    RESOURCE_ENTITY_TYPES[1], // AWS::Resource
    NAMED_ENTITY_TYPES[2],    // AWS::Service
    NAMED_ENTITY_TYPES[1],    // RemoteService
    RESOURCE_ENTITY_TYPES[0], // Resource
    NAMED_ENTITY_TYPES[0],    // Service
];

/// The attributes BESIDE `Type` that an identity of `entity_type` is read by: the ones the
/// member's own per-`Type` list makes applicable to it.
///
/// A service is named and placed (`Name` and `Environment`); a resource is picked out by
/// what kind of resource it is and which one (`ResourceType` and `Identifier`), for which
/// `Name` has no meaning — which is why the pair depends on the `Type`. The resource pair is
/// the identity the member doc writes out for a resource: `{ "Type": "AWS::Resource",
/// "ResourceType": "AWS::DynamoDB::Table", "Identifier": "Customers" }`
/// (`smithy_query(query="doc", shape="ListEntityEventsInput", member="Entity")`), beside the
/// service identity the same doc writes as `{ "Type": "Service", "Name": "visits-service",
/// "Environment": "petclinic-test" }`.
fn identifying_attributes(entity_type: &str) -> [&'static str; 2] {
    if RESOURCE_ENTITY_TYPES.contains(&entity_type) {
        [RESOURCE_TYPE_ATTRIBUTE, IDENTIFIER_ATTRIBUTE]
    } else {
        [NAME_ATTRIBUTE, ENVIRONMENT_ATTRIBUTE]
    }
}

/// The account id an entity map states, or `None` when it states none — the one place an
/// `Attributes` entry is read as an ACCOUNT rather than as part of an identity
/// ([`ACCOUNT_ID_ATTRIBUTE`]).
///
/// Hand the answer to [`names_other_account`], exactly as a reader whose account id arrives
/// in a member of its own does: an account named inside a map and one named beside it are the
/// same fact, so the reads that must agree about which account a request names cannot drift
/// apart.
pub fn entity_account_id(entity: &HashMap<String, String>) -> Option<&str> {
    entity.get(ACCOUNT_ID_ATTRIBUTE).map(String::as_str)
}

/// The identity an entity map names a stored record BY: its entries except the account id,
/// which is an account rather than an identifying attribute ([`ACCOUNT_ID_ATTRIBUTE`]).
///
/// This is the identity a record is matched against ([`attributes_contain`]) and the resolved
/// subject a page cursor over that selection is tagged with ([`subject_token_tag`]): the
/// account the entity is in is decided once, as an account ([`entity_account_id`] through
/// [`names_other_account`]), so naming the account this store holds selects exactly what
/// naming no account selects instead of selecting nothing.
pub fn entity_identity_attributes(entity: &HashMap<String, String>) -> HashMap<String, String> {
    entity
        .iter()
        .filter(|(attribute, _)| attribute.as_str() != ACCOUNT_ID_ATTRIBUTE)
        .map(|(attribute, value)| (attribute.clone(), value.clone()))
        .collect()
}

/// The refusal a key-attribute map that is supposed to IDENTIFY an entity earns, or `None`
/// when it identifies one.
///
/// The map must first designate the KIND of object it names, with `Type` "designating the
/// type of object this is" and carrying one of the documented values ([`ENTITY_TYPES`]): a
/// map with no `Type`, or with a `Type` outside that set, names no entity at all and is
/// refused with the modelled `ValidationException` rather than answered with the caller's own
/// map echoed back as a discovered entity.
///
/// It must then carry the MINIMUM IDENTITY the member documents — "You must specify at least
/// the `Type`, `Name`, and `Environment` attributes" — read against the member's own
/// per-`Type` list: the attributes that `Type` makes applicable
/// ([`identifying_attributes`]) are the ones saying WHICH object of that kind the read is
/// about, so a map missing any of them pins down no single entity, however many of the
/// others it carries.
///
/// Shared so the reads that take this member as the entity to report on — `GetService`,
/// `ListServiceOperations`, `ListServiceDependencies` and `ListServiceDependents` — cannot
/// disagree about which identities are serviceable. The member is NOT the same rule
/// everywhere it appears: `ListServiceLevelObjectives` takes an OPTIONAL filter whose doc
/// drops the "must specify" sentence, an SLO's own `KeyAttributes` records the object the SLO
/// relates to, and `ListEntityEvents` takes the entity to report on under a member whose doc
/// carries no "must specify" sentence either — so none of them is subject to the minimum,
/// and `ListEntityEvents` reads its entity by the type rule alone
/// ([`entity_type_refusal_at`]). A caller may still name as few attributes BEYOND the minimum
/// as pins the entity down ([`attributes_contain`]).
pub fn entity_identity_refusal(key_attributes: &HashMap<String, String>) -> Option<String> {
    let entity_type = match entity_type_clause(KEY_ATTRIBUTES_MEMBER_PATH, key_attributes) {
        Err(clause) => return Some(validation_envelope(&[clause])),
        Ok(entity_type) => entity_type,
    };
    let clauses: Vec<String> = identifying_attributes(entity_type)
        .into_iter()
        .filter(|attribute| !key_attributes.contains_key(*attribute))
        .map(|attribute| {
            constraint_clause(
                KEY_ATTRIBUTES_MEMBER_PATH,
                &format!("Member must contain the entity identity attribute '{attribute}'"),
            )
        })
        .collect();
    (!clauses.is_empty()).then(|| validation_envelope(&clauses))
}

/// The refusal a map that must designate the KIND of object it names earns, or `None` when it
/// does — the type half of [`entity_identity_refusal`] on its own, for an operation whose
/// member documents no minimum identity: `ListEntityEvents` takes the entity to report on as
/// `Entity` ([`ENTITY_MEMBER_PATH`]), whose doc lists the same attributes per `Type` without
/// the "must specify" sentence.
///
/// The NAME is a parameter because a refusal states the member the request carries, and
/// naming a member the request never sent would leave a client with nothing to fix.
pub fn entity_type_refusal_at(
    member_path: &str,
    attributes: &HashMap<String, String>,
) -> Option<String> {
    entity_type_clause(member_path, attributes)
        .err()
        .map(|clause| validation_envelope(&[clause]))
}

/// The entity type `attributes` designates, or the clause a map that designates none earns.
///
/// The ONE place the `Type` entry is read, so every operation that reads an entity by this
/// map agrees on which values designate a kind of object.
fn entity_type_clause<'attributes>(
    member_path: &str,
    attributes: &'attributes HashMap<String, String>,
) -> Result<&'attributes str, String> {
    match attributes.get(TYPE_ATTRIBUTE) {
        None => Err(constraint_clause(
            member_path,
            &format!("Member must contain the entity type attribute '{TYPE_ATTRIBUTE}'"),
        )),
        Some(entity_type) if !ENTITY_TYPES.contains(&entity_type.as_str()) => {
            Err(constraint_clause(
                &format!("{member_path}.{TYPE_ATTRIBUTE}"),
                &format!(
                    "Member must satisfy enum value set: [{}]",
                    ENTITY_TYPES.join(", ")
                ),
            ))
        }
        Some(entity_type) => Ok(entity_type.as_str()),
    }
}

/// The member path a refusal of the entity-identity map names: the member in the lowerCamel
/// casing this service's `ValidationException` envelope states a member with, as the
/// framework-level refusal of the same member's modelled bounds already does.
const KEY_ATTRIBUTES_MEMBER_PATH: &str = "keyAttributes";

/// [`KEY_ATTRIBUTES_MEMBER_PATH`], for the `Entity` member `ListEntityEvents` takes the
/// entity to report on under.
pub const ENTITY_MEMBER_PATH: &str = "entity";

/// Whether two `DependencyConfig`s name the same dependency. The shape carries no
/// `PartialEq`, so the comparison is written out over its two members.
pub fn same_dependency(left: Option<&DependencyConfig>, right: &DependencyConfig) -> bool {
    left.is_some_and(|left| {
        left.dependency_operation_name == right.dependency_operation_name
            && left.dependency_key_attributes == right.dependency_key_attributes
    })
}

/// Whether a stored `MetricSource` passes a `MetricSource` selector.
///
/// The shape carries TWO members — the key attributes that identify the source and the
/// additional attributes beside them — and each member the selector states is a further
/// condition the stored source meets, compared by the rule of its own type: an `Attributes`
/// map is matched by containment, since a map names as few of its entries as pins the source
/// down ([`attributes_contain`]). A member the selector LEAVES OUT narrows nothing, which is
/// the only half of a structure-valued selector that does; an absent stored source carries no
/// entry at all, so it passes only a selector that names none.
pub fn same_metric_source(stored: Option<&MetricSource>, filter: &MetricSource) -> bool {
    attributes_contain(
        stored.map(|source| &source.metric_source_key_attributes),
        &filter.metric_source_key_attributes,
    ) && filter
        .metric_source_attributes
        .as_ref()
        .is_none_or(|additional| {
            attributes_contain(
                stored.and_then(|source| source.metric_source_attributes.as_ref()),
                additional,
            )
        })
}

// =============================================================================
// The exclusion windows configured on an SLO
// =============================================================================

/// The most exclusion windows one SLO holds.
///
/// The API states the limit per SLO rather than per request:
/// `BatchUpdateExclusionWindows.AddExclusionWindows` documents "You can add up to 10
/// exclusion windows per SLO", and the modelled `ExclusionWindows` list caps both that
/// member and the `ListServiceLevelObjectiveExclusionWindows` output at 10 entries \u2014 so a
/// set beyond this size could not be reported back at all.
pub const MAX_EXCLUSION_WINDOWS_PER_SLO: usize = 10;

/// The most per-SLO failures one `BatchUpdateExclusionWindows` response reports: the modelled
/// maximum of `BatchUpdateExclusionWindowsErrors` (`len ..10`), while `SloIds` admits up to
/// 50 ids.
///
/// Which SLOs a request WAS applied to is reported in full by the response's `SloIds`, so an
/// id missing from that list was not updated whether or not the capped `Errors` list also
/// names it.
pub const MAX_EXCLUSION_WINDOW_ERRORS: usize = 10;

/// The largest page `ListServiceLevelObjectiveExclusionWindows` returns when the caller
/// names no `MaxResults`.
///
/// The member's doc sentence says "the default of 50 is used", but its modelled
/// `@range` is `1..10` and the `ExclusionWindows` list it fills holds at most 10 entries, so
/// 50 is not a page this operation can ever return: the effective default is the largest page
/// the model admits, which is also the whole window set of one SLO
/// ([`MAX_EXCLUSION_WINDOWS_PER_SLO`]).
pub const DEFAULT_EXCLUSION_WINDOW_PAGE: i32 = MAX_EXCLUSION_WINDOWS_PER_SLO as i32;

/// [`SERVICE_OPERATIONS_TOKEN_TAG`], for `ListServiceLevelObjectiveExclusionWindows`, whose
/// projection is one SLO's own window set — so every token it issues carries this tag with
/// that SLO bound into it ([`subject_token_tag`]).
pub const EXCLUSION_WINDOWS_TOKEN_TAG: &str = "exclusion-windows/1/";

/// The ordinal the first window added to an SLO is stored under
/// ([`next_exclusion_window_ordinal`]).
///
/// The line starts at ONE, so zero is an ordinal no stored window ever holds and therefore a
/// page boundary this service cannot have issued ([`issued_window_ordinal`]): the first window
/// of a set is served by the request that states no cursor, so a cursor of zero would re-serve
/// that first page under a token.
const FIRST_EXCLUSION_WINDOW_ORDINAL: u64 = 1;

/// Why one SLO named by a `BatchUpdateExclusionWindows` request was not updated.
///
/// The failure is per SLO because the request is: one set of windows is applied to each id
/// in `SloIds` independently, and the response reports the ids it was applied to beside an
/// `Errors` entry for each one it was not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusionWindowFailure {
    /// The id names no stored SLO.
    NoSuchSlo,
    /// The additions would take this SLO past [`MAX_EXCLUSION_WINDOWS_PER_SLO`] windows.
    TooManyWindows,
}

impl ExclusionWindowFailure {
    /// The `ErrorCode` this failure reports.
    ///
    /// `ExclusionWindowErrorCode` is an unconstrained string in the model
    /// (`smithy_query(shape="ExclusionWindowErrorCode")` \u2014 a `string` with no enum), so the
    /// code reported is the NAME of the modelled exception a single-SLO call would have been
    /// refused with, which is the same token a client already branches on when it reads an
    /// error envelope — and the exception a request whose every id failed is answered with,
    /// so the two paths cannot disagree about one failure.
    pub fn error_code(self) -> &'static str {
        match self {
            Self::NoSuchSlo => "ResourceNotFoundException",
            Self::TooManyWindows => "ValidationException",
        }
    }

    /// The `ErrorMessage` this failure reports for the SLO `id` named, in `region`.
    ///
    /// A not-found failure is worded by [`slo_not_found`] and a constraint failure by
    /// [`validation_envelope`] over [`constraint_clause`], so a per-item error reads exactly
    /// like the exception the same failure earns on its own, and neither echoes the window
    /// configuration the request carried.
    pub fn error_message(self, region: &str, id: &str) -> String {
        match self {
            Self::NoSuchSlo => slo_not_found(region, id).message,
            Self::TooManyWindows => validation_envelope(&[constraint_clause(
                "addExclusionWindows",
                &format!(
                    "Member must not take the SLO past {MAX_EXCLUSION_WINDOWS_PER_SLO} \
                     exclusion windows"
                ),
            )]),
        }
    }
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// One page of the exclusion windows configured on the SLO an `Id` names, in the order
    /// they were added, starting strictly after the window the `token` bounds the page at,
    /// together with the token for the page after it (`None` once the page is the last one).
    ///
    /// An SLO holding no windows and an id naming no SLO both answer an empty page — a caller
    /// tells the two apart from [`Self::resolve`], which is what the reads refuse an unknown id
    /// by.
    ///
    /// The cursor is the stored KEY of the last window the previous page served (the ordinal
    /// it was added under, [`next_exclusion_window_ordinal`]) rather than a count of the
    /// windows already served, so a window removed between two pages cannot make the next page
    /// skip or repeat a window the SLO still holds: the boundary names a position in the stored
    /// order, and removing a window leaves every other window's ordinal exactly where it was.
    /// That is the property the stored instrumentation set already gets from its own key cursor
    /// ([`Self::page_instrumentation_configurations`]); [`page_of`] is the offset cursor of the
    /// reads whose projection is DERIVED on every call instead.
    ///
    /// At most `page_size + 1` windows are visited, so the response is bounded by the page
    /// rather than by the number of stored windows ([`page_keyed`]).
    ///
    /// The token is opaque but not authenticated, so the key inside the tag is a value a caller
    /// may have written: it is honoured only when it is a boundary this paging can have minted
    /// over the set being read ([`issued_window_ordinal`]), and `Err` otherwise carries the
    /// service's ONE wording for a token it did not issue ([`UNISSUED_NEXT_TOKEN`]).
    pub fn page_exclusion_windows(
        &self,
        id: &str,
        token_tag: &str,
        token: Option<&str>,
        page_size: usize,
    ) -> Result<(Vec<ExclusionWindow>, Option<String>), String> {
        let stored = slo_name_of(id).and_then(|name| self.exclusion_windows.get(name));
        // The ordinal of the LAST window this SLO holds, and so the one position past which
        // this paging can have minted no boundary over the set being read. An SLO holding no
        // windows has minted none at all.
        let last_ordinal = stored
            .and_then(|windows| windows.keys().next_back().copied())
            .unwrap_or(0);
        let boundary = match token {
            None => None,
            Some(token) => Some(
                decode_cursor(token_tag, token)
                    .and_then(|cursor| issued_window_ordinal(&cursor, last_ordinal))
                    .ok_or_else(|| UNISSUED_NEXT_TOKEN.to_string())?,
            ),
        };
        let Some(windows) = stored else {
            // An SLO holding no windows reports none, and the only cursor that could have
            // reached this line over an empty set was refused just above.
            return Ok((Vec::new(), None));
        };
        let lower = match &boundary {
            Some(ordinal) => Bound::Excluded(ordinal),
            None => Bound::Unbounded,
        };
        let (page, next) = page_keyed(windows, lower, page_size);
        Ok((
            page.into_iter().cloned().collect(),
            next.map(|ordinal| encode_cursor(token_tag, &ordinal.to_string())),
        ))
    }

    /// Remove `remove` from the window set of the SLO an `Id` names and then add `add` to it,
    /// or answer the single [`ExclusionWindowFailure`] that left the SLO untouched.
    ///
    /// Removals are applied BEFORE additions, so one call can replace a window: the
    /// configuration being removed frees its slot before the additions are counted against
    /// [`MAX_EXCLUSION_WINDOWS_PER_SLO`].
    ///
    /// A window is named by its configuration, which is the identity a removal uses, so the
    /// set holds each configuration once and each half of the update is a SET operation on
    /// that set: adding a configuration the SLO already holds changes nothing (and consumes no
    /// slot), and removing one it does not hold takes nothing away. Neither is a failure of
    /// the id — the sentence `RemoveExclusionWindows` carries ("The window configuration must
    /// match an existing exclusion window") says WHICH window an entry names, not that naming
    /// none refuses the call — so a reconciling caller that states its desired add/remove set
    /// converges: the SLO ends up holding the additions and not the removals, whatever it held
    /// before.
    ///
    /// The whole update is applied to a COPY of the set and stored only once every window has
    /// been applied, so an SLO that earns a failure is left exactly as it was.
    ///
    /// Every window in `add` must already be in the form the service stores
    /// ([`stored_exclusion_window`]): the server-defaulted members are settled once per call,
    /// so every SLO one request names gains the same window.
    ///
    /// An addition takes the next ORDINAL of the set it joins and a removal gives its own up
    /// without moving any other ([`next_exclusion_window_ordinal`]), so the set keeps the order
    /// the windows were added in while every surviving window keeps the key a page cursor may
    /// already have been issued on ([`Self::page_exclusion_windows`]).
    pub fn update_exclusion_windows(
        &mut self,
        id: &str,
        add: &[ExclusionWindow],
        remove: &[ExclusionWindow],
    ) -> Result<(), ExclusionWindowFailure> {
        let Some(name) = slo_name_of(id).filter(|name| self.slos.contains_key(*name)) else {
            return Err(ExclusionWindowFailure::NoSuchSlo);
        };

        let mut windows = self.exclusion_windows.get(name).cloned().unwrap_or_default();
        for window in remove {
            // A removal entry that matches no stored window takes nothing away, and is not a
            // failure of this id: the set simply does not hold that configuration, which is
            // the state the caller asked for.
            let Some(at) = windows
                .iter()
                .find(|(_, stored)| same_exclusion_window(stored, window))
                .map(|(ordinal, _)| *ordinal)
            else {
                continue;
            };
            windows.remove(&at);
        }
        for window in add {
            if windows
                .values()
                .any(|stored| same_exclusion_window(stored, window))
            {
                continue;
            }
            if windows.len() >= MAX_EXCLUSION_WINDOWS_PER_SLO {
                return Err(ExclusionWindowFailure::TooManyWindows);
            }
            windows.insert(next_exclusion_window_ordinal(&windows), window.clone());
        }

        if windows.is_empty() {
            self.exclusion_windows.remove(name);
        } else {
            self.exclusion_windows.insert(name.to_string(), windows);
        }
        Ok(())
    }
}

/// The ordinal the next window added to `windows` is stored under: one past the highest one
/// that set holds, and [`FIRST_EXCLUSION_WINDOW_ORDINAL`] for a set holding none.
///
/// Counting past the HIGHEST ordinal rather than the window count is what keeps the ordinals of
/// one set increasing in the order the windows were added while a removal is only ever the loss
/// of one key: an addition never takes an ordinal a stored window already holds, and never
/// takes one ahead of a window added before it, so a page cursor keyed on an ordinal stays the
/// boundary it was issued as ([`ComAmazonawsApplicationsignalsApplicationsignalsState::page_exclusion_windows`]).
fn next_exclusion_window_ordinal(windows: &BTreeMap<u64, ExclusionWindow>) -> u64 {
    windows
        .keys()
        .next_back()
        .map_or(FIRST_EXCLUSION_WINDOW_ORDINAL, |highest| {
            highest.saturating_add(1)
        })
}

/// `window` in the form this service STORES it: every member as the addition stated it, and a
/// `StartTime` the addition omitted settled to the instant the call is served.
///
/// `ExclusionWindow.StartTime` is optional and "defaults to current time if not specified"
/// (`smithy_query(query="doc", shape="ExclusionWindow", member="StartTime")`), so an
/// omission is a request for the time of the call rather than for a window with no start: the
/// stored window — and so every later read of it — reports when the exclusion begins, and the
/// period it excludes is anchored at that instant.
///
/// The defaulted instant keeps its SUB-SECOND part, settled at the resolution an AWS
/// `Timestamp` crosses the wire at: epoch seconds carrying milliseconds
/// ([`wire_instant`]). An instant the service picks is reported at the resolution
/// the protocol can carry it at, so the window a read reports begins where the exclusion
/// begins rather than at the top of the second it began in.
///
/// Settling it at the wire's own resolution — rather than keeping the clock's nanoseconds —
/// is also what keeps the set a `ListServiceLevelObjectiveExclusionWindows` reports a set a
/// `RemoveExclusionWindows` can name back member for member ([`same_exclusion_window`]): the
/// stored instant is one a response renders in full.
///
/// A STATED `StartTime` is stored exactly as sent — the default settles an omission, it never
/// re-times an instant the caller chose.
pub fn stored_exclusion_window(window: &ExclusionWindow, served_at: DateTime) -> ExclusionWindow {
    ExclusionWindow {
        start_time: Some(window.start_time.unwrap_or(wire_instant(served_at))),
        ..window.clone()
    }
}

/// Nanoseconds in one millisecond, which is the resolution an instant this service settles
/// itself is stated and compared at.
///
/// A `Timestamp` on this service's protocols is epoch seconds with a fractional part, and AWS
/// renders that fraction to the millisecond, which is also the resolution the recorded
/// responses of this service carry. It is finer than any `Window.DurationUnit` the model
/// admits, so nothing a caller states is lost by it, and it is the finest resolution a value
/// survives the wire at: an epoch-seconds `Timestamp` is a JSON NUMBER, so the sub-millisecond
/// digits of an instant do not come back from a client unchanged.
pub const NANOS_PER_MILLISECOND: u32 = 1_000_000;

/// The millisecond `at` falls in, nearest-rounded: the key two `Timestamp` values are the same
/// instant by once one of them has crossed the wire.
///
/// NEAREST, because the comparison's job is to recognise one instant on both sides of a
/// round trip: a millisecond this service rendered comes back from a client as the nearest
/// `f64` to it, whose sub-millisecond digits fall on either side of the value sent. The
/// MINTING of an instant truncates instead, for the reason [`wire_instant`] gives, and the two
/// agree on every instant this service stores: a truncated instant is already whole to the
/// millisecond, so it rounds to itself.
fn instant_millis(at: &DateTime) -> i64 {
    at.secs().saturating_mul(1_000).saturating_add(i64::from(
        (at.subsec_nanos().saturating_add(NANOS_PER_MILLISECOND / 2)) / NANOS_PER_MILLISECOND,
    ))
}

/// `at` stated to the millisecond ([`NANOS_PER_MILLISECOND`]), which is the form every instant
/// this service MINTS is stored and reported in.
///
/// The sub-millisecond part is DROPPED rather than rounded to the nearest millisecond (which
/// is what [`instant_millis`] does for its comparison): an instant the service settles from
/// its own clock is the instant the call was served at, and rounding up would state one that
/// had not arrived yet.
pub fn wire_instant(at: DateTime) -> DateTime {
    DateTime::from_millis(
        at.secs()
            .saturating_mul(1_000)
            .saturating_add(i64::from(at.subsec_nanos() / NANOS_PER_MILLISECOND)),
    )
}

/// Whether two exclusion windows are the same window: the configuration a removal names one
/// by, member for member.
///
/// Every member counts, because every member is part of what the window excludes: its length
/// (`Window`), when it starts (`StartTime`), how it repeats (`RecurrenceRule`) and why
/// (`Reason`). An omitted optional member is part of the configuration too \u2014 it matches an
/// omission, not an arbitrary value \u2014 so the window a caller added is named by exactly the
/// configuration they sent, and the set a `ListServiceLevelObjectiveExclusionWindows` reports
/// is the set a `RemoveExclusionWindows` can address — and since every stored window carries
/// the `StartTime` it was added at ([`stored_exclusion_window`]), a removal names the window
/// it means by the start time the read reported for it. `Window` and `RecurrenceRule` carry no
/// `PartialEq`, so those members are compared out, as [`same_dependency`] is; `StartTime` is
/// compared at the resolution the wire carries it at ([`instant_millis`]), because that is the
/// resolution a caller can name an instant AT: the value a read reports is a JSON number, and
/// the instant it re-parses to on the way back in is the same instant only to the millisecond,
/// so comparing the nanosecond fields instead would make a reported window one no client could
/// remove.
pub fn same_exclusion_window(left: &ExclusionWindow, right: &ExclusionWindow) -> bool {
    left.window.duration_unit == right.window.duration_unit
        && left.window.duration == right.window.duration
        && left.start_time.as_ref().map(instant_millis)
            == right.start_time.as_ref().map(instant_millis)
        && left
            .recurrence_rule
            .as_ref()
            .map(|rule| rule.expression.as_str())
            == right
                .recurrence_rule
                .as_ref()
                .map(|rule| rule.expression.as_str())
        && left.reason == right.reason
}

// =============================================================================
// The time range every time-ranged read takes
// =============================================================================

/// The refusal a `StartTime`/`EndTime` pair earns, or `None` when the range is one this
/// service serves.
///
/// Every operation that takes the pair documents it as a period to report over
/// (`smithy_query(query="operation", operation="GetService")`: both members `@required`),
/// and a period runs forwards: the real service refuses an inverted one with
/// `ValidationException` and states the rule with both offending instants in it. The
/// instants are rendered the way the service states them — the `Date.toString` form, in
/// UTC — so the message names the times the request carried rather than a re-encoding the
/// caller never sent.
///
/// The comparison is STRICT, because that is the rule the message states: a window whose
/// start equals its end spans no time at all, so it is refused with the same wording as an
/// inverted one rather than reporting over an empty period. It is made on the same instant
/// key every window comparison in this module uses ([`instant_nanos`]), so a pair this
/// refuses and a pair a projection reports over are ordered by one rule.
pub fn time_range_refusal(start_time: &DateTime, end_time: &DateTime) -> Option<String> {
    if instant_nanos(start_time) < instant_nanos(end_time) {
        return None;
    }
    Some(format!(
        "Invalid start time {} and end time {} provided: start time must be before end time",
        stated_instant(start_time),
        stated_instant(end_time),
    ))
}

/// One instant as this service states it in a refusal: `EEE MMM dd HH:mm:ss UTC yyyy`
/// (`Sun Sep 06 02:00:00 UTC 2026`), the `java.util.Date` rendering of a UTC instant.
///
/// The WHOLE calendar conversion is `chrono`'s: placing the epoch second on the civil
/// calendar, naming its weekday and naming its month are all that crate's, so no weekday
/// cycle, month-abbreviation table or leap-year rule is re-derived here — the same reason
/// the calendar-month evaluation interval steps its tiles with `checked_add_months`.
///
/// The stated form carries WHOLE seconds, so a sub-second part of the instant is dropped
/// rather than rounded: the message names the second the request fell in.
///
/// An instant too far from the epoch for `chrono` to place on the calendar is stated as its
/// epoch-second value, so the message still names the instant the request carried.
fn stated_instant(at: &DateTime) -> String {
    /// The `java.util.Date` field order, with the month and the weekday named and the
    /// zone stated as `UTC`.
    const STATED_FORM: &str = "%a %b %d %H:%M:%S UTC %Y";

    let Some(placed) = ::chrono::DateTime::from_timestamp(at.secs(), 0) else {
        return at.as_secs_f64().to_string();
    };
    placed.format(STATED_FORM).to_string()
}

// =============================================================================
// The service topology the stored SLOs are evidence of
// =============================================================================

/// Whether a stored SLO is evidence of the entity it monitors over a window ending at
/// `window_end` — the half of the window test that looks at the RECORD.
///
/// An SLO monitors its entity from the moment it is created until it is deleted, so the
/// period it is evidence for is `[CreatedTime, now]`, and the two periods overlap exactly
/// when the window does not END before the SLO existed and does not BEGIN after now. The
/// first of those is this test, one record at a time; the second says nothing about any
/// record and is the request's own, decided once per read ([`window_has_begun`]). So a read
/// over a past window reports an entity the account has only just started monitoring as
/// absent, and a window entirely in the future observes nothing at all.
///
/// Both bounds are compared on the module's one instant key ([`instant_nanos`]), the same
/// one [`time_range_refusal`] orders a requested range by and
/// [`ChangeRecord::inside`] places a recorded change by: a window this admits an SLO over
/// and a window a change read reports that SLO's writes in are therefore decided by one
/// comparison at one resolution, so an SLO created a fraction of a microsecond after the
/// window ended is outside it in every read.
fn monitored_in_window(slo: &ServiceLevelObjective, window_end: &DateTime) -> bool {
    instant_nanos(&slo.created_time) <= instant_nanos(window_end)
}

/// Whether the requested window had begun at `now`: nothing has been observed in one that
/// starts after the instant the read is served at, however far ahead it ends. Compared on
/// the same instant key as every other window test here ([`instant_nanos`]).
///
/// This asks about the REQUEST, not about any stored record, so every read decides it ONCE
/// from the single clock sample its handler took when it started serving and applies that
/// one answer to the whole projection — the way every other operation of this service uses
/// its one sample. Re-sampling the clock per record would let one answer admit some entities
/// and reject others on a condition they all share, so two reads of one window could
/// disagree about which entities exist.
fn window_has_begun(window_start: &DateTime, now: &DateTime) -> bool {
    instant_nanos(window_start) <= instant_nanos(now)
}

/// A dedup/order key for one `Attributes` map: its entries in key order. Used only to
/// order and deduplicate derived topology entries, never put on the wire.
fn attribute_key(attributes: &HashMap<String, String>) -> String {
    attributes
        .iter()
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .map(|(key, value)| format!("{key}\u{1f}{value}"))
        .collect::<Vec<_>>()
        .join("\u{1e}")
}

/// The CloudWatch namespace Application Signals publishes the metrics it collects for the
/// entities it discovers in.
///
/// Every topology projection reports those metrics in a `@required` `MetricReferences`
/// member documented as "one metric associated with this \<entity\> that was discovered by
/// Application Signals" (`smithy_query(shape="ServiceDependent")` /
/// `smithy_query(shape="ServiceOperation")`, and the same sentence without the clause on
/// `Service`/`ServiceSummary`/`ServiceDependency`), so an entity this emulator reports as
/// discovered carries the metrics the service collects for it.
///
/// The model states only the SHAPE of a reference (`smithy_query(shape="MetricReference")`:
/// `Namespace`, `MetricName` and `MetricType` all `@required`, `Dimensions` optional), so
/// the CATALOGUE — this namespace, the metric names in [`STANDARD_METRICS`] and the
/// dimension names in [`metric_dimensions`] — is the one the Amazon CloudWatch User Guide's
/// Application Signals chapter documents: Application Signals collects `Latency`, `Fault`
/// and `Error` in the `ApplicationSignals` namespace, dimensioned by the entity that emits
/// them (`Service`, `Environment`, `Operation`, and the `Resource*` pair for an entity
/// identified as a resource rather than named) and, for a call that entity makes, by the
/// far end of that call (the `Remote*` dimensions).
///
/// Every dimension VALUE is read out of the stored record the reference describes, so the
/// references of two entities differ exactly as their stored identities do.
const APPLICATION_SIGNALS_NAMESPACE: &str = "ApplicationSignals";

/// The metrics of that catalogue, as `(MetricName, MetricType)`, in the order they are
/// reported.
///
/// `MetricType` is "used to display the appropriate statistics in the CloudWatch console"
/// (`smithy_query(shape="MetricReference")`, member `MetricType`, `pattern
/// ^[A-Za-z0-9 -]+$`) and names the standard metric in the upper-case form the model itself
/// uses for the one of these names it spells
/// (`smithy_query(shape="ServiceLevelIndicatorMetricType")` -> `["LATENCY",
/// "AVAILABILITY"]`).
const STANDARD_METRICS: [(&str, &str); 3] = [
    ("Latency", "LATENCY"),
    ("Fault", "FAULT"),
    ("Error", "ERROR"),
];

/// The `Attributes` entries a dimension value is read from, and the dimension each becomes.
///
/// The attribute names are the member's own (`smithy_query(shape="ServiceDependent")`,
/// member `DependentKeyAttributes`): `Name` "specifies the name of the object ... used only
/// if the value of the Type field is Service, RemoteService, or AWS::Service",
/// `Environment` "specifies the location where this object is hosted", and `ResourceType`
/// and `Identifier` are used "only when the value of the Type field is Resource or
/// AWS::Resource". So an end of an edge contributes the environment it runs in plus WHAT IT
/// IS — the service it is named as, or the kind of resource it is and which one of them —
/// whichever of those its own `Type` makes applicable ([`attribute_states_identity`]).
///
/// Every attribute an identity can be read by therefore has a dimension on both ends: the
/// four entries here, and on the far end of a call the same facts under the `Remote`-prefixed
/// names the catalogue states them with ([`FAR_END_DIMENSIONS`]). An entity identified as a
/// resource is dimensioned by the pair that identifies it exactly as a named one is by its
/// name, so the references of two entities differ wherever their stored identities do.
const NEAR_END_DIMENSIONS: [(&str, &str); 4] = [
    (ENVIRONMENT_ATTRIBUTE, "Environment"),
    (NAME_ATTRIBUTE, "Service"),
    (RESOURCE_TYPE_ATTRIBUTE, "ResourceType"),
    (IDENTIFIER_ATTRIBUTE, "ResourceIdentifier"),
];

/// [`NEAR_END_DIMENSIONS`], for the far end of a call: the same facts under the names the
/// catalogue gives the far end of a call, which are the near-end ones prefixed `Remote`.
///
/// The far end carries no environment dimension: the catalogue names none for it, so where
/// the call went is stated by the remote service or remote resource it reached.
const FAR_END_DIMENSIONS: [(&str, &str); 3] = [
    (NAME_ATTRIBUTE, "RemoteService"),
    (RESOURCE_TYPE_ATTRIBUTE, "RemoteResourceType"),
    (IDENTIFIER_ATTRIBUTE, "RemoteResourceIdentifier"),
];

/// Whether the kind of object a record designates makes `attribute` part of what that record
/// states about it.
///
/// An attribute is applicable to an identity only for the types its own doc names: `Name` "is
/// used only if the value of the `Type` field is `Service`, `RemoteService`, or
/// `AWS::Service`" and `ResourceType`/`Identifier` "only when the value of the `Type` field
/// is `Resource` or `AWS::Resource`" — the same per-`Type` split [`identifying_attributes`]
/// reads an identity by. So an attribute the designated type does not make applicable states
/// nothing about the entity and contributes no dimension, just as an absent one does.
/// `Environment` is restricted to no type, and a record designating no documented kind of
/// object ([`ENTITY_TYPES`]) restricts nothing: all its mapped attributes state what they
/// state.
fn attribute_states_identity(entity_type: Option<&str>, attribute: &str) -> bool {
    let Some(entity_type) = entity_type.filter(|entity_type| ENTITY_TYPES.contains(entity_type))
    else {
        return true;
    };
    attribute == ENVIRONMENT_ATTRIBUTE || identifying_attributes(entity_type).contains(&attribute)
}

/// The dimensions one end of a reference contributes: one per entry of `table` that the
/// record holds and its `Type` makes applicable to it.
fn end_dimensions(attributes: &HashMap<String, String>, table: &[(&str, &str)]) -> Vec<Dimension> {
    let entity_type = attributes.get(TYPE_ATTRIBUTE).map(String::as_str);
    table
        .iter()
        .filter(|(attribute, _)| attribute_states_identity(entity_type, attribute))
        .filter_map(|(attribute, dimension)| {
            attributes.get(*attribute).map(|value| Dimension {
                name: (*dimension).to_string(),
                value: value.clone(),
            })
        })
        .collect()
}

/// The dimension carrying the operation of the entity the metrics were collected for.
const OPERATION_DIMENSION: &str = "Operation";

/// The dimension carrying the operation the far end of a call serves.
const REMOTE_OPERATION_DIMENSION: &str = "RemoteOperation";

/// The dimensions that identify the standard metrics of ONE discovered entity, or of one
/// edge out of it: the entity's own identity and operation, plus the far end's identity and
/// operation when the reference describes a call.
///
/// A dimension whose attribute the stored identity does not carry, or whose attribute that
/// identity's own `Type` makes no part of it ([`attribute_states_identity`]), is LEFT OUT
/// rather than reported empty — `Dimension.Value` is `length 1..1024`
/// (`smithy_query(shape="Dimension")`), so an absent fact is an absent dimension, and the
/// dimension set states exactly what the record states.
fn metric_dimensions(
    key_attributes: &HashMap<String, String>,
    operation_name: Option<&str>,
    far_end: Option<(&HashMap<String, String>, &str)>,
) -> Vec<Dimension> {
    let mut dimensions = end_dimensions(key_attributes, &NEAR_END_DIMENSIONS);
    if let Some(operation_name) = operation_name {
        dimensions.push(Dimension {
            name: OPERATION_DIMENSION.to_string(),
            value: operation_name.to_string(),
        });
    }
    if let Some((far_attributes, far_operation_name)) = far_end {
        dimensions.extend(end_dimensions(far_attributes, &FAR_END_DIMENSIONS));
        dimensions.push(Dimension {
            name: REMOTE_OPERATION_DIMENSION.to_string(),
            value: far_operation_name.to_string(),
        });
    }
    dimensions
}

/// The references to the standard metrics collected under `dimensions`: one per metric in
/// [`STANDARD_METRICS`], each identified by the same dimension set, which is what says
/// WHICH entity's or edge's metrics these are.
///
/// `AccountId` stays absent: the member is "Amazon Web Services account ID"
/// (`smithy_query(shape="MetricReference")`) and this store is one account's, so the metric
/// is in the caller's own account and there is no source account to name.
///
/// A dimension set with nothing to state leaves `Dimensions` ABSENT rather than reporting an
/// empty list: the member is "an array of one or more dimensions that further define the
/// metric" (`smithy_query(shape="MetricReference")`), so an absent fact is an absent member,
/// exactly as it is for every other derived list here.
///
/// Single-homed here so the five topology projections cannot differ on the catalogue, and
/// so this reasoning is stated once.
fn standard_metric_references(dimensions: Vec<Dimension>) -> Vec<MetricReference> {
    let dimensions = (!dimensions.is_empty()).then_some(dimensions);
    STANDARD_METRICS
        .iter()
        .map(|(metric_name, metric_type)| MetricReference {
            namespace: APPLICATION_SIGNALS_NAMESPACE.to_string(),
            metric_type: (*metric_type).to_string(),
            dimensions: dimensions.clone(),
            metric_name: (*metric_name).to_string(),
            account_id: None,
        })
        .collect()
}

/// The `GroupSource` of a group whose value was DERIVED from the entity's own attributes
/// under one of the definition's source keys.
///
/// `GroupSource` is "the source of the grouping attribute, such as `TAG`, `OTEL`, or
/// `DEFAULT`" (`smithy_query(shape="ServiceGroup")`), and the source keys "derive the
/// grouping attribute value from telemetry data, Amazon Web Services tags, or other
/// sources" (`smithy_query(shape="GroupingAttributeDefinition")`, member
/// `GroupingSourceKeys`). The attributes an entity is reported with here are the identity
/// the instrumented application's telemetry carries, not a tag this emulator holds, so a
/// value read out of them comes from telemetry.
const GROUP_SOURCE_TELEMETRY: &str = "OTEL";

/// The `GroupSource` of a group whose value came from the definition's
/// `DefaultGroupingValue`, which is "the default value to use for this grouping attribute
/// when no value can be derived from the source keys".
const GROUP_SOURCE_DEFAULT: &str = "DEFAULT";

/// The longest value a reported group may carry: `GroupValue` is modelled `length 1..255`
/// (`smithy_query(shape="GroupValue")`), while an attribute value is `length 1..1024`
/// (`smithy_query(shape="Attributes")`), so a long attribute value is not a value this
/// member can state.
const MAX_GROUP_VALUE: usize = 255;

/// What separates the grouping name from the value in a minted `GroupIdentifier`.
///
/// The identifier is "a unique identifier for this grouping attribute value"
/// (`smithy_query(shape="ServiceGroup")`), so it must distinguish one grouping's value from
/// another's: the pair of name and value IS that identity. `GroupingString` (the grouping
/// name) is `pattern ^[a-zA-Z0-9\s+\-=\._:/@]*$`, which admits no `#`, so the first `#`
/// always ends the name and the pair can be read back out of the identifier.
const GROUP_IDENTIFIER_SEPARATOR: char = '#';

/// The value one grouping attribute definition gives the entity `key_attributes`
/// identifies, with the `GroupSource` that value came from — or `None` when the definition
/// gives the entity no value at all.
///
/// A source key names an attribute to read the value from, and the keys are tried in the
/// order the definition lists them, so the first key the entity carries a USABLE value under
/// wins — a key whose attribute holds a value the modelled `GroupValue` cannot state is
/// passed over and the next key is tried. The comparison ignores ASCII case: a source key is
/// written in the casing of the telemetry field it names (`["business_unit", "team"]`) while
/// an entity's attributes are the capitalised `Attributes` names, and the same field named in
/// either casing is the same field. Which attribute ONE source key selects does not depend on
/// iteration order: the attribute whose name matches exactly is preferred, and among
/// attributes that match only case-insensitively the smallest name by byte order is taken, so
/// two reads of one entity report the same value ([`attribute_matching`]).
///
/// When no source key yields a value the definition's `DefaultGroupingValue` does, since it
/// "ensures all services have a grouping value even if the source data is missing". A
/// definition that declares no default and matches no attribute leaves the entity out of
/// that grouping entirely rather than reporting an empty value, which `GroupValue`
/// (`length 1..255`) cannot carry.
fn grouping_value(
    definition: &GroupingAttributeDefinition,
    key_attributes: &HashMap<String, String>,
) -> Option<(String, &'static str)> {
    let derived = definition
        .grouping_source_keys
        .iter()
        .flatten()
        .filter_map(|source_key| attribute_matching(key_attributes, source_key))
        .find(|value| reportable_group_value(value))
        .map(|value| (value.clone(), GROUP_SOURCE_TELEMETRY));
    derived.or_else(|| {
        definition
            .default_grouping_value
            .as_deref()
            .filter(|value| reportable_group_value(value))
            .map(|value| (value.to_string(), GROUP_SOURCE_DEFAULT))
    })
}

/// Whether a derived value is one the modelled `GroupValue` can state: non-empty and no
/// longer than [`MAX_GROUP_VALUE`].
fn reportable_group_value(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= MAX_GROUP_VALUE
}

/// The value `key_attributes` holds under the attribute `name` denotes, comparing attribute
/// names without ASCII case.
///
/// The choice is deterministic rather than the first entry a map walk happens to reach: the
/// exactly-named attribute is preferred, and failing that the smallest matching name by byte
/// order. `Attributes` is a map and its iteration order is unspecified, so an entity carrying
/// two names that differ only in case would otherwise be reported differently from one read
/// to the next.
fn attribute_matching<'a>(
    key_attributes: &'a HashMap<String, String>,
    name: &str,
) -> Option<&'a String> {
    if let Some(value) = key_attributes.get(name) {
        return Some(value);
    }
    key_attributes
        .iter()
        .filter(|(attribute, _)| attribute.eq_ignore_ascii_case(name))
        .min_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, value)| value)
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// The groups the entity `key_attributes` identifies belongs to, as every projection
    /// that carries `ServiceGroups` reports them: one group per stored grouping attribute
    /// definition that gives this entity a value ([`grouping_value`]), in the order the
    /// configuration lists its definitions.
    ///
    /// `ServiceGroups` is "an array of service groups that this service belongs to, BASED ON
    /// THE CONFIGURED GROUPING ATTRIBUTES" (`smithy_query(shape="Service")`), so the stored
    /// grouping configuration — the one `PutGroupingConfiguration` wrote and
    /// `DeleteGroupingConfiguration` clears — is what the groups are computed from. With no
    /// configuration stored, or with none of its definitions giving this entity a value,
    /// the member stays absent: the entity belongs to no group, and the member is optional.
    ///
    /// Single-homed here so `GetService` and `ListServices` cannot disagree about which
    /// groups one entity belongs to.
    fn service_groups(&self, key_attributes: &HashMap<String, String>) -> Option<Vec<ServiceGroup>> {
        let configuration = self.grouping_configuration.as_ref()?;
        let groups: Vec<ServiceGroup> = configuration
            .grouping_attribute_definitions
            .iter()
            .filter_map(|definition| {
                let (group_value, group_source) = grouping_value(definition, key_attributes)?;
                Some(ServiceGroup {
                    group_identifier: format!(
                        "{}{GROUP_IDENTIFIER_SEPARATOR}{group_value}",
                        definition.grouping_name
                    ),
                    group_name: definition.grouping_name.clone(),
                    group_value,
                    group_source: group_source.to_string(),
                })
            })
            .collect();
        (!groups.is_empty()).then_some(groups)
    }

    /// Every stored SLO that monitors the entity `key_attributes` identifies, over a window
    /// ending at `window_end`, as of the instant `now` the read is served at.
    ///
    /// The requested attributes identify the entity, and an entity is identified by as few
    /// as one of its up to four `Attributes` entries, so an SLO matches when its own entity
    /// attributes hold every entry the request named ([`attributes_contain`] — the same
    /// match `ListServiceLevelObjectives` already applies to the same member).
    ///
    /// Whether the requested period has begun is the request's own condition, so it is
    /// decided ONCE for the whole walk ([`window_has_begun`]): a period that has not begun
    /// observed nothing, so there is no candidate record to visit at all.
    fn monitoring<'a>(
        &'a self,
        key_attributes: &'a HashMap<String, String>,
        window_start: &DateTime,
        window_end: &'a DateTime,
        now: &DateTime,
    ) -> impl Iterator<Item = &'a ServiceLevelObjective> + 'a {
        let observable = window_has_begun(window_start, now).then(|| self.slos.values());
        observable.into_iter().flatten().filter(move |slo| {
            monitored_in_window(slo, window_end)
                && attributes_contain(entity_of(slo).key_attributes.as_ref(), key_attributes)
        })
    }

    /// The ONE identity this emulator reports the entity `key_attributes` names by over the
    /// window, or `None` when no stored SLO monitors it — so nothing was discovered for that
    /// identity.
    ///
    /// An entity is identified by as few as one of its up to four `Attributes` entries
    /// ([`attributes_contain`]), so the same entity can be named by a subset of the
    /// attributes one of its SLOs records and by a fuller set another records. It is still
    /// ONE entity, which the real service holds one identity for, so a read reports it by the
    /// FULLEST identity this store holds for it — the caller's own set when the matching SLOs
    /// hold none fuller, since the attributes ARE the identity and the caller named it.
    ///
    /// Single-homed so every projection of one entity — its own summary, its operations and
    /// the edges it is an end of — reports and dimensions it the same way, and two reads
    /// over one window cannot disagree about an entity's identity or about the dimensions of
    /// the metrics collected for it. It is the same resolution
    /// [`Self::observed_services`] applies from the other side, where a strict-subset identity
    /// is folded into the fuller one rather than reported as a second service.
    ///
    /// A map that names NO attribute identifies no entity — every stored record matches it —
    /// so there is no entity to resolve an identity for.
    ///
    /// The map handed in is the identity the entity is NAMED by: the request's own where the
    /// caller named the entity the read is about — which already carries the minimum identity
    /// the member documents for its `Type` ([`entity_identity_refusal`]), so it pins the
    /// entity down and the whole answer is about that one entity — or a stored record's own
    /// where the entity is one this store named rather than the caller
    /// ([`Self::observed_dependents`], whose near end is a dependent the caller did not name).
    fn observed_identity(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
        now: &DateTime,
    ) -> Option<HashMap<String, String>> {
        // Every record this walk admits holds every entry the named identity states
        // ([`attributes_contain`]), so a matched identity is a SUPERSET of the named one and
        // the fullest of them is the fullest this store monitors the entity under
        // ([`fullest_identity`], the one rule every identity resolution here applies).
        let monitored: Vec<HashMap<String, String>> = self
            .monitoring(key_attributes, window_start, window_end, now)
            .filter_map(|slo| entity_of(slo).key_attributes)
            .collect();
        fullest_identity(&monitored, key_attributes).cloned()
    }

    /// The identities the stored edges record a FAR end by, over the window: the candidates an
    /// invoked entity's one identity is resolved out of ([`far_end_identity`]).
    ///
    /// Collected ONCE per read, so an answer over many edges walks the store once rather than
    /// once per edge, and every entry of that answer resolves an identity against the same
    /// candidates.
    ///
    /// The request's own half of the window test is the caller's one decision
    /// ([`window_has_begun`]), so only the per-record half is applied here
    /// ([`monitored_in_window`]) — the same window every other topology read derives its
    /// answer over.
    fn recorded_far_identities(&self, window_end: &DateTime) -> Vec<HashMap<String, String>> {
        self.slos
            .values()
            .filter(|slo| monitored_in_window(slo, window_end))
            .filter_map(|slo| entity_of(slo).dependency_config)
            .map(|dependency| dependency.dependency_key_attributes)
            .collect()
    }

    /// The entity `key_attributes` identifies, as `GetService` reports it over the window.
    ///
    /// The reported key attributes are the identity this emulator holds for the entity
    /// ([`Self::observed_identity`]) — the request's own when no stored SLO holds a fuller
    /// one, since the attributes ARE the entity's identity and the caller named it. So an
    /// entity no stored SLO monitors is reported by its identity alone rather than refused:
    /// Application Signals identifies an entity by its key attributes rather than by a
    /// stored resource id, and this read reports what it observed for that identity.
    ///
    /// `MetricReferences` reports the standard metrics Application Signals collects for the
    /// entity ([`standard_metric_references`]), dimensioned by the identity this read
    /// reports for it — but only for an entity the store is evidence of over the window: an
    /// identity nothing monitored was not DISCOVERED, so there is nothing collecting metrics
    /// for it and the list is empty, exactly as it is for a window in which nothing was
    /// observed.
    ///
    /// `ServiceGroups` reports the groups the configured grouping attributes put this entity
    /// in ([`Self::service_groups`]) — again only for an entity the store is evidence of,
    /// since an identity nothing monitored is no service to group.
    ///
    /// `AttributeMaps` and `LogGroupReferences` stay absent: they are platform and
    /// log-group facts the real service learns from the telemetry an instrumented
    /// application sends it, which this emulator never receives.
    pub fn observed_service(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
        now: &DateTime,
    ) -> Service {
        let (reported, metric_references, service_groups) =
            match self.observed_identity(key_attributes, window_start, window_end, now) {
                Some(reported) => (
                    reported.clone(),
                    standard_metric_references(metric_dimensions(&reported, None, None)),
                    self.service_groups(&reported),
                ),
                None => (key_attributes.clone(), Vec::new(), None),
            };
        Service {
            key_attributes: reported,
            attribute_maps: None,
            service_groups,
            metric_references,
            log_group_references: None,
        }
    }

    /// Every service entity this emulator has observed over the window, as `ListServices`
    /// reports it: one summary per distinct discovered service, in a deterministic order.
    ///
    /// The stored SLOs are this emulator's only evidence of a discovered service — it
    /// receives no instrumentation telemetry — so the discovered set is the set of entities
    /// the stored SLOs monitor, each over the window it is evidence for
    /// ([`monitored_in_window`], the same window rule the other topology reads apply). A
    /// dependency an SLO names is NOT in that set: the edge is evidence that the monitored
    /// entity calls it, not that Application Signals discovered and instrumented the callee,
    /// and `ListServiceDependencies` is where the edge is reported.
    ///
    /// An identity that is a STRICT SUBSET of another observed identity is not reported
    /// separately: as few as one of an entity's up to four `Attributes` entries identifies
    /// it ([`attributes_contain`]), so the subset names the same entity as the fuller set,
    /// and `GetService` answers for it with exactly that fuller set
    /// ([`Self::observed_service`]). Reporting both would list one service twice and hand
    /// the caller a summary whose `KeyAttributes` no `GetService` echoes back.
    ///
    /// `MetricReferences` reports the standard metrics collected for each reported identity
    /// ([`standard_metric_references`]) and `ServiceGroups` the groups the configured
    /// grouping attributes put it in ([`Self::service_groups`]) — every summary here IS a
    /// discovered service, so each carries both, exactly as [`Self::observed_service`]
    /// reports them for the same entity. `AttributeMaps` stays absent, as it does there: it
    /// is a platform and telemetry fact the real service learns from an instrumented
    /// application, which this emulator never receives.
    pub fn observed_services(
        &self,
        window_start: &DateTime,
        window_end: &DateTime,
        now: &DateTime,
    ) -> Vec<ServiceSummary> {
        // The request's own half of the window test, decided ONCE for the whole answer: a
        // period that has not begun by the instant this read is served at observed no
        // service, so every reported entity is admitted or rejected by this one decision
        // rather than by a clock read of its own ([`window_has_begun`]).
        if !window_has_begun(window_start, now) {
            return Vec::new();
        }
        let mut by_identity: BTreeMap<String, HashMap<String, String>> = BTreeMap::new();
        for slo in self
            .slos
            .values()
            .filter(|slo| monitored_in_window(slo, window_end))
        {
            let Some(attributes) = entity_of(slo).key_attributes else {
                continue;
            };
            // `ServiceSummary.KeyAttributes` is `@required` with `length 1..4`: an SLO whose
            // indicator names no entity at all is evidence of no service.
            if attributes.is_empty() {
                continue;
            }
            by_identity.insert(attribute_key(&attributes), attributes);
        }

        let identities: Vec<HashMap<String, String>> = by_identity.into_values().collect();
        identities
            .iter()
            .filter(|candidate| {
                !identities.iter().any(|other| {
                    other.len() > candidate.len() && attributes_contain(Some(other), candidate)
                })
            })
            .map(|key_attributes| ServiceSummary {
                key_attributes: key_attributes.clone(),
                attribute_maps: None,
                service_groups: self.service_groups(key_attributes),
                metric_references: standard_metric_references(metric_dimensions(
                    key_attributes,
                    None,
                    None,
                )),
            })
            .collect()
    }

    /// The operations of the entity `key_attributes` identifies, in name order: one entry
    /// per distinct operation the stored SLOs name on it.
    ///
    /// Each entry's `MetricReferences` reports the standard metrics collected for THAT
    /// operation ([`standard_metric_references`]): the dimensions are the entity's identity
    /// plus the operation, so two operations of one service carry different references. The
    /// identity is the ONE this emulator holds for the entity ([`Self::observed_identity`]),
    /// resolved once for the whole answer rather than per operation, so the entries cannot
    /// disagree with each other or with the `GetService` answer for the same entity over the
    /// same window — a `ServiceOperation` carries no identity of its own for a caller to tell
    /// two dimension sets apart by.
    pub fn observed_operations(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
        now: &DateTime,
    ) -> Vec<ServiceOperation> {
        let Some(identity) = self.observed_identity(key_attributes, window_start, window_end, now)
        else {
            return Vec::new();
        };
        let mut names: BTreeSet<String> = BTreeSet::new();
        for slo in self.monitoring(key_attributes, window_start, window_end, now) {
            if let Some(name) = entity_of(slo).operation_name {
                names.insert(name);
            }
        }
        names
            .into_iter()
            .map(|name| ServiceOperation {
                metric_references: standard_metric_references(metric_dimensions(
                    &identity,
                    Some(&name),
                    None,
                )),
                name,
            })
            .collect()
    }

    /// The dependencies of the entity `key_attributes` identifies: one entry per distinct
    /// edge the stored SLOs record from one of its operations to an operation of another
    /// entity.
    ///
    /// An SLO that names a dependency but no operation of its own records no edge: the
    /// modelled `ServiceDependency` states the edge as "the operation in this service that
    /// calls the dependency" (`OperationName`, `@required`), so without it there is no edge
    /// to report.
    ///
    /// Each edge's `MetricReferences` reports the standard metrics collected for THAT call
    /// ([`standard_metric_references`]): the dimensions are the calling entity's identity
    /// and operation plus the far end's identity and operation, so two edges out of one
    /// service carry different references. The near end's identity is the ONE this emulator
    /// holds for the entity ([`Self::observed_identity`]), resolved once for the whole
    /// answer rather than per edge, so no two edges out of one entity — and no other read of
    /// it over the same window — disagree about the entity's own dimensions. The far end's is
    /// the ONE this store holds for the INVOKED entity ([`far_end_identity`]) rather than
    /// whichever subset of it the record this edge was read off happens to name, so one far
    /// entity is reported under one identity — the identity the dependent view of this edge
    /// dimensions its far end by, and one `ListServiceDependents` answers for — and this
    /// entry's dimensions state the same far end its `@required` `DependencyKeyAttributes`
    /// does.
    pub fn observed_dependencies(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
        now: &DateTime,
    ) -> Vec<ServiceDependency> {
        let Some(identity) = self.observed_identity(key_attributes, window_start, window_end, now)
        else {
            return Vec::new();
        };
        let mut by_edge: BTreeMap<String, ServiceDependency> = BTreeMap::new();
        let recorded_far_identities = self.recorded_far_identities(window_end);
        // The identity resolved for each far end, by the record's own identity: several
        // records can name one invoked entity, so the answer resolves one identity per
        // distinct recorded identity rather than one per edge.
        let mut far_identities: BTreeMap<String, HashMap<String, String>> = BTreeMap::new();
        for slo in self.monitoring(key_attributes, window_start, window_end, now) {
            let entity = entity_of(slo);
            let (Some(operation_name), Some(dependency)) =
                (entity.operation_name, entity.dependency_config)
            else {
                continue;
            };
            let dependency_key_attributes = far_identities
                .entry(attribute_key(&dependency.dependency_key_attributes))
                .or_insert_with(|| {
                    far_end_identity(
                        &recorded_far_identities,
                        &dependency.dependency_key_attributes,
                    )
                })
                .clone();
            // The edge is the near operation, the entity it reached and the operation of that
            // entity it called — the far end by the ONE identity this store holds for it, so
            // two records naming it by identities of different fullness are the one edge they
            // are evidence of rather than two.
            let edge = format!(
                "{operation_name}\u{1d}{}\u{1d}{}",
                attribute_key(&dependency_key_attributes),
                dependency.dependency_operation_name,
            );
            let metric_references = standard_metric_references(metric_dimensions(
                &identity,
                Some(&operation_name),
                Some((
                    &dependency_key_attributes,
                    &dependency.dependency_operation_name,
                )),
            ));
            by_edge.entry(edge).or_insert_with(|| ServiceDependency {
                operation_name,
                dependency_key_attributes,
                dependency_operation_name: dependency.dependency_operation_name,
                metric_references,
            });
        }
        by_edge.into_values().collect()
    }

    /// The dependents of the entity `key_attributes` identifies: the inverse of
    /// [`Self::observed_dependencies`], one entry per distinct edge the stored SLOs record
    /// from another entity INTO one of this entity's operations.
    ///
    /// `OperationName` is the operation of the named entity that was invoked (the
    /// dependency operation the edge points at), and `DependentOperationName` the operation
    /// of the dependent that invoked it — which an SLO that names no operation of its own
    /// leaves absent, as the model allows.
    ///
    /// `MetricReferences` reports the standard metrics collected for THAT call
    /// ([`standard_metric_references`]). The metrics of a call are the INVOKING entity's, so
    /// the dimensions are the dependent's identity and operation on the near end and the
    /// invoked entity and operation on the far end — the same dimension set
    /// [`Self::observed_dependencies`] reports for the same edge seen from the caller, which
    /// is what keeps the two directions from disagreeing about one edge. Each end is the ONE
    /// identity this store holds for the entity at it rather than whichever subset of it the
    /// record this edge was read off happens to name: the dependent is a DISCOVERED entity of
    /// its own, so its identity is the one a `GetService` on it echoes back
    /// ([`Self::observed_identity`]) and the one the dependency view of this edge dimensions
    /// its near end by, while the invoked end is the entity the caller named, whose identity
    /// is the fullest the recorded edges hold for it ([`far_end_identity`]) — the identity
    /// that view reports the same edge's far end under.
    ///
    /// A record is evidence of an edge into the named entity when the identity it records the
    /// invoked end by NAMES that entity, which is the resolved identity rather than the
    /// record's own map: a record naming a strict subset of the request's identity records a
    /// call into the SAME entity ([`attributes_contain`]), so the edge set this read reports
    /// is the inverse of the one [`Self::observed_dependencies`] reports for its near end.
    /// Two records that agree on the dependent, its operation and the invoked operation are
    /// evidence of the one edge between them, so they are reported once: a `ServiceDependent`
    /// states nothing else for a caller to tell two entries apart by.
    pub fn observed_dependents(
        &self,
        key_attributes: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
        now: &DateTime,
    ) -> Vec<ServiceDependent> {
        // Decided ONCE for the whole answer, as the sibling topology reads decide it: a
        // period that has not begun by the instant this read is served at recorded no edge
        // ([`window_has_begun`]).
        if !window_has_begun(window_start, now) {
            return Vec::new();
        }
        let mut by_edge: BTreeMap<String, ServiceDependent> = BTreeMap::new();
        // The identity resolved for each dependent, by the record's own identity: several
        // edges can come out of one dependent, and resolving its identity walks the store, so
        // the answer is resolved once per distinct recorded identity rather than once per
        // edge.
        let mut identities: BTreeMap<String, HashMap<String, String>> = BTreeMap::new();
        // The same, for the invoked end each record names, resolved by the same rule and out
        // of the same candidates the caller-side view resolves it by.
        let recorded_far_identities = self.recorded_far_identities(window_end);
        let mut invoked_identities: BTreeMap<String, HashMap<String, String>> = BTreeMap::new();
        for slo in self
            .slos
            .values()
            .filter(|slo| monitored_in_window(slo, window_end))
        {
            let entity = entity_of(slo);
            let Some(dependency) = entity.dependency_config else {
                continue;
            };
            let invoked = invoked_identities
                .entry(attribute_key(&dependency.dependency_key_attributes))
                .or_insert_with(|| {
                    far_end_identity(
                        &recorded_far_identities,
                        &dependency.dependency_key_attributes,
                    )
                })
                .clone();
            if !attributes_contain(Some(&invoked), key_attributes) {
                continue;
            }
            let Some(recorded) = entity.key_attributes else {
                continue;
            };
            let dependent_key_attributes = identities
                .entry(attribute_key(&recorded))
                .or_insert_with(|| {
                    self.observed_identity(&recorded, window_start, window_end, now)
                        .unwrap_or(recorded)
                })
                .clone();
            let edge = format!(
                "{}\u{1d}{}\u{1d}{}",
                attribute_key(&dependent_key_attributes),
                entity.operation_name.clone().unwrap_or_default(),
                dependency.dependency_operation_name,
            );
            let metric_references = standard_metric_references(metric_dimensions(
                &dependent_key_attributes,
                entity.operation_name.as_deref(),
                Some((&invoked, &dependency.dependency_operation_name)),
            ));
            by_edge.entry(edge).or_insert_with(|| ServiceDependent {
                operation_name: Some(dependency.dependency_operation_name),
                dependent_key_attributes,
                dependent_operation_name: entity.operation_name,
                metric_references,
            });
        }
        by_edge.into_values().collect()
    }

    /// The state of each service this emulator observed over the window, as
    /// `ListServiceStates` reports it.
    ///
    /// The services considered are the DISCOVERED ones ([`Self::observed_services`]), so this
    /// read cannot report a service `ListServices` does not, and `filters` then narrows that
    /// set ([`passes_attribute_filter`]). Each reported entry carries the one most recent
    /// change recorded for its service inside the window ([`latest_change`]) —
    /// `ServiceState.LatestChangeEvents` is `@required` with `length 1..1`, so a service with
    /// no recorded change inside the window has no state to report rather than one with an
    /// empty event list. The store's change records are enumerated ONCE for the whole answer
    /// ([`Self::change_records`]) and then read per service, so answering for many services
    /// walks the store once rather than once each.
    ///
    /// `AttributeFilters` echoes the filters that were applied, which is what the member
    /// states it carries; a request that named none has none applied, and holding no filters
    /// and holding an empty set are the same state.
    ///
    /// `ChangeEvent` states WHAT changed, WHEN and WHERE, and it is built from the RECORD the
    /// change was read off ([`ChangeRecord::event`]) exactly as `ListEntityEvents` builds it
    /// for the same record: the entity is the identity this read reports for the service, and
    /// the instant, the Region and the `EventName` are the write's own — nothing about the
    /// event depends on who is reading it, so neither read can attribute one recorded change
    /// to a Region or an operation the other does not.
    pub fn observed_service_states(
        &self,
        window_start: &DateTime,
        window_end: &DateTime,
        now: &DateTime,
        filters: &[AttributeFilter],
    ) -> Vec<ServiceState> {
        let applied = (!filters.is_empty()).then(|| filters.to_vec());
        let window = (instant_nanos(window_start), instant_nanos(window_end));
        // Enumerated once for the whole answer, and narrowed to the window once too: a record
        // dated outside the period is news to no service in it, so the per-service read below
        // never looks at it again.
        let mut records = self.change_records();
        records.retain(|(_, writes)| writes.iter().any(|record| record.inside(window)));
        self.observed_services(window_start, window_end, now)
            .into_iter()
            .filter(|summary| {
                filters
                    .iter()
                    .all(|filter| passes_attribute_filter(&summary.key_attributes, filter))
            })
            .filter_map(|summary| {
                let change = latest_change(&records, &summary.key_attributes, window)?;
                Some(ServiceState {
                    attribute_filters: applied.clone(),
                    latest_change_events: vec![change.event(&summary.key_attributes)],
                    service: summary.key_attributes,
                })
            })
            .collect()
    }
}

/// The `ChangeEventType` of a change to what Application Signals is configured to watch on a
/// service (`smithy_query(shape="ChangeEventType")` -> `["DEPLOYMENT", "CONFIGURATION"]`).
///
/// Every change this emulator records is of this kind: a deployment is an event of the
/// customer's own release pipeline, which the real service learns about from the telemetry
/// and the trail of an instrumented account, and this emulator receives neither.
pub const CONFIGURATION_CHANGE_EVENT: &str = "CONFIGURATION";

/// `ChangeEvent.EventId` for an event this emulator did not read off a CloudTrail record,
/// which is the value the member documents for one: "For CloudTrail-based events, this is the
/// CloudTrail event id. For other events, this will be `Unknown`"
/// (`smithy_query(query="doc", shape="ChangeEvent", member="EventId")`).
pub const UNRECORDED_EVENT_ID: &str = "Unknown";

/// Whether two key-attribute maps name the same service.
///
/// As few as one of an entity's up to four `Attributes` entries identifies it
/// ([`attributes_contain`]), so one map names the same service as the other when either is a
/// subset of it — which is the same rule that lets
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::observed_services`] fold a subset
/// identity into the fuller one rather than reporting two services. A map that names no
/// attributes at all names no service.
fn names_same_service(left: Option<&HashMap<String, String>>, right: &HashMap<String, String>) -> bool {
    let Some(left) = left else {
        return false;
    };
    if left.is_empty() || right.is_empty() {
        return false;
    }
    attributes_contain(Some(left), right) || attributes_contain(Some(right), left)
}

/// Whether a service's attributes pass one `AttributeFilter`.
///
/// The filter names an attribute and the values it accepts for it, and "services that have
/// any of these values for the attribute will be included in the results"
/// (`smithy_query(shape="AttributeFilter")`), so one filter passes when the service carries
/// that attribute with one of those values — and a request's filters narrow together, each
/// one being a further condition on the same service.
///
/// The attributes a service HAS here are its key attributes: the platform, business and other
/// descriptive attributes a filter may also name (`AttributeFilterName`: "such as `Platform`,
/// `Environment`, or `BusinessUnit`") reach the real service in the telemetry an instrumented
/// application sends it, which this emulator never receives — so a filter on an attribute the
/// service's identity does not carry selects nothing rather than everything.
fn passes_attribute_filter(
    key_attributes: &HashMap<String, String>,
    filter: &AttributeFilter,
) -> bool {
    key_attributes
        .get(&filter.attribute_filter_name)
        .is_some_and(|value| {
            filter
                .attribute_filter_values
                .iter()
                .any(|accepted| accepted == value)
        })
}

// =============================================================================
// The error budget of one stored SLO
// =============================================================================

/// The modelled `ServiceLevelObjectiveBudgetStatus` values
/// (`smithy_query(shape="ServiceLevelObjectiveBudgetStatus")` -> `["OK", "WARNING",
/// "BREACHED", "INSUFFICIENT_DATA"]`), each as the member's own doc defines it: `OK` while
/// the remaining budget is above the warning threshold, `WARNING` once it is below it,
/// `BREACHED` once the budget is exhausted, and `INSUFFICIENT_DATA` when the requested time
/// precedes the SLO or attainment data is missing.
///
/// The status of one stored objective at one instant is ONE quantity, so it is derived in one
/// place ([`budget_assessment`]) for every read that reports it — the budget report, which
/// reports it as `BudgetStatus`, and the SLO auditor, which reports it in the `Data` entry of
/// that name. Two derivations would let two reads of one record at one instant contradict
/// each other.
pub const BUDGET_STATUS_OK: &str = "OK";
/// [`BUDGET_STATUS_OK`], for a remaining budget under the SLO's warning threshold.
pub const BUDGET_STATUS_WARNING: &str = "WARNING";
/// [`BUDGET_STATUS_OK`], for an exhausted budget.
pub const BUDGET_STATUS_BREACHED: &str = "BREACHED";
/// [`BUDGET_STATUS_OK`], for an objective nothing was evaluated for over the period asked
/// about.
pub const BUDGET_STATUS_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The `Attainment` of a period-based SLO whose evaluated window holds no period in breach
/// of its SLI threshold: every period attained the goal, which is 100% of them.
///
/// `Attainment` is "a number between 0 and 100 that represents the success percentage of
/// your application compared to the goal set by the SLO ... the percentage of time periods
/// that the service has attained the SLO's attainment goal"
/// (`smithy_query(query="doc", shape="ServiceLevelObjectiveBudgetReport",
/// member="Attainment")`).
pub const FULL_ATTAINMENT: f64 = 100.0;

/// The seconds a `MINUTE` of interval duration spans, and the units built from it
/// (`smithy_query(shape="DurationUnit")` -> `["MINUTE", "HOUR", "DAY", "MONTH"]`).
const MINUTE_SECONDS: i64 = 60;
/// [`MINUTE_SECONDS`], for `HOUR`.
const HOUR_SECONDS: i64 = 60 * MINUTE_SECONDS;
/// [`MINUTE_SECONDS`], for `DAY`.
const DAY_SECONDS: i64 = 24 * HOUR_SECONDS;
/// [`MINUTE_SECONDS`], for the ROLLING `MONTH` — the 30-day month
/// `BatchGetServiceLevelObjectiveBudgetReport`'s own doc measures a monthly error budget in
/// ("an error budget of 21.9 minutes of downtime in a 30-day month").
///
/// A CALENDAR month is not a fixed span, so a calendar interval of months is stepped by the
/// calendar instead ([`calendar_month_interval`]).
const ROLLING_MONTH_SECONDS: i64 = 30 * DAY_SECONDS;

/// The `DurationUnit` whose span is a calendar month rather than a fixed number of seconds.
const DURATION_UNIT_MONTH: &str = "MONTH";

/// The seconds `duration` units of `duration_unit` span, or `None` for a unit this service
/// does not know or a span that is not a forward one.
fn span_seconds(duration_unit: &str, duration: i32) -> Option<i64> {
    let unit = match duration_unit {
        "MINUTE" => MINUTE_SECONDS,
        "HOUR" => HOUR_SECONDS,
        "DAY" => DAY_SECONDS,
        DURATION_UNIT_MONTH => ROLLING_MONTH_SECONDS,
        _ => return None,
    };
    let span = unit.checked_mul(i64::from(duration))?;
    (span > 0).then_some(span)
}

/// The evaluation interval of `interval` that the instant `at` falls in, as `(start, end)`
/// epoch seconds.
///
/// A ROLLING interval is the span ending at the instant asked about: "if `Duration` is `7`
/// and `DurationUnit` is `DAY`, each rolling interval is seven days"
/// (`smithy_query(shape="RollingInterval")`), and a budget report is "as of the time that you
/// specified in `TimeStamp`".
///
/// A CALENDAR interval tiles forward from its own `StartTime` — "as soon as one calendar
/// interval ends, another automatically begins"
/// (`smithy_query(shape="CalendarInterval")`) — so the interval asked about is the tile that
/// contains `at`. `None` when `at` precedes the first tile: no interval has begun yet, so
/// there is nothing to report a budget for.
fn evaluation_interval(interval: &Interval, at: i64) -> Option<(i64, i64)> {
    match interval {
        Interval::RollingInterval(rolling) => {
            let span = span_seconds(&rolling.duration_unit, rolling.duration)?;
            Some((at.checked_sub(span)?, at))
        }
        Interval::CalendarInterval(calendar) => {
            let anchor = calendar.start_time.secs();
            if at < anchor {
                return None;
            }
            if calendar.duration_unit == DURATION_UNIT_MONTH {
                return calendar_month_interval(anchor, calendar.duration, at);
            }
            let span = span_seconds(&calendar.duration_unit, calendar.duration)?;
            let start = anchor.checked_add(at.checked_sub(anchor)? / span * span)?;
            Some((start, start.checked_add(span)?))
        }
    }
}

/// The most corrections [`calendar_month_interval`] makes to its stride estimate.
///
/// The estimate is derived from the two instants' calendar year and month, so it can be off
/// by one stride — the clamping of a day-of-month the target month does not have moves a
/// tile boundary within its month — and one correction in either direction settles it. The
/// bound is what keeps the search finite for any instant a `Timestamp` can carry.
const MAX_STRIDE_CORRECTIONS: u32 = 4;

/// The `duration`-month calendar tile containing `at`, stepped from `anchor` by the
/// CALENDAR: a calendar interval is "aligned with the calendar"
/// (`smithy_query(shape="CalendarInterval")`, member `Duration`), and calendar months are of
/// different lengths, so a fixed span would drift a month boundary by days over a year.
///
/// The calendar arithmetic is `chrono`'s — `checked_add_months`, which clamps a
/// day-of-month the target month does not have — so no month-length or leap-year table is
/// re-derived here.
fn calendar_month_interval(anchor: i64, duration: i32, at: i64) -> Option<(i64, i64)> {
    let months_per_stride = u32::try_from(duration).ok().filter(|months| *months > 0)?;
    let anchored = ::chrono::DateTime::from_timestamp(anchor, 0)?;
    let requested = ::chrono::DateTime::from_timestamp(at, 0)?;

    let months_between = (requested.year().checked_sub(anchored.year())?).checked_mul(12)?
        + (i32::try_from(requested.month()).ok()? - i32::try_from(anchored.month()).ok()?);
    let estimate = months_between / i32::try_from(months_per_stride).ok()?;
    let mut strides = u32::try_from(estimate.max(0)).ok()?;

    for _ in 0..MAX_STRIDE_CORRECTIONS {
        let start = stepped_months(&anchored, strides, months_per_stride)?;
        let end = stepped_months(&anchored, strides.checked_add(1)?, months_per_stride)?;
        if start.timestamp() > at {
            strides = strides.checked_sub(1)?;
            continue;
        }
        if end.timestamp() <= at {
            strides = strides.checked_add(1)?;
            continue;
        }
        return Some((start.timestamp(), end.timestamp()));
    }
    None
}

/// `anchored` advanced by `strides` tiles of `months_per_stride` calendar months each.
fn stepped_months(
    anchored: &::chrono::DateTime<::chrono::Utc>,
    strides: u32,
    months_per_stride: u32,
) -> Option<::chrono::DateTime<::chrono::Utc>> {
    anchored.checked_add_months(::chrono::Months::new(strides.checked_mul(months_per_stride)?))
}

/// Whether anything of `slo` inside the interval starting at `interval_start` has been
/// EVALUATED over the period a read asks about, which ends at `at` and is served at `now`.
///
/// The window a read covers is the part of the evaluation interval that falls in the period
/// asked about, follows the SLO's creation AND has already elapsed, so an EMPTY window means
/// the SLO was measured over nothing: `INSUFFICIENT_DATA` "means that the specified start and
/// end times were before the SLO was created, or that attainment data is missing"
/// (`smithy_query(query="doc", shape="ServiceLevelObjectiveBudgetReport",
/// member="BudgetStatus")`). An SLO monitors its entity only from the moment it is created,
/// which is the same window rule this service's topology reads apply.
///
/// `period_start` is the beginning of the period asked about, for a read that states one; a
/// read asking about an INSTANT states none, and its window reaches back to the start of the
/// evaluation interval.
///
/// The instants are ordered on the module's one instant key ([`instant_nanos`]), the same key
/// those reads apply, so a creation a fraction of a microsecond after the window ends leaves
/// the window empty here exactly as it leaves the entity undiscovered there.
fn evaluated(
    interval_start: i64,
    slo: &ServiceLevelObjective,
    period_start: Option<&DateTime>,
    at: &DateTime,
    now: &DateTime,
) -> bool {
    let interval_start = instant_nanos(&DateTime::from_secs(interval_start));
    let mut window_start = interval_start.max(instant_nanos(&slo.created_time));
    if let Some(period_start) = period_start {
        window_start = window_start.max(instant_nanos(period_start));
    }
    let window_end = instant_nanos(at).min(instant_nanos(now));
    window_start < window_end
}

/// A real-valued budget of seconds as the whole seconds the modelled integer
/// `TotalBudgetSeconds`/`BudgetSecondsRemaining` members count: the budget is a share of
/// the interval and so is rarely a whole number of seconds (1% of 7 days is 6048 seconds,
/// 0.1% of 12 hours is 43.2), and the nearest second is the closest the member can come to
/// it.
///
/// Rounding rather than truncating also keeps the reported budget off the representation of
/// the percentage it came from: an attainment goal is a decimal percentage, which binary
/// floating point holds to within a tiny error either way, and truncation would turn that
/// error into a whole second missing from an exact budget.
fn whole_seconds(budget: f64) -> i32 {
    budget.round().clamp(0.0, f64::from(i32::MAX)) as i32
}

/// The `BudgetStatus` a remaining error budget earns against the SLO's warning threshold.
///
/// `WarningThreshold` is "the percentage of remaining budget over total budget that you want
/// to get warnings for" (`smithy_query(shape="Goal")`), so the comparison is on that
/// percentage, and a budget with nothing left of it is the exhausted one `BREACHED` names. A
/// ZERO total budget leaves no percentage to compare — an SLO whose attainment goal tolerates
/// no unhealthy time at all — so it is `OK` exactly while nothing has been spent.
fn budget_status_of(total: i32, remaining: i32, warning_threshold: Option<f64>) -> &'static str {
    if remaining < 0 || (remaining == 0 && total > 0) {
        return BUDGET_STATUS_BREACHED;
    }
    let remaining_percent = if total > 0 {
        f64::from(remaining) / f64::from(total) * FULL_ATTAINMENT
    } else {
        FULL_ATTAINMENT
    };
    match warning_threshold {
        Some(threshold) if remaining_percent < threshold => BUDGET_STATUS_WARNING,
        _ => BUDGET_STATUS_OK,
    }
}

/// The error budget of one stored SLO over the period a read asks about: the budget status
/// the objective's goal earns, and the quantities a budget report states that status beside.
///
/// Every read that reports any of these derives them HERE ([`budget_assessment`]), so a
/// budget report and an audit finding of one objective over one period cannot disagree about
/// its budget.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetAssessment {
    /// The modelled `ServiceLevelObjectiveBudgetStatus` the objective's budget earns, one of
    /// [`BUDGET_STATUS_OK`] and the values beside it.
    pub budget_status: &'static str,
    /// `Attainment`, absent for an objective nothing was evaluated for.
    pub attainment: Option<f64>,
    /// `TotalBudgetSeconds`: the budget the goal allows over the evaluation interval, which
    /// is a fact about the goal even where nothing was evaluated.
    pub total_budget_seconds: Option<i32>,
    /// `BudgetSecondsRemaining`, absent for an objective nothing was evaluated for.
    pub budget_seconds_remaining: Option<i32>,
}

/// The [`BudgetAssessment`] of one stored SLO over a period ending at `at`, served at `now`,
/// and beginning at `period_start` for a read that asks about a period rather than an
/// instant.
///
/// The error budget of a PERIOD-BASED SLO is an amount of TIME, and follows from the stored
/// goal alone: it is the share of the evaluation interval that the attainment goal leaves
/// unattained — "an SLO with a threshold of 99.95% and a monthly interval translates to an
/// error budget of 21.9 minutes of downtime in a 30-day month"
/// (`BatchGetServiceLevelObjectiveBudgetReport`'s own doc). Nothing in the evaluated window
/// has been observed in breach of the SLI threshold, since this emulator receives no
/// telemetry and a period with no breaching data point is a good period, so every period of
/// the window attained the goal and none of that budget has been spent.
///
/// The error budget of a REQUEST-BASED SLO is a number of REQUESTS — `TotalBudgetRequests`
/// "displays the total number of failed requests that can be tolerated" and `Attainment` is
/// "the number of successful requests divided by the number of total requests"
/// (`smithy_query(query="doc", shape="ServiceLevelObjectiveBudgetReport",
/// member="Attainment")`) — and a request exists only if one was made, where a period exists
/// because time passed. No request has been observed, so there is no count to take a share
/// of and no quotient to report: those members stay ABSENT and the status is the
/// [`BUDGET_STATUS_INSUFFICIENT_DATA`] the member's doc defines for missing attainment data,
/// rather than a zero budget the SLO never set or an attainment nothing measured.
pub fn budget_assessment(
    slo: &ServiceLevelObjective,
    period_start: Option<&DateTime>,
    at: &DateTime,
    now: &DateTime,
) -> BudgetAssessment {
    // The goal with every member the model documents a default for filled in, through the
    // same builder the create path stored the record with — so the interval, the attainment
    // goal and the warning threshold this assessment is computed from are the ones the SLO is
    // evaluated against.
    let goal = goal_with_defaults(Some(&slo.goal));
    let interval = goal
        .interval
        .as_ref()
        .and_then(|interval| evaluation_interval(interval, at.secs()));

    // `TotalBudgetSeconds` and `BudgetSecondsRemaining` are "included only if the SLO is a
    // period-based SLO", which is the SLO that carries the period-based indicator.
    let total_budget_seconds = match (slo.sli.is_some(), interval, goal.attainment_goal) {
        (true, Some((start, end)), Some(attainment_goal)) => {
            let unattained = (1.0 - attainment_goal / FULL_ATTAINMENT).clamp(0.0, 1.0);
            Some(whole_seconds((end - start) as f64 * unattained))
        }
        _ => None,
    };
    let evaluated_window =
        interval.is_some_and(|(start, _)| evaluated(start, slo, period_start, at, now));

    match total_budget_seconds {
        // The whole budget remains, so this call site can only produce `OK` or the `WARNING`
        // a threshold above the whole budget asks for: no part of a budget is spent while
        // nothing is observed in breach. The comparison is [`budget_status_of`]'s all the
        // same, so a spent budget would be classified by the member's own rule rather than by
        // a second one written here.
        Some(total) if evaluated_window => BudgetAssessment {
            budget_status: budget_status_of(total, total, goal.warning_threshold),
            attainment: Some(FULL_ATTAINMENT),
            total_budget_seconds: Some(total),
            budget_seconds_remaining: Some(total),
        },
        // Either the window was empty — the interval had not begun, or had not begun for
        // this SLO, or the period asked about holds no elapsed part of it — or the SLO's
        // attainment is a quotient of request counts nothing has reported. The interval's own
        // total budget is still a fact about the goal, so a period-based read keeps it; what
        // nothing was measured for is left absent.
        _ => BudgetAssessment {
            budget_status: BUDGET_STATUS_INSUFFICIENT_DATA,
            attainment: None,
            total_budget_seconds,
            budget_seconds_remaining: None,
        },
    }
}

// =============================================================================
// The audit findings the stored SLOs are evidence of
// =============================================================================

// =============================================================================
// Dynamic instrumentation configurations
// =============================================================================

// =============================================================================
// The change events this service holds a record of for an entity
// =============================================================================

/// The [`ENTITY_TYPES`] value designating a service entity — the kind of object an
/// instrumentation configuration's `Service`/`Environment` pair names, and the `Type` of the
/// service identity the member doc spells out: `{ "Type": "Service", "Name":
/// "visits-service", "Environment": "petclinic-test" }` (same member doc).
pub const SERVICE_ENTITY_TYPE: &str = "Service";

/// The `EventName` of the event a stored SLO's `CreatedTime` is the record of.
///
/// `EventName` is "The name or description of this change event"
/// (`smithy_query(query="doc", shape="ChangeEvent", member="EventName")`), so it says WHICH
/// change the event reports. Each dated member a record carries is written by exactly one
/// operation — `CreatedTime` by the create, `LastUpdatedTime` by the update — so naming that
/// operation describes the change without claiming anything the record does not hold.
const SLO_CREATED_EVENT_NAME: &str = "CreateServiceLevelObjective";

/// [`SLO_CREATED_EVENT_NAME`], for the `LastUpdatedTime` of an SLO that has been updated
/// since it was created.
const SLO_UPDATED_EVENT_NAME: &str = "UpdateServiceLevelObjective";

/// [`SLO_CREATED_EVENT_NAME`], for the `CreatedAt` of a stored instrumentation
/// configuration.
const INSTRUMENTATION_CREATED_EVENT_NAME: &str = "CreateInstrumentationConfiguration";

/// The service entity an instrumentation configuration's `Service` and `Environment` name,
/// as an `Attributes` identity.
///
/// A configuration is applied to the service it names in the environment it names, and that
/// pair IS a service entity's identity in the form the `Entity` member documents (`{ "Type":
/// "Service", "Name": …, "Environment": … }`), so the configuration's own record is what the
/// identity is read from.
pub fn service_entity_attributes(service: &str, environment: &str) -> HashMap<String, String> {
    HashMap::from([
        (TYPE_ATTRIBUTE.to_string(), SERVICE_ENTITY_TYPE.to_string()),
        (NAME_ATTRIBUTE.to_string(), service.to_string()),
        (ENVIRONMENT_ATTRIBUTE.to_string(), environment.to_string()),
    ])
}

/// One instant as a count of nanoseconds since the epoch, which is the precision a
/// [`DateTime`] holds.
///
/// Instants are compared through this rather than through `DateTime::as_secs_f64`, whose
/// resolution at present epoch values is around a hundred nanoseconds: the window bounds are
/// INCLUSIVE, so a comparison that cannot tell two instants apart would make the bound
/// approximate, and two distinct instants would order as equal.
///
/// The seconds and the nanoseconds within them are read through
/// [`status_event_instant`], the module's one reader of an instant's parts, so a comparison
/// here and the chronology a status-event token carries cannot disagree about what instant a
/// `Timestamp` denotes.
pub fn instant_nanos(at: &DateTime) -> i128 {
    let (secs, subsec_nanos) = status_event_instant(at);
    i128::from(secs) * i128::from(NANOS_PER_SECOND) + i128::from(subsec_nanos)
}

/// One span as a count of nanoseconds, so a span and an instant are measured on the one
/// key instants are compared on ([`instant_nanos`]).
///
/// A span too long to count in nanoseconds on that key saturates, which keeps it the longer
/// of any pair it is compared with rather than wrapping to a short one; no span this module
/// holds comes near that.
fn duration_nanos(span: Duration) -> i128 {
    i128::try_from(span.as_nanos()).unwrap_or(i128::MAX)
}

/// The instant `span` after `at`, derived on the nanosecond key ([`instant_nanos`]) rather
/// than through float seconds.
///
/// A horizon is compared against a stored instant, so it is derived at the precision that
/// comparison is made at: deriving it through `DateTime::as_secs_f64` would round it by
/// around a hundred nanoseconds, which would move the horizon off the instant plus the
/// span.
fn instant_plus(at: &DateTime, span: Duration) -> DateTime {
    instant_at_nanos(instant_nanos(at).saturating_add(duration_nanos(span)))
}

/// The instant a count of nanoseconds since the epoch denotes — the inverse of
/// [`instant_nanos`], for a derived instant that is stored and later compared.
///
/// The seconds are floored and the nanoseconds within them are the non-negative remainder,
/// so an instant before the epoch keeps the sub-second part a `Timestamp` holds. A count
/// beyond the seconds a `DateTime` can hold saturates there rather than wrapping to an
/// unrelated instant.
fn instant_at_nanos(nanos: i128) -> DateTime {
    let nanos_per_second = i128::from(NANOS_PER_SECOND);
    let secs = i64::try_from(nanos.div_euclid(nanos_per_second)).unwrap_or(i64::MAX);
    let subsec_nanos = u32::try_from(nanos.rem_euclid(nanos_per_second)).unwrap_or_default();
    DateTime::from_secs_and_nanos(secs, subsec_nanos)
}

/// The Region a stored record was written in: the one its own ARN carries, read through
/// `service_common::arn` so it comes out of the ARN's REGION field rather than out of a
/// substring match.
///
/// `ChangeEvent.Region` is "The Amazon Web Services region where this change event
/// occurred", and the change this event reports is the write of that record, so the Region
/// is the record's own rather than the reader's. A record whose ARN carries no Region label
/// falls back to the emulator's default Region, exactly as [`region_of`] does for a minted
/// ARN.
fn record_region(arn: &str) -> &str {
    ::service_common::arn::region_of(arn, SIGNING_NAME).unwrap_or(DEFAULT_REGION)
}

/// One change event: the dated change `event_name` names — the operation that made it — to
/// the entity `entity` identifies, in `region`.
///
/// The ONE place this service builds a [`ChangeEvent`], so the two reads that report one —
/// `ListEntityEvents` through
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::entity_change_events`] and
/// `ListServiceStates` through
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::observed_service_states`] —
/// cannot differ on what an event of this emulator carries: the only kind of change it
/// records ([`CONFIGURATION_CHANGE_EVENT`]), the documented id of an event it did not read
/// off a CloudTrail record ([`UNRECORDED_EVENT_ID`]), and this store's account
/// (`engine::emulator_account_id()`, the account every ARN it mints carries), in which the
/// record the event is derived from is held. `UserName` ("the name of the user who initiated
/// this change event, if available") stays absent, because no record this service keeps holds
/// the identity that wrote it.
fn change_event(
    at: DateTime,
    entity: &HashMap<String, String>,
    region: &str,
    event_name: &str,
) -> ChangeEvent {
    ChangeEvent {
        timestamp: at,
        account_id: ::engine::emulator_account_id().to_string(),
        region: region.to_string(),
        entity: entity.clone(),
        change_event_type: CONFIGURATION_CHANGE_EVENT.to_string(),
        event_id: UNRECORDED_EVENT_ID.to_string(),
        user_name: None,
        event_name: Some(event_name.to_string()),
    }
}

/// One dated write this service holds the record of, as BOTH reads that report a
/// [`ChangeEvent`] read it: the instant, the ARN of the record the instant was read off, and
/// the operation that wrote that dated member.
///
/// A change event is a report of a WRITE, so everything the event says about where and what
/// the change was comes off the record the write left behind rather than off the request
/// reading it — which is what this type carries from the store to whichever read reports it.
#[derive(Clone, Copy, Debug)]
struct ChangeRecord<'store> {
    at: DateTime,
    arn: &'store str,
    event_name: &'static str,
}

/// The key a [`ChangeRecord`] is ordered by ([`ChangeRecord::order_key`]).
type ChangeOrder<'store> = (::std::cmp::Reverse<i128>, &'store str, &'static str);

impl<'store> ChangeRecord<'store> {
    /// This record reported as the change event of the entity `entity` identifies: the
    /// Region of the record's OWN ARN ([`record_region`]) and the name of the operation that
    /// wrote it, through the one [`change_event`] builder.
    fn event(&self, entity: &HashMap<String, String>) -> ChangeEvent {
        change_event(self.at, entity, record_region(self.arn), self.event_name)
    }

    /// Whether this write's instant falls inside the window, given as the pair of
    /// nanosecond bounds ([`instant_nanos`]), INCLUSIVE at both ends.
    fn inside(&self, (window_start, window_end): (i128, i128)) -> bool {
        let at = instant_nanos(&self.at);
        window_start <= at && at <= window_end
    }

    /// The order a history of changes is read in: newest first, with the record's ARN and the
    /// event's name breaking a tie between two writes at the same instant. ASCENDING in this
    /// key, so the smallest is the latest change — the order has to be TOTAL for a page
    /// cursor into it to be a stable boundary, and for the ONE latest change to be the same
    /// write however it is asked for.
    fn order_key(&self) -> ChangeOrder<'store> {
        (
            ::std::cmp::Reverse(instant_nanos(&self.at)),
            self.arn,
            self.event_name,
        )
    }
}

/// The dated writes one stored SLO is the record of: its creation, and the update that last
/// touched it when one followed.
///
/// A record that has never been updated carries its create instant in both members, which is
/// ONE change, not two. Single-homed here so the reads that report a change of an SLO cannot
/// disagree about which writes it is the record of.
fn slo_change_records(slo: &ServiceLevelObjective) -> Vec<ChangeRecord<'_>> {
    let mut records = vec![ChangeRecord {
        at: slo.created_time,
        arn: &slo.arn,
        event_name: SLO_CREATED_EVENT_NAME,
    }];
    if instant_nanos(&slo.last_updated_time) > instant_nanos(&slo.created_time) {
        records.push(ChangeRecord {
            at: slo.last_updated_time,
            arn: &slo.arn,
            event_name: SLO_UPDATED_EVENT_NAME,
        });
    }
    records
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// Every dated change this service holds a record of, grouped by the STORED RECORD holding
    /// it: the identity of the entity the record was written against, and the dated writes
    /// that record is the evidence of.
    ///
    /// The records held against an entity are the stored SLOs configured on it (dated by the
    /// `CreatedTime` of the create and the `LastUpdatedTime` of an update that followed it,
    /// [`slo_change_records`]) and the instrumentation configurations applied to it (dated by
    /// their `CreatedAt`). Single-homed here, so the reads that report a [`ChangeEvent`] —
    /// `ListEntityEvents` through [`Self::entity_change_events`] and `ListServiceStates`
    /// through [`Self::observed_service_states`] — cannot disagree about WHICH changes this
    /// store is evidence of: each applies its own entity match and its own window to the same
    /// set of records. A record this service no longer holds leaves no change behind, since
    /// the store keeps no tombstone for a deletion.
    ///
    /// GROUPED by record rather than flattened per write, and built from the identity each
    /// record already yields, so enumerating the store copies one identity per record however
    /// many dated writes that record holds — and a read that answers for many entities
    /// enumerates it ONCE and then matches each entity against the result.
    ///
    /// A record whose identity names no attribute at all is left out: `ChangeEvent.Entity` is
    /// `@required` with `length 1..4`, so such a record is the record of a change to nothing
    /// and neither read could report it.
    fn change_records(&self) -> Vec<(HashMap<String, String>, Vec<ChangeRecord<'_>>)> {
        let mut records: Vec<(HashMap<String, String>, Vec<ChangeRecord<'_>>)> = Vec::new();

        for slo in self.slos.values() {
            let Some(attributes) = entity_of(slo).key_attributes else {
                continue;
            };
            if attributes.is_empty() {
                continue;
            }
            records.push((attributes, slo_change_records(slo)));
        }

        for (scope, set) in &self.instrumentation_configurations {
            for stored in set.configurations.values() {
                records.push((
                    service_entity_attributes(&scope.service, &scope.environment),
                    vec![ChangeRecord {
                        at: stored.created_at,
                        arn: &stored.arn,
                        event_name: INSTRUMENTATION_CREATED_EVENT_NAME,
                    }],
                ));
            }
        }

        records
    }

    /// The change events this service holds a record of for the entity `entity` identifies,
    /// as `ListEntityEvents` reports them: those whose instant falls in the requested period,
    /// most recent first.
    ///
    /// A change event is a DATED CHANGE this emulator has a record of, and the records it
    /// holds against an entity are the ones [`Self::change_records`] enumerates: the stored
    /// SLOs configured on it and the instrumentation configurations applied to it. Each event
    /// is read out of one such record, so an event is reported exactly when the store is
    /// evidence of the change it describes.
    ///
    /// The entity is matched the way every other entity read matches it ([`attributes_contain`]):
    /// as few as one of an entity's up to four `Attributes` entries identifies it, so a
    /// record matches when its own identity holds every entry the request named. The
    /// REPORTED `Entity` is the record's identity rather than the request's, so a caller
    /// that named an entity partially is told the full identity the change was recorded
    /// against.
    ///
    /// The period is inclusive at both ends: an event at the instant the window begins or
    /// ends falls within it. Nothing else bounds the set — a window entirely in the future
    /// selects no event because no record is dated after now.
    ///
    /// The order is newest first ([`ChangeRecord::order_key`]), which is the order a history
    /// is read in, with the record's ARN and the event's name breaking a tie between two
    /// events at the same instant: the order has to be TOTAL for a page cursor into it to be
    /// a stable boundary.
    ///
    /// `ListServiceStates` reads the same records ([`latest_change`]) but answers a different
    /// question — the ONE latest change of each DISCOVERED service, matching identities
    /// symmetrically ([`names_same_service`]) because the service it reports is one this
    /// emulator discovered rather than one a request named — so neither read can be expressed
    /// as the other; both report a record as [`ChangeRecord::event`] builds it, which is
    /// where what an event of this emulator carries is stated once.
    pub fn entity_change_events<'store>(
        &'store self,
        entity: &HashMap<String, String>,
        window_start: &DateTime,
        window_end: &DateTime,
    ) -> Vec<ChangeEvent> {
        // `(sort key, event)`: the key is kept beside the event rather than re-derived in the
        // comparator, so the order cannot drift from the key.
        let window = (instant_nanos(window_start), instant_nanos(window_end));
        let mut dated: Vec<(ChangeOrder<'store>, ChangeEvent)> = self
            .change_records()
            .into_iter()
            .filter(|(attributes, _)| attributes_contain(Some(attributes), entity))
            .flat_map(|(attributes, writes)| {
                writes
                    .into_iter()
                    .filter(|record| record.inside(window))
                    .map(move |record| (record.order_key(), record.event(&attributes)))
            })
            .collect();

        dated.sort_by(|left, right| left.0.cmp(&right.0));
        dated.into_iter().map(|(_, event)| event).collect()
    }
}

/// The last change recorded against the service `identity` names inside `window`, as the
/// RECORD that change was read off, or `None` when `records` holds none there.
///
/// `records` is the store's whole set of change records
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::change_records`], the same set
/// `ListEntityEvents` reads its events off), so the two reads cannot disagree about which
/// change of an entity is its most recent one. It is passed in rather than enumerated here
/// because the read that uses this answers for every discovered service at once. The records
/// are matched to the service symmetrically ([`names_same_service`]), because the service
/// reported is one this emulator discovered rather than one a request named.
///
/// Each instant comes off the stored record rather than being re-derived, so the instant
/// reported is the one the read of that record reports. Only an instant INSIDE the window
/// counts: the window is the period the caller asks about, so a change before it is not news
/// in it and a change after it has not happened yet. The bounds are the nanosecond pair every
/// window in this module is compared through ([`instant_nanos`]), so this read and
/// `ListEntityEvents` cannot disagree about whether a change falls inside one.
///
/// The RECORD rather than the bare instant is what comes back, because the event reported for
/// a change states the Region and the operation of the write it was read off
/// ([`ChangeRecord::event`]). "The last" is decided by the TOTAL order a history is read in
/// ([`ChangeRecord::order_key`]), so two writes at one instant cannot resolve differently here
/// and in the read that reports them as a list.
fn latest_change<'store>(
    records: &[(HashMap<String, String>, Vec<ChangeRecord<'store>>)],
    identity: &HashMap<String, String>,
    window: (i128, i128),
) -> Option<ChangeRecord<'store>> {
    records
        .iter()
        .filter(|(attributes, _)| names_same_service(Some(attributes), identity))
        .flat_map(|(_, writes)| writes.iter().copied())
        .filter(|record| record.inside(window))
        .min_by_key(ChangeRecord::order_key)
}

#[cfg(test)]
mod tests;
mod instrumentation;
mod audit;
pub use audit::*;
pub use instrumentation::*;
