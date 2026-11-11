// SPDX-License-Identifier: CC0-1.0

//! The `default` prompt routers Amazon Bedrock publishes, read end to end over the same
//! `MultiServiceRouter` the emulator boots.
//!
//! # The rule
//!
//! A default prompt router exists in every account for each model LINE Amazon Bedrock
//! publishes one for, addressed by `arn:…:default-prompt-router/<provider>.<family>:<n>`
//! (`GetPromptRouter`'s `promptRouterArn` is an `@httpLabel` constrained to exactly that
//! form — `smithy_query(query="operation",operation="GetPromptRouter")`), and it reports
//! every member a `PromptRouterSummary` carries: the `@required` name, routing criteria,
//! ARN, models, fallback model, status and type, plus the modelled `description` and the
//! `createdAt`/`updatedAt` instants (`query="shape",shape="PromptRouterSummary"`).
//!
//! The models it routes between and the model it falls back to are reported as
//! CROSS-REGION INFERENCE PROFILE ARNs — the `(inference-profile|application-inference-
//! profile)/…` alternative `PromptRouterTargetModelArn`'s pattern admits beside a
//! `FoundationModelArn` (`query="shape",shape="PromptRouterTargetModel"`) — because a
//! default router routes a prompt across the regions of its geography rather than at one
//! region's copy of a model. Each target therefore names a profile this control plane
//! itself serves, which this suite checks by reading it back through
//! `GetInferenceProfile` rather than by trusting the ARN's spelling.
//!
//! # What the tables below generalise over
//!
//! The rows differ along what the rule spans: the region a request addresses (one region
//! of each published geography, two of one geography, and a region belonging to none),
//! which router is read (EVERY published one, by id, including ones no issue cited), and
//! which spelling the listing is asked for (the default type and the custom one). Nothing
//! here asserts a recorded instant: what is asserted is the RULE each member follows.

use serde_json::Value;

use crate::support::{
    date_time_seconds, emulator, label, profile_model_id, Emulator, Reply, ACCOUNT, REGION,
};

/// `PromptRouterStatus` carries exactly one value
/// (`smithy_query(query="shape",shape="PromptRouterStatus")`).
const ROUTER_AVAILABLE: &str = "AVAILABLE";

/// The two values `PromptRouterType` carries
/// (`smithy_query(query="shape",shape="PromptRouterType")`).
const TYPE_DEFAULT: &str = "default";

/// `InferenceProfileType` — the type a published profile reports.
const SYSTEM_DEFINED: &str = "SYSTEM_DEFINED";

/// The resource half of the `promptRouterArn` of every router this emulator's Amazon
/// Bedrock publishes, with the brand each one is published under and the model LINE it
/// routes within: `(router id, brand, line)`.
///
/// One row per model line AWS publishes a default router for in "Intelligent prompt
/// routing"; the ids are the `<provider>.<family>:<n>` form `PromptRouterArn`'s pattern
/// admits after `default-prompt-router/`. A line is named WITHOUT a version qualifier,
/// which is why it is not the `modelFamily` `ListFoundationModels` reports for the model a
/// router falls back to — that family is one GENERATION of the line, a relation this suite
/// checks rather than assumes. Includes the Amazon Nova router, which no earlier row of
/// this suite cited.
const PUBLISHED_ROUTERS: &[(&str, &str, &str)] = &[
    ("amazon.nova:1", "Nova", "Nova"),
    ("anthropic.claude:1", "Anthropic", "Claude"),
    ("meta.llama:1", "Meta", "LLaMA"),
];

/// Regions this suite addresses: one of each published geography, a second region of one
/// geography (the published set is the same set, under that region's own ARNs), and a
/// region belonging to no geography at all.
const GEOGRAPHY_REGIONS: &[(&str, &str)] = &[
    ("a region of the us geography", REGION),
    ("another region of the us geography", "us-east-1"),
    ("a region of the eu geography", "eu-central-1"),
    ("a region of the apac geography", "ap-northeast-1"),
];

/// A region no published geography carries, so no cross-region profile is published there.
const REGION_OF_NO_GEOGRAPHY: &str = "sa-east-1";

async fn list_routers(emu: &Emulator, region: &str, query: &str) -> Reply {
    emu.call_in_region(region, "GET", &format!("/prompt-routers?{query}"), None)
        .await
}

async fn get_router(emu: &Emulator, region: &str, arn: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/prompt-routers/{}", label(arn)),
        None,
    )
    .await
}

async fn get_profile(emu: &Emulator, region: &str, identifier: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/inference-profiles/{}", label(identifier)),
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

/// `(modelId, modelFamily)` for every model `ListFoundationModels` reports in `region` —
/// the catalogue a router's targets are checked against, rather than a second copy of it.
async fn model_families(emu: &Emulator, region: &str) -> Vec<(String, String)> {
    let listed = emu
        .call_in_region(region, "GET", "/foundation-models", None)
        .await;
    assert_eq!(
        listed.status, 200,
        "case {region:?}: ListFoundationModels must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    rows(region, &listed, "modelSummaries")
        .iter()
        .map(|row| (text(row, "modelId"), text(row, "modelFamily")))
        .collect()
}

/// The `modelArn`s a router summary names: the models it routes between, then its fallback.
fn target_arns(summary: &Value) -> Vec<String> {
    let mut arns: Vec<String> = summary
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|model| text(model, "modelArn"))
        .collect();
    arns.push(
        summary
            .get("fallbackModel")
            .map(|model| text(model, "modelArn"))
            .unwrap_or_default(),
    );
    arns
}

// ---------------------------------------------------------------------------------------
// 1. Every published router of the region is listed, and every member it carries is there.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn every_published_router_is_listed_with_every_member_it_carries() {
    let emu = emulator();
    for (case, region) in GEOGRAPHY_REGIONS {
        let listed = list_routers(&emu, region, &format!("type={TYPE_DEFAULT}")).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListPromptRouters must answer 200, got {} {:?}",
            listed.status, listed.body
        );
        let summaries = rows(case, &listed, "promptRouterSummaries");

        // The whole published set, in `promptRouterArn` order — which for these ids is the
        // order the table above lists them in.
        let listed_arns: Vec<String> = summaries
            .iter()
            .map(|summary| text(summary, "promptRouterArn"))
            .collect();
        let expected_arns: Vec<String> = PUBLISHED_ROUTERS
            .iter()
            .map(|(id, _, _)| {
                format!("arn:aws:bedrock:{region}:{ACCOUNT}:default-prompt-router/{id}")
            })
            .collect();
        assert_eq!(
            listed_arns, expected_arns,
            "case {case:?}: every published router must be listed once, in ARN order, got {:?}",
            listed.body
        );

        for (summary, (id, label, line)) in summaries.iter().zip(PUBLISHED_ROUTERS.iter()) {
            let case = format!("{case} / {id}");
            assert_eq!(
                text(summary, "status"),
                ROUTER_AVAILABLE,
                "case {case:?}: a published router is serving, got {summary:?}"
            );
            assert_eq!(
                text(summary, "type"),
                TYPE_DEFAULT,
                "case {case:?}: a published router reports the default type, got {summary:?}"
            );
            // The name and the description are the one brand the router is published
            // under: the model line AWS publishes it for.
            assert_eq!(
                text(summary, "promptRouterName"),
                format!("{label} Prompt Router"),
                "case {case:?}: the name must be the published brand of the router, got \
                 {summary:?}"
            );
            assert!(
                summary
                    .get("routingCriteria")
                    .and_then(|criteria| criteria.get("responseQualityDifference"))
                    .and_then(Value::as_f64)
                    .is_some(),
                "case {case:?}: routingCriteria is @required, got {summary:?}"
            );

            // The description states the model LINE the router routes within, named at line
            // level: a router is published for the line and not for one generation of it.
            assert_eq!(
                text(summary, "description"),
                format!("Routes requests among models in the {line} family"),
                "case {case:?}: the description must name the line the router routes \
                 within, got {summary:?}"
            );

            // The instants: a resource no call created came into being with the models it
            // routes between, and nothing has updated it since.
            let created = text(summary, "createdAt");
            let updated = text(summary, "updatedAt");
            assert!(
                !created.is_empty() && !updated.is_empty(),
                "case {case:?}: a published router carries createdAt and updatedAt, got \
                 {summary:?}"
            );
            assert_eq!(
                created, updated,
                "case {case:?}: nothing has updated a published router, so the two instants \
                 agree, got {summary:?}"
            );
            // An instant on the wire, not an epoch number: `@timestampFormat("date-time")`.
            date_time_seconds(&case, &created);
        }
    }
}

// ---------------------------------------------------------------------------------------
// 2. Every target is a cross-region inference profile this control plane serves.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn every_router_target_is_a_published_inference_profile_of_the_region() {
    let emu = emulator();
    for (case, region) in GEOGRAPHY_REGIONS {
        let listed = list_routers(&emu, region, &format!("type={TYPE_DEFAULT}")).await;
        let summaries = rows(case, &listed, "promptRouterSummaries");
        assert!(
            !summaries.is_empty(),
            "case {case:?}: the published routers of this region, got {:?}",
            listed.body
        );
        let families = model_families(&emu, region).await;

        for (summary, (_, _, line)) in summaries.iter().zip(PUBLISHED_ROUTERS.iter()) {
            let router = text(summary, "promptRouterArn");
            let targets = target_arns(summary);
            assert!(
                targets.len() >= 2,
                "case {case:?} / {router:?}: a router routes between the models it reports \
                 and falls back to one of them, got {summary:?}"
            );
            // The fallback is one of the models the router routes between: it is where a
            // prompt goes when the criteria are not met, not a fourth model.
            let (models, fallback) = targets.split_at(targets.len() - 1);
            assert!(
                models.contains(&fallback[0]),
                "case {case:?} / {router:?}: the fallback model must be one of the models, \
                 got {summary:?}"
            );

            for target in &targets {
                let case = format!("{case} / {router} / {target}");
                assert!(
                    target.starts_with(&format!(
                        "arn:aws:bedrock:{region}:{ACCOUNT}:inference-profile/"
                    )),
                    "case {case:?}: a target is the inference-profile ARN of this region, \
                     which is the form PromptRouterTargetModelArn admits for a routed model"
                );
                // Backed, not merely well-spelled: the profile the ARN names is one this
                // control plane serves, read back through GetInferenceProfile.
                let read = get_profile(&emu, region, target).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: the profile a target names must resolve, got {} {:?}",
                    read.status, read.body
                );
                assert_eq!(
                    read.get("type"),
                    Value::String(SYSTEM_DEFINED.to_string()),
                    "case {case:?}: a router routes through a profile Amazon Bedrock \
                     publishes, got {:?}",
                    read.body
                );
                assert_eq!(
                    read.text("inferenceProfileArn"),
                    *target,
                    "case {case:?}: the profile read must be the one the target named, got \
                     {:?}",
                    read.body
                );
                // And the model behind that profile is one this control plane publishes.
                let model_id = profile_model_id(target);
                assert!(
                    families.iter().any(|(id, _)| *id == model_id),
                    "case {case:?}: the routed model {model_id:?} must be one \
                     ListFoundationModels reports"
                );
            }

            // The line the router's own description names is the line the fallback model's
            // published family is a generation OF: the family carries the version
            // qualifier, the line does not.
            let fallback_model = profile_model_id(&fallback[0]);
            let fallback_family = families
                .iter()
                .find(|(id, _)| *id == fallback_model)
                .map(|(_, listed_family)| listed_family.clone())
                .unwrap_or_default();
            assert!(
                fallback_family
                    .to_lowercase()
                    .starts_with(&line.to_lowercase()),
                "case {case:?} / {router:?}: the fallback model {fallback_model:?} is \
                 reported in the {fallback_family:?} family, which must be a generation of \
                 the {line:?} line the router names"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 3. A read of one router reports what the listing reports for it, under its own ARN.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_read_of_a_published_router_agrees_with_the_listing() {
    let emu = emulator();
    for (case, region) in GEOGRAPHY_REGIONS {
        let listed = list_routers(&emu, region, &format!("type={TYPE_DEFAULT}")).await;
        let summaries = rows(case, &listed, "promptRouterSummaries");
        for summary in &summaries {
            let arn = text(summary, "promptRouterArn");
            let case = format!("{case} / {arn}");
            let read = get_router(&emu, region, &arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: GetPromptRouter must answer 200 for a published router, got \
                 {} {:?}",
                read.status, read.body
            );
            for member in [
                "promptRouterName",
                "routingCriteria",
                "description",
                "createdAt",
                "updatedAt",
                "promptRouterArn",
                "models",
                "fallbackModel",
                "status",
                "type",
            ] {
                assert_eq!(
                    summary.get(member).cloned().unwrap_or(Value::Null),
                    read.get(member),
                    "case {case:?} / {member:?}: the listed summary and the read must agree, \
                     got {:?} and {:?}",
                    summary.get(member),
                    read.get(member)
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// 4. A router is published where the profiles it routes through are, and nowhere else.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_region_with_no_published_profiles_publishes_no_router() {
    let emu = emulator();
    // The published routers route through the cross-region profiles of the region's
    // geography, so a region belonging to none has no router to route through them.
    let listed = list_routers(
        &emu,
        REGION_OF_NO_GEOGRAPHY,
        &format!("type={TYPE_DEFAULT}"),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case \"a region of no geography\": ListPromptRouters must answer 200, got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        rows("a region of no geography", &listed, "promptRouterSummaries"),
        Vec::<Value>::new(),
        "case \"a region of no geography\": no profile is published there, so no default \
         router routes through one, got {:?}",
        listed.body
    );

    // And the router of a region that HAS them is not addressable from one that has not:
    // a published router carries the ARN of the region it is offered in.
    for (id, _, _) in PUBLISHED_ROUTERS {
        let arn =
            format!("arn:aws:bedrock:{REGION_OF_NO_GEOGRAPHY}:{ACCOUNT}:default-prompt-router/{id}");
        let read = get_router(&emu, REGION_OF_NO_GEOGRAPHY, &arn).await;
        assert_eq!(
            read.status, 404,
            "case {id:?}: a router of a region that publishes none must answer 404, got {} \
             {:?}",
            read.status, read.body
        );
    }
}
