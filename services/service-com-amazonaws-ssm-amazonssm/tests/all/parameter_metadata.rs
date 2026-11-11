// SPDX-License-Identifier: CC0-1.0

//! `DescribeParameters` end to end: `PutParameter` writes parameters, the metadata read
//! reports them — whichever identity of the account asks.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol"). Nothing reaches into the
//! state struct, so a response that is not computed from persisted state cannot pass:
//! every describe follows the writes that created the parameters.
//!
//! The suites are tables. Their rows differ along what the rules under test generalise
//! over — the signing identity and whether it carries a session token, the Region, the
//! parameter's type, key, tier, data type, tags and hierarchy depth, the filter key, its
//! comparison option and its values, the page size, and the spelling of a name —
//! rather than the single listing the recorded unit happened to ask for.

use std::collections::BTreeSet;

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

/// The access key id the base identity of the account signs with.
const BASE_ACCESS_KEY: &str = "AKIDBASEIDENTITY0001";

/// The access key id a temporary session of the same account signs with, which a
/// session also carries a token beside.
const SESSION_ACCESS_KEY: &str = "ASIASESSIONIDENTITY1";

/// The published bounds of `DescribeParameters`'s `MaxResults`
/// (`smithy_query(query="operation", operation="DescribeParameters")`: range 1..50).
const MIN_PAGE_SIZE: i64 = 1;

/// See [`MIN_PAGE_SIZE`].
const MAX_PAGE_SIZE: i64 = 50;

/// The key a `SecureString` is encrypted under when the put names none ("the system uses
/// the default key associated with your Amazon Web Services account",
/// `PutParameterRequest$KeyId`), which Parameter Store reaches through this alias.
const DEFAULT_KEY_ALIAS: &str = "alias/aws/ssm";

/// How many filters one describe carries across both of its filter members, which the
/// service bounds because each filter is compared against every parameter held
/// (`service_com_amazonaws_ssm_amazonssm::state::MAX_DESCRIBE_PARAMETERS_FILTERS`).
const MAX_FILTERS: usize =
    service_com_amazonaws_ssm_amazonssm::state::MAX_DESCRIBE_PARAMETERS_FILTERS;

/// [`MAX_FILTERS`] filters that between them match only `/Describe/tagged`: every
/// published filter key of a describe, in more than one of the comparisons it accepts.
fn the_most_filters_a_describe_carries() -> Vec<Value> {
    let filters = vec![
        json!({ "Key": "Name", "Values": ["/Describe/tagged"] }),
        json!({ "Key": "Name", "Option": "BeginsWith", "Values": ["/Describe"] }),
        json!({ "Key": "Name", "Option": "Contains", "Values": ["tagged"] }),
        json!({ "Key": "Type", "Values": ["String"] }),
        json!({ "Key": "Type", "Option": "BeginsWith", "Values": ["Str"] }),
        json!({ "Key": "Tier", "Values": ["Standard"] }),
        json!({ "Key": "DataType", "Values": ["text"] }),
        json!({ "Key": "Path", "Option": "OneLevel", "Values": ["/Describe"] }),
        json!({ "Key": "tag:Environment", "Values": ["Dev"] }),
        json!({ "Key": "tag:Environment", "Option": "BeginsWith", "Values": ["D"] }),
    ];
    assert_eq!(
        filters.len(),
        MAX_FILTERS,
        "the table must carry exactly the {MAX_FILTERS} filters a describe allows"
    );
    filters
}

/// A router with this service and KMS mounted on one shared engine (the crate's shared
/// harness, `all/parameter_store.rs`).
///
/// KMS is there because a `SecureString`'s value is sealed under the key its put names,
/// and the seeded hierarchy holds one that names a customer managed key.
fn router() -> MultiServiceRouter {
    super::parameter_store::router_with_kms()
}

/// Call one operation signed for `region` with `access_key`, carrying a session token
/// when `session` is set, returning `(status, body)`.
async fn call_as(
    router: &MultiServiceRouter,
    region: &str,
    access_key: &str,
    session: Option<&str>,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key}/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        );
    if let Some(token) = session {
        request = request.header("x-amz-security-token", token);
    }
    let request = request
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

/// Call one operation as the base identity, signed for [`REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_as(router, REGION, BASE_ACCESS_KEY, None, operation, body).await
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

/// `DescribeParameters`, asserting it succeeded, and returning its whole response.
async fn describe_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = call(router, "DescribeParameters", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeParameters {body} must succeed, got {status} {response}"
    );
    response
}

/// The `Name` of every parameter a describe reported, in the order it reported them.
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

/// The metadata the describe reported for `name`.
fn reported(response: &Value, name: &str) -> Value {
    response["Parameters"]
        .as_array()
        .expect("the response carries a Parameters list")
        .iter()
        .find(|parameter| parameter["Name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("the describe must report {name}, got {response}"))
        .clone()
}

/// The `NextToken` a describe handed out, if any.
fn next_token(response: &Value) -> Option<String> {
    response
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The ARN the service must mint for `name` in `region`.
fn parameter_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:ssm:{region}:{ACCOUNT}:parameter/{}",
        name.strip_prefix('/').unwrap_or(name)
    )
}

/// The puts every case below describes: several hierarchy depths, both spellings of a
/// top-level name, every `ParameterType`, the default and a named `SecureString` key,
/// both tiers, two of the documented `DataType`s, a tagged parameter, a described one
/// with an `AllowedPattern`, and one written twice so a describe has a newer version to
/// report than the one the parameter was created with.
fn seed_puts() -> Vec<Value> {
    vec![
        json!({ "Name": "/Describe/Dev/db-port", "Type": "String", "Value": "3306" }),
        json!({
            "Name": "/Describe/Dev/db-port",
            "Type": "String",
            "Value": "5432",
            "Overwrite": true,
        }),
        json!({
            "Name": "/Describe/Dev/db-url",
            "Type": "String",
            "Value": "mysql://db",
            "Description": "the dev url",
            "AllowedPattern": "^[a-z:/]+$",
        }),
        json!({
            "Name": "/Describe/Dev/keyed",
            "Type": "SecureString",
            "Value": "k",
            "KeyId": "alias/custom-key",
        }),
        json!({ "Name": "/Describe/Dev/secret", "Type": "SecureString", "Value": "s3cr3t" }),
        json!({ "Name": "/Describe/Prod/db-url", "Type": "StringList", "Value": "a,b" }),
        json!({
            "Name": "/Describe/Prod/image",
            "Type": "String",
            "Value": "ami-0123456789abcdef0",
            "DataType": "aws:ec2:image",
        }),
        json!({
            "Name": "/Describe/advanced",
            "Type": "String",
            "Value": "big",
            "Tier": "Advanced",
            "Policies": "[{\"Type\":\"Expiration\",\"Version\":\"1.0\"}]",
        }),
        json!({
            "Name": "/Describe/tagged",
            "Type": "String",
            "Value": "t",
            "Tags": [{ "Key": "Environment", "Value": "Dev" }],
        }),
        json!({ "Name": "/RootLevel", "Type": "String", "Value": "root" }),
        json!({ "Name": "TopLevel", "Type": "String", "Value": "top" }),
    ]
}

/// Every seeded name AS A READ REPORTS IT, in the ascending name order the store keeps
/// them in (a canonical name drops the leading separator, so the parameter put as
/// `/RootLevel` is reported as `RootLevel` and sorts beside `TopLevel`, after the
/// `Describe` hierarchy).
const SEEDED_NAMES: &[&str] = &[
    "/Describe/Dev/db-port",
    "/Describe/Dev/db-url",
    "/Describe/Dev/keyed",
    "/Describe/Dev/secret",
    "/Describe/Prod/db-url",
    "/Describe/Prod/image",
    "/Describe/advanced",
    "/Describe/tagged",
    "RootLevel",
    "TopLevel",
];

/// Seed [`seed_puts`] as `access_key` in `region`.
async fn seed_as(router: &MultiServiceRouter, region: &str, access_key: &str) {
    // The customer managed key one of the seeds names, created through KMS's own
    // operations: a `SecureString` is sealed under the key its put names.
    super::parameter_store::kms_key_with_alias_in(router, region, "alias/custom-key").await;
    for body in seed_puts() {
        let (status, response) =
            call_as(router, region, access_key, None, "PutParameter", &body).await;
        assert_eq!(
            status, 200,
            "seeding {body} in {region} must succeed, got {status} {response}"
        );
    }
}

/// Seed [`seed_puts`] as the base identity in [`REGION`].
async fn seed(router: &MultiServiceRouter) {
    seed_as(router, REGION, BASE_ACCESS_KEY).await;
}

/// What a describe must report for one seeded parameter: its `Type`, the `KeyId` it is
/// encrypted under, the version it is reported at, its `Tier`, its `DataType`, its
/// `Description`, its `AllowedPattern`, and how many inline policies it carries.
type Metadata<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    i64,
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    usize,
);

/// The metadata each seeded put persisted.
fn expected_metadata() -> Vec<Metadata<'static>> {
    vec![
        (
            "/Describe/Dev/db-port",
            "String",
            None,
            // Written twice: a describe reports a parameter at its newest version.
            2,
            "Standard",
            "text",
            None,
            None,
            0,
        ),
        (
            "/Describe/Dev/db-url",
            "String",
            None,
            1,
            "Standard",
            "text",
            Some("the dev url"),
            Some("^[a-z:/]+$"),
            0,
        ),
        (
            "/Describe/Dev/keyed",
            "SecureString",
            Some("alias/custom-key"),
            1,
            "Standard",
            "text",
            None,
            None,
            0,
        ),
        (
            "/Describe/Dev/secret",
            "SecureString",
            Some(DEFAULT_KEY_ALIAS),
            1,
            "Standard",
            "text",
            None,
            None,
            0,
        ),
        (
            "/Describe/Prod/db-url",
            "StringList",
            None,
            1,
            "Standard",
            "text",
            None,
            None,
            0,
        ),
        (
            "/Describe/Prod/image",
            "String",
            None,
            1,
            "Standard",
            "aws:ec2:image",
            None,
            None,
            0,
        ),
        (
            "/Describe/advanced",
            "String",
            None,
            1,
            "Advanced",
            "text",
            None,
            None,
            1,
        ),
        (
            "/Describe/tagged",
            "String",
            None,
            1,
            "Standard",
            "text",
            None,
            None,
            0,
        ),
        (
            "RootLevel",
            "String",
            None,
            1,
            "Standard",
            "text",
            None,
            None,
            0,
        ),
        (
            "TopLevel",
            "String",
            None,
            1,
            "Standard",
            "text",
            None,
            None,
            0,
        ),
    ]
}

/// A describe reports every parameter the signed Region holds, in ascending name order,
/// each at its newest version and carrying the metadata the put persisted.
#[tokio::test]
async fn a_describe_reports_the_metadata_of_every_parameter_the_region_holds() {
    let router = router();
    seed(&router).await;

    let response = describe_ok(&router, "every parameter", &json!({})).await;
    assert_eq!(
        reported_names(&response),
        SEEDED_NAMES,
        "every seeded parameter must be reported in ascending name order, got {response}"
    );
    assert!(
        next_token(&response).is_none(),
        "a listing shorter than the page size must hand out no token, got {response}"
    );

    for (name, type_, key_id, version, tier, data_type, description, pattern, policies) in
        expected_metadata()
    {
        let parameter = reported(&response, name);
        assert_eq!(
            parameter["ARN"],
            json!(parameter_arn(REGION, name)),
            "case {name:?}: the reported ARN must name the signed Region, got {parameter}"
        );
        assert_eq!(
            parameter["Type"],
            json!(type_),
            "case {name:?}: expected Type {type_}, got {parameter}"
        );
        assert_eq!(
            parameter["KeyId"].as_str(),
            key_id,
            "case {name:?}: expected KeyId {key_id:?}, got {parameter}"
        );
        assert_eq!(
            parameter["Version"],
            json!(version),
            "case {name:?}: expected Version {version}, got {parameter}"
        );
        assert_eq!(
            parameter["Tier"],
            json!(tier),
            "case {name:?}: expected Tier {tier}, got {parameter}"
        );
        assert_eq!(
            parameter["DataType"],
            json!(data_type),
            "case {name:?}: expected DataType {data_type}, got {parameter}"
        );
        assert_eq!(
            parameter["Description"].as_str(),
            description,
            "case {name:?}: expected Description {description:?}, got {parameter}"
        );
        assert_eq!(
            parameter["AllowedPattern"].as_str(),
            pattern,
            "case {name:?}: expected AllowedPattern {pattern:?}, got {parameter}"
        );
        assert_eq!(
            parameter["Policies"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            policies,
            "case {name:?}: expected {policies} inline policies, got {parameter}"
        );
        assert!(
            parameter["LastModifiedDate"].is_number(),
            "case {name:?}: the write's instant must be reported, got {parameter}"
        );
        // `ParameterMetadata` models no `Value`: a metadata listing reports what the
        // parameter is, never what it holds.
        assert!(
            parameter.get("Value").is_none(),
            "case {name:?}: a metadata listing must report no value, got {parameter}"
        );
    }
}

/// The store is keyed by Region and name, never by the credential a request was signed
/// with: a parameter one identity of the account wrote is described for every other
/// identity of it, and only in the Region it was written in.
#[tokio::test]
async fn every_identity_in_the_account_describes_the_same_parameters() {
    let router = router();
    // The base identity writes; a temporary session of the same account reads.
    seed(&router).await;

    let baseline = describe_ok(&router, "the writer's own listing", &json!({})).await;

    // (case, the signed Region, the access key id, the session token it carries)
    let readers: Vec<(&str, &str, &str, Option<&str>)> = vec![
        ("the identity that wrote them", REGION, BASE_ACCESS_KEY, None),
        (
            "another long-term key of the same account",
            REGION,
            "AKIDOTHERIDENTITY002",
            None,
        ),
        (
            "a temporary session of the same account",
            REGION,
            SESSION_ACCESS_KEY,
            Some("a-session-token"),
        ),
    ];

    for (case, region, access_key, session) in readers {
        let (status, response) = call_as(
            &router,
            region,
            access_key,
            session,
            "DescribeParameters",
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the describe must succeed, got {status} {response}"
        );
        assert_eq!(
            response, baseline,
            "case {case:?}: must describe the same account state, expected {baseline}, \
             got {response}"
        );
    }

    // The same session writes a parameter of its own: the base identity reads it back,
    // so the two halves of one account's state are one store in both directions.
    let (status, response) = call_as(
        &router,
        REGION,
        SESSION_ACCESS_KEY,
        Some("a-session-token"),
        "PutParameter",
        &json!({ "Name": "/Describe/by-the-session", "Type": "String", "Value": "v" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the session writes\": the put must succeed, got {status} {response}"
    );
    let response = describe_ok(&router, "the base identity reads it back", &json!({})).await;
    assert!(
        reported_names(&response).contains(&"/Describe/by-the-session".to_string()),
        "case \"the session writes\": the base identity must describe it, got {response}"
    );

    // A Region holds its own parameters only: the same credentials signed for another
    // Region describe none of these.
    let (status, response) = call_as(
        &router,
        "eu-west-1",
        BASE_ACCESS_KEY,
        None,
        "DescribeParameters",
        &json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"another Region\": the describe must succeed, got {status} {response}"
    );
    assert_eq!(
        reported_names(&response),
        Vec::<String>::new(),
        "case \"another Region\": a Region holding no parameter must report none, got {response}"
    );
}

/// Each filter narrows the listing to the parameters whose named attribute matches one
/// of its values under the comparison it asks for, and several filters narrow it to the
/// parameters satisfying all of them — whichever member spells them.
#[tokio::test]
async fn filters_narrow_the_listing_to_the_parameters_they_match() {
    let router = router();
    seed(&router).await;

    let every_describe = &SEEDED_NAMES[..8];

    // (case, the request body, the names reported, in order)
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "Name Equals one whole name",
            json!({ "ParameterFilters": [{ "Key": "Name", "Values": ["/Describe/Dev/secret"] }] }),
            vec!["/Describe/Dev/secret"],
        ),
        (
            // The leading separator is not part of a name's identity, so either
            // spelling asks for the same parameter.
            "Name Equals the same name without its leading separator",
            json!({ "ParameterFilters": [{ "Key": "Name", "Values": ["Describe/Dev/secret"] }] }),
            vec!["/Describe/Dev/secret"],
        ),
        (
            "Name Equals a top-level name spelled with a separator",
            json!({ "ParameterFilters": [{ "Key": "Name", "Values": ["/TopLevel"] }] }),
            vec!["TopLevel"],
        ),
        (
            "Name Equals two names",
            json!({
                "ParameterFilters": [{
                    "Key": "Name",
                    "Values": ["/RootLevel", "/Describe/tagged"],
                }],
            }),
            vec!["/Describe/tagged", "RootLevel"],
        ),
        (
            "Name BeginsWith a hierarchy",
            json!({
                "ParameterFilters": [{
                    "Key": "Name",
                    "Option": "BeginsWith",
                    "Values": ["/Describe/Dev"],
                }],
            }),
            vec![
                "/Describe/Dev/db-port",
                "/Describe/Dev/db-url",
                "/Describe/Dev/keyed",
                "/Describe/Dev/secret",
            ],
        ),
        (
            "Name Contains a fragment of the name",
            json!({
                "ParameterFilters": [{
                    "Key": "Name",
                    "Option": "Contains",
                    "Values": ["db-url"],
                }],
            }),
            vec!["/Describe/Dev/db-url", "/Describe/Prod/db-url"],
        ),
        (
            "Type Equals SecureString",
            json!({ "ParameterFilters": [{ "Key": "Type", "Values": ["SecureString"] }] }),
            vec!["/Describe/Dev/keyed", "/Describe/Dev/secret"],
        ),
        (
            "Type Equals StringList",
            json!({ "ParameterFilters": [{ "Key": "Type", "Values": ["StringList"] }] }),
            vec!["/Describe/Prod/db-url"],
        ),
        (
            "Type BeginsWith a prefix two types share",
            json!({
                "ParameterFilters": [{
                    "Key": "Type",
                    "Option": "BeginsWith",
                    "Values": ["Str"],
                }],
            }),
            vec![
                "/Describe/Dev/db-port",
                "/Describe/Dev/db-url",
                "/Describe/Prod/db-url",
                "/Describe/Prod/image",
                "/Describe/advanced",
                "/Describe/tagged",
                "RootLevel",
                "TopLevel",
            ],
        ),
        (
            "KeyId Equals a named key",
            json!({ "ParameterFilters": [{ "Key": "KeyId", "Values": ["alias/custom-key"] }] }),
            vec!["/Describe/Dev/keyed"],
        ),
        (
            "KeyId BeginsWith the default alias's prefix",
            json!({
                "ParameterFilters": [{
                    "Key": "KeyId",
                    "Option": "BeginsWith",
                    "Values": ["alias/aws"],
                }],
            }),
            vec!["/Describe/Dev/secret"],
        ),
        (
            // "For filters using the key Path, valid options include Recursive and
            // OneLevel" (ParameterStringFilter$Option), and a filter naming none asks
            // for the narrower reach a hierarchy read defaults to.
            "Path naming no option reaches one level",
            json!({ "ParameterFilters": [{ "Key": "Path", "Values": ["/Describe"] }] }),
            vec!["/Describe/advanced", "/Describe/tagged"],
        ),
        (
            "Path OneLevel",
            json!({
                "ParameterFilters": [{
                    "Key": "Path",
                    "Option": "OneLevel",
                    "Values": ["/Describe/Dev"],
                }],
            }),
            vec![
                "/Describe/Dev/db-port",
                "/Describe/Dev/db-url",
                "/Describe/Dev/keyed",
                "/Describe/Dev/secret",
            ],
        ),
        (
            "Path Recursive",
            json!({
                "ParameterFilters": [{
                    "Key": "Path",
                    "Option": "Recursive",
                    "Values": ["/Describe"],
                }],
            }),
            every_describe.to_vec(),
        ),
        (
            "Path OneLevel at the root, however a top-level name is spelled",
            json!({
                "ParameterFilters": [{ "Key": "Path", "Option": "OneLevel", "Values": ["/"] }],
            }),
            vec!["RootLevel", "TopLevel"],
        ),
        (
            "Tier Equals Advanced",
            json!({ "ParameterFilters": [{ "Key": "Tier", "Values": ["Advanced"] }] }),
            vec!["/Describe/advanced"],
        ),
        (
            "DataType Equals a documented non-default data type",
            json!({ "ParameterFilters": [{ "Key": "DataType", "Values": ["aws:ec2:image"] }] }),
            vec!["/Describe/Prod/image"],
        ),
        (
            "DataType Equals the default data type",
            json!({ "ParameterFilters": [{ "Key": "DataType", "Values": ["text"] }] }),
            vec![
                "/Describe/Dev/db-port",
                "/Describe/Dev/db-url",
                "/Describe/Dev/keyed",
                "/Describe/Dev/secret",
                "/Describe/Prod/db-url",
                "/Describe/advanced",
                "/Describe/tagged",
                "RootLevel",
                "TopLevel",
            ],
        ),
        (
            "a tag Equals the value it was created with",
            json!({ "ParameterFilters": [{ "Key": "tag:Environment", "Values": ["Dev"] }] }),
            vec!["/Describe/tagged"],
        ),
        (
            "a tag BeginsWith a prefix of its value",
            json!({
                "ParameterFilters": [{
                    "Key": "tag:Environment",
                    "Option": "BeginsWith",
                    "Values": ["D"],
                }],
            }),
            vec!["/Describe/tagged"],
        ),
        (
            "a tag nobody set",
            json!({ "ParameterFilters": [{ "Key": "tag:Missing", "Values": ["Dev"] }] }),
            vec![],
        ),
        (
            "a name nobody created",
            json!({ "ParameterFilters": [{ "Key": "Name", "Values": ["/Describe/nothing"] }] }),
            vec![],
        ),
        (
            "two filters narrow to the parameters satisfying both",
            json!({
                "ParameterFilters": [
                    { "Key": "Path", "Option": "Recursive", "Values": ["/Describe"] },
                    { "Key": "Type", "Values": ["SecureString"] },
                ],
            }),
            vec!["/Describe/Dev/keyed", "/Describe/Dev/secret"],
        ),
        (
            "the most filters a describe carries",
            json!({ "ParameterFilters": the_most_filters_a_describe_carries() }),
            vec!["/Describe/tagged"],
        ),
        (
            // "This data type is deprecated. Instead, use ParameterStringFilter": the
            // deprecated member carries no option, so it compares whole values.
            "the deprecated Filters member on Name",
            json!({ "Filters": [{ "Key": "Name", "Values": ["/Describe/Dev/secret"] }] }),
            vec!["/Describe/Dev/secret"],
        ),
        (
            "the deprecated Filters member on Type",
            json!({ "Filters": [{ "Key": "Type", "Values": ["StringList"] }] }),
            vec!["/Describe/Prod/db-url"],
        ),
        (
            "the deprecated Filters member on KeyId",
            json!({ "Filters": [{ "Key": "KeyId", "Values": ["alias/custom-key"] }] }),
            vec!["/Describe/Dev/keyed"],
        ),
        (
            "both filter members at once",
            json!({
                "Filters": [{ "Key": "Type", "Values": ["SecureString"] }],
                "ParameterFilters": [{
                    "Key": "Name",
                    "Option": "BeginsWith",
                    "Values": ["/Describe/Dev/k"],
                }],
            }),
            vec!["/Describe/Dev/keyed"],
        ),
    ];

    for (case, body, expected) in cases {
        let response = describe_ok(&router, case, &body).await;
        assert_eq!(
            reported_names(&response),
            expected,
            "case {case:?}: filtering with {body} must report {expected:?}, got {response}"
        );
    }
}

/// Paging walks the whole listing exactly once, whatever page size the caller asks for,
/// and the token each page hands out resumes the next one.
#[tokio::test]
async fn paging_walks_every_parameter_exactly_once() {
    let router = router();
    seed(&router).await;

    // (case, MaxResults, the filters to narrow by, the names the walk must cover)
    let cases: Vec<(&str, i64, Value, Vec<&str>)> = vec![
        (
            "the published minimum page size",
            MIN_PAGE_SIZE,
            Value::Null,
            SEEDED_NAMES.to_vec(),
        ),
        ("a page size the listing exceeds", 3, Value::Null, SEEDED_NAMES.to_vec()),
        (
            "a page size that holds the whole listing",
            SEEDED_NAMES.len() as i64,
            Value::Null,
            SEEDED_NAMES.to_vec(),
        ),
        (
            "the published maximum page size",
            MAX_PAGE_SIZE,
            Value::Null,
            SEEDED_NAMES.to_vec(),
        ),
        (
            "a page size that does not divide the listing",
            7,
            Value::Null,
            SEEDED_NAMES.to_vec(),
        ),
        (
            "one page at a time through a filter",
            MIN_PAGE_SIZE,
            json!([{ "Key": "Path", "Option": "Recursive", "Values": ["/Describe"] }]),
            SEEDED_NAMES[..8].to_vec(),
        ),
    ];

    for (case, page_size, filters, expected) in cases {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One request per page plus a guard: a token that never advanced would loop.
        for _ in 0..=expected.len() + 1 {
            let mut body = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            if !filters.is_null() {
                body["ParameterFilters"] = filters.clone();
            }
            let response = describe_ok(&router, case, &body).await;
            let page = reported_names(&response);
            assert!(
                page.len() <= page_size as usize,
                "case {case:?}: a page must hold at most {page_size} parameters, got {response}"
            );
            walked.extend(page);
            token = next_token(&response);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end with no token, got {token:?}"
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: paging at {page_size} must walk {expected:?} once, got {walked:?}"
        );
        let unique: BTreeSet<&String> = walked.iter().collect();
        assert_eq!(
            unique.len(),
            walked.len(),
            "case {case:?}: paging must report no parameter twice, got {walked:?}"
        );
    }
}

/// `Shared` asks for the parameters shared with the account rather than the ones it
/// holds; nothing shares a parameter in this emulator, so that listing is empty while
/// the account's own listing is not.
#[tokio::test]
async fn a_shared_listing_reports_the_parameters_shared_with_the_account() {
    let router = router();
    seed(&router).await;

    // (case, Shared, the names reported)
    let cases: Vec<(&str, Option<bool>, Vec<&str>)> = vec![
        ("Shared defaults to false", None, SEEDED_NAMES.to_vec()),
        ("the account's own parameters", Some(false), SEEDED_NAMES.to_vec()),
        ("the parameters shared with it", Some(true), vec![]),
    ];

    for (case, shared, expected) in cases {
        let mut body = json!({});
        if let Some(shared) = shared {
            body["Shared"] = json!(shared);
        }
        let response = describe_ok(&router, case, &body).await;
        assert_eq!(
            reported_names(&response),
            expected,
            "case {case:?}: {body} must report {expected:?}, got {response}"
        );
    }
}

/// A describe changes nothing a later caller sees: the listing, the versions and the
/// values a read reports are the same before and after any number of describes.
#[tokio::test]
async fn a_describe_changes_nothing_a_later_caller_sees() {
    let router = router();
    seed(&router).await;

    let first = describe_ok(&router, "the first listing", &json!({})).await;
    let (status, before) = call(
        &router,
        "GetParameter",
        &json!({ "Name": "/Describe/Dev/db-port" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the read before the describes must succeed, got {status} {before}"
    );

    // Every shape of describe the suites above exercise, run again: a filtered one, a
    // paged one, a shared one and a bare one.
    let repeats: Vec<(&str, Value)> = vec![
        ("a bare describe", json!({})),
        (
            "a filtered describe",
            json!({ "ParameterFilters": [{ "Key": "Type", "Values": ["SecureString"] }] }),
        ),
        ("a paged describe", json!({ "MaxResults": MIN_PAGE_SIZE })),
        ("a shared describe", json!({ "Shared": true })),
    ];
    for (case, body) in repeats {
        describe_ok(&router, case, &body).await;
    }

    let again = describe_ok(&router, "the listing after the describes", &json!({})).await;
    assert_eq!(
        again, first,
        "the listing must be unchanged by reading it, expected {first}, got {again}"
    );
    let (status, after) = call(
        &router,
        "GetParameter",
        &json!({ "Name": "/Describe/Dev/db-port" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the read after the describes must succeed, got {status} {after}"
    );
    assert_eq!(
        after, before,
        "a read must report the same parameter after a describe, expected {before}, got {after}"
    );
}

/// A request the service refuses is refused with the error its model names for that
/// rule, and never with the scaffold's 501.
#[tokio::test]
async fn a_refused_describe_reports_the_modelled_error() {
    let router = router();
    seed(&router).await;

    // (case, the request body, the wire error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            // "For DescribeParameters, all of the listed patterns are valid except
            // Label" (ParameterStringFilter$Key).
            "a Label filter key",
            json!({ "ParameterFilters": [{ "Key": "Label", "Values": ["release"] }] }),
            "InvalidFilterKey",
        ),
        (
            // "For all filters used with DescribeParameters, valid options include
            // Equals and BeginsWith. The Name filter additionally supports the Contains
            // option" (ParameterStringFilter$Option).
            "Contains on a filter other than Name",
            json!({
                "ParameterFilters": [{
                    "Key": "Type",
                    "Option": "Contains",
                    "Values": ["String"],
                }],
            }),
            "InvalidFilterOption",
        ),
        (
            "a hierarchy option on a Name filter",
            json!({
                "ParameterFilters": [{
                    "Key": "Name",
                    "Option": "Recursive",
                    "Values": ["/Describe"],
                }],
            }),
            "InvalidFilterOption",
        ),
        (
            // "(Exception: For filters using the key Path, valid options include
            // Recursive and OneLevel.)" (the same doc).
            "BeginsWith on a Path filter",
            json!({
                "ParameterFilters": [{
                    "Key": "Path",
                    "Option": "BeginsWith",
                    "Values": ["/Describe"],
                }],
            }),
            "InvalidFilterOption",
        ),
        (
            "an option no filter publishes",
            json!({
                "ParameterFilters": [{
                    "Key": "Tier",
                    "Option": "OneLevel",
                    "Values": ["Standard"],
                }],
            }),
            "InvalidFilterOption",
        ),
        (
            "a Type equality no ParameterType can satisfy",
            json!({
                "ParameterFilters": [{ "Key": "Type", "Option": "Equals", "Values": ["Secret"] }],
            }),
            "InvalidFilterValue",
        ),
        (
            "a Tier equality no ParameterTier can satisfy",
            json!({ "ParameterFilters": [{ "Key": "Tier", "Values": ["Premium"] }] }),
            "InvalidFilterValue",
        ),
        (
            "a deprecated Type filter no ParameterType can satisfy",
            json!({ "Filters": [{ "Key": "Type", "Values": ["Secret"] }] }),
            "InvalidFilterValue",
        ),
        (
            "a Path value that starts with no forward slash",
            json!({ "ParameterFilters": [{ "Key": "Path", "Values": ["Describe"] }] }),
            "InvalidFilterValue",
        ),
        (
            "a Path value with spaces between characters",
            json!({ "ParameterFilters": [{ "Key": "Path", "Values": ["/Desc ribe"] }] }),
            "InvalidFilterValue",
        ),
        (
            "a filter carrying no value to search for",
            json!({ "ParameterFilters": [{ "Key": "Name" }] }),
            "InvalidFilterValue",
        ),
        (
            "a token this service never issued",
            json!({ "NextToken": "!!not-a-token!!" }),
            "InvalidNextToken",
        ),
        (
            "a token that is an ARN rather than a name",
            json!({ "NextToken": "arn:aws:ssm:us-west-2:123456789012:parameter/Describe/tagged" }),
            "InvalidNextToken",
        ),
        (
            "one filter more than a describe carries",
            json!({
                "ParameterFilters": the_most_filters_a_describe_carries(),
                "Filters": [{ "Key": "Type", "Values": ["String"] }],
            }),
            "ValidationException",
        ),
        (
            "MaxResults one below the published minimum",
            json!({ "MaxResults": MIN_PAGE_SIZE - 1 }),
            "ValidationException",
        ),
        (
            "MaxResults one above the published maximum",
            json!({ "MaxResults": MAX_PAGE_SIZE + 1 }),
            "ValidationException",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, response) = call(&router, "DescribeParameters", &body).await;
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
