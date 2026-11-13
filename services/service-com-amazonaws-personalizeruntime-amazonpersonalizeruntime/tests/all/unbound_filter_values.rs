// SPDX-License-Identifier: CC0-1.0

//! What happens when a request names a filter expression parameter but binds it to no
//! value at all.
//!
//! The `filterValues` documentation states one rule for a parameter the request does
//! not bind, and the rule does not care HOW the request failed to bind it: "For filter
//! expressions that use an INCLUDE element to include items, you must provide values
//! for all parameters that are defined in the expression", while "for filters with
//! expressions that use an EXCLUDE element ... you can omit the filter-values. In this
//! case, Amazon Personalize doesn't use that portion of the expression to filter
//! recommendations".
//!
//! A `filterValues` entry holds "the filter value(s)" of one parameter, separated by
//! commas; text that names none of them — nothing, only whitespace, or only separators
//! — supplies no value, so such an entry leaves its parameter exactly as unbound as an
//! absent key. `FilterAttributeValue` is modelled `length 0..1000`
//! (`smithy_query(query="shape", shape="FilterAttributeValue")`), so the empty string
//! reaches a handler and the handler owes it that rule.
//!
//! The suites below drive the rule as TABLES whose rows differ in the FORM of the
//! entry that names no value, in the PREDICATE whose parameter it leaves unbound (`IN`,
//! `NOT IN`, a comparison), in how many clauses the expression carries and which of
//! them stays bound, in the filter KIND, and in the route the request takes — all three
//! runtime operations plus a promotion's own `filterValues`. A DELIMITED empty value is
//! the contrast row: the caller wrote a value, so the request is served.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, Item, RecipeType,
};

use crate::support::{
    actions, ids, items, Emulator, ACTION_RECOMMENDATIONS, COLUMN_GENRE, COLUMN_YEAR,
    PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// The `USER_PERSONALIZATION` campaign the `GetRecommendations` rows read.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/unbound-user";
/// The `PERSONALIZED_RANKING` campaign the `GetPersonalizedRanking` rows re-rank.
const CAMPAIGN_RANKING: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/unbound-ranking";
/// The `PERSONALIZED_ACTIONS` campaign the `GetActionRecommendations` rows read.
const CAMPAIGN_ACTIONS: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/unbound-actions";

/// `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)`.
const INCLUDE_GENRE: &str = "arn:aws:personalize:us-east-1:000000000000:filter/unbound-in";
/// `INCLUDE ItemID WHERE Items.YEAR >= $YEAR`.
const INCLUDE_FROM_YEAR: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/unbound-compare";
/// `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE) AND Items.YEAR >= $YEAR`.
const INCLUDE_GENRE_AND_YEAR: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/unbound-two-clauses";
/// `EXCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)`.
const EXCLUDE_NOT_IN_GENRE: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/unbound-exclude-not-in";
/// `EXCLUDE ItemID WHERE Items.YEAR >= $YEAR`.
const EXCLUDE_FROM_YEAR: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/unbound-exclude-compare";
/// `EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE) AND Items.YEAR >= $YEAR`.
const EXCLUDE_GENRE_AND_YEAR: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/unbound-exclude-two-clauses";
/// `INCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)`.
const INCLUDE_CHANNEL: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/unbound-include-channel";
/// `EXCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)`.
const EXCLUDE_CHANNEL: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/unbound-exclude-channel";

/// How many Items the seeded catalogs hold.
const CATALOG_SIZE: usize = 12;
/// How many Actions the seeded actions campaign holds.
const ACTION_CATALOG_SIZE: usize = 6;

/// The `GENRE` one seeded entry carries, used as the value that STAYS bound in a
/// two-clause row — the catalog's first genre, read from the catalog itself so no row
/// depends on a literal.
fn bound_genre() -> String {
    items(CATALOG_SIZE)[0]
        .attributes
        .get(COLUMN_GENRE)
        .cloned()
        .expect("the seeded catalog carries a GENRE column")
}

/// The `YEAR` the seeded catalog's first entry carries, used as the value that STAYS
/// bound in a two-clause row.
fn bound_year() -> String {
    items(CATALOG_SIZE)[0]
        .attributes
        .get(COLUMN_YEAR)
        .cloned()
        .expect("the seeded catalog carries a YEAR column")
}

/// Whether a stored entry's `GENRE` column holds `wanted`.
fn holds_genre(item: &Item, wanted: &str) -> bool {
    item.attributes
        .get(COLUMN_GENRE)
        .is_some_and(|stored| stored.split('|').any(|held| held == wanted))
}

/// Every id the seeded Items catalog holds.
fn every_item() -> Vec<String> {
    items(CATALOG_SIZE)
        .iter()
        .map(|item| item.item_id.clone())
        .collect()
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
                Campaign::new(arn, "unbound", recipe_type, items(CATALOG_SIZE)),
            ),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new_actions(
                CAMPAIGN_ACTIONS,
                "unbound-actions",
                actions(ACTION_CATALOG_SIZE),
            ),
        ),
        "the campaign seam must register the actions campaign"
    );

    let filters = [
        (INCLUDE_GENRE, "INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)"),
        (INCLUDE_FROM_YEAR, "INCLUDE ItemID WHERE Items.YEAR >= $YEAR"),
        (
            INCLUDE_GENRE_AND_YEAR,
            "INCLUDE ItemID WHERE Items.GENRE IN ($GENRE) AND Items.YEAR >= $YEAR",
        ),
        (
            EXCLUDE_NOT_IN_GENRE,
            "EXCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)",
        ),
        (EXCLUDE_FROM_YEAR, "EXCLUDE ItemID WHERE Items.YEAR >= $YEAR"),
        (
            EXCLUDE_GENRE_AND_YEAR,
            "EXCLUDE ItemID WHERE Items.GENRE IN ($GENRE) AND Items.YEAR >= $YEAR",
        ),
        (
            INCLUDE_CHANNEL,
            "INCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)",
        ),
        (
            EXCLUDE_CHANNEL,
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

/// One route row: its name, the route, the request body, the response list member and
/// its id member, and the ids that list must hold.
type RouteRow = (
    &'static str,
    &'static str,
    Value,
    &'static str,
    &'static str,
    Vec<String>,
);

/// Every form in which a request names no value for a parameter: `None` leaves the key
/// out, and each `Some` supplies text that carries no value between its separators.
const NAMES_NO_VALUE: [(&str, Option<&str>); 7] = [
    ("the key absent", None),
    ("the empty string", Some("")),
    ("a space", Some(" ")),
    ("whitespace only", Some("  \t ")),
    ("one bare separator", Some(",")),
    ("several bare separators", Some(",,,")),
    ("a separator surrounded by whitespace", Some(" , ")),
];

/// A `filterValues` object binding `parameter` the way one [`NAMES_NO_VALUE`] form
/// does, on top of whatever `already_bound` the row keeps bound.
fn filter_values(parameter: &str, form: Option<&str>, already_bound: &Value) -> Value {
    let mut values = already_bound.clone();
    if let Some(raw) = form {
        values[parameter] = json!(raw);
    }
    values
}

#[tokio::test]
async fn an_include_parameter_the_request_names_no_value_for_is_refused() {
    let emulator = seeded();

    // The rows differ in the predicate the unbound parameter sits in, and in whether
    // the expression's OTHER clause is bound: "you must provide values for all
    // parameters that are defined in the expression", so one unbound parameter is
    // enough whatever else the request bound.
    let targets: [(&str, &str, &str, Value); 4] = [
        (
            "an IN set whose operand is the parameter",
            INCLUDE_GENRE,
            COLUMN_GENRE,
            json!({}),
        ),
        (
            "a comparison against the parameter",
            INCLUDE_FROM_YEAR,
            COLUMN_YEAR,
            json!({}),
        ),
        (
            "the comparison of two clauses, the IN clause bound",
            INCLUDE_GENRE_AND_YEAR,
            COLUMN_YEAR,
            json!({ COLUMN_GENRE: format!("\"{}\"", bound_genre()) }),
        ),
        (
            "the IN clause of two, the comparison bound",
            INCLUDE_GENRE_AND_YEAR,
            COLUMN_GENRE,
            json!({ COLUMN_YEAR: bound_year() }),
        ),
    ];

    for (target, filter_arn, parameter, already_bound) in targets {
        for (form, raw) in NAMES_NO_VALUE {
            let case = format!("{target} bound to {form}");
            let refusal = emulator
                .refusal(
                    RECOMMENDATIONS,
                    json!({
                        "campaignArn": CAMPAIGN_USER,
                        "userId": "user-unbound",
                        "numResults": CATALOG_SIZE,
                        "filterArn": filter_arn,
                        "filterValues": filter_values(parameter, raw, &already_bound),
                    }),
                )
                .await;

            assert_eq!(
                (refusal.status, refusal.code.as_str()),
                (400, "InvalidInputException"),
                "case {case}: an unbound INCLUDE parameter is invalid input, got {refusal:?}"
            );
            assert!(
                refusal.message.contains("filterValues"),
                "case {case}: the refusal must name the member it is built from, got {refusal:?}"
            );
            assert!(
                refusal.message.contains(&format!("${parameter}")),
                "case {case}: the refusal must name the parameter the expression requires, \
                 got {refusal:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_unbound_include_parameter_is_refused_on_every_route_that_binds_a_filter() {
    let emulator = seeded();
    let input_list = every_item();

    // The same unbound parameter, reached through each operation that binds a filter
    // expression, plus a promotion's own `filterValues` — one rule, four call sites.
    let rows: [(&str, &str, Value); 4] = [
        (
            "GetRecommendations",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-unbound",
                "numResults": CATALOG_SIZE,
                "filterArn": INCLUDE_GENRE,
                "filterValues": { COLUMN_GENRE: "" },
            }),
        ),
        (
            "GetRecommendations with a promotion",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-unbound",
                "numResults": CATALOG_SIZE,
                "promotions": [{
                    "name": "promo-unbound",
                    "percentPromotedItems": 50,
                    "filterArn": INCLUDE_GENRE,
                    "filterValues": { COLUMN_GENRE: "   " },
                }],
            }),
        ),
        (
            "GetPersonalizedRanking",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-unbound",
                "inputList": input_list,
                "filterArn": INCLUDE_FROM_YEAR,
                "filterValues": { COLUMN_YEAR: "," },
            }),
        ),
        (
            "GetActionRecommendations",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": "user-unbound",
                "numResults": ACTION_CATALOG_SIZE,
                "filterArn": INCLUDE_CHANNEL,
                "filterValues": { "CHANNEL": " , " },
            }),
        ),
    ];

    for (case, route, body) in rows {
        let refusal = emulator.refusal(route, body).await;
        assert_eq!(
            (refusal.status, refusal.code.as_str()),
            (400, "InvalidInputException"),
            "case {case}: an unbound INCLUDE parameter is invalid input, got {refusal:?}"
        );
        assert!(
            refusal.message.contains("filterValues"),
            "case {case}: the refusal must name the member it is built from, got {refusal:?}"
        );
    }
}

#[tokio::test]
async fn an_exclude_parameter_the_request_names_no_value_for_drops_only_its_clause() {
    let emulator = seeded();
    let stored = items(CATALOG_SIZE);
    let genre = bound_genre();

    // "Amazon Personalize doesn't use that portion of the expression to filter
    // recommendations": the rows differ in the predicate left unbound, in whether the
    // expression has another clause that stays bound, and in the form that names no
    // value. `expected` is computed from the clauses that REMAIN.
    type Retains = fn(&Item, &str) -> bool;
    let targets: [(&str, &str, &str, Value, Retains); 3] = [
        (
            "a NOT IN clause, the only clause",
            EXCLUDE_NOT_IN_GENRE,
            COLUMN_GENRE,
            json!({}),
            // No clause is left, so the expression excludes nothing.
            |_, _| true,
        ),
        (
            "a comparison, the only clause",
            EXCLUDE_FROM_YEAR,
            COLUMN_YEAR,
            json!({}),
            |_, _| true,
        ),
        (
            "the comparison of two clauses, the IN clause bound",
            EXCLUDE_GENRE_AND_YEAR,
            COLUMN_YEAR,
            json!({ COLUMN_GENRE: format!("\"{}\"", bound_genre()) }),
            // The IN clause survives on its own, so the entries it matches are removed.
            |item, genre| !holds_genre(item, genre),
        ),
    ];

    for (target, filter_arn, parameter, already_bound, retains) in targets {
        let expected: Vec<String> = stored
            .iter()
            .filter(|item| retains(item, &genre))
            .map(|item| item.item_id.clone())
            .collect();
        assert!(
            !expected.is_empty(),
            "case {target}: the row must keep something, got {expected:?}"
        );

        for (form, raw) in NAMES_NO_VALUE {
            let case = format!("{target} bound to {form}");
            let response = emulator
                .ok(
                    RECOMMENDATIONS,
                    json!({
                        "campaignArn": CAMPAIGN_USER,
                        "userId": "user-unbound",
                        "numResults": CATALOG_SIZE,
                        "filterArn": filter_arn,
                        "filterValues": filter_values(parameter, raw, &already_bound),
                    }),
                )
                .await;

            let mut returned = ids(&response, "itemList", "itemId");
            returned.sort();
            let mut wanted = expected.clone();
            wanted.sort();
            assert_eq!(
                returned, wanted,
                "case {case}: an EXCLUDE expression must apply only the clauses the \
                 request bound, got {returned:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_unbound_exclude_parameter_removes_nothing_on_every_route() {
    let emulator = seeded();
    let input_list = every_item();
    let every_action: Vec<String> = actions(ACTION_CATALOG_SIZE)
        .iter()
        .map(|action| action.action_id.clone())
        .collect();

    let rows: [RouteRow; 2] = [
        (
            "GetPersonalizedRanking",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-unbound",
                "inputList": input_list.clone(),
                "filterArn": EXCLUDE_FROM_YEAR,
                "filterValues": { COLUMN_YEAR: "" },
            }),
            "personalizedRanking",
            "itemId",
            input_list,
        ),
        (
            "GetActionRecommendations",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": "user-unbound",
                "numResults": ACTION_CATALOG_SIZE,
                "filterArn": EXCLUDE_CHANNEL,
                "filterValues": { "CHANNEL": " , " },
            }),
            "actionList",
            "actionId",
            every_action,
        ),
    ];

    for (case, route, body, member, id_member, expected) in rows {
        let response = emulator.ok(route, body).await;
        let mut returned = ids(&response, member, id_member);
        returned.sort();
        let mut wanted = expected;
        wanted.sort();
        assert_eq!(
            returned, wanted,
            "case {case}: an EXCLUDE expression the request left unbound must remove \
             nothing, got {returned:?}"
        );
    }
}

#[tokio::test]
async fn a_delimited_empty_value_binds_the_parameter_instead_of_leaving_it_unbound() {
    let emulator = seeded();
    let stored = items(CATALOG_SIZE);

    // The contrast row: the quotes DELIMIT a value, so an entry written as `""` names
    // one value — the empty string — and the request is served rather than refused. No
    // seeded entry stores an empty GENRE, so an INCLUDE keeps nothing and the matching
    // EXCLUDE keeps everything.
    for (case, raw) in [
        ("a delimited empty value", "\"\""),
        (
            "a delimited empty value whose JSON escaping was left in",
            "\\\"\\\"",
        ),
    ] {
        let included = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-unbound",
                    "numResults": CATALOG_SIZE,
                    "filterArn": INCLUDE_GENRE,
                    "filterValues": { COLUMN_GENRE: raw },
                }),
            )
            .await;
        assert_eq!(
            ids(&included, "itemList", "itemId"),
            Vec::<String>::new(),
            "case {case}: no stored entry holds the empty string, so an INCLUDE keeps \
             nothing, got {included}"
        );

        let excluded = emulator
            .ok(
                RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_USER,
                    "userId": "user-unbound",
                    "numResults": CATALOG_SIZE,
                    "filterArn": EXCLUDE_GENRE_AND_YEAR,
                    "filterValues": { COLUMN_GENRE: raw, COLUMN_YEAR: bound_year() },
                }),
            )
            .await;
        let mut returned = ids(&excluded, "itemList", "itemId");
        returned.sort();
        // The GENRE clause matches nothing, so only the bound YEAR comparison removes
        // entries: `GENRE IN ("") AND YEAR >= <first year>` holds for no entry.
        let mut wanted: Vec<String> = stored.iter().map(|item| item.item_id.clone()).collect();
        wanted.sort();
        assert_eq!(
            returned, wanted,
            "case {case}: the bound empty value matches no entry, so the EXCLUDE removes \
             nothing, got {returned:?}"
        );
    }
}
