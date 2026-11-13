// SPDX-License-Identifier: CC0-1.0

//! @op GetPersonalizedRanking
//!
//! ## How this handler answers
//!
//! "Re-ranks a list of recommended items for the given user. The first item in the
//! list is deemed the most likely item to be of interest to the user." So the
//! response is the REQUEST'S OWN `inputList`, re-ordered — never a list this handler
//! chooses:
//!
//! 1. What the REQUEST alone settles is checked FIRST, before anything is resolved:
//!    `inputList`'s length against the bound the same request fixes ("If you are
//!    including metadata in recommendations, the maximum is 50. Otherwise, the maximum
//!    is 500." — a longer list is 400 `Length of 'inputList' cannot be greater than
//!    <max>`), the `@required` `userId`, and the `metadataColumns` rules that read
//!    nothing else ("The map key is `ITEMS`", "The maximum number of columns you can
//!    provide is 10" — see [`crate::state::requested_metadata_columns`]).
//! 2. `campaignArn` is resolved against the registry in [`crate::state`] (400 "does
//!    not exist or is not active yet" when it resolves to nothing — see
//!    [`crate::state::Denied::unresolvable`]), and the campaign must deploy a
//!    `PERSONALIZED_RANKING` solution
//!    version ("The solution backing the campaign must have been created using a
//!    recipe of type PERSONALIZED_RANKING") — otherwise 400.
//! 3. Each `inputList` entry is looked up in the campaign's STORED Items catalog. An
//!    entry the catalog holds is scored by [`crate::state::scoring::Seed::relevance`] from
//!    its stored `base_score` and the request's `userId`/`context`; an entry the
//!    catalog does not hold "is appended to the end of the reranked list", with no
//!    relevance score for the user. The reported score is the entry's share of the
//!    certainty over the WHOLE stored catalog scored for this request, so an item
//!    carries the same score however long `inputList` is and whatever `filterArn`
//!    removed — a ranking of part of the catalog simply sums to less than 1.0.
//! 4. With no `filterArn`, the result is therefore a PERMUTATION of the DISTINCT ids
//!    `inputList` names, ordered by score descending. `InputList` carries no
//!    `@uniqueItems`, so a caller may list an id twice; both occurrences name the one
//!    catalog entry, whose share of the certainty the ranking already reports, so the
//!    repeat adds no entry (a repeated id reported twice would report its share twice
//!    and the reported scores would sum past the one unit of certainty the scored
//!    catalog apportions). With a `filterArn` ("a filter you created to include items
//!    or exclude items from recommendations for a given user") the entries the filter
//!    removes are not in the ranking.
//! 5. `metadataColumns` is served from the stored item attributes, for EVERY entry
//!    the response reports: with no `metadataColumns` the `metadata` member is
//!    omitted entirely, and with `metadataColumns` it is present on every entry,
//!    holding only the requested columns that entry stores — so an appended entry,
//!    which stores none, reports an empty map rather than no member.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetPersonalizedRanking`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/smithy-model/com-amazonaws-personalizeruntime-amazonpersonalizeruntime.json")
//! ```
//!
//! Re-ranks a list of recommended items for the given user. The first item in the list is
//! deemed the most likely item to be of interest to the user. The solution backing the
//! campaign must have been created using a recipe of type PERSONALIZED_RANKING.
//!
//! ## Input
//!
//! * `campaignArn` (Arn, len 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$)
//!   **required** — The Amazon Resource Name (ARN) of the campaign to use for generating the
//!   personalized ranking.
//! * `context` (Context, len 0..150) — The contextual metadata to use when getting
//!   recommendations. Contextual metadata includes any interaction information that might be
//!   relevant when getting a user's recommendations, such as the user's current location or
//!   device type.
//! * `filterArn` (Arn, len 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$) — The
//!   Amazon Resource Name (ARN) of a filter you created to include items or exclude items
//!   from recommendations for a given user. For more information, see Filtering
//!   Recommendations.
//! * `filterValues` (FilterValues, len 0..25) — The values to use when filtering
//!   recommendations. For each placeholder parameter in your filter expression, provide the
//!   parameter name (in matching case) as a key and the filter value(s) as the corresponding
//!   value. Separate multiple values for one parameter with a comma. For filter expressions
//!   that use an INCLUDE element to include items, you must provide values for all parameters
//!   that are defined in the expression. For filters with expressions that use an EXCLUDE
//!   element to exclude items, you can omit the filter-values.In this case, Amazon
//!   Personalize doesn't use that portion of the expression to filter recommendations. For
//!   more information, see Filtering Recommendations.
//! * `inputList` (InputList) **required** — A list of items (by itemId) to rank. If an item
//!   was not included in the training dataset, the item is appended to the end of the
//!   reranked list. If you are including metadata in recommendations, the maximum is 50.
//!   Otherwise, the maximum is 500.
//! * `metadataColumns` (MetadataColumns, len 0..1) — If you enabled metadata in
//!   recommendations when you created or updated the campaign, specify metadata columns from
//!   your Items dataset to include in the personalized ranking. The map key is ITEMS and the
//!   value is a list of column names from your Items dataset. The maximum number of columns
//!   you can provide is 10. For information about enabling metadata for a campaign, see
//!   Enabling metadata in recommendations for a campaign.
//! * `userId` (UserID, len 0..256) **required** — The user for which you want the campaign to
//!   provide a personalized ranking.
//!
//! ## Output
//!
//! * `personalizedRanking` (ItemList) — A list of items in order of most likely interest to
//!   the user. The maximum is 500.
//! * `recommendationId` (RecommendationID) — The ID of the recommendation.
//!
//! ## Errors
//!
//! * `InvalidInputException` (HTTP 400) — Provide a valid value for the field or parameter.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource does not exist.

#[allow(unused_imports)]
use super::types::*;

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use crate::state::{self as registry, scoring, Denied, Entity, RecipeType};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetPersonalizedRankingError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-personalizeruntime-amazonpersonalizeruntime.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetPersonalizedRankingError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.personalizeruntime#GetPersonalizedRanking")]
#[derive(Debug)]
pub enum GetPersonalizedRankingErrorModeled {
    InvalidInputException(InvalidInputException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_personalized_ranking::GetPersonalizedRankingError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetPersonalizedRankingError = crate::scaffolded::Scaffolded<GetPersonalizedRankingErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetPersonalizedRankingErrorModeled> for GetPersonalizedRankingError {
    fn from(modeled: GetPersonalizedRankingErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetPersonalizedRanking` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetPersonalizedRanking<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetPersonalizedRanking<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetPersonalizedRanking<Eng>
{
    type Input = GetPersonalizedRankingInput;
    type Output = GetPersonalizedRankingOutput;
    type Error = GetPersonalizedRankingError;

    const NAME: &'static str = "GetPersonalizedRanking";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PERSONALIZED_RANKING_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PERSONALIZED_RANKING_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // 0. Whether this CALLER may make the call at all — the decision AWS takes in front
        //    of the handler, on the action this operation names and on every resource ARN
        //    the request names ([`crate::state::authorization`]). It is taken before
        //    anything is read, so a refused request consults no registry entry and tells
        //    its caller nothing about one.
        if let Some(refusal) = crate::state::authorization_refusal(
            &*self.engine,
            ctx,
            Self::NAME,
            &referenced_resources(&input),
        ) {
            return Err(GetPersonalizedRankingError::access_denied(refusal));
        }

        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(|| registry::registry_unavailable("GetPersonalizedRanking"))?;
        let now = SystemTime::now();
        let mut guard = state.lock();

        // 1. What the REQUEST alone settles, before the campaign is resolved.
        //
        //    `inputList`'s length is a fact about the REQUEST — "If you are including
        //    metadata in recommendations, the maximum is 50. Otherwise, the maximum is
        //    500." — so it is checked BEFORE the campaign is resolved: a list past the
        //    bound is refused whatever the campaign turns out to be.
        let max_input_list = registry::metadata_bounded_max(registry::asks_for_metadata(
            input.metadata_columns.as_ref(),
        ));
        if input.input_list.len() > max_input_list {
            return Err(deny(registry::too_long("inputList", max_input_list)));
        }
        //    `userId` is a fact about the request too: this operation serves
        //    `PERSONALIZED_RANKING` campaigns only, so the personalization input it
        //    needs never depends on what the ARN resolves to.
        if input.user_id.is_empty() {
            return Err(invalid(
                "userId is required: it is the user the campaign provides a personalized ranking \
                 for.",
            ));
        }
        //    So are the `metadataColumns` rules that need nothing but the request: the
        //    map key, and how many columns it may name.
        let requested_metadata =
            registry::requested_metadata_columns(input.metadata_columns.as_ref()).map_err(deny)?;

        // 2. The campaign, and the recipe type this operation serves. `campaignArn` is
        //    `@required` and targets `Arn`, whose `@pattern` no empty or blank string
        //    satisfies, so the boundary has already refused anything that names no
        //    campaign at all: every value reaching this line is an ARN the boundary
        //    admitted, which the registry either resolves or reports unresolvable.
        //    WHICH operation serves a campaign is the registry's rule, stated once in
        //    [`RecipeType::operation`] and asked here with this operation's own name
        //    ([`RecipeType::served_by`]).
        let campaign = guard.campaign(&input.campaign_arn).map_err(deny)?.clone();
        if !campaign.recipe_type.served_by(Self::NAME) {
            return Err(invalid(format!(
                "Campaign {} deploys a solution version trained with the {} recipe type; the \
                 solution backing a GetPersonalizedRanking campaign must have been created using \
                 a recipe of type {}.",
                campaign.arn,
                campaign.recipe_type.as_str(),
                RecipeType::PersonalizedRanking.as_str()
            )));
        }

        // 3. Metadata, validated against the resolved campaign and its catalog — the
        //    parts of `metadataColumns` that need it; its map key and column cap were
        //    already settled in step 1, from the request.
        let metadata_columns = registry::metadata_columns_in_catalog(
            requested_metadata,
            campaign.metadata_enabled,
            &registry::catalog_columns(&campaign.items),
        )
        .map_err(deny)?;

        let request_filter = guard
            .bind_filter(
                registry::present(input.filter_arn.as_deref()),
                Entity::Items,
                input.filter_values.as_ref(),
            )
            .map_err(deny)?;

        // 4. Score the WHOLE stored catalog for this request, and settle each entry's
        //    share of the certainty over it. The reported score of an item is the
        //    model's certainty for the (campaign, userId, context, item) tuple, so the
        //    denominator is the catalog — never the entries this `inputList` happened
        //    to name, nor the ones the request's `filterArn` kept. Re-ranking the same
        //    user over a longer list, or with a filter, therefore reports an item with
        //    the same score, and a ranking of part of the catalog sums to at most 1.0.
        let seed = scoring::Seed::with_context(
            &campaign.arn,
            &input.user_id,
            input.context.as_ref(),
        );
        let mut catalog_shares: Vec<f64> = campaign
            .items
            .iter()
            .map(|item| seed.relevance(item.base_score, &item.item_id))
            .collect();
        scoring::normalize(&mut catalog_shares);

        // 5. Report the entries `inputList` names, and collect the ones the catalog
        //    does not hold. An entry with no catalog record carries no stored
        //    attributes, so the request's filter decides on an empty attribute set: an
        //    INCLUDE filter cannot keep it, an EXCLUDE filter cannot remove it.
        //
        //    Each DISTINCT id is reported once. An id the caller listed again names the
        //    same entry, and an entry's reported score is its share of the one unit of
        //    certainty spread over the scored catalog, so reporting it a second time
        //    would hand out that share twice and the ranking would sum past 1.0. The
        //    FIRST occurrence is the one kept, so repetition changes neither which ids
        //    come back nor the order a tie between them resolves in.
        let no_attributes: BTreeMap<String, String> = BTreeMap::new();
        let mut ranked: Vec<(usize, f64, usize)> = Vec::new();
        let mut appended: Vec<String> = Vec::new();
        let mut reported: BTreeSet<&str> = BTreeSet::new();
        for (position, item_id) in input.input_list.iter().enumerate() {
            if !reported.insert(item_id.as_str()) {
                continue;
            }
            match campaign
                .items
                .iter()
                .position(|item| &item.item_id == item_id)
            {
                Some(index) => {
                    let item = &campaign.items[index];
                    if request_filter
                        .as_ref()
                        .is_some_and(|filter| !filter.retains(&item.attributes))
                    {
                        continue;
                    }
                    ranked.push((index, catalog_shares[index], position));
                }
                None => {
                    if request_filter
                        .as_ref()
                        .is_some_and(|filter| !filter.retains(&no_attributes))
                    {
                        continue;
                    }
                    appended.push(item_id.clone());
                }
            }
        }
        // Score descending through the one comparator all three operations share;
        // `inputList` order is this operation's tie-break key, so an equal pair keeps
        // the order the caller sent and the ranking stays stable across identical calls.
        ranked.sort_by(|left, right| scoring::by_score_desc((left.2, left.1), (right.2, right.1)));

        let mut personalized_ranking: Vec<PredictedItem> = ranked
            .iter()
            .map(|(index, score, _)| {
                let item = &campaign.items[*index];
                PredictedItem {
                    item_id: Some(item.item_id.clone()),
                    score: Some(*score),
                    promotion_name: None,
                    metadata: registry::metadata_for(&item.attributes, metadata_columns.as_deref()),
                    reason: None,
                }
            })
            .collect();
        // "If an item was not included in the training dataset, the item is appended
        // to the end of the reranked list" — with no relevance score for the user, so
        // the list stays ordered by score descending.
        // Whether `metadata` is reported is a fact about the REQUEST, so an appended
        // entry is reported through the same helper, with the empty attribute set it
        // stores: a request that asked for metadata gets the member on every entry,
        // holding only the requested columns the entry actually carries — none, here.
        personalized_ranking.extend(appended.into_iter().map(|item_id| PredictedItem {
            item_id: Some(item_id),
            score: Some(0.0),
            promotion_name: None,
            metadata: registry::metadata_for(&no_attributes, metadata_columns.as_deref()),
            reason: None,
        }));

        let recommendation_id = guard.issue_recommendation_id(&campaign.arn, now);
        Ok(GetPersonalizedRankingOutput {
            personalized_ranking: Some(personalized_ranking),
            recommendation_id: Some(recommendation_id),
        })
    }
}

/// Every resource ARN this request NAMES.
///
/// The authorization decision is taken over all of them and nothing else: the campaign the
/// action is performed on (the resource type `personalize:GetPersonalizedRanking` is
/// published for) and the request's own filter. Read from the REQUEST, so the decision is
/// independent of whether either exists — which is what keeps a refused call from telling an
/// unauthorized caller whether one does.
fn referenced_resources(input: &GetPersonalizedRankingInput) -> Vec<&str> {
    let mut referenced: Vec<&str> = Vec::new();
    referenced.extend(registry::present(Some(input.campaign_arn.as_str())));
    referenced.extend(registry::present(input.filter_arn.as_deref()));
    referenced
}

/// The modelled `InvalidInputException` (HTTP 400): "Provide a valid value for the
/// field or parameter."
fn invalid(message: impl Into<String>) -> GetPersonalizedRankingError {
    GetPersonalizedRankingErrorModeled::InvalidInputException(InvalidInputException {
        message: Some(message.into()),
    })
    .into()
}

/// Lift a registry refusal into this operation's error channel.
///
/// Every refusal the registry produces is the invalid-input channel (see
/// [`Denied`]), so this is where that channel's HTTP 400 is chosen for this operation.
fn deny(denied: Denied) -> GetPersonalizedRankingError {
    invalid(denied.message())
}
