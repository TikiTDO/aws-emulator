// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for, decided
//! BEFORE the operation reads or writes anything.
//!
//! Athena publishes its workgroups as an IAM RESOURCE, and the operations that act on one
//! document the requirement in their own words — "Requires you to have access to the
//! workgroup in which the query ran" (`StartQueryExecution`), "Requires that you have
//! access to the workgroup" (`CreateNamedQuery`). So a caller whose identity-based policies
//! grant an Athena action only on one workgroup's ARN may perform it there and nowhere
//! else: the identical request naming another workgroup is refused, and the refusal is a
//! property of the (principal, action, resource) triple rather than of anything the
//! workgroup holds.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same record
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one the
//!    caller reads back for the credentials it signed with), else the IAM user that owns the
//!    long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), so a document
//!    `iam:PutUserPolicy` wrote takes effect on the next request and deleting it lifts it
//!    again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike: an explicit `Deny` wins, otherwise an `Allow`
//!    about the request grants it, and with neither the request falls to IAM's default deny.
//!
//! How the refusal is WORDED is Athena's own ([`not_authorized`]): it names the action the
//! caller asked for and nothing else — not the principal, not the resource, and not which of
//! IAM's rules refused — and closes with the sentence that tells the caller what to do about
//! it. So the decision is shared and the wording is not.
//!
//! Only the store LOOKUPS and this service's own wording live here; the decision does not.
//! That is the split every other gate in this tree makes, and the lookups stay in this crate
//! for the reason those keep theirs there: hoisting them into `service-common` would make
//! every service that depends on that crate link the IAM and STS service crates. WHICH IAM
//! entity a principal ARN names carries no such dependency, so it is read through the shared
//! [`::service_common::iam_role`] rather than parsed again here.
//!
//! ## The action and the resource
//!
//! The action is `athena:<OperationName>` — this service's `arnNamespace`, which is also its
//! `sigv4Name` (`smithy_query(query="service_info")`) — so one statement scoped to this
//! service's actions covers every operation of it and nothing outside it.
//!
//! The resource is the workgroup the REQUEST names ([`work_group_arn`]), spelled as the
//! request spelled it, in the Region the call was signed for. Taking it from the request
//! rather than from a stored record is what keeps the decision independent of whether that
//! workgroup exists — which is also what keeps a refused call from telling an unauthorized
//! caller whether it does. An operation whose workgroup member is optional addresses the
//! account's default workgroup ([`crate::state::DEFAULT_WORK_GROUP`]) when the request names
//! none, because that is the workgroup the call would act in.
//!
//! Athena publishes no resource-based policy of its own — the model declares no policy
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

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::scaffolded::AccessDenied;

/// This service's IAM action prefix, and the namespace every Athena ARN carries: its
/// `arnNamespace`, which is also its `sigv4Name` (`smithy_query(query="service_info")`).
pub const ACTION_PREFIX: &str = "athena";

/// The resource type an Athena workgroup is addressed by in an ARN, from the "Resource
/// types defined by Amazon Athena" table of the IAM Service Authorization Reference:
/// `arn:${Partition}:athena:${Region}:${Account}:workgroup/${WorkGroupName}`.
const WORK_GROUP_RESOURCE_TYPE: &str = "workgroup";

/// The Region an ARN this gate builds carries when the request carries no SigV4 credential
/// scope to read one from: the Region an AWS SDK falls back to when none is configured.
const DEFAULT_REGION: &str = "us-east-1";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The Region the ARN a request is judged against carries: the Region of the caller's SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request carries none.
///
/// The scope is parsed by `emulator_server::sigv4_credential_scope`, the single source of
/// SigV4-scope parsing, so a header that is not a well-formed SigV4 `Authorization` yields
/// the default rather than a bogus Region.
#[must_use]
pub fn region_from_request(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the workgroup `work_group` names, in the Region the call was signed for.
///
/// Built through [`::engine::emulator_arn`], so the account segment is the one canonical
/// account id every generated emulator presents.
#[must_use]
pub fn work_group_arn(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    work_group: &str,
) -> String {
    ::engine::emulator_arn(
        ACTION_PREFIX,
        &region_from_request(ctx.parts()),
        &format!("{WORK_GROUP_RESOURCE_TYPE}/{work_group}"),
    )
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the ARN the request NAMES
/// ([`work_group_arn`]).
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the refusal otherwise, which each operation answers as its own
/// `AccessDeniedException`. Call it BEFORE the store is read or written, and with the ARN
/// built from what the request carried rather than from a stored record.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called, which is why a REPORTING operation of this service
/// takes this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDenied> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and Athena publishes no resource-based policy (see the module docs), so
    // the decision below rests on the action and the resource.
    let request = iam::Request::new(&action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(&action));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(&action));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not
    // a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        if iam::identity_decision(::std::slice::from_ref(boundary), &request) != Decision::Allowed {
            return Err(not_authorized(&action));
        }
    }
    Ok(())
}

/// The refusal Athena states an authorization failure with: the action the caller asked for,
/// and what to do about it.
///
/// FIXED text either side of the action, which is the only part that comes from the request.
/// Athena names NOTHING else in it — not the calling principal, not the resource the request
/// addressed, and not which of IAM's rules refused — so a refusal discloses neither what the
/// caller's policies say nor whether the resource it named exists.
fn not_authorized(action: &str) -> AccessDenied {
    AccessDenied::new(format!(
        "You are not authorized to perform: {action} on the resource. After your AWS \
         administrator or you have updated your permissions, please try again."
    ))
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — which IAM entity's
    /// policies are the ones in force is read off it.
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
        Err(_) => {
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

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `athena:<OperationName>`, so a policy scoped
    /// to this service's actions covers every operation of it and nothing outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"athena:*","Resource":"*"}]}"#;
        for operation in [
            "StartQueryExecution",
            "ListQueryExecutions",
            "CreateWorkGroup",
            "GetWorkGroup",
            "UpdateWorkGroup",
            "CreatePreparedStatement",
            "ImportNotebook",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(document, &action, "*");
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected athena:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "glue:GetTable", "*");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected athena:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: a workgroup's resource portion is `workgroup/<WorkGroupName>`, so the resource a
    /// decision is taken on is the workgroup the request named.
    #[test]
    fn a_work_groups_resource_portion_names_the_requested_work_group() {
        for work_group in ["primary", "analytics-team", "wg.one_2-3"] {
            let arn = ::engine::emulator_arn(
                ACTION_PREFIX,
                DEFAULT_REGION,
                &format!("{WORK_GROUP_RESOURCE_TYPE}/{work_group}"),
            );
            assert_eq!(
                arn,
                format!(
                    "arn:aws:athena:{DEFAULT_REGION}:{}:workgroup/{work_group}",
                    ::engine::emulator_account_id()
                ),
                "case {work_group:?}: expected this workgroup's ARN, got {arn:?}"
            );
        }
    }

    /// RULE: the refusal names the action the caller asked for, between Athena's own fixed
    /// text, and nothing else of the request or of the policies that refused it.
    #[test]
    fn the_refusal_names_the_action_and_nothing_else() {
        let principal = "arn:aws:iam::000000000000:user/analyst";
        let work_group = "analytics-team";
        for operation in ["StartQueryExecution", "GetWorkGroup"] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let refusal = not_authorized(&action);
            let stated = format!(
                "You are not authorized to perform: {action} on the resource. After your AWS \
                 administrator or you have updated your permissions, please try again."
            );
            assert_eq!(
                refusal.message, stated,
                "case {operation:?}: expected Athena's wording {stated:?}, got {:?}",
                refusal.message
            );
            assert!(
                !refusal.message.contains(principal) && !refusal.message.contains(work_group),
                "case {operation:?}: a refusal names neither the principal nor the resource, \
                 got {:?}",
                refusal.message
            );
        }
    }
}
