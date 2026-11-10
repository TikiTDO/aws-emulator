// SPDX-License-Identifier: CC0-1.0

//! Refusals whose whole `Message` is the service's OWN fixed text for the condition.
//!
//! RULE under test: where this service states a refusal in a sentence of its own, the
//! answer is that sentence — not a rebuilt description of the rule — and it is FIXED for
//! the condition: the same text for every request that meets it, carrying nothing the
//! caller sent (no member name, no value, no identifier), so a client matching or
//! logging the service's refusal recognises it.
//!
//! Three conditions, each asserted over a TABLE whose rows differ along what the
//! condition generalises over rather than along the one request a report cited:
//!
//! 1. a server configuration that hosts its endpoint in a VPC and names no
//!    `EndpointDetails` — the rows span both `EndpointType` values that host in a VPC,
//!    crossed with the other members the create carries, and with the operation (create
//!    and update);
//! 2. a connector transfer that names no file to transfer — the rows span both connector
//!    kinds and both ways of naming nothing;
//! 3. host key material the service cannot read — the rows span the FORMS of unreadable
//!    material, including both published bounds of `HostKeyBody`'s `len 0..4096`.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`], so what
//! is asserted is what an SDK sees.

use serde_json::{json, Value};

use crate::harness::{generated_host_key, member_of, Answer, Emulator, ROLE, SSH_KEY};

/// Assert `answer` is 400 `InvalidRequestException` whose `Message` is exactly
/// `expected` and carries none of `absent` — the values the request sent.
fn assert_fixed_refusal(case: &str, answer: &Answer, expected: &str, absent: &[&str]) {
    assert_eq!(
        (answer.status, answer.error_code().as_str()),
        (400, "InvalidRequestException"),
        "case {case:?}: expected 400 InvalidRequestException, got {} with body {}",
        answer.status,
        answer.body
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        message, expected,
        "case {case:?}: expected the service's fixed text {expected:?}, got {message:?}"
    );
    for value in absent {
        assert!(
            !value.is_empty() && !message.contains(value),
            "case {case:?}: expected no request data in Message, got {message:?} carrying \
             {value:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 1. A VPC-hosted endpoint with no settings
// ---------------------------------------------------------------------------

/// The text the service states for that condition.
const ENDPOINT_DETAILS_REQUIRED: &str = "EndpointDetails required for VPC";

/// RULE: a server whose endpoint would be hosted in a VPC with no `EndpointDetails` is
/// refused with the service's fixed text, and nothing is created or changed.
///
/// The rows differ along the `EndpointType` that hosts the endpoint in a VPC (both
/// values of the modelled `[PUBLIC, VPC, VPC_ENDPOINT]` set that do), the other members
/// the request carries, and the operation that resolves the configuration — a create and
/// an update of a stored server, which the same requirement holds for.
#[tokio::test]
async fn a_vpc_hosted_endpoint_with_no_settings_is_refused_with_one_fixed_text() {
    let emulator = Emulator::new();
    // A stored server the update rows address, hosted on a PUBLIC endpoint.
    let server_id = emulator.create_server(json!({})).await;

    // (case, the members beside EndpointType)
    let shapes: Vec<(&str, Value)> = vec![
        ("nothing else", json!({})),
        (
            "a logging role and a banner",
            json!({
                "LoggingRole": ROLE,
                "PreAuthenticationLoginBanner": "restricted",
            }),
        ),
        (
            "an EFS domain",
            json!({"Domain": "EFS"}),
        ),
    ];

    for endpoint_type in ["VPC", "VPC_ENDPOINT"] {
        for (shape, extra) in &shapes {
            for op in ["CreateServer", "UpdateServer"] {
                let case = format!("{op} with EndpointType {endpoint_type} and {shape}");
                let mut request = json!({"EndpointType": endpoint_type});
                let object = request.as_object_mut().expect("the request is an object");
                for (member, value) in extra.as_object().expect("the row is an object") {
                    object.insert(member.clone(), value.clone());
                }
                if op == "UpdateServer" {
                    object.insert("ServerId".to_string(), json!(&server_id));
                }
                let answer = emulator.call(op, request).await;
                assert_fixed_refusal(
                    &case,
                    &answer,
                    ENDPOINT_DETAILS_REQUIRED,
                    &[server_id.as_str(), ROLE],
                );
                assert!(
                    member_of(&answer.body, "ServerId").is_null(),
                    "case {case:?}: expected the refusal to answer no ServerId, got {}",
                    answer.body
                );
            }
        }
    }

    // The requirement is about the VPC-hosted configuration alone: the stored server is
    // untouched and a PUBLIC create still succeeds.
    let described = emulator
        .call("DescribeServer", json!({"ServerId": &server_id}))
        .await;
    assert_eq!(
        member_of(described.get("Server"), "EndpointType").as_str(),
        Some("PUBLIC"),
        "case \"the stored server after the refusals\": expected it untouched, got {}",
        described.body
    );
    let public = emulator
        .call("CreateServer", json!({"EndpointType": "PUBLIC"}))
        .await;
    assert_eq!(
        public.status, 200,
        "case \"a PUBLIC create\": expected 200, got {} with body {}",
        public.status, public.body
    );
}

// ---------------------------------------------------------------------------
// 2. A transfer that names no file
// ---------------------------------------------------------------------------

/// The text the service states for that condition.
const EMPTY_SEND_FILE_LIST: &str = "Empty send file list";

/// RULE: a `StartFileTransfer` that names no file to transfer is refused with the
/// service's fixed text, whichever connector it was addressed at, and accepts nothing.
///
/// The rows differ along the connector kind (AS2 and SFTP, the two a connector can
/// configure) and along HOW the request names nothing — no path member at all, and the
/// directory members a transfer may carry without naming a file.
#[tokio::test]
async fn a_transfer_that_names_no_file_is_refused_with_one_fixed_text() {
    let emulator = Emulator::new();

    let as2 = emulator
        .call(
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": "https://partner.example.com/as2",
                "As2Config": {"LocalProfileId": "p-0123456789abcdef0"},
            }),
        )
        .await;
    let sftp = emulator
        .call(
            "CreateConnector",
            json!({
                "AccessRole": ROLE,
                "Url": "sftp://partner.example.com",
                "SftpConfig": {"UserSecretId": "sftp-partner"},
            }),
        )
        .await;
    let connectors: Vec<(&str, String)> = vec![
        ("an AS2 connector", connector_id("AS2", &as2)),
        ("an SFTP connector", connector_id("SFTP", &sftp)),
    ];

    let shapes: Vec<(&str, Value)> = vec![
        ("no path member at all", json!({})),
        (
            "a remote directory but no file",
            json!({"RemoteDirectoryPath": "/inbox"}),
        ),
        (
            "a local directory but no file",
            json!({"LocalDirectoryPath": "/bucket/landing"}),
        ),
    ];

    for (kind, connector_id) in &connectors {
        for (shape, extra) in &shapes {
            let case = format!("{kind} with {shape}");
            let mut request = json!({"ConnectorId": connector_id});
            let object = request.as_object_mut().expect("the request is an object");
            for (member, value) in extra.as_object().expect("the row is an object") {
                object.insert(member.clone(), value.clone());
            }
            let answer = emulator.call("StartFileTransfer", request).await;
            assert_fixed_refusal(
                &case,
                &answer,
                EMPTY_SEND_FILE_LIST,
                &[connector_id.as_str()],
            );
            assert!(
                member_of(&answer.body, "TransferId").is_null(),
                "case {case:?}: expected the refusal to accept nothing, got {}",
                answer.body
            );
        }
    }
}

/// The `ConnectorId` a create answered.
fn connector_id(kind: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status, 200,
        "the {kind} connector must be created, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("ConnectorId")
        .as_str()
        .unwrap_or_else(|| panic!("the {kind} create answered no ConnectorId: {}", answer.body))
        .to_string()
}

// ---------------------------------------------------------------------------
// 3. Host key material the service cannot read
// ---------------------------------------------------------------------------

/// The text the service states for that condition.
const INVALID_HOST_KEY_FORMAT: &str = "Invalid host key format";

/// RULE: an `ImportHostKey` whose `HostKeyBody` the service cannot read as a private key
/// is refused with the service's fixed text — which names neither the member nor any
/// expected encoding — and imports nothing.
///
/// The rows differ along the FORM of the unreadable material: the empty body and a body
/// at the modelled maximum (both published bounds of `HostKeyBody`'s `len 0..4096`),
/// prose, a PUBLIC key instead of a private one, a PEM envelope with no key inside, and
/// a truncated OpenSSH private key.
#[tokio::test]
async fn host_key_material_the_service_cannot_read_is_refused_with_one_fixed_text() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    let (material, _fingerprint) = generated_host_key();
    let truncated: String = material.chars().take(40).collect();

    let unreadable: Vec<(&str, String)> = vec![
        ("the empty body the modelled minimum admits", String::new()),
        ("prose", "this is not a private key".to_string()),
        ("a public key instead of a private one", SSH_KEY.to_string()),
        (
            "a PEM envelope with no key inside",
            "-----BEGIN PRIVATE KEY-----\n-----END PRIVATE KEY-----\n".to_string(),
        ),
        ("a truncated OpenSSH private key", truncated),
        (
            "a body at the modelled maximum length",
            "k".repeat(4096),
        ),
    ];

    for (case, body) in &unreadable {
        let answer = emulator
            .call(
                "ImportHostKey",
                json!({"ServerId": &server_id, "HostKeyBody": body}),
            )
            .await;
        assert_fixed_refusal(case, &answer, INVALID_HOST_KEY_FORMAT, &[server_id.as_str()]);
        assert!(
            member_of(&answer.body, "HostKeyId").is_null(),
            "case {case:?}: expected the refusal to mint no HostKeyId, got {}",
            answer.body
        );
    }

    // The server is untouched: readable material still imports.
    let imported = emulator
        .call(
            "ImportHostKey",
            json!({"ServerId": &server_id, "HostKeyBody": &material}),
        )
        .await;
    assert_eq!(
        imported.status, 200,
        "case \"readable material after the refusals\": expected 200, got {} with body {}",
        imported.status, imported.body
    );
}
