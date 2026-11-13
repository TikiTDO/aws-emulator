// SPDX-License-Identifier: CC0-1.0

//! Secrets Manager state survives a restart through `service_common::persist`: a secret
//! sealed under a customer-managed KMS key before the snapshot decrypts to the same
//! plaintext, version stages, and tags in a fresh engine restored from JSON text.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state::ComAmazonawsKmsTrentserviceState as KmsState;
use service_com_amazonaws_secretsmanager_secretsmanager::state::ComAmazonawsSecretsmanagerSecretsmanagerState as SmState;
use service_common::persist::Persisted;
use smithy_server_runtime::MultiServiceRouter;

const REGION: &str = "us-west-2";

fn router_with_kms_and_sm() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

async fn call_json(
    router: &MultiServiceRouter,
    signing_service: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_service}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(serde_json::to_vec(body).unwrap()))
        .unwrap();
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "{target_prefix}.{operation} failed: {}",
        String::from_utf8_lossy(response.body())
    );
    serde_json::from_slice(response.body()).unwrap_or(Value::Null)
}

#[tokio::test]
async fn a_kms_sealed_secret_survives_restart_with_versions_and_tags() {
    let (before, engine) = router_with_kms_and_sm();
    let key = call_json(
        &before,
        "kms",
        "TrentService",
        "CreateKey",
        &json!({ "Description": "db-credentials-key" }),
    )
    .await;
    let key_id = key["KeyMetadata"]["KeyId"].as_str().unwrap().to_string();

    let created = call_json(
        &before,
        "secretsmanager",
        "secretsmanager",
        "CreateSecret",
        &json!({
            "Name": "local/rds/master-password",
            "Description": "RDS master credentials",
            "KmsKeyId": key_id,
            "ClientRequestToken": "11111111-1111-4111-8111-111111111111",
            "SecretString": "{\"username\":\"postgres\",\"password\":\"s3cr3t-v1\"}",
            "Tags": [{ "Key": "env", "Value": "local" }]
        }),
    )
    .await;
    let arn = created["ARN"].as_str().unwrap().to_string();

    call_json(
        &before,
        "secretsmanager",
        "secretsmanager",
        "PutSecretValue",
        &json!({
            "SecretId": arn,
            "ClientRequestToken": "22222222-2222-4222-8222-222222222222",
            "SecretString": "{\"username\":\"postgres\",\"password\":\"s3cr3t-v2\"}"
        }),
    )
    .await;

    let kms_json = serde_json::to_string(&engine.state::<KmsState>().lock().snapshot()).unwrap();
    let sm_json = serde_json::to_string(&engine.state::<SmState>().lock().snapshot()).unwrap();

    let (after, fresh) = router_with_kms_and_sm();
    *fresh.state::<KmsState>().lock() =
        KmsState::restore(KmsState::VERSION, serde_json::from_str(&kms_json).unwrap()).unwrap();
    *fresh.state::<SmState>().lock() =
        SmState::restore(SmState::VERSION, serde_json::from_str(&sm_json).unwrap()).unwrap();

    let current = call_json(
        &after,
        "secretsmanager",
        "secretsmanager",
        "GetSecretValue",
        &json!({ "SecretId": "local/rds/master-password" }),
    )
    .await;
    assert_eq!(
        current["SecretString"],
        "{\"username\":\"postgres\",\"password\":\"s3cr3t-v2\"}"
    );
    assert_eq!(current["ARN"], arn);

    let previous = call_json(
        &after,
        "secretsmanager",
        "secretsmanager",
        "GetSecretValue",
        &json!({
            "SecretId": arn,
            "VersionStage": "AWSPREVIOUS"
        }),
    )
    .await;
    assert_eq!(
        previous["SecretString"],
        "{\"username\":\"postgres\",\"password\":\"s3cr3t-v1\"}"
    );

    let described = call_json(
        &after,
        "secretsmanager",
        "secretsmanager",
        "DescribeSecret",
        &json!({ "SecretId": arn }),
    )
    .await;
    assert_eq!(described["Description"], "RDS master credentials");
    assert_eq!(described["Tags"], json!([{ "Key": "env", "Value": "local" }]));
}
