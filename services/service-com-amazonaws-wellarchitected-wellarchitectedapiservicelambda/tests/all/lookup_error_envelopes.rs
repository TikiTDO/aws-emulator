// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the envelopes this service answers a failed LOOKUP and a
//! name COLLISION with, plus the milestone lifecycle those lookups resolve against.
//!
//! Two rules are pinned here, each over a table of inputs rather than one recorded
//! request:
//!
//! 1. A path-bound resource identifier is RESOLVED, not validated: a value naming no
//!    stored resource is answered `404 ResourceNotFoundException` carrying the
//!    identifier as sent and the missing resource's type — whatever the value's syntax,
//!    so a `WorkloadId` outside `^[0-9a-f]{32}$` and a `MilestoneNumber` outside
//!    `@range(1, 100)` take the lookup's answer, not a 400.
//! 2. The `Message` of such a failure is built from the request and the owning account:
//!    a not-found names the resource asked for and the account it was looked for in; a
//!    collision names the resource already holding the value, as stored.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{created_id, minimal_create, Answer, Client};

/// A well-formed `WorkloadId` (32 lowercase hex) that no workload carries.
const ABSENT_HEX_ID: &str = "00000000000000000000000000000000";

/// The not-found `Message` the service builds for a workload lookup: the id asked for
/// plus the account it was looked for in. The account comes from the emulator's single
/// source of truth, never a literal.
fn workload_not_found_message(workload_id: &str) -> String {
    format!(
        "[ResourceNotFound] No workload with ID {workload_id} was found in account {}.",
        engine::emulator_account_id()
    )
}

/// The same for a milestone lookup, which names the number and the workload it was
/// looked for in.
fn milestone_not_found_message(workload_id: &str, milestone_number: i64) -> String {
    format!(
        "[ResourceNotFound] No milestone with number {milestone_number} in a workload with ID \
         {workload_id} was found in account {}.",
        engine::emulator_account_id()
    )
}

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

async fn create_milestone(client: &Client, workload_id: &str, name: &str, token: &str) -> Answer {
    client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({ "MilestoneName": name, "ClientRequestToken": token })),
        )
        .await
}

async fn get_milestone(client: &Client, workload_id: &str, milestone_number: i64) -> Answer {
    client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/milestones/{milestone_number}"),
            None,
        )
        .await
}

/// Rule 1 + 2 for a workload id, on every operation that resolves one. The rows differ
/// along the SYNTAX of the id — well-formed-but-absent, and six shapes the modelled
/// `@length(32, 32)` / `@pattern("^[0-9a-f]{32}$")` rejects — because the lookup, not
/// input validation, owns a path id: every row is the same 404, naming the id exactly
/// as sent.
#[tokio::test]
async fn an_unresolvable_workload_id_is_a_not_found_naming_the_id_and_the_account() {
    let client = Client::new();
    // One workload exists, so the 404s below are a real miss and not an empty store.
    let present = created_id(
        &client.create(minimal_create("present-workload", "tok-present")).await,
        "present",
    );

    let cases: Vec<(&str, String)> = vec![
        ("well-formed-absent-zeros", ABSENT_HEX_ID.to_string()),
        ("well-formed-absent-f", "f".repeat(32)),
        (
            "well-formed-absent-mixed",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        // Below here every id breaks the modelled WorkloadId constraints.
        ("not-hex-at-all", "not-a-workload-id".to_string()),
        ("one-character-short", "0".repeat(31)),
        ("one-character-long", "0".repeat(33)),
        ("uppercase-hex", "A".repeat(32)),
        ("hex-with-a-non-hex-digit", format!("{}z", "0".repeat(31))),
        ("digits-only-but-short", "12345".to_string()),
    ];

    for (case, workload_id) in cases {
        assert_ne!(
            workload_id, present,
            "case {case:?}: the row must not name the workload that exists"
        );
        for (operation, answer) in [
            ("GetWorkload", client.get(&workload_id).await),
            (
                "UpdateWorkload",
                client
                    .update(&workload_id, json!({"Description": "no such workload"}))
                    .await,
            ),
            (
                "DeleteWorkload",
                client.delete(&workload_id, &format!("tok-{case}")).await,
            ),
            (
                "CreateMilestone",
                create_milestone(&client, &workload_id, "a-milestone", &format!("ms-{case}")).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{operation}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceType"),
                "WORKLOAD",
                "case {case:?}/{operation}: expected ResourceType WORKLOAD, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceId"),
                workload_id,
                "case {case:?}/{operation}: ResourceId must be the id as sent, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "Message"),
                workload_not_found_message(&workload_id),
                "case {case:?}/{operation}: expected the id/account not-found message, got {answer:?}"
            );
        }
    }

    // The workload that DOES exist still resolves, so the rows above fail on resolution
    // and not on a broken route.
    let fetched = client.get(&present).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored workload must still resolve, got {fetched:?}"
    );
}

/// Rule 1 + 2 for a milestone, which is identified by the `(WorkloadId,
/// MilestoneNumber)` pair: the 404 names the composite `<workloadId>.<milestoneNumber>`
/// and the resource type `MILESTONE`. Rows differ along the number — each published
/// bound of `@range(1, 100)`, one step past each bound, and a recorded workload against
/// an absent one — so a number the model refuses is answered by the lookup all the same.
#[tokio::test]
async fn an_unrecorded_milestone_is_a_not_found_naming_the_composite_id() {
    let client = Client::new();
    let workload_id = created_id(
        &client.create(minimal_create("milestone-host", "tok-host")).await,
        "host",
    );
    // Milestone 1 is recorded, so every row below is a real miss within a workload that
    // does have milestones.
    let recorded = create_milestone(&client, &workload_id, "first-milestone", "tok-ms-1").await;
    assert_eq!(
        recorded.status, 200,
        "case \"recorded\": CreateMilestone must succeed, got {recorded:?}"
    );

    // Rows: inside the published range but never recorded, each bound, one past each
    // bound, and a negative number.
    let numbers: [(&str, i64); 6] = [
        ("inside-the-range", 7),
        ("range-minimum", 1),
        ("range-maximum", 100),
        ("one-below-the-range", 0),
        ("one-above-the-range", 101),
        ("negative", -1),
    ];

    for (case, number) in numbers {
        for (host, host_id) in [
            ("existing-workload", workload_id.as_str()),
            ("absent-workload", ABSENT_HEX_ID),
        ] {
            // Number 1 of the existing workload IS recorded; that row is the round-trip
            // below, not a miss.
            if host == "existing-workload" && number == 1 {
                continue;
            }
            let answer = get_milestone(&client, host_id, number).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{host}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}/{host}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceType"),
                "MILESTONE",
                "case {case:?}/{host}: expected ResourceType MILESTONE, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceId"),
                format!("{host_id}.{number}"),
                "case {case:?}/{host}: ResourceId must be <workloadId>.<number>, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "Message"),
                milestone_not_found_message(host_id, number),
                "case {case:?}/{host}: expected the number/id/account not-found message, got {answer:?}"
            );
        }
    }
}

/// Rule 2 for a collision: the 409 identifies the resource already holding the value
/// through its `ResourceId`, and its message names that resource by the name it is STORED
/// under rather than the spelling the refused request submitted. Rows differ along the
/// spelling submitted (the uniqueness check ignores spaces and capitalization) and along
/// which operation submits it, so every row must read back the one stored spelling.
#[tokio::test]
async fn a_name_collision_is_a_conflict_naming_the_workload_that_holds_the_name() {
    let client = Client::new();
    const STORED_NAME: &str = "Shared Name";
    let original = created_id(
        &client.create(minimal_create(STORED_NAME, "tok-original")).await,
        "original",
    );
    let other = created_id(
        &client.create(minimal_create("other-name", "tok-other")).await,
        "other",
    );

    for (case, submitted) in [
        ("exact", "Shared Name"),
        ("upper", "SHARED NAME"),
        ("lower", "shared name"),
        ("spaces-removed", "SharedName"),
        ("extra-spaces", "  Shared   Name  "),
    ] {
        for (operation, answer) in [
            (
                "CreateWorkload",
                client
                    .create(minimal_create(submitted, &format!("tok-dup-{case}")))
                    .await,
            ),
            (
                "UpdateWorkload",
                client
                    .update(&other, json!({ "WorkloadName": submitted }))
                    .await,
            ),
        ] {
            assert_eq!(
                answer.status, 409,
                "case {case:?}/{operation}: expected HTTP 409, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ConflictException",
                "case {case:?}/{operation}: expected ConflictException, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceType"),
                "WORKLOAD",
                "case {case:?}/{operation}: expected ResourceType WORKLOAD, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceId"),
                original,
                "case {case:?}/{operation}: ResourceId must name the workload holding the name, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "Message"),
                format!("[Conflict] A workload with name {STORED_NAME} already exists."),
                "case {case:?}/{operation}: the message must name the STORED workload name, got {answer:?}"
            );
        }
    }

    // A milestone name is unique within its workload, and the conflict is worded the
    // same way — with the composite id of the milestone already holding the name, the
    // workload the uniqueness is scoped to, and the comparison rule. The comparison is
    // the service's loose one, so every spelling that differs only in capitalization or
    // whitespace is the same 409, each quoting the spelling THAT request submitted.
    let host = created_id(
        &client.create(minimal_create("milestone-conflict-host", "tok-mc")).await,
        "milestone-host",
    );
    for (case, name) in [("plain", "a-milestone"), ("with-spaces", "a milestone")] {
        let first = create_milestone(&client, &host, name, &format!("tok-first-{case}")).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first CreateMilestone must succeed, got {first:?}"
        );
        let number = first
            .body
            .get("MilestoneNumber")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        for (spelling, submitted) in [
            ("exact", name.to_string()),
            ("upper", name.to_uppercase()),
            ("padded", format!("  {name}  ")),
            ("spaces-removed", name.replace(' ', "")),
        ] {
            let again = create_milestone(
                &client,
                &host,
                &submitted,
                &format!("tok-again-{case}-{spelling}"),
            )
            .await;
            assert_eq!(
                again.status, 409,
                "case {case:?}/{spelling}: a repeated MilestoneName must be a 409, got {again:?}"
            );
            assert_eq!(
                again.error_code, "ConflictException",
                "case {case:?}/{spelling}: expected ConflictException, got {again:?}"
            );
            assert_eq!(
                member(&again, "ResourceType"),
                "MILESTONE",
                "case {case:?}/{spelling}: expected ResourceType MILESTONE, got {again:?}"
            );
            assert_eq!(
                member(&again, "ResourceId"),
                format!("{host}.{number}"),
                "case {case:?}/{spelling}: ResourceId must name the milestone holding the \
                 name, got {again:?}"
            );
            assert_eq!(
                member(&again, "Message"),
                format!(
                    "[Conflict] Milestone with name {submitted} already exists under workload \
                     {host}. Capitalization and whitespace are ignored in milestone names."
                ),
                "case {case:?}/{spelling}: the message must quote the submitted name and the \
                 workload it is unique within, got {again:?}"
            );
        }
    }
}

/// A recorded milestone reads back through `GetMilestone`, carries the workload as it
/// stood when it was recorded, and keeps its number. Rows differ along how many
/// milestones the workload holds, so the numbering is pinned over a span rather than at
/// one value.
#[tokio::test]
async fn created_milestones_round_trip_through_get_milestone() {
    let client = Client::new();
    let workload_id = created_id(
        &client.create(minimal_create("round-trip-host", "tok-rt")).await,
        "host",
    );

    for expected_number in 1i64..=4 {
        let case = format!("milestone-{expected_number}");
        let name = format!("snapshot-{expected_number}");
        let created = create_milestone(
            &client,
            &workload_id,
            &name,
            &format!("tok-rt-{expected_number}"),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateMilestone must succeed, got {created:?}"
        );
        assert_eq!(
            created.body.get("MilestoneNumber").and_then(Value::as_i64),
            Some(expected_number),
            "case {case:?}: milestone numbers run from 1 upwards, got {created:?}"
        );
        assert_eq!(
            member(&created, "WorkloadId"),
            workload_id,
            "case {case:?}: the output must name the workload, got {created:?}"
        );

        // The same token with the same parameters replays instead of recording a
        // second milestone.
        let replay = create_milestone(
            &client,
            &workload_id,
            &name,
            &format!("tok-rt-{expected_number}"),
        )
        .await;
        assert_eq!(
            replay.status, 200,
            "case {case:?}: an idempotent replay must succeed, got {replay:?}"
        );
        assert_eq!(
            replay.body.get("MilestoneNumber").and_then(Value::as_i64),
            Some(expected_number),
            "case {case:?}: a replay must return the original number, got {replay:?}"
        );

        let fetched = get_milestone(&client, &workload_id, expected_number).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetMilestone must find the recorded milestone, got {fetched:?}"
        );
        assert_eq!(
            member(&fetched, "WorkloadId"),
            workload_id,
            "case {case:?}: GetMilestone must name the workload, got {fetched:?}"
        );
        let milestone = fetched
            .body
            .get("Milestone")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            milestone.get("MilestoneNumber").and_then(Value::as_i64),
            Some(expected_number),
            "case {case:?}: the milestone must report its number, got {fetched:?}"
        );
        assert_eq!(
            milestone.get("MilestoneName").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: the milestone must report the name it was recorded with, got {fetched:?}"
        );
        assert!(
            milestone.get("RecordedAt").is_some(),
            "case {case:?}: the milestone must report when it was recorded, got {fetched:?}"
        );
        assert_eq!(
            milestone
                .get("Workload")
                .and_then(|w| w.get("WorkloadId"))
                .and_then(Value::as_str),
            Some(workload_id.as_str()),
            "case {case:?}: the milestone must carry the workload it recorded, got {fetched:?}"
        );
    }
}

/// A milestone is a snapshot: updating the workload afterwards changes what
/// `GetWorkload` reports and leaves every recorded milestone alone. Deleting the
/// workload takes its milestones with it, so they stop resolving.
#[tokio::test]
async fn a_recorded_milestone_is_unaffected_by_later_workload_changes() {
    let client = Client::new();
    let workload_id = created_id(
        &client.create(minimal_create("snapshot-host", "tok-snap")).await,
        "host",
    );
    let recorded = create_milestone(&client, &workload_id, "before-the-edit", "tok-snap-1").await;
    assert_eq!(
        recorded.status, 200,
        "case \"record\": CreateMilestone must succeed, got {recorded:?}"
    );

    // Rows differ along which member the later update changes.
    for (case, member_name, value) in [
        ("description", "Description", json!("an edited description")),
        ("environment", "Environment", json!("PREPRODUCTION")),
        ("notes", "Notes", json!("a note added after the milestone")),
    ] {
        let updated = client
            .update(&workload_id, json!({ member_name: value.clone() }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: UpdateWorkload must succeed, got {updated:?}"
        );

        let fetched = get_milestone(&client, &workload_id, 1).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the milestone must still resolve, got {fetched:?}"
        );
        let snapshot = fetched
            .body
            .get("Milestone")
            .and_then(|m| m.get("Workload"))
            .cloned()
            .unwrap_or(Value::Null);
        assert_ne!(
            snapshot.get(member_name).cloned().unwrap_or(Value::Null),
            value,
            "case {case:?}: the recorded milestone must not take the later edit, got {fetched:?}"
        );
        // The live workload DID take it, so the test is comparing against a real change.
        let live = client.get(&workload_id).await;
        assert_eq!(
            live.body
                .get("Workload")
                .and_then(|w| w.get(member_name))
                .cloned()
                .unwrap_or(Value::Null),
            value,
            "case {case:?}: the live workload must report the edit, got {live:?}"
        );
    }

    // Deleting the workload drops its milestones: the lookup answers the composite 404.
    let deleted = client.delete(&workload_id, "tok-snap-delete").await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete\": DeleteWorkload must succeed, got {deleted:?}"
    );
    let orphan = get_milestone(&client, &workload_id, 1).await;
    assert_eq!(
        orphan.status, 404,
        "case \"delete\": a milestone of a deleted workload must not resolve, got {orphan:?}"
    );
    assert_eq!(
        member(&orphan, "ResourceId"),
        format!("{workload_id}.1"),
        "case \"delete\": the 404 must name the composite id, got {orphan:?}"
    );
    assert_eq!(
        member(&orphan, "Message"),
        milestone_not_found_message(&workload_id, 1),
        "case \"delete\": the 404 must name the number, the workload and the account, got {orphan:?}"
    );
}

/// Skipping constraint validation on a path-bound id does not open the BODY: the
/// members `CreateMilestone` validates are still refused with 400, at each published
/// `@length` bound and one step past it.
#[tokio::test]
async fn create_milestone_still_refuses_an_out_of_bounds_body_with_400() {
    let client = Client::new();
    let workload_id = created_id(
        &client.create(minimal_create("validation-host", "tok-val")).await,
        "host",
    );

    // `MilestoneName` is @length(min: 3, max: 100); `ClientRequestToken` is
    // @length(min: 1, max: 2048) (smithy_query: CreateMilestone input).
    for (case, name, token) in [
        ("name-one-below-the-minimum", "ab".to_string(), "tok-a".to_string()),
        ("name-one-above-the-maximum", "n".repeat(101), "tok-b".to_string()),
        ("token-empty", "a-valid-name".to_string(), String::new()),
        (
            "token-one-above-the-maximum",
            "another-valid-name".to_string(),
            "t".repeat(2049),
        ),
    ] {
        let answer = create_milestone(&client, &workload_id, &name, &token).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // Each published bound itself is accepted.
    for (case, name, token) in [
        ("name-at-the-minimum", "abc".to_string(), "tok-min".to_string()),
        ("name-at-the-maximum", "n".repeat(100), "tok-max".to_string()),
        ("token-at-the-maximum", "name-for-long-token".to_string(), "t".repeat(2048)),
    ] {
        let answer = create_milestone(&client, &workload_id, &name, &token).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a value on the published bound must be accepted, got {answer:?}"
        );
    }
}

/// "A workload can have a maximum of 100 milestones" (`MilestoneNumber` doc), which is
/// also why the number is `@range(1, 100)`: the hundredth is accepted and the
/// hundred-and-first is refused with the modelled `ServiceQuotaExceededException`
/// (402), so the milestone map cannot grow without bound. Checked one below the quota,
/// at the quota, and one past it.
#[tokio::test]
async fn the_milestone_quota_bounds_one_workload() {
    const QUOTA: i32 = service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state::MAX_MILESTONES_PER_WORKLOAD;

    let client = Client::new();
    let workload_id = created_id(
        &client.create(minimal_create("quota-host", "tok-quota")).await,
        "host",
    );

    for number in 1..=QUOTA {
        let answer = create_milestone(
            &client,
            &workload_id,
            &format!("milestone-{number}"),
            &format!("tok-quota-{number}"),
        )
        .await;
        let case = format!("milestone-{number}");
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a milestone within the quota must be recorded, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("MilestoneNumber").and_then(Value::as_i64),
            Some(i64::from(number)),
            "case {case:?}: the numbers must run 1..={QUOTA}, got {answer:?}"
        );
    }

    let past = create_milestone(
        &client,
        &workload_id,
        "one-too-many",
        "tok-quota-past",
    )
    .await;
    assert_eq!(
        past.status, 402,
        "case \"one-past-the-quota\": expected HTTP 402, got {past:?}"
    );
    assert_eq!(
        past.error_code, "ServiceQuotaExceededException",
        "case \"one-past-the-quota\": expected ServiceQuotaExceededException, got {past:?}"
    );

    // The last milestone inside the quota still resolves, so the refusal above is the
    // quota and not a broken store.
    let last = get_milestone(&client, &workload_id, i64::from(QUOTA)).await;
    assert_eq!(
        last.status, 200,
        "case \"at-the-quota\": the hundredth milestone must resolve, got {last:?}"
    );
}
