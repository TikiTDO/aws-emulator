// SPDX-License-Identifier: CC0-1.0

//! How a refusal QUOTES the regular expression a value had to match.
//!
//! The rule: a constraint refusal states a `@pattern` requirement as the expression the
//! service PUBLISHES — the model's text with the whole-value anchor pair (`^` … `$`) it
//! wraps the expression in removed — while the CHECK still runs the anchored expression,
//! so a value that merely CONTAINS a match is refused. The expression is the requirement,
//! so the rule holds for every pattern-constrained member of this service and for every
//! clause count a refusal carries, not only for a refusal whose single clause is a
//! pattern.
//!
//! The rows below span both pattern-constrained members this model declares and the forms
//! a value can miss an expression by (too short, too long, wrong case, a character the
//! expression does not admit, a match that is not the whole value), plus a refusal that
//! states a pattern clause beside a length clause and refusals of the other constraint
//! classes, whose wording this rule does not touch.

use serde_json::{json, Value};

use super::pricing_support::{call, err, error_message, ok, router, ARN_PREFIX, JAN_1_2024};

/// The `@pattern` the model declares on `com.amazonaws.pricing#CurrencyCode`, as written
/// (`smithy_query` query="shape" shape="CurrencyCode").
const CURRENCY_CODE_PATTERN_AS_MODELLED: &str = "^[A-Z]{3}$";

/// The `@pattern` the model declares on `com.amazonaws.pricing#PriceListArn`, as written
/// (`smithy_query` query="shape" shape="PriceListArn").
const PRICE_LIST_ARN_PATTERN_AS_MODELLED: &str =
    "^arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/[A-Za-z0-9+_/.-]{1,1023}$";

/// The expression a refusal quotes for `CurrencyCode`: the modelled text without the
/// whole-value anchor pair.
const CURRENCY_CODE_PATTERN_PUBLISHED: &str = "[A-Z]{3}";

/// The expression a refusal quotes for `PriceListArn`.
const PRICE_LIST_ARN_PATTERN_PUBLISHED: &str =
    "arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/[A-Za-z0-9+_/.-]{1,1023}";

/// A `PriceListArn` the model's expression admits in full, for the rows that violate it by
/// carrying something around a legal ARN rather than by being malformed.
const WELL_FORMED_ARN: &str =
    "arn:aws:pricing:::price-list/aws/AmazonEC2/USD/20230101000000/us-east-1";

/// What a clause opens with before it names the requirement.
fn clause(value: &str, member: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{member}' failed to satisfy constraint: {requirement}")
}

/// The requirement clause for a `@pattern`, over the expression as a refusal quotes it.
fn pattern_requirement(published: &str) -> String {
    format!("Member must satisfy regular expression pattern: {published}")
}

/// A clause stating NO value: a list or a map has no scalar rendering to quote, so a
/// COLLECTION that broke its `@length` is stated without the value half.
fn valueless_clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// A `GetProducts` request carrying `count` well-formed filters, for the row that breaks
/// the `Filters` list's own `@length` (0..50) rather than any element's constraint.
fn get_products_with_filters(count: usize) -> Value {
    let filters: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "Type": "TERM_MATCH",
                "Field": "instanceType",
                "Value": format!("t3.filter{index}"),
            })
        })
        .collect();
    json!({ "ServiceCode": "AmazonEC2", "Filters": filters })
}

/// The envelope a refusal states its clauses in.
fn envelope(clauses: &[String]) -> String {
    if clauses.len() == 1 {
        format!("1 validation error detected: {}", clauses[0])
    } else {
        format!(
            "{} validation errors detected: {}",
            clauses.len(),
            clauses.join("; ")
        )
    }
}

/// A `ListPriceLists` request whose only questionable member is `CurrencyCode`.
fn list_price_lists(currency_code: &str) -> Value {
    json!({
        "ServiceCode": "AmazonEC2",
        "EffectiveDate": JAN_1_2024,
        "CurrencyCode": currency_code,
    })
}

/// A `GetPriceListFileUrl` request whose only questionable member is `PriceListArn`.
fn get_price_list_file_url(arn: &str) -> Value {
    json!({ "PriceListArn": arn, "FileFormat": "json" })
}

#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_the_whole_value_anchors() {
    let router = router();

    // Each row: the case, the operation, the request, and the clauses the refusal states
    // in order. Every row is a `400 InvalidParameterException` — the only code this model
    // declares for a bad input.
    let cases: Vec<(&str, &str, Value, Vec<String>)> = vec![
        (
            "currencyCode shorter than the expression admits",
            "ListPriceLists",
            list_price_lists("US"),
            vec![clause(
                "US",
                "currencyCode",
                &pattern_requirement(CURRENCY_CODE_PATTERN_PUBLISHED),
            )],
        ),
        (
            "currencyCode in the wrong case",
            "ListPriceLists",
            list_price_lists("usd"),
            vec![clause(
                "usd",
                "currencyCode",
                &pattern_requirement(CURRENCY_CODE_PATTERN_PUBLISHED),
            )],
        ),
        (
            "currencyCode longer than the expression admits",
            "ListPriceLists",
            list_price_lists("USDX"),
            vec![clause(
                "USDX",
                "currencyCode",
                &pattern_requirement(CURRENCY_CODE_PATTERN_PUBLISHED),
            )],
        ),
        (
            // The value carries the very characters the anchors are: the requirement is
            // quoted without the model's pair all the same, and the value is echoed whole.
            "currencyCode carrying the anchor characters itself",
            "ListPriceLists",
            list_price_lists("^A$"),
            vec![clause(
                "^A$",
                "currencyCode",
                &pattern_requirement(CURRENCY_CODE_PATTERN_PUBLISHED),
            )],
        ),
        (
            // The quoted expression matches INSIDE this value; the check anchors, so it
            // is refused, and the requirement it states is still the unanchored one.
            "currencyCode containing a match that is not the whole value",
            "ListPriceLists",
            list_price_lists("xUSDx"),
            vec![clause(
                "xUSDx",
                "currencyCode",
                &pattern_requirement(CURRENCY_CODE_PATTERN_PUBLISHED),
            )],
        ),
        (
            "priceListArn of legal length whose shape the expression does not admit",
            "GetPriceListFileUrl",
            get_price_list_file_url("arn:aws:pricing:::not-a-price-list/AmazonEC2/some-fake-id"),
            vec![clause(
                "arn:aws:pricing:::not-a-price-list/AmazonEC2/some-fake-id",
                "priceListArn",
                &pattern_requirement(PRICE_LIST_ARN_PATTERN_PUBLISHED),
            )],
        ),
        (
            // A legal ARN with text in front of it: the quoted expression matches inside
            // the value, the anchored check does not.
            "priceListArn containing a match that is not the whole value",
            "GetPriceListFileUrl",
            get_price_list_file_url(&format!("not-an-arn {WELL_FORMED_ARN}")),
            vec![clause(
                &format!("not-an-arn {WELL_FORMED_ARN}"),
                "priceListArn",
                &pattern_requirement(PRICE_LIST_ARN_PATTERN_PUBLISHED),
            )],
        ),
        (
            // Two clauses about one member: the pattern one is quoted the same way when it
            // is not the only thing the request broke.
            "priceListArn below its length and outside its expression",
            "GetPriceListFileUrl",
            get_price_list_file_url("arn:aws:pricing"),
            vec![
                clause(
                    "arn:aws:pricing",
                    "priceListArn",
                    "Member must have length greater than or equal to 18",
                ),
                clause(
                    "arn:aws:pricing",
                    "priceListArn",
                    &pattern_requirement(PRICE_LIST_ARN_PATTERN_PUBLISHED),
                ),
            ],
        ),
        (
            // The other constraint classes are untouched by this rule: a length, a range,
            // an enumerated value set and an absent required member keep their wording.
            "fileFormat below its length",
            "GetPriceListFileUrl",
            json!({ "PriceListArn": WELL_FORMED_ARN, "FileFormat": "" }),
            vec![clause(
                "",
                "fileFormat",
                "Member must have length greater than or equal to 1",
            )],
        ),
        (
            "maxResults below its range",
            "DescribeServices",
            json!({ "MaxResults": 0 }),
            vec![clause(
                "0",
                "maxResults",
                "Member must have value greater than or equal to 1",
            )],
        ),
        (
            "maxResults above its range",
            "DescribeServices",
            json!({ "MaxResults": 101 }),
            vec![clause(
                "101",
                "maxResults",
                "Member must have value less than or equal to 100",
            )],
        ),
        (
            // A COLLECTION over its own `@length`: the clause names the list member and
            // states no value, because a list has no scalar rendering to quote.
            "filters above the list's length",
            "GetProducts",
            get_products_with_filters(51),
            vec![valueless_clause(
                "filters",
                "Member must have length less than or equal to 50",
            )],
        ),
        (
            "a required member absent",
            "ListPriceLists",
            json!({ "ServiceCode": "AmazonEC2", "EffectiveDate": JAN_1_2024 }),
            vec![
                "Value null at 'currencyCode' failed to satisfy constraint: \
                 Member must not be null"
                    .to_string(),
            ],
        ),
    ];

    for (case, operation, body, clauses) in cases {
        let parsed = err(&router, operation, &body, 400, "InvalidParameterException").await;
        let actual = error_message(&parsed);
        let expected = envelope(&clauses);
        assert_eq!(
            actual, expected,
            "case {case}: expected message {expected:?}, got {actual:?}"
        );
        // The same invariant stated over the message itself rather than over the
        // expectation, so it holds for EVERY pattern clause a refusal carries, wherever
        // the clause sits: a clause ends at the separator, and what each one quotes after
        // the requirement opens carries neither anchor.
        for stated in actual.split("; ") {
            let Some((_, quoted)) = stated.split_once("regular expression pattern: ") else {
                continue;
            };
            assert!(
                !quoted.starts_with('^') && !quoted.ends_with('$'),
                "case {case}: the quoted expression must carry neither anchor, got \
                 {quoted:?}"
            );
        }
        assert!(
            !actual.contains(CURRENCY_CODE_PATTERN_AS_MODELLED)
                && !actual.contains(PRICE_LIST_ARN_PATTERN_AS_MODELLED),
            "case {case}: the modelled text must not be quoted as written, got {actual:?}"
        );
    }

    // The CHECKS themselves are unchanged, so the bounds the rows above crossed still
    // admit a value AT each published bound: the `Filters` list at its minimum (0 entries,
    // the member absent in effect) and at its maximum (50).
    for count in [0usize, 50] {
        let served = ok(&router, "GetProducts", &get_products_with_filters(count)).await;
        assert!(
            served.get("PriceList").is_some(),
            "case {count} filters: a filter list at a published bound must be served, got \
             {served}"
        );
    }
}

#[tokio::test]
async fn the_anchored_expression_is_still_what_a_value_is_checked_against() {
    let router = router();

    // The rendering changed, the CHECK did not: a value the anchored expression admits is
    // served, and one it does not is refused — including a value the UNANCHORED expression
    // would admit, which is what proves the check still anchors.
    let admitted = ok(
        &router,
        "ListPriceLists",
        &json!({
            "ServiceCode": "AmazonEC2",
            "EffectiveDate": JAN_1_2024,
            "CurrencyCode": "USD",
            "MaxResults": 1,
        }),
    )
    .await;
    let price_lists = admitted
        .get("PriceLists")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !price_lists.is_empty(),
        "case CurrencyCode=USD: a currency code the expression admits must be served, got \
         {admitted}"
    );

    // The ARN the listing published is admitted by the same expression a malformed one
    // breaks, and resolves to a file URL.
    let arn = price_lists
        .first()
        .and_then(|entry| entry.get("PriceListArn"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        arn.starts_with(ARN_PREFIX),
        "case published ARN: expected an ARN under {ARN_PREFIX}, got {arn:?}"
    );
    let resolved = ok(
        &router,
        "GetPriceListFileUrl",
        &json!({ "PriceListArn": arn, "FileFormat": "json" }),
    )
    .await;
    assert!(
        resolved.get("Url").and_then(Value::as_str).is_some(),
        "case published ARN: expected a file URL, got {resolved}"
    );

    // And every refused value is refused with the one code this model declares, whatever
    // form it misses the expression by.
    let refused: &[(&str, &str, Value)] = &[
        ("currencyCode too short", "ListPriceLists", list_price_lists("US")),
        ("currencyCode too long", "ListPriceLists", list_price_lists("USDX")),
        ("currencyCode wrong case", "ListPriceLists", list_price_lists("Usd")),
        ("currencyCode digits", "ListPriceLists", list_price_lists("123")),
        (
            "currencyCode a match inside a longer value",
            "ListPriceLists",
            list_price_lists("USDUSD"),
        ),
        (
            "priceListArn of another service",
            "GetPriceListFileUrl",
            get_price_list_file_url("arn:aws:s3:::price-list/aws/AmazonEC2"),
        ),
        (
            "priceListArn with a trailing match",
            "GetPriceListFileUrl",
            get_price_list_file_url(&format!("{WELL_FORMED_ARN} and more than the arn\n")),
        ),
    ];
    for (case, operation, body) in refused.iter().cloned() {
        let (status, parsed) = call(&router, operation, &body).await;
        assert_eq!(
            (status, parsed.get("__type").and_then(Value::as_str)),
            (400, Some("InvalidParameterException")),
            "case {case}: expected 400 InvalidParameterException, got {status} with {parsed}"
        );
    }
}
