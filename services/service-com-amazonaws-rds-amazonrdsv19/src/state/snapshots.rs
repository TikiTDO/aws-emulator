// SPDX-License-Identifier: CC0-1.0

use super::*;

/// A stored DB CLUSTER snapshot: the cluster's settings as they stood when the snapshot
/// was taken, which is why they are copied rather than re-read later.
///
/// `CreateDBClusterSnapshot` writes one (a snapshot OF a cluster, through
/// [`DbClusterSnapshotRecord::from_cluster`]), `CopyDBClusterSnapshot` writes one (a copy
/// of another snapshot), the final snapshot `DeleteDBCluster` takes is one more and
/// `DeleteDBClusterSnapshot` removes one; `RestoreDBClusterFromSnapshot` is what READS one
/// back into a cluster. The reading operations are implemented against these records rather
/// than against a canned answer, so they report exactly what is stored.
#[derive(Debug, Clone)]
pub struct DbClusterSnapshotRecord {
    /// `DBClusterSnapshotIdentifier`, lowercased, and this record's key within its
    /// region: the member documents "This value is stored as a lowercase string".
    pub snapshot_identifier: String,
    /// The region the snapshot lives in; the scope of its ARN.
    pub region: String,
    /// `DBClusterIdentifier` of the cluster the snapshot was taken from.
    pub db_cluster_identifier: String,
    /// `DbClusterResourceId`, which `DescribeDBClusterSnapshots` can select on.
    pub db_cluster_resource_id: String,
    pub engine: String,
    pub engine_version: Option<String>,
    pub engine_mode: Option<String>,
    /// `DatabaseName` of the cluster the snapshot was taken from — the database its data
    /// lives in — which is the name a cluster restored from this snapshot carries when the
    /// restore names none of its own. Not a member of the `DBClusterSnapshot` wire shape:
    /// it is one of the cluster settings the snapshot copied, like the ones below.
    pub database_name: Option<String>,
    /// `DBClusterInstanceClass` of the cluster the snapshot was taken from — the compute
    /// and memory capacity of each DB instance in a Multi-AZ DB cluster — which is the
    /// class a DB instance restored from this snapshot is provisioned with when the
    /// request names none. `None` for a cluster that carries no instance class of its own
    /// (an Aurora cluster, whose snapshots a DB instance cannot be restored from).
    pub db_instance_class: Option<String>,
    /// `AllocatedStorage` of the cluster the snapshot was taken from, which the snapshot
    /// reports until its own size is known; see [`Self::to_wire`].
    pub allocated_storage: Option<i32>,
    /// Whether the cluster the snapshot was taken from had a DB INSTANCE in it, i.e.
    /// whether the snapshot holds any data at all.
    pub holds_data: bool,
    pub master_username: Option<String>,
    /// The master user's password as the source cluster held it, kept only as the DIGESTS a
    /// login can be verified against — the password itself is `@sensitive`, is never
    /// reported and is not stored ([`data_plane::wire_session::MasterPasswordHash`]). It is
    /// part of what the snapshot copied off the cluster, because a cluster restored from it
    /// keeps the source's master credential: the restore mints no new one. `None` for a
    /// source cluster that had no master password of its own.
    pub master_user_password: Option<data_plane::wire_session::MasterPasswordHash>,
    pub availability_zones: Vec<String>,
    pub port: i32,
    pub storage_type: Option<String>,
    pub storage_encrypted: bool,
    pub kms_key_id: Option<String>,
    pub license_model: Option<String>,
    /// `VpcId`: the VPC of the cluster the snapshot was taken from.
    pub vpc_id: String,
    /// `BackupRetentionPeriod`, as the source cluster retained automated backups when the
    /// snapshot was taken.
    pub backup_retention_period: Option<i32>,
    /// `PreferredBackupWindow`, as the source cluster carried it when the snapshot was
    /// taken.
    pub preferred_backup_window: Option<String>,
    /// `IAMDatabaseAuthenticationEnabled`, as the source cluster carried it.
    pub iam_database_authentication_enabled: bool,
    /// `SnapshotType`: `manual` for every snapshot a caller asked for.
    pub snapshot_type: String,
    /// `Status`: [`SNAPSHOT_STATUS_COPYING`] while a copy is being made,
    /// [`SNAPSHOT_STATUS_AVAILABLE`] once the clock has made it due.
    pub status: String,
    /// The status this record owes, and when; see [`PendingStatus`].
    pub pending_status: Option<PendingStatus>,
    pub cluster_create_time: ::aws_smithy_types::DateTime,
    pub snapshot_create_time: ::aws_smithy_types::DateTime,
    /// `SourceDBClusterSnapshotArn`: the snapshot this one was copied from, absent for a
    /// snapshot taken from a cluster.
    pub source_snapshot_arn: Option<String>,
    /// The accounts the `restore` attribute authorizes, which
    /// `ModifyDBClusterSnapshotAttribute` adds to and removes from and
    /// `DescribeDBClusterSnapshotAttributes` reports; see [`SNAPSHOT_RESTORE_ATTRIBUTE`].
    /// A set, so authorizing an account twice changes nothing and the report is ordered.
    pub restore_attribute: BTreeSet<String>,
    /// A COPY of the data the source cluster's storage held when the snapshot was taken,
    /// taken by the engine itself ([`data_plane::sql_engine::InstanceDatabase::capture`]):
    /// this is what makes a cluster snapshot restorable, because
    /// `RestoreDBClusterFromSnapshot` loads it into the restored cluster's own storage.
    /// `None` for a snapshot of a cluster that has no engine endpoint at all — an engine
    /// whose wire this emulator serves no data plane for
    /// ([`data_plane::cluster_endpoint::cluster_engine_wire`]).
    ///
    /// The copy is in memory and private to this snapshot, it is never written to the host,
    /// and its size is bounded by the same per-database page ceiling every database of this
    /// service's data plane carries; the number of copies is bounded by
    /// [`DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION`] and [`MAX_REGIONS`], like every other record
    /// here. Deleting the snapshot drops it.
    pub storage: Option<::std::sync::Arc<data_plane::sql_engine::InstanceDatabase>>,
    pub tags: Vec<Tag>,
}

impl DbClusterSnapshotRecord {
    /// Take a snapshot OF `cluster`: its settings as they stand now, copied onto the
    /// snapshot so a later change to the cluster cannot rewrite what the snapshot holds.
    ///
    /// `SnapshotType` is `manual` — the class AWS gives every snapshot a caller asked for,
    /// `automated` being reserved for the backups the service schedules itself — and the
    /// snapshot answers [`SNAPSHOT_STATUS_CREATING`] with the clock owing it
    /// [`SNAPSHOT_STATUS_AVAILABLE`], so a caller that reads it straight back sees the
    /// snapshot still being taken. `source_snapshot_arn` is absent because nothing was
    /// copied: that member names "the DB cluster snapshot that was copied to create this
    /// one".
    /// `storage` is the copy of the cluster's storage the caller took before it locked the
    /// store ([`capture_cluster_storage`]), so the settings the snapshot reports and the
    /// data it holds are from one instant.
    pub fn from_cluster(
        cluster: &DbClusterRecord,
        cluster_has_instances: bool,
        snapshot_identifier: &str,
        storage: Option<::std::sync::Arc<data_plane::sql_engine::InstanceDatabase>>,
        tags: Vec<Tag>,
    ) -> Self {
        Self {
            snapshot_identifier: snapshot_identifier.to_string(),
            region: cluster.region.clone(),
            db_cluster_identifier: cluster.identifier.clone(),
            db_cluster_resource_id: cluster.resource_id.clone(),
            engine: cluster.engine.clone(),
            engine_version: cluster.engine_version.clone(),
            engine_mode: Some(cluster.engine_mode.clone()),
            database_name: cluster.database_name.clone(),
            db_instance_class: cluster.db_cluster_instance_class.clone(),
            // "The allocated storage size of the DB cluster snapshot in gibibytes (GiB)":
            // the storage of the cluster the snapshot is being taken from, which is what
            // the API reports until the snapshot's own size is known ([`Self::to_wire`]).
            allocated_storage: cluster.reported_allocated_storage(),
            holds_data: cluster_has_instances,
            master_username: cluster.master_username.clone(),
            // The credential a restored cluster logs its master user in with; see the
            // field's own note.
            master_user_password: cluster.master_user_password.clone(),
            availability_zones: cluster.availability_zones.clone(),
            // "The port that the DB cluster was listening on at the time of the snapshot":
            // a fact about the cluster's DB INSTANCES, so it is the port the cluster
            // PUBLISHES — the one a client was connecting to. A cluster with none had
            // nothing listening, so the snapshot was taken on no port at all.
            port: if cluster_has_instances {
                cluster.published_port()
            } else {
                0
            },
            storage_type: cluster.storage_type.clone(),
            storage_encrypted: cluster.storage_encrypted,
            kms_key_id: cluster.kms_key_id.clone(),
            // The license the engine the snapshot was taken from runs under; see
            // [`default_license_model`] for where each engine's comes from.
            license_model: default_license_model(&cluster.engine).map(str::to_string),
            // The cluster settings the snapshot reports members of its own for, copied here
            // with the rest so a later change to the cluster cannot rewrite what the
            // snapshot reports.
            vpc_id: cluster.vpc_id(),
            backup_retention_period: Some(cluster.backup_retention_period),
            preferred_backup_window: Some(cluster.preferred_backup_window.clone()),
            iam_database_authentication_enabled: cluster.iam_database_authentication_enabled,
            snapshot_type: SNAPSHOT_TYPE_MANUAL.to_string(),
            status: SNAPSHOT_STATUS_CREATING.to_string(),
            pending_status: Some(PendingStatus::in_a_moment(SNAPSHOT_STATUS_AVAILABLE)),
            cluster_create_time: cluster.cluster_create_time,
            snapshot_create_time: now_as_wire_timestamp(),
            source_snapshot_arn: None,
            restore_attribute: BTreeSet::new(),
            storage,
            tags,
        }
    }

    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, exactly as [`DbInstanceRecord::settle`] does.
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

    /// `arn:aws:rds:<region>:<account>:cluster-snapshot:<identifier>`.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("cluster-snapshot:{}", self.snapshot_identifier),
        )
    }

    /// Whether this snapshot is public — its `restore` attribute authorizes every
    /// account; see [`SNAPSHOT_ATTRIBUTE_ALL`].
    pub fn is_public(&self) -> bool {
        self.restore_attribute.contains(SNAPSHOT_ATTRIBUTE_ALL)
    }

    /// Project the stored record onto the modelled `DBClusterSnapshot` output shape.
    pub fn to_wire(&self) -> DBClusterSnapshot {
        DBClusterSnapshot {
            availability_zones: Some(self.availability_zones.clone()),
            db_cluster_snapshot_identifier: Some(self.snapshot_identifier.clone()),
            db_cluster_identifier: Some(self.db_cluster_identifier.clone()),
            snapshot_create_time: Some(self.snapshot_create_time),
            engine: Some(self.engine.clone()),
            engine_mode: self.engine_mode.clone(),
            // The storage the snapshot holds. That size is only known once the snapshot has
            // been taken: while it is still being made the API reports the allocated
            // storage of the CLUSTER it is being taken from, and a finished snapshot of a
            // cluster that had no DB instance in it holds no data at all.
            allocated_storage: if self.status == SNAPSHOT_STATUS_AVAILABLE {
                Some(if self.holds_data {
                    self.allocated_storage.unwrap_or(0)
                } else {
                    0
                })
            } else {
                self.allocated_storage
            },
            status: Some(self.status.clone()),
            port: Some(self.port),
            cluster_create_time: Some(self.cluster_create_time),
            master_username: self.master_username.clone(),
            engine_version: self.engine_version.clone(),
            license_model: self.license_model.clone(),
            snapshot_type: Some(self.snapshot_type.clone()),
            // The progress of the copy this record stands for: complete once the
            // snapshot is available, and not yet begun while it is still being made.
            percent_progress: Some(if self.status == SNAPSHOT_STATUS_AVAILABLE {
                100
            } else {
                0
            }),
            storage_encrypted: Some(self.storage_encrypted),
            kms_key_id: self.kms_key_id.clone(),
            db_cluster_snapshot_arn: Some(self.arn()),
            source_db_cluster_snapshot_arn: self.source_snapshot_arn.clone(),
            tag_list: Some(self.tags.clone()),
            storage_type: self.storage_type.clone(),
            db_cluster_resource_id: Some(self.db_cluster_resource_id.clone()),
            vpc_id: Some(self.vpc_id.clone()),
            backup_retention_period: self.backup_retention_period,
            preferred_backup_window: self.preferred_backup_window.clone(),
            iam_database_authentication_enabled: Some(self.iam_database_authentication_enabled),
            ..DBClusterSnapshot::default()
        }
    }
}

/// The tags a snapshot COPY carries: the ones the request named, plus the source's where
/// `CopyTags` asked for them.
///
/// `CopyTags` "specifies whether to copy all tags from the source DB snapshot to the target
/// DB snapshot" and `Tags` is the caller's own list; the two members are independent, so a
/// request that sets both gets both. A key the request names keeps the value the REQUEST
/// gave it — the copy's own tag wins over the source's for that key — so nothing a caller
/// asked for is silently replaced.
///
/// The one rule for both copies, the DB snapshot's and the DB cluster snapshot's.
pub fn copied_snapshot_tags(copy_tags: Option<bool>, requested: &[Tag], source: &[Tag]) -> Vec<Tag> {
    let mut tags = requested.to_vec();
    if copy_tags.unwrap_or(false) {
        for tag in source {
            if !tags.iter().any(|held| held.key == tag.key) {
                tags.push(tag.clone());
            }
        }
    }
    tags
}

/// The tags a RESTORE that names none of its own gives the resource it creates.
///
/// `CopyTagsToSnapshot` states the rule: "RDS checks whether you specify new tags. If yes,
/// the new tags are added to the restored DB instance. If there are no new tags, RDS looks
/// for the tags from the source DB instance for the DB snapshot, and then adds those tags to
/// the restored DB instance." `source` is that resource's tags where this region still holds
/// it; where it is gone — the final snapshot of a deleted instance or cluster — what is left
/// of them is what the snapshot itself carries.
///
/// The one rule for every restore, the DB instance's and the DB cluster's, beside
/// [`copied_snapshot_tags`] — the same question answered for a snapshot COPY.
pub fn inherited_snapshot_tags(
    source: Option<Vec<Tag>>,
    snapshot: &[Tag],
) -> Vec<Tag> {
    match source {
        Some(tags) if !tags.is_empty() => tags,
        _ => snapshot.to_vec(),
    }
}

/// How Amazon RDS words the not-found fault for the SOURCE of a snapshot copy, looked for
/// from a request signed for `request_region`.
///
/// A copy names its source either by identifier, in the region the request was signed for,
/// or by ARN, in the region that ARN names — a copy whose source lives in another region is
/// exactly what the copy operations exist for — so the refusal names BOTH the snapshot that
/// was asked for and the region it was looked for in. `requested` is carried in the
/// request's own spelling, and a region-less ARN (whose region segment is empty) was looked
/// for in the request's own region.
///
/// The one wording for both copies, the DB snapshot's and the DB cluster snapshot's: the
/// sentence names the resource it could not find rather than the operation.
pub fn snapshot_copy_source_not_found_message(requested: &str, request_region: &str) -> String {
    let source_region = arn_region(requested)
        .filter(|region| !region.is_empty())
        .unwrap_or(request_region);
    format!("Requested snapshot {requested} not found in source region {source_region}")
}

/// How Amazon RDS words the DB-snapshot not-found fault: the `<type> not found: <name>`
/// form it uses for a resource a request NAMED, carrying the request's own spelling of the
/// identifier. Stated once here so every operation that resolves a DB snapshot answers the
/// one text.
pub fn db_snapshot_not_found_message(named: &str) -> String {
    format!("DBSnapshot not found: {named}")
}

/// How Amazon RDS words the DB CLUSTER snapshot not-found fault: the same
/// `<type> not found: <name>` form [`db_snapshot_not_found_message`] words, for the cluster
/// snapshot a request NAMED, carrying the request's own spelling of the identifier.
pub fn db_cluster_snapshot_not_found_message(named: &str) -> String {
    format!("DBClusterSnapshot not found: {named}")
}

/// The sentence the `DBClusterSnapshotAlreadyExistsFault` carries for `named`: "the user
/// already has a DB cluster snapshot with the given identifier", worded as the resource
/// type and the identifier the snapshot would be stored under.
///
/// One wording for every operation that MINTS a cluster snapshot — `CreateDBClusterSnapshot`,
/// `CopyDBClusterSnapshot` and the final snapshot `DeleteDBCluster` takes — so no two of them
/// word the same collision differently.
pub fn db_cluster_snapshot_already_exists_message(named: &str) -> String {
    format!("DB cluster snapshot already exists: {named}")
}

/// The sentence the `SnapshotQuotaExceededFault` carries when another manual DB cluster
/// snapshot cannot be stored: the bound this emulator writes under, stated as the per-region
/// quota and the number of regions it keeps records for
/// ([`ComAmazonawsRdsAmazonrdsv19State::cluster_snapshot_room_is_exhausted`], which is the
/// predicate this sentence reports). Shared by the same three operations as
/// [`db_cluster_snapshot_already_exists_message`].
pub fn db_cluster_snapshot_quota_message() -> String {
    format!(
        "the request would exceed the allowed number of manual DB cluster snapshots ({} per \
         region, in at most {} regions)",
        DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION, MAX_REGIONS
    )
}

/// A stored DB snapshot: the source instance's settings as they stood when the
/// snapshot was taken, which is why they are copied rather than re-read later.
#[derive(Debug, Clone)]
pub struct DbSnapshotRecord {
    pub snapshot_identifier: String,
    /// The region the snapshot lives in; the scope of its ARN.
    pub region: String,
    pub db_instance_identifier: String,
    pub dbi_resource_id: String,
    pub engine: String,
    pub engine_version: Option<String>,
    /// `DBInstanceClass` of the instance the snapshot was taken from, which is what a
    /// restore of this snapshot provisions when the request names no class of its own
    /// ("Default: The same DBInstanceClass as the original DB instance").
    pub db_instance_class: String,
    /// `DBName` of the instance the snapshot was taken from: the database the snapshot's
    /// data lives in, so a restore of it serves the same database name.
    pub db_name: Option<String>,
    pub allocated_storage: Option<i32>,
    pub master_username: Option<String>,
    /// The master user's credential, as the DIGESTS a login is verified against — never
    /// the password itself, exactly as [`DbInstanceRecord::master_user_password`] holds
    /// it. A restore keeps the source's master credential, so the snapshot has to carry
    /// it.
    pub master_user_password: Option<data_plane::wire_session::MasterPasswordHash>,
    pub availability_zone: String,
    pub port: i32,
    pub storage_type: String,
    /// The storage throughput the instance the snapshot was taken from was provisioned
    /// with, in MiBps, which a restore provisions again.
    pub storage_throughput: i32,
    /// `BackupRetentionPeriod` as the source instance carried it, which is what a restore
    /// uses when the request names none ("Default: Uses existing setting").
    pub backup_retention_period: i32,
    /// `PreferredBackupWindow` as the source instance carried it, for the same reason.
    pub preferred_backup_window: Option<String>,
    pub encrypted: bool,
    pub kms_key_id: Option<String>,
    pub iops: Option<i32>,
    pub license_model: Option<String>,
    pub status: String,
    pub pending_status: Option<PendingStatus>,
    pub snapshot_type: String,
    pub instance_create_time: ::aws_smithy_types::DateTime,
    pub snapshot_create_time: ::aws_smithy_types::DateTime,
    /// A COPY of the data the source instance's storage held when the snapshot was taken,
    /// taken by the engine itself ([`data_plane::sql_engine::InstanceDatabase::capture`]):
    /// this is what makes a snapshot restorable, because a restore loads it into the new
    /// instance's own storage. `None` for a snapshot of an instance that has no engine
    /// endpoint at all — an engine whose wire this emulator serves no data plane for.
    ///
    /// The copy is in memory and private to this snapshot, it is never written to the
    /// host, and its size is bounded by the same per-database page ceiling every database
    /// of this service's data plane carries; the number of copies is bounded by
    /// [`DB_SNAPSHOT_QUOTA_PER_REGION`] and [`MAX_REGIONS`], like every other record here.
    /// Deleting the snapshot drops it.
    pub storage: Option<::std::sync::Arc<data_plane::sql_engine::InstanceDatabase>>,
    pub tags: Vec<Tag>,
}

impl DbSnapshotRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, exactly as [`DbInstanceRecord::settle`] does.
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

    /// Snapshot the source instance's settings and a copy of its data. `snapshot_type` is
    /// `manual` for a `CreateDBSnapshot` and for the final snapshot `DeleteDBInstance`
    /// takes — `automated` is reserved for the backups RDS itself schedules.
    ///
    /// `storage` is the copy of the instance's storage the caller took under the same
    /// state lock ([`ComAmazonawsRdsAmazonrdsv19State::capture_instance_storage`]), so the
    /// settings the snapshot reports and the data it holds are from one instant.
    pub fn from_instance(
        instance: &DbInstanceRecord,
        snapshot_identifier: &str,
        tags: Vec<Tag>,
        storage: Option<::std::sync::Arc<data_plane::sql_engine::InstanceDatabase>>,
    ) -> Self {
        Self {
            snapshot_identifier: snapshot_identifier.to_string(),
            region: instance.region.clone(),
            db_instance_identifier: instance.identifier.clone(),
            dbi_resource_id: instance.dbi_resource_id.clone(),
            engine: instance.engine.clone(),
            engine_version: instance.engine_version.clone(),
            db_instance_class: instance.db_instance_class.clone(),
            db_name: instance.db_name.clone(),
            allocated_storage: instance.allocated_storage,
            master_username: instance.master_username.clone(),
            master_user_password: instance.master_user_password.clone(),
            availability_zone: instance.availability_zone.clone(),
            port: instance.endpoint_port,
            storage_type: instance.storage_type.clone(),
            storage_throughput: instance.storage_throughput,
            backup_retention_period: instance.backup_retention_period,
            preferred_backup_window: instance.preferred_backup_window.clone(),
            encrypted: instance.storage_encrypted,
            kms_key_id: instance.kms_key_id.clone(),
            iops: instance.iops,
            license_model: instance.license_model.clone(),
            status: SNAPSHOT_STATUS_CREATING.to_string(),
            pending_status: Some(PendingStatus::in_a_moment(SNAPSHOT_STATUS_AVAILABLE)),
            snapshot_type: SNAPSHOT_TYPE_MANUAL.to_string(),
            instance_create_time: instance.instance_create_time,
            snapshot_create_time: now_as_wire_timestamp(),
            storage,
            tags,
        }
    }

    /// `arn:aws:rds:<region>:<account>:snapshot:<identifier>`.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("snapshot:{}", self.snapshot_identifier),
        )
    }

    /// Project the stored record onto the modelled `DBSnapshot` output shape.
    pub fn to_wire(&self) -> DBSnapshot {
        DBSnapshot {
            db_snapshot_identifier: Some(self.snapshot_identifier.clone()),
            db_instance_identifier: Some(self.db_instance_identifier.clone()),
            snapshot_create_time: Some(self.snapshot_create_time),
            engine: Some(self.engine.clone()),
            allocated_storage: self.allocated_storage,
            status: Some(self.status.clone()),
            port: Some(self.port),
            availability_zone: Some(self.availability_zone.clone()),
            instance_create_time: Some(self.instance_create_time),
            master_username: self.master_username.clone(),
            engine_version: self.engine_version.clone(),
            license_model: self.license_model.clone(),
            snapshot_type: Some(self.snapshot_type.clone()),
            iops: self.iops,
            // 0 while the snapshot is still being taken, 100 once it is available:
            // the progress of the copy this record stands for, not a stored field.
            percent_progress: Some(if self.status == "available" { 100 } else { 0 }),
            storage_type: Some(self.storage_type.clone()),
            storage_throughput: Some(self.storage_throughput),
            // The automated-backup settings the source instance carried when the snapshot
            // was taken, which is what a restore of it uses when the request names none.
            backup_retention_period: Some(self.backup_retention_period),
            preferred_backup_window: self.preferred_backup_window.clone(),
            encrypted: Some(self.encrypted),
            kms_key_id: self.kms_key_id.clone(),
            db_snapshot_arn: Some(self.arn()),
            dbi_resource_id: Some(self.dbi_resource_id.clone()),
            tag_list: Some(self.tags.clone()),
            ..DBSnapshot::default()
        }
    }
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// Whether this store already holds a DB cluster snapshot under `key` in `region` —
    /// what every operation that MINTS one is held to, since a second snapshot under one
    /// identifier would overwrite the cluster state the first one holds.
    ///
    /// One reading for the three operations that mint a cluster snapshot
    /// (`CreateDBClusterSnapshot`, `CopyDBClusterSnapshot`, and the final snapshot
    /// `DeleteDBCluster` takes), each of which lifts it into its own modelled
    /// `DBClusterSnapshotAlreadyExistsFault`.
    pub fn holds_cluster_snapshot(&self, region: &str, key: &str) -> bool {
        self.db_cluster_snapshots
            .contains_key(&(region.to_string(), key.to_string()))
    }

    /// Whether another manual DB cluster snapshot can be stored for `region` at all: the
    /// region's own quota ([`DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION`]) and the number of
    /// regions this store keeps records for ([`MAX_REGIONS`]).
    ///
    /// The one bound the three minting operations write under, so no two of them differ
    /// about how many snapshots a region holds; each lifts it into its own modelled
    /// `SnapshotQuotaExceededFault`, worded by
    /// [`db_cluster_snapshot_quota_message`].
    pub fn cluster_snapshot_room_is_exhausted(&self, region: &str) -> bool {
        self.cluster_snapshots_in(region).len() >= DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION
            || !self.has_room_for_region(region)
    }

    /// The key of the DB snapshot `value` names IN `region`: either its identifier, as the
    /// snapshot was taken under it, or its ARN — `RestoreDBInstanceFromDBSnapshot`
    /// documents both ("If you are restoring from a shared manual DB snapshot, the
    /// `DBSnapshotIdentifier` must be the ARN of the shared DB snapshot").
    ///
    /// A DB snapshot's identifier is stored as the caller spelled it — `DBSnapshotIdentifier`
    /// documents no case folding, unlike the instance and group names — so the identifier
    /// form matches exactly, the way `DescribeDBSnapshots` and `DeleteDBSnapshot` read it.
    pub fn resolve_db_snapshot_key(&self, region: &str, value: &str) -> Option<String> {
        if self
            .db_snapshots
            .contains_key(&(region.to_string(), value.to_string()))
        {
            return Some(value.to_string());
        }
        // Only an ARN can match the scan below, so a plain identifier that named nothing
        // does not pay for rendering a region's stored ARNs.
        if !value.starts_with("arn:") {
            return None;
        }
        self.snapshots_in(region)
            .into_iter()
            .find(|record| record.arn() == value)
            .map(|record| record.snapshot_identifier.clone())
    }
}
