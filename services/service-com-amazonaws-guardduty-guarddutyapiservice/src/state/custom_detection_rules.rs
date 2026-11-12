// SPDX-License-Identifier: CC0-1.0

//! Custom detection rules: the catalogue GuardDuty PUBLISHES, and the
//! associations and organization configurations an account makes against one.
//!
//! A custom detection rule is published by GuardDuty: the API models no create,
//! update or delete for one, only the associations an account makes against it,
//! so the catalogue here is the service's own and the same for every caller.

use super::*;

/// The wire name of the `RuleId` member, as a request carries it — a `jsonName`
/// `ruleId` on `CreateCustomDetectionRuleAssociation` and the URI label on
/// `GetCustomDetectionRule`
/// (`smithy_query(operation="GetCustomDetectionRule,CreateCustomDetectionRuleAssociation")`).
pub const RULE_ID_MEMBER: &str = "ruleId";

/// The wire name of `ListCustomDetectionRules`'s filter list: its `Filters` member
/// has `jsonName` `filters` (`smithy_query(operation="ListCustomDetectionRules")`).
pub const FILTERS_MEMBER: &str = "filters";

/// `RuleLanguage`'s only member (`smithy_query(shape="RuleLanguage")`): the
/// language every published rule expresses its detection logic in.
pub const RULE_LANGUAGE_SQL: &str = "SQL";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RULE_LANGUAGE_VALUES,
    RULE_LANGUAGE_SQL
));

/// `RuleSchema`'s only member (`smithy_query(shape="RuleSchema")`): the schema a
/// rule over CloudTrail management events is written against. A rule over another
/// data source has no schema member to report, so it reports none.
pub const RULE_SCHEMA_CLOUD_TRAIL: &str = "CloudTrail";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::RULE_SCHEMA_VALUES,
    RULE_SCHEMA_CLOUD_TRAIL
));

/// The two `DetectionRuleDataSource` members — "The data source that the rule
/// analyzes" (`smithy_query(shape="DetectionRuleDataSource")`).
pub const DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL: &str = "CloudTrailManagementEvent";
/// See [`DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL`].
pub const DETECTION_RULE_DATA_SOURCE_RUNTIME: &str = "RuntimeMonitoring";

/// The `DetectionRuleSeverity` members (`smithy_query(shape="DetectionRuleSeverity")`).
pub const DETECTION_RULE_SEVERITY_CRITICAL: &str = "CRITICAL";
/// See [`DETECTION_RULE_SEVERITY_CRITICAL`].
pub const DETECTION_RULE_SEVERITY_HIGH: &str = "HIGH";
/// See [`DETECTION_RULE_SEVERITY_CRITICAL`].
pub const DETECTION_RULE_SEVERITY_MEDIUM: &str = "MEDIUM";
/// See [`DETECTION_RULE_SEVERITY_CRITICAL`].
pub const DETECTION_RULE_SEVERITY_LOW: &str = "LOW";

/// The `DetectionRuleFilterCondition` members — "The condition to apply to the
/// filter. For example, `EQUALS` or `CONTAINS`"
/// (`smithy_query(query="doc",shape="DetectionRuleFilter",member="Condition")`).
/// [`FILTER_CONDITION_EQUALS`] is what a filter that names no condition asks for:
/// a filter is a selection by value, and the exact value is the narrower reading.
pub const FILTER_CONDITION_EQUALS: &str = "EQUALS";
/// See [`FILTER_CONDITION_EQUALS`].
pub const FILTER_CONDITION_CONTAINS: &str = "CONTAINS";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::DETECTION_RULE_FILTER_CONDITION_VALUES,
    FILTER_CONDITION_EQUALS
));
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::DETECTION_RULE_FILTER_CONDITION_VALUES,
    FILTER_CONDITION_CONTAINS
));

/// Every `FilterFieldName` member — the rule fields a `DetectionRuleFilter` may
/// name (`smithy_query(shape="FilterFieldName")`). Each one is the `jsonName` of a
/// `RuleSummary` member, which is why [`CustomDetectionRuleRecord::field`] resolves
/// them straight onto the record.
pub const DETECTION_RULE_FILTER_FIELDS: &[&str] =
    crate::ops::types::shared_types::FILTER_FIELD_NAME_VALUES;

/// The platforms a Runtime Monitoring rule can run on, as
/// `DetectionRuleRuntimePlatforms` names them: "The presence of a platform member
/// indicates that the rule is supported on that platform"
/// (`smithy_query(query="doc",shape="DetectionRuleRuntimeMonitoringDetails",member="Platforms")`).
pub const RUNTIME_PLATFORM_EKS: &str = "EKS";
/// See [`RUNTIME_PLATFORM_EKS`].
pub const RUNTIME_PLATFORM_EC2: &str = "EC2";
/// See [`RUNTIME_PLATFORM_EKS`].
pub const RUNTIME_PLATFORM_ECS_FARGATE: &str = "ECS_FARGATE";

/// The message a read or an association of a rule the catalogue does not publish
/// answers with.
///
/// A rule is addressed by its `RuleId`, which is the caller's own request value and
/// the only thing that identifies which rule was missing, so GuardDuty echoes it
/// after a fixed prefix: a caller resolving a batch of rule ids can tell WHICH one
/// the catalogue does not carry.
///
/// `RuleId` carries `@pattern` `^[a-z0-9]+(-[a-z0-9]+)*$` and `@length` 1..100
/// (`smithy_query(operation="GetCustomDetectionRule")`), both refused at the request
/// boundary, so the value interpolated here is a short lower-case identifier and
/// never arbitrary caller text.
pub fn custom_detection_rule_not_found_message(rule_id: &str) -> String {
    format!("Rule not found: {rule_id}")
}

/// The FIXED resource path every custom-detection-rule ARN opens with: the
/// resource type the ARN's own shape names (`DetectionRuleArn`), qualified by the
/// `custom` family the rule belongs to, so a custom rule's ARN is never the ARN of
/// a rule of another family.
///
/// One constant for the rule and for the associations hanging off it
/// ([`custom_detection_rule_association_arn`]), so no operation can report a path
/// another one does not.
pub(super) const CUSTOM_DETECTION_RULE_ARN_PATH: &str = "detection-rule/custom";

/// The account segment of an ARN whose resource Amazon Web Services itself owns.
///
/// `DetectionRuleArn` admits `([0-9]*|aws)` there
/// (`smithy_query(shape="DetectionRuleArn")`): an account-owned resource carries
/// the twelve digits of its account, and one AWS owns carries the literal `aws`.
/// It is the same segment an AWS-managed IAM policy is published under.
pub const AWS_MANAGED_ACCOUNT_SEGMENT: &str = "aws";

/// The ARN of the custom detection rule `rule_id`, as EVERY endpoint reports it.
///
/// A published rule is an AWS-MANAGED catalogue object rather than a resource of
/// the account reading it: it is the same rule in every account and every Region,
/// nothing in the API creates or changes one, so its ARN carries neither the
/// caller's account nor the Region serving the read. Both segments the ARN's own
/// shape makes variable are therefore fixed here — the Region segment is EMPTY and
/// the account segment is [`AWS_MANAGED_ACCOUNT_SEGMENT`], the form
/// `DetectionRuleArn`'s pattern admits for exactly this case — so a client that
/// keys the catalogue by ARN reads the same key AWS publishes.
///
/// Derived, never stored: the rule's own id is the only variable part, so a read
/// cannot report an ARN that disagrees with the id it was asked about. The resource
/// path is [`CUSTOM_DETECTION_RULE_ARN_PATH`] followed by the rule's own id.
///
/// An association against the rule is the ACCOUNT's own resource and keeps the
/// account-bearing form ([`custom_detection_rule_association_arn`]).
pub fn custom_detection_rule_arn(rule_id: &str) -> String {
    format!(
        "arn:{PARTITION}:{GUARDDUTY_ARN_SERVICE}::{AWS_MANAGED_ACCOUNT_SEGMENT}:{CUSTOM_DETECTION_RULE_ARN_PATH}/{rule_id}"
    )
}

// ---------------------------------------------------------------------------
// Custom detection rule associations and organization configurations
//
// A rule is published by GuardDuty; what an ACCOUNT owns is its association with
// one, and — for an organization's delegated administrator — the organization
// configuration that says which member accounts the rule applies to. Both are
// identified by the `(RuleId, Mode)` pair they are created for.
// ---------------------------------------------------------------------------

/// The wire name of the `Mode` member — `jsonName` `mode` on the body-bound
/// operations and the query parameter `mode` on the rest
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation,GetCustomDetectionRuleOrgConfiguration")`).
pub const MODE_MEMBER: &str = "mode";

/// The wire name of the `AssociationId` member — the URI label
/// `AssociationId`, rendered `associationId` in a response body
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation")`).
pub const ASSOCIATION_ID_MEMBER: &str = "associationId";

/// `DetectionRuleConfigurationStatus`: the organization configuration is in
/// effect. One of `ACTIVE | PROCESSING | FAILED`
/// (`smithy_query(shape="DetectionRuleConfigurationStatus")`).
pub const DETECTION_RULE_CONFIGURATION_ACTIVE: &str = "ACTIVE";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::DETECTION_RULE_CONFIGURATION_STATUS_VALUES,
    DETECTION_RULE_CONFIGURATION_ACTIVE
));

/// `DetectionRuleConfigurationStatus`: the organization configuration has been
/// accepted and is being rolled out to the organization's accounts. See
/// [`DETECTION_RULE_CONFIGURATION_ACTIVE`] for the enum.
pub const DETECTION_RULE_CONFIGURATION_PROCESSING: &str = "PROCESSING";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::DETECTION_RULE_CONFIGURATION_STATUS_VALUES,
    DETECTION_RULE_CONFIGURATION_PROCESSING
));

/// How long an organization configuration stays
/// [`DETECTION_RULE_CONFIGURATION_PROCESSING`] before the CLOCK owes it
/// [`DETECTION_RULE_CONFIGURATION_ACTIVE`].
///
/// GuardDuty documents no duration for rolling a rule configuration out to an
/// organization, so this emulator CHOOSES one — the same span
/// [`IP_SET_TRANSITION_MILLIS`] uses, and non-zero for the same reason: a caller
/// that reads the configuration back immediately after creating it still observes
/// the transient `PROCESSING` status.
pub const ORG_CONFIGURATION_TRANSITION_MILLIS: i64 = 500;

/// How many Regions this store will hold custom-detection-rule associations and
/// organization configurations for.
///
/// Per Region the records are bounded by construction — one association per
/// published rule, and one organization configuration per published rule per
/// `AssociationMode` member — but the Region is read from the request's
/// credential scope ([`request_region`]), so the caller chooses the partition key:
/// without a ceiling on the number of partitions a caller signing for invented
/// Region names could make the process retain a full set per distinct Region
/// string. Emulator-chosen, like [`MAX_MALWARE_PROTECTION_PLAN_REGIONS`], and
/// enforced before either create writes
/// ([`ComAmazonawsGuarddutyGuarddutyapiserviceState::detection_rule_regions_exhausted`]).
pub const MAX_DETECTION_RULE_REGIONS: usize = 50;

/// The message a create that would partition the custom-detection-rule stores
/// into one Region more than [`MAX_DETECTION_RULE_REGIONS`] is refused with.
pub const DETECTION_RULE_REGION_LIMIT_MESSAGE: &str =
    "The request is rejected because the maximum number of custom detection rule associations has been reached.";

/// The message a read, an update or a delete of an association the account does not
/// hold answers with.
///
/// An association is addressed by its `AssociationId`, which is the caller's own
/// request value and the only thing that says WHICH association was missing, so
/// GuardDuty echoes it after a fixed prefix — the same shape it answers a missing
/// rule with ([`custom_detection_rule_not_found_message`]).
///
/// `AssociationId` carries `@pattern` `^[a-zA-Z0-9_-]{1,64}$` and `@length` 1..64
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation")`), both refused at
/// the request boundary, so the value interpolated here is a short identifier and
/// never arbitrary caller text.
pub fn detection_rule_association_not_found_message(association_id: &str) -> String {
    format!("Association not found: {association_id}")
}

/// The message a second association for a rule the account has already associated
/// is refused with — the `ConflictException` the create models. Creating is not
/// updating: `UpdateCustomDetectionRuleAssociation` is the operation that changes
/// an existing association's mode, so the refusal holds whatever mode either call
/// names.
///
/// The conflict is about ONE rule, named by the `RuleId` the request carried, so
/// GuardDuty echoes that id after a fixed prefix: a caller associating a batch of
/// rules can tell which of them was already associated. The id is bounded by the
/// member's own pattern and length, as it is for
/// [`custom_detection_rule_not_found_message`].
pub fn detection_rule_association_conflict_message(rule_id: &str) -> String {
    format!("Association already exists for rule {rule_id}")
}

/// The message a read of an organization configuration the account has not
/// created answers with.
pub fn detection_rule_org_configuration_not_found_message() -> String {
    format!(
        "The request is rejected because the input {RULE_ID_MEMBER} and {MODE_MEMBER} do not refer to an organization configuration of the current account."
    )
}

/// The message a second organization configuration for a `(RuleId, Mode)` pair
/// the account has already configured is refused with — the `ConflictException`
/// the create models.
pub fn detection_rule_org_configuration_conflict_message() -> String {
    format!(
        "The request is rejected because the current account already has an organization configuration for the given {RULE_ID_MEMBER} and {MODE_MEMBER}."
    )
}

/// The message an update of a rule the account holds no organization
/// configuration for answers with.
///
/// It names only `RuleId`, because that is what the update identifies the
/// configuration by: `Mode` is one of the values the update SETS, not a selector
/// (`smithy_query(query="doc",shape="UpdateCustomDetectionRuleOrgConfiguration")`
/// — "including the mode and include/exclude account lists").
pub fn detection_rule_org_configuration_rule_not_found_message() -> String {
    format!(
        "The request is rejected because the input {RULE_ID_MEMBER} does not refer to a custom detection rule with an organization configuration of the current account."
    )
}

/// The wire names of the two account lists an organization configuration's scope is
/// carried by (`jsonName` `includeAccountIds` / `excludeAccountIds` —
/// `smithy_query(shape="DetectionRuleOrgConfiguration")`), so a refusal that names
/// the list it was handed spells it the way the request did.
pub const INCLUDE_ACCOUNT_IDS_MEMBER: &str = "includeAccountIds";
/// See [`INCLUDE_ACCOUNT_IDS_MEMBER`].
pub const EXCLUDE_ACCOUNT_IDS_MEMBER: &str = "excludeAccountIds";

/// The message a request naming both `IncludeAccountIds` and `ExcludeAccountIds`
/// is refused with: the two are "mutually exclusive"
/// (`smithy_query(query="doc",shape="DetectionRuleOrgConfiguration",member="IncludeAccountIds")`).
pub const ORG_CONFIGURATION_ACCOUNT_SCOPE_MESSAGE: &str =
    "The request is rejected because includeAccountIds and excludeAccountIds are mutually exclusive.";

/// The ARN of the association `association_id` of rule `rule_id`, as `region`'s
/// endpoint reports it.
///
/// Derived, never stored: the ids and the Region serving the read are the only
/// variable parts, so a read cannot report an ARN that disagrees with the ids it
/// was asked about. The resource path EXTENDS the rule's own — the same
/// [`CUSTOM_DETECTION_RULE_ARN_PATH`] plus the rule's id — because an association
/// belongs to a rule.
///
/// Unlike the rule's own ARN ([`custom_detection_rule_arn`]) this one is
/// ACCOUNT-SCOPED: an association is a resource the account created in one Region,
/// so it carries that Region and the account that owns it, which is the form
/// `::engine::emulator_arn` builds.
pub fn custom_detection_rule_association_arn(
    region: &str,
    rule_id: &str,
    association_id: &str,
) -> String {
    ::engine::emulator_arn(
        GUARDDUTY_ARN_SERVICE,
        region,
        &format!("{CUSTOM_DETECTION_RULE_ARN_PATH}/{rule_id}/association/{association_id}"),
    )
}

/// `AssociationMode`: the association evaluates the rule without producing
/// findings — one of `LIVE | DRY_RUN`
/// (`smithy_query(shape="AssociationMode")`).
pub const ASSOCIATION_MODE_DRY_RUN: &str = "DRY_RUN";
const _: () = assert!(enum_carries(
    crate::ops::types::shared_types::ASSOCIATION_MODE_VALUES,
    ASSOCIATION_MODE_DRY_RUN
));

/// How long after it enters [`ASSOCIATION_MODE_DRY_RUN`] an association stops
/// evaluating its rule — fourteen days, the dry-run trial window GuardDuty gives
/// an association it does not keep evaluating for ever.
///
/// Not a wait a caller asks for and not a status the clock moves: it is the
/// instant `ExpiresAt` reports, so a caller can tell when a temporary association
/// stops and schedule its renewal.
pub const DRY_RUN_ASSOCIATION_LIFETIME_MILLIS: i64 = 14 * 24 * 60 * 60 * 1_000;

/// When an association that entered `mode` at `since_millis` expires, or `None`
/// for a mode that does not expire.
///
/// `ExpiresAt` is "the timestamp when the association expires"
/// (`smithy_query(shape="AssociationDetail")`) and only a DRY_RUN association has
/// one: a `LIVE` association evaluates its rule until the account deletes it, so
/// there is no instant to report for it and the optional member stays absent.
/// Stored on the record ([`DetectionRuleAssociationRecord::expires_at_millis`])
/// rather than recomputed per read, so every read of one association reports the
/// same instant.
pub fn dry_run_association_expiry(mode: &str, since_millis: i64) -> Option<i64> {
    (mode == ASSOCIATION_MODE_DRY_RUN)
        .then(|| since_millis.saturating_add(DRY_RUN_ASSOCIATION_LIFETIME_MILLIS))
}

/// One custom detection rule association as the store holds it.
///
/// Timestamps are epoch milliseconds, the single source for the instants anything
/// reports.
#[derive(Debug, Clone)]
pub struct DetectionRuleAssociationRecord {
    /// The minted `AssociationId`: `len 1..64`, `pattern ^[a-zA-Z0-9_-]{1,64}$`
    /// (`smithy_query(shape="AssociationDetail")`), which the 32 hexadecimal
    /// characters [`ComAmazonawsGuarddutyGuarddutyapiserviceState::mint_id`] mints
    /// satisfy.
    pub association_id: String,
    /// The Region whose endpoint made this association, and the only one that
    /// reports it.
    pub region: String,
    /// The `RuleId` of the published rule this association binds.
    pub rule_id: String,
    /// `AssociationDetail.AccountId` — "the Amazon Web Services account ID
    /// associated with this rule association", which is the account the emulator
    /// serves and NOT caller input.
    pub account_id: String,
    /// `AssociationMode`: `LIVE` or `DRY_RUN`.
    pub mode: String,
    /// The tags of the association resource.
    pub tags: BTreeMap<String, String>,
    /// The `ClientToken` the create carried, so a replay answers with this same
    /// association instead of making a second one.
    pub client_token: Option<String>,
    /// When this association stops evaluating its rule, for a mode that expires —
    /// [`dry_run_association_expiry`] of the instant it entered its current mode.
    /// `None` for a mode that does not expire.
    pub expires_at_millis: Option<i64>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
}

/// One custom detection rule organization configuration as the store holds it.
#[derive(Debug, Clone)]
pub struct DetectionRuleOrgConfigurationRecord {
    /// The Region whose endpoint created this configuration, and the only one that
    /// reports it.
    pub region: String,
    /// The `RuleId` of the published rule this configuration is for.
    pub rule_id: String,
    /// `AssociationMode`: `LIVE` or `DRY_RUN`.
    pub mode: String,
    /// The member accounts the configuration includes, empty when it names none.
    pub include_account_ids: Vec<String>,
    /// The member accounts the configuration excludes, empty when it names none.
    /// Mutually exclusive with [`Self::include_account_ids`], which the create
    /// refuses before it writes.
    pub exclude_account_ids: Vec<String>,
    /// When the clock owes this configuration
    /// [`DETECTION_RULE_CONFIGURATION_ACTIVE`]. Absent once nothing is owed.
    pub status_due_millis: Option<i64>,
    /// The `ClientToken` the create carried.
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
}

impl DetectionRuleOrgConfigurationRecord {
    /// This configuration's `Status` at `now`.
    ///
    /// DERIVED from the instant the create recorded rather than stored: the move
    /// out of [`DETECTION_RULE_CONFIGURATION_PROCESSING`] was owed at
    /// [`Self::status_due_millis`] whether or not anybody called, so a read
    /// reports it without writing anything — which is what keeps a report from
    /// changing what a later caller sees.
    pub fn status(&self, now: i64) -> &'static str {
        match self.status_due_millis {
            Some(due) if now < due => DETECTION_RULE_CONFIGURATION_PROCESSING,
            _ => DETECTION_RULE_CONFIGURATION_ACTIVE,
        }
    }
}

/// The wire `AssociationDetail` for a stored association.
///
/// One builder for the create that mints an association and the read that reports
/// it, so the two cannot disagree — the same reason [`member_view`] is shared. The
/// `Arn` is DERIVED from the record's own ids and `region`, never stored.
///
/// `ExpiresAt` is the shape's only optional member — "the timestamp when the
/// association expires" (`smithy_query(shape="AssociationDetail")`) — and is read
/// off the record, so it is reported for an association whose mode expires
/// ([`dry_run_association_expiry`]) and absent for one that does not.
pub fn detection_rule_association_view(
    record: &DetectionRuleAssociationRecord,
) -> crate::ops::types::AssociationDetail {
    crate::ops::types::AssociationDetail {
        association_id: record.association_id.clone(),
        arn: custom_detection_rule_association_arn(
            &record.region,
            &record.rule_id,
            &record.association_id,
        ),
        rule_id: record.rule_id.clone(),
        account_id: record.account_id.clone(),
        mode: record.mode.clone(),
        created_at: ::aws_smithy_types::DateTime::from_millis(record.created_at_millis),
        updated_at: ::aws_smithy_types::DateTime::from_millis(record.updated_at_millis),
        expires_at: record
            .expires_at_millis
            .map(::aws_smithy_types::DateTime::from_millis),
    }
}

/// The wire `AssociationSummary` for a stored association — the entry a listing
/// reports it as.
///
/// `AssociationSummary` is `AssociationDetail` without `AccountId`
/// (`smithy_query(shape="AssociationSummary,AssociationDetail")`), so it is built
/// BY DROPPING that member from [`detection_rule_association_view`] rather than by
/// reading the record a second time: one projection then serves the create, the
/// read and every listing, and the identity a summary advertises
/// (`AssociationId`) and the one its own `Arn` names cannot be derived from
/// different values.
pub fn detection_rule_association_summary(
    record: &DetectionRuleAssociationRecord,
) -> crate::ops::types::AssociationSummary {
    let detail = detection_rule_association_view(record);
    crate::ops::types::AssociationSummary {
        association_id: detail.association_id,
        arn: detail.arn,
        rule_id: detail.rule_id,
        mode: detail.mode,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
        expires_at: detail.expires_at,
    }
}

/// The wire `DetectionRuleOrgConfiguration` for a stored organization
/// configuration, as of `now`.
///
/// Shared by the create and the read for the same reason
/// [`detection_rule_association_view`] is. `Status` comes from
/// [`DetectionRuleOrgConfigurationRecord::status`], so both report the same
/// derivation of the clock; `StatusReason` is "the reason for the current
/// configuration status" and a configuration that is proceeding normally has
/// none, so it stays absent. `ExpiresAt` is absent for the same reason an
/// association's is.
pub fn detection_rule_org_configuration_view(
    record: &DetectionRuleOrgConfigurationRecord,
    now: i64,
) -> crate::ops::types::DetectionRuleOrgConfiguration {
    crate::ops::types::DetectionRuleOrgConfiguration {
        rule_id: record.rule_id.clone(),
        mode: record.mode.clone(),
        status: record.status(now).to_string(),
        status_reason: None,
        include_account_ids: record.include_account_ids.clone(),
        exclude_account_ids: record.exclude_account_ids.clone(),
        created_at: ::aws_smithy_types::DateTime::from_millis(record.created_at_millis),
        updated_at: ::aws_smithy_types::DateTime::from_millis(record.updated_at_millis),
        expires_at: None,
    }
}

/// One custom detection rule as the store holds it.
///
/// `Language`/`Schema`/`UpdatedAt` are the model's optional members and are held as
/// `Option`, so a rule that has none reports none rather than an invented value.
#[derive(Debug, Clone)]
pub struct CustomDetectionRuleRecord {
    /// `RuleId`: `len 1..100`, `pattern ^[a-z0-9]+(-[a-z0-9]+)*$`
    /// (`smithy_query(shape="RuleId")`).
    pub rule_id: String,
    /// `RuleSummary.Name` — the rule's display name.
    pub name: String,
    /// `RuleSummary.Description` — "A description of what the rule detects".
    pub description: String,
    /// One of the [`DETECTION_RULE_SEVERITY_CRITICAL`] family.
    pub severity: String,
    /// [`DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL`] or
    /// [`DETECTION_RULE_DATA_SOURCE_RUNTIME`].
    pub data_source: String,
    /// The MITRE ATT&CK tactic the rule detects.
    pub tactic: String,
    /// The MITRE ATT&CK technique the rule detects.
    pub technique: String,
    /// The Amazon Web Services service the rule is about.
    pub service: String,
    /// `RuleDefinition.Expression` — the rule's detection logic, which only
    /// `GetCustomDetectionRule` reports (`RuleSummary` carries no definition).
    pub expression: String,
    /// The language the expression is written in, or `None` when the rule declares
    /// none.
    pub language: Option<String>,
    /// The schema the expression is written against, or `None` when the rule
    /// declares none.
    pub schema: Option<String>,
    /// The Runtime Monitoring platforms the rule is supported on, empty for a rule
    /// that is not a Runtime Monitoring rule.
    pub runtime_platforms: Vec<String>,
    /// When the store first held this rule.
    pub created_at_millis: i64,
    /// When it last changed, or `None` for a rule nothing has changed since it was
    /// published — `UpdatedAt` is optional on both `RuleSummary` and `RuleDetail`.
    pub updated_at_millis: Option<i64>,
}

impl CustomDetectionRuleRecord {
    /// The value of the rule field a `DetectionRuleFilter` names, by the field's
    /// own wire name.
    ///
    /// The `FilterFieldName` members are the `jsonName`s of `RuleSummary`'s own
    /// members, so a filter selects on the value a list REPORTS for that member.
    /// A name outside [`DETECTION_RULE_FILTER_FIELDS`] resolves to nothing, so a
    /// filter over a field no rule carries selects no rule.
    pub fn field(&self, field: &str) -> Option<&str> {
        match field {
            "name" => Some(&self.name),
            "description" => Some(&self.description),
            "dataSource" => Some(&self.data_source),
            "severity" => Some(&self.severity),
            "tactic" => Some(&self.tactic),
            "technique" => Some(&self.technique),
            "service" => Some(&self.service),
            _ => None,
        }
    }
}

// Every field name the model lists is one `field` above resolves: the arms there
// and the enum's members cannot drift apart without failing the build, so a member
// the model gains is a build error rather than a filter that silently selects
// nothing.
const _: () = {
    let mut index = 0;
    while index < DETECTION_RULE_FILTER_FIELDS.len() {
        assert!(enum_carries(
            &[
                "name",
                "description",
                "dataSource",
                "severity",
                "tactic",
                "technique",
                "service"
            ],
            DETECTION_RULE_FILTER_FIELDS[index]
        ));
        index += 1;
    }
    assert!(DETECTION_RULE_FILTER_FIELDS.len() == 7);
};

/// GuardDuty's catalogue of custom detection rules, held by this service's store.
///
/// Opaque on purpose: the catalogue is READ by every operation that resolves a
/// rule, and written by nothing, so it hands out `&`-references and no way to
/// insert. That is what makes "a rule a caller cannot create" also "a rule a
/// caller cannot change".
#[derive(Debug)]
pub struct CustomDetectionRuleCatalog {
    /// Every published rule, in `RuleId` order — the total order a page token names
    /// a position in.
    pub(super) rules: Vec<CustomDetectionRuleRecord>,
}

impl Default for CustomDetectionRuleCatalog {
    /// Materialise the published catalogue, every rule carrying the publication
    /// instants the seed row names.
    ///
    /// Nothing here reads the clock: a published rule's `CreatedAt` and `UpdatedAt`
    /// are properties of the RULE — when GuardDuty published it and when it last
    /// revised it — not of the process serving the read, so they are carried by the
    /// catalogue itself and are the same on every run. A caller that caches one
    /// entry, or compares `UpdatedAt` against what it cached to detect a catalogue
    /// revision, therefore reads the same answer after a restart as before it.
    fn default() -> Self {
        let mut rules: Vec<CustomDetectionRuleRecord> = PUBLISHED_CUSTOM_DETECTION_RULES
            .iter()
            .map(PublishedRule::record)
            .collect();
        rules.sort_by(|left, right| left.rule_id.cmp(&right.rule_id));
        Self { rules }
    }
}

impl CustomDetectionRuleCatalog {
    /// Every published rule, in `RuleId` order.
    pub fn rules(&self) -> &[CustomDetectionRuleRecord] {
        &self.rules
    }

    /// The published rule `rule_id` names, if the catalogue carries it.
    ///
    /// Matching is EXACT: a rule id is an identifier, so a near miss is a different
    /// string and not a rule. A linear scan over a fixed table of single-digit
    /// length: the catalogue has no key a map would index it by faster.
    pub fn rule(&self, rule_id: &str) -> Option<&CustomDetectionRuleRecord> {
        self.rules.iter().find(|rule| rule.rule_id == rule_id)
    }
}

/// One entry of the published catalogue.
///
/// `Language` and `Schema` are not members here: every published rule expresses its
/// logic in the one `RuleLanguage` member the model carries, and the one
/// `RuleSchema` member is `CloudTrail`, so both follow from the data source — see
/// [`Self::record`].
pub(super) struct PublishedRule {
    pub(super) rule_id: &'static str,
    /// `RuleSummary.Name` — "the display name of the rule", which for a published
    /// rule is the name GuardDuty publishes it under rather than prose about it:
    /// one unspaced word per word of the rule's id, each capitalised
    /// ([`is_published_rule_name_of`] holds the two to each other).
    pub(super) name: &'static str,
    pub(super) description: &'static str,
    pub(super) severity: &'static str,
    pub(super) data_source: &'static str,
    pub(super) tactic: &'static str,
    pub(super) technique: &'static str,
    pub(super) service: &'static str,
    pub(super) expression: &'static str,
    /// The Runtime Monitoring platforms the rule runs on; empty for a rule over
    /// another data source, which has no Runtime Monitoring details to report.
    pub(super) runtime_platforms: &'static [&'static str],
    /// When GuardDuty published this rule, in the milliseconds the store holds
    /// every instant as. A property of the published rule, so it is written here
    /// and never taken from the clock.
    pub(super) published_at_millis: i64,
    /// When GuardDuty last revised this rule, for one it has revised since
    /// publishing it; `None` for one it has not, whose optional `UpdatedAt` is
    /// then absent.
    pub(super) revised_at_millis: Option<i64>,
}

impl PublishedRule {
    /// This entry as a stored record.
    pub(super) fn record(&self) -> CustomDetectionRuleRecord {
        CustomDetectionRuleRecord {
            rule_id: self.rule_id.to_string(),
            name: self.name.to_string(),
            description: self.description.to_string(),
            severity: self.severity.to_string(),
            data_source: self.data_source.to_string(),
            tactic: self.tactic.to_string(),
            technique: self.technique.to_string(),
            service: self.service.to_string(),
            expression: self.expression.to_string(),
            language: Some(RULE_LANGUAGE_SQL.to_string()),
            // `RuleSchema` carries exactly one member, `CloudTrail`, so a rule over
            // CloudTrail management events reports it and a rule over any other
            // data source has no schema the model can name.
            schema: (self.data_source == DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL)
                .then(|| RULE_SCHEMA_CLOUD_TRAIL.to_string()),
            runtime_platforms: self
                .runtime_platforms
                .iter()
                .map(|platform| (*platform).to_string())
                .collect(),
            // Both instants are the PUBLISHED rule's own, read off this row: the
            // instant GuardDuty published it, and the instant it last revised it
            // for a rule it has revised.
            created_at_millis: self.published_at_millis,
            updated_at_millis: self.revised_at_millis,
        }
    }
}

/// Every custom detection rule GuardDuty publishes, as the recorded catalogue
/// reports it.
///
/// The catalogue is the SERVICE's own, the way [`INVESTIGATION_REGIONS`] and the
/// sample-finding type catalogue are: the Smithy model names no rule and binds no
/// operation that creates one, so there is nothing for a caller to add and nothing
/// to read a rule list out of. Each entry's `severity` and `dataSource` are members
/// of the model's own enums (asserted below), each `tactic`/`technique` pair names
/// an MITRE ATT&CK Enterprise tactic and technique, and each `expression` is the
/// SQL that detects it over the entry's data source.
///
/// MEMBERSHIP is the published set and nothing besides: a rule this table does not
/// name is a rule GuardDuty does not publish, and an operation handed such a
/// `ruleId` — a `CreateCustomDetectionRuleAssociation`, say — answers the modelled
/// "no such rule" rather than inventing a rule to match it. A row for a rule AWS
/// does not publish would be worse than a missing one: a client that discovers
/// rules here and associates one would be associating a rule that exists nowhere
/// else.
///
/// Every entry is in the form GuardDuty PUBLISHES a rule in, which the assertions
/// below hold each row to:
///
/// * the `name` is the rule's published name — one unspaced word per word of its
///   id, each capitalised — and not prose about the rule;
/// * the `description` is the one clause that says what the rule detects, opening
///   with the verb GuardDuty's own descriptions open with;
/// * the `technique` opens with the MITRE ATT&CK technique id, then ` - `, then
///   the technique's name;
/// * both instants are the rule's own publication and revision dates, so they do
///   not move when this process restarts.
pub(super) const PUBLISHED_CUSTOM_DETECTION_RULES: &[PublishedRule] = &[
    PublishedRule {
        rule_id: "admin-policy-attached-to-role",
        name: "AdminPolicyAttachedToRole",
        description: "Detects when admin policy is attached to an IAM role",
        severity: DETECTION_RULE_SEVERITY_HIGH,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Privilege Escalation",
        technique: "T1098.001 - Account Manipulation: Additional Cloud Credentials",
        service: "iam",
        expression: "SELECT * FROM cloudtrail WHERE eventSource = 'iam.amazonaws.com' AND eventName IN ('AttachRolePolicy', 'PutRolePolicy') AND requestParameters.policyArn = 'arn:aws:iam::aws:policy/AdministratorAccess'",
        runtime_platforms: &[],
        published_at_millis: CATALOGUE_PUBLISHED_AT_MILLIS,
        revised_at_millis: Some(CATALOGUE_REVISED_AT_MILLIS),
    },
    PublishedRule {
        rule_id: "admin-policy-attached-to-user",
        name: "AdminPolicyAttachedToUser",
        description: "Detects when admin policy is attached to an IAM user",
        severity: DETECTION_RULE_SEVERITY_HIGH,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Privilege Escalation",
        technique: "T1098.001 - Account Manipulation: Additional Cloud Credentials",
        service: "iam",
        expression: "SELECT * FROM cloudtrail WHERE eventSource = 'iam.amazonaws.com' AND eventName IN ('AttachUserPolicy', 'PutUserPolicy') AND requestParameters.policyArn = 'arn:aws:iam::aws:policy/AdministratorAccess'",
        runtime_platforms: &[],
        published_at_millis: CATALOGUE_PUBLISHED_AT_MILLIS,
        revised_at_millis: Some(CATALOGUE_REVISED_AT_MILLIS),
    },
    PublishedRule {
        rule_id: "cloudtrail-logging-disabled",
        name: "CloudTrailLoggingDisabled",
        description: "Detects when CloudTrail logging is disabled",
        severity: DETECTION_RULE_SEVERITY_HIGH,
        data_source: DETECTION_RULE_DATA_SOURCE_CLOUD_TRAIL,
        tactic: "Defense Evasion",
        technique: "T1562.008 - Impair Defenses: Disable or Modify Cloud Logs",
        service: "cloudtrail",
        expression: "SELECT * FROM cloudtrail WHERE eventSource = 'cloudtrail.amazonaws.com' AND eventName IN ('StopLogging', 'DeleteTrail', 'UpdateTrail')",
        runtime_platforms: &[],
        published_at_millis: CATALOGUE_PUBLISHED_AT_MILLIS,
        revised_at_millis: Some(CATALOGUE_REVISED_AT_MILLIS),
    },
];

/// When GuardDuty published this catalogue: midnight UTC on
/// [`CATALOGUE_PUBLISHED_ON`], in the milliseconds the store holds every instant
/// as.
///
/// FIXED, and deliberately NOT the instant this store came up: a published rule is
/// not created by anything this process does, so a `CreatedAt` taken from the local
/// clock would move on every restart and make freshness and change-detection logic
/// in a client wrong. The particular date is this emulator's own — no recording in
/// this corpus discloses when AWS published the catalogue — and it is in the past
/// of any run, which is what `CreatedAt` means for an entry a caller has always
/// been able to read.
const CATALOGUE_PUBLISHED_AT_MILLIS: i64 = 1_732_060_800_000;

/// The date [`CATALOGUE_PUBLISHED_AT_MILLIS`] is midnight UTC on, as a timestamp
/// reader renders it. The pair is checked against each other over the wire by this
/// crate's `published_detection_rule_catalog` suite, so the number above cannot
/// drift from the date it claims to be.
pub const CATALOGUE_PUBLISHED_ON: &str = "2024-11-20T00:00:00Z";

/// When GuardDuty last revised the entries it has revised since publishing them —
/// the `UpdatedAt` those rows report — midnight UTC on [`CATALOGUE_REVISED_ON`]. A
/// rule it has not revised carries no `UpdatedAt` at all, which is why the member
/// is optional on both `RuleSummary` and `RuleDetail`. Fixed for the same reason
/// the publication instant is.
const CATALOGUE_REVISED_AT_MILLIS: i64 = 1_749_513_600_000;

/// The date [`CATALOGUE_REVISED_AT_MILLIS`] is midnight UTC on. See
/// [`CATALOGUE_PUBLISHED_ON`].
pub const CATALOGUE_REVISED_ON: &str = "2025-06-10T00:00:00Z";

// Every published rule carries values the model admits: a `severity` and a
// `dataSource` from their own enums, and a `ruleId` matching `RuleId`'s pattern and
// length. The build fails rather than the wire carrying a value the model does not
// name.
const _: () = {
    let mut index = 0;
    while index < PUBLISHED_CUSTOM_DETECTION_RULES.len() {
        let rule = &PUBLISHED_CUSTOM_DETECTION_RULES[index];
        assert!(enum_carries(
            crate::ops::types::shared_types::DETECTION_RULE_SEVERITY_VALUES,
            rule.severity
        ));
        assert!(enum_carries(
            crate::ops::types::shared_types::DETECTION_RULE_DATA_SOURCE_VALUES,
            rule.data_source
        ));
        assert!(is_rule_id_pattern(rule.rule_id));
        assert!(!rule.name.is_empty() && rule.name.len() <= 255);
        assert!(!rule.description.is_empty() && rule.description.len() <= 1024);
        assert!(!rule.tactic.is_empty() && rule.tactic.len() <= 255);
        assert!(!rule.technique.is_empty() && rule.technique.len() <= 255);
        assert!(!rule.service.is_empty() && rule.service.len() <= 128);
        assert!(!rule.expression.is_empty() && rule.expression.len() <= 4096);
        // The published FORM of each catalogue value, so a row written in prose
        // instead fails the build rather than reaching the wire.
        assert!(is_published_rule_name_of(rule.name, rule.rule_id));
        assert!(is_published_description(rule.description));
        assert!(is_mitre_technique(rule.technique));
        // A publication instant is in the past of any run and a revision never
        // precedes it, so `UpdatedAt` cannot report a rule revised before it
        // existed.
        assert!(rule.published_at_millis > 0);
        if let Some(revised) = rule.revised_at_millis {
            assert!(revised >= rule.published_at_millis);
        }
        index += 1;
    }
};

/// Is `name` the name GuardDuty publishes the rule `rule_id` under?
///
/// The recorded catalogue entry gives the relation between the two: the id is the
/// published name in lower case with a hyphen between its words
/// (`AdminPolicyAttachedToRole` is published under the id
/// `admin-policy-attached-to-role`). So a name is the published name of an id when
/// it is an unspaced run of ASCII letters and digits opening with a capital, and
/// lower-casing it gives the id with its hyphens taken out. Where the words run
/// together the id cannot say where one ends — which is why this is checked rather
/// than derived.
const fn is_published_rule_name_of(name: &str, rule_id: &str) -> bool {
    let name = name.as_bytes();
    let id = rule_id.as_bytes();
    if name.is_empty() || !name[0].is_ascii_uppercase() {
        return false;
    }
    let mut at_name = 0;
    let mut at_id = 0;
    while at_name < name.len() {
        let letter = name[at_name];
        if !letter.is_ascii_alphanumeric() {
            return false;
        }
        let lowered = letter.to_ascii_lowercase();
        // Skip the hyphens the id separates its words with; the name runs them
        // together.
        while at_id < id.len() && id[at_id] == b'-' {
            at_id += 1;
        }
        if at_id >= id.len() || id[at_id] != lowered {
            return false;
        }
        at_name += 1;
        at_id += 1;
    }
    at_id == id.len()
}

/// Is `description` in the form GuardDuty publishes a rule's description in — the
/// one clause saying what the rule detects, opening with the verb the recorded
/// entry's own description opens with?
const fn is_published_description(description: &str) -> bool {
    let opening = DESCRIPTION_OPENING.as_bytes();
    let bytes = description.as_bytes();
    if bytes.len() <= opening.len() {
        return false;
    }
    let mut index = 0;
    while index < opening.len() {
        if bytes[index] != opening[index] {
            return false;
        }
        index += 1;
    }
    // A published description is a clause and not a sentence: it carries no
    // terminating full stop.
    bytes[bytes.len() - 1] != b'.'
}

/// What every published description opens with, as the recorded catalogue entry's
/// own description does ("Detects when admin policy is attached to an IAM role").
const DESCRIPTION_OPENING: &str = "Detects ";

/// Is `technique` in the form a published rule names its MITRE ATT&CK technique
/// in — the technique id, then ` - `, then the technique's name, as the recorded
/// catalogue entry carries it ("T1098.001 - Account Manipulation: Additional Cloud
/// Credentials")?
const fn is_mitre_technique(technique: &str) -> bool {
    let bytes = technique.as_bytes();
    if bytes.is_empty() || bytes[0] != b'T' {
        return false;
    }
    let mut index = 1;
    // The technique id: digits, then optionally a sub-technique after a dot.
    let mut digits = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
        digits += 1;
    }
    if digits == 0 {
        return false;
    }
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        let mut sub_digits = 0;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
            sub_digits += 1;
        }
        if sub_digits == 0 {
            return false;
        }
    }
    // Then the separator, then a non-empty technique name.
    let separator = TECHNIQUE_SEPARATOR.as_bytes();
    let mut at_separator = 0;
    while at_separator < separator.len() {
        if index >= bytes.len() || bytes[index] != separator[at_separator] {
            return false;
        }
        index += 1;
        at_separator += 1;
    }
    index < bytes.len()
}

/// What stands between a published technique's id and its name.
const TECHNIQUE_SEPARATOR: &str = " - ";

/// Does `rule_id` satisfy `RuleId`'s own form — `len 1..100`, `pattern
/// ^[a-z0-9]+(-[a-z0-9]+)*$` (`smithy_query(shape="RuleId")`)?
///
/// A `const fn` so the catalogue above is checked at BUILD time; the request
/// boundary checks a caller's `ruleId` against the same model constraint before any
/// handler runs.
pub(super) const fn is_rule_id_pattern(rule_id: &str) -> bool {
    let bytes = rule_id.as_bytes();
    if bytes.is_empty() || bytes.len() > 100 {
        return false;
    }
    let mut index = 0;
    // A segment must be non-empty, so a dash may neither open nor close the id nor
    // follow another dash.
    let mut segment_len = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'-' {
            if segment_len == 0 {
                return false;
            }
            segment_len = 0;
        } else if matches!(byte, b'a'..=b'z' | b'0'..=b'9') {
            segment_len += 1;
        } else {
            return false;
        }
        index += 1;
    }
    segment_len > 0
}
