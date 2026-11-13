// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The one TLS certificate every listener in this emulator process presents.
//!
//! The data port, every `listeners()` listener and the signed-URL listener all serve this
//! certificate, so a client that trusts it once (standalone: the published `ca.pem` or
//! `truststore.p12`; embedded: [`TlsCertificateAuthority::pem`] and friends) can reach every
//! endpoint the emulator hands out. It is self-signed: the certificate is its own root.

use std::net::IpAddr;
use std::sync::OnceLock;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// The TLS root of this emulator process: the certificate every listener presents.
///
/// There is one per process, made on first use. Only the public certificate is reachable here;
/// the private key never leaves this crate.
pub struct TlsCertificateAuthority {
    der: CertificateDer<'static>,
    pem: String,
    key_der: Vec<u8>,
    names: Vec<String>,
}

static PROCESS: OnceLock<TlsCertificateAuthority> = OnceLock::new();

impl TlsCertificateAuthority {
    /// The certificate of this process, made on first use.
    ///
    /// # Panics
    ///
    /// When the process cannot generate a key pair, which leaves it no TLS at all.
    pub fn for_this_process() -> &'static Self {
        Self::covering(None).expect("the emulator could not generate its TLS certificate")
    }

    /// The certificate of this process, made on first use for `localhost`, `127.0.0.1`, `::1`,
    /// the `EMULATOR_BIND_ADDRESS` interface and `client_host` when they name one address.
    ///
    /// The first call decides the names, so a `client_host` the certificate was not made for is
    /// not added later: check [`Self::covers`].
    pub(crate) fn covering(client_host: Option<IpAddr>) -> Result<&'static Self, rcgen::Error> {
        if let Some(identity) = crate::boot_identity::retained_or_ephemeral() {
            return Ok(&identity.tls);
        }
        if let Some(made) = PROCESS.get() {
            return Ok(made);
        }
        let mut names: Vec<String> = ["localhost", "127.0.0.1", "::1"].map(String::from).into();
        let bound = crate::runtime_profile::network()
            .ok()
            .map(|profile| profile.bind_address());
        for ip in [bound, client_host].into_iter().flatten() {
            let name = ip.to_string();
            if !ip.is_unspecified() && !names.contains(&name) {
                names.push(name);
            }
        }
        let _ = PROCESS.set(Self::generate(names)?);
        Ok(PROCESS.get().expect("set above"))
    }

    pub(crate) fn generate(names: Vec<String>) -> Result<Self, rcgen::Error> {
        let made = rcgen::generate_simple_self_signed(names.clone())?;
        Ok(Self {
            der: made.cert.der().clone(),
            pem: made.cert.pem(),
            key_der: made.key_pair.serialize_der(),
            names,
        })
    }

    pub(crate) fn private_key_der(&self) -> &[u8] {
        &self.key_der
    }

    // Only the private boot identity owner calls this after native key/name/time validation.
    pub(crate) fn restored(der: Vec<u8>, key_der: Vec<u8>, names: Vec<String>) -> Self {
        let encoded = aws_smithy_types::base64::encode(&der);
        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for chunk in encoded.as_bytes().chunks(64) {
            // Base64 is ASCII, so these bytes are always valid UTF-8.
            for byte in chunk {
                pem.push(char::from(*byte));
            }
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
        Self {
            der: CertificateDer::from(der),
            pem,
            key_der,
            names,
        }
    }

    /// The certificate in DER, for a rustls, tokio-rustls or hyper-rustls root store, or
    /// `reqwest::Certificate::from_der`.
    pub fn der(&self) -> &CertificateDer<'static> {
        &self.der
    }

    /// The certificate as PEM text, for `AWS_CA_BUNDLE`, `requests(verify=..)` or
    /// `curl --cacert`. The same bytes as the `ca.pem` a standalone emulator publishes.
    pub fn pem(&self) -> String {
        self.pem.clone()
    }

    /// A password-less PKCS#12 trust store holding only this certificate, for
    /// `-Djavax.net.ssl.trustStore`. The same bytes as the `truststore.p12` a standalone
    /// emulator publishes; see [`crate::java_truststore`] for what a JVM pointed at it trusts.
    pub fn pkcs12_trust_store(&self) -> Vec<u8> {
        crate::java_truststore::emulator_only(&self.der)
    }

    /// Whether the certificate names `ip`, so a client dialing it verifies the hostname.
    pub(crate) fn covers(&self, ip: IpAddr) -> bool {
        self.names.contains(&ip.to_string())
    }

    /// A rustls server config presenting this certificate, with no client auth and no ALPN;
    /// each listener sets the protocols it offers.
    pub(crate) fn server_config(&self) -> Result<rustls::ServerConfig, rustls::Error> {
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key_der.clone()));
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![self.der.clone()], key)
    }
}

/// Names the addresses the certificate covers; the key is never printed.
impl std::fmt::Debug for TlsCertificateAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsCertificateAuthority")
            .field("names", &self.names)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_certificate_serves_the_whole_process_and_covers_loopback() {
        let first = TlsCertificateAuthority::for_this_process();
        let again = TlsCertificateAuthority::covering(Some("10.9.8.7".parse().unwrap())).unwrap();
        assert!(
            std::ptr::eq(first, again),
            "a second caller gets the same certificate"
        );
        for ip in ["127.0.0.1", "::1"] {
            assert!(first.covers(ip.parse().unwrap()), "{ip}");
        }
        assert!(
            !first.covers("10.9.8.7".parse().unwrap()),
            "names are fixed by the first call"
        );
    }

    #[test]
    fn the_pem_and_trust_store_carry_the_der_certificate() {
        let ca = TlsCertificateAuthority::for_this_process();
        assert!(ca.pem().starts_with("-----BEGIN CERTIFICATE-----"));
        assert_eq!(
            ca.pkcs12_trust_store(),
            crate::java_truststore::emulator_only(ca.der())
        );
        assert!(
            !format!("{ca:?}").contains("key_der"),
            "the key stays out of the debug form"
        );
    }

    #[test]
    fn the_server_config_presents_the_certificate() {
        let ca = TlsCertificateAuthority::for_this_process();
        let config = ca.server_config().expect("the key matches the certificate");
        assert!(
            config.alpn_protocols.is_empty(),
            "each listener chooses its own protocols"
        );
    }
}
