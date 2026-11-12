// SPDX-License-Identifier: CC0-1.0

//! What a `Condition` over the MANAGED POLICY an IAM call names is decided from, end to
//! end over the awsQuery wire.
//!
//! The rule under test: the request a managed-policy attachment call is decided about
//! carries the ARN of the policy that call NAMES, under the service-specific
//! `iam:PolicyARN` key, read off the request's own `PolicyArn` parameter — so a grant
//! conditioned on it permits exactly the attach or detach that names a policy the clause
//! holds for and withholds every other, and a `Deny` conditioned on it refuses exactly
//! the calls it holds for. The key is DECLARED to the evaluation at those call sites, so
//! the existence operator over it is answerable there too. The decision is still made
//! against the ARN of the IDENTITY the attachment is taken on, and it precedes the write,
//! so a refusal leaves that identity's attachments exactly as they were.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which of
//! the six attachment operations is called and therefore which kind of identity it
//! addresses, whether the call is an attach or a detach, which operator compares the key
//! (equality, a pattern, their negations, the existence operator), whether the clause's
//! value is a literal ARN or an ARN pattern, whether the policy the call names is the one
//! the clause admits, and whether the conditioned statement allows or denies.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::principal;
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// modelled example carries. The fixture policies need one to exist at all; what they
/// GRANT is never read here, because every row is about which policy a delegate may
/// attach, not about what the attached policy permits.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy for the fixture role, which `CreateRole` requires.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The policy the delegate's clause admits, and the path it lives under: a path of its
/// own, so a row whose clause is an ARN PATTERN can name the path rather than the policy.
const SCOPED_POLICY: (&str, &str) = ("ScopedPolicy", "/delegated/");

/// The policy the delegate's clause does not admit, at the account root — so the two
/// fixture policies differ in path as well as in name.
const OTHER_POLICY: (&str, &str) = ("OtherPolicy", "/");

/// The ARN of a customer managed policy of this account at `path` under `name`:
/// `arn:aws:iam::<account>:policy<path><name>`.
fn policy_arn((name, path): (&str, &str)) -> String {
    format!("arn:aws:iam::{}:policy{path}{name}", account_id())
}

/// One kind of identity a managed policy attaches to: the three the attachment
/// operations address, with the calls that attach to it, detach from it, report its
/// attachments and create it.
#[derive(Debug, Clone, Copy)]
struct Identity {
    /// The entity type, which is both the ARN's resource type and the word a refusal
    /// names the entity by.
    kind: &'static str,
    /// The awsQuery parameter the request addresses the identity by.
    parameter: &'static str,
    /// The awsQuery action that creates one, and whether that create requires a trust
    /// policy.
    create: &'static str,
    trusted: bool,
    /// The two attachment calls and the reader of their result.
    attach: &'static str,
    detach: &'static str,
    list: &'static str,
}

impl Identity {
    /// The ARN this account holds the identity `name` under, at the default path.
    fn arn(&self, name: &str) -> String {
        format!("arn:aws:iam::{}:{}/{name}", account_id(), self.kind)
    }

    /// The identity as a refusal names it: the entity's type followed by the name the
    /// request carried.
    fn named_as(&self, name: &str) -> String {
        format!("{} {name}", self.kind)
    }
}

/// The three kinds, so every one of the six attachment operations is driven.
const IDENTITIES: &[Identity] = &[
    Identity {
        kind: "user",
        parameter: "UserName",
        create: "CreateUser",
        trusted: false,
        attach: "AttachUserPolicy",
        detach: "DetachUserPolicy",
        list: "ListAttachedUserPolicies",
    },
    Identity {
        kind: "role",
        parameter: "RoleName",
        create: "CreateRole",
        trusted: true,
        attach: "AttachRolePolicy",
        detach: "DetachRolePolicy",
        list: "ListAttachedRolePolicies",
    },
    Identity {
        kind: "group",
        parameter: "GroupName",
        create: "CreateGroup",
        trusted: false,
        attach: "AttachGroupPolicy",
        detach: "DetachGroupPolicy",
        list: "ListAttachedGroupPolicies",
    },
];

/// The IAM action name an awsQuery `Action` is authorized on.
fn iam_action(action: &str) -> String {
    format!("iam:{action}")
}

/// The reason clause closing a refusal no statement in force allowed.
fn no_allow(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The reason clause closing a refusal a statement about the request explicitly denied.
const EXPLICIT_DENY: &str = "with an explicit deny in an identity-based policy";

/// Create the two fixture managed policies, which every row names one of.
async fn fixture_policies(router: &MultiServiceRouter) {
    for (name, path) in [SCOPED_POLICY, OTHER_POLICY] {
        let (status, created) = call(
            router,
            "CreatePolicy",
            &[
                ("PolicyName", name),
                ("Path", path),
                ("PolicyDocument", POLICY_DOCUMENT),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "the fixture CreatePolicy {name:?} must succeed, got {status}: {created}"
        );
    }
}

/// Create the identity `name` of kind `identity`, which the rows' calls address.
async fn create_identity(
    router: &MultiServiceRouter,
    case: &str,
    identity: Identity,
    name: &str,
) {
    let mut params: Vec<(&str, &str)> = vec![(identity.parameter, name)];
    if identity.trusted {
        params.push(("AssumeRolePolicyDocument", TRUST_POLICY));
    }
    let (status, created) = call(router, identity.create, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture {} {name:?} must succeed, got {status}: {created}",
        identity.create
    );
}

/// The managed policies `name` carries, ascending by ARN, read back through the reader of
/// that identity's attachments — the round trip every write below is checked by.
async fn attached(
    router: &MultiServiceRouter,
    case: &str,
    identity: Identity,
    name: &str,
) -> Vec<String> {
    let (status, body) = call(router, identity.list, &[(identity.parameter, name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture {} must succeed, got {status}: {body}",
        identity.list
    );
    let mut arns = elements(&body, "PolicyArn");
    arns.sort();
    arns
}

/// Attach `policy_arn` to `name` as the ACCOUNT itself, which is the fixture state a
/// detach row starts from.
async fn attach_as_account(
    router: &MultiServiceRouter,
    case: &str,
    identity: Identity,
    name: &str,
    policy_arn: &str,
) {
    let (status, body) = call(
        router,
        identity.attach,
        &[(identity.parameter, name), ("PolicyArn", policy_arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture {} of {policy_arn} must succeed, got {status}: {body}",
        identity.attach
    );
}

/// A one-statement document whose `effect` covers `actions` on `resource`, conditioned on
/// `iam:PolicyARN` comparing to `value` under `operator`.
fn conditioned(
    effect: &str,
    actions: &[String],
    resource: &str,
    operator: &str,
    value: &str,
) -> String {
    let quoted: Vec<String> = actions.iter().map(|action| format!("\"{action}\"")).collect();
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":[{}],"Resource":"{resource}","Condition":{{"{operator}":{{"iam:PolicyARN":"{value}"}}}}}}]}}"#,
        quoted.join(",")
    )
}

/// RULE: a delegate whose grant for the attachment calls is conditioned on
/// `iam:PolicyARN` is permitted the call that names a policy the clause admits, and
/// refused — with the identity's attachments untouched — for every other policy, at each
/// of the six operations.
#[tokio::test]
async fn a_grant_scoped_to_one_policy_arn_decides_every_attachment_call() {
    let router = router();
    fixture_policies(&router).await;
    let scoped = policy_arn(SCOPED_POLICY);
    let other = policy_arn(OTHER_POLICY);

    // Rows: each kind of identity, and each of the two calls taken on it.
    for identity in IDENTITIES {
        for detaching in [false, true] {
            let action = if detaching {
                identity.detach
            } else {
                identity.attach
            };
            let case = format!("{action} scoped to one policy ARN");
            let target = format!("target-{}-{action}", identity.kind);
            create_identity(&router, &case, *identity, &target).await;
            if detaching {
                // A detach row starts from both policies attached, so what separates its
                // two calls is the clause and nothing else.
                for policy in [&other, &scoped] {
                    attach_as_account(&router, &case, *identity, &target, policy).await;
                }
            }

            // The delegate may make BOTH calls on this identity, for the one policy the
            // clause names.
            let document = conditioned(
                "Allow",
                &[
                    iam_action(identity.attach),
                    iam_action(identity.detach),
                ],
                &identity.arn(&target),
                "ArnEquals",
                &scoped,
            );
            let delegate = format!("delegate-{}-{action}", identity.kind);
            let access_key_id = principal(
                &router,
                &case,
                &delegate,
                &[("only-the-scoped-policy".to_string(), document)],
            )
            .await;

            let before: Vec<String> = if detaching {
                let mut both = vec![other.clone(), scoped.clone()];
                both.sort();
                both
            } else {
                Vec::new()
            };
            assert_eq!(
                attached(&router, &case, *identity, &target).await,
                before,
                "case {case:?}: the fixture must start from the attachments the row set up"
            );

            // The policy the clause does NOT admit: refused, and nothing written.
            let (status, body) = call_signed_by(
                &router,
                Some(&access_key_id),
                action,
                &[(identity.parameter, &target), ("PolicyArn", &other)],
            )
            .await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected the policy the clause excludes to be refused, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {case:?}: expected AccessDenied, got {:?}",
                error_code(&body)
            );
            assert_access_denied_message(
                &case,
                &element(&body, "Message"),
                &format!("arn:aws:iam::{}:user/{delegate}", account_id()),
                &iam_action(action),
                &identity.named_as(&target),
                &no_allow(&iam_action(action)),
            );
            assert_eq!(
                attached(&router, &case, *identity, &target).await,
                before,
                "case {case:?}: expected the refused call to leave the attachments untouched"
            );

            // The policy the clause admits: served, and the write is readable back.
            let (status, body) = call_signed_by(
                &router,
                Some(&access_key_id),
                action,
                &[(identity.parameter, &target), ("PolicyArn", &scoped)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the policy the clause admits to be served, got \
                 {status}: {body}"
            );
            let after = if detaching {
                vec![other.clone()]
            } else {
                vec![scoped.clone()]
            };
            assert_eq!(
                attached(&router, &case, *identity, &target).await,
                after,
                "case {case:?}: expected the served call to be the only write"
            );
        }
    }
}

/// One row of the operator table: the clause the delegate's document carries, which of
/// the two fixture policies the call then names, and whether the rule requires that call
/// to be served.
struct Clause {
    case: &'static str,
    operator: &'static str,
    value: String,
    /// Whether the call names [`SCOPED_POLICY`] (the other row names [`OTHER_POLICY`]).
    names_scoped: bool,
    served: bool,
}

/// The rows: the ARN operators IAM compares an ARN-valued key under (equality, a
/// pattern, and the negation of each), the existence operator, and a literal that differs
/// from the named ARN only in case — ARN comparison being case-sensitive.
fn clauses() -> Vec<Clause> {
    let scoped = policy_arn(SCOPED_POLICY);
    // The PATH of the scoped policy, wildcarded over the name: the pattern belongs to the
    // document, so it matches a run of the compared ARN.
    let pattern = format!(
        "arn:aws:iam::{}:policy{}*",
        account_id(),
        SCOPED_POLICY.1
    );
    let miscased = policy_arn((&SCOPED_POLICY.0.to_ascii_uppercase(), SCOPED_POLICY.1));
    vec![
        Clause {
            case: "equality, the ARN the clause names",
            operator: "ArnEquals",
            value: scoped.clone(),
            names_scoped: true,
            served: true,
        },
        Clause {
            case: "equality, another ARN",
            operator: "ArnEquals",
            value: scoped.clone(),
            names_scoped: false,
            served: false,
        },
        Clause {
            case: "a pattern the named ARN matches",
            operator: "ArnLike",
            value: pattern.clone(),
            names_scoped: true,
            served: true,
        },
        Clause {
            case: "a pattern the named ARN does not match",
            operator: "ArnLike",
            value: pattern.clone(),
            names_scoped: false,
            served: false,
        },
        Clause {
            case: "the negated equality, on another ARN",
            operator: "ArnNotEquals",
            value: scoped.clone(),
            names_scoped: false,
            served: true,
        },
        Clause {
            case: "the negated equality, on the ARN the clause names",
            operator: "ArnNotEquals",
            value: scoped.clone(),
            names_scoped: true,
            served: false,
        },
        Clause {
            case: "the negated pattern, on an ARN it does not match",
            operator: "ArnNotLike",
            value: pattern.clone(),
            names_scoped: false,
            served: true,
        },
        Clause {
            case: "the negated pattern, on an ARN it matches",
            operator: "ArnNotLike",
            value: pattern,
            names_scoped: true,
            served: false,
        },
        // ARN comparison is case-sensitive, so a value differing only in the policy
        // name's case names another policy.
        Clause {
            case: "a literal differing from the named ARN only in case",
            operator: "ArnEquals",
            value: miscased,
            names_scoped: true,
            served: false,
        },
        // The existence operator is answerable because the gate declares the key at these
        // call sites: every attachment call carries a value for it, whichever policy it
        // names.
        Clause {
            case: "the key asserted present, naming the scoped policy",
            operator: "Null",
            value: "false".to_string(),
            names_scoped: true,
            served: true,
        },
        Clause {
            case: "the key asserted present, naming the other policy",
            operator: "Null",
            value: "false".to_string(),
            names_scoped: false,
            served: true,
        },
        Clause {
            case: "the key asserted absent",
            operator: "Null",
            value: "true".to_string(),
            names_scoped: true,
            served: false,
        },
    ]
}

/// RULE: the clause over `iam:PolicyARN` is decided by comparing the ARN the request
/// carries against the clause's value under the operator the document names, so each ARN
/// operator, its negation and the existence operator decide the same request differently.
#[tokio::test]
async fn the_clause_over_the_policy_arn_is_decided_under_each_operator() {
    let router = router();
    fixture_policies(&router).await;
    let scoped = policy_arn(SCOPED_POLICY);
    let other = policy_arn(OTHER_POLICY);
    // One identity kind, so what separates the rows is the clause alone; the six
    // operations are the other suite's table.
    let identity = IDENTITIES[0];

    for (row, clause) in clauses().into_iter().enumerate() {
        let target = format!("clause-target{row}");
        create_identity(&router, clause.case, identity, &target).await;
        let document = conditioned(
            "Allow",
            &[iam_action(identity.attach)],
            &identity.arn(&target),
            clause.operator,
            &clause.value,
        );
        let delegate = format!("clause-delegate{row}");
        let access_key_id = principal(
            &router,
            clause.case,
            &delegate,
            &[("conditioned".to_string(), document)],
        )
        .await;

        let named = if clause.names_scoped { &scoped } else { &other };
        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            identity.attach,
            &[(identity.parameter, &target), ("PolicyArn", named)],
        )
        .await;

        if clause.served {
            assert_eq!(
                status, 200,
                "case {:?}: expected the clause to hold and the attach to be served, got \
                 {status}: {body}",
                clause.case
            );
            assert_eq!(
                attached(&router, clause.case, identity, &target).await,
                vec![named.clone()],
                "case {:?}: expected the served attach to be readable back",
                clause.case
            );
        } else {
            assert_eq!(
                status, 403,
                "case {:?}: expected the clause not to grant the attach, got {status}: {body}",
                clause.case
            );
            assert_eq!(
                error_code(&body),
                "AccessDenied",
                "case {:?}: expected AccessDenied, got {:?}",
                clause.case,
                error_code(&body)
            );
            assert_access_denied_message(
                clause.case,
                &element(&body, "Message"),
                &format!("arn:aws:iam::{}:user/{delegate}", account_id()),
                &iam_action(identity.attach),
                &identity.named_as(&target),
                &no_allow(&iam_action(identity.attach)),
            );
            assert!(
                attached(&router, clause.case, identity, &target)
                    .await
                    .is_empty(),
                "case {:?}: expected the refused attach to write nothing",
                clause.case
            );
        }
    }
}

/// RULE: a `Deny` conditioned on `iam:PolicyARN` is about exactly the calls whose policy
/// the clause holds for — so it refuses those and leaves an otherwise granted call that
/// names another policy served.
#[tokio::test]
async fn a_deny_conditioned_on_the_policy_arn_refuses_only_the_policy_it_names() {
    let router = router();
    fixture_policies(&router).await;
    let scoped = policy_arn(SCOPED_POLICY);
    let other = policy_arn(OTHER_POLICY);
    let identity = IDENTITIES[0];
    let case = "a deny scoped to one policy ARN";
    let target = "deny-target";
    create_identity(&router, case, identity, target).await;

    // An unconditioned grant for the call, so what the row demonstrates is the `Deny`
    // and not a missing `Allow`.
    let allow = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":["{}"],"Resource":"{}"}}]}}"#,
        iam_action(identity.attach),
        identity.arn(target)
    );
    let deny = conditioned(
        "Deny",
        &[iam_action(identity.attach)],
        &identity.arn(target),
        "ArnEquals",
        &other,
    );
    let delegate = "deny-delegate";
    let access_key_id = principal(
        &router,
        case,
        delegate,
        &[
            ("allow-the-attach".to_string(), allow),
            ("deny-one-policy".to_string(), deny),
        ],
    )
    .await;

    let (status, body) = call_signed_by(
        &router,
        Some(&access_key_id),
        identity.attach,
        &[(identity.parameter, target), ("PolicyArn", &other)],
    )
    .await;
    assert_eq!(
        status, 403,
        "case {case:?}: expected the denied policy's attach to be refused, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "AccessDenied",
        "case {case:?}: expected AccessDenied, got {:?}",
        error_code(&body)
    );
    assert_access_denied_message(
        case,
        &element(&body, "Message"),
        &format!("arn:aws:iam::{}:user/{delegate}", account_id()),
        &iam_action(identity.attach),
        &identity.named_as(target),
        EXPLICIT_DENY,
    );
    assert!(
        attached(&router, case, identity, target).await.is_empty(),
        "case {case:?}: expected the refused attach to write nothing"
    );

    let (status, body) = call_signed_by(
        &router,
        Some(&access_key_id),
        identity.attach,
        &[(identity.parameter, target), ("PolicyArn", &scoped)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the policy the deny does not name to be served, got \
         {status}: {body}"
    );
    assert_eq!(
        attached(&router, case, identity, target).await,
        vec![scoped],
        "case {case:?}: expected the served attach to be readable back"
    );
}
