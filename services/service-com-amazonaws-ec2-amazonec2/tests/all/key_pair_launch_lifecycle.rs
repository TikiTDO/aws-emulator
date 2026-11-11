// SPDX-License-Identifier: CC0-1.0

//! The KEY PAIR AS A RESOURCE THE REST OF THE SERVICE USES: what a created key pair is good
//! for while it is held, and what its delete does and does not change — driven end to end
//! over the wire the service speaks (`ec2Query`, through the shared [`super::ec2_query`]
//! harness).
//!
//! The rule under test is the key pair's LIFECYCLE as seen from outside the key-pair
//! operations themselves: a key pair the account created is reported by `DescribeKeyPairs`
//! and is usable as `RunInstances`' "name of the key pair" (`RunInstancesRequest$KeyName`),
//! which the launch and `DescribeInstances` then report for that instance; deleting it —
//! "deletes the specified key pair, by removing the public key from Amazon EC2"
//! (`DeleteKeyPair`) — removes only EC2's copy of the public key, so the name stops resolving
//! for `DescribeKeyPairs` while an instance already launched with it keeps reporting the name
//! it was launched with, and the name is free for a `CreateKeyPair` to take again, which mints
//! a DIFFERENT key pair under it.
//!
//! That is the lifecycle an end-to-end deploy driver walks: create a key pair, launch with it,
//! tear it down, and run again. `CreateKeyPair`'s own suite is [`super::key_pairs`], the
//! import's is [`super::import_key_pair`] and the delete's refusals are
//! [`super::delete_key_pair`]; nothing here repeats those — this file is about what holding
//! and then not holding a key pair means to the launch path.
//!
//! Every table row is a distinct input along what the rule generalises over — the key's type
//! and the name's character set — so no row is the specification.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, error_message, indexed};
use super::wire::{is_long_id, param, Param};

/// The Region every call here is signed for: the account's default VPC and its default
/// subnets are seeded for the emulator's default Region, and the launches below go into one
/// of them.
const REGION: &str = "us-east-1";

/// The AMI every launch here names. Which image an instance came from has nothing to do with
/// the key pair it was launched with, so one serves every row.
const AMI: &str = "ami-0123456789abcdef0";

/// The instance type every launch here names.
const INSTANCE_TYPE: &str = "t3.micro";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. Every
/// assertion here goes over the wire, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`, required to succeed: the body it answered.
async fn call_ok(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[Param],
) -> String {
    let (status, body) = super::ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Create one key pair of `key_type` under `key_name` and answer the id the service minted
/// for it.
async fn create_key_pair(
    case: &str,
    router: &MultiServiceRouter,
    key_name: &str,
    key_type: &str,
) -> String {
    let body = call_ok(
        case,
        router,
        "CreateKeyPair",
        &[param("KeyName", key_name), param("KeyType", key_type)],
    )
    .await;
    let key_pair_id = element(&body, "keyPairId")
        .unwrap_or_else(|| panic!("case {case:?}: no <keyPairId> from CreateKeyPair in {body}"));
    assert!(
        is_long_id(ec2::state::KEY_PAIR_ID_PREFIX, &key_pair_id),
        "case {case:?}: expected a service-minted key pair id, got {key_pair_id:?}"
    );
    key_pair_id
}

/// Launch one instance with `key_name` and answer its instance id, having pinned that the
/// launch's own answer reports the key pair the request named.
async fn launch_with_key_pair(case: &str, router: &MultiServiceRouter, key_name: &str) -> String {
    let body = call_ok(
        case,
        router,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("InstanceType", INSTANCE_TYPE),
            param("KeyName", key_name),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ],
    )
    .await;
    let launched_key_name = element(&body, "keyName");
    assert_eq!(
        launched_key_name.as_deref(),
        Some(key_name),
        "case {case:?}: the launch must report the key pair {key_name:?} it was given, got \
         {launched_key_name:?} in {body}"
    );
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: no <instanceId> from RunInstances in {body}"))
}

/// The `keyName` `DescribeInstances` reports for `instance_id`, or `None` when it reports
/// none.
async fn described_instance_key_name(
    case: &str,
    router: &MultiServiceRouter,
    instance_id: &str,
) -> Option<String> {
    let body = call_ok(
        case,
        router,
        "DescribeInstances",
        &[param("InstanceId.1", instance_id)],
    )
    .await;
    element(&body, "keyName")
}

/// The key pair id a name-scoped `DescribeKeyPairs` reports for `key_name`.
async fn described_key_pair_id(case: &str, router: &MultiServiceRouter, key_name: &str) -> String {
    let body = call_ok(
        case,
        router,
        "DescribeKeyPairs",
        &indexed("KeyName", &[key_name]),
    )
    .await;
    element(&body, "keyPairId")
        .unwrap_or_else(|| panic!("case {case:?}: no key pair reported for {key_name:?}: {body}"))
}

/// One row of the lifecycle table: the case, the key pair's name, and its type.
type LifecycleRow = (&'static str, &'static str, &'static str);

/// RULE: a created key pair is reported by `DescribeKeyPairs` and usable as a launch's
/// `KeyName`; its delete removes EC2's copy of the public key — the name stops resolving,
/// an instance already launched with it keeps the name it was launched with, and the name is
/// free to be created again as a DIFFERENT key pair.
#[tokio::test]
async fn a_key_pair_is_usable_by_a_launch_and_its_delete_frees_the_name() {
    let router = emulator();

    // (case, key name, key type) — both modelled key types, and names of different
    // character sets (`CreateKeyPairRequest$KeyName` admits up to 255 ASCII characters).
    let rows: Vec<LifecycleRow> = vec![
        ("an ed25519 key pair", "deploy-key", "ed25519"),
        ("an rsa key pair", "ops_key.1", "rsa"),
        (
            "a name of punctuation and digits",
            "key-2026.06_v2/west",
            "ed25519",
        ),
    ];

    for (case, key_name, key_type) in rows {
        let key_pair_id = create_key_pair(case, &router, key_name, key_type).await;

        // Held: the read reports exactly the key pair the create minted.
        let described = described_key_pair_id(case, &router, key_name).await;
        assert_eq!(
            described, key_pair_id,
            "case {case:?}: expected {key_name:?} to be the key pair {key_pair_id:?}, got \
             {described:?}"
        );

        // Usable: a launch takes the name, and the instance reports it.
        let instance_id = launch_with_key_pair(case, &router, key_name).await;
        let reported = described_instance_key_name(case, &router, &instance_id).await;
        assert_eq!(
            reported.as_deref(),
            Some(key_name),
            "case {case:?}: expected {instance_id} to report the key pair {key_name:?} it \
             was launched with, got {reported:?}"
        );

        // Deleted: the name stops resolving, with EC2's own refusal for a key pair the
        // account does not hold.
        let delete = call_ok(case, &router, "DeleteKeyPair", &[param("KeyName", key_name)]).await;
        assert_eq!(
            element(&delete, "keyPairId").as_deref(),
            Some(key_pair_id.as_str()),
            "case {case:?}: expected the delete to report the removed id {key_pair_id:?}, \
             got body {delete}"
        );
        let (status, body) = super::ec2_query::call(
            &router,
            REGION,
            "DescribeKeyPairs",
            &indexed("KeyName", &[key_name]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from a read of the deleted {key_name:?}, got \
             {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "InvalidKeyPair.NotFound",
            "case {case:?}: expected the code \"InvalidKeyPair.NotFound\" for the deleted \
             {key_name:?}, got {code:?} in {body}"
        );
        let message = error_message(&body);
        let expected = format!("The key pair '{key_name}' does not exist");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        // The instance keeps what it was launched with: the delete removed EC2's copy of the
        // public key, not the launch's record of which key pair it was given.
        let after_delete = described_instance_key_name(case, &router, &instance_id).await;
        assert_eq!(
            after_delete.as_deref(),
            Some(key_name),
            "case {case:?}: expected {instance_id} to still report the key pair \
             {key_name:?} it was launched with, got {after_delete:?}"
        );

        // The name is free again, and what takes it is a DIFFERENT key pair.
        let recreated_id = create_key_pair(case, &router, key_name, key_type).await;
        assert_ne!(
            recreated_id, key_pair_id,
            "case {case:?}: expected the re-created {key_name:?} to be a new key pair, got \
             the removed id {key_pair_id:?} again"
        );
        let described_again = described_key_pair_id(case, &router, key_name).await;
        assert_eq!(
            described_again, recreated_id,
            "case {case:?}: expected {key_name:?} to be the re-created key pair \
             {recreated_id:?}, got {described_again:?}"
        );
    }
}
