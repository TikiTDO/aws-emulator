// SPDX-License-Identifier: CC0-1.0

//! The rule this suite pins: **a request member is judged by EVERY constraint the model
//! puts on it, so the empty string is read against that member's own constraints and not
//! against `@length` alone.**
//!
//! `smithy_query(query="shape",shape="Arn,UserID,ItemID")` reports the two classes this
//! service's string members fall into:
//!
//! * `Arn` — the shape `campaignArn`, `recommenderArn`, `filterArn` and a promotion's
//!   `filterArn` all target — carries `length 0..256` AND
//!   `pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$`. The empty string satisfies the
//!   length bound and misses the pattern, so an empty (or blank, or truncated) ARN is a
//!   BOUNDARY `@pattern` violation: HTTP 400 `ValidationException` whose clause names the
//!   member and the expression the value has to satisfy. No handler ever runs, so no
//!   handler resolves an empty ARN and no refusal claims that a resource the request
//!   never named "does not exist or is not active yet".
//! * `UserID` / `ItemID` — `length 0..256` and no pattern. The empty string satisfies
//!   every constraint they carry, so it reaches the handler, which reads it as the member
//!   not being supplied (`crate::state::present`) and refuses NAMING THE MEMBER.
//!
//! The suite spans both classes: every operation × every ARN-carrying member × a spread
//! of values the length bound admits and the pattern refuses, and every length-only
//! personalization member of every operation. The length bound is exercised at both ends,
//! so a value past it is reported as the length violation it is rather than as a pattern
//! one, and a value at it gets past the boundary to the data plane's own refusal.

use serde_json::{json, Map, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, Campaign, RecipeType, Recommender,
};

use crate::support::{
    actions, entries, items, recommendation_id, Emulator, ACTION_RECOMMENDATIONS,
    PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// A registered `USER_PERSONALIZATION` campaign.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/user";
/// A registered `RELATED_ITEMS` campaign, which needs `itemId` instead of `userId`.
const CAMPAIGN_RELATED: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/related";
/// A registered `PERSONALIZED_RANKING` campaign — the one `GetPersonalizedRanking` serves.
const CAMPAIGN_RANKING: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/ranking";
/// A registered `PERSONALIZED_ACTIONS` campaign.
const CAMPAIGN_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/actions";
/// A registered domain recommender.
const RECOMMENDER: &str = "arn:aws:personalize:us-east-1:000000000000:recommender/top-picks";
/// The recommender's use case, one of `state::RECOMMENDER_USE_CASES`; this one requires
/// `userId`.
const USE_CASE: &str = "Top picks for you";

/// The user a row that is not varying `userId` supplies.
const USER: &str = "user-1";

/// How many entries the seeded catalogs hold.
const CATALOG_SIZE: usize = 6;

/// The modelled `@length` ceiling on `Arn`, `UserID` and `ItemID`
/// (`smithy_query(query="shape",shape="Arn")` — `length 0..256`).
const MAX_ARN_LENGTH: usize = 256;

/// The modelled `@pattern` on `Arn`, as the model spells it.
const ARN_PATTERN: &str = r"^arn:([a-z\d-]+):personalize:.*:.*:.+$";

/// That same expression without the anchors this service's clause drops.
const ARN_PATTERN_UNANCHORED: &str = r"arn:([a-z\d-]+):personalize:.*:.*:.+";

/// An emulator whose registry holds one resource of every kind, so the only thing a row
/// leaves unusable is the member it is varying.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    for (arn, recipe_type) in [
        (CAMPAIGN_USER, RecipeType::UserPersonalization),
        (CAMPAIGN_RELATED, RecipeType::RelatedItems),
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
            Recommender::new(RECOMMENDER, "top-picks", USE_CASE, items(CATALOG_SIZE))
        ),
        "the recommender seam must register the recommender"
    );
    emulator
}

/// The clause this service states for a value at `member` that misses [`ARN_PATTERN`].
fn pattern_clause(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: Member \
         must satisfy regular expression pattern: {ARN_PATTERN_UNANCHORED}"
    )
}

/// The sentence the data plane answers for an ARN it ADMITTED and cannot resolve. For an
/// empty ARN it would name nothing at all, which is why no row below may earn it.
fn unresolvable_message(arn: &str) -> String {
    format!("{arn} does not exist or is not active yet.")
}

/// How a request carries the ARN value under test.
type Build = fn(&str) -> Value;

/// One ARN-carrying member of this service: the route that takes it, the path a boundary
/// clause names it by, and the request that carries it.
struct ArnMember {
    case: &'static str,
    route: &'static str,
    path: &'static str,
    build: Build,
}

fn recommendations_campaign(value: &str) -> Value {
    json!({ "campaignArn": value, "userId": USER })
}

fn recommendations_recommender(value: &str) -> Value {
    json!({ "recommenderArn": value, "userId": USER })
}

fn recommendations_filter(value: &str) -> Value {
    json!({ "campaignArn": CAMPAIGN_USER, "userId": USER, "filterArn": value })
}

fn recommendations_promotion_filter(value: &str) -> Value {
    json!({
        "campaignArn": CAMPAIGN_USER,
        "userId": USER,
        "promotions": [{ "name": "promo", "percentPromotedItems": 50, "filterArn": value }],
    })
}

fn ranking_campaign(value: &str) -> Value {
    json!({ "campaignArn": value, "userId": USER, "inputList": ["item-1", "item-2"] })
}

fn ranking_filter(value: &str) -> Value {
    json!({
        "campaignArn": CAMPAIGN_RANKING,
        "userId": USER,
        "inputList": ["item-1", "item-2"],
        "filterArn": value,
    })
}

fn action_campaign(value: &str) -> Value {
    json!({ "campaignArn": value, "userId": USER })
}

fn action_filter(value: &str) -> Value {
    json!({ "campaignArn": CAMPAIGN_ACTIONS, "userId": USER, "filterArn": value })
}

/// Every member of this service that targets `Arn`, on every operation that takes it.
///
/// A member inside a list of structures is named the way the validation envelope names it
/// — the list member, the entry's 1-based index, the list's `member` element and then the
/// struct member — which is what `promotions[0].filterArn` reads as.
fn arn_members() -> Vec<ArnMember> {
    vec![
        ArnMember {
            case: "GetRecommendations campaignArn",
            route: RECOMMENDATIONS,
            path: "campaignArn",
            build: recommendations_campaign,
        },
        ArnMember {
            case: "GetRecommendations recommenderArn",
            route: RECOMMENDATIONS,
            path: "recommenderArn",
            build: recommendations_recommender,
        },
        ArnMember {
            case: "GetRecommendations filterArn",
            route: RECOMMENDATIONS,
            path: "filterArn",
            build: recommendations_filter,
        },
        ArnMember {
            case: "GetRecommendations promotions[0].filterArn",
            route: RECOMMENDATIONS,
            path: "promotions.1.member.filterArn",
            build: recommendations_promotion_filter,
        },
        ArnMember {
            case: "GetPersonalizedRanking campaignArn",
            route: PERSONALIZE_RANKING,
            path: "campaignArn",
            build: ranking_campaign,
        },
        ArnMember {
            case: "GetPersonalizedRanking filterArn",
            route: PERSONALIZE_RANKING,
            path: "filterArn",
            build: ranking_filter,
        },
        ArnMember {
            case: "GetActionRecommendations campaignArn",
            route: ACTION_RECOMMENDATIONS,
            path: "campaignArn",
            build: action_campaign,
        },
        ArnMember {
            case: "GetActionRecommendations filterArn",
            route: ACTION_RECOMMENDATIONS,
            path: "filterArn",
            build: action_filter,
        },
    ]
}

/// A value of exactly `length` characters that matches [`ARN_PATTERN`].
fn arn_of_length(length: usize) -> String {
    let prefix = "arn:aws:personalize:us-east-1:000000000000:campaign/";
    assert!(
        length > prefix.len(),
        "a {length}-character ARN cannot carry the {}-character prefix",
        prefix.len()
    );
    format!("{prefix}{}", "c".repeat(length - prefix.len()))
}

/// Values the `@length 0..256` bound ADMITS and the `@pattern` refuses — the empty string
/// among them, so it is one row of the class rather than the whole test.
fn values_inside_the_length_bound_and_outside_the_pattern() -> Vec<(&'static str, String)> {
    vec![
        ("the empty string", String::new()),
        ("a single space", " ".to_string()),
        ("a tab", "\t".to_string()),
        ("the ARN scheme alone", "arn:".to_string()),
        (
            "an ARN with no resource part",
            "arn:aws:personalize:us-east-1:000000000000:".to_string(),
        ),
        (
            "a value at the 256-character length bound that is no ARN",
            "x".repeat(MAX_ARN_LENGTH),
        ),
    ]
}

#[tokio::test]
async fn a_value_the_length_bound_admits_and_the_pattern_refuses_names_the_member_at_the_boundary()
{
    assert_eq!(
        ARN_PATTERN,
        format!("^{ARN_PATTERN_UNANCHORED}$"),
        "the unanchored expression must be the modelled pattern minus its anchors, got \
         {ARN_PATTERN_UNANCHORED:?} against {ARN_PATTERN:?}"
    );
    let emulator = seeded();

    for (form, value) in values_inside_the_length_bound_and_outside_the_pattern() {
        assert!(
            value.len() <= MAX_ARN_LENGTH,
            "case {form}: the row must stay inside the modelled length bound, got {} characters",
            value.len()
        );
        for member in arn_members() {
            let case = format!("{} with {form}", member.case);
            let refusal = emulator.refusal(member.route, (member.build)(&value)).await;
            assert_eq!(
                (
                    refusal.status,
                    refusal.code.as_str(),
                    refusal.errortype.as_str()
                ),
                (400, "ValidationException", "ValidationException"),
                "case {case}: expected the boundary's 400 ValidationException in both __type and \
                 x-amzn-errortype, got {refusal:?}"
            );
            assert_eq!(
                refusal.message,
                pattern_clause(member.path),
                "case {case}: expected the clause to name the member and the expression, got {:?}",
                refusal.message
            );
            assert_ne!(
                refusal.message,
                unresolvable_message(&value),
                "case {case}: a value no handler ever receives must not be reported as an \
                 unresolved resource, got {:?}",
                refusal.message
            );
        }
    }
}

#[tokio::test]
async fn a_value_past_the_length_bound_is_reported_as_the_length_violation_it_is() {
    // The complement of the rule: the pattern is not the only constraint the member
    // carries, so a value that SATISFIES the pattern and misses the length bound is
    // answered as the length violation — and one AT the bound gets past the boundary
    // altogether, to a refusal that names the ARN the request sent.
    let emulator = seeded();

    let past = arn_of_length(MAX_ARN_LENGTH + 1);
    for member in arn_members() {
        let case = format!("{} one character past the length bound", member.case);
        let refusal = emulator.refusal(member.route, (member.build)(&past)).await;
        assert_eq!(
            (refusal.status, refusal.code.as_str()),
            (400, "ValidationException"),
            "case {case}: expected the boundary's 400 ValidationException, got {refusal:?}"
        );
        for fragment in [
            format!("at '{}' failed to satisfy constraint", member.path),
            format!("Member must have length less than or equal to {MAX_ARN_LENGTH}"),
        ] {
            assert!(
                refusal.message.contains(&fragment),
                "case {case}: expected the clause to state {fragment:?}, got {:?}",
                refusal.message
            );
        }
    }

    let at_bound = arn_of_length(MAX_ARN_LENGTH);
    for (case, route, body) in [
        (
            "GetRecommendations campaignArn at the length bound",
            RECOMMENDATIONS,
            recommendations_campaign(&at_bound),
        ),
        (
            "GetPersonalizedRanking campaignArn at the length bound",
            PERSONALIZE_RANKING,
            ranking_campaign(&at_bound),
        ),
        (
            "GetActionRecommendations campaignArn at the length bound",
            ACTION_RECOMMENDATIONS,
            action_campaign(&at_bound),
        ),
    ] {
        let refusal = emulator.refusal(route, body).await;
        assert_eq!(
            (refusal.status, refusal.code.as_str()),
            (400, "InvalidInputException"),
            "case {case}: an admitted ARN is refused by the data plane, got {refusal:?}"
        );
        assert_eq!(
            refusal.message,
            unresolvable_message(&at_bound),
            "case {case}: expected the refusal to name the ARN the request sent, got {:?}",
            refusal.message
        );
    }
}

/// One personalization member whose only modelled constraint is `length` from 0, so the
/// empty string satisfies every constraint it carries and reaches the handler.
struct LengthOnlyMember {
    case: &'static str,
    route: &'static str,
    /// The member the row sends empty.
    member: &'static str,
    /// The request WITHOUT that member.
    base: Value,
    /// What the handler answers when the member names nobody.
    expected: String,
    /// Whether the model marks the member `@required`, in which case leaving it out is
    /// the boundary's refusal rather than the handler's.
    required: bool,
}

/// `base` with `member` set to `value`.
fn with(base: &Value, member: &str, value: &str) -> Value {
    let mut object: Map<String, Value> = base
        .as_object()
        .expect("a request body is a JSON object")
        .clone();
    object.insert(member.to_string(), Value::from(value));
    Value::Object(object)
}

fn length_only_members() -> Vec<LengthOnlyMember> {
    vec![
        LengthOnlyMember {
            case: "GetPersonalizedRanking userId",
            route: PERSONALIZE_RANKING,
            member: "userId",
            base: json!({ "campaignArn": CAMPAIGN_RANKING, "inputList": ["item-1"] }),
            expected: "userId is required: it is the user the campaign provides a personalized \
                       ranking for."
                .to_string(),
            required: true,
        },
        LengthOnlyMember {
            case: "GetRecommendations userId on a USER_PERSONALIZATION campaign",
            route: RECOMMENDATIONS,
            member: "userId",
            base: json!({ "campaignArn": CAMPAIGN_USER }),
            expected: format!(
                "userId is required to get recommendations from campaign {CAMPAIGN_USER} \
                 (USER_PERSONALIZATION)."
            ),
            required: false,
        },
        LengthOnlyMember {
            case: "GetRecommendations userId on a recommender whose use case needs one",
            route: RECOMMENDATIONS,
            member: "userId",
            base: json!({ "recommenderArn": RECOMMENDER }),
            expected: format!(
                "userId is required to get recommendations from recommender {RECOMMENDER} \
                 ({USE_CASE})."
            ),
            required: false,
        },
        LengthOnlyMember {
            case: "GetRecommendations itemId on a RELATED_ITEMS campaign",
            route: RECOMMENDATIONS,
            member: "itemId",
            base: json!({ "campaignArn": CAMPAIGN_RELATED }),
            expected: format!(
                "itemId is required to get recommendations from campaign {CAMPAIGN_RELATED} \
                 (RELATED_ITEMS)."
            ),
            required: false,
        },
        LengthOnlyMember {
            case: "GetActionRecommendations userId on a PERSONALIZED_ACTIONS campaign",
            route: ACTION_RECOMMENDATIONS,
            member: "userId",
            base: json!({ "campaignArn": CAMPAIGN_ACTIONS }),
            expected: format!(
                "userId is required to get action recommendations from campaign \
                 {CAMPAIGN_ACTIONS} (PERSONALIZED_ACTIONS)."
            ),
            required: false,
        },
    ]
}

#[tokio::test]
async fn a_member_the_model_bounds_only_by_length_reaches_the_handler_and_reads_empty_as_absent() {
    let emulator = seeded();

    for row in length_only_members() {
        let case = format!("{} sent empty", row.case);
        let refusal = emulator
            .refusal(row.route, with(&row.base, row.member, ""))
            .await;
        assert_eq!(
            (
                refusal.status,
                refusal.code.as_str(),
                refusal.errortype.as_str()
            ),
            (400, "InvalidInputException", "InvalidInputException"),
            "case {case}: expected the handler's 400 InvalidInputException in both __type and \
             x-amzn-errortype, got {refusal:?}"
        );
        assert_eq!(
            refusal.message, row.expected,
            "case {case}: expected the refusal to name the member, got {:?}",
            refusal.message
        );
        assert!(
            refusal.message.contains(row.member),
            "case {case}: the refusal must name {}, got {:?}",
            row.member,
            refusal.message
        );
        assert_ne!(
            refusal.message,
            unresolvable_message(""),
            "case {case}: a member that names nobody must not be reported as an unresolved \
             resource, got {:?}",
            refusal.message
        );

        if !row.required {
            // "Read as absent" is the whole rule: the member sent empty earns exactly the
            // refusal the member left out earns.
            let left_out = emulator.refusal(row.route, row.base.clone()).await;
            assert_eq!(
                (left_out.status, left_out.code.as_str(), left_out.message),
                (refusal.status, refusal.code.as_str(), refusal.message),
                "case {}: the member sent empty must be read exactly as the member left out",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_request_whose_members_are_all_well_formed_is_still_served() {
    // The refusals above are not unconditional: the same operations answer 200 once every
    // member carries a value its own constraints admit.
    let emulator = seeded();

    for (case, route, member, body) in [
        (
            "GetRecommendations on a registered campaign",
            RECOMMENDATIONS,
            "itemList",
            json!({ "campaignArn": CAMPAIGN_USER, "userId": USER, "numResults": 3 }),
        ),
        (
            "GetRecommendations on a registered recommender",
            RECOMMENDATIONS,
            "itemList",
            json!({ "recommenderArn": RECOMMENDER, "userId": USER }),
        ),
        (
            "GetPersonalizedRanking on a registered campaign",
            PERSONALIZE_RANKING,
            "personalizedRanking",
            ranking_campaign(CAMPAIGN_RANKING),
        ),
        (
            "GetActionRecommendations on a registered campaign",
            ACTION_RECOMMENDATIONS,
            "actionList",
            action_campaign(CAMPAIGN_ACTIONS),
        ),
    ] {
        let (status, parsed) = emulator.call(route, &body).await;
        assert_eq!(
            status, 200,
            "case {case}: a request every constraint admits must be served, got {status} with \
             {parsed}"
        );
        // Served means answered FROM the seeded catalog, not merely answered: the list the
        // operation models carries entries, and the call minted a `recommendationId`.
        let served = entries(&parsed, member);
        assert!(
            !served.is_empty(),
            "case {case}: expected {member} to report the seeded catalog, got {parsed}"
        );
        recommendation_id(case, &parsed);
    }
}
