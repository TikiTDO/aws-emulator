// SPDX-License-Identifier: CC0-1.0

//! The stage-transition plane: what a reader reports for a stage's inbound transition, and
//! who is allowed to change one — end to end over the wire the service speaks
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The two rules under test:
//!
//! * EVERY stage of a pipeline reports its `inboundTransitionState`, the first stage's
//!   included: the member states whether the transition INTO the stage is enabled, which
//!   is a fact every stage has, and it is enabled until a call disables it;
//! * a request signed by a credential this account minted for an IAM identity changes a
//!   transition only when that identity's own identity-based policies allow the
//!   `codepipeline:<Action>` it performs on the pipeline it names. An identity nothing
//!   allows it for is refused with the error code `AccessDeniedException` (HTTP 400) and
//!   the stored transition is left exactly as it was. A credential belonging to no IAM
//!   identity is the account's own published one, which no identity-based policy
//!   constrains.
//!
//! The identity is created through IAM's OWN wire path (`CreateUser`, `PutUserPolicy`,
//! `CreateAccessKey`), with IAM and STS mounted on the same engine as this service —
//! exactly how `core/router` assembles them — so the documents the decision reads are the
//! ones a client would have written.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The region every request of this suite is signed for, and so the region of the ARN a
/// decision is taken about.
const REGION: &str = "us-east-1";

/// The access key id a request presents when it is NOT one IAM minted: the account's own
/// published credential, which holds no identity policy.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The role every accepted declaration names, created in IAM by [`create_role`].
const ROLE_NAME: &str = "cp-execution-role";

/// A trust policy with a single statement — the form `CreateRole`'s own modelled example
/// carries, and all it asks of one.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"codepipeline.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The ARN of that role, as IAM holds it: `CreatePipeline` resolves the declaration's
/// `roleArn` in IAM, which is mounted here.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/{ROLE_NAME}",
        ::engine::emulator_account_id()
    )
}

/// The pipeline every case addresses.
const PIPELINE: &str = "demo";

/// The stages that pipeline declares, in order.
const STAGES: [&str; 3] = ["Source", "Build", "Deploy"];

/// The two operations that change a stage transition: one plane, one decision, so every
/// authorization row is asserted on both.
const TRANSITION_OPERATIONS: [&str; 2] = ["DisableStageTransition", "EnableStageTransition"];

/// The wire error code of an authorization refusal.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// One emulator: this service, IAM and STS over ONE shared engine, the way the router
/// mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 credential scope naming `access_key_id` and `signing_name`, in [`REGION`].
///
/// Request authentication is switched off in the harness, so the signature is never
/// verified; the credential scope is the only place the signing key id appears on the wire,
/// and it is what the service reads the caller's identity from.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host;x-amz-date, Signature=00"
    )
}

/// Call one operation of this service, presented as signed by `access_key_id`.
async fn call_signed_by(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "codepipeline.us-east-1.amazonaws.com")
        .header("authorization", authorization(access_key_id, "codepipeline"))
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the harness builds a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|error| {
            panic!("{operation}: response body is not JSON ({error}): {payload:?}")
        })
    };
    (status, parsed)
}

/// Call one operation as the account's own credential, and require a success.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, answer) = call_signed_by(router, ACCOUNT_KEY, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200, got {status} with {answer}"
    );
    answer
}

/// One awsQuery call against the IAM service mounted beside this one.
async fn iam_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("authorization", authorization(ACCOUNT_KEY, "iam"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("the harness builds a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 body");
    (status, payload)
}

/// Percent-encode one `application/x-www-form-urlencoded` component through the shared
/// harness's rule (`pipeline_lifecycle::form_encode`), rather than a second copy.
fn form_encode(value: &str) -> String {
    crate::pipeline_lifecycle::form_encode(value)
}

/// The text of the single `<tag>` element an awsQuery response carries.
fn xml_element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("<{tag}> is absent from {body}"));
    let rest = &body[start + open.len()..];
    let end = rest
        .find(&close)
        .unwrap_or_else(|| panic!("<{tag}> is never closed in {body}"));
    rest[..end].to_string()
}

/// Create an IAM user carrying `document` as an inline policy (when one is given), and
/// answer the access key id minted for it.
async fn user_with_key(
    router: &MultiServiceRouter,
    user_name: &str,
    document: Option<&str>,
) -> String {
    let (status, body) = iam_call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(status, 200, "CreateUser {user_name} must succeed: {body}");
    if let Some(document) = document {
        let (status, body) = iam_call(
            router,
            "PutUserPolicy",
            &[
                ("UserName", user_name),
                ("PolicyName", "transitions"),
                ("PolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(status, 200, "PutUserPolicy {user_name} must succeed: {body}");
    }
    let (status, body) = iam_call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "CreateAccessKey {user_name} must succeed: {body}"
    );
    xml_element(&body, "AccessKeyId")
}

/// Create the role every accepted declaration names, over IAM's awsQuery wire.
async fn create_role(router: &MultiServiceRouter) {
    let (status, body) = iam_call(
        router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(status, 200, "the fixture role must be created: {body}");
}

/// The declaration every case's pipeline carries: three stages, all `Custom`-owned so no
/// action's work runs while a case is asserting about the transitions.
fn declaration() -> Value {
    json!({
        "name": PIPELINE,
        "roleArn": role_arn(),
        "artifactStore": {"type": "S3", "location": "my-bucket"},
        "stages": [
            {
                "name": STAGES[0],
                "actions": [{
                    "name": "Src",
                    "actionTypeId": {
                        "category": "Source", "owner": "Custom",
                        "provider": "MySource", "version": "1",
                    },
                    "outputArtifacts": [{"name": "SourceOut"}],
                }],
            },
            {
                "name": STAGES[1],
                "actions": [{
                    "name": "Compile",
                    "actionTypeId": {
                        "category": "Build", "owner": "Custom",
                        "provider": "MyBuilder", "version": "1",
                    },
                    "inputArtifacts": [{"name": "SourceOut"}],
                }],
            },
            {
                "name": STAGES[2],
                "actions": [{
                    "name": "Ship",
                    "actionTypeId": {
                        "category": "Deploy", "owner": "Custom",
                        "provider": "MyDeployer", "version": "1",
                    },
                    "inputArtifacts": [{"name": "SourceOut"}],
                }],
            },
        ],
    })
}

/// The pipeline ARN a decision about [`PIPELINE`] is taken on.
fn pipeline_arn() -> String {
    format!(
        "arn:aws:codepipeline:{REGION}:{}:{PIPELINE}",
        ::engine::emulator_account_id()
    )
}

/// The `inboundTransitionState` the reader reports for `stage`, or `None` when the member
/// is absent.
async fn inbound_transition(router: &MultiServiceRouter, stage: &str) -> Option<Value> {
    let state = ok(router, "GetPipelineState", &json!({"name": PIPELINE})).await;
    state["stageStates"]
        .as_array()
        .unwrap_or_else(|| panic!("GetPipelineState reports stageStates, got {state}"))
        .iter()
        .find(|reported| reported["stageName"] == json!(stage))
        .and_then(|reported| reported.get("inboundTransitionState").cloned())
}

/// RULE: every stage reports the state of the transition INTO it — the first stage's
/// included — enabled until a call disables it, and a disable of any one stage's
/// transition is reported on that stage alone.
#[tokio::test]
async fn every_stage_reports_the_state_of_the_transition_into_it() {
    let reason = "Holding for release review";
    // The rows differ along which stage's transition is held: the FIRST one (the case the
    // member was absent for), a middle one and the last one — plus a row that holds none.
    let rows: Vec<Option<&str>> = vec![None, Some(STAGES[0]), Some(STAGES[1]), Some(STAGES[2])];
    for held in rows {
        let case = held.unwrap_or("nothing held");
        let router = emulator();
        create_role(&router).await;
        ok(&router, "CreatePipeline", &json!({"pipeline": declaration()})).await;
        if let Some(held) = held {
            ok(
                &router,
                "DisableStageTransition",
                &json!({
                    "pipelineName": PIPELINE,
                    "stageName": held,
                    "transitionType": "Inbound",
                    "reason": reason,
                }),
            )
            .await;
        }
        for stage in STAGES {
            let reported = inbound_transition(&router, stage).await.unwrap_or_else(|| {
                panic!("case {case:?}: stage {stage} reports no inboundTransitionState")
            });
            let disabled = held == Some(stage);
            assert_eq!(
                reported["enabled"].as_bool(),
                Some(!disabled),
                "case {case:?}: stage {stage} enabled: expected {}, got {reported}",
                !disabled
            );
            assert_eq!(
                reported.get("disabledReason").and_then(Value::as_str),
                disabled.then_some(reason),
                "case {case:?}: stage {stage} disabledReason, got {reported}"
            );
        }
        // Enabling it again restores the state every stage started in.
        if let Some(held) = held {
            ok(
                &router,
                "EnableStageTransition",
                &json!({
                    "pipelineName": PIPELINE,
                    "stageName": held,
                    "transitionType": "Inbound",
                }),
            )
            .await;
            let reported = inbound_transition(&router, held).await.unwrap_or_else(|| {
                panic!("case {case:?}: stage {held} reports no inboundTransitionState")
            });
            assert_eq!(
                reported["enabled"].as_bool(),
                Some(true),
                "case {case:?}: enabling restores stage {held}, got {reported}"
            );
        }
    }
}

/// RULE: a stage transition is changed only by a principal whose own identity-based
/// policies allow that `codepipeline:` action on the STAGE it names; one no policy allows
/// it for is refused with `AccessDeniedException` and changes nothing.
#[tokio::test]
async fn a_transition_is_changed_only_by_a_principal_whose_policies_allow_it() {
    let arn = pipeline_arn();
    // The resource a stage-transition decision is taken on: the stage's own ARN.
    let stage_arn = format!("{arn}/{}", STAGES[1]);
    let other = format!(
        "arn:aws:codepipeline:{REGION}:{}:another/{}",
        ::engine::emulator_account_id(),
        STAGES[1]
    );
    // The rows differ along what the rule generalises over: whether the principal carries
    // a policy at all, the effect it states, whether its `Action` is the exact action or a
    // wildcard, whether its `Resource` is this stage, this pipeline's stages, another
    // pipeline's or every resource, and whether `Action`/`Resource` are a string or a list.
    let rows: Vec<(&str, Option<String>, bool)> = vec![
        ("no policy at all", None, false),
        (
            "an allow of this service's actions on every resource",
            Some(r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
              "Action":"codepipeline:*","Resource":"*"}]}"#.to_string()),
            true,
        ),
        (
            "an allow of exactly these actions on this stage",
            Some(format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
                "Action":["codepipeline:DisableStageTransition",
                          "codepipeline:EnableStageTransition"],
                "Resource":["{stage_arn}"]}}]}}"#
            )),
            true,
        ),
        (
            "an allow of every stage of this pipeline",
            Some(format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
                "Action":"codepipeline:*","Resource":"{arn}/*"}}]}}"#
            )),
            true,
        ),
        (
            "an allow naming the pipeline but none of its stages",
            Some(format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
                "Action":"codepipeline:*","Resource":"{arn}"}}]}}"#
            )),
            false,
        ),
        (
            "an allow of another pipeline's transitions",
            Some(format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
                "Action":"codepipeline:*","Resource":"{other}"}}]}}"#
            )),
            false,
        ),
        (
            "an allow of another action on this stage",
            Some(format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
                "Action":"codepipeline:GetPipeline","Resource":"{stage_arn}"}}]}}"#
            )),
            false,
        ),
        (
            "a wildcard allow with an explicit deny of this stage",
            Some(format!(
                r#"{{"Version":"2012-10-17","Statement":[
                {{"Effect":"Allow","Action":"codepipeline:*","Resource":"*"}},
                {{"Effect":"Deny","Action":"codepipeline:*","Resource":"{stage_arn}"}}]}}"#
            )),
            false,
        ),
        (
            "an allow of every action of every service",
            Some(r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow",
              "Action":"*","Resource":"*"}]}"#.to_string()),
            true,
        ),
    ];

    for (index, (case, document, allowed)) in rows.iter().enumerate() {
        for operation in TRANSITION_OPERATIONS {
            let router = emulator();
            create_role(&router).await;
            ok(&router, "CreatePipeline", &json!({"pipeline": declaration()})).await;
            // The transition starts disabled for the ENABLE row, so a served call has
            // something to change and a refused one can be told apart from it.
            let enabling = operation == TRANSITION_OPERATIONS[1];
            if enabling {
                ok(
                    &router,
                    "DisableStageTransition",
                    &json!({
                        "pipelineName": PIPELINE,
                        "stageName": STAGES[1],
                        "transitionType": "Inbound",
                        "reason": "Set up by the fixture",
                    }),
                )
                .await;
            }
            let user = format!("caller-{index}");
            let key = user_with_key(&router, &user, document.as_deref()).await;
            let mut body = json!({
                "pipelineName": PIPELINE,
                "stageName": STAGES[1],
                "transitionType": "Inbound",
            });
            if !enabling {
                body["reason"] = json!("Held by the restricted caller");
            }
            let (status, answer) = call_signed_by(&router, &key, operation, &body).await;
            let reported = inbound_transition(&router, STAGES[1])
                .await
                .unwrap_or_else(|| panic!("case {case:?}: no inboundTransitionState"));
            if *allowed {
                assert_eq!(
                    status, 200,
                    "case {case:?} on {operation}: a principal its policies allow is \
                     served, got {status} with {answer}"
                );
                assert_eq!(
                    reported["enabled"].as_bool(),
                    Some(enabling),
                    "case {case:?} on {operation}: the served call changed the \
                     transition, got {reported}"
                );
                continue;
            }
            let code = answer
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?} on {operation}: expected a refusal, got {answer}")
                })
                .rsplit('#')
                .next()
                .unwrap_or_default();
            assert_eq!(
                (status, code),
                (400, ACCESS_DENIED),
                "case {case:?} on {operation}: expected a 400 {ACCESS_DENIED}, got \
                 {status} with {answer}"
            );
            let message = answer
                .get("Message")
                .or_else(|| answer.get("message"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                message.contains(&format!("is not authorized to perform: codepipeline:{operation}"))
                    && message.contains(stage_arn.as_str()),
                "case {case:?} on {operation}: the refusal names the action and the \
                 resource it was taken on, got {message:?}"
            );
            // The refused call changed nothing: the transition is still in the state the
            // fixture left it in.
            assert_eq!(
                reported["enabled"].as_bool(),
                Some(!enabling),
                "case {case:?} on {operation}: a refused call changes no transition, got \
                 {reported}"
            );
        }
    }
}
