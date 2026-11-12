// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER's own identity-based IAM policies let a request to this read plane
//! perform the action it asks for — the decision every operation takes BEFORE it resolves
//! the stream the request names.
//!
//! A signed call is decided by IAM ahead of the service, so a principal whose policies do
//! not allow the action is told it is not authorized rather than told anything about the
//! stream it addressed: whether that stream exists, what it retains and what it holds are
//! all answers this refusal withholds. That is what keeps a MISSING GRANT distinguishable
//! from this service's own verdicts — the first is HTTP 403 `AccessDeniedException`, the
//! second one of the modelled refusals (`ResourceNotFoundException`,
//! `NoDataRetentionException`, `InvalidArgumentException`).
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from ([`sts_store::resolve_caller`]) for a
//!    session credential, and from IAM's own `access_keys` map (the map `iam:GetUser`
//!    resolves a caller through) for a long-term key, so the principal a refusal blames is
//!    the one the caller reads back for the credentials it signed with;
//! 2. **what it may do** — the identity-based policy documents in force on that principal
//!    and the permissions boundary that caps them, read through the IAM service's OWN
//!    accessors (`principal_policies`, `permissions_boundary_document`), so a document
//!    `iam:PutUserPolicy` wrote takes effect on the next call and deleting it lifts it
//!    again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared evaluation
//!    of those documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does. The
//! lookups stay in this service crate because the IAM service crate depends on
//! `service-common`, so the shared crate cannot read IAM's store without a dependency
//! cycle. WHICH IAM entity a principal ARN names carries no such dependency, so it is read
//! through the shared [`::service_common::iam_role`] rather than parsed again here.
//!
//! ## The action and the resource
//!
//! The action is `kinesisvideo:<OperationName>` ([`action_for`]): this endpoint's actions
//! are published under the Kinesis Video Streams namespace it signs and is addressed by
//! (`aws.api#service` `arnNamespace`/`endpointPrefix` and `aws.auth#sigv4` `name`, all
//! `kinesisvideo` — `smithy_query(query="service_info")`), which is why a policy written
//! for the control plane's `kinesisvideo:*` also covers these reads.
//!
//! The resource an IAM statement names for one of these calls is the STREAM's ARN, the
//! only ARN form this namespace publishes for a stream, so a request that addresses its
//! stream by `StreamName` is decided against the ARN the control plane holds that stream
//! under ([`stream_resource`]). An identifier the control plane does not hold is decided
//! AS CARRIED: there is no ARN to map it to, so no statement scoped to a stream ARN covers
//! it, while a statement over every resource still does — and the caller learns nothing
//! from the verdict about whether that stream exists, since the decision is taken before
//! any handler resolves it.
//!
//! ## The two callers the decision cannot refuse
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to [`sts_store::ambient_identity`], which is not a
//!   principal the IAM store holds policies for — the position AWS account root credentials
//!   are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to the account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//!
//! An expired session credential speaks for nobody: it carries no document, so every action
//! it asks for is refused, and the refusal names that state rather than the key it
//! presented, which this module never quotes back.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::state as control_plane;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The action namespace this endpoint's operations are published under, which is also its
/// SigV4 signing name and its ARN namespace: a policy statement spells one of them
/// `kinesisvideo:<OperationName>`.
pub const ACTION_PREFIX: &str = "kinesisvideo";

/// The IAM action `operation` is performed under: `kinesisvideo:<OperationName>`.
#[must_use]
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The refusal a request to `operation` addressing the stream `stream_name`/`stream_arn`
/// names gets when the calling principal's IAM policies do not allow it, and `None` when
/// they do.
///
/// IAM's order: an explicit `Deny` in a document that CAN be read refuses the request
/// whatever else the principal carries; otherwise a grant is needed, which a caller this
/// gate does not bound has by definition (see the module docs); and a grant is capped by
/// the permissions boundary, since "a permissions boundary sets the maximum permissions
/// that an identity-based policy can grant" (IAM, permissions boundaries).
///
/// Asked with NO store guard of this service held, and before the stream the request names
/// is resolved.
#[must_use]
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    stream_name: Option<&str>,
    stream_arn: Option<&str>,
) -> Option<String> {
    let action = action_for(operation);
    let resource = stream_resource(engine, stream_name, stream_arn);
    let caller = caller(engine, ctx);
    // An identity-based document states no `Principal` element, so the decision rests on
    // the action and the resource the request addressed.
    let request = iam::Request::new(&action, &resource, &[]);
    if allows(&caller, &request) {
        return None;
    }
    // The stream as the REQUEST named it, which is what the refusal quotes back — the ARN
    // the decision was taken against is this emulator's reading of that identifier, not
    // something the caller sent.
    let stated = stream_name.or(stream_arn).unwrap_or_default();
    Some(not_authorized(&caller.arn, &action, stated))
}

/// Whether IAM allows `request` for `caller`, in IAM's own evaluation order: an explicit
/// `Deny` in a document that CAN be read refuses it whatever else the principal carries;
/// otherwise a grant is needed, which a caller this gate does not bound has by definition
/// (see the module docs); and a grant is capped by the permissions boundary, since "a
/// permissions boundary sets the maximum permissions that an identity-based policy can
/// grant" (IAM, permissions boundaries).
fn allows(caller: &Caller, request: &iam::Request<'_>) -> bool {
    let said = iam::identity_decision(&caller.documents, request);
    if said == Decision::Denied {
        return false;
    }
    if !(iam::is_allowed(said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return false;
    }
    caller.boundary.as_ref().is_none_or(|boundary| {
        iam::identity_decision(::std::slice::from_ref(boundary), request) == Decision::Allowed
    })
}

/// The authorization refusal of this request as any operation's error channel carries it —
/// HTTP 403 `AccessDeniedException` with the sentence [`refusal`] states — or `None` when
/// the calling principal's policies allow the call.
///
/// The one form the six operations ask the gate in, so no handler words the refusal or
/// chooses its status and code for itself.
#[must_use]
pub fn refused<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    stream_name: Option<&str>,
    stream_arn: Option<&str>,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    refusal(engine, ctx, operation, stream_name, stream_arn)
        .map(crate::scaffolded::Scaffolded::access_denied)
}

/// The sentence this endpoint refuses an unauthorized call with: the principal that signed,
/// the action it asked for and the stream it named, each as the request and the signing
/// identity carry them.
///
/// It closes at the resource. IAM's own sentence — the shared
/// [`::service_common::iam_authorization::not_authorized_by_identity_policy`] — goes on to
/// state WHICH of IAM's rules withheld the grant; the Kinesis Video Streams read plane
/// words the same refusal without that clause, so the three fixed fragments are all of its
/// fixed text and every value in it comes from the request and the signing identity.
fn not_authorized(principal: &str, action: &str, resource: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: {resource}")
}

/// The resource the decision for a request addressing `stream_name`/`stream_arn` is taken
/// against: the ARN the Kinesis Video Streams control plane holds that stream under, and
/// otherwise the identifier the request carried.
///
/// The ARN is what an IAM statement names a stream by, so a request addressing its stream
/// by name is decided against the same ARN a statement scoped to that stream states. The
/// registry is read without this service's own store guard held and nothing here writes to
/// it.
fn stream_resource<Eng: Send + Sync + 'static>(
    engine: &Eng,
    stream_name: Option<&str>,
    stream_arn: Option<&str>,
) -> String {
    let carried = stream_name.or(stream_arn).unwrap_or_default();
    let registered = control_plane::service_state(engine).and_then(|registry| {
        let registry = registry.lock();
        registry
            .resolve(stream_name, stream_arn)
            .ok()
            .map(|stream| stream.stream_arn.clone())
    });
    registered.unwrap_or_else(|| carried.to_string())
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
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

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's OWN
    /// accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
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
///
/// Resolving the caller goes through STS's own `resolve_caller`, which materialises the
/// session expiries the clock has already made due before it answers: that write is owed at
/// the same instant whether or not anybody called, which is why a reporting operation of
/// this service may take this decision too.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::emulator_server::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential, which holds no identity policy.
        None => Ok(sts_store::ambient_identity()),
    };
    // An expired session credential speaks for nobody: it carries no document, so every
    // action it asks for is refused — and the refusal names the credential's STATE rather
    // than the key id that was presented, which this module never quotes back.
    let identity = match identity {
        Ok(identity) => identity,
        Err(_) => {
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                ..Caller::default()
            }
        }
    };
    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        arn: identity.arn,
        ..Caller::default()
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (the map `iam:GetUser` resolves a caller through), so a request
    // signed with one speaks for THAT user and is judged on the user's policies. A key the
    // IAM store does not hold either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
        }
    }
    // Which IAM identity the principal's policies are held under is read through the tree's
    // shared readings of a principal ARN.
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(role_arn) = store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone())
        {
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
    use super::{action_for, iam, Decision};

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// A stream ARN of this emulator's own account, in the form the control plane mints:
    /// `arn:aws:kinesisvideo:<region>:<account>:stream/<name>/<creation epoch seconds>`.
    fn stream_arn(region: &str, name: &str, created: u64) -> String {
        ::engine::emulator_arn("kinesisvideo", region, &format!("stream/{name}/{created}"))
    }

    /// RULE: the action this gate asks about carries the Kinesis Video Streams namespace,
    /// so a document written over that namespace covers these reads and one written over
    /// another service's covers none of them.
    #[test]
    fn the_action_asked_about_carries_the_kinesis_video_namespace() {
        let resource = stream_arn("us-east-1", "sensor-feed", 1_761_000_000);
        let action = action_for("GetImages");
        assert_eq!(
            action, "kinesisvideo:GetImages",
            "case \"the action spelling\": expected the kinesisvideo namespace, got {action:?}"
        );
        // (case, the document in force, what it says about this action)
        let rows: &[(&str, &str, Decision)] = &[
            (
                "every action of this namespace",
                r#"{"Statement":[{"Effect":"Allow","Action":"kinesisvideo:*","Resource":"*"}]}"#,
                Decision::Allowed,
            ),
            (
                "exactly this action",
                r#"{"Statement":[{"Effect":"Allow","Action":"kinesisvideo:GetImages",
                 "Resource":"*"}]}"#,
                Decision::Allowed,
            ),
            (
                "another operation of this namespace",
                r#"{"Statement":[{"Effect":"Allow","Action":"kinesisvideo:ListFragments",
                 "Resource":"*"}]}"#,
                Decision::NotStated,
            ),
            (
                "another service entirely",
                r#"{"Statement":[{"Effect":"Allow","Action":"kinesis:*","Resource":"*"}]}"#,
                Decision::NotStated,
            ),
            (
                "an explicit deny of this action beside an allow of the namespace",
                r#"{"Statement":[
                 {"Effect":"Allow","Action":"kinesisvideo:*","Resource":"*"},
                 {"Effect":"Deny","Action":"kinesisvideo:GetImages","Resource":"*"}]}"#,
                Decision::Denied,
            ),
        ];
        for (case, document, expected) in rows {
            let reached = decision(document, &action, &resource);
            assert_eq!(
                reached, *expected,
                "case {case:?}: expected {expected:?} for {action}, got {reached:?}"
            );
        }
    }

    /// RULE: the decision is taken against the STREAM's ARN, so a statement scoped to one
    /// stream's ARN covers that stream and no other — not another stream of the same
    /// account and Region, and not the same stream name in another Region.
    #[test]
    fn a_statement_is_read_for_the_stream_arn_the_request_resolves_to() {
        let addressed = stream_arn("us-east-1", "sensor-feed", 1_761_000_000);
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny",
             "Action":"kinesisvideo:GetImages","Resource":"{addressed}"}}]}}"#
        );
        // (case, the resource the decision is taken against, whether the deny covers it)
        let rows: &[(&str, String, bool)] = &[
            ("the stream the request names", addressed.clone(), true),
            (
                "another stream of the same account and Region",
                stream_arn("us-east-1", "door-camera", 1_761_000_000),
                false,
            ),
            (
                "the same stream name in another Region",
                stream_arn("eu-west-3", "sensor-feed", 1_761_000_000),
                false,
            ),
            (
                "the same stream name created at another instant",
                stream_arn("us-east-1", "sensor-feed", 1_762_000_000),
                false,
            ),
            (
                "the bare stream name, which names no ARN",
                "sensor-feed".to_string(),
                false,
            ),
        ];
        for (case, resource, covered) in rows {
            let said = decision(&document, &action_for("GetImages"), resource);
            let denied = said == Decision::Denied;
            assert_eq!(
                denied, *covered,
                "case {case:?}: expected the deny to {}cover {resource}, got {said:?}",
                if *covered { "" } else { "not " }
            );
        }
    }
}
