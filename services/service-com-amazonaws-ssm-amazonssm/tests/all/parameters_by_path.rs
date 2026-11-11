// SPDX-License-Identifier: CC0-1.0

//! `GetParametersByPath` end to end: `PutParameter` writes a hierarchy, the path read
//! reports the parameters under one level of it.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol"). Nothing reaches into the
//! state struct, so a response that is not computed from persisted state cannot pass:
//! every read follows the writes that created the hierarchy.
//!
//! The suites are tables. Their rows differ along what the rules under test generalise
//! over — hierarchy depth, the level a path names, recursion, the spelling of a
//! top-level name, page size, the filter key, its comparison option and its values, and
//! the signed Region — rather than the single path the recorded unit happened to read.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise. Deliberately not
/// the emulator's default, so an ARN carrying it proves the Region came from the
/// request's SigV4 credential scope.
const REGION: &str = "us-west-2";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// The `ParameterType` values the model's enum declares
/// (`smithy_query(query="shape", shape="ParameterType")`).
const PARAMETER_TYPES: &[&str] = &["String", "StringList", "SecureString"];

/// The published bounds of `GetParametersByPathMaxResults`
/// (`smithy_query(query="shape", shape="GetParametersByPathMaxResults")`: range 1..10).
const MIN_PAGE_SIZE: i64 = 1;

/// See [`MIN_PAGE_SIZE`].
const MAX_PAGE_SIZE: i64 = 10;

/// The key a `SecureString` is encrypted under when the put names none ("the system uses
/// the default key associated with your Amazon Web Services account",
/// `PutParameterRequest$KeyId`), which Parameter Store reaches through this alias.
const DEFAULT_KEY_ALIAS: &str = "alias/aws/ssm";

/// A router with this service and KMS mounted on one shared engine (the crate's shared
/// harness, `all/parameter_store.rs`).
///
/// KMS is there because a `SecureString`'s value is sealed under the key its put names:
/// the hierarchy below holds `SecureString` parameters, and the one that names a customer
/// managed key names a key KMS created.
fn router() -> MultiServiceRouter {
    super::parameter_store::router_with_kms()
}

/// Call one operation signed for `region`, returning `(status, body)`.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // The error shape travels in `__type`; fold in the header the framework also sets
    // so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Call one operation signed for [`REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// `PutParameter`, asserting it succeeded.
async fn put_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = call(router, "PutParameter", body).await;
    assert_eq!(
        status, 200,
        "case {case}: PutParameter must succeed, got {status} {response}"
    );
    response
}

/// `GetParametersByPath` signed for `region`.
async fn by_path_in_region(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in_region(router, region, "GetParametersByPath", body).await
}

/// `GetParametersByPath`, asserting it succeeded, and returning its whole response.
async fn by_path_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = by_path_in_region(router, REGION, body).await;
    assert_eq!(
        status, 200,
        "case {case}: GetParametersByPath {body} must succeed, got {status} {response}"
    );
    response
}

/// The `Name` of every parameter a path read reported, in the order it reported them.
fn reported_names(response: &Value) -> Vec<String> {
    response["Parameters"]
        .as_array()
        .expect("the response carries a Parameters list")
        .iter()
        .map(|parameter| {
            parameter["Name"]
                .as_str()
                .expect("every reported parameter carries a Name")
                .to_string()
        })
        .collect()
}

/// The ARN the service must mint for `name` in `region`.
fn parameter_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:ssm:{region}:{ACCOUNT}:parameter/{}",
        name.strip_prefix('/').unwrap_or(name)
    )
}

/// One seeded parameter: its name, its `ParameterType`, its value and the `KeyId` to
/// name (only a `SecureString` may carry one).
type Seed<'a> = (&'a str, &'a str, &'a str, Option<&'a str>);

/// The hierarchy every path case below reads: several depths, two spellings of a
/// top-level name, a sibling whose name merely starts with another path's text
/// (`/DevOps` beside `/Dev`), and a parameter that is itself a path's whole text.
const HIERARCHY: &[Seed<'static>] = &[
    ("/Dev/DBServer/MySQL/db-url", "String", "mysql-url", None),
    ("/Dev/DBServer/MySQL/db-port", "String", "3306", None),
    ("/Dev/DBServer/Postgres/db-url", "String", "pg-url", None),
    ("/Dev/DBServer/connection", "String", "conn", None),
    ("/Dev/secret", "SecureString", "s3cr3t", None),
    ("/DevOps/tool", "String", "tool", None),
    ("/Prod/DBServer/db-url", "String", "prod-url", None),
    ("TopLevel", "String", "top", None),
    ("/RootLevel", "String", "root", None),
];

/// Seed `parameters` in `region`.
async fn seed_in_region(router: &MultiServiceRouter, region: &str, parameters: &[Seed<'_>]) {
    for (name, type_, value, key_id) in parameters {
        let mut body = json!({ "Name": name, "Type": type_, "Value": value });
        if let Some(key_id) = key_id {
            body["KeyId"] = json!(key_id);
        }
        let (status, response) = call_in_region(router, region, "PutParameter", &body).await;
        assert_eq!(
            status, 200,
            "seeding {name} in {region} must succeed, got {status} {response}"
        );
    }
}

/// Seed `parameters` in [`REGION`].
async fn seed(router: &MultiServiceRouter, parameters: &[Seed<'_>]) {
    seed_in_region(router, REGION, parameters).await;
}

/// A path read reports the parameters whose name carries the path as a whole prefix of
/// its levels: the level immediately below the path, or every level under it when
/// `Recursive` is set — and never the parameter stored AT the path.
#[tokio::test]
async fn a_path_read_reports_the_parameters_under_that_level() {
    let router = router();
    seed(&router, HIERARCHY).await;

    // (case, path, recursive, the names reported, in order)
    let cases: Vec<(&str, &str, bool, Vec<&str>)> = vec![
        ("one level below a path", "/Dev", false, vec!["/Dev/secret"]),
        (
            "every level below a path",
            "/Dev",
            true,
            vec![
                "/Dev/DBServer/MySQL/db-port",
                "/Dev/DBServer/MySQL/db-url",
                "/Dev/DBServer/Postgres/db-url",
                "/Dev/DBServer/connection",
                "/Dev/secret",
            ],
        ),
        (
            "one level below a deeper path",
            "/Dev/DBServer",
            false,
            vec!["/Dev/DBServer/connection"],
        ),
        (
            "every level below a deeper path",
            "/Dev/DBServer",
            true,
            vec![
                "/Dev/DBServer/MySQL/db-port",
                "/Dev/DBServer/MySQL/db-url",
                "/Dev/DBServer/Postgres/db-url",
                "/Dev/DBServer/connection",
            ],
        ),
        (
            "the deepest level",
            "/Dev/DBServer/MySQL",
            false,
            vec!["/Dev/DBServer/MySQL/db-port", "/Dev/DBServer/MySQL/db-url"],
        ),
        (
            "a trailing separator names the same level",
            "/Dev/DBServer/",
            false,
            vec!["/Dev/DBServer/connection"],
        ),
        (
            "surrounding spaces are removed from the path",
            "  /Dev/DBServer  ",
            false,
            vec!["/Dev/DBServer/connection"],
        ),
        (
            "the root's own level, however a top-level name is spelled",
            "/",
            false,
            // Both top-level parameters are reported without a leading separator,
            // whichever spelling their put used: the separator is not part of a
            // top-level name.
            vec!["RootLevel", "TopLevel"],
        ),
        (
            "every parameter under the root",
            "/",
            true,
            vec![
                "/Dev/DBServer/MySQL/db-port",
                "/Dev/DBServer/MySQL/db-url",
                "/Dev/DBServer/Postgres/db-url",
                "/Dev/DBServer/connection",
                "/Dev/secret",
                "/DevOps/tool",
                "/Prod/DBServer/db-url",
                "RootLevel",
                "TopLevel",
            ],
        ),
        (
            "a sibling that merely starts with the path's text is not under it",
            "/DevOps",
            true,
            vec!["/DevOps/tool"],
        ),
        (
            "the parameter at the path is not under the path",
            "/Dev/DBServer/connection",
            true,
            vec![],
        ),
        ("a hierarchy nobody created", "/Nothing", true, vec![]),
        (
            "Recursive defaults to false",
            "/Prod",
            false,
            // `/Prod/DBServer/db-url` is two levels below `/Prod`.
            vec![],
        ),
    ];

    for (case, path, recursive, expected) in cases {
        let mut body = json!({ "Path": path });
        if recursive {
            body["Recursive"] = json!(true);
        }
        let response = by_path_ok(&router, case, &body).await;
        assert_eq!(
            reported_names(&response),
            expected,
            "case {case:?}: reading {path} (recursive {recursive}) must report \
             {expected:?}, got {response}"
        );
        assert!(
            response.get("NextToken").is_none_or(Value::is_null),
            "case {case:?}: a page that exhausted the hierarchy carries no NextToken, \
             got {response}"
        );
    }
}

/// Each parameter a path read reports is exactly what the store holds: the same
/// `Parameter` the single-parameter read reports for that name, at its newest version.
#[tokio::test]
async fn each_reported_parameter_is_the_persisted_one() {
    let router = router();
    // The customer managed key one of the seeds names, created through KMS's own
    // operations: a `SecureString` is sealed under the key its put names.
    super::parameter_store::kms_key_with_alias(&router, "alias/custom-key").await;
    let seeds: &[Seed<'_>] = &[
        ("/Round/plain", "String", "plain-value", None),
        ("/Round/list", "StringList", "alpha,beta,gamma", None),
        ("/Round/secret", "SecureString", "s3cr3t", None),
        (
            "/Round/custom",
            "SecureString",
            "custom-secret",
            Some("alias/custom-key"),
        ),
        ("/Round/deep/leaf", "String", "leaf-value", None),
    ];
    for type_ in PARAMETER_TYPES {
        assert!(
            seeds.iter().any(|(_, seeded, _, _)| seeded == type_),
            "the round-trip table must cover ParameterType {type_}"
        );
    }
    seed(&router, seeds).await;

    // An overwrite appends a version, so the newest value is what a path read reports.
    let updated = put_ok(
        &router,
        "overwrite",
        &json!({
            "Name": "/Round/plain",
            "Type": "String",
            "Value": "second-value",
            "Overwrite": true,
        }),
    )
    .await;
    assert_eq!(
        updated["Version"],
        json!(2),
        "an overwrite creates version 2, got {updated}"
    );

    let response = by_path_ok(
        &router,
        "round-trip",
        &json!({ "Path": "/Round", "Recursive": true }),
    )
    .await;
    let reported = response["Parameters"]
        .as_array()
        .expect("the read reports a Parameters list")
        .clone();
    assert_eq!(
        reported.len(),
        seeds.len(),
        "the read must report every seeded parameter, got {response}"
    );

    for parameter in &reported {
        let name = parameter["Name"].as_str().expect("a reported Name");
        let case = format!("round-trip {name}");
        let (status, single) = call(&router, "GetParameter", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {case}: the reported name must read back, got {status} {single}"
        );
        assert_eq!(
            *parameter, single["Parameter"],
            "case {case}: the path read must report what the single read reports, got \
             {parameter} and {single}"
        );
        assert_eq!(
            parameter["ARN"],
            json!(parameter_arn(REGION, name)),
            "case {case}: the ARN carries the signed Region and the parameter path, got \
             {parameter}"
        );
        assert!(
            parameter["Selector"].is_null(),
            "case {case}: a read that named no selector reports none, got {parameter}"
        );
    }

    let plain = reported
        .iter()
        .find(|parameter| parameter["Name"] == json!("/Round/plain"))
        .expect("the overwritten parameter is reported");
    assert_eq!(
        (plain["Version"].clone(), plain["Value"].clone()),
        (json!(2), json!("second-value")),
        "the read reports the newest version of an overwritten parameter, got {plain}"
    );
}

/// Paging walks every parameter under the path exactly once, whatever page size the
/// request asks for, and the last page carries no token.
#[tokio::test]
async fn paging_walks_the_hierarchy_once() {
    let router = router();
    // More parameters than the largest page holds, so every page size needs several
    // round trips.
    let names: Vec<String> = (0..23).map(|index| format!("/Page/p{index:02}")).collect();
    let seeds: Vec<Seed<'_>> = names
        .iter()
        .map(|name| (name.as_str(), "String", "v", None))
        .collect();
    seed(&router, &seeds).await;

    // Both published bounds of `MaxResults`, a size in between, and a request that names
    // none (served up to the modelled maximum).
    let page_sizes: Vec<Option<i64>> = vec![
        Some(MIN_PAGE_SIZE),
        Some(3),
        Some(MAX_PAGE_SIZE),
        Some(7),
        None,
    ];
    for page_size in page_sizes {
        let case = format!("MaxResults={page_size:?}");
        let limit = page_size.unwrap_or(MAX_PAGE_SIZE) as usize;
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "Path": "/Page" });
            if let Some(page_size) = page_size {
                body["MaxResults"] = json!(page_size);
            }
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let response = by_path_ok(&router, &case, &body).await;
            let page = reported_names(&response);
            assert!(
                page.len() <= limit,
                "case {case}: a page holds at most {limit} parameters, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case}: paging must terminate, walked {walked:?}"
            );
            token = response
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, names,
            "case {case}: paging must walk every parameter once, in name order"
        );
    }
}

/// A filter selects on what the store holds for the key it names, under the comparison
/// option it carries.
#[tokio::test]
async fn filters_select_on_the_stored_parameter() {
    let router = router();
    super::parameter_store::kms_key_with_alias(&router, "alias/custom-key").await;
    seed(
        &router,
        &[
            ("/Filt/plain", "String", "plain", None),
            ("/Filt/list", "StringList", "a,b", None),
            ("/Filt/secret", "SecureString", "s3cr3t", None),
            (
                "/Filt/custom",
                "SecureString",
                "custom",
                Some("alias/custom-key"),
            ),
        ],
    )
    .await;

    // (case, the ParameterFilters to send, the names reported, in order)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "Type Equals String",
            json!([{ "Key": "Type", "Option": "Equals", "Values": ["String"] }]),
            vec!["/Filt/plain"],
        ),
        (
            "Type Equals StringList",
            json!([{ "Key": "Type", "Option": "Equals", "Values": ["StringList"] }]),
            vec!["/Filt/list"],
        ),
        (
            "Type Equals SecureString",
            json!([{ "Key": "Type", "Option": "Equals", "Values": ["SecureString"] }]),
            vec!["/Filt/custom", "/Filt/secret"],
        ),
        (
            "a filter naming no option compares whole values",
            json!([{ "Key": "Type", "Values": ["String"] }]),
            vec!["/Filt/plain"],
        ),
        (
            "two values of one filter are alternatives",
            json!([{ "Key": "Type", "Values": ["String", "StringList"] }]),
            vec!["/Filt/list", "/Filt/plain"],
        ),
        (
            "Type BeginsWith a whole type",
            json!([{ "Key": "Type", "Option": "BeginsWith", "Values": ["String"] }]),
            vec!["/Filt/list", "/Filt/plain"],
        ),
        (
            "Type BeginsWith a prefix every type carries",
            json!([{ "Key": "Type", "Option": "BeginsWith", "Values": ["S"] }]),
            vec![
                "/Filt/custom",
                "/Filt/list",
                "/Filt/plain",
                "/Filt/secret",
            ],
        ),
        (
            "KeyId Equals the default alias",
            json!([{ "Key": "KeyId", "Option": "Equals", "Values": [DEFAULT_KEY_ALIAS] }]),
            vec!["/Filt/secret"],
        ),
        (
            "KeyId Equals a custom key",
            json!([{ "Key": "KeyId", "Option": "Equals", "Values": ["alias/custom-key"] }]),
            vec!["/Filt/custom"],
        ),
        (
            "KeyId BeginsWith matches every encrypted parameter",
            json!([{ "Key": "KeyId", "Option": "BeginsWith", "Values": ["alias/"] }]),
            vec!["/Filt/custom", "/Filt/secret"],
        ),
        (
            "two filters must both match",
            json!([
                { "Key": "Type", "Option": "Equals", "Values": ["SecureString"] },
                { "Key": "KeyId", "Option": "BeginsWith", "Values": ["alias/aws"] },
            ]),
            vec!["/Filt/secret"],
        ),
        (
            "a filter nothing matches reports nothing",
            json!([{ "Key": "KeyId", "Option": "Equals", "Values": ["alias/absent"] }]),
            vec![],
        ),
        (
            // Nothing in this suite attaches a label, so no stored version carries one
            // and a label filter resolves against nothing invented.
            "Label Equals a label nothing created",
            json!([{ "Key": "Label", "Option": "Equals", "Values": ["release"] }]),
            vec![],
        ),
    ];

    for (case, filters, expected) in cases {
        let response = by_path_ok(
            &router,
            case,
            &json!({ "Path": "/Filt", "ParameterFilters": filters }),
        )
        .await;
        assert_eq!(
            reported_names(&response),
            expected,
            "case {case:?}: filtering with {filters} must report {expected:?}, got {response}"
        );
    }
}

/// A request the service refuses is refused with the error its model names for that
/// rule, and never with the scaffold's 501.
#[tokio::test]
async fn a_refused_read_reports_the_modelled_error() {
    let router = router();
    seed(&router, HIERARCHY).await;

    // (case, the request body, the wire error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a path that starts with no forward slash",
            json!({ "Path": "Dev" }),
            "ValidationException",
        ),
        (
            "a path with spaces between characters",
            json!({ "Path": "/Dev/DB Server" }),
            "ValidationException",
        ),
        (
            // "For GetParametersByPath, the following patterns listed for Key aren't
            // valid: tag, DataType, Name, Path, and Tier" (ParameterStringFilter$Key).
            "a Name filter key",
            json!({ "Path": "/Dev", "ParameterFilters": [{ "Key": "Name", "Values": ["x"] }] }),
            "InvalidFilterKey",
        ),
        (
            "a Path filter key",
            json!({ "Path": "/Dev", "ParameterFilters": [{ "Key": "Path", "Values": ["/Dev"] }] }),
            "InvalidFilterKey",
        ),
        (
            "a Tier filter key",
            json!({
                "Path": "/Dev",
                "ParameterFilters": [{ "Key": "Tier", "Values": ["Standard"] }],
            }),
            "InvalidFilterKey",
        ),
        (
            "a DataType filter key",
            json!({
                "Path": "/Dev",
                "ParameterFilters": [{ "Key": "DataType", "Values": ["text"] }],
            }),
            "InvalidFilterKey",
        ),
        (
            "a tag filter key",
            json!({
                "Path": "/Dev",
                "ParameterFilters": [{ "Key": "tag:Env", "Values": ["dev"] }],
            }),
            "InvalidFilterKey",
        ),
        (
            // "For filters used with GetParametersByPath, valid options include Equals
            // and BeginsWith" (ParameterStringFilter$Option).
            "a Contains option",
            json!({
                "Path": "/Dev",
                "ParameterFilters": [{ "Key": "Type", "Option": "Contains", "Values": ["String"] }],
            }),
            "InvalidFilterOption",
        ),
        (
            "a OneLevel option",
            json!({
                "Path": "/Dev",
                "ParameterFilters": [{ "Key": "Type", "Option": "OneLevel", "Values": ["String"] }],
            }),
            "InvalidFilterOption",
        ),
        (
            // "(Exception: For filters using Label as the Key name, the only valid
            // option is Equals.)" (the same doc).
            "BeginsWith on a Label filter",
            json!({
                "Path": "/Dev",
                "ParameterFilters": [{
                    "Key": "Label",
                    "Option": "BeginsWith",
                    "Values": ["rel"],
                }],
            }),
            "InvalidFilterOption",
        ),
        (
            "a Type equality no ParameterType can satisfy",
            json!({
                "Path": "/Dev",
                "ParameterFilters": [{ "Key": "Type", "Option": "Equals", "Values": ["Secret"] }],
            }),
            "InvalidFilterValue",
        ),
        (
            "a filter carrying no value to search for",
            json!({ "Path": "/Dev", "ParameterFilters": [{ "Key": "Type" }] }),
            "InvalidFilterValue",
        ),
        (
            "a token this service never issued",
            json!({ "Path": "/Dev", "NextToken": "!!not-a-token!!" }),
            "InvalidNextToken",
        ),
        (
            "a token that is an ARN rather than a name",
            json!({
                "Path": "/Dev",
                "NextToken": "arn:aws:ssm:us-west-2:123456789012:parameter/Dev/secret",
            }),
            "InvalidNextToken",
        ),
        (
            "MaxResults one below the published minimum",
            json!({ "Path": "/Dev", "MaxResults": MIN_PAGE_SIZE - 1 }),
            "ValidationException",
        ),
        (
            "MaxResults one above the published maximum",
            json!({ "Path": "/Dev", "MaxResults": MAX_PAGE_SIZE + 1 }),
            "ValidationException",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, response) = by_path_in_region(&router, REGION, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {body} must be refused with HTTP 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            expected,
            "case {case:?}: {body} must be refused with {expected}, got {response}"
        );
    }
}

/// A hierarchy read is scoped to the Region the request was signed for, and changes
/// nothing a later caller sees.
#[tokio::test]
async fn a_read_is_regional_and_reports_the_same_twice() {
    let router = router();
    seed_in_region(
        &router,
        "us-west-2",
        &[("/Scope/west", "String", "west-value", None)],
    )
    .await;
    seed_in_region(
        &router,
        "eu-west-1",
        &[("/Scope/east", "String", "east-value", None)],
    )
    .await;

    // (case, the signed Region, the names reported)
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("the Region a parameter was created in", "us-west-2", vec!["/Scope/west"]),
        ("another Region of the same account", "eu-west-1", vec!["/Scope/east"]),
        ("a Region holding none of them", "ap-south-1", vec![]),
    ];

    for (case, region, expected) in cases {
        let body = json!({ "Path": "/Scope", "Recursive": true });
        let first = by_path_in_region(&router, region, &body).await;
        assert_eq!(
            first.0, 200,
            "case {case:?}: the read must succeed in {region}, got {first:?}"
        );
        assert_eq!(
            reported_names(&first.1),
            expected,
            "case {case:?}: {region} must report {expected:?}, got {}",
            first.1
        );

        // Reading again must report exactly the same: a read persists nothing.
        let second = by_path_in_region(&router, region, &body).await;
        assert_eq!(
            second.1, first.1,
            "case {case:?}: a second read of {region} must report the same, got {} then {}",
            first.1, second.1
        );
    }
}
