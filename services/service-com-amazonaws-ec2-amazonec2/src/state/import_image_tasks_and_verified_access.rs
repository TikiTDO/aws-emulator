// SPDX-License-Identifier: CC0-1.0

//! The import-image and import-snapshot task stores, the secondary-network spec they share,
//! and the Verified Access instance, group, endpoint and trust-provider stores.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// The resource type an `ImportImage` `TagSpecification` may name: "The tags to apply to
/// the import image task during creation" (`ImportImageRequest$TagSpecifications`), which
/// the modelled `ResourceType` enum spells `import-image-task`.
pub const IMPORT_IMAGE_TASK_TAGGABLE_RESOURCE: &str = "import-image-task";

/// The id prefix of an import image task, the form `ImportImageTask$ImportTaskId` carries.
pub const IMPORT_IMAGE_TASK_ID_PREFIX: &str = "import-ami";

/// [`IMPORT_IMAGE_TASK_ID_PREFIX`] and the hyphen that separates it from the id's hex
/// suffix, as one static, so recognising an id allocates nothing on the request path.
pub const IMPORT_IMAGE_TASK_ID_PREFIX_WITH_HYPHEN: &str = "import-ami-";

/// How many disk containers one `ImportImage` request may name.
///
/// CHOSEN, like [`MAX_SECURITY_GROUPS_PER_INSTANCE`]: neither the model nor the request
/// parsers bound the length of `DiskContainers` (the list carries no `@length` and the
/// query parser bounds nesting depth only), while every container retains a
/// `SnapshotDetail` on the stored task for the process's lifetime. It is well above the
/// disks of any VM a test would import.
pub const MAX_IMPORT_DISK_CONTAINERS: usize = 32;

/// How many license configurations one `ImportImage` request may name, for the same reason
/// and in the same style as [`MAX_IMPORT_DISK_CONTAINERS`]: the modelled list is unbounded
/// and every entry is retained on the stored task.
pub const MAX_IMPORT_LICENSE_SPECIFICATIONS: usize = 32;

/// What one element of a stored import task's caller-chosen lists costs against the store's
/// byte budget beyond the strings it carries.
///
/// A `SnapshotDetail` or a license specification whose every string is empty still retains
/// a record, so charging string lengths alone would let a request store many elements for
/// nothing. The number is the order of the stored struct, not a measurement of it — it only
/// has to make the per-element cost non-zero.
pub const IMPORT_TASK_ELEMENT_OVERHEAD_BYTES: usize = 256;

/// The state of an import task that is still running.
///
/// Source for this and the three below: `DescribeImportImageTasksRequest$Filters` —
/// "Filter tasks using the task-state filter and one of the following values: active,
/// completed, deleting, or deleted."
pub const IMPORT_TASK_STATE_ACTIVE: &str = "active";

/// The state of an import task whose AMI has been created.
pub const IMPORT_TASK_STATE_COMPLETED: &str = "completed";

/// The state of an import task a cancel is working through.
pub const IMPORT_TASK_STATE_DELETING: &str = "deleting";

/// The state of an import task a cancel has finished with.
pub const IMPORT_TASK_STATE_DELETED: &str = "deleted";

/// Every state an import task reports, the catalog the `task-state` filter selects over.
pub const IMPORT_TASK_STATES: [&str; 4] = [
    IMPORT_TASK_STATE_ACTIVE,
    IMPORT_TASK_STATE_COMPLETED,
    IMPORT_TASK_STATE_DELETING,
    IMPORT_TASK_STATE_DELETED,
];

/// The architectures `ImportImage` accepts: "Valid values: i386 | x86_64"
/// (`ImportImageRequest$Architecture`).
pub const IMPORT_IMAGE_ARCHITECTURES: [&str; 2] = ["i386", "x86_64"];

/// The target hypervisors `ImportImage` accepts: "Valid values: xen"
/// (`ImportImageRequest$Hypervisor`), which is therefore also the one a task that named
/// none reports.
pub const IMPORT_IMAGE_HYPERVISORS: [&str; 1] = [IMAGE_HYPERVISOR_XEN];

/// The operating systems `ImportImage` accepts: "Valid values: Windows | Linux"
/// (`ImportImageRequest$Platform`).
pub const IMPORT_IMAGE_PLATFORMS: [&str; 2] = ["Linux", IMAGE_PLATFORM_WINDOWS];

/// The license type that replaces the source-system license: "Specify AWS to replace the
/// source-system license with an Amazon Web Services license" (`ImportImageRequest$LicenseType`).
pub const IMPORT_IMAGE_LICENSE_TYPE_AWS: &str = "AWS";

/// The license type that retains the source-system license: "or BYOL to retain the
/// source-system license" (`ImportImageRequest$LicenseType`).
pub const IMPORT_IMAGE_LICENSE_TYPE_BYOL: &str = "BYOL";

/// The license types `ImportImage` accepts.
pub const IMPORT_IMAGE_LICENSE_TYPES: [&str; 2] = [
    IMPORT_IMAGE_LICENSE_TYPE_AWS,
    IMPORT_IMAGE_LICENSE_TYPE_BYOL,
];

/// The disk image formats a disk container may carry: "Valid values: OVA | VHD | VHDX |
/// VMDK | RAW" (`ImageDiskContainer$Format`).
pub const IMPORT_DISK_IMAGE_FORMATS: [&str; 5] = ["OVA", "VHD", "VHDX", "VMDK", "RAW"];

/// The status message an import image task carries while its disks are being converted,
/// which is the whole of the window this emulator's import takes.
pub const IMPORT_TASK_STATUS_MESSAGE_CONVERTING: &str = "converting";

/// The highest percentage an ACTIVE import task reports: a task that has not finished has
/// not reached 100, and 100 is only ever read off a `completed` one.
pub const MAX_ACTIVE_IMPORT_PROGRESS: u64 = 99;

/// How many import image tasks this emulator stores before `ImportImage` is refused.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a finished task, so the store needs a ceiling that is not the process's
/// memory. A count ceiling is not a memory bound on its own — see [`MAX_STORED_BYTES`] —
/// so the caller-chosen bytes of each task are charged against that budget too
/// ([`import_image_task_payload_bytes`]).
pub const MAX_STORED_IMPORT_IMAGE_TASKS: usize = 4_096;

/// EC2's generic resource-count quota code, which a refusal to store another resource of a
/// kind with no quota code of its own answers with.
pub const RESOURCE_COUNT_EXCEEDED: &str = "ResourceCountExceeded";

/// EC2's code for a conversion (import) task id it cannot resolve — the code every
/// operation that names an import task answers for a value that is not an import task id,
/// and for an id no task of the account has.
pub const INVALID_CONVERSION_TASK_ID: &str = "InvalidConversionTaskId";

/// One stored import image task: the modelled [`ImportImageTask`] every operation that
/// reports one answers with, plus the window the import takes.
///
/// The stored view is the truth about the task: its `Status` starts at
/// [`IMPORT_TASK_STATE_ACTIVE`] and the clock-owed completion is written onto it ONCE by
/// [`ComAmazonawsEc2Amazonec2State::complete_due_import_image_tasks`], because completing
/// MINTS resources (the AMI, and a snapshot id per imported disk) that have to be the same
/// in every later read. Only the progress PERCENTAGE is derived per read
/// ([`Self::view_at`]): it is a fact of elapsed time, so no stored value could be right
/// twice.
#[derive(Debug, Clone)]
pub struct ImportImageTaskRecord {
    /// The task as the model shapes it, with `Progress` left for [`Self::view_at`] to fill
    /// while the task is active.
    pub view: ImportImageTask,
    /// When the import started, which progress is measured from.
    pub started_at: Instant,
    /// The instant from which this task's `active` -> `completed` move is due. Monotonic
    /// ([`Instant`]), because it is only ever compared against the clock and never
    /// reported, so a wall-clock adjustment cannot move a task back to `active`.
    pub completed_at: Instant,
}

impl ImportImageTaskRecord {
    /// This task as of `now`: the stored view, with the percentage of progress an active
    /// import has made by `now` on the task and on each of its disks. A read observes it
    /// without writing anything.
    pub fn view_at(&self, now: Instant) -> ImportImageTask {
        let mut view = self.view.clone();
        // A task that is no longer active has no progress left to report: "The percentage
        // of progress of the import image task" (`ImportImageTask$Progress`).
        if view.status.as_deref() != Some(IMPORT_TASK_STATE_ACTIVE) {
            return view;
        }
        let progress = self.progress_at(now);
        for detail in view.snapshot_details.iter_mut().flatten() {
            detail.progress = Some(progress.clone());
        }
        view.progress = Some(progress);
        view
    }

    /// The percentage of progress this import has made by `now`, as the string the
    /// `Progress` members carry.
    ///
    /// Derived from the share of the import's window that has elapsed, through the ONE
    /// reader of that share ([`import_task_progress_at`]), so an import image task and an
    /// import snapshot task cannot report two different percentages for the same elapsed
    /// share of their window.
    pub fn progress_at(&self, now: Instant) -> String {
        import_task_progress_at(self.started_at, self.completed_at, now)
    }

    /// This task as `ImportImage` reports it: [`Self::view_at`] through the members
    /// `ImportImageResult` declares, which are the task's own minus the `BootMode` that
    /// shape carries no member for.
    pub fn import_response(&self, now: Instant) -> ImportImageOutput {
        let view = self.view_at(now);
        ImportImageOutput {
            architecture: view.architecture,
            description: view.description,
            encrypted: view.encrypted,
            hypervisor: view.hypervisor,
            image_id: view.image_id,
            import_task_id: view.import_task_id,
            kms_key_id: view.kms_key_id,
            license_type: view.license_type,
            platform: view.platform,
            progress: view.progress,
            snapshot_details: view.snapshot_details,
            status: view.status,
            status_message: view.status_message,
            license_specifications: view.license_specifications,
            tags: view.tags,
            usage_operation: view.usage_operation,
        }
    }
}

/// The size, in whole GiB, of the EBS volume a disk image of `disk_image_size` GiB needs:
/// "the size of the uploaded disk image, in GiB" (`ClientData$UploadSize`) rounded UP,
/// because a volume a GiB short of the disk could not hold it.
///
/// `None` for a size no disk has — a size the request never named is `None` before it gets
/// here, and one that is not a finite number of at least one GiB names no volume either, so
/// the caller falls back to the size it would have used anyway. The ONE reader of that
/// rounding for every kind of VM import, so an imported AMI's root volume and an imported
/// snapshot's volume cannot disagree about the same disk.
pub fn imported_disk_volume_size(disk_image_size: f64) -> Option<i32> {
    (disk_image_size.is_finite() && disk_image_size >= 1.0)
        .then(|| i32::try_from(disk_image_size.ceil() as i64).unwrap_or(i32::MAX))
}

/// The percentage of progress an import running from `started_at` to `ends_at` has made by
/// `now`, as the string an import task's `Progress` members carry.
///
/// Derived from the share of the import's window that has elapsed, and held below
/// [`MAX_ACTIVE_IMPORT_PROGRESS`] because a task still reporting progress has not finished.
/// The ONE reader of that share, for every kind of import task: a percentage is a fact of
/// elapsed time, so no stored value could be right twice and no second definition of it
/// could agree with this one.
pub fn import_task_progress_at(started_at: Instant, ends_at: Instant, now: Instant) -> String {
    let window = ends_at.saturating_duration_since(started_at).as_nanos();
    let elapsed = now
        .saturating_duration_since(started_at)
        .as_nanos()
        .min(window);
    let percent = elapsed
        .saturating_mul(100)
        .checked_div(window)
        .and_then(|percent| u64::try_from(percent).ok())
        .unwrap_or(MAX_ACTIVE_IMPORT_PROGRESS)
        .min(MAX_ACTIVE_IMPORT_PROGRESS);
    percent.to_string()
}

/// The caller-chosen bytes one stored import image task holds: its description, the KMS
/// key and usage operation the request named, its tags, the license configuration ARNs,
/// the per-disk strings of its snapshot details, and the `ClientToken` the token map keeps
/// for it. Everything else on the record — the ids, the states, the architecture, the
/// platform and the license type — is derived here or drawn from a closed catalog, so it
/// is fixed-size.
///
/// Each ELEMENT of the two caller-chosen lists is charged
/// [`IMPORT_TASK_ELEMENT_OVERHEAD_BYTES`] beyond the strings it carries, so a request of
/// many empty-string disk containers cannot store a record per container for a zero-byte
/// charge — the same reason [`launch_template_payload_bytes`] charges the rendered size of
/// a nested caller structure rather than the string lengths inside it.
///
/// The ONE definition of that charge: `ImportImage` asks it before the write and charges
/// exactly it.
pub fn import_image_task_payload_bytes(view: &ImportImageTask, client_token: Option<&str>) -> usize {
    let snapshot_bytes: usize = view
        .snapshot_details
        .iter()
        .flatten()
        .map(|detail| {
            IMPORT_TASK_ELEMENT_OVERHEAD_BYTES
                + detail.description.as_ref().map_or(0, String::len)
                + detail.device_name.as_ref().map_or(0, String::len)
                + detail.url.as_ref().map_or(0, String::len)
                + detail.user_bucket.as_ref().map_or(0, |bucket| {
                    bucket.s3_bucket.as_ref().map_or(0, String::len)
                        + bucket.s3_key.as_ref().map_or(0, String::len)
                })
        })
        .sum();
    let license_bytes: usize = view
        .license_specifications
        .iter()
        .flatten()
        .map(|specification| {
            IMPORT_TASK_ELEMENT_OVERHEAD_BYTES
                + specification
                    .license_configuration_arn
                    .as_ref()
                    .map_or(0, String::len)
        })
        .sum();
    view.description
        .as_ref()
        .map_or(0, String::len)
        .saturating_add(view.kms_key_id.as_ref().map_or(0, String::len))
        .saturating_add(view.usage_operation.as_ref().map_or(0, String::len))
        .saturating_add(tag_list_bytes(view.tags.as_deref().unwrap_or_default()))
        .saturating_add(snapshot_bytes)
        .saturating_add(license_bytes)
        .saturating_add(client_token.map_or(0, str::len))
}

/// Whether `id` is a well-formed import task id of the family `prefix` (carrying its
/// hyphen) names: that prefix followed by a non-empty suffix of lowercase hexadecimal
/// digits.
///
/// The suffix's LENGTH is deliberately not part of the form, unlike the 8-or-17-digit
/// reading of the resource ids this service mints elsewhere: the recorded refusal behind
/// `issues/.../20261022T235108Z-ec2-importimage-stub.md` names an `import-ami-` id with an
/// eighteen-digit suffix and is answered as a task that DOES NOT EXIST, not as a value of
/// the wrong format — so this parameter is read by the task KIND its prefix names, and
/// whether the account holds such a task is what decides the rest.
///
/// The ONE reader of that form for every kind of import task, so the ids of an import image
/// task and of an import snapshot task can differ only in their prefix.
pub fn import_task_id_is_well_formed(prefix: &str, id: &str) -> bool {
    let Some(suffix) = id.strip_prefix(prefix) else {
        return false;
    };
    !suffix.is_empty()
        && suffix
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Refuse an `ImportTaskIds` entry that is not an import task id of the family `prefix`
/// names: HTTP 400 [`INVALID_CONVERSION_TASK_ID`], EC2's code for a conversion task id it
/// cannot resolve.
///
/// Only a MALFORMED id is refused. A well-formed id no task of the account has is not an
/// error: an import-task describe answers "a list of zero or more" tasks, so such an id
/// simply selects no row — which is what the real service does with one. The ONE definition
/// of that refusal for every import-task describe.
pub fn require_well_formed_import_task_id<E>(
    prefix: &str,
    task_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if import_task_id_is_well_formed(prefix, task_id) {
        return Ok(());
    }
    Err(invalid_import_task_id_format(task_id))
}

/// Refuse a value a request named as an import task id that is not an import task id at
/// all: HTTP 400 [`INVALID_CONVERSION_TASK_ID`].
///
/// The id form is read BEFORE the store, and the import task family carries one code for
/// every id it cannot resolve: a value outside the form is answered under the same code as
/// a well-formed id that names no task ([`invalid_conversion_task_id_not_found`]), not
/// under the code for an id of a resource kind the service does not know. The ONE
/// definition of that refusal for every operation naming an import task, of either kind.
pub fn invalid_import_task_id_format<E>(task_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_CONVERSION_TASK_ID,
        400,
        invalid_id_message(task_id),
    )
}

/// Every kind of import task, as the prefix (with its hyphen) an id of that kind is minted
/// under paired with the modelled `ResourceType` the service spells that kind by: "the ID of
/// the import image or import snapshot task" (`CancelImportTaskRequest$ImportTaskId`) names
/// both kinds.
///
/// The two are one table rather than two parallel lists so the pairing cannot come apart,
/// and the ONE place the kinds are enumerated, so a kind added to the store is recognised
/// by every reader of an import task id at once.
pub const IMPORT_TASK_KINDS: [(&str, &str); 2] = [
    (
        IMPORT_IMAGE_TASK_ID_PREFIX_WITH_HYPHEN,
        IMPORT_IMAGE_TASK_TAGGABLE_RESOURCE,
    ),
    (
        IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN,
        IMPORT_SNAPSHOT_TASK_TAGGABLE_RESOURCE,
    ),
];

/// Whether `id` is a well-formed import task id of ANY kind this store holds, for the
/// operations that act on an import task whichever kind it is.
pub fn import_task_id_of_any_kind_is_well_formed(id: &str) -> bool {
    import_task_kind_of(id).is_some()
}

/// The modelled `ResourceType` of the import task kind `id`'s prefix names — the spelling a
/// refusal naming such a task uses for its kind — or `None` when the value names no kind of
/// import task at all.
///
/// The ONE place a kind is read off an id, so the reader of the id's form and the wording of
/// a refusal cannot disagree about which kind a value names.
pub fn import_task_kind_of(id: &str) -> Option<&'static str> {
    IMPORT_TASK_KINDS
        .iter()
        .find(|(prefix, _)| import_task_id_is_well_formed(prefix, id))
        .map(|(_, resource_type)| *resource_type)
}

/// Refuse a request naming an import task id this account holds no import task for: HTTP
/// 400 [`INVALID_CONVERSION_TASK_ID`], with EC2's not-found sentence for the KIND the id
/// names — the task kind's modelled resource type, then `ID`, then the id the request sent.
///
/// The counterpart of [`invalid_import_task_id_format`], which answers the same code for a
/// value that names no import task kind at all. The ONE wording for an import task of
/// either kind a request names and this account does not hold.
pub fn import_task_id_not_found<E>(import_task_id: &str) -> crate::scaffolded::Scaffolded<E> {
    match import_task_kind_of(import_task_id) {
        // The id is echoed through [`echoed_resource_id`], the bound every refusal that
        // quotes a caller's value holds to: a suffix of any length reads as an id of a
        // kind, so what the message repeats back is bounded here rather than by the form.
        Some(resource_type) => id_not_found(
            INVALID_CONVERSION_TASK_ID,
            &format!("{resource_type} ID"),
            &echoed_resource_id(import_task_id),
        ),
        // Unreachable through an operation, which reads the id's kind before it looks a
        // task up: a value naming no kind is answered as a format failure instead.
        None => invalid_import_task_id_format(import_task_id),
    }
}

/// Whether `id` is a well-formed import IMAGE task id: [`import_task_id_is_well_formed`] on
/// the `import-ami-` family.
pub fn import_image_task_id_is_well_formed(id: &str) -> bool {
    import_task_id_is_well_formed(IMPORT_IMAGE_TASK_ID_PREFIX_WITH_HYPHEN, id)
}

/// Refuse a `DescribeImportImageTasks` `ImportTaskIds` entry that is not an import image
/// task id at all: [`require_well_formed_import_task_id`] on the `import-ami-` family, so
/// "a list of zero or more import image tasks"
/// (`DescribeImportImageTasksResult$ImportImageTasks`) still covers a well-formed id the
/// account holds no task for.
pub fn require_well_formed_import_image_task_id<E>(
    task_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    require_well_formed_import_task_id(IMPORT_IMAGE_TASK_ID_PREFIX_WITH_HYPHEN, task_id)
}

/// Refuse a create that would take the store past a ceiling of its own:
/// HTTP 400 [`RESOURCE_COUNT_EXCEEDED`].
pub fn resource_count_exceeded<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(RESOURCE_COUNT_EXCEEDED, 400, message)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another import image task fits inside the store's ceiling
    /// ([`MAX_STORED_IMPORT_IMAGE_TASKS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// instead of growing the store past its ceiling.
    pub fn has_import_image_task_capacity(&self) -> bool {
        self.import_image_tasks.len() < MAX_STORED_IMPORT_IMAGE_TASKS
    }

    /// Complete every import image task whose `active` -> `completed` move the clock owed
    /// by `now`.
    ///
    /// Reached only through [`Self::materialize_due_transitions`], so a reporting operation
    /// observes the completion without having caused it: the move was owed at this instant
    /// whether or not anybody called. It is a WRITE rather than a derive because completing
    /// an import MINTS resources — "the ID of the Amazon Machine Image (AMI) of the
    /// imported virtual machine" (`ImportImageTask$ImageId`) and "the snapshot ID of the
    /// disk being imported" (`SnapshotDetail$SnapshotId`) — which every later read has to
    /// agree on.
    pub fn complete_due_import_image_tasks(&mut self, now: Instant) {
        let due: Vec<String> = self
            .import_image_tasks
            .iter()
            .filter(|(_, record)| {
                record.view.status.as_deref() == Some(IMPORT_TASK_STATE_ACTIVE)
                    && now >= record.completed_at
            })
            .map(|(task_id, _)| task_id.clone())
            .collect();
        for task_id in due {
            // The AMI the import creates is an image of this store like any other, so it is
            // stored through the one writer of an image rather than beside it.
            let Some(spec) = self.imported_image_spec(&task_id) else {
                continue;
            };
            // No room for the AMI means the import cannot produce one at all, so it ends in
            // the state a failed import ends in rather than staying `active` for ever: the
            // task is `deleted` and its message says which ceiling stopped it — the number of
            // images this store holds, or the caller-chosen bytes they hold, a count of
            // records being no bound on the bytes each one carries.
            let no_room = if !self.has_image_capacity() {
                Some(format!(
                    "This emulator holds {MAX_IMAGES} images, which is as many as it \
                     stores, so the imported AMI could not be created"
                ))
            } else {
                let payload_bytes = image_payload_bytes(&spec);
                self.would_exceed_stored_bytes(payload_bytes).then(|| {
                    format!(
                        "This emulator holds {held} of at most {MAX_STORED_BYTES} bytes of \
                         caller-supplied data, so the imported AMI could not be created \
                         with {payload_bytes} more",
                        held = self.stored_bytes()
                    )
                })
            };
            if let Some(reason) = no_room {
                if let Some(record) = self.import_image_tasks.get_mut(&task_id) {
                    record.view.status = Some(IMPORT_TASK_STATE_DELETED.to_string());
                    record.view.status_message = Some(reason);
                    record.view.progress = None;
                    for detail in record.view.snapshot_details.iter_mut().flatten() {
                        detail.status = Some(IMPORT_TASK_STATE_DELETED.to_string());
                        detail.progress = None;
                    }
                }
                continue;
            }
            let image_id = self.insert_image(spec);
            let disks = self
                .import_image_tasks
                .get(&task_id)
                .map_or(0, |record| {
                    record.view.snapshot_details.as_ref().map_or(0, Vec::len)
                });
            let snapshot_ids: Vec<String> = (0..disks).map(|_| self.next_id("snap")).collect();
            if let Some(record) = self.import_image_tasks.get_mut(&task_id) {
                record.view.status = Some(IMPORT_TASK_STATE_COMPLETED.to_string());
                // The conversion is over, so there is no stage left to report and no
                // percentage left to make.
                record.view.status_message = None;
                record.view.progress = None;
                record.view.image_id = Some(image_id);
                for (detail, snapshot_id) in record
                    .view
                    .snapshot_details
                    .iter_mut()
                    .flatten()
                    .zip(snapshot_ids)
                {
                    detail.snapshot_id = Some(snapshot_id);
                    detail.status = Some(IMPORT_TASK_STATE_COMPLETED.to_string());
                    detail.progress = None;
                }
            }
        }
    }

    /// The AMI a completed import image task created, as [`Self::insert_image`] takes it.
    ///
    /// Everything comes off the task: the AMI is named after the import that created it,
    /// carries the architecture, platform and boot mode the request named, and its root
    /// volume is as large as the largest disk the import converted. The task's own tags are
    /// NOT copied onto it — "the tags to apply to the import image task during creation"
    /// (`ImportImageRequest$TagSpecifications`) tags the task, not the image.
    pub(super) fn imported_image_spec(&self, task_id: &str) -> Option<ImageSpec> {
        let record = self.import_image_tasks.get(task_id)?;
        let windows = record.view.platform.as_deref() == Some(IMAGE_PLATFORM_WINDOWS);
        let defaults = ImageSpec::default();
        let default_architecture = defaults.architecture.clone();
        let root_volume_size = record
            .view
            .snapshot_details
            .iter()
            .flatten()
            .filter_map(|detail| detail.disk_image_size)
            .filter_map(imported_disk_volume_size)
            .max()
            .unwrap_or(if windows {
                WINDOWS_ROOT_VOLUME_SIZE
            } else {
                LINUX_ROOT_VOLUME_SIZE
            });
        Some(ImageSpec {
            name: task_id.to_string(),
            description: record.view.description.clone().unwrap_or_default(),
            architecture: record
                .view
                .architecture
                .clone()
                .unwrap_or(default_architecture),
            platform: record.view.platform.clone().filter(|_| windows),
            platform_details: if windows {
                PLATFORM_DETAILS_WINDOWS.to_string()
            } else {
                PLATFORM_DETAILS_LINUX.to_string()
            },
            usage_operation: record.view.usage_operation.clone().unwrap_or_else(|| {
                if windows {
                    USAGE_OPERATION_WINDOWS.to_string()
                } else {
                    USAGE_OPERATION_LINUX.to_string()
                }
            }),
            boot_mode: record.view.boot_mode.clone(),
            root_device_name: if windows {
                WINDOWS_ROOT_DEVICE_NAME.to_string()
            } else {
                LINUX_ROOT_DEVICE_NAME.to_string()
            },
            root_volume_size,
            state: IMAGE_STATE_AVAILABLE.to_string(),
            ..defaults
        })
    }
}

/// EC2's message for a snapshot id no snapshot of the account has, in ONE place: the
/// request-level refusal ([`invalid_snapshot_id_not_found`]) and the per-pair item a fast
/// snapshot restore call reports it in ([`unresolvable_snapshot_id_error`]) carry the same
/// sentence, so a client reads one wording per cause whichever channel it arrives in.
pub(super) fn snapshot_id_not_found_message(snapshot_id: &str) -> String {
    format!("The snapshot '{snapshot_id}' does not exist.")
}

/// EC2's message for a value that is not a snapshot id at all: "Value
/// (snap-00000000000000000) for snapshot ID is invalid.", as AWS answered the recorded
/// `DisableFastSnapshotRestores` call behind
/// `issues/.../20261023T102725Z-ec2-disablefastsnapshotrestores-stub.md` — the message is
/// built from the value the request gave, so the VALUE is the request's and the sentence
/// around it is the service's.
pub(super) fn snapshot_id_malformed_message(snapshot_id: &str) -> String {
    format!("Value ({snapshot_id}) for snapshot ID is invalid.")
}

/// Refuse a request that named a snapshot id no snapshot of the account has, the way
/// EC2 does: HTTP 400 `InvalidSnapshot.NotFound`.
pub fn invalid_snapshot_id_not_found<E>(snapshot_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_SNAPSHOT_ID_NOT_FOUND,
        400,
        snapshot_id_not_found_message(snapshot_id),
    )
}

/// Whether `id` is a well-formed EBS snapshot id — one EC2 could have ISSUED.
///
/// A snapshot id is `snap-` followed by 8 or 17 lowercase hexadecimal digits — the short
/// and the long id form, as the model's own `^snap-([0-9a-f]{8}|[0-9a-f]{17})$` style id
/// patterns read — of which not all are `0`.
///
/// That last clause is the ALL-ZERO id, the value every client reaches for as a
/// placeholder: EC2 answers it `InvalidSnapshotID.Malformed` rather than
/// `InvalidSnapshot.NotFound`, because the mint counts from one so no snapshot can ever
/// hold the null id. The evidence is the recorded `DisableFastSnapshotRestores` call behind
/// `issues/.../20261023T102725Z-ec2-disablefastsnapshotrestores-stub.md`, which named
/// `snap-00000000000000000` — seventeen hexadecimal digits, so well formed under the
/// structural rule alone — and was answered `InvalidSnapshotID.Malformed`. It is the same
/// clause [`instance_id_is_well_formed`] carries on its own recorded evidence, and it is a
/// fact about THESE id kinds rather than about every EC2 id: a network interface id answers
/// `NotFound` for the same all-zero suffix, so the clause is not generalised to the readers
/// built on [`prefixed_id_is_well_formed`].
pub fn snapshot_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed("snap", id) && id_is_not_the_null_id("snap", id)
}

/// The error code and message EC2 answers a snapshot id this service cannot resolve with: a
/// malformed one as malformed, and a well-formed one as not found — the way the service
/// orders the two, and the same ordering [`require_known_volume`] keeps.
///
/// The classification is a PAIR rather than an error so that both channels EC2 carries it
/// in read it: the request-level refusal ([`unresolvable_snapshot_id`], which `CreateVolume`
/// and `ImportImage` answer a `SnapshotId` with) and the per-snapshot item a fast snapshot
/// restore call reports, where one unresolvable snapshot must not refuse the snapshots
/// beside it.
pub fn unresolvable_snapshot_id_error(snapshot_id: &str) -> (&'static str, String) {
    if snapshot_id_is_well_formed(snapshot_id) {
        return (
            INVALID_SNAPSHOT_ID_NOT_FOUND,
            snapshot_id_not_found_message(snapshot_id),
        );
    }
    (
        INVALID_SNAPSHOT_ID_MALFORMED,
        snapshot_id_malformed_message(snapshot_id),
    )
}

/// Refuse a snapshot id the request named that this service cannot resolve, through the one
/// classification [`unresolvable_snapshot_id_error`] holds: HTTP 400, with that cause's own
/// code.
pub fn unresolvable_snapshot_id<E>(snapshot_id: &str) -> crate::scaffolded::Scaffolded<E> {
    let (code, message) = unresolvable_snapshot_id_error(snapshot_id);
    crate::scaffolded::Scaffolded::service_error(code, 400, message)
}

/// Refuse a create that would take the store past [`MAX_STORED_VOLUMES`]: HTTP 400
/// `VolumeLimitExceeded`, EC2's own volumes-per-Region quota code.
pub fn volume_limit_exceeded<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(VOLUME_LIMIT_EXCEEDED, 400, message)
}

// =============================================================================
// EBS volume modifications
// =============================================================================

/// A modification that has been accepted and whose change is being applied:
/// `VolumeModificationState`'s `modifying`
/// (`smithy_query(query="shape",shape="VolumeModificationState")`).
pub const VOLUME_MODIFICATION_STATE_MODIFYING: &str = "modifying";

/// A modification whose change is in place and whose volume is being brought to its new
/// performance: `VolumeModificationState`'s `optimizing`.
pub const VOLUME_MODIFICATION_STATE_OPTIMIZING: &str = "optimizing";

/// A modification that is finished: `VolumeModificationState`'s `completed`. The fourth
/// value of the catalog, `failed`, is never stored here — nothing in this emulator fails a
/// modification after it has been accepted; a change it cannot apply is refused at the
/// call that asks for it.
pub const VOLUME_MODIFICATION_STATE_COMPLETED: &str = "completed";

/// The `Progress` a finished modification reports: "The modification progress, from 0 to
/// 100 percent complete" (`VolumeModification$Progress`).
pub const VOLUME_MODIFICATION_PROGRESS_COMPLETE: i64 = 100;

/// One EBS volume's most recent modification: the modelled [`VolumeModification`] every
/// operation that reports one answers with, plus the instants the clock owes it its
/// `modifying` -> `optimizing` -> `completed` moves.
///
/// The view holds no `ModificationState`, `Progress` or `EndTime`: all three are facts of
/// the modification's age, so [`VolumeModificationRecord::view_at`] derives them rather
/// than letting two writers store two different answers — the arrangement
/// [`VolumeRecord`] uses for a volume's own `State`.
#[derive(Debug, Clone)]
pub struct VolumeModificationRecord {
    /// The modification as the model shapes it: the volume it belongs to, its original
    /// values, its target values and its `StartTime`. `ModificationState`, `Progress` and
    /// `EndTime` are left for [`Self::view_at`] to fill.
    pub view: VolumeModification,
    /// The instant the modification started, which [`Self::progress_at`] measures from.
    pub started_at: Instant,
    /// The instant from which this modification's `modifying` -> `optimizing` move is
    /// due. Monotonic ([`Instant`]), because it is only ever compared against the clock
    /// and never reported, so a wall-clock adjustment cannot move a finished
    /// modification back to `modifying`.
    pub optimizing_at: Instant,
    /// The instant from which its `optimizing` -> `completed` move is due.
    pub completed_at: Instant,
    /// The wall-clock instant the modification finishes on, which it reports as its
    /// `EndTime` once it has: the modification ended when that move fell due, not when
    /// somebody next asked about it.
    pub ends_at: SystemTime,
}

impl VolumeModificationRecord {
    /// A modification of `view` that starts now: `modifying`, due `optimizing` one
    /// [`TRANSITION_DELAY`] later and `completed` one further [`TRANSITION_DELAY`] after
    /// that.
    ///
    /// Each move falls due a span after the move before it, read off the same monotonic
    /// clock as every other clock-owed transition in this crate and recorded rather than
    /// slept for, so a caller that reads straight after the call that started the
    /// modification still observes it `modifying`.
    pub fn starting_now(view: VolumeModification) -> Self {
        let started_at = Instant::now();
        Self {
            view,
            started_at,
            optimizing_at: started_at + TRANSITION_DELAY,
            completed_at: started_at + TRANSITION_DELAY * 2,
            ends_at: SystemTime::now() + TRANSITION_DELAY * 2,
        }
    }

    /// The modification's `VolumeModificationState` at `now`.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.optimizing_at {
            VOLUME_MODIFICATION_STATE_MODIFYING
        } else if now < self.completed_at {
            VOLUME_MODIFICATION_STATE_OPTIMIZING
        } else {
            VOLUME_MODIFICATION_STATE_COMPLETED
        }
    }

    /// How far along the modification is at `now`, "from 0 to 100 percent complete"
    /// (`VolumeModification$Progress`): the share of the span between its start and its
    /// completion that the clock has passed, and [`VOLUME_MODIFICATION_PROGRESS_COMPLETE`]
    /// once it is `completed`.
    pub fn progress_at(&self, now: Instant) -> i64 {
        let whole = self.completed_at.saturating_duration_since(self.started_at);
        let elapsed = now.saturating_duration_since(self.started_at);
        if whole.is_zero() || elapsed >= whole {
            return VOLUME_MODIFICATION_PROGRESS_COMPLETE;
        }
        let percent = elapsed
            .as_nanos()
            .saturating_mul(u128::try_from(VOLUME_MODIFICATION_PROGRESS_COMPLETE).unwrap_or(100))
            / whole.as_nanos();
        i64::try_from(percent)
            .unwrap_or(VOLUME_MODIFICATION_PROGRESS_COMPLETE)
            .clamp(0, VOLUME_MODIFICATION_PROGRESS_COMPLETE)
    }

    /// This modification as of `now`: the stored view with the state, the progress and the
    /// end time the clock has reached. A read observes the moves the clock already owed
    /// without writing anything, so it cannot change what a later caller sees.
    ///
    /// `EndTime` is "the modification completion or failure time"
    /// (`VolumeModification$EndTime`), so a modification that has not finished reports
    /// none at all.
    pub fn view_at(&self, now: Instant) -> VolumeModification {
        let mut view = self.view.clone();
        view.modification_state = Some(self.state_at(now).to_string());
        view.progress = Some(self.progress_at(now));
        view.end_time = (now >= self.completed_at).then(|| system_time_to_date_time(self.ends_at));
        view
    }
}

/// Refuse a request that named an EBS volume this account holds no modification for, the
/// way EC2 does: HTTP 400 `InvalidVolumeModification.NotFound`, whose message names the
/// volume the request asked about — the one thing about the refusal that is not fixed text.
pub fn invalid_volume_modification_not_found<E>(
    volume_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VOLUME_MODIFICATION_NOT_FOUND,
        400,
        format!("Modification for volume '{volume_id}' does not exist."),
    )
}

// =============================================================================
// Secondary networks
// =============================================================================

/// The prefix of a secondary network's id.
///
/// `com.amazonaws.ec2#SecondaryNetworkId` carries no `@pattern`, so the model publishes no
/// spelling for it; the recorded wire corpus does. AWS answers
/// `secnet-00000000000000000` with `InvalidSecondaryNetworkId.Malformed` rather than with
/// the `UnknownParameter`/`InvalidParameterValue` it answers a value it cannot place at
/// all, and the SAME recorded catalog probes `afi-00000000000000000` and
/// `imgusgrpt-00000000000000000` — prefixes EC2 certainly issues — and gets each resource's
/// own `.Malformed` back too. What those probes share is the ALL-ZERO long-form suffix, not
/// an unknown prefix, so `secnet` is read as the prefix AWS issues for this resource and
/// the suffix as what makes the probe malformed (see
/// [`secondary_network_id_is_well_formed`]).
///
/// [`ComAmazonawsEc2Amazonec2State::next_id`] mints EC2's own
/// `<prefix>-<17 lowercase hex digits>` form under it, the way every other id of this store
/// is minted, so every id this store issues is one that reader accepts.
pub const SECONDARY_NETWORK_ID_PREFIX: &str = "secnet";

/// The prefix of the id of a secondary network's IPv4 CIDR block association
/// (`com.amazonaws.ec2#SecondaryNetworkCidrAssociationId`, which publishes no pattern
/// either and which no recording probes), minted the same way as
/// [`SECONDARY_NETWORK_ID_PREFIX`]: CHOSEN, spelled as that prefix extended by the
/// association it names.
pub const SECONDARY_NETWORK_CIDR_ASSOCIATION_ID_PREFIX: &str = "secnet-cidr-assoc";

/// A secondary network AWS has not finished creating: `SecondaryNetworkState`'s
/// `create-in-progress` (`smithy_query(query="shape",shape="SecondaryNetworkState")`).
pub const SECONDARY_NETWORK_STATE_CREATE_IN_PROGRESS: &str = "create-in-progress";

/// A secondary network AWS has finished creating: `SecondaryNetworkState`'s
/// `create-complete`.
pub const SECONDARY_NETWORK_STATE_CREATE_COMPLETE: &str = "create-complete";

/// A secondary network AWS has been asked to delete and has not finished deleting:
/// `SecondaryNetworkState`'s `delete-in-progress`.
pub const SECONDARY_NETWORK_STATE_DELETE_IN_PROGRESS: &str = "delete-in-progress";

/// A secondary network AWS has finished deleting: `SecondaryNetworkState`'s
/// `delete-complete`.
///
/// A network in either delete state is still REPORTED: `DescribeSecondaryNetworks`
/// documents `delete-in-progress` and `delete-complete` among the values of its `state`
/// filter (`smithy_query(query="doc",shape="DescribeSecondaryNetworksRequest",member="Filters")`),
/// which no report could ever select on if a deleted network left the account's networks.
pub const SECONDARY_NETWORK_STATE_DELETE_COMPLETE: &str = "delete-complete";

/// The IPv4 CIDR block of a secondary network that is being deleted:
/// `SecondaryNetworkCidrBlockAssociationState`'s `disassociating`.
pub const SECONDARY_NETWORK_CIDR_STATE_DISASSOCIATING: &str = "disassociating";

/// The IPv4 CIDR block of a secondary network that has finished being deleted:
/// `SecondaryNetworkCidrBlockAssociationState`'s `disassociated`.
pub const SECONDARY_NETWORK_CIDR_STATE_DISASSOCIATED: &str = "disassociated";

/// An IPv4 CIDR block whose association with a secondary network is still being made:
/// `SecondaryNetworkCidrBlockAssociationState`'s `associating`.
pub const SECONDARY_NETWORK_CIDR_STATE_ASSOCIATING: &str = "associating";

/// An IPv4 CIDR block associated with a secondary network:
/// `SecondaryNetworkCidrBlockAssociationState`'s `associated`.
pub const SECONDARY_NETWORK_CIDR_STATE_ASSOCIATED: &str = "associated";

/// The one resource type `CreateSecondaryNetwork`'s `TagSpecifications` may name: "The tags
/// to assign to the secondary network"
/// (`CreateSecondaryNetworkRequest$TagSpecifications`), spelled as the modelled
/// `ResourceType` enum spells it.
pub const SECONDARY_NETWORK_TAGGABLE_RESOURCE: &str = "secondary-network";

/// The largest number of secondary networks this in-memory store holds.
///
/// A capacity bound in the style of [`MAX_VPCS`], not an AWS quota: each network is a
/// record in this process's memory, so the number a caller can create has to be finite.
/// A count ceiling is not a memory bound on its own — see [`MAX_STORED_BYTES`] — so the
/// caller-chosen bytes of each network are charged against that budget too
/// ([`secondary_network_payload_bytes`]).
pub const MAX_SECONDARY_NETWORKS: usize = 1_000;

/// The error code a create past [`MAX_SECONDARY_NETWORKS`] is refused with.
///
/// AWS publishes no quota code for this resource, so this follows EC2's own
/// `<Resource>LimitExceeded` spelling ([`VPC_LIMIT_EXCEEDED`], [`SUBNET_LIMIT_EXCEEDED`],
/// [`SECURITY_GROUP_LIMIT_EXCEEDED`]).
pub const SECONDARY_NETWORK_LIMIT_EXCEEDED: &str = "SecondaryNetworkLimitExceeded";

/// The error code for a request naming a secondary network id the account holds no network
/// for, in EC2's `Invalid<Resource>Id.NotFound` spelling — the same spelling as
/// [`INVALID_LAUNCH_TEMPLATE_ID_NOT_FOUND`].
pub const INVALID_SECONDARY_NETWORK_ID_NOT_FOUND: &str = "InvalidSecondaryNetworkId.NotFound";

/// The error code for a request naming something that is not a secondary network id at all,
/// in EC2's `Invalid<Resource>Id.Malformed` spelling — the code AWS answers before any
/// lookup, as the recorded wire corpus behind this issue shows
/// (`secnet-00000000000000000` -> `InvalidSecondaryNetworkId.Malformed`).
pub const INVALID_SECONDARY_NETWORK_ID_MALFORMED: &str = "InvalidSecondaryNetworkId.Malformed";

/// One stored secondary network: the modelled [`SecondaryNetwork`] every operation that
/// reports one answers with, plus the instant the clock owes it its `create-in-progress`
/// -> `create-complete` move.
///
/// The view holds no `State` — neither the network's own nor that of its IPv4 CIDR block
/// associations: both are facts of the network's age, so [`SecondaryNetworkRecord::view_at`]
/// derives them rather than letting two writers store two different words for one network.
#[derive(Debug, Clone)]
pub struct SecondaryNetworkRecord {
    /// The network as the model shapes it, with the `State` members left for
    /// [`SecondaryNetworkRecord::view_at`] to fill.
    pub view: SecondaryNetwork,
    /// The instant from which this network's `create-in-progress` -> `create-complete`
    /// move is due. Monotonic ([`Instant`]), because it is only ever compared against the
    /// clock and never reported, so a wall-clock adjustment cannot move a network back to
    /// `create-in-progress`.
    pub create_complete_at: Instant,
    /// The instant from which this network's `delete-in-progress` -> `delete-complete`
    /// move is due, and `None` for a network nobody has asked to delete. Set once, by the
    /// delete that asked for it ([`ComAmazonawsEc2Amazonec2State::begin_secondary_network_deletion`]),
    /// and monotonic for the same reason as [`Self::create_complete_at`].
    pub delete_complete_at: Option<Instant>,
}

impl SecondaryNetworkRecord {
    /// This network as of `now`: the stored view with the states the clock has reached. A
    /// read observes the transition the clock already owed without writing anything.
    pub fn view_at(&self, now: Instant) -> SecondaryNetwork {
        let mut view = self.view.clone();
        view.state = Some(self.state_at(now).to_string());
        let association_state = self.cidr_association_state_at(now);
        for association in view.ipv4_cidr_block_associations.iter_mut().flatten() {
            association.state = Some(association_state.to_string());
        }
        view
    }

    /// The network's `SecondaryNetworkState` at `now`.
    ///
    /// A deletion that has been asked for is the only move left, so it takes precedence:
    /// a network asked to delete while it was still being created does not report
    /// `create-complete` once the creation's own move falls due.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if let Some(delete_complete_at) = self.delete_complete_at {
            return if now < delete_complete_at {
                SECONDARY_NETWORK_STATE_DELETE_IN_PROGRESS
            } else {
                SECONDARY_NETWORK_STATE_DELETE_COMPLETE
            };
        }
        if now < self.create_complete_at {
            SECONDARY_NETWORK_STATE_CREATE_IN_PROGRESS
        } else {
            SECONDARY_NETWORK_STATE_CREATE_COMPLETE
        }
    }

    /// The `SecondaryNetworkCidrBlockAssociationState` of this network's IPv4 CIDR block
    /// associations at `now`: a block is associated once the network it belongs to has
    /// finished being created and disassociated once it has finished being deleted, so the
    /// block's moves fall due with the network's own.
    pub fn cidr_association_state_at(&self, now: Instant) -> &'static str {
        if let Some(delete_complete_at) = self.delete_complete_at {
            return if now < delete_complete_at {
                SECONDARY_NETWORK_CIDR_STATE_DISASSOCIATING
            } else {
                SECONDARY_NETWORK_CIDR_STATE_DISASSOCIATED
            };
        }
        if now < self.create_complete_at {
            SECONDARY_NETWORK_CIDR_STATE_ASSOCIATING
        } else {
            SECONDARY_NETWORK_CIDR_STATE_ASSOCIATED
        }
    }
}

/// What one secondary network is created with: everything a writer chooses, with every id
/// and every derived member left to [`ComAmazonawsEc2Amazonec2State::insert_secondary_network`].
#[derive(Debug, Clone)]
pub struct SecondaryNetworkSpec {
    /// The network's IPv4 CIDR block, already canonical
    /// ([`canonical_secondary_network_ipv4_cidr`]).
    pub ipv4_cidr_block: String,
    /// The `SecondaryNetworkType` the request named.
    pub network_type: String,
    /// The Region the request was signed for, which the network's ARN names.
    pub region: String,
    /// The tags the request assigned to the network.
    pub tags: Vec<Tag>,
    /// When this network's `create-in-progress` -> `create-complete` move falls due.
    pub create_complete_at: Instant,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another secondary network fits inside the store's ceiling
    /// ([`MAX_SECONDARY_NETWORKS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`SECONDARY_NETWORK_LIMIT_EXCEEDED`] when it answers `false`.
    pub fn has_secondary_network_capacity(&self) -> bool {
        self.secondary_networks.len() < MAX_SECONDARY_NETWORKS
    }

    /// Insert a secondary network, and answer the `SecondaryNetworkId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the
    /// caller — the network's id, its ARN, the association id of its IPv4 CIDR block and
    /// the owning account — so two writers cannot report the same network differently.
    ///
    /// Like [`Self::insert_vpc`], the caller-chosen bytes of `spec.tags` are charged
    /// against the store's byte budget here, so a writer cannot store them uncharged; a
    /// create still asks [`Self::has_secondary_network_capacity`] and
    /// [`Self::would_exceed_stored_bytes`] first so it can refuse instead of overshooting.
    pub fn insert_secondary_network(&mut self, spec: SecondaryNetworkSpec) -> String {
        let secondary_network_id = self.next_id(SECONDARY_NETWORK_ID_PREFIX);
        let association_id = self.next_id(SECONDARY_NETWORK_CIDR_ASSOCIATION_ID_PREFIX);
        let view = SecondaryNetwork {
            secondary_network_id: Some(secondary_network_id.clone()),
            secondary_network_arn: Some(::engine::emulator_arn(
                "ec2",
                &spec.region,
                &format!("{SECONDARY_NETWORK_TAGGABLE_RESOURCE}/{secondary_network_id}"),
            )),
            owner_id: Some(::engine::emulator_account_id().to_string()),
            r#type: Some(spec.network_type),
            // Both states are derived from `create_complete_at` on every read.
            state: None,
            // "The reason for the current state" (`SecondaryNetwork$StateReason`): a
            // network that is being created as asked has nothing to explain, and no
            // operation of this emulator fails one.
            state_reason: None,
            ipv4_cidr_block_associations: Some(vec![SecondaryNetworkIpv4CidrBlockAssociation {
                association_id: Some(association_id),
                cidr_block: Some(spec.ipv4_cidr_block),
                state: None,
                state_reason: None,
            }]),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(tag_list_bytes(view.tags.as_deref().unwrap_or_default()));
        self.secondary_networks.insert(
            secondary_network_id.clone(),
            SecondaryNetworkRecord {
                view,
                create_complete_at: spec.create_complete_at,
                // Nobody has asked to delete a network that is being created.
                delete_complete_at: None,
            },
        );
        secondary_network_id
    }

    /// Start deleting the secondary network `secondary_network_id`, and answer it as the
    /// store then holds it — `None` when the account holds no such network.
    ///
    /// The deletion is a CLOCK-OWED move, like the creation: the network is
    /// `delete-in-progress` with its IPv4 CIDR block `disassociating` until
    /// `delete_complete_at`, which is stored rather than slept for, and
    /// [`SecondaryNetworkRecord::view_at`] derives both states from it on every read.
    ///
    /// The record STAYS in the store, because a network in either delete state is still
    /// reported ([`SECONDARY_NETWORK_STATE_DELETE_COMPLETE`]) — so its caller-chosen bytes
    /// stay charged to the store's budget, which still holds them, and it still counts
    /// against [`MAX_SECONDARY_NETWORKS`], which bounds the records this process holds.
    ///
    /// A network already being deleted keeps the due instant its own delete gave it: a
    /// repeat of the call reports the deletion already under way instead of restarting its
    /// clock.
    pub fn begin_secondary_network_deletion(
        &mut self,
        secondary_network_id: &str,
        now: Instant,
        delete_complete_at: Instant,
    ) -> Option<SecondaryNetwork> {
        let record = self.secondary_networks.get_mut(secondary_network_id)?;
        record.delete_complete_at.get_or_insert(delete_complete_at);
        Some(record.view_at(now))
    }
}

/// The caller-chosen bytes one stored secondary network holds: its tags and the
/// `ClientToken` the token map keeps for it. Everything else on the record — the ids, the
/// ARN, the account, the canonical CIDR block and the states — is derived by this module or
/// bounded by the model, so it is fixed-size.
///
/// The ONE definition of that charge: `CreateSecondaryNetwork` asks it before the write and
/// charges exactly it, and a removal path must give back the same number.
pub fn secondary_network_payload_bytes(tags: &[Tag], client_token: Option<&str>) -> usize {
    tag_list_bytes(tags).saturating_add(client_token.map_or(0, str::len))
}

/// Whether `id` is a well-formed secondary network id — one this service could have ISSUED.
///
/// Read by the stricter of the two generic id readers
/// ([`issued_long_id_is_well_formed`]): the long EC2 resource id form only
/// (`<prefix>-<17 lowercase hex digits>`, leading `0`, not all `0`) under this resource's
/// own prefix ([`SECONDARY_NETWORK_ID_PREFIX`], whose doc carries the recorded evidence).
/// Secondary networks are a resource EC2 only ever issued LONG ids for — there is no
/// eight-digit era behind them — and the all-zero suffix is what the recorded catalog shows
/// AWS calling malformed, under this prefix and under the `afi-` and `imgusgrpt-` prefixes
/// it probes beside it.
///
/// Anything else is [`INVALID_SECONDARY_NETWORK_ID_MALFORMED`] BEFORE any lookup, so a
/// caller can tell "not an id of this kind" from "no such network".
pub fn secondary_network_id_is_well_formed(id: &str) -> bool {
    issued_long_id_is_well_formed(SECONDARY_NETWORK_ID_PREFIX, id)
}

/// Refuse a secondary network id the request named that this service cannot resolve.
///
/// The two questions EC2 asks in order: is this an id of this resource kind at all
/// ([`secondary_network_id_is_well_formed`], answered
/// [`INVALID_SECONDARY_NETWORK_ID_MALFORMED`] before any lookup), and does the account hold
/// a network with it ([`INVALID_SECONDARY_NETWORK_ID_NOT_FOUND`])? A caller branches on the
/// code, and the two mean different things: a malformed id is a permanent client bug, while
/// a missing one is a resource that is gone.
pub fn require_known_secondary_network<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    secondary_network_id: &str,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    if !secondary_network_id_is_well_formed(secondary_network_id) {
        return Err(invalid_secondary_network_id_malformed(secondary_network_id));
    }
    if store.secondary_networks.contains_key(secondary_network_id) {
        return Ok(());
    }
    Err(invalid_secondary_network_id_not_found(secondary_network_id))
}

/// Refuse something that is not a secondary network id at all: HTTP 400
/// [`INVALID_SECONDARY_NETWORK_ID_MALFORMED`], worded the way EC2 words a malformed id of a
/// resource kind ([`invalid_ipam_internet_registry_association_id_malformed`]) and built
/// from the request's own value.
pub fn invalid_secondary_network_id_malformed<E>(
    secondary_network_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_SECONDARY_NETWORK_ID_MALFORMED,
        400,
        format!(
            "The {SECONDARY_NETWORK_TAGGABLE_RESOURCE} ID '{secondary_network_id}' is malformed"
        ),
    )
}

/// Refuse a request that named a secondary network id no network of the account has: HTTP
/// 400 [`INVALID_SECONDARY_NETWORK_ID_NOT_FOUND`].
pub fn invalid_secondary_network_id_not_found<E>(
    secondary_network_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_SECONDARY_NETWORK_ID_NOT_FOUND,
        400,
        format!("The secondary network '{secondary_network_id}' does not exist."),
    )
}

/// Refuse a create that would take the store past [`MAX_SECONDARY_NETWORKS`]: HTTP 400
/// [`SECONDARY_NETWORK_LIMIT_EXCEEDED`].
pub fn secondary_network_limit_exceeded<E>(
    message: impl Into<String>,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(SECONDARY_NETWORK_LIMIT_EXCEEDED, 400, message)
}

/// The Region this request was signed for, from its SigV4 credential scope; the
/// emulator's default Region when it carries none.
///
/// The scope is caller-controlled text that is rendered into zone names and zone ids,
/// so a value that is not shaped like a Region name is refused here and the default
/// used instead.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEc2Amazonec2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEc2Amazonec2State>)
}

/// Whether this request was made by ANOTHER emulated service acting on a caller's
/// behalf, rather than by the caller itself.
///
/// The two are told apart by where the request came from, not by anything in it: the
/// framework stamps a request a handler sent through its in-process callback with that
/// chain's depth, and a request that arrived from a client carries none. A client
/// cannot spell the marker, because it is a typed request extension of this process
/// and not anything on the wire.
///
/// What it decides here is the reserved tag namespace. "Do not use the `aws:` prefix …
/// it is reserved for AWS use" is a rule about a CALLER's request ([`RESERVED_TAG_PREFIX`]):
/// the AWS services that provision resources on a caller's behalf are what SET those
/// tags on what they provision, and a resource created for another service's resource
/// carries them.
///
/// The allowance is on where the request CAME FROM, not on which key it names: EC2 cannot
/// tell which service is calling, so it is the calling service's own job not to forward a
/// key its caller supplied — a service that takes tags from a caller and re-sends them
/// must drop the reserved ones first, as the one in this tree does.
pub fn is_cross_service_request(parts: &::http::request::Parts) -> bool {
    parts
        .extensions
        .get::<::smithy_server_runtime::CallbackRecursionDepth>()
        .is_some_and(|depth| depth.0 > 0)
}

// =============================================================================
// Verified Access trust providers
// =============================================================================

/// The id prefix a Verified Access trust provider is minted under.
///
/// EC2 names the resource's ids `vatp-<hex>`, the same `<prefix>-<hex>` form every other
/// EC2 resource id takes, so [`ComAmazonawsEc2Amazonec2State::next_id`] mints them.
pub const VERIFIED_ACCESS_TRUST_PROVIDER_ID_PREFIX: &str = "vatp";

/// The one `ResourceType` a `CreateVerifiedAccessTrustProvider` tag specification may
/// name: "The tags to assign to the Verified Access trust provider"
/// (`CreateVerifiedAccessTrustProviderRequest$TagSpecifications`), spelled as the
/// modelled `ResourceType` enum spells it
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const VERIFIED_ACCESS_TRUST_PROVIDER_TAGGABLE_RESOURCE: &str =
    "verified-access-trust-provider";

/// `TrustProviderType`'s `user` (`smithy_query(query="shape",shape="TrustProviderType")`).
pub const TRUST_PROVIDER_TYPE_USER: &str = "user";

/// `TrustProviderType`'s `device`.
pub const TRUST_PROVIDER_TYPE_DEVICE: &str = "device";

/// `UserTrustProviderType`'s `oidc`
/// (`smithy_query(query="shape",shape="UserTrustProviderType")`) — the one user-based
/// kind that is OpenID Connect, so the one `OidcOptions` describe.
pub const USER_TRUST_PROVIDER_TYPE_OIDC: &str = "oidc";

/// What a reported `OidcOptions$ClientSecret` reads as.
///
/// The OIDC client secret is WRITE-ONLY: it is the one member of that shape the model
/// marks sensitive (`smithy_query(query="shape",shape="OidcOptions")`), and a report of a
/// trust provider answers this fixed mask in its place rather than the value the request
/// configured — the same arrangement every other AWS API that takes a secret and reports
/// the resource back uses (CodePipeline's webhook `SecretToken`, which reads as `****`).
/// So the secret a caller configures is never stored and no read can report it; a trust
/// provider configured without one reports no secret at all.
pub const REDACTED_CLIENT_SECRET: &str = "REDACTED";

/// How many answered deletes this store remembers for their idempotency tokens.
///
/// A delete FREES a live record's slot, so [`MAX_VERIFIED_ACCESS_TRUST_PROVIDERS`] does not
/// bound a history that outlives the resource, and neither does [`MAX_STORED_BYTES`]: that
/// budget charges the text a caller chose the length of, not a record's fixed fields or a
/// map entry's own footprint, so a create-and-delete loop would grow this history without
/// end. It therefore keeps the most recent deletes and drops the oldest, the way every
/// other post-delete history of this emulator is bounded — a retry arrives right after the
/// call it repeats, so the entries a retry can still reach are the newest ones, and a
/// delete is never refused because the history is full.
pub const MAX_REMEMBERED_VERIFIED_ACCESS_TRUST_PROVIDER_DELETIONS: usize = 200;

/// How many Verified Access trust providers this emulator stores before
/// `CreateVerifiedAccessTrustProvider` is refused.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: nothing in this emulator
/// reclaims a stored trust provider, so the store needs a ceiling that is not the
/// process's memory. It is well above the published per-Region Verified Access quota a
/// test would exercise. A count ceiling is not a memory bound on its own — see
/// [`MAX_STORED_BYTES`] — so the caller-chosen bytes of each record are charged against
/// that budget too ([`verified_access_trust_provider_payload_bytes`]).
pub const MAX_VERIFIED_ACCESS_TRUST_PROVIDERS: usize = 1_000;

/// The largest page `DescribeVerifiedAccessTrustProviders` serves, and the page it serves
/// when the request names no size.
///
/// The top of the modelled `MaxResults` range, `range 5..200`
/// (`smithy_query(query="operation",operation="DescribeVerifiedAccessTrustProviders")`),
/// so a request that names no page size is answered with the largest page the operation
/// can be ASKED for and the `NextToken` path behaves the same either way.
pub const MAX_VERIFIED_ACCESS_TRUST_PROVIDERS_PAGE_SIZE: usize = 200;

/// EC2's code for a Verified Access trust provider id that names no trust provider of the
/// account.
///
/// EC2's model declares no error shape at all (see [`crate::scaffolded::ServiceError`]),
/// and the service answers `Invalid<Resource>Id.NotFound` for an id of a resource it
/// cannot resolve — `InvalidVolume.NotFound`, `InvalidLaunchTemplateId.NotFound` — so
/// that is the code this resource's id gets.
pub const INVALID_VERIFIED_ACCESS_TRUST_PROVIDER_ID_NOT_FOUND: &str =
    "InvalidVerifiedAccessTrustProviderId.NotFound";

/// EC2's quota code for one Verified Access trust provider too many, named the way every
/// other per-resource EC2 quota code is (`VolumeLimitExceeded`,
/// `LaunchTemplateLimitExceeded`).
pub const VERIFIED_ACCESS_TRUST_PROVIDER_LIMIT_EXCEEDED: &str =
    "VerifiedAccessTrustProviderLimitExceeded";

/// The caller-chosen bytes one stored trust provider holds for the process's lifetime:
/// every string the REQUEST chose the length of, plus the client token the token map
/// keeps.
///
/// The ids, the timestamps and the trust-provider kinds are the service's own or come from
/// a modelled enum, so they are bounded already and not counted here. One definition of
/// the charge, so a writer cannot store bytes the store's budget never saw.
pub fn verified_access_trust_provider_payload_bytes(
    provider: &crate::ops::types::VerifiedAccessTrustProvider,
    client_token: Option<&str>,
) -> usize {
    let text = |value: &Option<String>| value.as_ref().map_or(0, String::len);
    let mut bytes = tag_list_bytes(provider.tags.as_deref().unwrap_or_default())
        .saturating_add(text(&provider.description))
        .saturating_add(text(&provider.policy_reference_name))
        .saturating_add(client_token.map_or(0, str::len));
    if let Some(options) = provider.oidc_options.as_ref() {
        bytes = bytes
            .saturating_add(text(&options.issuer))
            .saturating_add(text(&options.authorization_endpoint))
            .saturating_add(text(&options.token_endpoint))
            .saturating_add(text(&options.user_info_endpoint))
            .saturating_add(text(&options.client_id))
            .saturating_add(text(&options.client_secret))
            .saturating_add(text(&options.scope));
    }
    if let Some(options) = provider.device_options.as_ref() {
        bytes = bytes
            .saturating_add(text(&options.tenant_id))
            .saturating_add(text(&options.public_signing_key_url));
    }
    if let Some(options) = provider.native_application_oidc_options.as_ref() {
        bytes = bytes
            .saturating_add(text(&options.public_signing_key_endpoint))
            .saturating_add(text(&options.issuer))
            .saturating_add(text(&options.authorization_endpoint))
            .saturating_add(text(&options.token_endpoint))
            .saturating_add(text(&options.user_info_endpoint))
            .saturating_add(text(&options.client_id))
            .saturating_add(text(&options.scope));
    }
    if let Some(sse) = provider.sse_specification.as_ref() {
        bytes = bytes.saturating_add(text(&sse.kms_key_arn));
    }
    bytes
}

/// Refuse a request that named a Verified Access trust provider id this account holds no
/// trust provider for: HTTP 400
/// [`INVALID_VERIFIED_ACCESS_TRUST_PROVIDER_ID_NOT_FOUND`].
///
/// One code covers an id of any shape: an id this service never minted names no trust
/// provider of the account whether or not it reads like one. The message names the id the
/// REQUEST gave, in the wording the real service answers this resource with
/// ("VerifiedAccessTrustProvider <id> does not exist"), so a client reading the text sees
/// which id it was.
pub fn invalid_verified_access_trust_provider_id_not_found<E>(
    trust_provider_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VERIFIED_ACCESS_TRUST_PROVIDER_ID_NOT_FOUND,
        400,
        format!("VerifiedAccessTrustProvider {trust_provider_id} does not exist"),
    )
}

/// Refuse a create that would take the store past
/// [`MAX_VERIFIED_ACCESS_TRUST_PROVIDERS`]: HTTP 400
/// [`VERIFIED_ACCESS_TRUST_PROVIDER_LIMIT_EXCEEDED`].
pub fn verified_access_trust_provider_limit_exceeded<E>(
    message: impl Into<String>,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        VERIFIED_ACCESS_TRUST_PROVIDER_LIMIT_EXCEEDED,
        400,
        message,
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Verified Access trust provider fits inside the store's ceiling
    /// ([`MAX_VERIFIED_ACCESS_TRUST_PROVIDERS`]).
    ///
    /// The same contract as [`Self::has_vpc_capacity`]: a create asks first and refuses
    /// with [`verified_access_trust_provider_limit_exceeded`] when it answers `false`.
    pub fn has_verified_access_trust_provider_capacity(&self) -> bool {
        self.verified_access_trust_providers.len() < MAX_VERIFIED_ACCESS_TRUST_PROVIDERS
    }

    /// Store `provider` under a freshly minted `VerifiedAccessTrustProviderId`, charge the
    /// caller-chosen bytes it and `client_token` retain against the store's byte budget,
    /// and answer the id it was given.
    ///
    /// The id is minted HERE rather than by the writer, so a trust provider never shares
    /// an id with another resource of this store, and the record is written with that id on
    /// it so no reader has to learn the key twice. The charge happens at the insert the way
    /// [`Self::insert_vpc`] charges a VPC's tags, so no path can store those bytes
    /// uncharged; a create still asks
    /// [`Self::has_verified_access_trust_provider_capacity`] and
    /// [`Self::would_exceed_stored_bytes`] first so it can refuse instead of overshooting.
    pub fn insert_verified_access_trust_provider(
        &mut self,
        mut provider: crate::ops::types::VerifiedAccessTrustProvider,
        client_token: Option<&str>,
    ) -> String {
        let trust_provider_id = self.next_id(VERIFIED_ACCESS_TRUST_PROVIDER_ID_PREFIX);
        provider.verified_access_trust_provider_id = Some(trust_provider_id.clone());
        self.add_stored_bytes(verified_access_trust_provider_payload_bytes(
            &provider,
            client_token,
        ));
        if let Some(token) = client_token {
            self.verified_access_trust_provider_client_tokens
                .insert(token.to_string(), trust_provider_id.clone());
        }
        self.verified_access_trust_providers
            .insert(trust_provider_id.clone(), provider);
        trust_provider_id
    }

    /// Remove the Verified Access trust provider `trust_provider_id` and answer it as it
    /// was last reported, or `None` when the store holds no such trust provider.
    ///
    /// The `CreateVerifiedAccessTrustProvider` token that minted it goes with it — a token
    /// must never answer with a trust provider that is no longer stored — and the bytes the
    /// record held go back to the store's budget on the terms
    /// [`verified_access_trust_provider_payload_bytes`] took them on.
    ///
    /// `client_token` is the DELETE's own token: when the caller gave one, the removed
    /// record is kept under it ([`Self::verified_access_trust_provider_deletions`]) so a
    /// retry of that delete answers what the first one answered, and the bytes it keeps are
    /// charged again under that token. That history is bounded by
    /// [`MAX_REMEMBERED_VERIFIED_ACCESS_TRUST_PROVIDER_DELETIONS`] entries, the oldest
    /// dropped (and its bytes given back) to make room, so a create-and-delete loop cannot
    /// grow it; a delete that names no token frees the record outright.
    ///
    /// An instance that had this trust provider ATTACHED loses that attachment here
    /// ([`Self::resync_verified_access_trust_provider_attachments`]): the attachment is a
    /// reference to a trust provider, so it cannot outlive the trust provider it names.
    pub fn remove_verified_access_trust_provider(
        &mut self,
        trust_provider_id: &str,
        client_token: Option<&str>,
    ) -> Option<crate::ops::types::VerifiedAccessTrustProvider> {
        let provider = self
            .verified_access_trust_providers
            .remove(trust_provider_id)?;
        let minting_token = self
            .verified_access_trust_provider_client_tokens
            .iter()
            .find(|(_, minted)| minted.as_str() == trust_provider_id)
            .map(|(token, _)| token.clone());
        if let Some(token) = &minting_token {
            self.verified_access_trust_provider_client_tokens.remove(token);
        }
        self.release_stored_bytes(verified_access_trust_provider_payload_bytes(
            &provider,
            minting_token.as_deref(),
        ));
        if let Some(token) = client_token {
            // One token names ONE delete, so a token used again replaces what it named
            // rather than leaving the history two entries to answer with.
            if let Some(position) = self
                .verified_access_trust_provider_deletions
                .iter()
                .position(|(remembered, _)| remembered == token)
            {
                self.forget_verified_access_trust_provider_deletion(position);
            }
            self.add_stored_bytes(verified_access_trust_provider_payload_bytes(
                &provider,
                Some(token),
            ));
            self.verified_access_trust_provider_deletions
                .push_back((token.to_string(), provider.clone()));
            while self.verified_access_trust_provider_deletions.len()
                > MAX_REMEMBERED_VERIFIED_ACCESS_TRUST_PROVIDER_DELETIONS
            {
                self.forget_verified_access_trust_provider_deletion(0);
            }
        }
        self.resync_verified_access_trust_provider_attachments(
            trust_provider_id,
            SystemTime::now(),
        );
        Some(provider)
    }

    /// The trust provider the delete that carried `client_token` removed, while this store
    /// still remembers that delete.
    ///
    /// The history is bounded
    /// ([`MAX_REMEMBERED_VERIFIED_ACCESS_TRUST_PROVIDER_DELETIONS`]), so a token whose
    /// delete has been dropped answers `None` — the same answer as a token no delete of this
    /// store ever carried.
    pub fn verified_access_trust_provider_deletion(
        &self,
        client_token: &str,
    ) -> Option<&crate::ops::types::VerifiedAccessTrustProvider> {
        self.verified_access_trust_provider_deletions
            .iter()
            .find(|(remembered, _)| remembered == client_token)
            .map(|(_, provider)| provider)
    }

    /// Drop the remembered deletion at `position`, giving the bytes it held back to the
    /// store's budget: the record leaves the history on the same terms it entered it on.
    pub(super) fn forget_verified_access_trust_provider_deletion(&mut self, position: usize) {
        if let Some((token, provider)) = self
            .verified_access_trust_provider_deletions
            .remove(position)
        {
            self.release_stored_bytes(verified_access_trust_provider_payload_bytes(
                &provider,
                Some(&token),
            ));
        }
    }
}

// =============================================================================
// Verified Access instances, groups and endpoints
// =============================================================================

/// The id prefix a Verified Access instance is minted under: EC2 names an instance's ids
/// `vai-<hex>`, the same `<prefix>-<hex>` form every other EC2 resource id takes, so
/// [`ComAmazonawsEc2Amazonec2State::next_id`] mints them.
pub const VERIFIED_ACCESS_INSTANCE_ID_PREFIX: &str = "vai";

/// The one `ResourceType` a `CreateVerifiedAccessInstance` tag specification may name: "The
/// tags to assign to the Verified Access instance"
/// (`CreateVerifiedAccessInstanceRequest$TagSpecifications`), spelled as the modelled
/// `ResourceType` enum spells it (`smithy_query(query="shape",shape="ResourceType")`).
pub const VERIFIED_ACCESS_INSTANCE_TAGGABLE_RESOURCE: &str = "verified-access-instance";

/// How many Verified Access instances this emulator stores before
/// `CreateVerifiedAccessInstance` is refused.
///
/// CHOSEN, like [`MAX_VERIFIED_ACCESS_GROUPS`] and for the same reason: nothing in this
/// emulator reclaims a stored instance, so the store needs a ceiling that is not the
/// process's memory. Well above the published per-Region Verified Access quota a test would
/// exercise. A count ceiling is not a memory bound on its own — see [`MAX_STORED_BYTES`] —
/// so the caller-chosen bytes of each record are charged against that budget too
/// ([`verified_access_instance_payload_bytes`]).
pub const MAX_VERIFIED_ACCESS_INSTANCES: usize = 1_000;

/// EC2's quota code for one Verified Access instance too many, named the way every other
/// per-resource EC2 quota code is (`VerifiedAccessGroupLimitExceeded`,
/// `VerifiedAccessEndpointLimitExceeded`).
pub const VERIFIED_ACCESS_INSTANCE_LIMIT_EXCEEDED: &str = "VerifiedAccessInstanceLimitExceeded";

/// How many answered `DeleteVerifiedAccessInstance` calls this store remembers for their
/// idempotency tokens.
///
/// The same terms as [`MAX_REMEMBERED_VERIFIED_ACCESS_GROUP_DELETIONS`], and for the same
/// reason: a delete FREES a live record's slot, so [`MAX_VERIFIED_ACCESS_INSTANCES`] does not
/// bound a history that outlives the resource. The newest deletes are kept and the oldest
/// dropped, so a create-and-delete loop cannot grow the history without end and a delete is
/// never refused because it is full.
pub const MAX_REMEMBERED_VERIFIED_ACCESS_INSTANCE_DELETIONS: usize = 200;

/// The id prefix a Verified Access group is minted under: EC2 names a group's ids
/// `vagr-<hex>`, the same `<prefix>-<hex>` form every other EC2 resource id takes, so
/// [`ComAmazonawsEc2Amazonec2State::next_id`] mints them.
pub const VERIFIED_ACCESS_GROUP_ID_PREFIX: &str = "vagr";

/// The one `ResourceType` a `CreateVerifiedAccessGroup` tag specification may name: "The
/// tags to assign to the Verified Access group"
/// (`CreateVerifiedAccessGroupRequest$TagSpecifications`), spelled as the modelled
/// `ResourceType` enum spells it (`smithy_query(query="shape",shape="ResourceType")`).
pub const VERIFIED_ACCESS_GROUP_TAGGABLE_RESOURCE: &str = "verified-access-group";

/// How many Verified Access groups this emulator stores before `CreateVerifiedAccessGroup`
/// is refused.
///
/// CHOSEN, like [`MAX_VERIFIED_ACCESS_ENDPOINTS`] and for the same reason: nothing in this
/// emulator reclaims a stored group, so the store needs a ceiling that is not the process's
/// memory. Well above the published per-Region Verified Access quota a test would exercise.
/// A count ceiling is not a memory bound on its own — see [`MAX_STORED_BYTES`] — so the
/// caller-chosen bytes of each record are charged against that budget too
/// ([`verified_access_group_payload_bytes`]).
pub const MAX_VERIFIED_ACCESS_GROUPS: usize = 1_000;

/// How many answered `DeleteVerifiedAccessGroup` calls this store remembers for their
/// idempotency tokens.
///
/// The same terms as [`MAX_REMEMBERED_VERIFIED_ACCESS_TRUST_PROVIDER_DELETIONS`], and for
/// the same reason: a delete FREES a live record's slot, so [`MAX_VERIFIED_ACCESS_GROUPS`]
/// does not bound a history that outlives the resource. The newest deletes are kept and the
/// oldest dropped, so a create-and-delete loop cannot grow the history without end and a
/// delete is never refused because it is full.
pub const MAX_REMEMBERED_VERIFIED_ACCESS_GROUP_DELETIONS: usize = 200;

/// EC2's code for a Verified Access instance id that names no instance of the account,
/// named the way [`INVALID_VERIFIED_ACCESS_GROUP_ID_NOT_FOUND`] is.
pub const INVALID_VERIFIED_ACCESS_INSTANCE_ID_NOT_FOUND: &str =
    "InvalidVerifiedAccessInstanceId.NotFound";

/// EC2's quota code for one Verified Access group too many, named the way every other
/// per-resource EC2 quota code is (`VolumeLimitExceeded`,
/// `VerifiedAccessEndpointLimitExceeded`).
pub const VERIFIED_ACCESS_GROUP_LIMIT_EXCEEDED: &str = "VerifiedAccessGroupLimitExceeded";

/// The id prefix a Verified Access endpoint is minted under: `vae-<hex>`, beside the
/// group's [`VERIFIED_ACCESS_GROUP_ID_PREFIX`] and the trust provider's
/// [`VERIFIED_ACCESS_TRUST_PROVIDER_ID_PREFIX`].
pub const VERIFIED_ACCESS_ENDPOINT_ID_PREFIX: &str = "vae";

/// The one `ResourceType` a `CreateVerifiedAccessEndpoint` tag specification may name:
/// "The tags to assign to the Verified Access endpoint"
/// (`CreateVerifiedAccessEndpointRequest$TagSpecifications`), spelled as the modelled
/// `ResourceType` enum spells it (`smithy_query(query="shape",shape="ResourceType")`).
pub const VERIFIED_ACCESS_ENDPOINT_TAGGABLE_RESOURCE: &str = "verified-access-endpoint";

/// `VerifiedAccessEndpointType`'s `load-balancer`
/// (`smithy_query(query="shape",shape="VerifiedAccessEndpointType")`) — the kind whose
/// options are `LoadBalancerOptions`.
pub const VERIFIED_ACCESS_ENDPOINT_TYPE_LOAD_BALANCER: &str = "load-balancer";

/// `VerifiedAccessEndpointType`'s `network-interface` — the kind whose options are
/// `NetworkInterfaceOptions`.
pub const VERIFIED_ACCESS_ENDPOINT_TYPE_NETWORK_INTERFACE: &str = "network-interface";

/// `VerifiedAccessEndpointType`'s `rds` — the kind whose options are `RdsOptions`.
pub const VERIFIED_ACCESS_ENDPOINT_TYPE_RDS: &str = "rds";

/// `VerifiedAccessEndpointType`'s `cidr` — the kind whose options are `CidrOptions`.
pub const VERIFIED_ACCESS_ENDPOINT_TYPE_CIDR: &str = "cidr";

/// `VerifiedAccessEndpointAttachmentType`'s only member, `vpc`
/// (`smithy_query(query="shape",shape="VerifiedAccessEndpointAttachmentType")`) — the
/// attachment that makes `SecurityGroupIds` required ("Required if AttachmentType is set to
/// vpc", `CreateVerifiedAccessEndpointRequest$SecurityGroupIds`).
pub const VERIFIED_ACCESS_ENDPOINT_ATTACHMENT_TYPE_VPC: &str = "vpc";

/// The status `CreateVerifiedAccessEndpoint` answers a fresh endpoint with: the
/// `VerifiedAccessEndpointStatusCode` enum's `pending` member
/// (`smithy_query(query="shape",shape="VerifiedAccessEndpointStatusCode")`), which is not
/// terminal — so the clock owes the endpoint [`VERIFIED_ACCESS_ENDPOINT_STATUS_ACTIVE`] one
/// [`TRANSITION_DELAY`] later.
pub const VERIFIED_ACCESS_ENDPOINT_STATUS_PENDING: &str = "pending";

/// The status a `pending` endpoint reaches once its move falls due: the
/// `VerifiedAccessEndpointStatusCode` enum's `active` member.
pub const VERIFIED_ACCESS_ENDPOINT_STATUS_ACTIVE: &str = "active";

/// How many Verified Access endpoints this emulator stores before
/// `CreateVerifiedAccessEndpoint` is refused.
///
/// CHOSEN, like [`MAX_VERIFIED_ACCESS_TRUST_PROVIDERS`] and for the same reason: nothing in
/// this emulator reclaims a stored endpoint, so the store needs a ceiling that is not the
/// process's memory. Well above the published per-Region Verified Access quota a test would
/// exercise. A count ceiling is not a memory bound on its own — see [`MAX_STORED_BYTES`] —
/// so the caller-chosen bytes of each record are charged against that budget too
/// ([`verified_access_endpoint_payload_bytes`]).
pub const MAX_VERIFIED_ACCESS_ENDPOINTS: usize = 1_000;

/// The largest page `DescribeVerifiedAccessEndpoints` serves, and the page it serves when
/// the request names no size.
///
/// The top of the modelled `MaxResults` range, `range 5..1000`
/// (`smithy_query(query="operation",operation="DescribeVerifiedAccessEndpoints")`), so a
/// request that names no page size is answered with the largest page the operation can be
/// ASKED for and the `NextToken` path behaves the same either way.
pub const MAX_VERIFIED_ACCESS_ENDPOINTS_PAGE_SIZE: usize = 1000;

/// EC2's code for a Verified Access group id that names no group of the account.
///
/// EC2's model declares no error shape at all (see [`crate::scaffolded::ServiceError`]), and
/// the service answers `Invalid<Resource>Id.NotFound` for an id of a resource it cannot
/// resolve — `InvalidVolume.NotFound`, `InvalidLaunchTemplateId.NotFound` — so that is the
/// code this resource's id gets.
pub const INVALID_VERIFIED_ACCESS_GROUP_ID_NOT_FOUND: &str = "InvalidVerifiedAccessGroupId.NotFound";

/// EC2's code for a Verified Access endpoint id that names no endpoint of the account,
/// named the same way as [`INVALID_VERIFIED_ACCESS_GROUP_ID_NOT_FOUND`].
pub const INVALID_VERIFIED_ACCESS_ENDPOINT_ID_NOT_FOUND: &str =
    "InvalidVerifiedAccessEndpointId.NotFound";

/// EC2's quota code for one Verified Access endpoint too many, named the way every other
/// per-resource EC2 quota code is (`VolumeLimitExceeded`, `LaunchTemplateLimitExceeded`).
pub const VERIFIED_ACCESS_ENDPOINT_LIMIT_EXCEEDED: &str = "VerifiedAccessEndpointLimitExceeded";

/// How many `PortRanges` entries one Verified Access endpoint's options block may carry.
///
/// CHOSEN, like [`MAX_SECURITY_GROUPS_PER_INSTANCE`] and for the same reason: the list's
/// length is the CALLER's, the modelled
/// `CreateVerifiedAccessEndpointPortRangeList` declares no `@length`
/// (`smithy_query(query="shape",shape="CreateVerifiedAccessEndpointPortRangeList")`), and
/// every entry is retained for the process's lifetime and cloned into each describe page. So
/// the count is bounded here, well above any endpoint a test or a real application
/// configures, and the entries are charged against [`MAX_STORED_BYTES`] as well
/// ([`verified_access_endpoint_payload_bytes`]), so neither the record count nor the byte
/// budget can be sidestepped through this list.
pub const MAX_VERIFIED_ACCESS_ENDPOINT_PORT_RANGES: usize = 64;

/// The ARN of a Verified Access group:
/// `arn:aws:ec2:<region>:<account>:verified-access-group/<id>`, the form
/// `VerifiedAccessGroup$VerifiedAccessGroupArn` carries ("The ARN of the Verified Access
/// group"), built from the same [`::engine::emulator_arn`] every other ARN of this store is.
pub fn verified_access_group_arn(region: &str, group_id: &str) -> String {
    ::engine::emulator_arn("ec2", region, &format!("verified-access-group/{group_id}"))
}

/// The DNS suffix a Verified Access endpoint's generated name carries.
///
/// CHOSEN: "A DNS name that is generated for the endpoint"
/// (`VerifiedAccessEndpoint$EndpointDomain`) is all the model says about the name's form, and
/// the real service's name also carries an opaque edge label nothing in the API reports. What
/// a caller can rely on is what [`verified_access_endpoint_domain`] gives it: a name that
/// carries the prefix the request asked for, is unique to the endpoint (it carries the minted
/// endpoint id), names the Region the endpoint was created in, and does not change between
/// reads.
pub const VERIFIED_ACCESS_ENDPOINT_DOMAIN_SUFFIX: &str = "prod.verified-access";

/// "A DNS name that is generated for the endpoint"
/// (`VerifiedAccessEndpoint$EndpointDomain`), for the endpoint `endpoint_id` in `region`.
///
/// `prefix` is "A custom identifier that is prepended to the DNS name that is generated for
/// the endpoint" (`CreateVerifiedAccessEndpointRequest$EndpointDomainPrefix`) — prepended
/// when the request named one, and left off the name when it did not. The rest is derived
/// from the endpoint's own id, the Region and the DNS suffix of that Region's PARTITION
/// ([`partition_dns_suffix`], the one definition of that suffix in this module, so a Region
/// outside the commercial partition gets a name in its own), making the name unique per
/// endpoint and stable across reads; see [`VERIFIED_ACCESS_ENDPOINT_DOMAIN_SUFFIX`].
pub fn verified_access_endpoint_domain(
    prefix: Option<&str>,
    endpoint_id: &str,
    region: &str,
) -> String {
    let tail = format!(
        "{endpoint_id}.{VERIFIED_ACCESS_ENDPOINT_DOMAIN_SUFFIX}.{region}.{}",
        partition_dns_suffix(region)
    );
    match prefix.filter(|prefix| !prefix.is_empty()) {
        Some(prefix) => format!("{prefix}.{tail}"),
        None => tail,
    }
}

/// One stored Verified Access endpoint: the modelled shape every operation that reports one
/// answers with, plus the two facts that shape carries no member for.
#[derive(Debug, Clone)]
pub struct VerifiedAccessEndpointRecord {
    /// The endpoint as `CreateVerifiedAccessEndpoint` answered it and
    /// `DescribeVerifiedAccessEndpoints` reports it.
    pub view: crate::ops::types::VerifiedAccessEndpoint,
    /// The `pending` -> `active` move the clock owes the endpoint, until
    /// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] applies it. `None` on
    /// an endpoint that has nothing owed.
    pub pending: Option<ClockTransition<&'static str>>,
    /// "The Verified Access policy document"
    /// (`CreateVerifiedAccessEndpointRequest$PolicyDocument`), which the modelled endpoint
    /// shape carries no member for: `GetVerifiedAccessEndpointPolicy` is what reports it
    /// (still a scaffold stub with its own open issue). Kept here so the document the create
    /// accepted is the one that reader will answer with, rather than dropped on the floor.
    pub policy_document: Option<String>,
}

/// One stored Verified Access group: the modelled shape every operation that reports one
/// answers with, plus the one fact that shape carries no member for.
#[derive(Debug, Clone, Default)]
pub struct VerifiedAccessGroupRecord {
    /// The group as `CreateVerifiedAccessGroup` answered it and a describe of the groups
    /// reports it.
    pub view: crate::ops::types::VerifiedAccessGroup,
    /// "The Verified Access policy document"
    /// (`CreateVerifiedAccessGroupRequest$PolicyDocument`), which the modelled group shape
    /// carries no member for: `GetVerifiedAccessGroupPolicy` is what reports it (still a
    /// scaffold stub with its own open issue). Kept here so the document the create accepted
    /// is the one that reader will answer with, rather than dropped on the floor.
    pub policy_document: Option<String>,
}

/// The caller-chosen bytes one stored Verified Access group holds for the process's
/// lifetime: its tags, its description, the key ARN its SSE block names, the policy document
/// beside the view and the client token the token map keeps. The id, the ARN and the owning
/// account are derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_verified_access_group`], and the timestamps are
/// this module's own, so they are bounded already and not counted here.
///
/// ONE definition of the charge, computed identically by the create's ceiling pre-check and
/// by that insert, so a writer cannot store bytes the store's budget never saw.
pub fn verified_access_group_payload_bytes(
    record: &VerifiedAccessGroupRecord,
    client_token: Option<&str>,
) -> usize {
    let text = |value: &Option<String>| value.as_ref().map_or(0, String::len);
    let group = &record.view;
    tag_list_bytes(group.tags.as_deref().unwrap_or_default())
        .saturating_add(text(&group.description))
        .saturating_add(
            group
                .sse_specification
                .as_ref()
                .map_or(0, |sse| text(&sse.kms_key_arn)),
        )
        .saturating_add(text(&record.policy_document))
        .saturating_add(client_token.map_or(0, str::len))
}

/// The caller-chosen bytes one stored Verified Access instance holds for the process's
/// lifetime: its tags, its description, the custom subdomain it was given, the description of
/// each trust provider attached to it (the condensed entry carries a copy of it) and the
/// client token the token map keeps. The id, the timestamps and the trust provider kinds are
/// the service's own or come from a modelled enum, so they are bounded already and not counted
/// here.
///
/// ONE definition of the charge, computed identically by the create's ceiling pre-check and
/// by [`ComAmazonawsEc2Amazonec2State::insert_verified_access_instance`], so a writer cannot
/// store bytes the store's budget never saw.
pub fn verified_access_instance_payload_bytes(
    instance: &crate::ops::types::VerifiedAccessInstance,
    client_token: Option<&str>,
) -> usize {
    let text = |value: &Option<String>| value.as_ref().map_or(0, String::len);
    tag_list_bytes(instance.tags.as_deref().unwrap_or_default())
        .saturating_add(text(&instance.description))
        .saturating_add(
            instance
                .cidr_endpoints_custom_sub_domain
                .as_ref()
                .map_or(0, |subdomain| text(&subdomain.sub_domain)),
        )
        .saturating_add(
            instance
                .verified_access_trust_providers
                .iter()
                .flatten()
                .map(|attached| text(&attached.description))
                .fold(0usize, |total, bytes| total.saturating_add(bytes)),
        )
        .saturating_add(client_token.map_or(0, str::len))
}

/// One `ModifyVerifiedAccessInstance` request, as the token it was given with recorded it:
/// the instance it modified and the members it set.
///
/// "A unique, case-sensitive token that you provide to ensure idempotency of your
/// modification request" (`ModifyVerifiedAccessInstanceRequest$ClientToken`) — so a repeat of
/// one request applies nothing further, while the SAME token carrying a DIFFERENT request is
/// the mismatch EC2 refuses ([`idempotent_parameter_mismatch`]) rather than a second
/// modification. Telling those two apart needs the REQUEST, which is why a recorded
/// modification carries the members it set and not just the instance it named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAccessInstanceModification {
    /// The instance the request named.
    pub verified_access_instance_id: String,
    /// "A description for the Verified Access instance"
    /// (`ModifyVerifiedAccessInstanceRequest$Description`), or `None` when the request
    /// carried none and so left the stored description as it was.
    pub description: Option<String>,
    /// "The custom subdomain"
    /// (`ModifyVerifiedAccessInstanceRequest$CidrEndpointsCustomSubDomain`), or `None` when
    /// the request carried none and so left the stored subdomain as it was.
    pub cidr_endpoints_custom_sub_domain: Option<String>,
}

impl VerifiedAccessInstanceModification {
    /// `instance` as this modification leaves it: every member the request carried replaces
    /// the stored value, and every member it did not carry keeps it.
    ///
    /// The ONE definition of what a modification does to an instance, so the pre-check of the
    /// store's byte budget and
    /// [`ComAmazonawsEc2Amazonec2State::modify_verified_access_instance`]'s write cannot
    /// disagree about the record the call leaves behind. `LastUpdatedTime` is NOT set here:
    /// the instant a modification was applied is the store's to stamp, not a figure a
    /// pre-check may invent.
    pub fn applied_to(
        &self,
        instance: &crate::ops::types::VerifiedAccessInstance,
    ) -> crate::ops::types::VerifiedAccessInstance {
        let mut applied = instance.clone();
        if let Some(description) = self.description.clone() {
            applied.description = Some(description);
        }
        if let Some(sub_domain) = self.cidr_endpoints_custom_sub_domain.clone() {
            applied.cidr_endpoints_custom_sub_domain =
                Some(crate::ops::types::VerifiedAccessInstanceCustomSubDomain {
                    sub_domain: Some(sub_domain),
                    // "The name servers"
                    // (`VerifiedAccessInstanceCustomSubDomain$Nameservers`) are the servers of
                    // the zone the subdomain is delegated to. This emulator delegates no zone
                    // and answers no DNS query, so that set is EMPTY — exactly as a create
                    // leaves it.
                    nameservers: Some(Vec::new()),
                });
        }
        applied
    }
}

/// The caller-chosen bytes one recorded Verified Access instance modification holds for the
/// process's lifetime: the members the request set and the token the map is keyed by, over a
/// fixed floor for the entry itself. The instance id is the service's own, so it is bounded
/// already and not counted here.
///
/// The FLOOR is what lets [`MAX_STORED_BYTES`] bound how many modifications can be recorded:
/// a client token is the caller's and may be a single byte, nothing reclaims a recorded
/// modification, and an instance may be modified any number of times, so this map has no
/// record-count ceiling the way a create's store has. Without the floor a stream of
/// one-character tokens would grow this process's memory while charging almost nothing
/// against the budget.
pub fn verified_access_instance_modification_payload_bytes(
    modification: &VerifiedAccessInstanceModification,
    client_token: &str,
) -> usize {
    let text = |value: &Option<String>| value.as_ref().map_or(0, String::len);
    ::std::mem::size_of::<VerifiedAccessInstanceModification>()
        .saturating_add(text(&modification.description))
        .saturating_add(text(&modification.cidr_endpoints_custom_sub_domain))
        .saturating_add(client_token.len())
}

/// The logging configuration a Verified Access instance carries until a
/// `ModifyVerifiedAccessInstanceLoggingConfiguration` sets one: no destination enabled, and
/// no trust data in the logs.
///
/// The ONE definition of that configuration — the describe of the logging configurations
/// reports it for an instance with no stored entry, and the modify applies the members a
/// request carries to it — so an instance nothing has configured cannot be reported two ways.
/// It carries no `LogVersion`, because nothing has chosen a log schema for this instance
/// ("The log version", `VerifiedAccessLogs$LogVersion`), and no `DeliveryStatus` on any
/// destination, because nothing has delivered a log record whose delivery there would be a
/// status of ("The delivery status for access logs",
/// `VerifiedAccessLogCloudWatchLogsDestination$DeliveryStatus`).
pub fn default_verified_access_logs() -> crate::ops::types::VerifiedAccessLogs {
    crate::ops::types::VerifiedAccessLogs {
        s3: Some(crate::ops::types::VerifiedAccessLogS3Destination {
            enabled: Some(false),
            ..Default::default()
        }),
        cloud_watch_logs: Some(crate::ops::types::VerifiedAccessLogCloudWatchLogsDestination {
            enabled: Some(false),
            ..Default::default()
        }),
        kinesis_data_firehose: Some(
            crate::ops::types::VerifiedAccessLogKinesisDataFirehoseDestination {
                enabled: Some(false),
                ..Default::default()
            },
        ),
        log_version: None,
        include_trust_context: Some(false),
    }
}

/// One `ModifyVerifiedAccessInstanceLoggingConfiguration` request: the instance it names and
/// the logging options it carried.
///
/// Kept whole, rather than reduced to the configuration it produced, for the reason
/// [`VerifiedAccessInstanceModification`] is: telling a REPLAY of one request from the SAME
/// token carrying a DIFFERENT one ([`idempotent_parameter_mismatch`]) needs the request.
#[derive(Debug, Clone)]
pub struct VerifiedAccessInstanceLoggingConfigurationUpdate {
    /// The instance the request named.
    pub verified_access_instance_id: String,
    /// "The configuration options for Verified Access instances"
    /// (`ModifyVerifiedAccessInstanceLoggingConfigurationRequest$AccessLogs`), as the
    /// request carried them.
    pub access_logs: crate::ops::types::VerifiedAccessLogOptions,
}

/// One S3 logging destination's options as a comparable value: whether logging to it is
/// enabled, and the bucket, prefix and owner the request named for it.
///
/// Named rather than written inline so
/// [`VerifiedAccessInstanceLoggingConfigurationUpdate::same_request_as`] compares the
/// destination's members as ONE value, the modelled options shapes deriving no equality of
/// their own.
pub(super) type S3LogDestinationComparison<'a> = (bool, Option<&'a str>, Option<&'a str>, Option<&'a str>);

/// One CloudWatch Logs or Kinesis Data Firehose destination's options as a comparable value:
/// whether logging to it is enabled, and the log group or delivery stream it names.
pub(super) type NamedLogDestinationComparison<'a> = (bool, Option<&'a str>);

impl VerifiedAccessInstanceLoggingConfigurationUpdate {
    /// `logs` as this update leaves them: every destination block and option the request
    /// carried replaces the stored one, and every one it did not carry keeps its value — the
    /// same rule [`VerifiedAccessInstanceModification::applied_to`] reads a modify's members
    /// by.
    ///
    /// A destination block the request carries is the WHOLE configuration of that
    /// destination, so a block naming no bucket leaves that destination with none. No
    /// `DeliveryStatus` is set: nothing has delivered a log record to report the delivery of,
    /// exactly as [`default_verified_access_logs`] leaves it.
    ///
    /// The ONE definition of what an update does to a configuration, so the pre-check of the
    /// store's byte budget and
    /// [`ComAmazonawsEc2Amazonec2State::set_verified_access_instance_logging_configuration`]'s
    /// write cannot disagree about the configuration the call leaves behind.
    pub fn applied_to(
        &self,
        logs: &crate::ops::types::VerifiedAccessLogs,
    ) -> crate::ops::types::VerifiedAccessLogs {
        let mut applied = logs.clone();
        if let Some(s3) = self.access_logs.s3.as_ref() {
            applied.s3 = Some(crate::ops::types::VerifiedAccessLogS3Destination {
                enabled: Some(s3.enabled),
                delivery_status: None,
                bucket_name: s3.bucket_name.clone(),
                prefix: s3.prefix.clone(),
                bucket_owner: s3.bucket_owner.clone(),
            });
        }
        if let Some(cloud_watch_logs) = self.access_logs.cloud_watch_logs.as_ref() {
            applied.cloud_watch_logs =
                Some(crate::ops::types::VerifiedAccessLogCloudWatchLogsDestination {
                    enabled: Some(cloud_watch_logs.enabled),
                    delivery_status: None,
                    log_group: cloud_watch_logs.log_group.clone(),
                });
        }
        if let Some(kinesis_data_firehose) = self.access_logs.kinesis_data_firehose.as_ref() {
            applied.kinesis_data_firehose = Some(
                crate::ops::types::VerifiedAccessLogKinesisDataFirehoseDestination {
                    enabled: Some(kinesis_data_firehose.enabled),
                    delivery_status: None,
                    delivery_stream: kinesis_data_firehose.delivery_stream.clone(),
                },
            );
        }
        if let Some(log_version) = self.access_logs.log_version.clone() {
            applied.log_version = Some(log_version);
        }
        if let Some(include_trust_context) = self.access_logs.include_trust_context {
            applied.include_trust_context = Some(include_trust_context);
        }
        applied
    }

    /// Whether `other` asks for exactly what this update asks for: the same instance and the
    /// same logging options, compared member by member because the modelled options shapes
    /// derive no equality of their own.
    ///
    /// What tells a replay of one request from the same `ClientToken` carrying a different
    /// one: "A unique, case-sensitive token that you provide to ensure idempotency of your
    /// modification request"
    /// (`ModifyVerifiedAccessInstanceLoggingConfigurationRequest$ClientToken`).
    pub fn same_request_as(&self, other: &Self) -> bool {
        fn s3(
            options: &crate::ops::types::VerifiedAccessLogOptions,
        ) -> Option<S3LogDestinationComparison<'_>> {
            options.s3.as_ref().map(|destination| {
                (
                    destination.enabled,
                    destination.bucket_name.as_deref(),
                    destination.prefix.as_deref(),
                    destination.bucket_owner.as_deref(),
                )
            })
        }
        fn cloud_watch_logs(
            options: &crate::ops::types::VerifiedAccessLogOptions,
        ) -> Option<NamedLogDestinationComparison<'_>> {
            options
                .cloud_watch_logs
                .as_ref()
                .map(|destination| (destination.enabled, destination.log_group.as_deref()))
        }
        fn kinesis_data_firehose(
            options: &crate::ops::types::VerifiedAccessLogOptions,
        ) -> Option<NamedLogDestinationComparison<'_>> {
            options
                .kinesis_data_firehose
                .as_ref()
                .map(|destination| (destination.enabled, destination.delivery_stream.as_deref()))
        }

        self.verified_access_instance_id == other.verified_access_instance_id
            && s3(&self.access_logs) == s3(&other.access_logs)
            && cloud_watch_logs(&self.access_logs) == cloud_watch_logs(&other.access_logs)
            && kinesis_data_firehose(&self.access_logs)
                == kinesis_data_firehose(&other.access_logs)
            && self.access_logs.log_version.as_deref() == other.access_logs.log_version.as_deref()
            && self.access_logs.include_trust_context == other.access_logs.include_trust_context
    }
}

/// The caller-chosen bytes one stored logging configuration holds for the process's lifetime:
/// the destination names the request chose and the log version it named. The enabled flags
/// and the instance id the configuration is keyed by are bounded already, so they are not
/// counted here.
///
/// ONE definition of the charge, computed identically by the modify's ceiling pre-check and
/// by
/// [`ComAmazonawsEc2Amazonec2State::set_verified_access_instance_logging_configuration`], so
/// a writer cannot store bytes the store's budget never saw.
pub fn verified_access_logs_payload_bytes(
    logs: &crate::ops::types::VerifiedAccessLogs,
    client_token: Option<&str>,
) -> usize {
    let text = |value: &Option<String>| value.as_ref().map_or(0, String::len);
    let mut bytes = text(&logs.log_version).saturating_add(client_token.map_or(0, str::len));
    if let Some(s3) = logs.s3.as_ref() {
        bytes = bytes
            .saturating_add(text(&s3.bucket_name))
            .saturating_add(text(&s3.prefix))
            .saturating_add(text(&s3.bucket_owner));
    }
    if let Some(cloud_watch_logs) = logs.cloud_watch_logs.as_ref() {
        bytes = bytes.saturating_add(text(&cloud_watch_logs.log_group));
    }
    if let Some(kinesis_data_firehose) = logs.kinesis_data_firehose.as_ref() {
        bytes = bytes.saturating_add(text(&kinesis_data_firehose.delivery_stream));
    }
    bytes
}

/// The caller-chosen bytes one recorded logging-configuration update holds for the process's
/// lifetime: the options the request carried and the token the map is keyed by, over a fixed
/// floor for the entry itself.
///
/// The FLOOR is there for the reason
/// [`verified_access_instance_modification_payload_bytes`] has one: nothing reclaims a
/// recorded update, a client token may be a single byte, and an instance's logging
/// configuration may be set any number of times, so this map has no record-count ceiling of
/// its own.
pub fn verified_access_instance_logging_update_payload_bytes(
    update: &VerifiedAccessInstanceLoggingConfigurationUpdate,
    client_token: &str,
) -> usize {
    ::std::mem::size_of::<VerifiedAccessInstanceLoggingConfigurationUpdate>()
        .saturating_add(verified_access_logs_payload_bytes(
            &update.applied_to(&default_verified_access_logs()),
            None,
        ))
        .saturating_add(client_token.len())
}

/// The caller-chosen bytes one stored Verified Access endpoint holds for the process's
/// lifetime: every string the REQUEST chose the length of, the policy document beside the
/// record, the `EndpointDomainPrefix` the generated endpoint domain carries, and the client
/// token the token map keeps.
///
/// The ids, the timestamps, the status and the endpoint and attachment kinds are the
/// service's own or come from a modelled enum, so they are bounded already and not counted
/// here, and so is the rest of the generated endpoint domain (the minted id, the Region and
/// the partition suffix) — only `domain_prefix`, the one part of that name the CALLER chose
/// the length of, is charged, which is why the figure does not depend on whether the domain
/// has been minted yet. The `PortRanges` entries of the endpoint's options block ARE charged,
/// at the size of the stored range: the list's length is the caller's
/// ([`MAX_VERIFIED_ACCESS_ENDPOINT_PORT_RANGES`] caps how many, this charge makes the byte
/// budget see them). ONE definition of the charge, computed identically by the create's
/// ceiling pre-check and by
/// [`ComAmazonawsEc2Amazonec2State::insert_verified_access_endpoint`], so a writer cannot
/// store bytes the store's budget never saw.
pub fn verified_access_endpoint_payload_bytes(
    record: &VerifiedAccessEndpointRecord,
    domain_prefix: Option<&str>,
    client_token: Option<&str>,
) -> usize {
    let text = |value: &Option<String>| value.as_ref().map_or(0, String::len);
    let ids = |values: &Option<Vec<String>>| {
        values
            .iter()
            .flatten()
            .fold(0usize, |total, value| total.saturating_add(value.len()))
    };
    // A port range holds two bounded integers, so what the caller chooses is HOW MANY of
    // them the record keeps: each entry is charged at the size of the stored range.
    let port_ranges = |ranges: &Option<Vec<crate::ops::types::VerifiedAccessEndpointPortRange>>| {
        ranges.iter().flatten().count().saturating_mul(
            ::std::mem::size_of::<crate::ops::types::VerifiedAccessEndpointPortRange>(),
        )
    };
    let endpoint = &record.view;
    let mut bytes = tag_list_bytes(endpoint.tags.as_deref().unwrap_or_default())
        .saturating_add(text(&endpoint.description))
        .saturating_add(text(&endpoint.application_domain))
        .saturating_add(text(&endpoint.domain_certificate_arn))
        .saturating_add(domain_prefix.map_or(0, str::len))
        .saturating_add(ids(&endpoint.security_group_ids))
        .saturating_add(text(&record.policy_document))
        .saturating_add(client_token.map_or(0, str::len));
    if let Some(options) = endpoint.load_balancer_options.as_ref() {
        bytes = bytes
            .saturating_add(text(&options.load_balancer_arn))
            .saturating_add(ids(&options.subnet_ids))
            .saturating_add(port_ranges(&options.port_ranges));
    }
    if let Some(options) = endpoint.network_interface_options.as_ref() {
        bytes = bytes
            .saturating_add(text(&options.network_interface_id))
            .saturating_add(port_ranges(&options.port_ranges));
    }
    if let Some(options) = endpoint.rds_options.as_ref() {
        bytes = bytes
            .saturating_add(text(&options.rds_db_instance_arn))
            .saturating_add(text(&options.rds_db_cluster_arn))
            .saturating_add(text(&options.rds_db_proxy_arn))
            .saturating_add(text(&options.rds_endpoint))
            .saturating_add(ids(&options.subnet_ids));
    }
    if let Some(options) = endpoint.cidr_options.as_ref() {
        bytes = bytes
            .saturating_add(text(&options.cidr))
            .saturating_add(ids(&options.subnet_ids))
            .saturating_add(port_ranges(&options.port_ranges));
    }
    if let Some(sse) = endpoint.sse_specification.as_ref() {
        bytes = bytes.saturating_add(text(&sse.kms_key_arn));
    }
    bytes
}

/// Refuse a request that named a Verified Access group id this account holds no group for:
/// HTTP 400 [`INVALID_VERIFIED_ACCESS_GROUP_ID_NOT_FOUND`].
///
/// One code covers an id of any shape, the way
/// [`invalid_verified_access_trust_provider_id_not_found`] does: an id this service never
/// minted names no group of the account whether or not it reads like one. The message names
/// the id the REQUEST gave, in the `The <resource> ID '<id>' does not exist` wording EC2
/// answers an unresolvable id with ([`crate::scaffolded::Scaffolded::invalid_instance_id_not_found`],
/// [`invalid_dhcp_option_id_not_found`]), so a client reading the text sees which id it was.
pub fn invalid_verified_access_group_id_not_found<E>(group_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VERIFIED_ACCESS_GROUP_ID_NOT_FOUND,
        400,
        format!("The verified-access-group ID '{group_id}' does not exist"),
    )
}

/// Refuse a request that named a Verified Access endpoint id this account holds no endpoint
/// for: HTTP 400 [`INVALID_VERIFIED_ACCESS_ENDPOINT_ID_NOT_FOUND`], worded like
/// [`invalid_verified_access_group_id_not_found`].
pub fn invalid_verified_access_endpoint_id_not_found<E>(
    endpoint_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VERIFIED_ACCESS_ENDPOINT_ID_NOT_FOUND,
        400,
        format!("The verified-access-endpoint ID '{endpoint_id}' does not exist"),
    )
}

/// Refuse a request that named a Verified Access instance id this account holds no instance
/// for: HTTP 400 [`INVALID_VERIFIED_ACCESS_INSTANCE_ID_NOT_FOUND`], worded like
/// [`invalid_verified_access_group_id_not_found`].
///
/// This is the wording the operations that MUTATE an instance — or that take one as a
/// reference — answer, which is the wording recorded for them. A REPORT of the instances
/// words the same refusal as its own sentence
/// ([`verified_access_instance_id_not_found_in_report`]): what a refusal says is a fact of
/// the operation that answers it (see `crate::scaffolded`), not of the resource, so the two
/// wordings live side by side rather than one being made to stand for both.
pub fn invalid_verified_access_instance_id_not_found<E>(
    instance_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VERIFIED_ACCESS_INSTANCE_ID_NOT_FOUND,
        400,
        format!("The verified-access-instance ID '{instance_id}' does not exist"),
    )
}

/// Refuse a REPORT of the Verified Access instances that named an instance id this account
/// holds no instance for: HTTP 400 [`INVALID_VERIFIED_ACCESS_INSTANCE_ID_NOT_FOUND`] — the
/// same code the mutating paths answer — worded as the sentence the real service answers a
/// report with: `VerifiedAccessInstance <id> does not exist`, built from the id the REQUEST
/// gave.
///
/// The sentence form is this resource family's own on that path — the same shape
/// [`invalid_verified_access_trust_provider_id_not_found`] answers for a trust provider —
/// rather than the quoted `The <resource> ID '<id>' does not exist` wording
/// [`invalid_verified_access_instance_id_not_found`] answers for a mutation of an instance.
pub fn verified_access_instance_id_not_found_in_report<E>(
    instance_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        INVALID_VERIFIED_ACCESS_INSTANCE_ID_NOT_FOUND,
        400,
        format!("VerifiedAccessInstance {instance_id} does not exist"),
    )
}

/// Refuse a create that would take the store past [`MAX_VERIFIED_ACCESS_GROUPS`]: HTTP 400
/// [`VERIFIED_ACCESS_GROUP_LIMIT_EXCEEDED`].
pub fn verified_access_group_limit_exceeded<E>(
    message: impl Into<String>,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(VERIFIED_ACCESS_GROUP_LIMIT_EXCEEDED, 400, message)
}

/// Refuse a create that would take the store past [`MAX_VERIFIED_ACCESS_ENDPOINTS`]: HTTP
/// 400 [`VERIFIED_ACCESS_ENDPOINT_LIMIT_EXCEEDED`].
pub fn verified_access_endpoint_limit_exceeded<E>(
    message: impl Into<String>,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        VERIFIED_ACCESS_ENDPOINT_LIMIT_EXCEEDED,
        400,
        message,
    )
}

/// Refuse a create that would take the store past [`MAX_VERIFIED_ACCESS_INSTANCES`]: HTTP
/// 400 [`VERIFIED_ACCESS_INSTANCE_LIMIT_EXCEEDED`].
pub fn verified_access_instance_limit_exceeded<E>(
    message: impl Into<String>,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_error(
        VERIFIED_ACCESS_INSTANCE_LIMIT_EXCEEDED,
        400,
        message,
    )
}
