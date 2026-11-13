// SPDX-License-Identifier: CC0-1.0

//! A prediction score is a function of the request's inputs AS A TUPLE: two requests
//! whose inputs differ must be scored differently, and two requests whose inputs are
//! the same tuple identically — whatever bytes those inputs carry.
//!
//! The inputs a score is seeded from are strings a caller fully controls and whose
//! shapes bound only their LENGTH: `smithy_query(query="shape", shape="Context")` gives
//! the map's key an `AttributeName` pattern (`^[A-Za-z\d_]+$`) but its value an
//! `AttributeValue` that carries length `0..1000` and no pattern at all, and `UserID` /
//! `ItemID` are likewise length-only. So no byte is reserved for the emulator to
//! compose seed parts with, and a composition that delimits parts lets a value carrying
//! the delimiter imitate a part boundary: two different requests would then present one
//! seed and share an ordering and scores.
//!
//! The rows differ along what the rule generalizes over: which operation serves the
//! request (both operations that model `context`), where the part boundary falls (a map
//! entry boundary swallowed into the preceding value, a key/value boundary, a boundary
//! between the personalization input and the context), how many keys the map holds, and
//! — in [`every_carving_of_one_context_text_seeds_its_own_score`] — EVERY way of
//! carving one pair text into a context map, enumerated exhaustively rather than
//! sampled.

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, scoring, Campaign, RecipeType,
};

use crate::support::{
    assert_sums_to_one, items, scores, scores_by_id, Emulator, PERSONALIZE_RANKING,
    RECOMMENDATIONS,
};

/// The `USER_PERSONALIZATION` campaign `GetRecommendations` reads here.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/boundary-user";
/// The `PERSONALIZED_RANKING` campaign `GetPersonalizedRanking` re-ranks here.
const CAMPAIGN_RANKING: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/boundary-ranking";

/// How many entries the seeded catalogs hold.
const CATALOG_SIZE: usize = 8;

/// The user every row personalizes for, unless the row varies the user itself.
const USER: &str = "user-7";

/// The byte the module composed seed parts with before this rule was stated (ASCII unit
/// separator). An `AttributeValue` and a `UserID` may both carry it, so a row sends it
/// inside a value on purpose.
const UNIT_SEPARATOR: char = '\u{1f}';

/// Two scores may not differ at all for inputs that are the same tuple, so they are
/// compared exactly up to double-rounding noise.
const TOLERANCE: f64 = 1e-12;

/// An emulator holding both campaigns, over one catalog.
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
                Campaign::new(arn, "boundary", recipe_type, items(CATALOG_SIZE))
            ),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    emulator
}

/// Every id of the seeded catalog.
fn catalog_ids() -> Vec<String> {
    items(CATALOG_SIZE)
        .into_iter()
        .map(|item| item.item_id)
        .collect()
}

/// The response member a route reports its scored entries in.
fn list_member(route: &str) -> &'static str {
    match route {
        RECOMMENDATIONS => "itemList",
        PERSONALIZE_RANKING => "personalizedRanking",
        other => panic!("case {other}: not a route that models a context"),
    }
}

/// A request to `route` that reports the WHOLE scored catalog, personalizing for
/// `user_id` with `context` as sent (a `context` of `Value::Null` is not sent at all).
fn request(route: &str, user_id: &str, context: Value) -> Value {
    let mut body = match route {
        RECOMMENDATIONS => json!({
            "campaignArn": CAMPAIGN_USER,
            "userId": user_id,
            "numResults": CATALOG_SIZE,
        }),
        PERSONALIZE_RANKING => json!({
            "campaignArn": CAMPAIGN_RANKING,
            "userId": user_id,
            "inputList": catalog_ids(),
        }),
        other => panic!("case {other}: not a route that models a context"),
    };
    if !context.is_null() {
        body["context"] = context;
    }
    body
}

/// The score each entry of a response carries, keyed by entry id, plus the invariant
/// every one of these responses keeps: a response that reports the whole scored catalog
/// is a descending ranking whose shares sum to 1.0.
async fn reported(emulator: &Emulator, case: &str, route: &str, body: Value) -> BTreeMap<String, f64> {
    let member = list_member(route);
    let response = emulator.ok(route, body).await;
    assert_sums_to_one(case, &scores(&response, member));
    scores_by_id(&response, member, "itemId")
}

/// One row: two requests, and whether their inputs are the same tuple.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// The operation's modelled route.
    route: &'static str,
    /// The personalization input and the `context` of the first request.
    left: (&'static str, Value),
    /// Those of the second request.
    right: (&'static str, Value),
}

/// Rows whose two requests carry DIFFERENT input tuples, so the two responses may not
/// report one entry identically.
fn distinct_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for route in [RECOMMENDATIONS, PERSONALIZE_RANKING] {
        rows.extend([
            Row {
                case: "a map entry boundary swallowed into the preceding value",
                route,
                left: (USER, json!({"ALPHA": "one", "BETA": "two"})),
                right: (USER, json!({"ALPHA": "one,BETA=two"})),
            },
            Row {
                case: "one of two entry boundaries swallowed, out of a three-key map",
                route,
                left: (
                    USER,
                    json!({"ALPHA": "one", "BETA": "two", "GAMMA": "three"}),
                ),
                right: (USER, json!({"ALPHA": "one,BETA=two", "GAMMA": "three"})),
            },
            Row {
                case: "every entry boundary of a three-key map swallowed",
                route,
                left: (
                    USER,
                    json!({"ALPHA": "one", "BETA": "two", "GAMMA": "three"}),
                ),
                right: (USER, json!({"ALPHA": "one,BETA=two,GAMMA=three"})),
            },
            Row {
                case: "a key/value boundary moved into the value",
                route,
                left: (USER, json!({"ALPHA_ONE": "two"})),
                right: (USER, json!({"ALPHA": "ONE=two"})),
            },
            Row {
                case: "the boundary between the personalization input and the context",
                route,
                left: (USER, json!({"ALPHA": format!("one{UNIT_SEPARATOR}")})),
                right: (
                    // The same text, carved one part earlier: the user id absorbs the
                    // context and the request sends no context at all.
                    concat!("user-7", "\u{1f}", "ALPHA=one"),
                    Value::Null,
                ),
            },
            Row {
                case: "a value that differs only past an embedded separator",
                route,
                left: (USER, json!({"ALPHA": format!("one{UNIT_SEPARATOR}two")})),
                right: (USER, json!({"ALPHA": format!("one{UNIT_SEPARATOR}three")})),
            },
        ]);
    }
    rows
}

/// Rows whose two requests carry the SAME input tuple, so the two responses must report
/// every entry identically — the other half of the rule: a seed is a function of the
/// inputs, not of how they arrived.
fn equal_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for route in [RECOMMENDATIONS, PERSONALIZE_RANKING] {
        rows.extend([
            Row {
                case: "the same two-key map, the keys sent in the other order",
                route,
                left: (USER, json!({"ALPHA": "one", "BETA": "two"})),
                right: (USER, json!({"BETA": "two", "ALPHA": "one"})),
            },
            Row {
                case: "the same map whose value carries both former delimiters",
                route,
                left: (USER, json!({"ALPHA": "one,BETA=two"})),
                right: (USER, json!({"ALPHA": "one,BETA=two"})),
            },
            Row {
                case: "an empty map against no context member at all",
                route,
                left: (USER, json!({})),
                right: (USER, Value::Null),
            },
        ]);
    }
    rows
}

#[tokio::test]
async fn requests_whose_context_differs_are_scored_differently() {
    let emulator = seeded();
    for row in distinct_rows() {
        let case = format!("{} ({})", row.case, row.route);
        let left = reported(
            &emulator,
            &case,
            row.route,
            request(row.route, row.left.0, row.left.1.clone()),
        )
        .await;
        let right = reported(
            &emulator,
            &case,
            row.route,
            request(row.route, row.right.0, row.right.1.clone()),
        )
        .await;
        let differs = left
            .keys()
            .filter(|id| right.contains_key(*id))
            .any(|id| (left[id] - right[id]).abs() > TOLERANCE);
        assert!(
            differs,
            "case {case}: the two requests carry different seed inputs, so they must not \
             share one score per entry, got {left:?} and {right:?}"
        );
    }
}

#[tokio::test]
async fn requests_whose_context_is_the_same_tuple_are_scored_identically() {
    let emulator = seeded();
    for row in equal_rows() {
        let case = format!("{} ({})", row.case, row.route);
        let left = reported(
            &emulator,
            &case,
            row.route,
            request(row.route, row.left.0, row.left.1.clone()),
        )
        .await;
        let right = reported(
            &emulator,
            &case,
            row.route,
            request(row.route, row.right.0, row.right.1.clone()),
        )
        .await;
        assert_eq!(
            left.keys().collect::<Vec<_>>(),
            right.keys().collect::<Vec<_>>(),
            "case {case}: equal inputs must report the same entries, got {left:?} and \
             {right:?}"
        );
        for (id, score) in &left {
            let other = right[id];
            assert!(
                (score - other).abs() <= TOLERANCE,
                "case {case}: equal inputs must report entry {id} identically, got {score} \
                 and {other}"
            );
        }
    }
}

/// The pairs the carved context maps are built from: keys spelled the way
/// `AttributeName` requires (`^[A-Za-z\d_]+$`) and values a caller may spell any way
/// `AttributeValue` permits (length only).
const CARVED_PAIRS: [(&str, &str); 4] = [
    ("ALPHA", "one"),
    ("BETA", "two"),
    ("GAMMA", "three"),
    ("DELTA", "four"),
];

/// Every way of carving [`CARVED_PAIRS`] into a context map: each boundary between two
/// consecutive pairs is either a boundary between two map entries, or swallowed into the
/// preceding VALUE spelled with the `=` and `,` a delimiting renderer would use.
///
/// All `2^(n-1)` carvings are DIFFERENT maps — they hold different keys and different
/// values — so a score that is a function of the map must give each carving its own.
fn carved_contexts() -> Vec<(String, HashMap<String, String>)> {
    let boundaries = CARVED_PAIRS.len() - 1;
    (0..1_u32 << boundaries)
        .map(|mask| {
            let mut map = HashMap::new();
            let (mut key, mut value) = (CARVED_PAIRS[0].0.to_string(), CARVED_PAIRS[0].1.to_string());
            for (at, (next_key, next_value)) in CARVED_PAIRS.iter().enumerate().skip(1) {
                if ((mask >> (at - 1)) & 1) == 1 {
                    value.push(',');
                    value.push_str(next_key);
                    value.push('=');
                    value.push_str(next_value);
                } else {
                    map.insert(std::mem::take(&mut key), std::mem::take(&mut value));
                    key = (*next_key).to_string();
                    value = (*next_value).to_string();
                }
            }
            map.insert(key, value);
            (format!("carving {mask:03b}"), map)
        })
        .collect()
}

/// The invariant behind both suites above, over the whole enumerated family rather than
/// a pair of it: the seed every operation scores through is INJECTIVE in the context
/// map, so each of the carvings scores one stored entry its own way.
#[test]
fn every_carving_of_one_context_text_seeds_its_own_score() {
    let carvings = carved_contexts();
    assert_eq!(
        carvings.len(),
        1 << (CARVED_PAIRS.len() - 1),
        "case enumeration: every boundary of the pair text must be carved both ways, got \
         {carvings:?}"
    );
    let entry = &catalog_ids()[0];
    let mut scored: Vec<(&str, BTreeMap<&str, &str>, f64)> = Vec::new();
    for (case, context) in &carvings {
        let sorted: BTreeMap<&str, &str> = context
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let score = scoring::Seed::with_context(CAMPAIGN_USER, USER, Some(context))
            .relevance(1.0, entry);
        for (other_case, other_map, other_score) in &scored {
            assert_ne!(
                &sorted, other_map,
                "case {case}: every carving is a different map, but it equals {other_case}"
            );
            assert!(
                (score - other_score).abs() > TOLERANCE,
                "case {case}: a map that differs from {other_case} must seed its own score, \
                 got {score} and {other_score}"
            );
        }
        scored.push((case.as_str(), sorted, score));
    }
}
