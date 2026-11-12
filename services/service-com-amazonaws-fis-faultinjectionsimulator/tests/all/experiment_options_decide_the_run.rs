// SPDX-License-Identifier: CC0-1.0

//! What an experiment's OPTIONS do to the run they were started with, end to end over
//! HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! RULE: an experiment's options select which of the template's actions the run CARRIES
//! OUT, and only an action the run carries out affects the run — `actionsMode: skip-all`
//! carries out none of them, and `emptyTargetResolutionMode` decides what a target that
//! resolved to zero resources does: `skip` skips the actions that name it, `fail` (the
//! documented default) ends the run `failed` on the resolution. So an action the options
//! leave out spends none of its `duration` and earns no failure attributed to it, and it
//! reports the modelled `skipped` action status instead of none.
//!
//! The rows differ along what that rule generalises over: the `actionsMode` the start
//! chooses (absent, `run-all`, `skip-all`), the template's `emptyTargetResolutionMode`
//! (absent, `fail`, `skip`), whether a target resolves to resources or to nothing, whether
//! an action names such a target, whether any action names it at all, how many actions the
//! template carries and whether they are chained through `startAfter`, and the modelled
//! duration they add up to.
//!
//! Two things are read off the RECORD rather than off the wall clock, which is what makes
//! them exact: `endTime - startTime` is the span the service itself planned for the run
//! (the transition the start owes plus the compressed run span), so a run that carried out
//! no action reports the compressed FLOOR while one that carried out an eight-minute
//! action reports the compressed eight minutes. Nothing here waits out a modelled duration.
//!
//! What a skipped action does NOT claim: `startTime`, `endTime` and any effect on another
//! service. `skipped` is the decision the options made about the action, which is known at
//! start — not a report of work this emulator performed.
//!
//! What an action the run CARRIES OUT claims is the other half of that: its own progress —
//! where the RUN is in that action's own span, which every action of the plan reports,
//! because the spans are what the run's own window is computed from. What stays the emulator's
//! own distinction is the EFFECT: it is performed only for the actions whose work this
//! emulator can really perform (the `aws:fis` actions it carries out itself, and the ones
//! whose effect it performs on the service that owns the targeted resource), and nothing at
//! all is announced for an action whose effect is on something this emulator does not hold.

use std::time::Duration;

use serde_json::{json, Value};

use crate::experiment_lifecycle::{
    create_template, emulator, get_experiment, poll_until_final, start_experiment, token,
};

/// The window the emulator compresses a modelled run into, and the span it owes a
/// transition it chooses the length of. Spelled out here rather than imported from the
/// crate, so a wrong value in `src/state.rs` fails this suite.
const MIN_RUN_SPAN: Duration = Duration::from_secs(5);
/// See [`MIN_RUN_SPAN`].
const MAX_RUN_SPAN: Duration = Duration::from_secs(15);
/// See [`MIN_RUN_SPAN`].
const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// The longest span a run can REPORT when it carried out no action that names a duration:
/// the compressed floor plus the transition the start owes. A run that carried out the
/// eight-minute action below reports more than this, and one whose actions were skipped
/// reports at most this.
const FLOOR_SPAN_SECONDS: f64 = MIN_RUN_SPAN.as_secs_f64() + TRANSITION_DELAY.as_secs_f64();

/// The tolerance the span comparisons allow. The stamps are reported as epoch seconds, so
/// subtracting two of them loses the last digits of a value around 1.7e9 — far below this,
/// and far below the seconds that separate a floor run from one that carried out
/// [`LONG_DURATION`].
const SPAN_SLACK: f64 = 0.25;

/// `ExperimentActionStatus`'s published values
/// (`smithy_query(query="shape",shape="ExperimentActionStatus")`), so an action's reported
/// status is compared against the model's own catalog and not against one spelling.
const ACTION_STATUS_VALUES: [&str; 9] = [
    "pending",
    "initiating",
    "running",
    "completed",
    "cancelled",
    "stopping",
    "stopped",
    "failed",
    "skipped",
];

/// How long a row may take to reach its terminal status: the whole compressed window, the
/// transitions around it and slack for a loaded machine.
const TERMINAL_BUDGET: Duration = Duration::from_secs(MAX_RUN_SPAN.as_secs() + 12);

/// The modelled duration of the long action every "the durations were spent" row carries.
/// Compressed (one modelled minute per second of real time) it is well past
/// [`FLOOR_SPAN_SECONDS`], and well inside [`MAX_RUN_SPAN`].
const LONG_DURATION: &str = "PT8M";

/// A role ARN the emulator's IAM store holds no role for, so nothing is shown about its
/// trust policy or its permissions and the run goes ahead. That a SKIPPED action earns no
/// privilege failure either is asserted against the real IAM service in
/// `core/router/tests/all/fis_execution_role_permissions.rs`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-experiment-options";

/// The account id every ARN this emulator mints carries, which a failed experiment's
/// `ExperimentError$accountId` names.
const ACCOUNT_ID: &str = "000000000000";

/// How many experiments may be active at once (the published FIS quota), which is how many
/// rows a group starts before waiting for them.
const ACTIVE_EXPERIMENTS_QUOTA: usize = 5;

/// A target identified by ARN: this emulator resolves it to the resource it names.
///
/// `aws:iam:role` is the resource type the api-fault family of actions takes under its
/// `Roles` target key, so this is a target those actions may name.
fn named_target() -> Value {
    json!({
        "resourceType": "aws:iam:role",
        "resourceArns": ["arn:aws:iam::000000000000:role/fis-targeted"],
        "selectionMode": "ALL"
    })
}

/// A target identified by TAG: no resource of this emulator carries that tag — the suite
/// mounts FIS alone, so the stores a tag-identified target is resolved against hold
/// nothing — so it resolves to nothing, which is the empty resolution
/// `emptyTargetResolutionMode` decides about.
fn tagged_target() -> Value {
    json!({
        "resourceType": "aws:iam:role",
        "resourceTags": { "Stage": "test" },
        "selectionMode": "ALL"
    })
}

/// One action of the api-fault family, with the parameters FIS documents as required for
/// it, acting on the target `target` and lasting `duration`.
fn fault_action(duration: &str, target: Option<&str>) -> Value {
    let mut action = json!({
        "actionId": "aws:fis:inject-api-internal-error",
        "parameters": {
            "duration": duration,
            "service": "ec2",
            "operations": "DescribeAvailabilityZones",
            "percentage": "100"
        }
    });
    if let Some(target) = target {
        action["targets"] = json!({ "Roles": target });
    }
    action
}

/// What a row expects of the run its options planned.
struct Expected {
    /// The actions that must report the modelled `skipped` status, each beside the text
    /// its reason has to name — the option that skipped it, or the target that resolved to
    /// nothing.
    skipped: &'static [(&'static str, &'static str)],
    /// The actions the run carries out whose EFFECT this emulator does not perform. They
    /// report their own progress all the same: the status says where the RUN is in the
    /// action's own span, not that its effect was produced.
    carried_out: &'static [&'static str],
    /// The actions whose work this emulator also carries out, so the progress they report
    /// is backed by it: they reach a terminal `completed` by the time the run has, with the
    /// `startTime` and `endTime` of the window they were carried out in.
    reported: &'static [&'static str],
    /// The terminal status the run must reach.
    terminal: &'static str,
    /// For a run that ends `failed` on the resolution: the target the failure must locate
    /// and name.
    empty_target: Option<&'static str>,
    /// Whether the run must report the FLOOR span — true when no action it carried out
    /// names a duration, false when it carried out the long one.
    floor_span: bool,
}

/// One row: a template, the options the experiment runs under, and what they must make of
/// the run.
struct Row {
    case: &'static str,
    /// The template's `experimentOptions.emptyTargetResolutionMode`, absent for a row that
    /// relies on the documented default.
    empty_mode: Option<&'static str>,
    /// The `experimentOptions.actionsMode` the start names, absent for a row that relies
    /// on the documented default.
    actions_mode: Option<&'static str>,
    targets: Value,
    actions: Value,
    expected: Expected,
}

/// The rows the rule is asserted over. See the suite docs for what they span.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an-absent-actions-mode-carries-out-every-action",
            empty_mode: None,
            actions_mode: None,
            targets: json!({ "named": named_target() }),
            actions: json!({
                "fault": fault_action(LONG_DURATION, Some("named")),
                "after": {
                    "actionId": "aws:fis:wait",
                    "parameters": { "duration": "PT1M" },
                    "startAfter": ["fault"]
                }
            }),
            expected: Expected {
                skipped: &[],
                carried_out: &["fault"],
                reported: &["after"],
                terminal: "completed",
                empty_target: None,
                floor_span: false,
            },
        },
        Row {
            case: "run-all-named-explicitly-carries-out-every-action",
            empty_mode: Some("fail"),
            actions_mode: Some("run-all"),
            targets: json!({ "named": named_target() }),
            actions: json!({
                "fault": fault_action(LONG_DURATION, Some("named")),
                "after": {
                    "actionId": "aws:fis:wait",
                    "parameters": { "duration": "PT1M" },
                    "startAfter": ["fault"]
                }
            }),
            expected: Expected {
                skipped: &[],
                carried_out: &["fault"],
                reported: &["after"],
                terminal: "completed",
                empty_target: None,
                floor_span: false,
            },
        },
        Row {
            // The documented use of `skip-all`: the experiment is started and resolves its
            // targets, and none of the actions is carried out — so the run spends none of
            // their durations and still completes.
            case: "skip-all-carries-out-no-action-and-spends-no-duration",
            empty_mode: None,
            actions_mode: Some("skip-all"),
            targets: json!({ "named": named_target() }),
            actions: json!({
                "fault": fault_action(LONG_DURATION, Some("named")),
                "after": {
                    "actionId": "aws:fis:wait",
                    "parameters": { "duration": "PT1M" },
                    "startAfter": ["fault"]
                }
            }),
            expected: Expected {
                skipped: &[("fault", "skip-all"), ("after", "skip-all")],
                carried_out: &[],
                reported: &[],
                terminal: "completed",
                empty_target: None,
                floor_span: true,
            },
        },
        Row {
            // `skip` with nothing empty to skip: every action is carried out, exactly as
            // under `fail`.
            case: "skip-skips-nothing-when-every-target-resolves",
            empty_mode: Some("skip"),
            actions_mode: None,
            targets: json!({ "named": named_target() }),
            actions: json!({ "fault": fault_action(LONG_DURATION, Some("named")) }),
            expected: Expected {
                skipped: &[],
                carried_out: &["fault"],
                reported: &[],
                terminal: "completed",
                empty_target: None,
                floor_span: false,
            },
        },
        Row {
            case: "an-absent-empty-mode-fails-the-run-on-an-empty-resolution",
            empty_mode: None,
            actions_mode: None,
            targets: json!({ "tagged": tagged_target(), "named": named_target() }),
            actions: json!({
                "onTagged": fault_action(LONG_DURATION, Some("tagged")),
                "onNamed": fault_action("PT1M", Some("named"))
            }),
            expected: Expected {
                skipped: &[],
                carried_out: &["onTagged", "onNamed"],
                reported: &[],
                terminal: "failed",
                empty_target: Some("tagged"),
                floor_span: true,
            },
        },
        Row {
            // The resolution is decided before any action runs, so a `skip-all` start —
            // the one a caller makes to CHECK the resolution — sees it fail too.
            case: "skip-all-still-fails-on-an-empty-resolution-under-fail",
            empty_mode: Some("fail"),
            actions_mode: Some("skip-all"),
            targets: json!({ "tagged": tagged_target(), "named": named_target() }),
            actions: json!({
                "onTagged": fault_action(LONG_DURATION, Some("tagged")),
                "onNamed": fault_action("PT1M", Some("named"))
            }),
            expected: Expected {
                skipped: &[("onTagged", "skip-all"), ("onNamed", "skip-all")],
                carried_out: &[],
                reported: &[],
                terminal: "failed",
                empty_target: Some("tagged"),
                floor_span: true,
            },
        },
        Row {
            // The same template as the row above, under the other mode: the action that
            // names the empty target is skipped and the rest of the experiment runs.
            case: "skip-skips-only-the-action-that-names-the-empty-target",
            empty_mode: Some("skip"),
            actions_mode: None,
            targets: json!({ "tagged": tagged_target(), "named": named_target() }),
            actions: json!({
                "onTagged": fault_action(LONG_DURATION, Some("tagged")),
                "onNamed": fault_action("PT1M", Some("named"))
            }),
            expected: Expected {
                skipped: &[("onTagged", "tagged")],
                carried_out: &["onNamed"],
                reported: &[],
                terminal: "completed",
                // The long action was skipped, so the run is the floor: the only action it
                // carried out models a minute, which compresses below it.
                empty_target: None,
                floor_span: true,
            },
        },
        Row {
            // A target no action names, which still resolved to nothing: `fail` is stated
            // of the TARGET's resolution, so the run fails on it.
            case: "fail-fails-on-an-empty-target-no-action-names",
            empty_mode: Some("fail"),
            actions_mode: None,
            targets: json!({ "tagged": tagged_target() }),
            actions: json!({ "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT1M" } } }),
            expected: Expected {
                skipped: &[],
                carried_out: &["wait"],
                reported: &[],
                terminal: "failed",
                empty_target: Some("tagged"),
                floor_span: true,
            },
        },
        Row {
            // The same template under `skip`: there is no action to skip for that target,
            // so the experiment runs its actions and completes — which is what makes the
            // two modes behave differently on one template.
            case: "skip-has-no-action-to-skip-for-an-unused-empty-target",
            empty_mode: Some("skip"),
            actions_mode: None,
            targets: json!({ "tagged": tagged_target() }),
            actions: json!({ "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT1M" } } }),
            expected: Expected {
                skipped: &[],
                carried_out: &[],
                reported: &["wait"],
                terminal: "completed",
                empty_target: None,
                floor_span: true,
            },
        },
    ]
}

/// The create body for `row`.
fn template_body(row: &Row) -> Value {
    let mut body = json!({
        "clientToken": token(&format!("options-template-{}", row.case)),
        "description": format!("experiment options decide the run: {}", row.case),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": row.targets.clone(),
        "actions": row.actions.clone()
    });
    if let Some(mode) = row.empty_mode {
        body["experimentOptions"] = json!({ "emptyTargetResolutionMode": mode });
    }
    body
}

/// The start body for `row`'s template.
fn start_body(row: &Row, template_id: &str) -> Value {
    let mut body = json!({
        "clientToken": token(&format!("options-start-{}", row.case)),
        "experimentTemplateId": template_id
    });
    if let Some(mode) = row.actions_mode {
        body["experimentOptions"] = json!({ "actionsMode": mode });
    }
    body
}

/// The `startTime`/`endTime` an experiment reports, as epoch seconds.
fn stamp(case: &str, experiment: &Value, member: &str) -> f64 {
    experiment[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} on {experiment:?}"))
}

/// Assert that `experiment` reports exactly the action states `row` expects.
fn assert_action_states(case: &str, where_from: &str, row: &Row, experiment: &Value) {
    for (name, says) in row.expected.skipped {
        let state = &experiment["actions"][name]["state"];
        assert_eq!(
            state["status"].as_str().unwrap_or_default(),
            "skipped",
            "case {case:?} ({where_from}): expected action {name:?} to report the skipped \
             status, got {state:?}"
        );
        let reason = state["reason"].as_str().unwrap_or_default();
        assert!(
            reason.contains(says),
            "case {case:?} ({where_from}): expected the reason of the skipped action \
             {name:?} to name {says:?}, got {reason:?}"
        );
        // A skipped action never ran, so it claims no time.
        for member in ["startTime", "endTime"] {
            assert!(
                experiment["actions"][name][member].is_null(),
                "case {case:?} ({where_from}): a skipped action claims no {member}, got {:?}",
                experiment["actions"][name]
            );
        }
    }
    // A run the resolution FAILED never got going, so nothing was carried out for any of
    // its actions and none of them claims a progress; every other run reports where it has
    // got in each action's own span — `pending` until the run reaches it, and `completed`
    // once the run has ended.
    let never_ran = row.expected.terminal == "failed";
    let final_read = where_from == "after the run";
    for name in row
        .expected
        .carried_out
        .iter()
        .chain(row.expected.reported)
        .copied()
    {
        let state = &experiment["actions"][name]["state"];
        if never_ran {
            assert!(
                state.is_null(),
                "case {case:?} ({where_from}): an action of a run that never got going \
                 claims no state, got {state:?}"
            );
            continue;
        }
        let status = state["status"].as_str().unwrap_or_default();
        // Never the `skipped` status the options stamp on an action nothing carries out.
        assert!(
            ACTION_STATUS_VALUES.contains(&status) && status != "skipped",
            "case {case:?} ({where_from}): expected action {name:?} to report a modelled \
             status other than skipped, got {state:?}"
        );
        let allowed: &[&str] = if final_read {
            &["completed"]
        } else {
            &["pending", "running", "completed"]
        };
        assert!(
            allowed.contains(&status),
            "case {case:?} ({where_from}): expected action {name:?} to report one of \
             {allowed:?}, got {state:?}"
        );
        assert!(
            state["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "case {case:?} ({where_from}): an action that reports a state says why, got \
             {state:?}"
        );
        if final_read {
            for member in ["startTime", "endTime"] {
                assert!(
                    !experiment["actions"][name][member].is_null(),
                    "case {case:?} ({where_from}): an action the run carried out reports its \
                     {member}, got {:?}",
                    experiment["actions"][name]
                );
            }
        }
    }
}

#[tokio::test]
async fn the_experiment_options_decide_which_actions_the_run_carries_out() {
    let router = emulator();
    let rows = rows();

    // The rows are started in groups no larger than the account's active-experiment quota,
    // so the runs inside a group overlap instead of being waited out one after another.
    for group in rows.chunks(ACTIVE_EXPERIMENTS_QUOTA) {
        let mut started_rows: Vec<(&Row, String)> = Vec::new();
        for row in group {
            let case = row.case;
            let created = create_template(&router, template_body(row)).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: expected the template to be created, got {} with {:?}",
                created.status, created.body
            );
            let template_id = created.member("experimentTemplate")["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no template id in {:?}", created.body))
                .to_string();

            let started = start_experiment(&router, start_body(row, &template_id)).await;
            assert_eq!(
                started.status, 200,
                "case {case:?}: expected the start to be accepted, got {} with {:?}",
                started.status, started.body
            );
            let experiment = started.member("experiment").clone();
            // The options the experiment reports are the ones it runs under: the mode the
            // start chose (or the documented default) and the template's own.
            assert_eq!(
                experiment["experimentOptions"]["actionsMode"]
                    .as_str()
                    .unwrap_or_default(),
                row.actions_mode.unwrap_or("run-all"),
                "case {case:?}: expected the actionsMode the start chose, got {:?}",
                experiment["experimentOptions"]
            );
            assert_eq!(
                experiment["experimentOptions"]["emptyTargetResolutionMode"]
                    .as_str()
                    .unwrap_or_default(),
                row.empty_mode.unwrap_or("fail"),
                "case {case:?}: expected the template's emptyTargetResolutionMode, got {:?}",
                experiment["experimentOptions"]
            );
            // Which actions the run carries out is decided AT THE START, so the start's own
            // answer already says so.
            assert_action_states(case, "StartExperiment", row, &experiment);

            let id = experiment["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no experiment id in {experiment:?}"))
                .to_string();
            started_rows.push((row, id));
        }

        for (row, id) in &started_rows {
            let case = row.case;
            // The same decision is PERSISTED: a later read reports it, not a re-derived one.
            let read = get_experiment(&router, id).await;
            assert_action_states(case, "GetExperiment", row, read.member("experiment"));

            let experiment = poll_until_final(&router, id, TERMINAL_BUDGET).await;
            assert_eq!(
                experiment["state"]["status"].as_str().unwrap_or_default(),
                row.expected.terminal,
                "case {case:?}: expected the run its options planned to end \
                 {:?}, got {experiment:?}",
                row.expected.terminal
            );
            assert!(
                experiment["state"]["reason"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty()),
                "case {case:?}: an experiment that ended says why, got {experiment:?}"
            );
            // The decision survives the whole run.
            assert_action_states(case, "after the run", row, &experiment);

            match row.expected.empty_target {
                // A run that ended on its resolution says so in BOTH halves of the
                // attribution: the reason names the target, and the error locates it.
                Some(target) => {
                    let reason = experiment["state"]["reason"].as_str().unwrap_or_default();
                    assert!(
                        reason.contains(target),
                        "case {case:?}: expected the reason to name the empty target \
                         {target:?}, got {reason:?}"
                    );
                    let error = &experiment["state"]["error"];
                    assert_eq!(
                        error["location"].as_str().unwrap_or_default(),
                        format!("targets.{target}"),
                        "case {case:?}: expected the error to locate the target whose \
                         resolution failed, got {error:?}"
                    );
                    let code = error["code"].as_str().unwrap_or_default();
                    assert!(
                        !code.is_empty() && !code.chars().any(char::is_whitespace),
                        "case {case:?}: expected an ExperimentError code within the \
                         modelled ^[\\S]+$, got {code:?}"
                    );
                    assert_ne!(
                        code, "insufficientPrivileges",
                        "case {case:?}: an empty resolution is not a privilege failure, \
                         got {error:?}"
                    );
                    assert_eq!(
                        error["accountId"].as_str().unwrap_or_default(),
                        ACCOUNT_ID,
                        "case {case:?}: expected the error to name the account the run \
                         failed in, got {error:?}"
                    );
                }
                // `ExperimentError` is the error information "when the action has failed",
                // so a run that completed reports none.
                None => assert!(
                    experiment["state"]["error"].is_null(),
                    "case {case:?}: a completed experiment reports no error, got {experiment:?}"
                ),
            }

            // The span the service PLANNED for the run, read off the record: an action the
            // run did not carry out spent none of its duration, so such a run reports the
            // compressed floor, while a run that carried out the eight-minute action
            // reports more than it.
            let span = stamp(case, &experiment, "endTime") - stamp(case, &experiment, "startTime");
            let ceiling = FLOOR_SPAN_SECONDS + SPAN_SLACK;
            if row.expected.floor_span {
                assert!(
                    span <= ceiling,
                    "case {case:?}: expected a run that carried out no duration to report \
                     at most {ceiling}s, got {span}s in {experiment:?}"
                );
            } else {
                assert!(
                    span > ceiling,
                    "case {case:?}: expected a run that carried out {LONG_DURATION} to \
                     report more than {ceiling}s, got {span}s in {experiment:?}"
                );
            }
        }
    }
}
