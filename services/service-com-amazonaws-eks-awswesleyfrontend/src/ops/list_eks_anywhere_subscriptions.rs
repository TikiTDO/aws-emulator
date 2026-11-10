// SPDX-License-Identifier: CC0-1.0

//! @op ListEksAnywhereSubscriptions
//! IMPLEMENTED. Pages the
//! [`crate::state::EksAnywhereSubscriptionRecord`]s of the Region the request is
//! signed for — "a list of all subscription objects in the region, filtered by
//! includeStatus and paginated by nextToken and maxResults" — computing every entry
//! from the record `CreateEksAnywhereSubscription` wrote.
//!
//! The rules it enforces:
//!
//! * "If you don't use this parameter, ListEksAnywhereSubscriptions returns up to 10
//!   results" (`$maxResults`), a default this operation states and the other
//!   listings of this service do not share;
//! * `includeStatus` is "an array of subscription statuses to filter on", so a
//!   request that names some reports only the subscriptions in those statuses and one
//!   that names none reports them all;
//! * a `nextToken` names the subscription to resume AFTER, and one this service did
//!   not issue is `InvalidParameterException` rather than an empty page.
//!
//! The read materialises the status moves the clock already owes each record and
//! writes nothing else.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListEksAnywhereSubscriptions`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")
//! ```
//!
//! Displays the full description of the subscription.
//!
//! ## Input
//!
//! * `includeStatus` (EksAnywhereSubscriptionStatusValues) — An array of subscription
//!   statuses to filter on.
//! * `maxResults` (ListEksAnywhereSubscriptionsRequestMaxResults, range 1..100) — The maximum
//!   number of cluster results returned by ListEksAnywhereSubscriptions in paginated output.
//!   When you use this parameter, ListEksAnywhereSubscriptions returns only maxResults
//!   results in a single page along with a nextToken response element. You can see the
//!   remaining results of the initial request by sending another ListEksAnywhereSubscriptions
//!   request with the returned nextToken value. This value can be between 1 and 100. If you
//!   don't use this parameter, ListEksAnywhereSubscriptions returns up to 10 results and a
//!   nextToken value if applicable.
//! * `nextToken` (String) — The nextToken value returned from a previous paginated
//!   ListEksAnywhereSubscriptions request where maxResults was used and the results exceeded
//!   the value of that parameter. Pagination continues from the end of the previous results
//!   that returned the nextToken value.
//!
//! ## Output
//!
//! * `nextToken` (String) — The nextToken value to include in a future
//!   ListEksAnywhereSubscriptions request. When the results of a ListEksAnywhereSubscriptions
//!   request exceed maxResults, you can use this value to retrieve the next page of results.
//!   This value is null when there are no more results to return.
//! * `subscriptions` (EksAnywhereSubscriptionList) — A list of all subscription objects in
//!   the region, filtered by includeStatus and paginated by nextToken and maxResults.
//!
//! ## Errors
//!
//! * `ClientException` (HTTP 400) — These errors are usually caused by a client action.
//!   Actions can include using an action or resource on behalf of an IAM principal that
//!   doesn't have permissions to use the action or resource or specifying an identifier that
//!   is not valid.
//! * `InvalidParameterException` (HTTP 400) — The specified parameter is invalid. Review the
//!   available parameters for the API request.
//! * `ServerException` (HTTP 500) — These errors are usually caused by a server-side issue.
//! * `ServiceUnavailableException` (HTTP 503) — The service is unavailable. Back off and
//!   retry the operation.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{self, EKS_ANYWHERE_SUBSCRIPTIONS_LISTING};
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListEksAnywhereSubscriptionsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-eks-awswesleyfrontend.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListEksAnywhereSubscriptionsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.eks#ListEksAnywhereSubscriptions")]
#[derive(Debug)]
pub enum ListEksAnywhereSubscriptionsErrorModeled {
    ClientException(ClientException),
    InvalidParameterException(InvalidParameterException),
    ServerException(ServerException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_eks_anywhere_subscriptions::ListEksAnywhereSubscriptionsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListEksAnywhereSubscriptionsError = crate::scaffolded::Scaffolded<ListEksAnywhereSubscriptionsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListEksAnywhereSubscriptionsErrorModeled> for ListEksAnywhereSubscriptionsError {
    fn from(modeled: ListEksAnywhereSubscriptionsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListEksAnywhereSubscriptions` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListEksAnywhereSubscriptions<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListEksAnywhereSubscriptions<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListEksAnywhereSubscriptions<Eng>
{
    type Input = ListEksAnywhereSubscriptionsInput;
    type Output = ListEksAnywhereSubscriptionsOutput;
    type Error = ListEksAnywhereSubscriptionsError;

    const NAME: &'static str = "ListEksAnywhereSubscriptions";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_EKS_ANYWHERE_SUBSCRIPTIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_EKS_ANYWHERE_SUBSCRIPTIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = state::request_region(ctx.parts());
        let store = state::service_state(&*self.engine).ok_or_else(|| {
            ListEksAnywhereSubscriptionsError::from(
                ListEksAnywhereSubscriptionsErrorModeled::ServerException(ServerException {
                    message: Some("the eks emulator store is unavailable".to_string()),
                    ..Default::default()
                }),
            )
        })?;

        let invalid_parameter = |message: String| {
            ListEksAnywhereSubscriptionsError::from(
                ListEksAnywhereSubscriptionsErrorModeled::InvalidParameterException(
                    InvalidParameterException {
                        message: Some(message),
                        ..Default::default()
                    },
                ),
            )
        };

        // `maxResults` is modelled 1..100, so a value outside that range is REFUSED
        // before this handler runs. Only the absent case gets a value, and this
        // operation states its own: "If you don't use this parameter,
        // ListEksAnywhereSubscriptions returns up to 10 results". `.max(1)` is the
        // floor that bound already guarantees, written out so a page size of zero —
        // which would answer an empty page with no `nextToken`, a success claiming the
        // account holds nothing — cannot be reached even if the boundary validation
        // changes.
        let page_size = input
            .max_results
            .unwrap_or(DEFAULT_SUBSCRIPTION_PAGE_SIZE)
            .max(1) as usize;

        // A `nextToken` names the subscription to resume AFTER. One this service did
        // not issue is a bad parameter, not an empty page.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                state::decode_scoped_page_token(EKS_ANYWHERE_SUBSCRIPTIONS_LISTING, token)
                    .ok_or_else(|| {
                        invalid_parameter(state::INVALID_NEXT_TOKEN_MESSAGE.to_string())
                    })?,
            ),
        };

        let mut guard = store.lock();
        guard.materialize_due_transitions(SystemTime::now());

        // "Filtered by includeStatus": a request that names statuses reports only the
        // subscriptions in them, and one that names none reports every subscription of
        // the Region. The member targets an enum, so a status outside the modelled set
        // is already refused before this handler runs.
        let included = input.include_status.clone().unwrap_or_default();
        let mut remaining = guard
            .eks_anywhere_subscriptions_in(&region)
            .filter(|record| included.is_empty() || included.contains(&record.status))
            .filter(|record| {
                resume_after
                    .as_deref()
                    .is_none_or(|after| record.id.as_str() > after)
            });
        let page: Vec<&crate::state::EksAnywhereSubscriptionRecord> =
            remaining.by_ref().take(page_size).collect();
        // "This value is null when there are no more results to return", so a token is
        // returned only when something is left to resume from.
        let next_token = match (remaining.next(), page.last()) {
            (Some(_), Some(last)) => Some(state::encode_scoped_page_token(
                EKS_ANYWHERE_SUBSCRIPTIONS_LISTING,
                &last.id,
            )),
            _ => None,
        };
        let subscriptions: Vec<EksAnywhereSubscription> = page
            .iter()
            .map(|record| record.to_subscription())
            .collect();

        Ok(ListEksAnywhereSubscriptionsOutput {
            subscriptions: Some(subscriptions),
            next_token,
        })
    }
}

/// The page size this listing uses when the request names none: "If you don't use
/// this parameter, ListEksAnywhereSubscriptions returns up to 10 results"
/// (`ListEksAnywhereSubscriptionsRequest$maxResults`).
///
/// Its own constant rather than the service's
/// [`crate::state::DEFAULT_LIST_PAGE_SIZE`], because this operation documents a
/// different default from every other listing here.
const DEFAULT_SUBSCRIPTION_PAGE_SIZE: i32 = 10;
