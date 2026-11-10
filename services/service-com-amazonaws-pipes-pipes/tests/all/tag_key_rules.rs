// SPDX-License-Identifier: CC0-1.0

//! What a tag KEY and VALUE may contain, end to end over the wire.
//!
//! The merge semantics of the tagging slice live in `tests/all/pipe_tagging.rs`; this
//! suite is about the rules a tag's CONTENT must satisfy before it is stored at all, on
//! EVERY write that installs tags — `TagResource` and the `Tags` a `CreatePipe` carries:
//!
//! * `TagKey` is `@length(min: 1, max: 128)` and `TagValue` is `@length(min: 0, max: 256)`
//!   (source: the `TagMap` key and value members of
//!   `smithy-model/com-amazonaws-pipes-pipes.json`). A constraint on a map KEY is one the
//!   boundary validator does not walk, so a key outside the bound reaches the handler and
//!   has to be refused there.
//! * a tag may contain only letters, numbers, spaces and `_ . : / = + - @` (AWS's
//!   documented tag rule), so a tag carrying anything else — an emoji, a control
//!   character, punctuation the rule leaves out — is refused, while letters of any script
//!   are accepted and round-trip byte-identically.
//! * `aws:` is reserved for AWS's own tags (AWS General Reference, "Tag naming limits and
//!   requirements"), so a caller may not assign a key using that prefix, in any casing.
//!
//! Every one of those rules is enforced by the resource-tagging layer that fronts this
//! API, so a tag breaking one is refused with HTTP 400 and the error code
//! `BadRequestException` — not with the `ValidationException` the operations model for the
//! refusals the SERVICE itself states (how many tags a pipe may carry, which
//! `tests/all/pipe_tagging.rs` pins). Each row below also spells out the message the
//! broken rule is stated by, independently of the handler that built it: a broken
//! `@length` is the request-validation sentence inside the JSON document that layer's
//! message is, and a rule the model expresses nowhere is the documented sentence itself —
//! `tests/all/tagging_refusal_wording.rs` owns that rule and its envelope.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, reusing the harness
//! `pipe_lifecycle` owns and the tagging helpers `pipe_tagging` owns, and every refused
//! call is checked to have persisted NOTHING through the readers of the stored map.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::pipe_lifecycle::{create, describe, router};
use crate::pipe_tagging::{
    assert_tags_agree, create_body, pipe_arn, tag, tag_map, TAG_MUTATION_STATUS,
};
use crate::tagging_refusal_wording::tag_refusal_document;

/// `TagKey`'s published maximum (128) and minimum (1), and `TagValue`'s (256 and 0).
/// Source: the `TagKey` / `TagValue` shapes of
/// `smithy-model/com-amazonaws-pipes-pipes.json`.
const MAX_TAG_KEY_LEN: usize = 128;
const MIN_TAG_KEY_LEN: usize = 1;
const MAX_TAG_VALUE_LEN: usize = 256;
const MIN_TAG_VALUE_LEN: usize = 0;

/// The code the tagging layer refuses a broken tag rule with.
const TAG_RULE_CODE: &str = "BadRequestException";

/// The sentence AWS documents for a tag outside the allowed character set.
pub(crate) const CHARACTER_SET_SENTENCE: &str = "Tags can only contain letters, numbers, \
                                                spaces, and the following special \
                                                characters: _ . : / = + - @";

/// The fixed sentence a tag key using the reserved `aws:` prefix is refused with. It names
/// neither the key nor the resource, and its parenthetical states the rule itself.
pub(crate) const RESERVED_PREFIX_SENTENCE: &str =
    "Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)";

/// The member a refused tag is reported at, per write: `TagResource`'s request validation
/// names the tags it merges `addedTags`, `CreatePipe`'s names them as the model does.
const ADDED_TAGS: &str = "addedTags";
const CREATE_TAGS: &str = "tags";

/// How a refused row must be worded. Spelled out here rather than read from the service,
/// so what the test pins is the sentence a client sees.
#[derive(Debug, Clone)]
enum Broken {
    /// A KEY of the map is outside `TagKey`'s bound: request validation echoes the value
    /// of the MEMBER — every tag the request carried, in key order — names the member the
    /// tags arrived on, and lists both published bounds.
    KeyBound(String),
    /// A VALUE of the map is outside `TagValue`'s bound. Caught by the boundary validator
    /// (a map's values ARE walked), which knows the member only as the model names it and
    /// has only the entry that broke the bound to echo.
    ValueBound(String),
    /// The reserved key prefix, which no modelled constraint expresses: the fixed
    /// documented sentence, naming nothing from the request.
    ReservedPrefix,
    /// A character outside the documented set: the documented sentence, which names
    /// nothing from the request.
    CharacterSet,
}

impl Broken {
    /// The message this refusal carries for a write whose tags arrived on `member`.
    ///
    /// A broken `@length` is stated as request validation states one, inside the JSON
    /// document the tagging layer's message is (`tagging_refusal_wording`, which owns that
    /// envelope and the rule behind it); a rule the model expresses nowhere is the
    /// documented sentence itself, stated bare.
    fn message(&self, member: &str) -> String {
        match self {
            Self::KeyBound(shown) => tag_refusal_document(&format!(
                "1 validation error detected: Value '{shown}' at '{member}' failed to \
                 satisfy constraint: Map keys must satisfy constraint: [Member must have \
                 length less than or equal to {MAX_TAG_KEY_LEN}, Member must have length \
                 greater than or equal to {MIN_TAG_KEY_LEN}]"
            )),
            Self::ValueBound(shown) => tag_refusal_document(&format!(
                "1 validation error detected: Value '{shown}' at '{CREATE_TAGS}' failed to \
                 satisfy constraint: Map value must satisfy constraint: [Member must have \
                 length less than or equal to {MAX_TAG_VALUE_LEN}, Member must have length \
                 greater than or equal to {MIN_TAG_VALUE_LEN}]"
            )),
            Self::ReservedPrefix => RESERVED_PREFIX_SENTENCE.to_string(),
            Self::CharacterSet => CHARACTER_SET_SENTENCE.to_string(),
        }
    }
}

/// The tags of a request as a validation sentence echoes the member's value: every entry
/// in KEY order, which is the one order a refusal states them in.
pub(crate) fn shown(pairs: &[(&str, &str)]) -> String {
    let mut entries: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    entries.sort();
    format!("{{{}}}", entries.join(", "))
}

/// A value past `TagValue`'s bound as a refusal echoes it: the boundary validator elides
/// an echoed value at 256 bytes, on a character boundary, and states the full byte count,
/// so a request cannot be amplified into an arbitrarily long response.
pub(crate) fn elided(value: &str) -> String {
    format!("{}... [{} bytes]", &value[..MAX_TAG_VALUE_LEN], value.len())
}

/// A tag object from pairs whose keys are computed rather than literal (`json!` takes
/// only literal keys).
pub(crate) fn tags_of(pairs: Vec<(String, String)>) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        map.insert(key, Value::String(value));
    }
    Value::Object(map)
}

/// One tag, from borrowed parts.
pub(crate) fn one_tag(key: &str, value: &str) -> Value {
    tags_of(vec![(key.to_string(), value.to_string())])
}

/// The rows every write that installs tags must agree on: the tag, and either nothing
/// (it is accepted) or the rule it breaks.
///
/// They span what the rules generalise over: each length bound from inside and one past
/// it, at both ends; the reserved prefix in several casings and in a position where it is
/// NOT reserved; unicode keys, which are measured in characters rather than bytes; every
/// special character the documented set admits, letters of several scripts, and a
/// character outside the set in the key and in the value.
fn rows() -> Vec<(&'static str, Value, Option<Broken>)> {
    let at_key_bound = "k".repeat(MAX_TAG_KEY_LEN);
    let past_key_bound = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let at_value_bound = "v".repeat(MAX_TAG_VALUE_LEN);
    let past_value_bound = "v".repeat(MAX_TAG_VALUE_LEN + 1);
    // 128 characters, 384 bytes: `@length` counts CHARACTERS, so this is ON the bound.
    let unicode_at_key_bound = "é".repeat(MAX_TAG_KEY_LEN);
    let unicode_past_key_bound = "é".repeat(MAX_TAG_KEY_LEN + 1);

    vec![
        ("plain-key-and-value", one_tag("env", "prod"), None),
        ("shortest-key-and-empty-value", one_tag("k", ""), None),
        (
            "key-and-value-exactly-at-their-bounds",
            tags_of(vec![(at_key_bound, at_value_bound)]),
            None,
        ),
        (
            "empty-key",
            one_tag("", "prod"),
            Some(Broken::KeyBound(shown(&[("", "prod")]))),
        ),
        (
            "key-one-character-past-its-bound",
            tags_of(vec![(past_key_bound.clone(), "prod".to_string())]),
            Some(Broken::KeyBound(shown(&[(&past_key_bound, "prod")]))),
        ),
        (
            // Several tags in one request: what the sentence echoes is the whole MEMBER,
            // not only the entry that broke the rule.
            "a-legal-tag-beside-a-key-past-its-bound",
            tags_of(vec![
                ("env".to_string(), "prod".to_string()),
                (past_key_bound.clone(), "x".to_string()),
            ]),
            Some(Broken::KeyBound(shown(&[
                ("env", "prod"),
                (&past_key_bound, "x"),
            ]))),
        ),
        (
            "value-one-character-past-its-bound",
            tags_of(vec![("env".to_string(), past_value_bound.clone())]),
            Some(Broken::ValueBound(shown(&[(
                "env",
                &elided(&past_value_bound),
            )]))),
        ),
        (
            "unicode-key-exactly-at-its-bound",
            tags_of(vec![(unicode_at_key_bound, "prod".to_string())]),
            None,
        ),
        (
            "unicode-key-one-character-past-its-bound",
            tags_of(vec![(unicode_past_key_bound.clone(), "prod".to_string())]),
            Some(Broken::KeyBound(shown(&[(
                &unicode_past_key_bound,
                "prod",
            )]))),
        ),
        (
            "reserved-prefix-lower-case",
            one_tag("aws:cloudformation:stack-name", "demo"),
            Some(Broken::ReservedPrefix),
        ),
        (
            "reserved-prefix-upper-case",
            one_tag("AWS:createdBy", "demo"),
            Some(Broken::ReservedPrefix),
        ),
        (
            "reserved-prefix-mixed-case",
            one_tag("Aws:Owner", "demo"),
            Some(Broken::ReservedPrefix),
        ),
        (
            "reserved-prefix-alone",
            one_tag("aws:", "demo"),
            Some(Broken::ReservedPrefix),
        ),
        // Only the PREFIX is reserved: `aws` elsewhere in the key is an ordinary key, and
        // so is a value that starts with it.
        (
            "aws-later-in-the-key-is-ordinary",
            one_tag("team-aws:owner", "aws:demo"),
            None,
        ),
        (
            "a-legal-tag-beside-an-illegal-one",
            tags_of(vec![
                ("env".to_string(), "prod".to_string()),
                ("aws:owner".to_string(), "demo".to_string()),
            ]),
            Some(Broken::ReservedPrefix),
        ),
        // Every special character the documented set admits, in one key and one value.
        (
            "every-documented-special-character",
            one_tag("cost_center.a:b/c=d+e-f@g", "_.:/=+-@"),
            None,
        ),
        ("digits-and-spaces", one_tag("tag 1", "value 2"), None),
        // Letters are letters in every script, and must round-trip byte-identically.
        (
            "letters-of-other-scripts",
            one_tag("名前", "パイプ 製造"),
            None,
        ),
        (
            "accented-latin-letters",
            one_tag("équipe", "prôd-ação"),
            None,
        ),
        ("cyrillic-letters", one_tag("ключ", "значение"), None),
        // …and anything that is not a letter, a number, a space or one of the eight
        // special characters is refused, in the value or in the key.
        (
            "emoji-value",
            one_tag("emoji-tag", "🚀"),
            Some(Broken::CharacterSet),
        ),
        (
            "emoji-key",
            one_tag("🚀", "value"),
            Some(Broken::CharacterSet),
        ),
        (
            "punctuation-outside-the-set",
            one_tag("env", "prod!"),
            Some(Broken::CharacterSet),
        ),
        (
            "hash-outside-the-set",
            one_tag("env#1", "prod"),
            Some(Broken::CharacterSet),
        ),
        (
            "comma-outside-the-set",
            one_tag("env", "a,b"),
            Some(Broken::CharacterSet),
        ),
        (
            "newline-outside-the-set",
            one_tag("env", "a\nb"),
            Some(Broken::CharacterSet),
        ),
        (
            "a-legal-tag-beside-one-outside-the-set",
            tags_of(vec![
                ("env".to_string(), "prod".to_string()),
                ("sticker".to_string(), "🚀".to_string()),
            ]),
            Some(Broken::CharacterSet),
        ),
    ]
}

/// RULE: `TagResource` refuses a tag breaking a published tag rule — `TagKey`'s or
/// `TagValue`'s length bound, the documented character set, or the reserved key prefix —
/// with 400 `BadRequestException` stating that rule, and persists nothing for it; a tag
/// inside every rule is stored and reported back byte-identically.
#[tokio::test]
async fn tag_resource_refuses_a_tag_whose_key_or_value_breaks_the_published_rules() {
    let router = router();

    for (index, (case, tags, broken)) in rows().into_iter().enumerate() {
        let name = format!("tagrules{index}");
        let initial = json!({"kept": "yes"});
        let created = create(&router, &name, create_body(Some(&initial))).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );

        let reply = tag(&router, &pipe_arn(&name), &tags).await;
        match &broken {
            None => assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (TAG_MUTATION_STATUS, ""),
                "case {case:?}: a tag inside every rule must be accepted, got {reply:?}"
            ),
            Some(broken) => {
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (400, TAG_RULE_CODE),
                    "case {case:?}: expected 400 {TAG_RULE_CODE}, got {reply:?}"
                );
                assert_eq!(
                    reply.message(),
                    broken.message(ADDED_TAGS),
                    "case {case:?}: expected the sentence the broken rule is stated by, \
                     got {reply:?}"
                );
            }
        }

        // A refused call leaves the pipe exactly as it was; an accepted one merges.
        let mut stored = tag_map(&initial);
        if broken.is_none() {
            stored.extend(tag_map(&tags));
        }
        assert_tags_agree(&router, case, &name, &stored).await;
    }
}

/// RULE, on the other write that installs tags: `CreatePipe` enforces the same rules over
/// the `Tags` it carries, so a pipe can never be created carrying a tag `TagResource`
/// would refuse — and a refused create stores no pipe at all.
#[tokio::test]
async fn create_pipe_refuses_the_same_tags_tag_resource_refuses() {
    let router = router();

    for (index, (case, tags, broken)) in rows().into_iter().enumerate() {
        let name = format!("createrules{index}");
        let created = create(&router, &name, create_body(Some(&tags))).await;
        match &broken {
            None => assert_eq!(
                (created.status, created.error_code.as_str()),
                (200, ""),
                "case {case:?}: a tag inside every rule must be accepted, got {created:?}"
            ),
            Some(broken) => {
                assert_eq!(
                    (created.status, created.error_code.as_str()),
                    (400, TAG_RULE_CODE),
                    "case {case:?}: expected 400 {TAG_RULE_CODE}, got {created:?}"
                );
                assert_eq!(
                    created.message(),
                    broken.message(CREATE_TAGS),
                    "case {case:?}: expected the sentence the broken rule is stated by, \
                     got {created:?}"
                );
            }
        }

        if broken.is_none() {
            assert_tags_agree(&router, case, &name, &tag_map(&tags)).await;
        } else {
            let described = describe(&router, &name).await;
            assert_eq!(
                (described.status, described.error_code.as_str()),
                (404, "NotFoundException"),
                "case {case:?}: a refused create must store no pipe, got {described:?}"
            );
        }
    }
}

/// RULE: the refusal names the member the tags arrived on, which differs between the two
/// writes — `TagResource`'s request validation calls the tags it merges `addedTags`,
/// `CreatePipe`'s names them as the model does — and carries no modelled `fieldList`,
/// because the layer that refuses it has no modelled shape to put one in.
#[tokio::test]
async fn a_refused_tag_names_the_input_member_it_arrived_on() {
    let router = router();
    let name = "tagrulesmember";
    let created = create(&router, name, create_body(None)).await;
    assert_eq!(
        created.status, 200,
        "CreatePipe must succeed, got {created:?}"
    );

    let long_key = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let refused = tag(&router, &pipe_arn(name), &one_tag(&long_key, "demo")).await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, TAG_RULE_CODE),
        "TagResource must refuse an over-long key with 400 {TAG_RULE_CODE}, got {refused:?}"
    );
    // Inside the document the layer's message is, the apostrophes the sentence quotes the
    // member name with are backslash-escaped.
    assert!(
        refused.message().contains(&format!(r"at \'{ADDED_TAGS}\'")),
        "TagResource's refusal must name its own validation member, got {refused:?}"
    );
    assert_eq!(
        refused.member("fieldList"),
        &Value::Null,
        "the layer's refusal carries no modelled fieldList, got {refused:?}"
    );

    let refused_create = create(
        &router,
        "tagrulesmembercreate",
        create_body(Some(&one_tag(&long_key, "demo"))),
    )
    .await;
    assert_eq!(
        (refused_create.status, refused_create.error_code.as_str()),
        (400, TAG_RULE_CODE),
        "CreatePipe must refuse an over-long key with 400 {TAG_RULE_CODE}, \
         got {refused_create:?}"
    );
    assert!(
        refused_create.message().contains(&format!(r"at \'{CREATE_TAGS}\'")),
        "CreatePipe's refusal must name its own member, got {refused_create:?}"
    );

    // Neither refusal wrote anything.
    assert_tags_agree(&router, "untouched-after-refusals", name, &BTreeMap::new()).await;
}
