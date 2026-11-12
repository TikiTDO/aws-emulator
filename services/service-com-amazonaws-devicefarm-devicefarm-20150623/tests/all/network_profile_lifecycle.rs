// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the mutating half of the Device Farm **network profile**
//! lifecycle: `UpdateNetworkProfile` and `DeleteNetworkProfile`, each read back through
//! `GetNetworkProfile`.
//!
//! The rules under test:
//!
//! * an update applies every member the request supplies onto the stored profile, leaves
//!   the members it does not supply as they stand, and reports the profile as it now is —
//!   so a later read of the same ARN reports the UPDATED resource, never the create-time
//!   one;
//! * a delete releases the record, so the ARN afterwards resolves to nothing for every
//!   operation addressed by it.
//!
//! The rows differ along what those rules generalise over: which member the update
//! supplies (none, one, one direction, all of them), the values at both documented bounds,
//! a name at both ends of its modelled length range, the type the request asks for, and
//! how many updates are applied in sequence — so no single request is the specification.
//!
//! The wire harness is the one [`crate::projects`] owns and every suite in this crate
//! shares.

use serde_json::{json, Value};

use crate::projects::{arn_of, error_code, Emulator, REGION};

/// The members a `NetworkProfile` response carries besides its ARN, so a row asserts the
/// WHOLE reported profile rather than the one member it varies.
const PROFILE_MEMBERS: [&str; 11] = [
    "name",
    "description",
    "type",
    "uplinkBandwidthBits",
    "downlinkBandwidthBits",
    "uplinkDelayMs",
    "downlinkDelayMs",
    "uplinkJitterMs",
    "downlinkJitterMs",
    "uplinkLossPercent",
    "downlinkLossPercent",
];

/// The two members the model gives a `@default` of 0 and marks required
/// (`smithy_query(query="operation",operation="UpdateNetworkProfile")`: `uplinkLossPercent`
/// and `downlinkLossPercent`, `PercentInteger`, range 0..100, default 0). A request that
/// names neither still carries the default for both, so an update always resolves them —
/// unlike the members the model leaves optional, which an update that omits them leaves
/// alone.
const DEFAULTED_MEMBERS: [&str; 2] = ["uplinkLossPercent", "downlinkLossPercent"];

/// The shaping every profile in this suite starts from: no member sits on a default, so
/// any member an update fails to carry through is visible.
fn created_request(name: &str) -> Value {
    json!({
        "name": name,
        "description": "the profile as created",
        "uplinkBandwidthBits": 1_000_000_i64,
        "downlinkBandwidthBits": 2_000_000_i64,
        "uplinkDelayMs": 11,
        "downlinkDelayMs": 22,
        "uplinkJitterMs": 3,
        "downlinkJitterMs": 4,
        "uplinkLossPercent": 5,
        "downlinkLossPercent": 6
    })
}

/// The profile an update carrying `patch` leaves behind, given the profile `before` it is
/// applied to: a supplied member replaces the stored one, an omitted optional member keeps
/// it, the two defaulted members resolve to what the request carries (0 when it names
/// none), and `type` is the service's own classification of a caller-owned profile
/// (`PRIVATE`) whatever the request asks for.
fn expected_after(before: &Value, patch: &Value) -> Value {
    let mut expected = before.clone();
    let members = patch
        .as_object()
        .unwrap_or_else(|| panic!("the patch is an object: {patch}"));
    for (member, value) in members {
        if member != "type" {
            expected[member.as_str()] = value.clone();
        }
    }
    for member in DEFAULTED_MEMBERS {
        expected[member] = members.get(member).cloned().unwrap_or(json!(0));
    }
    expected["type"] = json!("PRIVATE");
    expected
}

/// Assert that `profile` reports exactly `expected`, member by member, and that
/// `GetNetworkProfile` on the same ARN reports the identical resource.
async fn assert_reads_back(emu: &Emulator, case: &str, profile: &Value, expected: &Value) {
    for member in PROFILE_MEMBERS {
        assert_eq!(
            profile[member], expected[member],
            "case {case:?}: expected {member} {:?}, got {:?}",
            expected[member], profile[member]
        );
    }
    let arn = arn_of(profile);
    let (status, response) = emu
        .call("GetNetworkProfile", REGION, &json!({ "arn": arn }))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetNetworkProfile to answer 200, got {status} with {response}"
    );
    assert_eq!(
        response["networkProfile"], *profile,
        "case {case:?}: expected GetNetworkProfile to report {profile}, got {}",
        response["networkProfile"]
    );
}

/// Create a profile under `project_arn` named `name` and return the reported profile.
async fn create_profile(emu: &Emulator, project_arn: &str, name: &str) -> Value {
    let mut request = created_request(name);
    request["projectArn"] = json!(project_arn);
    let (status, response) = emu.call("CreateNetworkProfile", REGION, &request).await;
    assert_eq!(
        status, 200,
        "case creating {name:?}: expected 200, got {status} with {response}"
    );
    response["networkProfile"].clone()
}

/// UpdateNetworkProfile applies what the request supplies, keeps what it does not, and
/// GetNetworkProfile then reports the UPDATED profile under the same ARN.
#[tokio::test]
async fn updated_network_profiles_are_persisted_and_read_back_identically() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "update-owner" })).await);

    // (case, the members the update supplies besides `arn`)
    let cases: Vec<(String, Value)> = vec![
        (
            "the name only: the rename the confirm-after-write pattern reads back"
                .to_string(),
            json!({ "name": "renamed" }),
        ),
        (
            "an empty name (modelled Name length minimum of 0)".to_string(),
            json!({ "name": "" }),
        ),
        (
            "a name at the modelled Name length maximum of 256".to_string(),
            json!({ "name": "n".repeat(256) }),
        ),
        (
            "the description only".to_string(),
            json!({ "description": "re-tuned for a lossy link" }),
        ),
        (
            "a description at the modelled Message length maximum of 16384".to_string(),
            json!({ "description": "d".repeat(16384) }),
        ),
        (
            "the uplink direction only: the downlink keeps its stored shaping".to_string(),
            json!({
                "uplinkBandwidthBits": 9_000_i64,
                "uplinkDelayMs": 100,
                "uplinkJitterMs": 7,
                "uplinkLossPercent": 9,
                "downlinkLossPercent": 6
            }),
        ),
        (
            "the downlink direction only: the uplink keeps its stored shaping".to_string(),
            json!({
                "downlinkBandwidthBits": 8_000_i64,
                "downlinkDelayMs": 200,
                "downlinkJitterMs": 8,
                "uplinkLossPercent": 5,
                "downlinkLossPercent": 10
            }),
        ),
        (
            "every shaping member at its documented maximum".to_string(),
            json!({
                "name": "fully-throttled",
                "uplinkBandwidthBits": 104_857_600_i64,
                "downlinkBandwidthBits": 104_857_600_i64,
                "uplinkDelayMs": 2000,
                "downlinkDelayMs": 2000,
                "uplinkJitterMs": 2000,
                "downlinkJitterMs": 2000,
                "uplinkLossPercent": 100,
                "downlinkLossPercent": 100
            }),
        ),
        (
            "every shaping member at its documented minimum of zero".to_string(),
            json!({
                "name": "wide-open",
                "description": "",
                "uplinkBandwidthBits": 0,
                "downlinkBandwidthBits": 0,
                "uplinkDelayMs": 0,
                "downlinkDelayMs": 0,
                "uplinkJitterMs": 0,
                "downlinkJitterMs": 0,
                "uplinkLossPercent": 0,
                "downlinkLossPercent": 0
            }),
        ),
        (
            "nothing but the required arn: the stored shaping survives and the two \
             defaulted members resolve to 0"
                .to_string(),
            json!({}),
        ),
        (
            "type CURATED asked for: a caller-owned profile stays PRIVATE".to_string(),
            json!({ "type": "CURATED" }),
        ),
        (
            "type PRIVATE asked for, beside a rename".to_string(),
            json!({ "name": "private-and-renamed", "type": "PRIVATE" }),
        ),
    ];

    for (index, (case, patch)) in cases.iter().enumerate() {
        let before = create_profile(&emu, &project_arn, &format!("created-{index}")).await;
        let arn = arn_of(&before);
        let expected = expected_after(&before, patch);

        let mut request = patch.clone();
        request["arn"] = json!(arn);
        let (status, response) = emu.call("UpdateNetworkProfile", REGION, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateNetworkProfile to answer 200, got {status} with \
             {response}"
        );
        let updated = response["networkProfile"].clone();
        assert_eq!(
            arn_of(&updated),
            arn,
            "case {case:?}: expected the update to keep the ARN {arn:?}, got {:?}",
            updated["arn"]
        );
        assert_reads_back(&emu, case, &updated, &expected).await;
    }
}

/// Updates accumulate on the one record, and an update touches no other profile: two
/// updates in sequence leave the second one's values in place, and the bystander profile
/// in the same project still reads back as it was created.
#[tokio::test]
async fn updates_accumulate_on_the_addressed_profile_only() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "sequence-owner" })).await);
    let target = create_profile(&emu, &project_arn, "target").await;
    let bystander = create_profile(&emu, &project_arn, "bystander").await;
    let target_arn = arn_of(&target);

    let patches = [
        json!({ "name": "first-rename", "uplinkDelayMs": 50 }),
        json!({ "description": "second update", "downlinkDelayMs": 60, "uplinkLossPercent": 1 }),
        json!({ "name": "third-rename" }),
    ];

    let mut expected = target.clone();
    for (step, patch) in patches.iter().enumerate() {
        let case = format!("update {} of {}", step + 1, patches.len());
        expected = expected_after(&expected, patch);

        let mut request = patch.clone();
        request["arn"] = json!(target_arn);
        let (status, response) = emu.call("UpdateNetworkProfile", REGION, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {response}"
        );
        assert_reads_back(&emu, &case, &response["networkProfile"], &expected).await;
    }

    let (status, response) = emu
        .call(
            "GetNetworkProfile",
            REGION,
            &json!({ "arn": arn_of(&bystander) }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the bystander after three updates: expected 200, got {status} with {response}"
    );
    assert_eq!(
        response["networkProfile"], bystander,
        "case the bystander after three updates: expected {bystander}, got {}",
        response["networkProfile"]
    );
}

/// DeleteNetworkProfile releases the record: the ARN then resolves to nothing for every
/// operation addressed by it, while the profiles beside it are untouched.
#[tokio::test]
async fn deleted_network_profiles_resolve_to_nothing_afterwards() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "delete-owner" })).await);
    let doomed = create_profile(&emu, &project_arn, "doomed").await;
    let survivor = create_profile(&emu, &project_arn, "survivor").await;
    let doomed_arn = arn_of(&doomed);

    let (status, response) = emu
        .call("DeleteNetworkProfile", REGION, &json!({ "arn": doomed_arn }))
        .await;
    assert_eq!(
        status, 200,
        "case deleting the profile: expected 200, got {status} with {response}"
    );

    // Device Farm words a network profile's absence with the resource-type token its ARN
    // carries, spelled out here as a literal rather than asked of the implementation.
    let expected_message = format!("No networkprofile found for arn {doomed_arn}");

    // Every operation addressed by the released ARN reports it as missing — including a
    // second delete, which may not report a success for a record that is no longer there.
    for (case, op, extra) in [
        ("GetNetworkProfile after the delete", "GetNetworkProfile", json!({})),
        (
            "UpdateNetworkProfile after the delete",
            "UpdateNetworkProfile",
            json!({ "name": "too-late" }),
        ),
        (
            "DeleteNetworkProfile a second time",
            "DeleteNetworkProfile",
            json!({}),
        ),
    ] {
        let mut request = extra.clone();
        request["arn"] = json!(doomed_arn);
        let (status, response) = emu.call(op, REGION, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {response}"
        );
        assert_eq!(
            response["message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the message {expected_message:?}, got {:?}",
            response["message"]
        );
    }

    // The profile beside it survived, unchanged.
    let (status, response) = emu
        .call(
            "GetNetworkProfile",
            REGION,
            &json!({ "arn": arn_of(&survivor) }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the surviving profile: expected 200, got {status} with {response}"
    );
    assert_eq!(
        response["networkProfile"], survivor,
        "case the surviving profile: expected {survivor}, got {}",
        response["networkProfile"]
    );
}

/// An update or a delete whose ARN names another Device Farm resource type is a bad
/// argument, not a missing profile — and neither call changes the profile that does exist.
#[tokio::test]
async fn mutations_naming_another_resource_type_are_argument_failures() {
    let emu = Emulator::new();
    let project_arn = arn_of(&emu.create(&json!({ "name": "argument-owner" })).await);
    let profile = create_profile(&emu, &project_arn, "untouched").await;

    for (case, op, arn) in [
        (
            "UpdateNetworkProfile given the owning project's ARN",
            "UpdateNetworkProfile",
            project_arn.clone(),
        ),
        (
            "DeleteNetworkProfile given the owning project's ARN",
            "DeleteNetworkProfile",
            project_arn.clone(),
        ),
        (
            "UpdateNetworkProfile given a device-pool ARN",
            "UpdateNetworkProfile",
            crate::projects::foreign_arn("devicepool"),
        ),
        (
            "DeleteNetworkProfile given an upload ARN",
            "DeleteNetworkProfile",
            crate::projects::foreign_arn("upload"),
        ),
    ] {
        let (status, response) = emu.call(op, REGION, &json!({ "arn": arn })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ArgumentException",
            "case {case:?}: expected ArgumentException, got {response}"
        );
    }

    let (status, response) = emu
        .call(
            "GetNetworkProfile",
            REGION,
            &json!({ "arn": arn_of(&profile) }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the profile after four refusals: expected 200, got {status} with {response}"
    );
    assert_eq!(
        response["networkProfile"], profile,
        "case the profile after four refusals: expected {profile}, got {}",
        response["networkProfile"]
    );
}
