// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DisassociateLenses`.
//!
//! The rule these pin, over a TABLE of requests rather than one recorded call:
//! "Disassociate a lens from a workload" (`smithy_query(query="doc",
//! shape="DisassociateLenses")`) drops, from the workload the request names, the review of
//! every DISTINCT lens its `LensAliases` name — a lens is one lens however it is spelled,
//! so an alias and that lens's ARN name the same review — along with the answers and notes
//! recorded under that review, and reports the change through `Workload$UpdatedAt`. Both
//! identifiers are resolved before anything is removed, so a workload that is not stored is
//! a modelled 404 over the `WORKLOAD` resource type and a lens the workload does not review
//! is the modelled 400 naming `LensAlias`, each removing nothing. Two refusals bound
//! it: "Up to 10 lenses can be disassociated from a workload in a single API operation"
//! bounds ONE request, checked before any per-entry work since the member publishes no
//! modelled maximum, and "The Amazon Web Services Well-Architected Framework lens
//! (`wellarchitected`) cannot be removed from a workload" refuses a request naming it —
//! each answered with the modelled 400 `ValidationException`, because the operation models
//! no other error for a request the service will not serve; the quota names the member that
//! carried the aliases and the default lens names the workload's own `Lenses`.
//!
//! Every workload and custom lens is minted over HTTP and everything asserted crosses the
//! HTTP boundary: requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production.

use std::time::Duration;

use serde_json::{json, Value};

use super::custom_lens_import::{
    import, imported_arn, lens_document, wait_for_the_import_to_land, ABSENT_LENS_ARN,
};
use super::custom_lens_versions::create_version;
use super::lens_resolution::field_names;
use super::workload_harness::{
    age_workload, assert_field_validation, assert_not_found, created_id, encode_path_segment,
    member, minimal_create, official_lens_arn, Answer, Client,
};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

/// The modelled `ResourceType` of a workload (`state::WORKLOAD_RESOURCE_TYPE`), as the wire
/// carries it on a failure about one.
const WORKLOAD_TYPE: &str = "WORKLOAD";

/// How far back a workload's `UpdatedAt` is moved before a request under test, so a write
/// the handler makes to it is observable whatever the clock's granularity between two
/// in-process requests.
const AGE: Duration = Duration::from_secs(5);

/// A well-formed `WorkloadId` (32 lowercase hex, the modelled `^[0-9a-f]{32}$`) that no
/// workload carries.
const ABSENT_HEX_ID: &str = "0123456789abcdef0123456789abcdef";

/// `DisassociateLenses`: `PATCH /workloads/{WorkloadId}/disassociateLenses`.
async fn disassociate_lenses(client: &Client, workload_id: &str, lenses: &[String]) -> Answer {
    client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/disassociateLenses"),
            Some(json!({ "LensAliases": lenses })),
        )
        .await
}

/// `AssociateLenses`: `PATCH /workloads/{WorkloadId}/associateLenses` — how a workload comes
/// to review more lenses than one create can attach, and how a dropped review is asked for
/// again.
async fn associate_lenses(client: &Client, workload_id: &str, lenses: &[String]) -> Answer {
    client
        .send(
            "PATCH",
            &format!("/workloads/{workload_id}/associateLenses"),
            Some(json!({ "LensAliases": lenses })),
        )
        .await
}

/// A workload created over HTTP reviewing `lenses`, and nothing else.
async fn created_workload(client: &Client, name: &str, lenses: &[String]) -> String {
    let mut request = minimal_create(name, &format!("tok-{name}"));
    request["Lenses"] = json!(lenses);
    created_id(&client.create(request).await, name)
}

/// The `Workload$Lenses` a stored workload reports through `GetWorkload`, and the
/// `Workload$UpdatedAt` it reports beside them, as the epoch seconds restJson1 carries it.
async fn stored_lenses(client: &Client, workload_id: &str, case: &str) -> (Vec<String>, f64) {
    let answer = client.get(workload_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetWorkload 200, got {answer:?}"
    );
    let workload = answer.body.get("Workload").cloned().unwrap_or(Value::Null);
    let lenses = workload
        .get("Lenses")
        .and_then(Value::as_array)
        .map(|lenses| {
            lenses
                .iter()
                .map(|lens| lens.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    let updated_at = workload
        .get("UpdatedAt")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: the workload must report UpdatedAt"));
    (lenses, updated_at)
}

/// The `LensAlias` of every lens review `ListLensReviews` reports for a workload, in the
/// order it pages over them.
async fn reviewed_lenses(client: &Client, workload_id: &str, case: &str) -> Vec<String> {
    let answer = client
        .send("GET", &format!("/workloads/{workload_id}/lensReviews"), None)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ListLensReviews 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReviewSummaries")
        .and_then(Value::as_array)
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    summary
                        .get("LensAlias")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `GetLensReview`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}`. The alias is
/// percent-encoded because a custom lens is named by its ARN, which carries `:` and `/`
/// inside ONE path segment.
async fn get_lens_review(client: &Client, workload_id: &str, lens: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(lens)
            ),
            None,
        )
        .await
}

/// `UpdateLensReview`: the same path written with `PATCH` — what writes the notes a dropped
/// review must take with it.
async fn update_lens_review(client: &Client, workload_id: &str, lens: &str, body: Value) -> Answer {
    client
        .send(
            "PATCH",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(lens)
            ),
            Some(body),
        )
        .await
}

/// `GetAnswer`: `GET /workloads/{WorkloadId}/lensReviews/{LensAlias}/answers/{QuestionId}`.
async fn get_answer(client: &Client, workload_id: &str, lens: &str, question_id: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}/answers/{}",
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            None,
        )
        .await
}

/// `UpdateAnswer`: the same path written with `PATCH` — what records the answer a dropped
/// review must take with it.
async fn update_answer(
    client: &Client,
    workload_id: &str,
    lens: &str,
    question_id: &str,
    body: Value,
) -> Answer {
    client
        .send(
            "PATCH",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}/answers/{}",
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            Some(body),
        )
        .await
}

/// The aliases of AWS-official lenses other than the framework lens, which is the one lens a
/// disassociation can never name — read off the catalog the service resolves a lens against
/// (`state::AWS_OFFICIAL_LENS_ALIASES`), so a row states a count rather than wire names.
fn removable_official_aliases(count: usize) -> Vec<String> {
    let aliases: Vec<String> = state::AWS_OFFICIAL_LENS_ALIASES
        .iter()
        .filter(|alias| **alias != state::FRAMEWORK_LENS_ALIAS)
        .take(count)
        .map(|alias| (*alias).to_string())
        .collect();
    assert_eq!(
        aliases.len(),
        count,
        "the AWS-official lens catalog must supply {count} removable aliases, got {aliases:?}"
    );
    aliases
}

/// One row of the disassociation table: the lenses the workload is created reviewing, the
/// aliases the request names, and the lens list the workload must then report, in order.
struct DisassociationCase {
    case: &'static str,
    name: &'static str,
    created: Vec<String>,
    disassociated: Vec<String>,
    remaining: Vec<String>,
}

/// `DisassociateLenses` drops the review of every distinct lens the request names, leaves
/// the rest of the workload's reviews as they stand, and reports the change through
/// `UpdatedAt` — and every review it dropped is gone from `GetWorkload`, `ListLensReviews`
/// and `GetLensReview`, while every review it kept is still readable.
#[tokio::test]
async fn disassociate_lenses_drops_the_review_of_every_distinct_lens_named() {
    // The rows differ along what the rule generalises over: how many aliases one request
    // names, which SPELLING names the lens (a bare alias or that lens's official ARN, whose
    // Region is deliberately not the request's) and whether the two spellings agree with the
    // one the association recorded, whether the list repeats a lens, and how many reviews
    // are left behind.
    let rows = vec![
        DisassociationCase {
            case: "one alias",
            name: "lens-disassociate-one",
            created: vec!["wellarchitected".to_string(), "serverless".to_string()],
            disassociated: vec!["serverless".to_string()],
            remaining: vec!["wellarchitected".to_string()],
        },
        DisassociationCase {
            case: "several aliases in one request",
            name: "lens-disassociate-several",
            created: vec![
                "wellarchitected".to_string(),
                "serverless".to_string(),
                "iot".to_string(),
                "sap".to_string(),
            ],
            disassociated: vec!["serverless".to_string(), "sap".to_string()],
            remaining: vec!["wellarchitected".to_string(), "iot".to_string()],
        },
        DisassociationCase {
            case: "a lens attached by alias, removed by its ARN",
            name: "lens-disassociate-by-arn",
            created: vec!["wellarchitected".to_string(), "serverless".to_string()],
            disassociated: vec![official_lens_arn("us-east-1", "serverless")],
            remaining: vec!["wellarchitected".to_string()],
        },
        DisassociationCase {
            case: "a lens attached by its ARN, removed by alias",
            name: "lens-disassociate-by-alias",
            created: vec![
                "wellarchitected".to_string(),
                official_lens_arn("us-east-1", "iot"),
            ],
            disassociated: vec!["iot".to_string()],
            remaining: vec!["wellarchitected".to_string()],
        },
        DisassociationCase {
            case: "the same lens named twice is dropped once",
            name: "lens-disassociate-duplicate",
            created: vec![
                "wellarchitected".to_string(),
                "serverless".to_string(),
                "iot".to_string(),
            ],
            disassociated: vec![
                "serverless".to_string(),
                official_lens_arn("us-west-2", "serverless"),
            ],
            remaining: vec!["wellarchitected".to_string(), "iot".to_string()],
        },
        DisassociationCase {
            case: "every lens but the framework one",
            name: "lens-disassociate-all-removable",
            created: vec![
                "wellarchitected".to_string(),
                "serverless".to_string(),
                "iot".to_string(),
                "genai".to_string(),
            ],
            disassociated: vec![
                "genai".to_string(),
                "serverless".to_string(),
                "iot".to_string(),
            ],
            remaining: vec!["wellarchitected".to_string()],
        },
    ];

    for row in rows {
        let client = Client::new();
        let workload_id = created_workload(&client, row.name, &row.created).await;
        age_workload(&client, &workload_id, AGE);
        let (_, before) = stored_lenses(&client, &workload_id, row.case).await;

        let answer = disassociate_lenses(&client, &workload_id, &row.disassociated).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected DisassociateLenses 200, got {answer:?}",
            row.case
        );

        let (lenses, after) = stored_lenses(&client, &workload_id, row.case).await;
        assert_eq!(
            lenses, row.remaining,
            "case {:?}: the workload must report only the lenses it still reviews, got {lenses:?}",
            row.case
        );
        assert!(
            after > before,
            "case {:?}: disassociating a lens must move UpdatedAt, got {after} after {before}",
            row.case
        );

        let reviews = reviewed_lenses(&client, &workload_id, row.case).await;
        assert_eq!(
            reviews.len(),
            row.remaining.len(),
            "case {:?}: expected {} lens reviews, got {reviews:?}",
            row.case,
            row.remaining.len()
        );

        // Every lens the request named — including one named twice, and one named by a
        // spelling the association did not use — has no review left to read: the read of it
        // is refused as a value naming no lens review of the workload.
        for lens in &row.disassociated {
            let answer = get_lens_review(&client, &workload_id, lens).await;
            assert_eq!(
                answer.status, 400,
                "case {:?}: the review of {lens:?} must be gone, got {answer:?}",
                row.case
            );
        }
        // Every lens left behind is still readable, under the spelling the create recorded.
        for lens in &row.remaining {
            let answer = get_lens_review(&client, &workload_id, lens).await;
            assert_eq!(
                answer.status, 200,
                "case {:?}: the review of {lens:?} must survive, got {answer:?}",
                row.case
            );
        }
    }
}

/// A lens the workload does not review is the modelled 400 naming `LensAlias` and removes
/// nothing — whether that name resolves to a lens at all, since a lens
/// review exists exactly while the lens is associated with the workload. A workload that is
/// not stored is a 404 over the `WORKLOAD` type, resolved first.
#[tokio::test]
async fn a_lens_the_workload_does_not_review_is_refused_and_removes_nothing() {
    // The rows differ along WHAT the unreviewed name is: an official alias of a lens that
    // exists and is not attached, that lens's ARN, a catalog alias in another case (catalog
    // membership is exact — AWS's aliases are published lowercase), a bare word no alias
    // carries, and a custom-form ARN naming no stored lens.
    let unreviewed: Vec<(&str, String)> = vec![
        ("official-alias-not-attached", "machinelearning".to_string()),
        (
            "official-arn-not-attached",
            official_lens_arn("us-east-1", "machinelearning"),
        ),
        ("alias-in-another-case", "SERVERLESS".to_string()),
        ("bare-word", "not-a-lens".to_string()),
        ("custom-arn-of-no-stored-lens", ABSENT_LENS_ARN.to_string()),
    ];

    let client = Client::new();
    let workload_id = created_workload(
        &client,
        "lens-disassociate-unreviewed",
        &[
            "wellarchitected".to_string(),
            "serverless".to_string(),
            "iot".to_string(),
        ],
    )
    .await;

    for (name, entry) in &unreviewed {
        // Each entry is sent alone, after a lens the workload DOES review, and before ones
        // it does — so the refusal depends neither on the entry's POSITION in the list nor
        // on the list's length.
        for (position, lenses) in [
            ("alone", vec![entry.clone()]),
            (
                "after-a-reviewed-lens",
                vec!["serverless".to_string(), entry.clone()],
            ),
            (
                "before-reviewed-lenses",
                vec![entry.clone(), "serverless".to_string(), "iot".to_string()],
            ),
        ] {
            let case = format!("{name}-{position}");
            age_workload(&client, &workload_id, AGE);
            let (before, updated_before) = stored_lenses(&client, &workload_id, &case).await;

            let answer = disassociate_lenses(&client, &workload_id, &lenses).await;
            assert_field_validation(
                &case,
                &answer,
                "LensAlias",
                &format!(
                    "No lens review for lens alias {entry} was found in workload {workload_id}."
                ),
            );

            let (after, updated_after) = stored_lenses(&client, &workload_id, &case).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused disassociation must drop no lens, got {after:?}"
            );
            assert_eq!(
                updated_after, updated_before,
                "case {case:?}: a refused disassociation must not move UpdatedAt, got \
                 {updated_after} after {updated_before}"
            );
        }
    }

    // A workload that is not stored is resolved FIRST, so a caller whose workload is gone is
    // not told its lens review is — even when the request also names a lens no workload
    // reviews.
    for (case, lenses) in [
        ("reviewable-lens", vec!["serverless".to_string()]),
        ("unknown-lens-too", vec!["not-a-lens".to_string()]),
    ] {
        let answer = disassociate_lenses(&client, ABSENT_HEX_ID, &lenses).await;
        assert_not_found(
            case,
            &answer,
            WORKLOAD_TYPE,
            ABSENT_HEX_ID,
            &format!("workload with ID {ABSENT_HEX_ID}"),
        );
    }
}

/// "The Amazon Web Services Well-Architected Framework lens (`wellarchitected`) cannot be
/// removed from a workload" (`DisassociateLenses` doc): a request naming it is the modelled
/// 400 naming the workload's `Lenses` and removes nothing, however the name is spelled,
/// wherever it sits in the list, and whether or not the workload reviews that lens.
#[tokio::test]
async fn the_framework_lens_cannot_be_removed_from_a_workload() {
    let client = Client::new();
    let reviewing_framework = created_workload(
        &client,
        "lens-disassociate-framework",
        &[
            "wellarchitected".to_string(),
            "serverless".to_string(),
            "iot".to_string(),
        ],
    )
    .await;
    // A workload the framework lens was never applied to: the lens is unremovable there
    // too, so the refusal is about the LENS the request names rather than about this
    // workload's reviews.
    let without_framework = created_workload(
        &client,
        "lens-disassociate-no-framework",
        &["serverless".to_string()],
    )
    .await;

    // The rows differ along the spelling that names the framework lens, its position in the
    // list, and whether the rest of the list names lenses the workload does review. The last
    // element is the alias the refusal must quote: the one the request named the default lens
    // by.
    let rows: Vec<(&str, &str, Vec<String>, String)> = vec![
        (
            "bare-alias",
            reviewing_framework.as_str(),
            vec![state::FRAMEWORK_LENS_ALIAS.to_string()],
            state::FRAMEWORK_LENS_ALIAS.to_string(),
        ),
        (
            "official-arn",
            reviewing_framework.as_str(),
            vec![official_lens_arn(
                "us-east-1",
                state::FRAMEWORK_LENS_ALIAS,
            )],
            official_lens_arn("us-east-1", state::FRAMEWORK_LENS_ALIAS),
        ),
        (
            "last-of-a-list",
            reviewing_framework.as_str(),
            vec![
                "serverless".to_string(),
                state::FRAMEWORK_LENS_ALIAS.to_string(),
            ],
            state::FRAMEWORK_LENS_ALIAS.to_string(),
        ),
        (
            "first-of-a-list",
            reviewing_framework.as_str(),
            vec![
                state::FRAMEWORK_LENS_ALIAS.to_string(),
                "iot".to_string(),
            ],
            state::FRAMEWORK_LENS_ALIAS.to_string(),
        ),
        (
            "a-workload-that-does-not-review-it",
            without_framework.as_str(),
            vec![state::FRAMEWORK_LENS_ALIAS.to_string()],
            state::FRAMEWORK_LENS_ALIAS.to_string(),
        ),
        // Any name the crate's one lens identity rule resolves to the framework lens names
        // it here too: `state::lens_alias` reads the alias off an ARN's trailing
        // `lens/<alias>`, and it is the SAME rule the workload's reviews are resolved by, so
        // a name that would otherwise reach the framework lens's review is refused rather
        // than removing it.
        (
            "an-arn-whose-trailing-alias-is-the-framework-lens",
            reviewing_framework.as_str(),
            vec![format!(
                "arn:aws:wellarchitected:us-west-2:999999999999:lens/{}",
                state::FRAMEWORK_LENS_ALIAS
            )],
            format!(
                "arn:aws:wellarchitected:us-west-2:999999999999:lens/{}",
                state::FRAMEWORK_LENS_ALIAS
            ),
        ),
    ];

    for (case, workload_id, lenses, quoted) in rows {
        age_workload(&client, workload_id, AGE);
        let (before, updated_before) = stored_lenses(&client, workload_id, case).await;

        let answer = disassociate_lenses(&client, workload_id, &lenses).await;
        assert_refused(case, &answer, state::LENSES_MEMBER);
        // The sentence states the alias the request named the default lens by, once, in
        // `Message` and in the one `Fields` entry.
        let sentence = format!("Cannot disassociate default lens: {quoted}.");
        assert_eq!(
            member(&answer, "Message"),
            format!("[Validation] {sentence}"),
            "case {case:?}: expected the default-lens sentence, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Fields").cloned().unwrap_or(Value::Null),
            json!([{ "Name": state::LENSES_MEMBER, "Message": sentence }]),
            "case {case:?}: the finding must carry the same sentence, got {answer:?}"
        );

        let (after, updated_after) = stored_lenses(&client, workload_id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused disassociation must drop no lens, got {after:?}"
        );
        assert_eq!(
            updated_after, updated_before,
            "case {case:?}: a refused disassociation must not move UpdatedAt, got \
             {updated_after} after {updated_before}"
        );
    }

    // The framework lens's review is still readable, and so is every other review of the
    // workload the refusals were sent to.
    for lens in ["wellarchitected", "serverless", "iot"] {
        let answer = get_lens_review(&client, &reviewing_framework, lens).await;
        assert_eq!(
            answer.status, 200,
            "the review of {lens:?} must survive every refusal, got {answer:?}"
        );
    }
}

/// One request names at most `state::MAX_LENS_ALIASES_PER_REQUEST` lenses ("Up to 10 lenses
/// can be disassociated from a workload in a single API operation", this operation's doc),
/// which the member itself does not state. Checked below the ceiling and at it, both served,
/// and one past it, which is refused and removes nothing. Each row runs against its OWN
/// workload, so what the row varies is the length of the request and nothing else.
#[tokio::test]
async fn the_lens_aliases_quota_bounds_one_request() {
    const MAX_ALIASES: usize = state::MAX_LENS_ALIASES_PER_REQUEST;

    // The over-the-ceiling row needs MAX_ALIASES + 1 removable lenses attached, so the quota
    // is what it meets rather than the workload's own reviews.
    let removable = removable_official_aliases(MAX_ALIASES + 1);

    for (case, count) in [
        ("one-alias", 1usize),
        ("at-the-ceiling", MAX_ALIASES),
        ("one-past-the-ceiling", MAX_ALIASES + 1),
    ] {
        let client = Client::new();
        // One create attaches at most its own quota, so the framework lens and the first
        // batch arrive with the create and the rest are associated afterwards.
        let mut attached = vec![state::FRAMEWORK_LENS_ALIAS.to_string()];
        attached.extend(removable.iter().cloned());
        let at_create = attached.len().min(state::MAX_LENSES_PER_WORKLOAD);
        let workload_id = created_workload(
            &client,
            match case {
                "one-alias" => "lens-disassociate-quota-one",
                "at-the-ceiling" => "lens-disassociate-quota-max",
                _ => "lens-disassociate-quota-over",
            },
            &attached[..at_create],
        )
        .await;
        for chunk in attached[at_create..].chunks(state::MAX_LENS_ALIASES_PER_REQUEST) {
            let answer = associate_lenses(&client, &workload_id, chunk).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: attaching {chunk:?} must be served, got {answer:?}"
            );
        }

        let requested: Vec<String> = removable.iter().take(count).cloned().collect();
        age_workload(&client, &workload_id, AGE);
        let (before, updated_before) = stored_lenses(&client, &workload_id, case).await;

        let answer = disassociate_lenses(&client, &workload_id, &requested).await;

        if count <= MAX_ALIASES {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: {count} aliases are within the quota, got {answer:?}"
            );
            let (after, _) = stored_lenses(&client, &workload_id, case).await;
            assert_eq!(
                after.len(),
                before.len() - count,
                "case {case:?}: every named lens must be dropped, got {after:?}"
            );
        } else {
            assert_refused(case, &answer, state::LENS_ALIASES_MEMBER);
            let (after, updated_after) = stored_lenses(&client, &workload_id, case).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused disassociation must drop no lens, got {after:?}"
            );
            assert_eq!(
                updated_after, updated_before,
                "case {case:?}: a refused disassociation must not move UpdatedAt, got \
                 {updated_after} after {updated_before}"
            );
        }
    }
}

/// A published custom lens is disassociated by the ARN it is named under, exactly as an
/// AWS-official lens is by its alias: "For custom lenses, this is the lens ARN"
/// (`smithy_query(query="doc", shape="LensAlias")`), so the kind of lens changes nothing
/// about the rule.
#[tokio::test]
async fn a_custom_lens_is_disassociated_by_its_arn() {
    let client = Client::new();
    let workload_id = created_workload(
        &client,
        "lens-disassociate-custom",
        &["wellarchitected".to_string()],
    )
    .await;

    let document = lens_document("Disassociable lens", None, &[("governance", 2, 2)]).to_string();
    let lens_arn = imported_arn(
        &import(&client, &document, None, "tok-import-disassociable", None).await,
        "custom",
    );
    wait_for_the_import_to_land();
    let published = create_version(&client, &lens_arn, "1.0", Some(true), "tok-publish").await;
    assert_eq!(
        published.status, 200,
        "CreateLensVersion must publish the imported lens, got {published:?}"
    );

    let associated = associate_lenses(&client, &workload_id, std::slice::from_ref(&lens_arn)).await;
    assert_eq!(
        associated.status, 200,
        "the published custom lens must be associable, got {associated:?}"
    );
    let review = get_lens_review(&client, &workload_id, &lens_arn).await;
    assert_eq!(
        review.status, 200,
        "the review of the custom lens must be readable before it is dropped, got {review:?}"
    );

    // The ARN with the version pinned after it names the SAME lens, so it drops that one
    // review.
    age_workload(&client, &workload_id, AGE);
    let (_, before) = stored_lenses(&client, &workload_id, "custom").await;
    let answer = disassociate_lenses(&client, &workload_id, &[format!("{lens_arn}/1.0")]).await;
    assert_eq!(
        answer.status, 200,
        "a version-pinned ARN names the same custom lens, got {answer:?}"
    );

    let (lenses, after) = stored_lenses(&client, &workload_id, "custom").await;
    assert_eq!(
        lenses,
        vec!["wellarchitected".to_string()],
        "the custom lens must be gone from the workload, got {lenses:?}"
    );
    assert!(
        after > before,
        "dropping the custom lens must move UpdatedAt, got {after} after {before}"
    );
    let review = get_lens_review(&client, &workload_id, &lens_arn).await;
    assert_field_validation(
        "custom",
        &review,
        "LensAlias",
        &format!("No lens review for lens alias {lens_arn} was found in workload {workload_id}."),
    );

    // The lens itself still exists — only the workload's review of it was dropped — so it
    // can be associated again.
    let again = associate_lenses(&client, &workload_id, std::slice::from_ref(&lens_arn)).await;
    assert_eq!(
        again.status, 200,
        "the custom lens must be associable again, got {again:?}"
    );
}

/// Dropping a lens drops the review's own data with it: the answers recorded against that
/// review and the notes written to it are gone, and a later association of the same lens
/// starts from an unanswered review. A milestone records the workload BY VALUE, so a
/// milestone taken before the disassociation still reports the lens it froze.
#[tokio::test]
async fn disassociating_a_lens_drops_the_answers_and_notes_of_its_review() {
    let client = Client::new();
    let workload_id = created_workload(
        &client,
        "lens-disassociate-answers",
        &["wellarchitected".to_string(), "serverless".to_string()],
    )
    .await;

    // The question and the choice come off the lens content the service answers from
    // (`state::lens_questions`), so this case states a lens and not its wire ids.
    let questions = state::lens_questions("serverless");
    let question = questions
        .first()
        .expect("the serverless lens must ask at least one question");
    let question_id = question.question_id;
    let choice_id = question.choice_id(0);

    let recorded = update_answer(
        &client,
        &workload_id,
        "serverless",
        question_id,
        json!({ "SelectedChoices": [choice_id], "Notes": "an answer under test" }),
    )
    .await;
    assert_eq!(
        recorded.status, 200,
        "UpdateAnswer must record the answer, got {recorded:?}"
    );
    let noted = update_lens_review(
        &client,
        &workload_id,
        "serverless",
        json!({ "LensNotes": "notes under test" }),
    )
    .await;
    assert_eq!(
        noted.status, 200,
        "UpdateLensReview must record the notes, got {noted:?}"
    );

    let read = get_answer(&client, &workload_id, "serverless", question_id).await;
    assert_eq!(
        read.status, 200,
        "the recorded answer must be readable before the lens is dropped, got {read:?}"
    );

    // A milestone freezes the workload as it stands, reviews and answers included.
    let milestone = client
        .send(
            "POST",
            &format!("/workloads/{workload_id}/milestones"),
            Some(json!({
                "MilestoneName": "before the disassociation",
                "ClientRequestToken": "tok-milestone-disassociate",
            })),
        )
        .await;
    assert_eq!(
        milestone.status, 200,
        "CreateMilestone must freeze the workload, got {milestone:?}"
    );
    let milestone_number = milestone
        .body
        .get("MilestoneNumber")
        .and_then(Value::as_i64)
        .expect("CreateMilestone must answer a MilestoneNumber");

    let answer = disassociate_lenses(&client, &workload_id, &["serverless".to_string()]).await;
    assert_eq!(
        answer.status, 200,
        "the lens under test must be disassociated, got {answer:?}"
    );

    let dropped = get_answer(&client, &workload_id, "serverless", question_id).await;
    assert_field_validation(
        "dropped-review",
        &dropped,
        "LensAlias",
        &format!("No lens review for lens alias serverless was found in workload {workload_id}."),
    );

    // The milestone still reports the lens it froze: it holds the workload as it stood.
    let frozen = client
        .send(
            "GET",
            &format!("/workloads/{workload_id}/milestones/{milestone_number}"),
            None,
        )
        .await;
    assert_eq!(
        frozen.status, 200,
        "GetMilestone must report the frozen workload, got {frozen:?}"
    );
    let frozen_lenses: Vec<String> = frozen
        .body
        .get("Milestone")
        .and_then(|milestone| milestone.get("Workload"))
        .and_then(|workload| workload.get("Lenses"))
        .and_then(Value::as_array)
        .map(|lenses| {
            lenses
                .iter()
                .map(|lens| lens.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        frozen_lenses.contains(&"serverless".to_string()),
        "the milestone must still report the lens it froze, got {frozen_lenses:?}"
    );

    // Associating the same lens again starts from an unanswered review: the answers and the
    // notes went with the review that was dropped.
    let again = associate_lenses(&client, &workload_id, &["serverless".to_string()]).await;
    assert_eq!(
        again.status, 200,
        "the lens must be associable again, got {again:?}"
    );
    let reread = get_answer(&client, &workload_id, "serverless", question_id).await;
    assert_eq!(
        reread.status, 200,
        "the new review must answer the question again, got {reread:?}"
    );
    let reported = reread.body.get("Answer").cloned().unwrap_or(Value::Null);
    assert_eq!(
        reported.get("Risk").and_then(Value::as_str),
        Some(state::RISK_UNANSWERED),
        "a question of the new review must be UNANSWERED, got {reread:?}"
    );
    assert!(
        reported
            .get("SelectedChoices")
            .and_then(Value::as_array)
            .is_none_or(|selected| selected.is_empty()),
        "the dropped review's selected choices must not come back, got {reread:?}"
    );
    assert!(
        reported.get("Notes").is_none(),
        "the dropped answer's notes must not come back, got {reread:?}"
    );
    let review = get_lens_review(&client, &workload_id, "serverless").await;
    assert_eq!(
        review.status, 200,
        "the new review must be readable, got {review:?}"
    );
    assert!(
        review
            .body
            .get("LensReview")
            .and_then(|review| review.get("Notes"))
            .is_none(),
        "the dropped review's notes must not come back, got {review:?}"
    );
}

/// The refusal a request the service will not serve gets: the modelled 400
/// `ValidationException` naming, in `member`, the field the rule it broke is about.
/// `DisassociateLenses` models no `ServiceQuotaExceededException` and no error for an
/// unremovable lens, so both are reported as a bad request.
fn assert_refused(case: &str, answer: &Answer, member_name: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_code, "ValidationException",
        "case {case:?}: expected ValidationException, got {answer:?}"
    );
    assert_eq!(
        member(answer, "Reason"),
        "FIELD_VALIDATION_FAILED",
        "case {case:?}: expected Reason FIELD_VALIDATION_FAILED, got {answer:?}"
    );
    assert_eq!(
        field_names(answer),
        vec![member_name.to_string()],
        "case {case:?}: the refusal must name the offending field, got {answer:?}"
    );
}
