// SPDX-License-Identifier: CC0-1.0

//! The PERIOD a `ListInvoiceSummaries` request may ask about, driven end to end over this
//! service's wire protocol.
//!
//! The model hangs three temporal rules off `InvoiceSummariesFilter$ReceiverRole`
//! (`smithy_query(query="doc", shape="InvoiceSummariesFilter", member="ReceiverRole")`):
//! "Data is available starting 2025-06-01. Queries for periods before 2025-06-01 return a
//! validation error. … `TimeInterval` supports a time interval of up to 5 years. Without
//! `ReceiverRole`, `TimeInterval` is limited to one month." So a request has to state a
//! period this operation answers about: one that begins before the data a `ReceiverRole`
//! query is answered out of, or an interval longer than the bound the filter it states
//! carries, is the modelled `ValidationException` rather than a page of results.
//!
//! The tables below span that class rather than one request: each period member a request
//! states its period in, each side of the one date the rule names, both interval bounds at
//! the bound itself and one second past it, the calendar months of a whole year and a leap
//! day — and the same intervals under the other filter, where the other bound applies.
//! A refusal is asserted to PRECEDE the registry read as well, so a request outside the
//! rules is answered the same way whatever the registry holds.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::{
    Entity, InvoiceSummary, RECEIVER_ROLE_VALUES,
};

/// The account every row asks about.
const ACCOUNT: &str = "111111111111";

/// The one invoice the registry is seeded with, issued inside the window every admitted
/// row below covers.
const SEEDED_INVOICE_ID: &str = "INV-001";
const SEEDED_ISSUED_AT: &str = "2025-06-15T00:00:00Z";
/// The role that invoice was issued under, one of the model's own `ReceiverRole` values
/// (`smithy_query(query="shape", shape="ReceiverRole")`), pinned against that catalog by
/// [`the_seeded_role_is_a_modelled_value`].
const SEEDED_ROLE: &str = "BUYER";

/// The date the model documents the data a `ReceiverRole` query is answered out of as
/// beginning on, as the text that documentation states it in.
const DATA_START: &str = "2025-06-01";

/// The epoch second an ISO-8601 instant names, so every date below reads as a date rather
/// than as an epoch count.
fn secs(instant: &str) -> i64 {
    ::chrono::DateTime::parse_from_rfc3339(instant)
        .unwrap_or_else(|error| panic!("case {instant:?}: a valid instant, got {error}"))
        .timestamp()
}

/// The instant `months` calendar months after the ISO-8601 instant `from`, as the epoch
/// second it names: the calendar the model's bounds are stated in.
fn months_after(from: &str, months: u32) -> i64 {
    ::chrono::DateTime::parse_from_rfc3339(from)
        .unwrap_or_else(|error| panic!("case {from:?}: a valid instant, got {error}"))
        .checked_add_months(::chrono::Months::new(months))
        .unwrap_or_else(|| panic!("case {from:?}: {months} months after it is a calendar instant"))
        .timestamp()
}

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operation unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Put the one seeded invoice in the service's registry. An invoice is issued by AWS out
/// of a billing period's activity and no operation of this API creates one, so the
/// registry is seeded the way the other suites of this crate seed it.
fn seed(engine: &engine::InMemoryEngine) {
    let state = service_com_amazonaws_invoicing_invoicing::state::service_state(engine)
        .expect("the shared engine holds this service's state");
    let mut store = state.lock();
    store.invoices.insert(
        SEEDED_INVOICE_ID.to_string(),
        service_com_amazonaws_invoicing_invoicing::state::StoredInvoice::new(InvoiceSummary {
            account_id: Some(ACCOUNT.to_string()),
            invoice_id: Some(SEEDED_INVOICE_ID.to_string()),
            issued_date: Some(::aws_smithy_types::DateTime::from_secs(secs(
                SEEDED_ISSUED_AT,
            ))),
            receiver_role: Some(SEEDED_ROLE.to_string()),
            entity: Some(Entity {
                invoicing_entity: Some("Amazon Web Services, Inc.".to_string()),
                billing_entity: Some("AWS".to_string()),
            }),
            billing_period: Some(
                service_com_amazonaws_invoicing_invoicing::ops::types::BillingPeriod {
                    month: 6,
                    year: 2025,
                },
            ),
            ..Default::default()
        }),
    );
}

/// How many invoices the registry holds, so a refused request can be shown not to touch
/// it.
fn invoice_count(engine: &engine::InMemoryEngine) -> usize {
    let state = service_com_amazonaws_invoicing_invoicing::state::service_state(engine)
        .expect("the shared engine holds this service's state");
    let store = state.lock();
    store.invoices.len()
}

/// One awsJson request, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", "Invoicing.ListInvoiceSummaries")
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), without the
/// namespace a body may qualify it with.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A request for `ACCOUNT`, carrying `filter`.
fn request(filter: Value) -> Value {
    json!({
        "Selector": {"ResourceType": "ACCOUNT_ID", "Value": ACCOUNT},
        "Filter": filter,
    })
}

/// A `TimeInterval` over two ISO-8601 instants, as the request spells it.
fn interval(start: &str, end: &str) -> Value {
    json!({"StartDate": secs(start), "EndDate": secs(end)})
}

/// Assert `body` is answered with a page of results.
async fn admitted(router: &MultiServiceRouter, case: &str, body: &Value) {
    let (status, response) = call(router, body).await;
    assert_eq!(
        status, 200,
        "case {case} ({body}): expected HTTP 200, got {status} {response}"
    );
    assert!(
        response
            .get("InvoiceSummaries")
            .is_some_and(Value::is_array),
        "case {case} ({body}): expected a reported list of summaries, got {response}"
    );
}

/// Assert `body` is refused with the modelled `ValidationException`, whose message states
/// `must_state`.
async fn refused(router: &MultiServiceRouter, case: &str, body: &Value, must_state: &[&str]) {
    let (status, response) = call(router, body).await;
    assert_eq!(
        status, 400,
        "case {case} ({body}): expected HTTP 400, got {status} {response}"
    );
    let code = error_code(&response);
    assert_eq!(
        code, "ValidationException",
        "case {case} ({body}): expected ValidationException, got {code} ({response})"
    );
    let reason = response
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        reason, "invalidInput",
        "case {case} ({body}): expected the invalidInput reason, got {reason:?} ({response})"
    );
    let message = response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    for stated in must_state {
        assert!(
            message.contains(stated),
            "case {case} ({body}): the refusal must state {stated:?}, got {message:?}"
        );
    }
    assert!(
        response.get("InvoiceSummaries").is_none(),
        "case {case} ({body}): a refused request reports no summaries, got {response}"
    );
}

/// The seeded role is a value the MODEL declares, so the rows that filter by it are read
/// off the model rather than invented here.
#[test]
fn the_seeded_role_is_a_modelled_value() {
    assert!(
        RECEIVER_ROLE_VALUES.contains(&SEEDED_ROLE),
        "case seed: {SEEDED_ROLE:?} is not one of the modelled ReceiverRole values {RECEIVER_ROLE_VALUES:?}"
    );
}

/// RULE: a request that states a `ReceiverRole` is answered only about periods from the
/// date the model documents that data as beginning on — whichever member it states its
/// period in — while a request that states no `ReceiverRole` is answered about any period.
#[tokio::test]
async fn a_receiver_role_query_is_answered_only_from_the_data_start() {
    let (router, engine) = emulator();
    seed(&engine);

    // (the period the request states, whether it begins before the data start)
    let periods: &[(Value, bool)] = &[
        // A TimeInterval begins at its StartDate: the data start itself is in, one second
        // before it is out, and so is a period years earlier.
        (json!({"TimeInterval": interval("2025-06-01T00:00:00Z", "2025-07-01T00:00:00Z")}), false),
        (json!({"TimeInterval": interval("2025-05-31T23:59:59Z", "2025-06-30T00:00:00Z")}), true),
        (json!({"TimeInterval": interval("2024-01-15T00:00:00Z", "2024-02-15T00:00:00Z")}), true),
        (json!({"TimeInterval": interval("2025-06-15T00:00:00Z", "2025-07-15T00:00:00Z")}), false),
        // A BillingPeriod begins on the first day of the month it names: the month the
        // data starts in is in, the month before it is out. The bounds of `Year` are the
        // model's own (`smithy_query(query="shape", shape="BillingPeriod")`: @range(min:
        // 2005, max: 2050)).
        (json!({"BillingPeriod": {"Month": 6, "Year": 2025}}), false),
        (json!({"BillingPeriod": {"Month": 5, "Year": 2025}}), true),
        (json!({"BillingPeriod": {"Month": 12, "Year": 2024}}), true),
        (json!({"BillingPeriod": {"Month": 1, "Year": 2005}}), true),
        (json!({"BillingPeriod": {"Month": 7, "Year": 2025}}), false),
        (json!({"BillingPeriod": {"Month": 12, "Year": 2050}}), false),
        // Both members at once: either one before the data start refuses the request.
        (
            json!({
                "TimeInterval": interval("2025-06-01T00:00:00Z", "2025-07-01T00:00:00Z"),
                "BillingPeriod": {"Month": 1, "Year": 2024},
            }),
            true,
        ),
        (
            json!({
                "TimeInterval": interval("2025-06-01T00:00:00Z", "2025-07-01T00:00:00Z"),
                "BillingPeriod": {"Month": 6, "Year": 2025},
            }),
            false,
        ),
        // A request that states no period at all states nothing to refuse.
        (json!({}), false),
        (json!({"InvoicingEntity": "Amazon Web Services, Inc."}), false),
    ];

    for (period, before_data_start) in periods {
        // Every value the model declares for the member the rule hangs off, so the rule is
        // not keyed to one role.
        for role in RECEIVER_ROLE_VALUES {
            let mut filter = period.clone();
            filter["ReceiverRole"] = json!(role);
            let case = format!("{role} over {period}");
            if *before_data_start {
                refused(
                    &router,
                    &case,
                    &request(filter),
                    &[DATA_START, "ReceiverRole"],
                )
                .await;
            } else {
                admitted(&router, &case, &request(filter)).await;
            }
        }
        // Without a ReceiverRole the data start does not apply: the same period is
        // answered, whichever side of it the period falls on.
        admitted(
            &router,
            &format!("no ReceiverRole over {period}"),
            &request(period.clone()),
        )
        .await;
    }

    assert_eq!(
        invoice_count(&engine),
        1,
        "case the whole table: neither an answered nor a refused read changes the registry"
    );
}

/// RULE: a `TimeInterval` may be as long as the bound the model gives the filter it is
/// stated with — five years with a `ReceiverRole`, one month without one — and no longer,
/// measured in CALENDAR months from its `StartDate`.
#[tokio::test]
async fn an_interval_is_bounded_by_the_filter_it_is_stated_with() {
    let (router, engine) = emulator();
    seed(&engine);

    // (StartDate, EndDate, admitted without a ReceiverRole, admitted with one)
    let cases: &[(&str, &str, bool, bool)] = &[
        // Exactly one month is in under either bound; one second past it is out of the
        // one-month bound only.
        ("2025-06-01T00:00:00Z", "2025-07-01T00:00:00Z", true, true),
        ("2025-06-01T00:00:00Z", "2025-07-01T00:00:01Z", false, true),
        // A month is as long as the month the interval starts in: February is 28 days in
        // 2025 and 29 in 2024, and a month from the last day of January ends on the last
        // day of February.
        ("2025-02-01T00:00:00Z", "2025-03-01T00:00:00Z", true, true),
        ("2025-02-01T00:00:00Z", "2025-03-02T00:00:00Z", false, true),
        ("2024-02-01T00:00:00Z", "2024-03-01T00:00:00Z", true, true),
        ("2025-01-31T00:00:00Z", "2025-02-28T00:00:00Z", true, true),
        ("2025-01-31T00:00:00Z", "2025-03-01T00:00:00Z", false, true),
        ("2024-01-31T00:00:00Z", "2024-02-29T00:00:00Z", true, true),
        ("2024-01-31T00:00:00Z", "2024-03-01T00:00:00Z", false, true),
        // The time of day is part of the interval, not only the date.
        ("2025-06-01T09:30:00Z", "2025-07-01T09:30:00Z", true, true),
        ("2025-06-01T09:30:00Z", "2025-07-01T09:30:01Z", false, true),
        // Exactly five years is in with a ReceiverRole and out without one; one second
        // past it is out of both.
        ("2025-06-01T00:00:00Z", "2030-06-01T00:00:00Z", false, true),
        ("2025-06-01T00:00:00Z", "2030-06-01T00:00:01Z", false, false),
        // Five years from a leap day ends on the last day of that February.
        ("2028-02-29T00:00:00Z", "2033-02-28T00:00:00Z", false, true),
        ("2028-02-29T00:00:00Z", "2033-03-01T00:00:00Z", false, false),
        // More than one month but inside five years: the two bounds differ.
        ("2025-06-01T00:00:00Z", "2026-06-01T00:00:00Z", false, true),
        ("2025-06-01T00:00:00Z", "2025-08-01T00:00:00Z", false, true),
        // An empty interval and a reversed one are no longer than any bound.
        ("2025-06-01T00:00:00Z", "2025-06-01T00:00:00Z", true, true),
        ("2025-07-01T00:00:00Z", "2025-06-01T00:00:00Z", true, true),
    ];

    for (start, end, without_role, with_role) in cases {
        let stated = interval(start, end);
        let case = format!("[{start}, {end}) without a ReceiverRole");
        if *without_role {
            admitted(&router, &case, &request(json!({"TimeInterval": stated}))).await;
        } else {
            refused(
                &router,
                &case,
                &request(json!({"TimeInterval": stated})),
                &["TimeInterval", "one month"],
            )
            .await;
        }

        // With a ReceiverRole the five-year bound applies instead — and the data start
        // does too, so a row before it is not one this bound is read off.
        if secs(start) < secs(&format!("{DATA_START}T00:00:00Z")) {
            continue;
        }
        let case = format!("[{start}, {end}) with a ReceiverRole");
        let filter = json!({"TimeInterval": interval(start, end), "ReceiverRole": SEEDED_ROLE});
        if *with_role {
            admitted(&router, &case, &request(filter)).await;
        } else {
            refused(&router, &case, &request(filter), &["TimeInterval", "5 years"]).await;
        }
    }

    assert_eq!(
        invoice_count(&engine),
        1,
        "case the whole table: neither an answered nor a refused read changes the registry"
    );
}

/// RULE, over a whole year: an interval of exactly one calendar month from the first of
/// any month is admitted without a `ReceiverRole`, and one second more is refused —
/// whatever that month's length.
#[tokio::test]
async fn the_one_month_bound_follows_every_calendar_month() {
    let (router, engine) = emulator();
    seed(&engine);

    // 2024 is a leap year, so February is 29 days there and 28 in 2025.
    for year in [2024, 2025] {
        for month in 1..=12 {
            let start = format!("{year}-{month:02}-01T00:00:00Z");
            let limit = months_after(&start, 1);
            admitted(
                &router,
                &format!("a month from {start}"),
                &request(json!({
                    "TimeInterval": {"StartDate": secs(&start), "EndDate": limit},
                })),
            )
            .await;
            refused(
                &router,
                &format!("a month and a second from {start}"),
                &request(json!({
                    "TimeInterval": {"StartDate": secs(&start), "EndDate": limit + 1},
                })),
                &["TimeInterval", "one month"],
            )
            .await;
        }
    }

    assert_eq!(
        invoice_count(&engine),
        1,
        "case the whole year: neither an answered nor a refused read changes the registry"
    );
}

/// RULE: the refusal PRECEDES the registry read, so a request outside the rules is
/// answered the same way whatever the registry holds — including about an invoice id the
/// registry does not hold, which would otherwise be the modelled 404.
#[tokio::test]
async fn a_period_outside_the_rules_is_refused_before_the_registry_is_read() {
    // An empty registry and a seeded one answer the same refusals.
    for seeded in [false, true] {
        let (router, engine) = emulator();
        if seeded {
            seed(&engine);
        }

        // (the selector, the filter, what the refusal must state)
        let cases: &[(Value, Value, &[&str])] = &[
            (
                json!({"ResourceType": "ACCOUNT_ID", "Value": ACCOUNT}),
                json!({
                    "ReceiverRole": SEEDED_ROLE,
                    "TimeInterval": interval("2024-01-15T00:00:00Z", "2024-02-15T00:00:00Z"),
                }),
                &[DATA_START, "ReceiverRole"],
            ),
            // An invoice id the registry does not hold: the period is decided first, so
            // this is the 400 rather than the 404 the same selector gets on its own.
            (
                json!({"ResourceType": "INVOICE_ID", "Value": "INV-999"}),
                json!({"TimeInterval": interval("2025-06-01T00:00:00Z", "2025-09-01T00:00:00Z")}),
                &["TimeInterval", "one month"],
            ),
            // The invoice the registry DOES hold, asked about over a refused interval.
            (
                json!({"ResourceType": "INVOICE_ID", "Value": SEEDED_INVOICE_ID}),
                json!({"TimeInterval": interval("2025-06-01T00:00:00Z", "2025-09-01T00:00:00Z")}),
                &["TimeInterval", "one month"],
            ),
        ];

        for (selector, filter, must_state) in cases {
            let body = json!({"Selector": selector, "Filter": filter});
            refused(
                &router,
                &format!("seeded={seeded}"),
                &body,
                must_state,
            )
            .await;
        }

        // The same selector WITHOUT a refused period is answered about the registry: the
        // unknown invoice id is the modelled 404.
        let (status, response) = call(
            &router,
            &json!({"Selector": {"ResourceType": "INVOICE_ID", "Value": "INV-999"}}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case seeded={seeded}: an unknown invoice id with no refused period is the modelled 404, got {status} {response}"
        );

        assert_eq!(
            invoice_count(&engine),
            usize::from(seeded),
            "case seeded={seeded}: a refused read creates nothing"
        );
    }
}
