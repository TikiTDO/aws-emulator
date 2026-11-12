// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER's own identity-based policies let a request to this service
//! perform the action it asks for — the decision an operation takes before it
//! resolves anything the request names.
//!
//! AWS authorizes at the IAM layer, ahead of the service evaluating the resource, so
//! a caller whose identity policies deny the action is told it is not authorized
//! rather than told whether the resource exists. This service's model declares no
//! shape for that refusal, so it leaves through the same channel the
//! resource-scoped decision's does ([`crate::state::authorization`]), carrying the
//! code and status the real service answers.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the
//!    SAME place `sts:GetCallerIdentity` answers from (`sts_store::resolve_caller`),
//!    so the principal a refusal blames is the one the caller reads back from STS
//!    for the credentials it signed with;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds
//!    for that principal, read through IAM's OWN accessor (`principal_policies`,
//!    where the collection rule lives), so a document `iam:PutRolePolicy` wrote
//!    takes effect on the next request and deleting it lifts the refusal again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared
//!    evaluation of those documents against `(action, resource)`, so this service
//!    and every other one handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; neither the decision nor the ARN readings do.
//! That is the split every other gate in this tree makes, and the lookups stay in
//! this crate for the reason those keep theirs here: the IAM service crate depends
//! on `service-common`, so the shared crate cannot read IAM's store without a
//! dependency cycle. WHICH IAM ENTITY a principal ARN names carries no such
//! dependency, so it is read through the shared [`::service_common::iam_role`]
//! rather than parsed again here.
//!
//! ## What is refused, and what is not
//!
//! An EXPLICIT `Deny` about the action is the decision this gate takes, and it is
//! final whatever else the account holds. An ABSENT `Allow` is NOT evidence that the
//! caller lacks the permission: the rest of a principal's permission set (its
//! groups' policies, service control policies, a session policy the mint passed)
//! does not all reach this store, so refusing on its absence would deny calls AWS
//! permits. A request signed with a credential neither store knows — the emulator's
//! own published credential, the position AWS account root credentials are in —
//! carries no identity policy and so is never refused here, and neither is a
//! request served by an engine carrying no IAM or STS store.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The principal the REQUEST's credential speaks for, as `sts:GetCallerIdentity`
/// reports it — the identity an authorization refusal blames.
///
/// Resolved from the same two stores the decision below reads, so a refusal names
/// the session or the user that signed the call rather than the account's own
/// published principal; a credential neither store knows is that published
/// principal, which is the identity such a request speaks for.
pub fn calling_principal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    let caller = caller(engine, ctx);
    if caller.arn.is_empty() {
        return crate::state::authorization::caller_principal();
    }
    caller.arn
}

/// The refusal an operation returns for a request the calling principal's identity
/// policies explicitly deny, and `None` for every request no readable document
/// denies: the one call the shared authorization gate
/// ([`crate::state::authorization::authorize_action`]) makes, so every operation
/// that authorizes a target takes this decision first.
///
/// `action` is the fully qualified IAM action the call asks for — `kafka:<ApiName>`
/// for a control-plane call ([`crate::state::authorization::control_plane_action`]),
/// the data-plane action for a call that reaches a cluster's Apache Kafka plane —
/// and `resource` is the ARN the request NAMES, passed as submitted, before
/// anything is resolved, so the decision does not depend on whether that resource
/// exists and a refused call tells an unauthorized caller nothing about whether it
/// does.
///
/// Generic over the operation's own modelled error enum because the refusal is none
/// of them: it leaves through the error channel's unmodelled variant, with the code
/// and status [`crate::state::authorization`] names.
pub fn denied<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    explicit_deny(engine, ctx, action, resource).map(crate::state::authorization::access_denied)
}

/// `Some(message)` when the calling principal's identity policies explicitly deny
/// `action` on `resource` — the sentence the refusal carries — and `None` when no
/// readable document denies it.
fn explicit_deny<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Option<String> {
    let caller = caller(engine, ctx);
    if caller.documents.is_empty() {
        return None;
    }
    // An identity-based document states no `Principal` element, so the decision
    // rests on the action and the resource the request addressed.
    let request = iam::Request::new(action, resource, &[]);
    (iam::identity_decision(&caller.documents, &request) == Decision::Denied)
        .then(|| not_authorized_by_explicit_deny(&caller.arn, action, resource))
}

/// The sentence IAM states an explicit-deny decision with: the principal, the action
/// and the resource — each taken from the request and the credential it presented —
/// closed by the clause AWS names an identity-based document with, so a client
/// reading it learns that fixing the resource's own policy will not help.
fn not_authorized_by_explicit_deny(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "{} with an explicit deny in an identity-based policy",
        crate::state::authorization::not_authorized_as(principal, action, resource)
    )
}

/// The principal a request speaks for, and the identity-based documents in force on
/// it.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal
    /// names.
    arn: String,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
}

/// The principal this request speaks for, with the identity-based policies in force
/// on it.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which materialises
/// the session expiries the clock has already made due before it answers: that write
/// is owed at the same instant whether or not anybody called, which is why a
/// REPORTING operation of this service may take this decision too.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is
        // the account's own published credential, which holds no identity policy.
        None => Ok(sts_store::ambient_identity()),
    };
    // An expired session credential speaks for nobody: it carries no documents, so
    // nothing here refuses it (the layer that authenticates a request is what
    // rejects it).
    let Ok(identity) = identity else {
        return Caller::default();
    };
    let mut caller = Caller {
        arn: identity.arn,
        documents: Vec::new(),
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A long-term access key names no session, so STS answers the account's own
    // identity for it — but IAM's store maps an access key id to the USER that owns
    // it, and a request signed with one speaks for that user and is judged on the
    // user's policies.
    if let Some(user) = store
        .access_keys
        .get(&access_key_id)
        .and_then(|record| store.users.get(&record.user_key))
    {
        caller.arn = user.arn.clone();
    }
    // The identity whose policies are in force: for an assumed-role session the
    // ROLE's, addressed by the ARN the account holds the role under (which carries
    // its path, where the session ARN does not); for an IAM user its own.
    let identity_arn = match ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        Some(role_name) => store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone()),
        None => ::service_common::iam_role::user_name(&caller.arn).map(|_| caller.arn.clone()),
    };
    if let Some(identity_arn) = identity_arn {
        if let Some(policies) = store.principal_policies(&identity_arn) {
            caller.documents = policies.documents;
        }
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision one stored document reaches about an action on a resource,
    /// through the shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `kafka:<ApiName>`, so a document
    /// scoped to this service's actions decides every operation of it and nothing
    /// outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Deny","Action":"kafka:*","Resource":"*"}]}"#;
        for operation in ["RebootBroker", "DescribeCluster", "UpdateBrokerCount"] {
            let action = crate::state::authorization::control_plane_action(operation);
            let reached = decision(document, &action, "*");
            assert_eq!(
                reached,
                Decision::Denied,
                "case {operation:?}: expected kafka:* to deny {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "ec2:DescribeSubnets", "*");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected kafka:* to state nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: the refusal names the principal the credential speaks for, the action
    /// asked for, the resource the request addressed, and that an identity-based
    /// document explicitly denied it.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = format!(
            "arn:aws:sts::{}:assumed-role/kafka-denied/session-1",
            ::engine::emulator_account_id()
        );
        let cluster = ::engine::emulator_arn(
            "kafka",
            "us-west-2",
            "cluster/orders/11111111-2222-3333-4444-555555555555-1",
        );
        let action = crate::state::authorization::control_plane_action("RebootBroker");
        let message = not_authorized_by_explicit_deny(&principal, &action, &cluster);
        assert_eq!(
            message,
            format!(
                "User: {principal} is not authorized to perform: {action} on resource: \
                 {cluster} with an explicit deny in an identity-based policy"
            ),
            "case \"an explicit deny\": expected IAM's wording, got {message:?}"
        );
    }
}
