// SPDX-License-Identifier: CC0-1.0

//! The one score-descending ordering all three inference operations report, and the
//! tie-break key each of them settles an equal pair with.
//!
//! Every one of these operations reports its list "sorted in descending order by
//! prediction score", so ONE comparator decides the order for all three; the key that
//! settles a tied pair is the only part that is per-operation — the entry's own id
//! where the operation chooses the list (`GetRecommendations`,
//! `GetActionRecommendations`), the caller's `inputList` position where it re-ranks a
//! list the caller sent (`GetPersonalizedRanking`).
//!
//! A catalog whose entries carry no popularity scores every entry 0.0, so EVERY pair
//! ties and the tie-break key alone decides the response order: that is what pins the
//! shared comparator's tie half per operation. A catalog that does carry popularity
//! pins its score half, through each operation.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Action, Campaign, Item, RecipeType,
};

use crate::support::{
    actions, assert_descending_and_bounded, ids, items, scores, Emulator,
    ACTION_RECOMMENDATIONS, PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// A `PERSONALIZED_RANKING` campaign whose catalog entries carry no popularity.
const CAMPAIGN_FLAT_RANKING: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/tie-flat-ranking";
/// A `USER_PERSONALIZATION` campaign whose catalog entries carry no popularity.
const CAMPAIGN_FLAT_USER: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/tie-flat-user";
/// A `PERSONALIZED_ACTIONS` campaign whose actions carry no popularity.
const CAMPAIGN_FLAT_ACTIONS: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/tie-flat-actions";

/// A `PERSONALIZED_RANKING` campaign over a catalog that does carry popularity.
const CAMPAIGN_RANKING: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/tie-ranking";
/// A `USER_PERSONALIZATION` campaign over a catalog that does carry popularity.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/tie-user";
/// A `PERSONALIZED_ACTIONS` campaign over actions that do carry popularity.
const CAMPAIGN_ACTIONS: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/tie-actions";

/// How many entries the seeded Items catalogs hold. Past ten, so the ids' ascending
/// order (`item-0`, `item-1`, `item-10`, …) is NOT the catalog order and an assertion
/// on it cannot pass by accident.
const CATALOG_SIZE: usize = 12;
/// How many entries the seeded Actions catalogs hold — also past ten, for the same
/// reason.
const ACTION_CATALOG_SIZE: usize = 11;

/// An id no seeded catalog holds: "If an item was not included in the training
/// dataset, the item is appended to the end of the reranked list"
/// (`GetPersonalizedRanking$inputList`).
const UNKNOWN_ITEM: &str = "item-not-in-the-catalog";

/// One row of an operation that chooses the returned list itself: its name, the route
/// and body it sends, the response list member and id member to read, and the order
/// the response must come back in.
type SelectionRow = (
    &'static str,
    &'static str,
    Value,
    &'static str,
    &'static str,
    Vec<String>,
);

/// An emulator holding the six campaigns these suites read: three over catalogs with
/// no popularity (every score ties) and three over catalogs that carry it.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    let campaigns = [
        (
            CAMPAIGN_FLAT_RANKING,
            RecipeType::PersonalizedRanking,
            flat_items(CATALOG_SIZE),
        ),
        (
            CAMPAIGN_FLAT_USER,
            RecipeType::UserPersonalization,
            flat_items(CATALOG_SIZE),
        ),
        (
            CAMPAIGN_RANKING,
            RecipeType::PersonalizedRanking,
            items(CATALOG_SIZE),
        ),
        (
            CAMPAIGN_USER,
            RecipeType::UserPersonalization,
            items(CATALOG_SIZE),
        ),
    ];
    for (arn, recipe_type, catalog) in campaigns {
        assert!(
            registry::register_campaign(
                engine,
                Campaign::new(arn, "tie-break", recipe_type, catalog),
            ),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    let action_campaigns = [
        (CAMPAIGN_FLAT_ACTIONS, flat_actions(ACTION_CATALOG_SIZE)),
        (CAMPAIGN_ACTIONS, actions(ACTION_CATALOG_SIZE)),
    ];
    for (arn, catalog) in action_campaigns {
        assert!(
            registry::register_campaign(
                engine,
                Campaign::new_actions(arn, "tie-break", catalog),
            ),
            "case {arn}: the campaign seam must register the action campaign"
        );
    }
    emulator
}

/// [`crate::support::items`] with every entry's popularity removed, so each entry
/// scores 0.0 and every pair of them ties.
fn flat_items(size: usize) -> Vec<Item> {
    items(size)
        .into_iter()
        .map(|item| Item {
            base_score: 0.0,
            ..item
        })
        .collect()
}

/// [`crate::support::actions`] with every action's popularity removed.
fn flat_actions(size: usize) -> Vec<Action> {
    actions(size)
        .into_iter()
        .map(|action| Action {
            base_score: 0.0,
            ..action
        })
        .collect()
}

/// Every id of the seeded Items catalog, in catalog order.
fn catalog_ids() -> Vec<String> {
    items(CATALOG_SIZE)
        .into_iter()
        .map(|item| item.item_id)
        .collect()
}

/// Every id of the seeded Actions catalog, in catalog order.
fn action_ids() -> Vec<String> {
    actions(ACTION_CATALOG_SIZE)
        .into_iter()
        .map(|action| action.action_id)
        .collect()
}

/// The same ids ASCENDING — the tie-break key of the two operations that choose the
/// list themselves.
fn ascending(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

/// The `inputList` a row sends, as a JSON array.
fn input_list(ids: &[String]) -> Value {
    Value::Array(ids.iter().map(|id| json!(id)).collect())
}

#[tokio::test]
async fn tied_scores_are_ordered_by_each_operations_own_tie_break_key() {
    let emulator = seeded();
    let catalog = catalog_ids();

    // `GetPersonalizedRanking` re-ranks the caller's own list, so a tie keeps the
    // position the caller sent it in. The rows differ along the shape of `inputList`:
    // how many ids it names, in what order, and whether the catalog holds them.
    let reversed: Vec<String> = catalog.iter().rev().cloned().collect();
    let rotated: Vec<String> = catalog[5..]
        .iter()
        .chain(catalog[..5].iter())
        .cloned()
        .collect();
    let every_other: Vec<String> = catalog.iter().step_by(2).cloned().collect();
    let with_unknown: Vec<String> = vec![
        UNKNOWN_ITEM.to_string(),
        catalog[7].clone(),
        catalog[2].clone(),
    ];

    let ranking_rows: [(&str, Vec<String>, Vec<String>); 6] = [
        (
            "ranking, catalog order",
            catalog.clone(),
            catalog.clone(),
        ),
        ("ranking, reverse catalog order", reversed.clone(), reversed),
        ("ranking, rotated by five", rotated.clone(), rotated),
        ("ranking, every other id", every_other.clone(), every_other),
        (
            "ranking, a single id",
            vec![catalog[9].clone()],
            vec![catalog[9].clone()],
        ),
        (
            "ranking, an id the catalog does not hold, sent first",
            with_unknown,
            // The unknown id carries no relevance for the user, so it is appended to
            // the end; the two the catalog holds keep their `inputList` order.
            vec![
                catalog[7].clone(),
                catalog[2].clone(),
                UNKNOWN_ITEM.to_string(),
            ],
        ),
    ];

    for (case, sent, expected) in ranking_rows {
        let response = emulator
            .ok(
                PERSONALIZE_RANKING,
                json!({
                    "campaignArn": CAMPAIGN_FLAT_RANKING,
                    "userId": "tie-user",
                    "inputList": input_list(&sent),
                }),
            )
            .await;
        let returned = ids(&response, "personalizedRanking", "itemId");
        assert_eq!(
            returned, expected,
            "case {case}: every score ties, so the ranking must keep the inputList \
             order {expected:?}, got {returned:?}"
        );
        let reported = scores(&response, "personalizedRanking");
        assert!(
            reported.iter().all(|score| *score == 0.0),
            "case {case}: a catalog with no popularity ties every score at 0.0, got \
             {reported:?}"
        );
    }

    // The two operations that choose the list themselves settle a tie by the entry's
    // id ASCENDING, which is not the catalog order for these ids. The rows differ
    // along how many entries come back.
    let ascending_items = ascending(catalog);
    let ascending_actions = ascending(action_ids());
    let selection_rows: [SelectionRow; 5] = [
        (
            "recommendations, three results",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_FLAT_USER,
                "userId": "tie-user",
                "numResults": 3,
            }),
            "itemList",
            "itemId",
            ascending_items[..3].to_vec(),
        ),
        (
            "recommendations, five results",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_FLAT_USER,
                "userId": "another-user",
                "numResults": 5,
            }),
            "itemList",
            "itemId",
            ascending_items[..5].to_vec(),
        ),
        (
            "recommendations, the whole catalog",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_FLAT_USER,
                "userId": "tie-user",
                "numResults": CATALOG_SIZE,
            }),
            "itemList",
            "itemId",
            ascending_items.clone(),
        ),
        (
            "action recommendations, two results",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_FLAT_ACTIONS,
                "userId": "tie-user",
                "numResults": 2,
            }),
            "actionList",
            "actionId",
            ascending_actions[..2].to_vec(),
        ),
        (
            "action recommendations, the whole catalog",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_FLAT_ACTIONS,
                "userId": "tie-user",
                "numResults": ACTION_CATALOG_SIZE,
            }),
            "actionList",
            "actionId",
            ascending_actions.clone(),
        ),
    ];

    for (case, route, body, member, id_member, expected) in selection_rows {
        let response = emulator.ok(route, body).await;
        let returned = ids(&response, member, id_member);
        assert_eq!(
            returned, expected,
            "case {case}: every score ties, so the response must be ordered by id \
             ascending {expected:?}, got {returned:?}"
        );
        let reported = scores(&response, member);
        assert!(
            reported.iter().all(|score| *score == 0.0),
            "case {case}: a catalog with no popularity ties every score at 0.0, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn every_operation_reports_its_list_by_score_descending() {
    let emulator = seeded();
    let catalog = catalog_ids();

    // One rule, three operations: the reported list never increases in score. The rows
    // differ along the operation, the user, how many entries come back and the order
    // the caller sent them in.
    let reversed: Vec<String> = catalog.iter().rev().cloned().collect();
    let rows: [(&str, &str, Value, &str); 6] = [
        (
            "ranking, catalog order",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-a",
                "inputList": input_list(&catalog),
            }),
            "personalizedRanking",
        ),
        (
            "ranking, reverse order, another user",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-b",
                "inputList": input_list(&reversed),
            }),
            "personalizedRanking",
        ),
        (
            "ranking, an id the catalog does not hold",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-c",
                "inputList": json!([catalog[3], UNKNOWN_ITEM, catalog[0]]),
            }),
            "personalizedRanking",
        ),
        (
            "recommendations, four results",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-a",
                "numResults": 4,
            }),
            "itemList",
        ),
        (
            "recommendations, the whole catalog, another user",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-d",
                "numResults": CATALOG_SIZE,
            }),
            "itemList",
        ),
        (
            "action recommendations, the whole catalog",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": "user-a",
                "numResults": ACTION_CATALOG_SIZE,
            }),
            "actionList",
        ),
    ];

    for (case, route, body, member) in rows {
        let response = emulator.ok(route, body).await;
        assert_descending_and_bounded(case, &scores(&response, member));
    }
}
