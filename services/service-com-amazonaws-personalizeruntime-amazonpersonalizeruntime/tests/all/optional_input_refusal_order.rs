// SPDX-License-Identifier: CC0-1.0

//! When the request leaves out a personalization input the model marks OPTIONAL.
//!
//! The rule, over every inference operation: a member the model does not mark
//! `@required` is an input of the RESOLVED resource, never a fact about the request, so
//! its absence is answered only after the request's `campaignArn` / `recommenderArn`
//! has been resolved — the resource's own refusal (an ARN the registry does not hold, a
//! recipe type another operation serves) wins over it, and the refusal for the missing
//! input names the resource that needed it.
//!
//! The rows span the operations that take such a member (`GetRecommendations`'s `userId`
//! and `itemId`, `GetActionRecommendations`'s `userId`), the two forms an absent member
//! arrives in (left out, or sent empty — the modelled `@length` of each starts at 0, so
//! the boundary admits the empty string), the resource kinds (campaign and recommender)
//! and the resource states (unregistered, registered-but-wrong-recipe, and
//! registered-and-served).
//!
//! `GetPersonalizedRanking$userId` is NOT one of these members — the model marks it
//! `@required`, so it is settled from the request alone; that order is pinned in
//! `request_only_refusal_order.rs`.

use serde_json::{json, Map, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, RecipeType, Recommender,
};

use crate::support::{actions, items, Emulator, ACTION_RECOMMENDATIONS, RECOMMENDATIONS};

/// A registered `USER_PERSONALIZATION` campaign: what `GetRecommendations` serves with
/// `userId`, and the wrong recipe type for `GetActionRecommendations`.
const CAMPAIGN_USER: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/user-personalization";
/// A registered `RELATED_ITEMS` campaign: what `GetRecommendations` serves with `itemId`.
const CAMPAIGN_RELATED: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/related-items";
/// A registered `PERSONALIZED_ACTIONS` campaign: what `GetActionRecommendations` serves,
/// and the wrong recipe type for `GetRecommendations`.
const CAMPAIGN_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/actions";
/// A registered recommender whose use case needs `userId` (`RECOMMENDER_USE_CASES`).
const RECOMMENDER_FOR_YOU: &str =
    "arn:aws:personalize:us-east-1:000000000000:recommender/recommended-for-you";
/// The use case behind [`RECOMMENDER_FOR_YOU`], as `RECOMMENDER_USE_CASES` spells it.
const USE_CASE_FOR_YOU: &str = "Recommended for you";

/// A well-formed campaign ARN the registry does not hold.
const UNREGISTERED_CAMPAIGN: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/never-created";
/// A well-formed recommender ARN the registry does not hold.
const UNREGISTERED_RECOMMENDER: &str =
    "arn:aws:personalize:us-east-1:000000000000:recommender/never-created";

/// How many entries the seeded catalogs hold.
const CATALOG_SIZE: usize = 6;

/// An emulator whose registry holds one resource of every kind the rows name.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;
    for (arn, recipe_type) in [
        (CAMPAIGN_USER, RecipeType::UserPersonalization),
        (CAMPAIGN_RELATED, RecipeType::RelatedItems),
    ] {
        assert!(
            registry::register_campaign(
                engine,
                Campaign::new(arn, "demo", recipe_type, items(CATALOG_SIZE))
            ),
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
    assert!(
        registry::register_recommender(
            engine,
            Recommender::new(
                RECOMMENDER_FOR_YOU,
                "for-you",
                USE_CASE_FOR_YOU,
                items(CATALOG_SIZE)
            )
        ),
        "the recommender seam must register the recommender"
    );
    emulator
}

/// The ways a request can leave an optional member out, each of which the service reads
/// as "not supplied": the member is absent from the body, or present and empty (every
/// one of these members is modelled `@length` from 0, so the request boundary admits the
/// empty string and the handler is what reads it as "no user" / "no item"). A member sent
/// as JSON `null` is neither — the request boundary refuses it as a type violation before
/// any handler runs.
fn absent_forms() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("the member left out", None),
        ("the member sent empty", Some(Value::from(""))),
    ]
}

/// `base` with `member` absent in one of the [`ABSENT_FORMS`].
fn without(base: &Value, member: &str, form: &Option<Value>) -> Value {
    let mut object: Map<String, Value> = base
        .as_object()
        .expect("a request body is a JSON object")
        .clone();
    match form {
        Some(value) => {
            object.insert(member.to_string(), value.clone());
        }
        None => {
            object.remove(member);
        }
    }
    Value::Object(object)
}

/// `base` with `member` set to `value`.
fn with(base: &Value, member: &str, value: &str) -> Value {
    without(base, member, &Some(Value::from(value)))
}

/// The refusal for a resource ARN the registry cannot resolve.
fn unresolvable(arn: &str) -> String {
    format!("{arn} does not exist or is not active yet.")
}

/// One row: an operation, the optional personalization member it takes, the resource the
/// request names, and the refusal that request earns while the member is absent.
struct Row {
    /// How the row reads in an assertion message.
    case: &'static str,
    /// The operation's route.
    route: &'static str,
    /// The request, with the optional member supplied — the row's positive control.
    body: Value,
    /// The optional member the row leaves out.
    member: &'static str,
    /// The ARN of the resource the request names, which every one of these refusals
    /// reports.
    arn: &'static str,
    /// The refusal an absent `member` earns: the resource's own when the resource cannot
    /// be served, else the one naming the resource that needs the member.
    refusal: String,
    /// Whether this row's resource is one the operation SERVES, so supplying `member` is
    /// all the request was missing.
    served: bool,
}

/// Every row: each optional member against each resource state.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "GetActionRecommendations, an unregistered campaign",
            route: ACTION_RECOMMENDATIONS,
            body: json!({ "campaignArn": UNREGISTERED_CAMPAIGN, "userId": "user-1" }),
            member: "userId",
            arn: UNREGISTERED_CAMPAIGN,
            refusal: unresolvable(UNREGISTERED_CAMPAIGN),
            served: false,
        },
        Row {
            case: "GetActionRecommendations, a campaign of a recipe type another \
                   operation serves",
            route: ACTION_RECOMMENDATIONS,
            body: json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1" }),
            member: "userId",
            arn: CAMPAIGN_USER,
            refusal: format!(
                "Campaign {CAMPAIGN_USER} deploys a solution version trained with the \
                 USER_PERSONALIZATION recipe type; this campaign must deploy a solution version \
                 trained with a PERSONALIZED_ACTIONS recipe."
            ),
            served: false,
        },
        Row {
            case: "GetActionRecommendations, the campaign it serves",
            route: ACTION_RECOMMENDATIONS,
            body: json!({ "campaignArn": CAMPAIGN_ACTIONS, "userId": "user-1" }),
            member: "userId",
            arn: CAMPAIGN_ACTIONS,
            refusal: format!(
                "userId is required to get action recommendations from campaign \
                 {CAMPAIGN_ACTIONS} (PERSONALIZED_ACTIONS)."
            ),
            served: true,
        },
        Row {
            case: "GetRecommendations, an unregistered campaign",
            route: RECOMMENDATIONS,
            body: json!({ "campaignArn": UNREGISTERED_CAMPAIGN, "userId": "user-1" }),
            member: "userId",
            arn: UNREGISTERED_CAMPAIGN,
            refusal: unresolvable(UNREGISTERED_CAMPAIGN),
            served: false,
        },
        Row {
            case: "GetRecommendations, an unregistered recommender",
            route: RECOMMENDATIONS,
            body: json!({ "recommenderArn": UNREGISTERED_RECOMMENDER, "userId": "user-1" }),
            member: "userId",
            arn: UNREGISTERED_RECOMMENDER,
            refusal: unresolvable(UNREGISTERED_RECOMMENDER),
            served: false,
        },
        Row {
            case: "GetRecommendations, a campaign of a recipe type another operation serves",
            route: RECOMMENDATIONS,
            body: json!({ "campaignArn": CAMPAIGN_ACTIONS, "userId": "user-1" }),
            member: "userId",
            arn: CAMPAIGN_ACTIONS,
            refusal: format!(
                "Campaign {CAMPAIGN_ACTIONS} deploys a solution version trained with the \
                 PERSONALIZED_ACTIONS recipe type; use the GetActionRecommendations API."
            ),
            served: false,
        },
        Row {
            case: "GetRecommendations, the user-personalization campaign it serves",
            route: RECOMMENDATIONS,
            body: json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1" }),
            member: "userId",
            arn: CAMPAIGN_USER,
            refusal: format!(
                "userId is required to get recommendations from campaign {CAMPAIGN_USER} \
                 (USER_PERSONALIZATION)."
            ),
            served: true,
        },
        Row {
            case: "GetRecommendations, the recommender it serves",
            route: RECOMMENDATIONS,
            body: json!({ "recommenderArn": RECOMMENDER_FOR_YOU, "userId": "user-1" }),
            member: "userId",
            arn: RECOMMENDER_FOR_YOU,
            refusal: format!(
                "userId is required to get recommendations from recommender \
                 {RECOMMENDER_FOR_YOU} ({USE_CASE_FOR_YOU})."
            ),
            served: true,
        },
        Row {
            case: "GetRecommendations, an unregistered campaign and no itemId",
            route: RECOMMENDATIONS,
            body: json!({ "campaignArn": UNREGISTERED_CAMPAIGN, "itemId": "item-1" }),
            member: "itemId",
            arn: UNREGISTERED_CAMPAIGN,
            refusal: unresolvable(UNREGISTERED_CAMPAIGN),
            served: false,
        },
        Row {
            case: "GetRecommendations, the related-items campaign it serves",
            route: RECOMMENDATIONS,
            body: json!({ "campaignArn": CAMPAIGN_RELATED, "itemId": "item-1" }),
            member: "itemId",
            arn: CAMPAIGN_RELATED,
            refusal: format!(
                "itemId is required to get recommendations from campaign {CAMPAIGN_RELATED} \
                 (RELATED_ITEMS)."
            ),
            served: true,
        },
    ]
}

#[tokio::test]
async fn an_absent_optional_input_is_answered_only_once_the_resource_is_resolved() {
    let emulator = seeded();

    for row in rows() {
        for (form, absent) in &absent_forms() {
            let case = format!("{}: {} ({form})", row.case, row.member);
            let body = without(&row.body, row.member, absent);
            let refusal = emulator.refusal(row.route, body).await;
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
                refusal.message, row.refusal,
                "case {case}: expected the refusal {:?}, got {:?}",
                row.refusal, refusal.message
            );
            // The absent member never earns a refusal of its own: whatever the request
            // left out, the sentence reports the resource the request named.
            assert!(
                refusal.message.contains(row.arn),
                "case {case}: expected a refusal naming {}, got {:?}",
                row.arn,
                refusal.message
            );
        }
    }
}

#[tokio::test]
async fn the_same_request_with_the_input_supplied_is_served_or_refused_for_its_resource() {
    // The complement: the input is what the RESOLVED resource was missing, so supplying
    // it either serves the request (a resource this operation serves) or leaves the
    // resource's own refusal exactly as it was (an unresolvable ARN, a wrong recipe type).
    let emulator = seeded();

    for row in rows() {
        let case = format!("{}: {} supplied", row.case, row.member);
        let body = with(&row.body, row.member, "supplied-1");
        let (status, parsed) = emulator.call(row.route, &body).await;
        if row.served {
            let entries = parsed
                .get("itemList")
                .or_else(|| parsed.get("actionList"))
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                (status, entries > 0),
                (200, true),
                "case {case}: expected a served response, got {status} with {parsed}"
            );
        } else {
            assert_eq!(
                (status, crate::support::error_message(&parsed)),
                (400, row.refusal.clone()),
                "case {case}: the resource's own refusal must stand, got {status} with {parsed}"
            );
        }
    }
}
