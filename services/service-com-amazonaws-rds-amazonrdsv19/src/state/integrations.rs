// SPDX-License-Identifier: CC0-1.0

//! The zero-ETL integration records `CreateIntegration` and `DescribeIntegrations` share.
//!
//! An integration pairs a database this service already holds (the source DB cluster or DB
//! instance) with an Amazon Redshift data warehouse (the target, which lives in another
//! service and is named here only by its ARN). Only the pairing lives here: the source is
//! resolved in this service's own store before the record is written, so an integration
//! cannot name a source database no create ever stored.

use std::collections::HashMap;
use std::time::Instant;

use super::{PendingChange, PendingStatus, ARN_NAMESPACE};
use crate::ops::types::{CreateIntegrationOutput, Tag};

/// How many integrations one region holds: Amazon RDS's published default quota for
/// zero-ETL integrations (100 per Region), which `DescribeAccountAttributes` reports as
/// the `Integrations` maximum and `CreateIntegration` refuses past, so the report and the
/// refusal state one and the same number. It bounds the store and every scan over it, the
/// way every other per-region quota in `src/state.rs` does.
pub const INTEGRATION_QUOTA_PER_REGION: usize = 100;

/// `IntegrationStatus` — the status a just-created integration reports while this service
/// brings it up.
pub const INTEGRATION_STATUS_CREATING: &str = "creating";

/// `IntegrationStatus` — the status an integration reports once it is up.
pub const INTEGRATION_STATUS_ACTIVE: &str = "active";

/// `IntegrationStatus` — the status an integration reports once a delete has been accepted
/// for it, until the clock has made its removal from the store due.
pub const INTEGRATION_STATUS_DELETING: &str = "deleting";

/// How Amazon RDS words the integration not-found fault.
///
/// `IntegrationIdentifier` is "the unique identifier of the integration", which a request
/// may give as the integration's name or as its ARN; the refusal names it by the ARN either
/// way, because that is the identifier that names one integration of one account in one
/// region. A value that is already an ARN is carried as the request spelled it, and a plain
/// name is rendered as the ARN it would name in `region`.
///
/// The sentence is the one the modelled fault publishes for ITSELF — "the specified
/// integration could not be found" — with the identifier it is about named in it. Stated
/// once here so every operation that resolves an integration answers the one text.
pub fn integration_not_found_message(region: &str, named: &str) -> String {
    let identifier = if named.starts_with("arn:") {
        named.to_string()
    } else {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            region,
            &format!("{ARN_RESOURCE_TYPE}:{named}"),
        )
    };
    format!("Integration with identifier {identifier} could not be found.")
}

/// The ARN resource type of an integration, as `IntegrationArn`'s own pattern spells it
/// (`^arn:aws[a-z\-]*:rds(-[a-z]*)?:[a-z0-9\-]*:[0-9]*:integration:...$`).
const ARN_RESOURCE_TYPE: &str = "integration";

/// The services an integration TARGET's ARN names: `TargetArn` is "the ARN of the Redshift
/// data warehouse used as the target for replication", and the service's own refusal for a
/// target of any other kind names the two it accepts — "choose either an Amazon Redshift
/// data warehouse or an AWS Glue Data Catalog with Redshift Managed Storage (RMS) as the
/// target" — which Redshift serves as a provisioned cluster or a Redshift Serverless
/// namespace, and Glue as a catalog.
const TARGET_ARN_SERVICES: [&str; 3] = ["redshift", "redshift-serverless", "glue"];

/// What the service answers a `TargetArn` that names no acceptable target with.
///
/// Its own fixed wording for this condition, carrying no value from the request: the
/// caller has to choose a different KIND of target, which is what the message says.
pub const INVALID_TARGET_TYPE_MESSAGE: &str =
    "Amazon RDS can't create the zero-ETL integration because the target type isn't valid. \
     Choose either an Amazon Redshift data warehouse or an AWS Glue Data Catalog with \
     Redshift Managed Storage (RMS) as the target.";

/// The id this service assigns a new integration: the 8-4-4-4-12 lowercase-hexadecimal
/// token `IntegrationArn`'s pattern requires, minted from the store's one monotone counter
/// by [`super::mint_uuid_token`] so two integrations cannot share one.
pub fn mint_integration_id(mint: u64) -> String {
    super::mint_uuid_token("integration", mint)
}

/// Refuse a `TargetArn` that is not the ARN of an acceptable integration target.
///
/// The member publishes a LENGTH bound and no pattern, so its syntax is checked here, and
/// it is checked BEFORE the source database is looked up: a request whose target ARN is
/// not an ARN at all is refused for that, whatever its source names. The message is the
/// service's own for an invalid target type ([`INVALID_TARGET_TYPE_MESSAGE`]).
pub fn check_integration_target_arn(value: &str) -> Result<(), String> {
    let fields: Vec<&str> = value.splitn(6, ':').collect();
    let [prefix, partition, service, region, account, resource] = fields.as_slice() else {
        return Err(INVALID_TARGET_TYPE_MESSAGE.to_string());
    };
    if *prefix != "arn"
        || !partition.starts_with("aws")
        || !TARGET_ARN_SERVICES.contains(service)
        || region.is_empty()
        || account.is_empty()
        || !account.bytes().all(|byte| byte.is_ascii_digit())
        || resource.is_empty()
    {
        return Err(INVALID_TARGET_TYPE_MESSAGE.to_string());
    }
    Ok(())
}

/// Refuse a `KMSKeyId` that is not a KMS key identifier at all.
///
/// The member takes "the Amazon Web Services Key Management System (Amazon Web Services
/// KMS) key identifier for the key used to encrypt the integration", which KMS documents
/// as a key id, a key ARN, an alias name or an alias ARN. A value of none of those forms
/// names no key, which is what the modelled `KMSKeyNotAccessibleFault` reports.
pub fn check_kms_key_id(value: &str) -> Result<(), String> {
    let is_alias_name = value.starts_with("alias/");
    let is_kms_arn = value.starts_with("arn:") && value.split(':').nth(2) == Some("kms");
    // A KMS key id is a 36-character lowercase uuid.
    let is_key_id = value.len() == 36
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f' | b'-'));
    if is_alias_name || is_kms_arn || is_key_id {
        return Ok(());
    }
    Err(format!(
        "invalid value '{value}' for KMSKeyId: expected a KMS key id, key ARN, alias name \
         or alias ARN"
    ))
}

/// A persisted zero-ETL integration.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct IntegrationRecord {
    /// `IntegrationName`, which is half this record's key.
    pub name: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// The minted, immutable id the integration's ARN carries.
    pub integration_id: String,
    /// `SourceArn`: the DB cluster or DB instance replication reads from.
    pub source_arn: String,
    /// `TargetArn`: the Amazon Redshift data warehouse replication writes to.
    pub target_arn: String,
    /// `KMSKeyId`.
    pub kms_key_id: Option<String>,
    /// `AdditionalEncryptionContext`.
    pub additional_encryption_context: HashMap<String, String>,
    /// `DataFilter`.
    pub data_filter: Option<String>,
    /// `Description`.
    pub description: Option<String>,
    /// `Status`.
    pub status: String,
    /// The status change the clock owes this record, if any.
    pub pending_status: Option<PendingStatus>,
    /// `CreateTime`.
    #[serde(with = "::service_common::persist::smithy_time")]
    pub create_time: ::aws_smithy_types::DateTime,
    /// `Tags`.
    pub tags: Vec<Tag>,
}

impl IntegrationRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, so the caller drops it from the store.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        if self.pending_status.as_ref().is_some_and(|p| now >= p.due_at) {
            let pending = self.pending_status.take().expect("checked just above");
            match pending.change {
                PendingChange::Status(status) => self.status = status,
                PendingChange::Removal => return true,
            }
        }
        false
    }

    /// `IntegrationArn`, from the stored region and the minted integration id.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("{ARN_RESOURCE_TYPE}:{}", self.integration_id),
        )
    }

    /// Accept a delete: the integration reports [`INTEGRATION_STATUS_DELETING`] from now on,
    /// and the clock owes its removal from the store a moment later, after which every read
    /// of the identifier is an `IntegrationNotFoundFault`. Nothing sleeps: the due instant is
    /// stored and materialised by
    /// [`super::ComAmazonawsRdsAmazonrdsv19State::settle_due_transitions`], exactly as every
    /// other accepted delete in this store does it.
    pub fn begin_deletion(&mut self) {
        self.status = INTEGRATION_STATUS_DELETING.to_string();
        self.pending_status = Some(PendingStatus::removal_in_a_moment());
    }

    /// Project the stored record onto the modelled `Integration` shape.
    ///
    /// `Errors` is reported EMPTY rather than left out: the member carries "any errors
    /// associated with the integration", and an integration this service holds has none.
    pub fn to_wire(&self) -> CreateIntegrationOutput {
        CreateIntegrationOutput {
            source_arn: Some(self.source_arn.clone()),
            target_arn: Some(self.target_arn.clone()),
            integration_name: Some(self.name.clone()),
            integration_arn: Some(self.arn()),
            kms_key_id: self.kms_key_id.clone(),
            additional_encryption_context: Some(self.additional_encryption_context.clone()),
            status: Some(self.status.clone()),
            tags: Some(self.tags.clone()),
            data_filter: self.data_filter.clone(),
            description: self.description.clone(),
            create_time: Some(self.create_time),
            errors: Some(Vec::new()),
        }
    }
}
