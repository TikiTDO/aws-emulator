// SPDX-License-Identifier: CC0-1.0

//! The account-class gate on the bill-planning surface, driven end-to-end over this
//! service's awsJson1_0 wire through the same router the production front door uses.
//!
//! The RULE every suite here asserts: the bill-planning surface — the bill scenarios and
//! everything inside them, and the WRITE of the account-level rate-type preferences —
//! belongs to the account that pays the bills being modelled, so a request for one of
//! those subjects from an AWS Organizations MEMBER account is refused with HTTP 400
//! `ValidationException` carrying `reason` `INVALID_REQUEST_FROM_MEMBER` and ONE fixed,
//! operation-independent message, whatever the request carries and whether it reads or
//! writes, and changes nothing; the same request from an account that pays its own bills
//! (a payer/management account, or a standalone account in no organization) is admitted.
//! Reading the preferences, and the whole workload-estimate surface, are not gated for any
//! class.
//!
//! The rows span what the rule generalises over: the operation (every implemented one of
//! the gated subjects, read and write alike), the account class (all three values of
//! `state::AccountClass`), and — for the subjects that are not gated — the operation that
//! must still answer.

use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::{
    state::{self, AccountClass},
    ComAmazonawsBcmpricingcalculatorAwsbcmpricingcalculatorState as ServiceState,
};

use crate::bill_scenario_lifecycle::{error_code, Emulator};

/// A well-formed `ResourceId` (the lowercase-hex v4 UUID form the shape pins) that no
/// live resource carries: the gate is answered before the request is resolved, so a
/// gated read needs no resource to exist.
const ABSENT_ID: &str = "0f9e8d7c-6b5a-4938-2716-05f4e3d2c1b0";

/// A 12-digit `AccountId`, the form `^\d{12}$` requires.
const USAGE_ACCOUNT_ID: &str = "000000000000";

/// The `ValidationExceptionReason` value an account-class refusal reports, in the
/// spelling the GATE puts on the wire: the SCREAMING_SNAKE_CASE rendering of the model's
/// own `invalidRequestFromMember` (smithy_query: `shape="ValidationExceptionReason"` →
/// `[unknownOperation, cannotParse, fieldValidationFailed, invalidRequestFromMember,
/// disallowedRate, other]`). RECORDED behaviour, identical in every recorded refusal the
/// gate makes; a refusal raised by the modelled input validator carries the model's own
/// spelling instead (see `tests/all/validation_refusal_detail.rs`).
const INVALID_REQUEST_FROM_MEMBER: &str = "INVALID_REQUEST_FROM_MEMBER";

/// The one sentence the gate refuses with, whatever the operation. RECORDED behaviour:
/// byte-identical in every recorded refusal the gate makes, of seven different
/// operations, and naming none of them.
const REFUSAL_MESSAGE: &str = "Operation not permitted for member accounts. This API is \
     only allowed for regular or payer accounts.";

/// Every account class a store can present (smithy_query corroborates the three-way
/// split: `GetPreferencesResponse` reports one selection per class).
const ACCOUNT_CLASSES: [AccountClass; 3] = [
    AccountClass::Member,
    AccountClass::Management,
    AccountClass::Standalone,
];

/// One row of the gate table: an operation of a gated subject, a well-formed request for
/// it, and the status an account that MAY plan bills gets for that same request — the
/// operation's own answer, which is what the gate must not replace.
///
/// `pub(crate)` with [`gated_calls`]: `tests/all/account_class_refusal_announcer.rs` drives
/// the same table to pin what the gate's refusal is ANNOUNCED under, so the two suites
/// cannot disagree about which calls the gate refuses.
pub(crate) struct GatedCall {
    pub(crate) operation: &'static str,
    pub(crate) request: Value,
    pub(crate) admitted_status: u16,
}

/// Every IMPLEMENTED operation of the gated subjects: the bill scenario itself, the usage
/// modifications that only exist inside one, and the preferences write. Each request is
/// well formed, so nothing but the gate can refuse it; the ones addressing a resource
/// name [`ABSENT_ID`], whose own answer (404, or a success for the idempotent delete) is
/// what an admitted caller gets.
///
/// The remaining operations of these subjects are still scaffold stubs answering HTTP 501
/// `NotImplemented`, so they have no behaviour to gate yet.
pub(crate) fn gated_calls() -> Vec<GatedCall> {
    vec![
        GatedCall {
            operation: "CreateBillScenario",
            request: json!({ "name": "gate-create" }),
            admitted_status: 200,
        },
        GatedCall {
            operation: "ListBillScenarios",
            request: json!({ "maxResults": 25 }),
            admitted_status: 200,
        },
        GatedCall {
            operation: "GetBillScenario",
            request: json!({ "identifier": ABSENT_ID }),
            admitted_status: 404,
        },
        GatedCall {
            operation: "DeleteBillScenario",
            request: json!({ "identifier": ABSENT_ID }),
            admitted_status: 200,
        },
        GatedCall {
            operation: "ListBillScenarioUsageModifications",
            request: json!({ "billScenarioId": ABSENT_ID }),
            admitted_status: 404,
        },
        GatedCall {
            operation: "BatchCreateBillScenarioUsageModification",
            request: json!({
                "billScenarioId": ABSENT_ID,
                "usageModifications": [{
                    "key": "k0",
                    "serviceCode": "AmazonEC2",
                    "usageType": "BoxUsage:m5.xlarge",
                    "operation": "RunInstances",
                    "usageAccountId": USAGE_ACCOUNT_ID,
                }],
            }),
            admitted_status: 404,
        },
        GatedCall {
            operation: "UpdatePreferences",
            request: json!({ "memberAccountRateTypeSelections": ["AFTER_DISCOUNTS"] }),
            admitted_status: 200,
        },
    ]
}

/// Assert `body` is the account-class refusal: the modelled code, the `reason` in the
/// spelling the gate puts on the wire, and the ONE fixed message — which names no
/// operation, so it is byte-identical for every row of the table.
pub(crate) fn assert_member_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case}: expected ValidationException, got {body}"
    );
    assert_eq!(
        body["reason"].as_str(),
        Some(INVALID_REQUEST_FROM_MEMBER),
        "case {case}: expected reason {INVALID_REQUEST_FROM_MEMBER:?}, got {:?} in {body}",
        body["reason"]
    );
    assert_eq!(
        body["message"].as_str(),
        Some(REFUSAL_MESSAGE),
        "case {case}: expected the fixed message {REFUSAL_MESSAGE:?}, got {:?}",
        body["message"]
    );
    // No field of the request is at fault, so the structured half a field-validation
    // failure carries is absent.
    assert!(
        body.get("fieldList").is_none(),
        "case {case}: an account-class refusal names no field, got {body}"
    );
}

#[tokio::test]
async fn a_member_account_is_refused_the_whole_bill_planning_surface() {
    for GatedCall {
        operation,
        request,
        admitted_status: _,
    } in gated_calls()
    {
        // A fresh store per row, so no row depends on what another left behind.
        let emulator = Emulator::with_account_class(AccountClass::Member);
        let case = format!("{operation} from a member account");
        let (status, body) = emulator.call(operation, request).await;
        assert_member_refusal(&case, status, &body);
        // The message is the gate's, not the operation's: it does not name the operation
        // it refused, so a client matching the sentence matches it for every one.
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains(operation),
            "case {case}: the message names the operation: {message:?}"
        );
    }
}

#[tokio::test]
async fn an_account_that_pays_its_own_bills_is_admitted() {
    for class in [AccountClass::Management, AccountClass::Standalone] {
        for GatedCall {
            operation,
            request,
            admitted_status,
        } in gated_calls()
        {
            let emulator = Emulator::with_account_class(class);
            let case = format!("{operation} from {class:?}");
            let (status, body) = emulator.call(operation, request).await;
            assert_eq!(
                status, admitted_status,
                "case {case}: expected the operation's own answer {admitted_status}, got \
                 {status} with {body}"
            );
            assert_ne!(
                body["reason"].as_str(),
                Some(INVALID_REQUEST_FROM_MEMBER),
                "case {case}: expected no account-class refusal, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_request_changes_nothing() {
    let emulator = Emulator::with_account_class(AccountClass::Member);
    let effective = vec![state::DEFAULT_RATE_TYPE_SELECTION.to_string()];

    for GatedCall {
        operation,
        request,
        admitted_status: _,
    } in gated_calls()
    {
        let case = format!("{operation} refused");
        let (status, body) = emulator.call(operation, request).await;
        assert_member_refusal(&case, status, &body);

        // Nothing the refused request asked for reached the store. The bill-scenario
        // collection is read through the store rather than `ListBillScenarios`, which
        // the same gate refuses.
        let store = emulator.engine.state::<ServiceState>();
        let state = store.lock();
        assert!(
            state.bill_scenarios.is_empty(),
            "case {case}: expected no bill scenario to have been stored, got {} of them",
            state.bill_scenarios.len()
        );
        for (member, stored) in [
            (
                "managementAccountRateTypeSelections",
                &state.preferences.management_account_rate_type_selections,
            ),
            (
                "memberAccountRateTypeSelections",
                &state.preferences.member_account_rate_type_selections,
            ),
            (
                "standaloneAccountRateTypeSelections",
                &state.preferences.standalone_account_rate_type_selections,
            ),
        ] {
            assert_eq!(
                stored, &effective,
                "case {case} member {member}: expected the stored selection untouched at \
                 {effective:?}, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn reading_the_preferences_is_permitted_to_every_account_class() {
    // Reading the rate types one's own bills are quoted with is not gated: every class
    // gets all three selections, and on a store nobody wrote they are the effective
    // ones. That is what makes `GetPreferences` usable as the read half of the
    // read-modify-write a member account's write is refused in the middle of.
    let effective = json!([state::DEFAULT_RATE_TYPE_SELECTION]);
    for class in ACCOUNT_CLASSES {
        let emulator = Emulator::with_account_class(class);
        let (status, body) = emulator.call("GetPreferences", json!({})).await;
        assert_eq!(
            status, 200,
            "case {class:?}: expected GetPreferences 200, got {status} with {body}"
        );
        for member in [
            "managementAccountRateTypeSelections",
            "memberAccountRateTypeSelections",
            "standaloneAccountRateTypeSelections",
        ] {
            assert_eq!(
                body.get(member),
                Some(&effective),
                "case {class:?} member {member}: expected {effective}, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_member_account_keeps_the_surfaces_the_gate_does_not_cover() {
    // The boundary of the rule: a workload estimate models one account's own future
    // usage rather than a bill somebody else pays, and tagging addresses a resource that
    // already exists, so neither subject is gated — a member account uses both, and the
    // round trip through the reader proves the create was stored.
    let emulator = Emulator::with_account_class(AccountClass::Member);

    let (status, created) = emulator
        .call("CreateWorkloadEstimate", json!({ "name": "gate-estimate" }))
        .await;
    assert_eq!(
        status, 200,
        "CreateWorkloadEstimate from a member account: expected 200, got {status} with {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateWorkloadEstimate: no id in {created}"))
        .to_string();

    let (status, fetched) = emulator
        .call("GetWorkloadEstimate", json!({ "identifier": id }))
        .await;
    assert_eq!(
        status, 200,
        "GetWorkloadEstimate from a member account: expected 200, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["id"].as_str(),
        Some(id.as_str()),
        "GetWorkloadEstimate: expected the created estimate {id}, got {fetched}"
    );

    let (status, listed) = emulator
        .call("ListWorkloadEstimates", json!({ "maxResults": 25 }))
        .await;
    assert_eq!(
        status, 200,
        "ListWorkloadEstimates from a member account: expected 200, got {status} with {listed}"
    );
}
