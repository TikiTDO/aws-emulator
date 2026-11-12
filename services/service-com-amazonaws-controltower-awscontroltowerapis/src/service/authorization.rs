// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step an operation takes BEFORE it reads or writes anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at the
//! request's own input, so a caller whose identity-based policies do not grant the action
//! is told it is not authorized rather than told anything about the resource it named, and
//! the call performs nothing. Every operation of this service whose model declares
//! `AccessDeniedException` (HTTP 403) takes that decision; an operation whose model
//! declares no 403 is NOT judged here, because answering a caller an error its operation
//! does not model would be a different divergence.
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
//!    where the collection rule itself lives — so a policy `iam:PutRolePolicy` wrote takes
//!    effect on the next request, deleting it lifts it again, and this service reads an
//!    identity's permissions exactly as IAM's own authorizer does;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny. The refusal's SENTENCE comes from that same module, so two
//!    services that reach one decision word it one way.
//!
//! Only the store LOOKUPS live here; neither the decision, nor its wording, nor the
//! readings of an identity ARN do. That is the same split every other gate in this tree
//! makes (Amazon Textract, AWS Certificate Manager, AWS Application Auto Scaling), and the
//! lookups stay in the service crate for the reason those keep theirs there: hoisting them
//! into `service-common` would make every service that depends on that crate link the IAM
//! and STS service crates.
//!
//! ## What the decision is made about
//!
//! The ACTION is `controltower:<OperationName>` ([`ACTION_PREFIX`]): this service's
//! `arnNamespace` in front of the operation the request named.
//!
//! The RESOURCE is the one the request ADDRESSES — the ARN it names, when it names one of
//! this service's ([`Addressed`]) — and otherwise the COLLECTION of the resource type the
//! operation acts on, `<type>/*` in the Region the caller signed for and this account,
//! which is what IAM evaluates for a call that addresses no single resource (a create, a
//! listing, a report over a whole resource type): the id such a call's resource would get
//! is no value a policy could have been written against. Every collection comes from the
//! same ARN builders this service mints resource ARNs with, so a policy scoped to this
//! account's resources covers it.
//!
//! Whether the identifier a request carries names a resource of a given type is read off
//! the ARN itself — the tree's one ARN parse — rather than through this crate's
//! `state::is_*_arn` form checks: those decide whether the REQUEST is acceptable, which is
//! the handler's own refusal and happens after this decision, while this asks only which
//! resource the request addresses.
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
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// This service's IAM action prefix: an action is `controltower:<OperationName>` ("Actions
/// defined by AWS Control Tower", IAM Service Authorization Reference), which is this
/// service's `arnNamespace` (`smithy_query(query="service_info")`) — the same string
/// [`crate::state::ARN_NAMESPACE`] spells into every ARN this service builds.
pub const ACTION_PREFIX: &str = crate::state::ARN_NAMESPACE;

/// The resource segment every resource of a collection is covered by, as IAM writes it:
/// any id of the type.
const ANY_ID: &str = "*";

/// A class of resource this service's operations address, which is what an operation is
/// judged against when it names no single resource of its own.
///
/// One variant per resource type this service mints ARNs for, so the collection an
/// operation falls back to is built by the SAME builder as the ARNs of that type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addressed {
    /// A landing zone, and the asynchronous operations over one.
    LandingZone,
    /// An enabled control, and the asynchronous operations over one.
    EnabledControl,
    /// An enabled baseline, and the asynchronous operations over one.
    EnabledBaseline,
    /// A baseline of the published catalog.
    Baseline,
}

impl Addressed {
    /// The resource TYPE this class is written with in an ARN's resource field.
    #[must_use]
    fn resource_type(self) -> &'static str {
        match self {
            Self::LandingZone => "landingzone",
            Self::EnabledControl => "enabledcontrol",
            Self::EnabledBaseline => "enabledbaseline",
            Self::Baseline => "baseline",
        }
    }

    /// Every resource of this class in `region`, as an ARN of the same form this service
    /// mints for one of them.
    #[must_use]
    fn collection(self, region: &str) -> String {
        match self {
            Self::LandingZone => crate::state::landing_zone_arn(region, ANY_ID),
            Self::EnabledControl => crate::state::enabled_control_arn(region, ANY_ID),
            Self::EnabledBaseline => crate::state::enabled_baseline_arn(region, ANY_ID),
            Self::Baseline => crate::state::baseline_arn(region, ANY_ID),
        }
    }

    /// Whether `identifier` is an ARN addressing a resource of this class: an ARN of this
    /// service's namespace whose resource type is this one.
    ///
    /// Read through the tree's one ARN parse ([`::service_common::arn`]), so nothing about
    /// the id itself is required — the question is which resource the request addresses,
    /// not whether the request is acceptable.
    #[must_use]
    fn addressed_by(self, identifier: &str) -> bool {
        ::service_common::arn::parse(identifier).is_some_and(|arn| {
            arn.vendor == crate::state::ARN_NAMESPACE && arn.resource_type() == self.resource_type()
        })
    }
}

/// The resource IAM judges this request against: the ARN it names when that ARN addresses
/// a resource of `class`, and otherwise every resource of `class` in the Region the caller
/// signed for.
///
/// The identifier is taken as the request CARRIED it rather than resolved from a stored
/// record, so the decision does not depend on whether that resource exists — which is also
/// what keeps a refused call from telling an unauthorized caller whether it does.
#[must_use]
fn resource(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    class: Addressed,
    identifier: Option<&str>,
) -> String {
    match identifier.filter(|identifier| class.addressed_by(identifier)) {
        Some(addressed) => addressed.to_string(),
        None => class.collection(&crate::state::request_region(ctx.parts())),
    }
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]), `class` the kind of resource the
/// operation acts on and `identifier` the one its request NAMES, when it names one.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise, which each operation returns as its
/// own `AccessDeniedException`. Call it BEFORE the store is read or written. The resource
/// the decision is made against is derived here ([`resource`]) rather than by the caller,
/// so no operation can judge itself against a resource of another form.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called, which is why a REPORTING operation of this
/// service may take this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    class: Addressed,
    identifier: Option<&str>,
) -> Result<(), AccessDeniedException> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let resource = resource(ctx, class, identifier);
    let resource = resource.as_str();
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and AWS Control Tower publishes no resource-based policy of its own —
    // its model declares no policy operation and no policy member — so the decision below
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

/// The refusal IAM states an authorization failure with, as this service's modelled
/// `AccessDeniedException` (HTTP 403): who was refused, which action, on which resource,
/// and which of IAM's rules refused it.
///
/// The sentence is the shared one ([`iam::not_authorized_by_identity_policy`]), and every
/// value in it comes from the request and from the stored policies — the principal from the
/// credential the request presented, the action from the operation it named, the resource
/// from the ARN it addressed.
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

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `controltower:<OperationName>`, so a policy
    /// scoped to this service's actions covers every operation of it and nothing outside
    /// it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"controltower:*","Resource":"*"}]}"#;
        for operation in [
            "ListLandingZones",
            "CreateLandingZone",
            "EnableControl",
            "GetEnabledBaseline",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(document, &action, "*");
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected controltower:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "s3:GetObject", "*");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected controltower:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: an identifier addresses a resource of a class when it is an ARN of this
    /// service's namespace whose resource type is that class's — whatever the id, the
    /// Region or the account it carries — and nothing else does.
    #[test]
    fn an_identifier_addresses_the_class_its_resource_type_names() {
        let account = ::engine::emulator_account_id();
        // (case, the identifier, the class it addresses)
        let rows: Vec<(&str, String, Option<Addressed>)> = vec![
            (
                "a landing zone ARN",
                crate::state::landing_zone_arn("us-east-1", "1a2b3c"),
                Some(Addressed::LandingZone),
            ),
            (
                "an enabled control ARN",
                crate::state::enabled_control_arn("eu-west-1", "9z8y7x"),
                Some(Addressed::EnabledControl),
            ),
            (
                "an enabled baseline ARN",
                crate::state::enabled_baseline_arn("ap-south-1", "4d5e6f"),
                Some(Addressed::EnabledBaseline),
            ),
            (
                "a baseline ARN, which carries no account",
                crate::state::baseline_arn("us-west-2", "17BSJV3IGJ2QSGA2"),
                Some(Addressed::Baseline),
            ),
            (
                "an ARN of another service",
                "arn:aws:s3:::landingzone/1a2b3c".to_string(),
                None,
            ),
            (
                "an ARN of this service naming another resource type",
                format!("arn:aws:controltower:us-east-1:{account}:control/AWS-GR_ONE"),
                None,
            ),
            ("a value that is no ARN at all", "1a2b3c".to_string(), None),
        ];

        let classes = [
            Addressed::LandingZone,
            Addressed::EnabledControl,
            Addressed::EnabledBaseline,
            Addressed::Baseline,
        ];
        for (case, identifier, addressed) in rows {
            for class in classes {
                let actual = class.addressed_by(&identifier);
                let expected = addressed == Some(class);
                assert_eq!(
                    actual, expected,
                    "case {case:?} against {class:?}: expected addressed_by == {expected}, got \
                     {actual}"
                );
            }
        }
    }

    /// RULE: a collection is every resource of its type in the caller's Region, written by
    /// the same ARN builder this service mints one of them with — so a policy scoped to
    /// this account's resources covers it, and the collection itself is addressed by its
    /// own class.
    #[test]
    fn a_collection_is_every_resource_of_its_type_in_the_region() {
        let region = "eu-central-1";
        // (the class, the resource segment its collection carries)
        let rows = [
            (Addressed::LandingZone, "landingzone/*"),
            (Addressed::EnabledControl, "enabledcontrol/*"),
            (Addressed::EnabledBaseline, "enabledbaseline/*"),
            (Addressed::Baseline, "baseline/*"),
        ];
        for (class, segment) in rows {
            let collection = class.collection(region);
            assert!(
                collection.ends_with(segment),
                "case {class:?}: expected a collection ending in {segment}, got {collection:?}"
            );
            assert!(
                collection.contains(&format!(":{region}:")),
                "case {class:?}: expected the caller's Region in the collection, got \
                 {collection:?}"
            );
            assert!(
                class.addressed_by(&collection),
                "case {class:?}: a collection of a class is addressed by that class, got \
                 {collection:?}"
            );
        }
    }
}
