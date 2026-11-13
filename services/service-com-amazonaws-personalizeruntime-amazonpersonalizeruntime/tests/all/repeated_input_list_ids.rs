// SPDX-License-Identifier: CC0-1.0

//! What a `GetPersonalizedRanking` request that names the same id more than once gets
//! back.
//!
//! `InputList` carries no `@uniqueItems`
//! (`smithy_query(query="shape",shape="InputList")`), so a caller may legitimately
//! repeat an id inside the list bound the same request fixes ("If you are including
//! metadata in recommendations, the maximum is 50. Otherwise, the maximum is 500." —
//! `GetPersonalizedRankingRequest$inputList`). Every occurrence of an id names the ONE
//! entry of the scored catalog that carries that id, and a reported `score` is that
//! entry's share of the single unit of certainty the scored catalog apportions — so the
//! ranking reports each DISTINCT id once, keeping the place of its first occurrence.
//! Reporting it per occurrence would hand out one entry's share several times and the
//! reported scores would sum past 1.0.
//!
//! The rows span what the rule generalizes over: how often an id repeats, which ids do,
//! how far apart the occurrences are, how much of the catalog the list covers, whether
//! the catalog holds the repeated id at all, whether the catalog carries popularity,
//! and which filter removes part of the list. The list LENGTH is measured before any of
//! this — it is a fact about the request — so the second suite pins the bound against
//! occurrences rather than distinct ids.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, Item, RecipeType,
};

use crate::support::{
    assert_descending_and_bounded, assert_sums_to_one, ids, items, scores, scores_by_id,
    Emulator, PERSONALIZE_RANKING,
};

/// The `PERSONALIZED_RANKING` campaign over a catalog that carries popularity.
const CAMPAIGN_RANKING: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/repeat-ranking";
/// A `PERSONALIZED_RANKING` campaign whose entries carry NO popularity, so every
/// entry's share is 0.0 and the tie-break alone orders the ranking.
const CAMPAIGN_FLAT: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/repeat-flat";
/// A `PERSONALIZED_RANKING` campaign with metadata in recommendations enabled, which is
/// what lets a request ask for `metadataColumns` and so take the lower list bound.
const CAMPAIGN_METADATA: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/repeat-metadata";

/// `EXCLUDE ItemID WHERE Items.GENRE IN ("Comedy")`.
const FILTER_EXCLUDE_COMEDY: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/repeat-exclude-comedy";
/// `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
const FILTER_INCLUDE_GENRE: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/repeat-include-genre";

/// How many entries the seeded Items catalogs hold. Past ten, so the ids' ascending
/// order is not the catalog order and no assertion on order can pass by accident.
const CATALOG_SIZE: usize = 16;

/// Two calls may not report an entry's certainty differently at all, so scores are
/// compared exactly up to double-rounding noise.
const TOLERANCE: f64 = 1e-12;

/// Ids no seeded catalog holds: "If an item was not included in the training dataset,
/// the item is appended to the end of the reranked list".
const UNKNOWN_ITEMS: [&str; 2] = ["no-such-item-a", "no-such-item-b"];

/// The predicate a row's registered filter expression means, over a stored entry.
type Retains = fn(&Item) -> bool;

/// One re-ranking row: its name, the campaign it re-ranks against, the `inputList` it
/// sends, the `filterArn` and `filterValues` it names, and the predicate that filter
/// expression means.
type Row = (
    &'static str,
    &'static str,
    Vec<String>,
    Option<&'static str>,
    Value,
    Retains,
);

/// An emulator holding the three campaigns and two filters this suite reads.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    assert!(
        registry::register_campaign(
            engine,
            Campaign::new(
                CAMPAIGN_RANKING,
                "repeat-ranking",
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
                CAMPAIGN_FLAT,
                "repeat-flat",
                RecipeType::PersonalizedRanking,
                items(CATALOG_SIZE)
                    .into_iter()
                    .map(|item| Item {
                        base_score: 0.0,
                        ..item
                    })
                    .collect(),
            ),
        ),
        "the campaign seam must register the popularity-less ranking campaign"
    );
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new(
                CAMPAIGN_METADATA,
                "repeat-metadata",
                RecipeType::PersonalizedRanking,
                items(CATALOG_SIZE),
            )
            .with_metadata_enabled(),
        ),
        "the campaign seam must register the metadata-enabled ranking campaign"
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

/// Every id of the seeded Items catalog, in catalog order.
fn catalog_ids() -> Vec<String> {
    items(CATALOG_SIZE)
        .into_iter()
        .map(|item| item.item_id)
        .collect()
}

/// The stored `GENRE` of a seeded catalog entry.
fn genre(item: &Item) -> &str {
    item.attributes
        .get(crate::support::COLUMN_GENRE)
        .map(String::as_str)
        .unwrap_or_default()
}

/// The ids of `sent` in the order of their FIRST occurrence, with every repeat removed
/// — the list a caller who de-duplicated it himself would have sent.
fn distinct(sent: &[String]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for id in sent {
        if !seen.iter().any(|kept| kept == id) {
            seen.push(id.clone());
        }
    }
    seen
}

/// One `GetPersonalizedRanking` request body.
fn request(
    campaign_arn: &str,
    input_list: &[String],
    filter_arn: Option<&str>,
    filter_values: Value,
) -> Value {
    let mut body = json!({
        "campaignArn": campaign_arn,
        "userId": "user-7",
        "inputList": input_list,
    });
    if let Some(filter_arn) = filter_arn {
        body["filterArn"] = json!(filter_arn);
        body["filterValues"] = filter_values;
    }
    body
}

#[tokio::test]
async fn a_repeated_input_list_id_is_reported_once_with_its_own_share() {
    let emulator = seeded();
    let catalog = items(CATALOG_SIZE);
    let every_id = catalog_ids();

    // The reference every reported score is measured against: the whole catalog listed
    // once, which reports every scored entry and therefore the whole certainty.
    let mut reference: BTreeMap<&str, BTreeMap<String, f64>> = BTreeMap::new();
    for campaign in [CAMPAIGN_RANKING, CAMPAIGN_FLAT] {
        let whole = emulator
            .ok(PERSONALIZE_RANKING, request(campaign, &every_id, None, json!({})))
            .await;
        let reported = scores_by_id(&whole, "personalizedRanking", "itemId");
        assert_eq!(
            reported.len(),
            CATALOG_SIZE,
            "case {campaign}: the reference must report every catalog entry, got {reported:?}"
        );
        reference.insert(campaign, reported);
    }

    let twice_interleaved: Vec<String> = every_id
        .iter()
        .flat_map(|id| [id.clone(), id.clone()])
        .collect();
    let twice_appended: Vec<String> = every_id.iter().chain(every_id.iter()).cloned().collect();
    let thrice_reversed: Vec<String> = (0..3)
        .flat_map(|_| every_id.iter().rev().cloned())
        .collect();
    let rows: Vec<Row> = vec![
        (
            "one id twice, adjacent",
            CAMPAIGN_RANKING,
            vec![every_id[3].clone(), every_id[3].clone()],
            None,
            json!({}),
            |_| true,
        ),
        (
            "one id twice, with another id between them",
            CAMPAIGN_RANKING,
            vec![
                every_id[3].clone(),
                every_id[11].clone(),
                every_id[3].clone(),
            ],
            None,
            json!({}),
            |_| true,
        ),
        (
            "one id five times",
            CAMPAIGN_RANKING,
            vec![every_id[9].clone(); 5],
            None,
            json!({}),
            |_| true,
        ),
        (
            "every catalog id twice, interleaved",
            CAMPAIGN_RANKING,
            twice_interleaved.clone(),
            None,
            json!({}),
            |_| true,
        ),
        (
            "the whole catalog, then the whole catalog again",
            CAMPAIGN_RANKING,
            twice_appended.clone(),
            None,
            json!({}),
            |_| true,
        ),
        (
            "the whole catalog three times, in reverse order",
            CAMPAIGN_RANKING,
            thrice_reversed.clone(),
            None,
            json!({}),
            |_| true,
        ),
        (
            "an id the catalog never held, twice",
            CAMPAIGN_RANKING,
            vec![
                every_id[1].clone(),
                UNKNOWN_ITEMS[0].to_string(),
                UNKNOWN_ITEMS[0].to_string(),
                every_id[5].clone(),
            ],
            None,
            json!({}),
            |_| true,
        ),
        (
            "only ids the catalog never held, each twice",
            CAMPAIGN_RANKING,
            vec![
                UNKNOWN_ITEMS[0].to_string(),
                UNKNOWN_ITEMS[1].to_string(),
                UNKNOWN_ITEMS[0].to_string(),
                UNKNOWN_ITEMS[1].to_string(),
            ],
            None,
            json!({}),
            |_| true,
        ),
        (
            "no id repeats at all",
            CAMPAIGN_RANKING,
            vec![
                every_id[2].clone(),
                every_id[5].clone(),
                every_id[13].clone(),
            ],
            None,
            json!({}),
            |_| true,
        ),
        (
            "every catalog id twice over a catalog with no popularity",
            CAMPAIGN_FLAT,
            twice_interleaved.clone(),
            None,
            json!({}),
            |_| true,
        ),
        (
            "one id twice over a catalog with no popularity",
            CAMPAIGN_FLAT,
            vec![every_id[4].clone(), every_id[4].clone()],
            None,
            json!({}),
            |_| true,
        ),
        (
            "every catalog id twice, EXCLUDE Comedy",
            CAMPAIGN_RANKING,
            twice_appended.clone(),
            Some(FILTER_EXCLUDE_COMEDY),
            json!({}),
            |item| genre(item) != "Comedy",
        ),
        (
            "a repeated id the filter removes",
            CAMPAIGN_RANKING,
            vec![
                every_id[0].clone(),
                every_id[0].clone(),
                every_id[1].clone(),
            ],
            Some(FILTER_EXCLUDE_COMEDY),
            json!({}),
            |item| genre(item) != "Comedy",
        ),
        (
            "every catalog id twice, INCLUDE GENRE Comedy or Drama",
            CAMPAIGN_RANKING,
            twice_interleaved.clone(),
            Some(FILTER_INCLUDE_GENRE),
            json!({ "GENRE": "Comedy,Drama" }),
            |item| matches!(genre(item), "Comedy" | "Drama"),
        ),
    ];

    for (case, campaign, sent, filter_arn, filter_values, retains) in rows {
        let response = emulator
            .ok(
                PERSONALIZE_RANKING,
                request(campaign, &sent, filter_arn, filter_values.clone()),
            )
            .await;
        let returned = ids(&response, "personalizedRanking", "itemId");
        let reported = scores(&response, "personalizedRanking");

        // The ranking holds each DISTINCT listed id the filter kept, exactly once.
        let mut expected: Vec<String> = distinct(&sent)
            .into_iter()
            .filter(|id| {
                catalog
                    .iter()
                    .find(|item| &item.item_id == id)
                    .is_none_or(retains)
            })
            .collect();
        let mut sorted_returned = returned.clone();
        sorted_returned.sort();
        expected.sort();
        assert_eq!(
            sorted_returned, expected,
            "case {case}: the ranking must hold each distinct listed id its filter kept \
             exactly once, got {returned:?}"
        );

        // Every reported score is the share the whole-catalog call gave that entry; an
        // id the catalog never held carries no prediction at all. Read entry by entry,
        // so a duplicate copy cannot hide behind a map keyed by `itemId`.
        for (position, (id, score)) in returned.iter().zip(reported.iter()).enumerate() {
            match reference[campaign].get(id) {
                Some(expected) => assert!(
                    (score - expected).abs() < TOLERANCE,
                    "case {case}: {id} at position {position} scores {expected} over the \
                     whole scored catalog, got {score}"
                ),
                None => assert_eq!(
                    *score, 0.0,
                    "case {case}: {id} at position {position} is not in the catalog, so it \
                     carries no prediction, got {score}"
                ),
            }
        }

        // The invariant the shares are: descending, each in [0.0, 1.0], and summing to
        // at most the one unit of certainty — however often the caller repeated an id.
        assert_descending_and_bounded(case, &reported);
        // A ranking that reports every entry of a catalog carrying popularity reports
        // the whole certainty: exactly 1.0, never more.
        if campaign == CAMPAIGN_RANKING && expected.len() == CATALOG_SIZE {
            assert_sums_to_one(case, &reported);
        }

        // Repetition is not an input the response depends on: the same request with the
        // caller's own repeats removed answers with the same ids in the same order,
        // carrying the same scores.
        let once = emulator
            .ok(
                PERSONALIZE_RANKING,
                request(campaign, &distinct(&sent), filter_arn, filter_values.clone()),
            )
            .await;
        let once_ids = ids(&once, "personalizedRanking", "itemId");
        assert_eq!(
            returned, once_ids,
            "case {case}: repeating an id must not change which ids come back or their \
             order, expected {once_ids:?}, got {returned:?}"
        );
        for (position, (score, expected)) in reported
            .iter()
            .zip(scores(&once, "personalizedRanking"))
            .enumerate()
        {
            assert!(
                (score - expected).abs() < TOLERANCE,
                "case {case}: position {position} must score {expected} whether or not the \
                 list repeats an id, got {score}"
            );
        }
    }
}

#[tokio::test]
async fn the_input_list_bound_counts_occurrences_not_distinct_ids() {
    let emulator = seeded();
    let every_id = catalog_ids();

    // "If you are including metadata in recommendations, the maximum is 50. Otherwise,
    // the maximum is 500." (`GetPersonalizedRankingRequest$inputList`) — the two
    // published bounds, each tested AT the bound and one past it. The length is a fact
    // about the REQUEST, so it counts the entries the caller sent, not the distinct ids
    // they name: a list of one id repeated past the bound is refused.
    let rows: [(&str, &str, bool, usize); 8] = [
        ("one id, at the bound", CAMPAIGN_RANKING, false, registry::MAX_NUM_RESULTS),
        (
            "one id, one past the bound",
            CAMPAIGN_RANKING,
            false,
            registry::MAX_NUM_RESULTS + 1,
        ),
        (
            "one id, at the metadata bound",
            CAMPAIGN_METADATA,
            true,
            registry::MAX_NUM_RESULTS_WITH_METADATA,
        ),
        (
            "one id, one past the metadata bound",
            CAMPAIGN_METADATA,
            true,
            registry::MAX_NUM_RESULTS_WITH_METADATA + 1,
        ),
        (
            "one id, past the metadata bound but inside the plain one",
            CAMPAIGN_RANKING,
            false,
            registry::MAX_NUM_RESULTS_WITH_METADATA + 1,
        ),
        (
            "one id, well past both bounds",
            CAMPAIGN_RANKING,
            false,
            registry::MAX_NUM_RESULTS * 2,
        ),
        ("one id, twice", CAMPAIGN_RANKING, false, 2),
        ("one id, once", CAMPAIGN_RANKING, false, 1),
    ];

    for (case, campaign, with_metadata, occurrences) in rows {
        let sent = vec![every_id[6].clone(); occurrences];
        let mut body = request(campaign, &sent, None, json!({}));
        let max = if with_metadata {
            body["metadataColumns"] = json!({ "ITEMS": [crate::support::COLUMN_GENRE] });
            registry::MAX_NUM_RESULTS_WITH_METADATA
        } else {
            registry::MAX_NUM_RESULTS
        };

        if occurrences > max {
            let refusal = emulator.refusal(PERSONALIZE_RANKING, body).await;
            assert_eq!(
                (
                    refusal.status,
                    refusal.code.as_str(),
                    refusal.message.as_str()
                ),
                (
                    400,
                    "InvalidInputException",
                    format!("Length of 'inputList' cannot be greater than {max}").as_str()
                ),
                "case {case}: {occurrences} occurrences are past the bound of {max}, so the \
                 request is refused for its length, got {refusal:?}"
            );
            continue;
        }

        let response = emulator.ok(PERSONALIZE_RANKING, body).await;
        let returned = ids(&response, "personalizedRanking", "itemId");
        assert_eq!(
            returned,
            vec![every_id[6].clone()],
            "case {case}: {occurrences} occurrences of one id name one catalog entry, so \
             the ranking reports it once, got {returned:?}"
        );
        assert_descending_and_bounded(case, &scores(&response, "personalizedRanking"));
    }
}
