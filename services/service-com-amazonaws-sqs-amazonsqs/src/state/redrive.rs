// SPDX-License-Identifier: CC0-1.0

//! The two redrive attributes a queue carries, and what each document has to look like to
//! be accepted at all.
//!
//! A source queue's `RedrivePolicy` names the dead-letter queue its messages are driven to
//! and how many deliveries it allows; a dead-letter queue's own `RedriveAllowPolicy` says
//! which source queues may name it. Both are judged when they are WRITTEN — by `CreateQueue`
//! and by `SetQueueAttributes` alike — so a document that could configure no redrive any
//! delivery could ever make is refused rather than stored, and the queue keeps the value it
//! had.

use super::*;

/// The `deadLetterTargetArn` a `RedrivePolicy` attribute value names, or `None` when
/// the value is not a JSON object carrying one.
///
/// "The string that includes the parameters for the dead-letter queue functionality of
/// the source queue as a JSON object", whose `deadLetterTargetArn` is "the Amazon
/// Resource Name (ARN) of the dead-letter queue to which Amazon SQS moves messages
/// after the value of maxReceiveCount is exceeded" (the `RedrivePolicy` attribute's own
/// documentation).
pub fn dead_letter_target_arn(policy: &str) -> Option<String> {
    let policy = parsed_redrive_policy(policy)?;
    redrive_target_arn(&policy).map(ToString::to_string)
}

/// A `RedrivePolicy` attribute value as the JSON object it is, or `None` for a value that
/// is not JSON at all. One parse, so a reader wanting both members pays for one.
pub(super) fn parsed_redrive_policy(policy: &str) -> Option<::serde_json::Value> {
    ::serde_json::from_str::<::serde_json::Value>(policy).ok()
}

/// [`dead_letter_target_arn`] over a policy already parsed.
///
/// A member carrying no text names no queue, so it is read as a policy that carries no
/// dead-letter target at all rather than as one naming the queue called "".
pub(super) fn redrive_target_arn(policy: &::serde_json::Value) -> Option<&str> {
    policy
        .get(DEAD_LETTER_TARGET_ARN_MEMBER)?
        .as_str()
        .map(str::trim)
        .filter(|target| !target.is_empty())
}

/// The `maxReceiveCount` a parsed `RedrivePolicy` carries, or `None` when it carries no
/// usable count.
///
/// `maxReceiveCount` is "the number of times a message is delivered to the source queue
/// before being moved to the dead-letter queue" (the `RedrivePolicy` attribute's own
/// documentation), so a count of zero or less admits no delivery to count and configures
/// no dead-lettering. The count is read whether the JSON carries it as a number or as a
/// string of digits: either spelling says the same thing about the queue, and a policy is
/// STORED with the member as the number its documentation gives it
/// ([`canonical_redrive_document`]).
pub(super) fn redrive_max_receive_count(policy: &::serde_json::Value) -> Option<u32> {
    let count = policy.get(MAX_RECEIVE_COUNT_MEMBER)?;
    count
        .as_i64()
        .or_else(|| {
            count
                .as_str()
                .and_then(|raw| raw.trim().parse::<i64>().ok())
        })
        .filter(|count| *count > 0)
        .and_then(|count| u32::try_from(count).ok())
}

/// The reason a refusal gives for a `deadLetterTargetArn` naming a queue of ANOTHER
/// REGION than the queue the policy is written on.
///
/// A redrive moves a message from the source queue to the target queue, and that delivery
/// never crosses a region ([`Queue::dead_letter_target`]), so the rule the value broke is
/// about the dead-letter TARGET: it has to be in the source's own region. Amazon SQS states
/// that rule in its own sentence, and it is NOT the sentence about which source queues a
/// dead-letter queue admits ([`REDRIVE_ALLOW_POLICY_REGION_REASON`] and the `allowAll`
/// wording quoted on [`REDRIVE_PERMISSION_ALLOW_ALL`]), which is the dead-letter queue's
/// own `RedriveAllowPolicy` rule: a caller branching on the `Reason` clause has only that
/// clause to tell it which of the two constraints to go and fix.
pub const DEAD_LETTER_TARGET_REGION_REASON: &str =
    "Dead-letter target must be in same region as the source.";

/// The reason a refusal gives for a `deadLetterTargetArn` naming a queue of ANOTHER
/// ACCOUNT than the queue the policy is written on.
///
/// The other half of the namespace rule, stated for the other field an ARN carries: a
/// redrive delivery crosses no account boundary either, so a target of some other account's
/// is refused — with the rule about the target's OWNER rather than the one about its region,
/// so the clause names the field the caller has to change.
pub const DEAD_LETTER_TARGET_OWNER_REASON: &str =
    "Dead-letter target owner should be same as the source.";

/// Why the `deadLetterTargetArn` value `target` names no queue a source queue of `region`
/// could ever drive its messages to, as the `Reason` clause stating the rule it broke, or
/// `None` when it names one of this region's own queues.
///
/// Each field of the ARN that puts the target outside the source's namespace has its OWN
/// rule and its own sentence: the region field ([`DEAD_LETTER_TARGET_REGION_REASON`]) and
/// the account field ([`DEAD_LETTER_TARGET_OWNER_REASON`]). A value that is no queue ARN at
/// all — another service's resource, or text with the wrong shape entirely — names no
/// dead-letter target to judge the namespace of, and is left to the store's own rule about a
/// target that is not there ([`DEAD_LETTER_TARGET_MISSING_REASON`]).
pub(super) fn dead_letter_target_namespace_reason(
    target: &str,
    region: &str,
) -> Option<&'static str> {
    let (service, named_region, account, _) = queue_arn_fields(target)?;
    if service != ARN_SERVICE {
        return None;
    }
    if account != ::engine::emulator_account_id() {
        return Some(DEAD_LETTER_TARGET_OWNER_REASON);
    }
    (named_region != region).then_some(DEAD_LETTER_TARGET_REGION_REASON)
}

/// Why the `RedrivePolicy` value `policy` cannot be stored on a queue of `region`, as the
/// text of the refusal a caller has to fix the request from, or `None` when it can.
///
/// A dead-letter configuration is judged when it is WRITTEN, against what the attribute's
/// own documentation says such a document carries: both documented members, a
/// `maxReceiveCount` inside the range that member publishes, and a `deadLetterTargetArn`
/// naming a queue of the same account and region. A policy breaking any of them configures
/// no redrive any delivery could ever make, so it is refused rather than stored — otherwise
/// the wiring looks configured and black-holes.
///
/// A redrive link is between two queues of ONE account and region: the target a policy
/// names is read in the region the queue carrying the policy lives in, so a
/// `deadLetterTargetArn` naming another region or another account names a queue no redrive
/// from this queue could ever deliver to — each of those two fields refused with the rule
/// stated for IT ([`dead_letter_target_namespace_reason`]).
///
/// Each refusal is worded in the one template this service answers a refused parameter
/// VALUE in ([`invalid_parameter_message`]), with the value taken from the request: the
/// `RedrivePolicy` the caller supplied, exactly as supplied, and a `Reason` clause stating
/// the one rule that value broke.
///
/// What this judges is the document ALONE. Whether the two queues it links may be linked
/// needs the store: whether the target is there at all and whether it is of the source's
/// type ([`ComAmazonawsSqsAmazonsqsState::dead_letter_target_refusal`]), and the target's
/// own `RedriveAllowPolicy`
/// ([`ComAmazonawsSqsAmazonsqsState::redrive_allow_policy_refusal`]). A value that is not
/// JSON at all is left to the attribute's own form rules ([`check_attribute_value`]).
pub fn redrive_policy_refusal(region: &str, policy: &str) -> Option<String> {
    let parsed = parsed_redrive_policy(policy)?;
    let refused =
        |reason: &str| Some(invalid_parameter_message(policy, REDRIVE_POLICY, reason));
    // Both members the attribute documents are mandatory: a policy missing the target names
    // nowhere to move a message to, and one missing the count bounds no delivery, so neither
    // configures dead-lettering at all.
    let Some(target) = redrive_target_arn(&parsed) else {
        return refused(&missing_redrive_member_reason(DEAD_LETTER_TARGET_ARN_MEMBER));
    };
    let Some(count) = parsed.get(MAX_RECEIVE_COUNT_MEMBER) else {
        return refused(&missing_redrive_member_reason(MAX_RECEIVE_COUNT_MEMBER));
    };
    // A count the member's published range does not hold — including one that is not a whole
    // number at all — is refused with the bounds it has to lie between, and the count quoted
    // as the request wrote it.
    let (min, max) = MAX_RECEIVE_COUNT_RANGE;
    let within = redrive_max_receive_count(&parsed)
        .is_some_and(|count| (min..=max).contains(&i64::from(count)));
    if !within {
        return refused(&max_receive_count_reason(&supplied_member_text(count)));
    }
    match dead_letter_target_namespace_reason(target, region) {
        Some(reason) => refused(reason),
        None => None,
    }
}

/// The reason a refusal gives for a `RedrivePolicy` that does not carry one of the two
/// members the attribute documents, naming the member the policy left out — the one thing
/// the caller has to add.
pub(super) fn missing_redrive_member_reason(member: &str) -> String {
    format!("Redrive policy does not contain mandatory attribute: {member}.")
}

/// The reason a refusal gives for a `maxReceiveCount` outside the range that member
/// publishes: the count as the request wrote it, and the bounds it has to lie between
/// ([`MAX_RECEIVE_COUNT_RANGE`]).
pub(super) fn max_receive_count_reason(supplied: &str) -> String {
    let (min, max) = MAX_RECEIVE_COUNT_RANGE;
    format!(
        "Invalid value for maxReceiveCount: {supplied}, valid values are from {min} to {max} \
         both inclusive."
    )
}

/// One member of a policy document as the REQUEST wrote it, for a refusal to quote back: a
/// string member without its quotes, and every other form as its own JSON.
pub(super) fn supplied_member_text(member: &::serde_json::Value) -> String {
    member
        .as_str()
        .map(ToString::to_string)
        .unwrap_or_else(|| member.to_string())
}

/// The reason a refusal gives for a `RedrivePolicy` whose dead-letter target is not a queue
/// of the SOURCE queue's own type.
///
/// A redrive moves a message from the source queue to the target queue, and the two queue
/// types order and deliver messages differently, so a message cannot cross between them —
/// "the dead-letter queue of a FIFO queue must also be a FIFO queue. Similarly, the
/// dead-letter queue of a standard queue must also be a standard queue"
/// (`smithy_query(query="doc", shape="SetQueueAttributesRequest", member="Attributes",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`).
///
/// That is the model's PROSE about the constraint; what the refusal carries on the wire is
/// the one sentence Amazon SQS answers this rule with, which is what a caller branching on
/// the `Reason` clause matches. BOTH terminal periods belong to that sentence as the service
/// words it, and the second one is deliberate: the clause really does end in two, as the
/// recorded 400 response of this refusal shows. [`invalid_parameter_message`] appends
/// nothing, so neither comes from the template — do not "correct" the pair away.
pub const DEAD_LETTER_TARGET_TYPE_REASON: &str =
    "Dead-letter queue must be same type of queue as the source..";

/// The reason a refusal gives for a `RedrivePolicy` whose dead-letter target is not a queue
/// this store holds.
///
/// A redrive MOVES a message from the source queue to the target queue, so a policy naming
/// a queue that is not there configures a move nothing could ever make: the association is
/// refused when it is written rather than discovered when a message is owed a redrive.
/// Worded as Amazon SQS words what a dead-letter target has to be, the way the
/// region and type rules above are.
pub const DEAD_LETTER_TARGET_MISSING_REASON: &str = "Dead letter target does not exist.";

/// The two attributes whose value is a JSON object this service's own documentation defines
/// member by member, with the members it documents as NUMBERS.
///
/// A value of one of them is stored in the canonical rendering of that object rather than
/// in the caller's own spelling ([`canonical_redrive_document`]), which is what makes a
/// later read report one document for every spelling of it. `Policy` is not here: "a valid
/// Amazon Web Services policy" is a document this service does not define member by member,
/// so what it normalises is the one spelling AWS canonicalises
/// ([`authorization::normalised_policy`]).
pub(super) const CANONICAL_JSON_ATTRIBUTES: &[(&str, &[&str])] = &[
    (REDRIVE_POLICY, &[MAX_RECEIVE_COUNT_MEMBER]),
    (REDRIVE_ALLOW_POLICY, &[]),
];

/// A redrive policy document as this service STORES and REPORTS it: the compact JSON of the
/// object the caller supplied, with every member the attribute documents as a number held
/// as that number even where the request quoted it.
///
/// The caller's whitespace, its member order and whether it wrote a numeric member quoted
/// are its own formatting and say nothing about the queue, so they do not survive into what
/// a later read reports: a client that json-loads the attribute and compares the count
/// numerically, or re-submits the value it read, sees one document. A value that is not a
/// JSON object is kept as supplied — its own form rules are what judge it.
///
/// The members are rendered in NAME ORDER, which is also the order the two attributes'
/// documentation lists theirs in (`deadLetterTargetArn` before `maxReceiveCount`,
/// `redrivePermission` before `sourceQueueArns`), so the rendering is a function of the
/// document alone and not of the order a request happened to write it in.
pub(super) fn canonical_redrive_document(value: &str, numeric_members: &[&str]) -> String {
    let Ok(::serde_json::Value::Object(members)) =
        ::serde_json::from_str::<::serde_json::Value>(value)
    else {
        return value.to_string();
    };
    let canonical: BTreeMap<String, ::serde_json::Value> = members
        .into_iter()
        .map(|(member, supplied)| {
            let held = if numeric_members.contains(&member.as_str()) {
                numeric_member(supplied)
            } else {
                supplied
            };
            (member, held)
        })
        .collect();
    ::serde_json::to_string(&canonical).unwrap_or_else(|_| value.to_string())
}

/// One member documented as a number, as the number it is: a count the request wrote as a
/// string of digits is held as those digits' value, and anything else is left as it came so
/// a malformed document is not rewritten into a different one.
pub(super) fn numeric_member(supplied: ::serde_json::Value) -> ::serde_json::Value {
    match supplied
        .as_str()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
    {
        Some(number) => ::serde_json::Value::from(number),
        None => supplied,
    }
}

/// The reason a refusal gives for a `RedrivePolicy` whose dead-letter target does not admit
/// the queue that would be driving its messages there.
///
/// The target queue's `RedriveAllowPolicy` is what admits a source queue
/// ([`Queue::admits_dead_letter_source`]); the sentence states that the attribute of the
/// dead-letter queue is what stopped the request, which is the one thing the caller has to
/// go and change.
pub const REDRIVE_ALLOW_POLICY_REASON: &str =
    "The value for the dead-letter queue's RedriveAllowPolicy attribute prevents you from \
     completing this request.";

/// The reason a refusal gives for a `RedriveAllowPolicy` that lists a source queue ARN of
/// another region than the queue the permission is written on.
///
/// The SAME boundary as [`DEAD_LETTER_TARGET_REGION_REASON`], seen from the other end of the
/// link: a `sourceQueueArns` entry names "the source queues that can specify this queue as
/// the dead-letter queue", and a queue of another region can never be one of them, so the
/// permission would admit nothing. Amazon SQS words this end of the rule for the parameter
/// that carried the ARNs, and that wording is what the caller has to fix the document from.
pub const REDRIVE_ALLOW_POLICY_REGION_REASON: &str =
    "Amazon SQS can't create the redrive allow policy. All source queue ARNs must be in the \
     same region as the current SQS queue.";

/// The permission types a `RedriveAllowPolicy` may name: "Valid values are: allowAll ...
/// denyAll ... byQueue" (the `redrivePermission` member's own documentation). Any other value
/// names no permission type this attribute has.
pub const REDRIVE_PERMISSIONS: &[&str] = &[
    REDRIVE_PERMISSION_ALLOW_ALL,
    REDRIVE_PERMISSION_DENY_ALL,
    REDRIVE_PERMISSION_BY_QUEUE,
];

/// How many source queues one permission may list: "You can specify up to 10 source queue
/// ARNs" (the `sourceQueueArns` member's own documentation). A published limit, not an
/// emulator ceiling — the same documentation says what to do instead of listing more.
pub const MAX_SOURCE_QUEUE_ARNS: usize = 10;

/// The reason a refusal gives for a `redrivePermission` that is not one of the permission
/// types the member publishes ([`REDRIVE_PERMISSIONS`]).
///
/// A value outside that set names no permission type the attribute has, so the document as a
/// whole is not one this attribute can be read in: Amazon SQS answers that with its own
/// sentence about the document's FORM, and that sentence — not a paraphrase of the member's
/// value set — is what a caller branching on the `Reason` clause matches. The comma and the
/// contraction belong to the sentence as the service words it.
pub const REDRIVE_PERMISSION_REASON: &str =
    "Amazon SQS can't create the redrive allow policy, as it\u{2019}s in an unsupported format.";

/// The reason a refusal gives for a `sourceQueueArns` supplied beside a permission that takes
/// no list: "You can specify this parameter only when the redrivePermission parameter is set
/// to byQueue" (the member's own documentation). The two members are coupled, so a document
/// that lists source queues under `allowAll` or `denyAll` says two different things about
/// which queues are admitted.
///
/// Amazon SQS words that coupling from the end the caller has to change — the permission type
/// the document must name for the list it supplied — and that sentence is what the refusal
/// carries on the wire.
pub const SOURCE_QUEUE_ARNS_PERMISSION_REASON: &str =
    "Amazon SQS can't create the redrive allow policy. To specify one or more source queue \
     ARNs, you must set redrivePermission to byQueue.";

/// The reason a refusal gives for a `byQueue` permission that names no source queue at all.
///
/// The rule is the `redrivePermission` member's own: "byQueue – Only queues specified by the
/// sourceQueueArns parameter can specify this queue as the dead-letter queue", so under that
/// permission the list is a parameter the document has to carry — the other half of the
/// coupling [`SOURCE_QUEUE_ARNS_PERMISSION_REASON`] states.
///
/// What the refusal carries on the wire is the one sentence Amazon SQS answers THIS rule with,
/// which is what a caller telling this refusal apart from the limit
/// ([`source_queue_arns_limit_reason`]) and the coupling
/// ([`SOURCE_QUEUE_ARNS_PERMISSION_REASON`]) matches — all three share the code and the
/// `Reason` prefix, so the clause is the only thing that distinguishes them. It is the
/// service's wording and not a restatement of the rule in the emulator's own words.
pub const SOURCE_QUEUE_ARNS_REQUIRED_REASON: &str =
    "Amazon SQS can't create the redrive allow policy. When you specify the byQueue permission \
     type, you must also specify one or more sourceQueueArns values.";

/// The reason a refusal gives for a `sourceQueueArns` listing more source queues than the
/// member admits, stating the published ceiling ([`MAX_SOURCE_QUEUE_ARNS`]) the list has to
/// stay within — the one thing the caller has to change the document by.
pub fn source_queue_arns_limit_reason() -> String {
    format!(
        "Amazon SQS can't create the redrive allow policy. The maximum number of source queue \
         ARNs you can specify is {MAX_SOURCE_QUEUE_ARNS}."
    )
}

/// The reason a refusal gives for a document that is not the shape the attribute documents —
/// one that is not a JSON object at all, or whose `sourceQueueArns` is not a list of ARNs:
/// "The string that includes the parameters for the permissions for the dead-letter queue
/// redrive permission and which source queues can specify dead-letter queues as a JSON
/// object", whose `sourceQueueArns` is "the Amazon Resource Names (ARN)s of the source
/// queues".
pub const REDRIVE_ALLOW_POLICY_FORM_REASON: &str =
    "Amazon SQS can't create the redrive allow policy. The redrive allow policy must be a JSON \
     object whose sourceQueueArns parameter lists Amazon SQS queue ARNs.";

/// Why a `RedriveAllowPolicy` document cannot be stored, told apart by the class the refusal
/// answers under: each handler renders it as the modelled error its own operation declares.
#[derive(Debug)]
pub enum RedriveAllowPolicyRefusal {
    /// The DOCUMENT is not one this attribute takes: the wrong shape, a list under a
    /// permission that takes none, more source queues than the member admits, or a source
    /// queue outside the region. Answered as a refused parameter value, which quotes the
    /// document back with the rule it broke.
    Document(String),
    /// The `redrivePermission` names no permission type the member has — the one refusal about
    /// a VALUE inside the document rather than about the document, so it carries the attribute
    /// and the rule and not the document.
    Permission(InvalidAttributeValue),
}

/// Why the `RedriveAllowPolicy` value `policy` cannot be stored on a queue of `region`, or
/// `None` when it can.
///
/// The permission document is judged when it is WRITTEN, exactly as a `RedrivePolicy`
/// document is ([`redrive_policy_refusal`]), against what the attribute's own documentation
/// says such a document carries: a JSON object, a `redrivePermission` of the three the member
/// publishes, a `sourceQueueArns` only under the permission that takes one, listing at least
/// one queue and at most [`MAX_SOURCE_QUEUE_ARNS`] of them, and none naming a queue of another
/// region — a queue that
/// could never specify this one as its dead-letter target, so the entry admits nothing
/// ([`REDRIVE_ALLOW_POLICY_REGION_REASON`]). A document breaking any of them is refused rather
/// than stored, and the queue keeps the permission it had: otherwise the permission looks
/// configured while admitting whatever the malformed document degrades to.
///
/// What this judges is the document ALONE, and only the ARNs it lists: whether the queues it
/// names exist is not part of the permission — a source queue may be created after the
/// dead-letter queue has already been told to admit it — and an entry that is no queue ARN at
/// all names no source queue in any region, which the permission simply never admits
/// ([`Queue::admits_dead_letter_source`]). A value that is not JSON at all is left to the
/// attribute's own form rules ([`check_attribute_value`]).
pub fn redrive_allow_policy_document_refusal(
    region: &str,
    policy: &str,
) -> Option<RedriveAllowPolicyRefusal> {
    let parsed = ::serde_json::from_str::<::serde_json::Value>(policy).ok()?;
    let refused = |reason: &str| {
        Some(RedriveAllowPolicyRefusal::Document(
            invalid_parameter_message(policy, REDRIVE_ALLOW_POLICY, reason),
        ))
    };
    let Some(members) = parsed.as_object() else {
        return refused(REDRIVE_ALLOW_POLICY_FORM_REASON);
    };
    // The permission type first: every other rule of the document is stated in terms of it,
    // so a document naming no permission type the member has cannot be judged on the rest.
    let named = members.get(REDRIVE_PERMISSION_MEMBER);
    let permission = named.and_then(::serde_json::Value::as_str).map(str::trim);
    if named.is_some() && !permission.is_some_and(|value| REDRIVE_PERMISSIONS.contains(&value)) {
        return Some(RedriveAllowPolicyRefusal::Permission(InvalidAttributeValue {
            message: Some(invalid_value_message_with_reason(
                REDRIVE_ALLOW_POLICY,
                REDRIVE_PERMISSION_REASON,
            )),
        }));
    }
    let listed = match members.get(SOURCE_QUEUE_ARNS_MEMBER) {
        // The permission that admits exactly the queues its list names needs a list; the two
        // that admit by rule rather than by name take none.
        None if permission == Some(REDRIVE_PERMISSION_BY_QUEUE) => {
            return refused(SOURCE_QUEUE_ARNS_REQUIRED_REASON)
        }
        None => return None,
        // "You can specify this parameter only when the redrivePermission parameter is set to
        // byQueue" — and a document naming no permission type is on the default one, which is
        // not that.
        Some(_) if permission != Some(REDRIVE_PERMISSION_BY_QUEUE) => {
            return refused(SOURCE_QUEUE_ARNS_PERMISSION_REASON)
        }
        Some(listed) => listed,
    };
    let Some(arns) = listed.as_array().filter(|listed| {
        listed
            .iter()
            .all(|entry| entry.as_str().is_some_and(|arn| !arn.trim().is_empty()))
    }) else {
        return refused(REDRIVE_ALLOW_POLICY_FORM_REASON);
    };
    if arns.is_empty() {
        return refused(SOURCE_QUEUE_ARNS_REQUIRED_REASON);
    }
    if arns.len() > MAX_SOURCE_QUEUE_ARNS {
        return refused(&source_queue_arns_limit_reason());
    }
    if arns
        .iter()
        .filter_map(::serde_json::Value::as_str)
        .any(|arn| source_queue_arn_of_another_region(arn.trim(), region))
    {
        return refused(REDRIVE_ALLOW_POLICY_REGION_REASON);
    }
    None
}

/// Whether `arn` names an Amazon SQS queue of a region OTHER than `region` — the one thing
/// [`redrive_allow_policy_document_refusal`] judges a listed source queue on.
pub(super) fn source_queue_arn_of_another_region(arn: &str, region: &str) -> bool {
    queue_arn_fields(arn).is_some_and(|(service, named_region, _, _)| {
        service == ARN_SERVICE && named_region != region
    })
}

/// How a refusal of a `RedrivePolicy` VALUE names the policy it refused: the two parameters
/// the policy carries, each as the request supplied it, rather than the document's own
/// spelling — whitespace, member order and whether the count came as a number or as a
/// string of digits are a caller's formatting and say nothing about what was refused.
pub(super) fn refused_redrive_policy_value(
    max_receive_count: &str,
    dead_letter_target_arn: &str,
) -> String {
    format!(
        "RedrivePolicy(maxReceiveCount={max_receive_count}, \
         deadLetterTargetArn={dead_letter_target_arn})"
    )
}

/// The `maxReceiveCount` a parsed `RedrivePolicy` carries, as the digits the request
/// supplied: the member is read whether the JSON carries it as a number or as a string
/// (see [`redrive_max_receive_count`]), and a refusal quotes back what the caller wrote.
pub(super) fn redrive_max_receive_count_text(policy: &::serde_json::Value) -> Option<String> {
    let count = policy.get(MAX_RECEIVE_COUNT_MEMBER)?;
    count
        .as_str()
        .map(|raw| raw.trim().to_string())
        .or_else(|| count.as_i64().map(|count| count.to_string()))
}
