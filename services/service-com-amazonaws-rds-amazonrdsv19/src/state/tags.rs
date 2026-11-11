// SPDX-License-Identifier: CC0-1.0

//! Resolving an Amazon RDS resource ARN to the tag list stored on that resource.
//!
//! `AddTagsToResource`, `ListTagsForResource` and `RemoveTagsFromResource` all address
//! their resource the same way — by its ARN, through `ResourceName` ("This value is an
//! Amazon Resource Name (ARN)") — and every one of them has to reach the SAME stored tag
//! list the create that made the resource wrote, so a tag written through one operation is
//! read back by another and reported by the resource's own describe. That resolution lives
//! here once rather than three times.
//!
//! Each resource family keeps its tags on its own record (`DbInstanceRecord::tags`,
//! `DbClusterRecord::tags`, …) because that is where the create put them and where the
//! record's `to_wire` projection reads them from; this module adds no second copy.

use std::collections::BTreeMap;

use crate::ops::types::Tag;

use super::{ComAmazonawsRdsAmazonrdsv19State, RegionalKey, MAX_TAGS_PER_RESOURCE};

/// A family of taggable Amazon RDS resources: which store holds its records, and which
/// not-found fault names it.
///
/// One variant per resource type this service keeps records of, identified by the resource
/// TYPE field of the ARN — the form published under "Constructing an ARN for Amazon RDS",
/// and the same field each record's own `arn()` renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedKind {
    /// `db` — a DB instance.
    DbInstance,
    /// `cluster` — a DB cluster.
    DbCluster,
    /// `snapshot` — a DB (instance) snapshot.
    DbSnapshot,
    /// `cluster-snapshot` — a DB cluster snapshot.
    DbClusterSnapshot,
    /// `pg` — a DB parameter group.
    DbParameterGroup,
    /// `cluster-pg` — a DB cluster parameter group.
    DbClusterParameterGroup,
    /// `og` — an option group.
    OptionGroup,
    /// `subgrp` — a DB subnet group.
    DbSubnetGroup,
    /// `secgrp` — a DB security group.
    DbSecurityGroup,
    /// `es` — an event subscription.
    EventSubscription,
    /// `global-cluster` — an Aurora global database cluster.
    GlobalCluster,
    /// `shard-group` — a DB shard group.
    DbShardGroup,
    /// `blue-green-deployment` — a blue/green deployment.
    BlueGreenDeployment,
    /// `integration` — a zero-ETL integration.
    Integration,
    /// `db-proxy` — an RDS proxy.
    DbProxy,
    /// `tenant-database` — a tenant database of a multi-tenant DB instance.
    TenantDatabase,
    /// `cev` — a custom DB engine version.
    CustomDbEngineVersion,
}

impl TaggedKind {
    /// The family an ARN's resource TYPE field names, or `None` for a type this service
    /// keeps no records of.
    pub fn from_arn_resource_type(resource_type: &str) -> Option<Self> {
        Some(match resource_type {
            "db" => Self::DbInstance,
            "cluster" => Self::DbCluster,
            "snapshot" => Self::DbSnapshot,
            "cluster-snapshot" => Self::DbClusterSnapshot,
            "pg" => Self::DbParameterGroup,
            "cluster-pg" => Self::DbClusterParameterGroup,
            "og" => Self::OptionGroup,
            "subgrp" => Self::DbSubnetGroup,
            "secgrp" => Self::DbSecurityGroup,
            "es" => Self::EventSubscription,
            "global-cluster" => Self::GlobalCluster,
            "shard-group" => Self::DbShardGroup,
            "blue-green-deployment" => Self::BlueGreenDeployment,
            "integration" => Self::Integration,
            "db-proxy" => Self::DbProxy,
            "tenant-database" => Self::TenantDatabase,
            "cev" => Self::CustomDbEngineVersion,
            _ => return None,
        })
    }

    /// How this front door NAMES the family when it reports that no resource of it
    /// matched: the resource type as Amazon RDS writes it in prose, with the indefinite
    /// article the sentence reads with, so one sentence serves every family and none of
    /// them reads ungrammatically.
    ///
    /// It is the only part of [`not_found_message`] that varies by family, so the
    /// sentence itself is written once.
    pub fn resource_phrase(self) -> &'static str {
        match self {
            Self::DbInstance => "a DB Instance",
            Self::DbCluster => "a DB Cluster",
            Self::DbSnapshot => "a DB Snapshot",
            Self::DbClusterSnapshot => "a DB Cluster Snapshot",
            Self::DbParameterGroup => "a DB Parameter Group",
            Self::DbClusterParameterGroup => "a DB Cluster Parameter Group",
            Self::OptionGroup => "an Option Group",
            Self::DbSubnetGroup => "a DB Subnet Group",
            Self::DbSecurityGroup => "a DB Security Group",
            Self::EventSubscription => "an Event Subscription",
            Self::GlobalCluster => "a Global Cluster",
            Self::DbShardGroup => "a DB Shard Group",
            Self::BlueGreenDeployment => "a Blue Green Deployment",
            Self::Integration => "an Integration",
            Self::DbProxy => "a DB Proxy",
            Self::TenantDatabase => "a Tenant Database",
            Self::CustomDbEngineVersion => "a Custom Engine Version",
        }
    }
}

/// Every ARN resource type a tag operation of this service resolves, in the order
/// [`TaggedKind`] declares them: the published `rds` ARN resource types of the families
/// this store keeps records of. Reference data for a reader of this module — it is not put
/// in a client-visible message, which carries AWS's own wording instead.
pub const TAGGABLE_ARN_RESOURCE_TYPES: &str = "db, cluster, snapshot, cluster-snapshot, pg, \
                                               cluster-pg, og, subgrp, secgrp, es, \
                                               global-cluster, shard-group, \
                                               blue-green-deployment, integration, db-proxy, \
                                               tenant-database, cev";

/// The stored resource a `ResourceName` resolved to: which family it belongs to, and its
/// key within that family's store.
///
/// Holding the KEY rather than a borrow is what lets one resolution serve both a read and
/// a write of the same tag list without resolving twice.
#[derive(Debug, Clone)]
pub struct TagTarget {
    /// The family whose store holds the record.
    pub kind: TaggedKind,
    /// The record's key in that store.
    pub key: RegionalKey,
}

/// Why a `ResourceName` resolved to no stored tag list.
///
/// The two cases are different answers, which is why they are different variants: a value
/// that could never name a resource of this service is a bad parameter (HTTP 400), while a
/// well-formed ARN naming nothing stored is that resource family's own not-found fault
/// (HTTP 404) — and a client branches on the difference.
#[derive(Debug, Clone)]
pub enum TagTargetMiss {
    /// `ResourceName` is not an ARN of a resource type this service tags at all. Carries
    /// the message the request is refused with.
    Unusable(String),
    /// A well-formed ARN of a family this service stores, naming no record of it. Carries
    /// the family and the message its not-found fault reports.
    NotFound(TaggedKind, String),
}

/// The prefixes a tag key or value a CALLER writes may not carry: `Tag$Key` — "can't be
/// prefixed with `aws:` or `rds:`" — and `Tag$Value`, which documents the same two. They
/// are reserved for the tags AWS and Amazon RDS assign themselves, so a caller naming one
/// is refused rather than silently shadowing a service tag.
pub const RESERVED_TAG_PREFIXES: [&str; 2] = ["aws:", "rds:"];

/// Refuse a `Tags` list a caller asks this service to WRITE onto a resource: everything
/// [`super::check_tags`] refuses for a stored tag list, plus the two rules `Tag$Key`
/// documents for the key itself.
///
/// "A key is the required name of the tag. The string value can be from 1 to 128 Unicode
/// characters in length and can't be prefixed with `aws:` or `rds:`" — and a tag is
/// identified BY its key, which is why a tag carrying none is refused instead of stored:
/// nothing could ever address it again, and [`merge_tags`] could not tell two apart.
///
/// Only a caller's own tag write is held to this. A create's `Tags` is not, because the
/// tags a create carries include the ones another service of this emulator assigns to a
/// resource it provisions on the caller's behalf, which AWS itself assigns under exactly
/// these reserved prefixes.
pub fn check_tags_to_write(tags: &[Tag]) -> Result<(), String> {
    super::check_tags(tags)?;
    for tag in tags {
        let key = tag.key.as_deref().unwrap_or_default();
        if key.is_empty() {
            return Err("Tags.Key is the required name of a tag: it cannot be empty".to_string());
        }
        check_tag_prefix("Tags.Key", key)?;
        if let Some(value) = &tag.value {
            check_tag_prefix("Tags.Value", value)?;
        }
    }
    Ok(())
}

/// Refuse a tag key or value carrying one of [`RESERVED_TAG_PREFIXES`], naming the member
/// and the prefix it broke.
fn check_tag_prefix(member: &str, value: &str) -> Result<(), String> {
    for prefix in RESERVED_TAG_PREFIXES {
        if value.starts_with(prefix) {
            return Err(format!(
                "{member} '{value}' is prefixed with '{prefix}', which Amazon RDS reserves"
            ));
        }
    }
    Ok(())
}

/// Refuse a tag write that would leave one resource carrying more tags than Amazon RDS
/// allows ([`MAX_TAGS_PER_RESOURCE`]).
///
/// The per-request bound [`super::check_tags`] applies is not enough on its own: tags
/// accumulate across calls, so a resource tagged one call at a time would grow past the
/// quota — and past what this store is documented to hold — without this. `stored` is how
/// many tags the resource already carries and `added` how many of the request's keys are
/// NEW to it, since a key already present is replaced rather than added.
pub fn check_tag_capacity(stored: usize, added: usize) -> Result<(), String> {
    let total = stored + added;
    if total > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "the request would leave the resource carrying {total} tags: Amazon RDS allows at \
             most {MAX_TAGS_PER_RESOURCE} tags per resource"
        ));
    }
    Ok(())
}

/// Merge `added` into `tags`: a key already present keeps its position and takes the new
/// value, a key that is new is appended.
///
/// `AddTagsToResource` "adds metadata tags to an Amazon RDS resource", and a tag is a
/// key/value pair whose key identifies it, so re-tagging with a key the resource already
/// carries sets that key's value rather than storing the key twice. Order is preserved so
/// a resource's tag list reads the same way on every report of it.
pub fn merge_tags(tags: &mut Vec<Tag>, added: &[Tag]) {
    for tag in added {
        match tags
            .iter_mut()
            .find(|stored| stored.key.as_deref() == tag.key.as_deref())
        {
            Some(stored) => stored.value = tag.value.clone(),
            None => tags.push(tag.clone()),
        }
    }
}

/// How many of `added`'s keys the resource does not already carry — the number of tags a
/// merge would really ADD, which is what [`check_tag_capacity`] is asked about.
pub fn added_key_count(tags: &[Tag], added: &[Tag]) -> usize {
    let mut fresh: Vec<Option<&str>> = Vec::new();
    for tag in added {
        let key = tag.key.as_deref();
        let present = tags.iter().any(|stored| stored.key.as_deref() == key);
        if !present && !fresh.contains(&key) {
            fresh.push(key);
        }
    }
    fresh.len()
}

/// The key of the record in `map` whose ARN is `arn`, within `region`.
///
/// Reads ONE region's ordered range rather than the whole map — every key's first half is
/// the region, so one region's records are a contiguous span — and that span is bounded by
/// the store's own per-region quota, the same bound every per-request scan in this service
/// runs under.
fn key_in<R, F: Fn(&R) -> String>(
    map: &BTreeMap<RegionalKey, R>,
    region: &str,
    arn: &str,
    arn_of: F,
) -> Option<RegionalKey> {
    map.range((region.to_string(), String::new())..)
        .take_while(|(key, _)| key.0 == region)
        .find(|(_, record)| arn_of(record) == arn)
        .map(|(key, _)| key.clone())
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// The stored resource `resource_name` names, for a request signed for `region`.
    ///
    /// RDS is a regional service, so only the records of the request's own region are in
    /// scope — an ARN naming another region's resource resolves to nothing here, exactly
    /// as it would against the real service's regional endpoint. The one exception is a
    /// global cluster, an account-scoped resource whose ARN carries no region at all.
    ///
    /// Resolution compares the ARN each stored record RENDERS, so a `ResourceName` only
    /// ever resolves to a record whose own reports carry that same ARN.
    pub fn resolve_tag_target(
        &self,
        region: &str,
        resource_name: &str,
    ) -> Result<TagTarget, TagTargetMiss> {
        // Both refusals carry the wording Amazon RDS answers an unusable `ResourceName`
        // with: the ARN it could not resolve, and nothing about this emulator's own reach.
        // A resource type this service keeps no records of reads the same way, because
        // there is no resource of it to find.
        let Some(parsed) = super::parse_rds_resource_arn(resource_name) else {
            return Err(TagTargetMiss::Unusable(invalid_resource_name_message(
                resource_name,
            )));
        };
        let Some(kind) = TaggedKind::from_arn_resource_type(parsed.resource_type) else {
            return Err(TagTargetMiss::Unusable(invalid_resource_name_message(
                resource_name,
            )));
        };
        let found = match kind {
            TaggedKind::DbInstance => {
                key_in(&self.db_instances, region, resource_name, |record| {
                    record.arn()
                })
            }
            TaggedKind::DbCluster => key_in(&self.db_clusters, region, resource_name, |record| {
                record.arn()
            }),
            TaggedKind::DbSnapshot => {
                key_in(&self.db_snapshots, region, resource_name, |record| {
                    record.arn()
                })
            }
            TaggedKind::DbClusterSnapshot => {
                key_in(&self.db_cluster_snapshots, region, resource_name, |record| {
                    record.arn()
                })
            }
            TaggedKind::DbParameterGroup => {
                key_in(&self.db_parameter_groups, region, resource_name, |record| {
                    record.arn()
                })
            }
            TaggedKind::DbClusterParameterGroup => key_in(
                &self.db_cluster_parameter_groups,
                region,
                resource_name,
                |record| record.arn(),
            ),
            TaggedKind::OptionGroup => {
                key_in(&self.option_groups, region, resource_name, |record| {
                    record.arn()
                })
            }
            TaggedKind::DbSubnetGroup => {
                key_in(&self.db_subnet_groups, region, resource_name, |record| {
                    record.arn()
                })
            }
            TaggedKind::DbSecurityGroup => {
                key_in(&self.db_security_groups, region, resource_name, |record| {
                    record.arn()
                })
            }
            TaggedKind::EventSubscription => {
                key_in(&self.event_subscriptions, region, resource_name, |record| {
                    record.arn()
                })
            }
            // A global cluster is keyed by its identifier ALONE — it is account-scoped and
            // its ARN carries no region — so its key's region half is empty here, which is
            // what [`Self::tags_at`] reads it back with.
            TaggedKind::GlobalCluster => self
                .global_clusters
                .iter()
                .find(|(_, record)| record.arn() == resource_name)
                .map(|(key, _)| (String::new(), key.clone())),
            TaggedKind::DbShardGroup => {
                key_in(&self.db_shard_groups, region, resource_name, |record| {
                    record.arn()
                })
            }
            TaggedKind::BlueGreenDeployment => {
                key_in(&self.blue_green_deployments, region, resource_name, |record| {
                    record.arn()
                })
            }
            TaggedKind::Integration => {
                key_in(&self.integrations, region, resource_name, |record| {
                    record.arn()
                })
            }
            TaggedKind::DbProxy => key_in(&self.db_proxies, region, resource_name, |record| {
                record.arn()
            }),
            TaggedKind::TenantDatabase => {
                key_in(&self.tenant_databases, region, resource_name, |record| {
                    record.arn()
                })
            }
            TaggedKind::CustomDbEngineVersion => key_in(
                &self.custom_db_engine_versions,
                region,
                resource_name,
                |record| record.arn(),
            ),
        };

        match found {
            Some(key) => Ok(TagTarget { kind, key }),
            None => Err(TagTargetMiss::NotFound(
                kind,
                not_found_message(kind, resource_name),
            )),
        }
    }

    /// The tags the resolved resource carries.
    ///
    /// `None` only if the record went away between resolving and reading it, which cannot
    /// happen while the one state lock is held.
    pub fn tags_at(&self, target: &TagTarget) -> Option<&Vec<Tag>> {
        match target.kind {
            TaggedKind::DbInstance => self.db_instances.get(&target.key).map(|r| &r.tags),
            TaggedKind::DbCluster => self.db_clusters.get(&target.key).map(|r| &r.tags),
            TaggedKind::DbSnapshot => self.db_snapshots.get(&target.key).map(|r| &r.tags),
            TaggedKind::DbClusterSnapshot => {
                self.db_cluster_snapshots.get(&target.key).map(|r| &r.tags)
            }
            TaggedKind::DbParameterGroup => {
                self.db_parameter_groups.get(&target.key).map(|r| &r.tags)
            }
            TaggedKind::DbClusterParameterGroup => self
                .db_cluster_parameter_groups
                .get(&target.key)
                .map(|r| &r.tags),
            TaggedKind::OptionGroup => self.option_groups.get(&target.key).map(|r| &r.tags),
            TaggedKind::DbSubnetGroup => self.db_subnet_groups.get(&target.key).map(|r| &r.tags),
            TaggedKind::DbSecurityGroup => {
                self.db_security_groups.get(&target.key).map(|r| &r.tags)
            }
            TaggedKind::EventSubscription => {
                self.event_subscriptions.get(&target.key).map(|r| &r.tags)
            }
            TaggedKind::GlobalCluster => self.global_clusters.get(&target.key.1).map(|r| &r.tags),
            TaggedKind::DbShardGroup => self.db_shard_groups.get(&target.key).map(|r| &r.tags),
            TaggedKind::BlueGreenDeployment => {
                self.blue_green_deployments.get(&target.key).map(|r| &r.tags)
            }
            TaggedKind::Integration => self.integrations.get(&target.key).map(|r| &r.tags),
            TaggedKind::DbProxy => self.db_proxies.get(&target.key).map(|r| &r.tags),
            TaggedKind::TenantDatabase => self.tenant_databases.get(&target.key).map(|r| &r.tags),
            TaggedKind::CustomDbEngineVersion => self
                .custom_db_engine_versions
                .get(&target.key)
                .map(|r| &r.tags),
        }
    }

    /// The tags the resolved resource carries, for writing — the SAME list its create
    /// wrote and its describe reports, never a copy.
    pub fn tags_at_mut(&mut self, target: &TagTarget) -> Option<&mut Vec<Tag>> {
        match target.kind {
            TaggedKind::DbInstance => self.db_instances.get_mut(&target.key).map(|r| &mut r.tags),
            TaggedKind::DbCluster => self.db_clusters.get_mut(&target.key).map(|r| &mut r.tags),
            TaggedKind::DbSnapshot => self.db_snapshots.get_mut(&target.key).map(|r| &mut r.tags),
            TaggedKind::DbClusterSnapshot => self
                .db_cluster_snapshots
                .get_mut(&target.key)
                .map(|r| &mut r.tags),
            TaggedKind::DbParameterGroup => self
                .db_parameter_groups
                .get_mut(&target.key)
                .map(|r| &mut r.tags),
            TaggedKind::DbClusterParameterGroup => self
                .db_cluster_parameter_groups
                .get_mut(&target.key)
                .map(|r| &mut r.tags),
            TaggedKind::OptionGroup => self.option_groups.get_mut(&target.key).map(|r| &mut r.tags),
            TaggedKind::DbSubnetGroup => self
                .db_subnet_groups
                .get_mut(&target.key)
                .map(|r| &mut r.tags),
            TaggedKind::DbSecurityGroup => self
                .db_security_groups
                .get_mut(&target.key)
                .map(|r| &mut r.tags),
            TaggedKind::EventSubscription => self
                .event_subscriptions
                .get_mut(&target.key)
                .map(|r| &mut r.tags),
            TaggedKind::GlobalCluster => self
                .global_clusters
                .get_mut(&target.key.1)
                .map(|r| &mut r.tags),
            TaggedKind::DbShardGroup => self
                .db_shard_groups
                .get_mut(&target.key)
                .map(|r| &mut r.tags),
            TaggedKind::BlueGreenDeployment => self
                .blue_green_deployments
                .get_mut(&target.key)
                .map(|r| &mut r.tags),
            TaggedKind::Integration => self.integrations.get_mut(&target.key).map(|r| &mut r.tags),
            TaggedKind::DbProxy => self.db_proxies.get_mut(&target.key).map(|r| &mut r.tags),
            TaggedKind::TenantDatabase => self
                .tenant_databases
                .get_mut(&target.key)
                .map(|r| &mut r.tags),
            TaggedKind::CustomDbEngineVersion => self
                .custom_db_engine_versions
                .get_mut(&target.key)
                .map(|r| &mut r.tags),
        }
    }
}

/// What this front door answers a value that is no usable Amazon RDS resource ARN at all —
/// one that is no ARN, or an ARN naming a resource type this service keeps no records of.
///
/// The value itself is the problem, so the sentence names the value the request carried and
/// nothing about this emulator's own reach. One wording for every operation that addresses
/// its resource by ARN, so a tag write and a Data API endpoint toggle cannot drift in how
/// they word the same unusable value.
pub fn invalid_resource_name_message(resource_name: &str) -> String {
    format!("Invalid resource name: {resource_name}")
}

/// What a tag operation's not-found fault reports for a `ResourceName` that named no
/// stored record.
///
/// These operations address their resource BY `ResourceName` — "This value is an Amazon
/// Resource Name (ARN)" — so the fault names that value, the ARN the request carried,
/// rather than the identifier segment parsed out of it: the ARN is the only handle the
/// caller gave the resource here, and it is what a caller matching the refusal against
/// what it sent looks for. The family it looked for the resource in is named by its own
/// label ([`TaggedKind::resource_phrase`]), so one sentence serves every store this
/// resolver reaches — the operation that failed to find it is a tag operation whichever
/// family it was, which is why the wording is the tag family's own and not the resource
/// family's describe wording.
fn not_found_message(kind: TaggedKind, resource_name: &str) -> String {
    format!(
        "Could not find {} matching the resource name: {resource_name}",
        kind.resource_phrase()
    )
}

/// The refusal one of this service's tag operations answers a [`TagTargetMiss`] with, in
/// that operation's OWN error channel.
///
/// Each operation has its own generated error enum, so the mapping from a family to its
/// not-found fault cannot be a function over one type; it is this macro instead, expanded
/// once per handler. `$modeled` is the handler's `{Op}ErrorModeled` enum, which is why
/// every tag operation widens its error set to the same families through `extra_errors`.
#[macro_export]
macro_rules! tag_target_refusal {
    ($modeled:ident, $miss:expr) => {{
        use $crate::state::tags::{TagTargetMiss, TaggedKind};
        match $miss {
            TagTargetMiss::Unusable(message) => {
                $crate::scaffolded::Scaffolded::invalid_parameter_value(message)
            }
            TagTargetMiss::NotFound(kind, text) => {
                let message = Some(text);
                let modeled = match kind {
                    TaggedKind::DbInstance => $modeled::DBInstanceNotFoundFault(
                        $crate::ops::types::DBInstanceNotFoundFault { message },
                    ),
                    TaggedKind::DbCluster => $modeled::DBClusterNotFoundFault(
                        $crate::ops::types::DBClusterNotFoundFault { message },
                    ),
                    TaggedKind::DbSnapshot => $modeled::DBSnapshotNotFoundFault(
                        $crate::ops::types::DBSnapshotNotFoundFault { message },
                    ),
                    TaggedKind::DbClusterSnapshot => $modeled::DBClusterSnapshotNotFoundFault(
                        $crate::ops::types::DBClusterSnapshotNotFoundFault { message },
                    ),
                    TaggedKind::DbParameterGroup => $modeled::DBParameterGroupNotFoundFault(
                        $crate::ops::types::DBParameterGroupNotFoundFault { message },
                    ),
                    TaggedKind::DbClusterParameterGroup => {
                        $modeled::DBClusterParameterGroupNotFoundFault(
                            $crate::ops::types::DBClusterParameterGroupNotFoundFault { message },
                        )
                    }
                    TaggedKind::OptionGroup => $modeled::OptionGroupNotFoundFault(
                        $crate::ops::types::OptionGroupNotFoundFault { message },
                    ),
                    TaggedKind::DbSubnetGroup => $modeled::DBSubnetGroupNotFoundFault(
                        $crate::ops::types::DBSubnetGroupNotFoundFault { message },
                    ),
                    TaggedKind::DbSecurityGroup => $modeled::DBSecurityGroupNotFoundFault(
                        $crate::ops::types::DBSecurityGroupNotFoundFault { message },
                    ),
                    TaggedKind::EventSubscription => $modeled::SubscriptionNotFoundFault(
                        $crate::ops::types::SubscriptionNotFoundFault { message },
                    ),
                    TaggedKind::GlobalCluster => $modeled::GlobalClusterNotFoundFault(
                        $crate::ops::types::GlobalClusterNotFoundFault { message },
                    ),
                    TaggedKind::DbShardGroup => $modeled::DBShardGroupNotFoundFault(
                        $crate::ops::types::DBShardGroupNotFoundFault { message },
                    ),
                    TaggedKind::BlueGreenDeployment => $modeled::BlueGreenDeploymentNotFoundFault(
                        $crate::ops::types::BlueGreenDeploymentNotFoundFault { message },
                    ),
                    TaggedKind::Integration => $modeled::IntegrationNotFoundFault(
                        $crate::ops::types::IntegrationNotFoundFault { message },
                    ),
                    TaggedKind::DbProxy => $modeled::DBProxyNotFoundFault(
                        $crate::ops::types::DBProxyNotFoundFault { message },
                    ),
                    TaggedKind::TenantDatabase => $modeled::TenantDatabaseNotFoundFault(
                        $crate::ops::types::TenantDatabaseNotFoundFault { message },
                    ),
                    TaggedKind::CustomDbEngineVersion => {
                        $modeled::CustomDBEngineVersionNotFoundFault(
                            $crate::ops::types::CustomDBEngineVersionNotFoundFault { message },
                        )
                    }
                };
                modeled.into()
            }
        }
    }};
}
