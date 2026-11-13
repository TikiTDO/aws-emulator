// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! decision taken in front of every operation, before any handler reads or writes
//! anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at the
//! request's own input, so a caller whose identity-based policies do not grant the action
//! is told it is not authorized rather than told anything about what the account holds,
//! and the call changes nothing — not even for an operation this emulator has not
//! implemented, because the refusal is older than the handler. It is a fact of the
//! CREDENTIAL the request presented and of the policies in force on the principal it
//! speaks for, not of the store, so it is decided once here rather than in each handler.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    owns the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a policy `iam:PutUserPolicy` wrote takes
//!    effect on the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor its wording does.
//!
//! ## What the decision is made about
//!
//! The ACTION is `timestream-influxdb:<OperationName>` ("Actions defined by Amazon
//! Timestream InfluxDB", IAM Service Authorization Reference), this service's
//! `arnNamespace` (`smithy_query(query="service_info")`) in front of the operation the
//! request named.
//!
//! The RESOURCE is the one the REQUEST addresses, read from the request rather than from a
//! stored record so the decision does not depend on whether that resource exists — which
//! is also what keeps a refused call from telling an unauthorized caller whether it does:
//! the ARN of the record a request names the id of, and otherwise every resource of the
//! addressed family in that Region, which is what a create (whose id is minted after the
//! decision, so no policy could have been written against it) and a listing address
//! ([`JUDGED_RESOURCES`]).
//!
//! This service publishes no resource-based policy — the model declares no policy
//! operation and no policy member — so the decision here is the identity-based one alone.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the IAM store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits.
//!   An explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no user can exist.
//! * **An operation whose model declares no `AccessDeniedException`** — the three tagging
//!   operations of this service — because there is no modelled answer for it to refuse an
//!   unauthorized caller with, and answering an error its own model does not declare would
//!   be a different divergence. [`JUDGED_RESOURCES`] is held to the modelled set by this
//!   module's own test.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)
//!
//! Resolving the caller goes through STS's own `resolve_caller`, which first materialises
//! the session expiries the clock has already made due; that write is owed at the same
//! instant whether or not anybody called, which is why a REPORTING operation of this
//! service may take this decision too.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, ServerHttpProtocol, Service,
};

use crate::ops::types::AccessDeniedException;
use crate::state as st;

/// The wire name of the refusal this gate answers with, and the status the model gives it:
/// `AccessDeniedException`, `@error("client")` at `@httpError(403)`
/// (`smithy_query(query="shape", shape="AccessDeniedException")`).
pub const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The media type this awsJson1_0 service renders an error in, for the one path that
/// builds the envelope by hand ([`refusal_response`]).
const JSON_CONTENT_TYPE: &str = "application/x-amz-json-1.0";

/// The id segment of the ARN naming EVERY resource of a family in one Region, as IAM
/// writes it: the resource a request that names no single existing record is judged
/// against.
pub const ANY_RESOURCE: &str = "*";

/// What each operation this gate judges addresses: the ARN resource segment of the family
/// the action acts on, and the input member carrying the id of the record it names —
/// `None` where the request names no existing record of its own, which is a create (whose
/// id is minted only after the decision) or a listing of the whole family.
///
/// Source: the input shapes of this model
/// (`smithy_query(query="operation", operation="…")`). The operations listed here are
/// EXACTLY the ones whose model declares `AccessDeniedException` — asserted against the
/// model by this module's own test — because that is the refusal this gate answers with.
pub const JUDGED_RESOURCES: &[(&str, &str, Option<&str>)] = &[
    ("CreateDbBackup", st::ARN_SEGMENT_DB_BACKUP, None),
    ("CreateDbCluster", st::ARN_SEGMENT_DB_CLUSTER, None),
    ("CreateDbInstance", st::ARN_SEGMENT_DB_INSTANCE, None),
    (
        "CreateDbParameterGroup",
        st::ARN_SEGMENT_DB_PARAMETER_GROUP,
        None,
    ),
    (
        "DeleteDbBackup",
        st::ARN_SEGMENT_DB_BACKUP,
        Some("identifier"),
    ),
    (
        "DeleteDbCluster",
        st::ARN_SEGMENT_DB_CLUSTER,
        Some("dbClusterId"),
    ),
    (
        "DeleteDbInstance",
        st::ARN_SEGMENT_DB_INSTANCE,
        Some("identifier"),
    ),
    ("GetDbBackup", st::ARN_SEGMENT_DB_BACKUP, Some("identifier")),
    (
        "GetDbCluster",
        st::ARN_SEGMENT_DB_CLUSTER,
        Some("dbClusterId"),
    ),
    (
        "GetDbInstance",
        st::ARN_SEGMENT_DB_INSTANCE,
        Some("identifier"),
    ),
    (
        "GetDbParameterGroup",
        st::ARN_SEGMENT_DB_PARAMETER_GROUP,
        Some("identifier"),
    ),
    ("ListDbBackups", st::ARN_SEGMENT_DB_BACKUP, None),
    ("ListDbClusters", st::ARN_SEGMENT_DB_CLUSTER, None),
    ("ListDbInstances", st::ARN_SEGMENT_DB_INSTANCE, None),
    (
        "ListDbInstancesForCluster",
        st::ARN_SEGMENT_DB_CLUSTER,
        Some("dbClusterId"),
    ),
    (
        "ListDbParameterGroups",
        st::ARN_SEGMENT_DB_PARAMETER_GROUP,
        None,
    ),
    (
        "RebootDbCluster",
        st::ARN_SEGMENT_DB_CLUSTER,
        Some("dbClusterId"),
    ),
    (
        "RebootDbInstance",
        st::ARN_SEGMENT_DB_INSTANCE,
        Some("identifier"),
    ),
    (
        "RestoreFromDbBackup",
        st::ARN_SEGMENT_DB_BACKUP,
        Some("dbBackupId"),
    ),
    (
        "UpdateDbCluster",
        st::ARN_SEGMENT_DB_CLUSTER,
        Some("dbClusterId"),
    ),
    (
        "UpdateDbInstance",
        st::ARN_SEGMENT_DB_INSTANCE,
        Some("identifier"),
    ),
];

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// Every operation this gate judges, as `Operation::NAME` spells it.
#[must_use]
pub fn judged_operations() -> Vec<&'static str> {
    JUDGED_RESOURCES
        .iter()
        .map(|(operation, _, _)| *operation)
        .collect()
}

/// The ARN the action of `operation` is judged against for a request carrying `document`
/// as its body, in `region`.
///
/// The record's own ARN where the request names that record's id, and otherwise every
/// resource of the addressed family in the Region ([`ANY_RESOURCE`]) — see the module
/// docs. An operation this gate does not judge has no resource to answer.
#[must_use]
pub fn judged_resource(
    operation: &str,
    document: &::serde_json::Value,
    region: &str,
) -> Option<String> {
    let (_, segment, member) = JUDGED_RESOURCES
        .iter()
        .find(|(name, _, _)| *name == operation)?;
    let id = member
        .and_then(|member| document.get(member))
        .and_then(::serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(ANY_RESOURCE);
    Some(st::family_arn(segment, region, id))
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`Operation::NAME`]) and `resource` the ARN the request ADDRESSES.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise. Taken BEFORE the store is reached, so
/// a refused call writes nothing.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{}:{operation}", st::ARN_NAMESPACE);
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and this service publishes no resource-based policy (see the module
    // docs), so the decision below rests on the action and the resource.
    let request = iam::Request::new(&action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            &iam::no_identity_policy_allows(&action),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
    // is not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(&action),
        };
        return Err(not_authorized(&caller.arn, &action, resource, &reason));
    }
    Ok(())
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it — the one sentence every
/// authorizing service in this tree words it as
/// ([`iam::not_authorized_by_identity_policy`]). Every value in it comes from the request
/// and from the stored policies.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: iam::not_authorized_by_identity_policy(principal, action, resource, reason),
    }
}

/// The wire answer to a request this gate refused: the modelled `AccessDeniedException` at
/// its modelled 403, serialized through the SAME protocol a handler-returned error of that
/// shape goes through, so a refusal taken in this layer and one taken in a handler are
/// indistinguishable on the wire.
///
/// The hand-built fallback carries the same two things a client branches on — the status
/// and the shape in the body's `__type` — for the one case the serializer can fail: a
/// value it cannot render.
#[must_use]
pub fn refusal_response(
    protocol: &dyn ServerHttpProtocol,
    denial: &AccessDeniedException,
) -> RtResponse {
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
                "message": denial.message.clone(),
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

/// One modelled operation, served behind IAM's decision about the action it names.
///
/// It carries the operation TYPE — which is what names the action and resolves the
/// delegate — and a handle on the ONE shared engine, which is what the decision reads the
/// caller's identity and its policies through. Every request it does not refuse is
/// delegated to the one operation instance the generated routing table built from that
/// same engine, so there is no second instance of any operation and no second store.
pub struct Gated<Op, Eng> {
    operation: PhantomData<fn() -> Op>,
    engine: Arc<Eng>,
}

impl<Op, Eng> Gated<Op, Eng> {
    /// Wrap one operation, judged against the policies held in `engine`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            operation: PhantomData,
            engine,
        }
    }
}

impl<Op: Operation, Eng: Send + Sync + 'static> ErasedOperation for Gated<Op, Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // IAM's decision, taken before the operation reads or writes anything: the
            // action is this operation's own name under this service's prefix, and the
            // resource is the one the REQUEST names. A refused request never reaches the
            // delegate, so it mints nothing, moves no status and reports nothing.
            let region = st::request_region(ctx.parts());
            let document = request_document(req);
            let refusal = judged_resource(Op::NAME, &document, &region)
                .and_then(|resource| authorize(&*self.engine, ctx, Op::NAME, &resource).err());
            if let Some(denial) = refusal {
                // The protocol is read, used and dropped inside this one statement:
                // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
                // await below would make this future non-`Send`.
                return refusal_response(svc.definition().protocol(), &denial);
            }
            match svc.definition().routing_table().get(Op::NAME) {
                // The modelled walk — deserialize, validate, handle, serialize — runs
                // behind the delegate exactly as it did before, so nothing of how this
                // service refuses a bad request changes, and an operation that is still a
                // scaffold still answers its 501 to a caller IAM permits.
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under a key the
                // generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapper is not an operation of its own: the request is signed as the MODEL
        // says for the operation it names, so authentication is decided by the modelled
        // operation this wrapper stands in front of.
        None
    }
}

/// The request's body as a JSON document — this service's protocol is `awsJson1_0`, so an
/// input member is a member of that document. A body that is absent or is not JSON carries
/// no member, and is judged as a request that names no resource of its own.
fn request_document(req: &RtRequest) -> ::serde_json::Value {
    req.body()
        .bytes()
        .and_then(|body| ::serde_json::from_slice(body).ok())
        .unwrap_or(::serde_json::Value::Null)
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the
    /// IAM role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// own accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's
            // own catalog this emulator does not carry: a grant it cannot read.
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
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every action it asks for falls to the default deny. The
        // key id it presented is NOT quoted back — this module echoes no value of the
        // request — and in the assembled emulator the layer that authenticates a request
        // refuses an expired credential before any handler runs.
        Err(expired) => {
            let _ = expired;
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                principals: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the
    // user's policies rather than served as the account. A key the IAM store does not hold
    // either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the session
        // ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            let role_arn = held.arn.clone();
            caller.principals.push(role_arn.clone());
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

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-timestreaminfluxdb-amazontimestreaminfluxdb.json"
    );

    /// The Region every row below is read through, so an ARN built from it carries it.
    const REGION: &str = "us-west-2";

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE applied to the whole surface: the judged set is EXACTLY the set of operations
    /// whose model declares `AccessDeniedException`.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// that gains or loses the 403 fails here instead of silently serving an unauthorized
    /// caller (or answering an error its own model does not declare).
    #[test]
    fn the_judged_set_is_exactly_the_operations_modelling_the_refusal() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: ::serde_json::Value =
            ::serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let mut modelled: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter(|(_, shape)| {
                shape["errors"].as_array().is_some_and(|errors| {
                    errors.iter().any(|error| {
                        error["target"].as_str().is_some_and(|target| {
                            target.ends_with(&format!("#{ACCESS_DENIED_EXCEPTION}"))
                        })
                    })
                })
            })
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        modelled.sort_unstable();
        let mut judged = judged_operations();
        judged.sort_unstable();
        assert_eq!(
            judged, modelled,
            "case \"the whole surface\": expected the judged set to be the operations \
             modelling {ACCESS_DENIED_EXCEPTION}, got {judged:?} against {modelled:?}"
        );
    }

    /// RULE: the action this gate asks about is `timestream-influxdb:<OperationName>`, so
    /// a policy scoped to this service's actions covers every judged operation and nothing
    /// outside this service.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let prefix = st::ARN_NAMESPACE;
        let document = format!(
            r#"{{"Statement":[{{"Effect":"Allow","Action":"{prefix}:*","Resource":"*"}}]}}"#
        );
        for operation in judged_operations() {
            let action = format!("{prefix}:{operation}");
            let reached = decision(&document, &action, ANY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected {prefix}:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(&document, "iam:CreateUser", ANY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected {prefix}:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: the resource a call is judged against is the one the REQUEST names — the ARN
    /// of the record whose id it carries, and every resource of the addressed family in
    /// the Region for a request that names no existing record (a create, a listing, or one
    /// whose identifier member is absent).
    #[test]
    fn the_request_decides_the_resource_the_action_is_judged_against() {
        // (the case, the operation, the request body, the resource path of the ARN)
        let rows: &[(&str, &str, ::serde_json::Value, String)] = &[
            (
                "a create, whose id is minted after the decision",
                "CreateDbParameterGroup",
                ::serde_json::json!({ "name": "group-one" }),
                format!("{}/{ANY_RESOURCE}", st::ARN_SEGMENT_DB_PARAMETER_GROUP),
            ),
            (
                "a read of one parameter group",
                "GetDbParameterGroup",
                ::serde_json::json!({ "identifier": "abc123" }),
                format!("{}/abc123", st::ARN_SEGMENT_DB_PARAMETER_GROUP),
            ),
            (
                "a read of one DB instance",
                "GetDbInstance",
                ::serde_json::json!({ "identifier": "inst456" }),
                format!("{}/inst456", st::ARN_SEGMENT_DB_INSTANCE),
            ),
            (
                "an update of one DB cluster",
                "UpdateDbCluster",
                ::serde_json::json!({ "dbClusterId": "clus789", "port": 8086 }),
                format!("{}/clus789", st::ARN_SEGMENT_DB_CLUSTER),
            ),
            (
                "a restore, judged against the backup it names",
                "RestoreFromDbBackup",
                ::serde_json::json!({ "name": "restored", "dbBackupId": "back321" }),
                format!("{}/back321", st::ARN_SEGMENT_DB_BACKUP),
            ),
            (
                "a listing of a whole family",
                "ListDbInstances",
                ::serde_json::json!({}),
                format!("{}/{ANY_RESOURCE}", st::ARN_SEGMENT_DB_INSTANCE),
            ),
            (
                "a read whose identifier member is absent",
                "GetDbBackup",
                ::serde_json::json!({}),
                format!("{}/{ANY_RESOURCE}", st::ARN_SEGMENT_DB_BACKUP),
            ),
        ];
        for (case, operation, document, resource) in rows {
            let judged = judged_resource(operation, document, REGION);
            let expected = Some(::engine::emulator_arn(
                st::ARN_NAMESPACE,
                REGION,
                resource,
            ));
            assert_eq!(
                judged, expected,
                "case {case:?}: expected {expected:?}, got {judged:?}"
            );
        }
        // An operation this gate does not judge has no resource to answer: the three
        // tagging operations, whose model declares no `AccessDeniedException`.
        for operation in ["TagResource", "UntagResource", "ListTagsForResource"] {
            let judged = judged_resource(
                operation,
                &::serde_json::json!({ "resourceArn": "arn:aws:timestream-influxdb:::x" }),
                REGION,
            );
            assert_eq!(
                judged, None,
                "case {operation:?}: an operation modelling no 403 is not judged, got \
                 {judged:?}"
            );
        }
    }
}
