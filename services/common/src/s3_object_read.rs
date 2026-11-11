// SPDX-License-Identifier: CC0-1.0

//! The ONE read of an Amazon S3 OBJECT a service was pointed at, and the ONE reading of
//! what the object store's answer established — the mirror of [`crate::s3_client`], which
//! is the hop this read is made over.
//!
//! A service handed a bucket and a key has to READ that object before it can answer:
//! whether the resource a caller asked for can be built at all, and every property of it
//! that is a property of the object's BYTES (how many certificates a bundle holds, how
//! large an archive is), is knowable only from the object. This emulator holds an Amazon S3
//! of its own, reached the way any emulated service reaches another — the AWS SDK over the
//! emulator's in-process transport, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), which opens no socket, resolves
//! no name and reaches nothing outside this process — so an object a caller staged with
//! `PutObject` is the object such a read serves.
//!
//! What a read establishes is one of five things, and none of them is a success the read did
//! not establish:
//!
//! * the store SERVED the object, so its bytes are known ([`ObjectRead::Served`]);
//! * the store ANSWERED that it holds nothing at that address ([`ObjectRead::Absent`]) —
//!   which is also the answer for an address that names no object anywhere (a blank bucket
//!   or key, or a bucket name Amazon S3 holds no bucket under,
//!   [`crate::s3_bucket_name::is_well_formed`]), since no read of it could ever serve one;
//! * the store ANSWERED that the read is not permitted ([`ObjectRead::Denied`]) — the object
//!   may well be there, and what the read established is that it may not be fetched;
//! * the store ANSWERED something else about that object, or the object is larger than the
//!   caller reads ([`ObjectRead::Unreadable`]): there is an object store and this object
//!   cannot be read through it. An EMPTY object is this answer, its first byte not existing;
//! * nothing ANSWERED the read at all ([`ObjectRead::Unanswered`]) — no object store is
//!   mounted to ask, or the call could not be dispatched — so nothing is known about the
//!   object and nothing is the caller's fault.
//!
//! Each service keeps what its own API answers for each of those (the status, the error
//! shape and the wording); the read itself is stated once here so no two services disagree
//! about what the store's answer MEANT.

use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;
use ::smithy_server_runtime::EmbeddedEmulator;

/// What a read of one Amazon S3 object established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectRead {
    /// The store served the object, and these are its bytes.
    Served(Vec<u8>),
    /// The store answered that it holds nothing at the address the request named, or that
    /// address names no object anywhere.
    Absent,
    /// The store answered that the read of the object is not permitted.
    Denied,
    /// There is an object store and this object cannot be read through it: it is larger
    /// than the caller reads, or the store answered something else about it.
    Unreadable,
    /// Nothing answered the read: there is no object store to ask, or the call could not be
    /// dispatched.
    Unanswered,
}

/// The Amazon S3 error codes in which the store answers that it holds NOTHING at the
/// address a request named.
pub const OBJECT_ABSENT_CODES: &[&str] =
    &["NoSuchBucket", "NoSuchKey", "NoSuchVersion", "NotFound"];

/// The Amazon S3 error codes in which the store answers that the read is NOT PERMITTED.
pub const OBJECT_DENIED_CODES: &[&str] = &["AccessDenied", "AllAccessDisabled"];

/// Read the object at `bucket`/`key` (optionally at `version`) of `region`, through this
/// emulator's own Amazon S3, buffering at most `max_bytes` of it.
///
/// The address is checked for being an address at all BEFORE anything is asked of the
/// store: a blank bucket or key, and a bucket name Amazon S3 holds no bucket under, name no
/// object anywhere. The read is RANGED to `max_bytes`, so a larger object is reported as
/// [`ObjectRead::Unreadable`] rather than buffered whole.
pub async fn read_object(
    callback: &EmbeddedEmulator,
    region: &str,
    bucket: &str,
    key: &str,
    version: Option<&str>,
    max_bytes: usize,
) -> ObjectRead {
    let (bucket, key) = (bucket.trim(), key.trim());
    if bucket.is_empty() || key.is_empty() || !crate::s3_bucket_name::is_well_formed(bucket) {
        ::tracing::debug!(
            bucket = ?bucket,
            key = ?key,
            "an object address names no Amazon S3 object"
        );
        return ObjectRead::Absent;
    }
    // Built by the one shared constructor, so this hop is configured exactly as every
    // other emulated service's hop into the object store is. The transport is in-process:
    // the endpoint it carries is addressing for the emulator's own router, not a host
    // anything connects to.
    let client = crate::s3_client::s3_client(callback, region);
    let mut read = client
        .get_object()
        .bucket(bucket)
        .key(key)
        // Ranged, so at most `max_bytes` plus the one byte that shows the object is past
        // the bound is ever buffered.
        .range(format!("bytes=0-{max_bytes}"));
    if let Some(version) = version.map(str::trim) {
        if !version.is_empty() {
            read = read.version_id(version);
        }
    }
    let answered = match read.send().await {
        Ok(answered) => answered,
        Err(error) => return answered_refusal(&error),
    };
    let body = match answered.body.collect().await {
        Ok(body) => body.into_bytes(),
        Err(error) => {
            ::tracing::warn!(
                error = %error,
                "an object did not arrive in full from the emulator's Amazon S3"
            );
            return ObjectRead::Unanswered;
        }
    };
    if body.len() > max_bytes {
        ::tracing::debug!(
            bytes = body.len(),
            "an object is larger than this read buffers"
        );
        return ObjectRead::Unreadable;
    }
    ObjectRead::Served(body.to_vec())
}

/// What the object store's refusal `error` establishes about the object.
///
/// An answer naming one of [`OBJECT_ABSENT_CODES`] or [`OBJECT_DENIED_CODES`] is the store
/// itself saying it holds nothing at that address, or that the read is not permitted — each
/// a fact about the address the request named. Any OTHER answer the store gave about that
/// object is one no object can be read through.
///
/// A failure carrying no error CODE is not an answer about the object at all — there was no
/// object store to ask, or the call could not be dispatched — so it establishes nothing and
/// the caller's request is not what is at fault. It is warned about, because a deployment
/// where no object can be read is worth noticing.
pub fn answered_refusal<E: ProvideErrorMetadata + ::std::fmt::Debug>(error: &E) -> ObjectRead {
    let Some(code) = error.code() else {
        ::tracing::warn!(
            error = ?error,
            "nothing answered an object read: this emulator has no Amazon S3 to read from"
        );
        return ObjectRead::Unanswered;
    };
    if OBJECT_ABSENT_CODES.contains(&code) {
        return ObjectRead::Absent;
    }
    if OBJECT_DENIED_CODES.contains(&code) {
        return ObjectRead::Denied;
    }
    ::tracing::warn!(
        code = ?code,
        error = ?error,
        "the emulator's Amazon S3 answered an object read with something no object can be \
         read through"
    );
    ObjectRead::Unreadable
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failure carrying the error metadata an object store's answer would carry, so the
    /// mapping below is driven by a CODE rather than by a real S3 round trip.
    #[derive(Debug)]
    struct Answered(::aws_smithy_types::error::ErrorMetadata);

    impl Answered {
        /// An answer naming `code`, as the object store names one.
        fn with_code(code: &str) -> Self {
            Self(
                ::aws_smithy_types::error::ErrorMetadata::builder()
                    .code(code)
                    .build(),
            )
        }

        /// A failure that names no code at all — nothing answered.
        fn unanswered() -> Self {
            Self(::aws_smithy_types::error::ErrorMetadata::builder().build())
        }
    }

    impl ProvideErrorMetadata for Answered {
        fn meta(&self) -> &::aws_smithy_types::error::ErrorMetadata {
            &self.0
        }
    }

    /// RULE: what the object store ANSWERED decides what the read established — the codes
    /// in which it says it holds nothing are an absent object, the codes in which it says
    /// the read is not permitted are a denied one, any other answer about that object is
    /// one no object can be read through, and a failure naming no code at all answered
    /// nothing.
    #[test]
    fn the_outcome_is_the_answer_the_object_store_gave() {
        let rows: &[(&str, Answered, ObjectRead)] = &[
            (
                "no such bucket",
                Answered::with_code("NoSuchBucket"),
                ObjectRead::Absent,
            ),
            (
                "no such key",
                Answered::with_code("NoSuchKey"),
                ObjectRead::Absent,
            ),
            (
                "no such version",
                Answered::with_code("NoSuchVersion"),
                ObjectRead::Absent,
            ),
            (
                "the object was not found",
                Answered::with_code("NotFound"),
                ObjectRead::Absent,
            ),
            (
                "the read is not permitted",
                Answered::with_code("AccessDenied"),
                ObjectRead::Denied,
            ),
            (
                "every access is disabled",
                Answered::with_code("AllAccessDisabled"),
                ObjectRead::Denied,
            ),
            (
                "an empty object, whose first byte does not exist",
                Answered::with_code("InvalidRange"),
                ObjectRead::Unreadable,
            ),
            (
                "an answer nothing here anticipated",
                Answered::with_code("PermanentRedirect"),
                ObjectRead::Unreadable,
            ),
            (
                "a failure naming no code at all",
                Answered::unanswered(),
                ObjectRead::Unanswered,
            ),
        ];
        for (case, error, expected) in rows {
            let actual = answered_refusal(error);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
