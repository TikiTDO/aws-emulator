// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER's own identity-based IAM policies let a request to this service
//! perform the action it asks for — the decision every operation of this service takes
//! before it computes anything.
//!
//! A routing call is made under some principal's credentials, and AWS decides a signed
//! call against that principal's identity-based policies at the IAM layer, ahead of the
//! service looking at the request's own input. So a caller whose policies grant none of
//! this service's actions is told it is not authorized, whatever positions or options the
//! request carried, and the call computes nothing — which is what makes a WITHHELD GRANT
//! distinguishable from a request the service cannot serve: the first is the modelled
//! `AccessDeniedException` (HTTP 403), the second the modelled `ValidationException` (HTTP
//! 400).
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from ([`sts_store::resolve_caller`]) for a
//!    session credential, and from IAM's own `access_keys` map (the map `iam:GetUser`
//!    resolves a caller through) for a long-term key;
//! 2. **what it may do** — the identity-based policy documents in force on that principal
//!    and the permissions boundary that caps them, read through the IAM service's OWN
//!    accessors (`principal_policies`, `permissions_boundary_document`), so a policy
//!    `iam:PutUserPolicy` or `iam:PutRolePolicy` wrote takes effect on the next call and
//!    rewriting it from `Allow` to `Deny` takes effect on the one after that;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared
//!    evaluation of those documents against `(action, resource)`, and the SENTENCE a
//!    refusal carries, which is that module's
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
//! The ACTION is `geo-routes:<OperationName>`: `geo-routes` is this service's own action
//! namespace — its `arnNamespace`, which is also the SigV4 signing name its model declares
//! (`smithy_query(query="service_info")`) — in front of the operation the request named.
//!
//! The RESOURCE is the routing PROVIDER the action is taken on, the single resource type
//! this service publishes: `provider/<name>` under that namespace, in the Region the caller
//! signed for, with an EMPTY account field — the provider belongs to the service rather
//! than to the account addressing it, so no account id appears in its ARN, which is why it
//! is not built through the account-stamping ARN builder. Every operation of this service
//! routes over the one provider it serves ([`DEFAULT_PROVIDER`]), so the resource is a fact
//! of the request's Region alone and a decision can be taken before any input is read.
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

/// The action namespace this service's operations are published under, which is also the
/// SigV4 signing name its own model declares (`aws.auth#sigv4` name `geo-routes`, and the
/// `arnNamespace` of its `aws.api#service` trait): a policy statement spells one of them
/// `geo-routes:<OperationName>`.
pub const ACTION_PREFIX: &str = "geo-routes";

/// The routing provider every action of this service is taken on.
///
/// This service exposes no customer-created resource: a route is computed over the
/// provider the service routes with, which is the one its own calls address.
pub const DEFAULT_PROVIDER: &str = "default";

/// The Region an ARN carries for a request whose credential scope names none that is
/// Region-shaped — the Region this emulator's own ARNs default to.
const DEFAULT_REGION: &str = "us-east-1";

/// The IAM action `operation` is performed under: `geo-routes:<OperationName>`.
#[must_use]
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The ARN of the routing provider this service's actions are taken on, in `region`.
///
/// The account field is EMPTY: the provider is the service's own, not a resource of the
/// account calling it (see the module docs), so this is spelled here rather than built
/// through the shared builder that stamps the account id into every ARN it makes.
#[must_use]
pub fn provider_resource(region: &str) -> String {
    format!("arn:aws:{ACTION_PREFIX}:{region}::provider/{DEFAULT_PROVIDER}")
}

/// The provider ARN one REQUEST is decided against: [`provider_resource`] in the Region the
/// caller signed for.
///
/// The Region comes from the caller's own SigV4 credential scope, and is held to the Region
/// LABEL grammar first ([`::service_common::arn::region_label`], the tree's one reading of
/// it): a scope naming something that is not a Region falls back to [`DEFAULT_REGION`]
/// rather than letting caller-chosen text reshape the ARN's colon-delimited fields.
#[must_use]
pub fn provider_addressed(parts: &::http::request::Parts) -> String {
    let signed = ::emulator_server::sigv4_credential_scope(&parts.headers).map(|scope| scope.region);
    let region = signed
        .as_deref()
        .and_then(::service_common::arn::region_label)
        .unwrap_or(DEFAULT_REGION);
    provider_resource(region)
}

/// `Some(message)` when IAM does not allow `geo-routes:<operation>` on the provider this
/// request addresses for the calling principal — the sentence the refusal carries — and
/// `None` when it does.
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
) -> Option<String> {
    let action = action_for(operation);
    let resource = provider_addressed(ctx.parts());
    let caller = caller(engine, ctx);
    // An identity-based document states no `Principal` element, so the decision rests on
    // the action and the resource the request addressed.
    let request = iam::Request::new(&action, &resource, &[]);
    let said = iam::identity_decision(&caller.documents, &request);
    if said == Decision::Denied {
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            &action,
            &resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    if !(iam::is_allowed(said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            &action,
            &resource,
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
        &resource,
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
/// the same instant whether or not anybody called, which is why a REPORTING operation may
/// take this decision too.
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

    /// Every operation this service serves, by the name its own model gives it — the names
    /// the actions this gate asks about are built from (`smithy_query(query="digest")`).
    const OPERATIONS: &[&str] = &[
        "CalculateIsolines",
        "CalculateRouteMatrix",
        "CalculateRoutes",
        "OptimizeWaypoints",
        "SnapToRoads",
    ];

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `geo-routes:<OperationName>`, so a document
    /// scoped to this service's namespace decides every one of its operations and nothing
    /// outside them.
    #[test]
    fn the_action_asked_about_carries_this_services_namespace() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"geo-routes:*","Resource":"*"}]}"#;
        let resource = provider_resource("us-east-1");
        for operation in OPERATIONS {
            let action = action_for(operation);
            let reached = decision(document, &action, &resource);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected geo-routes:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "geo-places:GetPlace", &resource);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected geo-routes:* to state nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: a grant is read for the action AND the provider ARN it names, so a statement
    /// naming the provider of another Region allows nothing.
    #[test]
    fn a_grant_is_read_for_the_provider_it_names() {
        let granted = provider_resource("us-west-2");
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Action":"geo-routes:CalculateRoutes","Resource":"{granted}"}}]}}"#
        );
        // (case, operation, the provider ARN the call addressed, allowed)
        let rows: &[(&str, &str, String, bool)] = &[
            (
                "the granted action on the granted provider",
                "CalculateRoutes",
                granted.clone(),
                true,
            ),
            (
                "another action on the granted provider",
                "SnapToRoads",
                granted.clone(),
                false,
            ),
            (
                "the granted action in another Region",
                "CalculateRoutes",
                provider_resource("eu-central-1"),
                false,
            ),
            (
                "the granted action against every resource",
                "CalculateRoutes",
                "*".to_string(),
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
            {"Effect":"Allow","Action":"geo-routes:*","Resource":"*"},
            {"Effect":"Deny","Action":"geo-routes:CalculateRoutes","Resource":"*"}]}"#;
        let resource = provider_resource("us-east-1");
        let denied = decision(document, &action_for("CalculateRoutes"), &resource);
        assert_eq!(
            denied,
            Decision::Denied,
            "case \"the denied action\": expected Denied, got {denied:?}"
        );
        let allowed = decision(document, &action_for("SnapToRoads"), &resource);
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
        let mut request = ::http::Request::builder().uri("https://localhost/v2/routes");
        if let Some(region) = region {
            request = request.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/geo-routes/\
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

    /// RULE: the provider ARN a request is decided against carries the Region that request
    /// was SIGNED for, and a credential scope whose Region field is not a Region label
    /// cannot reshape the ARN — it falls back to the Region the emulator's own ARNs default
    /// to, so no caller-supplied text reaches an ARN field.
    #[test]
    fn the_region_of_the_provider_arn_comes_from_the_credential_scope() {
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
            ("no credential scope at all", None, "us-east-1"),
        ];
        for (case, signed, expected) in rows {
            let parts = signed_for(*signed);
            let actual = provider_addressed(&parts);
            let wanted = provider_resource(expected);
            assert_eq!(
                actual, wanted,
                "case {case:?}: expected the ARN {wanted:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the provider ARN carries no account id — the provider is the service's own, not
    /// a resource of the account addressing it — and names the one provider this service
    /// routes over.
    #[test]
    fn the_provider_arn_carries_an_empty_account_field() {
        let arn = provider_resource("us-east-1");
        let fields: Vec<&str> = arn.split(':').collect();
        assert_eq!(
            fields.len(),
            6,
            "case \"the provider ARN\": expected ARN's six fields, got {arn:?}"
        );
        assert_eq!(
            fields[4], "",
            "case \"the account field\": expected it empty, got {arn:?}"
        );
        assert_eq!(
            fields[2], ACTION_PREFIX,
            "case \"the service field\": expected {ACTION_PREFIX}, got {arn:?}"
        );
        assert_eq!(
            fields[5],
            format!("provider/{DEFAULT_PROVIDER}"),
            "case \"the resource field\": expected the default provider, got {arn:?}"
        );
    }
}
