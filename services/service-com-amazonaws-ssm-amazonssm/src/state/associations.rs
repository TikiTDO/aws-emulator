// SPDX-License-Identifier: CC0-1.0

//! The State Manager slice: the associations an account holds, and the versions each of
//! them keeps.
//!
//! `CreateAssociation` writes an [`AssociationRecord`] here and `DescribeAssociation`
//! reports it back, so what a create persisted is what a later read reports. An
//! association is keyed by `(region, association id)`: it is created through the Systems
//! Manager endpoint of one Region and is identified there by the id the create minted —
//! "If you created the association by using the Targets parameter, then you must retrieve
//! the association by using the association ID" (`DescribeAssociation`).
//!
//! An association holds its VERSIONS oldest-first ([`AssociationVersionRecord`]), the way
//! a document holds its own: every member a create or an update can set lives on a
//! version rather than on the association, because `DescribeAssociation` reads one of them
//! by `AssociationVersion` ("Specify the association version to retrieve. To view the
//! latest version, either specify $LATEST for this parameter, or omit this parameter").
//! The association itself holds only what identifies it for its whole life — its id, its
//! Region, the document it associates, the node it was created for and its tag set.
//!
//! Nothing here runs an association: dispatching one is the SSM Agent's own poll from a
//! managed node, and no node is registered in this emulator, so no execution exists for an
//! association and nothing reports a last-execution date, an aggregated status count or a
//! triggered alarm for work nothing ran. What a read does report is the association's own
//! progress: a create is owed its move off `Creating` ([`ASSOCIATION_CREATION_DELAY`]),
//! which is derived from the stored creation instant rather than written by a read, so two
//! callers see the same thing whether or not either of them called.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AlarmConfiguration, Association, AssociationDescription, AssociationOverview,
    InstanceAssociationOutputLocation, Target, TargetLocation,
    ASSOCIATION_FILTER_KEY_VALUES,
};

use super::ComAmazonawsSsmAmazonssmState;

/// The `AssociationVersion` value that names whichever version an association holds last:
/// "To view the latest version, either specify $LATEST for this parameter, or omit this
/// parameter" (`DescribeAssociationRequest$AssociationVersion`).
pub const ASSOCIATION_VERSION_LATEST: &str = "$LATEST";

/// The version a create mints: an association's first version, which the modelled
/// `AssociationVersion` pattern spells as a decimal starting at 1.
pub const FIRST_ASSOCIATION_VERSION: u64 = 1;

/// The `AssociationOverview$Status` of an association nothing has run: "Status can be:
/// Pending, Success, or Failed", and the modelled `AssociationStatusName` enum spells the
/// first `Pending` (`crate::ops::types::ASSOCIATION_STATUS_NAME_VALUES`).
pub const ASSOCIATION_STATUS_PENDING: &str = "Pending";

/// The `AssociationOverview$DetailedStatus` of an association whose create has not
/// settled yet: the association is being created, which is the detail a create reports
/// before anything else is true of it.
pub const ASSOCIATION_DETAILED_STATUS_CREATING: &str = "Creating";

/// How long an association reports [`ASSOCIATION_DETAILED_STATUS_CREATING`] for. A create
/// answers before the association is in place, so a caller reading straight after it sees
/// the association still being created; the move off that detail is owed by the clock and
/// is due this long after the create, whether or not anybody reads in between.
pub const ASSOCIATION_CREATION_DELAY: Duration = Duration::from_secs(3);

/// How many associations one Region holds: "You can have at most 2,000 active
/// associations" (`AssociationLimitExceeded`). A create past it is refused with that
/// error rather than persisted.
pub const MAX_ASSOCIATIONS_PER_REGION: usize = 2_000;

/// How many associations this store holds across EVERY Region.
///
/// Emulator resource guard, NOT a published AWS quota: the Region an association is stored
/// under comes from the request's own credential scope (`super::request_region`), so the
/// per-Region limit above alone would be one limit per caller-chosen partition of the key
/// space and the store could grow without bound.
pub const MAX_ASSOCIATIONS_TOTAL: usize = 2 * MAX_ASSOCIATIONS_PER_REGION;

/// How many versions ONE association keeps: "You have reached the maximum number versions
/// allowed for an association. Each association has a limit of 1,000 versions"
/// (`AssociationVersionLimitExceeded`). An update past it is refused with that error rather
/// than appending a version.
pub const MAX_ASSOCIATION_VERSIONS: u64 = 1_000;

/// How this service states that an association can hold no further version.
/// `AssociationVersionLimitExceeded`'s own documentation is "You have reached the maximum
/// number versions allowed for an association. Each association has a limit of 1,000
/// versions." — the limit being [`MAX_ASSOCIATION_VERSIONS`], written the way that sentence
/// writes it.
pub fn association_version_limit_exceeded() -> String {
    format!(
        "You have reached the maximum number versions allowed for an association. Each \
         association has a limit of {} versions.",
        thousands(MAX_ASSOCIATION_VERSIONS)
    )
}

/// `count` with a comma every three digits, which is how the sentence above writes the limit.
fn thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// The `ResourceTypeForTagging` value whose id is an association's minted id (from the
/// modelled enum, `crate::ops::types::RESOURCE_TYPE_FOR_TAGGING_VALUES`).
pub const TAGGING_TYPE_ASSOCIATION: &str = "Association";

/// How this service states a schedule it cannot run an association on. `InvalidSchedule`'s
/// own documentation is "The schedule is invalid. Verify your cron or rate expression and
/// try again."
pub const INVALID_SCHEDULE: &str =
    "The schedule is invalid. Verify your cron or rate expression and try again.";

/// How this service states that `Targets` and `TargetMaps` were both named: "Both Targets
/// and TargetMaps can't be specified together" (`CreateAssociationRequest$TargetMaps`).
pub const TARGETS_AND_TARGET_MAPS: &str = "Both Targets and TargetMaps can't be specified together";

/// How this service states an association an id names none of. `AssociationDoesNotExist`'s
/// own documentation is "The specified association doesn't exist."
pub const ASSOCIATION_DOES_NOT_EXIST: &str = "The specified association doesn't exist.";

/// How this service states an `AssociationVersion` the association does not hold.
/// `InvalidAssociationVersion`'s own documentation is "The version you specified isn't
/// valid. Use ListAssociationVersions to view all versions of an association according to
/// the association ID. Or, use the $LATEST parameter to view the latest version of the
/// association."
pub const INVALID_ASSOCIATION_VERSION: &str =
    "The version you specified isn't valid. Use ListAssociationVersions to view all \
     versions of an association according to the association ID. Or, use the $LATEST \
     parameter to view the latest version of the association.";

/// The smallest interval a `rate` schedule may name, in minutes: the Systems Manager User
/// Guide's "Reference: Cron and rate expressions for Systems Manager" gives 30 minutes as
/// the shortest rate a State Manager association can be applied at.
pub const MIN_RATE_MINUTES: u64 = 30;

/// The units a `rate` schedule is written in, with how many minutes each one is, from the
/// same reference: `rate(value unit)` where the unit is minutes, hours or days. The
/// singular spelling is the one a value of 1 takes ("rate(1 hour)"), the plural the one
/// every larger value takes ("rate(2 hours)").
const RATE_UNITS: &[(&str, &str, u64)] = &[
    ("minute", "minutes", 1),
    ("hour", "hours", 60),
    ("day", "days", 24 * 60),
];

/// How many fields a `cron` schedule carries, from the same reference: minutes, hours,
/// day-of-month, month, day-of-week and year.
const CRON_FIELDS: usize = 6;

/// Why `expression` is not a schedule an association can be applied on, or `None` when it
/// is one.
///
/// "A cron expression when the association will be applied to the targets"
/// (`CreateAssociationRequest$ScheduleExpression`); the Systems Manager User Guide's
/// "Reference: Cron and rate expressions for Systems Manager" gives the two forms such an
/// expression is written in — `cron(<six fields>)` and `rate(<value> <unit>)` — so a value
/// in neither form names no schedule at all. The FORM is what is read here: which minutes
/// a well-formed cron expression selects is the schedule's own meaning, and an association
/// this emulator dispatches nothing for never has to resolve one.
pub fn schedule_rejection(expression: &str) -> Option<&'static str> {
    if let Some(fields) = wrapped_in(expression, "cron") {
        let fields = fields.split_whitespace().count();
        return (fields != CRON_FIELDS).then_some(INVALID_SCHEDULE);
    }
    let Some(rate) = wrapped_in(expression, "rate") else {
        return Some(INVALID_SCHEDULE);
    };
    let mut words = rate.split_whitespace();
    let (Some(value), Some(unit), None) = (words.next(), words.next(), words.next()) else {
        return Some(INVALID_SCHEDULE);
    };
    let Ok(value) = value.parse::<u64>() else {
        return Some(INVALID_SCHEDULE);
    };
    let Some(minutes) = RATE_UNITS.iter().find_map(|(singular, plural, minutes)| {
        let spelled = if value == 1 { singular } else { plural };
        // The value comes off the wire and the modelled `ScheduleExpression` bounds only
        // its length, so the interval is computed with checked arithmetic: a value whose
        // interval does not fit names no interval at all, and is refused like any other
        // expression this form cannot be read from.
        (unit == *spelled).then(|| minutes.checked_mul(value))
    }) else {
        return Some(INVALID_SCHEDULE);
    };
    minutes.is_none_or(|minutes| minutes < MIN_RATE_MINUTES).then_some(INVALID_SCHEDULE)
}

/// What `expression` carries inside `form(..)`, or `None` when it is not written in that
/// form at all.
fn wrapped_in<'a>(expression: &'a str, form: &str) -> Option<&'a str> {
    expression
        .strip_prefix(form)?
        .strip_prefix('(')?
        .strip_suffix(')')
}

/// Whether the output location a create named is not one an association's output can be
/// written to.
///
/// "An Amazon Simple Storage Service (Amazon S3) bucket where you want to store the output
/// details of the request" (`CreateAssociationRequest$OutputLocation`): the location IS the
/// bucket, so an `S3Location` that names no bucket names no location.
pub fn names_no_output_bucket(location: Option<&InstanceAssociationOutputLocation>) -> bool {
    let Some(s3) = location.and_then(|location| location.s3_location.as_ref()) else {
        return false;
    };
    s3.output_s3_bucket_name
        .as_deref()
        .is_none_or(str::is_empty)
}

// ---------------------------------------------------------------------------
// Listing the associations an account holds
// ---------------------------------------------------------------------------

/// The lower bound the modelled `MaxResults` of an association listing carries
/// (`ListAssociationsRequest$MaxResults`, range 1..50).
pub const MIN_ASSOCIATION_PAGE_SIZE: i32 = 1;

/// The upper bound of the same member, and the page size a request naming no `MaxResults`
/// is served up to (see [`super::page_size`]).
pub const MAX_ASSOCIATION_PAGE_SIZE: i32 = 50;

/// The page size an association listing serves (see [`super::page_size`]).
pub fn association_page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(
        max_results,
        MIN_ASSOCIATION_PAGE_SIZE,
        MAX_ASSOCIATION_PAGE_SIZE,
    )
}

/// Whether `candidate` could be an association id, and so whether a `NextToken` is one
/// this service issued: a listing's cursor is the id of the association the next page
/// starts at, so a value the modelled `AssociationId` pattern
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$` does not
/// admit was not issued here.
pub fn is_association_id(candidate: &str) -> bool {
    let mut groups = candidate.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width || !group.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return false;
        }
    }
    groups.next().is_none()
}

/// Whether `key` is a key an association listing filters on — the modelled
/// `AssociationFilterKey` enum, which is what tells an accepted filter from a refused one.
/// The comparison is exact: a filter key is a name, not a prose label.
pub fn is_association_filter_key(key: &str) -> bool {
    ASSOCIATION_FILTER_KEY_VALUES.contains(&key)
}

/// One version of an association: everything a create or an update sets, held as that call
/// supplied it.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AssociationVersionRecord {
    /// This version's number, counting from [`FIRST_ASSOCIATION_VERSION`]; what
    /// `AssociationDescription$AssociationVersion` reports and what an
    /// `AssociationVersion` selector resolves through.
    pub version: u64,
    /// When this version came into being — the create's instant for the first version, an
    /// update's for every later one.
    pub created_at: SystemTime,
    /// "The document version you want to associate with the targets", as supplied; `None`
    /// for a call that named none.
    pub document_version: Option<String>,
    /// "The parameters for the runtime configuration of the document", ordered so two
    /// reads of one version report them alike.
    pub parameters: BTreeMap<String, Vec<String>>,
    /// "The targets for the association", as supplied.
    pub targets: Vec<Target>,
    pub schedule_expression: Option<String>,
    pub output_location: Option<InstanceAssociationOutputLocation>,
    pub association_name: Option<String>,
    pub automation_target_parameter_name: Option<String>,
    pub max_errors: Option<String>,
    pub max_concurrency: Option<String>,
    pub compliance_severity: Option<String>,
    pub sync_compliance: Option<String>,
    /// Whether the association runs only on its schedule; modelled with a default of
    /// `false`, which is the value a call that names none runs with.
    pub apply_only_at_cron_interval: bool,
    pub calendar_names: Vec<String>,
    pub target_locations: Vec<TargetLocation>,
    pub schedule_offset: Option<i32>,
    pub duration: Option<i32>,
    pub target_maps: Vec<std::collections::HashMap<String, Vec<String>>>,
    pub alarm_configuration: Option<AlarmConfiguration>,
    pub association_dispatch_assume_role: Option<String>,
}

/// One association, as `CreateAssociation` persisted it and `DescribeAssociation` reports
/// it.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AssociationRecord {
    /// "The association ID" the create minted; part of the store key and what every read
    /// resolves the association by.
    pub association_id: String,
    /// The Region the association was created in, the other half of its key.
    pub region: String,
    /// "The name of the SSM document" the association applies, by the name the document is
    /// held under rather than any ARN spelling the create may have named it by.
    pub name: String,
    /// "The managed node ID" the association was created for, for a create that named one
    /// through the deprecated `InstanceId` member.
    pub instance_id: Option<String>,
    /// "The date when the association was made".
    pub created_at: SystemTime,
    /// This association's versions, oldest first and never empty: a create mints
    /// [`FIRST_ASSOCIATION_VERSION`].
    pub versions: Vec<AssociationVersionRecord>,
    /// The tag set the association carries, written by its create and by the tag
    /// operations (`super::tagging`).
    pub tags: BTreeMap<String, String>,
}

impl AssociationRecord {
    /// The association a create persists: its first version, holding everything that call
    /// set.
    pub fn new(
        association_id: String,
        region: String,
        name: String,
        instance_id: Option<String>,
        created_at: SystemTime,
        version: AssociationVersionRecord,
        tags: BTreeMap<String, String>,
    ) -> Self {
        Self {
            association_id,
            region,
            name,
            instance_id,
            created_at,
            versions: vec![version],
            tags,
        }
    }

    /// The version this association holds last, which is what a read that names none
    /// reports.
    pub fn latest(&self) -> Option<&AssociationVersionRecord> {
        self.versions.last()
    }

    /// The number of that version, or [`FIRST_ASSOCIATION_VERSION`] for a record holding
    /// none.
    pub fn latest_version(&self) -> u64 {
        self.latest()
            .map(|version| version.version)
            .unwrap_or(FIRST_ASSOCIATION_VERSION)
    }

    /// The number the next version to be appended takes.
    pub fn next_version(&self) -> u64 {
        self.latest_version() + 1
    }

    /// "The date when the association was last updated": when the version it holds last
    /// came into being, which for an association nothing has updated is its creation.
    pub fn updated_at(&self) -> SystemTime {
        self.latest()
            .map(|version| version.created_at)
            .unwrap_or(self.created_at)
    }

    /// The version `selector` names — [`ASSOCIATION_VERSION_LATEST`] or a decimal version
    /// number — or `None` when the association holds no such version, which is what
    /// `InvalidAssociationVersion` reports.
    pub fn select(&self, selector: &str) -> Option<&AssociationVersionRecord> {
        if selector == ASSOCIATION_VERSION_LATEST {
            return self.latest();
        }
        let wanted: u64 = selector.parse().ok()?;
        self.versions
            .iter()
            .find(|version| version.version == wanted)
    }

    /// Append `version` to this association, which is what an update persists.
    pub fn push_version(&mut self, version: AssociationVersionRecord) {
        self.versions.push(version);
    }

    /// "Information about the association" as of `now`: its own progress, which is all
    /// that is true of an association nothing has run.
    ///
    /// The status is `Pending` — no execution has reported a result — and the detailed
    /// status is `Creating` for as long as the create is still settling
    /// ([`ASSOCIATION_CREATION_DELAY`]), after which there is no further detail to report
    /// and the aggregated count stays empty, because a count of targets by status is a
    /// count over executions and there are none.
    pub fn overview(&self, now: SystemTime) -> AssociationOverview {
        let creating = now < self.created_at + ASSOCIATION_CREATION_DELAY;
        AssociationOverview {
            status: Some(ASSOCIATION_STATUS_PENDING.to_string()),
            detailed_status: creating.then(|| ASSOCIATION_DETAILED_STATUS_CREATING.to_string()),
            association_status_aggregated_count: None,
        }
    }

    /// The modelled `AssociationDescription` of one of this association's versions, as of
    /// `now`.
    ///
    /// Every member is what the create or the update that wrote `version` supplied, the id
    /// and the version number this store minted, or the overview above. A member no call
    /// set is left unset rather than invented, and the members that describe an execution —
    /// the last and last successful execution dates, the per-node status and the alarms an
    /// execution invoked — are unset because nothing has run this association.
    pub fn to_association_description(
        &self,
        version: &AssociationVersionRecord,
        now: SystemTime,
    ) -> AssociationDescription {
        AssociationDescription {
            name: Some(self.name.clone()),
            instance_id: self.instance_id.clone(),
            association_version: Some(version.version.to_string()),
            date: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            last_update_association_date: Some(::aws_smithy_types::DateTime::from(
                self.updated_at(),
            )),
            // "The association status" is the result a managed node reports for the
            // association through `UpdateAssociationStatus`; no node has reported one.
            status: None,
            overview: Some(self.overview(now)),
            document_version: version.document_version.clone(),
            automation_target_parameter_name: version.automation_target_parameter_name.clone(),
            parameters: (!version.parameters.is_empty()).then(|| {
                version
                    .parameters
                    .iter()
                    .map(|(name, values)| (name.clone(), values.clone()))
                    .collect()
            }),
            association_id: Some(self.association_id.clone()),
            targets: (!version.targets.is_empty()).then(|| version.targets.clone()),
            schedule_expression: version.schedule_expression.clone(),
            output_location: version.output_location.clone(),
            last_execution_date: None,
            last_successful_execution_date: None,
            association_name: version.association_name.clone(),
            max_errors: version.max_errors.clone(),
            max_concurrency: version.max_concurrency.clone(),
            compliance_severity: version.compliance_severity.clone(),
            sync_compliance: version.sync_compliance.clone(),
            apply_only_at_cron_interval: version.apply_only_at_cron_interval,
            calendar_names: (!version.calendar_names.is_empty())
                .then(|| version.calendar_names.clone()),
            target_locations: (!version.target_locations.is_empty())
                .then(|| version.target_locations.clone()),
            schedule_offset: version.schedule_offset,
            duration: version.duration,
            target_maps: (!version.target_maps.is_empty()).then(|| version.target_maps.clone()),
            alarm_configuration: version.alarm_configuration.clone(),
            triggered_alarms: None,
            association_dispatch_assume_role: version.association_dispatch_assume_role.clone(),
        }
    }

    /// The modelled `Association` of this association as of `now`: what a listing reports,
    /// which is this association's version in force narrowed to the members that shape
    /// carries.
    ///
    /// Every member is read from the same place the `AssociationDescription` above reads
    /// it, so a listing and a describe of one association never disagree:
    /// `AssociationVersion` is the version in force, and the members a version holds
    /// (`DocumentVersion`, the targets, the schedule, the association name, the schedule
    /// offset, the duration and the target maps) are that version's. `LastExecutionDate` is
    /// unset because nothing has run this association.
    pub fn to_association(&self, now: SystemTime) -> Association {
        let version = self.latest();
        Association {
            name: Some(self.name.clone()),
            instance_id: self.instance_id.clone(),
            association_id: Some(self.association_id.clone()),
            association_version: Some(self.latest_version().to_string()),
            document_version: version.and_then(|version| version.document_version.clone()),
            targets: version
                .map(|version| version.targets.clone())
                .filter(|targets| !targets.is_empty()),
            last_execution_date: None,
            overview: Some(self.overview(now)),
            schedule_expression: version.and_then(|version| version.schedule_expression.clone()),
            association_name: version.and_then(|version| version.association_name.clone()),
            schedule_offset: version.and_then(|version| version.schedule_offset),
            duration: version.and_then(|version| version.duration),
            target_maps: version
                .map(|version| version.target_maps.clone())
                .filter(|target_maps| !target_maps.is_empty()),
        }
    }

    /// Whether the filter `key`/`value` admits this association as of `now`.
    ///
    /// One filter compares ONE attribute of the association against the whole value it
    /// carries: "Use a filter to return a more specific list of results"
    /// (`ListAssociationsRequest$AssociationFilterList`), and the keys are the modelled
    /// `AssociationFilterKey` enum ([`is_association_filter_key`]). A key naming an
    /// attribute no association here carries admits none: a key this service does not know
    /// admits none either, which is a case a caller cannot reach, because an unknown key is
    /// refused before any association is read.
    pub fn matches_filter(&self, key: &str, value: &str, now: SystemTime) -> bool {
        let version = self.latest();
        match key {
            // "The name of the SSM document" the association applies, by the name the
            // document is held under.
            "Name" => self.name == value,
            // "The ID created by the system when you create an association"
            // (`Association$AssociationId`).
            "AssociationId" => self.association_id == value,
            // "Filtering associations using the InstanceID attribute only returns legacy
            // associations created using the InstanceID attribute. Associations targeting
            // the managed node that are part of the Target Attributes ResourceGroup or Tags
            // aren't returned" (same member), so the node a create NAMED is compared and the
            // nodes a target selects are not.
            "InstanceId" => self.instance_id.as_deref() == Some(value),
            // "The association name" (`Association$AssociationName`), as the version in
            // force carries it.
            "AssociationName" => {
                version.and_then(|version| version.association_name.as_deref()) == Some(value)
            }
            // "The status of the association" (`AssociationOverview$Status`), which is the
            // association's own progress as of this instant.
            "AssociationStatusName" => self.overview(now).status.as_deref() == Some(value),
            // The resource group the association targets, named by either target-key
            // spelling the `Target` doc gives a resource group
            // (`super::commands::TARGET_KEY_RESOURCE_GROUP_NAME` and
            // `TARGET_KEY_RESOURCE_GROUP`).
            "ResourceGroupName" => version.is_some_and(|version| {
                version.targets.iter().any(|target| {
                    target.key.as_deref().is_some_and(|key| {
                        key == super::commands::TARGET_KEY_RESOURCE_GROUP_NAME
                            || key == super::commands::TARGET_KEY_RESOURCE_GROUP
                    }) && target
                        .values
                        .iter()
                        .flatten()
                        .any(|targeted| targeted == value)
                })
            }),
            // "The date on which the association was last run"
            // (`Association$LastExecutionDate`): running an association is the SSM Agent's
            // own poll from a managed node and happens on no node here, so no association
            // has a date either side of the one a filter names.
            "LastExecutedBefore" | "LastExecutedAfter" => false,
            // A cloud connector is not something a create or an update of an association
            // names, so no association carries a connector id to compare.
            "CloudConnectorId" => false,
            _ => false,
        }
    }

    /// The description of the version `selector` names, or `None` when this association
    /// holds no such version ([`AssociationRecord::select`]).
    pub fn description_of(
        &self,
        selector: &str,
        now: SystemTime,
    ) -> Option<AssociationDescription> {
        let version = self.select(selector)?;
        Some(self.to_association_description(version, now))
    }

    /// The description of the version this association holds last.
    pub fn latest_description(&self, now: SystemTime) -> Option<AssociationDescription> {
        self.description_of(ASSOCIATION_VERSION_LATEST, now)
    }
}

impl ComAmazonawsSsmAmazonssmState {
    /// The association `association_id` names in `region`, or `None` when that Region
    /// holds none — which is what `AssociationDoesNotExist` reports.
    pub fn association(&self, region: &str, association_id: &str) -> Option<&AssociationRecord> {
        self.associations
            .get(&(region.to_string(), association_id.to_string()))
    }

    /// The same association, to append a version or rewrite its tags.
    pub fn association_mut(
        &mut self,
        region: &str,
        association_id: &str,
    ) -> Option<&mut AssociationRecord> {
        self.associations
            .get_mut(&(region.to_string(), association_id.to_string()))
    }

    /// Every association `region` holds, in ascending association-id order, as
    /// `(association id, record)`. The order is what makes a listing's resume cursor
    /// total: it is an association id, not an offset into a list that may have changed
    /// between two pages.
    pub fn associations_in_region<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a AssociationRecord)> + 'a {
        self.associations
            .iter()
            .filter(move |((held, _), _)| held == region)
            .map(|((_, id), record)| (id.as_str(), record))
    }

    /// The association `region` holds for the document `name` on the node `instance_id`,
    /// or `None` when it holds none.
    ///
    /// The legacy pairing: "InstanceId has been deprecated. To specify a managed node ID
    /// for an association, use the Targets parameter"
    /// (`CreateAssociationRequest$InstanceId`), and while a node may be named that way it
    /// is associated with one document at most once — a second create of the same pair is
    /// the association that already exists.
    pub fn association_of_node(
        &self,
        region: &str,
        name: &str,
        instance_id: &str,
    ) -> Option<&AssociationRecord> {
        self.associations
            .iter()
            .filter(|((held, _), _)| held == region)
            .map(|(_, record)| record)
            .find(|record| {
                record.name == name && record.instance_id.as_deref() == Some(instance_id)
            })
    }

    /// Store `record` as `region`'s association of that id.
    pub fn insert_association(&mut self, record: AssociationRecord) {
        let key = (record.region.clone(), record.association_id.clone());
        self.associations.insert(key, record);
    }

    /// Drop `region`'s association of that id, answering whether it held one.
    pub fn remove_association(&mut self, region: &str, association_id: &str) -> bool {
        self.associations
            .remove(&(region.to_string(), association_id.to_string()))
            .is_some()
    }

    /// How many associations `region` holds, which is what the documented association
    /// limit is measured against ([`MAX_ASSOCIATIONS_PER_REGION`]).
    pub fn associations_in_region_count(&self, region: &str) -> usize {
        self.associations
            .keys()
            .filter(|(held, _)| held == region)
            .count()
    }

    /// Whether this store can take another association at all: the emulator's own total
    /// guard ([`MAX_ASSOCIATIONS_TOTAL`]), measured across EVERY Region.
    pub fn can_hold_another_association(&self) -> bool {
        self.associations.len() < MAX_ASSOCIATIONS_TOTAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One version record, which is all the version arithmetic below reads.
    fn version(number: u64) -> AssociationVersionRecord {
        AssociationVersionRecord {
            version: number,
            created_at: SystemTime::UNIX_EPOCH,
            document_version: None,
            parameters: BTreeMap::new(),
            targets: Vec::new(),
            schedule_expression: None,
            output_location: None,
            association_name: None,
            automation_target_parameter_name: None,
            max_errors: None,
            max_concurrency: None,
            compliance_severity: None,
            sync_compliance: None,
            apply_only_at_cron_interval: false,
            calendar_names: Vec::new(),
            target_locations: Vec::new(),
            schedule_offset: None,
            duration: None,
            target_maps: Vec::new(),
            alarm_configuration: None,
            association_dispatch_assume_role: None,
        }
    }

    /// An association holding versions up to `held`.
    fn record(held: u64) -> AssociationRecord {
        let mut record = AssociationRecord::new(
            "00000000-0000-4000-8000-000000000000".to_string(),
            "us-west-2".to_string(),
            "AWS-UpdateSSMAgent".to_string(),
            None,
            SystemTime::UNIX_EPOCH,
            version(FIRST_ASSOCIATION_VERSION),
            BTreeMap::new(),
        );
        for number in (FIRST_ASSOCIATION_VERSION + 1)..=held {
            record.push_version(version(number));
        }
        record
    }

    /// RULE: a create mints version 1 and each update appends the next number, so the number
    /// an update would take is one past the version in force — which is what the documented
    /// per-association version limit is measured against.
    #[test]
    fn the_next_version_is_one_past_the_version_in_force() {
        // (case, how many versions the association holds, the next number, whether an update
        // is still within the limit)
        let rows: &[(&str, u64, u64, bool)] = &[
            ("a create alone", 1, 2, true),
            ("a few updates", 4, 5, true),
            ("one version short of the limit", 999, 1_000, true),
            ("the limit reached", MAX_ASSOCIATION_VERSIONS, 1_001, false),
        ];
        for (case, held, expected, within) in rows {
            let record = record(*held);
            assert_eq!(
                record.latest_version(),
                *held,
                "case {case:?}: expected the version in force to be {held}"
            );
            let next = record.next_version();
            assert_eq!(
                next, *expected,
                "case {case:?}: expected the next version to be {expected}, got {next}"
            );
            assert_eq!(
                next <= MAX_ASSOCIATION_VERSIONS,
                *within,
                "case {case:?}: expected within-the-limit to be {within} at version {next}"
            );
        }
    }

    /// RULE: the refusal an association past its version limit answers is AWS's own sentence,
    /// carrying the limit the way that sentence writes it.
    #[test]
    fn the_version_limit_refusal_is_aws_own_sentence() {
        let message = association_version_limit_exceeded();
        let expected = "You have reached the maximum number versions allowed for an \
                        association. Each association has a limit of 1,000 versions.";
        assert_eq!(
            message, expected,
            "case \"the version limit\": expected {expected:?}, got {message:?}"
        );
    }
}
