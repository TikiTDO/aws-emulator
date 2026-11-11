// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the refusal a read SCOPED to a `PillarId` the lens under
//! review does not define is answered with.
//!
//! The rule pinned here, over a TABLE of lenses, values and operations rather than the one
//! request an issue cited: a `PillarId` a request supplies is MATCHED against the pillars
//! the lens under review defines, so a value naming none of them is refused `400
//! ValidationException` whose `Reason` is `FIELD_VALIDATION_FAILED`, whose `Message` states
//! only the CLASS of the failure — and so carries no value from the request or the store —
//! and whose ONE `Fields` entry names the input member `PillarId` and states the rule the
//! value broke together with the pillars that lens DOES define: the valid values, in the
//! lens's own order. That is the envelope this service's recorded refusal of the same class
//! uses for a choice outside a question's catalog, rather than the lookup sentence a member
//! that IDENTIFIES a lens review or one of its questions is answered with. The valid values
//! are the LENS's own content, which is why the rows span two lenses with different pillar
//! sets: an AWS-official lens and an imported custom lens whose pillars are its document's.
//!
//! The rows differ along what the rule generalises over: the lens (official, imported),
//! the FORM of the value (a word no lens defines, a pillar of this lens in another case, a
//! pillar of the OTHER lens, a value at the member's published `@length` maximum) and the
//! operation the value arrives at (`ListAnswers`, `ListLensReviewImprovements`,
//! `ListReviewTemplateAnswers`). Each lens's own pillars are the positive control: they
//! still SELECT their questions.
//!
//! The expected valid-value list is read back over the wire from `GetLensReview`'s
//! `PillarReviewSummaries` rather than spelled out here, so the assertion compares two
//! read paths over the same lens content instead of restating a catalog.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the workload/template seeding are the shared
//! `super::workload_harness`; the import and publish wire helpers are the ones
//! `super::custom_lens_review_answers` already spells.

use serde_json::{json, Value};

use super::custom_lens_import::wait_for_the_import_to_land;
use super::custom_lens_review_answers::{import_shape, publish, Shape};
use super::workload_harness::{
    assert_fields_failed, created_id, encode_path_segment, minimal_create, review_template, Answer,
    Client,
};

/// The AWS-official lens this emulator ships a question set for.
const FRAMEWORK_LENS: &str = "wellarchitected";

/// The document the imported row publishes: `(pillar id, questions in it, choices per
/// question)`, with pillar ids of its OWN, so a valid-value list that names them can only
/// have come from the lens the request named.
const CUSTOM_SHAPE: Shape = &[("governance", 2, 2), ("platform", 1, 3)];

/// `PillarId`'s published bound, `@length(1, 64)`
/// (`smithy_query(query="shape", shape="PillarId")`): the longest value the member accepts
/// is still judged by the pillar rule rather than by that bound.
const PILLAR_ID_MAX_LENGTH: usize = 64;

/// A workload reviewed against `lens`, and its `WorkloadId`.
async fn workload_reviewing(client: &Client, case: &str, lens: &str) -> String {
    let mut request = minimal_create(&format!("wl-{case}"), &format!("tok-wl-{case}"));
    request["Lenses"] = json!([lens]);
    created_id(&client.create(request).await, case)
}

/// The `PillarId`s one lens review of a workload reports a summary for, in the order it
/// serves them — the lens's own pillar order, which is the order a refusal lists the valid
/// values in. Shared with `super::lens_content_validation`, which pins the same finding
/// over the official lens alone.
pub(crate) async fn reported_pillars(
    client: &Client,
    case: &str,
    workload_id: &str,
    lens: &str,
) -> Vec<String> {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(lens)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetLensReview must report the lens's pillars, got {answer:?}"
    );
    let pillars: Vec<String> = answer
        .body
        .get("LensReview")
        .and_then(|review| review.get("PillarReviewSummaries"))
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .filter_map(|summary| summary.get("PillarId"))
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    assert!(
        pillars.len() > 1,
        "case {case:?}: the lens under test must define several pillars, got {pillars:?}"
    );
    pillars
}

/// The three reads a `PillarId` scopes, each answered for one value: the answer list and
/// the improvement plan of a workload's lens review, and the answer list of a review
/// template's.
async fn scoped_reads(
    client: &Client,
    workload_id: &str,
    template_arn: &str,
    lens: &str,
    pillar_id: &str,
) -> Vec<(&'static str, Answer)> {
    let encoded_lens = encode_path_segment(lens);
    let review = format!("/workloads/{workload_id}/lensReviews/{encoded_lens}");
    vec![
        (
            "ListAnswers",
            client
                .send("GET", &format!("{review}/answers?PillarId={pillar_id}"), None)
                .await,
        ),
        (
            "ListLensReviewImprovements",
            client
                .send(
                    "GET",
                    &format!("{review}/improvements?PillarId={pillar_id}"),
                    None,
                )
                .await,
        ),
        (
            "ListReviewTemplateAnswers",
            client
                .list_review_template_answers(template_arn, lens, &format!("PillarId={pillar_id}"))
                .await,
        ),
    ]
}

/// A `PillarId` outside the lens under review is refused on every read it scopes, with the
/// finding naming `PillarId` and handing back that lens's own pillar set; a pillar the lens
/// DOES define still selects.
#[tokio::test]
async fn a_pillar_outside_the_lens_is_refused_with_that_lenss_own_pillars() {
    let client = Client::new();

    let custom_arn = import_shape(&client, "pillar-filter", CUSTOM_SHAPE).await;
    wait_for_the_import_to_land();
    publish(&client, "pillar-filter", &custom_arn).await;

    // The two lenses the rule is checked over, each with a pillar of the OTHER as one of
    // the values it must refuse.
    let rows: [(&str, String, &str); 2] = [
        ("official-lens", FRAMEWORK_LENS.to_string(), CUSTOM_SHAPE[0].0),
        ("imported-lens", custom_arn.clone(), "security"),
    ];

    for (lens_case, lens, foreign_pillar) in rows {
        let workload_id = workload_reviewing(&client, lens_case, &lens).await;
        let template_arn = review_template(&client, &format!("tpl-{lens_case}"), &[&lens]).await;
        let defined = reported_pillars(&client, lens_case, &workload_id, &lens).await;
        let sentence = format!(
            "PillarId contains an invalid pillar. Valid pillars are: [{}].",
            defined.join(", ")
        );

        let undefined: [(&str, String); 4] = [
            ("word-no-lens-defines", "notapillar".to_string()),
            ("own-pillar-in-another-case", defined[0].to_uppercase()),
            ("pillar-of-the-other-lens", foreign_pillar.to_string()),
            ("value-at-the-published-maximum", "p".repeat(PILLAR_ID_MAX_LENGTH)),
        ];

        for (form, pillar_id) in undefined {
            for (operation, answer) in
                scoped_reads(&client, &workload_id, &template_arn, &lens, &pillar_id).await
            {
                let case = format!("{lens_case}/{form}/{operation}");
                assert_fields_failed(&case, &answer, "PillarId", &sentence);
            }
        }

        // Every pillar the lens DOES define still selects on every one of those reads, so
        // the rule refuses values rather than the filter itself.
        for pillar_id in &defined {
            for (operation, answer) in
                scoped_reads(&client, &workload_id, &template_arn, &lens, pillar_id).await
            {
                let case = format!("{lens_case}/{pillar_id}/{operation}");
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a pillar the lens defines must still select, got {answer:?}"
                );
            }
        }
    }
}
