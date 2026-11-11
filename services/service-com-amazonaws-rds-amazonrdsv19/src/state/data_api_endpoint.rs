// SPDX-License-Identifier: CC0-1.0

//! The RDS Data API HTTP endpoint a DB cluster carries, and the one write both toggles of
//! it perform.
//!
//! `EnableHttpEndpoint` and `DisableHttpEndpoint` are the same operation with two values:
//! each addresses a DB cluster by `ResourceArn` ("The Amazon Resource Name (ARN) of the DB
//! cluster"), writes `DBCluster$HttpEndpointEnabled` on the stored record and reports that
//! flag back beside the cluster's ARN. The resolution, the preconditions and the refusal
//! wording therefore live here once, so the two handlers cannot drift from each other —
//! and so the flag a toggle writes is the SAME field `CreateDBCluster` initialises,
//! `ModifyDBCluster$EnableHttpEndpoint` changes and `DescribeDBClusters` reports, which is
//! what the service serving the Data API reads the cluster's endpoint state from.

use super::*;

/// Why a Data API endpoint toggle was refused, as the class of refusal rather than as one
/// operation's error enum: each handler lifts these into its own modelled errors, which are
/// the same two for both operations (`ResourceNotFoundFault`, `InvalidResourceStateFault` —
/// `smithy_query(query="operation", operation="EnableHttpEndpoint")`).
#[derive(Debug, Clone)]
pub enum DataApiEndpointRefusal {
    /// `ResourceArn` carries no Amazon RDS DB cluster ARN at all — a value that is no ARN,
    /// or one naming another resource type. The value itself is unusable, so this is
    /// refused on the MEMBER that carried it rather than reported as a resource that might
    /// yet appear.
    UnusableArn(String),
    /// A DB cluster ARN that names no cluster this account holds in the request's region.
    NotFound(String),
    /// The cluster is there, but is not one this operation can be performed on: either it
    /// has no Data API HTTP endpoint to toggle, or it is busy with another operation.
    InvalidState(String),
}

/// The sentence a toggle refuses a cluster whose type has no Data API HTTP endpoint with.
///
/// It is the operation's OWN published precondition — "This operation applies only to
/// Aurora Serverless v2 and provisioned DB clusters" — and has no variable part: the
/// refusal states which clusters the operation serves, which is what tells a caller that no
/// amount of waiting will change the answer and that an Aurora Serverless v1 cluster's
/// endpoint is toggled through `ModifyDBCluster$EnableHttpEndpoint` instead.
pub const DATA_API_ENDPOINT_CLUSTER_TYPE_MESSAGE: &str =
    "This operation applies only to Aurora Serverless v2 and provisioned DB clusters.";

/// Whether a cluster running `engine` in `engine_mode` has an HTTP endpoint these two
/// operations toggle.
///
/// The RDS Data API serves an AURORA cluster's HTTP endpoint, so no other engine has one
/// ([`is_aurora`]); and of the Aurora clusters, the operation "applies only to Aurora
/// Serverless v2 and provisioned DB clusters", which are exactly the clusters in the
/// [`DEFAULT_DB_CLUSTER_ENGINE_MODE`] engine mode — `EngineMode`'s other value
/// (`serverless`) "only applies for Aurora Serverless v1 DB clusters", whose endpoint
/// `ModifyDBCluster` owns.
pub fn cluster_has_toggleable_http_endpoint(engine: &str, engine_mode: &str) -> bool {
    is_aurora(engine) && engine_mode.eq_ignore_ascii_case(DEFAULT_DB_CLUSTER_ENGINE_MODE)
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// Write the Data API HTTP endpoint flag of the cluster `resource_arn` names in
    /// `region`, and answer the cluster's own ARN with the flag as it now stands.
    ///
    /// Idempotent in the value: a toggle that asks for the setting the cluster already has
    /// is taken and reports it, because `HttpEndpointEnabled` is a setting rather than a
    /// transition. The write needs no restart of the cluster's instances — nothing
    /// connects THROUGH the Data API endpoint — so it is applied at once and leaves the
    /// cluster's `Status` where it was, the way every other no-downtime modification of a
    /// cluster does.
    ///
    /// Resolution is by the ARN each stored cluster RENDERS ([`DbClusterRecord::arn`]), so
    /// an ARN of another account, partition or region resolves to nothing here: RDS is
    /// regional, and the only cluster this request can act on is one whose own reports
    /// carry the ARN it named.
    pub fn set_cluster_http_endpoint(
        &mut self,
        region: &str,
        resource_arn: &str,
        enabled: bool,
    ) -> Result<(String, bool), DataApiEndpointRefusal> {
        let unusable = || {
            DataApiEndpointRefusal::UnusableArn(tags::invalid_resource_name_message(resource_arn))
        };
        let Some(parsed) = parse_rds_resource_arn(resource_arn) else {
            return Err(unusable());
        };
        if tags::TaggedKind::from_arn_resource_type(parsed.resource_type)
            != Some(tags::TaggedKind::DbCluster)
        {
            return Err(unusable());
        }

        // The stored key of the cluster the ARN names, folded the way every DB cluster key
        // is ("stored as a lowercase string").
        let key = (region.to_string(), parsed.identifier.to_ascii_lowercase());
        let not_found = || {
            DataApiEndpointRefusal::NotFound(db_cluster_not_found_colon_message(resource_arn))
        };
        let Some(record) = self.db_clusters.get_mut(&key) else {
            return Err(not_found());
        };
        // The ARN the cluster itself renders, which is what the request has to have named.
        let arn = record.arn();
        if arn != resource_arn {
            return Err(not_found());
        }
        if !cluster_has_toggleable_http_endpoint(&record.engine, &record.engine_mode) {
            return Err(DataApiEndpointRefusal::InvalidState(
                DATA_API_ENDPOINT_CLUSTER_TYPE_MESSAGE.to_string(),
            ));
        }
        // "The operation can't be performed because another operation is in progress": a
        // toggle changes the cluster's configuration, so it needs the same available
        // cluster every other modification of one does, and it is refused with the sentence
        // RDS words for a modification a cluster's status is not in a state for.
        if record.status != DB_CLUSTER_STATUS_AVAILABLE {
            return Err(DataApiEndpointRefusal::InvalidState(
                db_cluster_not_available_for_modification_message(&record.status),
            ));
        }

        record.http_endpoint_enabled = enabled;
        Ok((arn, record.http_endpoint_enabled))
    }
}
