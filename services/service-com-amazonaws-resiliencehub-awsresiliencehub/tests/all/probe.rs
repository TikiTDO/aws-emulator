// SPDX-License-Identifier: CC0-1.0

//! The owning-service PROBE a resource mapping makes, where that service cannot be
//! reached.
//!
//! `CreateAppVersionResource` resolves the `physicalResourceId` it is handed against
//! the service that owns a resource of that `resourceType`, over the emulator's own
//! in-process callback, and refuses the mapping when that service answers that it
//! holds no such resource (the cross-service half of that rule is
//! `core/router/tests/all/resiliencehub_resource_mapping_existence.rs`, which needs
//! the owning services mounted).
//!
//! The rule under test HERE is the other half: a mapping is refused only where the
//! component that owns the resource says it holds none, so a question nobody
//! answered never refuses one. With this service mounted ALONE — nothing serves the
//! object store, the queue, the topic, the table or the function — every mapping is
//! accepted and read back, whatever resource class it names.

use serde_json::json;

use super::application_lifecycle::{emulator, post};
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::SUPPORTED_RESOURCE_TYPES;

/// With no owning service reachable, a mapping of any supported resource class is
/// stored and reads back: the emulator refuses nothing on the strength of a probe
/// that could not be answered.
///
/// The rows are every type in the service's catalogue — the classes a probe exists
/// for among them — each named by a physical identifier nothing anywhere created.
#[tokio::test]
async fn a_mapping_no_owner_answered_for_is_accepted() {
    let router = emulator();
    let created = post(&router, "/create-app", &json!({"name": "unprobed-mappings"})).await;
    let app_arn = created.ok("the application")["app"]["appArn"]
        .as_str()
        .expect("a minted appArn")
        .to_string();

    for (index, resource_type) in SUPPORTED_RESOURCE_TYPES.iter().enumerate() {
        let logical = format!("Unprobed{index}");
        let physical = format!("unprobed-{index}");
        let mapped = post(
            &router,
            "/create-app-version-resource",
            &json!({
                "appArn": app_arn,
                "logicalResourceId": {"identifier": logical},
                "physicalResourceId": physical,
                "resourceType": resource_type,
                "appComponents": ["appcommon"],
            }),
        )
        .await;
        mapped.ok(resource_type);

        let read = post(
            &router,
            "/describe-app-version-resource",
            &json!({
                "appArn": app_arn,
                "appVersion": "draft",
                "logicalResourceId": {"identifier": logical},
            }),
        )
        .await;
        let read = read.ok(resource_type);
        assert_eq!(
            read["physicalResource"]["physicalResourceId"]["identifier"].as_str(),
            Some(physical.as_str()),
            "case {resource_type:?}: the accepted mapping must read back under the physical \
             identifier the request named, got {read:?}"
        );
    }
}
