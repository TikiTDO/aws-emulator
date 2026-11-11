// SPDX-License-Identifier: CC0-1.0

//! The catalogues of ROUTING resources Amazon Bedrock publishes itself: the `default`
//! prompt routers and the `SYSTEM_DEFINED` cross-region inference profiles.
//!
//! Both are published data rather than anything a request brings into being — this service
//! has no operation that creates one — so, like the model catalogue they are built over,
//! these rows are the same for every account and no handler writes one.

use super::*;

/// `PromptRouterType` — the type of a router Amazon Bedrock itself publishes.
///
/// The other of the two values the enum carries, beside [`PROMPT_ROUTER_TYPE_CUSTOM`].
pub const PROMPT_ROUTER_TYPE_DEFAULT: &str = "default";

/// The `responseQualityDifference` a published router reports.
///
/// `RoutingCriteria$responseQualityDifference` is modelled `@range(min: 0, max: 100)` and
/// is `@required`, so a published router reports one; AWS publishes no tolerance for its
/// own routers, so this is the minimum that range admits — the strictest criteria, under
/// which a prompt is routed to the smaller model only when its answer is no worse.
pub const PUBLISHED_ROUTING_QUALITY_DIFFERENCE: f64 = 0.0;

/// One prompt router Amazon Bedrock publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultPromptRouter {
    /// The resource half of the router's `PromptRouterArn`, whose pattern admits
    /// `[a-zA-Z0-9-:.]+` there ([`prompt_router_arn`]). Spelled `<provider>.<family>:<n>`,
    /// the form AWS publishes a default router's ARN under.
    pub id: &'static str,
    /// The BRAND the router is published under: the publisher's own name for the model
    /// line it routes within — a third-party provider's name for that provider's line, and
    /// the line's own name for a line AWS publishes itself, because AWS does not brand its
    /// own models by its provider name. It is the same split the published cross-region
    /// profiles name their models by ([`cross_region_model_brand`]).
    ///
    /// The router's `promptRouterName` is built from it ([`default_prompt_router_name`]),
    /// so the brand a router is published under is stated once.
    pub label: &'static str,
    /// The model LINE the router routes within, as its publisher brands that line: the
    /// family name WITHOUT the per-generation version qualifier, which is what the router's
    /// description names ([`default_prompt_router_description`]).
    ///
    /// Published data of the ROUTER rather than of one of its targets, and not derivable
    /// from them: a target's [`FoundationModel::model_family`] is the version-qualified
    /// family of that one model ("the 3.5 generation of the line"), while a router is
    /// published for the line itself, and the line is spelled as its publisher spells it,
    /// which re-casing a family name cannot produce.
    pub line: &'static str,
    /// The instant the router came into being: `createdAt`, and — because no operation of
    /// this service updates a published router — `updatedAt` too.
    ///
    /// Published data of the router, as RFC 3339. The day AWS published the router is a
    /// fact about the ROUTER and not about the models it routes between: a router is
    /// published when intelligent prompt routing began offering it, which is no earlier
    /// than its targets' launches but need not be the day any one of them launched.
    pub published_at: &'static str,
    /// The `modelId`s the router routes between, in the order the summary reports them —
    /// the smaller model first, because the criteria are what sends a prompt UP to the
    /// larger one. The set AWS publishes for the router; the last of them is the fallback,
    /// the strongest model a prompt is routed up to — the model a caller gets when the
    /// criteria are not met. The two need not share a `modelFamily`: AWS publishes a router
    /// across two generations of a provider's line as readily as within one, which is why
    /// the line the description names is the row's own [`DefaultPromptRouter::line`].
    /// Every id is a row of [`FOUNDATION_MODELS`] that also carries a cross-region profile
    /// ([`CROSS_REGION_MODELS`]), because that profile is what a target is reported as.
    pub models: &'static [&'static str],
    /// The `modelId` of the model the router falls back to when the criteria are not met;
    /// one of [`DefaultPromptRouter::models`].
    pub fallback_model: &'static str,
}

/// The prompt routers this emulator's Amazon Bedrock publishes, in `id` order — the order
/// their ARNs sort in, which is the order a listing pages through.
///
/// The ids, brands, lines, publication days and model sets are the ones AWS publishes in
/// "Intelligent prompt routing": one router per provider LINE, routing a prompt from a
/// smaller model to the stronger model it falls back to — which AWS publishes ACROSS two
/// generations of a line as readily as within one, so a row's two models need not share a
/// `modelFamily`. The model ids are the rows [`FOUNDATION_MODELS`] carries, so the two
/// catalogs agree about which models exist.
pub const DEFAULT_PROMPT_ROUTERS: &[DefaultPromptRouter] = &[
    DefaultPromptRouter {
        id: "amazon.nova:1",
        label: "Nova",
        line: "Nova",
        published_at: "2025-04-21T07:00:00Z",
        models: &["amazon.nova-lite-v1:0", "amazon.nova-pro-v1:0"],
        fallback_model: "amazon.nova-pro-v1:0",
    },
    DefaultPromptRouter {
        id: "anthropic.claude:1",
        label: "Anthropic",
        line: "Claude",
        published_at: "2024-11-20T00:00:00Z",
        models: &[
            "anthropic.claude-3-haiku-20240307-v1:0",
            "anthropic.claude-3-5-sonnet-20240620-v1:0",
        ],
        fallback_model: "anthropic.claude-3-5-sonnet-20240620-v1:0",
    },
    DefaultPromptRouter {
        id: "meta.llama:1",
        label: "Meta",
        line: "LLaMA",
        published_at: "2024-11-20T00:00:00Z",
        models: &[
            "meta.llama3-1-8b-instruct-v1:0",
            "meta.llama3-1-70b-instruct-v1:0",
        ],
        fallback_model: "meta.llama3-1-70b-instruct-v1:0",
    },
];

/// The `promptRouterName` a published router reports: the brand it is published under,
/// then what it is.
///
/// `PromptRouterName` is `@required` (len 1..64, `^([0-9a-zA-Z][ _-]?)+$`), so a published
/// router carries one; it is built from [`DefaultPromptRouter::label`] rather than written
/// out per router, so the brand a router is published under is stated once.
#[must_use]
pub fn default_prompt_router_name(router: &DefaultPromptRouter) -> String {
    format!("{} Prompt Router", router.label)
}

/// The `description` a published router reports: the model LINE it routes within.
///
/// `PromptRouterDescription` is modelled (len 1..200, `^([0-9a-zA-Z:.][ _-]?)+$`) and a
/// published router carries one, so it is stated for every row by ONE rule rather than
/// written out per router: what a default router IS, is the line of models it routes a
/// prompt within, named at LINE level as its publisher brands that line
/// ([`DefaultPromptRouter::line`]) — without a version qualifier, because a router is
/// published for the line and not for one generation of it.
#[must_use]
pub fn default_prompt_router_description(router: &DefaultPromptRouter) -> String {
    format!("Routes requests among models in the {} family", router.line)
}

/// Every prompt router Amazon Bedrock publishes in `region`, as the `PromptRouterSummary`
/// a listing carries, in `promptRouterArn` order.
///
/// Each one reports the models it routes between as `PromptRouterTargetModel`s carrying
/// the ARN of the CROSS-REGION INFERENCE PROFILE of this region's geography for that model
/// ([`system_defined_inference_profiles`]) — the `inference-profile` form
/// `PromptRouterTargetModelArn` admits beside a `FoundationModelArn` — because a default
/// router routes a prompt across the regions of its geography rather than at one region's
/// copy of a model, and the profile is the resource that routing is entitled through.
/// A target therefore names a profile this control plane itself publishes: a router is
/// offered in a region exactly while every model it routes between has a published profile
/// there, which is why a region of no geography publishes none.
///
/// `createdAt` and `updatedAt` are the instant the router was published
/// ([`DefaultPromptRouter::published_at`]), and the same instant for both: no operation of
/// this service updates a published router, so nothing has moved `updatedAt` off the
/// instant the router came into being.
#[must_use]
pub fn default_prompt_routers(region: &str) -> Vec<PromptRouterSummary> {
    let published = system_defined_inference_profiles(region);
    let target = |model_id: &str| {
        let id = cross_region_profile_id(region, model_id)?;
        published
            .iter()
            .find(|profile| profile.inference_profile_id == id)
            .map(|profile| PromptRouterTargetModel {
                model_arn: profile.inference_profile_arn.clone(),
            })
    };
    DEFAULT_PROMPT_ROUTERS
        .iter()
        .filter_map(|router| {
            let models: Vec<PromptRouterTargetModel> = router
                .models
                .iter()
                .map(|model_id| target(model_id))
                .collect::<Option<_>>()?;
            let published_at = published_instant(router.published_at);
            Some(PromptRouterSummary {
                prompt_router_name: default_prompt_router_name(router),
                routing_criteria: RoutingCriteria {
                    response_quality_difference: PUBLISHED_ROUTING_QUALITY_DIFFERENCE,
                },
                description: Some(default_prompt_router_description(router)),
                created_at: published_at,
                updated_at: published_at,
                prompt_router_arn: prompt_router_arn(region, router.id),
                models,
                fallback_model: target(router.fallback_model)?,
                status: PROMPT_ROUTER_AVAILABLE.to_string(),
                r#type: PROMPT_ROUTER_TYPE_DEFAULT.to_string(),
            })
        })
        .collect()
}

/// The prompt router Amazon Bedrock publishes in `region` under `arn`, if it publishes one.
///
/// `GetPromptRouter` addresses a router by its `PromptRouterArn` alone, and a published
/// router carries the ARN of the region it is offered in, so one of another region names
/// another account-region's router.
#[must_use]
pub fn default_prompt_router(arn: &str, region: &str) -> Option<PromptRouterSummary> {
    default_prompt_routers(region)
        .into_iter()
        .find(|published| published.prompt_router_arn == arn)
}

// =============================================================================
// The published cross-region inference profile catalog
// =============================================================================
//
// Amazon Bedrock publishes the SYSTEM_DEFINED inference profiles itself — "the inference
// profile is defined by Amazon Bedrock. You can route inference requests across regions
// with these inference profiles"
// (`smithy_query(query="shape",shape="InferenceProfileSummary")`) — and there is no
// operation that creates one, so these rows are published data the same way
// [`FOUNDATION_MODELS`] is: the same for every account, written by no handler.

/// `InferenceProfileType` — the type of a profile Amazon Bedrock itself defines.
///
/// The other of the two values `InferenceProfileType` carries, beside
/// [`INFERENCE_PROFILE_TYPE_APPLICATION`].
pub const INFERENCE_PROFILE_TYPE_SYSTEM_DEFINED: &str = "SYSTEM_DEFINED";

/// One geography a cross-region inference profile routes invocations within.
///
/// A geography is the unit the published profiles are cut along: a profile exists once per
/// geography per model, is offered in that geography's regions, and routes requests to
/// those same regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InferenceProfileGeography {
    /// The leading dot-separated segment of the published `inferenceProfileId`
    /// (`us.<modelId>`), which is also what `ops/create_inference_profile.rs` reads back
    /// off such an id to find the model behind it.
    pub prefix: &'static str,
    /// How the published `inferenceProfileName` spells the geography, ahead of the model's
    /// own brand ([`cross_region_model_brand`]).
    pub label: &'static str,
    /// The regions of the geography: where the profile is offered, and the regions it may
    /// route invocations to — which are the ones its own model is offered in
    /// ([`CrossRegionModel::regions`]). At most five, because `InferenceProfileModels` is
    /// `@length(min: 1, max: 5)` and a published profile reports one model per routed
    /// region.
    pub regions: &'static [&'static str],
}

/// The geographies this emulator's Amazon Bedrock publishes cross-region profiles for, in
/// `prefix` order — the order their profile ids sort in.
///
/// The prefixes and the regions are the ones AWS publishes in "Supported Regions and
/// models for inference profiles": a geography's profile routes to the commercial regions
/// of that geography. A SUBSET of what AWS offers, spanning geographies of three different
/// sizes, because the listing rule — not the catalog's size — is what this emulator has to
/// get right.
pub const INFERENCE_PROFILE_GEOGRAPHIES: &[InferenceProfileGeography] = &[
    InferenceProfileGeography {
        prefix: "apac",
        label: "APAC",
        regions: &[
            "ap-northeast-1",
            "ap-northeast-2",
            "ap-south-1",
            "ap-southeast-1",
            "ap-southeast-2",
        ],
    },
    InferenceProfileGeography {
        prefix: "eu",
        label: "EU",
        regions: &["eu-central-1", "eu-north-1", "eu-west-1", "eu-west-3"],
    },
    InferenceProfileGeography {
        prefix: "us",
        label: "US",
        regions: &["us-east-1", "us-east-2", "us-west-2"],
    },
];

/// One base model this emulator's Amazon Bedrock publishes a cross-region inference
/// profile for, with the regions that model is offered in.
///
/// The regions travel WITH the model because they are what a published profile reports and
/// describes: a profile fans a request out to the regions of its geography that hold a copy
/// of its own model, which is a per-model published fact and not a property of the
/// geography — two models of one geography are offered in different subsets of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossRegionModel {
    /// The `modelId` of the base model, a row of [`FOUNDATION_MODELS`].
    pub model_id: &'static str,
    /// Every region the model is offered in, in region order.
    ///
    /// A profile for this model is published in a geography while the geography holds at
    /// least two of them — a profile that could route to one region only would route
    /// across no region at all — and the ones it holds are the regions that profile reports
    /// a model for and names in its description.
    pub regions: &'static [&'static str],
}

/// The base models this emulator's Amazon Bedrock publishes a cross-region inference
/// profile for, in `modelId` order.
///
/// The models AWS lists as supported for cross-region inference on "Supported Regions and
/// models for inference profiles", with the regions that page publishes each model in,
/// restricted to the ids [`FOUNDATION_MODELS`] carries so no published profile routes to a
/// model this control plane does not offer — an id with no row publishes no profile rather
/// than one naming a model nothing serves. A SUBSET of what AWS offers, spanning three
/// providers and models offered in one geography as well as in all of them, and CLOSED
/// under [`DEFAULT_PROMPT_ROUTERS`]: every model a published router routes between has a
/// profile here, because that profile is what the router reports as its target.
///
/// Only the ids and the regions are written here. Everything else a profile reports about
/// its model — the brand its name carries, the launch its instants are bounded below by —
/// is read off that model's published row, so the two catalogs cannot disagree about one
/// model.
pub const CROSS_REGION_MODELS: &[CrossRegionModel] = &[
    CrossRegionModel {
        model_id: "amazon.nova-lite-v1:0",
        regions: NOVA_REGIONS,
    },
    CrossRegionModel {
        model_id: "amazon.nova-micro-v1:0",
        regions: NOVA_REGIONS,
    },
    CrossRegionModel {
        model_id: "amazon.nova-pro-v1:0",
        regions: NOVA_REGIONS,
    },
    CrossRegionModel {
        model_id: "anthropic.claude-3-5-sonnet-20240620-v1:0",
        regions: &[
            "ap-northeast-1",
            "ap-northeast-2",
            "ap-south-1",
            "ap-southeast-1",
            "ap-southeast-2",
            "eu-central-1",
            "eu-west-1",
            "eu-west-3",
            "us-east-1",
            "us-east-2",
            "us-west-2",
        ],
    },
    CrossRegionModel {
        model_id: "anthropic.claude-3-5-sonnet-20241022-v2:0",
        regions: &[
            "ap-northeast-1",
            "ap-northeast-2",
            "ap-south-1",
            "ap-southeast-1",
            "ap-southeast-2",
            "us-east-1",
            "us-east-2",
            "us-west-2",
        ],
    },
    CrossRegionModel {
        model_id: "anthropic.claude-3-haiku-20240307-v1:0",
        regions: &[
            "ap-northeast-1",
            "ap-northeast-2",
            "ap-south-1",
            "ap-southeast-1",
            "ap-southeast-2",
            "eu-central-1",
            "eu-west-1",
            "eu-west-3",
            "us-east-1",
            "us-west-2",
        ],
    },
    CrossRegionModel {
        model_id: "anthropic.claude-3-sonnet-20240229-v1:0",
        regions: &[
            "ap-northeast-1",
            "ap-southeast-1",
            "ap-southeast-2",
            "eu-central-1",
            "eu-west-1",
            "eu-west-3",
            "us-east-1",
            "us-west-2",
        ],
    },
    CrossRegionModel {
        model_id: "deepseek.r1-v1:0",
        regions: &["us-east-1", "us-east-2", "us-west-2"],
    },
    CrossRegionModel {
        model_id: "meta.llama3-1-70b-instruct-v1:0",
        regions: LLAMA_3_1_REGIONS,
    },
    CrossRegionModel {
        model_id: "meta.llama3-1-8b-instruct-v1:0",
        regions: LLAMA_3_1_REGIONS,
    },
    CrossRegionModel {
        model_id: "meta.llama3-3-70b-instruct-v1:0",
        regions: &["us-east-1", "us-east-2", "us-west-2"],
    },
];

/// The regions the Amazon Nova text models are offered in: every region of every geography
/// this catalog carries.
const NOVA_REGIONS: &[&str] = &[
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-south-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "eu-central-1",
    "eu-north-1",
    "eu-west-1",
    "eu-west-3",
    "us-east-1",
    "us-east-2",
    "us-west-2",
];

/// The regions the Meta Llama 3.1 models are offered in — the two sizes of that line are
/// offered in the same ones, which is why the list is stated once.
const LLAMA_3_1_REGIONS: &[&str] = &[
    "ap-northeast-1",
    "ap-southeast-1",
    "ap-southeast-2",
    "eu-central-1",
    "eu-west-1",
    "eu-west-3",
    "us-east-1",
    "us-east-2",
    "us-west-2",
];

/// The day Amazon Bedrock began publishing cross-region inference profiles: no profile came
/// into being before the capability it belongs to was offered, however long its model had
/// been available by then.
///
/// The lower bound of every published profile's instants
/// ([`cross_region_profile_published_at`]); a profile for a model that launched later came
/// into being with THAT model instead.
pub const CROSS_REGION_PROFILES_PUBLISHED_AT: &str = "2024-08-26T00:00:00Z";

/// How many regions of a geography must hold a model's copy for a cross-region profile to
/// be published there: a profile that could reach one region only would route across none.
pub const MIN_CROSS_REGION_ROUTED_REGIONS: usize = 2;

/// The regions of `geography` a profile for `model` routes invocations to, in the
/// geography's own region order: the regions of the geography that hold a copy of the
/// model.
#[must_use]
pub(super) fn cross_region_routed_regions(
    geography: &InferenceProfileGeography,
    model: &CrossRegionModel,
) -> Vec<&'static str> {
    geography
        .regions
        .iter()
        .copied()
        .filter(|region| model.regions.contains(region))
        .collect()
}

/// The instant a published cross-region profile for `model` came into being: the later of
/// the day cross-region inference profiles began being published
/// ([`CROSS_REGION_PROFILES_PUBLISHED_AT`]) and the day the model itself launched
/// ([`FoundationModel::start_of_life_time`]).
///
/// A profile can be no older than either of the two, and nothing has updated it since, so
/// it is what both `createdAt` and `updatedAt` report. Both are RFC 3339 days written in
/// this file, which order lexicographically as they order in time.
#[must_use]
pub fn cross_region_profile_published_at(model: &FoundationModel) -> &'static str {
    if model.start_of_life_time > CROSS_REGION_PROFILES_PUBLISHED_AT {
        model.start_of_life_time
    } else {
        CROSS_REGION_PROFILES_PUBLISHED_AT
    }
}

/// The BRAND a published cross-region profile names its model by, after the geography's
/// own label.
///
/// A model's brand is what its publisher sells it as: the provider's name followed by the
/// model's name for a model published by a third party, and the model's name alone for one
/// AWS publishes itself ([`AWS_MODEL_PROVIDER`]), because AWS does not brand its own models
/// by its provider name. Both halves are the row's own published members
/// ([`FoundationModel::provider_name`], [`FoundationModel::model_name`]), so a profile
/// names a model exactly as the model catalogue does.
///
/// A brand is reported AS PUBLISHED, version separator included, so a name may carry a `.`
/// that `InferenceProfileName`'s pattern (`^([0-9a-zA-Z][ _-]?)+$`) does not admit. That
/// pattern constrains the name a CALLER may give a profile of its own — the validating walk
/// runs on operation inputs — while this is the name AWS itself publishes for a profile no
/// caller created, and re-spelling a branded name to fit an input constraint would report a
/// name no client could look the model up by.
#[must_use]
pub fn cross_region_model_brand(model: &FoundationModel) -> String {
    if model.provider_name == AWS_MODEL_PROVIDER {
        model.model_name.to_string()
    } else {
        format!("{} {}", model.provider_name, model.model_name)
    }
}

/// The ARN of a system-defined inference profile in `region`:
/// `arn:aws:bedrock:<region>:<account>:inference-profile/<id>`.
///
/// `inference-profile` is the resource type `InferenceProfileArn`'s pattern admits beside
/// `application-inference-profile` (see [`application_inference_profile_arn`]), and it is
/// the one the profiles Amazon Bedrock publishes carry.
#[must_use]
pub fn system_defined_inference_profile_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("inference-profile/{id}"))
}

/// The `inferenceProfileId` of the cross-region profile that routes `model_id` within the
/// geography `region` belongs to (`<prefix>.<modelId>`), or `None` when the catalog
/// publishes no profile in that region.
///
/// Stated once because it is the id BOTH catalogs name a cross-region profile by: a
/// listing builds the published profile under it, and a default router names its target
/// profile by it ([`default_prompt_routers`]).
#[must_use]
pub fn cross_region_profile_id(region: &str, model_id: &str) -> Option<String> {
    inference_profile_geography(region).map(|geography| format!("{}.{model_id}", geography.prefix))
}

/// The geography `region` belongs to, or `None` when this catalog publishes no
/// cross-region profile there.
#[must_use]
pub fn inference_profile_geography(region: &str) -> Option<&'static InferenceProfileGeography> {
    INFERENCE_PROFILE_GEOGRAPHIES
        .iter()
        .find(|geography| geography.regions.contains(&region))
}

/// The `description` a published profile reports: the model it routes to, and the CONCRETE
/// regions it routes that model's requests to, as one sentence.
///
/// `InferenceProfileDescription` is modelled (len 1..200, `@sensitive`) and a published
/// profile carries one, so it is stated for every row by ONE rule rather than written out
/// per profile: what a cross-region profile IS, is the model it fans out and the regions it
/// fans it out to — the same regions its `models` member reports, named rather than
/// summarised, so a reader of the sentence and a reader of the member learn the same set.
///
/// The shape also declares `^([0-9a-zA-Z:.][ _-]?)+$`, which admits no comma, so a sentence
/// naming three or more regions does not satisfy it. That pattern constrains the
/// description a CALLER may give a profile of its own — the validating walk runs on
/// operation inputs — while this is the description Amazon Bedrock itself publishes for a
/// profile no caller created, and it names the regions comma-separated; re-spelling it to
/// fit an input constraint would report a region list no client could read. The same split
/// holds of a published profile's NAME ([`cross_region_model_brand`]).
#[must_use]
pub fn system_defined_inference_profile_description(
    model: &FoundationModel,
    routed_regions: &[&str],
) -> String {
    format!(
        "Routes requests to {} in {}.",
        cross_region_model_brand(model),
        region_list(routed_regions)
    )
}

/// A list of regions as a sentence names them: comma-separated, with the last joined by
/// `and`, which is how a description that names more than one region reads.
#[must_use]
pub(super) fn region_list(regions: &[&str]) -> String {
    match regions {
        [] => String::new(),
        [only] => (*only).to_string(),
        [leading @ .., last] => format!("{} and {last}", leading.join(", ")),
    }
}

/// Every system-defined inference profile Amazon Bedrock publishes in `region`, as the
/// `InferenceProfileSummary` a listing carries, in `inferenceProfileName` order — the
/// order a listing reports them in, because a published profile is created by no call and
/// so has nothing else to be ordered by.
///
/// A profile is published in the regions of its own geography, so a region belonging to no
/// geography this catalog carries has none, and so does a geography holding fewer than two
/// of the regions the profile's own model is offered in ([`cross_region_routed_regions`]):
/// routing across one region is not cross-region routing. Each one reports one model per
/// region it routes to — the base model's copy there, and only where that copy exists —
/// names those same regions in its description, and carries the instant a resource nobody
/// created came into being ([`cross_region_profile_published_at`]) as both `createdAt` and
/// `updatedAt`, because nothing has updated it since.
#[must_use]
pub fn system_defined_inference_profiles(region: &str) -> Vec<InferenceProfileSummary> {
    let Some(geography) = inference_profile_geography(region) else {
        return Vec::new();
    };
    let mut profiles: Vec<InferenceProfileSummary> = CROSS_REGION_MODELS
        .iter()
        .filter_map(|published| {
            let model = catalogued_model(published.model_id)?;
            let routed = cross_region_routed_regions(geography, published);
            if routed.len() < MIN_CROSS_REGION_ROUTED_REGIONS {
                return None;
            }
            let id = format!("{}.{}", geography.prefix, model.model_id);
            let published_at = published_instant(cross_region_profile_published_at(model));
            Some(InferenceProfileSummary {
                inference_profile_name: format!(
                    "{} {}",
                    geography.label,
                    cross_region_model_brand(model)
                ),
                description: Some(system_defined_inference_profile_description(model, &routed)),
                created_at: published_at,
                updated_at: published_at,
                inference_profile_arn: system_defined_inference_profile_arn(region, &id),
                models: routed
                    .iter()
                    .map(|routed| InferenceProfileModel {
                        model_arn: Some(foundation_model_arn(routed, model.model_id)),
                    })
                    .collect(),
                inference_profile_id: id,
                status: INFERENCE_PROFILE_ACTIVE.to_string(),
                r#type: INFERENCE_PROFILE_TYPE_SYSTEM_DEFINED.to_string(),
            })
        })
        .collect();
    profiles.sort_by(|left, right| {
        left.inference_profile_name
            .cmp(&right.inference_profile_name)
    });
    profiles
}
