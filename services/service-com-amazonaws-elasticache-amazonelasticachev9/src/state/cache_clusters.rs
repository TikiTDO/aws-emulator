// SPDX-License-Identifier: CC0-1.0

use super::*;

/// One log-delivery configuration of a cluster, as this emulator persists it.
///
/// `LogDeliveryConfigurationRequest` carries an `Enabled` flag; a disabled
/// request removes the configuration rather than storing one, so a stored record
/// is always an enabled one and needs no flag of its own.
#[derive(Debug, Clone)]
pub struct LogDeliveryRecord {
    pub log_type: Option<String>,
    pub destination_type: Option<String>,
    pub destination_details: Option<DestinationDetails>,
    pub log_format: Option<String>,
}

impl LogDeliveryRecord {
    /// This record as the modelled `LogDeliveryConfiguration`.
    pub fn to_modeled(&self) -> LogDeliveryConfiguration {
        LogDeliveryConfiguration {
            log_type: self.log_type.clone(),
            destination_type: self.destination_type.clone(),
            destination_details: self.destination_details.clone(),
            log_format: self.log_format.clone(),
            status: Some(LOG_DELIVERY_STATUS.to_string()),
            // `message` carries the reason a log delivery is in an error state,
            // and a stored configuration is settled.
            message: None,
        }
    }
}

/// Apply `requests` to the log-delivery configurations `stored`: a request
/// replaces the configuration for its log type, and `Enabled: false` removes it.
pub fn apply_log_delivery(
    stored: &mut Vec<LogDeliveryRecord>,
    requests: Vec<LogDeliveryConfigurationRequest>,
) {
    for request in requests {
        stored.retain(|existing| existing.log_type != request.log_type);
        if request.enabled == Some(false) {
            continue;
        }
        stored.push(LogDeliveryRecord {
            log_type: request.log_type,
            destination_type: request.destination_type,
            destination_details: request.destination_details,
            log_format: request.log_format,
        });
    }
}

/// One cache node of a cluster, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct CacheNodeRecord {
    /// A 4-digit numeric identifier (`0001`, `0002`, …), per
    /// `CacheNode$CacheNodeId`.
    pub cache_node_id: String,
    /// One of `CacheNode$CacheNodeStatus`'s values.
    pub status: String,
    pub create_time: SystemTime,
    pub endpoint_address: String,
    pub endpoint_port: i32,
    pub availability_zone: Option<String>,
    /// `CacheNode$ParameterGroupStatus`: [`PARAMETER_APPLY_STATUS`] while this
    /// node runs the current values of its cluster's parameter group,
    /// [`PARAMETER_APPLY_STATUS_PENDING_REBOOT`] once a parameter the engine
    /// applies only at a restart has been changed on that group. Held per NODE
    /// because a reboot of some of a cluster's nodes applies the group to those
    /// nodes alone, and the cluster's own status is read off its nodes (see
    /// [`CacheClusterRecord::parameter_apply_status`]).
    pub parameter_group_status: String,
}

impl CacheNodeRecord {
    /// This record as the modelled `CacheNode`.
    ///
    /// A node still being provisioned reports neither endpoint nor creation
    /// time: `DescribeCacheClusters` documents that "if cache nodes are
    /// currently being added to the cluster, node endpoint information and
    /// creation time for the additional nodes are not displayed until they are
    /// completely provisioned", and that a removed node's endpoint is not
    /// displayed either.
    pub fn to_modeled(&self) -> CacheNode {
        let provisioned = self.status == STATUS_AVAILABLE;
        CacheNode {
            cache_node_id: Some(self.cache_node_id.clone()),
            cache_node_status: Some(self.status.clone()),
            cache_node_create_time: provisioned
                .then(|| ::aws_smithy_types::DateTime::from(self.create_time)),
            endpoint: provisioned.then(|| Endpoint {
                address: Some(self.endpoint_address.clone()),
                port: Some(self.endpoint_port),
            }),
            parameter_group_status: Some(self.parameter_group_status.clone()),
            // The primary a read replica syncs from, as a cache NODE id within
            // the reporting cluster. A standalone cluster has no primary, and a
            // replication group's member cluster holds a single node whose
            // primary is a node of a DIFFERENT cluster, which this member cannot
            // name — the group's `NodeGroupMembers`/`CurrentRole` is where that
            // relationship is reported.
            source_cache_node_id: None,
            customer_availability_zone: self.availability_zone.clone(),
            // Outposts are not modelled by this slice.
            customer_outpost_arn: None,
        }
    }
}

/// The changes a `ModifyCacheCluster` deferred to the maintenance window: the
/// members `PendingModifiedValues` models, which are exactly the ones
/// `ApplyImmediately` governs. A member that shape does NOT model (the
/// maintenance window, the snapshot settings) has nowhere to be reported as
/// pending, so it is applied at once.
#[derive(Debug, Clone, Default)]
pub struct PendingValues {
    pub num_cache_nodes: Option<i32>,
    pub cache_node_type: Option<String>,
    pub engine_version: Option<String>,
    pub cache_node_ids_to_remove: Vec<String>,
    /// The Memcached scaling configuration. `PendingModifiedValues` is the ONLY
    /// place the model reports a `ScaleConfig`, so a requested one is pending
    /// whatever `ApplyImmediately` says — there is no applied member for it.
    pub scale_config: Option<ScaleConfig>,
}

impl PendingValues {
    /// True when nothing is pending.
    pub fn is_empty(&self) -> bool {
        self.num_cache_nodes.is_none()
            && self.cache_node_type.is_none()
            && self.engine_version.is_none()
            && self.cache_node_ids_to_remove.is_empty()
            && self.scale_config.is_none()
    }

    /// These values as the modelled shape, or `None` when nothing is pending —
    /// a cluster with no pending change reports no member rather than an empty
    /// structure.
    pub fn to_modeled(&self) -> Option<PendingModifiedValues> {
        if self.is_empty() {
            return None;
        }
        Some(PendingModifiedValues {
            num_cache_nodes: self.num_cache_nodes,
            cache_node_ids_to_remove: (!self.cache_node_ids_to_remove.is_empty())
                .then(|| self.cache_node_ids_to_remove.clone()),
            engine_version: self.engine_version.clone(),
            cache_node_type: self.cache_node_type.clone(),
            scale_config: self.scale_config.clone(),
            // An auth token change and a log-delivery change are applied by the
            // handler rather than deferred, so neither is ever pending on a
            // CLUSTER; a member cluster's in-transit encryption is the group's,
            // and `ModifyReplicationGroup` reports what it defers under the
            // GROUP's own pending values.
            auth_token_status: None,
            log_delivery_configurations: None,
            transit_encryption_enabled: None,
            transit_encryption_mode: None,
        })
    }
}

/// A status change the clock owes a resource: the instant it falls due, and the
/// status the resource carries from then on.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub status: String,
}

/// A cluster's materialised status change: what the clock did, and the published
/// ElastiCache event its completion raises.
///
/// The event is the one named by the call that BEGAN the transition — only that
/// call knows what the work was — so it travels on the record
/// ([`CacheClusterRecord::pending_event`]) and comes back out here, where the
/// store records the event and queues the notification the cluster's topic is
/// owed.
#[derive(Debug, Clone)]
pub struct SettledClusterTransition {
    /// The change the clock made.
    pub transition: SettledTransition,
    /// The published `ElastiCache:<Event>` name of the completion, or `None`
    /// where the published event list names no event for this work.
    pub event: Option<&'static str>,
}

/// One cache cluster, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct CacheClusterRecord {
    pub cache_cluster_id: String,
    pub region: String,
    pub engine: String,
    pub engine_version: String,
    pub cache_node_type: Option<String>,
    pub num_cache_nodes: i32,
    /// One of `CacheCluster$CacheClusterStatus`'s values.
    pub status: String,
    /// The status change the clock owes this cluster, if any.
    pub pending_status: Option<PendingTransition>,
    /// The published ElastiCache event the completion of that change raises, as
    /// the call that began it named it; see [`SettledClusterTransition`].
    pub pending_event: Option<&'static str>,
    pub created_at: SystemTime,
    /// The DNS label every endpoint of this cluster carries, minted once by
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::mint_dns_token`] and
    /// stored, so an endpoint is stable across reads.
    pub dns_token: String,
    pub endpoint_address: String,
    pub endpoint_port: i32,
    pub arn: String,
    /// The pool a NEW node's Availability Zone is taken from, in order: one zone
    /// for `single-az`, one per node for `cross-az` or an explicit
    /// `PreferredAvailabilityZones`. A node keeps the zone it was created in —
    /// `AZMode` documents that "existing Memcached nodes remain in their current
    /// Availability Zone. Only newly created nodes are located in different
    /// Availability Zones".
    pub availability_zones: Vec<String>,
    /// The `AZMode` the cluster was created or last modified with, which is what
    /// decides whether the next nodes added to it spread over zones.
    pub az_mode: Option<String>,
    pub preferred_maintenance_window: String,
    pub cache_subnet_group_name: Option<String>,
    /// The replication group this cluster belongs to: set on the member
    /// clusters `CreateReplicationGroup` mints, absent on a standalone cluster.
    /// The field is what
    /// `DescribeCacheClusters$ShowCacheClustersNotInReplicationGroups` filters
    /// on, so that filter reads stored state rather than a constant.
    pub replication_group_id: Option<String>,
    /// `CacheCluster$ReplicationGroupLogDeliveryEnabled` — whether log delivery
    /// is enabled for the replication group this cluster belongs to. `None` on a
    /// standalone cluster, which belongs to no group.
    pub replication_group_log_delivery_enabled: Option<bool>,
    pub cache_parameter_group_name: String,
    pub snapshot_retention_limit: Option<i32>,
    pub snapshot_window: Option<String>,
    pub auto_minor_version_upgrade: Option<bool>,
    pub security_group_ids: Vec<String>,
    /// The SNS topic ElastiCache publishes this cluster's events to.
    pub notification_topic_arn: Option<String>,
    /// That topic's `NotificationConfiguration$TopicStatus`:
    /// [`NOTIFICATION_STATUS_ACTIVE`] while ElastiCache publishes this cluster's
    /// events to it, [`NOTIFICATION_STATUS_INACTIVE`] once a modify has paused
    /// them. Held beside the ARN rather than derived from it, because
    /// "notifications are sent only if the status is active" pauses the
    /// publishing without detaching the topic — so a deactivated configuration
    /// still reports its `TopicArn` and a later modify re-activates the same
    /// one.
    pub notification_topic_status: String,
    /// Whether an `AuthToken` was set. The token itself is a SECRET and is NOT
    /// stored: the model reports only `AuthTokenEnabled` and
    /// `AuthTokenLastModifiedDate`, so those are the only two things kept.
    pub auth_token_enabled: bool,
    /// When the auth token was last set, reported as
    /// `AuthTokenLastModifiedDate`.
    pub auth_token_last_modified: Option<SystemTime>,
    pub transit_encryption_enabled: Option<bool>,
    /// The in-transit encryption MODE, which only a replication group sets, so
    /// it is absent on a standalone cluster.
    pub transit_encryption_mode: Option<String>,
    /// Whether data is encrypted at rest. `CreateCacheCluster` has no member for
    /// it, so it is absent on a standalone cluster and carries the group's
    /// setting on a member cluster.
    pub at_rest_encryption_enabled: Option<bool>,
    pub network_type: Option<String>,
    pub ip_discovery: Option<String>,
    pub log_delivery_configurations: Vec<LogDeliveryRecord>,
    pub nodes: Vec<CacheNodeRecord>,
    pub pending_modified_values: PendingValues,
    /// The tags `CreateCacheCluster` was given. The ONE copy of this cluster's
    /// tags: `ListTagsForResource` reports it and `AddTagsToResource` /
    /// `RemoveTagsFromResource` change it, through
    /// [`ComAmazonawsElasticacheAmazonelasticachev9State::resource_tags`].
    pub tags: Vec<(String, String)>,
}

impl CacheClusterRecord {
    /// The configuration endpoint address of this cluster.
    ///
    /// `ConfigurationEndpoint` documents the shape of the name — "The
    /// configuration endpoint will always have `.cfg` in it. Example:
    /// `mem-3.9dvc4r.cfg.usw2.cache.amazonaws.com:11211`" — so the emulator
    /// builds the cluster id, this cluster's stored DNS token, `cfg`, the region
    /// and the service's DNS suffix.
    pub(super) fn configuration_endpoint_address(cache_cluster_id: &str, dns_token: &str, region: &str) -> String {
        format!("{cache_cluster_id}.{dns_token}.cfg.{region}.cache.amazonaws.com")
    }

    /// The endpoint address of one node, which names the node rather than the
    /// cluster's configuration endpoint.
    pub(super) fn node_endpoint_address(&self, cache_node_id: &str) -> String {
        format!(
            "{}-{cache_node_id}.{}.{}.cache.amazonaws.com",
            self.cache_cluster_id, self.dns_token, self.region
        )
    }

    /// A node of this cluster with the 4-digit id `ordinal` names, in the status
    /// a just-created node has, placed in `zone`.
    pub(super) fn new_node(
        &self,
        ordinal: u32,
        now: SystemTime,
        status: &str,
        zone: Option<String>,
    ) -> CacheNodeRecord {
        let cache_node_id = format!("{ordinal:04}");
        CacheNodeRecord {
            endpoint_address: self.node_endpoint_address(&cache_node_id),
            cache_node_id,
            status: status.to_string(),
            create_time: now,
            endpoint_port: self.endpoint_port,
            availability_zone: zone,
            parameter_group_status: PARAMETER_APPLY_STATUS.to_string(),
        }
    }

    /// The Availability Zone the `index`-th node this cluster ever gained is
    /// placed in: the pool, in order, wrapping when there are more nodes than
    /// zones.
    pub(super) fn zone_for(&self, index: usize) -> Option<String> {
        if self.availability_zones.is_empty() {
            return None;
        }
        Some(self.availability_zones[index % self.availability_zones.len()].clone())
    }

    /// What `CacheCluster$PreferredAvailabilityZone` reports: the zone the
    /// cluster's nodes are in, or [`MULTIPLE_AVAILABILITY_ZONES`] when they are
    /// not all in one.
    pub fn preferred_availability_zone(&self) -> Option<String> {
        let mut zones = self
            .nodes
            .iter()
            .filter_map(|node| node.availability_zone.clone())
            .collect::<Vec<String>>();
        zones.sort();
        zones.dedup();
        match zones.len() {
            0 => self.availability_zones.first().cloned(),
            1 => zones.into_iter().next(),
            _ => Some(MULTIPLE_AVAILABILITY_ZONES.to_string()),
        }
    }

    /// Give this cluster `target` nodes, removing the ids in `ids_to_remove`
    /// first and then, if that is not enough, from the end; a growing cluster
    /// gets new nodes whose ids continue past the highest one in use.
    ///
    /// `new_zones`, when it is not empty, is where the ADDED nodes go, in order
    /// — `ModifyCacheCluster$NewAvailabilityZones` is "the list of Availability
    /// Zones where the new Memcached cache nodes are created". Otherwise the
    /// added nodes take the cluster's own zone pool.
    ///
    /// `target` is clamped to a count some engine permits (1 ..=
    /// [`MEMCACHED_MAX_NODES`]) before anything is allocated. Both callers
    /// already refuse a count outside what their engine permits
    /// ([`validate_num_cache_nodes`]) so the clamp changes no request the service
    /// accepts; it is what keeps ONE request from sizing the allocation this
    /// loop does, and it is the value stored, so the node list and
    /// `num_cache_nodes` cannot disagree.
    pub fn resize_nodes(
        &mut self,
        target: i32,
        ids_to_remove: &[String],
        now: SystemTime,
        status: &str,
        new_zones: &[String],
    ) {
        let target = target.clamp(MIN_CACHE_NODES_PER_CLUSTER, MAX_CACHE_NODES_PER_CLUSTER);
        if !ids_to_remove.is_empty() {
            self.nodes
                .retain(|node| !ids_to_remove.contains(&node.cache_node_id));
        }
        // The growth loop below allocates one record per node, so the node
        // count is bounded at the top of this function — where both the create
        // and the modify path go through it — rather than left to each caller's
        // own validation: no engine permits more than
        // [`MAX_CACHE_NODES_PER_CLUSTER`] nodes and no cluster has fewer than
        // [`MIN_CACHE_NODES_PER_CLUSTER`] (see [`validate_num_cache_nodes`]), so
        // a count outside that range cannot be a cluster this service serves and
        // is clamped instead of allocated.
        let target_len = usize::try_from(target).unwrap_or(1);
        self.nodes
            .reserve(target_len.saturating_sub(self.nodes.len()));
        while self.nodes.len() > target_len {
            self.nodes.pop();
        }
        let mut highest = self
            .nodes
            .iter()
            .filter_map(|node| node.cache_node_id.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        let mut added = 0usize;
        while self.nodes.len() < target_len {
            // `saturating_add`: the ordinal is bounded by the clamp above, so
            // this cannot wrap, and it must not panic in a debug build if a
            // future caller ever brings it close.
            highest = highest.saturating_add(1);
            let zone = if new_zones.is_empty() {
                self.zone_for(usize::try_from(highest).unwrap_or(1).saturating_sub(1))
            } else {
                Some(new_zones[added % new_zones.len()].clone())
            };
            let node = self.new_node(highest, now, status, zone);
            self.nodes.push(node);
            added += 1;
        }
        // The count stored is the clamped target, which is also the number of
        // nodes the loop above left in place, so the two cannot disagree.
        self.num_cache_nodes = target;
    }

    /// Move every node of this cluster to `status`.
    pub fn set_node_status(&mut self, status: &str) {
        for node in &mut self.nodes {
            node.status = status.to_string();
        }
    }

    /// Materialise the status change the clock owes this cluster, if `now` is
    /// past its due instant, and hand back the change it made.
    ///
    /// This is the ONE write a reporting operation may make: the transition was
    /// owed at this instant whether or not anybody called. Nothing else about
    /// the cluster moves. `None` means the clock owed nothing.
    pub fn settle(&mut self, now: SystemTime) -> Option<SettledClusterTransition> {
        let pending = self.pending_status.as_ref()?;
        if now < pending.due {
            return None;
        }
        let settled = SettledTransition {
            due: pending.due,
            from: self.status.clone(),
            to: pending.status.clone(),
        };
        self.status = pending.status.clone();
        self.pending_status = None;
        if self.status == STATUS_AVAILABLE {
            self.set_node_status(STATUS_AVAILABLE);
        }
        Some(SettledClusterTransition {
            transition: settled,
            event: self.pending_event.take(),
        })
    }

    /// Start the span after which the clock owes this cluster `available`,
    /// leaving it in `status` until then.
    ///
    /// `event` is the published ElastiCache event the COMPLETION of this work
    /// raises, which only the caller knows (a reboot's completion and a node
    /// addition's completion are different events of one transition shape), or
    /// `None` where the published event list names no event for it.
    pub fn begin_transition(&mut self, status: &str, now: SystemTime, event: Option<&'static str>) {
        self.status = status.to_string();
        self.pending_event = event;
        self.pending_status = Some(PendingTransition {
            due: now + CLUSTER_TRANSITION,
            status: STATUS_AVAILABLE.to_string(),
        });
    }

    /// Whether a describe reports this cluster's node list at all.
    ///
    /// `DescribeCacheClusters`: "If the cluster is in the creating state, only
    /// cluster-level information is displayed until all of the nodes are
    /// successfully provisioned. If the cluster is in the deleting state, only
    /// cluster-level information is displayed."
    pub fn reports_node_detail(&self) -> bool {
        self.status != STATUS_CREATING && self.status != STATUS_DELETING
    }

    /// Record that every node of this cluster is owed a reboot before it runs
    /// the values its cache parameter group holds: what
    /// `CacheParameterGroupStatus$ParameterApplyStatus` reports as
    /// `pending-reboot` and `CacheNodeIdsToReboot` names, until
    /// `RebootCacheCluster` restarts them.
    ///
    /// The one write of that deferral, so a parameter CHANGED on the group a
    /// cluster runs
    /// ([`ComAmazonawsElasticacheAmazonelasticachev9State::defer_parameter_change_to_reboot`])
    /// and a cluster MOVED onto another group leave the same state behind.
    pub fn defer_parameters_to_reboot(&mut self) {
        for node in &mut self.nodes {
            node.parameter_group_status = PARAMETER_APPLY_STATUS_PENDING_REBOOT.to_string();
        }
    }

    /// What `CacheParameterGroupStatus$ParameterApplyStatus` reports for this
    /// cluster's association with its parameter group: the group's values are
    /// applied to the cluster only once every node runs them, so a cluster with
    /// any node still owed a reboot is pending one; see
    /// [`CacheNodeRecord::parameter_group_status`].
    pub fn parameter_apply_status(&self) -> &'static str {
        if self.cache_node_ids_to_reboot().is_empty() {
            PARAMETER_APPLY_STATUS
        } else {
            PARAMETER_APPLY_STATUS_PENDING_REBOOT
        }
    }

    /// The nodes `CacheParameterGroupStatus$CacheNodeIdsToReboot` names: "a
    /// list of the cache node IDs which need to be rebooted for parameter
    /// changes to be applied", in the cluster's own node order.
    pub fn cache_node_ids_to_reboot(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|node| node.parameter_group_status == PARAMETER_APPLY_STATUS_PENDING_REBOOT)
            .map(|node| node.cache_node_id.clone())
            .collect()
    }

    /// This record as the modelled `CacheCluster`.
    ///
    /// `CacheNodes` is populated only when the caller asked for it
    /// (`DescribeCacheClusters$ShowCacheNodeInfo`) AND the cluster is past the
    /// states that report cluster-level information only; see
    /// [`CacheClusterRecord::reports_node_detail`]. `include_nodes` is `true`
    /// for the create / modify / delete responses, which carry the cluster's
    /// own node list.
    pub fn to_modeled(&self, include_nodes: bool) -> CacheCluster {
        CacheCluster {
            cache_cluster_id: Some(self.cache_cluster_id.clone()),
            configuration_endpoint: Some(Endpoint {
                address: Some(self.endpoint_address.clone()),
                port: Some(self.endpoint_port),
            }),
            // The client-library download page is a console URL, not state this
            // emulator holds.
            client_download_landing_page: None,
            cache_node_type: self.cache_node_type.clone(),
            engine: Some(self.engine.clone()),
            engine_version: Some(self.engine_version.clone()),
            cache_cluster_status: Some(self.status.clone()),
            num_cache_nodes: Some(self.num_cache_nodes),
            preferred_availability_zone: self.preferred_availability_zone(),
            // Outposts: `CreateCacheCluster` refuses every Outpost member,
            // because this emulator provisions no Outpost capacity to place a
            // cluster in.
            preferred_outpost_arn: None,
            cache_cluster_create_time: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            preferred_maintenance_window: Some(self.preferred_maintenance_window.clone()),
            pending_modified_values: self.pending_modified_values.to_modeled(),
            notification_configuration: self.notification_topic_arn.as_ref().map(|arn| {
                NotificationConfiguration {
                    topic_arn: Some(arn.clone()),
                    // The STORED status, not a constant: "notifications are sent
                    // only if the status is active", and a modify that pauses
                    // them leaves the topic configured and reports it
                    // `inactive`.
                    topic_status: Some(self.notification_topic_status.clone()),
                }
            }),
            // Cache security groups (the pre-VPC, EC2-Classic mechanism) cannot
            // exist here: the whole surface is closed for this account, see
            // [`CACHE_SECURITY_GROUPS_UNSUPPORTED`].
            cache_security_groups: None,
            cache_parameter_group: Some(CacheParameterGroupStatus {
                cache_parameter_group_name: Some(self.cache_parameter_group_name.clone()),
                parameter_apply_status: Some(self.parameter_apply_status().to_string()),
                // "A list of the cache node IDs which need to be rebooted for
                // parameter changes to be applied": nothing to name while the
                // nodes already run the group's current values.
                cache_node_ids_to_reboot: {
                    let ids = self.cache_node_ids_to_reboot();
                    (!ids.is_empty()).then_some(ids)
                },
            }),
            cache_subnet_group_name: self.cache_subnet_group_name.clone(),
            cache_nodes: (include_nodes && self.reports_node_detail())
                .then(|| self.nodes.iter().map(CacheNodeRecord::to_modeled).collect()),
            auto_minor_version_upgrade: self.auto_minor_version_upgrade,
            security_groups: (!self.security_group_ids.is_empty()).then(|| {
                self.security_group_ids
                    .iter()
                    .map(|id| SecurityGroupMembership {
                        security_group_id: Some(id.clone()),
                        status: Some(SECURITY_GROUP_STATUS.to_string()),
                    })
                    .collect()
            }),
            // The group this cluster is a member of, set when
            // `CreateReplicationGroup` minted it as one of the group's member
            // clusters.
            replication_group_id: self.replication_group_id.clone(),
            snapshot_retention_limit: self.snapshot_retention_limit,
            snapshot_window: self.snapshot_window.clone(),
            auth_token_enabled: Some(self.auth_token_enabled),
            auth_token_last_modified_date: self
                .auth_token_last_modified
                .map(::aws_smithy_types::DateTime::from),
            transit_encryption_enabled: self.transit_encryption_enabled,
            // At-rest encryption "cannot be modified after the cluster is
            // created" and `CreateCacheCluster` has no member to set it, so a
            // standalone cluster carries none; a replication group's member
            // cluster carries the group's setting.
            at_rest_encryption_enabled: self.at_rest_encryption_enabled,
            arn: Some(self.arn.clone()),
            // Log delivery "for the replication group": absent on a standalone
            // cluster, and on a member cluster whatever its group was created
            // with.
            replication_group_log_delivery_enabled: self.replication_group_log_delivery_enabled,
            log_delivery_configurations: (!self.log_delivery_configurations.is_empty()).then(|| {
                self.log_delivery_configurations
                    .iter()
                    .map(LogDeliveryRecord::to_modeled)
                    .collect()
            }),
            network_type: self.network_type.clone(),
            ip_discovery: self.ip_discovery.clone(),
            // The in-transit encryption MODE is set when a replication group is
            // created; `CreateCacheCluster` has no member for it, so a
            // standalone cluster carries none.
            transit_encryption_mode: self.transit_encryption_mode.clone(),
        }
    }
}
