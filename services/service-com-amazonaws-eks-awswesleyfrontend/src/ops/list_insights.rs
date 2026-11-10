// SPDX-License-Identifier: CC0-1.0

//! @op ListInsights
//! IMPLEMENTED. Reports "all insights checked for against the specified cluster",
//! computed from this service's own state by
//! [`crate::state::ComAmazonawsEksAwswesleyfrontendState::cluster_insights`]: every
//! [`crate::state::InsightCheck`] evaluated against the cluster, the node groups and
//! add-ons inside it, and the add-on catalog. An insight is the RESULT of a check, so
//! nothing creates one and nothing here keeps a second copy of one.
//!
//! It is a REPORTING operation: the only write it performs is materialising the
//! transitions the clock already owed — among them the completion of an on-demand
//! insights refresh, which is what moves every insight's `lastRefreshTime`
//! ([`crate::state::ClusterRecord::insights_refreshed_at`]) — each of which was due at
//! that instant whether or not anybody called.
//!
//! The rules it enforces:
//!
//! * an unknown `clusterName` in the region the request is signed for is the same
//!   `ResourceNotFoundException` every cluster-subject operation gives;
//! * cluster insights are an analysis Amazon EKS runs against the control plane it
//!   manages, so a cluster `RegisterCluster` connected has none at all — the
//!   permanent `ClientException` refusal this service's EKS-managed plane gives,
//!   rather than an empty listing;
//! * "The default filter lists all categories and every status", so an absent
//!   `filter` narrows nothing; a `filter` member keeps only the insights whose
//!   category, associated Kubernetes version or status it names, and a token no
//!   modelled enum declares is an `InvalidParameterException`;
//! * `maxResults` / `nextToken` page the filtered listing, and a token this listing
//!   did not issue — or one carried over to a differently filtered call — is an
//!   `InvalidParameterException` rather than a silent restart.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListInsights`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Returns a list of all insights checked for against the specified cluster. You can filter
//! which insights are returned by category, associated Kubernetes version, and status. The
//! default filter lists all categories and every status. The following lists the available
//! categories: UPGRADE_READINESS: Amazon EKS identifies issues that could impact your
//! ability to upgrade to new versions of Kubernetes. These are called upgrade insights.
//! MISCONFIGURATION: Amazon EKS identifies misconfiguration in your EKS Hybrid Nodes setup
//! that could impair functionality of your cluster or workloads. These are called
//! configuration insights.
//!
//! ## Input
//!
//! * `clusterName` (String) **required** — The name of the Amazon EKS cluster associated with
//!   the insights.
//! * `filter` (InsightsFilter) — The criteria to filter your list of insights for your
//!   cluster. You can filter which insights are returned by category, associated Kubernetes
//!   version, and status.
//! * `maxResults` (ListInsightsMaxResults, range 1..100) — The maximum number of identity
//!   provider configurations returned by ListInsights in paginated output. When you use this
//!   parameter, ListInsights returns only maxResults results in a single page along with a
//!   nextToken response element. You can see the remaining results of the initial request by
//!   sending another ListInsights request with the returned nextToken value. This value can
//!   be between 1 and 100. If you don't use this parameter, ListInsights returns up to 100
//!   results and a nextToken value, if applicable.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated
//!   ListInsights request. When the results of a ListInsights request exceed maxResults, you
//!   can use this value to retrieve the next page of results. This value is null when there
//!   are no more results to return.
//!
//! ## Output
//!
//! * `insights` (InsightSummaries) — The returned list of insights.
//! * `nextToken` (String) — The nextToken value to include in a future ListInsights request.
//!   When the results of a ListInsights request exceed maxResults, you can use this value to
//!   retrieve the next page of results. This value is null when there are no more results to
//!   return.
//!
//! ## Errors
//!
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
use crate::state::{self, InsightView, DEFAULT_LIST_PAGE_SIZE, INSIGHTS_LISTING};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListInsightsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListInsightsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
//
// `ClientException` is widened in through `extra_errors`: this SERVICE models that
// shape and many of its operations declare it, but this one does not list it — and it
// is the class every operation of the Amazon EKS-managed plane refuses a cluster this
// service does not manage with ([`crate::state::connected_cluster_refusal`]), which is
// what the other two insights operations answer for such a cluster too.
#[smithy_macros::operation_error(
    "com.amazonaws.eks#ListInsights",
    extra_errors = ["ClientException"]
)]
#[derive(Debug)]
pub enum ListInsightsErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    InvalidRequestException(InvalidRequestException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServerException(ServerException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_insights::ListInsightsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListInsightsError = crate::scaffolded::Scaffolded<ListInsightsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListInsightsErrorModeled> for ListInsightsError {
    fn from(modeled: ListInsightsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListInsights` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListInsights<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListInsights<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListInsights<Eng>
{
    type Input = ListInsightsInput;
    type Output = ListInsightsOutput;
    type Error = ListInsightsError;

    const NAME: &'static str = "ListInsights";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_INSIGHTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_INSIGHTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListInsightsError::from(ListInsightsErrorModeled::ServerException(ServerException {
                cluster_name: Some(input.cluster_name.clone()),
                message: Some("the eks emulator store is unavailable".to_string()),
                ..Default::default()
            }))
        })?;

        let invalid_parameter = |message: String| {
            ListInsightsError::from(ListInsightsErrorModeled::InvalidParameterException(
                InvalidParameterException {
                    cluster_name: Some(input.cluster_name.clone()),
                    message: Some(message),
                    ..Default::default()
                },
            ))
        };

        // "This value can be between 1 and 100" is modelled as a range on
        // `maxResults`, so a value outside it is REFUSED before this handler runs.
        // Only the absent case gets a value: "If you don't use this parameter,
        // ListInsights returns up to 100 results". The floor that bound already
        // guarantees is written out so a page size of zero — an empty page with no
        // `nextToken`, a success claiming the cluster has no insights — cannot be
        // reached even if the boundary validation changes.
        let page_size = usize::try_from(input.max_results.unwrap_or(DEFAULT_LIST_PAGE_SIZE))
            .unwrap_or(DEFAULT_LIST_PAGE_SIZE as usize)
            .max(1);

        // "You can filter which insights are returned by category, associated
        // Kubernetes version, and status. The default filter lists all categories and
        // every status", so an absent filter — and an absent member of one — narrows
        // nothing. A token the filtered member's enum does not declare names no
        // insight at all, which is a bad parameter rather than an empty page.
        let filter = input.filter.clone().unwrap_or_default();
        let categories = filter.categories.unwrap_or_default();
        let statuses = filter.statuses.unwrap_or_default();
        let kubernetes_versions = filter.kubernetes_versions.unwrap_or_default();
        for category in &categories {
            if !CATEGORY_VALUES.contains(&category.as_str()) {
                return Err(unknown_enum_member(
                    &invalid_parameter,
                    "filter.categories",
                    category,
                    CATEGORY_VALUES,
                ));
            }
        }
        for status in &statuses {
            if !INSIGHT_STATUS_VALUE_VALUES.contains(&status.as_str()) {
                return Err(unknown_enum_member(
                    &invalid_parameter,
                    "filter.statuses",
                    status,
                    INSIGHT_STATUS_VALUE_VALUES,
                ));
            }
        }

        // A `nextToken` names the insight to resume AFTER. One this listing did not
        // issue — including a well-formed token another listing minted — is a bad
        // parameter, not an empty page.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                state::decode_scoped_page_token(INSIGHTS_LISTING, token).ok_or_else(|| {
                    invalid_parameter(state::INVALID_NEXT_TOKEN_MESSAGE.to_string())
                })?,
            ),
        };

        let now = SystemTime::now();
        let mut guard = store.lock();
        // Every clock-owed move first — including the completion of an on-demand
        // insights refresh, which is the instant the insights were last analysed at —
        // so what this reports is what every later caller reports too.
        guard.materialize_due_transitions(now);

        // THE CLUSTER FIRST: the insights of a cluster this region does not hold are
        // the missing cluster, whatever else the request asks for.
        let Some(cluster) = guard.clusters.get(&(region.clone(), input.cluster_name.clone()))
        else {
            return Err(ListInsightsErrorModeled::ResourceNotFoundException(
                state::cluster_not_found(&input.cluster_name),
            )
            .into());
        };
        // Cluster insights are an analysis Amazon EKS runs against the control plane it
        // manages, and a cluster `RegisterCluster` connected has none: this operation
        // does not apply to it, which is a refusal rather than an empty listing because
        // the analysis itself is not something EKS keeps for that cluster.
        if cluster.is_connected() {
            return Err(ListInsightsErrorModeled::ClientException(
                state::connected_cluster_refusal(&input.cluster_name),
            )
            .into());
        }

        let matching: Vec<InsightView> = guard
            .cluster_insights(&region, &input.cluster_name)
            .into_iter()
            .filter(|insight| {
                categories.is_empty() || categories.iter().any(|wanted| wanted == insight.check.category())
            })
            .filter(|insight| {
                statuses.is_empty() || statuses.contains(&insight.status)
            })
            // An insight that is about no Kubernetes version is not one a version
            // filter names: the filter asks for "the Kubernetes versions associated
            // with the insights to return".
            .filter(|insight| {
                kubernetes_versions.is_empty()
                    || insight
                        .kubernetes_version
                        .as_ref()
                        .is_some_and(|version| kubernetes_versions.contains(version))
            })
            .collect();

        // The resume point is a position in THIS filtered listing: a token carried over
        // to a differently filtered call names an insight that is not in it, which is a
        // bad parameter rather than a silent restart from the top.
        let start = match resume_after {
            None => 0,
            Some(id) => matching
                .iter()
                .position(|insight| insight.id == id)
                .map(|position| position + 1)
                .ok_or_else(|| invalid_parameter(state::INVALID_NEXT_TOKEN_MESSAGE.to_string()))?,
        };

        let remaining = &matching[start..];
        let returned = remaining.len().min(page_size);
        let insights: Vec<InsightSummary> = remaining[..returned]
            .iter()
            .map(InsightView::to_summary)
            .collect();
        // A token is returned only when something is left to resume from, so the last
        // page omits it.
        let next_token = (returned < remaining.len())
            .then(|| returned.checked_sub(1).and_then(|last| remaining.get(last)))
            .flatten()
            .map(|insight| state::encode_scoped_page_token(INSIGHTS_LISTING, &insight.id));

        Ok(ListInsightsOutput {
            insights: Some(insights),
            next_token,
        })
    }
}

/// Refuse a filter member whose token the model's enum does not declare, naming the
/// member, the value and the members that enum accepts — built from the request and
/// the model rather than from a fixed sentence.
fn unknown_enum_member(
    invalid_parameter: &impl Fn(String) -> ListInsightsError,
    member: &str,
    token: &str,
    accepted: &[&str],
) -> ListInsightsError {
    invalid_parameter(format!(
        "Value '{token}' at '{member}' failed to satisfy constraint: Member must satisfy enum \
         value set: [{}]",
        accepted.join(", ")
    ))
}
