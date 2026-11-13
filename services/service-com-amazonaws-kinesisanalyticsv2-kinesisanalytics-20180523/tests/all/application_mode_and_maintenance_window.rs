// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the members every reader of an application reports about it:
//! `ApplicationMode`, `ApplicationMaintenanceConfigurationDescription`,
//! `LastUpdateTimestamp` and `ConditionalToken` — on `CreateApplication`,
//! `DescribeApplication`, `ListApplications`, `UpdateApplication` and
//! `UpdateApplicationMaintenanceConfiguration`.
//!
//! Every test drives the service the way a client does — one `POST /` per call through
//! the real `MultiServiceRouter`, with `X-Amz-Target` naming the operation — so what is
//! asserted is the wire behaviour, not a handler's return value.

use serde_json::{json, Value};

use super::harness::{
    call, create, create_body, create_startable, describe, error_code, error_message, router,
    SETTLE_WAIT,
};

/// Every `RuntimeEnvironment` an application can be created on, as the model spells them
/// (`smithy_query(query="shape",shape="RuntimeEnvironment")`), paired with the mode an
/// application on it runs in: `INTERACTIVE` for the Studio-notebook (Zeppelin) family and
/// `STREAMING` for every streaming one. The retired SQL family is refused at create
/// (`modeled_client_errors.rs` asserts that), so no application exists on it.
const RUNTIME_MODES: [(&str, &str); 13] = [
    ("FLINK-1_6", "STREAMING"),
    ("FLINK-1_8", "STREAMING"),
    ("ZEPPELIN-FLINK-1_0", "INTERACTIVE"),
    ("FLINK-1_11", "STREAMING"),
    ("FLINK-1_13", "STREAMING"),
    ("ZEPPELIN-FLINK-2_0", "INTERACTIVE"),
    ("FLINK-1_15", "STREAMING"),
    ("ZEPPELIN-FLINK-3_0", "INTERACTIVE"),
    ("FLINK-1_18", "STREAMING"),
    ("FLINK-1_19", "STREAMING"),
    ("FLINK-1_20", "STREAMING"),
    ("FLINK-2_2", "STREAMING"),
    ("FLINK-2_3", "STREAMING"),
];

/// An application reports a mode whether or not its create named one: the mode the
/// creator supplied, else the one its runtime implies — and the same value on every
/// reader of it.
#[tokio::test]
async fn every_application_reports_the_mode_its_runtime_implies() {
    for (runtime, implied) in RUNTIME_MODES {
        // The row's two inputs: a create that names the mode, and one that leaves it out.
        for supplied in [None, Some(implied)] {
            let router = router();
            let name = "mode-probe";
            let case = format!("{runtime} mode={supplied:?}");
            let mut body = create_body(name, runtime);
            if let Some(mode) = supplied {
                body["ApplicationMode"] = json!(mode);
            }

            let (status, response) = call(&router, "CreateApplication", &body).await;
            assert_eq!(status, 200, "case {case:?}: CreateApplication got {response}");
            let created = &response["ApplicationDetail"];
            assert_eq!(
                created["ApplicationMode"].as_str(),
                Some(implied),
                "case {case:?}: expected the mode this runtime runs in, got {created}"
            );

            let described = describe(&router, name).await;
            assert_eq!(
                described["ApplicationMode"].as_str(),
                Some(implied),
                "case {case:?}: expected describe to report the stored mode, got {described}"
            );

            let (status, listed) = call(&router, "ListApplications", &json!({})).await;
            assert_eq!(status, 200, "case {case:?}: ListApplications got {listed}");
            let summary = &listed["ApplicationSummaries"][0];
            assert_eq!(
                summary["ApplicationMode"].as_str(),
                Some(implied),
                "case {case:?}: expected the summary to carry the stored mode, got {summary}"
            );
        }
    }
}

/// A mode a create names explicitly is the one stored, including on a runtime whose
/// family implies the other one: what the creator asked for is what the application runs
/// in.
#[tokio::test]
async fn a_create_that_names_a_mode_stores_that_mode() {
    // (runtime, the mode named, which is NOT the one the runtime implies)
    let cases = [
        ("FLINK-1_20", "INTERACTIVE"),
        ("ZEPPELIN-FLINK-3_0", "STREAMING"),
    ];
    for (runtime, mode) in cases {
        let router = router();
        let name = "explicit-mode";
        let mut body = create_body(name, runtime);
        body["ApplicationMode"] = json!(mode);
        let (status, response) = call(&router, "CreateApplication", &body).await;
        assert_eq!(
            status, 200,
            "case {runtime:?}/{mode:?}: CreateApplication got {response}"
        );
        let described = describe(&router, name).await;
        assert_eq!(
            described["ApplicationMode"].as_str(),
            Some(mode),
            "case {runtime:?}/{mode:?}: expected the mode the create named, got {described}"
        );
    }
}

/// `UpdateApplication` leaves the mode alone: the API models no update of it, so a
/// changed runtime does not re-decide what the application runs in.
#[tokio::test]
async fn an_update_does_not_change_the_stored_mode() {
    let router = router();
    let name = "mode-after-update";
    let mut body = create_body(name, "ZEPPELIN-FLINK-3_0");
    body["ApplicationMode"] = json!("INTERACTIVE");
    let (status, response) = call(&router, "CreateApplication", &body).await;
    assert_eq!(status, 200, "CreateApplication got {response}");

    let (status, updated) = call(
        &router,
        "UpdateApplication",
        &json!({
            "ApplicationName": name,
            "CurrentApplicationVersionId": 1,
            "RuntimeEnvironmentUpdate": "FLINK-1_20",
        }),
    )
    .await;
    assert_eq!(status, 200, "UpdateApplication got {updated}");
    let detail = &updated["ApplicationDetail"];
    assert_eq!(
        detail["ApplicationMode"].as_str(),
        Some("INTERACTIVE"),
        "case runtime-changed: expected the stored mode to be untouched, got {detail}"
    );
}

// ---------------------------------------------------------------------------
// The maintenance window
// ---------------------------------------------------------------------------

/// The window start times a maintenance configuration update is driven with, spanning
/// what `ApplicationMaintenanceWindowStartTime` admits
/// (`smithy_query(query="shape",shape="ApplicationMaintenanceWindowStartTime")`: len 5,
/// `^([01][0-9]|2[0-3]):[0-5][0-9]$`) — both published bounds of the hour and of the
/// minute, a start whose window runs past midnight, and one that does not.
const WINDOW_STARTS: [&str; 6] = ["00:00", "02:30", "15:59", "16:00", "19:45", "23:59"];

/// The end of a maintenance window that starts at `start`: eight hours later on the same
/// day's clock, with the minutes the start carries.
fn expected_end(start: &str) -> String {
    let (hours, minutes) = start.split_once(':').expect("a HH:MM start time");
    let end = (hours.parse::<u32>().expect("an hour") + 8) % 24;
    format!("{end:02}:{minutes}")
}

/// Every application carries a maintenance window from the moment it is created, and
/// `UpdateApplicationMaintenanceConfiguration` moves its start — reported identically by
/// the update itself and by a later describe.
#[tokio::test]
async fn the_maintenance_window_is_reported_from_create_and_moves_with_the_update() {
    for start in WINDOW_STARTS {
        let router = router();
        let name = "window-probe";
        let created = create(&router, name, "FLINK-1_20").await;

        // The window an application has before anybody set one: present, and eight hours
        // long like every other.
        let window = &created["ApplicationMaintenanceConfigurationDescription"];
        let default_start = window["ApplicationMaintenanceWindowStartTime"]
            .as_str()
            .unwrap_or_else(|| panic!("case {start:?}: expected a window at create, got {created}"))
            .to_string();
        assert_eq!(
            window["ApplicationMaintenanceWindowEndTime"].as_str(),
            Some(expected_end(&default_start).as_str()),
            "case {start:?}: expected the window to end eight hours after it starts, got {window}"
        );

        let (status, answered) = call(
            &router,
            "UpdateApplicationMaintenanceConfiguration",
            &json!({
                "ApplicationName": name,
                "ApplicationMaintenanceConfigurationUpdate": {
                    "ApplicationMaintenanceWindowStartTimeUpdate": start,
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {start:?}: UpdateApplicationMaintenanceConfiguration got {answered}"
        );
        assert_eq!(
            answered["ApplicationARN"].as_str(),
            created["ApplicationARN"].as_str(),
            "case {start:?}: expected the application's own ARN, got {answered}"
        );
        let expected = json!({
            "ApplicationMaintenanceWindowStartTime": start,
            "ApplicationMaintenanceWindowEndTime": expected_end(start),
        });
        assert_eq!(
            answered["ApplicationMaintenanceConfigurationDescription"], expected,
            "case {start:?}: expected the window the update set, got {answered}"
        );

        // The round trip: the reader reports the window the update persisted.
        let described = describe(&router, name).await;
        assert_eq!(
            described["ApplicationMaintenanceConfigurationDescription"], expected,
            "case {start:?}: expected describe to report the window just set, got {described}"
        );
        // A window is scheduling, not configuration: the application stays on its version.
        assert_eq!(
            described["ApplicationVersionId"].as_i64(),
            Some(1),
            "case {start:?}: a maintenance update does not mint a version, got {described}"
        );
    }
}

/// The last window set is the one reported: repeated updates replace each other rather
/// than accumulating.
#[tokio::test]
async fn the_last_maintenance_window_set_is_the_one_reported() {
    let router = router();
    let name = "window-sequence";
    create(&router, name, "FLINK-1_19").await;
    for start in WINDOW_STARTS {
        let (status, answered) = call(
            &router,
            "UpdateApplicationMaintenanceConfiguration",
            &json!({
                "ApplicationName": name,
                "ApplicationMaintenanceConfigurationUpdate": {
                    "ApplicationMaintenanceWindowStartTimeUpdate": start,
                },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {start:?}: UpdateApplicationMaintenanceConfiguration got {answered}"
        );
        let described = describe(&router, name).await;
        assert_eq!(
            described["ApplicationMaintenanceConfigurationDescription"]
                ["ApplicationMaintenanceWindowStartTime"]
                .as_str(),
            Some(start),
            "case {start:?}: expected the window last set, got {described}"
        );
    }
}

/// A maintenance update of an application nobody created is a lookup miss, worded the way
/// every reader of an absent application words it.
#[tokio::test]
async fn a_maintenance_update_of_an_absent_application_is_refused() {
    let router = router();
    let (status, body) = call(
        &router,
        "UpdateApplicationMaintenanceConfiguration",
        &json!({
            "ApplicationName": "never-created",
            "ApplicationMaintenanceConfigurationUpdate": {
                "ApplicationMaintenanceWindowStartTimeUpdate": "06:00",
            },
        }),
    )
    .await;
    assert_eq!(status, 400, "case absent: expected a refusal, got {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case absent: expected the lookup-miss error, got {body}"
    );
    assert_eq!(
        error_message(&body),
        "Application 'never-created' does not exist.",
        "case absent: expected the lookup-miss sentence, got {body}"
    );
}

/// An application in a transient status is not available for a maintenance update: the
/// operation is invoked on one that is READY or RUNNING.
#[tokio::test]
async fn a_maintenance_update_of_an_application_in_use_is_refused() {
    let router = router();
    let name = "window-while-starting";
    create_startable(&router, name, "FLINK-1_20").await;
    let (status, started) = call(&router, "StartApplication", &json!({"ApplicationName": name})).await;
    assert_eq!(status, 200, "case starting: StartApplication got {started}");

    let update = json!({
        "ApplicationName": name,
        "ApplicationMaintenanceConfigurationUpdate": {
            "ApplicationMaintenanceWindowStartTimeUpdate": "08:00",
        },
    });
    let (status, body) = call(
        &router,
        "UpdateApplicationMaintenanceConfiguration",
        &update,
    )
    .await;
    assert_eq!(status, 400, "case starting: expected a refusal, got {body}");
    assert_eq!(
        error_code(&body),
        "ResourceInUseException",
        "case starting: expected the in-use error, got {body}"
    );
    assert_eq!(
        error_message(&body),
        format!("Application {name} cannot be updated while it is in the STARTING state"),
        "case starting: expected the in-use sentence, got {body}"
    );

    // Once the start has settled the application is RUNNING, which the operation accepts.
    tokio::time::sleep(SETTLE_WAIT).await;
    let (status, answered) = call(
        &router,
        "UpdateApplicationMaintenanceConfiguration",
        &update,
    )
    .await;
    assert_eq!(status, 200, "case running: expected the update to be accepted, got {answered}");
    assert_eq!(
        answered["ApplicationMaintenanceConfigurationDescription"]
            ["ApplicationMaintenanceWindowStartTime"]
            .as_str(),
        Some("08:00"),
        "case running: expected the window the update set, got {answered}"
    );
}

/// A start time outside what the model admits never reaches the store: the boundary
/// refuses it, and the stored window is untouched.
#[tokio::test]
async fn a_start_time_the_model_refuses_leaves_the_window_alone() {
    let router = router();
    let name = "window-validation";
    let created = create(&router, name, "FLINK-1_20").await;
    let stored = created["ApplicationMaintenanceConfigurationDescription"].clone();

    // Each case breaks one requirement of ApplicationMaintenanceWindowStartTime: the
    // hour's range, the minute's range, the length, and the form itself.
    for start in ["24:00", "06:60", "6:00", "060000", "not-a-time"] {
        let (status, body) = call(
            &router,
            "UpdateApplicationMaintenanceConfiguration",
            &json!({
                "ApplicationName": name,
                "ApplicationMaintenanceConfigurationUpdate": {
                    "ApplicationMaintenanceWindowStartTimeUpdate": start,
                },
            }),
        )
        .await;
        assert_eq!(status, 400, "case {start:?}: expected a refusal, got {body}");
        let described = describe(&router, name).await;
        assert_eq!(
            described["ApplicationMaintenanceConfigurationDescription"], stored,
            "case {start:?}: expected the stored window to be untouched, got {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// LastUpdateTimestamp and ConditionalToken
// ---------------------------------------------------------------------------

/// `LastUpdateTimestamp` is reported from the create onward — the create is itself the
/// application's latest change — and moves on with every change after it.
#[tokio::test]
async fn last_update_timestamp_is_reported_from_create_and_moves_with_a_change() {
    let router = router();
    for (name, runtime) in [("stamp-flink", "FLINK-1_20"), ("stamp-older", "FLINK-1_11")] {
        let created = create(&router, name, runtime).await;
        let at_create = created["LastUpdateTimestamp"].as_i64().unwrap_or_else(|| {
            panic!("case {name:?}: expected a LastUpdateTimestamp at create, got {created}")
        });
        assert_eq!(
            Some(at_create),
            created["CreateTimestamp"].as_i64(),
            "case {name:?}: a never-updated application was last changed when it was \
             created, got {created}"
        );

        let described = describe(&router, name).await;
        assert_eq!(
            described["LastUpdateTimestamp"].as_i64(),
            Some(at_create),
            "case {name:?}: expected describe to report the stored stamp, got {described}"
        );

        let (status, updated) = call(
            &router,
            "UpdateApplication",
            &json!({
                "ApplicationName": name,
                "CurrentApplicationVersionId": 1,
                "ApplicationConfigurationUpdate": {
                    "ApplicationSnapshotConfigurationUpdate": {"SnapshotsEnabledUpdate": false},
                },
            }),
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: UpdateApplication got {updated}");
        let after = updated["ApplicationDetail"]["LastUpdateTimestamp"].as_i64();
        assert!(
            after.is_some_and(|after| after >= at_create),
            "case {name:?}: expected the update to stamp LastUpdateTimestamp, got {updated}"
        );
    }
}

/// Whether `token` satisfies `ConditionalToken` (len 1..512, pattern
/// `^[a-zA-Z0-9-_+/=]+$`, `smithy_query(query="shape",shape="ConditionalToken")`), which
/// is every requirement the model places on a token's form.
pub(crate) fn is_modeled_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 512
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '/' | '='))
}

/// Whether `token` is written in the 8-4-4-4-12 layout of a UUID — the structure a token
/// must NOT wear.
///
/// A `ConditionalToken` is opaque: "a value you use to implement strong concurrency for
/// application updates", handed back as received. A value punctuated as a UUID invites a
/// client to parse it as one, which it is not; this service's server-assigned IDS are
/// written that way, and a token is not an id.
pub(crate) fn is_uuid_layout(token: &str) -> bool {
    let groups: Vec<&str> = token.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// A `ConditionalToken` names the version in force: it satisfies the modelled form, is
/// distinct per application, is stable across reads, and a fresh one is minted whenever
/// the version it names moves.
#[tokio::test]
async fn a_conditional_token_is_stable_per_version_and_rotates_with_it() {
    let router = router();
    let mut seen: Vec<String> = Vec::new();

    for (name, runtime) in [
        ("token-a", "FLINK-1_20"),
        ("token-b", "FLINK-1_11"),
        ("token-c", "ZEPPELIN-FLINK-3_0"),
    ] {
        let created = create(&router, name, runtime).await;
        let token = token_of(&created, name);
        assert!(
            is_modeled_token(&token),
            "case {name:?}: expected a token of the modelled form, got {token:?}"
        );
        assert!(
            !is_uuid_layout(&token),
            "case {name:?}: expected an opaque token rather than UUID text, got {token:?}"
        );
        assert!(
            !seen.contains(&token),
            "case {name:?}: token {token:?} was already handed out"
        );
        seen.push(token.clone());

        // A describe reports the token in force; it does not mint another.
        let described = describe(&router, name).await;
        assert_eq!(
            token_of(&described, name),
            token,
            "case {name:?}: expected describe to report the stored token, got {described}"
        );

        // The token names the CURRENT version, so an update that moves the version
        // rotates it.
        let (status, updated) = call(
            &router,
            "UpdateApplication",
            &json!({"ApplicationName": name, "ConditionalToken": token}),
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: UpdateApplication got {updated}");
        let rotated = token_of(&updated["ApplicationDetail"], name);
        assert_ne!(
            rotated, token,
            "case {name:?}: expected a fresh token for the new version, got {updated}"
        );
        assert!(
            is_modeled_token(&rotated),
            "case {name:?}: expected another token of the modelled form, got {rotated:?}"
        );
        assert!(
            !is_uuid_layout(&rotated),
            "case {name:?}: expected another opaque token rather than UUID text, got \
             {rotated:?}"
        );
        assert!(
            !seen.contains(&rotated),
            "case {name:?}: token {rotated:?} was already handed out"
        );
        seen.push(rotated);
    }
}

/// The `ConditionalToken` of an `ApplicationDetail`.
pub(crate) fn token_of(detail: &Value, case: &str) -> String {
    detail["ConditionalToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConditionalToken, got {detail}"))
        .to_string()
}
