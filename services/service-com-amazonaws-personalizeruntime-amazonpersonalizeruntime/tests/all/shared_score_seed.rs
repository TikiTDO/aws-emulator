// SPDX-License-Identifier: CC0-1.0

//! ONE composition of the prediction-score seed, driven through all three inference
//! operations.
//!
//! A reported `score` is the entry's share of the certainty the service has for one
//! (resolved resource, personalization input, context, entry) tuple, so WHICH of those
//! inputs seed the score — and in which order — is a single rule the whole service
//! obeys, not a per-operation detail. The rule lives in
//! `crate::state::scoring::Seed`; these suites call that same helper from the test
//! side and compare its shares against what each operation actually reports, so a
//! handler that composed the tuple its own way would fail here even though the three
//! operations serve DISJOINT recipe types and no single request can reach two of them.
//!
//! The rows differ along what the rule generalizes over: which operation serves the
//! request, which resource it resolves (two resources over an identical catalog),
//! which personalization input that resource uses (`userId` or the `itemId` anchor of
//! a related-items campaign), whether a `context` is sent and how many keys it
//! carries, how much of the catalog the response reports, and whether the stored
//! catalog carries a popularity gradient or none at all.

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, scoring, Action, Campaign, Item, RecipeType,
};

use crate::support::{
    actions, assert_descending_and_bounded, assert_sums_to_one, items, scores, scores_by_id,
    Emulator, ACTION_RECOMMENDATIONS, PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// The `USER_PERSONALIZATION` campaign the `GetRecommendations` rows read.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/seed-user";
/// A second `USER_PERSONALIZATION` campaign over an IDENTICAL catalog: the resolved
/// resource is a seed part, so the two score the same request differently.
const CAMPAIGN_USER_TWIN: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/seed-user-twin";
/// The `RELATED_ITEMS` campaign whose personalization input is `itemId`.
const CAMPAIGN_RELATED: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/seed-related";
/// The `PERSONALIZED_RANKING` campaign the `GetPersonalizedRanking` rows re-rank.
const CAMPAIGN_RANKING: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/seed-ranking";
/// A second `PERSONALIZED_RANKING` campaign over an identical catalog.
const CAMPAIGN_RANKING_TWIN: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/seed-ranking-twin";
/// The `PERSONALIZED_ACTIONS` campaign the `GetActionRecommendations` rows read.
const CAMPAIGN_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/seed-actions";
/// A second `PERSONALIZED_ACTIONS` campaign over an identical Actions catalog.
const CAMPAIGN_ACTIONS_TWIN: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/seed-actions-twin";

/// A `USER_PERSONALIZATION` campaign whose entries are all EQUALLY popular.
const CAMPAIGN_FLAT_USER: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/seed-flat-user";
/// A `PERSONALIZED_RANKING` campaign whose entries are all equally popular.
const CAMPAIGN_FLAT_RANKING: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/seed-flat-ranking";
/// A `PERSONALIZED_ACTIONS` campaign whose actions are all equally popular.
const CAMPAIGN_FLAT_ACTIONS: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/seed-flat-actions";

/// How many entries the seeded Items catalogs hold.
const CATALOG_SIZE: usize = 8;
/// How many entries the seeded Actions catalogs hold.
const ACTION_CATALOG_SIZE: usize = 6;
/// The popularity every entry of a flat catalog carries.
const FLAT_POPULARITY: f64 = 1.0;

/// The user the baseline rows personalize for.
const USER_A: &str = "user-7";
/// A second user, to vary the personalization part.
const USER_B: &str = "user-8";

/// The item a related-items row is the anchor of.
const ANCHOR_A: &str = "item-2";
/// A second anchor item.
const ANCHOR_B: &str = "item-5";

/// Two responses may not report an entry's certainty differently at all, so scores are
/// compared exactly up to double-rounding noise.
const TOLERANCE: f64 = 1e-12;

/// An emulator holding every campaign these suites read.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    let campaigns = [
        (
            CAMPAIGN_USER,
            "seed-user",
            RecipeType::UserPersonalization,
            items(CATALOG_SIZE),
        ),
        (
            CAMPAIGN_USER_TWIN,
            "seed-user-twin",
            RecipeType::UserPersonalization,
            items(CATALOG_SIZE),
        ),
        (
            CAMPAIGN_RELATED,
            "seed-related",
            RecipeType::RelatedItems,
            items(CATALOG_SIZE),
        ),
        (
            CAMPAIGN_RANKING,
            "seed-ranking",
            RecipeType::PersonalizedRanking,
            items(CATALOG_SIZE),
        ),
        (
            CAMPAIGN_RANKING_TWIN,
            "seed-ranking-twin",
            RecipeType::PersonalizedRanking,
            items(CATALOG_SIZE),
        ),
        (
            CAMPAIGN_FLAT_USER,
            "seed-flat-user",
            RecipeType::UserPersonalization,
            flat_items(),
        ),
        (
            CAMPAIGN_FLAT_RANKING,
            "seed-flat-ranking",
            RecipeType::PersonalizedRanking,
            flat_items(),
        ),
    ];
    for (arn, name, recipe_type, catalog) in campaigns {
        assert!(
            registry::register_campaign(engine, Campaign::new(arn, name, recipe_type, catalog)),
            "case {arn}: the campaign seam must register the campaign"
        );
    }

    let action_campaigns = [
        (
            CAMPAIGN_ACTIONS,
            "seed-actions",
            actions(ACTION_CATALOG_SIZE),
        ),
        (
            CAMPAIGN_ACTIONS_TWIN,
            "seed-actions-twin",
            actions(ACTION_CATALOG_SIZE),
        ),
        (CAMPAIGN_FLAT_ACTIONS, "seed-flat-actions", flat_actions()),
    ];
    for (arn, name, catalog) in action_campaigns {
        assert!(
            registry::register_campaign(engine, Campaign::new_actions(arn, name, catalog)),
            "case {arn}: the campaign seam must register the actions campaign"
        );
    }
    emulator
}

/// An Items catalog whose entries are all equally popular, so the only seed part that
/// still tells them apart is the entry id.
fn flat_items() -> Vec<Item> {
    items(CATALOG_SIZE)
        .into_iter()
        .map(|item| Item {
            base_score: FLAT_POPULARITY,
            ..item
        })
        .collect()
}

/// An Actions catalog whose entries are all equally popular.
fn flat_actions() -> Vec<Action> {
    actions(ACTION_CATALOG_SIZE)
        .into_iter()
        .map(|action| Action {
            base_score: FLAT_POPULARITY,
            ..action
        })
        .collect()
}

/// The seeded Items catalog as `(entry id, stored popularity)`.
fn catalog() -> Vec<(String, f64)> {
    items(CATALOG_SIZE)
        .into_iter()
        .map(|item| (item.item_id, item.base_score))
        .collect()
}

/// The seeded Items catalog minus one entry — the candidate set a related-items
/// request has, which never includes the anchor item itself.
fn catalog_without(anchor: &str) -> Vec<(String, f64)> {
    catalog()
        .into_iter()
        .filter(|(item_id, _)| item_id != anchor)
        .collect()
}

/// The seeded Actions catalog as `(entry id, stored popularity)`.
fn action_catalog() -> Vec<(String, f64)> {
    actions(ACTION_CATALOG_SIZE)
        .into_iter()
        .map(|action| (action.action_id, action.base_score))
        .collect()
}

/// Every id of the seeded Items catalog, in catalog order.
fn catalog_ids() -> Vec<String> {
    catalog().into_iter().map(|(item_id, _)| item_id).collect()
}

/// The response member a route reports its scored entries in, and the member each
/// entry carries its id in.
fn list_member(route: &str) -> (&'static str, &'static str) {
    match route {
        RECOMMENDATIONS => ("itemList", "itemId"),
        PERSONALIZE_RANKING => ("personalizedRanking", "itemId"),
        ACTION_RECOMMENDATIONS => ("actionList", "actionId"),
        other => panic!("case {other}: not one of this service's routes"),
    }
}

/// One row of the table: a request, and the seed parts the service must have scored it
/// with.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// The operation's modelled route.
    route: &'static str,
    /// The request body.
    body: Value,
    /// The resolved resource the scores are derived under.
    scope: &'static str,
    /// The ONE personalization input that resource uses.
    personalization: &'static str,
    /// The WHOLE scored candidate set the resolved resource holds for this request.
    candidates: Vec<(String, f64)>,
    /// Whether the response reports every one of those candidates (and so carries the
    /// whole certainty).
    reports_every_candidate: bool,
}

/// The `context` part of a row's seed: the map the request sent, or `None` for
/// `GetActionRecommendations`, whose model carries no `context` member at all.
fn context_part(row: &Row) -> Option<HashMap<String, String>> {
    if row.route == ACTION_RECOMMENDATIONS {
        return None;
    }
    let sent = row
        .body
        .get("context")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Some(sent)
}

/// The share of the certainty the SHARED seed composition gives each candidate of a
/// row — the numbers the row's operation has to report.
fn expected_shares(row: &Row) -> BTreeMap<String, f64> {
    let context = context_part(row);
    let seed = match context.as_ref() {
        Some(context) => {
            scoring::Seed::with_context(row.scope, row.personalization, Some(context))
        }
        None => scoring::Seed::without_context(row.scope, row.personalization),
    };
    let mut shares: Vec<f64> = row
        .candidates
        .iter()
        .map(|(entry_id, base_score)| seed.relevance(*base_score, entry_id))
        .collect();
    scoring::normalize(&mut shares);
    row.candidates
        .iter()
        .map(|(entry_id, _)| entry_id.clone())
        .zip(shares)
        .collect()
}

/// Dispatch one request that must succeed and return the score it reports per entry
/// id.
async fn reported_shares(emulator: &Emulator, route: &str, body: Value) -> BTreeMap<String, f64> {
    let (member, id_member) = list_member(route);
    let response = emulator.ok(route, body).await;
    scores_by_id(&response, member, id_member)
}

/// Assert two responses over a shared candidate set report at least one entry
/// differently — the part `varies` names reached the seed.
fn assert_differs(
    case: &str,
    varies: &str,
    left: &BTreeMap<String, f64>,
    right: &BTreeMap<String, f64>,
) {
    let shared: Vec<&String> = left.keys().filter(|id| right.contains_key(*id)).collect();
    assert!(
        !shared.is_empty(),
        "case {case}: the two responses must share at least one entry, got {left:?} and {right:?}"
    );
    let differs = shared
        .iter()
        .any(|id| (left[*id] - right[*id]).abs() > TOLERANCE);
    assert!(
        differs,
        "case {case}: {varies} seeds the score, so the two responses must differ on some \
         shared entry, got {left:?} and {right:?}"
    );
}

/// The table: every operation, scored against the one shared seed composition.
#[tokio::test]
async fn every_operation_scores_through_the_one_shared_seed_composition() {
    let emulator = seeded();
    let every_id = catalog_ids();
    let half: Vec<String> = every_id.iter().take(CATALOG_SIZE / 2).cloned().collect();

    let rows: Vec<Row> = vec![
        Row {
            case: "user personalization, no context",
            route: RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": USER_A,
                "numResults": CATALOG_SIZE,
            }),
            scope: CAMPAIGN_USER,
            personalization: USER_A,
            candidates: catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "user personalization, a one-key context",
            route: RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": USER_A,
                "numResults": CATALOG_SIZE,
                "context": {"DEVICE": "Mobile"},
            }),
            scope: CAMPAIGN_USER,
            personalization: USER_A,
            candidates: catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "user personalization, a two-key context",
            route: RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": USER_A,
                "numResults": CATALOG_SIZE,
                "context": {"DEVICE": "Mobile", "WEATHER": "Rain"},
            }),
            scope: CAMPAIGN_USER,
            personalization: USER_A,
            candidates: catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "user personalization, another user",
            route: RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": USER_B,
                "numResults": CATALOG_SIZE,
            }),
            scope: CAMPAIGN_USER,
            personalization: USER_B,
            candidates: catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "user personalization, a second campaign over the same catalog",
            route: RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_USER_TWIN,
                "userId": USER_A,
                "numResults": CATALOG_SIZE,
            }),
            scope: CAMPAIGN_USER_TWIN,
            personalization: USER_A,
            candidates: catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "user personalization, fewer results than the catalog holds",
            route: RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": USER_A,
                "numResults": 3,
            }),
            scope: CAMPAIGN_USER,
            personalization: USER_A,
            candidates: catalog(),
            reports_every_candidate: false,
        },
        Row {
            case: "related items, the itemId anchor is the personalization input",
            route: RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_RELATED,
                "itemId": ANCHOR_A,
                "numResults": CATALOG_SIZE,
            }),
            scope: CAMPAIGN_RELATED,
            personalization: ANCHOR_A,
            candidates: catalog_without(ANCHOR_A),
            reports_every_candidate: true,
        },
        Row {
            case: "related items, another anchor and a context",
            route: RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_RELATED,
                "itemId": ANCHOR_B,
                "numResults": CATALOG_SIZE,
                "context": {"DEVICE": "TV"},
            }),
            scope: CAMPAIGN_RELATED,
            personalization: ANCHOR_B,
            candidates: catalog_without(ANCHOR_B),
            reports_every_candidate: true,
        },
        Row {
            case: "ranking, the whole catalog in the input list",
            route: PERSONALIZE_RANKING,
            body: json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": USER_A,
                "inputList": every_id,
            }),
            scope: CAMPAIGN_RANKING,
            personalization: USER_A,
            candidates: catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "ranking, a context",
            route: PERSONALIZE_RANKING,
            body: json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": USER_A,
                "inputList": every_id,
                "context": {"DEVICE": "Mobile", "WEATHER": "Rain"},
            }),
            scope: CAMPAIGN_RANKING,
            personalization: USER_A,
            candidates: catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "ranking, another user",
            route: PERSONALIZE_RANKING,
            body: json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": USER_B,
                "inputList": every_id,
            }),
            scope: CAMPAIGN_RANKING,
            personalization: USER_B,
            candidates: catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "ranking, a second campaign over the same catalog",
            route: PERSONALIZE_RANKING,
            body: json!({
                "campaignArn": CAMPAIGN_RANKING_TWIN,
                "userId": USER_A,
                "inputList": every_id,
            }),
            scope: CAMPAIGN_RANKING_TWIN,
            personalization: USER_A,
            candidates: catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "ranking, half the catalog in the input list",
            route: PERSONALIZE_RANKING,
            body: json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": USER_A,
                "inputList": half,
            }),
            scope: CAMPAIGN_RANKING,
            personalization: USER_A,
            candidates: catalog(),
            reports_every_candidate: false,
        },
        Row {
            case: "action recommendations, no context member is modelled",
            route: ACTION_RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": USER_A,
                "numResults": ACTION_CATALOG_SIZE,
            }),
            scope: CAMPAIGN_ACTIONS,
            personalization: USER_A,
            candidates: action_catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "action recommendations, another user",
            route: ACTION_RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": USER_B,
                "numResults": ACTION_CATALOG_SIZE,
            }),
            scope: CAMPAIGN_ACTIONS,
            personalization: USER_B,
            candidates: action_catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "action recommendations, a second campaign over the same catalog",
            route: ACTION_RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_ACTIONS_TWIN,
                "userId": USER_A,
                "numResults": ACTION_CATALOG_SIZE,
            }),
            scope: CAMPAIGN_ACTIONS_TWIN,
            personalization: USER_A,
            candidates: action_catalog(),
            reports_every_candidate: true,
        },
        Row {
            case: "action recommendations, fewer results than the catalog holds",
            route: ACTION_RECOMMENDATIONS,
            body: json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": USER_A,
                "numResults": 2,
            }),
            scope: CAMPAIGN_ACTIONS,
            personalization: USER_A,
            candidates: action_catalog(),
            reports_every_candidate: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        let (member, id_member) = list_member(row.route);
        let expected = expected_shares(row);
        let response = emulator.ok(row.route, row.body.clone()).await;
        let reported = scores_by_id(&response, member, id_member);
        let ordered = scores(&response, member);

        assert!(
            !reported.is_empty(),
            "case {case}: expected scored entries, got {response}"
        );
        if row.reports_every_candidate {
            assert_eq!(
                reported.len(),
                row.candidates.len(),
                "case {case}: expected every candidate reported, got {reported:?}"
            );
            assert_sums_to_one(case, &ordered);
        } else {
            assert!(
                reported.len() < row.candidates.len(),
                "case {case}: expected part of the candidate set, got {reported:?}"
            );
            assert_descending_and_bounded(case, &ordered);
        }
        for (entry_id, score) in &reported {
            let want = expected.get(entry_id).unwrap_or_else(|| {
                panic!("case {case}: {entry_id} is not one of the scored candidates {expected:?}")
            });
            assert!(
                (score - want).abs() < TOLERANCE,
                "case {case}: the shared seed scores {entry_id} {want}, got {score}"
            );
        }

        // The operations are `@idempotent`: the same seed parts, so the same numbers.
        let replayed = reported_shares(&emulator, row.route, row.body.clone()).await;
        assert_eq!(
            replayed, reported,
            "case {case}: a replayed identical request must report identical scores"
        );
    }
}

/// Every part of the shared seed has to reach the score of the operations that carry
/// it, and no part may be dropped by one of them.
#[tokio::test]
async fn each_seed_part_moves_the_scores_of_every_operation_that_carries_it() {
    let emulator = seeded();
    let every_id = catalog_ids();

    // (what the pair varies, route, the two requests that differ only in that part).
    let pairs: Vec<(&str, &str, Value, Value)> = vec![
        (
            "the resolved resource",
            RECOMMENDATIONS,
            json!({"campaignArn": CAMPAIGN_USER, "userId": USER_A, "numResults": CATALOG_SIZE}),
            json!({
                "campaignArn": CAMPAIGN_USER_TWIN,
                "userId": USER_A,
                "numResults": CATALOG_SIZE,
            }),
        ),
        (
            "the userId the resource uses",
            RECOMMENDATIONS,
            json!({"campaignArn": CAMPAIGN_USER, "userId": USER_A, "numResults": CATALOG_SIZE}),
            json!({"campaignArn": CAMPAIGN_USER, "userId": USER_B, "numResults": CATALOG_SIZE}),
        ),
        (
            "the itemId the resource uses",
            RECOMMENDATIONS,
            json!({"campaignArn": CAMPAIGN_RELATED, "itemId": ANCHOR_A, "numResults": CATALOG_SIZE}),
            json!({"campaignArn": CAMPAIGN_RELATED, "itemId": ANCHOR_B, "numResults": CATALOG_SIZE}),
        ),
        (
            "the context",
            RECOMMENDATIONS,
            json!({"campaignArn": CAMPAIGN_USER, "userId": USER_A, "numResults": CATALOG_SIZE}),
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": USER_A,
                "numResults": CATALOG_SIZE,
                "context": {"DEVICE": "Mobile"},
            }),
        ),
        (
            "the resolved resource",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": USER_A,
                "inputList": every_id,
            }),
            json!({
                "campaignArn": CAMPAIGN_RANKING_TWIN,
                "userId": USER_A,
                "inputList": every_id,
            }),
        ),
        (
            "the userId the resource uses",
            PERSONALIZE_RANKING,
            json!({"campaignArn": CAMPAIGN_RANKING, "userId": USER_A, "inputList": every_id}),
            json!({"campaignArn": CAMPAIGN_RANKING, "userId": USER_B, "inputList": every_id}),
        ),
        (
            "the context",
            PERSONALIZE_RANKING,
            json!({"campaignArn": CAMPAIGN_RANKING, "userId": USER_A, "inputList": every_id}),
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": USER_A,
                "inputList": every_id,
                "context": {"DEVICE": "Mobile"},
            }),
        ),
        (
            "the resolved resource",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": USER_A,
                "numResults": ACTION_CATALOG_SIZE,
            }),
            json!({
                "campaignArn": CAMPAIGN_ACTIONS_TWIN,
                "userId": USER_A,
                "numResults": ACTION_CATALOG_SIZE,
            }),
        ),
        (
            "the userId the resource uses",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": USER_A,
                "numResults": ACTION_CATALOG_SIZE,
            }),
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": USER_B,
                "numResults": ACTION_CATALOG_SIZE,
            }),
        ),
    ];

    for (varies, route, left_body, right_body) in pairs {
        let case = format!("{route} varying {varies}");
        let left = reported_shares(&emulator, route, left_body).await;
        let right = reported_shares(&emulator, route, right_body).await;
        assert_differs(&case, varies, &left, &right);
    }
}

/// The entry's own id is a seed part too: over a catalog whose entries are all equally
/// popular, the only thing left to tell two entries apart is their id, so every
/// reported score is still distinct — in all three operations.
#[tokio::test]
async fn the_entry_id_seeds_the_score_of_every_operation() {
    let emulator = seeded();
    let every_id = catalog_ids();

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "recommendations over an equally popular catalog",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_FLAT_USER,
                "userId": USER_A,
                "numResults": CATALOG_SIZE,
            }),
        ),
        (
            "ranking over an equally popular catalog",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_FLAT_RANKING,
                "userId": USER_A,
                "inputList": every_id,
            }),
        ),
        (
            "action recommendations over an equally popular catalog",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_FLAT_ACTIONS,
                "userId": USER_A,
                "numResults": ACTION_CATALOG_SIZE,
            }),
        ),
    ];

    for (case, route, body) in rows {
        let reported = reported_shares(&emulator, route, body).await;
        assert!(
            reported.len() > 1,
            "case {case}: expected more than one scored entry, got {reported:?}"
        );
        for (left_id, left) in &reported {
            for (right_id, right) in &reported {
                if left_id >= right_id {
                    continue;
                }
                assert!(
                    (left - right).abs() > TOLERANCE,
                    "case {case}: {left_id} and {right_id} are equally popular, so their ids \
                     must score them apart, got {left} and {right}"
                );
            }
        }
    }
}
