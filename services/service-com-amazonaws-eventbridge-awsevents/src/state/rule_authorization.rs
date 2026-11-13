// SPDX-License-Identifier: CC0-1.0

//! Whether the policies in force let the calling principal CHANGE a rule — the
//! authorization pass every rule mutator of this service takes before it writes anything.
//!
//! Changing a rule is the action `events:<OperationName>` on that rule's own ARN
//! ([`rule_action`], built on this service's `arnNamespace`), so AWS decides it the way it
//! decides any call on a resource: against the identity-based policies in force on the
//! principal the request's credential speaks for, at the instant of the call. A rule carries
//! no resource-based policy of its own — `PutPermission` installs one on an event BUS
//! ([`super::BusPolicy`]) and on nothing else — so the identity half is the whole decision
//! here.
//!
//! The piece this module exists for is the CONDITION CONTEXT. A statement about a rule can
//! be scoped by the tags the rule HOLDS, through the global condition key family
//! `aws:ResourceTag/<tag-key>` ("compares the tag key-value pair that you specify in the
//! policy with the key-value pair attached to the resource"), which is the primary way an
//! account scopes who may touch which rules. The values of that family are a fact of the
//! TARGET rule rather than of the request, so they are read off the stored rule
//! ([`rule_condition_context`]) and the decision follows that rule's tags: the identical call
//! against a rule carrying another tag value is refused, and retagging the rule changes the
//! answer with no change to the call.
//!
//! WHO the caller is, and which documents are in force on it, is
//! [`super::authorization::request_caller`] — the same resolution the publication gate takes,
//! so one credential speaks for one principal whatever it asks for. The decision itself is
//! [`::service_common::iam_authorization`], the tree's ONE evaluation of policy documents
//! against `(action, resource, condition context)`.
//!
//! ## The rule
//!
//! In IAM's own order: an explicit `Deny` about the call in an identity-based document
//! refuses it whatever grants it; otherwise the call needs an `Allow` about it in those
//! documents; with neither it is refused, which is IAM's implicit deny, and the rule is left
//! exactly as it was.
//!
//! The two callers this gate does not bound are the two
//! [`super::authorization`] names, for the same reasons: the account's own published
//! credential, which the IAM store holds no policies for and which has full control of the
//! account's own rules, and a principal carrying an attachment whose document this emulator
//! cannot read, where refusing would deny access AWS permits.
//!
//! ## The keys a call publishes out of its own REQUEST
//!
//! Beside the resource half, AWS publishes this service's OWN request condition keys for the
//! call being decided — the keys named after the members that call carries — so a statement
//! scoped by one of them decides the calls whose members satisfy it and refuses only the
//! others. A call that attaches targets to a rule carries the ARN of each target it attaches,
//! so it publishes [`TARGET_ARN_CONDITION_KEY`], one value per entry
//! ([`target_attachment_denial`]); a call that carries no target ARN publishes none of them
//! ([`rule_mutation_denial`]), which is every other rule mutator of this service — the
//! removal operation names the targets to remove by `Id` and so carries no ARN at all.
//!
//! ## The condition keys this gate does not publish yet
//!
//! AWS also publishes the REQUEST-tag families for a call that carries tags of its own —
//! `aws:RequestTag/<tag-key>` and `aws:TagKeys` — which here would be the `Tags` a
//! rule-creating `PutRule` or a `TagResource` carries and the `TagKeys` an `UntagResource`
//! names, and the keys read out of a rule's submitted event PATTERN. A statement conditioned
//! on one of those is therefore left UNDECIDED, which never
//! grants and never weakens a deny ([`::service_common::iam_authorization`]): such a grant
//! refuses a call AWS would serve, which is the fail-closed direction, and reading the
//! request's own tags onto the context is what closes it.

use ::service_common::iam_authorization::{self as iam, Decision};

use super::authorization::Caller;
use super::*;

/// The IAM action namespace this service's actions are published under: an action is
/// `events:<OperationName>`, this service's `arnNamespace`
/// (`smithy_query(query="service_info")`).
pub const ACTION_NAMESPACE: &str = ARN_NAMESPACE;

/// The condition key family carrying the tags the ADDRESSED resource holds:
/// "`aws:ResourceTag/tag-key` — compares the tag key-value pair that you specify in the
/// policy with the key-value pair attached to the resource".
pub const RESOURCE_TAG_PREFIX: &str = "aws:ResourceTag/";

/// The condition keys a call on a rule DECLARES it models whatever else it carries: the
/// whole [`RESOURCE_TAG_PREFIX`] family, because the addressed rule's tag set is known in
/// full — which is what lets a clause written with IAM's `IfExists` suffix, or a `Null`
/// clause, be answered for a tag key the rule does not carry. A key outside this family, and
/// outside the request keys the CALL publishes ([`RequestKeys`]), leaves its statement
/// undecided, which never grants and never weakens a deny.
const RULE_CONDITION_KEYS: [&str; 1] = [RESOURCE_TAG_PREFIX];

/// `events:TargetArn`: this service's own request condition key carrying the ARN of a target
/// a rule may invoke — "the ARN of a target that can be invoked by a rule".
///
/// A call that attaches targets publishes one value for it per entry of its target list
/// (`Target$Arn`, `smithy_query(query="shape",shape="Target")`, required on every entry), so
/// a statement scoped by it is decided over that SET of values: under one of IAM's set
/// qualifiers across every ARN the request carries, and unqualified only where the request
/// carries a single one.
pub const TARGET_ARN_CONDITION_KEY: &str = "events:TargetArn";

/// The condition keys one CALL publishes out of its own request members, beside the
/// resource-tag family every call on a rule carries ([`RULE_CONDITION_KEYS`]).
///
/// A key is published only by a call that really carries the member it is named after, so a
/// statement scoped by it decides that call and leaves every other one undecided rather than
/// reading a value the request never sent.
#[derive(Debug, Clone, Default)]
struct RequestKeys {
    /// The keys this call DECLARES it models, whether or not it carries a value for each:
    /// what makes an absent value mean "the request carries none" rather than "this emulator
    /// does not model the key"
    /// ([`::service_common::iam_authorization::Request::with_published_keys`]).
    published: Vec<&'static str>,
    /// One `(key, value)` pair per value the request carries, in the request's own order — a
    /// key the request carries SEVERAL values for appears once per value, which is how IAM's
    /// set qualifiers read it.
    context: Vec<(String, String)>,
}

impl RequestKeys {
    /// The keys a call that ATTACHES targets to a rule publishes: one
    /// [`TARGET_ARN_CONDITION_KEY`] value per target ARN the request carries.
    fn of_targets<'a>(target_arns: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            published: vec![TARGET_ARN_CONDITION_KEY],
            context: target_arns
                .into_iter()
                .map(|arn| (TARGET_ARN_CONDITION_KEY.to_string(), arn.to_string()))
                .collect(),
        }
    }
}

/// The IAM action the operation `operation` performs — `events:DisableRule` for
/// `DisableRule`, and so on for every operation of this service.
#[must_use]
pub fn rule_action(operation: &str) -> String {
    format!("{ACTION_NAMESPACE}:{operation}")
}

/// The condition context a call on a rule carrying `tags` is decided with: one
/// `aws:ResourceTag/<key>` pair per tag the rule holds.
#[must_use]
pub fn rule_condition_context(tags: &[Tag]) -> Vec<(String, String)> {
    tags.iter()
        .map(|tag| (format!("{RESOURCE_TAG_PREFIX}{}", tag.key), tag.value.clone()))
        .collect()
}

/// The `AccessDeniedException` message the call `operation` makes on the rule `rule_name` of
/// the bus `bus_name` in `region` earns when the policies in force do not permit it, or
/// `None` when they do.
///
/// The rule is read from `store` for the two facts the decision is made from: the ARN IAM
/// judges the action against, and the tags that give [`RESOURCE_TAG_PREFIX`] its values. A
/// name this store holds no rule under is judged against the ARN such a rule WOULD carry
/// ([`rule_arn`], the one place this service mints one) and an empty tag set, because IAM
/// decides before the service looks at the resource — so a caller holding no grant reads
/// that it is not authorized whether or not the rule exists, and a caller holding one goes
/// on to the operation's own answer about a resource that is missing.
///
/// Decided with no store of another service reached: `caller` is resolved BEFORE this
/// service's store is locked ([`super::authorization::request_caller`]), and everything
/// below is a pure evaluation of the documents it carries.
#[must_use]
pub fn rule_mutation_denial(
    store: &ComAmazonawsEventbridgeAwseventsState,
    caller: &Caller,
    operation: &str,
    region: &str,
    bus_name: &str,
    rule_name: &str,
) -> Option<String> {
    rule_denial(
        store,
        caller,
        operation,
        region,
        bus_name,
        rule_name,
        &RequestKeys::default(),
    )
}

/// [`rule_mutation_denial`] for a call that ATTACHES targets to the rule, which publishes
/// the ARN of each target it attaches as [`TARGET_ARN_CONDITION_KEY`] — so a statement scoped
/// by that key grants the calls whose targets satisfy it and refuses only the others.
///
/// `target_arns` are the ARNs the request carries, in its own order, one published value
/// each.
#[must_use]
pub fn target_attachment_denial<'a>(
    store: &ComAmazonawsEventbridgeAwseventsState,
    caller: &Caller,
    operation: &str,
    region: &str,
    bus_name: &str,
    rule_name: &str,
    target_arns: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    rule_denial(
        store,
        caller,
        operation,
        region,
        bus_name,
        rule_name,
        &RequestKeys::of_targets(target_arns),
    )
}

/// The one resolution every call on a rule by NAME is decided through: the rule's ARN and
/// tags read off `store`, and the keys the call itself publishes (`request`).
fn rule_denial(
    store: &ComAmazonawsEventbridgeAwseventsState,
    caller: &Caller,
    operation: &str,
    region: &str,
    bus_name: &str,
    rule_name: &str,
    request: &RequestKeys,
) -> Option<String> {
    let stored = store.rule(region, bus_name, rule_name);
    let rule_arn = stored.map_or_else(
        || rule_arn(region, bus_name, rule_name),
        |rule| rule.arn.clone(),
    );
    let tags = stored.map(|rule| rule.tags.as_slice()).unwrap_or_default();
    action_denial(caller, &rule_action(operation), &rule_arn, tags, request)
}

/// [`rule_mutation_denial`] for a call that addressed its rule by ARN rather than by name:
/// the rule the tagging operations resolved ([`parse_tagged_resource`]).
///
/// A call that addressed an event BUS is outside this gate — a bus is authorized against its
/// own resource policy as well as the caller's documents, which is a decision of its own —
/// so such a call earns no refusal here.
#[must_use]
pub fn tagged_rule_denial(
    store: &ComAmazonawsEventbridgeAwseventsState,
    caller: &Caller,
    operation: &str,
    resource: &TaggedResource,
) -> Option<String> {
    let TaggedResource::Rule {
        region,
        event_bus_name,
        name,
    } = resource
    else {
        return None;
    };
    rule_mutation_denial(store, caller, operation, region, event_bus_name, name)
}

/// The refusal the identity-based documents in force on `caller` state against `action` on
/// `rule_arn`, evaluated with the rule's own `tags`, or `None` when they permit it.
///
/// The module docs' rule, in IAM's own order.
#[must_use]
pub fn rule_action_denial(
    caller: &Caller,
    action: &str,
    rule_arn: &str,
    tags: &[Tag],
) -> Option<String> {
    action_denial(caller, action, rule_arn, tags, &RequestKeys::default())
}

/// [`rule_action_denial`] for a call publishing request keys of its own: the decision is
/// taken with the rule's tags AND those keys in the condition context, so one evaluation
/// answers every call on a rule.
fn action_denial(
    caller: &Caller,
    action: &str,
    rule_arn: &str,
    tags: &[Tag],
    keys: &RequestKeys,
) -> Option<String> {
    // The resource half (the addressed rule's tags) and the request half (what the call
    // itself carries) are one context: a statement may be scoped by either.
    let mut context = rule_condition_context(tags);
    context.extend(keys.context.iter().cloned());
    let mut published: Vec<&str> = RULE_CONDITION_KEYS.to_vec();
    published.extend(keys.published.iter().copied());
    // An identity-based document states no `Principal` element, so the decision rests on
    // the action, the resource and the context; the principal spellings travel with the
    // request because that is what such an element would be matched against.
    let request = iam::Request::new(action, rule_arn, &caller.principals)
        .with_context(&context)
        .with_published_keys(&published);
    match iam::identity_decision(&caller.documents, &request) {
        // An explicit deny in a document that CAN be read refuses the call whatever else
        // the principal carries.
        Decision::Denied => Some(not_authorized(
            &caller.arn,
            action,
            rule_arn,
            "with an explicit deny in an identity-based policy",
        )),
        Decision::Allowed => None,
        // The two callers this gate does not bound (see the module docs).
        Decision::NotStated if caller.account_credential || caller.unreadable_attachment => None,
        Decision::NotStated => Some(not_authorized(
            &caller.arn,
            action,
            rule_arn,
            &format!("because no identity-based policy allows the {action} action"),
        )),
    }
}
