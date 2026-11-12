// SPDX-License-Identifier: CC0-1.0

//! WHICH KMS failure S3 relays when the key an SSE-KMS write names will not mint a data key.
//!
//! RULE: a write that asks for SSE-KMS is encrypted under a data key KMS mints for the key
//! the write names, so the write performs that envelope call BEFORE anything is stored and a
//! call KMS fails is relayed to the caller: HTTP 400, the KMS exception's own name qualified
//! by the service that raised it, and KMS's own sentence — with nothing written. WHICH
//! exception that is comes from the KEY STATE, because it is KMS that raises it: a key whose
//! `KeyState` is `Disabled` refuses a cryptographic operation with `DisabledException` ("the
//! request was rejected because the specified KMS key is not enabled",
//! `smithy_query(query="shape",shape="DisabledException",model_path="services/service-com-amazonaws-kms-trentservice/smithy-model/com-amazonaws-kms-trentservice.json")`),
//! while a key in any OTHER state outside `Enabled` refuses it with
//! `KMSInvalidStateException` ("the state of the specified resource is not valid for this
//! request") — two error shapes KMS's data-key operations declare separately, so the two
//! refusals are two classes and not one. The sentence is the same either way: the key's ARN
//! and the state it is in.
//!
//! The rows differ along the KEY STATE a key was put into — the dimension the rule
//! generalises over — and the table is walked for each of the write operations that resolve
//! an SSE-KMS key of their own (`PutObject`, `CopyObject`, `CreateMultipartUpload`), since
//! the class belongs to the key rather than to one operation. The states are
//! `smithy_query(query="shape",shape="KeyState")` values, reached through KMS's own
//! operations: `DisableKey` for `Disabled`, `ScheduleKeyDeletion` for `PendingDeletion`, and
//! `CreateKey` with an `EXTERNAL` origin for `PendingImport` — a key still awaiting its
//! material. `Enabled` is the served control, and re-enabling the disabled key shows the
//! class tracks the STATE rather than the key.
//!
//! Every assertion drives real HTTP requests through a router carrying S3 and KMS on ONE
//! engine ([`super::kms_key_references::router_with_kms`]), so the key a write names is the
//! key KMS really holds and every expected ARN and key state is the one KMS itself reports.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::kms_key_references::{
    copy_object, create_multipart_upload, created_key, described_key_member, kms, router_with_kms,
};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The `ServerSideEncryption` value these writes ask for: the SSE-KMS one a key id belongs to
/// (`smithy_query(query="shape",shape="ServerSideEncryption")`).
const KMS_ALGORITHM: &str = "aws:kms";

/// The bound headers a write states its server-side encryption and its KMS key in.
const ENCRYPTION_HEADER: &str = "x-amz-server-side-encryption";
const KEY_ID_HEADER: &str = "x-amz-server-side-encryption-aws-kms-key-id";

/// The status every one of these relays carries. Recorded behaviour: the s3 model binds no
/// error shape for the relay, so the status cannot come from it.
const KMS_RELAY_STATUS: u16 = 400;

/// The relayed code for a key that is merely DISABLED, and the one for every other key state
/// that mints no data key.
const KMS_DISABLED_CODE: &str = "KMS.DisabledException";
const KMS_INVALID_STATE_CODE: &str = "KMS.KMSInvalidStateException";

/// The bytes every write in this suite carries.
const BODY: &[u8] = b"relay-error-class-payload";

/// One row of the key-state table: the case, the key's ARN, the `KeyState` KMS must report for
/// it, the code S3 must relay and the sentence KMS words for it.
type StateRow = (&'static str, String, &'static str, &'static str, String);

/// A key created with KMS's own `CreateKey` whose key material is to be IMPORTED, as
/// `(key id, key ARN)`: such a key holds no material yet, so it is a key in a state no
/// cryptographic operation can run under.
async fn created_external_key(router: &MultiServiceRouter) -> String {
    let (status, payload) = kms(
        router,
        "CreateKey",
        &json!({"Description": "the key awaiting imported material", "Origin": "EXTERNAL"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the key with an EXTERNAL origin must succeed, got {status} with {payload}"
    );
    payload["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateKey must answer the key ARN, got {payload}"))
        .to_string()
}

/// The rows, each with the key it had to put into that state — every state reached through
/// KMS's own operations rather than by planting anything in a store.
async fn unusable_rows(router: &MultiServiceRouter) -> Vec<StateRow> {
    let (disabled_id, disabled_arn) = created_key(router, "the key that is disabled").await;
    let (turned_off, payload) = kms(router, "DisableKey", &json!({"KeyId": disabled_id})).await;
    assert_eq!(
        turned_off, 200,
        "disabling the key must succeed, got {turned_off} with {payload}"
    );

    let (pending_id, pending_arn) = created_key(router, "the key scheduled for deletion").await;
    let (scheduled, payload) = kms(
        router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": pending_id, "PendingWindowInDays": 7}),
    )
    .await;
    assert_eq!(
        scheduled, 200,
        "scheduling the key's deletion must succeed, got {scheduled} with {payload}"
    );

    let importing_arn = created_external_key(router).await;

    vec![
        (
            "a key that is disabled",
            disabled_arn.clone(),
            "Disabled",
            KMS_DISABLED_CODE,
            format!("{disabled_arn} is disabled."),
        ),
        (
            "a key whose deletion is scheduled",
            pending_arn.clone(),
            "PendingDeletion",
            KMS_INVALID_STATE_CODE,
            format!("{pending_arn} is pending deletion."),
        ),
        (
            "a key still awaiting its imported material",
            importing_arn.clone(),
            "PendingImport",
            KMS_INVALID_STATE_CODE,
            format!("{importing_arn} is pending import."),
        ),
    ]
}

/// The encryption headers a write naming `key_arn` for SSE-KMS carries.
fn sse_kms_headers(key_arn: &str) -> Vec<(&'static str, &str)> {
    vec![(ENCRYPTION_HEADER, KMS_ALGORITHM), (KEY_ID_HEADER, key_arn)]
}

#[tokio::test]
async fn the_relayed_kms_exception_is_the_one_the_key_s_own_state_raises() {
    let router = router_with_kms();
    let name = "relay-error-class";
    bucket(&router, name).await;
    // The source of every copy: an object written under the bucket's own default encryption,
    // so a refused copy is refused for its DESTINATION's key and nothing else.
    let source = put_object(&router, name, "source.bin", BODY, &[]).await;
    assert_eq!(
        source.status, 200,
        "the copy source must be stored first, got {} with body {:?}",
        source.status,
        text(&source)
    );

    for (case, key_arn, key_state, code, message) in unusable_rows(&router).await {
        // The row's premise, as KMS itself reports it: the key really is in that state.
        let reported = described_key_member(&router, &key_arn, "KeyState").await;
        assert_eq!(
            reported, key_state,
            "case {case:?}: KMS must report the key state {key_state:?}, got {reported:?}"
        );

        let headers = sse_kms_headers(&key_arn);
        for (operation, object_key, refused) in [
            (
                "PutObject",
                format!("refused/{key_state}-put.bin"),
                put_object(
                    &router,
                    name,
                    &format!("refused/{key_state}-put.bin"),
                    BODY,
                    &headers,
                )
                .await,
            ),
            (
                "CopyObject",
                format!("refused/{key_state}-copy.bin"),
                copy_object(
                    &router,
                    name,
                    &format!("refused/{key_state}-copy.bin"),
                    &format!("/{name}/source.bin"),
                    &headers,
                )
                .await,
            ),
            (
                "CreateMultipartUpload",
                format!("refused/{key_state}-upload.bin"),
                create_multipart_upload(
                    &router,
                    name,
                    &format!("refused/{key_state}-upload.bin"),
                    &headers,
                )
                .await,
            ),
        ] {
            assert_eq!(
                refused.status, KMS_RELAY_STATUS,
                "case {case:?}/{operation}: expected HTTP {KMS_RELAY_STATUS}, got {} with body \
                 {:?}",
                refused.status,
                text(&refused)
            );
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}/{operation}: expected the code {code:?}, got {:?} ({:?})",
                refused.error_code(),
                text(&refused)
            );
            assert_eq!(
                refused.error_message(),
                message,
                "case {case:?}/{operation}: expected the message {message:?}, got {:?}",
                refused.error_message()
            );

            // Nothing was stored under the refused key: the relay happens before the write.
            let read = get_object(&router, name, &object_key, "", &[]).await;
            assert_eq!(
                read.status, 404,
                "case {case:?}/{operation}: the refused write must leave nothing at \
                 {object_key:?}, got {} with body {:?}",
                read.status,
                text(&read)
            );
        }
    }
}

#[tokio::test]
async fn a_key_whose_state_is_restored_serves_the_write_the_state_refused() {
    // The refusal is about the key STATE, not about the key: the one key refuses the write
    // while it is disabled and serves the identical write once `EnableKey` restores it.
    let router = router_with_kms();
    let name = "relay-error-class-restored";
    bucket(&router, name).await;
    let (key_id, key_arn) = created_key(&router, "the key disabled and then enabled again").await;
    let headers = sse_kms_headers(&key_arn);

    let (turned_off, payload) = kms(&router, "DisableKey", &json!({"KeyId": key_id})).await;
    assert_eq!(
        turned_off, 200,
        "case disabled: disabling the key must succeed, got {turned_off} with {payload}"
    );
    let refused = put_object(&router, name, "restored.bin", BODY, &headers).await;
    assert_eq!(
        refused.error_code(),
        KMS_DISABLED_CODE,
        "case disabled: expected the code {KMS_DISABLED_CODE:?}, got {:?} ({:?})",
        refused.error_code(),
        text(&refused)
    );
    assert_eq!(
        refused.error_message(),
        format!("{key_arn} is disabled."),
        "case disabled: expected KMS's own sentence for the state, got {:?}",
        refused.error_message()
    );

    let (turned_on, payload) = kms(&router, "EnableKey", &json!({"KeyId": key_id})).await;
    assert_eq!(
        turned_on, 200,
        "case enabled: enabling the key must succeed, got {turned_on} with {payload}"
    );
    let served = put_object(&router, name, "restored.bin", BODY, &headers).await;
    assert_eq!(
        served.status, 200,
        "case enabled: the identical write must be served, got {} with body {:?}",
        served.status,
        text(&served)
    );
    assert_eq!(
        served.header(KEY_ID_HEADER).as_deref(),
        Some(key_arn.as_str()),
        "case enabled: the served write must name the key ARN {key_arn:?}, got {:?}",
        served.header(KEY_ID_HEADER)
    );
}
