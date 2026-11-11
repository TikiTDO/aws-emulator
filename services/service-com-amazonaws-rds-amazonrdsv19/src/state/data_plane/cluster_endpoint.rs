// SPDX-License-Identifier: CC0-1.0

//! The DB CLUSTER data plane: the engine endpoint `Endpoint`/`Port` publishes.
//!
//! A DB cluster's endpoint is "the connection endpoint for the primary instance of the DB
//! cluster" — AN ADDRESS A CLIENT CONNECTS TO with its ordinary driver and then speaks the
//! engine's own protocol on. So for every cluster whose engine speaks a wire this emulator
//! serves ([`cluster_engine_wire`]) `CreateDBCluster` opens a listener through the CORE and
//! the control plane publishes THAT listener's host and port, exactly as
//! [`super::open_engine_endpoint`] does for a DB instance: an address nothing listens on is
//! not an endpoint. A cluster whose engine speaks neither wire keeps the DNS-form endpoint
//! this service has always reported and gets no listener.
//!
//! BOTH cluster endpoints are served, and so is each MEMBER instance's own. The reader
//! endpoint "load-balances connections across the Aurora Replicas that are available in a DB
//! cluster", and those replicas are read-only instances reading the same cluster volume: it
//! is a second listener of the same engine over the same storage, whose sessions are answered
//! read-only ([`super::EndpointRole`], [`super::read_only`]). A member instance's endpoint is
//! a third kind of address onto that one volume — "each Aurora DB cluster hosts copies of its
//! data" — and it serves writes exactly while that member is the one the cluster writes
//! through, which `FailoverDBCluster` moves ([`serves_reads_only`]). A cluster whose engine
//! speaks neither wire publishes the DNS name for all of them, and gets no listener for any.
//!
//! Everything a session on such a listener needs is the same as on a DB instance's — the
//! two wires, the SQL engine, the server statements are shared — except WHERE the facts of
//! a login come from: a cluster's master user, engine version, status and parameter group
//! live on the CLUSTER record, which is what [`cluster_facts`] reads.

use std::sync::Arc;

use super::sql_engine::InstanceDatabase;
use super::wire_session::{
    facts_of, parameter_group_family, running_engine, EndpointKind, EngineRuntime, InstanceFacts,
    RecordFacts,
};
use super::{EndpointRole, EngineWire};
use crate::state::{ComAmazonawsRdsAmazonrdsv19State, DbClusterRecord};

/// The wire the engine of a DB CLUSTER speaks, or `None` for a cluster this emulator
/// serves no data plane for.
///
/// A cluster's data plane is addressed through the cluster's own endpoints, whichever of
/// the cluster types it is, so the engines named here are the documented `Engine` values of
/// both: an Aurora cluster (`aurora-postgresql`, `aurora-mysql`, and the `aurora` alias of
/// Aurora MySQL 5.6) and a Multi-AZ DB cluster (`postgres`, `mysql`). Aurora PostgreSQL and
/// RDS for PostgreSQL speak the PostgreSQL frontend/backend protocol and Aurora MySQL and
/// RDS for MySQL the MySQL client/server protocol, which are the two wires this emulator
/// serves.
///
/// Every other engine `CreateDBCluster` accepts is deliberately absent, so it gets no
/// listener: an Amazon DocumentDB cluster speaks the MongoDB wire and an Amazon Neptune
/// cluster the graph protocols, neither of which this emulator runs, and publishing a port
/// for one would be worse than publishing a name. The engine names are matched exactly, as
/// the sibling [`super::engine_wire`] matches an instance's.
#[must_use]
pub fn cluster_engine_wire(engine: &str) -> Option<EngineWire> {
    match engine.to_ascii_lowercase().as_str() {
        "aurora-postgresql" | "postgres" => Some(EngineWire::Postgres),
        "aurora-mysql" | "aurora" | "mysql" => Some(EngineWire::MySql),
        _ => None,
    }
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The DB cluster in `region` whose `DbClusterResourceId` is `resource_id`.
    ///
    /// A session on a cluster's engine endpoint resolves its cluster by this id rather than
    /// by identifier, because `ModifyDBCluster` can rename a cluster under a live
    /// connection while the resource id is "Region-unique, immutable". Reads one region's
    /// ordered range, which the region's own cluster quota bounds.
    #[must_use]
    pub fn cluster_by_resource_id(
        &self,
        region: &str,
        resource_id: &str,
    ) -> Option<&DbClusterRecord> {
        self.db_clusters
            .range((region.to_string(), String::new())..)
            .take_while(|((stored, _), _)| stored == region)
            .map(|(_, record)| record)
            .find(|record| record.resource_id == resource_id)
    }

    /// The engine endpoint of the DB cluster `key` names, for the clusters that have one.
    #[must_use]
    pub fn cluster_engine_endpoint(
        &self,
        key: &crate::state::RegionalKey,
    ) -> Option<&super::DbEngineEndpoint> {
        self.db_cluster_engine_endpoints.get(key)
    }

    /// What a MEMBER instance's own endpoint is opened onto: the wire the DB cluster `named`
    /// speaks, the cluster's immutable resource id, and the cluster's STORAGE — "Aurora
    /// cluster volumes ... each Aurora DB cluster hosts copies of its data", one volume every
    /// member reads and the primary writes.
    ///
    /// `None` when the cluster is not held here, or speaks no wire this emulator serves, or
    /// has no listener of its own: such a member keeps the DNS-form endpoint it has always
    /// reported rather than publishing a port onto nothing.
    #[must_use]
    pub fn cluster_volume_of(
        &self,
        region: &str,
        named: &str,
    ) -> Option<(EngineWire, String, Arc<InstanceDatabase>)> {
        let cluster = self.resolve_db_cluster(region, named)?;
        let wire = cluster_engine_wire(&cluster.engine)?;
        let endpoint =
            self.cluster_engine_endpoint(&(cluster.region.clone(), cluster.identifier.clone()))?;
        Some((wire, cluster.resource_id.clone(), endpoint.database()))
    }

    /// Move a cluster's engine endpoint with the cluster when `ModifyDBCluster` renames it,
    /// so the listener a client is connected to stays the endpoint the control plane
    /// publishes for that cluster.
    pub fn move_cluster_engine_endpoint(
        &mut self,
        from: &crate::state::RegionalKey,
        to: &crate::state::RegionalKey,
    ) {
        if from == to {
            return;
        }
        if let Some(endpoint) = self.db_cluster_engine_endpoints.remove(from) {
            self.db_cluster_engine_endpoints.insert(to.clone(), endpoint);
        }
    }

    /// Whether a session on the cluster `record`'s engine endpoint must be encrypted.
    ///
    /// The engines carry this as a PARAMETER, so the answer is the effective value of that
    /// parameter for this cluster — read through the ONE reading of it this module tree has
    /// ([`super::secure_transport_in_force`]), so a cluster's endpoint and an instance's
    /// cannot disagree about a plaintext login: what the cluster's own DB CLUSTER parameter
    /// group sets for it, and the engine's own default where the group sets nothing (a group
    /// this store holds no record of is a default group AWS provides, which sets no
    /// parameter of its own).
    #[must_use]
    pub fn cluster_requires_secure_transport(&self, record: &DbClusterRecord) -> bool {
        let Some(wire) = cluster_engine_wire(&record.engine) else {
            return false;
        };
        let set = record
            .db_cluster_parameter_group_name
            .as_ref()
            .and_then(|name| {
                self.db_cluster_parameter_groups
                    .get(&(record.region.clone(), name.to_ascii_lowercase()))
            })
            .and_then(|group| {
                group
                    .parameters
                    .get(super::secure_transport_parameter(wire))
            });
        super::secure_transport_in_force(
            wire,
            set.map(|setting| setting.value.as_str()),
            record.engine_version.as_deref(),
        )
    }
}

/// The facts a login on the cluster `region`/`resource_id` is admitted against, read under
/// the state lock the caller holds.
///
/// What the CLUSTER record says is read here; everything both kinds of endpoint derive the
/// same way is [`super::wire_session::facts_of`]'s, so the two cannot drift apart.
///
/// `None` when the store holds no such cluster any more, or when its engine speaks no wire
/// this emulator serves — both of which the wire answers as an engine that is not there.
pub(super) fn cluster_facts(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    region: &str,
    resource_id: &str,
    database: &Arc<InstanceDatabase>,
    role: EndpointRole,
) -> Option<InstanceFacts> {
    let record = store.cluster_by_resource_id(region, resource_id)?;
    let wire = cluster_engine_wire(&record.engine)?;
    let endpoint =
        store.cluster_engine_endpoint(&(record.region.clone(), record.identifier.clone()));
    // The MEMBER instance this endpoint belongs to, for the one role that names one: its own
    // record carries the status a session on it is admitted against and the address it was
    // published at, while everything a login is checked against — the master user, the
    // engine version, the database — is the CLUSTER's, which is where an Aurora member's
    // credential lives ("the password is managed by the DB cluster").
    let member = match &role {
        EndpointRole::ClusterMember(identifier) => {
            Some(store.db_instances.get(&(
                record.region.clone(),
                identifier.to_ascii_lowercase(),
            ))?)
        }
        EndpointRole::Writer | EndpointRole::Reader => None,
    };
    // The address of the endpoint the client DIALLED, which is the one an authentication
    // token has to have been generated for and the one a refusal names.
    let served = match &role {
        EndpointRole::Writer => endpoint.map(|endpoint| &endpoint.endpoint),
        EndpointRole::Reader => endpoint.and_then(|endpoint| endpoint.reader.as_ref()),
        EndpointRole::ClusterMember(_) => {
            member.and_then(|member| member.served_endpoint.as_ref())
        }
    };
    Some(facts_of(
        RecordFacts {
            // A session on a member's own endpoint is the MEMBER's: that is the resource a
            // record of it is attributable to, and the status it is admitted against.
            kind: match member {
                Some(_) => EndpointKind::DbInstance,
                None => EndpointKind::DbCluster,
            },
            identifier: member.map_or_else(
                || record.identifier.clone(),
                |member| member.identifier.clone(),
            ),
            engine: record.engine.clone(),
            status: member.map_or_else(
                || record.status.clone(),
                |member| member.status.clone(),
            ),
            wire,
            engine_version: record.engine_version.clone(),
            database_name: record.database_name.clone(),
            master_username: record.master_username.clone(),
            password: record.master_user_password.clone(),
            requires_secure_transport: store.cluster_requires_secure_transport(record),
            served_host: served.map(|served| served.host.clone()),
            served_port: served.and_then(|served| u16::try_from(served.port).ok()),
            region: record.region.clone(),
            iam_database_authentication_enabled: record.iam_database_authentication_enabled,
            // A cluster's writer endpoint is "the connection endpoint for the primary
            // instance of the DB cluster", which is what writes go through; its reader
            // endpoint reaches the read-only instances. A MEMBER's own endpoint serves
            // writes exactly while that member is the one the cluster writes through, which
            // `FailoverDBCluster` moves.
            read_only: serves_reads_only(store, record, &role),
            runtime: cluster_runtime_of(store, record),
        },
        database,
    ))
}

/// Whether an endpoint of the cluster `record` in the role `role` serves NO writes, read
/// under the state lock the caller holds.
///
/// The one reading of it, so a login and every later statement of the same session agree:
/// the writer endpoint writes, the reader endpoint never does, and a MEMBER's own endpoint
/// writes exactly while that member is the one the cluster writes through — the member
/// `DBClusterMembers` reports `IsClusterWriter` for, which is what `FailoverDBCluster`
/// moves.
pub(super) fn serves_reads_only(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    record: &DbClusterRecord,
    role: &EndpointRole,
) -> bool {
    match role {
        EndpointRole::Writer => false,
        EndpointRole::Reader => true,
        EndpointRole::ClusterMember(identifier) => store
            .cluster_members(record)
            .first()
            .is_none_or(|writer| !writer.identifier.eq_ignore_ascii_case(identifier)),
    }
}

/// The engine runtime of the cluster `record`, read under the state lock the caller holds.
///
/// The parameters are the cluster-level ones — a DB CLUSTER parameter group's own catalog
/// ([`crate::state::published_cluster_parameters`]) over the values the cluster's group sets
/// — because those are the parameters a cluster is configured by. A cluster's engine came up
/// when the cluster was created: nothing this service serves restarts it, so the creation
/// time is when the server has been running since.
pub(super) fn cluster_runtime_of(
    store: &ComAmazonawsRdsAmazonrdsv19State,
    record: &DbClusterRecord,
) -> EngineRuntime {
    let group = record
        .db_cluster_parameter_group_name
        .as_ref()
        .and_then(|name| {
            store
                .db_cluster_parameter_groups
                .get(&(record.region.clone(), name.to_ascii_lowercase()))
        });
    let family = parameter_group_family(
        group.map(|group| group.family.as_str()),
        record.db_cluster_parameter_group_name.as_deref(),
    );
    running_engine(
        &family,
        crate::state::published_cluster_parameters(&family),
        group.map(|group| &group.parameters),
        record.cluster_create_time.as_secs_f64(),
        record.master_username.clone(),
    )
}
