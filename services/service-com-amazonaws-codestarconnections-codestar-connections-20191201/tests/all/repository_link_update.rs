// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateRepositoryLink`.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front door builds (`POST /` + `X-Amz-Target`, awsJson1_0 body), reusing the
//! `connection_lifecycle` suite's harness, so what is asserted here is the service's HTTP
//! behaviour and not a handler called directly.
//!
//! RULE — an update of a persisted resource WRITES the members the request supplies onto
//! the stored record and leaves every member the request omits as it was, so the record
//! the update answers with is the record every later reader serves
//! (`GetRepositoryLink` by id, `ListRepositoryLinks` over the set). A refused update
//! writes nothing: a `RepositoryLinkId` the service holds no link under is answered with
//! the modelled `ResourceNotFoundException`, and a `ConnectionArn` that either resolves to
//! no connection of this account or whose provider type differs from the link's own is
//! answered with the modelled `InvalidInputException` — both at the HTTP 400 this
//! endpoint's recorded wire answers its client errors with rather than the `@httpError`
//! the shapes declare.
//!
//! The provider-type condition is the operation's own documented one: "The updated
//! connection ARN must have the same providerType (such as GitHub) as the original
//! connection ARN for the repo link"
//! (`smithy_query(query="operation", operation="UpdateRepositoryLink")`, member
//! `ConnectionArn`).
//!
//! The tables below span the classes the rule generalises over — every combination of the
//! two updatable members (neither, each alone, both), every modelled `ProviderType`, links
//! created with and without an encryption key, a chain of successive re-pointings, and
//! several ways for an update to be refused — rather than one recorded request.

use serde_json::{Value, json};

use super::connection_lifecycle::{ACCOUNT_ID, Emulator, PROVIDER_TYPES, REGION, error_code,
    error_message};
use super::repository_link_lifecycle::{Row, create_body, tags_of};

/// A KMS key ARN of the shape `UpdateRepositoryLink.EncryptionKeyArn`'s modelled pattern
/// requires (`^arn:aws(-[\w]+)*:kms:[a-z\-0-9]+:\d{12}:key/[a-zA-Z0-9\-]+$`, from
/// `smithy_query(query="operation", operation="UpdateRepositoryLink")`).
fn key_arn(key_id: &str) -> String {
    format!("arn:aws:kms:{REGION}:{ACCOUNT_ID}:key/{key_id}")
}

/// One row of the update table: which members the update supplies, over a link created
/// against a connection of `provider_type` with `initial_key` (if any).
struct UpdateRow {
    case: String,
    provider_type: &'static str,
    /// The `EncryptionKeyArn` the link is CREATED with, if any.
    initial_key: Option<String>,
    /// Whether the update re-points the link at a second connection of the same provider
    /// type.
    repoint: bool,
    /// The `EncryptionKeyArn` the update supplies, if any.
    new_key: Option<String>,
}

/// The rows: every combination of the two updatable members over links with and without a
/// create-time key, plus one re-pointing row per modelled provider type.
fn update_rows() -> Vec<UpdateRow> {
    let mut rows: Vec<UpdateRow> = Vec::new();
    for initial_key in [None, Some(key_arn("create-time-key-0001"))] {
        for repoint in [false, true] {
            for new_key in [None, Some(key_arn("updated-key-0002"))] {
                rows.push(UpdateRow {
                    case: format!(
                        "initial key {}, repoint {repoint}, new key {}",
                        initial_key.is_some(),
                        new_key.is_some()
                    ),
                    provider_type: "GitHub",
                    initial_key: initial_key.clone(),
                    repoint,
                    new_key,
                });
            }
        }
    }
    // One re-pointing row per modelled provider type: the condition on a new association
    // is the provider type, so the rule has to hold for each value the model declares and
    // not only for the one the recorded run used.
    for provider_type in PROVIDER_TYPES {
        rows.push(UpdateRow {
            case: format!("repoint a {provider_type} link"),
            provider_type,
            initial_key: None,
            repoint: true,
            new_key: None,
        });
    }
    rows
}

/// A `CreateRepositoryLink` row for one update case.
fn link_row(row: &UpdateRow) -> Row {
    Row {
        case: "a link to update",
        provider_type: row.provider_type,
        owner_id: format!("owner-{}", row.provider_type.to_ascii_lowercase()),
        repository_name: format!("repo-{}", row.provider_type.to_ascii_lowercase()),
        encryption_key_arn: row.initial_key.clone(),
        tags: vec![("Suite".to_string(), "update".to_string())],
    }
}

/// `CreateRepositoryLink` for `row` against `connection_arn`, asserting success, and
/// return the `RepositoryLinkInfo` it answered with.
async fn create_link(emulator: &Emulator, case: &str, row: &Row, connection_arn: &str) -> Value {
    let (status, body) = emulator
        .call("CreateRepositoryLink", create_body(row, connection_arn))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRepositoryLink to answer 200, got {status} {body}"
    );
    body["RepositoryLinkInfo"].clone()
}

/// `GetRepositoryLink` for `id`, as `(status, parsed body)`.
async fn get_link(emulator: &Emulator, id: &str) -> (u16, Value) {
    emulator
        .call("GetRepositoryLink", json!({ "RepositoryLinkId": id }))
        .await
}

/// The `RepositoryLinkInfo` `GetRepositoryLink` reports for `id`, asserting the read
/// succeeded.
async fn read_link(emulator: &Emulator, case: &str, id: &str) -> Value {
    let (status, body) = get_link(emulator, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetRepositoryLink to report the link, got {status} {body}"
    );
    body["RepositoryLinkInfo"].clone()
}

/// The `RepositoryLinkInfo` `ListRepositoryLinks` reports for `id`, asserting the link is
/// on exactly one row of the listing.
async fn listed_link(emulator: &Emulator, case: &str, id: &str) -> Value {
    let (status, body) = emulator.call("ListRepositoryLinks", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListRepositoryLinks to answer 200, got {status} {body}"
    );
    let rows: Vec<Value> = body["RepositoryLinks"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinks list in {body}"))
        .iter()
        .filter(|info| info["RepositoryLinkId"].as_str() == Some(id))
        .cloned()
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "case {case:?}: expected exactly one listed row for {id}, got {body}"
    );
    rows[0].clone()
}

/// The `UpdateRepositoryLink` request body for one row.
fn update_body(id: &str, connection_arn: Option<&str>, new_key: Option<&str>) -> Value {
    let mut body = json!({ "RepositoryLinkId": id });
    if let Some(connection_arn) = connection_arn {
        body["ConnectionArn"] = json!(connection_arn);
    }
    if let Some(new_key) = new_key {
        body["EncryptionKeyArn"] = json!(new_key);
    }
    body
}

/// The RULE's writing half, over the whole table: the members an update supplies are
/// persisted onto the stored record, the members it omits keep the values they had, and
/// every later reader — the update's own answer, `GetRepositoryLink` and
/// `ListRepositoryLinks` — serves that one record.
#[tokio::test]
async fn an_update_persists_the_members_it_supplies_and_every_reader_serves_them() {
    for row in update_rows() {
        let emulator = Emulator::new();
        let case = row.case.clone();
        let link_row = link_row(&row);
        let first_connection = emulator
            .create(
                &case,
                json!({
                    "ConnectionName": "first-connection",
                    "ProviderType": row.provider_type,
                }),
            )
            .await;
        let created = create_link(&emulator, &case, &link_row, &first_connection).await;
        let id = created["RepositoryLinkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinkId in {created}"))
            .to_string();

        // The second connection carries the SAME provider type, which is the operation's
        // documented condition on a new association.
        let second_connection = emulator
            .create(
                &case,
                json!({
                    "ConnectionName": "second-connection",
                    "ProviderType": row.provider_type,
                }),
            )
            .await;

        let expected_connection = if row.repoint {
            second_connection.clone()
        } else {
            first_connection.clone()
        };
        // An omitted member keeps the value it had; a supplied one replaces it.
        let expected_key = row.new_key.clone().or_else(|| row.initial_key.clone());

        let (status, body) = emulator
            .call(
                "UpdateRepositoryLink",
                update_body(
                    &id,
                    row.repoint.then_some(second_connection.as_str()),
                    row.new_key.as_deref(),
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateRepositoryLink to answer 200, got {status} {body}"
        );
        let updated = body["RepositoryLinkInfo"].clone();
        assert_eq!(
            body,
            json!({ "RepositoryLinkInfo": updated.clone() }),
            "case {case:?}: expected the answer to carry only RepositoryLinkInfo, got {body}"
        );

        // The record the update answers with: the supplied members written, every other
        // member — the link's identity, its owner, repository and provider type — as the
        // create registered it.
        let expected = json!({
            "ConnectionArn": expected_connection,
            "EncryptionKeyArn": expected_key,
            "OwnerId": link_row.owner_id,
            "ProviderType": row.provider_type,
            "RepositoryLinkArn": created["RepositoryLinkArn"].clone(),
            "RepositoryLinkId": id,
            "RepositoryName": link_row.repository_name,
        });
        let expected = Value::Object(
            expected
                .as_object()
                .expect("the expected record is an object")
                .iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        assert_eq!(
            updated, expected,
            "case {case:?}: expected the update to answer the stored record with the \
             supplied members written, got {updated:?}"
        );

        // …and it is the record both readers serve from then on.
        let read = read_link(&emulator, &case, &id).await;
        assert_eq!(
            read, expected,
            "case {case:?}: expected GetRepositoryLink to serve the updated record, got \
             {read:?}"
        );
        let listed = listed_link(&emulator, &case, &id).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected ListRepositoryLinks to serve the updated record, got \
             {listed:?}"
        );

        // The update touched the members it was given and nothing else about the link:
        // its create-time tags are still readable under its own ARN.
        let arn = created["RepositoryLinkArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinkArn in {created}"))
            .to_string();
        let mut expected_tags = link_row.tags.clone();
        expected_tags.sort();
        assert_eq!(
            tags_of(&emulator, &case, &arn).await,
            expected_tags,
            "case {case:?}: expected the update to leave the link's tags alone"
        );
    }
}

/// The RULE applied to a CHAIN: successive updates each land on the one stored record, so
/// a read always serves the LAST association written — re-pointing a link twice is not the
/// creation-time association, and pointing it back is not the intermediate one.
#[tokio::test]
async fn successive_updates_each_replace_the_stored_association() {
    const CONNECTIONS: usize = 4;

    let emulator = Emulator::new();
    let case = "a chain of re-pointings";
    let mut connections: Vec<String> = Vec::new();
    for index in 0..CONNECTIONS {
        connections.push(
            emulator
                .create(
                    case,
                    json!({
                        "ConnectionName": format!("chain-connection-{index}"),
                        "ProviderType": "GitLab",
                    }),
                )
                .await,
        );
    }

    let row = Row {
        case: "a link re-pointed along a chain",
        provider_type: "GitLab",
        owner_id: "owner-chain".to_string(),
        repository_name: "repo-chain".to_string(),
        encryption_key_arn: None,
        tags: Vec::new(),
    };
    let created = create_link(&emulator, case, &row, &connections[0]).await;
    let id = created["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinkId in {created}"))
        .to_string();

    // Walk forward through the connections, back down again, and end on the first one —
    // so the last write is at times a later connection, at times an earlier one, and at
    // times the one the link was created against.
    let order: Vec<usize> = (1..CONNECTIONS).chain((0..CONNECTIONS - 1).rev()).collect();
    for step in order {
        let step_case = format!("{case}: point at connection {step}");
        let (status, body) = emulator
            .call(
                "UpdateRepositoryLink",
                update_body(&id, Some(&connections[step]), None),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {step_case:?}: expected the update to answer 200, got {status} {body}"
        );
        assert_eq!(
            body["RepositoryLinkInfo"]["ConnectionArn"].as_str(),
            Some(connections[step].as_str()),
            "case {step_case:?}: expected the update to answer the new association, got \
             {body}"
        );
        let read = read_link(&emulator, &step_case, &id).await;
        assert_eq!(
            read["ConnectionArn"].as_str(),
            Some(connections[step].as_str()),
            "case {step_case:?}: expected the read to serve the association last written, \
             got {read:?}"
        );
        let listed = listed_link(&emulator, &step_case, &id).await;
        assert_eq!(
            listed["ConnectionArn"].as_str(),
            Some(connections[step].as_str()),
            "case {step_case:?}: expected the listing to serve the association last \
             written, got {listed:?}"
        );
    }
}

/// The RULE's refusal half: an update this service cannot accept writes NOTHING — the
/// stored record is still the one the last accepted write left. Each row is a well-formed
/// request (the modelled constraints are enforced before the handler runs), differing in
/// why it cannot be accepted.
#[tokio::test]
async fn a_refused_update_leaves_the_stored_link_as_it_was() {
    let emulator = Emulator::new();
    let case = "refused updates";
    let connection = emulator
        .create(
            case,
            json!({ "ConnectionName": "refusal-connection", "ProviderType": "GitHub" }),
        )
        .await;
    // A connection of ANOTHER provider type: the operation documents that a new
    // association must carry the link's own provider type.
    let other_provider = emulator
        .create(
            case,
            json!({ "ConnectionName": "other-provider", "ProviderType": "Bitbucket" }),
        )
        .await;
    let row = Row {
        case: "a link no refused update may change",
        provider_type: "GitHub",
        owner_id: "owner-refusal".to_string(),
        repository_name: "repo-refusal".to_string(),
        encryption_key_arn: Some(key_arn("refusal-key-0003")),
        tags: Vec::new(),
    };
    let created = create_link(&emulator, case, &row, &connection).await;
    let id = created["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinkId in {created}"))
        .to_string();

    // A `RepositoryLinkId` this store holds no link under — every one of them satisfying
    // the modelled `RepositoryLinkId` pattern, so the handler and not the framework's
    // validation is what answers.
    for (absent_case, absent_id) in [
        ("an all-zero id", "00000000-0000-0000-0000-000000000000"),
        (
            "an id this emulator never minted",
            "9f8e7d6c-5b4a-4938-8271-605f4e3d2c1b",
        ),
        (
            "the same id in upper-case hex, which the pattern also admits",
            "9F8E7D6C-5B4A-4938-8271-605F4E3D2C1B",
        ),
        ("an all-f id", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
    ] {
        let (status, body) = emulator
            .call(
                "UpdateRepositoryLink",
                update_body(absent_id, Some(&connection), None),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {absent_case:?}: expected the recorded client-error status for an id \
             naming no link, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {absent_case:?}: expected ResourceNotFoundException, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(
                format!("Repository not found for accountId: {ACCOUNT_ID} and uuid: {absent_id}")
                    .as_str()
            ),
            "case {absent_case:?}: expected the refusal to name the account and the id that \
             resolved to nothing, got {message:?}"
        );
        assert!(
            body["RepositoryLinkInfo"].is_null(),
            "case {absent_case:?}: a refusal must carry no record, got {body}"
        );
    }

    // A `ConnectionArn` the update cannot be accepted with: one that resolves to no
    // connection of this account, and one whose provider type differs from the link's.
    let absent_connection =
        format!("arn:aws:codestar-connections:{REGION}:{ACCOUNT_ID}:connection/00000000-0000-0000-0000-0000000000ff");
    for (bad_case, connection_arn) in [
        ("a connection ARN naming no connection", &absent_connection),
        ("a connection of another provider type", &other_provider),
    ] {
        let (status, body) = emulator
            .call(
                "UpdateRepositoryLink",
                update_body(&id, Some(connection_arn), Some(&key_arn("rejected-0004"))),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {bad_case:?}: expected the modelled client-error status, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {bad_case:?}: expected InvalidInputException, got {body}"
        );
        assert!(
            body["RepositoryLinkInfo"].is_null(),
            "case {bad_case:?}: a refusal must carry no record, got {body}"
        );
        // Nothing was written: neither the association nor the encryption key the same
        // refused request also carried.
        let read = read_link(&emulator, bad_case, &id).await;
        assert_eq!(
            read, created,
            "case {bad_case:?}: expected the refused update to leave the record as the \
             create registered it, got {read:?}"
        );
    }
}
