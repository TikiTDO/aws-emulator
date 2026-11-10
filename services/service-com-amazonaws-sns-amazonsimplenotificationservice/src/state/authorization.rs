// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a signed request perform the action it asks for on the TOPIC it
//! addresses — the decision an operation takes before it reads or writes anything.
//!
//! Every operation of this service is signed, so every call is made by a principal, and a
//! principal may perform an action on a topic only when the documents in force allow it.
//! Two documents are in force on such a call, and AWS evaluates them together: the calling
//! principal's own identity-based policies, and the topic's access policy (its `Policy`
//! attribute, the resource-based policy SNS generates for a topic whose owner has set
//! none). An explicit `Deny` in either refuses, an `Allow` from either grants, and with
//! neither the request falls to IAM's default deny — answered with the error this service
//! declares for an unauthorized call, `AuthorizationErrorException` (HTTP 403, wire code
//! `AuthorizationError`).
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the IAM
//!    store of the same shared engine, which holds both halves of that question: the
//!    role-session record a temporary credential's mint left there, and the registry
//!    `iam:CreateAccessKey` wrote for a long-term key. So the principal a refusal blames is
//!    the one the account really holds for the credential the request signed with.
//! 2. **what it may do** — the identity-based documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through IAM's own
//!    accessors (`principal_policies`, `permissions_boundary_document`), where the
//!    collection rule itself lives — so a policy `iam:PutUserPolicy` wrote takes effect on
//!    the next request and deleting it lifts it again — beside the topic's own `Policy`,
//!    read from this service's store, where `SetTopicAttributes` and `AddPermission` write
//!    it.
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of policy documents against `(action, resource, principal)`, so this service and
//!    every other one handed the same documents answer alike; and the SENTENCES a refusal
//!    carries, which are that module's, so nothing of IAM's wording is spelled here beyond
//!    the one reason clause it does not publish.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does. The
//! lookups stay in this service crate because the IAM service crate depends on
//! `service-common`, so the shared crate cannot read IAM's store without a dependency
//! cycle.
//!
//! ## Which half grants
//!
//! A resource-based statement that names the ACCOUNT rather than the principal — a root
//! ARN or a bare account id, which is the form `AddPermission` writes a delegated account
//! as — delegates the decision to that account's own identity-based policies rather than
//! granting the call outright, so the account spellings
//! ([`::service_common::iam_authorization::account_principals`]) are read for a `Deny` and
//! not for an `Allow`. That is also why the policy SNS generates for a topic whose owner
//! set none grants nothing by itself: it allows every principal of the topic's own account
//! only where `aws:SourceOwner` equals that account, a key a cross-service request carries
//! and a direct call does not, and a statement whose condition the request cannot decide
//! never grants (the shared evaluator's fail-closed reading).
//!
//! A grant the topic's policy gives the principal BY NAME stands on its own: "resource-based
//! policies that specify the IAM entity as the principal are not limited by the permissions
//! boundary" (IAM, evaluating effective permissions with boundaries), so only a grant that
//! came from the identity-based documents is capped by it.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id the IAM store
//!   holds no record for speaks for the account itself — the position AWS account root
//!   credentials are in — which holds no identity-based policies, and it is the credential an
//!   emulated AWS service signs its in-process calls into this one with: the principal behind
//!   such a call is a SERVICE, not an identity of this account, so neither document is in
//!   force on it and nothing here refuses it. (A cross-service caller that has a resource
//!   policy of its own to satisfy reads it where that question belongs — the alarm action
//!   that publishes to a topic asks the topic's policy about the CloudWatch service
//!   principal, which is not this account's credential.)
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the account's store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no IAM store**, where no user and no policy can exist.
//!
//! Nothing here writes: both stores are only read, each under its own lock and neither held
//! while the other is taken, and the decision is reached before the operation touches its
//! own records.

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_authorization::{self as iam, Decision, PreparedDocument};

/// The IAM action `Publish` performs, and the one `PublishBatch` performs too: the batch
/// call publishes messages, and the SNS action namespace publishes no action of its own for
/// it ("Actions defined by Amazon Simple Notification Service", IAM Service Authorization
/// Reference).
pub const PUBLISH: &str = "SNS:Publish";

/// The IAM action `GetTopicAttributes` performs.
pub const GET_TOPIC_ATTRIBUTES: &str = "SNS:GetTopicAttributes";

/// The IAM action `SetTopicAttributes` performs.
pub const SET_TOPIC_ATTRIBUTES: &str = "SNS:SetTopicAttributes";

/// The IAM action `AddPermission` performs.
pub const ADD_PERMISSION: &str = "SNS:AddPermission";

/// The IAM action `RemovePermission` performs.
pub const REMOVE_PERMISSION: &str = "SNS:RemovePermission";

/// The IAM action `DeleteTopic` performs.
pub const DELETE_TOPIC: &str = "SNS:DeleteTopic";

/// The IAM action `Subscribe` performs.
pub const SUBSCRIBE: &str = "SNS:Subscribe";

/// The IAM action `ListSubscriptionsByTopic` performs.
pub const LIST_SUBSCRIPTIONS_BY_TOPIC: &str = "SNS:ListSubscriptionsByTopic";

/// The reason clause for a request an explicit `Deny` in the RESOURCE's own policy refuses:
/// a fact about that document, so the sentence names it rather than the action.
///
/// The shared module publishes the identity-side clauses and this one for the resource side
/// is spelled the way every other service in this tree that refuses on a resource policy
/// spells it (`grep -rl 'with an explicit deny in a resource-based policy' services/*/src`).
const RESOURCE_POLICY_EXPLICIT_DENY: &str = "with an explicit deny in a resource-based policy";

/// The refusal this request earns from the documents in force on it, or `None` when IAM
/// permits it (and for every caller this gate does not bound, see the module docs).
///
/// `action` is the IAM action the operation performs (one of the constants above) and
/// `topic_arn` the ARN of the topic the REQUEST addresses, as the request named it rather
/// than as a stored record resolves it — so the decision does not depend on whether that
/// topic exists, which is what keeps a refusal from telling an unauthorized caller whether
/// it does.
///
/// Call it with this service's own store NOT locked: both stores are reached here, each
/// under its own lock.
#[must_use]
pub fn topic_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    topic_arn: &str,
) -> Option<String> {
    let policy = topic_policy(engine, topic_arn);
    refusal(
        &caller(engine, ctx),
        action,
        topic_arn,
        policy.as_deref(),
    )
}

/// The access policy of the topic `topic_arn`, as it reports it — the document its owner set
/// or the one SNS generates for a topic with none — and `None` when this region's store
/// holds no such topic, which is a call with no resource-based policy in force.
fn topic_policy<Eng: Send + Sync + 'static>(engine: &Eng, topic_arn: &str) -> Option<String> {
    let store = super::service_state(engine)?;
    let store = store.lock();
    store
        .topics
        .get(topic_arn)
        .map(super::TopicRecord::policy_attribute)
}

/// The refusal `action` on `resource` earns from the documents in force — `caller`'s own,
/// and the resource policy `resource_policy` carries — or `None` when they permit it.
///
/// IAM's order, which is the shared evaluation's: an explicit `Deny` in either document
/// refuses whatever else is written; otherwise a grant from either suffices; and a grant the
/// identity-based documents gave is capped by the principal's permissions boundary.
fn refusal(
    caller: &Caller,
    action: &str,
    resource: &str,
    resource_policy: Option<&str>,
) -> Option<String> {
    // The account's own published credential is the one an emulated service signs its
    // in-process calls into this one with, and the principal it speaks for is not an IAM
    // identity the account holds documents for: neither document is in force on it, so there
    // is nothing here to refuse it with (see the module docs).
    if caller.account_credential {
        return None;
    }
    // An identity-based document states no `Principal` element, so the principal spellings
    // travel with the request for the RESOURCE policy's sake: that is what its `Principal`
    // element is matched against.
    let asked = iam::Request::new(action, resource, &caller.principals);
    let identity = iam::identity_decision(&caller.documents, &asked);
    if identity == Decision::Denied {
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            action,
            resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    let prepared = resource_policy.map(PreparedDocument::read);
    // A statement that DENIES the account refuses each of the account's principals, so the
    // deny is read with the account's own spellings beside the principal's.
    let mut delegated = caller.principals.clone();
    delegated.extend(iam::account_principals(&caller.arn));
    let denying = iam::Request::new(action, resource, &delegated);
    let stated_deny = prepared
        .as_ref()
        .map_or(Decision::NotStated, |document| {
            iam::resource_outcome_prepared(document, &denying).decision
        });
    if stated_deny == Decision::Denied {
        // The same sentence the shared module builds for an identity-based refusal: AWS
        // words every authorization failure alike — who, what action, which resource, and
        // what refused it — so only the reason clause differs.
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            action,
            resource,
            RESOURCE_POLICY_EXPLICIT_DENY,
        ));
    }
    // The grant is read with the principal's OWN spellings alone: a statement naming the
    // account delegates to that account's identity-based policies rather than granting.
    let stated_allow = prepared
        .as_ref()
        .map_or(Decision::NotStated, |document| {
            iam::resource_outcome_prepared(document, &asked).decision
        });
    if stated_allow == Decision::Allowed {
        // A grant the resource policy gives the principal by name is not capped by the
        // principal's permissions boundary (see the module docs).
        return None;
    }
    if !(identity == Decision::Allowed || caller.identity_is_unbounded()) {
        // Neither document allows the call, which within one account is IAM's default deny:
        // what the principal is missing is a permission of its OWN, so the sentence names the
        // identity-based policies that did not grant it, the clause AWS states for a
        // same-account caller no policy allows.
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            action,
            resource,
            &iam::no_identity_policy_allows(action),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not
    // a permission the principal has. The account's own credential carries none.
    let boundary = caller.boundary.as_ref()?;
    let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &asked) {
        Decision::Allowed => return None,
        // An explicit deny is a fact about the boundary document, so the refusal names it; a
        // boundary that merely states nothing withheld the grant the policies gave.
        Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
        Decision::NotStated => iam::no_permissions_boundary_allows(action),
    };
    Some(iam::not_authorized_by_identity_policy(
        &caller.arn,
        action,
        resource,
        &reason,
    ))
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal a policy's `Principal` element can name it by: the
    /// session ARN a temporary credential speaks under, and the ARN of the IAM role behind
    /// it.
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
}

/// Who this request speaks for, resolved from the access key id it signed with.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let Some(iam) = iam_store::service_state(engine) else {
        return account_caller();
    };
    let store = iam.lock();
    let now = ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now());
    // A TEMPORARY credential of a role session: the principal is the SESSION, and the
    // policies in force are the ROLE's ("the temporary security credentials … have the
    // permissions of the role", `AssumeRole`). The role's own ARN is a spelling a policy
    // names the caller by too.
    if let Some(session) = store.role_session(&access_key_id, now) {
        let mut caller = Caller {
            arn: session.session_arn.clone(),
            principals: vec![session.session_arn.clone(), session.role_arn.clone()],
            ..Caller::default()
        };
        let role_arn = session.role_arn.clone();
        read_policies_of(&store, &role_arn, &mut caller);
        return caller;
    }
    // A LONG-TERM access key speaks for the IAM user that holds it, which is the identity
    // `sts:GetCallerIdentity` reports for it; a key the account holds no record for is the
    // account's own published credential.
    let user_arn = store
        .access_keys
        .get(&access_key_id)
        .and_then(|record| store.users.get(&record.user_key))
        .map(|user| user.arn.clone());
    match user_arn {
        Some(user_arn) => {
            let mut caller = Caller {
                arn: user_arn.clone(),
                principals: vec![user_arn.clone()],
                ..Caller::default()
            };
            read_policies_of(&store, &user_arn, &mut caller);
            caller
        }
        None => account_caller(),
    }
}

/// The caller a request presenting the account's own published credential speaks for: the
/// principal `sts:GetCallerIdentity` reports for it, which the IAM store holds no policies
/// for (see the module docs).
fn account_caller() -> Caller {
    let arn = ::service_common::caller_principal::ambient_user_arn();
    Caller {
        arn: arn.clone(),
        principals: vec![arn],
        account_credential: true,
        ..Caller::default()
    }
}

/// Read what the IAM identity `identity_arn` names may do, through the IAM service's own
/// accessors: the documents in force on it, whether it carries an attachment whose document
/// this emulator cannot read, and the permissions boundary that caps it.
fn read_policies_of(
    store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
    identity_arn: &str,
    caller: &mut Caller,
) {
    if let Some(policies) = store.principal_policies(identity_arn) {
        caller.documents = policies.documents;
        // An attachment the collection resolved no document for is an entry of AWS's own
        // catalog this emulator does not carry: a grant it cannot read.
        caller.unreadable_attachment = !policies.uncarried.is_empty();
    }
    caller.boundary = store.permissions_boundary_document(identity_arn);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A topic of this account, as every ARN this service mints spells one.
    fn topic_arn() -> String {
        super::super::topic_arn("us-east-1", "gate-topic")
    }

    /// An IAM user of this account, with the identity-based documents `documents`.
    fn user(documents: &[&str]) -> Caller {
        let arn = ::emulator_server::emulator_arn("iam", "", "user/gate-user");
        Caller {
            arn: arn.clone(),
            principals: vec![arn],
            documents: documents.iter().map(|text| (*text).to_string()).collect(),
            ..Caller::default()
        }
    }

    /// RULE: a principal whose identity-based documents say nothing is refused a topic whose
    /// policy grants it nothing, and the same call is allowed once either document allows
    /// the action on that topic.
    #[test]
    fn a_grant_from_either_document_allows_and_neither_refuses() {
        let topic = topic_arn();
        let principal = user(&[]).arn;
        let identity_allow =
            format!(r#"{{"Statement":[{{"Effect":"Allow","Action":"sns:Publish","Resource":"{topic}"}}]}}"#);
        let resource_allow = format!(
            r#"{{"Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{principal}"}},
            "Action":"SNS:Publish","Resource":"{topic}"}}]}}"#
        );
        let default_policy =
            super::super::default_topic_policy(&topic, &super::super::owner_account_id()).to_json();
        // (case, the identity documents, the topic policy, whether the call is allowed)
        let rows: &[(&str, &[&str], Option<&str>, bool)] = &[
            ("no document at all", &[], None, false),
            (
                "the policy SNS generates for a topic with none",
                &[],
                Some(&default_policy),
                false,
            ),
            (
                "an identity-based grant",
                &[&identity_allow],
                Some(&default_policy),
                true,
            ),
            (
                "a grant in the topic's own policy",
                &[],
                Some(&resource_allow),
                true,
            ),
        ];
        for (case, documents, policy, expected) in rows {
            let refused = refusal(&user(documents), PUBLISH, &topic, *policy);
            assert_eq!(
                refused.is_none(),
                *expected,
                "case {case:?}: expected allowed == {expected}, got {refused:?}"
            );
        }
    }

    /// RULE: an explicit `Deny` wins, from either document, whatever the other allows.
    #[test]
    fn an_explicit_deny_in_either_document_refuses() {
        let topic = topic_arn();
        let principal = user(&[]).arn;
        let identity_allow =
            r#"{"Statement":[{"Effect":"Allow","Action":"sns:*","Resource":"*"}]}"#.to_string();
        let identity_deny = format!(
            r#"{{"Statement":[{{"Effect":"Deny","Action":"sns:Publish","Resource":"{topic}"}}]}}"#
        );
        let resource_deny = format!(
            r#"{{"Statement":[{{"Effect":"Deny","Principal":{{"AWS":"{principal}"}},
            "Action":"SNS:Publish","Resource":"{topic}"}}]}}"#
        );
        let refused = refusal(&user(&[&identity_allow, &identity_deny]), PUBLISH, &topic, None);
        assert_eq!(
            refused,
            Some(iam::not_authorized_by_identity_policy(
                &principal,
                PUBLISH,
                &topic,
                iam::IDENTITY_POLICY_EXPLICIT_DENY,
            )),
            "case \"an identity-based deny beside a wildcard allow\": unexpected answer"
        );
        let refused = refusal(
            &user(&[&identity_allow]),
            PUBLISH,
            &topic,
            Some(&resource_deny),
        );
        assert_eq!(
            refused,
            Some(iam::not_authorized_by_identity_policy(
                &principal,
                PUBLISH,
                &topic,
                RESOURCE_POLICY_EXPLICIT_DENY,
            )),
            "case \"a topic-policy deny beside an identity-based allow\": unexpected answer"
        );
    }

    /// RULE: the action asked about is the one the operation performs, so a document scoped
    /// to one action decides that action alone — and SNS's own spelling of an action and
    /// IAM's differ only in case, which IAM compares case-insensitively.
    #[test]
    fn a_grant_is_read_for_the_action_it_names() {
        let topic = topic_arn();
        let document =
            format!(r#"{{"Statement":[{{"Effect":"Allow","Action":"sns:publish","Resource":"{topic}"}}]}}"#);
        // (case, the action asked about, whether the grant covers it)
        let rows: &[(&str, &str, bool)] = &[
            ("the granted action", PUBLISH, true),
            ("another action on the same topic", SUBSCRIBE, false),
            ("a read of the same topic", GET_TOPIC_ATTRIBUTES, false),
            ("a write of the same topic", SET_TOPIC_ATTRIBUTES, false),
        ];
        for (case, action, expected) in rows {
            let refused = refusal(&user(&[&document]), action, &topic, None);
            assert_eq!(
                refused.is_none(),
                *expected,
                "case {case:?}: expected allowed == {expected}, got {refused:?}"
            );
        }
    }

    /// RULE: the grant has to name the resource the request addresses, so a grant on one
    /// topic allows nothing on another.
    #[test]
    fn a_grant_is_read_for_the_resource_it_names() {
        let granted = super::super::topic_arn("us-east-1", "granted-topic");
        let document = format!(
            r#"{{"Statement":[{{"Effect":"Allow","Action":"sns:Publish","Resource":"{granted}"}}]}}"#
        );
        // (case, the topic addressed, whether the grant covers it)
        let rows: &[(&str, String, bool)] = &[
            ("the granted topic", granted.clone(), true),
            (
                "another topic of the same account",
                super::super::topic_arn("us-east-1", "other-topic"),
                false,
            ),
            (
                "the same name in another region",
                super::super::topic_arn("eu-west-1", "granted-topic"),
                false,
            ),
        ];
        for (case, topic, expected) in rows {
            let refused = refusal(&user(&[&document]), PUBLISH, topic, None);
            assert_eq!(
                refused.is_none(),
                *expected,
                "case {case:?}: expected allowed == {expected}, got {refused:?}"
            );
        }
    }

    /// RULE: the account's own published credential — the one an emulated service signs its
    /// in-process calls with — has neither document in force on it, so no policy of a topic
    /// refuses it, and a service that reads a topic's own policy is not refused by it.
    #[test]
    fn the_accounts_own_credential_is_not_bounded_by_a_topic_policy() {
        let topic = topic_arn();
        let denied = r#"{"Statement":[{"Effect":"Deny","Principal":"*","Action":"*",
            "Resource":"*"}]}"#
            .to_string();
        // (case, the topic's policy)
        let rows: &[(&str, Option<&str>)] = &[
            ("a topic with no policy at all", None),
            ("a topic policy denying every principal every action", Some(&denied)),
        ];
        for (case, policy) in rows {
            for action in [PUBLISH, GET_TOPIC_ATTRIBUTES, DELETE_TOPIC] {
                let refused = refusal(&account_caller(), action, &topic, *policy);
                assert_eq!(
                    refused, None,
                    "case {case:?} asking {action}: expected the call to be allowed, got \
                     {refused:?}"
                );
            }
        }
    }
}
