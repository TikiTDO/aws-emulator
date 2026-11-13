// SPDX-License-Identifier: CC0-1.0

//! The image-usage-report store: the reports an account asked for and the refusals their ids
//! are read through.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The code EC2 answers for a report id the account holds no AMI usage report for.
///
/// EC2 names a resource a request could not resolve `Invalid<Resource>Id.NotFound` — as
/// [`INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND`] and [`INVALID_FPGA_IMAGE_ID_NOT_FOUND`] read —
/// so a report id that names nothing is answered the same way.
pub const INVALID_IMAGE_USAGE_REPORT_ID_NOT_FOUND: &str = "InvalidImageUsageReportId.NotFound";

/// The code EC2 answers for something that is not an AMI usage report id at all.
///
/// EC2 separates "not an id of this kind" from "no such resource" with the resource's own
/// `Invalid<Kind>.Malformed` code, the way [`INVALID_FPGA_IMAGE_ID_MALFORMED`] and
/// [`INVALID_LAUNCH_TEMPLATE_ID_MALFORMED`] read; the recorded unit behind
/// `issues/.../20261023T102725Z-ec2-deleteimageusagereport-stub.md` is where this code's
/// spelling for a report id comes from — AWS answered `DeleteImageUsageReport` for the null
/// report id with `InvalidImageUsageReportId.Malformed` (HTTP 400).
pub const INVALID_IMAGE_USAGE_REPORT_ID_MALFORMED: &str = "InvalidImageUsageReportId.Malformed";

/// The id prefix every AMI usage report id carries: `imgusgrpt-`.
///
/// `ImageUsageReportId` is a plain `String` in the model
/// (`smithy_query(query="shape",shape="ImageUsageReportId")`), so the prefix is taken from
/// the id AWS itself read as an "image-usage-report ID" in the recorded unit behind this
/// resource's issue (`imgusgrpt-00000000000000000`). It is the prefix a report id is minted
/// under as well, so one id form serves the mint and the readers.
pub const IMAGE_USAGE_REPORT_ID_PREFIX: &str = "imgusgrpt";

/// Whether `id` is a well-formed AMI usage report id — one EC2 could have ISSUED.
///
/// Two clauses, and each is a clause about the ID SPACE rather than about any one request:
///
/// * The EC2 resource id form: [`IMAGE_USAGE_REPORT_ID_PREFIX`] followed by 8 or 17
///   lowercase hexadecimal digits, the short and the long form every id reader in this
///   crate accepts for a resource whose own `@pattern` the model does not publish
///   ([`prefixed_id_is_well_formed`]).
/// * NOT the NULL id — the one definition of that clause, [`id_is_not_the_null_id`]: AWS
///   answers the all-zero suffix `InvalidImageUsageReportId.Malformed` rather than
///   `.NotFound`, and the mint counts from one
///   ([`ComAmazonawsEc2Amazonec2State::next_id`]), so no report can ever hold it.
///
/// The null-id clause is this resource's, not a rule about every EC2 id: the same all-zero
/// suffix on a network interface (`eni-00000000000000000`) IS answered `NotFound`, which is
/// why [`prefixed_id_is_well_formed`] does not carry it — the same split
/// [`instance_id_is_well_formed`] records.
pub fn image_usage_report_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(IMAGE_USAGE_REPORT_ID_PREFIX, id)
        && id_is_not_the_null_id(IMAGE_USAGE_REPORT_ID_PREFIX, id)
}

/// Refuse a request that named a report id the account holds no AMI usage report for: HTTP
/// 400 `InvalidImageUsageReportId.NotFound`.
pub fn invalid_image_usage_report_id_not_found<E>(report_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_IMAGE_USAGE_REPORT_ID_NOT_FOUND,
        400,
        format!("The image usage report id '{report_id}' does not exist"),
    )
}

/// Refuse a report id the request named that is not a report id at all: HTTP 400
/// `InvalidImageUsageReportId.Malformed`, which EC2 answers BEFORE it looks the id up.
///
/// The wording is AWS's own for this resource — "The image-usage-report ID
/// '<id>' is malformed" — built from the id the request carried, as every malformed-id
/// message in this crate is.
pub fn invalid_image_usage_report_id_malformed<E>(report_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_IMAGE_USAGE_REPORT_ID_MALFORMED,
        400,
        format!("The image-usage-report ID '{report_id}' is malformed"),
    )
}

// =============================================================================
// Instance export tasks
// =============================================================================

/// The code EC2 answers for an export task id the account holds no export task for.
pub const INVALID_EXPORT_TASK_ID_NOT_FOUND: &str = "InvalidExportTaskID.NotFound";

/// An export task that is still running: `ExportTaskState`'s `active`
/// (`crate::ops::types::EXPORT_TASK_STATE_VALUES` is the generated catalog).
pub const EXPORT_TASK_STATE_ACTIVE: &str = "active";

/// An export task a cancel has started taking down: `ExportTaskState`'s `cancelling`.
pub const EXPORT_TASK_STATE_CANCELLING: &str = "cancelling";

/// An export task whose cancellation is done: `ExportTaskState`'s `cancelled`.
pub const EXPORT_TASK_STATE_CANCELLED: &str = "cancelled";

/// An export task that finished: `ExportTaskState`'s `completed`.
pub const EXPORT_TASK_STATE_COMPLETED: &str = "completed";

/// One stored instance export task: the modelled [`ExportTask`] every operation that reports
/// one answers with, plus the move the clock owes a cancellation.
///
/// "Cancels an active export task" (`CancelExportTask`): a cancel puts the task into the
/// non-terminal `cancelling` state, so the clock owes it `cancelled` [`TRANSITION_DELAY`]
/// later whether or not anybody calls again. The due instant is stored and
/// [`ExportTaskRecord::view_at`] derives the state from it, the way [`VolumeRecord`] does
/// for a volume.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ExportTaskRecord {
    /// The export task as the model shapes it, carrying the state it has reached on its own.
    pub view: ExportTask,
    /// When the `cancelling` -> `cancelled` move of a cancelled task falls due; `None` for a
    /// task nothing has cancelled.
    #[serde(with = "::service_common::persist::instant_opt")]
    pub cancelled_at: Option<Instant>,
}

impl ExportTaskRecord {
    /// This export task as of `now`: the stored view with the state the clock has reached.
    pub fn view_at(&self, now: Instant) -> ExportTask {
        let mut view = self.view.clone();
        if let Some(state) = self.cancellation_state_at(now) {
            view.state = Some(state.to_string());
        }
        view
    }

    /// The state a cancelled task has reached at `now`, or `None` for a task nothing has
    /// cancelled — whose state is the one the task itself carries.
    pub(super) fn cancellation_state_at(&self, now: Instant) -> Option<&'static str> {
        self.cancelled_at.map(|due_at| {
            if now >= due_at {
                EXPORT_TASK_STATE_CANCELLED
            } else {
                EXPORT_TASK_STATE_CANCELLING
            }
        })
    }

    /// The task's `ExportTaskState` at `now`.
    pub fn state_at(&self, now: Instant) -> String {
        self.cancellation_state_at(now)
            .map(str::to_string)
            .or_else(|| self.view.state.clone())
            .unwrap_or_else(|| EXPORT_TASK_STATE_ACTIVE.to_string())
    }
}

/// Refuse a request that named an export task id the account holds no export task for: HTTP
/// 400 `InvalidExportTaskID.NotFound`.
pub fn invalid_export_task_id_not_found<E>(export_task_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_EXPORT_TASK_ID_NOT_FOUND,
        400,
        format!("The export task id '{export_task_id}' does not exist"),
    )
}

// ---------------------------------------------------------------------------
// Block public access for snapshots
// ---------------------------------------------------------------------------

/// "`unblocked` - Public sharing is not blocked. Users can publicly share snapshots"
/// (`GetSnapshotBlockPublicAccessStateResult$State`): the `SnapshotBlockPublicAccessState`
/// enum's `unblocked` member.
///
/// It is both where an account that has never enabled the setting sits and the state
/// `DisableSnapshotBlockPublicAccess` returns it to — "after you disable block public
/// access for snapshots in a Region, users can publicly share snapshots in that Region".
pub const SNAPSHOT_BLOCK_PUBLIC_ACCESS_UNBLOCKED: &str = "unblocked";

/// "`block-all-sharing` - Prevents all public sharing of snapshots in the Region"
/// (`EnableSnapshotBlockPublicAccessRequest$State`): the `SnapshotBlockPublicAccessState`
/// enum's `block-all-sharing` member.
pub const SNAPSHOT_BLOCK_PUBLIC_ACCESS_BLOCK_ALL_SHARING: &str = "block-all-sharing";

/// "`block-new-sharing` - Prevents only new public sharing of snapshots in the Region"
/// (`EnableSnapshotBlockPublicAccessRequest$State`): the `SnapshotBlockPublicAccessState`
/// enum's `block-new-sharing` member.
pub const SNAPSHOT_BLOCK_PUBLIC_ACCESS_BLOCK_NEW_SHARING: &str = "block-new-sharing";

/// The modes `EnableSnapshotBlockPublicAccess` accepts: the two blocking members of the
/// `SnapshotBlockPublicAccessState` catalog, in the order its own documentation lists
/// them.
///
/// The third member is NOT here on purpose: "`unblocked` is not a valid value for
/// **EnableSnapshotBlockPublicAccess**" (`EnableSnapshotBlockPublicAccessRequest$State`) —
/// turning the setting off is `DisableSnapshotBlockPublicAccess`'s job. The whole catalog
/// is the generated `crate::ops::types::SNAPSHOT_BLOCK_PUBLIC_ACCESS_STATE_VALUES`.
pub const SNAPSHOT_BLOCK_PUBLIC_ACCESS_MODES: [&str; 2] = [
    SNAPSHOT_BLOCK_PUBLIC_ACCESS_BLOCK_ALL_SHARING,
    SNAPSHOT_BLOCK_PUBLIC_ACCESS_BLOCK_NEW_SHARING,
];

/// "`account` - The state is managed by the account"
/// (`GetSnapshotBlockPublicAccessStateResult$ManagedBy`): the `ManagedBy` enum's `account`
/// member.
///
/// Every state this emulator can hold is one the account itself set through
/// `EnableSnapshotBlockPublicAccess` / `DisableSnapshotBlockPublicAccess`, or the default
/// it has never changed. The enum's other member, `declarative-policy`, names a state set
/// OUTSIDE EC2 — by an Organizations declarative policy — and "can't be modified by the
/// account", so no operation of this API can put the setting into it.
///
/// The same `ManagedBy` enum carries `InstanceMetadataDefaultsResponse$ManagedBy` ("the
/// entity that manages the IMDS default settings"), which
/// [`AccountInstanceMetadataDefaults::account_level`] reports from this same constant for
/// the same reason: the account's own writers are the only ones this emulator has.
pub const MANAGED_BY_ACCOUNT: &str = "account";

/// The [`SNAPSHOT_BLOCK_PUBLIC_ACCESS_MODES`] member `requested` names, or `None` when it
/// names none.
///
/// Answering a `&'static str` out of this module's own constants is what keeps request
/// text out of the store: a writer stores what this returns, so the stored word is always
/// a modelled catalog member. One line over [`catalog_member`], the module's single
/// catalog resolver, so this and the IMDS defaults resolve a request word the same way;
/// the catalog it searches is the documented SUBSET an enable accepts, not the whole
/// `SnapshotBlockPublicAccessState` enum.
pub fn snapshot_block_public_access_mode(requested: &str) -> Option<&'static str> {
    catalog_member(&SNAPSHOT_BLOCK_PUBLIC_ACCESS_MODES, requested)
}

impl ComAmazonawsEc2Amazonec2State {
    /// The state of the account's *block public access for snapshots* setting right now.
    ///
    /// A pure read: the setting has no clock-owed move — AWS reports the mode an enable
    /// asked for as soon as the enable answers — so nothing about it changes with time.
    pub fn snapshot_block_public_access_state(&self) -> &'static str {
        self.snapshot_block_public_access
    }

    /// Put the setting into `state` and answer the state now in effect.
    ///
    /// `state` is a `&'static str` so only one of this module's catalog constants can be
    /// stored; the answer is read back off the field rather than echoed, so a writer
    /// reports exactly what a later read will.
    pub fn set_snapshot_block_public_access_state(&mut self, state: &'static str) -> &'static str {
        self.snapshot_block_public_access = state;
        self.snapshot_block_public_access
    }
}
