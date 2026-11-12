// SPDX-License-Identifier: CC0-1.0

//! `ListHostKeys`: the host keys a server holds, read back over the wire.
//!
//! RULE: `ListHostKeys` reports exactly the host-key records stored for the server it
//! names — "a list of host keys for the server that's specified by the ServerId
//! parameter" — one entry per stored key, each projected from the SAME record
//! `DescribeHostKey` reports, in `HostKeyId` order, paged through the opaque
//! `NextToken` every `List*` in this service hands out. A server holds the key it serves
//! with and every key imported into it, a key that is deleted stops being listed, and an
//! unknown server is refused.
//!
//! The rows differ along what the import carried (description, tags, storage type, the
//! key's own material), which server is read, and whether the key is still stored — the
//! lifecycle of one key is [`crate::host_key_lifecycle`]'s subject, the page sizes are
//! [`crate::host_keys`]', and this suite is about what the LISTING answers over the
//! set.

use serde_json::{json, Value};

use crate::harness::{
    generated_host_key, host_key_algorithm, is_modelled_host_key_id, member_of, Emulator, ACCOUNT,
    REGION,
};

/// The algorithm names `ListedHostKey.Type` documents: "the encryption algorithm that
/// is used for the host key. The Type parameter is specified by using one of the
/// following values: ssh-rsa, ssh-ed25519, ecdsa-sha2-nistp256, ecdsa-sha2-nistp384,
/// ecdsa-sha2-nistp521" (source: `smithy_query(query="shape",shape="ListedHostKey")`,
/// member `Type`). Every listed key must name one of them.
const DOCUMENTED_HOST_KEY_TYPES: [&str; 5] = [
    "ssh-rsa",
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
];

/// `HostKeyStorageType`'s two values: "whether the host key is stored by Transfer
/// Family (SERVICE_MANAGED) or in Secrets Manager (SECRETS_MANAGER)".
const SERVICE_MANAGED: &str = "SERVICE_MANAGED";
const SECRETS_MANAGER: &str = "SECRETS_MANAGER";

/// Import one host key and return its minted `HostKeyId`.
pub(crate) async fn import_host_key(emulator: &Emulator, case: &str, request: Value) -> String {
    let answer = emulator.call("ImportHostKey", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ImportHostKey 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("HostKeyId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a HostKeyId, got {}", answer.body))
        .to_string()
}

/// The `DescribedHostKey` of one stored key — the other projection of the record the
/// listing renders.
async fn described_host_key(
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

/// One `ListHostKeys` call that must succeed, returning the whole answer body.
pub(crate) async fn list_host_keys(emulator: &Emulator, request: Value) -> Value {
    let answer = emulator.call("ListHostKeys", request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case {request:?}: expected ListHostKeys 200, got {} with body {}",
        answer.status, answer.body
    );
    answer.body
}

/// The `HostKeys` of one answer, as a list. `HostKeys` is modelled required, so it is
/// present even when the server holds nothing.
pub(crate) fn host_keys_of(case: &str, body: &Value) -> Vec<Value> {
    member_of(body, "HostKeys")
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: HostKeys is modelled required, got {body}"))
        .clone()
}

/// Read every host key of `server_id`, one page of `max_results` at a time (or in one
/// unbounded call when it is `None`), asserting the paging contract as it walks: every
/// page echoes the `ServerId`, no page is longer than the caller's bound, a key is
/// never repeated, and a `NextToken` comes back only while a further page exists.
pub(crate) async fn walk_host_keys(
    emulator: &Emulator,
    server_id: &str,
    max_results: Option<u32>,
) -> Vec<Value> {
    let case = format!("server {server_id} paged by {max_results:?}");
    let mut collected: Vec<Value> = Vec::new();
    let mut seen_ids: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;

    // One page per stored key is the worst case; the extra iterations catch a token
    // that never clears instead of looping forever.
    for _ in 0..64 {
        let mut request = json!({"ServerId": server_id});
        if let Some(limit) = max_results {
            request["MaxResults"] = json!(limit);
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let body = list_host_keys(emulator, request).await;

        assert_eq!(
            member_of(&body, "ServerId").as_str(),
            Some(server_id),
            "case {case:?}: expected the answer to echo ServerId, got {body}"
        );

        let page = host_keys_of(&case, &body);
        if let Some(limit) = max_results {
            assert!(
                page.len() <= limit as usize,
                "case {case:?}: expected at most {limit} keys per page, got {}",
                page.len()
            );
        }
        for listed in &page {
            let id = member_of(listed, "HostKeyId")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !seen_ids.contains(&id),
                "case {case:?}: expected each host key once, got {id:?} twice"
            );
            seen_ids.push(id);
        }
        collected.extend(page);

        next_token = member_of(&body, "NextToken").as_str().map(str::to_string);
        if next_token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: ListHostKeys kept handing out a NextToken after 64 pages")
}

/// The listed entry for `host_key_id`, which must be present exactly once.
fn listed(case: &str, keys: &[Value], host_key_id: &str) -> Value {
    let matching: Vec<&Value> = keys
        .iter()
        .filter(|listed| member_of(listed, "HostKeyId").as_str() == Some(host_key_id))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected {host_key_id} listed exactly once, got {keys:?}"
    );
    matching[0].clone()
}

/// What one listed host key must report, as the row that imported it states it.
struct Expect<'a> {
    /// The id the import minted for the key.
    host_key_id: &'a str,
    /// The fingerprint of the material the test generated.
    fingerprint: &'a str,
    /// The SSH algorithm name of that same material.
    key_type: &'a str,
    /// The `Description` the import carried, if any.
    description: Option<&'a str>,
    /// The `HostKeyStorageType` the key is stored under.
    storage_type: &'a str,
}

/// Assert the whole rule on one listed key: an ARN under its server, a modelled
/// `HostKeyId`, the fingerprint and algorithm of the imported material, a documented
/// `Type`, the stored storage type, an import date, no invented secret id — and the
/// SAME values `DescribeHostKey` reports for that record.
fn assert_listed_host_key(
    case: &str,
    entry: &Value,
    described: &Value,
    server_id: &str,
    expect: &Expect<'_>,
) {
    let Expect {
        host_key_id,
        fingerprint,
        key_type,
        description,
        storage_type,
    } = *expect;
    assert!(
        is_modelled_host_key_id(host_key_id),
        "case {case:?}: expected a HostKeyId matching ^hostkey-[0-9a-f]{{17}}$, got \
         {host_key_id:?}"
    );
    assert_eq!(
        member_of(entry, "Arn").as_str(),
        Some(
            format!("arn:aws:transfer:{REGION}:{ACCOUNT}:host-key/{server_id}/{host_key_id}")
                .as_str()
        ),
        "case {case:?}: expected the host key ARN under its server, got {entry}"
    );
    assert_eq!(
        member_of(entry, "Fingerprint").as_str(),
        Some(fingerprint),
        "case {case:?}: expected the imported key's own fingerprint {fingerprint:?}, got {entry}"
    );
    assert_eq!(
        member_of(entry, "Type").as_str(),
        Some(key_type),
        "case {case:?}: expected the imported key's own algorithm {key_type:?}, got {entry}"
    );
    assert!(
        DOCUMENTED_HOST_KEY_TYPES.contains(&key_type),
        "case {case:?}: expected one of the documented host key types \
         {DOCUMENTED_HOST_KEY_TYPES:?}, got {key_type:?}"
    );
    assert_eq!(
        member_of(entry, "Description").as_str(),
        description,
        "case {case:?}: expected Description {description:?}, got {entry}"
    );
    assert_eq!(
        member_of(entry, "StorageType").as_str(),
        Some(storage_type),
        "case {case:?}: expected StorageType {storage_type:?}, got {entry}"
    );
    assert!(
        member_of(entry, "DateImported").is_number(),
        "case {case:?}: expected a DateImported timestamp, got {entry}"
    );
    assert!(
        member_of(entry, "SecretId").is_null(),
        "case {case:?}: expected no SecretId for a key whose material no secret holds, \
         got {entry}"
    );

    // The listing and the describe are two projections of ONE stored record, so every
    // member both carry must agree.
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
            member_of(entry, listed_member),
            member_of(described, described_member),
            "case {case:?}: expected the listing's {listed_member} to equal the described \
             {described_member}, got {entry} against {described}"
        );
    }
}

/// RULE: the listing reports one entry per STORED host key, projected from the same
/// record `DescribeHostKey` reports — so a server lists nothing before an import and
/// every imported key afterwards, with that key's own fingerprint and algorithm.
///
/// The rows differ along what the import carried: nothing but the material, a
/// description, tags, each `StorageType` value — a renamed key is not a different
/// input, so each row changes one of those instead.
#[tokio::test]
async fn the_listing_reports_every_stored_host_key_of_the_server() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    // Before any import the server holds exactly ONE host key: its own, the key it
    // serves with (`crate::server_host_key_resource` is that rule's suite). No token is
    // handed out for a single page.
    let before = list_host_keys(&emulator, json!({"ServerId": &server_id})).await;
    let own_keys = host_keys_of("before any import", &before);
    assert_eq!(
        own_keys.len(),
        1,
        "case \"before any import\": expected the server's own host key alone, got {before}"
    );
    assert!(
        member_of(&before, "NextToken").is_null(),
        "case \"before any import\": expected no NextToken on a single page, got {before}"
    );
    // Every count below is against what the server already held.
    let own = own_keys.len();

    // (case, extra members of the import, the Description expected back, the
    // StorageType expected back)
    let rows: Vec<(&str, Value, Option<&str>, &str)> = vec![
        ("material alone", json!({}), None, SERVICE_MANAGED),
        (
            "a description",
            json!({"Description": "rotation key 2026"}),
            Some("rotation key 2026"),
            SERVICE_MANAGED,
        ),
        (
            "tags",
            json!({"Tags": [{"Key": "team", "Value": "sftp"}]}),
            None,
            SERVICE_MANAGED,
        ),
        (
            "an explicit SERVICE_MANAGED storage type",
            json!({"StorageType": SERVICE_MANAGED}),
            None,
            SERVICE_MANAGED,
        ),
        (
            "a SECRETS_MANAGER storage type and a description",
            json!({"StorageType": SECRETS_MANAGER, "Description": "in a secret"}),
            Some("in a secret"),
            SECRETS_MANAGER,
        ),
    ];

    let mut expected_ids: Vec<String> = Vec::new();
    for (case, extra, description, storage_type) in rows {
        let (material, fingerprint) = generated_host_key();
        let mut request = json!({"ServerId": &server_id, "HostKeyBody": &material});
        for (member, value) in extra.as_object().expect("the row's extra is an object") {
            request[member] = value.clone();
        }
        let host_key_id = import_host_key(&emulator, case, request).await;
        expected_ids.push(host_key_id.clone());

        let keys = walk_host_keys(&emulator, &server_id, None).await;
        assert_eq!(
            keys.len(),
            own + expected_ids.len(),
            "case {case:?}: expected the server's own key and the {} imported ones listed, \
             got {keys:?}",
            expected_ids.len()
        );
        let entry = listed(case, &keys, &host_key_id);
        let described = described_host_key(&emulator, case, &server_id, &host_key_id).await;
        assert_listed_host_key(
            case,
            &entry,
            &described,
            &server_id,
            &Expect {
                host_key_id: &host_key_id,
                fingerprint: &fingerprint,
                key_type: &host_key_algorithm(&material),
                description,
                storage_type,
            },
        );

        // Every earlier key is still listed: an import adds one entry and changes none.
        for earlier in &expected_ids {
            listed(case, &keys, earlier);
        }
    }
}

/// RULE: a host key belongs to ONE server, and the listing follows the store: a key is
/// listed for the server that holds it and for no other, a deleted key stops being
/// listed, and a listing for a server the store does not hold is refused.
#[tokio::test]
async fn the_listing_follows_the_store_per_server() {
    let emulator = Emulator::new();
    let first = emulator.create_server(json!({})).await;
    let second = emulator.create_server(json!({})).await;

    // The same key material on two servers: a host key is per server, so each server
    // holds its own record for it.
    let (shared, shared_fingerprint) = generated_host_key();
    let on_first = import_host_key(
        &emulator,
        "the shared key on the first server",
        json!({"ServerId": &first, "HostKeyBody": &shared}),
    )
    .await;
    let on_second = import_host_key(
        &emulator,
        "the shared key on the second server",
        json!({"ServerId": &second, "HostKeyBody": &shared}),
    )
    .await;
    // One more key on the first server, so the two servers differ in how many they
    // hold.
    let (extra, extra_fingerprint) = generated_host_key();
    let extra_id = import_host_key(
        &emulator,
        "a second key on the first server",
        json!({"ServerId": &first, "HostKeyBody": &extra}),
    )
    .await;

    for (case, server_id, expected) in [
        (
            "the first server",
            &first,
            vec![
                (on_first.as_str(), shared_fingerprint.as_str()),
                (extra_id.as_str(), extra_fingerprint.as_str()),
            ],
        ),
        (
            "the second server",
            &second,
            vec![(on_second.as_str(), shared_fingerprint.as_str())],
        ),
    ] {
        let keys = walk_host_keys(&emulator, server_id, None).await;
        // Each server also holds the key it serves with, beside the imported ones.
        assert_eq!(
            keys.len(),
            expected.len() + 1,
            "case {case:?}: expected the server's own key and the {} imported ones, got \
             {keys:?}",
            expected.len()
        );
        for (host_key_id, fingerprint) in expected {
            let entry = listed(case, &keys, host_key_id);
            assert_eq!(
                member_of(&entry, "Fingerprint").as_str(),
                Some(fingerprint),
                "case {case:?}: expected {host_key_id} to report {fingerprint:?}, got {entry}"
            );
            assert_eq!(
                member_of(&entry, "Arn").as_str(),
                Some(
                    format!(
                        "arn:aws:transfer:{REGION}:{ACCOUNT}:host-key/{server_id}/{host_key_id}"
                    )
                    .as_str()
                ),
                "case {case:?}: expected every listed key ARNed under {server_id}, got {entry}"
            );
        }
    }

    // A deleted key stops being listed, and only that one: the listing is the store.
    let deleted = emulator
        .call(
            "DeleteHostKey",
            json!({"ServerId": &first, "HostKeyId": &extra_id}),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteHostKey 200, got {} with body {}",
        deleted.status, deleted.body
    );
    let after_delete = walk_host_keys(&emulator, &first, None).await;
    assert_eq!(
        after_delete.len(),
        2,
        "case \"after deleting one key\": expected the other imported key and the server's \
         own alone, got {after_delete:?}"
    );
    listed("after deleting one key", &after_delete, &on_first);

    // Deleting the SERVER takes its keys with it, so the listing is refused for it —
    // exactly as for a server that never existed.
    let dropped = emulator
        .call("DeleteServer", json!({"ServerId": &first}))
        .await;
    assert_eq!(
        dropped.status, 200,
        "expected DeleteServer 200, got {} with body {}",
        dropped.status, dropped.body
    );
    for (case, server_id) in [
        ("a deleted server", first.as_str()),
        ("a server never created", "s-0000000000000dead"),
    ] {
        let answer = emulator
            .call("ListHostKeys", json!({"ServerId": server_id}))
            .await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected 400 ResourceNotFoundException, got {} with body {}",
            answer.status,
            answer.body
        );
    }

    // The other server is untouched by either delete.
    let survivor = walk_host_keys(&emulator, &second, None).await;
    assert_eq!(
        survivor.len(),
        2,
        "case \"after deleting another server\": expected the second server's imported key \
         and its own intact, got {survivor:?}"
    );
}

/// RULE: a `NextToken` this service never issued resumes nothing, so it is refused with
/// `InvalidNextTokenException` (HTTP 400); the token the service DID hand out resumes
/// the listing after the page it closed; and the named server is resolved first, so a
/// malformed token on an unknown server is a not-found.
#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    // One import beside the key the server itself serves with, so the server holds TWO
    // keys and a page size of one has exactly one further page.
    for index in 0..1 {
        let (material, _) = generated_host_key();
        let case = format!("key {index}");
        import_host_key(
            &emulator,
            &case,
            json!({"ServerId": &server_id, "HostKeyBody": material}),
        )
        .await;
    }

    // Rows: a token that is not the codec's hex at all, one with an odd number of hex
    // digits, one that is hex of bytes that are not UTF-8, and one at `NextToken`'s
    // modelled minimum length (`len 1..6144`).
    for token in ["not-a-token", "abc", "ff", "+"] {
        let answer = emulator
            .call(
                "ListHostKeys",
                json!({"ServerId": &server_id, "NextToken": token}),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code().as_str()),
            (400, "InvalidNextTokenException"),
            "case {token:?}: expected 400 InvalidNextTokenException, got {} with body {}",
            answer.status,
            answer.body
        );
    }

    // The named resource is resolved first: the SAME malformed token on a server that
    // does not exist is a not-found, not an invalid-token.
    for (case, absent) in [
        ("a server never created", "s-0000000000000dead"),
        ("another server never created", "s-fffffffffffffffff"),
    ] {
        let answer = emulator
            .call(
                "ListHostKeys",
                json!({"ServerId": absent, "NextToken": "not-a-token"}),
            )
            .await;
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected the unknown server to be refused first, got {}",
            answer.body
        );
    }

    // The token the service hands out resumes the listing after the page it closed.
    let first_page =
        list_host_keys(&emulator, json!({"ServerId": &server_id, "MaxResults": 1})).await;
    let token = member_of(&first_page, "NextToken")
        .as_str()
        .unwrap_or_else(|| panic!("expected a NextToken while a page remains, got {first_page}"))
        .to_string();
    let second_page = list_host_keys(
        &emulator,
        json!({"ServerId": &server_id, "MaxResults": 1, "NextToken": token}),
    )
    .await;
    let first_keys = host_keys_of("first page", &first_page);
    let second_keys = host_keys_of("second page", &second_page);
    assert_ne!(
        member_of(&first_keys[0], "HostKeyId"),
        member_of(&second_keys[0], "HostKeyId"),
        "case \"resumed page\": expected the next key, got the same one twice"
    );
    assert!(
        member_of(&second_page, "NextToken").is_null(),
        "case \"last page\": expected no NextToken once the listing is exhausted, \
         got {second_page}"
    );
}
