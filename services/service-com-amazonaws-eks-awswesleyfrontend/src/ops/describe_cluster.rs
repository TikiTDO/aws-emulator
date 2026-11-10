// SPDX-License-Identifier: CC0-1.0

//! @op DescribeCluster
//! IMPLEMENTED. A pure read of the [`crate::state::ClusterRecord`] `CreateCluster`
//! wrote: the `Cluster` this answers with carries the stored `arn`, `createdAt`,
//! `roleArn`, `resourcesVpcConfig`, `version` and tags, not a recomputed copy.
//!
//! The one write it makes is materialising a status move the CLOCK already owed —
//! `CREATING` to `ACTIVE` once [`crate::state::CLUSTER_ACTIVATION_DELAY`] has passed
//! since the create. That move was due whether or not anybody called, so it does not
//! change what a later caller sees.
//!
//! The name it is addressed by is judged BEFORE that read: a value outside the
//! cluster-name grammar ([`crate::state::is_cluster_name`]) names no cluster that
//! could exist, so it is refused as the invalid identifier it is rather than reported
//! as a cluster this Region does not hold.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeCluster`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Describes an Amazon EKS cluster. The API server endpoint and certificate authority data
//! returned by this operation are required for kubelet and kubectl to communicate with your
//! Kubernetes API server. For more information, see Creating or updating a kubeconfig file
//! for an Amazon EKS cluster. The API server endpoint and certificate authority data aren't
//! available until the cluster reaches the ACTIVE state.
//!
//! ## Input
//!
//! * `name` (String) **required** — The name of your cluster.
//!
//! ## Output
//!
//! * `cluster` (Cluster) — The full description of your specified cluster.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

#[allow(unused_imports)]
use super::types::*;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeClusterError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeClusterError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `InvalidParameterException` — a shape this SERVICE models, and nearly every
// operation that takes input declares, which this one's `errors` list omits — is
// widened in through `extra_errors`: Amazon EKS judges the cluster-name grammar of the
// name this read is addressed by, and the recorded refusal of a value outside it is
// that shape, in the sentence this service states for a cluster-name member.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#DescribeCluster",
    extra_errors = ["InvalidParameterException"]
)]
#[derive(Debug)]
pub enum DescribeClusterErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_cluster::DescribeClusterError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeClusterError = crate::scaffolded::Scaffolded<DescribeClusterErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeClusterErrorModeled> for DescribeClusterError {
    fn from(modeled: DescribeClusterErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeCluster` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeCluster<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeCluster<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeCluster<Eng>
{
    type Input = DescribeClusterInput;
    type Output = DescribeClusterOutput;
    type Error = DescribeClusterError;

    const NAME: &'static str = "DescribeCluster";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CLUSTER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_CLUSTER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DescribeClusterError::from(DescribeClusterErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // THE NAME'S FORM FIRST, before anything is looked up: `name` is modelled as a
        // plain `String` on this operation (`smithy_query(query="operation",
        // operation="DescribeCluster")` reports `name` -> `String`, `httpLabel`,
        // required), but every cluster of this API carries a name inside the
        // cluster-name grammar ([`crate::state::is_cluster_name`]), so a value outside
        // it names no cluster that could exist in any Region rather than one this
        // Region does not hold. Amazon EKS enforces the grammar here and refuses the
        // value as the invalid parameter it is — 400 `InvalidParameterException`, the
        // shape this service states a bad parameter with everywhere, widened into this
        // operation's channel above — in the sentence it states for a cluster-name
        // member ([`crate::state::cluster_name_refusal`]), naming the cluster the
        // request asked about. A 404 here would tell a caller that passed a cluster ARN
        // where a name belongs that its cluster does not exist.
        if !crate::state::is_cluster_name(&input.name) {
            return Err(DescribeClusterErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(crate::state::cluster_name_refusal("name", &input.name)),
                    ..Default::default()
                },
            )
            .into());
        }

        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        // A cluster name identifies a cluster only within one region, so the record
        // this reads is the one in the region the request is signed for.
        match guard.clusters.get(&(region, input.name.clone())) {
            Some(record) => Ok(DescribeClusterOutput {
                cluster: Some(record.to_cluster()),
            }),
            None => Err(DescribeClusterErrorModeled::ResourceNotFoundException(
                crate::state::cluster_not_found(&input.name),
            )
            .into()),
        }
    }
}
