// SPDX-License-Identifier: CC0-1.0

//! What a read of a configured-table analysis rule answers when the
//! `(configuredTableIdentifier, analysisRuleType)` pair the request path carries holds no
//! rule.
//!
//! The rule under test: an analysis rule is identified by that pair ("Configured table
//! analysis rules are uniquely identified by their configured table identifier and
//! analysis rule type"), so the read resolves the pair against persisted state and
//! refuses a pair the store holds no rule for with the modelled 404
//! `ResourceNotFoundException` — naming the pair, and carrying the restJson1
//! `x-amzn-errortype` header. The operations that address a rule through the same pair
//! (the update and the delete) refuse it the same way.
//!
//! The table varies what the rule generalises over: a table that holds a rule of ANOTHER
//! type (so the type is part of the key, not decoration), and identifiers in the form
//! `ConfiguredTableIdentifier` requires that no create minted — the two extremes of the
//! space and one in between. Every request goes over the wire through the same
//! `MultiServiceRouter` the production router uses.

use super::cleanrooms_collaboration_lifecycle::{error_code, error_message, Emulator};
use super::cleanrooms_configured_table::COLUMN_NAMES;
use super::configured_table_analysis_rule_round_trip::{
    aggregation_policy, custom_policy, list_policy, no_such_analysis_rule,
};

/// A policy of `rule_type`, so each row below breaks ONE thing — the pair holds no rule
/// — rather than also carrying a policy of another type than the pair's.
pub(crate) fn policy_of(rule_type: &str) -> serde_json::Value {
    match rule_type {
        "LIST" => list_policy(&COLUMN_NAMES[..1], &COLUMN_NAMES[1..], Some(&["AND"]), None),
        "AGGREGATION" => aggregation_policy(&COLUMN_NAMES[1..2], "SUM", None, None, 2),
        "CUSTOM" => custom_policy(None, None),
        other => panic!("no ConfiguredTableAnalysisRulePolicyV1 member for rule type {other:?}"),
    }
}

#[tokio::test]
async fn an_analysis_rule_the_pair_holds_no_rule_for_is_not_found_by_every_reader() {
    let emulator = Emulator::new();

    // A table holding ONE rule, so the pairs below differ from a stored one only in the
    // analysis rule type.
    let table_id = emulator.analysis_rule_table(0).await;
    let (status, created) = emulator
        .create_analysis_rule(
            &table_id,
            "LIST",
            list_policy(&COLUMN_NAMES[..1], &COLUMN_NAMES[1..], Some(&["OR"]), None),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the LIST create to succeed, got {status} with {created}"
    );

    let mut pairs: Vec<(String, &str)> = vec![
        (table_id.clone(), "AGGREGATION"),
        (table_id.clone(), "CUSTOM"),
    ];
    for unheld in [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
        "deadbeef-0000-4000-8000-000000000001",
    ] {
        assert_ne!(
            unheld, table_id,
            "the unheld identifiers must not include the created one"
        );
        pairs.push((unheld.to_string(), "LIST"));
    }

    for (configured_table_id, rule_type) in pairs {
        let label = format!("pair ({configured_table_id}, {rule_type})");
        let expected = no_such_analysis_rule(&configured_table_id, rule_type);

        // The read.
        let (status, error_type, body) = emulator
            .get_analysis_rule_with_error_type(&configured_table_id, rule_type)
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected GetConfiguredTableAnalysisRule 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            error_type.ends_with("ResourceNotFoundException"),
            "case {label:?}: expected the x-amzn-errortype header to name ResourceNotFoundException, got {error_type:?}"
        );
        assert_eq!(
            error_message(&body), expected,
            "case {label:?}: expected the message {expected:?}, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(configured_table_id.as_str()),
            "case {label:?}: expected resourceId {configured_table_id:?}, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("CONFIGURED_TABLE"),
            "case {label:?}: expected resourceType CONFIGURED_TABLE, got {body}"
        );

        // The update and the delete address a rule by the same pair, so a pair holding
        // none is the same refusal for them.
        let (status, body) = emulator
            .update_analysis_rule(&configured_table_id, rule_type, policy_of(rule_type))
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected UpdateConfiguredTableAnalysisRule 404, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body), expected,
            "case {label:?}: expected the update's message {expected:?}, got {:?}",
            error_message(&body)
        );

        let (status, body) = emulator
            .delete_analysis_rule(&configured_table_id, rule_type)
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected DeleteConfiguredTableAnalysisRule 404, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body), expected,
            "case {label:?}: expected the delete's message {expected:?}, got {:?}",
            error_message(&body)
        );
    }

    // None of those refusals touched the stored rule.
    let (status, read) = emulator.get_analysis_rule(&table_id, "LIST").await;
    assert_eq!(
        status, 200,
        "the stored LIST rule must still read back, got {status} with {read}"
    );
    assert_eq!(
        read["analysisRule"], created["analysisRule"],
        "the stored rule must still be {}, got {}",
        created["analysisRule"], read["analysisRule"]
    );
}
