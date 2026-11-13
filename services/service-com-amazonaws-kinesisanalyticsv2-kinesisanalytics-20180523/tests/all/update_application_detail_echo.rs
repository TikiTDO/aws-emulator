// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `ApplicationDetail` a WRITE answers with: the detail an
//! accepted `UpdateApplication` echoes is the stored application's, complete — so it
//! carries `ApplicationMode` and the `ConditionalToken` in force — and a fresh token is
//! minted for EVERY accepted update, so consecutive updates of one application never
//! hand out the same token twice.
//!
//! Every test drives the service the way a client does — one `POST /` per call through
//! the real `MultiServiceRouter`, with `X-Amz-Target` naming the operation — so what is
//! asserted is the wire behaviour, not a handler's return value.

use serde_json::{json, Value};

use super::application_mode_and_maintenance_window::{is_modeled_token, is_uuid_layout, token_of};
use super::harness::{call, create_body, describe, router};

/// The applications an update is driven against: both `ApplicationMode` values
/// (`smithy_query(query="shape",shape="ApplicationMode")`: STREAMING | INTERACTIVE), each
/// reached both ways — named by the create and implied by the runtime it is created on —
/// and one whose named mode is NOT the one its runtime implies.
///
/// Columns: (case, `RuntimeEnvironment`, the mode the create names, the mode the
/// application runs in, the runtime a later update moves it to).
const CASES: [(&str, &str, Option<&str>, &str, &str); 5] = [
    (
        "streaming-implied",
        "FLINK-1_20",
        None,
        "STREAMING",
        "FLINK-1_19",
    ),
    (
        "streaming-named",
        "FLINK-1_15",
        Some("STREAMING"),
        "STREAMING",
        "FLINK-1_18",
    ),
    (
        "notebook-implied",
        "ZEPPELIN-FLINK-3_0",
        None,
        "INTERACTIVE",
        "ZEPPELIN-FLINK-2_0",
    ),
    (
        "notebook-named",
        "ZEPPELIN-FLINK-2_0",
        Some("INTERACTIVE"),
        "INTERACTIVE",
        "ZEPPELIN-FLINK-3_0",
    ),
    (
        "streaming-on-a-notebook-runtime",
        "ZEPPELIN-FLINK-3_0",
        Some("STREAMING"),
        "STREAMING",
        "FLINK-2_2",
    ),
];

/// The updates each application is driven through, in order: a sub-configuration
/// replacement, a role replacement, a runtime replacement (`{runtime}` stands for the
/// row's target runtime) and a property-group replacement — four different kinds of
/// accepted update, so what is asserted holds for an accepted update and not for one
/// shape of request.
fn update_bodies(runtime_update: &str) -> [Value; 4] {
    [
        json!({
            "ApplicationConfigurationUpdate": {
                "ApplicationSnapshotConfigurationUpdate": {"SnapshotsEnabledUpdate": true},
            },
        }),
        json!({"ServiceExecutionRoleUpdate": "arn:aws:iam::000000000000:role/kda-other-role"}),
        json!({"RuntimeEnvironmentUpdate": runtime_update}),
        json!({
            "ApplicationConfigurationUpdate": {
                "EnvironmentPropertyUpdates": {
                    "PropertyGroups": [{
                        "PropertyGroupId": "producer.config",
                        "PropertyMap": {"flink.stream.initpos": "LATEST"},
                    }],
                },
            },
        }),
    ]
}

/// An accepted `UpdateApplication` answers with the stored application's whole detail: it
/// reports the mode the application runs in, and a `ConditionalToken` of the modelled
/// opaque form that is fresh for every accepted update — never the one the previous
/// version was named by.
#[tokio::test]
async fn an_accepted_update_echoes_the_stored_detail_with_a_fresh_token() {
    // Every token this service hands out across every application of every row, so no
    // token can be re-issued for a different version or a different application.
    let mut seen: Vec<String> = Vec::new();

    for (case, runtime, supplied_mode, mode, runtime_update) in CASES {
        let router = router();
        let name = "update-echo";
        let mut body = create_body(name, runtime);
        if let Some(supplied) = supplied_mode {
            body["ApplicationMode"] = json!(supplied);
        }
        let (status, created) = call(&router, "CreateApplication", &body).await;
        assert_eq!(status, 200, "case {case:?}: CreateApplication got {created}");
        let created = &created["ApplicationDetail"];
        assert_eq!(
            created["ApplicationMode"].as_str(),
            Some(mode),
            "case {case:?}: expected the mode the application runs in, got {created}"
        );
        let mut token = token_of(created, case);
        assert_opaque_token(&token, case, &mut seen);

        for (step, update) in update_bodies(runtime_update).into_iter().enumerate() {
            let version = step as i64 + 1;
            let mut request = update;
            // The two members an update addresses a version by — "You must provide the
            // CurrentApplicationVersionId or the ConditionalToken" — used in turn, so the
            // answer is asserted for both ways of naming the version in force.
            request["ApplicationName"] = json!(name);
            if step % 2 == 0 {
                request["CurrentApplicationVersionId"] = json!(version);
            } else {
                request["ConditionalToken"] = json!(token.clone());
            }
            let case = format!("{case}/update#{}", step + 1);

            let (status, answered) = call(&router, "UpdateApplication", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateApplication got {answered}"
            );
            let detail = &answered["ApplicationDetail"];

            // The mode is a property of the application, not of the request: every
            // accepted update reports the one the application runs in.
            assert_eq!(
                detail["ApplicationMode"].as_str(),
                Some(mode),
                "case {case:?}: expected the mode the application runs in, got {detail}"
            );
            // The version moved, which is what the token names.
            assert_eq!(
                detail["ApplicationVersionId"].as_i64(),
                Some(version + 1),
                "case {case:?}: expected the update to mint the next version, got {detail}"
            );

            let rotated = token_of(detail, &case);
            assert_ne!(
                rotated, token,
                "case {case:?}: expected a token minted for the new version, got {detail}"
            );
            assert_opaque_token(&rotated, &case, &mut seen);

            // The detail a write echoes IS the stored one: a client that reads the update's
            // answer instead of making a second call sees exactly what the reader reports.
            let described = describe(&router, name).await;
            assert_eq!(
                &described, detail,
                "case {case:?}: expected the echoed detail to be the stored one, \
                 describe reported {described}"
            );
            token = rotated;
        }
    }
}

/// An update refused for naming a version other than the one in force mints no token: the
/// token names the version in force, and that version did not move.
#[tokio::test]
async fn a_refused_update_leaves_the_token_in_force() {
    for (case, runtime, supplied_mode, _, _) in CASES {
        let router = router();
        let name = "update-refused";
        let mut body = create_body(name, runtime);
        if let Some(supplied) = supplied_mode {
            body["ApplicationMode"] = json!(supplied);
        }
        let (status, created) = call(&router, "CreateApplication", &body).await;
        assert_eq!(status, 200, "case {case:?}: CreateApplication got {created}");
        let token = token_of(&created["ApplicationDetail"], case);

        // Each row of refusals breaks one requirement of an accepted update: a consumed
        // version id, a token that names no version, and neither member at all.
        for refusal in [
            json!({"ApplicationName": name, "CurrentApplicationVersionId": 7}),
            json!({"ApplicationName": name, "ConditionalToken": "not-the-stored-token"}),
            json!({"ApplicationName": name}),
        ] {
            let (status, answered) = call(&router, "UpdateApplication", &refusal).await;
            assert!(
                (400..500).contains(&status),
                "case {case:?}: expected {refusal} to be refused, got {status} {answered}"
            );
            let described = describe(&router, name).await;
            assert_eq!(
                token_of(&described, case),
                token,
                "case {case:?}: expected the token in force to be untouched after \
                 {refusal}, got {described}"
            );
            assert_eq!(
                described["ApplicationVersionId"].as_i64(),
                Some(1),
                "case {case:?}: expected the version in force to be untouched after \
                 {refusal}, got {described}"
            );
        }
    }
}

/// Assert `token` is a token of the modelled opaque form nothing has handed out before,
/// and record it as handed out.
fn assert_opaque_token(token: &str, case: &str, seen: &mut Vec<String>) {
    assert!(
        is_modeled_token(token),
        "case {case:?}: expected a token of the modelled form, got {token:?}"
    );
    assert!(
        !is_uuid_layout(token),
        "case {case:?}: expected an opaque token rather than UUID text, got {token:?}"
    );
    assert!(
        !seen.contains(&token.to_string()),
        "case {case:?}: token {token:?} was already handed out"
    );
    seen.push(token.to_string());
}

