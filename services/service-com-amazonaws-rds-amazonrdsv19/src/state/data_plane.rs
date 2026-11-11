// SPDX-License-Identifier: CC0-1.0

//! The DB instance DATA plane: the engine endpoint `Endpoint{Address,Port}` publishes.
//!
//! A DB instance's endpoint is an ADDRESS A CLIENT CONNECTS TO with its ordinary driver
//! and then speaks the engine's own protocol on — there is no API operation for running
//! SQL against a DB instance. So for every engine whose wire this emulator speaks
//! ([`engine_wire`]) `CreateDBInstance` opens a listener through the CORE
//! (`engine.listeners().tcp_plaintext(..)`, which owns the socket, the bind interface,
//! the port and the TLS material) and the control plane publishes THAT listener's host
//! and port, because an address nothing listens on is not an endpoint. An engine whose
//! wire this emulator does not speak keeps the DNS-form endpoint it has always reported
//! and gets no listener: a port that answered nothing would be worse than a name.
//!
//! The listener is PLAINTEXT on byte 0 and upgrades in band, which is what both wires
//! require: a PostgreSQL client sends `SSLRequest` in the clear and expects the server to
//! answer before any TLS, and a MySQL client reads the server's greeting in the clear and
//! only then sets `CLIENT_SSL`. The handshake itself is run by the core's own
//! [`::emulator_server::TlsUpgrader`], so this crate configures no TLS.
//!
//! What each part owns:
//!
//! * this module — the endpoint record the store holds per instance, which engine speaks
//!   which wire, and the facts a session is admitted against ([`InstanceFacts`]);
//! * [`cluster_endpoint`] — the same for a DB CLUSTER's writer endpoint, whose facts come
//!   from the cluster's own record;
//! * [`sql_engine`] — the in-process SQLite database each of an instance's databases runs
//!   its statements against;
//! * [`session_transactions`] — the transaction each SESSION has open on it, and the rows
//!   its `ROLLBACK` puts back;
//! * [`row_locks`] — the rows those transactions hold against each other, and
//!   [`lock_plan`] — which rows one statement asks for;
//! * [`constraint_names`] — the constraint a refused row violated, named as the client's own
//!   engine names it;
//! * [`wire_session`] — what is shared by the two wires: the admission decision, the
//!   server commands (`SHOW`, `SET`) and the session values a statement can read;
//! * [`server_statements`] — the statements the server answers out of the instance's own
//!   catalog: installing an extension, creating a database, opening and ending a
//!   transaction;
//! * [`pg_grants`] — the role memberships and object privileges a PostgreSQL client grants
//!   and revokes, and the catalogs they live in;
//! * [`mysql_s3_load`] — `LOAD DATA FROM S3`, the Amazon S3 import the MySQL engines serve
//!   under the IAM role their parameter group names;
//! * [`iam_auth_token`] — the authentication token an IAM-authenticated login presents in
//!   place of a password;
//! * [`pg_wire`] and [`mysql_wire`] — the server half of each protocol.
//!
//! The endpoint is the RESOURCE: dropping the stored [`DbEngineEndpoint`] closes the port
//! and severs every session on it, which is what a delete does. A stop or a reboot keeps
//! the port (RDS "retains the DB instance's metadata, including its endpoint") and severs
//! the sessions instead ([`DbEngineEndpoint::sever_sessions`]).

use std::sync::atomic::AtomicU32;
use std::sync::Arc;

use tokio::sync::watch;

use super::{ComAmazonawsRdsAmazonrdsv19State, DbInstanceRecord, RegionalKey};

pub mod cluster_endpoint;
pub mod constraint_names;
pub mod dialect;
pub mod iam_auth_token;
pub mod lock_plan;
pub mod mysql_s3_load;
pub mod mysql_wire;
pub mod pg_copy;
pub mod pg_grants;
pub mod pg_server;
pub mod pg_sessions;
pub mod pg_wire;
pub mod row_locks;
pub mod server_statements;
pub mod session_transactions;
pub mod sql_engine;
pub mod tls_probe;
pub mod wire_session;

/// Which native wire an engine speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineWire {
    /// The PostgreSQL frontend/backend protocol, version 3.0.
    Postgres,
    /// The MySQL client/server protocol, version 10 (which MariaDB speaks too).
    MySql,
}

/// The wire the engine `engine` speaks, or `None` for an engine this emulator serves no
/// data plane for.
///
/// The engines here are the ones whose DB INSTANCE endpoint is the engine's own endpoint:
/// Amazon RDS for PostgreSQL, for MySQL and for MariaDB — MariaDB speaking the MySQL
/// protocol. The Oracle, SQL Server and Db2 engines speak neither protocol, and this
/// emulator runs no engine for them.
///
/// An AURORA DB instance is deliberately not here: its data plane is addressed through its
/// DB CLUSTER's reader and writer endpoints, which `CreateDBCluster` publishes and which
/// [`cluster_endpoint`] serves, so opening a listener on the member endpoint alone would
/// publish half a cluster's data plane.
#[must_use]
pub fn engine_wire(engine: &str) -> Option<EngineWire> {
    match engine.to_ascii_lowercase().as_str() {
        "postgres" => Some(EngineWire::Postgres),
        "mysql" | "mariadb" => Some(EngineWire::MySql),
        _ => None,
    }
}

/// The first version of RDS for PostgreSQL whose default parameter group turns
/// `rds.force_ssl` on.
///
/// Amazon RDS publishes the parameter's default as 1 (on) for RDS for PostgreSQL 15 and
/// higher and 0 (off) for the earlier versions, so a plaintext login is refused by
/// default on an instance of 15 or later and admitted on one before it.
const FORCE_SSL_DEFAULT_FROM_MAJOR_VERSION: u32 = 15;

/// Whether a PostgreSQL instance running `engine_version` forces TLS by default; see
/// [`FORCE_SSL_DEFAULT_FROM_MAJOR_VERSION`]. A version this cannot read is treated as a
/// current one, which is the default a new instance gets.
///
/// The one place that rule lives: the login gate above reads it for an INSTANCE's engine
/// version, and the published record the parameter catalog reports for a DB parameter
/// group FAMILY — the parameter's default AND its apply type, both of which moved at this
/// release — is the same rule over that family's version
/// (`crate::state::published_spec_for_family`), so for RDS for PostgreSQL, whose default
/// this release moved, what a describe reports for `rds.force_ssl` and what a session on
/// such an instance is held to cannot disagree.
pub(crate) fn postgres_forces_ssl_by_default(engine_version: &str) -> bool {
    engine_version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .is_none_or(|major| major >= FORCE_SSL_DEFAULT_FROM_MAJOR_VERSION)
}

/// WHICH parameter decides that a session on `wire` must be encrypted: `rds.force_ssl` on
/// PostgreSQL and `require_secure_transport` on MySQL and MariaDB.
#[must_use]
pub(crate) fn secure_transport_parameter(wire: EngineWire) -> &'static str {
    match wire {
        EngineWire::Postgres => "rds.force_ssl",
        EngineWire::MySql => "require_secure_transport",
    }
}

/// Whether an encrypted session is REQUIRED, from the value the resource's parameter group
/// sets for [`secure_transport_parameter`] (`None` where it sets none) and the engine
/// version the resource runs.
///
/// The ONE reading of that parameter, for a DB instance's endpoint and a DB cluster's
/// alike, so the two cannot disagree about a login this emulator admits:
///
/// * PostgreSQL: Amazon RDS documents `rds.force_ssl`'s default as 1 for version 15 and
///   higher and 0 for the versions before it, so the resource's own major version decides
///   it where its group sets nothing.
/// * MySQL and MariaDB: `require_secure_transport`, whose documented default is OFF.
#[must_use]
pub(crate) fn secure_transport_in_force(
    wire: EngineWire,
    set: Option<&str>,
    engine_version: Option<&str>,
) -> bool {
    match set {
        Some(value) => matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "on" | "true" | "yes"
        ),
        None => match wire {
            EngineWire::Postgres => {
                postgres_forces_ssl_by_default(engine_version.unwrap_or_default())
            }
            EngineWire::MySql => false,
        },
    }
}

/// The address a DB instance's engine listener accepts connections on, which is what the
/// modelled `Endpoint` reports for an instance that has one.
#[derive(Debug, Clone)]
pub struct ServedEndpoint {
    /// The host the core bound the listener to.
    pub host: String,
    /// The port the core assigned it. The port a request ASKED for (`Port`,
    /// `DBPortNumber`) is kept on the record as the instance's configured port, but the
    /// core owns the port range, so the published port is the one a client can reach.
    pub port: i32,
}

/// One DB instance's engine endpoint: the live listener, the database behind it, and the
/// handle that severs its sessions.
pub struct DbEngineEndpoint {
    /// Where the control plane publishes it.
    pub endpoint: ServedEndpoint,
    /// Which protocol the listener speaks.
    pub wire: EngineWire,
    /// The instance's databases, shared with every session on the listener.
    database: Arc<sql_engine::InstanceDatabase>,
    /// The sessions open on this listener, shared with every session on it: what
    /// `pg_stat_activity` reports and what `max_connections` is counted against.
    sessions: pg_sessions::SessionRegistry,
    /// The engine's run: a session serves only while the value it started under is
    /// current, so bumping it severs every live session (a reboot, a stop).
    generation: watch::Sender<u64>,
    /// Held only to keep the port open; the core owns the socket and its TLS.
    _listener: ::emulator_server::ResourceListener,
}

/// Written by hand rather than derived: the endpoint holds a core `ResourceListener` and a
/// database connection, neither of which is `Debug`. What a reader needs is the address it
/// published and the protocol it speaks.
impl std::fmt::Debug for DbEngineEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbEngineEndpoint")
            .field("endpoint", &self.endpoint)
            .field("wire", &self.wire)
            .field("generation", &*self.generation.borrow())
            .finish()
    }
}

impl DbEngineEndpoint {
    /// The database every session on this endpoint runs its statements against.
    #[must_use]
    pub fn database(&self) -> Arc<sql_engine::InstanceDatabase> {
        Arc::clone(&self.database)
    }

    /// The sessions open on this endpoint.
    #[must_use]
    pub fn sessions(&self) -> pg_sessions::SessionRegistry {
        self.sessions.clone()
    }

    /// End every session open on this endpoint, leaving the port bound.
    ///
    /// What a restart of the database engine does: "Rebooting a DB instance results in a
    /// momentary outage" and a stopped instance serves nobody, so the connections an
    /// engine held do not survive either. The work none of them committed does not survive
    /// it either: the engine's run ends, so every transaction still open is rolled back here
    /// rather than waiting for each severed session's own teardown to get to it
    /// ([`sql_engine::InstanceDatabase::rollback_open_transactions`], which is what each
    /// session's teardown then finds nothing left to do).
    pub fn sever_sessions(&self) {
        let next = self.generation.borrow().wrapping_add(1);
        let _ = self.generation.send(next);
        self.database.rollback_open_transactions();
    }
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The engine endpoint of the DB instance `key` names, for the instances that have
    /// one.
    #[must_use]
    pub fn engine_endpoint(&self, key: &RegionalKey) -> Option<&DbEngineEndpoint> {
        self.db_engine_endpoints.get(key)
    }

    /// End every session open on the DB instance `key` names. Does nothing for an
    /// instance with no engine endpoint.
    pub fn sever_engine_sessions(&self, key: &RegionalKey) {
        if let Some(endpoint) = self.db_engine_endpoints.get(key) {
            endpoint.sever_sessions();
        }
    }

    /// Move an instance's engine endpoint with the instance when `ModifyDBInstance`
    /// renames it, so the listener a client is connected to stays the endpoint the
    /// control plane publishes for that instance.
    pub fn move_engine_endpoint(&mut self, from: &RegionalKey, to: &RegionalKey) {
        if from == to {
            return;
        }
        if let Some(endpoint) = self.db_engine_endpoints.remove(from) {
            self.db_engine_endpoints.insert(to.clone(), endpoint);
        }
    }

    /// Whether a session on `record`'s engine endpoint must be encrypted.
    ///
    /// The engines carry this as a PARAMETER, so the answer is the effective value of
    /// that parameter for this instance: what the instance's own DB parameter group sets
    /// for it, and the engine's default where the group sets nothing (a group this store
    /// holds no record of is a default group AWS provides, which sets no parameter of its
    /// own).
    ///
    /// * PostgreSQL: `rds.force_ssl`. Amazon RDS documents the default as 1 for RDS for
    ///   PostgreSQL version 15 and higher and 0 for the versions before it, so the
    ///   instance's own major version decides it.
    /// * MySQL and MariaDB: `require_secure_transport`, whose documented default is OFF.
    #[must_use]
    pub fn requires_secure_transport(&self, record: &DbInstanceRecord) -> bool {
        let Some(wire) = engine_wire(&record.engine) else {
            return false;
        };
        let set = record
            .db_parameter_group_name
            .as_ref()
            .and_then(|name| {
                self.db_parameter_groups
                    .get(&(record.region.clone(), name.to_ascii_lowercase()))
            })
            .and_then(|group| group.parameters.get(secure_transport_parameter(wire)));
        secure_transport_in_force(
            wire,
            set.map(|setting| setting.value.as_str()),
            record.engine_version.as_deref(),
        )
    }

    /// The DB instance in `region` whose `DbiResourceId` is `resource_id`.
    ///
    /// A session on an engine endpoint resolves its instance by this id rather than by
    /// identifier, because `ModifyDBInstance` can rename an instance under a live
    /// connection while the resource id "never changes". Reads one region's ordered range,
    /// which the region's own instance quota bounds.
    #[must_use]
    pub fn instance_by_resource_id(
        &self,
        region: &str,
        resource_id: &str,
    ) -> Option<&DbInstanceRecord> {
        self.db_instances
            .range((region.to_string(), String::new())..)
            .take_while(|((stored, _), _)| stored == region)
            .map(|(_, record)| record)
            .find(|record| record.dbi_resource_id == resource_id)
    }
}

/// A COPY of the data the DB instance `key` names holds right now: what a DB snapshot of
/// that instance holds, so restoring the snapshot brings those rows back.
///
/// `Ok(None)` for an instance with no engine endpoint — an engine whose wire this emulator
/// serves no data plane for ([`engine_wire`]), which has no storage to copy, and an
/// identifier this store holds no instance under. `Err` when the engine could not make the
/// copy, which the caller answers as a failure of this emulator rather than storing a
/// snapshot that holds nothing.
///
/// The store lock is taken only to read the instance's database HANDLE and is released
/// before the copy starts, and the copy itself runs on a blocking task — it is engine
/// work, like a statement a session runs, so it belongs off the runtime's worker threads
/// and outside the lock every other request to this service needs. Its own bounds are
/// [`sql_engine::COPY_DEADLINE`] and the page ceiling each database carries.
///
/// Called BEFORE the handler takes the store lock for the work the snapshot is part of, so
/// no state guard is held across its await.
pub async fn capture_instance_storage(
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    key: &RegionalKey,
) -> Result<Option<Arc<sql_engine::InstanceDatabase>>, String> {
    let database = state
        .lock()
        .engine_endpoint(key)
        .map(DbEngineEndpoint::database);
    capture_storage(database, "DB instance").await
}

/// A COPY of the data the DB CLUSTER `key` names holds right now: what a DB cluster
/// snapshot of that cluster holds, so restoring the snapshot brings those rows back.
///
/// The cluster sibling of [`capture_instance_storage`], under the same contract: `Ok(None)`
/// for a cluster with no engine endpoint — an engine whose wire this emulator serves no
/// data plane for ([`cluster_endpoint::cluster_engine_wire`]), and an identifier this store
/// holds no cluster under — and `Err` when the engine could not make the copy, which the
/// caller answers as a failure of this emulator rather than storing a snapshot that holds
/// nothing. The store lock is taken only to read the database HANDLE, the copy runs on a
/// blocking task, and the call belongs BEFORE the handler takes the store lock for the work
/// the snapshot is part of.
pub async fn capture_cluster_storage(
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    key: &RegionalKey,
) -> Result<Option<Arc<sql_engine::InstanceDatabase>>, String> {
    let database = state
        .lock()
        .cluster_engine_endpoint(key)
        .map(DbEngineEndpoint::database);
    capture_storage(database, "DB cluster").await
}

/// The copy itself, for a resource whose database handle has already been read out of the
/// store: the one body the two captures above share, so a DB instance's snapshot and a DB
/// cluster's are taken by the same engine call under the same bounds and report the same
/// failure, each naming its own `resource`.
///
/// `database` is `None` for a resource with no engine endpoint, which has no storage to copy
/// — the `Ok(None)` both callers document. The handle is read under the store's guard by the
/// caller and the guard is released before this is awaited, so no state lock is held across
/// the copy.
async fn capture_storage(
    database: Option<Arc<sql_engine::InstanceDatabase>>,
    resource: &str,
) -> Result<Option<Arc<sql_engine::InstanceDatabase>>, String> {
    let Some(database) = database else {
        return Ok(None);
    };
    let copied = ::tokio::task::spawn_blocking(move || database.capture())
        .await
        .map_err(|error| format!("the {resource}'s storage could not be copied: {error}"))?;
    copied.map(|copy| Some(Arc::new(copy))).map_err(|error| {
        format!(
            "the {resource}'s storage could not be copied: {}",
            error.message
        )
    })
}

/// Open the engine endpoint of `target` — a DB instance's own endpoint or a DB cluster's
/// writer endpoint — on the wire its engine speaks.
///
/// The core binds the socket and assigns the port; this only hands it the protocol
/// handler and reports back where it landed. The caller stores the returned endpoint in
/// the state — which is what keeps the port open — and publishes
/// [`DbEngineEndpoint::endpoint`] as the resource's `Endpoint`.
///
/// `Err` when this service was mounted on an engine with no listener surface, or when the
/// core could not bind: the caller answers that with a modelled failure rather than
/// publishing an address nothing serves.
pub async fn open_engine_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ComAmazonawsRdsAmazonrdsv19State>,
    target: wire_session::SessionTarget,
    wire: EngineWire,
) -> Result<DbEngineEndpoint, String> {
    let listeners = shared_engine(engine)
        .ok_or_else(|| "this emulator's engine has no listener surface".to_string())?
        .listeners();
    let upgrader = listeners
        .tls_upgrader()
        .map_err(|error| format!("the engine endpoint's TLS could not be prepared: {error}"))?;
    let database = Arc::new(
        sql_engine::InstanceDatabase::open()
            .map_err(|error| format!("the SQL engine could not be started: {}", error.message))?,
    );
    let (generation, generation_rx) = watch::channel(0u64);
    let sessions = pg_sessions::SessionRegistry::default();
    let context = wire_session::SessionContext {
        state: state.clone(),
        target,
        database: Arc::clone(&database),
        sessions: sessions.clone(),
        upgrader,
        // The object store the S3 import statements read through, on the same engine this
        // service is mounted on: what Amazon S3's own `PutObject` wrote is what such a
        // statement loads.
        objects: mysql_s3_load::object_store(engine),
        generation: generation_rx,
        next_connection_id: Arc::new(AtomicU32::new(1)),
    };
    let listener = match wire {
        EngineWire::Postgres => listeners.tcp_plaintext(pg_wire::PostgresWire::new(context)).await,
        EngineWire::MySql => listeners.tcp_plaintext(mysql_wire::MySqlWire::new(context)).await,
    }
    .map_err(|error| format!("the engine endpoint could not be opened: {error}"))?;
    Ok(DbEngineEndpoint {
        endpoint: ServedEndpoint {
            host: listener.host(),
            port: i32::from(listener.port()),
        },
        wire,
        database,
        sessions,
        generation,
        _listener: listener,
    })
}

/// The SHARED engine this service is mounted on, which owns the listener surface: call it
/// with a handler's `&*self.engine`.
fn shared_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<&::engine::InMemoryEngine> {
    (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()
}
