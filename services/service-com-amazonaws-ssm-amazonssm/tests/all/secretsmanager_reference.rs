// SPDX-License-Identifier: CC0-1.0

//! Reading an Amazon Web Services Secrets Manager secret through Parameter Store's reserved
//! `/aws/reference/secretsmanager` hierarchy, end to end over the front door.
//!
//! RULE under test: a name under that reserved hierarchy names a SECRET rather than a
//! parameter, so Parameter Store resolves it against Secrets Manager and reports it as a
//! `SecureString` parameter of the SECRET's identity — the reference path as the request
//! wrote it, the secret's ARN, the instant the version it answers with was created, the
//! source's own answer in `SourceResult`, and no parameter version of its own. A
//! `name:selector` suffix selects by Secrets Manager STAGING LABEL (or by version id), so a
//! read of `:AWSPREVIOUS` serves the demoted version's own bytes; a read naming no selector
//! serves `AWSCURRENT`. `WithDecryption` is what asks for those bytes, Parameter Store
//! holding no ciphertext of its own for a reference. The reserved hierarchy is not the
//! account's, so no hierarchy read enumerates it, and a reference nothing resolves — no such
//! secret, no such label, another Region's secret, a secret scheduled for deletion — is
//! `ParameterNotFound` whose message names the reference and carries the SOURCE's own
//! sentence about why nothing was reported for it. WHETHER the caller may read it is Secrets
//! Manager's decision, on its own `GetSecretValue` action for that secret.
//!
//! Every case goes through the wire a client uses: `POST /` with `X-Amz-Target:
//! AmazonSSM.<Operation>` for Parameter Store and `X-Amz-Target:
//! secretsmanager.<Operation>` for Secrets Manager, both `awsJson1_1`. Secrets Manager and
//! KMS are mounted BESIDE Systems Manager on one engine, so every secret, version, staging
//! label and key a case uses is one those services' own operations created; nothing reaches
//! into a state struct, and every value a reference serves is cross-checked against what
//! Secrets Manager's own `GetSecretValue` answers for the same version.
//!
//! The tables span what the rule generalises over rather than the one recorded read: the
//! form of the secret's name (flat, hierarchical, punctuated) and of its value (a JSON
//! document, a plain string, a multi-line one), the key the secret is encrypted under (the
//! service-managed one and a customer managed key KMS holds), the selector form (none, each
//! reserved staging label, a caller-attached label, a version id), the `WithDecryption`
//! flag, the single and the batch read, and each way a reference resolves to nothing.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::just_in_time_node_access::{assumed_role, call_as};
use super::parameter_store::{call, error_code, error_message, REGION};

/// The reserved hierarchy, as a client writes it.
const REFERENCE_ROOT: &str = "/aws/reference/secretsmanager";

/// What a read through the reserved path is refused with when the service it depends on
/// would not serve the value: Parameter Store's own wrapper for a failed downstream call.
const DEPENDENCY_FAILURE: &str =
    service_com_amazonaws_ssm_amazonssm::state::secretsmanager_reference::DEPENDENCY_FAILURE_MESSAGE;

/// The `ParameterType` a reference reports, from the modelled enum
/// (`smithy_query(query="shape", shape="ParameterType")`).
const REFERENCE_TYPE: &str = "SecureString";

/// The `ParameterDataType` it reports: the default (`smithy_query(query="doc",
/// shape="Parameter", member="DataType")`: "The data type of the parameter, such as text or
/// aws:ec2:image. The default is text").
const REFERENCE_DATA_TYPE: &str = "text";

/// The `Parameter$Version` a reference reports: the member's own modelled default
/// (`smithy_query(query="shape", shape="Parameter")`: `"Version": { "default": 0 }`), a
/// reference having no parameter version of its own.
const REFERENCE_VERSION: i64 = 0;

/// The staging label Secrets Manager keeps on a secret's current value.
const STAGE_CURRENT: &str = "AWSCURRENT";

/// The label it moves to the version `AWSCURRENT` was taken from.
const STAGE_PREVIOUS: &str = "AWSPREVIOUS";

/// A staging label of the CALLER's own, attached through
/// `UpdateSecretVersionStage` — so the selector axis spans more than the labels the service
/// moves itself.
const STAGE_CUSTOM: &str = "EMULATORPINNED";

/// An emulator holding Systems Manager, Secrets Manager, KMS, IAM and STS on one shared
/// engine — the way the assembled router mounts them.
///
/// Secrets Manager is there because a reference resolves against ITS secrets, KMS because a
/// secret encrypted under a customer managed key is opened under that key, and IAM and STS
/// because WHETHER the caller may read the secret is decided from the policies those
/// services hold for the principal its credentials speak for.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suites sign nothing: the `Authorization` header carries only the credential
    // scope, which is where each service reads the Region and the caller's access key id.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One call to another `awsJson1_1` service of this emulator, signed for `region`.
async fn peer(
    router: &MultiServiceRouter,
    service: &str,
    target_prefix: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/{service}/aws4_request, \
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
    (status, parsed)
}

/// One Secrets Manager call in [`REGION`] that must succeed, returning its body.
async fn secrets_ok(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    case: &str,
) -> Value {
    secrets_ok_in(router, REGION, operation, body, case).await
}

/// [`secrets_ok`] at `region`'s endpoint: a secret is regional, so a reference resolves one
/// only in the Region the read is signed for.
async fn secrets_ok_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
    case: &str,
) -> Value {
    let (status, parsed) = peer(
        router,
        "secretsmanager",
        "secretsmanager",
        region,
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: Secrets Manager {operation} {body} must succeed, got {status} with \
         {parsed}"
    );
    parsed
}

/// A symmetric encryption key in [`REGION`], created through KMS's own operations.
async fn kms_key(router: &MultiServiceRouter, description: &str, case: &str) -> String {
    let (status, created) = peer(
        router,
        "kms",
        "TrentService",
        REGION,
        "CreateKey",
        &json!({ "Description": description }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: KMS CreateKey must succeed, got {status} with {created}"
    );
    text(&created["KeyMetadata"], "KeyId", case)
}

/// A required string member of a body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// A version id of the modelled length (`SecretVersionIdType` is 32..64), built from a
/// readable seed so a failing case names the version it meant.
fn version_id(seed: &str) -> String {
    format!("{seed:0<36}")
}

/// One secret a reference read is served from: the forms this rule generalises over.
struct Secret {
    /// What a failing assertion calls this row.
    case: &'static str,
    /// The secret's name, which is also the leaf of its reference path.
    name: &'static str,
    /// The value the create stores, which `AWSPREVIOUS` ends up on.
    first: &'static str,
    /// The value `PutSecretValue` stores, which `AWSCURRENT` ends up on.
    second: &'static str,
    /// Whether the secret is encrypted under a customer managed key rather than the
    /// service-managed one.
    customer_key: bool,
}

/// The secrets every suite below runs over.
const SECRETS: &[Secret] = &[
    Secret {
        case: "a JSON credentials document",
        name: "emu/database/credentials",
        first: r#"{"username":"admin","password":"first-one"}"#,
        second: r#"{"username":"admin","password":"second-one"}"#,
        customer_key: false,
    },
    Secret {
        case: "a flat name holding a plain string",
        name: "plain-token",
        first: "token-one",
        second: "token-two",
        customer_key: false,
    },
    Secret {
        case: "a punctuated name holding a multi-line value",
        name: "team.alpha_secret-1",
        first: "line-one\nline-two",
        second: "line-one\nline-two\nline-three",
        customer_key: false,
    },
    Secret {
        case: "a secret encrypted under a customer managed key",
        name: "emu/cmk/credentials",
        first: r#"{"password":"under-a-key"}"#,
        second: r#"{"password":"under-a-key-again"}"#,
        customer_key: true,
    },
];

/// What a secret this suite staged holds: its identity, and the version each staging label
/// sits on.
struct Staged {
    /// The reference path a read asks for it by.
    path: String,
    /// The secret's ARN, which a reference reports as the parameter's.
    arn: String,
    /// The version `AWSPREVIOUS` and [`STAGE_CUSTOM`] sit on.
    first_version: String,
    /// The version `AWSCURRENT` sits on.
    second_version: String,
}

/// Create `secret`, mint a second version so the first is demoted, and pin a label of the
/// caller's own to that first version — all through Secrets Manager's own operations.
async fn stage(router: &MultiServiceRouter, secret: &Secret) -> Staged {
    let case = secret.case;
    let mut create = json!({
        "Name": secret.name,
        "SecretString": secret.first,
        "ClientRequestToken": version_id("first-"),
    });
    if secret.customer_key {
        let key_id = kms_key(router, secret.name, case).await;
        create["KmsKeyId"] = json!(key_id);
    }
    let created = secrets_ok(router, "CreateSecret", &create, case).await;
    let arn = text(&created, "ARN", case);
    let first_version = text(&created, "VersionId", case);

    let put = secrets_ok(
        router,
        "PutSecretValue",
        &json!({
            "SecretId": secret.name,
            "SecretString": secret.second,
            "ClientRequestToken": version_id("second-"),
        }),
        case,
    )
    .await;
    let second_version = text(&put, "VersionId", case);

    secrets_ok(
        router,
        "UpdateSecretVersionStage",
        &json!({
            "SecretId": secret.name,
            "VersionStage": STAGE_CUSTOM,
            "MoveToVersionId": first_version,
        }),
        case,
    )
    .await;

    Staged {
        path: format!("{REFERENCE_ROOT}/{}", secret.name),
        arn,
        first_version,
        second_version,
    }
}

/// The `Parameter` a reference read reports, asserted to have succeeded.
async fn reference(
    router: &MultiServiceRouter,
    name: &str,
    with_decryption: bool,
    case: &str,
) -> Value {
    let (status, body) = call(
        router,
        "GetParameter",
        &json!({ "Name": name, "WithDecryption": with_decryption }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetParameter {name} must serve the secret, got {status} with {body}"
    );
    body["Parameter"].clone()
}

/// Everything a reference reports that is the SECRET's identity rather than any parameter's,
/// checked on one reported parameter.
fn assert_reference_shape(
    reported: &Value,
    staged: &Staged,
    expected_name: &str,
    expected_selector: Option<&str>,
    case: &str,
) {
    assert_eq!(
        reported["Name"].as_str(),
        Some(expected_name),
        "case {case:?}: the reference is reported under the path the read asked for, got \
         {reported}"
    );
    assert_eq!(
        reported["Type"].as_str(),
        Some(REFERENCE_TYPE),
        "case {case:?}: a reference is reported as {REFERENCE_TYPE}, got {reported}"
    );
    assert_eq!(
        reported["DataType"].as_str(),
        Some(REFERENCE_DATA_TYPE),
        "case {case:?}: a reference carries the default data type, got {reported}"
    );
    assert_eq!(
        reported["Version"].as_i64(),
        Some(REFERENCE_VERSION),
        "case {case:?}: a reference has no parameter version of its own, got {reported}"
    );
    assert_eq!(
        reported["ARN"].as_str(),
        Some(staged.arn.as_str()),
        "case {case:?}: a reference reports the secret's own ARN, got {reported}"
    );
    assert_eq!(
        reported["Selector"].as_str(),
        expected_selector,
        "case {case:?}: the selector reported is the one the read carried, got {reported}"
    );
    assert!(
        reported["LastModifiedDate"].is_number(),
        "case {case:?}: a reference reports when its version was created, got {reported}"
    );
}

/// The source's own answer, as the reference reports it in `SourceResult`.
fn source_result(reported: &Value, case: &str) -> Value {
    let raw = reported["SourceResult"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no SourceResult in {reported}"));
    serde_json::from_str(raw)
        .unwrap_or_else(|_| panic!("case {case:?}: SourceResult is not the source's JSON: {raw}"))
}

/// RULE: a read of the reserved path serves the secret's value for the staging label the
/// selector names — `AWSCURRENT` when it names none — reported as a `SecureString` parameter
/// of the secret's identity, with the source's own answer in `SourceResult`.
#[tokio::test]
async fn reference_serves_the_secret_for_the_selected_staging_label() {
    let router = emulator();
    for secret in SECRETS {
        let staged = stage(&router, secret).await;
        // Each row is one selector form, against the version and value it must resolve to.
        let selections: &[(Option<&str>, &str, &str)] = &[
            (None, secret.second, &staged.second_version),
            (Some(STAGE_CURRENT), secret.second, &staged.second_version),
            (Some(STAGE_PREVIOUS), secret.first, &staged.first_version),
            (Some(STAGE_CUSTOM), secret.first, &staged.first_version),
            (
                Some(&staged.first_version),
                secret.first,
                &staged.first_version,
            ),
            (
                Some(&staged.second_version),
                secret.second,
                &staged.second_version,
            ),
        ];
        for (selector, expected_value, expected_version) in selections {
            let case = format!("{} selected by {selector:?}", secret.case);
            let asked = match selector {
                Some(selector) => format!("{}:{selector}", staged.path),
                None => staged.path.clone(),
            };
            let reported = reference(&router, &asked, true, &case).await;
            let echoed = selector.map(|selector| format!(":{selector}"));
            assert_reference_shape(
                &reported,
                &staged,
                &staged.path,
                echoed.as_deref(),
                &case,
            );
            assert_eq!(
                reported["Value"].as_str(),
                Some(*expected_value),
                "case {case:?}: the value served is the one that staging label's version \
                 holds, got {reported}"
            );
            let source = source_result(&reported, &case);
            assert_eq!(
                source["versionId"].as_str(),
                Some(*expected_version),
                "case {case:?}: the source's answer names the version it served, got {source}"
            );
            assert_eq!(
                source["arn"].as_str(),
                Some(staged.arn.as_str()),
                "case {case:?}: the source's answer carries the secret's ARN, got {source}"
            );
            assert_eq!(
                source["name"].as_str(),
                Some(secret.name),
                "case {case:?}: the source's answer carries the secret's name, got {source}"
            );
            assert_eq!(
                source["secretString"].as_str(),
                Some(*expected_value),
                "case {case:?}: the source's answer carries the value it served, got {source}"
            );

            // The same version read through Secrets Manager's own front door must answer
            // with exactly what the reference served, so the reference cannot be serving
            // anything the source does not hold.
            let direct = secrets_ok(
                &router,
                "GetSecretValue",
                &json!({ "SecretId": secret.name, "VersionId": expected_version }),
                &case,
            )
            .await;
            assert_eq!(
                direct["SecretString"].as_str(),
                reported["Value"].as_str(),
                "case {case:?}: the reference serves what GetSecretValue answers, got \
                 {direct} against {reported}"
            );
            let mut stages: Vec<String> = source["versionStages"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no versionStages in {source}"))
                .iter()
                .map(|stage| stage.as_str().unwrap_or_default().to_string())
                .collect();
            stages.sort();
            let mut held: Vec<String> = direct["VersionStages"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no VersionStages in {direct}"))
                .iter()
                .map(|stage| stage.as_str().unwrap_or_default().to_string())
                .collect();
            held.sort();
            assert_eq!(
                stages, held,
                "case {case:?}: the labels the source reports are the ones the secret \
                 holds, got {stages:?} against {held:?}"
            );
        }
    }
}

/// RULE: `WithDecryption` is what asks for a reference's bytes — Parameter Store holds no
/// ciphertext of its own for one — so a read that asks for no decryption is served the
/// reference's identity and NO value, and no part of the source's answer either.
#[tokio::test]
async fn a_reference_read_without_decryption_serves_no_secret_bytes() {
    let router = emulator();
    for secret in SECRETS {
        let staged = stage(&router, secret).await;
        for selector in [None, Some(STAGE_PREVIOUS), Some(STAGE_CURRENT)] {
            let case = format!("{} selected by {selector:?}", secret.case);
            let asked = match selector {
                Some(selector) => format!("{}:{selector}", staged.path),
                None => staged.path.clone(),
            };
            let expected = if selector == Some(STAGE_PREVIOUS) {
                secret.first
            } else {
                secret.second
            };
            let decrypted = reference(&router, &asked, true, &case).await;
            assert_eq!(
                decrypted["Value"].as_str(),
                Some(expected),
                "case {case:?}: a decrypting read serves the version's value, got {decrypted}"
            );

            let plain = reference(&router, &asked, false, &case).await;
            assert_eq!(
                plain["Value"].as_str(),
                None,
                "case {case:?}: a read asking for no decryption serves no value, got {plain}"
            );
            assert_eq!(
                plain["SourceResult"].as_str(),
                None,
                "case {case:?}: nor the source's answer, which carries that value, got {plain}"
            );
            assert!(
                !plain.to_string().contains(expected),
                "case {case:?}: no part of the answer carries the secret's bytes, got {plain}"
            );
            // The reference's own identity is still reported, so the read answered.
            assert_reference_shape(
                &plain,
                &staged,
                &staged.path,
                selector.map(|selector| format!(":{selector}")).as_deref(),
                &case,
            );
        }
    }
}

/// The sentence Secrets Manager itself words for a `SecretId` it holds no secret for, which
/// a reference nothing resolves carries through.
const SOURCE_NO_SUCH_SECRET: &str = "Secrets Manager can't find the specified secret.";

/// The sentence it words for a secret that is inside its recovery window.
const SOURCE_MARKED_FOR_DELETION: &str =
    "You can't perform this operation on the secret because it was marked for deletion.";

/// The sentence it words for a staging label no version of the secret carries.
fn source_no_such_label(label: &str) -> String {
    format!("Secrets Manager can't find the specified secret value for staging label: {label}")
}

/// The message `ParameterNotFound` carries for a read of the reserved hierarchy nothing
/// resolved, as AWS words it: the reference named CANONICALLY (no leading `/`, selector
/// stripped), and the source's own sentence about why it reported nothing.
fn expected_reference_miss(asked: &str, source: &str) -> String {
    let canonical = asked.trim_start_matches('/');
    let named = match canonical.rsplit_once(':') {
        Some((before, _)) if !before.is_empty() => before,
        _ => canonical,
    };
    format!(
        "An error occurred (ParameterNotFound) when referencing Secrets Manager: Secret \
         {named} not found. {source}"
    )
}

/// RULE: a reference that resolves to nothing is `ParameterNotFound`, and that refusal STATES
/// why — it names the reference and carries the source's own sentence about the secret, the
/// staging label or the scheduled deletion that left it with nothing to report. A name in the
/// `/aws` hierarchy that references no secret at all is the ordinary miss instead, which
/// carries no message.
#[tokio::test]
async fn unresolvable_references_name_the_reference_and_the_sources_sentence() {
    let router = emulator();
    let secret = &SECRETS[0];
    let staged = stage(&router, secret).await;

    // A secret of ANOTHER Region, so a read signed for this one resolves nothing by that
    // name: a secret is reached through the endpoint that holds it.
    secrets_ok_in(
        &router,
        "us-east-1",
        "CreateSecret",
        &json!({
            "Name": "elsewhere-only",
            "SecretString": "not-here",
            "ClientRequestToken": version_id("elsewhere-"),
        }),
        "another Region's secret",
    )
    .await;

    // A secret scheduled for deletion: "its details, including the encrypted secret value,
    // is not accessible" while it is.
    secrets_ok(
        &router,
        "CreateSecret",
        &json!({
            "Name": "going-away",
            "SecretString": "doomed",
            "ClientRequestToken": version_id("doomed-"),
        }),
        "a secret scheduled for deletion",
    )
    .await;
    secrets_ok(
        &router,
        "DeleteSecret",
        &json!({ "SecretId": "going-away" }),
        "a secret scheduled for deletion",
    )
    .await;

    // Each row is one way a reference resolves to nothing, and WHICH sentence the source
    // words for it; `None` is a name that references no secret at all, which is the ordinary
    // miss and carries no message.
    let cases: &[(&str, String, Option<String>)] = &[
        (
            "a name no secret is held under",
            format!("{REFERENCE_ROOT}/nobody-created-this"),
            Some(SOURCE_NO_SUCH_SECRET.to_string()),
        ),
        (
            "a staging label no version carries",
            format!("{}:AWSPENDING", staged.path),
            Some(source_no_such_label("AWSPENDING")),
        ),
        (
            "a label nothing attached",
            format!("{}:NOSUCHLABEL", staged.path),
            Some(source_no_such_label("NOSUCHLABEL")),
        ),
        (
            "a version id of another secret",
            format!("{}:{}", staged.path, version_id("other-")),
            Some(source_no_such_label(&version_id("other-"))),
        ),
        (
            "the reserved hierarchy itself",
            REFERENCE_ROOT.to_string(),
            None,
        ),
        (
            "the hierarchy level above it",
            "/aws/reference".to_string(),
            None,
        ),
        (
            "another Region's secret",
            format!("{REFERENCE_ROOT}/elsewhere-only"),
            Some(SOURCE_NO_SUCH_SECRET.to_string()),
        ),
        (
            "a secret scheduled for deletion",
            format!("{REFERENCE_ROOT}/going-away"),
            Some(SOURCE_MARKED_FOR_DELETION.to_string()),
        ),
    ];
    for (case, asked, source) in cases {
        let (status, body) = call(
            &router,
            "GetParameter",
            &json!({ "Name": asked, "WithDecryption": true }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: {asked} resolves to nothing, got {status} with {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ParameterNotFound",
            "case {case:?}: expected ParameterNotFound, got {code} with {body}"
        );
        let message = error_message(&body);
        let expected = source
            .as_deref()
            .map(|source| expected_reference_miss(asked, source))
            .unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal to carry {expected:?}, got {message:?} in \
             {body}"
        );
    }
}

/// RULE: the reserved hierarchy is not the account's, so no hierarchy read enumerates a
/// secret through it — whatever level it is asked at, and whether or not it recurses.
#[tokio::test]
async fn hierarchy_reads_never_enumerate_the_reserved_path() {
    let router = emulator();
    for secret in SECRETS {
        stage(&router, secret).await;
    }
    // The secret IS resolvable by name, which is what makes the empty listings a statement
    // about enumeration rather than about an empty emulator.
    let staged = format!("{REFERENCE_ROOT}/{}", SECRETS[0].name);
    let served = reference(&router, &staged, true, "the staged secret").await;
    assert_eq!(
        served["Value"].as_str(),
        Some(SECRETS[0].second),
        "the staged secret must be served by name, got {served}"
    );

    let paths = [
        REFERENCE_ROOT.to_string(),
        format!("{REFERENCE_ROOT}/"),
        "/aws/reference".to_string(),
        "/aws".to_string(),
    ];
    for path in &paths {
        for recursive in [false, true] {
            let case = format!("{path} recursive={recursive}");
            let (status, body) = call(
                &router,
                "GetParametersByPath",
                &json!({ "Path": path, "Recursive": recursive, "WithDecryption": true }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a hierarchy read of the reserved path answers, got {status} \
                 with {body}"
            );
            let reported: Vec<String> = body["Parameters"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|parameter| parameter["Name"].as_str().unwrap_or_default().to_string())
                .collect();
            let leaked: Vec<&String> = reported
                .iter()
                .filter(|name| name.starts_with(REFERENCE_ROOT))
                .collect();
            assert!(
                leaked.is_empty(),
                "case {case:?}: no hierarchy read enumerates the reserved path, got \
                 {leaked:?} among {reported:?}"
            );
        }
    }
}

/// RULE: the batch read resolves a reference exactly as the single read does, and reports a
/// reference it cannot resolve among the names it could not resolve rather than refusing the
/// call.
#[tokio::test]
async fn the_batch_read_resolves_references_beside_parameters() {
    let router = emulator();
    let secret = &SECRETS[0];
    let staged = stage(&router, secret).await;

    // One ordinary parameter of the account, so the batch spans both kinds of name.
    let (status, body) = call(
        &router,
        "PutParameter",
        &json!({ "Name": "/emu/plain", "Value": "a-parameter", "Type": "String" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the ordinary parameter must be stored, got {status} with {body}"
    );

    let missing = format!("{REFERENCE_ROOT}/nobody-created-this");
    let previous = format!("{}:{STAGE_PREVIOUS}", staged.path);
    let (status, body) = call(
        &router,
        "GetParameters",
        &json!({
            "Names": [staged.path.clone(), previous.clone(), missing.clone(), "/emu/plain"],
            "WithDecryption": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a batch read carrying references answers, got {status} with {body}"
    );
    let reported = body["Parameters"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("no Parameters in {body}"));
    let values: Vec<(String, Option<String>, Option<String>)> = reported
        .iter()
        .map(|parameter| {
            (
                parameter["Name"].as_str().unwrap_or_default().to_string(),
                parameter["Selector"].as_str().map(str::to_string),
                parameter["Value"].as_str().map(str::to_string),
            )
        })
        .collect();
    let expected: Vec<(String, Option<String>, Option<String>)> = vec![
        (
            staged.path.clone(),
            None,
            Some(secret.second.to_string()),
        ),
        (
            staged.path.clone(),
            Some(format!(":{STAGE_PREVIOUS}")),
            Some(secret.first.to_string()),
        ),
        (
            "/emu/plain".to_string(),
            None,
            Some("a-parameter".to_string()),
        ),
    ];
    for row in &expected {
        assert!(
            values.contains(row),
            "case {:?}: the batch read reports it, got {values:?}",
            row.0
        );
    }
    assert_eq!(
        values.len(),
        expected.len(),
        "the batch read reports exactly the names it resolved, got {values:?}"
    );
    let invalid: Vec<String> = body["InvalidParameters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        invalid,
        vec![missing.clone()],
        "a reference nothing resolves is reported among the unresolvable names, got \
         {invalid:?}"
    );
}

/// RULE: a reference read reports what the source holds and changes nothing — not the
/// secret's versions or labels, and not this service's own parameters: two reads of one
/// reference answer alike, and no parameter appears for the path that was read.
#[tokio::test]
async fn reading_a_reference_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let secret = &SECRETS[1];
    let staged = stage(&router, secret).await;
    let case = "repeated reads";

    let first = reference(&router, &staged.path, true, case).await;
    let again = reference(&router, &staged.path, true, case).await;
    assert_eq!(
        first["Value"].as_str(),
        again["Value"].as_str(),
        "case {case:?}: a second read serves the same value, got {first} against {again}"
    );
    assert_eq!(
        first["ARN"].as_str(),
        again["ARN"].as_str(),
        "case {case:?}: a second read reports the same secret, got {first} against {again}"
    );

    let described = secrets_ok(
        &router,
        "DescribeSecret",
        &json!({ "SecretId": secret.name }),
        case,
    )
    .await;
    let stages = described["VersionIdsToStages"]
        .as_object()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no VersionIdsToStages in {described}"));
    let current = stages
        .iter()
        .find(|(_, labels)| {
            labels
                .as_array()
                .is_some_and(|labels| labels.iter().any(|label| label == STAGE_CURRENT))
        })
        .map(|(version, _)| version.clone());
    assert_eq!(
        current.as_deref(),
        Some(staged.second_version.as_str()),
        "case {case:?}: reading a reference moves no staging label, got {described}"
    );

    // The reference is not a parameter of the account, so the account's own hierarchy
    // reports nothing for it however many times it was read.
    let (status, listed) = call(
        &router,
        "DescribeParameters",
        &json!({ "MaxResults": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeParameters answers, got {status} with {listed}"
    );
    let names: Vec<String> = listed["Parameters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|parameter| parameter["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !names.iter().any(|name| name.starts_with(REFERENCE_ROOT)),
        "case {case:?}: no parameter is created by reading a reference, got {names:?}"
    );
}

/// RULE: the SECRETS MANAGER permission decides a reference read, so a caller holding
/// `ssm:GetParameter` but not `secretsmanager:GetSecretValue` on the secret gets no bytes —
/// the read is refused as the dependency this service called failing — while the same
/// caller's reads of the account's own parameters are untouched, and a caller the secret's
/// policies do allow is served.
#[tokio::test]
async fn the_secrets_manager_permission_decides_the_reference_read() {
    let router = emulator();
    let secret = &SECRETS[0];
    let staged = stage(&router, secret).await;

    // An ordinary parameter of the account, so each role's reads of Parameter Store's own
    // parameters are seen beside its reads of the reference.
    let (status, body) = call(
        &router,
        "PutParameter",
        &json!({ "Name": "/emu/ordinary", "Value": "ordinary-parameter-value", "Type": "String" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the ordinary parameter must be stored, got {status} with {body}"
    );

    // Each row is one role's whole authority, and whether the reference read is served to
    // it: the permission that decides is the Secrets Manager one, so the rows differ only in
    // which Secrets Manager action they allow — on everything, on the secret's own ARN, and
    // not at all.
    let policies: &[(&str, &str, String, bool)] = &[
        (
            "the Secrets Manager read allowed on everything",
            "emu-reference-allowed",
            allow(&[("ssm:GetParameter", "*"), ("secretsmanager:GetSecretValue", "*")]),
            true,
        ),
        (
            "the Secrets Manager read allowed on this secret's ARN",
            "emu-reference-scoped",
            allow(&[
                ("ssm:GetParameter", "*"),
                ("secretsmanager:GetSecretValue", staged.arn.as_str()),
            ]),
            true,
        ),
        (
            "every Parameter Store read allowed, the Secrets Manager read not",
            "emu-reference-refused",
            allow(&[
                ("ssm:GetParameter", "*"),
                ("secretsmanager:DescribeSecret", "*"),
            ]),
            false,
        ),
        (
            "the Secrets Manager read allowed on another secret only",
            "emu-reference-elsewhere",
            allow(&[
                ("ssm:GetParameter", "*"),
                (
                    "secretsmanager:GetSecretValue",
                    "arn:aws:secretsmanager:us-west-2:000000000000:secret:someone-else-AbCdEf",
                ),
            ]),
            false,
        ),
    ];
    for (case, role, policy, served) in policies {
        let principal = assumed_role(&router, role, Some(policy)).await;
        let (status, body) = call_as(
            &router,
            &principal.access_key_id,
            REGION,
            "GetParameter",
            &json!({ "Name": staged.path, "WithDecryption": true }),
        )
        .await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: {} must be served the reference, got {status} with {body}",
                principal.arn
            );
            assert_eq!(
                body["Parameter"]["Value"].as_str(),
                Some(secret.second),
                "case {case:?}: the served value is the secret's current one, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {} must not be served the reference, got {status} with {body}",
                principal.arn
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ValidationException",
                "case {case:?}: a refused dependency is a ValidationException, got {code} \
                 with {body}"
            );
            assert_eq!(
                error_message(&body),
                DEPENDENCY_FAILURE,
                "case {case:?}: expected {DEPENDENCY_FAILURE:?}, got {body}"
            );
            assert!(
                !body.to_string().contains(secret.second),
                "case {case:?}: a refused read serves no secret bytes, got {body}"
            );
        }

        // Whatever the secret's permissions said, this role's read of an ordinary parameter
        // is Parameter Store's own decision and is served.
        let (status, ordinary) = call_as(
            &router,
            &principal.access_key_id,
            REGION,
            "GetParameter",
            &json!({ "Name": "/emu/ordinary" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the ordinary parameter still reads, got {status} with {ordinary}"
        );
        assert_eq!(
            ordinary["Parameter"]["Value"].as_str(),
            Some("ordinary-parameter-value"),
            "case {case:?}: the ordinary parameter's own value is served, got {ordinary}"
        );
    }
}

/// An identity policy allowing each `(action, resource)` pair and nothing else, as
/// `iam:PutRolePolicy` stores one.
fn allow(grants: &[(&str, &str)]) -> String {
    let statements: Vec<Value> = grants
        .iter()
        .map(|(action, resource)| {
            json!({ "Effect": "Allow", "Action": action, "Resource": resource })
        })
        .collect();
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}
