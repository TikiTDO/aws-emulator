// SPDX-License-Identifier: CC0-1.0

//! WHICH announcer answers a refusal of a dispatcher-fronted operation, driven end-to-end
//! over this service's awsJson1_0 wire through the same router the production front door
//! uses.
//!
//! The RULE under test: which of this service's two announcers owns a response is decided by
//! the GROUND the response states and not by the error shape it is worded in — a refusal
//! reporting the REQUEST ITSELF as invalid is the request validator's, in front of the
//! dispatcher, and keeps the `application/x-amz-json-1.0` the model's
//! `aws.protocols#awsJson1_0` trait implies, while a refusal of a WELL-FORMED request on the
//! CALLER's account class is the dispatcher's own front door, like its successes and its
//! errors about stored state, and announces `application/x-amz-json-1.1`. Both grounds are
//! answered in the one modelled `ValidationException` (smithy_query: `shape="ValidationException"`
//! → `message`, `reason`, `fieldList`; `shape="ValidationExceptionReason"` →
//! `[unknownOperation, cannotParse, fieldValidationFailed, invalidRequestFromMember,
//! disallowedRate, other]`), so the shape name tells the two apart nowhere and the `reason`
//! each states is what does.
//!
//! The rows span what the rule generalises over: the GROUND (an account-class refusal, and
//! an input-constraint refusal of the same operation from the same caller), the OPERATION
//! (every one the gate refuses, read and write, single and batch, plus the preferences
//! write), the CONSTRAINT KIND the validator's rows violate (a `@pattern`, an absent
//! `@required` member, a `@range` maximum, a list `@length` minimum), the ACCOUNT CLASS (a
//! refused member account and the two classes that are admitted) and the OPERATION FAMILY
//! (the resource-tagging family, which has no dispatcher in front of it at all). No status,
//! code, message or body byte is expected to change with the announcer, which the gate rows
//! assert alongside the header.
//!
//! The gated calls are `tests/all/account_class_gate.rs`'s own table and the two media types
//! and the request framing are `tests/all/response_media_type.rs`'s, so this suite cannot
//! disagree with either about which calls the gate refuses or what each announcer answers
//! under.

use serde_json::{json, Value};
use service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::state::{
    self, AccountClass,
};

use crate::account_class_gate::{assert_member_refusal, gated_calls, GatedCall};
use crate::bill_scenario_lifecycle::Emulator;
use crate::response_media_type::{
    assert_media_type, send, DISPATCHER_MEDIA_TYPE, PROTOCOL_MEDIA_TYPE,
};

/// A `ResourceId` outside the lowercase-hex v4 UUID form the shape pins (`@pattern`
/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, smithy_query:
/// `shape="ResourceId"`), but of its length — so the pattern is the one constraint it breaks.
const MALFORMED_ID: &str = "zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz";

/// The `ValidationExceptionReason` a refusal about a MEMBER OF THE REQUEST reports, in the
/// model's own spelling: what this service's input validator states (smithy_query:
/// `shape="ValidationExceptionReason"`).
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The body a response carries, as a client parses it.
fn body(response: &http::Response<bytes::Bytes>) -> Value {
    serde_json::from_slice(response.body()).unwrap_or(Value::Null)
}

/// One request the VALIDATOR refuses: a dispatcher-fronted operation of a GATED subject,
/// called with a member outside the constraint its shape declares, so the refusal reports
/// the request rather than the caller.
///
/// Each row names the constraint it breaks; the constraints are the model's (smithy_query
/// `query="operation"` on each operation below).
struct ValidatorCall {
    case: &'static str,
    operation: &'static str,
    request: Value,
}

fn validator_calls() -> Vec<ValidatorCall> {
    vec![
        ValidatorCall {
            case: "a @pattern on an identifier, on a gated read",
            operation: "GetBillScenario",
            request: json!({ "identifier": MALFORMED_ID }),
        },
        ValidatorCall {
            case: "an absent @required member, on a gated create",
            operation: "CreateBillScenario",
            request: json!({}),
        },
        ValidatorCall {
            case: "a @pattern on a name, on a gated create",
            operation: "CreateBillScenario",
            request: json!({ "name": "not a legal name!" }),
        },
        ValidatorCall {
            case: "a @range maximum on a page size, on a gated listing",
            operation: "ListBillScenarios",
            request: json!({ "maxResults": 26 }),
        },
        ValidatorCall {
            case: "a list @length minimum, on the gated preferences write",
            operation: "UpdatePreferences",
            request: json!({ "memberAccountRateTypeSelections": [] }),
        },
        ValidatorCall {
            case: "two members at fault, on a gated batch write",
            operation: "BatchCreateBillScenarioUsageModification",
            request: json!({ "billScenarioId": MALFORMED_ID, "usageModifications": [] }),
        },
    ]
}

/// The account-class gate's refusal is the dispatcher's: it announces the dispatcher's media
/// type for every operation the gate refuses, and nothing else about the refusal changes.
#[tokio::test]
async fn the_account_class_refusal_announces_the_dispatchers_media_type() {
    for GatedCall {
        operation,
        request,
        admitted_status: _,
    } in gated_calls()
    {
        // A fresh store per row, so no row depends on what another left behind.
        let emulator = Emulator::with_account_class(AccountClass::Member);
        let case = format!("{operation} from a member account");
        let response = send(&emulator, operation, &request).await;
        assert_media_type(&case, &response, 400, DISPATCHER_MEDIA_TYPE);
        // The announcer owns one header: the refusal a client reads is the same one.
        assert_member_refusal(&case, response.status().as_u16(), &body(&response));
    }
}

/// A refusal of the SAME operation by the SAME caller, on the ground that the request itself
/// is invalid, stays the request validator's and keeps the protocol-implied media type: the
/// two announcers are told apart by the ground, not by the caller or the operation.
#[tokio::test]
async fn a_refusal_about_the_request_keeps_the_protocols_media_type() {
    for ValidatorCall {
        case,
        operation,
        request,
    } in validator_calls()
    {
        for class in [
            AccountClass::Member,
            AccountClass::Management,
            AccountClass::Standalone,
        ] {
            let emulator = Emulator::with_account_class(class);
            let case = format!("{case} ({operation}, from {class:?})");
            let response = send(&emulator, operation, &request).await;
            assert_media_type(&case, &response, 400, PROTOCOL_MEDIA_TYPE);
            let refused = body(&response);
            assert_eq!(
                refused["reason"].as_str(),
                Some(REASON_FIELD_VALIDATION_FAILED),
                "case {case}: expected the validator's reason \
                 {REASON_FIELD_VALIDATION_FAILED:?}, got {refused}"
            );
            assert!(
                refused["fieldList"].is_array(),
                "case {case}: the validator's refusal names the members at fault, got {refused}"
            );
        }
    }
}

/// An admitted caller gets the operation's own answer under the dispatcher's media type: the
/// gate's refusal and the answer it replaces are announced by the same front door, so the
/// header does not move with the account class.
#[tokio::test]
async fn an_admitted_caller_keeps_the_dispatchers_media_type() {
    for class in [AccountClass::Management, AccountClass::Standalone] {
        for GatedCall {
            operation,
            request,
            admitted_status,
        } in gated_calls()
        {
            let emulator = Emulator::with_account_class(class);
            let case = format!("{operation} from {class:?}");
            let response = send(&emulator, operation, &request).await;
            assert_media_type(&case, &response, admitted_status, DISPATCHER_MEDIA_TYPE);
        }
    }
}

/// The resource-tagging family keeps the protocol-implied media type on every status for
/// every account class: no dispatcher fronts it, so neither ground reaches that announcer.
#[tokio::test]
async fn the_tagging_family_keeps_the_protocols_media_type_for_every_account_class() {
    for class in [
        AccountClass::Member,
        AccountClass::Management,
        AccountClass::Standalone,
    ] {
        let emulator = Emulator::with_account_class(class);
        // Tagging addresses a resource that already exists, and the workload-estimate
        // surface is not gated for any class, so every class can make one to tag.
        let created = send(
            &emulator,
            "CreateWorkloadEstimate",
            &json!({ "name": "announcer-tagging" }),
        )
        .await;
        assert_media_type(
            &format!("the fixture: a created workload estimate, from {class:?}"),
            &created,
            200,
            DISPATCHER_MEDIA_TYPE,
        );
        let id = body(&created)["id"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateWorkloadEstimate from {class:?}: no id"))
            .to_string();
        let arn = engine::emulator_arn(
            state::ARN_NAMESPACE,
            state::ARN_REGION,
            &format!("{}/{id}", state::WORKLOAD_ESTIMATE_ARN_TYPE),
        );

        let cases: [(&str, &str, Value, u16); 3] = [
            (
                "a tag write",
                "TagResource",
                json!({ "arn": arn.clone(), "tags": { "env": "test" } }),
                200,
            ),
            (
                "a tag listing",
                "ListTagsForResource",
                json!({ "arn": arn.clone() }),
                200,
            ),
            (
                "a tag write refused for a reserved key",
                "TagResource",
                json!({ "arn": arn.clone(), "tags": { "aws:internal": "x" } }),
                400,
            ),
        ];
        for (case, operation, request, status) in cases {
            let case = format!("{case} ({operation}, from {class:?})");
            let response = send(&emulator, operation, &request).await;
            assert_media_type(&case, &response, status, PROTOCOL_MEDIA_TYPE);
        }
    }
}
