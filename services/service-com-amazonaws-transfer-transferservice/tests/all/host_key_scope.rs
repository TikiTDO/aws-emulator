// SPDX-License-Identifier: CC0-1.0

//! What a host key a caller IMPORTED reports back.
//!
//! RULE: every property `ListHostKeys` and `DescribeHostKey` report about an imported
//! key comes from that import: the `HostKeyId` it minted and the ARN under the server
//! that holds it, the fingerprint and the algorithm `Type` derived from the material it
//! carried, the instant it landed, and the members it named. A key belongs to the ONE
//! server it was imported into.
//!
//! The rows differ along what the rule generalises over: the key material (freshly
//! generated per row, so every fingerprint differs), which optional members the import
//! carried, how many keys the server already held, and which server is read. Renaming a
//! key is not a different input, so no row varies that.
//!
//! WHICH of a server's keys are host-key resources at all is
//! [`crate::server_host_key_resource`]'s subject.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{
    generated_host_key, host_key_algorithm, is_modelled_host_key_id, member_of, Emulator, ACCOUNT,
    REGION,
};
use crate::host_key_listing::{import_host_key, walk_host_keys};

/// The algorithm names `ListedHostKey.Type` and `DescribedHostKey.Type` document
/// (source: `smithy_query(query="shape",shape="ListedHostKey")`, member `Type`).
const DOCUMENTED_HOST_KEY_TYPES: [&str; 5] = [
    "ssh-rsa",
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
];

/// `HostKeyStorageType`'s two values (`smithy_query(query="shape",
/// shape="HostKeyStorageType")` -> `[SERVICE_MANAGED, SECRETS_MANAGER]`).
const SERVICE_MANAGED: &str = "SERVICE_MANAGED";
const SECRETS_MANAGER: &str = "SECRETS_MANAGER";

/// The `DescribedHostKey` of one stored key.
async fn describe_host_key(
    emulator: &Emulator,
    case: &str,
    server_id: &str,
    host_key_id: &str,
) -> Value {
    let answer = emulator
        .call(
            "DescribeHostKey",
            json!({"ServerId": server_id, "HostKeyId": host_key_id}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeHostKey 200, got {} with body {}",
        answer.status, answer.body
    );
    answer.get("HostKey").clone()
}

/// RULE: every property a host-key read reports about a key comes from that key's own
/// import — the id it minted, the ARN under the server that holds it, the fingerprint
/// and algorithm of the material, the import instant, and the `StorageType` it is stored
/// under.
///
/// The rows differ along the material (freshly generated per row, so every fingerprint
/// differs), the optional members the import carried, and how many keys the server
/// already held.
#[tokio::test]
async fn an_imported_host_key_reports_its_own_imported_properties() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    // (case, extra import members, expected Description, expected StorageType)
    let rows: Vec<(&str, Value, Option<&str>, &str)> = vec![
        ("material alone", json!({}), None, SERVICE_MANAGED),
        (
            "a description and tags",
            json!({"Description": "primary", "Tags": [{"Key": "env", "Value": "prod"}]}),
            Some("primary"),
            SERVICE_MANAGED,
        ),
        (
            "an explicit service-managed storage type",
            json!({"StorageType": SERVICE_MANAGED}),
            None,
            SERVICE_MANAGED,
        ),
        (
            "a Secrets Manager storage type",
            json!({"StorageType": SECRETS_MANAGER}),
            None,
            SECRETS_MANAGER,
        ),
    ];

    // The server already holds the key it serves with; every count below is against it.
    let own = walk_host_keys(&emulator, &server_id, None).await.len();

    let mut seen: Vec<String> = Vec::new();
    for (case, extra, description, storage_type) in rows {
        let (material, fingerprint) = generated_host_key();
        let mut request = json!({"ServerId": &server_id, "HostKeyBody": &material});
        for (member, value) in extra.as_object().expect("the row's extra is an object") {
            request[member] = value.clone();
        }
        let host_key_id = import_host_key(&emulator, case, request).await;
        assert!(
            is_modelled_host_key_id(&host_key_id),
            "case {case:?}: expected a modelled HostKeyId, got {host_key_id:?}"
        );
        assert!(
            !seen.contains(&host_key_id),
            "case {case:?}: expected a fresh id, got one already minted: {seen:?}"
        );
        seen.push(host_key_id.clone());

        let listed = walk_host_keys(&emulator, &server_id, None).await;
        assert_eq!(
            listed.len(),
            own + seen.len(),
            "case {case:?}: expected the server's own key and the {} imported ones listed, \
             got {listed:?}",
            seen.len()
        );
        let entry = listed
            .iter()
            .find(|key| member_of(key, "HostKeyId").as_str() == Some(host_key_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: expected {host_key_id} listed, got {listed:?}"))
            .clone();

        let algorithm = host_key_algorithm(&material);
        assert!(
            DOCUMENTED_HOST_KEY_TYPES.contains(&algorithm.as_str()),
            "case {case:?}: the material's own algorithm must be a documented host key \
             type, got {algorithm:?}"
        );
        let described = describe_host_key(&emulator, case, &server_id, &host_key_id).await;
        let expected_arn =
            format!("arn:aws:transfer:{REGION}:{ACCOUNT}:host-key/{server_id}/{host_key_id}");
        for (member, actual, expected) in [
            ("Arn", member_of(&entry, "Arn").as_str(), Some(expected_arn.as_str())),
            (
                "Fingerprint",
                member_of(&entry, "Fingerprint").as_str(),
                Some(fingerprint.as_str()),
            ),
            ("Type", member_of(&entry, "Type").as_str(), Some(algorithm.as_str())),
            ("Description", member_of(&entry, "Description").as_str(), description),
            ("StorageType", member_of(&entry, "StorageType").as_str(), Some(storage_type)),
        ] {
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the listed {member} to be {expected:?}, got \
                 {actual:?} in {entry}"
            );
        }
        assert!(
            member_of(&entry, "DateImported").is_number(),
            "case {case:?}: expected a DateImported on an imported key, got {entry}"
        );
        for (listed_member, described_member) in [
            ("Arn", "Arn"),
            ("HostKeyId", "HostKeyId"),
            ("Fingerprint", "HostKeyFingerprint"),
            ("Description", "Description"),
            ("Type", "Type"),
            ("DateImported", "DateImported"),
            ("StorageType", "StorageType"),
        ] {
            assert_eq!(
                member_of(&entry, listed_member),
                member_of(&described, described_member),
                "case {case:?}: expected the listing's {listed_member} to equal the \
                 described {described_member}, got {entry} against {described}"
            );
        }
    }

    // The keys of one server are not the keys of another: a second server imports the
    // same material and gets its own record for it, and the first server's set is
    // unchanged.
    let other = emulator.create_server(json!({})).await;
    let (shared, shared_fingerprint) = generated_host_key();
    let on_other = import_host_key(
        &emulator,
        "the same material on a second server",
        json!({"ServerId": &other, "HostKeyBody": &shared}),
    )
    .await;
    let on_first = import_host_key(
        &emulator,
        "the same material on the first server",
        json!({"ServerId": &server_id, "HostKeyBody": &shared}),
    )
    .await;
    assert_ne!(
        on_first, on_other,
        "case \"the same material on two servers\": expected a record per server, got one id"
    );
    let other_keys = walk_host_keys(&emulator, &other, None).await;
    assert_eq!(
        other_keys.len(),
        own + 1,
        "case \"the same material on two servers\": expected the second server's own key \
         and its one imported key, got {other_keys:?}"
    );
    assert!(
        other_keys
            .iter()
            .any(|key| member_of(key, "Fingerprint").as_str() == Some(shared_fingerprint.as_str())),
        "case \"the same material on two servers\": expected the shared key's fingerprint \
         among {other_keys:?}"
    );
    assert_eq!(
        walk_host_keys(&emulator, &server_id, None).await.len(),
        own + seen.len() + 1,
        "case \"the same material on two servers\": expected the first server's keys plus \
         the shared one"
    );
}
