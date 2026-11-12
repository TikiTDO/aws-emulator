// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of Amazon ElastiCache: the cache cluster an
//! `AWS::ElastiCache::CacheCluster` declares. One file of [`super`]'s per-type arms,
//! dispatched to by its `Provisioner`.
//!
//! The type's properties ARE `CreateCacheCluster`'s own members, spelled identically —
//! `Engine`, `CacheNodeType`, `NumCacheNodes`, `Port`, `CacheSubnetGroupName`,
//! `VpcSecurityGroupIds` and the rest — so the declaration is walked against that
//! operation's member catalogue ([`CREATE_CACHE_CLUSTER_MEMBERS`]) exactly as an
//! `AWS::RDS::DBInstance` is, with the two properties ElastiCache spells differently
//! renamed ([`CACHE_CLUSTER_RENAMED_PROPERTIES`]).
//!
//! "Ref returns the name of the cache cluster", which is the `ClusterName` the declaration
//! states (`super::template::declared_name_property`) and the `CacheClusterId` the cluster
//! is created under — the identifier every ElastiCache call on one takes — so the resource's
//! physical id needs no replacing by what the create answered.
//!
//! `CreateCacheCluster` answers the cluster `creating`, in which it serves nothing and
//! carries no endpoint, so the resource's creation is held open until ElastiCache reports it
//! `available` ([`cache_cluster_readiness`]) and the endpoint the type publishes is read off
//! it then ([`cache_cluster_ready_attributes`]).

use super::*;

/// Amazon ElastiCache's signing name and API version, from its model (`sigv4Name:
/// elasticache`, service version `2015-02-02`), which speaks `awsQuery` — so its calls ride
/// on [`call_query`], exactly as Amazon RDS's and Amazon Redshift's do.
pub(crate) const ELASTICACHE_SIGNING_NAME: &str = "elasticache";
pub(crate) const ELASTICACHE_API_VERSION: &str = "2015-02-02";

/// `AWS::ElastiCache::CacheCluster`: "Ref returns the name of the cache cluster", which is
/// the identifier `DescribeCacheClusters`, `ModifyCacheCluster` and `DeleteCacheCluster`
/// each address one by.
pub(crate) const ELASTICACHE_CACHE_CLUSTER: &str = "AWS::ElastiCache::CacheCluster";

/// The error code Amazon ElastiCache answers for a cache cluster it does not hold:
/// `CacheClusterNotFoundFault`'s own `awsQueryError` code, which is what an awsQuery client
/// branches on.
const CACHE_CLUSTER_NOT_FOUND: &str = "CacheClusterNotFound";

/// The `CacheCluster$CacheClusterStatus` a cluster is usable in, and the ones a NEW cluster
/// is still coming up out of (`smithy_query(query="doc", shape="CacheCluster",
/// member="CacheClusterStatus")`: `available`, `creating`, `deleted`, `deleting`,
/// `incompatible-network`, `modifying`, `rebooting cluster nodes`, `restore-failed`,
/// `snapshotting`). A cluster in any other status is in one no wait brings a new cluster out
/// of, which is a resource that failed to come up rather than one still coming.
const CACHE_CLUSTER_STATUS_AVAILABLE: &str = "available";
const CACHE_CLUSTER_STATUSES_COMING_UP: &[&str] = &["creating", "modifying"];

/// The members `CreateCacheCluster` takes, from Amazon ElastiCache's own model
/// (`smithy_query(query="operation", operation="CreateCacheCluster")`), with each list's
/// element name from its list shape's member `@xmlName`
/// (`smithy_query(query="shape", shape="SecurityGroupIdsList,PreferredAvailabilityZoneList,
/// CacheSecurityGroupNameList,SnapshotArnsList,TagList,LogDeliveryConfigurationRequestList")`).
///
/// This is the catalogue the declaration of an `AWS::ElastiCache::CacheCluster` is walked
/// against: the type's properties ARE these members, so a property whose name is one of them
/// (or renames to one, [`CACHE_CLUSTER_RENAMED_PROPERTIES`]) is passed through under it.
const CREATE_CACHE_CLUSTER_MEMBERS: &[(&str, QueryMember)] = &[
    ("AZMode", QueryMember::Scalar),
    ("AuthToken", QueryMember::Scalar),
    ("AutoMinorVersionUpgrade", QueryMember::Scalar),
    ("CacheClusterId", QueryMember::Scalar),
    ("CacheNodeType", QueryMember::Scalar),
    ("CacheParameterGroupName", QueryMember::Scalar),
    (
        "CacheSecurityGroupNames",
        QueryMember::List("CacheSecurityGroupName"),
    ),
    ("CacheSubnetGroupName", QueryMember::Scalar),
    ("Engine", QueryMember::Scalar),
    ("EngineVersion", QueryMember::Scalar),
    ("IpDiscovery", QueryMember::Scalar),
    (
        "LogDeliveryConfigurations",
        QueryMember::List("LogDeliveryConfigurationRequest"),
    ),
    ("NetworkType", QueryMember::Scalar),
    ("NotificationTopicArn", QueryMember::Scalar),
    ("NumCacheNodes", QueryMember::Scalar),
    ("Port", QueryMember::Scalar),
    ("PreferredAvailabilityZone", QueryMember::Scalar),
    (
        "PreferredAvailabilityZones",
        QueryMember::List("PreferredAvailabilityZone"),
    ),
    ("PreferredMaintenanceWindow", QueryMember::Scalar),
    ("ReplicationGroupId", QueryMember::Scalar),
    ("SecurityGroupIds", QueryMember::List("SecurityGroupId")),
    ("SnapshotArns", QueryMember::List("SnapshotArn")),
    ("SnapshotName", QueryMember::Scalar),
    ("SnapshotRetentionLimit", QueryMember::Scalar),
    ("SnapshotWindow", QueryMember::Scalar),
    ("Tags", QueryMember::List("Tag")),
    ("TransitEncryptionEnabled", QueryMember::Scalar),
];

/// The properties of `AWS::ElastiCache::CacheCluster` whose `CreateCacheCluster` member is
/// spelled differently: the type's `VpcSecurityGroupIds` is the API's `SecurityGroupIds`
/// ("one or more VPC security groups associated with the cluster"), and its `ClusterName` is
/// the `CacheClusterId` the cluster is created under. Every other property is its member's
/// own name.
const CACHE_CLUSTER_RENAMED_PROPERTIES: &[(&str, &str)] = &[
    ("VpcSecurityGroupIds", "SecurityGroupIds"),
    ("ClusterName", "CacheClusterId"),
];

/// The properties of the type the walk over the declaration does NOT turn into a
/// `CreateCacheCluster` member. Every other property the catalogue does not carry is refused
/// rather than dropped ([`query_members`]).
const CACHE_CLUSTER_PROPERTIES_NOT_SENT: &[&str] = &[
    // The identifier CloudFormation decides, which is this resource's physical id ("Ref
    // returns the name of the cache cluster"): the arm sets the member from that id, so the
    // walk must not also push the declared spelling of it.
    "ClusterName",
    // The tag list carries the declared tags AND the ones CloudFormation propagates to the
    // resource, so the arm builds it out of both ([`tags_of`]).
    "Tags",
];

/// Create the cache cluster an `AWS::ElastiCache::CacheCluster` declares, under the physical
/// id the naming rules settled — the `ClusterName` the declaration states, or the generated
/// name where it states none.
///
/// What the create ANSWERS replaces nothing of the identity: the cluster is created under
/// the identifier this resource already carries, which is what the type's `Ref` returns. Its
/// ARN is read off that answer, because it is ElastiCache's to mint; the endpoint is not —
/// `CreateCacheCluster` answers a cluster that is still `creating`, which has none yet, so
/// the endpoint attributes are read once that service reports the cluster up
/// ([`cache_cluster_ready_attributes`]).
pub(crate) async fn create_cache_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut parameters = query_members(
        &resource.properties,
        CREATE_CACHE_CLUSTER_MEMBERS,
        CACHE_CLUSTER_RENAMED_PROPERTIES,
        CACHE_CLUSTER_PROPERTIES_NOT_SENT,
        ELASTICACHE_CACHE_CLUSTER,
    )?;
    parameters.push((
        "CacheClusterId".to_string(),
        resource.physical_resource_id.clone(),
    ));
    push_tag_list(
        &mut parameters,
        "Tags",
        "Tag",
        &tags_of(&resource.properties, propagated),
    );
    let answered = call_query(
        ctx,
        ELASTICACHE_SIGNING_NAME,
        region,
        ELASTICACHE_API_VERSION,
        "CreateCacheCluster",
        &parameters,
    )
    .await?;

    let mut attributes = BTreeMap::new();
    if let Some(arn) = xml_element(&answered, "ARN").filter(|arn| !arn.is_empty()) {
        // The type publishes one ARN, so the conventional `Arn` attribute every type
        // answers is that same value ([`super::template::conventional_attributes`]).
        attributes.insert("Arn".to_string(), arn);
    }
    Ok(Created::attributes(attributes))
}

/// Whether Amazon ElastiCache reports the cache cluster a stack created UP.
///
/// "When the cluster is in the `creating` state, only cluster-level information is
/// displayed" and the cluster serves nothing: a client connects to the endpoint the cluster
/// has once it is `available`, so the resource's creation is held open until that service
/// reports it there. A status no new cluster comes out of — and a cluster ElastiCache no
/// longer holds — is a failure, because nothing is going to bring it up.
pub(crate) async fn cache_cluster_readiness(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Readiness, Refusal> {
    let Some(reported) = read_cache_cluster(ctx, region, physical_resource_id).await? else {
        return Ok(Readiness::Failed(format!(
            "Amazon ElastiCache holds no cache cluster named {physical_resource_id}"
        )));
    };
    let status = reported
        .get("CacheClusterStatus")
        .and_then(scalar_text)
        .unwrap_or_default();
    if status == CACHE_CLUSTER_STATUS_AVAILABLE {
        return Ok(Readiness::Ready);
    }
    if CACHE_CLUSTER_STATUSES_COMING_UP.contains(&status.as_str()) {
        return Ok(Readiness::Pending(status));
    }
    Ok(Readiness::Failed(format!(
        "Amazon ElastiCache reports the cache cluster {physical_resource_id} in status {status}"
    )))
}

/// The endpoint attributes the type publishes, read off the cluster once ElastiCache reports
/// it up: a cluster still `creating` has no endpoint, so the create's answer carries none.
///
/// The type publishes `ConfigurationEndpoint.Address` / `ConfigurationEndpoint.Port` (the
/// Memcached cluster's configuration endpoint) and `RedisEndpoint.Address` /
/// `RedisEndpoint.Port` (the single-node Valkey or Redis OSS cluster's), and ElastiCache
/// reports ONE endpoint for a cluster: `CacheCluster$ConfigurationEndpoint`, which is the
/// address a client of either engine connects to. So both pairs are reported off it, and a
/// template that reads the pair its engine publishes gets the address the cluster answers
/// on.
pub(crate) async fn cache_cluster_ready_attributes(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    let mut attributes = BTreeMap::new();
    let Some(reported) = read_cache_cluster(ctx, region, physical_resource_id).await? else {
        return Ok(attributes);
    };
    for member in ["Address", "Port"] {
        let Some(value) = reported
            .get("ConfigurationEndpoint")
            .and_then(|endpoint| endpoint.get(member))
            .and_then(scalar_text)
        else {
            continue;
        };
        attributes.insert(format!("ConfigurationEndpoint.{member}"), value.clone());
        attributes.insert(format!("RedisEndpoint.{member}"), value);
    }
    Ok(attributes)
}

/// Delete the cache cluster a stack created, addressed by its identifier — this resource's
/// physical id. A cluster Amazon ElastiCache no longer has (`CacheClusterNotFound`) is
/// already in the state the delete asks for.
///
/// `DeleteCacheCluster$FinalSnapshotIdentifier` is "the user-supplied name of a final
/// cluster snapshot", which is what a `Snapshot` deletion policy asks for
/// ([`super::provisioning::Policies`]); without one the call carries no snapshot member at
/// all, which is the form CloudFormation makes for the `Delete` policy.
pub(crate) async fn delete_cache_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
    final_snapshot: Option<&str>,
) -> Result<(), Refusal> {
    let mut parameters = vec![(
        "CacheClusterId".to_string(),
        physical_resource_id.to_string(),
    )];
    if let Some(snapshot_identifier) = final_snapshot {
        parameters.push((
            "FinalSnapshotIdentifier".to_string(),
            snapshot_identifier.to_string(),
        ));
    }
    match call_query(
        ctx,
        ELASTICACHE_SIGNING_NAME,
        region,
        ELASTICACHE_API_VERSION,
        "DeleteCacheCluster",
        &parameters,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == CACHE_CLUSTER_NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Whether Amazon ElastiCache has finished removing the cache cluster a stack deleted.
///
/// "The `DeleteCacheCluster` operation deletes a previously provisioned cluster" and leaves
/// it `deleting` until the deletion completes, so the cluster is still there when the delete
/// answers: the deletion is finished once that service no longer holds it, which is what
/// keeps a stack from deleting the subnet group the cluster still sits in.
pub(crate) async fn cache_cluster_removal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Removal, Refusal> {
    match read_cache_cluster(ctx, region, physical_resource_id).await? {
        Some(reported) => Ok(Removal::Pending(
            reported
                .get("CacheClusterStatus")
                .and_then(scalar_text)
                .unwrap_or_default(),
        )),
        None => Ok(Removal::Gone),
    }
}

/// The cluster's configuration as Amazon ElastiCache reports it, for the readiness and
/// removal reads above and for drift detection. A cluster it no longer holds
/// (`CacheClusterNotFound`) was deleted.
///
/// A cluster is reported as one `<CacheCluster>` of the answer's `CacheClusters` list (the
/// element name that list's member `@xmlName` fixes — `smithy_query(query="shape",
/// shape="CacheClusterList")`), whose SCALAR members are read out by name;
/// `ConfigurationEndpoint` is a structure, so it is read out of the same document on its own
/// ([`xml_scalar_children`] leaves a composite member out).
pub(crate) async fn read_cache_cluster(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let parameters = vec![(
        "CacheClusterId".to_string(),
        physical_resource_id.to_string(),
    )];
    let answered = match call_query(
        ctx,
        ELASTICACHE_SIGNING_NAME,
        region,
        ELASTICACHE_API_VERSION,
        "DescribeCacheClusters",
        &parameters,
    )
    .await
    {
        Ok(answered) => answered,
        Err(refusal) if refusal.code == CACHE_CLUSTER_NOT_FOUND => return Ok(None),
        Err(refusal) => return Err(refusal),
    };
    let mut reported = xml_scalar_children(&answered, "CacheCluster");
    if reported.is_empty() {
        return Ok(None);
    }
    let endpoint = xml_scalar_children(&answered, "ConfigurationEndpoint");
    if !endpoint.is_empty() {
        reported.insert(
            "ConfigurationEndpoint".to_string(),
            Value::Object(endpoint),
        );
    }
    Ok(Some(Value::Object(reported)))
}
