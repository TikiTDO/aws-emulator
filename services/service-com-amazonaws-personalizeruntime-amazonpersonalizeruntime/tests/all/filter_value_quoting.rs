// SPDX-License-Identifier: CC0-1.0

//! How a request's `filterValues` entry is read: the quotes that enclose a value
//! DELIMIT it, so what a filter expression compares against a stored attribute is the
//! text between them.
//!
//! `filterValues` is documented as "the parameter name (in matching case) as a key and
//! the filter value(s) as the corresponding value. Separate multiple values for one
//! parameter with a comma", and the request format encloses a string value in double
//! quotes (written escaped in JSON) while leaving a numeric value bare. The rule
//! generalizes over every operation that binds a filter and over every form a value
//! can take, so the suites below drive it as a TABLE whose rows differ in how many
//! values an entry carries, whether they are quoted, how they are spaced, whether a
//! value carries the separator itself, and whether the clause compares strings or
//! numbers — and over all three routes plus a promotion's own `filterValues`.
//!
//! The last two suites drive the same rule over GENERATED entries instead of a table:
//! every subset of a candidate value list up to a bounded size, each value written in a
//! different one of the forms the request format admits, so no single written form or
//! value count is privileged.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Action, Campaign, Item, RecipeType, REASON_PROMOTED_ITEM,
};

use crate::support::{
    assert_descending_and_bounded, entries, ids, scores, Emulator, ACTION_RECOMMENDATIONS,
    COLUMN_GENRE, COLUMN_YEAR, PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// The `PERSONALIZED_RANKING` campaign the ranking rows re-rank against.
const CAMPAIGN_RANKING: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/quoting-ranking";
/// The `USER_PERSONALIZATION` campaign the `GetRecommendations` rows read.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/quoting-user";
/// The `PERSONALIZED_ACTIONS` campaign the `GetActionRecommendations` rows read.
const CAMPAIGN_ACTIONS: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/quoting-actions";

/// `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
const FILTER_INCLUDE_GENRE: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/quoting-include-genre";
/// `EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
const FILTER_EXCLUDE_GENRE: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/quoting-exclude-genre";
/// `INCLUDE ItemID WHERE Items.YEAR >= $YEAR`.
const FILTER_FROM_YEAR: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/quoting-from-year";
/// `INCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)`.
const FILTER_INCLUDE_CHANNEL: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/quoting-include-channel";
/// `EXCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)`.
const FILTER_EXCLUDE_CHANNEL: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/quoting-exclude-channel";

/// The `GENRE` values the seeded Items catalog cycles through.
///
/// Test data, chosen so the rows below can bind every form a value takes: the third
/// entry carries the value SEPARATOR inside the value, and the fourth is a
/// multi-valued categorical attribute (`|` is the separator a Personalize Items CSV
/// uses within one column, which the filter module documents).
const GENRES: [&str; 5] = [
    "Comedy",
    "Drama",
    "Science Fiction, Fantasy",
    "Comedy|Documentary",
    "Action",
];

/// The `CHANNEL` values the seeded Actions catalog cycles through.
const CHANNELS: [&str; 2] = ["App", "Email"];

/// The first `YEAR` the seeded catalog carries; the column rises by one per entry, so
/// a `>= $YEAR` clause selects a known suffix of the catalog.
const FIRST_YEAR: i64 = 1990;

/// How many entries the seeded Items catalog holds.
const CATALOG_SIZE: usize = 15;
/// How many entries the seeded Actions catalog holds.
const ACTION_CATALOG_SIZE: usize = 8;

/// The Items catalog these suites filter.
fn catalog() -> Vec<Item> {
    (0..CATALOG_SIZE)
        .map(|index| {
            Item::new(format!("item-{index}"), (CATALOG_SIZE - index) as f64)
                .with_attribute(COLUMN_GENRE, GENRES[index % GENRES.len()])
                .with_attribute(COLUMN_YEAR, (FIRST_YEAR + index as i64).to_string())
        })
        .collect()
}

/// The Actions catalog these suites filter.
fn action_catalog() -> Vec<Action> {
    (0..ACTION_CATALOG_SIZE)
        .map(|index| {
            Action::new(format!("action-{index}"), (ACTION_CATALOG_SIZE - index) as f64)
                .with_attribute("CHANNEL", CHANNELS[index % CHANNELS.len()])
        })
        .collect()
}

/// Whether a stored entry's `GENRE` column holds `wanted` — the attribute may carry
/// several values separated by `|`, and `IN` matches when any of them does.
fn holds_genre(item: &Item, wanted: &str) -> bool {
    item.attributes
        .get(COLUMN_GENRE)
        .is_some_and(|stored| stored.split('|').any(|held| held == wanted))
}

/// The stored `YEAR` of one entry, as a number.
fn year(item: &Item) -> i64 {
    item.attributes
        .get(COLUMN_YEAR)
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}

/// Whether a stored action's `CHANNEL` column holds `wanted`.
fn on_channel(action: &Action, wanted: &str) -> bool {
    action
        .attributes
        .get("CHANNEL")
        .is_some_and(|stored| stored == wanted)
}

/// An emulator holding the campaigns and the placeholder filters these suites bind.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    let campaigns = [
        (CAMPAIGN_RANKING, RecipeType::PersonalizedRanking),
        (CAMPAIGN_USER, RecipeType::UserPersonalization),
    ];
    for (arn, recipe_type) in campaigns {
        assert!(
            registry::register_campaign(
                engine,
                Campaign::new(arn, "quoting", recipe_type, catalog()),
            ),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new_actions(CAMPAIGN_ACTIONS, "quoting-actions", action_catalog()),
        ),
        "the campaign seam must register the actions campaign"
    );

    let filters = [
        (
            FILTER_INCLUDE_GENRE,
            "INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)",
        ),
        (
            FILTER_EXCLUDE_GENRE,
            "EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE)",
        ),
        (FILTER_FROM_YEAR, "INCLUDE ItemID WHERE Items.YEAR >= $YEAR"),
        (
            FILTER_INCLUDE_CHANNEL,
            "INCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)",
        ),
        (
            FILTER_EXCLUDE_CHANNEL,
            "EXCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)",
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

/// The predicate a row's filter, bound to that row's `filterValues`, means over a
/// stored catalog entry.
type Retains = fn(&Item) -> bool;

/// One row: its name, the filter it names, the `filterValues` it sends, and what that
/// combination selects.
type Row = (&'static str, &'static str, Value, Retains);

/// Every form of `filterValues` entry the rule covers, with the subset each selects.
///
/// The JSON string the rows send is what a client writes after JSON escaping is
/// undone: `"\"Comedy\""` on the wire arrives as `"Comedy"`, quote characters
/// included.
fn rows() -> Vec<Row> {
    vec![
        (
            "one quoted value",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\"Comedy\"" }),
            |item| holds_genre(item, "Comedy"),
        ),
        (
            "two quoted values separated by a comma",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\"Comedy\",\"Drama\"" }),
            |item| holds_genre(item, "Comedy") || holds_genre(item, "Drama"),
        ),
        (
            "quoted values spaced around the separator",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": " \"Drama\" , \"Action\" " }),
            |item| holds_genre(item, "Drama") || holds_genre(item, "Action"),
        ),
        (
            "a quoted value carrying the separator itself",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\"Science Fiction, Fantasy\"" }),
            |item| holds_genre(item, "Science Fiction, Fantasy"),
        ),
        (
            "a quoted value naming one of several values the attribute holds",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\"Documentary\"" }),
            |item| holds_genre(item, "Documentary"),
        ),
        (
            "a quoted value no stored entry carries",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\"Opera\"" }),
            |_| false,
        ),
        (
            "an undelimited value",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "Comedy" }),
            |item| holds_genre(item, "Comedy"),
        ),
        (
            "a quoted value whose JSON escaping was left in",
            FILTER_INCLUDE_GENRE,
            json!({ "GENRE": "\\\"Comedy\\\"" }),
            |item| holds_genre(item, "Comedy"),
        ),
        (
            "EXCLUDE with one quoted value",
            FILTER_EXCLUDE_GENRE,
            json!({ "GENRE": "\"Comedy\"" }),
            |item| !holds_genre(item, "Comedy"),
        ),
        (
            "EXCLUDE with three quoted values",
            FILTER_EXCLUDE_GENRE,
            json!({ "GENRE": "\"Comedy\",\"Drama\",\"Action\"" }),
            |item| {
                !holds_genre(item, "Comedy")
                    && !holds_genre(item, "Drama")
                    && !holds_genre(item, "Action")
            },
        ),
        (
            "a bare number in a comparison",
            FILTER_FROM_YEAR,
            json!({ "YEAR": "1995" }),
            |item| year(item) >= 1995,
        ),
        (
            "a quoted number in a comparison",
            FILTER_FROM_YEAR,
            json!({ "YEAR": "\"1995\"" }),
            |item| year(item) >= 1995,
        ),
    ]
}

#[tokio::test]
async fn a_ranking_keeps_exactly_the_entries_its_bound_filter_values_name() {
    let emulator = seeded();
    let stored = catalog();
    // Every id the catalog holds, so the ranking shows exactly what the filter removed.
    let input_list: Vec<String> = stored.iter().map(|item| item.item_id.clone()).collect();

    for (case, filter_arn, filter_values, retains) in rows() {
        let expected: Vec<String> = stored
            .iter()
            .filter(|item| retains(item))
            .map(|item| item.item_id.clone())
            .collect();
        assert!(
            expected.len() < stored.len(),
            "case {case}: the row must select a strict subset of the catalog, got \
             {expected:?}"
        );

        let response = emulator
            .ok(
                PERSONALIZE_RANKING,
                json!({
                    "campaignArn": CAMPAIGN_RANKING,
                    "userId": "user-1",
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
            "case {case}: the ranking must hold exactly the entries the bound filter \
             values name, got {returned:?}"
        );
    }
}

#[tokio::test]
async fn recommendations_keep_exactly_the_entries_the_same_values_name() {
    let emulator = seeded();
    let stored = catalog();

    for (case, filter_arn, filter_values, retains) in rows() {
        let expected: Vec<String> = stored
            .iter()
            .filter(|item| retains(item))
            .map(|item| item.item_id.clone())
            .collect();

        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-2",
                    // More than the filter can leave, so the response size shows what
                    // the filter removed.
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
            "case {case}: the recommendations must hold exactly the entries the bound \
             filter values name, got {returned:?}"
        );
    }
}

#[tokio::test]
async fn an_action_filter_binds_the_same_values_the_same_way() {
    let emulator = seeded();
    let stored = action_catalog();

    // The rows differ in the filter kind and in whether the value is delimited.
    let rows: [(&str, &str, Value, &str, bool); 4] = [
        (
            "INCLUDE with a quoted value",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": "\"App\"" }),
            "App",
            true,
        ),
        (
            "INCLUDE with an undelimited value",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": "App" }),
            "App",
            true,
        ),
        (
            "EXCLUDE with a quoted value",
            FILTER_EXCLUDE_CHANNEL,
            json!({ "CHANNEL": "\"App\"" }),
            "App",
            false,
        ),
        (
            "EXCLUDE with a quoted value spaced around the separator",
            FILTER_EXCLUDE_CHANNEL,
            json!({ "CHANNEL": " \"Email\" " }),
            "Email",
            false,
        ),
    ];

    for (case, filter_arn, filter_values, channel, keeps_matches) in rows {
        let expected: Vec<String> = stored
            .iter()
            .filter(|action| on_channel(action, channel) == keeps_matches)
            .map(|action| action.action_id.clone())
            .collect();
        assert!(
            !expected.is_empty() && expected.len() < stored.len(),
            "case {case}: the row must select a strict, non-empty subset, got {expected:?}"
        );

        let response = emulator
            .ok(
                ACTION_RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_ACTIONS,
                    "userId": "user-3",
                    "numResults": ACTION_CATALOG_SIZE,
                    "filterArn": filter_arn,
                    "filterValues": filter_values,
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
            "case {case}: the action list must hold exactly the actions the bound filter \
             values name, got {returned:?}"
        );
    }
}

/// The single values a stored `GENRE` column can hold, plus one value no stored entry
/// carries at all.
///
/// `Comedy|Documentary` is ONE stored entry holding two of these — `|` is the separator
/// a Personalize Items CSV uses WITHIN one column, which the filter module documents —
/// so each of them appears here on its own.
const CANDIDATES: [&str; 6] = [
    "Comedy",
    "Drama",
    "Science Fiction, Fantasy",
    "Documentary",
    "Action",
    "Opera",
];

/// The value separator inside one `filterValues` entry: "Separate multiple values for
/// one parameter with a comma."
const SEPARATOR: char = ',';

/// How one single value of a generated entry is written: the quoted form a string value
/// takes, that same form with the JSON escaping of the documented request format left
/// in, and the bare form a numeric value takes.
const FORMS: [Form; 3] = [
    ("quoted", |value| format!("\"{value}\"")),
    ("quoted with its JSON escaping left in", |value| {
        format!("\\\"{value}\\\"")
    }),
    ("undelimited", |value| value.to_string()),
];

/// One written form: its name, and how it writes a single value.
type Form = (&'static str, fn(&str) -> String);

/// The index of the undelimited form in [`FORMS`], the one form a value carrying the
/// separator cannot use.
const UNDELIMITED: usize = 2;

/// `values` written as ONE `filterValues` entry — the rendered values joined by the
/// separator — plus a description of the forms used.
///
/// Each value takes a different form, rotated by `rotation` so the generated entries
/// mix the forms differently, and every other value is spaced around the separator:
/// what delimits a value is the quotes, so neither the form of its neighbours nor the
/// spacing around it may change which value it names.
fn written_entry(values: &[&str], rotation: usize) -> (String, String) {
    let mut rendered: Vec<String> = Vec::new();
    let mut described: Vec<String> = Vec::new();
    for (position, value) in values.iter().enumerate() {
        let mut form = (position + rotation) % FORMS.len();
        // A value carrying the separator can only be written delimited: the delimiters
        // are what make the separator inside it part of the value.
        if form == UNDELIMITED && value.contains(SEPARATOR) {
            form = 0;
        }
        let (form_name, write) = FORMS[form];
        let written = write(value);
        described.push(format!("{value:?} {form_name}"));
        rendered.push(if (position + rotation).is_multiple_of(2) {
            written
        } else {
            format!("  {written} ")
        });
    }
    (rendered.join(&SEPARATOR.to_string()), described.join(" + "))
}

/// Every subset of [`CANDIDATES`] holding at most `max_values` of them, each as the one
/// `filterValues` entry that binds it, with the subset it names.
///
/// Bounded by construction: the subsets of a six-value list up to that size.
fn generated_entries(max_values: u32) -> Vec<(String, String, Vec<&'static str>)> {
    let mut generated = Vec::new();
    for mask in 1u32..(1 << CANDIDATES.len()) {
        if mask.count_ones() > max_values {
            continue;
        }
        let selected: Vec<&'static str> = CANDIDATES
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, value)| *value)
            .collect();
        let (written, forms) = written_entry(&selected, mask as usize);
        generated.push((written, forms, selected));
    }
    generated
}

#[tokio::test]
async fn a_generated_entry_selects_exactly_the_entries_its_values_name() {
    let emulator = seeded();
    let stored = catalog();

    for (written, forms, selected) in generated_entries(3) {
        for (kind, filter_arn, keeps_matches) in [
            ("INCLUDE", FILTER_INCLUDE_GENRE, true),
            ("EXCLUDE", FILTER_EXCLUDE_GENRE, false),
        ] {
            let case = format!("{kind} bound to {forms} written as {written:?}");
            let expected: Vec<String> = stored
                .iter()
                .filter(|item| {
                    selected.iter().any(|value| holds_genre(item, value)) == keeps_matches
                })
                .map(|item| item.item_id.clone())
                .collect();

            let response = emulator
                .ok(
                    RECOMMENDATIONS,
                    json!({
                        "campaignArn": CAMPAIGN_USER,
                        "userId": "user-5",
                        // More than the filter can leave, so the response size shows
                        // exactly what the filter removed.
                        "numResults": CATALOG_SIZE,
                        "filterArn": filter_arn,
                        "filterValues": { COLUMN_GENRE: written },
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
                "case {case}: the recommendations must hold exactly the entries the \
                 written values name, got {returned:?}"
            );
            assert_descending_and_bounded(&case, &scores(&response, "itemList"));
        }
    }
}

#[tokio::test]
async fn a_promotions_generated_entry_promotes_exactly_what_it_names() {
    let emulator = seeded();
    let stored = catalog();

    /// How many slots the request asks for.
    const NUM_RESULTS: usize = 10;
    /// The share of them the promotion takes.
    const PERCENT: usize = 50;

    let slots = NUM_RESULTS * PERCENT / 100;

    for (written, forms, selected) in generated_entries(2) {
        let case = format!("a promotion bound to {forms} written as {written:?}");
        // The promotion takes its share of the returned slots, or as many as its own
        // filter leaves, whichever is fewer.
        let matching = stored
            .iter()
            .filter(|item| selected.iter().any(|value| holds_genre(item, value)))
            .count();
        let expected_promoted = slots.min(matching);

        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-6",
                    "numResults": NUM_RESULTS,
                    "promotions": [{
                        "name": "promo-generated",
                        "percentPromotedItems": PERCENT,
                        "filterArn": FILTER_INCLUDE_GENRE,
                        "filterValues": { COLUMN_GENRE: written },
                    }],
                }),
            )
            .await;

        let returned = entries(&response, "itemList");
        let promoted: Vec<&Value> = returned
            .iter()
            .filter(|entry| entry.get("promotionName").is_some())
            .collect();
        assert_eq!(
            promoted.len(),
            expected_promoted,
            "case {case}: the promotion must take {expected_promoted} of the returned \
             slots, got {promoted:?}"
        );
        for entry in &promoted {
            assert_eq!(
                entry.get("reason").and_then(Value::as_array),
                Some(&vec![json!(REASON_PROMOTED_ITEM)]),
                "case {case}: a promoted item must carry the documented reason, got {entry}"
            );
            let item_id = entry
                .get("itemId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let item = stored
                .iter()
                .find(|item| item.item_id == item_id)
                .unwrap_or_else(|| panic!("case {case}: {item_id} is not in the stored catalog"));
            assert!(
                selected.iter().any(|value| holds_genre(item, value)),
                "case {case}: a promoted item must satisfy the values the promotion \
                 bound {selected:?}, got {item_id} with GENRE {:?}",
                item.attributes.get(COLUMN_GENRE)
            );
        }
    }
}

#[tokio::test]
async fn a_promotions_own_filter_values_bind_the_same_way() {
    let emulator = seeded();
    let stored = catalog();

    // A promotion carries its own `filterValues`, documented in the same words, so the
    // promoted slots are the entries those values name. The rows differ in how many
    // values the entry carries and in whether they are delimited.
    let rows: [(&str, Value, Vec<&str>); 3] = [
        (
            "one quoted value",
            json!({ "GENRE": "\"Comedy\"" }),
            vec!["Comedy"],
        ),
        (
            "two quoted values",
            json!({ "GENRE": "\"Drama\",\"Action\"" }),
            vec!["Drama", "Action"],
        ),
        (
            "an undelimited value",
            json!({ "GENRE": "Comedy" }),
            vec!["Comedy"],
        ),
    ];

    for (case, filter_values, wanted) in rows {
        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-4",
                    "numResults": 10,
                    "promotions": [{
                        "name": "promo-quoting",
                        "percentPromotedItems": 50,
                        "filterArn": FILTER_INCLUDE_GENRE,
                        "filterValues": filter_values,
                    }],
                }),
            )
            .await;

        let returned = entries(&response, "itemList");
        let promoted: Vec<&Value> = returned
            .iter()
            .filter(|entry| entry.get("promotionName").is_some())
            .collect();
        assert_eq!(
            promoted.len(),
            5,
            "case {case}: half of ten slots must carry the promotion, got {promoted:?}"
        );
        for entry in &promoted {
            assert_eq!(
                entry.get("reason").and_then(Value::as_array),
                Some(&vec![json!(REASON_PROMOTED_ITEM)]),
                "case {case}: a promoted item must carry the documented reason, got {entry}"
            );
            let item_id = entry
                .get("itemId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let item = stored
                .iter()
                .find(|item| item.item_id == item_id)
                .unwrap_or_else(|| panic!("case {case}: {item_id} is not in the stored catalog"));
            assert!(
                wanted.iter().any(|value| holds_genre(item, value)),
                "case {case}: a promoted item must satisfy the promotion's bound filter \
                 values {wanted:?}, got {item_id} with GENRE {:?}",
                item.attributes.get(COLUMN_GENRE)
            );
        }
    }
}
