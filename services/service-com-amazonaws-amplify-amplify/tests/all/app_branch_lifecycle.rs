// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Amplify app + branch lifecycle.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `amplify` signing name — so what is asserted is
//! what an AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: each operation of the app/branch loop computes its answer
//! from PERSISTED state. A created record is readable field-for-field afterwards, a
//! list operation pages over exactly what exists in a stable order, a branch is
//! reachable only through its own app, and a deleted app (with its branches) is gone.
//! Each table spans the class the rule generalises over — platform, stage, page size,
//! which optional members a request carries — rather than one recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_amplify_amplify::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `amplify`
/// signing name and the `us-east-1` region: the scope is how the router picks the
/// claiming service, and how the service picks the region its ARNs are scoped to.
/// The signature itself is never checked here — the harness turns request
/// authentication off, exactly as an unauthenticated emulator runs.
const SIGNED_FOR_AMPLIFY: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAMPLIFY/20260101/us-east-1/amplify/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The account id every emulated ARN carries, and the region the harness signs for.
///
/// `pub(crate)` for the same reason as [`Emulator`]: a sibling suite asserting an ARN
/// reads the one account and region this harness signs for rather than repeating them.
pub(crate) const ACCOUNT_ID: &str = "000000000000";
pub(crate) const REGION: &str = "us-east-1";

/// One response, reduced to what a client branches on.
///
/// `pub(crate)` so a sibling suite in this test target (`tests/all/*.rs`) can drive the
/// same wire path through this harness instead of cloning it.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_code: Option<String>,
    pub(crate) body: Value,
}

impl Reply {
    /// The `app` / `branch` member of a success body.
    pub(crate) fn member(&self, name: &str) -> Value {
        self.body
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("response has no `{name}` member: {:?}", self.body))
    }

    /// The `message` member of an error body, when it carries one.
    pub(crate) fn message(&self) -> Option<String> {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// The emulator under test: ONE shared engine with the Amplify service mounted on
/// it, reached the way the production router reaches it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// The same emulator, handing back the engine it is mounted on.
    ///
    /// For a suite that has to read what NO modelled operation reports — the asset
    /// paths a deployment published onto a branch — after driving every call that
    /// produced it over the wire.
    pub(crate) fn with_state() -> (Self, Arc<engine::InMemoryEngine>) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        (Self { router }, engine)
    }

    /// The same emulator with the S3 service mounted BESIDE Amplify on the one shared
    /// engine — what the assembled router does — handing back that engine too.
    ///
    /// For the suite that drives both front doors
    /// (`tests/all/s3_sourced_deployments.rs`): `StartDeployment` reads its source by
    /// calling S3 in process, so S3 has to be there for its answer to be S3's.
    pub(crate) fn with_object_storage() -> (Self, Arc<engine::InMemoryEngine>) {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        (Self { router }, engine)
    }

    /// Dispatch a request the caller built itself, for a front door that is not
    /// Amplify's — the object storage an S3-sourced deployment is seeded through.
    pub(crate) async fn dispatch(
        &self,
        request: http::Request<bytes::Bytes>,
    ) -> http::Response<bytes::Bytes> {
        self.router.dispatch(request).await
    }

    /// The WHOLE response to a request on this service's front door, for a suite that
    /// asserts the framing (the status line, the body bytes, the headers) rather than
    /// the parsed body. [`Self::call`] is this reduced to what a client branches on.
    pub(crate) async fn call_http(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> http::Response<bytes::Bytes> {
        self.call_http_at(
            &format!("amplify.{REGION}.amazonaws.com"),
            method,
            path,
            body,
        )
        .await
    }

    /// The same call ADDRESSED to `host` — the endpoint the client was pointed at, which
    /// is what a URL or a domain this service announces has to lead back to.
    ///
    /// `pub(crate)` for the suite asserting the address an app's hosting is announced at:
    /// that value follows the endpoint the create was addressed to, so the case has to
    /// choose it.
    pub(crate) async fn call_http_at(
        &self,
        host: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> http::Response<bytes::Bytes> {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("authorization", SIGNED_FOR_AMPLIFY)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        self.router.dispatch(request).await
    }

    /// One call addressed to `host`, reduced to what a client branches on.
    pub(crate) async fn call_at(
        &self,
        host: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Reply {
        Self::read(self.call_http_at(host, method, path, body).await)
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        Self::read(self.call_http(method, path, body).await)
    }

    /// One response, reduced to what a client branches on: the status, the error code and
    /// the parsed body.
    fn read(response: http::Response<bytes::Bytes>) -> Reply {
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    pub(crate) async fn create_app(&self, request: &Value) -> Reply {
        self.call("POST", "/apps", Some(request)).await
    }

    pub(crate) async fn get_app(&self, app_id: &str) -> Reply {
        self.call("GET", &format!("/apps/{app_id}"), None).await
    }

    pub(crate) async fn delete_app(&self, app_id: &str) -> Reply {
        self.call("DELETE", &format!("/apps/{app_id}"), None).await
    }

    pub(crate) async fn list_apps(&self, query: &str) -> Reply {
        self.call("GET", &format!("/apps{query}"), None).await
    }

    pub(crate) async fn create_branch(&self, app_id: &str, request: &Value) -> Reply {
        self.call("POST", &format!("/apps/{app_id}/branches"), Some(request))
            .await
    }

    pub(crate) async fn get_branch(&self, app_id: &str, branch_name: &str) -> Reply {
        self.call("GET", &format!("/apps/{app_id}/branches/{branch_name}"), None)
            .await
    }

    pub(crate) async fn delete_branch(&self, app_id: &str, branch_name: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/apps/{app_id}/branches/{branch_name}"),
            None,
        )
        .await
    }

    pub(crate) async fn list_branches(&self, app_id: &str, query: &str) -> Reply {
        self.call("GET", &format!("/apps/{app_id}/branches{query}"), None)
            .await
    }

    pub(crate) async fn update_app(&self, app_id: &str, request: &Value) -> Reply {
        self.call("POST", &format!("/apps/{app_id}"), Some(request))
            .await
    }

    pub(crate) async fn update_branch(
        &self,
        app_id: &str,
        branch_name: &str,
        request: &Value,
    ) -> Reply {
        self.call(
            "POST",
            &format!("/apps/{app_id}/branches/{branch_name}"),
            Some(request),
        )
        .await
    }

    /// A webhook created on `app_id`.
    pub(crate) async fn create_webhook(&self, app_id: &str, request: &Value) -> Reply {
        self.call("POST", &format!("/apps/{app_id}/webhooks"), Some(request))
            .await
    }

    /// The webhook `webhook_id` names \u2014 addressed by its own id, not through an app.
    pub(crate) async fn get_webhook(&self, webhook_id: &str) -> Reply {
        self.call("GET", &format!("/webhooks/{webhook_id}"), None)
            .await
    }

    /// The webhook `webhook_id` names, updated — addressed by its own id, like the read.
    pub(crate) async fn update_webhook(&self, webhook_id: &str, request: &Value) -> Reply {
        self.call("POST", &format!("/webhooks/{webhook_id}"), Some(request))
            .await
    }

    /// One page of `app_id`'s webhooks; `query` is the raw query string (`""` for
    /// none), so a suite can drive `maxResults` and `nextToken` exactly as a client
    /// would.
    pub(crate) async fn list_webhooks(&self, app_id: &str, query: &str) -> Reply {
        self.call("GET", &format!("/apps/{app_id}/webhooks{query}"), None)
            .await
    }

    /// Create an app and return its minted `appId`.
    pub(crate) async fn app_id_of_new_app(&self, name: &str) -> String {
        let reply = self.create_app(&json!({ "name": name })).await;
        assert_eq!(reply.status, 200, "CreateApp {name}: {:?}", reply);
        string_member(&reply.member("app"), "appId")
    }

    /// Create an app CONNECTED TO A REPOSITORY PROVIDER and return its minted `appId`.
    ///
    /// `StartJob` starts a build of the app's connected repository, so it is served
    /// only for an app that has one: a suite that starts jobs creates its app this way,
    /// and one that reserves manual deployments uses [`Self::app_id_of_new_app`].
    pub(crate) async fn app_id_of_new_repository_app(&self, name: &str) -> String {
        let reply = self
            .create_app(&json!({
                "name": name,
                "repository": format!("https://example.invalid/org/{name}"),
            }))
            .await;
        assert_eq!(reply.status, 200, "CreateApp {name}: {:?}", reply);
        string_member(&reply.member("app"), "appId")
    }
}

/// Does `candidate` satisfy `AppId`'s modelled `pattern ^d[a-z0-9]+$` and
/// `length 1..20`? (`smithy_query(query="shape",shape="App")`.)
fn is_modelled_app_id(candidate: &str) -> bool {
    let mut rest = candidate.chars();
    rest.next() == Some('d')
        && candidate.len() <= 20
        && candidate.len() >= 2
        && rest.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
}

pub(crate) fn string_member(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("member `{member}` is not a string in {record:?}"))
        .to_string()
}

pub(crate) fn bool_member(record: &Value, member: &str) -> bool {
    record
        .get(member)
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("member `{member}` is not a boolean in {record:?}"))
}

pub(crate) fn list_member<'a>(record: &'a Value, member: &str) -> &'a Vec<Value> {
    record
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("member `{member}` is not a list in {record:?}"))
}

/// A timestamp member, as epoch seconds.
pub(crate) fn timestamp_member(record: &Value, member: &str) -> f64 {
    record
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("member `{member}` is not a timestamp in {record:?}"))
}

/// `record` without its `tags` member.
///
/// A READ of an app or a branch reports the resource's tag map — the empty map when
/// it has none — while the operation that WROTE the record echoes only the tags the
/// request set, so two records taken from opposite sides of that rule are compared
/// without it. The rule itself is asserted in `tests/all/app_branch_projection.rs`.
pub(crate) fn without_tags(record: &Value) -> Value {
    let mut copy = record.clone();
    if let Some(members) = copy.as_object_mut() {
        members.remove("tags");
    }
    copy
}

/// One `CreateApp` request plus the record the rule says it must produce.
///
/// `description`, `repository` and `environment_variables` are the WIRE values
/// expected for those members, so a member the request leaves unset is
/// `Value::Null` — Amplify reports a member it has no value for as JSON null, never
/// as the empty string or the empty map (see `tests/all/app_branch_projection.rs`).
struct AppCase {
    case: &'static str,
    request: Value,
    platform: &'static str,
    description: Value,
    repository: Value,
    environment_variables: Value,
    enable_branch_auto_build: bool,
    enable_basic_auth: bool,
}

/// The `CreateApp` table: it varies the platform over the WHOLE modelled enum
/// (`Platform = WEB | WEB_COMPUTE | WEB_DYNAMIC`, from
/// `smithy_query(query="shape",shape="App")`), the environment-variable map over 0,
/// 1 and 3 entries, the optional members over absent and present, and `name` over
/// the short case and the modelled `length` maximum of 255.
fn app_cases() -> Vec<AppCase> {
    vec![
        AppCase {
            case: "only the required name",
            request: json!({ "name": "demo-app" }),
            platform: "WEB",
            description: Value::Null,
            repository: Value::Null,
            environment_variables: Value::Null,
            enable_branch_auto_build: false,
            enable_basic_auth: false,
        },
        AppCase {
            case: "static app with one environment variable",
            request: json!({
                "name": "static-site",
                "platform": "WEB",
                "description": "a static site",
                "repository": "https://example.invalid/org/static-site",
                "environmentVariables": { "KEY": "VAL" },
                "enableBranchAutoBuild": true,
            }),
            platform: "WEB",
            description: json!("a static site"),
            repository: json!("https://example.invalid/org/static-site"),
            environment_variables: json!({ "KEY": "VAL" }),
            enable_branch_auto_build: true,
            enable_basic_auth: false,
        },
        AppCase {
            case: "compute app with three environment variables and basic auth",
            request: json!({
                "name": "ssr-app",
                "platform": "WEB_COMPUTE",
                "environmentVariables": { "A": "1", "B": "2", "C": "3" },
                "enableBasicAuth": true,
                "basicAuthCredentials": "dXNlcjpwYXNzd29yZA==",
                "buildSpec": "version: 1",
            }),
            platform: "WEB_COMPUTE",
            description: Value::Null,
            repository: Value::Null,
            environment_variables: json!({ "A": "1", "B": "2", "C": "3" }),
            enable_branch_auto_build: false,
            enable_basic_auth: true,
        },
        AppCase {
            case: "dynamic app with custom rules and tags",
            request: json!({
                "name": "legacy-ssr",
                "platform": "WEB_DYNAMIC",
                "description": "original SSR support",
                "customRules": [{ "source": "/old", "target": "/new", "status": "301" }],
                "tags": { "team": "web" },
            }),
            platform: "WEB_DYNAMIC",
            description: json!("original SSR support"),
            repository: Value::Null,
            environment_variables: Value::Null,
            enable_branch_auto_build: false,
            enable_basic_auth: false,
        },
        AppCase {
            case: "name at the modelled length maximum",
            request: json!({ "name": "n".repeat(255) }),
            platform: "WEB",
            description: Value::Null,
            repository: Value::Null,
            environment_variables: Value::Null,
            enable_branch_auto_build: false,
            enable_basic_auth: false,
        },
        // The two token members exist to authorise a repository and both document
        // "The token is not stored.", so a request carrying one must still come back
        // with no token anywhere on the record.
        AppCase {
            case: "github app authorised with an accessToken",
            request: json!({
                "name": "github-app",
                "repository": "https://example.invalid/org/github-app",
                "accessToken": "ghp_0123456789abcdefghijklmnopqrstuvwxyz",
            }),
            platform: "WEB",
            description: Value::Null,
            repository: json!("https://example.invalid/org/github-app"),
            environment_variables: Value::Null,
            enable_branch_auto_build: false,
            enable_basic_auth: false,
        },
        AppCase {
            case: "third-party app authorised with an oauthToken",
            request: json!({
                "name": "bitbucket-app",
                "repository": "https://example.invalid/org/bitbucket-app",
                "oauthToken": "oauth-0123456789abcdef",
            }),
            platform: "WEB",
            description: Value::Null,
            repository: json!("https://example.invalid/org/bitbucket-app"),
            environment_variables: Value::Null,
            enable_branch_auto_build: false,
            enable_basic_auth: false,
        },
    ]
}

#[tokio::test]
async fn create_app_mints_and_persists_a_record_get_app_reads_back_field_for_field() {
    let emulator = Emulator::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for case in app_cases() {
        let label = case.case;
        let created = emulator.create_app(&case.request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateApp, got {created:?}"
        );

        let app = created.member("app");
        let app_id = string_member(&app, "appId");
        assert!(
            is_modelled_app_id(&app_id),
            "case {label:?}: expected a minted appId matching ^d[a-z0-9]+$ of at most 20 \
             characters, got {app_id:?}"
        );
        assert!(
            minted.insert(app_id.clone()),
            "case {label:?}: expected a fresh appId, got the already-minted {app_id:?}"
        );

        assert_eq!(
            string_member(&app, "appArn"),
            format!("arn:aws:amplify:{REGION}:{ACCOUNT_ID}:apps/{app_id}"),
            "case {label:?}: appArn must be derived from the minted appId, got {:?}",
            app.get("appArn")
        );
        assert_eq!(
            string_member(&app, "defaultDomain"),
            format!("{app_id}.amplifyapp.com"),
            "case {label:?}: defaultDomain must be derived from the minted appId, got {:?}",
            app.get("defaultDomain")
        );
        assert_eq!(
            app.get("name"),
            case.request.get("name"),
            "case {label:?}: name must be the requested one, got {:?}",
            app.get("name")
        );
        assert_eq!(
            string_member(&app, "platform"),
            case.platform,
            "case {label:?}: unexpected platform, got {:?}",
            app.get("platform")
        );
        assert_eq!(
            app.get("description"),
            Some(&case.description),
            "case {label:?}: unexpected description, got {:?}",
            app.get("description")
        );
        assert_eq!(
            app.get("repository"),
            Some(&case.repository),
            "case {label:?}: unexpected repository, got {:?}",
            app.get("repository")
        );
        assert_eq!(
            app.get("environmentVariables"),
            Some(&case.environment_variables),
            "case {label:?}: unexpected environmentVariables, got {:?}",
            app.get("environmentVariables")
        );
        assert_eq!(
            bool_member(&app, "enableBranchAutoBuild"),
            case.enable_branch_auto_build,
            "case {label:?}: unexpected enableBranchAutoBuild, got {:?}",
            app.get("enableBranchAutoBuild")
        );
        assert_eq!(
            bool_member(&app, "enableBasicAuth"),
            case.enable_basic_auth,
            "case {label:?}: unexpected enableBasicAuth, got {:?}",
            app.get("enableBasicAuth")
        );

        let create_time = timestamp_member(&app, "createTime");
        let update_time = timestamp_member(&app, "updateTime");
        assert!(
            create_time > 0.0,
            "case {label:?}: expected a real createTime, got {create_time}"
        );
        assert_eq!(
            create_time, update_time,
            "case {label:?}: a newly created app has not been updated since, so createTime \
             and updateTime must match, got {create_time} and {update_time}"
        );

        // The round trip: GetApp must answer with the record CreateApp persisted,
        // every member included.
        let fetched = emulator.get_app(&app_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected HTTP 200 from GetApp, got {fetched:?}"
        );
        assert_eq!(
            without_tags(&fetched.member("app")),
            without_tags(&app),
            "case {label:?}: GetApp must return the created record field-for-field, got {:?}",
            fetched.member("app")
        );

        // A secret the request supplied is never echoed back on the record: both
        // token members document "The token is not stored."
        let serialised = created.body.to_string();
        assert!(
            app.get("accessToken").is_none() && app.get("oauthToken").is_none(),
            "case {label:?}: a repository token must not be persisted on the app, got {app:?}"
        );
        for secret in ["ghp_", "oauth-"] {
            assert!(
                !serialised.contains(secret),
                "case {label:?}: the response must not carry the repository token, got \
                 {serialised}"
            );
        }
        let read_back = emulator.get_app(&app_id).await.body.to_string();
        for secret in ["ghp_", "oauth-"] {
            assert!(
                !read_back.contains(secret),
                "case {label:?}: the stored record must not carry the repository token, got \
                 {read_back}"
            );
        }
    }
}

#[tokio::test]
async fn list_apps_pages_over_everything_in_state_for_every_page_bound() {
    let emulator = Emulator::new();

    let mut expected: BTreeSet<String> = BTreeSet::new();
    for index in 0..7 {
        expected.insert(emulator.app_id_of_new_app(&format!("page-app-{index}")).await);
    }

    // The page bounds span 1, a middle value, exactly the number of records, more
    // than the number of records, the modelled `range` maximum of 100, one past
    // that maximum (refused, not served), and the "caller set no bound" cases: the
    // member absent, and its modelled default of 0.
    let bounds: Vec<(&str, String, Option<usize>)> = vec![
        ("maxResults=1", "?maxResults=1".to_string(), Some(1)),
        ("maxResults=3", "?maxResults=3".to_string(), Some(3)),
        ("maxResults=7", "?maxResults=7".to_string(), Some(7)),
        ("maxResults=8", "?maxResults=8".to_string(), Some(8)),
        ("maxResults=100", "?maxResults=100".to_string(), Some(100)),
        ("maxResults absent", String::new(), None),
        ("maxResults=0", "?maxResults=0".to_string(), None),
    ];

    for (label, query, bound) in bounds {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let separator = if query.is_empty() { '?' } else { '&' };
            let paged = match &token {
                Some(value) => format!("{query}{separator}nextToken={value}"),
                None => query.clone(),
            };
            let reply = emulator.list_apps(&paged).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected HTTP 200 from ListApps, got {reply:?}"
            );
            let apps = list_member(&reply.body, "apps").clone();
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {label:?}: paging did not terminate after {pages} pages"
            );

            let remaining = expected.len() - seen.len();
            let want = bound.unwrap_or(100).min(remaining);
            assert_eq!(
                apps.len(),
                want,
                "case {label:?}: page {pages} must hold min(bound, remaining) = {want} apps, \
                 got {}",
                apps.len()
            );

            for app in &apps {
                seen.push(string_member(app, "appId"));
            }

            token = reply
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {label:?}: a paged walk must not repeat an app, got {seen:?}"
        );
        assert_eq!(
            unique, expected,
            "case {label:?}: a paged walk must visit every app in state, got {unique:?}"
        );
    }

    // One past the modelled maximum is refused by the boundary validator, not served.
    let refused = emulator.list_apps("?maxResults=101").await;
    assert_eq!(
        refused.status, 400,
        "case \"maxResults=101\": a value past the modelled range maximum must be refused, \
         got {refused:?}"
    );

    // A token this service never issued is refused, not served as an arbitrary
    // suffix of the collection. Every token it does issue is an appId.
    for (label, token) in [
        ("token that is not an appId", "not-an-app-id"),
        ("token with an uppercase letter", "dABC123"),
        ("token without the leading d", "x0123456789"),
    ] {
        let reply = emulator.list_apps(&format!("?nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: an unissued nextToken must be refused, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("BadRequestException"),
            "case {label:?}: expected BadRequestException, got {:?}",
            reply.error_code
        );
    }
}

/// One `CreateBranch` request plus the record the rule says it must produce.
///
/// `environment_variables` is the WIRE value expected for that member, so a request
/// that sets none expects `Value::Null`: Amplify reports a member it has no value
/// for as JSON null (see `tests/all/app_branch_projection.rs`).
struct BranchCase {
    case: &'static str,
    request: Value,
    stage: &'static str,
    display_name: &'static str,
    ttl: &'static str,
    framework: Value,
    enable_auto_build: bool,
    environment_variables: Value,
}

/// The `CreateBranch` table: it covers EVERY value of the modelled `Stage` enum
/// (`BETA | DEVELOPMENT | EXPERIMENTAL | PRODUCTION | PULL_REQUEST`, from
/// `smithy_query(query="shape",shape="Stage")`) — the issue cited only
/// `PRODUCTION` — and varies `ttl`, `displayName`, `framework`, `enableAutoBuild`
/// (absent, explicitly true, explicitly false) and the environment-variable map
/// between absent and present.
fn branch_cases() -> Vec<BranchCase> {
    vec![
        BranchCase {
            case: "only the required branchName",
            request: json!({ "branchName": "dev" }),
            // A branch created without a stage is assigned none: `NONE`, not one of
            // the five values `Stage` models (see `create_branch`'s `DEFAULT_STAGE`).
            stage: "NONE",
            display_name: "dev",
            ttl: "5",
            framework: Value::Null,
            // An omitted `enableAutoBuild` leaves the branch auto-building: the
            // service's default, not the Rust type's zero.
            enable_auto_build: true,
            environment_variables: Value::Null,
        },
        BranchCase {
            case: "production branch with a framework and an explicit ttl",
            request: json!({
                "branchName": "main",
                "stage": "PRODUCTION",
                "framework": "React",
                "ttl": "60",
                "enableAutoBuild": true,
                "description": "the primary branch",
            }),
            stage: "PRODUCTION",
            display_name: "main",
            ttl: "60",
            framework: json!("React"),
            enable_auto_build: true,
            environment_variables: Value::Null,
        },
        BranchCase {
            case: "beta branch with its own display name and environment",
            request: json!({
                "branchName": "beta-one",
                "stage": "BETA",
                "displayName": "beta-display",
                "environmentVariables": { "A": "1", "B": "2" },
            }),
            stage: "BETA",
            display_name: "beta-display",
            ttl: "5",
            framework: Value::Null,
            enable_auto_build: true,
            environment_variables: json!({ "A": "1", "B": "2" }),
        },
        BranchCase {
            case: "pull-request branch with previews enabled",
            request: json!({
                "branchName": "pr-7",
                "stage": "PULL_REQUEST",
                "enablePullRequestPreview": true,
                "pullRequestEnvironmentName": "pr-env",
                // The request decides the member in EITHER direction, so this row
                // pins an explicit `false` against the service default of `true`.
                "enableAutoBuild": false,
            }),
            stage: "PULL_REQUEST",
            display_name: "pr-7",
            ttl: "5",
            framework: Value::Null,
            enable_auto_build: false,
            environment_variables: Value::Null,
        },
        BranchCase {
            case: "experimental branch with a zero ttl and basic auth",
            request: json!({
                "branchName": "Experimental.1",
                "stage": "EXPERIMENTAL",
                "ttl": "0",
                "enableNotification": true,
                "enableBasicAuth": true,
                "basicAuthCredentials": "dXNlcjpwYXNz",
            }),
            stage: "EXPERIMENTAL",
            display_name: "Experimental.1",
            ttl: "0",
            framework: Value::Null,
            enable_auto_build: true,
            environment_variables: Value::Null,
        },
    ]
}

#[tokio::test]
async fn create_branch_persists_under_its_app_and_get_branch_reads_it_back() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("branch-host").await;
    let app_arn = string_member(&emulator.get_app(&app_id).await.member("app"), "appArn");

    for case in branch_cases() {
        let label = case.case;
        let created = emulator.create_branch(&app_id, &case.request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );

        let branch = created.member("branch");
        let branch_name = string_member(&branch, "branchName");
        assert_eq!(
            branch.get("branchName"),
            case.request.get("branchName"),
            "case {label:?}: branchName must be the requested one, got {branch_name:?}"
        );
        assert_eq!(
            string_member(&branch, "branchArn"),
            format!("{app_arn}/branches/{branch_name}"),
            "case {label:?}: branchArn must be derived from the parent app's ARN, got {:?}",
            branch.get("branchArn")
        );
        assert_eq!(
            string_member(&branch, "stage"),
            case.stage,
            "case {label:?}: unexpected stage, got {:?}",
            branch.get("stage")
        );
        assert_eq!(
            string_member(&branch, "displayName"),
            case.display_name,
            "case {label:?}: unexpected displayName, got {:?}",
            branch.get("displayName")
        );
        assert_eq!(
            string_member(&branch, "ttl"),
            case.ttl,
            "case {label:?}: unexpected ttl, got {:?}",
            branch.get("ttl")
        );
        assert_eq!(
            branch.get("framework"),
            Some(&case.framework),
            "case {label:?}: unexpected framework, got {:?}",
            branch.get("framework")
        );
        assert_eq!(
            bool_member(&branch, "enableAutoBuild"),
            case.enable_auto_build,
            "case {label:?}: unexpected enableAutoBuild, got {:?}",
            branch.get("enableAutoBuild")
        );
        assert_eq!(
            branch.get("environmentVariables"),
            Some(&case.environment_variables),
            "case {label:?}: unexpected environmentVariables, got {:?}",
            branch.get("environmentVariables")
        );
        assert_eq!(
            branch.get("activeJobId"),
            Some(&Value::Null),
            "case {label:?}: a branch that was never built has no active job, so Amplify \
             reports activeJobId as null, got {:?}",
            branch.get("activeJobId")
        );
        assert_eq!(
            string_member(&branch, "totalNumberOfJobs"),
            "0",
            "case {label:?}: a branch that was never built has run no jobs, got {:?}",
            branch.get("totalNumberOfJobs")
        );
        assert_eq!(
            branch.get("customDomains"),
            Some(&Value::Null),
            "case {label:?}: a new branch has no custom domains, so Amplify reports \
             customDomains as null, got {:?}",
            branch.get("customDomains")
        );

        let create_time = timestamp_member(&branch, "createTime");
        assert_eq!(
            create_time,
            timestamp_member(&branch, "updateTime"),
            "case {label:?}: a newly created branch has not been updated since"
        );

        let fetched = emulator.get_branch(&app_id, &branch_name).await;
        assert_eq!(
            fetched.status, 200,
            "case {label:?}: expected HTTP 200 from GetBranch, got {fetched:?}"
        );
        assert_eq!(
            without_tags(&fetched.member("branch")),
            without_tags(&branch),
            "case {label:?}: GetBranch must return the created record field-for-field, got {:?}",
            fetched.member("branch")
        );

        // A second create under the same name is refused, not silently merged.
        let duplicate = emulator.create_branch(&app_id, &case.request).await;
        assert_eq!(
            duplicate.status, 400,
            "case {label:?}: a duplicate branchName must be refused, got {duplicate:?}"
        );
        assert_eq!(
            duplicate.error_code.as_deref(),
            Some("BadRequestException"),
            "case {label:?}: a duplicate branchName must be refused as BadRequestException, \
             got {:?}",
            duplicate.error_code
        );
    }

    // Every branch of the table is listed for this app, and the list pages the same
    // way ListApps does.
    let expected: BTreeSet<String> = branch_cases()
        .iter()
        .map(|case| string_member(&case.request, "branchName"))
        .collect();

    for bound in [None, Some(1), Some(2), Some(50)] {
        let label = bound.map_or("maxResults absent".to_string(), |b| format!("maxResults={b}"));
        let query = bound.map_or(String::new(), |b| format!("?maxResults={b}"));
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;

        loop {
            let separator = if query.is_empty() { '?' } else { '&' };
            let paged = match &token {
                Some(value) => format!("{query}{separator}nextToken={value}"),
                None => query.clone(),
            };
            let reply = emulator.list_branches(&app_id, &paged).await;
            assert_eq!(
                reply.status, 200,
                "case {label:?}: expected HTTP 200 from ListBranches, got {reply:?}"
            );
            let branches = list_member(&reply.body, "branches").clone();
            let remaining = expected.len() - seen.len();
            let want = bound.unwrap_or(50).min(remaining);
            assert_eq!(
                branches.len(),
                want,
                "case {label:?}: a page must hold min(bound, remaining) = {want} branches, got {}",
                branches.len()
            );
            for branch in &branches {
                seen.push(string_member(branch, "branchName"));
            }
            token = reply
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }

        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {label:?}: a paged walk must not repeat a branch, got {seen:?}"
        );
        assert_eq!(
            unique, expected,
            "case {label:?}: a paged walk must visit every branch of the app, got {unique:?}"
        );
    }

    // A token this service never issued is refused. Every token it does issue is a
    // branchName, which is modelled `length 1..255`.
    for (label, token) in [
        ("empty token", String::new()),
        ("token past the branchName length maximum", "b".repeat(256)),
    ] {
        let reply = emulator
            .list_branches(&app_id, &format!("?nextToken={token}"))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: an unissued nextToken must be refused, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("BadRequestException"),
            "case {label:?}: expected BadRequestException, got {:?}",
            reply.error_code
        );
    }
}

/// The published AWS Amplify service quotas this emulator enforces: 25 apps per
/// Region, 50 branches per app. Past either, the create answers the modelled
/// `LimitExceededException` (HTTP 429) and stores nothing.
const MAX_APPS: usize = 25;
const MAX_BRANCHES_PER_APP: usize = 50;

#[tokio::test]
async fn a_create_past_the_service_quota_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();

    let mut app_ids: Vec<String> = Vec::new();
    for index in 0..MAX_APPS {
        let reply = emulator.create_app(&json!({ "name": format!("quota-app-{index}") })).await;
        assert_eq!(
            reply.status, 200,
            "case \"app {index} of the quota\": expected HTTP 200 from CreateApp, got {reply:?}"
        );
        app_ids.push(string_member(&reply.member("app"), "appId"));
    }

    let past_quota = emulator.create_app(&json!({ "name": "one-too-many" })).await;
    assert_eq!(
        past_quota.status, 429,
        "case \"app past the quota\": expected HTTP 429, got {past_quota:?}"
    );
    assert_eq!(
        past_quota.error_code.as_deref(),
        Some("LimitExceededException"),
        "case \"app past the quota\": expected LimitExceededException, got {:?}",
        past_quota.error_code
    );

    // Nothing was stored for the refused create: the walk still finds exactly the
    // apps that were accepted.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(value) => format!("?nextToken={value}"),
            None => String::new(),
        };
        let reply = emulator.list_apps(&query).await;
        for app in list_member(&reply.body, "apps") {
            seen.insert(string_member(app, "appId"));
        }
        token = reply
            .body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen,
        app_ids.iter().cloned().collect::<BTreeSet<String>>(),
        "case \"app past the quota\": a refused create must not store a record, got {seen:?}"
    );

    // And the same bound holds for the branches of one app.
    let host = &app_ids[0];
    for index in 0..MAX_BRANCHES_PER_APP {
        let reply = emulator
            .create_branch(host, &json!({ "branchName": format!("quota-branch-{index:03}") }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"branch {index} of the quota\": expected HTTP 200 from CreateBranch, got \
             {reply:?}"
        );
    }

    let past_branch_quota = emulator
        .create_branch(host, &json!({ "branchName": "one-too-many" }))
        .await;
    assert_eq!(
        past_branch_quota.status, 429,
        "case \"branch past the quota\": expected HTTP 429, got {past_branch_quota:?}"
    );
    assert_eq!(
        past_branch_quota.error_code.as_deref(),
        Some("LimitExceededException"),
        "case \"branch past the quota\": expected LimitExceededException, got {:?}",
        past_branch_quota.error_code
    );
    assert_eq!(
        emulator.get_branch(host, "one-too-many").await.status,
        404,
        "case \"branch past the quota\": a refused create must not store a record"
    );

    // The quota is per app, so another app can still take a branch of its own.
    let other = emulator
        .create_branch(&app_ids[1], &json!({ "branchName": "one-too-many" }))
        .await;
    assert_eq!(
        other.status, 200,
        "case \"branch on another app\": the branch quota is per app, got {other:?}"
    );
}

#[tokio::test]
async fn a_branch_is_reachable_only_through_the_app_that_owns_it() {
    let emulator = Emulator::new();
    let first = emulator.app_id_of_new_app("first-app").await;
    let second = emulator.app_id_of_new_app("second-app").await;

    // The SAME branch names on two apps, so a leak between them is visible.
    for (app_id, names) in [(&first, ["main", "dev"]), (&second, ["main", "release"])] {
        for name in names {
            let created = emulator
                .create_branch(app_id, &json!({ "branchName": name }))
                .await;
            assert_eq!(
                created.status, 200,
                "case {app_id}/{name:?}: expected HTTP 200 from CreateBranch, got {created:?}"
            );
        }
    }

    for (app_id, expected) in [(&first, ["dev", "main"]), (&second, ["main", "release"])] {
        let listed = emulator.list_branches(app_id, "").await;
        let names: Vec<String> = list_member(&listed.body, "branches")
            .iter()
            .map(|branch| string_member(branch, "branchName"))
            .collect();
        assert_eq!(
            names, expected,
            "case {app_id}: ListBranches must return exactly this app's branches, in \
             branchName order, got {names:?}"
        );
    }

    // A branch name that exists only on the first app is not found on the second.
    let cross = emulator.get_branch(&second, "dev").await;
    assert_eq!(
        cross.status, 404,
        "case \"cross-app read\": a branch of another app must not be readable, got {cross:?}"
    );
    assert_eq!(
        cross.error_code.as_deref(),
        Some("NotFoundException"),
        "case \"cross-app read\": expected NotFoundException, got {:?}",
        cross.error_code
    );

    // And each branchArn names its own app.
    for app_id in [&first, &second] {
        let branch = emulator.get_branch(app_id, "main").await.member("branch");
        let arn = string_member(&branch, "branchArn");
        assert!(
            arn.contains(&format!("apps/{app_id}/branches/main")),
            "case {app_id}: branchArn must name the owning app, got {arn:?}"
        );
    }
}

#[tokio::test]
async fn an_operation_addressed_to_a_record_that_is_not_in_state_answers_not_found() {
    let emulator = Emulator::new();
    let app_id = emulator.app_id_of_new_app("the-only-app").await;

    // `dnosuchapp1` satisfies AppId's modelled pattern, so these reach the handler
    // rather than the boundary validator.
    let missing = "dnosuchapp1";
    let cases: Vec<(&str, Reply)> = vec![
        ("GetApp", emulator.get_app(missing).await),
        ("DeleteApp", emulator.delete_app(missing).await),
        ("ListBranches", emulator.list_branches(missing, "").await),
        ("GetBranch", emulator.get_branch(missing, "main").await),
        (
            "CreateBranch",
            emulator
                .create_branch(missing, &json!({ "branchName": "main" }))
                .await,
        ),
        (
            "GetBranch on an existing app",
            emulator.get_branch(&app_id, "no-such-branch").await,
        ),
    ];

    for (label, reply) in cases {
        assert_eq!(
            reply.status, 404,
            "case {label:?}: an unknown record must answer HTTP 404, got {reply:?}"
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some("NotFoundException"),
            "case {label:?}: expected the NotFoundException shape, got {:?}",
            reply.error_code
        );
    }
}

#[tokio::test]
async fn delete_app_returns_the_deleted_record_and_removes_it_with_its_branches() {
    let emulator = Emulator::new();
    let doomed = emulator.app_id_of_new_app("doomed-app").await;
    let survivor = emulator.app_id_of_new_app("surviving-app").await;

    for name in ["main", "dev"] {
        let created = emulator
            .create_branch(&doomed, &json!({ "branchName": name }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected HTTP 200 from CreateBranch, got {created:?}"
        );
    }

    let before = emulator.get_app(&doomed).await.member("app");
    let deleted = emulator.delete_app(&doomed).await;
    assert_eq!(
        deleted.status, 200,
        "case \"DeleteApp\": expected HTTP 200, got {deleted:?}"
    );
    assert_eq!(
        without_tags(&deleted.member("app")),
        without_tags(&before),
        "case \"DeleteApp\": the response must be the record as it existed at delete time, \
         got {:?}",
        deleted.member("app")
    );

    let after = emulator.get_app(&doomed).await;
    assert_eq!(
        after.status, 404,
        "case \"GetApp after DeleteApp\": the app must be gone, got {after:?}"
    );

    for (label, reply) in [
        ("GetBranch after DeleteApp", emulator.get_branch(&doomed, "main").await),
        ("ListBranches after DeleteApp", emulator.list_branches(&doomed, "").await),
    ] {
        assert_eq!(
            reply.status, 404,
            "case {label:?}: a deleted app's branches must be gone too, got {reply:?}"
        );
    }

    let listed = emulator.list_apps("").await;
    let ids: Vec<String> = list_member(&listed.body, "apps")
        .iter()
        .map(|app| string_member(app, "appId"))
        .collect();
    assert_eq!(
        ids,
        vec![survivor.clone()],
        "case \"ListApps after DeleteApp\": only the surviving app may be listed, got {ids:?}"
    );

    // The surviving app is untouched, and a second delete of the same app is a 404.
    let again = emulator.delete_app(&doomed).await;
    assert_eq!(
        again.status, 404,
        "case \"DeleteApp twice\": the second delete must answer 404, got {again:?}"
    );
    assert_eq!(
        emulator.get_app(&survivor).await.status,
        200,
        "case \"survivor\": deleting one app must not disturb another"
    );
}
