// SPDX-License-Identifier: CC0-1.0

//! What an ARN of this service names, and how a value that is not one is refused.
//!
//! The parsing of an ARN's fields, the topic-name rule a topic ARN's resource is held to,
//! and the shared refusal sentences each fault takes. One place, because every operation
//! that resolves an ARN asks the same questions of it: a value one call accepts cannot be
//! one another refuses.

use super::*;

/// How many colon-separated elements an ARN has:
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`.
pub const ARN_ELEMENTS: usize = 6;

/// Why `value` is not an ARN at all, worded as the `Reason:` clause of an
/// `InvalidParameterException` states it — or `None` when it has an ARN's shape.
///
/// This is ARGUMENT validation, not a lookup: a value that could not name any
/// resource in any account is bad input, so an ARN-typed member is checked for
/// this BEFORE the store is consulted. Reporting it as a resource that does not
/// exist tells a caller the opposite of what happened — "it is gone" instead of
/// "you sent me something that is not an ARN" — and an idempotent delete then
/// swallows the defect entirely.
///
/// Only the SHAPE is judged here. An ARN-shaped value that names no resource of
/// this account is a lookup miss, which each operation reports as its own
/// modelled `NotFoundException`.
pub fn arn_shape_reason(value: &str) -> Option<String> {
    let elements = value.split(':').count();
    if elements < ARN_ELEMENTS {
        return Some(format!(
            "An ARN must have at least {ARN_ELEMENTS} elements, not {elements}"
        ));
    }
    if !value.starts_with("arn:") {
        return Some("An ARN must begin with 'arn:'".to_string());
    }
    None
}

/// The message an ARN-typed `member` holding `value` is refused with, or `None`
/// when `value` has an ARN's shape: it names the member and the constraint the
/// value failed, and never echoes the value itself.
pub fn arn_refusal(member: &str, value: &str) -> Option<String> {
    arn_shape_reason(value).map(|reason| format!("Invalid parameter: {member} Reason: {reason}"))
}

/// The longest topic name SNS accepts: "must be between 1 and 256 characters long"
/// (`CreateTopicInput$Name` doc).
pub const MAX_TOPIC_NAME_LEN: usize = 256;

/// Whether `name` satisfies the name rule SNS documents for a topic: 1 to
/// [`MAX_TOPIC_NAME_LEN`] characters of ASCII letters, digits, `_` and `-`, with the
/// [`FIFO_SUFFIX`] of a FIFO topic the one exception.
///
/// Shared, because it is both what `CreateTopic` refuses a bad name with and what
/// tells a topic ARN apart from another SNS ARN (a platform endpoint's, say) whose
/// resource part is not a topic name.
pub fn topic_name_is_valid(name: &str) -> bool {
    let base = name.strip_suffix(FIFO_SUFFIX).unwrap_or(name);
    !base.is_empty()
        && name.len() <= MAX_TOPIC_NAME_LEN
        && base
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `name` is a name a topic may be CREATED under, given whether the create asks
/// for a FIFO topic.
///
/// The name rule and the topic's KIND constrain each other: "For a FIFO
/// (first-in-first-out) topic, the name must end with the .fifo suffix"
/// (`CreateTopicInput$Name` doc), and the suffix is the one place a topic name carries a
/// period at all — so a `.fifo` name names a FIFO topic and nothing else, and a create
/// that does not ask for one is asking for a name no standard topic may hold. The two
/// readings of the rule are therefore one: the suffix is present exactly when
/// `FifoTopic` is asked for.
///
/// [`topic_name_is_valid`] stays the question an ARN's resource part is judged by, where
/// both kinds of name are a topic's; this is the question a CREATE is judged by.
pub fn create_topic_name_is_valid(name: &str, fifo_requested: bool) -> bool {
    topic_name_is_valid(name) && name.ends_with(FIFO_SUFFIX) == fifo_requested
}

/// The topic name an SNS topic ARN names, or `None` when `arn` is not one
/// (`arn:aws:sns:<region>:<account>:<name>` — six colon-separated fields whose last
/// one is a topic name, which is what tells it from an `…:endpoint/GCM/app/…` ARN).
pub fn topic_name_from_arn(arn: &str) -> Option<&str> {
    let fields: Vec<&str> = arn.split(':').collect();
    match fields[..] {
        ["arn", _partition, "sns", _region, _account, name] if topic_name_is_valid(name) => {
            Some(name)
        }
        _ => None,
    }
}

/// The resource part of `arn` when `arn` is an ARN of `service`, or `None` when it is
/// not an ARN at all or names another service.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account-id>:<resource>`, and the
/// resource is the ONLY field that may itself hold `:` separators (a Lambda
/// function's `function:<name>:<qualifier>`, say) — hence the split bounded at six
/// fields. Shared, because "is this string an ARN of that service?" is the question
/// every per-protocol endpoint rule asks.
pub fn arn_resource<'a>(arn: &'a str, service: &str) -> Option<&'a str> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    match fields[..] {
        ["arn", partition, found, region, account, resource]
            if found == service
                && !partition.is_empty()
                && !region.is_empty()
                && !account.is_empty()
                && !resource.is_empty() =>
        {
            Some(resource)
        }
        _ => None,
    }
}

/// The region an ARN names, or `None` when `arn` is not shaped like an ARN.
///
/// The fourth field of `arn:<partition>:<service>:<region>:<account-id>:<resource>` —
/// see [`arn_resource`] for why the split is bounded at six fields.
pub fn arn_region(arn: &str) -> Option<&str> {
    let fields: Vec<&str> = arn.splitn(ARN_ELEMENTS, ':').collect();
    match fields[..] {
        ["arn", partition, service, region, account, resource]
            if !partition.is_empty()
                && !service.is_empty()
                && !region.is_empty()
                && !account.is_empty()
                && !resource.is_empty() =>
        {
            Some(region)
        }
        _ => None,
    }
}

/// Whether the resource `arn` names belongs to `region`.
///
/// SNS is a regional service: a resource lives in the region its own ARN carries, and
/// an endpoint of one region neither reports nor reaches another region's resources.
/// This is the one rule each account-wide listing selects the records it reports by,
/// so the same request answered by two regional endpoints reports two disjoint sets.
pub fn belongs_to_region(arn: &str, region: &str) -> bool {
    arn_region(arn) == Some(region)
}

/// The message an ARN-typed `member` naming a resource of ANOTHER region is refused
/// with, or `None` when `value` names a resource of `region` — or is not an ARN at all,
/// which is a fault of its own that each operation already judges.
///
/// A regional endpoint serves the resources of its own region only, so an ARN naming
/// another region names nothing this endpoint can read or act on — and that is a fault
/// in the VALUE the caller sent, judged before any record is looked up, rather than a
/// resource that is merely absent here. The sentence is this service's bare parameter
/// refusal: the member whose value was not accepted, with neither the region nor the
/// ARN echoed back.
///
/// Shared by every operation that resolves an ARN of this service, so the rule does not
/// depend on which call carried the ARN.
pub fn foreign_region_refusal(member: &str, value: &str, region: &str) -> Option<String> {
    arn_region(value)
        .is_some_and(|named| named != region)
        .then(|| format!("Invalid parameter: {member}"))
}

/// The whole refusal an ARN-typed `member` of a REGIONAL lookup is held to, or `None`
/// when `value` is an ARN of `region`: it has to be an ARN at all ([`arn_refusal`]), and
/// it has to name this endpoint's own region ([`foreign_region_refusal`]).
///
/// Both clauses are argument validation — neither consults the store — so an operation
/// judges them before it looks anything up.
pub fn regional_arn_refusal(member: &str, value: &str, region: &str) -> Option<String> {
    arn_refusal(member, value).or_else(|| foreign_region_refusal(member, value, region))
}

/// The resource part of `arn` whatever service it names, or `None` when `arn` is not
/// shaped like an ARN.
///
/// The sixth field of `arn:<partition>:<service>:<region>:<account-id>:<resource>` — see
/// [`arn_resource`] for why the split is bounded at six fields. Where [`arn_resource`]
/// asks "is this an ARN of THAT service?", this asks only what the value names, which is
/// what a rule about the resource part itself is judged on.
pub fn arn_resource_part(arn: &str) -> Option<&str> {
    let fields: Vec<&str> = arn.splitn(ARN_ELEMENTS, ':').collect();
    match fields[..] {
        ["arn", partition, service, region, account, resource]
            if !partition.is_empty()
                && !service.is_empty()
                && !region.is_empty()
                && !account.is_empty() =>
        {
            Some(resource)
        }
        _ => None,
    }
}

/// The message an ARN-shaped `value` whose resource is not a name a TOPIC could ever
/// have is refused with, or `None` when the resource part is a legal topic name.
///
/// A topic ARN carries the topic's NAME as its resource, so a value whose resource is
/// not a name SNS would accept for a topic ([`topic_name_is_valid`] — the same rule
/// `CreateTopic` holds a name to) names no topic that could exist anywhere, in this
/// account or any other: it is bad input, judged before any lookup, rather than a topic
/// that is merely absent here. The sentence is SNS's fixed [`INVALID_TOPIC_NAME`],
/// which names the part of the ARN at fault and reproduces nothing of the value.
///
/// Shared by every operation that addresses a topic by ARN, so the rule does not depend
/// on which call carried the ARN.
pub fn topic_name_refusal(value: &str) -> Option<String> {
    let resource = arn_resource_part(value)?;
    (!topic_name_is_valid(resource)).then(|| INVALID_TOPIC_NAME.to_string())
}

/// The whole refusal a `TopicArn`-typed `member` of a regional lookup is held to, or
/// `None` when `value` is an ARN of `region` whose resource is a topic name: the shared
/// ARN rules ([`regional_arn_refusal`]) and then the topic-name rule
/// ([`topic_name_refusal`]).
pub fn topic_arn_refusal(member: &str, value: &str, region: &str) -> Option<String> {
    regional_arn_refusal(member, value, region).or_else(|| topic_name_refusal(value))
}
