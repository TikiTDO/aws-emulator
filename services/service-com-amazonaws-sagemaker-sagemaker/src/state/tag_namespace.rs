// SPDX-License-Identifier: CC0-1.0

//! The TAG NAMESPACE slice of this service's state: which tag keys a caller may write.
//!
//! Amazon Web Services keeps the `aws:` key prefix for the tags IT generates, so such a
//! key is never one a request supplies: a call that asks SageMaker to store one is
//! refused and the resource's tag set is left exactly as it was. The prefix is a property
//! of the TAG KEY — not of one operation — so the reading and the sentence live here
//! once, beside [`crate::state::TAGS_PER_RESOURCE_QUOTA`], rather than in the handler
//! that happens to carry the tag list.
//!
//! `TagKey`'s own pattern (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`,
//! `smithy_query(query="shape",shape="TagKey")`) admits `:`, so the boundary validator
//! passes a reserved key and this is where it is refused.

/// The tag-key prefix Amazon Web Services keeps for its own tags.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] keeps.
///
/// Matched however the caller cased it: the namespace is AWS's, so `AWS:` names it as
/// surely as `aws:` does, and a case-sensitive test would be a way around the rule.
#[must_use]
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The first key of `keys`, in the order the request carries them, that
/// [`is_reserved_tag_key`] — or `None` when every key is the caller's own to write.
///
/// The FIRST is reported so one request is always refused for the same key, whatever
/// else it carries.
pub fn first_reserved_tag_key<'keys>(
    keys: impl IntoIterator<Item = &'keys str>,
) -> Option<&'keys str> {
    keys.into_iter().find(|key| is_reserved_tag_key(key))
}

/// How this service states a tag key a request may not write: the prefix the key is
/// inside, who the namespace belongs to, and the key AS THE CALLER SENT IT.
///
/// Stated once so every operation that takes a tag list words the one fact the same way.
#[must_use]
pub fn reserved_tag_key_refusal(key: &str) -> String {
    format!(
        "System prefix '{RESERVED_TAG_KEY_PREFIX}' reserved for AWS usage in tag key: {key}"
    )
}

#[cfg(test)]
mod tests {
    use super::{first_reserved_tag_key, is_reserved_tag_key, RESERVED_TAG_KEY_PREFIX};

    /// RULE: a key is reserved exactly when it OPENS with the prefix, in any casing.
    #[test]
    fn the_reserved_namespace_is_the_opening_prefix_in_any_casing() {
        // (case, key, reserved)
        let rows: [(&str, &str, bool); 10] = [
            ("the prefix alone", "aws:", true),
            ("a key AWS generates", "aws:cloudformation:stack-name", true),
            ("upper case", "AWS:created-by", true),
            ("mixed case", "Aws:CreatedBy", true),
            ("the prefix without its colon", "aws", false),
            ("the prefix inside the key", "my-aws:thing", false),
            ("a key that merely starts with the letters", "awsome", false),
            ("an ordinary key", "team", false),
            ("an empty key", "", false),
            ("a key shorter than the prefix", "aw", false),
        ];

        for (case, key, reserved) in rows {
            let answer = is_reserved_tag_key(key);
            assert!(
                answer == reserved,
                "case {case:?}: expected is_reserved_tag_key({key:?})={reserved}, got {answer}"
            );
        }
    }

    /// RULE: the FIRST reserved key of the list is the one reported, and a list holding
    /// none reports nothing.
    #[test]
    fn the_first_reserved_key_of_a_list_is_the_one_reported() {
        let first = format!("{RESERVED_TAG_KEY_PREFIX}one");
        let second = format!("{RESERVED_TAG_KEY_PREFIX}two");
        // (case, keys, the key reported)
        let rows: Vec<(&str, Vec<&str>, Option<&str>)> = vec![
            ("no keys at all", vec![], None),
            ("only the caller's own keys", vec!["team", "owner"], None),
            (
                "one reserved key after an ordinary one",
                vec!["team", first.as_str()],
                Some(first.as_str()),
            ),
            (
                "two reserved keys",
                vec![first.as_str(), second.as_str()],
                Some(first.as_str()),
            ),
            (
                "a reserved key before an ordinary one",
                vec![second.as_str(), "team"],
                Some(second.as_str()),
            ),
        ];

        for (case, keys, expected) in rows {
            let answer = first_reserved_tag_key(keys.iter().copied());
            assert_eq!(
                answer, expected,
                "case {case:?}: expected {expected:?}, got {answer:?}"
            );
        }
    }
}
