// SPDX-License-Identifier: CC0-1.0

//! The `appTemplateBody` document a template read renders, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rule under test: the document `DescribeAppVersionTemplate` renders is a
//! function of the addressed app version's STORED CONTENTS alone, so no part of it
//! is derived from the label the request addressed that version by (the fixed
//! release alias, a publish sequence number, the draft label) nor from the
//! application it belongs to. Two consequences this pins: one published snapshot
//! answers a BYTE-IDENTICAL document however it is addressed and however many
//! publishes have moved the alias off it, and two versions holding the same
//! contents answer the same document whatever history reached those contents.
//!
//! The document's own `version` member is part of that: it describes the template
//! DOCUMENT (the same document `PutDraftAppVersionTemplate` takes back in), so it
//! is a number that never varies with the read, and never the `EntityVersion`
//! string the request carried.

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

/// The members the rendered document carries for the versions these cases build:
/// the structure the model documents for `appTemplateBody` (`smithy_query
/// query="doc" shape="DescribeAppVersionTemplateResponse"
/// member="appTemplateBody"`), every one of which the document always carries —
/// `excludedResources` included, as an empty list when nothing is excluded.
const DOCUMENT_MEMBERS: [&str; 4] = [
    "appComponents",
    "excludedResources",
    "resources",
    "version",
];

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

/// One edit to an application's DRAFT version — what a row of the table below is
/// built out of.
#[derive(Debug, Clone, Copy)]
enum Edit {
    /// Add one Application Component, by name and `type`.
    Component {
        name: &'static str,
        kind: &'static str,
    },
    /// Map one physical resource into the draft, assigned to `component`. `extra`
    /// carries whatever else the resource varies — `additionalInfo`, the optional
    /// source members of its `logicalResourceId` — as a JSON object merged into the
    /// request.
    Resource {
        name: &'static str,
        component: &'static str,
        resource_type: &'static str,
        extra: &'static str,
    },
    /// Remove one resource from the draft again, by the name it was added under.
    DeleteResource { name: &'static str },
}

/// An application to publish versions of, and its ARN.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string()
}

/// The `logicalResourceId$identifier` the suite gives the resource named `name`,
/// so the logical id and the resource name are distinguishable in an assertion.
fn logical_identifier(name: &str) -> String {
    format!("Logical-{name}")
}

/// Apply one edit to `app_arn`'s draft version.
async fn apply(router: &MultiServiceRouter, app_arn: &str, edit: &Edit) {
    match edit {
        Edit::Component { name, kind } => {
            post(
                router,
                "/create-app-version-app-component",
                &json!({"appArn": app_arn, "id": name, "name": name, "type": kind}),
            )
            .await
            .ok(name);
        }
        Edit::Resource {
            name,
            component,
            resource_type,
            extra,
        } => {
            let mut request = json!({
                "appArn": app_arn,
                "appComponents": [component],
                "resourceName": name,
                "resourceType": resource_type,
                "physicalResourceId": name,
                "logicalResourceId": {"identifier": logical_identifier(name)},
            });
            let extra: Value = serde_json::from_str(extra).expect("a JSON object of extra members");
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
        Edit::DeleteResource { name } => {
            post(
                router,
                "/delete-app-version-resource",
                &json!({"appArn": app_arn, "resourceName": name}),
            )
            .await
            .ok(name);
        }
    }
}

/// Publish `app_arn`'s draft, answering the publish sequence number it reported.
async fn publish(router: &MultiServiceRouter, app_arn: &str, case: &str) -> i64 {
    let reply = post(router, "/publish-app-version", &json!({"appArn": app_arn})).await;
    let body = reply.ok(case).clone();
    body["identifier"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: a publish reports its identifier, got {body:?}"))
}

/// The RAW `appTemplateBody` string of one app version — unparsed, because what
/// this suite asserts is that two reads of one snapshot answer the same BYTES.
async fn document(router: &MultiServiceRouter, app_arn: &str, app_version: &str) -> String {
    let case = format!("the template of version {app_version}");
    let reply = post(
        router,
        "/describe-app-version-template",
        &json!({"appArn": app_arn, "appVersion": app_version}),
    )
    .await;
    let body = reply.ok(&case);
    assert_eq!(
        (body["appArn"].as_str(), body["appVersion"].as_str()),
        (Some(app_arn), Some(app_version)),
        "case {case:?}: the reply must echo the application and version it was \
         asked for, got {body:?}"
    );
    body["appTemplateBody"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: appTemplateBody is @required, got {body:?}"))
        .to_string()
}

/// One row of the table: the contents to build in a fresh application's draft, and
/// the row whose contents these end up equal to, when another row reaches the same
/// contents by a different history.
struct Contents {
    label: &'static str,
    /// The `name` of the application this row builds, within `EntityName`'s
    /// pattern `^[A-Za-z0-9][A-Za-z0-9_\-]{1,59}$`.
    app_name: &'static str,
    edits: &'static [Edit],
    /// The `label` of the row holding the same contents as this one.
    same_contents_as: Option<&'static str>,
}

/// The table. The rows differ along what the rule generalises over — how many
/// components and resources a version holds, the form of each resource
/// (`logicalResourceId` source members, an `additionalInfo` map, the component it
/// is assigned to), and the HISTORY that reached those contents (built up, or built
/// up and cut back down by a draft delete).
fn contents_table() -> Vec<Contents> {
    vec![
        Contents {
            label: "one component and no resources",
            app_name: "doc-bare-component",
            edits: &[Edit::Component {
                name: "storage",
                kind: "AWS::ResilienceHub::StorageAppComponent",
            }],
            same_contents_as: None,
        },
        Contents {
            label: "one component and two resources",
            app_name: "doc-two-resources",
            edits: &[
                Edit::Component {
                    name: "storage",
                    kind: "AWS::ResilienceHub::StorageAppComponent",
                },
                Edit::Resource {
                    name: "first-bucket",
                    component: "storage",
                    resource_type: "AWS::S3::Bucket",
                    extra: "{}",
                },
                Edit::Resource {
                    name: "second-bucket",
                    component: "storage",
                    resource_type: "AWS::S3::Bucket",
                    extra: "{}",
                },
            ],
            same_contents_as: None,
        },
        Contents {
            label: "two components, a stack source and an additionalInfo map",
            app_name: "doc-split-components",
            edits: &[
                Edit::Component {
                    name: "storage",
                    kind: "AWS::ResilienceHub::StorageAppComponent",
                },
                Edit::Component {
                    name: "compute",
                    kind: "AWS::ResilienceHub::ComputeAppComponent",
                },
                Edit::Resource {
                    name: "stacked-table",
                    component: "storage",
                    resource_type: "AWS::DynamoDB::Table",
                    extra: r#"{"logicalResourceId": {"logicalStackName": "orders-stack"}}"#,
                },
                Edit::Resource {
                    name: "annotated-function",
                    component: "compute",
                    resource_type: "AWS::Lambda::Function",
                    extra: r#"{"additionalInfo": {"failover-regions": ["us-west-2", "eu-west-1"]}}"#,
                },
            ],
            same_contents_as: None,
        },
        Contents {
            label: "two resources reached by adding a third and deleting it again",
            app_name: "doc-deleted-third",
            edits: &[
                Edit::Component {
                    name: "storage",
                    kind: "AWS::ResilienceHub::StorageAppComponent",
                },
                Edit::Resource {
                    name: "first-bucket",
                    component: "storage",
                    resource_type: "AWS::S3::Bucket",
                    extra: "{}",
                },
                Edit::Resource {
                    name: "third-bucket",
                    component: "storage",
                    resource_type: "AWS::S3::Bucket",
                    extra: "{}",
                },
                Edit::Resource {
                    name: "second-bucket",
                    component: "storage",
                    resource_type: "AWS::S3::Bucket",
                    extra: "{}",
                },
                Edit::DeleteResource {
                    name: "third-bucket",
                },
            ],
            same_contents_as: Some("one component and two resources"),
        },
        Contents {
            label: "a component whose only resource was deleted from the draft",
            app_name: "doc-emptied-component",
            edits: &[
                Edit::Component {
                    name: "storage",
                    kind: "AWS::ResilienceHub::StorageAppComponent",
                },
                Edit::Resource {
                    name: "only-bucket",
                    component: "storage",
                    resource_type: "AWS::S3::Bucket",
                    extra: "{}",
                },
                Edit::DeleteResource { name: "only-bucket" },
            ],
            same_contents_as: Some("one component and no resources"),
        },
    ]
}

/// Build one row's contents in a fresh application's draft, answering the ARN.
async fn build(router: &MultiServiceRouter, row: &Contents, app_name: &str) -> String {
    let app_arn = create_an_app(router, app_name).await;
    for edit in row.edits {
        apply(router, &app_arn, edit).await;
    }
    app_arn
}

/// The members one rendered document carries, and its `version`.
fn members_and_version(case: &str, rendered: &str) -> (BTreeSet<String>, Value) {
    let parsed: Value = serde_json::from_str(rendered).unwrap_or_else(|error| {
        panic!("case {case:?}: the document must be JSON ({error}), got {rendered:?}")
    });
    let object = parsed
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the document is a JSON object, got {rendered:?}"));
    (
        object.keys().cloned().collect(),
        object.get("version").cloned().unwrap_or(Value::Null),
    )
}

/// One snapshot has ONE document: every label that addresses it — the draft it was
/// frozen from, the release alias while it is current, its own publish sequence
/// number once the alias has moved off it (once, and again) — answers the identical
/// bytes, while a snapshot holding different contents answers different bytes.
#[tokio::test]
async fn one_snapshot_renders_one_document_whatever_label_addresses_it() {
    let router = emulator();

    // The `version` member every row's document carries, collected so the document
    // version can be checked to be one value across every version of every
    // application — a property of the DOCUMENT, not of the app version read.
    let mut document_versions: BTreeMap<&str, Value> = BTreeMap::new();

    for row in contents_table() {
        let case = row.label;
        let app_arn = build(&router, &row, row.app_name).await;

        // The draft's document, before anything is published.
        let drafted = document(&router, &app_arn, DRAFT_APP_VERSION).await;

        // The first publish freezes it: the alias names the snapshot, twice over,
        // because reading changes nothing a later caller sees.
        let first = publish(&router, &app_arn, case).await;
        let at_publish = document(&router, &app_arn, RELEASE_APP_VERSION).await;
        let read_again = document(&router, &app_arn, RELEASE_APP_VERSION).await;

        // A second publish of the UNCHANGED draft: the alias moves to the new
        // snapshot and the first one is addressed by its own sequence number.
        let second = publish(&router, &app_arn, case).await;
        let by_number = document(&router, &app_arn, &first.to_string()).await;
        let alias_moved_once = document(&router, &app_arn, RELEASE_APP_VERSION).await;

        // A draft edit and a third publish: the alias has now moved off both
        // snapshots, and neither one's document changed with it.
        apply(
            &router,
            &app_arn,
            &Edit::Resource {
                name: "late-addition",
                component: "storage",
                resource_type: "AWS::S3::Bucket",
                extra: "{}",
            },
        )
        .await;
        publish(&router, &app_arn, case).await;
        let by_number_again = document(&router, &app_arn, &first.to_string()).await;
        let second_by_number = document(&router, &app_arn, &second.to_string()).await;
        let newest = document(&router, &app_arn, RELEASE_APP_VERSION).await;

        for (addressed_by, rendered) in [
            ("the draft it was frozen from", drafted.as_str()),
            ("the release alias at publish time", at_publish.as_str()),
            ("a repeated read at the release alias", read_again.as_str()),
            ("its own publish number, once superseded", by_number.as_str()),
            (
                "the alias on an identical second snapshot",
                alias_moved_once.as_str(),
            ),
            (
                "its own publish number, twice superseded",
                by_number_again.as_str(),
            ),
            ("the second snapshot's own number", second_by_number.as_str()),
        ] {
            assert_eq!(
                rendered,
                drafted.as_str(),
                "case {case:?}: addressed by {addressed_by}, the document must be \
                 byte-identical to the one these contents render, expected \
                 {drafted:?}, got {rendered:?}"
            );
        }

        // Not vacuous: the snapshot holding DIFFERENT contents renders a different
        // document, so the equalities above are about the contents and not about
        // every read answering one fixed string.
        assert_ne!(
            newest, drafted,
            "case {case:?}: the snapshot published after a draft edit holds other \
             contents, so its document must differ, got {newest:?}"
        );

        // Nothing in the document is the request's label: its members are the
        // documented ones, and `version` is a number describing the document.
        let (members, version) = members_and_version(case, &drafted);
        let expected: BTreeSet<String> = DOCUMENT_MEMBERS.iter().map(|m| m.to_string()).collect();
        assert_eq!(
            members, expected,
            "case {case:?}: expected the document members {expected:?}, got {members:?} \
             in {drafted:?}"
        );
        assert!(
            version.is_number(),
            "case {case:?}: the document version describes the template document, so \
             it is a number and never the appVersion label of the read, got {version:?}"
        );
        document_versions.insert(case, version);
    }

    // One document version across every row: it is no part of the app version it
    // was read from.
    let distinct: BTreeSet<String> = document_versions
        .values()
        .map(|version| version.to_string())
        .collect();
    assert_eq!(
        distinct.len(),
        1,
        "case \"the document version\": every document carries the same version, got \
         {document_versions:?}"
    );
}

/// The document is a function of the CONTENTS: two applications holding the same
/// contents render the same document, two histories that reach the same contents
/// render the same document, and contents that differ render documents that differ.
#[tokio::test]
async fn the_document_follows_the_contents_and_not_the_application_or_its_history() {
    let router = emulator();

    // Each row's published document, keyed by the row's label.
    let mut rendered: BTreeMap<&str, String> = BTreeMap::new();

    for row in contents_table() {
        let case = row.label;

        // The SAME contents in two separate applications: the document carries no
        // part of the application it belongs to, so the two must agree.
        let mut both: Vec<String> = Vec::new();
        for copy in ["a", "b"] {
            let app_name = format!("{}-{copy}", row.app_name);
            let app_arn = build(&router, &row, &app_name).await;
            publish(&router, &app_arn, case).await;
            both.push(document(&router, &app_arn, RELEASE_APP_VERSION).await);
        }
        assert_eq!(
            both[0], both[1],
            "case {case:?}: two applications holding the same contents must render \
             the identical document, got {:?} and {:?}",
            both[0], both[1]
        );
        rendered.insert(case, both.remove(0));
    }

    // A row whose edits CUT BACK to another row's contents renders that row's
    // document: the history that reached the contents is no part of them.
    for row in contents_table() {
        let Some(twin) = row.same_contents_as else {
            continue;
        };
        let case = row.label;
        let ours = &rendered[case];
        let theirs = &rendered[twin];
        assert_eq!(
            ours, theirs,
            "case {case:?}: the same contents as {twin:?} must render the identical \
             document, expected {theirs:?}, got {ours:?}"
        );
    }

    // Rows whose contents genuinely differ render documents that differ, so the
    // equalities above cannot be satisfied by one fixed string.
    let distinct: BTreeSet<&String> = contents_table()
        .iter()
        .filter(|row| row.same_contents_as.is_none())
        .map(|row| &rendered[row.label])
        .collect();
    let expected = contents_table()
        .iter()
        .filter(|row| row.same_contents_as.is_none())
        .count();
    assert_eq!(
        distinct.len(),
        expected,
        "case \"rows with distinct contents\": expected {expected} distinct documents, \
         got {distinct:?}"
    );
}
