// SPDX-License-Identifier: CC0-1.0

//! What a described server reports for the members the SERVICE fills in, and how its
//! `State` moves when a caller stops and starts it.
//!
//! Two rules, each asserted over a TABLE of inputs rather than one recorded call:
//!
//! 1. every member of a described server that the service computes or defaults is
//!    materialised when the server is created and reported on every read — so a
//!    configuration read-back never reports a configured server as unconfigured
//!    ([`described_servers_report_every_defaulted_and_computed_member`],
//!    [`a_host_key_fingerprint_is_minted_per_server_and_stable_across_reads`]);
//! 2. an operation documented to change a server's condition writes the intermediate
//!    state at once and the terminal one when the clock makes it due, so a caller
//!    polling a read reaches it ([`stop_and_start_move_the_server_through_its_states`]);
//! 3. an update changes the members it NAMES and leaves the rest of the stored server
//!    where they were ([`update_server_changes_only_the_members_it_names`]).
//!
//! The harness (router, request signing, `assert_members`) is the shared
//! [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{
    assert_members, generated_host_key, member_of, Emulator, PAST_START_SPAN, PAST_STOP_SPAN,
};

/// What `DescribedServer.ProtocolDetails` reports for a server created without any
/// `ProtocolDetails`: each member at the default its own model doc states (`PassiveIp`
/// AUTO, `TlsSessionResumptionMode` ENFORCED, `SetStatOption` DEFAULT, `SftpPorts` port
/// 22 with SERVER_TALK_FIRST, `ProxyConfig.SftpMode` NONE).
fn default_protocol_details() -> Value {
    json!({
        "PassiveIp": "AUTO",
        "TlsSessionResumptionMode": "ENFORCED",
        "SetStatOption": "DEFAULT",
        "SftpPorts": [{"SftpPort": 22, "CommunicationMode": "SERVER_TALK_FIRST"}],
        "ProxyConfig": {"SftpMode": "NONE"},
    })
}

/// `PreAuthenticationLoginBanner` is modelled `len 0..4096`, pattern
/// `^[\x09-\x0D\x20-\x7E]*$`; this is the published upper bound, every character legal.
fn banner_at_the_modelled_maximum() -> String {
    "x".repeat(4096)
}

// ---------------------------------------------------------------------------
// Rule 1: defaulted and computed members.
// ---------------------------------------------------------------------------

/// RULE: every member of a described server that the service defaults or computes is
/// resolved when the server is created and reported on every read — a member the
/// caller supplied is reported exactly as supplied, a member it left out is reported at
/// the default the model documents for it, and the computed `HostKeyFingerprint` and
/// the server's tag set are always reported.
///
/// The rows differ along which of the nested members the create request supplies
/// (none, one, several, all), which enum value each carries, how many `SftpPorts`
/// entries and tags there are, and whether the banner is absent, short, or at its
/// modelled maximum length.
#[tokio::test]
async fn described_servers_report_every_defaulted_and_computed_member() {
    let emulator = Emulator::new();
    let max_banner = banner_at_the_modelled_maximum();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "nothing supplied: every default materialised",
            json!({}),
            json!({
                "ProtocolDetails": default_protocol_details(),
                "S3StorageOptions": {"DirectoryListingOptimization": "DISABLED"},
                "SecurityPolicyName": "TransferSecurityPolicy-2024-01",
                "Tags": [],
                "PreAuthenticationLoginBanner": null,
            }),
        ),
        (
            "the recorded shape: SERVICE_MANAGED/S3/PUBLIC with a banner",
            json!({
                "Domain": "S3",
                "EndpointType": "PUBLIC",
                "Protocols": ["SFTP"],
                "IdentityProviderType": "SERVICE_MANAGED",
                "PreAuthenticationLoginBanner": "authorized users only",
            }),
            json!({
                "ProtocolDetails": default_protocol_details(),
                "S3StorageOptions": {"DirectoryListingOptimization": "DISABLED"},
                "SecurityPolicyName": "TransferSecurityPolicy-2024-01",
                "PreAuthenticationLoginBanner": "authorized users only",
                "Tags": [],
            }),
        ),
        (
            "one nested member supplied: PassiveIp kept, the rest defaulted",
            json!({"ProtocolDetails": {"PassiveIp": "203.0.113.7"}}),
            json!({
                "ProtocolDetails": {
                    "PassiveIp": "203.0.113.7",
                    "TlsSessionResumptionMode": "ENFORCED",
                    "SetStatOption": "DEFAULT",
                    "SftpPorts": [{"SftpPort": 22, "CommunicationMode": "SERVER_TALK_FIRST"}],
                    "ProxyConfig": {"SftpMode": "NONE"},
                },
                "S3StorageOptions": {"DirectoryListingOptimization": "DISABLED"},
            }),
        ),
        (
            "two nested enums supplied at their non-default values",
            json!({
                "ProtocolDetails": {
                    "SetStatOption": "ENABLE_NO_OP",
                    "TlsSessionResumptionMode": "DISABLED",
                },
                "S3StorageOptions": {"DirectoryListingOptimization": "ENABLED"},
                "Tags": [{"Key": "env", "Value": "prod"}],
            }),
            json!({
                "ProtocolDetails": {
                    "PassiveIp": "AUTO",
                    "TlsSessionResumptionMode": "DISABLED",
                    "SetStatOption": "ENABLE_NO_OP",
                    "SftpPorts": [{"SftpPort": 22, "CommunicationMode": "SERVER_TALK_FIRST"}],
                    "ProxyConfig": {"SftpMode": "NONE"},
                },
                "S3StorageOptions": {"DirectoryListingOptimization": "ENABLED"},
                "Tags": [{"Key": "env", "Value": "prod"}],
            }),
        ),
        (
            "three SftpPorts, one without a CommunicationMode",
            json!({
                "ProtocolDetails": {
                    "SftpPorts": [
                        {"SftpPort": 2222, "CommunicationMode": "CLIENT_TALK_FIRST"},
                        {"SftpPort": 2022},
                        {"SftpPort": 65535, "CommunicationMode": "SERVER_TALK_FIRST"},
                    ],
                },
            }),
            json!({
                "ProtocolDetails": {
                    "PassiveIp": "AUTO",
                    "TlsSessionResumptionMode": "ENFORCED",
                    "SetStatOption": "DEFAULT",
                    "SftpPorts": [
                        {"SftpPort": 2222, "CommunicationMode": "CLIENT_TALK_FIRST"},
                        {"SftpPort": 2022, "CommunicationMode": "SERVER_TALK_FIRST"},
                        {"SftpPort": 65535, "CommunicationMode": "SERVER_TALK_FIRST"},
                    ],
                    "ProxyConfig": {"SftpMode": "NONE"},
                },
            }),
        ),
        (
            "proxy and AS2 transport supplied, an explicit security policy and 3 tags",
            json!({
                "Protocols": ["SFTP", "AS2"],
                "EndpointType": "VPC",
                // `EndpointDetails` is required when the endpoint is hosted in a VPC,
                // and AS2 requires a VPC endpoint on the S3 domain.
                "EndpointDetails": {"VpcId": "vpc-0123456789abcdef0"},
                "ProtocolDetails": {
                    "ProxyConfig": {"SftpMode": "PROXY_PROTOCOL_V2_ENFORCED"},
                    "As2Transports": ["HTTP"],
                },
                "SecurityPolicyName": "TransferSecurityPolicy-FIPS-2023-05",
                "Tags": [
                    {"Key": "a", "Value": "1"},
                    {"Key": "b", "Value": "2"},
                    {"Key": "c", "Value": "3"},
                ],
            }),
            json!({
                "ProtocolDetails": {
                    "PassiveIp": "AUTO",
                    "TlsSessionResumptionMode": "ENFORCED",
                    "SetStatOption": "DEFAULT",
                    "SftpPorts": [{"SftpPort": 22, "CommunicationMode": "SERVER_TALK_FIRST"}],
                    "ProxyConfig": {"SftpMode": "PROXY_PROTOCOL_V2_ENFORCED"},
                    "As2Transports": ["HTTP"],
                },
                "SecurityPolicyName": "TransferSecurityPolicy-FIPS-2023-05",
                "EndpointDetails": {"VpcId": "vpc-0123456789abcdef0"},
                "Tags": [
                    {"Key": "a", "Value": "1"},
                    {"Key": "b", "Value": "2"},
                    {"Key": "c", "Value": "3"},
                ],
            }),
        ),
        (
            "an EFS server with a banner at the modelled maximum length",
            json!({
                "Domain": "EFS",
                "PreAuthenticationLoginBanner": max_banner.clone(),
                "PostAuthenticationLoginBanner": "welcome",
            }),
            json!({
                "Domain": "EFS",
                "PreAuthenticationLoginBanner": max_banner.clone(),
                "PostAuthenticationLoginBanner": "welcome",
                "ProtocolDetails": default_protocol_details(),
                "S3StorageOptions": {"DirectoryListingOptimization": "DISABLED"},
                "SecurityPolicyName": "TransferSecurityPolicy-2024-01",
            }),
        ),
    ];

    for (name, request, expected) in cases {
        let server_id = emulator.create_server(request).await;
        let answer = emulator
            .call("DescribeServer", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected DescribeServer 200, got {} with body {}",
            answer.status, answer.body
        );

        let server = answer.get("Server").clone();
        assert_members(name, &server, &expected);
        assert_well_formed_fingerprint(name, member_of(&server, "HostKeyFingerprint"));
    }
}

/// An OpenSSH SHA256 fingerprint: the literal `SHA256:` followed by the unpadded
/// Base64 of a 32-byte digest, i.e. 43 Base64 characters. This is what
/// `ssh-keygen -l` prints and what `HostKeyFingerprint` is documented to carry.
fn assert_well_formed_fingerprint(case: &str, actual: &Value) {
    let fingerprint = actual.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: expected a HostKeyFingerprint string, got {actual:?}")
    });
    let digest = fingerprint.strip_prefix("SHA256:").unwrap_or_else(|| {
        panic!("case {case:?}: expected a SHA256: fingerprint, got {fingerprint:?}")
    });
    assert_eq!(
        digest.len(),
        43,
        "case {case:?}: expected 43 Base64 characters of SHA256 digest, got {fingerprint:?}"
    );
    assert!(
        digest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/'),
        "case {case:?}: expected Base64 digest characters, got {fingerprint:?}"
    );
}

/// RULE: a server's host key is its own — each created server reports a different
/// fingerprint — and the value is a property of the stored server, so every read of one
/// server answers the same fingerprint.
#[tokio::test]
async fn a_host_key_fingerprint_is_minted_per_server_and_stable_across_reads() {
    let emulator = Emulator::new();

    async fn fingerprint_of(emulator: &Emulator, server_id: &str) -> String {
        let answer = emulator
            .call("DescribeServer", json!({"ServerId": server_id}))
            .await;
        member_of(answer.get("Server"), "HostKeyFingerprint")
            .as_str()
            .unwrap_or_else(|| {
                panic!("server {server_id}: expected a HostKeyFingerprint, got {}", answer.body)
            })
            .to_string()
    }

    // Rows: creates that differ in what they supply, so the fingerprint cannot be a
    // function of the request.
    let requests = vec![
        ("empty create", json!({})),
        ("EFS domain", json!({"Domain": "EFS"})),
        (
            "tagged, with a banner",
            json!({
                "PreAuthenticationLoginBanner": "banner",
                "Tags": [{"Key": "k", "Value": "v"}],
            }),
        ),
        ("empty create again", json!({})),
    ];

    let mut seen: Vec<(String, String)> = Vec::new();
    for (name, request) in requests {
        let server_id = emulator.create_server(request).await;
        let first = fingerprint_of(&emulator, &server_id).await;
        let second = fingerprint_of(&emulator, &server_id).await;
        assert_eq!(
            first, second,
            "case {name:?}: expected the same fingerprint on a second read, got {first:?} then {second:?}"
        );
        assert!(
            !seen.iter().any(|(_, other)| *other == first),
            "case {name:?}: expected a fingerprint distinct from {seen:?}, got {first:?}"
        );
        seen.push((name.to_string(), first));
    }
}

/// RULE: a host key the caller supplies IS the server's host key, so the fingerprint
/// reported for the server is that key's own — at create time and after an update that
/// rotates it — while a server given no key reports the fingerprint of the one the
/// service minted for it.
///
/// The rows are the two keys and the two ways a key arrives (create, update), each
/// asserted against the fingerprint computed from the key material itself.
#[tokio::test]
async fn a_supplied_host_key_is_the_fingerprint_the_server_reports() {
    let emulator = Emulator::new();

    async fn fingerprint_of(emulator: &Emulator, server_id: &str) -> String {
        let answer = emulator
            .call("DescribeServer", json!({"ServerId": server_id}))
            .await;
        member_of(answer.get("Server"), "HostKeyFingerprint")
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "server {server_id}: expected a HostKeyFingerprint, got {}",
                    answer.body
                )
            })
            .to_string()
    }

    let (first_key, first_fingerprint) = generated_host_key();
    let (second_key, second_fingerprint) = generated_host_key();

    // Row 1: supplied at create time.
    let supplied = emulator
        .create_server(json!({"HostKey": &first_key}))
        .await;
    let reported = fingerprint_of(&emulator, &supplied).await;
    assert_eq!(
        reported, first_fingerprint,
        "case \"HostKey at CreateServer\": expected the supplied key's fingerprint {first_fingerprint:?}, got {reported:?}"
    );

    // Row 2: the SAME key on a second server reports the same fingerprint — it is a
    // function of the key, not of the server.
    let same_key = emulator
        .create_server(json!({"HostKey": &first_key, "Domain": "EFS"}))
        .await;
    let reported = fingerprint_of(&emulator, &same_key).await;
    assert_eq!(
        reported, first_fingerprint,
        "case \"the same HostKey on another server\": expected {first_fingerprint:?}, got {reported:?}"
    );

    // Row 3: a different key rotates it through UpdateServer.
    let updated = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &supplied, "HostKey": &second_key}),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case \"HostKey at UpdateServer\": expected 200, got {} with body {}",
        updated.status, updated.body
    );
    let reported = fingerprint_of(&emulator, &supplied).await;
    assert_eq!(
        reported, second_fingerprint,
        "case \"HostKey at UpdateServer\": expected the new key's fingerprint {second_fingerprint:?}, got {reported:?}"
    );

    // Row 4: a server given no key reports neither supplied key's fingerprint.
    let minted = emulator.create_server(json!({})).await;
    let reported = fingerprint_of(&emulator, &minted).await;
    assert!(
        reported != first_fingerprint && reported != second_fingerprint,
        "case \"no HostKey\": expected the server's own minted fingerprint, got {reported:?}"
    );
    assert_well_formed_fingerprint("no HostKey", &json!(reported));

    // Row 5: material the emulator cannot read is REFUSED — an update that answered
    // success while leaving the server serving the key it had would be indistinguishable
    // from the rotation the caller asked for — and the server keeps the key it has.
    let before = fingerprint_of(&emulator, &minted).await;
    let unreadable = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &minted, "HostKey": "not a private key the emulator can read"}),
        )
        .await;
    assert_eq!(
        (unreadable.status, unreadable.error_code().as_str()),
        (400, "InvalidRequestException"),
        "case \"unreadable HostKey\": expected 400 InvalidRequestException, got {} with \
         body {}",
        unreadable.status,
        unreadable.body
    );
    let after = fingerprint_of(&emulator, &minted).await;
    assert_eq!(
        after, before,
        "case \"unreadable HostKey\": expected the server's existing fingerprint {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: the stop/start state machine.
// ---------------------------------------------------------------------------

/// Read a server's `State` through `DescribeServer`.
async fn described_state(emulator: &Emulator, server_id: &str) -> String {
    let answer = emulator
        .call("DescribeServer", json!({"ServerId": server_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "DescribeServer {server_id}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    member_of(answer.get("Server"), "State")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Read a server's `State` through `ListServers`.
async fn listed_state(emulator: &Emulator, server_id: &str) -> String {
    let answer = emulator.call("ListServers", json!({})).await;
    answer
        .get("Servers")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find(|server| server.get("ServerId").and_then(Value::as_str) == Some(server_id))
        .and_then(|server| server.get("State").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// RULE: `StopServer` takes a server out of service and `StartServer` puts it back —
/// each writes the intermediate state (`STOPPING`, `STARTING`) at once and the terminal
/// one (`OFFLINE`, `ONLINE`) when the span it began comes due on the clock, so a caller
/// polling a read reaches it. A call on a server that is already in, or already on its
/// way to, the state it targets "has no impact" and answers success; a call on a server
/// in flight the other way has no such transition to make and is refused with the
/// modelled `InvalidRequestException`; and neither call touches the server's other
/// properties.
///
/// The rows are the steps of one lifecycle — stop, a repeated stop, start, a repeated
/// start — each observed before and after its span and through both reporting
/// operations.
#[tokio::test]
async fn stop_and_start_move_the_server_through_its_states() {
    let emulator = Emulator::new();

    let server_id = emulator
        .create_server(json!({
            "PreAuthenticationLoginBanner": "stays put",
            "Tags": [{"Key": "keep", "Value": "me"}],
        }))
        .await;
    emulator.create_user(&server_id, "someone").await;

    // A server still STARTING has no stop to make yet, while a start of it has no
    // impact.
    let too_early = emulator
        .call("StopServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        too_early.status, 400,
        "case \"StopServer while STARTING\": expected HTTP 400, got {} with body {}",
        too_early.status, too_early.body
    );
    assert_eq!(
        too_early.error_code(),
        "InvalidRequestException",
        "case \"StopServer while STARTING\": expected InvalidRequestException, got {}",
        too_early.body
    );
    let start_while_starting = emulator
        .call("StartServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        start_while_starting.status, 200,
        "case \"StartServer while STARTING\": expected 200, got {} with body {}",
        start_while_starting.status, start_while_starting.body
    );
    let still_starting = described_state(&emulator, &server_id).await;
    assert_eq!(
        still_starting, "STARTING",
        "case \"StartServer while STARTING\": expected STARTING, got {still_starting:?}"
    );

    tokio::time::sleep(PAST_START_SPAN).await;
    let online = described_state(&emulator, &server_id).await;
    assert_eq!(
        online, "ONLINE",
        "case \"a created server past its start span\": expected ONLINE, got {online:?}"
    );

    // Step 1: StopServer answers 200 with an empty body and writes STOPPING at once.
    let stopped = emulator
        .call("StopServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        stopped.status, 200,
        "case \"StopServer\": expected 200, got {} with body {}",
        stopped.status, stopped.body
    );
    let stopping = described_state(&emulator, &server_id).await;
    assert_eq!(
        stopping, "STOPPING",
        "case \"straight after StopServer\": expected STOPPING, got {stopping:?}"
    );
    // A read is not a tick: reading again before the span answers the same thing.
    let stopping_again = described_state(&emulator, &server_id).await;
    assert_eq!(
        stopping_again, "STOPPING",
        "case \"second read before the stop span\": expected STOPPING, got {stopping_again:?}"
    );
    // Step 2: a repeated stop is already on its way to OFFLINE, so it has no impact and
    // must not restart the span; a start cannot cancel a stop in flight.
    let repeated = emulator
        .call("StopServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        repeated.status, 200,
        "case \"StopServer while STOPPING\": expected 200, got {} with body {}",
        repeated.status, repeated.body
    );
    let start_while_stopping = emulator
        .call("StartServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        start_while_stopping.status, 400,
        "case \"StartServer while STOPPING\": expected HTTP 400, got {} with body {}",
        start_while_stopping.status, start_while_stopping.body
    );
    assert_eq!(
        start_while_stopping.error_code(),
        "InvalidRequestException",
        "case \"StartServer while STOPPING\": expected InvalidRequestException, got {}",
        start_while_stopping.body
    );

    tokio::time::sleep(PAST_STOP_SPAN).await;
    let offline = described_state(&emulator, &server_id).await;
    assert_eq!(
        offline, "OFFLINE",
        "case \"after the stop span\": expected OFFLINE, got {offline:?}"
    );
    let offline_listed = listed_state(&emulator, &server_id).await;
    assert_eq!(
        offline_listed, "OFFLINE",
        "case \"ListServers after the stop span\": expected OFFLINE, got {offline_listed:?}"
    );
    // And a stop of an already-OFFLINE server has nothing to do: success, still OFFLINE.
    let stop_offline = emulator
        .call("StopServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        stop_offline.status, 200,
        "case \"StopServer while OFFLINE\": expected 200, got {} with body {}",
        stop_offline.status, stop_offline.body
    );
    let still_offline = described_state(&emulator, &server_id).await;
    assert_eq!(
        still_offline, "OFFLINE",
        "case \"StopServer while OFFLINE\": expected OFFLINE, got {still_offline:?}"
    );

    // Stopping changes the condition, not the configuration: "Information tied to your
    // server, such as server and user properties, are not affected by stopping".
    let answer = emulator
        .call("DescribeServer", json!({"ServerId": &server_id}))
        .await;
    assert_members(
        "after the stop span",
        answer.get("Server"),
        &json!({
            "PreAuthenticationLoginBanner": "stays put",
            "Tags": [{"Key": "keep", "Value": "me"}],
            "SecurityPolicyName": "TransferSecurityPolicy-2024-01",
            "ProtocolDetails": default_protocol_details(),
            "S3StorageOptions": {"DirectoryListingOptimization": "DISABLED"},
            "UserCount": 1,
        }),
    );

    // Step 3: a stopped server starts again, through STARTING.
    let started = emulator
        .call("StartServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        started.status, 200,
        "case \"StartServer\": expected 200, got {} with body {}",
        started.status, started.body
    );
    let starting = described_state(&emulator, &server_id).await;
    assert_eq!(
        starting, "STARTING",
        "case \"straight after StartServer\": expected STARTING, got {starting:?}"
    );
    tokio::time::sleep(PAST_START_SPAN).await;
    let back_online = described_state(&emulator, &server_id).await;
    assert_eq!(
        back_online, "ONLINE",
        "case \"after the start span\": expected ONLINE, got {back_online:?}"
    );

    // Step 4: "It has no impact on a server that is already ONLINE".
    let again = emulator
        .call("StartServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        again.status, 200,
        "case \"StartServer while ONLINE\": expected 200, got {} with body {}",
        again.status, again.body
    );
    let unchanged = described_state(&emulator, &server_id).await;
    assert_eq!(
        unchanged, "ONLINE",
        "case \"StartServer while ONLINE\": expected ONLINE, got {unchanged:?}"
    );
}

/// RULE: both state-changing calls answer this service's not-found error for a
/// `ServerId` the store does not hold — HTTP 400 with `__type`
/// `ResourceNotFoundException` and `ResourceType` `Server`, as every other operation
/// that resolves a server does (see `not_found_errors.rs`) — and change nothing.
#[tokio::test]
async fn stopping_or_starting_an_unknown_server_is_refused() {
    let emulator = Emulator::new();

    // A well-formed `ServerId` (`^s-([0-9a-f]{17})$`) that was never created, and one
    // that was created and then deleted: both are unknown to the store.
    let deleted = emulator.create_server(json!({})).await;
    let removed = emulator
        .call("DeleteServer", json!({"ServerId": &deleted}))
        .await;
    assert_eq!(
        removed.status, 200,
        "DeleteServer: expected 200, got {} with body {}",
        removed.status, removed.body
    );

    for op in ["StopServer", "StartServer"] {
        for (case, server_id) in [
            ("never created", "s-0123456789abcdef0".to_string()),
            ("created then deleted", deleted.clone()),
        ] {
            let answer = emulator.call(op, json!({"ServerId": server_id})).await;
            assert_eq!(
                answer.status, 400,
                "case {op} {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {op} {case:?}: expected ResourceNotFoundException, got {}",
                answer.body
            );
            assert_eq!(
                member_of(&answer.body, "ResourceType").as_str(),
                Some("Server"),
                "case {op} {case:?}: expected ResourceType Server, got {}",
                answer.body
            );
        }
    }
}

/// RULE: a server's condition is its own — stopping one server leaves every other
/// server in the store where it was.
#[tokio::test]
async fn stopping_one_server_leaves_the_others_online() {
    let emulator = Emulator::new();

    // Rows: three servers, one stopped. The untouched ones must not move.
    let ids: Vec<String> = vec![
        emulator.create_server(json!({})).await,
        emulator.create_server(json!({"Domain": "EFS"})).await,
        emulator
            .create_server(json!({"Tags": [{"Key": "k", "Value": "v"}]}))
            .await,
    ];
    tokio::time::sleep(PAST_START_SPAN).await;

    let stopped = &ids[1];
    let answer = emulator
        .call("StopServer", json!({"ServerId": stopped}))
        .await;
    assert_eq!(
        answer.status, 200,
        "StopServer: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    tokio::time::sleep(PAST_STOP_SPAN).await;

    for id in &ids {
        let want = if id == stopped { "OFFLINE" } else { "ONLINE" };
        let got = described_state(&emulator, id).await;
        assert_eq!(
            got, want,
            "case \"server {id} after stopping {stopped}\": expected {want}, got {got:?}"
        );
        let listed = listed_state(&emulator, id).await;
        assert_eq!(
            listed, want,
            "case \"listed server {id} after stopping {stopped}\": expected {want}, got {listed:?}"
        );
    }

    // A stopped server still holds its users and still answers reads of them.
    emulator.create_user(stopped, "still_here").await;
    let user = emulator
        .call(
            "DescribeUser",
            json!({"ServerId": stopped, "UserName": "still_here"}),
        )
        .await;
    assert_eq!(
        user.status, 200,
        "case \"DescribeUser on an OFFLINE server\": expected 200, got {} with body {}",
        user.status, user.body
    );
}

// ---------------------------------------------------------------------------
// Rule 3: UpdateServer.
// ---------------------------------------------------------------------------

/// RULE: `UpdateServer` replaces exactly the members the request NAMES, leaves every
/// other member of the stored server where it was, keeps the two fully-materialised
/// structures materialised (a named sub-member changes, the rest keep their current
/// values), and returns the `ServerId` it updated — and `DescribeServer` rebuilds all of
/// it from the same stored record.
///
/// The rows differ along which member is updated (a banner, a nested sub-member, a list,
/// an enum, nothing at all) and are applied in sequence to ONE server, so each row also
/// asserts that the previous rows' changes survived.
#[tokio::test]
async fn update_server_changes_only_the_members_it_names() {
    let emulator = Emulator::new();

    let server_id = emulator
        .create_server(json!({
            "PreAuthenticationLoginBanner": "the created banner",
            "Tags": [{"Key": "env", "Value": "dev"}],
            "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logs",
        }))
        .await;

    // (update request, expected members of the described server afterwards)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a named banner replaces the created one",
            json!({"PreAuthenticationLoginBanner": "the updated banner"}),
            json!({
                "PreAuthenticationLoginBanner": "the updated banner",
                // Untouched by this update:
                "Tags": [{"Key": "env", "Value": "dev"}],
                "LoggingRole": "arn:aws:iam::000000000000:role/transfer-logs",
                "ProtocolDetails": default_protocol_details(),
                "SecurityPolicyName": "TransferSecurityPolicy-2024-01",
            }),
        ),
        (
            "one nested sub-member: the rest of ProtocolDetails stays",
            json!({"ProtocolDetails": {"SetStatOption": "ENABLE_NO_OP"}}),
            json!({
                "ProtocolDetails": {
                    "PassiveIp": "AUTO",
                    "TlsSessionResumptionMode": "ENFORCED",
                    "SetStatOption": "ENABLE_NO_OP",
                    "SftpPorts": [{"SftpPort": 22, "CommunicationMode": "SERVER_TALK_FIRST"}],
                    "ProxyConfig": {"SftpMode": "NONE"},
                },
                "PreAuthenticationLoginBanner": "the updated banner",
            }),
        ),
        (
            "a second nested sub-member keeps the first one's new value",
            json!({"ProtocolDetails": {"PassiveIp": "198.51.100.9"}}),
            json!({
                "ProtocolDetails": {
                    "PassiveIp": "198.51.100.9",
                    "TlsSessionResumptionMode": "ENFORCED",
                    "SetStatOption": "ENABLE_NO_OP",
                    "SftpPorts": [{"SftpPort": 22, "CommunicationMode": "SERVER_TALK_FIRST"}],
                    "ProxyConfig": {"SftpMode": "NONE"},
                },
            }),
        ),
        (
            // FTPS needs a certificate, a VPC endpoint and a non-SERVICE_MANAGED
            // provider: the whole set has to arrive for the post-update configuration to
            // satisfy the documented cross-member requirements.
            "an enum member and several list members together",
            json!({
                "S3StorageOptions": {"DirectoryListingOptimization": "ENABLED"},
                "Protocols": ["SFTP", "FTPS"],
                "EndpointType": "VPC",
                "EndpointDetails": {"VpcId": "vpc-0123456789abcdef0"},
                "IdentityProviderType": "API_GATEWAY",
                "IdentityProviderDetails": {"Url": "https://example.invalid/auth"},
                "Certificate": "arn:aws:acm:us-east-1:000000000000:certificate/abcd",
                "SecurityPolicyName": "TransferSecurityPolicy-FIPS-2025-03",
            }),
            json!({
                "S3StorageOptions": {"DirectoryListingOptimization": "ENABLED"},
                "Protocols": ["SFTP", "FTPS"],
                "EndpointType": "VPC",
                "EndpointDetails": {"VpcId": "vpc-0123456789abcdef0"},
                "IdentityProviderType": "API_GATEWAY",
                "IdentityProviderDetails": {"Url": "https://example.invalid/auth"},
                "Certificate": "arn:aws:acm:us-east-1:000000000000:certificate/abcd",
                "SecurityPolicyName": "TransferSecurityPolicy-FIPS-2025-03",
                "PreAuthenticationLoginBanner": "the updated banner",
            }),
        ),
        (
            // "You can clear it ... by providing an empty value for this parameter."
            // Cleared means NOT CONFIGURED, so the member goes absent; the dedicated
            // table for the logging members is in `server_logging_configuration.rs`.
            "an empty StructuredLogDestinations clears structured logging",
            json!({"StructuredLogDestinations": []}),
            json!({
                "StructuredLogDestinations": null,
                "Protocols": ["SFTP", "FTPS"],
            }),
        ),
        (
            "a request that names nothing changes nothing",
            json!({}),
            json!({
                "PreAuthenticationLoginBanner": "the updated banner",
                "Protocols": ["SFTP", "FTPS"],
                "S3StorageOptions": {"DirectoryListingOptimization": "ENABLED"},
                "SecurityPolicyName": "TransferSecurityPolicy-FIPS-2025-03",
                "Tags": [{"Key": "env", "Value": "dev"}],
            }),
        ),
    ];

    for (name, mut request, expected) in cases {
        request
            .as_object_mut()
            .expect("an update request object")
            .insert("ServerId".to_string(), json!(&server_id));
        let answer = emulator.call("UpdateServer", request).await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected UpdateServer 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("ServerId").as_str(),
            Some(server_id.as_str()),
            "case {name:?}: expected UpdateServer to return the updated ServerId, got {}",
            answer.body
        );

        let described = emulator
            .call("DescribeServer", json!({"ServerId": &server_id}))
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected DescribeServer 200, got {} with body {}",
            described.status, described.body
        );
        assert_members(name, described.get("Server"), &expected);
    }
}

/// RULE: the documented cross-member requirements hold for the configuration the server
/// would HAVE after an update, not only for the one a create resolves to — so an update
/// that would leave the stored server in a configuration the service never hosts is
/// refused with the modelled `InvalidRequestException` and writes nothing.
///
/// The rows are the requirement sentences the model states, each triggered by an update
/// of a server that satisfies them today: a VPC endpoint without its settings, FTPS
/// without a certificate, FTP on a public endpoint, and
/// `SERVICE_MANAGED` with identity-provider details.
#[tokio::test]
async fn an_update_that_would_break_a_cross_member_requirement_is_refused() {
    let emulator = Emulator::new();
    let server_id = emulator
        .create_server(json!({"PreAuthenticationLoginBanner": "untouched"}))
        .await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "EndpointType VPC without EndpointDetails",
            json!({"EndpointType": "VPC"}),
        ),
        (
            "FTPS without a Certificate",
            json!({
                "Protocols": ["SFTP", "FTPS"],
                "EndpointType": "VPC",
                "EndpointDetails": {"VpcId": "vpc-0123456789abcdef0"},
                "IdentityProviderType": "API_GATEWAY",
                "IdentityProviderDetails": {"Url": "https://example.invalid/auth"},
            }),
        ),
        (
            "FTP on a PUBLIC endpoint",
            json!({"Protocols": ["SFTP", "FTP"]}),
        ),
        (
            "IdentityProviderDetails on a SERVICE_MANAGED server",
            json!({"IdentityProviderDetails": {"Url": "https://example.invalid/auth"}}),
        ),
    ];

    for (name, mut request) in cases {
        request
            .as_object_mut()
            .expect("an update request object")
            .insert("ServerId".to_string(), json!(&server_id));
        let answer = emulator.call("UpdateServer", request).await;
        assert_eq!(
            answer.status, 400,
            "case {name:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {name:?}: expected InvalidRequestException, got {}",
            answer.body
        );

        // The refusal is BEFORE the write: the server is still what it was.
        let described = emulator
            .call("DescribeServer", json!({"ServerId": &server_id}))
            .await;
        assert_members(
            name,
            described.get("Server"),
            &json!({
                "EndpointType": "PUBLIC",
                "Protocols": ["SFTP"],
                "IdentityProviderType": "SERVICE_MANAGED",
                "IdentityProviderDetails": null,
                "Certificate": null,
                "EndpointDetails": null,
                "PreAuthenticationLoginBanner": "untouched",
            }),
        );
    }
}

/// RULE: `IpAddressType` "cannot be changed while the server is online. You must stop
/// the server before modifying this parameter" — so the change is refused with the
/// modelled `InvalidRequestException` unless the server is `OFFLINE`, while a request
/// naming the value the server already has changes nothing and is accepted.
#[tokio::test]
async fn ip_address_type_changes_only_while_the_server_is_offline() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;
    tokio::time::sleep(PAST_START_SPAN).await;

    // Row 1: a change while the server is ONLINE is refused.
    let refused = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &server_id, "IpAddressType": "DUALSTACK"}),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"IpAddressType change while ONLINE\": expected HTTP 400, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "InvalidRequestException",
        "case \"IpAddressType change while ONLINE\": expected InvalidRequestException, got {}",
        refused.body
    );
    let unchanged = emulator
        .call("DescribeServer", json!({"ServerId": &server_id}))
        .await;
    assert_members(
        "IpAddressType change while ONLINE",
        unchanged.get("Server"),
        &json!({"IpAddressType": "IPV4"}),
    );

    // Row 2: naming the value it already has is no change at all, so it is accepted.
    let same = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &server_id, "IpAddressType": "IPV4"}),
        )
        .await;
    assert_eq!(
        same.status, 200,
        "case \"IpAddressType unchanged while ONLINE\": expected 200, got {} with body {}",
        same.status, same.body
    );

    // Row 3: stop the server, and the change goes through.
    let stopped = emulator
        .call("StopServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        stopped.status, 200,
        "StopServer: expected 200, got {} with body {}",
        stopped.status, stopped.body
    );
    // While STOPPING the server is not OFFLINE yet, so the change is still refused.
    let mid_flight = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &server_id, "IpAddressType": "DUALSTACK"}),
        )
        .await;
    assert_eq!(
        mid_flight.status, 400,
        "case \"IpAddressType change while STOPPING\": expected HTTP 400, got {} with body {}",
        mid_flight.status, mid_flight.body
    );

    tokio::time::sleep(PAST_STOP_SPAN).await;
    let accepted = emulator
        .call(
            "UpdateServer",
            json!({"ServerId": &server_id, "IpAddressType": "DUALSTACK"}),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"IpAddressType change while OFFLINE\": expected 200, got {} with body {}",
        accepted.status, accepted.body
    );
    let described = emulator
        .call("DescribeServer", json!({"ServerId": &server_id}))
        .await;
    assert_members(
        "IpAddressType change while OFFLINE",
        described.get("Server"),
        &json!({"IpAddressType": "DUALSTACK", "State": "OFFLINE"}),
    );
}

/// RULE: an update of a `ServerId` the store does not hold answers the same not-found
/// error as every other operation that resolves a server, and writes nothing.
#[tokio::test]
async fn updating_an_unknown_server_is_refused() {
    let emulator = Emulator::new();

    let deleted = emulator.create_server(json!({})).await;
    let removed = emulator
        .call("DeleteServer", json!({"ServerId": &deleted}))
        .await;
    assert_eq!(
        removed.status, 200,
        "DeleteServer: expected 200, got {} with body {}",
        removed.status, removed.body
    );

    for (case, server_id) in [
        ("never created", "s-0123456789abcdef0".to_string()),
        ("created then deleted", deleted.clone()),
    ] {
        let answer = emulator
            .call(
                "UpdateServer",
                json!({"ServerId": server_id, "PreAuthenticationLoginBanner": "nope"}),
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        assert_eq!(
            member_of(&answer.body, "ResourceType").as_str(),
            Some("Server"),
            "case {case:?}: expected ResourceType Server, got {}",
            answer.body
        );
    }

    // Nothing was created by the refused updates.
    let listed = emulator.call("ListServers", json!({})).await;
    let count = listed.get("Servers").as_array().map_or(0, Vec::len);
    assert_eq!(
        count, 0,
        "case \"after two refused updates\": expected an empty store, got {}",
        listed.body
    );
}
