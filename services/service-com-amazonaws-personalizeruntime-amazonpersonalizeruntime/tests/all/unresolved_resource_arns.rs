// SPDX-License-Identifier: CC0-1.0

//! The rule this suite pins: **an inference request that names a well-formed
//! Personalize resource ARN the data plane cannot resolve to an ACTIVE resource is
//! refused through the INVALID-INPUT channel** — HTTP 400, error shape
//! `InvalidInputException` (in `x-amzn-errortype` and in the body's `__type`), with the
//! message `<arn> does not exist or is not active yet.` naming the ARN the request
//! itself sent. The data plane serves a resource only once it is ACTIVE and cannot tell
//! "never created" from "created, not ACTIVE yet", so both answer the same way; the
//! modelled `ResourceNotFoundException` (404) is never the answer on this path.
//!
//! It holds for every operation of this service and for every request member that
//! names a registry resource — `campaignArn`, `recommenderArn`, `filterArn` and a
//! promotion's own `filterArn` — so the table below spans operations, members and ARN
//! forms (partition, region, account, resource type, and the resource-name length at
//! both ends of the modelled `Arn` `length 0..256` bound). The recorded case from the
//! issue is one row of it.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, RecipeType, Recommender,
};

use crate::support::{
    actions, items, Emulator, ACTION_RECOMMENDATIONS, PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// The one message form the data plane uses for an ARN it cannot resolve.
fn unresolvable_message(arn: &str) -> String {
    format!("{arn} does not exist or is not active yet.")
}

/// A registered `USER_PERSONALIZATION` campaign.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/user-personalization";
/// A registered `PERSONALIZED_RANKING` campaign.
const CAMPAIGN_RANKING: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/ranking";
/// A registered `PERSONALIZED_ACTIONS` campaign.
const CAMPAIGN_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/actions";
/// A registered domain recommender.
const RECOMMENDER: &str = "arn:aws:personalize:us-east-1:000000000000:recommender/top-picks";
/// A registered filter over the Items dataset.
const FILTER_ITEMS: &str = "arn:aws:personalize:us-east-1:000000000000:filter/recent";

/// The modelled bound on `Arn`: `length 0..256`
/// (`smithy_query(query="operation",operation="GetRecommendations")` —
/// `campaignArn`/`recommenderArn`/`filterArn` all target `Arn`).
const MAX_ARN_LENGTH: usize = 256;

/// How many entries the seeded catalogs hold.
const CATALOG_SIZE: usize = 8;

/// A campaign ARN of exactly `length` characters that matches the modelled pattern
/// `^arn:([a-z\d-]+):personalize:.*:.*:.+$`.
fn campaign_arn_of_length(length: usize) -> String {
    let prefix = "arn:aws:personalize:us-east-1:000000000000:campaign/";
    assert!(
        length > prefix.len(),
        "a {length}-character ARN cannot carry the {}-character prefix",
        prefix.len()
    );
    format!("{prefix}{}", "c".repeat(length - prefix.len()))
}

/// An emulator whose registry holds one resource of each kind, so a row that varies
/// only the UNRESOLVABLE member leaves everything else resolvable.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    for (arn, recipe_type) in [
        (CAMPAIGN_USER, RecipeType::UserPersonalization),
        (CAMPAIGN_RANKING, RecipeType::PersonalizedRanking),
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
        "the campaign seam must register the PERSONALIZED_ACTIONS campaign"
    );
    assert!(
        registry::register_recommender(
            engine,
            Recommender::new(RECOMMENDER, "top-picks", "Top picks for you", items(CATALOG_SIZE))
        ),
        "the recommender seam must register the recommender"
    );
    assert_eq!(
        registry::register_filter(engine, FILTER_ITEMS, "INCLUDE ItemID WHERE Items.YEAR >= 2010"),
        Ok(true),
        "the filter seam must register the filter"
    );
    emulator
}

/// One row of the table: a request, and the ARN in it that resolves to nothing.
struct Row {
    case: String,
    route: &'static str,
    arn: String,
    body: Value,
}

/// Every operation × every request member that names a registry resource × a spread of
/// ARN forms.
fn rows() -> Vec<Row> {
    // Partitions and regions are varied because the rule is about RESOLUTION, not about
    // any one ARN spelling; every value below still matches the modelled `Arn` pattern.
    let unresolvable: [(&'static str, String); 6] = [
        (
            "commercial partition",
            "arn:aws:personalize:us-east-1:000000000000:campaign/never-created".to_string(),
        ),
        (
            "china partition and region",
            "arn:aws-cn:personalize:cn-north-1:000000000000:campaign/never-created".to_string(),
        ),
        (
            "govcloud partition",
            "arn:aws-us-gov:personalize:us-gov-west-1:000000000000:campaign/never-created"
                .to_string(),
        ),
        (
            "a one-character resource name",
            "arn:aws:personalize:eu-west-3:000000000000:campaign/c".to_string(),
        ),
        (
            "a resource name at the modelled 256-character Arn bound",
            campaign_arn_of_length(MAX_ARN_LENGTH),
        ),
        (
            "a resource name one character inside the bound",
            campaign_arn_of_length(MAX_ARN_LENGTH - 1),
        ),
    ];

    let mut rows: Vec<Row> = Vec::new();

    // `campaignArn`, on all three operations.
    for (form, arn) in unresolvable {
        rows.push(Row {
            case: format!("GetRecommendations campaignArn, {form}"),
            route: RECOMMENDATIONS,
            arn: arn.clone(),
            body: json!({ "campaignArn": arn, "userId": "user-1", "numResults": 3 }),
        });
        rows.push(Row {
            case: format!("GetPersonalizedRanking campaignArn, {form}"),
            route: PERSONALIZE_RANKING,
            arn: arn.clone(),
            body: json!({
                "campaignArn": arn,
                "userId": "user-1",
                "inputList": ["item-1", "item-2"],
            }),
        });
        rows.push(Row {
            case: format!("GetActionRecommendations campaignArn, {form}"),
            route: ACTION_RECOMMENDATIONS,
            arn: arn.clone(),
            body: json!({ "campaignArn": arn, "userId": "user-1", "numResults": 2 }),
        });
    }

    // `recommenderArn`, the other resource `GetRecommendations` recommends from.
    for resource in ["recommender/never-created", "recommender/r"] {
        let arn = format!("arn:aws:personalize:ap-south-1:000000000000:{resource}");
        rows.push(Row {
            case: format!("GetRecommendations recommenderArn, {resource}"),
            route: RECOMMENDATIONS,
            arn: arn.clone(),
            body: json!({ "recommenderArn": arn, "userId": "user-1" }),
        });
    }

    // `filterArn`, on every operation that takes one — with the campaign resolvable, so
    // the filter is the only thing that does not resolve.
    for resource in ["filter/never-created", "filter/f"] {
        let arn = format!("arn:aws:personalize:us-east-1:000000000000:{resource}");
        rows.push(Row {
            case: format!("GetRecommendations filterArn, {resource}"),
            route: RECOMMENDATIONS,
            arn: arn.clone(),
            body: json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1", "filterArn": arn }),
        });
        rows.push(Row {
            case: format!("GetRecommendations promotions[0].filterArn, {resource}"),
            route: RECOMMENDATIONS,
            arn: arn.clone(),
            body: json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-1",
                "promotions": [{
                    "name": "promo",
                    "percentPromotedItems": 50,
                    "filterArn": arn,
                }],
            }),
        });
        rows.push(Row {
            case: format!("GetPersonalizedRanking filterArn, {resource}"),
            route: PERSONALIZE_RANKING,
            arn: arn.clone(),
            body: json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-1",
                "inputList": ["item-1"],
                "filterArn": arn,
            }),
        });
        rows.push(Row {
            case: format!("GetActionRecommendations filterArn, {resource}"),
            route: ACTION_RECOMMENDATIONS,
            arn: arn.clone(),
            body: json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": "user-1",
                "filterArn": arn,
            }),
        });
    }

    // A filter that IS registered, named by an operation whose campaign is not: the
    // first ARN that fails to resolve is the one reported.
    rows.push(Row {
        case: "GetRecommendations unresolvable campaign beside a registered filter".to_string(),
        route: RECOMMENDATIONS,
        arn: "arn:aws:personalize:us-east-1:000000000000:campaign/never-created".to_string(),
        body: json!({
            "campaignArn": "arn:aws:personalize:us-east-1:000000000000:campaign/never-created",
            "userId": "user-1",
            "filterArn": FILTER_ITEMS,
        }),
    });

    rows
}

#[tokio::test]
async fn an_arn_that_resolves_to_nothing_is_invalid_input_naming_that_arn() {
    let emulator = seeded();

    for row in rows() {
        let refusal = emulator.refusal(row.route, row.body.clone()).await;
        assert_eq!(
            (refusal.status, refusal.code.as_str(), refusal.errortype.as_str()),
            (400, "InvalidInputException", "InvalidInputException"),
            "case {} {}: expected 400 InvalidInputException in both __type and \
             x-amzn-errortype, got {refusal:?}",
            row.case,
            row.arn
        );
        assert_eq!(
            refusal.message,
            unresolvable_message(&row.arn),
            "case {} {}: expected the message to name the request's own ARN, got {:?}",
            row.case,
            row.arn,
            refusal.message
        );
    }
}

#[tokio::test]
async fn resolvable_resources_are_still_served_so_the_refusal_is_not_unconditional() {
    let emulator = seeded();

    let served: [(&str, &str, Value); 4] = [
        (
            "GetRecommendations on a registered campaign",
            RECOMMENDATIONS,
            json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1", "numResults": 3 }),
        ),
        (
            "GetRecommendations on a registered recommender and filter",
            RECOMMENDATIONS,
            json!({ "recommenderArn": RECOMMENDER, "userId": "user-1", "filterArn": FILTER_ITEMS }),
        ),
        (
            "GetPersonalizedRanking on a registered campaign",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-1",
                "inputList": ["item-1", "item-2"],
            }),
        ),
        (
            "GetActionRecommendations on a registered campaign",
            ACTION_RECOMMENDATIONS,
            json!({ "campaignArn": CAMPAIGN_ACTIONS, "userId": "user-1", "numResults": 2 }),
        ),
    ];

    for (case, route, body) in served {
        let (status, parsed) = emulator.call(route, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: a registered resource must be served, got {status} with {parsed}"
        );
    }
}

#[tokio::test]
async fn the_refusal_is_a_read_so_it_repeats_and_mints_nothing() {
    let emulator = seeded();
    let engine = &*emulator.engine;
    let arn = "arn:aws:personalize:us-east-1:000000000000:campaign/never-created";
    let body = json!({ "campaignArn": arn, "userId": "user-1", "numResults": 3 });

    let before = registry::issued_recommendation_ids(engine).expect("the registry is reachable");
    let first = emulator.refusal(RECOMMENDATIONS, body.clone()).await;
    let second = emulator.refusal(RECOMMENDATIONS, body).await;
    let after = registry::issued_recommendation_ids(engine).expect("the registry is reachable");

    assert_eq!(
        (first.status, first.code.as_str(), first.message.as_str()),
        (
            second.status,
            second.code.as_str(),
            second.message.as_str()
        ),
        "case repeated refusal: the same request must be refused the same way, got \
         {first:?} then {second:?}"
    );
    assert_eq!(
        before, after,
        "case repeated refusal: a refused request must mint no recommendationId, got \
         {before:?} then {after:?}"
    );
    assert!(
        registry::campaign(engine, arn).is_none(),
        "case repeated refusal: a refused request must not register the ARN it named"
    );
}
