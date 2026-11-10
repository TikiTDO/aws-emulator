// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Clean Rooms schema slice (`GetSchema`).
//!
//! The rule under test: a schema is "a relation within a collaboration", and no request
//! creates one — a member contributes a relation by associating a configured table of its
//! own with its membership. So the relation a collaboration holds under a name is the
//! configured table association of that name in one of that collaboration's memberships,
//! read together with the configured table it refers to, and `GetSchema` reports it
//! DERIVED from those two stored records:
//!
//! * the relation's identity and timestamps come from the ASSOCIATION (its `name` is the
//!   relation's name, its id travels in `schemaTypeProperties`, and its ARN is restated
//!   in the `SchemaResourceArn` form the schema plane declares);
//! * the columns, the analysis methods and the analysis rule types come from the
//!   CONFIGURED TABLE: one column per column it allows "within collaborations", in the
//!   order it declares them, each typed as the stored `TableReference` declares it;
//! * each analysis rule type is READY only once BOTH halves of that rule exist — the data
//!   provider's rule on the configured table and the association's own rule of the same
//!   type — and the missing half is the reason it is NOT_READY;
//! * a name the collaboration holds no relation under is the modelled 404, and a
//!   collaboration the caller is no party to is the 403 every collaboration-scoped read
//!   answers with;
//! * the read reports the same relation however often it is called and changes nothing.
//!
//! The table rows vary what the rule generalises over: every `TableReference` member (so
//! both a reference that declares its columns' types and the three that do not), the
//! number of allowed columns, the description present or absent, every
//! `ConfiguredTableAnalysisRuleType` plus the table that holds no rule at all, and
//! whether the association's own rule of that type is configured. Every request goes over
//! the wire through the same `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    account_id, collaboration_owned_elsewhere, error_code, error_message, is_model_uuid,
    minimal_collaboration, Emulator, REGION,
};
use super::cleanrooms_configured_table::{
    create_body, table_reference, ALL_SELECTED_ANALYSIS_METHODS, ALL_TABLE_REFERENCE_TYPES,
    COLUMN_NAMES,
};
// Every `ConfiguredTableAnalysisRuleType` the model declares, and the shared way to
// configure one on a configured table, as the analysis-rule suite already exports them.
use super::configured_table_analysis_rules::ALL_ANALYSIS_RULE_TYPES;

/// A `UUID` in the form the model's pattern declares that no create here mints.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

impl Emulator {
    async fn get_schema(&self, collaboration_id: &str, name: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/collaborations/{collaboration_id}/schemas/{name}"),
            None,
        )
        .await
    }

    async fn create_schema_association(
        &self,
        membership_id: &str,
        body: Value,
    ) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/memberships/{membership_id}/configuredTableAssociations"),
            Some(body),
        )
        .await
    }

    async fn create_schema_association_rule(
        &self,
        membership_id: &str,
        association_id: &str,
        rule_type: &str,
    ) -> (u16, Value) {
        self.call(
            "POST",
            &format!(
                "/memberships/{membership_id}/configuredTableAssociations/{association_id}/analysisRule"
            ),
            Some(json!({
                "analysisRuleType": rule_type,
                "analysisRulePolicy": { "v1": { association_policy_member(rule_type): {} } },
            })),
        )
        .await
    }

}

/// The member of `ConfiguredTableAssociationAnalysisRulePolicyV1` that carries the policy
/// of `rule_type`; every member of each of those three policy shapes is optional, so an
/// empty policy is a valid one.
fn association_policy_member(rule_type: &str) -> &'static str {
    match rule_type {
        "AGGREGATION" => "aggregation",
        "LIST" => "list",
        "CUSTOM" => "custom",
        other => panic!("no association policy member for analysis rule type {other:?}"),
    }
}

/// A `ConfiguredTableAnalysisRulePolicyV1` of `rule_type` naming only the FIRST allowed
/// column, with `differentialPrivacy` on the custom rule when asked for.
///
/// The analysis-rule suite's own `aggregation_policy` / `list_policy` / `custom_policy`
/// builders name `COLUMN_NAMES[2]` as well, and a policy may only name columns the
/// configured table allows; the rows here vary how many columns the table allows down to
/// one, so the policy is built to the narrowest table instead.
fn table_policy(rule_type: &str, differential_privacy: bool) -> Value {
    let v1 = match rule_type {
        "AGGREGATION" => json!({
            "aggregation": {
                "aggregateColumns": [{ "columnNames": [COLUMN_NAMES[0]], "function": "COUNT" }],
                "joinColumns": [COLUMN_NAMES[0]],
                "dimensionColumns": [],
                "scalarFunctions": ["ABS"],
                "outputConstraints": [{
                    "columnName": COLUMN_NAMES[0],
                    "minimum": 2,
                    "type": "COUNT_DISTINCT",
                }],
            }
        }),
        "LIST" => json!({
            "list": { "joinColumns": [COLUMN_NAMES[0]], "listColumns": [] }
        }),
        "CUSTOM" => {
            let mut custom = json!({
                "allowedAnalyses": ["ANY_QUERY"],
                "allowedAnalysisProviders": [account_id()],
            });
            if differential_privacy {
                // `AnalysisRuleCustom$differentialPrivacy` names the user-identifier
                // column the noise is applied per.
                custom["differentialPrivacy"] = json!({
                    "columns": [{ "name": COLUMN_NAMES[0] }]
                });
            }
            json!({ "custom": custom })
        }
        other => panic!("no configured table policy for analysis rule type {other:?}"),
    };
    json!({ "v1": v1 })
}

/// The type a column reads as when the configured table's stored `TableReference`
/// declares none for it: the catalog-independent type of a column of unknown physical
/// type. Written out here rather than read from the service's own constant, so a change
/// to that constant is caught by this suite.
const UNDECLARED_COLUMN_TYPE: &str = "string";

/// The type the relation must report for `column`: the one the configured table's stored
/// `TableReference` declares for it, else [`UNDECLARED_COLUMN_TYPE`]. The declared type is
/// read from the SAME reference the create carried, so the expectation cannot drift from
/// the request.
fn expected_column_type(reference: &Value, column: &str) -> String {
    reference["snowflake"]["tableSchema"]["v1"]
        .as_array()
        .and_then(|columns| {
            columns
                .iter()
                .find(|declared| declared["columnName"].as_str() == Some(column))
        })
        .and_then(|declared| declared["columnType"].as_str())
        .unwrap_or(UNDECLARED_COLUMN_TYPE)
        .to_string()
}

/// The `SchemaResourceArn` form the model declares
/// (`^arn:aws:cleanrooms:…:membership\/[\d\w-]+\/(configuredtableassociation|idmappingtable)\/[\d\w-]+$`).
fn schema_resource_arn(membership_id: &str, association_id: &str) -> String {
    format!(
        "arn:aws:cleanrooms:{REGION}:{}:membership/{membership_id}/configuredtableassociation/{association_id}",
        account_id()
    )
}

/// An IAM role ARN of the caller's own account, in the form the model's `RoleArn` pattern
/// declares, so the association's pass-role check admits it.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", account_id())
}

/// What to contribute: a collaboration to join, a configured table to configure, and the
/// association that carries its relation into the collaboration.
struct Contribution<'a> {
    label: &'a str,
    alias: &'a str,
    table_type: &'static str,
    analysis_method: &'static str,
    column_count: usize,
    /// `selectedAnalysisMethods` the configured table is created with, if any (the model
    /// requires at least two distinct methods when the member is configured).
    selected_analysis_methods: Option<&'a [&'static str]>,
    description: Option<&'a str>,
    /// The analysis rule type the data provider configures on the configured table.
    table_rule: Option<&'a str>,
    /// Whether that rule's policy configures differential privacy.
    differential_privacy: bool,
    /// The analysis rule type the association configures for the collaboration.
    association_rule: Option<&'a str>,
}

impl<'a> Contribution<'a> {
    /// The smallest contribution: a relation of `column_count` columns with no analysis
    /// rule on either side.
    fn new(label: &'a str, alias: &'a str, table_type: &'static str, column_count: usize) -> Self {
        Self {
            label,
            alias,
            table_type,
            analysis_method: "DIRECT_QUERY",
            column_count,
            selected_analysis_methods: None,
            description: None,
            table_rule: None,
            differential_privacy: false,
            association_rule: None,
        }
    }
}

/// One contributed relation: everything the test knows about what it must report.
struct Contributed {
    collaboration_id: String,
    collaboration_arn: String,
    membership_id: String,
    association_id: String,
    association: Value,
    /// The `TableReference` the configured table was created with.
    reference: Value,
    columns: Vec<String>,
    analysis_method: &'static str,
    selected_analysis_methods: Option<Vec<&'static str>>,
}

/// Create a collaboration, join it, configure a table and associate it under the alias,
/// configuring whichever halves of an analysis rule the contribution asks for.
async fn contribute(emulator: &Emulator, request: Contribution<'_>) -> Contributed {
    let label = request.label;
    let (status, created) = emulator
        .create_collaboration(minimal_collaboration(&format!("{label}-collab")))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected CreateCollaboration 200, got {status} with {created}"
    );
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no collaboration.id in {created}"))
        .to_string();
    let collaboration_arn = created["collaboration"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no collaboration.arn in {created}"))
        .to_string();

    let (status, joined) = emulator
        .create_membership(json!({
            "collaborationIdentifier": collaboration_id,
            "queryLogStatus": "ENABLED",
        }))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected CreateMembership 200, got {status} with {joined}"
    );
    let membership_id = joined["membership"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no membership.id in {joined}"))
        .to_string();

    let (status, table) = emulator
        .create_configured_table({
            let mut body = create_body(
                &format!("{label}-table"),
                request.table_type,
                request.analysis_method,
                None,
                request.column_count,
            );
            if let Some(methods) = request.selected_analysis_methods {
                body["selectedAnalysisMethods"] = json!(methods.to_vec());
            }
            body
        })
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected CreateConfiguredTable 200, got {status} with {table}"
    );
    let table_id = table["configuredTable"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no configuredTable.id in {table}"))
        .to_string();

    if let Some(rule_type) = request.table_rule {
        // The analysis-rule suite's own create helper, so the request shape cannot drift
        // between the two suites.
        let (status, rule) = emulator
            .create_analysis_rule(
                &table_id,
                rule_type,
                table_policy(rule_type, request.differential_privacy),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateConfiguredTableAnalysisRule 200 for \
             {rule_type}, got {status} with {rule}"
        );
    }

    let mut body = json!({
        "name": request.alias,
        "configuredTableIdentifier": table_id,
        "roleArn": role_arn("clean-rooms-schema-role"),
    });
    if let Some(description) = request.description {
        body["description"] = json!(description);
    }
    let (status, associated) = emulator
        .create_schema_association(&membership_id, body)
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected CreateConfiguredTableAssociation 200, got {status} \
         with {associated}"
    );
    let association = associated["configuredTableAssociation"].clone();
    let association_id = association["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label:?}: no configuredTableAssociation.id in {associated}"))
        .to_string();

    if let Some(rule_type) = request.association_rule {
        let (status, rule) = emulator
            .create_schema_association_rule(&membership_id, &association_id, rule_type)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected CreateConfiguredTableAssociationAnalysisRule 200 \
             for {rule_type}, got {status} with {rule}"
        );
    }

    // Re-read the association AFTER the rule, so the expectation carries the timestamps
    // the store holds now rather than the ones the create answered with.
    let (status, reread) = emulator
        .call(
            "GET",
            &format!(
                "/memberships/{membership_id}/configuredTableAssociations/{association_id}"
            ),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected GetConfiguredTableAssociation 200, got {status} with {reread}"
    );

    Contributed {
        collaboration_id,
        collaboration_arn,
        membership_id,
        association_id,
        association: reread["configuredTableAssociation"].clone(),
        reference: table_reference(request.table_type),
        columns: COLUMN_NAMES[..request.column_count]
            .iter()
            .map(|name| name.to_string())
            .collect(),
        analysis_method: request.analysis_method,
        selected_analysis_methods: request
            .selected_analysis_methods
            .map(|methods| methods.to_vec()),
    }
}

/// One row of the relation table.
struct Case {
    label: String,
    alias: String,
    table_type: &'static str,
    analysis_method: &'static str,
    column_count: usize,
    selected_analysis_methods: Option<&'static [&'static str]>,
    description: Option<&'static str>,
    /// The analysis rule type the data provider configures on the configured table, if
    /// any.
    table_rule: Option<&'static str>,
    /// Whether that rule's policy configures differential privacy.
    differential_privacy: bool,
    /// Whether the association also holds its own rule of that type.
    association_rule: bool,
}

/// Every `TableReference` member crossed with every analysis-rule state (none, each type
/// without the association's half, each type with it), with the allowed-column count, the
/// analysis method, the selected analysis methods, the description and whether the custom
/// policy configures differential privacy varying across the rows.
fn relation_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    let rule_states: Vec<(Option<&'static str>, bool)> = std::iter::once((None, false))
        .chain(
            ALL_ANALYSIS_RULE_TYPES
                .iter()
                .flat_map(|rule_type| [(Some(*rule_type), false), (Some(*rule_type), true)]),
        )
        .collect();
    for (i, table_type) in ALL_TABLE_REFERENCE_TYPES.iter().enumerate() {
        for (j, (table_rule, association_rule)) in rule_states.iter().enumerate() {
            let row = i * rule_states.len() + j;
            // Only the CUSTOM rule admits every analysis method; the aggregation and
            // list rules are the SQL-query rules, so a table holding one declares
            // DIRECT_QUERY.
            let analysis_method = match table_rule {
                Some("CUSTOM") | None => ["MULTIPLE", "DIRECT_QUERY", "DIRECT_JOB"][row % 3],
                _ => "DIRECT_QUERY",
            };
            cases.push(Case {
                label: format!(
                    "tableReference={table_type}, tableRule={table_rule:?}, \
                     associationRule={association_rule}, columns={}",
                    1 + row % COLUMN_NAMES.len()
                ),
                alias: format!("relation_{row}"),
                table_type,
                analysis_method,
                column_count: 1 + row % COLUMN_NAMES.len(),
                // A table that admits both analysis methods is the one that declares
                // which of them is selected; the model requires at least two distinct
                // values when the member is configured.
                selected_analysis_methods: (analysis_method == "MULTIPLE")
                    .then_some(&ALL_SELECTED_ANALYSIS_METHODS[..]),
                description: if row.is_multiple_of(2) {
                    Some("the relation the association contributes")
                } else {
                    None
                },
                table_rule: *table_rule,
                // Differential privacy is a member of the CUSTOM policy only, so the
                // rows that configure it are the complete custom ones.
                differential_privacy: *table_rule == Some("CUSTOM") && *association_rule,
                association_rule: *association_rule,
            });
        }
    }
    cases
}

#[tokio::test]
async fn a_schema_reports_the_relation_its_association_contributes() {
    for case in relation_cases() {
        let label = case.label.clone();
        let emulator = Emulator::new();
        let contributed = contribute(
            &emulator,
            Contribution {
                label: &label,
                alias: &case.alias,
                table_type: case.table_type,
                analysis_method: case.analysis_method,
                column_count: case.column_count,
                selected_analysis_methods: case.selected_analysis_methods,
                description: case.description,
                table_rule: case.table_rule,
                differential_privacy: case.differential_privacy,
                association_rule: case.association_rule.then_some(case.table_rule).flatten(),
            },
        )
        .await;

        let (status, body) = emulator
            .get_schema(&contributed.collaboration_id, &case.alias)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetSchema 200, got {status} with {body}"
        );
        let schema = &body["schema"];

        assert_eq!(
            schema["name"].as_str(),
            Some(case.alias.as_str()),
            "case {label:?}: the relation's name is the association's table alias, got {schema}"
        );
        assert_eq!(
            schema["type"].as_str(),
            Some("TABLE"),
            "case {label:?}: a configured table association contributes a TABLE, got {schema}"
        );
        assert_eq!(
            schema["collaborationId"].as_str(),
            Some(contributed.collaboration_id.as_str()),
            "case {label:?}: expected the collaboration the relation was read through, got {schema}"
        );
        assert_eq!(
            schema["collaborationArn"].as_str(),
            Some(contributed.collaboration_arn.as_str()),
            "case {label:?}: expected the stored collaboration's ARN, got {schema}"
        );
        assert_eq!(
            schema["creatorAccountId"].as_str(),
            Some(account_id()),
            "case {label:?}: the relation is owned by the account holding the membership, \
             got {schema}"
        );
        assert_eq!(
            schema["description"].as_str(),
            Some(case.description.unwrap_or_default()),
            "case {label:?}: expected the association's description, got {schema}"
        );
        assert_eq!(
            schema["createTime"], contributed.association["createTime"],
            "case {label:?}: the relation was created with its association, got {schema}"
        );
        assert_eq!(
            schema["updateTime"], contributed.association["updateTime"],
            "case {label:?}: the relation moves with its association, got {schema}"
        );
        assert_eq!(
            schema["resourceArn"].as_str(),
            Some(
                schema_resource_arn(&contributed.membership_id, &contributed.association_id)
                    .as_str()
            ),
            "case {label:?}: expected the association named in the SchemaResourceArn form, \
             got {schema}"
        );
        assert_eq!(
            schema["schemaTypeProperties"]["configuredTableAssociation"]
                ["configuredTableAssociationId"]
                .as_str(),
            Some(contributed.association_id.as_str()),
            "case {label:?}: expected the contributing association's id, got {schema}"
        );
        assert!(
            is_model_uuid(&contributed.association_id),
            "case {label:?}: the association id must be a model UUID, got {:?}",
            contributed.association_id
        );

        // The columns are the configured table's allowed columns, in its order, each
        // typed as the stored table reference declares it.
        let expected_columns: Vec<Value> = contributed
            .columns
            .iter()
            .map(|name| {
                json!({
                    "name": name,
                    "type": expected_column_type(&contributed.reference, name),
                })
            })
            .collect();
        assert_eq!(
            schema["columns"],
            Value::Array(expected_columns.clone()),
            "case {label:?}: expected columns {expected_columns:?}, got {}",
            schema["columns"]
        );
        assert_eq!(
            schema["partitionKeys"],
            json!([]),
            "case {label:?}: no partition keys are declared to this service, got {}",
            schema["partitionKeys"]
        );
        assert_eq!(
            schema["analysisMethod"].as_str(),
            Some(contributed.analysis_method),
            "case {label:?}: expected the configured table's analysis method, got {schema}"
        );
        assert_eq!(
            schema["analysisRuleTypes"],
            json!(case.table_rule.map(|t| vec![t]).unwrap_or_default()),
            "case {label:?}: expected the configured table's analysis rule types, got {}",
            schema["analysisRuleTypes"]
        );
        assert_eq!(
            schema["selectedAnalysisMethods"],
            match &contributed.selected_analysis_methods {
                Some(methods) => json!(methods),
                None => Value::Null,
            },
            "case {label:?}: expected the configured table's selected analysis methods, got {}",
            schema["selectedAnalysisMethods"]
        );

        // The status details: one entry per rule type the table holds, READY only once
        // the association holds its own rule of that type too.
        let details = schema["schemaStatusDetails"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: no schemaStatusDetails in {schema}"));
        assert_eq!(
            details.len(),
            1,
            "case {label:?}: expected one status detail, got {}",
            schema["schemaStatusDetails"]
        );
        let detail = &details[0];
        assert_eq!(
            detail["analysisType"].as_str(),
            Some("DIRECT_ANALYSIS"),
            "case {label:?}: a relation of an association is analysed directly, got {detail}"
        );
        match (case.table_rule, case.association_rule) {
            (None, _) => {
                assert_eq!(
                    detail["status"].as_str(),
                    Some("NOT_READY"),
                    "case {label:?}: a table with no analysis rule is not ready, got {detail}"
                );
                assert_eq!(
                    detail["reasons"][0]["code"].as_str(),
                    Some("ANALYSIS_RULE_MISSING"),
                    "case {label:?}: expected the missing-rule reason, got {detail}"
                );
                assert_eq!(
                    detail["analysisRuleType"],
                    Value::Null,
                    "case {label:?}: no rule type has been evaluated, got {detail}"
                );
            }
            (Some(rule_type), false) => {
                assert_eq!(
                    detail["status"].as_str(),
                    Some("NOT_READY"),
                    "case {label:?}: the collaboration-side rule is missing, got {detail}"
                );
                assert_eq!(
                    detail["reasons"][0]["code"].as_str(),
                    Some("COLLABORATION_ANALYSIS_RULE_NOT_CONFIGURED"),
                    "case {label:?}: expected the collaboration-rule reason, got {detail}"
                );
                assert_eq!(
                    detail["analysisRuleType"].as_str(),
                    Some(rule_type),
                    "case {label:?}: expected the evaluated rule type, got {detail}"
                );
            }
            (Some(rule_type), true) => {
                assert_eq!(
                    detail["status"].as_str(),
                    Some("READY"),
                    "case {label:?}: both halves of the rule exist, got {detail}"
                );
                assert_eq!(
                    detail["reasons"],
                    Value::Null,
                    "case {label:?}: a ready relation states no reason, got {detail}"
                );
                assert_eq!(
                    detail["analysisRuleType"].as_str(),
                    Some(rule_type),
                    "case {label:?}: expected the evaluated rule type, got {detail}"
                );
            }
        }
        assert_eq!(
            detail["configurations"],
            if case.differential_privacy {
                json!(["DIFFERENTIAL_PRIVACY"])
            } else {
                Value::Null
            },
            "case {label:?}: expected the configurations the stored policy declares, got {detail}"
        );
    }
}

#[tokio::test]
async fn a_relation_the_collaboration_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    let held = contribute(
        &emulator,
        Contribution {
            table_rule: Some("LIST"),
            association_rule: Some("LIST"),
            ..Contribution::new("notfound", "relation_held", "GLUE", COLUMN_NAMES.len())
        },
    )
    .await;
    // A second collaboration, holding a relation of its own under another name: a
    // relation is addressed through ONE collaboration, so neither is visible in the
    // other.
    let other = contribute(
        &emulator,
        Contribution::new("notfound-other", "relation_elsewhere", "GLUE", 1),
    )
    .await;

    let rows = [
        ("a name nothing was associated under", "relation_absent"),
        (
            "a name held by another collaboration's association",
            "relation_elsewhere",
        ),
        ("the alias in another case", "relation_0"),
    ];
    for (why, name) in rows {
        let (status, body) = emulator.get_schema(&held.collaboration_id, name).await;
        assert_eq!(
            status, 404,
            "case {why:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {why:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "No schema found with name {name} in collaboration {}.",
                held.collaboration_id
            ),
            "case {why:?}: expected the not-found wording, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(name),
            "case {why:?}: the refusal names the relation asked for, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("SCHEMA"),
            "case {why:?}: expected the missing resource's type, got {body}"
        );
    }

    // Each collaboration still reports its own relation, so the misses above are scoping
    // and not a broken lookup.
    for (collaboration_id, name) in [
        (&held.collaboration_id, "relation_held"),
        (&other.collaboration_id, "relation_elsewhere"),
    ] {
        let (status, body) = emulator.get_schema(collaboration_id, name).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected GetSchema 200 in its own collaboration, got {status} \
             with {body}"
        );
        assert_eq!(
            body["schema"]["name"].as_str(),
            Some(name),
            "case {name:?}: expected the relation of that name, got {body}"
        );
    }
}

#[tokio::test]
async fn a_collaboration_the_caller_is_no_party_to_is_denied() {
    let emulator = Emulator::new();
    let foreign = collaboration_owned_elsewhere(&emulator, "schema-foreign", None).await;

    for (why, collaboration_id) in [
        ("owned by another account", foreign.as_str()),
        ("never minted", ABSENT_UUID),
    ] {
        let (status, body) = emulator.get_schema(collaboration_id, "relation_0").await;
        assert_eq!(
            status, 403,
            "case {why:?}: expected 403, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {why:?}: expected AccessDeniedException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "User: arn:aws:iam::{account}:user/emulator is not authorized to perform: \
                 cleanrooms:GetSchema on resource: \
                 arn:aws:cleanrooms:{REGION}:{account}:collaboration/{collaboration_id}",
                account = account_id()
            ),
            "case {why:?}: expected the authorization-failure wording, got {:?}",
            error_message(&body)
        );
        assert_eq!(
            body["reason"].as_str(),
            Some("INSUFFICIENT_PERMISSIONS"),
            "case {why:?}: expected the modelled reason, got {body}"
        );
    }
}

#[tokio::test]
async fn reading_a_schema_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let contributed = contribute(
        &emulator,
        Contribution {
            description: Some("read twice"),
            table_rule: Some("CUSTOM"),
            association_rule: Some("CUSTOM"),
            ..Contribution::new("repeat", "relation_repeat", "SNOWFLAKE", COLUMN_NAMES.len())
        },
    )
    .await;

    let (status, first) = emulator
        .get_schema(&contributed.collaboration_id, "relation_repeat")
        .await;
    assert_eq!(
        status, 200,
        "case repeat: expected the first GetSchema 200, got {status} with {first}"
    );
    let (status, second) = emulator
        .get_schema(&contributed.collaboration_id, "relation_repeat")
        .await;
    assert_eq!(
        status, 200,
        "case repeat: expected the second GetSchema 200, got {status} with {second}"
    );
    assert_eq!(
        first, second,
        "case repeat: a read must report the same relation, got {first} then {second}"
    );

    // The reporting operation stored nothing of its own: the association and the
    // configured table it derives from are still the only records.
    let store = contributed_store_counts(&emulator);
    let (status, _) = emulator
        .get_schema(&contributed.collaboration_id, "relation_repeat")
        .await;
    assert_eq!(status, 200, "case repeat: expected a third GetSchema 200");
    assert_eq!(
        store,
        contributed_store_counts(&emulator),
        "case repeat: a read must not change the store, got {:?} after {:?}",
        contributed_store_counts(&emulator),
        store
    );
}

/// How many records of each kind the store holds — what a reporting operation must leave
/// exactly as it found.
fn contributed_store_counts(emulator: &Emulator) -> (usize, usize, usize, usize) {
    let store = emulator.store();
    let store = store.lock();
    (
        store.collaborations.len(),
        store.memberships.len(),
        store.configured_tables.len(),
        store.configured_table_associations.len(),
    )
}
