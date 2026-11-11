// SPDX-License-Identifier: CC0-1.0

//! What the BUILD of a revision reads: the deployment package the revision's
//! `buildConfig.codeConfig.s3Object` names, and whether that object is an archive the
//! build can open at all.
//!
//! `CodeConfig$s3Object` is "The Amazon S3 location of the deployment artifact" and
//! `RevisionState` documents the build the create starts as asynchronous — `Pending` is
//! "the revision is being built", `Active` "ready to serve traffic", `Failed` "the
//! revision build failed". So the build has an OUTCOME, and the outcome is decided by the
//! object: a deployment package is a .zip archive, and an object no zip reader can open
//! builds nothing.
//!
//! The object is read by ASKING AMAZON S3 for it, over the in-process callback into this
//! same emulator ([`smithy_server_runtime::OperationContext::callback`]) through the one
//! shared client construction ([`::service_common::s3_client`]): the read is a `GetObject`
//! signed with this emulator's own credentials and dispatched in process, so the bytes the
//! build judges are the ones `PutObject` wrote, and nothing here opens a socket or
//! resolves a host. It is made with no store guard held, because it is a call out of the
//! handler.
//!
//! Three outcomes, kept apart because they are different claims:
//!
//! * the object is an archive the build could open — the build succeeds, and the revision
//!   settles `Active` when the clock makes that transition due;
//! * Amazon S3 ANSWERED that it will not serve the object, or it served bytes that are no
//!   zip archive — a fact about the object the revision names, so the build FAILS and the
//!   revision settles `Failed` carrying the reason;
//! * nothing answered the read at all, or nothing could be read from the answer (this
//!   emulator was assembled with no object store to ask, the body stopped part-way, or the
//!   object is longer than this emulator reads) — NOTHING about the package is established,
//!   so no failure is reported: a build is refuted by an answer about its object, never by
//!   this emulator failing to ask.

use ::aws_sdk_s3::error::ProvideErrorMetadata as _;

use crate::ops::types::S3Object;

/// The `stateReason` a build settles with when the object it fetched is not a zip archive.
///
/// The whole fixed sentence AWS words this failure class as — it has no variable part, and
/// it is the same sentence whatever the object held and whatever reader read it, so the
/// words are a constant here rather than the Rust reader's own diagnosis.
pub const NOT_A_ZIP_ARCHIVE_REASON: &str = "Failed to read zip file: zip: not a valid zip file";

/// How many bytes of a deployment package this emulator holds in memory to judge it.
///
/// CHOSEN, not cited: the model publishes no package ceiling for a web function. The read
/// is bounded all the same, because the object is a caller's and the judgement needs only
/// the archive's own structure. The bound is applied to the READ: an object Amazon S3
/// reports as longer is not fetched, and a body that turns out to be longer than it claimed
/// stops one byte past the bound — in both cases nothing about the package is established,
/// rather than a judgement being made on part of it.
pub const MAX_PACKAGE_BYTES: usize = 64 * 1024 * 1024;

/// [`MAX_PACKAGE_BYTES`] as the type Amazon S3 reports an object's length in.
const MAX_PACKAGE_BYTES_I64: i64 = MAX_PACKAGE_BYTES as i64;

/// What following a revision's `s3Object` established about its deployment package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodePackage {
    /// The object store served the object and it is an archive the build could open.
    Built,
    /// The build cannot read a deployment package there, and this is the reason it fails
    /// with: either Amazon S3's own answer about the object, or that the bytes it served
    /// are no zip archive.
    Unbuildable(String),
    /// Nothing established whether there is a package there at all: no object store
    /// answered, or its answer could not be read.
    Unestablished,
}

impl CodePackage {
    /// The `stateReason` the revision this verdict was taken for settles `Failed` with,
    /// and `None` for a revision whose build settles successfully.
    ///
    /// An [`Self::Unestablished`] resolution reports no failure: see the module header.
    #[must_use]
    pub fn failure_reason(&self) -> Option<String> {
        match self {
            Self::Unbuildable(reason) => Some(reason.clone()),
            Self::Built | Self::Unestablished => None,
        }
    }
}

/// The sentence a build that was refused the object fails with: the call it made, and the
/// two parts AMAZON S3 itself worded — the code it classified the refusal under and the
/// message it answered.
///
/// This is Lambda's own wording for a deployment package it could not read out of Amazon
/// S3 (the sentence `services/service-com-amazonaws-lambda-awsgirapiservice` states for
/// the same read), so the two Lambda surfaces report one refusal alike.
#[must_use]
fn refusal_reason(code: &str, message: &str) -> String {
    format!("Error occurred while GetObject. S3 Error Code: {code}. S3 Error Message: {message}")
}

/// What the build of a revision whose code object is `object` can establish about its
/// deployment package, read through the emulator's own Amazon S3 in `region`.
///
/// A READ: it fetches and judges, and changes nothing in the object store.
pub async fn resolve(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    object: &S3Object,
) -> CodePackage {
    let read = ::service_common::s3_client::s3_client(ctx.callback(), region)
        .get_object()
        .bucket(&object.bucket)
        .key(&object.key)
        .set_version_id(object.version_id.clone())
        .send()
        .await;
    let read = match read {
        Ok(read) => read,
        Err(error) => {
            // What AMAZON S3 answered, when it answered at all: a hop that produced no
            // coded refusal of its own establishes nothing about the object, and is worth
            // a warning, because a deployment with no Amazon S3 mounted to ask is one
            // where no build's package is ever established. The caller's own values are
            // rendered in their escaped `Debug` form, so a bucket or key carrying a
            // newline cannot forge a log line.
            let Some((code, message)) = error.as_service_error().and_then(|service| {
                let code = service.code()?;
                Some((
                    code.to_string(),
                    service.message().unwrap_or(code).to_string(),
                ))
            }) else {
                ::tracing::warn!(
                    bucket = ?object.bucket,
                    key = ?object.key,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a web function revision's code object could not be resolved against the \
                     emulator's Amazon S3"
                );
                return CodePackage::Unestablished;
            };
            return CodePackage::Unbuildable(refusal_reason(&code, &message));
        }
    };
    // The object's length as AMAZON S3 reported it, judged BEFORE a byte of it is held:
    // an object longer than this emulator reads is not fetched at all, so nothing about it
    // is established and no failure is claimed for it.
    if read
        .content_length()
        .is_some_and(|length| length > MAX_PACKAGE_BYTES_I64)
    {
        ::tracing::warn!(
            bucket = ?object.bucket,
            key = ?object.key,
            content_length = ?read.content_length(),
            "a web function revision's code object is longer than this emulator reads, so its \
             build outcome is not established"
        );
        return CodePackage::Unestablished;
    }
    // The object's bytes, read out of the in-process answer through a reader that stops ONE
    // byte past the bound — reading more than the bound is what says the object is over it,
    // whatever its answer claimed — so what this process holds for one request is bounded by
    // [`MAX_PACKAGE_BYTES`] and not by the object a caller staged. A body that stopped
    // part-way through is an answer this emulator could not read, which shows nothing about
    // the object.
    let mut bytes = Vec::new();
    let allowed = MAX_PACKAGE_BYTES as u64 + 1;
    let mut reader = ::tokio::io::AsyncReadExt::take(read.body.into_async_read(), allowed);
    if ::tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut bytes)
        .await
        .is_err()
    {
        return CodePackage::Unestablished;
    }
    if bytes.len() > MAX_PACKAGE_BYTES {
        ::tracing::warn!(
            bucket = ?object.bucket,
            key = ?object.key,
            "a web function revision's code object is longer than this emulator reads, so its \
             build outcome is not established"
        );
        return CodePackage::Unestablished;
    }
    judge_package(&bytes)
}

/// Whether `bytes` are a deployment package the build can open: an archive a zip reader
/// accepts, which is what a .zip deployment package is.
///
/// Only the ARCHIVE is judged — its entries are not inflated, because what decides this
/// build's outcome is whether there is an archive to build from at all.
#[must_use]
pub fn judge_package(bytes: &[u8]) -> CodePackage {
    match ::zip::ZipArchive::new(::std::io::Cursor::new(bytes)) {
        Ok(_) => CodePackage::Built,
        Err(_) => CodePackage::Unbuildable(NOT_A_ZIP_ARCHIVE_REASON.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::std::io::Write as _;

    /// A zip archive carrying one entry, for the rows below.
    fn archive(name: &str, contents: &[u8]) -> Vec<u8> {
        let mut writer = ::zip::ZipWriter::new(::std::io::Cursor::new(Vec::new()));
        writer
            .start_file(name, ::zip::write::SimpleFileOptions::default())
            .expect("the entry starts");
        writer.write_all(contents).expect("the entry is written");
        writer
            .finish()
            .expect("the archive is finished")
            .into_inner()
    }

    /// RULE: a build succeeds exactly when the object it fetched is an archive a zip
    /// reader can open; anything else is a build that failed, with the one fixed sentence
    /// AWS words that class as.
    #[test]
    fn only_a_readable_archive_builds() {
        let empty = archive("index.mjs", b"");
        let rows: Vec<(&str, Vec<u8>, Option<&str>)> = vec![
            ("an archive with one entry", archive("index.mjs", b"export const handler = 1;"), None),
            ("an archive with an empty entry", empty, None),
            ("a nested entry path", archive("src/app/index.mjs", b"x"), None),
            ("plain text", b"not a zip archive".to_vec(), Some(NOT_A_ZIP_ARCHIVE_REASON)),
            ("no bytes at all", Vec::new(), Some(NOT_A_ZIP_ARCHIVE_REASON)),
            (
                "a truncated archive: the local header alone",
                b"PK\x03\x04".to_vec(),
                Some(NOT_A_ZIP_ARCHIVE_REASON),
            ),
            (
                "an archive with its central directory cut off",
                {
                    let mut built = archive("index.mjs", b"x");
                    built.truncate(built.len() / 2);
                    built
                },
                Some(NOT_A_ZIP_ARCHIVE_REASON),
            ),
        ];
        for (case, bytes, expected) in rows {
            let judged = judge_package(&bytes);
            let reason = judged.failure_reason();
            assert_eq!(
                reason.as_deref(),
                expected,
                "case {case:?}: expected the failure reason {expected:?}, got {reason:?}"
            );
            assert_eq!(
                judged == CodePackage::Built,
                expected.is_none(),
                "case {case:?}: expected built {}, got {judged:?}",
                expected.is_none()
            );
        }
    }

    /// A refusal is reported as the call that was made and the two parts Amazon S3
    /// worded; a resolution that establishes nothing reports no reason at all.
    #[test]
    fn a_refused_read_reports_what_amazon_s3_answered() {
        let rows: [(&str, CodePackage, Option<&str>); 4] = [
            (
                "a key the bucket holds nothing under",
                CodePackage::Unbuildable(refusal_reason(
                    "NoSuchKey",
                    "The specified key does not exist.",
                )),
                Some(
                    "Error occurred while GetObject. S3 Error Code: NoSuchKey. S3 Error \
                     Message: The specified key does not exist.",
                ),
            ),
            (
                "a read the bucket will not serve",
                CodePackage::Unbuildable(refusal_reason("AccessDenied", "Access Denied")),
                Some(
                    "Error occurred while GetObject. S3 Error Code: AccessDenied. S3 Error \
                     Message: Access Denied",
                ),
            ),
            ("an object that is an archive", CodePackage::Built, None),
            ("nothing answered the read", CodePackage::Unestablished, None),
        ];
        for (case, package, expected) in rows {
            let reason = package.failure_reason();
            assert_eq!(
                reason.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {reason:?}"
            );
        }
    }
}
