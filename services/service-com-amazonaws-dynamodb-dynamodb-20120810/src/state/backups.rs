// SPDX-License-Identifier: CC0-1.0

//! On-demand backups: the catalogue a table's COPIES live in.
//!
//! Their status vocabularies, the ARNs they are addressed by, the snapshot a backup
//! keeps of its source table, and the window a listing reports them within. They live
//! beside [`super`] rather than inside it because they are about a COPY of a table taken
//! at one instant, while the parent module is the live store; `super` re-exports them, so
//! every caller still names them `crate::state::…`. The import and export catalogues
//! stay in `state.rs`, which is where every caller reaches them.

use super::*;

/// The `BackupStatus` a backup is in while it is still being taken: "the backup is
/// created asynchronously" (the `CreateBackup` doc), so a backup is `CREATING` for a
/// span before it is ready.
pub const BACKUP_STATUS_CREATING: &str = "CREATING";

/// The `BackupStatus` a backup is in once that span has elapsed and it can be restored
/// from.
pub const BACKUP_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `BackupStatus` reported for the backup a `DeleteBackup` has just removed — the
/// third and last value the model's `BackupStatus` enum declares
/// ([`crate::ops::types::BACKUP_STATUS_VALUES`]).
pub const BACKUP_STATUS_DELETED: &str = "DELETED";

/// The `BackupType` of a backup a CALLER asked for: "USER - On-demand backup created by
/// you" (`ListBackups$BackupType`). Every backup this API catalogues is one.
pub const BACKUP_TYPE_USER: &str = "USER";

/// The `BackupType` of a backup the service took by itself: "SYSTEM - On-demand backup
/// automatically created by DynamoDB" (`ListBackups$BackupType`). Nothing here takes a
/// backup on its own, so no stored backup carries it — it is named only by the filter
/// rule below.
pub const BACKUP_TYPE_SYSTEM: &str = "SYSTEM";

/// The `BackupTypeFilter` a list that names none is answered for: "USER - On-demand
/// backup created by you. (The default setting if no other backup types are specified.)"
/// (`ListBackups$BackupType`).
pub const DEFAULT_BACKUP_TYPE_FILTER: &str = "USER";

/// The `BackupTypeFilter` that spans several types at once: "ALL - All types of
/// on-demand backups (USER and SYSTEM)" (`ListBackups$BackupType`).
pub const BACKUP_TYPE_FILTER_ALL: &str = "ALL";

/// The most backups this emulator will hold IN ONE REGION.
///
/// AWS states no ceiling — "There is no limit to the number of on-demand backups that
/// can be taken" (the `CreateBackup` doc) — so this one is this emulator's own, and it
/// is the per-region table quota over again ([`MAX_TABLES`]): a backup is catalogued
/// from a table, and this keeps the catalogue of one region bounded by the number of
/// tables that region may hold. Reaching it answers the modelled
/// `LimitExceededException` rather than growing without end.
///
/// It bounds the whole catalogue too: a backup is only ever catalogued from a table of
/// its own region, and the table store holds at most [`MAX_REGIONS`] regions, so the
/// records here can never outnumber `MAX_REGIONS * MAX_BACKUPS_PER_REGION`.
pub const MAX_BACKUPS_PER_REGION: usize = MAX_TABLES;

/// How many bytes of ITEM DATA the whole backup catalogue may hold.
///
/// A backup keeps the items its source table held at the instant it was taken — that is
/// what a restore writes into the new table — so the catalogue is a growth site of its
/// own, beside the tables themselves. AWS states no ceiling here either, so this is the
/// table store's own budget over again ([`MAX_STORE_SIZE_BYTES`]): a `CreateBackup` that
/// would push the catalogue past it answers the modelled `LimitExceededException`
/// instead of growing the process without end.
pub const MAX_BACKUP_CATALOGUE_BYTES: i64 = MAX_STORE_SIZE_BYTES;

/// How many restores may be in flight at once: "Any number of users can execute up to 50
/// concurrent restores (any type of restore) in a given account" (the
/// `RestoreTableFromBackup` doc, repeated on `RestoreTableToPointInTime`). A restore past
/// it answers the modelled `LimitExceededException`.
pub const MAX_CONCURRENT_RESTORES: usize = 50;

/// Whether the backup path can ACT on a table that is in this status.
///
/// A table becomes one a backup operation can work on only once its creation has
/// finished, and stops being one as soon as its deletion has started: in either
/// transient status there is no table for the backup path to take a copy of or to hold
/// continuous-backups settings on, so the operation reports no table under that name
/// ([`table_not_found_message`]) instead of a conflict a caller would retry in place. A
/// table whose configuration is being updated IS a table, and is backed up.
pub fn backup_path_acts_on(status: TableStatus) -> bool {
    matches!(status, TableStatus::Active | TableStatus::Updating)
}

/// How a backup-path operation refuses a table it has none of: the sentence
/// `TableNotFoundException` carries, whose fixed text is `Table not found: ` and whose
/// variable part is the table the request named.
///
/// One wording for every operation that models that error, so a table the backup path
/// cannot address is refused in the same words whether it was never there, has gone, or
/// has not finished being created.
pub fn table_not_found_message(table_name: &str) -> String {
    format!("Table not found: {table_name}")
}

/// Whether a stored `BackupType` is one a request's `BackupTypeFilter` selects.
///
/// One rule for every filter value the model declares: `ALL` spans the on-demand types
/// this API catalogues, and any other value selects exactly the type it names. A backup
/// "made with Amazon Web Services Backup" is listed by that service's own API rather
/// than this one (the `ListBackups` doc), so none is ever stored here to select.
pub fn backup_type_selected(filter: &str, stored: &str) -> bool {
    match filter {
        BACKUP_TYPE_FILTER_ALL => stored == BACKUP_TYPE_USER || stored == BACKUP_TYPE_SYSTEM,
        named => named == stored,
    }
}

/// Whether `instant` falls inside the time range a request named: at or after the lower
/// bound and strictly before the upper one — "In the request, start time is inclusive,
/// but end time is exclusive" (the `ListBackups` doc). A bound the request omits bounds
/// nothing.
///
/// Both sides are compared at [`at_timestamp_precision`], the precision a reported
/// timestamp carries, so a bound a client formed from an instant this service reported
/// selects exactly what it names rather than landing a fraction either side of it.
pub fn within_time_range(
    instant: SystemTime,
    lower: Option<&::aws_smithy_types::DateTime>,
    upper: Option<&::aws_smithy_types::DateTime>,
) -> bool {
    let instant = epoch_millis(instant);
    lower.is_none_or(|bound| instant >= wire_millis(bound))
        && upper.is_none_or(|bound| instant < wire_millis(bound))
}

/// A time range a request names must be a range: its lower bound at or before its upper
/// one.
///
/// A window whose bounds are the wrong way round selects nothing whatever is stored, so
/// it states no window at all — the request contradicts itself and is refused before
/// anything is listed, rather than answered with the empty page the inversion would
/// produce. One bound on its own is a half-open window and bounds the other side of
/// nothing, so it is a range.
pub fn check_time_range(
    lower: Option<&::aws_smithy_types::DateTime>,
    upper: Option<&::aws_smithy_types::DateTime>,
) -> Result<(), RequestError> {
    let (Some(lower), Some(upper)) = (lower, upper) else {
        return Ok(());
    };
    if wire_millis(lower) <= wire_millis(upper) {
        return Ok(());
    }
    Err(RequestError::Validation(
        "One or more parameter values were invalid: Time range lower bound must be less than or \
         equal to upper bound"
            .to_string(),
    ))
}

/// How precise an instant this service reports and compares is: whole milliseconds, the
/// precision an AWS timestamp carries on the wire — and the one DynamoDB's own backup
/// identifier is built from, since a `BackupArn`'s last segment is the epoch millisecond
/// of the request.
///
/// This returns `instant` at that precision, which is what a record stores: the instant
/// it reports is then exactly the instant a client can send back.
pub fn at_timestamp_precision(instant: SystemTime) -> SystemTime {
    match instant.duration_since(UNIX_EPOCH) {
        Ok(since) => {
            UNIX_EPOCH
                + Duration::from_millis(u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        }
        // An instant before the epoch is not one this store mints; it is left as it is
        // rather than moved.
        Err(_) => instant,
    }
}

/// `instant` at the NEAREST reported timestamp, for an instant a client ECHOED BACK.
///
/// [`at_timestamp_precision`] truncates, which is right for an instant this service is
/// MINTING. A value that has travelled out through the wire's decimal form and back can
/// land a fraction either side of the instant it names, so one being read is rounded
/// instead: a client that sends back a timestamp this service reported then names exactly
/// that instant rather than the millisecond below it.
pub fn at_nearest_timestamp_precision(instant: SystemTime) -> SystemTime {
    match instant.duration_since(UNIX_EPOCH) {
        Ok(since) => {
            let millis = since.as_nanos().saturating_add(500_000) / 1_000_000;
            // An instant so far ahead that rounding it cannot be represented is left as
            // it is rather than wrapped: the value arrives from a request, so no caller
            // can steer this into an overflow.
            UNIX_EPOCH
                .checked_add(Duration::from_millis(u64::try_from(millis).unwrap_or(u64::MAX)))
                .unwrap_or(instant)
        }
        Err(_) => instant,
    }
}

pub(crate) fn epoch_millis(instant: SystemTime) -> i128 {
    match instant.duration_since(UNIX_EPOCH) {
        Ok(since) => i128::try_from(since.as_millis()).unwrap_or(i128::MAX),
        Err(before) => i128::try_from(before.duration().as_millis())
            .map_or(i128::MIN, |millis| -millis),
    }
}

/// The same figure for a timestamp a request carried.
fn wire_millis(timestamp: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(timestamp.secs()) * 1_000 + i128::from(timestamp.subsec_nanos() / 1_000_000)
}

/// A new backup's `BackupArn`:
/// `arn:aws:dynamodb:<region>:<account>:table/<table>/backup/<millis>-<id>`, built
/// through the same core ARN helper every other identifier here is minted with
/// ([`table_arn`]).
///
/// The last segment is the instant the backup was requested, in zero-padded epoch
/// milliseconds, followed by a unique suffix drawn from the `uuid` crate's v4 generator
/// (OS randomness): the instant alone does not identify a backup, because two calls can
/// land in the same millisecond. Minted ONCE, at create, and stored on the backup, so
/// every later read reports the same ARN — and because the fixed-width instant sits
/// inside it, ascending ARN order IS ascending (table, request instant) order, which is
/// what the `ExclusiveStartBackupArn` cursor pages through.
pub fn new_backup_arn(region: &str, table: &str, requested_at: SystemTime) -> String {
    let millis = requested_at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let unique = ::uuid::Uuid::new_v4().simple().to_string();
    ::engine::emulator_arn(
        "dynamodb",
        region,
        &format!(
            "table/{table}/backup/{millis:014}-{}",
            unique.get(..8).unwrap_or(unique.as_str())
        ),
    )
}

/// One stored on-demand backup: what the request asked for, when it was catalogued, and
/// the snapshot of the source table taken at that instant.
///
/// A backup is a RESOURCE OF ITS OWN rather than part of a table: it is addressed by its
/// own ARN, it outlives the table it was taken from, and every member a backup operation
/// reports is read from here.
#[derive(Debug, Clone)]
pub struct Backup {
    /// The `BackupArn` minted for this backup at create ([`new_backup_arn`]), which is
    /// also the key the store holds it under.
    pub arn: String,
    /// The `BackupName` the request named.
    pub name: String,
    /// The region this backup lives in — the one its source table lives in, which is the
    /// scope every backup operation resolves inside.
    pub region: String,
    pub table_name: String,
    pub table_id: String,
    pub table_arn: String,
    /// `USER` for every backup catalogued through `CreateBackup` ([`BACKUP_TYPE_USER`]).
    pub backup_type: String,
    /// The bytes the source table held at the instant of the request, which is what this
    /// backup holds: "the entire table data is backed up" (the `CreateBackup` doc).
    pub size_bytes: i64,
    /// When the backup was requested: "a time marker of the request is cataloged" (the
    /// `CreateBackup` doc), which is the `BackupCreationDateTime` every read reports and
    /// the instant `ListBackups`'s time range is measured against ("these boundaries are
    /// for the time at which the original backup was requested" — the `ListBackups` doc).
    pub created_at: SystemTime,
    /// When the clock owes this backup its `CREATING` -> `AVAILABLE` transition, one
    /// [`STATUS_TRANSITION`] after the request: the backup "is created asynchronously"
    /// and becomes "available for restore" afterwards (the `CreateBackup` doc). Derived
    /// from the stored instant on every read, so nothing sleeps and no read writes.
    pub available_at: SystemTime,
    /// When this backup expires by itself, for a kind that does: "Time at which the
    /// automatic on-demand backup created by DynamoDB will expire" — a `SYSTEM` backup's
    /// own member (`BackupDetails$BackupExpiryDateTime`). A backup a caller asked for
    /// does not expire, so this is `None` for every backup this API catalogues.
    pub expires_at: Option<SystemTime>,
    /// When the restore in flight from this backup finishes, while one is: a backup "is
    /// either being created, deleted or restored to a table" (`BackupInUseException`),
    /// so for that span the backup is in use and no second backup control-plane call may
    /// have it. `None` once no restore is in flight; whether the span has elapsed is
    /// derived from the clock on every read, so nothing sleeps.
    pub restored_until: Option<SystemTime>,
    /// The source table as it stood when the backup was requested.
    pub source: SourceTableSnapshot,
    /// The ITEMS the source table held at the instant the backup was requested, which is
    /// the data this backup holds: "the entire table data is backed up" (the
    /// `CreateBackup` doc). Copied HERE rather than read off the table later, because a
    /// backup outlives the table it was taken from — and they are what
    /// `RestoreTableFromBackup` writes into the table it creates.
    pub items: BTreeMap<ItemKey, Item>,
}

/// The source table as a backup records it: the configuration and figures
/// `SourceTableDetails` and `SourceTableFeatureDetails` report for the backup, frozen at
/// the instant the backup was requested rather than read off the table later — the table
/// may have moved on, or be gone.
#[derive(Debug, Clone)]
pub struct SourceTableSnapshot {
    pub key_schema: Vec<KeySchemaElement>,
    /// The attribute definitions the source table declared. No backup reader reports
    /// them — `SourceTableDetails` carries none — but they are part of the table's
    /// schema, so a table restored from this backup declares the same ones.
    pub attribute_definitions: Vec<AttributeDefinition>,
    pub table_creation_date_time: SystemTime,
    pub billing_mode: String,
    /// The capacity the table was provisioned for, as a description reports it: zeroes
    /// for an on-demand table ([`reported_capacity`]).
    pub provisioned_throughput: ProvisionedThroughput,
    pub item_count: i64,
    pub table_size_bytes: i64,
    pub global_secondary_indexes: Vec<GlobalSecondaryIndexInfo>,
    pub local_secondary_indexes: Vec<LocalSecondaryIndexInfo>,
    /// The vector indexes the source table carried, as `SourceTableFeatureDetails`
    /// reports them — and the configuration a table restored from this backup rebuilds
    /// them from, since "all vector indexes from the backup are restored"
    /// (`RestoreTableFromBackup$VectorIndexOverride`).
    pub vector_indexes: Vec<VectorIndexInfo>,
    /// The table's stream configuration, for a table that had a stream.
    pub stream_specification: Option<StreamSpecification>,
    /// The table's Time to Live SETTING as it stood then — what the last accepted
    /// `UpdateTimeToLive` had asked for, and when the clock owes that change its move out
    /// of the transient status — which is what
    /// `SourceTableFeatureDetails$TimeToLiveDescription` is projected from for this backup.
    ///
    /// The setting is copied rather than the DESCRIPTION it projected at that instant: a
    /// transient status (`ENABLING` / `DISABLING`) is not a state a record can be frozen
    /// in, because it is the half-finished half of a change the clock still owes, and that
    /// change settles on its own clock whether or not anything reads it. So this snapshot
    /// holds the setting and the status is derived at every read, exactly as the source
    /// table's own reader derives it — the copy is still frozen, since a later change to
    /// the table's TTL is a change to the table's own setting and not to this one.
    pub time_to_live: TimeToLive,
}

impl Backup {
    /// The backup a `CreateBackup` at `now` catalogues from `table`.
    pub fn taken_from(table: &Table, backup_name: &str, now: SystemTime) -> Self {
        // The time marker this backup is catalogued under, at the precision it is
        // reported in, so the instant in its ARN, the instant it reports and the instant
        // a client can send back as a range bound are all the same one.
        let created_at = at_timestamp_precision(now);
        Self {
            arn: new_backup_arn(&table.region, &table.name, created_at),
            name: backup_name.to_string(),
            region: table.region.clone(),
            table_name: table.name.clone(),
            table_id: table.id.clone(),
            table_arn: table.arn.clone(),
            backup_type: BACKUP_TYPE_USER.to_string(),
            size_bytes: table.stored_bytes,
            created_at,
            available_at: now + STATUS_TRANSITION,
            expires_at: None,
            restored_until: None,
            source: table.backup_snapshot(),
            items: table.items.clone(),
        }
    }

    /// The status at `now`, derived from the stored instants.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.available_at {
            BACKUP_STATUS_CREATING
        } else {
            BACKUP_STATUS_AVAILABLE
        }
    }

    /// Whether the clock still owes this backup its `CREATING` -> `AVAILABLE`
    /// transition, which is what a conflicting backup operation on the table is.
    pub fn still_creating(&self, now: SystemTime) -> bool {
        now < self.available_at
    }

    /// Whether a restore from this backup is still in flight at `now`.
    pub fn still_restoring(&self, now: SystemTime) -> bool {
        self.restored_until.is_some_and(|until| now < until)
    }

    /// Whether another backup control-plane operation still has this backup, so a call
    /// that needs it to itself is refused: "The backup is either being created, deleted
    /// or restored to a table" (`BackupInUseException`).
    pub fn in_use(&self, now: SystemTime) -> bool {
        self.still_creating(now) || self.still_restoring(now)
    }

    /// How a backup another operation still has is refused, naming the operation that
    /// has it — ONE sentence per in-flight operation, so the two callers that report
    /// `BackupInUseException` cannot word the same conflict differently. A backup that is
    /// in use for neither reason has nothing to report.
    pub fn in_use_message(&self, now: SystemTime) -> Option<String> {
        if self.still_creating(now) {
            return Some(format!("Backup is being created: {}", self.arn));
        }
        if self.still_restoring(now) {
            return Some(format!("Backup is being used to restore table: {}", self.arn));
        }
        None
    }

    /// Project this backup into the `BackupDetails` every backup operation reports,
    /// in the status the CALLING operation reports it in.
    pub fn details(&self, status: &str) -> BackupDetails {
        BackupDetails {
            backup_arn: self.arn.clone(),
            backup_name: self.name.clone(),
            backup_size_bytes: Some(self.size_bytes),
            backup_status: status.to_string(),
            backup_type: self.backup_type.clone(),
            backup_creation_date_time: super::reported_date(self.created_at),
            backup_expiry_date_time: self.expires_at.map(super::reported_date),
        }
    }

    /// Project this backup into the `BackupSummary` a list reports, as it stands at
    /// `now`.
    pub fn summary(&self, now: SystemTime) -> BackupSummary {
        BackupSummary {
            table_name: Some(self.table_name.clone()),
            table_id: Some(self.table_id.clone()),
            table_arn: Some(self.table_arn.clone()),
            backup_arn: Some(self.arn.clone()),
            backup_name: Some(self.name.clone()),
            backup_creation_date_time: Some(super::reported_date(self.created_at)),
            backup_expiry_date_time: self.expires_at.map(super::reported_date),
            backup_status: Some(self.status(now).to_string()),
            backup_type: Some(self.backup_type.clone()),
            backup_size_bytes: Some(self.size_bytes),
        }
    }

    /// Project this backup into the `BackupDescription` the two single-backup operations
    /// report: the backup itself, plus the source table as this backup recorded it, as the
    /// recorded setting stands at `now`.
    pub fn describe(&self, status: &str, now: SystemTime) -> BackupDescription {
        BackupDescription {
            backup_details: Some(self.details(status)),
            source_table_details: Some(SourceTableDetails {
                table_name: self.table_name.clone(),
                table_id: self.table_id.clone(),
                table_arn: Some(self.table_arn.clone()),
                table_size_bytes: Some(self.source.table_size_bytes),
                key_schema: self.source.key_schema.clone(),
                table_creation_date_time: super::reported_date(
                    self.source.table_creation_date_time,
                ),
                provisioned_throughput: self.source.provisioned_throughput.clone(),
                on_demand_throughput: None,
                item_count: Some(self.source.item_count),
                billing_mode: Some(self.source.billing_mode.clone()),
            }),
            source_table_feature_details: Some(SourceTableFeatureDetails {
                // A table that had no secondary index of a kind reports no list for it,
                // exactly as its own description does.
                local_secondary_indexes: (!self.source.local_secondary_indexes.is_empty())
                    .then(|| self.source.local_secondary_indexes.clone()),
                global_secondary_indexes: (!self.source.global_secondary_indexes.is_empty())
                    .then(|| self.source.global_secondary_indexes.clone()),
                vector_indexes: (!self.source.vector_indexes.is_empty())
                    .then(|| self.source.vector_indexes.clone()),
                stream_description: self.source.stream_specification.clone(),
                // The recorded setting, projected exactly as the table's own TTL reader
                // projects it, so a status the clock has settled is reported settled here
                // too.
                time_to_live_description: Some(self.source.time_to_live.describe(now)),
                ..SourceTableFeatureDetails::default()
            }),
        }
    }
}

/// The `BackupNotFoundException` message a backup operation answers an ARN it holds no
/// backup for with: built from the request — the ARN the caller addressed — and nothing
/// about what the catalogue does hold.
pub fn backup_not_found(backup_arn: &str) -> RequestError {
    RequestError::NotFound(format!("Backup not found: {backup_arn}"))
}

/// How a write this emulator's own host-protection ceilings cannot admit is refused
/// ([`MAX_ITEMS_PER_TABLE`], [`MAX_TABLE_SIZE_BYTES`], [`MAX_STORE_SIZE_BYTES`]).
///
/// NOT an AWS quota, and not a value the request got wrong: DynamoDB bounds ONE item and
/// not what a table holds, and these ceilings sit far above the data any workload stores.
/// So the message names THE EMULATOR as what ran out, and says the service itself imposes
/// no such limit, rather than sending a client to fix its own input; it is the one refusal
/// this store answers for its own exhaustion.
pub(crate) fn store_exhausted(table: &str) -> RequestError {
    RequestError::Validation(format!(
        "This emulator cannot hold more data for table {table}: its in-process item store \
         is full (at most {MAX_ITEMS_PER_TABLE} items and {MAX_TABLE_SIZE_BYTES} bytes per \
         table, and {MAX_STORE_SIZE_BYTES} bytes across all tables). DynamoDB itself \
         imposes no such limit; delete items to make room."
    ))
}

/// The `ValidationException` message every backup-addressed operation answers a
/// `BackupArn` it cannot read as a backup ARN of this endpoint with.
///
/// AWS's own text, fixed: it states the member's kind and nothing else — no `<N>
/// validation error(s) detected: ` envelope, no constraint clause and no echo of the
/// value that was sent, whichever way the value failed.
pub const INVALID_BACKUP_ARN_MESSAGE: &str = "Invalid Backup ARN";

/// Hold an ARN-addressed BACKUP operation's `BackupArn` to the form such an ARN takes in
/// the scope the request itself was made in.
///
/// The same two-part rule [`resolve_resource_arn`] states for a table ARN, over the
/// deeper form a backup carries
/// (`arn:<partition>:dynamodb:<region>:<account>:table/<table>/backup/<id>`): the value
/// must be an ARN of this service naming a backup, and it must address the partition,
/// account and REGION this request reached — which is exactly the prefix [`table_arn`]
/// mints for a table of this region. Anything else names no backup whose existence could
/// be looked up, so it is an INPUT failure ([`RequestError::Validation`]) rather than a
/// missing resource, and only a well-formed, in-scope ARN reaches the catalogue.
///
/// However the value fails, the refusal is AWS's FIXED sentence for this member
/// ([`INVALID_BACKUP_ARN_MESSAGE`]): one text for "that is not a backup ARN I can
/// address", with no clause envelope around it and the rejected value NOT echoed back.
pub fn check_backup_arn(backup_arn: &str, request_region: &str) -> Result<(), RequestError> {
    let invalid = || RequestError::Validation(INVALID_BACKUP_ARN_MESSAGE.to_string());
    // `arn`, partition, service, region, account, resource — the resource field is the
    // last one and carries `/`, so the split stops at six.
    let fields: Vec<&str> = backup_arn.splitn(6, ':').collect();
    let resource = match fields.as_slice() {
        ["arn", _partition, "dynamodb", _region, _account, resource] => *resource,
        _ => return Err(invalid()),
    };
    let (table, backup) = resource
        .strip_prefix("table/")
        .and_then(|path| path.split_once("/backup/"))
        .ok_or_else(invalid)?;
    if backup.is_empty() || backup.contains('/') {
        return Err(invalid());
    }
    let table = resolve_table_name(table).map_err(|_| invalid())?;
    if backup_arn != format!("{}/backup/{backup}", table_arn(request_region, &table)) {
        return Err(invalid());
    }
    Ok(())
}






