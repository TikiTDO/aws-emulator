// SPDX-License-Identifier: CC0-1.0

//! The export-image-task store: the AMI exports an account has started, the status the clock
//! has moved each one to, and the ceilings the store keeps.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The resource type an `ExportImage` `TagSpecification` may name: "The tags to apply to the
/// export image task during creation" (`ExportImageRequest$TagSpecifications`), which the
/// modelled `ResourceType` enum spells `export-image-task`
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const EXPORT_IMAGE_TASK_TAGGABLE_RESOURCE: &str = "export-image-task";

/// "The status of the export image task. The possible values are `active`, `completed`,
/// `deleting`, and `deleted`" (`ExportImageTask$Status`): the export is still running.
pub const EXPORT_IMAGE_TASK_STATUS_ACTIVE: &str = "active";

/// The status of an export image task whose VM file has been written: `completed`, from the
/// catalog `ExportImageTask$Status` publishes.
pub const EXPORT_IMAGE_TASK_STATUS_COMPLETED: &str = "completed";

/// The status of an export image task a cancellation is taking down: `deleting`, from the
/// catalog `ExportImageTask$Status` publishes.
///
/// "Cancels an active export task. The request removes all artifacts of the export"
/// (`CancelExportTask`) — the non-terminal status that removal runs in, which the clock then
/// owes [`EXPORT_IMAGE_TASK_STATUS_DELETED`].
pub const EXPORT_IMAGE_TASK_STATUS_DELETING: &str = "deleting";

/// The status of an export image task whose cancellation is done: `deleted`, from the catalog
/// `ExportImageTask$Status` publishes.
pub const EXPORT_IMAGE_TASK_STATUS_DELETED: &str = "deleted";

/// Every status an export image task can report, in the order `ExportImageTask$Status` lists
/// them — which is also the list `DescribeExportImageTasksRequest$Filters` names for its
/// `task-state` filter ("one of the following values: active, completed, deleting, or
/// deleted").
///
/// `ExportImageTask$Status` is modelled as a plain `String` rather than as an enum, so this is
/// the crate's one reading of that published catalog: there is no generated
/// `..._VALUES` constant to defer to.
pub const EXPORT_IMAGE_TASK_STATUS_VALUES: [&str; 4] = [
    EXPORT_IMAGE_TASK_STATUS_ACTIVE,
    EXPORT_IMAGE_TASK_STATUS_COMPLETED,
    EXPORT_IMAGE_TASK_STATUS_DELETING,
    EXPORT_IMAGE_TASK_STATUS_DELETED,
];

/// How many export image tasks this emulator stores before `ExportImage` is refused.
///
/// CHOSEN, like [`MAX_STORED_IMPORT_IMAGE_TASKS`] and for the same reason: nothing in this
/// emulator reclaims a finished task, so the store needs a ceiling that is not the process's
/// memory. A count ceiling is not a memory bound on its own — see [`MAX_STORED_BYTES`] — so
/// the caller-chosen bytes of each task are charged against that budget too
/// ([`export_image_task_payload_bytes`]).
pub const MAX_STORED_EXPORT_IMAGE_TASKS: usize = 4_096;

/// One stored export image task: the modelled [`ExportImageTask`] every operation that
/// reports one answers with, plus the window the export takes and the two members
/// `ExportImageResult` carries that the task shape does not.
///
/// NOTHING about the status is stored: it is a fact of the clock — `active` until the export's
/// window has run out, `completed` after it, and `deleting` then `deleted` once a cancellation
/// has been started — so [`Self::view_at`] derives it on every read and no reporting operation
/// has to write to tell the truth about it. Completing an export MINTS nothing (the VM file it
/// writes is an object of another service), which is what lets it be derived here where an
/// import's completion has to be applied once.
#[derive(Debug, Clone)]
pub struct ExportImageTaskRecord {
    /// The task as the model shapes it, with `Status` and `Progress` left for
    /// [`Self::view_at`] to fill from the clock.
    pub view: ExportImageTask,
    /// "The disk image format for the exported image" (`ExportImageResult$DiskImageFormat`):
    /// a fact of the task that [`ExportImageTask`] carries no member for, kept here so a
    /// repeat of the request that started the task reports it unchanged.
    pub disk_image_format: String,
    /// "The name of the role that grants VM Import/Export permission to export images to your
    /// Amazon S3 bucket" (`ExportImageResult$RoleName`) — the other `ExportImageResult` member
    /// the task shape does not carry; `None` for a request that named none.
    pub role_name: Option<String>,
    /// When the export started, which progress is measured from.
    pub started_at: Instant,
    /// The instant from which this task's `active` -> `completed` move is due. Monotonic
    /// ([`Instant`]), because it is only ever compared against the clock and never reported,
    /// so a wall-clock adjustment cannot move a task back to `active`.
    pub completed_at: Instant,
    /// When the `deleting` -> `deleted` move of a cancelled task falls due; `None` for a task
    /// nothing has cancelled.
    pub deleted_at: Option<Instant>,
}

impl ExportImageTaskRecord {
    /// The status this task reports at `now`.
    ///
    /// A started cancellation decides it — `deleting` until that removal is done, `deleted`
    /// after — and otherwise the export's own window does: `active` until it has run out and
    /// `completed` once it has.
    pub fn status_at(&self, now: Instant) -> &'static str {
        if let Some(deleted_at) = self.deleted_at {
            return if now >= deleted_at {
                EXPORT_IMAGE_TASK_STATUS_DELETED
            } else {
                EXPORT_IMAGE_TASK_STATUS_DELETING
            };
        }
        if now >= self.completed_at {
            EXPORT_IMAGE_TASK_STATUS_COMPLETED
        } else {
            EXPORT_IMAGE_TASK_STATUS_ACTIVE
        }
    }

    /// This task as of `now`: the stored view, with the status the clock has reached and the
    /// percentage an export still running has made by then. A read observes both without
    /// writing anything.
    pub fn view_at(&self, now: Instant) -> ExportImageTask {
        let mut view = self.view.clone();
        let status = self.status_at(now);
        view.status = Some(status.to_string());
        // "The percent complete of the export image task" (`ExportImageTask$Progress`): a task
        // that is no longer running has no progress left to report, which is how an import
        // image task reports a finished conversion too.
        view.progress = (status == EXPORT_IMAGE_TASK_STATUS_ACTIVE).then(|| {
            // The share of the task's window that has elapsed, read through the crate's ONE
            // reader of that share, so a VM conversion reports one percentage for one elapsed
            // share whichever direction it runs in.
            import_task_progress_at(self.started_at, self.completed_at, now)
        });
        view
    }

    /// This task as `ExportImage` reports it: [`Self::view_at`] through the members
    /// `ExportImageResult` declares, which are the task's own plus the disk image format and
    /// the role the export runs under.
    pub fn export_response(&self, now: Instant) -> ExportImageOutput {
        let view = self.view_at(now);
        ExportImageOutput {
            description: view.description,
            disk_image_format: Some(self.disk_image_format.clone()),
            export_image_task_id: view.export_image_task_id,
            image_id: view.image_id,
            role_name: self.role_name.clone(),
            progress: view.progress,
            s3_export_location: view.s3_export_location,
            status: view.status,
            status_message: view.status_message,
            tags: view.tags,
        }
    }
}

/// The caller-chosen bytes one stored export image task holds: the description and the S3
/// destination the request named, its tags, the role it runs under and the `ClientToken` the
/// token map keeps for it. Everything else on the record — the ids, the status, the disk image
/// format — is derived here or drawn from a closed catalog, so it is fixed-size.
///
/// The ONE definition of that charge: `ExportImage` asks it before the write and charges
/// exactly it, the way [`import_image_task_payload_bytes`] is asked and charged.
pub fn export_image_task_payload_bytes(
    view: &ExportImageTask,
    role_name: Option<&str>,
    client_token: Option<&str>,
) -> usize {
    let destination = view.s3_export_location.as_ref().map_or(0, |location| {
        location.s3_bucket.as_ref().map_or(0, String::len)
            + location.s3_prefix.as_ref().map_or(0, String::len)
    });
    view.description
        .as_ref()
        .map_or(0, String::len)
        .saturating_add(tag_list_bytes(view.tags.as_deref().unwrap_or_default()))
        .saturating_add(destination)
        .saturating_add(role_name.map_or(0, str::len))
        .saturating_add(client_token.map_or(0, str::len))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another export image task fits inside the store's ceiling
    /// ([`MAX_STORED_EXPORT_IMAGE_TASKS`]).
    ///
    /// The same contract as [`Self::has_import_image_task_capacity`]: a create asks first and
    /// refuses instead of growing the store past its ceiling.
    pub fn has_export_image_task_capacity(&self) -> bool {
        self.export_image_tasks.len() < MAX_STORED_EXPORT_IMAGE_TASKS
    }
}
