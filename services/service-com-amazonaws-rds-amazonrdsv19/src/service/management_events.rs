// SPDX-License-Identifier: CC0-1.0

//! This front door's AUDIT leg: every call it serves is a management event of
//! `rds.amazonaws.com`, recorded in the account's event history where CloudTrail's
//! `LookupEvents` reads it.
//!
//! Every control-plane call of an account is a management event, which makes the
//! recording a property of SERVING a call and not of any one operation — so it is not
//! written per handler here either. The shared wrapper
//! ([`service_common::audited_operation`]) is handed the deserialized input before the
//! handler runs and the handler's own `Result` after, and this module is only what THIS
//! service contributes to each record:
//!
//! * its `eventSource` ([`EVENT_SOURCE`]);
//! * the Region and the account the call was served for;
//! * the members its model declares `@sensitive` ([`SENSITIVE_MEMBERS`]), whose values are
//!   replaced by the redaction literal and never stored;
//! * the resources a call addressed ([`RdsAudit::resources`]); and
//! * which operations REPORT state and which CHANGE it — the one fact a record cannot
//!   read off a request, so it is stated per operation in [`audited_table`].
//!
//! A call this emulator has not implemented answers 501 and is NOT recorded: the wrapper
//! drops it, because event history is the record of the calls an account's APIs actually
//! served.

use std::sync::Arc;

use service_common::audited_operation::{AuditedService, AuditedTable, CallKind};
use service_common::management_events::{
    AuditedResource, ManagementEventLog, RecordedSession, ServedApiCall,
};
use service_common::query_decimal_floats::DecimalFloatsOperation;
use smithy_server_runtime::{OperationContext, OperationRoutingTable, PreDispatchProtocol};

/// The `eventSource` every record of this service carries: this service's endpoint prefix
/// (`smithy_query(query="service_info")` -> `endpointPrefix: rds`) in the
/// `<prefix>.amazonaws.com` form CloudTrail stamps on every event.
///
/// The SERVICE's source, not the serving crate's: the Amazon DocumentDB and Amazon
/// Neptune views of this model are the same RDS front door, so a call reaching this
/// service is a call to Amazon RDS.
pub const EVENT_SOURCE: &str = "rds.amazonaws.com";

/// The request members this service's model declares `@sensitive`, named as the model
/// spells them.
///
/// Read off the model rather than guessed:
/// `smithy_query(query="operation",operation="CreateDBInstance,ModifyDBInstance,CreateDBCluster,ModifyDBCluster")`
/// reports each of these as `{"target": "SensitiveString", "sensitive": true}`. The set is
/// by member NAME, so it holds for every operation that carries a member of that name —
/// a password on a create and the same password on a modify are both withheld.
///
/// What it is for: a value held at one of these members is replaced by CloudTrail's fixed
/// redaction literal before anything is stored, so no plaintext a caller sent is
/// retrievable from event history.
pub const SENSITIVE_MEMBERS: &[&str] = &[
    // The master user's password, on every create, modify and restore that takes one.
    "MasterUserPassword",
    // The password of the TDE credential an instance is created or modified with.
    "TdeCredentialPassword",
    // The pre-signed URL a cross-Region copy or replica create carries: it is a signed
    // request, so it authorises the call it names.
    "PreSignedUrl",
];

/// The CloudFormation-style type name of a DB instance, as an event's `Resources` names
/// one.
const DB_INSTANCE_RESOURCE_TYPE: &str = "AWS::RDS::DBInstance";

/// The CloudFormation-style type name of a DB cluster.
const DB_CLUSTER_RESOURCE_TYPE: &str = "AWS::RDS::DBCluster";

/// What this service contributes to the record of each call it serves.
///
/// Holds the shared engine — the record goes into the log that engine carries, the one
/// CloudTrail reads — so it is generic over the engine type exactly as the service is.
pub struct RdsAudit<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> RdsAudit<Eng> {
    /// Build the recorder from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The account's log in that engine, or `None` for an engine that carries none.
    fn log(&self) -> Option<::engine::ServiceState<ManagementEventLog>> {
        crate::state::management_event_log(&*self.engine)
    }

    /// The identifier `member` names in `request_parameters`, folded to the resource name
    /// the store is keyed by (a member that accepts an ARN carries the resource in its
    /// last field).
    fn named<'a>(request_parameters: &'a serde_json::Value, member: &str) -> Option<&'a str> {
        request_parameters
            .get(member)
            .and_then(serde_json::Value::as_str)
            .map(crate::state::arn_resource_name_of)
    }
}

impl<Eng: Send + Sync + 'static> AuditedService for RdsAudit<Eng> {
    fn record(
        &self,
        call: ServedApiCall,
        answered_as: &str,
    ) -> Option<::service_common::api_call_events::DeliveredApiCall> {
        if !call.read_only && call.error_code.is_none() {
            if let Some(state) = crate::state::service_state(&*self.engine) {
                let store = state.lock();
                if let Err(error) = ::service_common::persist::save(&*store) {
                    ::tracing::warn!(%error, "could not persist RDS state snapshot");
                }
            }
        }
        let Some(log) = self.log() else {
            // Not silent: an unrecorded call leaves no trace in the log by definition, so
            // it leaves one here instead.
            ::tracing::warn!(
                operation = call.event_name,
                "the engine carries no management-event log: the call was served unrecorded"
            );
            return None;
        };
        // The record, and the event the account's CloudTrail fans out for it while a trail
        // of the Region is logging calls of its kind; publishing it is the wrapper's.
        // The guard is BOUND rather than used as a temporary: a local is released before
        // the handle it borrows, which a temporary at the end of this block would not be.
        let mut recording = log.lock();
        recording.record_delivered(
            call,
            ::std::time::SystemTime::now(),
            answered_as.to_string(),
        )
    }

    fn recorded_session(&self, access_key_id: &str) -> Option<RecordedSession> {
        // The account's own session directory, so a call made with a role session is
        // recorded as that session: the shared store the minting service wrote it to.
        crate::state::session_audit_directory(&*self.engine)
            .and_then(|directory| directory.lock().session(access_key_id).cloned())
    }

    fn event_source(&self) -> &'static str {
        EVENT_SOURCE
    }

    fn region(&self, ctx: &OperationContext<'_>) -> String {
        // The Region the call was served in, read the way every handler of this service
        // reads it. A scope this service would refuse names no Region, and the record
        // reports the Region a generated emulator defaults to rather than a value the
        // request made up.
        crate::state::request_region(ctx).unwrap_or_else(|_| DEFAULT_REGION.to_string())
    }

    fn account_id(&self) -> String {
        emulator_server::emulator_account_id().to_string()
    }

    fn resources(
        &self,
        operation: &str,
        request_parameters: &serde_json::Value,
        region: &str,
    ) -> Vec<AuditedResource> {
        // The two resources whose ARN this service mints, named by the member every
        // operation that addresses one carries. The member alone names the resource
        // kind here, so which operation served the call does not enter into it. A call
        // that names neither records no resource rather than one built from a member
        // whose resource kind is unknown.
        let _ = operation;
        let mut addressed = Vec::new();
        if let Some(identifier) = Self::named(
            request_parameters,
            &::service_common::management_events::recorded_member_name("DBInstanceIdentifier"),
        ) {
            addressed.push(AuditedResource::new(
                DB_INSTANCE_RESOURCE_TYPE,
                crate::state::db_instance_arn(region, identifier),
            ));
        }
        if let Some(identifier) = Self::named(
            request_parameters,
            &::service_common::management_events::recorded_member_name("DBClusterIdentifier"),
        ) {
            addressed.push(AuditedResource::new(
                DB_CLUSTER_RESOURCE_TYPE,
                crate::state::db_cluster_arn(region, identifier),
            ));
        }
        addressed
    }

    fn sensitive_members(&self) -> &'static [&'static str] {
        SENSITIVE_MEMBERS
    }
}

/// The Region a record reports for a call whose credential scope names none this service
/// accepts — the Region a generated emulator defaults to, which is also what
/// [`crate::state::request_region`] falls back to.
const DEFAULT_REGION: &str = "us-east-1";

/// The envelope a pre-dispatch failure of one of this service's operations renders in:
/// this service speaks `awsQuery` (not the EC2 dialect), which is the same value the
/// generated per-operation shim passes.
const QUERY_ENVELOPE: PreDispatchProtocol<'static> = PreDispatchProtocol::Query { ec2: false };

/// A call that only REPORTS state, as the table's rows name that kind.
const fn reports() -> CallKind {
    CallKind::Reporting
}

/// A call that CHANGES the account's resources.
const fn mutates() -> CallKind {
    CallKind::Mutating
}

/// Serve one operation of this service audited, as a call of the stated kind: `reports`
/// for an operation that only REPORTS state, `mutates` for one that CHANGES the account's
/// resources — which is the event's `readOnly`, and what a lookup can filter on.
///
/// Each operation is served behind one more wrapper, OUTSIDE the audited call: this front
/// door speaks an XML protocol, whose floating-point members carry a fractional part that
/// the framework's writer drops for a whole value (see
/// [`service_common::query_decimal_floats`]). It wraps the response's RENDERING only, so
/// the record the audited call writes still reports the operation's own output.
macro_rules! audited {
    (
        $table:expr, $engine:expr,
        $( $module:ident :: $operation:ident => $kind:ident ),* $(,)?
    ) => {{
        $(
            $table.serve_behind(
                crate::ops::$module::$operation::new(::std::sync::Arc::clone($engine)),
                $kind(),
                DecimalFloatsOperation::new,
            );
        )*
    }};
}

/// Every modelled operation of this service, each constructed from the shared engine and
/// served behind the audit recorder.
///
/// EVERY operation is here, the ones this emulator has not implemented included: a stub
/// answers 501 and the wrapper records nothing for it, so an operation implemented later
/// is auditable the moment it serves a call rather than when someone remembers to add it.
///
/// The kind per row is the operation's own semantics: `reports` for the `Describe*` /
/// `List*` readers and for `DownloadDBLogFilePortion`, which read state and change none
/// of it, and `mutates` for every operation that creates, changes, moves or removes a
/// resource.
pub fn audited_table<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> OperationRoutingTable {
    let recorder = Arc::new(RdsAudit::new(Arc::clone(engine)));
    let mut table = AuditedTable::new(recorder, QUERY_ENVELOPE);
    audited!(
        table, engine,
        add_role_to_db_cluster::AddRoleToDBCluster => mutates,
        add_role_to_db_instance::AddRoleToDBInstance => mutates,
        add_source_identifier_to_subscription::AddSourceIdentifierToSubscription => mutates,
        add_tags_to_resource::AddTagsToResource => mutates,
        apply_pending_maintenance_action::ApplyPendingMaintenanceAction => mutates,
        authorize_db_security_group_ingress::AuthorizeDBSecurityGroupIngress => mutates,
        backtrack_db_cluster::BacktrackDBCluster => mutates,
        cancel_db_capacity_reservation::CancelDBCapacityReservation => mutates,
        cancel_export_task::CancelExportTask => mutates,
        copy_db_cluster_parameter_group::CopyDBClusterParameterGroup => mutates,
        copy_db_cluster_snapshot::CopyDBClusterSnapshot => mutates,
        copy_db_parameter_group::CopyDBParameterGroup => mutates,
        copy_db_snapshot::CopyDBSnapshot => mutates,
        copy_option_group::CopyOptionGroup => mutates,
        create_blue_green_deployment::CreateBlueGreenDeployment => mutates,
        create_custom_db_engine_version::CreateCustomDBEngineVersion => mutates,
        create_db_capacity_reservation::CreateDBCapacityReservation => mutates,
        create_db_cluster::CreateDBCluster => mutates,
        create_db_cluster_endpoint::CreateDBClusterEndpoint => mutates,
        create_db_cluster_parameter_group::CreateDBClusterParameterGroup => mutates,
        create_db_cluster_snapshot::CreateDBClusterSnapshot => mutates,
        create_db_instance::CreateDBInstance => mutates,
        create_db_instance_read_replica::CreateDBInstanceReadReplica => mutates,
        create_db_parameter_group::CreateDBParameterGroup => mutates,
        create_db_proxy::CreateDBProxy => mutates,
        create_db_proxy_endpoint::CreateDBProxyEndpoint => mutates,
        create_db_security_group::CreateDBSecurityGroup => mutates,
        create_db_shard_group::CreateDBShardGroup => mutates,
        create_db_snapshot::CreateDBSnapshot => mutates,
        create_db_subnet_group::CreateDBSubnetGroup => mutates,
        create_event_subscription::CreateEventSubscription => mutates,
        create_global_cluster::CreateGlobalCluster => mutates,
        create_integration::CreateIntegration => mutates,
        create_option_group::CreateOptionGroup => mutates,
        create_tenant_database::CreateTenantDatabase => mutates,
        delete_blue_green_deployment::DeleteBlueGreenDeployment => mutates,
        delete_custom_db_engine_version::DeleteCustomDBEngineVersion => mutates,
        delete_db_cluster::DeleteDBCluster => mutates,
        delete_db_cluster_automated_backup::DeleteDBClusterAutomatedBackup => mutates,
        delete_db_cluster_endpoint::DeleteDBClusterEndpoint => mutates,
        delete_db_cluster_parameter_group::DeleteDBClusterParameterGroup => mutates,
        delete_db_cluster_snapshot::DeleteDBClusterSnapshot => mutates,
        delete_db_instance::DeleteDBInstance => mutates,
        delete_db_instance_automated_backup::DeleteDBInstanceAutomatedBackup => mutates,
        delete_db_parameter_group::DeleteDBParameterGroup => mutates,
        delete_db_proxy::DeleteDBProxy => mutates,
        delete_db_proxy_endpoint::DeleteDBProxyEndpoint => mutates,
        delete_db_security_group::DeleteDBSecurityGroup => mutates,
        delete_db_shard_group::DeleteDBShardGroup => mutates,
        delete_db_snapshot::DeleteDBSnapshot => mutates,
        delete_db_subnet_group::DeleteDBSubnetGroup => mutates,
        delete_event_subscription::DeleteEventSubscription => mutates,
        delete_global_cluster::DeleteGlobalCluster => mutates,
        delete_integration::DeleteIntegration => mutates,
        delete_option_group::DeleteOptionGroup => mutates,
        delete_tenant_database::DeleteTenantDatabase => mutates,
        deregister_db_proxy_targets::DeregisterDBProxyTargets => mutates,
        describe_account_attributes::DescribeAccountAttributes => reports,
        describe_blue_green_deployments::DescribeBlueGreenDeployments => reports,
        describe_certificates::DescribeCertificates => reports,
        describe_db_capacity_reservations::DescribeDBCapacityReservations => reports,
        describe_db_cluster_automated_backups::DescribeDBClusterAutomatedBackups => reports,
        describe_db_cluster_backtracks::DescribeDBClusterBacktracks => reports,
        describe_db_cluster_endpoints::DescribeDBClusterEndpoints => reports,
        describe_db_cluster_parameter_groups::DescribeDBClusterParameterGroups => reports,
        describe_db_cluster_parameters::DescribeDBClusterParameters => reports,
        describe_db_cluster_snapshot_attributes::DescribeDBClusterSnapshotAttributes => reports,
        describe_db_cluster_snapshots::DescribeDBClusterSnapshots => reports,
        describe_db_clusters::DescribeDBClusters => reports,
        describe_db_engine_versions::DescribeDBEngineVersions => reports,
        describe_db_instance_automated_backups::DescribeDBInstanceAutomatedBackups => reports,
        describe_db_instances::DescribeDBInstances => reports,
        describe_db_log_files::DescribeDBLogFiles => reports,
        describe_db_major_engine_versions::DescribeDBMajorEngineVersions => reports,
        describe_db_parameter_groups::DescribeDBParameterGroups => reports,
        describe_db_parameters::DescribeDBParameters => reports,
        describe_db_proxies::DescribeDBProxies => reports,
        describe_db_proxy_endpoints::DescribeDBProxyEndpoints => reports,
        describe_db_proxy_target_groups::DescribeDBProxyTargetGroups => reports,
        describe_db_proxy_targets::DescribeDBProxyTargets => reports,
        describe_db_recommendations::DescribeDBRecommendations => reports,
        describe_db_security_groups::DescribeDBSecurityGroups => reports,
        describe_db_shard_groups::DescribeDBShardGroups => reports,
        describe_db_snapshot_attributes::DescribeDBSnapshotAttributes => reports,
        describe_db_snapshot_tenant_databases::DescribeDBSnapshotTenantDatabases => reports,
        describe_db_snapshots::DescribeDBSnapshots => reports,
        describe_db_subnet_groups::DescribeDBSubnetGroups => reports,
        describe_engine_default_cluster_parameters::DescribeEngineDefaultClusterParameters => reports,
        describe_engine_default_parameters::DescribeEngineDefaultParameters => reports,
        describe_event_categories::DescribeEventCategories => reports,
        describe_event_subscriptions::DescribeEventSubscriptions => reports,
        describe_events::DescribeEvents => reports,
        describe_export_tasks::DescribeExportTasks => reports,
        describe_global_clusters::DescribeGlobalClusters => reports,
        describe_integrations::DescribeIntegrations => reports,
        describe_option_group_options::DescribeOptionGroupOptions => reports,
        describe_option_groups::DescribeOptionGroups => reports,
        describe_orderable_db_instance_options::DescribeOrderableDBInstanceOptions => reports,
        describe_pending_maintenance_actions::DescribePendingMaintenanceActions => reports,
        describe_reserved_db_instances::DescribeReservedDBInstances => reports,
        describe_reserved_db_instances_offerings::DescribeReservedDBInstancesOfferings => reports,
        describe_serverless_v2_platform_versions::DescribeServerlessV2PlatformVersions => reports,
        describe_source_regions::DescribeSourceRegions => reports,
        describe_tenant_databases::DescribeTenantDatabases => reports,
        describe_valid_db_instance_modifications::DescribeValidDBInstanceModifications => reports,
        disable_http_endpoint::DisableHttpEndpoint => mutates,
        download_db_log_file_portion::DownloadDBLogFilePortion => reports,
        enable_http_endpoint::EnableHttpEndpoint => mutates,
        failover_db_cluster::FailoverDBCluster => mutates,
        failover_global_cluster::FailoverGlobalCluster => mutates,
        list_tags_for_resource::ListTagsForResource => reports,
        modify_activity_stream::ModifyActivityStream => mutates,
        modify_certificates::ModifyCertificates => mutates,
        modify_current_db_cluster_capacity::ModifyCurrentDBClusterCapacity => mutates,
        modify_custom_db_engine_version::ModifyCustomDBEngineVersion => mutates,
        modify_db_capacity_reservation::ModifyDBCapacityReservation => mutates,
        modify_db_cluster::ModifyDBCluster => mutates,
        modify_db_cluster_endpoint::ModifyDBClusterEndpoint => mutates,
        modify_db_cluster_parameter_group::ModifyDBClusterParameterGroup => mutates,
        modify_db_cluster_snapshot_attribute::ModifyDBClusterSnapshotAttribute => mutates,
        modify_db_instance::ModifyDBInstance => mutates,
        modify_db_parameter_group::ModifyDBParameterGroup => mutates,
        modify_db_proxy::ModifyDBProxy => mutates,
        modify_db_proxy_endpoint::ModifyDBProxyEndpoint => mutates,
        modify_db_proxy_target_group::ModifyDBProxyTargetGroup => mutates,
        modify_db_recommendation::ModifyDBRecommendation => mutates,
        modify_db_shard_group::ModifyDBShardGroup => mutates,
        modify_db_snapshot::ModifyDBSnapshot => mutates,
        modify_db_snapshot_attribute::ModifyDBSnapshotAttribute => mutates,
        modify_db_subnet_group::ModifyDBSubnetGroup => mutates,
        modify_event_subscription::ModifyEventSubscription => mutates,
        modify_global_cluster::ModifyGlobalCluster => mutates,
        modify_integration::ModifyIntegration => mutates,
        modify_option_group::ModifyOptionGroup => mutates,
        modify_tenant_database::ModifyTenantDatabase => mutates,
        promote_read_replica::PromoteReadReplica => mutates,
        promote_read_replica_db_cluster::PromoteReadReplicaDBCluster => mutates,
        purchase_reserved_db_instances_offering::PurchaseReservedDBInstancesOffering => mutates,
        reboot_db_cluster::RebootDBCluster => mutates,
        reboot_db_instance::RebootDBInstance => mutates,
        reboot_db_shard_group::RebootDBShardGroup => mutates,
        register_db_proxy_targets::RegisterDBProxyTargets => mutates,
        remove_from_global_cluster::RemoveFromGlobalCluster => mutates,
        remove_role_from_db_cluster::RemoveRoleFromDBCluster => mutates,
        remove_role_from_db_instance::RemoveRoleFromDBInstance => mutates,
        remove_source_identifier_from_subscription::RemoveSourceIdentifierFromSubscription => mutates,
        remove_tags_from_resource::RemoveTagsFromResource => mutates,
        reset_db_cluster_parameter_group::ResetDBClusterParameterGroup => mutates,
        reset_db_parameter_group::ResetDBParameterGroup => mutates,
        restore_db_cluster_from_s3::RestoreDBClusterFromS3 => mutates,
        restore_db_cluster_from_snapshot::RestoreDBClusterFromSnapshot => mutates,
        restore_db_cluster_to_point_in_time::RestoreDBClusterToPointInTime => mutates,
        restore_db_instance_from_db_snapshot::RestoreDBInstanceFromDBSnapshot => mutates,
        restore_db_instance_from_s3::RestoreDBInstanceFromS3 => mutates,
        restore_db_instance_to_point_in_time::RestoreDBInstanceToPointInTime => mutates,
        revoke_db_security_group_ingress::RevokeDBSecurityGroupIngress => mutates,
        start_activity_stream::StartActivityStream => mutates,
        start_db_cluster::StartDBCluster => mutates,
        start_db_instance::StartDBInstance => mutates,
        start_db_instance_automated_backups_replication::StartDBInstanceAutomatedBackupsReplication => mutates,
        start_export_task::StartExportTask => mutates,
        stop_activity_stream::StopActivityStream => mutates,
        stop_db_cluster::StopDBCluster => mutates,
        stop_db_instance::StopDBInstance => mutates,
        stop_db_instance_automated_backups_replication::StopDBInstanceAutomatedBackupsReplication => mutates,
        switchover_blue_green_deployment::SwitchoverBlueGreenDeployment => mutates,
        switchover_global_cluster::SwitchoverGlobalCluster => mutates,
        switchover_read_replica::SwitchoverReadReplica => mutates,
    );
    table.finish()
}
