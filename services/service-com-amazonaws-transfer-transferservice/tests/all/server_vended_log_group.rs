// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs group a server's logging configuration provisions.
//!
//! The rule under test: a server whose configuration names a `LoggingRole` publishes to
//! the SERVICE-managed log group named after it — the service's logging prefix followed
//! by that server's own `ServerId` — so the write that configures the role creates that
//! group in CloudWatch Logs, and a client reading the group list finds exactly it. A
//! server configured with no logging role provisions none: there is nothing publishing
//! to a managed destination.
//!
//! What pins the NAME is the group it has to name: CloudWatch Logs' own `LogGroupName` is
//! 1..512 characters matching `^[\.\-_/#A-Za-z0-9]+$`
//! (`smithy_query(query="shape",shape="LogGroupName",model_path="services/service-com-\
//! amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-\
//! logs-20140328.json")`), and the server part of it is a `ServerId`, modelled
//! `len 19..19` with pattern `^s-([0-9a-f]{17})$`.
//!
//! The rows span what the rule generalises over: whether the role is configured by the
//! create or by a later update, the empty value that configures NO role, how many
//! servers an account holds at once (each gets its own group, and no two share one), and
//! a repeated write of the same configuration (which adopts the one group rather than
//! creating a second).
//!
//! The group is read back through CloudWatch Logs' own `DescribeLogGroups` — the service
//! that OWNS it, mounted beside this one by the shared [`crate::harness`] — so what is
//! asserted is a group that exists for a client, not a name this service remembers.

use serde_json::{json, Value};

use crate::harness::{Emulator, REGION};

/// The prefix the service-managed log group of a server carries, as a client builds it.
const LOG_GROUP_PREFIX: &str = "/aws/transfer/";

/// The bound `LogGroupName` places on the name the group is created under.
const MAX_LOG_GROUP_NAME_LEN: usize = 512;

/// A logging role ARN satisfying `NullableRole`'s modelled pattern
/// `^(|arn:.*role/\S+)$`.
const LOGGING_ROLE: &str = "arn:aws:iam::000000000000:role/transfer-logs";

/// A second one, so a row that re-configures the role changes its value too.
const OTHER_LOGGING_ROLE: &str = "arn:aws:iam::000000000000:role/transfer-logs-2";

/// The service-managed group name a client expects for `server_id`.
fn expected_group(server_id: &str) -> String {
    format!("{LOG_GROUP_PREFIX}{server_id}")
}

/// Whether `character` may appear in a `LogGroupName`: its pattern is
/// `^[\.\-_/#A-Za-z0-9]+$`.
fn is_log_group_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '/' | '#')
}

/// The `logGroupName`s CloudWatch Logs reports under `prefix`, read through its own
/// `DescribeLogGroups`.
async fn log_groups_under(emulator: &Emulator, case: &str, prefix: &str) -> Vec<String> {
    let answer = emulator
        .call_service(
            REGION,
            "logs",
            "Logs_20140328.DescribeLogGroups",
            json!({"logGroupNamePrefix": prefix}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeLogGroups 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .get("logGroups")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|group| {
            group
                .get("logGroupName")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// Assert `name` is a name CloudWatch Logs admits for a log group.
fn assert_admissible_name(case: &str, name: &str) {
    assert!(
        !name.is_empty() && name.chars().count() <= MAX_LOG_GROUP_NAME_LEN,
        "case {case:?}: the group name must be 1..{MAX_LOG_GROUP_NAME_LEN} characters, \
         got {} in {name:?}",
        name.chars().count()
    );
    assert!(
        name.chars().all(is_log_group_character),
        "case {case:?}: the group name must match LogGroupName's pattern, got {name:?}"
    );
}

/// RULE: the group named after a server exists exactly when that server has a logging
/// role configured, whichever write configured it.
#[tokio::test]
async fn a_logging_role_provisions_the_group_named_after_the_server() {
    let emulator = Emulator::with_logs();

    // (case, CreateServer request, UpdateServer requests applied in order, whether the
    // server ends up with a logging role configured)
    let cases: Vec<(&str, Value, Vec<Value>, bool)> = vec![
        (
            "no logging member at all: nothing publishes to a managed group",
            json!({}),
            vec![],
            false,
        ),
        (
            // `NullableRole`'s pattern admits the empty string for exactly this purpose.
            "an empty LoggingRole configures no role",
            json!({"LoggingRole": ""}),
            vec![],
            false,
        ),
        (
            "a structured destination alone: the server declares its own group",
            json!({
                "StructuredLogDestinations": [
                    "arn:aws:logs:us-east-1:000000000000:log-group:declared:*",
                ],
            }),
            vec![],
            false,
        ),
        (
            "the create configures the role",
            json!({"LoggingRole": LOGGING_ROLE}),
            vec![],
            true,
        ),
        (
            "an update configures the role on a server created without one",
            json!({}),
            vec![json!({"LoggingRole": LOGGING_ROLE})],
            true,
        ),
        (
            "an update replaces the role the create configured",
            json!({"LoggingRole": LOGGING_ROLE}),
            vec![json!({"LoggingRole": OTHER_LOGGING_ROLE})],
            true,
        ),
        (
            "an update that configures the same role again adopts the one group",
            json!({"LoggingRole": LOGGING_ROLE}),
            vec![
                json!({"LoggingRole": LOGGING_ROLE}),
                json!({"LoggingRole": LOGGING_ROLE}),
            ],
            true,
        ),
        (
            // Clearing the role leaves the logs already in the group where they are.
            "an update clears the role the create configured",
            json!({"LoggingRole": LOGGING_ROLE}),
            vec![json!({"LoggingRole": ""})],
            true,
        ),
        (
            "an update that names no logging member provisions nothing",
            json!({}),
            vec![json!({"PreAuthenticationLoginBanner": "unrelated"})],
            false,
        ),
    ];

    for (case, create, updates, expects_group) in cases {
        let server_id = emulator.create_server(create).await;
        for update in updates {
            let mut request = update;
            request
                .as_object_mut()
                .expect("an update request object")
                .insert("ServerId".to_string(), json!(&server_id));
            let answer = emulator.call("UpdateServer", request).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected UpdateServer 200, got {} with body {}",
                answer.status, answer.body
            );
        }

        let group = expected_group(&server_id);
        assert_admissible_name(case, &group);
        let reported = log_groups_under(&emulator, case, &group).await;
        let expected: Vec<String> = if expects_group {
            vec![group.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected DescribeLogGroups to report {expected:?} for \
             {server_id}, got {reported:?}"
        );
    }
}

/// RULE (the same one, over the group SET): every server with a logging role has a group
/// of its OWN, and the groups under the prefix are exactly those servers' — no server
/// without a role contributes one, and no two servers share a group.
#[tokio::test]
async fn each_logging_server_gets_its_own_group_and_no_other_group_appears() {
    let emulator = Emulator::with_logs();
    let case = "four servers, three of which log to the managed destination";

    let mut expected: Vec<String> = Vec::new();
    for request in [
        json!({"LoggingRole": LOGGING_ROLE}),
        json!({"LoggingRole": OTHER_LOGGING_ROLE}),
        json!({}),
        json!({"LoggingRole": LOGGING_ROLE, "Protocols": ["SFTP"]}),
    ] {
        let logs = request.get("LoggingRole").is_some();
        let server_id = emulator.create_server(request).await;
        if logs {
            expected.push(expected_group(&server_id));
        }
    }
    expected.sort();

    let mut reported = log_groups_under(&emulator, case, LOG_GROUP_PREFIX).await;
    reported.sort();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the groups under {LOG_GROUP_PREFIX:?} to be exactly \
         {expected:?}, got {reported:?}"
    );
    for group in &reported {
        assert_admissible_name(case, group);
    }
}
