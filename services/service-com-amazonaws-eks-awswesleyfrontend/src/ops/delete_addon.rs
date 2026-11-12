// SPDX-License-Identifier: CC0-1.0

//! @op DeleteAddon
//!
//! IMPLEMENTED. `DeleteAddon` MUTATES: it starts taking the
//! [`crate::state::AddonRecord`] `CreateAddon` wrote off its cluster
//! ([`crate::state::ComAmazonawsEksAwswesleyfrontendState::start_addon_removal`]) and
//! answers with that record in the `DELETING` status the removal runs in.
//!
//! The removal is ASYNCHRONOUS, like the install: the add-on reports `DELETING` from
//! this call until the clock has finished it
//! ([`crate::state::ADDON_DELETION_DELAY`]), so `DescribeAddon` and `ListAddons`
//! still find it — in `DELETING` — and installing the same add-on again collides
//! with the one on its way out until then. Only after that is the add-on no longer an
//! add-on of its cluster.
//!
//! The layers are the same as `DescribeAddon`'s, outside in: the cluster named by the
//! `@httpLabel` must exist in the request's region, that cluster must have an Amazon
//! EKS-managed plane for an add-on to live in, and then the add-on must be installed
//! on it — a missing resource is the modelled `ResourceNotFoundException` (404).
//!
//! `preserve` decides what happens to what Amazon EKS manages FOR the add-on: the EKS
//! Pod Identity associations the add-on owns go with it, unless the caller asked for
//! the add-on software to be preserved with Amazon EKS no longer managing its settings.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteAddon`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Deletes an Amazon EKS add-on. When you remove an add-on, it's deleted from the cluster.
//! You can always manually start an add-on on the cluster using the Kubernetes API.
//!
//! ## Input
//!
//! * `addonName` (String) **required** — The name of the add-on. The name must match one of
//!   the names returned by ListAddons .
//! * `clusterName` (ClusterName, len 1..100, pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$)
//!   **required** — The name of your cluster.
//! * `preserve` (Boolean, default false) **required** — Specifying this option preserves the
//!   add-on software on your cluster but Amazon EKS stops managing any settings for the
//!   add-on. If an IAM account is associated with the add-on, it isn't removed.
//!
//! ## Output
//!
//! * `addon` (Addon) — An Amazon EKS add-on. For more information, see Amazon EKS add-ons in
//!   the Amazon EKS User Guide.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — The request is invalid given the state of the
//!   cluster. Check the state of the cluster and the associated operations.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteAddonError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteAddonError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#DeleteAddon")]
#[derive(Debug)]
pub enum DeleteAddonErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_addon::DeleteAddonError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteAddonError = crate::scaffolded::Scaffolded<DeleteAddonErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteAddonErrorModeled> for DeleteAddonError {
    fn from(modeled: DeleteAddonErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteAddon` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteAddon<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteAddon<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteAddon<Eng>
{
    type Input = DeleteAddonInput;
    type Output = DeleteAddonOutput;
    type Error = DeleteAddonError;

    const NAME: &'static str = "DeleteAddon";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_ADDON_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_ADDON_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteAddonError::from(DeleteAddonErrorModeled::ServerException(ServerException {
                cluster_name: Some(input.cluster_name.clone()),
                addon_name: Some(input.addon_name.clone()),
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        // This delete addresses ONE resource — the `(cluster, add-on)` pair — so a miss
        // of either half is the service's generic, memberless not-found payload: the
        // same one `DescribeAddon` answers a miss of that pair with
        // ([`crate::state::resource_not_found`]).
        let not_found = || {
            DeleteAddonError::from(DeleteAddonErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(),
            ))
        };

        // THE REQUEST'S OWN ADDRESSING FIRST, before the store is even locked:
        // `clusterName` arrives in the path and every cluster name of this API obeys
        // one grammar and one length maximum (`ClusterName` is `@length(1..100)` +
        // `@pattern`, [`crate::state::is_cluster_name`]), so a label outside it names
        // no cluster in ANY Region — it is the invalid parameter it is, in the sentence
        // Amazon EKS states for that parameter
        // ([`crate::state::cluster_name_refusal`]), rather than a not-found that would
        // invite the caller to create a cluster under a name Amazon EKS will never
        // accept. The framework's validating walk does not judge a value bound to an
        // `@httpLabel`, so this is where it is judged.
        if !crate::state::is_cluster_name(&input.cluster_name) {
            return Err(DeleteAddonErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(crate::state::cluster_name_refusal(
                        "clusterName",
                        &input.cluster_name,
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.materialize_due_transitions(now);

        // An add-on is addressed through its cluster, and a cluster name identifies a
        // cluster only within one region, so this deletes the add-on of the cluster in
        // the region the request is signed for.
        let Some(cluster) = guard
            .clusters
            .get(&(region.clone(), input.cluster_name.clone()))
        else {
            return Err(not_found());
        };
        // An add-on is software Amazon EKS manages inside the cluster, and a cluster
        // `RegisterCluster` connected has no EKS-managed plane for one to live in: the
        // operation does not apply to it. The gate is a fact about the CLUSTER, so it
        // precedes resolving the add-on.
        if cluster.is_connected() {
            return Err(DeleteAddonErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        let key = (region, input.cluster_name.clone(), input.addon_name.clone());
        // The associations this add-on owns are read BEFORE the removal: the answer
        // is "the full description of the add-on that was deleted", so it reports
        // the set the add-on owned as it left, not the empty set the removal leaves
        // behind.
        let live = guard
            .addons
            .get(&key)
            .map(|record| guard.association_arns_owned_by(&record.arn))
            .unwrap_or_default();
        let Some(record) = guard.start_addon_removal(&key, input.preserve, now) else {
            return Err(not_found());
        };

        Ok(DeleteAddonOutput {
            addon: Some(record.to_addon(&live)),
        })
    }
}
