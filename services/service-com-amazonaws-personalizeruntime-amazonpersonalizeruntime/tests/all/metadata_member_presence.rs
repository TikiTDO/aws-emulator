// SPDX-License-Identifier: CC0-1.0

//! Whether a reported entry carries the `metadata` member, and what that member holds.
//!
//! Presence is a fact about the REQUEST: a request that names `metadataColumns` gets
//! the member on EVERY entry the response reports, and a request that names none gets
//! it on none of them. What the member holds is a fact about the ENTRY: only those
//! requested columns the entry stores a value for, so an entry storing none of them is
//! reported with an empty map — never with the member missing, which would tell the
//! client no metadata was requested.
//!
//! The suites below drive that rule as tables whose rows differ in which columns are
//! asked for, in which of those columns the entry stores (both, one, the other, none at
//! all), in whether the entry is in the stored catalog at all — a
//! `GetPersonalizedRanking` `inputList` id the catalog never held carries no stored
//! attributes and is appended to the ranking — and in the route the request takes.
//! Every expected map is computed from the SEEDED catalog, so no row can pass against a
//! canned body.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, Item, RecipeType,
};

use crate::support::{
    entries, ids, Emulator, COLUMN_GENRE, COLUMN_YEAR, GENRES, PERSONALIZE_RANKING,
    RECOMMENDATIONS,
};

/// The `PERSONALIZED_RANKING` campaign the ranking rows re-rank, with metadata in
/// recommendations enabled.
const CAMPAIGN_RANKING: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/metadata-ranking";
/// The `USER_PERSONALIZATION` campaign the recommendation rows read, over the same
/// catalog and with metadata in recommendations enabled.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/metadata-user";

/// How many entries the seeded Items catalog holds — a multiple of the four column
/// shapes below, so every shape is carried by more than one entry.
const CATALOG_SIZE: usize = 8;

/// The year the seeded catalog's first entry carries; later entries count up from it.
const FIRST_YEAR: i64 = 2001;

/// `inputList` ids the catalog never held, so they store no attributes at all and are
/// "appended to the end of the reranked list".
const UNTRAINED_IDS: [&str; 2] = ["no-such-item", "never-in-the-training-dataset"];

/// The Items catalog both campaigns are registered over.
///
/// The entries differ in WHICH of the two columns they store — both, one, the other, or
/// neither — which is what the rule under test generalizes over; both columns are
/// carried by some entry, so either can be asked for.
fn mixed_items() -> Vec<Item> {
    (0..CATALOG_SIZE)
        .map(|index| {
            let item = Item::new(format!("item-{index}"), (CATALOG_SIZE - index) as f64);
            let genre = GENRES[index % GENRES.len()];
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

/// An emulator holding both campaigns over that catalog.
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
                Campaign::new(arn, "metadata", recipe_type, mixed_items())
                    .with_metadata_enabled(),
            ),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    emulator
}

/// Which column sets the rows ask for: one column, the other, and both.
fn column_rows() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("one column", vec![COLUMN_GENRE]),
        ("the other column", vec![COLUMN_YEAR]),
        ("both columns", vec![COLUMN_GENRE, COLUMN_YEAR]),
    ]
}

/// The attributes the seeded catalog stores for `item_id`, or an empty set when the
/// catalog never held it.
fn stored_attributes(item_id: &str) -> BTreeMap<String, String> {
    mixed_items()
        .into_iter()
        .find(|item| item.item_id == item_id)
        .map(|item| item.attributes)
        .unwrap_or_default()
}

/// The map the rule says an entry storing `attributes` reports for `columns`: the
/// requested columns the entry stores a value for, and nothing else.
fn expected_metadata(
    attributes: &BTreeMap<String, String>,
    columns: &[&str],
) -> BTreeMap<String, String> {
    columns
        .iter()
        .filter_map(|column| {
            attributes
                .get(*column)
                .map(|value| ((*column).to_string(), value.clone()))
        })
        .collect()
}

/// The `metadata` member of one reported entry, as a map, or `None` when the entry
/// carries no `metadata` member at all.
fn reported_metadata(case: &str, entry: &Value) -> Option<BTreeMap<String, String>> {
    let member = entry.get("metadata")?;
    let object = member
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: {entry} reports a non-object metadata member"));
    Some(
        object
            .iter()
            .map(|(column, value)| {
                (
                    column.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case}: {entry} reports a non-string metadata value")
                        })
                        .to_string(),
                )
            })
            .collect(),
    )
}

/// Assert the rule over every entry of one response's list member.
fn assert_metadata_served(case: &str, response: &Value, member: &str, columns: &[&str]) {
    let reported = entries(response, member);
    assert!(
        !reported.is_empty(),
        "case {case}: the response must report entries to read metadata from, got {response}"
    );
    for entry in reported {
        let item_id = entry
            .get("itemId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: {entry} carries no itemId"))
            .to_string();
        let expected = expected_metadata(&stored_attributes(&item_id), columns);
        let actual = reported_metadata(case, &entry).unwrap_or_else(|| {
            panic!(
                "case {case}: {item_id} must carry the metadata member a request asking for \
                 {columns:?} gets, got {entry}"
            )
        });
        assert_eq!(
            actual, expected,
            "case {case}: {item_id} must report exactly the requested columns it stores, got \
             {actual:?}"
        );
    }
}

#[tokio::test]
async fn every_ranked_entry_carries_the_requested_metadata_columns_it_stores() {
    let emulator = seeded();
    let mut input_list: Vec<String> = mixed_items()
        .iter()
        .map(|item| item.item_id.clone())
        .collect();
    input_list.extend(UNTRAINED_IDS.iter().map(ToString::to_string));

    for (case, columns) in column_rows() {
        let response = emulator
            .ok(
                PERSONALIZE_RANKING,
                json!({
                    "campaignArn": CAMPAIGN_RANKING,
                    "userId": "user-metadata",
                    "inputList": input_list,
                    "metadataColumns": { "ITEMS": columns },
                }),
            )
            .await;

        // Every id the request listed comes back: the ones the catalog holds, scored,
        // and the ones it never held, appended.
        let returned = ids(&response, "personalizedRanking", "itemId");
        for untrained in UNTRAINED_IDS {
            assert!(
                returned.iter().any(|id| id == untrained),
                "case {case}: an id the catalog never held is appended to the ranking, got \
                 {returned:?}"
            );
        }
        assert_metadata_served(case, &response, "personalizedRanking", &columns);
    }
}

#[tokio::test]
async fn recommendations_serve_the_same_metadata_rule_as_a_ranking() {
    let emulator = seeded();

    for (case, columns) in column_rows() {
        let response = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-metadata",
                    // The whole catalog, so every column shape is reported.
                    "numResults": CATALOG_SIZE,
                    "metadataColumns": { "ITEMS": columns },
                }),
            )
            .await;
        assert_metadata_served(case, &response, "itemList", &columns);
    }
}

#[tokio::test]
async fn a_request_asking_for_no_metadata_columns_gets_the_member_on_no_entry() {
    let emulator = seeded();
    let mut input_list: Vec<String> = mixed_items()
        .iter()
        .map(|item| item.item_id.clone())
        .collect();
    input_list.extend(UNTRAINED_IDS.iter().map(ToString::to_string));

    // The rows differ in the route and in how `metadataColumns` is left unasked for:
    // absent entirely, an empty map, and an empty column list.
    let rows: [(&str, &str, &str, Value); 4] = [
        (
            "a ranking with no metadataColumns at all",
            PERSONALIZE_RANKING,
            "personalizedRanking",
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-metadata",
                "inputList": input_list.clone(),
            }),
        ),
        (
            "a ranking with an empty metadataColumns map",
            PERSONALIZE_RANKING,
            "personalizedRanking",
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-metadata",
                "inputList": input_list.clone(),
                "metadataColumns": {},
            }),
        ),
        (
            "a ranking with an empty column list",
            PERSONALIZE_RANKING,
            "personalizedRanking",
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-metadata",
                "inputList": input_list,
                "metadataColumns": { "ITEMS": [] },
            }),
        ),
        (
            "a recommendation with no metadataColumns at all",
            RECOMMENDATIONS,
            "itemList",
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-metadata",
                "numResults": CATALOG_SIZE,
            }),
        ),
    ];

    for (case, route, member, body) in rows {
        let response = emulator.ok(route, body).await;
        let reported = entries(&response, member);
        assert!(
            !reported.is_empty(),
            "case {case}: the response must report entries, got {response}"
        );
        for entry in reported {
            assert!(
                reported_metadata(case, &entry).is_none(),
                "case {case}: no metadata was requested, so the member is omitted entirely, \
                 got {entry}"
            );
        }
    }
}
