// SPDX-License-Identifier: CC0-1.0

//! What the published security policy catalog PUBLISHES: which policies, and each
//! policy's own algorithm lists in the order the service lists them.
//!
//! RULE: `ListSecurityPolicies` and `DescribeSecurityPolicy` report service-published
//! reference data as published — the policies the service publishes, in the order it
//! lists them, and for each policy the exact member set of each algorithm list in its
//! published preference order (the AEAD suites ahead of the CTR ones, the NIST ECDH
//! exchanges ahead of the Diffie-Hellman groups), never re-sorted and never widened with
//! an algorithm the policy does not enable. A client reads these lists to decide whether
//! a policy permits its negotiation and picks the first algorithm both ends support, so
//! both the set and the order are part of the answer.
//!
//! The rows differ along what the rule generalises over: the policy generation, the
//! resource type it attaches to (server, SFTP connector), its FIPS status and which
//! member is read. The generations exercised here are the ones the catalog publishes
//! beside the default, including the entries no other suite in this crate names.
//!
//! Source for the published sets: the per-policy tables in the AWS Transfer Family User
//! Guide (*Security policies for AWS Transfer Family servers*, *...for AWS Transfer
//! Family SFTP connectors*), whose columns are `DescribedSecurityPolicy`'s members
//! (`smithy_query(query="shape",shape="DescribedSecurityPolicy")`).
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_transfer_transferservice::state::security_policy_names;

use crate::harness::{member_of, Emulator};

/// The algorithm members of a description that carry an ordered preference list.
const ALGORITHM_MEMBERS: [&str; 5] = [
    "SshCiphers",
    "SshKexs",
    "SshMacs",
    "TlsCiphers",
    "SshHostKeyAlgorithms",
];

/// The published generations a client is expected to be able to select now. Each must be
/// listed and must describe; they are the catalog entries the rest of this crate's
/// suites do not name.
const CURRENT_GENERATIONS: [&str; 5] = [
    "TransferSecurityPolicy-2025-03",
    "TransferSecurityPolicy-AS2Restricted-2025-07",
    "TransferSecurityPolicy-FIPS-2025-03",
    "TransferSecurityPolicy-SshAuditCompliant-2025-02",
    "TransferSFTPConnectorSecurityPolicy-FIPS-2024-10",
];

/// Describe one policy over the wire, expecting the published row back.
async fn describe(emulator: &Emulator, name: &str) -> Value {
    let answer = emulator
        .call(
            "DescribeSecurityPolicy",
            json!({"SecurityPolicyName": name}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: expected DescribeSecurityPolicy 200, got {} with body {}",
        answer.status, answer.body
    );
    answer.get("SecurityPolicy").clone()
}

/// One algorithm list of a description, as a `Vec` of strings in the order reported.
fn listed(policy: &Value, member: &str) -> Vec<String> {
    member_of(policy, member)
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

/// The index of the first entry satisfying `pick`, if any.
fn position(entries: &[String], pick: impl Fn(&str) -> bool) -> Option<usize> {
    entries.iter().position(|entry| pick(entry))
}

/// RULE: each published policy reports the EXACT member set of each algorithm list it
/// enables, in the published order.
///
/// The rows name, per policy and member, the whole list that policy publishes — so an
/// answer that re-sorts it, drops an entry or adds one the policy does not enable fails.
/// They cover a current server generation, a current FIPS one, the SSH-audit-compliant
/// one, a restricted one and a connector one, none of which the other suites assert.
#[tokio::test]
async fn each_published_policy_reports_its_published_list_in_order() {
    // (policy, member, the whole published list in published order)
    let rows: [(&str, &str, &[&str]); 9] = [
        // The current non-FIPS server generation: AES only, encrypt-then-MAC only.
        (
            "TransferSecurityPolicy-2025-03",
            "SshCiphers",
            &[
                "aes128-gcm@openssh.com",
                "aes256-gcm@openssh.com",
                "aes128-ctr",
                "aes256-ctr",
                "aes192-ctr",
            ],
        ),
        (
            "TransferSecurityPolicy-2025-03",
            "SshMacs",
            &[
                "hmac-sha2-256-etm@openssh.com",
                "hmac-sha2-512-etm@openssh.com",
            ],
        ),
        (
            "TransferSecurityPolicy-2025-03",
            "SshKexs",
            &[
                "ecdh-sha2-nistp256",
                "ecdh-sha2-nistp384",
                "ecdh-sha2-nistp521",
                "curve25519-sha256",
                "curve25519-sha256@libssh.org",
                "diffie-hellman-group18-sha512",
                "diffie-hellman-group16-sha512",
                "diffie-hellman-group-exchange-sha256",
            ],
        ),
        // The current FIPS server generation: no Curve25519 key exchange, and no
        // ChaCha20-Poly1305 TLS suite.
        (
            "TransferSecurityPolicy-FIPS-2025-03",
            "SshKexs",
            &[
                "ecdh-sha2-nistp256",
                "ecdh-sha2-nistp384",
                "ecdh-sha2-nistp521",
                "diffie-hellman-group18-sha512",
                "diffie-hellman-group16-sha512",
                "diffie-hellman-group-exchange-sha256",
            ],
        ),
        // The current non-FIPS generation's TLS suites, in the published order: each
        // suite's ECDSA form ahead of its RSA form, GCM ahead of CBC, 128 ahead of 256.
        (
            "TransferSecurityPolicy-2025-03",
            "TlsCiphers",
            &[
                "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
                "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
                "TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA256",
                "TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA256",
                "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
                "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
                "TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA384",
                "TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA384",
            ],
        ),
        (
            "TransferSecurityPolicy-FIPS-2025-03",
            "TlsCiphers",
            &[
                "TLS_AES_128_GCM_SHA256",
                "TLS_AES_256_GCM_SHA384",
                "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
                "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
                "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
                "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
            ],
        ),
        // The SSH-audit-compliant generation: AEAD ciphers only, and no NIST curve.
        (
            "TransferSecurityPolicy-SshAuditCompliant-2025-02",
            "SshCiphers",
            &[
                "chacha20-poly1305@openssh.com",
                "aes256-gcm@openssh.com",
                "aes128-gcm@openssh.com",
            ],
        ),
        (
            "TransferSecurityPolicy-SshAuditCompliant-2025-02",
            "SshKexs",
            &[
                "curve25519-sha256",
                "curve25519-sha256@libssh.org",
                "diffie-hellman-group18-sha512",
                "diffie-hellman-group16-sha512",
                "diffie-hellman-group-exchange-sha256",
            ],
        ),
        // A connector policy is the only kind that publishes host key algorithms.
        (
            "TransferSFTPConnectorSecurityPolicy-FIPS-2024-10",
            "SshHostKeyAlgorithms",
            &[
                "ecdsa-sha2-nistp256",
                "ecdsa-sha2-nistp384",
                "ecdsa-sha2-nistp521",
                "rsa-sha2-256",
                "rsa-sha2-512",
            ],
        ),
    ];

    let emulator = Emulator::new();
    for (name, member, published) in rows {
        let case = format!("{name} {member}");
        let policy = describe(&emulator, name).await;
        let reported = listed(&policy, member);
        assert_eq!(
            reported,
            published
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: expected the published list in published order, got {reported:?}"
        );

        // A read reports; it must answer the same thing twice.
        let again = describe(&emulator, name).await;
        assert_eq!(
            listed(&again, member),
            reported,
            "case {case:?}: a second read answered a different list"
        );
    }
}

/// RULE: the published preference order holds across the catalog, not only in the rows
/// above — wherever a policy enables both kinds, the AEAD cipher suites precede the CTR
/// ones and the NIST ECDH key exchanges precede the Diffie-Hellman groups.
///
/// The rows are every published policy, read off the wire, so a policy the catalog gains
/// is covered without touching this test.
#[tokio::test]
async fn the_published_preference_order_holds_for_every_policy() {
    let emulator = Emulator::new();

    for name in security_policy_names() {
        let policy = describe(&emulator, name).await;

        let ciphers = listed(&policy, "SshCiphers");
        let last_aead = ciphers
            .iter()
            .rposition(|cipher| cipher.contains("gcm@") || cipher.starts_with("chacha20"));
        let first_ctr = position(&ciphers, |cipher| cipher.ends_with("-ctr"));
        if let (Some(last_aead), Some(first_ctr)) = (last_aead, first_ctr) {
            assert!(
                last_aead < first_ctr,
                "case {name:?}: expected every AEAD cipher before the CTR ones, got {ciphers:?}"
            );
        }

        let kexs = listed(&policy, "SshKexs");
        let last_ecdh = kexs
            .iter()
            .rposition(|kex| kex.starts_with("ecdh-sha2-nistp"));
        let first_dh = position(&kexs, |kex| kex.starts_with("diffie-hellman"));
        if let (Some(last_ecdh), Some(first_dh)) = (last_ecdh, first_dh) {
            assert!(
                last_ecdh < first_dh,
                "case {name:?}: expected every NIST ECDH exchange before the \
                 Diffie-Hellman groups, got {kexs:?}"
            );
        }

        // No list a policy publishes repeats an algorithm, and none is empty: an empty
        // list is reported as an ABSENT member instead.
        for member in ALGORITHM_MEMBERS {
            let entries = listed(&policy, member);
            if member_of(&policy, member).is_null() {
                assert!(
                    entries.is_empty(),
                    "case {name:?} {member}: an absent member must list nothing"
                );
                continue;
            }
            assert!(
                !entries.is_empty(),
                "case {name:?} {member}: a reported list must carry at least one algorithm"
            );
            let mut deduped = entries.clone();
            deduped.sort();
            deduped.dedup();
            assert_eq!(
                deduped.len(),
                entries.len(),
                "case {name:?} {member}: expected no algorithm listed twice, got {entries:?}"
            );
        }
    }
}

/// RULE: the catalog publishes the generations a client can select now, and the listing
/// answers them in the catalog's own order — a client that picks a name out of the
/// listing can attach it and describe it.
#[tokio::test]
async fn the_current_generations_are_published_and_selectable() {
    let emulator = Emulator::new();
    let answer = emulator.call("ListSecurityPolicies", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "expected ListSecurityPolicies 200, got {} with body {}",
        answer.status, answer.body
    );
    let listed_names: Vec<String> = answer
        .get("SecurityPolicyNames")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    let published: Vec<String> = security_policy_names().map(ToString::to_string).collect();
    assert_eq!(
        listed_names, published,
        "expected the listing to answer the catalog in its own order, got {listed_names:?}"
    );
    // The catalog's order IS ascending by name, which is what lets a `NextToken` resume
    // from the previous page's last name.
    let mut ascending = published.clone();
    ascending.sort();
    ascending.dedup();
    assert_eq!(
        ascending, published,
        "expected the catalog in strictly ascending name order, got {published:?}"
    );

    for name in CURRENT_GENERATIONS {
        assert!(
            listed_names.iter().any(|listed| listed == name),
            "case {name:?}: expected the current generation to be listed, got {listed_names:?}"
        );
        let policy = describe(&emulator, name).await;
        assert_eq!(
            member_of(&policy, "SecurityPolicyName").as_str(),
            Some(name),
            "case {name:?}: expected the described policy to be the one named, got {policy}"
        );

        // A server policy a client selects is attachable, and the server reports it.
        if !name.contains("ConnectorSecurityPolicy") {
            let server_id = emulator
                .create_server(json!({"SecurityPolicyName": name}))
                .await;
            let described = emulator
                .call("DescribeServer", json!({"ServerId": &server_id}))
                .await;
            assert_eq!(
                described.status, 200,
                "case {name:?}: expected DescribeServer 200, got {} with body {}",
                described.status, described.body
            );
            assert_eq!(
                member_of(described.get("Server"), "SecurityPolicyName").as_str(),
                Some(name),
                "case {name:?}: expected the server to report the policy it was created \
                 with, got {}",
                described.body
            );
        }
    }
}
