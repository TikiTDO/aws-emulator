// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for reading a CodeStar connections repository link back by its id
//! (`GetRepositoryLink`).
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front door builds (`POST /` + `X-Amz-Target`, awsJson1_0 body), reusing the
//! `connection_lifecycle` suite's harness and the `repository_link_lifecycle` suite's
//! table, so what is asserted here is the service's HTTP behaviour and not a handler
//! called directly.
//!
//! RULE — reporting a resource by its identifier answers the record this service HOLDS
//! under that identifier, member for member, without changing it; an identifier the
//! service holds no record under is answered with the modelled "the resource is not
//! there" error (`ResourceNotFoundException`) instead of a record, at the HTTP 400 this
//! endpoint's recorded wire answers its client errors with rather than the `@httpError
//! 404` the shape declares (the crate's `scaffolded::Scaffolded::modeled_with_status`
//! channel, as `CreateRepositoryLink`'s duplicate refusal and `GetHost`'s absent host
//! already do).
//!
//! The refusal's STATUS, wire CODE and WORDING are what that rule fixes: the message is
//! the fixed template the live service answers an absent link with — the label `uuid:`
//! among its fixed text — filled from the request and from the response itself: the
//! account the lookup was made in, the id that resolved to nothing, and the framework
//! tail's own fields (see `state::with_framework_tail`), whose request id is the one
//! per-run part of it.
//!
//! The tables below span the classes that rule generalises over — every modelled
//! `ProviderType`, both published bounds of `OwnerId` and `RepositoryName`, a link with
//! and without an encryption key, links with 0, 1 and several tags, several ids naming
//! nothing (including one a delete has just freed), and sets of links read in every
//! order — rather than one recorded request.

use serde_json::{Value, json};

use super::connection_lifecycle::{ACCOUNT_ID, Emulator, error_code};
use super::repository_link_lifecycle::{Row, assert_link_info, create_body, rows, tags_of};

/// `GetRepositoryLink` for `id`, as `(status, parsed body)`.
async fn get_link(emulator: &Emulator, id: &str) -> (u16, Value) {
    emulator
        .call("GetRepositoryLink", json!({ "RepositoryLinkId": id }))
        .await
}

/// The message an awsJson error envelope carries, under either casing.
fn error_message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Assert one answer is this endpoint's "no link under that id" refusal, and return its
/// message so a caller can compare two of them.
fn assert_not_found(case: &str, id: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the recorded client-error status for an id naming no \
         link, got {status} {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {body}"
    );
    let message = error_message(body);
    for needle in [
        // The refusal names the account the lookup was made in and the id that resolved
        // to nothing, so a caller can tell which of the two is wrong.
        format!("Repository not found for accountId: {ACCOUNT_ID} and uuid: {id}").as_str(),
        "(Service: CodeConnections;",
        // The tail states the status the Git-sync component RAISED the exception with —
        // the modelled `@httpError` of the shape — which on this legacy endpoint is not
        // the status the response itself carries (see `state::with_framework_tail`).
        "Status Code: 404;",
        "Error Code: ResourceNotFoundException;",
        "Proxy: null)",
    ] {
        assert!(
            message.contains(needle),
            "case {case:?}: expected the refusal message to carry {needle:?}, got \
             {message:?}"
        );
    }
    message
}

/// The RULE's reporting half, over the whole table: the link a create registered is
/// reported by its id member for member — the SAME record the create answered with, not
/// a second rendering of the request — and reading it changes nothing a later caller
/// sees (the record, its tags and its taken triple all survive repeated reads).
#[tokio::test]
async fn a_stored_repository_link_is_reported_by_its_id() {
    for row in rows() {
        let emulator = Emulator::new();
        let case = format!("{} ({})", row.case, row.provider_type);
        let connection_arn = emulator
            .create(
                &case,
                json!({
                    "ConnectionName": "link-backing-connection",
                    "ProviderType": row.provider_type,
                }),
            )
            .await;

        let (status, created) = emulator
            .call("CreateRepositoryLink", create_body(&row, &connection_arn))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateRepositoryLink to answer 200, got {status} {created}"
        );
        let created_info = created["RepositoryLinkInfo"].clone();
        let (id, arn) = assert_link_info(&case, &row, &connection_arn, &created_info);

        // Read it back three times: a report is repeatable, so every read answers the
        // one stored record.
        for attempt in 1..=3 {
            let read_case = format!("{case}: read attempt {attempt}");
            let (status, body) = get_link(&emulator, &id).await;
            assert_eq!(
                status, 200,
                "case {read_case:?}: expected GetRepositoryLink to report the stored link, \
                 got {status} {body}"
            );
            let info = body["RepositoryLinkInfo"].clone();
            assert_link_info(&read_case, &row, &connection_arn, &info);
            assert_eq!(
                info, created_info,
                "case {read_case:?}: expected the reported record to be the one the create \
                 registered, got {info} for {created_info}"
            );
            assert_eq!(
                body,
                json!({ "RepositoryLinkInfo": created_info }),
                "case {read_case:?}: expected the answer to carry only RepositoryLinkInfo, \
                 got {body}"
            );
        }

        // Reading the link perturbed nothing else about it: its create-time tags are
        // still readable under its own ARN…
        let mut expected_tags = row.tags.clone();
        expected_tags.sort();
        assert_eq!(
            tags_of(&emulator, &case, &arn).await,
            expected_tags,
            "case {case:?}: expected the reads to leave the link's tags alone"
        );
        // …and its (connection, owner, repository) triple is still taken.
        let (status, body) = emulator
            .call("CreateRepositoryLink", create_body(&row, &connection_arn))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the read link to still hold its triple, got {status} \
             {body}"
        );
    }
}

/// The RULE's refusal half: an id this service holds no link under is answered with the
/// modelled `ResourceNotFoundException`, never with a record and never with the
/// scaffold's 501. Each row is a well-formed `RepositoryLinkId` (the modelled pattern is
/// enforced before the handler runs), differing in how it fails to resolve.
#[tokio::test]
async fn an_id_that_names_no_repository_link_is_refused() {
    let emulator = Emulator::new();
    let connection_arn = emulator
        .create(
            "absent ids",
            json!({ "ConnectionName": "absent-id-connection", "ProviderType": "GitHub" }),
        )
        .await;
    let row = Row {
        case: "a link to delete before reading it",
        provider_type: "GitHub",
        owner_id: "owner-absent".to_string(),
        repository_name: "repo-absent".to_string(),
        encryption_key_arn: None,
        tags: Vec::new(),
    };
    let (status, created) = emulator
        .call("CreateRepositoryLink", create_body(&row, &connection_arn))
        .await;
    assert_eq!(
        status, 200,
        "the link to delete must be created first, got {status} {created}"
    );
    let deleted_id = created["RepositoryLinkInfo"]["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateRepositoryLink returned no id: {created}"))
        .to_string();
    let (status, body) = emulator
        .call(
            "DeleteRepositoryLink",
            json!({ "RepositoryLinkId": deleted_id.clone() }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the delete that frees the id must succeed, got {status} {body}"
    );

    // (case, an id this store holds no link under) — every one of them satisfying the
    // modelled `RepositoryLinkId` pattern
    // `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`
    // (`smithy_query(query="shape", shape="RepositoryLinkId")`), so the handler, not the
    // framework's validation, is what answers.
    let absent_ids = [
        ("an all-zero id", "00000000-0000-0000-0000-000000000000"),
        (
            "an id this emulator never minted",
            "9f8e7d6c-5b4a-4938-8271-605f4e3d2c1b",
        ),
        (
            "the same id written in upper-case hex, which the pattern also admits",
            "9F8E7D6C-5B4A-4938-8271-605F4E3D2C1B",
        ),
        ("an all-f id", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
        ("the id of a link this store has deleted", &deleted_id),
    ];

    for (case, id) in absent_ids {
        let (status, body) = get_link(&emulator, id).await;
        assert_not_found(case, id, status, &body);
        assert!(
            body["RepositoryLinkInfo"].is_null(),
            "case {case:?}: a refusal must carry no record, got {body}"
        );
    }

    // The refusal's tail carries a per-call request id, so refusing the same absent id
    // twice differs in exactly that field.
    let (_, first) = get_link(&emulator, &deleted_id).await;
    let (_, second) = get_link(&emulator, &deleted_id).await;
    assert_ne!(
        error_message(&first),
        error_message(&second),
        "the tail's request id is minted per call, so two refusals must not report the \
         same one: {first} / {second}"
    );
}

/// The RULE applied to a SET: each id reports exactly its own link, whatever order the
/// ids are read in and whichever links share a connection — so a report resolves the id
/// rather than answering with whatever the store happens to hold first.
#[tokio::test]
async fn a_read_reports_exactly_the_link_its_id_names() {
    const LINKS: usize = 4;

    // Read the set in forward, reverse and rotated order: a report must not depend on
    // the order the reads arrive in.
    for rotation in 0..LINKS {
        for reversed in [false, true] {
            let emulator = Emulator::new();
            let case = format!("rotation {rotation}, reversed {reversed}");
            let connection_arn = emulator
                .create(
                    &case,
                    json!({ "ConnectionName": "many-links", "ProviderType": "GitHub" }),
                )
                .await;

            let mut links: Vec<(Row, String, Value)> = Vec::new();
            for index in 0..LINKS {
                let row = Row {
                    case: "one of a set",
                    provider_type: "GitHub",
                    owner_id: format!("owner-{index}"),
                    repository_name: format!("repo-{index}"),
                    encryption_key_arn: None,
                    tags: vec![("Index".to_string(), index.to_string())],
                };
                let (status, body) = emulator
                    .call("CreateRepositoryLink", create_body(&row, &connection_arn))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected link {index} to be created, got {status} {body}"
                );
                let info = body["RepositoryLinkInfo"].clone();
                let (id, _) = assert_link_info(&case, &row, &connection_arn, &info);
                links.push((row, id, info));
            }

            let mut order: Vec<usize> = (0..LINKS).map(|index| (index + rotation) % LINKS).collect();
            if reversed {
                order.reverse();
            }
            for index in order {
                let (row, id, created_info) = &links[index];
                let read_case = format!("{case}: link {index}");
                let (status, body) = get_link(&emulator, id).await;
                assert_eq!(
                    status, 200,
                    "case {read_case:?}: expected the link to be reported, got {status} {body}"
                );
                let info = body["RepositoryLinkInfo"].clone();
                assert_link_info(&read_case, row, &connection_arn, &info);
                assert_eq!(
                    &info, created_info,
                    "case {read_case:?}: expected the record registered under this id, got \
                     {info}"
                );
            }
        }
    }
}
