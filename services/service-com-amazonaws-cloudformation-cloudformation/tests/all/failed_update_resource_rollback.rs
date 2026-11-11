// SPDX-License-Identifier: CC0-1.0

//! What a stack update does with the resources whose change the services that own them
//! REFUSED: how many of them it attempts, what it reports about each, and how each one is put
//! back.
//!
//! The rule every row here holds the emulator to: **a deployment attempts every resource
//! whose dependencies allow it — so each resource whose provider refused its change reports
//! its own `UPDATE_IN_PROGRESS`/`UPDATE_FAILED` pair carrying that provider's refusal, and the
//! stack-level reason names the WHOLE failed set — and each of those resources is then put
//! back to the configuration in force before the update, reported by its own
//! `UPDATE_ROLLBACK_IN_PROGRESS`/`UPDATE_ROLLBACK_COMPLETE` transitions where the stack rests
//! in `UPDATE_FAILED` (`DisableRollback`) and as its own update reaching `UPDATE_COMPLETE`
//! where the stack rolls back.**
//!
//! Why it matters: a client diagnoses a failed deploy from the failed-resource list and from
//! the per-resource events — read the events, derive what each broken resource needs, fix
//! forward. A deployment that stops at the first failure hides every other broken resource, so
//! the fix that follows fails again; and a resource left reporting a status its live
//! configuration contradicts makes every drift and inventory decision built on resource
//! statuses wrong.
//!
//! The rows differ along what the rule generalises over: HOW MANY independent resources the
//! update breaks (one, two, three), whether the update also carries a change that SUCCEEDS
//! beside the broken ones, and which way the request asked a failure to be handled
//! (`DisableRollback=true`, and the documented default where the stack rolls itself back).
//! The broken property is one AWS Lambda itself decides: "the function version that the alias
//! invokes" has to be a version the function has, so an alias pointed at a version number the
//! function never published is refused with `ResourceNotFoundException` — and the version
//! numbers each row points at are its own.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::hotswapped_function_code::lambda;
use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, member, resource_event_reason,
    resource_event_statuses, router_with_engine, wait_for_the_transition, xml_blocks, xml_text,
};

/// The runtime every function these templates declare is created with.
const RUNTIME: &str = "python3.11";

/// The handler and the inline source every one of them carries: the resources under test are
/// the ALIASES, so one body serves every row.
const HANDLER: &str = "index.handler";
const SOURCE: &str = "def handler(event, context):\n    return {'ok': True}\n";

/// The alias name every function's alias is declared under. Not a version-number spelling:
/// the `Alias` pattern (`^(?!^[0-9]+$)([a-zA-Z0-9-_]+)$`) keeps an alias name out of that
/// namespace.
const ALIAS_NAME: &str = "live";

/// The version every alias invokes while the stack is healthy: the function's unpublished
/// version, which a function always has.
const HEALTHY_VERSION: &str = "$LATEST";

/// The wrapper CloudFormation reports a provider's own refusal inside, on the failed
/// resource's own event: `ResourceStatusReason` is the "success/failure message associated
/// with the resource".
const HANDLER_MESSAGE_PREFIX: &str = "Resource handler returned message: \"";

/// The `HandlerErrorCode` class a refusal that the resource was NOT FOUND falls in, which is
/// what Lambda answers for a function version it does not hold.
const NOT_FOUND_HANDLER_CODE: &str = "HandlerErrorCode: NotFound";

/// One row: the stack, the version each of its aliases is pointed at by the update, whether
/// the update also changes something that succeeds, and how the request asked the failure to
/// be handled.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The version number each broken alias is pointed at — one entry per function the
    /// template declares, so the row's own length is how many resources it breaks.
    missing_versions: &'static [&'static str],
    /// Whether the update also carries a change the owning service APPLIES: the function's
    /// own `Description`, beside the aliases it breaks.
    changes_a_healthy_resource: bool,
    /// Whether the request asks for the failed state to be kept.
    disables_rollback: bool,
}

/// The class the rule covers.
const ROWS: &[Row] = &[
    Row {
        case: "two independent aliases broken, the failed state kept",
        stack: "cfnuf-two-kept",
        missing_versions: &["593", "268"],
        changes_a_healthy_resource: false,
        disables_rollback: true,
    },
    Row {
        case: "three independent aliases broken, the failed state kept",
        stack: "cfnuf-three-kept",
        missing_versions: &["4", "17", "901"],
        changes_a_healthy_resource: false,
        disables_rollback: true,
    },
    Row {
        case: "one alias broken beside a change the owning service applies, the failed state \
               kept",
        stack: "cfnuf-one-kept",
        missing_versions: &["2"],
        changes_a_healthy_resource: true,
        disables_rollback: true,
    },
    Row {
        case: "two independent aliases broken, the stack rolling itself back",
        stack: "cfnuf-two-rolled",
        missing_versions: &["31", "77"],
        changes_a_healthy_resource: false,
        disables_rollback: false,
    },
];

/// The execution role, and one function plus one alias per entry of `aliased`, where each
/// entry is the version THAT alias invokes.
///
/// `description` is the function `Description` this revision declares, which is what a row
/// changes to carry an update the owning service applies beside the aliases it breaks.
fn template(stack: &str, aliased: &[&str], description: &str) -> String {
    let mut resources = serde_json::Map::new();
    resources.insert(
        "Role".to_string(),
        json!({
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": format!("{stack}-exec"),
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": "lambda.amazonaws.com" },
                        "Action": "sts:AssumeRole"
                    }]
                }
            }
        }),
    );
    for (index, version) in aliased.iter().enumerate() {
        resources.insert(
            function_logical_id(index),
            json!({
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": function_name(stack, index),
                    "Runtime": RUNTIME,
                    "Handler": HANDLER,
                    "Description": description,
                    "Role": { "Fn::GetAtt": ["Role", "Arn"] },
                    "Code": { "ZipFile": SOURCE }
                }
            }),
        );
        resources.insert(
            alias_logical_id(index),
            json!({
                "Type": "AWS::Lambda::Alias",
                "Properties": {
                    "FunctionName": { "Ref": function_logical_id(index) },
                    "Name": ALIAS_NAME,
                    "FunctionVersion": version
                }
            }),
        );
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": Value::Object(resources),
    })
    .to_string()
}

/// The logical id of the `index`-th function, and of the alias that follows it.
fn function_logical_id(index: usize) -> String {
    format!("Fn{index}")
}

fn alias_logical_id(index: usize) -> String {
    format!("Fn{index}Alias")
}

/// The function name the `index`-th function declares, and so the physical id the stack
/// publishes for it.
fn function_name(stack: &str, index: usize) -> String {
    format!("{stack}-fn{index}")
}

/// The `StackStatus` the stack reports right now.
async fn stack_status(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status_code, body) = describe_stack(router, stack).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStacks to answer, got {status_code} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The `StackStatusReason` the stack reports right now, or the empty string for a stack
/// reporting none.
async fn stack_status_reason(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status_code, body) = describe_stack(router, stack).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStacks to answer, got {status_code} {body}"
    );
    xml_text(&body, "StackStatusReason").unwrap_or_default()
}

/// The `ResourceStatus` the stack's own per-resource view reports for `logical_id`.
async fn resource_status(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (status_code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStackResources to answer, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .find(|entry| xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id))
        .and_then(|entry| xml_text(entry, "ResourceStatus"))
        .unwrap_or_default()
}

/// Every `ResourceStatus` the stack's event stream carries for `logical_id`, OLDEST first:
/// `DescribeStackEvents` answers in reverse chronological order, and what a row asserts is the
/// order the walk did these things in.
async fn resource_event_sequence(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> Vec<String> {
    let mut statuses = resource_event_statuses(router, case, stack, logical_id).await;
    statuses.reverse();
    statuses
}

/// The version AWS Lambda itself reports the alias of `function` invokes, or `None` where that
/// service holds no such alias.
async fn alias_version(router: &MultiServiceRouter, function: &str) -> Option<String> {
    let answered = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{function}/aliases/{ALIAS_NAME}"),
        &[],
        &[],
    )
    .await;
    if answered.status != 200 {
        return None;
    }
    answered
        .body
        .get("FunctionVersion")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `Description` AWS Lambda reports for `function`, or `None` where it holds no such
/// function.
async fn function_description(router: &MultiServiceRouter, function: &str) -> Option<String> {
    let answered = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{function}/configuration"),
        &[],
        &[],
    )
    .await;
    if answered.status != 200 {
        return None;
    }
    answered
        .body
        .get("Description")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `Description` the healthy revision declares, and the one the broken update declares
/// instead for a row that carries a change beside the aliases it breaks.
const CREATED_DESCRIPTION: &str = "the deployed revision";
const UPDATED_DESCRIPTION: &str = "the revision the update declares";

/// RULE: an update attempts every resource whose dependencies allow it, names the whole failed
/// set in the stack-level reason, carries each provider's own refusal on that resource's own
/// event, and puts every failed resource back the way it was.
#[tokio::test]
async fn every_resource_an_update_breaks_reports_its_own_failure_and_is_put_back() {
    for row in ROWS {
        let case = row.case;
        let (router, _engine) = router_with_engine();

        // ---- The healthy deployment the broken update is made against. ----
        let healthy: Vec<&str> = row.missing_versions.iter().map(|_| HEALTHY_VERSION).collect();
        let deployed = template(row.stack, &healthy, CREATED_DESCRIPTION);
        let (status_code, body) =
            create_stack(&router, row.stack, &deployed, &acknowledging_iam(&[])).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let created = stack_status(&router, case, row.stack).await;
        assert_eq!(
            created, "CREATE_COMPLETE",
            "case {case:?}: expected the create to complete, got {created:?}"
        );
        for index in 0..row.missing_versions.len() {
            let invoked = alias_version(&router, &function_name(row.stack, index)).await;
            assert_eq!(
                invoked.as_deref(),
                Some(HEALTHY_VERSION),
                "case {case:?}: expected AWS Lambda to hold the alias of {:?} invoking \
                 {HEALTHY_VERSION:?}, got {invoked:?}",
                function_logical_id(index)
            );
        }

        // ---- The update: every alias is pointed at a version its function never published.
        let description = if row.changes_a_healthy_resource {
            UPDATED_DESCRIPTION
        } else {
            CREATED_DESCRIPTION
        };
        let broken = template(row.stack, row.missing_versions, description);
        let mut members = vec![
            member("StackName", row.stack),
            member("TemplateBody", &broken),
        ];
        members.extend(acknowledging_iam(&[]));
        if row.disables_rollback {
            members.push(member("DisableRollback", "true"));
        }
        let (status_code, body) = call(&router, "UpdateStack", &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected UpdateStack to be accepted, got {status_code} {body}"
        );
        wait_for_the_transition().await;

        // The stack ends where the request asked a failure to leave it.
        let settled = stack_status(&router, case, row.stack).await;
        let expected_status = if row.disables_rollback {
            "UPDATE_FAILED"
        } else {
            "UPDATE_ROLLBACK_COMPLETE"
        };
        assert_eq!(
            settled, expected_status,
            "case {case:?}: expected the failed update to leave the stack {expected_status:?}, \
             got {settled:?}"
        );

        // The stack-level reason names EVERY resource that failed, in declaration order.
        let named: Vec<String> = (0..row.missing_versions.len())
            .map(alias_logical_id)
            .collect();
        let reason = stack_status_reason(&router, case, row.stack).await;
        let expected_reason = format!(
            "The following resource(s) failed to update: [{}]. ",
            named.join(", ")
        );
        assert_eq!(
            reason, expected_reason,
            "case {case:?}: expected the stack to name the whole failed set \
             ({expected_reason:?}), got {reason:?}"
        );

        // Each broken resource reports its own transitions, and the revert that follows them.
        let put_back = if row.disables_rollback {
            vec![
                "UPDATE_ROLLBACK_IN_PROGRESS".to_string(),
                "UPDATE_ROLLBACK_COMPLETE".to_string(),
            ]
        } else {
            vec![
                "UPDATE_IN_PROGRESS".to_string(),
                "UPDATE_COMPLETE".to_string(),
            ]
        };
        for index in 0..row.missing_versions.len() {
            let logical_id = alias_logical_id(index);
            let mut expected = vec![
                "UPDATE_IN_PROGRESS".to_string(),
                "UPDATE_FAILED".to_string(),
            ];
            expected.extend(put_back.iter().cloned());
            let sequence: Vec<String> =
                resource_event_sequence(&router, case, row.stack, &logical_id)
                    .await
                    .into_iter()
                    // The create's own transitions came first; what this row is about is what
                    // the UPDATE recorded for the resource.
                    .skip_while(|status| status.starts_with("CREATE_"))
                    .collect();
            assert_eq!(
                sequence, expected,
                "case {case:?}: expected {logical_id:?} to report {expected:?}, got {sequence:?}"
            );

            // ...and the UPDATE_FAILED event carries the provider's own refusal, classified.
            let reported =
                resource_event_reason(&router, row.stack, &logical_id, "UPDATE_FAILED").await;
            assert!(
                reported.starts_with(HANDLER_MESSAGE_PREFIX),
                "case {case:?}: expected {logical_id:?}'s failure to open with \
                 {HANDLER_MESSAGE_PREFIX:?}, got {reported:?}"
            );
            assert!(
                reported.contains(NOT_FOUND_HANDLER_CODE),
                "case {case:?}: expected {logical_id:?}'s failure to carry \
                 {NOT_FOUND_HANDLER_CODE:?}, got {reported:?}"
            );
            assert!(
                reported.contains(row.missing_versions[index]),
                "case {case:?}: expected {logical_id:?}'s failure to name the version \
                 {:?} it could not be pointed at, got {reported:?}",
                row.missing_versions[index]
            );

            // ...and the stack's per-resource view reports what the revert left it in.
            let status = resource_status(&router, case, row.stack, &logical_id).await;
            let expected_status = put_back
                .last()
                .expect("the revert ends in a status")
                .to_string();
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected {logical_id:?} to report {expected_status:?}, got \
                 {status:?}"
            );

            // ...and AWS Lambda still has the alias invoking the version it invoked before
            // the update: the resource really is back the way it was.
            let invoked = alias_version(&router, &function_name(row.stack, index)).await;
            assert_eq!(
                invoked.as_deref(),
                Some(HEALTHY_VERSION),
                "case {case:?}: expected the alias of {:?} to be left invoking \
                 {HEALTHY_VERSION:?}, got {invoked:?}",
                function_logical_id(index)
            );
        }

        // A change the owning service APPLIED beside them is reported as applied, and is in
        // force there — until the stack's own rollback puts it back too.
        if row.changes_a_healthy_resource {
            let function = function_name(row.stack, 0);
            let described = function_description(&router, &function).await;
            let expected = if row.disables_rollback {
                UPDATED_DESCRIPTION
            } else {
                CREATED_DESCRIPTION
            };
            assert_eq!(
                described.as_deref(),
                Some(expected),
                "case {case:?}: expected AWS Lambda to hold the description {expected:?} for \
                 {function:?}, got {described:?}"
            );
        }
    }
}

/// RULE (the rollback's own phases): a rollback of a stack that rests in a failed update
/// passes through the cleanup phase — `UPDATE_ROLLBACK_IN_PROGRESS`,
/// `UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS`, `UPDATE_ROLLBACK_COMPLETE` — whichever call
/// asked for it.
#[tokio::test]
async fn a_requested_rollback_reports_every_documented_phase() {
    let stack = "cfnuf-rollback-phases";
    let (router, _engine) = router_with_engine();
    let deployed = template(stack, &[HEALTHY_VERSION], CREATED_DESCRIPTION);
    let (status_code, body) =
        create_stack(&router, stack, &deployed, &acknowledging_iam(&[])).await;
    assert_eq!(
        status_code, 200,
        "case \"a healthy deployment\": expected CreateStack to be accepted, got \
         {status_code} {body}"
    );
    wait_for_the_transition().await;

    let broken = template(stack, &["404"], CREATED_DESCRIPTION);
    let mut members = vec![member("StackName", stack), member("TemplateBody", &broken)];
    members.extend(acknowledging_iam(&[]));
    members.push(member("DisableRollback", "true"));
    let (status_code, body) = call(&router, "UpdateStack", &members).await;
    assert_eq!(
        status_code, 200,
        "case \"the broken update\": expected UpdateStack to be accepted, got \
         {status_code} {body}"
    );
    wait_for_the_transition().await;
    let failed = stack_status(&router, "the broken update", stack).await;
    assert_eq!(
        failed, "UPDATE_FAILED",
        "case \"the broken update\": expected the stack to rest in UPDATE_FAILED, got {failed:?}"
    );

    let (status_code, body) = call(&router, "RollbackStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "case \"the requested rollback\": expected RollbackStack to be accepted, got \
         {status_code} {body}"
    );
    wait_for_the_transition().await;

    // The stack's OWN events are the ones naming the stack itself.
    let phases = resource_event_sequence(&router, "the requested rollback", stack, stack).await;
    let rolled_back: Vec<String> = phases
        .into_iter()
        .skip_while(|status| status != "UPDATE_ROLLBACK_IN_PROGRESS")
        .collect();
    assert_eq!(
        rolled_back,
        vec![
            "UPDATE_ROLLBACK_IN_PROGRESS".to_string(),
            "UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS".to_string(),
            "UPDATE_ROLLBACK_COMPLETE".to_string(),
        ],
        "case \"the requested rollback\": expected the documented rollback phases, got \
         {rolled_back:?}"
    );
}

/// RULE (the same accounting on a CREATE): a create that fails with its rollback disabled
/// keeps everything it had made — each resource reporting the status the walk left it in — and
/// the resources are still there in the services that own them.
#[tokio::test]
async fn a_failed_create_with_rollback_disabled_keeps_what_it_made() {
    let stack = "cfnuf-create-kept";
    let (router, _engine) = router_with_engine();
    // The alias names a version the function never published, so its create is refused.
    let broken = template(stack, &["593"], CREATED_DESCRIPTION);
    let mut members = acknowledging_iam(&[]);
    members.push(member("DisableRollback", "true"));
    let (status_code, body) = create_stack(&router, stack, &broken, &members).await;
    assert_eq!(
        status_code, 200,
        "case \"a create whose alias names a missing version\": expected CreateStack to be \
         accepted, got {status_code} {body}"
    );
    wait_for_the_transition().await;

    let settled = stack_status(&router, "the kept create", stack).await;
    assert_eq!(
        settled, "CREATE_FAILED",
        "case \"the kept create\": expected the stack to rest in CREATE_FAILED, got {settled:?}"
    );
    let kept: &[(&str, &str)] = &[
        ("Role", "CREATE_COMPLETE"),
        ("Fn0", "CREATE_COMPLETE"),
        ("Fn0Alias", "CREATE_FAILED"),
    ];
    for (logical_id, expected) in kept {
        let status = resource_status(&router, "the kept create", stack, logical_id).await;
        assert_eq!(
            status, *expected,
            "case \"the kept create\": expected {logical_id:?} to report {expected:?}, got \
             {status:?}"
        );
    }
    // The function the create DID make is still there in AWS Lambda: nothing was rolled back.
    let function = function_name(stack, 0);
    let described = function_description(&router, &function).await;
    assert_eq!(
        described.as_deref(),
        Some(CREATED_DESCRIPTION),
        "case \"the kept create\": expected AWS Lambda to still hold {function:?}, got \
         {described:?}"
    );
    // ...and the alias its create was refused for exists nowhere.
    let invoked = alias_version(&router, &function).await;
    assert_eq!(
        invoked, None,
        "case \"the kept create\": expected no alias of {function:?} to exist, got {invoked:?}"
    );

    // The rollback the caller was keeping the estate for removes it again.
    let (status_code, body) = call(&router, "RollbackStack", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "case \"the rollback of the kept create\": expected RollbackStack to be accepted, got \
         {status_code} {body}"
    );
    wait_for_the_transition().await;
    let described = function_description(&router, &function).await;
    assert_eq!(
        described, None,
        "case \"the rollback of the kept create\": expected AWS Lambda to no longer hold \
         {function:?}, got {described:?}"
    );
}

/// RULE (the same accounting through a CHANGE SET): executing a change set deploys a
/// template over the resources the stack already has, so a resource whose change its provider
/// refuses is named in the whole failed set, put back the way it was, and left reporting the
/// status that revert gave it — exactly as a failed `UpdateStack` leaves it.
#[tokio::test]
async fn an_executed_change_set_reports_and_puts_back_the_resources_it_breaks() {
    let case = "a change set whose aliases name missing versions, the failed state kept";
    let stack = "cfnuf-change-set";
    let change_set = "the-broken-change-set";
    let (router, _engine) = router_with_engine();

    let deployed = template(stack, &[HEALTHY_VERSION, HEALTHY_VERSION], CREATED_DESCRIPTION);
    let (status_code, body) =
        create_stack(&router, stack, &deployed, &acknowledging_iam(&[])).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {status_code} {body}"
    );
    wait_for_the_transition().await;

    let broken = template(stack, &["593", "268"], CREATED_DESCRIPTION);
    let mut members = vec![
        member("StackName", stack),
        member("ChangeSetName", change_set),
        member("ChangeSetType", "UPDATE"),
        member("TemplateBody", &broken),
    ];
    members.extend(acknowledging_iam(&[]));
    let (status_code, body) = call(&router, "CreateChangeSet", &members).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected CreateChangeSet to be accepted, got {status_code} {body}"
    );
    wait_for_the_transition().await;

    let (status_code, body) = call(
        &router,
        "ExecuteChangeSet",
        &[
            member("ChangeSetName", change_set),
            member("StackName", stack),
            member("DisableRollback", "true"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected ExecuteChangeSet to be accepted, got {status_code} {body}"
    );
    wait_for_the_transition().await;

    let settled = stack_status(&router, case, stack).await;
    assert_eq!(
        settled, "UPDATE_FAILED",
        "case {case:?}: expected the failed execution to rest in UPDATE_FAILED, got \
         {settled:?}"
    );
    let reason = stack_status_reason(&router, case, stack).await;
    assert_eq!(
        reason, "The following resource(s) failed to update: [Fn0Alias, Fn1Alias]. ",
        "case {case:?}: expected the execution to name the whole failed set, got {reason:?}"
    );
    for index in 0..2 {
        let logical_id = alias_logical_id(index);
        let sequence: Vec<String> = resource_event_sequence(&router, case, stack, &logical_id)
            .await
            .into_iter()
            .skip_while(|status| status.starts_with("CREATE_"))
            .collect();
        assert_eq!(
            sequence,
            vec![
                "UPDATE_IN_PROGRESS".to_string(),
                "UPDATE_FAILED".to_string(),
                "UPDATE_ROLLBACK_IN_PROGRESS".to_string(),
                "UPDATE_ROLLBACK_COMPLETE".to_string(),
            ],
            "case {case:?}: expected {logical_id:?} to report its failure and its revert, got \
             {sequence:?}"
        );
        let status = resource_status(&router, case, stack, &logical_id).await;
        assert_eq!(
            status, "UPDATE_ROLLBACK_COMPLETE",
            "case {case:?}: expected {logical_id:?} to report the status its revert left, got \
             {status:?}"
        );
        let invoked = alias_version(&router, &function_name(stack, index)).await;
        assert_eq!(
            invoked.as_deref(),
            Some(HEALTHY_VERSION),
            "case {case:?}: expected the alias of {:?} to be left invoking \
             {HEALTHY_VERSION:?}, got {invoked:?}",
            function_logical_id(index)
        );
    }
}

/// RULE (the other half of cause 1's rule): a resource that DEPENDS on one the deployment
/// could not carry out is left alone — nothing is asked of its owning service, it reports no
/// transition of its own, and the stack-level reason names only the resource that failed.
#[tokio::test]
async fn a_resource_depending_on_a_failed_one_is_left_alone() {
    let case = "a function whose update waits on the alias the update breaks";
    let stack = "cfnuf-dependent";
    let (router, _engine) = router_with_engine();

    // `Fn1` waits for `Fn0Alias`, so the alias the update breaks is what it depends on.
    let waiting_on_the_alias = |source: &str, description: &str| {
        let mut declared: serde_json::Map<String, Value> =
            serde_json::from_str(&template(stack, &[source], description))
                .ok()
                .and_then(|body: Value| body.get("Resources").and_then(Value::as_object).cloned())
                .expect("the template declares resources");
        declared.insert(
            "Fn1".to_string(),
            json!({
                "Type": "AWS::Lambda::Function",
                "DependsOn": alias_logical_id(0),
                "Properties": {
                    "FunctionName": function_name(stack, 1),
                    "Runtime": RUNTIME,
                    "Handler": HANDLER,
                    "Description": description,
                    "Role": { "Fn::GetAtt": ["Role", "Arn"] },
                    "Code": { "ZipFile": SOURCE },
                },
            }),
        );
        json!({
            "AWSTemplateFormatVersion": "2010-09-09",
            "Resources": Value::Object(declared),
        })
        .to_string()
    };

    let (status_code, body) = create_stack(
        &router,
        stack,
        &waiting_on_the_alias(HEALTHY_VERSION, CREATED_DESCRIPTION),
        &acknowledging_iam(&[]),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    let created = stack_status(&router, case, stack).await;
    assert_eq!(
        created, "CREATE_COMPLETE",
        "case {case:?}: expected the create to complete, got {created:?}"
    );

    // The update breaks the alias AND asks for a change to the function that waits on it.
    let mut members = vec![
        member("StackName", stack),
        member(
            "TemplateBody",
            &waiting_on_the_alias("593", UPDATED_DESCRIPTION),
        ),
        member("DisableRollback", "true"),
    ];
    members.extend(acknowledging_iam(&[]));
    let (status_code, body) = call(&router, "UpdateStack", &members).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected UpdateStack to be accepted, got {status_code} {body}"
    );
    wait_for_the_transition().await;

    let reason = stack_status_reason(&router, case, stack).await;
    assert_eq!(
        reason,
        format!(
            "The following resource(s) failed to update: [{}]. ",
            alias_logical_id(0)
        ),
        "case {case:?}: expected only the resource that failed to be named, got {reason:?}"
    );
    // The dependent reports nothing of its own: the update never reached it.
    let sequence: Vec<String> = resource_event_sequence(&router, case, stack, "Fn1")
        .await
        .into_iter()
        .filter(|status| status.starts_with("UPDATE_"))
        .collect();
    assert!(
        sequence.is_empty(),
        "case {case:?}: expected the dependent to report no transition of its own, got \
         {sequence:?}"
    );
    // ...and nothing was asked of AWS Lambda for it: the function it holds is unchanged.
    let described = function_description(&router, &function_name(stack, 1)).await;
    assert_eq!(
        described.as_deref(),
        Some(CREATED_DESCRIPTION),
        "case {case:?}: expected the dependent's own configuration to be left as it was, got \
         {described:?}"
    );
}

/// The SAM template the version rows deploy: one `AWS::Serverless::Function` whose
/// `AutoPublishAlias` comes from the `Globals` section, with the code this revision declares.
///
/// `names_the_function` is whether the declaration states a `FunctionName`: a SAM template
/// usually does not, and the function then carries the name CloudFormation generates for it.
fn sam_template(stack: &str, source: &str, names_the_function: bool) -> String {
    let mut properties = serde_json::Map::new();
    if names_the_function {
        properties.insert(
            "FunctionName".to_string(),
            Value::String(format!("{stack}-fn")),
        );
    }
    properties.insert("InlineCode".to_string(), Value::String(source.to_string()));
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Transform": "AWS::Serverless-2016-10-31",
        "Globals": {
            "Function": {
                "Runtime": RUNTIME,
                "Handler": HANDLER,
                "AutoPublishAlias": ALIAS_NAME,
            },
        },
        "Resources": {
            "Fn": {
                "Type": "AWS::Serverless::Function",
                "Properties": Value::Object(properties),
            },
        },
    })
    .to_string()
}

/// The physical id the stack published for one of its logical resources.
async fn physical_id_of(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> String {
    let (status_code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStackResources to answer, got {status_code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .find(|entry| xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id))
        .and_then(|entry| xml_text(entry, "PhysicalResourceId"))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the stack to publish {logical_id:?}")
        })
}

/// Every version AWS Lambda itself reports for `function`, in the order it answers them.
async fn published_versions(router: &MultiServiceRouter, function: &str) -> Vec<String> {
    let answered = lambda(
        router,
        "GET",
        &format!("/2015-03-31/functions/{function}/versions"),
        &[],
        &[],
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "expected ListVersionsByFunction({function}) to answer, got {} with body {}",
        answered.status, answered.body
    );
    answered
        .body
        .get("Versions")
        .and_then(Value::as_array)
        .map(|versions| {
            versions
                .iter()
                .filter_map(|entry| entry.get("Version").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The SAM template whose code comes from a template PARAMETER: what a deployment pipeline
/// does when the handler's source (or the asset it points at) is what each deploy restates.
///
/// `carried_by` is which intrinsic carries the parameter into the code — the `Ref` to it, the
/// parameter interpolated into an `Fn::Sub`'s source string, or the same through `Fn::Sub`'s
/// other documented form, `[ String, { Name: Value } ]`.
fn parameterised_sam_template(stack: &str, carried_by: CarriedBy) -> String {
    let body = "def handler(event, context):\n    return {'build': '${Source}'}\n";
    let code = match carried_by {
        CarriedBy::Reference => json!({ "Ref": "Source" }),
        CarriedBy::Substitution => json!({ "Fn::Sub": body }),
        CarriedBy::SubstitutionWithVariables => json!({
            "Fn::Sub": [
                "def handler(event, context):\n    return {'build': '${Source}', 'at': '${Where}'}\n",
                { "Where": { "Ref": "AWS::Region" } },
            ]
        }),
    };
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Transform": "AWS::Serverless-2016-10-31",
        "Parameters": { "Source": { "Type": "String" } },
        "Globals": {
            "Function": {
                "Runtime": RUNTIME,
                "Handler": HANDLER,
                "AutoPublishAlias": ALIAS_NAME,
            },
        },
        "Resources": {
            "Fn": {
                "Type": "AWS::Serverless::Function",
                "Properties": {
                    "FunctionName": format!("{stack}-fn"),
                    "InlineCode": code,
                },
            },
        },
    })
    .to_string()
}

/// Which intrinsic a row carries the code's parameter into the declaration with.
#[derive(Debug, Clone, Copy)]
enum CarriedBy {
    Reference,
    Substitution,
    SubstitutionWithVariables,
}

/// RULE (the same rule, reached through a parameter): the snapshot each generated
/// `AWS::Lambda::Version` resource stands for is the function's code as the parameter values in
/// force resolve it, so an update that changes only a parameter the code reads publishes the
/// next version and moves the alias onto it.
///
/// The rows differ along which intrinsic carries the parameter into the code, and along
/// whether the update restates the template or reuses the one the stack holds.
#[tokio::test]
async fn a_sam_code_change_carried_by_a_parameter_publishes_the_next_version() {
    let rows: &[(&str, &str, CarriedBy, bool)] = &[
        (
            "the code read by a Ref, the stack's own template reused",
            "sam-autopublish-ref",
            CarriedBy::Reference,
            true,
        ),
        (
            "the code interpolated by an Fn::Sub, the template restated",
            "sam-autopublish-sub",
            CarriedBy::Substitution,
            false,
        ),
        (
            "the code interpolated by an Fn::Sub that supplies variables of its own",
            "sam-autopublish-sub-vars",
            CarriedBy::SubstitutionWithVariables,
            true,
        ),
    ];
    for (case, stack, carried_by, reuses_the_template) in rows {
        let (router, _engine) = router_with_engine();
        let function = format!("{stack}-fn");
        let first = "first";
        let revised = "second";

        let mut members = acknowledging_iam(&[member(
            "Capabilities.member.2",
            "CAPABILITY_AUTO_EXPAND",
        )]);
        members.push(member("Parameters.member.1.ParameterKey", "Source"));
        members.push(member("Parameters.member.1.ParameterValue", first));
        let (status_code, body) = create_stack(
            &router,
            stack,
            &parameterised_sam_template(stack, *carried_by),
            &members,
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let created = stack_status(&router, case, stack).await;
        assert_eq!(
            created, "CREATE_COMPLETE",
            "case {case:?}: expected the create to complete, got {created:?}"
        );
        let versions = published_versions(&router, &function).await;
        assert_eq!(
            versions,
            vec!["$LATEST".to_string(), "1".to_string()],
            "case {case:?}: expected the create to publish version 1, got {versions:?}"
        );

        // The update restates the parameter the code reads, and nothing else.
        let mut members = vec![
            member("StackName", stack),
            member("Parameters.member.1.ParameterKey", "Source"),
            member("Parameters.member.1.ParameterValue", revised),
        ];
        if *reuses_the_template {
            members.push(member("UsePreviousTemplate", "true"));
        } else {
            members.push(member(
                "TemplateBody",
                &parameterised_sam_template(stack, *carried_by),
            ));
        }
        members.extend(acknowledging_iam(&[member(
            "Capabilities.member.2",
            "CAPABILITY_AUTO_EXPAND",
        )]));
        let (status_code, body) = call(&router, "UpdateStack", &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected UpdateStack to be accepted, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let settled = stack_status(&router, case, stack).await;
        assert_eq!(
            settled, "UPDATE_COMPLETE",
            "case {case:?}: expected the update to complete, got {settled:?}"
        );

        let versions = published_versions(&router, &function).await;
        assert_eq!(
            versions,
            vec!["$LATEST".to_string(), "1".to_string(), "2".to_string()],
            "case {case:?}: expected the changed code to publish version 2 and keep version \
             1, got {versions:?}"
        );
        let invoked = alias_version(&router, &function).await;
        assert_eq!(
            invoked.as_deref(),
            Some("2"),
            "case {case:?}: expected the alias to move onto version 2, got {invoked:?}"
        );
    }
}

/// One version row: the code the update deploys instead, whether the template names the
/// function, and what AWS Lambda must then hold.
struct Published {
    case: &'static str,
    stack: &'static str,
    updated_source: &'static str,
    names_the_function: bool,
    expected_versions: &'static [&'static str],
    expected_alias: &'static str,
}

/// RULE: `AutoPublishAlias` makes each snapshot of a function's code an
/// `AWS::Lambda::Version`
/// resource of its own, so an update that changes the code publishes the NEXT version in AWS
/// Lambda, retains the one it published before and moves the alias onto the new one.
///
/// The rows differ along how the update changes the code (the handler's own source, and the
/// same source deployed again, which changes nothing and so publishes nothing) and along
/// whether the template NAMES the function or leaves CloudFormation to generate its name.
#[tokio::test]
async fn a_sam_code_change_publishes_the_next_version_and_moves_the_alias() {
    let rows: &[Published] = &[
        Published {
            case: "a changed handler body, the function named by the template",
            stack: "sam-autopublish-changed",
            updated_source: "def handler(event, context):\n    return {'build': 2}\n",
            names_the_function: true,
            expected_versions: &["$LATEST", "1", "2"],
            expected_alias: "2",
        },
        Published {
            case: "a changed handler body, the function name generated",
            stack: "sam-autopublish-generated",
            updated_source: "def handler(event, context):\n    return {'build': 2}\n",
            names_the_function: false,
            expected_versions: &["$LATEST", "1", "2"],
            expected_alias: "2",
        },
        Published {
            case: "the same code deployed again",
            stack: "sam-autopublish-same",
            updated_source: SOURCE,
            names_the_function: true,
            expected_versions: &["$LATEST", "1"],
            expected_alias: "1",
        },
    ];
    for row in rows {
        let Published {
            case,
            stack,
            updated_source,
            names_the_function,
            expected_versions,
            expected_alias,
        } = row;
        let (router, _engine) = router_with_engine();
        let (status_code, body) = create_stack(
            &router,
            stack,
            &sam_template(stack, SOURCE, *names_the_function),
            &acknowledging_iam(&[member("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND")]),
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        let created = stack_status(&router, case, stack).await;
        assert_eq!(
            created, "CREATE_COMPLETE",
            "case {case:?}: expected the create to complete, got {created:?}"
        );
        // The function AWS Lambda holds is the one the stack published a handle for.
        let function = physical_id_of(&router, case, stack, "Fn").await;
        let versions = published_versions(&router, &function).await;
        assert_eq!(
            versions,
            vec!["$LATEST".to_string(), "1".to_string()],
            "case {case:?}: expected the create to publish version 1, got {versions:?}"
        );

        let mut members = vec![
            member("StackName", stack),
            member(
                "TemplateBody",
                &sam_template(stack, updated_source, *names_the_function),
            ),
        ];
        members.extend(acknowledging_iam(&[member(
            "Capabilities.member.2",
            "CAPABILITY_AUTO_EXPAND",
        )]));
        let (status_code, body) = call(&router, "UpdateStack", &members).await;
        let updated_the_stack = status_code == 200;
        assert!(
            updated_the_stack || body.contains("No updates are to be performed"),
            "case {case:?}: expected UpdateStack to be accepted or refused as a no-op, got \
             {status_code} {body}"
        );
        if updated_the_stack {
            wait_for_the_transition().await;
            let settled = stack_status(&router, case, stack).await;
            assert_eq!(
                settled, "UPDATE_COMPLETE",
                "case {case:?}: expected the update to complete, got {settled:?}"
            );
        }

        let versions = published_versions(&router, &function).await;
        let expected: Vec<String> = expected_versions.iter().map(|v| (*v).to_string()).collect();
        assert_eq!(
            versions, expected,
            "case {case:?}: expected AWS Lambda to hold {expected:?}, got {versions:?}"
        );
        let invoked = alias_version(&router, &function).await;
        assert_eq!(
            invoked.as_deref(),
            Some(*expected_alias),
            "case {case:?}: expected the alias to invoke {expected_alias:?}, got {invoked:?}"
        );
    }
}
