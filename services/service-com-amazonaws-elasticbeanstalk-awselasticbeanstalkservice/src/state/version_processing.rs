// SPDX-License-Identifier: CC0-1.0

//! The pre-processing of an application version's source bundle: what
//! `CreateApplicationVersion$Process` asks for, what it validates, and the
//! statuses a version moves through while it runs.
//!
//! `Process` "pre-processes and validates the environment manifest (`env.yaml`)
//! and configuration files (`*.config` files in the `.ebextensions` folder) in
//! the source bundle", and `ApplicationVersionDescription$Status` reports how
//! far that work has got: `Processing` ("Elastic Beanstalk is currently
//! processing the application version") while it runs, then the terminal status
//! it reached — `Processed` ("was successfully pre-processed and validated") or
//! `Failed` ("configuration files didn't pass validation. This application
//! version isn't usable"). A version whose create did NOT ask for it is
//! `Unprocessed` ("Application version wasn't pre-processed or validated"),
//! which is terminal too.
//!
//! So the validation here is REAL: the archive the request named is downloaded
//! from object storage while the create is served, its manifest and its
//! configuration files are parsed, and the verdict that reading reached is what
//! the version settles in one [`STATUS_TRANSITION_SPAN`] later. Nothing decides
//! a verdict from the shape of the bucket, the key or the version label.
//!
//! The archive is CALLER content, so both the bytes downloaded
//! ([`MAX_ARCHIVE_BYTES`]) and the manifest and configuration text parsed out
//! of them ([`MAX_VALIDATED_TEXT_BYTES`], applied to what is DECOMPRESSED) are
//! bounded; how many entries the archive declares is bounded by the first of
//! those, since nothing but an entry's name is read for an entry that is neither
//! of the two files validated, and nothing is retained once the verdict is
//! reached. Both bounds and the download itself are
//! [`super::caller_archive`]'s, shared with the platform build that reads the
//! other caller-named archive. The reading happens before this service's store
//! is locked, so no other caller waits on it.
//!
//! Split out of [`super`], which holds the store the versions live in.

use super::*;

/// `ApplicationVersionStatus$Processing` — "Elastic Beanstalk is currently
/// processing the application version". Reported until the clock makes the
/// terminal status due; see [`PendingVersionProcessing`].
pub const VERSION_STATUS_PROCESSING: &str = "Processing";

/// `ApplicationVersionStatus$Processed` — "Elastic Beanstalk was successfully
/// pre-processed and validated".
pub const VERSION_STATUS_PROCESSED: &str = "Processed";

/// `ApplicationVersionStatus$Failed` — "configuration files didn't pass
/// validation. This application version isn't usable."
pub const VERSION_STATUS_FAILED: &str = "Failed";

/// The environment manifest pre-processing validates, at the root of the source
/// bundle: `env.yaml`, as `CreateApplicationVersion$Process` names it.
const ENVIRONMENT_MANIFEST: &str = "env.yaml";

/// The folder in the source bundle whose configuration files pre-processing
/// validates, as `CreateApplicationVersion$Process` names it: the `.ebextensions`
/// folder.
const EBEXTENSIONS_FOLDER: &str = ".ebextensions/";

/// The extension of a configuration file in that folder, as
/// `CreateApplicationVersion$Process` names it: `*.config`.
const CONFIG_FILE_EXTENSION: &str = ".config";

/// What pre-processing a source bundle concluded about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleProcessing {
    /// The archive was read and its manifest and configuration files parsed:
    /// the version is `Processed`.
    Processed,
    /// The archive could not be read as an archive, or one of the files
    /// pre-processing validates did not parse: the version is `Failed`.
    Failed,
    /// NOTHING validated the bundle, so there is no verdict to report and the
    /// version stays `Unprocessed` ("Application version wasn't pre-processed or
    /// validated"): the archive is past [`MAX_ARCHIVE_BYTES`], or object
    /// storage answered something that says nothing about the object at all (an
    /// emulator assembled without object storage answers the router's own
    /// `UnknownService`, and a transport failure answers nothing) — the same
    /// class of answers [`archive_download_refusal`] declines to refuse a create
    /// for.
    NotProcessed,
}

impl BundleProcessing {
    /// The terminal `ApplicationVersionStatus` this verdict settles the version
    /// in, or `None` when nothing validated the bundle.
    fn terminal_status(self) -> Option<&'static str> {
        match self {
            Self::Processed => Some(VERSION_STATUS_PROCESSED),
            Self::Failed => Some(VERSION_STATUS_FAILED),
            Self::NotProcessed => None,
        }
    }
}

/// The pre-processing of an application version the clock owes: the terminal
/// status the version moves to, and when that move is due.
///
/// The verdict is already DECIDED — it was reached by reading the bundle while
/// the create was served — and what the clock owes is only when the version
/// stops reporting `Processing` and reports it. The move is due one
/// [`STATUS_TRANSITION_SPAN`] after the create, so a caller that describes the
/// version straight after creating it still observes the transient status the
/// real service reports, and it is materialised by the next call that observes
/// it as due — nothing sleeps.
#[derive(Debug, Clone)]
pub struct PendingVersionProcessing {
    pub due_at: Instant,
    /// The same instant on the server clock, so the version's `DateUpdated`
    /// records when the status CHANGED rather than when a later call happened to
    /// observe the change as due.
    pub due_date: ::aws_smithy_types::DateTime,
    /// The terminal status the move lands in: one of
    /// [`VERSION_STATUS_PROCESSED`] and [`VERSION_STATUS_FAILED`].
    pub status: String,
}

impl PendingVersionProcessing {
    /// The move a create that asked for pre-processing owes, for the `verdict`
    /// that reading the bundle reached — and `None` when nothing validated the
    /// bundle, because then there is no status to move to.
    pub fn started(verdict: BundleProcessing) -> Option<Self> {
        verdict.terminal_status().map(|status| Self {
            due_at: Instant::now() + STATUS_TRANSITION_SPAN,
            due_date: due_date(),
            status: status.to_string(),
        })
    }
}

impl ApplicationVersion {
    /// The status a version starts in for the pre-processing `pending` the clock
    /// owes it: `Processing` while there is one to wait for, and `Unprocessed`
    /// when there is not.
    pub fn initial_status(pending: Option<&PendingVersionProcessing>) -> String {
        match pending {
            Some(_) => VERSION_STATUS_PROCESSING.to_string(),
            None => VERSION_STATUS_UNPROCESSED.to_string(),
        }
    }

    /// Materialise the pre-processing this version owes, if the clock has made it
    /// due by `at`.
    ///
    /// The move was due at that instant whether or not anybody called, so
    /// applying it on a read cannot hide a status that was still reportable:
    /// until then the version reports `Processing`, the way the real service
    /// reports a version it is still working on.
    pub(super) fn materialize_due_processing(&mut self, at: Instant) {
        let Some(pending) = self
            .pending_processing
            .take_if(|pending| pending.due_at <= at)
        else {
            return;
        };
        self.status = pending.status;
        // The record CHANGED, and it changed when the move came due.
        self.date_updated = pending.due_date;
    }
}

impl ComAmazonawsElasticbeanstalkAwselasticbeanstalkserviceState {
    /// Materialise just the application-version pre-processing the clock has
    /// already finished — the part of [`Self::materialize_due_transitions`] that
    /// an operation reporting an APPLICATION VERSION owes before it reads the
    /// version store, so a version whose processing is already due is reported in
    /// the status it reached. An operation that touches an environment or a
    /// platform version calls the whole of it instead.
    pub fn materialize_due_version_processing(&mut self) {
        self.materialize_due_version_processing_at(Instant::now());
    }

    /// [`Self::materialize_due_version_processing`] at the instant the caller is
    /// materialising the rest of the store for, so no two changes one call
    /// materialises disagree about when "now" is.
    pub(super) fn materialize_due_version_processing_at(&mut self, at: Instant) {
        for version in self.application_versions.values_mut() {
            version.materialize_due_processing(at);
        }
    }
}

/// What a source bundle IS, for the log line a read that learned nothing leaves
/// behind. See [`download_caller_archive`].
const SOURCE_BUNDLE_SUBJECT: &str = "an application version's source bundle";

/// The verdict pre-processing reaches for the source bundle at `location`:
/// download it from object storage, in process, and validate the files
/// `CreateApplicationVersion$Process` names.
///
/// The read is the shared one ([`download_caller_archive`]), so this
/// pre-processing and the platform build read a caller-named archive exactly
/// alike; what belongs to pre-processing is only what it CONCLUDES from the
/// bytes.
///
/// The body IS needed here, unlike the existence check the create performs
/// first ([`archive_download_refusal`], a `HeadObject`): the verdict is about
/// what the archive CONTAINS, so there is nothing to conclude without reading
/// it.
pub async fn process_source_bundle(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &S3Location,
) -> BundleProcessing {
    match download_caller_archive(ctx, region, location, SOURCE_BUNDLE_SUBJECT).await {
        DownloadedArchive::Bytes(bytes) => validate_source_bundle(&bytes),
        // Object storage said the archive is not there, or not readable: the
        // bundle pre-processing was asked to validate cannot be read at all,
        // which is a processing failure rather than silence.
        DownloadedArchive::NotReadable => BundleProcessing::Failed,
        DownloadedArchive::NotRead => BundleProcessing::NotProcessed,
    }
}

/// The verdict pre-processing reaches for the source bundle `bytes`.
///
/// An Elastic Beanstalk source bundle is a single archive, so reading the
/// archive FORMAT is the `zip` crate's job and parsing the manifest and the
/// configuration files is the YAML reader's — a `.config` file is YAML or the
/// JSON that YAML is a superset of. What is owned here is WHICH files are
/// validated (the two `CreateApplicationVersion$Process` names) and the bound on
/// how much of them is parsed.
///
/// Everything that fails is a validation failure of the files
/// pre-processing is about: an archive no archive reader will read, a manifest or
/// configuration file that is not text, and one that is text but not a YAML
/// document. A bundle carrying neither file has nothing to fail on, which is why
/// an archive with no `env.yaml` and no `.ebextensions` is `Processed`.
fn validate_source_bundle(bytes: &[u8]) -> BundleProcessing {
    let mut archive = match ::zip::ZipArchive::new(::std::io::Cursor::new(bytes)) {
        Ok(archive) => archive,
        Err(failure) => {
            ::tracing::debug!(
                detail = %failure,
                "an application version's source bundle is not an archive this emulator can \
                 read, so its pre-processing failed",
            );
            return BundleProcessing::Failed;
        }
    };
    let mut budget = MAX_VALIDATED_TEXT_BYTES;
    for index in 0..archive.len() {
        let mut entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(failure) => {
                ::tracing::debug!(
                    detail = %failure,
                    "an entry of an application version's source bundle could not be read, so \
                     its pre-processing failed",
                );
                return BundleProcessing::Failed;
            }
        };
        // The archive reader reports a name as a `Result`, because a name that
        // is not valid UTF-8 is no path at all — and such an entry is neither of
        // the two files validated here, so it is skipped rather than failed.
        let Ok(name) = entry.name().map(|name| name.to_string()) else {
            continue;
        };
        if !is_validated_file(&name) {
            continue;
        }
        // The bound is applied to what is DECOMPRESSED, by the one reader both
        // archive validations take an entry's text through.
        let text = match archive_entry_text(&mut entry, &mut budget) {
            ArchiveEntryText::Text(text) => text,
            ArchiveEntryText::PastBudget => {
                ::tracing::warn!(
                    entry = %name,
                    "an application version's source bundle carries more manifest and \
                     configuration text than this emulator parses, so nothing pre-processed the \
                     version",
                );
                return BundleProcessing::NotProcessed;
            }
            ArchiveEntryText::NotText => {
                ::tracing::debug!(
                    entry = %name,
                    "a file an application version's source bundle is pre-processed on is not \
                     text, so its pre-processing failed",
                );
                return BundleProcessing::Failed;
            }
        };
        if let Err(failure) = ::yaml_rust2::YamlLoader::load_from_str(&text) {
            ::tracing::debug!(
                entry = %name,
                detail = %failure,
                "a file an application version's source bundle is pre-processed on did not \
                 parse, so its pre-processing failed",
            );
            return BundleProcessing::Failed;
        }
    }
    BundleProcessing::Processed
}

/// Whether `path`, an entry of the source bundle, is one of the files
/// pre-processing validates: the environment manifest at the bundle's root, or a
/// configuration file in its `.ebextensions` folder — `*.config` files IN that
/// folder, so an entry deeper under it is not one.
fn is_validated_file(path: &str) -> bool {
    if path == ENVIRONMENT_MANIFEST {
        return true;
    }
    path.strip_prefix(EBEXTENSIONS_FOLDER)
        .is_some_and(|name| !name.contains('/') && name.ends_with(CONFIG_FILE_EXTENSION))
}
