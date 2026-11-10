// SPDX-License-Identifier: CC0-1.0

//! The BUILD of a custom platform version from its definition archive: what
//! `CreatePlatformVersion` validates, and the terminal `PlatformStatus` the
//! version settles in for it.
//!
//! "Create a new version of your custom platform" builds that version FROM
//! `PlatformDefinitionBundle`, "the location of the platform definition archive
//! in Amazon S3", and `PlatformStatus` reports how far the build has got:
//! `Creating` while it runs, then the terminal status it reached — `Ready` for a
//! version that built and can be launched on, `Failed` for one that did not. A
//! platform definition archive is built by the platform builder from the
//! PLATFORM DEFINITION FILE at its root, `platform.yaml`: an archive that
//! carries no such file defines no platform, so its build cannot produce one.
//!
//! So the validation here is REAL: the archive the request named is downloaded
//! from object storage while the create is served, its manifest is looked for
//! and parsed, and the verdict that reading reached is what the version settles
//! in one [`STATUS_TRANSITION_SPAN`] later. Nothing decides a verdict from the
//! shape of the bucket, the key, the platform name or the version.
//!
//! The download and both bounds on what is read are
//! [`super::caller_archive`]'s, the same ones the application version's
//! pre-processing reads its archive through, so neither reads caller content the
//! other would not.
//!
//! Split out of [`super`], which holds the platform versions the builds belong
//! to.

use super::*;

/// The platform definition file at the ROOT of a custom platform's definition
/// archive, as Elastic Beanstalk's custom-platform documentation names it:
/// `platform.yaml`, the file the platform builder reads the platform's
/// definition from.
const PLATFORM_MANIFEST: &str = "platform.yaml";

/// What a platform definition archive IS, for the log line a read that learned
/// nothing leaves behind. See [`download_caller_archive`].
const PLATFORM_BUNDLE_SUBJECT: &str = "a custom platform version's definition bundle";

/// What the build of a platform definition archive concluded about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformBuild {
    /// The archive was read and carries a platform definition file that parses:
    /// the version is `Ready`.
    Built,
    /// The archive could not be read as an archive, carries no platform
    /// definition file, or carries one that did not parse: the build failed, so
    /// the version is `Failed`.
    Failed,
    /// NOTHING read the archive, so the build has no verdict of its own to
    /// report: the archive is past [`MAX_ARCHIVE_BYTES`], or object storage
    /// answered something that says nothing about the object at all (an
    /// emulator assembled without object storage answers the router's own
    /// `UnknownService`, and a transport failure answers nothing) — the same
    /// class of answers [`archive_download_refusal`] declines to refuse a
    /// create for.
    ///
    /// Nothing contradicted the build and `PlatformStatus` has no value for
    /// "not known", so such a version settles `Ready`, exactly as it did before
    /// the archive was read at all.
    NotBuilt,
}

impl PlatformBuild {
    /// The terminal `PlatformStatus` this verdict settles the version in.
    pub fn terminal_status(self) -> &'static str {
        match self {
            Self::Built | Self::NotBuilt => PLATFORM_STATUS_READY,
            Self::Failed => PLATFORM_STATUS_FAILED,
        }
    }
}

/// The verdict the build reaches for the platform definition archive at
/// `location`: download it from object storage, in process, and validate the
/// platform definition file it has to carry.
///
/// The read is a read: it changes nothing a later caller sees, in this service's
/// store or in object storage. The body IS needed, unlike the existence check
/// the create performs first ([`archive_download_refusal`], a `HeadObject`):
/// the verdict is about what the archive CONTAINS, so there is nothing to
/// conclude without reading it.
pub async fn build_platform_definition(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &S3Location,
) -> PlatformBuild {
    match download_caller_archive(ctx, region, location, PLATFORM_BUNDLE_SUBJECT).await {
        DownloadedArchive::Bytes(bytes) => validate_platform_definition(&bytes),
        // Object storage said the archive is not there, or not readable: the
        // archive the build was to be made from cannot be read at all, so the
        // build fails rather than reporting a platform version nothing built.
        DownloadedArchive::NotReadable => PlatformBuild::Failed,
        DownloadedArchive::NotRead => PlatformBuild::NotBuilt,
    }
}

/// The verdict the build reaches for the platform definition archive `bytes`.
///
/// A platform definition archive is a single `.zip`, so reading the archive
/// FORMAT is the `zip` crate's job and parsing the platform definition file is
/// the YAML reader's. What is owned here is WHICH file the build needs — the
/// manifest at the archive's root — and the bound on how much of it is parsed.
///
/// Everything that fails is a failure of the build: an archive no archive
/// reader will read, an archive carrying no platform definition file at all, and
/// one whose definition file is not text or not a YAML document.
fn validate_platform_definition(bytes: &[u8]) -> PlatformBuild {
    let mut archive = match ::zip::ZipArchive::new(::std::io::Cursor::new(bytes)) {
        Ok(archive) => archive,
        Err(failure) => {
            ::tracing::debug!(
                detail = %failure,
                "a custom platform version's definition bundle is not an archive this emulator \
                 can read, so its build failed",
            );
            return PlatformBuild::Failed;
        }
    };
    let mut budget = MAX_VALIDATED_TEXT_BYTES;
    for index in 0..archive.len() {
        let mut entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(failure) => {
                ::tracing::debug!(
                    detail = %failure,
                    "an entry of a custom platform version's definition bundle could not be \
                     read, so its build failed",
                );
                return PlatformBuild::Failed;
            }
        };
        // The archive reader reports a name as a `Result`, because a name that
        // is not valid UTF-8 is no path at all — and such an entry is not the
        // manifest, so it is skipped rather than failed.
        let Ok(name) = entry.name().map(|name| name.to_string()) else {
            continue;
        };
        if name != PLATFORM_MANIFEST {
            continue;
        }
        let text = match archive_entry_text(&mut entry, &mut budget) {
            ArchiveEntryText::Text(text) => text,
            // Nothing parsed the manifest, so the build concluded nothing about
            // it: the same silence an archive this emulator does not read at
            // all leaves.
            ArchiveEntryText::PastBudget => {
                ::tracing::warn!(
                    entry = %name,
                    "a custom platform version's definition file is larger than this emulator \
                     parses, so nothing built the version",
                );
                return PlatformBuild::NotBuilt;
            }
            ArchiveEntryText::NotText => {
                ::tracing::debug!(
                    entry = %name,
                    "a custom platform version's definition file is not text, so its build \
                     failed",
                );
                return PlatformBuild::Failed;
            }
        };
        if let Err(failure) = ::yaml_rust2::YamlLoader::load_from_str(&text) {
            ::tracing::debug!(
                entry = %name,
                detail = %failure,
                "a custom platform version's definition file did not parse, so its build failed",
            );
            return PlatformBuild::Failed;
        }
        return PlatformBuild::Built;
    }
    // The archive was read in full and carries no platform definition file, so
    // it defines no platform: the build has nothing to build from and fails.
    ::tracing::debug!(
        "a custom platform version's definition bundle carries no platform definition file, so \
         its build failed",
    );
    PlatformBuild::Failed
}
