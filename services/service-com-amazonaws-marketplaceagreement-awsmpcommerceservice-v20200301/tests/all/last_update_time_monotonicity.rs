// SPDX-License-Identifier: CC0-1.0

//! What one agreement's reported last-update time does when a call CHANGES the
//! record and the clock owes that same record a transition, end to end over
//! HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: the instant one agreement reports as its last update
//! never moves backwards. `AgreementViewSummary$lastUpdateTime` is "when the
//! agreement was last updated. An agreement is updated when any of its
//! attributes or accepted terms change. Amendments, renewals, and a party
//! changing whether the agreement renews are all examples"
//! (`smithy_query(query="doc", shape="AgreementViewSummary",
//! member="lastUpdateTime")`), so it reports the LATEST change the record
//! carries: a call that changes the agreement reports a last-update time no
//! earlier than that call, and materialising a transition the clock owed at an
//! earlier instant — the expiry of an agreement whose "endTime" has passed, or
//! the renewal that date creates — keeps the later change in place instead of
//! re-dating the record before it. Two reads of an unchanged record report the
//! same instant, which is what makes the value usable as the key
//! `SearchAgreements` sorts by and as the instant its `AfterLastUpdateTime` and
//! `BeforeLastUpdateTime` filters select on.
//!
//! The rows differ along what the rule generalises over: which door changes the
//! record (the purchase-order association an approved payment request makes, and
//! the amendment an accepted `AMEND` request applies), which contract length the
//! acceptor purchased, how far past its own end date the agreement is, what its
//! terms do at that date (expire, or renew into a new agreement), and whether
//! the transition was already due when the call arrived or became due BY it —
//! with the un-aged and still-running rows as the control, where the clock owes
//! the record nothing.
//!
//! The clock cannot be wound forward 30 days inside a test, so the agreement's
//! own stored window is wound BACK instead: the harness that does it
//! (`signed_ago`, `days`) is the one `agreement_expiry` owns, the wire harness
//! (`emulator_over_engine`, `call`, `create`, `accept`, `search`,
//! `agreement_over`, `upfront_term`, `renewal_term`, `variable_payment_term`,
//! `agreement_type_filter`) is `agreement_lifecycle`'s, the payment-request send
//! is `payment_requests`', the term a payment request is sent under is
//! `inactive_agreement_refusals`', and the summary lookup (`summary_of`) and the
//! filter-value formatter (`filter_instant`) are `agreement_end_dates`'.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_end_dates::{filter_instant, summary_of};
use crate::agreement_expiry::{days, signed_ago};
use crate::agreement_lifecycle::{
    accept, agreement_over, agreement_type_filter, call, create, emulator_over_engine,
    renewal_term, search, upfront_term, variable_payment_term, Answer,
};

use crate::inactive_agreement_refusals::PAYMENT_TERM;
use crate::payment_requests::sent;

/// `AgreementStatus` values these rows expect, from
/// `smithy_query(query="shape", shape="AgreementStatus")`: "ACTIVE – The terms of
/// the agreement are active", "EXPIRED – The agreement ended on the defined end
/// date" and "RENEWED – The agreement was renewed into a new agreement (for
/// example, an auto-renewal)". Spelled out here rather than imported from the
/// crate, so a handler reporting another value fails these rows instead of
/// agreeing with them.
const ACTIVE: &str = "ACTIVE";
/// See [`ACTIVE`].
const EXPIRED: &str = "EXPIRED";
/// See [`ACTIVE`].
const RENEWED: &str = "RENEWED";

/// How much of a second two instants may differ by and still be called the same:
/// the wire spells an instant in epoch seconds with a fraction, and the bound a
/// row captures from the test's own clock is read on the same scale.
const TOLERANCE: f64 = 0.001;

/// One second, the step a row's filter bound sits away from the instant it
/// brackets.
const SECOND: f64 = 1.0;

/// The name a payment request these rows send carries
/// (`PaymentRequestName @length(5..64)`).
const PAYMENT_NAME: &str = "Variable payment for January";

/// The amount a payment request these rows send charges, well inside what the
/// variable payment term it is sent under allows.
const PAYMENT_AMOUNT: &str = "12.50";

/// Now, in epoch seconds, as the wire spells an instant — the bound a row
/// captures just before the call that changes the record.
fn now_epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is past the epoch")
        .as_secs_f64()
}

/// The instant `agreement` reports as its last update, and the status it reports
/// beside it.
async fn reported(router: &MultiServiceRouter, agreement: &str) -> (f64, String) {
    let summary = summary_of(router, agreement).await;
    let instant = summary["lastUpdateTime"].as_f64().unwrap_or_else(|| {
        panic!("{agreement} must report a lastUpdateTime, got {summary:?}");
    });
    let status = summary["status"].as_str().unwrap_or_default().to_string();
    (instant, status)
}

/// The agreement ids one `…LastUpdateTime`-bounded search selects.
async fn bounded_by(router: &MultiServiceRouter, filter: &str, instant: f64) -> Vec<String> {
    let answer = search(
        router,
        json!({"filters": [
            {"name": filter, "values": [filter_instant(instant)]},
            agreement_type_filter(),
        ]}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a search bounded by {filter} must be served, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["agreementId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Assert, for the agreement a row changed at `changed_at`, everything the rule
/// implies about the instant it reports: it is no earlier than the change, it is
/// the same on a second read, the status the clock owed it is reported beside it,
/// and the two `lastUpdateTime` filters select on the value that was reported.
async fn assert_last_update_not_rewound(
    case: &str,
    router: &MultiServiceRouter,
    agreement: &str,
    changed_at: f64,
    status: &str,
) {
    let (first, reported_status) = reported(router, agreement).await;
    assert!(
        first + TOLERANCE >= changed_at,
        "case {case:?}: the call changed the agreement at {changed_at}, so its lastUpdateTime \
         cannot be earlier, got {first}"
    );
    assert_eq!(
        reported_status, status,
        "case {case:?}: expected the status the clock owed the agreement {status:?}, got \
         {reported_status:?}"
    );

    // A read is not a change: the instant is the same however often it is read.
    for attempt in 1..=2 {
        let (again, _) = reported(router, agreement).await;
        assert!(
            (again - first).abs() < TOLERANCE,
            "case {case:?}: read {attempt} must report the same lastUpdateTime {first}, got \
             {again}"
        );
    }

    // What consumes the value: a window whose lower bound sits before the change
    // selects the agreement, and one whose upper bound sits there does not.
    let after = bounded_by(router, "AfterLastUpdateTime", changed_at - SECOND).await;
    assert!(
        after.contains(&agreement.to_string()),
        "case {case:?}: the newest change to {agreement} happened after the bound, so \
         AfterLastUpdateTime must select it, got {after:?}"
    );
    let before = bounded_by(router, "BeforeLastUpdateTime", changed_at - SECOND).await;
    assert!(
        !before.contains(&agreement.to_string()),
        "case {case:?}: {agreement} was last changed after the bound, so BeforeLastUpdateTime \
         must not select it, got {before:?}"
    );
}

/// One row driving the purchase-order association an approved payment request
/// makes, against an agreement whose own end date the clock has already reached
/// and nobody has materialised.
struct ApprovalRow {
    case: &'static str,
    /// The contract length the acceptor purchased
    /// (`ConfigurableUpfrontPricingTermConfiguration$selectorValue`).
    length: &'static str,
    /// How far back the agreement's own stored window is wound before the
    /// approval arrives, in days.
    aged_days: u64,
    /// Whether the acceptor enabled auto-renewal, which is what decides whether
    /// the end date expires the agreement or renews it into a new one.
    auto_renew: bool,
    /// The `AgreementStatus` the clock owes it by the time the approval arrives.
    status: &'static str,
}

/// SOURCE: the contract lengths the emulator's rate card publishes, the table
/// `agreement_expiry` ages the same records over.
fn approval_rows() -> Vec<ApprovalRow> {
    vec![
        ApprovalRow {
            case: "a one-month purchase a day past its end date",
            length: "P1M",
            aged_days: 31,
            auto_renew: false,
            status: EXPIRED,
        },
        ApprovalRow {
            case: "a one-year purchase five weeks past its end date",
            length: "P12M",
            aged_days: 400,
            auto_renew: false,
            status: EXPIRED,
        },
        ApprovalRow {
            case: "a one-month purchase past an end date its terms renew it at",
            length: "P1M",
            aged_days: 45,
            auto_renew: true,
            status: RENEWED,
        },
        ApprovalRow {
            case: "a three-year purchase still well inside its window",
            length: "P36M",
            aged_days: 400,
            auto_renew: false,
            status: ACTIVE,
        },
        ApprovalRow {
            case: "a purchase whose window has not been aged at all",
            length: "P1M",
            aged_days: 0,
            auto_renew: false,
            status: ACTIVE,
        },
    ]
}

/// Approve one payment request, recording the buyer's purchase order reference
/// against the charge the approval creates.
async fn approve(
    router: &MultiServiceRouter,
    agreement: &str,
    payment_request: &str,
    reference: &str,
) -> Answer {
    call(
        router,
        "AcceptAgreementPaymentRequest",
        json!({
            "agreementId": agreement,
            "paymentRequestId": payment_request,
            "purchaseOrderReference": reference,
        }),
    )
    .await
}

/// The purchase order references the charges of `agreement` carry, in the order
/// they are reported — what proves the association the approval made is really
/// held, so the instant reported for the record is the instant of a change it
/// carries.
async fn charge_references(router: &MultiServiceRouter, agreement: &str) -> Vec<String> {
    let listed = call(
        router,
        "ListAgreementCharges",
        json!({"agreementId": agreement}),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "listing the charges of {agreement} must succeed, got {} with {:?}",
        listed.status, listed.body
    );
    listed.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|charge| charge["purchaseOrderReference"].as_str().map(str::to_string))
        .collect()
}

/// A write made through a door that first materialises what the clock owed the
/// agreement keeps its own instant: the association an approved payment request
/// records is the newest change the record reports, whatever the end date the
/// same call settled.
#[tokio::test]
async fn an_association_on_an_unsettled_agreement_is_the_newest_change_it_reports() {
    for (index, row) in approval_rows().into_iter().enumerate() {
        let case = row.case;
        let (router, engine) = emulator_over_engine();

        let mut terms = vec![
            upfront_term("cfg", row.length, &[("Users", 2)]),
            variable_payment_term(PAYMENT_TERM),
        ];
        if row.auto_renew {
            terms.push(renewal_term("renewal", true));
        }
        let agreement = agreement_over(&router, &format!("ap-monotonic{index}"), terms).await;

        // The payment request is sent while the agreement is inside its window,
        // which is the only time a send is served; the window is wound back
        // afterwards, so the approval is the first call to see what the clock
        // owes this record.
        let payment_request = sent(
            &router,
            &agreement,
            PAYMENT_TERM,
            PAYMENT_NAME,
            PAYMENT_AMOUNT,
        )
        .await;
        signed_ago(&engine, &agreement, days(row.aged_days));

        let reference = format!("PO-monotonic{index}");
        let changed_at = now_epoch_secs();
        let answer = approve(&router, &agreement, &payment_request, &reference).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: approving the payment request must succeed, got {} with {:?}",
            answer.status, answer.body
        );
        let references = charge_references(&router, &agreement).await;
        assert!(
            references.contains(&reference),
            "case {case:?}: the approval associates {reference} with the charge it created, got \
             {references:?}"
        );

        assert_last_update_not_rewound(case, &router, &agreement, changed_at, row.status).await;
    }
}

/// One row amending an agreement whose own start date is far behind, so the
/// window the amendment commits it for may already be over.
struct AmendmentRow {
    case: &'static str,
    /// The contract length the agreement was accepted over.
    purchased: &'static str,
    /// How far back its own stored window is wound before it is amended, in
    /// days.
    aged_days: u64,
    /// The contract length the amendment commits it for, measured from the
    /// start date the amendment does not move.
    amended_to: &'static str,
    /// Whether the window the amendment commits is already over.
    ended: bool,
    /// The `AgreementStatus` the clock owes it once the amendment is applied.
    status: &'static str,
}

/// SOURCE: the contract lengths the emulator's rate card publishes, as
/// [`approval_rows`] names them. Each row amends a long purchase that has been
/// running 400 days, so the window the amendment commits ends before, around or
/// after that.
fn amendment_rows() -> Vec<AmendmentRow> {
    vec![
        AmendmentRow {
            case: "an amendment to a month, on an agreement running for 400 days",
            purchased: "P36M",
            aged_days: 400,
            amended_to: "P1M",
            ended: true,
            status: EXPIRED,
        },
        AmendmentRow {
            case: "an amendment to a year, on an agreement running for 400 days",
            purchased: "P36M",
            aged_days: 400,
            amended_to: "P12M",
            ended: true,
            status: EXPIRED,
        },
        AmendmentRow {
            case: "an amendment to two years, on an agreement running for 400 days",
            purchased: "P36M",
            aged_days: 400,
            amended_to: "P24M",
            ended: false,
            status: ACTIVE,
        },
        AmendmentRow {
            case: "an amendment to a month, on an agreement accepted just now",
            purchased: "P12M",
            aged_days: 0,
            amended_to: "P1M",
            ended: false,
            status: ACTIVE,
        },
    ]
}

/// Accepting an amendment answers the agreement it modified.
async fn amend(
    router: &MultiServiceRouter,
    case: &str,
    source: &str,
    terms: Vec<Value>,
) -> Answer {
    let quoted = create(
        router,
        json!({
            "intent": "AMEND",
            "sourceAgreementIdentifier": source,
            "requestedTerms": terms,
        }),
    )
    .await;
    assert_eq!(
        quoted.status, 200,
        "case {case:?}: quoting the amendment must succeed, got {} with {:?}",
        quoted.status, quoted.body
    );
    let accepted = accept(
        router,
        json!({"agreementRequestId": quoted.str_at("/agreementRequestId")}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: accepting the amendment must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.str_at("/agreementId"),
        source,
        "case {case:?}: an amendment modifies the agreement it names, got {:?}",
        accepted.body
    );
    accepted
}

/// The same rule where the amendment itself is what makes the transition due:
/// an amendment that commits the agreement for a window its own start date has
/// already ended still reports the amendment as the newest change, and the
/// expiry that window owes is materialised beside it rather than instead of it.
#[tokio::test]
async fn an_amendment_into_a_window_already_over_is_still_the_newest_change() {
    for (index, row) in amendment_rows().into_iter().enumerate() {
        let case = row.case;
        let (router, engine) = emulator_over_engine();
        let agreement = agreement_over(
            &router,
            &format!("ap-amendmonotonic{index}"),
            vec![upfront_term("cfg", row.purchased, &[("Users", 4)])],
        )
        .await;
        signed_ago(&engine, &agreement, days(row.aged_days));

        let changed_at = now_epoch_secs();
        amend(
            &router,
            case,
            &agreement,
            vec![upfront_term("cfg", row.amended_to, &[("Users", 4)])],
        )
        .await;

        // The window the amendment committed, which is what makes the row's
        // transition due or leaves nothing owed.
        let summary = summary_of(&router, &agreement).await;
        let end_time = summary["endTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: the amended agreement has an end date, got {summary:?}");
        });
        assert_eq!(
            end_time < changed_at,
            row.ended,
            "case {case:?}: expected the committed window to be over: {}, got an end date of \
             {end_time} against {changed_at}",
            row.ended
        );

        assert_last_update_not_rewound(case, &router, &agreement, changed_at, row.status).await;
    }
}

/// The complement, so the rule is about a change the record carries rather than
/// about reads in general: an agreement no call has changed since the clock
/// expired it reports the END DATE the expiry was due at — not the instant
/// somebody looked — however often it is read.
#[tokio::test]
async fn an_expiry_no_later_change_follows_is_still_dated_at_the_end_date() {
    let (router, engine) = emulator_over_engine();
    let agreement = agreement_over(
        &router,
        "ap-monotonicexpiry",
        vec![upfront_term("cfg", "P1M", &[("Users", 2)])],
    )
    .await;
    signed_ago(&engine, &agreement, days(45));

    let looked_at = now_epoch_secs();
    let summary = summary_of(&router, &agreement).await;
    let end_time = summary["endTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("the purchase has an end date, got {summary:?}"));
    let (instant, status) = reported(&router, &agreement).await;
    assert_eq!(
        status, EXPIRED,
        "the clock owed the agreement its expiry, got {status:?}"
    );
    assert!(
        (instant - end_time).abs() < TOLERANCE,
        "the expiry is dated at the end date {end_time} it was due at, got {instant}"
    );
    assert!(
        instant < looked_at,
        "nothing changed the record after its end date, so the instant it reports stays before \
         the read at {looked_at}, got {instant}"
    );
}
