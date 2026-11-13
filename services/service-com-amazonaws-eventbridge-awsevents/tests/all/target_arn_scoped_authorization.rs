// SPDX-License-Identifier: CC0-1.0

//! A call that ATTACHES targets to a rule is authorized with the ARN of every target it
//! attaches in the condition context — end to end over the wire through the real
//! `MultiServiceRouter`, with IAM and the token service mounted beside EventBridge.
//!
//! The RULE asserted here: attaching targets is the action `events:PutTargets` on the rule's
//! ARN, and the call publishes this service's own request condition key `events:TargetArn`,
//! one value per entry of the submitted target list. So a statement scoped by that key is
//! DECIDED by the ARNs the request carries — it grants the calls whose targets satisfy it,
//! refuses only the calls whose targets do not, and a refused call leaves the rule's target
//! list exactly as it was.
//!
//! The rows span the class rather than the one recorded case:
//!
//! * [`a_target_arn_conditioned_statement_is_decided_by_the_submitted_target_arns`] varies
//!   the condition OPERATOR (ARN equality, ARN wildcard, a negated ARN operator, a string
//!   operator over the same value, the existence operator), the SET qualifier (none,
//!   `ForAnyValue:`, `ForAllValues:`), the statement's EFFECT, the NUMBER of entries the
//!   request carries (one and two) and the SERVICE each target belongs to (Amazon SQS,
//!   Amazon SNS, Lambda) — plus a grant conditioned on a key this call publishes no value
//!   for, which must grant nothing.
//! * [`each_call_is_decided_against_the_targets_that_call_carries`] holds one conditioned
//!   grant fixed and makes two calls under it, which is what makes the decision a fact of the
//!   request rather than of the rule or of the principal.
//!
//! The principal is an assumed ROLE, because that is the identity a least-privilege policy
//! ('this role may only wire rules to queues') is attached to. Nothing is ever published to
//! the attached targets here: this suite is about who may WIRE a rule to what, so the target
//! ARNs name resources no delivery of this suite reaches.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element reader this suite needs are the ones
// `tests/all/target_dispatch.rs` owns for this test binary, so a request is built and read
// exactly one way across the crate.
use crate::target_dispatch::{form_encode, xml_text};

/// The Region every call below is signed for.
const REGION: &str = "us-east-2";

/// The access key id the account's own administrative calls present: the account holds no
/// record of it, so it resolves to the account's own published credential, which no statement
/// of this suite bounds.
const ACCOUNT_KEY: &str = "AKIDTESTTESTTEST";

/// The bus the rules below live on, and the rule the targets are attached to.
const BUS: &str = "target-scoped-bus";
const RULE: &str = "wiring";

/// The role the scoped policy is attached to, the session name it is assumed under, and the
/// name of its inline policy.
const ROLE_NAME: &str = "target-wirer";
const SESSION_NAME: &str = "probe";
const POLICY_NAME: &str = "wire-allowed-targets";

/// The pattern every rule below is created with.
const PATTERN: &str = r#"{"source":["com.demo.orders"]}"#;

/// The request condition key a target-attaching call publishes, as AWS names it.
const TARGET_ARN_KEY: &str = "events:TargetArn";

/// The action every call of this suite asks for.
const ACTION: &str = "events:PutTargets";

/// EventBridge, IAM and the token service over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: the role a session is minted of is the record IAM holds, and this service
/// reads both.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite signs nothing: the `Authorization` header carries only the credential scope,
    // which is where a service reads the Region and the caller's access key id.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    Emulator { router }
}

/// The emulator's account id, which every ARN below carries.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The ARN of the rule the targets are attached to, the form this service mints.
fn rule_arn() -> String {
    format!("arn:aws:events:{REGION}:{}:rule/{BUS}/{RULE}", account())
}

/// The ARN of the IAM role the scoped policy is attached to.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/{ROLE_NAME}", account())
}

/// The ARN of an Amazon SQS queue named `name` — the target class the grants below admit.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{}:{name}", account())
}

/// The ARN of an Amazon SNS topic named `name` — a target of another service, which needs no
/// `RoleArn` either, so what decides the call is the authorization and nothing else.
fn topic_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{}:{name}", account())
}

/// The ARN of a Lambda function named `name`, a third target service.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{}:function:{name}", account())
}

/// One EventBridge `awsJson1_1` call presenting `access_key_id`, as its status and parsed
/// body. The access key id is what selects the principal the request speaks for.
async fn events_as(
    emulator: &Emulator,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed test request");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&payload).to_string()))
    };
    (status, parsed)
}

/// One EventBridge call the account's own credential makes, which must be served.
async fn events(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = events_as(emulator, ACCOUNT_KEY, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} by the account's own credential answered {status} {answered}"
    );
    answered
}

/// One awsQuery call against the service whose signing name is `signing_name`, presenting
/// `access_key_id`, as its status and raw body.
async fn query(
    emulator: &Emulator,
    access_key_id: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    parameters: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = emulator.router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// Create the role carrying `policy` as its whole inline policy, assume it through the token
/// service, and answer `(access key id, the principal ARN STS reports)` — which is the
/// identity a refusal names.
async fn assumed_role(emulator: &Emulator, case: &str, policy: &Value) -> (String, String) {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", account()) },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, body) = query(
        emulator,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole answered {status} {body}"
    );
    let (status, body) = query(
        emulator,
        ACCOUNT_KEY,
        "iam",
        "2010-05-08",
        "PutRolePolicy",
        &[
            ("RoleName", ROLE_NAME.to_string()),
            ("PolicyName", POLICY_NAME.to_string()),
            ("PolicyDocument", policy.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy answered {status} {body}"
    );
    let (status, body) = query(
        emulator,
        ACCOUNT_KEY,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[
            ("RoleArn", role_arn()),
            ("RoleSessionName", SESSION_NAME.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssumeRole answered {status} {body}"
    );
    let key = xml_text(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("case {case:?}: AssumeRole answered no AccessKeyId: {body}"));
    // The principal the session speaks for, read from the answer rather than spelled here: it
    // is what `sts:GetCallerIdentity` reports and what a refusal blames.
    let principal = xml_text(&body, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: AssumeRole answered no Arn: {body}"));
    (key, principal)
}

/// An identity-based document holding `statements`.
fn document(statements: Vec<Value>) -> Value {
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// One identity-based statement: `effect` on [`ACTION`] over the rule the suite attaches
/// targets to, scoped by `condition`.
fn statement(effect: &str, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": effect,
        "Action": ACTION,
        "Resource": rule_arn(),
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// A document whose only statement is an `Allow` scoped by `condition`.
fn scoped_allow(condition: Option<Value>) -> Value {
    document(vec![statement("Allow", condition)])
}

/// A clause over `key` under `operator`, the way a policy writes one.
fn on_key(operator: &str, key: &str, value: &str) -> Value {
    json!({ operator: { key: value } })
}

/// A clause over the target-ARN request key under `operator`.
fn on_target_arn(operator: &str, value: &str) -> Value {
    on_key(operator, TARGET_ARN_KEY, value)
}

/// Create [`BUS`] and the rule [`RULE`] on it, with no targets yet.
async fn given_rule(emulator: &Emulator, case: &str) {
    events(emulator, case, "CreateEventBus", &json!({ "Name": BUS })).await;
    events(
        emulator,
        case,
        "PutRule",
        &json!({ "Name": RULE, "EventBusName": BUS, "EventPattern": PATTERN }),
    )
    .await;
}

/// The `(Id, Arn)` of every target the rule carries, in the order it pages them — read with
/// the account's own credential, so the read is never the one being refused.
async fn attached_targets(emulator: &Emulator, case: &str) -> Vec<(String, String)> {
    let listed = events(
        emulator,
        case,
        "ListTargetsByRule",
        &json!({ "Rule": RULE, "EventBusName": BUS }),
    )
    .await;
    let targets = listed["Targets"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListTargetsByRule answered no Targets: {listed}"))
        .clone();
    targets
        .iter()
        .map(|target| {
            let text = |member: &str| {
                target[member]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a listed target carries no {member}: {target}")
                    })
                    .to_string()
            };
            (text("Id"), text("Arn"))
        })
        .collect()
}

/// The wire error code of a failure body (`__type` on awsJson), without its namespace.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` a failure body carries.
fn message(body: &Value) -> &str {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Which of IAM's rules withheld the grant, which is what a refusal's closing clause names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// Nothing in force allowed the call.
    NoGrant,
    /// An explicit deny in the caller's identity-based policy.
    ExplicitDeny,
}

impl Refusal {
    /// The sentence this refusal carries for `principal` asking for [`ACTION`] on the rule.
    fn message(self, principal: &str) -> String {
        let reason = match self {
            Self::NoGrant => format!("because no identity-based policy allows the {ACTION} action"),
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        };
        format!(
            "User: {principal} is not authorized to perform: {ACTION} on resource: {} {reason}",
            rule_arn()
        )
    }
}

/// One row: the document in force on the caller, the targets the call attaches, and the
/// verdict.
struct Row {
    case: &'static str,
    /// The caller's whole identity-based document.
    policy: Value,
    /// The `Targets` the call carries, as `(Id, Arn)` in request order.
    targets: Vec<(&'static str, String)>,
    /// The refusal the call earns, or `None` when it is served.
    refusal: Option<Refusal>,
}

/// The rows. They differ along what the rule generalises over: the condition operator, the
/// set qualifier, the statement's effect, how many entries the request carries and which
/// service each target belongs to.
///
/// The operators are IAM's own
/// (`ArnEquals`/`ArnLike`/`ArnNotLike` — "case-sensitive matching of the ARN"), the set
/// qualifiers `ForAllValues:`/`ForAnyValue:`, and the existence operator `Null`.
fn rows() -> Vec<Row> {
    let queue = queue_arn("orders");
    let other_queue = queue_arn("shipments");
    let topic = topic_arn("announcements");
    let function = function_arn("handler");
    let any_queue = "arn:aws:sqs:*";
    vec![
        Row {
            case: "ForAnyValue:ArnLike over the one queue target it covers",
            policy: scoped_allow(Some(on_target_arn("ForAnyValue:ArnLike", any_queue))),
            targets: vec![("sqs", queue.clone())],
            refusal: None,
        },
        Row {
            case: "ForAnyValue:ArnLike over one topic target, outside the allowed service",
            policy: scoped_allow(Some(on_target_arn("ForAnyValue:ArnLike", any_queue))),
            targets: vec![("sns", topic.clone())],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "ForAnyValue:ArnLike over one function target, outside the allowed service",
            policy: scoped_allow(Some(on_target_arn("ForAnyValue:ArnLike", any_queue))),
            targets: vec![("lambda", function.clone())],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "ForAnyValue:ArnLike over two targets, one of which it covers",
            policy: scoped_allow(Some(on_target_arn("ForAnyValue:ArnLike", any_queue))),
            targets: vec![("sns", topic.clone()), ("sqs", queue.clone())],
            refusal: None,
        },
        Row {
            case: "ForAllValues:ArnLike over two targets, one of which it does not cover",
            policy: scoped_allow(Some(on_target_arn("ForAllValues:ArnLike", any_queue))),
            targets: vec![("sqs", queue.clone()), ("sns", topic.clone())],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "ForAllValues:ArnLike over two targets it covers",
            policy: scoped_allow(Some(on_target_arn("ForAllValues:ArnLike", any_queue))),
            targets: vec![("sqs", queue.clone()), ("second", other_queue.clone())],
            refusal: None,
        },
        Row {
            case: "an unqualified ArnLike over the single target it covers",
            policy: scoped_allow(Some(on_target_arn("ArnLike", any_queue))),
            targets: vec![("sqs", queue.clone())],
            refusal: None,
        },
        Row {
            case: "an unqualified ArnLike over the single target it does not cover",
            policy: scoped_allow(Some(on_target_arn("ArnLike", any_queue))),
            targets: vec![("sns", topic.clone())],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            // Two entries give the key two values, which a single-valued comparison states
            // nothing about: the statement is undecided, and an undecided `Allow` is no
            // grant.
            case: "an unqualified ArnLike over two targets, which it cannot compare",
            policy: scoped_allow(Some(on_target_arn("ArnLike", any_queue))),
            targets: vec![("sqs", queue.clone()), ("second", other_queue.clone())],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "ForAnyValue:ArnEquals on the exact target ARN",
            policy: scoped_allow(Some(on_target_arn("ForAnyValue:ArnEquals", &queue))),
            targets: vec![("sqs", queue.clone())],
            refusal: None,
        },
        Row {
            case: "ForAnyValue:ArnEquals on another queue's ARN",
            policy: scoped_allow(Some(on_target_arn("ForAnyValue:ArnEquals", &queue))),
            targets: vec![("second", other_queue.clone())],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "ForAllValues:ArnNotLike excluding the topic service, over a queue target",
            policy: scoped_allow(Some(on_target_arn(
                "ForAllValues:ArnNotLike",
                "arn:aws:sns:*",
            ))),
            targets: vec![("sqs", queue.clone())],
            refusal: None,
        },
        Row {
            case: "ForAllValues:ArnNotLike excluding the topic service, over a topic target",
            policy: scoped_allow(Some(on_target_arn(
                "ForAllValues:ArnNotLike",
                "arn:aws:sns:*",
            ))),
            targets: vec![("sns", topic.clone())],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "ForAnyValue:StringLike reading the same key as text",
            policy: scoped_allow(Some(on_target_arn("ForAnyValue:StringLike", any_queue))),
            targets: vec![("sqs", queue.clone())],
            refusal: None,
        },
        Row {
            case: "Null asserting the key is present, which every attaching call carries",
            policy: scoped_allow(Some(on_target_arn("Null", "false"))),
            targets: vec![("sqs", queue.clone())],
            refusal: None,
        },
        Row {
            case: "Null asserting the key is absent",
            policy: scoped_allow(Some(on_target_arn("Null", "true"))),
            targets: vec![("sqs", queue.clone())],
            refusal: Some(Refusal::NoGrant),
        },
        Row {
            case: "an unconditioned Allow with a Deny scoped to the topic service, over a queue",
            policy: document(vec![
                statement("Allow", None),
                statement(
                    "Deny",
                    Some(on_target_arn("ForAnyValue:ArnLike", "arn:aws:sns:*")),
                ),
            ]),
            targets: vec![("sqs", queue.clone())],
            refusal: None,
        },
        Row {
            case: "an unconditioned Allow with a Deny scoped to the topic service, over a topic",
            policy: document(vec![
                statement("Allow", None),
                statement(
                    "Deny",
                    Some(on_target_arn("ForAnyValue:ArnLike", "arn:aws:sns:*")),
                ),
            ]),
            targets: vec![("sns", topic.clone())],
            refusal: Some(Refusal::ExplicitDeny),
        },
        Row {
            // The key family this call publishes is the one it carries values for; a grant
            // conditioned on another of this service's keys is undecided, so it grants
            // nothing.
            case: "a grant conditioned on a key this call publishes no value for",
            policy: scoped_allow(Some(on_key(
                "ForAnyValue:ArnLike",
                "events:ApiDestinationArn",
                "arn:aws:events:*",
            ))),
            targets: vec![("sqs", queue.clone())],
            refusal: Some(Refusal::NoGrant),
        },
    ]
}

/// The `Targets` member of a `PutTargets` request carrying `targets`.
fn targets_member(targets: &[(&'static str, String)]) -> Value {
    Value::Array(
        targets
            .iter()
            .map(|(id, arn)| json!({ "Id": id, "Arn": arn }))
            .collect(),
    )
}

/// RULE — a statement scoped by `events:TargetArn` is decided by the target ARNs the request
/// carries: the call whose targets satisfy it is served 200 and its targets are readable
/// back, and the call whose targets do not is refused `AccessDeniedException` with AWS's
/// wording while the rule's target list stays empty.
#[tokio::test]
async fn a_target_arn_conditioned_statement_is_decided_by_the_submitted_target_arns() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        given_rule(&emulator, case).await;
        let (key, principal) = assumed_role(&emulator, case, &row.policy).await;

        let (status, answered) = events_as(
            &emulator,
            &key,
            "PutTargets",
            &json!({
                "Rule": RULE,
                "EventBusName": BUS,
                "Targets": targets_member(&row.targets),
            }),
        )
        .await;
        let attached = attached_targets(&emulator, case).await;
        match row.refusal {
            Some(refusal) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the call to be refused, got {status} {answered}"
                );
                let code = error_code(&answered);
                assert_eq!(
                    code, "AccessDeniedException",
                    "case {case:?}: expected the code AccessDeniedException, got {code:?} in \
                     {answered}"
                );
                let expected = refusal.message(&principal);
                let reported = message(&answered);
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected the refusal {expected:?}, got {reported:?}"
                );
                assert!(
                    attached.is_empty(),
                    "case {case:?}: a refused call must attach nothing, got {attached:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the call to be served, got {status} {answered}"
                );
                assert_eq!(
                    answered["FailedEntryCount"],
                    json!(0),
                    "case {case:?}: expected no failed entry, got {answered}"
                );
                let expected: Vec<(String, String)> = row
                    .targets
                    .iter()
                    .map(|(id, arn)| ((*id).to_string(), arn.clone()))
                    .collect();
                assert_eq!(
                    attached, expected,
                    "case {case:?}: expected the rule to carry {expected:?}, got {attached:?}"
                );
            }
        }
    }
}

/// RULE — the decision is a fact of the CALL: under one conditioned grant held fixed, the
/// call carrying a covered target is served and the next call carrying an uncovered one is
/// refused, leaving the rule with exactly the target the first call attached.
#[tokio::test]
async fn each_call_is_decided_against_the_targets_that_call_carries() {
    let case = "one grant, two calls";
    let emulator = emulator();
    given_rule(&emulator, case).await;
    let queue = queue_arn("orders");
    let topic = topic_arn("announcements");
    let policy = scoped_allow(Some(on_target_arn("ForAnyValue:ArnLike", "arn:aws:sqs:*")));
    let (key, principal) = assumed_role(&emulator, case, &policy).await;

    let (status, answered) = events_as(
        &emulator,
        &key,
        "PutTargets",
        &json!({
            "Rule": RULE,
            "EventBusName": BUS,
            "Targets": [{ "Id": "sqs", "Arn": queue }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the covered call to be served, got {status} {answered}"
    );
    let after_first = attached_targets(&emulator, case).await;
    let expected = vec![("sqs".to_string(), queue.clone())];
    assert_eq!(
        after_first, expected,
        "case {case:?}: expected the rule to carry {expected:?}, got {after_first:?}"
    );

    let (status, answered) = events_as(
        &emulator,
        &key,
        "PutTargets",
        &json!({
            "Rule": RULE,
            "EventBusName": BUS,
            "Targets": [{ "Id": "sns", "Arn": topic }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the uncovered call to be refused, got {status} {answered}"
    );
    let reported = message(&answered);
    let refusal = Refusal::NoGrant.message(&principal);
    assert_eq!(
        reported, refusal,
        "case {case:?}: expected the refusal {refusal:?}, got {reported:?}"
    );
    let after_second = attached_targets(&emulator, case).await;
    assert_eq!(
        after_second, expected,
        "case {case:?}: a refused call must change nothing, expected {expected:?}, got \
         {after_second:?}"
    );
}
