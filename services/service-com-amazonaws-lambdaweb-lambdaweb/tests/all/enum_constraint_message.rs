// SPDX-License-Identifier: CC0-1.0

//! What a refusal of a value outside one of this model's enumerated types SAYS, driven
//! over the wire through the same router the emulator mounts.
//!
//! RULE: a value outside an enumerated type is refused with `ValidationException` and
//! AWS's standard constraint sentence — `1 validation error detected: Value '<value>' at
//! '<member path>' failed to satisfy constraint: Member must satisfy enum value set:
//! [<the type's values>]` — whose bracketed set is listed in the order the front door's
//! own TABLE of the admissible values is walked (the table's buckets in ascending order,
//! each bucket being the low bits of the value's own string hash:
//! `service_common::validation_message::hashed_enum_order`), NOT in the order the model
//! declares the type's members in.
//!
//! The order is computed from the values themselves, so it belongs to the enumerated TYPE
//! and holds for every member targeting one: the rows below span the enums this model
//! carries on an input member and the depths they sit at — a member nested four levels
//! down inside `revisionConfig`, one directly under `endpointConfig`, on both create
//! paths — and the recorded case (`applicationLogLevel = "VERBOSE"`) is one row.

use serde_json::{json, Value};

use service_common::validation_message::hashed_enum_order;
use service_com_amazonaws_lambdaweb_lambdaweb::ops::types::{
    APPLICATION_LOG_LEVEL_VALUES, AUTH_TYPE_VALUES, AUTO_DEPLOYMENT_MODE_VALUES,
    ENDPOINT_TYPE_VALUES, SYSTEM_LOG_LEVEL_VALUES,
};

use super::web_function_lifecycle::{build_config, role_arn, Emulator, Reply};

/// The region every request here signs with.
const REGION: &str = "us-east-1";

/// The whole sentence a refusal of `value` at `path` states, for an enumerated type whose
/// admissible values are `declared`.
///
/// Every part outside the angle brackets is fixed text; the value and the member path come
/// from the request, and the bracketed set is the type's own, listed in the order this
/// service lists it.
fn expected_message(value: &str, path: &str, declared: &[&str]) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: Member must satisfy enum value set: [{}]",
        hashed_enum_order(declared).join(", ")
    )
}

/// A `revisionConfig` carrying `logging_config` under its `serviceConfig`.
fn revision_config_with_logging(case: &str, logging_config: Value) -> Value {
    json!({
        "buildConfig": build_config("nodejs24.x", "demo-bucket", "app.zip", None),
        "serviceConfig": {
            "executionRoleArn": role_arn(case),
            "telemetryConfig": { "loggingConfig": logging_config },
        },
    })
}

/// A minimal valid `revisionConfig`, for the rows whose offending member is on the
/// endpoint instead.
fn revision_config(case: &str) -> Value {
    json!({
        "buildConfig": build_config("nodejs24.x", "demo-bucket", "app.zip", None),
        "serviceConfig": { "executionRoleArn": role_arn(case) },
    })
}

/// One row: the case, the create body, the offending member's path, the value it carried
/// and the enum it had to be in.
struct Row {
    case: &'static str,
    body: Value,
    path: &'static str,
    value: &'static str,
    declared: &'static [&'static str],
}

/// The rows: one per enumerated type this model carries on a `CreateWebFunction` input
/// member, at the depth that member sits at.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case.
            case: "applicationLogLevel, four levels down",
            body: json!({
                "functionName": "enum-application-log-level",
                "revisionConfig": revision_config_with_logging(
                    "enum-application-log-level",
                    json!({ "applicationLogLevel": "VERBOSE" }),
                ),
            }),
            path: "revisionConfig.serviceConfig.telemetryConfig.loggingConfig.\
                   applicationLogLevel",
            value: "VERBOSE",
            declared: APPLICATION_LOG_LEVEL_VALUES,
        },
        Row {
            case: "systemLogLevel, the sibling member of the same structure",
            body: json!({
                "functionName": "enum-system-log-level",
                "revisionConfig": revision_config_with_logging(
                    "enum-system-log-level",
                    json!({ "systemLogLevel": "TRACE" }),
                ),
            }),
            path: "revisionConfig.serviceConfig.telemetryConfig.loggingConfig.systemLogLevel",
            value: "TRACE",
            declared: SYSTEM_LOG_LEVEL_VALUES,
        },
        Row {
            case: "endpointType, two levels down",
            body: json!({
                "functionName": "enum-endpoint-type",
                "revisionConfig": revision_config("enum-endpoint-type"),
                "endpointConfig": {
                    "endpointName": "ep",
                    "endpointType": "Global",
                    "authType": "ApplicationManaged",
                },
            }),
            path: "endpointConfig.endpointType",
            value: "Global",
            declared: ENDPOINT_TYPE_VALUES,
        },
        Row {
            case: "authType",
            body: json!({
                "functionName": "enum-auth-type",
                "revisionConfig": revision_config("enum-auth-type"),
                "endpointConfig": {
                    "endpointName": "ep",
                    "endpointType": "HomeRegion",
                    "authType": "None",
                },
            }),
            path: "endpointConfig.authType",
            value: "None",
            declared: AUTH_TYPE_VALUES,
        },
        Row {
            case: "autoDeploymentMode",
            body: json!({
                "functionName": "enum-auto-deployment-mode",
                "revisionConfig": revision_config("enum-auto-deployment-mode"),
                "endpointConfig": {
                    "endpointName": "ep",
                    "endpointType": "HomeRegion",
                    "authType": "ApplicationManaged",
                    "autoDeploymentMode": "Always",
                },
            }),
            path: "endpointConfig.autoDeploymentMode",
            value: "Always",
            declared: AUTO_DEPLOYMENT_MODE_VALUES,
        },
    ]
}

/// The `message` member of a refusal.
fn message(reply: &Reply) -> String {
    reply
        .string("message")
        .or_else(|| reply.string("Message"))
        .unwrap_or_default()
}

/// RULE, on the create path the issue reports: the refusal states the whole sentence, with
/// the enum's value set listed in the order this service lists that type.
#[tokio::test]
async fn a_value_outside_an_enum_is_refused_with_the_value_set_this_service_lists() {
    let emulator = Emulator::new();
    for row in rows() {
        let refused = emulator
            .send("PUT", "/web-functions", REGION, Some(row.body.clone()))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected 400, got {} with {:?}",
            row.case, refused.status, refused.body
        );
        assert_eq!(
            refused.error(),
            "ValidationException",
            "case {:?}: expected ValidationException, got {:?}",
            row.case,
            refused.body
        );
        let expected = expected_message(row.value, row.path, row.declared);
        assert_eq!(
            message(&refused),
            expected,
            "case {:?}: expected {expected:?}, got {:?}",
            row.case,
            message(&refused)
        );
    }
}

/// RULE, over the OTHER operation that carries the same validator: `applicationLogLevel`
/// is stated the same way wherever the member sits, so the revision create — where the
/// member is one level less deep — words it identically.
#[tokio::test]
async fn the_revision_create_states_one_enum_the_same_way() {
    let emulator = Emulator::new();
    let created = emulator
        .send(
            "PUT",
            "/web-functions",
            REGION,
            Some(json!({
                "functionName": "enum-revision-create",
                "revisionConfig": revision_config("enum-revision-create"),
            })),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "the function is created, got {} with {:?}",
        created.status, created.body
    );

    // (case, the logging config, the member path, the value, the enum)
    let rows: [(&str, Value, &str, &str, &[&str]); 2] = [
        (
            "applicationLogLevel",
            json!({ "applicationLogLevel": "VERBOSE" }),
            "serviceConfig.telemetryConfig.loggingConfig.applicationLogLevel",
            "VERBOSE",
            APPLICATION_LOG_LEVEL_VALUES,
        ),
        (
            "systemLogLevel",
            json!({ "systemLogLevel": "FATAL" }),
            "serviceConfig.telemetryConfig.loggingConfig.systemLogLevel",
            "FATAL",
            SYSTEM_LOG_LEVEL_VALUES,
        ),
    ];
    for (case, logging_config, path, value, declared) in rows {
        let refused = emulator
            .send(
                "POST",
                "/web-functions/enum-revision-create/revisions",
                REGION,
                Some(json!({
                    "buildConfig": build_config("nodejs24.x", "demo-bucket", "app.zip", None),
                    "serviceConfig": {
                        "executionRoleArn": role_arn("enum-revision-create"),
                        "telemetryConfig": { "loggingConfig": logging_config },
                    },
                })),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            refused.status, refused.body
        );
        let expected = expected_message(value, path, declared);
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&refused)
        );
    }
}

/// The order this service lists a set in is the one AWS's own refusal of that set states,
/// which is what pins the listing to something other than the model's declaration order.
///
/// The recorded refusal of `ApplicationLogLevel` lists the type as
/// `[FATAL, TRACE, WARN, ERROR, INFO, DEBUG]`, while the model declares it `TRACE, DEBUG,
/// INFO, WARN, ERROR, FATAL` (`smithy_query(query="shape", shape="ApplicationLogLevel")`),
/// so a listing in declaration order — or a sorted one — would be the wrong sentence.
#[test]
fn the_listing_order_is_the_one_the_service_states_and_not_the_models() {
    let stated = hashed_enum_order(APPLICATION_LOG_LEVEL_VALUES).join(", ");
    assert_eq!(
        stated, "FATAL, TRACE, WARN, ERROR, INFO, DEBUG",
        "ApplicationLogLevel is listed as the service states it, got {stated:?}"
    );
    let declared = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL"];
    assert_ne!(
        stated,
        declared.join(", "),
        "the stated order must differ from the model's declaration order"
    );
    let mut sorted: Vec<&str> = APPLICATION_LOG_LEVEL_VALUES.to_vec();
    sorted.sort_unstable();
    assert_ne!(
        stated,
        sorted.join(", "),
        "the stated order must differ from a sort of the values"
    );
}
