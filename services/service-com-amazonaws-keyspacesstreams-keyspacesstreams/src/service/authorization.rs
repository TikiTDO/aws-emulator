// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! decision every operation takes BEFORE it serves anything out of the stream store.
//!
//! `AccessDeniedException` (HTTP 403) is a SERVICE-COMMON error of this model
//! (`smithy_query(query="service_info")` lists it under `common_errors`), so it is the
//! answer every operation here gives a caller whose permissions do not cover the action it
//! named — including the data-plane read, which is the one an unauthorized caller gets
//! change data out of. AWS takes that decision at the IAM layer, ahead of the service
//! looking at the stream the request named, so the refusal says nothing about whether that
//! stream exists.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same record
//!    `sts:GetCallerIdentity` answers from), else the IAM user that owns the long-term
//!    key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's own accessors (`principal_policies`, `permissions_boundary_document`), so
//!    a document `iam:PutUserPolicy` wrote takes effect on the next request and deleting
//!    it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny. The refusal SENTENCE and its four reason clauses come from
//!    that same module, so this gate words a denial exactly as every other one does.
//!
//! Only the store LOOKUPS live here; neither the decision nor the reading of a principal
//! ARN does. That is the split every other gate in this tree makes, and the lookups stay
//! in this crate for the reason those keep theirs there: hoisting them into
//! `service-common` would make every service depending on that crate link the IAM and STS
//! service crates.
//!
//! ## The action and the resource
//!
//! The action is `<arn namespace>:<OperationName>` — this service's `arnNamespace`, which
//! is also its signing name ([`action_prefix`]) — so one policy statement scoped to this
//! service's actions covers every operation of it and nothing outside it.
//!
//! The resource is the STREAM the request addresses, as the Amazon Keyspaces stream ARN
//! `arn:aws:cassandra:<region>:<account>:/keyspace/<ks>/table/<tbl>/stream/<label>` — the
//! ARN the request itself carries (`streamArn`, or the stream a registered `shardIterator`
//! bookmarks), never one resolved out of a stored record, so a refused call tells an
//! unauthorized caller nothing about what the store holds. An operation that enumerates a
//! SET of streams rather than addressing one is judged against that set
//! ([`stream_set_resource`]): the stream ARN of the keyspace/table scope its filters name,
//! with `*` where a filter leaves the segment open.
//!
//! This service publishes no resource-based policy of its own — the model declares no
//! policy operation and no policy member — so the decision here is the identity-based one
//! alone.
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

/// The segment an enumeration's resource ARN carries where its filters leave the
/// keyspace or the table open: IAM's own "any run of characters" wildcard, which is how a
/// caller writes the stream scope of a whole table or keyspace in a policy.
const ANY_SEGMENT: &str = "*";

/// This service's IAM action prefix, which is its `arnNamespace`
/// (`smithy_query(query="service_info")`) and also the name every request to it is signed
/// under — so the prefix is read off the request's own resolved identity rather than
/// restated here, and the ARNs this gate builds and the actions it asks about cannot name
/// two different services.
#[must_use]
pub fn action_prefix<'ctx>(ctx: &'ctx ::smithy_server_runtime::OperationContext<'_>) -> &'ctx str {
    ctx.identity().signing_name()
}

/// The ARN of the SET of streams an enumeration addresses: the stream ARN of the
/// keyspace/table scope its filters name, with [`ANY_SEGMENT`] where a filter leaves the
/// segment open.
///
/// `ListStreams` names no single stream — "Returns an array of stream identifiers" for the
/// keyspace and table its filters select — so the resource it is judged against is that
/// selection, in the Region the request was signed for. Both filters come from the request,
/// so nothing of the store reaches the decision here either.
#[must_use]
pub fn stream_set_resource(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    keyspace_name: Option<&str>,
    table_name: Option<&str>,
) -> String {
    let keyspace = keyspace_name.unwrap_or(ANY_SEGMENT);
    let table = table_name.unwrap_or(ANY_SEGMENT);
    ::engine::emulator_arn(
        action_prefix(ctx),
        crate::state::request_region(ctx.parts())
            .as_deref()
            .unwrap_or_default(),
        &format!("/keyspace/{keyspace}/table/{table}/stream/{ANY_SEGMENT}"),
    )
}

/// Authorize this request against `resource`, where `operation` is the operation's own
/// name ([`smithy_server_runtime::Operation::NAME`]).
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise, which each operation lifts into its
/// own error channel. Call it FIRST in a handler, before the stream store is read.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which materialises the
/// session expiries the clock has already made due; that write is owed at the same instant
/// whether or not anybody called, which is why a REPORTING operation of this service may
/// take this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{}:{operation}", action_prefix(ctx));
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

/// The modelled refusal carrying the sentence IAM states an authorization failure with: who
/// was refused, which action, on which resource, and which of IAM's rules refused it.
///
/// The sentence itself is [`::service_common::iam_authorization::not_authorized_by_identity_policy`],
/// the one wording every gate in this tree states a denial with; this function only lifts it
/// into the error shape this service's model declares. Every value in it comes from the
/// request and from the stored policies — the principal from the credential the request
/// presented, the action from the operation it named, the resource from the stream it
/// addressed.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(iam::not_authorized_by_identity_policy(
            principal, action, resource, reason,
        )),
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

    /// RULE: the action this gate asks about carries this service's own prefix, so a
    /// statement scoped to this service's actions covers every operation of it and nothing
    /// outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"cassandra:*","Resource":"*"}]}"#;
        for operation in ["GetRecords", "GetShardIterator", "GetStream", "ListStreams"] {
            let action = format!("cassandra:{operation}");
            let reached = decision(document, &action, "*");
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected the service-wide allow to cover {action}, got \
                 {reached:?}"
            );
        }
        let outside = decision(document, "dynamodb:GetRecords", "*");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected the service-wide allow to grant nothing outside \
             this service, got {outside:?}"
        );
    }

    /// RULE: a grant scoped to one table's streams covers that table's stream and no
    /// other, which is how a caller writes a CDC read policy.
    #[test]
    fn a_grant_scoped_to_a_tables_streams_covers_that_tables_stream() {
        let account = ::engine::emulator_account_id();
        let document = format!(
            r#"{{"Statement":[{{"Effect":"Allow","Action":"cassandra:GetRecords",
             "Resource":"arn:aws:cassandra:us-east-1:{account}:/keyspace/shop/table/orders/stream/*"}}]}}"#
        );
        // (case, the stream ARN a request addresses, whether the grant covers it)
        let rows = [
            ("the granted table's stream", "/keyspace/shop/table/orders/stream/2026-10-30T07:59:02.000", true),
            ("another table of the keyspace", "/keyspace/shop/table/carts/stream/2026-10-30T07:59:02.000", false),
            ("another keyspace's table", "/keyspace/warehouse/table/orders/stream/2026-10-30T07:59:02.000", false),
        ];
        for (case, resource, covered) in rows {
            let arn = ::engine::emulator_arn("cassandra", "us-east-1", resource);
            let reached = decision(&document, "cassandra:GetRecords", &arn);
            assert_eq!(
                reached == Decision::Allowed,
                covered,
                "case {case:?}: expected the grant to {}cover {arn}, got {reached:?}",
                if covered { "" } else { "not " }
            );
        }
    }

    /// RULE: the refusal reaching this service's error shape carries IAM's whole sentence
    /// — the principal, the action, the resource the request addressed and which of IAM's
    /// rules refused it — whichever of the four reasons the decision rested on.
    ///
    /// The expectation is SPELLED OUT here rather than built from the same helpers the
    /// implementation calls, so a change to either half of the wording fails this test.
    #[test]
    fn the_refusal_states_iams_whole_sentence() {
        let principal = format!(
            "arn:aws:iam::{}:user/stream-reader",
            ::engine::emulator_account_id()
        );
        let action = "cassandra:GetRecords";
        let resource = ::engine::emulator_arn(
            "cassandra",
            "us-east-1",
            "/keyspace/shop/table/orders/stream/2026-10-30T07:59:02.000",
        );
        // (case, the reason the decision rested on, the clause IAM closes with)
        let rows = [
            (
                "a default deny",
                iam::no_identity_policy_allows(action),
                format!("because no identity-based policy allows the {action} action"),
            ),
            (
                "an explicit deny in an identity-based policy",
                iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string(),
                "with an explicit deny in an identity-based policy".to_string(),
            ),
            (
                "an explicit deny in a permissions boundary",
                iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
                "with an explicit deny in a permissions boundary".to_string(),
            ),
            (
                "a grant no permissions boundary allows",
                iam::no_permissions_boundary_allows(action),
                format!("because no permissions boundary allows the {action} action"),
            ),
        ];
        for (case, reason, clause) in rows {
            let refusal = not_authorized(&principal, action, &resource, &reason);
            assert_eq!(
                refusal.message,
                Some(format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {resource} {clause}"
                )),
                "case {case:?}: expected IAM's wording, got {:?}",
                refusal.message
            );
        }
    }
}
