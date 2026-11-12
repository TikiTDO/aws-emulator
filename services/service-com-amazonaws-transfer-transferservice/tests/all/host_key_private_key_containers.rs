// SPDX-License-Identifier: CC0-1.0

//! Which private-key CONTAINERS the service reads host key material in, and what the
//! key the service generates for a server itself is.
//!
//! RULE: host key material is read in every container the service's own documentation
//! tells a caller to produce — `CreateServer`'s `HostKey` documents `ssh-keygen -t rsa
//! -b 2048 -N "" -m PEM`, `-t ecdsa -b 256 -N "" -m PEM` and `-t ed25519`, and
//! `ImportHostKey`'s `HostKeyBody` "accepts RSA, ECDSA, and ED25519 keys", so the PEM
//! containers `-m PEM` and `-m PKCS8` write and the OpenSSH container the plain
//! invocation writes all carry material a caller may send — and the algorithm `Type`,
//! the fingerprint and the public half reported for it are computed from the key inside,
//! never from the container. Material in no container the service reads is refused
//! rather than replaced by a key the caller never supplied. The key the SERVICE generates
//! for a server created without one is an RSA key, as the service's own documented
//! generation command writes, and it is that server's own.
//!
//! The rows differ along what the rule generalises over: the container (PKCS#1 PEM,
//! SEC1 PEM, PKCS#8 PEM, the OpenSSH one), the key inside it (a second modulus, each
//! documented algorithm class), the member the material arrives in (`ImportHostKey`'s
//! `HostKeyBody`, `CreateServer`'s and `UpdateServer`'s `HostKey`) and, for the
//! refusals, the way the material is not a key.
//!
//! Each row's expectation is computed from the key's own material with the OpenSSH
//! key-format crate, independently of the handler under test.

use serde_json::json;
use service_com_amazonaws_transfer_transferservice::state::{mint_host_key, read_host_key};

use crate::harness::{generated_host_key, member_of, Emulator};

/// The PEM label of a PKCS#1 RSA private key (`ssh-keygen -t rsa -m PEM`).
const LABEL_RSA_PRIVATE_KEY: &str = "RSA PRIVATE KEY";

/// The PEM label of a PKCS#8 private key (`ssh-keygen -m PKCS8`).
const LABEL_PRIVATE_KEY: &str = "PRIVATE KEY";

/// The PEM label of a SEC1 EC private key (`ssh-keygen -t ecdsa -m PEM`).
const LABEL_EC_PRIVATE_KEY: &str = "EC PRIVATE KEY";

/// The `id-Ed25519` algorithm identifier a PKCS#8 container names an Ed25519 key with
/// (RFC 8410).
const OID_ED25519: pkcs8::ObjectIdentifier = pkcs8::ObjectIdentifier::new_unwrap("1.3.101.112");

/// The `rsaEncryption` algorithm identifier a PKCS#8 container names an RSA key with
/// (RFC 8017).
const OID_RSA_ENCRYPTION: pkcs8::ObjectIdentifier =
    pkcs8::ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");

/// The PKCS#1 `RSAPrivateKey` of the key with this modulus and exponent, DER-encoded by
/// the crate that implements the structure. The private fields are the structure's
/// remaining integers; what identifies the key to a reader of its PUBLIC half — and so
/// everything a host-key read reports — is the modulus and the exponent.
fn pkcs1_der(modulus: &[u8], exponent: &[u8]) -> Vec<u8> {
    use pkcs1::der::Encode;

    let one = pkcs1::UintRef::new(&[1]).expect("one");
    let key = pkcs1::RsaPrivateKey {
        modulus: pkcs1::UintRef::new(modulus).expect("the modulus"),
        public_exponent: pkcs1::UintRef::new(exponent).expect("the exponent"),
        private_exponent: one,
        prime1: one,
        prime2: one,
        exponent1: one,
        exponent2: one,
        coefficient: one,
        other_prime_infos: None,
    };
    key.to_der().expect("the PKCS#1 DER")
}

/// The same key in a PKCS#8 `PrivateKeyInfo` naming `rsaEncryption`.
fn pkcs8_der(modulus: &[u8], exponent: &[u8]) -> Vec<u8> {
    use pkcs8::der::Encode;

    let inner = pkcs1_der(modulus, exponent);
    let info = pkcs8::PrivateKeyInfo {
        algorithm: pkcs8::AlgorithmIdentifierRef {
            oid: OID_RSA_ENCRYPTION,
            parameters: Some(pkcs8::der::asn1::AnyRef::NULL),
        },
        private_key: &inner,
        public_key: None,
    };
    info.to_der().expect("the PKCS#8 DER")
}

/// PEM armour around `der`, written by the crate that implements the container.
fn pem(label: &str, der: &[u8]) -> String {
    pkcs1::pem::encode_string(label, pkcs1::pem::LineEnding::LF, der).expect("the PEM")
}

/// The fingerprint `ssh-keygen -l` prints for the RSA key with these numbers, computed
/// here from the key itself rather than from the code under test.
fn expected_rsa_fingerprint(modulus: &[u8], exponent: &[u8]) -> String {
    let public = ssh_key::public::RsaPublicKey {
        e: ssh_key::Mpint::from_positive_bytes(exponent).expect("the exponent"),
        n: ssh_key::Mpint::from_positive_bytes(modulus).expect("the modulus"),
    };
    ssh_key::PublicKey::new(ssh_key::public::KeyData::Rsa(public), "")
        .fingerprint(ssh_key::HashAlg::Sha256)
        .to_string()
}

/// A 2048-bit modulus, built so that its leading byte keeps it positive for the DER
/// integer the structure holds it in, and so the rows differ in every byte.
fn modulus(seed: u8) -> Vec<u8> {
    (0..256u32)
        .map(|index| (u8::try_from(index % 251).expect("below 251")) ^ seed | 0x01)
        .collect()
}

/// A real ECDSA P-256 key pair, in the PKCS#8 PEM container `ssh-keygen -m PKCS8` writes
/// and in the SEC1 one `-m PEM` writes, with the `ecdsa-sha2-nistp256` fingerprint the
/// key itself has.
fn ecdsa_key() -> (String, String, String) {
    let generated = rcgen::KeyPair::generate().expect("an ECDSA P-256 key pair");
    let pkcs8 = generated.serialize_der();
    let info = pkcs8::PrivateKeyInfo::try_from(pkcs8.as_slice()).expect("the PKCS#8 key");
    // A PKCS#8 EC key's `privateKey` octets ARE the SEC1 structure, so the same key in
    // the SEC1 container is that inner DER under the SEC1 label.
    let sec1 = pem(LABEL_EC_PRIVATE_KEY, info.private_key);
    let ec = sec1::EcPrivateKey::try_from(info.private_key).expect("the SEC1 key");
    let public = ssh_key::public::EcdsaPublicKey::from_sec1_bytes(
        ec.public_key.expect("the SEC1 public point"),
    )
    .expect("the ECDSA public key");
    let fingerprint = ssh_key::PublicKey::new(ssh_key::public::KeyData::Ecdsa(public), "")
        .fingerprint(ssh_key::HashAlg::Sha256)
        .to_string();
    (pem(LABEL_PRIVATE_KEY, &pkcs8), sec1, fingerprint)
}

/// A real Ed25519 key pair in the PKCS#8 PEM container, with the fingerprint that key
/// has. The seed is the key's own: the container is built from the generated key rather
/// than from a literal.
fn ed25519_pkcs8_key() -> (String, String) {
    use pkcs8::der::Encode;

    let keypair = ssh_key::private::Ed25519Keypair::random(&mut ssh_key::rand_core::OsRng);
    let seed = keypair.private.to_bytes();
    let inner = pkcs8::der::asn1::OctetStringRef::new(&seed)
        .expect("the seed")
        .to_der()
        .expect("the seed's DER");
    let info = pkcs8::PrivateKeyInfo {
        algorithm: pkcs8::AlgorithmIdentifierRef {
            oid: OID_ED25519,
            parameters: None,
        },
        private_key: &inner,
        public_key: None,
    };
    let fingerprint =
        ssh_key::PublicKey::new(ssh_key::public::KeyData::Ed25519(keypair.public), "")
            .fingerprint(ssh_key::HashAlg::Sha256)
            .to_string();
    (
        pem(
            LABEL_PRIVATE_KEY,
            &info.to_der().expect("the PKCS#8 DER"),
        ),
        fingerprint,
    )
}

/// RULE (see the module docs): a key in a PEM container reads as the key inside it,
/// whichever member carried it, and reports that key's own algorithm and fingerprint.
#[tokio::test]
async fn host_key_material_in_a_pem_container_reads_as_its_own_key() {
    let emulator = Emulator::new();
    let (ecdsa_pkcs8, ecdsa_sec1, ecdsa_fingerprint) = ecdsa_key();
    let (ed25519_pkcs8, ed25519_fingerprint) = ed25519_pkcs8_key();
    let (openssh, openssh_fingerprint) = generated_host_key();

    // (case, the material, the algorithm the key inside it is, the fingerprint it has)
    let rows: Vec<(&str, String, &str, String)> = vec![
        (
            "a PKCS#1 RSA private key",
            pem(LABEL_RSA_PRIVATE_KEY, &pkcs1_der(&modulus(0x11), &[1, 0, 1])),
            "ssh-rsa",
            expected_rsa_fingerprint(&modulus(0x11), &[1, 0, 1]),
        ),
        (
            "a PKCS#1 RSA private key with another modulus",
            pem(LABEL_RSA_PRIVATE_KEY, &pkcs1_der(&modulus(0x5c), &[1, 0, 1])),
            "ssh-rsa",
            expected_rsa_fingerprint(&modulus(0x5c), &[1, 0, 1]),
        ),
        (
            "a PKCS#8 RSA private key",
            pem(LABEL_PRIVATE_KEY, &pkcs8_der(&modulus(0x23), &[1, 0, 1])),
            "ssh-rsa",
            expected_rsa_fingerprint(&modulus(0x23), &[1, 0, 1]),
        ),
        (
            "a SEC1 ECDSA P-256 private key",
            ecdsa_sec1,
            "ecdsa-sha2-nistp256",
            ecdsa_fingerprint.clone(),
        ),
        (
            "a PKCS#8 ECDSA P-256 private key",
            ecdsa_pkcs8,
            "ecdsa-sha2-nistp256",
            ecdsa_fingerprint,
        ),
        (
            "a PKCS#8 Ed25519 private key",
            ed25519_pkcs8,
            "ssh-ed25519",
            ed25519_fingerprint,
        ),
        (
            "an Ed25519 key in the OpenSSH container",
            openssh,
            "ssh-ed25519",
            openssh_fingerprint,
        ),
    ];

    for (case, material, algorithm, fingerprint) in rows {
        // Read directly: the three properties come from the key inside the container.
        let read = read_host_key(&material)
            .unwrap_or_else(|| panic!("case {case:?}: expected the material to read"));
        assert_eq!(
            (read.key_type.as_str(), read.fingerprint.as_str()),
            (algorithm, fingerprint.as_str()),
            "case {case:?}: expected the key's own algorithm and fingerprint, got {read:?}"
        );

        // ...through `ImportHostKey`'s `HostKeyBody`...
        let server_id = emulator.create_server(json!({})).await;
        let imported = emulator
            .call(
                "ImportHostKey",
                json!({"ServerId": &server_id, "HostKeyBody": &material}),
            )
            .await;
        assert_eq!(
            imported.status, 200,
            "case {case:?}: expected ImportHostKey 200, got {} with body {}",
            imported.status, imported.body
        );
        let host_key_id = imported
            .get("HostKeyId")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a HostKeyId"))
            .to_string();
        let described = emulator
            .call(
                "DescribeHostKey",
                json!({"ServerId": &server_id, "HostKeyId": &host_key_id}),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeHostKey 200, got {} with body {}",
            described.status, described.body
        );
        let record = described.get("HostKey").clone();
        assert_eq!(
            member_of(&record, "Type").as_str(),
            Some(algorithm),
            "case {case:?}: expected the imported key's own algorithm, got {record}"
        );
        assert_eq!(
            member_of(&record, "HostKeyFingerprint").as_str(),
            Some(fingerprint.as_str()),
            "case {case:?}: expected the imported key's own fingerprint, got {record}"
        );

        // ...and through `CreateServer`'s `HostKey`, which reports the same key, as does
        // the `UpdateServer` that rotates a server onto it.
        let other = emulator.create_server(json!({"HostKey": &material})).await;
        let server = emulator
            .call("DescribeServer", json!({"ServerId": &other}))
            .await;
        assert_eq!(
            member_of(server.get("Server"), "HostKeyFingerprint").as_str(),
            Some(fingerprint.as_str()),
            "case {case:?}: expected the server to report the supplied key's own \
             fingerprint, got {}",
            server.body
        );

        let rotated = emulator.create_server(json!({})).await;
        let updated = emulator
            .call(
                "UpdateServer",
                json!({"ServerId": &rotated, "HostKey": &material}),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected UpdateServer 200, got {} with body {}",
            updated.status, updated.body
        );
        let server = emulator
            .call("DescribeServer", json!({"ServerId": &rotated}))
            .await;
        assert_eq!(
            member_of(server.get("Server"), "HostKeyFingerprint").as_str(),
            Some(fingerprint.as_str()),
            "case {case:?}: expected the rotated server to report the supplied key's own \
             fingerprint, got {}",
            server.body
        );
    }
}

/// RULE: material in no container the service reads is REFUSED — the server is never
/// given a key its caller did not supply, and no id is minted for one.
#[tokio::test]
async fn material_in_no_readable_container_is_refused() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    // Rows: a PEM label this service implements no reader for, a readable label whose
    // payload is not that structure, a PKCS#8 container naming an algorithm the service
    // reads no key of, text in no container at all, and the empty body `HostKeyBody`'s
    // modelled `len 0..4096` admits.
    let rows: Vec<(&str, String)> = vec![
        (
            "a PEM label the service reads no key from",
            pem("DSA PRIVATE KEY", &pkcs1_der(&modulus(0x44), &[1, 0, 1])),
        ),
        (
            "a readable label whose payload is not that structure",
            pem(LABEL_RSA_PRIVATE_KEY, b"not a key at all"),
        ),
        (
            "a PKCS#8 container naming an algorithm the service reads no key of",
            pem(LABEL_PRIVATE_KEY, &pkcs8_dsa_der()),
        ),
        ("no container at all", "ssh-rsa AAAAB3NzaC1".to_string()),
        ("the empty body", String::new()),
    ];

    for (case, material) in rows {
        assert!(
            read_host_key(&material).is_none(),
            "case {case:?}: expected material the service cannot read"
        );

        let imported = emulator
            .call(
                "ImportHostKey",
                json!({"ServerId": &server_id, "HostKeyBody": &material}),
            )
            .await;
        assert_eq!(
            (imported.status, imported.error_code().as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected 400 InvalidRequestException, got {} with body {}",
            imported.status,
            imported.body
        );
        assert!(
            imported.get("HostKeyId").is_null(),
            "case {case:?}: expected a refusal to mint no id, got {}",
            imported.body
        );

        let created = emulator.call("CreateServer", json!({"HostKey": &material})).await;
        assert_eq!(
            (created.status, created.error_code().as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected CreateServer to refuse the same material, got {} \
             with body {}",
            created.status,
            created.body
        );
        assert!(
            created.get("ServerId").is_null(),
            "case {case:?}: expected a refused create to mint no server, got {}",
            created.body
        );

        // ...and `UpdateServer` too: a rotation onto material the service cannot read is
        // refused rather than dropped, so the server keeps the key it was serving with.
        let serving = member_of(
            emulator
                .call("DescribeServer", json!({"ServerId": &server_id}))
                .await
                .get("Server"),
            "HostKeyFingerprint",
        )
        .as_str()
        .unwrap_or_default()
        .to_string();
        let updated = emulator
            .call(
                "UpdateServer",
                json!({"ServerId": &server_id, "HostKey": &material}),
            )
            .await;
        assert_eq!(
            (updated.status, updated.error_code().as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected UpdateServer to refuse the same material, got {} \
             with body {}",
            updated.status,
            updated.body
        );
        assert_eq!(
            member_of(
                emulator
                    .call("DescribeServer", json!({"ServerId": &server_id}))
                    .await
                    .get("Server"),
                "HostKeyFingerprint"
            )
            .as_str(),
            Some(serving.as_str()),
            "case {case:?}: expected the refused rotation to leave the server serving \
             with the key it had"
        );
    }
}

/// A PKCS#8 container naming an algorithm this service reads no key of (DSA), so the
/// refusal is about the algorithm rather than about the container being unreadable.
fn pkcs8_dsa_der() -> Vec<u8> {
    use pkcs8::der::Encode;

    let inner = pkcs1_der(&modulus(0x6e), &[1, 0, 1]);
    let info = pkcs8::PrivateKeyInfo {
        algorithm: pkcs8::AlgorithmIdentifierRef {
            // id-dsa (RFC 3279).
            oid: pkcs8::ObjectIdentifier::new_unwrap("1.2.840.10040.4.1"),
            parameters: None,
        },
        private_key: &inner,
        public_key: None,
    };
    info.to_der().expect("the PKCS#8 DER")
}

/// RULE: the key the SERVICE generates for a server created without one is an RSA key,
/// per server, and it is one of that server's host keys.
#[tokio::test]
async fn a_generated_host_key_is_a_per_server_rsa_key() {
    // Generated directly: a real RSA key, and a different one each time.
    let first = mint_host_key().expect("a generated host key");
    let second = mint_host_key().expect("a generated host key");
    for (case, key) in [("the first", &first), ("the second", &second)] {
        assert_eq!(
            key.key_type, "ssh-rsa",
            "case {case:?}: expected a generated RSA key, got {key:?}"
        );
        assert!(
            key.public_key.starts_with("ssh-rsa "),
            "case {case:?}: expected an OpenSSH RSA public key, got {key:?}"
        );
        assert!(
            key.fingerprint.starts_with("SHA256:"),
            "case {case:?}: expected the fingerprint ssh-keygen -l prints, got {key:?}"
        );
    }
    assert_ne!(
        first.fingerprint, second.fingerprint,
        "expected a generated key per server"
    );

    // ...and over the wire: two servers created with no host key report RSA keys of
    // their own, each listed as that server's host key.
    let emulator = Emulator::new();
    let (material, supplied_fingerprint) = generated_host_key();
    let mut fingerprints: Vec<String> = Vec::new();
    for (case, request, expected_type) in [
        ("the first generated key", json!({}), "ssh-rsa"),
        ("the second generated key", json!({}), "ssh-rsa"),
        // A create that SUPPLIES material reports that key instead, whatever its
        // algorithm: the service generates one only where the caller named none.
        (
            "a supplied Ed25519 key",
            json!({"HostKey": &material}),
            "ssh-ed25519",
        ),
    ] {
        let server_id = emulator.create_server(request).await;
        let listed = emulator
            .call("ListHostKeys", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected ListHostKeys 200, got {} with body {}",
            listed.status, listed.body
        );
        let keys = listed
            .get("HostKeys")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: expected the server's one host key, got {}",
            listed.body
        );
        assert_eq!(
            member_of(&keys[0], "Type").as_str(),
            Some(expected_type),
            "case {case:?}: expected Type {expected_type:?}, got {}",
            listed.body
        );
        let fingerprint = member_of(&keys[0], "Fingerprint")
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Fingerprint"))
            .to_string();
        assert!(
            !fingerprints.contains(&fingerprint),
            "case {case:?}: expected a key of this server's own, got one already seen"
        );
        fingerprints.push(fingerprint);
    }
    assert_eq!(
        fingerprints.last().map(String::as_str),
        Some(supplied_fingerprint.as_str()),
        "case \"a supplied Ed25519 key\": expected the supplied key's own fingerprint"
    );
}
