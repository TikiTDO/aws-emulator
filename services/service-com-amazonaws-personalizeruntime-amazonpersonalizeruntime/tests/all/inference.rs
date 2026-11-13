// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the three Amazon Personalize Runtime inference operations,
//! driven over the real wire path.
//!
//! Each suite asserts a RULE over a table of inputs rather than one recorded request:
//! the rows differ along whatever the rule generalizes over (the number of results,
//! the user, the filter expression, the promotion percentage, the shape of
//! `inputList`), and every expected value is computed from the SEEDED catalog rather
//! than written out, so a test cannot pass against a canned list.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, Item, RecipeType, Recommender, RequiredInput,
    MAX_METADATA_COLUMNS, MAX_NUM_ACTION_RESULTS, MAX_NUM_RESULTS,
    MAX_NUM_RESULTS_WITH_METADATA, MAX_RETAINED_RECOMMENDATION_IDS, RECOMMENDER_USE_CASES,
};

use crate::support::{
    actions, assert_descending_and_bounded, assert_sums_to_one, entries, error_code, ids,
    items, recommendation_id, scores, scores_by_id, Emulator, ACTION_RECOMMENDATIONS,
    COLUMN_GENRE, COLUMN_YEAR, PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// A `USER_PERSONALIZATION` campaign, with metadata in recommendations enabled.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/user-personalization";
/// A `RELATED_ITEMS` campaign, with metadata in recommendations NOT enabled.
const CAMPAIGN_RELATED: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/related-items";
/// A `PERSONALIZED_RANKING` campaign, with metadata in recommendations enabled.
const CAMPAIGN_RANKING: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/ranking";
/// A `PERSONALIZED_ACTIONS` campaign.
const CAMPAIGN_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/actions";
/// A domain recommender for the "Top picks for you" use case.
const RECOMMENDER_TOP_PICKS: &str =
    "arn:aws:personalize:us-east-1:000000000000:recommender/top-picks";

/// `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
const FILTER_INCLUDE_GENRE: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/include-genre";
/// `EXCLUDE ItemID WHERE Items.GENRE IN ("Comedy")`.
const FILTER_EXCLUDE_COMEDY: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/exclude-comedy";
/// `EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
const FILTER_EXCLUDE_GENRE: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/exclude-genre";
/// `INCLUDE ItemID WHERE Items.YEAR >= 2010`.
const FILTER_RECENT: &str = "arn:aws:personalize:us-east-1:000000000000:filter/recent";
/// `INCLUDE ItemID WHERE Items.GENRE IN ("Comedy") OR Items.GENRE IN ("Drama")`.
const FILTER_COMEDY_OR_DRAMA: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/comedy-or-drama";
/// `INCLUDE ItemID WHERE Items.GENRE IN ("Comedy") AND Items.YEAR >= 2010`.
const FILTER_RECENT_COMEDY: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/recent-comedy";
/// `INCLUDE ActionID WHERE Actions.CHANNEL IN ("App")`.
const FILTER_APP_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:filter/app-actions";

/// An ARN that is well formed but names nothing the registry holds.
const UNREGISTERED_CAMPAIGN: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/does-not-exist";
/// A filter ARN that names nothing the registry holds.
const UNREGISTERED_FILTER: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/does-not-exist";

/// How many items every seeded Items catalog holds.
const CATALOG_SIZE: usize = 40;
/// How many actions the seeded Actions catalog holds.
const ACTION_CATALOG_SIZE: usize = 12;

/// An emulator whose registry holds the campaigns, the recommender and the filters
/// every suite below reads.
///
/// This is the registration seam the issue calls for: the sibling Amazon Personalize
/// control plane creates these resources in production, so a test (or that crate,
/// once it serves `CreateCampaign`) puts them in the shared engine the same way.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    let campaigns = [
        (
            CAMPAIGN_USER,
            RecipeType::UserPersonalization,
            true,
        ),
        (CAMPAIGN_RELATED, RecipeType::RelatedItems, false),
        (CAMPAIGN_RANKING, RecipeType::PersonalizedRanking, true),
    ];
    for (arn, recipe_type, metadata) in campaigns {
        let campaign = Campaign::new(arn, "demo", recipe_type, items(CATALOG_SIZE));
        let campaign = if metadata {
            campaign.with_metadata_enabled()
        } else {
            campaign
        };
        assert!(
            registry::register_campaign(engine, campaign),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new_actions(CAMPAIGN_ACTIONS, "actions", actions(ACTION_CATALOG_SIZE))
        ),
        "the campaign seam must register the action campaign"
    );
    assert!(
        registry::register_recommender(
            engine,
            Recommender::new(
                RECOMMENDER_TOP_PICKS,
                "top-picks",
                "Top picks for you",
                items(CATALOG_SIZE)
            )
            .with_metadata_enabled()
        ),
        "the recommender seam must register the recommender"
    );

    let filters = [
        (
            FILTER_INCLUDE_GENRE,
            "INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)",
        ),
        (
            FILTER_EXCLUDE_COMEDY,
            "EXCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\")",
        ),
        (
            FILTER_EXCLUDE_GENRE,
            "EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE)",
        ),
        (FILTER_RECENT, "INCLUDE ItemID WHERE Items.YEAR >= 2010"),
        (
            FILTER_COMEDY_OR_DRAMA,
            "INCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\") OR Items.GENRE IN (\"Drama\")",
        ),
        (
            FILTER_RECENT_COMEDY,
            "INCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\") AND Items.YEAR >= 2010",
        ),
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

#[tokio::test]
async fn recommendations_come_from_the_stored_catalog_in_descending_score_order() {
    let emulator = seeded();
    let catalog = items(CATALOG_SIZE);

    // `numResults` is "The number of results to return. The default is 25", and the
    // modelled default of 0 means the same as absent. A request asking for more than
    // the catalog holds gets the catalog.
    let rows: [(&str, Option<i32>, usize); 6] = [
        ("absent numResults", None, 25),
        ("numResults 0", Some(0), 25),
        ("numResults 1", Some(1), 1),
        ("numResults 10", Some(10), 10),
        ("numResults = catalog size", Some(CATALOG_SIZE as i32), CATALOG_SIZE),
        ("numResults past the catalog", Some(100), CATALOG_SIZE),
    ];

    for (case, num_results, expected) in rows {
        let mut body = json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1" });
        if let Some(num_results) = num_results {
            body["numResults"] = json!(num_results);
        }
        let response = emulator.ok(RECOMMENDATIONS, body).await;
        let returned = ids(&response, "itemList", "itemId");
        assert_eq!(
            returned.len(),
            expected,
            "case {case}: expected {expected} items, got {returned:?}"
        );
        for item_id in &returned {
            assert!(
                catalog.iter().any(|item| &item.item_id == item_id),
                "case {case}: {item_id} is not in the stored catalog"
            );
        }
        let mut unique = returned.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            returned.len(),
            "case {case}: the list must not repeat an item, got {returned:?}"
        );
        let returned_scores = scores(&response, "itemList");
        if expected == CATALOG_SIZE {
            // Every candidate is reported, so the whole certainty is reported.
            assert_sums_to_one(case, &returned_scores);
        } else {
            assert_descending_and_bounded(case, &returned_scores);
        }
        recommendation_id(case, &response);
        // No `metadataColumns` was asked for, so the member is omitted entirely.
        for entry in entries(&response, "itemList") {
            assert!(
                entry.get("metadata").is_none(),
                "case {case}: metadata must be omitted when no metadataColumns was \
                 requested, got {entry}"
            );
        }
    }
}

#[tokio::test]
async fn an_identical_request_returns_an_identical_ordering_with_a_fresh_id() {
    let emulator = seeded();
    let body = json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1", "numResults": 10 });

    let first = emulator.ok(RECOMMENDATIONS, body.clone()).await;
    let second = emulator.ok(RECOMMENDATIONS, body.clone()).await;

    assert_eq!(
        ids(&first, "itemList", "itemId"),
        ids(&second, "itemList", "itemId"),
        "the operation is @idempotent: an identical request must return the same ordering"
    );
    assert_eq!(
        scores(&first, "itemList"),
        scores(&second, "itemList"),
        "an identical request must return the same scores"
    );
    let first_id = recommendation_id("first call", &first);
    let second_id = recommendation_id("second call", &second);
    assert_ne!(
        first_id, second_id,
        "each call mints its own recommendationId, got {first_id} twice"
    );
}

#[tokio::test]
async fn each_user_sees_an_ordering_of_its_own_over_the_same_catalog() {
    let emulator = seeded();

    // Exhaustive over the declared input space (16 users x 5 sizes) rather than
    // sampled, so there is no seed to report: every case below is always run.
    let mut orderings: Vec<Vec<String>> = Vec::new();
    for user in 0..16 {
        for num_results in 1..=5 {
            let case = format!("user-{user} numResults {num_results}");
            let response = emulator
                .ok(
                    RECOMMENDATIONS,
                    json!({
                        "campaignArn": CAMPAIGN_USER,
                        "userId": format!("user-{user}"),
                        "numResults": num_results,
                    }),
                )
                .await;
            let returned = ids(&response, "itemList", "itemId");
            assert_eq!(
                returned.len(),
                num_results as usize,
                "case {case}: expected {num_results} items, got {returned:?}"
            );
            assert_descending_and_bounded(&case, &scores(&response, "itemList"));
            if num_results == 5 {
                orderings.push(returned);
            }
        }
    }
    let first = orderings.first().cloned().unwrap_or_default();
    assert!(
        orderings.iter().any(|ordering| ordering != &first),
        "scores are personalized, so the 16 users must not all see one ordering; every one \
         returned {first:?}"
    );
}

#[tokio::test]
async fn context_changes_the_ordering_and_an_equal_context_does_not() {
    let emulator = seeded();
    let request = |context: Value| {
        json!({
            "campaignArn": CAMPAIGN_USER,
            "userId": "user-7",
            "numResults": 10,
            "context": context,
        })
    };

    let plain = emulator
        .ok(
            RECOMMENDATIONS,
            json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-7", "numResults": 10 }),
        )
        .await;
    let mobile = emulator
        .ok(RECOMMENDATIONS, request(json!({ "DEVICE": "mobile" })))
        .await;
    let desktop = emulator
        .ok(RECOMMENDATIONS, request(json!({ "DEVICE": "desktop" })))
        .await;
    let mobile_again = emulator
        .ok(RECOMMENDATIONS, request(json!({ "DEVICE": "mobile" })))
        .await;

    let plain_ids = ids(&plain, "itemList", "itemId");
    let mobile_ids = ids(&mobile, "itemList", "itemId");
    let desktop_ids = ids(&desktop, "itemList", "itemId");
    assert_eq!(
        mobile_ids,
        ids(&mobile_again, "itemList", "itemId"),
        "an equal context must produce an equal ordering, got {mobile_ids:?}"
    );
    assert_ne!(
        mobile_ids, desktop_ids,
        "contextual metadata is part of the score, so two contexts must not share one \
         ordering; both returned {mobile_ids:?}"
    );
    assert_ne!(
        mobile_ids, plain_ids,
        "a request with context must not score like one without it; both returned \
         {mobile_ids:?}"
    );
}

#[tokio::test]
async fn metadata_columns_are_served_from_the_stored_item_attributes() {
    let emulator = seeded();
    let catalog = items(CATALOG_SIZE);

    // The rows differ in which stored columns are asked for; the expected value of
    // every column is read back out of the seeded catalog.
    let rows: [(&str, Vec<&str>); 3] = [
        ("one column", vec![COLUMN_GENRE]),
        ("the other column", vec![COLUMN_YEAR]),
        ("both columns", vec![COLUMN_GENRE, COLUMN_YEAR]),
    ];

    for (case, columns) in rows {
        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-3",
                    "numResults": 8,
                    "metadataColumns": { "ITEMS": columns },
                }),
            )
            .await;
        for entry in entries(&response, "itemList") {
            let item_id = entry
                .get("itemId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let stored = catalog
                .iter()
                .find(|item| item.item_id == item_id)
                .unwrap_or_else(|| panic!("case {case}: {item_id} is not in the stored catalog"));
            let metadata = entry
                .get("metadata")
                .and_then(Value::as_object)
                .unwrap_or_else(|| panic!("case {case}: {entry} carries no metadata"));
            assert_eq!(
                metadata.len(),
                columns.len(),
                "case {case}: expected {} metadata columns, got {metadata:?}",
                columns.len()
            );
            for column in &columns {
                let expected = stored.attributes.get(*column).map(String::as_str);
                let actual = metadata.get(*column).and_then(Value::as_str);
                assert_eq!(
                    actual, expected,
                    "case {case}: {item_id}.{column} must come from the stored Items dataset, \
                     got {actual:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn metadata_requests_outside_the_documented_rules_are_refused() {
    let emulator = seeded();
    let too_many: Vec<String> = (0..=MAX_METADATA_COLUMNS)
        .map(|index| format!("COLUMN_{index}"))
        .collect();

    let rows: [(&str, Value); 5] = [
        (
            "a map key other than ITEMS",
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-1",
                "metadataColumns": { "INTERACTIONS": [COLUMN_GENRE] },
            }),
        ),
        (
            "more than the documented maximum of columns",
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-1",
                "metadataColumns": { "ITEMS": too_many },
            }),
        ),
        (
            "a column the Items dataset does not carry",
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-1",
                "metadataColumns": { "ITEMS": ["DIRECTOR"] },
            }),
        ),
        (
            "a campaign without metadata in recommendations enabled",
            json!({
                "campaignArn": CAMPAIGN_RELATED,
                "itemId": "item-3",
                "metadataColumns": { "ITEMS": [COLUMN_GENRE] },
            }),
        ),
        (
            "numResults past the lower maximum that applies with metadata",
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-1",
                "numResults": MAX_NUM_RESULTS_WITH_METADATA + 1,
                "metadataColumns": { "ITEMS": [COLUMN_GENRE] },
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, code) = emulator.err(RECOMMENDATIONS, body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn an_unregistered_arn_is_invalid_input_for_every_operation() {
    let emulator = seeded();

    let rows: [(&str, &str, Value); 7] = [
        (
            "GetRecommendations, unknown campaign",
            RECOMMENDATIONS,
            json!({ "campaignArn": UNREGISTERED_CAMPAIGN, "userId": "user-1", "numResults": 3 }),
        ),
        (
            "GetRecommendations, unknown recommender",
            RECOMMENDATIONS,
            json!({
                "recommenderArn": "arn:aws:personalize:us-east-1:000000000000:recommender/nope",
                "userId": "user-1",
            }),
        ),
        (
            "GetRecommendations, unknown filter",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-1",
                "filterArn": UNREGISTERED_FILTER,
            }),
        ),
        (
            "GetRecommendations, unknown promotion filter",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-1",
                "promotions": [{
                    "name": "promo1",
                    "percentPromotedItems": 50,
                    "filterArn": UNREGISTERED_FILTER,
                }],
            }),
        ),
        (
            "GetPersonalizedRanking, unknown campaign",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": UNREGISTERED_CAMPAIGN,
                "userId": "user-1",
                "inputList": ["item-1"],
            }),
        ),
        (
            "GetPersonalizedRanking, unknown filter",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-1",
                "inputList": ["item-1"],
                "filterArn": UNREGISTERED_FILTER,
            }),
        ),
        (
            "GetActionRecommendations, unknown campaign",
            ACTION_RECOMMENDATIONS,
            json!({ "campaignArn": UNREGISTERED_CAMPAIGN, "userId": "user-1", "numResults": 3 }),
        ),
    ];

    for (case, route, body) in rows {
        let (status, code) = emulator.err(route, body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn an_arn_outside_the_modelled_pattern_is_refused_with_400() {
    let emulator = seeded();

    // `Arn` is modelled `pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$`, so the
    // boundary refuses each of these before any handler runs.
    let rows: [(&str, &str); 4] = [
        ("not an ARN at all", "not-an-arn"),
        ("an ARN of another service", "arn:aws:s3:::bucket/key"),
        ("an uppercase partition", "arn:AWS:personalize:us-east-1:1:campaign/x"),
        ("no resource part", "arn:aws:personalize:us-east-1:000000000000:"),
    ];

    for (case, arn) in rows {
        let (status, _) = emulator
            .err(
                RECOMMENDATIONS,
                json!({ "campaignArn": arn, "userId": "user-1" }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: {arn} violates the modelled Arn pattern, expected HTTP 400, got {status}"
        );
    }
}

#[tokio::test]
async fn a_filter_expression_removes_exactly_the_items_it_names() {
    let emulator = seeded();
    let catalog = items(CATALOG_SIZE);

    // Each row names a registered expression, the `filterValues` the request binds,
    // and the predicate that expression means. The expected id set is computed from
    // the seeded catalog with that predicate, never written out.
    type Retains = fn(&Item) -> bool;
    let rows: [(&str, &str, Value, Retains); 6] = [
        (
            "INCLUDE with a bound placeholder",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "Comedy" }),
            |item| genre(item) == "Comedy",
        ),
        (
            "INCLUDE with two values for one placeholder",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "Comedy,Drama" }),
            |item| matches!(genre(item), "Comedy" | "Drama"),
        ),
        (
            "EXCLUDE with a literal",
            FILTER_EXCLUDE_COMEDY,
            json!({}),
            |item| genre(item) != "Comedy",
        ),
        (
            "INCLUDE with a numeric comparison",
            FILTER_RECENT,
            json!({}),
            |item| year(item) >= 2010,
        ),
        (
            "two clauses joined by OR",
            FILTER_COMEDY_OR_DRAMA,
            json!({}),
            |item| matches!(genre(item), "Comedy" | "Drama"),
        ),
        (
            "two clauses joined by AND",
            FILTER_RECENT_COMEDY,
            json!({}),
            |item| genre(item) == "Comedy" && year(item) >= 2010,
        ),
    ];

    for (case, filter_arn, filter_values, retains) in rows {
        let expected: Vec<String> = catalog
            .iter()
            .filter(|item| retains(item))
            .map(|item| item.item_id.clone())
            .collect();
        assert!(
            !expected.is_empty() && expected.len() < CATALOG_SIZE,
            "case {case}: the row must select a strict, non-empty subset, got {expected:?}"
        );
        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-5",
                    // More than the filter can leave, so the response size shows what
                    // the filter removed.
                    "numResults": CATALOG_SIZE,
                    "filterArn": filter_arn,
                    "filterValues": filter_values,
                }),
            )
            .await;
        let mut returned = ids(&response, "itemList", "itemId");
        // The filter decides WHICH items come back, never what one of them scores, so
        // a filtered response carries only the share of the certainty its surviving
        // items hold out of the whole scored catalog.
        assert_descending_and_bounded(case, &scores(&response, "itemList"));
        returned.sort();
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(
            returned, expected_sorted,
            "case {case}: the filter must leave exactly the items its expression names"
        );
    }
}

#[tokio::test]
async fn an_exclude_expression_with_no_filter_values_removes_nothing() {
    let emulator = seeded();

    // "For filters with expressions that use an EXCLUDE element to exclude items, you
    // can omit the filter-values. In this case, Amazon Personalize doesn't use that
    // portion of the expression to filter recommendations."
    let response = emulator
        .ok(
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-2",
                "numResults": CATALOG_SIZE,
                "filterArn": FILTER_EXCLUDE_GENRE,
            }),
        )
        .await;
    assert_eq!(
        ids(&response, "itemList", "itemId").len(),
        CATALOG_SIZE,
        "an EXCLUDE expression whose placeholder the request left unbound must remove \
         nothing, got {response}"
    );

    // The same placeholder, bound: now the expression does filter.
    let bound = emulator
        .ok(
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-2",
                "numResults": CATALOG_SIZE,
                "filterArn": FILTER_EXCLUDE_GENRE,
                "filterValues": { "GENRE": "Comedy" },
            }),
        )
        .await;
    let catalog = items(CATALOG_SIZE);
    let expected = catalog.iter().filter(|item| genre(item) != "Comedy").count();
    assert_eq!(
        ids(&bound, "itemList", "itemId").len(),
        expected,
        "a bound EXCLUDE placeholder must remove the items it names, got {bound}"
    );
}

#[tokio::test]
async fn an_include_expression_with_an_unbound_placeholder_is_refused() {
    let emulator = seeded();

    // "For filter expressions that use an INCLUDE element to include items, you must
    // provide values for all parameters that are defined in the expression."
    let rows: [(&str, Value); 2] = [
        ("no filterValues at all", json!({})),
        (
            "filterValues binding another parameter",
            json!({ "YEAR": "2010" }),
        ),
    ];
    for (case, filter_values) in rows {
        let mut body = json!({
            "campaignArn": CAMPAIGN_USER,
            "userId": "user-1",
            "filterArn": FILTER_INCLUDE_GENRE,
        });
        if filter_values != json!({}) {
            body["filterValues"] = filter_values;
        }
        let (status, code) = emulator.err(RECOMMENDATIONS, body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_promotion_takes_its_documented_share_of_the_returned_items() {
    let emulator = seeded();

    // "The percentage of recommended items to apply the promotion to", rounded up.
    let rows: [(&str, usize, i32, usize); 5] = [
        ("half of ten", 10, 50, 5),
        ("all of ten", 10, 100, 10),
        ("a quarter of four", 4, 25, 1),
        ("a tenth of eight, rounded up", 8, 10, 1),
        ("three tenths of ten", 10, 30, 3),
    ];

    for (case, num_results, percent, expected_promoted) in rows {
        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-9",
                    "numResults": num_results,
                    "promotions": [{
                        "name": "promo1",
                        "percentPromotedItems": percent,
                        "filterArn": FILTER_INCLUDE_GENRE,
                        "filterValues": { "GENRE": "Comedy" },
                    }],
                }),
            )
            .await;
        let returned = entries(&response, "itemList");
        assert_eq!(
            returned.len(),
            num_results,
            "case {case}: expected {num_results} items, got {returned:?}"
        );
        assert_descending_and_bounded(case, &scores(&response, "itemList"));

        let catalog = items(CATALOG_SIZE);
        let promoted: Vec<&Value> = returned
            .iter()
            .filter(|entry| entry.get("promotionName").is_some())
            .collect();
        assert_eq!(
            promoted.len(),
            expected_promoted,
            "case {case}: expected {expected_promoted} promoted items, got {promoted:?}"
        );
        for entry in &promoted {
            assert_eq!(
                entry.get("promotionName").and_then(Value::as_str),
                Some("promo1"),
                "case {case}: a promoted item must carry the promotion's name, got {entry}"
            );
            assert_eq!(
                entry.get("reason").and_then(Value::as_array),
                Some(&vec![json!(registry::REASON_PROMOTED_ITEM)]),
                "case {case}: a promoted item must carry the documented reason, got {entry}"
            );
            let item_id = entry
                .get("itemId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let stored = catalog
                .iter()
                .find(|item| item.item_id == item_id)
                .unwrap_or_else(|| panic!("case {case}: {item_id} is not in the stored catalog"));
            assert_eq!(
                genre(stored),
                "Comedy",
                "case {case}: a promoted item must satisfy the promotion's filter, got \
                 {item_id} with GENRE {}",
                genre(stored)
            );
        }
    }
}

#[tokio::test]
async fn a_promotion_that_defines_no_subset_is_refused() {
    let emulator = seeded();

    let rows: [(&str, Value); 3] = [
        (
            "no name",
            json!({ "percentPromotedItems": 50, "filterArn": FILTER_EXCLUDE_COMEDY }),
        ),
        (
            "no percentPromotedItems",
            json!({ "name": "promo1", "filterArn": FILTER_EXCLUDE_COMEDY }),
        ),
        (
            "no filterArn",
            json!({ "name": "promo1", "percentPromotedItems": 50 }),
        ),
    ];

    for (case, promotion) in rows {
        let (status, code) = emulator
            .err(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-1",
                    "promotions": [promotion],
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn related_items_recommends_from_the_catalog_but_never_the_queried_item() {
    let emulator = seeded();

    // "RELATED_ITEMS - itemId required, userId not used".
    for item_id in ["item-0", "item-7", "item-39"] {
        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_RELATED,
                    "itemId": item_id,
                    "numResults": CATALOG_SIZE,
                }),
            )
            .await;
        let returned = ids(&response, "itemList", "itemId");
        assert_eq!(
            returned.len(),
            CATALOG_SIZE - 1,
            "case {item_id}: the queried item is not one of its own related items, got \
             {} items",
            returned.len()
        );
        assert!(
            !returned.iter().any(|returned| returned == item_id),
            "case {item_id}: the queried item must not be recommended, got {returned:?}"
        );
        assert_sums_to_one(item_id, &scores(&response, "itemList"));
    }

    let (status, code) = emulator
        .err(
            RECOMMENDATIONS,
            json!({ "campaignArn": CAMPAIGN_RELATED, "userId": "user-1" }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidInputException"),
        "a RELATED_ITEMS campaign requires itemId, expected 400 InvalidInputException, got \
         {status} {code}"
    );
}

#[tokio::test]
async fn personalized_ranking_is_a_permutation_of_the_input_list() {
    let emulator = seeded();

    // The rows differ in cardinality, in whether the ids are in the training catalog,
    // and in whether they repeat.
    let rows: [(&str, Vec<&str>, usize); 6] = [
        ("one known item", vec!["item-5"], 1),
        ("three known items", vec!["item-5", "item-32", "item-7"], 3),
        (
            "known items out of catalog order",
            vec!["item-39", "item-0", "item-20", "item-11"],
            4,
        ),
        // An id the caller repeats names the same catalog entry, which the ranking
        // already reports, so the repeat adds no entry.
        ("a repeated id", vec!["item-5", "item-5", "item-9"], 2),
        (
            "an id the training dataset never held",
            vec!["item-5", "no-such-item", "item-9"],
            3,
        ),
        ("only unknown ids", vec!["nope-1", "nope-2"], 2),
    ];

    for (case, input_list, expected_len) in rows {
        let response = emulator
            .ok(
                PERSONALIZE_RANKING,
                json!({
                    "campaignArn": CAMPAIGN_RANKING,
                    "userId": "user-4",
                    "inputList": input_list,
                }),
            )
            .await;
        let returned = ids(&response, "personalizedRanking", "itemId");
        assert_eq!(
            returned.len(),
            expected_len,
            "case {case}: the ranking is a permutation of the distinct ids of inputList, \
             so it must hold {expected_len} entries, got {returned:?}"
        );
        let mut sorted_returned = returned.clone();
        sorted_returned.sort();
        let mut sorted_input: Vec<String> =
            input_list.iter().map(ToString::to_string).collect();
        sorted_input.sort();
        sorted_input.dedup();
        assert_eq!(
            sorted_returned, sorted_input,
            "case {case}: the ranking must hold exactly the distinct ids of inputList"
        );

        let returned_scores = scores(&response, "personalizedRanking");
        for pair in returned_scores.windows(2) {
            assert!(
                pair[0] >= pair[1],
                "case {case}: the ranking must not increase in score, got {returned_scores:?}"
            );
        }
        // An id the catalog never held is appended with no relevance score for the
        // user, so it sorts last.
        let catalog = items(CATALOG_SIZE);
        for (position, item_id) in returned.iter().enumerate() {
            let known = catalog.iter().any(|item| &item.item_id == item_id);
            if !known {
                assert_eq!(
                    returned_scores[position], 0.0,
                    "case {case}: {item_id} is not in the training dataset, so it carries no \
                     relevance score, got {}",
                    returned_scores[position]
                );
                let known_after = returned[position + 1..]
                    .iter()
                    .any(|later| catalog.iter().any(|item| &item.item_id == later));
                assert!(
                    !known_after,
                    "case {case}: {item_id} must be appended after every known item, got \
                     {returned:?}"
                );
            }
        }
        recommendation_id(case, &response);
    }
}

#[tokio::test]
async fn personalized_ranking_drops_the_items_its_filter_excludes() {
    let emulator = seeded();
    let catalog = items(CATALOG_SIZE);
    let input_list: Vec<String> = catalog
        .iter()
        .take(8)
        .map(|item| item.item_id.clone())
        .collect();

    let response = emulator
        .ok(
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-4",
                "inputList": input_list,
                "filterArn": FILTER_EXCLUDE_COMEDY,
            }),
        )
        .await;

    let expected: Vec<String> = catalog
        .iter()
        .take(8)
        .filter(|item| genre(item) != "Comedy")
        .map(|item| item.item_id.clone())
        .collect();
    let mut returned = ids(&response, "personalizedRanking", "itemId");
    returned.sort();
    let mut expected_sorted = expected.clone();
    expected_sorted.sort();
    assert_eq!(
        returned, expected_sorted,
        "a filter the request names must remove the items it excludes from the ranking"
    );
}

#[tokio::test]
async fn action_recommendations_come_from_the_stored_action_catalog() {
    let emulator = seeded();
    let catalog = actions(ACTION_CATALOG_SIZE);

    // "The default is 5. The maximum is 100."
    let rows: [(&str, Option<i32>, usize); 4] = [
        ("absent numResults", None, 5),
        ("numResults 1", Some(1), 1),
        ("the whole catalog", Some(ACTION_CATALOG_SIZE as i32), ACTION_CATALOG_SIZE),
        ("more than the catalog holds", Some(100), ACTION_CATALOG_SIZE),
    ];

    for (case, num_results, expected) in rows {
        let mut body = json!({ "campaignArn": CAMPAIGN_ACTIONS, "userId": "user-1" });
        if let Some(num_results) = num_results {
            body["numResults"] = json!(num_results);
        }
        let response = emulator.ok(ACTION_RECOMMENDATIONS, body).await;
        let returned = ids(&response, "actionList", "actionId");
        assert_eq!(
            returned.len(),
            expected,
            "case {case}: expected {expected} actions, got {returned:?}"
        );
        for action_id in &returned {
            assert!(
                catalog.iter().any(|action| &action.action_id == action_id),
                "case {case}: {action_id} is not in the stored action catalog"
            );
        }
        let returned_scores = scores(&response, "actionList");
        if expected == ACTION_CATALOG_SIZE {
            assert_sums_to_one(case, &returned_scores);
        } else {
            assert_descending_and_bounded(case, &returned_scores);
        }
        recommendation_id(case, &response);
    }
}

/// Assert that every entry of `reported` carries the score `reference` — the score the
/// same request computed over the whole candidate set — gives it.
fn assert_scores_match_the_candidate_set(
    case: &str,
    reported: &BTreeMap<String, f64>,
    reference: &BTreeMap<String, f64>,
) {
    for (id, score) in reported {
        let expected = reference.get(id).unwrap_or_else(|| {
            panic!("case {case}: {id} is not one of the scored candidates {reference:?}")
        });
        assert!(
            (score - expected).abs() < 1e-12,
            "case {case}: {id} scores {expected} over the whole candidate set, got {score}"
        );
    }
}

#[tokio::test]
async fn an_action_score_does_not_change_with_the_requested_number_of_results() {
    let emulator = seeded();
    let catalog = actions(ACTION_CATALOG_SIZE);
    // `INCLUDE ActionID WHERE Actions.CHANNEL IN ("App")` keeps exactly these.
    let app_candidates = catalog
        .iter()
        .filter(|action| action.attributes.get("CHANNEL").map(String::as_str) == Some("App"))
        .count();

    // A group is one CANDIDATE SET: the request members that decide which actions are
    // scored at all (the user the prediction is for, and the filter the request
    // names). Within a group the rows differ only in how many results the caller asks
    // back — which bounds the list's length and must leave every reported score, and
    // the identity of the actions reported, exactly as the whole-candidate-set call
    // reports them.
    let groups: [(&str, &str, Option<&str>, usize); 4] = [
        ("no filter, user-1", "user-1", None, ACTION_CATALOG_SIZE),
        ("no filter, user-12", "user-12", None, ACTION_CATALOG_SIZE),
        ("no filter, user-with-a-long-id", "user-with-a-long-id", None, ACTION_CATALOG_SIZE),
        (
            "CHANNEL IN (App), user-1",
            "user-1",
            Some(FILTER_APP_ACTIONS),
            app_candidates,
        ),
    ];
    // "The number of results to return. The default is 5. The maximum is 100."
    let rows: [Option<i32>; 6] = [
        Some(1),
        Some(2),
        Some(3),
        None,
        Some(ACTION_CATALOG_SIZE as i32),
        Some(MAX_NUM_ACTION_RESULTS as i32),
    ];

    for (group, user_id, filter_arn, candidates) in groups {
        let request = |num_results: Option<i32>| {
            let mut body = json!({ "campaignArn": CAMPAIGN_ACTIONS, "userId": user_id });
            if let Some(filter_arn) = filter_arn {
                body["filterArn"] = json!(filter_arn);
            }
            if let Some(num_results) = num_results {
                body["numResults"] = json!(num_results);
            }
            body
        };

        // The reference: a request for more actions than the candidate set holds, so
        // every candidate is reported and the whole certainty with it.
        let whole = emulator
            .ok(
                ACTION_RECOMMENDATIONS,
                request(Some(MAX_NUM_ACTION_RESULTS as i32)),
            )
            .await;
        let reference = scores_by_id(&whole, "actionList", "actionId");
        assert_eq!(
            reference.len(),
            candidates,
            "case {group}: expected {candidates} scored candidates, got {reference:?}"
        );
        if filter_arn.is_none() {
            assert_sums_to_one(group, &scores(&whole, "actionList"));
        } else {
            // A filtered response reports only part of the scored catalog, so it
            // carries only part of the certainty.
            assert_descending_and_bounded(group, &scores(&whole, "actionList"));
        }

        for num_results in rows {
            let case = format!("{group} numResults {num_results:?}");
            let response = emulator.ok(ACTION_RECOMMENDATIONS, request(num_results)).await;
            let reported = scores_by_id(&response, "actionList", "actionId");
            let asked = num_results
                .map(|num_results| num_results as usize)
                .unwrap_or(registry::DEFAULT_NUM_ACTION_RESULTS);
            let expected_len = asked.min(candidates);
            assert_eq!(
                reported.len(),
                expected_len,
                "case {case}: expected {expected_len} actions, got {reported:?}"
            );
            assert_scores_match_the_candidate_set(&case, &reported, &reference);
            assert_descending_and_bounded(&case, &scores(&response, "actionList"));

            let total: f64 = reported.values().sum();
            if expected_len < candidates || filter_arn.is_some() {
                // The certainty the unreported actions carry stays with them: a short
                // or filtered response reports a share of 1.0, not 1.0 renormalized.
                assert!(
                    total < 1.0 - 1e-9,
                    "case {case}: a response reporting {expected_len} of {candidates} \
                     candidates must not carry the whole certainty, got {total}"
                );
            } else {
                assert!(
                    (total - 1.0).abs() < 1e-9,
                    "case {case}: a response reporting every candidate must sum to 1.0, \
                     got {total}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_item_score_does_not_change_with_the_requested_number_of_results() {
    let emulator = seeded();
    let catalog = items(CATALOG_SIZE);
    // `INCLUDE ItemID WHERE Items.YEAR >= 2010` keeps exactly these.
    let recent_candidates = catalog.iter().filter(|item| year(item) >= 2010).count();

    // One group per candidate set, across both resource kinds and both personalization
    // inputs `GetRecommendations` serves; a `RELATED_ITEMS` candidate set excludes the
    // queried item. The flag says whether the group's reported items are the WHOLE
    // scored catalog — a filtered group reports only part of it, and so carries only
    // part of the certainty.
    let groups: [(&str, Value, usize, bool); 4] = [
        (
            "campaign, no filter",
            json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-3" }),
            CATALOG_SIZE,
            true,
        ),
        (
            "campaign, YEAR >= 2010",
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-3",
                "filterArn": FILTER_RECENT,
            }),
            recent_candidates,
            false,
        ),
        (
            "recommender, no filter",
            json!({ "recommenderArn": RECOMMENDER_TOP_PICKS, "userId": "user-8" }),
            CATALOG_SIZE,
            true,
        ),
        (
            "related items",
            json!({ "campaignArn": CAMPAIGN_RELATED, "itemId": "item-4" }),
            CATALOG_SIZE - 1,
            true,
        ),
    ];
    // "The number of results to return. The default is 25."
    let rows: [Option<i32>; 5] = [Some(1), Some(3), Some(10), None, Some(CATALOG_SIZE as i32)];

    for (group, base, candidates, whole_catalog) in groups {
        let request = |num_results: Option<i32>| {
            let mut body = base.clone();
            if let Some(num_results) = num_results {
                body["numResults"] = json!(num_results);
            }
            body
        };

        let whole = emulator
            .ok(RECOMMENDATIONS, request(Some(CATALOG_SIZE as i32)))
            .await;
        let reference = scores_by_id(&whole, "itemList", "itemId");
        assert_eq!(
            reference.len(),
            candidates,
            "case {group}: expected {candidates} scored candidates, got {reference:?}"
        );
        if whole_catalog {
            assert_sums_to_one(group, &scores(&whole, "itemList"));
        } else {
            assert_descending_and_bounded(group, &scores(&whole, "itemList"));
        }

        for num_results in rows {
            let case = format!("{group} numResults {num_results:?}");
            let response = emulator.ok(RECOMMENDATIONS, request(num_results)).await;
            let reported = scores_by_id(&response, "itemList", "itemId");
            let asked = num_results
                .map(|num_results| num_results as usize)
                .unwrap_or(registry::DEFAULT_NUM_RESULTS);
            let expected_len = asked.min(candidates);
            assert_eq!(
                reported.len(),
                expected_len,
                "case {case}: expected {expected_len} items, got {reported:?}"
            );
            assert_scores_match_the_candidate_set(&case, &reported, &reference);
            assert_descending_and_bounded(&case, &scores(&response, "itemList"));

            let total: f64 = reported.values().sum();
            if expected_len < candidates {
                assert!(
                    total < 1.0 - 1e-9,
                    "case {case}: a response reporting {expected_len} of {candidates} \
                     candidates must not carry the whole certainty, got {total}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_promotion_does_not_change_what_an_item_scores() {
    let emulator = seeded();

    // A promotion decides WHICH candidates fill the returned slots, not what any of
    // them scores, so every item a promoted response reports carries the score the
    // whole-candidate-set response gives it.
    let whole = emulator
        .ok(
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-9",
                "numResults": CATALOG_SIZE,
            }),
        )
        .await;
    let reference = scores_by_id(&whole, "itemList", "itemId");

    for percent in [10, 50, 100] {
        let case = format!("percentPromotedItems {percent}");
        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-9",
                    "numResults": 10,
                    "promotions": [{
                        "name": "promo1",
                        "percentPromotedItems": percent,
                        "filterArn": FILTER_INCLUDE_GENRE,
                        "filterValues": { "GENRE": "Comedy" },
                    }],
                }),
            )
            .await;
        let reported = scores_by_id(&response, "itemList", "itemId");
        assert_eq!(
            reported.len(),
            10,
            "case {case}: expected 10 items, got {reported:?}"
        );
        assert_scores_match_the_candidate_set(&case, &reported, &reference);
    }
}

#[tokio::test]
async fn an_action_filter_removes_the_actions_it_names() {
    let emulator = seeded();
    let catalog = actions(ACTION_CATALOG_SIZE);

    let response = emulator
        .ok(
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": "user-1",
                "numResults": ACTION_CATALOG_SIZE,
                "filterArn": FILTER_APP_ACTIONS,
            }),
        )
        .await;

    let expected: Vec<String> = catalog
        .iter()
        .filter(|action| {
            action.attributes.get("CHANNEL").map(String::as_str) == Some("App")
        })
        .map(|action| action.action_id.clone())
        .collect();
    let mut returned = ids(&response, "actionList", "actionId");
    returned.sort();
    let mut expected_sorted = expected.clone();
    expected_sorted.sort();
    assert_eq!(
        returned, expected_sorted,
        "an Actions filter must leave exactly the actions its expression names"
    );

    // An Items filter cannot be evaluated against an Actions catalog.
    let (status, code) = emulator
        .err(
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": "user-1",
                "filterArn": FILTER_EXCLUDE_COMEDY,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidInputException"),
        "an Items filter does not describe actions, expected 400 InvalidInputException, got \
         {status} {code}"
    );
}

#[tokio::test]
async fn an_operation_refuses_a_campaign_another_operation_serves() {
    let emulator = seeded();

    // Each recipe type is served by exactly one of the three operations.
    let rows: [(&str, &str, Value); 4] = [
        (
            "GetRecommendations against a PERSONALIZED_RANKING campaign",
            RECOMMENDATIONS,
            json!({ "campaignArn": CAMPAIGN_RANKING, "userId": "user-1" }),
        ),
        (
            "GetRecommendations against a PERSONALIZED_ACTIONS campaign",
            RECOMMENDATIONS,
            json!({ "campaignArn": CAMPAIGN_ACTIONS, "userId": "user-1" }),
        ),
        (
            "GetPersonalizedRanking against a USER_PERSONALIZATION campaign",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-1",
                "inputList": ["item-1"],
            }),
        ),
        (
            "GetActionRecommendations against a USER_PERSONALIZATION campaign",
            ACTION_RECOMMENDATIONS,
            json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1" }),
        ),
    ];

    for (case, route, body) in rows {
        let (status, code) = emulator.err(route, body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn naming_both_a_campaign_and_a_recommender_or_neither_is_refused() {
    let emulator = seeded();

    let rows: [(&str, Value); 2] = [
        (
            "both",
            json!({
                "campaignArn": CAMPAIGN_USER,
                "recommenderArn": RECOMMENDER_TOP_PICKS,
                "userId": "user-1",
            }),
        ),
        ("neither", json!({ "userId": "user-1" })),
    ];

    for (case, body) in rows {
        let (status, code) = emulator.err(RECOMMENDATIONS, body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException, got {status} {code}"
        );
    }
}

#[tokio::test]
async fn a_recommender_requires_the_input_its_use_case_documents() {
    let emulator = seeded();
    let engine = &*emulator.engine;

    // Every entry of the catalog in `state::RECOMMENDER_USE_CASES`, not only the one
    // the issue cited: each use case is registered as its own recommender, then
    // called with nothing, with `userId`, and with `itemId`.
    for (index, (use_case, required)) in RECOMMENDER_USE_CASES.iter().enumerate() {
        let arn =
            format!("arn:aws:personalize:us-east-1:000000000000:recommender/use-case-{index}");
        assert!(
            registry::register_recommender(
                engine,
                Recommender::new(&arn, format!("use-case-{index}"), *use_case, items(8))
            ),
            "case {use_case}: the recommender seam must register a documented use case"
        );

        let rows: [(&str, Value, bool); 3] = [
            (
                "no personalization input",
                json!({ "recommenderArn": arn, "numResults": 3 }),
                *required == RequiredInput::Nothing,
            ),
            (
                "userId",
                json!({ "recommenderArn": arn, "userId": "user-1", "numResults": 3 }),
                *required != RequiredInput::Item,
            ),
            (
                "itemId",
                json!({ "recommenderArn": arn, "itemId": "item-1", "numResults": 3 }),
                *required != RequiredInput::User,
            ),
        ];
        for (shape, body, expected_ok) in rows {
            let (status, parsed) = emulator.call(RECOMMENDATIONS, &body).await;
            if expected_ok {
                assert_eq!(
                    status, 200,
                    "case {use_case} with {shape}: the use case requires {required:?}, so this \
                     request must succeed; got {status} with {parsed}"
                );
                assert_eq!(
                    ids(&parsed, "itemList", "itemId").len(),
                    3,
                    "case {use_case} with {shape}: expected 3 items, got {parsed}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&parsed).as_str()),
                    (400, "InvalidInputException"),
                    "case {use_case} with {shape}: the use case requires {required:?}, so this \
                     request must be refused; got {status} with {parsed}"
                );
            }
        }
    }

    assert!(
        !registry::register_recommender(
            engine,
            Recommender::new(
                "arn:aws:personalize:us-east-1:000000000000:recommender/unknown-use-case",
                "unknown",
                "No such use case",
                items(4)
            )
        ),
        "the seam must refuse a use case the control plane would never create"
    );
}

#[tokio::test]
async fn num_results_past_the_documented_maximum_is_refused() {
    let emulator = seeded();

    let rows: [(&str, &str, usize, u16); 4] = [
        (
            "GetRecommendations at its maximum",
            RECOMMENDATIONS,
            MAX_NUM_RESULTS,
            200,
        ),
        (
            "GetRecommendations past its maximum",
            RECOMMENDATIONS,
            MAX_NUM_RESULTS + 1,
            400,
        ),
        (
            "GetActionRecommendations at its maximum",
            ACTION_RECOMMENDATIONS,
            MAX_NUM_ACTION_RESULTS,
            200,
        ),
        (
            "GetActionRecommendations past its maximum",
            ACTION_RECOMMENDATIONS,
            MAX_NUM_ACTION_RESULTS + 1,
            400,
        ),
    ];

    for (case, route, num_results, expected) in rows {
        let campaign = if route == ACTION_RECOMMENDATIONS {
            CAMPAIGN_ACTIONS
        } else {
            CAMPAIGN_USER
        };
        let (status, parsed) = emulator
            .call(
                route,
                &json!({
                    "campaignArn": campaign,
                    "userId": "user-1",
                    "numResults": num_results,
                }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {case}: expected HTTP {expected}, got {status} with {parsed}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&parsed),
                "InvalidInputException",
                "case {case}: expected InvalidInputException, got {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn every_call_mints_its_own_recommendation_id_and_the_registry_stays_bounded() {
    let emulator = seeded();
    let calls = MAX_RETAINED_RECOMMENDATION_IDS + 6;

    let mut minted: Vec<String> = Vec::new();
    for call in 0..calls {
        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1", "numResults": 1 }),
            )
            .await;
        minted.push(recommendation_id(&format!("call {call}"), &response));
    }

    let mut unique = minted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        minted.len(),
        "every call mints a fresh recommendationId, got {} distinct ids from {} calls",
        unique.len(),
        minted.len()
    );

    let (retained, issued) = registry::issued_recommendation_ids(&*emulator.engine)
        .expect("this service's registry");
    assert_eq!(
        issued, calls as u64,
        "the registry must have counted every issued id, got {issued}"
    );
    assert_eq!(
        retained.len(),
        MAX_RETAINED_RECOMMENDATION_IDS,
        "the retained window is bounded at {MAX_RETAINED_RECOMMENDATION_IDS}, got {}",
        retained.len()
    );
    assert_eq!(
        retained.last(),
        minted.last(),
        "the newest retained id must be the one the last call returned"
    );
}

#[test]
fn the_filter_seam_refuses_an_expression_the_runtime_cannot_apply() {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    // A filter's expression is validated where the filter is created, so a stored
    // filter is always one the runtime can apply.
    let accepted = [
        "INCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\")",
        "EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE)",
        "INCLUDE ItemID WHERE Items.YEAR >= 2010 AND Items.GENRE NOT IN (\"Drama\")",
        "INCLUDE ActionID WHERE Actions.CHANNEL IN (\"App\", \"Email\")",
    ];
    for (index, expression) in accepted.iter().enumerate() {
        let arn = format!("arn:aws:personalize:us-east-1:000000000000:filter/ok-{index}");
        assert_eq!(
            registry::register_filter(engine, &arn, *expression),
            Ok(true),
            "case {expression:?}: a documented expression must be accepted"
        );
    }

    let refused = [
        ("no action", "ItemID WHERE Items.GENRE IN (\"Comedy\")"),
        ("no WHERE", "INCLUDE ItemID Items.GENRE IN (\"Comedy\")"),
        ("no predicate", "INCLUDE ItemID WHERE Items.GENRE"),
        ("an unqualified column", "INCLUDE ItemID WHERE GENRE IN (\"Comedy\")"),
        (
            "a dataset this emulator does not store",
            "INCLUDE ItemID WHERE Interactions.EVENT_TYPE IN (\"click\")",
        ),
        ("an empty value set", "INCLUDE ItemID WHERE Items.GENRE IN ()"),
        (
            "an unterminated string",
            "INCLUDE ItemID WHERE Items.GENRE IN (\"Comedy)",
        ),
        (
            "a dangling combinator",
            "INCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\") AND",
        ),
    ];
    for (case, expression) in refused {
        let arn = format!("arn:aws:personalize:us-east-1:000000000000:filter/bad-{case}");
        let outcome = registry::register_filter(engine, &arn, expression);
        assert!(
            outcome.is_err(),
            "case {case}: {expression:?} must be refused, got {outcome:?}"
        );
        assert!(
            registry::stored_filter(engine, &arn).is_none(),
            "case {case}: a refused expression must store no filter"
        );
    }

    // A resource is created once: re-registering an ARN changes nothing.
    let arn = "arn:aws:personalize:us-east-1:000000000000:filter/ok-0";
    assert_eq!(
        registry::register_filter(engine, arn, "EXCLUDE ItemID WHERE Items.GENRE IN ($G)"),
        Ok(false),
        "re-registering an ARN must change nothing"
    );
    let stored = registry::stored_filter(engine, arn).expect("the first registration stands");
    assert_eq!(
        stored.expression, accepted[0],
        "the first registration must stand, got {stored:?}"
    );
    assert_eq!(
        stored.referenced_parameters(),
        Vec::<String>::new(),
        "the stored expression references no placeholder, got {stored:?}"
    );
}

#[tokio::test]
async fn an_empty_registry_recommends_nothing_and_invents_no_campaign() {
    let emulator = Emulator::new();

    let (status, code) = emulator
        .err(
            RECOMMENDATIONS,
            json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1", "numResults": 10 }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidInputException"),
        "a campaign nobody created resolves to nothing, expected 400 InvalidInputException, \
         got {status} {code}"
    );

    // A campaign with an EMPTY catalog is a different thing from one that does not
    // exist: it is served, and it recommends nothing.
    assert!(
        registry::register_campaign(
            &*emulator.engine,
            Campaign::new(
                CAMPAIGN_USER,
                "empty",
                RecipeType::UserPersonalization,
                Vec::new()
            )
        ),
        "the campaign seam must register a campaign with an empty catalog"
    );
    let response = emulator
        .ok(
            RECOMMENDATIONS,
            json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1", "numResults": 10 }),
        )
        .await;
    assert!(
        ids(&response, "itemList", "itemId").is_empty(),
        "an empty catalog can recommend nothing, got {response}"
    );
    recommendation_id("empty catalog", &response);
}
