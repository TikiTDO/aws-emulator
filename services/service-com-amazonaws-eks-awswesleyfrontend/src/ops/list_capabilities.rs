// SPDX-License-Identifier: CC0-1.0

//! @op ListCapabilities
//! IMPLEMENTED. Walks the capabilities one cluster holds, in capability-name order, and
//! reports each as a `CapabilitySummary` computed from the stored
//! [`crate::state::CapabilityRecord`] — so a listing agrees with `DescribeCapability`
//! name for name.
//!
//! Paging follows `ListClusters`: a page holds at most `maxResults` summaries
//! ([`crate::state::DEFAULT_LIST_PAGE_SIZE`] when the caller gives none, the whole page
//! the model's 1..100 bound allows), and `nextToken` is returned only when something is
//! left — it names the capability to resume AFTER, so it stays valid across creates and
//! deletes, and a token this listing did not mint (including one another listing issued,
//! which is what the [`crate::state::CAPABILITIES_LISTING`] token scope rules out) is an
//! `InvalidParameterException` rather than an empty page.
//!
//! An unknown `clusterName` is a `ResourceNotFoundException` — the same 404
//! `DescribeCapability` and `DeleteCapability` give, kept as ONE rule across the resource
//! (see the `extra_errors` widening below) — and never an empty page, which would claim a
//! cluster that does not exist holds no capabilities.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListCapabilities`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Lists all managed capabilities in your Amazon EKS cluster. You can use this operation to
//! get an overview of all capabilities and their current status.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of the Amazon EKS cluster for which you
//!   want to list capabilities.
//! * `maxResults` (ListCapabilitiesRequestMaxResults, range 1..100) — The maximum number of
//!   results to return in a single call. To retrieve the remaining results, make another call
//!   with the returned nextToken value. If you don't specify a value, the default is 100
//!   results.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return.
//!
//! ## Output
//!
//! * `capabilities` (CapabilitySummaryList) — A list of capability summary objects, each
//!   containing basic information about a capability including its name, ARN, type, status,
//!   version, and timestamps.
//! * `nextToken` (String) — The nextToken value to include in a future ListCapabilities
//!   request. When the results of a ListCapabilities request exceed maxResults, you can use
//!   this value to retrieve the next page of results. This value is null when there are no
//!   more results to return.
//!
//! ## Errors
//!
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, CAPABILITIES_LISTING, DEFAULT_LIST_PAGE_SIZE};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListCapabilitiesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors, widened by `ResourceNotFoundException`. The
// scaffold's `NotImplemented` cannot live here — `#[operation_error]` accepts only
// variants the model declares — hence the `ListCapabilitiesError` alias below.
//
// `extra_errors` is deliberate: `ListCapabilities` takes `clusterName` as an
// @httpLabel, so a request can name a cluster that does not exist, and the only
// honest answer is the same 404 every other cluster-scoped operation gives. The
// model's error union for THIS operation omits the shape (the service declares it,
// and `DescribeCapability`, `DeleteCapability`, `DescribeCluster` and `DeleteCluster`
// all list it), so the widening keeps one rule across the resource instead of
// inventing a second one here.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#ListCapabilities",
    extra_errors = ["ResourceNotFoundException", "ClientException"]
)]
#[derive(Debug)]
pub enum ListCapabilitiesErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_capabilities::ListCapabilitiesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListCapabilitiesError = crate::scaffolded::Scaffolded<ListCapabilitiesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListCapabilitiesErrorModeled> for ListCapabilitiesError {
    fn from(modeled: ListCapabilitiesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListCapabilities` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListCapabilities<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListCapabilities<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListCapabilities<Eng>
{
    type Input = ListCapabilitiesInput;
    type Output = ListCapabilitiesOutput;
    type Error = ListCapabilitiesError;

    const NAME: &'static str = "ListCapabilities";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CAPABILITIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CAPABILITIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListCapabilitiesError::from(ListCapabilitiesErrorModeled::ServerException(
                ServerException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                },
            ))
        })?;

        let invalid_parameter = |message: String| {
            ListCapabilitiesError::from(ListCapabilitiesErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        // `maxResults` is modelled 1..100, so a value outside that range is REFUSED by
        // schema validation before this handler runs. Only the absent case gets a
        // value, and it is a whole page: "If you don't specify a value, the default is
        // 100 results." `.max(1)` is the floor that bound already guarantees, written
        // out so a page size of zero — which would answer an empty page with no
        // `nextToken`, a success claiming the cluster holds nothing — cannot be
        // reached even if the boundary validation changes.
        let page_size = input
            .max_results
            .unwrap_or(DEFAULT_LIST_PAGE_SIZE)
            .max(1) as usize;

        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        // THE PARENT RESOURCE FIRST, so the one rule this resource has for an unknown
        // parent wins over anything else wrong with the request — a listing of a
        // cluster that does not exist is that 404 whether or not the token is also
        // malformed.
        let Some(cluster) = guard
            .clusters
            .get(&(region.clone(), input.cluster_name.clone()))
        else {
            return Err(ListCapabilitiesErrorModeled::ResourceNotFoundException(
                crate::state::cluster_not_found(&input.cluster_name),
            )
            .into());
        };
        // A managed capability is software Amazon EKS runs in the cluster's control
        // plane, and a cluster `RegisterCluster` connected has none of that plane: this
        // operation does not apply to it — which is a refusal, not an empty listing,
        // because the listing itself is not something EKS keeps for that cluster.
        if cluster.is_connected() {
            return Err(ListCapabilitiesErrorModeled::ClientException(
                crate::state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        // A `nextToken` names the capability to resume AFTER. One this service did not
        // issue is a bad parameter, not an empty page.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                state::decode_scoped_page_token(CAPABILITIES_LISTING, token).ok_or_else(|| {
                    invalid_parameter(state::INVALID_NEXT_TOKEN_MESSAGE.to_string())
                })?,
            ),
        };

        let mut remaining = guard
            .capabilities_of(&region, &input.cluster_name)
            .filter(|record| {
                resume_after
                    .as_deref()
                    .is_none_or(|after| record.capability_name.as_str() > after)
            });
        let page: Vec<&crate::state::CapabilityRecord> =
            remaining.by_ref().take(page_size).collect();
        // A token is returned only when something is left to resume from, so the last
        // page omits it.
        let next_token = match (remaining.next(), page.last()) {
            (Some(_), Some(last)) => Some(state::encode_scoped_page_token(
                CAPABILITIES_LISTING,
                &last.capability_name,
            )),
            _ => None,
        };
        let capabilities: Vec<CapabilitySummary> = page
            .iter()
            .map(|record| record.to_summary())
            .collect();

        Ok(ListCapabilitiesOutput {
            capabilities: Some(capabilities),
            next_token,
        })
    }
}
