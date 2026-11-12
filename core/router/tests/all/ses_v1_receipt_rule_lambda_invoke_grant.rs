// SPDX-License-Identifier: CC0-1.0

//! SES v1's `LambdaAction` pre-flight, driven through the WHOLE router: the receipt rule
//! set is created over the `awsQuery` surface, the function and its resource policy are
//! made through Lambda's OWN operations, and the rule is then stored — or refused — on
//! what Lambda really holds.
//!
//! Two rules, one trace:
//!
//! 1. A request is served by the ONE emulated service whose endpoint and wire form it
//!    addresses, and that service answers in its own modelled envelope — for an `awsQuery`
//!    service an XML document for EVERY response, a success and a modelled failure alike.
//!    Four mounted crates sign as `ses` (`smithy_query(query="service_info")` on each
//!    model: SES v1 `awsQuery`/`email`, SESv2 and Pinpoint Email `restJson1`/`email`,
//!    MailManager `awsJson1_0`/`mail-manager`), so a service claiming on the signing name
//!    alone leaves every request with four claimants and the router refuses it in its own
//!    JSON envelope — which an `awsQuery` client cannot parse at all, so the caller reads
//!    no status/code pair out of it.
//! 2. A service that acts on a caller's behalf on ANOTHER service's resource may be
//!    recorded as doing so only where that resource's own policy, as the owning service
//!    holds it, grants this service's principal the action FOR THIS resource; a request
//!    that would record an action the principal is not granted is refused with the error
//!    the addressed member models, and nothing is persisted. For a receipt rule's
//!    `LambdaAction` the owning service is Lambda, the principal is `ses.amazonaws.com`,
//!    the action is `lambda:InvokeFunction` ("To enable Amazon SES to call your Amazon Web
//!    Services Lambda function … Amazon SES must have permission to access those
//!    resources", `LambdaAction`), the resource a grant is scoped to is the receipt rule
//!    being stored, and the refusal is `InvalidLambdaFunction` at HTTP 400
//!    (`smithy_query(query="operation", operation="CreateReceiptRule")`:
//!    `InvalidLambdaFunctionException`, `awsQueryError` code `InvalidLambdaFunction`,
//!    `httpError` 400).
//!
//! The rows span what rule 2 generalises over — the FORM of the grant the owning service
//! holds: no function at all, a function with no policy, a grant for this rule, an
//! unscoped grant, a grant whose action is a wildcard over the service's actions, a grant
//! naming a different action, a grant scoped to another rule of the same set, one scoped
//! to a rule of another set, one naming another principal, and one scoped to another
//! account. Each row drives the SAME call; only what Lambda holds differs.
//!
//! The owning crate's own `ses_receipt_rule_destination_preflight` suite drives this
//! pre-flight over a router holding SES v1, Lambda and Amazon SNS alone. What this suite
//! adds is the assembled router, where the signing name `ses` really has four claimants
//! and where the pre-flight's in-process read of Lambda has to resolve among every mounted
//! service: both halves of the trace at once, so a rule that is refused here is refused on
//! what Lambda holds and not because the request never reached SES v1.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and
//! every request driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

use crate::query_wire::{elements, opt_element};
use crate::ses_family_signing_name_routing::{
    error_code, send, Call, BOOT_STACK_SIZE, REGION,
};
use crate::ses_v1_receipt_rule_sets_through_the_router::ses_v1;

/// The rule set every rule below is added to.
const RULE_SET: &str = "lambda-action-rules";

/// The service principal Amazon SES carries out a receipt rule's actions under, and so the
/// principal a destination's resource policy has to name to grant it.
const SES_PRINCIPAL: &str = "ses.amazonaws.com";

/// The IAM action an invocation of a function performs ("requires permission for the
/// `lambda:InvokeFunction` action"), which is what a grant for this pre-flight has to name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// A deployment package as `Code.ZipFile` carries it. Its content does not matter here:
/// what matters is that Lambda accepts the function and holds a policy for it.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// An account that is not the one this emulator serves, for the row whose grant is scoped
/// to someone else's account. Twelve digits, as `SourceAccount` requires
/// (`smithy_query(query="operation", operation="AddPermission")`: `SourceOwner`, pattern
/// `^\d{12}$`).
const ANOTHER_ACCOUNT: &str = "210987654321";

/// The account this emulator serves, which owns every function and rule set here.
fn account() -> String {
    engine::EMULATOR_ACCOUNT_ID.to_string()
}

/// An execution role for every created function: a role is normally provisioned outside
/// the client that creates the function, so it is a value the create carries.
fn role_arn() -> String {
    format!("arn:aws:iam::{}:role/service-role/ses-receipt", account())
}

/// The ARN of the receipt rule `rule` of the rule set `rule_set`, which is the value a
/// grant written for one receipt rule scopes itself to (`aws:SourceArn`).
fn rule_arn(rule_set: &str, rule: &str) -> String {
    format!(
        "arn:aws:ses:{REGION}:{}:receipt-rule-set/{rule_set}:receipt-rule/{rule}",
        account()
    )
}

/// One Lambda `restJson1` call through the router, as its status and parsed body.
fn lambda(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &'static str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let call = Call {
        case: "a Lambda control-plane call",
        method,
        uri: uri.to_string(),
        host: format!("lambda.{REGION}.amazonaws.com"),
        signing_name: Some("lambda"),
        content_type: Some("application/json"),
        target: None,
        body: body.to_string(),
    };
    let (status, answered) = send(router, runtime, &call);
    let parsed = serde_json::from_str(&answered).unwrap_or(Value::Null);
    (status, parsed)
}

/// Create the function `name` through Lambda's own `CreateFunction`, and answer the ARN
/// Lambda minted for it — the value the row's `LambdaAction` then names.
fn create_function(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    name: &str,
) -> String {
    let (status, created) = lambda(
        router,
        runtime,
        "POST",
        "/2015-03-31/functions",
        &json!({
            "FunctionName": name,
            "Role": role_arn(),
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": ZIP_FILE },
        }),
    );
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda CreateFunction, got {status} with \
         {created}"
    );
    created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateFunction must mint an ARN, got {created}"))
        .to_string()
}

/// Add one statement to `name`'s resource-based policy through Lambda's own
/// `AddPermission`, which is how a client grants a service principal the invocation.
fn add_permission(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    name: &str,
    grant: &Grant,
) {
    let mut body = json!({
        "StatementId": "ses-receipt-invoke",
        "Action": grant.action,
        "Principal": grant.principal,
    });
    if let Some(source_arn) = &grant.source_arn {
        body["SourceArn"] = json!(source_arn);
    }
    if let Some(source_account) = grant.source_account {
        body["SourceAccount"] = json!(source_account);
    }
    let (status, answered) = lambda(
        router,
        runtime,
        "POST",
        &format!("/2015-03-31/functions/{name}/policy"),
        &body,
    );
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from Lambda AddPermission, got {status} with \
         {answered}"
    );
}

/// The statement, if any, a row has Lambda hold for its function.
struct Grant {
    /// The `Action` the statement names.
    action: &'static str,
    /// The `Principal` it grants to.
    principal: &'static str,
    /// The `SourceArn` it is scoped by, or `None` for an unscoped grant.
    source_arn: Option<String>,
    /// The `SourceAccount` it is scoped by, or `None`.
    source_account: Option<&'static str>,
}

/// What Lambda holds for the row's function.
enum Held {
    /// No function at all: the ARN the action names is one Lambda holds nothing under.
    NoFunction,
    /// The function exists and carries no resource policy.
    NoPolicy,
    /// The function exists and carries exactly this one statement.
    Granted(Grant),
}

/// One row: what Lambda holds, and whether the rule may therefore be stored.
struct Row {
    case: &'static str,
    /// The receipt rule's own name, which also names the row's function.
    rule: &'static str,
    held: Held,
    /// Whether the pre-flight must let the rule be stored.
    stored: bool,
}

/// A statement granting `principal` `action`, scoped by `source_arn` and `source_account`.
fn grant(
    action: &'static str,
    principal: &'static str,
    source_arn: Option<String>,
    source_account: Option<&'static str>,
) -> Held {
    Held::Granted(Grant {
        action,
        principal,
        source_arn,
        source_account,
    })
}

/// The forms of grant the rule generalises over. The call each row drives is identical:
/// only the statement Lambda holds for the function differs.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a grant to this service principal for this very rule",
            rule: "grants-this-rule",
            held: grant(
                INVOKE_ACTION,
                SES_PRINCIPAL,
                Some(rule_arn(RULE_SET, "grants-this-rule")),
                None,
            ),
            stored: true,
        },
        Row {
            case: "a grant to this service principal scoped to nothing",
            rule: "grants-unscoped",
            held: grant(INVOKE_ACTION, SES_PRINCIPAL, None, None),
            stored: true,
        },
        Row {
            case: "a grant whose action is a wildcard over the service's actions",
            rule: "grants-wildcard-action",
            held: grant(
                "lambda:*",
                SES_PRINCIPAL,
                Some(rule_arn(RULE_SET, "grants-wildcard-action")),
                None,
            ),
            stored: true,
        },
        Row {
            case: "a function Lambda holds no resource policy for",
            rule: "holds-no-policy",
            held: Held::NoPolicy,
            stored: false,
        },
        Row {
            case: "a grant naming another action of the same service",
            rule: "grants-another-action",
            held: grant(
                "lambda:GetFunction",
                SES_PRINCIPAL,
                Some(rule_arn(RULE_SET, "grants-another-action")),
                None,
            ),
            stored: false,
        },
        Row {
            case: "a grant scoped to another rule of the same set",
            rule: "grants-another-rule",
            held: grant(
                INVOKE_ACTION,
                SES_PRINCIPAL,
                Some(rule_arn(RULE_SET, "some-other-rule")),
                None,
            ),
            stored: false,
        },
        Row {
            case: "a grant scoped to a rule of another set",
            rule: "grants-another-rule-set",
            held: grant(
                INVOKE_ACTION,
                SES_PRINCIPAL,
                Some(rule_arn("some-other-rule-set", "grants-another-rule-set")),
                None,
            ),
            stored: false,
        },
        Row {
            case: "a grant to another service principal",
            rule: "grants-another-principal",
            held: grant(
                INVOKE_ACTION,
                "sns.amazonaws.com",
                Some(rule_arn(RULE_SET, "grants-another-principal")),
                None,
            ),
            stored: false,
        },
        Row {
            case: "a grant scoped to another account",
            rule: "grants-another-account",
            held: grant(INVOKE_ACTION, SES_PRINCIPAL, None, Some(ANOTHER_ACCOUNT)),
            stored: false,
        },
        Row {
            case: "a well-formed function ARN Lambda holds nothing under",
            rule: "holds-no-function",
            held: Held::NoFunction,
            stored: false,
        },
    ]
}

/// `CreateReceiptRule` carrying one `LambdaAction` naming `function_arn`, over the
/// `awsQuery` surface.
fn create_rule(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    rule: &str,
    function_arn: &str,
) -> (u16, String) {
    ses_v1(
        router,
        runtime,
        case,
        "CreateReceiptRule",
        &[
            ("RuleSetName", RULE_SET),
            ("Rule.Name", rule),
            ("Rule.Enabled", "true"),
            ("Rule.ScanEnabled", "false"),
            (
                "Rule.Actions.member.1.LambdaAction.FunctionArn",
                function_arn,
            ),
            ("Rule.Actions.member.1.LambdaAction.InvocationType", "Event"),
        ],
    )
}

/// The `FunctionArn` the stored rule `rule` carries, or `None` when the set holds no rule
/// of that name at all.
fn described_function_arn(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    case: &str,
    rule: &str,
) -> Option<String> {
    let (status, body) = ses_v1(
        router,
        runtime,
        case,
        "DescribeReceiptRule",
        &[("RuleSetName", RULE_SET), ("RuleName", rule)],
    );
    if status != 200 {
        return None;
    }
    Some(opt_element(&body, "FunctionArn").unwrap_or_else(|| {
        panic!("case {case:?}: the stored rule carries no FunctionArn: {body}")
    }))
}

/// The rule names `DescribeReceiptRuleSet` reports for [`RULE_SET`], sorted — the set's
/// ORDER is another suite's business; what this one reads is which rules it holds.
fn held_rules(
    router: &smithy_server_runtime::MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
) -> Vec<String> {
    let (status, body) = ses_v1(
        router,
        runtime,
        "the rules the set holds",
        "DescribeReceiptRuleSet",
        &[("RuleSetName", RULE_SET)],
    );
    assert_eq!(
        status, 200,
        "case \"the rules the set holds\": expected HTTP 200, got {status}: {body}"
    );
    let mut names = elements(&body, "Name");
    // `Metadata.Name` is the SET's own name, which is not one of its rules.
    names.retain(|name| name != RULE_SET);
    names.sort();
    names
}

#[test]
fn a_lambda_action_is_stored_only_where_lambda_grants_this_rule_the_invocation() {
    std::thread::Builder::new()
        .name("ses-v1-lambda-action-grant".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            // The rows exercise the SERVICES, so authentication is off: what they pin is
            // which service answered, in what envelope, and what it stored. The sibling
            // suite `ses_family_pre_dispatch_refusals` drives this family with
            // authentication ON.
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                // The TIME driver this trace needs: invoking a Lambda function starts that
                // service's concurrency sampler, which waits on the clock from a background
                // task, and a runtime with no timer panics inside tokio when it does.
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // The trace's first SES v1 call: the rule set every rule below is added to,
            // served by SES v1 itself in its own XML envelope.
            let (status, body) = ses_v1(
                &router,
                &runtime,
                "the rule set the rules are added to",
                "CreateReceiptRuleSet",
                &[("RuleSetName", RULE_SET)],
            );
            assert_eq!(
                status, 200,
                "case \"the rule set the rules are added to\": expected HTTP 200, got \
                 {status}: {body}"
            );

            let mut expected_rules: Vec<String> = Vec::new();
            for row in rows() {
                let case = row.case;
                let rule = row.rule;
                let function = format!("{rule}-function");
                let function_arn = match &row.held {
                    Held::NoFunction => format!(
                        "arn:aws:lambda:{REGION}:{}:function:{function}",
                        account()
                    ),
                    Held::NoPolicy => create_function(&router, &runtime, case, &function),
                    Held::Granted(grant) => {
                        let arn = create_function(&router, &runtime, case, &function);
                        add_permission(&router, &runtime, case, &function, grant);
                        arn
                    }
                };

                let (status, body) = create_rule(&router, &runtime, case, rule, &function_arn);
                if row.stored {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: Lambda grants this rule the invocation, so the rule \
                         must be stored, got HTTP {status}: {body}"
                    );
                    let reported = described_function_arn(&router, &runtime, case, rule);
                    assert_eq!(
                        reported.as_deref(),
                        Some(function_arn.as_str()),
                        "case {case:?}: expected the stored rule to carry \
                         {function_arn:?}, got {reported:?}"
                    );
                    expected_rules.push(rule.to_string());
                } else {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: an action Amazon SES is not granted is refused 400, \
                         got HTTP {status}: {body}"
                    );
                    let code = error_code(&body);
                    assert_eq!(
                        code, "InvalidLambdaFunction",
                        "case {case:?}: expected the modelled InvalidLambdaFunction code, \
                         got {code:?}: {body}"
                    );
                    let expected_message =
                        format!("Could not invoke Lambda function: {function_arn}");
                    let messages = elements(&body, "Message");
                    assert!(
                        messages.contains(&expected_message),
                        "case {case:?}: expected the message {expected_message:?}, got \
                         {messages:?}: {body}"
                    );
                    let reported = described_function_arn(&router, &runtime, case, rule);
                    assert_eq!(
                        reported, None,
                        "case {case:?}: a refused create must persist no rule, got \
                         {reported:?}"
                    );
                }

                // Whatever the row asked for, the set holds exactly the rules the stored
                // rows put in it: a refusal neither records its own rule nor disturbs one
                // an earlier row stored.
                let held = held_rules(&router, &runtime);
                let mut expected = expected_rules.clone();
                expected.sort();
                assert_eq!(
                    held, expected,
                    "case {case:?}: the set must hold exactly {expected:?}, got {held:?}"
                );
            }
        })
        .expect("spawn the lambda-action thread")
        .join()
        .expect("a LambdaAction must be stored only where Lambda grants this rule the invocation");
}
