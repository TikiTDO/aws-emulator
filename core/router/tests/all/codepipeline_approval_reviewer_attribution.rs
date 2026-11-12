// SPDX-License-Identifier: CC0-1.0

//! WHO released a change: each manual-approval decision is audited as the session that made
//! THAT decision, and is reachable in event history by asking for that caller's calls.
//!
//! RULE: a management event records the credential the call it reports was signed with, so
//! two calls of ONE operation made with two different role sessions are each attributed to
//! their own session and to the role that issued it; a lookup by a caller attribute
//! (`Username`, `AccessKeyId` — two of the eight keys CloudTrail's `LookupAttributeKey`
//! declares) reports only that caller's calls; and the member the serving API itself reports
//! for who last changed the resource names the same principal as the record's
//! `userIdentity.arn`.
//!
//! `codepipeline_management_events` pins the record of ONE reviewer's decision and of its
//! refused replay. This suite varies the CALLER and the decision: two reviewers, each with
//! its own role and session, each answering its own pipeline's approval, with both
//! `ApprovalStatus` values the model declares. With one reviewer a recorder that names any
//! session at all passes; with two, only a recorder that reads the credential of the call it
//! is recording does — which is the fact four-eyes evidence rests on.
//!
//! Only a cross-service suite can pin this: the roles are written by `iam:CreateRole`, each
//! reviewer session is minted by `sts:AssumeRole`, the decisions are served by CodePipeline
//! (whose execution engine moves each pipeline's artifacts through S3), the control plane's
//! own answer comes from `codepipeline:GetPipelineState`, and every record is read back
//! through `cloudtrail:LookupEvents` — five services over ONE engine, as
//! `core/router/src/lib.rs` mounts them.
//!
//! Every call goes over the wire on a router with request authentication off, as every suite
//! in this crate runs it, and the whole assembly is driven on a sized thread (see
//! `boot_serve.rs`). The harness is the sibling suite's
//! (`crate::codepipeline_management_events`), so both read event history the same way.

use serde_json::{json, Value};

use crate::codepipeline_management_events::{
    assert_codepipeline_record, assert_reviewer_attribution, at, declaration, reviewer_trust,
    settle, Audited, ACTION, BOOT_STACK_SIZE, PIPELINE_TRUST_POLICY, SETUP_KEY_ID,
    SOURCE_OBJECT_KEY, STAGE,
};

/// The operation every record this suite asserts on reports.
const OPERATION: &str = "PutApprovalResult";

/// The bytes each pipeline's source object carries.
const SOURCE_BYTES: &[u8] = b"the bytes a reviewer released";

/// The role the pipelines themselves run under: shared by both, and the principal that must
/// appear in NO reviewer's record.
const PIPELINE_ROLE: &str = "cp-attributed-pipeline";

/// One row of the table: a decision made by its OWN reviewer on its OWN pipeline.
struct Decision {
    /// What the row is, named in every assertion.
    case: &'static str,
    /// The pipeline the decision is made on — also the bucket its source lives in.
    pipeline: &'static str,
    /// The reviewer's role, and the name of the session that role is assumed under. The
    /// session name is what CloudTrail reports as an event's `Username`, so it is also what
    /// the `Username` lookup of this row asks for.
    role_name: &'static str,
    session_name: &'static str,
    /// The decision submitted, and the summary submitted with it.
    status: &'static str,
    summary: &'static str,
    /// The status the answered action is left in: an approved action succeeded, a rejected
    /// one failed.
    action_status: &'static str,
}

/// The decisions this suite makes, one per reviewer.
///
/// The rows differ along the caller (a distinct role and session each), the pipeline the
/// decision is made on, and the decision itself — both values of `ApprovalStatus`, which
/// `smithy_query(query="shape", shape="ApprovalStatus")` reports as `["Approved",
/// "Rejected"]` and whose consequence for the action is what `action_status` states.
const DECISIONS: [Decision; 2] = [
    Decision {
        case: "the approval",
        pipeline: "cp-attributed-release",
        role_name: "cp-attributed-reviewer-one",
        session_name: "reviewer-one-session",
        status: "Approved",
        summary: "four eyes: the first reviewer released it",
        action_status: "Succeeded",
    },
    Decision {
        case: "the rejection",
        pipeline: "cp-attributed-holdback",
        role_name: "cp-attributed-reviewer-two",
        session_name: "reviewer-two-session",
        status: "Rejected",
        summary: "four eyes: the second reviewer held it back",
        action_status: "Failed",
    },
];

/// What the suite holds for one row once its reviewer exists and its pipeline is created.
struct Reviewer {
    /// The reviewer ROLE's own ARN, which a record names as the session's issuer.
    role_arn: String,
    /// The session's access key id, which the decision is signed with.
    session_key: String,
    /// The SESSION's own ARN, as the assume reported it.
    session_arn: String,
}

/// The approval action of `pipeline`, as `GetPipelineState` reports it right now.
fn approval_action(emulator: &Audited, pipeline: &str) -> Value {
    let state = emulator.cp_ok(SETUP_KEY_ID, "GetPipelineState", &json!({ "name": pipeline }));
    state["stageStates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|stage| stage["stageName"] == json!(STAGE))
        .filter_map(|stage| stage["actionStates"].as_array())
        .flatten()
        .find(|action| action["actionName"] == json!(ACTION))
        .cloned()
        .unwrap_or_else(|| panic!("{pipeline} declares a {STAGE}/{ACTION} action, got {state}"))
}

/// Wait until `pipeline`'s approval publishes the token its reviewer answers it with.
fn approval_token(emulator: &Audited, decision: &Decision) -> String {
    settle(
        &format!("case {:?}: the approval token", decision.case),
        || {
            let action = approval_action(emulator, decision.pipeline);
            action["latestExecution"]["token"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| action.to_string())
        },
    )
}

/// The one record of [`OPERATION`] among the events a lookup reported.
fn sole_decision(case: &str, found_by: &str, events: &[Value]) -> Value {
    let decisions: Vec<&Value> = events
        .iter()
        .filter(|event| at(event, "/eventName").as_deref() == Some(OPERATION))
        .collect();
    assert_eq!(
        decisions.len(),
        1,
        "case {case:?}: a lookup by {found_by} reports exactly one {OPERATION} record, got {:?}",
        events
            .iter()
            .map(|event| at(event, "/eventName"))
            .collect::<Vec<_>>()
    );
    decisions[0].clone()
}

#[test]
fn each_approval_decision_is_audited_as_the_session_that_made_it() {
    std::thread::Builder::new()
        .name("codepipeline-reviewer-attribution".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let emulator = Audited::new();
            let account = engine::emulator_account_id().to_string();
            // The role both pipelines run under, which no reviewer's record may name.
            let pipeline_role = emulator.role(PIPELINE_ROLE, PIPELINE_TRUST_POLICY);

            // Each row's reviewer, and the pipeline whose approval it will answer.
            let reviewers: Vec<Reviewer> = DECISIONS
                .iter()
                .map(|decision| {
                    let role_arn = emulator.role(decision.role_name, &reviewer_trust(&account));
                    let (session_key, session_arn) =
                        emulator.assume(&role_arn, decision.session_name);
                    assert!(
                        session_arn.ends_with(&format!(
                            "assumed-role/{}/{}",
                            decision.role_name, decision.session_name
                        )),
                        "case {:?}: the assume must report this reviewer's session, got \
                         {session_arn}",
                        decision.case
                    );
                    emulator.bucket_with_source(
                        decision.pipeline,
                        SOURCE_OBJECT_KEY,
                        SOURCE_BYTES,
                    );
                    emulator.cp_ok(
                        SETUP_KEY_ID,
                        "CreatePipeline",
                        // The artifact bucket of each pipeline is the one named after it.
                        &declaration(decision.pipeline, &pipeline_role, decision.pipeline),
                    );
                    Reviewer {
                        role_arn,
                        session_key,
                        session_arn,
                    }
                })
                .collect();

            // Every reviewer answers its own pipeline's approval, signed with its own
            // session — and the suite keeps the token the request carried, so the record's
            // parameters are compared against what was sent rather than against a literal.
            let tokens: Vec<String> = DECISIONS
                .iter()
                .zip(&reviewers)
                .map(|(decision, reviewer)| {
                    let token = approval_token(&emulator, decision);
                    let answered = emulator.cp_ok(
                        &reviewer.session_key,
                        OPERATION,
                        &json!({
                            "pipelineName": decision.pipeline,
                            "stageName": STAGE,
                            "actionName": ACTION,
                            "result": { "status": decision.status, "summary": decision.summary },
                            "token": token,
                        }),
                    );
                    assert!(
                        answered["approvedAt"].is_number() || answered["approvedAt"].is_string(),
                        "case {:?}: the accepted decision reports approvedAt, got {answered}",
                        decision.case
                    );
                    token
                })
                .collect();

            // Both decisions are in event history, as two records of one operation.
            let by_name = emulator.lookup("EventName", OPERATION);
            assert_eq!(
                by_name.len(),
                DECISIONS.len(),
                "every decision is its own record, got {by_name:?}"
            );
            let attributed: Vec<(Option<String>, Option<String>)> = by_name
                .iter()
                .map(|event| {
                    (
                        at(event, "/requestParameters/pipelineName"),
                        at(event, "/userIdentity/arn"),
                    )
                })
                .collect();
            for (decision, reviewer) in DECISIONS.iter().zip(&reviewers) {
                assert!(
                    attributed.contains(&(
                        Some(decision.pipeline.to_string()),
                        Some(reviewer.session_arn.clone())
                    )),
                    "case {:?}: the decision on {} is attributed to {}, got {attributed:?}",
                    decision.case,
                    decision.pipeline,
                    reviewer.session_arn
                );
            }

            for (index, ((decision, reviewer), token)) in
                DECISIONS.iter().zip(&reviewers).zip(&tokens).enumerate()
            {
                let case = decision.case;
                // The reviewer's OWN calls, asked for the two ways a caller is named: by the
                // name the session speaks under and by the key it signs with.
                let by_user = emulator.lookup("Username", decision.session_name);
                let record = sole_decision(case, "Username", &by_user);
                let by_key = emulator.lookup("AccessKeyId", &reviewer.session_key);
                let keyed = sole_decision(case, "AccessKeyId", &by_key);
                assert_eq!(
                    at(&record, "/eventID"),
                    at(&keyed, "/eventID"),
                    "case {case:?}: both lookups report the same record, got {record} and {keyed}"
                );
                for event in &by_key {
                    assert_eq!(
                        at(event, "/userIdentity/accessKeyId").as_deref(),
                        Some(reviewer.session_key.as_str()),
                        "case {case:?}: a lookup by access key reports only that key's calls, \
                         got {event}"
                    );
                }

                assert_codepipeline_record(case, &record, OPERATION, false);
                assert_eq!(
                    at(&record, "/errorCode"),
                    None,
                    "case {case:?}: the served decision carries no error code, got {record}"
                );
                // The session that made THIS call, and the role that issued that session:
                // the sibling suite's own assertion of that attribution, so one statement of
                // it holds for both.
                assert_reviewer_attribution(
                    case,
                    &record,
                    &reviewer.session_key,
                    &reviewer.session_arn,
                    &reviewer.role_arn,
                    &pipeline_role,
                );
                // Neither the role the pipeline runs under nor the OTHER reviewer appears
                // anywhere in the record: attribution follows the call, not the resource and
                // not whichever session was minted last.
                let other = &reviewers[(index + 1) % reviewers.len()];
                let whole = record.to_string();
                for (what, principal) in [
                    ("the pipeline's service role", pipeline_role.as_str()),
                    ("the other reviewer's session", other.session_arn.as_str()),
                    ("the other reviewer's role", other.role_arn.as_str()),
                ] {
                    assert!(
                        !whole.contains(principal),
                        "case {case:?}: {what} ({principal}) must appear nowhere in the record, \
                         got {whole}"
                    );
                }

                // The decision as the request carried it, token included.
                for (pointer, expected) in [
                    ("/requestParameters/pipelineName", decision.pipeline),
                    ("/requestParameters/stageName", STAGE),
                    ("/requestParameters/actionName", ACTION),
                    ("/requestParameters/result/status", decision.status),
                    ("/requestParameters/result/summary", decision.summary),
                    ("/requestParameters/token", token.as_str()),
                ] {
                    assert_eq!(
                        at(&record, pointer).as_deref(),
                        Some(expected),
                        "case {case:?}: the record carries {pointer} as sent, got {record}"
                    );
                }

                // And the serving API's own answer for who last changed the action names the
                // same principal the record does, so the audit trail and the control plane
                // agree on who decided.
                let action = approval_action(&emulator, decision.pipeline);
                assert_eq!(
                    action["latestExecution"]["lastUpdatedBy"].as_str(),
                    Some(reviewer.session_arn.as_str()),
                    "case {case:?}: GetPipelineState reports this reviewer as the last to \
                     change the action, got {action}"
                );
                assert_eq!(
                    action["latestExecution"]["status"].as_str(),
                    Some(decision.action_status),
                    "case {case:?}: expected the action {}, got {action}",
                    decision.action_status
                );
                assert_eq!(
                    action["latestExecution"]["summary"].as_str(),
                    Some(decision.summary),
                    "case {case:?}: the answered action reports the summary submitted, got \
                     {action}"
                );
                assert_eq!(
                    action["latestExecution"]["token"],
                    Value::Null,
                    "case {case:?}: the answered approval publishes no token, got {action}"
                );
            }
        })
        .expect("spawn the CodePipeline reviewer-attribution thread")
        .join()
        .expect("each decision must be audited as the session that made it");
}
