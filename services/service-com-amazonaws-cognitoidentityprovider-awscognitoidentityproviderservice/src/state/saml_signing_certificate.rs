// SPDX-License-Identifier: CC0-1.0

//! The X.509 certificate a user pool signs its SAML 2.0 federation traffic with.
//!
//! `GetSigningCertificate` "returns the signing certificate for SAML 2.0
//! federation" of the pool it names, and the member it answers carries "the
//! x.509 certificate that signs SAML 2.0 authentication requests for your user
//! pool" (this service's model) — so what the member carries has to be a
//! certificate the SAML identity provider on the other side can decode, parse
//! and install as the pool's signature verifier, not text in the shape of one.
//! That is what every identity provider configured for a Cognito user pool does
//! with it: the operator copies the value into the provider's trust
//! configuration, where it is read as the base64 of a DER X.509 certificate —
//! the same coding a SAML metadata `X509Certificate` element carries.
//!
//! A child module of [`super`] rather than more of `state.rs`: issuing that
//! certificate is one concern, and the private half of its key pair is generated
//! here, used to sign the certificate, and dropped. Nothing stores or reports
//! it — no shape of this service has a member for a private key — so a pool
//! publishes only the certificate.

use super::*;

/// How long an issued signing certificate is valid: "Issued certificates are
/// valid for 10 years from the date of issue" (`GetSigningCertificate` in this
/// service's model), counted as the 3652 days ten Gregorian years span.
pub const SIGNING_CERTIFICATE_VALIDITY: Duration = Duration::from_secs(3652 * 24 * 60 * 60);

/// The SAML 2.0 service-provider entity id of the user pool `user_pool_id`:
/// `urn:amazon:cognito:sp:<UserPoolId>`, the name a pool identifies itself under
/// in the SAML exchange and therefore the subject its signing certificate is
/// issued to (Amazon Cognito Developer Guide, "Adding SAML identity providers to
/// a user pool", which has the operator configure that URN as the provider's
/// audience).
#[must_use]
pub fn saml_service_provider_entity_id(user_pool_id: &str) -> String {
    format!("urn:amazon:cognito:sp:{user_pool_id}")
}

/// The signing certificate of the pool `user_pool_id`, issued at `issued_at`:
/// the base64 of a REAL self-signed X.509 certificate, issued to that pool's
/// SAML service-provider entity id ([`saml_service_provider_entity_id`]) and
/// valid for [`SIGNING_CERTIFICATE_VALIDITY`] from the date of issue.
///
/// Issued ONCE, when the pool is created, and kept on the pool's record: the
/// operation that reports it renders the stored value, so every read of one pool
/// answers the same certificate, and two pools answer different ones because
/// each is issued to its own entity id under its own freshly generated key.
///
/// `None` when the certificate does not issue, which the caller reports rather
/// than storing a pool whose signing certificate is a value nothing signed.
#[must_use]
pub fn issue_signing_certificate(user_pool_id: &str, issued_at: SystemTime) -> Option<String> {
    let certificate = signing_certificate_der(user_pool_id, issued_at);
    match certificate {
        Some(der) => Some(::aws_smithy_types::base64::encode(der)),
        None => {
            // The caller answers its modelled server error, which says nothing
            // about WHY: the generator's own failure is worth a line so the path
            // is diagnosable if it ever fires.
            ::tracing::warn!(
                user_pool_id,
                "the user pool's SAML signing certificate could not be issued"
            );
            None
        }
    }
}

/// The DER-encoded certificate itself, which [`issue_signing_certificate`] is
/// the base64 of.
///
/// The key pair is generated here and its private half is dropped with this
/// function's frame: signing a SAML authentication request is the pool's own
/// work, and nothing this service stores or reports can present the key.
fn signing_certificate_der(user_pool_id: &str, issued_at: SystemTime) -> Option<Vec<u8>> {
    let key = ::rcgen::KeyPair::generate().ok()?;
    // No subject alternative name: the pool is named by a URN entity id rather
    // than by a host it serves, so there is no DNS name or address to certify.
    let mut params = ::rcgen::CertificateParams::new(Vec::<String>::new()).ok()?;
    let mut subject = ::rcgen::DistinguishedName::new();
    subject.push(
        ::rcgen::DnType::CommonName,
        saml_service_provider_entity_id(user_pool_id),
    );
    params.distinguished_name = subject;
    params.not_before = offset_date_time(issued_at)?;
    params.not_after = offset_date_time(issued_at.checked_add(SIGNING_CERTIFICATE_VALIDITY)?)?;
    // An end-entity certificate, marked explicitly as not a certificate
    // authority: it signs the pool's own SAML messages and certifies no other
    // certificate.
    params.is_ca = ::rcgen::IsCa::ExplicitNoCa;
    params.key_usages = vec![::rcgen::KeyUsagePurpose::DigitalSignature];
    Some(params.self_signed(&key).ok()?.der().to_vec())
}

/// `at` as the date-time type the certificate generator takes a validity bound
/// in, at the one-second resolution an X.509 validity period is stated at.
///
/// `None` for an instant outside the range that type represents, which is the
/// same answer the caller gives for a certificate that does not issue.
fn offset_date_time(at: SystemTime) -> Option<::time::OffsetDateTime> {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since_epoch| i64::try_from(since_epoch.as_secs()).ok())?;
    ::time::OffsetDateTime::from_unix_timestamp(seconds).ok()
}
