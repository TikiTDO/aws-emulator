// SPDX-License-Identifier: CC0-1.0

//! WHO may reach the operations of this service, driven end to end over the real wire
//! path (an awsJson1_0 request through the multi-service router into the mounted service),
//! so what is asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: every operation whose model declares `AccessDeniedException`
//! evaluates the identity-based IAM policies in force on the principal its credentials
//! name against `arc-region-switch:<OperationName>`, on the resource the request
//! ADDRESSES, BEFORE it reads or writes anything — so a request no statement allows (IAM's
//! default deny), or one an explicit `Deny` covers, is answered HTTP 403
//! `AccessDeniedException` whatever state the plan is in, and nothing is created, changed
//! or reported; a request a matching `Allow` covers reaches the operation's own answer,
//! including the operation's own refusal.
//!
//! That ORDER is the point of the first test: a plan another execution is already running
//! would answer an authorized caller `IllegalStateException` (400), and an unauthorized one
//! is still answered 403 — authorization is not reached through the state machine.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME
//! shared engine — the stores those services' own handlers write — so the documents under
//! test are the ones `iam:PutRolePolicy` stores and the principal a refusal blames is the
//! one `sts:GetCallerIdentity` answers for the key the request signed with.

use serde_json::{json, Value};

use crate::switch_support::{
    allow, approval_step, error_code, error_message, given_role_policy, given_session,
    plan_with_steps, poll_until_state, session_arn, start_execution, Emulator, ACCOUNT_KEY_ID,
    SESSION_KEY_ID,
};

/// The IAM action prefix of this service: its `arnNamespace`
/// (`smithy_query(query="service_info", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`).
const ACTION_PREFIX: &str = "arc-region-switch";

/// Which of IAM's two refusals a row expects.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    /// The default deny of a request no statement allows.
    DefaultDeny,
    /// The explicit deny of a request a `Deny` statement covers.
    ExplicitDeny,
}

impl Refusal {
    /// The clause IAM closes the refusal with.
    fn reason(self, action: &str) -> String {
        match self {
            Self::DefaultDeny => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// The sentence IAM refuses a request with: who, which action, on which resource, and
/// which of its two rules refused.
fn not_authorized(principal: &str, action: &str, resource: &str, refusal: Refusal) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {}",
        refusal.reason(action)
    )
}

/// The documents a row puts in force on the calling role, and which of IAM's refusals the
/// request then meets — `None` for a request a matching `Allow` permits.
fn documents_in_force(action: &str, plan_arn: &str) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    vec![
        ("no policy at all", None, Some(Refusal::DefaultDeny)),
        (
            "an allow of another action of this service, but not this one",
            Some(allow(json!("arc-region-switch:ListPlans"), json!("*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of every action of another service",
            Some(allow(json!("route53:*"), json!("*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of this action on ANOTHER plan",
            Some(allow(
                json!(action),
                json!(format!("{plan_arn}-not-this-one")),
            )),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": "arc-region-switch:*", "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            "an allow of exactly this action on every resource",
            Some(allow(json!(action), json!("*"))),
            None,
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow
            // is not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()), json!("*"))),
            None,
        ),
        (
            "an allow of this action on exactly this plan",
            Some(allow(json!(action), json!(plan_arn))),
            None,
        ),
        (
            "an allow of every action of this service",
            Some(allow(json!("arc-region-switch:*"), json!("*"))),
            None,
        ),
        (
            "an allow of every action",
            Some(allow(json!("*"), json!("*"))),
            None,
        ),
    ]
}

/// RULE: the authorization decision is taken BEFORE the plan's own state is looked at, so
/// the answer an unprivileged caller gets does not depend on whether another execution
/// happens to be running — and an authorized caller meets the operation's own answer.
#[tokio::test]
async fn authorization_is_decided_before_the_execution_state_precondition() {
    let action = format!("{ACTION_PREFIX}:StartPlanExecution");
    let rows = documents_in_force(&action, "arn:aws:arc-region-switch::0:plan/x:aaaaaa").len();
    // The rows differ along both halves of the rule: the document in force on the caller,
    // and whether the plan is free or already held by an execution the state machine would
    // refuse a second start for. Each row gets its own emulator and its own plan, so no
    // row's answer depends on what another row left behind.
    for held in [false, true] {
        for row in 0..rows {
            let emulator = Emulator::with_identity();
            given_session(&emulator.engine);
            let plan_arn = plan_with_steps(
                &emulator,
                "a guarded plan",
                "guarded",
                vec![approval_step("Approve")],
            )
            .await;
            if held {
                // The plan is held: an AUTHORIZED second start is the state machine's own
                // `IllegalStateException`, which is exactly what an unauthorized one must
                // NOT be answered.
                let execution_id =
                    start_execution(&emulator, "a held plan", &plan_arn, "us-west-2").await;
                poll_until_state(
                    &emulator,
                    "a held plan",
                    &plan_arn,
                    &execution_id,
                    &["pendingManualApproval"],
                )
                .await;
            }

            let mut in_force = documents_in_force(&action, &plan_arn);
            let (document_case, document, refusal) = in_force.remove(row);
            let case = format!("{document_case}, plan held: {held}");
            given_role_policy(&emulator.engine, document.as_ref());
            let (status, body) = emulator
                .call_as(
                    SESSION_KEY_ID,
                    "StartPlanExecution",
                    &json!({
                        "planArn": plan_arn,
                        "targetRegion": "us-west-2",
                        "action": "activate",
                    }),
                )
                .await;
            match (refusal, held) {
                (Some(refusal), _) => {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (403, "AccessDeniedException"),
                        "case {case:?}: expected 403 AccessDeniedException, got {status} with \
                         {body}"
                    );
                    assert_eq!(
                        error_message(&body),
                        not_authorized(&session_arn(), &action, &plan_arn, refusal),
                        "case {case:?}: expected IAM's refusal wording, got {body}"
                    );
                }
                (None, true) => {
                    assert_eq!(
                        (status, error_code(&body).as_str()),
                        (400, "IllegalStateException"),
                        "case {case:?}: an authorized caller meets the operation's own \
                         refusal, got {status} with {body}"
                    );
                }
                (None, false) => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: an authorized caller is served, got {status} with {body}"
                    );
                    assert!(
                        body["executionId"].is_string(),
                        "case {case:?}: expected a started execution, got {body}"
                    );
                }
            }
        }
    }
}

/// RULE: a refused call changes nothing — the plan records no execution for it.
#[tokio::test]
async fn a_refused_start_records_no_execution() {
    let emulator = Emulator::with_identity();
    given_session(&emulator.engine);
    let case = "a policy-less caller";
    let plan_arn = plan_with_steps(&emulator, case, "untouched", vec![approval_step("Approve")]).await;
    given_role_policy(&emulator.engine, None);

    let (status, body) = emulator
        .call_as(
            SESSION_KEY_ID,
            "StartPlanExecution",
            &json!({"planArn": plan_arn, "targetRegion": "us-west-2", "action": "activate"}),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (403, "AccessDeniedException"),
        "case {case:?}: expected 403 AccessDeniedException, got {status} with {body}"
    );

    let (status, page) = emulator
        .call("ListPlanExecutions", &json!({"planArn": plan_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPlanExecutions expected 200, got {status} with {page}"
    );
    assert_eq!(
        page["items"],
        json!([]),
        "case {case:?}: a refused start must record no execution, got {}",
        page["items"]
    );
    let (status, plans) = emulator.call("ListPlans", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListPlans expected 200, got {status} with {plans}"
    );
    assert!(
        plans["plans"][0]["activePlanExecution"].is_null(),
        "case {case:?}: a refused start must leave the plan free, got {}",
        plans["plans"][0]
    );
}

/// One operation under the gate: its name and the request that reaches it.
struct Guarded {
    operation: &'static str,
    request: fn(&str) -> Value,
}

/// Every implemented operation whose model declares `AccessDeniedException`
/// (`smithy_query(query="digest", model_path="services/service-com-amazonaws-arcregionswitch-arcregionswitch/smithy-model/com-amazonaws-arcregionswitch-arcregionswitch.json")`),
/// with a request each would otherwise answer. The gate runs before anything is read, so
/// an execution id nothing holds is enough.
const GUARDED: &[Guarded] = &[
    Guarded {
        operation: "ApprovePlanExecutionStep",
        request: |plan| {
            json!({
                "planArn": plan,
                "executionId": "us-west-2/0123456789abcdef",
                "stepName": "Approve",
                "approval": "approve",
            })
        },
    },
    Guarded {
        operation: "GetPlanEvaluationStatus",
        request: |plan| json!({"planArn": plan}),
    },
    Guarded {
        operation: "GetPlanExecution",
        request: |plan| json!({"planArn": plan, "executionId": "us-west-2/0123456789abcdef"}),
    },
    Guarded {
        operation: "GetPlanInRegion",
        request: |plan| json!({"arn": plan}),
    },
    Guarded {
        operation: "ListPlanExecutionEvents",
        request: |plan| json!({"planArn": plan, "executionId": "us-west-2/0123456789abcdef"}),
    },
    Guarded {
        operation: "ListPlanExecutions",
        request: |plan| json!({"planArn": plan}),
    },
    Guarded {
        operation: "ListRoute53HealthChecks",
        request: |plan| json!({"arn": plan}),
    },
    Guarded {
        operation: "ListRoute53HealthChecksInRegion",
        request: |plan| json!({"arn": plan}),
    },
    Guarded {
        operation: "StartPlanExecution",
        request: |plan| json!({"planArn": plan, "targetRegion": "us-west-2", "action": "activate"}),
    },
    Guarded {
        operation: "UpdatePlanExecution",
        request: |plan| {
            json!({
                "planArn": plan,
                "executionId": "us-west-2/0123456789abcdef",
                "action": "pause",
            })
        },
    },
];

/// RULE: the gate covers every operation that models the refusal, each judged against its
/// OWN action name and the plan its own input addresses; and the account's own published
/// credential — which holds no IAM policies — is not bounded by it.
#[tokio::test]
async fn every_operation_that_models_the_refusal_takes_the_decision() {
    let emulator = Emulator::with_identity();
    given_session(&emulator.engine);
    let plan_arn = plan_with_steps(&emulator, "guarded", "guarded", vec![approval_step("Approve")]).await;
    given_role_policy(&emulator.engine, None);

    for Guarded { operation, request } in GUARDED {
        let body = request(&plan_arn);
        let action = format!("{ACTION_PREFIX}:{operation}");

        let (status, refused) = emulator.call_as(SESSION_KEY_ID, operation, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (403, "AccessDeniedException"),
            "case {operation:?}: expected 403 AccessDeniedException, got {status} with {refused}"
        );
        assert_eq!(
            error_message(&refused),
            not_authorized(&session_arn(), &action, &plan_arn, Refusal::DefaultDeny),
            "case {operation:?}: expected IAM's refusal naming this operation's action, got \
             {refused}"
        );

        // The account's own credential holds no IAM policies, so it is not refused by this
        // gate: whatever it gets back, it is not this refusal.
        let (status, served) = emulator.call_as(ACCOUNT_KEY_ID, operation, &body).await;
        assert_ne!(
            (status, error_code(&served).as_str()),
            (403, "AccessDeniedException"),
            "case {operation:?}: the account's own credential must not be denied, got \
             {status} with {served}"
        );
    }
}
