// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it names — the decision
//! taken in FRONT of the operation, before it reads or writes anything.
//!
//! An operation of this API that the model requires a SigV4 signature for is called with
//! IAM credentials, and "Amazon Cognito evaluates Identity and Access Management (IAM)
//! policies in requests for this API operation ... you must grant yourself the
//! corresponding IAM permission in a policy"
//! (`smithy_query(query="operation",operation="GetCSVHeader")`, which carries that
//! paragraph). So a caller whose policies do not allow the action it asked for is told it
//! is not authorized — with the AWS-common `AccessDeniedException`, which this model
//! declares no shape of its own for — rather than told whether the pool it named exists,
//! and nothing of the account changes on the way to that refusal.
//!
//! The operations the model marks `@optionalAuth` (`InitiateAuth`, `SignUp`, `GetUser`,
//! `ChangePassword`, … — `smithy_query(query="digest")`) are the unauthenticated and
//! access-token surface: such a request carries no IAM credential and speaks for no
//! principal, so none of them is judged here. WHICH operation is which is read off the
//! operation's own modelled auth schemes ([`ErasedOperation::auth_schemes`], which the
//! generated code leaves EMPTY for `@optionalAuth`), so the set cannot drift from the
//! model and no second list of operation names is kept.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the SESSION it was
//!    minted for, read from the account's shared session directory
//!    ([`::service_common::management_events::SessionAuditDirectory`], where the minting
//!    service records every session it hands out), else the IAM user that owns that
//!    long-term key, read from IAM's own `access_keys` map (the map `iam:GetUser` resolves a
//!    caller through, so the principal a refusal blames is the one IAM holds the key under),
//!    else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's own accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule lives — so a document `iam:PutUserPolicy` wrote takes
//!    effect on the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` wins, otherwise an `Allow`
//!    about the request grants it, and with neither the request falls to IAM's default
//!    deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the reading of a principal
//! ARN does. That is the split every other gate in this tree makes, and the lookups stay in
//! this crate for the reason those keep theirs there: hoisting them into `service-common`
//! would make every service depending on that crate link the IAM service crate.
//!
//! ## The action and the resource
//!
//! The action is `cognito-idp:<OperationName>` — this service's `arnNamespace`
//! ([`crate::state::SIGNING_NAME`], `smithy_query(query="service_info")`) and the
//! operation's own shape name — so one policy statement scoped to this service's actions
//! covers every operation of it and nothing outside it.
//!
//! The resource is what the request ADDRESSES, computed before anything is looked up so the
//! decision cannot depend on whether that resource exists (and so a refused caller learns
//! nothing about what the account holds): the ARN of the user pool a request naming a
//! `UserPoolId` addresses, the `ResourceArn` a tagging call addresses, and otherwise every
//! user pool of this account in the request's Region ([`any_user_pool_arn`]) for a call that
//! addresses no single pool — a listing, or a create whose pool id does not exist yet.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id the IAM store
//!   holds no record for resolves to the account's own identity, which is not a principal
//!   that store holds policies for — the position AWS account root credentials are in. Such
//!   a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no IAM store**, where no principal and no policy can exist.
//! * **A session whose ISSUING ROLE this account does not hold a record of.** The
//!   permissions of an assumed-role session are the role's, so the documents in force are
//!   the ones IAM holds under the role's own ARN; a session the directory names no issuer for
//!   is judged on the documents IAM holds for the session principal itself.
//!
//! WHICH session a temporary credential belongs to is read from the SHARED directory rather
//! than from the minting service's own store, because that store is unreachable from here:
//! STS depends on THIS service (a user pool is an identity provider it verifies web identity
//! tokens against), so a dependency back on STS is a cycle Cargo refuses. The directory is
//! the one place in this tree where the facts about a session live for every service that
//! serves a call made with it, which is exactly the position this gate is in.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::protocol::ServerProtocol;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The wire error code of an identity-based authorization refusal, as the AWS "Common
/// Errors" table names it — what a client branches on for "not permitted". This model
/// declares no shape for it: the refusal belongs to the authorization layer in front of the
/// service rather than to any one operation.
pub const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The status this endpoint answers that refusal under, which is the status it answers every
/// refusal of the caller under ([`super::error_wire`]): on this `awsJson1_1` endpoint the
/// error class travels in the body's `__type` and the transport status stays 400.
const ACCESS_DENIED_STATUS: u16 = 400;

/// The member a request names the user pool it addresses by.
const USER_POOL_ID_MEMBER: &str = "UserPoolId";

/// The member a tagging call addresses its resource by.
const RESOURCE_ARN_MEMBER: &str = "ResourceArn";

/// The `message` member of the synthetic `AccessDeniedException` error shape.
static ACCESS_DENIED_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#AccessDeniedException$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
)
.with_xml_name("Message");

/// Schema for the synthetic `AccessDeniedException` error shape.
///
/// Deliberately in the `smithy.framework` namespace: `AccessDeniedException` is one of the
/// AWS COMMON errors, which the authorization layer ahead of a service's own code answers
/// and this service's model declares no shape for, so it must not collide with a modelled
/// one. The wire `__type` comes from [`ACCESS_DENIED_CODE`], not from this schema.
static ACCESS_DENIED_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#AccessDeniedException",
        "smithy.framework",
        "AccessDeniedException",
    ),
    ShapeType::Structure,
    &[&ACCESS_DENIED_MESSAGE],
);

/// The payload of an `AccessDeniedException`: which principal was refused which action on
/// which resource, and why no grant was found.
#[derive(Debug, Clone)]
struct AccessDeniedException {
    message: String,
}

impl SerializableStruct for AccessDeniedException {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        serializer.write_string(&ACCESS_DENIED_MESSAGE, self.message.as_str())?;
        Ok(())
    }
}

/// One modelled operation, served only to a caller IAM allows to perform it.
///
/// Wrapping is what makes the decision ONE decision: every operation the model requires a
/// signature for is judged by the same rule, in front of the handler, so no handler can
/// forget it and none of them takes it twice. An operation the model marks `@optionalAuth`
/// is handed straight through — see the module docs.
pub struct IamAuthorized<Eng> {
    /// The operation shape name, which is both this entry's routing key and the name of the
    /// IAM action the request is judged under.
    name: &'static str,
    /// The shared engine, so the principal's policies are read from the one store every
    /// service of this emulator shares.
    engine: Arc<Eng>,
    /// The operation this one wraps, served for every request IAM allows.
    operation: Box<dyn ErasedOperation>,
}

impl<Eng> IamAuthorized<Eng> {
    /// Put `operation`, already constructed from the shared engine, behind the IAM decision.
    pub fn new(name: &'static str, engine: Arc<Eng>, operation: Box<dyn ErasedOperation>) -> Self {
        Self {
            name,
            engine,
            operation,
        }
    }

    /// Whether the model requires a SigV4 signature for the wrapped operation, which is what
    /// makes it a call IAM policies are evaluated for (see the module docs). The schemes are
    /// the operation's OWN modelled ones, empty for `@optionalAuth`.
    fn signed(&self) -> bool {
        self.operation
            .auth_schemes()
            .is_some_and(|schemes| !schemes.is_empty())
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for IamAuthorized<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if self.signed() {
                if let Some(refused) = refusal(&*self.engine, ctx, self.name, req) {
                    return refused;
                }
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The IAM action `operation` is performed under: `cognito-idp:<OperationName>`.
pub fn action_for(operation: &str) -> String {
    format!("{}:{operation}", crate::state::SIGNING_NAME)
}

/// The ARN of the user pool `user_pool_id` names, in `region` — the resource a statement
/// about one pool matches, built from the request rather than from a stored record.
pub fn user_pool_arn(region: &str, user_pool_id: &str) -> String {
    ::engine::emulator_arn(
        crate::state::SIGNING_NAME,
        region,
        &format!(
            "{}/{user_pool_id}",
            crate::state::USER_POOL_RESOURCE_TYPE
        ),
    )
}

/// Every user pool of this account in `region`, spelled with IAM's own resource wildcard:
/// the resource a call that addresses no single pool is decided against — a listing, or a
/// create whose pool does not exist yet. A statement scoped to one pool cannot allow such a
/// call, while one covering the account's pools can.
pub fn any_user_pool_arn(region: &str) -> String {
    user_pool_arn(region, "*")
}

/// The response a request IAM does not allow is answered with, or `None` when the caller may
/// perform the action — which includes every caller this gate does not bound (see the module
/// docs).
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    operation: &str,
    req: &RtRequest,
) -> Option<RtResponse> {
    let caller = caller(engine, ctx);
    // Nothing this gate can read bears on the request: there is no document that could deny
    // it, no boundary that could cap it, and the credential is one the gate does not bound.
    // Taken first so the ordinary caller's request is not read for a decision already made.
    if caller.documents.is_empty() && caller.boundary.is_none() && caller.unbounded() {
        return None;
    }
    let action = action_for(operation);
    let resource = request_resource(ctx, req);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, so the decision below rests on the action and the resource.
    let request = iam::Request::new(&action, &resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    let reason = if identity_said == Decision::Denied {
        "with an explicit deny in an identity-based policy".to_string()
    } else if !(iam::is_allowed(identity_said, Decision::NotStated) || caller.unbounded()) {
        format!("because no identity-based policy allows the {action} action")
    } else {
        // "A permissions boundary sets the maximum permissions that an identity-based policy
        // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
        // is not a permission the principal has. The account's own credential carries none.
        match caller.boundary.as_ref().map(|boundary| {
            iam::identity_decision(::std::slice::from_ref(boundary), &request)
        }) {
            None | Some(Decision::Allowed) => return None,
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Some(Decision::Denied) => "with an explicit deny in a permissions boundary".to_string(),
            Some(Decision::NotStated) => {
                format!("because no permissions boundary allows the {action} action")
            }
        }
    };
    let message = not_authorized(&caller.arn, &action, &resource, &reason);
    tracing::info!(
        operation = %operation,
        action = %action,
        resource = %resource,
        "iam does not allow this caller to perform this action"
    );
    Some(denied_response(&message))
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it. Every value in it comes from the
/// request and from the stored policies — the principal from the credential the request
/// presented, the action from the operation it named, the resource from what it addressed.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// The resource this request is judged against: what it ADDRESSES, read off the request
/// itself (see the module docs).
fn request_resource(ctx: &OperationContext<'_>, req: &RtRequest) -> String {
    let region = crate::state::request_region(ctx.parts());
    let submitted: Option<serde_json::Value> = req
        .body()
        .bytes()
        .and_then(|body| serde_json::from_slice(body).ok());
    let member = |name: &str| {
        submitted
            .as_ref()
            .and_then(|body| body.get(name))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    member(USER_POOL_ID_MEMBER)
        .map(|user_pool_id| user_pool_arn(&region, &user_pool_id))
        .or_else(|| member(RESOURCE_ARN_MEMBER))
        .unwrap_or_else(|| any_user_pool_arn(&region))
}

/// The refusal as this endpoint puts it on the wire: the AWS-common `AccessDeniedException`,
/// rendered in the same `awsJson1_1` envelope every other error of this endpoint is rendered
/// in and naming its shape in the header an AWS client resolves the error class from.
fn denied_response(message: &str) -> RtResponse {
    let refused = AccessDeniedException {
        message: message.to_string(),
    };
    let rendered = ::aws_smithy_schema_server::rpc::AwsJson11().serialize_error(
        &refused,
        &ACCESS_DENIED_SCHEMA,
        ACCESS_DENIED_CODE,
        ACCESS_DENIED_STATUS,
    );
    match rendered {
        Ok(response) => super::error_wire::naming_the_error_shape(response, ACCESS_DENIED_CODE),
        // The envelope is built from this module's own schema and a message of its own, so
        // it cannot fail to render; a failure is still answered rather than letting the
        // caller wait on nothing, and it is answered as a REFUSAL, never as a success.
        Err(failed) => {
            tracing::warn!(
                error = %failed,
                "the authorization refusal could not be rendered in this service's envelope"
            );
            super::error_wire::naming_the_error_shape(
                ::aws_smithy_runtime_api::http::Response::new(
                    ::aws_smithy_runtime_api::http::StatusCode::try_from(ACCESS_DENIED_STATUS)
                        .expect("400 is a status"),
                    ::aws_smithy_types::body::SdkBody::from(
                        format!("{{\"__type\":\"{ACCESS_DENIED_CODE}\"}}").as_bytes(),
                    ),
                ),
                ACCESS_DENIED_CODE,
            )
        }
    }
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the IAM
    /// role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
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
    fn unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's own
    /// accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
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
    let mut caller = Caller {
        account_credential: true,
        principals: vec![::service_common::caller_principal::ambient_user_arn()],
        arn: ::service_common::caller_principal::ambient_user_arn(),
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
    };
    // A SESSION credential: the account's shared session directory holds the session every
    // temporary credential this emulator minted belongs to, so a request signed with one
    // speaks for THAT session — the principal `sts:GetCallerIdentity` reports for it — and is
    // judged on the documents in force on the role that issued it.
    let session = session_directory(engine)
        .and_then(|directory| directory.lock().session(&access_key_id).cloned());
    // No IAM store on this engine: no principal and no policy can exist, so nothing can be
    // judged and the caller is left unbounded (see the module docs) — whatever credential it
    // presented.
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    if let Some(session) = session {
        caller.account_credential = false;
        caller.arn = session.arn.clone();
        caller.principals = vec![session.arn];
        // "The permissions of the temporary security credentials are determined by the role
        // that was assumed": the documents are the ISSUER's, under the ARN the account holds
        // the role by — which carries its PATH, where the session ARN does not.
        if let Some(issuer) = session.issuer {
            caller.principals.push(issuer.arn.clone());
            caller.read_policies_of(&store, &issuer.arn);
        } else {
            let principal = caller.arn.clone();
            caller.read_policies_of(&store, &principal);
        }
        return caller;
    }
    // IAM's store maps an access key id to the user that owns it (`access_keys`, the map
    // IAM's own `GetUser` resolves a caller through), so a request signed with one speaks for
    // THAT user and is judged on the user's policies rather than served as the account. A key
    // that store does not hold is the account's own published credential.
    if let Some(user) = store
        .access_keys
        .get(&access_key_id)
        .and_then(|record| store.users.get(&record.user_key))
    {
        caller.account_credential = false;
        caller.arn = user.arn.clone();
        caller.principals = vec![user.arn.clone()];
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

/// The account's SESSION directory in the shared engine: every session this emulator minted,
/// keyed by the access key id it was minted under.
///
/// A SHARED type ([`::service_common::management_events::SessionAuditDirectory`]) rather than
/// a field of any service's own state, because the service that MINTS a session is not the
/// service that serves the calls made with it (see the module docs). Read-only here: the gate
/// writes nothing.
fn session_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::management_events::SessionAuditDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::management_events::SessionAuditDirectory,
            >,
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about carries this service's own prefix, so a
    /// statement scoped to this service's actions covers every operation of it and nothing
    /// outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"cognito-idp:*","Resource":"*"}]}"#;
        for operation in [
            "GetCSVHeader",
            "CreateUserImportJob",
            "DescribeUserPool",
            "AdminCreateUser",
            "TagResource",
        ] {
            let action = action_for(operation);
            let reached = decision(document, &action, "*");
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected the service-wide allow to cover {action}, got \
                 {reached:?}"
            );
        }
        let outside = decision(document, "cognito-identity:DescribeIdentityPool", "*");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected the service-wide allow to grant nothing outside \
             this service, got {outside:?}"
        );
    }

    /// RULE: a grant is read for the action AND the resource it names, so a statement scoped
    /// to one pool allows nothing about another and nothing about a call that addresses no
    /// single pool.
    #[test]
    fn a_grant_is_read_for_the_action_and_the_resource_it_names() {
        let region = "us-west-2";
        let pool = user_pool_arn(region, "us-west-2_1a2b3c4d5");
        let another = user_pool_arn(region, "us-west-2_9z8y7x6w5");
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Action":"cognito-idp:GetCSVHeader","Resource":"{pool}"}}]}}"#
        );
        // (case, operation, resource, allowed)
        let rows: &[(&str, &str, &str, bool)] = &[
            ("the granted action on the granted pool", "GetCSVHeader", &pool, true),
            ("another action on the same pool", "CreateUserImportJob", &pool, false),
            ("the granted action on another pool", "GetCSVHeader", &another, false),
            (
                "the granted action with no single pool named",
                "GetCSVHeader",
                &any_user_pool_arn(region),
                false,
            ),
        ];
        for (case, operation, resource, expected) in rows {
            let said = decision(&document, &action_for(operation), resource);
            let actual = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected allowed == {expected}, got {actual} ({said:?})"
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource the request addressed
    /// and which of IAM's rules refused it — every part taken from the request and the
    /// stored policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = format!(
            "arn:aws:iam::{}:user/noperm-probe",
            ::engine::emulator_account_id()
        );
        let action = action_for("GetCSVHeader");
        let resource = user_pool_arn("us-west-2", "us-west-2_1a2b3c4d5");
        // (case, the clause the message must carry)
        let rows: &[(&str, String)] = &[
            (
                "no policy allows it",
                format!("because no identity-based policy allows the {action} action"),
            ),
            (
                "a policy denies it",
                "with an explicit deny in an identity-based policy".to_string(),
            ),
            (
                "a boundary withholds it",
                format!("because no permissions boundary allows the {action} action"),
            ),
        ];
        for (case, reason) in rows {
            let message = not_authorized(&principal, &action, &resource, reason);
            let expected = format!(
                "User: {principal} is not authorized to perform: {action} on resource: \
                 {resource} {reason}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}
