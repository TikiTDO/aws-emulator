// SPDX-License-Identifier: CC0-1.0

//! `GetActionRecommendations` reads a `filterValues` entry the documented way: the
//! double quotes that enclose a value DELIMIT it, so what an `Actions.<COLUMN>` clause
//! compares against a stored Actions attribute is the text BETWEEN them.
//!
//! `GetActionRecommendationsRequest$filterValues` is documented as "For each
//! placeholder parameter in your filter expression, provide the parameter name (in
//! matching case) as a key and the filter value(s) as the corresponding value.
//! Separate multiple values for one parameter with a comma", and the request format
//! encloses a string value in double quotes (written escaped in JSON) while leaving a
//! numeric value bare. The same doc states the asymmetry between the two filter kinds:
//! an `INCLUDE` expression needs "values for all parameters that are defined in the
//! expression", while for an `EXCLUDE` expression "you can omit the filter-values. In
//! this case, Amazon Personalize doesn't use that portion of the expression to filter
//! recommendations".
//!
//! The suite below drives that rule over the `/action-recommendations` route as a
//! TABLE whose rows differ in the FORM of the entry — how many values it carries,
//! whether they are delimited, whether the JSON escaping was left in, how they are
//! spaced, whether a value carries the separator itself, whether it names one of
//! several values a column holds, and whether the clause compares strings or numbers —
//! and in the predicate the expression applies (`IN`, `NOT IN`, `>=`, `INCLUDE` vs
//! `EXCLUDE`). Every expected action set is computed from the SEEDED Actions catalog,
//! so no row can pass against a canned list.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Action, Campaign,
};

use crate::support::{
    assert_descending_and_bounded, ids, recommendation_id, scores, Emulator,
    ACTION_RECOMMENDATIONS,
};

/// The `PERSONALIZED_ACTIONS` campaign every row below recommends from.
const CAMPAIGN_ACTIONS: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/action-value-forms";

/// `INCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)`.
const FILTER_INCLUDE_CHANNEL: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/forms-include-channel";
/// `EXCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)`.
const FILTER_EXCLUDE_CHANNEL: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/forms-exclude-channel";
/// `INCLUDE ActionID WHERE Actions.CHANNEL NOT IN ($CHANNEL)`.
const FILTER_OTHER_CHANNELS: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/forms-other-channels";
/// `INCLUDE ActionID WHERE Actions.VALUE >= $VALUE`.
const FILTER_FROM_VALUE: &str =
    "arn:aws:personalize:us-east-1:000000000000:filter/forms-from-value";

/// The categorical Actions column the string rows filter on.
const COLUMN_CHANNEL: &str = "CHANNEL";
/// The numeric Actions column the comparison rows filter on.
const COLUMN_VALUE: &str = "VALUE";

/// The `CHANNEL` values the seeded Actions catalog cycles through.
///
/// Test data, chosen so the rows can bind every form a value takes: the third entry
/// carries the value SEPARATOR inside the value, and the fourth holds SEVERAL values
/// separated by `|` — the separator a Personalize Actions/Items CSV uses within one
/// column, which `state::filter`'s module documentation describes, and which `IN`
/// matches when ANY of them matches.
const CHANNELS: [&str; 4] = ["App", "Email", "Push, SMS", "App|Web"];

/// How many entries the seeded Actions catalog holds — a multiple of [`CHANNELS`], so
/// every channel form is carried by more than one action.
const ACTION_CATALOG_SIZE: usize = 12;

/// How far apart two neighbouring actions' `VALUE` columns sit.
const VALUE_STEP: i64 = 2;

/// The Actions catalog these rows filter.
///
/// `base_score` falls with the index, so the stored popularity order is known to the
/// test, and `VALUE` rises with it, so a `>= $VALUE` clause selects a known suffix.
fn action_catalog() -> Vec<Action> {
    (0..ACTION_CATALOG_SIZE)
        .map(|index| {
            Action::new(format!("action-{index}"), (ACTION_CATALOG_SIZE - index) as f64)
                .with_attribute(COLUMN_CHANNEL, CHANNELS[index % CHANNELS.len()])
                .with_attribute(COLUMN_VALUE, (index as i64 * VALUE_STEP).to_string())
        })
        .collect()
}

/// Whether a stored action's `CHANNEL` column holds `wanted` — the column may carry
/// several values separated by `|`, and `IN` matches when any of them does.
fn on_channel(action: &Action, wanted: &str) -> bool {
    action
        .attributes
        .get(COLUMN_CHANNEL)
        .is_some_and(|stored| stored.split('|').any(|held| held == wanted))
}

/// The stored `VALUE` of one action, as a number.
fn value(action: &Action) -> i64 {
    action
        .attributes
        .get(COLUMN_VALUE)
        .and_then(|held| held.parse().ok())
        .unwrap_or_default()
}

/// An emulator holding the actions campaign and the four placeholder filters.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    assert!(
        registry::register_campaign(
            engine,
            Campaign::new_actions(CAMPAIGN_ACTIONS, "action-value-forms", action_catalog()),
        ),
        "the campaign seam must register the actions campaign"
    );

    let filters = [
        (
            FILTER_INCLUDE_CHANNEL,
            "INCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)",
        ),
        (
            FILTER_EXCLUDE_CHANNEL,
            "EXCLUDE ActionID WHERE Actions.CHANNEL IN ($CHANNEL)",
        ),
        (
            FILTER_OTHER_CHANNELS,
            "INCLUDE ActionID WHERE Actions.CHANNEL NOT IN ($CHANNEL)",
        ),
        (
            FILTER_FROM_VALUE,
            "INCLUDE ActionID WHERE Actions.VALUE >= $VALUE",
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

/// What a row's filter, bound to that row's `filterValues`, keeps.
type Retains = fn(&Action) -> bool;

/// One row: its name, the filter it names, the `filterValues` it sends, and which
/// stored actions that combination keeps.
type Row = (&'static str, &'static str, Value, Retains);

/// Every form of `filterValues` entry the rule covers, with the action subset each
/// selects.
///
/// The JSON string a row sends is what a client writes once JSON escaping is undone:
/// the documented `"CHANNEL": "\"App\""` arrives at the handler as `"App"`, quote
/// characters included.
fn rows() -> Vec<Row> {
    vec![
        (
            "one quoted value",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": "\"App\"" }),
            |action| on_channel(action, "App"),
        ),
        (
            "a quoted value whose JSON escaping was left in",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": "\\\"App\\\"" }),
            |action| on_channel(action, "App"),
        ),
        (
            "two quoted values separated by a comma",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": "\"App\",\"Email\"" }),
            |action| on_channel(action, "App") || on_channel(action, "Email"),
        ),
        (
            "quoted values spaced around the separator",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": " \"Email\" , \"Web\" " }),
            |action| on_channel(action, "Email") || on_channel(action, "Web"),
        ),
        (
            "a quoted value carrying the separator itself",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": "\"Push, SMS\"" }),
            |action| on_channel(action, "Push, SMS"),
        ),
        (
            "a quoted value naming one of several values the column holds",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": "\"Web\"" }),
            |action| on_channel(action, "Web"),
        ),
        (
            "an undelimited value",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": "Email" }),
            |action| on_channel(action, "Email"),
        ),
        (
            // The one row whose expected set is EMPTY: an INCLUDE expression keeps
            // nothing only when the value it is bound to names nothing stored.
            "a quoted value no stored action carries",
            FILTER_INCLUDE_CHANNEL,
            json!({ "CHANNEL": "\"Fax\"" }),
            |_| false,
        ),
        (
            "EXCLUDE with one quoted value",
            FILTER_EXCLUDE_CHANNEL,
            json!({ "CHANNEL": "\"App\"" }),
            |action| !on_channel(action, "App"),
        ),
        (
            "EXCLUDE with three quoted values",
            FILTER_EXCLUDE_CHANNEL,
            json!({ "CHANNEL": "\"App\",\"Email\",\"Web\"" }),
            |action| {
                !on_channel(action, "App")
                    && !on_channel(action, "Email")
                    && !on_channel(action, "Web")
            },
        ),
        (
            "NOT IN with a quoted value",
            FILTER_OTHER_CHANNELS,
            json!({ "CHANNEL": "\"App\"" }),
            |action| !on_channel(action, "App"),
        ),
        (
            "a bare number in a comparison",
            FILTER_FROM_VALUE,
            json!({ "VALUE": "6" }),
            |action| value(action) >= 6,
        ),
        (
            "a quoted number in a comparison",
            FILTER_FROM_VALUE,
            json!({ "VALUE": "\"6\"" }),
            |action| value(action) >= 6,
        ),
        (
            "a bare number past every stored value",
            FILTER_FROM_VALUE,
            json!({ "VALUE": "1000" }),
            |action| value(action) >= 1000,
        ),
    ]
}

#[tokio::test]
async fn action_recommendations_keep_exactly_the_actions_the_bound_values_name() {
    let emulator = seeded();
    let stored = action_catalog();

    for (case, filter_arn, filter_values, retains) in rows() {
        let expected: Vec<String> = stored
            .iter()
            .filter(|action| retains(action))
            .map(|action| action.action_id.clone())
            .collect();
        assert!(
            expected.len() < stored.len(),
            "case {case}: the row must select a strict subset of the catalog, got {expected:?}"
        );

        let response = emulator
            .ok(
                ACTION_RECOMMENDATIONS,
                json!({
                    "campaignArn": CAMPAIGN_ACTIONS,
                    "userId": "user-forms",
                    // The whole catalog, so the response shows exactly what the bound
                    // filter removed rather than what `numResults` truncated.
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
        assert_descending_and_bounded(case, &scores(&response, "actionList"));
        recommendation_id(case, &response);
    }
}

#[tokio::test]
async fn an_action_exclude_expression_with_no_bound_values_removes_nothing() {
    let emulator = seeded();
    let stored = action_catalog();
    let every_action: Vec<String> = stored
        .iter()
        .map(|action| action.action_id.clone())
        .collect();

    // "For filters with expressions that use an EXCLUDE element to exclude actions, you
    // can omit the filter-values" — the rows differ in how the request omits them.
    let rows: [(&str, Value); 3] = [
        ("filterValues absent", json!(null)),
        ("filterValues empty", json!({})),
        (
            "filterValues binding another parameter",
            json!({ "OTHER": "\"App\"" }),
        ),
    ];

    for (case, filter_values) in rows {
        let mut request = json!({
            "campaignArn": CAMPAIGN_ACTIONS,
            "userId": "user-forms",
            "numResults": ACTION_CATALOG_SIZE,
            "filterArn": FILTER_EXCLUDE_CHANNEL,
        });
        if !filter_values.is_null() {
            request["filterValues"] = filter_values;
        }

        let response = emulator.ok(ACTION_RECOMMENDATIONS, request).await;
        let mut returned = ids(&response, "actionList", "actionId");
        returned.sort();
        let mut expected = every_action.clone();
        expected.sort();
        assert_eq!(
            returned, expected,
            "case {case}: an EXCLUDE expression with no bound value must remove nothing, \
             got {returned:?}"
        );
    }
}

#[tokio::test]
async fn an_action_include_expression_with_no_bound_value_is_refused() {
    let emulator = seeded();

    // "For filter expressions that use an INCLUDE element to include actions, you must
    // provide values for all parameters that are defined in the expression": the rows
    // differ in which INCLUDE expression is left unbound and in how the request fails
    // to bind it.
    let rows: [(&str, &str, Value); 4] = [
        ("IN with filterValues absent", FILTER_INCLUDE_CHANNEL, json!(null)),
        ("IN with filterValues empty", FILTER_INCLUDE_CHANNEL, json!({})),
        (
            "IN with another parameter bound",
            FILTER_INCLUDE_CHANNEL,
            json!({ "OTHER": "\"App\"" }),
        ),
        ("a comparison left unbound", FILTER_FROM_VALUE, json!({})),
    ];

    for (case, filter_arn, filter_values) in rows {
        let mut request = json!({
            "campaignArn": CAMPAIGN_ACTIONS,
            "userId": "user-forms",
            "numResults": ACTION_CATALOG_SIZE,
            "filterArn": filter_arn,
        });
        if !filter_values.is_null() {
            request["filterValues"] = filter_values;
        }

        let refusal = emulator.refusal(ACTION_RECOMMENDATIONS, request).await;
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
