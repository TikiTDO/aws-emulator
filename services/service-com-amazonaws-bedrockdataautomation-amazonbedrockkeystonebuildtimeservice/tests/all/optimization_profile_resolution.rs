// SPDX-License-Identifier: CC0-1.0

//! Which `dataAutomationProfileArn` `InvokeBlueprintOptimizationAsync` accepts, driven
//! over the real `restJson1` wire path (`MultiServiceRouter::dispatch` with a
//! signed-looking request), so what is asserted is what a client sees.
//!
//! RULE. A submission runs under the Data Automation profile its `dataAutomationProfileArn`
//! names, and AWS publishes those profiles rather than an account creating them: an ARN
//! that satisfies the modelled `DataAutomationProfileArn` `@pattern` but whose resource id
//! names no published profile resolves to nothing, so the submission is refused 400
//! `ValidationException` with the fixed sentence
//! `state::data_automation_profile::INVALID_PROFILE_MESSAGE` and NOTHING is minted — the
//! blueprint it referenced is left exactly as it was, and no optimization job exists.
//!
//! The rows are a TABLE spanning what the rule generalises over: every geography AWS
//! publishes a profile for, both account forms the pattern admits, the partitions it
//! admits, and the ids that name no profile (an opaque id, an unpublished geography,
//! another version, a published id in the wrong case, the version suffix with no
//! geography).
//!
//! The "nothing is minted" half of the rule is asserted against the STORE as well as the
//! answer: after a refused submission the service holds no optimization job for a later
//! `GetBlueprintOptimizationStatus` to resolve, and the accepted submission that follows
//! is the store's only job.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::state::ComAmazonawsBedrockdataautomationAmazonbedrockkeystonebuildtimeserviceState as ServiceState;
use service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{encode_label, error_code, string_at};

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header for this service's signing name (`bedrock`, per
/// `OPERATIONS.md`).
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTKEY/20240101/{REGION}/bedrock/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The emulator with a handle on the shared engine, so a case can assert what the store
/// HOLDS and not only what it answers.
fn emulator_with_store() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

fn emulator() -> MultiServiceRouter {
    emulator_with_store().0
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-data-automation.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization())
        .body(Bytes::from(
            serde_json::to_vec(body).expect("serializable body"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// `CreateBlueprint`, per its modelled `@http` `{method: PUT, uri: /blueprints/}`.
/// Returns the ARN it minted.
async fn create_blueprint(router: &MultiServiceRouter, name: &str) -> String {
    let (status, payload) = call(
        router,
        "PUT",
        "/blueprints/",
        &json!({
            "blueprintName": name,
            "type": "DOCUMENT",
            "schema": SCHEMA,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected CreateBlueprint 201, got {status} with {payload:?}"
    );
    let blueprint = payload
        .get("blueprint")
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: the create answered no blueprint"));
    string_at(&blueprint, "blueprintArn", name)
}

/// The schema every fixture blueprint carries, so a read-back can be compared against it.
const SCHEMA: &str = r#"{"class":"invoice"}"#;

/// The output location every submission here names, kept in one place so a read-back can
/// be compared against exactly what the submission carried.
fn output_configuration() -> Value {
    json!({ "s3Object": { "s3Uri": "s3://bda-output" } })
}

/// One submission naming `profile_arn` for the blueprint `blueprint_arn`, with only the
/// members the model marks required beside it.
fn submission(blueprint_arn: &str, profile_arn: &str) -> Value {
    json!({
        "blueprint": { "blueprintArn": blueprint_arn },
        "samples": [{
            "assetS3Object": { "s3Uri": "s3://bda-samples/assets/invoice-0.pdf" },
            "groundTruthS3Object": { "s3Uri": "s3://bda-samples/truth/invoice-0.json" },
        }],
        "outputConfiguration": output_configuration(),
        "dataAutomationProfileArn": profile_arn,
    })
}

/// RULE: a submission is accepted only where its `dataAutomationProfileArn` names a
/// published Data Automation profile; one that names none is refused 400
/// `ValidationException` with the service's fixed sentence.
#[tokio::test]
async fn only_a_published_profile_arn_is_accepted() {
    let router = emulator();

    // (case, the profile ARN the submission names, whether it resolves)
    let rows: [(&str, String, bool); 10] = [
        (
            "the us profile under this account",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 us.data-automation-v1"
            ),
            true,
        ),
        (
            "the eu profile",
            "arn:aws:bedrock:eu-west-1:000000000000:data-automation-profile/eu.data-automation-v1"
                .to_string(),
            true,
        ),
        (
            "the apac profile",
            "arn:aws:bedrock:ap-southeast-2:000000000000:data-automation-profile/\
             apac.data-automation-v1"
                .to_string(),
            true,
        ),
        (
            "the AWS-owned account field the pattern admits",
            format!("arn:aws:bedrock:{REGION}:aws:data-automation-profile/us.data-automation-v1"),
            true,
        ),
        (
            "another partition",
            "arn:aws-us-gov:bedrock:us-gov-west-1:000000000000:data-automation-profile/\
             us.data-automation-v1"
                .to_string(),
            true,
        ),
        (
            "the china partition",
            "arn:aws-cn:bedrock:cn-north-1:000000000000:data-automation-profile/\
             apac.data-automation-v1"
                .to_string(),
            true,
        ),
        (
            "an opaque id naming no profile",
            format!("arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/4f2b9c1d8a7e"),
            false,
        ),
        (
            "a geography AWS publishes no profile for",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 mars.data-automation-v1"
            ),
            false,
        ),
        (
            "another profile version",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 us.data-automation-v2"
            ),
            false,
        ),
        (
            "a published id in the wrong case",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 US.data-automation-v1"
            ),
            false,
        ),
    ];

    for (index, (case, profile_arn, resolves)) in rows.into_iter().enumerate() {
        let blueprint_arn = create_blueprint(&router, &format!("profile-row-{index}")).await;
        let (status, payload) = call(
            &router,
            "POST",
            "/invokeBlueprintOptimizationAsync",
            &submission(&blueprint_arn, &profile_arn),
        )
        .await;
        if resolves {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the submission to be accepted, got {status} with \
                 {payload:?}"
            );
            assert!(
                !string_at(&payload, "invocationArn", case).is_empty(),
                "case {case:?}: an accepted submission answers the job's ARN, got {payload:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload:?}"
        );
        assert_eq!(
            string_at(&payload, "message", case),
            state::data_automation_profile::INVALID_PROFILE_MESSAGE,
            "case {case:?}: expected the service's fixed sentence, got {payload:?}"
        );
        assert!(
            payload.get("invocationArn").is_none(),
            "case {case:?}: a refused submission mints no invocation, got {payload:?}"
        );
    }
}

/// RULE: a refused submission performs NOTHING — the blueprint it referenced is unchanged
/// and no optimization job was minted for it.
#[tokio::test]
async fn a_refused_submission_leaves_the_blueprint_untouched() {
    let router = emulator();
    let blueprint_arn = create_blueprint(&router, "untouched-by-a-refusal").await;

    // The ids that name no published profile, each one a separate refused submission.
    let unresolvable = [
        format!("arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/4f2b9c1d8a7e"),
        format!(
            "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
             mars.data-automation-v1"
        ),
    ];
    for profile_arn in &unresolvable {
        let (status, payload) = call(
            &router,
            "POST",
            "/invokeBlueprintOptimizationAsync",
            &submission(&blueprint_arn, profile_arn),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {profile_arn:?}: expected 400, got {status} with {payload:?}"
        );
    }

    let (status, read) = call(
        &router,
        "POST",
        &format!("/blueprints/{}/", encode_label(&blueprint_arn)),
        &json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the blueprint survives\": expected 200, got {status} with {read:?}"
    );
    let blueprint = read
        .get("blueprint")
        .cloned()
        .expect("the read answers a blueprint");
    assert_eq!(
        string_at(&blueprint, "blueprintArn", "the blueprint survives"),
        blueprint_arn,
        "case \"the blueprint survives\": the ARN must be the one the create minted"
    );
    assert_eq!(
        string_at(&blueprint, "schema", "the blueprint survives"),
        SCHEMA,
        "case \"the blueprint survives\": the schema must be the one the create carried"
    );
}

/// `GetBlueprintOptimizationStatus`, per its modelled `@http`
/// `{method: POST, uri: /getBlueprintOptimizationStatus/{invocationArn}}`.
async fn get_status(router: &MultiServiceRouter, invocation_arn: &str) -> (u16, Value) {
    let uri = format!(
        "/getBlueprintOptimizationStatus/{}",
        encode_label(invocation_arn)
    );
    call(router, "POST", &uri, &json!({})).await
}

/// RULE: the refusal lands BEFORE anything is minted, so a submission naming an
/// unresolvable profile persists NO optimization job — the store holds nothing a later
/// `GetBlueprintOptimizationStatus` could resolve — while a submission naming a published
/// profile is the store's only job and reads back under the ARN it answered.
///
/// The rows span the id forms that name no published profile: an opaque id, a geography
/// AWS publishes none for, another profile version, a published id in the wrong case, and
/// the version suffix with no geography at all (the pattern's `[a-zA-Z0-9-_.]+` admits
/// every one of them, so only resolution can refuse them).
#[tokio::test]
async fn a_refused_submission_persists_no_optimization_job() {
    let (router, engine) = emulator_with_store();
    let blueprint_arn = create_blueprint(&router, "nothing-minted").await;

    let unresolvable: [(&str, String); 5] = [
        (
            "an opaque id naming no profile",
            format!("arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/4f2b9c1d8a7e"),
        ),
        (
            "a geography AWS publishes no profile for",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 mars.data-automation-v1"
            ),
        ),
        (
            "another profile version",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 us.data-automation-v2"
            ),
        ),
        (
            "a published id in the wrong case",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 US.data-automation-v1"
            ),
        ),
        (
            "the version suffix with no geography",
            format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 .data-automation-v1"
            ),
        ),
    ];

    for (case, profile_arn) in &unresolvable {
        let (status, payload) = call(
            &router,
            "POST",
            "/invokeBlueprintOptimizationAsync",
            &submission(&blueprint_arn, profile_arn),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {payload:?}"
        );
        // The LITERAL sentence AWS answers for this error class, written out here rather
        // than read from the implementation's own constant, so a drift in that constant's
        // wording fails this case.
        assert_eq!(
            string_at(&payload, "message", case),
            "Request has invalid Data Automation Profile Arn",
            "case {case:?}: expected the service's fixed sentence, got {payload:?}"
        );
        let held = engine.state::<ServiceState>();
        let store = held.lock();
        let minted: Vec<&String> = store.optimization_jobs.keys().collect();
        assert!(
            minted.is_empty(),
            "case {case:?}: a refused submission mints no job, the store holds {minted:?}"
        );
    }

    // The same submission with a published profile is accepted, and it is the store's
    // ONLY job: the refusals above persisted nothing that could be counted here.
    let case = "the us profile after the refusals";
    let (status, payload) = call(
        &router,
        "POST",
        "/invokeBlueprintOptimizationAsync",
        &submission(
            &blueprint_arn,
            &format!(
                "arn:aws:bedrock:{REGION}:000000000000:data-automation-profile/\
                 us.data-automation-v1"
            ),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the submission to be accepted, got {status} with {payload:?}"
    );
    let invocation_arn = string_at(&payload, "invocationArn", case);
    {
        let held = engine.state::<ServiceState>();
        let store = held.lock();
        let minted: Vec<&String> = store.optimization_jobs.keys().collect();
        assert_eq!(
            minted.len(),
            1,
            "case {case:?}: exactly one job must exist, the store holds {minted:?}"
        );
        assert!(
            store.optimization_jobs.contains_key(&invocation_arn),
            "case {case:?}: the answered ARN must key the stored job, the store holds \
             {minted:?}"
        );
    }
    let (status, reported) = get_status(&router, &invocation_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the accepted job must read back, got {status} with {reported:?}"
    );
    // The read is about the STORED job: a submission starts it, so the status read
    // straight after it is the first, transient one.
    assert_eq!(
        string_at(&reported, "status", case),
        state::OPTIMIZATION_CREATED,
        "case {case:?}: a just-submitted job is {} until the clock moves it: {reported:?}",
        state::OPTIMIZATION_CREATED
    );
    assert_eq!(
        reported.get("outputConfiguration"),
        Some(&output_configuration()),
        "case {case:?}: the read must report the submitted outputConfiguration: {reported:?}"
    );
}
