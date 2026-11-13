// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `HistorySummary` of a `ConfigurationUpdate` item: the audit line
//! `DescribeAlarmHistory` reports for a write that configured an alarm.
//!
//! RULE under test: a `ConfigurationUpdate` summary ("A summary of the alarm history, in text
//! format.", `AlarmHistoryItem$HistorySummary`, `@length 1..255`) NAMES the alarm it is about,
//! in double quotes, and says what the write did to it — a write that stored an alarm of a name
//! no alarm held `created` it, and one that replaced an alarm that already existed `updated` it.
//! It holds for every writer that configures an alarm, whichever kind of alarm it writes, so a
//! page of history mixing several alarms (`DescribeAlarmHistory` can be called with no
//! `AlarmName`) attributes each item to its own alarm from the summary alone.
//!
//! The rows span what the rule generalises over: the kind of alarm (the three `AlarmType`
//! values), whether the write created or replaced it, the form of the name interpolated, and
//! which writer configured the alarm — the three `Put*Alarm` operations and the action switches,
//! which configure an alarm without replacing it.

use crate::harness::{alarm_history_items, build_router, ok, AlarmKind};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The item type under test (`smithy_query(query="shape",shape="HistoryItemType")`:
/// ConfigurationUpdate, StateUpdate, Action, AlarmContributorStateUpdate,
/// AlarmContributorAction).
const CONFIGURATION_UPDATE: &str = "ConfigurationUpdate";

/// The modelled maximum length of `HistorySummary` and of `AlarmName` alike
/// (`smithy_query(query="shape",shape="HistorySummary,AlarmName")`: both `@length 1..255`), so a
/// name at its own bound cannot fit inside a summary that quotes it.
const MAX_SUMMARY_LEN: usize = 255;

/// The summary a write that CREATED the alarm `name` records.
fn created(name: &str) -> String {
    format!("Alarm \"{name}\" created")
}

/// The summary a write that REPLACED the configuration of the alarm `name` records.
fn updated(name: &str) -> String {
    format!("Alarm \"{name}\" updated")
}

/// Every `ConfigurationUpdate` summary recorded for `name`, oldest first.
async fn summaries(router: &MultiServiceRouter, kind: AlarmKind, name: &str) -> Vec<String> {
    let mut items = alarm_history_items(router, kind, name, CONFIGURATION_UPDATE).await;
    // The reader's default direction is newest first; a summary is asserted against the order
    // the writes happened in.
    items.reverse();
    items
        .iter()
        .map(|item| {
            item.get("HistorySummary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE: the create records `created` and a re-put of the same alarm records `updated`, each
/// naming the alarm, for an alarm of any kind and a name of any form.
#[tokio::test]
async fn a_configuration_write_records_what_it_did_to_the_alarm_it_names() {
    // Rows differ along the kind of alarm written and the form of the name interpolated.
    let cases: [(&str, AlarmKind, &str); 7] = [
        ("a metric alarm", AlarmKind::Metric, "summary-metric"),
        ("a composite alarm", AlarmKind::Composite, "summary-composite"),
        ("a log alarm", AlarmKind::Log, "summary-log"),
        ("a name with spaces", AlarmKind::Metric, "orders p99 latency"),
        ("a name with punctuation", AlarmKind::Metric, "svc.api/5xx_rate-1"),
        ("a one-character name", AlarmKind::Metric, "a"),
        (
            "a name that looks like a summary",
            AlarmKind::Metric,
            "Alarm \"inner\" created",
        ),
    ];
    for (case, kind, name) in cases {
        let router = build_router();

        kind.create(&router, name, None).await;
        let after_create = summaries(&router, kind, name).await;
        assert_eq!(
            after_create,
            vec![created(name)],
            "case {case:?}: the write that created the alarm must record {:?}, got \
             {after_create:?}",
            created(name)
        );

        kind.create(&router, name, None).await;
        let after_update = summaries(&router, kind, name).await;
        assert_eq!(
            after_update,
            vec![created(name), updated(name)],
            "case {case:?}: the re-put must add {:?}, got {after_update:?}",
            updated(name)
        );
    }
}

/// RULE: the action switches configure the alarm they name without replacing it, so each write
/// that CHANGES the setting records the same `updated` summary — naming the alarm.
#[tokio::test]
async fn an_action_switch_records_an_update_naming_the_alarm_it_changed() {
    // Rows span the kinds that hold the member and both directions of the switch.
    let cases: [(&str, AlarmKind, &str, bool); 4] = [
        ("a metric alarm turned on", AlarmKind::Metric, "switch-metric", true),
        (
            "a composite alarm turned on",
            AlarmKind::Composite,
            "switch-composite",
            true,
        ),
        ("a log alarm turned on", AlarmKind::Log, "switch-log", true),
        (
            "a metric alarm turned off",
            AlarmKind::Metric,
            "switch-metric-off",
            false,
        ),
    ];
    for (case, kind, name, enable) in cases {
        let router = build_router();
        // Created with the setting the switch will CHANGE, so the write is a real change
        // rather than the no-op that asking for the setting an alarm already has is.
        kind.create(&router, name, Some(!enable)).await;
        let operation = if enable {
            "EnableAlarmActions"
        } else {
            "DisableAlarmActions"
        };
        ok(&router, operation, &json!({"AlarmNames": [name]})).await;

        let recorded = summaries(&router, kind, name).await;
        assert_eq!(
            recorded,
            vec![created(name), updated(name)],
            "case {case:?}: {operation} must record {:?} after the create, got {recorded:?}",
            updated(name)
        );

        // Asking for the setting the alarm already has changes nothing, so it records nothing.
        ok(&router, operation, &json!({"AlarmNames": [name]})).await;
        let unchanged = summaries(&router, kind, name).await;
        assert_eq!(
            unchanged, recorded,
            "case {case:?}: a second {operation} changes nothing, so it records nothing, got \
             {unchanged:?}"
        );
    }
}

/// RULE at the modelled bound: a name long enough that the summary quoting it would overrun
/// `HistorySummary`'s own `@length 1..255` is cut at that bound rather than dropping the
/// summary, and what is kept is the beginning — the wording and the start of the name.
#[tokio::test]
async fn a_summary_naming_a_long_alarm_stays_inside_the_modelled_bound() {
    // Rows: a name whose summary fits exactly, one character past that, and a name at
    // `AlarmName`'s own maximum.
    let fitting = MAX_SUMMARY_LEN - created("").len();
    let cases: [(&str, usize); 3] = [
        ("a name whose summary fits exactly", fitting),
        ("one character past the bound", fitting + 1),
        ("a name at the name bound", MAX_SUMMARY_LEN),
    ];
    for (case, name_len) in cases {
        let router = build_router();
        let name = "n".repeat(name_len);
        AlarmKind::Metric.create(&router, &name, None).await;

        let recorded = summaries(&router, AlarmKind::Metric, &name).await;
        let whole = created(&name);
        let expected: String = whole.chars().take(MAX_SUMMARY_LEN).collect();
        assert_eq!(
            recorded,
            vec![expected.clone()],
            "case {case:?}: expected the summary {expected:?}, got {recorded:?}"
        );
        let length = recorded.first().map_or(0, String::len);
        assert!(
            (1..=MAX_SUMMARY_LEN).contains(&length),
            "case {case:?}: a summary stays inside 1..={MAX_SUMMARY_LEN}, got {length}"
        );
    }
}
