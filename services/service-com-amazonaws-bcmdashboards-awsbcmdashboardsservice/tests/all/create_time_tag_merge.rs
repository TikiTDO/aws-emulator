// SPDX-License-Identifier: CC0-1.0

//! Create-time tagging over the wire: a created resource's tags are resolved BY TAG KEY.
//!
//! The rule under test: a resource's tags are keyed by tag key, so whichever path
//! supplies them, the stored list holds each key once — the value the request named LAST
//! for that key, at the position the key first appeared. `ResourceTagList` is a plain
//! list the model lets repeat a key (length 0..200, no `uniqueItems`; source:
//! `smithy_query(query="shape", shape="ResourceTagList")`), so create-time tagging
//! resolves a repeated key exactly as applying those entries one request at a time
//! through `TagResource` does — the equivalence each test below asserts.
//!
//! Both create paths that take `resourceTags` are covered (`CreateDashboard` and
//! `CreateScheduledReport`), because the rule is about a resource's tags, not about one
//! operation. The rows differ in how often a key repeats, where the repeats sit, how many
//! keys repeat, what the repeated values are, and the modelled key/value/list bounds.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees: the tags a read reports come from the persisted record.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{ok_call, router};
use super::scheduled_report_lifecycle::EXECUTION_ROLE_ARN;
use super::untag_resource::{create_dashboard_with_tags, pairs, stored_tags, tags_json, Xorshift};

/// One row of the create-time table: the tag list the create request carries and the
/// tags the resource must carry afterwards — in order.
struct Case {
    label: String,
    supplied: Vec<(String, String)>,
    expected: Vec<(String, String)>,
}

/// The table. Bounds come from the model: `ResourceTag.key` length 1..128,
/// `ResourceTag.value` length 0..256, `ResourceTagList` length 0..200 (source:
/// `smithy_query(query="shape", shape="ResourceTag,ResourceTagList")`).
fn create_cases() -> Vec<Case> {
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);

    let mut cases = vec![
        Case {
            label: "distinct keys are stored as they arrived".to_string(),
            supplied: pairs(&[("env", "prod"), ("owner", "fin")]),
            expected: pairs(&[("env", "prod"), ("owner", "fin")]),
        },
        Case {
            label: "one key twice keeps the last value at the first position".to_string(),
            supplied: pairs(&[("env", "a"), ("env", "b")]),
            expected: pairs(&[("env", "b")]),
        },
        Case {
            label: "one key three times keeps the last value".to_string(),
            supplied: pairs(&[("env", "a"), ("env", "b"), ("env", "c")]),
            expected: pairs(&[("env", "c")]),
        },
        Case {
            label: "a repeat with other keys in between keeps the first position".to_string(),
            supplied: pairs(&[("env", "a"), ("owner", "fin"), ("env", "b")]),
            expected: pairs(&[("env", "b"), ("owner", "fin")]),
        },
        Case {
            label: "two keys each repeated".to_string(),
            supplied: pairs(&[
                ("env", "a"),
                ("tier", "silver"),
                ("env", "b"),
                ("tier", "gold"),
            ]),
            expected: pairs(&[("env", "b"), ("tier", "gold")]),
        },
        Case {
            label: "the repeat carries the same value".to_string(),
            supplied: pairs(&[("env", "prod"), ("env", "prod")]),
            expected: pairs(&[("env", "prod")]),
        },
        Case {
            label: "the last value is the empty string the value minimum admits".to_string(),
            supplied: pairs(&[("env", "prod"), ("env", "")]),
            expected: pairs(&[("env", "")]),
        },
        Case {
            label: "a repeat at the 128-character key and 256-character value maxima".to_string(),
            supplied: vec![
                (longest_key.clone(), "first".to_string()),
                (longest_key.clone(), longest_value.clone()),
            ],
            expected: vec![(longest_key, longest_value)],
        },
        Case {
            label: "keys differing only in case are different keys".to_string(),
            supplied: pairs(&[("Env", "a"), ("env", "b"), ("ENV", "c")]),
            expected: pairs(&[("Env", "a"), ("env", "b"), ("ENV", "c")]),
        },
        Case {
            label: "no tags at all (the list minimum)".to_string(),
            supplied: Vec::new(),
            expected: Vec::new(),
        },
    ];

    // The list maximum, every entry of it a repeat of one of 100 keys: a create may carry
    // 200 entries, and the resource ends up holding the 100 keys those entries resolve to.
    let mut bulk: Vec<(String, String)> = Vec::new();
    for round in 0..2 {
        for index in 0..100 {
            bulk.push((format!("key-{index:03}"), format!("round-{round}")));
        }
    }
    let bulk_expected: Vec<(String, String)> = (0..100)
        .map(|index| (format!("key-{index:03}"), "round-1".to_string()))
        .collect();
    cases.push(Case {
        label: "the 200-entry list maximum, every key repeated once".to_string(),
        supplied: bulk,
        expected: bulk_expected,
    });

    cases
}

/// Create a scheduled report on `dashboard_arn` carrying `tags`, and return the ARN it
/// was stored under. The other resource type a `ResourceArn` names.
async fn create_report_with_tags(
    router: &MultiServiceRouter,
    name: &str,
    dashboard_arn: &str,
    tags: &[(String, String)],
) -> String {
    let created = ok_call(
        router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": name,
                "dashboardArn": dashboard_arn,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 12 L * ? *)"}
            },
            "resourceTags": tags_json(tags),
        }),
    )
    .await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: CreateScheduledReport returned no arn: {created}"))
        .to_string()
}

/// Apply `tags` one entry at a time through `TagResource`, which is the other way a
/// caller asks for the same tag set.
async fn tag_one_at_a_time(router: &MultiServiceRouter, arn: &str, tags: &[(String, String)]) {
    for (key, value) in tags {
        ok_call(
            router,
            "TagResource",
            &json!({
                "resourceArn": arn,
                "resourceTags": [{"key": key, "value": value}],
            }),
        )
        .await;
    }
}

/// A dashboard created with a tag list carries each key once, holding the value the
/// request named last for it, at the position that key first appeared.
#[tokio::test]
async fn a_created_dashboard_stores_each_tag_key_once() {
    for (index, case) in create_cases().into_iter().enumerate() {
        let router = router();
        let name = format!("Created Tags {index}");
        let arn = create_dashboard_with_tags(&router, &name, &case.supplied).await;

        let stored = stored_tags(&router, &arn, &case.label).await;
        assert_eq!(
            stored, case.expected,
            "case {}: expected the resolved tags {:?}, got {stored:?}",
            case.label, case.expected
        );
    }
}

/// The same rule on the other resource type `ResourceArn` names: a scheduled report
/// created with a tag list stores each key once.
#[tokio::test]
async fn a_created_scheduled_report_stores_each_tag_key_once() {
    for (index, case) in create_cases().into_iter().enumerate() {
        let router = router();
        let dashboard = create_dashboard_with_tags(&router, "Report Host", &[]).await;
        // `ScheduledReportName` is 2..50 characters matching
        // `^(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]$` (source:
        // `smithy_query(query="shape", shape="ScheduledReportInput")`).
        let name = format!("Created Report Tags {index}");
        let arn = create_report_with_tags(&router, &name, &dashboard, &case.supplied).await;

        let stored = stored_tags(&router, &arn, &case.label).await;
        assert_eq!(
            stored, case.expected,
            "case {}: expected the resolved tags {:?}, got {stored:?}",
            case.label, case.expected
        );
    }
}

/// Create-time tagging answers exactly what applying the same entries one request at a
/// time through `TagResource` answers: one rule, whichever path supplies the tags.
#[tokio::test]
async fn tagging_at_create_time_matches_tagging_afterwards() {
    for (index, case) in create_cases().into_iter().enumerate() {
        let router = router();
        let at_create = create_dashboard_with_tags(
            &router,
            &format!("At Create {index}"),
            &case.supplied,
        )
        .await;
        let afterwards = create_dashboard_with_tags(&router, &format!("Afterwards {index}"), &[])
            .await;
        tag_one_at_a_time(&router, &afterwards, &case.supplied).await;

        let created = stored_tags(&router, &at_create, &case.label).await;
        let tagged = stored_tags(&router, &afterwards, &case.label).await;
        assert_eq!(
            created, tagged,
            "case {}: create-time tagging must answer what TagResource answers {tagged:?}, got {created:?}",
            case.label
        );
    }
}

/// What a duplicated key would break: a later `TagResource` for that key must leave the
/// resource holding ONE entry for it, with the new value, and a read must report that
/// value — not a superseded one from a second copy.
#[tokio::test]
async fn updating_a_key_supplied_twice_at_create_time_replaces_it_wholly() {
    // The rows differ in how many times the key arrived at create time and in what else
    // the resource carries, so the update is exercised at the front, the middle and the
    // end of the stored list.
    let cases = vec![
        Case {
            label: "the only key, supplied twice".to_string(),
            supplied: pairs(&[("env", "a"), ("env", "b")]),
            expected: pairs(&[("env", "c")]),
        },
        Case {
            label: "a key supplied twice before another key".to_string(),
            supplied: pairs(&[("env", "a"), ("env", "b"), ("owner", "fin")]),
            expected: pairs(&[("env", "c"), ("owner", "fin")]),
        },
        Case {
            label: "a key supplied three times after another key".to_string(),
            supplied: pairs(&[("owner", "fin"), ("env", "a"), ("env", "b"), ("env", "x")]),
            expected: pairs(&[("owner", "fin"), ("env", "c")]),
        },
    ];

    for (index, case) in cases.into_iter().enumerate() {
        let Case {
            label,
            supplied,
            expected,
        } = case;
        let router = router();
        let arn =
            create_dashboard_with_tags(&router, &format!("Updated Dup {index}"), &supplied).await;

        ok_call(
            &router,
            "TagResource",
            &json!({
                "resourceArn": arn,
                "resourceTags": [{"key": "env", "value": "c"}],
            }),
        )
        .await;

        let stored = stored_tags(&router, &arn, &label).await;
        assert_eq!(
            stored, expected,
            "case {label}: expected the updated key to be stored once as {expected:?}, got {stored:?}"
        );
    }
}

/// The rule as a property, over generated tag lists that MAY repeat a key: whatever the
/// list, a created resource carries each key once, holding the value the list named last
/// for it, in first-appearance order — and the answer is the same as applying the same
/// entries one at a time through `TagResource`.
///
/// The run is bounded (32 cases), seeded from the constant below (so a failure is
/// reproducible and every assertion names the seed and the case) and persists nothing to
/// disk. The generator is the `untag_resource` suite's, which explains there why it is
/// arithmetic rather than a new `[dev-dependencies]` entry; its three-letter key
/// alphabet is what makes repeats common.
#[tokio::test]
async fn creating_with_generated_tag_lists_stores_each_key_once() {
    const SEED: u64 = 0x0dd1_7a65;
    const CASES: usize = 32;

    let mut generator = Xorshift::new(SEED);
    // A degenerate generator must not make this property vacuous.
    let mut saw_a_repeat = 0_usize;
    for case in 0..CASES {
        let supplied: Vec<(String, String)> = (0..generator.below(8))
            .map(|_| (generator.key(), generator.value()))
            .collect();

        // The expected answer, computed from the list alone: first-appearance order,
        // last value per key.
        let mut expected: Vec<(String, String)> = Vec::new();
        for (key, value) in &supplied {
            match expected.iter_mut().find(|(stored, _)| stored == key) {
                Some(entry) => {
                    entry.1 = value.clone();
                    saw_a_repeat += 1;
                }
                None => expected.push((key.clone(), value.clone())),
            }
        }

        let router = router();
        let label = format!("seed {SEED:#x} #{case} supplied {supplied:?}");
        let at_create =
            create_dashboard_with_tags(&router, &format!("Generated Create {case}"), &supplied)
                .await;
        let stored = stored_tags(&router, &at_create, &label).await;
        assert_eq!(
            stored, expected,
            "case {label}: expected {expected:?}, got {stored:?}"
        );

        let afterwards =
            create_dashboard_with_tags(&router, &format!("Generated Tagged {case}"), &[]).await;
        tag_one_at_a_time(&router, &afterwards, &supplied).await;
        let tagged = stored_tags(&router, &afterwards, &label).await;
        assert_eq!(
            stored, tagged,
            "case {label}: create-time tagging must answer what TagResource answers {tagged:?}, got {stored:?}"
        );
    }

    assert!(
        saw_a_repeat > 0,
        "case seed {SEED:#x}: the generated lists must repeat a key at least once, saw {saw_a_repeat}"
    );
}
