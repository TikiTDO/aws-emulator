// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the refusal a request that NAMES lens content gets when that
//! content does not resolve.
//!
//! The rule pinned here, over TABLES of operations and values rather than one recorded
//! request: a lens a request asks to APPLY, the lens whose review of a workload or of a
//! review template a request addresses, and the question, pillar or choice of such a review
//! are VALUES the request carries, not resources this API identifies — so a value of that
//! kind that resolves to nothing is refused as INPUT, `400 ValidationException` naming the
//! member that carried it, and never reported as a resource that was not found. The
//! resources this API does identify — the workload, the review template, the milestone —
//! keep answering `404 ResourceNotFoundException`, which is what lets a client tell "fix
//! the request" from "fix the state".
//!
//! Each table spans the class the rule generalises over rather than the one instance an
//! issue cited: every operation that resolves such a value, and for each the spellings and
//! forms the value can take (a bare word, an alias in another case, an official lens ARN, a
//! custom lens ARN, a question or pillar of ANOTHER lens, a choice of another question).
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client, the workload/template seeding and the two refusal
//! assertions are the shared `super::workload_harness`.

use serde_json::{json, Value};

use super::pillar_filter_refusal::reported_pillars;
use super::workload_harness::{
    assert_field_validation, assert_fields_failed, assert_unresolvable_lens, created_id,
    encode_path_segment, member, minimal_create, official_lens_arn, review_template, Answer,
    Client, SIGNED_REGION,
};

/// The AWS-official lens this emulator ships a question set for, and one AWS-official lens
/// it ships none for — both published aliases (`state::AWS_OFFICIAL_LENS_ALIASES`), so a
/// row that names the second one names a lens that EXISTS and is simply not under review.
const FRAMEWORK_LENS: &str = "wellarchitected";
const UNREVIEWED_LENS: &str = "serverless";

/// A `WorkloadId` of the published form (`^[0-9a-f]{32}$`) that no workload carries, and a
/// `TemplateArn` of the published form that no review template carries.
const ABSENT_WORKLOAD_ID: &str = "0123456789abcdef0123456789abcdef";

/// The `QuestionId` and `PillarId` the framework lens's catalog really carries, used as the
/// resolvable value a row is varied against.
const FRAMEWORK_QUESTION: &str = "organization-priorities";
const FRAMEWORK_PILLAR: &str = "operationalExcellence";

/// A `TemplateArn` of the published form that nothing is stored under.
fn absent_template_arn() -> String {
    format!(
        "arn:aws:wellarchitected:{SIGNED_REGION}:{}:review-template/{}",
        engine::emulator_account_id(),
        "0".repeat(32)
    )
}

/// A custom-form lens ARN of this account that no import minted.
fn absent_custom_lens_arn() -> String {
    format!(
        "arn:aws:wellarchitected:{SIGNED_REGION}:{}:lens/{}",
        engine::emulator_account_id(),
        "0".repeat(32)
    )
}

/// The forms a lens name a request carries can take that resolve to NO lens of this
/// account: a bare word, a near miss of a published alias, a published alias in another
/// case (catalog membership is exact — AWS publishes its aliases lowercase), an
/// official-form ARN whose alias is outside the catalog, and a custom-form ARN nothing was
/// imported under.
fn unresolvable_lens_names() -> Vec<(&'static str, String)> {
    vec![
        ("bare-word", "not-a-lens".to_string()),
        ("near-miss-of-an-alias", "serverles".to_string()),
        ("alias-in-another-case", "SERVERLESS".to_string()),
        (
            "official-arn-with-an-unknown-alias",
            official_lens_arn("us-east-1", "not-a-lens"),
        ),
        ("custom-arn-nothing-was-imported-under", absent_custom_lens_arn()),
    ]
}

/// The forms a lens whose review the request addresses can take when that review does not
/// exist: a published lens that is simply not under review, under either spelling of its
/// name, and a name that resolves to no lens at all. A lens review exists exactly while the
/// lens is applied, so both are the same refusal.
fn unreviewed_lens_names() -> Vec<(&'static str, String)> {
    vec![
        ("published-lens-not-under-review", UNREVIEWED_LENS.to_string()),
        (
            "published-lens-not-under-review-by-arn",
            official_lens_arn("us-east-1", UNREVIEWED_LENS),
        ),
        ("name-of-no-lens-at-all", "not-a-lens".to_string()),
        ("custom-arn-nothing-was-imported-under", absent_custom_lens_arn()),
    ]
}

async fn workload_reviewing(client: &Client, name: &str, lenses: &[&str]) -> String {
    let mut body = minimal_create(name, &format!("tok-{name}"));
    body["Lenses"] = json!(lenses);
    created_id(&client.create(body).await, name)
}

async fn associate_lenses(client: &Client, workload_id: &str, lenses: &[String]) -> Answer {
    client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/associateLenses"),
            Some(json!({ "LensAliases": lenses })),
        )
        .await
}

async fn disassociate_lenses(client: &Client, workload_id: &str, lenses: &[String]) -> Answer {
    client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/disassociateLenses"),
            Some(json!({ "LensAliases": lenses })),
        )
        .await
}

fn lens_review_path(workload_id: &str, lens: &str) -> String {
    format!(
        "/workloads/{workload_id}/lensReviews/{}",
        encode_path_segment(lens)
    )
}

/// Every operation that addresses one lens review OF A WORKLOAD, each asked for the review
/// of `lens`. The requests that WRITE are here too: the refusal is decided before anything
/// is written, which the round-trip at the end of each test re-reads.
async fn workload_lens_review_requests(
    client: &Client,
    workload_id: &str,
    lens: &str,
) -> Vec<(&'static str, Answer)> {
    let review = lens_review_path(workload_id, lens);
    vec![
        (
            "GetLensReview",
            client.send("GET", &review, None).await,
        ),
        (
            "UpdateLensReview",
            client
                .send("PATCH", &review, Some(json!({ "LensNotes": "a note" })))
                .await,
        ),
        (
            "UpgradeLensReview",
            client
                .send(
                    "PUT",
                    &format!("{review}/upgrade"),
                    Some(json!({ "MilestoneName": "a milestone of nothing" })),
                )
                .await,
        ),
        (
            "ListAnswers",
            client.send("GET", &format!("{review}/answers"), None).await,
        ),
        (
            "GetAnswer",
            client
                .send("GET", &format!("{review}/answers/{FRAMEWORK_QUESTION}"), None)
                .await,
        ),
        (
            "UpdateAnswer",
            client
                .send(
                    "PATCH",
                    &format!("{review}/answers/{FRAMEWORK_QUESTION}"),
                    Some(json!({ "Notes": "nothing to record" })),
                )
                .await,
        ),
        (
            "ListLensReviewImprovements",
            client
                .send("GET", &format!("{review}/improvements"), None)
                .await,
        ),
        (
            "DisassociateLenses",
            disassociate_lenses(client, workload_id, &[lens.to_string()]).await,
        ),
    ]
}

/// Every operation that addresses one lens review OF A REVIEW TEMPLATE, each asked for the
/// review of `lens`.
async fn template_lens_review_requests(
    client: &Client,
    template_arn: &str,
    lens: &str,
) -> Vec<(&'static str, Answer)> {
    let review = format!(
        "/reviewTemplates/{}/lensReviews/{}",
        encode_path_segment(template_arn),
        encode_path_segment(lens)
    );
    vec![
        (
            "GetReviewTemplateLensReview",
            client.get_review_template_lens_review(template_arn, lens).await,
        ),
        (
            "UpdateReviewTemplateLensReview",
            client
                .update_review_template_lens_review(
                    template_arn,
                    lens,
                    json!({ "LensNotes": "a note" }),
                )
                .await,
        ),
        (
            "UpgradeReviewTemplateLensReview",
            client
                .send("PUT", &format!("{review}/upgrade"), Some(json!({})))
                .await,
        ),
        (
            "ListReviewTemplateAnswers",
            client.list_review_template_answers(template_arn, lens, "").await,
        ),
        (
            "GetReviewTemplateAnswer",
            client
                .get_review_template_answer(template_arn, lens, FRAMEWORK_QUESTION)
                .await,
        ),
        (
            "UpdateReviewTemplateAnswer",
            client
                .update_review_template_answer(
                    template_arn,
                    lens,
                    FRAMEWORK_QUESTION,
                    json!({ "Notes": "nothing to record" }),
                )
                .await,
        ),
        (
            "UpdateReviewTemplate",
            client
                .update_review_template(
                    template_arn,
                    json!({ "LensesToDisassociate": [lens] }),
                )
                .await,
        ),
    ]
}

/// A lens a request asks to APPLY that resolves to no lens is the modelled 400 naming the
/// lens the service could not get — on every operation that applies lenses, for every form
/// the name can take, and whatever else the list carries.
#[tokio::test]
async fn a_lens_that_resolves_to_nothing_cannot_be_applied() {
    let client = Client::new();
    let workload_id = workload_reviewing(&client, "apply-host", &[FRAMEWORK_LENS]).await;
    let template_arn = review_template(&client, "apply-template", &[FRAMEWORK_LENS]).await;

    for (form, name) in unresolvable_lens_names() {
        // The entry is sent alone and beside a lens that DOES resolve, so the refusal
        // depends neither on the entry's position in the list nor on the list's length.
        for (position, lenses) in [
            ("alone", vec![name.clone()]),
            (
                "after-a-resolvable-lens",
                vec![UNREVIEWED_LENS.to_string(), name.clone()],
            ),
        ] {
            let mut created = minimal_create(
                &format!("wl-{form}-{position}"),
                &format!("tok-wl-{form}-{position}"),
            );
            created["Lenses"] = json!(lenses);
            let mut template = minimal_create(
                &format!("tmpl-{form}-{position}"),
                &format!("tok-tmpl-{form}-{position}"),
            );
            template["Lenses"] = json!(lenses);
            template["TemplateName"] = json!(format!("tmpl-{form}-{position}"));
            template["Description"] = json!("a review template under test");

            for (operation, answer) in [
                (
                    "AssociateLenses",
                    associate_lenses(&client, &workload_id, &lenses).await,
                ),
                ("CreateWorkload", client.create(created).await),
                (
                    "CreateReviewTemplate",
                    client.create_review_template(template).await,
                ),
                (
                    "UpdateReviewTemplate",
                    client
                        .update_review_template(
                            &template_arn,
                            json!({ "LensesToAssociate": lenses }),
                        )
                        .await,
                ),
            ] {
                assert_unresolvable_lens(&format!("{form}/{position}/{operation}"), &answer, &name);
            }
        }
    }

    // Nothing was applied: the workload and the template still review exactly the lens they
    // were created with.
    for (case, lenses) in [
        (
            "workload",
            client
                .get(&workload_id)
                .await
                .body
                .get("Workload")
                .and_then(|workload| workload.get("Lenses"))
                .cloned(),
        ),
        (
            "template",
            client
                .get_review_template(&template_arn)
                .await
                .body
                .get("ReviewTemplate")
                .and_then(|template| template.get("Lenses"))
                .cloned(),
        ),
    ] {
        assert_eq!(
            lenses,
            Some(json!([FRAMEWORK_LENS])),
            "case {case:?}: a refused application must record no lens, got {lenses:?}"
        );
    }
}

/// A lens whose review of a WORKLOAD does not exist is the modelled 400 naming `LensAlias`
/// — on every operation that addresses one, for every form the name can take.
#[tokio::test]
async fn a_lens_review_a_workload_does_not_hold_is_refused_naming_the_lens() {
    let client = Client::new();
    let workload_id = workload_reviewing(&client, "review-host", &[FRAMEWORK_LENS]).await;

    for (form, lens) in unreviewed_lens_names() {
        let sentence =
            format!("No lens review for lens alias {lens} was found in workload {workload_id}.");
        for (operation, answer) in workload_lens_review_requests(&client, &workload_id, &lens).await
        {
            assert_field_validation(
                &format!("{form}/{operation}"),
                &answer,
                "LensAlias",
                &sentence,
            );
        }
    }

    // The review the workload DOES hold still resolves, so every refusal above is the value
    // it names rather than a broken read.
    let present = client
        .send("GET", &lens_review_path(&workload_id, FRAMEWORK_LENS), None)
        .await;
    assert_eq!(
        present.status, 200,
        "case \"present\": the lens the workload reviews must still resolve, got {present:?}"
    );
}

/// The same rule on the other resource that holds lens reviews: a lens whose review of a
/// REVIEW TEMPLATE does not exist is the modelled 400 naming `LensAlias`.
#[tokio::test]
async fn a_lens_review_a_template_does_not_hold_is_refused_naming_the_lens() {
    let client = Client::new();
    let template_arn = review_template(&client, "review-template", &[FRAMEWORK_LENS]).await;

    for (form, lens) in unreviewed_lens_names() {
        let sentence = format!(
            "No lens review for lens alias {lens} was found in review template {template_arn}."
        );
        for (operation, answer) in
            template_lens_review_requests(&client, &template_arn, &lens).await
        {
            assert_field_validation(
                &format!("{form}/{operation}"),
                &answer,
                "LensAlias",
                &sentence,
            );
        }
    }

    let present = client
        .get_review_template_lens_review(&template_arn, FRAMEWORK_LENS)
        .await;
    assert_eq!(
        present.status, 200,
        "case \"present\": the lens the template applies must still resolve, got {present:?}"
    );
}

/// A `QuestionId` the lens under review does not ask is the modelled 400 naming
/// `QuestionId`, on every operation that resolves one — of a workload's review and of a
/// template's, which judge it against the same lens content.
#[tokio::test]
async fn a_question_the_lens_does_not_ask_is_refused_naming_the_question() {
    let client = Client::new();
    let workload_id = workload_reviewing(&client, "question-host", &[FRAMEWORK_LENS]).await;
    let template_arn = review_template(&client, "question-template", &[FRAMEWORK_LENS]).await;

    // The forms a `QuestionId` that is no question of the lens can take: a bare word, the
    // right question in another case (a `QuestionId` is matched exactly), a question id past
    // the lens's own numbering, and a question of ANOTHER pillar's numbering that this lens
    // does not ask.
    let unasked: [(&str, &str); 4] = [
        ("bare-word", "not-a-question"),
        ("right-question-in-another-case", "OPS-01"),
        ("past-the-lenss-numbering", "ops-99"),
        ("question-of-no-pillar-of-this-lens", "governance_q0"),
    ];

    for (form, question_id) in unasked {
        let review = lens_review_path(&workload_id, FRAMEWORK_LENS);
        let sentence = format!(
            "No question with ID {question_id} was found in the lens with alias {FRAMEWORK_LENS}."
        );
        for (operation, answer) in [
            (
                "GetAnswer",
                client
                    .send("GET", &format!("{review}/answers/{question_id}"), None)
                    .await,
            ),
            (
                "UpdateAnswer",
                client
                    .send(
                        "PATCH",
                        &format!("{review}/answers/{question_id}"),
                        Some(json!({ "Notes": "nothing to record" })),
                    )
                    .await,
            ),
            (
                "GetReviewTemplateAnswer",
                client
                    .get_review_template_answer(&template_arn, FRAMEWORK_LENS, question_id)
                    .await,
            ),
            (
                "UpdateReviewTemplateAnswer",
                client
                    .update_review_template_answer(
                        &template_arn,
                        FRAMEWORK_LENS,
                        question_id,
                        json!({ "Notes": "nothing to record" }),
                    )
                    .await,
            ),
        ] {
            assert_field_validation(
                &format!("{form}/{operation}"),
                &answer,
                "QuestionId",
                &sentence,
            );
        }
    }

    // A question the lens DOES ask still resolves through both reads.
    let present = client
        .send(
            "GET",
            &format!(
                "{}/answers/{FRAMEWORK_QUESTION}",
                lens_review_path(&workload_id, FRAMEWORK_LENS)
            ),
            None,
        )
        .await;
    assert_eq!(
        present.status, 200,
        "case \"present\": a question the lens asks must still resolve, got {present:?}"
    );
}

/// A `PillarId` the lens under review does not define is the modelled 400 naming
/// `PillarId`, on every operation that takes one — rather than an empty page, which a
/// client cannot tell from a rejected filter. The value is MATCHED against the lens's
/// pillar set rather than naming a resource, so the finding states the rule and hands back
/// the pillars that lens defines; the whole table of lenses, forms and operations the rule
/// covers is `super::pillar_filter_refusal`.
#[tokio::test]
async fn a_pillar_the_lens_does_not_define_is_refused_naming_the_pillar() {
    let client = Client::new();
    let workload_id = workload_reviewing(&client, "pillar-host", &[FRAMEWORK_LENS]).await;
    let template_arn = review_template(&client, "pillar-template", &[FRAMEWORK_LENS]).await;

    // The forms a `PillarId` that is no pillar of the lens can take: a bare word, a pillar
    // of the lens in another case, and a word a custom lens document might define but this
    // lens does not. The valid values the finding must hand back are read back over the
    // wire from the lens review itself (`super::pillar_filter_refusal`), so the expectation
    // comes from a READ of the lens content rather than from a list spelled here.
    let valid = reported_pillars(&client, "pillar-host", &workload_id, FRAMEWORK_LENS)
        .await
        .join(", ");
    for (form, pillar_id) in [
        ("bare-word", "notapillar"),
        ("right-pillar-in-another-case", "OPERATIONALEXCELLENCE"),
        ("pillar-of-another-lenss-content", "governance"),
    ] {
        let review = lens_review_path(&workload_id, FRAMEWORK_LENS);
        for (operation, answer) in [
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
                    .list_review_template_answers(
                        &template_arn,
                        FRAMEWORK_LENS,
                        &format!("PillarId={pillar_id}"),
                    )
                    .await,
            ),
        ] {
            assert_fields_failed(
                &format!("{form}/{operation}"),
                &answer,
                "PillarId",
                &format!(
                    "PillarId contains an invalid pillar. Valid pillars are: [{valid}]."
                ),
            );
        }
    }

    // A pillar the lens DOES define still selects that pillar's questions.
    let present = client
        .send(
            "GET",
            &format!(
                "{}/answers?PillarId={FRAMEWORK_PILLAR}",
                lens_review_path(&workload_id, FRAMEWORK_LENS)
            ),
            None,
        )
        .await;
    assert_eq!(
        present.status, 200,
        "case \"present\": a pillar the lens defines must still select, got {present:?}"
    );
}

/// Every `ChoiceId` of the question `question_id` of the framework lens, read back over the
/// wire from the answer's own choice catalog — the only place a client discovers them.
async fn offered_choices(client: &Client, workload_id: &str, question_id: &str) -> Vec<String> {
    let answer = client
        .send(
            "GET",
            &format!(
                "{}/answers/{question_id}",
                lens_review_path(workload_id, FRAMEWORK_LENS)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {question_id:?}: GetAnswer must report the question's choices, got {answer:?}"
    );
    answer
        .body
        .get("Answer")
        .and_then(|value| value.get("Choices"))
        .and_then(Value::as_array)
        .map(|choices| {
            choices
                .iter()
                .filter_map(|choice| choice.get("ChoiceId"))
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A `ChoiceId` the question does not offer is the modelled 400 whose finding names the
/// member that carried it and hands back that question's OWN choice catalog as the valid
/// values — through either member that carries a choice, and on both operations that record
/// an answer.
#[tokio::test]
async fn a_choice_the_question_does_not_offer_is_refused_with_the_questions_catalog() {
    let client = Client::new();
    let workload_id = workload_reviewing(&client, "choice-host", &[FRAMEWORK_LENS]).await;
    let template_arn = review_template(&client, "choice-template", &[FRAMEWORK_LENS]).await;

    let offered = offered_choices(&client, &workload_id, FRAMEWORK_QUESTION).await;
    assert!(
        offered.len() > 1,
        "case \"catalog\": the question under test must offer several choices, got {offered:?}"
    );
    let valid = offered.join(", ");
    // The forms a `ChoiceId` that is no choice of the question can take: a word no question
    // offers, a choice of ANOTHER question of the same lens, and a choice of this question
    // in another case.
    let other_question = offered_choices(&client, &workload_id, "region-selection").await;
    let undefined: [(&str, String); 3] = [
        ("word-no-question-offers", "not-a-choice".to_string()),
        (
            "choice-of-another-question",
            other_question
                .first()
                .cloned()
                .expect("the other question must offer a choice"),
        ),
        (
            "right-choice-in-another-case",
            offered[0].to_uppercase(),
        ),
    ];

    for (form, choice_id) in undefined {
        for (member_name, body) in [
            (
                "SelectedChoices",
                json!({ "SelectedChoices": [offered[0].clone(), choice_id.clone()] }),
            ),
            (
                "ChoiceUpdates",
                json!({ "ChoiceUpdates": { choice_id.clone(): { "Status": "SELECTED" } } }),
            ),
        ] {
            let sentence = format!(
                "{member_name} contains invalid choices. Valid choices are: [{valid}]."
            );
            for (operation, answer) in [
                (
                    "UpdateAnswer",
                    client
                        .send(
                            "PATCH",
                            &format!(
                                "{}/answers/{FRAMEWORK_QUESTION}",
                                lens_review_path(&workload_id, FRAMEWORK_LENS)
                            ),
                            Some(body.clone()),
                        )
                        .await,
                ),
                (
                    "UpdateReviewTemplateAnswer",
                    client
                        .update_review_template_answer(
                            &template_arn,
                            FRAMEWORK_LENS,
                            FRAMEWORK_QUESTION,
                            body.clone(),
                        )
                        .await,
                ),
            ] {
                let case = format!("{form}/{member_name}/{operation}");
                // The findings are all in `Fields`, so `Message` states the class of the
                // failure and carries no variable part — the one envelope assertion this
                // crate shares for that class.
                assert_fields_failed(&case, &answer, member_name, &sentence);
            }
        }
    }

    // Nothing was recorded: the question the refused requests named is still unanswered.
    let read = client
        .send(
            "GET",
            &format!(
                "{}/answers/{FRAMEWORK_QUESTION}",
                lens_review_path(&workload_id, FRAMEWORK_LENS)
            ),
            None,
        )
        .await;
    assert_eq!(
        read.body
            .get("Answer")
            .and_then(|answer| answer.get("Risk"))
            .and_then(Value::as_str),
        Some("UNANSWERED"),
        "case \"after-the-refusals\": a refused update records nothing, got {read:?}"
    );
}

/// The other half of the rule: the resources this API IDENTIFIES still answer `404
/// ResourceNotFoundException`, so the 400s above are about the request's values and not a
/// blanket change of this service's lookup answer.
#[tokio::test]
async fn the_resources_a_request_identifies_are_still_not_found() {
    let client = Client::new();
    let workload_id = workload_reviewing(&client, "not-found-host", &[FRAMEWORK_LENS]).await;
    let absent_template = absent_template_arn();

    // (case, the request, the modelled ResourceType the 404 must name)
    for (case, answer, resource_type) in [
        (
            "absent-workload",
            client
                .send("GET", &lens_review_path(ABSENT_WORKLOAD_ID, FRAMEWORK_LENS), None)
                .await,
            "WORKLOAD",
        ),
        (
            "absent-workload-with-an-unresolvable-lens",
            client
                .send("GET", &lens_review_path(ABSENT_WORKLOAD_ID, "not-a-lens"), None)
                .await,
            "WORKLOAD",
        ),
        (
            "absent-milestone",
            client
                .send(
                    "GET",
                    &format!(
                        "{}?MilestoneNumber=7",
                        lens_review_path(&workload_id, FRAMEWORK_LENS)
                    ),
                    None,
                )
                .await,
            "MILESTONE",
        ),
        (
            "absent-review-template",
            client
                .get_review_template_lens_review(&absent_template, FRAMEWORK_LENS)
                .await,
            "REVIEW_TEMPLATE",
        ),
        (
            "absent-review-template-with-an-unresolvable-lens",
            client
                .get_review_template_lens_review(&absent_template, "not-a-lens")
                .await,
            "REVIEW_TEMPLATE",
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a resource this API identifies is a 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            member(&answer, "ResourceType"),
            resource_type,
            "case {case:?}: expected ResourceType {resource_type}, got {answer:?}"
        );
    }
}
