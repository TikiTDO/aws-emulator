// SPDX-License-Identifier: CC0-1.0

//! @op ListUpdates
//! IMPLEMENTED. Reports the ids of the asynchronous updates the store holds for the
//! resource the request names: the cluster's own control plane, or — when
//! `nodegroupName`, `addonName` or `capabilityName` narrows it — one resource inside
//! that cluster. The updates live on the cluster record
//! ([`crate::state::ClusterRecord::updates`]), each carrying the
//! [`crate::state::UpdateScope`] of the resource it is an update OF, so this listing
//! and `DescribeUpdate` report one set of records rather than two copies.
//!
//! Paging follows `ListCapabilities`: a page holds at most `maxResults` ids
//! ([`crate::state::DEFAULT_LIST_PAGE_SIZE`] when the caller gives none, the whole page
//! the model's 1..100 bound allows), and `nextToken` is returned only when something is
//! left — it names the update id to resume AFTER, so it stays valid across new updates,
//! and a token this listing did not mint (including one another listing issued, which
//! is what the [`crate::state::UPDATES_LISTING`] token scope rules out) is an
//! `InvalidParameterException` rather than an empty page.
//!
//! A cluster the store does not hold for the request's region is a
//! `ResourceNotFoundException` naming it, and so is a node group, add-on or capability
//! the named cluster does not hold — "You can view your available clusters with
//! ListClusters. You can view your available managed node groups with ListNodegroups"
//! (`ResourceNotFoundException`) — never an empty page, which would claim a resource
//! that does not exist has no updates. A cluster `RegisterCluster` connected has no
//! Amazon EKS-managed control plane to update, so it is refused like every other
//! operation of that plane.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListUpdates`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Lists the updates associated with an Amazon EKS resource in your Amazon Web Services
//! account, in the specified Amazon Web Services Region.
//!
//! ## Input
//!
//! * `addonName` (String) — The names of the installed add-ons that have available updates.
//! * `capabilityName` (String) — The name of the capability for which you want to list
//!   updates.
//! * `maxResults` (ListUpdatesRequestMaxResults, range 1..100) — The maximum number of
//!   results, returned in paginated output. You receive maxResults in a single page, along
//!   with a nextToken response element. You can see the remaining results of the initial
//!   request by sending another request with the returned nextToken value. This value can be
//!   between 1 and 100. If you don't use this parameter, 100 results and a nextToken value,
//!   if applicable, are returned.
//! * `name` (String) **required** — The name of the Amazon EKS cluster to list updates for.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//! * `nodegroupName` (String) — The name of the Amazon EKS managed node group to list updates
//!   for.
//!
//! ## Output
//!
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//! * `updateIds` (StringList) — A list of all the updates for the specified cluster and
//!   Region.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource could not be found. You
//!   can view your available clusters with ListClusters. You can view your available managed
//!   node groups with ListNodegroups. Amazon EKS clusters and node groups are Amazon Web
//!   Services Region specific.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, UpdateScope, DEFAULT_LIST_PAGE_SIZE, UPDATES_LISTING};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListUpdatesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListUpdatesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#ListUpdates")]
#[derive(Debug)]
pub enum ListUpdatesErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_updates::ListUpdatesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListUpdatesError = crate::scaffolded::Scaffolded<ListUpdatesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListUpdatesErrorModeled> for ListUpdatesError {
    fn from(modeled: ListUpdatesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListUpdates` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListUpdates<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListUpdates<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListUpdates<Eng>
{
    type Input = ListUpdatesInput;
    type Output = ListUpdatesOutput;
    type Error = ListUpdatesError;

    const NAME: &'static str = "ListUpdates";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_UPDATES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_UPDATES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = state::service_state(&*self.engine).ok_or_else(|| {
            ListUpdatesError::from(ListUpdatesErrorModeled::ServerException(ServerException {
                cluster_name: Some(input.name.clone()),
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        let invalid_parameter = |message: String| {
            ListUpdatesError::from(ListUpdatesErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        // `maxResults` is modelled 1..100, so a value outside that range is REFUSED by
        // schema validation before this handler runs. Only the absent case gets a
        // value, and it is a whole page: "If you don't use this parameter, 100 results
        // and a nextToken value, if applicable, are returned." `.max(1)` is the floor
        // that bound already guarantees, written out so a page size of zero — which
        // would answer an empty page with no `nextToken`, a success claiming the
        // resource has no updates — cannot be reached even if that validation changes.
        let page_size = input
            .max_results
            .unwrap_or(DEFAULT_LIST_PAGE_SIZE)
            .max(1) as usize;

        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        // THE PARENT RESOURCE FIRST, so the one rule this listing has for an unknown
        // cluster wins over anything else wrong with the request.
        let Some(cluster) = guard.clusters.get(&(region.clone(), input.name.clone())) else {
            return Err(ListUpdatesErrorModeled::ResourceNotFoundException(
                state::cluster_not_found(&input.name),
            )
            .into());
        };
        // An update of this kind is a change to an Amazon EKS-managed control plane, and
        // a cluster `RegisterCluster` connected has none — so it has no such update
        // history at all, which is a refusal rather than an empty listing.
        if cluster.is_connected() {
            return Err(ListUpdatesErrorModeled::ClientException(
                state::connected_cluster_refusal(&input.name),
            )
            .into());
        }

        // Each of the three members narrows the listing to ONE resource INSIDE the
        // cluster, so they are alternatives rather than filters that compose: a request
        // naming two of them names no single resource to list the updates of. The
        // resolution is shared with `DescribeUpdate` ([`state::update_scope_of`]), so
        // the two cannot disagree about which updates belong to the resource a request
        // names.
        let Some(scope) = state::update_scope_of(
            input.nodegroup_name.as_deref(),
            input.addon_name.as_deref(),
            input.capability_name.as_deref(),
        ) else {
            return Err(invalid_parameter(
                state::ONE_UPDATE_SCOPE_MESSAGE.to_string(),
            ));
        };
        // The named resource must exist, else there is no update history to report —
        // the generic not-found payload every read of a resource inside a cluster
        // carries.
        let named_resource_exists = match &scope {
            UpdateScope::Cluster => true,
            UpdateScope::Nodegroup(nodegroup) => guard
                .nodegroups
                .contains_key(&(region.clone(), input.name.clone(), nodegroup.clone())),
            UpdateScope::Addon(addon) => guard
                .addons
                .contains_key(&(region.clone(), input.name.clone(), addon.clone())),
            UpdateScope::Capability(capability) => guard
                .capabilities
                .contains_key(&(region.clone(), input.name.clone(), capability.clone())),
        };
        if !named_resource_exists {
            return Err(ListUpdatesErrorModeled::ResourceNotFoundException(
                state::resource_not_found(),
            )
            .into());
        }

        // A `nextToken` names the update id to resume AFTER. One this listing did not
        // issue is a bad parameter, not an empty page.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                state::decode_scoped_page_token(UPDATES_LISTING, token).ok_or_else(|| {
                    invalid_parameter(state::INVALID_NEXT_TOKEN_MESSAGE.to_string())
                })?,
            ),
        };

        let mut remaining = cluster.update_ids_in_scope(&scope).filter(|id| {
            resume_after
                .as_deref()
                .is_none_or(|after| *id > after)
        });
        let page: Vec<String> = remaining.by_ref().take(page_size).map(str::to_owned).collect();
        // A token is returned only when something is left to resume from, so the last
        // page omits it.
        let next_token = match (remaining.next(), page.last()) {
            (Some(_), Some(last)) => Some(state::encode_scoped_page_token(UPDATES_LISTING, last)),
            _ => None,
        };

        Ok(ListUpdatesOutput {
            update_ids: Some(page),
            next_token,
        })
    }
}
