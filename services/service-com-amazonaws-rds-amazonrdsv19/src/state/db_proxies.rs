// SPDX-License-Identifier: CC0-1.0

//! The RDS Proxy records `CreateDBProxy`, `DescribeDBProxies`,
//! `RegisterDBProxyTargets`, `DescribeDBProxyTargets` and `DeregisterDBProxyTargets`
//! share.
//!
//! A proxy's TARGETS are stored as registrations — which DB instance or DB cluster was
//! registered, and nothing else — so everything a target reports about the database behind
//! it (its ARN, its endpoint, its port) is read from that database's own record when the
//! target is projected. A deleted database therefore cannot leave a target reporting an
//! endpoint that no longer exists.

use std::time::Instant;

use super::{PendingChange, PendingStatus, ARN_NAMESPACE, STATUS_TRANSITION_DELAY};
use crate::ops::types::{
    ConnectionPoolConfiguration, ConnectionPoolConfigurationInfo, DBProxy, DBProxyTarget,
    DBProxyTargetGroup, Tag, TargetHealth, UserAuthConfig, UserAuthConfigInfo,
};

/// How many proxies one region holds. The cap is this emulator's, and it bounds the store
/// the same way the other per-region quotas in `src/state.rs` do.
pub const DB_PROXY_QUOTA_PER_REGION: usize = 20;

/// How many targets one proxy's target group holds. "Currently, each target group is
/// associated with exactly one RDS DB instance or Aurora DB cluster"
/// (`DBProxyTargetGroup`), so this is generous; it bounds a caller-sized list that enters
/// the store.
pub const DB_PROXY_TARGET_QUOTA_PER_PROXY: usize = 20;

/// `DBProxyStatus` — the status a just-created proxy reports while this service brings it
/// up.
pub const DB_PROXY_STATUS_CREATING: &str = "creating";

/// `DBProxyStatus` — "A status of `available` means the proxy is ready to handle
/// requests."
pub const DB_PROXY_STATUS_AVAILABLE: &str = "available";

/// `DBProxyStatus` — the status a proxy reports while this service applies a change a
/// modification asked for, and which it leaves for [`DB_PROXY_STATUS_AVAILABLE`] on its
/// own.
pub const DB_PROXY_STATUS_MODIFYING: &str = "modifying";

/// `DBProxyStatus` — the status a proxy reports once a delete has been accepted for it and
/// before this service has finished taking it away.
pub const DB_PROXY_STATUS_DELETING: &str = "deleting";

/// The only target group name a proxy has: "Currently, this name must be `default`"
/// (`RegisterDBProxyTargets$TargetGroupName`), because each proxy has exactly one target
/// group and the service names it that.
pub const DEFAULT_DB_PROXY_TARGET_GROUP: &str = "default";

/// `CreateDBProxy$IdleClientTimeout`'s documented default: "Default: 1800 (30 minutes)".
pub const DEFAULT_IDLE_CLIENT_TIMEOUT: i32 = 1800;

/// The `EngineFamily` whose connection pool defaults differ from every other engine's.
pub const ENGINE_FAMILY_SQLSERVER: &str = "SQLSERVER";

/// `ConnectionPoolConfiguration$MaxConnectionsPercent`'s documented default for RDS for
/// Microsoft SQL Server: "Default: 10 for RDS for Microsoft SQL Server, and 100 for all
/// other engines".
pub const DEFAULT_MAX_CONNECTIONS_PERCENT_SQLSERVER: i32 = 10;

/// `ConnectionPoolConfiguration$MaxConnectionsPercent`'s documented default for every
/// other engine (same sentence as above).
pub const DEFAULT_MAX_CONNECTIONS_PERCENT: i32 = 100;

/// `ConnectionPoolConfiguration$ConnectionBorrowTimeout`'s documented default:
/// "Default: 120" seconds.
pub const DEFAULT_CONNECTION_BORROW_TIMEOUT: i32 = 120;

/// The connection pool settings a proxy's target group starts with, which are the ones
/// `ConnectionPoolConfiguration` documents as its defaults for `engine_family`:
/// `MaxConnectionsPercent` is engine-family dependent, and "the default value of
/// `MaxIdleConnectionsPercent` is half of the value of `MaxConnectionsPercent`" — which is
/// the 5 and 50 that member documents for SQL Server and for all other engines.
pub fn default_connection_pool_config(engine_family: &str) -> ConnectionPoolConfigurationInfo {
    let max_connections_percent = if engine_family == ENGINE_FAMILY_SQLSERVER {
        DEFAULT_MAX_CONNECTIONS_PERCENT_SQLSERVER
    } else {
        DEFAULT_MAX_CONNECTIONS_PERCENT
    };
    ConnectionPoolConfigurationInfo {
        max_connections_percent: Some(max_connections_percent),
        max_idle_connections_percent: Some(max_connections_percent / 2),
        connection_borrow_timeout: Some(DEFAULT_CONNECTION_BORROW_TIMEOUT),
        // "Each item in the list represents a class of SQL operations" exempted from
        // pinning: a target group starts with none exempted.
        session_pinning_filters: Some(Vec::new()),
        // "One or more SQL statements for the proxy to run when opening each new database
        // connection" — none until a modification sets one.
        init_query: None,
    }
}

/// `ConnectionPoolConfiguration$MaxConnectionsPercent`'s published constraint: "Must be
/// between 1 and 100."
const MAX_CONNECTIONS_PERCENT_RANGE: ::std::ops::RangeInclusive<i32> = 1..=100;

/// `ConnectionPoolConfiguration$ConnectionBorrowTimeout`'s published constraint: "Must be
/// between 0 and 300."
const CONNECTION_BORROW_TIMEOUT_RANGE: ::std::ops::RangeInclusive<i32> = 0..=300;

/// How many session pinning filters one target group holds. The cap is this emulator's: it
/// bounds a caller-sized list that enters the store, which the API publishes no limit for.
pub const MAX_SESSION_PINNING_FILTERS: usize = 20;

/// The sentence `ModifyDBProxyTargetGroup` refuses a rename of the group every proxy has
/// with: "You can't rename the default target group" (`ModifyDBProxyTargetGroup$NewName`),
/// and that group is the only one a proxy has.
pub const DEFAULT_TARGET_GROUP_RENAME_MESSAGE: &str =
    "You can't rename the default target group.";

/// The settings a target group holds once `ConnectionPoolConfiguration` has been applied
/// over them.
///
/// `ModifyDBProxyTargetGroup` carries the settings a caller wants CHANGED, so a member the
/// request leaves out keeps the value the group already holds — which is what makes the
/// call that sets an initialization query alone leave the pool sizes as they were. `Err`
/// carries the sentence a value outside the constraints that shape publishes is refused
/// with: each range is quoted on the constant that holds it, and
/// `MaxIdleConnectionsPercent` "must be between 0 and the value of
/// `MaxConnectionsPercent`" — the value in force after this call, not the one the group
/// held before it.
///
/// `MaxIdleConnectionsPercent` is the one member whose value is DERIVED rather than held:
/// its own doc states it as a default computed from the pool size — "The default value is
/// half of the value of `MaxConnectionsPercent`. For example, if `MaxConnectionsPercent` is
/// 80, then the default value of `MaxIdleConnectionsPercent` is 40" — so whenever a call
/// puts a new pool size in force without stating an idle percentage, that percentage is the
/// default for the size now in force, on every such change and not only at creation. Where
/// the call states both, the stated percentage is what stands. No other member is derived
/// from another: `ConnectionBorrowTimeout`, `SessionPinningFilters` and `InitQuery` each
/// publish a default of their own, so a call that does not state one leaves it as it is.
pub fn merged_connection_pool_config(
    held: &ConnectionPoolConfigurationInfo,
    requested: &ConnectionPoolConfiguration,
) -> Result<ConnectionPoolConfigurationInfo, String> {
    let mut merged = held.clone();
    if let Some(percent) = requested.max_connections_percent {
        check_range(
            "MaxConnectionsPercent",
            percent,
            &MAX_CONNECTIONS_PERCENT_RANGE,
        )?;
        merged.max_connections_percent = Some(percent);
        merged.max_idle_connections_percent = Some(percent / 2);
    }
    if let Some(timeout) = requested.connection_borrow_timeout {
        check_range(
            "ConnectionBorrowTimeout",
            timeout,
            &CONNECTION_BORROW_TIMEOUT_RANGE,
        )?;
        merged.connection_borrow_timeout = Some(timeout);
    }
    if let Some(percent) = requested.max_idle_connections_percent {
        let ceiling = merged
            .max_connections_percent
            .unwrap_or(*MAX_CONNECTIONS_PERCENT_RANGE.end());
        check_range("MaxIdleConnectionsPercent", percent, &(0..=ceiling))?;
        merged.max_idle_connections_percent = Some(percent);
    }
    if let Some(filters) = &requested.session_pinning_filters {
        super::check_request_list_len(
            "SessionPinningFilters",
            filters.len(),
            MAX_SESSION_PINNING_FILTERS,
        )?;
        for filter in filters {
            super::check_free_form("SessionPinningFilters", filter)?;
        }
        merged.session_pinning_filters = Some(filters.clone());
    }
    if let Some(query) = &requested.init_query {
        super::check_free_form("InitQuery", query)?;
        merged.init_query = Some(query.clone());
    }
    Ok(merged)
}

/// Refuse a connection pool setting outside the range that member's own doc publishes.
fn check_range(
    member: &str,
    value: i32,
    range: &::std::ops::RangeInclusive<i32>,
) -> Result<(), String> {
    if range.contains(&value) {
        return Ok(());
    }
    Err(format!(
        "Invalid value {value} for {member}. Must be between {} and {}.",
        range.start(),
        range.end()
    ))
}

/// `CreateDBProxy$DefaultAuthScheme`'s default, the `DefaultAuthScheme` value that leaves
/// authentication to the `Auth` list rather than using end-to-end IAM authentication.
pub const DEFAULT_AUTH_SCHEME_NONE: &str = "NONE";

/// `EndpointNetworkType` / `TargetConnectionNetworkType`: the IPv4 value both default to,
/// the only network type an emulated proxy endpoint serves.
pub const NETWORK_TYPE_IPV4: &str = "IPV4";

/// `TargetHealth$State` — "`AVAILABLE`", the state of a registered target whose database
/// this service still holds and whose registration has come into force.
pub const TARGET_STATE_AVAILABLE: &str = "AVAILABLE";

/// `TargetHealth$State` — "`REGISTERING`", the state a registration reports while this
/// service is still putting it in force.
///
/// `TargetHealth$State` publishes the lifecycle a target walks — "`registering` >
/// `unavailable` > `available`" — so the state a registration is made in is not the state
/// it settles on: a caller that polls the target right after registering it sees this one
/// first, and [`TARGET_STATE_AVAILABLE`] [`STATUS_TRANSITION_DELAY`] later.
pub const TARGET_STATE_REGISTERING: &str = "REGISTERING";

/// `TargetHealth$State` — "`UNAVAILABLE`", the state of a registered target whose database
/// is gone.
pub const TARGET_STATE_UNAVAILABLE: &str = "UNAVAILABLE";

/// `TargetHealthReason` — "`UNREACHABLE`", why a target whose database is gone is
/// unavailable.
pub const TARGET_HEALTH_REASON_UNREACHABLE: &str = "UNREACHABLE";

/// `TargetType` — "`RDS_INSTANCE`", the kind of database a registered DB instance is.
pub const TARGET_TYPE_RDS_INSTANCE: &str = "RDS_INSTANCE";

/// `TargetType` — "`TRACKED_CLUSTER`", the kind of database a registered DB cluster is.
pub const TARGET_TYPE_TRACKED_CLUSTER: &str = "TRACKED_CLUSTER";

/// `TargetRole` — "`READ_WRITE`", the role of the writer a proxy connects to.
pub const TARGET_ROLE_READ_WRITE: &str = "READ_WRITE";

/// `TargetRole` — "`READ_ONLY`", the role of a reader a proxy connects to: a DB instance
/// of a tracked DB cluster that is not that cluster's writer serves read-only operations.
pub const TARGET_ROLE_READ_ONLY: &str = "READ_ONLY";

/// The ARN resource type of a proxy, as AWS writes it under the `rds` namespace.
const ARN_RESOURCE_TYPE: &str = "db-proxy";

/// The ARN resource type of a proxy's target group.
const TARGET_GROUP_ARN_RESOURCE_TYPE: &str = "target-group";

/// The message a `DBProxyNotFoundFault` carries for a `DBProxyName` that resolves no proxy:
/// the fault reports "the DB proxy name doesn't refer to an existing DB proxy", and Amazon
/// RDS words it with the requested name in SINGLE QUOTES inside the sentence.
///
/// It lives here rather than in one handler because every operation that resolves a
/// `DBProxyName` answers the same one fault, and the quoting is part of the text a caller
/// matches the refusal against. `named` is the value the REQUEST carried, which is the
/// spelling a proxy is stored under too (`DBProxyName` is not folded).
pub fn db_proxy_not_found_message(named: &str) -> String {
    format!("DBProxy '{named}' not found.")
}

/// Refuse an `Auth` entry carrying text this store will not hold.
///
/// `UserAuthConfigList`'s published length: "len 0..200" (`smithy_query(query="shape",
/// shape="UserAuthConfigList")`), so a request stating more entries than that states more
/// than the member accepts. The COUNT is bounded here beside each entry's strings, because
/// the whole list is what enters the store and outlives the request.
pub const MAX_USER_AUTH_CONFIGS: usize = 200;

/// One check for `CreateDBProxy`'s authentication settings and for the ones a modification
/// replaces them with, so neither can take a value the other refuses. `Err` carries the
/// sentence naming the member at fault.
pub fn check_user_auth_config(entries: &[UserAuthConfig]) -> Result<(), String> {
    super::check_request_list_len("Auth", entries.len(), MAX_USER_AUTH_CONFIGS)?;
    for entry in entries {
        for (member, value) in [
            ("Auth.Description", entry.description.as_deref()),
            ("Auth.UserName", entry.user_name.as_deref()),
            ("Auth.AuthScheme", entry.auth_scheme.as_deref()),
            ("Auth.SecretArn", entry.secret_arn.as_deref()),
            ("Auth.IAMAuth", entry.iam_auth.as_deref()),
            (
                "Auth.ClientPasswordAuthType",
                entry.client_password_auth_type.as_deref(),
            ),
        ] {
            if let Some(value) = value {
                super::check_free_form(member, value)?;
            }
        }
    }
    Ok(())
}

/// Project the authentication settings a request carries onto the ones a proxy reports
/// back: `UserAuthConfigInfo` "returns the details of authentication used by a proxy", one
/// entry per `UserAuthConfig` the request stated, member for member.
///
/// One projection for the create that sets a proxy's authentication settings and the
/// modification that replaces them.
pub fn user_auth_config_to_info(entries: Vec<UserAuthConfig>) -> Vec<UserAuthConfigInfo> {
    entries
        .into_iter()
        .map(|entry| UserAuthConfigInfo {
            description: entry.description,
            user_name: entry.user_name,
            auth_scheme: entry.auth_scheme,
            secret_arn: entry.secret_arn,
            iam_auth: entry.iam_auth,
            client_password_auth_type: entry.client_password_auth_type,
        })
        .collect()
}

/// The message a `DBProxyAlreadyExistsFault` carries for a name the region already holds a
/// proxy under. The sentence is this service's chosen form, carried unchanged from the
/// create that first answered the fault: it states that a proxy of the requested name
/// exists, which is what the fault reports ("The specified proxy name must be unique for
/// all proxies owned by your Amazon Web Services account in the specified Amazon Web
/// Services Region").
///
/// It lives here rather than in one handler because the create that mints a proxy under a
/// name and the modification that renames one onto a name answer the same one fault, so
/// neither can drift from the other. `named` is the value the REQUEST carried, which is the
/// spelling a proxy is stored under too (`DBProxyName` is not folded).
pub fn db_proxy_already_exists_message(named: &str) -> String {
    format!("A DB proxy named {named} already exists")
}

/// The sentence `RegisterDBProxyTargets` and `DeregisterDBProxyTargets` refuse a request
/// that names no database to act on with.
///
/// Both members that name one are optional, so a request can carry neither — and a
/// registration of nothing is not a registration. Amazon RDS words the refusal as a
/// statement about what is missing, naming the two KINDS of database rather than the
/// modelled member names, and interpolates nothing from the request.
pub const MISSING_DB_PROXY_TARGET_IDENTIFIER_MESSAGE: &str =
    "Missing DB instance or DB cluster identifier.";

/// The sentence `DeregisterDBProxyTargets` refuses a request naming a database that the
/// proxy's target group does not have registered with — `DBProxyTargetNotFoundFault`,
/// "the specified RDS DB instance or Aurora DB cluster isn't available for a proxy owned
/// by your Amazon Web Services account".
///
/// Amazon RDS words it as a fixed statement about the SET of targets the request named and
/// the group does not hold, interpolating nothing from the request: one call can name
/// several databases, and the fault reports that targets of it are missing rather than
/// which one was looked up first.
pub const MISSING_DB_PROXY_TARGETS_MESSAGE: &str = "Missing targets";

/// The sentence `RegisterDBProxyTargets` refuses a registration that would make the proxy
/// reach a database it already reaches with — `DBProxyTargetAlreadyRegisteredFault`.
///
/// It is the statement the fault itself publishes ("The proxy is already associated with
/// the specified RDS DB instance or Aurora DB cluster"), fixed text naming the two KINDS of
/// database and interpolating nothing from the request, as the other fixed refusals of
/// these operations do.
pub const DB_PROXY_TARGET_ALREADY_REGISTERED_MESSAGE: &str =
    "The proxy is already associated with the specified RDS DB instance or Aurora DB cluster";

/// Refuse a `TargetGroupName` that is not the one target group a proxy has.
///
/// "Currently, this name must be `default`" — each proxy has exactly one target group and
/// the service names it that, so any other name names no target group of this proxy, which
/// is what `DBProxyTargetGroupNotFoundFault` reports. `Err` carries the message.
pub fn check_target_group_name(proxy_name: &str, requested: Option<&str>) -> Result<(), String> {
    match requested {
        None => Ok(()),
        Some(name) if name == DEFAULT_DB_PROXY_TARGET_GROUP => Ok(()),
        Some(name) => Err(format!(
            "DBProxyTargetGroup {name} not found for DB proxy {proxy_name}: a proxy has one \
             target group, named {DEFAULT_DB_PROXY_TARGET_GROUP}"
        )),
    }
}

/// The one target group a proxy has, as this service persists it.
///
/// "Currently, each target group is associated with exactly one RDS DB instance or Aurora
/// DB cluster" and each proxy has exactly one target group, so the group is held on the
/// proxy record rather than keyed separately; its NAME is therefore not stored (it is
/// always [`DEFAULT_DB_PROXY_TARGET_GROUP`], which is what `check_target_group_name`
/// resolves against) and its registrations live in `DbProxyRecord::targets`. What IS
/// stored is what a modification of the group changes: its connection pool settings, and
/// when the group itself was created and last updated.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DbProxyTargetGroupRecord {
    /// `DBProxyTargetGroup$ConnectionPoolConfig`.
    pub connection_pool_config: ConnectionPoolConfigurationInfo,
    /// `DBProxyTargetGroup$CreatedDate`.
    #[serde(with = "::service_common::persist::smithy_time")]
    pub created_date: ::aws_smithy_types::DateTime,
    /// `DBProxyTargetGroup$UpdatedDate`.
    #[serde(with = "::service_common::persist::smithy_time")]
    pub updated_date: ::aws_smithy_types::DateTime,
}

impl DbProxyTargetGroupRecord {
    /// The target group a proxy of `engine_family` is created with at `now`: the
    /// documented default connection pool settings, created and last updated at the
    /// instant the proxy was.
    pub fn new(engine_family: &str, now: ::aws_smithy_types::DateTime) -> Self {
        Self {
            connection_pool_config: default_connection_pool_config(engine_family),
            created_date: now,
            updated_date: now,
        }
    }
}

/// One registration in a proxy's target group: which database was registered, and when.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DbProxyTargetRecord {
    /// The `DBInstanceIdentifier` or `DBClusterIdentifier` that was registered, stored
    /// lowercased like the record it names.
    pub identifier: String,
    /// Whether the registration names a DB CLUSTER rather than a DB instance.
    pub is_cluster: bool,
    /// The instant from which this registration is in force, so the target reports
    /// [`TARGET_STATE_AVAILABLE`] rather than [`TARGET_STATE_REGISTERING`]. Nothing
    /// sleeps and nothing writes it back: the health a target reports is read from this
    /// instant when the target is projected, so it is the clock alone that moves it.
    #[serde(with = "crate::state::serde_instant_deadline")]
    pub healthy_at: Instant,
}

impl DbProxyTargetRecord {
    /// The registration of `identifier` this call makes: in force
    /// [`STATUS_TRANSITION_DELAY`] from now, which is the span this service takes to put
    /// one into force. The span is never zero, so the caller that registered a target
    /// still observes the `registering` state its health lifecycle starts in.
    pub fn registering(identifier: String, is_cluster: bool) -> Self {
        Self {
            identifier,
            is_cluster,
            healthy_at: Instant::now() + STATUS_TRANSITION_DELAY,
        }
    }

    /// Whether the registration is in force at `now`.
    pub fn is_in_force(&self, now: Instant) -> bool {
        now >= self.healthy_at
    }
}

/// A persisted RDS proxy.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DbProxyRecord {
    /// `DBProxyName`, which is half this record's key.
    pub name: String,
    /// The region whose credential scope the create was signed with.
    pub region: String,
    /// `EngineFamily`.
    pub engine_family: String,
    /// `DefaultAuthScheme`.
    pub default_auth_scheme: String,
    /// `Auth`, as the proxy reports it back.
    pub auth: Vec<UserAuthConfigInfo>,
    /// `RoleArn`.
    pub role_arn: String,
    /// `VpcSubnetIds`.
    pub vpc_subnet_ids: Vec<String>,
    /// `VpcSecurityGroupIds`.
    pub vpc_security_group_ids: Vec<String>,
    /// `RequireTLS`.
    pub require_tls: bool,
    /// `IdleClientTimeout`.
    pub idle_client_timeout: i32,
    /// `DebugLogging`.
    pub debug_logging: bool,
    /// `EndpointNetworkType`.
    pub endpoint_network_type: String,
    /// `TargetConnectionNetworkType`.
    pub target_connection_network_type: String,
    /// The minted DNS label the proxy's endpoint carries.
    pub endpoint_host: String,
    /// `Status`.
    pub status: String,
    /// The status change the clock owes this record, if any.
    pub pending_status: Option<PendingStatus>,
    /// `CreatedDate`.
    #[serde(with = "::service_common::persist::smithy_time")]
    pub created_date: ::aws_smithy_types::DateTime,
    /// `UpdatedDate`.
    #[serde(with = "::service_common::persist::smithy_time")]
    pub updated_date: ::aws_smithy_types::DateTime,
    /// The proxy's one target group, which it is created with.
    pub target_group: DbProxyTargetGroupRecord,
    /// The databases registered in the proxy's one target group, in registration order.
    pub targets: Vec<DbProxyTargetRecord>,
    /// The tags the create carried. The `DBProxy` shape reports none, so these are held
    /// for the tagging operations rather than projected onto the proxy.
    pub tags: Vec<Tag>,
}

impl DbProxyRecord {
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

    /// Whether a delete has been accepted for the proxy, so it is on its way out of this
    /// service: the clock owes its removal and there is nothing left for a modification of
    /// it, or of the target group it takes with it, to act on. Named as the crate's other
    /// records name the same predicate.
    pub fn is_deleting(&self) -> bool {
        self.status == DB_PROXY_STATUS_DELETING
    }

    /// Accept a modification: the change is already applied to the record, and the proxy
    /// reports `modifying` until this service has finished putting it in force
    /// [`crate::state::STATUS_TRANSITION_DELAY`] from now, after which it is `available`
    /// again. Nothing sleeps: the due instant is stored and materialised by
    /// `settle_due_transitions`, so a caller reading the proxy straight afterwards still
    /// sees it applying the change.
    pub fn begin_modifying(&mut self) {
        self.status = DB_PROXY_STATUS_MODIFYING.to_string();
        self.pending_status = Some(PendingStatus::in_a_moment(DB_PROXY_STATUS_AVAILABLE));
    }

    /// Accept a delete: the proxy reports `deleting` from now on, and the clock owes its
    /// removal from the store [`crate::state::STATUS_TRANSITION_DELAY`] from now, after
    /// which every read of the name is a `DBProxyNotFoundFault`. Nothing sleeps: the due
    /// instant is stored and materialised by `settle_due_transitions`.
    pub fn begin_deletion(&mut self) {
        self.status = DB_PROXY_STATUS_DELETING.to_string();
        self.pending_status = Some(PendingStatus::removal_in_a_moment());
    }

    /// `DBProxyArn`, from the stored region and the proxy's name.
    pub fn arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("{ARN_RESOURCE_TYPE}:{}", self.name),
        )
    }

    /// `DBProxyTargetGroup$TargetGroupArn` for this proxy's one target group.
    pub fn target_group_arn(&self) -> String {
        emulator_server::emulator_arn(
            ARN_NAMESPACE,
            &self.region,
            &format!("{TARGET_GROUP_ARN_RESOURCE_TYPE}:{}", self.name),
        )
    }

    /// `DBProxyTargetGroup$Status` for the proxy's one target group.
    ///
    /// "A status of `available` means the target group is correctly associated with a
    /// database. Other values indicate that you must wait for the target group to be
    /// ready": the group is created WITH the proxy and has no provisioning of its own, so
    /// it is ready exactly when the proxy record exists — a proxy that is still coming up
    /// already has its group, and there is nothing for a caller to wait for. A proxy on
    /// its way out takes its group with it, so the group then reports the status the proxy
    /// is leaving in.
    pub fn target_group_status(&self) -> &str {
        if self.is_deleting() {
            DB_PROXY_STATUS_DELETING
        } else {
            DB_PROXY_STATUS_AVAILABLE
        }
    }

    /// Project the proxy's one target group onto the modelled `DBProxyTargetGroup`.
    ///
    /// `IsDefault` is `true` because "each proxy is currently associated with a single
    /// target group", and the group's readiness is [`Self::target_group_status`].
    pub fn target_group_to_wire(&self) -> DBProxyTargetGroup {
        DBProxyTargetGroup {
            db_proxy_name: Some(self.name.clone()),
            target_group_name: Some(DEFAULT_DB_PROXY_TARGET_GROUP.to_string()),
            target_group_arn: Some(self.target_group_arn()),
            is_default: Some(true),
            status: Some(self.target_group_status().to_string()),
            connection_pool_config: Some(self.target_group.connection_pool_config.clone()),
            created_date: Some(self.target_group.created_date),
            updated_date: Some(self.target_group.updated_date),
        }
    }

    /// "The endpoint that you can use to connect to the DB proxy", whose published form is
    /// `<proxy name>.proxy-<server-assigned label>.<region>.rds.amazonaws.com` (Amazon RDS
    /// User Guide, "Connecting to a database through RDS Proxy").
    pub fn endpoint(&self) -> String {
        format!(
            "{}.proxy-{}.{}.rds.amazonaws.com",
            self.name, self.endpoint_host, self.region
        )
    }

    /// Whether `identifier` is already registered in the proxy's target group.
    pub fn target_position(&self, identifier: &str, is_cluster: bool) -> Option<usize> {
        self.targets
            .iter()
            .position(|target| target.identifier == identifier && target.is_cluster == is_cluster)
    }

    /// Project the stored record onto the modelled `DBProxy` shape. `VpcId` is left out
    /// rather than invented: this service holds no VPC, so a proxy it stores belongs to
    /// none it could name.
    pub fn to_wire(&self) -> DBProxy {
        DBProxy {
            db_proxy_name: Some(self.name.clone()),
            db_proxy_arn: Some(self.arn()),
            status: Some(self.status.clone()),
            engine_family: Some(self.engine_family.clone()),
            vpc_id: None,
            vpc_security_group_ids: Some(self.vpc_security_group_ids.clone()),
            vpc_subnet_ids: Some(self.vpc_subnet_ids.clone()),
            default_auth_scheme: Some(self.default_auth_scheme.clone()),
            auth: Some(self.auth.clone()),
            role_arn: Some(self.role_arn.clone()),
            endpoint: Some(self.endpoint()),
            require_tls: Some(self.require_tls),
            idle_client_timeout: Some(self.idle_client_timeout),
            debug_logging: Some(self.debug_logging),
            created_date: Some(self.created_date),
            updated_date: Some(self.updated_date),
            endpoint_network_type: Some(self.endpoint_network_type.clone()),
            target_connection_network_type: Some(self.target_connection_network_type.clone()),
        }
    }
}

/// WHICH database a proxy target reports: the identifier it is reported under and whether
/// that identifier names a DB CLUSTER.
///
/// Both halves are needed because the two kinds of database are separate namespaces — a DB
/// instance and a DB cluster of one and the same identifier are two databases — so this,
/// and not the identifier alone, is what decides whether two targets report the same
/// database.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProxyTargetKey {
    /// `RdsResourceId`.
    pub identifier: String,
    /// Whether the target reports a DB cluster rather than a DB instance.
    pub is_cluster: bool,
}

/// One row of a proxy's target report: the database the target reports, how the proxy
/// reaches it, and whether the registration behind it is in force yet.
///
/// A row is built either from a registration itself ([`Self::registration`]) or from a DB
/// instance that a registered DB cluster tracks ([`Self::tracked_instance`]), so every
/// target — however it came to be in the report — carries ONE identity rule and ONE health
/// rule.
#[derive(Debug, Clone)]
pub struct ProxyTargetRow {
    /// `RdsResourceId`: "the identifier representing the target. It can be the instance
    /// identifier for an RDS DB instance, or the cluster identifier for an Aurora DB
    /// cluster" — never a server-assigned id the caller never named.
    identifier: String,
    /// `Type`.
    target_type: &'static str,
    /// `TrackedClusterId`: the DB cluster the target is reached through, which only a
    /// target that came from a registered cluster has.
    tracked_cluster_id: Option<String>,
    /// `Role`.
    role: &'static str,
    /// What the database behind the target reports, read from that database's own record,
    /// or `None` when this service no longer holds it.
    database: Option<RegisteredDatabase>,
    /// Whether the registration behind the target is in force (see
    /// [`DbProxyTargetRecord::is_in_force`]).
    in_force: bool,
}

impl ProxyTargetRow {
    /// The target for the registration itself: the DB instance or the DB cluster the
    /// caller named, reported under the identifier it was registered as.
    ///
    /// `database` is `None` when that DB instance or DB cluster is no longer in the store:
    /// the registration stands, and the target reports the health of one whose database
    /// cannot be reached rather than an endpoint that answers nothing.
    pub fn registration(
        target: &DbProxyTargetRecord,
        database: Option<RegisteredDatabase>,
        now: Instant,
    ) -> Self {
        Self {
            identifier: target.identifier.clone(),
            target_type: if target.is_cluster {
                TARGET_TYPE_TRACKED_CLUSTER
            } else {
                TARGET_TYPE_RDS_INSTANCE
            },
            // "The DB cluster identifier when the target represents an Aurora DB cluster.
            // This field is blank when the target represents an RDS DB instance"
            // (`DBProxyTarget$TrackedClusterId`).
            tracked_cluster_id: target.is_cluster.then(|| target.identifier.clone()),
            role: TARGET_ROLE_READ_WRITE,
            database,
            in_force: target.is_in_force(now),
        }
    }

    /// The target for one DB instance a registered DB cluster tracks: "a proxy that is
    /// associated with a DB cluster can connect to any of its DB instances"
    /// (`TargetType$TRACKED_CLUSTER` — the proxy tracks the cluster's instances), so the
    /// report carries one `RDS_INSTANCE` target per member beside the cluster's own
    /// target, each naming the cluster it is reached through and the operations it can
    /// serve: the writer "can be used for read/write" and every reader for read-only ones
    /// (`DBProxyTarget$Role`).
    ///
    /// The members are read from the cluster's CURRENT membership rather than stored, so a
    /// member added to or taken out of the cluster after the registration is reported
    /// exactly as the cluster reports it.
    pub fn tracked_instance(
        tracked_cluster_id: &str,
        identifier: &str,
        database: Option<RegisteredDatabase>,
        writes: bool,
        in_force: bool,
    ) -> Self {
        Self {
            identifier: identifier.to_string(),
            target_type: TARGET_TYPE_RDS_INSTANCE,
            tracked_cluster_id: Some(tracked_cluster_id.to_string()),
            role: if writes {
                TARGET_ROLE_READ_WRITE
            } else {
                TARGET_ROLE_READ_ONLY
            },
            database,
            in_force,
        }
    }

    /// WHICH database this target reports, as everything that has to tell two targets
    /// apart keys it: the identifier AND whether it is a DB cluster, because a DB instance
    /// and a DB cluster are separate namespaces and may carry one and the same identifier.
    pub fn database_key(&self) -> ProxyTargetKey {
        ProxyTargetKey {
            identifier: self.identifier.clone(),
            is_cluster: self.target_type == TARGET_TYPE_TRACKED_CLUSTER,
        }
    }

    /// The key the report pages on — one database is one target, so this is
    /// [`Self::database_key`] written as the opaque text a `Marker` carries.
    pub fn page_key(&self) -> String {
        format!("{}:{}", self.target_type, self.identifier)
    }

    /// Project the row onto the modelled `DBProxyTarget`.
    pub fn to_wire(self) -> DBProxyTarget {
        let tracked_cluster_id = self.tracked_cluster_id;
        let rds_resource_id = Some(self.identifier.clone());
        let r#type = Some(self.target_type.to_string());
        let role = Some(self.role.to_string());
        match self.database {
            Some(database) => DBProxyTarget {
                target_arn: Some(database.arn),
                endpoint: Some(database.endpoint),
                tracked_cluster_id,
                rds_resource_id,
                port: Some(database.port),
                r#type,
                role,
                // A registration this service has not finished putting in force is still
                // `registering`; only the clock moves it on, and "if the State is
                // AVAILABLE, a description is not included"
                // (`TargetHealth$Description`).
                target_health: Some(TargetHealth {
                    state: Some(if self.in_force {
                        TARGET_STATE_AVAILABLE.to_string()
                    } else {
                        TARGET_STATE_REGISTERING.to_string()
                    }),
                    reason: None,
                    description: None,
                }),
            },
            None => DBProxyTarget {
                target_arn: None,
                endpoint: None,
                tracked_cluster_id,
                rds_resource_id,
                port: None,
                r#type,
                role,
                target_health: Some(TargetHealth {
                    state: Some(TARGET_STATE_UNAVAILABLE.to_string()),
                    reason: Some(TARGET_HEALTH_REASON_UNREACHABLE.to_string()),
                    description: Some(format!(
                        "{} {} is no longer available",
                        if self.target_type == TARGET_TYPE_TRACKED_CLUSTER {
                            "DB cluster"
                        } else {
                            "DB instance"
                        },
                        self.identifier
                    )),
                }),
            },
        }
    }
}

/// What a proxy target reports about the database behind it, read from that database's own
/// record.
#[derive(Debug, Clone)]
pub struct RegisteredDatabase {
    /// The database's ARN.
    pub arn: String,
    /// The writer endpoint a client connects to.
    pub endpoint: String,
    /// The port the proxy connects to the database on.
    pub port: i32,
}
