// SPDX-License-Identifier: CC0-1.0

//! Sandbox Certificate Authority for `AMAZON_ISSUED` certificates when host workloads or an
//! explicit sandbox CA directory are enabled.
//!
//! By default ACM operates no signing authority (`Certificate::certificate_body` stays `None` for
//! requested certificates, and only `ImportCertificate` populates
//! [`CertificateKeyMaterialDirectory`]). When [`SANDBOX_CA_DIR_VARIABLE`]
//! (`EMULATOR_SANDBOX_CA_DIR`) is set or `EMULATOR_HOST_WORKLOADS` is enabled (`on`, `true`, `1`),
//! this module maintains a local ECDSA P-256 Sandbox ACM Root CA (persisting `sandbox-acm-ca.pem`
//! and `sandbox-acm-ca-key.pem` in `EMULATOR_SANDBOX_CA_DIR` when given so container runtimes can
//! mount one combined CA bundle), signs real X.509 leaf certificates upon issuance, and publishes
//! their PEM material into [`CertificateKeyMaterialDirectory`] so Elastic Load Balancing HTTPS
//! listeners terminate SNI TLS with the exact issued certificate.

use std::path::PathBuf;
use std::sync::OnceLock;

use ::service_common::acm_certificate_key_material::{
    CertificateKeyMaterial, CertificateKeyMaterialDirectory,
};

use super::{Certificate, STATUS_ISSUED, TYPE_AMAZON_ISSUED};

/// Optional directory where `sandbox-acm-ca.pem` and `sandbox-acm-ca-key.pem` are persisted or
/// reused across restarts so sandbox containers can mount the root CA alongside the emulator CA.
pub const SANDBOX_CA_DIR_VARIABLE: &str = "EMULATOR_SANDBOX_CA_DIR";

const HOST_WORKLOADS_VARIABLE: &str = "EMULATOR_HOST_WORKLOADS";
const CA_CERT_FILENAME: &str = "sandbox-acm-ca.pem";
const CA_KEY_FILENAME: &str = "sandbox-acm-ca-key.pem";
const CA_COMMON_NAME: &str = "AWS Emulator Sandbox ACM Root CA";

struct SandboxAuthority {
    ca_cert: rcgen::Certificate,
    ca_key: rcgen::KeyPair,
    ca_pem: String,
}

/// Whether issued ACM certificates should be signed with sandbox TLS key material.
pub fn enabled() -> bool {
    if std::env::var(SANDBOX_CA_DIR_VARIABLE)
        .map(|val| !val.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    std::env::var(HOST_WORKLOADS_VARIABLE)
        .map(|val| matches!(val.trim().to_ascii_lowercase().as_str(), "on" | "true" | "1"))
        .unwrap_or(false)
}

fn ca_params() -> Option<rcgen::CertificateParams> {
    let mut params = rcgen::CertificateParams::new(Vec::new()).ok()?;
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, CA_COMMON_NAME);
    params.distinguished_name = dn;
    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.not_after = rcgen::date_time_ymd(2036, 1, 1);
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    Some(params)
}

fn build_or_persist_authority() -> Option<SandboxAuthority> {
    let dir = std::env::var(SANDBOX_CA_DIR_VARIABLE)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|trimmed| !trimmed.is_empty())
        .map(PathBuf::from);

    if let Some(ref dir_path) = dir {
        let key_path = dir_path.join(CA_KEY_FILENAME);
        if let Ok(existing_key_pem) = std::fs::read_to_string(&key_path) {
            if let Ok(ca_key) = rcgen::KeyPair::from_pem(&existing_key_pem) {
                if let Some(ca_cert) = ca_params().and_then(|params| params.self_signed(&ca_key).ok())
                {
                    let ca_pem = ca_cert.pem();
                    let _ = std::fs::write(dir_path.join(CA_CERT_FILENAME), &ca_pem);
                    return Some(SandboxAuthority {
                        ca_cert,
                        ca_key,
                        ca_pem,
                    });
                }
            }
        }
    }

    let ca_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).ok()?;
    let ca_cert = ca_params()?.self_signed(&ca_key).ok()?;
    let ca_pem = ca_cert.pem();

    if let Some(dir_path) = dir {
        if std::fs::create_dir_all(&dir_path).is_ok() {
            let _ = std::fs::write(dir_path.join(CA_CERT_FILENAME), &ca_pem);
            let key_path = dir_path.join(CA_KEY_FILENAME);
            let _ = std::fs::write(&key_path, ca_key.serialize_pem());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }

    Some(SandboxAuthority {
        ca_cert,
        ca_key,
        ca_pem,
    })
}

fn authority() -> Option<&'static SandboxAuthority> {
    static AUTHORITY: OnceLock<Option<SandboxAuthority>> = OnceLock::new();
    AUTHORITY.get_or_init(build_or_persist_authority).as_ref()
}

/// Mint a signed X.509 leaf certificate + ECDSA P-256 private key covering `domain_names` using
/// an explicit or process-cached sandbox CA.
pub fn mint_leaf_material(domain_names: &[String]) -> Option<CertificateKeyMaterial> {
    let auth = authority()?;
    mint_with_authority(auth, domain_names)
}

fn mint_with_authority(
    auth: &SandboxAuthority,
    domain_names: &[String],
) -> Option<CertificateKeyMaterial> {
    let mut unique_names: Vec<String> = Vec::new();
    for raw in domain_names {
        let trimmed = raw.trim().trim_end_matches('.');
        if !trimmed.is_empty()
            && !unique_names
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(trimmed))
        {
            unique_names.push(trimmed.to_string());
        }
    }
    if unique_names.is_empty() {
        return None;
    }

    let mut params = rcgen::CertificateParams::new(unique_names.clone()).ok()?;
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, &unique_names[0]);
    params.distinguished_name = dn;
    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.not_after = rcgen::date_time_ymd(2030, 1, 1);
    params.is_ca = rcgen::IsCa::ExplicitNoCa;
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![
        rcgen::ExtendedKeyUsagePurpose::ServerAuth,
        rcgen::ExtendedKeyUsagePurpose::ClientAuth,
    ];

    let leaf_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).ok()?;
    let leaf_cert = params
        .signed_by(&leaf_key, &auth.ca_cert, &auth.ca_key)
        .ok()?;

    Some(CertificateKeyMaterial {
        certificate_pem: leaf_cert.pem(),
        certificate_chain_pem: Some(auth.ca_pem.clone()),
        private_key_pem: leaf_key.serialize_pem(),
        domain_names: unique_names,
    })
}

/// Populate `certificate_body`, `certificate_chain`, and [`CertificateKeyMaterialDirectory`] for
/// any newly `ISSUED` `AMAZON_ISSUED` certificates when sandbox CA mode is enabled.
pub fn populate_issued_key_material(
    certificates: &mut std::collections::BTreeMap<String, Certificate>,
    directory: Option<&::engine::ServiceState<CertificateKeyMaterialDirectory>>,
) {
    if !enabled() {
        return;
    }
    let mut minted_entries: Vec<(String, CertificateKeyMaterial)> = Vec::new();
    for (arn, cert) in certificates.iter_mut() {
        if cert.status != STATUS_ISSUED
            || cert.cert_type != TYPE_AMAZON_ISSUED
            || cert.certificate_body.is_some()
        {
            continue;
        }
        let mut names = vec![cert.domain_name.clone()];
        for san in &cert.subject_alternative_names {
            if !names.iter().any(|existing| existing == san) {
                names.push(san.clone());
            }
        }
        if let Some(material) = mint_leaf_material(&names) {
            cert.certificate_body = Some(material.certificate_pem.clone());
            cert.certificate_chain = material.certificate_chain_pem.clone();
            minted_entries.push((arn.clone(), material));
        }
    }
    if minted_entries.is_empty() {
        return;
    }
    if let Some(dir_state) = directory {
        let mut dir = dir_state.lock();
        for (arn, material) in minted_entries {
            dir.record(&arn, material);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_leaf_covering_exact_and_wildcard_sans_under_sandbox_ca() {
        let ca_key =
            rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("ca key");
        let ca_cert = ca_params()
            .expect("ca params")
            .self_signed(&ca_key)
            .expect("ca cert");
        let auth = SandboxAuthority {
            ca_pem: ca_cert.pem(),
            ca_cert,
            ca_key,
        };

        let material = mint_with_authority(
            &auth,
            &[
                "api.sandbox.local".to_string(),
                "*.sandbox.local".to_string(),
            ],
        )
        .expect("minted leaf");

        assert!(material.certificate_pem.contains("BEGIN CERTIFICATE"));
        assert!(material
            .certificate_chain_pem
            .as_deref()
            .is_some_and(|chain| chain.contains("BEGIN CERTIFICATE")));
        assert!(material.private_key_pem.contains("BEGIN PRIVATE KEY"));
        assert!(material.covers("api.sandbox.local"));
        assert!(material.covers("web.sandbox.local"));
        assert!(!material.covers("sandbox.local"));
    }
}
