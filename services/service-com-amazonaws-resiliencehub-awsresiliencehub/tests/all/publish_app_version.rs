// SPDX-License-Identifier: CC0-1.0

//! Publishing an application version, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rule under test: `PublishAppVersion` freezes a COPY of the addressed
//! application's DRAFT version as that application's CURRENT RELEASE, reporting the
//! fixed release alias as the `appVersion` a later read addresses it by and the
//! publish sequence number — separately — as the `identifier` long, echoing the
//! `versionName` the request named; the snapshot a publish supersedes stays readable
//! under its own sequence number; the draft stays as it was, so a later write to it
//! cannot change what was published; and a draft whose app template maps no
//! resource is refused with the modelled 409 `ConflictException` while an
//! application that does not exist is refused with the modelled 404.

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

/// The refusal a publish of a draft whose template maps no resource earns: FIXED
/// text, naming the template requirement the request fails to meet and carrying no
/// part of the request.
const EMPTY_TEMPLATE_MESSAGE: &str =
    "Invalid app template. App template must have at least 1 resource";

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

    /// Assert the call was refused with `status` and `code`.
    fn refused(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error_code.as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {} {:?} {:?}",
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

/// Add one Application Component with `id` to `app_arn`'s draft version.
async fn add_a_component(router: &MultiServiceRouter, app_arn: &str, id: &str) {
    let reply = post(
        router,
        "/create-app-version-app-component",
        &json!({
            "appArn": app_arn,
            "id": id,
            "name": id,
            "type": "AWS::ResilienceHub::ComputeAppComponent",
        }),
    )
    .await;
    reply.ok(id);
}

/// Map one physical resource named `label` into `app_arn`'s draft version, so the
/// draft's app template maps the resource a publish requires.
///
/// The resource is bound to the AppCommon Application Component every app version
/// already carries, so mapping it adds no Application Component of its own and the
/// component set each publish freezes stays the one the table built.
async fn map_a_resource(router: &MultiServiceRouter, app_arn: &str, label: &str) {
    let reply = post(
        router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "logicalResourceId": {"identifier": format!("{label}-resource")},
            "physicalResourceId": format!("{label}-queue"),
            "resourceType": "AWS::SQS::Queue",
            "appComponents": ["appcommon"],
        }),
    )
    .await;
    reply.ok(label);
}

/// Whether `app_version` of `app_arn` holds the component `id`, as the implemented
/// reader answers: `Some(true)` when it does, `Some(false)` when the version exists
/// without it, `None` when the version itself does not exist.
async fn version_holds_component(
    router: &MultiServiceRouter,
    app_arn: &str,
    app_version: &str,
    id: &str,
) -> Option<bool> {
    let reply = post(
        router,
        "/describe-app-version-app-component",
        &json!({"appArn": app_arn, "appVersion": app_version, "id": id}),
    )
    .await;
    if reply.status == 200 {
        return Some(true);
    }
    // Both misses are the modelled 404; the `resourceType` is what says which link
    // of the `appArn`/`appVersion`/`id` path did not resolve.
    match reply.body["resourceType"].as_str() {
        Some("AppVersion") => None,
        Some("AppComponent") => Some(false),
        other => panic!(
            "unexpected reply for {app_version}/{id}: {other:?} {} {:?}",
            reply.status, reply.body
        ),
    }
}

/// A `versionName` at the top of `EntityVersion`'s modelled pattern
/// `^\S{1,50}$` (from `smithy_query query="operation"
/// operation="PublishAppVersion"`): 50 non-space characters.
fn max_length_version_name() -> String {
    format!("v{}", "9".repeat(49))
}

/// A row of the publish table: how much the draft gains before this publish, and
/// what `versionName` (if any) the publish names.
///
/// The rows differ along what the rule generalises over — how many components the
/// published draft holds, and the form of the `versionName`: absent, a short name,
/// one carrying punctuation, and one at the top of `EntityVersion`'s length.
struct Publish {
    label: &'static str,
    /// The component ids added to the draft before this publish.
    adds: &'static [&'static str],
    /// The `versionName` the publish request carries, if any.
    version_name: Option<String>,
}

/// The table: one row per publish, in order, against ONE application.
fn publish_table() -> Vec<Publish> {
    vec![
        Publish {
            label: "first publish, one component, no versionName",
            adds: &["compute-tier"],
            version_name: None,
        },
        Publish {
            label: "second publish, two more components, a dotted versionName",
            adds: &["database-tier", "storage-tier"],
            version_name: Some("v1.0.0".to_string()),
        },
        Publish {
            label: "third publish, nothing new in the draft, a named version",
            adds: &[],
            version_name: Some("release-candidate".to_string()),
        },
        Publish {
            label: "fourth publish, three more components, a 50-character versionName",
            adds: &["queue-tier", "networking-tier", "custom.tier/1"],
            version_name: Some(max_length_version_name()),
        },
    ]
}

/// The whole table against one application: each publish reports the release alias
/// as its `appVersion` and the next sequence number as its `identifier`, echoes the
/// `versionName`, and freezes exactly the components the draft held at that point —
/// the newest snapshot addressed by the alias, every superseded one by its own
/// number.
#[tokio::test]
async fn each_publish_freezes_the_draft_as_the_current_release() {
    let router = emulator();
    let app_arn = create_an_app(&router, "publish-sequence").await;
    // The draft's template maps a resource throughout, which is what makes it
    // publishable at all; the table below varies its COMPONENTS.
    map_a_resource(&router, &app_arn, "published").await;

    // What the draft held at each published version, in publish order, with the
    // sequence number that publish was minted with.
    let mut frozen: Vec<(i64, Vec<&'static str>)> = Vec::new();
    let mut draft_components: Vec<&'static str> = Vec::new();

    for (index, row) in publish_table().into_iter().enumerate() {
        let case = row.label;
        for id in row.adds {
            add_a_component(&router, &app_arn, id).await;
            draft_components.push(id);
        }

        let mut request = json!({"appArn": app_arn});
        if let Some(version_name) = row.version_name.as_deref() {
            request["versionName"] = json!(version_name);
        }
        let published = post(&router, "/publish-app-version", &request).await;
        let published = published.ok(case).clone();

        let expected_identifier = index as i64 + 1;
        let mut expected = json!({
            "appArn": app_arn,
            "appVersion": RELEASE_APP_VERSION,
            "identifier": expected_identifier,
        });
        if let Some(version_name) = row.version_name.as_deref() {
            expected["versionName"] = json!(version_name);
        }
        assert_eq!(
            published, expected,
            "case {case:?}: publish number {expected_identifier} must report the \
             release alias with that sequence number and echo its versionName, got \
             {published:?}"
        );

        frozen.push((expected_identifier, draft_components.clone()));

        // The draft is untouched by publishing: everything added to it so far is
        // still readable there.
        for id in &draft_components {
            assert_eq!(
                version_holds_component(&router, &app_arn, DRAFT_APP_VERSION, id).await,
                Some(true),
                "case {case:?}: the draft must still hold {id:?} after publishing"
            );
        }
    }

    // Every published version is frozen: it holds exactly what the draft held when
    // it was published — nothing added later, and nothing dropped. The newest is
    // addressed by the alias, each superseded one by its own sequence number.
    let newest = frozen.len() as i64;
    let every_component: Vec<&'static str> = frozen
        .last()
        .map(|(_, components)| components.clone())
        .unwrap_or_default();
    for (identifier, components) in &frozen {
        let app_version = if *identifier == newest {
            RELEASE_APP_VERSION.to_string()
        } else {
            identifier.to_string()
        };
        for id in &every_component {
            let case = format!("version {app_version} holds {id:?}");
            let expected = Some(components.contains(id));
            assert_eq!(
                version_holds_component(&router, &app_arn, &app_version, id).await,
                expected,
                "case {case:?}: expected {expected:?}"
            );
        }
    }

    // The newest snapshot's own number addresses nothing: while it is the current
    // release, the alias is what names it. Nor does a number past the last publish.
    for app_version in [newest.to_string(), (newest + 1).to_string()] {
        assert_eq!(
            version_holds_component(&router, &app_arn, &app_version, "compute-tier").await,
            None,
            "case {app_version:?}: only a SUPERSEDED snapshot is addressed by its number"
        );
    }
}

/// Sequence numbers are per application: two applications publish their own 1, 2,
/// … independently of each other, and each addresses its own current release by the
/// same alias.
#[tokio::test]
async fn publish_sequence_numbers_are_per_application() {
    let router = emulator();
    let first = create_an_app(&router, "publish-sequence-a").await;
    let second = create_an_app(&router, "publish-sequence-b").await;
    add_a_component(&router, &first, "first-tier").await;
    add_a_component(&router, &second, "second-tier").await;
    map_a_resource(&router, &first, "first").await;
    map_a_resource(&router, &second, "second").await;

    // The first application publishes twice, the second once in between.
    for (case, app_arn, expected) in [
        ("first app, publish 1", &first, 1_i64),
        ("second app, publish 1", &second, 1),
        ("first app, publish 2", &first, 2),
        ("second app, publish 2", &second, 2),
        ("first app, publish 3", &first, 3),
    ] {
        let published = post(&router, "/publish-app-version", &json!({"appArn": app_arn})).await;
        let published = published.ok(case).clone();
        assert_eq!(
            published["identifier"],
            json!(expected),
            "case {case:?}: expected identifier {expected}, got {published:?}"
        );
        assert_eq!(
            published["appVersion"],
            json!(RELEASE_APP_VERSION),
            "case {case:?}: appVersion must be the release alias, got {published:?}"
        );
    }

    // Each application's versions hold only its own component: the first published
    // three times (1, 2 superseded, 3 the release), the second twice (1 superseded,
    // 2 the release).
    for (case, app_arn, own, other, superseded) in [
        (
            "first app's versions",
            &first,
            "first-tier",
            "second-tier",
            &["1", "2"][..],
        ),
        (
            "second app's versions",
            &second,
            "second-tier",
            "first-tier",
            &["1"][..],
        ),
    ] {
        for app_version in superseded
            .iter()
            .copied()
            .chain(std::iter::once(RELEASE_APP_VERSION))
        {
            assert_eq!(
                version_holds_component(&router, app_arn, app_version, own).await,
                Some(true),
                "case {case:?}: version {app_version} must hold {own:?}"
            );
            assert_eq!(
                version_holds_component(&router, app_arn, app_version, other).await,
                Some(false),
                "case {case:?}: version {app_version} must not hold {other:?}"
            );
        }
    }
}

/// A draft whose app template maps no resource is refused with the modelled 409,
/// and nothing is minted — whatever the request's `versionName`, and whichever
/// OTHER application already has published versions.
#[tokio::test]
async fn publishing_an_empty_draft_is_refused_with_a_conflict() {
    let router = emulator();

    // An unrelated application WITH a published version, so the refusal below is
    // about the addressed application's own draft.
    let published_elsewhere = create_an_app(&router, "empty-draft-neighbour").await;
    add_a_component(&router, &published_elsewhere, "neighbour-tier").await;
    map_a_resource(&router, &published_elsewhere, "neighbour").await;
    post(
        &router,
        "/publish-app-version",
        &json!({"appArn": published_elsewhere}),
    )
    .await
    .ok("the neighbour publishes");

    // The rows differ along the one thing a publish request can vary: its
    // `versionName`.
    let version_names: Vec<(&str, Option<String>)> = vec![
        ("no versionName", None),
        ("a short versionName", Some("v1".to_string())),
        (
            "a 50-character versionName",
            Some(max_length_version_name()),
        ),
    ];

    for (index, (case, version_name)) in version_names.into_iter().enumerate() {
        let app_arn = create_an_app(&router, &format!("empty-draft-{index}")).await;
        let mut request = json!({"appArn": app_arn});
        if let Some(version_name) = version_name.as_deref() {
            request["versionName"] = json!(version_name);
        }

        let refused = post(&router, "/publish-app-version", &request).await;
        refused.refused(case, 409, "ConflictException");
        // The refusal names the TEMPLATE requirement, with no part of the request
        // in it: the same sentence whatever the application and whatever the
        // request's `versionName`.
        assert_eq!(
            refused.body["message"].as_str(),
            Some(EMPTY_TEMPLATE_MESSAGE),
            "case {case:?}: expected the empty-template refusal {EMPTY_TEMPLATE_MESSAGE:?}, \
             got {:?}",
            refused.body
        );

        // Nothing was minted: the application has no version 1 at all.
        assert_eq!(
            version_holds_component(&router, &app_arn, "1", "anything").await,
            None,
            "case {case:?}: a refused publish must mint no version"
        );

        // And once the draft's template maps a resource, the same request succeeds
        // as version 1 — the refusal was about the template, not about the
        // application. An Application Component on its own does not lift it: the
        // component only GROUPS resources, so the publish is still refused until a
        // resource is mapped.
        add_a_component(&router, &app_arn, "compute-tier").await;
        let still_refused = post(&router, "/publish-app-version", &request).await;
        still_refused.refused(case, 409, "ConflictException");
        assert_eq!(
            still_refused.body["message"].as_str(),
            Some(EMPTY_TEMPLATE_MESSAGE),
            "case {case:?}: a draft holding only Application Components maps no resource, \
             so it earns the same refusal, got {:?}",
            still_refused.body
        );
        map_a_resource(&router, &app_arn, "compute").await;
        let published = post(&router, "/publish-app-version", &request).await;
        let published = published.ok(case).clone();
        assert_eq!(
            (
                published["appVersion"].clone(),
                published["identifier"].clone()
            ),
            (json!(RELEASE_APP_VERSION), json!(1)),
            "case {case:?}: the first publish of this application is the release \
             alias with sequence number 1, got {published:?}"
        );
    }
}

/// An application that does not exist is refused with the modelled 404, whether it
/// was never created or was deleted.
#[tokio::test]
async fn publishing_an_unknown_application_is_refused() {
    let router = emulator();

    let deleted = create_an_app(&router, "publish-then-delete").await;
    add_a_component(&router, &deleted, "compute-tier").await;
    post(&router, "/delete-app", &json!({"appArn": deleted}))
        .await
        .ok("delete the application");

    let never_created =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/00000000-0000-0000-0000-000000000000");

    for (case, app_arn) in [
        ("a deleted application", deleted.as_str()),
        ("an application that was never created", never_created.as_str()),
    ] {
        let refused = post(
            &router,
            "/publish-app-version",
            &json!({"appArn": app_arn, "versionName": "v1"}),
        )
        .await;
        refused.refused(case, 404, "ResourceNotFoundException");
    }
}
