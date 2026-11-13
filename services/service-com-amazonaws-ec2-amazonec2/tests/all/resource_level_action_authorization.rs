// SPDX-License-Identifier: CC0-1.0

//! WHO may drive an EC2 action that supports resource-level permissions — a create, a tag,
//! an allocation — driven end to end over the wire the service speaks (`ec2Query`), with
//! the role, its policies, its permissions boundary and its session held by the IAM and STS
//! services mounted on the SAME engine, so the documents under test are the ones
//! `iam:PutRolePolicy` / `iam:PutRolePermissionsBoundary` store and the principal a refusal
//! blames is the one `sts:AssumeRole` minted.
//!
//! THE RULE UNDER TEST: such an action is authorized against the resources the REQUEST
//! addresses, so EC2 serves it only when the identity-based documents in force on the
//! principal its credentials name (and the permissions boundary capping them) allow
//! `ec2:<OperationName>` on each of those resources, and otherwise refuses it with HTTP 403
//! `UnauthorizedOperation` naming the principal, the action, the resource it was refused ON
//! and which of IAM's rules withheld the grant — before the operation reads or writes
//! anything. Under `DryRun` the authorized side of that decision is answered HTTP 412
//! `DryRunOperation`: "If you have the required permissions, the error response is
//! DryRunOperation. Otherwise, it is UnauthorizedOperation" (the `DryRun` member's own
//! documentation), so the two codes are the two sides of the IAM decision.
//!
//! The tables span what the rule generalises over: the operation (five actions that support
//! resource-level permissions, addressing a subnet, a VPC and nothing at all), the documents
//! in force (none, another service's actions only, the whole service, the write verbs, a
//! grant scoped to the subnet the request names, one scoped to every resource of ANOTHER
//! kind, the service in another ASCII case, an allow of everything BUT the service, an
//! explicit deny and a wildcard deny), the permissions boundary capping them, and whether
//! the request carries `DryRun`.

use std::sync::Arc;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{elements, error_code, error_message};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in — the
/// identity that provisions the fixtures the rows are about.
const ACCOUNT_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

/// The EC2 API version a client sends in an ec2Query body.
const EC2_API_VERSION: &str = "2016-11-15";

/// The IAM API version a client sends in an awsQuery body.
const IAM_API_VERSION: &str = "2010-05-08";

/// The STS API version a client sends in an awsQuery body.
const STS_API_VERSION: &str = "2011-06-15";

/// EC2's error code for a caller that may not perform the action it asked for.
const UNAUTHORIZED_OPERATION: &str = "UnauthorizedOperation";

/// EC2's error code for a `DryRun` whose caller DOES hold the action.
const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// The sentence EC2 opens an `UnauthorizedOperation` refusal with.
const NOT_AUTHORIZED_SENTENCE: &str = "You are not authorized to perform this operation.";

/// What `DryRunOperation` states: the request was authorized and then stopped.
const DRY_RUN_MESSAGE: &str = "Request would have succeeded, but DryRun flag is set.";

/// How many actions each row of the tables below answers for.
const ACTION_COUNT: usize = 5;

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

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — a policy document is JSON, so every brace, quote
/// and colon in it has to survive the round trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// One query-protocol call to `service`, presented as signed by the access key id `akid` —
/// the only place the credential appears on the wire, and so what the service reads the
/// caller's identity from.
async fn call_as(
    router: &MultiServiceRouter,
    service: &str,
    api_version: &str,
    akid: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={api_version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{service}.{REGION}.amazonaws.com/"))
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{REGION}/{service}/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed query request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One EC2 call signed by `akid`.
async fn ec2_as(
    router: &MultiServiceRouter,
    akid: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call_as(router, "ec2", EC2_API_VERSION, akid, action, params).await
}

/// One EC2 call made by the account's own published credential, which holds no IAM policies
/// and is therefore not bounded by them: the fixtures and the read-backs go through it.
async fn ec2_as_account(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = ec2_as(router, ACCOUNT_KEY_ID, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {action} by the account's own credential must be served, got \
         {status} {body}"
    );
    body
}

/// One IAM call, made by the account's own published credential.
async fn iam_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
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

/// `(name, value)` form fields from string literals.
fn params(fields: &[(&str, &str)]) -> Vec<(String, String)> {
    fields
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
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

/// A one-statement document allowing everything EXCEPT `action`, through the negated
/// spelling of the action element.
fn allow_everything_but(action: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","NotAction":"{action}","Resource":"*"}}]}}"#
    )
}

/// Create the role `role_name` with `inline` policies put on it, then assume it: answers the
/// session's access key id and the ARN `sts:GetCallerIdentity` reports for it, which a
/// refusal must name.
async fn session_of(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
    inline: &[(&str, String)],
) -> (String, String) {
    let (status, created) = iam_call(
        router,
        "CreateRole",
        &[
            ("RoleName".to_string(), role_name.to_string()),
            ("AssumeRolePolicyDocument".to_string(), trust_policy()),
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
                ("RoleName".to_string(), role_name.to_string()),
                ("PolicyName".to_string(), (*policy_name).to_string()),
                ("PolicyDocument".to_string(), document.clone()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the inline policy {policy_name:?} must be stored, got {status} {put}"
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
        &[
            ("RoleArn".to_string(), role_arn),
            ("RoleSessionName".to_string(), session_name),
        ],
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

/// Attach `document` to `role_name` as its permissions boundary, through the managed policy
/// a boundary is always attached as.
async fn boundary_of(router: &MultiServiceRouter, case: &str, role_name: &str, document: &str) {
    let policy_name = format!("{role_name}-boundary");
    let (status, created) = iam_call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName".to_string(), policy_name),
            ("PolicyDocument".to_string(), document.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the boundary policy must be created, got {status} {created}"
    );
    let policy_arn = element(case, &created, "Arn");
    let (status, put) = iam_call(
        router,
        "PutRolePermissionsBoundary",
        &[
            ("RoleName".to_string(), role_name.to_string()),
            ("PermissionsBoundary".to_string(), policy_arn),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the boundary must be attached, got {status} {put}"
    );
}

/// One EC2 action that supports resource-level permissions: the members its request carries
/// beside `DryRun`, and the resource those members address.
struct Action {
    /// The operation name, which is also the `ec2:` action IAM judges it by.
    name: &'static str,
    /// The request's own members.
    members: Vec<(String, String)>,
    /// The ARN of the resource the request addresses, which the decision is taken on and a
    /// refusal names; `None` for a request that addresses none.
    judged_on: Option<String>,
}

/// The five actions the rows drive — a create of an interface (the action a dry-run
/// permission probe is the documented way to pre-flight), a create of a VPC, of a security
/// group, an address allocation and a tag write — spanning the resources a request can
/// address: a subnet, a VPC and nothing at all.
async fn resource_level_actions(router: &MultiServiceRouter) -> Vec<Action> {
    // The Region's default network, read back through the account's own credential: a
    // subnet to put an interface in, and the VPC that subnet sits in.
    let described = ec2_as_account(router, "the default network", "DescribeSubnets", &[]).await;
    let subnet_id = element("the default network", &described, "subnetId");
    let vpc_id = element("the default network", &described, "vpcId");
    let subnet_arn = ::engine::emulator_arn("ec2", REGION, &format!("subnet/{subnet_id}"));
    let vpc_arn = ::engine::emulator_arn("ec2", REGION, &format!("vpc/{vpc_id}"));
    let actions = vec![
        Action {
            name: "CreateNetworkInterface",
            members: vec![("SubnetId".to_string(), subnet_id.clone())],
            judged_on: Some(subnet_arn.clone()),
        },
        Action {
            name: "CreateVpc",
            members: params(&[("CidrBlock", "10.77.0.0/16")]),
            judged_on: None,
        },
        Action {
            name: "CreateSecurityGroup",
            members: vec![
                ("GroupName".to_string(), "probe-group".to_string()),
                ("GroupDescription".to_string(), "a probed group".to_string()),
                ("VpcId".to_string(), vpc_id),
            ],
            judged_on: Some(vpc_arn),
        },
        Action {
            name: "AllocateAddress",
            members: Vec::new(),
            judged_on: None,
        },
        Action {
            name: "CreateTags",
            members: vec![
                ("ResourceId.1".to_string(), subnet_id),
                ("Tag.1.Key".to_string(), "owner".to_string()),
                ("Tag.1.Value".to_string(), "probe".to_string()),
            ],
            judged_on: Some(subnet_arn),
        },
    ];
    assert_eq!(
        actions.len(),
        ACTION_COUNT,
        "every row states one expectation per action"
    );
    actions
}

/// The members of `action`, with `DryRun` set when `dry_run`.
fn request_of(action: &Action, dry_run: bool) -> Vec<(String, String)> {
    let mut members = action.members.clone();
    if dry_run {
        members.push(("DryRun".to_string(), "true".to_string()));
    }
    members
}

/// What a row expects the call to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 412 `DryRunOperation`: the caller holds the action on the resources the request
    /// addresses, so the probe reports that the request would have succeeded.
    HoldsTheAction,
    /// HTTP 403 `UnauthorizedOperation` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `UnauthorizedOperation` because no readable document allows the action on
    /// the resource.
    NoAllow,
    /// HTTP 403 `UnauthorizedOperation` because the permissions boundary allows no such
    /// grant.
    NoBoundaryAllow,
}

/// The refusal EC2 words for `principal` asking for `action` on `resource`, with the clause
/// the rule the decision was taken under states.
fn refusal_message(
    principal: &str,
    action: &str,
    resource: Option<&String>,
    expected: Expect,
) -> String {
    let reason = match expected {
        Expect::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        Expect::NoAllow => format!("because no identity-based policy allows the {action} action"),
        Expect::NoBoundaryAllow => {
            format!("because no permissions boundary allows the {action} action")
        }
        Expect::HoldsTheAction => unreachable!("a served call carries no refusal"),
    };
    match resource {
        // A decision taken on a resource the request addressed names it, and closes the
        // sentence.
        Some(resource) => format!(
            "{NOT_AUTHORIZED_SENTENCE} User: {principal} is not authorized to perform: \
             {action} on resource: {resource} {reason}."
        ),
        None => format!(
            "{NOT_AUTHORIZED_SENTENCE} User: {principal} is not \
             authorized to perform: {action} {reason}"
        ),
    }
}

/// Assert that the call of `action` signed by `akid` answered what `expected` requires, with
/// EC2's code, status and wording for each side of the decision.
async fn assert_answer(
    router: &MultiServiceRouter,
    case: &str,
    akid: &str,
    principal: &str,
    action: &Action,
    dry_run: bool,
    expected: Expect,
) {
    let (status, body) = ec2_as(router, akid, action.name, &request_of(action, dry_run)).await;
    let name = action.name;
    match expected {
        Expect::HoldsTheAction => {
            assert_eq!(
                status, 412,
                "case {case:?} / {name}: expected HTTP 412, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                DRY_RUN_OPERATION,
                "case {case:?} / {name}: expected {DRY_RUN_OPERATION}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                DRY_RUN_MESSAGE,
                "case {case:?} / {name}: expected the dry-run wording, got {body}"
            );
        }
        refused => {
            assert_eq!(
                status, 403,
                "case {case:?} / {name}: expected HTTP 403, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                UNAUTHORIZED_OPERATION,
                "case {case:?} / {name}: expected {UNAUTHORIZED_OPERATION}, got {body}"
            );
            let iam_action = format!("ec2:{name}");
            assert_eq!(
                error_message(&body),
                refusal_message(principal, &iam_action, action.judged_on.as_ref(), refused),
                "case {case:?} / {name}: expected IAM's refusal wording, got {body}"
            );
        }
    }
}

/// [`assert_answer`] for a `DryRun` of `action`: a dry run either way, so nothing a row
/// asserts about depends on what another row created.
async fn assert_probe(
    router: &MultiServiceRouter,
    case: &str,
    akid: &str,
    principal: &str,
    action: &Action,
    expected: Expect,
) {
    assert_answer(router, case, akid, principal, action, true, expected).await;
}

// ---------------------------------------------------------------------------
// A principal holding none of this service's actions
// ---------------------------------------------------------------------------

/// RULE: a caller whose readable documents hold no `Allow` of the requested
/// `ec2:<OperationName>` is permitted on no resource, so EC2 refuses it with HTTP 403
/// `UnauthorizedOperation` — whether or not the request carries `DryRun`, and for every
/// action that supports resource-level permissions.
#[tokio::test]
async fn a_principal_holding_no_ec2_action_is_refused_with_or_without_a_dry_run() {
    let router = emulator();
    let actions = resource_level_actions(&router).await;
    let case = "a role granted another service's actions only";
    let (akid, principal) = session_of(
        &router,
        case,
        "secrets-only-role",
        &[(
            "allow-secretsmanager",
            statement("Allow", "secretsmanager:*", "*"),
        )],
    )
    .await;

    for action in &actions {
        for dry_run in [true, false] {
            assert_answer(
                &router,
                case,
                &akid,
                &principal,
                action,
                dry_run,
                Expect::NoAllow,
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// The documents in force decide the action, on the resources the request names
// ---------------------------------------------------------------------------

/// One row of the table below: the documents in force on the role the caller is a session
/// of, and what each of the five actions must answer for it.
struct Row {
    case: &'static str,
    role: &'static str,
    inline: Vec<(&'static str, String)>,
    /// What the five actions answer, in the order [`resource_level_actions`] lists them.
    expect: [Expect; ACTION_COUNT],
}

/// RULE: the two sides of the permission probe are the two sides of IAM's decision — a
/// caller holding the action on the resources the request addresses is answered
/// `DryRunOperation` (HTTP 412), one that does not `UnauthorizedOperation` (HTTP 403) — and
/// a grant SCOPED to resource ARNs grants the action exactly on the resources it names, so
/// the same role can hold one of these actions and not another.
#[tokio::test]
async fn the_documents_in_force_decide_a_resource_level_action() {
    let router = emulator();
    let actions = resource_level_actions(&router).await;
    // The subnet the interface create and the tag write address, and every VPC of the
    // Region: two grants scoped to resources, each covering some of the actions below and
    // not others.
    let subnet = actions[0]
        .judged_on
        .clone()
        .expect("the interface create addresses a subnet");
    let every_vpc = ::engine::emulator_arn("ec2", REGION, "vpc/*");
    let rows: Vec<Row> = vec![
        Row {
            case: "a role carrying no policy at all",
            role: "bare-role",
            inline: Vec::new(),
            expect: [Expect::NoAllow; ACTION_COUNT],
        },
        Row {
            case: "a role granted another service only",
            role: "other-service-role",
            inline: vec![("allow-s3", statement("Allow", "s3:*", "*"))],
            expect: [Expect::NoAllow; ACTION_COUNT],
        },
        Row {
            case: "a role granted every action of this service",
            role: "ec2-full-role",
            inline: vec![("allow-ec2", statement("Allow", "ec2:*", "*"))],
            expect: [Expect::HoldsTheAction; ACTION_COUNT],
        },
        Row {
            case: "a role granted the write verbs of this service",
            role: "ec2-writer-role",
            inline: vec![
                ("allow-creates", statement("Allow", "ec2:Create*", "*")),
                ("allow-allocates", statement("Allow", "ec2:Allocate*", "*")),
            ],
            expect: [Expect::HoldsTheAction; ACTION_COUNT],
        },
        Row {
            case: "a role whose grant is scoped to the subnet the request names",
            role: "subnet-scoped-role",
            inline: vec![("allow-subnet", statement("Allow", "ec2:*", &subnet))],
            expect: [
                // The interface create and the tag write address that subnet…
                Expect::HoldsTheAction,
                // …a create that addresses no resource at all is not refused on a grant
                // that may well cover whatever it addresses…
                Expect::HoldsTheAction,
                // …and the group create addresses the VPC, which the grant does not name.
                Expect::NoAllow,
                Expect::HoldsTheAction,
                Expect::HoldsTheAction,
            ],
        },
        Row {
            case: "a role whose grant is scoped to every VPC",
            role: "vpc-scoped-role",
            inline: vec![("allow-vpcs", statement("Allow", "ec2:*", &every_vpc))],
            expect: [
                Expect::NoAllow,
                Expect::HoldsTheAction,
                Expect::HoldsTheAction,
                Expect::HoldsTheAction,
                Expect::NoAllow,
            ],
        },
        Row {
            case: "a role granted this service in another ASCII case",
            // "IAM compares action names case-insensitively", so the spelling of the
            // statement's action is not what decides the call.
            role: "mixed-case-role",
            inline: vec![("allow-mixed-case", statement("Allow", "EC2:*", "*"))],
            expect: [Expect::HoldsTheAction; ACTION_COUNT],
        },
        Row {
            case: "a role granted everything but this service's actions",
            role: "all-but-ec2-role",
            inline: vec![("allow-but-ec2", allow_everything_but("ec2:*"))],
            expect: [Expect::NoAllow; ACTION_COUNT],
        },
        Row {
            case: "a role allowed everything under an explicit deny of this service",
            role: "denied-role",
            inline: vec![
                ("allow-ec2", statement("Allow", "ec2:*", "*")),
                ("deny-ec2", statement("Deny", "ec2:*", "*")),
            ],
            expect: [Expect::ExplicitDeny; ACTION_COUNT],
        },
        Row {
            case: "a role allowed everything under a wildcard deny over the actions spelled \
                   with C",
            role: "wildcard-denied-role",
            inline: vec![
                ("allow-ec2", statement("Allow", "ec2:*", "*")),
                ("deny-c", statement("Deny", "ec2:C*", "*")),
            ],
            expect: [
                Expect::ExplicitDeny,
                Expect::ExplicitDeny,
                Expect::ExplicitDeny,
                // `AllocateAddress` is not one of the actions the deny names.
                Expect::HoldsTheAction,
                Expect::ExplicitDeny,
            ],
        },
    ];

    for row in &rows {
        let (akid, principal) = session_of(&router, row.case, row.role, &row.inline).await;
        for (action, expected) in actions.iter().zip(row.expect.iter()) {
            assert_probe(&router, row.case, &akid, &principal, action, *expected).await;
        }
    }
}

// ---------------------------------------------------------------------------
// The ceiling the permissions boundary sets
// ---------------------------------------------------------------------------

/// RULE: "a permissions boundary sets the maximum permissions that an identity-based policy
/// can grant", so a grant the boundary does not also allow is not a permission the
/// principal has — and the refusal names the boundary as the rule that withheld it.
#[tokio::test]
async fn a_permissions_boundary_caps_a_resource_level_grant() {
    let router = emulator();
    let actions = resource_level_actions(&router).await;
    let allow_ec2 = vec![("allow-ec2", statement("Allow", "ec2:*", "*"))];

    let case = "a boundary allowing another service only";
    let (capped_akid, capped_arn) = session_of(&router, case, "capped-role", &allow_ec2).await;
    boundary_of(
        &router,
        case,
        "capped-role",
        &statement("Allow", "secretsmanager:*", "*"),
    )
    .await;
    for action in &actions {
        assert_probe(
            &router,
            case,
            &capped_akid,
            &capped_arn,
            action,
            Expect::NoBoundaryAllow,
        )
        .await;
    }

    let case = "a boundary allowing this service";
    let (admitted_akid, admitted_arn) =
        session_of(&router, case, "admitted-role", &allow_ec2).await;
    boundary_of(
        &router,
        case,
        "admitted-role",
        &statement("Allow", "ec2:*", "*"),
    )
    .await;
    for action in &actions {
        assert_probe(
            &router,
            case,
            &admitted_akid,
            &admitted_arn,
            action,
            Expect::HoldsTheAction,
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// What a refused call leaves behind, and what a permitted one writes
// ---------------------------------------------------------------------------

/// RULE: the decision is taken before the operation runs, so a refused create writes
/// nothing — the interface it would have minted does not exist — while the same request by a
/// caller the documents allow is served and reads back through `DescribeNetworkInterfaces`.
#[tokio::test]
async fn a_refused_create_writes_nothing_and_a_permitted_one_is_read_back() {
    let router = emulator();
    let case = "a create of a network interface";
    let described = ec2_as_account(&router, case, "DescribeSubnets", &[]).await;
    let subnet_id = element(case, &described, "subnetId");
    let subnet_arn = ::engine::emulator_arn("ec2", REGION, &format!("subnet/{subnet_id}"));
    let create = vec![("SubnetId".to_string(), subnet_id.clone())];

    let held_before = elements(
        &ec2_as_account(&router, case, "DescribeNetworkInterfaces", &[]).await,
        "networkInterfaceId",
    );

    let (denied_akid, denied_arn) = session_of(
        &router,
        case,
        "write-denied-role",
        &[(
            "allow-secretsmanager",
            statement("Allow", "secretsmanager:*", "*"),
        )],
    )
    .await;
    let (status, refused) = ec2_as(&router, &denied_akid, "CreateNetworkInterface", &create).await;
    assert_eq!(
        status, 403,
        "case {case:?}: the unauthorized create must be refused, got {status} {refused}"
    );
    assert_eq!(
        error_message(&refused),
        refusal_message(
            &denied_arn,
            "ec2:CreateNetworkInterface",
            Some(&subnet_arn),
            Expect::NoAllow
        ),
        "case {case:?}: expected IAM's refusal wording, got {refused}"
    );
    let held_after = elements(
        &ec2_as_account(&router, case, "DescribeNetworkInterfaces", &[]).await,
        "networkInterfaceId",
    );
    assert_eq!(
        held_after, held_before,
        "case {case:?}: a refused create writes no interface, expected {held_before:?}, \
         got {held_after:?}"
    );

    let (allowed_akid, _) = session_of(
        &router,
        case,
        "write-allowed-role",
        &[(
            "allow-subnet",
            statement("Allow", "ec2:CreateNetworkInterface", &subnet_arn),
        )],
    )
    .await;
    let (status, created) =
        ec2_as(&router, &allowed_akid, "CreateNetworkInterface", &create).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the authorized create must be served, got {status} {created}"
    );
    let interface_id = element(case, &created, "networkInterfaceId");
    let held_now = elements(
        &ec2_as_account(
            &router,
            case,
            "DescribeNetworkInterfaces",
            &params(&[("NetworkInterfaceId.1", interface_id.as_str())]),
        )
        .await,
        "networkInterfaceId",
    );
    assert_eq!(
        held_now,
        vec![interface_id.clone()],
        "case {case:?}: expected {interface_id} to be read back, got {held_now:?}"
    );
}
