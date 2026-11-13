// SPDX-License-Identifier: CC0-1.0

//! WHO may drive EC2's account-wide reads — the `Describe` family the CDK environment
//! context providers and every other read-only client walk — driven end to end over the
//! wire the service speaks (`ec2Query`), with the role, its policies and the session held
//! by the IAM and STS services mounted on the SAME engine, so the documents under test are
//! the ones `iam:PutRolePolicy` / `iam:AttachRolePolicy` store and the principal a refusal
//! blames is the one `sts:AssumeRole` minted.
//!
//! THE RULE UNDER TEST: an EC2 `Describe` action supports no resource-level permissions
//! ("the Amazon EC2 API actions that begin with `Describe` do not support resource-level
//! permissions", Amazon EC2 User Guide), so IAM judges it against EVERY resource: EC2
//! serves such a request only when the identity-based policies in force on the principal
//! its credentials name allow `ec2:<OperationName>` over every resource, and otherwise
//! refuses it with HTTP 403 `UnauthorizedOperation` — the other side of the permission
//! probe the `DryRun` member documents ("If you have the required permissions, the error
//! response is DryRunOperation. Otherwise, it is UnauthorizedOperation") — naming the
//! principal, the action and which of IAM's rules withheld the grant, without reading or
//! changing anything.
//!
//! The tables span what the rule generalises over: the operation (four of the `Describe`
//! family), the documents in force (none at all, an attached AWS-managed policy, an inline
//! allow of one action, of the whole service, of the action in another case, a grant scoped
//! to one resource ARN, an explicit deny of one action and a wildcard deny over the
//! family), the kind of credential (an assumed-role session, an IAM user's long-term access
//! key, the account's own published credential) and whether the request carries `DryRun`.

use std::sync::Arc;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The IAM API version a client sends in an awsQuery body.
const IAM_API_VERSION: &str = "2010-05-08";

/// The STS API version a client sends in an awsQuery body.
const STS_API_VERSION: &str = "2011-06-15";

/// The AWS-managed policy a `cdk bootstrap` lookup role carries, which grants the read
/// verbs of every service — the one attachment the positive rows are served through.
const READ_ONLY_ACCESS: &str = "arn:aws:iam::aws:policy/ReadOnlyAccess";

/// EC2's error code for a caller that may not perform the action it asked for.
const UNAUTHORIZED_OPERATION: &str = "UnauthorizedOperation";

/// EC2's error code for a `DryRun` whose caller DOES hold the action.
const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// The four `Describe` operations the rows drive: the reads the CDK VPC context provider
/// walks, which is also the span of the rule — each is its own `ec2:` action, so a grant or
/// a deny of one says nothing about the others.
const READS: [&str; 4] = [
    "DescribeAvailabilityZones",
    "DescribeRouteTables",
    "DescribeSubnets",
    "DescribeVpcs",
];

/// What a row expects the call to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force allow the action.
    Served,
    /// HTTP 403 `UnauthorizedOperation` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `UnauthorizedOperation` because no statement about the request allows it.
    NoAllow,
}

/// The emulator the rows drive: EC2 beside the IAM and STS services on ONE shared engine,
/// so the policies a row stores and the session it mints are the ones EC2's gate reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(iam::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// One query-protocol call to `service`, presented as signed by the access key id `akid` —
/// the only place the credential appears on the wire, and so what the service reads the
/// caller's identity from.
///
/// The request itself is built by the harness every suite shares
/// ([`super::ec2_query::query_call_as`]), so this suite carries no second copy of the
/// envelope or of its encoding.
async fn call_as(
    router: &MultiServiceRouter,
    service: &str,
    api_version: &str,
    akid: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    super::ec2_query::query_call_as(router, REGION, service, api_version, akid, action, params)
        .await
}

/// One EC2 call signed by `akid`.
async fn ec2_as(
    router: &MultiServiceRouter,
    akid: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    call_as(router, "ec2", EC2_API_VERSION, akid, action, params).await
}

/// One IAM call, made by the account's own published credential — the identity that
/// provisions the roles and users the rows are about.
async fn iam_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    call_as(
        router,
        "iam",
        IAM_API_VERSION,
        ACCOUNT_KEY_ID,
        action,
        params,
    )
    .await
}

/// The text of the first `<tag>` element of a query response, or a panic naming the case.
fn element(case: &str, body: &str, tag: &str) -> String {
    super::ec2_query::element(body, tag)
        .unwrap_or_else(|| panic!("case {case:?}: no <{tag}> in {body}"))
}

/// This emulator's account id, which every ARN below carries.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// A trust policy admitting the account's own principals to assume the role, which is what
/// lets the published credential mint the session a row signs with.
fn trust_policy() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{}:root"}},"Action":"sts:AssumeRole"}}]}}"#,
        account()
    )
}

/// A one-statement identity-based document: `effect` on `action` over `resource`.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

/// Create the role `role_name` with `inline` policies put on it and `attached` managed
/// policies attached, then assume it: answers the session's access key id and the ARN
/// `sts:GetCallerIdentity` reports for it, which a refusal must name.
async fn session_of(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    inline: &[(&str, String)],
    attached: &[&str],
) -> (String, String) {
    let trust = trust_policy();
    let (status, created) = iam_call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", &trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the role must be created, got {status} {created}"
    );
    for (policy_name, document) in inline {
        let (status, put) = iam_call(
            router,
            "PutRolePolicy",
            &[
                ("RoleName", role_name),
                ("PolicyName", policy_name),
                ("PolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the inline policy {policy_name:?} must be stored, got {status} {put}"
        );
    }
    for policy_arn in attached {
        let (status, attachment) = iam_call(
            router,
            "AttachRolePolicy",
            &[("RoleName", role_name), ("PolicyArn", policy_arn)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {policy_arn} must be attached, got {status} {attachment}"
        );
    }
    let role_arn = format!("arn:aws:iam::{}:role/{role_name}", account());
    let session_name = format!("{role_name}-session");
    let (status, assumed) = call_as(
        router,
        "sts",
        STS_API_VERSION,
        ACCOUNT_KEY_ID,
        "AssumeRole",
        &[("RoleArn", &role_arn), ("RoleSessionName", &session_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the role must be assumable, got {status} {assumed}"
    );
    (
        element(case, &assumed, "AccessKeyId"),
        element(case, &assumed, "Arn"),
    )
}

/// Create the IAM user `user_name` with `inline` policies put on it and mint its long-term
/// access key: answers that key's id and the user's ARN.
async fn user_key_of(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    inline: &[(&str, String)],
) -> (String, String) {
    let (status, created) = iam_call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the user must be created, got {status} {created}"
    );
    let user_arn = element(case, &created, "Arn");
    for (policy_name, document) in inline {
        let (status, put) = iam_call(
            router,
            "PutUserPolicy",
            &[
                ("UserName", user_name),
                ("PolicyName", policy_name),
                ("PolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the inline policy {policy_name:?} must be stored, got {status} {put}"
        );
    }
    let (status, minted) = iam_call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the access key must be minted, got {status} {minted}"
    );
    (element(case, &minted, "AccessKeyId"), user_arn)
}

/// The refusal EC2 words for `principal` asking for `action`, with the clause the rule the
/// decision was taken under states.
fn refusal_message(principal: &str, action: &str, expected: Expect) -> String {
    let reason = match expected {
        Expect::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        Expect::NoAllow => format!("because no identity-based policy allows the {action} action"),
        Expect::Served => unreachable!("a served call carries no refusal"),
    };
    format!(
        "You are not authorized to perform this operation. User: {principal} is not \
         authorized to perform: {action} {reason}"
    )
}

/// Assert that the call `read` signed by `akid` answered what `expected` requires of it,
/// and — when it was refused — that the refusal carries EC2's code and sentence.
async fn assert_read(
    router: &MultiServiceRouter,
    case: &str,
    akid: &str,
    principal: &str,
    read: &str,
    expected: Expect,
) {
    let (status, body) = ec2_as(router, akid, read, &[]).await;
    match expected {
        Expect::Served => assert_eq!(
            status, 200,
            "case {case:?} / {read}: expected the read to be served, got {status} {body}"
        ),
        refused => {
            assert_eq!(
                status, 403,
                "case {case:?} / {read}: expected HTTP 403, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                UNAUTHORIZED_OPERATION,
                "case {case:?} / {read}: expected {UNAUTHORIZED_OPERATION}, got {body}"
            );
            let action = format!("ec2:{read}");
            assert_eq!(
                error_message(&body),
                refusal_message(principal, &action, refused),
                "case {case:?} / {read}: expected IAM's refusal wording, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The documents in force decide the read
// ---------------------------------------------------------------------------

/// One row: the documents in force on the role the caller is a session of, and what each of
/// the four reads must answer.
struct Row {
    case: &'static str,
    role: &'static str,
    inline: Vec<(&'static str, String)>,
    attached: Vec<&'static str>,
    /// What the four [`READS`] answer, in that order.
    expect: [Expect; 4],
}

/// RULE: an assumed-role session's EC2 `Describe` is decided by the identity-based
/// documents in force on its ROLE, judged over every resource — an explicit `Deny` refuses
/// whatever any `Allow` says, an `Allow` covering the action over every resource serves it,
/// and with neither the request falls to IAM's default deny.
#[tokio::test]
async fn the_documents_in_force_decide_an_account_wide_read() {
    let vpc_arn = format!("arn:aws:ec2:{REGION}:{}:vpc/vpc-0123456789abcdef0", account());
    let rows = vec![
        Row {
            case: "a role carrying the read-only managed policy",
            role: "read-only-role",
            inline: Vec::new(),
            attached: vec![READ_ONLY_ACCESS],
            expect: [Expect::Served; 4],
        },
        Row {
            case: "that role with an inline deny of one read",
            role: "denied-lookup-role",
            inline: vec![(
                "deny-describe-vpcs",
                statement("Deny", "ec2:DescribeVpcs", "*"),
            )],
            attached: vec![READ_ONLY_ACCESS],
            // The deny names ONE action, so the other three reads are untouched.
            expect: [
                Expect::Served,
                Expect::Served,
                Expect::Served,
                Expect::ExplicitDeny,
            ],
        },
        Row {
            case: "a role carrying no policy at all",
            role: "bare-role",
            inline: Vec::new(),
            attached: Vec::new(),
            expect: [Expect::NoAllow; 4],
        },
        Row {
            case: "a role allowed every action of this service",
            role: "ec2-full-role",
            inline: vec![("allow-ec2", statement("Allow", "ec2:*", "*"))],
            attached: Vec::new(),
            expect: [Expect::Served; 4],
        },
        Row {
            case: "a role allowed exactly one of the reads",
            role: "subnets-only-role",
            inline: vec![(
                "allow-describe-subnets",
                statement("Allow", "ec2:DescribeSubnets", "*"),
            )],
            attached: Vec::new(),
            expect: [
                Expect::NoAllow,
                Expect::NoAllow,
                Expect::Served,
                Expect::NoAllow,
            ],
        },
        Row {
            case: "a role allowed the read in another case",
            // "IAM compares action names case-insensitively", so the spelling of the
            // statement's action is not what decides the call.
            role: "mixed-case-role",
            inline: vec![(
                "allow-mixed-case",
                statement("Allow", "EC2:describevpcs", "*"),
            )],
            attached: Vec::new(),
            expect: [
                Expect::NoAllow,
                Expect::NoAllow,
                Expect::NoAllow,
                Expect::Served,
            ],
        },
        Row {
            case: "a role allowed everything under a wildcard deny over the family",
            role: "wildcard-denied-role",
            inline: vec![
                ("allow-ec2", statement("Allow", "ec2:*", "*")),
                ("deny-describes", statement("Deny", "ec2:Describe*", "*")),
            ],
            attached: Vec::new(),
            expect: [Expect::ExplicitDeny; 4],
        },
        Row {
            case: "a role whose grant is scoped to one resource ARN",
            // A `Describe` action supports no resource-level permissions, so a statement
            // naming particular ARNs is not about a request IAM judges over every
            // resource — it grants nothing here, exactly as it grants nothing on AWS.
            role: "resource-scoped-role",
            inline: vec![(
                "allow-one-vpc",
                statement("Allow", "ec2:DescribeVpcs", &vpc_arn),
            )],
            attached: Vec::new(),
            expect: [Expect::NoAllow; 4],
        },
    ];

    let router = emulator();
    for row in &rows {
        let (akid, principal) =
            session_of(&router, row.case, row.role, &row.inline, &row.attached).await;
        for (read, expected) in READS.iter().zip(row.expect.iter()) {
            assert_read(&router, row.case, &akid, &principal, read, *expected).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Which credential the request presented
// ---------------------------------------------------------------------------

/// RULE: the principal a request is judged on is the one its access key id speaks for —
/// the session STS minted the key for, the IAM user that holds a long-term key, or the
/// account's own published credential, which holds no IAM policies and is therefore not
/// bounded by them.
#[tokio::test]
async fn the_credential_decides_which_principal_the_read_is_judged_on() {
    let router = emulator();

    // The account's own published credential: a key neither store holds a record for.
    for read in READS {
        let (status, body) = ec2_as(&router, ACCOUNT_KEY_ID, read, &[]).await;
        assert_eq!(
            status, 200,
            "case \"the account's own credential\" / {read}: expected the read to be \
             served, got {status} {body}"
        );
    }

    // An IAM user holding no policy at all is refused; the same user, once a policy
    // allowing the family is put on it, is served.
    let (user_akid, user_arn) = user_key_of(&router, "an IAM user with no policy", "bare-user", &[])
        .await;
    for read in READS {
        assert_read(
            &router,
            "an IAM user with no policy",
            &user_akid,
            &user_arn,
            read,
            Expect::NoAllow,
        )
        .await;
    }
    let (status, put) = iam_call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "bare-user"),
            ("PolicyName", "allow-describes"),
            ("PolicyDocument", &statement("Allow", "ec2:Describe*", "*")),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the user's policy must be stored, got {status} {put}"
    );
    for read in READS {
        assert_read(
            &router,
            "an IAM user allowed the family",
            &user_akid,
            &user_arn,
            read,
            Expect::Served,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// The permission probe, and what a refused read leaves behind
// ---------------------------------------------------------------------------

/// RULE: `DryRun` asks whether the caller holds the action's permissions — "If you have
/// the required permissions, the error response is DryRunOperation. Otherwise, it is
/// UnauthorizedOperation" — so the two sides of the decision are the two answers, for
/// every one of the reads.
#[tokio::test]
async fn a_dry_run_reports_which_side_of_the_decision_the_caller_is_on() {
    let router = emulator();
    let (allowed_akid, _) = session_of(
        &router,
        "a dry run by a permitted caller",
        "dry-run-allowed-role",
        &[("allow-ec2", statement("Allow", "ec2:*", "*"))],
        &[],
    )
    .await;
    let (denied_akid, denied_arn) = session_of(
        &router,
        "a dry run by a refused caller",
        "dry-run-denied-role",
        &[],
        &[],
    )
    .await;

    for read in READS {
        let (status, body) = ec2_as(&router, &allowed_akid, read, &[("DryRun", "true")]).await;
        assert_eq!(
            status, 412,
            "case \"a permitted caller\" / {read}: expected HTTP 412, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case \"a permitted caller\" / {read}: expected {DRY_RUN_OPERATION}, got {body}"
        );

        let (status, body) = ec2_as(&router, &denied_akid, read, &[("DryRun", "true")]).await;
        assert_eq!(
            status, 403,
            "case \"a refused caller\" / {read}: expected HTTP 403, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            UNAUTHORIZED_OPERATION,
            "case \"a refused caller\" / {read}: expected {UNAUTHORIZED_OPERATION}, got {body}"
        );
        let action = format!("ec2:{read}");
        assert_eq!(
            error_message(&body),
            refusal_message(&denied_arn, &action, Expect::NoAllow),
            "case \"a refused caller\" / {read}: expected IAM's refusal wording, got {body}"
        );
    }
}

/// RULE: the decision is taken before the operation reads anything, so a refused read
/// reports nothing of the account's estate and leaves it exactly as it was — the same
/// resource is still reported to a caller the policies allow.
#[tokio::test]
async fn a_refused_read_reports_nothing_and_changes_nothing() {
    let router = emulator();
    let case = "a VPC the account holds";
    let (status, created) = ec2_as(
        &router,
        ACCOUNT_KEY_ID,
        "CreateVpc",
        &[("CidrBlock", "10.42.0.0/16")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the VPC must be created, got {status} {created}"
    );
    let vpc_id = element(case, &created, "vpcId");

    let (denied_akid, denied_arn) = session_of(
        &router,
        case,
        "estate-denied-role",
        &[(
            "deny-describe-vpcs",
            statement("Deny", "ec2:DescribeVpcs", "*"),
        )],
        &[READ_ONLY_ACCESS],
    )
    .await;
    let (status, refused) = ec2_as(
        &router,
        &denied_akid,
        "DescribeVpcs",
        &[("VpcId.1", &vpc_id)],
    )
    .await;
    assert_eq!(
        status, 403,
        "case {case:?}: the denied read must be refused, got {status} {refused}"
    );
    assert!(
        !refused.contains(&vpc_id),
        "case {case:?}: a refused read discloses no VPC of the account, got {refused}"
    );
    assert_eq!(
        error_message(&refused),
        refusal_message(
            &denied_arn,
            "ec2:DescribeVpcs",
            Expect::ExplicitDeny
        ),
        "case {case:?}: expected IAM's refusal wording, got {refused}"
    );

    // Read back through the same operation as a caller the policies allow: the estate the
    // refusal did not disclose is still exactly what the account holds.
    let (status, reported) = ec2_as(
        &router,
        ACCOUNT_KEY_ID,
        "DescribeVpcs",
        &[("VpcId.1", &vpc_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the permitted read must be served, got {status} {reported}"
    );
    assert!(
        reported.contains(&vpc_id),
        "case {case:?}: expected {vpc_id} to still be reported, got {reported}"
    );
}
