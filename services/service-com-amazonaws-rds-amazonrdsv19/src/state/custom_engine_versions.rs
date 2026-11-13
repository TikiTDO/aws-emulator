// SPDX-License-Identifier: CC0-1.0

//! The custom DB engine version (CEV) records `CreateCustomDBEngineVersion` writes.
//!
//! A CEV is the one engine version that is ACCOUNT state rather than this service's
//! published reference data: it exists only because a request created it from
//! customer-supplied installation media or from an Amazon Machine Image, and it is
//! reported only to the account and region that created it
//! (`CreateCustomDBEngineVersion$EngineVersion`: "The CEV name is unique per customer per
//! Amazon Web Services Regions"). The catalogue of AWS-PROVIDED engine versions stays
//! where it is, in `crate::ops::describe_db_engine_versions`, and that module projects a
//! record of this kind onto the `DBEngineVersion` wire shape, so one CEV reads the same
//! through the create that minted it and through the report that lists it.
//!
//! Only the record lives here: everything derived from it — its major version, its
//! parameter group family and the capability members of its engine — is computed from the
//! engine-version catalogue rather than stored twice.

use std::time::Instant;

use super::{PendingChange, PendingStatus, ARN_NAMESPACE, INVALID_DB_ENGINE_MESSAGE};
use crate::ops::types::{Tag, CUSTOM_ENGINE_VERSION_STATUS_VALUES};

/// How many custom engine versions one region holds. The cap is this emulator's, for the
/// reason every other per-region quota in `src/state.rs` exists: it bounds the store and
/// every scan over it. Reaching it is what the modelled
/// `CustomDBEngineVersionQuotaExceededFault` reports.
pub const CUSTOM_DB_ENGINE_VERSION_QUOTA_PER_REGION: usize = 20;

/// The `Status` a CEV reports once this service has finished creating it: the first value
/// `ModifyCustomDBEngineVersion$Status` publishes ("`available` — You can use this CEV to
/// create a new RDS Custom DB instance"), and the status `DescribeDBEngineVersions` lists
/// an engine version under by default. `ModifyCustomDBEngineVersion` is what moves a CEV
/// between this status and the other availability statuses afterwards ("you can change any
/// status to any status").
pub const CUSTOM_ENGINE_VERSION_STATUS_AVAILABLE: &str = "available";

/// The `Status` a CEV reports while this service is still creating it.
///
/// A CEV is built from media the account supplies, which RDS imports AFTER the create is
/// accepted — `ModifyCustomDBEngineVersion`'s own documentation names "the MediaImport
/// service that imports files from Amazon S3 to create CEVs" — so a create answers with a
/// CEV that is not usable yet, and this service owes it the move to
/// [`CUSTOM_ENGINE_VERSION_STATUS_AVAILABLE`] a moment later (`super::STATUS_TRANSITION_DELAY`).
/// It is not one of the availability statuses, so a CEV in it is neither reported by
/// default nor modifiable ([`custom_engine_version_is_modifiable`]).
pub const CUSTOM_ENGINE_VERSION_STATUS_CREATING: &str = "creating";

/// Whether a CEV in `status` is one `ModifyCustomDBEngineVersion` can change.
///
/// The availability statuses that operation's `Status` member publishes are exactly the
/// ones a CEV can be moved between — "you can change any status to any status" ranges over
/// those — so a CEV in any OTHER status is one whose creation has not settled yet, and
/// changing it is refused with the modelled `InvalidCustomDBEngineVersionStateFault` rather
/// than applied.
pub fn custom_engine_version_is_modifiable(status: &str) -> bool {
    CUSTOM_ENGINE_VERSION_STATUS_VALUES.contains(&status)
}

/// The engines a custom engine version can be created for, as
/// `CreateCustomDBEngineVersion$Engine` publishes them: the four RDS Custom for Oracle
/// engines, the four RDS Custom for SQL Server engines, and the RDS for SQL Server
/// Bring Your Own Media engines (`sqlserver-ee`, `sqlserver-se`) with
/// `sqlserver-dev-ee`. Alphabetical, so the refusal below lists them in one stable order.
pub const CUSTOM_ENGINE_VERSION_ENGINES: [&str; 11] = [
    "custom-oracle-ee",
    "custom-oracle-ee-cdb",
    "custom-oracle-se2",
    "custom-oracle-se2-cdb",
    "custom-sqlserver-dev",
    "custom-sqlserver-ee",
    "custom-sqlserver-se",
    "custom-sqlserver-web",
    "sqlserver-dev-ee",
    "sqlserver-ee",
    "sqlserver-se",
];

/// The ARN resource type of a custom engine version.
const ARN_RESOURCE_TYPE: &str = "cev";

/// Refuse an `Engine` no custom engine version can be created for.
///
/// The member's pattern (`^[A-Za-z0-9-]{1,35}$`) admits any name in any case, so the SET of
/// engines is checked here, against the value the request SENT: Amazon RDS publishes its
/// engine names as lowercase tokens and matches them exactly, so a value that differs from
/// a published name in nothing but case names no engine an account mints versions of
/// either. Either way the refusal is the service's own fixed text for a value that is no
/// engine of this operation — it states no rule and echoes nothing from the request — which
/// the caller answers with the Query API's `InvalidParameterValue`, the same code and the
/// same sentence [`super::check_engine`] answers an unpublished `Engine` elsewhere with.
pub fn check_custom_engine(engine: &str) -> Result<(), String> {
    if CUSTOM_ENGINE_VERSION_ENGINES.contains(&engine) {
        return Ok(());
    }
    Err(INVALID_DB_ENGINE_MESSAGE.to_string())
}

/// Refuse an `(Engine, EngineVersion)` pair that names no engine build a CEV can be
/// created on.
///
/// `CreateCustomDBEngineVersion$EngineVersion` documents the name format a CEV carries:
/// "for RDS Custom for Oracle, the name format is `19.`*customized_string*", and for the
/// SQL Server engines "*major_engine_version*.*minor_engine_version*.*customized_string*".
/// Either way the name BEGINS with the major engine version of the build it packages — that
/// is how the service resolves which DB engine version the CEV is built on, and it is what
/// [`crate::ops::describe_db_engine_versions::custom_version_basis`] reads the CEV's
/// `MajorEngineVersion` and `DBParameterGroupFamily` out of. A name whose leading component
/// is not a major engine version resolves no engine version at all, which the service
/// refuses with the same fixed `Invalid DB engine` sentence an `Engine` that names no engine
/// gets ([`check_custom_engine`]): in both cases the pair names no DB engine to build on.
///
/// That the two halves of the pair share ONE sentence is what the corpus records for this
/// operation's refusal, read as a statement about the pair rather than about the member;
/// should a later recording word the name-format half differently, this is the one place
/// that changes.
pub fn check_custom_engine_version_name(engine_version: &str) -> Result<(), String> {
    let (major, customized) = match engine_version.split_once('.') {
        Some(halves) => halves,
        // No separator at all: the whole name would have to be the major version, and then
        // it names a version the engine already publishes rather than a new CEV.
        None => return Err(INVALID_DB_ENGINE_MESSAGE.to_string()),
    };
    let names_a_major_version =
        !major.is_empty() && major.chars().all(|c| c.is_ascii_digit()) && !customized.is_empty();
    if names_a_major_version {
        return Ok(());
    }
    Err(INVALID_DB_ENGINE_MESSAGE.to_string())
}

/// Whether a CEV of `engine` is created from an Amazon Machine Image rather than from
/// installation files: RDS Custom for SQL Server, whose `ImageId` documentation states
/// that "for RDS Custom for SQL Server, an AMI ID is required to create a CEV".
pub fn is_ami_backed_custom_engine(engine: &str) -> bool {
    engine.starts_with("custom-sqlserver")
}

/// Whether `engine` reports a `DBEngineMediaType`.
///
/// The member is "applicable to RDS Custom for SQL Server and to RDS for SQL Server engine
/// versions (`sqlserver-ee` and `sqlserver-se` with the bring-your-own-media license
/// model, and `sqlserver-dev-ee`)" — the SQL Server engines, and no other.
pub fn reports_engine_media_type(engine: &str) -> bool {
    engine.contains("sqlserver")
}

/// The `DBEngineMediaType` of a custom engine version: "a value of `Customer Provided`
/// indicates that the engine version was created from customer-supplied installation media
/// using `CreateCustomDBEngineVersion`" — which is what every record here was.
pub const MEDIA_TYPE_CUSTOMER_PROVIDED: &str = "Customer Provided";

/// The ARN a custom engine version is NAMED by: the published resource form
/// `cev:<engine name>/<engine version name>/<CEV id>`, where the id is the minted,
/// immutable one that makes a single ARN name a single record even after a CEV of the same
/// name has been deleted and created again.
#[must_use]
pub fn custom_db_engine_version_resource_arn(
    region: &str,
    engine: &str,
    engine_version: &str,
    cev_id: &str,
) -> String {
    emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{ARN_RESOURCE_TYPE}:{engine}/{engine_version}/{cev_id}"),
    )
}

/// How a request this service will not serve against a custom engine version is refused:
/// ONE sentence, the same for every such request, which is why it takes no argument.
///
/// Managing a custom engine version — deleting it, modifying it — is withheld both from a
/// caller that may not manage CEVs at all and for a `(Engine, EngineVersion)` pair that
/// names no CEV the account holds, and the two are answered with the IDENTICAL text —
/// nothing in it is taken from the request, from the caller's identity or from the store,
/// so the refusal cannot be used to learn whether a version of that name exists or who
/// asked about it.
#[must_use]
pub fn custom_db_engine_version_denied_message() -> &'static str {
    "The custom engine version you requested may not exist or you don't have the required \
     permission."
}

/// The second half of a CEV's store key: one `(Engine, EngineVersion)` pair names one
/// custom engine version in a region, since the name "is unique per customer per Amazon
/// Web Services Regions" and two engines can carry the same version name.
pub fn custom_db_engine_version_key(engine: &str, engine_version: &str) -> String {
    format!("{engine}/{engine_version}")
}

/// A persisted custom DB engine version.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CustomDbEngineVersionRecord {
    /// `Engine`, folded to the lower case the published engine names are written in, and
    /// half this record's key.
    pub engine: String,
    /// `EngineVersion` — the CEV name, the other half of the key.
    pub engine_version: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// The minted, immutable id the CEV's ARN carries.
    pub cev_id: String,
    /// `MajorEngineVersion`, resolved from the CEV name when the record was written: the
    /// core attributes it is derived from cannot change, so it is resolved once.
    pub major_engine_version: String,
    /// `DBParameterGroupFamily`, resolved beside the major version.
    pub parameter_group_family: String,
    /// `Description` — reported as `DBEngineVersionDescription`.
    pub description: Option<String>,
    /// `Manifest` — reported as `CustomDBEngineVersionManifest`.
    pub manifest: Option<String>,
    /// `ImageId` — reported inside `Image`.
    pub image_id: Option<String>,
    /// `KMSKeyId`.
    pub kms_key_id: Option<String>,
    /// `DatabaseInstallationFilesS3BucketName`.
    pub database_installation_files_s3_bucket_name: Option<String>,
    /// `DatabaseInstallationFilesS3Prefix`.
    pub database_installation_files_s3_prefix: Option<String>,
    /// `DatabaseInstallationFiles`.
    pub database_installation_files: Vec<String>,
    /// `Status`.
    pub status: String,
    /// The status change the clock owes this record, if any: the move out of
    /// [`CUSTOM_ENGINE_VERSION_STATUS_CREATING`] a create owes it.
    pub pending_status: Option<PendingStatus>,
    /// `CreateTime`.
    #[serde(with = "::service_common::persist::smithy_time")]
    pub create_time: ::aws_smithy_types::DateTime,
    /// `Tags`, reported as `TagList`.
    pub tags: Vec<Tag>,
}

impl CustomDbEngineVersionRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, so the caller drops it from the store.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        let due = self
            .pending_status
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at);
        if !due {
            return false;
        }
        match self.pending_status.take().map(|pending| pending.change) {
            Some(PendingChange::Status(status)) => self.status = status,
            Some(PendingChange::Removal) => return true,
            // Unreachable: the change was there a line ago, under this one `&mut self`.
            // Answered as "nothing was due" rather than with a panic in a request path.
            None => {}
        }
        false
    }

    /// This record's store key within its region.
    pub fn key(&self) -> String {
        custom_db_engine_version_key(&self.engine, &self.engine_version)
    }

    /// `DBEngineVersionArn` — "the ARN of the custom engine version": the engine and the
    /// CEV name it is known by, plus its minted id, in the one ARN form above.
    pub fn arn(&self) -> String {
        custom_db_engine_version_resource_arn(
            &self.region,
            &self.engine,
            &self.engine_version,
            &self.cev_id,
        )
    }
}
