// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the name a 409 `ConflictException` names when a request
//! collides with a name already taken.
//!
//! The rule these pin, over a TABLE of spellings and every resource of this service whose
//! name is metered for uniqueness rather than one recorded request: the comparison that
//! finds the collision is the service's loose one (capitalization and surrounding
//! whitespace ignored), `ResourceId` identifies the resource that is in the way, and WHICH
//! spelling of the name the sentence carries is each resource's own recorded fact — a
//! workload collision names the workload as STORED, while a review template's and a
//! milestone's quote the name AS THE REFUSED REQUEST SUBMITTED IT, padding, capitalization
//! and all. Normalization stays confined to the comparison either way.
//!
//! The rows cover the three names this service meters: a workload's (`CreateWorkload` and
//! the rename `UpdateWorkload` performs), a review template's (`CreateReviewTemplate` and
//! `UpdateReviewTemplate`) and a milestone's within its workload (`CreateMilestone`).
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{
    created_arn, created_id, encode_path_segment, minimal_create, minimal_template, Answer, Client,
};

/// `MilestoneName`, `WorkloadName` and `TemplateName` are each `@length(min: 3, max: 100)`
/// (`smithy_query(query="shape", shape="MilestoneName")` / `"WorkloadName"` /
/// `"TemplateName"`), so every spelling a row submits stays inside that bound.
const NAME_RANGE: (usize, usize) = (3, 100);

fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// The spellings a row submits against a stored name, each of which the service's loose
/// comparison treats as that same name: the identical one, both capitalizations, the name
/// padded at either end, the padding on one side only, and the inner whitespace changed.
fn collisions(stored: &str) -> Vec<(String, String)> {
    vec![
        ("identical".to_string(), stored.to_string()),
        ("upper".to_string(), stored.to_uppercase()),
        ("lower".to_string(), stored.to_lowercase()),
        ("padded-both-ends".to_string(), format!("  {stored}  ")),
        ("padded-leading".to_string(), format!(" {stored}")),
        ("padded-trailing".to_string(), format!("{stored}\t")),
        ("inner-space-removed".to_string(), stored.replace(' ', "")),
        (
            "upper-and-padded".to_string(),
            format!(" {} ", stored.to_uppercase()),
        ),
    ]
}

/// The refusal every row owes: 409 `ConflictException` of the named resource type, with
/// `ResourceId` identifying the resource in the way and the sentence this resource's
/// collision is worded with.
fn assert_conflict(
    case: &str,
    answer: &Answer,
    resource_type: &str,
    resource_id: &str,
    expected_message: &str,
) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected HTTP 409, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ConflictException",
        "case {case:?}: expected ConflictException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceType"),
        resource_type,
        "case {case:?}: expected ResourceType {resource_type}, got {answer:?}"
    );
    assert_eq!(
        member(answer, "ResourceId"),
        resource_id,
        "case {case:?}: ResourceId must identify the resource in the way, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Message"),
        expected_message,
        "case {case:?}: expected the message {expected_message:?}, got {answer:?}"
    );
}

/// Every spelling a row submits must stay inside the member's published bound, so the row
/// is refused on the NAME it collides with rather than on its length.
fn assert_inside_the_bound(case: &str, submitted: &str) {
    assert!(
        (NAME_RANGE.0..=NAME_RANGE.1).contains(&submitted.len()),
        "case {case:?}: the row must stay inside the name's @length{NAME_RANGE:?}, got \
         {submitted:?}"
    );
}

/// A workload-name collision, through both operations that take a `WorkloadName`: the
/// sentence names the workload already holding the name, by the spelling it is STORED
/// under, whatever spelling the refused request submitted.
#[tokio::test]
async fn a_workload_name_collision_names_the_stored_spelling() {
    // Rows differ along the stored spelling: one plain, one with inner spaces, one mixed
    // case — so the submitted spelling is not simply the stored one in disguise.
    for (index, stored_name) in ["wl-plain", "work load name", "MiXeD Case Workload"]
        .iter()
        .enumerate()
    {
        let client = Client::new();
        let holder = created_id(
            &client
                .create(minimal_create(stored_name, &format!("tok-holder-{index}")))
                .await,
            stored_name,
        );
        let other = created_id(
            &client
                .create(minimal_create(
                    &format!("other-{index}"),
                    &format!("tok-other-{index}"),
                ))
                .await,
            "other",
        );

        for (spelling, submitted) in collisions(stored_name) {
            let case = format!("{stored_name:?}/{spelling}");
            assert_inside_the_bound(&case, &submitted);
            let expected =
                format!("[Conflict] A workload with name {stored_name} already exists.");

            let created = client
                .create(minimal_create(
                    &submitted,
                    &format!("tok-dup-{index}-{spelling}"),
                ))
                .await;
            assert_conflict(
                &format!("{case}/CreateWorkload"),
                &created,
                "WORKLOAD",
                &holder,
                &expected,
            );

            let renamed = client
                .update(&other, json!({ "WorkloadName": submitted }))
                .await;
            assert_conflict(
                &format!("{case}/UpdateWorkload"),
                &renamed,
                "WORKLOAD",
                &holder,
                &expected,
            );
        }
    }
}

/// A review-template-name collision, through both operations that take a `TemplateName`.
#[tokio::test]
async fn a_review_template_name_collision_quotes_the_submitted_spelling() {
    for (index, stored_name) in ["tpl-plain", "template name", "MiXeD Case Template"]
        .iter()
        .enumerate()
    {
        let client = Client::new();
        let holder = created_arn(
            &client
                .create_review_template(minimal_template(
                    stored_name,
                    &format!("tok-holder-{index}"),
                ))
                .await,
            stored_name,
        );
        let other_name = format!("other-template-{index}");
        let other = created_arn(
            &client
                .create_review_template(minimal_template(
                    &other_name,
                    &format!("tok-other-{index}"),
                ))
                .await,
            &other_name,
        );

        for (spelling, submitted) in collisions(stored_name) {
            let case = format!("{stored_name:?}/{spelling}");
            assert_inside_the_bound(&case, &submitted);
            let expected =
                format!("[Conflict] A review template with name {submitted} already exists.");

            let created = client
                .create_review_template(minimal_template(
                    &submitted,
                    &format!("tok-dup-{index}-{spelling}"),
                ))
                .await;
            assert_conflict(
                &format!("{case}/CreateReviewTemplate"),
                &created,
                "REVIEW_TEMPLATE",
                &holder,
                &expected,
            );

            let renamed = client
                .update_review_template(&other, json!({ "TemplateName": submitted }))
                .await;
            assert_conflict(
                &format!("{case}/UpdateReviewTemplate"),
                &renamed,
                "REVIEW_TEMPLATE",
                &holder,
                &expected,
            );
        }
    }
}

/// A milestone-name collision, which is scoped to ONE workload: the message quotes the
/// submitted spelling and names the workload the uniqueness is scoped to, and `ResourceId`
/// is the composite id of the milestone already holding the name.
#[tokio::test]
async fn a_milestone_name_collision_quotes_the_submitted_spelling() {
    for (index, stored_name) in ["ms-plain", "mile stone name", "MiXeD Case Milestone"]
        .iter()
        .enumerate()
    {
        let client = Client::new();
        let workload_id = created_id(
            &client
                .create(minimal_create(
                    &format!("milestone-host-{index}"),
                    &format!("tok-host-{index}"),
                ))
                .await,
            "host",
        );
        let recorded = client
            .send(
                "POST",
                &format!("/workloads/{workload_id}/milestones"),
                Some(json!({
                    "MilestoneName": stored_name,
                    "ClientRequestToken": format!("tok-first-{index}"),
                })),
            )
            .await;
        assert_eq!(
            recorded.status, 200,
            "case {stored_name:?}: the first CreateMilestone must succeed, got {recorded:?}"
        );
        let number = recorded
            .body
            .get("MilestoneNumber")
            .and_then(Value::as_i64)
            .unwrap_or_default();

        for (spelling, submitted) in collisions(stored_name) {
            let case = format!("{stored_name:?}/{spelling}");
            assert_inside_the_bound(&case, &submitted);
            let refused = client
                .send(
                    "POST",
                    &format!("/workloads/{workload_id}/milestones"),
                    Some(json!({
                        "MilestoneName": submitted,
                        "ClientRequestToken": format!("tok-dup-{index}-{spelling}"),
                    })),
                )
                .await;
            assert_conflict(
                &case,
                &refused,
                "MILESTONE",
                &format!("{workload_id}.{number}"),
                &format!(
                    "[Conflict] Milestone with name {submitted} already exists under workload \
                     {workload_id}. Capitalization and whitespace are ignored in milestone names."
                ),
            );
        }

        // The refusals recorded nothing: the workload still holds the one milestone, under
        // the spelling it was stored with.
        let listed = client
            .send(
                "POST",
                &format!("/workloads/{workload_id}/milestonesSummaries"),
                Some(json!({})),
            )
            .await;
        let summaries = listed
            .body
            .get("MilestoneSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries.len(),
            1,
            "case {stored_name:?}: every colliding request must record nothing, got {listed:?}"
        );
        assert_eq!(
            summaries
                .first()
                .and_then(|summary| summary.get("MilestoneName"))
                .and_then(Value::as_str),
            Some(*stored_name),
            "case {stored_name:?}: the stored milestone keeps the name it was created \
             with, got {listed:?}"
        );
    }
}

/// The collision is about the NAME, not about the resource's own identity: a rename to the
/// resource's OWN name, in any spelling the comparison treats as it, is accepted and stored
/// as sent. The counterpart of the rows above, so a message quoting the submitted spelling
/// cannot come from refusing requests that should succeed.
#[tokio::test]
async fn renaming_a_resource_to_its_own_name_is_not_a_collision() {
    let client = Client::new();
    const WORKLOAD_NAME: &str = "self rename workload";
    const TEMPLATE_NAME: &str = "self rename template";

    let workload_id = created_id(
        &client
            .create(minimal_create(WORKLOAD_NAME, "tok-self-workload"))
            .await,
        "workload",
    );
    let template_arn = created_arn(
        &client
            .create_review_template(minimal_template(TEMPLATE_NAME, "tok-self-template"))
            .await,
        "template",
    );

    for (spelling, submitted) in collisions(WORKLOAD_NAME) {
        let answer = client
            .update(&workload_id, json!({ "WorkloadName": submitted.clone() }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {spelling:?}: renaming a workload to its own name must be accepted, got \
             {answer:?}"
        );
        let read = client.get(&workload_id).await;
        assert_eq!(
            read.body
                .get("Workload")
                .and_then(|workload| workload.get("WorkloadName"))
                .and_then(Value::as_str),
            Some(submitted.as_str()),
            "case {spelling:?}: the stored name must be the spelling as sent, got {read:?}"
        );
    }

    for (spelling, submitted) in collisions(TEMPLATE_NAME) {
        let answer = client
            .update_review_template(&template_arn, json!({ "TemplateName": submitted.clone() }))
            .await;
        assert_eq!(
            answer.status, 200,
            "case {spelling:?}: renaming a template to its own name must be accepted, got \
             {answer:?}"
        );
        let read = client
            .send(
                "GET",
                &format!("/reviewTemplates/{}", encode_path_segment(&template_arn)),
                None,
            )
            .await;
        assert_eq!(
            read.body
                .get("ReviewTemplate")
                .and_then(|template| template.get("TemplateName"))
                .and_then(Value::as_str),
            Some(submitted.as_str()),
            "case {spelling:?}: the stored name must be the spelling as sent, got {read:?}"
        );
    }
}
