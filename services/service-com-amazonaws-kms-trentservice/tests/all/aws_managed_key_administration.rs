// SPDX-License-Identifier: CC0-1.0

//! Who may ADMINISTER a key Amazon Web Services manages, driven over the wire through the
//! shared [`super::harness`].
//!
//! RULE: a key whose `KeyManager` is `AWS` is one Amazon Web Services manages rather than one
//! the customer created — "KMS keys in your Amazon Web Services account are either customer
//! managed or Amazon Web Services managed"
//! (`smithy_query(query="doc",shape="KeyMetadata",member="KeyManager")`) — and the manager of
//! a key is who administers it: the key policy such a key carries is the owning service's
//! own, written to let that service use the key for the account's resources and to let the
//! account read the key, so it delegates no administration of the key to the account. Every
//! call the account makes that would CHANGE such a key — its key state, its key material, its
//! key policy, its description, its tags or its multi-Region layout — is therefore refused
//! for want of permission: HTTP 400 `AccessDeniedException` carrying AWS's denial sentence for
//! an action no resource-based policy allows, whose variable parts are the caller it refused,
//! the `kms:` action it asked for and the key ARN it asked for it on — and the key is left
//! exactly as it was. READING such a key stays open to the account: `DescribeKey`,
//! `GetKeyPolicy`, `GetKeyRotationStatus` and `ListResourceTags` all answer.
//!
//! The rows differ along the OPERATION — the dimension the rule generalises over, walked
//! across every operation of this service that administers an existing key — and along which
//! managed key they are aimed at, since the rule is about the manager rather than about one
//! service's key. The control beside each row is the identical call on a CUSTOMER managed
//! key, which is never refused for want of permission: it is served, or refused on a property
//! of that key itself (its single-Region layout, or the KMS-generated material an import
//! cannot replace).
//!
//! The two rotation-configuration operations are deliberately NOT in this table: KMS "always
//! rotates the key material of Amazon Web Services managed keys every year" and "you cannot
//! enable or disable automatic rotation of Amazon Web Services managed KMS keys"
//! (`smithy_query(query="doc",shape="EnableKeyRotation")`), which is a documented
//! `UnsupportedOperationException` rather than an authorization failure — the suite beside
//! this one ([`super::aws_managed_keys`]) spans it.

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, create_key, describe, ok, refused, router, text, REGION};

/// The predefined Amazon Web Services aliases the managed keys under test are materialised
/// through: such an alias has "the format aws/<service-name>"
/// (`smithy_query(query="doc",shape="ListAliases")`), and `DescribeKey` on one associates it
/// with the Amazon Web Services managed key of that service.
const MANAGED_ALIASES: [&str; 2] = ["alias/aws/s3", "alias/aws/ebs"];

/// The `KeyManager` KMS reports for a key Amazon Web Services manages
/// (`smithy_query(query="shape",shape="KeyManagerType")`).
const KEY_MANAGER_AWS: &str = "AWS";

/// The code AWS answers an authorization failure with, whatever this service's model declares.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The caller every request below is signed as, which is the principal a refusal names.
fn caller() -> String {
    state::caller_identity_arn()
}

/// A key policy that keeps the account's own administration of a customer managed key: the
/// default document's one statement, naming the account root.
fn account_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Id": "key-default-1",
        "Statement": [{
            "Sid": "Enable IAM User Permissions",
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
            "Action": "kms:*",
            "Resource": "*",
        }],
    })
    .to_string()
}

/// The request one administration operation makes of the key `key_id`.
///
/// Every member beyond `KeyId` is the operation's own required input
/// (`smithy_query(query="operation",operation="<Operation>")`); none of them is what the row
/// is about, since the decision under test is made before the key is changed.
fn request(operation: &str, key_id: &str) -> Value {
    match operation {
        "ScheduleKeyDeletion" => json!({"KeyId": key_id, "PendingWindowInDays": 7}),
        "PutKeyPolicy" => json!({"KeyId": key_id, "Policy": account_policy()}),
        "UpdateKeyDescription" => json!({"KeyId": key_id, "Description": "renamed by the account"}),
        "TagResource" => {
            json!({"KeyId": key_id, "Tags": [{"TagKey": "Project", "TagValue": "inventory"}]})
        }
        "UntagResource" => json!({"KeyId": key_id, "TagKeys": ["Project"]}),
        "ReplicateKey" => json!({"KeyId": key_id, "ReplicaRegion": "eu-west-1"}),
        "UpdatePrimaryRegion" => json!({"KeyId": key_id, "PrimaryRegion": "eu-west-1"}),
        // The blobs are base64 placeholders: an import of material into a key the caller may
        // not administer is refused before anything about the material is read.
        "ImportKeyMaterial" => {
            json!({"KeyId": key_id, "ImportToken": "AAEC", "EncryptedKeyMaterial": "AAEC"})
        }
        _ => json!({"KeyId": key_id}),
    }
}

/// One row: the operation, and whether the identical call on a fresh CUSTOMER managed key is
/// SERVED rather than refused on a property of that key.
///
/// The operations are every one of this service's that changes an existing key. The ones whose
/// control is not served are refused for a reason of their own on a plain single-Region key
/// whose material KMS generated: there is no deletion to cancel, no set to replicate or
/// re-designate, and no imported material to install or delete.
const ADMINISTRATION: [(&str, bool); 12] = [
    ("DisableKey", true),
    ("EnableKey", true),
    ("ScheduleKeyDeletion", true),
    ("CancelKeyDeletion", false),
    ("PutKeyPolicy", true),
    ("UpdateKeyDescription", true),
    ("TagResource", true),
    ("UntagResource", true),
    ("ReplicateKey", false),
    ("UpdatePrimaryRegion", false),
    ("ImportKeyMaterial", false),
    ("DeleteImportedKeyMaterial", false),
];

/// The managed key behind `alias`, as `(key id, key ARN)` — materialised and asserted to be
/// one Amazon Web Services manages, enabled, through KMS's own `DescribeKey`.
async fn managed_key(router: &MultiServiceRouter, alias: &str) -> (String, String) {
    let metadata = describe(router, alias).await;
    assert_eq!(
        metadata["KeyManager"],
        json!(KEY_MANAGER_AWS),
        "case {alias}: the key of a predefined alias must be managed by Amazon Web Services, \
         got {metadata}"
    );
    assert_eq!(
        metadata["KeyState"],
        json!("Enabled"),
        "case {alias}: the managed key must start enabled, got {metadata}"
    );
    (
        text(alias, &metadata, "KeyId"),
        text(alias, &metadata, "Arn"),
    )
}

#[tokio::test]
async fn an_aws_managed_key_refuses_every_administration_call_the_account_makes() {
    let router = router();

    for alias in MANAGED_ALIASES {
        let (key_id, key_arn) = managed_key(&router, alias).await;
        let before = describe(&router, &key_arn).await;

        for (operation, _) in ADMINISTRATION {
            let case = format!("{alias}/{operation}");
            let (status, code, message) =
                refused(&router, REGION, operation, &request(operation, &key_arn)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} ({code}: {message})"
            );
            assert_eq!(
                code, ACCESS_DENIED,
                "case {case:?}: expected {ACCESS_DENIED}, got {code} ({message})"
            );
            let expected = format!(
                "User: {} is not authorized to perform: kms:{operation} on resource: {key_arn} \
                 because no resource-based policy allows the kms:{operation} action",
                caller()
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected AWS's denial sentence, got {message:?}"
            );

            // The refusal left the key exactly as it was, read back by the id the refused
            // call named and by the key's own ARN.
            for identifier in [key_id.as_str(), key_arn.as_str()] {
                let after = describe(&router, identifier).await;
                assert_eq!(
                    after, before,
                    "case {case:?} read by {identifier}: a refused administration call must \
                     change nothing, got {after}"
                );
            }
        }

        // What the account MAY do with the key it may not administer: read it. Each of these
        // answers, and none of them changes it.
        for (reader, body) in [
            ("GetKeyRotationStatus", json!({"KeyId": key_arn})),
            ("GetKeyPolicy", json!({"KeyId": key_arn, "PolicyName": "default"})),
            ("ListResourceTags", json!({"KeyId": key_arn})),
        ] {
            let (status, payload) = call(&router, REGION, reader, &body).await;
            assert_eq!(
                status, 200,
                "case {alias}/{reader}: a read of the managed key must answer 200, got {status} \
                 with {payload}"
            );
        }
        let after_reads = describe(&router, &key_arn).await;
        assert_eq!(
            after_reads, before,
            "case {alias}: reading the managed key must change nothing, got {after_reads}"
        );
    }
}

#[tokio::test]
async fn a_customer_managed_key_is_never_refused_administration_for_want_of_permission() {
    // The control for every row above: the SAME calls on a key the customer created. None of
    // them is an authorization failure — each is served, or refused on a property of that key.
    let router = router();

    for (operation, served) in ADMINISTRATION {
        let metadata = create_key(
            &router,
            &json!({"Description": format!("the customer key {operation} is asked of")}),
        )
        .await;
        let key_arn = text(operation, &metadata, "Arn");
        // The one row whose control needs a tag to remove: `UntagResource` removes nothing
        // from a key carrying none, which is a success either way, but giving it the tag makes
        // the control a real removal.
        if operation == "UntagResource" {
            ok(
                &router,
                "TagResource",
                &json!({"KeyId": key_arn, "Tags": [{"TagKey": "Project", "TagValue": "inventory"}]}),
            )
            .await;
        }

        let (status, payload) = call(&router, REGION, operation, &request(operation, &key_arn)).await;
        if served {
            assert_eq!(
                status, 200,
                "case {operation}: the identical call on a customer managed key must be served, \
                 got {status} with {payload}"
            );
            continue;
        }
        assert_ne!(
            super::harness::error_code(&payload),
            ACCESS_DENIED,
            "case {operation}: a customer managed key is never refused administration for want \
             of permission, got {payload}"
        );
    }
}
