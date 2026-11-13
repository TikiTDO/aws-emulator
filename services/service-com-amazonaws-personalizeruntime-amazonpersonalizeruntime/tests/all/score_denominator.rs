// SPDX-License-Identifier: CC0-1.0

//! What a reported `score` is a function of, over the three inference operations.
//!
//! `PredictedItem$score` / `PredictedAction$score` is "a numeric representation of the
//! model's certainty that the item will be the next user selection": the certainty for
//! one (resolved resource, personalization inputs, entry) tuple. So the denominator
//! behind it is the resource's WHOLE scored catalog, and no member of the request that
//! merely selects which entries come back — the ids `inputList` names, the `filterArn`
//! applied, `numResults`, a promotion — may move the number reported for an entry. A
//! response that reports part of the catalog simply carries part of the certainty.
//!
//! Each suite asserts that rule over a TABLE whose rows differ along what the rule
//! generalizes over (how many ids the request names, in what order, how many of them
//! the catalog holds, which filter removes which part of it, how many results are
//! asked back) and compares every reported score against ONE reference: the same
//! request over the whole catalog.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Action, Campaign, Item, RecipeType,
};

use crate::support::{
    actions, assert_descending_and_bounded, assert_sums_to_one, ids, items, scores,
    scores_by_id, Emulator, ACTION_RECOMMENDATIONS, COLUMN_GENRE, COLUMN_YEAR,
    PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// The `PERSONALIZED_RANKING` campaign every ranking row re-ranks against.
const CAMPAIGN_RANKING: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/denominator-ranking";
/// The `USER_PERSONALIZATION` campaign the `GetRecommendations` rows read.
const CAMPAIGN_USER: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/denominator-user";
/// The `PERSONALIZED_ACTIONS` campaign the `GetActionRecommendations` rows read.
const CAMPAIGN_ACTIONS: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/denominator-actions";

/// A `PERSONALIZED_RANKING` campaign whose catalog entries carry NO popularity.
const CAMPAIGN_FLAT_RANKING: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/flat-ranking";
/// A `USER_PERSONALIZATION` campaign whose catalog entries carry no popularity.
const CAMPAIGN_FLAT_USER: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/flat-user";
/// A `PERSONALIZED_ACTIONS` campaign whose actions carry no popularity.
const CAMPAIGN_FLAT_ACTIONS: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/flat-actions";

/// `EXCLUDE ItemID WHERE Items.GENRE IN ("Comedy")`.
const FILTER_EXCLUDE_COMEDY: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/denominator-exclude-comedy";
/// `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
const FILTER_INCLUDE_GENRE: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/denominator-include-genre";
/// `INCLUDE ItemID WHERE Items.YEAR >= 2010`.
const FILTER_RECENT: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/denominator-recent";
/// `INCLUDE ActionID WHERE Actions.CHANNEL IN ("App")`.
const FILTER_APP_ACTIONS: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/denominator-app-actions";

/// How many entries the seeded Items catalogs hold.
const CATALOG_SIZE: usize = 24;
/// How many entries the seeded Actions catalogs hold.
const ACTION_CATALOG_SIZE: usize = 10;

/// Two responses may not report an entry's certainty differently at all, so scores are
/// compared exactly up to double-rounding noise.
const TOLERANCE: f64 = 1e-12;

/// The predicate a row's registered filter expression means, over a stored entry.
type Retains = fn(&Item) -> bool;

/// One re-ranking row: its name, the `inputList` it sends, the `filterArn` and
/// `filterValues` it names, and the predicate that filter expression means.
type RankingRow = (
    &'static str,
    Vec<String>,
    Option<&'static str>,
    Value,
    Retains,
);

/// An emulator holding the campaigns and filters these suites read.
///
/// The campaigns come in two shapes: a popularity-carrying catalog (`items` /
/// `actions`, whose `base_score` decreases with the index) and a catalog whose entries
/// all carry `base_score` 0 — a catalog the model has no popularity to predict from.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    assert!(
        registry::register_campaign(
            engine,
            Campaign::new(
                CAMPAIGN_RANKING,
                "ranking",
                RecipeType::PersonalizedRanking,
                items(CATALOG_SIZE),
            ),
        ),
        "the campaign seam must register the ranking campaign"
    );
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new(
                CAMPAIGN_USER,
                "user",
                RecipeType::UserPersonalization,
                items(CATALOG_SIZE),
            ),
        ),
        "the campaign seam must register the user-personalization campaign"
    );
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new_actions(CAMPAIGN_ACTIONS, "actions", actions(ACTION_CATALOG_SIZE)),
        ),
        "the campaign seam must register the actions campaign"
    );
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new(
                CAMPAIGN_FLAT_RANKING,
                "flat-ranking",
                RecipeType::PersonalizedRanking,
                flat_items(CATALOG_SIZE),
            ),
        ),
        "the campaign seam must register the popularity-less ranking campaign"
    );
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new(
                CAMPAIGN_FLAT_USER,
                "flat-user",
                RecipeType::UserPersonalization,
                flat_items(CATALOG_SIZE),
            ),
        ),
        "the campaign seam must register the popularity-less user campaign"
    );
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new_actions(
                CAMPAIGN_FLAT_ACTIONS,
                "flat-actions",
                flat_actions(ACTION_CATALOG_SIZE),
            ),
        ),
        "the campaign seam must register the popularity-less actions campaign"
    );

    let filters = [
        (
            FILTER_EXCLUDE_COMEDY,
            "EXCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\")",
        ),
        (
            FILTER_INCLUDE_GENRE,
            "INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)",
        ),
        (FILTER_RECENT, "INCLUDE ItemID WHERE Items.YEAR >= 2010"),
        (
            FILTER_APP_ACTIONS,
            "INCLUDE ActionID WHERE Actions.CHANNEL IN (\"App\")",
        ),
    ];
    for (arn, expression) in filters {
        assert_eq!(
            registry::register_filter(engine, arn, expression),
            Ok(true),
            "case {arn}: the filter seam must accept {expression:?}"
        );
    }
    emulator
}

/// A catalog of `size` entries that carry the same metadata columns as
/// [`crate::support::items`] but NO popularity at all.
fn flat_items(size: usize) -> Vec<Item> {
    items(size)
        .into_iter()
        .map(|item| Item {
            base_score: 0.0,
            ..item
        })
        .collect()
}

/// An Actions catalog of `size` entries that carry no popularity.
fn flat_actions(size: usize) -> Vec<Action> {
    actions(size)
        .into_iter()
        .map(|action| Action {
            base_score: 0.0,
            ..action
        })
        .collect()
}

/// The stored `GENRE` of one seeded catalog entry.
fn genre(item: &Item) -> &str {
    item.attributes
        .get(COLUMN_GENRE)
        .map(String::as_str)
        .unwrap_or_default()
}

/// The stored `YEAR` of one seeded catalog entry, as a number.
fn year(item: &Item) -> i64 {
    item.attributes
        .get(COLUMN_YEAR)
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}

/// Every id of the seeded Items catalog, in catalog order.
fn catalog_ids() -> Vec<String> {
    items(CATALOG_SIZE)
        .into_iter()
        .map(|item| item.item_id)
        .collect()
}

/// Assert that each entry of `reported` carries exactly the score `reference` — the
/// same request over the whole catalog — gives it.
fn assert_matches_the_catalog_reference(
    case: &str,
    reported: &BTreeMap<String, f64>,
    reference: &BTreeMap<String, f64>,
) {
    for (id, score) in reported {
        let expected = reference.get(id).unwrap_or_else(|| {
            panic!("case {case}: {id} is not one of the scored catalog entries {reference:?}")
        });
        assert!(
            (score - expected).abs() < TOLERANCE,
            "case {case}: {id} scores {expected} over the whole scored catalog, got {score}"
        );
    }
}

#[tokio::test]
async fn a_ranking_score_does_not_change_with_the_input_list_or_the_filter() {
    let emulator = seeded();
    let catalog = items(CATALOG_SIZE);
    let every_id = catalog_ids();
    let user_id = "user-7";

    // The reference: re-rank the WHOLE catalog with no filter, which reports every
    // scored entry and therefore the whole certainty.
    let whole = emulator
        .ok(
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": user_id,
                "inputList": every_id,
            }),
        )
        .await;
    let reference = scores_by_id(&whole, "personalizedRanking", "itemId");
    assert_eq!(
        reference.len(),
        CATALOG_SIZE,
        "the reference must report every catalog entry, got {reference:?}"
    );
    assert_sums_to_one("the whole catalog", &scores(&whole, "personalizedRanking"));

    // The rows differ in how many ids the request names, in their order, in whether
    // the catalog holds them, in repetition, and in which filter removes which part of
    // the list. `retains` is the predicate the row's filter expression means; an id the
    // catalog does not hold carries no stored attributes, so it survives an EXCLUDE
    // expression and is only listed in rows whose filter cannot drop it.
    let two: Vec<String> = every_id.iter().take(2).cloned().collect();
    let half: Vec<String> = every_id.iter().take(CATALOG_SIZE / 2).cloned().collect();
    let reversed: Vec<String> = every_id.iter().rev().cloned().collect();
    let every_other: Vec<String> = every_id.iter().step_by(2).cloned().collect();
    let rows: Vec<RankingRow> = vec![
        (
            "one known id",
            vec![every_id[5].clone()],
            None,
            json!({}),
            |_| true,
        ),
        ("two known ids", two.clone(), None, json!({}), |_| true),
        ("half the catalog", half.clone(), None, json!({}), |_| true),
        (
            "the whole catalog in reverse order",
            reversed.clone(),
            None,
            json!({}),
            |_| true,
        ),
        (
            "every other id",
            every_other.clone(),
            None,
            json!({}),
            |_| true,
        ),
        (
            "a repeated id",
            vec![
                every_id[3].clone(),
                every_id[3].clone(),
                every_id[9].clone(),
            ],
            None,
            json!({}),
            |_| true,
        ),
        (
            "known ids and an id the catalog never held",
            vec![
                every_id[1].clone(),
                "no-such-item".to_string(),
                every_id[7].clone(),
            ],
            None,
            json!({}),
            |_| true,
        ),
        (
            "two ids, EXCLUDE Comedy",
            two.clone(),
            Some(FILTER_EXCLUDE_COMEDY),
            json!({}),
            |item| genre(item) != "Comedy",
        ),
        (
            "half the catalog, EXCLUDE Comedy",
            half.clone(),
            Some(FILTER_EXCLUDE_COMEDY),
            json!({}),
            |item| genre(item) != "Comedy",
        ),
        (
            "an unknown id, EXCLUDE Comedy (an empty attribute set cannot be excluded)",
            vec![
                every_id[0].clone(),
                "no-such-item".to_string(),
                every_id[2].clone(),
            ],
            Some(FILTER_EXCLUDE_COMEDY),
            json!({}),
            |item| genre(item) != "Comedy",
        ),
        (
            "the whole catalog, INCLUDE GENRE Comedy",
            every_id.clone(),
            Some(FILTER_INCLUDE_GENRE),
            json!({ "GENRE": "Comedy" }),
            |item| genre(item) == "Comedy",
        ),
        (
            "half the catalog, INCLUDE GENRE Comedy or Drama",
            half.clone(),
            Some(FILTER_INCLUDE_GENRE),
            json!({ "GENRE": "Comedy,Drama" }),
            |item| matches!(genre(item), "Comedy" | "Drama"),
        ),
        (
            "every other id, INCLUDE YEAR >= 2010",
            every_other.clone(),
            Some(FILTER_RECENT),
            json!({}),
            |item| year(item) >= 2010,
        ),
    ];

    for (case, input_list, filter_arn, filter_values, retains) in rows {
        let mut body = json!({
            "campaignArn": CAMPAIGN_RANKING,
            "userId": user_id,
            "inputList": input_list,
        });
        if let Some(filter_arn) = filter_arn {
            body["filterArn"] = json!(filter_arn);
            body["filterValues"] = filter_values;
        }
        let response = emulator.ok(PERSONALIZE_RANKING, body).await;

        // The ranking holds exactly the distinct listed ids the filter kept: an id the
        // caller repeated names one catalog entry, whose share the ranking already
        // reports.
        let mut expected: Vec<String> = input_list
            .iter()
            .filter(|id| {
                catalog
                    .iter()
                    .find(|item| &item.item_id == *id)
                    .is_none_or(retains)
            })
            .cloned()
            .collect();
        let mut returned = ids(&response, "personalizedRanking", "itemId");
        let reported = scores_by_id(&response, "personalizedRanking", "itemId");
        returned.sort();
        expected.sort();
        expected.dedup();
        assert_eq!(
            returned, expected,
            "case {case}: the ranking must hold exactly the distinct listed ids its \
             filter kept"
        );

        // Every id the catalog holds keeps the score the whole-catalog call gave it;
        // an id the catalog never held carries no prediction at all.
        for (id, score) in &reported {
            match reference.get(id) {
                Some(expected) => assert!(
                    (score - expected).abs() < TOLERANCE,
                    "case {case}: {id} scores {expected} over the whole scored catalog, \
                     got {score}"
                ),
                None => assert_eq!(
                    *score, 0.0,
                    "case {case}: {id} is not in the catalog, so it carries no prediction, \
                     got {score}"
                ),
            }
        }
        assert_descending_and_bounded(case, &scores(&response, "personalizedRanking"));

        // A ranking of part of the catalog carries only part of the certainty: the
        // share of the unreported entries stays with them.
        let total: f64 = reported.values().sum();
        if expected.len() < CATALOG_SIZE {
            assert!(
                total < 1.0 - 1e-9,
                "case {case}: a ranking of {} of {CATALOG_SIZE} entries must not carry the \
                 whole certainty, got {total}",
                expected.len()
            );
        }
    }
}

#[tokio::test]
async fn a_recommendation_score_does_not_change_with_the_filter_applied() {
    let emulator = seeded();
    let catalog = items(CATALOG_SIZE);
    let action_catalog = actions(ACTION_CATALOG_SIZE);
    let user_id = "user-11";

    // `GetRecommendations`: one reference over the whole catalog, then the same request
    // with each filter and each result count.
    let whole = emulator
        .ok(
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": user_id,
                "numResults": CATALOG_SIZE,
            }),
        )
        .await;
    let reference = scores_by_id(&whole, "itemList", "itemId");
    assert_sums_to_one("the whole catalog", &scores(&whole, "itemList"));

    let item_rows: [(&str, &str, Value, Retains); 3] = [
        (
            "EXCLUDE Comedy",
            FILTER_EXCLUDE_COMEDY,
            json!({}),
            |item| genre(item) != "Comedy",
        ),
        (
            "INCLUDE GENRE Comedy",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "Comedy" }),
            |item| genre(item) == "Comedy",
        ),
        (
            "INCLUDE YEAR >= 2010",
            FILTER_RECENT,
            json!({}),
            |item| year(item) >= 2010,
        ),
    ];
    for (row, filter_arn, filter_values, retains) in item_rows {
        let surviving = catalog.iter().filter(|item| retains(item)).count();
        assert!(
            surviving > 0 && surviving < CATALOG_SIZE,
            "case {row}: the row must select a strict, non-empty subset, got {surviving}"
        );
        for num_results in [1_usize, 3, CATALOG_SIZE] {
            let case = format!("{row} numResults {num_results}");
            let response = emulator
                .ok(
                    RECOMMENDATIONS,
                    json!({
                        "campaignArn": CAMPAIGN_USER,
                        "userId": user_id,
                        "numResults": num_results,
                        "filterArn": filter_arn,
                        "filterValues": filter_values,
                    }),
                )
                .await;
            let reported = scores_by_id(&response, "itemList", "itemId");
            assert_eq!(
                reported.len(),
                num_results.min(surviving),
                "case {case}: expected {} items, got {reported:?}",
                num_results.min(surviving)
            );
            assert_matches_the_catalog_reference(&case, &reported, &reference);
            assert_descending_and_bounded(&case, &scores(&response, "itemList"));
            let total: f64 = reported.values().sum();
            assert!(
                total < 1.0 - 1e-9,
                "case {case}: a filtered response reports part of the catalog, so it must \
                 carry part of the certainty, got {total}"
            );
        }
    }

    // `GetActionRecommendations`: the same rule over the Actions catalog.
    let whole_actions = emulator
        .ok(
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": user_id,
                "numResults": ACTION_CATALOG_SIZE,
            }),
        )
        .await;
    let action_reference = scores_by_id(&whole_actions, "actionList", "actionId");
    assert_sums_to_one(
        "the whole action catalog",
        &scores(&whole_actions, "actionList"),
    );

    let app_actions = action_catalog
        .iter()
        .filter(|action| action.attributes.get("CHANNEL").map(String::as_str) == Some("App"))
        .count();
    for num_results in [1_usize, 2, ACTION_CATALOG_SIZE] {
        let case = format!("CHANNEL IN (App) numResults {num_results}");
        let response = emulator
            .ok(
                ACTION_RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_ACTIONS,
                    "userId": user_id,
                    "numResults": num_results,
                    "filterArn": FILTER_APP_ACTIONS,
                }),
            )
            .await;
        let reported = scores_by_id(&response, "actionList", "actionId");
        assert_eq!(
            reported.len(),
            num_results.min(app_actions),
            "case {case}: expected {} actions, got {reported:?}",
            num_results.min(app_actions)
        );
        assert_matches_the_catalog_reference(&case, &reported, &action_reference);
        assert_descending_and_bounded(&case, &scores(&response, "actionList"));
        let total: f64 = reported.values().sum();
        assert!(
            total < 1.0 - 1e-9,
            "case {case}: a filtered response reports part of the action catalog, so it must \
             carry part of the certainty, got {total}"
        );
    }
}

#[tokio::test]
async fn a_catalog_with_no_popularity_reports_no_certainty() {
    let emulator = seeded();
    let every_id = catalog_ids();

    // A catalog whose entries carry no popularity gives the model nothing to be
    // certain about, so no entry may be reported with an invented share — least of all
    // the even 1/N a renormalized zero set would produce.
    let rows: [(&str, &str, Value, &str, &str, usize); 5] = [
        (
            "ranking, two ids",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_FLAT_RANKING,
                "userId": "user-1",
                "inputList": [every_id[0].clone(), every_id[1].clone()],
            }),
            "personalizedRanking",
            "itemId",
            2,
        ),
        (
            "ranking, the whole catalog",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_FLAT_RANKING,
                "userId": "user-2",
                "inputList": every_id,
            }),
            "personalizedRanking",
            "itemId",
            CATALOG_SIZE,
        ),
        (
            "recommendations, three results",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_FLAT_USER,
                "userId": "user-3",
                "numResults": 3,
            }),
            "itemList",
            "itemId",
            3,
        ),
        (
            "recommendations, the whole catalog",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_FLAT_USER,
                "userId": "user-4",
                "numResults": CATALOG_SIZE,
            }),
            "itemList",
            "itemId",
            CATALOG_SIZE,
        ),
        (
            "action recommendations, the whole catalog",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_FLAT_ACTIONS,
                "userId": "user-5",
                "numResults": ACTION_CATALOG_SIZE,
            }),
            "actionList",
            "actionId",
            ACTION_CATALOG_SIZE,
        ),
    ];

    for (case, route, body, member, id_member, expected_len) in rows {
        let response = emulator.ok(route, body).await;
        let returned = ids(&response, member, id_member);
        assert_eq!(
            returned.len(),
            expected_len,
            "case {case}: expected {expected_len} entries, got {returned:?}"
        );
        for (id, score) in scores_by_id(&response, member, id_member) {
            assert_eq!(
                score, 0.0,
                "case {case}: {id} carries no popularity, so the response must report no \
                 certainty for it, got {score}"
            );
        }
    }
}
