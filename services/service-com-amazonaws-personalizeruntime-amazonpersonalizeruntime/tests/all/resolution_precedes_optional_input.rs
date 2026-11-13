// SPDX-License-Identifier: CC0-1.0

//! The rule this suite pins as an INVARIANT: on an inference request whose named
//! resource cannot be served, the refusal is the SAME whatever a member the model
//! leaves optional carries — absent, empty, or any value — because the resource
//! reference is resolved first and an optional member's absence never earns a refusal
//! of its own ahead of it.
//!
//! Where the sibling suite `optional_input_refusal_order.rs` pins the WORDING each of
//! those refusals carries, this one pins the independence: for every unservable
//! resource the refusal is a function of the resource alone, so a client that got its
//! resource reference wrong is told so on the first call rather than after supplying a
//! member the model never required. The refusal of an unresolvable ARN is additionally
//! pinned verbatim — HTTP 400 `InvalidInputException` with
//! `<arn> does not exist or is not active yet.`, the ARN taken from the request.
//!
//! The rows span the operations that take such a member (`GetActionRecommendations`'s
//! `userId`, `GetRecommendations`'s `userId` and `itemId`), the resource kinds the
//! request can name (campaign, recommender), the ways a resource can be unservable
//! (an ARN the registry holds nothing under, and a registered campaign whose recipe
//! type another operation serves), and the ARN forms the modelled `Arn`
//! (`length 0..256`, pattern `^arn:([a-z\d-]+):personalize:.*:.*:.+$`) admits —
//! partition, region, account, and the resource-name length at both ends of the bound.
//! Each row is driven through every form the optional member can arrive in.
//!
//! `GetPersonalizedRanking$userId` is NOT such a member — the model marks it
//! `@required`, so it is settled from the request alone (`request_only_refusal_order.rs`).

use serde_json::{json, Map, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, RecipeType, Recommender,
};

use crate::support::{actions, items, Emulator, Refusal, ACTION_RECOMMENDATIONS, RECOMMENDATIONS};

/// A registered `USER_PERSONALIZATION` campaign: served by `GetRecommendations` with a
/// `userId`, and the wrong recipe type for `GetActionRecommendations`.
const CAMPAIGN_USER: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/user-personalization";
/// A registered `RELATED_ITEMS` campaign: served by `GetRecommendations` with an
/// `itemId`, and the wrong recipe type for `GetActionRecommendations`.
const CAMPAIGN_RELATED: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/related-items";
/// A registered `PERSONALIZED_ACTIONS` campaign: served by `GetActionRecommendations`,
/// and the wrong recipe type for `GetRecommendations`.
const CAMPAIGN_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/actions";
/// A registered recommender whose use case needs a `userId`.
const RECOMMENDER: &str = "arn:aws:personalize:us-east-1:000000000000:recommender/top-picks";
/// The domain use case behind [`RECOMMENDER`], as `RECOMMENDER_USE_CASES` spells it.
const USE_CASE: &str = "Recommended for you";

/// The modelled bound on `Arn`: `length 0..256`
/// (`smithy_query(query="operation",operation="GetActionRecommendations")` — `campaignArn`
/// and `filterArn` both target `Arn`).
const MAX_ARN_LENGTH: usize = 256;

/// How many entries the seeded catalogs hold.
const CATALOG_SIZE: usize = 6;

/// A campaign ARN of exactly `length` characters matching the modelled `Arn` pattern.
fn campaign_arn_of_length(length: usize) -> String {
    let prefix = "arn:aws:personalize:eu-west-1:000000000000:campaign/";
    assert!(
        length > prefix.len(),
        "a {length}-character ARN cannot carry the {}-character prefix",
        prefix.len()
    );
    format!("{prefix}{}", "n".repeat(length - prefix.len()))
}

/// The one message form the data plane uses for an ARN it cannot resolve to an ACTIVE
/// resource.
fn unresolvable(arn: &str) -> String {
    format!("{arn} does not exist or is not active yet.")
}

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
        "the campaign seam must register the PERSONALIZED_ACTIONS campaign"
    );
    assert!(
        registry::register_recommender(
            engine,
            Recommender::new(RECOMMENDER, "top-picks", USE_CASE, items(CATALOG_SIZE))
        ),
        "the recommender seam must register the recommender"
    );
    emulator
}

/// Every form an optional personalization member can arrive in: left out of the body,
/// present and empty (its modelled `@length` starts at 0, so the request boundary admits
/// the empty string), and carrying a value — two different values, because the refusal of
/// an unservable resource is a function of the resource and not of what the member says.
fn member_forms() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("left out", None),
        ("sent empty", Some("")),
        ("carrying a value", Some("subject-1")),
        ("carrying a different value", Some("a-quite-different-subject")),
    ]
}

/// `base` with `member` in the given form.
fn in_form(base: &Value, member: &str, form: Option<&str>) -> Value {
    let mut object: Map<String, Value> = base
        .as_object()
        .expect("a request body is a JSON object")
        .clone();
    match form {
        Some(value) => {
            object.insert(member.to_string(), Value::from(value));
        }
        None => {
            object.remove(member);
        }
    }
    Value::Object(object)
}

/// One row: an operation, the optional member it takes, and a request naming a resource
/// it cannot serve.
struct Row {
    /// How the row reads in an assertion message.
    case: String,
    /// The operation's route.
    route: &'static str,
    /// The optional member whose form this row varies.
    member: &'static str,
    /// The ARN of the resource the request names, which the refusal must report.
    arn: String,
    /// The request, without the optional member.
    body: Value,
    /// The verbatim refusal, where the rule fixes it: an ARN that resolves to nothing.
    /// `None` where the resource resolves but the operation does not serve it, whose
    /// wording `optional_input_refusal_order.rs` owns — here only its independence of
    /// the optional member is pinned.
    message: Option<String>,
}

/// Every row: each optional member against each way its resource cannot be served.
fn rows() -> Vec<Row> {
    // The ARN forms are varied because the rule is about RESOLUTION, not about any one
    // ARN spelling; every value below matches the modelled `Arn` pattern.
    let unregistered: [(&str, String); 5] = [
        (
            "commercial partition",
            "arn:aws:personalize:us-east-1:000000000000:campaign/never-created".to_string(),
        ),
        (
            "china partition and region",
            "arn:aws-cn:personalize:cn-north-1:000000000000:campaign/never-created".to_string(),
        ),
        (
            "a one-character resource name",
            "arn:aws:personalize:ap-south-1:000000000000:campaign/c".to_string(),
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

    for (form, arn) in unregistered {
        // `GetActionRecommendations`: the recorded case is this row with the member left
        // out, and `numResults` varies across the rows because the result count the
        // request asks for is settled from the request and never changes WHICH refusal a
        // resource earns.
        rows.push(Row {
            case: format!("GetActionRecommendations, an unregistered campaign ({form})"),
            route: ACTION_RECOMMENDATIONS,
            member: "userId",
            arn: arn.clone(),
            body: json!({ "campaignArn": arn }),
            message: Some(unresolvable(&arn)),
        });
        rows.push(Row {
            case: format!(
                "GetActionRecommendations, an unregistered campaign and numResults ({form})"
            ),
            route: ACTION_RECOMMENDATIONS,
            member: "userId",
            arn: arn.clone(),
            body: json!({ "campaignArn": arn, "numResults": 3 }),
            message: Some(unresolvable(&arn)),
        });
        rows.push(Row {
            case: format!("GetRecommendations userId, an unregistered campaign ({form})"),
            route: RECOMMENDATIONS,
            member: "userId",
            arn: arn.clone(),
            body: json!({ "campaignArn": arn }),
            message: Some(unresolvable(&arn)),
        });
        rows.push(Row {
            case: format!("GetRecommendations itemId, an unregistered campaign ({form})"),
            route: RECOMMENDATIONS,
            member: "itemId",
            arn: arn.clone(),
            body: json!({ "campaignArn": arn }),
            message: Some(unresolvable(&arn)),
        });
    }

    // The other resource kind `GetRecommendations` recommends from.
    for resource in ["recommender/never-created", "recommender/r"] {
        let arn = format!("arn:aws:personalize:us-west-2:000000000000:{resource}");
        rows.push(Row {
            case: format!("GetRecommendations userId, an unregistered {resource}"),
            route: RECOMMENDATIONS,
            member: "userId",
            arn: arn.clone(),
            body: json!({ "recommenderArn": arn }),
            message: Some(unresolvable(&arn)),
        });
    }

    // A resource the registry DOES hold, of a recipe type the operation does not serve:
    // resolution still decides the refusal, and the optional member still earns none.
    let wrong_recipe: [(&str, &'static str, &'static str); 4] = [
        (
            "GetActionRecommendations, a USER_PERSONALIZATION campaign",
            ACTION_RECOMMENDATIONS,
            CAMPAIGN_USER,
        ),
        (
            "GetActionRecommendations, a RELATED_ITEMS campaign",
            ACTION_RECOMMENDATIONS,
            CAMPAIGN_RELATED,
        ),
        (
            "GetRecommendations userId, a PERSONALIZED_ACTIONS campaign",
            RECOMMENDATIONS,
            CAMPAIGN_ACTIONS,
        ),
        (
            "GetRecommendations itemId, a PERSONALIZED_ACTIONS campaign",
            RECOMMENDATIONS,
            CAMPAIGN_ACTIONS,
        ),
    ];
    for (case, route, arn) in wrong_recipe {
        let member = if case.contains("itemId") {
            "itemId"
        } else {
            "userId"
        };
        rows.push(Row {
            case: case.to_string(),
            route,
            member,
            arn: arn.to_string(),
            body: json!({ "campaignArn": arn }),
            message: None,
        });
    }

    rows
}

/// What a client keys its error handling off, as one comparable tuple.
fn keyed(refusal: &Refusal) -> (u16, String, String, String) {
    (
        refusal.status,
        refusal.errortype.clone(),
        refusal.code.clone(),
        refusal.message.clone(),
    )
}

#[tokio::test]
async fn an_unservable_resource_earns_the_same_refusal_whatever_an_optional_member_carries() {
    let emulator = seeded();

    for row in rows() {
        let mut answers: Vec<(&str, Refusal)> = Vec::new();
        for (form, value) in member_forms() {
            let case = format!("{}: {} {form}", row.case, row.member);
            let body = in_form(&row.body, row.member, value);
            let refusal = emulator.refusal(row.route, body).await;
            assert_eq!(
                (
                    refusal.status,
                    refusal.code.as_str(),
                    refusal.errortype.as_str()
                ),
                (400, "InvalidInputException", "InvalidInputException"),
                "case {case}: expected 400 InvalidInputException in both __type and \
                 x-amzn-errortype, got {refusal:?}"
            );
            // The refusal reports the resource the request named, never the member the
            // model leaves optional.
            assert!(
                refusal.message.contains(&row.arn),
                "case {case}: expected a refusal naming {}, got {:?}",
                row.arn,
                refusal.message
            );
            assert!(
                !refusal.message.contains(row.member),
                "case {case}: an optional member must earn no refusal of its own, got {:?}",
                refusal.message
            );
            if let Some(expected) = &row.message {
                assert_eq!(
                    &refusal.message, expected,
                    "case {case}: expected the refusal {expected:?}, got {:?}",
                    refusal.message
                );
            }
            answers.push((form, refusal));
        }

        let (first_form, first) = &answers[0];
        for (form, refusal) in &answers[1..] {
            assert_eq!(
                keyed(refusal),
                keyed(first),
                "case {}: the refusal must not depend on {} ({form} vs {first_form}), got \
                 {refusal:?} and {first:?}",
                row.case,
                row.member
            );
        }
    }
}

#[tokio::test]
async fn a_resource_the_operation_serves_still_decides_the_optional_member_itself() {
    // The complement, so the invariant above is not satisfied by refusing everything the
    // same way: once the resource RESOLVES and the operation serves it, the member the
    // resource needs decides the answer — absent it is refused naming that resource,
    // supplied the request is served.
    let emulator = seeded();

    let served: [(&str, &str, &'static str, &'static str, Value); 4] = [
        (
            "GetActionRecommendations on the PERSONALIZED_ACTIONS campaign it serves",
            ACTION_RECOMMENDATIONS,
            CAMPAIGN_ACTIONS,
            "userId",
            json!({ "campaignArn": CAMPAIGN_ACTIONS, "numResults": 2 }),
        ),
        (
            "GetRecommendations on the USER_PERSONALIZATION campaign it serves",
            RECOMMENDATIONS,
            CAMPAIGN_USER,
            "userId",
            json!({ "campaignArn": CAMPAIGN_USER, "numResults": 3 }),
        ),
        (
            "GetRecommendations on the RELATED_ITEMS campaign it serves",
            RECOMMENDATIONS,
            CAMPAIGN_RELATED,
            "itemId",
            json!({ "campaignArn": CAMPAIGN_RELATED }),
        ),
        (
            "GetRecommendations on the recommender it serves",
            RECOMMENDATIONS,
            RECOMMENDER,
            "userId",
            json!({ "recommenderArn": RECOMMENDER }),
        ),
    ];

    for (case, route, arn, member, body) in served {
        let refusal = emulator
            .refusal(route, in_form(&body, member, None))
            .await;
        assert_eq!(
            (
                refusal.status,
                refusal.code.as_str(),
                refusal.errortype.as_str()
            ),
            (400, "InvalidInputException", "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException, got {refusal:?}"
        );
        assert!(
            refusal.message.starts_with(member) && refusal.message.contains(arn),
            "case {case}: expected the refusal to report {member} and name {arn}, got {:?}",
            refusal.message
        );

        let (status, parsed) = emulator
            .call(route, &in_form(&body, member, Some("subject-1")))
            .await;
        let entries = parsed
            .get("itemList")
            .or_else(|| parsed.get("actionList"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            (status, entries > 0),
            (200, true),
            "case {case}: supplying {member} must serve the request, got {status} with {parsed}"
        );
    }
}
