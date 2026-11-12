// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the Device Farm **device pool** operations:
//! `CreateDevicePool` and `GetDevicePool`.
//!
//! Every case drives the real wire path through the shared harness in
//! [`crate::projects`] — `POST /` with `X-Amz-Target: DeviceFarm_20150623.<Operation>`
//! through the multi-service router — so what is asserted is what an SDK client sees.
//!
//! The tables below span the classes the rules generalise over: how many rules the request
//! carries (none, one, several), which rule members are present, which attribute/operator
//! pair each rule uses, whether a device ceiling was asked for and its value, the name and
//! description at their modelled length bounds, the type reported for a created pool, and
//! the region the request is signed for (including none at all).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::projects::{arn_of, error_code, foreign_arn, resource_id, Emulator, DEFAULT_REGION, REGION};

/// The members a `DevicePool` response carries besides its ARN, so a row asserts the WHOLE
/// reported pool rather than the one member it varies. From
/// `smithy_query(query="shape",shape="DevicePool")`.
const POOL_MEMBERS: [&str; 5] = ["name", "description", "type", "rules", "maxDevices"];

/// The resource type a device-pool ARN carries, as it appears in the ARN's resource field.
const DEVICE_POOL_TYPE: &str = "devicepool";

/// Rules spanning the attribute/operator pairs the model declares, one row per form a rule
/// can take.
///
/// The attributes and operators are the model's own enum values, read with
/// `smithy_query(query="shape",shape="DeviceAttribute,RuleOperator")`: `DeviceAttribute` is
/// one of ARN, PLATFORM, FORM_FACTOR, MANUFACTURER, REMOTE_ACCESS_ENABLED,
/// REMOTE_DEBUG_ENABLED, APPIUM_VERSION, INSTANCE_ARN, INSTANCE_LABELS, FLEET_TYPE,
/// OS_VERSION, MODEL, AVAILABILITY, and `RuleOperator` one of EQUALS, LESS_THAN,
/// LESS_THAN_OR_EQUALS, GREATER_THAN, GREATER_THAN_OR_EQUALS, IN, NOT_IN, CONTAINS. A rule
/// value is a stringified attribute value, so it is a plain string on the wire.
fn rule_catalog() -> Vec<Value> {
    vec![
        json!({ "attribute": "PLATFORM", "operator": "EQUALS", "value": "\"ANDROID\"" }),
        json!({ "attribute": "OS_VERSION", "operator": "GREATER_THAN_OR_EQUALS", "value": "\"11\"" }),
        json!({ "attribute": "FORM_FACTOR", "operator": "NOT_IN", "value": "[\"TABLET\"]" }),
        json!({ "attribute": "AVAILABILITY", "operator": "EQUALS", "value": "\"HIGHLY_AVAILABLE\"" }),
        json!({ "attribute": "INSTANCE_LABELS", "operator": "CONTAINS", "value": "\"golden\"" }),
        json!({ "attribute": "MANUFACTURER", "operator": "IN", "value": "[\"Google\",\"Samsung\"]" }),
        json!({ "attribute": "REMOTE_ACCESS_ENABLED", "operator": "EQUALS", "value": "\"true\"" }),
        json!({ "attribute": "MODEL", "operator": "LESS_THAN", "value": "\"Pixel 9\"" }),
    ]
}

/// The first `n` rules of [`rule_catalog`], for the list-size rows.
fn rules(n: usize) -> Value {
    Value::Array(rule_catalog().into_iter().take(n).collect())
}

/// The pool a `CreateDevicePool` request resolves to, patched with whatever the row expects
/// differently.
///
/// A created pool's type is `PRIVATE` — one of the two `DevicePoolType` values, the model
/// documenting `CURATED` as "a device pool that is created and managed by AWS Device Farm"
/// and `PRIVATE` as one "created and managed by the device pool developer".
fn expected_pool(name: &str, rules: Value, patches: Value) -> Value {
    let mut pool = json!({ "name": name, "type": "PRIVATE", "rules": rules });
    for (member, value) in patches
        .as_object()
        .unwrap_or_else(|| panic!("the patch is an object: {patches}"))
    {
        pool[member] = value.clone();
    }
    pool
}

/// CreateDevicePool mints a pool under an existing project, keeps the caller's rules and
/// device ceiling exactly as supplied, classifies the pool as PRIVATE, and GetDevicePool
/// answers with the IDENTICAL stored pool.
#[tokio::test]
async fn created_device_pools_are_persisted_and_read_back_identically() {
    let emu = Emulator::new();

    // (case, signing region, CreateDevicePool input beyond projectArn, expected pool)
    let cases: Vec<(String, Option<&str>, Value, Value)> = vec![
        (
            "only the required members, with an empty rule list".to_string(),
            REGION,
            json!({ "name": "no-rules", "rules": [] }),
            expected_pool("no-rules", json!([]), json!({})),
        ),
        (
            "a single rule, no device ceiling".to_string(),
            REGION,
            json!({ "name": "one-rule", "rules": rules(1) }),
            expected_pool("one-rule", rules(1), json!({})),
        ),
        (
            "every attribute/operator pair in the catalog, in order".to_string(),
            REGION,
            json!({ "name": "every-rule", "rules": rules(8) }),
            expected_pool("every-rule", rules(8), json!({})),
        ),
        (
            "the same rules in the REVERSE order: the order asked for is kept".to_string(),
            REGION,
            json!({
                "name": "reversed-rules",
                "rules": Value::Array(rule_catalog().into_iter().rev().collect())
            }),
            expected_pool(
                "reversed-rules",
                Value::Array(rule_catalog().into_iter().rev().collect()),
                json!({}),
            ),
        ),
        (
            "a rule carrying only its attribute: the absent members stay absent".to_string(),
            REGION,
            json!({ "name": "partial-rule", "rules": [{ "attribute": "FLEET_TYPE" }] }),
            expected_pool(
                "partial-rule",
                json!([{ "attribute": "FLEET_TYPE" }]),
                json!({}),
            ),
        ),
        (
            "a rule carrying only its value".to_string(),
            Some("eu-west-1"),
            json!({ "name": "value-only-rule", "rules": [{ "value": "\"PUBLIC\"" }] }),
            expected_pool(
                "value-only-rule",
                json!([{ "value": "\"PUBLIC\"" }]),
                json!({}),
            ),
        ),
        (
            "a device ceiling of one device".to_string(),
            REGION,
            json!({ "name": "single-device", "rules": rules(2), "maxDevices": 1 }),
            expected_pool("single-device", rules(2), json!({ "maxDevices": 1 })),
        ),
        (
            "a device ceiling far above any fleet".to_string(),
            Some("ap-south-1"),
            json!({ "name": "wide-open", "rules": rules(3), "maxDevices": 2_147_483_647_i64 }),
            expected_pool(
                "wide-open",
                rules(3),
                json!({ "maxDevices": 2_147_483_647_i64 }),
            ),
        ),
        (
            "a device ceiling of zero: the number asked for is reported, not replaced"
                .to_string(),
            REGION,
            json!({ "name": "zero-devices", "rules": rules(1), "maxDevices": 0 }),
            expected_pool("zero-devices", rules(1), json!({ "maxDevices": 0 })),
        ),
        (
            "a description, and an empty name (modelled Name length minimum of 0)".to_string(),
            REGION,
            json!({ "name": "", "rules": rules(2), "description": "the android phones" }),
            expected_pool(
                "",
                rules(2),
                json!({ "description": "the android phones" }),
            ),
        ),
        (
            "name and description at their modelled length maxima (256 and 16384)".to_string(),
            REGION,
            json!({
                "name": "n".repeat(256),
                "rules": rules(1),
                "description": "d".repeat(16384),
                "maxDevices": 5
            }),
            expected_pool(
                &"n".repeat(256),
                rules(1),
                json!({ "description": "d".repeat(16384), "maxDevices": 5 }),
            ),
        ),
        (
            "unsigned request: the ARN carries the default region".to_string(),
            None,
            json!({ "name": "unsigned-caller", "rules": rules(1) }),
            expected_pool("unsigned-caller", rules(1), json!({})),
        ),
    ];

    let mut arns: BTreeSet<String> = BTreeSet::new();
    for (case, region, input, expected) in &cases {
        // The owning project is created by the SAME caller, so its ARN and the pool's
        // share a region.
        let (status, response) = emu
            .call("CreateProject", *region, &json!({ "name": "device-pool-owner" }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateProject to answer 200, got {status:?} with {response}"
        );
        let project_arn = arn_of(&response["project"]);

        let mut request = input.clone();
        request["projectArn"] = json!(project_arn);
        let (status, response) = emu.call("CreateDevicePool", *region, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDevicePool to answer 200, got {status:?} with \
             {response}"
        );

        let pool = response["devicePool"].clone();
        let arn = arn_of(&pool);
        let expected_prefix = format!(
            "arn:aws:devicefarm:{}:{}:{DEVICE_POOL_TYPE}:{}/",
            region.unwrap_or(DEFAULT_REGION),
            engine::EMULATOR_ACCOUNT_ID,
            resource_id(&project_arn)
        );
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case:?}: expected an ARN starting {expected_prefix:?}, got {arn:?}"
        );
        assert!(
            arn.len() >= 32 && arn.len() <= 1011,
            "case {case:?}: expected the ARN within the modelled length 32..1011, got {:?}",
            arn.len()
        );
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: expected a fresh ARN, got one already minted: {arn:?}"
        );

        for member in POOL_MEMBERS {
            assert_eq!(
                pool[member], expected[member],
                "case {case:?}: expected {member} {:?}, got {:?}",
                expected[member], pool[member]
            );
        }

        // The round trip: GetDevicePool reports the persisted record, byte for byte, and
        // reading it twice cannot change what the next caller sees.
        for attempt in 1..=2 {
            let (status, response) = emu
                .call("GetDevicePool", REGION, &json!({ "arn": arn }))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} read {attempt}: expected GetDevicePool to answer 200, got \
                 {status:?} with {response}"
            );
            assert_eq!(
                response["devicePool"], pool,
                "case {case:?} read {attempt}: expected GetDevicePool to return {pool}, got {}",
                response["devicePool"]
            );
        }
    }
}

/// A device pool is a resource of its own: deleting the project it was created in releases
/// the project only, so every pool created under it still reads back unchanged, and a new
/// pool can still be created under that released project's ARN — the create is judged on
/// the ARN's form, not on the project record still being there.
#[tokio::test]
async fn device_pools_outlive_the_project_they_were_created_in() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "doomed-parent" })).await);

    // (case, CreateDevicePool input beyond projectArn)
    let inputs: Vec<(&str, Value)> = vec![
        ("a pool with no rules", json!({ "name": "survivor-a", "rules": [] })),
        (
            "a pool with rules and a ceiling",
            json!({ "name": "survivor-b", "rules": rules(4), "maxDevices": 7 }),
        ),
        (
            "a pool with a description",
            json!({ "name": "survivor-c", "rules": rules(1), "description": "kept after delete" }),
        ),
    ];

    let mut created: Vec<(&str, String, Value)> = Vec::new();
    for (case, input) in &inputs {
        let mut request = input.clone();
        request["projectArn"] = json!(project_arn);
        let (status, response) = emu.call("CreateDevicePool", REGION, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateDevicePool to answer 200, got {status:?} with \
             {response}"
        );
        let pool = response["devicePool"].clone();
        created.push((case, arn_of(&pool), pool));
    }

    let (status, response) = emu
        .call("DeleteProject", REGION, &json!({ "arn": project_arn }))
        .await;
    assert_eq!(
        status, 200,
        "case deleting the parent project: expected 200, got {status:?} with {response}"
    );

    for (case, arn, pool) in &created {
        let (status, response) = emu
            .call("GetDevicePool", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} after the parent project was deleted: expected 200, got {status:?} \
             with {response}"
        );
        assert_eq!(
            response["devicePool"], *pool,
            "case {case:?} after the parent project was deleted: expected {pool}, got {}",
            response["devicePool"]
        );
    }

    // The project record really is gone — and a pool is still minted under its id, because
    // a device pool's identity is the project id its ARN carries and nothing of the
    // project record is read.
    let (status, response) = emu
        .call(
            "CreateDevicePool",
            REGION,
            &json!({ "projectArn": project_arn, "name": "after-the-project", "rules": [] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case a new pool for the released project: expected 200, got {status:?} with {response}"
    );
    let orphan_arn = arn_of(&response["devicePool"]);
    let expected_prefix = format!(
        "arn:aws:devicefarm:us-west-2:{}:{DEVICE_POOL_TYPE}:{}/",
        engine::EMULATOR_ACCOUNT_ID,
        resource_id(&project_arn)
    );
    assert!(
        orphan_arn.starts_with(&expected_prefix),
        "case a new pool for the released project: expected an ARN starting \
         {expected_prefix:?}, got {orphan_arn:?}"
    );

    let (status, response) = emu
        .call("GetProject", REGION, &json!({ "arn": project_arn }))
        .await;
    assert_eq!(
        status, 400,
        "case the released project itself: expected a 400 refusal, got {status:?} with {response}"
    );
    assert_eq!(
        error_code(&response),
        "NotFoundException",
        "case the released project itself: expected NotFoundException, got {response}"
    );
}

/// Two pools created with the same name in the same project are different resources: each
/// gets its own ARN and reports its own rules.
#[tokio::test]
async fn pools_sharing_a_name_are_distinct_resources() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "shared-pool-names" })).await);

    let mut created: Vec<(i64, String, Value)> = Vec::new();
    for max_devices in [1_i64, 4] {
        let (status, response) = emu
            .call(
                "CreateDevicePool",
                REGION,
                &json!({
                    "projectArn": project_arn,
                    "name": "same-name",
                    "rules": rules(1),
                    "maxDevices": max_devices
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case maxDevices {max_devices}: expected 200, got {status:?} with {response}"
        );
        let pool = response["devicePool"].clone();
        created.push((max_devices, arn_of(&pool), pool));
    }

    assert_ne!(
        created[0].1, created[1].1,
        "expected two distinct ARNs for two pools sharing a name, got {:?} twice",
        created[0].1
    );
    for (max_devices, arn, pool) in &created {
        let (status, response) = emu
            .call("GetDevicePool", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case maxDevices {max_devices}: expected 200, got {status:?} with {response}"
        );
        assert_eq!(
            response["devicePool"], *pool,
            "case maxDevices {max_devices}: expected {pool}, got {}",
            response["devicePool"]
        );
        assert_eq!(
            response["devicePool"]["maxDevices"].as_i64(),
            Some(*max_devices),
            "case maxDevices {max_devices}: expected maxDevices {max_devices}, got {:?}",
            response["devicePool"]["maxDevices"]
        );
    }
}

/// Every way a device-pool request can name a resource that is not there is refused with
/// the modelled HTTP 400 — never a 501 and never a 200: `ArgumentException` for an ARN that
/// does not name the resource type the member expects, and `NotFoundException` for a
/// well-formed device-pool ARN the store has no record for. A `projectArn` naming a project
/// the store does not hold is NOT one of these: a pool is minted under the project id the
/// ARN carries, which [`device_pools_outlive_the_project_they_were_created_in`] and
/// [`crate::reference_checks`] cover.
#[tokio::test]
async fn device_pool_requests_naming_absent_resources_are_refused() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "pool-refusals" })).await);
    let pool_arn = {
        let (status, response) = emu
            .call(
                "CreateDevicePool",
                REGION,
                &json!({ "projectArn": project_arn, "name": "present", "rules": rules(1) }),
            )
            .await;
        assert_eq!(status, 200, "CreateDevicePool answered {status}: {response}");
        arn_of(&response["devicePool"])
    };

    // (case, operation, request, expected error code)
    let cases: Vec<(String, &str, Value, &str)> = vec![
        (
            "CreateDevicePool given an upload ARN".to_string(),
            "CreateDevicePool",
            json!({ "projectArn": foreign_arn("upload"), "name": "wrong-type", "rules": [] }),
            "ArgumentException",
        ),
        (
            "CreateDevicePool given a device-pool ARN".to_string(),
            "CreateDevicePool",
            json!({ "projectArn": pool_arn.clone(), "name": "wrong-type", "rules": [] }),
            "ArgumentException",
        ),
        (
            "GetDevicePool for a device-pool ARN no pool has".to_string(),
            "GetDevicePool",
            json!({ "arn": foreign_arn(DEVICE_POOL_TYPE) }),
            "NotFoundException",
        ),
        (
            "GetDevicePool given the owning project's ARN".to_string(),
            "GetDevicePool",
            json!({ "arn": project_arn.clone() }),
            "ArgumentException",
        ),
        (
            "GetDevicePool given a network-profile ARN".to_string(),
            "GetDevicePool",
            json!({ "arn": foreign_arn("networkprofile") }),
            "ArgumentException",
        ),
    ];

    for (case, op, request, expected_code) in &cases {
        let (status, response) = emu.call(op, REGION, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status:?} with {response}"
        );
        assert_eq!(
            error_code(&response),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {response}"
        );
    }

    // The refused calls changed nothing: the one pool that exists still reads back.
    let (status, response) = emu
        .call("GetDevicePool", REGION, &json!({ "arn": pool_arn }))
        .await;
    assert_eq!(
        status, 200,
        "case the surviving pool after every refusal: expected 200, got {status:?} with \
         {response}"
    );
    assert_eq!(
        response["devicePool"]["name"],
        json!("present"),
        "case the surviving pool after every refusal: expected name \"present\", got {}",
        response["devicePool"]["name"]
    );
}
