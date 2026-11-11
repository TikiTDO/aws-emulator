// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER's own identity-based IAM policies let a request to this data plane
//! perform the action it asks for — the decision each operation takes before it reads a
//! session or the control plane.
//!
//! A configuration session is opened and polled by an application running under some
//! principal's credentials — a function's execution role, an instance profile — and AWS
//! decides a signed call against that principal's identity-based policies before the
//! service looks at the request's own input. So a caller whose policies grant none of this
//! service's actions is told it is not authorized, whatever the identifiers it named, which
//! is what makes a MISSING GRANT distinguishable from an identifier that resolves to
//! nothing: the first is an authorization refusal, the second the modelled
//! `ResourceNotFoundException`.
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from ([`sts_store::resolve_caller`]) for a
//!    session credential, and from IAM's own `access_keys` map (the map `iam:GetUser`
//!    resolves a caller through) for a long-term key;
//! 2. **what it may do** — the identity-based policy documents in force on that principal
//!    and the permissions boundary that caps them, read through the IAM service's OWN
//!    accessors (`principal_policies`, `permissions_boundary_document`), so a policy
//!    `iam:PutRolePolicy` wrote takes effect on the next call and deleting it lifts it
//!    again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared evaluation
//!    of those documents against `(action, resource)`, and the SENTENCE a refusal carries,
//!    which is that module's
//!    [`::service_common::iam_authorization::not_authorized_by_identity_policy`] and the
//!    reason clauses beside it, so nothing of IAM's wording is spelled here.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does. The
//! lookups stay in this service crate because the IAM service crate depends on
//! `service-common`, so the shared crate cannot read IAM's store without a dependency
//! cycle. WHICH IAM entity a principal ARN names carries no such dependency, so it is read
//! through the shared [`::service_common::iam_role`] rather than parsed again here.
//!
//! ## The action and the resource
//!
//! Both operations of this service are published under the `appconfig` action namespace —
//! this service signs as `appconfig` (`aws.auth#sigv4` in its own model,
//! `smithy_query(query="service_info")`), and AppConfig publishes
//! `appconfig:StartConfigurationSession` and `appconfig:GetLatestConfiguration` among its
//! actions — so [`action_for`] builds the action from that namespace and the operation's
//! own name.
//!
//! The resource is the CONFIGURATION the call addresses, spelled as AppConfig spells one:
//! `application/<application>/environment/<environment>/configuration/<configuration
//! profile>` under the `appconfig` namespace ([`configuration_addressed`]), in the Region the
//! caller signed for — read through the control plane's own `request_region`, so the two
//! services spell one ARN one way. It is built from the identifiers the request carried
//! rather than from a resolved record, so the decision is the same whether or not those
//! identifiers name anything — which is what keeps a refusal from telling an unauthorized
//! caller what the account holds. A call whose own input names no configuration at all is
//! judged against [`ANY_RESOURCE`].
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
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The resource a call that names no single one is judged against, spelled with IAM's own
/// wildcard.
pub const ANY_RESOURCE: &str = "*";

/// The action namespace this service's operations are published under, which is also the
/// SigV4 signing name its own model declares (`aws.auth#sigv4` name `appconfig`): a policy
/// statement spells one of them `appconfig:<OperationName>`.
const ACTION_PREFIX: &str = "appconfig";

/// The IAM action `operation` is performed under: `appconfig:<OperationName>`.
#[must_use]
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The ARN of the configuration a session over this triple is served from, as AppConfig
/// addresses one: the configuration profile under the environment it is deployed into,
/// under their application.
///
/// Built through the shared ARN builder every ARN this emulator emits comes from, under the
/// [`ACTION_PREFIX`] namespace — the same namespace AppConfig's own resource ARNs carry —
/// from the identifiers the call carried.
#[must_use]
pub fn configuration_resource(
    region: &str,
    application_identifier: &str,
    environment_identifier: &str,
    configuration_profile_identifier: &str,
) -> String {
    ::engine::emulator_arn(
        ACTION_PREFIX,
        region,
        &format!(
            "application/{application_identifier}/environment/{environment_identifier}\
             /configuration/{configuration_profile_identifier}"
        ),
    )
}

/// The configuration ARN one REQUEST addresses: [`configuration_resource`] in the Region the
/// caller signed for.
///
/// The Region comes from the caller's own SigV4 credential scope, read through the AppConfig
/// control plane's `request_region` — the one reading of it this pair of services shares, so
/// an ARN a policy is matched against is spelled exactly as the ARNs AppConfig's own
/// responses carry, and a scope naming something that is not a Region cannot reshape the
/// ARN's colon-delimited fields.
#[must_use]
pub fn configuration_addressed(
    parts: &::http::request::Parts,
    application_identifier: &str,
    environment_identifier: &str,
    configuration_profile_identifier: &str,
) -> String {
    configuration_resource(
        &::service_com_amazonaws_appconfig_amazonappconfig::state::request_region(parts),
        application_identifier,
        environment_identifier,
        configuration_profile_identifier,
    )
}

/// `Some(message)` when IAM does not allow `appconfig:<operation>` on `resource` for the
/// calling principal — the sentence the refusal carries — and `None` when it does.
///
/// IAM's order: an explicit `Deny` in a document that CAN be read refuses the request
/// whatever else the principal carries; otherwise a grant is needed, which a caller this
/// gate does not bound has by definition (see the module docs); and a grant is capped by the
/// permissions boundary, since "a permissions boundary sets the maximum permissions that an
/// identity-based policy can grant" (IAM, permissions boundaries).
///
/// Asked with NO store guard of this service held, and before the request's own input is
/// read.
#[must_use]
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Option<String> {
    let action = action_for(operation);
    let caller = caller(engine, ctx);
    // An identity-based document states no `Principal` element, so the decision rests on
    // the action and the resource the request addressed.
    let request = iam::Request::new(&action, resource, &[]);
    let said = iam::identity_decision(&caller.documents, &request);
    if said == Decision::Denied {
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            &action,
            resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    if !(iam::is_allowed(said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            &action,
            resource,
            &iam::no_identity_policy_allows(&action),
        ));
    }
    let boundary = caller.boundary.as_ref()?;
    let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
        Decision::Allowed => return None,
        // An explicit deny is a fact about the boundary document, so the refusal names it;
        // a boundary that merely states nothing withheld the grant the policies gave.
        Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
        Decision::NotStated => iam::no_permissions_boundary_allows(&action),
    };
    Some(iam::not_authorized_by_identity_policy(
        &caller.arn,
        &action,
        resource,
        &reason,
    ))
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
/// the same instant whether or not anybody called, which is why the REPORTING operation of
/// this service may take this decision too.
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
            };
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
    use super::*;

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `appconfig:<OperationName>`, so a document
    /// scoped to AppConfig's actions decides both operations of this data plane and nothing
    /// outside them.
    #[test]
    fn the_action_asked_about_carries_appconfigs_namespace() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"appconfig:*","Resource":"*"}]}"#;
        for operation in ["StartConfigurationSession", "GetLatestConfiguration"] {
            let action = action_for(operation);
            let reached = decision(document, &action, ANY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected appconfig:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "s3:GetObject", ANY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected appconfig:* to state nothing outside \
             AppConfig, got {outside:?}"
        );
    }

    /// RULE: a grant is read for the action AND the configuration it names, so a statement
    /// naming another configuration of the same application allows nothing.
    #[test]
    fn a_grant_is_read_for_the_configuration_it_names() {
        let granted = configuration_resource("us-west-2", "app-1", "env-1", "profile-1");
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Action":"appconfig:StartConfigurationSession","Resource":"{granted}"}}]}}"#
        );
        // (case, operation, the configuration the call addressed, allowed)
        let rows: &[(&str, &str, String, bool)] = &[
            (
                "the granted action on the granted configuration",
                "StartConfigurationSession",
                granted.clone(),
                true,
            ),
            (
                "the other action on the same configuration",
                "GetLatestConfiguration",
                granted.clone(),
                false,
            ),
            (
                "the granted action on another profile of the same environment",
                "StartConfigurationSession",
                configuration_resource("us-west-2", "app-1", "env-1", "profile-2"),
                false,
            ),
            (
                "the granted action on the same profile in another environment",
                "StartConfigurationSession",
                configuration_resource("us-west-2", "app-1", "env-2", "profile-1"),
                false,
            ),
            (
                "the granted action in another Region",
                "StartConfigurationSession",
                configuration_resource("eu-central-1", "app-1", "env-1", "profile-1"),
                false,
            ),
            (
                "the granted action naming no configuration",
                "StartConfigurationSession",
                ANY_RESOURCE.to_string(),
                false,
            ),
        ];
        for (case, operation, resource, expected) in rows {
            let said = decision(&document, &action_for(operation), resource);
            let actual = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected allowed == {expected}, got {actual} ({said:?})"
            );
        }
    }

    /// RULE: an explicit `Deny` is final, whatever else the principal carries.
    #[test]
    fn an_explicit_deny_wins_over_a_wildcard_grant() {
        let document = r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"appconfig:*","Resource":"*"},
            {"Effect":"Deny","Action":"appconfig:GetLatestConfiguration","Resource":"*"}]}"#;
        let resource = configuration_resource("us-east-1", "app-1", "env-1", "profile-1");
        let denied = decision(document, &action_for("GetLatestConfiguration"), &resource);
        assert_eq!(
            denied,
            Decision::Denied,
            "case \"the denied action\": expected Denied, got {denied:?}"
        );
        let allowed = decision(document, &action_for("StartConfigurationSession"), &resource);
        assert_eq!(
            allowed,
            Decision::Allowed,
            "case \"an action the deny does not name\": expected Allowed, got {allowed:?}"
        );
    }

    /// The request parts of a call whose SigV4 credential scope carries `region` in its
    /// Region field, which is where the ARN a decision is taken against reads it from. A row
    /// passing `None` carries no `Authorization` header at all.
    fn signed_for(region: Option<&str>) -> ::http::request::Parts {
        let mut request = ::http::Request::builder().uri("https://localhost/configuration");
        if let Some(region) = region {
            request = request.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/appconfig/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            );
        }
        request
            .body(())
            .expect("the request is well formed")
            .into_parts()
            .0
    }

    /// RULE: the configuration ARN a request is decided against carries the Region that
    /// request was SIGNED for, and a credential scope whose Region field is not a Region name
    /// cannot reshape the ARN — it falls back to the Region the emulator's own ARNs default
    /// to, so no caller-supplied text reaches an ARN field.
    #[test]
    fn the_region_of_a_resource_arn_comes_from_the_credential_scope() {
        // (case, the scope's Region field, the Region the ARN must carry)
        let rows: &[(&str, Option<&str>, &str)] = &[
            ("a Region name", Some("eu-west-3"), "eu-west-3"),
            (
                "another Region name",
                Some("ap-southeast-4"),
                "ap-southeast-4",
            ),
            ("one carrying a colon", Some("us-east-1:evil"), "us-east-1"),
            ("one carrying a slash", Some("us-east-1/evil"), "us-east-1"),
            ("one in another case", Some("US-EAST-1"), "us-east-1"),
            ("the empty field", Some(""), "us-east-1"),
            ("no credential scope at all", None, "us-east-1"),
        ];
        for (case, signed, expected) in rows {
            let parts = signed_for(*signed);
            let actual = configuration_addressed(&parts, "app-1", "env-1", "profile-1");
            let wanted = configuration_resource(expected, "app-1", "env-1", "profile-1");
            assert_eq!(
                actual, wanted,
                "case {case:?}: expected the ARN {wanted:?}, got {actual:?}"
            );
        }
    }
}
