// SPDX-License-Identifier: CC0-1.0

//! @op ListOpsItemRelatedItems
//! Lists the related-item resources associated with an OpsItem: every summary it reports
//! is projected from an association `AssociateOpsItemRelatedItem` persisted in
//! [`crate::state::ops_item_related_items`], narrowed by the filters the model declares
//! and paged by the cursor that store's order makes total.
//!
//!
//! ---
//!
//! # Modeled contract for `ListOpsItemRelatedItems`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Lists all related-item resources associated with a Systems Manager OpsCenter OpsItem.
//! OpsCenter is a tool in Amazon Web Services Systems Manager.
//!
//! ## Input
//!
//! * `Filters` (OpsItemRelatedItemsFilters) — One or more OpsItem filters. Use a filter to
//!   return a more specific list of results.
//! * `MaxResults` (OpsItemRelatedItemsMaxResults, range 1..50) — The maximum number of items
//!   to return for this call. The call also returns a token that you can specify in a
//!   subsequent call to get the next set of results.
//! * `NextToken` (String) — The token for the next set of items to return. (You received this
//!   token from a previous call.)
//! * `OpsItemId` (OpsItemId, pattern ^(oi)-[0-9a-f]{12}$) — The ID of the OpsItem for which
//!   you want to list all related-item resources.
//!
//! ## Output
//!
//! * `NextToken` (String) — The token for the next set of items to return. Use this token to
//!   get the next set of results.
//! * `Summaries` (OpsItemRelatedItemSummaries) — A list of related-item resources for the
//!   specified OpsItem.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `OpsItemInvalidParameterException` (HTTP 400) — A specified parameter argument isn't
//!   valid. Verify the available arguments and try again.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListOpsItemRelatedItemsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListOpsItemRelatedItemsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ssm#ListOpsItemRelatedItems")]
#[derive(Debug)]
pub enum ListOpsItemRelatedItemsErrorModeled {
    InternalServerError(InternalServerError),
    OpsItemInvalidParameterException(OpsItemInvalidParameterException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_ops_item_related_items::ListOpsItemRelatedItemsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListOpsItemRelatedItemsError = crate::scaffolded::Scaffolded<ListOpsItemRelatedItemsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListOpsItemRelatedItemsErrorModeled> for ListOpsItemRelatedItemsError {
    fn from(modeled: ListOpsItemRelatedItemsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListOpsItemRelatedItems` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListOpsItemRelatedItems<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListOpsItemRelatedItems<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListOpsItemRelatedItems<Eng>
{
    type Input = ListOpsItemRelatedItemsInput;
    type Output = ListOpsItemRelatedItemsOutput;
    type Error = ListOpsItemRelatedItemsError;

    const NAME: &'static str = "ListOpsItemRelatedItems";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_OPS_ITEM_RELATED_ITEMS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_OPS_ITEM_RELATED_ITEMS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::ops_item_related_items as related_items;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListOpsItemRelatedItemsErrorModeled::InternalServerError(InternalServerError {
                message: Some("the OpsItem store is unavailable".to_string()),
            })
        })?;

        let invalid_parameter = |parameter: &str, message: String| {
            ListOpsItemRelatedItemsError::from(
                ListOpsItemRelatedItemsErrorModeled::OpsItemInvalidParameterException(
                    OpsItemInvalidParameterException {
                        parameter_names: Some(vec![parameter.to_string()]),
                        message: Some(message),
                    },
                ),
            )
        };

        // A filter that names no property of a related item, or an operator this
        // operation does not support, cannot be served: the request is refused before
        // anything is asked of the store, rather than answered as if the filter had been
        // applied.
        let filters = input.filters.as_deref().unwrap_or_default();
        if let Some(rejection) = filters
            .iter()
            .find_map(|filter| {
                related_items::filter_rejection(&filter.key, &filter.operator, &filter.values)
            })
        {
            return Err(invalid_parameter(&rejection.parameter, rejection.message));
        }

        let page_size = related_items::related_item_page_size(input.max_results)
            .map_err(|message| invalid_parameter("MaxResults", message))?;

        // The token this operation hands out is the association id the next page starts
        // at, so a token naming an association the store no longer holds in view is not a
        // token this listing can resume from.
        let resume_from = input
            .next_token
            .as_deref()
            .filter(|token| !token.is_empty());

        let region = crate::state::request_region(ctx.parts());
        let mut store = state.lock();
        // The removal of an OpsItem whose delete had settled by now was owed whether or not
        // anybody called, and the associations held against it went with it, so this listing
        // reports them neither.
        store.materialize_due(::std::time::SystemTime::now());
        // Systems Manager is regional, and an association belongs to the OpsItem it was
        // created against: a listing reports this Region's associations, narrowed to the
        // one OpsItem when the request names one and spanning every OpsItem of the Region
        // when it names none. The store yields them in ascending
        // `(ops item id, association id)` order, which is what makes the resume cursor
        // total.
        let selected = store
            .related_items_in_region(&region)
            .filter(|record| {
                input
                    .ops_item_id
                    .as_deref()
                    .is_none_or(|wanted| record.ops_item_id == wanted)
            })
            // Two filters narrow together, and the values of one filter are alternatives
            // for the property it names.
            .filter(|record| {
                filters
                    .iter()
                    .all(|filter| record.matches_filter(&filter.key, &filter.values))
            });

        let mut summaries = Vec::new();
        let mut next_token = None;
        let mut resuming = resume_from.is_some();
        for record in selected {
            if resuming {
                if resume_from.is_some_and(|from| from != record.association_id) {
                    continue;
                }
                resuming = false;
            }
            if summaries.len() == page_size {
                // The page is full and an association is still waiting: hand the caller
                // the id it resumes at, so the next request continues where this one
                // stopped.
                next_token = Some(record.association_id.clone());
                break;
            }
            summaries.push(record.to_summary());
        }
        if resuming {
            return Err(invalid_parameter(
                "NextToken",
                "The specified NextToken isn't valid. Verify the token and try again."
                    .to_string(),
            ));
        }

        Ok(ListOpsItemRelatedItemsOutput {
            summaries: Some(summaries),
            next_token,
        })
    }
}
