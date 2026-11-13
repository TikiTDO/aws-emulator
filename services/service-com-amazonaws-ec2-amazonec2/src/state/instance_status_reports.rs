// SPDX-License-Identifier: CC0-1.0

//! The instance status FEEDBACK this service collects, and what bounds it.
//!
//! `ReportInstanceStatus` "submits feedback about the status of an instance": the caller
//! says whether its own experience of the instance was `ok` or `impaired`, why
//! (`ReasonCodes`), over which window (`StartTime`/`EndTime`) and in what words
//! (`Description`). "Amazon EC2 collects this information to improve the accuracy of
//! status checks. Use of this action does not change the value returned by
//! DescribeInstanceStatus" — so a report is STORED, keyed by the instance it is about, and
//! nothing projects it into a describe: the submission is a mutation of this store and of
//! nothing else.
//!
//! Each report is kept as submitted rather than merged into the one before it, because the
//! feedback EC2 collects is the sequence of what callers reported, and a second report
//! about the same instance is a second observation and not a correction of the first.
//!
//! Nothing deletes a report, so both of this crate's ceilings apply to the store: the
//! number of records it holds ([`super::MAX_STORED_RECORDS_PER_STORE`]) and the
//! caller-chosen bytes those records carry ([`super::MAX_STORED_BYTES`], asked about
//! [`instance_status_report_payload_bytes`]) — a count being no bound on the bytes one
//! record carries, since the model publishes no length for `Description`.

use std::time::SystemTime;

use super::{ComAmazonawsEc2Amazonec2State, MAX_STORED_RECORDS_PER_STORE};

/// One status report a caller submitted about one instance.
///
/// Not a modelled shape: `ReportInstanceStatus` answers `Unit` and no operation reports a
/// submitted report back, so there is no describe projection for these members to
/// round-trip through. They are held as the request gave them — the status word and the
/// reason codes as the modelled catalogs spell them, and the window as wall-clock instants
/// so a stored report can be compared with the clock without assuming anything about the
/// modelled timestamp type's own ordering.
///
/// [`Self::description`] is `@sensitive` in the model, so it is held in this process's
/// memory and nowhere else: no operation puts it in a response, nothing writes it to disk,
/// and no refusal of this operation quotes it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct InstanceStatusReport {
    /// "The status of all instances listed" (`ReportInstanceStatusRequest$Status`), one of
    /// the modelled `ReportStatusType` catalog.
    pub status: String,
    /// "The reason codes that describe the health state of your instance"
    /// (`ReportInstanceStatusRequest$ReasonCodes`), each one of the modelled
    /// `ReportInstanceReasonCodes` catalog.
    pub reason_codes: Vec<String>,
    /// "The time at which the reported instance health state began"
    /// (`ReportInstanceStatusRequest$StartTime`), when the request named one.
    pub start_time: Option<SystemTime>,
    /// "The time at which the reported instance health state ended"
    /// (`ReportInstanceStatusRequest$EndTime`), when the request named one.
    pub end_time: Option<SystemTime>,
    /// "Descriptive text about the health state of your instance"
    /// (`ReportInstanceStatusRequest$Description`), when the request carried any.
    pub description: Option<String>,
    /// When this report was submitted — the service's own clock reading, not a value the
    /// request carries, so the collected feedback says when it was collected.
    pub submitted_at: SystemTime,
}

/// The caller-chosen bytes one report holds, for the store's byte budget: the instance id
/// it is filed under, the status word, the reason codes and the description. The submitted
/// instant is the service's own and is not caller-chosen, so it is not charged.
pub fn instance_status_report_payload_bytes(
    instance_id: &str,
    report: &InstanceStatusReport,
) -> usize {
    instance_id.len()
        + report.status.len()
        + report
            .reason_codes
            .iter()
            .map(String::len)
            .sum::<usize>()
        + report.description.as_deref().map_or(0, str::len)
}

impl ComAmazonawsEc2Amazonec2State {
    /// How many status reports this store holds, across every instance.
    pub fn instance_status_reports_held(&self) -> usize {
        self.instance_status_reports.values().map(Vec::len).sum()
    }

    /// Whether `more` further reports fit inside the store's record ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]). A submission asks first and refuses with EC2's
    /// `ResourceLimitExceeded` when this answers `false`, nothing here ever reclaiming a
    /// stored report.
    pub fn has_instance_status_report_capacity(&self, more: usize) -> bool {
        self.instance_status_reports_held().saturating_add(more) <= MAX_STORED_RECORDS_PER_STORE
    }

    /// Collect one report about `instance_id`, charging what it holds against the store's
    /// byte budget. The ONE writer of this store, so every report is charged exactly once.
    pub fn collect_instance_status_report(
        &mut self,
        instance_id: &str,
        report: InstanceStatusReport,
    ) {
        self.add_stored_bytes(instance_status_report_payload_bytes(instance_id, &report));
        self.instance_status_reports
            .entry(instance_id.to_string())
            .or_default()
            .push(report);
    }

    /// The reports collected about `instance_id`, in submission order — empty for an
    /// instance nobody has reported on.
    pub fn instance_status_reports_for(&self, instance_id: &str) -> &[InstanceStatusReport] {
        self.instance_status_reports
            .get(instance_id)
            .map_or(&[], Vec::as_slice)
    }
}
