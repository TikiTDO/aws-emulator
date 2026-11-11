// SPDX-License-Identifier: CC0-1.0

//! The TIMING of the rollout gate, read from the data plane: which version a session is
//! served as the CLOCK carries a rollout through its growth intervals.
//!
//! RULE: how far a deployment's rollout has got is owed by the clock, not by anybody
//! calling — a strategy that declares a rollout window reaches its targets one growth
//! interval at a time — and the configuration in force for a session is the one the
//! rollout has reached that session with (`PercentageComplete` is "the percentage of
//! targets for which the deployment is available"). So a session opened before the first
//! interval has elapsed is served the version that was in force BEFORE the rollout
//! started, and a session opened once the whole rollout window has passed is served the
//! new version — with nothing about the deployment written by hand.
//!
//! The companion suite `rollout_percentage_splits_sessions` asserts the gate itself over
//! a table of percentages; this one asserts that the clock is what moves a deployment
//! through them, which is why it waits rather than writing a percentage into the store.

use std::time::Duration;

use bytes::Bytes;

use super::configuration_session_poll::{Emulator, Hierarchy};

/// The two versions deployed, told apart by their bytes.
const PREVIOUS: &[u8] = br#"{"tier":"one"}"#;
/// See [`PREVIOUS`].
const NEW: &[u8] = br#"{"tier":"two"}"#;

/// RULE: the rollout gate moves with the clock — before the rollout's first growth
/// interval a session is served the previous version, and once the whole rollout window
/// has passed it is served the new one.
#[tokio::test]
async fn the_version_a_session_is_served_follows_the_clock_through_the_rollout() {
    let emulator = Emulator::new();
    let hierarchy = emulator
        .create_hierarchy("paced-app", "production", "flags")
        .await;

    // The previous version goes out with a strategy that has nothing to wait for, and is
    // left to settle, so it is in force for every session when the rollout starts.
    emulator
        .deploy(
            &hierarchy,
            &hierarchy.profile_id,
            PREVIOUS,
            "application/json",
            None,
        )
        .await;
    emulator.settle_deployments(&hierarchy);

    // A strategy that declares a rollout window and grows in TWO intervals, so the
    // rollout has reached nobody when the deployment starts and every target once its
    // window has passed.
    let version = host_version(&emulator, &hierarchy, NEW).await;
    let created = emulator
        .post_json(
            "/deploymentstrategies",
            r#"{"Name":"two-intervals","DeploymentDurationInMinutes":1,"GrowthFactor":50.0,
                "GrowthType":"LINEAR","FinalBakeTimeInMinutes":1440,"ReplicateTo":"NONE"}"#,
        )
        .await;
    assert_eq!(
        created.status,
        201,
        "CreateDeploymentStrategy: expected 201, got {} body {}",
        created.status,
        created.text()
    );
    let strategy = created.string("Id");
    let application_id = &hierarchy.application_id;
    let environment_id = &hierarchy.environment_id;
    let profile_id = &hierarchy.profile_id;
    let started = emulator
        .post_json(
            &format!("/applications/{application_id}/environments/{environment_id}/deployments"),
            &format!(
                r#"{{"DeploymentStrategyId":"{strategy}",
                      "ConfigurationProfileId":"{profile_id}",
                      "ConfigurationVersion":"{version}"}}"#
            ),
        )
        .await;
    assert_eq!(
        started.status,
        201,
        "StartDeployment: expected 201, got {} body {}",
        started.status,
        started.text()
    );

    // Before the first growth interval has elapsed the rollout has reached nobody.
    let served = poll_one_session(&emulator, &hierarchy).await;
    assert_eq!(
        served.as_slice(),
        PREVIOUS,
        "case a rollout whose first interval has not elapsed: expected the previous \
         version, got {:?}",
        String::from_utf8_lossy(&served)
    );

    // Once the whole rollout window has passed, every target has been reached. The wait
    // is the rollout's own span — two growth intervals of
    // `service_com_amazonaws_appconfig_amazonappconfig::state::STAGE_DURATION` — plus a
    // margin, and nothing is written into either store.
    let rollout = service_com_amazonaws_appconfig_amazonappconfig::state::STAGE_DURATION * 2;
    tokio::time::sleep(rollout + Duration::from_millis(500)).await;
    let served = poll_one_session(&emulator, &hierarchy).await;
    assert_eq!(
        served.as_slice(),
        NEW,
        "case a rollout whose window has passed: expected the new version, got {:?}",
        String::from_utf8_lossy(&served)
    );
}

/// Open one session over the triple, poll it once, and answer the bytes it was served.
async fn poll_one_session(emulator: &Emulator, hierarchy: &Hierarchy) -> Vec<u8> {
    let token = emulator
        .session_token(
            &hierarchy.application_id,
            &hierarchy.environment_id,
            &hierarchy.profile_id,
        )
        .await;
    let reply = emulator.get_latest_configuration(&token).await;
    assert_eq!(
        reply.status,
        200,
        "GetLatestConfiguration must answer 200, got {} body {}",
        reply.status,
        reply.text()
    );
    reply.body.to_vec()
}

/// Write one hosted configuration version, answering the version number AppConfig minted.
async fn host_version(emulator: &Emulator, hierarchy: &Hierarchy, content: &[u8]) -> String {
    let application_id = &hierarchy.application_id;
    let profile_id = &hierarchy.profile_id;
    let written = emulator
        .send(
            "POST",
            &format!(
                "/applications/{application_id}/configurationprofiles/{profile_id}\
                 /hostedconfigurationversions"
            ),
            "application/json",
            &[],
            Bytes::copy_from_slice(content),
        )
        .await;
    assert_eq!(
        written.status,
        201,
        "CreateHostedConfigurationVersion: expected 201, got {} body {}",
        written.status,
        written.text()
    );
    written
        .header("version-number")
        .unwrap_or_else(|| panic!("no Version-Number header; headers {:?}", written.headers))
        .to_string()
}
