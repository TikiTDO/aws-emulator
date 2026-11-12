// SPDX-License-Identifier: CC0-1.0

//! @op GetActionRecommendations
//!
//! ## How this handler answers
//!
//! "Returns a list of recommended actions in sorted in descending order by prediction
//! score. Use the `GetActionRecommendations` API if you have a custom campaign that
//! deploys a solution version trained with a PERSONALIZED_ACTIONS recipe." Every
//! action id and score comes from the registry in [`crate::state`]:
//!
//! 1. `campaignArn` is resolved against the registry (400 "does not exist or is not
//!    active yet" when it resolves to nothing — see
//!    [`crate::state::Denied::unresolvable`]) and the
//!    campaign must deploy a `PERSONALIZED_ACTIONS` solution version — otherwise 400.
//!    `numResults` and the maximum it has to satisfy ("The default is 5. The maximum
//!    is 100.") are facts about the REQUEST, so that bound is settled before the
//!    campaign is resolved.
//! 2. `userId` is the personalization input the RESOLVED campaign needs, not a fact
//!    about the request: the model marks it optional, so its absence is answered only
//!    once the campaign is in hand, and the refusal names that campaign.
//! 3. `filterArn` is resolved the same way (400 when unknown) and its expression, bound to
//!    `filterValues`, decides which stored Actions-catalog entries come back — not
//!    what any of them scores.
//! 4. Each action's score is [`crate::state::scoring::Seed::relevance`] of its STORED
//!    `base_score` and the request's `userId`, so an identical request returns an
//!    identical ordering (the operation is `@idempotent`), and the list is reported
//!    descending by score. The scores are normalized across the whole stored Actions
//!    catalog BEFORE the request's filter and `numResults` decide which of them come
//!    back, so one action's score is the same whatever the request asked for.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetActionRecommendations`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/smithy-model/com-amazonaws-personalizeruntime-amazonpersonalizeruntime.json")
//! ```
//!
//! Returns a list of recommended actions in sorted in descending order by prediction score.
//! Use the GetActionRecommendations API if you have a custom campaign that deploys a
//! solution version trained with a PERSONALIZED_ACTIONS recipe. For more information about
//! PERSONALIZED_ACTIONS recipes, see PERSONALIZED_ACTIONS recipes. For more information
//! about getting action recommendations, see Getting action recommendations.
//!
//! ## Input
//!
//! * `campaignArn` (Arn, len 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$) — The
//!   Amazon Resource Name (ARN) of the campaign to use for getting action recommendations.
//!   This campaign must deploy a solution version trained with a PERSONALIZED_ACTIONS recipe.
//! * `filterArn` (Arn, len 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$) — The ARN
//!   of the filter to apply to the returned recommendations. For more information, see
//!   Filtering Recommendations. When using this parameter, be sure the filter resource is
//!   ACTIVE.
//! * `filterValues` (FilterValues, len 0..25) — The values to use when filtering
//!   recommendations. For each placeholder parameter in your filter expression, provide the
//!   parameter name (in matching case) as a key and the filter value(s) as the corresponding
//!   value. Separate multiple values for one parameter with a comma. For filter expressions
//!   that use an INCLUDE element to include actions, you must provide values for all
//!   parameters that are defined in the expression. For filters with expressions that use an
//!   EXCLUDE element to exclude actions, you can omit the filter-values. In this case, Amazon
//!   Personalize doesn't use that portion of the expression to filter recommendations. For
//!   more information, see Filtering recommendations and user …
//! * `numResults` (NumResults, range 0.., default 0) **required** — The number of results to
//!   return. The default is 5. The maximum is 100.
//! * `userId` (UserID, len 0..256) — The user ID of the user to provide action
//!   recommendations for.
//!
//! ## Output
//!
//! * `actionList` (ActionList) — A list of action recommendations sorted in descending order
//!   by prediction score. There can be a maximum of 100 actions in the list. For information
//!   about action scores, see How action recommendation scoring works.
//! * `recommendationId` (RecommendationID) — The ID of the recommendation.
//!
//! ## Errors
//!
//! * `InvalidInputException` (HTTP 400) — Provide a valid value for the field or parameter.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource does not exist.

#[allow(unused_imports)]
use super::types::*;

use std::time::SystemTime;

use crate::state::{self as registry, scoring, Action, Denied, Entity, RecipeType, RequiredInput};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetActionRecommendationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-personalizeruntime-amazonpersonalizeruntime.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetActionRecommendationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.personalizeruntime#GetActionRecommendations")]
#[derive(Debug)]
pub enum GetActionRecommendationsErrorModeled {
    InvalidInputException(InvalidInputException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_action_recommendations::GetActionRecommendationsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetActionRecommendationsError = crate::scaffolded::Scaffolded<GetActionRecommendationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetActionRecommendationsErrorModeled> for GetActionRecommendationsError {
    fn from(modeled: GetActionRecommendationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetActionRecommendations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetActionRecommendations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetActionRecommendations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetActionRecommendations<Eng>
{
    type Input = GetActionRecommendationsInput;
    type Output = GetActionRecommendationsOutput;
    type Error = GetActionRecommendationsError;

    const NAME: &'static str = "GetActionRecommendations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ACTION_RECOMMENDATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_ACTION_RECOMMENDATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(|| registry::registry_unavailable("GetActionRecommendations"))?;
        let now = SystemTime::now();
        let mut guard = state.lock();

        // 1. What the REQUEST alone settles, before anything is resolved.
        //
        //    `campaignArn` carries no `@required` in the model, but it is "the campaign
        //    to use for getting action recommendations" — a request that gives no
        //    campaign ARN gives this operation nothing to recommend from, and the
        //    refusal reports the ARN it WAS given (see [`invalid_campaign_arn`]).
        let Some(campaign_arn) = registry::present(input.campaign_arn.as_deref()) else {
            return Err(invalid_campaign_arn(input.campaign_arn.as_deref()));
        };
        //    How many results to compute is the value the caller sent against the
        //    documented maximum ("The default is 5. The maximum is 100."), both facts
        //    about the request — so it is answered BEFORE the campaign is resolved,
        //    exactly as the sibling operations settle their request-only bounds: a
        //    `numResults` past the bound is refused whatever the campaign turns out to
        //    be.
        let num_results = registry::effective_num_results(
            input.num_results,
            registry::DEFAULT_NUM_ACTION_RESULTS,
            registry::MAX_NUM_ACTION_RESULTS,
        )
        .map_err(deny)?;

        // 2. The campaign, and the recipe type this operation serves. WHICH operation
        //    serves a campaign is the registry's rule, stated once in
        //    [`RecipeType::operation`]: this handler asks it with its own name
        //    ([`RecipeType::served_by`]) rather than naming the recipe type it expects,
        //    so a recipe type added to the enum cannot leak into the wrong operation.
        let campaign = guard.campaign(campaign_arn).map_err(deny)?.clone();
        if !campaign.recipe_type.served_by(Self::NAME) {
            return Err(invalid(format!(
                "Campaign {} deploys a solution version trained with the {} recipe type; this \
                 campaign must deploy a solution version trained with a {} recipe.",
                campaign.arn,
                campaign.recipe_type.as_str(),
                RecipeType::PersonalizedActions.as_str()
            )));
        }

        // 3. The personalization input the RESOLVED campaign needs. `userId` carries no
        //    `@required` and its modelled length starts at 0, so nothing about the
        //    request alone makes its absence a fault: it is the input the resolved
        //    campaign's recipe type asks for
        //    ([`crate::state::Campaign::required_input`]), so it is evaluated once that
        //    campaign is in hand and the refusal names it — exactly as the sibling
        //    `GetRecommendations` reports the input ITS resolved resource needs.
        let user_id = registry::present(input.user_id.as_deref()).unwrap_or_default();
        if campaign.required_input() == RequiredInput::User && user_id.is_empty() {
            return Err(invalid(format!(
                "userId is required to get action recommendations from campaign {} ({}).",
                campaign.arn,
                campaign.recipe_type.as_str()
            )));
        }

        // 4. The request's filter, over the Actions dataset.
        let request_filter = guard
            .bind_filter(
                registry::present(input.filter_arn.as_deref()),
                Entity::Actions,
                input.filter_values.as_ref(),
            )
            .map_err(deny)?;

        // 5. Score every stored action, settle each one's share of the certainty over
        //    the whole stored Actions catalog, then let the request's filter and
        //    `numResults` decide which of them come back. The seed is composed by the
        //    same shared helper the sibling operations score through, without the
        //    `context` member this operation's model does not carry.
        let seed = scoring::Seed::without_context(&campaign.arn, user_id);
        let mut ranked: Vec<(&Action, f64)> = campaign
            .actions
            .iter()
            .map(|action| {
                let score = seed.relevance(action.base_score, &action.action_id);
                (action, score)
            })
            .collect();
        // The reported score of an action is the model's certainty for the (campaign,
        // userId, action) triple, so the denominator is the WHOLE scored catalog and is
        // settled before any member that only selects which actions come back — the
        // request's filter and `numResults`. A response that reports fewer actions
        // reports each of them with the same score a larger one would, and sums to at
        // most 1.0.
        scoring::normalize_by(&mut ranked, |(_, score)| score);
        // "the filter to apply to the returned recommendations" — it decides WHICH
        // actions come back, never what one of them scores.
        ranked.retain(|(action, _)| {
            request_filter
                .as_ref()
                .is_none_or(|filter| filter.retains(&action.attributes))
        });
        ranked.sort_by(|(left, left_score), (right, right_score)| {
            scoring::by_score_desc(
                (left.action_id.as_str(), *left_score),
                (right.action_id.as_str(), *right_score),
            )
        });
        ranked.truncate(num_results);

        let action_list: Vec<PredictedAction> = ranked
            .iter()
            .map(|(action, score)| PredictedAction {
                action_id: Some(action.action_id.clone()),
                score: Some(*score),
            })
            .collect();

        let recommendation_id = guard.issue_recommendation_id(&campaign.arn, now);
        Ok(GetActionRecommendationsOutput {
            action_list: Some(action_list),
            recommendation_id: Some(recommendation_id),
        })
    }
}

/// The refusal for a request whose `campaignArn` names no campaign at all.
///
/// The message is BUILT FROM THE REQUEST: it reports the campaign ARN the request
/// gave, and an absent member gave the JSON `null`. `campaignArn` is the only resource
/// this operation can recommend from and it carries no `@required` trait, so the
/// boundary cannot refuse its absence and this handler is where it is refused — as
/// `InvalidInputException` (HTTP 400), the invalid-input channel every unusable
/// resource reference on the inference path goes through. The sentence is the registry's
/// one invalid-reference sentence ([`Denied::invalid_arn`]), the same one a `campaignArn`
/// that references another resource type earns.
fn invalid_campaign_arn(given: Option<&str>) -> GetActionRecommendationsError {
    deny(Denied::invalid_arn(
        registry::ResourceKind::Campaign,
        given.unwrap_or("null"),
    ))
}

/// The modelled `InvalidInputException` (HTTP 400): "Provide a valid value for the
/// field or parameter."
fn invalid(message: impl Into<String>) -> GetActionRecommendationsError {
    GetActionRecommendationsErrorModeled::InvalidInputException(InvalidInputException {
        message: Some(message.into()),
    })
    .into()
}

/// Lift a registry refusal into this operation's error channel.
///
/// Every refusal the registry produces is the invalid-input channel (see
/// [`Denied`]), so this is where that channel's HTTP 400 is chosen for this operation.
fn deny(denied: Denied) -> GetActionRecommendationsError {
    invalid(denied.message())
}
