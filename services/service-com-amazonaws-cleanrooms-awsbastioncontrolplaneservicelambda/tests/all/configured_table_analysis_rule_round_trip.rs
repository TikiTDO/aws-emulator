// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Clean Rooms configured-table analysis-rule round trip:
//! `CreateConfiguredTableAnalysisRule` → `GetConfiguredTableAnalysisRule` →
//! `UpdateConfiguredTableAnalysisRule` → `GetConfiguredTableAnalysisRule` →
//! `DeleteConfiguredTableAnalysisRule` → `GetConfiguredTableAnalysisRule`.
//!
//! The rules under test:
//!
//! * a read of an analysis rule reports the policy the LAST write persisted, member for
//!   member — every optional member that write carried and none it did not — so an update
//!   replaces the stored policy wholesale and the read after it reports the new one;
//! * an analysis rule is identified by the `(configuredTableIdentifier,
//!   analysisRuleType)` pair the request path carries ("Configured table analysis rules
//!   are uniquely identified by their configured table identifier and analysis rule
//!   type"), so every operation that addresses a rule through that pair resolves it
//!   against persisted state and refuses a pair the store holds no rule for with the
//!   modelled 404 `ResourceNotFoundException` naming that pair (the refusal itself is
//!   pinned in `configured_table_analysis_rule_reads`);
//! * a delete drops the rule and the type it is held under, so the configured table
//!   stops reporting it in `analysisRuleTypes` and the pair is free for a new create;
//! * a refused update persists nothing: the stored policy a later read reports is the one
//!   from before it.
//!
//! The tables vary what those rules generalise over — every
//! `ConfiguredTableAnalysisRuleType`, and for the policies the optional members a policy
//! may carry (absent, present, how many entries, in which order) plus which columns and
//! how many each names. Every request goes over the wire through the same
//! `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{account_id, error_code, error_message, Emulator};
use super::cleanrooms_configured_table::{create_body, COLUMN_NAMES};

/// Every `JoinOperator` the model declares (shape `JoinOperator`: OR | AND).
const ALL_JOIN_OPERATORS: [&str; 2] = ["OR", "AND"];

/// The published maximum of `JoinOperatorsList` (model shape `JoinOperatorsList`,
/// `@length max 2`), which is also how many distinct `JoinOperator` values exist.
const MAX_JOIN_OPERATORS: usize = 2;

/// Every `AdditionalAnalyses` the model declares (shape `AdditionalAnalyses`).
const ALL_ADDITIONAL_ANALYSES: [&str; 3] = ["ALLOWED", "REQUIRED", "NOT_ALLOWED"];

/// The refusal a `(configured table, analysis rule type)` pair holding no analysis rule
/// is answered with, as the service words it: the type is named by the INTERNAL name of
/// its rule class ([`internal_analysis_rule_class`]), not by the catalog value the request
/// path carried.
pub(crate) fn no_such_analysis_rule(configured_table_id: &str, analysis_rule_type: &str) -> String {
    format!(
        "No such analysis rule with configuredTableId: {configured_table_id} and type {}",
        internal_analysis_rule_class(analysis_rule_type)
    )
}

/// The internal, SHOUTY_SNAKE_CASE name of the rule class a `ConfiguredTableAnalysisRuleType`
/// names (model shape `ConfiguredTableAnalysisRuleType`: AGGREGATION | LIST | CUSTOM): the
/// catalog value itself, except for LIST, whose class is named for the intersection a list
/// analysis returns.
pub(crate) fn internal_analysis_rule_class(analysis_rule_type: &str) -> &str {
    match analysis_rule_type {
        "LIST" => "INTERSECTION_LIST",
        other => other,
    }
}

/// An `AnalysisRuleList` policy: `joinColumns` and `listColumns` are required;
/// `allowedJoinOperators` and `additionalAnalyses` are optional and present only when
/// given.
pub(crate) fn list_policy(
    join_columns: &[&str],
    list_columns: &[&str],
    allowed_join_operators: Option<&[&str]>,
    additional_analyses: Option<&str>,
) -> Value {
    let mut list = json!({
        "joinColumns": join_columns.to_vec(),
        "listColumns": list_columns.to_vec(),
    });
    if let Some(operators) = allowed_join_operators {
        list["allowedJoinOperators"] = json!(operators.to_vec());
    }
    if let Some(additional) = additional_analyses {
        list["additionalAnalyses"] = json!(additional);
    }
    json!({ "v1": { "list": list } })
}

/// An `AnalysisRuleAggregation` policy: `aggregateColumns`, `joinColumns`,
/// `dimensionColumns`, `scalarFunctions` and `outputConstraints` are required;
/// `allowedJoinOperators` and `joinRequired` are optional.
pub(crate) fn aggregation_policy(
    aggregate_columns: &[&str],
    function: &str,
    allowed_join_operators: Option<&[&str]>,
    join_required: Option<&str>,
    minimum: i64,
) -> Value {
    let mut aggregation = json!({
        "aggregateColumns": [{ "columnNames": aggregate_columns.to_vec(), "function": function }],
        "joinColumns": [COLUMN_NAMES[0]],
        "dimensionColumns": [COLUMN_NAMES[2]],
        "scalarFunctions": ["ABS"],
        "outputConstraints": [{
            "columnName": COLUMN_NAMES[2],
            "minimum": minimum,
            "type": "COUNT_DISTINCT",
        }],
    });
    if let Some(operators) = allowed_join_operators {
        aggregation["allowedJoinOperators"] = json!(operators.to_vec());
    }
    if let Some(required) = join_required {
        aggregation["joinRequired"] = json!(required);
    }
    json!({ "v1": { "aggregation": aggregation } })
}

/// An `AnalysisRuleCustom` policy: `allowedAnalyses` is required, and the provider list
/// is "Required when allowedAnalyses is ANY_QUERY"; `disallowedOutputColumns` and
/// `additionalAnalyses` are optional.
pub(crate) fn custom_policy(
    disallowed_output_columns: Option<&[&str]>,
    additional_analyses: Option<&str>,
) -> Value {
    let mut custom = json!({
        "allowedAnalyses": ["ANY_QUERY"],
        "allowedAnalysisProviders": [account_id()],
    });
    if let Some(columns) = disallowed_output_columns {
        custom["disallowedOutputColumns"] = json!(columns.to_vec());
    }
    if let Some(additional) = additional_analyses {
        custom["additionalAnalyses"] = json!(additional);
    }
    json!({ "v1": { "custom": custom } })
}

impl Emulator {
    pub(crate) async fn update_analysis_rule(
        &self,
        configured_table_id: &str,
        rule_type: &str,
        policy: Value,
    ) -> (u16, Value) {
        self.call(
            "PATCH",
            &format!("/configuredTables/{configured_table_id}/analysisRule/{rule_type}"),
            Some(json!({ "analysisRulePolicy": policy })),
        )
        .await
    }

    pub(crate) async fn delete_analysis_rule(
        &self,
        configured_table_id: &str,
        rule_type: &str,
    ) -> (u16, Value) {
        self.call(
            "DELETE",
            &format!("/configuredTables/{configured_table_id}/analysisRule/{rule_type}"),
            None,
        )
        .await
    }

    /// As [`Emulator::get_analysis_rule`], plus the `x-amzn-errortype` header restJson1
    /// names the error shape in.
    pub(crate) async fn get_analysis_rule_with_error_type(
        &self,
        configured_table_id: &str,
        rule_type: &str,
    ) -> (u16, String, Value) {
        self.call_with_error_type(
            "GET",
            &format!("/configuredTables/{configured_table_id}/analysisRule/{rule_type}"),
            None,
        )
        .await
    }

    /// A fresh configured table over every column name, and its id.
    pub(crate) async fn analysis_rule_table(&self, row: usize) -> String {
        let (status, created) = self
            .create_configured_table(create_body(
                &format!("round-trip-table-{row}"),
                "GLUE",
                "MULTIPLE",
                None,
                COLUMN_NAMES.len(),
            ))
            .await;
        assert_eq!(
            status, 200,
            "row {row}: expected CreateConfiguredTable 200, got {status} with {created}"
        );
        created["configuredTable"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// One row of the update table: the policy a rule is created with and the policy an
/// update replaces it by.
struct UpdateCase {
    label: &'static str,
    rule_type: &'static str,
    created: Value,
    updated: Value,
}

/// The update table. The rows differ in which optional members each policy carries
/// (absent → present, present → absent, one value → another), how many entries an
/// optional list carries (one, and `JoinOperatorsList`'s published maximum of two), the
/// ORDER of those entries, and which columns and how many the policy names — crossed
/// with every `ConfiguredTableAnalysisRuleType`, since each one's policy is a different
/// member of the `ConfiguredTableAnalysisRulePolicyV1` union.
fn update_cases() -> Vec<UpdateCase> {
    let (first, second) = (ALL_JOIN_OPERATORS[0], ALL_JOIN_OPERATORS[1]);
    vec![
        UpdateCase {
            label: "LIST: allowedJoinOperators absent -> one operator",
            rule_type: "LIST",
            created: list_policy(&COLUMN_NAMES[..1], &COLUMN_NAMES[1..], None, None),
            updated: list_policy(
                &COLUMN_NAMES[..1],
                &COLUMN_NAMES[1..],
                Some(&[first]),
                None,
            ),
        },
        UpdateCase {
            label: "LIST: one operator -> absent",
            rule_type: "LIST",
            created: list_policy(
                &COLUMN_NAMES[..1],
                &COLUMN_NAMES[1..],
                Some(&[first]),
                Some(ALL_ADDITIONAL_ANALYSES[0]),
            ),
            updated: list_policy(&COLUMN_NAMES[..1], &COLUMN_NAMES[1..], None, None),
        },
        UpdateCase {
            label: "LIST: one operator -> the other operator, and more list columns",
            rule_type: "LIST",
            created: list_policy(
                &COLUMN_NAMES[..1],
                &COLUMN_NAMES[1..2],
                Some(&[first]),
                None,
            ),
            updated: list_policy(
                &COLUMN_NAMES[..1],
                &COLUMN_NAMES[1..],
                Some(&[second]),
                Some(ALL_ADDITIONAL_ANALYSES[2]),
            ),
        },
        UpdateCase {
            label: "LIST: two operators -> the same two reversed (order is the caller's)",
            rule_type: "LIST",
            created: list_policy(
                &COLUMN_NAMES[..1],
                &COLUMN_NAMES[1..],
                Some(&[first, second]),
                Some(ALL_ADDITIONAL_ANALYSES[1]),
            ),
            updated: list_policy(
                &COLUMN_NAMES[..1],
                &COLUMN_NAMES[1..],
                Some(&[second, first]),
                Some(ALL_ADDITIONAL_ANALYSES[1]),
            ),
        },
        UpdateCase {
            label: "LIST: one operator -> the published maximum of two",
            rule_type: "LIST",
            created: list_policy(
                &COLUMN_NAMES[..1],
                &COLUMN_NAMES[1..],
                Some(&[second]),
                None,
            ),
            updated: list_policy(
                &COLUMN_NAMES[..1],
                &COLUMN_NAMES[1..],
                Some(&ALL_JOIN_OPERATORS[..MAX_JOIN_OPERATORS]),
                None,
            ),
        },
        UpdateCase {
            label: "AGGREGATION: allowedJoinOperators absent -> two, other function and minimum",
            rule_type: "AGGREGATION",
            created: aggregation_policy(&COLUMN_NAMES[1..2], "SUM", None, None, 2),
            updated: aggregation_policy(
                &COLUMN_NAMES[1..2],
                "COUNT_DISTINCT",
                Some(&ALL_JOIN_OPERATORS[..MAX_JOIN_OPERATORS]),
                Some("QUERY_RUNNER"),
                100_000,
            ),
        },
        UpdateCase {
            label: "AGGREGATION: two operators -> one, and joinRequired dropped",
            rule_type: "AGGREGATION",
            created: aggregation_policy(
                &COLUMN_NAMES[1..2],
                "AVG",
                Some(&[first, second]),
                Some("QUERY_RUNNER"),
                7,
            ),
            updated: aggregation_policy(&COLUMN_NAMES[1..2], "AVG", Some(&[second]), None, 7),
        },
        UpdateCase {
            label: "CUSTOM: optional members absent -> present",
            rule_type: "CUSTOM",
            created: custom_policy(None, None),
            updated: custom_policy(
                Some(&COLUMN_NAMES[..2]),
                Some(ALL_ADDITIONAL_ANALYSES[0]),
            ),
        },
        UpdateCase {
            label: "CUSTOM: optional members present -> absent",
            rule_type: "CUSTOM",
            created: custom_policy(
                Some(&COLUMN_NAMES[2..]),
                Some(ALL_ADDITIONAL_ANALYSES[1]),
            ),
            updated: custom_policy(None, None),
        },
    ]
}

/// The timestamps a rule reports, as the wire carries them.
fn timestamps(rule: &Value) -> (f64, f64) {
    let read = |member: &str| {
        rule[member]
            .as_f64()
            .unwrap_or_else(|| panic!("{member} must be a timestamp, got {rule}"))
    };
    (read("createTime"), read("updateTime"))
}

#[tokio::test]
async fn update_configured_table_analysis_rule_replaces_the_policy_a_later_read_reports() {
    let emulator = Emulator::new();

    for (row, case) in update_cases().into_iter().enumerate() {
        let label = case.label;
        let table_id = emulator.analysis_rule_table(row).await;

        let (status, created) = emulator
            .create_analysis_rule(&table_id, case.rule_type, case.created.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateConfiguredTableAnalysisRule 200, got {status} with {created}"
        );
        let created_rule = created["analysisRule"].clone();
        let (create_time, _) = timestamps(&created_rule);

        // The read before the update reports exactly what the create persisted.
        let (status, before) = emulator.get_analysis_rule(&table_id, case.rule_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetConfiguredTableAnalysisRule 200, got {status} with {before}"
        );
        assert_eq!(
            before["analysisRule"], created_rule,
            "case {label:?}: the read must report the created rule {created_rule}, got {}",
            before["analysisRule"]
        );

        let (status, updated) = emulator
            .update_analysis_rule(&table_id, case.rule_type, case.updated.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected UpdateConfiguredTableAnalysisRule 200, got {status} with {updated}"
        );
        let updated_rule = updated["analysisRule"].clone();
        assert_eq!(
            updated_rule["policy"], case.updated,
            "case {label:?}: expected the new policy {}, got {}",
            case.updated, updated_rule["policy"]
        );
        assert_eq!(
            updated_rule["type"].as_str(),
            Some(case.rule_type),
            "case {label:?}: an update does not change the rule's type, got {updated_rule}"
        );
        assert_eq!(
            updated_rule["configuredTableId"].as_str(),
            Some(table_id.as_str()),
            "case {label:?}: expected configuredTableId {table_id:?}, got {updated_rule}"
        );
        assert_eq!(
            updated_rule["configuredTableArn"], created_rule["configuredTableArn"],
            "case {label:?}: an update does not re-mint the configuredTableArn {}, got {}",
            created_rule["configuredTableArn"], updated_rule["configuredTableArn"]
        );
        let (updated_create_time, update_time) = timestamps(&updated_rule);
        assert_eq!(
            updated_create_time, create_time,
            "case {label:?}: an update keeps the createTime {create_time}, got {updated_create_time}"
        );
        assert!(
            update_time >= create_time,
            "case {label:?}: expected an updateTime at or after the createTime {create_time}, got {update_time}"
        );

        // The read after the update reports exactly what the update persisted, and
        // reading changes nothing.
        let (status, after) = emulator.get_analysis_rule(&table_id, case.rule_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetConfiguredTableAnalysisRule 200, got {status} with {after}"
        );
        assert_eq!(
            after["analysisRule"], updated_rule,
            "case {label:?}: the read must report the updated rule {updated_rule}, got {}",
            after["analysisRule"]
        );
        let (status, reread) = emulator.get_analysis_rule(&table_id, case.rule_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected a second GetConfiguredTableAnalysisRule 200, got {status} with {reread}"
        );
        assert_eq!(
            reread, after,
            "case {label:?}: a read must not change what a later read sees; first {after}, then {reread}"
        );

        // The set of types the table holds a rule for has not changed.
        let (status, table) = emulator.get_configured_table(&table_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetConfiguredTable 200, got {status} with {table}"
        );
        assert_eq!(
            table["configuredTable"]["analysisRuleTypes"], json!([case.rule_type]),
            "case {label:?}: expected analysisRuleTypes [{:?}], got {}",
            case.rule_type, table["configuredTable"]["analysisRuleTypes"]
        );
    }
}

#[tokio::test]
async fn delete_configured_table_analysis_rule_drops_the_rule_and_frees_the_pair() {
    let emulator = Emulator::new();

    // Every rule type: each one's policy is a different member of the policy union, and
    // each is held under its own type on the table.
    let rows: [(&str, Value); 3] = [
        (
            "LIST",
            list_policy(
                &COLUMN_NAMES[..1],
                &COLUMN_NAMES[1..],
                Some(&ALL_JOIN_OPERATORS[..MAX_JOIN_OPERATORS]),
                None,
            ),
        ),
        (
            "AGGREGATION",
            aggregation_policy(&COLUMN_NAMES[1..2], "SUM", Some(&["OR"]), None, 2),
        ),
        ("CUSTOM", custom_policy(Some(&COLUMN_NAMES[..1]), None)),
    ];

    for (row, (rule_type, policy)) in rows.into_iter().enumerate() {
        let label = format!("analysisRuleType={rule_type}");
        let table_id = emulator.analysis_rule_table(row).await;
        let (status, created) = emulator
            .create_analysis_rule(&table_id, rule_type, policy.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the create to succeed, got {status} with {created}"
        );

        let (status, body) = emulator.delete_analysis_rule(&table_id, rule_type).await;
        assert_eq!(
            status, 204,
            "case {label:?}: expected DeleteConfiguredTableAnalysisRule 204, got {status} with {body}"
        );
        assert!(
            body == Value::Null || body == json!({}),
            "case {label:?}: a 204 carries no members, got {body}"
        );

        // The rule is gone: the read is the modelled 404, the table no longer reports
        // the type, and repeating the delete is the same 404.
        let (status, read) = emulator.get_analysis_rule(&table_id, rule_type).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected the read after the delete to be 404, got {status} with {read}"
        );
        assert_eq!(
            error_message(&read),
            no_such_analysis_rule(&table_id, rule_type),
            "case {label:?}: expected the pair's refusal, got {:?}",
            error_message(&read)
        );
        let (status, table) = emulator.get_configured_table(&table_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetConfiguredTable 200, got {status} with {table}"
        );
        assert_eq!(
            table["configuredTable"]["analysisRuleTypes"], json!([]),
            "case {label:?}: a deleted rule leaves analysisRuleTypes empty, got {}",
            table["configuredTable"]["analysisRuleTypes"]
        );
        let (status, repeated) = emulator.delete_analysis_rule(&table_id, rule_type).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected a repeated delete to be 404, got {status} with {repeated}"
        );

        // The pair is free again: a create for it succeeds and reads back.
        let (status, recreated) = emulator
            .create_analysis_rule(&table_id, rule_type, policy.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected a create after the delete to succeed, got {status} with {recreated}"
        );
        let (status, read) = emulator.get_analysis_rule(&table_id, rule_type).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the re-created rule to read back, got {status} with {read}"
        );
        assert_eq!(
            read["analysisRule"]["policy"], policy,
            "case {label:?}: expected the re-created policy {policy}, got {}",
            read["analysisRule"]["policy"]
        );
    }
}

#[tokio::test]
async fn a_refused_update_leaves_the_stored_policy_alone() {
    let emulator = Emulator::new();

    // A stored LIST rule, then updates that break one rule each: a policy of another
    // type than the pair's, and a policy naming a column the table does not allow.
    let narrow_table = {
        let (status, created) = emulator
            .create_configured_table(create_body(
                "refused-update-table",
                "GLUE",
                "DIRECT_QUERY",
                None,
                2,
            ))
            .await;
        assert_eq!(status, 200, "expected CreateConfiguredTable 200, got {status} with {created}");
        created["configuredTable"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let stored_policy = list_policy(
        &COLUMN_NAMES[..1],
        &COLUMN_NAMES[1..2],
        Some(&["OR"]),
        Some(ALL_ADDITIONAL_ANALYSES[0]),
    );
    let (status, created) = emulator
        .create_analysis_rule(&narrow_table, "LIST", stored_policy.clone())
        .await;
    assert_eq!(status, 200, "expected the LIST create to succeed, got {status} with {created}");
    let stored_rule = created["analysisRule"].clone();

    let refused: [(&str, Value); 3] = [
        (
            "a policy of another type than the pair's",
            aggregation_policy(&COLUMN_NAMES[1..2], "SUM", None, None, 2),
        ),
        (
            "a policy naming a column outside allowedColumns",
            list_policy(&COLUMN_NAMES[..1], &COLUMN_NAMES[2..], Some(&["AND"]), None),
        ),
        (
            "a policy joining on a column outside allowedColumns",
            list_policy(&COLUMN_NAMES[2..], &COLUMN_NAMES[1..2], None, None),
        ),
    ];

    for (label, policy) in refused {
        let (status, response) = emulator
            .update_analysis_rule(&narrow_table, "LIST", policy)
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected UpdateConfiguredTableAnalysisRule 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {response}"
        );
        assert_eq!(
            response["fieldList"][0]["name"].as_str(),
            Some("analysisRulePolicy"),
            "case {label:?}: expected the fieldList to name analysisRulePolicy, got {response}"
        );
        assert!(
            response.get("analysisRule").is_none(),
            "case {label:?}: a refused update reports no analysis rule, got {response}"
        );

        // The stored rule is untouched, timestamps included.
        let (status, read) = emulator.get_analysis_rule(&narrow_table, "LIST").await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the stored rule to still read back, got {status} with {read}"
        );
        assert_eq!(
            read["analysisRule"], stored_rule,
            "case {label:?}: the stored rule must still be {stored_rule}, got {}",
            read["analysisRule"]
        );
    }
}
