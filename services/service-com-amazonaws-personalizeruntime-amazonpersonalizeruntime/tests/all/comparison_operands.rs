// SPDX-License-Identifier: CC0-1.0

//! What a filter expression's comparison operators compare.
//!
//! `IN`, `NOT IN`, `=` and `<>` test EQUALITY of the value(s) an entry stores for the
//! clause's column, whether that metadata is categorical or numerical: a clause naming
//! a string selects the entries that hold it, so an `INCLUDE` expression keeps them and
//! an `EXCLUDE` expression removes them, and two spellings of one number are one value.
//! The ORDERING operators (`<`, `<=`, `>`, `>=`) are defined over numerical metadata
//! only, so an operand naming no number leaves the clause
//! unapplicable, and an unapplicable clause is REFUSED where it becomes known — where
//! the filter is created for a literal written into the expression, and where the
//! request is served for a `$placeholder` the request binds — rather than applied as a
//! clause nothing can satisfy.
//!
//! The suites below drive that rule as TABLES whose rows differ in the operator, in the
//! FORM of the operand (a quoted literal, a bare literal, a parameter bound to a quoted,
//! bare or still-JSON-escaped value), in whether the column is categorical or numerical,
//! in whether the column holds one value or several, in the filter kind, and in the route
//! the request takes — all three runtime operations plus a promotion's own
//! `filterValues`. Every expected set is computed from the SEEDED catalog, so no row can
//! pass against a canned list.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Action, Campaign, Item, RecipeType,
};

use crate::support::{
    assert_descending_and_bounded, ids, scores, Emulator, ACTION_RECOMMENDATIONS, COLUMN_GENRE,
    COLUMN_YEAR, PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// The `USER_PERSONALIZATION` campaign the `GetRecommendations` rows read.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/compare-user";
/// The `PERSONALIZED_RANKING` campaign the `GetPersonalizedRanking` rows re-rank.
const CAMPAIGN_RANKING: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/compare-ranking";
/// The `PERSONALIZED_ACTIONS` campaign the `GetActionRecommendations` rows read.
const CAMPAIGN_ACTIONS: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/compare-actions";

/// `INCLUDE ItemID WHERE Items.GENRE = "Comedy"`.
const INCLUDE_LITERAL_GENRE: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-include-literal";
/// `EXCLUDE ItemID WHERE Items.GENRE = "Comedy"`.
const EXCLUDE_LITERAL_GENRE: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-exclude-literal";
/// `INCLUDE ItemID WHERE Items.GENRE = $GENRE`.
const INCLUDE_GENRE_EQUALS: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-include-equals";
/// `INCLUDE ItemID WHERE Items.GENRE <> $GENRE`.
const INCLUDE_GENRE_DIFFERS: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-include-differs";
/// `EXCLUDE ItemID WHERE Items.GENRE <> $GENRE`.
const EXCLUDE_GENRE_DIFFERS: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-exclude-differs";
/// `INCLUDE ItemID WHERE Items.YEAR = $YEAR`.
const INCLUDE_YEAR_EQUALS: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-include-year";
/// `INCLUDE ItemID WHERE Items.YEAR IN ($YEAR)`.
const INCLUDE_YEAR_IN: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-include-year-in";
/// `INCLUDE ItemID WHERE Items.YEAR >= $YEAR`.
const INCLUDE_FROM_YEAR: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-include-from-year";
/// `EXCLUDE ItemID WHERE Items.YEAR >= $YEAR`.
const EXCLUDE_FROM_YEAR: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-exclude-from-year";
/// `INCLUDE ActionID WHERE Actions.CHANNEL = $CHANNEL`.
const INCLUDE_CHANNEL_EQUALS: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-include-channel";
/// `EXCLUDE ActionID WHERE Actions.CHANNEL = $CHANNEL`.
const EXCLUDE_CHANNEL_EQUALS: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-exclude-channel";
/// `INCLUDE ActionID WHERE Actions.VALUE > $VALUE`.
const INCLUDE_ABOVE_VALUE: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/compare-include-above-value";

/// The categorical Actions column the action rows filter on.
const COLUMN_CHANNEL: &str = "CHANNEL";
/// The numerical Actions column the action comparison rows filter on.
const COLUMN_VALUE: &str = "VALUE";

/// The `GENRE` values the seeded Items catalog cycles through.
///
/// Test data, chosen so the rows can name every shape a categorical value takes: the
/// third entry holds SEVERAL values separated by `|` (the separator a Personalize Items
/// CSV uses within one column), and the fourth carries a space inside the value.
const GENRES: [&str; 5] = [
    "Comedy",
    "Drama",
    "Comedy|Documentary",
    "Science Fiction",
    "Action",
];

/// The `CHANNEL` values the seeded Actions catalog cycles through, one of them
/// multi-valued for the same reason.
const CHANNELS: [&str; 3] = ["App", "Email", "App|Web"];

/// How many entries the seeded Items catalog holds — a multiple of [`GENRES`], so every
/// genre shape is carried by more than one entry.
const CATALOG_SIZE: usize = 15;
/// How many entries the seeded Actions catalog holds.
const ACTION_CATALOG_SIZE: usize = 9;

/// The `YEAR` the seeded catalog's first entry carries; the column counts up by one per
/// entry, so each entry carries a distinct year and `>=` selects a known suffix.
const FIRST_YEAR: i64 = 2000;

/// The year the `=` rows name: one inside the seeded range, so exactly one entry
/// carries it.
const TARGET_YEAR: i64 = FIRST_YEAR + 3;

/// How far apart two neighbouring actions' `VALUE` columns sit.
const VALUE_STEP: i64 = 5;

/// The Items catalog these suites filter. `base_score` falls with the index, so the
/// stored popularity order is known to the test.
fn catalog() -> Vec<Item> {
    (0..CATALOG_SIZE)
        .map(|index| {
            Item::new(format!("item-{index}"), (CATALOG_SIZE - index) as f64)
                .with_attribute(COLUMN_GENRE, GENRES[index % GENRES.len()])
                .with_attribute(COLUMN_YEAR, (FIRST_YEAR + index as i64).to_string())
        })
        .collect()
}

/// The Actions catalog the action rows filter.
fn action_catalog() -> Vec<Action> {
    (0..ACTION_CATALOG_SIZE)
        .map(|index| {
            Action::new(
                format!("action-{index}"),
                (ACTION_CATALOG_SIZE - index) as f64,
            )
            .with_attribute(COLUMN_CHANNEL, CHANNELS[index % CHANNELS.len()])
            .with_attribute(COLUMN_VALUE, (index as i64 * VALUE_STEP).to_string())
        })
        .collect()
}

/// Whether a stored entry's `GENRE` column holds `wanted` — the column may carry several
/// values separated by `|`, and an equality test holds when any of them is the value.
fn holds_genre(item: &Item, wanted: &str) -> bool {
    item.attributes
        .get(COLUMN_GENRE)
        .is_some_and(|stored| stored.split('|').any(|held| held == wanted))
}

/// The stored `YEAR` of one entry, as a number.
fn year(item: &Item) -> i64 {
    item.attributes
        .get(COLUMN_YEAR)
        .and_then(|held| held.parse().ok())
        .unwrap_or_default()
}

/// Whether a stored action's `CHANNEL` column holds `wanted`.
fn on_channel(action: &Action, wanted: &str) -> bool {
    action
        .attributes
        .get(COLUMN_CHANNEL)
        .is_some_and(|stored| stored.split('|').any(|held| held == wanted))
}

/// The stored `VALUE` of one action, as a number.
fn action_value(action: &Action) -> i64 {
    action
        .attributes
        .get(COLUMN_VALUE)
        .and_then(|held| held.parse().ok())
        .unwrap_or_default()
}

/// Every id the seeded Items catalog holds.
fn every_item() -> Vec<String> {
    catalog().iter().map(|item| item.item_id.clone()).collect()
}

/// An emulator holding the three campaigns and every filter these suites bind.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    for (arn, recipe_type) in [
        (CAMPAIGN_USER, RecipeType::UserPersonalization),
        (CAMPAIGN_RANKING, RecipeType::PersonalizedRanking),
    ] {
        assert!(
            registry::register_campaign(
                engine,
                Campaign::new(arn, "compare", recipe_type, catalog()),
            ),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new_actions(CAMPAIGN_ACTIONS, "compare-actions", action_catalog()),
        ),
        "the campaign seam must register the actions campaign"
    );

    let filters = [
        (
            INCLUDE_LITERAL_GENRE,
            "INCLUDE ItemID WHERE Items.GENRE = \"Comedy\"",
        ),
        (
            EXCLUDE_LITERAL_GENRE,
            "EXCLUDE ItemID WHERE Items.GENRE = \"Comedy\"",
        ),
        (
            INCLUDE_GENRE_EQUALS,
            "INCLUDE ItemID WHERE Items.GENRE = $GENRE",
        ),
        (
            INCLUDE_GENRE_DIFFERS,
            "INCLUDE ItemID WHERE Items.GENRE <> $GENRE",
        ),
        (
            EXCLUDE_GENRE_DIFFERS,
            "EXCLUDE ItemID WHERE Items.GENRE <> $GENRE",
        ),
        (
            INCLUDE_YEAR_EQUALS,
            "INCLUDE ItemID WHERE Items.YEAR = $YEAR",
        ),
        (
            INCLUDE_YEAR_IN,
            "INCLUDE ItemID WHERE Items.YEAR IN ($YEAR)",
        ),
        (INCLUDE_FROM_YEAR, "INCLUDE ItemID WHERE Items.YEAR >= $YEAR"),
        (EXCLUDE_FROM_YEAR, "EXCLUDE ItemID WHERE Items.YEAR >= $YEAR"),
        (
            INCLUDE_CHANNEL_EQUALS,
            "INCLUDE ActionID WHERE Actions.CHANNEL = $CHANNEL",
        ),
        (
            EXCLUDE_CHANNEL_EQUALS,
            "EXCLUDE ActionID WHERE Actions.CHANNEL = $CHANNEL",
        ),
        (
            INCLUDE_ABOVE_VALUE,
            "INCLUDE ActionID WHERE Actions.VALUE > $VALUE",
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

/// Which stored entries a row's bound expression keeps.
type RetainsItem = fn(&Item) -> bool;

/// One equality row: its name, the filter it names, the `filterValues` it sends, and
/// which stored entries that combination keeps.
type ItemRow = (&'static str, &'static str, Value, RetainsItem);

/// Every equality row: one per form the operand can take, over a categorical column and
/// over a numerical one.
fn equality_rows() -> Vec<ItemRow> {
    vec![
        (
            "a quoted literal an INCLUDE expression names",
            INCLUDE_LITERAL_GENRE,
            json!({}),
            |item| holds_genre(item, "Comedy"),
        ),
        (
            "the same literal in an EXCLUDE expression",
            EXCLUDE_LITERAL_GENRE,
            json!({}),
            |item| !holds_genre(item, "Comedy"),
        ),
        (
            "a parameter bound to a quoted value",
            INCLUDE_GENRE_EQUALS,
            json!({ COLUMN_GENRE: "\"Drama\"" }),
            |item| holds_genre(item, "Drama"),
        ),
        (
            "a parameter bound to an undelimited value",
            INCLUDE_GENRE_EQUALS,
            json!({ COLUMN_GENRE: "Drama" }),
            |item| holds_genre(item, "Drama"),
        ),
        (
            "a parameter bound to a value whose JSON escaping was left in",
            INCLUDE_GENRE_EQUALS,
            json!({ COLUMN_GENRE: "\\\"Drama\\\"" }),
            |item| holds_genre(item, "Drama"),
        ),
        (
            "a value that is one of several the column holds",
            INCLUDE_GENRE_EQUALS,
            json!({ COLUMN_GENRE: "\"Documentary\"" }),
            |item| holds_genre(item, "Documentary"),
        ),
        (
            "a value carrying a space",
            INCLUDE_GENRE_EQUALS,
            json!({ COLUMN_GENRE: "\"Science Fiction\"" }),
            |item| holds_genre(item, "Science Fiction"),
        ),
        (
            // The contrast row: an equality clause keeps nothing only when no stored
            // entry holds the value it names.
            "a value no stored entry holds",
            INCLUDE_GENRE_EQUALS,
            json!({ COLUMN_GENRE: "\"Fax\"" }),
            |_| false,
        ),
        (
            "<> in an INCLUDE expression",
            INCLUDE_GENRE_DIFFERS,
            json!({ COLUMN_GENRE: "\"Comedy\"" }),
            |item| !holds_genre(item, "Comedy"),
        ),
        (
            "<> in an EXCLUDE expression",
            EXCLUDE_GENRE_DIFFERS,
            json!({ COLUMN_GENRE: "\"Comedy\"" }),
            |item| holds_genre(item, "Comedy"),
        ),
        (
            "= over a numerical column, bound to a bare number",
            INCLUDE_YEAR_EQUALS,
            json!({ COLUMN_YEAR: TARGET_YEAR.to_string() }),
            |item| year(item) == TARGET_YEAR,
        ),
        (
            "= over a numerical column, bound to a quoted number",
            INCLUDE_YEAR_EQUALS,
            json!({ COLUMN_YEAR: format!("\"{TARGET_YEAR}\"") }),
            |item| year(item) == TARGET_YEAR,
        ),
        (
            // Two spellings of one quantity are one value, so the fraction selects the
            // same entry the integer does.
            "= over a numerical column, bound to the same quantity written with a fraction",
            INCLUDE_YEAR_EQUALS,
            json!({ COLUMN_YEAR: format!("{TARGET_YEAR}.0") }),
            |item| year(item) == TARGET_YEAR,
        ),
        (
            // Set membership is equality over a set, so it answers the two spellings the
            // same way `=` does.
            "IN over a numerical column, bound to the same quantity written with a fraction",
            INCLUDE_YEAR_IN,
            json!({ COLUMN_YEAR: format!("{TARGET_YEAR}.0") }),
            |item| year(item) == TARGET_YEAR,
        ),
        (
            "IN over a numerical column, bound to two numbers",
            INCLUDE_YEAR_IN,
            json!({ COLUMN_YEAR: format!("{TARGET_YEAR},{}", TARGET_YEAR + 2) }),
            |item| year(item) == TARGET_YEAR || year(item) == TARGET_YEAR + 2,
        ),
    ]
}

#[tokio::test]
async fn an_equality_clause_selects_the_entries_whose_stored_value_it_names() {
    let emulator = seeded();
    let stored = catalog();

    for (case, filter_arn, filter_values, retains) in equality_rows() {
        let expected: Vec<String> = stored
            .iter()
            .filter(|item| retains(item))
            .map(|item| item.item_id.clone())
            .collect();
        assert!(
            expected.len() < stored.len(),
            "case {case}: the row must select a strict subset of the catalog, got {expected:?}"
        );

        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-compare",
                    // The whole catalog, so the response shows what the filter removed
                    // rather than what `numResults` truncated.
                    "numResults": CATALOG_SIZE,
                    "filterArn": filter_arn,
                    "filterValues": filter_values,
                }),
            )
            .await;

        let returned = ids(&response, "itemList", "itemId");
        let mut sorted_returned = returned.clone();
        sorted_returned.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_returned, sorted_expected,
            "case {case}: the item list must hold exactly the entries whose stored value the \
             clause names, got {returned:?}"
        );
        assert_descending_and_bounded(case, &scores(&response, "itemList"));
    }
}

/// One route row: its name, the route, the request body, the response list member and
/// its id member, and the ids the bound equality clause keeps.
type RouteRow = (
    &'static str,
    &'static str,
    Value,
    &'static str,
    &'static str,
    Vec<String>,
);

/// One refusal row: its name, the route, the parameter the clause reads, and the body
/// built around one form of value that names no number.
type RefusalRow = (&'static str, &'static str, &'static str, fn(&str) -> Value);

#[tokio::test]
async fn an_equality_clause_applies_on_every_route_that_binds_a_filter() {
    let emulator = seeded();
    let stored = catalog();
    let actions = action_catalog();
    let input_list = every_item();

    let ranked: Vec<String> = stored
        .iter()
        .filter(|item| holds_genre(item, "Comedy"))
        .map(|item| item.item_id.clone())
        .collect();
    let on_app: Vec<String> = actions
        .iter()
        .filter(|action| on_channel(action, "App"))
        .map(|action| action.action_id.clone())
        .collect();
    let off_app: Vec<String> = actions
        .iter()
        .filter(|action| !on_channel(action, "App"))
        .map(|action| action.action_id.clone())
        .collect();

    // One rule, every route that binds a filter: the list member, its id member and the
    // ids the bound equality clause keeps.
    let rows: Vec<RouteRow> = vec![
        (
            "GetPersonalizedRanking with a literal",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-compare",
                "inputList": input_list,
                "filterArn": INCLUDE_LITERAL_GENRE,
            }),
            "personalizedRanking",
            "itemId",
            ranked,
        ),
        (
            "GetActionRecommendations with INCLUDE",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": "user-compare",
                "numResults": ACTION_CATALOG_SIZE,
                "filterArn": INCLUDE_CHANNEL_EQUALS,
                "filterValues": { COLUMN_CHANNEL: "\"App\"" },
            }),
            "actionList",
            "actionId",
            on_app.clone(),
        ),
        (
            "GetActionRecommendations with EXCLUDE",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": "user-compare",
                "numResults": ACTION_CATALOG_SIZE,
                "filterArn": EXCLUDE_CHANNEL_EQUALS,
                "filterValues": { COLUMN_CHANNEL: "\"App\"" },
            }),
            "actionList",
            "actionId",
            off_app,
        ),
    ];

    for (case, route, body, member, id_member, expected) in rows {
        assert!(
            !expected.is_empty(),
            "case {case}: the row must keep something, got {expected:?}"
        );
        let response = emulator.ok(route, body).await;
        let mut returned = ids(&response, member, id_member);
        returned.sort();
        let mut wanted = expected;
        wanted.sort();
        assert_eq!(
            returned, wanted,
            "case {case}: an equality clause must keep exactly the entries holding its value, \
             got {returned:?}"
        );
    }

    // A promotion carries a filter of its own, bound the same way: the promoted slots
    // can only hold entries the clause keeps.
    let response = emulator
        .ok(
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-compare",
                "numResults": CATALOG_SIZE,
                "promotions": [{
                    "name": "promo-compare",
                    "percentPromotedItems": 50,
                    "filterArn": INCLUDE_GENRE_EQUALS,
                    "filterValues": { COLUMN_GENRE: "\"Drama\"" },
                }],
            }),
        )
        .await;
    let returned = ids(&response, "itemList", "itemId");
    let promoted: Vec<&String> = returned
        .iter()
        .take(CATALOG_SIZE / 2)
        .filter(|id| {
            stored
                .iter()
                .any(|item| &&item.item_id == id && holds_genre(item, "Drama"))
        })
        .collect();
    assert!(
        !promoted.is_empty(),
        "case a promotion's own filter: the promoted slots must hold entries the equality \
         clause keeps, got {returned:?}"
    );
}

#[test]
fn the_filter_seam_refuses_an_ordering_comparison_against_a_non_numeric_literal() {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    // An ordering operator compares numbers, so a literal naming none is refused where
    // the filter is CREATED. The rows cover every ordering operator, both filter kinds,
    // both datasets and every way a literal can fail to name a finite number.
    let refused = [
        ("< against a quoted word", "INCLUDE ItemID WHERE Items.YEAR < \"recent\""),
        (
            "<= against a quoted word",
            "INCLUDE ItemID WHERE Items.YEAR <= \"recent\"",
        ),
        ("> against a bare word", "INCLUDE ItemID WHERE Items.YEAR > recent"),
        (
            ">= against a quoted word",
            "INCLUDE ItemID WHERE Items.YEAR >= \"recent\"",
        ),
        (
            ">= against a delimited empty value",
            "INCLUDE ItemID WHERE Items.YEAR >= \"\"",
        ),
        (
            ">= against text that only starts with a number",
            "INCLUDE ItemID WHERE Items.YEAR >= \"2010s\"",
        ),
        (
            ">= against a not-a-number",
            "INCLUDE ItemID WHERE Items.YEAR >= nan",
        ),
        (
            ">= against an infinity",
            "INCLUDE ItemID WHERE Items.YEAR >= inf",
        ),
        (
            "an EXCLUDE expression ordering a quoted word",
            "EXCLUDE ItemID WHERE Items.YEAR >= \"recent\"",
        ),
        (
            "a second clause ordering a quoted word",
            "INCLUDE ItemID WHERE Items.GENRE = \"Comedy\" AND Items.YEAR > \"recent\"",
        ),
        (
            "an Actions clause ordering a quoted word",
            "INCLUDE ActionID WHERE Actions.VALUE > \"high\"",
        ),
    ];
    for (case, expression) in refused {
        let arn = format!("arn:aws:personalize:us-east-1:000000000000:filter/order-bad-{case}");
        let outcome = registry::register_filter(engine, &arn, expression);
        assert!(
            outcome.is_err(),
            "case {case}: {expression:?} orders a value that is no number, so it must be \
             refused, got {outcome:?}"
        );
        assert!(
            registry::stored_filter(engine, &arn).is_none(),
            "case {case}: a refused expression must store no filter"
        );
    }

    // The contrast rows: an ordering operator against a number in any form, and an
    // EQUALITY operator against a string — which is a clause the evaluator applies.
    let accepted = [
        ("a bare integer", "INCLUDE ItemID WHERE Items.YEAR >= 2010"),
        ("a quoted integer", "INCLUDE ItemID WHERE Items.YEAR <= \"2010\""),
        ("a decimal", "INCLUDE ItemID WHERE Items.YEAR > 2010.5"),
        ("a negative number", "INCLUDE ItemID WHERE Items.YEAR < -1"),
        (
            "a parameter, whose value the request binds",
            "INCLUDE ItemID WHERE Items.YEAR >= $YEAR",
        ),
        (
            "= against a quoted word",
            "INCLUDE ItemID WHERE Items.GENRE = \"Comedy\"",
        ),
        (
            "= against a bare word",
            "INCLUDE ItemID WHERE Items.GENRE = Comedy",
        ),
        (
            "<> against a quoted word",
            "EXCLUDE ItemID WHERE Items.GENRE <> \"Comedy\"",
        ),
        (
            "= against a parameter over the Actions dataset",
            "INCLUDE ActionID WHERE Actions.CHANNEL = $CHANNEL",
        ),
    ];
    for (case, expression) in accepted {
        let arn = format!("arn:aws:personalize:us-east-1:000000000000:filter/order-ok-{case}");
        assert_eq!(
            registry::register_filter(engine, &arn, expression),
            Ok(true),
            "case {case}: {expression:?} is a clause the evaluator can apply, so it must be \
             accepted"
        );
    }
}

/// Every form a `filterValues` entry can take that names no number.
const NAMES_NO_NUMBER: [(&str, &str); 6] = [
    ("a quoted word", "\"recent\""),
    ("an undelimited word", "recent"),
    ("a delimited empty value", "\"\""),
    ("a delimited space", "\" \""),
    ("text that only starts with a number", "\"2010s\""),
    ("a not-a-number", "\"nan\""),
];

#[tokio::test]
async fn a_request_binding_an_ordering_comparison_to_a_non_numeric_value_is_refused() {
    let emulator = seeded();
    let input_list = every_item();

    // The value reaches the clause from the REQUEST, so the refusal is owed there — for
    // either filter kind, because the caller SUPPLIED the value rather than omitting it,
    // and on every route that binds a filter.
    let routes: Vec<RefusalRow> = vec![
        ("GetRecommendations", RECOMMENDATIONS, COLUMN_YEAR, |raw| {
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-compare",
                "numResults": CATALOG_SIZE,
                "filterArn": INCLUDE_FROM_YEAR,
                "filterValues": { COLUMN_YEAR: raw },
            })
        }),
        (
            "GetRecommendations with an EXCLUDE filter",
            RECOMMENDATIONS,
            COLUMN_YEAR,
            |raw| {
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-compare",
                    "numResults": CATALOG_SIZE,
                    "filterArn": EXCLUDE_FROM_YEAR,
                    "filterValues": { COLUMN_YEAR: raw },
                })
            },
        ),
        (
            "GetRecommendations with a promotion's own filterValues",
            RECOMMENDATIONS,
            COLUMN_YEAR,
            |raw| {
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-compare",
                    "numResults": CATALOG_SIZE,
                    "promotions": [{
                        "name": "promo-compare",
                        "percentPromotedItems": 50,
                        "filterArn": INCLUDE_FROM_YEAR,
                        "filterValues": { COLUMN_YEAR: raw },
                    }],
                })
            },
        ),
        (
            "GetActionRecommendations",
            ACTION_RECOMMENDATIONS,
            COLUMN_VALUE,
            |raw| {
                json!({
                    "campaignArn": CAMPAIGN_ACTIONS,
                    "userId": "user-compare",
                    "numResults": ACTION_CATALOG_SIZE,
                    "filterArn": INCLUDE_ABOVE_VALUE,
                    "filterValues": { COLUMN_VALUE: raw },
                })
            },
        ),
    ];

    for (route_case, route, parameter, body) in routes {
        for (form, raw) in NAMES_NO_NUMBER {
            let case = format!("{route_case} binding {form}");
            let refusal = emulator.refusal(route, body(raw)).await;
            assert_eq!(
                (refusal.status, refusal.code.as_str()),
                (400, "InvalidInputException"),
                "case {case}: a value the comparison cannot apply is invalid input, got \
                 {refusal:?}"
            );
            assert!(
                refusal.message.contains("filterValues"),
                "case {case}: the refusal must name the member it is built from, got {refusal:?}"
            );
            assert!(
                refusal.message.contains(&format!("${parameter}")),
                "case {case}: the refusal must name the parameter whose value it cannot apply, \
                 got {refusal:?}"
            );
        }
    }

    // The same rule over the ranking route, whose filter is an Items one.
    for (form, raw) in NAMES_NO_NUMBER {
        let refusal = emulator
            .refusal(
                PERSONALIZE_RANKING,
                json!({
                    "campaignArn": CAMPAIGN_RANKING,
                    "userId": "user-compare",
                    "inputList": input_list,
                    "filterArn": INCLUDE_FROM_YEAR,
                    "filterValues": { COLUMN_YEAR: raw },
                }),
            )
            .await;
        assert_eq!(
            (refusal.status, refusal.code.as_str()),
            (400, "InvalidInputException"),
            "case GetPersonalizedRanking binding {form}: a value the comparison cannot apply is \
             invalid input, got {refusal:?}"
        );
    }
}

#[tokio::test]
async fn an_ordering_comparison_keeps_the_entries_whose_number_satisfies_it() {
    let emulator = seeded();
    let stored = catalog();
    let actions = action_catalog();

    // A number in any of the forms a `filterValues` entry admits still orders the
    // column, so these rows are what the refusal above is a refusal AGAINST.
    let rows: Vec<(&str, Value, f64)> = vec![
        (
            "a bare integer",
            json!({ COLUMN_YEAR: (FIRST_YEAR + 5).to_string() }),
            (FIRST_YEAR + 5) as f64,
        ),
        (
            "a quoted integer",
            json!({ COLUMN_YEAR: format!("\"{}\"", FIRST_YEAR + 5) }),
            (FIRST_YEAR + 5) as f64,
        ),
        (
            "a decimal between two stored years",
            json!({ COLUMN_YEAR: format!("{}.5", FIRST_YEAR + 4) }),
            (FIRST_YEAR + 4) as f64 + 0.5,
        ),
        ("a negative number below every stored year", json!({ COLUMN_YEAR: "-1" }), -1.0),
    ];

    for (case, filter_values, bound) in rows {
        let expected: Vec<String> = stored
            .iter()
            .filter(|item| year(item) as f64 >= bound)
            .map(|item| item.item_id.clone())
            .collect();

        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-compare",
                    "numResults": CATALOG_SIZE,
                    "filterArn": INCLUDE_FROM_YEAR,
                    "filterValues": filter_values,
                }),
            )
            .await;

        let mut returned = ids(&response, "itemList", "itemId");
        returned.sort();
        let mut wanted = expected;
        wanted.sort();
        assert_eq!(
            returned, wanted,
            "case {case}: a >= clause must keep exactly the entries whose stored number \
             satisfies it, got {returned:?}"
        );
    }

    // And over the Actions dataset, with the strict operator.
    let bound = VALUE_STEP * 3;
    let expected: Vec<String> = actions
        .iter()
        .filter(|action| action_value(action) > bound)
        .map(|action| action.action_id.clone())
        .collect();
    let response = emulator
        .ok(
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": "user-compare",
                "numResults": ACTION_CATALOG_SIZE,
                "filterArn": INCLUDE_ABOVE_VALUE,
                "filterValues": { COLUMN_VALUE: bound.to_string() },
            }),
        )
        .await;
    let mut returned = ids(&response, "actionList", "actionId");
    returned.sort();
    let mut wanted = expected;
    wanted.sort();
    assert_eq!(
        returned, wanted,
        "case Actions.VALUE > {bound}: a strict comparison must keep exactly the actions whose \
         stored number exceeds it, got {returned:?}"
    );
}
