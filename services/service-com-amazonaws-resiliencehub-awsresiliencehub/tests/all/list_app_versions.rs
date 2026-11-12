// SPDX-License-Identifier: CC0-1.0

//! Listing an application's versions, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request — and asserts the response BODY, not just the status.
//!
//! The rule under test: `ListAppVersions` reports every app version the addressed
//! application holds — its DRAFT version, which exists from the moment the
//! application is created and carries the application's own `creationTime`, and
//! every version `PublishAppVersion` froze from it, the newest under the release
//! alias and every superseded one under its own publish sequence number, each with
//! that number as its `identifier` and the `versionName` the publish named —
//! oldest first, narrowed to the `startTime`/`endTime` bounds on a
//! version's creation time, in pages of at most `maxResults` that a `nextToken`
//! resumes exactly where the previous page stopped; an `appArn` naming no stored
//! application is the modelled 404.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::{
    DRAFT_APP_VERSION, DRAFT_APP_VERSION_IDENTIFIER, RELEASE_APP_VERSION,
};
use service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-east-1";

/// The account id `engine::mock_account_id` mints every ARN under.
const ACCOUNT: &str = "000000000000";

/// How long the time-range case waits between the calls that record a version, so
/// two versions' creation instants are far enough apart to be told apart at the
/// resolution the wire's epoch-seconds double carries. A TEST pacing constant, not
/// emulator behaviour.
const CLOCK_GAP: Duration = Duration::from_millis(20);

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

/// An application to list the versions of: its ARN and its `creationTime`, as
/// `CreateApp` reported them.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> (String, Value) {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    let app = reply.ok(name)["app"].clone();
    (
        app["appArn"].as_str().expect("a minted appArn").to_string(),
        app["creationTime"].clone(),
    )
}

/// Add one Application Component with `id` to `app_arn`'s draft version, and map
/// one physical resource into the draft beside it, so the draft's app template
/// maps the resource a publish requires.
///
/// The resource is bound to the AppCommon Application Component every app version
/// already carries, so filling the draft adds no component other than `id`.
async fn add_a_component_and_a_resource(router: &MultiServiceRouter, app_arn: &str, id: &str) {
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
    let reply = post(
        router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "logicalResourceId": {"identifier": format!("{id}-resource")},
            "physicalResourceId": format!("{id}-queue"),
            "resourceType": "AWS::SQS::Queue",
            "appComponents": ["appcommon"],
        }),
    )
    .await;
    reply.ok(id);
}

/// Publish `app_arn`'s draft, optionally under `version_name`, and answer the
/// `appVersion` the publish minted.
async fn publish(
    router: &MultiServiceRouter,
    app_arn: &str,
    version_name: Option<&str>,
) -> String {
    let mut request = json!({"appArn": app_arn});
    if let Some(version_name) = version_name {
        request["versionName"] = json!(version_name);
    }
    let reply = post(router, "/publish-app-version", &request).await;
    reply.ok("publish")["appVersion"]
        .as_str()
        .expect("a minted appVersion")
        .to_string()
}

/// `ListAppVersions` for `app_arn`, with whatever extra members `request` carries.
async fn list_versions(
    router: &MultiServiceRouter,
    app_arn: &str,
    request: &Value,
) -> Reply {
    let mut body = request.clone();
    body["appArn"] = json!(app_arn);
    post(router, "/list-app-versions", &body).await
}

/// The `appVersions` of a successful listing.
fn summaries(reply: &Reply, case: &str) -> Vec<Value> {
    reply.ok(case)["appVersions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: appVersions is @required, got {:?}",
                reply.body
            )
        })
        .clone()
}

/// The `appVersion` of each summary, in the order they were returned.
fn version_order(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| {
            summary["appVersion"]
                .as_str()
                .expect("appVersion is @required")
                .to_string()
        })
        .collect()
}

/// A `versionName` at the top of `EntityVersion`'s modelled pattern `^\S{1,50}$`
/// (from `smithy_query query="operation" operation="PublishAppVersion"`): 50
/// non-space characters.
fn max_length_version_name() -> String {
    format!("v{}", "9".repeat(49))
}

/// A row of the publish table: what the draft gains before this publish, and what
/// `versionName` (if any) the publish names.
///
/// The rows differ along what the rule generalises over — how many components the
/// published draft holds, and the form of the `versionName`: absent, a short name,
/// one carrying punctuation, and one at the top of `EntityVersion`'s length.
struct Publish {
    label: &'static str,
    adds: &'static [&'static str],
    version_name: Option<String>,
}

/// The table: one publish per row, in order, against ONE application.
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
            label: "fourth publish, a 50-character versionName",
            adds: &["queue-tier"],
            version_name: Some(max_length_version_name()),
        },
    ]
}

/// A freshly created application already has exactly one version — the draft — and
/// each publish adds exactly one more: the newest under the release alias, the one
/// it supersedes under its own sequence number, each with its `identifier` and its
/// `versionName`.
#[tokio::test]
async fn lists_the_draft_and_then_every_published_version() {
    let router = emulator();
    let (app_arn, app_created_at) = create_an_app(&router, "list-versions-sequence").await;

    // Before anything is published: the draft alone, carrying the application's own
    // creation time, the draft identifier and no version name.
    let listed = list_versions(&router, &app_arn, &json!({})).await;
    let page = summaries(&listed, "a freshly created application");
    assert_eq!(
        page,
        vec![json!({
            "appVersion": DRAFT_APP_VERSION,
            "identifier": DRAFT_APP_VERSION_IDENTIFIER,
            "creationTime": app_created_at,
        })],
        "case \"a freshly created application\": expected only the {DRAFT_APP_VERSION} \
         version with identifier {DRAFT_APP_VERSION_IDENTIFIER} and the application's \
         creationTime {app_created_at:?}, got {page:?}"
    );
    assert_eq!(
        listed.body["nextToken"],
        Value::Null,
        "case \"a freshly created application\": one version fits in a page, so no \
         nextToken, got {:?}",
        listed.body["nextToken"]
    );

    let mut expected_names: Vec<Option<String>> = Vec::new();
    for (index, row) in publish_table().into_iter().enumerate() {
        let case = row.label;
        for id in row.adds {
            add_a_component_and_a_resource(&router, &app_arn, id).await;
        }
        let minted = publish(&router, &app_arn, row.version_name.as_deref()).await;
        let sequence = index as i64 + 1;
        assert_eq!(
            minted, RELEASE_APP_VERSION,
            "case {case:?}: publish number {sequence} is addressed by the release \
             alias, got {minted:?}"
        );
        expected_names.push(row.version_name.clone());

        let listed = list_versions(&router, &app_arn, &json!({})).await;
        let page = summaries(&listed, case);

        // The draft, then every superseded version in publish order, then the
        // current release — the newest publish, which the alias names.
        let mut expected_order = vec![DRAFT_APP_VERSION.to_string()];
        expected_order.extend((1..sequence).map(|number| number.to_string()));
        expected_order.push(RELEASE_APP_VERSION.to_string());
        assert_eq!(
            version_order(&page),
            expected_order,
            "case {case:?}: expected the draft, then versions 1..{} by number, then \
             the release alias, got {page:?}",
            sequence - 1
        );

        // The draft is still the application's own creation time, untouched by
        // publishing.
        assert_eq!(
            page[0]["creationTime"], app_created_at,
            "case {case:?}: the draft keeps the application's creationTime \
             {app_created_at:?}, got {:?}",
            page[0]["creationTime"]
        );
        assert_eq!(
            page[0]["identifier"],
            json!(DRAFT_APP_VERSION_IDENTIFIER),
            "case {case:?}: the draft keeps identifier {DRAFT_APP_VERSION_IDENTIFIER}, \
             below every published version's, got {:?}",
            page[0]["identifier"]
        );

        // Every published version reports its sequence number as its identifier and
        // the versionName its publish named, and nothing else drifted.
        let mut previous = app_created_at.as_f64().expect("an epoch-seconds number");
        for (position, expected_name) in expected_names.iter().enumerate() {
            let summary = &page[position + 1];
            let number = position as i64 + 1;
            assert_eq!(
                summary["identifier"],
                json!(number),
                "case {case:?}: version {number} must report identifier {number}, \
                 got {summary:?}"
            );
            assert_eq!(
                summary.get("versionName").and_then(Value::as_str),
                expected_name.as_deref(),
                "case {case:?}: version {number} must report versionName \
                 {expected_name:?}, got {summary:?}"
            );
            let created_at = summary["creationTime"]
                .as_f64()
                .unwrap_or_else(|| panic!(
                    "case {case:?}: version {number} must report an epoch-seconds \
                     creationTime, got {summary:?}"
                ));
            assert!(
                created_at >= previous,
                "case {case:?}: version {number} was published after everything \
                 before it, so its creationTime {created_at} cannot precede \
                 {previous}"
            );
            previous = created_at;
        }
    }

    // A read changes nothing a later caller sees: the same request answers the same
    // listing.
    let again = list_versions(&router, &app_arn, &json!({})).await;
    let repeated = list_versions(&router, &app_arn, &json!({})).await;
    assert_eq!(
        summaries(&again, "repeated listing"),
        summaries(&repeated, "repeated listing"),
        "case \"repeated listing\": listing twice must answer identically"
    );
}

/// `maxResults` is a real page bound and `nextToken` resumes exactly where the
/// previous page stopped, for every page size — including both published bounds.
#[tokio::test]
async fn pages_through_the_versions_with_every_page_size() {
    let router = emulator();
    let (app_arn, _) = create_an_app(&router, "list-versions-pages").await;
    add_a_component_and_a_resource(&router, &app_arn, "compute-tier").await;
    const PUBLISHES: usize = 6;
    for index in 0..PUBLISHES {
        publish(&router, &app_arn, Some(&format!("v{index}"))).await;
    }
    // The draft plus every published version.
    let total = PUBLISHES + 1;

    let unpaged = list_versions(&router, &app_arn, &json!({})).await;
    let forward = version_order(&summaries(&unpaged, "unpaged"));
    assert_eq!(
        forward.len(),
        total,
        "case \"unpaged\": expected all {total} versions, got {forward:?}"
    );

    // Both published bounds of `maxResults` (@range 1..100), plus the sizes in
    // between that actually split this set.
    for page_size in [1usize, 2, 3, 6, 7, 100] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(next) = token.as_deref() {
                request["nextToken"] = json!(next);
            }
            let reply = list_versions(&router, &app_arn, &request).await;
            let page = summaries(&reply, &case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page returned {} summaries, more than maxResults",
                page.len()
            );
            walked.extend(version_order(&page));
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match reply.body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, forward,
            "case {case:?}: walking the pages must yield the unpaged order exactly"
        );
    }

    // One past each published bound of `maxResults` is refused by the modeled range.
    for out_of_range in [0, 101] {
        let case = format!("maxResults={out_of_range}");
        list_versions(&router, &app_arn, &json!({"maxResults": out_of_range}))
            .await
            .refused(&case, 400, "ValidationException");
    }

    // A token this operation never issued is refused, not silently ignored.
    for token in ["not-a-token", "6e6f7065"] {
        let case = format!("nextToken={token}");
        list_versions(&router, &app_arn, &json!({"nextToken": token}))
            .await
            .refused(&case, 400, "ValidationException");
    }
}

/// `startTime` and `endTime` bound a version's creation time, both ends inclusive,
/// and an absent bound admits everything on its side.
#[tokio::test]
async fn filters_the_versions_by_their_creation_time() {
    let router = emulator();
    let (app_arn, _) = create_an_app(&router, "list-versions-range").await;
    add_a_component_and_a_resource(&router, &app_arn, "compute-tier").await;
    // Spaced out, so each version's creation instant is separable on the wire.
    for index in 0..3 {
        std::thread::sleep(CLOCK_GAP);
        publish(&router, &app_arn, Some(&format!("v{index}"))).await;
    }

    let unpaged = list_versions(&router, &app_arn, &json!({})).await;
    let page = summaries(&unpaged, "unpaged");
    let order = version_order(&page);
    let instants: Vec<f64> = page
        .iter()
        .map(|summary| {
            summary["creationTime"]
                .as_f64()
                .expect("an epoch-seconds creationTime")
        })
        .collect();
    assert_eq!(
        order.len(),
        4,
        "case \"unpaged\": expected the draft and three published versions, got {order:?}"
    );

    /// An instant strictly between the versions at `before` and `after`.
    fn between(instants: &[f64], before: usize, after: usize) -> f64 {
        assert!(
            instants[after] > instants[before],
            "the version at {after} must be recorded after the one at {before}: \
             {instants:?}"
        );
        (instants[before] + instants[after]) / 2.0
    }

    let first = instants[0];
    let last = instants[instants.len() - 1];

    // The rows differ along which bounds the request carries and where they fall:
    // neither bound, each bound alone, both, a window around the middle, a window
    // past everything, and an inverted window.
    let cases: Vec<(String, Value, Vec<String>)> = vec![
        ("neither bound".to_string(), json!({}), order.clone()),
        (
            "startTime at the epoch".to_string(),
            json!({"startTime": 0.0}),
            order.clone(),
        ),
        (
            "endTime an hour ahead of the last version".to_string(),
            json!({"endTime": last + 3600.0}),
            order.clone(),
        ),
        (
            "startTime between the draft and version 1".to_string(),
            json!({"startTime": between(&instants, 0, 1)}),
            order[1..].to_vec(),
        ),
        (
            "endTime between the draft and version 1".to_string(),
            json!({"endTime": between(&instants, 0, 1)}),
            order[..1].to_vec(),
        ),
        (
            "a window holding versions 1 and 2".to_string(),
            json!({
                "startTime": between(&instants, 0, 1),
                "endTime": between(&instants, 2, 3),
            }),
            order[1..3].to_vec(),
        ),
        (
            "a window ending before every version".to_string(),
            json!({"endTime": first - 3600.0}),
            Vec::new(),
        ),
        (
            "a window starting after every version".to_string(),
            json!({"startTime": last + 1.0}),
            Vec::new(),
        ),
        (
            "an inverted window".to_string(),
            json!({"startTime": last, "endTime": first - 1.0}),
            Vec::new(),
        ),
    ];

    for (case, request, expected) in cases {
        let reply = list_versions(&router, &app_arn, &request).await;
        let got = version_order(&summaries(&reply, &case));
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }

    // The bounds compose with paging: a one-at-a-time walk of a window yields that
    // window's versions and nothing else.
    let window = json!({
        "startTime": between(&instants, 0, 1),
        "maxResults": 1,
    });
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = window.clone();
        if let Some(next) = token.as_deref() {
            request["nextToken"] = json!(next);
        }
        let reply = list_versions(&router, &app_arn, &request).await;
        walked.extend(version_order(&summaries(&reply, "a paged window")));
        assert!(
            walked.len() <= order.len(),
            "case \"a paged window\": pagination did not terminate, walked {walked:?}"
        );
        match reply.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        walked,
        order[1..].to_vec(),
        "case \"a paged window\": expected {:?}, got {walked:?}",
        &order[1..]
    );
}

/// Versions are per application: one application's listing never shows another's,
/// and an `appArn` naming no stored application is the modelled 404.
#[tokio::test]
async fn lists_only_the_addressed_application_and_refuses_an_unknown_one() {
    let router = emulator();
    let (first, _) = create_an_app(&router, "list-versions-app-a").await;
    let (second, _) = create_an_app(&router, "list-versions-app-b").await;
    add_a_component_and_a_resource(&router, &first, "first-tier").await;
    add_a_component_and_a_resource(&router, &second, "second-tier").await;
    for _ in 0..3 {
        publish(&router, &first, None).await;
    }
    publish(&router, &second, Some("only-one")).await;

    for (case, app_arn, expected) in [
        (
            "the application with three publishes",
            &first,
            vec!["1", "2", RELEASE_APP_VERSION],
        ),
        (
            "the application with one publish",
            &second,
            vec![RELEASE_APP_VERSION],
        ),
    ] {
        let listed = list_versions(&router, app_arn, &json!({})).await;
        let got = version_order(&summaries(&listed, case));
        let mut want = vec![DRAFT_APP_VERSION.to_string()];
        want.extend(expected.iter().map(|version| version.to_string()));
        assert_eq!(
            got, want,
            "case {case:?}: expected {want:?}, got {got:?}"
        );
    }

    // An application that was deleted, and one that never existed, are both the
    // modelled 404 — never an empty listing, which would claim a stored application
    // with no versions.
    let deleted = create_an_app(&router, "list-versions-deleted").await.0;
    post(&router, "/delete-app", &json!({"appArn": deleted}))
        .await
        .ok("delete the application");
    let never_created =
        format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:app/00000000-0000-0000-0000-000000000000");

    for (case, app_arn) in [
        ("a deleted application", deleted.as_str()),
        ("an application that was never created", never_created.as_str()),
    ] {
        list_versions(&router, app_arn, &json!({}))
            .await
            .refused(case, 404, "ResourceNotFoundException");
    }
}
