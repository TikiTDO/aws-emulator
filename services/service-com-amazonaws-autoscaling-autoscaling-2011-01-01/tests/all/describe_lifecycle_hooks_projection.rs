// SPDX-License-Identifier: CC0-1.0

//! `DescribeLifecycleHooks` as a REPORT over one Auto Scaling group's stored hook
//! collection, over the awsQuery wire.
//!
//! The rule under test: a group-scoped read of a named child collection answers exactly
//! the children the group it names holds, narrowed to the names the request lists (every
//! child when it lists none), each projected from what is stored — a name the group does
//! not hold contributes nothing instead of refusing the read or inventing a child, a
//! child of ANOTHER group is never reported even under the same name, and because the
//! call only reports, the collection a later caller reads is the one the writers left.
//!
//! The tables here span what that rule generalises over: how many hooks the group holds
//! (none through several), which names the read filters for (none, one, a subset, every
//! name, one name repeated, a name the group does not hold, a held name beside an unheld
//! one, and a filter at the modelled maximum length), how many times the collection is
//! read, and which of two groups holds a hook of a given name.
//!
//! `lifecycle_hooks.rs` covers what each WRITER of the collection stores and OWNS the
//! harness both suites drive — the group create, the upsert and the projection of an
//! answered body are its helpers, used here rather than copied.

use std::collections::BTreeSet;

use crate::lifecycle_hooks::{
    create_group, described_hook, described_hooks, expect, put_hook, ReportedHook,
    MAX_HEARTBEAT_TIMEOUT, MIN_HEARTBEAT_TIMEOUT, RESULTS, TRANSITIONS,
};
use crate::support::{new_engine, p, router_with};

/// The most names ONE read may filter for, from the model's own constraint on
/// `DescribeLifecycleHooksType$LifecycleHookNames` (`length 0..50`, read with
/// `smithy_query(query="shape",shape="LifecycleHookNames")`).
const MAX_FILTER_NAMES: usize = 50;

/// The names a group of `count` hooks holds, inside the modelled `^[A-Za-z0-9\-_\/]+$`
/// pattern of a `LifecycleHookNames` member.
fn hook_names(count: usize) -> Vec<String> {
    (1..=count).map(|index| format!("hook-{index:02}")).collect()
}

/// One filter row for a group holding `names`: the case, the names the read lists (none
/// for a read that carries no filter member at all), and the hooks it must then report.
///
/// Generated FROM the group's own names, so one table spans every hook count a group can
/// be holding when it is read — a group holding none included.
fn filter_rows(names: &[String]) -> Vec<(String, Vec<String>, Vec<String>)> {
    let absent = "hook-not-held".to_string();
    let mut rows: Vec<(String, Vec<String>, Vec<String>)> = vec![
        (
            "no names, so every hook of the group".to_string(),
            Vec::new(),
            names.to_vec(),
        ),
        (
            "every name the group holds".to_string(),
            names.to_vec(),
            names.to_vec(),
        ),
        (
            "one name the group does not hold".to_string(),
            vec![absent.clone()],
            Vec::new(),
        ),
    ];
    if let (Some(first), Some(last)) = (names.first(), names.last()) {
        rows.push((
            "one name".to_string(),
            vec![first.clone()],
            vec![first.clone()],
        ));
        rows.push((
            "the same name listed three times".to_string(),
            vec![first.clone(), first.clone(), first.clone()],
            vec![first.clone()],
        ));
        rows.push((
            "a name the group holds beside one it does not".to_string(),
            vec![last.clone(), absent],
            vec![last.clone()],
        ));
        let subset: Vec<String> = names.iter().step_by(2).cloned().collect();
        rows.push(("every other name".to_string(), subset.clone(), subset));
    }
    // The modelled ceiling on how many names ONE read may list: the group's own names,
    // padded with names it does not hold, up to that maximum.
    let mut at_maximum = names.to_vec();
    while at_maximum.len() < MAX_FILTER_NAMES {
        at_maximum.push(format!("hook-unheld-{:02}", at_maximum.len()));
    }
    rows.push((
        format!("a filter at the modelled maximum of {MAX_FILTER_NAMES} names"),
        at_maximum,
        names.to_vec(),
    ));
    rows
}

/// Every filter row, over groups holding none through several hooks: the read answers
/// exactly the hooks the group holds that its filter names, and every hook it answers
/// names the group that was read.
#[tokio::test]
async fn a_read_reports_exactly_the_hooks_of_the_group_its_filter_names() {
    for count in [0_usize, 1, 2, 7] {
        let router = router_with(new_engine());
        let group = format!("asg-holding-{count:02}");
        create_group(&router, &group).await;
        let names = hook_names(count);
        for (index, name) in names.iter().enumerate() {
            let (status, code, message) = put_hook(
                &router,
                &group,
                name,
                &[p(
                    "LifecycleTransition",
                    TRANSITIONS[index % TRANSITIONS.len()],
                )],
            )
            .await;
            assert_eq!(
                status, 200,
                "the hook {name:?} of {group:?} must be created, got {status} with {code:?} \
                 {message:?}"
            );
        }

        for (row, filter, expected) in filter_rows(&names) {
            let case = format!("{count} hooks stored, {row}");
            let filter: Vec<&str> = filter.iter().map(String::as_str).collect();
            let reported = described_hooks(&router, &group, &filter).await;
            // The model documents no order for `LifecycleHooks`, so WHICH hooks were
            // reported is asserted as a set rather than binding one.
            let actual: BTreeSet<&str> = reported.iter().map(|hook| hook.name.as_str()).collect();
            let wanted: BTreeSet<&str> = expected.iter().map(String::as_str).collect();
            assert_eq!(
                actual, wanted,
                "case {case:?}: expected the hooks {wanted:?}, got {reported:?}"
            );
            for hook in &reported {
                assert_eq!(
                    hook.group, group,
                    "case {case:?}: every reported hook must name the group read, got {hook:?}"
                );
            }
        }
    }
}

/// The read only REPORTS: however often the collection is read, and however the reads are
/// narrowed, a later caller sees the hooks its writers left — and a hook stored after them
/// is still reported, so that stability is the stored state rather than a frozen answer.
#[tokio::test]
async fn a_read_leaves_the_collection_it_reports_unchanged() {
    let router = router_with(new_engine());
    let group = "asg-reported";
    create_group(&router, group).await;
    let written = [
        ("hook-a", p("HeartbeatTimeout", "90")),
        ("hook-b", p("DefaultResult", RESULTS[0])),
    ];
    for (hook, member) in &written {
        let (status, code, message) = put_hook(
            &router,
            group,
            hook,
            &[p("LifecycleTransition", TRANSITIONS[0]), member.clone()],
        )
        .await;
        assert_eq!(
            status, 200,
            "the hook {hook:?} must be created, got {status} with {code:?} {message:?}"
        );
    }

    let first = described_hooks(&router, group, &[]).await;
    assert_eq!(
        first.len(),
        written.len(),
        "the group must report both stored hooks, got {first:?}"
    );

    for round in 1..=3 {
        // A narrowed read in between, so a filter cannot be what the collection keeps.
        let narrowed = described_hooks(&router, group, &["hook-a"]).await;
        assert_eq!(
            narrowed.len(),
            1,
            "round {round}: the narrowed read must report its one hook, got {narrowed:?}"
        );
        let again = described_hooks(&router, group, &[]).await;
        assert_eq!(
            again, first,
            "round {round}: expected the hooks {first:?}, got {again:?}"
        );
    }

    let (status, code, message) = put_hook(
        &router,
        group,
        "hook-c",
        &[p("LifecycleTransition", TRANSITIONS[1])],
    )
    .await;
    assert_eq!(
        status, 200,
        "the third hook must be created, got {status} with {code:?} {message:?}"
    );
    let actual: Vec<String> = described_hooks(&router, group, &[])
        .await
        .into_iter()
        .map(|hook| hook.name)
        .collect();
    assert_eq!(
        actual,
        vec!["hook-a", "hook-b", "hook-c"],
        "expected the three stored hooks after the third was written, got {actual:?}"
    );
}

/// A hook is a child of ONE group: two groups each holding a hook under the SAME name
/// report their OWN hook, with their own members, and a group holding none under that
/// name reports none rather than another group's.
#[tokio::test]
async fn a_read_reports_the_group_s_own_hook_under_a_shared_name() {
    let router = router_with(new_engine());
    let hook = "shared-hook";
    // One row per group: its own transition, heartbeat timeout, default result and
    // metadata, all stored under the one shared hook name.
    let rows = [
        (
            "asg-left",
            TRANSITIONS[0],
            MIN_HEARTBEAT_TIMEOUT,
            RESULTS[0],
            "left side",
        ),
        (
            "asg-right",
            TRANSITIONS[1],
            MAX_HEARTBEAT_TIMEOUT,
            RESULTS[1],
            "right side",
        ),
    ];
    for (group, transition, heartbeat_timeout, default_result, metadata) in rows {
        create_group(&router, group).await;
        let (status, code, message) = put_hook(
            &router,
            group,
            hook,
            &[
                p("LifecycleTransition", transition),
                p("HeartbeatTimeout", heartbeat_timeout.to_string()),
                p("DefaultResult", default_result),
                p("NotificationMetadata", metadata),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {group:?}: the hook must be created, got {status} with {code:?} {message:?}"
        );
    }

    for (group, transition, heartbeat_timeout, default_result, metadata) in rows {
        let actual = described_hook(&router, group, hook).await;
        let expected = ReportedHook {
            notification_metadata: metadata.to_string(),
            ..expect(group, hook, transition, heartbeat_timeout, default_result)
        };
        assert_eq!(
            actual, expected,
            "case {group:?}: expected the hook {expected:?}, got {actual:?}"
        );
    }

    create_group(&router, "asg-neither").await;
    let reported = described_hooks(&router, "asg-neither", &[hook]).await;
    assert!(
        reported.is_empty(),
        "case \"a group holding no hook under the shared name\": expected no hook, got \
         {reported:?}"
    );
}
