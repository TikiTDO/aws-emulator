// SPDX-License-Identifier: CC0-1.0

//! `DescribeSecurityPolicy` over the whole published security policy catalog.
//!
//! RULE: a request naming a security policy the service publishes is answered with
//! that policy's own published properties — its name, the resource type it attaches
//! to, whether it is FIPS, the protocols it covers and its enabled SSH ciphers, SSH
//! key exchanges, SSH MACs and (where the policy has them) TLS ciphers and SSH host
//! key algorithms — while a name the service publishes no policy for is refused with
//! the modelled `ResourceNotFoundException`. The read reports; it changes nothing.
//!
//! The rows differ along what that rule generalises over: the resource type
//! (server vs SFTP connector), the FIPS status, the policy generation (which decides
//! which algorithms are enabled), and whether the policy covers one protocol or two.
//! Every published policy is exercised, not only the default one, and the two error
//! tables cover a name that is well-formed but unpublished and a name the modelled
//! pattern refuses.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_transfer_transferservice::state::{
    security_policy_names, DEFAULT_SECURITY_POLICY_NAME,
};

use crate::harness::{member_of, Answer, Emulator};

/// The only values `SecurityPolicyProtocol` models
/// (`smithy_query(query="shape",shape="SecurityPolicyProtocol")` -> `[SFTP, FTPS]`).
const MODELLED_PROTOCOLS: [&str; 2] = ["SFTP", "FTPS"];

/// The only values `SecurityPolicyResourceType` models
/// (`smithy_query(query="shape",shape="SecurityPolicyResourceType")`).
const TYPE_SERVER: &str = "SERVER";
const TYPE_CONNECTOR: &str = "CONNECTOR";

/// `Protocols` is modelled `len 1..5`.
const MAX_PROTOCOLS: usize = 5;

/// Primitives a FIPS policy must not enable: ChaCha20, Curve25519, UMAC, SHA-1 and
/// Ed25519 are not FIPS-approved, so no algorithm a FIPS policy lists may name one.
const NON_FIPS_PRIMITIVES: [&str; 5] = ["chacha20", "curve25519", "umac", "sha1", "ed25519"];

/// The three algorithm lists EVERY policy enables, whatever it attaches to.
const ALWAYS_PRESENT_LISTS: [&str; 3] = ["SshCiphers", "SshKexs", "SshMacs"];

/// Describe `name` over the wire.
async fn describe(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "DescribeSecurityPolicy",
            json!({"SecurityPolicyName": name}),
        )
        .await
}

/// A JSON array of strings as a `Vec`, or an empty one when the member is absent.
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .unwrap_or_else(|| panic!("expected a list of strings, got {item:?}"))
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every algorithm the description lists, from all five algorithm members.
fn all_algorithms(policy: &Value) -> Vec<String> {
    [
        "SshCiphers",
        "SshKexs",
        "SshMacs",
        "TlsCiphers",
        "SshHostKeyAlgorithms",
    ]
    .iter()
    .flat_map(|member| strings(member_of(policy, member)))
    .collect()
}

/// Assert the rule over one described policy, with no per-policy expectation baked
/// in: every claim is checked against the policy's OWN reported members.
fn assert_catalog_invariants(case: &str, requested: &str, policy: &Value) {
    assert_eq!(
        member_of(policy, "SecurityPolicyName").as_str(),
        Some(requested),
        "case {case:?}: expected the described policy to be the one named, got {policy}"
    );

    // The published naming carries both facts a client needs before it reads an
    // algorithm list: a connector policy is named `TransferSFTPConnectorSecurityPolicy-*`,
    // and a FIPS policy carries FIPS in its name.
    let expected_type = if requested.contains("ConnectorSecurityPolicy") {
        TYPE_CONNECTOR
    } else {
        TYPE_SERVER
    };
    let reported_type = member_of(policy, "Type").as_str().unwrap_or_default();
    assert_eq!(
        reported_type, expected_type,
        "case {case:?}: expected Type {expected_type:?}, got {reported_type:?}"
    );

    let expected_fips = requested.contains("FIPS");
    let reported_fips = member_of(policy, "Fips");
    assert_eq!(
        reported_fips.as_bool(),
        Some(expected_fips),
        "case {case:?}: expected Fips {expected_fips}, got {reported_fips:?}"
    );

    for member in ALWAYS_PRESENT_LISTS {
        let listed = strings(member_of(policy, member));
        assert!(
            !listed.is_empty(),
            "case {case:?}: expected {member} to list at least one algorithm, got {:?}",
            member_of(policy, member)
        );
    }

    let protocols = strings(member_of(policy, "Protocols"));
    assert!(
        !protocols.is_empty() && protocols.len() <= MAX_PROTOCOLS,
        "case {case:?}: expected 1..{MAX_PROTOCOLS} protocols, got {protocols:?}"
    );
    for protocol in &protocols {
        assert!(
            MODELLED_PROTOCOLS.contains(&protocol.as_str()),
            "case {case:?}: expected a protocol in {MODELLED_PROTOCOLS:?}, got {protocol:?}"
        );
    }

    // TLS ciphers are what an FTPS listener negotiates, so a policy reports them
    // exactly when it covers FTPS; an SSH-only policy reports the member as absent
    // rather than empty.
    let covers_ftps = protocols.iter().any(|protocol| protocol == "FTPS");
    let tls_ciphers = member_of(policy, "TlsCiphers");
    assert_eq!(
        !tls_ciphers.is_null(),
        covers_ftps,
        "case {case:?}: protocols {protocols:?} but TlsCiphers {tls_ciphers:?}"
    );
    assert_eq!(
        strings(tls_ciphers).is_empty(),
        !covers_ftps,
        "case {case:?}: expected TlsCiphers to be listed only for an FTPS policy, got {tls_ciphers:?}"
    );

    // `SshHostKeyAlgorithms` "only applies to security policies for connectors"
    // (the member's own modelled doc), so a server policy reports it absent.
    let host_keys = member_of(policy, "SshHostKeyAlgorithms");
    assert_eq!(
        !host_keys.is_null(),
        reported_type == TYPE_CONNECTOR,
        "case {case:?}: Type {reported_type:?} but SshHostKeyAlgorithms {host_keys:?}"
    );

    if expected_fips {
        for algorithm in all_algorithms(policy) {
            let lowered = algorithm.to_lowercase();
            for primitive in NON_FIPS_PRIMITIVES {
                assert!(
                    !lowered.contains(primitive),
                    "case {case:?}: a FIPS policy must not enable {primitive:?}, \
                     but it lists {algorithm:?}"
                );
            }
        }
    }
}

/// Every published policy describes, reports its own properties, and reports them the
/// same on a second read.
#[tokio::test]
async fn every_published_policy_describes_from_the_catalog() {
    let emulator = Emulator::new();
    let names: Vec<&str> = security_policy_names().collect();
    assert!(
        names.len() > 1,
        "the catalog must publish more than one policy, got {names:?}"
    );

    let mut seen: Vec<String> = Vec::new();
    for name in &names {
        let answer = describe(&emulator, name).await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let policy = answer.get("SecurityPolicy").clone();
        assert_catalog_invariants(name, name, &policy);

        // A read reports; it must not change what the next caller sees.
        let again = describe(&emulator, name).await;
        assert_eq!(
            again.body, answer.body,
            "case {name:?}: a second read answered differently: {} then {}",
            answer.body, again.body
        );

        // One policy per name, and no two names describing the same row.
        assert!(
            !seen.contains(&policy.to_string()),
            "case {name:?}: two published names describe the same policy: {policy}"
        );
        seen.push(policy.to_string());
    }

    // The catalog is reported, not synthesised: the rows differ from one another.
    assert!(
        seen.len() == names.len(),
        "expected one distinct description per published name, got {} for {}",
        seen.len(),
        names.len()
    );
}

/// Each policy generation reports the algorithms IT enables.
///
/// The rows name one algorithm the generation is known for and one it is known to
/// have dropped (source: the per-policy tables in the AWS Transfer Family User Guide,
/// *Security policies for AWS Transfer Family servers* / *...for AWS Transfer Family
/// SFTP connectors*), so a handler that answered one fixed row for every name fails.
#[tokio::test]
async fn each_policy_generation_reports_its_own_algorithms() {
    // (policy, member, must list, must not list)
    let cases: [(&str, &str, &[&str], &[&str]); 8] = [
        // The original server policy still enables the SHA1 key exchange and MAC.
        (
            "TransferSecurityPolicy-2018-11",
            "SshKexs",
            &["diffie-hellman-group14-sha1"],
            &[],
        ),
        (
            "TransferSecurityPolicy-2018-11",
            "SshMacs",
            &["hmac-sha1"],
            &[],
        ),
        // 2020-06 dropped the SHA1 algorithms and kept ChaCha20-Poly1305.
        (
            "TransferSecurityPolicy-2020-06",
            "SshKexs",
            &["curve25519-sha256"],
            &["diffie-hellman-group14-sha1"],
        ),
        (
            "TransferSecurityPolicy-2020-06",
            "SshCiphers",
            &["chacha20-poly1305@openssh.com"],
            &[],
        ),
        // 2025-03 enables the AES ciphers only, as every generation since 2022 does.
        (
            "TransferSecurityPolicy-2025-03",
            "SshCiphers",
            &["aes256-gcm@openssh.com"],
            &["chacha20-poly1305@openssh.com"],
        ),
        // The current default enables the AEAD suites and no SHA1 one.
        (
            DEFAULT_SECURITY_POLICY_NAME,
            "TlsCiphers",
            &[
                "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
                "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
            ],
            &["TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA"],
        ),
        // A connector policy reports the host key algorithms a connector accepts.
        (
            "TransferSFTPConnectorSecurityPolicy-2024-03",
            "SshHostKeyAlgorithms",
            &["ssh-ed25519", "rsa-sha2-512"],
            &[],
        ),
        // The experimental post-quantum policy offers the hybrid Kyber exchanges.
        (
            "TransferSecurityPolicy-PQ-SSH-Experimental-2023-04",
            "SshKexs",
            &["ecdh-nistp384-kyber-768r3-sha384-d00@openquantumsafe.org"],
            &[],
        ),
    ];

    let emulator = Emulator::new();
    for (policy_name, member, must_list, must_not_list) in cases {
        let case = format!("{policy_name} {member}");
        let answer = describe(&emulator, policy_name).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let policy = answer.get("SecurityPolicy").clone();
        assert_catalog_invariants(&case, policy_name, &policy);

        let listed = strings(member_of(&policy, member));
        for algorithm in must_list {
            assert!(
                listed.iter().any(|entry| entry == algorithm),
                "case {case:?}: expected {algorithm:?} to be enabled, got {listed:?}"
            );
        }
        for algorithm in must_not_list {
            assert!(
                !listed.iter().any(|entry| entry == algorithm),
                "case {case:?}: expected {algorithm:?} NOT to be enabled, got {listed:?}"
            );
        }
    }
}

/// A well-formed name the service publishes no policy for is refused with the modelled
/// not-found error, with no identifier in the message — the same shape every other
/// not-found in this service answers.
#[tokio::test]
async fn an_unpublished_policy_name_is_not_found() {
    // Each satisfies `SecurityPolicyName`'s modelled pattern
    // `^Transfer[A-Za-z0-9]*SecurityPolicy-[A-Za-z0-9-]+$` and `len 0..100`: a
    // published family with an unpublished date, an unpublished family, a
    // lower-cased spelling of a published name's suffix, and one at the length bound.
    let unpublished = [
        "TransferSecurityPolicy-1999-01",
        "TransferMadeUpSecurityPolicy-2024-01",
        "TransferSecurityPolicy-fips-2024-05",
        "TransferSecurityPolicy-aaaaaaaaaabbbbbbbbbbccccccccccddddddddddeeeeeeeeeeffffffffffgggggg",
    ];

    let emulator = Emulator::new();
    for name in unpublished {
        let answer = describe(&emulator, name).await;
        assert_eq!(
            answer.status, 400,
            "case {name:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got body {}",
            answer.body
        );
        assert_eq!(
            member_of(&answer.body, "ResourceType").as_str(),
            Some("SecurityPolicy"),
            "case {name:?}: expected ResourceType SecurityPolicy, got body {}",
            answer.body
        );
        assert_eq!(
            member_of(&answer.body, "Resource").as_str(),
            Some(name),
            "case {name:?}: expected Resource to be the name asked for, got body {}",
            answer.body
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.contains(name),
            "case {name:?}: the message must carry no identifier, got {message:?}"
        );
    }
}

/// A name outside the modelled pattern is refused at the boundary as a validation
/// failure — never as a not-found, and never as the scaffold's 501.
#[tokio::test]
async fn a_malformed_policy_name_is_a_validation_failure() {
    // Each breaks `^Transfer[A-Za-z0-9]*SecurityPolicy-[A-Za-z0-9-]+$`: a bare word,
    // a published name with trailing whitespace, a published name with its separator
    // missing, and a name with a character the pattern's class excludes.
    let malformed = [
        "nonsense",
        "TransferSecurityPolicy-2024-01 ",
        "TransferSecurityPolicy",
        "TransferSecurityPolicy-2024_01",
    ];

    let emulator = Emulator::new();
    for name in malformed {
        let answer = describe(&emulator, name).await;
        assert_eq!(
            answer.status, 400,
            "case {name:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {name:?}: expected ValidationException, got body {}",
            answer.body
        );
    }
}

/// The policy a server actually carries is describable: whatever `SecurityPolicyName`
/// `DescribeServer` reports — the create-time default, or a named one — names a
/// published policy, and describing it answers that same name.
#[tokio::test]
async fn the_policy_a_server_carries_is_describable() {
    // (case, CreateServer request) — one server taking the create-time default, one
    // naming a non-FIPS policy explicitly, one naming a FIPS policy.
    let cases = [
        ("create-time default", json!({})),
        (
            "explicit non-FIPS policy",
            json!({"SecurityPolicyName": "TransferSecurityPolicy-2025-03"}),
        ),
        (
            "explicit FIPS policy",
            json!({"SecurityPolicyName": "TransferSecurityPolicy-FIPS-2025-03"}),
        ),
    ];

    let emulator = Emulator::new();
    for (case, request) in cases {
        let server_id = emulator.create_server(request).await;
        let described = emulator
            .call("DescribeServer", json!({"ServerId": server_id}))
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: DescribeServer expected 200, got {} with body {}",
            described.status, described.body
        );
        let attached = member_of(described.get("Server"), "SecurityPolicyName")
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the server reported no SecurityPolicyName: {}",
                    described.body
                )
            })
            .to_string();

        let answer = describe(&emulator, &attached).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: describing the attached policy {attached:?} expected 200, \
             got {} with body {}",
            answer.status, answer.body
        );
        assert_catalog_invariants(case, &attached, answer.get("SecurityPolicy"));
    }

    // The default a server with no `SecurityPolicyName` gets is itself published.
    let default = describe(&emulator, DEFAULT_SECURITY_POLICY_NAME).await;
    assert_eq!(
        default.status, 200,
        "the default policy {DEFAULT_SECURITY_POLICY_NAME:?} must be published, got {} with body {}",
        default.status, default.body
    );
}
