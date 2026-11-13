// SPDX-License-Identifier: CC0-1.0

//! Declarative-policies account status reports: what one is stored as, the words and forms
//! its members take, and the one move the clock owes one.
//!
//! "Generates an account status report" (`StartDeclarativePoliciesReport`): a report is a
//! RESOURCE of the account, held in
//! [`ComAmazonawsEc2Amazonec2State::declarative_policies_reports`] as the modelled
//! [`DeclarativePoliciesReport`] shape `DescribeDeclarativePoliciesReports` reports, beside
//! the one fact that shape carries no member for — the instant its generation falls
//! complete.
//!
//! A report AWS hands back is still being generated — the `running` member of the modelled
//! `ReportState` enum — so a started report is stored [`REPORT_STATE_RUNNING`] and the clock
//! owes it [`REPORT_STATE_COMPLETE`] one [`TRANSITION_DELAY`] later, the span every other
//! non-terminal state of this store moves on. The due instant sits on the record rather than
//! being slept for, and the status a reader sees is DERIVED from it
//! ([`DeclarativePoliciesReportRecord::view_at`]), so reporting a report never writes and a
//! caller reading straight after the start still sees it `running`.
//!
//! A SUBMODULE of [`crate::state`], for the same reason [`super::image_usage_reports`] is
//! one: this surface grows as the sibling report operations are implemented, and it grows
//! here rather than in the parts of `state.rs` the other resource kinds own. `pub use` keeps
//! every name reachable as `crate::state::<name>`, which is where each handler names it.

use std::time::{Instant, SystemTime};

use crate::ops::types::{DeclarativePoliciesReport, Tag};
use crate::scaffolded::Scaffolded;

use super::{
    ComAmazonawsEc2Amazonec2State, TRANSITION_DELAY, echoed_resource_id,
    system_time_to_date_time, tag_list_bytes,
};

/// "`running`" — the report is being generated: the member of the modelled `ReportState`
/// enum (`smithy_query(query="shape",shape="ReportState")`) a started report sits in.
pub const REPORT_STATE_RUNNING: &str = "running";

/// "`complete`" — the generation finished: the state the clock owes a `running` report one
/// [`TRANSITION_DELAY`] after the call that started it.
pub const REPORT_STATE_COMPLETE: &str = "complete";

/// "`cancelled`" — the state `CancelDeclarativePoliciesReport` puts a running report in.
pub const REPORT_STATE_CANCELLED: &str = "cancelled";

/// The `ResourceType` a `TagSpecification` of a report start may name: the
/// `declarative-policies-report` member of the modelled `ResourceType` enum
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const DECLARATIVE_POLICIES_REPORT_TAGGABLE_RESOURCE: &str = "declarative-policies-report";

/// The id prefix a declarative-policies report's id carries, in the `<prefix>-<hex>` form
/// every EC2 resource id takes ([`ComAmazonawsEc2Amazonec2State::next_id`]).
///
/// The model publishes no pattern for `DeclarativePoliciesReportId`
/// (`smithy_query(query="shape",shape="DeclarativePoliciesReportId")`), so the prefix is the
/// one AWS's own examples of that member read in (`report-…`), kept here as the one place it
/// is spelled.
pub const DECLARATIVE_POLICIES_REPORT_ID_PREFIX: &str = "report";

/// How many reports one run of this emulator holds.
///
/// A count ceiling, the way every other store of this crate has one: nothing reclaims a
/// report, so without it a caller could grow the store without bound.
pub const MAX_STORED_DECLARATIVE_POLICIES_REPORTS: usize = 1_000;

/// `InvalidDeclarativePoliciesReportId.Malformed` (HTTP 400): the request named something
/// that is not a report id at all, so there is nothing to look up.
///
/// In the `Invalid<Resource>Id.Malformed` family this service refuses an unissuable id of any
/// other kind with ([`super::INVALID_CAPACITY_RESERVATION_ID_MALFORMED`]).
pub const INVALID_DECLARATIVE_POLICIES_REPORT_ID_MALFORMED: &str =
    "InvalidDeclarativePoliciesReportId.Malformed";

/// `InvalidDeclarativePoliciesReportId.NotFound` (HTTP 400): a report id inside the form
/// above that this organization started no report under — the other half of that pair, which
/// the recorded contract of `GetDeclarativePoliciesReportSummary` answers for such an id.
pub const INVALID_DECLARATIVE_POLICIES_REPORT_ID_NOT_FOUND: &str =
    "InvalidDeclarativePoliciesReportId.NotFound";

/// How this service names the resource in the refusal the recording settles: "report", which
/// is how the recorded `InvalidDeclarativePoliciesReportId.NotFound` sentence words it.
const REPORT_RESOURCE_NAME: &str = "report";

/// Whether `report_id` READS as a report id: the `report-` prefix every id this service mints
/// for one carries ([`DECLARATIVE_POLICIES_REPORT_ID_PREFIX`]), followed by something.
///
/// The model publishes no pattern for `DeclarativePoliciesReportId`
/// (`smithy_query(query="shape",shape="DeclarativePoliciesReportId")`), so the PREFIX is all
/// there is to read: a value that does not carry it is not a report id at all.
///
/// WHICH half of the refusal family an operation answers for an id it cannot resolve is the
/// OPERATION's, not this function's. The operation that ACTS on one named report
/// (`CancelDeclarativePoliciesReport`, through
/// [`require_declarative_policies_report`]) refuses a value that is no report id
/// [`INVALID_DECLARATIVE_POLICIES_REPORT_ID_MALFORMED`] before any lookup, and one inside the
/// form this organization started no report under
/// [`INVALID_DECLARATIVE_POLICIES_REPORT_ID_NOT_FOUND`] — the two halves its recorded contract
/// answers. The operations that REPORT reports (`GetDeclarativePoliciesReportSummary` and
/// `DescribeDeclarativePoliciesReports`) answer the NOT-FOUND half for every id they cannot
/// resolve, whatever its form, which is what their own recorded contracts answer — so a reader
/// of this surface does not reach for this function at all.
pub fn declarative_policies_report_id_is_well_formed(report_id: &str) -> bool {
    report_id
        .strip_prefix(DECLARATIVE_POLICIES_REPORT_ID_PREFIX)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|suffix| !suffix.is_empty())
}

/// Refuse a value that is not a report id at all: HTTP 400
/// [`INVALID_DECLARATIVE_POLICIES_REPORT_ID_MALFORMED`], naming the resource as EC2's own
/// `ResourceType` enum spells it ([`DECLARATIVE_POLICIES_REPORT_TAGGABLE_RESOURCE`]) and the
/// id the REQUEST sent.
///
/// The resource word differs from the not-found half's below because only that half's
/// sentence is recorded: this one keeps the wording it was written with rather than inferring
/// AWS's from the other half.
pub fn invalid_declarative_policies_report_id_malformed<E>(report_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_DECLARATIVE_POLICIES_REPORT_ID_MALFORMED,
        400,
        format!(
            "The {resource} ID '{report_id}' is malformed",
            resource = DECLARATIVE_POLICIES_REPORT_TAGGABLE_RESOURCE,
            report_id = echoed_resource_id(report_id)
        ),
    )
}

/// Refuse a report id this organization started no report under: HTTP 400
/// [`INVALID_DECLARATIVE_POLICIES_REPORT_ID_NOT_FOUND`], in the sentence the recorded refusal
/// carries — "The report ID '<id>' does not exist.", with the id the REQUEST sent.
pub fn invalid_declarative_policies_report_id_not_found<E>(report_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_DECLARATIVE_POLICIES_REPORT_ID_NOT_FOUND,
        400,
        format!(
            "The {REPORT_RESOURCE_NAME} ID '{report_id}' does not exist.",
            report_id = echoed_resource_id(report_id)
        ),
    )
}

/// The report `report_id` names, or the refusal for an id this service cannot resolve: a
/// malformed one before an unknown one, the way the service orders the two.
pub fn require_declarative_policies_report<'store, E>(
    store: &'store ComAmazonawsEc2Amazonec2State,
    report_id: &str,
) -> Result<&'store DeclarativePoliciesReportRecord, Scaffolded<E>> {
    if !declarative_policies_report_id_is_well_formed(report_id) {
        return Err(invalid_declarative_policies_report_id_malformed(report_id));
    }
    store
        .declarative_policies_reports
        .get(report_id)
        .ok_or_else(|| invalid_declarative_policies_report_id_not_found(report_id))
}

/// What one report is started from: the members the request chooses, every documented default
/// already filled in by the operation that took them.
#[derive(Debug, Clone, Default)]
pub struct DeclarativePoliciesReportSpec {
    /// "The name of the Amazon S3 bucket where the report will be saved"
    /// (`StartDeclarativePoliciesReportRequest$S3Bucket`).
    pub s3_bucket: String,
    /// "The prefix for your S3 object" (`$S3Prefix`); `None` for a request that names none.
    pub s3_prefix: Option<String>,
    /// "The root ID, organizational unit ID, or account ID" the report is generated for
    /// (`$TargetId`).
    pub target_id: String,
    /// "The tags to apply" (`$TagSpecifications`).
    pub tags: Vec<Tag>,
}

/// One stored report: the modelled shape every reader of it answers from, beside the instant
/// the clock owes it its completion.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DeclarativePoliciesReportRecord {
    /// The report as `DescribeDeclarativePoliciesReports` reports it. Its `Status` holds the
    /// word the last WRITER left — a move the clock owes is applied by
    /// [`Self::view_at`] rather than stored, so a read never writes.
    pub report: DeclarativePoliciesReport,
    /// When this report's `running` -> `complete` move falls due, and `None` once it has
    /// reached a terminal state (a cancelled report is owed nothing).
    #[serde(with = "::service_common::persist::instant_opt")]
    pub completes_at: Option<Instant>,
    /// The wall-clock instant the completion this record is owed happened at, which is what
    /// `EndTime` reports once it is due.
    pub ends_at: Option<SystemTime>,
}

impl DeclarativePoliciesReportRecord {
    /// The `Status` this report carries as of `now`: [`REPORT_STATE_COMPLETE`] once the move
    /// the clock owed it has fallen due, and the stored word otherwise.
    pub fn status_at(&self, now: Instant) -> &str {
        let stored = self.report.status.as_deref().unwrap_or(REPORT_STATE_RUNNING);
        match self.completes_at {
            Some(due) if now >= due => REPORT_STATE_COMPLETE,
            _ => stored,
        }
    }

    /// This report as of `now`: the stored shape with the move the clock owes it applied.
    ///
    /// "The time when the report generation ended" (`DeclarativePoliciesReport$EndTime`) is
    /// reported only once the generation HAS ended, so a report still running carries none.
    pub fn view_at(&self, now: Instant) -> DeclarativePoliciesReport {
        let mut view = self.report.clone();
        if self.status_at(now) == REPORT_STATE_COMPLETE
            && view.status.as_deref() != Some(REPORT_STATE_COMPLETE)
        {
            view.status = Some(REPORT_STATE_COMPLETE.to_string());
            view.end_time = self.ends_at.map(system_time_to_date_time);
        }
        view
    }
}

/// The caller-chosen bytes one stored report holds: its bucket, its prefix, the target it
/// names and its tags. Everything else on the record — the id, the instants, the status — is
/// derived by this module, so it is fixed-size.
///
/// The ONE definition of that charge: the start asks it against the store's budget before the
/// write and charges exactly it.
pub fn declarative_policies_report_payload_bytes(spec: &DeclarativePoliciesReportSpec) -> usize {
    spec.s3_bucket
        .len()
        .saturating_add(spec.s3_prefix.as_deref().map_or(0, str::len))
        .saturating_add(spec.target_id.len())
        .saturating_add(tag_list_bytes(&spec.tags))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another report fits inside the store's ceiling
    /// ([`MAX_STORED_DECLARATIVE_POLICIES_REPORTS`]).
    pub fn has_declarative_policies_report_capacity(&self) -> bool {
        self.declarative_policies_reports.len() < MAX_STORED_DECLARATIVE_POLICIES_REPORTS
    }

    /// Store one report started at `started_at`, and answer the `ReportId` it was minted
    /// with.
    ///
    /// The report is stored [`REPORT_STATE_RUNNING`] with [`REPORT_STATE_COMPLETE`] owed to
    /// the clock one [`TRANSITION_DELAY`] later (see the module docs), and its start instant
    /// is read off `started_at`. The bytes the record retains are NOT charged here: the start
    /// asks [`declarative_policies_report_payload_bytes`] against the store's budget before
    /// it calls this, the way every other create of this crate does.
    pub fn insert_declarative_policies_report(
        &mut self,
        spec: DeclarativePoliciesReportSpec,
        started_at: SystemTime,
    ) -> String {
        let report_id = self.next_id(DECLARATIVE_POLICIES_REPORT_ID_PREFIX);
        let report = DeclarativePoliciesReport {
            report_id: Some(report_id.clone()),
            s3_bucket: Some(spec.s3_bucket),
            s3_prefix: spec.s3_prefix,
            target_id: Some(spec.target_id),
            start_time: Some(system_time_to_date_time(started_at)),
            // "The time when the report generation ended": a generation that is still
            // running has not ended, so the member is reported only once it has.
            end_time: None,
            status: Some(REPORT_STATE_RUNNING.to_string()),
            tags: Some(spec.tags),
        };
        let record = DeclarativePoliciesReportRecord {
            report,
            completes_at: Some(Instant::now() + TRANSITION_DELAY),
            ends_at: started_at.checked_add(TRANSITION_DELAY),
        };
        self.declarative_policies_reports
            .insert(report_id.clone(), record);
        report_id
    }

    /// Put the report `report_id` into [`REPORT_STATE_CANCELLED`] as of `now`, and answer
    /// whether it was still running.
    ///
    /// The ONE writer of that transition: "cancels the generation of an account status
    /// report" (`CancelDeclarativePoliciesReport`), so the generation stops at `cancelled_at`
    /// — which is when it ENDED — and the completion the clock owed it is dropped, since a
    /// cancelled generation never completes. A report that has already reached a terminal
    /// state is left exactly where it was and `false` is answered, so the caller is told it
    /// cancelled nothing.
    pub fn cancel_declarative_policies_report(
        &mut self,
        report_id: &str,
        now: Instant,
        cancelled_at: SystemTime,
    ) -> bool {
        let Some(record) = self.declarative_policies_reports.get_mut(report_id) else {
            return false;
        };
        if record.status_at(now) != REPORT_STATE_RUNNING {
            return false;
        }
        record.report.status = Some(REPORT_STATE_CANCELLED.to_string());
        record.report.end_time = Some(system_time_to_date_time(cancelled_at));
        record.completes_at = None;
        record.ends_at = Some(cancelled_at);
        true
    }
}
