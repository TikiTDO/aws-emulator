// SPDX-License-Identifier: CC0-1.0

//! The `appTemplateBody` DOCUMENT a template read renders, and the refusal every
//! operation that addresses an app version states for one the application does not
//! have — end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! Two rules are under test:
//!
//! 1. the document is the whole structure the model documents for `appTemplateBody`
//!    (`smithy_query(query="doc",shape="DescribeAppVersionTemplateResponse",member="appTemplateBody")`):
//!    `resources`, `appComponents`, `excludedResources` and `version` are ALWAYS
//!    present, each resource's `logicalResourceId` states the CloudFormation stack
//!    and the resource group it belongs to — as JSON `null` when it belongs to
//!    neither — and names a Terraform or EKS source only when it has one, each
//!    Application Component carries the `id` it is addressed by, the AppCommon
//!    component the service maintains is part of every version's component set, and
//!    `version` is the numeric version of the DOCUMENT rather than the app version
//!    label the read carried;
//! 2. an `appVersion` the application does not have is refused with
//!    `ResourceNotFoundException`/404 and the one sentence this service states for
//!    that refusal, from every operation that resolves an app version.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::application_lifecycle::{emulator, post, Reply, ACCOUNT, REGION};

/// The members the document always carries — the four the model's structure names
/// (`smithy_query(query="doc",shape="DescribeAppVersionTemplateResponse",member="appTemplateBody")`).
const DOCUMENT_MEMBERS: [&str; 4] = [
    "appComponents",
    "excludedResources",
    "resources",
    "version",
];

/// The version of the app-template DOCUMENT, which describes the document's own
/// shape and so is the same for every app version of every application.
const DOCUMENT_VERSION: f64 = 2.0;

/// The `id`, `name` and `type` of the AppCommon Application Component the service
/// maintains in every app version — the component a caller never created and
/// cannot delete.
const APP_COMMON: (&str, &str) = ("appcommon", "AWS::ResilienceHub::AppCommonAppComponent");

/// An application, and its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// Add one Application Component to the draft, under an `id` of its own so a case
/// can tell the rendered `id` from the rendered `name`.
async fn add_a_component(
    router: &MultiServiceRouter,
    app_arn: &str,
    id: &str,
    name: &str,
    kind: &str,
) {
    post(
        router,
        "/create-app-version-app-component",
        &json!({"appArn": app_arn, "id": id, "name": name, "type": kind}),
    )
    .await
    .ok(id);
}

/// `DescribeAppVersionTemplate`'s reply for one version.
async fn read_template(
    router: &MultiServiceRouter,
    app_arn: &str,
    app_version: &str,
) -> Reply {
    post(
        router,
        "/describe-app-version-template",
        &json!({"appArn": app_arn, "appVersion": app_version}),
    )
    .await
}

/// The RAW `appTemplateBody` string of one version.
async fn document(router: &MultiServiceRouter, app_arn: &str, app_version: &str) -> String {
    let case = format!("the template of version {app_version}");
    let reply = read_template(router, app_arn, app_version).await;
    reply.ok(&case)["appTemplateBody"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: appTemplateBody is @required"))
        .to_string()
}

/// That body, parsed.
async fn template(router: &MultiServiceRouter, app_arn: &str, app_version: &str) -> Value {
    let rendered = document(router, app_arn, app_version).await;
    serde_json::from_str(&rendered).unwrap_or_else(|error| {
        panic!("the appTemplateBody must be JSON ({error}), got {rendered:?}")
    })
}

/// The document's resources, keyed by their logical identifier.
fn resources_by_identifier(body: &Value) -> BTreeMap<String, Value> {
    body["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("the document carries a resources array, got {body:?}"))
        .iter()
        .map(|resource| {
            let identifier = resource["logicalResourceId"]["identifier"]
                .as_str()
                .unwrap_or_else(|| panic!("every resource has a logical identifier, got {resource:?}"))
                .to_string();
            (identifier, resource.clone())
        })
        .collect()
}

/// The document's Application Components, keyed by the `id` they are addressed by,
/// with each one's `resourceNames` sorted: the assignment is a SET, so a case
/// asserts membership rather than the order the renderer walked its resources in.
fn components_by_id(body: &Value) -> BTreeMap<String, Value> {
    body["appComponents"]
        .as_array()
        .unwrap_or_else(|| panic!("the document carries an appComponents array, got {body:?}"))
        .iter()
        .map(|component| {
            let mut component = component.clone();
            let mut names: Vec<String> = component["resourceNames"]
                .as_array()
                .unwrap_or_else(|| panic!("every component has resourceNames, got {component:?}"))
                .iter()
                .map(|name| name.as_str().unwrap_or_default().to_string())
                .collect();
            names.sort();
            component["resourceNames"] = json!(names);
            let id = component["id"]
                .as_str()
                .unwrap_or_else(|| panic!("every component carries its id, got {component:?}"))
                .to_string();
            (id, component)
        })
        .collect()
}

/// One row of the resource table: the `logicalResourceId` members the resource is
/// created with beyond its `identifier`, and the object the document must render
/// for it.
struct Resource {
    label: &'static str,
    name: &'static str,
    component: &'static str,
    resource_type: &'static str,
    /// Merged into the created resource's `logicalResourceId`.
    source: Value,
    expected: Value,
}

/// The logical identifier the suite gives the resource named `name`.
fn logical_identifier(name: &str) -> String {
    format!("Logical-{name}")
}

/// The table. The rows differ along what the rule generalises over: which source
/// a resource belongs to — none, a CloudFormation stack, a resource group, a
/// Terraform state file, an EKS cluster namespace, all of them at once — and
/// whether it carries an `additionalInfo` map.
fn resource_table() -> Vec<Resource> {
    vec![
        Resource {
            label: "a resource belonging to no source",
            name: "plain-bucket",
            component: "storage",
            resource_type: "AWS::S3::Bucket",
            source: json!({}),
            expected: json!({
                "logicalResourceId": {
                    "identifier": "Logical-plain-bucket",
                    "logicalStackName": null,
                    "resourceGroupName": null,
                },
                "type": "AWS::S3::Bucket",
                "name": "plain-bucket",
            }),
        },
        Resource {
            label: "a resource in a CloudFormation stack",
            name: "stacked-table",
            component: "storage",
            resource_type: "AWS::DynamoDB::Table",
            source: json!({"logicalStackName": "orders-stack"}),
            expected: json!({
                "logicalResourceId": {
                    "identifier": "Logical-stacked-table",
                    "logicalStackName": "orders-stack",
                    "resourceGroupName": null,
                },
                "type": "AWS::DynamoDB::Table",
                "name": "stacked-table",
            }),
        },
        Resource {
            label: "a resource in a resource group",
            name: "grouped-queue",
            component: "storage",
            resource_type: "AWS::SQS::Queue",
            source: json!({"resourceGroupName": "orders-group"}),
            expected: json!({
                "logicalResourceId": {
                    "identifier": "Logical-grouped-queue",
                    "logicalStackName": null,
                    "resourceGroupName": "orders-group",
                },
                "type": "AWS::SQS::Queue",
                "name": "grouped-queue",
            }),
        },
        Resource {
            label: "a resource from a Terraform state file",
            name: "terraformed-topic",
            component: "messaging",
            resource_type: "AWS::SNS::Topic",
            source: json!({"terraformSourceName": "orders.tfstate"}),
            expected: json!({
                "logicalResourceId": {
                    "identifier": "Logical-terraformed-topic",
                    "logicalStackName": null,
                    "resourceGroupName": null,
                    "terraformSourceName": "orders.tfstate",
                },
                "type": "AWS::SNS::Topic",
                "name": "terraformed-topic",
            }),
        },
        Resource {
            label: "a resource from an EKS cluster namespace",
            name: "clustered-service",
            component: "compute",
            resource_type: "AWS::EKS::Cluster",
            source: json!({"eksSourceName": "orders-cluster/orders"}),
            expected: json!({
                "logicalResourceId": {
                    "identifier": "Logical-clustered-service",
                    "logicalStackName": null,
                    "resourceGroupName": null,
                    "eksSourceName": "orders-cluster/orders",
                },
                "type": "AWS::EKS::Cluster",
                "name": "clustered-service",
            }),
        },
        Resource {
            label: "a resource naming every source member, with an additionalInfo map",
            name: "every-source",
            component: "compute",
            resource_type: "AWS::AutoScaling::AutoScalingGroup",
            source: json!({
                "logicalStackName": "orders-stack",
                "resourceGroupName": "orders-group",
                "terraformSourceName": "orders.tfstate",
                "eksSourceName": "orders-cluster/orders",
            }),
            expected: json!({
                "logicalResourceId": {
                    "identifier": "Logical-every-source",
                    "logicalStackName": "orders-stack",
                    "resourceGroupName": "orders-group",
                    "terraformSourceName": "orders.tfstate",
                    "eksSourceName": "orders-cluster/orders",
                },
                "type": "AWS::AutoScaling::AutoScalingGroup",
                "name": "every-source",
                "additionalInfo": {"failover-regions": ["us-west-2"]},
            }),
        },
    ]
}

/// Add one row's resource to the draft.
async fn add_a_resource(router: &MultiServiceRouter, app_arn: &str, row: &Resource) {
    let mut logical = json!({"identifier": logical_identifier(row.name)});
    for (member, value) in row.source.as_object().into_iter().flatten() {
        logical[member] = value.clone();
    }
    let mut request = json!({
        "appArn": app_arn,
        "appComponents": [row.component],
        "resourceName": row.name,
        "resourceType": row.resource_type,
        "physicalResourceId": row.name,
        "logicalResourceId": logical,
    });
    if let Some(info) = row.expected.get("additionalInfo") {
        request["additionalInfo"] = info.clone();
    }
    post(router, "/create-app-version-resource", &request)
        .await
        .ok(row.label);
}

/// The document carries every documented member for every form of resource: the
/// source members the resource does not have as JSON `null`, each component's own
/// `id`, the AppCommon component, an always-present `excludedResources`, and the
/// numeric document version — and a publish carries all of it across unchanged.
#[tokio::test]
async fn the_document_carries_every_documented_member() {
    let router = emulator();
    let app_arn = create_an_app(&router, "document-members").await;

    // The components the rows assign resources to, each under an id of its own.
    let components: Vec<(&str, &str, &str)> = vec![
        ("storage", "storage-tier", "AWS::ResilienceHub::StorageAppComponent"),
        ("compute", "compute-tier", "AWS::ResilienceHub::ComputeAppComponent"),
        (
            "messaging",
            "messaging-tier",
            "AWS::ResilienceHub::MessagingAppComponent",
        ),
    ];
    for (id, name, kind) in &components {
        add_a_component(&router, &app_arn, id, name, kind).await;
    }

    let mut expected_resources: BTreeMap<String, Value> = BTreeMap::new();
    for row in resource_table() {
        add_a_resource(&router, &app_arn, &row).await;
        expected_resources.insert(logical_identifier(row.name), row.expected.clone());
    }

    // What each component must render: its own id and name, its type, and the
    // names of the resources assigned to it.
    let mut expected_components: BTreeMap<String, Value> = BTreeMap::new();
    for (id, name, kind) in &components {
        let mut assigned: Vec<String> = resource_table()
            .iter()
            .filter(|row| row.component == *id)
            .map(|row| row.name.to_string())
            .collect();
        assigned.sort();
        expected_components.insert(
            (*id).to_string(),
            json!({"id": id, "name": name, "type": kind, "resourceNames": assigned}),
        );
    }
    // The component the service maintains: nothing is assigned to it, so it
    // carries an empty `resourceNames`.
    expected_components.insert(
        APP_COMMON.0.to_string(),
        json!({
            "id": APP_COMMON.0,
            "name": APP_COMMON.0,
            "type": APP_COMMON.1,
            "resourceNames": [],
        }),
    );

    let draft = template(&router, &app_arn, "draft").await;
    for (identifier, expected) in &expected_resources {
        let rendered = resources_by_identifier(&draft);
        let actual = rendered.get(identifier).unwrap_or(&Value::Null);
        assert_eq!(
            actual, expected,
            "case {identifier:?}: expected the rendered resource {expected:?}, got \
             {actual:?}"
        );
    }
    assert_eq!(
        resources_by_identifier(&draft).len(),
        expected_resources.len(),
        "case \"the draft document\": expected {} resources, got {draft:?}",
        expected_resources.len()
    );
    assert_eq!(
        components_by_id(&draft),
        expected_components,
        "case \"the draft document\": expected the components {expected_components:?}, \
         got {draft:?}"
    );
    assert_eq!(
        draft["excludedResources"],
        json!({"logicalResourceIds": []}),
        "case \"the draft document\": nothing is excluded, so excludedResources is \
         present and empty, got {draft:?}"
    );
    let members: BTreeSet<String> = draft
        .as_object()
        .unwrap_or_else(|| panic!("the document is a JSON object, got {draft:?}"))
        .keys()
        .cloned()
        .collect();
    let wanted: BTreeSet<String> = DOCUMENT_MEMBERS.iter().map(|m| (*m).to_string()).collect();
    assert_eq!(
        members, wanted,
        "case \"the draft document\": expected the members {wanted:?}, got {members:?}"
    );

    // `version` describes the DOCUMENT: a number, never the `appVersion` label the
    // read carried.
    for app_version in ["draft", "release"] {
        if app_version == "release" {
            post(&router, "/publish-app-version", &json!({"appArn": app_arn}))
                .await
                .ok("publish the draft");
        }
        let body = template(&router, &app_arn, app_version).await;
        assert_eq!(
            body["version"],
            json!(DOCUMENT_VERSION),
            "case {app_version:?}: expected the document version \
             {DOCUMENT_VERSION}, got {body:?}"
        );
        assert!(
            body["version"].is_number(),
            "case {app_version:?}: the document version is a number, never the \
             appVersion label of the read, got {body:?}"
        );
    }

    // The publish froze the draft: the release document is the draft's, byte for
    // byte, so every member above is carried across rather than re-derived from
    // the label the read used.
    let drafted = document(&router, &app_arn, "draft").await;
    let released = document(&router, &app_arn, "release").await;
    assert_eq!(
        released, drafted,
        "case \"the release document\": the published snapshot renders the document \
         its draft did, expected {drafted:?}, got {released:?}"
    );
}

/// The AppCommon component the document names is REAL: the component reader
/// answers it for every app version — one nothing has been written to, a draft
/// holding components of its own, and a published release — and reports the same
/// id, name and type the document does.
#[tokio::test]
async fn the_app_common_component_is_readable_in_every_app_version() {
    let router = emulator();
    let untouched = create_an_app(&router, "appcommon-untouched").await;
    let composed = create_an_app(&router, "appcommon-composed").await;
    add_a_component(
        &router,
        &composed,
        "storage",
        "storage-tier",
        "AWS::ResilienceHub::StorageAppComponent",
    )
    .await;
    post(&router, "/create-app-version-resource", &json!({
        "appArn": composed,
        "appComponents": ["storage"],
        "resourceName": "only-bucket",
        "resourceType": "AWS::S3::Bucket",
        "physicalResourceId": "only-bucket",
        "logicalResourceId": {"identifier": "OnlyBucket"},
    }))
    .await
    .ok("a resource to publish");
    post(&router, "/publish-app-version", &json!({"appArn": composed}))
        .await
        .ok("publish the draft");

    for (case, app_arn, app_version) in [
        ("an untouched draft", untouched.as_str(), "draft"),
        ("a composed draft", composed.as_str(), "draft"),
        ("a published release", composed.as_str(), "release"),
    ] {
        let read = post(
            &router,
            "/describe-app-version-app-component",
            &json!({"appArn": app_arn, "appVersion": app_version, "id": APP_COMMON.0}),
        )
        .await;
        let body = read.ok(case);
        assert_eq!(
            body["appComponent"],
            json!({
                "id": APP_COMMON.0,
                "name": APP_COMMON.0,
                "type": APP_COMMON.1,
                "additionalInfo": {},
            }),
            "case {case:?}: the AppCommon component must read back as the document \
             names it, got {body:?}"
        );

        // And the document names exactly that component.
        let rendered = template(&router, app_arn, app_version).await;
        let listed = components_by_id(&rendered);
        let entry = listed.get(APP_COMMON.0).unwrap_or(&Value::Null);
        assert_eq!(
            (entry["id"].clone(), entry["type"].clone()),
            (json!(APP_COMMON.0), json!(APP_COMMON.1)),
            "case {case:?}: the document must carry the AppCommon component, got \
             {rendered:?}"
        );
    }
}

/// The AppCommon component is the one the SERVICE maintains: a caller cannot
/// create a second component under its id — by naming that id or by letting the
/// id default to the name — and a resource that names it is ASSIGNED to it rather
/// than quietly creating a copy of it.
#[tokio::test]
async fn the_app_common_component_cannot_be_redefined_and_can_be_assigned_to() {
    let router = emulator();
    let app_arn = create_an_app(&router, "appcommon-identity").await;

    for (case, request) in [
        (
            "the reserved id with another name",
            json!({
                "appArn": app_arn,
                "id": APP_COMMON.0,
                "name": "a-component-of-my-own",
                "type": "AWS::ResilienceHub::StorageAppComponent",
            }),
        ),
        (
            "the reserved id as the name, with no id",
            json!({
                "appArn": app_arn,
                "name": APP_COMMON.0,
                "type": "AWS::ResilienceHub::StorageAppComponent",
            }),
        ),
    ] {
        let refused = post(&router, "/create-app-version-app-component", &request).await;
        assert_eq!(
            (
                refused.status,
                refused.error_code.as_str(),
                refused.body["resourceType"].as_str().unwrap_or_default()
            ),
            (409, "ConflictException", "AppComponent"),
            "case {case:?}: the component the service maintains cannot be created \
             again, got {} {:?} {:?}",
            refused.status,
            refused.error_code,
            refused.body
        );
    }

    // A resource naming it binds to it: the created resource reports that one
    // component, and the document still carries exactly one component.
    let created = post(
        &router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "appComponents": [APP_COMMON.0],
            "resourceName": "common-bucket",
            "resourceType": "AWS::S3::Bucket",
            "physicalResourceId": "common-bucket",
            "logicalResourceId": {"identifier": "CommonBucket"},
        }),
    )
    .await;
    let bound = created.ok("a resource assigned to the AppCommon component")["physicalResource"]
        ["appComponents"]
        .clone();
    assert_eq!(
        (
            bound[0]["id"].clone(),
            bound[0]["name"].clone(),
            bound[0]["type"].clone(),
            bound.as_array().map(Vec::len)
        ),
        (
            json!(APP_COMMON.0),
            json!(APP_COMMON.0),
            json!(APP_COMMON.1),
            Some(1)
        ),
        "case \"a resource assigned to the AppCommon component\": expected the one \
         service-maintained component, got {bound:?}"
    );

    let rendered = template(&router, &app_arn, "draft").await;
    let listed = components_by_id(&rendered);
    assert_eq!(
        listed.keys().cloned().collect::<Vec<String>>(),
        vec![APP_COMMON.0.to_string()],
        "case \"the document after the assignment\": the assignment must not have \
         created a second component, got {rendered:?}"
    );
    assert_eq!(
        listed[APP_COMMON.0]["resourceNames"],
        json!(["common-bucket"]),
        "case \"the document after the assignment\": the AppCommon component must \
         report the resource assigned to it, got {rendered:?}"
    );
}

/// One operation that addresses an app version.
struct Addressed {
    operation: &'static str,
    target: &'static str,
    /// The request members beyond `appArn` and `appVersion`.
    extra: Value,
    /// Whether this operation addresses a version other than the release alias
    /// (`StartAppAssessment` assesses only the current release, so it judges the
    /// label before it resolves it).
    any_version: bool,
}

/// Every operation that resolves an app version, with the input it needs.
fn addressed_operations() -> Vec<Addressed> {
    vec![
        Addressed {
            operation: "DescribeAppVersionTemplate",
            target: "/describe-app-version-template",
            extra: json!({}),
            any_version: true,
        },
        Addressed {
            operation: "ListAppVersionResources",
            target: "/list-app-version-resources",
            extra: json!({}),
            any_version: true,
        },
        Addressed {
            operation: "DescribeAppVersionResource",
            target: "/describe-app-version-resource",
            extra: json!({"resourceName": "only-bucket"}),
            any_version: true,
        },
        Addressed {
            operation: "DescribeAppVersionAppComponent",
            target: "/describe-app-version-app-component",
            extra: json!({"id": "storage"}),
            any_version: true,
        },
        Addressed {
            operation: "ResolveAppVersionResources",
            target: "/resolve-app-version-resources",
            extra: json!({}),
            any_version: true,
        },
        Addressed {
            operation: "DescribeAppVersionResourcesResolutionStatus",
            target: "/describe-app-version-resources-resolution-status",
            extra: json!({}),
            any_version: true,
        },
        Addressed {
            operation: "StartAppAssessment",
            target: "/start-app-assessment",
            extra: json!({"assessmentName": "an-assessment"}),
            any_version: false,
        },
    ]
}

/// An `appVersion` the application does not have is refused with the modelled 404
/// and ONE sentence — `App version <appVersion> not found for appArn <appArn>` —
/// from every operation that resolves an app version, for every form of version
/// token.
#[tokio::test]
async fn an_app_version_the_application_does_not_have_is_refused_with_one_sentence() {
    let router = emulator();
    let app_arn = create_an_app(&router, "version-refusals").await;
    add_a_component(
        &router,
        &app_arn,
        "storage",
        "storage-tier",
        "AWS::ResilienceHub::StorageAppComponent",
    )
    .await;

    // The versions this application does NOT have: a sequence number no publish
    // minted, a `versionName` (which labels no version), a 50-character version at
    // the top of `EntityVersion`'s pattern `^\S{1,50}$`, and the release alias
    // before anything is published.
    let versions = [
        ("a sequence number nothing minted", "99", true),
        ("a versionName, not a version", "v1", true),
        (
            "a 50-character version",
            "v999999999999999999999999999999999999999999999999",
            true,
        ),
        (
            "the release alias before any publish",
            "release",
            false,
        ),
    ];

    for (version_case, app_version, only_any_version) in versions {
        let expected = format!("App version {app_version} not found for appArn {app_arn}");
        for addressed in addressed_operations() {
            if only_any_version && !addressed.any_version {
                continue;
            }
            let case = format!("{} / {version_case}", addressed.operation);
            let mut request = json!({"appArn": app_arn, "appVersion": app_version});
            for (member, value) in addressed.extra.as_object().into_iter().flatten() {
                request[member] = value.clone();
            }
            let reply = post(&router, addressed.target, &request).await;
            assert_eq!(
                (
                    reply.status,
                    reply.error_code.as_str(),
                    reply.body["message"].as_str().unwrap_or_default(),
                    reply.body["resourceType"].as_str().unwrap_or_default()
                ),
                (
                    404,
                    "ResourceNotFoundException",
                    expected.as_str(),
                    "AppVersion"
                ),
                "case {case:?}: expected HTTP 404 ResourceNotFoundException \
                 {expected:?}, got {} {:?} {:?}",
                reply.status,
                reply.error_code,
                reply.body
            );
        }
    }

    // Not vacuous: the version the application DOES have is served by the same
    // calls, and the refusal for an application that does not exist names the
    // APPLICATION instead.
    read_template(&router, &app_arn, "draft")
        .await
        .ok("the draft the application has");
    let never_created =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/00000000-0000-0000-0000-000000000000");
    let reply = read_template(&router, &never_created, "draft").await;
    assert_eq!(
        (
            reply.status,
            reply.body["message"].as_str().unwrap_or_default()
        ),
        (
            404,
            format!("App not found for appArn {never_created}").as_str()
        ),
        "case \"an application that was never created\": the refusal names the \
         application, got {} {:?}",
        reply.status,
        reply.body
    );
}
