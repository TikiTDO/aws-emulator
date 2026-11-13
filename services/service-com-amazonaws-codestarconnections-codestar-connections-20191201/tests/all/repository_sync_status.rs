// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetRepositorySyncStatus`.
//!
//! Every call goes over the wire: a request is dispatched through the
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) exactly as the
//! emulator's front door does it (`POST /` + `X-Amz-Target:
//! CodeStar_connections_20191201.GetRepositorySyncStatus`, awsJson1_0 body), so what is
//! asserted here is the service's HTTP behaviour and not a handler called directly.
//!
//! RULE — the operation reports the repository sync attempt this service has RECORDED
//! against the `(SyncType, RepositoryLinkId, Branch)` triple the request names, member for
//! member, without changing it; a triple it holds no attempt for — a branch that has never
//! synced, or an id naming no link at all — is answered with the modelled
//! `ResourceNotFoundException` rather than with an attempt, because the output's single
//! member `LatestSync` is `@required` and so there is no success that says "never synced"
//! (`smithy_query(query="operation", operation="GetRepositorySyncStatus")`).
//!
//! What that rule fixes is the refusal's wire CODE, its status — the HTTP 400 this
//! endpoint's recorded wire answers its client errors with, rather than the `@httpError
//! 404` the shape declares — and its MESSAGE, which for a triple holding no attempt is the
//! recorded sentence naming the link whose sync status could not be found. For an id naming
//! no link at all the refusal is worded as the link's own readers word it: the account the
//! lookup was made in, the id that resolved to nothing, and the framework tail's own
//! fields.
//!
//! A repository sync attempt is recorded by the service's Git-sync component, which this
//! emulator does not run, so the attempts reported here are put in the store directly,
//! through the SAME map the handler reads
//! (`codestar::state::repository_sync_attempts`) — which is what makes these assertions
//! evidence that the response is computed from persisted state rather than from a constant.
//!
//! The tables span what the rule generalises over: every modelled `RepositorySyncStatus`,
//! every modelled `SyncConfigurationType`, the published `BranchName` bounds and the forms
//! its pattern admits, how many events an attempt carries (none, one, several), whether an
//! event carries an external id, and which link an attempt was recorded against. The
//! recorded request (a branch with no sync history) is ONE row.

use serde_json::{json, Value};

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

use codestar::ops::types::{RepositorySyncAttempt, RepositorySyncEvent};
use codestar::state::{
    repository_sync_key,
    ComAmazonawsCodestarconnectionsCodestarConnections20191201State as CodestarState,
};

// The router harness this crate's suites share (one emulator, one engine, awsJson
// dispatch) and the two readers of a wire answer they already state, rather than a second
// copy of either.
use super::connection_lifecycle::{error_code, error_message, Emulator, ACCOUNT_ID};
use super::sync_blocker_summary::epoch_seconds;

/// Every value the model's `SyncConfigurationType` enum accepts.
///
/// Referenced rather than copied: this is the generated catalog
/// (`src/ops/types/shared_types.rs::SYNC_CONFIGURATION_TYPE_VALUES`, from
/// `smithy_query(query="shape", shape="SyncConfigurationType")`), so the tables below
/// widen with the model instead of drifting from it.
const SYNC_TYPES: &[&str] = codestar::ops::types::shared_types::SYNC_CONFIGURATION_TYPE_VALUES;

/// The `SyncType` the single-type tables address.
const SYNC_TYPE: &str = SYNC_TYPES[0];

/// Every value the model's `RepositorySyncStatus` enum accepts (the generated catalog,
/// from `smithy_query(query="shape", shape="RepositorySyncStatus")`): a recorded repository
/// sync attempt carries one of these.
const REPOSITORY_SYNC_STATUSES: &[&str] =
    codestar::ops::types::shared_types::REPOSITORY_SYNC_STATUS_VALUES;

/// The published bounds of the modelled `Branch` member (`length 1..255`,
/// `pattern ^.*$`), from
/// `smithy_query(query="operation", operation="GetRepositorySyncStatus")`.
const BRANCH_MIN_LEN: usize = 1;
const BRANCH_MAX_LEN: usize = 255;

/// The epoch second the seeded attempts start from; later rows step off it.
const BASE_EPOCH_SECS: i64 = 1_767_225_600;

/// The branch forms the rule generalises over: each published length bound, and the forms
/// the published pattern (`^.*$`, i.e. any text) admits — a plain name, the slashes and
/// dots Git refs carry, an underscore, a digit-only name. None of these is the branch the
/// issue's recording cited.
fn branches() -> Vec<(String, String)> {
    vec![
        ("the default trunk name", "main".to_string()),
        (
            "a branch at the published minimum length",
            "b".repeat(BRANCH_MIN_LEN),
        ),
        (
            "a branch at the published maximum length",
            "b".repeat(BRANCH_MAX_LEN),
        ),
        (
            "a namespaced ref the pattern admits",
            "feature/add-git-sync".to_string(),
        ),
        (
            "a ref carrying dots and digits",
            "release/1.0.0".to_string(),
        ),
        ("a ref carrying an underscore", "topic_branch".to_string()),
        ("a digit-only ref", "2026".to_string()),
    ]
    .into_iter()
    .map(|(label, branch)| (label.to_string(), branch))
    .collect()
}

/// A branch that is NOT `branch` but is still inside the published `Branch` bounds: the
/// name truncated far enough to carry `suffix`, so even the row at the published maximum
/// length yields a legal neighbour rather than an over-long one the boundary validator
/// would refuse before the handler ran.
fn neighbour_branch(branch: &str, suffix: &str) -> String {
    let keep = BRANCH_MAX_LEN - suffix.len();
    let mut neighbour: String = branch.chars().take(keep).collect();
    neighbour.push_str(suffix);
    neighbour
}

/// `GetRepositorySyncStatus` for one triple: the status and the parsed body.
async fn get_status(
    emulator: &Emulator,
    sync_type: &str,
    repository_link_id: &str,
    branch: &str,
) -> (u16, Value) {
    emulator
        .call(
            "GetRepositorySyncStatus",
            json!({
                "SyncType": sync_type,
                "RepositoryLinkId": repository_link_id,
                "Branch": branch,
            }),
        )
        .await
}

/// Register one repository link through the API and return its `RepositoryLinkId`: the id
/// a sync status is addressed by has to be a link this service actually holds, so it is
/// created the way a client creates one.
///
/// Shared with the sibling suites that address a link by its id (`repository_sync_definitions`),
/// so there is ONE statement of how a link under test is registered.
pub(crate) async fn create_link(emulator: &Emulator, case: &str, repository_name: &str) -> String {
    let connection_arn = emulator
        .create(
            case,
            json!({
                "ConnectionName": format!("conn-{repository_name}"),
                "ProviderType": "GitHub",
            }),
        )
        .await;
    let (status, created) = emulator
        .call(
            "CreateRepositoryLink",
            json!({
                "ConnectionArn": connection_arn,
                "OwnerId": format!("owner-{repository_name}"),
                "RepositoryName": repository_name,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRepositoryLink to answer 200, got {status} {created:?}"
    );
    created["RepositoryLinkInfo"]["RepositoryLinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no RepositoryLinkId in {created:?}"))
        .to_string()
}

/// Put one triple's sync attempt in the store — what the Git-sync side does, and what this
/// reader must report.
fn seed_attempt(
    emulator: &Emulator,
    sync_type: &str,
    repository_link_id: &str,
    branch: &str,
    attempt: RepositorySyncAttempt,
) {
    emulator
        .engine()
        .state::<CodestarState>()
        .lock()
        .repository_sync_attempts
        .insert(
            repository_sync_key(sync_type, repository_link_id, branch),
            attempt,
        );
}

/// Every `(SyncType, RepositoryLinkId, Branch)` triple the store holds an attempt for.
fn stored_triples(emulator: &Emulator) -> Vec<(String, String, String)> {
    emulator
        .engine()
        .state::<CodestarState>()
        .lock()
        .repository_sync_attempts
        .keys()
        .cloned()
        .collect()
}

/// A `RepositorySyncAttempt` with `events` events, as the Git-sync side records one: every
/// other event carries the optional `ExternalId`, so a row exercises both forms.
fn sync_attempt(marker: &str, status: &str, events: usize) -> RepositorySyncAttempt {
    RepositorySyncAttempt {
        started_at: ::aws_smithy_types::DateTime::from_secs(BASE_EPOCH_SECS),
        status: status.to_string(),
        events: (0..events)
            .map(|index| RepositorySyncEvent {
                event: format!("{marker}-event-{index}"),
                external_id: (index % 2 == 1).then(|| format!("external-{index}")),
                time: ::aws_smithy_types::DateTime::from_secs(BASE_EPOCH_SECS + index as i64),
                r#type: format!("{marker}-type-{index}"),
            })
            .collect(),
    }
}

/// One row of the recorded-attempt table: a branch, and the attempt recorded for it.
struct Row {
    /// What a failing assertion names this row by.
    label: String,
    branch: String,
    /// The status the recorded attempt carries.
    status: &'static str,
    /// How many events that attempt carries.
    events: usize,
}

impl Row {
    /// This row as the record the store holds.
    fn to_stored(&self) -> RepositorySyncAttempt {
        sync_attempt(&self.branch, self.status, self.events)
    }
}

/// The recorded-attempt table: every modelled `RepositorySyncStatus` crossed with the
/// branch forms, differing in how many events the attempt carries.
fn recorded_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (status_index, status) in REPOSITORY_SYNC_STATUSES.iter().enumerate() {
        for (branch_index, (branch_label, branch)) in branches().into_iter().enumerate() {
            rows.push(Row {
                label: format!("{status} on {branch_label}"),
                branch,
                status,
                events: (status_index + branch_index) % 4,
            });
        }
    }
    rows
}

/// Assert one wire `RepositorySyncAttempt` reports exactly the stored one, member for
/// member.
fn assert_reports_attempt(case: &str, reported: &Value, stored: &RepositorySyncAttempt) {
    assert_eq!(
        reported["Status"].as_str(),
        Some(stored.status.as_str()),
        "case {case:?}: LatestSync.Status must be the stored {:?}, got {reported:?}",
        stored.status
    );
    let started = epoch_seconds(case, &reported["StartedAt"]);
    assert_eq!(
        started,
        stored.started_at.as_secs_f64(),
        "case {case:?}: LatestSync.StartedAt must be the stored {}, got {started}",
        stored.started_at.as_secs_f64()
    );

    let events = reported["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: LatestSync.Events must be a list, got {reported:?}"));
    assert_eq!(
        events.len(),
        stored.events.len(),
        "case {case:?}: LatestSync.Events must report the {} stored events, got {reported:?}",
        stored.events.len()
    );
    for (index, (reported_event, stored_event)) in events.iter().zip(&stored.events).enumerate() {
        assert_eq!(
            reported_event["Event"].as_str(),
            Some(stored_event.event.as_str()),
            "case {case:?}: Events[{index}].Event must be the stored {:?}, got {reported_event:?}",
            stored_event.event
        );
        assert_eq!(
            reported_event["Type"].as_str(),
            Some(stored_event.r#type.as_str()),
            "case {case:?}: Events[{index}].Type must be the stored {:?}, got {reported_event:?}",
            stored_event.r#type
        );
        let at = epoch_seconds(case, &reported_event["Time"]);
        assert_eq!(
            at,
            stored_event.time.as_secs_f64(),
            "case {case:?}: Events[{index}].Time must be the stored {}, got {at}",
            stored_event.time.as_secs_f64()
        );
        match &stored_event.external_id {
            Some(external_id) => assert_eq!(
                reported_event["ExternalId"].as_str(),
                Some(external_id.as_str()),
                "case {case:?}: Events[{index}].ExternalId must be the stored {external_id:?}, \
                 got {reported_event:?}"
            ),
            None => assert!(
                reported_event["ExternalId"].is_null(),
                "case {case:?}: Events[{index}] records no ExternalId, got {reported_event:?}"
            ),
        }
    }
}

/// Assert one answer is this endpoint's "nothing is recorded for that triple" refusal.
fn assert_no_sync_history(case: &str, repository_link_id: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the recorded client-error status for a branch with no sync \
         history, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {body:?}"
    );
    let message = error_message(body);
    // The recorded wording, as a LITERAL: the point of asserting it is to pin what a client
    // reads, so it is not read back out of the constant the handler writes. It names the
    // link whose sync status could not be found, and carries none of the framework tail the
    // Git-sync refusals of this front carry.
    let expected =
        format!("Could not find a sync status for repository link: {repository_link_id}");
    assert_eq!(
        message, expected,
        "case {case:?}: expected the refusal message {expected:?}, got {message:?}"
    );
}

/// Assert one answer is the "no link under that id" refusal every reader of a
/// `RepositoryLinkId` answers an absent link with.
fn assert_no_such_link(case: &str, repository_link_id: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected the recorded client-error status for an id naming no link, \
         got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {body:?}"
    );
    let message = error_message(body);
    for needle in [
        format!("Repository not found for accountId: {ACCOUNT_ID} and uuid: {repository_link_id}"),
        "(Service: CodeConnections;".to_string(),
        "Status Code: 404;".to_string(),
        "Proxy: null)".to_string(),
    ] {
        assert!(
            message.contains(&needle),
            "case {case:?}: expected the refusal message to carry {needle:?}, got {message:?}"
        );
    }
}

/// A recorded attempt is reported member for member, and belongs to the triple it was
/// recorded against and to no other: a neighbouring branch, and the same branch under
/// another link, have no sync history.
#[tokio::test]
async fn a_recorded_repository_sync_attempt_is_reported_member_for_member() {
    let emulator = Emulator::new();
    let link_id = create_link(&emulator, "the synced link", "synced-repo").await;
    let other_link_id = create_link(&emulator, "a second link", "other-repo").await;

    for row in recorded_rows() {
        assert!(
            REPOSITORY_SYNC_STATUSES.contains(&row.status),
            "case {:?}: {:?} is not a modelled RepositorySyncStatus",
            row.label,
            row.status
        );

        for sync_type in SYNC_TYPES {
            let case = format!("{} under {sync_type}", row.label);
            let stored = row.to_stored();
            seed_attempt(&emulator, sync_type, &link_id, &row.branch, stored.clone());

            let (status, body) = get_status(&emulator, sync_type, &link_id, &row.branch).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetRepositorySyncStatus to answer 200, got {status} \
                 {body:?}"
            );
            assert_reports_attempt(&case, &body["LatestSync"], &stored);

            // The attempt belongs to this branch of this link and to nothing else.
            let neighbour = neighbour_branch(&row.branch, "-neighbour");
            let (status, body) = get_status(&emulator, sync_type, &link_id, &neighbour).await;
            assert_no_sync_history(
                &format!("neighbouring branch of {case}"),
                &link_id,
                status,
                &body,
            );

            let (status, body) = get_status(&emulator, sync_type, &other_link_id, &row.branch).await;
            assert_no_sync_history(
                &format!("another link of {case}"),
                &other_link_id,
                status,
                &body,
            );
        }
    }
}

/// A branch the store holds no attempt for is REFUSED — never an empty success, and never
/// the unmodelled 501 `NotImplemented` the scaffold answered — for every well-formed branch
/// form and every modelled sync type, even while the same link holds an attempt for another
/// branch.
#[tokio::test]
async fn a_branch_with_no_sync_history_is_refused() {
    let emulator = Emulator::new();
    let link_id = create_link(&emulator, "the link under test", "history-repo").await;

    // A branch that IS synced, so "no sync history" can never mean "the store is empty".
    seed_attempt(
        &emulator,
        SYNC_TYPE,
        &link_id,
        "already-synced",
        sync_attempt("live", REPOSITORY_SYNC_STATUSES[0], 2),
    );

    for sync_type in SYNC_TYPES {
        for (label, branch) in branches() {
            let case = format!("{label} under {sync_type}");
            let (status, body) = get_status(&emulator, sync_type, &link_id, &branch).await;
            assert_ne!(
                status, 501,
                "case {case:?}: the operation must not answer the scaffold's 501, got {body:?}"
            );
            assert!(
                body["LatestSync"].is_null(),
                "case {case:?}: no LatestSync may be reported for a sync nothing ran, got {body:?}"
            );
            assert_no_sync_history(&case, &link_id, status, &body);
        }
    }
}

/// An id naming no repository link is the SAME absence, worded as every other reader of a
/// `RepositoryLinkId` words it — including for an id whose link a delete has just freed,
/// even when an attempt is still recorded under that id.
#[tokio::test]
async fn an_id_naming_no_repository_link_is_refused() {
    let emulator = Emulator::new();
    let link_id = create_link(&emulator, "the link under test", "deleted-repo").await;
    seed_attempt(
        &emulator,
        SYNC_TYPE,
        &link_id,
        "main",
        sync_attempt("deleted", REPOSITORY_SYNC_STATUSES[0], 1),
    );

    // Ids of the modelled `RepositoryLinkId` form that name no link this service holds.
    for (label, id) in [
        (
            "a well-formed id never minted here",
            "11111111-2222-4333-8444-555555555555".to_string(),
        ),
        (
            "an upper-case id the pattern admits",
            "AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE".to_string(),
        ),
    ] {
        let (status, body) = get_status(&emulator, SYNC_TYPE, &id, "main").await;
        assert_no_such_link(label, &id, status, &body);
    }

    // The link goes away; the attempt recorded under its id does not make it resolvable.
    let (status, deleted) = emulator
        .call(
            "DeleteRepositoryLink",
            json!({ "RepositoryLinkId": link_id }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected DeleteRepositoryLink to answer 200, got {status} {deleted:?}"
    );
    let (status, body) = get_status(&emulator, SYNC_TYPE, &link_id, "main").await;
    assert_no_such_link("an id a delete has freed", &link_id, status, &body);
}

/// The report is a read: it changes nothing a later caller sees. Repeated reads answer
/// identically, and the set of triples the store holds an attempt for is exactly what was
/// seeded — a refused read records no attempt.
#[tokio::test]
async fn reading_the_sync_status_leaves_the_store_as_it_found_it() {
    let emulator = Emulator::new();
    let link_id = create_link(&emulator, "the link under test", "read-only-repo").await;

    let rows = recorded_rows();
    let mut seeded: Vec<(String, String, String)> = Vec::new();
    for row in &rows {
        seed_attempt(&emulator, SYNC_TYPE, &link_id, &row.branch, row.to_stored());
        let key = repository_sync_key(SYNC_TYPE, &link_id, &row.branch);
        if !seeded.contains(&key) {
            seeded.push(key);
        }
    }
    seeded.sort();
    let before = stored_triples(&emulator);

    for row in &rows {
        let mut answers: Vec<(u16, Value)> = Vec::new();
        for _ in 0..3 {
            answers.push(get_status(&emulator, SYNC_TYPE, &link_id, &row.branch).await);
        }
        let first = &answers[0];
        for (attempt, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer, first,
                "case {:?}: read {attempt} must answer exactly as the first did, got {answer:?}",
                row.label
            );
        }

        // And a read of a branch holding nothing, which must not record one.
        let unsynced = neighbour_branch(&row.branch, "-unsynced");
        let (status, body) = get_status(&emulator, SYNC_TYPE, &link_id, &unsynced).await;
        assert_no_sync_history(
            &format!("unsynced branch of {}", row.label),
            &link_id,
            status,
            &body,
        );
    }

    let after = stored_triples(&emulator);
    assert_eq!(
        after, before,
        "the reads must leave the stored triples as they were, expected {before:?}, got {after:?}"
    );
    assert_eq!(
        after, seeded,
        "only the seeded triples may hold an attempt, expected {seeded:?}, got {after:?}"
    );
}

/// The boundary of the modelled input constraints: a branch one character past the
/// published maximum, one short of the published minimum, and an id breaking the published
/// `RepositoryLinkId` pattern are refused with HTTP 400 `ValidationException` before the
/// handler runs — so the `ResourceNotFoundException` above is the handler's answer for a
/// well-formed triple, not a blanket refusal.
#[tokio::test]
async fn input_outside_the_modeled_constraints_is_refused() {
    let emulator = Emulator::new();
    let link_id = create_link(&emulator, "the link under test", "bounds-repo").await;

    for (label, id, branch) in [
        (
            "a branch one character past the published maximum",
            link_id.clone(),
            "b".repeat(BRANCH_MAX_LEN + 1),
        ),
        (
            "a branch one character short of the published minimum",
            link_id.clone(),
            String::new(),
        ),
        (
            "an id that is not of the published UUID form",
            "not-a-uuid".to_string(),
            "main".to_string(),
        ),
    ] {
        let (status, body) = get_status(&emulator, SYNC_TYPE, &id, &branch).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected GetRepositorySyncStatus to refuse the input with 400, got \
             {status} {body:?}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {label:?}: expected a ValidationException, got {body:?}"
        );
    }
}
