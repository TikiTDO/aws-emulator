// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING of three of this service's refusals: the
//! `CreateConnection` request that names no provider, the `DeleteHost` of a host
//! connections still hold, and the repository-link id that resolves to no link.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front door builds (`POST /` + `X-Amz-Target`, awsJson1_0 body), reusing the
//! `connection_lifecycle` suite's harness, so what is asserted here is the service's HTTP
//! behaviour and not a handler called directly.
//!
//! RULE: a refusal's message is the fixed template of the case it refuses — the service's
//! own wording, byte for byte — with only the request's own values (an ARN, an id) and the
//! service framework's own fields filled into it. Nothing else of the emulator's state
//! enters it: a template that carries no count does not grow one because the store holds
//! several records, and a template's fixed labels keep their casing.
//!
//! Each rule below is asserted over a TABLE: the inputs differ along what the rule
//! generalises over (which other members the refused request carries, how many records
//! the refusal is about, which id resolved to nothing, and which operation refused it),
//! and the message must be the same template in every row.

use serde_json::{json, Value};

use super::connection_lifecycle::{error_code, error_message, Emulator, ACCOUNT_ID};

/// What `CreateConnection` answers a request that names neither `ProviderType` nor
/// `HostArn`, as a LITERAL: the point of asserting it is to pin what a client reads, so it
/// is not read back out of the constant the handler writes. Entirely fixed text — the
/// request contributes nothing to it.
const PROVIDER_REQUIRED_MESSAGE: &str = "Must specify either HostArn or ProviderType";

/// The tail the service framework appends to a refusal its Git-sync component raised; the
/// request id in front of it is the only per-run part of the message, so the assertions
/// below split the message on it rather than matching it.
const FRAMEWORK_TAIL_HEAD: &str =
    " (Service: CodeConnections; Status Code: 404; Error Code: ResourceNotFoundException; \
     Request ID: ";
/// See [`FRAMEWORK_TAIL_HEAD`].
const FRAMEWORK_TAIL_END: &str = "; Proxy: null)";

/// RULE: `CreateConnection` refuses a request that names no provider with one fixed
/// sentence — the same text whatever else the request carries, with nothing of the request
/// interpolated into it.
#[tokio::test]
async fn a_create_connection_naming_no_provider_is_refused_with_one_fixed_sentence() {
    let emulator = Emulator::new();

    // The rows differ in every OTHER member the request may carry: `ConnectionName` is
    // modelled `len 1..32, pattern ^[\s\S]*$`, so both published bounds, a name with
    // spaces and a name that looks like a provider are all in range, and `Tags` is the
    // only other optional member there is.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a plain name",
            json!({ "ConnectionName": "no-provider" }),
        ),
        (
            "the shortest published name (1)",
            json!({ "ConnectionName": "c" }),
        ),
        (
            "the longest published name (32)",
            json!({ "ConnectionName": "n".repeat(32) }),
        ),
        (
            "a name with spaces and punctuation",
            json!({ "ConnectionName": "my connection (dev)" }),
        ),
        (
            // A name that happens to read like a provider is still no provider.
            "a name that spells a provider type",
            json!({ "ConnectionName": "GitHub" }),
        ),
        (
            "tags supplied, still no provider",
            json!({
                "ConnectionName": "no-provider-tagged",
                "Tags": [{ "Key": "env", "Value": "dev" }],
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, response) = emulator.call("CreateConnection", body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the request refused as invalid, got {status} {response:?}"
        );
        assert_eq!(
            error_message(&response),
            PROVIDER_REQUIRED_MESSAGE,
            "case {case:?}: wrong refusal message in {response:?}"
        );
    }

    // The positive control: naming a provider is accepted, so the refusal above is about
    // the missing provider and not about the request as a whole.
    let arn = emulator
        .create(
            "ProviderType alone",
            json!({ "ConnectionName": "direct", "ProviderType": "GitHub" }),
        )
        .await;
    let (status, response) = emulator
        .call("GetConnection", json!({ "ConnectionArn": arn }))
        .await;
    assert_eq!(
        status, 200,
        "the accepted connection must be readable back, got {status} {response:?}"
    );
}

/// RULE: `DeleteHost` refuses a host connections are still associated with by naming that
/// host — the `HostArn` the request supplied — and reports nothing of the emulator's own
/// state: the message is the same whether one connection or many still hold the host.
#[tokio::test]
async fn deleting_a_host_connections_hold_names_the_host_and_counts_nothing() {
    // The rows differ in how many connections hold the host — what a count-carrying
    // message would vary with — and in how many hold another host.
    let rows = [
        ("one connection holds the host", 1usize, 0usize),
        ("two connections hold the host", 2, 1),
        ("five connections hold the host", 5, 2),
    ];

    let mut messages: Vec<(String, String, String)> = Vec::new();
    for (case, attached, elsewhere) in rows {
        let emulator = Emulator::new();
        let host = emulator.create_host("busy-host").await;
        let other_host = emulator.create_host("other-host").await;

        for index in 0..attached {
            emulator
                .create(
                    case,
                    json!({
                        "ConnectionName": format!("attached-{index}"),
                        "HostArn": host,
                    }),
                )
                .await;
        }
        for index in 0..elsewhere {
            emulator
                .create(
                    case,
                    json!({
                        "ConnectionName": format!("elsewhere-{index}"),
                        "HostArn": other_host,
                    }),
                )
                .await;
        }

        let (status, response) = emulator
            .call("DeleteHost", json!({ "HostArn": &host }))
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceUnavailableException"),
            "case {case:?}: expected the delete refused as unavailable, got {status} \
             {response:?}"
        );
        let message = error_message(&response);
        assert_eq!(
            message,
            format!("There exist connections for host {host}, host cannot be deleted"),
            "case {case:?}: wrong refusal message in {response:?}"
        );
        messages.push((case.to_string(), host, message));
    }

    // One template for every row: replacing each row's own host ARN with a placeholder
    // leaves the SAME text, so nothing that varies between the rows (how many connections
    // hold the host) reached the message.
    let templates: Vec<String> = messages
        .iter()
        .map(|(_, host, message)| message.replace(host.as_str(), "<HostArn>"))
        .collect();
    assert!(
        templates.windows(2).all(|pair| pair[0] == pair[1]),
        "every row must word the refusal with one template, got {templates:?} for {:?}",
        messages.iter().map(|(case, ..)| case).collect::<Vec<_>>()
    );
}

/// RULE: a `RepositoryLinkId` that resolves to no link is refused with the repository-link
/// surface's own template — the account the lookup was made in and the id that resolved to
/// nothing, under fixed labels whose casing is part of the text — wherever a request
/// addresses a link by its id, and whatever the id is.
#[tokio::test]
async fn an_absent_repository_link_reads_the_same_template_in_every_operation() {
    let emulator = Emulator::new();
    let connection = emulator
        .create(
            "the backing connection",
            json!({ "ConnectionName": "link-owner", "ProviderType": "GitHub" }),
        )
        .await;

    // One id a link really had and a delete has freed, so the refusal is about a link that
    // is GONE and not only about one that never was.
    let (status, created) = emulator
        .call(
            "CreateRepositoryLink",
            json!({
                "ConnectionArn": connection,
                "OwnerId": "an-owner",
                "RepositoryName": "a-repository",
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected CreateRepositoryLink to answer 200, got {status} {created:?}"
    );
    let deleted_id = created["RepositoryLinkInfo"]["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("no RepositoryLinkId in {created:?}"))
        .to_string();
    let (status, response) = emulator
        .call(
            "DeleteRepositoryLink",
            json!({ "RepositoryLinkId": &deleted_id }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected DeleteRepositoryLink to answer 200, got {status} {response:?}"
    );

    // The ids: the freed one, and several that were never minted. `RepositoryLinkId` is
    // modelled `pattern ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`,
    // so the hex may be in either case and every digit may be zero.
    let ids: Vec<(&str, String)> = vec![
        ("an id a delete has freed", deleted_id),
        (
            "an id never minted",
            "8dfee30f-55d2-4e61-8b61-000000000001".to_string(),
        ),
        (
            "an id in upper-case hex",
            "8DFEE30F-55D2-4E61-8B61-00000000ABCD".to_string(),
        ),
        (
            "the all-zero id",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "the all-f id",
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        ),
    ];

    // Both operations that address a link by its id and answer out of this template; the
    // sync-status reader words it from the same formatter and is asserted in
    // `repository_sync_status.rs`.
    for (id_case, id) in &ids {
        for (operation, body) in [
            (
                "GetRepositoryLink",
                json!({ "RepositoryLinkId": id }),
            ),
            (
                "UpdateRepositoryLink",
                json!({ "RepositoryLinkId": id, "ConnectionArn": &connection }),
            ),
        ] {
            let case = format!("{operation} / {id_case}");
            let (status, response) = emulator.call(operation, body).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: expected the modelled absence, got {status} {response:?}"
            );
            let message = error_message(&response);
            let expected_head = format!(
                "Repository not found for accountId: {ACCOUNT_ID} and uuid: \
                 {id}{FRAMEWORK_TAIL_HEAD}"
            );
            assert!(
                message.starts_with(&expected_head),
                "case {case:?}: expected the refusal to open with {expected_head:?}, got \
                 {message:?}"
            );
            assert!(
                message.ends_with(FRAMEWORK_TAIL_END),
                "case {case:?}: expected the refusal to end with {FRAMEWORK_TAIL_END:?}, \
                 got {message:?}"
            );
            // The label is fixed text: its casing is part of what a client matches on, so
            // no other casing of it may appear.
            assert!(
                !message.contains("and UUID:"),
                "case {case:?}: the fixed label must read lower-case 'and uuid:', got \
                 {message:?}"
            );
            // The request id between the two halves is the only per-run field, and it is
            // there: the tail a client parses is complete.
            let request_id = message
                .strip_prefix(expected_head.as_str())
                .and_then(|rest| rest.strip_suffix(FRAMEWORK_TAIL_END))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the refusal is not this template: {message:?}")
                });
            assert!(
                !request_id.is_empty() && !request_id.contains(';'),
                "case {case:?}: expected one request id in the tail, got {request_id:?} \
                 from {message:?}"
            );
        }
    }
}
