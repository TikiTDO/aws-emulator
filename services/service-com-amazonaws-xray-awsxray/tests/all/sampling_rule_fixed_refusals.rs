// SPDX-License-Identifier: CC0-1.0

//! The two sampling-rule refusals X-Ray words as FIXED sentences, end to end
//! over the wire (the `Xray` harness in [`super::xray_wire`]).
//!
//! One rule per suite section, each a sentence into which nothing of the request
//! reaches:
//!
//! * a `DeleteSamplingRule` that selects the account's service-managed rule is
//!   refused because of WHAT that rule is, so the sentence states that condition
//!   alone — the same text whichever of the two documented selectors named it,
//!   in whatever region, and with whatever else the account holds; and
//! * an `UpdateSamplingRule` carrying a value outside a bound the modelled
//!   `SamplingRule` declares is refused as an unusable UPDATE, so the sentence
//!   names neither the member, nor the value, nor the bound: `SamplingRuleUpdate`
//!   declares no bound of its own for the boundary to report
//!   (`smithy_query(query="shape",shape="SamplingRuleUpdate")` — `Priority` and
//!   `ReservoirSize` target `NullableInteger`, `FixedRate` `NullableDouble`), so
//!   the refusal is the service's own.
//!
//! Both are refusals that must also WRITE NOTHING, which every row checks by
//! reading the rule back afterwards.

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::state::{
    DEFAULT_SAMPLING_RULE_NAME, DEFAULT_SAMPLING_RULE_UNDELETABLE_MESSAGE,
    INVALID_SAMPLING_RULE_UPDATE_MESSAGE,
};

use super::xray_wire::{Answer, Xray, DEFAULT_REGION};

/// The refusal message of an answer, or the empty string.
fn said(answer: &Answer) -> &str {
    answer.message().unwrap_or_default()
}

/// A sampling rule body `CreateSamplingRule` accepts, at `priority`.
fn rule_body(name: &str, priority: i64) -> Value {
    json!({
        "SamplingRule": {
            "RuleName": name,
            "ResourceARN": "*",
            "Priority": priority,
            "FixedRate": 0.05,
            "ReservoirSize": 1,
            "ServiceName": "*",
            "ServiceType": "*",
            "Host": "*",
            "HTTPMethod": "*",
            "URLPath": "*",
            // The model documents the sampling rule format as `1`.
            "Version": 1,
        }
    })
}

/// The ARN a rule of `name` is read with in `region`, which is the other
/// selector every rule-selecting operation takes.
fn rule_arn(region: &str, name: &str) -> String {
    service_com_amazonaws_xray_awsxray::state::sampling_rule_arn(region, name)
}

/// The record `GetSamplingRules` reports for `name` in `region`.
async fn stored_rule(xray: &Xray, region: &str, case: &str, name: &str) -> Value {
    let listed = xray.call_in_region(region, "/GetSamplingRules", &json!({})).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: listing the rules must succeed, got {listed:?}"
    );
    listed
        .get("SamplingRuleRecords")
        .as_array()
        .and_then(|records| {
            records
                .iter()
                .find(|record| {
                    record
                        .get("SamplingRule")
                        .and_then(|rule| rule.get("RuleName"))
                        .and_then(Value::as_str)
                        == Some(name)
                })
                .cloned()
        })
        .unwrap_or_else(|| panic!("case {case:?}: the rule {name:?} must be listed, got {listed:?}"))
}

// =============================================================================
// 1. Deleting the service-managed rule states one fixed sentence
// =============================================================================

#[tokio::test]
async fn deleting_the_service_managed_rule_is_refused_with_the_fixed_sentence() {
    // The sentence interpolates nothing, so it cannot vary from call to call —
    // not even with the reserved name it is about.
    assert_eq!(
        DEFAULT_SAMPLING_RULE_UNDELETABLE_MESSAGE, "Cannot delete default sampling rule",
        "the fixed text must be the service's own, got \
         {DEFAULT_SAMPLING_RULE_UNDELETABLE_MESSAGE:?}"
    );

    // Rows differ along everything the request and the account can vary that the
    // message does not carry: which of the two documented selectors names the
    // rule ("either name or ARN, but not both"), the region the call is signed
    // for, and whether the account holds rules of its own beside the
    // service-managed one.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    for region in regions {
        for seeded in [false, true] {
            let xray = Xray::new();
            if seeded {
                let created = xray
                    .call_in_region(region, "/CreateSamplingRule", &rule_body("own-rule", 100))
                    .await;
                assert_eq!(
                    created.status, 200,
                    "region {region:?}: the fixture rule must be created, got {created:?}"
                );
            }

            let selectors: [(&str, Value); 2] = [
                (
                    "by name",
                    json!({ "RuleName": DEFAULT_SAMPLING_RULE_NAME }),
                ),
                (
                    "by ARN",
                    json!({ "RuleARN": rule_arn(region, DEFAULT_SAMPLING_RULE_NAME) }),
                ),
            ];
            for (selector, body) in selectors {
                let case = format!("{selector} in {region}, seeded {seeded}");
                let refused = xray
                    .call_in_region(region, "/DeleteSamplingRule", &body)
                    .await;
                assert_eq!(
                    (refused.status, refused.error_type.as_deref()),
                    (400, Some("InvalidRequestException")),
                    "case {case:?}: expected 400 InvalidRequestException, got {refused:?}"
                );
                assert_eq!(
                    said(&refused),
                    DEFAULT_SAMPLING_RULE_UNDELETABLE_MESSAGE,
                    "case {case:?}: expected {DEFAULT_SAMPLING_RULE_UNDELETABLE_MESSAGE:?}, got \
                     {:?}",
                    said(&refused)
                );
                assert_eq!(
                    refused.get("SamplingRuleRecord"),
                    &Value::Null,
                    "case {case:?}: a refused delete answers no record, got {refused:?}"
                );

                // And the rule is still there to be read: the refusal removed
                // nothing.
                let still = stored_rule(&xray, region, &case, DEFAULT_SAMPLING_RULE_NAME).await;
                assert_eq!(
                    still
                        .get("SamplingRule")
                        .and_then(|rule| rule.get("RuleARN"))
                        .and_then(Value::as_str),
                    Some(rule_arn(region, DEFAULT_SAMPLING_RULE_NAME).as_str()),
                    "case {case:?}: the service-managed rule is still listed, got {still:?}"
                );
            }
        }
    }
}

// =============================================================================
// 2. An update the service cannot apply states one fixed sentence
// =============================================================================

#[tokio::test]
async fn an_update_outside_a_modelled_bound_is_refused_with_the_fixed_sentence() {
    // The sentence interpolates nothing: neither the member, nor the value, nor
    // the bound.
    assert_eq!(
        INVALID_SAMPLING_RULE_UPDATE_MESSAGE, "Invalid update request for sampling rule",
        "the fixed text must be the service's own, got {INVALID_SAMPLING_RULE_UPDATE_MESSAGE:?}"
    );

    let region = DEFAULT_REGION;
    let xray = Xray::new();
    let created = xray
        .call_in_region(region, "/CreateSamplingRule", &rule_body("update-probe", 500))
        .await;
    assert_eq!(
        created.status, 200,
        "the fixture rule must be created, got {created:?}"
    );
    let baseline = stored_rule(&xray, region, "baseline", "update-probe").await;

    // The rows differ along which member carries the value, which side of its
    // bound the value falls, how far past it reaches, and the TYPE of the value
    // (integer, double): the bounds are the modelled `SamplingRule`'s —
    // `Priority` @range 1..9999, `FixedRate` @range 0..1, `ReservoirSize`
    // @range 0.. (`smithy_query(query="shape",shape="Priority,FixedRate,
    // ReservoirSize")`) — which `SamplingRuleUpdate` does not re-declare, so
    // every one of them is refused by the service with the same sentence.
    let rows: Vec<(&str, Value)> = vec![
        ("a negative Priority", json!({ "Priority": -5 })),
        ("Priority one below its minimum", json!({ "Priority": 0 })),
        ("Priority one above its maximum", json!({ "Priority": 10_000 })),
        ("Priority far above its maximum", json!({ "Priority": 1_000_000 })),
        ("FixedRate below its minimum", json!({ "FixedRate": -0.000_1 })),
        ("FixedRate above its maximum", json!({ "FixedRate": 1.000_1 })),
        ("a negative ReservoirSize", json!({ "ReservoirSize": -1 })),
        (
            "two members at once, the first of them out of range",
            json!({ "Priority": -5, "FixedRate": 0.5 }),
        ),
        (
            "two members at once, the second of them out of range",
            json!({ "Priority": 500, "ReservoirSize": -7 }),
        ),
    ];

    for (case, update) in rows {
        let mut body = update.clone();
        body["RuleName"] = json!("update-probe");
        let refused = xray
            .call_in_region(
                region,
                "/UpdateSamplingRule",
                &json!({ "SamplingRuleUpdate": body }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {case:?}: expected 400 InvalidRequestException, got {refused:?}"
        );
        assert_eq!(
            said(&refused),
            INVALID_SAMPLING_RULE_UPDATE_MESSAGE,
            "case {case:?}: expected {INVALID_SAMPLING_RULE_UPDATE_MESSAGE:?}, got {:?}",
            said(&refused)
        );

        // Nothing was written: the stored rule is the one the create left.
        let after = stored_rule(&xray, region, case, "update-probe").await;
        assert_eq!(
            after, baseline,
            "case {case:?}: a refused update must write nothing, got {after:?}"
        );
    }

    // And a value INSIDE every bound is still applied, so the rows above refuse
    // the values and not the operation.
    let applied = xray
        .call_in_region(
            region,
            "/UpdateSamplingRule",
            &json!({
                "SamplingRuleUpdate": {
                    "RuleName": "update-probe",
                    "Priority": 501,
                    "FixedRate": 1.0,
                    "ReservoirSize": 0,
                }
            }),
        )
        .await;
    assert_eq!(
        applied.status, 200,
        "an update inside every bound must be applied, got {applied:?}"
    );
    let after = stored_rule(&xray, region, "applied", "update-probe").await;
    let member = |record: &Value, name: &str| {
        record
            .get("SamplingRule")
            .and_then(|rule| rule.get(name))
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(
        (
            member(&after, "Priority"),
            member(&after, "FixedRate"),
            member(&after, "ReservoirSize")
        ),
        (json!(501), json!(1.0), json!(0)),
        "the accepted update must be stored, got {after:?}"
    );
}
