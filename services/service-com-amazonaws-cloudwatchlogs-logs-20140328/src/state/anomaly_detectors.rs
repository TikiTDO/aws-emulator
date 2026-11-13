// SPDX-License-Identifier: CC0-1.0

//! The LOG ANOMALY DETECTORS this account holds, and the LOOKUP TABLES their queries enrich
//! log events from, as they are stored.

use super::*;

/// One log anomaly detector — "one anomaly detector in the account" — as it is stored.
/// Every member of the model's `AnomalyDetector` shape is reported from this record, so
/// `GetLogAnomalyDetector` and `ListLogAnomalyDetectors` answer from state alone.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AnomalyDetectorRecord {
    /// `AnomalyDetector$anomalyDetectorArn` — "The ARN of the anomaly detector", as
    /// [`anomaly_detector_arn`] builds it. Also the key the store is keyed by.
    pub arn: String,
    /// `AnomalyDetector$detectorName` — "The name of the anomaly detector". Optional on the
    /// create, so a detector created without one reports no name.
    pub detector_name: Option<String>,
    /// `AnomalyDetector$logGroupArnList` — "A list of the ARNs of the log groups that this
    /// anomaly detector watches", exactly as the create named them.
    pub log_group_arn_list: Vec<String>,
    /// `AnomalyDetector$evaluationFrequency` — "Specifies how often the anomaly detector
    /// runs and look for anomalies", one value of the model's `EvaluationFrequency` enum.
    /// `None` when neither the create nor an update named one: AWS documents no default for
    /// the member, so none is invented here.
    pub evaluation_frequency: Option<String>,
    /// `AnomalyDetector$filterPattern` — the pattern that limits "the anomaly detection
    /// model to examine only log events that match", when one was named.
    pub filter_pattern: Option<String>,
    /// `AnomalyDetector$anomalyDetectorStatus`, one of the
    /// [`ANOMALY_DETECTOR_STATUS_INITIALIZING`] family.
    pub status: String,
    /// `AnomalyDetector$kmsKeyId` — "The ARN of the KMS key assigned to this anomaly
    /// detector, if any."
    pub kms_key_id: Option<String>,
    /// `AnomalyDetector$creationTimeStamp` — "when this anomaly detector was created", in
    /// epoch milliseconds.
    pub creation_time_ms: i64,
    /// `AnomalyDetector$lastModifiedTimeStamp` — "when this anomaly detector was most
    /// recently modified", in epoch milliseconds. A status the clock owes is not a
    /// modification of the detector's configuration, so only a call that changes that
    /// configuration moves this on.
    pub last_modified_time_ms: i64,
    /// `AnomalyDetector$anomalyVisibilityTime` — "The number of days used as the life cycle
    /// of anomalies". `None` when no call named one; AWS documents no default.
    pub anomaly_visibility_time: Option<i64>,
    /// The tags `CreateLogAnomalyDetector$tags` assigned to this detector. A `BTreeMap` so
    /// the stored order is stable; no `AnomalyDetector` member reports them, so they are
    /// read back through the tagging operations once those reach this resource.
    pub tags: BTreeMap<String, String>,
    /// The instant this detector is due to reach its next status, stored rather than waited
    /// for: a reporting operation materialises the transition once the clock is past it
    /// ([`AnomalyDetectorRecord::materialize_due_transitions`]). `None` once the detector is
    /// in a status the clock owes nothing after.
    pub status_due_ms: Option<i64>,
    /// The status a PAUSED detector returns to when a later call restarts it, and how much
    /// of the span to its next status was still outstanding when it was paused — so a
    /// detector paused mid-lifecycle resumes that lifecycle instead of jumping ahead of it
    /// or starting it over. `None` for a detector no call has paused.
    pub resume_status: Option<String>,
    /// What was left of [`AnomalyDetectorRecord::status_due_ms`] when this detector was
    /// paused, re-armed against the clock by the call that restarts it. `None` when the
    /// status it was paused in is one the clock owed nothing after.
    pub resume_due_in_ms: Option<i64>,
    /// The anomalies this detector has REPORTED, keyed by `anomalyId`: the set
    /// `ListAnomalies` enumerates and every suppression request resolves its
    /// `anomalyId` / `patternId` against.
    ///
    /// An anomaly is stored as the modelled `Anomaly` shape itself rather than a parallel
    /// record, so every member its reader reports round-trips without a field-by-field copy
    /// that can silently drop one; the suppression members are filled in by the reader from
    /// [`AnomalyDetectorRecord::suppressions`], and the detector's own ARN and log groups
    /// from the record around it.
    ///
    /// A detector reports an anomaly from its own pattern-recognition pass over the log
    /// groups it watches — "The anomaly detector uses pattern recognition to find anomalies,
    /// which are unusual log events" (`CreateLogAnomalyDetector`) — so no REQUEST adds one,
    /// and an id outside this map names an anomaly this detector has not found.
    pub anomalies: BTreeMap<String, crate::ops::types::Anomaly>,
    /// The suppressions placed on this detector's findings, keyed by the `anomalyId` or
    /// `patternId` suppressed. `UpdateAnomaly` writes one here and removes it again when a
    /// later call ends the suppression.
    pub suppressions: BTreeMap<String, FindingSuppression>,
}

/// One suppression `UpdateAnomaly` placed on a finding of a detector: "Use this operation to
/// suppress anomaly detection for a specified anomaly or pattern."
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct FindingSuppression {
    /// The `SuppressionType` the call named — `INFINITE` or `LIMITED` — or `None` when it
    /// named none and the suppression lasts until a later call ends it.
    pub suppression_type: Option<String>,
    /// When the suppression was placed, in epoch milliseconds (`Anomaly$suppressedDate`).
    pub suppressed_at_ms: i64,
    /// When a `LIMITED` suppression runs out, in epoch milliseconds
    /// (`Anomaly$suppressedUntil`). `None` for one that does not run out.
    pub suppressed_until_ms: Option<i64>,
    /// `UpdateAnomaly$baseline` — "Set this to true to prevent CloudWatch Logs from
    /// displaying this behavior as an anomaly in the future."
    pub baseline: bool,
}

/// How long a `suppressionPeriod` lasts, in milliseconds: its `value` in the
/// `suppressionUnit` beside it (`SuppressionUnit` is `SECONDS | MINUTES | HOURS`). A period
/// naming no unit is taken as the value in seconds, the smallest unit the enum carries.
pub fn suppression_period_ms(period: &crate::ops::types::SuppressionPeriod) -> i64 {
    let unit_ms = match period.suppression_unit.as_deref() {
        Some("MINUTES") => 60 * 1_000,
        Some("HOURS") => 60 * 60 * 1_000,
        _ => 1_000,
    };
    i64::from(period.value).saturating_mul(unit_ms)
}

impl AnomalyDetectorRecord {
    /// A brand-new anomaly detector over the log groups it was asked to watch: still
    /// initializing, and due to start training [`ANOMALY_DETECTOR_TRAINING_DELAY_MS`] after
    /// it was created. This is what `CreateLogAnomalyDetector` stores.
    ///
    /// The optional members a create may name — `detectorName`, `evaluationFrequency`,
    /// `filterPattern`, `kmsKeyId`, `anomalyVisibilityTime`, `tags` — are left at their "the
    /// request named none" value and set on the returned record by the caller that has them.
    pub fn new(arn: &str, log_group_arn_list: Vec<String>, creation_time_ms: i64) -> Self {
        Self {
            arn: arn.to_string(),
            detector_name: None,
            log_group_arn_list,
            evaluation_frequency: None,
            filter_pattern: None,
            status: ANOMALY_DETECTOR_STATUS_INITIALIZING.to_string(),
            kms_key_id: None,
            creation_time_ms,
            last_modified_time_ms: creation_time_ms,
            anomaly_visibility_time: None,
            tags: BTreeMap::new(),
            status_due_ms: Some(creation_time_ms.saturating_add(ANOMALY_DETECTOR_TRAINING_DELAY_MS)),
            // Nothing has paused a detector the create just stored.
            resume_status: None,
            resume_due_in_ms: None,
            // A detector that has not run a pass over its log groups yet has reported
            // nothing, so it has no finding to suppress either.
            anomalies: BTreeMap::new(),
            suppressions: BTreeMap::new(),
        }
    }

    /// This detector as the model's `AnomalyDetector` structure, which is what
    /// `ListLogAnomalyDetectors` reports, so one record renders the same way through any
    /// reader of it.
    pub fn reported(&self) -> crate::ops::types::AnomalyDetector {
        crate::ops::types::AnomalyDetector {
            anomaly_detector_arn: Some(self.arn.clone()),
            detector_name: self.detector_name.clone(),
            log_group_arn_list: Some(self.log_group_arn_list.clone()),
            evaluation_frequency: self.evaluation_frequency.clone(),
            filter_pattern: self.filter_pattern.clone(),
            anomaly_detector_status: Some(self.status.clone()),
            kms_key_id: self.kms_key_id.clone(),
            creation_time_stamp: self.creation_time_ms,
            last_modified_time_stamp: self.last_modified_time_ms,
            anomaly_visibility_time: self.anomaly_visibility_time,
        }
    }

    /// Whether this detector has reported the anomaly `anomaly_id` names.
    pub fn reports_anomaly(&self, anomaly_id: &str) -> bool {
        self.anomalies.contains_key(anomaly_id)
    }

    /// Whether this detector has reported an anomaly of the pattern `pattern_id` names.
    pub fn reports_pattern(&self, pattern_id: &str) -> bool {
        self.anomalies
            .values()
            .any(|anomaly| anomaly.pattern_id == pattern_id)
    }

    /// The suppression in force over `anomaly` at `now_ms`, and whether it was placed on the
    /// anomaly's PATTERN rather than on the anomaly itself.
    ///
    /// "If you suppress a pattern, CloudWatch Logs won't report any anomalies related to that
    /// pattern" (`UpdateAnomaly`), so a pattern-level suppression covers every anomaly of
    /// that pattern; a suppression placed on the anomaly itself is the more specific one and
    /// wins. A `LIMITED` suppression stops being in force once the clock passes the instant
    /// it was due to run out, which is DERIVED here rather than written by a sweep.
    pub fn suppression_in_force(
        &self,
        anomaly: &crate::ops::types::Anomaly,
        now_ms: i64,
    ) -> Option<(&FindingSuppression, bool)> {
        let in_force = |suppression: &FindingSuppression| {
            suppression
                .suppressed_until_ms
                .is_none_or(|until| now_ms < until)
        };
        if let Some(suppression) = self.suppressions.get(&anomaly.anomaly_id).filter(|s| in_force(s))
        {
            return Some((suppression, false));
        }
        self.suppressions
            .get(&anomaly.pattern_id)
            .filter(|s| in_force(s))
            .map(|suppression| (suppression, true))
    }

    /// This detector's anomalies as `ListAnomalies` reports them, in `anomalyId` order: the
    /// stored shape, with the detector's own ARN and log groups and the suppression members
    /// the store keeps beside it.
    ///
    /// `state` is DERIVED here from the suppression in force at `now_ms` rather than read
    /// off the stored anomaly: "Indicates the current state of this anomaly. If it is still
    /// being treated as an anomaly, the value is Active. If you have suppressed this anomaly
    /// by using the UpdateAnomaly operation, the value is Suppressed. If this behavior is now
    /// considered to be normal, the value is Baseline." (`Anomaly$state`) — so a suppression
    /// placed as baseline behavior reports [`ANOMALY_STATE_BASELINE`], any other suppression
    /// [`ANOMALY_STATE_SUPPRESSED`], and an anomaly whose suppression a later call ended or
    /// the clock ran out of reports [`ANOMALY_STATE_ACTIVE`] again.
    pub fn reported_anomalies(&self, now_ms: i64) -> Vec<crate::ops::types::Anomaly> {
        self.anomalies
            .values()
            .map(|anomaly| {
                let mut reported = anomaly.clone();
                reported.anomaly_detector_arn = self.arn.clone();
                reported.log_group_arn_list = self.log_group_arn_list.clone();
                match self.suppression_in_force(anomaly, now_ms) {
                    Some((suppression, pattern_level)) => {
                        reported.suppressed = Some(true);
                        reported.suppressed_date = suppression.suppressed_at_ms;
                        reported.suppressed_until = suppression.suppressed_until_ms.unwrap_or(0);
                        reported.is_pattern_level_suppression = Some(pattern_level);
                        reported.state = if suppression.baseline {
                            ANOMALY_STATE_BASELINE.to_string()
                        } else {
                            ANOMALY_STATE_SUPPRESSED.to_string()
                        };
                    }
                    None => {
                        reported.suppressed = Some(false);
                        reported.suppressed_date = 0;
                        reported.suppressed_until = 0;
                        reported.is_pattern_level_suppression = Some(false);
                        reported.state = ANOMALY_STATE_ACTIVE.to_string();
                    }
                }
                reported
            })
            .collect()
    }

    /// Suppress the finding `id` names, or end its suppression.
    ///
    /// "If you have previously used this operation to suppress detection of a pattern or
    /// anomaly, you can use it again to cause CloudWatch Logs to end the suppression. To do
    /// this, use this operation and specify the anomaly or pattern to stop suppressing, and
    /// omit the suppressionType and suppressionPeriod parameters." (`UpdateAnomaly`) — so a
    /// call naming neither member removes the suppression the finding carries, and any other
    /// call places one. "If you specify INFINITE, any value for suppressionPeriod is
    /// ignored."
    pub fn suppress_finding(
        &mut self,
        id: &str,
        suppression_type: Option<&str>,
        suppression_period: Option<&crate::ops::types::SuppressionPeriod>,
        baseline: bool,
        now_ms: i64,
    ) {
        if suppression_type.is_none() && suppression_period.is_none() {
            self.suppressions.remove(id);
            return;
        }
        let suppressed_until_ms = (suppression_type != Some(SUPPRESSION_TYPE_INFINITE))
            .then(|| {
                suppression_period
                    .map(|period| now_ms.saturating_add(suppression_period_ms(period)))
            })
            .flatten();
        self.suppressions.insert(
            id.to_string(),
            FindingSuppression {
                suppression_type: suppression_type.map(ToString::to_string),
                suppressed_at_ms: now_ms,
                suppressed_until_ms,
                baseline,
            },
        );
    }

    /// Materialise the status changes the clock owes this detector: a detector whose next
    /// status was due before `now_ms` is already in it, and a detector that then became due
    /// for the one after is in that one too — each change taken AT the instant it was due,
    /// not at the time something happened to look.
    pub fn materialize_due_transitions(&mut self, now_ms: i64) {
        while let Some(due) = self.status_due_ms {
            if due > now_ms {
                return;
            }
            match self.status.as_str() {
                ANOMALY_DETECTOR_STATUS_INITIALIZING => {
                    self.status = ANOMALY_DETECTOR_STATUS_TRAINING.to_string();
                    self.status_due_ms =
                        Some(due.saturating_add(ANOMALY_DETECTOR_ANALYSIS_DELAY_MS));
                }
                ANOMALY_DETECTOR_STATUS_TRAINING => {
                    self.status = ANOMALY_DETECTOR_STATUS_ANALYZING.to_string();
                    self.status_due_ms = None;
                }
                // Every other status is one the clock owes nothing after: it was reached
                // because a call put the detector there.
                _ => self.status_due_ms = None,
            }
        }
    }

    /// Pause this detector, or restart a paused one: "Use this parameter to pause or restart
    /// the anomaly detector." (`UpdateLogAnomalyDetector$enabled`)
    ///
    /// Pausing takes the detector out of the status the clock had it in and into
    /// [`ANOMALY_DETECTOR_STATUS_PAUSED`], which only another call leaves. What the clock
    /// still owed it is kept rather than discarded, so restarting resumes the lifecycle from
    /// where the pause interrupted it — the remaining span is re-armed against the clock at
    /// the restart — instead of jumping a detector that was still initializing straight to
    /// analyzing. A detector the clock had already carried to its last status restarts into
    /// that status with nothing further owed.
    ///
    /// Asking for the state the detector is already in changes no status: pausing a paused
    /// detector keeps the one pause, and restarting a running one leaves the clock's own
    /// progression alone.
    pub fn set_enabled(&mut self, enabled: bool, now_ms: i64) {
        // What the clock already made due happened before this call, so it is materialised
        // first: the status this call pauses, and the span it keeps, are the ones in force at
        // the instant the call arrived.
        self.materialize_due_transitions(now_ms);
        let paused = self.status == ANOMALY_DETECTOR_STATUS_PAUSED;
        match (enabled, paused) {
            (false, false) => {
                self.resume_status = Some(self.status.clone());
                self.resume_due_in_ms = self
                    .status_due_ms
                    .map(|due| due.saturating_sub(now_ms).max(0));
                self.status = ANOMALY_DETECTOR_STATUS_PAUSED.to_string();
                self.status_due_ms = None;
            }
            (true, true) => {
                self.status = self
                    .resume_status
                    .take()
                    .unwrap_or_else(|| ANOMALY_DETECTOR_STATUS_ANALYZING.to_string());
                self.status_due_ms = self
                    .resume_due_in_ms
                    .take()
                    .map(|remaining| now_ms.saturating_add(remaining));
            }
            _ => {}
        }
    }
}

/// An anomaly detector's ARN, the form `AnomalyDetector$anomalyDetectorArn` carries: an
/// `anomaly-detector` resource of this service, identified by the detector's own id
/// ([`mint_anomaly_detector_id`]).
pub fn anomaly_detector_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("anomaly-detector:{id}"))
}

/// The identifier an anomaly detector's ARN carries, minted from the monotonic
/// anomaly-detector counter so it is unique and derived from stored state. AWS gives the
/// resource a UUID-shaped identifier, and the modelled `AnomalyDetectorArn` pattern
/// (`^[\w#+=/:,.@-]*$`) admits exactly that, so it is minted through the shared
/// [`mint_uuid_from_seq`] every UUID-shaped id of this service comes from.
pub fn mint_anomaly_detector_id(seq: u64) -> String {
    mint_uuid_from_seq(seq)
}

/// The resource-segment prefix of a lookup table's ARN. AWS documents no format for
/// `LookupTable$lookupTableArn`, so it is built the way every other named CloudWatch Logs
/// resource of this service builds one — a resource type then the name that identifies the
/// resource, as `log-group:<name>` and `delivery-destination:<name>` are built.
pub const LOOKUP_TABLE_ARN_PREFIX: &str = "lookup-table:";

/// A lookup table's ARN, derived from the name that identifies it ("The name must be
/// unique within your account and Region"), so the same table always answers under the
/// same ARN and no two tables share one.
pub fn lookup_table_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("logs", region, &format!("{LOOKUP_TABLE_ARN_PREFIX}{name}"))
}

/// The lookup-table name an ARN names, or `None` for anything that is not a lookup
/// table's ARN of this service — which is how every operation that identifies a table by
/// `lookupTableArn` reaches the record the store holds under its name.
pub fn lookup_table_name_from_arn(arn: &str) -> Option<&str> {
    let (service, resource) = arn_service_and_resource(arn)?;
    if service != "logs" {
        return None;
    }
    resource
        .strip_prefix(LOOKUP_TABLE_ARN_PREFIX)
        .filter(|name| !name.is_empty())
}

/// The refusal CloudWatch Logs answers when the lookup table an operation names is not one
/// the account holds. Unlike the generic [`resource_not_found_message`] sentence, a
/// lookup-table refusal states WHICH table and WHICH account was looked in, so a caller
/// whose identifier crossed accounts or Regions can see what was searched for and where.
///
/// Both variable parts come from the request and from state: `name` is the table the request
/// identified and `account_id` the account that identifier names.
pub fn lookup_table_not_found_message(name: &str, account_id: &str) -> String {
    format!("Lookup table '{name}' not found in account '{account_id}'")
}

/// The refusal a lookup-table CREATE answers a name the account already holds a table under
/// with. The sentence is the mirror of [`lookup_table_not_found_message`] — the same two
/// variable parts, the table the request named and the account it was looked for in — because
/// both state what was searched for and where.
pub fn lookup_table_already_exists_message(name: &str, account_id: &str) -> String {
    format!("Lookup table '{name}' already exists in account '{account_id}'")
}

/// The refusal a prefix-filtered lookup-table enumeration answers when the account holds no
/// table whose name starts with that prefix: the request names a set of tables, so a prefix
/// matching none names no resource at all.
///
/// Both variable parts are the request's and the caller's: the account the tables were looked
/// for in and the prefix they were looked for under.
pub fn lookup_tables_not_found_message(account_id: &str, prefix: &str) -> String {
    format!("No lookup tables found for accountId '{account_id}' with prefix: '{prefix}'")
}

/// The same refusal for an operation that identifies the table by its `lookupTableArn`: that
/// ARN carries both the table's name and the account the table would belong to.
///
/// An ARN that carries no lookup-table name identifies no table to name in the refusal — the
/// case [`UNIDENTIFIED_RESOURCE_NOT_FOUND_MESSAGE`] distinguishes elsewhere in this service —
/// so it is refused with the generic [`resource_not_found_message`] sentence rather than with
/// an empty name.
pub fn lookup_table_not_found_message_for_arn(arn: &str) -> String {
    match (lookup_table_name_from_arn(arn), arn_account_id(arn)) {
        (Some(name), Some(account_id)) => lookup_table_not_found_message(name, account_id),
        _ => resource_not_found_message("lookup table"),
    }
}

/// What populates a lookup table: "You must specify either tableBody or queryId, but not
/// both."
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupTableSource {
    /// The CSV content the request carried, verbatim.
    TableBody(String),
    /// The id of the completed or cancelled query whose results populate the table.
    QueryId(String),
}

/// The refusal for a request that names BOTH of a lookup table's content sources. Like the
/// log-group selector pair ([`LOG_GROUP_SELECTORS_EXCLUSIVE`]) this is checked ACROSS two
/// members, which no single member's constraint can express, so it is answered with the
/// `ValidationException` the lookup-table operations model rather than with their
/// `InvalidParameterException`.
pub static LOOKUP_TABLE_SOURCE_EXCLUSIVE: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        super::parameter_pairs::mutually_exclusive("tableBody", "queryId")
    });

/// The refusal for a lookup-table write that names neither content source: a table is
/// created "by uploading CSV data or from CloudWatch Logs query results" and updated by
/// "replacing all of its content with new CSV data or CloudWatch Logs query results", so a
/// create or an update naming neither names nothing to fill the table from. See
/// [`LOOKUP_TABLE_SOURCE_EXCLUSIVE`] for the error shape.
pub static LOOKUP_TABLE_SOURCE_REQUIRED: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| super::parameter_pairs::either_required("tableBody", "queryId"));

/// Resolve what a lookup-table request populates the table from. `Ok(None)` is a request
/// that names NEITHER source, which neither a create nor an update may be: both state "You
/// must specify either tableBody or queryId, but not both", and an update is itself a
/// content replacement ("Updates an existing lookup table by replacing all of its content
/// with new CSV data or CloudWatch Logs query results"). The `Err` carries the message to
/// refuse a request naming BOTH with; a caller refuses `Ok(None)` with
/// [`LOOKUP_TABLE_SOURCE_REQUIRED`].
pub fn resolve_lookup_table_source(
    table_body: Option<&str>,
    query_id: Option<&str>,
) -> Result<Option<LookupTableSource>, String> {
    match (table_body, query_id) {
        (Some(body), None) => Ok(Some(LookupTableSource::TableBody(body.to_string()))),
        (None, Some(query_id)) => Ok(Some(LookupTableSource::QueryId(query_id.to_string()))),
        (Some(_), Some(_)) => Err(LOOKUP_TABLE_SOURCE_EXCLUSIVE.clone()),
        (None, None) => Ok(None),
    }
}

/// The refusal for a `queryId` that names no query whose results could populate a lookup
/// table — "The ID of a completed or cancelled CloudWatch Logs query". The message is built
/// from the request so a caller can attribute it to the id it sent.
///
/// `StartQuery` is still a scaffold stub, so this account holds no query at all and EVERY
/// `queryId` names one that is not there. Implementing that operation is what gives this
/// service a query store to resolve an id against.
pub fn query_not_found_message(query_id: &str) -> String {
    format!("Query with id [{query_id}] is not a completed or cancelled query.")
}

/// The refusal `StopQuery` answers for a query that has already ended: it "stops a CloudWatch
/// Logs Insights query that is in progress", and "if the query has already ended, the
/// operation returns an error indicating that the specified query is not running". The
/// message is built from the request, in the same `Query with id [..]` form every refusal of
/// this service that names a query by its id takes, so a caller stopping queries in a loop
/// can attribute the refusal to one id.
pub fn query_not_running_message(query_id: &str) -> String {
    format!("Query with id [{query_id}] is not running.")
}

/// What a lookup table's CSV body describes: the column names of its header row and how
/// many data rows follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableBodyShape {
    /// `LookupTable$tableFields` — "The column headers from the first row of the CSV file."
    pub fields: Vec<String>,
    /// `LookupTable$recordsCount` — "The number of data rows in the lookup table, excluding
    /// the header row."
    pub records_count: i64,
}

/// The refusal for a CSV header row that names the same column name more than once: such a
/// row names no column SET, so a data row's values name no column unambiguously and the
/// table has no columns to be read by. The text is fixed for the error class — it
/// interpolates neither the repeated name, nor the member, nor a position.
pub const DUPLICATE_TABLE_FIELDS_MESSAGE: &str = "CSV header contains duplicate field names";

/// Why a lookup table's CSV body builds no table, and therefore which modelled error the
/// operation that was handed it refuses the request with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableBodyRejection {
    /// Content this service could not read as the CSV the `tableBody` member documents: the
    /// body names the defect and the member it was carried in, so it is refused with the
    /// `InvalidParameterException` every lookup-table write models for a member it will not
    /// accept.
    Unreadable(String),
    /// A body read as CSV whose header row declares one column name twice. The rule is about
    /// the header row as a WHOLE rather than about a value a single member's constraint could
    /// express, so — like the cross-member source rules of the same operations — it is
    /// refused with the `ValidationException` they model, carrying
    /// [`DUPLICATE_TABLE_FIELDS_MESSAGE`].
    DuplicateFields,
}

/// Read a lookup table's CSV body: "the CSV content must include a header row with column
/// names". The body is parsed by the `csv` crate rather than split by hand, so a quoted
/// field carrying a comma, a newline or an escaped quote is one field, as CSV defines it.
///
/// The `Err` says why the body builds no table: a header row naming no column at all, a data
/// row whose field count disagrees with the header's, and a header row naming the same column
/// twice are each content a table cannot be read by — the last one through its own
/// [`TableBodyRejection::DuplicateFields`], because a repeated column name is refused as a
/// validation failure rather than as an unreadable member.
pub fn parse_table_body(body: &str) -> Result<TableBodyShape, TableBodyRejection> {
    let unreadable = |error: csv::Error| TableBodyRejection::Unreadable(table_body_message(&error));
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        // A row must carry exactly the header's fields: a lookup table's rows are keyed by
        // its column names, so a short or long row names no value for some column.
        .flexible(false)
        .from_reader(body.as_bytes());
    let fields: Vec<String> = reader
        .headers()
        .map_err(unreadable)?
        .iter()
        .map(str::to_string)
        .collect();
    if fields.iter().all(|field| field.trim().is_empty()) {
        return Err(TableBodyRejection::Unreadable(
            "tableBody must begin with a header row naming the table's columns".to_string(),
        ));
    }
    // "The first row must be a header row with column names": the names it declares are the
    // columns the table is read by ([`LookupTableRecord::table_fields`]), so a name declared
    // twice leaves every value under it ambiguous and the body names no table. Compared
    // through a set, so a wide header costs one pass and not one per column.
    let mut seen = std::collections::BTreeSet::new();
    if !fields.iter().all(|field| seen.insert(field.as_str())) {
        return Err(TableBodyRejection::DuplicateFields);
    }
    let mut records_count: i64 = 0;
    for record in reader.records() {
        record.map_err(unreadable)?;
        records_count = records_count.saturating_add(1);
    }
    Ok(TableBodyShape {
        fields,
        records_count,
    })
}

/// The refusal for a CSV body the parser would not read, as the defect the parser reported:
/// a row disagreeing with the header names both counts and the row it is at, so a caller
/// can find the row in the body it sent.
pub(super) fn table_body_message(error: &csv::Error) -> String {
    match error.kind() {
        csv::ErrorKind::UnequalLengths {
            pos,
            expected_len,
            len,
        } => format!(
            "tableBody row {} has {len} fields but the header row names {expected_len}",
            pos.as_ref().map_or(0, csv::Position::line)
        ),
        _ => format!("tableBody is not valid CSV content: {error}"),
    }
}

/// One lookup table — the reference data "you can use [...] to enrich log data in
/// CloudWatch Logs queries" — as it is stored. Every member of the model's `LookupTable`
/// shape is derived from this record, so `GetLookupTable` and `DescribeLookupTables` answer
/// from state alone.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct LookupTableRecord {
    /// `LookupTable$lookupTableName` — "The name of the lookup table". Also the key the
    /// store is keyed by.
    pub name: String,
    /// `LookupTable$lookupTableArn` — the ARN [`lookup_table_arn`] built for this table in
    /// the region the create was signed for.
    pub arn: String,
    /// `LookupTable$description` — "The description of the lookup table", when a call named
    /// one.
    pub description: Option<String>,
    /// The CSV content this table holds, VERBATIM as the call that last wrote it sent it: a
    /// cell's text is the table's data, so nothing re-renders it and no numeric value is
    /// re-formatted on the way in or out. `GetLookupTable$tableBody` reports it.
    pub table_body: String,
    /// `LookupTable$tableFields` — the column names of [`Self::table_body`]'s header row, as
    /// [`parse_table_body`] read them.
    pub table_fields: Vec<String>,
    /// `LookupTable$recordsCount` — the data rows of [`Self::table_body`], excluding its
    /// header row.
    pub records_count: i64,
    /// `LookupTable$kmsKeyId` — "The ARN of the KMS key used to encrypt the lookup table
    /// data, if applicable". `None` when no call named one, which is the Amazon-owned key
    /// `CreateLookupTable` documents as the default ("If you don't specify a key, the data is
    /// encrypted with an Amazon Web Services-owned key") and not a key this emulator invents.
    pub kms_key_id: Option<String>,
    /// `CreateLookupTable$createdAt` — "The time when the lookup table was created", in epoch
    /// milliseconds.
    pub created_at_ms: i64,
    /// `LookupTable$lastUpdatedTime` — "The time when the lookup table was last updated", in
    /// epoch milliseconds. A table nothing has updated since it was created reports the
    /// instant it was created.
    pub last_updated_time_ms: i64,
    /// The tags `CreateLookupTable$tags` assigned to this table. A `BTreeMap` so the stored
    /// order is stable; no `LookupTable` member reports them, so they are read back through
    /// the tagging operations once those reach this resource.
    pub tags: BTreeMap<String, String>,
}
