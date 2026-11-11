// SPDX-License-Identifier: CC0-1.0

//! What EXECUTING a `CREATE` change set does to the stack it left in review, end to end over
//! the awsQuery wire.
//!
//! RULE: executing a change set applies its stored template and configuration to the stack it
//! belongs to and drives that stack out of `REVIEW_IN_PROGRESS` to the terminal status of the
//! operation it opens, exactly as `CreateStack` does — `CREATE_COMPLETE` once every declared
//! resource is created in the service that owns it, and a resource the owning service refuses
//! unwound as the execution's failure setting says: rolled back to `ROLLBACK_COMPLETE` by
//! default, left `CREATE_FAILED` with `DisableRollback` true and what the create had made
//! still in place when rollback is disabled (`ExecuteChangeSet$DisableRollback`, or the change
//! set's own `OnStackFailure=DO_NOTHING`, which the model documents as the same thing), and
//! taken away with its resources by `OnStackFailure=DELETE`. The stack a failure is parked on
//! carries the executed template and is a stack the next operation — an update, another change
//! set, a requested rollback, a delete — acts on.
//!
//! The recorded trace this suite was written for is ONE row of the tables below. The rows of
//! the first differ along what the rule generalises over: whether a declared resource is
//! refused at all, how many resources the template declares and whether any of them is
//! created before the refusal, and which of the documented failure settings is in force and
//! on which call it is stated. The rows of the second span the operations a parked stack
//! takes.
//!
//! Every read goes through the real request path — form-encoded body in, XML envelope out —
//! and each resource is read back through the OWNING service's own wire (Systems Manager for
//! an `AWS::SSM::Parameter`, IAM for an `AWS::IAM::Role`), so what is asserted is what a
//! client sees rather than a status word the stack published.
//!
//! `change_set_lifecycle.rs` holds the COMPLETING execution of a change set (the plan, the
//! changes it reports, the template that becomes the stack's) and
//! `change_set_import_and_drift.rs` an execution a name collision fails; what is new here is
//! the FAILURE setting in force on the execution — `ExecuteChangeSet$DisableRollback` beside
//! the change set's own `OnStackFailure` — and what the stack a failure is parked on then
//! takes, so the overlap in the rolled-back rows is deliberate: they are the contrast the
//! parked rows are read against.

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, resource_status, status, USER_INITIATED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::create_change_set;
use crate::stack_lifecycle::{
    acknowledging_iam, call, describe_stack, get_template, iam, member, resource_event_reason,
    resource_event_statuses, router, wait_for_the_transition, xml_text,
};
use crate::update_rollback_recovery::{parameter, parameter_value, template};

/// The change set every case of the first table creates and executes.
const CHANGE_SET: &str = "the-plan";

/// The value the template's parameter declares, so a case can tell the deployed parameter
/// from one another operation wrote.
const DEPLOYED: &str = "deployed";

/// A trust policy IAM accepts: an IAM JSON policy document carrying a `Statement`.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "glue.amazonaws.com"},
            "Action": "sts:AssumeRole"
        }]
    })
}

/// An `AWS::IAM::Role` declaration under a name of the template's own choosing: the resource
/// whose create IAM refuses when that account-global name is already taken.
fn role(name: &str) -> Value {
    json!({
        "Type": "AWS::IAM::Role",
        "Properties": {
            "RoleName": name,
            "AssumeRolePolicyDocument": trust_policy()
        }
    })
}

/// What a row's change set declares — the dimension that decides whether the execution gets
/// a resource of its own made before the one that fails it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Declares {
    /// A parameter Systems Manager accepts, declared first, and the role second: the logical
    /// ids order the two whether the template reader yields declaration order or sorted
    /// order.
    AParameterAndARole,
    /// The role alone: nothing is created before the refusal.
    OnlyARole,
}

impl Declares {
    /// The template body, over this row's parameter name and role name.
    fn body(self, param: &str, role_name: &str) -> String {
        match self {
            Declares::AParameterAndARole => template(json!({
                "AParam": parameter(param, DEPLOYED),
                "ZRole": role(role_name),
            })),
            Declares::OnlyARole => template(json!({"ZRole": role(role_name)})),
        }
    }

    /// The logical ids the template declares, in the order it declares them.
    fn logical_ids(self) -> &'static [&'static str] {
        match self {
            Declares::AParameterAndARole => &["AParam", "ZRole"],
            Declares::OnlyARole => &["ZRole"],
        }
    }

    /// Whether a completed execution leaves a parameter in Parameter Store.
    fn declares_the_parameter(self) -> bool {
        self == Declares::AParameterAndARole
    }
}

/// What the executed change set must have left behind once the clock owes its outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settles {
    /// `CREATE_COMPLETE`: every declared resource was created.
    Completed,
    /// `ROLLBACK_COMPLETE`: the refusal was rolled back and what the create had made is gone.
    RolledBack,
    /// `CREATE_FAILED`: the refusal was kept, and so is what the create had made.
    Parked,
    /// The stack itself is gone, with the resources the create had made.
    StackGone,
}

/// One row of the execution table.
struct Row<'a> {
    case: &'a str,
    /// Unique per row: it names the stack, and seeds the parameter and role names, so no two
    /// rows can collide.
    stack: &'a str,
    declares: Declares,
    /// Whether an IAM role of the template's name already exists when the change set is
    /// executed, which is what makes the owning service refuse the resource.
    name_taken: bool,
    /// `OnStackFailure`, exactly as `CreateChangeSet` carries it.
    on_stack_failure: Option<&'a str>,
    /// `DisableRollback`, exactly as `ExecuteChangeSet` carries it.
    disable_rollback: Option<&'a str>,
    settles_to: Settles,
}

/// The class the rule covers: any `CREATE` change set executed against the stack it brought
/// into existence, whatever its template declares and whatever it says about a failure.
fn rows() -> Vec<Row<'static>> {
    vec![
        Row {
            case: "every declared resource is created: the execution completes",
            stack: "xcs-complete",
            declares: Declares::AParameterAndARole,
            name_taken: false,
            on_stack_failure: None,
            disable_rollback: None,
            settles_to: Settles::Completed,
        },
        Row {
            case: "a refused resource with nothing stated: the create is rolled back",
            stack: "xcs-rollback",
            declares: Declares::AParameterAndARole,
            name_taken: true,
            on_stack_failure: None,
            disable_rollback: None,
            settles_to: Settles::RolledBack,
        },
        Row {
            case: "ExecuteChangeSet DisableRollback=true parks the failed create",
            stack: "xcs-parked",
            declares: Declares::AParameterAndARole,
            name_taken: true,
            on_stack_failure: None,
            disable_rollback: Some("true"),
            settles_to: Settles::Parked,
        },
        Row {
            case: "the change set's OnStackFailure=DO_NOTHING spells the same thing",
            stack: "xcs-do-nothing",
            declares: Declares::AParameterAndARole,
            name_taken: true,
            on_stack_failure: Some("DO_NOTHING"),
            disable_rollback: None,
            settles_to: Settles::Parked,
        },
        Row {
            case: "ExecuteChangeSet DisableRollback=false asks for the rollback",
            stack: "xcs-rollback-stated",
            declares: Declares::AParameterAndARole,
            name_taken: true,
            on_stack_failure: None,
            disable_rollback: Some("false"),
            settles_to: Settles::RolledBack,
        },
        Row {
            case: "the only declared resource is refused, and the failure is parked",
            stack: "xcs-parked-alone",
            declares: Declares::OnlyARole,
            name_taken: true,
            on_stack_failure: None,
            disable_rollback: Some("true"),
            settles_to: Settles::Parked,
        },
        Row {
            case: "the change set's OnStackFailure=DELETE takes the stack away",
            stack: "xcs-deleted",
            declares: Declares::AParameterAndARole,
            name_taken: true,
            on_stack_failure: Some("DELETE"),
            disable_rollback: None,
            settles_to: Settles::StackGone,
        },
    ]
}

/// `ExecuteChangeSet`, with whatever else the case states on the call.
///
/// `change_set_lifecycle::execute_change_set` is the shared caller for an execution that
/// states nothing else; the member this suite's rows vary (`DisableRollback`) is stated on
/// THIS call, which that helper takes no room for.
async fn execute_change_set(
    router: &MultiServiceRouter,
    change_set: &str,
    stack: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![
        member("ChangeSetName", change_set),
        member("StackName", stack),
    ];
    members.extend_from_slice(extra);
    call(router, "ExecuteChangeSet", &members).await
}

/// Does IAM hold this role?
async fn role_exists(router: &MultiServiceRouter, name: &str) -> bool {
    iam(router, "GetRole", &[("RoleName", name)]).await.0 == 200
}

/// Take the account-global role name before the stack asks for it, so the owning service's own
/// refusal is what fails the execution.
async fn take_the_role_name(case: &str, router: &MultiServiceRouter, name: &str) {
    let (http_status, body) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", &trust_policy().to_string()),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: the role name must be taken first, got {http_status} {body}"
    );
}

#[tokio::test]
async fn an_executed_create_change_set_settles_the_stack_it_left_in_review() {
    for row in rows() {
        let router = router();
        let param = format!("/{}/p", row.stack);
        let role_name = format!("{}-role", row.stack);
        let body = row.declares.body(&param, &role_name);
        if row.name_taken {
            take_the_role_name(row.case, &router, &role_name).await;
        }

        // The plan: a CREATE change set for a stack that does not exist yet, which is what
        // brings that stack into existence in review.
        let mut created_with = vec![
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", &body),
        ];
        if row.name_taken {
            // "Set to true to disable pre-deployment validations": with the name already
            // taken, the pre-deployment stage would fail the CHANGE SET itself
            // (`change_set_early_validation.rs` is the suite that holds it to that), and
            // what this suite is about is the EXECUTION — so the row turns that stage off
            // and leaves the collision to the deployment, which is what the member is for.
            created_with.push(member("DisableValidation", "true"));
        }
        if let Some(on_stack_failure) = row.on_stack_failure {
            created_with.push(member("OnStackFailure", on_stack_failure));
        }
        let (http_status, created) = create_change_set(
            &router,
            row.stack,
            CHANGE_SET,
            &acknowledging_iam(&created_with),
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateChangeSet to succeed, got {http_status} {created}",
            row.case
        );
        let stack_id = xml_text(&created, "StackId").expect("the change set's StackId");
        // A change set is executable once its own creation has completed, which the clock
        // owes a moment after the call that started it.
        wait_for_the_transition().await;

        // Before the execution the stack is the review record the change set made, with no
        // deployment of its own: the plan has not been applied yet.
        let (http_status, reviewed) = describe_stack(&router, row.stack).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: the stack the change set created is readable, got {http_status} \
             {reviewed}",
            row.case
        );
        assert_eq!(
            (
                xml_text(&reviewed, "StackStatus").as_deref(),
                xml_text(&reviewed, "StackStatusReason").as_deref(),
                xml_text(&reviewed, "DisableRollback").as_deref(),
            ),
            (
                Some(status::REVIEW_IN_PROGRESS),
                Some(USER_INITIATED),
                Some("false")
            ),
            "case {:?}: a change set that has not been executed leaves the stack in review, \
             got {reviewed}",
            row.case
        );

        // Executing it is the call that applies the plan.
        let mut executed_with = Vec::new();
        if let Some(disable_rollback) = row.disable_rollback {
            executed_with.push(member("DisableRollback", disable_rollback));
        }
        let (http_status, executed) =
            execute_change_set(&router, CHANGE_SET, row.stack, &executed_with).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected ExecuteChangeSet to succeed, got {http_status} {executed}",
            row.case
        );
        wait_for_the_transition().await;

        let (http_status, settled) = describe_stack(&router, row.stack).await;
        if row.settles_to == Settles::StackGone {
            assert_eq!(
                http_status, 400,
                "case {:?}: the failed create must have taken the stack away, got \
                 {http_status} {settled}",
                row.case
            );
            assert_eq!(
                parameter_value(&router, &param).await,
                None,
                "case {:?}: the parameter the create had made goes with the stack",
                row.case
            );
            // The role that was already there was never this stack's to remove.
            assert!(
                role_exists(&router, &role_name).await,
                "case {:?}: the role the execution could not create must survive",
                row.case
            );
            continue;
        }

        assert_eq!(
            http_status, 200,
            "case {:?}: expected DescribeStacks to answer 200, got {http_status} {settled}",
            row.case
        );
        let expected_status = match row.settles_to {
            Settles::Completed => status::CREATE_COMPLETE,
            Settles::RolledBack => status::ROLLBACK_COMPLETE,
            Settles::Parked => status::CREATE_FAILED,
            Settles::StackGone => unreachable!("answered above"),
        };
        assert_eq!(
            xml_text(&settled, "StackStatus").as_deref(),
            Some(expected_status),
            "case {:?}: expected the execution to settle the stack in {expected_status}, got \
             {settled}",
            row.case
        );
        assert_eq!(
            xml_text(&settled, "StackId").as_deref(),
            Some(stack_id.as_str()),
            "case {:?}: the settled stack is the record the change set created, got {settled}",
            row.case
        );
        // The STACK's reason is the summary sentence naming the resources the operation could
        // not carry through, and nothing for one that completed; the owning service's own
        // refusal is reported on that resource's event instead.
        let expected_reason = match row.settles_to {
            Settles::Completed => None,
            _ => Some(resource_failure_summary(failed_action::CREATE, ["ZRole"])),
        };
        assert_eq!(
            xml_text(&settled, "StackStatusReason"),
            expected_reason,
            "case {:?}: expected the stack reason to be the summary of what failed, got \
             {settled}",
            row.case
        );
        // "Preserve the state of previously provisioned resources when an operation fails":
        // the flag in force on the stack is the one the execution ran under.
        let expected_flag = if row.settles_to == Settles::Parked {
            "true"
        } else {
            "false"
        };
        assert_eq!(
            xml_text(&settled, "DisableRollback").as_deref(),
            Some(expected_flag),
            "case {:?}: expected DisableRollback {expected_flag}, got {settled}",
            row.case
        );

        // The executed change set's template became the stack's, so the stack has a template
        // of its own from here on.
        let (http_status, template_read) = get_template(&router, row.stack, None).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: the executed stack's template is readable, got {http_status} \
             {template_read}",
            row.case
        );
        assert_eq!(
            xml_text(&template_read, "TemplateBody").as_deref(),
            Some(body.as_str()),
            "case {:?}: the change set's template became the stack's, got {template_read}",
            row.case
        );

        // What the services that own the resources really hold: a completed execution and a
        // parked one leave the parameter they made, a rolled-back one removes it again.
        let kept = row.declares.declares_the_parameter()
            && matches!(row.settles_to, Settles::Completed | Settles::Parked);
        let value = parameter_value(&router, &param).await;
        assert_eq!(
            value.as_deref(),
            kept.then_some(DEPLOYED),
            "case {:?}: expected Parameter Store to hold {:?} for {param:?}, got {value:?}",
            row.case,
            kept.then_some(DEPLOYED)
        );
        // IAM holds a role of that name either way, for two different reasons: the one the
        // execution created where the name was free, and — where it was not — the one that
        // was already there, which no unwind of this stack may remove because this stack
        // never created it.
        let held_by_iam = role_exists(&router, &role_name).await;
        assert!(
            held_by_iam,
            "case {:?}: expected IAM to hold {role_name:?} \u{2014} {}",
            row.case,
            if row.name_taken {
                "the role that was already there must survive the failed execution"
            } else {
                "the role the execution created must be there"
            }
        );

        // The execution really walked the resources: the stack's event stream carries a
        // per-resource create for every resource the template declared, ending in the status
        // that resource reached — which is the only record of WHICH declared resource the
        // execution got to.
        for logical_id in row.declares.logical_ids() {
            let statuses =
                resource_event_statuses(&router, row.case, row.stack, logical_id).await;
            let expected_terminal = if *logical_id == "ZRole" && row.name_taken {
                resource_status::CREATE_FAILED
            } else {
                resource_status::CREATE_COMPLETE
            };
            assert!(
                statuses.contains(&resource_status::CREATE_IN_PROGRESS.to_string())
                    && statuses.contains(&expected_terminal.to_string()),
                "case {:?}: expected the execution's events for {logical_id} to carry \
                 {} then {expected_terminal}, got {statuses:?}",
                row.case,
                resource_status::CREATE_IN_PROGRESS
            );
        }
        // The service that owns the refused resource reported the refusal, and it is carried
        // on THAT resource's event rather than folded into the stack's summary.
        if row.name_taken {
            let on_the_resource =
                resource_event_reason(&router, row.stack, "ZRole", resource_status::CREATE_FAILED)
                    .await;
            assert!(
                on_the_resource.contains(&format!("Role with name {role_name} already exists.")),
                "case {:?}: expected IAM's own refusal on the failed resource's event, got \
                 {on_the_resource:?}",
                row.case
            );
        }
    }
}

/// What the next operation on a parked stack is, and what it must make of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NextOperation {
    /// `UpdateStack` with a template whose resources the owning services accept.
    AnUpdate,
    /// The same correction submitted as another change set and executed.
    AnotherChangeSet,
    /// `RollbackStack`: the stack has no stable state to go back to, so what the failed
    /// create had made is removed again.
    ARequestedRollback,
    /// `DeleteStack`.
    ADelete,
}

/// One row of the recovery table.
struct Recovery<'a> {
    case: &'a str,
    stack: &'a str,
    next: NextOperation,
    /// The status the next operation settles the stack in; `None` for a stack it takes away.
    settles_to: Option<&'a str>,
    /// What Parameter Store must hold for the parked create's parameter afterwards.
    parameter_after: Option<&'a str>,
}

/// The value the corrected template gives the parameter, so an operation that really ran is
/// told apart from one that only reported a status.
const CORRECTED: &str = "corrected";

fn recoveries() -> Vec<Recovery<'static>> {
    vec![
        Recovery {
            case: "an update with the corrected template",
            stack: "xcs-next-update",
            next: NextOperation::AnUpdate,
            settles_to: Some(status::UPDATE_COMPLETE),
            parameter_after: Some(CORRECTED),
        },
        Recovery {
            case: "the correction submitted as another change set",
            stack: "xcs-next-change-set",
            next: NextOperation::AnotherChangeSet,
            settles_to: Some(status::UPDATE_COMPLETE),
            parameter_after: Some(CORRECTED),
        },
        Recovery {
            case: "a rollback the client asks for",
            stack: "xcs-next-rollback",
            next: NextOperation::ARequestedRollback,
            settles_to: Some(status::ROLLBACK_COMPLETE),
            parameter_after: None,
        },
        Recovery {
            case: "a delete",
            stack: "xcs-next-delete",
            next: NextOperation::ADelete,
            settles_to: None,
            parameter_after: None,
        },
    ]
}

#[tokio::test]
async fn a_parked_change_set_create_takes_the_next_operation() {
    for row in recoveries() {
        let router = router();
        let param = format!("/{}/p", row.stack);
        let taken_role = format!("{}-role", row.stack);
        let free_role = format!("{}-role-free", row.stack);
        let parked_body = Declares::AParameterAndARole.body(&param, &taken_role);
        // The correction: the same parameter with another value, and a role name nothing
        // holds — so the operation that carries it out really creates a role.
        let corrected_body = template(json!({
            "AParam": parameter(&param, CORRECTED),
            "ZRole": role(&free_role),
        }));

        take_the_role_name(row.case, &router, &taken_role).await;
        let (http_status, created) = create_change_set(
            &router,
            row.stack,
            CHANGE_SET,
            &acknowledging_iam(&[
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", &parked_body),
                // The collision is left to the deployment, so what parks the stack is the
                // EXECUTION of the plan rather than its pre-deployment validation.
                member("DisableValidation", "true"),
            ]),
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateChangeSet to succeed, got {http_status} {created}",
            row.case
        );
        let stack_id = xml_text(&created, "StackId").expect("the change set's StackId");
        wait_for_the_transition().await;
        let (http_status, executed) = execute_change_set(
            &router,
            CHANGE_SET,
            row.stack,
            &[member("DisableRollback", "true")],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected ExecuteChangeSet to succeed, got {http_status} {executed}",
            row.case
        );
        wait_for_the_transition().await;
        let (_, parked) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&parked, "StackStatus").as_deref(),
            Some(status::CREATE_FAILED),
            "case {:?}: the failed create must be parked before the next operation, got \
             {parked}",
            row.case
        );

        // The next operation on that parked stack.
        match row.next {
            NextOperation::AnUpdate => {
                let (http_status, body) = call(
                    &router,
                    "UpdateStack",
                    &acknowledging_iam(&[
                        member("StackName", row.stack),
                        member("TemplateBody", &corrected_body),
                        // The parked stack is mid-change, so the fix-forward goes on asking
                        // for the failed state to be kept — which is what makes it an update
                        // of that state rather than one of a settled stack.
                        member("DisableRollback", "true"),
                    ]),
                )
                .await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: a parked stack must take an update, got {http_status} {body}",
                    row.case
                );
            }
            NextOperation::AnotherChangeSet => {
                let (http_status, body) = create_change_set(
                    &router,
                    row.stack,
                    "the-correction",
                    &acknowledging_iam(&[
                        member("ChangeSetType", "UPDATE"),
                        member("TemplateBody", &corrected_body),
                    ]),
                )
                .await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: a parked stack must take another change set, got {http_status} \
                     {body}",
                    row.case
                );
                wait_for_the_transition().await;
                let (http_status, body) =
                    execute_change_set(&router, "the-correction", row.stack, &[]).await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: expected the correction to be executable, got {http_status} \
                     {body}",
                    row.case
                );
            }
            NextOperation::ARequestedRollback => {
                let (http_status, body) =
                    call(&router, "RollbackStack", &[member("StackName", row.stack)]).await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: a parked stack must be rollback-able, got {http_status} {body}",
                    row.case
                );
            }
            NextOperation::ADelete => {
                let (http_status, body) =
                    call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
                assert_eq!(
                    http_status, 200,
                    "case {:?}: a parked stack must be deletable, got {http_status} {body}",
                    row.case
                );
            }
        }
        wait_for_the_transition().await;

        let (http_status, after) = describe_stack(&router, row.stack).await;
        match row.settles_to {
            Some(expected) => {
                assert_eq!(
                    http_status, 200,
                    "case {:?}: expected DescribeStacks to answer 200, got {http_status} \
                     {after}",
                    row.case
                );
                assert_eq!(
                    xml_text(&after, "StackStatus").as_deref(),
                    Some(expected),
                    "case {:?}: expected the next operation to settle the stack in {expected}, \
                     got {after}",
                    row.case
                );
                assert_eq!(
                    xml_text(&after, "StackId").as_deref(),
                    Some(stack_id.as_str()),
                    "case {:?}: the next operation acts on the SAME stack record, got {after}",
                    row.case
                );
            }
            None => assert_eq!(
                http_status, 400,
                "case {:?}: the deleted stack must be gone, got {http_status} {after}",
                row.case
            ),
        }

        // What the owning services really hold once that operation has run.
        let value = parameter_value(&router, &param).await;
        assert_eq!(
            value.as_deref(),
            row.parameter_after,
            "case {:?}: expected Parameter Store to hold {:?} for {param:?}, got {value:?}",
            row.case,
            row.parameter_after
        );
        let corrected_role = role_exists(&router, &free_role).await;
        let deployed_again = matches!(
            row.next,
            NextOperation::AnUpdate | NextOperation::AnotherChangeSet
        );
        assert_eq!(
            corrected_role, deployed_again,
            "case {:?}: expected the corrected role to be {}, got exists={corrected_role}",
            row.case,
            if deployed_again {
                "created by the operation that carried the correction"
            } else {
                "absent, nothing having deployed it"
            }
        );
    }
}
