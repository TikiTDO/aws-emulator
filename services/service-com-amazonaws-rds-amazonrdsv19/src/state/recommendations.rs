// SPDX-License-Identifier: CC0-1.0

//! The recommendations `DescribeDBRecommendations` reports.
//!
//! Amazon RDS analyses the resources of an account and publishes one recommendation per
//! issue it finds, each naming the resource it is about, how urgent it is, which
//! Well-Architected category it falls under and what to do about it. What this module
//! holds is that set for THIS service: one record per issue the analysis below finds in
//! the configuration this store already keeps for a DB instance or a DB cluster.
//!
//! Every record therefore follows from state a caller created: the analysis reads the
//! stored configuration of a resource that is `available` and raises exactly the checks
//! that hold for it, so an account that created nothing has no recommendations, and a
//! caller that turns a setting back on sees the recommendation it raised reported
//! `resolved` rather than disappearing. Nothing here analyses RUNNING work: this service
//! records no database load or wait-state metrics, so no record carries `IssueDetails`
//! and none of the checks is a performance finding — a metric nothing measured would be
//! an invented one.
//!
//! The members, their published value sets and the Well-Architected categories are the
//! model's ([`RECOMMENDATION_SEVERITIES`], [`RECOMMENDATION_CATEGORIES`],
//! [`RECOMMENDATION_STATUSES`]); the type ids and the wording of each check are this
//! emulator's, because the model publishes no catalog of either.
//!
//! The set is per REGION, like every other store of this service: a recommendation
//! belongs to the region of the resource it is about. It is bounded on both factors — at
//! most [`CONFIGURATION_CHECKS`]`.len()` records per resource, and the resources of a
//! region are capped by [`super::DB_INSTANCE_QUOTA_PER_REGION`] and
//! [`super::DB_CLUSTER_QUOTA_PER_REGION`], over at most [`super::MAX_REGIONS`] regions —
//! and a record carries only values this service computed: two minted ids, the resource's
//! own ARN and identifier, and one observed setting value.

use super::*;
use crate::ops::types::{
    ContextAttribute, DBRecommendation, RecommendedAction, RecommendedActionParameter,
};

/// `DBRecommendation$Source`: "the Amazon Web Services service that generated the
/// recommendations" — this control plane, which performed the analysis below.
pub const RECOMMENDATION_SOURCE: &str = "RDS";

/// `DBRecommendation$Status` `active`: "the recommendations which are ready for you to
/// apply".
pub const RECOMMENDATION_STATUS_ACTIVE: &str = "active";

/// `DBRecommendation$Status` `pending`: "the applied or scheduled recommendations which
/// are in progress".
pub const RECOMMENDATION_STATUS_PENDING: &str = "pending";

/// `DBRecommendation$Status` `resolved`: "the recommendations which are completed" — what
/// a raised check becomes once the configuration it found no longer holds.
pub const RECOMMENDATION_STATUS_RESOLVED: &str = "resolved";

/// `DBRecommendation$Status` `dismissed`: "the recommendations that you dismissed", which
/// is a caller's own decision about a recommendation and so survives a later analysis.
pub const RECOMMENDATION_STATUS_DISMISSED: &str = "dismissed";

/// The statuses `DBRecommendation$Status` publishes, in the order the model documents
/// them. Also the value set the `status` filter of `DescribeDBRecommendations` accepts.
pub const RECOMMENDATION_STATUSES: [&str; 4] = [
    RECOMMENDATION_STATUS_ACTIVE,
    RECOMMENDATION_STATUS_PENDING,
    RECOMMENDATION_STATUS_RESOLVED,
    RECOMMENDATION_STATUS_DISMISSED,
];

/// The severities `DBRecommendation$Severity` publishes ("the severity level can help you
/// decide the urgency with which to address the recommendation"), in the order the model
/// documents them. Also the value set the `severity` filter accepts.
pub const RECOMMENDATION_SEVERITIES: [&str; 4] = ["high", "medium", "low", "informational"];

/// The categories `DBRecommendation$Category` publishes, in the order the model documents
/// them.
pub const RECOMMENDATION_CATEGORIES: [&str; 6] = [
    "performance efficiency",
    "security",
    "reliability",
    "cost optimization",
    "operational excellence",
    "sustainability",
];

/// The languages `DescribeDBRecommendationsMessage$Locale` publishes as its valid values,
/// in the order the model documents them.
pub const RECOMMENDATION_LOCALES: [&str; 12] = [
    "en", "en_UK", "de", "es", "fr", "id", "it", "ja", "ko", "pt_BR", "zh_TW", "zh_CN",
];

/// `RecommendedAction$Status` `ready`: the action has not been applied yet.
pub const RECOMMENDED_ACTION_STATUS_READY: &str = "ready";

/// `RecommendedAction$Status` `resolved`: the issue the action addresses is gone.
pub const RECOMMENDED_ACTION_STATUS_RESOLVED: &str = "resolved";

/// `RecommendedAction$ApplyModes` `immediately`: "the action is applied immediately" —
/// the mode of an action that is one modify call on the resource.
pub const APPLY_MODE_IMMEDIATELY: &str = "immediately";

/// `RecommendedAction$ApplyModes` `manual`: "the action requires you to resolve the
/// recommendation manually" — the mode of an action no single modify call can perform.
pub const APPLY_MODE_MANUAL: &str = "manual";

/// Refuse a `Locale` outside the languages the member publishes: a value that names no
/// language the recommendations are available in would otherwise be answered with text in
/// a language the caller did not ask for.
pub fn check_recommendation_locale(locale: &str) -> Result<(), String> {
    if RECOMMENDATION_LOCALES.contains(&locale) {
        return Ok(());
    }
    Err(format!(
        "Invalid value {locale} for Locale. Valid values are: {}",
        RECOMMENDATION_LOCALES.join(", ")
    ))
}

/// Which family the resource a recommendation is about belongs to, which is what decides
/// the operation and the identifier member its recommended action names, and the filter
/// name that selects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum RecommendationSubject {
    /// A DB instance: selected by the `dbi-resource-id` filter, fixed by
    /// `ModifyDBInstance`.
    DbInstance,
    /// A DB cluster: selected by the `cluster-resource-id` filter, fixed by
    /// `ModifyDBCluster`.
    DbCluster,
}

impl RecommendationSubject {
    /// The operation a recommended action on this kind of resource names.
    pub fn modify_operation(self) -> &'static str {
        match self {
            Self::DbInstance => "ModifyDBInstance",
            Self::DbCluster => "ModifyDBCluster",
        }
    }

    /// The request member that names this kind of resource.
    pub fn identifier_member(self) -> &'static str {
        match self {
            Self::DbInstance => "DBInstanceIdentifier",
            Self::DbCluster => "DBClusterIdentifier",
        }
    }
}

/// The configuration the analysis reads, projected off a stored DB instance or DB cluster
/// record so one table of checks serves both.
///
/// A setting the resource family does not have at all is `None`, which no check can hold
/// on: `MultiAZ` is a property of a DB INSTANCE deployment, and a cluster's is read off
/// the Availability Zones of its members rather than configured on the cluster.
#[derive(Debug, Clone)]
pub struct AnalysedConfiguration {
    /// Which family the resource belongs to.
    pub subject: RecommendationSubject,
    /// The resource's immutable server-minted id: `DbiResourceId` for an instance,
    /// `DbClusterResourceId` for a cluster, which is what the `dbi-resource-id` and
    /// `cluster-resource-id` filters select on.
    pub resource_id: String,
    /// The resource's own identifier, as its recommended action names it.
    pub identifier: String,
    /// The resource's ARN, as its own record computes it.
    pub resource_arn: String,
    /// `BackupRetentionPeriod`: the days of automated backups the resource keeps.
    pub backup_retention_period: i32,
    /// `MultiAZ`, for the family that configures it.
    pub multi_az: Option<bool>,
    /// `PubliclyAccessible`, for the family that configures it.
    pub publicly_accessible: Option<bool>,
    /// `StorageEncrypted`.
    pub storage_encrypted: bool,
    /// `DeletionProtection`.
    pub deletion_protection: bool,
    /// `AutoMinorVersionUpgrade`.
    pub auto_minor_version_upgrade: bool,
    /// `MonitoringInterval`: the Enhanced Monitoring interval in seconds, 0 being off.
    pub monitoring_interval: i32,
    /// `PerformanceInsightsEnabled`.
    pub performance_insights_enabled: bool,
}

impl AnalysedConfiguration {
    /// The configuration of a stored DB instance.
    pub fn of_instance(record: &DbInstanceRecord) -> Self {
        Self {
            subject: RecommendationSubject::DbInstance,
            resource_id: record.dbi_resource_id.clone(),
            identifier: record.identifier.clone(),
            resource_arn: record.arn(),
            backup_retention_period: record.backup_retention_period,
            multi_az: Some(record.multi_az),
            publicly_accessible: Some(record.publicly_accessible),
            storage_encrypted: record.storage_encrypted,
            deletion_protection: record.deletion_protection,
            auto_minor_version_upgrade: record.auto_minor_version_upgrade,
            monitoring_interval: record.monitoring_interval,
            performance_insights_enabled: record.performance_insights_enabled,
        }
    }

    /// The configuration of a stored DB cluster.
    pub fn of_cluster(record: &DbClusterRecord) -> Self {
        Self {
            subject: RecommendationSubject::DbCluster,
            resource_id: record.resource_id.clone(),
            identifier: record.identifier.clone(),
            resource_arn: record.arn(),
            backup_retention_period: record.backup_retention_period,
            // Not a cluster setting: see [`AnalysedConfiguration`].
            multi_az: None,
            publicly_accessible: record.publicly_accessible,
            storage_encrypted: record.storage_encrypted,
            deletion_protection: record.deletion_protection,
            auto_minor_version_upgrade: record.auto_minor_version_upgrade,
            monitoring_interval: record.monitoring_interval,
            performance_insights_enabled: record.performance_insights_enabled,
        }
    }
}

/// The stored setting one check reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckedSetting {
    /// Holds while the resource keeps no automated backups.
    BackupRetentionPeriod,
    /// Holds while a DB instance is deployed in one Availability Zone.
    MultiAz,
    /// Holds while the resource's storage is unencrypted.
    StorageEncrypted,
    /// Holds while Enhanced Monitoring is off.
    MonitoringInterval,
    /// Holds while Performance Insights is off.
    PerformanceInsightsEnabled,
    /// Holds while the resource can be deleted without the protection being removed
    /// first.
    DeletionProtection,
    /// Holds while published minor engine versions are not applied automatically.
    AutoMinorVersionUpgrade,
    /// Holds while the resource resolves to a public address.
    PubliclyAccessible,
}

impl CheckedSetting {
    /// The member a describe of the resource reports this setting as, which is what the
    /// recommendation names when it says what was observed.
    pub fn member(self) -> &'static str {
        match self {
            Self::BackupRetentionPeriod => "BackupRetentionPeriod",
            Self::MultiAz => "MultiAZ",
            Self::StorageEncrypted => "StorageEncrypted",
            Self::MonitoringInterval => "MonitoringInterval",
            Self::PerformanceInsightsEnabled => "PerformanceInsightsEnabled",
            Self::DeletionProtection => "DeletionProtection",
            Self::AutoMinorVersionUpgrade => "AutoMinorVersionUpgrade",
            Self::PubliclyAccessible => "PubliclyAccessible",
        }
    }

    /// The value `config` holds for this setting, as the member reports it, or `None`
    /// where the resource family does not configure it at all.
    pub fn observed(self, config: &AnalysedConfiguration) -> Option<String> {
        match self {
            Self::BackupRetentionPeriod => Some(config.backup_retention_period.to_string()),
            Self::MultiAz => config.multi_az.map(|value| value.to_string()),
            Self::StorageEncrypted => Some(config.storage_encrypted.to_string()),
            Self::MonitoringInterval => Some(config.monitoring_interval.to_string()),
            Self::PerformanceInsightsEnabled => {
                Some(config.performance_insights_enabled.to_string())
            }
            Self::DeletionProtection => Some(config.deletion_protection.to_string()),
            Self::AutoMinorVersionUpgrade => Some(config.auto_minor_version_upgrade.to_string()),
            Self::PubliclyAccessible => config.publicly_accessible.map(|value| value.to_string()),
        }
    }

    /// Whether the issue this setting is checked for is present in `config`.
    fn issue_holds(self, config: &AnalysedConfiguration) -> bool {
        match self {
            // "To disable automated backups, set this value to 0"
            // (`BackupRetentionPeriod`), so a zero period is a resource keeping none.
            Self::BackupRetentionPeriod => config.backup_retention_period == 0,
            Self::MultiAz => config.multi_az == Some(false),
            Self::StorageEncrypted => !config.storage_encrypted,
            // "To disable collection of Enhanced Monitoring metrics, specify 0"
            // (`MonitoringInterval`).
            Self::MonitoringInterval => config.monitoring_interval == 0,
            Self::PerformanceInsightsEnabled => !config.performance_insights_enabled,
            Self::DeletionProtection => !config.deletion_protection,
            Self::AutoMinorVersionUpgrade => !config.auto_minor_version_upgrade,
            Self::PubliclyAccessible => config.publicly_accessible == Some(true),
        }
    }
}

/// One row of the configuration analysis: the setting it reads, how the recommendation it
/// raises is classified, the text it reports, and the action that fixes it.
///
/// The type id and the wording are this emulator's — the model publishes neither — while
/// `severity` is one of [`RECOMMENDATION_SEVERITIES`] and `category` one of
/// [`RECOMMENDATION_CATEGORIES`], which this module's own test holds every row to.
#[derive(Debug)]
pub struct ConfigurationCheck {
    /// `DBRecommendation$TypeId`: "a value that indicates the type of recommendation",
    /// naming the check that raised it.
    pub type_id: &'static str,
    /// The stored setting the check reads.
    pub setting: CheckedSetting,
    /// `DBRecommendation$Severity`.
    pub severity: &'static str,
    /// `DBRecommendation$Category`.
    pub category: &'static str,
    /// `DBRecommendation$TypeDetection`: the type-level summary of what was found, which
    /// the per-resource `Detection` extends with the value observed.
    pub detection: &'static str,
    /// `DBRecommendation$TypeRecommendation` and `DBRecommendation$Recommendation`: what
    /// to do about it.
    pub recommendation: &'static str,
    /// `DBRecommendation$Description`: the detailed description of the recommendation.
    pub description: &'static str,
    /// `DBRecommendation$Impact`: "a short description that explains the possible impact
    /// of an issue".
    pub impact: &'static str,
    /// `RecommendedAction$Title`.
    pub action_title: &'static str,
    /// `RecommendedAction$Description`.
    pub action_description: &'static str,
    /// The request member the recommended action sets and the value it sets it to, for a
    /// setting one modify call can change; `None` where it cannot be changed in place, in
    /// which case the action is reported with [`APPLY_MODE_MANUAL`] and no operation.
    pub action_parameter: Option<(&'static str, &'static str)>,
}

/// The configuration analysis this service performs, one row per issue it can find.
///
/// Each row reads a setting this store already keeps for a DB instance or a DB cluster, so
/// a raised recommendation is a statement about state a caller created, and the row's
/// action names the operation and the member that changes that same setting.
///
/// A `static` rather than a `const` because a raised record BORROWS the row that raised
/// it ([`DbRecommendationRecord::check`]), so there is one copy of the table and a record
/// cannot drift from the check it names.
pub static CONFIGURATION_CHECKS: [ConfigurationCheck; 8] = [
    ConfigurationCheck {
        type_id: "automated-backups-turned-off",
        setting: CheckedSetting::BackupRetentionPeriod,
        severity: "high",
        category: "reliability",
        detection: "Automated backups are turned off.",
        recommendation: "Set a backup retention period of at least one day.",
        description: "Amazon RDS takes an automated backup of a resource whose backup \
                      retention period is at least one day and keeps it for that period, \
                      which is what point-in-time restore replays. A resource with a \
                      retention period of zero keeps no automated backup at all.",
        impact: "The resource cannot be restored to a point in time, and the changes made \
                 since the last manual snapshot cannot be recovered.",
        action_title: "Set a backup retention period",
        action_description: "Modify the resource with a backup retention period of at \
                             least one day.",
        action_parameter: Some(("BackupRetentionPeriod", "7")),
    },
    ConfigurationCheck {
        type_id: "single-az-deployment",
        setting: CheckedSetting::MultiAz,
        severity: "medium",
        category: "reliability",
        detection: "The DB instance is deployed in a single Availability Zone.",
        recommendation: "Convert the DB instance to a Multi-AZ deployment.",
        description: "A Multi-AZ deployment keeps a standby in a second Availability Zone \
                      and fails the DB instance over to it, so the loss of one \
                      Availability Zone does not take the database down.",
        impact: "The database is unreachable for as long as its one Availability Zone is \
                 impaired, and recovering from a failure of it needs a restore.",
        action_title: "Turn on Multi-AZ",
        action_description: "Modify the DB instance to a Multi-AZ deployment.",
        action_parameter: Some(("MultiAZ", "true")),
    },
    ConfigurationCheck {
        type_id: "storage-not-encrypted",
        setting: CheckedSetting::StorageEncrypted,
        severity: "high",
        category: "security",
        detection: "Encryption at rest is turned off.",
        recommendation: "Re-create the resource from a snapshot with encryption at rest \
                         turned on.",
        description: "A resource created with encryption at rest turned on encrypts its \
                      storage, its automated backups and its snapshots. Encryption cannot \
                      be turned on in place: an encrypted copy of a snapshot is restored \
                      into a new resource instead.",
        impact: "The data on disk, and every backup and snapshot taken of it, is stored \
                 unencrypted.",
        action_title: "Restore into an encrypted resource",
        action_description: "Copy a snapshot of the resource with a KMS key, restore the \
                             encrypted copy, and move the clients to the resource it \
                             creates.",
        action_parameter: None,
    },
    ConfigurationCheck {
        type_id: "enhanced-monitoring-turned-off",
        setting: CheckedSetting::MonitoringInterval,
        severity: "low",
        category: "operational excellence",
        detection: "Enhanced Monitoring is turned off.",
        recommendation: "Set an Enhanced Monitoring interval.",
        description: "Enhanced Monitoring collects the operating-system metrics of the \
                      host the resource runs on at the configured interval, in seconds. \
                      An interval of zero collects none.",
        impact: "The host's operating-system metrics are not collected, so a resource \
                 exhaustion on it cannot be diagnosed from them.",
        action_title: "Set a monitoring interval",
        action_description: "Modify the resource with a non-zero Enhanced Monitoring \
                             interval.",
        action_parameter: Some(("MonitoringInterval", "60")),
    },
    ConfigurationCheck {
        type_id: "performance-insights-turned-off",
        setting: CheckedSetting::PerformanceInsightsEnabled,
        severity: "informational",
        category: "performance efficiency",
        detection: "Performance Insights is turned off.",
        recommendation: "Turn on Performance Insights.",
        description: "Performance Insights records the database load of a resource and \
                      keeps it for its retention period, which is what the analysis of \
                      load by wait state and by statement is computed from.",
        impact: "The database load is not recorded, so a performance problem cannot be \
                 analysed after it has passed.",
        action_title: "Turn on Performance Insights",
        action_description: "Modify the resource with Performance Insights turned on.",
        action_parameter: Some(("EnablePerformanceInsights", "true")),
    },
    ConfigurationCheck {
        type_id: "deletion-protection-turned-off",
        setting: CheckedSetting::DeletionProtection,
        severity: "low",
        category: "reliability",
        detection: "Deletion protection is turned off.",
        recommendation: "Turn on deletion protection.",
        description: "A resource with deletion protection turned on cannot be deleted \
                      until the protection is turned off, so deleting it takes two \
                      deliberate calls rather than one.",
        impact: "One delete call removes the resource and the automated backups it keeps.",
        action_title: "Turn on deletion protection",
        action_description: "Modify the resource with deletion protection turned on.",
        action_parameter: Some(("DeletionProtection", "true")),
    },
    ConfigurationCheck {
        type_id: "auto-minor-version-upgrade-turned-off",
        setting: CheckedSetting::AutoMinorVersionUpgrade,
        severity: "medium",
        category: "security",
        detection: "Automatic minor version upgrades are turned off.",
        recommendation: "Turn on automatic minor version upgrades.",
        description: "A minor engine version carries the fixes published for the engine, \
                      and is applied in the maintenance window of a resource that has \
                      automatic minor version upgrades turned on.",
        impact: "Published engine fixes, security fixes among them, are applied only when \
                 someone upgrades the engine by hand.",
        action_title: "Turn on automatic minor version upgrades",
        action_description: "Modify the resource with automatic minor version upgrades \
                             turned on.",
        action_parameter: Some(("AutoMinorVersionUpgrade", "true")),
    },
    ConfigurationCheck {
        type_id: "publicly-accessible",
        setting: CheckedSetting::PubliclyAccessible,
        severity: "high",
        category: "security",
        detection: "The resource is publicly accessible.",
        recommendation: "Make the resource private.",
        description: "A publicly accessible resource resolves to a public address, so \
                      anything the network and its security groups admit can open a \
                      connection to the database from outside its VPC.",
        impact: "The database endpoint is reachable from outside the VPC, so only the \
                 security groups and the credentials stand between it and the internet.",
        action_title: "Make the resource private",
        action_description: "Modify the resource so that it is no longer publicly \
                             accessible.",
        action_parameter: Some(("PubliclyAccessible", "false")),
    },
];

/// The second half of a recommendation's store key: the resource it is about and the check
/// that raised it, which is what makes one check raise at most one record per resource and
/// what orders the report within a region.
pub fn recommendation_key(resource_id: &str, type_id: &str) -> String {
    format!("{resource_id}/{type_id}")
}

mod serde_configuration_check {
    use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};

    use super::{ConfigurationCheck, CONFIGURATION_CHECKS};

    pub fn serialize<S: Serializer>(
        check: &&'static ConfigurationCheck,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        check.type_id.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<&'static ConfigurationCheck, D::Error> {
        let type_id = String::deserialize(deserializer)?;
        CONFIGURATION_CHECKS
            .iter()
            .find(|check| check.type_id == type_id)
            .ok_or_else(|| D::Error::custom(format!("unknown recommendation check {type_id}")))
    }
}

/// One recommendation this service raised: which resource it is about, which check raised
/// it, what that check observed, and where it stands.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DbRecommendationRecord {
    /// `DBRecommendation$RecommendationId`: minted once, so the `recommendation-id` filter
    /// and `ModifyDBRecommendation` name a stable record.
    pub recommendation_id: String,
    /// `RecommendedAction$ActionId`: "the unique identifier of the recommended action",
    /// minted with the record.
    pub action_id: String,
    /// The row of [`CONFIGURATION_CHECKS`] that raised this record.
    #[serde(with = "serde_configuration_check")]
    pub check: &'static ConfigurationCheck,
    /// Which family the resource belongs to.
    pub subject: RecommendationSubject,
    /// The resource's minted resource id, which the `dbi-resource-id` /
    /// `cluster-resource-id` filters select on.
    pub resource_id: String,
    /// The resource's own identifier, as the recommended action names it.
    pub identifier: String,
    /// `DBRecommendation$ResourceArn`.
    pub resource_arn: String,
    /// The value the check last observed for its setting, as the member reports it.
    pub observed_value: String,
    /// `DBRecommendation$Status`, one of [`RECOMMENDATION_STATUSES`].
    pub status: String,
    /// `DBRecommendation$CreatedTime`: when the analysis first raised this record.
    #[serde(with = "::service_common::persist::smithy_time")]
    pub created_time: ::aws_smithy_types::DateTime,
    /// `DBRecommendation$UpdatedTime`: when it last changed — a re-raise, a resolution, or
    /// a new value observed for its setting.
    #[serde(with = "::service_common::persist::smithy_time")]
    pub updated_time: ::aws_smithy_types::DateTime,
}

impl DbRecommendationRecord {
    /// This record's position in its region's report, which is also the `Marker` a page
    /// boundary at this record resumes from.
    pub fn page_key(&self) -> String {
        recommendation_key(&self.resource_id, self.check.type_id)
    }

    /// `DBRecommendation$Detection`: the type-level finding, plus the value this resource
    /// was observed to hold for the setting the check reads.
    pub fn detection(&self) -> String {
        format!(
            "{} {} is {} on {}.",
            self.check.detection,
            self.check.setting.member(),
            self.observed_value,
            self.resource_arn
        )
    }

    /// `DBRecommendation$Reason`: "the reason why this recommendation was created", which
    /// is the setting the check read and the value it found.
    pub fn reason(&self) -> String {
        format!(
            "{} is {} on {}.",
            self.check.setting.member(),
            self.observed_value,
            self.resource_arn
        )
    }

    /// The one action this recommendation reports: the call that changes the setting the
    /// check read, or a manual action where no single call can.
    fn recommended_action(&self) -> RecommendedAction {
        let (operation, parameters, apply_mode) = match self.check.action_parameter {
            Some((member, value)) => (
                Some(self.subject.modify_operation().to_string()),
                Some(vec![
                    RecommendedActionParameter {
                        key: Some(self.subject.identifier_member().to_string()),
                        value: Some(self.identifier.clone()),
                    },
                    RecommendedActionParameter {
                        key: Some(member.to_string()),
                        value: Some(value.to_string()),
                    },
                ]),
                APPLY_MODE_IMMEDIATELY,
            ),
            None => (None, None, APPLY_MODE_MANUAL),
        };
        RecommendedAction {
            action_id: Some(self.action_id.clone()),
            title: Some(self.check.action_title.to_string()),
            description: Some(self.check.action_description.to_string()),
            operation,
            parameters,
            apply_modes: Some(vec![apply_mode.to_string()]),
            // An action is `ready` while the issue stands and `resolved` once the
            // configuration it asked for is in place; nothing this service does applies
            // or schedules an action on a caller's behalf, so neither of the other two
            // published statuses is ever reported.
            status: Some(
                if self.status == RECOMMENDATION_STATUS_RESOLVED {
                    RECOMMENDED_ACTION_STATUS_RESOLVED
                } else {
                    RECOMMENDED_ACTION_STATUS_READY
                }
                .to_string(),
            ),
            // The supporting attributes that explain the action: the setting the check
            // read and the value it found.
            context_attributes: Some(vec![ContextAttribute {
                key: Some(self.check.setting.member().to_string()),
                value: Some(self.observed_value.clone()),
            }]),
            // No issue details: this service measures no database load, so there is no
            // metric to report under them.
            issue_details: None,
        }
    }

    /// Project the stored record onto the modelled `DBRecommendation` output shape.
    ///
    /// `Links` and `AdditionalInfo` are left out rather than invented: a documentation URL
    /// is not state this service holds, and `IssueDetails` would need measurements nothing
    /// here takes.
    pub fn to_wire(&self) -> DBRecommendation {
        DBRecommendation {
            recommendation_id: Some(self.recommendation_id.clone()),
            type_id: Some(self.check.type_id.to_string()),
            severity: Some(self.check.severity.to_string()),
            resource_arn: Some(self.resource_arn.clone()),
            status: Some(self.status.clone()),
            created_time: Some(self.created_time),
            updated_time: Some(self.updated_time),
            detection: Some(self.detection()),
            recommendation: Some(self.check.recommendation.to_string()),
            description: Some(self.check.description.to_string()),
            reason: Some(self.reason()),
            recommended_actions: Some(vec![self.recommended_action()]),
            category: Some(self.check.category.to_string()),
            source: Some(RECOMMENDATION_SOURCE.to_string()),
            type_detection: Some(self.check.detection.to_string()),
            type_recommendation: Some(self.check.recommendation.to_string()),
            impact: Some(self.check.impact.to_string()),
            ..DBRecommendation::default()
        }
    }
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The recommendations raised in `region`, in `(resource id, type id)` key order — the
    /// order `DescribeDBRecommendations` reports and pages them in.
    ///
    /// An ordered range rather than a scan of the whole map, so the work is bounded by one
    /// region's records however many regions the store holds.
    pub fn db_recommendations_in(&self, region: &str) -> Vec<&DbRecommendationRecord> {
        range_in(&self.db_recommendations, region)
    }

    /// The recommendation `recommendation_id` names in `region`, or `None` where this
    /// service raised none under that id.
    pub fn resolve_db_recommendation(
        &self,
        region: &str,
        recommendation_id: &str,
    ) -> Option<&DbRecommendationRecord> {
        self.db_recommendations_in(region)
            .into_iter()
            .find(|record| record.recommendation_id == recommendation_id)
    }

    /// Run the configuration analysis over `region` and bring its recommendations in line
    /// with what the resources there are configured to be.
    ///
    /// Called from [`ComAmazonawsRdsAmazonrdsv19State::settle_due_transitions`], so it
    /// runs where every other change the clock has already made due is materialised. What
    /// it writes was owed at this instant whether or not anybody called: a resource is
    /// analysed once it has reached `available`, which is itself a transition the clock
    /// owed, and a check that no longer holds — because a modify changed the setting it
    /// reads — leaves a `resolved` record behind rather than vanishing. A record a caller
    /// dismissed is that caller's own decision and is left alone.
    pub(super) fn settle_db_recommendations(&mut self, region: &str) {
        // The configurations to analyse, copied out before anything is written: a resource
        // is analysed once it is `available`, which is when this service has finished
        // bringing it up.
        let analysed: Vec<AnalysedConfiguration> = self
            .instances_in(region)
            .into_iter()
            .filter(|record| record.status == DB_INSTANCE_STATUS_AVAILABLE)
            .map(AnalysedConfiguration::of_instance)
            .chain(
                self.clusters_in(region)
                    .into_iter()
                    .filter(|record| record.status == DB_CLUSTER_STATUS_AVAILABLE)
                    .map(AnalysedConfiguration::of_cluster),
            )
            .collect();
        // Every resource of the region, whatever status it is in: a record about a
        // resource that is no longer stored is dropped below, while a resource that is not
        // `available` keeps the records it already has.
        let stored_resources: BTreeSet<String> = self
            .instances_in(region)
            .into_iter()
            .map(|record| record.dbi_resource_id.clone())
            .chain(
                self.clusters_in(region)
                    .into_iter()
                    .map(|record| record.resource_id.clone()),
            )
            .collect();

        let now = now_as_wire_timestamp();
        for config in &analysed {
            for check in &CONFIGURATION_CHECKS {
                let key = (
                    region.to_string(),
                    recommendation_key(&config.resource_id, check.type_id),
                );
                let observed = check
                    .setting
                    .observed(config)
                    .filter(|_| check.setting.issue_holds(config));
                let raised = self.db_recommendations.contains_key(&key);
                match (observed, raised) {
                    // The issue holds and no record covers it yet: raise one.
                    (Some(observed), false) => {
                        let recommendation_id =
                            mint_uuid_token("db-recommendation", self.next_mint());
                        let action_id =
                            mint_uuid_token("db-recommended-action", self.next_mint());
                        self.db_recommendations.insert(
                            key,
                            DbRecommendationRecord {
                                recommendation_id,
                                action_id,
                                check,
                                subject: config.subject,
                                resource_id: config.resource_id.clone(),
                                identifier: config.identifier.clone(),
                                resource_arn: config.resource_arn.clone(),
                                observed_value: observed,
                                status: RECOMMENDATION_STATUS_ACTIVE.to_string(),
                                created_time: now,
                                updated_time: now,
                            },
                        );
                    }
                    // The issue holds and a record covers it: a record that was resolved
                    // is raised again, and one whose setting now reads differently records
                    // the new value. Either is a change to the record, so its
                    // `UpdatedTime` moves; a record that says the same thing does not.
                    (Some(observed), true) => {
                        let Some(record) = self.db_recommendations.get_mut(&key) else {
                            // Unreachable: the key was there a line ago, under this one
                            // `&mut self`.
                            continue;
                        };
                        let resolved = record.status == RECOMMENDATION_STATUS_RESOLVED;
                        if resolved {
                            record.status = RECOMMENDATION_STATUS_ACTIVE.to_string();
                        }
                        if resolved || record.observed_value != observed {
                            record.observed_value = observed;
                            record.updated_time = now;
                        }
                        // The identifier a resource is named by can change under it
                        // (`ModifyDBInstance` renames one), and the ARN follows the
                        // identifier, so both are refreshed from the resource rather than
                        // left naming the old one.
                        record.identifier = config.identifier.clone();
                        record.resource_arn = config.resource_arn.clone();
                    }
                    // The issue does not hold any more: the recommendation is completed.
                    (None, true) => {
                        let Some(record) = self.db_recommendations.get_mut(&key) else {
                            // Unreachable, as above.
                            continue;
                        };
                        if record.status == RECOMMENDATION_STATUS_ACTIVE
                            || record.status == RECOMMENDATION_STATUS_PENDING
                        {
                            record.status = RECOMMENDATION_STATUS_RESOLVED.to_string();
                            record.updated_time = now;
                        }
                    }
                    // The issue does not hold and nothing was raised for it.
                    (None, false) => {}
                }
            }
        }

        // A recommendation is about a resource, so it goes when that resource does.
        let orphaned: Vec<RegionalKey> = self
            .db_recommendations
            .range((region.to_string(), String::new())..)
            .take_while(|((stored, _), _)| stored == region)
            .filter(|(_, record)| !stored_resources.contains(&record.resource_id))
            .map(|(key, _)| key.clone())
            .collect();
        for key in orphaned {
            self.db_recommendations.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configuration holding every check is a valid one for a DB instance, so every row
    /// of the analysis is reachable: a check nothing can hold would be dead text.
    #[test]
    fn a_configuration_that_holds_every_check_raises_every_one() {
        let config = AnalysedConfiguration {
            subject: RecommendationSubject::DbInstance,
            resource_id: "db-ANALYSED".to_string(),
            identifier: "analysed".to_string(),
            resource_arn: "arn:aws:rds:us-west-2:000000000000:db:analysed".to_string(),
            backup_retention_period: 0,
            multi_az: Some(false),
            publicly_accessible: Some(true),
            storage_encrypted: false,
            deletion_protection: false,
            auto_minor_version_upgrade: false,
            monitoring_interval: 0,
            performance_insights_enabled: false,
        };
        let raised: Vec<&str> = CONFIGURATION_CHECKS
            .iter()
            .filter(|check| {
                check
                    .setting
                    .observed(&config)
                    .filter(|_| check.setting.issue_holds(&config))
                    .is_some()
            })
            .map(|check| check.type_id)
            .collect();
        assert_eq!(
            raised.len(),
            CONFIGURATION_CHECKS.len(),
            "every check must hold on a configuration that turns every setting off, raised \
             {raised:?}"
        );
    }

    /// Every row of the analysis is classified with values the model publishes, so a
    /// client that built a `severity` filter out of the documented set can narrow the
    /// report to what this service raises.
    #[test]
    fn every_check_is_classified_with_published_values() {
        for check in &CONFIGURATION_CHECKS {
            assert!(
                RECOMMENDATION_SEVERITIES.contains(&check.severity),
                "check {:?}: severity {:?} must be one of {RECOMMENDATION_SEVERITIES:?}",
                check.type_id,
                check.severity
            );
            assert!(
                RECOMMENDATION_CATEGORIES.contains(&check.category),
                "check {:?}: category {:?} must be one of {RECOMMENDATION_CATEGORIES:?}",
                check.type_id,
                check.category
            );
        }
    }

    /// No two rows share a type id, so one resource cannot have two records under one key.
    #[test]
    fn every_check_has_its_own_type_id() {
        let ids: BTreeSet<&str> = CONFIGURATION_CHECKS
            .iter()
            .map(|check| check.type_id)
            .collect();
        assert_eq!(
            ids.len(),
            CONFIGURATION_CHECKS.len(),
            "the type ids of the analysis must be distinct, got {ids:?}"
        );
    }

    /// Only the languages the member publishes are accepted.
    #[test]
    fn only_published_locales_are_accepted() {
        for locale in RECOMMENDATION_LOCALES {
            assert!(
                check_recommendation_locale(locale).is_ok(),
                "locale {locale:?} is published and must be accepted"
            );
        }
        for locale in ["EN", "en-GB", "xx", ""] {
            assert!(
                check_recommendation_locale(locale).is_err(),
                "locale {locale:?} is not published and must be refused"
            );
        }
    }
}
