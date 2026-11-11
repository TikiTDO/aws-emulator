// SPDX-License-Identifier: CC0-1.0

//! `ListInferenceProfiles` end-to-end, over the same `MultiServiceRouter` the emulator
//! boots: the listing is computed out of the profiles `CreateInferenceProfile` persisted
//! and the cross-region profiles Amazon Bedrock publishes, and nothing else.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one recorded
//! request:
//!
//! 1. **Every stored profile of the region is listed**, and each `InferenceProfileSummary`
//!    carries exactly what `GetInferenceProfile` reports for that same profile — over
//!    profiles differing in description, tag count, name form and model source.
//! 2. **The `SYSTEM_DEFINED` profiles Amazon Bedrock publishes in the region are listed
//!    too**: one per base model offered for cross-region inference, reporting the model it
//!    routes to in each region of its geography.
//! 3. **`typeEquals` filters on the profile's own type** and an absent `typeEquals` filters
//!    nothing: `InferenceProfileType` is `[SYSTEM_DEFINED, APPLICATION]`
//!    (`smithy_query(query="shape",shape="InferenceProfileType")`).
//! 4. **The listing is scoped to the region the request addresses**, because an
//!    `InferenceProfileArn` carries the region the profile belongs to.
//! 5. **`maxResults` bounds the page and `nextToken` resumes it**, so the pages partition
//!    the listing with neither a gap nor a repeat — including when the row a token names
//!    has been deleted between two pages — and a `nextToken` this service never issued is
//!    refused rather than read as some page.
//! 6. **A listing reports what the store holds now**: a deleted profile leaves it, and a
//!    region holding none of a type reports the empty list rather than omitting the member.
//! 7. **An identifier naming no profile is refused with the error class's own wording**,
//!    whichever spelling it takes and whichever of the two operations that resolve an
//!    `inferenceProfileIdentifier` is asked.
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation",operation="ListInferenceProfiles")` / `query="shape"`).
//! The requests go through `crate::support` — the ONE harness every suite in this crate
//! drives the emulator with.

use serde_json::{json, Value};

use crate::support::{
    date_time_seconds, emulator, foundation_model_arn, label, Emulator, Reply, ACCOUNT, REGION,
};

// ---------------------------------------------------------------------------------------
// Values the model admits for the members these rows carry.
// ---------------------------------------------------------------------------------------

/// Base model ids this emulator's published catalog carries (the `modelId`s
/// `ListFoundationModels` answers with).
const CATALOG_MODEL: &str = "anthropic.claude-3-haiku-20240307-v1:0";
const OTHER_CATALOG_MODEL: &str = "amazon.nova-pro-v1:0";

/// `MaxResults` is `@range(min: 1, max: 1000)`
/// (`smithy_query(query="shape",shape="MaxResults")`) — the shape `maxResults` targets.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

/// The two values `InferenceProfileType` admits
/// (`smithy_query(query="shape",shape="InferenceProfileType")`).
const TYPE_APPLICATION: &str = "APPLICATION";
const TYPE_SYSTEM_DEFINED: &str = "SYSTEM_DEFINED";

/// The published cross-region catalog this emulator's Amazon Bedrock offers, as
/// `crate::state`'s `INFERENCE_PROFILE_GEOGRAPHIES` and `CROSS_REGION_MODELS` carry it:
/// `(the id prefix, the name label, the regions of the geography)`.
const GEOGRAPHIES: &[(&str, &str, &[&str])] = &[
    (
        "apac",
        "APAC",
        &[
            "ap-northeast-1",
            "ap-northeast-2",
            "ap-south-1",
            "ap-southeast-1",
            "ap-southeast-2",
        ],
    ),
    (
        "eu",
        "EU",
        &["eu-central-1", "eu-north-1", "eu-west-1", "eu-west-3"],
    ),
    ("us", "US", &["us-east-1", "us-east-2", "us-west-2"]),
];

/// The base models that catalog publishes a cross-region profile for, each with the brand
/// its published name carries and the regions the model is offered in — the provider's name
/// and the model's name as `ListFoundationModels` reports them, the model's name alone for
/// a model AWS publishes itself: `(modelId, brand, regions)`, in `modelId` order. The
/// listing reports them in `inferenceProfileName` order, which [`published_in`] sorts them
/// into.
const CROSS_REGION_MODELS: &[(&str, &str, &[&str])] = &[
    ("amazon.nova-lite-v1:0", "Nova Lite", EVERY_REGION),
    ("amazon.nova-micro-v1:0", "Nova Micro", EVERY_REGION),
    ("amazon.nova-pro-v1:0", "Nova Pro", EVERY_REGION),
    (
        "anthropic.claude-3-5-sonnet-20240620-v1:0",
        "Anthropic Claude 3.5 Sonnet",
        &[
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
    ),
    (
        "anthropic.claude-3-5-sonnet-20241022-v2:0",
        "Anthropic Claude 3.5 Sonnet",
        &[
            "ap-northeast-1",
            "ap-northeast-2",
            "ap-south-1",
            "ap-southeast-1",
            "ap-southeast-2",
            "us-east-1",
            "us-east-2",
            "us-west-2",
        ],
    ),
    (
        "anthropic.claude-3-haiku-20240307-v1:0",
        "Anthropic Claude 3 Haiku",
        &[
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
    ),
    (
        "anthropic.claude-3-sonnet-20240229-v1:0",
        "Anthropic Claude 3 Sonnet",
        &[
            "ap-northeast-1",
            "ap-southeast-1",
            "ap-southeast-2",
            "eu-central-1",
            "eu-west-1",
            "eu-west-3",
            "us-east-1",
            "us-west-2",
        ],
    ),
    (
        "deepseek.r1-v1:0",
        "DeepSeek DeepSeek-R1",
        &["us-east-1", "us-east-2", "us-west-2"],
    ),
    (
        "meta.llama3-1-70b-instruct-v1:0",
        "Meta Llama 3.1 70B",
        LLAMA_3_1_REGIONS,
    ),
    (
        "meta.llama3-1-8b-instruct-v1:0",
        "Meta Llama 3.1 8B",
        LLAMA_3_1_REGIONS,
    ),
    (
        "meta.llama3-3-70b-instruct-v1:0",
        "Meta Llama 3.3 70B",
        &["us-east-1", "us-east-2", "us-west-2"],
    ),
];

/// Every region every geography above carries — what a model offered in all of them lists.
const EVERY_REGION: &[&str] = &[
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

/// The regions the two sizes of the Meta Llama 3.1 line are offered in.
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

/// How many regions of a geography must hold a model for a profile to be published there:
/// routing across one region is not cross-region routing.
const MIN_ROUTED_REGIONS: usize = 2;

/// The geography a region belongs to, or `None` when the catalog publishes none there.
fn geography_of(region: &str) -> Option<(&'static str, &'static str, &'static [&'static str])> {
    GEOGRAPHIES
        .iter()
        .find(|(_, _, regions)| regions.contains(&region))
        .copied()
}

/// The regions of the geography a profile for this model routes to: the regions of the
/// geography that hold a copy of the model, in the geography's own order.
fn routed_regions(geography_regions: &[&'static str], model_regions: &[&str]) -> Vec<&'static str> {
    geography_regions
        .iter()
        .copied()
        .filter(|region| model_regions.contains(region))
        .collect()
}

/// The `SYSTEM_DEFINED` summaries the catalog publishes in `region`, in
/// `inferenceProfileName` order — what the listing must report, member for member, beside
/// the `description`, `createdAt` and `updatedAt` a published profile carries, which the
/// row below asserts on their own.
fn published_in(region: &str) -> Vec<Value> {
    let Some((prefix, label, regions)) = geography_of(region) else {
        return Vec::new();
    };
    let mut profiles: Vec<Value> = CROSS_REGION_MODELS
        .iter()
        .filter_map(|(model_id, model_label, model_regions)| {
            let routed = routed_regions(regions, model_regions);
            if routed.len() < MIN_ROUTED_REGIONS {
                return None;
            }
            let id = format!("{prefix}.{model_id}");
            Some(json!({
                "inferenceProfileName": format!("{label} {model_label}"),
                "inferenceProfileArn":
                    format!("arn:aws:bedrock:{region}:{ACCOUNT}:inference-profile/{id}"),
                "inferenceProfileId": id,
                "models": routed
                    .iter()
                    .map(|routed| json!({ "modelArn": foundation_model_arn(routed, model_id) }))
                    .collect::<Vec<Value>>(),
                "status": "ACTIVE",
                "type": TYPE_SYSTEM_DEFINED,
            }))
        })
        .collect();
    profiles.sort_by_key(|profile| {
        profile
            .get("inferenceProfileName")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    profiles
}

/// The `description` a published profile carries, as the catalogue states it: the model it
/// routes to, and the concrete regions it routes that model's requests to.
fn published_description(model_label: &str, routed: &[&str]) -> String {
    let regions = match routed {
        [] => String::new(),
        [only] => (*only).to_string(),
        [leading @ .., last] => format!("{} and {last}", leading.join(", ")),
    };
    format!("Routes requests to {model_label} in {regions}.")
}

/// One listed summary without the three members a published profile computes rather than
/// carries in the table above, so the comparison is against what the table does state.
fn without_published_metadata(summary: &Value) -> Value {
    let mut trimmed = summary.clone();
    if let Some(object) = trimmed.as_object_mut() {
        for member in ["description", "createdAt", "updatedAt"] {
            object.remove(member);
        }
    }
    trimmed
}

/// A `foundation-model` source ARN, as `InferenceProfileModelSource`'s pattern spells one.
fn foundation_model_source(region: &str, model_id: &str) -> String {
    foundation_model_arn(region, model_id)
}

/// A system-defined `inference-profile` source ARN — the other resource type that same
/// pattern admits — whose id is a base model id behind a geography prefix.
fn system_profile_source(region: &str, profile_id: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:inference-profile/{profile_id}")
}

/// The ARN prefix every profile minted for `region` carries (`InferenceProfileArn`'s
/// pattern: `…:bedrock:<region>:<account>:application-inference-profile/…`).
fn arn_prefix(region: &str) -> String {
    format!("arn:aws:bedrock:{region}:{ACCOUNT}:application-inference-profile/")
}

fn create_body(name: &str, source: &str) -> Value {
    json!({
        "inferenceProfileName": name,
        "modelSource": { "copyFrom": source },
    })
}

/// `GET /inference-profiles`, as the model binds the listing, with `query` as its query
/// string.
async fn list_in_region(emu: &Emulator, region: &str, query: &str) -> Reply {
    let path = if query.is_empty() {
        "/inference-profiles".to_string()
    } else {
        format!("/inference-profiles?{query}")
    };
    emu.call_in_region(region, "GET", &path, None).await
}

async fn list(emu: &Emulator, query: &str) -> Reply {
    list_in_region(emu, REGION, query).await
}

/// The query that asks for the profiles a caller created, which is what every row about
/// the account's own profiles filters on.
fn application_only() -> String {
    format!("type={TYPE_APPLICATION}")
}

async fn create_in_region(emu: &Emulator, region: &str, body: Value) -> Reply {
    emu.call_in_region(region, "POST", "/inference-profiles", Some(body))
        .await
}

/// Create a profile in `region` with only the required members and hand back its ARN.
async fn given_profile_in(emu: &Emulator, region: &str, name: &str, source: &str) -> String {
    let reply = create_in_region(emu, region, create_body(name, source)).await;
    assert_eq!(
        reply.status, 201,
        "case {name:?}: CreateInferenceProfile must answer 201, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("inferenceProfileArn")
}

async fn given_profile(emu: &Emulator, name: &str, source: &str) -> String {
    given_profile_in(emu, REGION, name, source).await
}

/// `GET /inference-profiles/{identifier}`, the read every stored summary is compared
/// against.
async fn get(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/inference-profiles/{}", label(identifier)),
        None,
    )
    .await
}

async fn delete(emu: &Emulator, identifier: &str) -> Reply {
    emu.call(
        "DELETE",
        &format!("/inference-profiles/{}", label(identifier)),
        None,
    )
    .await
}

/// The summaries a listing reported, panicking when the member is absent — it is modelled
/// as a list and a listing always carries it.
fn summaries(case: &str, reply: &Reply) -> Vec<Value> {
    reply
        .get("inferenceProfileSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: inferenceProfileSummaries must be a present array, got {:?}",
                reply.body
            )
        })
        .clone()
}

/// The `inferenceProfileArn`s a listing reported, in the order it reported them.
fn listed_arns(case: &str, reply: &Reply) -> Vec<String> {
    summaries(case, reply)
        .iter()
        .map(|row| {
            row.get("inferenceProfileArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// 1. Every stored profile of the region is listed, as the read reports it.
// ---------------------------------------------------------------------------------------

/// One row: the case, the profile name, its optional description, its tags and the
/// `modelSource.copyFrom` the create carries.
type ProfileRow = (
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static [(&'static str, &'static str)],
    String,
);

#[tokio::test]
async fn the_listing_reports_every_stored_profile_as_the_read_reports_it() {
    // The rows differ along what the rule generalises over: the optional description, the
    // tag count (none, one, several), the name forms `InferenceProfileName`'s pattern
    // (`^([0-9a-zA-Z][ _-]?)+$`) admits, and both resource types `copyFrom` admits.
    let rows: Vec<ProfileRow> = vec![
        (
            "only the required members",
            "listed-one",
            None,
            &[],
            foundation_model_source(REGION, CATALOG_MODEL),
        ),
        (
            "a description and one tag",
            "listed two",
            Some("tracks the haiku model"),
            &[("Environment", "production")],
            foundation_model_source(REGION, OTHER_CATALOG_MODEL),
        ),
        (
            "several tags and a system-defined source",
            "Listed_Three",
            Some("costs by team"),
            &[("Team", "search"), ("CostCenter", "4711")],
            system_profile_source(REGION, &format!("us.{CATALOG_MODEL}")),
        ),
        (
            "a source in another region",
            "listed four",
            None,
            &[],
            foundation_model_source("us-east-1", OTHER_CATALOG_MODEL),
        ),
    ];

    let emu = emulator();
    let mut minted: Vec<(&str, String)> = Vec::new();
    for (case, name, description, tags, source) in &rows {
        let mut body = create_body(name, source);
        if let Some(description) = description {
            body["description"] = json!(description);
        }
        if !tags.is_empty() {
            body["tags"] = Value::Array(
                tags.iter()
                    .map(|(key, value)| json!({ "key": key, "value": value }))
                    .collect(),
            );
        }
        let created = create_in_region(&emu, REGION, body).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: CreateInferenceProfile must answer 201, got {} {:?}",
            created.status, created.body
        );
        minted.push((case, created.text("inferenceProfileArn")));
    }

    let listing = list(&emu, &application_only()).await;
    assert_eq!(
        listing.status, 200,
        "ListInferenceProfiles must answer 200, got {} {:?}",
        listing.status, listing.body
    );
    assert_eq!(
        listing.get("nextToken"),
        Value::Null,
        "a listing that fits one page carries no nextToken, got {:?}",
        listing.get("nextToken")
    );

    // Every profile the store holds for this region, in the order it was created: the
    // listing is ordered by creation instant, so the minting order IS the listed order.
    let expected: Vec<String> = minted.iter().map(|(_, arn)| arn.clone()).collect();
    assert_eq!(
        listed_arns("the whole listing", &listing),
        expected,
        "case \"the whole listing\": every stored profile is listed once, in creation order, \
         got {:?}",
        listing.body
    );

    // Each summary is what the READ reports for that same profile: the summary and the
    // read carry the same members, so the one is the other.
    let reported = summaries("the whole listing", &listing);
    for ((case, arn), summary) in minted.iter().zip(reported.iter()) {
        let read = get(&emu, arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetInferenceProfile must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            summary, &read.body,
            "case {case:?}: the summary must report what the read reports, got {summary:?}"
        );
        // The two instants a summary carries are modelled `@timestampFormat("date-time")`,
        // so each is the RFC 3339 STRING form and not the protocol's epoch-number default.
        for member in ["createdAt", "updatedAt"] {
            let value = summary.get(member).cloned().unwrap_or(Value::Null);
            let text = value.as_str().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {member} is modelled @timestampFormat(\"date-time\"), so it \
                     must be an RFC 3339 string, got {value}"
                )
            });
            let _ = date_time_seconds(case, text);
        }
        assert!(
            arn.starts_with(&arn_prefix(REGION)),
            "case {case:?}: the listed ARN must be an application-inference-profile ARN of \
             {REGION}, got {arn:?}"
        );
    }
}

/// The two `Timestamp` members of an `InferenceProfileSummary` target a shape the model
/// marks `@timestampFormat("date-time")`
/// (`smithy_query(query="shape",shape="InferenceProfileSummary")`), so each member schema
/// must report that format — it is what makes the codec write the RFC 3339 string the
/// listing above reads rather than the protocol's default epoch number.
#[test]
fn the_date_time_members_of_a_listed_profile_carry_the_format() {
    use ::aws_smithy_schema::traits::TimestampFormat;
    use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::ops::types::InferenceProfileSummary;

    for (case, schema) in [
        (
            "InferenceProfileSummary$createdAt",
            &InferenceProfileSummary::M_CREATED_AT,
        ),
        (
            "InferenceProfileSummary$updatedAt",
            &InferenceProfileSummary::M_UPDATED_AT,
        ),
    ] {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        assert!(
            matches!(format, Some(TimestampFormat::DateTime)),
            "case {case}: the member schema must carry @timestampFormat(date-time), got \
             {format:?}"
        );
    }

    // The control row: a member the model gives no `@timestampFormat` reports none, so the
    // assertion above is about the trait and not about every member schema.
    let status = InferenceProfileSummary::M_STATUS
        .timestamp_format()
        .map(|trait_| trait_.format());
    assert!(
        status.is_none(),
        "case InferenceProfileSummary$status: the model gives this member no \
         @timestampFormat, got {status:?}"
    );
}

// ---------------------------------------------------------------------------------------
// 2. The published cross-region profiles of the region are listed too.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_published_cross_region_profiles_of_the_region_are_listed() {
    let emu = emulator();

    // The rows span every geography the catalog publishes — which differ in how many
    // regions each routes to — two regions of ONE geography, so the published set is the
    // same set addressed from either, and a region outside every geography.
    let rows: Vec<(&str, &str)> = vec![
        ("a region of the us geography", "us-west-2"),
        ("another region of the us geography", "us-east-2"),
        ("a region of the eu geography", "eu-west-1"),
        ("a region of the apac geography", "ap-northeast-1"),
        ("a region of no geography", "sa-east-1"),
    ];

    for (case, region) in rows {
        let listing = list_in_region(&emu, region, &format!("type={TYPE_SYSTEM_DEFINED}")).await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: the listing must answer 200, got {} {:?}",
            listing.status, listing.body
        );
        let expected = published_in(region);
        let listed: Vec<Value> = summaries(case, &listing)
            .iter()
            .map(without_published_metadata)
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: the published profiles of {region} must be reported member for \
             member, in inferenceProfileName order, got {:?}",
            listing.body
        );
        // A published profile carries the three members the table above leaves to this row:
        // the description stating which model it routes and to which regions, and the
        // instant it came into being — the same for `createdAt` and `updatedAt`, because no
        // call has updated a resource no call created.
        let geography = geography_of(region);
        for summary in summaries(case, &listing) {
            let id = summary
                .get("inferenceProfileId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let (_, _, geography_regions) = geography
                .unwrap_or_else(|| panic!("case {case:?}: a listed profile has a geography"));
            let (model_label, routed) = CROSS_REGION_MODELS
                .iter()
                .find(|(model_id, _, _)| id.ends_with(&format!(".{model_id}")))
                .map(|(_, model_label, model_regions)| {
                    (*model_label, routed_regions(geography_regions, model_regions))
                })
                .unwrap_or_else(|| panic!("case {case:?}: {id:?} must be a catalogued model"));
            assert_eq!(
                summary.get("description").and_then(Value::as_str),
                Some(published_description(model_label, &routed).as_str()),
                "case {case:?}: the published description must name the model and the \
                 regions it routes to, got {summary:?}"
            );
            let created = summary
                .get("createdAt")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a published profile carries createdAt, got {summary:?}"))
                .to_string();
            let updated = summary
                .get("updatedAt")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a published profile carries updatedAt, got {summary:?}"))
                .to_string();
            assert_eq!(
                created, updated,
                "case {case:?}: nothing has updated a published profile, so the two instants \
                 agree, got {summary:?}"
            );
            // And it is an instant on the wire, not an epoch number: `@timestampFormat`.
            date_time_seconds(case, &created);
        }
    }

    // A published profile is an `inference-profile` ARN of the region it was listed in, so
    // the same profile of two regions of one geography differs only in that field.
    let here = published_in("us-west-2");
    let there = published_in("us-east-2");
    assert_eq!(
        here.len(),
        CROSS_REGION_MODELS
            .iter()
            .filter(|(_, _, model_regions)| {
                routed_regions(&["us-east-1", "us-east-2", "us-west-2"], model_regions).len()
                    >= MIN_ROUTED_REGIONS
            })
            .count(),
        "case \"the us geography\": one published profile per cross-region model the \
         geography holds at least two regions of, got {here:?}"
    );
    for (left, right) in here.iter().zip(there.iter()) {
        assert_eq!(
            left.get("inferenceProfileId"),
            right.get("inferenceProfileId"),
            "case \"two regions of one geography\": the profile ids must agree, got {left:?} \
             and {right:?}"
        );
        assert_ne!(
            left.get("inferenceProfileArn"),
            right.get("inferenceProfileArn"),
            "case \"two regions of one geography\": the ARN carries the region it was listed \
             in, got {left:?}"
        );
    }
}

/// A published profile is a `modelSource.copyFrom` a create accepts, and the application
/// profile it mints tracks the base model behind that profile's geography prefix.
#[tokio::test]
async fn a_published_profile_is_a_model_source_a_create_accepts() {
    let emu = emulator();
    let published = published_in(REGION);
    assert!(
        !published.is_empty(),
        "case {REGION:?}: the catalog must publish profiles in this region"
    );

    for (index, summary) in published.iter().enumerate() {
        let case = format!("published profile {index}");
        let source = summary
            .get("inferenceProfileArn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let arn = given_profile_in(&emu, REGION, &format!("from-published-{index}"), &source).await;
        let read = get(&emu, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the created profile must read back, got {} {:?}",
            read.status, read.body
        );
        // The model behind the geography prefix, as the published profile's own id carries
        // it, tracked as a `FoundationModelArn` of this region.
        let profile_id = summary
            .get("inferenceProfileId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let model_id = profile_id
            .split_once('.')
            .map(|(_, model_id)| model_id)
            .unwrap_or_default();
        assert_eq!(
            read.get("models"),
            json!([{ "modelArn": foundation_model_arn(REGION, model_id) }]),
            "case {case:?}: the profile must track {model_id}, got {:?}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. `typeEquals` filters on the profile's own type.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_type_filter_reports_only_the_profiles_of_that_type() {
    let emu = emulator();
    let source = foundation_model_source(REGION, CATALOG_MODEL);
    let mut created: Vec<String> = Vec::new();
    for index in 0..3 {
        created.push(given_profile(&emu, &format!("typed-{index}"), &source).await);
    }
    let published: Vec<String> = published_in(REGION)
        .iter()
        .map(|summary| {
            summary
                .get("inferenceProfileArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();

    // (case, the query, the ARNs the listing must report). An absent `typeEquals` filters
    // nothing, so it reports both sets; each value the filter admits reports one of them.
    // A published profile was created by no call, so it is ordered ahead of the created
    // ones.
    let both: Vec<String> = published.iter().chain(created.iter()).cloned().collect();
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        ("no filter at all", String::new(), both),
        (
            "the type a caller's profile carries",
            format!("type={TYPE_APPLICATION}"),
            created.clone(),
        ),
        (
            "the type Amazon Bedrock defines",
            format!("type={TYPE_SYSTEM_DEFINED}"),
            published.clone(),
        ),
    ];

    for (case, query, expected) in rows {
        let listing = list(&emu, &query).await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: the listing must answer 200, got {} {:?}",
            listing.status, listing.body
        );
        assert_eq!(
            listed_arns(case, &listing),
            expected,
            "case {case:?}: the listing must report exactly the profiles of that type, got {:?}",
            listing.body
        );
        // Every row's own `type` agrees with the filter that selected it.
        for summary in summaries(case, &listing) {
            let arn = summary
                .get("inferenceProfileArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let reported = summary.get("type").and_then(Value::as_str).unwrap_or_default();
            let owned = if created.contains(&arn) {
                TYPE_APPLICATION
            } else {
                TYPE_SYSTEM_DEFINED
            };
            assert_eq!(
                reported, owned,
                "case {case:?}: {arn} must report {owned}, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// 4. The listing is scoped to the region the request addresses.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_listing_reports_only_the_profiles_of_the_region_it_addresses() {
    let emu = emulator();
    let source = foundation_model_source(REGION, CATALOG_MODEL);

    // The rows differ in how many profiles a region holds: several, one, and none at all.
    let here = vec![
        given_profile_in(&emu, REGION, "here one", &source).await,
        given_profile_in(&emu, REGION, "here two", &source).await,
    ];
    let elsewhere = vec![given_profile_in(&emu, "eu-central-1", "far away", &source).await];

    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        ("the region two profiles were created in", REGION, here),
        (
            "a region one profile was created in",
            "eu-central-1",
            elsewhere,
        ),
        ("a region no profile was created in", "ap-south-1", Vec::new()),
    ];

    for (case, region, expected) in rows {
        let listing = list_in_region(&emu, region, &application_only()).await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: the listing must answer 200, got {} {:?}",
            listing.status, listing.body
        );
        assert_eq!(
            listed_arns(case, &listing),
            expected,
            "case {case:?}: a profile of another region is another account-region's \
             resource, got {:?}",
            listing.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. `maxResults` bounds the page and `nextToken` resumes it.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn the_pages_partition_the_listing() {
    let emu = emulator();
    let source = foundation_model_source(REGION, CATALOG_MODEL);
    for index in 0..7 {
        given_profile(&emu, &format!("paged-{index}"), &source).await;
    }
    let whole = list(&emu, "").await;
    let all = listed_arns("the whole listing", &whole);
    assert_eq!(
        all.len(),
        7 + published_in(REGION).len(),
        "the whole listing must report every profile of this region, got {:?}",
        whole.body
    );

    // The page sizes span the published range: the minimum the model admits, sizes that do
    // and do not divide the listing, the exact count, a size past the count, and the
    // maximum the model admits.
    let count = i64::try_from(all.len()).unwrap_or(i64::MAX);
    for page_size in [MAX_RESULTS_MIN, 2, 5, count, count + 2, MAX_RESULTS_MAX] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=all.len() {
            let query = match token.as_deref() {
                Some(token) => format!("maxResults={page_size}&nextToken={}", label(token)),
                None => format!("maxResults={page_size}"),
            };
            let page = list(&emu, &query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: every page must answer 200, got {} {:?}",
                page.status, page.body
            );
            let rows = listed_arns(&case, &page);
            assert!(
                i64::try_from(rows.len()).unwrap_or(i64::MAX) <= page_size,
                "case {case:?}: a page must hold at most maxResults rows, got {}",
                rows.len()
            );
            walked.extend(rows);
            token = page.get("nextToken").as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "case {case:?}: the last page carries no nextToken, got {token:?}"
        );
        assert_eq!(
            walked, all,
            "case {case:?}: the pages must partition the listing with neither a gap nor a \
             repeat, got {walked:?}"
        );
    }

    // A page resumes at "the first row at or after this cursor", so deleting the row a
    // token names loses no other row. Driven over the created profiles, the ones a delete
    // can take away.
    let created = listed_arns("the created profiles", &list(&emu, &application_only()).await);
    let first = list(&emu, &format!("{}&maxResults=3", application_only())).await;
    let token = first.text("nextToken");
    let dropped = created[3].clone();
    let removed = delete(&emu, &dropped).await;
    assert_eq!(
        removed.status, 200,
        "case \"the row a token names\": the delete must be accepted, got {} {:?}",
        removed.status, removed.body
    );
    let resumed = list(
        &emu,
        &format!("{}&nextToken={}", application_only(), label(&token)),
    )
    .await;
    let expected: Vec<String> = created[4..].to_vec();
    assert_eq!(
        listed_arns("a token whose row is gone", &resumed),
        expected,
        "case \"a token whose row is gone\": the page resumes at the next row, got {:?}",
        resumed.body
    );
}

#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let emu = emulator();
    let source = foundation_model_source(REGION, CATALOG_MODEL);
    given_profile(&emu, "token refusal", &source).await;

    // The rows differ in HOW the token fails the cursor's form: not a cursor at all, an
    // instant half of the wrong width, a cursor naming no resource, and a cursor whose
    // instant half is not a number.
    for case in [
        "nope",
        "0000000000000000000:arn",
        "00000000000000000000:",
        "notacursor:atall",
    ] {
        let refused = list(&emu, &format!("nextToken={}", label(case))).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a foreign nextToken must be refused with 400 ValidationException, \
             got {} {:?}",
            refused.status,
            refused.body
        );
    }

    // And one past each published bound of `maxResults` is refused for its range, which is
    // what makes the accepted sizes above a statement about the range rather than about
    // every number.
    for bound in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let case = format!("maxResults={bound}");
        let refused = list(&emu, &case).await;
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a maxResults outside its range must be refused, got {} {:?}",
            refused.status,
            refused.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 6. A listing reports what the store holds now.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_profile_leaves_the_listing() {
    let emu = emulator();
    let source = foundation_model_source(REGION, CATALOG_MODEL);
    let mut live: Vec<String> = Vec::new();
    for index in 0..3 {
        live.push(given_profile(&emu, &format!("transient-{index}"), &source).await);
    }

    // Deleting them one at a time: after each delete the listing is exactly what is left,
    // so the rows span every count from the full store down to the empty one.
    while !live.is_empty() {
        let dropped = live.remove(0);
        let removed = delete(&emu, &dropped).await;
        assert_eq!(
            removed.status, 200,
            "case {dropped:?}: the delete must be accepted, got {} {:?}",
            removed.status, removed.body
        );
        let case = format!("{} profiles left", live.len());
        let listing = list(&emu, &application_only()).await;
        assert_eq!(
            listed_arns(&case, &listing),
            live,
            "case {case:?}: the listing must report exactly what the store holds, got {:?}",
            listing.body
        );
        assert_eq!(
            listing.get("nextToken"),
            Value::Null,
            "case {case:?}: a listing that fits one page carries no nextToken, got {:?}",
            listing.get("nextToken")
        );
    }

    // A region holding none of a type reports the EMPTY list rather than omitting the
    // member — here the account's own profiles, every one of which has been deleted.
    let empty = list(&emu, &application_only()).await;
    assert_eq!(
        empty.get("inferenceProfileSummaries"),
        json!([]),
        "case \"an empty store\": the listing must carry the empty list, got {:?}",
        empty.body
    );
}

// ---------------------------------------------------------------------------------------
// 7. The wording of the refusal for an identifier naming no profile.
// ---------------------------------------------------------------------------------------

/// What Amazon Bedrock states when an `inferenceProfileIdentifier` names no inference
/// profile. The wording is the error class's: it names no identifier, no account and no
/// region, so it is the same text whichever spelling was asked for.
const NOT_FOUND_MESSAGE: &str = "Inference profile not found.";

#[tokio::test]
async fn an_identifier_naming_no_profile_is_refused_with_the_error_class_wording() {
    let emu = emulator();
    let source = foundation_model_source(REGION, CATALOG_MODEL);
    // One live profile, so the refusals below are about the identifier and not about an
    // empty store, and one in another region, so a row can name a live profile of one.
    let live = given_profile(&emu, "the live one", &source).await;
    let elsewhere = given_profile_in(&emu, "eu-central-1", "far away", &source).await;

    // The rows differ along what the rule generalises over: the spelling of the identifier
    // (a bare id, an ARN of this region, an ARN of another region) and the length of the
    // id it carries.
    let rows: Vec<(&str, String)> = vec![
        ("a bare id nothing minted", "zzzzzzzzzzzz".to_string()),
        ("a one-character bare id", "z".to_string()),
        (
            "an application profile ARN nothing minted",
            format!("{}zzzzzzzzzzzz", arn_prefix(REGION)),
        ),
        ("a live profile of another region", elsewhere.clone()),
    ];

    for (case, identifier) in rows {
        for (operation, reply) in [
            ("GetInferenceProfile", get(&emu, &identifier).await),
            ("DeleteInferenceProfile", delete(&emu, &identifier).await),
        ] {
            assert_eq!(
                (reply.status, reply.error.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} ({operation}): expected 404 ResourceNotFoundException, got {} {:?}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.get("message"),
                json!(NOT_FOUND_MESSAGE),
                "case {case:?} ({operation}): expected {NOT_FOUND_MESSAGE:?}, got {:?}",
                reply.get("message")
            );
        }
    }

    // The live profile is untouched by any of those refusals, so each one refused the
    // identifier rather than the resource.
    let read = get(&emu, &live).await;
    assert_eq!(
        read.status, 200,
        "case \"the live profile\": a refused call must remove nothing, got {} {:?}",
        read.status, read.body
    );
}
