// SPDX-License-Identifier: CC0-1.0

//! Reading an invocation's delivered log back by FILTER PATTERN, which is how a client polls
//! for a line a handler printed.
//!
//! [`crate::invocation_log_delivery`] covers what the delivery WRITES — which group, which
//! stream, which events, and what the function's execution role has to allow for each write —
//! and pages the result with `GetLogEvents`. This suite covers the other end of the same rule:
//! that what was written is selectable through CloudWatch Logs' `FilterLogEvents` by a
//! `filterPattern`, which is the reader a client uses to WAIT for one marker line rather than
//! to page a whole stream. A quoted term in that syntax is one of "the terms the message has
//! to carry", so a pattern naming the handler's marker selects the handler's own lines and
//! none of the `START`/`END`/`REPORT` framing around them, and a term no line carries selects
//! nothing — which is what makes an empty page mean "not written yet" rather than "this reader
//! finds nothing".
//!
//! The rows differ along what the rule generalises over: the engine the handler runs (node,
//! python), how many lines it writes (one, several), and which group it writes to (the default
//! `/aws/lambda/<function name>`, and one the function's `LoggingConfig` names) — because
//! which reader finds a line is a property of none of those.
//!
//! The harness is the one that suite already states — one emulator with Lambda, CloudWatch
//! Logs and IAM on ONE engine, `create_function` waiting out the modelled transition, and
//! `invoke` reporting the run's own log tail beside the payload — so the two suites cannot
//! drift about what a delivery is. Both RUN handlers, so each takes the target's one
//! invocation stream ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use serde_json::json;

use crate::invocation_log_delivery::{
    create_function, emulator, invoke, logs, logs_answer, Created, NODE_RUNTIME, PYTHON_RUNTIME,
};

/// The execution role every row runs as. No policy is put on it, so nothing is known about it
/// and no log write is withheld — which dimension [`crate::invocation_log_delivery`] walks.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// One row: what the function runs, what it writes and where its log is expected.
struct Row {
    case: &'static str,
    /// The handler's source.
    source: &'static str,
    /// The runtime that runs it.
    runtime: &'static str,
    /// The lines the HANDLER itself writes, each of which the pattern below has to select.
    written: &'static [&'static str],
    /// The group the function's configuration names, or `None` to take the default.
    log_group: Option<&'static str>,
    /// The term a client polls with, which every one of `written` carries and no framing line
    /// does.
    marker: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a node handler writing one line to the default group",
            source: "exports.handler = async () => {\n    console.log('INVOKED:ALLOWED-CALL');\n \
                     return { ok: true };\n};",
            runtime: NODE_RUNTIME,
            written: &["INVOKED:ALLOWED-CALL"],
            log_group: None,
            marker: "INVOKED:",
        },
        Row {
            case: "a python handler writing several lines to the group its configuration names",
            source: "def lambda_handler(event, context):\n    for index in range(3):\n        \
                     print('MARKED:' + str(index))\n    return {'ok': True}\n",
            runtime: PYTHON_RUNTIME,
            written: &["MARKED:0", "MARKED:1", "MARKED:2"],
            log_group: Some("/aws/lambda/filtered-elsewhere"),
            marker: "MARKED:",
        },
    ]
}

/// The messages of the events `FilterLogEvents` selects in `group` with `filter_pattern`, in
/// the order it reports them.
///
/// `pub(crate)` because the sibling suite that walks HOW the logs grant reaches the execution
/// role ([`crate::invocation_log_delivery_grant_carrier`]) polls the delivery with the SAME
/// reader a client polls with, rather than a second copy of it.
pub(crate) async fn filtered(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    group: &str,
    filter_pattern: &str,
) -> Vec<String> {
    let answered = logs(
        router,
        case,
        "FilterLogEvents",
        &json!({ "logGroupName": group, "filterPattern": filter_pattern }),
    )
    .await;
    answered["events"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: FilterLogEvents must answer events, got {answered}")
        })
        .iter()
        .map(|event| {
            event["message"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: every event carries a message"))
                .to_string()
        })
        .collect()
}

/// RULE: what an invocation's log delivery wrote is selectable through `FilterLogEvents` by a
/// `filterPattern`, and a quoted term selects exactly the lines that carry it — the handler's
/// own output, never the framing the runtime wrote around it.
#[tokio::test]
async fn a_delivered_log_line_is_selectable_by_filter_pattern() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let name = format!("filter-probe-{index}");
        let group = row
            .log_group
            .map(str::to_string)
            .unwrap_or_else(|| format!("/aws/lambda/{name}"));
        create_function(
            &router,
            &Created {
                case,
                name: &name,
                source: row.source,
                runtime: row.runtime,
                role: ROLE,
                log_group: row.log_group,
            },
        )
        .await;
        // The invocation's own answer states what the run wrote, which is the independent
        // statement of what the reader below has to find.
        let (log, payload) = invoke(&router, case, &name).await;
        for line in row.written {
            assert!(
                log.contains(line),
                "case {case:?}: the run must have written {line:?}, got {log:?}"
            );
        }
        assert_eq!(
            payload,
            json!({ "ok": true }),
            "case {case:?}: expected the handler's own value, got {payload}"
        );

        // The whole log is there to read with the empty pattern, which matches every event.
        let every = filtered(&router, case, &group, "").await;
        for record in ["START", "END", "REPORT"] {
            assert!(
                every
                    .iter()
                    .any(|message| message.starts_with(&format!("{record} RequestId: "))),
                "case {case:?}: expected a {record} record in {group:?}, got {every:?}"
            );
        }

        // The quoted term selects the handler's own lines, in the order it wrote them, and
        // nothing of the framing — which is what a client polling for one marker reads.
        let selected = filtered(&router, case, &group, &format!("\"{}\"", row.marker)).await;
        assert_eq!(
            selected,
            row.written.to_vec(),
            "case {case:?}: expected the pattern {:?} to select {:?}, got {selected:?}",
            row.marker,
            row.written
        );

        // A term no line carries selects nothing, so an empty page means "not written yet"
        // rather than "this reader finds nothing".
        let absent = filtered(&router, case, &group, "\"NOTHING-WROTE-THIS\"").await;
        assert!(
            absent.is_empty(),
            "case {case:?}: expected no event for a term no line carries, got {absent:?}"
        );

        // A function that names its own group logs THERE: the default group it would otherwise
        // have used was never opened, so the reader is told the group is unknown.
        if row.log_group.is_some() {
            let (status, answered) = logs_answer(
                &router,
                case,
                "FilterLogEvents",
                &json!({ "logGroupName": format!("/aws/lambda/{name}") }),
            )
            .await;
            let code = answered["__type"]
                .as_str()
                .map(|raw| raw.rsplit(['#', '/']).next().unwrap_or(raw).to_string())
                .unwrap_or_default();
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: the default group must not exist for a function that names \
                 another, got {status} {answered}"
            );
        }
    }
}
