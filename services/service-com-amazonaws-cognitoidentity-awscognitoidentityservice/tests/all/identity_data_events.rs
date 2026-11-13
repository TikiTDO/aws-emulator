// SPDX-License-Identifier: CC0-1.0

//! Which calls to this service reach the account's management-event log.
//!
//! RULE: CloudTrail delivers a management event for this service's ADMINISTRATION of
//! identity pools — the calls a client makes with AWS developer credentials — and none for
//! the identity DATA PLANE: the five calls that mint or exchange an identity in a pool's
//! CONTENTS (`GetId`, `GetOpenIdToken`, `GetCredentialsForIdentity`, `UnlinkIdentity` and
//! `GetOpenIdTokenForDeveloperIdentity`), which CloudTrail reports as DATA events instead.
//! A data-plane call is therefore absent from the log whatever it answered, while the
//! `sts:AssumeRoleWithWebIdentity` a served vend makes IS a management event — STS's own —
//! and is recorded as that. Where those data events DO go — the bucket of each trail whose
//! advanced event selectors admit them — is asserted end to end by the cross-service suite
//! `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/tests/all/trail_log_file_delivery_of_data_events.rs`.
//!
//! Each case asserts the rule over a TABLE spanning the class, its rows differing along
//! what the rule generalises over: which operation was called, whether it was served or
//! refused, and whether the identity it named was a guest or a federated one.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds, so the records these calls write are
//! the ones a CloudTrail mounted on that engine would report.

use serde_json::{json, Value};

use crate::identity_pool_flow::{Emulator, ABSENT_POOL_ID};

/// The public login provider the fixture pool is fronted by, so a federated identity can be
/// minted for it.
const LOGIN_PROVIDER: &str = "graph.facebook.com";

/// The developer provider name the fixture pool declares, so the developer-identity exchange
/// has one to present a login under.
const DEVELOPER_PROVIDER: &str = "data-events-provider";

/// Every `eventName` the log holds, oldest first.
fn recorded_names(emulator: &Emulator) -> Vec<String> {
    emulator
        .recorded_events()
        .iter()
        .filter_map(|event| {
            event
                .get("eventName")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}

#[tokio::test]
async fn an_identity_data_plane_call_reaches_the_management_event_log_never() {
    let emulator = Emulator::new();
    // The pool declares both the public provider a federated identity is minted for and the
    // developer provider the backend exchange presents a login under.
    let pool_id = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": "data-events",
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                "SupportedLoginProviders": {LOGIN_PROVIDER: "declared-app-id"},
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool answers an IdentityPoolId")
        .to_string();
    let guest = emulator.identity(&pool_id, None).await;
    let federated = emulator
        .identity(&pool_id, Some(json!({LOGIN_PROVIDER: "a.login.token"})))
        .await;

    // The five data-plane operations, each in a served and in a refused shape: neither is
    // recorded, so the rule does not depend on the outcome.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a served GetId",
            "GetId",
            json!({"IdentityPoolId": &pool_id}),
        ),
        (
            "a refused GetId",
            "GetId",
            json!({"IdentityPoolId": ABSENT_POOL_ID}),
        ),
        (
            "a served GetOpenIdToken for a guest identity",
            "GetOpenIdToken",
            json!({"IdentityId": &guest}),
        ),
        (
            "a refused GetOpenIdToken",
            "GetOpenIdToken",
            json!({"IdentityId": "us-east-1:00000000-0000-0000-0000-000000000000"}),
        ),
        (
            "a vend with no role configured",
            "GetCredentialsForIdentity",
            json!({"IdentityId": &guest}),
        ),
        (
            "an unlink of a federated login",
            "UnlinkIdentity",
            json!({
                "IdentityId": &federated,
                "Logins": {LOGIN_PROVIDER: "a.login.token"},
                "LoginsToRemove": [LOGIN_PROVIDER],
            }),
        ),
        (
            "a refused unlink",
            "UnlinkIdentity",
            json!({
                "IdentityId": &guest,
                "Logins": {LOGIN_PROVIDER: "a.login.token"},
                "LoginsToRemove": [LOGIN_PROVIDER],
            }),
        ),
        (
            "a served developer-identity exchange",
            "GetOpenIdTokenForDeveloperIdentity",
            json!({
                "IdentityPoolId": &pool_id,
                "Logins": {DEVELOPER_PROVIDER: "developer-user-0"},
            }),
        ),
        (
            "a refused developer-identity exchange",
            "GetOpenIdTokenForDeveloperIdentity",
            json!({
                "IdentityPoolId": ABSENT_POOL_ID,
                "Logins": {DEVELOPER_PROVIDER: "developer-user-0"},
            }),
        ),
    ];

    for (case, operation, request) in rows {
        let before = recorded_names(&emulator);
        let (status, body) = emulator.call(operation, &request).await;
        let after = recorded_names(&emulator);
        assert_eq!(
            after, before,
            "case {case:?}: a data event reaches no record, but {operation} (answered {status} \
             with {body}) left {after:?} where there was {before:?}"
        );
    }
}

#[tokio::test]
async fn pool_administration_is_recorded_beside_it() {
    let emulator = Emulator::new();
    // The administrative calls, each of which IS a management event: the pool's own create,
    // a read of it, a write of its configuration and its delete.
    let pool_id = emulator
        .pool_with_providers("data-events-administration", true, &[LOGIN_PROVIDER])
        .await;
    emulator
        .ok("DescribeIdentityPool", &json!({"IdentityPoolId": &pool_id}))
        .await;
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({"IdentityPoolId": &pool_id, "Roles": {}}),
        )
        .await;
    // ...and one data-plane call in the middle of them, which is the one that is absent.
    emulator.identity(&pool_id, None).await;
    emulator
        .ok("DeleteIdentityPool", &json!({"IdentityPoolId": &pool_id}))
        .await;

    let names = recorded_names(&emulator);
    for recorded in [
        "CreateIdentityPool",
        "DescribeIdentityPool",
        "SetIdentityPoolRoles",
        "DeleteIdentityPool",
    ] {
        assert!(
            names.iter().any(|name| name == recorded),
            "case {recorded:?}: an administrative call is recorded, got {names:?}"
        );
    }
    for absent in [
        "GetId",
        "GetOpenIdToken",
        "GetCredentialsForIdentity",
        "UnlinkIdentity",
        "GetOpenIdTokenForDeveloperIdentity",
    ] {
        assert!(
            !names.iter().any(|name| name == absent),
            "case {absent:?}: a data-plane call is recorded nowhere, got {names:?}"
        );
    }
}
