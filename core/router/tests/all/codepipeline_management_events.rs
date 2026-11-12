// SPDX-License-Identifier: CC0-1.0

//! A manual-approval decision in CloudTrail event history, attributed to the REVIEWER's
//! session — across four services on one engine.
//!
//! RULE: a service that serves a control-plane API call records it as a management event of
//! that service, so `cloudtrail:LookupEvents` reports the call under its own `eventName`
//! with the serving service's `eventSource`, the `Management` category, `readOnly` from the
//! operation's own semantics, the parameters the request carried, and — for a call made
//! with a role session's temporary credentials — `userIdentity.type` `AssumedRole`, the
//! SESSION's own ARN as `userIdentity.arn` and the issuing ROLE's ARN as
//! `sessionContext.sessionIssuer.arn`. A call that was refused records the fault shape's
//! own name as `errorCode` with the message the caller saw and no `responseElements`, and
//! each served call is its own record with its own `eventID`.
//!
//! Only a cross-service suite can pin this: the roles are written by `iam:CreateRole`, the
//! reviewer session is minted by `sts:AssumeRole`, the decision is served by CodePipeline
//! (whose execution engine moves the pipeline's artifacts through S3), and every record is
//! read back through `cloudtrail:LookupEvents` — four services over ONE engine, as
//! `core/router/src/lib.rs` mounts them.
//!
//! Every call goes over the wire, on a router with request authentication off as every
//! suite in this crate runs it: what a credential scope carries is the key id a handler
//! reads the caller from. Assembling the whole router costs stack (see `boot_serve.rs`), so
//! it is built — and the requests driven — on a sized thread.
//!
//! The harness below — the whole router over one engine, and the roles, bucket and session
//! a pipeline's approval needs — is shared with
//! `codepipeline_approval_reviewer_attribution`, which pins the PER-CALLER half of the same
//! rule: that two reviewers' decisions are each attributed to their own session.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, element, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
pub(crate) const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// CodePipeline's `X-Amz-Target` prefix, from that service's own model (service shape
/// `CodePipeline_20150709`).
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The Region every call of this suite is signed for, and so the Region its records live in.
const REGION: &str = "us-east-1";

/// The long-term access key id the SETUP calls carry — the roles, the bucket and the
/// pipeline — so the reviewer's own calls are the only ones made with a session.
pub(crate) const SETUP_KEY_ID: &str = "AKIDTEST";

/// The `eventSource` CloudTrail stamps on a CodePipeline management event
/// (`smithy_query(query="service_info")` ->
/// `awsService.cloudTrailEventSource: codepipeline.amazonaws.com`).
const CODEPIPELINE_EVENT_SOURCE: &str = "codepipeline.amazonaws.com";

/// The `eventCategory` of a control-plane call's record.
const MANAGEMENT: &str = "Management";

/// The pipeline the decision is made on, and the bucket its source and artifacts live in.
const PIPELINE: &str = "cp-audited-approval";

/// The stage and action the approval is addressed by — the names [`declaration`] builds a
/// pipeline's approval stage under, so both suites that read the stage address it by these.
pub(crate) const STAGE: &str = "Review";
pub(crate) const ACTION: &str = "ManualApproval";

/// The key [`declaration`]'s source action reads, which is also the object a suite has to
/// put into the artifact bucket for the execution to get past that stage.
pub(crate) const SOURCE_OBJECT_KEY: &str = "source.zip";

/// The summary the reviewer submits with the decision.
const SUMMARY: &str = "four eyes: approved by the reviewer session";

/// The reviewer's role session name.
const SESSION_NAME: &str = "reviewer-session";

/// How long a wait for the approval token may take before the suite calls it stalled.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(20);

/// How often that wait re-reads `GetPipelineState`.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The trust policy of the pipeline's SERVICE role: the role the pipeline itself runs
/// under, which must appear in no record of the reviewer's call.
pub(crate) const PIPELINE_TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
"Principal":{"Service":"codepipeline.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The trust policy of a REVIEWER's role: one this account may assume, so the session a
/// decision is signed with is one STS actually minted for that role.
pub(crate) fn reviewer_trust(account: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::{account}:root\"}},\
         \"Action\":\"sts:AssumeRole\"}}]}}"
    )
}

/// The emulator this suite drives: the whole router over one engine, and the runtime its
/// requests are blocked on.
pub(crate) struct Audited {
    router: MultiServiceRouter,
    runtime: tokio::runtime::Runtime,
}

impl Audited {
    pub(crate) fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = build_router(engine);
        router.set_request_authentication(false);
        Self {
            router,
            // A MULTI-thread runtime: the pipeline's actions are performed through
            // in-process calls into other services of this same router.
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("a multi-thread runtime"),
        }
    }

    /// Drive one request signed (in the routing sense) with `access_key_id`.
    fn send(
        &self,
        access_key_id: &str,
        signing_name: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, Vec<u8>) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            );
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.runtime.block_on(self.router.dispatch(request));
        let status = response.status().as_u16();
        (status, response.into_body().to_vec())
    }

    /// One awsQuery call against `signing_name`, signed with `access_key_id`.
    fn query(
        &self,
        access_key_id: &str,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let (status, payload) = self.send(
            access_key_id,
            signing_name,
            "POST",
            "/",
            &[("content-type", "application/x-www-form-urlencoded")],
            body.into_bytes(),
        );
        (status, String::from_utf8_lossy(&payload).into_owned())
    }

    /// One awsJson1_1 CodePipeline operation, signed with `access_key_id`.
    pub(crate) fn cp(&self, access_key_id: &str, operation: &str, body: &Value) -> (u16, Value) {
        let (status, payload) = self.send(
            access_key_id,
            "codepipeline",
            "POST",
            "/",
            &[
                ("content-type", "application/x-amz-json-1.1"),
                ("x-amz-target", &format!("{TARGET_PREFIX}.{operation}")),
            ],
            serde_json::to_vec(body).expect("the request body serializes"),
        );
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or_else(|_| {
                json!({ "unparsed": String::from_utf8_lossy(&payload).into_owned() })
            })
        };
        (status, parsed)
    }

    /// One CodePipeline operation that has to succeed.
    pub(crate) fn cp_ok(&self, access_key_id: &str, operation: &str, body: &Value) -> Value {
        let (status, answer) = self.cp(access_key_id, operation, body);
        assert_eq!(
            status, 200,
            "{operation} must answer 200, got {status} with {answer}"
        );
        answer
    }

    /// Create a role with `policy` as its trust policy; answer its ARN.
    pub(crate) fn role(&self, name: &str, policy: &str) -> String {
        let (status, body) = self.query(
            SETUP_KEY_ID,
            "iam",
            "2010-05-08",
            "CreateRole",
            &[("RoleName", name), ("AssumeRolePolicyDocument", policy)],
        );
        assert_eq!(
            status, 200,
            "setup: CreateRole({name:?}) must answer 200, got {status} with {body}"
        );
        element(&body, "Arn")
    }

    /// `sts:AssumeRole` of `role_arn`, answering the access key id of the session minted
    /// and the SESSION's own ARN as the assume reported it (`AssumedRoleUser.Arn`), so the
    /// assertions below compare the record against what the emulator actually minted
    /// rather than against a second statement of the ARN form.
    pub(crate) fn assume(&self, role_arn: &str, session_name: &str) -> (String, String) {
        let (status, body) = self.query(
            SETUP_KEY_ID,
            "sts",
            "2011-06-15",
            "AssumeRole",
            &[("RoleArn", role_arn), ("RoleSessionName", session_name)],
        );
        assert_eq!(
            status, 200,
            "setup: AssumeRole({session_name:?}) must answer 200, got {status} with {body}"
        );
        let assumed = block_of(&body, "AssumedRoleUser").unwrap_or_else(|| {
            panic!("setup: the assume reports the session it minted, got {body}")
        });
        (element(&body, "AccessKeyId"), element(assumed, "Arn"))
    }

    /// Create a bucket and put the pipeline's source object into it.
    pub(crate) fn bucket_with_source(&self, bucket: &str, key: &str, body: &[u8]) {
        for (uri, payload) in [(format!("/{bucket}"), b"".as_slice()), (format!("/{bucket}/{key}"), body)] {
            let (status, answer) = self.send(
                SETUP_KEY_ID,
                "s3",
                "PUT",
                &uri,
                &[],
                payload.to_vec(),
            );
            assert_eq!(
                status,
                200,
                "setup: PUT {uri} must answer 200, got {status} with {:?}",
                String::from_utf8_lossy(&answer)
            );
        }
    }

    /// `cloudtrail:LookupEvents` filtered on one attribute, answering each event's
    /// `CloudTrailEvent` document, most recent first.
    pub(crate) fn lookup(&self, attribute_key: &str, attribute_value: &str) -> Vec<Value> {
        let (status, payload) = self.send(
            SETUP_KEY_ID,
            "cloudtrail",
            "POST",
            "/",
            &[
                ("content-type", "application/x-amz-json-1.1"),
                ("x-amz-target", "CloudTrail_20131101.LookupEvents"),
            ],
            json!({
                "LookupAttributes": [{
                    "AttributeKey": attribute_key,
                    "AttributeValue": attribute_value,
                }],
            })
            .to_string()
            .into_bytes(),
        );
        let text = String::from_utf8_lossy(&payload).into_owned();
        assert_eq!(
            status, 200,
            "LookupEvents({attribute_key}={attribute_value}) must succeed, got {status} with \
             {text}"
        );
        let answered: Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("the lookup answers JSON ({error}): {text}"));
        answered["Events"]
            .as_array()
            .unwrap_or_else(|| panic!("LookupEvents reports an Events list, got {text}"))
            .iter()
            .map(|event| {
                let document = event["CloudTrailEvent"]
                    .as_str()
                    .unwrap_or_else(|| panic!("an event carries its CloudTrailEvent: {event}"));
                serde_json::from_str(document)
                    .unwrap_or_else(|error| panic!("CloudTrailEvent is JSON ({error}): {document}"))
            })
            .collect()
    }
}

/// A string member of an event document.
pub(crate) fn at(event: &Value, pointer: &str) -> Option<String> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The SHAPE name of an `awsJson1_1` error answer, whose `__type` may carry the namespace.
fn shape_name(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error body, got {body}"))
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Poll until `read` answers, or give up after [`SETTLE_TIMEOUT`].
pub(crate) fn settle<T>(what: &str, mut read: impl FnMut() -> Result<T, String>) -> T {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    let mut last = String::new();
    while Instant::now() < deadline {
        match read() {
            Ok(value) => return value,
            Err(state) => last = state,
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    panic!("timed out waiting for {what}; last saw {last}");
}

/// The declaration of a pipeline a decision is made on: an S3 source stage and a
/// manual-approval stage, under the name and over the artifact bucket it is given.
pub(crate) fn declaration(pipeline: &str, role_arn: &str, bucket: &str) -> Value {
    json!({
        "pipeline": {
            "name": pipeline,
            "roleArn": role_arn,
            "artifactStore": { "type": "S3", "location": bucket },
            "stages": [
                {
                    "name": "Source",
                    "actions": [{
                        "name": "Src",
                        "actionTypeId": {
                            "category": "Source",
                            "owner": "AWS",
                            "provider": "S3",
                            "version": "1",
                        },
                        "configuration": {
                            "S3Bucket": bucket,
                            "S3ObjectKey": SOURCE_OBJECT_KEY,
                            "PollForSourceChanges": "false",
                        },
                        "outputArtifacts": [{ "name": "SourceArtifact" }],
                    }],
                },
                {
                    "name": STAGE,
                    "actions": [{
                        "name": ACTION,
                        "actionTypeId": {
                            "category": "Approval",
                            "owner": "AWS",
                            "provider": "Manual",
                            "version": "1",
                        },
                    }],
                },
            ],
        }
    })
}

/// Assert the facts every record of a CodePipeline call carries.
pub(crate) fn assert_codepipeline_record(
    case: &str,
    event: &Value,
    operation: &str,
    read_only: bool,
) {
    assert_eq!(
        at(event, "/eventName").as_deref(),
        Some(operation),
        "case {case:?}: the record names the operation, got {event}"
    );
    assert_eq!(
        at(event, "/eventSource").as_deref(),
        Some(CODEPIPELINE_EVENT_SOURCE),
        "case {case:?}: the record names CodePipeline as its source, got {event}"
    );
    assert_eq!(
        at(event, "/eventCategory").as_deref(),
        Some(MANAGEMENT),
        "case {case:?}: a control-plane call is a management event, got {event}"
    );
    assert_eq!(
        event.get("readOnly"),
        Some(&json!(read_only)),
        "case {case:?}: expected readOnly {read_only}, got {event}"
    );
    assert_eq!(
        at(event, "/awsRegion").as_deref(),
        Some(REGION),
        "case {case:?}: the record is in the Region the call was served in, got {event}"
    );
}

/// Assert that `event` attributes the call to the reviewer's SESSION and to the role that
/// issued it — and to neither the pipeline's service role nor the long-term setup key.
pub(crate) fn assert_reviewer_attribution(
    case: &str,
    event: &Value,
    session_key: &str,
    session_arn: &str,
    reviewer_role_arn: &str,
    pipeline_role_arn: &str,
) {
    assert_eq!(
        at(event, "/userIdentity/type").as_deref(),
        Some("AssumedRole"),
        "case {case:?}: the decision was made by an assumed role, got {event}"
    );
    assert_eq!(
        at(event, "/userIdentity/accessKeyId").as_deref(),
        Some(session_key),
        "case {case:?}: the record names the credential the call was signed with, got {event}"
    );
    assert_eq!(
        at(event, "/userIdentity/arn").as_deref(),
        Some(session_arn),
        "case {case:?}: userIdentity.arn is the reviewer's SESSION, got {event}"
    );
    assert_eq!(
        at(event, "/userIdentity/sessionContext/sessionIssuer/arn").as_deref(),
        Some(reviewer_role_arn),
        "case {case:?}: the session's issuer is the reviewer ROLE, got {event}"
    );
    let identity = event
        .get("userIdentity")
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        !identity.contains(pipeline_role_arn),
        "case {case:?}: the pipeline's service role must appear nowhere in userIdentity, got \
         {identity}"
    );
    assert!(
        !identity.contains(SETUP_KEY_ID),
        "case {case:?}: the reviewer's call must not be attributed to the setup credential, \
         got {identity}"
    );
}

#[test]
fn a_manual_approval_decision_and_its_refused_replay_are_audited_as_the_reviewer() {
    std::thread::Builder::new()
        .name("codepipeline-management-events".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let emulator = Audited::new();
            let account = engine::emulator_account_id().to_string();

            // The pipeline's own service role, and the SECOND, distinct role the reviewer
            // assumes to answer the approval.
            let pipeline_role = emulator.role("cp-audited-pipeline", PIPELINE_TRUST_POLICY);
            let reviewer_role = emulator.role("cp-audited-reviewer", &reviewer_trust(&account));
            // The session's own ARN comes from the assume's answer, so the attribution
            // assertions compare the record against the session STS actually minted.
            let (session_key, session_arn) = emulator.assume(&reviewer_role, SESSION_NAME);
            assert!(
                session_arn.ends_with(&format!("assumed-role/cp-audited-reviewer/{SESSION_NAME}")),
                "setup: the assume must report this reviewer's session, got {session_arn}"
            );

            emulator.bucket_with_source(PIPELINE, SOURCE_OBJECT_KEY, b"the approved source bytes");
            emulator.cp_ok(
                SETUP_KEY_ID,
                "CreatePipeline",
                &declaration(PIPELINE, &pipeline_role, PIPELINE),
            );

            // The execution the pipeline's own creation started parks at the approval and
            // publishes the token the reviewer answers.
            let token = settle("the approval token", || {
                let state =
                    emulator.cp_ok(SETUP_KEY_ID, "GetPipelineState", &json!({ "name": PIPELINE }));
                state["stageStates"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|stage| stage["stageName"] == json!(STAGE))
                    .filter_map(|stage| stage["actionStates"].as_array())
                    .flatten()
                    .find(|action| action["actionName"] == json!(ACTION))
                    .and_then(|action| action["latestExecution"]["token"].as_str())
                    .map(str::to_string)
                    .ok_or_else(|| state.to_string())
            });

            // The decision, made with the REVIEWER's session credentials.
            let decision = json!({
                "pipelineName": PIPELINE,
                "stageName": STAGE,
                "actionName": ACTION,
                "result": { "status": "Approved", "summary": SUMMARY },
                "token": token,
            });
            let answered = emulator.cp_ok(&session_key, "PutApprovalResult", &decision);
            assert!(
                answered["approvedAt"].is_number() || answered["approvedAt"].is_string(),
                "the accepted decision reports approvedAt, got {answered}"
            );

            // And the replay of the now-spent token, by the same session. Which refusal it
            // is belongs to the approval handler; the record must repeat the code and the
            // message the caller was answered with.
            let (status, refused) = emulator.cp(&session_key, "PutApprovalResult", &decision);
            assert_eq!(
                status, 400,
                "the replay of a spent token is refused, got {status} with {refused}"
            );
            let refused_code = shape_name(&refused);
            let refused_message = refused["message"]
                .as_str()
                .or_else(|| refused["Message"].as_str())
                .unwrap_or_default()
                .to_string();
            assert!(
                !refused_message.is_empty(),
                "the refusal must state a message, got {refused}"
            );

            // Found the way the check that filed this issue finds them: a lookup by event
            // name, and the same records reachable by the serving service's event source.
            let by_name = emulator.lookup("EventName", "PutApprovalResult");
            assert_eq!(
                by_name.len(),
                2,
                "the decision and its replay are two records, got {by_name:?}"
            );
            let by_source = emulator.lookup("EventSource", CODEPIPELINE_EVENT_SOURCE);
            let sourced: Vec<String> = by_source
                .iter()
                .filter_map(|event| at(event, "/eventName"))
                .collect();
            for expected in ["CreatePipeline", "GetPipelineState", "PutApprovalResult"] {
                assert!(
                    sourced.iter().any(|name| name == expected),
                    "a lookup by event source reports {expected}, got {sourced:?}"
                );
            }

            // `LookupEvents` lists the most recent event first, so the replay comes before
            // the decision.
            let replay = &by_name[0];
            let served = &by_name[1];
            assert_eq!(
                at(served, "/errorCode"),
                None,
                "the served decision carries no error code, got {served}"
            );
            assert_ne!(
                at(served, "/eventID"),
                at(replay, "/eventID"),
                "each served call is its own event, got {by_name:?}"
            );

            // (case, the record, the errorCode it must carry, whether it answered anything)
            for (case, event, error_code, answers) in [
                ("the decision", served, None, true),
                ("the replay", replay, Some(refused_code.as_str()), false),
            ] {
                assert_codepipeline_record(case, event, "PutApprovalResult", false);
                assert_reviewer_attribution(
                    case,
                    event,
                    &session_key,
                    &session_arn,
                    &reviewer_role,
                    &pipeline_role,
                );
                assert_eq!(
                    at(event, "/errorCode").as_deref(),
                    error_code,
                    "case {case:?}: expected errorCode {error_code:?}, got {event}"
                );
                // The decision the reviewer submitted, as the request carried it.
                for (pointer, expected) in [
                    ("/requestParameters/pipelineName", PIPELINE),
                    ("/requestParameters/stageName", STAGE),
                    ("/requestParameters/actionName", ACTION),
                    ("/requestParameters/result/status", "Approved"),
                    ("/requestParameters/result/summary", SUMMARY),
                ] {
                    assert_eq!(
                        at(event, pointer).as_deref(),
                        Some(expected),
                        "case {case:?}: the record carries {pointer} as sent, got {event}"
                    );
                }
                assert_eq!(
                    event
                        .get("responseElements")
                        .is_some_and(|elements| !elements.is_null()),
                    answers,
                    "case {case:?}: expected responseElements present={answers}, got {event}"
                );
            }
            assert_eq!(
                at(replay, "/errorMessage").as_deref(),
                Some(refused_message.as_str()),
                "the replay's record repeats the message the caller saw, got {replay}"
            );

            // A READ of the same pipeline is told apart from the write by `readOnly`, and
            // is attributed to the credential that made it rather than to the reviewer.
            let reads = emulator.lookup("EventName", "GetPipelineState");
            let read = reads
                .first()
                .unwrap_or_else(|| panic!("the pipeline reads are recorded, got {reads:?}"));
            assert_codepipeline_record("a read", read, "GetPipelineState", true);
            assert_eq!(
                read.get("responseElements"),
                Some(&Value::Null),
                "a reporting call records no responseElements, got {read}"
            );
            assert_eq!(
                at(read, "/userIdentity/accessKeyId").as_deref(),
                Some(SETUP_KEY_ID),
                "the read was made with the setup credential, got {read}"
            );
        })
        .expect("spawn the CodePipeline management-event thread")
        .join()
        .expect("a manual-approval decision must be audited as the reviewer's session");
}
