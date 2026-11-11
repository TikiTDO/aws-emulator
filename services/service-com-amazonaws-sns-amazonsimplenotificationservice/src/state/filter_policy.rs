// SPDX-License-Identifier: CC0-1.0

//! Deciding WHICH published messages a subscription with a filter policy receives.
//!
//! "FilterPolicy – The simple JSON object that lets your subscriber receive only a
//! subset of messages, rather than receiving every message published to the topic"
//! (`SubscribeInput$Attributes` doc), applied to the part of the message the
//! subscription's scope names: "MessageAttributes (default) – The filter is applied on
//! the message attributes. MessageBody – The filter is applied on the message body"
//! (`GetSubscriptionAttributesResponse$Attributes` doc).
//!
//! A policy is an object of PROPERTIES; a message satisfies it when EVERY property the
//! policy names is satisfied, and a property is satisfied when ANY of the conditions
//! listed for it holds — the matching Amazon SNS documents for its filter policies. The
//! grammar is Amazon's own and no crate implements it, so it is read here: a condition
//! is a value to equal, or one of the documented operator objects (`prefix`, `suffix`,
//! `equals-ignore-case`, `wildcard`, `anything-but`, `numeric`, `cidr`, `exists`, `$or`).
//!
//! Nothing here reads the subscription's own records: the decision is a function of the
//! stored policy, the stored scope and the published message, so the same message is
//! admitted or withheld however the two attributes came to be stored — a scope written
//! before the policy it governs included.
//!
//! What READING a policy can cost, and what it can reach. A policy is caller-supplied
//! text, so both the reading below and the matching are bounded, and neither can reach
//! anything but the document in front of it:
//!
//! * the document is at most the per-record attribute ceiling
//!   ([`super::MAX_ATTRIBUTE_BYTES_PER_RECORD`]), checked before it is stored, and it is
//!   parsed by `serde_json` — whose own recursion limit is what bounds how deeply the
//!   walks below can nest;
//! * the matching walk is bounded a second time, by depth ([`MAX_FILTER_POLICY_DEPTH`])
//!   and by a per-request comparison budget ([`MAX_FILTER_MATCH_COMPARISONS`]), and a
//!   policy past the published key and combination limits is refused at the write rather
//!   than matched;
//! * the operators are a CLOSED list of the eight the service documents plus the one
//!   relationship it names ([`OPERATORS`], [`OR_OPERATOR`]): a condition is read by this
//!   walker alone, so no part of a policy names anything to call, load or evaluate, and
//!   nothing here opens a socket, a process, a file or an environment lookup.

use std::collections::HashMap;

use serde_json::Value;

use super::{
    reported_filter_policy_scope, structured_message_body, PublishedMessage, SubscriptionRecord,
    FILTER_POLICY_ATTRIBUTE, MESSAGE_BODY_FILTER_POLICY_SCOPE,
};
use crate::ops::types::MessageAttributeValue;

/// How deep a filter policy is matched: a policy scoped to the message body may name a
/// property inside a property, and this bounds how far such a path is followed.
///
/// A policy document is caller-supplied and may nest as deeply as its JSON does, so the
/// walk is bounded rather than recursive without limit. Amazon SNS publishes a limit of
/// five nesting levels for a policy scoped to the message body, which this leaves room
/// for.
const MAX_FILTER_POLICY_DEPTH: usize = 8;

/// How many VALUES of one property the matching reads.
///
/// A property may hold a list — a `String.Array` message attribute, a JSON array in the
/// body — whose length the publishing caller chooses, bounded otherwise only by the
/// topic's message-size ceiling. This is the emulator's own ceiling on how many of those
/// values one property contributes to the matching, so the values one message can make the
/// matcher walk (and copy) are bounded by a constant rather than by the message size
/// multiplied by the subscription count. It is far above the handful of values a filter
/// policy is written against.
const MAX_VALUES_PER_PROPERTY: usize = 256;

/// How many condition-versus-value comparisons ONE request may spend on filter matching,
/// across every message it publishes and every subscription of the topic.
///
/// The cost of matching is a PRODUCT — the conditions a stored policy lists, the values a
/// published message holds for that property, the OPTIONS or relations one condition's
/// operator argument lists, the subscriptions of the topic and the entries of a batch — and
/// each factor is caller-chosen, so bounding them one by one does not bound the work. This
/// is what bounds how MANY comparisons that product comes to; what each of them READS is
/// bounded by [`MAX_FILTER_MATCH_BYTES`] beside it, exactly as `crate::ops::publish` bounds
/// its fan-out by a delivery count and a byte count together. It is set far above what any
/// realistic policy against a realistic message costs (a handful of conditions against a
/// handful of values, per subscriber).
pub const MAX_FILTER_MATCH_COMPARISONS: usize = 20_000;

/// How many BYTES of policy and message ONE request may compare while matching, across
/// every message it publishes and every subscription of the topic.
///
/// A counted comparison is not a constant amount of work: a string operator reads its
/// argument and the value it judges, both of them caller-chosen and bounded only by the
/// topic's message-size ceiling and the subscription's attribute ceiling. A count alone
/// would therefore admit a comparison count's worth of MESSAGE-SIZED reads, so the bytes
/// those comparisons read are capped as well. It is set far above what a realistic fan-out
/// reads — a message at the largest size SNS accepts, judged by every subscriber of a topic
/// — so no legitimate publish is withheld for want of it.
pub const MAX_FILTER_MATCH_BYTES: usize = 8 * 1024 * 1024;

/// What one request has left to spend on filter matching.
///
/// ONE budget for the whole request, spent across every message and every subscription:
/// that is what caps the product above rather than each factor on its own. A subscription
/// the budget runs out on is left out of the delivery — a filter that was not applied must
/// never be read as a filter that matched — and the exhaustion is logged once.
#[derive(Debug, Default)]
pub struct MatchBudget {
    comparisons: usize,
    bytes: usize,
    exhausted: bool,
}

impl MatchBudget {
    /// A fresh budget: [`MAX_FILTER_MATCH_COMPARISONS`] comparisons reading at most
    /// [`MAX_FILTER_MATCH_BYTES`] bytes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Charge `comparisons`: `false` once the budget is spent, in which case nothing more
    /// is matched.
    fn charge(&mut self, comparisons: usize) -> bool {
        let spent = self.comparisons.saturating_add(comparisons);
        if spent > MAX_FILTER_MATCH_COMPARISONS {
            return self.exhaust();
        }
        self.spend(|budget| budget.comparisons = spent)
    }

    /// Charge the `bytes` one comparison reads of the policy and of the message.
    fn charge_bytes(&mut self, bytes: usize) -> bool {
        let spent = self.bytes.saturating_add(bytes);
        if spent > MAX_FILTER_MATCH_BYTES {
            return self.exhaust();
        }
        self.spend(|budget| budget.bytes = spent)
    }

    /// Record what a charge spent, unless an earlier one already ran out.
    ///
    /// Spent is spent: once a charge has failed, every later one fails too, however small.
    /// Otherwise a request that ran out on a long comparison would go on answering the
    /// short ones, and a filter applied to part of a message would decide delivery.
    fn spend(&mut self, record: impl FnOnce(&mut Self)) -> bool {
        if self.exhausted {
            return false;
        }
        record(self);
        true
    }

    /// Note that the request has run out, once, and report the failed charge.
    fn exhaust(&mut self) -> bool {
        if !self.exhausted {
            self.exhausted = true;
            ::tracing::warn!(
                comparisons = MAX_FILTER_MATCH_COMPARISONS,
                bytes = MAX_FILTER_MATCH_BYTES,
                "this request reached the emulator's filter-matching budget; the \
                 remaining filtered subscriptions of the topic are not delivered to"
            );
        }
        false
    }

    /// Whether this request has run out of matching budget.
    ///
    /// A NEGATED condition needs this: `anything-but` holds when its argument does NOT
    /// match, and a match that stopped for want of budget is not evidence that it does not
    /// match — so the exhaustion has to withhold the message rather than invert into an
    /// admission.
    fn stopped(&self) -> bool {
        self.exhausted
    }
}

/// The operator that matches the beginning of a string value.
const PREFIX_OPERATOR: &str = "prefix";

/// The operator that matches the end of a string value.
const SUFFIX_OPERATOR: &str = "suffix";

/// The operator that matches a string value without regard to case.
const EQUALS_IGNORE_CASE_OPERATOR: &str = "equals-ignore-case";

/// The operator that matches a string value against a PATTERN: "wildcard matching" (Amazon
/// SNS Developer Guide, "Amazon SNS subscription filter policy"), whose argument is the
/// pattern the value has to match, with `*` standing for any run of characters. The
/// comparison itself is [`::service_common::event_pattern::matches_wildcard`], the tree's one
/// reading of that pattern form, because the same wildcard grammar decides an EventBridge
/// event pattern.
const WILDCARD_OPERATOR: &str = "wildcard";

/// The operator that matches every value but the ones it names.
const ANYTHING_BUT_OPERATOR: &str = "anything-but";

/// The operator that compares a numeric value against a list of relations.
const NUMERIC_OPERATOR: &str = "numeric";

/// The operator that matches an IP ADDRESS against a network: "IP address matching" (Amazon
/// SNS Developer Guide, "Amazon SNS subscription filter policy"), whose argument is the CIDR
/// block the address has to lie in. The comparison itself is
/// [`super::message_filter::matches_cidr`], because an address in a network is not a string
/// comparison.
const CIDR_OPERATOR: &str = "cidr";

/// The operator that matches on a property being present, or being absent.
const EXISTS_OPERATOR: &str = "exists";

/// The operator that holds when ANY of the policies it lists holds.
const OR_OPERATOR: &str = "$or";

/// Every operator the grammar above defines, which is also the vocabulary a policy is
/// WRITTEN against: a condition naming anything else names a match SNS does not perform.
const OPERATORS: &[&str] = &[
    PREFIX_OPERATOR,
    SUFFIX_OPERATOR,
    EQUALS_IGNORE_CASE_OPERATOR,
    WILDCARD_OPERATOR,
    ANYTHING_BUT_OPERATOR,
    NUMERIC_OPERATOR,
    CIDR_OPERATOR,
    EXISTS_OPERATOR,
];

/// The relations a `numeric` condition compares with, each taking one operand after it.
const NUMERIC_RELATIONS: &[&str] = &["=", "<", "<=", ">", ">="];

/// How many property names ONE filter policy may name, nesting included: the published
/// filter policy constraint, which counts a nested property's leaf name as one whatever
/// level it sits at (Amazon SNS Developer Guide, "Amazon SNS subscription filter
/// policies", filter policy constraints).
pub const MAX_FILTER_POLICY_KEYS: usize = 5;

/// How many combinations of values ONE filter policy may come to — the product of the
/// conditions its properties list, which is what the published constraint bounds (Amazon
/// SNS Developer Guide, "Amazon SNS subscription filter policies", filter policy
/// constraints).
pub const MAX_FILTER_POLICY_COMBINATIONS: usize = 150;

/// Why a filter policy document is not one SNS would apply: the reason, and WHICH layer
/// of the service refused it.
///
/// The two read differently on the wire. A GRAMMAR fault is the policy reader's own —
/// it names the condition it could not read and where in the document that condition
/// ends — and the reader's sentence stands as it is, whichever member carried the
/// document. A LIMIT is the service's constraint on the policy as a whole, and its
/// sentence is framed under the member the document arrived in
/// ([`super::AttributeMember`]).
#[derive(Debug, Clone)]
pub enum DocumentFault {
    /// The reader could not make a policy of the document.
    Grammar(String),
    /// The policy is past one of the published filter-policy constraints.
    Limit(String),
}

/// The fault a filter policy document carries, or `None` when SNS would apply it.
///
/// A stored policy decides, for the life of the subscription, which messages its
/// endpoint receives, so the document is judged against the GRAMMAR the matching reads
/// ([`matches_policy`]) and against the two published constraints on its size: a
/// condition naming an operator this grammar has no reading for would silently withhold
/// every message, and a policy past the key or combination limit is one the service does
/// not accept at all.
///
/// Shared by every operation that writes a `FilterPolicy`
/// ([`super::subscription_attribute_refusal`]), so a document `Subscribe` accepts cannot
/// be one `SetSubscriptionAttributes` refuses.
pub fn document_fault(document: &str) -> Option<DocumentFault> {
    let Ok(Value::Object(properties)) = serde_json::from_str::<Value>(document) else {
        // The two faults a document can have before its grammar is read — it does not
        // parse, or it is not an object — are reported by the caller, which states SNS's
        // own sentence for each.
        return None;
    };
    let policy = Value::Object(properties);
    // The diagnostic a grammar fault carries names the policy as the READER holds it —
    // the document re-serialized from the tree the parse produced — rather than the
    // caller's own spacing, so the position it names is a position in that text.
    let source = serde_json::to_string(&policy).unwrap_or_else(|_| document.to_string());
    let mut keys = 0;
    if let Some(reason) = policy_refusal(&policy, &source, &[], &mut keys) {
        return Some(DocumentFault::Grammar(reason));
    }
    if keys > MAX_FILTER_POLICY_KEYS {
        return Some(DocumentFault::Limit(format!(
            "Filter policy can not have more than {MAX_FILTER_POLICY_KEYS} keys"
        )));
    }
    (combinations(&policy) > MAX_FILTER_POLICY_COMBINATIONS)
        .then(|| DocumentFault::Limit("Filter policy is too complex".to_string()))
}

/// The sentence the policy reader states for a condition value that is none of the
/// values a match takes.
const MATCH_VALUE_REASON: &str = "Match value must be String, number, true, false, or null";

/// The sentence the reader states for an object naming more than one match type, and for
/// one naming none.
const ONE_MATCH_TYPE_ONLY: &str = "Only one key allowed in match expression";
const EMPTY_OBJECT: &str = "Empty objects are not allowed";

/// The sentence the reader states for a list with nothing in it to match.
const EMPTY_ARRAY: &str = "Empty arrays are not allowed";

/// The sentence the reader states for an `exists` whose argument is not a flag.
const EXISTS_NOT_BOOLEAN: &str = "exists match pattern must be either true or false.";

/// How much of the value a refusal names the reader had consumed when it refused — which
/// is what decides the position the diagnostic reports.
///
/// Reading the member NAME that introduces a value consumes the colon after the name and
/// the value's first character as well, so a refusal about the name stands one character
/// into the value; a refusal decided from the value's own TYPE had read its opening token
/// alone.
#[derive(Debug, Clone, Copy)]
enum Read {
    /// One character into the value, where reading its member name leaves the reader.
    Entered,
    /// Its opening token: a string's opening quote, an object's or a list's bracket, or
    /// the whole of any other scalar, which is one token.
    Opening,
    /// All of it, up to and past its closing token: what a refusal that had to read the
    /// node's CONTENT had read.
    Whole,
}

/// One step from the policy's root to one of its nodes, as the walk over the text the
/// reader holds follows it.
#[derive(Debug, Clone)]
enum Step {
    /// The value of the member with this name.
    Key(String),
    /// The member of a list at this position.
    Index(usize),
}

/// `path` with `step` after it: the path to a child of the node `path` names.
fn stepped(path: &[Step], step: Step) -> Vec<Step> {
    let mut steps = path.to_vec();
    steps.push(step);
    steps
}

/// The offset in `source` the reader stands at, having walked to the node `path` names
/// and consumed `read` of it.
///
/// The position a diagnostic reports is a fact about the TEXT rather than about the
/// parse tree, so the walk needs each node's own characters — which is what
/// `serde_json`'s `RawValue` is: a node read as a WINDOW onto the text it came from, so
/// where that window begins is where the node begins.
fn offset_of(source: &str, path: &[Step], read: Read) -> Option<usize> {
    let mut node: &serde_json::value::RawValue = serde_json::from_str(source).ok()?;
    for step in path {
        node = match step {
            Step::Index(index) => {
                serde_json::from_str::<Vec<&serde_json::value::RawValue>>(node.get())
                    .ok()?
                    .get(*index)
                    .copied()?
            }
            Step::Key(key) => serde_json::from_str::<
                std::collections::BTreeMap<String, &serde_json::value::RawValue>,
            >(node.get())
            .ok()?
            .remove(key.as_str())?,
        };
    }
    let text = node.get();
    let start = offset_in(source, text)?;
    Some(match read {
        Read::Entered => start + text.chars().next().map_or(0, char::len_utf8),
        // A string is read as far as its opening quote and an object or a list as far as
        // its bracket; every other value is a single token, so reading it at all reads
        // the whole of it.
        Read::Opening => match text.as_bytes().first() {
            Some(b'"' | b'{' | b'[') => start + 1,
            _ => start + text.len(),
        },
        Read::Whole => start + text.len(),
    })
}

/// The byte offset at which `window` sits inside `text`, or `None` when it is not a
/// slice of it: both are slices of the one string, so the offset is the distance between
/// where they start.
fn offset_in(text: &str, window: &str) -> Option<usize> {
    let offset = (window.as_ptr() as usize).checked_sub(text.as_ptr() as usize)?;
    offset
        .checked_add(window.len())
        .filter(|end| *end <= text.len())
        .map(|_| offset)
}

/// The location clause a diagnostic carries for the node `path` names in `source`, with
/// `read` of it consumed.
///
/// The position is a position in the TEXT the reader holds — `source`, the policy as the
/// parse re-serialized it — and the clause names that text, so a caller reading the
/// clause can find the character the reader stopped on. A node the walk cannot follow
/// leaves the position at the end of what was read, which is the honest answer rather
/// than an invented position.
fn location(source: &str, path: &[Step], read: Read) -> String {
    let offset = offset_of(source, path, read).unwrap_or(source.len());
    let before = source.get(..offset).unwrap_or(source);
    let line = 1 + before.matches('\n').count();
    let last = before.rsplit_once('\n').map_or(before, |(_, after)| after);
    let column = 1 + last.chars().count();
    format!(
        "\n at {}",
        ::service_common::json_parse_fault::location_clause(
            &format!("(String)\"{source}\""),
            line,
            column,
        )
    )
}

/// Why one level of a policy is not one SNS would apply, or `None` when it is — counting
/// the property names it carries into `keys` as it goes.
///
/// `source` is the text the reader holds, which a diagnostic names a position in, and
/// `path` names this level in it. The walk nests as deep as the policy does, which the
/// parse that produced it already bounds: `serde_json` refuses a document nested past
/// its own recursion limit, so no accepted document reaches this deeper than that.
fn policy_refusal(
    policy: &Value,
    source: &str,
    path: &[Step],
    keys: &mut usize,
) -> Option<String> {
    let Value::Object(properties) = policy else {
        return Some(format!(
            "{MATCH_VALUE_REASON}{}",
            location(source, path, Read::Opening)
        ));
    };
    for (name, conditions) in properties {
        let at = stepped(path, Step::Key(name.clone()));
        if name == OR_OPERATOR {
            // "$or" holds when any of the policies it lists holds, so its argument is a
            // list of policies, each judged as a policy of its own. It is the ONE
            // relationship the grammar names: every other member is a property, whose
            // list is read as conditions.
            let Value::Array(alternatives) = conditions else {
                return Some(format!(
                    "\"{name}\" must be an object or an array{}",
                    location(source, &at, Read::Opening)
                ));
            };
            // The relationship holds when one of SEVERAL policies does, so a list with
            // fewer than two names no alternative to choose between.
            if alternatives.len() < 2 {
                return Some(format!(
                    "There must have at least 2 Objects in {name} relationship.{}",
                    location(source, &at, Read::Whole)
                ));
            }
            for (index, alternative) in alternatives.iter().enumerate() {
                let branch = stepped(&at, Step::Index(index));
                if let Some(reason) = policy_refusal(alternative, source, &branch, keys) {
                    return Some(reason);
                }
            }
            continue;
        }
        match conditions {
            // A property whose value is an object is a NESTED policy, applied to the
            // object that property holds; its own leaf names are counted, not this one.
            Value::Object(nested) => {
                if nested.is_empty() {
                    return Some(format!(
                        "{EMPTY_OBJECT}{}",
                        location(source, &at, Read::Whole)
                    ));
                }
                if let Some(reason) = policy_refusal(conditions, source, &at, keys) {
                    return Some(reason);
                }
            }
            Value::Array(conditions) => {
                *keys += 1;
                if conditions.is_empty() {
                    return Some(format!(
                        "{EMPTY_ARRAY}{}",
                        location(source, &at, Read::Whole)
                    ));
                }
                for (index, condition) in conditions.iter().enumerate() {
                    let term = stepped(&at, Step::Index(index));
                    if let Some(reason) = condition_refusal(condition, source, &term, name) {
                        return Some(reason);
                    }
                }
            }
            // A property's value is the list of conditions to match it against, or the
            // nested policy its own properties are matched by: a value that is neither
            // names no match at all.
            _ => {
                return Some(format!(
                    "\"{name}\" must be an object or an array{}",
                    location(source, &at, Read::Opening)
                ))
            }
        }
    }
    None
}

/// Why one condition of the property `field` is not one SNS would apply, or `None` when
/// it is: a bare value to equal, or an operator object of the documented vocabulary
/// whose argument the operator can read. `path` names the condition itself.
fn condition_refusal(
    condition: &Value,
    source: &str,
    path: &[Step],
    field: &str,
) -> Option<String> {
    let operator = match condition {
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => return None,
        Value::Object(operator) => operator,
        _ => {
            return Some(format!(
                "{MATCH_VALUE_REASON}{}",
                location(source, path, Read::Opening)
            ))
        }
    };
    let Some((name, argument)) = operator.iter().next() else {
        return Some(format!(
            "{EMPTY_OBJECT}{}",
            location(source, path, Read::Whole)
        ));
    };
    if operator.len() != 1 {
        // The reader finds this out on the SECOND match type the object names, having
        // read that name and entered its argument.
        let second = operator.keys().nth(1).map_or(name.clone(), String::clone);
        let at = stepped(path, Step::Key(second));
        return Some(format!(
            "{ONE_MATCH_TYPE_ONLY}{}",
            location(source, &at, Read::Entered)
        ));
    }
    let at = stepped(path, Step::Key(name.clone()));
    if !OPERATORS.contains(&name.as_str()) {
        // SNS names the match type it does not implement and where the reader stood when
        // it found that out: it had entered the argument the name introduced.
        return Some(format!(
            "Unrecognized match type {name}{}",
            location(source, &at, Read::Opening)
        ));
    }
    argument_refusal(name, argument, source, &at, field)
}

/// The operators `anything-but` may negate: every one the matching reads inside it,
/// which is the one place the grammar admits an operator inside an operator.
const NEGATABLE_OPERATORS: &[&str] = OPERATORS;

/// A policy value as a refusal spells it back: a string as its own text, and anything
/// else as the JSON it is.
fn spelled_out(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

/// Why `argument` is not an argument the operator `name` can read, or `None` when it is.
///
/// `path` names the argument in `source`, and `field` is the property the operator was
/// found under — the reader names both in what it states, so a caller can find the
/// condition to change.
fn argument_refusal(
    name: &str,
    argument: &Value,
    source: &str,
    path: &[Step],
    field: &str,
) -> Option<String> {
    // A type the operator does not take is decided from the argument's opening token
    // alone, which is as much of it as the reader had read.
    let bad = |what: &str| {
        Some(format!(
            "{field}: {name} takes {what}{}",
            location(source, path, Read::Opening)
        ))
    };
    match name {
        EXISTS_OPERATOR => (!argument.is_boolean()).then(|| {
            format!(
                "{EXISTS_NOT_BOOLEAN}{}",
                location(source, path, Read::Opening)
            )
        }),
        // An affix is a string, or the case-insensitive form of one.
        PREFIX_OPERATOR | SUFFIX_OPERATOR => match argument {
            Value::String(_) => None,
            Value::Object(nested) if nested.len() == 1 => nested
                .get(EQUALS_IGNORE_CASE_OPERATOR)
                .filter(|folded| folded.is_string())
                .map_or_else(
                    || bad("a string, or {\"equals-ignore-case\": \"<string>\"}"),
                    |_| None,
                ),
            _ => bad("a string, or {\"equals-ignore-case\": \"<string>\"}"),
        },
        EQUALS_IGNORE_CASE_OPERATOR => (!argument.is_string()).then(|| bad("a string")).flatten(),
        // A wildcard value is a string whose `*` stands for any run of characters, and
        // the language admits no two of them in a row.
        WILDCARD_OPERATOR => match argument.as_str() {
            Some(text) => text.find("**").map(|at| {
                format!(
                    "Consecutive wildcard characters at pos {at}{}",
                    location(source, path, Read::Whole)
                )
            }),
            None => bad("a string"),
        },
        CIDR_OPERATOR => match argument.as_str() {
            // The block is read by the tree's IP-network parser — the same one the
            // matching compares an address with — so a condition this emulator could
            // never match an address against is one it does not store.
            Some(block) if block.parse::<::ipnet::IpNet>().is_ok() => None,
            _ => bad("an IPv4 or IPv6 CIDR block"),
        },
        // The negated form takes a value, a list of values, or one nested string
        // operator — the one place the grammar admits an operator inside an operator.
        ANYTHING_BUT_OPERATOR => match argument {
            Value::String(_) | Value::Number(_) => None,
            Value::Array(options) if !options.is_empty() => options
                .iter()
                .all(|option| option.is_string() || option.is_number())
                .then_some(())
                .map_or_else(|| bad("a list of strings or a list of numbers"), |()| None),
            Value::Object(nested) if nested.len() == 1 => {
                let Some((operator, nested_argument)) = nested.iter().next() else {
                    return bad("a value, a list of values, or one nested string operator");
                };
                let at = stepped(path, Step::Key(operator.clone()));
                if !NEGATABLE_OPERATORS.contains(&operator.as_str()) {
                    return Some(format!(
                        "Unsupported {name} pattern: {operator}{}",
                        location(source, &at, Read::Opening)
                    ));
                }
                match nested_argument {
                    Value::Array(arguments) if !arguments.is_empty() => {
                        arguments.iter().enumerate().find_map(|(index, one)| {
                            let one_at = stepped(&at, Step::Index(index));
                            argument_refusal(operator, one, source, &one_at, field)
                        })
                    }
                    one => argument_refusal(operator, one, source, &at, field),
                }
            }
            _ => bad("a value, a list of values, or one nested string operator"),
        },
        // A flat list of relation/operand pairs, every one of which has to hold.
        NUMERIC_OPERATOR => {
            let Some(terms) = argument.as_array() else {
                return bad("an array of operator/value pairs");
            };
            if terms.is_empty() {
                return Some(format!("{EMPTY_ARRAY}{}", location(source, path, Read::Whole)));
            }
            for (pair_index, pair) in terms.chunks(2).enumerate() {
                let Some(spelled) = pair.first() else { continue };
                let operand_at = stepped(path, Step::Index(pair_index * 2 + 1));
                let Some(relation) = spelled
                    .as_str()
                    .filter(|relation| NUMERIC_RELATIONS.contains(relation))
                else {
                    // The list is read PAIR by pair, so the reader has taken in the whole
                    // pair by the time it can say the relation names no comparison: the
                    // position is just past the pair's last member. A pair the list stops
                    // short of an operand for ends at the list itself.
                    let reason = format!(
                        "Unrecognized numeric range operator: {}",
                        spelled_out(spelled)
                    );
                    let at = if pair.len() == 1 { path } else { &operand_at };
                    return Some(format!("{reason}{}", location(source, at, Read::Whole)));
                };
                match pair.get(1) {
                    Some(operand) if operand.is_number() => {}
                    Some(_) => {
                        return Some(format!(
                            "Value of {relation} must be numeric{}",
                            location(source, &operand_at, Read::Opening)
                        ))
                    }
                    // The list stops short of the operand, so the reader has read the
                    // whole of it before it can tell.
                    None => {
                        return Some(format!(
                            "Value of {relation} must be numeric{}",
                            location(source, path, Read::Whole)
                        ))
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// How many COMBINATIONS of values this policy comes to: the product of the conditions
/// each property lists, with the alternatives of a `$or` summed rather than multiplied —
/// a message satisfies one branch or another, so the branches add up.
fn combinations(policy: &Value) -> usize {
    match policy {
        Value::Object(properties) => properties
            .iter()
            .map(|(name, conditions)| {
                if name == OR_OPERATOR {
                    return match conditions {
                        Value::Array(alternatives) => alternatives
                            .iter()
                            .map(combinations)
                            .fold(0usize, |total, branch| total.saturating_add(branch)),
                        _ => 1,
                    };
                }
                match conditions {
                    Value::Object(_) => combinations(conditions),
                    Value::Array(conditions) => conditions.len(),
                    _ => 1,
                }
            })
            .fold(1usize, |total, property| total.saturating_mul(property)),
        _ => 1,
    }
}

/// Whether `subscription` receives `published`: its filter policy, applied under the
/// scope it holds, admits the message — and a subscription that holds no filter policy
/// receives every message published to its topic.
///
/// `budget` is what the REQUEST has left to spend on matching ([`MatchBudget`]), shared
/// across every message and subscription of that request: a subscription the budget runs
/// out on receives nothing.
pub fn admits(
    subscription: &SubscriptionRecord,
    published: &PublishedMessage,
    budget: &mut MatchBudget,
) -> bool {
    // The scope is in force only while there is a policy to apply, so the two are read
    // together through the one definition of that pairing: no policy, no filtering.
    let Some(scope) = reported_filter_policy_scope(&subscription.attributes) else {
        return true;
    };
    let Some(policy) = subscription
        .attributes
        .get(FILTER_POLICY_ATTRIBUTE)
        .and_then(|document| serde_json::from_str::<Value>(document).ok())
    else {
        // A policy the parse cannot read matches nothing it could be applied to; the
        // write paths refuse such a document, so no stored policy reaches this.
        return false;
    };
    let source = match scope {
        MESSAGE_BODY_FILTER_POLICY_SCOPE => {
            // The body this subscriber receives is what its policy reads, which for a
            // message published with a per-protocol structure is the branch that
            // structure names for this subscription's protocol.
            let body = structured_message_body(
                &published.message,
                published.message_structure.as_deref(),
                &subscription.protocol,
            );
            match serde_json::from_str::<Value>(&body) {
                Ok(body) => Source::Body(body),
                // A policy applied on the message body has nothing to read in a body
                // that is not JSON, so no property of it is satisfied.
                Err(_) => Source::Body(Value::Null),
            }
        }
        // `MessageAttributes` is the documented default and the only other scope.
        _ => Source::Attributes(&published.message_attributes),
    };
    matches_policy(&policy, &source, 0, budget)
}

/// The part of a published message a policy is applied on.
enum Source<'a> {
    /// The message's attributes, each a flat name and value.
    Attributes(&'a HashMap<String, MessageAttributeValue>),
    /// The message body, parsed as JSON — `Value::Null` for a body that is not JSON at
    /// all, which holds no property.
    Body(Value),
}

impl Source<'_> {
    /// The values the property `name` holds, or `None` when the source does not carry
    /// that property at all — which is what the `exists` operator asks about.
    ///
    /// A property may hold SEVERAL values (a `String.Array` message attribute, a JSON
    /// array in the body), and a condition matching any one of them matches the
    /// property.
    fn property(&self, name: &str) -> Option<Vec<Value>> {
        // At most [`MAX_VALUES_PER_PROPERTY`] values of the property are read, so the list
        // a caller publishes cannot grow the matching (or the copy it makes) without
        // limit.
        match self {
            Source::Attributes(attributes) => attributes.get(name).map(attribute_values),
            Source::Body(Value::Object(members)) => members.get(name).map(|value| match value {
                Value::Array(items) => items.iter().take(MAX_VALUES_PER_PROPERTY).cloned().collect(),
                other => vec![other.clone()],
            }),
            Source::Body(_) => None,
        }
    }

    /// The source a NESTED policy is applied on: the object the property `name` holds,
    /// or `None` when the property is absent or is not an object.
    ///
    /// Only a policy applied on the message body nests — a message attribute is a flat
    /// name and value, which is why a nested policy scoped to the attributes is refused
    /// when it is written.
    fn nested(&self, name: &str) -> Option<Source<'static>> {
        match self {
            Source::Body(Value::Object(members)) => match members.get(name) {
                Some(held @ Value::Object(_)) => Some(Source::Body(held.clone())),
                _ => None,
            },
            _ => None,
        }
    }
}

/// One message attribute as the values a filter policy matches it against.
///
/// "String.Array – Strings, numbers, true, false and null" (`MessageAttributeValue`
/// doc): an array-typed attribute holds every element as a value of its own, so a
/// condition matching any element matches the attribute. A `Number` is the number it
/// names, and every other textual type is its text. A `Binary` attribute carries bytes,
/// which no filter condition names, so it holds no value to match.
fn attribute_values(value: &MessageAttributeValue) -> Vec<Value> {
    let Some(text) = value.string_value.as_deref() else {
        return Vec::new();
    };
    let as_text = || vec![Value::String(text.to_string())];
    // The custom label a publisher may append to a data type does not change what the
    // value MEANS, so the type is read without it.
    match super::attribute_data_type(value) {
        super::NUMBER_DATA_TYPE => match serde_json::from_str::<Value>(text) {
            Ok(number @ Value::Number(_)) => vec![number],
            // A `Number` whose value is not a number this service can read is matched as
            // the text the publisher sent, which is all there is of it.
            _ => as_text(),
        },
        super::STRING_DATA_TYPE if value.data_type == super::STRING_ARRAY_DATA_TYPE => {
            match serde_json::from_str::<Value>(text) {
                Ok(Value::Array(items)) => {
                    items.into_iter().take(MAX_VALUES_PER_PROPERTY).collect()
                }
                _ => as_text(),
            }
        }
        super::BINARY_DATA_TYPE => Vec::new(),
        _ => as_text(),
    }
}

/// Whether `source` satisfies the policy `policy`: EVERY property the policy names is
/// satisfied by the source.
fn matches_policy(
    policy: &Value,
    source: &Source<'_>,
    depth: usize,
    budget: &mut MatchBudget,
) -> bool {
    let Value::Object(properties) = policy else {
        // A policy that is not an object names no property to match; the write paths
        // refuse such a document.
        return false;
    };
    if depth > MAX_FILTER_POLICY_DEPTH {
        return false;
    }
    properties.iter().all(|(name, conditions)| {
        // Every property judged costs the request a comparison even before its conditions
        // are read, so a policy of many properties is bounded like one of many conditions.
        if !budget.charge(1) {
            return false;
        }
        if name == OR_OPERATOR {
            // "$or" holds when any of the policies it lists holds, each judged against
            // the same source.
            return match conditions {
                Value::Array(alternatives) => alternatives
                    .iter()
                    .any(|alternative| matches_policy(alternative, source, depth + 1, budget)),
                _ => false,
            };
        }
        match conditions {
            // A property whose value is an object is a NESTED policy, matched against
            // the object that property holds.
            Value::Object(_) => source
                .nested(name)
                .is_some_and(|nested| matches_policy(conditions, &nested, depth + 1, budget)),
            Value::Array(conditions) => {
                let held = source.property(name);
                conditions
                    .iter()
                    .any(|condition| matches_condition(condition, held.as_deref(), budget))
            }
            // Anything else names no condition list, so nothing satisfies it.
            _ => false,
        }
    })
}

/// Whether one condition of a policy property holds for the values that property holds
/// (`None` when the source does not carry the property at all).
fn matches_condition(
    condition: &Value,
    held: Option<&[Value]>,
    budget: &mut MatchBudget,
) -> bool {
    // What VISITING this condition costs the request: one comparison per value it is
    // judged against. What each of those comparisons then READS — the options an
    // operator's argument lists, its relations, its bytes — is charged where it is read,
    // because that is the other half of the product the budget exists to cap.
    let cost = held.map_or(1, |values| values.len().max(1));
    if !budget.charge(cost) {
        return false;
    }
    if let Value::Object(operator) = condition {
        return matches_operator(operator, held, budget);
    }
    // A bare value is an exact match of the property's value.
    held.is_some_and(|values| values.iter().any(|value| equals(value, condition)))
}

/// Whether an operator condition holds.
///
/// `budget` is charged for what each operator READS beyond the one comparison per value
/// [`matches_condition`] already charged: the options or relations its argument lists, and
/// the bytes a string operator compares. Every factor of the matching cost is caller-chosen,
/// so each is charged where it is spent and the whole product stays under
/// [`MAX_FILTER_MATCH_COMPARISONS`] comparisons reading [`MAX_FILTER_MATCH_BYTES`] bytes.
fn matches_operator(
    operator: &serde_json::Map<String, Value>,
    held: Option<&[Value]>,
    budget: &mut MatchBudget,
) -> bool {
    let Some((name, argument)) = operator.iter().next() else {
        return false;
    };
    if operator.len() != 1 {
        // An operator object carries exactly one operator.
        return false;
    }
    if name == EXISTS_OPERATOR {
        // "exists" asks about the PROPERTY, not about its value: a property the message
        // carries satisfies `true`, and one it does not carry satisfies `false`.
        return match argument {
            Value::Bool(expected) => held.is_some() == *expected,
            _ => false,
        };
    }
    let Some(values) = held else {
        // Every other operator judges a value, and a property the message does not
        // carry has none.
        return false;
    };
    match name.as_str() {
        PREFIX_OPERATOR => values.iter().any(|value| {
            matches_affix(value, argument, budget, |held, affix| held.starts_with(affix))
        }),
        SUFFIX_OPERATOR => values.iter().any(|value| {
            matches_affix(value, argument, budget, |held, affix| held.ends_with(affix))
        }),
        EQUALS_IGNORE_CASE_OPERATOR => values.iter().any(|value| {
            let (Some(held), Some(wanted)) = (value.as_str(), argument.as_str()) else {
                return false;
            };
            // Only the argument's own length is ever compared.
            budget.charge_bytes(wanted.len()) && held.eq_ignore_ascii_case(wanted)
        }),
        WILDCARD_OPERATOR => values.iter().any(|value| {
            let (Some(held), Some(pattern)) = (value.as_str(), argument.as_str()) else {
                return false;
            };
            // The match walks the pattern and the value once each, so both lengths are
            // what it reads.
            budget.charge_bytes(pattern.len().saturating_add(held.len()))
                && ::service_common::event_pattern::matches_wildcard(pattern, held)
        }),
        // A NEGATED condition: the message is withheld when the argument matches, and
        // also when the budget ran out before the argument could be judged.
        ANYTHING_BUT_OPERATOR => values
            .iter()
            .any(|value| !matches_any(argument, value, budget) && !budget.stopped()),
        NUMERIC_OPERATOR => match argument {
            Value::Array(relations) => values
                .iter()
                .any(|value| matches_numeric(relations, value, budget)),
            _ => false,
        },
        CIDR_OPERATOR => values.iter().any(|value| {
            budget.charge(1) && super::message_filter::matches_cidr(value, argument)
        }),
        // An operator this service does not read matches nothing: a message is withheld
        // rather than delivered to a subscriber whose condition was not applied.
        _ => false,
    }
}

/// Whether `value` is one of the values `wanted` names: one value, or any of a list of
/// them — the argument form `anything-but` takes, which may itself be an operator object.
///
/// Each option the argument lists is a comparison of its own, so each is charged.
fn matches_any(wanted: &Value, value: &Value, budget: &mut MatchBudget) -> bool {
    match wanted {
        Value::Array(options) => options
            .iter()
            .any(|option| budget.charge(1) && equals(value, option)),
        Value::Object(operator) => {
            matches_operator(operator, Some(std::slice::from_ref(value)), budget)
        }
        option => budget.charge(1) && equals(value, option),
    }
}

/// Whether the string `value` begins or ends with what `argument` names, by `test`.
///
/// The argument is a string, or `{"equals-ignore-case": "…"}` — the case-insensitive
/// form of the same affix match. The bytes the comparison reads are charged to `budget`.
fn matches_affix(
    value: &Value,
    argument: &Value,
    budget: &mut MatchBudget,
    test: impl Fn(&str, &str) -> bool,
) -> bool {
    let Some(held) = value.as_str() else {
        return false;
    };
    match argument {
        // Only the affix's own length is read, whichever end it is at.
        Value::String(affix) => budget.charge_bytes(affix.len()) && test(held, affix),
        Value::Object(operator) if operator.len() == 1 => {
            match operator.get(EQUALS_IGNORE_CASE_OPERATOR).and_then(Value::as_str) {
                // The case-folded form reads both sides whole.
                Some(affix) => {
                    budget.charge_bytes(affix.len().saturating_add(held.len()))
                        && test(&held.to_ascii_lowercase(), &affix.to_ascii_lowercase())
                }
                None => false,
            }
        }
        _ => false,
    }
}

/// Whether a numeric value satisfies every relation in a `numeric` condition.
///
/// The condition is a flat list of operator/operand pairs — `["=", 2]`, `[">", 0, "<=",
/// 5]` — and ALL of its relations have to hold, which is what makes the two-pair form a
/// range. Each relation judged is a comparison of its own, so each is charged.
fn matches_numeric(relations: &[Value], value: &Value, budget: &mut MatchBudget) -> bool {
    let Some(held) = value.as_f64() else {
        return false;
    };
    let mut pairs = relations.chunks(2);
    let mut judged = false;
    for pair in &mut pairs {
        if !budget.charge(1) {
            return false;
        }
        let [relation, operand] = pair else {
            // A trailing operator with no operand names no relation.
            return false;
        };
        let (Some(relation), Some(operand)) = (relation.as_str(), operand.as_f64()) else {
            return false;
        };
        let holds = match relation {
            "=" => held == operand,
            "<" => held < operand,
            "<=" => held <= operand,
            ">" => held > operand,
            ">=" => held >= operand,
            _ => return false,
        };
        if !holds {
            return false;
        }
        judged = true;
    }
    judged
}

/// Whether a value held by the message equals a value the policy names.
///
/// A number equals a number of the same VALUE however either was spelled (`1` and `1.0`
/// are one number), and every other value is compared as JSON: a string to a string, a
/// boolean to a boolean, null to null.
fn equals(held: &Value, wanted: &Value) -> bool {
    match (held, wanted) {
        (Value::Number(held), Value::Number(wanted)) => held.as_f64() == wanted.as_f64(),
        _ => held == wanted,
    }
}
