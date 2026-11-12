// SPDX-License-Identifier: CC0-1.0

//! The crate's own feature probe: the key formats this service's dependencies give it.
//!
//! RULE: every property this service reports about a host key is read off real key
//! material, so the key formats its dependencies implement are part of its contract:
//! the OpenSSH container must read as an Ed25519 key and the generator must produce an
//! RSA one. A dependency rebuilt without one of those features would answer a reader
//! with no algorithm at all, which no modelled `HostKeyType` is — this suite fails
//! instead of letting that reach a response.
//!
//! What the SERVICE does with that material is
//! [`crate::host_key_private_key_containers`]'s subject.

use service_com_amazonaws_transfer_transferservice::state::{mint_host_key, read_host_key};

use crate::harness::generated_host_key;

/// The algorithm names `HostKeyType` documents (source:
/// `smithy_query(query="shape",shape="DescribedHostKey")`, member `Type`).
const DOCUMENTED_HOST_KEY_TYPES: [&str; 5] = [
    "ssh-rsa",
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
];

/// RULE (see the module docs): both key sources this service reads report a documented
/// algorithm, a `ssh-keygen -l` fingerprint and an OpenSSH public half.
#[test]
fn every_key_source_reports_a_documented_algorithm() {
    let (material, fingerprint) = generated_host_key();
    let read = read_host_key(&material).expect("the OpenSSH container must read");
    let generated = mint_host_key().expect("the service generates a host key");

    for (case, key, expected_type) in [
        ("a key in the OpenSSH container", &read, "ssh-ed25519"),
        ("the key the service generates", &generated, "ssh-rsa"),
    ] {
        assert!(
            DOCUMENTED_HOST_KEY_TYPES.contains(&key.key_type.as_str()),
            "case {case:?}: expected one of {DOCUMENTED_HOST_KEY_TYPES:?}, got {key:?}"
        );
        assert_eq!(
            key.key_type, expected_type,
            "case {case:?}: expected {expected_type:?}, got {key:?}"
        );
        assert!(
            key.fingerprint.starts_with("SHA256:"),
            "case {case:?}: expected the fingerprint ssh-keygen -l prints, got {key:?}"
        );
        assert!(
            key.public_key.starts_with(&key.key_type),
            "case {case:?}: expected an OpenSSH public key of that algorithm, got {key:?}"
        );
    }

    assert_eq!(
        read.fingerprint, fingerprint,
        "expected the material's own fingerprint to be the one the key has"
    );
}
