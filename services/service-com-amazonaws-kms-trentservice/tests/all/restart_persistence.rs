// SPDX-License-Identifier: CC0-1.0

//! KMS state survives a restart through `service_common::persist`: a ciphertext made before
//! the snapshot decrypts, to the same plaintext, in a fresh engine restored from it.

use serde_json::json;
use service_com_amazonaws_kms_trentservice::state::ComAmazonawsKmsTrentserviceState as State;
use service_common::persist::Persisted;

use super::harness::{b64, ok, router_with_engine, unb64};

#[tokio::test]
async fn a_ciphertext_from_before_a_restart_decrypts_after_it() {
    let (before, engine) = router_with_engine();
    let created = ok(&before, "CreateKey", &json!({ "Description": "survives" })).await;
    let key_id = created["KeyMetadata"]["KeyId"].as_str().unwrap().to_string();
    ok(&before, "CreateAlias", &json!({ "AliasName": "alias/survives", "TargetKeyId": key_id })).await;
    let sealed = ok(
        &before,
        "Encrypt",
        &json!({ "KeyId": key_id, "Plaintext": b64(b"the overnight job's secret") }),
    )
    .await;
    let ciphertext = unb64("encrypt", &sealed, "CiphertextBlob");

    // What a shutdown writes, through the same JSON text the file holds.
    let snapshot = engine.state::<State>().lock().snapshot();
    let text = serde_json::to_string(&snapshot).unwrap();
    let restored = State::restore(State::VERSION, serde_json::from_str(&text).unwrap()).unwrap();

    let (after, fresh) = router_with_engine();
    *fresh.state::<State>().lock() = restored;
    let opened = ok(&after, "Decrypt", &json!({ "CiphertextBlob": b64(&ciphertext) })).await;
    assert_eq!(unb64("decrypt", &opened, "Plaintext"), b"the overnight job's secret");
    assert_eq!(opened["KeyId"].as_str().unwrap().rsplit('/').next(), Some(key_id.as_str()));
    let described = ok(&after, "DescribeKey", &json!({ "KeyId": "alias/survives" })).await;
    assert_eq!(described["KeyMetadata"]["KeyId"], key_id.as_str());
    assert_eq!(described["KeyMetadata"]["Description"], "survives");
}
