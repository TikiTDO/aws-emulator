// SPDX-License-Identifier: CC0-1.0

//! The route calculator control plane, end to end over the wire.
//!
//! Every call goes through the shared harness (`crate::harness`), so each one is a
//! separate HTTP request against one shared engine: a value that survives from the
//! create call to the describe call survived because it was PERSISTED, not because a
//! handler echoed its own input back.
//!
//! The tables span the classes the rules generalise over — the whole documented set
//! of data providers, the optional members of the create request, the shapes a
//! rejected `DataSource` can take (a case variant of a valid provider, padding, an
//! unknown provider, an empty value), the retired `PricingPlan` values, and the
//! request region the ARN is built from. The request in the issue report is one row
//! of the refusal table.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_location_locationservice::state;

// The one HTTP harness this crate's suites share, declared from `tests/all.rs`.
use crate::harness::{call, call_in_region, emulator, millis_of, Answer, ACCOUNT, REGION};

/// The data providers a route calculator may be created with, written out here
/// rather than read from the crate: the source is the
/// `CreateRouteCalculatorRequest$DataSource` member doc ("Valid values include:
/// Esri … Grab … Here"), so a catalog that silently loses or renames a provider
/// fails these tests instead of shrinking with them.
const DOCUMENTED_DATA_SOURCES: [&str; 3] = ["Esri", "Grab", "Here"];

/// The sentence a `DataSource` outside that set is refused with, written out as a
/// fixed string for the same reason: the member name in single quotes, "must be one
/// of", the documented values separated by ", ", then the period that ends it.
const DATA_SOURCE_REFUSAL: &str = "'DataSource' must be one of Esri, Grab, Here.";

/// The sentence a retired `PricingPlan` is refused with.
const PRICING_PLAN_REFUSAL: &str = "'PricingPlan' must be RequestBasedUsage";

async fn create_calculator(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/routes/v0/calculators", Some(body)).await
}

async fn describe_calculator(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/routes/v0/calculators/{name}"),
        None,
    )
    .await
}

/// Assert one refused create: the status and code the model declares for a bad
/// member, the sentence, and the two members the model marks required beside
/// `message`.
fn assert_field_refusal(case: &str, refused: &Answer, member: &str, sentence: &str) {
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected a 400 refusal, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case {case:?}: expected error code ValidationException, got {:?} ({:?})",
        refused.error_code(),
        refused.body
    );
    assert_eq!(
        refused.message(),
        sentence,
        "case {case:?}: expected message {sentence:?}, got {:?}",
        refused.message()
    );
    let body = refused.json();
    assert_eq!(
        body.get("reason").and_then(Value::as_str),
        Some("FieldValidationFailed"),
        "case {case:?}: expected reason FieldValidationFailed, got {body}"
    );
    assert_eq!(
        body.get("fieldList")
            .and_then(Value::as_array)
            .map(|entries| entries
                .iter()
                .map(|entry| (
                    entry.get("name").and_then(Value::as_str).unwrap_or_default(),
                    entry
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                ))
                .collect::<Vec<_>>()),
        Some(vec![(member, sentence)]),
        "case {case:?}: fieldList must name the offending member and repeat the sentence, got {body}"
    );
}

// ---------------------------------------------------------------------------
// CreateRouteCalculator / DescribeRouteCalculator
// ---------------------------------------------------------------------------

/// One row of the create-then-describe table.
struct CalculatorCase {
    name: &'static str,
    request: fn() -> Value,
    data_source: &'static str,
    /// What DescribeRouteCalculator must report: an absent `Description` reads back
    /// as the empty string, because the read models it as required.
    description: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The create-then-describe table.
///
/// The `DataSource` column covers the whole documented provider set, so the rows
/// include providers the issue report did not name; the other columns differ along
/// the optional members of the request and the punctuation a `ResourceName` admits
/// (`^[-._\w]+$`).
fn calculator_cases() -> Vec<CalculatorCase> {
    vec![
        CalculatorCase {
            // Every optional member absent: what the read reports then comes from
            // the service, not from the request.
            name: "minimal",
            request: || json!({ "CalculatorName": "minimal", "DataSource": "Esri" }),
            data_source: "Esri",
            description: "",
            tags: &[],
        },
        CalculatorCase {
            name: "grab.calculator_1",
            request: || {
                json!({
                    "CalculatorName": "grab.calculator_1",
                    "DataSource": "Grab",
                    "Description": "southeast asia routing",
                    "Tags": { "team": "logistics", "env": "staging", "cost-center": "42" },
                })
            },
            data_source: "Grab",
            description: "southeast asia routing",
            tags: &[
                ("cost-center", "42"),
                ("env", "staging"),
                ("team", "logistics"),
            ],
        },
        CalculatorCase {
            name: "Here-Calculator",
            request: || {
                json!({
                    "CalculatorName": "Here-Calculator",
                    "DataSource": "Here",
                    "Description": "",
                    // Deprecated, and the only value it still admits.
                    "PricingPlan": "RequestBasedUsage",
                    "Tags": { "owner": "routing" },
                })
            },
            data_source: "Here",
            description: "",
            tags: &[("owner", "routing")],
        },
    ]
}

#[tokio::test]
async fn create_then_describe_round_trips_every_documented_data_source() {
    let router = emulator();

    // The table is the rule's whole domain, not a sample of it.
    let mut covered: Vec<&str> = calculator_cases()
        .iter()
        .map(|case| case.data_source)
        .collect();
    covered.sort_unstable();
    assert_eq!(
        covered,
        DOCUMENTED_DATA_SOURCES.to_vec(),
        "the table must cover every documented data provider, got {covered:?}"
    );

    for case in calculator_cases() {
        let created = create_calculator(&router, (case.request)()).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateRouteCalculator must succeed, got {} {:?}",
            case.name, created.status, created.body
        );
        let created = created.json();
        let expected_arn =
            format!("arn:aws:geo:{REGION}:{ACCOUNT}:route-calculator/{}", case.name);
        assert_eq!(
            created.get("CalculatorName").and_then(Value::as_str),
            Some(case.name),
            "case {:?}: CreateRouteCalculator must answer the created name, got {created}",
            case.name
        );
        assert_eq!(
            created.get("CalculatorArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {:?}: expected CalculatorArn {expected_arn}, got {created}",
            case.name
        );
        let create_time = millis_of(&created, "CreateTime");

        let described = describe_calculator(&router, case.name).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: DescribeRouteCalculator must succeed, got {} {:?}",
            case.name, described.status, described.body
        );
        let described = described.json();

        assert_eq!(
            described.get("CalculatorName").and_then(Value::as_str),
            Some(case.name),
            "case {:?}: DescribeRouteCalculator must report the stored name, got {described}",
            case.name
        );
        assert_eq!(
            described.get("CalculatorArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {:?}: DescribeRouteCalculator must report the stored ARN, got {described}",
            case.name
        );
        assert_eq!(
            described.get("DataSource").and_then(Value::as_str),
            Some(case.data_source),
            "case {:?}: expected DataSource {:?}, got {described}",
            case.name,
            case.data_source
        );
        assert_eq!(
            described.get("Description").and_then(Value::as_str),
            Some(case.description),
            "case {:?}: expected Description {:?}, got {described}",
            case.name,
            case.description
        );
        // `DescribeRouteCalculatorResponse$PricingPlan` doc: "Always returns
        // RequestBasedUsage".
        assert_eq!(
            described.get("PricingPlan").and_then(Value::as_str),
            Some("RequestBasedUsage"),
            "case {:?}: PricingPlan must always be RequestBasedUsage, got {described}",
            case.name
        );

        let mut reported_tags: Vec<(String, String)> = described
            .get("Tags")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        reported_tags.sort();
        let expected_tags: Vec<(String, String)> = case
            .tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            reported_tags, expected_tags,
            "case {:?}: expected Tags {expected_tags:?}, got {reported_tags:?}",
            case.name
        );

        assert_eq!(
            millis_of(&described, "CreateTime"),
            create_time,
            "case {:?}: DescribeRouteCalculator must report the CreateTime the create stored, got {described}",
            case.name
        );
        assert_eq!(
            millis_of(&described, "UpdateTime"),
            create_time,
            "case {:?}: a never-updated calculator's UpdateTime must equal its CreateTime, got {described}",
            case.name
        );
    }
}

/// Every `DataSource` the documented set does not hold, exactly as a request could
/// spell it.
///
/// The rows differ along the shapes a rejected value takes: a case variant of each
/// documented provider (the member doc states the field is case-sensitive and that
/// `HERE` returns an error), a padded value, an unknown provider, two providers in
/// one value, and an empty one (the member carries no `@length`, so the request
/// boundary accepts it and the check is the service's).
const REJECTED_DATA_SOURCES: &[&str] = &[
    "HERE", "here", "ESRI", "esri", "GRAB", "grab", "Bing", "TomTom", " Esri", "Esri ",
    "Esri,Here", "",
];

#[tokio::test]
async fn a_data_source_outside_the_documented_set_is_refused_and_persists_nothing() {
    for (index, data_source) in REJECTED_DATA_SOURCES.iter().enumerate() {
        let router = emulator();
        let name = format!("refused-{index}");
        let refused = create_calculator(
            &router,
            json!({ "CalculatorName": name, "DataSource": data_source }),
        )
        .await;

        assert_field_refusal(data_source, &refused, "DataSource", DATA_SOURCE_REFUSAL);

        // A refused create stores nothing: the read answers 404, not a resource.
        let described = describe_calculator(&router, &name).await;
        assert_eq!(
            described.status, 404,
            "case {data_source:?}: a refused create must persist nothing, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.error_code(),
            "ResourceNotFoundException",
            "case {data_source:?}: expected ResourceNotFoundException, got {:?} ({:?})",
            described.error_code(),
            described.body
        );
    }
}

#[tokio::test]
async fn every_documented_data_source_is_accepted_as_spelled() {
    // The crate's catalog must be exactly the documented set: a provider dropped
    // from it, or one spelled differently, shows up here rather than in a client's
    // failed create.
    assert_eq!(
        state::ROUTE_CALCULATOR_DATA_SOURCES.to_vec(),
        DOCUMENTED_DATA_SOURCES.to_vec(),
        "the service's provider catalog must be the documented set, got {:?}",
        state::ROUTE_CALCULATOR_DATA_SOURCES
    );

    for data_source in DOCUMENTED_DATA_SOURCES {
        let router = emulator();
        let created = create_calculator(
            &router,
            json!({ "CalculatorName": "accepted", "DataSource": data_source }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {data_source:?}: a documented data provider must be accepted, got {} {:?}",
            created.status, created.body
        );
        let described = describe_calculator(&router, "accepted").await;
        assert_eq!(
            described.json().get("DataSource").and_then(Value::as_str),
            Some(data_source),
            "case {data_source:?}: the stored DataSource must read back as sent, got {:?}",
            described.body
        );
    }
}

#[tokio::test]
async fn a_retired_pricing_plan_is_refused_and_the_one_it_still_admits_is_not() {
    // `PricingPlan` doc: "No longer used. If included, the only allowed value is
    // RequestBasedUsage." The two rows are the enum's other members
    // (`smithy_query shape=PricingPlan`).
    for plan in ["MobileAssetManagement", "MobileAssetTracking"] {
        let router = emulator();
        let refused = create_calculator(
            &router,
            json!({ "CalculatorName": "retired", "DataSource": "Esri", "PricingPlan": plan }),
        )
        .await;
        assert_field_refusal(plan, &refused, "PricingPlan", PRICING_PLAN_REFUSAL);
        assert_eq!(
            describe_calculator(&router, "retired").await.status,
            404,
            "case {plan:?}: a refused create must persist nothing"
        );
    }

    // Present and allowed, and absent, are both accepted.
    for plan in [Some("RequestBasedUsage"), None] {
        let router = emulator();
        let mut body = json!({ "CalculatorName": "allowed", "DataSource": "Esri" });
        if let Some(plan) = plan {
            body["PricingPlan"] = json!(plan);
        }
        let created = create_calculator(&router, body).await;
        assert_eq!(
            created.status, 200,
            "case {plan:?}: the allowed PricingPlan must be accepted, got {} {:?}",
            created.status, created.body
        );
    }
}

#[tokio::test]
async fn calculator_arn_carries_the_region_the_request_is_addressed_to() {
    // One ARN per request region, so the region segment is read from the request
    // rather than baked in.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let router = emulator();
        let body = bytes::Bytes::from(
            serde_json::to_vec(&json!({ "CalculatorName": "regional", "DataSource": "Here" }))
                .expect("body serializes"),
        );
        let created = call_in_region(&router, "POST", "/routes/v0/calculators", body, region).await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: CreateRouteCalculator must succeed, got {} {:?}",
            created.status, created.body
        );
        let expected = format!("arn:aws:geo:{region}:{ACCOUNT}:route-calculator/regional");
        assert_eq!(
            created.json().get("CalculatorArn").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {region:?}: expected CalculatorArn {expected}, got {:?}",
            created.body
        );
    }
}

#[tokio::test]
async fn recreating_a_calculator_is_a_conflict_and_leaves_the_stored_one_alone() {
    let router = emulator();
    let first = create_calculator(
        &router,
        json!({
            "CalculatorName": "twice",
            "DataSource": "Esri",
            "Description": "the stored one",
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {} {:?}",
        first.status, first.body
    );
    let stored_create_time = millis_of(&first.json(), "CreateTime");

    // A second create for the same name, with different members, must not
    // overwrite anything.
    for data_source in ["Here", "Grab"] {
        let again = create_calculator(
            &router,
            json!({
                "CalculatorName": "twice",
                "DataSource": data_source,
                "Description": "the overwrite that must not happen",
            }),
        )
        .await;
        assert_eq!(
            again.status, 409,
            "case {data_source:?}: re-creating a name must conflict, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {data_source:?}: expected ConflictException, got {:?} ({:?})",
            again.error_code(),
            again.body
        );
        assert_eq!(
            again.message(),
            "Route calculator already exists: twice.",
            "case {data_source:?}: expected the conflict to name the resource and the requested name, got {:?}",
            again.message()
        );
    }

    let described = describe_calculator(&router, "twice").await;
    let described = described.json();
    assert_eq!(
        described.get("DataSource").and_then(Value::as_str),
        Some("Esri"),
        "the conflicting creates must not change the stored DataSource, got {described}"
    );
    assert_eq!(
        described.get("Description").and_then(Value::as_str),
        Some("the stored one"),
        "the conflicting creates must not change the stored Description, got {described}"
    );
    assert_eq!(
        millis_of(&described, "CreateTime"),
        stored_create_time,
        "the conflicting creates must not change the stored CreateTime, got {described}"
    );
}

#[tokio::test]
async fn describing_a_calculator_that_was_never_created_is_not_found() {
    let router = emulator();
    // One calculator exists; the names below satisfy `^[-._\w]+$` but were never
    // created, including case variants of the one that was: the lookup is by exact
    // name.
    let created = create_calculator(
        &router,
        json!({ "CalculatorName": "Exists", "DataSource": "Esri" }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the positive control must be created, got {} {:?}",
        created.status, created.body
    );

    for name in ["missing", "exists", "EXISTS", "Exists.2", "x"] {
        let described = describe_calculator(&router, name).await;
        assert_eq!(
            described.status, 404,
            "case {name:?}: an unknown calculator must be 404, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.error_code(),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {:?} ({:?})",
            described.error_code(),
            described.body
        );
        let expected = format!("Route calculator not found: {name}.");
        assert_eq!(
            described.message(),
            expected,
            "case {name:?}: expected message {expected:?}, got {:?}",
            described.message()
        );
    }
}

#[tokio::test]
async fn a_read_does_not_change_what_a_later_reader_sees() {
    let router = emulator();
    create_calculator(
        &router,
        json!({ "CalculatorName": "stable", "DataSource": "Grab", "Tags": { "k": "v" } }),
    )
    .await;

    let first = describe_calculator(&router, "stable").await.json();
    for round in 1..=3 {
        let again = describe_calculator(&router, "stable").await;
        assert_eq!(
            again.status, 200,
            "round {round}: the read must keep succeeding, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.json(),
            first,
            "round {round}: DescribeRouteCalculator must report the same resource every time, got {:?}",
            again.body
        );
    }
}
