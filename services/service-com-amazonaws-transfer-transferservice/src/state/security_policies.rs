// SPDX-License-Identifier: CC0-1.0

//! The service-managed security policy catalog.
//!
//! Security policies are SERVICE-managed, not per-account resources: no Transfer
//! Family operation creates, updates or deletes one. `CreateServer`/`UpdateServer`
//! and `CreateConnector`/`UpdateConnector` only ATTACH a published policy by name, and
//! `DescribeSecurityPolicy`/`ListSecurityPolicies` report the published set. So the
//! catalog is fixed data the service publishes — which is why it lives here as a
//! table rather than in [`ComAmazonawsTransferTransferserviceState`], and why nothing
//! in this crate writes to it.
//!
//! Source: the per-policy tables in the AWS Transfer Family User Guide (*Security
//! policies for AWS Transfer Family servers* and *Security policies for AWS Transfer
//! Family SFTP connectors*), whose columns are exactly `DescribedSecurityPolicy`'s
//! members — FIPS status, the protocols the policy covers and the enabled SSH ciphers,
//! SSH KEXs, SSH MACs, TLS ciphers and SSH host key algorithms
//! (`smithy_query(query="shape",shape="DescribedSecurityPolicy")`).
//!
//! Two properties of that reference data are the service's, not this crate's, and both
//! are reproduced here rather than normalised:
//!
//! * the ROW order — [`SECURITY_POLICIES`] is in the order the service LISTS the
//!   policies, which is ascending by `SecurityPolicyName` (the SFTP connector policies
//!   first, because `TransferSFTPConnectorSecurityPolicy-*` sorts ahead of
//!   `TransferSecurityPolicy-*`), so [`security_policy_names`] can be handed straight to
//!   a listing and that listing's `NextToken` can resume from the previous page's last
//!   name;
//! * the order WITHIN each algorithm list — the service publishes each list in its own
//!   negotiation-preference order (the AEAD suites ahead of the CTR ones, the ECDH
//!   exchanges ahead of the Diffie-Hellman groups), and a client that picks the first
//!   algorithm both ends support depends on it.

use super::*;

/// The `SecurityPolicyResourceType` enum's `SERVER` value: the policy is attachable to
/// a server.
pub const SECURITY_POLICY_TYPE_SERVER: &str = "SERVER";

/// The `SecurityPolicyResourceType` enum's `CONNECTOR` value: the policy is attachable
/// to an SFTP connector.
pub const SECURITY_POLICY_TYPE_CONNECTOR: &str = "CONNECTOR";

/// A server policy covers both file transfer protocols `SecurityPolicyProtocol`
/// models (`[SFTP, FTPS]`): its SSH algorithms apply to SFTP and its TLS ciphers to
/// FTPS.
pub(super) const POLICY_PROTOCOLS_SFTP_FTPS: &[&str] = &[DEFAULT_PROTOCOL, PROTOCOL_FTPS];

/// An SSH-only policy covers SFTP alone — every SFTP connector policy, and the server
/// policies that constrain SSH alone (post-quantum key exchange, SSH audit
/// compliance), which carry no TLS ciphers.
pub(super) const POLICY_PROTOCOLS_SFTP: &[&str] = &[DEFAULT_PROTOCOL];

/// The SSH ciphers the pre-2022 server policies enable: ChaCha20-Poly1305 on top of
/// the AES GCM and CTR set.
pub(super) const SSH_CIPHERS_WITH_CHACHA: &[&str] = &[
    "chacha20-poly1305@openssh.com",
    "aes128-gcm@openssh.com",
    "aes256-gcm@openssh.com",
    "aes128-ctr",
    "aes256-ctr",
    "aes192-ctr",
];

/// The AES-only SSH cipher set: the 2022-03 and later server policies, every FIPS and
/// restricted policy, and every SFTP connector policy.
pub(super) const SSH_CIPHERS_AES: &[&str] = &[
    "aes128-gcm@openssh.com",
    "aes256-gcm@openssh.com",
    "aes128-ctr",
    "aes256-ctr",
    "aes192-ctr",
];

/// The SSH ciphers an SSH-audit-compliant policy enables: the AEAD suites only, so no
/// CTR cipher, which an SSH audit reports as lacking authenticated encryption.
pub(super) const SSH_CIPHERS_AEAD_ONLY: &[&str] = &[
    "chacha20-poly1305@openssh.com",
    "aes256-gcm@openssh.com",
    "aes128-gcm@openssh.com",
];

/// The key exchanges the original 2018-11 server policy enables, including the SHA1
/// and group14 exchanges every later policy dropped.
pub(super) const SSH_KEXS_LEGACY: &[&str] = &[
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
    "curve25519-sha256",
    "curve25519-sha256@libssh.org",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group-exchange-sha256",
    "diffie-hellman-group14-sha256",
    "diffie-hellman-group14-sha1",
];

/// The key exchanges the 2020-06 and later non-FIPS policies enable: the legacy set
/// without the SHA1 and group14 exchanges.
pub(super) const SSH_KEXS_MODERN: &[&str] = &[
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
    "curve25519-sha256",
    "curve25519-sha256@libssh.org",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group-exchange-sha256",
];

/// The key exchanges the FIPS policies enable: the FIPS-approved ones only, so no
/// Curve25519.
pub(super) const SSH_KEXS_FIPS: &[&str] = &[
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group-exchange-sha256",
];

/// The key exchanges an SSH-audit-compliant policy enables: the NIST curves are not
/// among them, which is what an SSH audit asks of a key exchange.
pub(super) const SSH_KEXS_AUDIT_COMPLIANT: &[&str] = &[
    "curve25519-sha256",
    "curve25519-sha256@libssh.org",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group-exchange-sha256",
];

/// The hybrid NIST-P384 + Kyber-768 exchange the experimental post-quantum policies
/// offer first.
///
/// This published identifier is 56 characters, longer than the `len 0..50` the model
/// puts on `SecurityPolicyOption`'s member. That bound constrains a value a caller
/// SENDS, and no operation in this service takes a `SecurityPolicyOption` as input,
/// so reporting the published identifier is what lets a client match the algorithm
/// its SSH stack negotiates.
pub(super) const SSH_KEX_PQ_KYBER_768: &str =
    "ecdh-nistp384-kyber-768r3-sha384-d00@openquantumsafe.org";

/// The hybrid NIST-P256 + Kyber-512 exchange the experimental post-quantum policies
/// offer second. Same length note as [`SSH_KEX_PQ_KYBER_768`].
pub(super) const SSH_KEX_PQ_KYBER_512: &str =
    "ecdh-nistp256-kyber-512r3-sha256-d00@openquantumsafe.org";

/// The experimental post-quantum key exchanges, non-FIPS: the two hybrids in front of
/// [`SSH_KEXS_MODERN`]'s classical fallbacks.
pub(super) const SSH_KEXS_POST_QUANTUM_MODERN: &[&str] = &[
    SSH_KEX_PQ_KYBER_768,
    SSH_KEX_PQ_KYBER_512,
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
    "curve25519-sha256",
    "curve25519-sha256@libssh.org",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group-exchange-sha256",
];

/// The experimental post-quantum key exchanges, FIPS: the two hybrids in front of
/// [`SSH_KEXS_FIPS`]'s classical fallbacks.
pub(super) const SSH_KEXS_POST_QUANTUM_FIPS: &[&str] = &[
    SSH_KEX_PQ_KYBER_768,
    SSH_KEX_PQ_KYBER_512,
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group-exchange-sha256",
];

/// The message authentication codes the 2018-11 server policy enables, including the
/// SHA1 and 64-bit UMAC codes every later policy dropped.
pub(super) const SSH_MACS_LEGACY: &[&str] = &[
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512-etm@openssh.com",
    "umac-128-etm@openssh.com",
    "umac-64-etm@openssh.com",
    "hmac-sha1-etm@openssh.com",
    "hmac-sha2-256",
    "hmac-sha2-512",
    "umac-128@openssh.com",
    "umac-64@openssh.com",
    "hmac-sha1",
];

/// The SHA2 and 128-bit UMAC codes the 2020-06 through 2023-05 non-FIPS, non-restricted
/// policies enable.
pub(super) const SSH_MACS_SHA2: &[&str] = &[
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512-etm@openssh.com",
    "umac-128-etm@openssh.com",
    "hmac-sha2-256",
    "hmac-sha2-512",
    "umac-128@openssh.com",
];

/// The codes the FIPS policies through 2024-05 enable: HMAC-SHA2 only, so no UMAC.
pub(super) const SSH_MACS_FIPS: &[&str] = &[
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512-etm@openssh.com",
    "hmac-sha2-256",
    "hmac-sha2-512",
];

/// The codes the 2024-01 and later generations enable: encrypt-then-MAC HMAC-SHA2
/// only, so neither a UMAC variant nor a non-ETM code is among them.
pub(super) const SSH_MACS_ETM_SHA2: &[&str] = &[
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512-etm@openssh.com",
];

/// The codes the restricted and post-quantum policies enable: encrypt-then-MAC only.
pub(super) const SSH_MACS_ETM_ONLY: &[&str] = &[
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512-etm@openssh.com",
    "umac-128-etm@openssh.com",
];

/// The TLS ciphers the 2018-11 server policy enables, including the CBC suites every
/// later policy dropped.
pub(super) const TLS_CIPHERS_LEGACY: &[&str] = &[
    "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA",
    "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA",
];

/// The AEAD-only TLS 1.2 suites the 2020-06 through 2022-03 server policies enable.
pub(super) const TLS_CIPHERS_TLS12_GCM: &[&str] = &[
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
];

/// The suites the 2023-05 and later non-FIPS server policies enable, in the published
/// order: each ECDHE suite's ECDSA form ahead of its RSA form, the AES-GCM pair of a
/// key size ahead of the AES-CBC pair of that size, and the 128-bit sizes ahead of the
/// 256-bit ones.
pub(super) const TLS_CIPHERS_ECDHE_AES: &[&str] = &[
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA384",
];

/// The suites the generations that enable no ChaCha20-Poly1305 suite allow: the TLS
/// 1.3 and TLS 1.2 AES-GCM ones. The FIPS generations from 2023-05 on enable exactly
/// these — ChaCha20-Poly1305 is not FIPS-approved — and so does the AS2-restricted
/// generation, which restricts AS2's TLS to the same AES-GCM suites.
pub(super) const TLS_CIPHERS_TLS13_AES_ONLY: &[&str] = &[
    "TLS_AES_128_GCM_SHA256",
    "TLS_AES_256_GCM_SHA384",
    "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
];

/// The host key algorithms a connector accepts from the remote SFTP server under a
/// non-FIPS connector policy. `SshHostKeyAlgorithms` "only applies to security
/// policies for connectors", so every server policy leaves it empty.
pub(super) const HOST_KEY_ALGORITHMS_CONNECTOR: &[&str] = &[
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "rsa-sha2-256",
    "rsa-sha2-512",
    "ssh-ed25519",
    "ssh-rsa",
];

/// The host key algorithms a FIPS connector policy accepts: Ed25519 is not
/// FIPS-approved, so it is not among them.
pub(super) const HOST_KEY_ALGORITHMS_CONNECTOR_FIPS: &[&str] = &[
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "rsa-sha2-256",
    "rsa-sha2-512",
];

/// No list for this member under this policy — the member is absent from the
/// description rather than reported empty (TLS ciphers on an SSH-only policy, host key
/// algorithms on a server policy).
pub(super) const NO_OPTIONS: &[&str] = &[];

/// One published security policy, as `DescribeSecurityPolicy` reports it.
#[derive(Debug, Clone, Copy)]
pub struct SecurityPolicy {
    /// `SecurityPolicyName`: the policy's text name, the value a server or connector
    /// stores when it attaches this policy.
    pub name: &'static str,
    /// `Type`: [`SECURITY_POLICY_TYPE_SERVER`] or
    /// [`SECURITY_POLICY_TYPE_CONNECTOR`] — the resource kind the policy attaches to.
    pub resource_type: &'static str,
    /// `Fips`: whether the policy enables Federal Information Processing Standards
    /// algorithms only.
    pub fips: bool,
    /// `Protocols`: the file transfer protocols the policy's algorithms apply to.
    pub protocols: &'static [&'static str],
    /// `SshCiphers`.
    pub ssh_ciphers: &'static [&'static str],
    /// `SshKexs`.
    pub ssh_kexs: &'static [&'static str],
    /// `SshMacs`.
    pub ssh_macs: &'static [&'static str],
    /// `TlsCiphers` — empty for an SSH-only policy, which reports no TLS ciphers.
    pub tls_ciphers: &'static [&'static str],
    /// `SshHostKeyAlgorithms` — empty for a server policy, which reports none.
    pub ssh_host_key_algorithms: &'static [&'static str],
}

/// Every security policy this service publishes, sorted by `SecurityPolicyName`.
///
/// Sorted because that is the order the service lists them in: the SFTP connector
/// policies first (`TransferSFTPConnectorSecurityPolicy-*` sorts ahead of
/// `TransferSecurityPolicy-*`), then the server policies, ascending by name within each.
/// `ListSecurityPolicies` pages over this order with a token that resumes from the
/// previous page's last name, and [`security_policy`] is a lookup by name.
pub const SECURITY_POLICIES: &[SecurityPolicy] = &[
    // ---- SFTP connector policies: SSH only, so no TLS ciphers, and they are the
    // only policies that report host key algorithms. ----
    SecurityPolicy {
        name: "TransferSFTPConnectorSecurityPolicy-2023-07",
        resource_type: SECURITY_POLICY_TYPE_CONNECTOR,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_SHA2,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: HOST_KEY_ALGORITHMS_CONNECTOR,
    },
    SecurityPolicy {
        name: "TransferSFTPConnectorSecurityPolicy-2024-03",
        resource_type: SECURITY_POLICY_TYPE_CONNECTOR,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_SHA2,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: HOST_KEY_ALGORITHMS_CONNECTOR,
    },
    SecurityPolicy {
        // The current FIPS connector generation: like the 2024-03 connector policy,
        // narrowed to the FIPS-approved key exchanges, codes and host key algorithms.
        name: "TransferSFTPConnectorSecurityPolicy-FIPS-2024-10",
        resource_type: SECURITY_POLICY_TYPE_CONNECTOR,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_ETM_SHA2,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: HOST_KEY_ALGORITHMS_CONNECTOR_FIPS,
    },
    // ---- Server policies. ----
    SecurityPolicy {
        name: "TransferSecurityPolicy-2018-11",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_WITH_CHACHA,
        ssh_kexs: SSH_KEXS_LEGACY,
        ssh_macs: SSH_MACS_LEGACY,
        tls_ciphers: TLS_CIPHERS_LEGACY,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-2020-06",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_WITH_CHACHA,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_SHA2,
        tls_ciphers: TLS_CIPHERS_TLS12_GCM,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-2022-03",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_SHA2,
        tls_ciphers: TLS_CIPHERS_TLS12_GCM,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-2023-05",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_SHA2,
        tls_ciphers: TLS_CIPHERS_ECDHE_AES,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        // The policy a server gets when `CreateServer` names none; see
        // [`DEFAULT_SECURITY_POLICY_NAME`].
        name: DEFAULT_SECURITY_POLICY_NAME,
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_ETM_SHA2,
        tls_ciphers: TLS_CIPHERS_ECDHE_AES,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        // The current non-FIPS server generation: the 2024-01 algorithm sets with the
        // TLS 1.3 suites, and the first one a new server should be moved onto.
        name: "TransferSecurityPolicy-2025-03",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_ETM_SHA2,
        tls_ciphers: TLS_CIPHERS_ECDHE_AES,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        // The AS2-restricted generation: AS2 rides on TLS, so this policy narrows the
        // TLS suites and keeps the current SSH sets for the server's SFTP endpoint.
        name: "TransferSecurityPolicy-AS2Restricted-2025-07",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_ETM_SHA2,
        tls_ciphers: TLS_CIPHERS_TLS13_AES_ONLY,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-FIPS-2020-06",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_FIPS,
        tls_ciphers: TLS_CIPHERS_TLS12_GCM,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-FIPS-2023-05",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_FIPS,
        tls_ciphers: TLS_CIPHERS_TLS13_AES_ONLY,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-FIPS-2024-01",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_ETM_SHA2,
        tls_ciphers: TLS_CIPHERS_TLS13_AES_ONLY,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-FIPS-2024-05",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_ETM_SHA2,
        tls_ciphers: TLS_CIPHERS_TLS13_AES_ONLY,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        // The current FIPS server generation.
        name: "TransferSecurityPolicy-FIPS-2025-03",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_FIPS,
        ssh_macs: SSH_MACS_ETM_SHA2,
        tls_ciphers: TLS_CIPHERS_TLS13_AES_ONLY,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-PQ-SSH-Experimental-2023-04",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        // Post-quantum key exchange is an SSH mechanism, so this policy covers SFTP
        // alone and reports no TLS ciphers.
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_POST_QUANTUM_MODERN,
        ssh_macs: SSH_MACS_ETM_ONLY,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-PQ-SSH-FIPS-Experimental-2023-04",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: true,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_POST_QUANTUM_FIPS,
        ssh_macs: SSH_MACS_FIPS,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-Restricted-2018-11",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_ETM_ONLY,
        tls_ciphers: TLS_CIPHERS_TLS12_GCM,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        name: "TransferSecurityPolicy-Restricted-2020-06",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP_FTPS,
        ssh_ciphers: SSH_CIPHERS_AES,
        ssh_kexs: SSH_KEXS_MODERN,
        ssh_macs: SSH_MACS_ETM_ONLY,
        tls_ciphers: TLS_CIPHERS_TLS12_GCM,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
    SecurityPolicy {
        // The SSH-audit-compliant generation: it constrains SSH alone, so it covers
        // SFTP and reports no TLS ciphers, and it enables only the algorithms an SSH
        // audit accepts — the AEAD ciphers, the non-NIST key exchanges and the
        // encrypt-then-MAC codes.
        name: "TransferSecurityPolicy-SshAuditCompliant-2025-02",
        resource_type: SECURITY_POLICY_TYPE_SERVER,
        fips: false,
        protocols: POLICY_PROTOCOLS_SFTP,
        ssh_ciphers: SSH_CIPHERS_AEAD_ONLY,
        ssh_kexs: SSH_KEXS_AUDIT_COMPLIANT,
        ssh_macs: SSH_MACS_ETM_ONLY,
        tls_ciphers: NO_OPTIONS,
        ssh_host_key_algorithms: NO_OPTIONS,
    },
];

/// The published policy named `name`, or `None` when the service publishes none.
///
/// Exact match: `SecurityPolicyName` is a text name the caller copies from the
/// catalog, and the real service does not fold case.
pub fn security_policy(name: &str) -> Option<&'static SecurityPolicy> {
    SECURITY_POLICIES.iter().find(|policy| policy.name == name)
}

/// Every published policy name, in catalog order — what a listing pages over.
pub fn security_policy_names() -> impl Iterator<Item = &'static str> {
    SECURITY_POLICIES.iter().map(|policy| policy.name)
}

/// One `SecurityPolicyOptions` member of a description: the listed algorithms, or
/// `None` when this policy has no such list.
///
/// A policy that enables nothing for a member reports the member as ABSENT rather
/// than as an empty list, because the member is modelled optional and the real
/// service only reports a list where it applies — `TlsCiphers` "only applies to
/// security policies for servers", `SshHostKeyAlgorithms` "only applies to security
/// policies for connectors".
pub fn policy_options(options: &[&str]) -> Option<Vec<String>> {
    (!options.is_empty()).then(|| options.iter().map(|option| (*option).to_string()).collect())
}

/// `ResourceType` for a security policy, addressed by its `SecurityPolicyName`. Same
/// convention as [`RESOURCE_TYPE_SERVER`]; it lives beside the catalog rather than
/// beside the per-account resource types because a policy is published, not created.
pub const RESOURCE_TYPE_SECURITY_POLICY: &str = "SecurityPolicy";

/// A `SecurityPolicyName` the service publishes no policy for. The text names the
/// policy resource as the service's own wording does, like every other entry in the
/// not-found table.
pub const NOT_FOUND_SECURITY_POLICY: NotFoundResource = NotFoundResource {
    resource_type: RESOURCE_TYPE_SECURITY_POLICY,
    message: "Unknown SecurityPolicy",
};

/// The not-found payload for a `SecurityPolicyName` the service publishes no policy
/// for. Built through [`resource_not_found`] like every other not-found in this
/// service, so the body cannot drift from them; "not found" here means "not in
/// [`SECURITY_POLICIES`]", never "not created yet".
pub fn unknown_security_policy(security_policy_name: &str) -> ResourceNotFoundException {
    resource_not_found(&NOT_FOUND_SECURITY_POLICY, security_policy_name)
}
