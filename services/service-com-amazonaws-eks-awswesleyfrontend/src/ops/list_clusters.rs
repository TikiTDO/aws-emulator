// SPDX-License-Identifier: CC0-1.0

//! @op ListClusters
//! IMPLEMENTED. Walks the cluster names the store actually holds, in the
//! `BTreeMap`'s sorted order, and pages them with an opaque `nextToken` derived
//! from the last name returned (so a token stays meaningful across creates and
//! deletes rather than indexing a transient vector).
//!
//! `include` is the listing's PROJECTION: it "indicates whether external clusters
//! are included in the returned list. Use 'all' to return connected clusters, or
//! blank to return only Amazon EKS clusters. 'all' must be in lowercase otherwise
//! an error occurs" (`ListClustersRequest$include`). So a cluster `RegisterCluster`
//! connected is listed only for a caller that asked for it, and any other token —
//! `all` in another case included — is refused rather than case-folded.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListClusters`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Lists the Amazon EKS clusters in your Amazon Web Services account in the specified
//! Amazon Web Services Region.
//!
//! ## Input
//!
//! * `include` (IncludeClustersList) — Indicates whether external clusters are included in
//!   the returned list. Use 'all' to return
//!   https://docs.aws.amazon.com/eks/latest/userguide/eks-connector.htmlconnected clusters,
//!   or blank to return only Amazon EKS clusters. 'all' must be in lowercase otherwise an
//!   error occurs.
//! * `maxResults` (ListClustersRequestMaxResults, range 1..100) — The maximum number of
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
//! * `clusters` (StringList) — A list of all of the clusters for your account in the
//!   specified Amazon Web Services Region .
//! * `nextToken` (String) — The nextToken value returned from a previous paginated request,
//!   where maxResults was used and the results exceeded the value of that parameter.
//!   Pagination continues from the end of the previous results that returned the nextToken
//!   value. This value is null when there are no more results to return. This token should be
//!   treated as an opaque identifier that is used only to retrieve the next items in a list
//!   and not for other programmatic purposes.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `InvalidRequestException` (HTTP 400) — widened in through `extra_errors` (see below):
//!   the request is invalid given the state of the cluster. Check the state of the cluster
//!   and the associated operations.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, DEFAULT_LIST_PAGE_SIZE, LIST_INCLUDE_ALL};

/// The sentence an `include` token set this listing cannot project is refused with.
///
/// The member documents one token and one casing of it, so every value outside that
/// set — an unknown token, or `all` in another case — is the same refusal, and it says
/// which parameter is at fault without echoing what the caller sent.
const INVALID_INCLUDE_MESSAGE: &str = "One or more values in the 'include' parameter is not valid";

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListClustersError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListClustersError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `InvalidRequestException` is widened in through `extra_errors`: this SERVICE models
// that shape ("The request is invalid...") and most of its operations declare it, but
// `ListClusters` does not list it — and it is the class the real service refuses a bad
// `include` projection with, so the handler needs it to answer that refusal.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#ListClusters",
    extra_errors = ["InvalidRequestException"]
)]
#[derive(Debug)]
pub enum ListClustersErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_clusters::ListClustersError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListClustersError = crate::scaffolded::Scaffolded<ListClustersErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListClustersErrorModeled> for ListClustersError {
    fn from(modeled: ListClustersErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListClusters` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListClusters<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListClusters<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListClusters<Eng>
{
    type Input = ListClustersInput;
    type Output = ListClustersOutput;
    type Error = ListClustersError;

    const NAME: &'static str = "ListClusters";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CLUSTERS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_CLUSTERS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListClustersError::from(ListClustersErrorModeled::ServerException(ServerException {
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        // A `nextToken` names the cluster to resume AFTER. One this service did not
        // issue is a bad parameter, not an empty page, and it is refused in the
        // sentence every listing of this service states for one
        // ([`state::INVALID_NEXT_TOKEN_MESSAGE`]).
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(state::decode_page_token(token).ok_or_else(|| {
                ListClustersError::from(ListClustersErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        message: Some(state::INVALID_NEXT_TOKEN_MESSAGE.to_string()),
                        ..Default::default()
                    },
                ))
            })?),
        };

        // "Use 'all' to return connected clusters, or blank to return only Amazon EKS
        // clusters. 'all' must be in lowercase otherwise an error occurs" — so the token
        // set is matched as an exact set membership test and is never case-folded: `all`
        // in another case is refused exactly like a token the member does not document
        // at all. No value at all, or an empty list, is "blank": only Amazon EKS
        // clusters.
        let include_connected = match input.include.as_deref() {
            None => false,
            Some(values) => {
                // An empty value is "blank" too: a client that sends `include=` with
                // nothing after it has named no projection.
                let named: Vec<&str> = values
                    .iter()
                    .map(String::as_str)
                    .filter(|value| !value.is_empty())
                    .collect();
                if named.iter().any(|value| *value != LIST_INCLUDE_ALL) {
                    // The projection a caller asked for cannot be served, which is a
                    // refusal of the REQUEST rather than a complaint about the shape of
                    // a parameter — and it is permanent, so the sentence names no value
                    // from the request: every unacceptable token set gets this one
                    // refusal, as the real service's does.
                    return Err(ListClustersError::from(
                        ListClustersErrorModeled::InvalidRequestException(
                            InvalidRequestException {
                                message: Some(INVALID_INCLUDE_MESSAGE.to_string()),
                                ..Default::default()
                            },
                        ),
                    ));
                }
                !named.is_empty()
            }
        };

        // `maxResults` is modelled 1..100, so a value outside that range is REFUSED by
        // schema validation before this handler runs — it is not coerced here, which
        // would quietly hide a future gap in that validation. Only the absent case gets
        // a value, and it is a whole page. (`usize::try_from` cannot fail for a value
        // the model bounds above zero; a hypothetical negative falls back to the same
        // default rather than silently paging nothing.)
        let page_size = usize::try_from(input.max_results.unwrap_or(DEFAULT_LIST_PAGE_SIZE))
            .unwrap_or(DEFAULT_LIST_PAGE_SIZE as usize);

        let guard = store.lock();
        // Clusters are listed per REGION: a name identifies a cluster only within the
        // region its ARN names, so this walks only the region the request is signed
        // for, under the projection `include` asked for.
        let mut remaining = guard
            .listed_cluster_names_in(&region, include_connected)
            .filter(|name| resume_after.as_deref().is_none_or(|after| *name > after));
        let clusters: Vec<String> = remaining
            .by_ref()
            .take(page_size)
            .map(str::to_owned)
            .collect();
        // A token is returned only when something is left to resume from, so the last
        // page omits it.
        let next_token = match (remaining.next(), clusters.last()) {
            (Some(_), Some(last)) => Some(state::encode_page_token(last)),
            _ => None,
        };

        Ok(ListClustersOutput {
            clusters: Some(clusters),
            next_token,
        })
    }
}
