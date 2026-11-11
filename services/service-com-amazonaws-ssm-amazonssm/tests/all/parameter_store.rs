// SPDX-License-Identifier: CC0-1.0

//! Parameter Store end to end: `PutParameter` writes, `GetParameter` reads back.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol"). Nothing reaches into the
//! state struct, so a response that is not computed from persisted state cannot pass:
//! every read follows a write.
//!
//! The suites are tables: each row is one input along the axis the rule under test
//! generalises over (name form and depth, parameter type, data type, value size, tier,
//! version count, selector form, Region), rather than the single parameter the recorded
//! unit happened to read.
//!
//! The front-door helpers below ([`router`], [`call`], [`call_in_region`],
//! [`error_code`]) are this crate's ONE test harness: `all/documents.rs` reaches the
//! same emulator through them rather than building a second router of its own.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise. Deliberately not
/// the emulator's default, so an ARN carrying it proves the Region came from the
/// request's SigV4 credential scope.
pub(crate) const REGION: &str = "us-west-2";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
pub(crate) const TARGET_PREFIX: &str = "AmazonSSM";

/// The `ParameterType` values the model's enum declares
/// (`smithy_query(query="shape", shape="ParameterType")`).
const PARAMETER_TYPES: &[&str] = &["String", "StringList", "SecureString"];

/// The `DataType` values `PutParameter` documents
/// (`smithy_query(query="doc", shape="PutParameterRequest", member="DataType")`:
/// "text aws:ec2:image aws:ssm:integration").
const DATA_TYPES: &[&str] = &["text", "aws:ec2:image", "aws:ssm:integration"];

/// The published content-size limits of the two tiers (`PutParameterRequest$Tier`:
/// standard 4 KB, advanced 8 KB).
const STANDARD_LIMIT: usize = 4 * 1024;

/// See [`STANDARD_LIMIT`].
const ADVANCED_LIMIT: usize = 8 * 1024;

/// The published hierarchy-depth limit (`PutParameterRequest$Name`: "Parameter
/// hierarchies are limited to a maximum depth of fifteen levels").
const MAX_LEVELS: usize = 15;

/// The retained-version count (`ParameterMaxVersionLimitExceeded`: "Parameter Store
/// retains the 100 most recently created versions of a parameter").
const MAX_VERSIONS: usize = 100;

/// A router with just this service mounted on a fresh engine.
pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The same router, beside the engine it is mounted on — for a suite that must seed a
/// record no operation of this service creates yet.
pub(crate) fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where a stored parameter's Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// A router with this service AND KMS mounted on one shared engine.
///
/// A `SecureString`'s value is sealed under the KMS key its put names, so a suite whose
/// parameters name a customer managed key needs that key to be one KMS holds — and the
/// key is created through KMS's own operations ([`kms_key_with_alias`]) rather than
/// planted in a store.
pub(crate) fn router_with_kms() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(engine),
    );
    router
}

/// Create a symmetric encryption key in [`REGION`] and point `alias` at it, through KMS's
/// own operations, returning the key id.
pub(crate) async fn kms_key_with_alias(router: &MultiServiceRouter, alias: &str) -> String {
    kms_key_with_alias_in(router, REGION, alias).await
}

/// [`kms_key_with_alias`] at `region`'s endpoint: a KMS key is regional, so a parameter
/// and the key it is sealed under live at the same endpoint. KMS is an awsJson1_1 service
/// reached by `X-Amz-Target: TrentService.<Operation>`.
pub(crate) async fn kms_key_with_alias_in(
    router: &MultiServiceRouter,
    region: &str,
    alias: &str,
) -> String {
    let created = kms(router, region, "CreateKey", &json!({ "Description": alias })).await;
    let key_id = created["KeyMetadata"]["KeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateKey reports a KeyId, got {created}"))
        .to_string();
    kms(
        router,
        region,
        "CreateAlias",
        &json!({ "AliasName": alias, "TargetKeyId": key_id }),
    )
    .await;
    key_id
}

/// One KMS call that must succeed, returning its parsed body.
async fn kms(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/kms/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "KMS {operation} {body} must succeed, got {status} with {parsed}"
    );
    parsed
}

/// Call one operation signed for `region`, returning `(status, body)`.
pub(crate) async fn call_in_region(
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
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// The wire error code of a failure body (`__type`, without its namespace).
pub(crate) fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// The message a refusal carries. A modelled error of this service renders it as
/// `Message` (the model's own member name) and the front-door `ValidationException` as
/// `message`, so one accessor reads either.
pub(crate) fn error_message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

async fn put(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PutParameter", body).await
}

/// `GetParameter` of `name`, asking for the value decrypted.
///
/// A `SecureString` is stored as the KMS ciphertext its put sealed, so a round-trip that
/// compares what a read reports with what the put supplied asks for the decrypted value;
/// the flag "is ignored for String and StringList parameter types". What the undecrypted
/// read reports is `all/secure_string_decryption.rs`'s subject.
async fn get(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(
        router,
        "GetParameter",
        &json!({ "Name": name, "WithDecryption": true }),
    )
    .await
}

/// `PutParameter`, asserting it succeeded, and returning its response.
async fn put_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = put(router, body).await;
    assert_eq!(
        status, 200,
        "case {case}: PutParameter must succeed, got {status} {response}"
    );
    response
}

/// `GetParameter`'s `Parameter`, asserting the read succeeded.
async fn get_ok(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, response) = get(router, name).await;
    assert_eq!(
        status, 200,
        "case {case}: GetParameter {name} must succeed, got {status} {response}"
    );
    response["Parameter"].clone()
}

/// The ARN the service must mint for `name` in `region`.
fn parameter_arn(region: &str, name: &str) -> String {
    format!(
        "arn:aws:ssm:{region}:{ACCOUNT}:parameter/{}",
        name.strip_prefix('/').unwrap_or(name)
    )
}

/// A hierarchical name `levels` levels deep.
fn name_of_depth(levels: usize) -> String {
    (1..=levels).map(|level| format!("/level{level}")).collect()
}

/// One round-trip case: the name, the `ParameterType`, the value, and the `DataType`
/// to ask for (`None` leaves it to the service's `text` default).
type RoundTrip<'a> = (&'a str, &'a str, &'a str, Option<&'a str>);

/// Every `ParameterType`, every documented `DataType` and the name forms Parameter
/// Store accepts round-trip: what a put persisted is what a later get reports.
#[tokio::test]
async fn put_then_get_reports_what_was_persisted() {
    let router = router();
    let deep = name_of_depth(MAX_LEVELS);
    let cases: Vec<RoundTrip<'_>> = vec![
        ("MyParameter", "String", "flat-value", None),
        (
            "/Dev/DBServer/MySQL/db-string13",
            "String",
            "connection-string",
            None,
        ),
        ("/Dev/List", "StringList", "alpha,beta,gamma", None),
        ("/Dev/Secret", "SecureString", "s3cr3t", None),
        (
            "/Dev/Image",
            "String",
            "ami-0123456789abcdef0",
            Some("aws:ec2:image"),
        ),
        (
            "/Dev/Integration",
            "String",
            "https://example.invalid/webhook",
            Some("aws:ssm:integration"),
        ),
        (
            "/Dev/Explicit_Text.v1-2",
            "String",
            "text-value",
            Some("text"),
        ),
        (&deep, "String", "deep-value", None),
    ];
    for value in PARAMETER_TYPES {
        assert!(
            cases.iter().any(|(_, type_, _, _)| type_ == value),
            "the round-trip table must cover ParameterType {value}"
        );
    }
    for value in DATA_TYPES {
        assert!(
            cases
                .iter()
                .any(|(_, _, _, data_type)| data_type.unwrap_or("text") == *value),
            "the round-trip table must cover DataType {value}"
        );
    }

    for (name, type_, value, data_type) in cases {
        let case = format!("{name} {type_} {}", data_type.unwrap_or("text"));
        let mut body = json!({ "Name": name, "Type": type_, "Value": value });
        if let Some(data_type) = data_type {
            body["DataType"] = json!(data_type);
        }
        let created = put_ok(&router, &case, &body).await;
        assert_eq!(
            created["Version"],
            json!(1),
            "case {case}: a first put creates version 1, got {created}"
        );

        let parameter = get_ok(&router, &case, name).await;
        assert_eq!(
            parameter["Name"],
            json!(name),
            "case {case}: the read reports the stored name, got {parameter}"
        );
        assert_eq!(
            parameter["Value"],
            json!(value),
            "case {case}: the read reports the stored value, got {parameter}"
        );
        assert_eq!(
            parameter["Type"],
            json!(type_),
            "case {case}: the read reports the stored type, got {parameter}"
        );
        assert_eq!(
            parameter["Version"],
            json!(1),
            "case {case}: the read reports the stored version, got {parameter}"
        );
        assert_eq!(
            parameter["DataType"],
            json!(data_type.unwrap_or("text")),
            "case {case}: the read reports the stored data type, got {parameter}"
        );
        assert_eq!(
            parameter["ARN"],
            json!(parameter_arn(REGION, name)),
            "case {case}: the ARN carries the signed Region and the parameter path, got {parameter}"
        );
        assert!(
            parameter["Selector"].is_null(),
            "case {case}: a read with no selector reports none, got {parameter}"
        );
        assert!(
            parameter["LastModifiedDate"]
                .as_f64()
                .is_some_and(|seconds| seconds > 0.0),
            "case {case}: the read reports when the version was created, got {parameter}"
        );
    }
}

/// A top-level parameter is the one parameter however a read spells it, and a full ARN
/// names it too.
#[tokio::test]
async fn a_read_resolves_a_name_an_alias_and_an_arn() {
    let router = router();
    put_ok(
        &router,
        "alias",
        &json!({ "Name": "AliasedParameter", "Type": "String", "Value": "aliased" }),
    )
    .await;
    put_ok(
        &router,
        "alias-hierarchical",
        &json!({ "Name": "/Deep/Aliased", "Type": "String", "Value": "deep-aliased" }),
    )
    .await;

    let arn = parameter_arn(REGION, "AliasedParameter");
    let deep_arn = parameter_arn(REGION, "/Deep/Aliased");
    let cases: Vec<(&str, &str, &str)> = vec![
        ("bare name", "AliasedParameter", "aliased"),
        ("leading separator", "/AliasedParameter", "aliased"),
        ("full ARN", arn.as_str(), "aliased"),
        ("hierarchical name", "/Deep/Aliased", "deep-aliased"),
        ("hierarchical ARN", deep_arn.as_str(), "deep-aliased"),
        (
            "name with surrounding spaces",
            "  AliasedParameter  ",
            "aliased",
        ),
    ];
    for (case, name, expected) in cases {
        let parameter = get_ok(&router, case, name).await;
        assert_eq!(
            parameter["Value"],
            json!(expected),
            "case {case}: reading {name} must report the stored value, got {parameter}"
        );
    }
}

/// An overwrite appends a version: every earlier value stays reachable through its own
/// `name:version` selector, and a read with no selector reports the newest.
#[tokio::test]
async fn overwriting_appends_a_version_each_selector_resolves() {
    let router = router();
    let name = "/Dev/Versioned";
    let values = ["first", "second", "third", "fourth"];

    for (index, value) in values.iter().enumerate() {
        let case = format!("put {}", index + 1);
        let mut body = json!({ "Name": name, "Type": "String", "Value": value });
        if index > 0 {
            body["Overwrite"] = json!(true);
        }
        let created = put_ok(&router, &case, &body).await;
        let expected = i64::try_from(index + 1).expect("the table is small");
        assert_eq!(
            created["Version"],
            json!(expected),
            "case {case}: each put creates the next version, got {created}"
        );
        let latest = get_ok(&router, &case, name).await;
        assert_eq!(
            latest["Value"],
            json!(value),
            "case {case}: a read with no selector reports the newest value, got {latest}"
        );
        assert_eq!(
            latest["Version"],
            json!(expected),
            "case {case}: a read with no selector reports the newest version, got {latest}"
        );
    }

    for (index, value) in values.iter().enumerate() {
        let version = index + 1;
        let case = format!("selector {version}");
        let selected = get_ok(&router, &case, &format!("{name}:{version}")).await;
        assert_eq!(
            selected["Value"],
            json!(value),
            "case {case}: a version selector reports that version's value, got {selected}"
        );
        assert_eq!(
            selected["Selector"],
            json!(format!(":{version}")),
            "case {case}: the read echoes the selector it resolved, separator and all, \
             got {selected}"
        );
    }

    // A selector naming a version or a label this parameter does not carry.
    for selector in ["0", "5", "99", "production"] {
        let (status, response) = get(&router, &format!("{name}:{selector}")).await;
        assert_eq!(
            status, 400,
            "case selector {selector:?}: an unknown selector is refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ParameterVersionNotFound",
            "case selector {selector:?}: expected ParameterVersionNotFound, got {response}"
        );
    }

    // A name ending in the selector separator names no selector at all.
    let (status, response) = get(&router, &format!("{name}:")).await;
    assert_eq!(
        status, 400,
        "case empty selector: must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case empty selector: expected ValidationException, got {response}"
    );

    // A put that does not ask to overwrite leaves the store exactly as it was.
    let (status, response) = put(
        &router,
        &json!({ "Name": name, "Type": "String", "Value": "rejected" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case duplicate: a put without Overwrite is refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ParameterAlreadyExists",
        "case duplicate: expected ParameterAlreadyExists, got {response}"
    );
    let latest = get_ok(&router, "duplicate", name).await;
    assert_eq!(
        latest["Value"],
        json!("fourth"),
        "case duplicate: the refused put stored nothing, got {latest}"
    );
}

/// Once a parameter holds the retained maximum of versions, a new version displaces the
/// oldest — which then resolves to nothing — while every newer one stays.
#[tokio::test]
async fn version_retention_displaces_the_oldest_version() {
    let router = router();
    let name = "/Dev/Retained";
    let puts = MAX_VERSIONS + 1;

    for version in 1..=puts {
        let mut body = json!({
            "Name": name,
            "Type": "String",
            "Value": format!("value-{version}"),
        });
        if version > 1 {
            body["Overwrite"] = json!(true);
        }
        put_ok(&router, &format!("put {version}"), &body).await;
    }

    let (status, response) = get(&router, &format!("{name}:1")).await;
    assert_eq!(
        status, 400,
        "case displaced: version 1 is no longer retained, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ParameterVersionNotFound",
        "case displaced: expected ParameterVersionNotFound, got {response}"
    );
    for version in [2, MAX_VERSIONS, puts] {
        let case = format!("retained {version}");
        let selected = get_ok(&router, &case, &format!("{name}:{version}")).await;
        assert_eq!(
            selected["Value"],
            json!(format!("value-{version}")),
            "case {case}: that version is still stored, got {selected}"
        );
    }
    let latest = get_ok(&router, "latest", name).await;
    assert_eq!(
        latest["Version"],
        json!(i64::try_from(puts).expect("the count is small")),
        "case latest: the newest version is the last one put, got {latest}"
    );
}

/// A name this Region and account hold no parameter for is `ParameterNotFound`,
/// whatever shape the name takes.
#[tokio::test]
async fn reading_what_was_never_stored_is_parameter_not_found() {
    let router = router();
    put_ok(
        &router,
        "seed",
        &json!({ "Name": "/Dev/Stored", "Type": "String", "Value": "stored" }),
    )
    .await;

    let cases: Vec<(&str, &str)> = vec![
        ("flat name", "NeverStored"),
        ("hierarchical name", "/Dev/NeverStored"),
        ("sibling of a stored name", "/Dev/Stored2"),
        ("parent of a stored name", "/Dev"),
        (
            "a name in the AWS-owned hierarchy that AWS publishes nothing under",
            "/aws/service/ami-amazon-linux-latest/no-such-published-alias",
        ),
        (
            "ARN of another account",
            "arn:aws:ssm:us-west-2:111122223333:parameter/Dev/Stored",
        ),
        (
            "ARN of another Region",
            "arn:aws:ssm:eu-west-1:000000000000:parameter/Dev/Stored",
        ),
        (
            "ARN of another service",
            "arn:aws:secretsmanager:us-west-2:000000000000:secret/Dev/Stored",
        ),
        (
            "ARN of another resource type",
            "arn:aws:ssm:us-west-2:000000000000:document/Dev/Stored",
        ),
    ];
    for (case, name) in cases {
        let (status, response) = get(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case}: reading {name} must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ParameterNotFound",
            "case {case}: reading {name} must report ParameterNotFound, got {response}"
        );
    }
}

/// A parameter belongs to the Region it was created in: the same name in another Region
/// is another parameter, and each one's ARN carries its own Region.
#[tokio::test]
async fn parameters_are_unique_within_a_region() {
    let router = router();
    let name = "/Dev/Regional";
    let regions = [("us-west-2", "west-value"), ("eu-central-1", "central-value")];

    for (region, value) in regions {
        let (status, response) = call_in_region(
            &router,
            region,
            "PutParameter",
            &json!({ "Name": name, "Type": "String", "Value": value }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region}: PutParameter must succeed, got {status} {response}"
        );
        assert_eq!(
            response["Version"],
            json!(1),
            "case {region}: the name is free in this Region, so this is version 1, got {response}"
        );
    }
    for (region, value) in regions {
        let (status, response) =
            call_in_region(&router, region, "GetParameter", &json!({ "Name": name })).await;
        assert_eq!(
            status, 200,
            "case {region}: GetParameter must succeed, got {status} {response}"
        );
        let parameter = &response["Parameter"];
        assert_eq!(
            parameter["Value"],
            json!(value),
            "case {region}: the read reports that Region's value, got {parameter}"
        );
        assert_eq!(
            parameter["ARN"],
            json!(parameter_arn(region, name)),
            "case {region}: the ARN carries that Region, got {parameter}"
        );
    }

    let (status, response) = call_in_region(
        &router,
        "ap-southeast-2",
        "GetParameter",
        &json!({ "Name": name }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case ap-southeast-2: a Region holding no such parameter refuses, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ParameterNotFound",
        "case ap-southeast-2: expected ParameterNotFound, got {response}"
    );

    // An ARN naming another Region is not a way to reach that Region's parameter: a
    // parameter is resolved through the endpoint the request was signed for.
    let (status, response) = call_in_region(
        &router,
        "us-west-2",
        "GetParameter",
        &json!({ "Name": parameter_arn("eu-central-1", name) }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case cross-Region ARN: must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ParameterNotFound",
        "case cross-Region ARN: expected ParameterNotFound, got {response}"
    );
}

/// Every naming constraint `PutParameter` documents, and the error each one reports.
#[tokio::test]
async fn names_outside_the_naming_constraints_are_refused() {
    let router = router();
    let at_limit = name_of_depth(MAX_LEVELS);
    let past_limit = name_of_depth(MAX_LEVELS + 1);
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "reserved aws prefix",
            "/aws/reserved",
            "ParameterPatternMismatchException",
        ),
        (
            "reserved ssm prefix",
            "/ssm/reserved",
            "ParameterPatternMismatchException",
        ),
        (
            "reserved prefix, mixed case",
            "/AWS/reserved",
            "ParameterPatternMismatchException",
        ),
        (
            "hierarchical without a leading separator",
            "Dev/DBServer",
            "ParameterPatternMismatchException",
        ),
        (
            "empty hierarchy level",
            "/Dev//DBServer",
            "ParameterPatternMismatchException",
        ),
        (
            "trailing separator",
            "/Dev/",
            "ParameterPatternMismatchException",
        ),
        (
            "character outside the allowed set",
            "/Dev/DB%Server",
            "ParameterPatternMismatchException",
        ),
        (
            "an ARN instead of a name",
            "arn:aws:ssm:us-west-2:000000000000:parameter/Dev/Arn",
            "ParameterPatternMismatchException",
        ),
        (
            "spaces between characters",
            "/Dev/DB Server",
            "ValidationException",
        ),
        (
            past_limit.as_str(),
            past_limit.as_str(),
            "HierarchyLevelLimitExceededException",
        ),
    ];
    for (case, name, expected) in cases {
        let (status, response) = put(
            &router,
            &json!({ "Name": name, "Type": "String", "Value": "refused" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: putting {name} must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            expected,
            "case {case}: expected {expected}, got {response}"
        );
    }

    // The published bound itself is accepted, and so is a name of allowed symbols.
    for (case, name) in [
        ("at the depth limit", at_limit.as_str()),
        ("every allowed symbol", "/Dev_1/Server.2/db-3"),
    ] {
        put_ok(
            &router,
            case,
            &json!({ "Name": name, "Type": "String", "Value": "accepted" }),
        )
        .await;
        let parameter = get_ok(&router, case, name).await;
        assert_eq!(
            parameter["Value"],
            json!("accepted"),
            "case {case}: the accepted name round-trips, got {parameter}"
        );
    }

    // A read refuses the one name rule it can: the spaces rule the doc ties to
    // `ValidationException`. Every other malformed name is a parameter that does not
    // exist.
    let (status, response) = get(&router, "/Dev/DB Server").await;
    assert_eq!(
        status, 400,
        "case read with spaces: must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case read with spaces: expected ValidationException, got {response}"
    );
}

/// The content-size limit of each tier, at the published bound and one byte past it,
/// and how `Intelligent-Tiering` and an update pick the tier.
#[tokio::test]
async fn tier_rules_bound_the_content_size() {
    let router = router();
    /// One tier case: what it does, the `Tier` to ask for, the value size, and the tier
    /// the service must report — or `None` when the put must be refused.
    type TierCase<'a> = (&'a str, Option<&'a str>, usize, Option<&'a str>);
    let cases: Vec<TierCase<'_>> = vec![
        ("default tier at the limit", None, STANDARD_LIMIT, Some("Standard")),
        ("default tier past the limit", None, STANDARD_LIMIT + 1, None),
        (
            "standard at the limit",
            Some("Standard"),
            STANDARD_LIMIT,
            Some("Standard"),
        ),
        ("standard past the limit", Some("Standard"), STANDARD_LIMIT + 1, None),
        (
            "advanced past the standard limit",
            Some("Advanced"),
            STANDARD_LIMIT + 1,
            Some("Advanced"),
        ),
        (
            "advanced at its limit",
            Some("Advanced"),
            ADVANCED_LIMIT,
            Some("Advanced"),
        ),
        ("advanced past its limit", Some("Advanced"), ADVANCED_LIMIT + 1, None),
        (
            "intelligent tiering, small value",
            Some("Intelligent-Tiering"),
            16,
            Some("Standard"),
        ),
        (
            "intelligent tiering, large value",
            Some("Intelligent-Tiering"),
            STANDARD_LIMIT + 1,
            Some("Advanced"),
        ),
    ];

    for (index, (case, tier, size, expected)) in cases.iter().enumerate() {
        let name = format!("/Tier/case{index}");
        let value = "v".repeat(*size);
        let mut body = json!({ "Name": name, "Type": "String", "Value": value });
        if let Some(tier) = tier {
            body["Tier"] = json!(tier);
        }
        let (status, response) = put(&router, &body).await;
        match expected {
            Some(expected_tier) => {
                assert_eq!(
                    status, 200,
                    "case {case}: a {size}-byte value must be accepted, got {status} {response}"
                );
                assert_eq!(
                    response["Tier"],
                    json!(expected_tier),
                    "case {case}: expected the {expected_tier} tier, got {response}"
                );
                let parameter = get_ok(&router, case, &name).await;
                assert_eq!(
                    parameter["Value"].as_str().map(str::len),
                    Some(*size),
                    "case {case}: the whole value round-trips, got {} bytes",
                    parameter["Value"].as_str().map_or(0, str::len)
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case}: a {size}-byte value must be refused, got {status} {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "ValidationException",
                    "case {case}: expected ValidationException, got {response}"
                );
            }
        }
    }

    // An advanced parameter can't be reverted to the standard tier, but an update that
    // names no tier stays in the one it has.
    put_ok(
        &router,
        "raise to advanced",
        &json!({
            "Name": "/Tier/case0",
            "Type": "String",
            "Value": "raised",
            "Tier": "Advanced",
            "Overwrite": true,
        }),
    )
    .await;
    let (status, response) = put(
        &router,
        &json!({
            "Name": "/Tier/case0",
            "Type": "String",
            "Value": "reverted",
            "Tier": "Standard",
            "Overwrite": true,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case revert to standard: must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case revert to standard: expected ValidationException, got {response}"
    );
    let updated = put_ok(
        &router,
        "update without a tier",
        &json!({
            "Name": "/Tier/case0",
            "Type": "String",
            "Value": "v".repeat(STANDARD_LIMIT + 1),
            "Overwrite": true,
        }),
    )
    .await;
    assert_eq!(
        updated["Tier"],
        json!("Advanced"),
        "case update without a tier: the parameter stays advanced, got {updated}"
    );
    let parameter = get_ok(&router, "update without a tier", "/Tier/case0").await;
    assert_eq!(
        parameter["Value"].as_str().map(str::len),
        Some(STANDARD_LIMIT + 1),
        "case update without a tier: the whole value round-trips, got {} bytes",
        parameter["Value"].as_str().map_or(0, str::len)
    );
}

/// `AllowedPattern` is a regular expression the value must satisfy in full.
#[tokio::test]
async fn allowed_pattern_validates_the_value() {
    let router = router();
    /// One case: the pattern, the value, and whether the put must be accepted.
    type PatternCase<'a> = (&'a str, &'a str, bool);
    let cases: Vec<PatternCase<'_>> = vec![
        (r"^\d+$", "12345", true),
        (r"^\d+$", "12a45", false),
        (r"^\d+$", "", false),
        (r"^[a-z]+$", "abc", true),
        (r"^[a-z]+$", "ABC", false),
        (r"^(dev|prod)$", "prod", true),
        (r"^(dev|prod)$", "staging", false),
        // Unanchored in the request, but the value must still match in full.
        (r"[0-9]{3}", "123", true),
        (r"[0-9]{3}", "1234", false),
        (r"^ami-[0-9a-f]{17}$", "ami-0123456789abcdef0", true),
    ];
    for (index, (pattern, value, accepted)) in cases.iter().enumerate() {
        let case = format!("{pattern} against {value:?}");
        let name = format!("/Pattern/case{index}");
        let (status, response) = put(
            &router,
            &json!({
                "Name": name,
                "Type": "String",
                "Value": value,
                "AllowedPattern": pattern,
            }),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case}: the value satisfies the pattern, got {status} {response}"
            );
            let parameter = get_ok(&router, &case, &name).await;
            assert_eq!(
                parameter["Value"],
                json!(value),
                "case {case}: the accepted value round-trips, got {parameter}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: the value must be refused, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ParameterPatternMismatchException",
                "case {case}: expected ParameterPatternMismatchException, got {response}"
            );
            let (status, response) = get(&router, &name).await;
            assert_eq!(
                status, 400,
                "case {case}: the refused put stored nothing, got {status} {response}"
            );
        }
    }

    // A pattern that is not a regular expression at all is refused by the OTHER error:
    // the caller's own pattern is what is wrong, not the value it was matched against.
    let (status, response) = put(
        &router,
        &json!({
            "Name": "/Pattern/broken",
            "Type": "String",
            "Value": "anything",
            "AllowedPattern": "^([0-9+$",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case unparseable pattern: must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidAllowedPatternException",
        "case unparseable pattern: expected InvalidAllowedPatternException, got {response}"
    );

    // The content-size clamp dominates the pattern match: a value past the largest
    // limit any tier allows is refused for its size, before any pattern is matched
    // against it — this pattern would have accepted the value.
    let (status, response) = put(
        &router,
        &json!({
            "Name": "/Pattern/oversize",
            "Type": "String",
            "Value": "v".repeat(ADVANCED_LIMIT + 1),
            "AllowedPattern": "^v+$",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case oversize value: must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case oversize value: expected ValidationException for the size, got {response}"
    );
}

/// `Type`, `DataType`, `KeyId`, `Policies` and `Tags` are refused outside what
/// Parameter Store supports, and `Type` is required only when creating.
#[tokio::test]
async fn type_data_type_key_id_and_tag_rules() {
    // A `SecureString` names a KMS key and is sealed under it, so KMS is mounted beside
    // this service and the key the puts below name is one KMS created.
    let router = router_with_kms();
    kms_key_with_alias(&router, "alias/custom-key").await;

    let (status, response) = put(
        &router,
        &json!({ "Name": "/Types/missing", "Value": "no-type" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case create without Type: must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case create without Type: expected ValidationException, got {response}"
    );

    // An update inherits the stored type when it names none.
    put_ok(
        &router,
        "update inherits Type",
        &json!({ "Name": "/Types/secure", "Type": "SecureString", "Value": "first" }),
    )
    .await;
    put_ok(
        &router,
        "update inherits Type",
        &json!({
            "Name": "/Types/secure",
            "Value": "second",
            "KeyId": "alias/custom-key",
            "Overwrite": true,
        }),
    )
    .await;
    let parameter = get_ok(&router, "update inherits Type", "/Types/secure").await;
    assert_eq!(
        parameter["Type"],
        json!("SecureString"),
        "case update inherits Type: the stored type survives an update, got {parameter}"
    );
    assert_eq!(
        parameter["Value"],
        json!("second"),
        "case update inherits Type: the update is what a read reports, got {parameter}"
    );

    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "unsupported Type",
            json!({ "Name": "/Types/bad", "Type": "Json", "Value": "v" }),
            // `ParameterType` is a modelled enum, so the boundary walk refuses a value
            // outside its set before the handler sees it.
            "ValidationException",
        ),
        (
            "unsupported DataType",
            json!({ "Name": "/Types/bad", "Type": "String", "Value": "v", "DataType": "text/plain" }),
            "ValidationException",
        ),
        (
            "unsupported Tier",
            json!({ "Name": "/Types/bad", "Type": "String", "Value": "v", "Tier": "Premium" }),
            "ValidationException",
        ),
        (
            "value nesting another parameter",
            json!({ "Name": "/Types/bad", "Type": "String", "Value": "{{ssm:/Dev/Other}}" }),
            "ValidationException",
        ),
        (
            "policies on a standard parameter",
            json!({
                "Name": "/Types/bad",
                "Type": "String",
                "Value": "v",
                "Tier": "Standard",
                "Policies": "[{\"Type\":\"Expiration\",\"Version\":\"1.0\"}]",
            }),
            "ValidationException",
        ),
    ];
    for (case, body, expected) in refusals {
        let (status, response) = put(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            expected,
            "case {case}: expected {expected}, got {response}"
        );
        let (status, response) = get(&router, "/Types/bad").await;
        assert_eq!(
            status, 400,
            "case {case}: the refused put stored nothing, got {status} {response}"
        );
    }

    // Tags create a parameter; `AddTagsToResource` is how an existing one is tagged, so
    // an overwrite that carries tags is refused and one that carries none is served.
    let tags = json!([{ "Key": "Environment", "Value": "Dev" }]);
    put_ok(
        &router,
        "create with tags",
        &json!({ "Name": "/Tags/param", "Type": "String", "Value": "first", "Tags": &tags }),
    )
    .await;
    let (status, response) = put(
        &router,
        &json!({
            "Name": "/Tags/param",
            "Type": "String",
            "Value": "second",
            "Tags": &tags,
            "Overwrite": true,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case overwrite with tags: must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case overwrite with tags: expected ValidationException, got {response}"
    );
    let parameter = get_ok(&router, "overwrite with tags", "/Tags/param").await;
    assert_eq!(
        parameter["Value"],
        json!("first"),
        "case overwrite with tags: the refused put stored nothing, got {parameter}"
    );
    put_ok(
        &router,
        "overwrite without tags",
        &json!({
            "Name": "/Tags/param",
            "Type": "String",
            "Value": "second",
            "Overwrite": true,
        }),
    )
    .await;
    let parameter = get_ok(&router, "overwrite without tags", "/Tags/param").await;
    assert_eq!(
        parameter["Value"],
        json!("second"),
        "case overwrite without tags: the update is what a read reports, got {parameter}"
    );
}

