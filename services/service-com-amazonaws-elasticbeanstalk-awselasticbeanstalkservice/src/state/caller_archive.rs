// SPDX-License-Identifier: CC0-1.0

//! The bounded, in-process DOWNLOAD of a caller-named Amazon S3 archive.
//!
//! Two creates build a resource from an archive the request names —
//! `CreateApplicationVersion$SourceBundle` and
//! `CreatePlatformVersion$PlatformDefinitionBundle` — and each has to read what
//! that archive CONTAINS to reach the verdict it reports (the pre-processing a
//! version settles in, the build a platform version settles in). The read
//! itself is the same read for both, so it is written ONCE here: the archive is
//! fetched over the in-process callback, held inside [`MAX_ARCHIVE_BYTES`], and
//! handed to the caller as bytes to interpret — [`super::version_processing`]
//! validates a source bundle's manifest and configuration files,
//! [`super::platform_build`] a platform definition's manifest.
//!
//! The read is a read: it changes nothing a later caller sees, in this service's
//! store or in object storage. Exactly one `GetObject` per request, no loop,
//! nothing retained — the bytes are dropped once the verdict has been reached.
//!
//! Split out of [`super`], which holds the store and the existence check the
//! create performs first ([`super::archive_download_refusal`], a `HeadObject`
//! that needs no body).

use super::*;

/// How many bytes of a caller-named archive this emulator reads.
///
/// A bound on CALLER content: the archive is downloaded into this process, so
/// its size is capped rather than trusted. An archive past the cap is not read
/// at all, which is why nothing is reported as validated for it either — a
/// verdict is only ever reached by reading the files it is about.
pub const MAX_ARCHIVE_BYTES: usize = 32 * 1024 * 1024;

/// How many bytes of text this emulator parses out of ONE archive, in total —
/// the second bound on caller content, applied to what is DECOMPRESSED so a
/// small archive cannot expand into this process.
pub const MAX_VALIDATED_TEXT_BYTES: usize = 4 * 1024 * 1024;

/// What a read of a caller-named archive handed back.
#[derive(Debug)]
pub enum DownloadedArchive {
    /// The archive's bytes, within [`MAX_ARCHIVE_BYTES`].
    Bytes(Vec<u8>),
    /// Object storage said something about the OBJECT and that answer was not
    /// the archive: it is not there, or not readable (see
    /// [`answer_is_about_the_object`]). The archive the caller named cannot be
    /// read at all, which is a verdict about it rather than silence.
    NotReadable,
    /// NOTHING was read, and nothing about the caller's archive was learned:
    /// the location names no object at all, the archive is past
    /// [`MAX_ARCHIVE_BYTES`], or object storage answered something that says
    /// nothing about the object (an emulator assembled without object storage
    /// answers the router's own `UnknownService`, a transport failure answers
    /// nothing). An operation that reaches a verdict by reading reports none
    /// for this.
    NotRead,
}

/// Download the archive at `location` from object storage, in process.
///
/// `subject` names what the archive IS for the log line a read that learned
/// nothing leaves behind ("an application version's source bundle"), so the
/// warning says which create could not read its archive.
pub async fn download_caller_archive(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &S3Location,
    subject: &str,
) -> DownloadedArchive {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let bucket = location.s3_bucket.as_deref().unwrap_or_default();
    let key = location.s3_key.as_deref().unwrap_or_default();
    // A location that names no bucket, or no key, names no Amazon S3 object at
    // all: such a create is refused before anything is recorded, so there is
    // nothing here to read and nothing to conclude.
    if bucket.is_empty() || key.is_empty() {
        return DownloadedArchive::NotRead;
    }
    let read = match s3_client(ctx, region)
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
    {
        Ok(read) => read,
        Err(failure) => {
            let status = failure
                .raw_response()
                .map(|response| response.status().as_u16());
            let code = failure.code().unwrap_or_default().to_string();
            if answer_is_about_the_object(status, &code) {
                return DownloadedArchive::NotReadable;
            }
            ::tracing::warn!(
                bucket,
                key,
                subject,
                code = %code,
                status = ?status,
                detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                "the Amazon S3 read of a caller-named archive answered nothing about the object, \
                 so nothing was read from it",
            );
            return DownloadedArchive::NotRead;
        }
    };
    // The length the object REPORTS, before a single byte of it is held.
    if read
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES as i64)
    {
        ::tracing::warn!(
            bucket,
            key,
            subject,
            "a caller-named archive is past the bytes this emulator reads, so nothing was read \
             from it",
        );
        return DownloadedArchive::NotRead;
    }
    // The body is read CHUNK BY CHUNK and the bound applied before each chunk is
    // kept, so an object that reported no length — or a smaller one than it
    // sends — cannot grow this process past the cap: the read is abandoned the
    // moment the next chunk would carry it past, with nothing retained. Never
    // `collect()`, which would buffer the whole object before any bound could
    // look at it.
    let mut body = read.body;
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        let chunk = match body.next().await {
            // The object has been handed over in full, within the bound.
            None => break,
            Some(Ok(chunk)) => chunk,
            Some(Err(failure)) => {
                ::tracing::warn!(
                    bucket,
                    key,
                    subject,
                    detail = %::aws_smithy_types::error::display::DisplayErrorContext(&failure),
                    "a caller-named archive was handed over and then stopped, so nothing was \
                     read from it",
                );
                return DownloadedArchive::NotRead;
            }
        };
        if bytes.len().saturating_add(chunk.len()) > MAX_ARCHIVE_BYTES {
            ::tracing::warn!(
                bucket,
                key,
                subject,
                "a caller-named archive is past the bytes this emulator reads, so nothing was \
                 read from it",
            );
            return DownloadedArchive::NotRead;
        }
        bytes.extend_from_slice(&chunk);
    }
    DownloadedArchive::Bytes(bytes)
}

/// What reading one archive entry as text reached. See
/// [`archive_entry_text`].
#[derive(Debug)]
pub enum ArchiveEntryText {
    /// The entry's decompressed text, within the budget it was read inside.
    Text(String),
    /// The entry's text is past what this emulator parses, so NOTHING parsed
    /// it and nothing is concluded from it.
    PastBudget,
    /// The entry cannot be text at all — one the archive reader will not hand
    /// over, or bytes that are not UTF-8 — which is a validation failure of
    /// the file it is.
    NotText,
}

/// The text of the archive entry `entry`, decompressed inside what is left of
/// `budget` and read as UTF-8 — the one place an archive's own bytes become
/// text this emulator parses. What was read is taken out of `budget`, so one
/// archive's entries are parsed inside ONE allowance however many of them there
/// are.
pub fn archive_entry_text<R: ::std::io::Read>(
    entry: &mut R,
    budget: &mut usize,
) -> ArchiveEntryText {
    // One byte past the budget at most: `take` is what stops a stream that
    // claims to be small and is not.
    let mut text: Vec<u8> = Vec::new();
    if ::std::io::Read::read_to_end(
        &mut ::std::io::Read::take(entry, *budget as u64 + 1),
        &mut text,
    )
    .is_err()
    {
        return ArchiveEntryText::NotText;
    }
    if text.len() > *budget {
        return ArchiveEntryText::PastBudget;
    }
    *budget -= text.len();
    match String::from_utf8(text) {
        Ok(text) => ArchiveEntryText::Text(text),
        Err(_) => ArchiveEntryText::NotText,
    }
}
