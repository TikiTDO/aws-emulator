// SPDX-License-Identifier: CC0-1.0

//! `ListInvoiceSummaries`, driven end to end over this service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router
//! uses: `POST /` with `X-Amz-Target: Invoicing.ListInvoiceSummaries` and an awsJson
//! body, so what these tests see is exactly what an SDK client sees.
//!
//! The one thing these tests reach into the state struct for is SEEDING the invoice
//! registry: an invoice is issued by AWS out of a billing period's activity and no
//! operation of this service's API creates one, so there is no request that could put
//! one there (the same shape as the fixture catalogs other services in this repo seed).
//! Everything the operation answers is then read back over the wire.
//!
//! The tables below span the rules the operation implements rather than one recorded
//! request: which invoices each `Selector` kind selects, what each `Filter` member
//! tests, the inclusive/exclusive bounds of a `TimeInterval`, pagination at every page
//! size up to past the result count, the published bounds of `MaxResults` and one past
//! each, and the refusals — an unmodelled enum value, a `NextToken` this service did
//! not mint, and an invoice id the registry does not hold.
//!
//! Every instant below is stated as the date it is, and every interval a row states is
//! one the model documents the operation as answering about (see
//! `tests/all/invoice_summary_period_rules.rs`, which is where those temporal rules
//! themselves are asserted): the invoices are issued on dates from which a one-month
//! window is enough to tell the bounds of an interval apart.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::{
    Entity, InvoiceSummary, RECEIVER_ROLE_VALUES,
};

/// The epoch second an ISO-8601 instant names, so the dates below read as dates rather
/// than as epoch counts.
fn secs(instant: &str) -> i64 {
    ::chrono::DateTime::parse_from_rfc3339(instant)
        .unwrap_or_else(|error| panic!("case {instant:?}: a valid instant, got {error}"))
        .timestamp()
}

/// The account every row calls `ListInvoiceSummaries` for unless it says otherwise.
const ACCOUNT_A: &str = "111111111111";
/// The account that is only ever a BILL SOURCE of an invoice, never its receiver.
const ACCOUNT_B: &str = "222222222222";
/// The account `INV-003` is issued to, while account A is its bill source.
const ACCOUNT_C: &str = "333333333333";
/// The account `INV-004` is issued to; nothing links it to account A.
const ACCOUNT_D: &str = "444444444444";
/// An account the registry holds no invoice for.
const ACCOUNT_WITHOUT_INVOICES: &str = "999999999999";

/// One invoice to put in the registry, in the terms the modelled `InvoiceSummary`
/// states it.
#[derive(Debug, Clone, Copy)]
struct Seed {
    invoice_id: &'static str,
    account_id: &'static str,
    bill_source_accounts: &'static [&'static str],
    /// The `IssuedDate`, as the ISO-8601 instant it is.
    issued_at: &'static str,
    /// `(Month, Year)` of the invoice's `BillingPeriod`.
    billing_period: (i32, i32),
    invoicing_entity: &'static str,
    /// One of the model's `ReceiverRole` values.
    receiver_role: &'static str,
}

impl Seed {
    fn to_summary(self) -> InvoiceSummary {
        InvoiceSummary {
            account_id: Some(self.account_id.to_string()),
            invoice_id: Some(self.invoice_id.to_string()),
            issued_date: Some(::aws_smithy_types::DateTime::from_secs(secs(self.issued_at))),
            bill_source_accounts: Some(
                self.bill_source_accounts
                    .iter()
                    .map(|account| (*account).to_string())
                    .collect(),
            ),
            bill_source_accounts_total_count: Some(
                i32::try_from(self.bill_source_accounts.len()).unwrap_or(i32::MAX),
            ),
            receiver_role: Some(self.receiver_role.to_string()),
            entity: Some(Entity {
                invoicing_entity: Some(self.invoicing_entity.to_string()),
                billing_entity: Some("AWS".to_string()),
            }),
            billing_period: Some(
                service_com_amazonaws_invoicing_invoicing::ops::types::BillingPeriod {
                    month: self.billing_period.0,
                    year: self.billing_period.1,
                },
            ),
            ..Default::default()
        }
    }
}

/// The invoices every test below seeds, differing along each dimension the rules
/// generalise over: the account an invoice is issued to, the accounts it bills, the day
/// it was issued, its billing period, its invoicing entity and its receiver role. One
/// invoice a month, each issued inside the billing period it reports.
///
/// The `ReceiverRole` values are the model's own
/// (`smithy_query(query="shape", shape="ReceiverRole")` -> `SELLER`, `RESELLER`,
/// `BUYER`), pinned against that catalog by [`seeded_roles_are_modelled_values`].
const SEEDS: &[Seed] = &[
    Seed {
        invoice_id: "INV-001",
        account_id: ACCOUNT_A,
        bill_source_accounts: &[],
        issued_at: "2025-07-15T00:00:00Z",
        billing_period: (7, 2025),
        invoicing_entity: "Amazon Web Services, Inc.",
        receiver_role: "BUYER",
    },
    Seed {
        invoice_id: "INV-002",
        account_id: ACCOUNT_A,
        bill_source_accounts: &[ACCOUNT_B],
        issued_at: "2025-08-15T00:00:00Z",
        billing_period: (8, 2025),
        invoicing_entity: "Amazon Web Services EMEA SARL",
        receiver_role: "BUYER",
    },
    Seed {
        invoice_id: "INV-003",
        account_id: ACCOUNT_C,
        bill_source_accounts: &[ACCOUNT_A],
        issued_at: "2025-09-15T00:00:00Z",
        billing_period: (9, 2025),
        invoicing_entity: "Amazon Web Services, Inc.",
        receiver_role: "SELLER",
    },
    Seed {
        invoice_id: "INV-004",
        account_id: ACCOUNT_D,
        bill_source_accounts: &[],
        issued_at: "2025-10-15T00:00:00Z",
        billing_period: (10, 2025),
        invoicing_entity: "Amazon Internet Services Pvt. Ltd",
        receiver_role: "RESELLER",
    },
];

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services. The engine comes back so a
/// test can seed the invoice registry the service reads.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Put `seeds` in the service's invoice registry, keyed by invoice id.
fn seed(engine: &engine::InMemoryEngine, seeds: &[Seed]) {
    let state = service_com_amazonaws_invoicing_invoicing::state::service_state(engine)
        .expect("the shared engine holds this service's state");
    let mut store = state.lock();
    for entry in seeds {
        store.invoices.insert(
            entry.invoice_id.to_string(),
            service_com_amazonaws_invoicing_invoicing::state::StoredInvoice::new(
                entry.to_summary(),
            ),
        );
    }
}

/// How many invoices the registry holds, so a read can be shown not to change it.
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

/// The same call, asserting it succeeded.
async fn ok(router: &MultiServiceRouter, body: &Value) -> Value {
    let (status, parsed) = call(router, body).await;
    assert_eq!(
        status, 200,
        "case {body}: expected HTTP 200, got {status} {parsed}"
    );
    parsed
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), without
/// the namespace a body may qualify it with.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `InvoiceId` of every summary in a response, in the order they were reported.
fn invoice_ids(body: &Value) -> Vec<String> {
    let summaries = body
        .get("InvoiceSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    summaries
        .iter()
        .map(|summary| {
            summary
                .get("InvoiceId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// A `Selector` as the request spells it.
fn selector(resource_type: &str, value: &str) -> Value {
    json!({"ResourceType": resource_type, "Value": value})
}

/// The seeded `ReceiverRole` values are values the MODEL declares, so the role rows
/// below are a catalog read off the model rather than wording invented here.
#[test]
fn seeded_roles_are_modelled_values() {
    for entry in SEEDS {
        assert!(
            RECEIVER_ROLE_VALUES.contains(&entry.receiver_role),
            "case {entry:?}: {:?} is not one of the modelled ReceiverRole values {RECEIVER_ROLE_VALUES:?}",
            entry.receiver_role
        );
    }
}

/// An `ACCOUNT_ID` selector reports every invoice LINKED to that account — the ones it
/// was issued and the ones it is the bill source of — and an account the registry holds
/// nothing for is the empty list, not a failure.
#[tokio::test]
async fn an_account_selector_reports_the_invoices_linked_to_that_account() {
    let (router, engine) = emulator();
    seed(&engine, SEEDS);

    // (the account asked about, the invoice ids it is linked to)
    let cases: &[(&str, &[&str])] = &[
        // Issued two invoices, and the bill source of a third.
        (ACCOUNT_A, &["INV-001", "INV-002", "INV-003"]),
        // Only ever a bill source.
        (ACCOUNT_B, &["INV-002"]),
        // Issued one invoice.
        (ACCOUNT_C, &["INV-003"]),
        (ACCOUNT_D, &["INV-004"]),
        // Linked to nothing.
        (ACCOUNT_WITHOUT_INVOICES, &[]),
    ];

    for (account, expected) in cases {
        let body = json!({"Selector": selector("ACCOUNT_ID", account)});
        let response = ok(&router, &body).await;
        let actual = invoice_ids(&response);
        assert_eq!(
            actual, *expected,
            "case {account}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            response.get("NextToken").is_none(),
            "case {account}: one page holds the whole answer, so no NextToken is due, got {response}"
        );
    }
}

/// An `INVOICE_ID` selector reports that one invoice; an id the registry does not hold
/// is the modelled 404, because the request names a resource rather than a set.
#[tokio::test]
async fn an_invoice_selector_reports_one_invoice_or_404s() {
    let (router, engine) = emulator();
    seed(&engine, SEEDS);

    for entry in SEEDS {
        let body = json!({"Selector": selector("INVOICE_ID", entry.invoice_id)});
        let response = ok(&router, &body).await;
        let actual = invoice_ids(&response);
        assert_eq!(
            actual,
            vec![entry.invoice_id.to_string()],
            "case {}: expected exactly that invoice, got {actual:?}",
            entry.invoice_id
        );
    }

    for unknown in ["INV-999", "", "INV-001 "] {
        let body = json!({"Selector": selector("INVOICE_ID", unknown)});
        let (status, response) = call(&router, &body).await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: expected HTTP 404, got {status} {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {unknown:?}: expected ResourceNotFoundException, got {code} ({response})"
        );
    }
}

/// Each `Filter` member narrows the answer by what that member tests, and members of
/// different types are AND-ed.
#[tokio::test]
async fn each_filter_member_narrows_the_answer() {
    let (router, engine) = emulator();
    seed(&engine, SEEDS);

    // (the filter, the account asked about, the invoice ids it admits)
    let cases: &[(Value, &str, &[&str])] = &[
        // BillingPeriod: the invoice's own (Month, Year).
        (json!({"BillingPeriod": {"Month": 7, "Year": 2025}}), ACCOUNT_A, &["INV-001"]),
        (json!({"BillingPeriod": {"Month": 9, "Year": 2025}}), ACCOUNT_A, &["INV-003"]),
        // A period no seeded invoice covers, and the same month in another year.
        (json!({"BillingPeriod": {"Month": 12, "Year": 2024}}), ACCOUNT_A, &[]),
        (json!({"BillingPeriod": {"Month": 7, "Year": 2026}}), ACCOUNT_A, &[]),
        // InvoicingEntity: the invoice's Entity.InvoicingEntity.
        (
            json!({"InvoicingEntity": "Amazon Web Services, Inc."}),
            ACCOUNT_A,
            &["INV-001", "INV-003"],
        ),
        (
            json!({"InvoicingEntity": "Amazon Web Services EMEA SARL"}),
            ACCOUNT_A,
            &["INV-002"],
        ),
        (
            json!({"InvoicingEntity": "Amazon Internet Services Pvt. Ltd"}),
            ACCOUNT_A,
            &[],
        ),
        // ReceiverRole: every value the model declares.
        (json!({"ReceiverRole": "BUYER"}), ACCOUNT_A, &["INV-001", "INV-002"]),
        (json!({"ReceiverRole": "SELLER"}), ACCOUNT_A, &["INV-003"]),
        (json!({"ReceiverRole": "RESELLER"}), ACCOUNT_A, &[]),
        (json!({"ReceiverRole": "RESELLER"}), ACCOUNT_D, &["INV-004"]),
        // Two members together: both have to hold.
        (
            json!({"ReceiverRole": "BUYER", "BillingPeriod": {"Month": 8, "Year": 2025}}),
            ACCOUNT_A,
            &["INV-002"],
        ),
        (
            json!({"ReceiverRole": "SELLER", "BillingPeriod": {"Month": 8, "Year": 2025}}),
            ACCOUNT_A,
            &[],
        ),
        (
            json!({
                "ReceiverRole": "BUYER",
                "TimeInterval": {
                    "StartDate": secs("2025-07-15T00:00:00Z"),
                    "EndDate": secs("2025-09-16T00:00:00Z"),
                },
                "InvoicingEntity": "Amazon Web Services, Inc.",
            }),
            ACCOUNT_A,
            &["INV-001"],
        ),
    ];

    for (filter, account, expected) in cases {
        let body = json!({"Selector": selector("ACCOUNT_ID", account), "Filter": filter});
        let response = ok(&router, &body).await;
        let actual = invoice_ids(&response);
        assert_eq!(
            actual, *expected,
            "case {account} {filter}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// A `TimeInterval` admits an `IssuedDate` from `StartDate` INCLUSIVE up to `EndDate`
/// EXCLUSIVE, which is what both members document.
///
/// Every row states an interval of at most one month, which is the bound the model gives
/// an interval stated without a `ReceiverRole`.
#[tokio::test]
async fn a_time_interval_is_start_inclusive_and_end_exclusive() {
    let (router, engine) = emulator();
    seed(&engine, SEEDS);

    // (StartDate, EndDate, the invoice ids of account A it admits)
    let cases: &[(&str, &str, &[&str])] = &[
        // The start bound itself is in; the end bound itself is out.
        ("2025-07-15T00:00:00Z", "2025-08-15T00:00:00Z", &["INV-001"]),
        ("2025-08-15T00:00:00Z", "2025-09-15T00:00:00Z", &["INV-002"]),
        ("2025-09-15T00:00:00Z", "2025-10-15T00:00:00Z", &["INV-003"]),
        // One second past an issued date excludes it; one second before admits it.
        ("2025-07-15T00:00:01Z", "2025-08-15T00:00:00Z", &[]),
        ("2025-07-01T00:00:00Z", "2025-07-15T00:00:00Z", &[]),
        ("2025-07-01T00:00:00Z", "2025-07-15T00:00:01Z", &["INV-001"]),
        // An empty interval admits nothing, and so does a reversed one.
        ("2025-08-15T00:00:00Z", "2025-08-15T00:00:00Z", &[]),
        ("2025-09-15T00:00:00Z", "2025-07-15T00:00:00Z", &[]),
        // A window past every issued date.
        ("2025-11-01T00:00:00Z", "2025-11-02T00:00:00Z", &[]),
    ];

    for (start, end, expected) in cases {
        let body = json!({
            "Selector": selector("ACCOUNT_ID", ACCOUNT_A),
            "Filter": {"TimeInterval": {"StartDate": secs(start), "EndDate": secs(end)}},
        });
        let response = ok(&router, &body).await;
        let actual = invoice_ids(&response);
        assert_eq!(
            actual, *expected,
            "case [{start}, {end}): expected {expected:?}, got {actual:?}"
        );
    }
}

/// Pagination: a page holds at most `MaxResults`, the `NextToken` resumes where it
/// stopped, the last page carries none, and the pages together are the whole answer
/// exactly once — whatever the page size.
#[tokio::test]
async fn pagination_walks_the_whole_answer_once_at_every_page_size() {
    let (router, engine) = emulator();
    seed(&engine, SEEDS);

    let whole_answer = ["INV-001", "INV-002", "INV-003"];
    // Every size up to one past the result count, plus an omitted MaxResults and the
    // model's maximum.
    for page_size in [Some(1), Some(2), Some(3), Some(4), Some(100), None] {
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({"Selector": selector("ACCOUNT_ID", ACCOUNT_A)});
            if let Some(size) = page_size {
                body["MaxResults"] = json!(size);
            }
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let response = ok(&router, &body).await;
            let page = invoice_ids(&response);
            let limit = page_size.unwrap_or(100);
            assert!(
                page.len() <= usize::try_from(limit).unwrap_or(usize::MAX),
                "case MaxResults={page_size:?}: a page holds at most {limit}, got {page:?}"
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= whole_answer.len() + 1,
                "case MaxResults={page_size:?}: the walk must terminate, got {collected:?} over {pages} pages"
            );
            next_token = response
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, whole_answer,
            "case MaxResults={page_size:?}: expected {whole_answer:?} across the pages, got {collected:?}"
        );
    }
}

/// `MaxResults` is refused outside the range the model publishes, and accepted on each
/// bound.
#[tokio::test]
async fn max_results_is_bounded_by_the_modelled_range() {
    let (router, engine) = emulator();
    seed(&engine, SEEDS);

    // `InvoiceSummariesMaxResults` carries @range(min: 1, max: 100)
    // (`smithy_query(query="operation", operation="ListInvoiceSummaries")`).
    for (max_results, expected_status) in [(1, 200), (2, 200), (100, 200), (0, 400), (101, 400)] {
        let body = json!({
            "Selector": selector("ACCOUNT_ID", ACCOUNT_A),
            "MaxResults": max_results,
        });
        let (status, response) = call(&router, &body).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} {response}"
        );
        if expected_status == 400 {
            let code = error_code(&response);
            assert_eq!(
                code, "ValidationException",
                "case MaxResults={max_results}: expected ValidationException, got {code} ({response})"
            );
        }
    }
}

/// A `NextToken` this service did not mint is refused, with the model's own
/// `invalidNextToken` reason — never served as a position in the walk.
#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let (router, engine) = emulator();
    seed(&engine, SEEDS);

    for token in [
        "not-a-token",
        // Hex, but not of anything this operation minted.
        "deadbeef",
        // The plaintext a token encodes, sent unencoded.
        "invoice-summaries:INV-001",
    ] {
        let body = json!({
            "Selector": selector("ACCOUNT_ID", ACCOUNT_A),
            "NextToken": token,
        });
        let (status, response) = call(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, "ValidationException",
            "case {token:?}: expected ValidationException, got {code} ({response})"
        );
        let reason = response.get("reason").and_then(Value::as_str).unwrap_or_default();
        assert_eq!(
            reason, "invalidNextToken",
            "case {token:?}: expected the invalidNextToken reason, got {reason:?} ({response})"
        );
    }
}

/// A member targeting an enum shape admits only the values the model declares: anything
/// else is a 400, never a page of results and never a 501.
#[tokio::test]
async fn an_unmodelled_enum_value_is_refused() {
    let (router, engine) = emulator();
    seed(&engine, SEEDS);

    // (the request, the member it is refused for)
    let cases: &[(Value, &str)] = &[
        (json!({"Selector": selector("ARN", ACCOUNT_A)}), "selector.resourceType"),
        (json!({"Selector": selector("account_id", ACCOUNT_A)}), "selector.resourceType"),
        (json!({"Selector": selector("", ACCOUNT_A)}), "selector.resourceType"),
        (
            json!({
                "Selector": selector("ACCOUNT_ID", ACCOUNT_A),
                "Filter": {"ReceiverRole": "PAYER"},
            }),
            "filter.receiverRole",
        ),
        (
            json!({
                "Selector": selector("ACCOUNT_ID", ACCOUNT_A),
                "Filter": {"ReceiverRole": "buyer"},
            }),
            "filter.receiverRole",
        ),
    ];

    for (body, member) in cases {
        let (status, response) = call(&router, body).await;
        assert_eq!(
            status, 400,
            "case {body}: expected HTTP 400, got {status} {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, "ValidationException",
            "case {body}: expected ValidationException, got {code} ({response})"
        );
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(member),
            "case {body}: the refusal must name '{member}', got {message:?}"
        );
    }
}

/// A fresh emulator holds no invoice, so an account read answers the empty list with
/// HTTP 200 — the answer AWS owes for an account with no invoices — and a named invoice
/// is a 404.
#[tokio::test]
async fn an_empty_registry_answers_the_empty_list() {
    let (router, engine) = emulator();

    let body = json!({"Selector": selector("ACCOUNT_ID", ACCOUNT_A)});
    let response = ok(&router, &body).await;
    let actual = invoice_ids(&response);
    assert!(
        actual.is_empty(),
        "case empty registry: expected no summaries, got {actual:?}"
    );
    assert!(
        response
            .get("InvoiceSummaries")
            .is_some_and(Value::is_array),
        "case empty registry: InvoiceSummaries is required, so the list is reported empty rather than omitted, got {response}"
    );
    assert!(
        response.get("NextToken").is_none(),
        "case empty registry: nothing is left to resume at, got {response}"
    );

    let (status, named) = call(
        &router,
        &json!({"Selector": selector("INVOICE_ID", "INV-001")}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case empty registry, INVOICE_ID: expected HTTP 404, got {status} {named}"
    );
    assert_eq!(
        invoice_count(&engine),
        0,
        "case empty registry: a read must not create an invoice"
    );
}

/// The operation REPORTS: two identical reads answer identically and the registry is
/// unchanged by either, so what one caller reads does not change what the next sees.
#[tokio::test]
async fn a_read_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator();
    seed(&engine, SEEDS);
    let before = invoice_count(&engine);

    let body = json!({
        "Selector": selector("ACCOUNT_ID", ACCOUNT_A),
        "MaxResults": 1,
    });
    let first = ok(&router, &body).await;
    let second = ok(&router, &body).await;
    assert_eq!(
        first, second,
        "case repeated read: expected {first}, got {second}"
    );

    // And a page walked to its end leaves the registry exactly as it was.
    let mut token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    while let Some(next) = token {
        let mut paged = body.clone();
        paged["NextToken"] = json!(next);
        let response = ok(&router, &paged).await;
        token = response
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    let after = invoice_count(&engine);
    assert_eq!(
        after, before,
        "case walked pagination: expected the registry to still hold {before} invoices, got {after}"
    );
}
