// SPDX-License-Identifier: CC0-1.0

//! Which refusal a request earns when it is malformed on its OWN terms and also names
//! a resource that would be refused.
//!
//! The rule, over every inference operation: a refusal whose every input comes from the
//! REQUEST — the value the caller sent, measured against a documented bound or against
//! a member the model marks `@required` — is settled BEFORE the request's campaign or
//! recommender is resolved, so one malformed request earns one refusal whatever the
//! registry holds.
//! A refusal that needs the resolved resource (an ARN the registry does not hold, a
//! campaign of the wrong recipe type, the personalization input THAT resource needs)
//! only wins when the request is well formed on its own terms.
//!
//! The rows span the operations, the request-only members (`numResults`, `inputList`,
//! the `@required` `userId` of `GetPersonalizedRanking`, the `metadataColumns` map key
//! and column count, and the `promotions` members a promoted subset needs), the bound
//! each one is measured against — including the lower maximum a request that asks for
//! metadata gets — and the resource state the request names: unregistered,
//! registered-but-wrong-recipe, registered-but-missing-the-input, and a promotion whose
//! filter the registry does not hold.
//!
//! A member the model leaves OPTIONAL is not such a refusal: it is an input of the
//! resolved resource, so it is answered after resolution — see
//! `optional_input_refusal_order.rs`.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, RecipeType, MAX_METADATA_COLUMNS, MAX_NUM_ACTION_RESULTS,
    MAX_NUM_RESULTS, MAX_NUM_RESULTS_WITH_METADATA, METADATA_DATASET_ITEMS,
};

use crate::support::{
    actions, items, Emulator, ACTION_RECOMMENDATIONS, COLUMN_GENRE, PERSONALIZE_RANKING,
    RECOMMENDATIONS,
};

/// A registered `USER_PERSONALIZATION` campaign, with metadata in recommendations
/// enabled — what `GetRecommendations` serves, and the wrong recipe type for either
/// sibling operation.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/user-personalization";
/// A registered `RELATED_ITEMS` campaign, which needs `itemId` in the request.
const CAMPAIGN_RELATED: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/related-items";
/// A registered `PERSONALIZED_RANKING` campaign: the wrong recipe type for
/// `GetRecommendations`.
const CAMPAIGN_RANKING: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/ranking";
/// A registered `PERSONALIZED_ACTIONS` campaign.
const CAMPAIGN_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/actions";

/// A well-formed campaign ARN the registry does not hold.
const UNREGISTERED_CAMPAIGN: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/never-created";
/// A well-formed recommender ARN the registry does not hold.
const UNREGISTERED_RECOMMENDER: &str =
    "arn:aws:personalize:us-east-1:000000000000:recommender/never-created";
/// A well-formed filter ARN the registry does not hold — what a promotion that IS well
/// formed on the request's own terms still earns a refusal for.
const UNREGISTERED_FILTER: &str = "arn:aws:personalize:us-east-1:000000000000:filter/never-created";

/// How many entries the seeded catalogs hold.
const CATALOG_SIZE: usize = 8;

/// An emulator whose registry holds one campaign of every recipe type.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;
    let campaigns = [
        (CAMPAIGN_USER, RecipeType::UserPersonalization, true),
        (CAMPAIGN_RELATED, RecipeType::RelatedItems, false),
        (CAMPAIGN_RANKING, RecipeType::PersonalizedRanking, true),
    ];
    for (arn, recipe_type, metadata) in campaigns {
        let campaign = Campaign::new(arn, "demo", recipe_type, items(CATALOG_SIZE));
        let campaign = if metadata {
            campaign.with_metadata_enabled()
        } else {
            campaign
        };
        assert!(
            registry::register_campaign(engine, campaign),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new_actions(CAMPAIGN_ACTIONS, "actions", actions(CATALOG_SIZE))
        ),
        "the campaign seam must register the action campaign"
    );
    emulator
}

/// A resource state a request can name, and the refusal a request that is well formed
/// on its own terms earns from it.
struct Resource {
    /// How the row reads in an assertion message.
    case: &'static str,
    /// The resource members the request carries.
    members: Value,
    /// The resource-dependent refusal — the one the request-only refusal takes
    /// precedence over.
    refusal: String,
}

/// A way for a request to be malformed on its own terms, and the refusal it earns.
struct RequestOnly {
    /// How the row reads in an assertion message.
    case: &'static str,
    /// The request members that carry the violation.
    members: Value,
    /// The refusal the request earns from its own members alone.
    refusal: String,
}

/// The refusal for a resource ARN the registry cannot resolve.
fn unresolvable(arn: &str) -> String {
    format!("{arn} does not exist or is not active yet.")
}

/// The refusal for `numResults` past the maximum that applies to the request.
fn num_results_past(requested: usize, max: usize) -> String {
    format!(
        "Value '{requested}' at 'numResults' failed to satisfy constraint: Member must have value \
         less than or equal to {max}"
    )
}

/// `base` with every member of `patch` set on it.
fn merged(base: &Value, patch: &Value) -> Value {
    let mut body = base.clone();
    for (member, value) in patch.as_object().expect("a patch is a JSON object") {
        body[member.as_str()] = value.clone();
    }
    body
}

/// The refusal for a `metadataColumns` map key other than the documented one: "The map
/// key is `ITEMS`" (`GetRecommendationsRequest$metadataColumns` doc).
fn metadata_key_past(key: &str) -> String {
    format!(
        "Value '{key}' at 'metadataColumns' failed to satisfy constraint: the map key must be \
         {METADATA_DATASET_ITEMS}"
    )
}

/// The refusal for more `metadataColumns` than "The maximum number of columns you can
/// provide is 10." (`GetRecommendationsRequest$metadataColumns` doc).
fn metadata_columns_past(count: usize) -> String {
    format!(
        "Value '{count}' at 'metadataColumns' failed to satisfy constraint: at most \
         {MAX_METADATA_COLUMNS} columns can be provided"
    )
}

/// `count` distinct Items-dataset column names, for `metadataColumns`.
fn column_names(count: usize) -> Value {
    Value::from(
        (0..count)
            .map(|index| Value::from(format!("COLUMN_{index}")))
            .collect::<Vec<Value>>(),
    )
}

/// A one-entry `promotions` list carrying every member except `missing`, so the only
/// thing wrong with it is the member the refusal has to name.
fn promotion_without(missing: &str) -> Value {
    let mut promotion = json!({
        "name": "promo",
        "percentPromotedItems": 50,
        "filterArn": UNREGISTERED_FILTER,
    });
    promotion
        .as_object_mut()
        .expect("a promotion is a JSON object")
        .remove(missing)
        .unwrap_or_else(|| panic!("case {missing}: the promotion must carry the member to drop"));
    json!([promotion])
}

/// A list of `length` item ids, for `GetPersonalizedRanking$inputList`.
fn input_list(length: usize) -> Value {
    Value::from(
        (0..length)
            .map(|index| Value::from(format!("item-{index}")))
            .collect::<Vec<Value>>(),
    )
}

/// The resource states and the request-only violations of one operation, with the
/// well-formed base body every row is built on.
fn table(route: &str) -> (Value, Vec<Resource>, Vec<RequestOnly>) {
    match route {
        RECOMMENDATIONS => (
            json!({ "userId": "user-1" }),
            vec![
                Resource {
                    case: "an unregistered campaign",
                    members: json!({ "campaignArn": UNREGISTERED_CAMPAIGN }),
                    refusal: unresolvable(UNREGISTERED_CAMPAIGN),
                },
                Resource {
                    case: "an unregistered recommender",
                    members: json!({ "recommenderArn": UNREGISTERED_RECOMMENDER }),
                    refusal: unresolvable(UNREGISTERED_RECOMMENDER),
                },
                Resource {
                    case: "a campaign of a recipe type another operation serves",
                    members: json!({ "campaignArn": CAMPAIGN_RANKING }),
                    refusal: format!(
                        "Campaign {CAMPAIGN_RANKING} deploys a solution version trained with the \
                         PERSONALIZED_RANKING recipe type; use the GetPersonalizedRanking API."
                    ),
                },
                Resource {
                    case: "a related-items campaign the request gives no itemId for",
                    members: json!({ "campaignArn": CAMPAIGN_RELATED }),
                    refusal: format!(
                        "itemId is required to get recommendations from campaign \
                         {CAMPAIGN_RELATED} (RELATED_ITEMS)."
                    ),
                },
                Resource {
                    case: "a promotion whose filter the registry does not hold",
                    members: json!({
                        "campaignArn": CAMPAIGN_USER,
                        "promotions": [{
                            "name": "promo",
                            "percentPromotedItems": 50,
                            "filterArn": UNREGISTERED_FILTER,
                        }],
                    }),
                    refusal: unresolvable(UNREGISTERED_FILTER),
                },
            ],
            vec![
                RequestOnly {
                    case: "numResults one past its maximum",
                    members: json!({ "numResults": MAX_NUM_RESULTS + 1 }),
                    refusal: num_results_past(MAX_NUM_RESULTS + 1, MAX_NUM_RESULTS),
                },
                RequestOnly {
                    case: "numResults far past its maximum",
                    members: json!({ "numResults": MAX_NUM_RESULTS * 3 }),
                    refusal: num_results_past(MAX_NUM_RESULTS * 3, MAX_NUM_RESULTS),
                },
                RequestOnly {
                    case: "numResults one past the lower maximum a metadata request gets",
                    members: json!({
                        "numResults": MAX_NUM_RESULTS_WITH_METADATA + 1,
                        "metadataColumns": { "ITEMS": [COLUMN_GENRE] },
                    }),
                    refusal: num_results_past(
                        MAX_NUM_RESULTS_WITH_METADATA + 1,
                        MAX_NUM_RESULTS_WITH_METADATA,
                    ),
                },
                RequestOnly {
                    case: "a metadataColumns map key naming another dataset type",
                    members: json!({ "metadataColumns": { "INTERACTIONS": [COLUMN_GENRE] } }),
                    refusal: metadata_key_past("INTERACTIONS"),
                },
                RequestOnly {
                    case: "a metadataColumns map key that is the right word in the wrong case",
                    members: json!({ "metadataColumns": { "items": [COLUMN_GENRE] } }),
                    refusal: metadata_key_past("items"),
                },
                RequestOnly {
                    case: "one metadataColumns column past the documented maximum",
                    members: json!({
                        "metadataColumns": { "ITEMS": column_names(MAX_METADATA_COLUMNS + 1) },
                    }),
                    refusal: metadata_columns_past(MAX_METADATA_COLUMNS + 1),
                },
                RequestOnly {
                    case: "far more metadataColumns columns than the documented maximum",
                    members: json!({
                        "metadataColumns": { "ITEMS": column_names(MAX_METADATA_COLUMNS * 3) },
                    }),
                    refusal: metadata_columns_past(MAX_METADATA_COLUMNS * 3),
                },
                RequestOnly {
                    case: "a promotion with no name",
                    members: json!({ "promotions": promotion_without("name") }),
                    refusal: "promotions[0].name is required.".to_string(),
                },
                RequestOnly {
                    case: "a promotion with no percentPromotedItems",
                    members: json!({ "promotions": promotion_without("percentPromotedItems") }),
                    refusal: "promotions[0].percentPromotedItems is required.".to_string(),
                },
                RequestOnly {
                    case: "a promotion with no filterArn",
                    members: json!({ "promotions": promotion_without("filterArn") }),
                    refusal: "promotions[0].filterArn is required.".to_string(),
                },
            ],
        ),
        ACTION_RECOMMENDATIONS => (
            json!({ "userId": "user-1" }),
            vec![
                Resource {
                    case: "an unregistered campaign",
                    members: json!({ "campaignArn": UNREGISTERED_CAMPAIGN }),
                    refusal: unresolvable(UNREGISTERED_CAMPAIGN),
                },
                Resource {
                    case: "a campaign of a recipe type another operation serves",
                    members: json!({ "campaignArn": CAMPAIGN_USER }),
                    refusal: format!(
                        "Campaign {CAMPAIGN_USER} deploys a solution version trained with the \
                         USER_PERSONALIZATION recipe type; this campaign must deploy a solution \
                         version trained with a PERSONALIZED_ACTIONS recipe."
                    ),
                },
            ],
            vec![
                RequestOnly {
                    case: "numResults one past its maximum",
                    members: json!({ "numResults": MAX_NUM_ACTION_RESULTS + 1 }),
                    refusal: num_results_past(
                        MAX_NUM_ACTION_RESULTS + 1,
                        MAX_NUM_ACTION_RESULTS,
                    ),
                },
                RequestOnly {
                    case: "numResults far past its maximum",
                    members: json!({ "numResults": MAX_NUM_ACTION_RESULTS * 5 }),
                    refusal: num_results_past(
                        MAX_NUM_ACTION_RESULTS * 5,
                        MAX_NUM_ACTION_RESULTS,
                    ),
                },
            ],
        ),
        PERSONALIZE_RANKING => (
            json!({ "userId": "user-1", "inputList": ["item-1"] }),
            vec![
                Resource {
                    case: "an unregistered campaign",
                    members: json!({ "campaignArn": UNREGISTERED_CAMPAIGN }),
                    refusal: unresolvable(UNREGISTERED_CAMPAIGN),
                },
                Resource {
                    case: "a campaign of a recipe type another operation serves",
                    members: json!({ "campaignArn": CAMPAIGN_USER }),
                    refusal: format!(
                        "Campaign {CAMPAIGN_USER} deploys a solution version trained with the \
                         USER_PERSONALIZATION recipe type; the solution backing a \
                         GetPersonalizedRanking campaign must have been created using a recipe of \
                         type PERSONALIZED_RANKING."
                    ),
                },
            ],
            vec![
                RequestOnly {
                    case: "inputList one past its maximum",
                    members: json!({ "inputList": input_list(MAX_NUM_RESULTS + 1) }),
                    refusal: format!("Length of 'inputList' cannot be greater than {MAX_NUM_RESULTS}"),
                },
                RequestOnly {
                    case: "inputList one past the lower maximum a metadata request gets",
                    members: json!({
                        "inputList": input_list(MAX_NUM_RESULTS_WITH_METADATA + 1),
                        "metadataColumns": { "ITEMS": [COLUMN_GENRE] },
                    }),
                    refusal: format!(
                        "Length of 'inputList' cannot be greater than \
                         {MAX_NUM_RESULTS_WITH_METADATA}"
                    ),
                },
                RequestOnly {
                    case: "no userId at all",
                    members: json!({ "userId": "" }),
                    refusal: "userId is required: it is the user the campaign provides a \
                              personalized ranking for."
                        .to_string(),
                },
                RequestOnly {
                    case: "a metadataColumns map key naming another dataset type",
                    members: json!({ "metadataColumns": { "USERS": [COLUMN_GENRE] } }),
                    refusal: metadata_key_past("USERS"),
                },
                RequestOnly {
                    case: "one metadataColumns column past the documented maximum",
                    members: json!({
                        "metadataColumns": { "ITEMS": column_names(MAX_METADATA_COLUMNS + 1) },
                    }),
                    refusal: metadata_columns_past(MAX_METADATA_COLUMNS + 1),
                },
            ],
        ),
        other => panic!("case {other}: no table for this route"),
    }
}

/// Every operation's route, so the rule is asserted over all three.
const ROUTES: [&str; 3] = [RECOMMENDATIONS, ACTION_RECOMMENDATIONS, PERSONALIZE_RANKING];

#[tokio::test]
async fn a_request_only_refusal_is_answered_before_the_resource_is_resolved() {
    let emulator = seeded();

    for route in ROUTES {
        let (base, resources, violations) = table(route);
        for resource in &resources {
            for violation in &violations {
                let body = merged(&merged(&base, &resource.members), &violation.members);
                let case = format!("{route}: {} with {}", violation.case, resource.case);
                let refusal = emulator.refusal(route, body).await;
                assert_eq!(
                    (
                        refusal.status,
                        refusal.code.as_str(),
                        refusal.errortype.as_str()
                    ),
                    (400, "InvalidInputException", "InvalidInputException"),
                    "case {case}: expected 400 InvalidInputException, got {refusal:?}"
                );
                assert_eq!(
                    refusal.message, violation.refusal,
                    "case {case}: expected the request-only refusal {:?}, got {:?}",
                    violation.refusal, refusal.message
                );
                assert_ne!(
                    refusal.message, resource.refusal,
                    "case {case}: the resource-dependent refusal must not win over a \
                     request-only one, got {:?}",
                    refusal.message
                );
            }
        }
    }
}

#[tokio::test]
async fn a_well_formed_request_still_earns_its_resources_refusal() {
    // The complement of the rule: hoisting the request-only checks must not swallow the
    // refusals that genuinely need the resolved resource, so each resource row still
    // answers its own refusal once the request is well formed on its own terms.
    let emulator = seeded();

    for route in ROUTES {
        let (base, resources, _) = table(route);
        for resource in &resources {
            let body = merged(&base, &resource.members);
            let case = format!("{route}: {} with a well-formed request", resource.case);
            let refusal = emulator.refusal(route, body).await;
            assert_eq!(
                (refusal.status, refusal.code.as_str()),
                (400, "InvalidInputException"),
                "case {case}: expected 400 InvalidInputException, got {refusal:?}"
            );
            assert_eq!(
                refusal.message, resource.refusal,
                "case {case}: expected the resource-dependent refusal {:?}, got {:?}",
                resource.refusal, refusal.message
            );
        }
    }
}

#[tokio::test]
async fn a_request_at_its_bound_against_a_served_resource_is_answered() {
    // The bound itself is inclusive, and the hoisted check is the only thing in front of
    // a request that satisfies it: at the maximum, with the resource the operation
    // serves, the call succeeds and reports no more entries than the catalog holds.
    let emulator = seeded();

    // Each row's expected length is computed from what the operation reports: the two
    // recommendation operations report at most the seeded catalog, while
    // `GetPersonalizedRanking` reranks every entry `inputList` names, appending the ones
    // the catalog does not hold.
    let rows: [(&str, &str, Value, &str, usize); 4] = [
        (
            "GetRecommendations at its maximum",
            RECOMMENDATIONS,
            json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1", "numResults": MAX_NUM_RESULTS }),
            "itemList",
            CATALOG_SIZE,
        ),
        (
            "GetRecommendations at the lower maximum a metadata request gets",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-1",
                "numResults": MAX_NUM_RESULTS_WITH_METADATA,
                "metadataColumns": { "ITEMS": [COLUMN_GENRE] },
            }),
            "itemList",
            CATALOG_SIZE,
        ),
        (
            "GetActionRecommendations at its maximum",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": "user-1",
                "numResults": MAX_NUM_ACTION_RESULTS,
            }),
            "actionList",
            CATALOG_SIZE,
        ),
        (
            "GetPersonalizedRanking at its maximum",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-1",
                "inputList": input_list(MAX_NUM_RESULTS),
            }),
            "personalizedRanking",
            MAX_NUM_RESULTS,
        ),
    ];

    for (case, route, body, member, expected) in rows {
        let response = emulator.ok(route, body).await;
        let entries = response
            .get(member)
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: response {response} carries no {member}"))
            .len();
        assert_eq!(
            entries, expected,
            "case {case}: expected {expected} entries in {member}, got {entries}"
        );
    }
}
