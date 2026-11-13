// SPDX-License-Identifier: CC0-1.0

//! Whether the caller of an inference request may make it at all — the decision AWS takes
//! in front of this data plane, before any resource it names is looked up.
//!
//! Every operation of this service is an action of the `personalize` action namespace
//! (`smithy_query(query="service_info")` -> `awsService.arnNamespace`, which is also this
//! service's `sigv4Name`, so the prefix is read off the request's own resolved identity
//! rather than restated here) performed on a campaign or recommender ARN the REQUEST
//! names. AWS refuses a request that fails either half of that with HTTP 403
//! `AccessDeniedException` — a code this service's model declares no shape for, so it is
//! answered through the error channel that exists for exactly that (see
//! [`crate::scaffolded`]) — and the refusal is taken BEFORE the registry is consulted, so a
//! refused caller learns nothing about what exists.
//!
//! The two halves, in the order a refusal states them:
//!
//! 1. **The resource's account.** A resource ARN whose account segment is not the calling
//!    principal's own account names a resource no document of this account can grant over,
//!    and Personalize publishes no resource-based policy for a campaign, a recommender or a
//!    filter (its model binds no policy operation and no policy member), so no grant from
//!    the owning account can reach this caller either. Such a request is refused
//!    ([`::service_common::iam_authorization::not_authorized_to_access_resource`], the
//!    sentence that names the principal and NOTHING else: a caller must not learn from a
//!    denial what exists in an account that is not its own, not even which resource it
//!    asked about).
//! 2. **The principal's permissions.** What the caller may do comes from the identity-based
//!    policy documents the IAM store holds for the principal its credential names and the
//!    permissions boundary that caps them, read through the IAM service's OWN accessors
//!    (`principal_policies`, `permissions_boundary_document`), evaluated by the ONE shared
//!    reading of an IAM document ([`::service_common::iam_authorization`]): an explicit
//!    `Deny` wins, otherwise an `Allow` about the request grants it, and with neither the
//!    request falls to IAM's default deny.
//!
//! WHO the caller is comes from the same source `sts:GetCallerIdentity` answers from — the
//! STS session the request's access key id names, else the IAM user that owns the long-term
//! key, else the account's own published credential — so the principal a refusal blames is
//! the one the caller reads back for the credentials it signed with.
//!
//! Only the store LOOKUPS live here; the decision and both sentences do not. That is the
//! split every other gate in this tree makes (the in-tree precedents are
//! `services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/service/authorization.rs`
//! and
//! `services/service-com-amazonaws-kinesisvideosignaling-awsacuitysignalingservice/src/state/authorization.rs`),
//! and the lookups stay in this crate for the reason those keep theirs there: hoisting them
//! into `service-common` would make every service depending on that crate link the IAM and
//! STS service crates.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root credentials
//!   are in. Its ACTION is allowed; the resource account above still decides.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for falls to the default deny. (In the
//! assembled emulator the layer that authenticates a request refuses such a credential
//! before any handler runs.)
//!
//! What is NOT modelled here: a statement's `Condition` clauses. The decision is taken with
//! an empty request context, so a statement conditioned on a key this emulator does not
//! supply is read as the shared evaluator reads an undecided clause. Nothing here guesses a
//! condition value.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The resource IAM writes for a request that addresses no resource of this service: this
/// service's two ARN members are both optional on `GetRecommendations`, so a request naming
/// neither is still authorized — on the action alone.
pub const ANY_RESOURCE: &str = "*";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// Whether the caller of this request may perform `operation` on the resources it names,
/// and the sentence that says why not when it may not.
///
/// `operation` is the operation's own name (`Self::NAME` in a handler), which names the IAM
/// action `<signing name>:<operation>`. `resources` is every resource ARN the REQUEST names,
/// in any order: all of them have to belong to the caller's account, and the one the ACTION
/// is performed on is read out of them by resource type ([`action_resource`]). `None` when
/// the caller may make the call; otherwise the refusal's message, which the handler answers
/// in HTTP 403 `AccessDeniedException`.
///
/// Call it FIRST in a handler: before this service's store is locked, so the IAM and STS
/// stores are never reached while this service's guard is held, and before the request's own
/// values are checked, so a refused call reads nothing and reports nothing.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called, which is why a REPORTING operation may take this
/// decision.
pub fn authorization_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resources: &[&str],
) -> Option<String> {
    let caller = caller(engine, ctx);
    // The resource's account decides first, and its sentence names neither the action nor
    // the ARN (see the module docs): a reference into another account is refused whatever
    // the caller's own documents say, because nothing in this account grants over it.
    if resources
        .iter()
        .any(|resource| caller.addresses_another_account(resource))
    {
        return Some(iam::not_authorized_to_access_resource(&caller.arn));
    }
    // The action this request asks for, under the namespace it was signed in, on the
    // resource it is performed on.
    let action = format!("{}:{operation}", ctx.identity().signing_name());
    let resource = action_resource(resources);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and this service publishes no resource-based policy, so the decision
    // below rests on the action and the resource.
    let request = iam::Request::new(&action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // Taken FIRST, so an explicit `Deny` in a document that CAN be read refuses the request
    // whatever else the principal carries — including a grant this emulator cannot read,
    // which only lifts the DEFAULT deny of a request no readable statement allows.
    if identity_said == Decision::Denied {
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            &action,
            resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    if !(iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            &action,
            resource,
            &iam::no_identity_policy_allows(&action),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not
    // a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return None,
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(&action),
        };
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            &action,
            resource,
            &reason,
        ));
    }
    None
}

/// The resource the ACTION is performed on, among the ARNs a request names: the campaign or
/// recommender it recommends from.
///
/// Every action of this service is published for those two resource types alone — a filter
/// is a value the action READS, not a resource the action is performed on — so which ARN the
/// decision is taken against is read out of the resource type each value claims
/// ([`crate::state::ResourceKind`], the one place this crate spells those types). A request
/// that names neither is judged against [`ANY_RESOURCE`], as IAM writes a request that
/// addresses no resource of the service.
fn action_resource<'a>(resources: &'a [&'a str]) -> &'a str {
    let performed_on = [
        crate::state::ResourceKind::Campaign.as_str(),
        crate::state::ResourceKind::Recommender.as_str(),
    ];
    resources
        .iter()
        .copied()
        .find(|arn| {
            ::service_common::arn::resource_type(arn)
                .is_some_and(|kind| performed_on.contains(&kind))
        })
        .unwrap_or(ANY_RESOURCE)
}

/// The account the ARN `arn` names, or `None` when it carries none — read through this
/// tree's one parse of a caller-supplied resource ARN, so an account is never guessed out of
/// a value that is no ARN at all.
fn account_of(arn: &str) -> Option<&str> {
    ::service_common::arn::parse(arn)
        .map(|parsed| parsed.account)
        .filter(|account| !account.is_empty())
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// The account that principal belongs to, when its ARN carries one.
    account: Option<String>,
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
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Whether `resource` belongs to an account that is NOT this caller's.
    ///
    /// Both accounts have to be known for the comparison to say anything: a caller or a
    /// resource whose ARN carries no account is not treated as foreign — a value that names
    /// no account references no other account's resource, and what is wrong with it is
    /// reported where the request's own members are read.
    fn addresses_another_account(&self, resource: &str) -> bool {
        match (self.account.as_deref(), account_of(resource)) {
            (Some(principal), Some(owner)) => principal != owner,
            _ => false,
        }
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

    /// The caller a request speaking for nobody is judged as: an expired session credential,
    /// which carries no document and no principal spelling, so every action it asks for
    /// falls to IAM's default deny.
    fn speaks_for_nobody() -> Self {
        Self {
            arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
            account: None,
            principals: Vec::new(),
            documents: Vec::new(),
            boundary: None,
            account_credential: false,
            unreadable_attachment: false,
        }
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // The identity this emulator's own published credential speaks for, which is both the
    // answer for a key that names no session and what tells such a request apart from one
    // signed by a principal the stores hold.
    let ambient = sts_store::ambient_identity();
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(ambient.clone()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The key id the expired credential presented is NOT quoted back — this module
        // echoes no credential of the request.
        Err(_expired) => return Caller::speaks_for_nobody(),
    };

    let mut caller = Caller {
        account_credential: identity.arn == ambient.arn,
        principals: vec![identity.arn.clone()],
        account: account_of(&identity.arn).map(ToString::to_string),
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
            caller.account = account_of(&user.arn).map(ToString::to_string);
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
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

    /// The account every row's caller belongs to: the one this emulator serves.
    fn own_account() -> String {
        ::engine::emulator_account_id().to_string()
    }

    /// A caller speaking for an assumed-role session of `account`.
    fn session_caller(account: &str) -> Caller {
        let arn = format!("arn:aws:sts::{account}:assumed-role/inference-caller/session-1");
        Caller {
            account: account_of(&arn).map(ToString::to_string),
            arn,
            principals: Vec::new(),
            documents: Vec::new(),
            boundary: None,
            account_credential: false,
            unreadable_attachment: false,
        }
    }

    /// RULE: a resource ARN belongs to another account exactly when both accounts are known
    /// and differ; a value naming no account references no other account's resource.
    #[test]
    fn a_resource_is_foreign_only_when_both_accounts_are_known_and_differ() {
        let caller = session_caller(&own_account());
        let own = own_account();
        // The rows span what the rule generalises over: the resource type the ARN names, the
        // account it carries (the caller's own, another one, one differing in a single
        // digit, none at all) and values that are no resource ARN.
        let rows: Vec<(&str, String, bool)> = vec![
            (
                "a campaign of the caller's own account",
                format!("arn:aws:personalize:us-east-1:{own}:campaign/c"),
                false,
            ),
            (
                "a filter of the caller's own account",
                format!("arn:aws:personalize:eu-west-1:{own}:filter/f"),
                false,
            ),
            (
                "a recommender of another account",
                "arn:aws:personalize:us-west-2:123456789012:recommender/r".to_string(),
                true,
            ),
            (
                "a campaign of an account differing in one digit",
                "arn:aws:personalize:us-west-2:000000000001:campaign/c".to_string(),
                true,
            ),
            (
                "a campaign of another account in another partition",
                "arn:aws-cn:personalize:cn-north-1:210987654321:campaign/c".to_string(),
                true,
            ),
            (
                "an ARN carrying no account at all",
                "arn:aws:personalize:us-east-1::campaign/c".to_string(),
                false,
            ),
            (
                "a value that is no ARN at all",
                "campaign/c".to_string(),
                false,
            ),
        ];
        for (case, resource, expected) in rows {
            let actual = caller.addresses_another_account(&resource);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected foreign={expected}, got {actual} for {resource:?}"
            );
        }
    }

    /// RULE: the resource the ACTION is performed on is the campaign or recommender among
    /// the ARNs the request names, whatever order they arrive in; a request naming neither
    /// is judged against the resource IAM writes for one that addresses none.
    #[test]
    fn the_action_is_judged_against_the_campaign_or_recommender_the_request_names() {
        let own = own_account();
        let campaign = format!("arn:aws:personalize:us-east-1:{own}:campaign/c");
        let recommender = format!("arn:aws:personalize:us-east-1:{own}:recommender/r");
        let filter = format!("arn:aws:personalize:us-east-1:{own}:filter/f");
        // (case, the ARNs the request names, the resource the action is judged against)
        let rows: Vec<(&str, Vec<&str>, &str)> = vec![
            ("a campaign alone", vec![&campaign], &campaign),
            (
                "a campaign and the filters it reads",
                vec![&campaign, &filter, &filter],
                &campaign,
            ),
            (
                "a filter named before the campaign",
                vec![&filter, &campaign],
                &campaign,
            ),
            ("a recommender", vec![&recommender, &filter], &recommender),
            ("no resource at all", vec![], ANY_RESOURCE),
            ("filters alone", vec![&filter], ANY_RESOURCE),
            ("a value that is no ARN", vec!["campaign/c"], ANY_RESOURCE),
        ];
        for (case, resources, expected) in rows {
            let actual = action_resource(&resources);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a caller whose own ARN names no account treats no resource as foreign, because
    /// there is nothing to compare it with.
    #[test]
    fn a_caller_without_an_account_treats_no_resource_as_foreign() {
        let nobody = Caller::speaks_for_nobody();
        for resource in [
            "arn:aws:personalize:us-east-1:123456789012:campaign/c",
            "arn:aws:personalize:us-east-1:000000000000:campaign/c",
        ] {
            assert!(
                !nobody.addresses_another_account(resource),
                "case {resource:?}: a caller speaking for nobody compares no accounts"
            );
        }
    }
}
