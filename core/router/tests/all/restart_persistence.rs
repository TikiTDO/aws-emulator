// SPDX-License-Identifier: CC0-1.0

//! Restart persistence end to end through the production router wrapper: a KMS key and an IAM
//! role created through [`PersistingRouter`] are on disk the moment each response returns, with
//! no shutdown save (as after `kill -9`), and a fresh engine restored from that directory
//! decrypts the old ciphertext and reads the role back.

use std::sync::Arc;

use bytes::Bytes;
use emulator_server::EmulatorService;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use aws_emulator_router::persistence::{Persistence, PersistingRouter};

const REGION: &str = "us-east-1";
const KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

fn router_over(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // Nothing here signs: the `Authorization` header carries only the credential scope.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        Arc::clone(engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            engine,
        )),
    );
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
        Arc::clone(engine),
    ));
    router
}

fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={KEY_ID}/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-target, Signature=0000"
    )
}

async fn kms(service: &impl EmulatorService, operation: &str, body: Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header("authorization", authorization("kms"))
        .body(Bytes::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let response = service.handle_request(request).await;
    assert_eq!(response.status(), 200, "{operation}: {}", String::from_utf8_lossy(response.body()));
    serde_json::from_slice(response.body()).unwrap()
}

async fn iam(service: &impl EmulatorService, body: &str) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body.to_string()))
        .unwrap();
    let response = service.handle_request(request).await;
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(response.status(), 200, "{text}");
    text
}

#[tokio::test]
async fn an_acknowledged_change_is_on_disk_before_the_answer_and_comes_back_after_a_crash() {
    let dir = std::env::temp_dir().join(format!("aws-emulator-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Selected once, as boot does, and the same value used for the restart.
    let state_dir = service_common::persist::StateDir::new(&dir, "default").unwrap();
    let state = state_dir.dir();

    let before_engine = Arc::new(engine::InMemoryEngine::new());
    let persistence = Arc::new(Persistence::all(Arc::clone(&before_engine), state_dir.clone()).unwrap());
    let first = persistence.restore().unwrap();
    assert!(first.restored.is_empty(), "{first:?}");
    let before = PersistingRouter::new(router_over(&before_engine), Arc::clone(&persistence));

    let created = kms(&before, "CreateKey", json!({ "Description": "overnight" })).await;
    let key_id = created["KeyMetadata"]["KeyId"].as_str().unwrap().to_string();
    // Durable before the caller heard success: the snapshot already names the key.
    let on_disk = std::fs::read_to_string(state.join("kms.json")).expect("kms snapshot written");
    assert!(on_disk.contains(&key_id));
    // Key material: the directory and the snapshot are this user's alone.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&state), 0o700);
        assert_eq!(mode(&state.join("kms.json")), 0o600);
    }

    let plaintext = aws_smithy_types::base64::encode(b"night shift");
    let sealed = kms(&before, "Encrypt", json!({ "KeyId": key_id, "Plaintext": plaintext })).await;
    let ciphertext = sealed["CiphertextBlob"].as_str().unwrap().to_string();
    let role = iam(
        &before,
        "Action=CreateRole&Version=2010-05-08&RoleName=DeployRole&AssumeRolePolicyDocument=%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22ecs-tasks.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%7D%5D%7D",
    )
    .await;
    assert!(role.contains("<RoleName>DeployRole</RoleName>"), "{role}");

    // A read changes nothing, so nothing is rewritten.
    kms(&before, "DescribeKey", json!({ "KeyId": key_id })).await;
    assert_eq!(persistence.save_changed().await.unwrap(), Vec::<&str>::new());

    // The crash: no shutdown save, the old engine simply gone.
    drop(before);
    drop(persistence);

    let after_engine = Arc::new(engine::InMemoryEngine::new());
    let restored = Persistence::all(Arc::clone(&after_engine), state_dir).unwrap().restore().unwrap();
    assert!(restored.restored.contains(&"kms") && restored.restored.contains(&"iam"), "{restored:?}");
    let after = router_over(&after_engine);
    let opened = kms(&after, "Decrypt", json!({ "CiphertextBlob": ciphertext })).await;
    assert_eq!(opened["Plaintext"].as_str(), Some(plaintext.as_str()));
    let read = iam(&after, "Action=GetRole&Version=2010-05-08&RoleName=DeployRole").await;
    assert!(read.contains("<RoleName>DeployRole</RoleName>"), "{read}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[tokio::test]
async fn a_change_that_cannot_be_written_is_not_answered_as_success() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("aws-emulator-not-durable-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let state_dir = service_common::persist::StateDir::new(&dir, "default").unwrap();
    let engine = Arc::new(engine::InMemoryEngine::new());
    let persistence = Arc::new(Persistence::all(Arc::clone(&engine), state_dir.clone()).unwrap());
    persistence.restore().unwrap();
    // The empty starting state is written once, so only what the request changes is pending.
    assert_eq!(persistence.save_changed().await.unwrap(), vec!["kms", "iam", "ec2"]);
    let router = PersistingRouter::new(router_over(&engine), Arc::clone(&persistence));

    // The namespace directory stops accepting files: the next write fails.
    std::fs::set_permissions(state_dir.dir(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", "TrentService.CreateKey")
        .header("authorization", authorization("kms"))
        .body(Bytes::from_static(b"{}"))
        .unwrap();
    let response = router.handle_request(request).await;
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(response.status(), 500, "{text}");
    assert!(text.contains("InternalFailure") && text.contains("kms"), "{text}");

    // Writable again: the change still in memory is written by the next save.
    std::fs::set_permissions(state_dir.dir(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(persistence.save_changed().await.unwrap(), vec!["kms"]);
    let _ = std::fs::remove_dir_all(&dir);
}
