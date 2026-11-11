// SPDX-License-Identifier: CC0-1.0

//! What a revision's logging configuration reports for a member the request left out.
//!
//! RULE: each member of `loggingConfig` documents a default in the same words — "If you
//! don't specify a value, the default is <value>, and this default is returned in the
//! response" (`smithy_query(query="shape", shape="LoggingConfig")`: `logGroup` ->
//! `/aws/lambda/web/{functionName}`, `applicationLogLevel` -> `INFO`, `systemLogLevel` ->
//! `INFO`) — so a revision stores the EFFECTIVE configuration and every read of it reports
//! the default for a member that arrived without a value, and the submitted value for one
//! that did.
//!
//! The rows span what the rule generalises over: which of the three members the request
//! supplies (none, one, two, all) and how deep the configuration it supplies reaches (no
//! `telemetryConfig` at all, an empty one, an empty `loggingConfig`), on both create paths.

use serde_json::{json, Value};

use service_com_amazonaws_lambdaweb_lambdaweb::state::{
    default_log_group, DEFAULT_APPLICATION_LOG_LEVEL, DEFAULT_SYSTEM_LOG_LEVEL,
};

use super::web_function_lifecycle::{build_config, role_arn, Emulator, Reply};

/// The region every request here signs with.
const REGION: &str = "us-east-1";

/// One row: the case, the `serviceConfig` the request carries beside its execution role,
/// and the three members the response must report (`None` meaning "the documented
/// default").
struct Row {
    case: &'static str,
    supplied: Value,
    log_group: Option<&'static str>,
    application_log_level: Option<&'static str>,
    system_log_level: Option<&'static str>,
}

/// The rows both create paths are driven over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no telemetryConfig at all",
            supplied: json!({}),
            log_group: None,
            application_log_level: None,
            system_log_level: None,
        },
        Row {
            case: "a telemetryConfig with no loggingConfig",
            supplied: json!({ "telemetryConfig": {} }),
            log_group: None,
            application_log_level: None,
            system_log_level: None,
        },
        Row {
            case: "an empty loggingConfig",
            supplied: json!({ "telemetryConfig": { "loggingConfig": {} } }),
            log_group: None,
            application_log_level: None,
            system_log_level: None,
        },
        Row {
            case: "only the log group",
            supplied: json!({
                "telemetryConfig": { "loggingConfig": { "logGroup": "/aws/lambda/mine" } }
            }),
            log_group: Some("/aws/lambda/mine"),
            application_log_level: None,
            system_log_level: None,
        },
        Row {
            case: "only the application log level, at the most severe value",
            supplied: json!({
                "telemetryConfig": { "loggingConfig": { "applicationLogLevel": "FATAL" } }
            }),
            log_group: None,
            application_log_level: Some("FATAL"),
            system_log_level: None,
        },
        Row {
            case: "only the system log level, at the least severe value",
            supplied: json!({
                "telemetryConfig": { "loggingConfig": { "systemLogLevel": "DEBUG" } }
            }),
            log_group: None,
            application_log_level: None,
            system_log_level: Some("DEBUG"),
        },
        Row {
            case: "all three supplied",
            supplied: json!({
                "telemetryConfig": {
                    "loggingConfig": {
                        "logGroup": "/aws/lambda/all-three",
                        "applicationLogLevel": "WARN",
                        "systemLogLevel": "WARN",
                    }
                }
            }),
            log_group: Some("/aws/lambda/all-three"),
            application_log_level: Some("WARN"),
            system_log_level: Some("WARN"),
        },
    ]
}

/// The `serviceConfig` a row sends: its own members plus the required execution role.
fn service_config(row: &Row, function_name: &str) -> Value {
    let mut config = row.supplied.clone();
    config["executionRoleArn"] = json!(role_arn(function_name));
    config
}

/// The `loggingConfig` of a revision answer.
fn logging_config(reply: &Reply) -> Value {
    reply
        .field("serviceConfig")
        .and_then(|config| config.get("telemetryConfig"))
        .and_then(|telemetry| telemetry.get("loggingConfig"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// Assert `reported` carries exactly the three effective values `row` expects for the
/// function `function_name`.
fn assert_effective(case: &str, where_from: &str, reported: &Value, row: &Row, function_name: &str) {
    let default_group = default_log_group(function_name);
    let expected = [
        (
            "logGroup",
            row.log_group.unwrap_or(default_group.as_str()).to_string(),
        ),
        (
            "applicationLogLevel",
            row.application_log_level
                .unwrap_or(DEFAULT_APPLICATION_LOG_LEVEL)
                .to_string(),
        ),
        (
            "systemLogLevel",
            row.system_log_level
                .unwrap_or(DEFAULT_SYSTEM_LOG_LEVEL)
                .to_string(),
        ),
    ];
    for (member, value) in expected {
        assert_eq!(
            reported.get(member).and_then(Value::as_str),
            Some(value.as_str()),
            "case {case:?} ({where_from}): expected loggingConfig.{member} {value:?}, got \
             {reported:?}"
        );
    }
}

/// RULE, on the create that brings its own initial revision: the revision the create
/// reports — and the one a later read reports — carries the effective logging
/// configuration.
#[tokio::test]
async fn the_initial_revision_reports_the_effective_logging_configuration() {
    for row in rows() {
        let emulator = Emulator::new();
        let function_name = "logging-defaults-initial";
        let created = emulator
            .send(
                "PUT",
                "/web-functions",
                REGION,
                Some(json!({
                    "functionName": function_name,
                    "revisionConfig": {
                        "buildConfig": build_config("nodejs24.x", "demo-bucket", "app.zip", None),
                        "serviceConfig": service_config(&row, function_name),
                    },
                })),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {:?}: CreateWebFunction expected 202, got {} with {:?}",
            row.case, created.status, created.body
        );
        let revision_id = created
            .field("revision")
            .and_then(|revision| revision.get("revisionId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !revision_id.is_empty(),
            "case {:?}: the create reports its initial revision, got {:?}",
            row.case,
            created.body
        );

        let read = emulator
            .send(
                "GET",
                &format!("/web-functions/{function_name}/revisions/{revision_id}"),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: GetWebFunctionRevision expected 200, got {} with {:?}",
            row.case, read.status, read.body
        );
        assert_effective(
            row.case,
            "the read-back",
            &logging_config(&read),
            &row,
            function_name,
        );
    }
}

/// RULE, on the other create path: a revision added to an existing function reports the
/// same effective configuration, in the create's own answer and in the read-back.
#[tokio::test]
async fn a_later_revision_reports_the_effective_logging_configuration() {
    for row in rows() {
        let emulator = Emulator::new();
        let function_name = "logging-defaults-later";
        let created = emulator
            .send(
                "PUT",
                "/web-functions",
                REGION,
                Some(json!({ "functionName": function_name })),
            )
            .await;
        assert_eq!(
            created.status, 202,
            "case {:?}: the function is created, got {} with {:?}",
            row.case, created.status, created.body
        );

        let revision = emulator
            .send(
                "POST",
                &format!("/web-functions/{function_name}/revisions"),
                REGION,
                Some(json!({
                    "buildConfig": build_config("nodejs24.x", "demo-bucket", "app.zip", None),
                    "serviceConfig": service_config(&row, function_name),
                })),
            )
            .await;
        assert_eq!(
            revision.status, 202,
            "case {:?}: CreateWebFunctionRevision expected 202, got {} with {:?}",
            row.case, revision.status, revision.body
        );
        assert_effective(
            row.case,
            "the create's answer",
            &logging_config(&revision),
            &row,
            function_name,
        );

        let revision_id = revision.string("revisionId").unwrap_or_default();
        let read = emulator
            .send(
                "GET",
                &format!("/web-functions/{function_name}/revisions/{revision_id}"),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {:?}: GetWebFunctionRevision expected 200, got {} with {:?}",
            row.case, read.status, read.body
        );
        assert_effective(
            row.case,
            "the read-back",
            &logging_config(&read),
            &row,
            function_name,
        );
    }
}
