// SPDX-License-Identifier: CC0-1.0

//! What these write paths REFUSE, end to end over HTTP.
//!
//! Each suite drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — through the harness `application_lifecycle` owns, and asserts
//! the status, the error code, the whole `message` and that the refused request
//! persisted nothing.
//!
//! Four rules are under test, each over a TABLE spanning its class rather than the
//! one request that exposed it:
//!
//! 1. `CreateAppVersionAppComponent$type` has to name a Resilience Hub Application
//!    Component type — one in the service's own component-type namespace — and
//!    anything else (an AWS resource type, a word of the caller's own) is the
//!    modelled 400 `ValidationException`;
//! 2. `CreateAppVersionResource$resourceType` has to name a resource type the
//!    service supports, and that is judged BEFORE any duplicate-resource conflict,
//!    so an unsupported type is always the 400 rather than whichever conflicting
//!    condition the draft happens to hold;
//! 3. `PublishAppVersion` freezes an app template only once it maps at least one
//!    RESOURCE: a draft holding Application Components alone is the modelled 409
//!    `ConflictException`;
//! 4. a `DisruptionPolicy` that has no failure policy for a failure type — a key
//!    outside the modelled `DisruptionType` enum, or one of the failure types every
//!    policy has to state left out — is refused from either writer of a disruption
//!    set.
//!
//! The existence half of the mapping rule — a `physicalResourceId` the service that
//! OWNS that resource class says it holds nothing under — needs two crates to agree
//! and so lives in `core/router/tests/all/resiliencehub_resource_mapping_existence.rs`.

use serde_json::{json, Value};

use super::application_lifecycle::{emulator, post, required_objectives, Reply};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::SUPPORTED_RESOURCE_TYPES;
use smithy_server_runtime::MultiServiceRouter;

/// The Application Component types the Resilience Hub user guide's "Grouping
/// resources in an AppComponent" names, one per tier it groups resources into.
///
/// Transcribed from that documentation rather than from the model, which constrains
/// `CreateAppVersionAppComponent$type` only by length (`String255`, `smithy_query
/// query="operation" operation="CreateAppVersionAppComponent"`).
const DOCUMENTED_COMPONENT_TYPES: &[&str] = &[
    "AWS::ResilienceHub::ComputeAppComponent",
    "AWS::ResilienceHub::DatabaseAppComponent",
    "AWS::ResilienceHub::StorageAppComponent",
    "AWS::ResilienceHub::NetworkingAppComponent",
    "AWS::ResilienceHub::MessageQueueAppComponent",
    "AWS::ResilienceHub::AppCommonAppComponent",
];

/// The refusal a publish of a draft whose template maps no resource earns: FIXED
/// text carrying no part of the request.
const EMPTY_TEMPLATE_MESSAGE: &str =
    "Invalid app template. App template must have at least 1 resource";

/// Create an application named `name` and answer its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// The `message` a reply carried.
fn message(reply: &Reply) -> String {
    reply.body["message"].as_str().unwrap_or_default().to_string()
}

/// Map one resource of `resource_type` into `app_arn`'s draft, under the logical
/// identifier `logical`.
async fn map_a_resource(
    router: &MultiServiceRouter,
    app_arn: &str,
    logical: &str,
    resource_type: &str,
) -> Reply {
    post(
        router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "logicalResourceId": {"identifier": logical},
            "physicalResourceId": format!("{logical}-physical"),
            "resourceType": resource_type,
            "appComponents": ["appcommon"],
        }),
    )
    .await
}

/// Whether `app_arn`'s draft holds a resource under the logical identifier
/// `logical`, as the implemented reader answers.
async fn draft_holds_resource(router: &MultiServiceRouter, app_arn: &str, logical: &str) -> bool {
    let reply = post(
        router,
        "/describe-app-version-resource",
        &json!({
            "appArn": app_arn,
            "appVersion": "draft",
            "logicalResourceId": {"identifier": logical},
        }),
    )
    .await;
    reply.status == 200
}

/// A `type` that names no Application Component type is refused with the modelled
/// 400, naming the value the request carried — and no component is created.
///
/// The rows differ along HOW the value misses the service's own component-type
/// namespace: an AWS resource type of another service (the form a caller reaches for
/// most easily), a resource type of Resilience Hub's own namespace that is not a
/// component type, a namespace in the wrong case, a type with no tier in it, a tier
/// with a further namespace segment inside it, and a free-form word.
#[tokio::test]
async fn a_type_that_names_no_app_component_type_is_refused() {
    let router = emulator();
    let app_arn = create_an_app(&router, "component-type-refusals").await;

    for (case, component_type) in [
        ("an AWS resource type", "AWS::SNS::Topic"),
        ("another AWS resource type", "AWS::S3::Bucket"),
        ("a Resilience Hub type that is not a component", "AWS::ResilienceHub::App"),
        ("the namespace in the wrong case", "aws::resiliencehub::ComputeAppComponent"),
        ("a tier with no suffix", "AWS::ResilienceHub::Compute"),
        ("the suffix with no tier", "AWS::ResilienceHub::AppComponent"),
        (
            "a further namespace segment inside the tier",
            "AWS::ResilienceHub::Compute::AppComponent",
        ),
        ("a word of the caller's own", "my-own-component-type"),
        ("a single character", "x"),
    ] {
        let refused = post(
            &router,
            "/create-app-version-app-component",
            &json!({"appArn": app_arn, "id": case, "name": case, "type": component_type}),
        )
        .await;
        refused.refused(case, 400, "ValidationException");
        let expected = format!("Invalid AppComponent type: {component_type}.");
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );

        // Nothing was stored: the draft answers no component under that id.
        let read = post(
            &router,
            "/describe-app-version-app-component",
            &json!({"appArn": app_arn, "appVersion": "draft", "id": case}),
        )
        .await;
        read.refused(case, 404, "ResourceNotFoundException");
    }
}

/// Every documented Application Component type is accepted and read back unchanged —
/// so the refusal above is about the type naming no component type, not about this
/// operation refusing types at large.
#[tokio::test]
async fn every_documented_app_component_type_is_accepted() {
    let router = emulator();
    let app_arn = create_an_app(&router, "component-type-catalogue").await;

    for (index, component_type) in DOCUMENTED_COMPONENT_TYPES.iter().enumerate() {
        let id = format!("tier-{index}");
        let created = post(
            &router,
            "/create-app-version-app-component",
            &json!({"appArn": app_arn, "id": id, "name": id, "type": component_type}),
        )
        .await;
        let body = created.ok(component_type);
        assert_eq!(
            body["appComponent"]["type"].as_str(),
            Some(*component_type),
            "case {component_type:?}: the component must be stored with the type the \
             request named, got {body:?}"
        );

        let read = post(
            &router,
            "/describe-app-version-app-component",
            &json!({"appArn": app_arn, "appVersion": "draft", "id": id}),
        )
        .await;
        let read = read.ok(component_type);
        assert_eq!(
            read["appComponent"]["type"].as_str(),
            Some(*component_type),
            "case {component_type:?}: the stored component must read back with that type, \
             got {read:?}"
        );
    }
}

/// A `resourceType` the service does not support is refused with the modelled 400
/// naming the value — and the refusal is the TYPE's, whatever else about the request
/// also conflicts: the type is judged before the draft is consulted at all.
///
/// The rows differ along how the value misses the catalogue: a service namespace
/// that exists with a resource that does not, a namespace nothing publishes, a
/// supported type in the wrong case, a supported type misspelled, and a bare service
/// name.
#[tokio::test]
async fn an_unsupported_resource_type_is_refused_before_any_conflict() {
    let router = emulator();
    let app_arn = create_an_app(&router, "resource-type-refusals").await;

    // A resource the draft already holds, so each row below ALSO duplicates a
    // logical identifier and a resource name that are already taken — the condition
    // this operation answers its 409 `ConflictException` for.
    map_a_resource(&router, &app_arn, "HeldResource", "AWS::S3::Bucket")
        .await
        .ok("the resource the draft already holds");

    for (case, resource_type) in [
        ("a resource nothing publishes under a real namespace", "AWS::S3::Buckets"),
        ("a namespace nothing publishes", "AWS::Bogus::Type"),
        ("a supported type in the wrong case", "AWS::SQS::queue"),
        ("a supported type misspelled", "AWS::DynamoDB::Tables"),
        ("a bare service name", "s3"),
        ("a word of the caller's own", "my-own-resource-type"),
    ] {
        // The SAME logical identifier the draft already holds: AWS judges the type
        // first, so this is the 400 and never the duplicate's 409.
        let refused = map_a_resource(&router, &app_arn, "HeldResource", resource_type).await;
        refused.refused(case, 400, "ValidationException");
        let expected = format!("Invalid resource type: {resource_type}");
        let actual = message(&refused);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }

    // Not vacuous: the same request with a SUPPORTED type is the duplicate's 409, so
    // the rows above were refused for their type and not for being duplicates.
    let conflicted = map_a_resource(&router, &app_arn, "HeldResource", "AWS::S3::Bucket").await;
    conflicted.refused("a supported type on a held identifier", 409, "ConflictException");
}

/// Every resource type in the service's catalogue is accepted and read back
/// unchanged, so the refusal above is about the catalogue and not about this
/// operation refusing types at large.
#[tokio::test]
async fn every_supported_resource_type_is_accepted() {
    let router = emulator();
    let app_arn = create_an_app(&router, "resource-type-catalogue").await;

    for (index, resource_type) in SUPPORTED_RESOURCE_TYPES.iter().enumerate() {
        let logical = format!("Resource{index}");
        let created = map_a_resource(&router, &app_arn, &logical, resource_type).await;
        let body = created.ok(resource_type);
        assert_eq!(
            body["physicalResource"]["resourceType"].as_str(),
            Some(*resource_type),
            "case {resource_type:?}: the resource must be stored with the type the request \
             named, got {body:?}"
        );
        assert!(
            draft_holds_resource(&router, &app_arn, &logical).await,
            "case {resource_type:?}: the mapped resource must read back from the draft"
        );
    }
}

/// One row of the publish table: what the draft holds when the publish is attempted.
struct Draft {
    label: &'static str,
    /// The Application Component ids created in the draft.
    components: &'static [&'static str],
    /// The logical identifiers of the resources mapped into the draft, then deleted
    /// again.
    deleted_resources: &'static [&'static str],
}

/// A publish is refused until the draft's app template maps a RESOURCE — the
/// Application Components it holds do not make it publishable, however many there
/// are — and the refused publish mints nothing.
///
/// The rows differ along what the unpublishable draft holds: nothing at all, one
/// component, several components, and a draft that mapped a resource and had it
/// deleted again.
#[tokio::test]
async fn publishing_requires_the_draft_to_map_a_resource() {
    let drafts = [
        Draft {
            label: "a draft nobody has written to",
            components: &[],
            deleted_resources: &[],
        },
        Draft {
            label: "a draft holding one Application Component",
            components: &["compute-tier"],
            deleted_resources: &[],
        },
        Draft {
            label: "a draft holding three Application Components",
            components: &["compute-tier", "storage-tier", "database-tier"],
            deleted_resources: &[],
        },
        Draft {
            label: "a draft whose mapped resource was deleted again",
            components: &["compute-tier"],
            deleted_resources: &["OnlyResource"],
        },
    ];

    for (index, draft) in drafts.iter().enumerate() {
        let router = emulator();
        let case = draft.label;
        let app_arn = create_an_app(&router, &format!("publish-precondition-{index}")).await;

        for id in draft.components {
            post(
                &router,
                "/create-app-version-app-component",
                &json!({
                    "appArn": app_arn,
                    "id": id,
                    "name": id,
                    "type": "AWS::ResilienceHub::ComputeAppComponent",
                }),
            )
            .await
            .ok(case);
        }
        for logical in draft.deleted_resources {
            map_a_resource(&router, &app_arn, logical, "AWS::S3::Bucket")
                .await
                .ok(case);
            post(
                &router,
                "/delete-app-version-resource",
                &json!({
                    "appArn": app_arn,
                    "logicalResourceId": {"identifier": logical},
                }),
            )
            .await
            .ok(case);
        }

        let refused = post(&router, "/publish-app-version", &json!({"appArn": app_arn})).await;
        refused.refused(case, 409, "ConflictException");
        let actual = message(&refused);
        assert_eq!(
            actual, EMPTY_TEMPLATE_MESSAGE,
            "case {case:?}: expected {EMPTY_TEMPLATE_MESSAGE:?}, got {actual:?}"
        );

        // The refused publish minted nothing: the application has no release to read.
        let release = post(
            &router,
            "/describe-app-version-template",
            &json!({"appArn": app_arn, "appVersion": "release"}),
        )
        .await;
        release.refused(case, 404, "ResourceNotFoundException");

        // Mapping ONE resource is what makes the same request succeed.
        map_a_resource(&router, &app_arn, "PublishableResource", "AWS::S3::Bucket")
            .await
            .ok(case);
        let published = post(&router, "/publish-app-version", &json!({"appArn": app_arn})).await;
        published.ok(case);
    }
}

/// A `DisruptionPolicy` the service will not store is refused with the modelled 400
/// naming the failure type it has no failure policy for, from EITHER writer of a
/// disruption set.
///
/// Two ways a policy misses that, over one table: a map KEY outside the modelled
/// `DisruptionType` names no failure type at all (the keys are CASE-SENSITIVE, and
/// the model declares the values as Software, Hardware, AZ, Region; `smithy_query
/// query="shape" shape="DisruptionType"`), and a set that leaves out one of the
/// failure types every policy has to state — the application (`Software`),
/// infrastructure (`Hardware`) and Availability Zone objectives — has no failure
/// policy for that one. A missing failure type is named as the service's own
/// `FailureType` vocabulary spells it, in upper case; a key the request carried is
/// named as it was sent.
#[tokio::test]
async fn a_key_outside_the_modelled_disruption_types_does_not_exist() {
    let router = emulator();

    // A policy to update, so the rule is exercised through both writers.
    let created = post(
        &router,
        "/create-resiliency-policy",
        &json!({
            "policyName": "disruption-key-policy",
            "tier": "Important",
            "policy": required_objectives(3600),
        }),
    )
    .await;
    let policy_arn = created.ok("the policy to update")["policy"]["policyArn"]
        .as_str()
        .expect("a minted policyArn")
        .to_string();

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an upper-cased disruption type",
            json!({"HARDWARE": {"rtoInSecs": 3600, "rpoInSecs": 3600}}),
            "HARDWARE",
        ),
        (
            "a lower-cased disruption type",
            json!({"software": {"rtoInSecs": 60, "rpoInSecs": 30}}),
            "software",
        ),
        (
            "a modelled type carrying trailing space",
            json!({"Region ": {"rtoInSecs": 60, "rpoInSecs": 30}}),
            "Region ",
        ),
        (
            "a disruption the model does not name",
            json!({"Datacenter": {"rtoInSecs": 60, "rpoInSecs": 30}}),
            "Datacenter",
        ),
        (
            "one modelled key beside one that is not",
            json!({
                "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                "Hardware": {"rtoInSecs": 60, "rpoInSecs": 30},
                "AZ": {"rtoInSecs": 60, "rpoInSecs": 30},
                "Continent": {"rtoInSecs": 60, "rpoInSecs": 30},
            }),
            "Continent",
        ),
        // A set of modelled keys that leaves out a required failure type: the
        // missing one is named in upper case, in the order the model declares the
        // values in.
        ("no objective at all", json!({}), "SOFTWARE"),
        (
            "the application objective left out",
            json!({
                "Hardware": {"rtoInSecs": 60, "rpoInSecs": 30},
                "AZ": {"rtoInSecs": 60, "rpoInSecs": 30},
            }),
            "SOFTWARE",
        ),
        (
            "the infrastructure objective left out",
            json!({
                "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                "AZ": {"rtoInSecs": 60, "rpoInSecs": 30},
            }),
            "HARDWARE",
        ),
        (
            "the Availability Zone objective left out",
            json!({
                "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                "Hardware": {"rtoInSecs": 60, "rpoInSecs": 30},
            }),
            "AZ",
        ),
        (
            "the multi-Region objective alone",
            json!({"Region": {"rtoInSecs": 60, "rpoInSecs": 30}}),
            "SOFTWARE",
        ),
    ];

    for (case, policy, key) in rows {
        let expected = format!("FailureType {key} does not exist");

        let refused_create = post(
            &router,
            "/create-resiliency-policy",
            &json!({
                "policyName": "refused-policy",
                "tier": "Critical",
                "policy": policy,
            }),
        )
        .await;
        refused_create.refused(case, 400, "ValidationException");
        let actual = message(&refused_create);
        assert_eq!(
            actual, expected,
            "case {case:?}: CreateResiliencyPolicy expected {expected:?}, got {actual:?}"
        );

        let refused_update = post(
            &router,
            "/update-resiliency-policy",
            &json!({"policyArn": policy_arn, "policy": policy}),
        )
        .await;
        refused_update.refused(case, 400, "ValidationException");
        let actual = message(&refused_update);
        assert_eq!(
            actual, expected,
            "case {case:?}: UpdateResiliencyPolicy expected {expected:?}, got {actual:?}"
        );

        // The refused update changed nothing: the policy still states the
        // objectives its create carried.
        let read = post(
            &router,
            "/describe-resiliency-policy",
            &json!({"policyArn": policy_arn}),
        )
        .await;
        let read = read.ok(case);
        assert_eq!(
            read["policy"]["policy"],
            required_objectives(3600),
            "case {case:?}: a refused update must leave the disruption set as it was, got \
             {read:?}"
        );
    }

    // Not vacuous: the required three alone and the required three beside the
    // multi-Region objective are both accepted, so the rows above were refused for
    // what their sets hold and not for carrying a disruption set at all.
    let accepted = post(
        &router,
        "/create-resiliency-policy",
        &json!({
            "policyName": "required-disruption-policy",
            "tier": "Important",
            "policy": required_objectives(600),
        }),
    )
    .await;
    accepted.ok("the required disruption types");
    let accepted = post(
        &router,
        "/create-resiliency-policy",
        &json!({
            "policyName": "every-disruption-policy",
            "tier": "MissionCritical",
            "policy": {
                "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                "Hardware": {"rtoInSecs": 600, "rpoInSecs": 300},
                "AZ": {"rtoInSecs": 3600, "rpoInSecs": 1800},
                "Region": {"rtoInSecs": 86400, "rpoInSecs": 3600},
            },
        }),
    )
    .await;
    accepted.ok("every modelled disruption type");
}

/// The longest recovery objective a failure policy can state: thirty days, in
/// seconds. The model bounds the members only from below (`@range(min: 0)` on
/// `FailurePolicy$rtoInSecs`/`$rpoInSecs`, `smithy_query query="shape"
/// shape="FailurePolicy"`), so this is the service's own ceiling.
const MAX_OBJECTIVE_SECONDS: i64 = 30 * 24 * 60 * 60;

/// An objective past the longest one a failure policy can state is refused with the
/// modelled 400, naming the failure type, which of its two objectives, and the value
/// the request carried — while each published bound is accepted.
///
/// The rows differ along what the rule generalises over: which objective carries the
/// value (the recovery time or the recovery point), which failure type states it (a
/// required one and the optional multi-Region one), and where the value sits
/// relative to the range — its bottom, its ceiling, and one second past the ceiling.
#[tokio::test]
async fn an_objective_past_the_longest_one_is_refused() {
    let router = emulator();

    // (case, the disruption set, the refusal expected of it — `None` for a set the
    // service stores).
    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "every objective at the bottom of the range",
            required_objectives(0),
            None,
        ),
        (
            "every objective at the ceiling",
            required_objectives(MAX_OBJECTIVE_SECONDS),
            None,
        ),
        (
            "the application recovery point one second past the ceiling",
            json!({
                "Software": {"rtoInSecs": MAX_OBJECTIVE_SECONDS, "rpoInSecs": MAX_OBJECTIVE_SECONDS + 1},
                "Hardware": {"rtoInSecs": 600, "rpoInSecs": 300},
                "AZ": {"rtoInSecs": 3600, "rpoInSecs": 1800},
            }),
            Some(format!(
                "FailureType SOFTWARE RPO {} seconds is invalid",
                MAX_OBJECTIVE_SECONDS + 1
            )),
        ),
        (
            "the Availability Zone recovery time one second past the ceiling",
            json!({
                "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                "Hardware": {"rtoInSecs": 600, "rpoInSecs": 300},
                "AZ": {"rtoInSecs": MAX_OBJECTIVE_SECONDS + 1, "rpoInSecs": 1800},
            }),
            Some(format!(
                "FailureType AZ RTO {} seconds is invalid",
                MAX_OBJECTIVE_SECONDS + 1
            )),
        ),
        (
            "the multi-Region recovery time far past the ceiling",
            json!({
                "Software": {"rtoInSecs": 60, "rpoInSecs": 30},
                "Hardware": {"rtoInSecs": 600, "rpoInSecs": 300},
                "AZ": {"rtoInSecs": 3600, "rpoInSecs": 1800},
                "Region": {"rtoInSecs": MAX_OBJECTIVE_SECONDS * 2, "rpoInSecs": 3600},
            }),
            Some(format!(
                "FailureType REGION RTO {} seconds is invalid",
                MAX_OBJECTIVE_SECONDS * 2
            )),
        ),
    ];

    for (index, (case, policy, expected)) in rows.into_iter().enumerate() {
        let reply = post(
            &router,
            "/create-resiliency-policy",
            &json!({
                "policyName": format!("objective-range-{index}"),
                "tier": "Important",
                "policy": policy,
            }),
        )
        .await;
        match expected {
            None => {
                let body = reply.ok(case);
                assert_eq!(
                    body["policy"]["policy"], policy,
                    "case {case:?}: the policy must be stored with the objectives the request \
                     stated, got {body:?}"
                );
            }
            Some(expected) => {
                reply.refused(case, 400, "ValidationException");
                let actual = message(&reply);
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }
}
