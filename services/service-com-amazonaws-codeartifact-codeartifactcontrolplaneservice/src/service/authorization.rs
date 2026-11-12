// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the caller perform a CodeArtifact action — the decision every
//! control-plane operation that addresses a DOMAIN, a REPOSITORY or a PACKAGE takes before it
//! answers.
//!
//! A CodeArtifact API call is authorized, not merely routed: a principal may perform
//! `codeartifact:<OperationName>` on the resource the call addressed only when some policy in
//! force allows it — the CALLER'S OWN identity-based policies, or one of the resource policies
//! in force on that resource naming the caller as `Principal` — and an explicit `Deny`
//! in any of them refuses whatever the others say. A principal allowed by NONE of them is
//! refused with the operation's modelled `AccessDeniedException` (HTTP 403,
//! `smithy_query(query="operation", operation="GetRepositoryEndpoint")`), stated in IAM's own
//! words. That is one rule for the whole service, so it lives here once: two operations
//! handed the same documents must not answer differently.
//!
//! ## Which resource the action is judged against
//!
//! An action acts on ONE resource type, as the IAM Service Authorization Reference lists it
//! for CodeArtifact, and the ARN of that resource is what the decision is reached against —
//! so a statement naming one package decides that package alone, and a refusal names the
//! resource the caller was actually refused on. Hence three entry points: the domain
//! ([`authorize_domain_action`]), the repository ([`authorize_repository_action`]) and the
//! package inside it ([`authorize_package_action`]).
//!
//! The PACKAGE-GROUP operations that address a group by PATTERN are the one class this does
//! not yet resolve to its own resource type: they are judged against the domain they address,
//! because the pattern a request names has to be resolved against the stored hierarchy before
//! the group it selects — and therefore that group's ARN — is known, which is work for the
//! issue that owns those operations. The documents in force are the same either way (the
//! domain's policy is the only one a package group lives under), so what is deferred is which
//! ARN the decision and the refusal name, not whether the call is authorized.
//!
//! An operation whose subject is named by an ARN rather than by name members
//! ([`authorize_arn_addressed_action`]) needs no such resolution: the ARN already names one
//! resource of one kind, a package group included, so the decision is reached against that
//! resource's own ARN.
//!
//! ## Which resource policies are in force
//!
//! A repository lives INSIDE a domain, and a domain's own permissions policy governs the
//! domain and the repositories it contains, so the documents a decision is made from are:
//!
//! * a DOMAIN-scoped call — that domain's policy;
//! * a call on a repository, or on a package inside it — that repository's own policy AND the
//!   policy of the domain holding it.
//!
//! Several documents, one rule: a `Deny` in ANY of them refuses, and an `Allow` in any of
//! them (or in an identity-based policy) grants. That is what makes a domain-policy `Deny`
//! override a repository-policy `Allow` for the same action.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS minted
//!    that key for ([`caller_principal`], the same place `sts:GetCallerIdentity` answers
//!    from, so the principal a refusal blames is the one the caller reads back for the
//!    credentials it signed with);
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal, read through the IAM store's own records, so a policy `iam:PutRolePolicy`
//!    wrote takes effect on the next request and deleting it lifts it again; and the resource
//!    policies in force on what the call addressed ([`resource_policies`]), read out of THIS
//!    service's store, so a document `PutDomainPermissionsPolicy` or
//!    `PutRepositoryPermissionsPolicy` wrote is an authorization input rather than an inert
//!    record;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource, principal)`, so this service and every
//!    other one handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split every
//! other gate in this tree makes (AWS Key Management Service, AWS Certificate Manager,
//! Amazon OpenSearch Service), and the lookups stay in the service crate for the reason
//! those keep theirs there: hoisting them into `service-common` would make every service
//! that depends on that crate link the IAM and STS service crates.
//!
//! ## What a statement has to name to grant
//!
//! An assumed-role session's permissions are the ROLE's, so a resource policy naming the
//! role grants its sessions: the spellings a statement may name the caller ITSELF by are the
//! session ARN STS reports and the ARN of the role behind it ([`Caller::principals`]).
//! Naming the whole ACCOUNT is not naming the principal — it delegates to that account's own
//! identity policies — so those spellings decide a DENY only, which is the split the shared
//! evaluator publishes ([`iam::account_principals`]).
//!
//! ## The callers whose GRANTS this gate cannot read
//!
//! For some callers the identity-based half of the decision cannot be collected at all:
//!
//! * **The account's own published credential**, and any long-term key: STS holds no session
//!   for it, so there is no role whose identity policies could be read, which is the position
//!   AWS account root credentials are in.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! Such a caller is never refused for the ABSENCE of a grant ([`Caller::unreadable_grants`]).
//! An explicit `Deny` in a resource policy that CAN be read still refuses it, whichever
//! principal form it presents: a deny is decided from documents this service itself holds, so
//! there is no missing evidence to withhold judgement over — which is the same reading the
//! repository endpoint's own plane takes of a deny
//! ([`crate::service::npm_registry`]).

use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;
use crate::state::ResourceRef;

/// This service's IAM action prefix: an action is `codeartifact:<OperationName>` ("Actions
/// defined by AWS CodeArtifact", IAM Service Authorization Reference), which is this
/// service's `arnNamespace` (`smithy_query(query="service_info")`) — the same string
/// [`crate::state::SERVICE_SIGNING_NAME`] spells into every ARN this service builds.
pub const ACTION_PREFIX: &str = crate::state::SERVICE_SIGNING_NAME;

/// Authorize `operation` on the repository the request addressed.
///
/// `Ok(())` for a request IAM permits (and for a caller whose grants this gate cannot read,
/// where no readable document denies it, see the module docs); the modelled refusal
/// otherwise. Call it at the TOP of a handler, before this
/// service's store is locked and before the addressed resources are resolved, and with the
/// names the REQUEST carries rather than ones read back from a stored record: an
/// authorization decision does not depend on whether the repository exists, which is also
/// what keeps a refused call from telling an unauthorized caller whether it does.
pub fn authorize_repository_action<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    domain: &str,
    domain_owner: Option<&str>,
    repository: &str,
) -> Result<(), AccessDeniedException> {
    let caller = caller_or_unbounded(engine, ctx);
    let region = crate::state::request_region(ctx.parts());
    let resource = crate::state::repository_arn(&region, domain, repository);
    let action = format!("{ACTION_PREFIX}:{operation}");
    let policies = resource_policies(engine, domain, domain_owner, Some(repository));
    decide(&caller, &action, &resource, &policies)
}

/// The PACKAGE a request addressed, as its coordinate members carry it: the repository
/// holding it and the `(format, namespace, name)` triple a package is addressed by.
///
/// Passed as one value because it IS one: the four members together name the resource the
/// decision is reached against, and a caller that split them could pass a format from one
/// request beside a name from another.
#[derive(Debug, Clone, Copy)]
pub struct AddressedPackage<'a> {
    pub repository: &'a str,
    pub format: &'a str,
    /// Absent for a format whose packages carry no namespace component.
    pub namespace: Option<&'a str>,
    pub package: &'a str,
}

/// Authorize `operation` on the PACKAGE the request addressed.
///
/// The same decision [`authorize_repository_action`] takes, for the operations whose subject
/// is one package inside a repository: the resource is that package's own ARN
/// ([`crate::state::package_arn`]), so a statement that names one package grants or denies
/// that package alone, while the documents in force are still the repository's policy and its
/// domain's.
pub fn authorize_package_action<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    domain: &str,
    domain_owner: Option<&str>,
    addressed: &AddressedPackage<'_>,
) -> Result<(), AccessDeniedException> {
    let caller = caller_or_unbounded(engine, ctx);
    let region = crate::state::request_region(ctx.parts());
    let resource = crate::state::package_arn(
        &region,
        domain,
        addressed.repository,
        addressed.format,
        addressed.namespace,
        addressed.package,
    );
    let action = format!("{ACTION_PREFIX}:{operation}");
    let policies = resource_policies(engine, domain, domain_owner, Some(addressed.repository));
    decide(&caller, &action, &resource, &policies)
}

/// Authorize `operation` on the DOMAIN the request addressed.
///
/// The same decision [`authorize_repository_action`] takes, for the operations whose subject
/// is the domain itself: the resource is the domain's ARN and the resource-based input is that
/// domain's own permissions policy. Call it at the TOP of a handler, with the names the
/// REQUEST carries, for the reasons stated there.
pub fn authorize_domain_action<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    domain: &str,
    domain_owner: Option<&str>,
) -> Result<(), AccessDeniedException> {
    let caller = caller_or_unbounded(engine, ctx);
    let region = crate::state::request_region(ctx.parts());
    let resource = crate::state::domain_arn(&region, domain);
    let action = format!("{ACTION_PREFIX}:{operation}");
    let policies = resource_policies(engine, domain, domain_owner, None);
    decide(&caller, &action, &resource, &policies)
}

/// Authorize `operation` on the resource a caller-supplied ARN addressed.
///
/// The same decision [`authorize_domain_action`] takes, for the operations whose subject is
/// named by an ARN rather than by name members: the resource is the one that ARN resolved to
/// ([`crate::state::ResourceRef`]) and the decision is reached against that resource's OWN
/// ARN — a domain's, a repository's or a package group's — so a statement naming one of them
/// decides that resource alone. The documents in force are the ones that resource lives
/// under: its domain's policy, and for a repository that repository's own.
///
/// The ARN is canonicalised through the resource it resolved to rather than taken verbatim,
/// because an ARN a caller writes out by hand may pin a region or an account the store does
/// not partition by ([`crate::state::taggable_resource`]); the decision is reached against
/// the ARN the resource itself reports.
pub fn authorize_arn_addressed_action<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    reference: &crate::state::ResourceRef,
) -> Result<(), AccessDeniedException> {
    let caller = caller_or_unbounded(engine, ctx);
    let region = crate::state::request_region(ctx.parts());
    let (resource, domain, owner, repository) = match reference {
        ResourceRef::Domain { owner, name } => (
            crate::state::domain_arn(&region, name),
            name.as_str(),
            owner.as_deref(),
            None,
        ),
        ResourceRef::Repository {
            owner,
            domain,
            repository,
        } => (
            crate::state::repository_arn(&region, domain, repository),
            domain.as_str(),
            owner.as_deref(),
            Some(repository.as_str()),
        ),
        ResourceRef::PackageGroup {
            owner,
            domain,
            pattern,
        } => (
            crate::state::package_group_arn(&region, domain, pattern),
            domain.as_str(),
            owner.as_deref(),
            None,
        ),
    };
    let action = format!("{ACTION_PREFIX}:{operation}");
    let policies = resource_policies(engine, domain, owner, repository);
    decide(&caller, &action, &resource, &policies)
}

/// The decision itself, over documents already collected: the identity-based policies in
/// force on the caller and the resource policies in force on the addressed resource (empty
/// when neither the resource nor its domain carries one, or this service holds neither).
fn decide(
    caller: &Caller,
    action: &str,
    resource: &str,
    policies: &[String],
) -> Result<(), AccessDeniedException> {
    // Identity-based documents state no `Principal` element, so the principal spellings a
    // resource-based policy is matched against are not part of that half of the decision.
    let identity_request = iam::Request::new(action, resource, &[]);
    let identity = iam::identity_decision(&caller.documents, &identity_request);
    if identity == Decision::Denied {
        return Err(not_authorized(&caller.arn, action, resource, true));
    }

    // A `Deny` is decided against the caller's own spellings and its account's together,
    // because denying an account denies each of its principals; a GRANT is decided against
    // the caller's own alone (see the module docs).
    let mut denying = caller.principals.clone();
    denying.extend(iam::account_principals(&caller.arn));
    let mut resource_said = Decision::NotStated;
    for document in policies {
        if iam::resource_decision(document, &iam::Request::new(action, resource, &denying))
            == Decision::Denied
        {
            return Err(denied_by_resource_policy(&caller.arn, action, resource));
        }
        if iam::resource_decision(
            document,
            &iam::Request::new(action, resource, &caller.principals),
        ) == Decision::Allowed
        {
            resource_said = Decision::Allowed;
        }
    }

    if iam::is_allowed(identity, resource_said) {
        return Ok(());
    }
    // Nothing the stores hold grants the action — but a grant this emulator cannot READ may:
    // see the module docs.
    if caller.unreadable_grants {
        return Ok(());
    }
    Err(not_authorized(&caller.arn, action, resource, false))
}

/// The resource policy documents in force on the addressed resource: the policy of the domain
/// the request named, and — for a request that addressed a repository inside it — that
/// repository's own, each present only where this service holds the resource and the resource
/// carries a document.
///
/// Read under one short-lived guard, so no handler's state guard is held across this gate's
/// STS and IAM reads.
fn resource_policies<Eng: Send + Sync + 'static>(
    engine: &Eng,
    domain: &str,
    domain_owner: Option<&str>,
    repository: Option<&str>,
) -> Vec<String> {
    let Some(state) = crate::state::service_state(engine) else {
        return Vec::new();
    };
    let store = state.lock();
    let Some(held) = store.domain(domain, domain_owner) else {
        return Vec::new();
    };
    let mut documents = Vec::new();
    if let Some(document) = held.policy.as_ref().and_then(|policy| policy.document.clone()) {
        documents.push(document);
    }
    let repository_document = repository
        .and_then(|name| held.repositories.get(name))
        .and_then(|held| held.policy.as_ref())
        .and_then(|policy| policy.document.clone());
    if let Some(document) = repository_document {
        documents.push(document);
    }
    documents
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and why. Every value in it comes from the request and from the stored
/// policies: the principal from the credential the request presented, the action from the
/// operation being refused, the resource from the ARN the request addressed.
///
/// The resource clause is part of the sentence for EVERY refusal: an authorization decision
/// is always reached against some resource, and the clause is what tells an operator (and any
/// tooling parsing the message) which one the deny applies to. `explicit_deny` states the
/// clause IAM appends when a readable identity-based document DENIES the action rather than
/// merely not allowing it.
pub(crate) fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    explicit_deny: bool,
) -> AccessDeniedException {
    let because = if explicit_deny {
        " with an explicit deny in an identity-based policy".to_string()
    } else {
        format!(" because no identity-based policy allows the {action} action")
    };
    refusal(principal, action, resource, &because)
}

/// The refusal for an action a READABLE resource-based document denies: the same sentence,
/// naming the document class that denied it, because that is the document an operator has to
/// change.
fn denied_by_resource_policy(
    principal: &str,
    action: &str,
    resource: &str,
) -> AccessDeniedException {
    refusal(
        principal,
        action,
        resource,
        " with an explicit deny in a resource-based policy",
    )
}

/// IAM's refusal sentence: the principal, the action, the resource the action was judged
/// against, and the reason clause.
fn refusal(
    principal: &str,
    action: &str,
    resource: &str,
    because: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: format!(
            "User: {principal} is not authorized to perform: {action} on resource: \
             {resource}{because}"
        ),
    }
}

/// The principal this request speaks for, as `sts:GetCallerIdentity` reports it for the
/// credentials it was signed with.
///
/// The same resolution the authorization decision uses, exposed because the credential a mint
/// hands out carries its principal: a repository endpoint the token is later presented to has
/// to decide what THAT principal may do there. A request this emulator holds no session for
/// speaks for the account's own published identity, which is what STS answers for a long-term
/// key.
pub fn caller_principal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

    let Some(sts) = sts_store::service_state(engine) else {
        return sts_store::ambient_identity().arn;
    };
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let resolved = sts_store::resolve_caller(
        &mut sts.lock(),
        &access_key_id,
        ::std::time::SystemTime::now(),
        ctx.credential_store(),
    )
    .map(|identity| identity.arn)
    .unwrap_or_else(|_| sts_store::ambient_identity().arn);
    resolved
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
pub(crate) struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub(crate) arn: String,
    /// The spellings a resource policy statement names this principal ITSELF by: the session
    /// ARN STS reports, and the ARN of the IAM role behind that session. A statement naming
    /// one of them grants to the caller outright.
    pub(crate) principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub(crate) documents: Vec<String>,
    /// Set when the identity-based half of the decision could not be collected in full — a
    /// policy attached whose document the IAM store does not hold, or a caller STS holds no
    /// session for at all (see the module docs). Such a caller is not refused for the absence
    /// of a grant; a readable `Deny` still refuses it.
    pub(crate) unreadable_grants: bool,
}

/// The caller every decision is made about: the principal whose identity policies could be
/// READ, or — for the callers this gate cannot collect those for — the principal the request
/// speaks for, carrying no documents and marked [`Caller::unreadable_grants`].
///
/// Every request reaches a decision, because a resource policy's explicit `Deny` is a
/// refusal this service can reach from its OWN store whoever the caller is.
fn caller_or_unbounded<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    if let Some(bounded) = caller(engine, ctx) {
        return bounded;
    }
    let arn = caller_principal(engine, ctx);
    Caller {
        principals: vec![arn.clone()],
        arn,
        documents: Vec::new(),
        unreadable_grants: true,
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it,
/// or `None` for a caller whose identity-based grants this gate cannot read (see the module
/// docs).
pub(crate) fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<Caller> {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
    use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

    let sts = sts_store::service_state(engine)?;
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // Resolving the caller goes through STS's own `resolve_caller`, which first materialises
    // the session expiries the clock has already made due; that write is owed at the same
    // instant whether or not anybody called.
    let identity = sts_store::resolve_caller(
        &mut sts.lock(),
        &access_key_id,
        ::std::time::SystemTime::now(),
        ctx.credential_store(),
    )
    // An expired session credential is refused by the layer that verifies the request, not
    // here.
    .ok()?;
    // An assumed-role session's permissions are the ROLE's. Every other principal form — the
    // account's own published credential, a long-term key — is a caller this gate does not
    // bound.
    let role_name = assumed_role_name(&identity.arn)?;
    let iam = iam_store::service_state(engine)?;
    let store = iam.lock();
    // The role is addressed by the ARN the account holds it under — which carries its PATH,
    // where the session ARN does not — and a role the account does not hold is a caller this
    // gate does not bound.
    let role_arn = store
        .roles
        .get(&iam_store::name_key(role_name))?
        .arn
        .clone();
    // `principal_policies` is where the collection rule itself lives — the role's inline
    // policies and the default version of each managed policy attached to it — so this
    // service reads an identity's permissions exactly as IAM's own authorizer does.
    let policies = store.principal_policies(&role_arn)?;
    Some(Caller {
        principals: vec![identity.arn.clone(), role_arn],
        arn: identity.arn,
        documents: policies.documents,
        // An attachment the collection resolved no document for is an entry of AWS's own
        // catalog this emulator does not carry: a grant it cannot read.
        unreadable_grants: !policies.uncarried.is_empty(),
    })
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role is
/// the first segment after `assumed-role/`.
pub(crate) fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn.splitn(6, ':').nth(5)?;
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A caller carrying `documents`, named by its session and by the role behind it.
    fn session(role: &str, documents: &[&str], unreadable_grants: bool) -> Caller {
        let account = "000000000000";
        Caller {
            arn: format!("arn:aws:sts::{account}:assumed-role/{role}/session-1"),
            principals: vec![
                format!("arn:aws:sts::{account}:assumed-role/{role}/session-1"),
                format!("arn:aws:iam::{account}:role/{role}"),
            ],
            documents: documents.iter().map(|text| (*text).to_string()).collect(),
            unreadable_grants,
        }
    }

    /// The documents a decision is made from, as the collection hands them over.
    fn policies(documents: &[&str]) -> Vec<String> {
        documents.iter().map(|text| (*text).to_string()).collect()
    }

    /// RULE: either grant suffices and a single deny refuses — the decision is made from the
    /// caller's identity policies and the repository's own policy together.
    #[test]
    fn either_policy_may_grant_and_either_may_deny() {
        let action = "codeartifact:GetRepositoryEndpoint";
        let resource = "arn:aws:codeartifact:us-east-1:000000000000:repository/acme/releases";
        let identity_allow =
            r#"{"Statement":[{"Effect":"Allow","Action":"codeartifact:*","Resource":"*"}]}"#;
        let identity_deny = r#"{"Statement":[{"Effect":"Deny","Action":"codeartifact:GetRepositoryEndpoint","Resource":"*"}]}"#;
        let names_the_role = r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:role/granted"},"Action":["codeartifact:GetRepositoryEndpoint"],"Resource":"*"}]}"#;
        let denies_the_role = r#"{"Statement":[{"Effect":"Deny","Principal":{"AWS":"arn:aws:iam::000000000000:role/granted"},"Action":"codeartifact:*","Resource":"*"}]}"#;
        let denies_nobody = r#"{"Statement":[{"Effect":"Deny","Principal":{"AWS":"arn:aws:iam::000000000000:role/elsewhere"},"Action":"codeartifact:*","Resource":"*"}]}"#;
        let denies_the_account = r#"{"Statement":[{"Effect":"Deny","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"codeartifact:*","Resource":"*"}]}"#;

        // (case, role, identity documents, the resource policies in force, allowed)
        type Row<'a> = (&'a str, &'a str, &'a [&'a str], &'a [&'a str], bool);
        let rows: &[Row<'_>] = &[
            ("no policy anywhere", "other", &[], &[], false),
            (
                "the identity policy alone allows it",
                "other",
                &[identity_allow],
                &[],
                true,
            ),
            (
                "the repository policy alone names the role",
                "granted",
                &[],
                &[names_the_role],
                true,
            ),
            (
                "the repository policy names another role",
                "other",
                &[],
                &[names_the_role],
                false,
            ),
            (
                "the repository policy denies a role its identity policy allows",
                "granted",
                &[identity_allow],
                &[denies_the_role],
                false,
            ),
            (
                "the identity policy denies a role the repository policy allows",
                "granted",
                &[identity_deny],
                &[names_the_role],
                false,
            ),
            // Two documents in force — the domain's policy beside the repository's own.
            (
                "the first of two documents names the role",
                "granted",
                &[],
                &[names_the_role, denies_nobody],
                true,
            ),
            (
                "the second of two documents names the role",
                "granted",
                &[],
                &[denies_nobody, names_the_role],
                true,
            ),
            (
                "one document denies what the other allows",
                "granted",
                &[],
                &[denies_the_role, names_the_role],
                false,
            ),
            (
                "the allowing document is read before the denying one",
                "granted",
                &[],
                &[names_the_role, denies_the_role],
                false,
            ),
            (
                "a document denies the whole account",
                "granted",
                &[identity_allow],
                &[denies_the_account],
                false,
            ),
        ];
        for (case, role, documents, in_force, allowed) in rows {
            let decided = decide(
                &session(role, documents, false),
                action,
                resource,
                &policies(in_force),
            );
            assert_eq!(
                decided.is_ok(),
                *allowed,
                "case {case:?}: expected allowed={allowed}, got {decided:?}"
            );
        }
    }

    /// RULE: a caller whose grants this gate cannot read is not refused on evidence it does
    /// not have — but a readable deny still refuses it.
    #[test]
    fn a_caller_with_unreadable_grants_is_not_refused_but_a_readable_deny_still_refuses() {
        let action = "codeartifact:DescribeRepository";
        let resource = "arn:aws:codeartifact:us-west-2:000000000000:repository/acme/releases";
        let denies_everyone = r#"{"Statement":[{"Effect":"Deny","Principal":"*","Action":"codeartifact:*","Resource":"*"}]}"#;
        let names_someone_else = r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:role/elsewhere"},"Action":"codeartifact:*","Resource":"*"}]}"#;

        // (case, the documents in force, whether the call is served)
        let rows: &[(&str, &[&str], bool)] = &[
            ("no document in force", &[], true),
            ("a document naming another principal", &[names_someone_else], true),
            ("a document that denies everyone", &[denies_everyone], false),
            (
                "a denying document beside an unrelated one",
                &[names_someone_else, denies_everyone],
                false,
            ),
        ];
        for (case, in_force, served) in rows {
            let decided = decide(
                &session("other", &[], true),
                action,
                resource,
                &policies(in_force),
            );
            assert_eq!(
                decided.is_ok(),
                *served,
                "case {case:?}: expected served={served}, got {decided:?}"
            );
        }
    }

    /// RULE: a session ARN names the role whose policies are read; every other principal form
    /// names none, and is a caller this gate does not bound.
    #[test]
    fn a_principal_arn_names_the_role_its_policies_are_read_from() {
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/ca-publisher/session-1",
                Some("ca-publisher"),
            ),
            (
                "a session name carrying a slash",
                "arn:aws:sts::000000000000:assumed-role/ca-publisher/one/two",
                Some("ca-publisher"),
            ),
            (
                "an IAM user",
                "arn:aws:iam::000000000000:user/ca-user",
                None,
            ),
            ("the account's own identity", "arn:aws:iam::000000000000:root", None),
            ("a value that is no ARN", "AKIDTESTONLY", None),
        ];
        for (case, arn, expected) in rows {
            let named = assumed_role_name(arn);
            assert_eq!(
                named, *expected,
                "case {case:?}: expected the role {expected:?}, got {named:?}"
            );
        }
    }

    /// RULE: the refusal names the principal, the action and the resource the action was
    /// judged against, with the clause naming WHY — nothing allowed it, or a readable document
    /// of that class denied it.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = "arn:aws:sts::000000000000:assumed-role/ca-publisher/session-1";
        let action = "codeartifact:GetRepositoryEndpoint";
        let repository = "arn:aws:codeartifact:us-east-1:000000000000:repository/acme/releases";
        // (case, the refusal, the whole sentence expected)
        let rows: &[(&str, AccessDeniedException, String)] = &[
            (
                "allowed by nothing",
                not_authorized(principal, action, repository, false),
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {repository} because no identity-based policy allows the {action} action"
                ),
            ),
            (
                "denied by an identity-based policy",
                not_authorized(principal, action, repository, true),
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {repository} with an explicit deny in an identity-based policy"
                ),
            ),
            (
                "denied by the repository's own policy",
                denied_by_resource_policy(principal, action, repository),
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {repository} with an explicit deny in a resource-based policy"
                ),
            ),
        ];
        for (case, refusal, expected) in rows {
            assert_eq!(
                &refusal.message, expected,
                "case {case:?}: expected IAM's wording, got {:?}",
                refusal.message
            );
        }
    }
}
