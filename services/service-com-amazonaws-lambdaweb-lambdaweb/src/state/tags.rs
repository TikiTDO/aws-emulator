// SPDX-License-Identifier: CC0-1.0

//! The tag slice of this service's state.
//!
//! A web function's tags live in that function's own record
//! (`WebFunction::tags`), so `CreateWebFunction`, `TagResource`, `UntagResource`
//! and `ListTags` read and write ONE copy of the map and a function's delete
//! takes its tags with it. The two write paths share the rules below — the
//! reserved key namespace and the stored-map bound — rather than each carrying
//! its own copy.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::ComAmazonawsLambdawebLambdawebState;

/// The tag key namespace a caller may not write to.
///
/// AWS reserves tag keys beginning with `aws:` for its own use: a caller can
/// neither create a tag whose key carries it nor change or remove one, on any
/// resource of any service. The model cannot express the rule — `TagKey`'s
/// pattern `^[a-zA-Z0-9 _\.:/=+\-@]+$` admits `:` (source:
/// `smithy_query(query="shape", shape="TagKey")`) — so the request boundary
/// passes such a key and the handlers apply the rule themselves.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// AWS's `ValidationException` message for a tag key set this service refuses
/// itself.
///
/// One FIXED sentence with no variable parts: it covers every rule it lists —
/// the reserved key namespace ([`RESERVED_TAG_KEY_PREFIX`]), an empty key and a
/// key named twice — and it names neither the offending key nor the member it
/// arrived in. That is deliberately NOT the framework's per-violation clause,
/// which quotes the value and its member path: those rules are the request
/// boundary's, judged against the model, while this one is the service's own and
/// is stated in the service's own words.
pub const INVALID_TAG_KEYS_MESSAGE: &str = "One or more of your tags are invalid. Please ensure \
     none of your tag keys start with \"aws:\", none are empty, and there are no duplicate tag \
     keys.";

/// The most tags one web function holds.
///
/// Cited, not chosen: `Tags` is `@length(min: 0, max: 200)` and
/// `ListTagsResponse$tags` targets it (source: `smithy_query(query="shape",
/// shape="Tags")`), so a function holding more tags than this could not be
/// reported by its own reader. A merge that would cross it is refused instead,
/// with the quota error `TagResource` models.
pub const MAX_TAGS_PER_FUNCTION: usize = 200;

/// Whether `key` names a tag AWS owns rather than one the caller may write.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.starts_with(RESERVED_TAG_KEY_PREFIX)
}

/// Why a tag MAP a request carries may not be applied, or `None` when every key
/// of it is the caller's to write.
///
/// The WHOLE key set is judged before anything is written, so a request carrying
/// one reserved key among valid ones applies none of them — and, since the
/// refusal states one fixed sentence, which key tripped it changes neither the
/// answer nor the order the keys are looked at in.
pub fn tag_map_violation<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
    keys.into_iter()
        .any(is_reserved_tag_key)
        .then(|| INVALID_TAG_KEYS_MESSAGE.to_string())
}

/// Why a tag KEY LIST a request carries may not be applied, or `None` when every
/// element of it is the caller's to remove.
///
/// The same rule and the same sentence as the map path: the removal path reads
/// the keys from a list rather than from a map, and nothing else differs.
pub fn tag_key_list_violation(keys: &[String]) -> Option<String> {
    tag_map_violation(keys.iter().map(String::as_str))
}

/// The stored map as the modelled `Tags` shape: `HashMap`, since that is what the
/// generated input and output structures carry.
pub fn modeled_tag_map(tags: &BTreeMap<String, String>) -> HashMap<String, String> {
    tags.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

impl ComAmazonawsLambdawebLambdawebState {
    /// The tags stored for `function_name`, or `None` when no such function is
    /// stored.
    ///
    /// A function that carries no tags answers an EMPTY map, which is a different
    /// answer from `None`: the function exists and holds nothing, and `Tags`
    /// admits the empty map (`@length(min: 0)`).
    pub fn tags(&self, function_name: &str) -> Option<&BTreeMap<String, String>> {
        self.functions.get(function_name).map(|f| &f.tags)
    }

    /// How many tags `function_name` would hold once `keys` were merged into its
    /// stored map, or `None` when no such function is stored.
    ///
    /// A key the function already carries is overwritten rather than added, so it
    /// is counted once — which is what keeps re-tagging an existing key clear of
    /// the bound in [`MAX_TAGS_PER_FUNCTION`].
    pub fn tag_count_after_merge<'a>(
        &self,
        function_name: &str,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> Option<usize> {
        let stored = &self.functions.get(function_name)?.tags;
        // Only the keys the function does not already carry add to the count, and a
        // key named twice adds once.
        let added: BTreeSet<&str> = keys
            .into_iter()
            .filter(|key| !stored.contains_key(*key))
            .collect();
        Some(stored.len() + added.len())
    }

    /// Merge `tags` into `function_name`'s stored map, returning how many tags the
    /// function holds afterwards, or `None` when no such function is stored.
    ///
    /// A merge, not a replacement: a key the request names takes the value it
    /// carries ("If a tag key already exists, the existing value is overwritten
    /// with the new value.", source: `smithy_query(query="doc",
    /// shape="TagResource")`) and every key it does not name is left exactly as it
    /// was. The function's own `updatedAt` does not move: tags are addressed as
    /// their own sub-resource, like the resource-based policy, and the function's
    /// configuration is unchanged.
    pub fn merge_tags(
        &mut self,
        function_name: &str,
        tags: impl IntoIterator<Item = (String, String)>,
    ) -> Option<usize> {
        let function = self.functions.get_mut(function_name)?;
        function.tags.extend(tags);
        Some(function.tags.len())
    }

    /// Remove every key of `keys` from `function_name`'s stored map, returning how
    /// many of them were actually there, or `None` when no such function is
    /// stored.
    ///
    /// A key the function does not carry is passed over rather than refused, which
    /// is what makes the removal `@idempotent` as the model declares it: repeating
    /// it, or naming a key that was never set, leaves the same stored map.
    pub fn remove_tags<'a>(
        &mut self,
        function_name: &str,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> Option<usize> {
        let function = self.functions.get_mut(function_name)?;
        let removed = keys
            .into_iter()
            .filter(|key| function.tags.remove(*key).is_some())
            .count();
        Some(removed)
    }
}
