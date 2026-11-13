// SPDX-License-Identifier: CC0-1.0

//! SSM Parameter Store state survives a restart through `service_common::persist`:
//! `SecureString` parameters sealed under KMS, parameter version history, and resource
//! tags read back unchanged from a fresh engine restored from JSON text.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state::ComAmazonawsKmsTrentserviceState as KmsState;
use service_com_amazonaws_ssm_amazonssm::state::ComAmazonawsSsmAmazonssmState as SsmState;
use service_common::persist::Persisted;
use smithy_server_runtime::MultiServiceRouter;

const REGION: &str = "us-west-2";

fn router_with_kms_and_ssm() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
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
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{signing_service}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
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
async fn secure_parameters_history_and_tags_survive_restart() {
    let (before, engine) = router_with_kms_and_ssm();
    let key = call_json(
        &before,
        "kms",
        "TrentService",
        "CreateKey",
        &json!({ "Description": "ssm-secure-key" }),
    )
    .await;
    let key_id = key["KeyMetadata"]["KeyId"].as_str().unwrap().to_string();

    call_json(
        &before,
        "ssm",
        "AmazonSSM",
        "PutParameter",
        &json!({
            "Name": "/cdk-bootstrap/hnb659fds/version",
            "Type": "String",
            "Value": "21",
            "Tags": [{ "Key": "managed-by", "Value": "cdk" }]
        }),
    )
    .await;

    call_json(
        &before,
        "ssm",
        "AmazonSSM",
        "PutParameter",
        &json!({
            "Name": "/app/local/jwt-secret",
            "Type": "SecureString",
            "KeyId": key_id,
            "Value": "initial-secret"
        }),
    )
    .await;
    call_json(
        &before,
        "ssm",
        "AmazonSSM",
        "PutParameter",
        &json!({
            "Name": "/app/local/jwt-secret",
            "Type": "SecureString",
            "KeyId": key_id,
            "Overwrite": true,
            "Value": "rotated-secret"
        }),
    )
    .await;

    let kms_json = serde_json::to_string(&engine.state::<KmsState>().lock().snapshot()).unwrap();
    let ssm_json = serde_json::to_string(&engine.state::<SsmState>().lock().snapshot()).unwrap();

    let (after, fresh) = router_with_kms_and_ssm();
    *fresh.state::<KmsState>().lock() =
        KmsState::restore(KmsState::VERSION, serde_json::from_str(&kms_json).unwrap()).unwrap();
    *fresh.state::<SsmState>().lock() =
        SsmState::restore(SsmState::VERSION, serde_json::from_str(&ssm_json).unwrap()).unwrap();

    let bootstrap = call_json(
        &after,
        "ssm",
        "AmazonSSM",
        "GetParameter",
        &json!({ "Name": "/cdk-bootstrap/hnb659fds/version" }),
    )
    .await;
    assert_eq!(bootstrap["Parameter"]["Value"], "21");

    let decrypted = call_json(
        &after,
        "ssm",
        "AmazonSSM",
        "GetParameter",
        &json!({ "Name": "/app/local/jwt-secret", "WithDecryption": true }),
    )
    .await;
    assert_eq!(decrypted["Parameter"]["Value"], "rotated-secret");
    assert_eq!(decrypted["Parameter"]["Version"], 2);

    let history = call_json(
        &after,
        "ssm",
        "AmazonSSM",
        "GetParameterHistory",
        &json!({ "Name": "/app/local/jwt-secret", "WithDecryption": true }),
    )
    .await;
    let versions: Vec<_> = history["Parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["Value"].as_str().unwrap())
        .collect();
    assert_eq!(versions, vec!["initial-secret", "rotated-secret"]);

    let tags = call_json(
        &after,
        "ssm",
        "AmazonSSM",
        "ListTagsForResource",
        &json!({
            "ResourceType": "Parameter",
            "ResourceId": "/cdk-bootstrap/hnb659fds/version"
        }),
    )
    .await;
    assert_eq!(
        tags["TagList"],
        json!([{ "Key": "managed-by", "Value": "cdk" }])
    );
}
