// SPDX-License-Identifier: CC0-1.0

//! The METADATA the published routing catalogues report — the descriptions, the member
//! model ARNs and the two instants of the `SYSTEM_DEFINED` cross-region inference profiles
//! and of the `default` prompt routers — read end to end over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! # The rules under test
//!
//! 1. **A published cross-region profile routes to the regions of its geography that hold
//!    its own model**, and to no others: it reports one `foundation-model` ARN per such
//!    region, in region order, and a geography holding fewer than two of them publishes no
//!    profile for that model at all, because routing across one region is not cross-region
//!    routing.
//! 2. **Its description names exactly those regions**, in the same order, as one sentence
//!    ending in a period: a reader of the sentence and a reader of `models` learn the same
//!    region set.
//! 3. **Its `createdAt` and `updatedAt` are one instant**, no earlier than the day the
//!    model it routes to launched — which `GetFoundationModel` reports as
//!    `modelLifecycle.startOfLifeTime` — and no earlier than the day Amazon Bedrock began
//!    publishing cross-region profiles, because a profile predates neither.
//! 4. **A published prompt router's description names its model LINE**, at line level: no
//!    version qualifier, so the description carries no digit, while the `modelFamily` the
//!    fallback model is reported under is a generation OF that line.
//! 5. **The two instants of a prompt-router record are rendered at millisecond width** —
//!    the `date-time` wire form those members carry — and are one and the same instant,
//!    because no operation updates a published router.
//!
//! # What the tables below generalise over
//!
//! The rows span the three geographies the catalogue publishes (of three different sizes),
//! two regions of one geography, a region of none, models offered in every region as well
//! as in one geography only, and models that launched before and after cross-region
//! profiles began being published — plus every published router, including ones no issue
//! cited.

use serde_json::Value;

use crate::support::{emulator, foundation_model_arn, label, Emulator, Reply, ACCOUNT, REGION};

/// The day Amazon Bedrock began publishing cross-region inference profiles: the lower
/// bound of every published profile's instants, as `crate::state`'s
/// `CROSS_REGION_PROFILES_PUBLISHED_AT` states it.
const PROFILES_PUBLISHED_AT: &str = "2024-08-26T00:00:00Z";

/// How many regions of a geography must hold a model for a profile to be published there.
const MIN_ROUTED_REGIONS: usize = 2;

/// `InferenceProfileModels` is `@length(min: 1, max: 5)`
/// (`smithy_query(query="shape",shape="InferenceProfileModels")`), so a published profile
/// reports at most five routed regions.
const MAX_PROFILE_MODELS: usize = 5;

/// `(case, region)` rows: one region of each geography the catalogue publishes, a second
/// region of one of them, and a region belonging to none.
const REGIONS: &[(&str, &str)] = &[
    ("a region of the us geography", REGION),
    ("another region of the us geography", "us-east-1"),
    ("a region of the eu geography", "eu-central-1"),
    ("a region of the apac geography", "ap-northeast-1"),
    ("a region of no geography", "sa-east-1"),
];

/// Base models whose published cross-region coverage the rows below pin by hand, with the
/// regions of the us geography a profile for each must route to: a model offered in every
/// region of it, one offered in two of its three, and one offered in that geography only.
/// The region sets are the ones "Supported Regions and models for inference profiles"
/// publishes for the model; the second and third rows are entries no earlier suite cited.
const US_COVERAGE: &[(&str, &[&str])] = &[
    (
        "anthropic.claude-3-haiku-20240307-v1:0",
        &["us-east-1", "us-west-2"],
    ),
    (
        "deepseek.r1-v1:0",
        &["us-east-1", "us-east-2", "us-west-2"],
    ),
    (
        "meta.llama3-3-70b-instruct-v1:0",
        &["us-east-1", "us-east-2", "us-west-2"],
    ),
];

/// Base models the catalogue publishes a profile for in the US geography and NOT in the eu
/// one, because the eu geography holds fewer than two of the regions they are offered in.
const US_ONLY_MODELS: &[&str] = &["deepseek.r1-v1:0", "meta.llama3-3-70b-instruct-v1:0"];

async fn list_profiles(emu: &Emulator, region: &str) -> Reply {
    // `typeEquals` is bound to the `type` query parameter
    // (`smithy_query(query="operation",operation="ListInferenceProfiles")`).
    emu.call_in_region(region, "GET", "/inference-profiles?type=SYSTEM_DEFINED", None)
        .await
}

async fn list_routers(emu: &Emulator, region: &str) -> Reply {
    emu.call_in_region(region, "GET", "/prompt-routers?type=default", None)
        .await
}

async fn get_router(emu: &Emulator, region: &str, arn: &str) -> Reply {
    emu.call_in_region(region, "GET", &format!("/prompt-routers/{}", label(arn)), None)
        .await
}

async fn get_model(emu: &Emulator, region: &str, model_id: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/foundation-models/{}", label(model_id)),
        None,
    )
    .await
}

fn rows(case: &str, reply: &Reply, member: &str) -> Vec<Value> {
    reply.get(member).as_array().cloned().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member} must be a present array, got {:?}",
            reply.body
        )
    })
}

fn text(row: &Value, member: &str) -> String {
    row.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `modelArn`s a profile summary reports, in the order it reports them.
fn member_arns(summary: &Value) -> Vec<String> {
    summary
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|model| text(model, "modelArn"))
        .collect()
}

/// The region a `FoundationModelArn` names.
fn arn_region(model_arn: &str) -> String {
    model_arn
        .split(':')
        .nth(3)
        .unwrap_or_default()
        .to_string()
}

/// The base `modelId` a published `inferenceProfileId` (`<geography>.<modelId>`) routes to.
fn profile_model_id(profile_id: &str) -> String {
    profile_id
        .split_once('.')
        .map(|(_, model_id)| model_id.to_string())
        .unwrap_or_default()
}

/// The model id behind a published profile is one the control plane OFFERS: the
/// `GetFoundationModel` the row above drives refuses one it does not carry, so the read is
/// a check and not a formality.
const UNOFFERED_MODEL: &str = "anthropic.claude-3-5-sonnet-29991231-v9:0";

// ---------------------------------------------------------------------------------------
// 1 & 2. A profile routes to the regions of its geography that hold its model, and says so.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_published_profile_reports_and_describes_the_regions_it_routes_to() {
    let emu = emulator();

    // The control row: the read each member is checked with REFUSES an id the catalogue
    // does not carry — with one of the two refusals the operation models,
    // `ValidationException` (400) or `ResourceNotFoundException` (404)
    // (`smithy_query(query="operation",operation="GetFoundationModel")`) — so a member
    // naming a model nothing offers would fail below.
    let unoffered = get_model(&emu, REGION, UNOFFERED_MODEL).await;
    assert!(
        unoffered.status == 400 || unoffered.status == 404,
        "case \"a model the catalogue does not carry\": GetFoundationModel must refuse it \
         with a modelled error, got {} {:?}",
        unoffered.status,
        unoffered.body
    );

    for (case, region) in REGIONS {
        let listing = list_profiles(&emu, region).await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: ListInferenceProfiles must answer 200, got {} {:?}",
            listing.status, listing.body
        );
        for summary in rows(case, &listing, "inferenceProfileSummaries") {
            let profile_id = text(&summary, "inferenceProfileId");
            let case = format!("{case} / {profile_id}");
            let arns = member_arns(&summary);
            assert!(
                (MIN_ROUTED_REGIONS..=MAX_PROFILE_MODELS).contains(&arns.len()),
                "case {case:?}: a cross-region profile routes to between \
                 {MIN_ROUTED_REGIONS} and {MAX_PROFILE_MODELS} regions, got {arns:?}"
            );

            // Every member is the foundation-model ARN of the profile's own base model in
            // one routed region, and the regions are distinct and in region order.
            let model_id = profile_model_id(&profile_id);
            let routed: Vec<String> = arns.iter().map(|arn| arn_region(arn)).collect();
            let mut sorted = routed.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                sorted, routed,
                "case {case:?}: the routed regions must be distinct and in region order, \
                 got {arns:?}"
            );
            for (arn, routed_region) in arns.iter().zip(routed.iter()) {
                assert_eq!(
                    *arn,
                    foundation_model_arn(routed_region, &model_id),
                    "case {case:?}: a member is the foundation-model ARN of this profile's \
                     base model in the region it routes to"
                );
                // Backed, not merely well-spelled: the control plane offers that model in
                // the region the member names.
                let read = get_model(&emu, routed_region, &model_id).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: the model the profile routes to must be offered in \
                     {routed_region}, got {} {:?}",
                    read.status, read.body
                );
            }

            // The description names exactly those regions, in the same order, as one
            // sentence ending in a period.
            let description = text(&summary, "description");
            let named = description
                .strip_suffix('.')
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the description must end in a period, got {description:?}")
                })
                .rsplit_once(" in ")
                .map(|(_, regions)| regions.to_string())
                .unwrap_or_default();
            let expected = match routed.as_slice() {
                [leading @ .., last] => format!("{} and {last}", leading.join(", ")),
                [] => String::new(),
            };
            assert_eq!(
                named, expected,
                "case {case:?}: the description must name the regions the profile routes \
                 to, got {description:?}"
            );
        }
    }
}

/// The coverage rows: a profile routes to exactly the regions its model is offered in, and
/// a geography holding fewer than two of them publishes no profile for that model.
#[tokio::test]
async fn a_profile_is_published_only_where_its_model_is_offered() {
    let emu = emulator();
    let listing = list_profiles(&emu, REGION).await;
    let summaries = rows("the us geography", &listing, "inferenceProfileSummaries");

    for (model_id, regions) in US_COVERAGE {
        let case = format!("the us geography / {model_id}");
        let summary = summaries
            .iter()
            .find(|summary| text(summary, "inferenceProfileId") == format!("us.{model_id}"))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the catalogue must publish a profile for this model")
            });
        let expected: Vec<String> = regions
            .iter()
            .map(|region| foundation_model_arn(region, model_id))
            .collect();
        assert_eq!(
            member_arns(summary),
            expected,
            "case {case:?}: the profile must route to the regions of the geography that \
             hold the model, got {summary:?}"
        );
    }

    // A geography that holds fewer than two of a model's regions publishes no profile for
    // it — not an empty one, and not one routing to a single region.
    let elsewhere = list_profiles(&emu, "eu-central-1").await;
    let published_in_eu: Vec<String> = rows("the eu geography", &elsewhere, "inferenceProfileSummaries")
        .iter()
        .map(|summary| text(summary, "inferenceProfileId"))
        .collect();
    for model_id in US_ONLY_MODELS {
        let case = format!("the eu geography / {model_id}");
        assert!(
            !published_in_eu.contains(&format!("eu.{model_id}")),
            "case {case:?}: no profile is published where the geography holds fewer than \
             {MIN_ROUTED_REGIONS} of the model's regions, got {published_in_eu:?}"
        );
    }
    assert!(
        !published_in_eu.is_empty(),
        "case \"the eu geography\": the catalogue publishes profiles there for the models \
         it does hold, got {:?}",
        elsewhere.body
    );
}

// ---------------------------------------------------------------------------------------
// 3. A profile's two instants are one instant, bounded below by model and capability.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_published_profile_came_into_being_with_its_model_or_with_the_capability() {
    let emu = emulator();
    for (case, region) in REGIONS {
        let listing = list_profiles(&emu, region).await;
        for summary in rows(case, &listing, "inferenceProfileSummaries") {
            let profile_id = text(&summary, "inferenceProfileId");
            let case = format!("{case} / {profile_id}");
            let created = text(&summary, "createdAt");
            let updated = text(&summary, "updatedAt");
            assert_eq!(
                created, updated,
                "case {case:?}: nothing has updated a published profile, so its two \
                 instants agree, got {summary:?}"
            );

            // The launch the control plane itself reports for the model behind the profile.
            let model_id = profile_model_id(&profile_id);
            let read = get_model(&emu, region, &model_id).await;
            let launched = read
                .get("modelDetails")
                .get("modelLifecycle")
                .and_then(|lifecycle| lifecycle.get("startOfLifeTime"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                !launched.is_empty(),
                "case {case:?}: GetFoundationModel must report the model's launch, got {:?}",
                read.body
            );
            // RFC 3339 days of the same form order lexicographically as they order in time.
            let expected = if launched.as_str() > PROFILES_PUBLISHED_AT {
                launched.clone()
            } else {
                PROFILES_PUBLISHED_AT.to_string()
            };
            assert_eq!(
                created, expected,
                "case {case:?}: a profile came into being with the capability (launched \
                 {launched:?}), so it predates neither, got {summary:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 4 & 5. A published router names its line, and renders its instants at millisecond width.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_published_router_names_its_line_and_reports_one_instant_at_millisecond_width() {
    let emu = emulator();
    for (case, region) in REGIONS {
        let listing = list_routers(&emu, region).await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: ListPromptRouters must answer 200, got {} {:?}",
            listing.status, listing.body
        );
        let summaries = rows(case, &listing, "promptRouterSummaries");
        for summary in &summaries {
            let arn = text(summary, "promptRouterArn");
            let case = format!("{case} / {arn}");

            // The description names the LINE, which carries no version qualifier.
            let description = text(summary, "description");
            let line = description
                .strip_prefix("Routes requests among models in the ")
                .and_then(|rest| rest.strip_suffix(" family"))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the description must name the line the router \
                         routes within, got {description:?}"
                    )
                })
                .to_string();
            assert!(
                !line.is_empty() && !line.chars().any(char::is_numeric),
                "case {case:?}: a line is named without a version qualifier, got {line:?}"
            );

            // One instant, rendered with a three-digit fraction: the wire form these two
            // members carry.
            let created = text(summary, "createdAt");
            let updated = text(summary, "updatedAt");
            assert_eq!(
                created, updated,
                "case {case:?}: nothing has updated a published router, so its two \
                 instants agree, got {summary:?}"
            );
            for (member, rendered) in [("createdAt", &created), ("updatedAt", &updated)] {
                let fraction = rendered
                    .strip_suffix('Z')
                    .and_then(|instant| instant.split_once('.'))
                    .map(|(_, fraction)| fraction.to_string())
                    .unwrap_or_default();
                assert_eq!(
                    fraction.len(),
                    3,
                    "case {case:?} / {member}: a date-time of a prompt-router record keeps \
                     a three-digit fraction, got {rendered:?}"
                );
                assert!(
                    fraction.chars().all(|digit| digit.is_ascii_digit()),
                    "case {case:?} / {member}: the fraction must be digits, got {rendered:?}"
                );
            }

            // And the read of that one router renders the same text the listing does.
            let read = get_router(&emu, region, &arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetPromptRouter must answer 200, got {} {:?}",
                read.status, read.body
            );
            for member in ["description", "createdAt", "updatedAt"] {
                assert_eq!(
                    summary.get(member).cloned().unwrap_or(Value::Null),
                    read.get(member),
                    "case {case:?} / {member}: the listing and the read must report the \
                     same text, got {:?} and {:?}",
                    summary.get(member),
                    read.get(member)
                );
            }
        }
    }

    // The line a router names is NOT the version-qualified family of the model it falls
    // back to: that family is one generation of the line.
    let listing = list_routers(&emu, REGION).await;
    let summaries = rows("the us geography", &listing, "promptRouterSummaries");
    assert!(
        !summaries.is_empty(),
        "case \"the us geography\": the published routers of this region, got {:?}",
        listing.body
    );
    for summary in &summaries {
        let arn = text(summary, "promptRouterArn");
        let fallback = summary
            .get("fallbackModel")
            .map(|model| text(model, "modelArn"))
            .unwrap_or_default();
        assert!(
            fallback.starts_with(&format!(
                "arn:aws:bedrock:{REGION}:{ACCOUNT}:inference-profile/"
            )),
            "case {arn:?}: the fallback model is reported as an inference-profile ARN, got \
             {fallback:?}"
        );
        let model_id = profile_model_id(
            fallback
                .rsplit_once("inference-profile/")
                .map(|(_, id)| id)
                .unwrap_or_default(),
        );
        let read = get_model(&emu, REGION, &model_id).await;
        let family = read
            .get("modelDetails")
            .get("modelFamily")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let line = text(summary, "description")
            .strip_prefix("Routes requests among models in the ")
            .and_then(|rest| rest.strip_suffix(" family"))
            .unwrap_or_default()
            .to_string();
        assert!(
            family
                .to_lowercase()
                .starts_with(&line.to_lowercase()),
            "case {arn:?}: the fallback model {model_id:?} is reported in the {family:?} \
             family, which must be a generation of the {line:?} line"
        );
    }
}
