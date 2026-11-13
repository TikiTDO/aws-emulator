// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the principal that SIGNED a request perform the action it asks for —
//! the decision taken in FRONT of the operation, before the request's own input is read.
//!
//! Every request to this service is signed with IAM credentials (`sigv4` is the only auth
//! scheme its model declares, `smithy_query(query="service_info")`), and AWS decides such a
//! call at the IAM layer ahead of the service's own code: a caller whose identity-based
//! policies do not allow the action is told it is not authorized rather than told anything
//! about the request it sent, so the refusal comes before the body is validated and before
//! any state is read, and nothing of the account changes on the way to it. The decision is
//! taken per request, so a statement `iam:PutRolePolicy` wrote refuses the next call and one
//! `iam:DeleteRolePolicy` removed stops granting it — which is what makes a client's
//! least-privilege test, and a revocation drill, mean anything here.
//!
//! `AccessDeniedException` is the modelled answer, at the status the model binds it to
//! ([`ACCESS_DENIED_STATUS`], `@httpError(400)` —
//! `smithy_query(query="shape",shape="AccessDeniedException")`), whose documentation is
//! exactly this failure: "You don't have permissions to perform the requested operation. The
//! IAM principal making the request must have at least one IAM permissions policy attached
//! that grants the required permissions."
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS minted
//!    that key for (`sts_store::resolve_caller`, the same place `sts:GetCallerIdentity`
//!    answers from, so the principal a refusal blames is the one the caller reads back for
//!    the credentials it signed with), else the IAM user that owns the long-term key, else
//!    the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), so a policy
//!    `iam:PutRolePolicy` wrote takes effect on the next request and deleting it lifts it
//!    again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource)`, and the SENTENCE a refusal carries,
//!    so this service and every other one handed the same documents answer alike: an
//!    explicit `Deny` wins, otherwise an `Allow` about the request grants it, and with
//!    neither the request falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does.
//!
//! ## The action and the resource
//!
//! The action is the operation's own name under this service's action namespace
//! ([`ACTION_PREFIX`], the `arnNamespace` its model declares and the name every request to it
//! is signed under), so a statement spells it `eks-auth:AssumeRoleForPodIdentity`.
//!
//! The resource is the CLUSTER the request addresses, which EKS Auth publishes this action
//! against: the `cluster` resource type, `arn:<partition>:eks:<region>:<account>:cluster/<name>`
//! ([`crate::state::cluster_arn`], the same spelling a registered cluster's own record
//! carries). It is read off the request's URI alone ([`judged_resource`]) — the label the
//! modelled path binds the cluster name in — and built from what the request carried rather
//! than from a stored record, so the decision cannot depend on whether that cluster exists
//! and a refused caller learns nothing about what the account holds. A request whose URI
//! names no cluster at all is judged against [`ANY_RESOURCE`].
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root credentials
//!   are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An explicit
//!   `Deny` in a document that CAN be read still refuses, and a permissions boundary the
//!   store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents, so every action
//! it asks for is refused, and the refusal names that state rather than the key it presented,
//! which this module never quotes back.

use ::std::marker::PhantomData;
use ::std::sync::Arc;
use ::std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, ServerHttpProtocol, Service,
};

use crate::ops::types::AccessDeniedException;

/// This service's IAM action namespace: an action is `eks-auth:<OperationName>` ("Actions
/// defined by Amazon EKS Auth", IAM Service Authorization Reference), which is this service's
/// `arnNamespace` / `endpointPrefix` (`smithy_query(query="service_info")`) and also the name
/// every request to it is signed under.
pub const ACTION_PREFIX: &str = "eks-auth";

/// The wire name of the refusal this gate answers with, and the status the model binds it to:
/// `AccessDeniedException`, `@error("client")` at `@httpError(400)`
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
pub const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";
pub const ACCESS_DENIED_STATUS: u16 = 400;

/// The media type this restJson1 service renders an error in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The resource a request that addresses no cluster of its own is judged against, as IAM
/// writes it for such a request.
pub const ANY_RESOURCE: &str = "*";

/// The first path segment this service's one operation is bound under, whose next segment is
/// the `clusterName` label (`POST /clusters/{clusterName}/assume-role-for-pod-identity` —
/// `smithy_query(query="operation",operation="AssumeRoleForPodIdentity")`).
const CLUSTERS_PATH_SEGMENT: &str = "clusters";

/// Where the `clusterName` label sits among that path's segments.
const CLUSTER_NAME_SEGMENT: usize = 1;

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The IAM action `operation` is performed under: `eks-auth:<OperationName>`.
#[must_use]
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The Region this request was signed for, which the resource ARN a decision is taken against
/// carries.
///
/// Read from the caller's own SigV4 credential scope and checked against the Region grammar
/// before it is interpolated into an ARN: a scope naming something that is not a Region (or no
/// scope at all) falls back to [`crate::state::DEFAULT_REGION`] rather than letting
/// caller-chosen text reshape an ARN's colon-delimited fields.
#[must_use]
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| crate::state::DEFAULT_REGION.to_string())
}

/// Whether `region` is shaped like an AWS Region name: lower-case letters, digits and hyphens
/// only, and short enough that an ARN carrying it stays a readable ARN.
fn is_region_shaped(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 32
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The ARN the request ADDRESSES, which is what the action is judged against: the cluster its
/// URI names (see the module docs).
#[must_use]
pub fn judged_resource(ctx: &OperationContext<'_>, req: &RtRequest) -> String {
    let region = request_region(ctx.parts());
    match addressed_cluster(::service_common::request_path(req.uri())) {
        Some(cluster_name) => crate::state::cluster_arn(&region, &cluster_name),
        None => ANY_RESOURCE.to_string(),
    }
}

/// The cluster name `path` names, or `None` when it names none.
///
/// The label is decoded the way the codec that deserializes the request decodes it
/// ([`::service_common::percent_decode_path_label`]), so the gate judges the cluster name the
/// handler would read. An empty label names no cluster.
fn addressed_cluster(path: &str) -> Option<String> {
    if ::service_common::nth_path_segment(path, 0) != Some(CLUSTERS_PATH_SEGMENT) {
        return None;
    }
    ::service_common::nth_path_segment(path, CLUSTER_NAME_SEGMENT)
        .filter(|label| !label.is_empty())
        .map(::service_common::percent_decode_path_label)
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`Operation::NAME`]) and `resource` the ARN the request ADDRESSES.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see the
/// module docs); the modelled refusal otherwise. Taken BEFORE the input is read and before the
/// store is reached, so a refused call validates nothing, mints nothing and reports nothing
/// about what the account holds.
///
/// IAM's order: an explicit `Deny` in a document that CAN be read refuses the request whatever
/// else the principal carries; otherwise a grant is needed, which a caller this gate does not
/// bound has by definition; and a grant is capped by the permissions boundary, since "a
/// permissions boundary sets the maximum permissions that an identity-based policy can grant"
/// (IAM, permissions boundaries).
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises the
/// session expiries the clock has already made due — a write owed at the same instant whether
/// or not anybody called.
fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = action_for(operation);
    let caller = caller(engine, ctx);
    // An identity-based document states no `Principal` element, so the decision rests on the
    // action and the resource the request addressed.
    let request = iam::Request::new(&action, resource, &[]);
    let said = iam::identity_decision(&caller.documents, &request);
    if said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    if !(iam::is_allowed(said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            &iam::no_identity_policy_allows(&action),
        ));
    }
    let Some(boundary) = &caller.boundary else {
        return Ok(());
    };
    let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
        Decision::Allowed => return Ok(()),
        // An explicit deny is a fact about the boundary document, so the refusal names it; a
        // boundary that merely states nothing withheld the grant the policies gave.
        Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
        Decision::NotStated => iam::no_permissions_boundary_allows(&action),
    };
    Err(not_authorized(&caller.arn, &action, resource, &reason))
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it — the one sentence every authorizing
/// service in this tree words it as ([`iam::not_authorized_by_identity_policy`]). Every value
/// in it comes from the request and from the stored policies.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(iam::not_authorized_by_identity_policy(
            principal, action, resource, reason,
        )),
    }
}

/// The wire answer to a request this gate refused: the modelled `AccessDeniedException` at its
/// modelled status, serialized through the SAME protocol a handler-returned error of that
/// shape goes through, so a refusal taken in the layer and one taken in a handler are
/// indistinguishable on the wire.
///
/// The hand-built fallback carries the same three things a client branches on — the status, the
/// shape in `x-amzn-errortype` and the `__type` body — for the one case the serializer can
/// fail: a value it cannot render.
#[must_use]
fn refusal_response(protocol: &dyn ServerHttpProtocol, denial: &AccessDeniedException) -> RtResponse {
    protocol
        .serialize_error(
            denial,
            &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_STATUS,
        )
        .unwrap_or_else(|_| {
            let body = ::serde_json::json!({
                "__type": ACCESS_DENIED_EXCEPTION,
                "message": denial.message.clone().unwrap_or_default(),
            });
            let mut response = RtResponse::new(
                StatusCode::try_from(ACCESS_DENIED_STATUS)
                    .expect("the status is a constant of this module"),
                SdkBody::from(body.to_string()),
            );
            response
                .headers_mut()
                .insert("content-type", JSON_CONTENT_TYPE);
            response
                .headers_mut()
                .insert("x-amzn-errortype", ACCESS_DENIED_EXCEPTION);
            response
        })
}

/// One modelled operation, served only to a caller IAM allows to perform it.
///
/// It carries the operation TYPE — which is what names the action and resolves the delegate —
/// and a handle on the ONE shared engine, which is what the decision reads the caller's
/// identity and its policies through. Every request it does not refuse is delegated to the one
/// operation instance the generated routing table built from that same engine, so there is no
/// second instance of the operation and no second store, and the framework still owns the
/// whole deserialize -> validate -> handle -> serialize sequence behind it.
pub struct IamAuthorized<Op, Eng> {
    operation: PhantomData<fn() -> Op>,
    engine: Arc<Eng>,
}

impl<Op, Eng> IamAuthorized<Op, Eng> {
    /// Put one operation behind the IAM decision, judged against the policies held in
    /// `engine`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            operation: PhantomData,
            engine,
        }
    }
}

impl<Op: Operation, Eng: Send + Sync + 'static> ErasedOperation for IamAuthorized<Op, Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // IAM's decision, taken before the request's input is read: the action is this
            // operation's own name under this service's namespace, and the resource is the
            // cluster the REQUEST addresses. A refused request never reaches the delegate, so
            // its body is never validated, nothing is read from state and nothing is minted.
            let resource = judged_resource(ctx, req);
            if let Err(denial) = authorize(&*self.engine, ctx, Op::NAME, &resource) {
                // The access record for a refused call: who called, what they asked for and
                // the modelled error they were answered with. The request-derived values are
                // recorded with `?`, so each is quoted and its control characters escaped and
                // cannot forge a line; no part of the token is recorded at all.
                ::tracing::info!(
                    operation = Op::NAME,
                    caller_akid = ?::engine::extract_akid(&ctx.parts().headers),
                    resource = ?resource,
                    error_code = %ACCESS_DENIED_EXCEPTION,
                    "iam does not allow this caller to perform this action"
                );
                // The protocol is read, used and dropped inside this one statement:
                // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
                // below would make this future non-`Send`.
                return refusal_response(svc.definition().protocol(), &denial);
            }
            match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key the
                // generated table registered. Answer the framework's own no-such-operation
                // response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapper is not an operation of its own: the request is signed as the MODEL says
        // for the operation it names, so authentication is decided by the modelled operation
        // this wrapper stands in front of.
        None
    }
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose document
    /// this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no IAM
    /// policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's OWN
    /// accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's own
            // catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(engine: &Eng, ctx: &OperationContext<'_>) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential, which holds no identity policy.
        None => Ok(sts_store::ambient_identity()),
    };
    // An expired session credential speaks for nobody: it carries no document, so every action
    // it asks for is refused — and the refusal names the credential's STATE rather than the
    // key id that was presented, which this module never quotes back.
    let identity = match identity {
        Ok(identity) => identity,
        Err(_) => {
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                ..Caller::default()
            };
        }
    };
    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        arn: identity.arn,
        ..Caller::default()
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to the
    // user that owns it (the map `iam:GetUser` resolves a caller through), so a request signed
    // with one speaks for THAT user and is judged on the user's policies. A key the IAM store
    // does not hold either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
        }
    }
    // Which IAM identity the principal's policies are held under is read through the tree's
    // shared readings of a principal ARN.
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(role_arn) = store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone())
        {
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What one stored document says about `action` on `resource`, through the shared evaluator
    /// this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `eks-auth:<OperationName>`, so a document scoped
    /// to this service's actions decides its operation and nothing outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_namespace() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"eks-auth:*","Resource":"*"}]}"#;
        let action = action_for("AssumeRoleForPodIdentity");
        assert_eq!(
            action, "eks-auth:AssumeRoleForPodIdentity",
            "case \"the modelled operation\": expected this service's namespace, got {action:?}"
        );
        let reached = decision(document, &action, ANY_RESOURCE);
        assert_eq!(
            reached,
            Decision::Allowed,
            "case \"this service's actions\": expected eks-auth:* to allow {action}, got \
             {reached:?}"
        );
        // EKS proper is a DIFFERENT action namespace, so a grant of its actions grants nothing
        // here — which is the whole point of judging the action rather than the endpoint.
        let outside = decision(document, "eks:DescribeCluster", ANY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected eks-auth:* to state nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: a request is judged against the CLUSTER its URI names, read as the codec reads the
    /// label, and against [`ANY_RESOURCE`] when it names none.
    ///
    /// The rows span the modelled path, an escaped label, a label the model's pattern refuses
    /// (the gate decides before validation, so it must still read one), an empty label, a path
    /// under another first segment and the root path.
    #[test]
    fn a_request_is_judged_against_the_cluster_its_uri_names() {
        // (case, the request path, the cluster name it addresses)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "the modelled path",
                "/clusters/demo-cluster/assume-role-for-pod-identity",
                Some("demo-cluster"),
            ),
            (
                "another cluster name",
                "/clusters/Prod2/assume-role-for-pod-identity",
                Some("Prod2"),
            ),
            (
                "an escaped label",
                "/clusters/demo%2Dcluster/assume-role-for-pod-identity",
                Some("demo-cluster"),
            ),
            (
                "a label the modelled pattern refuses",
                "/clusters/-bad-cluster/assume-role-for-pod-identity",
                Some("-bad-cluster"),
            ),
            (
                "an empty label",
                "/clusters//assume-role-for-pod-identity",
                None,
            ),
            ("no label at all", "/clusters", None),
            ("another path space", "/pods/demo-cluster", None),
            ("the root path", "/", None),
        ];
        for (case, path, expected) in rows {
            let addressed = addressed_cluster(path);
            assert_eq!(
                addressed.as_deref(),
                *expected,
                "case {case:?}: expected the request to address {expected:?}, got {addressed:?}"
            );
        }
    }

    /// RULE: the cluster ARN a decision is taken against carries the Region the request was
    /// SIGNED for, and a credential scope whose Region field is not a Region name cannot
    /// reshape the ARN — it falls back to the Region this service's own ARNs default to, so no
    /// caller-supplied text reaches an ARN field.
    #[test]
    fn the_region_of_the_judged_arn_comes_from_the_credential_scope() {
        // (case, the scope's Region field, the Region the ARN must carry)
        let rows: &[(&str, Option<&str>, &str)] = &[
            ("a Region name", Some("us-west-2"), "us-west-2"),
            (
                "another Region name",
                Some("ap-southeast-4"),
                "ap-southeast-4",
            ),
            ("one carrying a colon", Some("us-east-2:evil"), "us-east-1"),
            ("one carrying a slash", Some("us-east-2/evil"), "us-east-1"),
            ("one in another case", Some("US-WEST-2"), "us-east-1"),
            ("the empty field", Some(""), "us-east-1"),
            ("no credential scope at all", None, "us-east-1"),
        ];
        for (case, signed, expected) in rows {
            let parts = signed_for(*signed);
            let actual = request_region(&parts);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected the Region {expected:?}, got {actual:?}"
            );
            assert_eq!(
                crate::state::cluster_arn(&actual, "demo-cluster"),
                crate::state::cluster_arn(expected, "demo-cluster"),
                "case {case:?}: expected the ARN of the cluster in {expected:?}"
            );
        }
    }

    /// The request parts of a call whose SigV4 credential scope carries `region` in its Region
    /// field. A row passing `None` carries no `Authorization` header at all.
    fn signed_for(region: Option<&str>) -> ::http::request::Parts {
        let mut request = ::http::Request::builder()
            .uri("https://localhost/clusters/demo-cluster/assume-role-for-pod-identity");
        if let Some(region) = region {
            request = request.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{ACTION_PREFIX}/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            );
        }
        request
            .body(())
            .expect("the request is well formed")
            .into_parts()
            .0
    }

    /// RULE: an explicit `Deny` is final, whatever else the principal carries — which is the
    /// case this gate was written for, and the one a least-privilege boundary rests on.
    #[test]
    fn an_explicit_deny_wins_over_a_wildcard_grant() {
        let document = r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"*","Resource":"*"},
            {"Effect":"Deny","Action":"eks-auth:AssumeRoleForPodIdentity","Resource":"*"}]}"#;
        let action = action_for("AssumeRoleForPodIdentity");
        let cluster = crate::state::cluster_arn("us-west-2", "demo-cluster");
        let denied = decision(document, &action, &cluster);
        assert_eq!(
            denied,
            Decision::Denied,
            "case \"the denied action\": expected Denied, got {denied:?}"
        );
        let allowed = decision(document, "eks:DescribeCluster", &cluster);
        assert_eq!(
            allowed,
            Decision::Allowed,
            "case \"an action the deny does not name\": expected Allowed, got {allowed:?}"
        );
    }

    /// RULE: a grant is read for the action AND the cluster it names, so a statement naming
    /// another cluster — or another Region's spelling of the same name — allows nothing.
    #[test]
    fn a_grant_is_read_for_the_cluster_it_names() {
        let granted = crate::state::cluster_arn("us-west-2", "demo-cluster");
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Action":"eks-auth:AssumeRoleForPodIdentity","Resource":"{granted}"}}]}}"#
        );
        // (case, the resource the call addressed, allowed)
        let rows: &[(&str, String, bool)] = &[
            ("the granted cluster", granted.clone(), true),
            (
                "another cluster of the same Region",
                crate::state::cluster_arn("us-west-2", "Prod2"),
                false,
            ),
            (
                "the same name in another Region",
                crate::state::cluster_arn("eu-central-1", "demo-cluster"),
                false,
            ),
            (
                "a request naming no cluster",
                ANY_RESOURCE.to_string(),
                false,
            ),
        ];
        let action = action_for("AssumeRoleForPodIdentity");
        for (case, resource, expected) in rows {
            let said = decision(&document, &action, resource);
            let actual = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected allowed == {expected}, got {actual} ({said:?})"
            );
        }
    }
}
