// SPDX-License-Identifier: CC0-1.0

//! @op GetRecommendations
//!
//! ## How this handler answers
//!
//! Everything it returns is read from the inference registry in [`crate::state`]:
//!
//! 1. **Settle what the REQUEST alone decides.** A refusal whose every input is the
//!    request is `InvalidInputException` (400) naming the member the caller got wrong,
//!    BEFORE anything is resolved, so one malformed request earns one refusal whatever
//!    the registry holds. That covers `numResults` against the bound the same request
//!    fixes ("The default is 25. If you are including metadata in recommendations, the
//!    maximum is 50. Otherwise, the maximum is 500."), the `metadataColumns` rules that
//!    read nothing else ("The map key is `ITEMS`", "The maximum number of columns you
//!    can provide is 10" — see [`crate::state::requested_metadata_columns`]), and the
//!    `promotions` members a promoted subset cannot be defined without.
//! 2. **Resolve the resource.** Exactly one of `campaignArn` / `recommenderArn`
//!    identifies what to recommend from; an ARN whose own resource part names another
//!    resource type references no such resource at all, which is
//!    `InvalidInputException` (400), "The given … ARN is invalid" (see
//!    [`crate::state::Denied::invalid_arn`]), and an ARN of the right type the registry
//!    does not hold is `InvalidInputException` (400), "does not exist or is not active
//!    yet" (see
//!    [`crate::state::Denied::unresolvable`]). A campaign whose solution version was
//!    trained with `PERSONALIZED_RANKING` or `PERSONALIZED_ACTIONS` is served by
//!    another operation, so it is `InvalidInputException` (400) here.
//! 3. **Check the personalization input the resolved resource needs** — "USER_PERSONALIZATION
//!    - userId required, itemId not used RELATED_ITEMS - itemId required, userId not
//!    used", and for a recommender the input its domain use case requires.
//! 4. **Filter.** `filterArn` is resolved against the registry (400 when unknown) and
//!    its expression, bound to `filterValues`, decides which stored catalog entries
//!    come back — not what any of them scores.
//! 5. **Score and order.** Each candidate's score is [`crate::state::scoring::Seed::relevance`]
//!    of its STORED `base_score`, the request's `context` and only the ONE
//!    personalization input the resolved resource uses (see
//!    [`crate::state::RequiredInput::personalization`]), so an identical request
//!    returns an identical ordering (the operation is `@idempotent`) and the input the
//!    resource does not use is ignored. The list is reported "sorted in descending
//!    order by prediction score". The scores are normalized across the whole stored catalog BEFORE the
//!    request's filter, `numResults` and any promotion decide which candidates come
//!    back, so one item's score is the same whatever the request asked for.
//! 6. **Promote.** A `promotions` entry takes `percentPromotedItems` of the returned
//!    slots from the entries its own filter selects; each such item carries
//!    `promotionName` and the documented `reason` "Promoted item". The share is taken
//!    of the entries the response actually REPORTS — `numResults` is what the caller
//!    asked for, and a catalog or filter that leaves fewer candidates reports fewer —
//!    so the promoted fraction of the list is the one the request named whatever the
//!    response's length turns out to be.
//! 7. **Metadata.** `metadataColumns` is served from the stored item attributes; with
//!    no `metadataColumns` the `metadata` member is omitted entirely.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetRecommendations`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/smithy-model/com-amazonaws-personalizeruntime-amazonpersonalizeruntime.json")
//! ```
//!
//! Returns a list of recommended items. For campaigns, the campaign's Amazon Resource Name
//! (ARN) is required and the required user and item input depends on the recipe type used
//! to create the solution backing the campaign as follows: USER_PERSONALIZATION - userId
//! required, itemId not used RELATED_ITEMS - itemId required, userId not used Campaigns
//! that are backed by a solution created using a recipe of type PERSONALIZED_RANKING use
//! the API. For recommenders, the recommender's ARN is required and the required item and
//! user input depends on the use case (domain-based recipe) backing the recommender. For
//! information on use case requirements see Choosing recommender use cases.
//!
//! ## Input
//!
//! * `campaignArn` (Arn, len 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$) — The
//!   Amazon Resource Name (ARN) of the campaign to use for getting recommendations.
//! * `context` (Context, len 0..150) — The contextual metadata to use when getting
//!   recommendations. Contextual metadata includes any interaction information that might be
//!   relevant when getting a user's recommendations, such as the user's current location or
//!   device type.
//! * `filterArn` (Arn, len 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$) — The ARN
//!   of the filter to apply to the returned recommendations. For more information, see
//!   Filtering Recommendations. When using this parameter, be sure the filter resource is
//!   ACTIVE.
//! * `filterValues` (FilterValues, len 0..25) — The values to use when filtering
//!   recommendations. For each placeholder parameter in your filter expression, provide the
//!   parameter name (in matching case) as a key and the filter value(s) as the corresponding
//!   value. Separate multiple values for one parameter with a comma. For filter expressions
//!   that use an INCLUDE element to include items, you must provide values for all parameters
//!   that are defined in the expression. For filters with expressions that use an EXCLUDE
//!   element to exclude items, you can omit the filter-values.In this case, Amazon
//!   Personalize doesn't use that portion of the expression to filter recommendations. For
//!   more information, see Filtering recommendations and user segments.
//! * `itemId` (ItemID, len 0..256) — The item ID to provide recommendations for. Required for
//!   RELATED_ITEMS recipe type.
//! * `metadataColumns` (MetadataColumns, len 0..1) — If you enabled metadata in
//!   recommendations when you created or updated the campaign or recommender, specify the
//!   metadata columns from your Items dataset to include in item recommendations. The map key
//!   is ITEMS and the value is a list of column names from your Items dataset. The maximum
//!   number of columns you can provide is 10. For information about enabling metadata for a
//!   campaign, see Enabling metadata in recommendations for a campaign. For information about
//!   enabling metadata for a recommender, see Enabling metadata in recommendations for a
//!   recommender.
//! * `numResults` (NumResults, range 0.., default 0) **required** — The number of results to
//!   return. The default is 25. If you are including metadata in recommendations, the maximum
//!   is 50. Otherwise, the maximum is 500.
//! * `promotions` (PromotionList, len 0..1) — The promotions to apply to the recommendation
//!   request. A promotion defines additional business rules that apply to a configurable
//!   subset of recommended items.
//! * `recommenderArn` (Arn, len 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$) — The
//!   Amazon Resource Name (ARN) of the recommender to use to get recommendations. Provide a
//!   recommender ARN if you created a Domain dataset group with a recommender for a domain
//!   use case.
//! * `userId` (UserID, len 0..256) — The user ID to provide recommendations for. Required for
//!   USER_PERSONALIZATION recipe type.
//!
//! ## Output
//!
//! * `itemList` (ItemList) — A list of recommendations sorted in descending order by
//!   prediction score. There can be a maximum of 500 items in the list.
//! * `recommendationId` (RecommendationID) — The ID of the recommendation.
//!
//! ## Errors
//!
//! * `InvalidInputException` (HTTP 400) — Provide a valid value for the field or parameter.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource does not exist.

#[allow(unused_imports)]
use super::types::*;

use std::time::SystemTime;

use crate::state::{
    self as registry, scoring, BoundFilter, Denied, Entity, Item, RequiredInput,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetRecommendationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-personalizeruntime-amazonpersonalizeruntime.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetRecommendationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.personalizeruntime#GetRecommendations")]
#[derive(Debug)]
pub enum GetRecommendationsErrorModeled {
    InvalidInputException(InvalidInputException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_recommendations::GetRecommendationsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetRecommendationsError = crate::scaffolded::Scaffolded<GetRecommendationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetRecommendationsErrorModeled> for GetRecommendationsError {
    fn from(modeled: GetRecommendationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetRecommendations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetRecommendations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetRecommendations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetRecommendations<Eng>
{
    type Input = GetRecommendationsInput;
    type Output = GetRecommendationsOutput;
    type Error = GetRecommendationsError;

    const NAME: &'static str = "GetRecommendations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_RECOMMENDATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_RECOMMENDATIONS_OUTPUT_SCHEMA;
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
            return Err(GetRecommendationsError::access_denied(refusal));
        }

        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(|| registry::registry_unavailable("GetRecommendations"))?;
        let now = SystemTime::now();
        let mut guard = state.lock();

        // 1. What the REQUEST alone settles, before anything is resolved: every
        //    refusal here measures a value the caller sent against a documented bound or
        //    against a member without which the request defines nothing, so a malformed
        //    request earns the clause naming that member whatever the registry holds.
        //
        //    How many results to compute. Both `numResults` and the maximum it has to
        //    satisfy — "If you are including metadata in recommendations, the maximum
        //    is 50. Otherwise, the maximum is 500." — are facts about the REQUEST: the
        //    bound is keyed off the same predicate `GetPersonalizedRanking` bounds
        //    `inputList` with, so one request means one bound in both operations.
        let max_num_results = registry::metadata_bounded_max(registry::asks_for_metadata(
            input.metadata_columns.as_ref(),
        ));
        let num_results = registry::effective_num_results(
            input.num_results,
            registry::DEFAULT_NUM_RESULTS,
            max_num_results,
        )
        .map_err(deny)?;
        //    The `metadataColumns` rules that need nothing but the request: the map key
        //    and how many columns it may name.
        let requested_metadata =
            registry::requested_metadata_columns(input.metadata_columns.as_ref()).map_err(deny)?;
        //    The `promotions` members themselves: a promotion missing one of them
        //    defines no promoted subset, which is a fact about the request.
        let requested_promotion = requested_promotion(input.promotions.as_deref())?;

        // 2. Resolve the campaign or recommender this request recommends from.
        let resolved = resolve(
            &guard,
            Self::NAME,
            registry::present(input.campaign_arn.as_deref()),
            registry::present(input.recommender_arn.as_deref()),
        )?;

        // 3. The personalization input the resolved resource needs.
        let user_id = registry::present(input.user_id.as_deref()).unwrap_or_default();
        let item_id = registry::present(input.item_id.as_deref()).unwrap_or_default();
        match resolved.required_input {
            RequiredInput::User if user_id.is_empty() => {
                return Err(invalid(format!(
                    "userId is required to get recommendations from {}.",
                    resolved.description
                )))
            }
            RequiredInput::Item if item_id.is_empty() => {
                return Err(invalid(format!(
                    "itemId is required to get recommendations from {}.",
                    resolved.description
                )))
            }
            _ => {}
        }

        // 4. Which metadata columns come back, validated against the RESOLVED resource
        //    and its catalog — the parts of `metadataColumns` that need it; the map key
        //    and the column cap were already settled in step 1, from the request.
        let metadata_columns = registry::metadata_columns_in_catalog(
            requested_metadata,
            resolved.metadata_enabled,
            &registry::catalog_columns(&resolved.items),
        )
        .map_err(deny)?;

        // 5. The request's filter, and the registry-bound filter of the promotion step
        //    1 already read off the request.
        let request_filter = guard
            .bind_filter(
                registry::present(input.filter_arn.as_deref()),
                Entity::Items,
                input.filter_values.as_ref(),
            )
            .map_err(deny)?;
        let promotion = bind_promotion(&guard, requested_promotion)?;

        // 6. Score the resolved resource's WHOLE stored catalog and settle each entry's
        //    share of the certainty over it. `RELATED_ITEMS` recommends items related
        //    to `itemId`, never the queried item itself, so that item is no candidate
        //    at all — `itemId` is a personalization input, not a selection member.
        //
        //    Only the personalization input the resolved resource USES reaches the
        //    score: "USER_PERSONALIZATION - userId required, itemId not used
        //    RELATED_ITEMS - itemId required, userId not used". So a spurious `itemId`
        //    on a user-personalization request, or a spurious `userId` on a
        //    related-items one, leaves the scored catalog — and therefore the items
        //    returned and their order — exactly as it was.
        let personalization = resolved.required_input.personalization(user_id, item_id);
        let seed = scoring::Seed::with_context(
            &resolved.scope,
            personalization,
            input.context.as_ref(),
        );
        let mut candidates: Vec<(usize, f64)> = resolved
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                !(resolved.required_input == RequiredInput::Item && item.item_id == item_id)
            })
            .map(|(index, item)| (index, seed.relevance(item.base_score, &item.item_id)))
            .collect();
        // The reported score of an item is the model's certainty for the resolved
        // resource, the request's personalization inputs and that item, so the
        // denominator is the WHOLE scored catalog and is settled before any member
        // that only selects which items come back — the request's filter, `numResults`
        // and a promotion. A response that reports fewer items reports each of them
        // with the same score a larger one would, and sums to at most 1.0.
        scoring::normalize_by(&mut candidates, |(_, score)| score);
        // "a filter you created to include items or exclude items from recommendations"
        // — it decides WHICH candidates come back, never what one of them scores.
        candidates.retain(|(index, _)| {
            request_filter
                .as_ref()
                .is_none_or(|filter| filter.retains(&resolved.items[*index].attributes))
        });
        candidates.sort_by(|(left, left_score), (right, right_score)| {
            scoring::by_score_desc(
                (&resolved.items[*left].item_id, *left_score),
                (&resolved.items[*right].item_id, *right_score),
            )
        });

        // 7. Fill the returned slots: the promotion's share first, then the rest of
        //    the ranking. Both are drawn in descending-score order, and the chosen
        //    positions are re-sorted so the response stays in that order.
        //
        //    How many slots the response REPORTS is settled here, before the promotion
        //    takes its share of them: `numResults` is what the request asked for, but a
        //    catalog (minus the item a related-items request queried) and a request
        //    filter that leave fewer candidates report fewer. The promotion's share is
        //    "the percentage of recommended items to apply the promotion to", so it is
        //    a percentage of the reported entries, not of the requested count.
        let reported = num_results.min(candidates.len());
        let mut chosen: Vec<(usize, bool)> = Vec::new();
        if let Some(promotion) = promotion.as_ref() {
            let slots = promoted_slots(reported, promotion.percent);
            for (position, (index, _)) in candidates.iter().enumerate() {
                if chosen.len() >= slots {
                    break;
                }
                if promotion.filter.retains(&resolved.items[*index].attributes) {
                    chosen.push((position, true));
                }
            }
        }
        for position in 0..candidates.len() {
            if chosen.len() >= reported {
                break;
            }
            if chosen.iter().any(|(taken, _)| *taken == position) {
                continue;
            }
            chosen.push((position, false));
        }
        chosen.sort_by_key(|(position, _)| *position);

        let item_list: Vec<PredictedItem> = chosen
            .iter()
            .map(|(position, promoted)| {
                let (index, score) = candidates[*position];
                let item = &resolved.items[index];
                PredictedItem {
                    item_id: Some(item.item_id.clone()),
                    score: Some(score),
                    promotion_name: promoted
                        .then(|| promotion.as_ref().map(|promotion| promotion.name.clone()))
                        .flatten(),
                    metadata: registry::metadata_for(&item.attributes, metadata_columns.as_deref()),
                    reason: promoted.then(|| vec![registry::REASON_PROMOTED_ITEM.to_string()]),
                }
            })
            .collect();

        let recommendation_id = guard.issue_recommendation_id(&resolved.scope, now);
        Ok(GetRecommendationsOutput {
            item_list: Some(item_list),
            recommendation_id: Some(recommendation_id),
        })
    }
}

/// Every resource ARN this request NAMES.
///
/// The authorization decision is taken over all of them and nothing else: the campaign or
/// recommender the action is performed on (the resource types
/// `personalize:GetRecommendations` is published for), the request's own filter, and the
/// filter of its promotion. Read from the REQUEST, so the decision is independent of whether
/// any of them exists — which is what keeps a refused call from telling an unauthorized
/// caller whether one does. An empty value names no resource at all and is left to the
/// refusals that report what is wrong with the request itself.
fn referenced_resources(input: &GetRecommendationsInput) -> Vec<&str> {
    let mut referenced: Vec<&str> = Vec::new();
    referenced.extend(registry::present(input.campaign_arn.as_deref()));
    referenced.extend(registry::present(input.recommender_arn.as_deref()));
    referenced.extend(registry::present(input.filter_arn.as_deref()));
    referenced.extend(
        input
            .promotions
            .iter()
            .flatten()
            .filter_map(|promotion| registry::present(promotion.filter_arn.as_deref())),
    );
    referenced
}

/// The refusal this operation states for a request that names NO inference target.
///
/// `campaignArn` and `recommenderArn` are alternatives and neither is `@required`, so a
/// request supplying neither reaches the handler and is refused here. The sentence is
/// FIXED text end to end: it names the two members the caller could have supplied and
/// substitutes nothing from the request or from state — which is what separates it from
/// [`crate::state::Denied::unresolvable`]'s sentence, the refusal a request that DID name
/// a target earns. Both carry the same code and status, so the message is the only
/// observable a client can discriminate the two conditions on.
const NO_INFERENCE_TARGET: &str = "campaignArn or recommenderArn required.";

/// The refusal this operation states for a request that names BOTH alternatives.
///
/// The other condition the two ARN members settle on their own, and likewise fixed text
/// end to end: the members are alternatives, so a request naming both identifies two
/// things to recommend from and the service states that without echoing either ARN.
const BOTH_INFERENCE_TARGETS: &str = "Cannot provide both a campaignArn and recommenderArn";

/// What the request's `campaignArn` or `recommenderArn` resolved to.
struct Resolved {
    /// The resolved ARN: the scope every score and the minted `recommendationId` are
    /// derived under, so two resources never share an ordering.
    scope: String,
    /// How the resource reads in a refusal message.
    description: String,
    /// The stored Items catalog it recommends from.
    items: Vec<Item>,
    /// Whether metadata in recommendations is enabled on it.
    metadata_enabled: bool,
    /// The personalization input it needs in the request.
    required_input: RequiredInput,
}

/// Resolve the request's `campaignArn` or `recommenderArn` against the registry.
///
/// "For campaigns, the campaign's Amazon Resource Name (ARN) is required" and "For
/// recommenders, the recommender's ARN is required": the two are ALTERNATIVES, so a
/// request naming neither identifies nothing to recommend from and one naming both is
/// contradictory.
///
/// `operation` is the caller's own `Operation::NAME`, which a resolved CAMPAIGN'S
/// recipe type has to be served by ([`crate::state::RecipeType::served_by`]): the
/// mapping from recipe type to operation is the registry's, so this function does not
/// re-state which recipe types it refuses.
fn resolve(
    state: &crate::state::ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState,
    operation: &str,
    campaign_arn: Option<&str>,
    recommender_arn: Option<&str>,
) -> Result<Resolved, GetRecommendationsError> {
    match (campaign_arn, recommender_arn) {
        (Some(_), Some(_)) => Err(invalid(BOTH_INFERENCE_TARGETS)),
        (None, None) => Err(invalid(NO_INFERENCE_TARGET)),
        (Some(arn), None) => {
            let campaign = state.campaign(arn).map_err(deny)?;
            if !campaign.recipe_type.served_by(operation) {
                return Err(invalid(format!(
                    "Campaign {arn} deploys a solution version trained with the {} recipe type; \
                     use the {} API.",
                    campaign.recipe_type.as_str(),
                    campaign.recipe_type.operation()
                )));
            }
            Ok(Resolved {
                scope: campaign.arn.clone(),
                description: format!(
                    "campaign {} ({})",
                    campaign.arn,
                    campaign.recipe_type.as_str()
                ),
                items: campaign.items.clone(),
                metadata_enabled: campaign.metadata_enabled,
                required_input: campaign.required_input(),
            })
        }
        (None, Some(arn)) => {
            let recommender = state.recommender(arn).map_err(deny)?;
            Ok(Resolved {
                scope: recommender.arn.clone(),
                description: format!("recommender {} ({})", recommender.arn, recommender.use_case),
                items: recommender.items.clone(),
                metadata_enabled: recommender.metadata_enabled,
                required_input: recommender.required_input(),
            })
        }
    }
}

/// The request's promotion, bound to the registry and to this request.
struct PreparedPromotion {
    /// `Promotion$name`, reported as `PredictedItem$promotionName`.
    name: String,
    /// `Promotion$percentPromotedItems`: the share of the REPORTED entries the
    /// promotion takes, applied once the response's length is known.
    percent: i32,
    /// The promotion's own filter: "This filter defines the criteria for promoted
    /// items."
    filter: BoundFilter,
}

/// The request's promotion, as the REQUEST states it: every member a promoted subset
/// needs, read off the request and not yet looked up anywhere.
struct RequestedPromotion<'a> {
    /// `Promotion$name`.
    name: &'a str,
    /// `Promotion$percentPromotedItems`.
    percent: i32,
    /// `Promotion$filterArn`, still to be resolved against the registry.
    filter_arn: &'a str,
    /// `Promotion$filterValues`, bound to that filter's placeholders.
    filter_values: Option<&'a std::collections::HashMap<String, String>>,
}

/// Read the request's promotion, if it carries one, from the REQUEST alone.
///
/// `PromotionList` is modelled `len 0..1`, so at most one promotion can arrive. A
/// promotion without a name, a percentage or a filter defines no promoted subset at
/// all, which is a refusal naming the member — and one that needs nothing but the
/// request, so it is settled before the campaign or recommender is resolved.
fn requested_promotion(
    promotions: Option<&[Promotion]>,
) -> Result<Option<RequestedPromotion<'_>>, GetRecommendationsError> {
    let Some(promotion) = promotions.and_then(<[Promotion]>::first) else {
        return Ok(None);
    };
    let Some(name) = registry::present(promotion.name.as_deref()) else {
        return Err(invalid("promotions[0].name is required."));
    };
    let Some(percent) = promotion.percent_promoted_items else {
        return Err(invalid("promotions[0].percentPromotedItems is required."));
    };
    let Some(filter_arn) = registry::present(promotion.filter_arn.as_deref()) else {
        return Err(invalid("promotions[0].filterArn is required."));
    };
    Ok(Some(RequestedPromotion {
        name,
        percent,
        filter_arn,
        filter_values: promotion.filter_values.as_ref(),
    }))
}

/// Bind the requested promotion's filter to the registry and to this request.
///
/// Everything left here needs the REGISTRY: "This filter defines the criteria for
/// promoted items", so an ARN the registry does not hold, or a `filterValues` binding
/// the stored expression cannot apply, is reported once the promotion's own members are
/// known to be well formed.
fn bind_promotion(
    state: &crate::state::ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState,
    requested: Option<RequestedPromotion<'_>>,
) -> Result<Option<PreparedPromotion>, GetRecommendationsError> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    let Some(filter) = state
        .bind_filter(
            Some(requested.filter_arn),
            Entity::Items,
            requested.filter_values,
        )
        .map_err(deny)?
    else {
        return Err(invalid("promotions[0].filterArn is required."));
    };
    Ok(Some(PreparedPromotion {
        name: requested.name.to_string(),
        percent: requested.percent,
        filter,
    }))
}

/// How many of the `reported` slots a promotion of `percent` percent takes.
///
/// `reported` is the number of entries the response actually carries, which is what
/// "the percentage of recommended items to apply the promotion to" is a percentage of:
/// a response shorter than the requested `numResults` gives the promotion a
/// proportionally smaller share, never the whole list.
///
/// Rounded UP, so a promotion the caller asked for is always applied to at least one
/// of the slots there are: `percentPromotedItems` is modelled `range 1..100`, so asking
/// for a promotion at all asks for a non-empty share. The rounding direction is
/// CHOSEN — AWS documents only the percentage itself.
fn promoted_slots(reported: usize, percent: i32) -> usize {
    let percent = percent.clamp(0, 100) as usize;
    reported.saturating_mul(percent).div_ceil(100)
}

/// The modelled `InvalidInputException` (HTTP 400): "Provide a valid value for the
/// field or parameter."
fn invalid(message: impl Into<String>) -> GetRecommendationsError {
    GetRecommendationsErrorModeled::InvalidInputException(InvalidInputException {
        message: Some(message.into()),
    })
    .into()
}

/// Lift a registry refusal into this operation's error channel.
///
/// Every refusal the registry produces is the invalid-input channel (see
/// [`Denied`]), so this is where that channel's HTTP 400 is chosen for this operation.
fn deny(denied: Denied) -> GetRecommendationsError {
    invalid(denied.message())
}
