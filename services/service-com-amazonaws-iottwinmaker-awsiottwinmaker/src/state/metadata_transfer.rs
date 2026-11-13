// SPDX-License-Identifier: CC0-1.0

//! What a metadata transfer job RUNS, and what the run leaves behind.
//!
//! A metadata transfer job is the service's bulk import/export path: it READS
//! the metadata its source names and WRITES what that metadata describes into
//! its destination, then reports the run — the state it ended in, how many
//! entries it moved, and where its report was written. Nothing here promises a
//! transfer: every count this reports is a count of entries that really landed,
//! and the component types and entities an import reports as succeeded are
//! records the destination workspace holds afterwards.
//!
//! The pieces:
//!
//! * [`document`] — the metadata document a transfer reads and writes, with one
//!   reader and one writer over it, so an export and an import of the same
//!   workspace describe the same thing.
//! * [`workspace_writer`] — landing a document's entries in a workspace, each
//!   entry held to the same rules `CreateComponentType` and `CreateEntity` hold
//!   a caller's own create to.
//! * [`transfer_run`] — the run itself: reading the source, writing the
//!   destination and writing the run's report, which is the one part that
//!   reaches another service (the emulator's own Amazon S3, over the in-process
//!   callback).
//!
//! WHEN the run happens: in the call that accepts the job, which is the one
//! place with a request context to reach the object store through. The job is
//! accepted in the non-terminal `VALIDATING` state and the OUTCOME of the run
//! is what the clock owes it ([`METADATA_TRANSFER_JOB_SETTLE_SPAN`]): the
//! instant is stored on the record and a reader materialises the transition
//! rather than sleeping for it, so a caller that reads the job back in the same
//! breath as the create still sees it in flight, and one that polls sees it
//! settle in the state its run earned.

use super::*;

mod document;
mod transfer_run;
mod workspace_writer;
pub use document::*;
pub use transfer_run::*;
pub use workspace_writer::*;

/// What one entry of a transferred document came to.
///
/// The three outcomes `MetadataTransferJobProgress` counts beside the total:
/// the entry was written (`succeededCount`), it was left alone because the
/// destination already held it (`skippedCount`), or it could not be written
/// (`failedCount`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryOutcome {
    Succeeded,
    Skipped,
    Failed,
}

impl EntryOutcome {
    /// How the run's report names this outcome.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

/// The kind of resource one transferred entry describes, as the run's report
/// names it.
pub const ENTRY_KIND_COMPONENT_TYPE: &str = "componentType";
/// See [`ENTRY_KIND_COMPONENT_TYPE`].
pub const ENTRY_KIND_ENTITY: &str = "entity";

/// One line of a run's report: what the entry was, what became of it, and why.
#[derive(Debug, Clone)]
pub struct TransferredEntry {
    /// [`ENTRY_KIND_COMPONENT_TYPE`] or [`ENTRY_KIND_ENTITY`].
    pub kind: &'static str,
    /// The id the entry named.
    pub id: String,
    pub outcome: EntryOutcome,
    /// Why the entry was skipped or could not be written, and `None` for one
    /// that was written: a success has nothing to explain.
    pub reason: Option<String>,
}

/// What a run did, entry by entry.
///
/// Built as the run goes, so the counts it reports are a MEASUREMENT of the run
/// rather than a restatement of what the document asked for: an entry counts as
/// written only once the destination holds it.
#[derive(Debug, Clone, Default)]
pub struct TransferReport {
    pub entries: Vec<TransferredEntry>,
}

impl TransferReport {
    /// Note that the entry `id` of kind `kind` was written.
    pub fn succeeded(&mut self, kind: &'static str, id: &str) {
        self.note(kind, id, EntryOutcome::Succeeded, None);
    }

    /// Note that the entry was left alone, and why.
    pub fn skipped(&mut self, kind: &'static str, id: &str, reason: String) {
        self.note(kind, id, EntryOutcome::Skipped, Some(reason));
    }

    /// Note that the entry could not be written, and why.
    pub fn failed(&mut self, kind: &'static str, id: &str, reason: String) {
        self.note(kind, id, EntryOutcome::Failed, Some(reason));
    }

    fn note(
        &mut self,
        kind: &'static str,
        id: &str,
        outcome: EntryOutcome,
        reason: Option<String>,
    ) {
        self.entries.push(TransferredEntry {
            kind,
            id: id.to_string(),
            outcome,
            // The reason is clamped to the bound the modelled `ErrorMessage`
            // carries, because the same sentence is what a failed run reports
            // as its `error.message`.
            reason: reason.map(|reason| clamped_message(reason, ERROR_MESSAGE_MAX_CHARS)),
        });
    }

    /// How many entries this run came to each outcome.
    pub fn count_of(&self, outcome: EntryOutcome) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.outcome == outcome)
            .count()
    }

    /// The modelled `MetadataTransferJobProgress` of this run: the entries it
    /// walked and what became of each.
    ///
    /// Every count is reported, including the zeroes: a run that transferred an
    /// empty document walked no entry, and `totalCount: 0` is what says so.
    pub fn progress(&self) -> crate::ops::types::MetadataTransferJobProgress {
        crate::ops::types::MetadataTransferJobProgress {
            total_count: Some(count(self.entries.len())),
            succeeded_count: Some(count(self.count_of(EntryOutcome::Succeeded))),
            skipped_count: Some(count(self.count_of(EntryOutcome::Skipped))),
            failed_count: Some(count(self.count_of(EntryOutcome::Failed))),
        }
    }

    /// The first reason an entry could not be written, for a run whose failures
    /// are what make it fail.
    pub fn first_failure(&self) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.outcome == EntryOutcome::Failed)
            .and_then(|entry| entry.reason.as_deref())
    }

    /// This run as the report document that is written for a caller to fetch:
    /// the job it belongs to, the state it ended in, its counts, and one line
    /// per entry.
    pub fn document(
        &self,
        metadata_transfer_job_id: &str,
        state: &str,
    ) -> ::serde_json::Value {
        let progress = self.progress();
        let entries: Vec<::serde_json::Value> = self
            .entries
            .iter()
            .map(|entry| {
                let mut written = ::serde_json::Map::new();
                written.insert("type".to_string(), ::serde_json::Value::from(entry.kind));
                written.insert(
                    "id".to_string(),
                    ::serde_json::Value::from(entry.id.as_str()),
                );
                written.insert(
                    "outcome".to_string(),
                    ::serde_json::Value::from(entry.outcome.as_str()),
                );
                if let Some(reason) = &entry.reason {
                    written.insert(
                        "reason".to_string(),
                        ::serde_json::Value::from(reason.as_str()),
                    );
                }
                ::serde_json::Value::Object(written)
            })
            .collect();
        ::serde_json::json!({
            "metadataTransferJobId": metadata_transfer_job_id,
            "state": state,
            "progress": {
                "totalCount": progress.total_count,
                "succeededCount": progress.succeeded_count,
                "skippedCount": progress.skipped_count,
                "failedCount": progress.failed_count,
            },
            "entries": entries,
        })
    }
}

/// A count of entries as the modelled `Integer` the progress reports it in.
///
/// The counts are bounded by what one document may describe
/// ([`MAX_ENTITIES_PER_WORKSPACE`] plus
/// [`MAX_COMPONENT_TYPES_PER_WORKSPACE`]), so the conversion cannot be lossy;
/// the saturating fallback keeps that true if those ceilings ever move.
fn count(entries: usize) -> i32 {
    i32::try_from(entries).unwrap_or(i32::MAX)
}

/// What a job's run established: the terminal state the clock owes the job, and
/// what the run has to report with it.
///
/// Carried on the stored job ([`MetadataTransferJob::transfer`]) because every
/// part of it is a measurement of ONE run: the job reports it, the destination
/// took the data, and nothing recomputes either per read.
#[derive(Debug, Clone)]
pub struct TransferOutcome {
    /// The `MetadataTransferJobState` the job settles in:
    /// [`METADATA_TRANSFER_JOB_STATE_COMPLETED`] for a run that read its source
    /// and wrote its destination, [`METADATA_TRANSFER_JOB_STATE_ERROR`] for one
    /// that could not.
    pub state: String,
    /// Why the run failed, as the modelled `ErrorDetails` the job carries, and
    /// `None` for a run that did not fail.
    pub error: Option<crate::ops::types::ErrorDetails>,
    /// The counts of the run, and `None` for a run that never got as far as
    /// walking an entry (one whose source could not be read at all): there is
    /// no progress to report for a document nothing was read out of.
    pub progress: Option<crate::ops::types::MetadataTransferJobProgress>,
    /// Where the run's report was written, when there was an object store to
    /// write it to ([`transfer_run`]), and `None` when there was none: a report
    /// URL no caller could fetch is worse than none.
    pub report_url: Option<String>,
}

impl TransferOutcome {
    /// The outcome of a run that COMPLETED: it read its source and wrote its
    /// destination, and these are its counts.
    pub fn completed(
        progress: crate::ops::types::MetadataTransferJobProgress,
        report_url: Option<String>,
    ) -> Self {
        Self {
            state: METADATA_TRANSFER_JOB_STATE_COMPLETED.to_string(),
            error: None,
            progress: Some(progress),
            report_url,
        }
    }

    /// The outcome of a run that FAILED, carrying the modelled `ErrorCode`
    /// `code` and the sentence that says what the run could not do.
    ///
    /// `progress` is the counts the run had reached when it failed, and `None`
    /// where it had reached none.
    pub fn failed(
        code: &str,
        message: String,
        progress: Option<crate::ops::types::MetadataTransferJobProgress>,
        report_url: Option<String>,
    ) -> Self {
        Self {
            state: METADATA_TRANSFER_JOB_STATE_ERROR.to_string(),
            error: Some(crate::ops::types::ErrorDetails {
                code: Some(code.to_string()),
                message: Some(clamped_message(message, ERROR_MESSAGE_MAX_CHARS)),
            }),
            progress,
            report_url,
        }
    }
}

/// The document a workspace's metadata describes, through the job's filters.
///
/// The SOURCE side of an IoT TwinMaker end: what the service reads out of a
/// workspace when a transfer names it. The workspace's own BUILT-IN component
/// types are not part of it — every workspace is provisioned with them, so a
/// document carrying them would describe resources no import could create — and
/// neither are the scenes and sync jobs it holds: a metadata transfer moves
/// component types and entities (`MetadataTransferJobProgress` counts entries of
/// a metadata document), and nothing else.
///
/// The filters are the `IotTwinMakerSourceConfigurationFilter`s the job named
/// (`filterByEntity`, `filterByComponentType`): a source naming none describes
/// the whole workspace, and one naming any describes exactly what they select,
/// which is the union over the filters since each names one resource.
pub fn workspace_document(
    workspace: &Workspace,
    filters: &[crate::ops::types::IotTwinMakerSourceConfigurationFilter],
) -> MetadataDocument {
    let mut wanted_entities: BTreeSet<&str> = BTreeSet::new();
    let mut wanted_component_types: BTreeSet<&str> = BTreeSet::new();
    for filter in filters {
        match filter {
            crate::ops::types::IotTwinMakerSourceConfigurationFilter::filterByEntity(by) => {
                wanted_entities.insert(by.entity_id.as_str());
            }
            crate::ops::types::IotTwinMakerSourceConfigurationFilter::filterByComponentType(
                by,
            ) => {
                wanted_component_types.insert(by.component_type_id.as_str());
            }
        }
    }
    let filtered = !filters.is_empty();
    MetadataDocument {
        component_types: workspace
            .created_component_types()
            .filter(|record| {
                !filtered || wanted_component_types.contains(record.component_type_id.as_str())
            })
            .map(component_type_entry_of)
            .collect(),
        entities: workspace
            .entities
            .values()
            .filter(|record| !filtered || wanted_entities.contains(record.entity_id.as_str()))
            .map(entity_entry_of)
            .collect(),
    }
}
