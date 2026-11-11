// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the KMS key policy a WRITE of caller-supplied data into a
//! KMS-encrypted execution is decided against, driven over both services' own wire
//! protocols through the production `MultiServiceRouter` with Step Functions and KMS
//! mounted on ONE engine, so the key policy the write is judged against is the
//! document KMS's own handlers stored.
//!
//! The rule under test: a call that records data of the CALLER's into an execution of
//! a state machine whose `encryptionConfiguration` is `CUSTOMER_MANAGED_KMS_KEY` has
//! that data sealed under the key the account named — "for an execution with
//! encryption enabled, Step Functions will encrypt the error and cause fields using
//! the KMS key" (`smithy_query(query="doc",shape="SendTaskFailure")`, and the same
//! sentence on `StopExecution`) — so the call is served only while KMS lets this
//! caller have the key used for it and is refused with HTTP 400
//! `KmsAccessDeniedException` while it does not. A request that carries none of that
//! data seals nothing and needs no grant at all: "a caller can mark a task as fail
//! without using any KMS permissions in the execution role if the caller provides a
//! null value for both error and cause fields because no data needs to be encrypted".
//!
//! The rows span the writers the rule covers (both activity-task replies and the stop
//! of an execution), the fields each one seals — error alone, cause alone, both,
//! neither, and a reply's output — and the two KMS operations sealing a payload makes
//! on the key: the `GenerateDataKey` the `kmsDataKeyReusePeriodSeconds` member names
//! ("when the period expires, Step Functions will call `GenerateDataKey`") and the
//! decrypt of a data key still inside that period.
//!
//! The harness (`Chain`, the key policies, `customer_managed`) is the one
//! `execution_data_encryption` builds for the READ side of the same key; `tests/all.rs`
//! is this binary's crate root, so it is reached through
//! `crate::execution_data_encryption`.

use serde_json::{json, Value};

use crate::activity_tasks::activity_definition;
use crate::execution_data_encryption::{
    customer_managed, granting_policy, withdrawn_policy, Chain, KMS_ACCESS_DENIED,
    KMS_ACCESS_DENIED_STATUS, SFN,
};
use crate::state_machine_lifecycle::{error_code, string, ACCOUNT, ROLE_ARN};

/// The bound the scheduling state declares on its task: longer than any test here
/// takes, so no bound the clock owes closes a task while a key policy is being
/// changed under it.
const TASK_BOUND_SECONDS: u32 = 300;

/// The name a worker here polls under, which the service only logs.
const WORKER: &str = "sealed-writes-worker";

/// The error a worker reports, inside the modelled `error` length (0..256).
const REPORTED_ERROR: &str = "WorkerUnavailable";

/// The cause a worker reports, inside the modelled `cause` length (0..32768).
const REPORTED_CAUSE: &str = "the worker could not reach the dependency it needs";

/// A key policy that delegates exactly `actions` to the account that owns the key:
/// the shape of the default key policy ("gives the Amazon Web Services account that
/// owns the KMS key permission to use it in all cryptographic operations"), narrowed
/// to the actions named, so one crypto operation can be withheld while the other is
/// granted.
fn policy_delegating(actions: &[&str]) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "EnableIAMUserPermissions",
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": actions,
            "Resource": "*"
        }]
    })
}

/// One write of data the caller supplies into an execution, and whether that data is
/// sealed under the execution's key.
pub(crate) struct Write {
    pub(crate) case: &'static str,
    pub(crate) operation: &'static str,
    /// Whether the request carries data the key must seal, which is what needs the
    /// caller's grant on the key.
    pub(crate) seals: bool,
    /// The request body, built from the `taskToken` a worker holds and the execution
    /// ARN.
    pub(crate) request: fn(&str, &str) -> Value,
}

/// Every write the rule covers, and — beside each — the request form of the same
/// operation that carries nothing to seal.
pub(crate) fn writes() -> Vec<Write> {
    vec![
        Write {
            case: "SendTaskFailure carrying only the error",
            operation: "SendTaskFailure",
            seals: true,
            request: |token, _execution| json!({ "taskToken": token, "error": REPORTED_ERROR }),
        },
        Write {
            case: "SendTaskFailure carrying only the cause",
            operation: "SendTaskFailure",
            seals: true,
            request: |token, _execution| json!({ "taskToken": token, "cause": REPORTED_CAUSE }),
        },
        Write {
            case: "SendTaskFailure carrying both the error and the cause",
            operation: "SendTaskFailure",
            seals: true,
            request: |token, _execution| json!({
                "taskToken": token,
                "error": REPORTED_ERROR,
                "cause": REPORTED_CAUSE
            }),
        },
        Write {
            case: "SendTaskFailure carrying neither field",
            operation: "SendTaskFailure",
            seals: false,
            request: |token, _execution| json!({ "taskToken": token }),
        },
        Write {
            case: "SendTaskSuccess carrying the task's output",
            operation: "SendTaskSuccess",
            seals: true,
            request: |token, _execution| {
                json!({ "taskToken": token, "output": r#"{"done":true}"# })
            },
        },
        Write {
            case: "StopExecution carrying both the error and the cause",
            operation: "StopExecution",
            seals: true,
            request: |_token, execution| json!({
                "executionArn": execution,
                "error": REPORTED_ERROR,
                "cause": REPORTED_CAUSE
            }),
        },
        Write {
            case: "StopExecution carrying only the cause",
            operation: "StopExecution",
            seals: true,
            request: |_token, execution| {
                json!({ "executionArn": execution, "cause": REPORTED_CAUSE })
            },
        },
        Write {
            case: "StopExecution carrying neither field",
            operation: "StopExecution",
            seals: false,
            request: |_token, execution| json!({ "executionArn": execution }),
        },
    ]
}

/// A state machine whose one `Task` state polls a fresh activity, one RUNNING
/// execution of it, and the `taskToken` a worker now holds: the records every write
/// below reports into.
///
/// The token is the proof the issue's own first cause asks for — an execution that
/// entered a `Task` state naming an activity ARN queues a task on that activity, and
/// the next poll is handed it.
pub(crate) async fn outstanding_task(
    chain: &Chain,
    name: &str,
    encryption: Option<Value>,
) -> (String, String) {
    let created = chain
        .sfn("CreateActivity", &json!({ "name": format!("{name}-activity") }))
        .await;
    let activity = string(&created, "activityArn");

    let mut request = json!({
        "name": name,
        "definition": activity_definition(&activity, json!({ "TimeoutSeconds": TASK_BOUND_SECONDS })),
        "roleArn": ROLE_ARN,
    });
    if let Some(encryption) = encryption {
        request["encryptionConfiguration"] = encryption;
    }
    let machine = string(
        &chain.sfn("CreateStateMachine", &request).await,
        "stateMachineArn",
    );
    let execution = string(
        &chain
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "input": "{}" }),
            )
            .await,
        "executionArn",
    );

    let answer = chain
        .sfn(
            "GetActivityTask",
            &json!({ "activityArn": activity, "workerName": WORKER }),
        )
        .await;
    let token = answer
        .get("taskToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {name:?}: expected the poll to be handed a taskToken, got {answer:?}")
        })
        .to_string();
    assert!(
        !token.is_empty(),
        "case {name:?}: expected a non-empty taskToken, got {answer:?}"
    );
    (execution, token)
}

/// The message a refusal carries, for an assertion that names it.
pub(crate) fn message(answer: &Value) -> String {
    answer
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A write of data the key seals follows the KEY POLICY at the instant of the call:
/// refused with 400 `KmsAccessDeniedException` while the policy allows the caller
/// nothing, and served — the very same request, so the refusal consumed neither the
/// task nor the execution — once the statement is put back. A request that carries
/// nothing to seal is served while the policy allows nothing.
#[tokio::test]
async fn a_write_of_sealed_data_follows_the_keys_own_policy() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;

    for (index, write) in writes().into_iter().enumerate() {
        let Write {
            case,
            operation,
            seals,
            request,
        } = write;

        // The execution is started, and its task claimed, while the policy grants:
        // what is under test is the write, not the setup.
        chain.put_policy(&key, &granting_policy()).await;
        let (execution, token) = outstanding_task(
            &chain,
            &format!("sealed-write-{index}"),
            Some(customer_managed(&key)),
        )
        .await;
        let body = request(&token, &execution);

        chain.put_policy(&key, &withdrawn_policy()).await;
        let (status, answer) = chain.call(&SFN, operation, &body).await;
        if !seals {
            assert_eq!(
                status, 200,
                "case {case:?}: a request that seals no data of the caller's needs no grant on \
                 the key, got {status} with {answer:?}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&answer).as_str()),
            (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
            "case {case:?}: with the caller's statement withdrawn the write is refused, got \
             {status} with {answer:?}"
        );
        // Every variable part of the relayed sentence comes from the request and the
        // state: the operation the write makes on the key, and the key it is made on.
        // A reply to a task the execution is already holding re-uses the data key its
        // payloads go under, which is what opening that data key asks for.
        let refusal = message(&answer);
        assert!(
            refusal.contains("kms:Decrypt") && refusal.contains(&key),
            "case {case:?}: expected the refusal to name the use of the data key and the key \
             itself, got {answer:?}"
        );

        // The refusal wrote nothing: the task is still outstanding and the execution
        // still running, so the VERY SAME request is served once the statement is back.
        chain.put_policy(&key, &granting_policy()).await;
        let (status, answer) = chain.call(&SFN, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the restored statement serves the same write, so the refusal \
             consumed nothing, got {status} with {answer:?}"
        );
    }
}

/// Sealing data under the key asks TWO operations of it — the mint of a data key, and
/// the decrypt of one still inside the reuse period — so a policy granting one of them
/// and withholding the other does not let the write through, and the refusal names the
/// operation that was withheld.
#[tokio::test]
async fn a_write_needs_both_the_mint_and_the_reuse_of_a_data_key() {
    let chain = Chain::new();
    let key = chain.create_key(&granting_policy()).await;

    // Each row: the crypto actions the key policy delegates to the account, and the
    // action the write is refused for — `None` when the policy covers the whole write.
    let rows: Vec<(&str, Vec<&str>, Option<&str>)> = vec![
        (
            "only the mint of a data key is granted",
            vec!["kms:GenerateDataKey"],
            Some("kms:Decrypt"),
        ),
        (
            "only the decrypt of a data key is granted",
            vec!["kms:Decrypt"],
            Some("kms:GenerateDataKey"),
        ),
        (
            "both are granted",
            vec!["kms:GenerateDataKey", "kms:Decrypt"],
            None,
        ),
    ];

    for (index, (case, actions, withheld)) in rows.into_iter().enumerate() {
        chain.put_policy(&key, &granting_policy()).await;
        let (_execution, token) = outstanding_task(
            &chain,
            &format!("two-actions-{index}"),
            Some(customer_managed(&key)),
        )
        .await;

        chain.put_policy(&key, &policy_delegating(&actions)).await;
        let (status, answer) = chain
            .call(
                &SFN,
                "SendTaskFailure",
                &json!({
                    "taskToken": token,
                    "error": REPORTED_ERROR,
                    "cause": REPORTED_CAUSE
                }),
            )
            .await;
        let Some(withheld) = withheld else {
            assert_eq!(
                status, 200,
                "case {case:?}: a policy covering the whole write serves it, got {status} with \
                 {answer:?}"
            );
            continue;
        };
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
            "case {case:?}: a write missing one of the two operations is refused, got {status} \
             with {answer:?}"
        );
        assert!(
            message(&answer).contains(withheld),
            "case {case:?}: expected the refusal to name {withheld:?}, got {answer:?}"
        );
    }
}

/// An execution on an Amazon Web Services owned key — what a state machine created
/// without an encryption configuration is on, and what `AWS_OWNED_KEY` names — seals
/// its data under a key AWS manages the policy of, so every write is served whatever
/// any customer managed key's policy says.
#[tokio::test]
async fn a_write_to_an_execution_on_an_aws_owned_key_needs_no_grant() {
    let chain = Chain::new();
    // A key whose policy grants the caller nothing, to show that an execution not on
    // it is unaffected by it.
    let key = chain.create_key(&withdrawn_policy()).await;
    assert!(
        !key.is_empty(),
        "case the key: expected a key ARN, got {key:?}"
    );

    // Rows differ along HOW the state machine ends up on the AWS owned key: by naming
    // no encryption configuration at all, and by naming that type.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("no encryption configuration", None),
        (
            "the AWS owned key named",
            Some(json!({ "type": "AWS_OWNED_KEY" })),
        ),
    ];

    for (index, (case, encryption)) in rows.into_iter().enumerate() {
        // The reply that seals the most for a customer managed key: both fields.
        let (_execution, token) = outstanding_task(
            &chain,
            &format!("owned-reply-{index}"),
            encryption.clone(),
        )
        .await;
        let (status, answer) = chain
            .call(
                &SFN,
                "SendTaskFailure",
                &json!({
                    "taskToken": token,
                    "error": REPORTED_ERROR,
                    "cause": REPORTED_CAUSE
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a reply to an execution on the AWS owned key needs no grant, got \
             {status} with {answer:?}"
        );

        // And the stop of an execution, which seals the same two fields.
        let (execution, _token) =
            outstanding_task(&chain, &format!("owned-stop-{index}"), encryption).await;
        let (status, answer) = chain
            .call(
                &SFN,
                "StopExecution",
                &json!({
                    "executionArn": execution,
                    "error": REPORTED_ERROR,
                    "cause": REPORTED_CAUSE
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a stop of an execution on the AWS owned key needs no grant, got \
             {status} with {answer:?}"
        );
    }
}
