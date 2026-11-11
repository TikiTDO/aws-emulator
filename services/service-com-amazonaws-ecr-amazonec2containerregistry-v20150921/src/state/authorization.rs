// SPDX-License-Identifier: CC0-1.0

//! Who a request to this registry speaks for, and whether a policy in force lets it reach
//! the images and layers of the repository it names.
//!
//! A private registry's contents are private: every operation of this service is signed
//! (`smithy_query(query="service_info")`: `authSchemes: [sigv4]`), so every call is made by
//! a principal, and a principal may only perform an action some policy in force ALLOWS.
//! Two documents can allow one — the calling principal's identity-based policies, and the
//! repository's own resource-based policy, which is what shares a repository's images with a
//! principal that has no identity-based grant — and with neither the call is refused with
//! the AWS-common `AccessDeniedException` BEFORE the operation reads or writes anything. So
//! a refused pull reports no manifest, a refused push stores nothing, and adding the missing
//! grant — in either document — makes the identical call succeed. Nothing is cached: a
//! repository policy that was just set grants immediately, and one that was just deleted
//! stops granting immediately.
//!
//! There is no `AccessDeniedException` in this model (its only service-common error is
//! `ServerException`) for the reason there is none in any other service's: the decision is
//! taken in front of the operation, so it is not one of the operation's own outcomes, and
//! the refusal travels as the common error ([`crate::scaffolded::Scaffolded::access_denied`]).
//!
//! The four pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from ([`sts_store::resolve_caller`]) for a
//!    session credential and from IAM's own `access_keys` map (the map `iam:GetUser`
//!    resolves a caller through) for a long-term key, so the principal a refusal blames is
//!    the one the caller reads back for the credentials it signed with;
//! 2. **what it may do** — the identity-based documents the IAM store holds for that
//!    principal, collected by IAM's own
//!    [`principal_policies`](iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State::principal_policies),
//!    so this service sees exactly what IAM's handlers wrote;
//! 3. **what the repository shares** — the document `SetRepositoryPolicy` stored on the
//!    repository the request names ([`crate::state::Repo::policy_text`]), read from this
//!    service's own store;
//! 4. **the decision** — [`::service_common::iam_authorization`], the shared evaluation of
//!    those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` on either side refuses,
//!    otherwise an `Allow` from either side within one account grants, and with neither the
//!    call falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split Amazon
//! ECR Public's own gate makes, and the lookups stay in this service crate for the reason it
//! keeps its there: hoisting them into `service-common` would make every service depending
//! on that crate link the IAM and STS service crates.
//!
//! ## What a decision is made ABOUT
//!
//! The action is `ecr:<OperationName>` — this service's `arnNamespace`
//! (`smithy_query(query="service_info")`: `arnNamespace: ecr`) and the operation's own
//! shape name, which the caller passes as its `Operation::NAME` so the action can never
//! drift from the operation taking it. The resource is the ARN of the repository the request
//! NAMES, built from the request and the account contract by the one builder every reported
//! repository ARN comes from ([`crate::state::repository_arn`]), so a statement scoped to
//! the repository a client reads back is the statement that allows the call — and so the
//! decision never depends on whether that repository exists, which is what keeps a refused
//! caller from learning what this registry holds.
//!
//! ## The two callers the identity decision cannot refuse
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to [`sts_store::ambient_identity`], the account's
//!   own identity, which is not a principal the IAM store holds policies for — the same
//!   position AWS account root credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   is owned by AWS, not by the account, so the IAM store holds no document for every entry
//!   of AWS's catalog (`PrincipalPolicies::uncarried`). A principal with such a policy
//!   attached is allowed: this service cannot see what that document grants, and refusing on
//!   evidence it does not have would deny access AWS permits. An attachment naming a
//!   CUSTOMER-managed policy the account does not hold grants nothing, because the account
//!   is the authority on its own policies.
//!
//! Either way an explicit `Deny` in a document that CAN be read still refuses: the decision
//! is taken first, and only the DEFAULT deny of a call no readable statement allows is
//! lifted for those two.
//!
//! An expired session credential speaks for nobody: it carries no document and no principal
//! spelling, so every action it asks for is refused.
//!
//! Resolving the caller goes through STS's own `resolve_caller`, which materialises the
//! session expiries the clock has already made due before it answers: that write is owed at
//! the same instant whether or not anybody called, which is why a REPORTING operation of
//! this service may make this decision — what a later caller sees is unchanged by the read.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `ecr:<OperationName>` ("Actions defined by
/// Amazon Elastic Container Registry", IAM Service Authorization Reference), which is also
/// this service's `arnNamespace` (`smithy_query(query="service_info")`: `arnNamespace: ecr`)
/// and the namespace segment of every repository ARN this registry mints.
pub const ACTION_PREFIX: &str = "ecr";

/// The ARN segment of an STS assumed-role session, whose principal is the ROLE it is a
/// session of.
const STS_NAMESPACE: &str = "sts";

/// The resource prefix of an assumed-role session's ARN:
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`.
const ASSUMED_ROLE_PREFIX: &str = "assumed-role/";

/// The IAM action `operation` is performed under: `ecr:<OperationName>`.
#[must_use]
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The resource a call addressing the repository `repository_name` is decided against: that
/// repository's ARN in the Region the request was signed for, minted by the one builder
/// every reported repository ARN comes from ([`crate::state::repository_arn`]).
#[must_use]
pub fn repository_resource(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    repository_name: &str,
) -> String {
    crate::state::repository_arn(
        &crate::state::request_region(ctx.parts()),
        repository_name,
    )
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub arn: String,
    /// Every spelling a resource-based policy may name this caller by: its own ARN, the ARN
    /// of the role behind an assumed-role session, and the account it belongs to.
    pub principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    pub account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an entry of AWS's own catalog (see the module docs).
    pub unreadable_attachment: bool,
}

impl Caller {
    /// Whether the DEFAULT deny cannot refuse this caller: the account's own credential, or
    /// a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }
}

/// The refusal AWS words an authorization decision with: who was refused, which action, on
/// which resource, and why the grant was withheld. Every value in it comes from the request
/// and the stored policies — the principal from the credential presented, the action from
/// the operation named, the resource from the repository addressed.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// Why no grant was found, as the reason clause of [`not_authorized`]: an explicit deny in
/// one of the two documents, or no statement allowing the action at all.
///
/// The ONE wording of that clause in this service: the in-process pull gate words its own
/// refusal with it too ([`super::registry_pull`]), so a caller refused a repository and a
/// compute service refused an image are told the same thing about the same documents.
pub(crate) fn withheld_because(identity: Decision, stated: Decision, action: &str) -> String {
    if identity == Decision::Denied {
        "with an explicit deny in an identity-based policy".to_string()
    } else if stated == Decision::Denied {
        "with an explicit deny in a resource-based policy".to_string()
    } else {
        format!("because no identity-based policy allows the {action} action")
    }
}

/// `Some(message)` when nothing in force lets the caller perform `ecr:<operation>` on the
/// repository `repository_name` names — the message the refusal carries — and `None` when
/// something does.
///
/// A READ: the two stores are only consulted, so deciding a call changes nothing a later
/// caller sees.
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    repository_name: &str,
) -> Option<String> {
    let action = action_for(operation);
    let resource = repository_resource(ctx, repository_name);
    let caller = caller(engine, ctx);
    // The document the repository SHARES itself with, read before the decision and released
    // again: the gate holds no lock while it reads the IAM and STS stores.
    let shared = repository_policy(engine, repository_name);

    let request = iam::Request::new(&action, &resource, &caller.principals);
    let identity = iam::identity_decision(&caller.documents, &request);
    let stated = shared
        .as_deref()
        .map_or(Decision::NotStated, |document| {
            iam::resource_decision(document, &request)
        });
    if iam::is_allowed(identity, stated) {
        return None;
    }
    // Only the default deny is lifted for the two callers the identity decision cannot
    // refuse, so an explicit deny in a document that CAN be read still refuses them.
    if identity != Decision::Denied && stated != Decision::Denied && caller.identity_is_unbounded()
    {
        return None;
    }
    // The refusal is the security-relevant event of this call, so it is attributable in the
    // emulator's audit log the way this service's layer operations log theirs: who asked,
    // for what, and the outcome. The policy documents themselves are not logged.
    ::tracing::info!(
        target: "emulator::audit",
        audit = true,
        operation,
        outcome = "refused_access_denied",
        principal = %caller.arn,
        action = %action,
        resource = %resource,
        explicit_deny = identity == Decision::Denied || stated == Decision::Denied,
        "no policy in force permits this caller to reach this repository"
    );
    Some(not_authorized(
        &caller.arn,
        &action,
        &resource,
        &withheld_because(identity, stated, &action),
    ))
}

/// The refusal an operation addressing the repository `repository_name` returns for a
/// request no policy in force permits, or `None` when it is permitted: the one call an
/// implemented handler makes, placed before it reads or writes this service's state.
///
/// Generic over the operation's own modelled error enum because the refusal is NOT one of
/// them — this service's model declares no `AccessDeniedException` — so it travels through
/// the error channel's own variant.
pub fn denied_on_repository<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    repository_name: &str,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    refusal(engine, ctx, operation, repository_name)
        .map(crate::scaffolded::Scaffolded::access_denied)
}

/// The resource-based policy the repository `repository_name` carries, cloned out of this
/// service's store so no lock is held while the decision is taken.
///
/// `None` for a repository that carries none AND for one this registry does not hold: a
/// repository that is not here shares nothing, which is what keeps the decision independent
/// of whether it exists.
fn repository_policy<Eng: Send + Sync + 'static>(
    engine: &Eng,
    repository_name: &str,
) -> Option<String> {
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    store
        .repositories
        .get(repository_name)
        .and_then(|repository| repository.policy_text.clone())
}

/// The principal this request speaks for, with the policies in force on it and every
/// spelling a resource-based policy may name it by.
pub fn caller<Eng: Send + Sync + 'static>(
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
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody.
        Err(expired) => {
            return Caller {
                arn: format!("an expired session credential ({})", expired.access_key_id),
                principals: Vec::new(),
                documents: Vec::new(),
                account_credential: false,
                unreadable_attachment: false,
            }
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map `iam:GetUser` resolves a caller
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
    // The IAM identity the principal's policies are held under: an assumed-role session's
    // are the ROLE's, and a long-term key's are the user's own. The role is also a principal
    // a resource-based policy may name the session by, which is how a repository policy
    // grants a role rather than one of its sessions.
    let entity_arn = match assumed_role_name(&caller.arn) {
        Some(role_name) => store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|role| role.arn.clone()),
        None => Some(caller.arn.clone()),
    };
    if let Some(entity_arn) = entity_arn.as_deref() {
        if entity_arn != caller.arn {
            caller.principals.push(entity_arn.to_string());
        }
        if let Some(policies) = store.principal_policies(entity_arn) {
            caller.unreadable_attachment = !policies.uncarried.is_empty();
            caller.documents = policies.documents;
        }
    }
    // The account the principal belongs to, in the spellings a resource-based policy may
    // name it by — the shared reading of them, so this service matches a `Principal`
    // element the way every other one does.
    let account = iam::account_principals(&caller.arn);
    caller.principals.extend(account);
    caller
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role is
/// the first segment after the resource type. `None` for any other ARN form. Read through
/// the tree's one ARN parse (`service_common::arn`), so a session ARN is split the way every
/// other ARN is.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != STS_NAMESPACE {
        return None;
    }
    let name = parsed
        .resource
        .strip_prefix(ASSUMED_ROLE_PREFIX)?
        .split('/')
        .next()?;
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository of this registry, as every ARN this service mints spells one.
    fn repository(name: &str) -> String {
        crate::state::repository_arn("us-east-1", name)
    }

    /// What one identity-based document says about `action` on `resource`, through the
    /// shared evaluation this gate consults.
    fn identity_says(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// What one repository policy says about `action` on `resource` for `principals`.
    fn repository_says(
        document: &str,
        action: &str,
        resource: &str,
        principals: &[String],
    ) -> Decision {
        iam::resource_decision(
            document,
            &iam::Request::new(action, resource, principals),
        )
    }

    #[test]
    fn a_principal_with_no_document_and_a_repository_with_no_policy_allow_nothing() {
        for operation in ["BatchGetImage", "GetDownloadUrlForLayer", "PutImage"] {
            let action = action_for(operation);
            let said = iam::identity_decision(
                &[],
                &iam::Request::new(&action, &repository("probe"), &[]),
            );
            assert_eq!(
                said,
                Decision::NotStated,
                "case {operation:?}: expected no grant from an empty document set, got {said:?}"
            );
            assert!(
                !iam::is_allowed(said, Decision::NotStated),
                "case {operation:?}: nothing in force must permit {action}"
            );
        }
    }

    #[test]
    fn a_grant_is_read_for_the_action_and_the_resource_it_names() {
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Action":"ecr:BatchGetImage","Resource":"{}"}}]}}"#,
            repository("granted/app")
        );
        // (case, operation, resource, allowed)
        let rows: &[(&str, &str, String, bool)] = &[
            (
                "the granted action on the granted repository",
                "BatchGetImage",
                repository("granted/app"),
                true,
            ),
            (
                "another action on the same repository",
                "PutImage",
                repository("granted/app"),
                false,
            ),
            (
                "the granted action on another repository",
                "BatchGetImage",
                repository("other-app"),
                false,
            ),
        ];
        for (case, operation, resource, expected) in rows {
            let said = identity_says(&document, &action_for(operation), resource);
            let actual = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected allowed == {expected}, got {actual} ({said:?})"
            );
        }
    }

    #[test]
    fn a_repository_policy_grants_the_principal_it_names_and_no_other() {
        let session = "arn:aws:sts::000000000000:assumed-role/puller/probe".to_string();
        let role = "arn:aws:iam::000000000000:role/puller".to_string();
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Principal":{{"AWS":"{role}"}},"Action":"ecr:BatchGetImage","Resource":"*"}}]}}"#
        );
        let action = action_for("BatchGetImage");
        let resource = repository("shared/app");
        // (case, the principals the caller is matched by, allowed)
        let rows: &[(&str, Vec<String>, bool)] = &[
            (
                "the session of the role the policy names",
                vec![session.clone(), role.clone()],
                true,
            ),
            (
                "a session of another role",
                vec![
                    "arn:aws:sts::000000000000:assumed-role/other/probe".to_string(),
                    "arn:aws:iam::000000000000:role/other".to_string(),
                ],
                false,
            ),
        ];
        for (case, principals, expected) in rows {
            let said = repository_says(&document, &action, &resource, principals);
            let actual = iam::is_allowed(Decision::NotStated, said);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected allowed == {expected}, got {actual} ({said:?})"
            );
        }
    }

    #[test]
    fn an_explicit_deny_wins_over_a_wildcard_grant() {
        let document = r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"ecr:*","Resource":"*"},
            {"Effect":"Deny","Action":"ecr:BatchGetImage","Resource":"*"}]}"#;
        let denied = identity_says(document, &action_for("BatchGetImage"), &repository("probe"));
        assert_eq!(
            denied,
            Decision::Denied,
            "case \"the denied action\": expected Denied, got {denied:?}"
        );
        let allowed = identity_says(document, &action_for("ListImages"), &repository("probe"));
        assert_eq!(
            allowed,
            Decision::Allowed,
            "case \"an action the deny does not name\": expected Allowed, got {allowed:?}"
        );
    }

    #[test]
    fn the_refusal_names_the_principal_the_action_the_resource_and_the_reason() {
        let principal = "arn:aws:sts::000000000000:assumed-role/noperm/probe";
        let action = action_for("BatchGetImage");
        let resource = repository("probe");
        // (case, what each document said, the clause the message must carry)
        let rows: &[(&str, Decision, Decision, String)] = &[
            (
                "nothing allows it",
                Decision::NotStated,
                Decision::NotStated,
                format!("because no identity-based policy allows the {action} action"),
            ),
            (
                "an identity-based policy denies it",
                Decision::Denied,
                Decision::NotStated,
                "with an explicit deny in an identity-based policy".to_string(),
            ),
            (
                "the repository policy denies it",
                Decision::NotStated,
                Decision::Denied,
                "with an explicit deny in a resource-based policy".to_string(),
            ),
        ];
        for (case, identity, stated, reason) in rows {
            let message = not_authorized(
                principal,
                &action,
                &resource,
                &withheld_because(*identity, *stated, &action),
            );
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

    #[test]
    fn a_session_arn_names_the_role_it_is_a_session_of() {
        // (case, ARN, the role it names)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/registry-admin/session-1",
                Some("registry-admin"),
            ),
            (
                "an IAM user",
                "arn:aws:iam::000000000000:user/noperm-probe",
                None,
            ),
            (
                "the account's own identity",
                "arn:aws:iam::000000000000:root",
                None,
            ),
            ("a value that is no ARN", "AKIDTESTONLY", None),
        ];
        for (case, arn, role) in rows {
            let actual = assumed_role_name(arn);
            assert_eq!(
                actual, *role,
                "case {case:?}: expected role {role:?}, got {actual:?}"
            );
        }
    }
}
