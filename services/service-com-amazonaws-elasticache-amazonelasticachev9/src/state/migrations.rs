// SPDX-License-Identifier: CC0-1.0

use super::*;

/// `MigrationSummary$Status` while the migration's resources are set up and no
/// data is moving yet: what `PrepareMigration` ("prepares the serverless cache
/// for data migration by validating the configuration and setting up the
/// migration resources") leaves behind.
pub const MIGRATION_STATUS_PREPARED: &str = "prepared";

/// `MigrationSummary$Status` while data is being copied from the source: the
/// phase `StopMigration` is documented against ("stops an ongoing data sync
/// between the source and the serverless cache \u2026 valid only while the migration
/// is syncing").
pub const MIGRATION_STATUS_SYNCING: &str = "syncing";

/// `MigrationSummary$Status` after a `StopMigration`: "the migration remains
/// open and the data sync can be restarted".
pub const MIGRATION_STATUS_STOPPED: &str = "stopped";

/// `MigrationSummary$Status` while requests are being moved onto the target: the
/// phase `StopMigration` refuses in ("during cutover it returns
/// `InvalidServerlessCacheStateFault`").
pub const MIGRATION_STATUS_CUTOVER: &str = "cutover";

/// How ElastiCache refuses a `CompleteMigration` on a target whose data
/// migration is not in flight: the cutover it asks for only exists while data is
/// being migrated, and the work that gets a target there is work the service
/// carries out after the call that started it — which is what "retry later"
/// states.
///
/// One sentence, stated once, for the one operation that completes a migration.
pub const MIGRATION_NOT_MIGRATING_MESSAGE: &str =
    "The replication group must be in migrating state to complete the migration. Please retry \
     later.";

/// How ElastiCache refuses a `StartMigration` on a replication group whose
/// status cannot take one: the sentence states the statuses the operation
/// accepts — a group that is provisioned and idle, or one already migrating
/// data — and names neither the group nor the status it is in.
///
/// The state gate of the operation that STARTS a migration, worded for that
/// operation rather than through the general modification refusal, because the
/// statuses it accepts are its own.
pub const MIGRATION_START_STATE_MESSAGE: &str =
    "Replication group must be in available or migrating status to start migration.";

/// How ElastiCache refuses a `TestMigration` on a replication group whose
/// status cannot take one: the sentence states the status the operation accepts
/// — a group that is provisioned and idle, the state a migration into it would
/// be started from — and names neither the group nor the status it is in.
pub const MIGRATION_TEST_STATE_MESSAGE: &str =
    "Replication group must be in available status to test migration.";

/// How ElastiCache refuses a migration source list that names more than one endpoint:
/// `TestMigration$CustomerNodeEndpointList`'s own rule, stated as the model states it
/// ("List of endpoints from which data should be migrated. List should have only one
/// element") — the sentence alone, with nothing of the request in it.
pub const MIGRATION_ONE_SOURCE_MESSAGE: &str = "List should have only one element.";

/// `MigrationSummary$CutoverMode` when ElastiCache cuts over by itself, which is
/// what `StartMigration$AutoCutover` asks for: "whether ElastiCache
/// automatically attempts to complete the migration after data sync is
/// established".
pub const CUTOVER_MODE_AUTOMATIC: &str = "automatic";

/// `MigrationSummary$CutoverMode` when the caller cuts over itself, with
/// `CompleteMigration`.
pub const CUTOVER_MODE_MANUAL: &str = "manual";

/// The widest TCP port an endpoint can name.
pub const MAX_PORT: i32 = 65_535;

/// The most source endpoints one migration reads from.
///
/// A migration reads one source per shard of the target's keyspace, and a
/// keyspace is partitioned across at most [`MAX_NODE_GROUPS`] shards, so a
/// longer list names no source a target could read from \u2014 which is also what
/// keeps the list a request stores on the migration record bounded.
pub const MAX_SOURCE_ENDPOINTS: usize = MAX_NODE_GROUPS as usize;

/// One endpoint a migration reads data FROM, as a request's
/// `CustomerNodeEndpoint` named it: "the endpoint from which data should be
/// migrated".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEndpointRecord {
    pub address: String,
    pub port: i32,
}

/// One open data migration, held on the resource it migrates INTO.
///
/// A migration is one thing however it was targeted, so there is one record of
/// it: `ServerlessCache$Migration` reports the copy a serverless cache carries
/// ([`ServerlessCacheRecord::migration`]), and the copy a replication group
/// carries ([`ReplicationGroupRecord::migration`]) is what the group's
/// already-under-migration refusal reads \u2014 `ReplicationGroup` models no member
/// for it.
#[derive(Debug, Clone)]
pub struct MigrationRecord {
    /// One of [`MIGRATION_STATUS_PREPARED`], [`MIGRATION_STATUS_SYNCING`],
    /// [`MIGRATION_STATUS_STOPPED`] or [`MIGRATION_STATUS_CUTOVER`].
    pub status: String,
    /// The endpoints data is read from: for a replication group the ones
    /// `StartMigration` named, for a serverless cache the ones
    /// `PrepareMigration` did ("for serverless cache migrations, ElastiCache
    /// uses the endpoint that you provided in `PrepareMigration`").
    pub source_endpoints: Vec<SourceEndpointRecord>,
    /// [`CUTOVER_MODE_AUTOMATIC`] or [`CUTOVER_MODE_MANUAL`], once a
    /// `StartMigration` has said which; a migration that has only been prepared
    /// has not been told yet.
    pub cutover_mode: Option<String>,
    /// When the data sync last started, which is what makes an open migration
    /// datable in the event log.
    pub started_at: Option<SystemTime>,
}

impl MigrationRecord {
    /// The migration a `PrepareMigration` sets up: its resources are ready and
    /// no data is moving yet.
    pub fn prepared(source_endpoints: Vec<SourceEndpointRecord>) -> Self {
        Self {
            status: MIGRATION_STATUS_PREPARED.to_string(),
            source_endpoints,
            cutover_mode: None,
            started_at: None,
        }
    }

    /// Start (or restart) the data sync, recording which side carries out the
    /// cutover.
    pub fn begin_sync(&mut self, cutover_mode: &str, now: SystemTime) {
        self.status = MIGRATION_STATUS_SYNCING.to_string();
        self.cutover_mode = Some(cutover_mode.to_string());
        self.started_at = Some(now);
    }

    /// Whether a data sync can be started from this phase: a prepared migration
    /// has not synced yet and a stopped one "can be restarted", while one
    /// already syncing or in cutover is past that point.
    pub fn can_begin_sync(&self) -> bool {
        self.status == MIGRATION_STATUS_PREPARED || self.status == MIGRATION_STATUS_STOPPED
    }

    /// Whether this migration is MIGRATING: its data sync is in flight
    /// ([`MIGRATION_STATUS_SYNCING`]) or its cutover already under way
    /// ([`MIGRATION_STATUS_CUTOVER`]), which is the phase a `CompleteMigration`
    /// can cut over from. A migration that has only been prepared, or that a
    /// `StopMigration` left stopped, is not migrating: there is no sync to
    /// complete, and the request is refused with
    /// [`MIGRATION_NOT_MIGRATING_MESSAGE`].
    pub fn is_migrating(&self) -> bool {
        self.status == MIGRATION_STATUS_SYNCING || self.status == MIGRATION_STATUS_CUTOVER
    }

    /// This record as the modelled `MigrationSummary`.
    ///
    /// `migration_endpoint` is "the standard endpoint of the serverless cache",
    /// which the cache holds once, so it is passed in rather than held a second
    /// time here.
    pub fn to_modeled(&self, migration_endpoint: Endpoint) -> MigrationSummary {
        MigrationSummary {
            status: Some(self.status.clone()),
            migration_endpoint: Some(migration_endpoint),
            cutover_mode: self.cutover_mode.clone(),
        }
    }
}

/// Validate a request's `CustomerNodeEndpointList` into the endpoints a
/// migration reads from, or answer the sentence the request is refused with.
///
/// The one place a migration's source endpoints are checked, so every migration
/// operation that takes the list ("the list of endpoints from which to migrate
/// data") holds it to the same rule: at least one endpoint, each with an address
/// and a port a TCP endpoint can carry.
pub fn source_endpoints(
    endpoints: &[CustomerNodeEndpoint],
) -> Result<Vec<SourceEndpointRecord>, String> {
    if endpoints.is_empty() {
        return Err(
            "The parameter CustomerNodeEndpointList must be provided and must not be blank."
                .to_string(),
        );
    }
    if endpoints.len() > MAX_SOURCE_ENDPOINTS {
        return Err(format!(
            "Invalid value for CustomerNodeEndpointList: a migration reads from at most \
             {MAX_SOURCE_ENDPOINTS} endpoints."
        ));
    }
    let mut out = Vec::with_capacity(endpoints.len());
    for endpoint in endpoints {
        let address = endpoint.address.as_deref().unwrap_or_default().trim();
        if address.is_empty() {
            return Err(
                "The parameter CustomerNodeEndpoint.Address must be provided and must not be \
                 blank."
                    .to_string(),
            );
        }
        let Some(port) = endpoint.port else {
            return Err(
                "The parameter CustomerNodeEndpoint.Port must be provided and must not be blank."
                    .to_string(),
            );
        };
        if !(1..=MAX_PORT).contains(&port) {
            return Err(format!(
                "Invalid value {port} for Port: must be between 1 and {MAX_PORT}."
            ));
        }
        out.push(SourceEndpointRecord {
            address: address.to_string(),
            port,
        });
    }
    Ok(out)
}
