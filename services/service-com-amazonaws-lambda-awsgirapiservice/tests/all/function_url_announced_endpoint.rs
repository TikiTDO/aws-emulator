// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ADDRESS a function URL's `FunctionUrl` announces.
//!
//! The rule, one level above the recorded case that filed the issue (a function URL read
//! back by a client pointed at the emulator's loopback endpoint):
//!
//! > The address this service announces for a function URL is one this emulator itself
//! > answers on, so a client that follows `FunctionUrl` verbatim — "the HTTP URL endpoint
//! > for your function" — reaches the function the URL is attached to. A client configured
//! > with the emulator's own endpoint is announced that endpoint with the url id behind the
//! > reserved data-plane path; a client that resolves AWS's own hosts is announced the
//! > `<url-id>.lambda-url.<region>.on.aws` host that member documents, which this service
//! > claims host-style. Whichever form it is, the value is resolved once at creation, so
//! > `CreateFunctionUrlConfig`, `GetFunctionUrlConfig`, `ListFunctionUrlConfigs` and
//! > `UpdateFunctionUrlConfig` all report the one announced address.
//!
//! The rows span what the rule generalises over: the endpoint the control-plane request was
//! addressed at (an IPv4 loopback literal with a port, a loopback NAME with a port, a
//! bracketed IPv6 literal, a bare IPv4 literal carrying no port, a full IPv6 literal, an
//! address so long that the announced local form would pass the member's modelled length
//! ceiling, and one of AWS's own service hosts), the Region the request is signed for, and
//! the qualifier the URL is attached to (the function's unpublished version and an alias).
//! Each row is checked against every one of the four reporters and the announced address is
//! then FETCHED, at the URL's own root and at a path below it, against a function whose
//! package RUNS — so an address that was announced but is served by nothing fails here.

use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_url_data_plane::{router_on, send, Answer};
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The runtime every function below is created with, and the handler its package exposes.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(200);

/// The bounds the model puts on `FunctionUrl` (`@length(min: 40, max: 100)`).
const FUNCTION_URL_MIN_LEN: usize = 40;
const FUNCTION_URL_MAX_LEN: usize = 100;

/// The reserved data-plane path segment the emulator's own endpoint carries a plane behind
/// (`service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE`), and the segment that
/// names THIS plane behind it (the host label `FunctionUrlConfig$FunctionUrl` documents).
const LOCAL_MARKER: &str = "/_aws/lambda-url";

/// The front door's envelope for a request no service claimed. The defect this suite pins
/// is an address nothing serves, so this is never a legitimate answer here.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// Which address a row must be announced on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Announced {
    /// The emulator's own endpoint, with the url id behind the reserved data-plane path.
    EmulatorEndpoint,
    /// The `<url-id>.lambda-url.<region>.on.aws` host `FunctionUrlConfig$FunctionUrl`
    /// documents.
    FunctionUrlHost,
}

/// One row: the endpoint the control-plane calls are addressed at, the Region they are
/// signed for, the qualifier the URL is attached to, and the address it must be announced
/// on.
struct Row {
    case: &'static str,
    /// The `Host` the control-plane request carries — the endpoint the client is configured
    /// with.
    host: &'static str,
    region: &'static str,
    /// The alias the URL is attached to, or `None` for the function's unpublished version.
    qualifier: Option<&'static str>,
    announced: Announced,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: a client configured with the emulator's loopback \
                   endpoint, on the function's unpublished version",
            host: "127.0.0.1:45923",
            region: "us-west-2",
            qualifier: None,
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "the same endpoint named by its loopback NAME, on an alias",
            host: "localhost:9297",
            region: "eu-west-1",
            qualifier: Some("live"),
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "the same endpoint named by a bracketed IPv6 literal",
            host: "[::1]:9297",
            region: "ap-southeast-2",
            qualifier: None,
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "the same endpoint named by a bare IPv4 literal carrying no port",
            host: "127.0.0.1",
            region: "us-east-1",
            qualifier: None,
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "a longer address for the same endpoint, a full IPv6 literal with a port, \
                   which the announced value still fits the modelled length with",
            host: "[2001:db8::1]:39355",
            region: "ap-northeast-1",
            qualifier: None,
            announced: Announced::EmulatorEndpoint,
        },
        Row {
            case: "an address so long that announcing the URL behind the emulator's own \
                   endpoint would pass the modelled FunctionUrl length: the documented \
                   endpoint is announced instead of a value the model refuses",
            host: "[2001:db8:85a3:8d3:1319:8a2e:370:7348]:39355",
            region: "us-west-2",
            qualifier: None,
            announced: Announced::FunctionUrlHost,
        },
        Row {
            case: "a client that resolves AWS's own hosts: the documented function URL host",
            host: "lambda.us-east-1.amazonaws.com",
            region: "us-east-1",
            qualifier: None,
            announced: Announced::FunctionUrlHost,
        },
        Row {
            case: "a client that resolves AWS's own hosts, an alias in another Region",
            host: "lambda.eu-central-1.amazonaws.com",
            region: "eu-central-1",
            qualifier: Some("stable"),
            announced: Announced::FunctionUrlHost,
        },
    ]
}

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `region`,
/// which is what routes a control-plane request here and which Region its resources carry.
/// Nothing in it is verified: the harness turns request authentication off.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{region}/lambda/\
         aws4_request, SignedHeaders=host, Signature=not-verified"
    )
}

/// One CONTROL-plane call, addressed at the endpoint `host` names and signed for `region`.
async fn control(
    router: &MultiServiceRouter,
    host: &str,
    region: &str,
    method: &str,
    uri: &str,
    body: &Value,
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let scope = authorization(region);
    send(
        router,
        method,
        host,
        uri,
        &payload,
        &[
            ("authorization", scope.as_str()),
            ("content-type", "application/json"),
        ],
    )
    .await
}

/// The handler every row runs: it answers a document naming the `rawPath` the event carried,
/// so the answer is the RUN's and it reports which path within the URL the request reached.
fn echoing_handler() -> String {
    r#"
def lambda_handler(event, context):
    return {"reached": event["rawPath"]}
"#
    .to_string()
}

/// Create the row's function, publish a version and point the row's alias at it when the row
/// attaches its URL to one, and grant the public invoke that makes an `AuthType=NONE`
/// endpoint public.
async fn given_function(router: &MultiServiceRouter, row: &Row, name: &str) {
    let created = control(
        router,
        row.host,
        row.region,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": 20,
            "MemorySize": 512,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&echoing_handler())),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {:?}: CreateFunction must answer 201, got {}",
        row.case,
        created.summary()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    if let Some(alias) = row.qualifier {
        let published = control(
            router,
            row.host,
            row.region,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/versions"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            published.status,
            201,
            "case {:?}: PublishVersion must answer 201, got {}",
            row.case,
            published.summary()
        );
        let version = published
            .json()
            .get("Version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let created_alias = control(
            router,
            row.host,
            row.region,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/aliases"),
            &json!({ "Name": alias, "FunctionVersion": version }),
        )
        .await;
        assert_eq!(
            created_alias.status,
            201,
            "case {:?}: CreateAlias must answer 201, got {}",
            row.case,
            created_alias.summary()
        );
    }

    let granted = control(
        router,
        row.host,
        row.region,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/policy{}", qualifier_query(row)),
        &json!({
            "StatementId": "public-url",
            "Action": "lambda:InvokeFunctionUrl",
            "Principal": "*",
            "FunctionUrlAuthType": "NONE",
        }),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "case {:?}: AddPermission must answer 201, got {}",
        row.case,
        granted.summary()
    );
}

/// The `?Qualifier=` the row's calls carry, empty for a URL on the unpublished version.
fn qualifier_query(row: &Row) -> String {
    row.qualifier
        .map_or_else(String::new, |alias| format!("?Qualifier={alias}"))
}

/// The `FunctionUrl` member of an answer that must carry one.
fn announced_url(case: &str, reporter: &str, answer: &Answer) -> String {
    answer
        .json()
        .get("FunctionUrl")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {reporter} must answer a FunctionUrl, got {}", answer.summary())
        })
}

/// The announced URL split into the authority a client dials and the path it carries — both
/// forms end in `/`, which is the path a function URL's own root is addressed by.
fn announced_parts(announced: &str) -> (String, String) {
    let without_scheme = announced
        .split_once("://")
        .map_or(announced, |(_scheme, rest)| rest);
    match without_scheme.find('/') {
        Some(at) => (
            without_scheme[..at].to_string(),
            without_scheme[at..].to_string(),
        ),
        None => (without_scheme.to_string(), String::new()),
    }
}

/// Assert `announced` is an address this emulator answers on, in the form this row's client
/// is announced, and answer the authority and the base path a client fetches it at.
fn assert_announced(row: &Row, reporter: &str, announced: &str) -> (String, String) {
    let case = row.case;
    assert!(
        announced.len() >= FUNCTION_URL_MIN_LEN && announced.len() <= FUNCTION_URL_MAX_LEN,
        "case {case:?}: {reporter} must announce a FunctionUrl inside the modelled \
         {FUNCTION_URL_MIN_LEN}..{FUNCTION_URL_MAX_LEN} length, got {} in {announced:?}",
        announced.len()
    );
    assert!(
        announced.starts_with("https://"),
        "case {case:?}: {reporter} must announce an https endpoint, got {announced:?}"
    );
    let (authority, base_path) = announced_parts(announced);
    assert!(
        base_path.ends_with('/'),
        "case {case:?}: {reporter} must announce the endpoint's own root, got {announced:?}"
    );
    match row.announced {
        Announced::EmulatorEndpoint => {
            assert_eq!(
                authority, row.host,
                "case {case:?}: {reporter} must announce the endpoint the request reached, \
                 got {announced:?}"
            );
            assert!(
                !authority.ends_with("on.aws") && !authority.ends_with("amazonaws.com"),
                "case {case:?}: {reporter} must not announce an AWS host no client of this \
                 emulator resolves, got {announced:?}"
            );
            let url_id = base_path
                .strip_prefix(&format!("{LOCAL_MARKER}/"))
                .and_then(|rest| rest.strip_suffix('/'))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {reporter} must name the url id behind the reserved \
                         data-plane path {LOCAL_MARKER}, got {announced:?}"
                    )
                });
            assert_opaque_url_id(case, reporter, url_id, announced);
        }
        Announced::FunctionUrlHost => {
            let suffix = format!(".lambda-url.{}.on.aws", row.region);
            let url_id = authority.strip_suffix(&suffix).unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {reporter} must announce the documented \
                     <url-id>{suffix} host, got {announced:?}"
                )
            });
            assert_opaque_url_id(case, reporter, url_id, announced);
            assert_eq!(
                base_path, "/",
                "case {case:?}: the documented endpoint carries no path of its own, got \
                 {announced:?}"
            );
        }
    }
    (authority, base_path)
}

/// The url id in either form is the opaque lowercase alphanumeric label AWS mints.
fn assert_opaque_url_id(case: &str, reporter: &str, url_id: &str, announced: &str) {
    assert!(
        !url_id.is_empty()
            && url_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
        "case {case:?}: {reporter} must name an opaque lowercase alphanumeric url id, got \
         {url_id:?} in {announced:?}"
    );
}

/// Fetch the announced URL at `path` and assert the function the URL is attached to served
/// it, reporting the path WITHIN the URL it was reached at.
async fn assert_served(
    router: &MultiServiceRouter,
    case: &str,
    form: &str,
    authority: &str,
    path: &str,
    reached: &str,
) {
    let answer = send(router, "GET", authority, path, &[], &[]).await;
    assert_ne!(
        answer.header("x-amzn-errortype").as_deref(),
        Some(UNKNOWN_SERVICE),
        "case {case:?}: the announced address must be one this emulator serves, but {form} \
         ({authority}{path}) is claimed by no service: {}",
        answer.summary()
    );
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: {form} ({authority}{path}) must reach the function, got {}",
        answer.summary()
    );
    assert_eq!(
        answer.json().get("reached").and_then(Value::as_str),
        Some(reached),
        "case {case:?}: {form} must reach the function at {reached:?}, got {}",
        answer.summary()
    );
}

/// Every reporter of a function URL announces one address this emulator serves, and the
/// function answers on it.
#[tokio::test]
async fn the_announced_function_url_is_an_address_this_emulator_serves() {
    for (index, row) in rows().into_iter().enumerate() {
        // One emulator per row, so ListFunctionUrlConfigs lists exactly this row's URL.
        let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let case = row.case;
        let function = format!("announced-url-{index}");
        given_function(&router, &row, &function).await;

        let query = qualifier_query(&row);
        let created = control(
            &router,
            row.host,
            row.region,
            "POST",
            &format!("/2021-10-31/functions/{function}/url{query}"),
            &json!({ "AuthType": "NONE" }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateFunctionUrlConfig must answer 201, got {}",
            created.summary()
        );
        let announced = announced_url(case, "CreateFunctionUrlConfig", &created);
        let (authority, base_path) = assert_announced(&row, "CreateFunctionUrlConfig", &announced);

        // Every other reporter of the same function URL reports the one announced address.
        let fetched = control(
            &router,
            row.host,
            row.region,
            "GET",
            &format!("/2021-10-31/functions/{function}/url{query}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetFunctionUrlConfig must answer 200, got {}",
            fetched.summary()
        );
        assert_eq!(
            announced_url(case, "GetFunctionUrlConfig", &fetched),
            announced,
            "case {case:?}: GetFunctionUrlConfig must report the announced FunctionUrl, got {}",
            fetched.summary()
        );

        let listed = control(
            &router,
            row.host,
            row.region,
            "GET",
            &format!("/2021-10-31/functions/{function}/urls"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: ListFunctionUrlConfigs must answer 200, got {}",
            listed.summary()
        );
        let announced_in_list: Vec<String> = listed.json()["FunctionUrlConfigs"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: ListFunctionUrlConfigs served no configurations, got {}",
                    listed.summary()
                )
            })
            .iter()
            .map(|entry| {
                entry
                    .get("FunctionUrl")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            announced_in_list,
            vec![announced.clone()],
            "case {case:?}: ListFunctionUrlConfigs must list the announced FunctionUrl, got {}",
            listed.summary()
        );

        let updated = control(
            &router,
            row.host,
            row.region,
            "PUT",
            &format!("/2021-10-31/functions/{function}/url{query}"),
            &json!({ "AuthType": "NONE", "InvokeMode": "BUFFERED" }),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: UpdateFunctionUrlConfig must answer 200, got {}",
            updated.summary()
        );
        assert_eq!(
            announced_url(case, "UpdateFunctionUrlConfig", &updated),
            announced,
            "case {case:?}: an update never re-mints the announced FunctionUrl, got {}",
            updated.summary()
        );

        // The URL a client follows verbatim: the announced value itself, which addresses the
        // endpoint's own root, and a path below it.
        for (form, path, reached) in [
            (
                "the announced FunctionUrl followed verbatim",
                base_path.clone(),
                "/",
            ),
            (
                "a path below the announced FunctionUrl",
                format!("{base_path}orders/42"),
                "/orders/42",
            ),
        ] {
            assert_served(&router, case, form, &authority, &path, reached).await;
        }
    }
}
