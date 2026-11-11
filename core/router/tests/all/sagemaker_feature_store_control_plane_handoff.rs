// SPDX-License-Identifier: CC0-1.0

//! The SageMaker Feature Store hand-off: the CONTROL plane defines a feature group, the
//! FeatureStore Runtime DATA plane serves its records.
//!
//! RULE: a runtime operation serves a `FeatureGroupName` only when it resolves to a
//! feature group `CreateFeatureGroup` created; a name that resolves to none is refused
//! with the modelled `ValidationError` (HTTP 400), never accepted and never reported as a
//! missing record (`ResourceNotFound`, 404). A group created with NO `OnlineStore` is
//! refused the same way on a read, which has nothing to serve from it, while a write to it
//! leaves the online store alone and succeeds — such a write goes to the group's
//! OfflineStore, where the group has one.
//!
//! The runtime API carries no feature-group definition — a `PutRecord` names a group and
//! nothing else — so the two services are only able to agree because the control plane
//! records the definition in ONE shared directory
//! (`service_common::feature_store_groups`). This suite drives both halves through the
//! SAME router and the SAME engine the emulator mounts them on, which is the only place
//! that agreement can be observed: with the control plane absent, nothing creates a
//! feature group at all.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the signing name the whole
/// SageMaker family shares. The signature is never checked: the router here runs with
/// request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/sagemaker/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The control plane and the FeatureStore runtime, mounted on one engine as
/// `core/router/src/lib.rs` mounts them.
///
/// `pub(crate)` so the deletion counterpart suite
/// (`sagemaker_feature_store_group_deletion`) drives the same two services on the same
/// router rather than assembling a second copy of it.
pub(crate) fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(
        Arc::clone(engine),
    ));
    router.register_service(
        service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// One answer: the status and the parsed JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Answer {
    /// The wire error code: restJson1 and awsJson1_1 both carry it as the body's
    /// `__type`.
    pub(crate) fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_owned()
    }
}

/// Send one request through the whole router.
pub(crate) async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://emulator.local{path}"))
        .header("host", "emulator.local")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(
            body.map(|value| Bytes::from(value.to_string()))
                .unwrap_or_default(),
        )
        .expect("build the request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null),
    }
}

/// `CreateFeatureGroup` on the control plane, in its awsJson1_1 RPC form.
pub(crate) async fn create_feature_group(
    router: &MultiServiceRouter,
    name: &str,
    enable_online_store: bool,
) -> Answer {
    dispatch(
        router,
        "POST",
        "/",
        &[("x-amz-target", "SageMaker.CreateFeatureGroup")],
        Some(json!({
            "FeatureGroupName": name,
            "RecordIdentifierFeatureName": "record_id",
            "EventTimeFeatureName": "event_time",
            "FeatureDefinitions": [
                { "FeatureName": "record_id", "FeatureType": "String" },
                { "FeatureName": "event_time", "FeatureType": "Fractional" },
                { "FeatureName": "feature_a", "FeatureType": "String" }
            ],
            "OnlineStoreConfig": { "EnableOnlineStore": enable_online_store }
        })),
    )
    .await
}

/// One `FeatureValue` with a scalar value.
pub(crate) fn feature(name: &str, value: &str) -> Value {
    json!({ "FeatureName": name, "ValueAsString": value })
}

/// The record a `PutRecord`/`UpdateRecord` row writes.
pub(crate) fn record(identifier: &str, event_time: &str, feature_a: &str) -> Value {
    json!([
        feature("record_id", identifier),
        feature("event_time", event_time),
        feature("feature_a", feature_a)
    ])
}

#[tokio::test]
async fn the_data_plane_serves_the_feature_groups_the_control_plane_created_and_refuses_the_rest() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);

    let created = create_feature_group(&router, "wire-customers", true).await;
    let arn = created
        .body
        .get("FeatureGroupArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    assert_eq!(
        (created.status, arn.is_empty()),
        (200, false),
        "case \"CreateFeatureGroup\": expected HTTP 200 and a minted FeatureGroupArn, got {} \
         with body {}",
        created.status,
        created.body
    );

    // A group with no OnlineStore holds no record this data plane can serve, and one that
    // was never created resolves to nothing at all.
    let offline = create_feature_group(&router, "wire-offlineonly", false).await;
    assert_eq!(
        offline.status, 200,
        "case \"CreateFeatureGroup without an OnlineStore\": expected HTTP 200, got {} with \
         body {}",
        offline.status, offline.body
    );

    // The class the rule spans: how the name the data plane is given came about —
    // created with an online store (by name and by the ARN the control plane minted),
    // created without one, never created.
    //
    // (case, the FeatureGroupName as the request carries it, whether a WRITE to it is
    //  accepted, whether a READ of it is served)
    let rows: Vec<(&str, String, bool, bool)> = vec![
        (
            "the created group, by name",
            "wire-customers".to_owned(),
            true,
            true,
        ),
        (
            "the created group, by the ARN CreateFeatureGroup answered",
            arn.replace(':', "%3A").replace('/', "%2F"),
            true,
            true,
        ),
        (
            // A write to a group with no OnlineStore is the OfflineStore's business (this
            // group was created with `EnableOnlineStore` false and no `OfflineStoreConfig`,
            // so it has nowhere to replicate to), so it is accepted and stores nothing
            // online; a read of it has nothing to serve and is refused.
            "a group created without an OnlineStore",
            "wire-offlineonly".to_owned(),
            true,
            false,
        ),
        (
            "a group never created",
            "wire-nosuchgroup".to_owned(),
            false,
            false,
        ),
    ];

    for (case, group, write_accepted, read_served) in rows {
        let put = dispatch(
            &router,
            "PUT",
            &format!("/FeatureGroup/{group}"),
            &[],
            Some(json!({ "Record": record("R1", "2026-01-01T00:00:00Z", "original") })),
        )
        .await;
        if write_accepted {
            assert_eq!(
                put.status, 200,
                "case {case:?}: expected PutRecord 200 for a resolved feature group, got {} \
                 with body {}",
                put.status, put.body
            );
        } else {
            assert_eq!(
                (put.status, put.error_code().as_str()),
                (400, "ValidationError"),
                "case {case:?}: expected PutRecord to refuse an unresolvable \
                 FeatureGroupName with HTTP 400 ValidationError, got {} {:?} with body {}",
                put.status,
                put.error_code(),
                put.body
            );
        }

        let got = dispatch(
            &router,
            "GET",
            &format!("/FeatureGroup/{group}?RecordIdentifierValueAsString=R1"),
            &[],
            None,
        )
        .await;
        if read_served {
            let served = got
                .body
                .get("Record")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                (got.status, served),
                (200, 3),
                "case {case:?}: expected GetRecord to serve the three features written \
                 through the other spelling, got {} with body {}",
                got.status,
                got.body
            );
        } else {
            assert_eq!(
                (got.status, got.error_code().as_str()),
                (400, "ValidationError"),
                "case {case:?}: expected GetRecord to refuse a FeatureGroupName with no \
                 online store to serve with HTTP 400 ValidationError — never \
                 ResourceNotFound, which is a missing record in a group that exists — got \
                 {} {:?} with body {}",
                got.status,
                got.error_code(),
                got.body
            );
        }
    }
}

#[tokio::test]
async fn an_update_through_the_whole_router_is_ordered_against_the_stored_records_event_time() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);
    create_feature_group(&router, "wire-updates", true).await;

    let put = dispatch(
        &router,
        "PUT",
        "/FeatureGroup/wire-updates",
        &[],
        Some(json!({ "Record": record("R1", "1792779613", "original") })),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "case \"PutRecord\": expected 200, got {} with body {}",
        put.status, put.body
    );

    // (case, the Features the update posts, expected status, the error code it must
    //  carry, the value feature_a must hold afterwards)
    let rows: Vec<(&str, Value, u16, &str, &str)> = vec![
        (
            "an EventTime older than the record's",
            json!([feature("feature_a", "stale"), feature("event_time", "1792779113")]),
            409,
            "ConflictException",
            "original",
        ),
        (
            "a record that was never ingested",
            json!([feature("feature_a", "ghost")]),
            404,
            "ResourceNotFound",
            "original",
        ),
        (
            "an EventTime equal to the record's",
            json!([feature("feature_a", "equal"), feature("event_time", "1792779613")]),
            200,
            "",
            "equal",
        ),
        (
            "an EventTime newer than the record's",
            json!([feature("feature_a", "fresh"), feature("event_time", "1792779999")]),
            200,
            "",
            "fresh",
        ),
        (
            "no EventTime at all",
            json!([feature("feature_a", "merged")]),
            200,
            "",
            "merged",
        ),
    ];

    for (case, features, expected_status, expected_code, expected_a) in rows {
        let identifier = if expected_status == 404 { "R-ghost" } else { "R1" };
        let updated = dispatch(
            &router,
            "POST",
            "/FeatureGroup/wire-updates/Record",
            &[],
            Some(json!({
                "RecordIdentifierValueAsString": identifier,
                "Features": features
            })),
        )
        .await;
        assert_eq!(
            (updated.status, updated.error_code().as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?}, got {} {:?} \
             with body {}",
            updated.status,
            updated.error_code(),
            updated.body
        );

        let got = dispatch(
            &router,
            "GET",
            "/FeatureGroup/wire-updates?RecordIdentifierValueAsString=R1",
            &[],
            None,
        )
        .await;
        let feature_a = got
            .body
            .get("Record")
            .and_then(Value::as_array)
            .and_then(|features| {
                features
                    .iter()
                    .find(|held| held["FeatureName"] == json!("feature_a"))
            })
            .and_then(|held| held["ValueAsString"].as_str())
            .unwrap_or_default()
            .to_owned();
        assert_eq!(
            feature_a, expected_a,
            "case {case:?}: expected feature_a {expected_a:?} after the update, got \
             {feature_a:?} in {}",
            got.body
        );
    }
}
