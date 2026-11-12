// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this signaling data plane perform the action it asks for.
//!
//! Both operations of this service model `NotAuthorizedException` — "the caller is not
//! authorized to perform this operation" — so a request is authorized BEFORE it reads or
//! writes anything: on the action `kinesisvideo:<OperationName>` (this service's
//! `arnNamespace`, `smithy_query(query="service_info")`) and on the channel ARN it
//! addresses. A request no statement allows — IAM's default deny — and one an explicit
//! `Deny` covers are both refused, and nothing is performed.
//!
//! The decision has two halves, and neither is invented here:
//!
//! * WHO the caller is comes from the same source `sts:GetCallerIdentity` answers from —
//!   the STS session registry, keyed by the access key id the request signed with
//!   ([`caller`]) — so the principal a refusal blames is the one the caller reads back from
//!   STS for its own credentials.
//! * WHAT that caller may do comes from the identity-based policy documents the IAM store
//!   holds for it (a role's inline policies and the default version of each managed policy
//!   attached to it; a user's inline policies), evaluated by the ONE shared reading of an
//!   IAM document ([`::service_common::iam_authorization`]). A signaling channel has no
//!   resource-based policy — neither model gives this service a `PutResourcePolicy` — so
//!   the identity-based decision is the whole decision.
//!
//! THE SAME RULE, AND THE SAME READING OF IT, as
//! `services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/src/state.rs`
//! (`authorize`) and `services/service-com-amazonaws-rum-rum/src/state/authorization.rs`,
//! which are the in-tree precedents: the three agree on the account's own credential, on an
//! attachment this emulator cannot read, and on an expired credential. The GATHERING is not
//! shared in `services/common` because it reads the IAM and STS service stores, and that
//! crate is a dependency of nearly every service crate; only the DECISION, which needs no
//! store, lives there.
//!
//! Three callers are not principals the IAM store holds policies for, and each is answered
//! the way those precedents answer it:
//!
//! * the account's own published credential (the long-term key this emulator hands out),
//!   which is the position AWS account root credentials are in: allowed;
//! * a principal with an AWS-MANAGED policy attached, whose document the IAM store
//!   deliberately does not hold: allowed, because refusing on a grant this emulator cannot
//!   read would deny access AWS permits. It lifts the DEFAULT deny only — an explicit
//!   `Deny` in a document that CAN be read still refuses the call;
//! * an expired session credential, which speaks for nobody: refused.
//!
//! What is NOT modelled here: a statement's `Condition` clauses. The decision is taken with
//! an empty request context, so a statement conditioned on a key this emulator does not
//! supply is read as the shared evaluator reads an undecided clause. Nothing here guesses a
//! condition value, and a reader should not infer that condition keys are enforced.

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_state;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;

/// The IAM service prefix this service's actions carry: `kinesisvideo:GetIceServerConfig`,
/// `kinesisvideo:SendAlexaOfferToMaster`. It is the ARN namespace
/// (`smithy_query(query="service_info")` -> `awsService.arnNamespace`), which this service
/// shares with the Kinesis Video control plane.
pub const ACTION_PREFIX: &str = "kinesisvideo";

/// Whether IAM permits `operation` on `resource` for the caller of this request, and the
/// sentence that says why not when it does not.
///
/// `operation` is the operation's name (`Self::NAME` in a handler), which names the IAM
/// action `kinesisvideo:<Operation>`; `resource` is the ARN the request acts on (the
/// channel's). `None` when the caller may make the call, and the refusal's message when it
/// may not — which each handler answers in its own modelled `NotAuthorizedException`.
///
/// Read BEFORE this service's own store is locked, so the IAM and STS stores are never
/// reached while this service's guard is held.
pub fn authorization_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Option<String> {
    use ::service_common::iam_authorization as iam;

    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    // A signaling channel carries no resource-based policy, so the identity-based decision
    // is the whole one and `NotStated` is IAM's implicit deny.
    let request = iam::Request::new(&action, resource, &[]);
    let identity = iam::identity_decision(&caller.documents, &request);
    // An explicit `Deny` in a document that CAN be read is taken FIRST: it refuses the
    // call whatever else the principal carries — including a grant this emulator cannot
    // read, which only lifts the default deny below.
    if identity != iam::Decision::Denied
        && (iam::is_allowed(identity, iam::Decision::NotStated) || caller.unbounded)
    {
        return None;
    }
    // The two sentences AWS refuses an unauthorized call with, as it words them: a
    // document that explicitly denied the call is NAMED, and IAM's default deny — no
    // document refused it and none allowed it — states which action no policy allows.
    let reason = if identity == iam::Decision::Denied {
        "with an explicit deny in an identity-based policy".to_string()
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    // Built from the request: the principal that signed it, the action it performs and the
    // resource it names. AWS words this refusal the same way, and a client reads the
    // principal and the action out of it to fix its policy.
    Some(format!(
        "User: {} is not authorized to perform: {action} on resource: {resource} {reason}",
        caller.arn
    ))
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// Set when the identity side cannot refuse this caller: the account's own published
    /// credential, or a principal carrying a grant this emulator cannot read (see this
    /// module's header).
    pub unbounded: bool,
}

/// The principal THIS request speaks for, and the identity-based policies in force on it.
///
/// Read from the same source `sts:GetCallerIdentity` answers from — the STS session the
/// request's access key id names — so the principal an authorization decision is made
/// about, and the one a refusal blames, is the one the caller reads back from STS for its
/// own credentials. `resolve_caller` materialises the session expiries the clock has
/// already made due before it answers, a write owed at the same instant whether or not
/// anybody called, which is why a REPORTING operation may make this decision.
pub fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_state::service_state(engine) {
        Some(sts) => sts_state::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(sts_state::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents and
        // nothing unbounded, which refuses every action it asks for.
        Err(expired) => {
            return Caller {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                documents: Vec::new(),
                unbounded: false,
            }
        }
    };

    let mut caller = Caller {
        unbounded: identity.arn == sts_state::ambient_identity().arn,
        arn: identity.arn,
        documents: Vec::new(),
    };
    let Some(iam) = iam_state::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        let role_key = iam_state::name_key(role_name);
        caller
            .documents
            .extend(inline_documents(&store.role_inline_policies, &role_key));
        for attached in store
            .role_attached_policies
            .get(&role_key)
            .into_iter()
            .flatten()
        {
            match store
                .policy_by_arn(attached)
                .and_then(default_version_document)
            {
                Some(document) => caller.documents.push(document),
                // Only an AWS-MANAGED policy is a grant this emulator cannot read; an
                // attachment naming a customer-managed policy the account does not hold
                // grants nothing, because the account IS the authority on its own.
                None if is_aws_managed_policy(attached) => caller.unbounded = true,
                None => {}
            }
        }
    } else if let Some(user_name) = iam_user_name(&caller.arn) {
        let user_key = iam_state::name_key(user_name);
        caller
            .documents
            .extend(inline_documents(&store.user_inline_policies, &user_key));
        for attached in store
            .user_attached_policies
            .get(&user_key)
            .into_iter()
            .flatten()
        {
            match store
                .policy_by_arn(attached)
                .and_then(default_version_document)
            {
                Some(document) => caller.documents.push(document),
                None if is_aws_managed_policy(attached) => caller.unbounded = true,
                None => {}
            }
        }
    }
    caller
}

/// Whether `arn` names an AWS-MANAGED policy: one whose account segment is IAM's `aws`, so
/// it belongs to AWS rather than to this account and the IAM store holds no document for
/// it. Read through IAM's own ARN parser, so this service and IAM agree on what an
/// AWS-managed policy ARN is.
fn is_aws_managed_policy(arn: &str) -> bool {
    iam_state::parse_policy_arn(arn)
        .is_some_and(|policy| policy.account == iam_state::AWS_MANAGED_POLICY_ACCOUNT)
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role is
/// the first segment after `assumed-role/`. `None` for any other ARN form.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "sts")?;
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// The user name an IAM user ARN names: the LAST segment of `user/<path…>/<name>`, since
/// the path is optional and only the last segment is the name.
fn iam_user_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "iam")?;
    let name = resource.strip_prefix("user/")?.split('/').next_back()?;
    (!name.is_empty()).then_some(name)
}

/// The resource part of an ARN whose service segment is `service`.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, and the resource
/// may itself carry colons, so only the first five fields are split off.
fn arn_resource<'a>(arn: &'a str, service: &str) -> Option<&'a str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(named), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    (named == service).then_some(resource)
}

/// The inline policy documents one entity holds, from the IAM store's
/// `<entity> -> <policy name> -> policy` map.
fn inline_documents(
    policies: &BTreeMap<String, BTreeMap<String, iam_state::InlinePolicy>>,
    entity_key: &str,
) -> Vec<String> {
    policies
        .get(entity_key)
        .map(|held| {
            held.values()
                .map(|policy| policy.policy_document.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The document of a managed policy's DEFAULT version — the one in force on every entity it
/// is attached to. `None` when the store holds no document for it.
fn default_version_document(managed: &iam_state::ManagedPolicy) -> Option<String> {
    let default = managed.policy.default_version_id.as_deref();
    managed
        .versions
        .values()
        .find(|version| default.is_some() && version.version_id.as_deref() == default)
        .or_else(|| {
            managed
                .versions
                .values()
                .find(|version| version.is_default_version)
        })
        .and_then(|version| version.document.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_principal_arn_names_the_entity_its_policies_are_read_from() {
        // The rows span the ARN forms a caller's identity carries: an assumed-role session,
        // a user at the account root, a user under a path, and ARNs of neither kind.
        let cases: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/viewer-role/session-1",
                Some("viewer-role"),
                None,
            ),
            (
                "a user at the account root",
                "arn:aws:iam::000000000000:user/emulator",
                None,
                Some("emulator"),
            ),
            (
                "a user under a path",
                "arn:aws:iam::000000000000:user/team/sub/reader",
                None,
                Some("reader"),
            ),
            (
                "a federated user, which is neither",
                "arn:aws:sts::000000000000:federated-user/someone",
                None,
                None,
            ),
            ("not an ARN at all", "nobody", None, None),
        ];
        for (case, arn, role, user) in cases {
            assert_eq!(
                assumed_role_name(arn),
                *role,
                "case {case:?}: expected role {role:?} in {arn:?}"
            );
            assert_eq!(
                iam_user_name(arn),
                *user,
                "case {case:?}: expected user {user:?} in {arn:?}"
            );
        }
    }
}
