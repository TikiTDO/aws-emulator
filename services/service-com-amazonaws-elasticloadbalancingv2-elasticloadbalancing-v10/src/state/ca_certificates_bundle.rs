// SPDX-License-Identifier: CC0-1.0

//! Where a trust store's CA certificates come from: the object Amazon S3 holds at the
//! address `CaCertificatesBundleS3Bucket`/`CaCertificatesBundleS3Key` names.
//!
//! `TrustStore$NumberOfCaCertificates` is "The number of ca certificates in the trust
//! store", so neither the create that mints a trust store nor the modification that rotates
//! its bundle can answer at all without READING that object: the count is a property of the
//! bundle's bytes, not of the request. The read itself — and what the object store's answer
//! established — is the shared one every service pointed at an object makes
//! ([`::service_common::s3_object_read`]), so an object a caller staged with `PutObject` is
//! the bundle a trust store is built from.
//!
//! ## What a read establishes, and what this API answers for it
//!
//! This model declares exactly two refusals about a bundle, and they split on WHERE the
//! failure is: `CaCertificatesBundleNotFoundException` ("The specified ca certificate bundle
//! does not exist") is about the ADDRESS, and `InvalidCaCertificatesBundleException` ("The
//! specified ca certificate bundle is in an invalid format, or corrupt") is about the
//! BYTES. So:
//!
//! * the store served the object and its bytes hold PEM-encoded certificates, so the count
//!   is known ([`CaCertificatesBundle::Counted`]);
//! * nothing can be fetched at that address — the store holds nothing there, or answered
//!   that the read is not permitted, which this API models no separate refusal for —
//!   so the bundle the request named does not exist ([`CaCertificatesBundle::Absent`]);
//! * something is there and it is no CA certificates bundle: an empty object, one larger
//!   than this emulator reads, bytes holding no certificate, or an answer no bundle can be
//!   read through ([`CaCertificatesBundle::Invalid`]). A call that could not read its bundle
//!   is REFUSED: no trust store is ever stored reporting certificates nothing counted;
//! * nothing answered the read at all — no object store is mounted to ask
//!   ([`CaCertificatesBundle::Unanswered`]): that is this emulator's own inability to serve
//!   the operation, not the caller's fault, and it is reported as such.
//!
//! The PEM layer and the certificate layer are each read by the crate that owns it:
//! `rustls-pemfile` splits the bundle into its blocks (the reader this service already loads
//! a listener's certificate chain through, `crate::service::listener_socket`), and
//! `x509-cert` decodes a block's DER as an X.509 certificate — so what is counted is
//! certificates a parser accepted, never occurrences of a marker in the text.

/// The message a call whose CA certificates bundle nothing can be fetched at is refused
/// with, under the modelled `CaCertificatesBundleNotFound`.
///
/// Elastic Load Balancing words this refusal as a FIXED sentence about the bundle: neither
/// the bucket nor the key the request named is in it, so nothing of the request is
/// interpolated into it.
pub const CA_CERTIFICATES_BUNDLE_NOT_FOUND_MESSAGE: &str =
    "Provided CA certificate bundle was not found";

/// The message a call whose CA certificates bundle could be fetched and is no CA
/// certificates bundle is refused with, under the modelled `InvalidCaCertificatesBundle`
/// ("The specified ca certificate bundle is in an invalid format, or corrupt").
///
/// Elastic Load Balancing words it about what reading the bundle FOUND rather than about
/// the address: no certificate could be read out of the object, which is the one thing
/// every outcome refused under this code has in common (bytes that are no PEM, PEM
/// carrying no certificate, a certificate block holding no X.509 structure, an object with
/// no bytes at all). Nothing of the request is interpolated into it.
pub const INVALID_CA_CERTIFICATES_BUNDLE_MESSAGE: &str =
    "CA certificate bundle does not contain any certificates";

/// The most bundle bytes this emulator reads into the process.
///
/// CHOSEN by this emulator rather than read off a published quota: it bounds what one
/// control-plane call buffers, and it is far above any plausible CA bundle (a PEM
/// certificate is on the order of a kilobyte). The read is RANGED to it, so a larger object
/// is refused rather than buffered whole.
pub const MAX_CA_CERTIFICATES_BUNDLE_BYTES: usize = 1_048_576;

/// What a read of a trust store's CA certificates bundle established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaCertificatesBundle {
    /// The store served the bundle, and it holds this many CA certificates (at least one).
    Counted(i32),
    /// Nothing can be fetched at the address the request named.
    Absent,
    /// Something is at that address and no CA certificates bundle can be read from it.
    Invalid,
    /// Nothing answered the read, so nothing is known about the bundle.
    Unanswered,
}

/// Where one call said its CA certificates bundle is: the three members every operation
/// that takes a bundle carries (`CaCertificatesBundleS3Bucket`, `CaCertificatesBundleS3Key`
/// and the optional `CaCertificatesBundleS3ObjectVersion` — "If undefined the current
/// version is used").
///
/// Persisted on the trust store's record, so the bundle a trust store was built from is
/// what the store holds rather than something re-derived per call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaCertificatesBundleLocation {
    pub bucket: String,
    pub key: String,
    pub version: Option<String>,
}

/// What the trust store built from `location` can establish about its CA certificates
/// bundle, read through the emulator's own Amazon S3 in `region` — the Region the call was
/// signed for, since neither operation's input carries one.
pub async fn resolve_ca_certificates_bundle(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &CaCertificatesBundleLocation,
) -> CaCertificatesBundle {
    let read = ::service_common::s3_object_read::read_object(
        ctx.callback(),
        region,
        &location.bucket,
        &location.key,
        location.version.as_deref(),
        MAX_CA_CERTIFICATES_BUNDLE_BYTES,
    )
    .await;
    match read {
        ::service_common::s3_object_read::ObjectRead::Served(body) => {
            match ca_certificates_in(&body) {
                0 => CaCertificatesBundle::Invalid,
                counted => CaCertificatesBundle::Counted(counted),
            }
        }
        // The address is what the caller fixes for both of these: the store holds nothing
        // there, or it will not serve what it holds there, and this API models one refusal
        // for "the bundle does not exist" and none for a permission of the object store.
        ::service_common::s3_object_read::ObjectRead::Absent
        | ::service_common::s3_object_read::ObjectRead::Denied => CaCertificatesBundle::Absent,
        ::service_common::s3_object_read::ObjectRead::Unreadable => CaCertificatesBundle::Invalid,
        ::service_common::s3_object_read::ObjectRead::Unanswered => {
            CaCertificatesBundle::Unanswered
        }
    }
}

/// Which of this model's refusals a call owes when the bundle it named could not be
/// counted — the ONE reading of that split, so the create and the rotation of a bundle
/// cannot word it differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaCertificatesBundleRefusal {
    /// `CaCertificatesBundleNotFoundException`: nothing can be fetched at that address.
    NotFound,
    /// `InvalidCaCertificatesBundleException`: something is there and no CA certificates
    /// bundle can be read from it.
    Invalid,
    /// Nothing answered the read, so this emulator cannot serve the operation at all —
    /// which is not the caller's request to fix.
    Unanswered,
}

/// How many CA certificates the bundle at `location` holds, or which refusal the read of
/// it earned ([`CaCertificatesBundleRefusal`]).
///
/// The one place the read outcome is turned into this model's answer: both operations that
/// take a bundle call it, and each maps the three refusals onto its own modelled error
/// enum.
pub async fn counted_ca_certificates(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &CaCertificatesBundleLocation,
) -> Result<i32, CaCertificatesBundleRefusal> {
    match resolve_ca_certificates_bundle(ctx, region, location).await {
        CaCertificatesBundle::Counted(counted) => Ok(counted),
        CaCertificatesBundle::Absent => Err(CaCertificatesBundleRefusal::NotFound),
        CaCertificatesBundle::Invalid => Err(CaCertificatesBundleRefusal::Invalid),
        CaCertificatesBundle::Unanswered => Err(CaCertificatesBundleRefusal::Unanswered),
    }
}

/// How many CA certificates the bundle `body` holds: every PEM certificate block whose
/// contents decode as an X.509 certificate.
///
/// Both layers are read by the crate that owns the format — `rustls-pemfile` for the
/// blocks, `x509-cert` for the DER inside one — so a block that only LOOKS like a
/// certificate (a truncated structure, another kind of DER, a key) is not counted as one.
/// Bytes that are no PEM at all hold no certificate, which is the zero this answers with.
pub fn ca_certificates_in(body: &[u8]) -> i32 {
    let mut reader = ::std::io::BufReader::new(body);
    let counted = ::rustls_pemfile::certs(&mut reader)
        .filter(|block| {
            block.as_ref().is_ok_and(|der| {
                <::x509_cert::Certificate as ::x509_cert::der::Decode>::from_der(der).is_ok()
            })
        })
        .count();
    i32::try_from(counted).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bundle of `count` real self-signed CA certificates, PEM encoded one after another
    /// — what a caller stages in Amazon S3. Each certificate is GENERATED here, so the
    /// count asserted below is the number of certificates a parser produced rather than a
    /// number this test and the counter both guessed.
    fn ca_bundle(count: usize) -> String {
        (0..count)
            .map(|index| {
                let key = ::rcgen::KeyPair::generate().expect("a key pair");
                let mut params =
                    ::rcgen::CertificateParams::new(vec![format!("ca{index}.example.com")])
                        .expect("certificate parameters");
                params.is_ca = ::rcgen::IsCa::Ca(::rcgen::BasicConstraints::Unconstrained);
                params
                    .self_signed(&key)
                    .expect("a self-signed certificate")
                    .pem()
            })
            .collect()
    }

    /// RULE: what is counted is PEM certificate blocks whose contents decode as an X.509
    /// certificate — so prose, another block label and a block that is not a certificate
    /// all count for nothing, and a bundle of real certificates counts as many as it holds.
    #[test]
    fn only_decodable_certificate_blocks_are_counted() {
        let rows: &[(&str, String, i32)] = &[
            ("bytes that are no PEM at all", "not a bundle".to_string(), 0),
            (
                "a PEM block that is not a certificate",
                "-----BEGIN PUBLIC KEY-----\nMTIzNDU2Nzg5MA==\n-----END PUBLIC KEY-----\n"
                    .to_string(),
                0,
            ),
            (
                "a certificate block holding no X.509 structure",
                "-----BEGIN CERTIFICATE-----\nMTIzNDU2Nzg5MA==\n-----END CERTIFICATE-----\n"
                    .to_string(),
                0,
            ),
            ("one certificate", ca_bundle(1), 1),
            ("two certificates", ca_bundle(2), 2),
            ("five certificates", ca_bundle(5), 5),
            (
                "a certificate beside prose",
                format!("Issued by the example CA\n{}", ca_bundle(1)),
                1,
            ),
        ];
        for (case, body, expected) in rows {
            let actual = ca_certificates_in(body.as_bytes());
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected} certificates, got {actual}"
            );
        }
    }
}
