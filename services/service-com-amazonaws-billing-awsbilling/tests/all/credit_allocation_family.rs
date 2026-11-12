// SPDX-License-Identifier: CC0-1.0

//! `GetCreditAllocationHistory` across the consolidated billing family, end to
//! end over the awsJson1_0 wire.
//!
//! The RULE under test: a billing history read for an account covers the bills
//! of every account that account pays for — its own always, and, when it is the
//! management (payer) account of the consolidated billing family
//! (`ComAmazonawsBillingAwsbillingState::consolidated_billing_family`), the
//! bills of every member of that family, which is how a credit applied to one
//! account's bill is reported as a cross-account application in the payer's
//! history. An account that pays for nobody — a member account, or an account
//! outside the family — sees only its own bills.
//!
//! The table below spans what the rule generalises over: the requested
//! account's place in the family (payer, each member, an account outside it),
//! the account a seeded allocation was applied to, the billing month (including
//! two accounts billed in the SAME month for the same credit), the credit id,
//! the currency and the page size.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::state::{CreditAllocation, CreditAmount};
use service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState;

use crate::billing_views::{call, standalone_emulator_with_engine, ACCOUNT_ID};
use crate::credit_allocation_history::at;

/// A member of the emulated account's consolidated billing family.
const MEMBER_ACCOUNT_ID: &str = "210987654321";

/// A second member, so the traversal is shown to reach more than one.
const SECOND_MEMBER_ACCOUNT_ID: &str = "333333333333";

/// A 12-digit account that is NOT in the family: nothing the payer pays for.
const OUTSIDE_ACCOUNT_ID: &str = "999999999999";

/// The display name a family member carries in
/// `consolidated_billing_family` (account id -> display name).
const MEMBER_NAME: &str = "Member Account";

/// One allocation entry, identified by everything that distinguishes one
/// application of one credit: billing month, credit id, the account billed and
/// the service it was applied to.
type EntryId = (String, String, String, String);

/// One persisted allocation this suite seeds.
struct Seed {
    credit_id: &'static str,
    account_id: &'static str,
    billing_month: &'static str,
    applied_service_name: &'static str,
    currency_code: &'static str,
    currency_amount: &'static str,
}

impl Seed {
    fn allocation(&self) -> CreditAllocation {
        CreditAllocation {
            credit_id: self.credit_id.to_string(),
            credit_amount: CreditAmount {
                currency_code: self.currency_code.to_string(),
                currency_amount: self.currency_amount.to_string(),
            },
            description: None,
            account_id: self.account_id.to_string(),
            applied_service_name: self.applied_service_name.to_string(),
            billing_month: self.billing_month.to_string(),
            is_estimated_bill: false,
        }
    }

    fn id(&self) -> EntryId {
        (
            self.billing_month.to_string(),
            self.credit_id.to_string(),
            self.account_id.to_string(),
            self.applied_service_name.to_string(),
        )
    }
}

/// Allocations applied to the bills of the payer account, of both family
/// members and of an account outside the family. Service names are real Amazon
/// Web Services service names and the amounts are modeled `CurrencyAmount`
/// decimal strings.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            credit_id: "7",
            account_id: ACCOUNT_ID,
            billing_month: "2025-03",
            applied_service_name: "Amazon Elastic Compute Cloud",
            currency_code: "USD",
            currency_amount: "10.00",
        },
        // The same credit, the same billing month, a DIFFERENT account's bill:
        // the cross-account application the family traversal captures.
        Seed {
            credit_id: "7",
            account_id: MEMBER_ACCOUNT_ID,
            billing_month: "2025-03",
            applied_service_name: "AWS Lambda",
            currency_code: "USD",
            currency_amount: "1.50",
        },
        Seed {
            credit_id: "42",
            account_id: MEMBER_ACCOUNT_ID,
            billing_month: "2025-05",
            applied_service_name: "Amazon Simple Storage Service",
            currency_code: "USD",
            currency_amount: "3.00",
        },
        Seed {
            credit_id: "7",
            account_id: SECOND_MEMBER_ACCOUNT_ID,
            billing_month: "2025-04",
            applied_service_name: "AmazonCloudWatch",
            currency_code: "EUR",
            currency_amount: "2.00",
        },
        // Outside the family: no traversal reaches it.
        Seed {
            credit_id: "7",
            account_id: OUTSIDE_ACCOUNT_ID,
            billing_month: "2025-04",
            applied_service_name: "AWS Lambda",
            currency_code: "USD",
            currency_amount: "9.99",
        },
    ]
}

/// Put both members in the consolidated billing family and persist every seeded
/// allocation in the shared engine's store.
fn seed(engine: &Arc<engine::InMemoryEngine>, seeds: &[Seed]) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    for member in [MEMBER_ACCOUNT_ID, SECOND_MEMBER_ACCOUNT_ID] {
        state
            .consolidated_billing_family
            .insert(member.to_string(), MEMBER_NAME.to_string());
    }
    for seed in seeds {
        assert!(
            state.record_credit_allocation(seed.allocation()),
            "case {:?}: the seeded allocation must fit in the store",
            seed.id()
        );
    }
}

/// The entries a response reports, in the order it reports them.
fn reported(body: &Value) -> Vec<EntryId> {
    body["creditAllocationHistoryList"]
        .as_array()
        .expect("creditAllocationHistoryList is a list")
        .iter()
        .map(|entry| {
            let field = |name: &str| entry[name].as_str().unwrap_or_default().to_string();
            (
                field("billingMonth"),
                field("creditId"),
                field("accountId"),
                field("appliedServiceName"),
            )
        })
        .collect()
}

/// The entries the rule says a history read for `requested` must report, newest
/// billing month first — the store's own key order, reversed.
///
/// `credit_only` narrows to one credit, as the request's `creditId` filter does.
fn expected_for(requested: &str, credit_only: Option<&str>) -> Vec<EntryId> {
    let payer = requested == ACCOUNT_ID;
    let family = [ACCOUNT_ID, MEMBER_ACCOUNT_ID, SECOND_MEMBER_ACCOUNT_ID];
    let mut ids: Vec<EntryId> = seeds()
        .iter()
        .filter(|seed| {
            seed.account_id == requested || (payer && family.contains(&seed.account_id))
        })
        .filter(|seed| credit_only.is_none_or(|credit| seed.credit_id == credit))
        .map(Seed::id)
        .collect();
    // `CreditAllocation::key` orders by billing month, then credit id as a
    // number, then account, then service; the operation walks that order in
    // reverse.
    ids.sort_by(|left, right| {
        let number = |id: &EntryId| id.1.parse::<i128>().unwrap_or_default();
        right
            .0
            .cmp(&left.0)
            .then_with(|| number(right).cmp(&number(left)))
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| right.3.cmp(&left.3))
    });
    ids
}

/// A request covering every seeded billing month for `account`.
fn whole_range(account: &str) -> Value {
    json!({
        "accountId": account,
        "startDate": at(2025, 1, 1, 0),
        "endDate": at(2025, 12, 31, 23),
    })
}

#[tokio::test]
async fn a_history_read_traverses_the_consolidated_billing_family_of_the_payer_account() {
    let (router, engine) = standalone_emulator_with_engine();
    seed(&engine, &seeds());

    // (case, the account whose history is requested)
    let cases: Vec<(&str, &str)> = vec![
        ("the family's management (payer) account", ACCOUNT_ID),
        ("a member account", MEMBER_ACCOUNT_ID),
        ("the second member account", SECOND_MEMBER_ACCOUNT_ID),
        ("an account outside the family", OUTSIDE_ACCOUNT_ID),
    ];

    for (case, requested) in cases {
        let (status, body) = call(
            &router,
            "GetCreditAllocationHistory",
            &whole_range(requested),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        let expected = expected_for(requested, None);
        assert_eq!(
            reported(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reported(&body)
        );
        let months: Vec<String> = reported(&body).into_iter().map(|id| id.0).collect();
        let mut descending = months.clone();
        descending.sort_by(|left, right| right.cmp(left));
        assert_eq!(
            months, descending,
            "case {case:?}: entries must be sorted by billingMonth descending, got {months:?}"
        );
    }
}

#[tokio::test]
async fn the_credit_id_filter_narrows_the_family_traversal() {
    let (router, engine) = standalone_emulator_with_engine();
    seed(&engine, &seeds());

    // (case, the account requested, the creditId filter)
    let cases: Vec<(&str, &str, &str)> = vec![
        ("payer, a credit applied across the family", ACCOUNT_ID, "7"),
        ("payer, a credit applied to one member only", ACCOUNT_ID, "42"),
        ("member, the family-wide credit", MEMBER_ACCOUNT_ID, "7"),
        ("outside account, the family-wide credit", OUTSIDE_ACCOUNT_ID, "7"),
    ];

    for (case, requested, credit) in cases {
        let mut request = whole_range(requested);
        request["creditId"] = json!(credit.parse::<i64>().expect("a numeric credit id"));
        let (status, body) = call(&router, "GetCreditAllocationHistory", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        let expected = expected_for(requested, Some(credit));
        assert_eq!(
            reported(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reported(&body)
        );
    }
}

#[tokio::test]
async fn the_family_traversal_pages_without_gaps_or_repeats() {
    let (router, engine) = standalone_emulator_with_engine();
    seed(&engine, &seeds());

    let whole = expected_for(ACCOUNT_ID, None);
    for page_size in [1_usize, 2, 3, whole.len(), whole.len() + 1] {
        let case = format!("maxResults {page_size}");
        let mut walked: Vec<EntryId> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=whole.len() {
            let mut request = whole_range(ACCOUNT_ID);
            request["maxResults"] = json!(page_size);
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "GetCreditAllocationHistory", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body:?}"
            );
            let page = reported(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} entries, got {page:?}"
            );
            walked.extend(page);
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end without a further token, got {token:?}"
        );
        assert_eq!(
            walked, whole,
            "case {case:?}: paging must report every family entry once, in order, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_family_history_read_writes_nothing() {
    let (router, engine) = standalone_emulator_with_engine();
    let seeded = seeds();
    seed(&engine, &seeded);

    let before = {
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let state = state.lock();
        (
            state.credit_allocations.clone(),
            state.consolidated_billing_family.clone(),
        )
    };

    for requested in [
        ACCOUNT_ID,
        MEMBER_ACCOUNT_ID,
        SECOND_MEMBER_ACCOUNT_ID,
        OUTSIDE_ACCOUNT_ID,
    ] {
        let (status, body) = call(
            &router,
            "GetCreditAllocationHistory",
            &whole_range(requested),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {requested:?}: expected 200, got {status} {body:?}"
        );
    }

    let after = {
        let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
        let state = state.lock();
        (
            state.credit_allocations.clone(),
            state.consolidated_billing_family.clone(),
        )
    };
    assert_eq!(
        after.0, before.0,
        "case \"reads do not write\": the allocation store must be untouched, got {:?}",
        after.0
    );
    assert_eq!(
        after.1, before.1,
        "case \"reads do not write\": the billing family must be untouched, got {:?}",
        after.1
    );
    assert_eq!(
        before.0.len(),
        seeded.len(),
        "case \"reads do not write\": every seeded allocation is resident, got {}",
        before.0.len()
    );
}
