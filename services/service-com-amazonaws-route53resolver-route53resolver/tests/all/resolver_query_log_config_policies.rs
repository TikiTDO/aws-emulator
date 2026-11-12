// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Resolver query-logging policy slice:
//! `PutResolverQueryLogConfigPolicy` and `GetResolverQueryLogConfigPolicy`.
//!
//! The rule under test: a resource policy is attached to the query logging
//! configuration its `Arn` names and is reported back, exactly as written, by a
//! read of that configuration's policy; while no write has attached one there is
//! no policy to read at all, so the read is refused with the modelled
//! `AccessDeniedException`. Each suite drives a TABLE of documents spanning the
//! forms the policy grammar admits, so no case is tied to one recorded
//! document.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, emulator, error_type, expect_error, ok, string_member, Emulator, ACCOUNT, REGION,
};

/// An account other than the one this emulator's resources belong to, so a
/// document that names it shares the configuration with somebody else.
const OTHER_ACCOUNT: &str = "123456789012";

/// The Route 53 Resolver service error code the read of a policy that is not
/// there is refused with, and the sentence it carries — the message the service
/// words that fault with, as the recorded response of that read states it.
const CODE_ACCESS_DENIED: &str = "RSLVR-01600";

/// Create a query logging configuration and return `(Id, Arn)`.
///
/// The destination it delivers to is one the account holds, which the create
/// resolves against the store of the service that owns it.
async fn query_log_config(router: &Emulator, creator: &str) -> (String, String) {
    router.hold_query_log_destinations(["arn:aws:s3:::amzn-s3-demo-bucket"]);
    let created = ok(
        router,
        "CreateResolverQueryLogConfig",
        json!({
            "CreatorRequestId": creator,
            "Name": creator,
            "DestinationArn": "arn:aws:s3:::amzn-s3-demo-bucket",
        }),
    )
    .await;
    let config = created
        .get("ResolverQueryLogConfig")
        .expect("the create answers a ResolverQueryLogConfig");
    (string_member(config, "Id"), string_member(config, "Arn"))
}

/// The `ResolverQueryLogConfigPolicy` a read of `arn` reports.
async fn stored_policy(router: &MultiServiceRouter, arn: &str) -> String {
    let read = ok(
        router,
        "GetResolverQueryLogConfigPolicy",
        json!({ "Arn": arn }),
    )
    .await;
    string_member(&read, "ResolverQueryLogConfigPolicy")
}

/// Assert a read of `arn`'s policy is refused as a policy that is not there:
/// the modelled `AccessDeniedException`, carrying the service error code and
/// the sentence that states the fault.
async fn expect_read_denied(router: &MultiServiceRouter, case: &str, arn: &str) {
    let (status, parsed) = call(
        router,
        "GetResolverQueryLogConfigPolicy",
        json!({ "Arn": arn }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: the read must answer HTTP 400, got {status}: {parsed}"
    );
    assert!(
        error_type(&parsed).contains("AccessDeniedException"),
        "case {case:?}: the read must answer AccessDeniedException, got {parsed}"
    );
    let message = string_member(&parsed, "Message");
    assert_eq!(
        message,
        format!("[{CODE_ACCESS_DENIED}] Access denied"),
        "case {case:?}: the refusal must state the access-denied fault, got {message:?}"
    );
}

/// Write `policy` for `arn` and assert the call reported success.
async fn put_policy(router: &MultiServiceRouter, case: &str, arn: &str, policy: &str) {
    let written = ok(
        router,
        "PutResolverQueryLogConfigPolicy",
        json!({ "Arn": arn, "ResolverQueryLogConfigPolicy": policy }),
    )
    .await;
    assert_eq!(
        written.get("ReturnValue"),
        Some(&Value::Bool(true)),
        "case {case:?}: PutResolverQueryLogConfigPolicy must report ReturnValue true, got {written}"
    );
}

/// One statement sharing `arn` with `principal`, in the grammar's statement
/// form: an `Effect`, an `Action` list and the `Resource` the doc asks for ("in
/// the Resource section of the statement, you specify the ARNs for the query
/// logging configurations that you want to share").
///
/// The actions are the ones
/// `PutResolverQueryLogConfigPolicy$ResolverQueryLogConfigPolicy` documents:
/// `route53resolver:AssociateResolverQueryLogConfig`,
/// `route53resolver:DisassociateResolverQueryLogConfig` and
/// `route53resolver:ListResolverQueryLogConfigs`.
fn share_statement(principal: Value, arn: &str) -> Value {
    json!({
        "Effect": "Allow",
        "Principal": principal,
        "Action": [
            "route53resolver:AssociateResolverQueryLogConfig",
            "route53resolver:DisassociateResolverQueryLogConfig",
        ],
        "Resource": arn,
    })
}

/// A whole document carrying `statements`.
fn policy_document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// Every document the policy grammar admits is stored as written, and the read
/// reports exactly that document — whatever form the grammar lets it take.
#[tokio::test]
async fn a_written_policy_is_reported_back_as_written() {
    let router = emulator();
    let (_, arn) = query_log_config(&router, "policy-written").await;

    // Each row is a different shape of document: whom it names as principal
    // and how, how many statements it carries, and which optional elements it
    // states.
    let cases: Vec<(&str, String)> = vec![
        (
            "principal by account id",
            policy_document(json!([share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn)])),
        ),
        (
            "principal by account root ARN",
            policy_document(json!([share_statement(
                json!({ "AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root") }),
                &arn,
            )])),
        ),
        (
            "principal list naming the owner and another account",
            policy_document(json!([share_statement(
                json!({ "AWS": [ACCOUNT, OTHER_ACCOUNT] }),
                &arn,
            )])),
        ),
        (
            "wildcard principal",
            policy_document(json!([share_statement(json!("*"), &arn)])),
        ),
        (
            "service principal",
            policy_document(json!([share_statement(
                json!({ "Service": "ram.amazonaws.com" }),
                &arn,
            )])),
        ),
        (
            "two statements",
            policy_document(json!([
                share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn),
                share_statement(json!({ "AWS": "210987654321" }), &arn),
            ])),
        ),
        (
            "one statement, not a list",
            policy_document(share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn)),
        ),
        (
            "statement with a Sid and a Condition, and a single Action",
            json!({
                "Statement": [{
                    "Sid": "ShareOneQueryLogConfig",
                    "Effect": "Allow",
                    "Principal": { "AWS": OTHER_ACCOUNT },
                    "Action": "route53resolver:ListResolverQueryLogConfigs",
                    "Resource": arn,
                    "Condition": { "StringEquals": { "aws:PrincipalOrgID": "o-example" } },
                }],
            })
            .to_string(),
        ),
        (
            "no Version element",
            json!({ "Statement": [share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn)] })
                .to_string(),
        ),
        (
            "a long document: one statement per shared account",
            policy_document(Value::Array(
                (0..40)
                    .map(|index| {
                        let account = format!("{:012}", 100_000_000_000u64 + index);
                        share_statement(json!({ "AWS": account }), &arn)
                    })
                    .collect(),
            )),
        ),
        // The emulator serves one account, so a caller that shares a
        // configuration has only its own account to name: that document is a
        // document like any other and is stored as written.
        (
            "principal by the owning account's id",
            policy_document(json!([share_statement(json!({ "AWS": ACCOUNT }), &arn)])),
        ),
        (
            "principal by the owning account's root ARN",
            policy_document(json!([share_statement(
                json!({ "AWS": format!("arn:aws:iam::{ACCOUNT}:root") }),
                &arn,
            )])),
        ),
        (
            "a statement with no Principal",
            policy_document(json!([{
                "Effect": "Allow",
                "Action": ["route53resolver:ListResolverQueryLogConfigs"],
                "Resource": arn,
            }])),
        ),
        ("an empty statement list", policy_document(json!([]))),
        ("no Statement element", json!({}).to_string()),
    ];

    for (case, policy) in cases {
        put_policy(&router, case, &arn, &policy).await;
        let read = stored_policy(&router, &arn).await;
        assert_eq!(
            read, policy,
            "case {case:?}: the read must report the document that was written, got {read:?}"
        );
        // A read reports the policy without changing it: a second read of the
        // same configuration answers the same document.
        let reread = stored_policy(&router, &arn).await;
        assert_eq!(
            reread, policy,
            "case {case:?}: a second read must report the same document, got {reread:?}"
        );
    }
}

/// A second write replaces the document the configuration carried: the read
/// reports the document the LAST write attached, never a merge of the two and
/// never the one before it.
#[tokio::test]
async fn a_second_write_replaces_the_document() {
    let router = emulator();
    let (_, arn) = query_log_config(&router, "policy-replaced").await;

    // The rows are walked in order, each overwriting the one before it, and
    // they differ along what a replacement can change: whom the document names,
    // how many statements it carries, and whether it grants anything at all.
    let cases: Vec<(&str, String)> = vec![
        (
            "one shared account",
            policy_document(json!([share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn)])),
        ),
        (
            "two shared accounts",
            policy_document(json!([
                share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn),
                share_statement(json!({ "AWS": "210987654321" }), &arn),
            ])),
        ),
        (
            "another single account",
            policy_document(json!([share_statement(json!({ "AWS": "210987654321" }), &arn)])),
        ),
        (
            "the owning account",
            policy_document(json!([share_statement(json!({ "AWS": ACCOUNT }), &arn)])),
        ),
        ("a document granting nothing", policy_document(json!([]))),
    ];

    for (case, policy) in cases {
        put_policy(&router, case, &arn, &policy).await;
        let read = stored_policy(&router, &arn).await;
        assert_eq!(
            read, policy,
            "case {case:?}: the read must report the document the last write attached, got {read:?}"
        );
    }
}

/// A configuration nobody has written a policy for has no policy to read, so
/// the read is refused rather than answered with a document — and the refusal
/// changes nothing, so a second read is refused the same way.
#[tokio::test]
async fn a_configuration_with_no_policy_refuses_the_read() {
    let router = emulator();
    let (_, arn) = query_log_config(&router, "policy-absent").await;

    expect_read_denied(&router, "a configuration with no policy", &arn).await;
    expect_read_denied(&router, "the same configuration, read again", &arn).await;
}

/// A document outside the policy grammar is refused with
/// `InvalidPolicyDocument`, and the policy the configuration already carried
/// stays.
#[tokio::test]
async fn an_invalid_document_is_refused_and_stores_nothing() {
    let router = emulator();
    let (_, arn) = query_log_config(&router, "policy-invalid").await;

    let shared = policy_document(json!([share_statement(
        json!({ "AWS": OTHER_ACCOUNT }),
        &arn,
    )]));
    put_policy(&router, "the policy to preserve", &arn, &shared).await;

    // Rows differ in which part of the grammar the document misses: it is not
    // JSON at all, it is JSON but not an object, or one of its statements is
    // not a statement.
    let cases: Vec<(&str, String)> = vec![
        ("the empty string", String::new()),
        ("not JSON", "this is not a policy".to_string()),
        ("truncated JSON", "{\"Statement\": [".to_string()),
        (
            "a JSON array",
            json!([share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn)]).to_string(),
        ),
        ("a JSON string", json!("Allow").to_string()),
        ("a JSON number", "17".to_string()),
        (
            "a statement that is not an object",
            policy_document(json!(["route53resolver:ListResolverQueryLogConfigs"])),
        ),
        (
            "a statement with no Effect",
            policy_document(json!([{
                "Principal": { "AWS": OTHER_ACCOUNT },
                "Action": ["route53resolver:ListResolverQueryLogConfigs"],
                "Resource": arn,
            }])),
        ),
        (
            "a statement with no Action",
            policy_document(json!([{
                "Effect": "Allow",
                "Principal": { "AWS": OTHER_ACCOUNT },
                "Resource": arn,
            }])),
        ),
        (
            "a second statement with no Effect",
            policy_document(json!([
                share_statement(json!({ "AWS": OTHER_ACCOUNT }), &arn),
                { "Action": ["route53resolver:ListResolverQueryLogConfigs"], "Resource": arn },
            ])),
        ),
    ];

    for (case, policy) in cases {
        expect_error(
            &router,
            "PutResolverQueryLogConfigPolicy",
            json!({ "Arn": arn, "ResolverQueryLogConfigPolicy": policy }),
            case,
            "InvalidPolicyDocument",
        )
        .await;
        let read = stored_policy(&router, &arn).await;
        assert_eq!(
            read, shared,
            "case {case:?}: a refused document must leave the stored policy alone, got {read:?}"
        );
    }
}

/// An `Arn` the store holds no query logging configuration under is refused
/// with `UnknownResourceException` by both operations, and the write persists
/// nothing for it.
#[tokio::test]
async fn an_arn_naming_no_configuration_is_refused() {
    let router = emulator();
    let (live_id, live_arn) = query_log_config(&router, "policy-live").await;

    // Rows differ in how the ARN misses a stored configuration: a
    // configuration id that was never minted, another resource type, another
    // region, another account, and the configuration's own id instead of its
    // ARN.
    let cases: Vec<(&str, String)> = vec![
        (
            "a configuration id nothing was created under",
            format!(
                "arn:aws:route53resolver:{REGION}:{ACCOUNT}:resolver-query-log-config/\
                 rqlc-0000000000000000"
            ),
        ),
        (
            "another resource type",
            live_arn.replace("resolver-query-log-config/", "resolver-rule/"),
        ),
        ("another region", live_arn.replace(REGION, "eu-west-1")),
        ("another account", live_arn.replace(ACCOUNT, OTHER_ACCOUNT)),
        ("the configuration id, not its ARN", live_id.clone()),
    ];

    let policy = policy_document(json!([share_statement(
        json!({ "AWS": OTHER_ACCOUNT }),
        &live_arn,
    )]));

    for (case, arn) in cases {
        expect_error(
            &router,
            "PutResolverQueryLogConfigPolicy",
            json!({ "Arn": arn, "ResolverQueryLogConfigPolicy": policy }),
            case,
            "UnknownResourceException",
        )
        .await;
        expect_error(
            &router,
            "GetResolverQueryLogConfigPolicy",
            json!({ "Arn": arn }),
            case,
            "UnknownResourceException",
        )
        .await;
        // The refusal names the ARN that was asked for, so the caller can see
        // which resource the service found nothing under.
        let (_, refusal) = call(
            &router,
            "GetResolverQueryLogConfigPolicy",
            json!({ "Arn": arn.clone() }),
        )
        .await;
        let message = string_member(&refusal, "Message");
        assert!(
            message.contains(&arn),
            "case {case:?}: the refusal must name the ARN {arn:?}, got {message:?} ({})",
            error_type(&refusal)
        );
    }

    // The configuration that does exist is untouched by every refusal above: it
    // still carries no policy at all.
    expect_read_denied(&router, "the configuration no refusal wrote to", &live_arn).await;
}
