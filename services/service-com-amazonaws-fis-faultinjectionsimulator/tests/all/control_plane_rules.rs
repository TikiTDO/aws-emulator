// SPDX-License-Identifier: CC0-1.0

//! The FIS control-plane rules that go beyond the SHAPE of a request, each asserted over
//! a table whose rows differ along what that rule generalises over.
//!
//! * the built-in `aws:fis:wait` action takes a required `duration`, an ISO 8601 duration
//!   within the documented 1 to 720 minutes, and a template outside that bound is refused
//!   when it is CREATED rather than persisted;
//! * a `clientToken` identifies a REQUEST: replaying it with the same request reports the
//!   resource it created, and reusing it for a different request is a conflict;
//! * the account's active-experiment quota refuses the start that would exceed it, and
//!   counts only the experiments that are still active;
//! * an experiment runs for as long as its wait actions last and then reaches the terminal
//!   status its actions earn, with an `endTime`; a call that would move one already in a
//!   final state is refused and changes nothing;
//! * a value longer than its member's `@length` maximum is refused with a message that
//!   names the MEMBER and does not echo the value, and an id naming no record is reported
//!   by the rule for its resource;
//! * `UpdateTargetAccountConfiguration` changes a configuration that exists and refuses
//!   when none does — it never creates one.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_fis_faultinjectionsimulator::ops::types::{
    ExperimentTemplateAction, ExperimentTemplateExperimentOptions,
};
use service_com_amazonaws_fis_faultinjectionsimulator::state::{
    run_plan, ExperimentFailure, ExperimentOutcome, ExperimentRecord, ExperimentTemplateRecord,
    TargetableResources,
};
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, emulator, get_experiment, poll_until_final, send, start_experiment,
    stop_experiment, token, Answer, FALLBACK_REGION, WAIT_DURATION,
};

/// The account's 'Active experiments' quota: the FIS user guide's quotas page lists it as
/// 5 per account per Region, non-adjustable. Spelled out here rather than imported from
/// the crate, so a wrong bound in `src/state.rs` fails this suite.
const ACTIVE_EXPERIMENTS_QUOTA: usize = 5;

/// `ExperimentId` / `ExperimentTemplateId`'s published ceiling (`@length(min: 0, max: 64)`).
const ID_CEILING: usize = 64;

/// `ExperimentTemplateDescription`'s published ceiling (`@length(min: 0, max: 512)`).
const DESCRIPTION_CEILING: usize = 512;

/// `ClientToken`'s published ceiling (`@length(min: 1, max: 1024)`).
const CLIENT_TOKEN_CEILING: usize = 1024;

/// `RoleArn`'s published ceiling (`@length(min: 20, max: 2048)`).
const ROLE_ARN_CEILING: usize = 2048;

/// `TagMap`'s published ceiling (`@length(min: 0, max: 50)` entries).
const TAG_MAP_CEILING: usize = 50;

/// How long a case waits for a transition this service owes the clock. Every span the
/// emulator chooses is a fraction of a second, so this is generous.
const SETTLE_BUDGET: Duration = Duration::from_secs(5);

/// The account id every ARN this emulator mints carries (`::engine::EMULATOR_ACCOUNT_ID`),
/// spelled out so a changed account fails here.
const ACCOUNT_ID: &str = "000000000000";

/// A role ARN the emulator's IAM store holds no role for, which is the case every row here
/// runs under: with no trust policy to read, `StartExperiment` has no evidence the role
/// withholds the grant and the experiment starts. The trust rule itself is asserted
/// against the real IAM service in `core/router/tests/all/fis_execution_role_trust.rs`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/fis-rules";

/// A `clientToken` for `case`: `ClientToken` is modelled `^[\S]+$`, so the spaces a case
/// label carries are not admitted in one.
fn case_token(case: &str) -> String {
    token(&case.replace(' ', "-"))
}

/// A create body for a template whose only action is a wait of `duration`.
fn wait_template(label: &str, duration: &str) -> Value {
    json!({
        "clientToken": case_token(label),
        "description": format!("control-plane rules: {label}"),
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {},
        "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": duration } } }
    })
}

/// The `id` of a template a case created, which every row here expects to succeed.
fn created_template_id(case: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateExperimentTemplate, got {} with {:?}",
        answer.status, answer.body
    );
    answer.member("experimentTemplate")["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `message` an error body carries.
fn message(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// How many templates the store holds.
async fn template_count(router: &MultiServiceRouter) -> usize {
    let listed = send(
        router,
        "GET",
        "/experimentTemplates?maxResults=100",
        None,
        FALLBACK_REGION,
    )
    .await;
    listed
        .member("experimentTemplates")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// How many experiments the store holds.
async fn experiment_count(router: &MultiServiceRouter) -> usize {
    let listed = send(
        router,
        "GET",
        "/experiments?maxResults=100",
        None,
        FALLBACK_REGION,
    )
    .await;
    listed
        .member("experiments")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// RULE: the built-in `aws:fis:wait` action takes a required `duration` parameter, an
/// ISO 8601 duration within 1 to 720 minutes (the AWS FIS action reference:
/// "aws:fis:wait — duration: The duration of the wait. Required: Yes", with a documented
/// range of 1 to 720 minutes), and `CreateExperimentTemplate` refuses anything else rather
/// than persisting a template the service would never run.
#[tokio::test]
async fn the_wait_actions_duration_is_bounded_when_the_template_is_created() {
    let router = emulator();

    // The rows differ along the form and the magnitude of the duration: each published
    // bound, one past each of them, the same bound spelled in other units, a combined
    // span, a span whose length depends on the calendar, a value that is not a duration at
    // all, a magnitude no `Duration` can hold, and the parameter left out entirely.
    let rows: [(&str, Option<&str>, bool); 17] = [
        ("at the floor", Some("PT1M"), true),
        ("the floor in seconds", Some("PT60S"), true),
        ("a combined span", Some("PT1M30S"), true),
        ("mid-range", Some("PT6H"), true),
        ("at the ceiling", Some("PT720M"), true),
        ("the ceiling in hours", Some("PT12H"), true),
        ("one second below the floor", Some("PT59S"), false),
        ("no wait at all", Some("PT0S"), false),
        ("one minute past the ceiling", Some("PT721M"), false),
        ("an hour past the ceiling", Some("PT13H"), false),
        ("a whole day", Some("P1D"), false),
        ("a calendar span", Some("P1Y"), false),
        ("not a duration", Some("13h"), false),
        // A magnitude past what a `Duration` can hold: the parse reports no span rather
        // than constructing one, so the request is REFUSED and the handler answers.
        (
            "more seconds than a Duration holds",
            Some("PT99999999999999999999S"),
            false,
        ),
        (
            "more days than a Duration holds",
            Some("P99999999999999999999D"),
            false,
        ),
        (
            "every field at that magnitude",
            Some("P99999999999999999999DT99999999999999999999H99999999999999999999M99999999999999999999S"),
            false,
        ),
        ("the parameter left out", None, false),
    ];

    let mut accepted = 0usize;
    for (case, duration, admitted) in rows {
        let body = match duration {
            Some(duration) => wait_template(case, duration),
            None => json!({
                "clientToken": case_token(case),
                "description": format!("control-plane rules: {case}"),
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": { "wait": { "actionId": "aws:fis:wait" } }
            }),
        };
        let answer = create_template(&router, body).await;

        if admitted {
            accepted += 1;
            let id = created_template_id(case, &answer);
            // The accepted duration is the one the caller asked for, read back from the
            // store rather than echoed by the create.
            let fetched = send(
                &router,
                "GET",
                &format!("/experimentTemplates/{id}"),
                None,
                FALLBACK_REGION,
            )
            .await;
            assert_eq!(
                fetched.member("experimentTemplate")["actions"]["wait"]["parameters"]["duration"]
                    .as_str(),
                duration,
                "case {case:?}: expected the stored duration to be the one created, got {:?}",
                fetched.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
            let stated = message(&answer);
            assert!(
                !stated.is_empty(),
                "case {case:?}: a refusal says what was wrong, got {:?}",
                answer.body
            );
        }
        assert_eq!(
            template_count(&router).await,
            accepted,
            "case {case:?}: a refused create must persist no template"
        );
    }
}

/// RULE: the documented 1-to-720-minute range belongs to the `aws:fis:wait` action's
/// `duration`, not to every parameter called `duration`: FIS publishes a range for the
/// wait's own duration and none for another action's, so another action's is stored as
/// sent.
#[tokio::test]
async fn another_actions_parameters_are_stored_as_sent() {
    let router = emulator();

    // The rows differ along the action id and along what its published contract says about
    // a `duration`: one whose contract records no such parameter, one that takes no
    // parameter at all, and one that REQUIRES a duration — each carrying a span well
    // outside the wait action's bound.
    let rows = [
        ("stop-instances", "aws:ec2:stop-instances", "PT13H"),
        ("reboot-instances", "aws:ec2:reboot-instances", "P2Y"),
        ("pause-volume-io", "aws:ebs:pause-volume-io", "PT0S"),
    ];

    for (case, action_id, duration) in rows {
        let answer = create_template(
            &router,
            json!({
                "clientToken": case_token(&format!("other-{case}")),
                "description": format!("control-plane rules: {case}"),
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": {
                    "act": { "actionId": action_id, "parameters": { "duration": duration } }
                }
            }),
        )
        .await;
        let id = created_template_id(case, &answer);
        let fetched = send(
            &router,
            "GET",
            &format!("/experimentTemplates/{id}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            fetched.member("experimentTemplate")["actions"]["act"]["parameters"]["duration"]
                .as_str(),
            Some(duration),
            "case {case:?}: expected the parameter to be stored as sent, got {:?}",
            fetched.body
        );
    }
}

/// RULE: an action's `startAfter` names actions the SAME template declares, and a template
/// carries no more actions than the published quota allows ("Actions per experiment
/// template": 20) — which is also what keeps the cost of planning one template's run
/// proportional to the request.
#[tokio::test]
async fn the_action_graph_is_bounded_and_refers_only_to_actions_the_template_declares() {
    let router = emulator();

    /// The published quota, spelled out here so a wrong cap in `src/state.rs` fails this
    /// suite.
    const ACTIONS_CEILING: usize = 20;

    // The rows differ along the number of actions: one, the published ceiling, and one
    // past it.
    for (case, count, admitted) in [
        ("one action", 1usize, true),
        ("at the ceiling", ACTIONS_CEILING, true),
        ("one past the ceiling", ACTIONS_CEILING + 1, false),
    ] {
        let actions: serde_json::Map<String, Value> = (0..count)
            .map(|index| {
                (
                    format!("wait{index}"),
                    json!({
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT1M" },
                        // A chain, so the graph has edges to walk as well as nodes.
                        "startAfter": if index == 0 {
                            Vec::new()
                        } else {
                            vec![format!("wait{}", index - 1)]
                        }
                    }),
                )
            })
            .collect();
        let answer = create_template(
            &router,
            json!({
                "clientToken": case_token(&format!("graph-{case}")),
                "description": format!("control-plane rules: {case}"),
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": actions
            }),
        )
        .await;
        if admitted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a template within the quota is accepted, got {} with {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 402,
                "case {case:?}: a template past the quota is refused, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ServiceQuotaExceededException",
                "case {case:?}: expected ServiceQuotaExceededException, got {:?}",
                answer.body
            );
            assert!(
                message(&answer).contains(&ACTIONS_CEILING.to_string()),
                "case {case:?}: the refusal names the quota, got {:?}",
                answer.body
            );
        }
    }

    // The rows differ along what `startAfter` names: an action of the same template, two
    // of them, and a name the template does not declare.
    for (case, start_after, admitted) in [
        ("an action of the template", vec!["first"], true),
        ("two actions of the template", vec!["first", "second"], true),
        ("a name the template does not declare", vec!["absent"], false),
        ("one declared and one not", vec!["first", "absent"], false),
    ] {
        let answer = create_template(
            &router,
            json!({
                "clientToken": case_token(&format!("start-after-{case}")),
                "description": format!("control-plane rules: {case}"),
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": {
                    "first": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT1M" } },
                    "second": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT1M" } },
                    "last": {
                        "actionId": "aws:fis:wait",
                        "parameters": { "duration": "PT1M" },
                        "startAfter": start_after
                    }
                }
            }),
        )
        .await;
        if admitted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a startAfter the template declares is accepted, \
                 got {} with {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a startAfter the template does not declare is refused, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: a `clientToken` is an `@idempotencyToken`, which identifies a REQUEST — replaying
/// it with the same request reports the resource the first call created, and reusing it for
/// a request that differs in any member is a parameter mismatch, refused with
/// `ConflictException` (HTTP 409) without creating or changing anything.
#[tokio::test]
async fn reusing_a_client_token_for_a_different_create_request_is_a_conflict() {
    let router = emulator();

    let first_body = json!({
        "clientToken": token("shared-create"),
        "description": "the first request",
        "roleArn": ROLE_ARN,
        "stopConditions": [{ "source": "none" }],
        "targets": {
            "web": {
                "resourceType": "aws:ec2:instance",
                // Several entries in each map, so a rendering that depended on a map's
                // iteration order would show up as a false mismatch.
                "resourceTags": { "Stage": "test", "Owner": "chaos", "Tier": "web" },
                "selectionMode": "ALL"
            }
        },
        "actions": {
            "stop": {
                "actionId": "aws:ec2:stop-instances",
                "parameters": { "startInstancesAfterDuration": WAIT_DURATION },
                // The one target key `aws:ec2:stop-instances` publishes, naming the target
                // this template declares for it.
                "targets": { "Instances": "web" }
            },
            "second": {
                "actionId": "aws:fis:wait",
                "parameters": { "duration": "PT9M" },
                "startAfter": ["stop"]
            }
        },
        "tags": { "Owner": "chaos", "Stage": "test", "Tier": "web" }
    });
    let first = create_template(&router, first_body.clone()).await;
    let created = created_template_id("first create", &first);

    // The rows differ along WHICH member of the request the replay changes — one row per
    // member the template is built from, plus the replay that changes nothing, repeated:
    // a map's iteration order is not stable, so one identical replay passing is not
    // evidence that the request was rendered the same way both times.
    let mut rows: Vec<(String, Value)> = Vec::new();
    for attempt in 1..=4 {
        rows.push((format!("identical replay {attempt}"), first_body.clone()));
    }
    for (case, member, value) in [
        ("description", "description", json!("another description")),
        (
            "roleArn",
            "roleArn",
            json!("arn:aws:iam::000000000000:role/other"),
        ),
        (
            "actions",
            "actions",
            json!({ "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": "PT9M" } } }),
        ),
        (
            "targets",
            "targets",
            json!({ "web": { "resourceType": "aws:ec2:instance", "resourceTags": { "Stage": "prod" }, "selectionMode": "ALL" } }),
        ),
        (
            "stopConditions",
            "stopConditions",
            json!([{ "source": "aws:cloudwatch:alarm", "value": "arn:aws:cloudwatch:us-east-1:000000000000:alarm:cpu" }]),
        ),
        ("tags", "tags", json!({ "Owner": "someone-else" })),
        (
            "experimentOptions",
            "experimentOptions",
            json!({ "accountTargeting": "multi-account" }),
        ),
    ] {
        let mut body = first_body.clone();
        body[member] = value;
        rows.push((case.to_string(), body));
    }

    for (case, body) in rows {
        let case = case.as_str();
        let answer = create_template(&router, body).await;
        if case.starts_with("identical") {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the replay to report the first template, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.member("experimentTemplate"),
                first.member("experimentTemplate"),
                "case {case:?}: expected the first template, got {:?}",
                answer.body
            );
        } else {
            assert_eq!(
                answer.status, 409,
                "case {case:?}: expected HTTP 409 for a token reused with a different \
                 request, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {:?}",
                answer.body
            );
        }
        // Whatever the answer, the store still holds exactly the one template the token
        // created, unchanged.
        assert_eq!(
            template_count(&router).await,
            1,
            "case {case:?}: a conflicted or replayed create must add no template"
        );
        let fetched = send(
            &router,
            "GET",
            &format!("/experimentTemplates/{created}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            fetched.member("experimentTemplate"),
            first.member("experimentTemplate"),
            "case {case:?}: the template the token created must be untouched, got {:?}",
            fetched.body
        );
    }
}

/// RULE: see [`reusing_a_client_token_for_a_different_create_request_is_a_conflict`] — the
/// same rule on `StartExperiment`, where the request names the template to run.
#[tokio::test]
async fn reusing_a_client_token_for_a_different_start_request_is_a_conflict() {
    let router = emulator();

    let first_template = create_template(&router, wait_template("start-conflict-one", "PT5M")).await;
    let first_template = created_template_id("first template", &first_template);
    let other_template = create_template(&router, wait_template("start-conflict-two", "PT5M")).await;
    let other_template = created_template_id("second template", &other_template);

    let shared = token("shared-start");
    let first_body = json!({
        "clientToken": shared,
        "experimentTemplateId": first_template,
        "tags": { "Run": "first" },
        "experimentOptions": { "actionsMode": "run-all" }
    });
    let started = start_experiment(&router, first_body.clone()).await;
    assert_eq!(
        started.status, 200,
        "expected the first StartExperiment to succeed, got {} with {:?}",
        started.status, started.body
    );
    let experiment = started.member("experiment").clone();

    // The rows differ along which member of the start request the replay changes: the
    // template it runs, the tags it applies and the actions mode it runs under.
    let rows: [(&str, Value); 4] = [
        ("identical", first_body.clone()),
        (
            "another template",
            json!({
                "clientToken": shared,
                "experimentTemplateId": other_template,
                "tags": { "Run": "first" },
                "experimentOptions": { "actionsMode": "run-all" }
            }),
        ),
        (
            "other tags",
            json!({
                "clientToken": shared,
                "experimentTemplateId": first_template,
                "tags": { "Run": "second" },
                "experimentOptions": { "actionsMode": "run-all" }
            }),
        ),
        (
            "another actions mode",
            json!({
                "clientToken": shared,
                "experimentTemplateId": first_template,
                "tags": { "Run": "first" },
                "experimentOptions": { "actionsMode": "skip-all" }
            }),
        ),
    ];

    for (case, body) in rows {
        let answer = start_experiment(&router, body).await;
        if case == "identical" {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the replay to report the first experiment, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.member("experiment")["id"], experiment["id"],
                "case {case:?}: expected the experiment the token started, got {:?}",
                answer.body
            );
        } else {
            assert_eq!(
                answer.status, 409,
                "case {case:?}: expected HTTP 409, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {:?}",
                answer.body
            );
            // The answer must never be another template's run.
            assert!(
                answer.member("experiment").is_null(),
                "case {case:?}: a conflict carries no experiment, got {:?}",
                answer.body
            );
        }
        assert_eq!(
            experiment_count(&router).await,
            1,
            "case {case:?}: a conflicted or replayed start must add no experiment"
        );
    }
}

/// RULE: the account's active-experiment quota refuses the `StartExperiment` that would
/// exceed it with `ServiceQuotaExceededException` (HTTP 402) and creates no experiment;
/// the count is of experiments that are still ACTIVE, so an experiment that has ended
/// frees its slot.
#[tokio::test]
async fn the_active_experiment_quota_refuses_the_start_beyond_it() {
    let router = emulator();

    // One template per start, so no row depends on the per-template state of another.
    let mut templates = Vec::new();
    for index in 0..=ACTIVE_EXPERIMENTS_QUOTA {
        let case = format!("quota-{index}");
        let created = create_template(&router, wait_template(&case, "PT5M")).await;
        templates.push(created_template_id(&case, &created));
    }

    let mut started = Vec::new();
    for (index, template) in templates.iter().enumerate() {
        let answer = start_experiment(
            &router,
            json!({
                "clientToken": token(&format!("quota-start-{index}")),
                "experimentTemplateId": template
            }),
        )
        .await;
        if index < ACTIVE_EXPERIMENTS_QUOTA {
            assert_eq!(
                answer.status, 200,
                "case start {index}: expected a start within the quota to succeed, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            started.push(
                answer.member("experiment")["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        } else {
            assert_eq!(
                answer.status, 402,
                "case start {index}: expected the start past the quota to be refused, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ServiceQuotaExceededException",
                "case start {index}: expected ServiceQuotaExceededException, got {:?}",
                answer.body
            );
            assert!(
                message(&answer).contains(&ACTIVE_EXPERIMENTS_QUOTA.to_string()),
                "case start {index}: the refusal names the quota, got {:?}",
                answer.body
            );
        }
        assert_eq!(
            experiment_count(&router).await,
            started.len(),
            "case start {index}: a refused start must create no experiment"
        );
    }

    // A slot an experiment no longer occupies is a slot the next start may have: stop one
    // and the start that was refused a moment ago succeeds.
    let stopped = stop_experiment(&router, &started[0]).await;
    assert_eq!(
        stopped.status, 200,
        "expected the stop to be accepted, got {} with {:?}",
        stopped.status, stopped.body
    );
    let settled = poll_until_final(&router, &started[0], SETTLE_BUDGET).await;
    assert_eq!(
        settled["state"]["status"].as_str().unwrap_or_default(),
        "stopped",
        "expected the stopped experiment to have ended, got {settled:?}"
    );

    let after_release = start_experiment(
        &router,
        json!({
            "clientToken": token("quota-start-after-release"),
            "experimentTemplateId": templates[ACTIVE_EXPERIMENTS_QUOTA].clone()
        }),
    )
    .await;
    assert_eq!(
        after_release.status, 200,
        "expected the freed slot to admit a start, got {} with {:?}",
        after_release.status, after_release.body
    );
}

/// One action of a graph row: its name, its action id, the `duration` parameter it
/// carries (if any) and the actions its `startAfter` names.
type ActionRow = (&'static str, &'static str, Option<&'static str>, Vec<&'static str>);

/// One row of the run-plan table: the case, its actions, and the MODELLED length of the
/// longest path through them, in seconds.
type GraphRow = (&'static str, Vec<ActionRow>, u64);

/// One row of the length-refusal table: the case, the method, the path prefix, the body,
/// the over-long value and the member it belongs to.
type LengthRow<'a> = (&'a str, &'a str, &'a str, Option<Value>, &'a str, &'a str);

/// How far the emulator compresses the modelled run against the wall clock, and the window
/// it clamps the result into. Spelled out here rather than imported from the crate, so a
/// wrong value in `src/state.rs` fails this suite.
const RUN_CLOCK_SCALE: u32 = 60;
/// See [`RUN_CLOCK_SCALE`].
const MIN_RUN_SPAN: Duration = Duration::from_secs(5);
/// See [`RUN_CLOCK_SCALE`].
const MAX_RUN_SPAN: Duration = Duration::from_secs(15);

/// The span the emulator owes a transition it chooses the length of, which is also the
/// whole run of an experiment that failed before its actions started.
const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// RULE: a started experiment RUNS. It is observable in `running` for the longest path
/// through its actions' `duration` parameters — compressed by [`RUN_CLOCK_SCALE`] and
/// clamped into [`MIN_RUN_SPAN`]..=[`MAX_RUN_SPAN`], so the modelled span orders the real
/// one without any caller waiting the modelled span out — and then reaches the terminal
/// status its actions earned: `completed`, or `failed` carrying the failure the start
/// showed against the run, which claims no run duration at all.
///
/// Asserted on the state machine itself, at instants this case chooses, because the
/// smallest duration FIS accepts for a wait is a whole minute: letting a modelled run
/// elapse per row would say nothing more than settling the record at the same offsets does.
#[test]
fn an_experiment_runs_for_its_actions_durations_and_then_ends_at_the_status_they_earn() {
    // The rows differ along the shape of the action graph (one action, several in
    // parallel, several chained through `startAfter`, no actions at all, a `startAfter`
    // cycle no FIS template could carry), along WHICH actions carry the duration (a wait,
    // an action that injects a fault, a mix of the two, an action that takes no `duration`
    // at all) and along the modelled length itself — under the compressed floor, between
    // the floor and the ceiling, and past the ceiling.
    let rows: [GraphRow; 10] = [
        (
            "one wait, under the compressed floor",
            vec![("wait", "aws:fis:wait", Some("PT1M"), vec![])],
            60,
        ),
        (
            "two waits in parallel",
            vec![
                ("short", "aws:fis:wait", Some("PT1M"), vec![]),
                ("long", "aws:fis:wait", Some("PT5M"), vec![]),
            ],
            300,
        ),
        (
            "two waits chained",
            vec![
                ("first", "aws:fis:wait", Some("PT1M"), vec![]),
                ("second", "aws:fis:wait", Some("PT2M"), vec!["first"]),
            ],
            180,
        ),
        (
            "three waits, two of them chained",
            vec![
                ("first", "aws:fis:wait", Some("PT1M"), vec![]),
                ("second", "aws:fis:wait", Some("PT2M"), vec!["first"]),
                ("aside", "aws:fis:wait", Some("PT4M"), vec![]),
            ],
            240,
        ),
        (
            // An action that is not a wait carries its own `duration`, and the run lasts
            // it: `duration` is "the duration of the action" for every action that takes
            // it, not a parameter of one action id.
            "one api-fault injection, inside the compressed window",
            vec![(
                "fault",
                "aws:fis:inject-api-internal-error",
                Some("PT10M"),
                vec![],
            )],
            600,
        ),
        (
            "two api-fault injections chained",
            vec![
                (
                    "throttle",
                    "aws:fis:inject-api-throttle-error",
                    Some("PT6M"),
                    vec![],
                ),
                (
                    "internal",
                    "aws:fis:inject-api-internal-error",
                    Some("PT6M"),
                    vec!["throttle"],
                ),
            ],
            720,
        ),
        (
            "a wait beside a fault injection, past the compressed ceiling",
            vec![
                ("wait", "aws:fis:wait", Some("PT1M"), vec![]),
                (
                    "fault",
                    "aws:fis:inject-api-internal-error",
                    Some("PT20M"),
                    vec![],
                ),
            ],
            1_200,
        ),
        (
            // `aws:ec2:stop-instances` takes no `duration` (its
            // `startInstancesAfterDuration` is some other span), so it names no length and
            // the run is the observable floor.
            "an action that names no duration",
            vec![("stop", "aws:ec2:stop-instances", None, vec![])],
            0,
        ),
        (
            // A cycle is not a graph FIS would accept; what matters is that the plan is
            // BOUNDED rather than endless, which this row asserts by answering at all.
            "a startAfter cycle",
            vec![
                ("a", "aws:fis:wait", Some("PT1M"), vec!["b"]),
                ("b", "aws:fis:wait", Some("PT1M"), vec!["a"]),
            ],
            0,
        ),
        ("no actions at all", Vec::new(), 0),
    ];

    for (case, actions, modelled_seconds) in rows {
        let actions: HashMap<String, ExperimentTemplateAction> = actions
            .into_iter()
            .map(|(name, action_id, duration, start_after)| {
                (
                    name.to_string(),
                    ExperimentTemplateAction {
                        action_id: Some(action_id.to_string()),
                        description: None,
                        parameters: duration.map(|duration| {
                            HashMap::from([("duration".to_string(), duration.to_string())])
                        }),
                        targets: None,
                        start_after: Some(
                            start_after.iter().map(|name| (*name).to_string()).collect(),
                        ),
                    },
                )
            })
            .collect();

        // The rule, computed here from the modelled length the row states rather than read
        // back from the crate: a cycle is the one row whose modelled length this suite does
        // not predict, and it only has to be bounded.
        let expected_span = (Duration::from_secs(modelled_seconds) / RUN_CLOCK_SCALE)
            .clamp(MIN_RUN_SPAN, MAX_RUN_SPAN);

        let (span, outcome) = run_plan(&actions, None);
        assert_eq!(
            outcome,
            ExperimentOutcome::Completed,
            "case {case:?}: expected a run nothing was shown against to complete"
        );
        assert_eq!(
            span, expected_span,
            "case {case:?}: expected the longest path through the actions' durations on the \
             compressed clock, got {span:?}"
        );
        // Whatever the modelled durations are, the run stays inside the window: never
        // instantaneous (so `running` is observable) and never unbounded.
        assert!(
            span >= MIN_RUN_SPAN && span <= MAX_RUN_SPAN,
            "case {case:?}: expected a run span within the compressed window, got {span:?}"
        );

        // A run the start showed a failure against never begins: it claims no duration and
        // ends `failed`, carrying that failure rather than any other.
        let failure = ExperimentFailure::insufficient_privileges(
            ROLE_ARN,
            "blocked",
            "aws:cloudwatch:assert-alarm-state",
            "cloudwatch:DescribeAlarms",
        );
        let (failed_span, failed_outcome) = run_plan(&actions, Some(failure.clone()));
        assert_eq!(
            failed_span, TRANSITION_DELAY,
            "case {case:?}: a run that never started claims no duration, got {failed_span:?}"
        );
        assert_eq!(
            failed_outcome,
            ExperimentOutcome::Failed(failure.clone()),
            "case {case:?}: expected the failure the start showed, got {failed_outcome:?}"
        );

        // The same plan, now driven through the record the clock settles: the status only
        // ever advances along the documented progression, and `endTime` arrives with the
        // terminal status and not before.
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let template = template_record(&actions);
        let record = ExperimentRecord::from_template(
            "EXP000000000001".to_string(),
            "arn:aws:fis:us-east-1:000000000000:experiment/EXP000000000001".to_string(),
            &template,
            "run-all",
            None,
            started,
            None,
            // No store of another service is read here: this record's targets name their
            // resources, so the snapshot a tag-identified target would be resolved against
            // is the empty one.
            &TargetableResources::none(),
        );
        let expected_terminal = "completed";
        for (offset, expected) in [
            (Duration::ZERO, "initiating"),
            (Duration::from_millis(200), "initiating"),
            (span, "running"),
            (span + Duration::from_millis(500), expected_terminal),
            (span + Duration::from_secs(3_600), expected_terminal),
        ] {
            let mut settled = record.clone();
            // No alarm is breached, so what the record reports is what the clock owed it.
            settled.settle(
                started + offset,
                &service_com_amazonaws_fis_faultinjectionsimulator::state::stop_conditions::AlarmBreaches::none(),
            );
            let reported = settled.to_state().status.unwrap_or_default();
            assert_eq!(
                reported, expected,
                "case {case:?} at +{offset:?}: expected status {expected}, got {reported}"
            );
            assert_eq!(
                settled.end_time.is_some(),
                expected == expected_terminal,
                "case {case:?} at +{offset:?}: endTime arrives with the terminal status, \
                 got {:?}",
                settled.end_time
            );
            if expected == expected_terminal {
                assert!(
                    !settled.reason.is_empty(),
                    "case {case:?}: an experiment that ended says why, got {:?}",
                    settled.reason
                );
            }
            // `ExperimentError` is "the error information of the experiment when the
            // action has FAILED", so a run that is initiating, running or completed
            // reports none at all.
            assert!(
                settled.to_state().error.is_none(),
                "case {case:?} at +{offset:?}: expected no error beside status {expected}, \
                 got {:?}",
                settled.to_state().error
            );
        }

        // The same graph started under a role the account's IAM store shows may not
        // perform one of its actions: the run ends `failed` carrying BOTH halves of the
        // attribution — the reason, and the `ExperimentError` the model pairs with it.
        let mut denied = ExperimentRecord::from_template(
            "EXP000000000002".to_string(),
            "arn:aws:fis:us-east-1:000000000000:experiment/EXP000000000002".to_string(),
            &template,
            "run-all",
            None,
            started,
            Some(failure.clone()),
            &TargetableResources::none(),
        );
        denied.settle(
            started + TRANSITION_DELAY + TRANSITION_DELAY,
            &service_com_amazonaws_fis_faultinjectionsimulator::state::stop_conditions::AlarmBreaches::none(),
        );
        let state = denied.to_state();
        assert_eq!(
            state.status.as_deref(),
            Some("failed"),
            "case {case:?}: expected a run its role may not perform to fail, got {state:?}"
        );
        assert_eq!(
            state.reason.as_deref(),
            Some(failure.reason.as_str()),
            "case {case:?}: expected the failure's own reason, got {state:?}"
        );
        let error = state
            .error
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: a failed experiment reports an error, got {state:?}"));
        assert_eq!(
            error.code.as_deref(),
            Some(failure.code.as_str()),
            "case {case:?}: expected the failure's own error code, got {error:?}"
        );
        assert_eq!(
            error.location.as_deref(),
            Some("actions.blocked"),
            "case {case:?}: expected the error to locate the action that failed, got {error:?}"
        );
        assert_eq!(
            error.account_id.as_deref(),
            Some(ACCOUNT_ID),
            "case {case:?}: expected the error to name the account the run failed in, \
             got {error:?}"
        );
    }
}

/// A template record carrying `actions` and nothing else of interest, as
/// `CreateExperimentTemplate` would have stored it.
fn template_record(actions: &HashMap<String, ExperimentTemplateAction>) -> ExperimentTemplateRecord {
    let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    ExperimentTemplateRecord {
        id: "EXT000000000001".to_string(),
        arn: "arn:aws:fis:us-east-1:000000000000:experiment-template/EXT000000000001".to_string(),
        description: "a template built by the rules suite".to_string(),
        targets: HashMap::new(),
        actions: actions.clone(),
        stop_conditions: Vec::new(),
        role_arn: ROLE_ARN.to_string(),
        tags: HashMap::new(),
        log_configuration: None,
        experiment_options: ExperimentTemplateExperimentOptions {
            account_targeting: Some("single-account".to_string()),
            empty_target_resolution_mode: Some("fail".to_string()),
        },
        experiment_report_configuration: None,
        creation_time: created,
        last_update_time: created,
        // The sub-resource map a template carries; `targetAccountConfigurationsCount` is
        // derived from it, and no row here configures another account.
        target_account_configurations: std::collections::BTreeMap::new(),
    }
}

/// RULE: an experiment reaches `completed` with an `endTime` once its waits have elapsed,
/// having passed through `running` — asserted over the wire, with the stored run shortened
/// so the case does not sit out the whole minute that is the smallest wait FIS accepts.
#[tokio::test]
async fn a_wait_experiment_is_reported_completed_once_its_run_has_elapsed() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let created = create_template(&router, wait_template("completes", "PT1M")).await;
    let template = created_template_id("completes", &created);
    let started = start_experiment(
        &router,
        json!({ "clientToken": token("completes"), "experimentTemplateId": template }),
    )
    .await;
    let experiment = started.member("experiment").clone();
    let id = experiment["id"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        experiment["state"]["status"].as_str().unwrap_or_default(),
        "initiating",
        "expected a started experiment to be initiating, got {experiment:?}"
    );

    // It passes through `running`: the move is owed to the clock, so it arrives on its own.
    let mut seen_running = false;
    for _ in 0..40 {
        let polled = get_experiment(&router, &id).await;
        if polled.member("experiment")["state"]["status"].as_str() == Some("running") {
            seen_running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        seen_running,
        "expected the experiment to be running once the transition fell due"
    );

    // The wait is a minute of real time; bring the END of it forward rather than sitting
    // it out. Nothing else about the record is touched, so what the next read reports is
    // the service's own completion of the run.
    {
        let state = service_com_amazonaws_fis_faultinjectionsimulator::state::service_state(
            &*Arc::clone(&engine),
        )
        .expect("the fis store is reachable");
        let mut store = state.lock();
        let record = store
            .experiments
            .get_mut(&id)
            .expect("the started experiment is in the store");
        record.transition_due = Some(SystemTime::now());
    }

    let completed = poll_until_final(&router, &id, SETTLE_BUDGET).await;
    assert_eq!(
        completed["state"]["status"].as_str().unwrap_or_default(),
        "completed",
        "expected the experiment to complete once its run had elapsed, got {completed:?}"
    );
    assert!(
        !completed["endTime"].is_null(),
        "a completed experiment carries an endTime, got {completed:?}"
    );
    assert_eq!(
        completed["startTime"], experiment["startTime"],
        "completing an experiment must not move its startTime, got {completed:?}"
    );
}

/// RULE: `StopExperiment` on an experiment in a final state is refused with
/// `ValidationException` (HTTP 400) naming that experiment, and the record — its status,
/// its reason and its `endTime` — is left exactly as the run left it.
#[tokio::test]
async fn stopping_an_experiment_in_a_final_state_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    // The rows differ along WHICH final state the experiment is in: one stopped by a
    // request, one that failed, and one that completed.
    for case in ["stopped", "failed", "completed"] {
        let body = match case {
            "failed" => json!({
                "clientToken": token(&format!("final-{case}")),
                "description": format!("control-plane rules: {case}"),
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": { "stop": { "actionId": "aws:ec2:stop-instances" } }
            }),
            _ => wait_template(&format!("final-{case}"), "PT1M"),
        };
        let created = create_template(&router, body).await;
        let template = created_template_id(case, &created);
        let started = start_experiment(
            &router,
            json!({
                "clientToken": token(&format!("final-start-{case}")),
                "experimentTemplateId": template
            }),
        )
        .await;
        let id = started.member("experiment")["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if case == "stopped" {
            let stopped = stop_experiment(&router, &id).await;
            assert_eq!(
                stopped.status, 200,
                "case {case:?}: expected the first stop to be accepted, got {} with {:?}",
                stopped.status, stopped.body
            );
        }
        if case == "completed" || case == "failed" {
            // As in `a_wait_experiment_is_reported_completed_once_its_run_has_elapsed`:
            // the end of the run is brought forward instead of waited out.
            let state = service_com_amazonaws_fis_faultinjectionsimulator::state::service_state(
                &*Arc::clone(&engine),
            )
            .expect("the fis store is reachable");
            let mut store = state.lock();
            let record = store
                .experiments
                .get_mut(&id)
                .expect("the started experiment is in the store");
            record.transition_due = Some(SystemTime::now());
            record.run_span = Duration::from_millis(1);
            if case == "failed" {
                // The one way a run of this service ends `failed`: the experiment's
                // execution role may not perform one of its actions. Put on the record
                // here rather than reached through IAM, because the role records live in
                // the IAM service and this suite carries only FIS — the rule that PRODUCES
                // this outcome is asserted against the real IAM service in
                // `core/router/tests/all/fis_execution_role_permissions.rs`.
                record.outcome =
                    ExperimentOutcome::Failed(ExperimentFailure::insufficient_privileges(
                        ROLE_ARN,
                        "stop",
                        "aws:ec2:stop-instances",
                        "ec2:StopInstances",
                    ));
            }
        }

        let before = poll_until_final(&router, &id, SETTLE_BUDGET).await;
        assert_eq!(
            before["state"]["status"].as_str().unwrap_or_default(),
            case,
            "case {case:?}: expected the experiment to reach that final state, got {before:?}"
        );

        let refused = stop_experiment(&router, &id).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            message(&refused),
            format!("Cannot modify the state of an experiment in final state: {id}."),
            "case {case:?}: expected the refusal to name the experiment, got {:?}",
            refused.body
        );

        let after = get_experiment(&router, &id).await;
        assert_eq!(
            after.member("experiment")["state"], before["state"],
            "case {case:?}: a refused stop must not touch the state, got {:?}",
            after.body
        );
        assert_eq!(
            after.member("experiment")["endTime"], before["endTime"],
            "case {case:?}: a refused stop must not move endTime, got {:?}",
            after.body
        );
    }
}

/// RULE: a value longer than its member's modelled `@length` maximum is refused with
/// `ValidationException` (HTTP 400) whose message names the MEMBER —
/// "Maximum length exceeded: <member>" — and never echoes the rejected value back.
#[tokio::test]
async fn a_length_refusal_names_the_member_and_echoes_no_value() {
    let router = emulator();

    let long_id = "x".repeat(ID_CEILING + 1);
    let longest_id = "y".repeat(ID_CEILING + 40);
    let long_description = "d".repeat(DESCRIPTION_CEILING + 1);
    let long_token = "t".repeat(CLIENT_TOKEN_CEILING + 1);
    let long_role = format!("arn:aws:iam::000000000000:role/{}", "r".repeat(ROLE_ARN_CEILING));

    // The rows differ along the member that is too long, the operation it belongs to, how
    // it is carried (a path label, a query parameter, a body member) and how far past the
    // bound it goes — one past it and far past it.
    let rows: [LengthRow; 8] = [
        (
            "GetExperiment one past the ceiling",
            "GET",
            "/experiments/",
            None,
            &long_id,
            "id",
        ),
        (
            "GetExperiment far past the ceiling",
            "GET",
            "/experiments/",
            None,
            &longest_id,
            "id",
        ),
        (
            "StopExperiment one past the ceiling",
            "DELETE",
            "/experiments/",
            None,
            &long_id,
            "id",
        ),
        (
            "GetExperimentTemplate one past the ceiling",
            "GET",
            "/experimentTemplates/",
            None,
            &long_id,
            "id",
        ),
        (
            "DeleteExperimentTemplate one past the ceiling",
            "DELETE",
            "/experimentTemplates/",
            None,
            &long_id,
            "id",
        ),
        (
            "ListExperiments filtered one past the ceiling",
            "GET",
            "/experiments?experimentTemplateId=",
            None,
            &long_id,
            "experimentTemplateId",
        ),
        (
            "ListExperiments filtered far past the ceiling",
            "GET",
            "/experiments?experimentTemplateId=",
            None,
            &longest_id,
            "experimentTemplateId",
        ),
        (
            "ListExperimentTemplates is not refused for a short filter",
            "GET",
            "/experimentTemplates?nextToken=",
            None,
            "EXT000000000001",
            "",
        ),
    ];

    for (case, method, path, body, value, member) in rows {
        let answer = send(
            &router,
            method,
            &format!("{path}{value}"),
            body,
            FALLBACK_REGION,
        )
        .await;
        if member.is_empty() {
            // The complement: a value INSIDE its bound is never refused for length.
            assert_ne!(
                message(&answer),
                "Maximum length exceeded: nextToken",
                "case {case:?}: a value inside its bound is not a length refusal, got {:?}",
                answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        let stated = message(&answer);
        assert_eq!(
            stated,
            format!("Maximum length exceeded: {member}"),
            "case {case:?}: expected the member to be named, got {stated:?}"
        );
        assert!(
            !stated.contains(value),
            "case {case:?}: the refused value must not be echoed back, got {stated:?}"
        );
    }

    // The same rule on members carried in the body, and the complement at each published
    // ceiling: a value exactly at the bound is accepted.
    let body_rows: [(&str, Value, Option<&str>); 6] = [
        (
            "description one past the ceiling",
            json!({
                "clientToken": token("len-description"),
                "description": long_description,
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
            }),
            Some("description"),
        ),
        (
            "description at the ceiling",
            json!({
                "clientToken": token("len-description-ok"),
                "description": "d".repeat(DESCRIPTION_CEILING),
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
            }),
            None,
        ),
        (
            "clientToken one past the ceiling",
            json!({
                "clientToken": long_token,
                "description": "a token past its bound",
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
            }),
            Some("clientToken"),
        ),
        (
            "clientToken at the ceiling",
            json!({
                "clientToken": "t".repeat(CLIENT_TOKEN_CEILING),
                "description": "a token at its bound",
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
            }),
            None,
        ),
        (
            "roleArn one past the ceiling",
            json!({
                "clientToken": token("len-role"),
                "description": "a role arn past its bound",
                "roleArn": long_role,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } }
            }),
            Some("roleArn"),
        ),
        (
            "more tags than the map admits",
            json!({
                "clientToken": case_token("len-tags"),
                "description": "more tags than the map admits",
                "roleArn": ROLE_ARN,
                "stopConditions": [{ "source": "none" }],
                "targets": {},
                "actions": { "wait": { "actionId": "aws:fis:wait", "parameters": { "duration": WAIT_DURATION } } },
                "tags": (0..=TAG_MAP_CEILING)
                    .map(|index| (format!("Key{index}"), Value::String("value".to_string())))
                    .collect::<serde_json::Map<String, Value>>()
            }),
            Some("tags"),
        ),
    ];

    for (case, body, member) in body_rows {
        let answer = create_template(&router, body).await;
        match member {
            Some(member) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected HTTP 400, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {:?}",
                    answer.body
                );
                assert_eq!(
                    message(&answer),
                    format!("Maximum length exceeded: {member}"),
                    "case {case:?}: expected the member to be named, got {:?}",
                    answer.body
                );
            }
            None => assert_eq!(
                answer.status, 200,
                "case {case:?}: a value at its published ceiling is accepted, got {} with {:?}",
                answer.status, answer.body
            ),
        }
    }
}

/// RULE: a well-formed id naming no record is reported with `ResourceNotFoundException`
/// (HTTP 404) by the rule for its resource: an experiment names the id that was looked up
/// ("Experiment with ID <id> not found."), while an experiment template is reported
/// WITHOUT one ("ExperimentTemplate not found."), so no request identifier is disclosed.
#[tokio::test]
async fn an_unknown_id_is_reported_by_the_rule_for_its_resource() {
    let router = emulator();

    // The rows differ along the shape of the id: one of the shape this service mints, one
    // of the other collection's shape, one that is not an id at all, and one at the
    // published 64-character ceiling.
    let ids = [
        "EXP000000000001".to_string(),
        "EXT000000000001".to_string(),
        "not-an-id".to_string(),
        "z".repeat(ID_CEILING),
    ];

    for id in &ids {
        // Every operation that resolves an EXPERIMENT states it the same way.
        for (operation, method, path) in [
            ("GetExperiment", "GET", "/experiments"),
            ("StopExperiment", "DELETE", "/experiments"),
        ] {
            let answer = send(
                &router,
                method,
                &format!("{path}/{id}"),
                None,
                FALLBACK_REGION,
            )
            .await;
            assert_eq!(
                answer.status, 404,
                "case {operation} {id:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {operation} {id:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                message(&answer),
                format!("Experiment with ID {id} not found."),
                "case {operation} {id:?}: expected the experiment rule, got {:?}",
                answer.body
            );
        }

        // Every operation that resolves an experiment TEMPLATE states it the same way,
        // and none of them discloses the identifier.
        for (operation, method, path, body) in [
            (
                "GetExperimentTemplate",
                "GET",
                format!("/experimentTemplates/{id}"),
                None,
            ),
            (
                "DeleteExperimentTemplate",
                "DELETE",
                format!("/experimentTemplates/{id}"),
                None,
            ),
            (
                "StartExperiment",
                "POST",
                "/experiments".to_string(),
                Some(json!({
                    "clientToken": token(&format!("missing-{id}")),
                    "experimentTemplateId": id
                })),
            ),
            (
                "CreateTargetAccountConfiguration",
                "POST",
                format!("/experimentTemplates/{id}/targetAccountConfigurations/111122223333"),
                Some(json!({ "roleArn": "arn:aws:iam::111122223333:role/target" })),
            ),
        ] {
            let answer = send(&router, method, &path, body, FALLBACK_REGION).await;
            assert_eq!(
                answer.status, 404,
                "case {operation} {id:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {operation} {id:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                message(&answer),
                "ExperimentTemplate not found.",
                "case {operation} {id:?}: expected the template rule, got {:?}",
                answer.body
            );
            assert!(
                !message(&answer).contains(id.as_str()),
                "case {operation} {id:?}: the template rule discloses no identifier, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: `UpdateTargetAccountConfiguration` updates the configuration a template holds for
/// one account id — the members the request names replace the stored ones and the members
/// it omits keep what they had — and refuses with `ResourceNotFoundException` (HTTP 404)
/// when the resource it names does not exist (the template holds no configuration for that
/// account, or there is no such template). There are no upsert semantics: a refused update
/// leaves the template with no configuration for that account.
#[tokio::test]
async fn updating_a_target_account_configuration_changes_only_what_the_request_names() {
    let router = emulator();

    // A target account configuration belongs to a template that targets MULTIPLE accounts:
    // `accountTargeting` and the configurations a template holds state the same thing
    // between them.
    let mut body = wait_template("target-accounts", "PT5M");
    body["experimentOptions"] = json!({ "accountTargeting": "multi-account" });
    let created = create_template(&router, body).await;
    let template = created_template_id("target-accounts", &created);

    // No configuration for that account yet: the update is refused and creates none.
    let accounts = ["111122223333", "444455556666"];
    for account in accounts {
        let refused = send(
            &router,
            "PATCH",
            &format!("/experimentTemplates/{template}/targetAccountConfigurations/{account}"),
            Some(json!({ "roleArn": format!("arn:aws:iam::{account}:role/fis-target") })),
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {account:?}: expected HTTP 404 with no configuration to update, \
             got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ResourceNotFoundException",
            "case {account:?}: expected ResourceNotFoundException, got {:?}",
            refused.body
        );
        // The refused update created nothing: reading the configuration back is the same
        // refusal, so there is no upsert hiding behind the 404.
        let read = send(
            &router,
            "GET",
            &format!("/experimentTemplates/{template}/targetAccountConfigurations/{account}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            read.status, 404,
            "case {account:?}: a refused update must create no configuration, got {} with {:?}",
            read.status, read.body
        );
    }

    // The configurations this case updates, created through the service's own
    // `CreateTargetAccountConfiguration`.
    for account in accounts {
        let created = send(
            &router,
            "POST",
            &format!("/experimentTemplates/{template}/targetAccountConfigurations/{account}"),
            Some(json!({
                "roleArn": format!("arn:aws:iam::{account}:role/created"),
                "description": "created"
            })),
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {account:?}: expected the configuration to be created, got {} with {:?}",
            created.status, created.body
        );
    }

    // The rows differ along which members the update names: both, each one alone, and a
    // description emptied out.
    let rows: [(&str, Value, Option<&str>, Option<&str>); 4] = [
        (
            "both members",
            json!({
                "roleArn": "arn:aws:iam::111122223333:role/first",
                "description": "the first update"
            }),
            Some("arn:aws:iam::111122223333:role/first"),
            Some("the first update"),
        ),
        (
            "the description alone",
            json!({ "description": "only the description moved" }),
            Some("arn:aws:iam::111122223333:role/first"),
            Some("only the description moved"),
        ),
        (
            "the role alone",
            json!({ "roleArn": "arn:aws:iam::111122223333:role/second" }),
            Some("arn:aws:iam::111122223333:role/second"),
            Some("only the description moved"),
        ),
        (
            // An explicitly empty description CLEARS the stored one: the configuration
            // then reports no `description` member at all, the same answer one that never
            // had a description gives.
            "an empty description",
            json!({ "description": "" }),
            Some("arn:aws:iam::111122223333:role/second"),
            None,
        ),
    ];

    let account = accounts[0];
    for (case, body, expected_role, expected_description) in rows {
        let answer = send(
            &router,
            "PATCH",
            &format!("/experimentTemplates/{template}/targetAccountConfigurations/{account}"),
            Some(body),
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = answer.member("targetAccountConfiguration").clone();
        assert_eq!(
            reported["accountId"].as_str(),
            Some(account),
            "case {case:?}: expected the configuration's own account id, got {reported:?}"
        );
        assert_eq!(
            reported["roleArn"].as_str(),
            expected_role,
            "case {case:?}: expected the stored roleArn, got {reported:?}"
        );
        assert_eq!(
            reported["description"].as_str(),
            expected_description,
            "case {case:?}: expected the stored description, got {reported:?}"
        );

        // The answer came from the store: reading the configuration back through
        // `GetTargetAccountConfiguration` reports the same record.
        let read_back = send(
            &router,
            "GET",
            &format!("/experimentTemplates/{template}/targetAccountConfigurations/{account}"),
            None,
            FALLBACK_REGION,
        )
        .await;
        assert_eq!(
            read_back.member("targetAccountConfiguration"),
            &reported,
            "case {case:?}: the update must be what a later read reports, got {:?}",
            read_back.body
        );
    }

    // The other account's configuration was never touched by any of those rows.
    let untouched = send(
        &router,
        "GET",
        &format!(
            "/experimentTemplates/{template}/targetAccountConfigurations/{}",
            accounts[1]
        ),
        None,
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        untouched.member("targetAccountConfiguration")["roleArn"].as_str(),
        Some(format!("arn:aws:iam::{}:role/created", accounts[1]).as_str()),
        "expected the other account's configuration to be untouched, got {:?}",
        untouched.body
    );

    // A template that does not exist holds no configuration either: the same refusal,
    // since the resource the request names does not exist.
    let unknown = send(
        &router,
        "PATCH",
        &format!(
            "/experimentTemplates/EXT000000000099/targetAccountConfigurations/{}",
            accounts[0]
        ),
        Some(json!({ "description": "a configuration of a template that is not there" })),
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        unknown.status, 404,
        "expected HTTP 404 for an unknown template, got {} with {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {:?}",
        unknown.body
    );
}
