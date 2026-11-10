// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the AWS Outposts order lifecycle, driven over the wire
//! through the same front door as [`crate::site_and_outpost_lifecycle`] (whose
//! harness this suite reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `CreateOrder` mints an `OrderId` of the form its model declares, persists the
//!    order the request describes against the Outpost it names — by bare id or by the
//!    full Outpost ARN, the two forms the modelled `OutpostIdentifier` pattern accepts
//!    — mints a `LineItemId` for every line item the request sent, and answers the
//!    order from what was persisted. `GetOrder` answers that same stored record, and
//!    reading it never changes what a later read sees.
//! 2. An order is submitted in the non-terminal `PREPARING` status, with every line
//!    item in the item-level status of the same name, and the clock moves the order to
//!    `IN_PROGRESS` and its items to `BUILDING` at `ORDER_PREPARATION`. The transition
//!    moves those statuses and nothing else.
//! 3. An order taken from a quote records that quote and moves it to
//!    `ORDER_SUBMITTED`, naming the order in the quote's `SubmittedOrderId`; a quote in
//!    any other modelled status cannot take an order (409 `ConflictException`), and a
//!    refused order persists nothing.
//! 4. An order outstanding on an Outpost is a resource that blocks that Outpost's
//!    decommission (`OUTPOST_ORDER_CANCELLABLE`), and an `OrderId` of that Outpost
//!    scopes `GetOutpostSupportedInstanceTypes` instead of being unknown to it.
//! 5. An identifier that names nothing is 404 `NotFoundException` with the identifier
//!    echo of the value AS SUPPLIED; a request that is not a valid order — no contents,
//!    a malformed identifier, a value outside a modelled constraint — is 400
//!    `ValidationException`.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    LINE_ITEM_STATUS_BUILDING, LINE_ITEM_STATUS_PREPARING, ORDER_PREPARATION,
    ORDER_STATUS_IN_PROGRESS, ORDER_STATUS_PREPARING, ORDER_TYPE_OUTPOST, QUOTE_STATUS_CREATED,
    QUOTE_STATUS_EXPIRED, QUOTE_STATUS_ORDER_SUBMITTED,
};

use crate::quote_lifecycle::{create_quote, request as quote_request};
use crate::quote_options::option_identifier;
use crate::site_and_outpost_lifecycle::{
    is_minted_id, refusal, segment, text, without, Outposts, Refusal, ABSENT_OUTPOST_ID,
    OUTPOST_ARN_PREFIX,
};

/// The ARN prefix of a quote of this emulator, from the modelled `QuoteIdentifier`
/// pattern's ARN branch plus the home region and the canonical emulator account id.
const QUOTE_ARN_PREFIX: &str = "arn:aws:outposts:us-east-1:000000000000:quote/";

/// A quote id this emulator never mints, used where a case needs an identifier that
/// is well formed (so the boundary accepts it) but names nothing.
const ABSENT_QUOTE_ID: &str = "oq-00000000000000000";

/// An order id this emulator never mints. See [`ABSENT_QUOTE_ID`].
const ABSENT_ORDER_ID: &str = "oo-00000000000000000";

/// Catalog item SKUs of the modelled `SkuCode` form (`^OR-[A-Z0-9]{7}$`), which is
/// the only thing this API says about a catalog item id a line item may name.
const SKUS: &[&str] = &["OR-1A2B3C4", "OR-ABCDEFG", "OR-0000001", "OR-ZZZZZZ9"];

/// The members an `Order` carries only once something has happened to it that this
/// slice of the API cannot make happen: an order is fulfilled by hardware being
/// installed on the site, which leaves a record this emulator does not have.
const UNREPORTED: &[&str] = &["OrderFulfilledDate"];

/// The members a `LineItem` carries only for hardware in transit, hardware installed,
/// or an order that replaces an earlier one — none of which a JUST-SUBMITTED order has:
/// `CreateOrder` is told nothing of a shipment or of a replaced order, and nothing is
/// installed by submitting an order, so the hardware a line item reports once it is
/// installed is reported then and not here
/// ([`crate::order_line_item_assets`]).
const UNREPORTED_ON_ITEM: &[&str] = &[
    "ShipmentInformation",
    "AssetInformationList",
    "PreviousLineItemId",
    "PreviousOrderId",
];

/// Wait until an order's clock-owed `PREPARING` -> `IN_PROGRESS` transition has
/// fallen due.
///
/// The service materialises the transition at the instant it stored, so a case that
/// wants settled orders waits past that span (plus a margin, so the case is not
/// racing it). Blocking with `std::thread::sleep` for the reason
/// [`crate::site_and_outpost_lifecycle::settle`] does: the workspace's `tokio`
/// carries no timer driver.
fn settle_orders() {
    std::thread::sleep(ORDER_PREPARATION + std::time::Duration::from_millis(400));
}

/// `CreateOrder`, returning the `Order` it answered with.
async fn create_order(outposts: &Outposts, body: &Value) -> Value {
    outposts.ok("POST", "/orders", Some(body)).await["Order"].clone()
}

/// `GetOrder`, returning the `Order` it answered with.
async fn get_order(outposts: &Outposts, order_id: &str) -> Value {
    outposts
        .ok("GET", &format!("/orders/{}", segment(order_id)), None)
        .await["Order"]
        .clone()
}

/// `GetQuote`, returning the `Quote` it answered with.
async fn get_quote(outposts: &Outposts, identifier: &str) -> Value {
    outposts
        .ok("GET", &format!("/quotes/{}", segment(identifier)), None)
        .await["Quote"]
        .clone()
}

/// One line item request: a catalog item SKU and a quantity.
fn line_item(sku: &str, quantity: i64) -> Value {
    json!({ "CatalogItemId": sku, "Quantity": quantity })
}

/// A `CreateOrder` body: the two members the model marks `@required`, with
/// `overrides` merged in.
fn order_request(outpost_identifier: &str, payment_option: &str, overrides: Value) -> Value {
    let mut body = json!({
        "OutpostIdentifier": outpost_identifier,
        "PaymentOption": payment_option,
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), overrides.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    body
}

/// One quote of `outpost_id` that an order can be submitted from, as its `QuoteId`
/// and the `QuoteOptionIdentifier` of the option it offers — the only option an order
/// can be submitted against, since a quote option is minted with the quote it belongs
/// to.
///
/// The country the quote is for is the country of the site the Outpost stands on
/// ([`Outposts::site_and_active_outpost`]), one of the checks a quote has to meet
/// before an order can be submitted from it.
async fn a_quote(outposts: &Outposts, outpost_id: &str) -> (String, String) {
    let quote = create_quote(
        outposts,
        &quote_request(
            "US",
            json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 4.0 }]),
            json!({ "OutpostIdentifier": outpost_id }),
        ),
    )
    .await;
    let option = option_identifier(&quote, "a quote of the Outpost");
    (text(&quote, "QuoteId"), option)
}

/// Put a stored quote in `status`, through the store seam
/// [`crate::quote_lifecycle`] ages a quote through: the two statuses other than
/// `CREATED` are reached by time passing or by an order already having been
/// submitted, and a case about what a status REFUSES must not depend on which.
fn set_quote_status(outposts: &Outposts, quote_id: &str, status: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let record = store
        .quotes
        .get_mut(quote_id)
        .unwrap_or_else(|| panic!("the create persisted quote {quote_id}"));
    record.quote.quote_status = Some(status.to_string());
}

/// The same order, as it reads once the transition the clock owed it has been
/// materialised: the order `IN_PROGRESS` and every line item `BUILDING`.
fn settled(order: &Value) -> Value {
    let mut settled = order.clone();
    settled["Status"] = json!(ORDER_STATUS_IN_PROGRESS);
    if let Some(items) = settled.get_mut("LineItems").and_then(Value::as_array_mut) {
        for item in items {
            item["Status"] = json!(LINE_ITEM_STATUS_BUILDING);
        }
    }
    settled
}

/// Every `LineItemId` an order reports, in the order the order reports them.
fn line_item_ids(order: &Value) -> Vec<String> {
    order
        .get("LineItems")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| text(item, "LineItemId")).collect())
        .unwrap_or_default()
}

/// Rule 1: a `CreateOrder` mints an `OrderId` of the modelled form, stores the order
/// the caller described, and answers it from what was stored.
///
/// The rows differ along everything `CreateOrder` generalises over: how the Outpost is
/// named (bare id, ARN), how many line items (one, three, the modelled maximum of
/// twenty), the quantities on them (the minimum the model allows and a large one),
/// which catalog item SKUs, every modelled payment option, every modelled payment
/// term and none, whether a quote and a quote option are named, and an order whose
/// contents come from the quote alone.
#[tokio::test]
async fn create_order_persists_and_get_order_reads_it_back() {
    let outposts = Outposts::new();
    let (outpost_id, outpost_arn) = outposts.site_and_active_outpost("order-outpost", "US").await;
    // One quote per row that names one: a quote takes exactly one order (rule 3).
    let (quote_by_id, option_of_quote_by_id) = a_quote(&outposts, &outpost_id).await;
    let (quote_by_arn, _) = a_quote(&outposts, &outpost_id).await;
    let twenty: Vec<Value> = (0..20)
        .map(|index| line_item(SKUS[index % SKUS.len()], (index as i64) + 1))
        .collect();

    let cases: Vec<(&str, Value)> = vec![
        (
            "the required members and one line item",
            order_request(
                &outpost_id,
                "ALL_UPFRONT",
                json!({ "LineItems": [line_item(SKUS[0], 1)] }),
            ),
        ),
        (
            "the Outpost named by ARN, three line items, a payment term",
            order_request(
                &outpost_arn,
                "NO_UPFRONT",
                json!({
                    "LineItems": [
                        line_item(SKUS[1], 1),
                        line_item(SKUS[2], 2),
                        line_item(SKUS[3], 500),
                    ],
                    "PaymentTerm": "ONE_YEAR",
                }),
            ),
        ),
        (
            "the modelled maximum of twenty line items",
            order_request(
                &outpost_id,
                "PARTIAL_UPFRONT",
                json!({ "LineItems": twenty, "PaymentTerm": "THREE_YEARS" }),
            ),
        ),
        (
            "the longest payment term",
            order_request(
                &outpost_id,
                "ALL_UPFRONT",
                json!({
                    "LineItems": [line_item(SKUS[2], 1)],
                    "PaymentTerm": "FIVE_YEARS",
                }),
            ),
        ),
        (
            "taken from a quote named by bare id, with no line items of its own",
            order_request(
                &outpost_id,
                "NO_UPFRONT",
                json!({
                    "QuoteIdentifier": quote_by_id.clone(),
                    "QuoteOptionIdentifier": option_of_quote_by_id.clone(),
                }),
            ),
        ),
        (
            "taken from a quote named by ARN, with line items of its own",
            order_request(
                &outpost_id,
                "PARTIAL_UPFRONT",
                json!({
                    "QuoteIdentifier": format!("{QUOTE_ARN_PREFIX}{quote_by_arn}"),
                    "LineItems": [line_item(SKUS[0], 7), line_item(SKUS[1], 7)],
                }),
            ),
        ),
    ];

    let mut created: Vec<(&str, Value, Value)> = Vec::new();
    let mut every_line_item_id: Vec<String> = Vec::new();
    for (case, body) in &cases {
        let order = create_order(&outposts, body).await;
        let order_id = text(&order, "OrderId");
        assert!(
            is_minted_id(&order_id, "oo"),
            "case {case:?}: OrderId {order_id:?} is not the modelled ^oo-[a-f0-9]{{17}}$ form"
        );
        // The Outpost is reported as the bare id the modelled `OutpostIdOnly` member
        // carries, whichever of the two identifier forms the request named it by.
        assert_eq!(
            text(&order, "OutpostId"),
            outpost_id,
            "case {case:?}: expected OutpostId {outpost_id:?}, got {order}"
        );
        assert_eq!(
            text(&order, "Status"),
            ORDER_STATUS_PREPARING,
            "case {case:?}: a just-submitted order is being prepared, got {order}"
        );
        assert_eq!(
            text(&order, "OrderType"),
            ORDER_TYPE_OUTPOST,
            "case {case:?}: an order placed for an Outpost is of that type, got {order}"
        );
        assert_eq!(
            order.get("PaymentOption"),
            body.get("PaymentOption"),
            "case {case:?}: expected the payment option the request sent, got {order}"
        );
        assert_eq!(
            order.get("PaymentTerm"),
            body.get("PaymentTerm"),
            "case {case:?}: expected the payment term the request sent, got {order}"
        );
        assert_eq!(
            order.get("QuoteOptionIdentifier"),
            body.get("QuoteOptionIdentifier"),
            "case {case:?}: expected the quote option the request sent, got {order}"
        );
        // A quote is reported as the quote's OWN id, whichever identifier form named
        // it; an order taken from no quote reports none.
        let expected_quote = body.get("QuoteIdentifier").map(|_| match case.contains("ARN") {
            true => quote_by_arn.clone(),
            false => quote_by_id.clone(),
        });
        assert_eq!(
            order.get("QuoteIdentifier").and_then(Value::as_str),
            expected_quote.as_deref(),
            "case {case:?}: expected QuoteIdentifier {expected_quote:?}, got {order}"
        );
        assert!(
            order.get("OrderSubmissionDate").is_some(),
            "case {case:?}: a submitted order carries the instant it was submitted, got {order}"
        );
        for member in UNREPORTED {
            assert!(
                order.get(member).is_none(),
                "case {case:?}: {member} is not something this emulator can compute, so it \
                 must be absent, got {order}"
            );
        }

        // Every line item the request sent is reported, in the order it was sent,
        // with its own minted `LineItemId` and the item-level status of a
        // just-submitted item; an order that sent none reports none.
        let requested = body.get("LineItems").and_then(Value::as_array);
        let reported = order.get("LineItems").and_then(Value::as_array);
        match (requested, reported) {
            (None, reported) => assert!(
                reported.is_none(),
                "case {case:?}: an order with no line items of its own must report none, \
                 got {order}"
            ),
            (Some(requested), Some(reported)) => {
                assert_eq!(
                    reported.len(),
                    requested.len(),
                    "case {case:?}: expected {} line items, got {order}",
                    requested.len()
                );
                for (index, (sent, item)) in requested.iter().zip(reported).enumerate() {
                    let line_item_id = text(item, "LineItemId");
                    assert!(
                        is_minted_id(&line_item_id, "ooi"),
                        "case {case:?}: line item {index} id {line_item_id:?} is not the \
                         modelled ^ooi-[a-f0-9]{{17}}$ form"
                    );
                    every_line_item_id.push(line_item_id);
                    for member in ["CatalogItemId", "Quantity"] {
                        assert_eq!(
                            item.get(member),
                            sent.get(member),
                            "case {case:?}: line item {index} must report the {member} the \
                             request sent ({:?}), got {item}",
                            sent.get(member)
                        );
                    }
                    assert_eq!(
                        text(item, "Status"),
                        LINE_ITEM_STATUS_PREPARING,
                        "case {case:?}: line item {index} of a just-submitted order is being \
                         prepared, got {item}"
                    );
                    for member in UNREPORTED_ON_ITEM {
                        assert!(
                            item.get(member).is_none(),
                            "case {case:?}: line item {index} must not report {member}, \
                             got {item}"
                        );
                    }
                }
            }
            (Some(_), None) => panic!(
                "case {case:?}: the line items the request sent must be reported, got {order}"
            ),
        }
        created.push((case, body.clone(), order));
    }

    // Every create minted its own identifiers: no two orders, and no two line items
    // of any order, were handed the same id.
    let order_ids: std::collections::BTreeSet<String> = created
        .iter()
        .map(|(_, _, order)| text(order, "OrderId"))
        .collect();
    assert_eq!(
        order_ids.len(),
        created.len(),
        "every create mints its own OrderId, got {order_ids:?}"
    );
    let distinct_items: std::collections::BTreeSet<&String> = every_line_item_id.iter().collect();
    assert_eq!(
        distinct_items.len(),
        every_line_item_id.len(),
        "every line item is minted its own LineItemId, got {every_line_item_id:?}"
    );

    // Rule 1's read half, stated past the clock-owed transition so what the read
    // answers is the stored record and not a race with it (rule 2 is what tests the
    // transition itself).
    settle_orders();
    for (case, _, order) in &created {
        let order_id = text(order, "OrderId");
        let expected = settled(order);
        for pass in ["first read", "second read"] {
            let read = get_order(&outposts, &order_id).await;
            assert_eq!(
                read, expected,
                "case {case:?} ({pass}): GetOrder must answer the stored order"
            );
        }
    }
}

/// Rule 2: an order is submitted in the non-terminal `PREPARING` status and the clock
/// moves it — and its line items — on at `ORDER_PREPARATION`, changing those statuses
/// and nothing else.
///
/// The rows differ along the shape of the order the transition applies to: one line
/// item, several, and an order with no line items of its own, so the item-level half
/// of the rule is exercised where there are items to move and where there are none.
#[tokio::test]
async fn an_order_is_prepared_and_then_moves_to_in_progress_on_the_clock() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts
        .site_and_active_outpost("order-progress", "US")
        .await;
    let (quote_id, _) = a_quote(&outposts, &outpost_id).await;
    let cases: Vec<(&str, Value)> = vec![
        (
            "one line item",
            order_request(
                &outpost_id,
                "ALL_UPFRONT",
                json!({ "LineItems": [line_item(SKUS[0], 1)] }),
            ),
        ),
        (
            "four line items",
            order_request(
                &outpost_id,
                "NO_UPFRONT",
                json!({
                    "LineItems": [
                        line_item(SKUS[0], 1),
                        line_item(SKUS[1], 2),
                        line_item(SKUS[2], 3),
                        line_item(SKUS[3], 4),
                    ],
                    "PaymentTerm": "THREE_YEARS",
                }),
            ),
        ),
        (
            "no line items of its own",
            order_request(
                &outpost_id,
                "PARTIAL_UPFRONT",
                json!({ "QuoteIdentifier": quote_id.clone() }),
            ),
        ),
    ];

    for (case, body) in &cases {
        // The span is never zero, so a caller acting straight after the submit still
        // sees the transient status. The read is only held to that where it provably
        // happened inside the span — the clock is the service's, not the test's.
        let submitted_at = std::time::Instant::now();
        let order = create_order(&outposts, body).await;
        let order_id = text(&order, "OrderId");
        let read = get_order(&outposts, &order_id).await;
        if submitted_at.elapsed() < ORDER_PREPARATION {
            assert_eq!(
                read, order,
                "case {case:?}: a read inside the preparation span answers the order the \
                 submit reported"
            );
        }

        settle_orders();
        let moved = get_order(&outposts, &order_id).await;
        assert_eq!(
            text(&moved, "Status"),
            ORDER_STATUS_IN_PROGRESS,
            "case {case:?}: past the preparation span the order has moved on, got {moved}"
        );
        for (index, item) in moved
            .get("LineItems")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            assert_eq!(
                text(item, "Status"),
                LINE_ITEM_STATUS_BUILDING,
                "case {case:?}: line item {index} moves with its order, got {item}"
            );
        }
        // The transition moved the statuses and nothing else.
        assert_eq!(
            without(&moved, "LineItems"),
            without(&settled(&order), "LineItems"),
            "case {case:?}: settling must change only the statuses the clock owed"
        );
        assert_eq!(
            line_item_ids(&moved),
            line_item_ids(&order),
            "case {case:?}: settling must not touch the line items themselves"
        );
        assert_eq!(
            moved,
            settled(&order),
            "case {case:?}: expected the submitted order with its statuses moved on"
        );
        // And it is applied once: a later read answers the same thing.
        assert_eq!(
            get_order(&outposts, &order_id).await,
            moved,
            "case {case:?}: a settled order reads the same every time"
        );
    }
}

/// Rule 3: a quote takes an order exactly when it is still `CREATED`, and taking one
/// is what makes its status `ORDER_SUBMITTED` and its `SubmittedOrderId` present.
///
/// The rows span every modelled `QuoteStatus`: the one an order can be submitted from,
/// the expiry, and a quote an order was already submitted from.
#[tokio::test]
async fn a_quote_takes_an_order_only_while_it_is_still_created() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts
        .site_and_active_outpost("order-from-quote", "US")
        .await;
    let cases: Vec<(&str, &str, Option<u16>)> = vec![
        ("a quote an order can be submitted from", QUOTE_STATUS_CREATED, None),
        ("a quote past its own expiry", QUOTE_STATUS_EXPIRED, Some(409)),
        (
            "a quote an order was already submitted from",
            QUOTE_STATUS_ORDER_SUBMITTED,
            Some(409),
        ),
    ];

    for (case, status, refused) in &cases {
        let (quote_id, _) = a_quote(&outposts, &outpost_id).await;
        set_quote_status(&outposts, &quote_id, status);
        let body = order_request(
            &outpost_id,
            "ALL_UPFRONT",
            json!({ "QuoteIdentifier": quote_id.clone() }),
        );
        let reply = outposts.call("POST", "/orders", Some(&body)).await;
        match refused {
            Some(expected) => {
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (*expected, "ConflictException"),
                    "case {case:?}: expected {expected} ConflictException, got {} {} {}",
                    reply.status,
                    reply.error_code,
                    reply.body
                );
                // A refused order persists nothing, and leaves the quote as it was.
                let quote = get_quote(&outposts, &quote_id).await;
                assert_eq!(
                    text(&quote, "QuoteStatus"),
                    *status,
                    "case {case:?}: a refused order must not move the quote, got {quote}"
                );
                assert!(
                    quote.get("SubmittedOrderId").is_none(),
                    "case {case:?}: a refused order must not be named on the quote, got {quote}"
                );
            }
            None => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected HTTP 200, got {} {}",
                    reply.status, reply.body
                );
                let order_id = text(&reply.body["Order"], "OrderId");
                let quote = get_quote(&outposts, &quote_id).await;
                assert_eq!(
                    text(&quote, "QuoteStatus"),
                    QUOTE_STATUS_ORDER_SUBMITTED,
                    "case {case:?}: submitting an order moves the quote, got {quote}"
                );
                assert_eq!(
                    text(&quote, "SubmittedOrderId"),
                    order_id,
                    "case {case:?}: the quote must name the order submitted from it, got {quote}"
                );
                // And the same quote cannot take a second order.
                let again = outposts.call("POST", "/orders", Some(&body)).await;
                assert_eq!(
                    (again.status, again.error_code.as_str()),
                    (409, "ConflictException"),
                    "case {case:?}: a second order from one quote is refused, got {} {} {}",
                    again.status,
                    again.error_code,
                    again.body
                );
                assert_eq!(
                    text(&get_quote(&outposts, &quote_id).await, "SubmittedOrderId"),
                    order_id,
                    "case {case:?}: the refused second order must not replace the first"
                );
            }
        }
    }
}

/// Rule 4: an order outstanding on an Outpost is a resource of this store that other
/// operations read — it blocks that Outpost's decommission, and it is an order
/// `GetOutpostSupportedInstanceTypes` can be scoped to.
///
/// The rows differ along which Outpost the order was placed against, which is the
/// whole of what makes an order relevant to another Outpost's call: its own, another
/// one, and none at all.
#[tokio::test]
async fn an_outstanding_order_is_read_by_the_operations_that_name_one() {
    let outposts = Outposts::new();
    // A decommission is refused on an Outpost whose hardware is not in service
    // ([`crate::precondition_gates`]), and the rows below decommission both, so both
    // are past that gate.
    let (ordered, _) = outposts
        .site_and_outpost_in_service("ordered-outpost")
        .await;
    let (bare, _) = outposts
        .site_and_outpost_in_service("unordered-outpost")
        .await;
    let order_id = text(
        &create_order(
            &outposts,
            &order_request(
                &ordered,
                "ALL_UPFRONT",
                json!({ "LineItems": [line_item(SKUS[0], 2)] }),
            ),
        )
        .await,
        "OrderId",
    );

    // The decommission half: the Outpost the order was placed against is blocked by
    // it, and the one nothing was ordered for is not.
    let cases: Vec<(&str, &str, &str, Option<Value>)> = vec![
        (
            "the Outpost the order was placed against",
            &ordered,
            "BLOCKED",
            Some(json!(["OUTPOST_ORDER_CANCELLABLE"])),
        ),
        (
            "an Outpost with no order against it",
            &bare,
            "REQUESTED",
            None,
        ),
    ];
    for (case, outpost_id, expected, blocking) in &cases {
        let answered = outposts
            .ok(
                "POST",
                &format!("/outposts/{}/decommission", segment(outpost_id)),
                Some(&json!({})),
            )
            .await;
        assert_eq!(
            text(&answered, "Status"),
            *expected,
            "case {case:?}: expected Status {expected}, got {answered}"
        );
        assert_eq!(
            answered.get("BlockingResourceTypes"),
            blocking.as_ref(),
            "case {case:?}: expected BlockingResourceTypes {blocking:?}, got {answered}"
        );
    }

    // The scoping half: an `OrderId` of the Outpost is an order the report knows and
    // is scoped by — these line items name SKUs this service publishes no
    // configuration under, so they record no hardware fact and narrow nothing, which
    // on an Outpost that recorded no hardware type is the whole published catalog
    // (what an order of published configurations scopes the report to is
    // [`crate::supported_instance_types_order_scope`]); one of another Outpost, or one
    // nothing holds, names no hardware here.
    //
    // The Outpost's OWN report is about the hardware INSTALLED on it, and nothing is
    // installed on this one, so it supports nothing — the rule
    // [`crate::supported_instance_types`] states.
    let installed = outposts
        .ok(
            "GET",
            &format!("/outposts/{}/supportedInstanceTypes", segment(&ordered)),
            None,
        )
        .await;
    assert_eq!(
        installed["InstanceTypes"],
        json!([]),
        "an Outpost holding no compute hardware supports no instance type, got {installed}"
    );
    let scoped = outposts
        .ok(
            "GET",
            &format!(
                "/outposts/{}/supportedInstanceTypes?OrderId={order_id}&MaxResults=1000",
                segment(&ordered)
            ),
            None,
        )
        .await;
    let catalog: Vec<Value> = crate::instance_type_catalogs::specifications()
        .iter()
        .map(|entry| {
            json!({
                "InstanceType": entry.get("InstanceType"),
                "VCPUs": entry.get("VCPUs"),
            })
        })
        .collect();
    assert_eq!(
        scoped["InstanceTypes"],
        json!(catalog),
        "an order whose line items name no published configuration records no hardware \
         fact, so it narrows the report by nothing"
    );
    for (case, outpost_id, order) in [
        ("an order of another Outpost", &bare, order_id.as_str()),
        ("an order nothing holds", &ordered, ABSENT_ORDER_ID),
    ] {
        let reply = outposts
            .call(
                "GET",
                &format!(
                    "/outposts/{}/supportedInstanceTypes?OrderId={order}",
                    segment(outpost_id)
                ),
                None,
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            text(&reply.body, "Message"),
            format!("No such order for order id {order}"),
            "case {case:?}: expected the order not-found wording, got {}",
            reply.body
        );
    }
}

/// Rule 5: what the order routes refuse, and with which status and wire error code.
///
/// The rows span an identifier that names nothing (the Outpost by both forms, the
/// quote by both forms, the order), an order with no contents at all, and the ways a
/// modelled constraint can be broken (the order id's pattern, the line item list's
/// length at both bounds, the quantity's range, the SKU's pattern, the payment
/// option's and term's value sets, and a missing required member).
#[tokio::test]
async fn absent_identifiers_are_not_found_and_bad_orders_are_refused() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("order-refusals").await;
    let one_item = json!({ "LineItems": [line_item(SKUS[0], 1)] });
    let twenty_one: Vec<Value> = (0..21)
        .map(|index| line_item(SKUS[index % SKUS.len()], 1))
        .collect();

    let cases: Vec<Refusal> = vec![
        refusal(
            "CreateOrder naming an Outpost that does not exist",
            "POST",
            "/orders".to_string(),
            404,
            "NotFoundException",
        )
        .with_body(order_request(
            ABSENT_OUTPOST_ID,
            "ALL_UPFRONT",
            one_item.clone(),
        )),
        refusal(
            "CreateOrder naming the ARN of an Outpost that does not exist",
            "POST",
            "/orders".to_string(),
            404,
            "NotFoundException",
        )
        .with_body(order_request(
            &format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}"),
            "ALL_UPFRONT",
            one_item.clone(),
        )),
        refusal(
            "CreateOrder naming a quote that does not exist",
            "POST",
            "/orders".to_string(),
            404,
            "NotFoundException",
        )
        .with_body(order_request(
            &outpost_id,
            "ALL_UPFRONT",
            json!({ "QuoteIdentifier": ABSENT_QUOTE_ID }),
        )),
        refusal(
            "CreateOrder naming the ARN of a quote that does not exist",
            "POST",
            "/orders".to_string(),
            404,
            "NotFoundException",
        )
        .with_body(order_request(
            &outpost_id,
            "ALL_UPFRONT",
            json!({ "QuoteIdentifier": format!("{QUOTE_ARN_PREFIX}{ABSENT_QUOTE_ID}") }),
        )),
        refusal(
            "CreateOrder naming neither line items nor a quote",
            "POST",
            "/orders".to_string(),
            400,
            "ValidationException",
        )
        .with_body(order_request(&outpost_id, "ALL_UPFRONT", json!({}))),
        refusal(
            "CreateOrder with an empty line item list",
            "POST",
            "/orders".to_string(),
            400,
            "ValidationException",
        )
        .with_body(order_request(
            &outpost_id,
            "ALL_UPFRONT",
            json!({ "LineItems": [] }),
        )),
        refusal(
            "CreateOrder one line item past the modelled maximum",
            "POST",
            "/orders".to_string(),
            400,
            "ValidationException",
        )
        .with_body(order_request(
            &outpost_id,
            "ALL_UPFRONT",
            json!({ "LineItems": twenty_one }),
        )),
        refusal(
            "CreateOrder with a quantity below the modelled minimum",
            "POST",
            "/orders".to_string(),
            400,
            "ValidationException",
        )
        .with_body(order_request(
            &outpost_id,
            "ALL_UPFRONT",
            json!({ "LineItems": [line_item(SKUS[0], 0)] }),
        )),
        refusal(
            "CreateOrder with a catalog item id the modelled pattern refuses",
            "POST",
            "/orders".to_string(),
            400,
            "ValidationException",
        )
        .with_body(order_request(
            &outpost_id,
            "ALL_UPFRONT",
            json!({ "LineItems": [line_item("not-a-sku", 1)] }),
        )),
        refusal(
            "CreateOrder with a payment option outside the modelled enum",
            "POST",
            "/orders".to_string(),
            400,
            "ValidationException",
        )
        .with_body(order_request(&outpost_id, "MONTHLY", one_item.clone())),
        refusal(
            "CreateOrder with a payment term outside the modelled enum",
            "POST",
            "/orders".to_string(),
            400,
            "ValidationException",
        )
        .with_body(order_request(
            &outpost_id,
            "ALL_UPFRONT",
            json!({ "LineItems": [line_item(SKUS[0], 1)], "PaymentTerm": "TWO_YEARS" }),
        )),
        refusal(
            "CreateOrder with no payment option at all",
            "POST",
            "/orders".to_string(),
            400,
            "ValidationException",
        )
        .with_body(json!({
            "OutpostIdentifier": outpost_id,
            "LineItems": [line_item(SKUS[0], 1)],
        })),
        refusal(
            "GetOrder on a well-formed order id that names nothing",
            "GET",
            format!("/orders/{ABSENT_ORDER_ID}"),
            404,
            "NotFoundException",
        ),
        refusal(
            "GetOrder on a label that is no identifier at all",
            "GET",
            "/orders/not-an-order-id".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "GetOrder on a quote id",
            "GET",
            format!("/orders/{ABSENT_QUOTE_ID}"),
            400,
            "ValidationException",
        ),
        refusal(
            "GetOrder on an order id whose suffix is not hex",
            "GET",
            "/orders/oo-zzzzzzzzzzzzzzzzz".to_string(),
            400,
            "ValidationException",
        ),
        refusal(
            "GetOrder on an order id one hex character short",
            "GET",
            "/orders/oo-0000000000000000".to_string(),
            400,
            "ValidationException",
        ),
    ];

    outposts.expect_refusals(&cases).await;
}

/// Rule 5's message half: a not-found refusal carries the wording the kind of
/// resource it is about is refused by — the order sentence for an order, the quote
/// sentence for a quote (naming the identifier as supplied, ARN form included), and
/// the resolved Outpost ARN for the Outpost an order would be placed on.
#[tokio::test]
async fn a_not_found_order_states_the_wording_of_the_resource_it_is_about() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("order-echoes").await;
    let absent_outpost_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");
    let absent_quote_arn = format!("{QUOTE_ARN_PREFIX}{ABSENT_QUOTE_ID}");
    let one_item = json!({ "LineItems": [line_item(SKUS[0], 1)] });
    let cases: Vec<(&str, &str, String, Option<Value>, String)> = vec![
        (
            "GetOrder by order id",
            "GET",
            format!("/orders/{ABSENT_ORDER_ID}"),
            None,
            format!("No such order for order id {ABSENT_ORDER_ID}"),
        ),
        (
            "CreateOrder naming an absent Outpost by bare id",
            "POST",
            "/orders".to_string(),
            Some(order_request(
                ABSENT_OUTPOST_ID,
                "ALL_UPFRONT",
                one_item.clone(),
            )),
            format!("outpostId={absent_outpost_arn}"),
        ),
        (
            "CreateOrder naming an absent Outpost by ARN",
            "POST",
            "/orders".to_string(),
            Some(order_request(
                &absent_outpost_arn,
                "ALL_UPFRONT",
                one_item.clone(),
            )),
            format!("outpostId={absent_outpost_arn}"),
        ),
        (
            "CreateOrder naming an absent quote by bare id",
            "POST",
            "/orders".to_string(),
            Some(order_request(
                &outpost_id,
                "ALL_UPFRONT",
                json!({ "QuoteIdentifier": ABSENT_QUOTE_ID }),
            )),
            format!("Could not find Quote with identifier {ABSENT_QUOTE_ID}"),
        ),
        (
            "CreateOrder naming an absent quote by ARN",
            "POST",
            "/orders".to_string(),
            Some(order_request(
                &outpost_id,
                "ALL_UPFRONT",
                json!({ "QuoteIdentifier": absent_quote_arn.clone() }),
            )),
            format!("Could not find Quote with identifier {absent_quote_arn}"),
        ),
    ];

    for (case, method, path, body, expected) in &cases {
        let reply = outposts.call(method, path, body.as_ref()).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let actual = reply
            .body
            .get("Message")
            .or_else(|| reply.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected $.Message {expected:?}, got {actual:?}"
        );
    }
}
