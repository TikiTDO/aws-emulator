// SPDX-License-Identifier: CC0-1.0

//! The SCHEMAS a collaboration holds — the relations its members contributed to it.
//!
//! A schema is "a relation within a collaboration" (model shape `Schema`), and no
//! operation creates one: a member contributes a relation by associating a configured
//! table of its own with its membership, so the relation a collaboration holds under a
//! name IS the configured table association of that name in one of the collaboration's
//! memberships, read together with the configured table it refers to. Everything a
//! schema reports is therefore derived from those two stored records and the analysis
//! rules each of them holds — [`CleanRoomsState::schema_source`] resolves the name,
//! [`CleanRoomsState::schema_view`] projects the pair onto the modelled shape — so a
//! schema is never stored a second time and cannot drift from the resources it describes.

use super::*;

/// `SchemaType` values this slice produces (model shape `SchemaType`: TABLE |
/// ID_MAPPING_TABLE | INTERMEDIATE_TABLE).
///
/// A relation a configured table association contributes is a TABLE; the other two
/// values belong to the ID mapping tables and intermediate tables, which are schemas of
/// their own resources rather than of an association.
pub const SCHEMA_TYPE_TABLE: &str = "TABLE";

/// `SchemaStatus` values (model shape `SchemaStatus`: READY | NOT_READY). READY means
/// "based on the schema analysis rule, queries of the given analysis rule type are
/// properly configured to run queries on this schema" (model shape `SchemaStatusDetail`).
pub const SCHEMA_STATUS_READY: &str = "READY";
/// See [`SCHEMA_STATUS_READY`].
pub const SCHEMA_STATUS_NOT_READY: &str = "NOT_READY";

/// The `AnalysisType` a relation's status is evaluated for (model shape `AnalysisType`:
/// DIRECT_ANALYSIS | ADDITIONAL_ANALYSIS): a configured table association's relation is
/// the one a protected query reads directly, which is what DIRECT_ANALYSIS names.
pub const ANALYSIS_TYPE_DIRECT_ANALYSIS: &str = "DIRECT_ANALYSIS";

/// `SchemaStatusReasonCode` for a relation whose configured table holds no analysis rule
/// at all, so no analysis of it is configured (model shape `SchemaStatusReasonCode`).
pub const SCHEMA_STATUS_REASON_ANALYSIS_RULE_MISSING: &str = "ANALYSIS_RULE_MISSING";

/// `SchemaStatusReasonCode` for a relation whose configured table holds an analysis rule
/// of a type the ASSOCIATION holds no rule for, so the collaboration-side half of that
/// rule is still missing (model shape `SchemaStatusReasonCode`).
pub const SCHEMA_STATUS_REASON_COLLABORATION_ANALYSIS_RULE_NOT_CONFIGURED: &str =
    "COLLABORATION_ANALYSIS_RULE_NOT_CONFIGURED";

/// The one `SchemaConfiguration` the model declares (model shape `SchemaConfiguration`:
/// DIFFERENTIAL_PRIVACY) — reported for an analysis rule type whose stored policy
/// configures differential privacy.
pub const SCHEMA_CONFIGURATION_DIFFERENTIAL_PRIVACY: &str = "DIFFERENTIAL_PRIVACY";

/// The resource type a schema's 404 reports, named the same way as
/// [`RESOURCE_TYPE_ANALYSIS_TEMPLATE`]: the model's `ResourceType` catalog names no
/// schema, so the refusal reports the type of the resource that was not found rather
/// than one of the four that is not it.
pub const RESOURCE_TYPE_SCHEMA: &str = "SCHEMA";

/// The type a schema reports for an allowed column whose underlying table declares none
/// to this service.
///
/// `Column$type` is "the type of the column" of the relation, and the service learns it
/// from the dataset the configured table points at. Of the four `TableReference` members
/// the model declares, only `SnowflakeTableReference` carries the columns' types
/// (`SnowflakeTableReference$tableSchema`), so for a column the stored reference declares
/// no type for this value — the catalog-independent type a column of unknown physical
/// type reads as — is reported instead. It is the emulator's own choice, not a value AWS
/// publishes: this store holds no catalog of its own to read a column's type from.
pub const UNDECLARED_COLUMN_TYPE: &str = "string";

/// The message a schema lookup reports when the collaboration holds no relation under
/// the requested name.
///
/// Composed from the request's `name` and `collaborationIdentifier`: a relation's name is
/// unique within ONE collaboration ("the schema relation is referred to by this name when
/// queried by a protected query"), so the collaboration is what says the lookup failed in
/// that collaboration's namespace rather than that the name exists nowhere.
pub fn schema_not_found_message(name: &str, collaboration_id: &str) -> String {
    format!("No schema found with name {name} in collaboration {collaboration_id}.")
}

/// The explanation [`SCHEMA_STATUS_REASON_ANALYSIS_RULE_MISSING`] carries: the data
/// provider has not configured an analysis rule on the configured table behind the
/// relation, so nothing may be analysed on it yet.
pub const SCHEMA_ANALYSIS_RULE_MISSING_MESSAGE: &str =
    "The configured table behind this schema has no analysis rule, so no analysis of it \
     is configured.";

/// The explanation [`SCHEMA_STATUS_REASON_COLLABORATION_ANALYSIS_RULE_NOT_CONFIGURED`]
/// carries: the configured table association that contributes the relation holds no
/// analysis rule of this type, so the collaboration-side half of the rule is missing.
pub const SCHEMA_COLLABORATION_ANALYSIS_RULE_NOT_CONFIGURED_MESSAGE: &str =
    "The configured table association that contributes this schema has no analysis rule \
     of this type configured for the collaboration.";

impl ComAmazonawsCleanroomsAwsbastioncontrolplaneservicelambdaState {
    /// The configured table association that contributes the relation `name` to
    /// `collaboration_id`, if the collaboration holds one.
    ///
    /// A relation reaches a collaboration through a membership IN that collaboration, so
    /// the association is resolved through its membership rather than through the
    /// `(membership, alias)` index: a name is unique within one membership, and the
    /// collaboration is what the caller addressed. Associations are visited in creation
    /// order, so two memberships that contributed one name resolve to the relation that
    /// reached the collaboration first, whichever order the maps happen to iterate in.
    pub fn schema_source(
        &self,
        collaboration_id: &str,
        name: &str,
    ) -> Option<&ConfiguredTableAssociation> {
        self.configured_table_association_order
            .iter()
            .filter_map(|id| self.configured_table_associations.get(id))
            .find(|association| {
                association.name == name
                    && self
                        .memberships
                        .get(&association.membership_id)
                        .is_some_and(|membership| membership.collaboration_id == collaboration_id)
            })
    }

    /// The account that holds the membership `membership_id`, as the membership index
    /// records it: that index is keyed by `(collaboration id, account id)`, so the owner
    /// of a membership is read from stored state rather than assumed. `None` when the
    /// store holds no membership under that id.
    pub fn membership_owner(&self, membership_id: &str) -> Option<&str> {
        self.membership_by_collaboration
            .iter()
            .find(|(_, id)| id.as_str() == membership_id)
            .map(|((_, account_id), _)| account_id.as_str())
    }

    /// The modelled `Schema` for the relation `association` contributes to `record`.
    ///
    /// Every member is read from the two stored records: the relation's identity and its
    /// timestamps from the ASSOCIATION (the relation exists because that association
    /// does), the columns, the analysis methods and the analysis rule types from the
    /// CONFIGURED TABLE the association refers to, and the collaboration's own identity
    /// from `record`. `region` is the one the request was signed for, so the schema's
    /// `resourceArn` is minted in the same region as the association it names.
    pub fn schema_view(
        &self,
        record: &CollaborationRecord,
        association: &ConfiguredTableAssociation,
        region: &str,
    ) -> Schema {
        // An association refers to one configured table; a table the store no longer
        // holds contributes no columns and no analysis rules rather than making the
        // relation unreadable.
        let table = self.configured_tables.get(&association.configured_table_id);
        Schema {
            columns: schema_columns(table),
            // "The partition keys for the dataset underlying this schema": a partition
            // key is a property of that dataset's catalog entry, which no Clean Rooms
            // request declares to this service, so the relation reports none.
            partition_keys: Vec::new(),
            analysis_rule_types: table
                .map(|table| table.analysis_rule_types.clone())
                .unwrap_or_default(),
            analysis_method: table.map(|table| table.analysis_method.clone()),
            selected_analysis_methods: table
                .and_then(|table| table.selected_analysis_methods.clone()),
            // "The unique account ID for the Amazon Web Services account that owns the
            // schema": the data provider is the account holding the membership the
            // association was created under, which is what the membership index records.
            creator_account_id: match self.membership_owner(&association.membership_id) {
                Some(account_id) => account_id.to_string(),
                None => caller_account_id().to_string(),
            },
            name: association.name.clone(),
            collaboration_id: record.id.clone(),
            collaboration_arn: record.arn.clone(),
            // `Schema$description` is required while
            // `ConfiguredTableAssociation$description` is not, so an association created
            // without one contributes a relation with an empty description.
            description: association.description.clone().unwrap_or_default(),
            create_time: association.create_time,
            update_time: association.update_time,
            r#type: SCHEMA_TYPE_TABLE.to_string(),
            schema_status_details: self.schema_status_details(association, table),
            resource_arn: Some(schema_resource_arn(
                region,
                &association.membership_id,
                &association.id,
            )),
            schema_type_properties: Some(SchemaTypeProperties::configuredTableAssociation(
                ConfiguredTableAssociationSchemaTypeProperties {
                    configured_table_association_id: association.id.clone(),
                },
            )),
        }
    }

    /// The `schemaStatusDetails` a relation reports: one entry per analysis rule type the
    /// configured table holds a rule for, and one bare entry for a table holding none.
    ///
    /// An analysis rule type is READY for a relation once BOTH halves of that rule exist:
    /// the data provider's rule on the configured table, and the association's own rule
    /// of the same type, which is what the association contributes to the collaboration.
    /// A missing half is the reason the entry is NOT_READY, so a caller polling a relation
    /// it has just contributed reads which half it still owes.
    fn schema_status_details(
        &self,
        association: &ConfiguredTableAssociation,
        table: Option<&ConfiguredTable>,
    ) -> Vec<SchemaStatusDetail> {
        let held: &[String] = table
            .map(|table| table.analysis_rule_types.as_slice())
            .unwrap_or_default();
        if held.is_empty() {
            return vec![SchemaStatusDetail {
                status: SCHEMA_STATUS_NOT_READY.to_string(),
                reasons: Some(vec![SchemaStatusReason {
                    code: SCHEMA_STATUS_REASON_ANALYSIS_RULE_MISSING.to_string(),
                    message: SCHEMA_ANALYSIS_RULE_MISSING_MESSAGE.to_string(),
                }]),
                // No rule type has been evaluated, because the table holds none.
                analysis_rule_type: None,
                configurations: None,
                analysis_type: ANALYSIS_TYPE_DIRECT_ANALYSIS.to_string(),
            }];
        }
        held.iter()
            .map(|analysis_rule_type| {
                let collaboration_rule = self
                    .configured_table_association_analysis_rule(&association.id, analysis_rule_type)
                    .is_some();
                SchemaStatusDetail {
                    status: if collaboration_rule {
                        SCHEMA_STATUS_READY.to_string()
                    } else {
                        SCHEMA_STATUS_NOT_READY.to_string()
                    },
                    reasons: (!collaboration_rule).then(|| {
                        vec![SchemaStatusReason {
                            code: SCHEMA_STATUS_REASON_COLLABORATION_ANALYSIS_RULE_NOT_CONFIGURED
                                .to_string(),
                            message: SCHEMA_COLLABORATION_ANALYSIS_RULE_NOT_CONFIGURED_MESSAGE
                                .to_string(),
                        }]
                    }),
                    analysis_rule_type: Some(analysis_rule_type.clone()),
                    configurations: self.schema_configurations(table, analysis_rule_type),
                    analysis_type: ANALYSIS_TYPE_DIRECT_ANALYSIS.to_string(),
                }
            })
            .collect()
    }

    /// "The configuration details of the schema analysis rule for the given type"
    /// (`SchemaStatusDetail$configurations`): the configurations the STORED policy of that
    /// rule type declares, which is differential privacy where the policy configures it.
    fn schema_configurations(
        &self,
        table: Option<&ConfiguredTable>,
        analysis_rule_type: &str,
    ) -> Option<Vec<String>> {
        let rule = self.configured_table_analysis_rule(&table?.id, analysis_rule_type)?;
        let ConfiguredTableAnalysisRulePolicy::V1(policy) = &rule.policy;
        // Of the three policy members, only the custom rule declares a differential
        // privacy configuration (`AnalysisRuleCustom$differentialPrivacy`).
        let ConfiguredTableAnalysisRulePolicyV1::Custom(custom) = policy else {
            return None;
        };
        custom
            .differential_privacy
            .as_ref()
            .map(|_| vec![SCHEMA_CONFIGURATION_DIFFERENTIAL_PRIVACY.to_string()])
    }
}

/// `arn:aws:cleanrooms:<region>:<account>:membership/<membership id>/configuredtableassociation/<association id>`,
/// the form the model's `SchemaResourceArn` pattern declares.
///
/// It is NOT the association's own `ConfiguredTableAssociationArn`
/// (`configuredtableassociation/<membership id>/<id>`): `Schema$resourceArn` names the
/// resource the relation comes from in the schema plane's own form, so it is minted here
/// rather than copied off the association.
pub fn schema_resource_arn(region: &str, membership_id: &str, association_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("membership/{membership_id}/configuredtableassociation/{association_id}"),
    )
}

/// The columns a relation reports: one per column the configured table allows "within
/// collaborations" (`ConfiguredTable$allowedColumns`), in the order that table declares
/// them, each with the type its stored table reference declares for it (see
/// [`UNDECLARED_COLUMN_TYPE`] for a column no reference declares a type for).
fn schema_columns(table: Option<&ConfiguredTable>) -> Vec<Column> {
    let Some(table) = table else {
        return Vec::new();
    };
    let declared = declared_column_types(&table.table_reference);
    table
        .allowed_columns
        .iter()
        .map(|name| Column {
            name: name.clone(),
            r#type: declared
                .get(name.as_str())
                .map(|column_type| (*column_type).to_string())
                .unwrap_or_else(|| UNDECLARED_COLUMN_TYPE.to_string()),
        })
        .collect()
}

/// The `(column name, column type)` pairs the stored table reference itself declares.
///
/// `SnowflakeTableReference$tableSchema` is the one member of the `TableReference` union
/// that carries the underlying table's column types; a Glue, Athena or Databricks
/// reference names a catalog entry whose columns this service is told nothing about, so
/// it declares none.
fn declared_column_types(reference: &TableReference) -> HashMap<&str, &str> {
    match reference {
        TableReference::Snowflake(snowflake) => match &snowflake.table_schema {
            SnowflakeTableSchema::V1(columns) => columns
                .iter()
                .map(|column| (column.column_name.as_str(), column.column_type.as_str()))
                .collect(),
        },
        TableReference::Glue(_) | TableReference::Athena(_) | TableReference::Databricks(_) => {
            HashMap::new()
        }
    }
}
