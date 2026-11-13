// SPDX-License-Identifier: CC0-1.0

//! The rule this suite pins: **every ARN-carrying member of an inference request
//! references exactly ONE Personalize resource type, and the type a value claims is read
//! out of the ARN's own resource part before the registry is consulted at all** — an ARN
//! whose resource type is another one references nothing that member could name, so it is
//! refused `InvalidInputException` (HTTP 400, in `x-amzn-errortype` and in the body's
//! `__type`) with the message `The given <kind> ARN is invalid: <arn>`, naming the member's
//! own resource kind and the value the request sent.
//!
//! That refusal is DISTINCT from the one an ARN of the right type the registry does not
//! hold earns (`<arn> does not exist or is not active yet.`): both carry the same code and
//! status, so the message is the only observable a client can separate "you handed me a
//! reference to something else" from "this resource does not exist, or is not ACTIVE yet"
//! on — and only the second can change by itself.
//!
//! The table below spans the class: every operation, every member that references a
//! registry resource (`campaignArn`, `recommenderArn`, `filterArn` and a promotion's own
//! `filterArn`), and every other Personalize resource type a caller holds an ARN of. The
//! recorded case from the issue is one row of it.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, RecipeType, Recommender,
};

use crate::support::{
    actions, items, Emulator, ACTION_RECOMMENDATIONS, PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// The sentence for a value that references no resource of `kind` at all.
fn invalid_arn_message(kind: &str, given: &str) -> String {
    format!("The given {kind} ARN is invalid: {given}")
}

/// The sentence for an ARN of the RIGHT kind the registry cannot resolve.
fn unresolvable_message(arn: &str) -> String {
    format!("{arn} does not exist or is not active yet.")
}

/// A registered `USER_PERSONALIZATION` campaign.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/personalization";
/// A registered `PERSONALIZED_RANKING` campaign.
const CAMPAIGN_RANKING: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/ranking";
/// A registered `PERSONALIZED_ACTIONS` campaign.
const CAMPAIGN_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/actions";
/// A registered domain recommender.
const RECOMMENDER: &str = "arn:aws:personalize:us-east-1:000000000000:recommender/top-picks";
/// A registered filter over the Items dataset.
const FILTER_ITEMS: &str = "arn:aws:personalize:us-east-1:000000000000:filter/every-item";
/// A registered filter over the Actions dataset.
const FILTER_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:filter/every-action";

/// The ARN prefix every value below is built on, so a row varies only the resource part.
const ARN_PREFIX: &str = "arn:aws:personalize:us-east-1:000000000000:";

/// How many entries the seeded catalogs hold.
const CATALOG_SIZE: usize = 6;

/// An emulator holding one resource of each kind, so a row that varies only the member
/// under test leaves everything else resolvable.
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
            Recommender::new(
                RECOMMENDER,
                "top-picks",
                "Top picks for you",
                items(CATALOG_SIZE)
            )
        ),
        "the recommender seam must register the recommender"
    );
    assert_eq!(
        registry::register_filter(
            engine,
            FILTER_ITEMS,
            "INCLUDE ItemID WHERE Items.YEAR >= 1900"
        ),
        Ok(true),
        "the filter seam must register the Items filter"
    );
    assert_eq!(
        registry::register_filter(
            engine,
            FILTER_ACTIONS,
            "INCLUDE ActionID WHERE Actions.CHANNEL IN (\"App\", \"Email\")"
        ),
        Ok(true),
        "the filter seam must register the Actions filter"
    );
    emulator
}

/// How a request carries the ARN value under test.
type Build = fn(&str) -> Value;

/// One ARN-carrying member of this service.
struct ArnMember {
    /// How the member reads in a case name.
    case: &'static str,
    /// The operation's modelled route.
    route: &'static str,
    /// The resource kind this member references, as a refusal names it.
    kind: &'static str,
    /// A request carrying the value under test in that member, with everything else
    /// resolvable.
    build: Build,
}

fn recommendations_campaign(value: &str) -> Value {
    json!({ "campaignArn": value, "userId": "user-1", "numResults": 3 })
}

fn recommendations_recommender(value: &str) -> Value {
    json!({ "recommenderArn": value, "userId": "user-1" })
}

fn recommendations_filter(value: &str) -> Value {
    json!({ "campaignArn": CAMPAIGN_USER, "userId": "user-1", "filterArn": value })
}

fn recommendations_promotion_filter(value: &str) -> Value {
    json!({
        "campaignArn": CAMPAIGN_USER,
        "userId": "user-1",
        "promotions": [{ "name": "promo", "percentPromotedItems": 50, "filterArn": value }],
    })
}

fn ranking_campaign(value: &str) -> Value {
    json!({ "campaignArn": value, "userId": "user-1", "inputList": ["item-1", "item-2"] })
}

fn ranking_filter(value: &str) -> Value {
    json!({
        "campaignArn": CAMPAIGN_RANKING,
        "userId": "user-1",
        "inputList": ["item-1", "item-2"],
        "filterArn": value,
    })
}

fn action_campaign(value: &str) -> Value {
    json!({ "campaignArn": value, "userId": "user-1", "numResults": 2 })
}

fn action_filter(value: &str) -> Value {
    json!({ "campaignArn": CAMPAIGN_ACTIONS, "userId": "user-1", "filterArn": value })
}

/// Every member of this service that references a registry resource, on every operation
/// that takes it.
fn arn_members() -> Vec<ArnMember> {
    vec![
        ArnMember {
            case: "GetRecommendations campaignArn",
            route: RECOMMENDATIONS,
            kind: "campaign",
            build: recommendations_campaign,
        },
        ArnMember {
            case: "GetRecommendations recommenderArn",
            route: RECOMMENDATIONS,
            kind: "recommender",
            build: recommendations_recommender,
        },
        ArnMember {
            case: "GetRecommendations filterArn",
            route: RECOMMENDATIONS,
            kind: "filter",
            build: recommendations_filter,
        },
        ArnMember {
            case: "GetRecommendations promotions[0].filterArn",
            route: RECOMMENDATIONS,
            kind: "filter",
            build: recommendations_promotion_filter,
        },
        ArnMember {
            case: "GetPersonalizedRanking campaignArn",
            route: PERSONALIZE_RANKING,
            kind: "campaign",
            build: ranking_campaign,
        },
        ArnMember {
            case: "GetPersonalizedRanking filterArn",
            route: PERSONALIZE_RANKING,
            kind: "filter",
            build: ranking_filter,
        },
        ArnMember {
            case: "GetActionRecommendations campaignArn",
            route: ACTION_RECOMMENDATIONS,
            kind: "campaign",
            build: action_campaign,
        },
        ArnMember {
            case: "GetActionRecommendations filterArn",
            route: ACTION_RECOMMENDATIONS,
            kind: "filter",
            build: action_filter,
        },
    ]
}

/// The resource parts of the OTHER Personalize resources a caller holds ARNs of.
///
/// Source: the resource part each ARN helper of the sibling control-plane crate mints —
/// `schema_arn`, `dataset_group_arn` and `dataset_arn` in
/// `services/service-com-amazonaws-personalize-amazonpersonalize/src/state.rs`, and
/// `solution_arn`, `solution_version_arn` and `dataset_import_job_arn` in that crate's
/// `src/state/pipeline.rs`. The issue cited only a schema ARN; every other row here is a
/// resource type it did not cite.
fn foreign_resource_parts() -> Vec<(&'static str, String)> {
    vec![
        ("a schema", "schema/sim-schema".to_string()),
        ("a dataset group", "dataset-group/sim-group".to_string()),
        ("a dataset", "dataset/sim-group/INTERACTIONS".to_string()),
        ("a solution", "solution/sim-solution".to_string()),
        (
            "a solution version",
            "solution/sim-solution/00000001".to_string(),
        ),
        (
            "a dataset import job",
            "dataset-import-job/sim-import".to_string(),
        ),
    ]
}

/// Resource parts whose type merely STARTS WITH the member's own type, which the rule
/// refuses too: the whole resource-type segment is what identifies the kind, not a prefix
/// of it.
fn near_miss_resource_parts(kind: &str) -> Vec<(String, String)> {
    vec![
        (
            format!("a type the {kind} type is a prefix of"),
            format!("{kind}-group/sim-near-miss"),
        ),
        (
            format!("the {kind} type pluralised"),
            format!("{kind}s/sim-near-miss"),
        ),
    ]
}

/// One row of the table: a request whose member under test references another kind.
struct Row {
    case: String,
    route: &'static str,
    kind: &'static str,
    arn: String,
    body: Value,
}

fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for member in arn_members() {
        let forms = foreign_resource_parts()
            .into_iter()
            .map(|(form, resource)| (form.to_string(), resource))
            .chain(near_miss_resource_parts(member.kind));
        for (form, resource) in forms {
            let arn = format!("{ARN_PREFIX}{resource}");
            rows.push(Row {
                case: format!("{} given {form}", member.case),
                route: member.route,
                kind: member.kind,
                arn: arn.clone(),
                body: (member.build)(&arn),
            });
        }
    }
    rows
}

#[tokio::test]
async fn an_arn_of_another_resource_type_is_refused_as_the_invalid_reference_it_is() {
    let emulator = seeded();

    for row in rows() {
        let refusal = emulator.refusal(row.route, row.body.clone()).await;
        assert_eq!(
            (
                refusal.status,
                refusal.code.as_str(),
                refusal.errortype.as_str()
            ),
            (400, "InvalidInputException", "InvalidInputException"),
            "case {}: expected 400 InvalidInputException in both __type and x-amzn-errortype, \
             got {refusal:?}",
            row.case
        );
        assert_eq!(
            refusal.message,
            invalid_arn_message(row.kind, &row.arn),
            "case {}: expected the refusal to name the member's resource kind and the value the \
             request sent, got {:?}",
            row.case,
            refusal.message
        );
        assert_ne!(
            refusal.message,
            unresolvable_message(&row.arn),
            "case {}: a reference to another resource type must not be reported as a resource \
             that does not exist yet, got {:?}",
            row.case,
            refusal.message
        );
    }
}

#[tokio::test]
async fn an_arn_of_the_right_type_the_registry_does_not_hold_keeps_its_own_message() {
    // The complement of the rule, and what makes the message the observable that separates
    // the two: with the resource TYPE right, the same member earns the existence refusal.
    let emulator = seeded();

    for member in arn_members() {
        for name in ["never-created", "n"] {
            let arn = format!("{ARN_PREFIX}{}/{name}", member.kind);
            let case = format!("{} given an unregistered {} ARN", member.case, member.kind);
            let refusal = emulator.refusal(member.route, (member.build)(&arn)).await;
            assert_eq!(
                (refusal.status, refusal.code.as_str()),
                (400, "InvalidInputException"),
                "case {case}: expected 400 InvalidInputException, got {refusal:?}"
            );
            assert_eq!(
                refusal.message,
                unresolvable_message(&arn),
                "case {case}: expected the existence refusal naming the request's own ARN, got \
                 {:?}",
                refusal.message
            );
            assert_ne!(
                refusal.message,
                invalid_arn_message(member.kind, &arn),
                "case {case}: an ARN of the right resource type must not be reported as an \
                 invalid reference, got {:?}",
                refusal.message
            );
        }
    }
}

#[tokio::test]
async fn the_type_check_precedes_existence_so_a_registered_name_under_another_type_still_fails() {
    // "Existence is never consulted" is part of the rule: a wrong-type ARN carrying the
    // NAME of a resource the registry does hold is refused exactly the same way, and the
    // resource it borrowed the name from is untouched.
    let emulator = seeded();
    let engine = &*emulator.engine;

    let borrowed = format!("{ARN_PREFIX}schema/personalization");
    for (case, route, build) in [
        (
            "GetRecommendations campaignArn",
            RECOMMENDATIONS,
            recommendations_campaign as Build,
        ),
        (
            "GetPersonalizedRanking campaignArn",
            PERSONALIZE_RANKING,
            ranking_campaign as Build,
        ),
        (
            "GetActionRecommendations campaignArn",
            ACTION_RECOMMENDATIONS,
            action_campaign as Build,
        ),
    ] {
        let refusal = emulator.refusal(route, build(&borrowed)).await;
        assert_eq!(
            refusal.message,
            invalid_arn_message("campaign", &borrowed),
            "case {case} given a registered campaign's name under another type: expected the \
             invalid-reference refusal, got {:?}",
            refusal.message
        );
    }

    assert!(
        registry::campaign(engine, CAMPAIGN_USER).is_some(),
        "case borrowed name: the registered campaign must still be held"
    );
    assert!(
        registry::campaign(engine, &borrowed).is_none(),
        "case borrowed name: a refused request must register nothing"
    );
}

#[tokio::test]
async fn the_refusal_is_a_read_so_it_repeats_and_mints_nothing() {
    let emulator = seeded();
    let engine = &*emulator.engine;
    let arn = format!("{ARN_PREFIX}schema/sim-schema");
    let body = json!({ "campaignArn": arn, "userId": "user-1" });

    let before = registry::issued_recommendation_ids(engine).expect("the registry is reachable");
    let first = emulator.refusal(RECOMMENDATIONS, body.clone()).await;
    let second = emulator.refusal(RECOMMENDATIONS, body).await;
    let after = registry::issued_recommendation_ids(engine).expect("the registry is reachable");

    assert_eq!(
        (first.status, first.code.as_str(), first.message.as_str()),
        (second.status, second.code.as_str(), second.message.as_str()),
        "case repeated refusal: the same request must be refused the same way, got {first:?} \
         then {second:?}"
    );
    assert_eq!(
        before, after,
        "case repeated refusal: a refused request must mint no recommendationId, got {before:?} \
         then {after:?}"
    );
}

#[tokio::test]
async fn a_request_whose_references_all_name_their_own_kind_is_still_served() {
    // The refusals above are not unconditional: every member answers 200 once its ARN
    // references a registered resource of the kind that member names.
    let emulator = seeded();

    for (case, route, body) in [
        (
            "GetRecommendations on a campaign with a filter",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-1",
                "numResults": 3,
                "filterArn": FILTER_ITEMS,
            }),
        ),
        (
            "GetRecommendations on a campaign with a promotion",
            RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_USER,
                "userId": "user-1",
                "promotions": [{
                    "name": "promo",
                    "percentPromotedItems": 50,
                    "filterArn": FILTER_ITEMS,
                }],
            }),
        ),
        (
            "GetRecommendations on a recommender",
            RECOMMENDATIONS,
            json!({ "recommenderArn": RECOMMENDER, "userId": "user-1" }),
        ),
        (
            "GetPersonalizedRanking on a campaign with a filter",
            PERSONALIZE_RANKING,
            json!({
                "campaignArn": CAMPAIGN_RANKING,
                "userId": "user-1",
                "inputList": ["item-1", "item-2"],
                "filterArn": FILTER_ITEMS,
            }),
        ),
        (
            "GetActionRecommendations on a campaign with a filter",
            ACTION_RECOMMENDATIONS,
            json!({
                "campaignArn": CAMPAIGN_ACTIONS,
                "userId": "user-1",
                "numResults": 2,
                "filterArn": FILTER_ACTIONS,
            }),
        ),
    ] {
        let (status, parsed) = emulator.call(route, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: a reference of the member's own kind must be served, got {status} \
             with {parsed}"
        );
    }
}
