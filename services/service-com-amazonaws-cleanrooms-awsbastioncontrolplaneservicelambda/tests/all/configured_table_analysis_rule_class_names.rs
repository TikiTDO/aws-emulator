// SPDX-License-Identifier: CC0-1.0

//! What a configured-table analysis-rule refusal calls the rule TYPE.
//!
//! The rule under test: when the `(configuredTableIdentifier, analysisRuleType)` pair a
//! request addresses holds no analysis rule, the refusal names the type by the INTERNAL
//! name of the rule CLASS that type selects, not by the catalog value the request path
//! carried — the class is named for its catalog value except where the analysis it
//! performs is named otherwise. The modelled catalog value stays what every successful
//! response reports (`analysisRule.type`), so the internal name appears in the refusal
//! sentence and nowhere else.
//!
//! The table spans the whole `ConfiguredTableAnalysisRuleType` catalog — the types whose
//! class is named for them and the one whose class is not — and each row is driven
//! through all three operations that address a rule by the pair (the read, the update and
//! the delete), before and after the rule exists. The association plane renders the same
//! rule class as one CamelCase word and is pinned in `addressed_resource_refusals`.
//! Every request goes over the wire through the same `MultiServiceRouter` the production
//! router uses.

use super::cleanrooms_collaboration_lifecycle::{error_code, error_message, Emulator};
use super::configured_table_analysis_rule_reads::policy_of;
use super::configured_table_analysis_rule_round_trip::{
    internal_analysis_rule_class, no_such_analysis_rule,
};

/// Every `ConfiguredTableAnalysisRuleType` the model declares (model shape
/// `ConfiguredTableAnalysisRuleType`: AGGREGATION | LIST | CUSTOM), each paired with the
/// internal name of the rule class it selects: the catalog value itself, except for LIST,
/// whose class is named for the intersection of the joined tables a list analysis returns.
const RULE_CLASS_NAMES: [(&str, &str); 3] = [
    ("AGGREGATION", "AGGREGATION"),
    ("LIST", "INTERSECTION_LIST"),
    ("CUSTOM", "CUSTOM"),
];

/// Asserts the 404 `body` is the refusal the pair holding no rule answers with, naming the
/// rule class rather than the catalog value.
fn assert_refusal_names_the_class(
    case: &str,
    operation: &str,
    status: u16,
    body: &serde_json::Value,
    configured_table_id: &str,
    rule_type: &str,
    internal: &str,
) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected {operation} 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ResourceNotFoundException",
        "case {case:?}: expected {operation} to answer ResourceNotFoundException, got {body}"
    );
    let expected = no_such_analysis_rule(configured_table_id, rule_type);
    assert_eq!(
        error_message(body),
        expected,
        "case {case:?}: expected {operation} message {expected:?}, got {:?}",
        error_message(body)
    );
    assert!(
        error_message(body).ends_with(&format!(" and type {internal}")),
        "case {case:?}: expected {operation} to name the rule class {internal:?}, got {:?}",
        error_message(body)
    );
    if internal != rule_type {
        assert!(
            !error_message(body).contains(&format!(" and type {rule_type}")),
            "case {case:?}: expected {operation} NOT to name the catalog value {rule_type:?}, got {:?}",
            error_message(body)
        );
    }
}

#[tokio::test]
async fn an_absent_analysis_rule_is_refused_by_the_rule_class_name() {
    let emulator = Emulator::new();

    for (row, (rule_type, internal)) in RULE_CLASS_NAMES.into_iter().enumerate() {
        let case = format!("analysisRuleType={rule_type}");
        assert_eq!(
            internal_analysis_rule_class(rule_type),
            internal,
            "case {case:?}: expected the rule class {internal:?}, got {:?}",
            internal_analysis_rule_class(rule_type)
        );

        // A table that holds no analysis rule at all, so the pair below resolves to
        // nothing for the type under test.
        let table_id = emulator.analysis_rule_table(row).await;

        let (status, error_type, body) = emulator
            .get_analysis_rule_with_error_type(&table_id, rule_type)
            .await;
        assert!(
            error_type.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected the x-amzn-errortype header to name ResourceNotFoundException, got {error_type:?}"
        );
        assert_refusal_names_the_class(
            &case,
            "GetConfiguredTableAnalysisRule",
            status,
            &body,
            &table_id,
            rule_type,
            internal,
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(table_id.as_str()),
            "case {case:?}: expected resourceId {table_id:?}, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("CONFIGURED_TABLE"),
            "case {case:?}: expected resourceType CONFIGURED_TABLE, got {body}"
        );

        // The update and the delete address a rule through the same pair, so they name the
        // class the same way.
        let (status, body) = emulator
            .update_analysis_rule(&table_id, rule_type, policy_of(rule_type))
            .await;
        assert_refusal_names_the_class(
            &case,
            "UpdateConfiguredTableAnalysisRule",
            status,
            &body,
            &table_id,
            rule_type,
            internal,
        );
        let (status, body) = emulator.delete_analysis_rule(&table_id, rule_type).await;
        assert_refusal_names_the_class(
            &case,
            "DeleteConfiguredTableAnalysisRule",
            status,
            &body,
            &table_id,
            rule_type,
            internal,
        );

        // A successful response is unaffected: it reports the MODELLED catalog value.
        let (status, created) = emulator
            .create_analysis_rule(&table_id, rule_type, policy_of(rule_type))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to succeed, got {status} with {created}"
        );
        assert_eq!(
            created["analysisRule"]["type"].as_str(),
            Some(rule_type),
            "case {case:?}: expected the created rule's type {rule_type:?}, got {created}"
        );
        let (status, read) = emulator.get_analysis_rule(&table_id, rule_type).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read of the stored rule to succeed, got {status} with {read}"
        );
        assert_eq!(
            read["analysisRule"]["type"].as_str(),
            Some(rule_type),
            "case {case:?}: expected the read rule's type {rule_type:?}, got {read}"
        );

        // And once the rule is dropped the pair is refused by the class name again.
        let (status, deleted) = emulator.delete_analysis_rule(&table_id, rule_type).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected the delete of the stored rule to answer 204, got {status} with {deleted}"
        );
        let (status, body) = emulator.get_analysis_rule(&table_id, rule_type).await;
        assert_refusal_names_the_class(
            &case,
            "GetConfiguredTableAnalysisRule after the delete",
            status,
            &body,
            &table_id,
            rule_type,
            internal,
        );
    }
}
