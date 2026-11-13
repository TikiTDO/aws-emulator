// SPDX-License-Identifier: CC0-1.0

//! What a filter expression does with an entry that stores NO value for a clause's
//! column — a catalog row that carries only some of the Items/Actions columns, and a
//! `GetPersonalizedRanking` `inputList` id the catalog never held at all.
//!
//! A clause asserts something about the value an entry stores for its column, so an
//! entry storing none satisfies the clause under no predicate: `NOT IN` cannot be
//! satisfied by an absent value any more than `IN` or a comparison can. Each clause is
//! decided on its own, so another clause of the same expression can still keep the
//! entry; an entry no clause holds for is therefore kept by an `EXCLUDE` expression
//! ("items to exclude from recommendations") and dropped by an `INCLUDE` one ("items to
//! include"), whatever shape the predicates take.
//!
//! The suites below drive that rule as TABLES whose rows differ in the predicate
//! (`IN`, `NOT IN`, `>=`, `<>`), in the filter kind, in how many clauses the expression
//! carries and how they combine (`AND`, `OR`), in which column the entry is missing,
//! and in the route the request takes — `GetPersonalizedRanking` over an `inputList`
//! holding ids the catalog never held, `GetRecommendations` over an Items catalog whose
//! rows carry different column sets, and `GetActionRecommendations` over an Actions
//! catalog of the same shape. Every expected set is computed from the SEEDED catalog,
//! so no row can pass against a canned list.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Action, Campaign, Item, RecipeType,
};

use crate::support::{
    assert_descending_and_bounded, ids, recommendation_id, scores, Emulator,
    ACTION_RECOMMENDATIONS, CHANNELS, COLUMN_GENRE, COLUMN_YEAR, GENRES, PERSONALIZE_RANKING,
    RECOMMENDATIONS,
};

/// The `PERSONALIZED_RANKING` campaign the ranking rows re-rank.
const CAMPAIGN_RANKING: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/sparse-ranking";
/// The `USER_PERSONALIZATION` campaign the recommendation rows read.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/sparse-user";
/// The `PERSONALIZED_ACTIONS` campaign the action rows read.
const CAMPAIGN_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/sparse-actions";

/// `EXCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)`.
const EXCLUDE_OTHER_GENRES: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/sparse-exclude-not-in";
/// `INCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)`.
const INCLUDE_OTHER_GENRES: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/sparse-include-not-in";
/// `EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
const EXCLUDE_GENRE: &str = "arn:aws:personalize:us-east-1:000000000000:filter/sparse-exclude-in";
/// `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
const INCLUDE_GENRE: &str = "arn:aws:personalize:us-east-1:000000000000:filter/sparse-include-in";
/// `EXCLUDE ItemID WHERE Items.YEAR >= $YEAR`.
const EXCLUDE_FROM_YEAR: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/sparse-exclude-from-year";
/// `INCLUDE ItemID WHERE Items.YEAR <> $YEAR`.
const INCLUDE_OTHER_YEARS: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/sparse-include-other-years";
/// `INCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE) OR Items.YEAR >= $YEAR`.
const INCLUDE_GENRE_OR_YEAR: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/sparse-include-or";
/// `EXCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE) AND Items.YEAR >= $YEAR`.
const EXCLUDE_GENRE_AND_YEAR: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/sparse-exclude-and";
/// `EXCLUDE ActionID WHERE Actions.CHANNEL NOT IN ($CHANNEL)`.
const EXCLUDE_OTHER_CHANNELS: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/sparse-exclude-channel";
/// `INCLUDE ActionID WHERE Actions.CHANNEL NOT IN ($CHANNEL)`.
const INCLUDE_OTHER_CHANNELS: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/sparse-include-channel";

/// The Actions-dataset column the action rows filter on, spelled the way an Actions
/// dataset spells a column.
const COLUMN_CHANNEL: &str = "CHANNEL";

/// How many Items the seeded catalog holds — a multiple of the four column shapes
/// below, so every shape is carried by more than one entry.
const CATALOG_SIZE: usize = 12;
/// How many Actions the seeded actions catalog holds.
const ACTION_CATALOG_SIZE: usize = 9;

/// The `GENRE` the rows bind `$GENRE` to: the first value the seeded catalog cycles
/// through, read from the catalog's own value list rather than written out here.
fn bound_genre() -> &'static str {
    GENRES[0]
}

/// The `CHANNEL` the action rows bind `$CHANNEL` to.
fn bound_channel() -> &'static str {
    CHANNELS[0]
}

/// The `YEAR` the rows bind `$YEAR` to — a year inside the seeded range, so a `>=`
/// clause selects a known, non-empty suffix of the entries that carry the column.
const BOUND_YEAR: i64 = FIRST_YEAR + (CATALOG_SIZE as i64) / 2;

/// The `YEAR` the seeded catalog's first entry carries; later entries count up from it.
const FIRST_YEAR: i64 = 2000;

/// The Items catalog these suites filter.
///
/// `base_score` falls with the index, so the stored popularity order is known to the
/// test. The entries differ in WHICH columns they store — both, one, the other, or
/// neither — which is what the rule under test is about, and the `GENRE` cycle is three
/// values long against a four-long column cycle, so no genre is tied to one shape.
fn mixed_items() -> Vec<Item> {
    (0..CATALOG_SIZE)
        .map(|index| {
            let item = Item::new(format!("item-{index}"), (CATALOG_SIZE - index) as f64);
            let genre = GENRES[index % 3];
            let year = (FIRST_YEAR + index as i64).to_string();
            match index % 4 {
                0 => item
                    .with_attribute(COLUMN_GENRE, genre)
                    .with_attribute(COLUMN_YEAR, year),
                1 => item.with_attribute(COLUMN_GENRE, genre),
                2 => item.with_attribute(COLUMN_YEAR, year),
                _ => item,
            }
        })
        .collect()
}

/// The Actions catalog the action rows filter: every third entry stores no `CHANNEL` at
/// all, and the ones that do cycle through the seeded channels.
fn mixed_actions() -> Vec<Action> {
    (0..ACTION_CATALOG_SIZE)
        .map(|index| {
            let action = Action::new(
                format!("action-{index}"),
                (ACTION_CATALOG_SIZE - index) as f64,
            );
            if index % 3 == 2 {
                action
            } else {
                action.with_attribute(COLUMN_CHANNEL, CHANNELS[index % CHANNELS.len()])
            }
        })
        .collect()
}

/// The `GENRE` a stored entry carries, or `None` when it stores no `GENRE` at all.
fn genre(item: &Item) -> Option<&str> {
    item.attributes.get(COLUMN_GENRE).map(String::as_str)
}

/// The `YEAR` a stored entry carries, as a number, or `None` when it stores no `YEAR`.
fn year(item: &Item) -> Option<i64> {
    item.attributes
        .get(COLUMN_YEAR)
        .and_then(|held| held.parse().ok())
}

/// The `CHANNEL` a stored action carries, or `None` when it stores no `CHANNEL`.
fn channel(action: &Action) -> Option<&str> {
    action.attributes.get(COLUMN_CHANNEL).map(String::as_str)
}

/// An emulator holding the three campaigns and every filter these suites bind.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    for (arn, recipe_type) in [
        (CAMPAIGN_RANKING, RecipeType::PersonalizedRanking),
        (CAMPAIGN_USER, RecipeType::UserPersonalization),
    ] {
        assert!(
            registry::register_campaign(
                engine,
                Campaign::new(arn, "sparse", recipe_type, mixed_items()),
            ),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new_actions(CAMPAIGN_ACTIONS, "sparse-actions", mixed_actions()),
        ),
        "the campaign seam must register the actions campaign"
    );

    let filters = [
        (
            EXCLUDE_OTHER_GENRES,
            "EXCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)",
        ),
        (
            INCLUDE_OTHER_GENRES,
            "INCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)",
        ),
        (EXCLUDE_GENRE, "EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE)"),
        (INCLUDE_GENRE, "INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)"),
        (EXCLUDE_FROM_YEAR, "EXCLUDE ItemID WHERE Items.YEAR >= $YEAR"),
        (
            INCLUDE_OTHER_YEARS,
            "INCLUDE ItemID WHERE Items.YEAR <> $YEAR",
        ),
        (
            INCLUDE_GENRE_OR_YEAR,
            "INCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE) OR Items.YEAR >= $YEAR",
        ),
        (
            EXCLUDE_GENRE_AND_YEAR,
            "EXCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE) AND Items.YEAR >= $YEAR",
        ),
        (
            EXCLUDE_OTHER_CHANNELS,
            "EXCLUDE ActionID WHERE Actions.CHANNEL NOT IN ($CHANNEL)",
        ),
        (
            INCLUDE_OTHER_CHANNELS,
            "INCLUDE ActionID WHERE Actions.CHANNEL NOT IN ($CHANNEL)",
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

/// One Items row: its name, the filter it names, the `filterValues` it sends, whether
/// the expression INCLUDEs (and therefore cannot keep an entry no clause holds for),
/// and which stored entries it keeps.
type ItemRow = (&'static str, &'static str, Value, bool, RetainsItem);

/// Every Items row: one per predicate shape the rule has to answer the same way.
///
/// Each `retains` is written from the clause the expression carries, with an entry
/// storing no value for a clause's column failing THAT clause — the rule under test —
/// and the other clauses of the expression still deciding for themselves.
fn item_rows() -> Vec<ItemRow> {
    let genre_values = json!({ COLUMN_GENRE: format!("\"{}\"", bound_genre()) });
    let year_values = json!({ COLUMN_YEAR: BOUND_YEAR.to_string() });
    let both_values = json!({
        COLUMN_GENRE: format!("\"{}\"", bound_genre()),
        COLUMN_YEAR: BOUND_YEAR.to_string(),
    });
    vec![
        (
            "EXCLUDE with NOT IN",
            EXCLUDE_OTHER_GENRES,
            genre_values.clone(),
            false,
            |item| genre(item).is_none_or(|held| held == bound_genre()),
        ),
        (
            "INCLUDE with NOT IN",
            INCLUDE_OTHER_GENRES,
            genre_values.clone(),
            true,
            |item| genre(item).is_some_and(|held| held != bound_genre()),
        ),
        (
            "EXCLUDE with IN",
            EXCLUDE_GENRE,
            genre_values.clone(),
            false,
            |item| genre(item).is_none_or(|held| held != bound_genre()),
        ),
        (
            "INCLUDE with IN",
            INCLUDE_GENRE,
            genre_values,
            true,
            |item| genre(item).is_some_and(|held| held == bound_genre()),
        ),
        (
            "EXCLUDE with a >= comparison",
            EXCLUDE_FROM_YEAR,
            year_values.clone(),
            false,
            |item| year(item).is_none_or(|held| held < BOUND_YEAR),
        ),
        (
            "INCLUDE with a <> comparison",
            INCLUDE_OTHER_YEARS,
            year_values,
            true,
            |item| year(item).is_some_and(|held| held != BOUND_YEAR),
        ),
        (
            // The row that shows the rule is per CLAUSE: an entry storing no GENRE
            // fails the NOT IN clause and is kept only when the YEAR clause holds.
            "INCLUDE with OR over two columns",
            INCLUDE_GENRE_OR_YEAR,
            both_values.clone(),
            true,
            |item| {
                genre(item).is_some_and(|held| held != bound_genre())
                    || year(item).is_some_and(|held| held >= BOUND_YEAR)
            },
        ),
        (
            "EXCLUDE with AND over two columns",
            EXCLUDE_GENRE_AND_YEAR,
            both_values,
            false,
            |item| {
                !(genre(item).is_some_and(|held| held != bound_genre())
                    && year(item).is_some_and(|held| held >= BOUND_YEAR))
            },
        ),
    ]
}

/// The `inputList` ids the ranking suite sends that the catalog never held, so they
/// carry no stored attributes at all.
const UNTRAINED_IDS: [&str; 2] = ["no-such-item", "never-in-the-training-dataset"];

#[tokio::test]
async fn a_ranked_id_the_catalog_never_held_is_decided_by_no_filter_clause() {
    let emulator = seeded();
    let catalog = mixed_items();
    let mut input_list: Vec<String> = catalog.iter().map(|item| item.item_id.clone()).collect();
    input_list.extend(UNTRAINED_IDS.iter().map(ToString::to_string));

    for (case, filter_arn, filter_values, includes, retains) in item_rows() {
        let kept_known: Vec<String> = catalog
            .iter()
            .filter(|item| retains(item))
            .map(|item| item.item_id.clone())
            .collect();
        assert!(
            !kept_known.is_empty() && kept_known.len() < catalog.len(),
            "case {case}: the row must keep a strict, non-empty subset of the catalog, got \
             {kept_known:?}"
        );
        // An id the catalog never held stores no value for any column, so no clause of
        // the expression holds for it: an EXCLUDE expression cannot remove it and an
        // INCLUDE expression cannot keep it.
        let mut expected = kept_known.clone();
        if !includes {
            expected.extend(UNTRAINED_IDS.iter().map(ToString::to_string));
        }

        let response = emulator
            .ok(
                PERSONALIZE_RANKING,
                json!({
                    "campaignArn": CAMPAIGN_RANKING,
                    "userId": "user-sparse",
                    "inputList": input_list,
                    "filterArn": filter_arn,
                    "filterValues": filter_values,
                }),
            )
            .await;

        let returned = ids(&response, "personalizedRanking", "itemId");
        let mut sorted_returned = returned.clone();
        sorted_returned.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_returned, sorted_expected,
            "case {case}: the ranking must hold exactly the listed ids its filter kept, got \
             {returned:?}"
        );

        let returned_scores = scores(&response, "personalizedRanking");
        for untrained in UNTRAINED_IDS {
            match returned.iter().position(|id| id == untrained) {
                Some(position) => {
                    // "If an item was not included in the training dataset, the item is
                    // appended to the end of the reranked list", with no relevance score.
                    assert_eq!(
                        returned_scores[position], 0.0,
                        "case {case}: {untrained} is not in the catalog, so it carries no \
                         relevance score, got {}",
                        returned_scores[position]
                    );
                    assert!(
                        returned[position + 1..]
                            .iter()
                            .all(|later| UNTRAINED_IDS.contains(&later.as_str())),
                        "case {case}: {untrained} must be appended after every known item, \
                         got {returned:?}"
                    );
                }
                None => assert!(
                    includes,
                    "case {case}: only an INCLUDE expression can drop {untrained}, got \
                     {returned:?}"
                ),
            }
        }
        assert_descending_and_bounded(case, &returned_scores);
        recommendation_id(case, &response);
    }
}

#[tokio::test]
async fn recommendations_decide_an_entry_by_the_columns_it_stores() {
    let emulator = seeded();
    let catalog = mixed_items();

    for (case, filter_arn, filter_values, _, retains) in item_rows() {
        let expected: Vec<String> = catalog
            .iter()
            .filter(|item| retains(item))
            .map(|item| item.item_id.clone())
            .collect();

        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-sparse",
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
            "case {case}: the item list must hold exactly the entries whose stored columns \
             satisfy the expression, got {returned:?}"
        );
        assert_descending_and_bounded(case, &scores(&response, "itemList"));
    }
}

#[tokio::test]
async fn action_recommendations_decide_an_action_by_the_columns_it_stores() {
    let emulator = seeded();
    let catalog = mixed_actions();

    // The same rule over the Actions dataset: the rows differ in the filter kind, and
    // both carry the predicate an absent value is most easily mistaken for satisfying.
    type RetainsAction = fn(&Action) -> bool;
    let rows: [(&str, &str, RetainsAction); 2] = [
        (
            "EXCLUDE with NOT IN",
            EXCLUDE_OTHER_CHANNELS,
            |action| channel(action).is_none_or(|held| held == bound_channel()),
        ),
        (
            "INCLUDE with NOT IN",
            INCLUDE_OTHER_CHANNELS,
            |action| channel(action).is_some_and(|held| held != bound_channel()),
        ),
    ];

    for (case, filter_arn, retains) in rows {
        let expected: Vec<String> = catalog
            .iter()
            .filter(|action| retains(action))
            .map(|action| action.action_id.clone())
            .collect();
        assert!(
            !expected.is_empty() && expected.len() < catalog.len(),
            "case {case}: the row must keep a strict, non-empty subset of the catalog, got \
             {expected:?}"
        );

        let response = emulator
            .ok(
                ACTION_RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_ACTIONS,
                    "userId": "user-sparse",
                    "numResults": ACTION_CATALOG_SIZE,
                    "filterArn": filter_arn,
                    "filterValues": { COLUMN_CHANNEL: format!("\"{}\"", bound_channel()) },
                }),
            )
            .await;

        let returned = ids(&response, "actionList", "actionId");
        let mut sorted_returned = returned.clone();
        sorted_returned.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_returned, sorted_expected,
            "case {case}: the action list must hold exactly the actions whose stored columns \
             satisfy the expression, got {returned:?}"
        );
        assert_descending_and_bounded(case, &scores(&response, "actionList"));
    }
}
