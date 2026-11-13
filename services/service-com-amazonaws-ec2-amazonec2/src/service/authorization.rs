// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the EC2 action it names — the
//! decision taken ahead of EVERY request, before any handler reaches the store.
//!
//! EC2 refuses a caller that may not perform the action it asked for with its own
//! `UnauthorizedOperation` code (HTTP 403), which the `DryRun` member of every request
//! documents as the other side of the permission probe: "Checks whether you have the
//! required permissions for the action … If you have the required permissions, the error
//! response is `DryRunOperation`. Otherwise, it is `UnauthorizedOperation`". So the
//! decision is a fact about the REQUEST's credential and the policies in force at the
//! instant of the call, not about the resources the request names — which is why it is
//! taken once here rather than per handler.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    holds the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a policy `iam:PutRolePolicy` wrote takes
//!    effect on the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the ARN readings do. WHICH
//! IAM entity a principal ARN names is read through the shared
//! [`::service_common::iam_role`] rather than parsed again.
//!
//! ## How each action is decided, and the resources it names
//!
//! Every EC2 action is decided here. What differs per action is the RESOURCE the decision
//! is taken against, which is what a statement has to cover to grant it.
//!
//! An EC2 `Describe` action supports no resource-level permissions — "the Amazon EC2 API
//! actions that begin with `Describe` do not support resource-level permissions", Amazon
//! EC2 User Guide, "Resource-level permissions for Amazon EC2" — so IAM judges it against
//! every resource ([`EVERY_RESOURCE`]) and a statement permits it only by covering every
//! resource. [`judged_over_every_resource`] is the test, and a refusal of such an action
//! names no resource because there is none to name.
//!
//! Every OTHER EC2 action is judged against the resources the request ADDRESSES, read from
//! the request's own ids ([`super::addressed_resources`]): one decision per resource, so a
//! statement scoped to resource ARNs grants the action exactly on the resources it names,
//! and the first resource no statement allows is the one the request is refused ON — which
//! the refusal then states.
//!
//! A request of such an action that addresses no resource this gate could read an ARN for
//! (a create that names nothing existing, a value of a kind the ARN reading does not carry)
//! is decided by the two readings no resource can change: a `Deny` covering every resource
//! refuses it, and documents holding no `Allow` of the action at all permit it nowhere.
//! Between those it is served, because a resource-scoped statement may well cover whatever
//! the request addresses and withholding the grant would refuse a call AWS permits.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the account's store may hold no
//!   document for it; refusing on a grant this emulator cannot see would deny access AWS
//!   permits. An explicit `Deny` in a document that CAN be read still refuses, and a
//!   permissions boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)
//!
//! Resolving the caller goes through STS's own `resolve_caller`, which first materialises
//! the session expiries the clock has already made due; that write is owed at the same
//! instant whether or not anybody called, which is why a REPORTING operation may take this
//! decision too.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `ec2:<OperationName>` ("Actions defined
/// by Amazon EC2", IAM Service Authorization Reference), which is this service's
/// `arnNamespace` (`smithy_query(query="service_info")`).
pub const ACTION_PREFIX: &str = "ec2";

/// The resource an action that supports no resource-level permissions is judged against,
/// as IAM writes it for such a request: every resource.
pub const EVERY_RESOURCE: &str = "*";

/// The sentence EC2 opens an `UnauthorizedOperation` refusal with, ahead of the IAM
/// clause that says who was refused what and why.
pub const NOT_AUTHORIZED_SENTENCE: &str = "You are not authorized to perform this operation.";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The name prefix of the EC2 operations whose action IAM judges over every resource. See
/// the module docs for the documented rule this is the one test of.
const ACCOUNT_WIDE_READ_PREFIX: &str = "Describe";

/// Whether IAM judges `operation`'s action over every resource, so a statement permits it
/// only by covering every resource (see the module docs).
#[must_use]
pub fn judged_over_every_resource(operation: &str) -> bool {
    operation.starts_with(ACCOUNT_WIDE_READ_PREFIX)
}

/// An authorization refusal: who was refused, which action, on which resource when the
/// decision was taken on one, and which of IAM's rules withheld the grant.
#[derive(Debug, Clone)]
pub struct Denied {
    /// The principal the request speaks for, as `sts:GetCallerIdentity` reports it.
    principal: String,
    /// The IAM action the request asked for.
    action: String,
    /// The ARN of the resource the request was refused ON, when the decision was taken
    /// against a resource the request addressed; `None` when there was none to name.
    resource: Option<String>,
    /// Why the grant was withheld, as the refusal's closing clause words it.
    reason: String,
}

impl Denied {
    /// The refusal in EC2's own wording: its fixed opening sentence, then the IAM clause
    /// naming the principal the credential speaks for, the action the request asked for,
    /// the resource it was refused on and which of IAM's rules withheld the grant.
    ///
    /// A refusal that NAMES a resource closes the sentence with a full stop; one that names
    /// none does not, there being no resource clause to close. A resource is named exactly
    /// when the decision was taken on one: an action IAM judges over every resource has no
    /// resource to name, and neither has a request that addresses none (see the module
    /// docs).
    #[must_use]
    pub fn message(&self) -> String {
        match &self.resource {
            Some(resource) => format!(
                "{NOT_AUTHORIZED_SENTENCE} {}.",
                iam::not_authorized_by_identity_policy(
                    &self.principal,
                    &self.action,
                    resource,
                    &self.reason,
                )
            ),
            None => format!(
                "{NOT_AUTHORIZED_SENTENCE} User: {} is not authorized to perform: {} {}",
                self.principal, self.action, self.reason
            ),
        }
    }
}

/// Whether IAM lets this request perform the action of `operation` — the operation's own
/// name, which is the routing key the request named — given `resources`, the ARNs of the
/// resources the request addresses ([`super::addressed_resources`]).
///
/// `Ok(())` for a request IAM permits and for every caller this gate does not bound (see
/// the module docs); `Err` carries the refusal EC2 answers as `UnauthorizedOperation`.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resources: &[String],
) -> Result<(), Denied> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    let documents = iam::prepare_documents(&caller.documents);
    let boundary = caller
        .boundary
        .as_ref()
        .map(|boundary| iam::prepare_documents(::std::slice::from_ref(boundary)));
    let boundary = boundary.as_deref();
    // An action that supports no resource-level permissions is judged over EVERY resource,
    // and a refusal of it names no resource because there is none to name.
    if judged_over_every_resource(operation) {
        return decide(&caller, &documents, boundary, &action, EVERY_RESOURCE, None);
    }
    // Every other action is judged against the resources the request addresses, one
    // decision per resource: a grant has to cover the resource, so the first one no
    // statement allows is the one the request is refused on.
    if resources.is_empty() {
        return decide_over_any_resource(&caller, &documents, boundary, &action);
    }
    for resource in resources {
        decide(
            &caller,
            &documents,
            boundary,
            &action,
            resource,
            Some(resource),
        )?;
    }
    Ok(())
}

/// IAM's decision about `action` on `judged`, refused in the name of `named`.
///
/// `named` is what a refusal states the request was refused ON — the resource the request
/// addressed, or `None` where the decision was taken over every resource and there is no
/// resource to name.
fn decide(
    caller: &Caller,
    documents: &[iam::PreparedDocument],
    boundary: Option<&[iam::PreparedDocument]>,
    action: &str,
    judged: &str,
    named: Option<&str>,
) -> Result<(), Denied> {
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, so the decision below rests on the action and the resource.
    let request = iam::Request::new(action, judged, &caller.principals);
    let identity_said = iam::identity_decision_prepared(documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(caller.refused(action, named, iam::IDENTITY_POLICY_EXPLICIT_DENY));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(caller.refused(action, named, &iam::no_identity_policy_allows(action)));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
    // is not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = boundary {
        let reason = match iam::identity_decision_prepared(boundary, &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(action),
        };
        return Err(caller.refused(action, named, &reason));
    }
    Ok(())
}

/// IAM's decision about `action` for a request that addresses no resource this gate could
/// read an ARN for, where the resource a statement must cover is not known.
///
/// Two readings the resource cannot change: a `Deny` covering EVERY resource refuses the
/// request whichever resource it turns out to address, and documents holding no `Allow` of
/// the action at all permit it on no resource
/// ([`::service_common::iam_authorization::identity_may_allow_action`]). Between the two,
/// the request is served — a statement scoped to resource ARNs grants the action on those
/// resources, and withholding the grant here would refuse a call AWS permits. A refusal
/// names no resource, there being none the gate read.
fn decide_over_any_resource(
    caller: &Caller,
    documents: &[iam::PreparedDocument],
    boundary: Option<&[iam::PreparedDocument]>,
    action: &str,
) -> Result<(), Denied> {
    let request = iam::Request::new(action, EVERY_RESOURCE, &caller.principals);
    if iam::identity_decision_prepared(documents, &request) == Decision::Denied {
        return Err(caller.refused(action, None, iam::IDENTITY_POLICY_EXPLICIT_DENY));
    }
    let granted =
        iam::identity_may_allow_action(documents, action) || caller.identity_is_unbounded();
    if !granted {
        return Err(caller.refused(action, None, &iam::no_identity_policy_allows(action)));
    }
    if let Some(boundary) = boundary {
        if iam::identity_may_allow_action(boundary, action) {
            return Ok(());
        }
        let reason = match iam::identity_decision_prepared(boundary, &request) {
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            _ => iam::no_permissions_boundary_allows(action),
        };
        return Err(caller.refused(action, None, &reason));
    }
    Ok(())
}

/// Every spelling of the principal this request speaks for: the session ARN
/// `sts:GetCallerIdentity` reports, and the ARN of the IAM role behind an assumed-role
/// session.
///
/// The SAME resolution the authorization gate decides on ([`caller`]), so the principal a
/// refusal blames and the principal a fault injected against a role affects
/// ([`super::injected_faults`]) are one answer and not two: a request signed with a session
/// of a role is the role's request for both questions. Empty for a credential that speaks
/// for nobody.
pub fn request_principals<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Vec<String> {
    caller(engine, ctx).principals
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

    /// This caller's refusal of `action` on `resource`, closed by `reason`.
    fn refused(&self, action: &str, resource: Option<&str>, reason: &str) -> Denied {
        Denied {
            principal: self.arn.clone(),
            action: action.to_string(),
            resource: resource.map(str::to_string),
            reason: reason.to_string(),
        }
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
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            caller.read_policies_of(&store, &held.arn.clone());
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

    /// The decision one stored document reaches about an action, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, EVERY_RESOURCE, &[]),
        )
    }

    /// RULE: the action this gate asks about is `ec2:<OperationName>`, so a policy scoped
    /// to this service's actions covers the operations of it and nothing outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"ec2:*","Resource":"*"}]}"#;
        for operation in [
            "DescribeVpcs",
            "DescribeSubnets",
            "DescribeRouteTables",
            "DescribeAvailabilityZones",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(document, &action);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected ec2:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "ssm:GetParameter");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected ec2:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: a grant scoped to particular resource ARNs does not cover an action IAM judges
    /// over EVERY resource, which is the reason this gate decides those actions alone.
    #[test]
    fn a_resource_scoped_grant_does_not_cover_an_account_wide_action() {
        let scoped = r#"{"Statement":[{"Effect":"Allow","Action":"ec2:DescribeVpcs",
             "Resource":"arn:aws:ec2:us-west-2:000000000000:vpc/vpc-0123456789abcdef0"}]}"#;
        let reached = decision(scoped, "ec2:DescribeVpcs");
        assert_eq!(
            reached,
            Decision::NotStated,
            "case \"a resource-scoped grant\": expected no grant of an account-wide \
             action, got {reached:?}"
        );
    }

    /// RULE: the refusal opens with EC2's fixed sentence and then names the principal, the
    /// action, the resource the decision was taken on and which of IAM's rules withheld the
    /// grant — every part taken from the request and the stored policies. A decision taken
    /// over every resource names none.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_rule() {
        let principal = "arn:aws:sts::000000000000:assumed-role/cdk-lookup/session-1";
        let action = format!("{ACTION_PREFIX}:DescribeVpcs");
        // (case, the clause the decision closed with)
        let rows: &[(&str, String)] = &[
            (
                "an explicit deny",
                "with an explicit deny in an identity-based policy".to_string(),
            ),
            (
                "no statement at all",
                format!("because no identity-based policy allows the {action} action"),
            ),
        ];
        for (case, reason) in rows {
            let refusal = Denied {
                principal: principal.to_string(),
                action: action.clone(),
                resource: None,
                reason: reason.clone(),
            };
            let message = refusal.message();
            assert_eq!(
                message,
                format!(
                    "{NOT_AUTHORIZED_SENTENCE} User: {principal} is not authorized to \
                     perform: {action} {reason}"
                ),
                "case {case:?}: expected EC2's wording, got {message:?}"
            );
            assert!(
                !message.contains("on resource"),
                "case {case:?}: an action judged over every resource names none, got \
                 {message:?}"
            );
        }

        // A decision taken on a resource the request addressed names it, and closes the
        // sentence.
        let action = format!("{ACTION_PREFIX}:CreateNetworkInterface");
        let resource = ::engine::emulator_arn("ec2", "us-west-2", "subnet/subnet-1a2b3c4d");
        let reason = iam::no_identity_policy_allows(&action);
        let refusal = Denied {
            principal: principal.to_string(),
            action: action.clone(),
            resource: Some(resource.clone()),
            reason: reason.clone(),
        };
        let message = refusal.message();
        assert_eq!(
            message,
            format!(
                "{NOT_AUTHORIZED_SENTENCE} User: {principal} is not authorized to perform: \
                 {action} on resource: {resource} {reason}."
            ),
            "case \"a resource-level action\": expected the resource to be named, got \
             {message:?}"
        );
    }

    /// RULE: the actions IAM judges over every resource — the ones that support no
    /// resource-level permissions, the `Describe` family — are the ones whose grant must
    /// cover every resource; every other action is judged against the resources the request
    /// addresses.
    #[test]
    fn the_describe_family_is_what_iam_judges_over_every_resource() {
        // (the operation, whether IAM judges its action over every resource)
        let rows: &[(&str, bool)] = &[
            ("DescribeVpcs", true),
            ("DescribeSubnets", true),
            ("DescribeRouteTables", true),
            ("DescribeAvailabilityZones", true),
            ("DescribeInstances", true),
            ("CreateVpc", false),
            ("RunInstances", false),
            ("CreateTags", false),
            ("GetConsoleOutput", false),
            ("ModifyVpcAttribute", false),
        ];
        for (operation, expected) in rows {
            let decided = judged_over_every_resource(operation);
            assert_eq!(
                decided, *expected,
                "case {operation:?}: expected judged_over_every_resource {expected}, got \
                 {decided}"
            );
        }
    }
}
