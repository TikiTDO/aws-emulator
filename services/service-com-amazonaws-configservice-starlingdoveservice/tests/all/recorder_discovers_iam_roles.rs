// SPDX-License-Identifier: CC0-1.0

//! Where an AGGREGATOR's resources come from: the GLOBAL resources the account holds in
//! IAM, which a running configuration recorder observes through IAM's own API.
//!
//! The rule under test: **while a configuration recorder is recording and its recording
//! group covers a resource type, the resources of that type the account holds are admitted
//! to Config's discovered-resource inventory as the owning service reports them — a GLOBAL
//! type only where the group asks for the global resource types or names the type itself —
//! and every aggregate read of an aggregator whose sources name this account and Region
//! serves that one inventory: the single read resolves such a resource and the batch read
//! reports one `BaseConfigurationItem` per identifier it resolves, leaving only the
//! identifiers it does not as unprocessed.**
//!
//! An IAM role is the global resource these rows are read over. Config identifies a role by
//! the `RoleId` IAM minted for it — the id that survives a rename — reports the role name
//! as the resource's custom name, and reports the ARN IAM minted as the item's own `arn`.
//!
//! Config and IAM are mounted on ONE engine, reusing the harness
//! `conformance_pack_rule_evaluation` builds: the recorder reads the roles it records from
//! IAM's own API, so without IAM mounted there is nothing to discover.
//!
//! The rows differ along what the rule generalises over: which recording group the recorder
//! records with, whether it is recording at all, how many roles the account holds, whether
//! an identifier names a role the account holds, and which of the two aggregate reads asks.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::conformance_pack_rule_evaluation::{
    Emulator, CONVERGENCE_BUDGET, POLL_INTERVAL, REGION,
};

/// The resource type Config records an IAM role under, an entry of
/// `com.amazonaws.configservice#ResourceType`.
const IAM_ROLE_RESOURCE_TYPE: &str = "AWS::IAM::Role";

/// The aggregator every row reads through.
const AGGREGATOR: &str = "the-aggregator";

/// The error a single aggregate read answers for a resource the aggregator holds nothing
/// of (`GetAggregateResourceConfig` models it: "you have specified a resource that is
/// either unknown or has not been discovered").
const NOT_DISCOVERED: &str = "ResourceNotDiscoveredException";

/// How long a row watches for an admission it does not expect — several passes' worth of
/// time, so an admission that was going to happen would have happened.
const QUIET_BUDGET: Duration = Duration::from_millis(2_500);

/// The trust document every role below carries, which `CreateRole` requires.
fn trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "ec2.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Create `role_name` through IAM's own API and answer its `RoleId` and its ARN — the two
/// values IAM mints, which a configuration item records as the resource's id and its ARN.
async fn create_role(case: &str, emulator: &Emulator, role_name: &str) -> (String, String) {
    let created = emulator
        .iam()
        .create_role()
        .role_name(role_name)
        .assume_role_policy_document(trust())
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: creating {role_name} must succeed, got {error:?}")
        });
    let role = created
        .role
        .unwrap_or_else(|| panic!("case {case:?}: CreateRole must answer the role"));
    (role.role_id, role.arn)
}

/// An aggregator that aggregates THIS account in every Region.
async fn put_aggregator(emulator: &Emulator) {
    emulator
        .ok(
            "PutConfigurationAggregator",
            &json!({
                "ConfigurationAggregatorName": AGGREGATOR,
                "AccountAggregationSources": [{
                    "AccountIds": [engine::emulator_account_id()],
                    "AllAwsRegions": true,
                }],
            }),
        )
        .await;
}

/// One aggregate resource identifier naming this account's data in the Region the harness
/// signs for.
fn identifier(resource_id: &str) -> Value {
    json!({
        "SourceAccountId": engine::emulator_account_id(),
        "SourceRegion": REGION,
        "ResourceType": IAM_ROLE_RESOURCE_TYPE,
        "ResourceId": resource_id,
    })
}

/// Read `resource_id` through the single aggregate read until it resolves, and answer the
/// configuration item it reported.
async fn await_aggregated(case: &str, emulator: &Emulator, resource_id: &str) -> Value {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let (status, body) = emulator
            .call(
                "GetAggregateResourceConfig",
                &json!({
                    "ConfigurationAggregatorName": AGGREGATOR,
                    "ResourceIdentifier": identifier(resource_id),
                }),
            )
            .await;
        if status == 200 {
            return body["ConfigurationItem"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {resource_id} to be discovered, got {status} with {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Assert that `resource_id` stays undiscovered for as long as [`QUIET_BUDGET`], and that
/// the read says so with the modelled refusal rather than an empty success.
async fn stays_undiscovered(case: &str, emulator: &Emulator, resource_id: &str) {
    let deadline = Instant::now() + QUIET_BUDGET;
    loop {
        let (status, body) = emulator
            .call(
                "GetAggregateResourceConfig",
                &json!({
                    "ConfigurationAggregatorName": AGGREGATOR,
                    "ResourceIdentifier": identifier(resource_id),
                }),
            )
            .await;
        let code = body["__type"]
            .as_str()
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
            .unwrap_or_default();
        assert_eq!(
            (status, code.as_str()),
            (400, NOT_DISCOVERED),
            "case {case:?}: expected {resource_id} to stay undiscovered, got {status} with {body}"
        );
        if Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_role_iam_holds_is_served_by_every_aggregate_read() {
    let emulator = Emulator::new();
    emulator.recording().await;
    put_aggregator(&emulator).await;

    // Both roles are created through IAM's own API. Config is told nothing about either.
    let case = "two roles the account holds";
    let (first_id, first_arn) = create_role(case, &emulator, "aggregated-first").await;
    let (second_id, second_arn) = create_role(case, &emulator, "aggregated-second").await;

    // The rows the single read is checked over: each role, by the id IAM minted for it.
    let expected: [(&str, &str, &str, &str); 2] = [
        ("the first role", "aggregated-first", &first_id, &first_arn),
        ("the second role", "aggregated-second", &second_id, &second_arn),
    ];
    for (row, role_name, role_id, role_arn) in expected {
        let item = await_aggregated(row, &emulator, role_id).await;
        assert_eq!(
            item["resourceType"].as_str(),
            Some(IAM_ROLE_RESOURCE_TYPE),
            "case {row:?}: the item names the type the role is recorded under, got {item}"
        );
        assert_eq!(
            item["resourceId"].as_str(),
            Some(role_id),
            "case {row:?}: a role is identified by its RoleId, got {item}"
        );
        assert_eq!(
            item["resourceName"].as_str(),
            Some(role_name),
            "case {row:?}: the role's name is its resource name, got {item}"
        );
        assert_eq!(
            item["arn"].as_str(),
            Some(role_arn),
            "case {row:?}: the item carries the ARN IAM minted, got {item}"
        );
        assert_eq!(
            item["accountId"].as_str(),
            Some(engine::emulator_account_id()),
            "case {row:?}: the item names this account, got {item}"
        );
        assert_eq!(
            item["awsRegion"].as_str(),
            Some(REGION),
            "case {row:?}: the item names the Region it is reported under, got {item}"
        );
        // The configuration document is the one IAM's own read reports, so the role's
        // immutable id and its name are IN it as well as on the item.
        let configuration: Value = serde_json::from_str(
            item["configuration"]
                .as_str()
                .unwrap_or_else(|| panic!("case {row:?}: expected a configuration, got {item}")),
        )
        .unwrap_or_else(|_| panic!("case {row:?}: the configuration is a JSON document"));
        assert_eq!(
            (
                configuration["roleId"].as_str(),
                configuration["roleName"].as_str(),
                configuration["arn"].as_str()
            ),
            (Some(role_id), Some(role_name), Some(role_arn)),
            "case {row:?}: the configuration is the role IAM reports, got {configuration}"
        );
    }

    // The batch read serves the SAME inventory: the rows differ along which identifiers it
    // is given — both roles, one of them, one role and one id the account holds no role
    // for, and an id alone.
    let absent = "AROAABSENTROLEIDXXXXX";
    let cases: [(&str, Vec<&str>, usize, usize); 4] = [
        ("both roles", vec![&first_id, &second_id], 2, 0),
        ("one role", vec![&second_id], 1, 0),
        (
            "one role and one id nothing holds",
            vec![&first_id, absent],
            1,
            1,
        ),
        ("an id nothing holds", vec![absent], 0, 1),
    ];
    for (row, identifiers, resolved, unprocessed) in cases {
        let body = emulator
            .ok(
                "BatchGetAggregateResourceConfig",
                &json!({
                    "ConfigurationAggregatorName": AGGREGATOR,
                    "ResourceIdentifiers": identifiers
                        .iter()
                        .map(|resource_id| identifier(resource_id))
                        .collect::<Vec<Value>>(),
                }),
            )
            .await;
        let items = body["BaseConfigurationItems"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let left_over = body["UnprocessedResourceIdentifiers"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            (items.len(), left_over.len()),
            (resolved, unprocessed),
            "case {row:?}: expected {resolved} items and {unprocessed} unprocessed \
             identifiers, got {body}"
        );
        // Every item the batch read reports is the one the single read reports for the
        // same identifier: one inventory, two reads of it.
        for item in &items {
            let resource_id = item["resourceId"].as_str().unwrap_or_default();
            let single = await_aggregated(row, &emulator, resource_id).await;
            for member in ["resourceType", "resourceId", "resourceName", "arn", "accountId"] {
                assert_eq!(
                    item[member], single[member],
                    "case {row:?}: expected the batch read's {member} to match the single \
                     read's, got {item} and {single}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_recording_group_decides_whether_a_global_type_is_discovered() {
    // The rows differ along which recording group the recorder records with — the global
    // resource types asked for, not asked for, the type enumerated, the type excluded —
    // and along whether the recorder is recording at all.
    let cases: [(&str, Value, bool, bool); 5] = [
        (
            "all supported types, with the global ones asked for",
            json!({ "allSupported": true, "includeGlobalResourceTypes": true }),
            true,
            true,
        ),
        (
            "all supported types, with the global ones not asked for",
            json!({ "allSupported": true }),
            true,
            false,
        ),
        (
            "an inclusion group naming this type",
            json!({ "resourceTypes": [IAM_ROLE_RESOURCE_TYPE] }),
            true,
            true,
        ),
        (
            "an exclusion group excluding this type",
            json!({ "exclusionByResourceTypes": { "resourceTypes": [IAM_ROLE_RESOURCE_TYPE] } }),
            true,
            false,
        ),
        (
            "a recorder that was never started",
            json!({ "allSupported": true, "includeGlobalResourceTypes": true }),
            false,
            false,
        ),
    ];

    for (case, group, recording, discovered) in cases {
        let emulator = Emulator::new();
        let role = emulator.config_role().await;
        emulator
            .ok(
                "PutConfigurationRecorder",
                &json!({
                    "ConfigurationRecorder": {
                        "name": "the-recorder",
                        "roleARN": role,
                        "recordingGroup": group,
                    }
                }),
            )
            .await;
        emulator
            .ok(
                "PutDeliveryChannel",
                &json!({ "DeliveryChannel": { "s3BucketName": "config-delivery" } }),
            )
            .await;
        if recording {
            emulator
                .ok(
                    "StartConfigurationRecorder",
                    &json!({ "ConfigurationRecorderName": "the-recorder" }),
                )
                .await;
        }
        put_aggregator(&emulator).await;
        let (role_id, _) = create_role(case, &emulator, "grouped-role").await;

        if discovered {
            await_aggregated(case, &emulator, &role_id).await;
        } else {
            stays_undiscovered(case, &emulator, &role_id).await;
        }
    }
}
