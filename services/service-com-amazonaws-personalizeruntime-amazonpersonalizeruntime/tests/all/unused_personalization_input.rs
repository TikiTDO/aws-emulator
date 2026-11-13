// SPDX-License-Identifier: CC0-1.0

//! Which request members may move a `GetRecommendations` response at all.
//!
//! "USER_PERSONALIZATION - `userId` required, `itemId` not used RELATED_ITEMS -
//! `itemId` required, `userId` not used" (`GetRecommendations` doc): a resolved
//! resource reads exactly the ONE personalization input it needs, so the other one is
//! ignored — sending it, or changing it, leaves the returned items, their order and
//! their scores untouched. A resource whose domain use case needs neither input is not
//! personalized, so it answers every caller the same way.
//!
//! The rule is asserted over a TABLE: every registered resource kind (a
//! `USER_PERSONALIZATION` campaign, a `RELATED_ITEMS` campaign and one recommender for
//! each entry of [`RECOMMENDER_USE_CASES`]) crossed with the forms the UNUSED input can
//! take (absent, empty, an id the catalog holds, an id it does not, a different case).
//! Each row is compared against ONE reference: the same request with the unused input
//! left out entirely.
//!
//! The companion suite asserts the rule is not vacuous — the input a resource DOES use
//! moves the response — and that a reported score stays a property of the
//! (resource, personalization input, entry) tuple however many results are asked for.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, RecipeType, Recommender, RequiredInput, RECOMMENDER_USE_CASES,
};

use crate::support::{
    assert_descending_and_bounded, assert_sums_to_one, entries, ids, items, scores,
    scores_by_id, Emulator, RECOMMENDATIONS,
};

/// The `USER_PERSONALIZATION` campaign every user row reads.
const CAMPAIGN_USER: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/unused-input-user";
/// The `RELATED_ITEMS` campaign every item row reads.
const CAMPAIGN_RELATED: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/unused-input-related";

/// How many entries every seeded Items catalog holds.
const CATALOG_SIZE: usize = 40;

/// Two responses may not report an entry's certainty differently at all, so scores are
/// compared up to double-rounding noise only.
const TOLERANCE: f64 = 1e-12;

/// One resolved resource a row recommends from: how it reads in a failure message, the
/// request member naming it, its ARN, and the personalization input it USES.
struct Resource {
    case: String,
    member: &'static str,
    arn: String,
    uses: RequiredInput,
}

/// An emulator holding a campaign of each `GetRecommendations` recipe type and one
/// recommender per domain use case, all over the same seeded Items catalog.
fn seeded() -> (Emulator, Vec<Resource>) {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    let campaigns = [
        (
            "USER_PERSONALIZATION campaign",
            CAMPAIGN_USER,
            RecipeType::UserPersonalization,
            RequiredInput::User,
        ),
        (
            "RELATED_ITEMS campaign",
            CAMPAIGN_RELATED,
            RecipeType::RelatedItems,
            RequiredInput::Item,
        ),
    ];
    let mut resources: Vec<Resource> = Vec::new();
    for (case, arn, recipe_type, uses) in campaigns {
        assert!(
            registry::register_campaign(
                engine,
                Campaign::new(arn, "unused-input", recipe_type, items(CATALOG_SIZE)),
            ),
            "case {case}: the campaign seam must register the campaign"
        );
        resources.push(Resource {
            case: case.to_string(),
            member: "campaignArn",
            arn: arn.to_string(),
            uses,
        });
    }

    // Every entry of the use-case catalog, not only the ones the issue cited: each use
    // case needs its own input, so each decides on its own which member is unused.
    for (index, (use_case, uses)) in RECOMMENDER_USE_CASES.iter().enumerate() {
        let arn = format!(
            "arn:aws:personalize:us-east-1:000000000000:recommender/unused-input-{index}"
        );
        assert!(
            registry::register_recommender(
                engine,
                Recommender::new(&arn, "unused-input", *use_case, items(CATALOG_SIZE)),
            ),
            "case {use_case}: the recommender seam must register the recommender"
        );
        resources.push(Resource {
            case: format!("recommender {use_case:?}"),
            member: "recommenderArn",
            arn,
            uses: *uses,
        });
    }

    (emulator, resources)
}

/// A `GetRecommendations` body naming `resource`, with the personalization members the
/// row carries.
fn request(
    resource: &Resource,
    user_id: Option<&str>,
    item_id: Option<&str>,
    num_results: usize,
) -> Value {
    let mut body = json!({ "numResults": num_results });
    body[resource.member] = json!(resource.arn);
    if let Some(user_id) = user_id {
        body["userId"] = json!(user_id);
    }
    if let Some(item_id) = item_id {
        body["itemId"] = json!(item_id);
    }
    body
}

/// The value of the input `resource` uses — the one it needs in the request.
fn used_input(resource: &Resource) -> (Option<&'static str>, Option<&'static str>) {
    match resource.uses {
        RequiredInput::User => (Some("user-7"), None),
        RequiredInput::Item => (None, Some("item-5")),
        RequiredInput::Nothing => (None, None),
    }
}

/// The forms an unused `userId` can take: absent, empty (no id at all on the wire),
/// ids of several shapes, and an id differing from another row's only in case.
const UNUSED_USER_IDS: [Option<&str>; 6] = [
    None,
    Some(""),
    Some("user-1"),
    Some("user-2"),
    Some("USER-1"),
    Some("a-user-that-never-interacted"),
];

/// The forms an unused `itemId` can take, including ids the catalog holds (the first
/// entry, a middle one, the last one) and one it never held.
const UNUSED_ITEM_IDS: [Option<&str>; 6] = [
    None,
    Some(""),
    Some("item-0"),
    Some("item-3"),
    Some("item-39"),
    Some("no-such-item"),
];

#[tokio::test]
async fn an_input_the_resolved_resource_does_not_use_is_ignored() {
    let (emulator, resources) = seeded();
    let num_results = 10;

    for resource in &resources {
        let (used_user, used_item) = used_input(resource);
        let reference = emulator
            .ok(
                RECOMMENDATIONS,
                request(resource, used_user, used_item, num_results),
            )
            .await;
        let expected = entries(&reference, "itemList");
        assert_eq!(
            expected.len(),
            num_results,
            "case {}: the reference must report {num_results} items, got {reference}",
            resource.case
        );
        assert_descending_and_bounded(&resource.case, &scores(&reference, "itemList"));

        // The unused member is whichever one the resource does not use; a resource that
        // uses neither has both unused, so both vary.
        let only_used_user = [used_user];
        let only_used_item = [used_item];
        let user_rows: &[Option<&str>] = if resource.uses == RequiredInput::User {
            &only_used_user
        } else {
            &UNUSED_USER_IDS
        };
        let item_rows: &[Option<&str>] = if resource.uses == RequiredInput::Item {
            &only_used_item
        } else {
            &UNUSED_ITEM_IDS
        };

        for user_id in user_rows {
            for item_id in item_rows {
                let case = format!(
                    "{} with userId {user_id:?} and itemId {item_id:?}",
                    resource.case
                );
                let response = emulator
                    .ok(
                        RECOMMENDATIONS,
                        request(resource, *user_id, *item_id, num_results),
                    )
                    .await;
                assert_eq!(
                    entries(&response, "itemList"),
                    expected,
                    "case {case}: the input this resource does not use must not change the \
                     response, got {response}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_input_the_resolved_resource_uses_moves_the_response() {
    let (emulator, resources) = seeded();
    let num_results = CATALOG_SIZE;

    // The rule above would hold trivially if nothing personalized the response, so the
    // input each resource DOES use must move it. A use case that needs neither input is
    // not personalized at all, and is asserted to answer every caller the same way.
    for resource in &resources {
        match resource.uses {
            RequiredInput::User => {
                let first = emulator
                    .ok(
                        RECOMMENDATIONS,
                        request(resource, Some("user-1"), None, num_results),
                    )
                    .await;
                let second = emulator
                    .ok(
                        RECOMMENDATIONS,
                        request(resource, Some("user-2"), None, num_results),
                    )
                    .await;
                assert_ne!(
                    ids(&first, "itemList", "itemId"),
                    ids(&second, "itemList", "itemId"),
                    "case {}: userId is the input this resource uses, so two users must not \
                     be ranked identically",
                    resource.case
                );
            }
            RequiredInput::Item => {
                let first = emulator
                    .ok(
                        RECOMMENDATIONS,
                        request(resource, None, Some("item-3"), num_results),
                    )
                    .await;
                let second = emulator
                    .ok(
                        RECOMMENDATIONS,
                        request(resource, None, Some("item-4"), num_results),
                    )
                    .await;
                assert_ne!(
                    ids(&first, "itemList", "itemId"),
                    ids(&second, "itemList", "itemId"),
                    "case {}: itemId is the input this resource uses, so two queried items \
                     must not return the same related items",
                    resource.case
                );
            }
            RequiredInput::Nothing => {
                let first = emulator
                    .ok(
                        RECOMMENDATIONS,
                        request(resource, Some("user-1"), None, num_results),
                    )
                    .await;
                let second = emulator
                    .ok(
                        RECOMMENDATIONS,
                        request(resource, Some("user-2"), Some("item-3"), num_results),
                    )
                    .await;
                assert_eq!(
                    entries(&first, "itemList"),
                    entries(&second, "itemList"),
                    "case {}: this use case needs no personalization input, so it must \
                     answer every caller the same way",
                    resource.case
                );
            }
        }
    }
}

#[tokio::test]
async fn a_reported_score_does_not_change_with_num_results() {
    let (emulator, resources) = seeded();

    // `PredictedItem$score` is "a numeric representation of the model's certainty that
    // the item will be the next user selection": the certainty for one (resource,
    // personalization input, item) tuple. `numResults` only says how many of the scored
    // catalog to report, so it may not move the number reported for an item, and a
    // response carrying part of the catalog carries part of the certainty.
    for resource in &resources {
        let (used_user, used_item) = used_input(resource);
        // A RELATED_ITEMS resource never recommends the queried item, so its scored
        // candidate set is the catalog minus that one entry.
        let candidates = if resource.uses == RequiredInput::Item {
            CATALOG_SIZE - 1
        } else {
            CATALOG_SIZE
        };

        let whole = emulator
            .ok(
                RECOMMENDATIONS,
                request(resource, used_user, used_item, CATALOG_SIZE),
            )
            .await;
        let reference = scores_by_id(&whole, "itemList", "itemId");
        assert_eq!(
            reference.len(),
            candidates,
            "case {}: expected {candidates} scored candidates, got {reference:?}",
            resource.case
        );
        // Reporting every scored candidate reports the whole certainty.
        assert_sums_to_one(&resource.case, &scores(&whole, "itemList"));

        for num_results in [1_usize, 2, 5, 10, 25, candidates] {
            let case = format!("{} numResults {num_results}", resource.case);
            let response = emulator
                .ok(
                    RECOMMENDATIONS,
                    request(resource, used_user, used_item, num_results),
                )
                .await;
            let reported = scores_by_id(&response, "itemList", "itemId");
            assert_eq!(
                reported.len(),
                num_results.min(candidates),
                "case {case}: expected {} items, got {reported:?}",
                num_results.min(candidates)
            );
            for (item_id, score) in &reported {
                let expected = reference.get(item_id).unwrap_or_else(|| {
                    panic!("case {case}: {item_id} is not a scored candidate of {reference:?}")
                });
                assert!(
                    (score - expected).abs() < TOLERANCE,
                    "case {case}: {item_id} scores {expected} over the whole scored catalog, \
                     got {score}"
                );
            }
            assert_descending_and_bounded(&case, &scores(&response, "itemList"));

            // A response that reports fewer entries than the catalog scored carries
            // strictly less than the whole certainty — asking for one result never
            // reports a certainty of 1.0.
            let total: f64 = reported.values().sum();
            if num_results < candidates {
                assert!(
                    total < 1.0 - 1e-9,
                    "case {case}: {num_results} of {candidates} scored candidates must carry \
                     part of the certainty, got {total}"
                );
            }
        }
    }
}
