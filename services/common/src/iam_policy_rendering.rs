// SPDX-License-Identifier: CC0-1.0

//! The ONE rendering of an IAM (Aspen) policy document a service stores and reports back.
//!
//! A service that takes a resource policy as a string member does not report the bytes the
//! caller submitted: it reports its own rendering of the policy, which differs from the
//! submitted text in exactly two ways —
//!
//! 1. the whitespace BETWEEN the document's JSON tokens is gone, which is
//!    [`super::json_document::minified`]'s rule for any opaque JSON document; and
//! 2. an element whose value the policy language types as "String or Array of strings"
//!    ([`STRING_OR_ARRAY_ELEMENTS`]) is rendered as the STRING when the caller submitted a
//!    one-element array holding one string, so a one-action statement is reported with a
//!    scalar `Action`.
//!
//! WHICH elements a given policy store collapses that way is the STORE's own: a service
//! whose reads are observed to collapse the principal lists alone and to leave an `Action`
//! list as the caller wrote it names the elements it collapses
//! ([`rendered_collapsing`], [`PRINCIPAL_ELEMENTS`]) instead of taking the whole set.
//!
//! Everything else is the caller's own data and survives untouched: member order, statement
//! count, the `Statement` array itself (a one-statement policy keeps its one-element array,
//! which is why `Statement` is not one of the elements above), the type and spelling of
//! every other value, and every byte inside a string literal — whitespace, escapes and
//! non-ASCII characters included. That is what the walk below is for: re-serialising a
//! parsed value would also rewrite the member order and the number spellings the caller
//! chose.
//!
//! Whether the document is a policy at all is [`super::iam_policy`]'s reading, and it runs
//! before this: the walk here is TOTAL over arbitrary UTF-8, so text that is not a policy
//! — truncated, or holding a string literal that never closes — is simply rendered as the
//! whitespace rule alone would render it. Nothing about a document's SIZE is judged either:
//! that bound belongs to the member carrying it, which the caller enforces before storing
//! anything. The walk is one pass with no recursion, so what it costs is the length of the
//! text it was handed.
//!
//! One service carried a copy of this walk before it was stated here — the CodeArtifact
//! domain and repository permissions policy (`stored_policy_document` in that crate's
//! `state.rs`) still holds its own. Nothing new should write another: a store that reports an
//! element's values in a spelling of its own states that spelling as an [`ElementValues`] and
//! renders through [`rendered_rewriting`], so the document is still walked once.

use super::json_document::minified;

/// The policy elements whose value the policy language types as "String or Array of
/// strings", so a one-element array and the bare string say the same thing: a statement's
/// `Action`/`NotAction` and `Resource`/`NotResource`, and the principal lists inside
/// `Principal`/`NotPrincipal` (`AWS`, `Service`, `Federated`, `CanonicalUser`).
///
/// `Statement` is deliberately NOT among them: a single-statement policy is reported as a
/// one-element array, not as a bare statement object.
pub const STRING_OR_ARRAY_ELEMENTS: [&str; 8] = [
    "Action",
    "NotAction",
    "Resource",
    "NotResource",
    "AWS",
    "Service",
    "Federated",
    "CanonicalUser",
];

/// The principal lists alone: the elements a `Principal`/`NotPrincipal` map keys its values
/// by, which are the subset of [`STRING_OR_ARRAY_ELEMENTS`] that sits inside a principal.
///
/// For a store whose reads collapse a one-element principal list and report every other
/// element as the caller wrote it ([`rendered_collapsing`]).
pub const PRINCIPAL_ELEMENTS: [&str; 4] = ["AWS", "Service", "Federated", "CanonicalUser"];

/// `document` as a service renders the policy it was handed: the whitespace between its
/// JSON tokens dropped, and every [`STRING_OR_ARRAY_ELEMENTS`] element written as the
/// string it names when the caller submitted a one-element array of one string.
///
/// The rendering is a FIXED POINT — rendering what was rendered changes nothing — so a
/// caller that puts back the document it read sees the same bytes again.
#[must_use]
pub fn rendered(document: &str) -> String {
    rendered_collapsing(document, &STRING_OR_ARRAY_ELEMENTS)
}

/// What a store writes the VALUES of one element as, beside the rendering above: the
/// element's name, and what one of its values is written as.
///
/// A store whose reads report an element's values in a spelling of its own — an account
/// principal reported as that account's root ARN rather than as the bare account id the caller
/// submitted — states that spelling here, so the document is still walked once. `written_as`
/// answers `None` for a value the store leaves as the caller wrote it.
///
/// The element is read by NAME, exactly as the collapse above reads one, so a member of that
/// name is one wherever the document carries it.
pub struct ElementValues<'a> {
    /// The element whose values are written through `written_as`.
    pub element: &'a str,
    /// What one value of that element is written as, or `None` for one written as it arrived.
    pub written_as: &'a dyn Fn(&str) -> Option<String>,
}

impl ::std::fmt::Debug for ElementValues<'_> {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("ElementValues")
            .field("element", &self.element)
            .finish_non_exhaustive()
    }
}

/// [`rendered`], with the values of the one element `values` names written as it writes them —
/// wherever that element carries them, alone or in a list.
///
/// One walk does both: the whitespace and one-value-list rules above, and the store's own
/// spelling of that element's values. Rendering what was rendered changes nothing, so long as
/// `written_as` is itself a fixed point (a spelling it already wrote is left alone).
#[must_use]
pub fn rendered_rewriting(document: &str, values: &ElementValues<'_>) -> String {
    collapsed(&minified(document), &STRING_OR_ARRAY_ELEMENTS, Some(values))
}

/// [`rendered`] collapsing only the elements `elements` names: the whitespace between the
/// document's JSON tokens dropped, and a one-element array of one string written as that
/// string for those elements alone.
///
/// For a store that collapses a narrower set than the policy language's whole string-or-array
/// set — [`PRINCIPAL_ELEMENTS`] for one whose reads keep an `Action` list as it arrived.
/// Rendering what was rendered changes nothing, for any set.
#[must_use]
pub fn rendered_collapsing(document: &str, elements: &[&str]) -> String {
    collapsed(&minified(document), elements, None)
}

/// Every one-element array of one string an element of `elements` carries in `document`,
/// written as that string, and every value of the element `values` names written as it writes
/// it. `document` carries no whitespace between its tokens (it is [`minified`]'s output), so
/// what is left to walk is the tokens themselves.
fn collapsed(document: &str, elements: &[&str], values: Option<&ElementValues<'_>>) -> String {
    let bytes = document.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    // The element whose value starts at the current position: set by a string token that a
    // `:` follows, and dropped as soon as that value is reached or anything else intervenes.
    let mut element: Option<&str> = None;
    // The element whose LIST the walk is inside: every string a value element's list holds is
    // a value of that element, which is what a rewrite of its values applies to.
    let mut list: Option<&str> = None;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => {
                let end = string_end(bytes, at);
                // A string is an element NAME exactly when a `:` follows it. A literal that
                // never closed (only reachable for text that is not JSON) names nothing.
                let closed = end > at + 1 && bytes[end - 1] == b'"';
                let names_an_element = closed && bytes.get(end) == Some(&b':');
                let contents = closed.then(|| &document[at + 1..end - 1]);
                // A string no `:` follows is a VALUE, so it is the one a store that writes
                // this element's values writes.
                let written = if names_an_element {
                    None
                } else {
                    written_as(values, element.or(list), contents)
                };
                match written {
                    Some(value) => push_value(&mut out, &value),
                    None => out.push_str(&document[at..end]),
                }
                element = names_an_element.then(|| contents.unwrap_or_default());
                at = end;
            }
            b'[' if element.is_some_and(|name| elements.contains(&name)) => {
                // `["value"]` is rendered as `"value"`. Anything else — an empty array,
                // several elements, or one that is not a string — is copied as it is.
                let single = (bytes.get(at + 1) == Some(&b'"'))
                    .then(|| string_end(bytes, at + 1))
                    .filter(|end| bytes.get(*end) == Some(&b']'));
                match single {
                    Some(end) => {
                        // The one value the list held is this element's, so a store that
                        // writes its values writes this one.
                        let contents = (end > at + 2).then(|| &document[at + 2..end - 1]);
                        match written_as(values, element, contents) {
                            Some(value) => push_value(&mut out, &value),
                            None => out.push_str(&document[at + 1..end]),
                        }
                        at = end + 1;
                    }
                    None => {
                        out.push('[');
                        at += 1;
                        // The list holds this element's values, however many.
                        list = element;
                    }
                }
                element = None;
            }
            _ => {
                // Every other token is copied byte for byte. The run stops at the bytes the
                // arms above own, each of which is ASCII, so it begins and ends on a
                // character boundary even when a non-ASCII sequence sits inside it.
                let start = at;
                at += 1;
                while at < bytes.len() && !matches!(bytes[at], b'"' | b'[' | b':') {
                    at += 1;
                }
                let token = &document[start..at];
                out.push_str(token);
                // `:` is what introduces an element's value; every other token ends the
                // name's reach, and the end of a list ends its own.
                if token != ":" {
                    element = None;
                }
                if token.contains(']') {
                    list = None;
                }
            }
        }
    }
    out
}

/// What `values` writes one value of `element` as, or `None` where it writes that value as the
/// caller submitted it — including where there is no rewrite at all, where the value belongs to
/// another element, and where the literal never closed.
fn written_as(
    values: Option<&ElementValues<'_>>,
    element: Option<&str>,
    contents: Option<&str>,
) -> Option<String> {
    let values = values?;
    if element? != values.element {
        return None;
    }
    (values.written_as)(contents?)
}

/// Write one string value: its contents between the quotes a JSON string carries.
fn push_value(out: &mut String, value: &str) {
    out.push('"');
    out.push_str(value);
    out.push('"');
}

/// The index just past the JSON string literal that starts at `start` (its opening `"`),
/// honouring `\` escapes so an escaped quote does not end the literal. An unterminated
/// literal — only reachable for text that is not JSON — runs to the end of the input.
fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut at = start + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return at + 1,
            _ => at += 1,
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row: what a failing assertion calls it, the submitted document, and the document
    /// a service reports for it.
    type Row = (&'static str, &'static str, &'static str);

    /// The rows differ along what the rule generalises over: the spacing of the submitted
    /// text, WHICH element carries the one-element array, how many elements that array holds
    /// (one collapses, none and several do not), whether the element is one the policy
    /// language types as string-or-array at all, what the array holds (a string or something
    /// else), and what sits inside a string literal — text that merely looks like an element
    /// included.
    #[test]
    fn a_one_element_string_array_is_rendered_as_the_string_and_nothing_else_moves() {
        let rows: [Row; 15] = [
            (
                "a document already in the rendered form",
                r#"{"Statement":[{"Effect":"Allow","Action":"svc:A","Principal":"*"}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Action":"svc:A","Principal":"*"}]}"#,
            ),
            (
                "a space after every ':' and ','",
                r#"{"Statement": [{"Effect": "Allow", "Action": "svc:A", "Principal": "*"}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Action":"svc:A","Principal":"*"}]}"#,
            ),
            (
                "a one-element Action list",
                r#"{"Statement":[{"Effect":"Allow","Action":["svc:A"],"Principal":"*"}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Action":"svc:A","Principal":"*"}]}"#,
            ),
            (
                "one-element NotAction, Resource and NotResource lists, spaced",
                r#"{"Statement": [{"Effect": "Deny", "NotAction": ["svc:A"], "Resource": ["*"], "NotResource": ["arn:aws:svc:::thing"], "Principal": "*"}]}"#,
                r#"{"Statement":[{"Effect":"Deny","NotAction":"svc:A","Resource":"*","NotResource":"arn:aws:svc:::thing","Principal":"*"}]}"#,
            ),
            (
                "one-element principal lists inside Principal and NotPrincipal",
                r#"{"Statement":[{"Effect":"Allow","Action":"svc:A","Principal":{"AWS":["arn:aws:iam::000000000000:root"],"Service":["svc.amazonaws.com"],"Federated":["accounts.google.com"],"CanonicalUser":["abc"]}},{"Effect":"Deny","Action":"svc:B","NotPrincipal":{"AWS":["arn:aws:iam::111122223333:root"]}}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Action":"svc:A","Principal":{"AWS":"arn:aws:iam::000000000000:root","Service":"svc.amazonaws.com","Federated":"accounts.google.com","CanonicalUser":"abc"}},{"Effect":"Deny","Action":"svc:B","NotPrincipal":{"AWS":"arn:aws:iam::111122223333:root"}}]}"#,
            ),
            (
                "lists of two and of none keep their list form",
                r#"{"Statement": [{"Effect": "Allow", "Action": ["svc:A", "svc:B"], "Resource": [], "Principal": "*"}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Action":["svc:A","svc:B"],"Resource":[],"Principal":"*"}]}"#,
            ),
            (
                "a one-statement Statement array stays an array",
                r#"{ "Statement" : [ { "Effect" : "Allow" , "Action" : "svc:A" , "Principal" : "*" } ] }"#,
                r#"{"Statement":[{"Effect":"Allow","Action":"svc:A","Principal":"*"}]}"#,
            ),
            (
                "an element outside the string-or-array set keeps its one-element array",
                r#"{"Id":["one"],"Statement":[{"Effect":"Allow","Action":"svc:A","Principal":"*","Condition":{"StringEquals":{"aws:PrincipalOrgID":["o-1"]}}}]}"#,
                r#"{"Id":["one"],"Statement":[{"Effect":"Allow","Action":"svc:A","Principal":"*","Condition":{"StringEquals":{"aws:PrincipalOrgID":["o-1"]}}}]}"#,
            ),
            (
                "a one-element array holding something that is not a string",
                r#"{"Statement":[{"Effect":"Allow","Action":[7],"Resource":[{"a":"b"}],"Principal":"*"}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Action":[7],"Resource":[{"a":"b"}],"Principal":"*"}]}"#,
            ),
            (
                "text inside a string literal is never read as an element",
                r#"{"Statement": [{"Sid": "\"Action\":[\"svc:A\"]", "Effect": "Allow", "Action": ["svc:A"], "Principal": "*"}]}"#,
                r#"{"Statement":[{"Sid":"\"Action\":[\"svc:A\"]","Effect":"Allow","Action":"svc:A","Principal":"*"}]}"#,
            ),
            (
                "numbers, booleans, null and non-ASCII text keep their spelling",
                r#"{"Statement": [{"Sid": "café ☕ 日本語", "Effect": "Allow", "Action": ["svc:A"], "Principal": "*", "Condition": {"NumericEquals": {"n": 1.50, "e": 1e2}, "Bool": {"t": true, "nothing": null}}}]}"#,
                r#"{"Statement":[{"Sid":"café ☕ 日本語","Effect":"Allow","Action":"svc:A","Principal":"*","Condition":{"NumericEquals":{"n":1.50,"e":1e2},"Bool":{"t":true,"nothing":null}}}]}"#,
            ),
            ("text that is not a policy at all", "not { a policy", "not{apolicy"),
            (
                "a document truncated inside a one-element array",
                "{\"Action\": [\"svc:A\"",
                "{\"Action\":[\"svc:A\"",
            ),
            (
                "a string literal that never closes",
                "{\"Sid\": \"unterminated",
                "{\"Sid\":\"unterminated",
            ),
            ("the empty text", "", ""),
        ];

        for (case, submitted, reported) in rows {
            let actual = rendered(submitted);
            assert_eq!(
                actual, reported,
                "case {case:?}: expected {reported:?}, got {actual:?}"
            );
            // The rendering is a fixed point: a caller that puts back what it read stores
            // the same document again.
            let again = rendered(&actual);
            assert_eq!(
                again, actual,
                "case {case:?}: the rendered form must be a fixed point, got {again:?}"
            );
        }
    }

    /// RULE: a store names the elements it collapses, and an element outside that set keeps
    /// the form the caller submitted.
    #[test]
    fn only_the_named_elements_collapse() {
        // (case, the elements the store collapses, the submitted document, the rendering)
        let rows: [(&str, &[&str], &str, &str); 4] = [
            (
                "the principal lists alone: Action keeps its list",
                &PRINCIPAL_ELEMENTS,
                r#"{"Statement": [{"Effect": "Allow", "Principal": {"AWS": ["arn:aws:iam::000000000000:root"]}, "Action": ["svc:A"], "Resource": ["*"]}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":["svc:A"],"Resource":["*"]}]}"#,
            ),
            (
                "the whole string-or-array set: Action collapses too",
                &STRING_OR_ARRAY_ELEMENTS,
                r#"{"Statement": [{"Effect": "Allow", "Principal": {"AWS": ["arn:aws:iam::000000000000:root"]}, "Action": ["svc:A"], "Resource": ["*"]}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"svc:A","Resource":"*"}]}"#,
            ),
            (
                "no element at all: the whitespace rule alone",
                &[],
                r#"{"Action": ["svc:A"]}"#,
                r#"{"Action":["svc:A"]}"#,
            ),
            (
                "a principal list of two, which no set collapses",
                &PRINCIPAL_ELEMENTS,
                r#"{"Principal": {"AWS": ["a", "b"]}}"#,
                r#"{"Principal":{"AWS":["a","b"]}}"#,
            ),
        ];

        for (case, elements, submitted, reported) in rows {
            let actual = rendered_collapsing(submitted, elements);
            assert_eq!(
                actual, reported,
                "case {case:?}: expected {reported:?}, got {actual:?}"
            );
            let again = rendered_collapsing(&actual, elements);
            assert_eq!(
                again, actual,
                "case {case:?}: the rendered form must be a fixed point, got {again:?}"
            );
        }
    }

    /// RULE: a store that reports one element's values in a spelling of its own writes each of
    /// them through that spelling — wherever the element carries them, alone or in a list —
    /// and leaves every other element's values as the caller wrote them.
    #[test]
    fn only_the_named_elements_values_are_rewritten() {
        // The store's own spelling, for the rows below: an account named by its id alone is
        // written as that account's root ARN, and every other value is left as it arrived.
        let account_root = |value: &str| {
            (value.len() == 12 && value.bytes().all(|digit| digit.is_ascii_digit()))
                .then(|| format!("arn:aws:iam::{value}:root"))
        };
        let values = ElementValues {
            element: "AWS",
            written_as: &account_root,
        };

        // The rows differ along what the rule generalises over: where the element's value sits
        // (alone, in a one-element list that also collapses, in a list of several), whether
        // the spelling applies to it at all, and whether the same text under ANOTHER element
        // is left alone.
        let rows: [Row; 7] = [
            (
                "an account named by its id alone",
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"210987654321"},"Action":"svc:A","Resource":"*"}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:root"},"Action":"svc:A","Resource":"*"}]}"#,
            ),
            (
                "a one-element list, which collapses and is written too",
                r#"{"Principal": {"AWS": ["210987654321"]}}"#,
                r#"{"Principal":{"AWS":"arn:aws:iam::210987654321:root"}}"#,
            ),
            (
                "a list of several, each written",
                r#"{"Principal": {"AWS": ["210987654321", "arn:aws:iam::000000000000:root", "123456789012"]}}"#,
                r#"{"Principal":{"AWS":["arn:aws:iam::210987654321:root","arn:aws:iam::000000000000:root","arn:aws:iam::123456789012:root"]}}"#,
            ),
            (
                "a value the spelling does not apply to",
                r#"{"Principal":{"AWS":"*"},"NotPrincipal":{"AWS":"arn:aws:iam::000000000000:user/u"}}"#,
                r#"{"Principal":{"AWS":"*"},"NotPrincipal":{"AWS":"arn:aws:iam::000000000000:user/u"}}"#,
            ),
            (
                "the same text under another element",
                r#"{"Statement":[{"Sid":"210987654321","Principal":{"Service":"210987654321"},"Action":"svc:210987654321"}]}"#,
                r#"{"Statement":[{"Sid":"210987654321","Principal":{"Service":"210987654321"},"Action":"svc:210987654321"}]}"#,
            ),
            (
                "the element's own name inside a string literal",
                r#"{"Sid": "\"AWS\":\"210987654321\"","Principal": {"AWS": "210987654321"}}"#,
                r#"{"Sid":"\"AWS\":\"210987654321\"","Principal":{"AWS":"arn:aws:iam::210987654321:root"}}"#,
            ),
            ("the empty text", "", ""),
        ];

        for (case, submitted, reported) in rows {
            let actual = rendered_rewriting(submitted, &values);
            assert_eq!(
                actual, reported,
                "case {case:?}: expected {reported:?}, got {actual:?}"
            );
            // The rendering stays a fixed point: the spelling it wrote is one it leaves alone.
            let again = rendered_rewriting(&actual, &values);
            assert_eq!(
                again, actual,
                "case {case:?}: the rendered form must be a fixed point, got {again:?}"
            );
        }
    }
}
