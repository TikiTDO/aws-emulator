// SPDX-License-Identifier: CC0-1.0

//! Whether a CodeArtifact control-plane call whose subject is named by an ARN is authorized,
//! over the wire.
//!
//! The rule under test is the service's one authorization rule, reached through the
//! operations that address their subject by ARN rather than by name members: a principal may
//! perform `codeartifact:<OperationName>` on the resource the call addressed only where some
//! policy in force allows it — the caller's own identity-based policies, or a resource policy
//! in force on that resource naming the caller as `Principal` — an explicit `Deny` in either
//! refuses whatever the other says, and a principal allowed by neither is refused
//! `AccessDeniedException` (HTTP 403;
//! `smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`
//! lists it with httpError 403) instead of being served.
//!
//! Two halves of that rule are what an ARN-addressed call adds to the suites beside this one:
//!
//! * the resource the decision is reached against — and therefore the ARN the refusal names —
//!   is the resource the ARN resolved to, which is a domain, a repository or a package group,
//!   so the rows below span all three kinds rather than the one an issue happened to name;
//! * the documents in force are the ones that resource lives under: its domain's permissions
//!   policy for every kind, and for a repository that repository's own policy as well — so a
//!   repository-policy grant reaches the repository's ARN alone, while a domain-policy `Deny`
//!   reaches every resource inside that domain.
//!
//! A refused WRITE must also leave the resource exactly as it was, which is asserted by
//! reading the tag set back through a caller this gate does not bound.
//!
//! Each case drives the production front door, and the session, the role and its policies live
//! in the STS and IAM stores of the SAME shared engine, the stores those services' own
//! handlers write.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_as, emulator_with_engine, given_role, given_role_policy, given_session, role_arn,
    session_arn, Answer,
};
use crate::resource_tags::encode_query;

/// The role every session below is assumed from, and the access key id of its session's
/// credentials: an `ASIA` key, as STS mints for a session.
const ROLE: &str = "ca-tagger";
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION30";
const SESSION_NAME: &str = "tagger";

/// A SECOND role, the one no policy in these cases names: a sibling principal in the same
/// account, which is what shows a grant is to a principal rather than to the account.
const OTHER_ROLE: &str = "ca-tagger-sibling";
const OTHER_SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION31";
const OTHER_SESSION_NAME: &str = "sibling";

/// The domain, the repository and the package group every case addresses.
const DOMAIN: &str = "arn-authz";
const REPOSITORY: &str = "Releases.one";
const PACKAGE_GROUP: &str = "/npm/team/*";

/// The tag every resource is created holding — what a refused write must leave behind.
const TAG_KEY: &str = "team";
const TAG_VALUE: &str = "platform";

/// The tag `TagResource` writes in these cases: absent until a call that carries it is
/// SERVED, which is what tells a refused write from an inert one.
const WRITTEN_KEY: &str = "written-by";
const WRITTEN_VALUE: &str = "the-addressed-call";

/// One ARN-addressed request, as a client sends it: the operation whose action is being
/// decided, the path it is bound to, and the body it carries. The resource travels in the
/// `resourceArn` query parameter for all three
/// (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`),
/// so the body is the only part that differs.
#[derive(Debug, Clone, Copy)]
struct Addressed {
    operation: &'static str,
    path: &'static str,
    body: fn() -> Value,
    /// Whether the operation WRITES the tag set, so a refusal has something to have left
    /// untouched.
    writes: bool,
}

/// The ARN-addressed operations these cases drive: every operation of this service whose
/// input names its subject by `resourceArn`.
const ADDRESSED: &[Addressed] = &[
    Addressed {
        operation: "ListTagsForResource",
        path: "/v1/tags",
        body: || json!({}),
        writes: false,
    },
    Addressed {
        operation: "TagResource",
        path: "/v1/tag",
        body: || json!({ "tags": [{ "key": WRITTEN_KEY, "value": WRITTEN_VALUE }] }),
        writes: true,
    },
    Addressed {
        operation: "UntagResource",
        path: "/v1/untag",
        body: || json!({ "tagKeys": [TAG_KEY] }),
        writes: true,
    },
];

/// Which reason clause IAM states a refusal with, which depends on WHICH document refused.
#[derive(Debug, Clone, Copy)]
enum Clause {
    /// Nothing in force allows the action.
    NoGrant,
    /// A readable identity-based document denies it.
    IdentityDeny,
    /// A readable resource-based document denies it.
    ResourceDeny,
}

/// One resource of this service, as an ARN addresses it: the kind (which is what decides the
/// documents in force) and the ARN the resource itself reports.
#[derive(Debug, Clone)]
struct Subject {
    kind: &'static str,
    arn: String,
}

/// An emulator with this one service mounted on one shared engine; a domain, a repository
/// inside it and a package group inside it, each created by the account's own credential and
/// each holding one tag; and both roles held with sessions recorded.
///
/// The subjects are the ARNs the CREATES reported, in the order `[domain, repository, package
/// group]`, so every decision below is reached against the ARN the resource itself publishes
/// rather than one this suite spells out.
async fn given_resources() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Vec<Subject>) {
    let (router, engine) = emulator_with_engine();
    let tags = json!([{ "key": TAG_KEY, "value": TAG_VALUE }]);
    let created = [
        (
            "domain",
            "domain",
            format!("/v1/domain?domain={DOMAIN}"),
            json!({ "tags": tags }),
        ),
        (
            "repository",
            "repository",
            format!("/v1/repository?domain={DOMAIN}&repository={REPOSITORY}"),
            json!({ "tags": tags }),
        ),
        (
            "package group",
            "packageGroup",
            format!("/v1/package-group?domain={DOMAIN}"),
            json!({ "packageGroup": PACKAGE_GROUP, "tags": tags }),
        ),
    ];
    let mut subjects = Vec::new();
    for (kind, member, uri, body) in created {
        let answer = call(&router, "POST", &uri, Some(body)).await;
        assert_eq!(
            answer.status, 200,
            "case {kind:?}: the create must succeed before anything is authorized, got {answer:?}"
        );
        let arn = answer.body[member]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {kind:?}: the create must report an ARN, got {answer:?}"))
            .to_string();
        subjects.push(Subject { kind, arn });
    }
    for (role, key, session) in [
        (ROLE, SESSION_KEY_ID, SESSION_NAME),
        (OTHER_ROLE, OTHER_SESSION_KEY_ID, OTHER_SESSION_NAME),
    ] {
        given_role(&engine, role);
        given_session(&engine, key, role, session, 3_600);
    }
    (router, engine, subjects)
}

/// A policy allowing or denying `actions` on every resource, as an identity policy states one
/// (no `Principal` element).
fn identity(effect: &str, actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": actions, "Resource": "*" }]
    })
}

/// A permissions policy stating `effect` for `principal` on `actions`, the shape
/// `PutDomainPermissionsPolicy` and `PutRepositoryPermissionsPolicy` both take.
fn resource_policy(effect: &str, principal: &str, actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "AWS": principal },
            "Action": actions,
            "Resource": "*"
        }]
    })
}

/// Attach `document` to the DOMAIN, or detach whatever it carries when `None`, as the
/// account's own credential (the caller this gate does not bound). `PutDomainPermissionsPolicy`
/// carries every member in the BODY; the delete takes them in the query string
/// (`smithy_query(query="operation",operation="PutDomainPermissionsPolicy,DeleteDomainPermissionsPolicy")`).
async fn given_domain_policy(router: &MultiServiceRouter, document: Option<&Value>) {
    let answer = match document {
        Some(document) => {
            call(
                router,
                "PUT",
                "/v1/domain/permissions/policy",
                Some(json!({ "domain": DOMAIN, "policyDocument": document.to_string() })),
            )
            .await
        }
        None => {
            call(
                router,
                "DELETE",
                &format!("/v1/domain/permissions/policy?domain={DOMAIN}"),
                None,
            )
            .await
        }
    };
    assert!(
        answer.status == 200 || answer.status == 404,
        "seeding the domain policy must succeed or find none to remove, got {answer:?}"
    );
}

/// [`given_domain_policy`], for the REPOSITORY's own policy. `DeleteRepositoryPermissionsPolicy`
/// is bound to the PLURAL path
/// (`smithy_query(query="operation",operation="DeleteRepositoryPermissionsPolicy")`:
/// DELETE /v1/repository/permissions/policies).
async fn given_repository_policy(router: &MultiServiceRouter, document: Option<&Value>) {
    let query = format!("domain={DOMAIN}&repository={REPOSITORY}");
    let answer = match document {
        Some(document) => {
            call(
                router,
                "PUT",
                &format!("/v1/repository/permissions/policy?{query}"),
                Some(json!({ "policyDocument": document.to_string() })),
            )
            .await
        }
        None => {
            call(
                router,
                "DELETE",
                &format!("/v1/repository/permissions/policies?{query}"),
                None,
            )
            .await
        }
    };
    assert!(
        answer.status == 200 || answer.status == 404,
        "seeding the repository policy must succeed or find none to remove, got {answer:?}"
    );
}

/// Send one ARN-addressed request as the session `access_key_id` speaks for.
async fn as_session(
    router: &MultiServiceRouter,
    access_key_id: &str,
    addressed: &Addressed,
    arn: &str,
) -> Answer {
    let uri = format!("{}?resourceArn={}", addressed.path, encode_query(arn));
    call_as(router, access_key_id, "POST", &uri, Some((addressed.body)())).await
}

/// The tags a resource holds, read back as the account's own credential — a caller this gate
/// does not bound, so the read reports what the store holds whatever the session was refused.
async fn held_tags(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let uri = format!("/v1/tags?resourceArn={}", encode_query(arn));
    let listed = call(router, "POST", &uri, Some(json!({}))).await;
    assert_eq!(
        listed.status, 200,
        "reading the tag set back must succeed, got {listed:?}"
    );
    listed.body["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("ListTagsForResource must report a tag list, got {listed:?}"))
        .iter()
        .map(|tag| {
            (
                tag["key"].as_str().unwrap_or_default().to_string(),
                tag["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// IAM's whole refusal sentence: the signing session, the service-qualified action, the ARN
/// of the resource the action was judged against, and the clause naming which document
/// refused.
fn refusal(action: &str, resource: &str, clause: Clause) -> String {
    let principal = session_arn(ROLE, SESSION_NAME);
    let because = match clause {
        Clause::NoGrant => format!(" because no identity-based policy allows the {action} action"),
        Clause::IdentityDeny => " with an explicit deny in an identity-based policy".to_string(),
        Clause::ResourceDeny => " with an explicit deny in a resource-based policy".to_string(),
    };
    format!("User: {principal} is not authorized to perform: {action} on resource: {resource}{because}")
}

/// One row of the table below: the documents in force, which subjects they serve, and the
/// clause a refused subject is refused with.
#[derive(Debug, Clone, Copy)]
struct Row<'a> {
    case: &'a str,
    identity_policy: Option<&'a Value>,
    domain_policy: Option<&'a Value>,
    repository_policy: Option<&'a Value>,
    /// Which subjects these documents serve, in the `[domain, repository, package group]`
    /// order the fixture builds them.
    served: [bool; 3],
    clause: Clause,
}

/// RULE: an ARN-addressed call is served only where some document in force allows its action,
/// the documents in force are the addressed resource's own and its domain's, and a refusal is
/// IAM's whole sentence naming the ARN the decision was reached against.
///
/// Rows differ along WHICH document grants or denies (neither, the identity policy, the
/// domain's, the repository's), along WHO a resource policy names (the role, a sibling role)
/// and along which resource KIND the ARN addresses — which is what decides whether a
/// repository-scoped document is in force at all.
#[tokio::test]
async fn an_arn_addressed_call_is_served_only_where_some_policy_allows_its_action() {
    let (router, engine, subjects) = given_resources().await;
    let names_the_role = resource_policy("Allow", &role_arn(ROLE), json!(["codeartifact:*"]));
    let names_the_other_role =
        resource_policy("Allow", &role_arn(OTHER_ROLE), json!(["codeartifact:*"]));
    let denies_the_role = resource_policy("Deny", &role_arn(ROLE), json!("codeartifact:*"));
    let allows_everything = identity("Allow", json!("*"));
    let allows_this_service = identity("Allow", json!("codeartifact:*"));
    let denies_this_service = identity("Deny", json!("codeartifact:*"));

    let rows = [
        Row {
            case: "no policy anywhere",
            identity_policy: None,
            domain_policy: None,
            repository_policy: None,
            served: [false; 3],
            clause: Clause::NoGrant,
        },
        Row {
            case: "the identity policy alone allows this service",
            identity_policy: Some(&allows_this_service),
            domain_policy: None,
            repository_policy: None,
            served: [true; 3],
            clause: Clause::NoGrant,
        },
        Row {
            case: "the identity policy allows every action of every service",
            identity_policy: Some(&allows_everything),
            domain_policy: None,
            repository_policy: None,
            served: [true; 3],
            clause: Clause::NoGrant,
        },
        Row {
            case: "the domain policy alone names the role",
            identity_policy: None,
            domain_policy: Some(&names_the_role),
            repository_policy: None,
            served: [true; 3],
            clause: Clause::NoGrant,
        },
        Row {
            case: "the repository policy alone names the role",
            identity_policy: None,
            domain_policy: None,
            repository_policy: Some(&names_the_role),
            served: [false, true, false],
            clause: Clause::NoGrant,
        },
        Row {
            case: "the domain policy names a sibling role",
            identity_policy: None,
            domain_policy: Some(&names_the_other_role),
            repository_policy: None,
            served: [false; 3],
            clause: Clause::NoGrant,
        },
        Row {
            case: "the domain policy denies the role its identity policy allows",
            identity_policy: Some(&allows_everything),
            domain_policy: Some(&denies_the_role),
            repository_policy: None,
            served: [false; 3],
            clause: Clause::ResourceDeny,
        },
        Row {
            case: "the repository policy denies the role its identity policy allows",
            identity_policy: Some(&allows_everything),
            domain_policy: None,
            repository_policy: Some(&denies_the_role),
            served: [true, false, true],
            clause: Clause::ResourceDeny,
        },
        Row {
            case: "the identity policy denies this service while the domain policy allows it",
            identity_policy: Some(&denies_this_service),
            domain_policy: Some(&names_the_role),
            repository_policy: None,
            served: [false; 3],
            clause: Clause::IdentityDeny,
        },
        Row {
            case: "both the identity policy and the domain policy allow it",
            identity_policy: Some(&allows_this_service),
            domain_policy: Some(&names_the_role),
            repository_policy: None,
            served: [true; 3],
            clause: Clause::NoGrant,
        },
    ];

    for row in &rows {
        let Row {
            case,
            identity_policy,
            domain_policy,
            repository_policy,
            served,
            clause,
        } = row;
        given_role_policy(&engine, ROLE, *identity_policy);
        given_domain_policy(&router, *domain_policy).await;
        given_repository_policy(&router, *repository_policy).await;
        for (subject, allowed) in subjects.iter().zip(served) {
            for addressed in ADDRESSED {
                let answer = as_session(&router, SESSION_KEY_ID, addressed, &subject.arn).await;
                let kind = subject.kind;
                let operation = addressed.operation;
                if *allowed {
                    assert_eq!(
                        answer.status,
                        200,
                        "case {case:?} on the {kind}'s {operation}: expected the call to be \
                         served, got {answer:?}"
                    );
                } else {
                    assert_eq!(
                        (answer.status, answer.code()),
                        (403, "AccessDeniedException".to_string()),
                        "case {case:?} on the {kind}'s {operation}: expected the modelled \
                         refusal, got {answer:?}"
                    );
                    assert_eq!(
                        answer.message(),
                        refusal(
                            &format!("codeartifact:{operation}"),
                            &subject.arn,
                            *clause
                        ),
                        "case {case:?} on the {kind}'s {operation}: expected IAM's whole \
                         sentence, got {:?}",
                        answer.message()
                    );
                }
            }
        }
    }
}

/// The one action the identity policy of the per-action case grants, named exactly: a reader,
/// so the writes of the same class are the calls left ungranted.
const GRANTED_ACTION: &str = "ListTagsForResource";

/// RULE: the grant is per ACTION, and a refused call writes nothing — a resource a caller may
/// read the tags of is not a resource it may change them on, and the refused write leaves the
/// tag set the resource held.
#[tokio::test]
async fn a_grant_reaches_the_action_it_names_and_a_refused_write_changes_nothing() {
    let (router, engine, subjects) = given_resources().await;
    // The only grant in force: one action, named exactly, on every resource.
    let one_action = identity("Allow", json!([format!("codeartifact:{GRANTED_ACTION}")]));
    given_role_policy(&engine, ROLE, Some(&one_action));

    for subject in &subjects {
        let kind = subject.kind;
        for addressed in ADDRESSED {
            let operation = addressed.operation;
            let answer = as_session(&router, SESSION_KEY_ID, addressed, &subject.arn).await;
            if operation == GRANTED_ACTION {
                assert_eq!(
                    answer.status, 200,
                    "case {kind:?} on the granted {operation}: expected the call to be served, \
                     got {answer:?}"
                );
            } else {
                assert_eq!(
                    (answer.status, answer.code()),
                    (403, "AccessDeniedException".to_string()),
                    "case {kind:?} on the ungranted {operation}: expected the modelled refusal, \
                     got {answer:?}"
                );
                assert_eq!(
                    answer.message(),
                    refusal(
                        &format!("codeartifact:{operation}"),
                        &subject.arn,
                        Clause::NoGrant
                    ),
                    "case {kind:?} on the ungranted {operation}: expected IAM's whole sentence, \
                     got {:?}",
                    answer.message()
                );
            }
        }
        // Every refused write above left the tag set exactly as the create made it.
        assert_eq!(
            held_tags(&router, &subject.arn).await,
            vec![(TAG_KEY.to_string(), TAG_VALUE.to_string())],
            "case {kind:?}: a refused write must leave the tag set untouched"
        );
    }

    // The control the rows above need: the SAME write, once the identity policy grants its
    // action, is served and DOES change the tag set — so what left the set untouched was the
    // refusal, not a request that would have written nothing anyway.
    let writer = ADDRESSED
        .iter()
        .find(|addressed| addressed.writes)
        .expect("the table drives at least one write");
    given_role_policy(&engine, ROLE, Some(&identity("Allow", json!("codeartifact:*"))));
    for subject in &subjects {
        let kind = subject.kind;
        let operation = writer.operation;
        let answer = as_session(&router, SESSION_KEY_ID, writer, &subject.arn).await;
        assert_eq!(
            answer.status, 200,
            "case {kind:?} on the granted {operation}: expected the call to be served, got \
             {answer:?}"
        );
        let held = held_tags(&router, &subject.arn).await;
        assert!(
            held.contains(&(WRITTEN_KEY.to_string(), WRITTEN_VALUE.to_string())),
            "case {kind:?} on the granted {operation}: expected the served write to be stored, \
             got {held:?}"
        );
    }
}

/// RULE: a grant is to the PRINCIPAL and the RESOURCE it names — a sibling session in the same
/// account is refused what the named one is served, a repository-policy grant does not reach a
/// neighbouring repository's ARN, and a caller whose identity-based grants this emulator cannot
/// read is not refused for the absence of one.
#[tokio::test]
async fn a_grant_reaches_the_principal_and_the_resource_it_names_only() {
    let (router, engine, subjects) = given_resources().await;
    for role in [ROLE, OTHER_ROLE] {
        given_role_policy(&engine, role, None);
    }
    // A neighbouring repository in the same domain, which the repository policy below does
    // not cover: its ARN is the one it reports itself.
    let neighbour = "Releases.two";
    let created = call(
        &router,
        "POST",
        &format!("/v1/repository?domain={DOMAIN}&repository={neighbour}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateRepository for the neighbour must succeed, got {created:?}"
    );
    let neighbour_arn = created.body["repository"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("the neighbour must report an ARN, got {created:?}"))
        .to_string();
    let names_the_role = resource_policy("Allow", &role_arn(ROLE), json!(["codeartifact:*"]));
    given_repository_policy(&router, Some(&names_the_role)).await;

    let repository = subjects
        .iter()
        .find(|subject| subject.kind == "repository")
        .expect("the fixture creates a repository");
    let reader = ADDRESSED
        .iter()
        .find(|addressed| !addressed.writes)
        .expect("the table drives at least one read");

    // (case, the session signing, the ARN addressed, whether the grant covers it)
    for (case, key, arn, granted) in [
        (
            "the named role on the granted repository",
            SESSION_KEY_ID,
            &repository.arn,
            true,
        ),
        (
            "the named role on a neighbouring repository",
            SESSION_KEY_ID,
            &neighbour_arn,
            false,
        ),
        (
            "a sibling role on the granted repository",
            OTHER_SESSION_KEY_ID,
            &repository.arn,
            false,
        ),
    ] {
        let answer = as_session(&router, key, reader, arn).await;
        if granted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the grant to serve the call, got {answer:?}"
            );
        } else {
            assert_eq!(
                (answer.status, answer.code()),
                (403, "AccessDeniedException".to_string()),
                "case {case:?}: expected the modelled refusal, got {answer:?}"
            );
            assert!(
                answer.body.get("tags").is_none(),
                "case {case:?}: a refused read must report no tag set, got {answer:?}"
            );
        }
    }

    // The account's own published credential: no identity policy governs it, so it is not
    // refused for the absence of a grant — on every ARN-addressed operation and every kind.
    for subject in &subjects {
        for addressed in ADDRESSED {
            let uri = format!(
                "{}?resourceArn={}",
                addressed.path,
                encode_query(&subject.arn)
            );
            let answer = call(&router, "POST", &uri, Some((addressed.body)())).await;
            let kind = subject.kind;
            let operation = addressed.operation;
            assert_eq!(
                answer.status, 200,
                "case \"the account's own published credential\" on the {kind}'s {operation}: \
                 expected the call to be served, got {answer:?}"
            );
        }
    }
}
