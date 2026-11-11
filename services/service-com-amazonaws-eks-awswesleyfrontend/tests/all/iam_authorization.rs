// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations, driven end to end over the real wire path (a
//! restJson1 request through the multi-service router into the mounted service), so what is
//! asserted is what a client holding those credentials sees.
//!
//! THE RULE UNDER TEST: before it resolves anything the request names, this service decides
//! whether the identity-based IAM policies in force on the principal the request's
//! credential speaks for allow `eks:<OperationName>` on the ARN of the resource the request
//! addresses — the cluster for an operation addressed to a cluster, the capability for one
//! addressed to a capability, the ARN the caller supplied for a tagging call, every resource
//! for an operation that addresses nothing of the account. A request no statement allows
//! (IAM's default deny), or one an explicit `Deny` covers, is answered HTTP 403
//! `AccessDeniedException` naming the principal, the action and that resource; it reports
//! nothing of the account and writes nothing, EVEN WHEN the cluster it names does not
//! exist. A request a matching `Allow` covers reaches the operation's own answer.
//!
//! The role, its inline policy and the session are made the way a client makes them —
//! `iam:CreateRole`, `iam:PutRolePolicy` and `sts:AssumeRole` over the wire, against the
//! same shared engine — so the documents under test are the ones IAM stores and the
//! principal a refusal blames is the one `sts:GetCallerIdentity` reports for the credential
//! the call was signed with.
//!
//! The tables span what the rule generalises over: the operation (the registration pair,
//! the whole capability resource, a cluster read, a cluster delete, a child listing and a
//! tagging read), the resource an operation is judged on (cluster, capability, the ARN the
//! request carried, every resource), the document in force (none at all, another action of
//! this service, another service's actions, the action in another case, a grant scoped to
//! the resource, a grant scoped to another resource, an explicit deny beside an allow) and
//! the principal (an assumed-role session and the account's own published credential).
//!
//! A capability's own ARN is what a request addressed to ONE capability is judged on, with
//! the wildcard in the position a nested Amazon EKS resource ARN carries the identifier the
//! service mints at create time (`nodegroup/<cluster>/<name>/<id>` is the shape every one
//! of them takes): a request names the cluster and the capability and never that
//! identifier.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{
    emulator_with_identity, encode_label, error_code, xml_element, Answer, DEFAULT_ACCESS_KEY_ID,
};

/// The Region every request in this suite is signed for; every ARN a decision is made
/// about is built for it.
const REGION: &str = "us-west-2";

/// The access key id of the account's own published credential: a key neither STS nor IAM
/// holds a record for, which is the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = DEFAULT_ACCESS_KEY_ID;

/// The error code and status an authorization refusal carries (`AccessDeniedException`,
/// `@httpError 403`).
const ACCESS_DENIED: &str = "AccessDeniedException";

/// A role this service would accept as a connector or capability role: an IAM role of the
/// single account this emulator serves.
fn passable_role() -> String {
    format!(
        "arn:aws:iam::{}:role/AmazonEKSConnectorRole",
        engine::emulator_account_id()
    )
}

/// The ARN of a cluster of this account in [`REGION`], as this service mints one.
fn cluster_arn(name: &str) -> String {
    engine::emulator_arn("eks", REGION, &format!("cluster/{name}"))
}

/// The ARN IAM is asked about for a request addressed to one capability of a cluster: this
/// service's capability ARN with the wildcard in the position a nested EKS resource ARN
/// carries the identifier the service mints at create time (`nodegroup/<cluster>/<name>/<id>`
/// is the shape every one of them takes) — the request names the two names and never that
/// identifier.
fn addressed_capability(cluster: &str, capability: &str) -> String {
    engine::emulator_arn(
        "eks",
        REGION,
        &format!("capability/{cluster}/{capability}/*"),
    )
}

/// One role session a row calls as: the access key id its requests are signed with, and the
/// session ARN a refusal names it by.
struct Session {
    access_key_id: String,
    session_arn: String,
}

/// This service beside the two a decision about a session's call reads from, as the shared
/// harness builds them ([`emulator_with_identity`]).
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        Self {
            router: emulator_with_identity(),
        }
    }

    /// One request to this service, signed with `access_key_id` in its SigV4 credential
    /// scope — which is where this emulator reads which principal a request speaks for.
    /// The request itself is built by the shared harness, so this suite sends exactly what
    /// every other one does.
    async fn eks(&self, access_key_id: &str, method: &str, uri: &str, body: &str) -> Answer {
        crate::wire::call_as(&self.router, access_key_id, REGION, method, uri, body).await
    }

    /// One awsQuery call to IAM or STS, as the account's own credential.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&encode_label(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Create an IAM role carrying `document` as its inline policy (when one is given),
    /// assume it, and answer the session that assume minted.
    async fn assumed_role(&self, role_name: &str, document: Option<&str>) -> Session {
        let account = engine::emulator_account_id();
        let role_arn = format!("arn:aws:iam::{account}:role/{role_name}");
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{account}:root")},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the role {role_name:?} must succeed, got {status} {body}"
        );
        if let Some(document) = document {
            let (status, body) = self
                .query(
                    "iam",
                    "2010-05-08",
                    "PutRolePolicy",
                    &[
                        ("RoleName", role_name.to_string()),
                        ("PolicyName", "inline".to_string()),
                        ("PolicyDocument", document.to_string()),
                    ],
                )
                .await;
            assert_eq!(
                status, 200,
                "putting the inline policy of {role_name:?} must succeed, got {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    ("RoleArn", role_arn),
                    ("RoleSessionName", "sim-session".to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "assuming {role_name:?} must succeed, got {status} {body}"
        );
        Session {
            access_key_id: xml_element(&body, "AccessKeyId")
                .unwrap_or_else(|| panic!("AssumeRole must answer an AccessKeyId, got {body}")),
            session_arn: xml_element(&body, "Arn")
                .unwrap_or_else(|| panic!("AssumeRole must answer the session Arn, got {body}")),
        }
    }

    /// `POST /clusters` as the account: the cluster a row addresses when it needs one that
    /// exists.
    async fn given_cluster(&self, name: &str) {
        let body = json!({
            "name": name,
            "roleArn": passable_role(),
            "resourcesVpcConfig": { "subnetIds": ["subnet-0a1b2c3d4e5f60718", "subnet-1a1b2c3d4e5f60719"] },
        })
        .to_string();
        let created = self.eks(ACCOUNT_KEY, "POST", "/clusters", &body).await;
        assert_eq!(
            created.status, 200,
            "the cluster {name:?} must be created first, got {created:?}"
        );
    }
}

/// The refusal IAM words for a principal that may not perform `action` on `resource`
/// because nothing granted it.
fn default_deny_message(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: eks:{action} on resource: {resource} \
         because no identity-based policy allows the eks:{action} action"
    )
}

/// A document allowing `actions` on `resources`.
fn allow(actions: &str, resources: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resources }],
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// RULE: every operation this gate decides refuses a session whose role allows
// nothing of it — with the IAM refusal naming the principal, the action and the
// resource the request addressed — and it does so BEFORE the cluster is
// resolved, so a request naming a cluster that does not exist is still the 403.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_role_that_allows_nothing_of_an_operation_is_refused_before_anything_is_resolved() {
    let emulator = Emulator::new();
    // The only grant: a different action of this service, so every row below falls to
    // IAM's default deny while one call the role DOES allow still goes through.
    let session = emulator
        .assumed_role("eks-list-only", Some(&allow("eks:ListClusters", "*")))
        .await;

    // Nothing in this test creates either resource: the refusal must not depend on
    // whether they exist.
    let cluster = "absent-cluster";
    let capability = "absent-capability";

    // (the operation, the HTTP method, the URI, the body, the resource IAM judges it on)
    let rows: Vec<(&str, &str, String, String, String)> = vec![
        (
            "RegisterCluster",
            "POST",
            "/cluster-registrations".to_string(),
            json!({
                "name": cluster,
                "connectorConfig": { "roleArn": passable_role(), "provider": "OTHER" },
            })
            .to_string(),
            cluster_arn(cluster),
        ),
        (
            "DeregisterCluster",
            "DELETE",
            format!("/cluster-registrations/{cluster}"),
            String::new(),
            cluster_arn(cluster),
        ),
        (
            "CreateCapability",
            "POST",
            format!("/clusters/{cluster}/capabilities"),
            json!({
                "capabilityName": capability,
                "type": "ARGOCD",
                "roleArn": passable_role(),
                "deletePropagationPolicy": "RETAIN",
            })
            .to_string(),
            cluster_arn(cluster),
        ),
        (
            "ListCapabilities",
            "GET",
            format!("/clusters/{cluster}/capabilities"),
            String::new(),
            cluster_arn(cluster),
        ),
        (
            "DescribeCapability",
            "GET",
            format!("/clusters/{cluster}/capabilities/{capability}"),
            String::new(),
            addressed_capability(cluster, capability),
        ),
        (
            "DeleteCapability",
            "DELETE",
            format!("/clusters/{cluster}/capabilities/{capability}"),
            String::new(),
            addressed_capability(cluster, capability),
        ),
        (
            "UpdateCapability",
            "POST",
            format!("/clusters/{cluster}/capabilities/{capability}"),
            json!({ "roleArn": passable_role() }).to_string(),
            addressed_capability(cluster, capability),
        ),
        (
            "DescribeCluster",
            "GET",
            format!("/clusters/{cluster}"),
            String::new(),
            cluster_arn(cluster),
        ),
        (
            "DeleteCluster",
            "DELETE",
            format!("/clusters/{cluster}"),
            String::new(),
            cluster_arn(cluster),
        ),
        (
            "ListNodegroups",
            "GET",
            format!("/clusters/{cluster}/node-groups"),
            String::new(),
            cluster_arn(cluster),
        ),
        (
            "ListTagsForResource",
            "GET",
            format!("/tags/{}", encode_label(&cluster_arn(cluster))),
            String::new(),
            cluster_arn(cluster),
        ),
    ];

    for (operation, method, uri, body, resource) in rows {
        let answer = emulator
            .eks(&session.access_key_id, method, &uri, &body)
            .await;
        assert_eq!(
            answer.status, 403,
            "case {operation:?}: expected HTTP 403, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            ACCESS_DENIED,
            "case {operation:?}: expected {ACCESS_DENIED}, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            default_deny_message(&session.session_arn, operation, &resource),
            "case {operation:?}: expected IAM's refusal, got {:?}",
            answer.message()
        );
    }

    // The one action the role DOES allow is served, which is what makes the refusals
    // above a decision about the policy rather than a blanket denial.
    let listed = emulator
        .eks(&session.access_key_id, "GET", "/clusters", "")
        .await;
    assert_eq!(
        listed.status, 200,
        "case \"the action the role allows\": expected the listing to be served, got {listed:?}"
    );

    // And the denied RegisterCluster wrote nothing: the account's own credential, which
    // this gate does not bound, finds no such cluster.
    let described = emulator
        .eks(ACCOUNT_KEY, "GET", &format!("/clusters/{cluster}"), "")
        .await;
    assert_eq!(
        error_code(&described),
        "ResourceNotFoundException",
        "case \"after the denied registration\": expected no cluster to have been created, \
         got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: the identity-based documents in force on the principal decide — an
// explicit deny wins, a matching allow grants, and anything else falls to IAM's
// default deny, with the action compared case-insensitively and the resource
// compared against the ARN the request addressed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_documents_in_force_decide_what_the_session_may_do() {
    let cluster = "policy-cluster";
    let action = "eks:ListCapabilities";

    // (case, the document in force, whether the call is served)
    let rows: Vec<(&str, Option<String>, bool)> = vec![
        ("no policy at all", None, false),
        ("every action of this service", Some(allow("eks:*", "*")), true),
        ("exactly this action", Some(allow(action, "*")), true),
        (
            "this action in another case",
            Some(allow("eks:listcapabilities", "*")),
            true,
        ),
        (
            "this action on the cluster the request addresses",
            Some(allow(action, &cluster_arn(cluster))),
            true,
        ),
        (
            "this action on every cluster of the Region",
            Some(allow(
                action,
                &engine::emulator_arn("eks", REGION, "cluster/*"),
            )),
            true,
        ),
        (
            "this action on another cluster",
            Some(allow(action, &cluster_arn("another-cluster"))),
            false,
        ),
        (
            "another action of this service",
            Some(allow("eks:DescribeCluster", "*")),
            false,
        ),
        (
            "another service's actions",
            Some(allow("ec2:*", "*")),
            false,
        ),
        (
            "an explicit deny beside an allow of everything",
            Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        { "Effect": "Allow", "Action": "eks:*", "Resource": "*" },
                        { "Effect": "Deny", "Action": action, "Resource": "*" },
                    ],
                })
                .to_string(),
            ),
            false,
        ),
    ];

    for (index, (case, document, served)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        emulator.given_cluster(cluster).await;
        let session = emulator
            .assumed_role(&format!("eks-policy-{index}"), document.as_deref())
            .await;
        let answer = emulator
            .eks(
                &session.access_key_id,
                "GET",
                &format!("/clusters/{cluster}/capabilities"),
                "",
            )
            .await;
        if *served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the operation's own answer, got {answer:?}"
            );
            assert!(
                answer.body.get("capabilities").is_some(),
                "case {case:?}: expected a capability listing, got {answer:?}"
            );
        } else {
            assert_eq!(
                answer.status, 403,
                "case {case:?}: expected HTTP 403, got {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                ACCESS_DENIED,
                "case {case:?}: expected {ACCESS_DENIED}, got {answer:?}"
            );
            assert!(
                answer.message().starts_with(&format!(
                    "User: {} is not authorized to perform: {action} on resource: {}",
                    session.session_arn,
                    cluster_arn(cluster)
                )),
                "case {case:?}: expected the refusal to name the principal, the action and \
                 the cluster the listing belongs to, got {:?}",
                answer.message()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a grant scoped to a capability's own ARN reaches the capability the
// request addresses — the identifier segment a nested EKS resource ARN carries
// is the wildcard here, because the request names the cluster and the
// capability and never the identifier the service mints — while a grant scoped
// to another cluster's capabilities, or to the cluster itself, does not.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_capability_scoped_grant_reaches_the_capability_the_request_addresses() {
    let cluster = "scoped-cluster";
    let capability = "scoped-capability";
    let action = "eks:DescribeCapability";

    // (case, the resource the grant is scoped to, whether the call is served)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "the capability the request addresses, identifier and all",
            addressed_capability(cluster, capability),
            true,
        ),
        (
            "every capability of that cluster",
            engine::emulator_arn("eks", REGION, &format!("capability/{cluster}/*")),
            true,
        ),
        (
            "every capability of another cluster",
            engine::emulator_arn("eks", REGION, "capability/another-cluster/*"),
            false,
        ),
        (
            "the cluster that holds it",
            cluster_arn(cluster),
            false,
        ),
    ];

    for (index, (case, resource, served)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        emulator.given_cluster(cluster).await;
        let session = emulator
            .assumed_role(
                &format!("eks-scoped-{index}"),
                Some(&allow(action, resource)),
            )
            .await;
        let answer = emulator
            .eks(
                &session.access_key_id,
                "GET",
                &format!("/clusters/{cluster}/capabilities/{capability}"),
                "",
            )
            .await;
        if *served {
            // Served means the request reached the operation, which then answers about
            // the capability itself — the cluster holds none of that name.
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case:?}: expected the operation's own answer, got {answer:?}"
            );
        } else {
            assert_eq!(
                error_code(&answer),
                ACCESS_DENIED,
                "case {case:?}: expected {ACCESS_DENIED}, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                default_deny_message(
                    &session.session_arn,
                    "DescribeCapability",
                    &addressed_capability(cluster, capability)
                ),
                "case {case:?}: expected IAM's refusal, got {:?}",
                answer.message()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the account's own published credential is not a principal the IAM store
// holds policies for, so this gate does not bound it — a request signed with it
// reaches the operation's own answer, whatever that is.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_accounts_own_credential_reaches_the_operations_own_answer() {
    let emulator = Emulator::new();
    let cluster = "account-cluster";
    emulator.given_cluster(cluster).await;

    // (case, the URI, the status and error code the operation itself answers)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "a cluster that exists",
            format!("/clusters/{cluster}/capabilities"),
            200,
            "",
        ),
        (
            "a cluster that does not",
            "/clusters/absent-cluster/capabilities".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a capability of a cluster that exists",
            format!("/clusters/{cluster}/capabilities/absent-capability"),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, uri, status, code) in rows {
        let answer = emulator.eks(ACCOUNT_KEY, "GET", &uri, "").await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            code,
            "case {case:?}: expected error code {code:?}, got {answer:?}"
        );
    }
}
