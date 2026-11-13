// SPDX-License-Identifier: CC0-1.0

//! What a `GetRecommendations` promotion's share is a percentage OF.
//!
//! `Promotion$percentPromotedItems` is "The percentage of recommended items to apply
//! the promotion to" (`smithy_query(query="shape", shape="Promotion")`), so the share
//! is taken of the entries the response actually REPORTS. `numResults` is only what
//! the caller asked for: a stored catalog, the item a `RELATED_ITEMS` request queried
//! and the request's own filter can each leave fewer candidates than that, and a
//! response that reports fewer entries gives the promotion a proportionally smaller
//! number of them — never a larger fraction of the list than the request named.
//!
//! The rows below span the ways a response ends up shorter than `numResults` (a small
//! catalog, a selective request filter, the queried item a related-items response
//! never reports), the percentages (1, 25, 30, 50, 100), the rounding (an exact share
//! and one that has to round up) and a promotion filter narrower than its own share,
//! with rows where the reported count EQUALS `numResults` as the control. Every
//! expected count is computed from the seeded catalog and the request, so no row can
//! pass against a canned list.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, Item, RecipeType, REASON_PROMOTED_ITEM,
};

use crate::support::{entries, items, Emulator, COLUMN_GENRE, COLUMN_YEAR, RECOMMENDATIONS};

/// A `USER_PERSONALIZATION` campaign over a 40-entry catalog: more entries than any
/// row asks for, so only a filter can shorten its response.
const CAMPAIGN_FULL: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/share-full";
/// A `USER_PERSONALIZATION` campaign whose whole catalog is shorter than the rows'
/// `numResults`.
const CAMPAIGN_TINY: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/share-tiny";
/// A `USER_PERSONALIZATION` campaign whose catalog is an ODD size, so a 50% share of
/// the reported entries has to round.
const CAMPAIGN_ODD: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/share-odd";
/// A `RELATED_ITEMS` campaign: its response never reports the queried item, so its
/// catalog reports one entry fewer than it holds.
const CAMPAIGN_RELATED: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/share-related";

/// `INCLUDE ItemID WHERE Items.YEAR >= 1990` — the seeded `YEAR` column starts at
/// 1990, so this keeps every stored entry.
const FILTER_EVERY_ENTRY: &str = "arn:aws:personalize:us-east-1:000000000000:filter/share-every";
/// `INCLUDE ItemID WHERE Items.GENRE IN ("Comedy")`.
const FILTER_COMEDY: &str = "arn:aws:personalize:us-east-1:000000000000:filter/share-comedy";
/// `INCLUDE ItemID WHERE Items.YEAR >= 2019` — the highest seeded `YEAR`, so it keeps
/// a single entry of the 40-entry catalog.
const FILTER_NEWEST: &str = "arn:aws:personalize:us-east-1:000000000000:filter/share-newest";

/// How many entries the 40-entry catalog holds.
const FULL_CATALOG: usize = 40;
/// How many entries the short catalog holds.
const TINY_CATALOG: usize = 4;
/// How many entries the odd-sized catalog holds.
const ODD_CATALOG: usize = 7;
/// How many entries the related-items catalog holds.
const RELATED_CATALOG: usize = 5;

/// Keeps every stored entry: what [`FILTER_EVERY_ENTRY`] selects, and what a request
/// carrying no `filterArn` leaves.
fn every_entry(_item: &Item) -> bool {
    true
}

/// What [`FILTER_COMEDY`] selects.
fn comedy(item: &Item) -> bool {
    item.attributes.get(COLUMN_GENRE).map(String::as_str) == Some("Comedy")
}

/// What [`FILTER_NEWEST`] selects.
fn newest(item: &Item) -> bool {
    item.attributes
        .get(COLUMN_YEAR)
        .and_then(|year| year.parse::<i64>().ok())
        .is_some_and(|year| year >= 2019)
}

/// A registered filter, paired with the predicate that says which stored entries its
/// expression keeps — so a row states the filter it sends and the test computes what
/// that filter leaves.
type SeededFilter = (&'static str, fn(&Item) -> bool);

/// One request, and everything about the seeded state the expected share is computed
/// from.
struct Row {
    /// How the case reads in an assertion.
    case: &'static str,
    /// The campaign the request recommends from.
    campaign: &'static str,
    /// Its stored catalog, as the test seeded it.
    catalog: Vec<Item>,
    /// The `itemId` a `RELATED_ITEMS` request queries, which its response never
    /// reports.
    queried_item: Option<&'static str>,
    /// The request's `filterArn`, and the entries it keeps.
    request_filter: Option<SeededFilter>,
    /// The promotion's `filterArn`, and the entries it promotes.
    promotion_filter: SeededFilter,
    /// The request's `numResults`.
    num_results: usize,
    /// The promotion's `percentPromotedItems`.
    percent: usize,
}

/// An emulator holding the campaigns and filters the rows read.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    let campaigns = [
        (CAMPAIGN_FULL, RecipeType::UserPersonalization, FULL_CATALOG),
        (CAMPAIGN_TINY, RecipeType::UserPersonalization, TINY_CATALOG),
        (CAMPAIGN_ODD, RecipeType::UserPersonalization, ODD_CATALOG),
        (CAMPAIGN_RELATED, RecipeType::RelatedItems, RELATED_CATALOG),
    ];
    for (arn, recipe_type, size) in campaigns {
        assert!(
            registry::register_campaign(
                engine,
                Campaign::new(arn, "share", recipe_type, items(size))
            ),
            "case {arn}: the campaign seam must register the campaign"
        );
    }

    let filters = [
        (FILTER_EVERY_ENTRY, "INCLUDE ItemID WHERE Items.YEAR >= 1990"),
        (
            FILTER_COMEDY,
            "INCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\")",
        ),
        (FILTER_NEWEST, "INCLUDE ItemID WHERE Items.YEAR >= 2019"),
    ];
    for (arn, expression) in filters {
        assert_eq!(
            registry::register_filter(engine, arn, expression),
            Ok(true),
            "case {arn}: the filter seam must register {expression}"
        );
    }

    emulator
}

#[tokio::test]
async fn a_promotion_takes_its_percentage_of_the_entries_the_response_reports() {
    let emulator = seeded();

    let rows = [
        Row {
            case: "a catalog shorter than numResults",
            campaign: CAMPAIGN_TINY,
            catalog: items(TINY_CATALOG),
            queried_item: None,
            request_filter: None,
            promotion_filter: (FILTER_EVERY_ENTRY, every_entry),
            num_results: 20,
            percent: 25,
        },
        Row {
            case: "a catalog shorter than numResults, promoted in whole",
            campaign: CAMPAIGN_TINY,
            catalog: items(TINY_CATALOG),
            queried_item: None,
            request_filter: None,
            promotion_filter: (FILTER_EVERY_ENTRY, every_entry),
            num_results: 20,
            percent: 100,
        },
        Row {
            case: "an odd-sized catalog, half of it rounded up",
            campaign: CAMPAIGN_ODD,
            catalog: items(ODD_CATALOG),
            queried_item: None,
            request_filter: None,
            promotion_filter: (FILTER_EVERY_ENTRY, every_entry),
            num_results: 25,
            percent: 50,
        },
        Row {
            case: "a request filter leaving fewer candidates than numResults",
            campaign: CAMPAIGN_FULL,
            catalog: items(FULL_CATALOG),
            queried_item: None,
            request_filter: Some((FILTER_COMEDY, comedy)),
            promotion_filter: (FILTER_EVERY_ENTRY, every_entry),
            num_results: 37,
            percent: 30,
        },
        Row {
            case: "the smallest share of a filtered response, rounded up",
            campaign: CAMPAIGN_FULL,
            catalog: items(FULL_CATALOG),
            queried_item: None,
            request_filter: Some((FILTER_COMEDY, comedy)),
            promotion_filter: (FILTER_EVERY_ENTRY, every_entry),
            num_results: registry::MAX_NUM_RESULTS,
            percent: 1,
        },
        Row {
            case: "a request filter leaving one candidate",
            campaign: CAMPAIGN_FULL,
            catalog: items(FULL_CATALOG),
            queried_item: None,
            request_filter: Some((FILTER_NEWEST, newest)),
            promotion_filter: (FILTER_EVERY_ENTRY, every_entry),
            num_results: 10,
            percent: 50,
        },
        Row {
            case: "a related-items response, which never reports the queried item",
            campaign: CAMPAIGN_RELATED,
            catalog: items(RELATED_CATALOG),
            queried_item: Some("item-0"),
            request_filter: None,
            promotion_filter: (FILTER_EVERY_ENTRY, every_entry),
            num_results: 50,
            percent: 50,
        },
        Row {
            case: "a promotion filter narrower than its own share of a short response",
            campaign: CAMPAIGN_TINY,
            catalog: items(TINY_CATALOG),
            queried_item: None,
            request_filter: None,
            promotion_filter: (FILTER_COMEDY, comedy),
            num_results: 20,
            percent: 100,
        },
        Row {
            case: "the control: as many candidates as numResults",
            campaign: CAMPAIGN_FULL,
            catalog: items(FULL_CATALOG),
            queried_item: None,
            request_filter: None,
            promotion_filter: (FILTER_EVERY_ENTRY, every_entry),
            num_results: 8,
            percent: 50,
        },
        Row {
            case: "the control: every candidate of a filtered response requested",
            campaign: CAMPAIGN_FULL,
            catalog: items(FULL_CATALOG),
            queried_item: None,
            request_filter: Some((FILTER_COMEDY, comedy)),
            promotion_filter: (FILTER_EVERY_ENTRY, every_entry),
            num_results: 10,
            percent: 30,
        },
    ];

    for row in rows {
        // What the response can report: the stored catalog, minus the item a
        // related-items request queried, minus what the request's filter drops.
        let candidates: Vec<&Item> = row
            .catalog
            .iter()
            .filter(|item| {
                row.queried_item
                    .is_none_or(|queried| item.item_id != queried)
            })
            .filter(|item| row.request_filter.is_none_or(|(_, keeps)| keeps(item)))
            .collect();
        let reported = row.num_results.min(candidates.len());
        // The percentage is taken of the entries the response reports, rounded up so a
        // promotion the caller asked for is applied to at least one of them; the
        // promotion's own filter can leave fewer entries to promote than that.
        let share = (reported * row.percent).div_ceil(100);
        let promotable = candidates
            .iter()
            .filter(|item| (row.promotion_filter.1)(item))
            .count();
        let expected_promoted = share.min(promotable);

        let mut body = json!({
            "campaignArn": row.campaign,
            "numResults": row.num_results,
            "promotions": [{
                "name": "promo-share",
                "percentPromotedItems": row.percent,
                "filterArn": row.promotion_filter.0,
            }],
        });
        match row.queried_item {
            Some(item_id) => body["itemId"] = json!(item_id),
            None => body["userId"] = json!("user-share"),
        }
        if let Some((filter_arn, _)) = row.request_filter {
            body["filterArn"] = json!(filter_arn);
        }

        let returned = entries(&emulator.ok(RECOMMENDATIONS, body).await, "itemList");
        assert_eq!(
            returned.len(),
            reported,
            "case {}: expected {reported} reported entries, got {returned:?}",
            row.case
        );
        let promoted: Vec<&Value> = returned
            .iter()
            .filter(|entry| entry.get("promotionName").is_some())
            .collect();
        assert_eq!(
            promoted.len(),
            expected_promoted,
            "case {}: a promotion of {}% of {reported} reported entries takes \
             {expected_promoted} of them, got {promoted:?}",
            row.case,
            row.percent
        );
        for entry in &promoted {
            assert_eq!(
                entry.get("promotionName").and_then(Value::as_str),
                Some("promo-share"),
                "case {}: a promoted entry carries the promotion's name, got {entry}",
                row.case
            );
            assert_eq!(
                entry.get("reason").and_then(Value::as_array),
                Some(&vec![json!(REASON_PROMOTED_ITEM)]),
                "case {}: a promoted entry carries the documented reason, got {entry}",
                row.case
            );
            let item_id = entry
                .get("itemId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let stored = candidates
                .iter()
                .find(|item| item.item_id == item_id)
                .unwrap_or_else(|| {
                    panic!("case {}: {item_id} is no candidate of this request", row.case)
                });
            assert!(
                (row.promotion_filter.1)(stored),
                "case {}: a promoted entry satisfies the promotion's filter, got \
                 {item_id} with {:?}",
                row.case,
                stored.attributes
            );
        }
        for entry in returned
            .iter()
            .filter(|entry| entry.get("promotionName").is_none())
        {
            assert_eq!(
                entry.get("reason"),
                None,
                "case {}: an entry outside the promotion carries no reason, got {entry}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_shorter_response_never_promotes_a_larger_fraction_than_the_request_named() {
    let emulator = seeded();

    // The invariant behind the rule: whatever shortens the response, the promoted
    // entries are at most the requested percentage of the reported ones (plus the one
    // entry rounding up can add). The rows walk every percentage the member models
    // (`range 1..100`, from `smithy_query(query="shape", shape="Promotion")`) at both
    // published bounds and across the middle, against a response the catalog itself
    // keeps short.
    for percent in [1_usize, 2, 25, 33, 50, 67, 99, 100] {
        let case = format!("{percent}% of a response shorter than numResults");
        let returned = entries(
            &emulator
                .ok(
                    RECOMMENDATIONS,
                    json!({
                        "campaignArn": CAMPAIGN_ODD,
                        "userId": "user-fraction",
                        "numResults": registry::MAX_NUM_RESULTS,
                        "promotions": [{
                            "name": "promo-fraction",
                            "percentPromotedItems": percent,
                            "filterArn": FILTER_EVERY_ENTRY,
                        }],
                    }),
                )
                .await,
            "itemList",
        );
        assert_eq!(
            returned.len(),
            ODD_CATALOG,
            "case {case}: the whole stored catalog is what can be reported, got {returned:?}"
        );
        let promoted = returned
            .iter()
            .filter(|entry| entry.get("promotionName").is_some())
            .count();
        assert_eq!(
            promoted,
            (returned.len() * percent).div_ceil(100),
            "case {case}: the promotion takes its percentage of the {} reported \
             entries, got {promoted}",
            returned.len()
        );
        assert!(
            promoted * 100 <= returned.len() * percent + 100,
            "case {case}: the promoted fraction must not exceed the requested \
             percentage beyond one rounded-up entry, got {promoted} of {}",
            returned.len()
        );
    }
}
