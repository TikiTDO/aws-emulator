// SPDX-License-Identifier: CC0-1.0

//! An application's CURRENT RELEASE and the template each app version holds, end to
//! end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rule under test: a published app version is LABELLED by the fixed
//! current-release alias and NUMBERED separately — `PublishAppVersion` answers that
//! alias as its `appVersion` and the publish sequence number as its `identifier`, the
//! alias always resolves to the newest snapshot, and the snapshot it supersedes stays
//! readable under its own number. `DescribeAppVersionTemplate` renders the addressed
//! version's stored contents as its `appTemplateBody`, so the release template keeps
//! reporting what the draft held at publish time however much the draft is written to
//! afterwards; a version the application does not have is the modelled 404.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::{
    DRAFT_APP_VERSION, RELEASE_APP_VERSION,
};
use service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-east-1";

/// The account id `engine::mock_account_id` mints every ARN under.
const ACCOUNT: &str = "000000000000";

/// The `version` every rendered `appTemplateBody` carries: the version of the
/// app-template DOCUMENT, which is the same for every app version of every
/// application — the body is the document the template operations read and write,
/// so no part of it depends on which label a read addressed the version by. A
/// DECIMAL number, as the document states it.
const APP_TEMPLATE_VERSION: f64 = 2.0;

/// The `id` and `name` of the AppCommon Application Component every app version
/// carries, and its `type` — the component the SERVICE maintains, which is part of
/// every template document rather than something a caller created.
const APP_COMMON_COMPONENT: (&str, &str) = ("appcommon", "AWS::ResilienceHub::AppCommonAppComponent");

/// That component as a template renders it: no resource is assigned to it unless a
/// caller names it, so it carries an empty `resourceNames`.
fn app_common_component() -> Value {
    json!({
        "id": APP_COMMON_COMPONENT.0,
        "name": APP_COMMON_COMPONENT.0,
        "type": APP_COMMON_COMPONENT.1,
        "resourceNames": [],
    })
}

/// An emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These requests carry an unverifiable placeholder signature: this suite tests
    // the operations, not SigV4.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype`; empty on success.
    error_code: String,
    body: Value,
}

impl Reply {
    /// The response body, asserting the call succeeded.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} {:?} {:?}",
            self.status, self.error_code, self.body
        );
        &self.body
    }

    /// Assert the call was refused with `status`, `code` and `resource_type`.
    fn refused(&self, case: &str, status: u16, code: &str, resource_type: &str) {
        assert_eq!(
            (
                self.status,
                self.error_code.as_str(),
                self.body["resourceType"].as_str().unwrap_or_default()
            ),
            (status, code, resource_type),
            "case {case:?}: expected HTTP {status} {code} for a missing \
             {resource_type}, got {} {:?} {:?}",
            self.status,
            self.error_code,
            self.body
        );
    }
}

/// POST one request through the router, exactly as the server front door does.
async fn post(router: &MultiServiceRouter, target: &str, body: &Value) -> Reply {
    let payload = serde_json::to_vec(body).expect("serializable body");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://resiliencehub.{REGION}.amazonaws.com{target}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/resiliencehub/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&raw)}))
    };
    Reply {
        status,
        error_code,
        body,
    }
}

/// An application to publish versions of, and its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// Add one Application Component to `app_arn`'s draft version.
async fn add_a_component(router: &MultiServiceRouter, app_arn: &str, name: &str, kind: &str) {
    let reply = post(
        router,
        "/create-app-version-app-component",
        &json!({"appArn": app_arn, "id": name, "name": name, "type": kind}),
    )
    .await;
    reply.ok(name);
}

/// Add one physical resource to `app_arn`'s draft version. `extra` carries whatever
/// else the row varies — the `logicalResourceId`'s optional source members, an
/// `additionalInfo` map — merged into the request.
async fn add_a_resource(
    router: &MultiServiceRouter,
    app_arn: &str,
    name: &str,
    component: &str,
    resource_type: &str,
    extra: &Value,
) {
    let mut request = json!({
        "appArn": app_arn,
        "appComponents": [component],
        "resourceName": name,
        "resourceType": resource_type,
        "physicalResourceId": name,
        "logicalResourceId": {"identifier": logical_identifier(name)},
    });
    for (member, value) in extra.as_object().into_iter().flatten() {
        if member == "logicalResourceId" {
            for (nested, value) in value.as_object().into_iter().flatten() {
                request["logicalResourceId"][nested] = value.clone();
            }
        } else {
            request[member] = value.clone();
        }
    }
    post(router, "/create-app-version-resource", &request)
        .await
        .ok(name);
}

/// The `logicalResourceId.identifier` the suite gives the resource named `name`:
/// the name without its separators, so the logical id and the resource name are
/// distinguishable in an assertion.
fn logical_identifier(name: &str) -> String {
    name.split('-')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Publish `app_arn`'s draft, under `version_name` when it names one, and answer the
/// whole response body.
async fn publish(router: &MultiServiceRouter, app_arn: &str, version_name: Option<&str>) -> Value {
    let mut request = json!({"appArn": app_arn});
    if let Some(version_name) = version_name {
        request["versionName"] = json!(version_name);
    }
    let reply = post(router, "/publish-app-version", &request).await;
    reply.ok("publish").clone()
}

/// `DescribeAppVersionTemplate`'s reply for one version.
async fn read_template(router: &MultiServiceRouter, app_arn: &str, app_version: &str) -> Reply {
    post(
        router,
        "/describe-app-version-template",
        &json!({"appArn": app_arn, "appVersion": app_version}),
    )
    .await
}

/// The `appTemplateBody` of one version, parsed — asserting the reply echoes the
/// `appArn` and `appVersion` the request addressed.
async fn template(router: &MultiServiceRouter, app_arn: &str, app_version: &str) -> Value {
    let reply = read_template(router, app_arn, app_version).await;
    let case = format!("the template of version {app_version}");
    let body = reply.ok(&case);
    assert_eq!(
        (body["appArn"].as_str(), body["appVersion"].as_str()),
        (Some(app_arn), Some(app_version)),
        "case {case:?}: the reply must echo the application and version it was \
         asked for, got {body:?}"
    );
    let rendered = body["appTemplateBody"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: appTemplateBody is @required, got {body:?}"));
    serde_json::from_str(rendered).unwrap_or_else(|error| {
        panic!("case {case:?}: appTemplateBody must be JSON ({error}), got {rendered:?}")
    })
}

/// The template's resources, keyed by their logical identifier.
fn resources_by_identifier(body: &Value) -> BTreeMap<String, Value> {
    body["resources"]
        .as_array()
        .unwrap_or_else(|| panic!("the template body carries a resources array, got {body:?}"))
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

/// The logical identifiers the template reports.
fn resource_identifiers(body: &Value) -> BTreeSet<String> {
    resources_by_identifier(body).into_keys().collect()
}

/// The template's Application Components, keyed by name, with each one's
/// `resourceNames` sorted: the assignment is a SET, so a case asserts membership
/// rather than the order the renderer walked its resources in.
fn components_by_name(body: &Value) -> BTreeMap<String, Value> {
    body["appComponents"]
        .as_array()
        .unwrap_or_else(|| panic!("the template body carries an appComponents array, got {body:?}"))
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
            let name = component["name"]
                .as_str()
                .unwrap_or_else(|| panic!("every component has a name, got {component:?}"))
                .to_string();
            (name, component)
        })
        .collect()
}

/// A row of the publish table: the resources the draft gains before this publish,
/// and the `versionName` (if any) the publish names.
///
/// The rows differ along what the rule generalises over — how many resources the
/// published draft holds (one, three, three again with nothing new, four) and the
/// form of the `versionName`.
struct Publish {
    label: &'static str,
    adds: &'static [&'static str],
    version_name: Option<&'static str>,
}

/// The table: one publish per row, in order, against ONE application.
fn publish_table() -> Vec<Publish> {
    vec![
        Publish {
            label: "first publish, one resource, no versionName",
            adds: &["first-bucket"],
            version_name: None,
        },
        Publish {
            label: "second publish, two more resources, a dotted versionName",
            adds: &["second-bucket", "third-bucket"],
            version_name: Some("v1.0.0"),
        },
        Publish {
            label: "third publish, nothing new in the draft, a named version",
            adds: &[],
            version_name: Some("release-candidate"),
        },
        Publish {
            label: "fourth publish, one more resource, a 50-character versionName",
            adds: &["fourth-bucket"],
            version_name: Some("v999999999999999999999999999999999999999999999999"),
        },
    ]
}

/// Each publish puts the frozen draft at the release alias and reports its own
/// sequence number; the snapshot it supersedes stays readable under that number,
/// holding exactly what the draft held when it was published.
#[tokio::test]
async fn the_release_alias_names_the_newest_snapshot_and_numbers_the_superseded_ones() {
    let router = emulator();
    let app_arn = create_an_app(&router, "release-alias").await;
    add_a_component(
        &router,
        &app_arn,
        "storage",
        "AWS::ResilienceHub::StorageAppComponent",
    )
    .await;

    // What the draft held at each publish, by that publish's sequence number, and
    // the template body the release alias answered the moment it was published.
    let mut frozen: Vec<(i64, BTreeSet<String>, Value)> = Vec::new();
    let mut in_draft: BTreeSet<String> = BTreeSet::new();

    for (index, row) in publish_table().into_iter().enumerate() {
        let case = row.label;
        for name in row.adds {
            add_a_resource(
                &router,
                &app_arn,
                name,
                "storage",
                "AWS::S3::Bucket",
                &json!({}),
            )
            .await;
            in_draft.insert(logical_identifier(name));
        }

        // The release template is FROZEN: the writes just made to the draft did not
        // reach the snapshot published before them.
        if let Some((_, published, _)) = frozen.last() {
            let still = resource_identifiers(&template(&router, &app_arn, RELEASE_APP_VERSION).await);
            assert_eq!(
                &still, published,
                "case {case:?}: writing to the draft must not change the release \
                 template, expected {published:?}, got {still:?}"
            );
        }

        let published = publish(&router, &app_arn, row.version_name).await;
        let sequence = index as i64 + 1;
        assert_eq!(
            (
                published["appVersion"].clone(),
                published["identifier"].clone()
            ),
            (json!(RELEASE_APP_VERSION), json!(sequence)),
            "case {case:?}: publish number {sequence} must answer the release alias \
             as its appVersion and {sequence} as its identifier, got {published:?}"
        );
        // The alias resolves to what was just published.
        let released = template(&router, &app_arn, RELEASE_APP_VERSION).await;
        frozen.push((sequence, in_draft.clone(), released.clone()));
        assert_eq!(
            resource_identifiers(&released),
            in_draft,
            "case {case:?}: the release template must hold exactly the draft's \
             resources at publish time, got {released:?}"
        );
        assert_eq!(
            released["version"],
            json!(APP_TEMPLATE_VERSION),
            "case {case:?}: the template reports the app-template document version \
             {APP_TEMPLATE_VERSION}, got {released:?}"
        );

        // Every SUPERSEDED snapshot is still readable under its own number, still
        // holding what it froze; the newest one's number addresses nothing, because
        // the alias is what names it.
        let newest = frozen.len() as i64;
        for (number, held, at_publish) in &frozen {
            let app_version = number.to_string();
            if *number == newest {
                read_template(&router, &app_arn, &app_version).await.refused(
                    &format!("case {case:?}: the current release by number"),
                    404,
                    "ResourceNotFoundException",
                    "AppVersion",
                );
                continue;
            }
            let superseded = template(&router, &app_arn, &app_version).await;
            assert_eq!(
                &resource_identifiers(&superseded),
                held,
                "case {case:?}: superseded version {app_version} must still hold \
                 {held:?}, got {superseded:?}"
            );
            // The SAME snapshot, read under its number instead of under the alias
            // it was published to: one published version has ONE body, so nothing
            // in it may depend on the label the read addressed it by.
            assert_eq!(
                &superseded, at_publish,
                "case {case:?}: version {app_version} must still answer the body it \
                 answered when it was published, expected {at_publish:?}, got \
                 {superseded:?}"
            );
        }

        // The draft keeps everything written to it, publish after publish.
        let draft = template(&router, &app_arn, DRAFT_APP_VERSION).await;
        assert_eq!(
            resource_identifiers(&draft),
            in_draft,
            "case {case:?}: the draft must still hold {in_draft:?}, got {draft:?}"
        );
    }
}

/// The template renders the version's stored resources and Application Components:
/// every form of `logicalResourceId` source member, an `additionalInfo` map, and the
/// resource-to-component assignment — and a publish carries all of it across
/// unchanged, so the release template is the draft's, document version included.
#[tokio::test]
async fn the_template_renders_every_stored_resource_and_component() {
    let router = emulator();
    let app_arn = create_an_app(&router, "template-rendering").await;
    for (name, kind) in [
        ("storage", "AWS::ResilienceHub::StorageAppComponent"),
        ("compute", "AWS::ResilienceHub::ComputeAppComponent"),
    ] {
        add_a_component(&router, &app_arn, name, kind).await;
    }

    // The rows differ along the form of the resource: a bare logical id, one in a
    // CloudFormation stack, one in a resource group, one carrying `additionalInfo`,
    // and one assigned to the other component.
    let rows: Vec<(&str, &str, &str, Value, Value)> = vec![
        (
            "plain-bucket",
            "storage",
            "AWS::S3::Bucket",
            json!({}),
            json!({
                "logicalResourceId": {
                    "identifier": "PlainBucket",
                    "logicalStackName": null,
                    "resourceGroupName": null,
                },
                "type": "AWS::S3::Bucket",
                "name": "plain-bucket",
            }),
        ),
        (
            "stacked-table",
            "storage",
            "AWS::DynamoDB::Table",
            json!({"logicalResourceId": {"logicalStackName": "my-stack"}}),
            json!({
                "logicalResourceId": {
                    "identifier": "StackedTable",
                    "logicalStackName": "my-stack",
                    "resourceGroupName": null,
                },
                "type": "AWS::DynamoDB::Table",
                "name": "stacked-table",
            }),
        ),
        (
            "grouped-queue",
            "storage",
            "AWS::SQS::Queue",
            json!({"logicalResourceId": {"resourceGroupName": "my-group"}}),
            json!({
                "logicalResourceId": {
                    "identifier": "GroupedQueue",
                    "logicalStackName": null,
                    "resourceGroupName": "my-group",
                },
                "type": "AWS::SQS::Queue",
                "name": "grouped-queue",
            }),
        ),
        (
            "annotated-function",
            "compute",
            "AWS::Lambda::Function",
            json!({"additionalInfo": {"some-key": ["one", "two"]}}),
            json!({
                "logicalResourceId": {
                    "identifier": "AnnotatedFunction",
                    "logicalStackName": null,
                    "resourceGroupName": null,
                },
                "type": "AWS::Lambda::Function",
                "name": "annotated-function",
                "additionalInfo": {"some-key": ["one", "two"]},
            }),
        ),
    ];

    let mut expected_resources: BTreeMap<String, Value> = BTreeMap::new();
    for (name, component, resource_type, extra, expected) in &rows {
        add_a_resource(&router, &app_arn, name, component, resource_type, extra).await;
        expected_resources.insert(logical_identifier(name), expected.clone());
    }

    let expected_components: BTreeMap<String, Value> = BTreeMap::from([
        (
            "storage".to_string(),
            json!({
                "id": "storage",
                "name": "storage",
                "type": "AWS::ResilienceHub::StorageAppComponent",
                "resourceNames": ["grouped-queue", "plain-bucket", "stacked-table"],
            }),
        ),
        (
            "compute".to_string(),
            json!({
                "id": "compute",
                "name": "compute",
                "type": "AWS::ResilienceHub::ComputeAppComponent",
                "resourceNames": ["annotated-function"],
            }),
        ),
        // The component the service maintains in every app version, whatever the
        // caller created in it.
        (APP_COMMON_COMPONENT.0.to_string(), app_common_component()),
    ]);

    let draft = template(&router, &app_arn, DRAFT_APP_VERSION).await;
    assert_eq!(
        resources_by_identifier(&draft),
        expected_resources,
        "case \"the draft template\": expected {expected_resources:?}, got {draft:?}"
    );
    assert_eq!(
        components_by_name(&draft),
        expected_components,
        "case \"the draft template\": expected {expected_components:?}, got {draft:?}"
    );

    // Reading changes nothing a later caller sees.
    let again = template(&router, &app_arn, DRAFT_APP_VERSION).await;
    assert_eq!(
        again, draft,
        "case \"a repeated read\": the same request must answer identically"
    );

    // The publish carries every member across: the release template is the draft's,
    // down to the rendered resource objects, under the release label.
    publish(&router, &app_arn, Some("v1")).await;
    let released = template(&router, &app_arn, RELEASE_APP_VERSION).await;
    assert_eq!(
        resources_by_identifier(&released),
        expected_resources,
        "case \"the release template\": expected {expected_resources:?}, got {released:?}"
    );
    assert_eq!(
        components_by_name(&released),
        expected_components,
        "case \"the release template\": expected {expected_components:?}, got {released:?}"
    );
    assert_eq!(
        released["version"],
        json!(APP_TEMPLATE_VERSION),
        "case \"the release template\": expected the app-template document version \
         {APP_TEMPLATE_VERSION}, got {released:?}"
    );
    assert_eq!(
        released["version"], draft["version"],
        "case \"the release template\": the document version is the same for every \
         app version, got {released:?} and {draft:?}"
    );
}

/// `ResolveAppVersionResources` for one version, answering its `resolutionId`.
async fn resolve(router: &MultiServiceRouter, app_arn: &str, app_version: &str) -> String {
    let reply = post(
        router,
        "/resolve-app-version-resources",
        &json!({"appArn": app_arn, "appVersion": app_version}),
    )
    .await;
    reply.ok("resolve")["resolutionId"]
        .as_str()
        .expect("a resolutionId")
        .to_string()
}

/// `DescribeAppVersionResourcesResolutionStatus` for one resolution of one version.
async fn read_resolution(
    router: &MultiServiceRouter,
    app_arn: &str,
    app_version: &str,
    resolution_id: &str,
) -> Reply {
    post(
        router,
        "/describe-app-version-resources-resolution-status",
        &json!({
            "appArn": app_arn,
            "appVersion": app_version,
            "resolutionId": resolution_id,
        }),
    )
    .await
}

/// A version the application does not have is the modelled 404, an application that
/// does not exist is the modelled 404 for the application, and a draft nobody has
/// written to has an EMPTY template rather than none.
#[tokio::test]
async fn an_unknown_version_is_refused_and_an_untouched_draft_is_empty() {
    let router = emulator();
    let app_arn = create_an_app(&router, "template-refusals").await;

    // Nothing has been written to this application at all: the draft exists, and
    // its template holds no resources and no component beyond the AppCommon one
    // every app version carries.
    let draft = template(&router, &app_arn, DRAFT_APP_VERSION).await;
    assert_eq!(
        (
            resource_identifiers(&draft),
            components_by_name(&draft)
                .into_iter()
                .collect::<Vec<(String, Value)>>()
        ),
        (
            BTreeSet::new(),
            vec![(APP_COMMON_COMPONENT.0.to_string(), app_common_component())]
        ),
        "case \"an untouched draft\": expected an empty template carrying only the \
         AppCommon component, got {draft:?}"
    );

    // The versions this application does NOT have: the release alias before any
    // publish, a sequence number no publish minted, a `versionName` (which labels no
    // version), and a 50-character version at the top of `EntityVersion`'s pattern
    // `^\S{1,50}$` (from `smithy_query query="operation"
    // operation="DescribeAppVersionTemplate"`).
    add_a_component(
        &router,
        &app_arn,
        "storage",
        "AWS::ResilienceHub::StorageAppComponent",
    )
    .await;
    for (case, app_version) in [
        ("the release alias before any publish", RELEASE_APP_VERSION),
        ("a sequence number nothing minted", "7"),
        ("a versionName, not a version", "v1"),
        (
            "a 50-character version",
            "v999999999999999999999999999999999999999999999999",
        ),
    ] {
        read_template(&router, &app_arn, app_version)
            .await
            .refused(case, 404, "ResourceNotFoundException", "AppVersion");
    }

    // Once published, the alias resolves and the number still does not.
    publish(&router, &app_arn, None).await;
    template(&router, &app_arn, RELEASE_APP_VERSION).await;
    read_template(&router, &app_arn, "1").await.refused(
        "the current release addressed by its number",
        404,
        "ResourceNotFoundException",
        "AppVersion",
    );

    // An application that was deleted, and one that never existed, are the 404 for
    // the APPLICATION — not for one of its versions.
    let deleted = create_an_app(&router, "template-deleted").await;
    post(&router, "/delete-app", &json!({"appArn": deleted}))
        .await
        .ok("delete the application");
    let never_created =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/00000000-0000-0000-0000-000000000000");
    for (case, app_arn) in [
        ("a deleted application", deleted.as_str()),
        ("an application that was never created", never_created.as_str()),
    ] {
        for app_version in [DRAFT_APP_VERSION, RELEASE_APP_VERSION] {
            read_template(&router, app_arn, app_version).await.refused(
                &format!("{case}, version {app_version}"),
                404,
                "ResourceNotFoundException",
                "App",
            );
        }
    }
}

/// Everything recorded ABOUT a version moves with it: a resolution of the release a
/// client resolved before re-publishing stays with the snapshot it resolved, under
/// that snapshot's own number, and the new release starts with none of its own.
#[tokio::test]
async fn a_resolution_follows_the_snapshot_it_resolved_across_a_republish() {
    let router = emulator();
    let app_arn = create_an_app(&router, "release-resolutions").await;
    add_a_component(
        &router,
        &app_arn,
        "storage",
        "AWS::ResilienceHub::StorageAppComponent",
    )
    .await;

    // Two publishes, the first one resolved while it is the current release.
    let mut resolutions: Vec<(i64, String)> = Vec::new();
    for (index, resource) in ["first-bucket", "second-bucket"].into_iter().enumerate() {
        add_a_resource(
            &router,
            &app_arn,
            resource,
            "storage",
            "AWS::S3::Bucket",
            &json!({}),
        )
        .await;
        publish(&router, &app_arn, None).await;
        let resolution = resolve(&router, &app_arn, RELEASE_APP_VERSION).await;
        resolutions.push((index as i64 + 1, resolution));
    }

    let newest = resolutions.len() as i64;
    for (sequence, resolution_id) in &resolutions {
        if *sequence == newest {
            // The current release keeps the resolution started against it.
            read_resolution(&router, &app_arn, RELEASE_APP_VERSION, resolution_id)
                .await
                .ok(&format!("the current release's own resolution {sequence}"));
            continue;
        }
        let app_version = sequence.to_string();
        read_resolution(&router, &app_arn, &app_version, resolution_id)
            .await
            .ok(&format!(
                "resolution {sequence}, now addressed under version {app_version}"
            ));
        read_resolution(&router, &app_arn, RELEASE_APP_VERSION, resolution_id)
            .await
            .refused(
                &format!("resolution {sequence} against the NEW release"),
                404,
                "ResourceNotFoundException",
                "Resolution",
            );
    }
}
