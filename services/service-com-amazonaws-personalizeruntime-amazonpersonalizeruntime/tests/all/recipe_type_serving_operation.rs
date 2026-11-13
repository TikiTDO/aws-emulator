// SPDX-License-Identifier: CC0-1.0

//! Which runtime operation serves a campaign, over EVERY (recipe type, operation)
//! pair.
//!
//! The rule: a campaign is served by exactly the one runtime operation its recipe
//! type names (`RecipeType::operation`), and the other two refuse it with
//! `InvalidInputException` (HTTP 400). Each of the three handlers asks that one helper
//! with its own `Operation::NAME`, so this suite drives the shared decision through
//! all three of them.
//!
//! Every row's expectation is COMPUTED from `RecipeType::operation()` rather than
//! written out per row, so the table says nothing the registry's own rule does not,
//! and a recipe type added to the enum extends the table instead of contradicting it.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, RecipeType,
};

use crate::support::{
    actions, items, recommendation_id, Emulator, ACTION_RECOMMENDATIONS, PERSONALIZE_RANKING,
    RECOMMENDATIONS,
};

/// Every recipe type a campaign can carry. Source: the `RecipeType` variants, which
/// name the recipe types the three operations' docs name
/// (`smithy_query(query="doc",shape="GetPersonalizedRanking")` and
/// `smithy_query(query="doc",shape="GetActionRecommendations")`).
const RECIPE_TYPES: [RecipeType; 4] = [
    RecipeType::UserPersonalization,
    RecipeType::RelatedItems,
    RecipeType::PersonalizedRanking,
    RecipeType::PersonalizedActions,
];

/// The three runtime operations, each as `(Operation::NAME, its modelled route)`.
const OPERATIONS: [(&str, &str); 3] = [
    ("GetRecommendations", RECOMMENDATIONS),
    ("GetPersonalizedRanking", PERSONALIZE_RANKING),
    ("GetActionRecommendations", ACTION_RECOMMENDATIONS),
];

/// How many entries each registered catalog holds.
const CATALOG_SIZE: usize = 8;

/// The campaign ARN a recipe type's registered campaign is keyed by.
fn campaign_arn(recipe_type: RecipeType) -> String {
    format!(
        "arn:aws:personalize:us-east-1:000000000000:campaign/{}",
        recipe_type.as_str().to_lowercase()
    )
}

/// An emulator holding one registered campaign per recipe type, each with the catalog
/// its recipe type recommends from.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    for recipe_type in RECIPE_TYPES {
        let arn = campaign_arn(recipe_type);
        let campaign = match recipe_type {
            RecipeType::PersonalizedActions => {
                Campaign::new_actions(&arn, "demo", actions(CATALOG_SIZE))
            }
            _ => Campaign::new(&arn, "demo", recipe_type, items(CATALOG_SIZE)),
        };
        assert!(
            registry::register_campaign(&*emulator.engine, campaign),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    emulator
}

/// A request that is well formed on its OWN terms for `operation`, naming `arn`:
/// every member the operation needs whatever the campaign turns out to be, plus both
/// personalization inputs, so the only thing left to settle is which operation serves
/// the campaign.
fn request(operation: &str, arn: &str) -> Value {
    match operation {
        "GetRecommendations" => json!({
            "campaignArn": arn,
            "userId": "user-1",
            "itemId": "item-1",
        }),
        "GetPersonalizedRanking" => json!({
            "campaignArn": arn,
            "userId": "user-1",
            "inputList": ["item-1", "item-2"],
        }),
        _ => json!({ "campaignArn": arn, "userId": "user-1" }),
    }
}

/// The response list member an operation reports its entries in.
fn list_member(operation: &str) -> &'static str {
    match operation {
        "GetRecommendations" => "itemList",
        "GetPersonalizedRanking" => "personalizedRanking",
        _ => "actionList",
    }
}

#[tokio::test]
async fn exactly_the_operation_a_recipe_type_names_serves_its_campaign() {
    let emulator = seeded();

    for recipe_type in RECIPE_TYPES {
        let arn = campaign_arn(recipe_type);
        for (operation, route) in OPERATIONS {
            let case = format!("{} campaign through {operation}", recipe_type.as_str());
            let body = request(operation, &arn);
            if recipe_type.operation() == operation {
                let response = emulator.ok(route, body).await;
                let entries = response
                    .get(list_member(operation))
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case}: expected a {} member, got {response}",
                            list_member(operation)
                        )
                    });
                assert!(
                    !entries.is_empty(),
                    "case {case}: the serving operation must report entries from the \
                     registered catalog, got {response}"
                );
                let _ = recommendation_id(&case, &response);
            } else {
                let refusal = emulator.refusal(route, body).await;
                assert_eq!(
                    (refusal.status, refusal.code.as_str()),
                    (400, "InvalidInputException"),
                    "case {case}: an operation that does not serve this recipe type must \
                     refuse it, got {refusal:?}"
                );
                assert!(
                    refusal.message.contains(recipe_type.as_str()),
                    "case {case}: the refusal must name the recipe type the campaign \
                     deploys, got {:?}",
                    refusal.message
                );
            }
        }
    }
}

#[tokio::test]
async fn an_unregistered_campaign_is_unresolvable_for_every_operation() {
    // The serving decision is reached only once the campaign resolves, so an ARN the
    // registry does not hold earns the unresolvable refusal from all three operations
    // — the same 400 channel, with the message that names the ARN.
    let emulator = seeded();
    let arn = "arn:aws:personalize:us-east-1:000000000000:campaign/never-created";

    for (operation, route) in OPERATIONS {
        let case = format!("unregistered campaign through {operation}");
        let refusal = emulator.refusal(route, request(operation, arn)).await;
        assert_eq!(
            (refusal.status, refusal.code.as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException, got {refusal:?}"
        );
        assert!(
            refusal.message.contains(arn),
            "case {case}: the refusal must name the ARN it could not resolve, got {:?}",
            refusal.message
        );
    }
}
