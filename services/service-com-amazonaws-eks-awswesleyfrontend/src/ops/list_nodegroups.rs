// SPDX-License-Identifier: CC0-1.0

//! @op ListNodegroups
//!
//! IMPLEMENTED. Walks the managed node group names the store holds for ONE cluster
//! (`crate::state::EksState::nodegroup_names_in`, the name-sorted range of the node
//! group map that belongs to that cluster), and pages them with an opaque
//! `nextToken` derived from the last name returned — so a token stays meaningful
//! across creates and deletes rather than indexing a transient vector.
//!
//! This is a REPORTING operation: it writes nothing of its own, and the cluster it
//! lists is the one in the region the request is signed for, because a node group
//! name identifies a node group only within one cluster.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListNodegroups`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Lists the managed node groups associated with the specified cluster in your Amazon Web
//! Services account in the specified Amazon Web Services Region. Self-managed node groups
//! aren't listed.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of your cluster.
//! * `maxResults` (ListNodegroupsRequestMaxResults, range 1..100) — The maximum number of
//!   results, returned in paginated output. You receive maxResults in a single page, along
//!   with a nextToken response element. You can see the remaining results of the initial
//!   request by sending another request with the returned nextToken value. This value can be
//!   between 1 and 100. If you don't use this parameter, 100 results and a nextToken value,
//!   if applicable, are returned.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//!
//! ## Output
//!
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//! * `nodegroups` (StringList) — A list of all of the node groups associated with the
//!   specified cluster.
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
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

use std::time::SystemTime;

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, DEFAULT_LIST_PAGE_SIZE, NODEGROUPS_LISTING};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListNodegroupsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListNodegroupsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#ListNodegroups")]
#[derive(Debug)]
pub enum ListNodegroupsErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_nodegroups::ListNodegroupsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListNodegroupsError = crate::scaffolded::Scaffolded<ListNodegroupsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListNodegroupsErrorModeled> for ListNodegroupsError {
    fn from(modeled: ListNodegroupsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListNodegroups` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListNodegroups<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListNodegroups<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListNodegroups<Eng>
{
    type Input = ListNodegroupsInput;
    type Output = ListNodegroupsOutput;
    type Error = ListNodegroupsError;

    const NAME: &'static str = "ListNodegroups";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_NODEGROUPS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_NODEGROUPS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListNodegroupsError::from(ListNodegroupsErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        // `maxResults` is modelled 1..100, so a value outside that range is REFUSED by
        // schema validation before this handler runs. Only the absent case gets a value,
        // and it is a whole page; the floor of 1 is what that bound already guarantees,
        // written out so a page size of zero cannot answer an empty page.
        let page_size = input
            .max_results
            .unwrap_or(DEFAULT_LIST_PAGE_SIZE)
            .max(1) as usize;

        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        // THE PARENT RESOURCE FIRST: a listing of a cluster this region does not hold is
        // that 404 whatever else the request carries.
        let Some(cluster) = guard
            .clusters
            .get(&(region.clone(), input.cluster_name.clone()))
        else {
            return Err(ListNodegroupsErrorModeled::ResourceNotFoundException(
                crate::state::cluster_not_found(&input.cluster_name),
            )
            .into());
        };
        // A managed node group is a resource of the Amazon EKS-managed plane, which a
        // cluster `RegisterCluster` connected has none of: EKS does not manage its
        // nodes, so this operation does not apply to it — which is a refusal, not an
        // empty listing, because the listing itself is not something EKS keeps for that
        // cluster.
        if cluster.is_connected() {
            return Err(ListNodegroupsErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        // A `nextToken` names the node group to resume AFTER. One this listing did not
        // issue is a bad parameter, not an empty page.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                state::decode_scoped_page_token(NODEGROUPS_LISTING, token).ok_or_else(|| {
                    ListNodegroupsError::from(
                        ListNodegroupsErrorModeled::InvalidParameterException(
                            InvalidParameterException {
                                cluster_name: Some(input.cluster_name.clone()),
                                message: Some(state::INVALID_NEXT_TOKEN_MESSAGE.to_string()),
                                ..Default::default()
                            },
                        ),
                    )
                })?,
            ),
        };

        let mut remaining = guard
            .nodegroup_names_in(&region, &input.cluster_name)
            .filter(|name| resume_after.as_deref().is_none_or(|after| *name > after));
        let nodegroups: Vec<String> = remaining
            .by_ref()
            .take(page_size)
            .map(str::to_owned)
            .collect();
        // A token is returned only when something is left to resume from, so the last
        // page omits it.
        let next_token = match (remaining.next(), nodegroups.last()) {
            (Some(_), Some(last)) => {
                Some(state::encode_scoped_page_token(NODEGROUPS_LISTING, last))
            }
            _ => None,
        };

        Ok(ListNodegroupsOutput {
            nodegroups: Some(nodegroups),
            next_token,
        })
    }
}
