// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the decision
//! taken BEFORE the operation it names looks up anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service resolving what the
//! request named, so a caller whose identity-based policies do not grant the action — or
//! that an explicit `Deny` covers — is told it is not authorized rather than told anything
//! about the contact it asked about. This service models that refusal as
//! `AccessDeniedException` ("You do not have sufficient access to perform this action.",
//! `@error("client")` at `@httpError(403)`), which the model binds to every operation
//! (`smithy_query(query="service_info")` lists it under `common_errors`).
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS minted
//!    that key for (`sts_store::resolve_caller`, the same record `sts:GetCallerIdentity`
//!    answers from, so the principal a refusal blames is the one the caller reads back for
//!    the credentials it signed with), else the IAM user that owns the long-term key, else
//!    the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), so a document
//!    `iam:PutUserPolicy` wrote takes effect on the next request and deleting it lifts it
//!    again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` wins, otherwise an `Allow`
//!    about the request grants it, and with neither the request falls to IAM's default deny.
//!    The refusal's WORDING comes from that module too, for the same reason the decision
//!    does.
//!
//! Only the store LOOKUPS live here; neither the decision nor its sentence does. The lookups
//! stay in this crate because hoisting them into `service-common` would make every service
//! that depends on that crate link the IAM and STS service crates. WHICH IAM entity a
//! principal ARN names carries no such dependency, so it is read through the shared
//! [`::service_common::iam_role`] rather than parsed again here.
//!
//! ## The action and the resource
//!
//! The action is `connect:<OperationName>`: Contact Lens publishes its actions under Amazon
//! Connect's namespace, which is this service's own `arnNamespace` and the name every
//! request to it is signed under (`smithy_query(query="service_info")`) — so the prefix is
//! read off the request's resolved identity rather than restated here.
//!
//! The resource is the CONTACT the request addresses, under the instance it names:
//! `arn:aws:connect:<region>:<account>:instance/<InstanceId>/contact/<ContactId>` — the ARN
//! shape Amazon Connect gives a contact. Both halves come from the REQUEST rather than from
//! a stored session, so the decision does not depend on whether that session exists, which
//! is also what keeps a refused call from telling an unauthorized caller whether it does.
//!
//! This service publishes no resource-based policy of its own — the model declares no policy
//! operation and no policy member — so the decision here is the identity-based one alone.
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
//! An expired session credential speaks for nobody: it carries no documents and no principal
//! spelling, so every action it asks for is refused. (A session the authenticating layer has
//! already refused never reaches a handler at all.)
//!
//! LOCK ORDER: the STS registry's lock and then the IAM store's, each taken and released in
//! turn BEFORE this service's own store lock is taken — the decision is reached ahead of
//! dispatch, so no handler of this service holds its store while this runs. Neither of those
//! services reads this one's state.

use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::{OperationContext, ServerHttpProtocol};

use crate::ops::types::AccessDeniedException;

/// The wire name of the refusal this gate answers with, and the status the model gives it:
/// `AccessDeniedException`, `@error("client")` at `@httpError(403)`
/// (`smithy_query(query="shape", shape="AccessDeniedException")`).
pub(super) const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The media type this restJson1 service renders an error in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The region the judged resource's ARN is built in: the one the caller signed for, else the
/// one Amazon Connect's own emulated service builds its ARNs in when a request names none.
fn request_region(ctx: &OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region an ARN is built in for a request whose credential scope names none.
const DEFAULT_REGION: &str = "us-east-1";

/// The ARN of the contact the request addresses, under the instance it names (see the module
/// docs). Both halves are taken from the request's own body, as
/// [`super::front_door::body_members`] read it; a member the request does not carry is empty
/// in the ARN, which names no contact any statement can match.
#[must_use]
pub(super) fn judged_resource(
    ctx: &OperationContext<'_>,
    members: Option<&::serde_json::Map<String, ::serde_json::Value>>,
) -> String {
    let instance_id = super::front_door::named(members, "InstanceId").unwrap_or_default();
    let contact_id = super::front_door::named(members, "ContactId").unwrap_or_default();
    ::engine::emulator_arn(
        action_prefix(ctx),
        &request_region(ctx),
        &format!("instance/{instance_id}/contact/{contact_id}"),
    )
}

/// This service's IAM action prefix, which is its `arnNamespace` and the name every request
/// to it is signed under (`smithy_query(query="service_info")`), so the ARNs this gate builds
/// and the actions it asks about cannot name two different services.
fn action_prefix<'ctx>(ctx: &'ctx OperationContext<'_>) -> &'ctx str {
    ctx.identity().signing_name()
}

/// Authorize this request against `resource`, where `operation` is the operation's own name
/// ([`::smithy_server_runtime::Operation::NAME`]).
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see the
/// module docs); the modelled refusal otherwise.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which materialises the
/// session expiries the clock has already made due; that write is owed at the same instant
/// whether or not anybody called, which is why a REPORTING operation of this service may take
/// this decision too.
pub(super) fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{}:{operation}", action_prefix(ctx));
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no such
    // element, and this service publishes no resource-based policy, so the decision below
    // rests on the action and the resource.
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
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not a
    // permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the identity-based
            // policies gave.
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(&action),
        };
        return Err(not_authorized(&caller.arn, &action, resource, &reason));
    }
    Ok(())
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
        message: iam::not_authorized_by_identity_policy(principal, action, resource, reason),
    }
}

/// The wire answer to a request this gate refused: the modelled `AccessDeniedException` at its
/// modelled 403, serialized through the SAME protocol a handler-returned error of that shape
/// goes through, so a refusal taken in this layer and one taken in a handler are
/// indistinguishable on the wire.
///
/// The hand-built fallback carries the same three things a client branches on — the status,
/// the shape in `x-amzn-errortype` and the `__type` body — for the one case the serializer can
/// fail: a value it cannot render.
#[must_use]
pub(super) fn refusal_response(
    protocol: &dyn ServerHttpProtocol,
    denial: &AccessDeniedException,
) -> RtResponse {
    protocol
        .serialize_error(
            denial,
            &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_CODE,
            ACCESS_DENIED_STATUS,
        )
        .unwrap_or_else(|_| {
            let body = ::serde_json::json!({
                "__type": ACCESS_DENIED_CODE,
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
                .insert("x-amzn-errortype", ACCESS_DENIED_CODE);
            response
        })
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
        // principal spelling, and every action it asks for falls to the default deny. The key
        // id it presented is NOT quoted back — this module echoes no value of the request —
        // and in the assembled emulator the layer that authenticates a request refuses an
        // expired credential before any handler runs.
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
    // minted is the account's ambient identity — but IAM's store maps an access key id to the
    // user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the user's
    // policies rather than served as the account. A key the IAM store does not hold either
    // really is the account's own published credential.
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
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
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

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is published under Amazon Connect's namespace, so
    /// a statement scoped to that service's actions covers this operation and nothing outside
    /// it.
    #[test]
    fn the_action_is_published_under_amazon_connects_namespace() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"connect:*","Resource":"*"}]}"#;
        let action = "connect:ListRealtimeContactAnalysisSegments";
        let reached = decision(document, action, "*");
        assert_eq!(
            reached,
            Decision::Allowed,
            "case \"this service's own action\": expected the service-wide allow to cover \
             {action}, got {reached:?}"
        );
        let outside = decision(document, "connectcases:GetCase", "*");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected the service-wide allow to grant nothing outside \
             Amazon Connect, got {outside:?}"
        );
    }

    /// RULE: the refusal names the principal, the action, the resource the request addressed
    /// and which of IAM's rules refused it — every part taken from the request and the stored
    /// policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = format!(
            "arn:aws:sts::{}:assumed-role/contact-lens-denied/session",
            ::engine::emulator_account_id()
        );
        let action = "connect:ListRealtimeContactAnalysisSegments";
        let resource = ::engine::emulator_arn(
            "connect",
            "us-east-1",
            "instance/4cd1aa0d-1f2e-4b3c-8d4e-5f60718293a4/contact/\
             b5f87632-1f2e-4b3c-8d4e-5f60718293a4",
        );
        for reason in [
            iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string(),
            iam::no_identity_policy_allows(action),
            iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            iam::no_permissions_boundary_allows(action),
        ] {
            let refusal = not_authorized(&principal, action, &resource, &reason);
            assert_eq!(
                refusal.message,
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {resource} {reason}"
                ),
                "case {reason:?}: expected IAM's wording, got {:?}",
                refusal.message
            );
        }
    }
}
