// SPDX-License-Identifier: CC0-1.0

//! EventBridge's INPUT TRANSFORMATION: what a rule's target receives when the
//! target does not take the whole event envelope.
//!
//! Moved out of `crate::state` unchanged, so one file holds one concern; the
//! rule it implements is stated where the module is declared
//! ([`crate::state`]).

use ::serde_json::Value;

/// The prefix EventBridge reserves for the placeholders it fills itself:
/// "the keys cannot start with `Amazon Web Services.`"
/// (`InputTransformer$InputPathsMap`), which is the `aws.` namespace on the
/// wire — so a key in it is never a caller's path and is always one of
/// [`reserved`]'s.
const RESERVED_KEY_PREFIX: &str = "aws.";

/// The placeholders EventBridge fills from the DELIVERY rather than from the
/// event: which rule matched, under which ARN, and when the event was
/// ingested.
#[derive(Debug, Clone, Copy)]
pub struct Delivery<'a> {
    /// The name of the rule whose target is being delivered to.
    pub rule: &'a str,
    /// That rule's ARN.
    pub rule_arn: &'a str,
    /// The instant the event carries, as the envelope's `time` renders it.
    pub ingestion_time: &'a str,
}

/// What the reserved placeholder `key` is filled with, or `None` when the
/// reserved namespace holds no such placeholder.
///
/// The five EventBridge fills itself: the whole event (`aws.events.event`), the
/// whole event as JSON text (`aws.events.event.json`), the matched rule's ARN and
/// name, and the event's ingestion time.
///
/// A placeholder standing for the whole EVENT carries that event's JSON DOCUMENT
/// wherever it stands — [`Fill::Document`] — because the event is a JSON object
/// and a reading of one as characters is not a thing any consumer is handed: a
/// template that is nothing but `<aws.events.event.json>` renders the event, which
/// is what the placeholder is for. The other three carry text a caller may put
/// anywhere, so they fill like a selected value does.
fn reserved(key: &str, event: &Value, delivery: &Delivery<'_>) -> Option<Fill> {
    match key {
        "aws.events.event" | "aws.events.event.json" => {
            Some(Fill::Document(event.to_string()))
        }
        "aws.events.rule-arn" => Some(Fill::Selected(Value::String(
            delivery.rule_arn.to_string(),
        ))),
        "aws.events.rule-name" => Some(Fill::Selected(Value::String(
            delivery.rule.to_string(),
        ))),
        "aws.events.event.ingestion-time" => Some(Fill::Selected(Value::String(
            delivery.ingestion_time.to_string(),
        ))),
        _ => None,
    }
}

/// What ONE placeholder is filled with, told apart by how it is WRITTEN into the
/// template.
#[derive(Debug, Clone)]
enum Fill {
    /// A value the event carries: its JSON text where the placeholder stands as a
    /// JSON value, and its characters ([`text`]) where it stands inside a string
    /// or in a template that is not JSON at all.
    Selected(Value),
    /// A JSON document, which is written as it stands wherever the placeholder is
    /// — the whole event is one.
    Document(String),
}

/// What a path selected, told apart the way a RENDERING has to tell it
/// apart: the value standing at the place the path named, and the absence of
/// any such place.
///
/// The two are different facts about the event — "the member is there and
/// holds `null`" is not "the event has no such member" — and a placeholder
/// stands for a different thing in each ([`render`]), so the selection
/// carries which of them it was rather than collapsing both onto one value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// The value at the one place the path named, or the list of what a slice
    /// in it spans.
    Value(Value),
    /// The path named no place this event carries.
    Nothing,
}

/// What `path` selects out of `event`: the value at the one place the path
/// names, the list of what a slice or wildcard in it spans, or
/// [`Selection::Nothing`] for a path that selects NO place of this event.
///
/// `Err` is the path this service did NOT evaluate: a form the bounded
/// reader does not read, or a selection past the places it selects. That is
/// not a path that selected nothing, so it is never rendered as an absent
/// value — the caller refuses the delivery instead of handing the target a
/// payload built from a path nobody evaluated. The message names the path,
/// which is what a consumer of the invocation log has to see to fix it.
pub fn selection(path: &str, event: &Value) -> Result<Selection, String> {
    use ::service_common::json_reference_path::{select_many, selection_segments, Selected};

    let Some(named) = selection_segments(path) else {
        return Err(format!(
            "the path {path} is not one this emulator evaluates"
        ));
    };
    match select_many(&named, event) {
        Selected::Value(value) => Ok(Selection::Value(value)),
        Selected::NoMatch => Ok(Selection::Nothing),
        Selected::NotSelected => Err(format!(
            "the path {path} names more places than the {} this emulator selects",
            ::service_common::json_reference_path::MAX_SELECTED_ELEMENTS
        )),
    }
}

/// [`selection`] as ONE value: a path that selects no place of the event
/// selects JSON's empty value, `null`.
///
/// This is the reading a target carrying `InputPath` is handed ([`by_input_path`]),
/// where what travels is the JSON TEXT of the selected part of the event and
/// an absent part has no text of its own. It is NOT how a placeholder of an
/// `InputTransformer` template is filled, which is a substitution rather than
/// a value and is stated at [`render`].
pub fn selected(path: &str, event: &Value) -> Result<Value, String> {
    selection(path, event).map(|selection| match selection {
        Selection::Value(value) => value,
        Selection::Nothing => Value::Null,
    })
}

/// What a target carrying `InputPath` receives: the JSON text of the one
/// part of the event that path selects — and `Err` for a path this service
/// did not evaluate ([`selected`]).
pub fn by_input_path(path: &str, event: &Value) -> Result<String, String> {
    selected(path, event).map(|selected| selected.to_string())
}

/// What a target carrying an `InputTransformer` receives: `template` with
/// each `<key>` placeholder replaced by what `paths` selects for that key.
///
/// The template's own FORM decides how a value is written, because the
/// documented templates are of two kinds:
///
/// * one that opens a JSON OBJECT or ARRAY carries its placeholders "in quotes
///   or out" (`InputTransformer$InputTemplate`), so a placeholder standing as a
///   JSON value carries the selected value's JSON text and one standing inside
///   one of the template's own strings carries that value's CHARACTERS
///   ([`text`]), escaped for the string it lands in — `{"n": <n>}` renders a
///   number and `{"n": "<n>"}` renders that number's digits as a string, and
///   neither can break the template's own JSON;
/// * any other template is PLAIN TEXT (`"<instance> is in state <status>"`, a
///   sentence): every placeholder carries the value's characters AS THEY READ,
///   nothing escapes them, and every character the template itself wrote — the
///   quotes a quoted template opens and closes with included — is delivered as
///   written. So such a rendering is a JSON document only where the characters
///   substituted into it happen not to break one, and an input that is not a
///   document is never handed to the target
///   ([`crate::ops::put_events::deliver`] fails that invocation instead).
///
/// A placeholder standing for the whole EVENT carries that event's JSON document
/// ([`Fill::Document`]), escaped where it stands inside a structured template's
/// own string, for the reason [`reserved`] gives.
///
/// A key the map does not hold and the reserved namespace does not name is
/// not a placeholder at all: the text is left exactly as the template wrote
/// it, since nothing says what it stands for.
///
/// A placeholder whose path selects NO place of the event is filled with the
/// EMPTY STRING, so `{"k": <absent>}` renders the JSON string `""` and
/// `{"k": "<absent>"}` renders an empty string too — which is not what a
/// placeholder for a place that IS there and holds `null` renders as (`null`
/// unquoted, the characters `null` inside a string). The two are different
/// facts about the event ([`Selection`]), so a consumer of the payload can
/// tell "no such element" from "the element was null".
///
/// `Err` is a transformation this service will not build: a path it did not
/// evaluate ([`selected`]), or a rendering past [`MAX_RENDERED_BYTES`] — a
/// template may repeat one placeholder as often as its own length allows and
/// each fill carries a value as large as the event, so the PRODUCT of the
/// two is what that bound refuses. Either way nothing is delivered rather
/// than a payload the target never asked for.
pub fn render(
    template: &str,
    paths: &::std::collections::BTreeMap<String, String>,
    event: &Value,
    delivery: &Delivery<'_>,
) -> Result<String, String> {
    // A template that opens a JSON object or array is one whose own JSON the
    // rendering keeps; anything else — a quoted string, a sentence — is plain
    // text, which escapes nothing (see above).
    let structured = template.trim_start().starts_with(['{', '[']);
    let mut rendered = String::with_capacity(template.len());
    // Whether the scan stands inside a string literal of a STRUCTURED
    // template, and whether the character before it escaped this one, which
    // is what tells a closing quote from an escaped one. A text template has
    // no JSON structure to stand inside.
    let mut in_string = false;
    let mut escaped = false;
    let mut rest = template;
    while !rest.is_empty() {
        // A placeholder is read before the character opening it is
        // classified: `<` is never part of a template's own JSON structure.
        if let Some((key, tail)) = placeholder(rest).filter(|_| !escaped) {
            if let Some(fill) = filling(key, paths, event, delivery)? {
                match fill {
                    // A JSON document stands as it is where the template takes a
                    // value, and as its characters — escaped for the string it
                    // lands in — inside one.
                    Fill::Document(document) if structured && in_string => {
                        rendered.push_str(&escaped_for_a_string(&document));
                    }
                    Fill::Document(document) => rendered.push_str(&document),
                    Fill::Selected(value) if structured && !in_string => {
                        rendered.push_str(&value.to_string());
                    }
                    Fill::Selected(value) if structured => {
                        rendered.push_str(&escaped_for_a_string(&text(&value)));
                    }
                    Fill::Selected(value) => rendered.push_str(&text(&value)),
                }
                if rendered.len() > MAX_RENDERED_BYTES {
                    return Err(format!(
                        "the transformation renders more than the {MAX_RENDERED_BYTES} \
                         bytes this emulator renders"
                    ));
                }
                rest = tail;
                continue;
            }
        }
        let character = rest.chars().next().unwrap_or_default();
        if structured {
            if in_string {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_string = false;
                }
            } else if character == '"' {
                in_string = true;
            }
        }
        rendered.push(character);
        rest = &rest[character.len_utf8()..];
    }
    Ok(rendered)
}

/// The most bytes ONE rendering may produce.
///
/// This service's own bound, not a limit AWS publishes: `InputTemplate` is
/// modelled at 8192 characters and each placeholder in it may carry a value
/// as large as a published entry ([`super::MAX_ENTRY_SIZE_BYTES`]), so their
/// product is not bounded by either. One rendering is one target's payload,
/// so it is held to the size of the event it was built from.
pub const MAX_RENDERED_BYTES: usize = super::MAX_ENTRY_SIZE_BYTES;

/// What the placeholder `key` is filled with: what the caller's own path for
/// it selects, or the reserved placeholder of that name — `Ok(None)` when
/// neither names it, which is what leaves the template's text alone, and
/// `Err` for a path this service did not evaluate ([`selected`]).
fn filling(
    key: &str,
    paths: &::std::collections::BTreeMap<String, String>,
    event: &Value,
    delivery: &Delivery<'_>,
) -> Result<Option<Fill>, String> {
    if let Some(path) = paths.get(key) {
        return selection(path, event).map(|selection| {
            Some(Fill::Selected(match selection {
                Selection::Value(value) => value,
                // Nothing was selected, so the placeholder stands for no
                // characters at all: the empty string, which is a JSON string
                // where the placeholder stands as a value and nothing where it
                // stands inside one.
                Selection::Nothing => Value::String(String::new()),
            }))
        });
    }
    Ok(key
        .starts_with(RESERVED_KEY_PREFIX)
        .then(|| reserved(key, event, delivery))
        .flatten())
}

/// The key the placeholder at the head of `text` names, with the text after
/// it — `None` when `text` does not open one.
///
/// A placeholder is `<key>` with no `<`, `>` or newline inside it, so an
/// unterminated `<` and a `<` that merely occurs in prose are left alone.
fn placeholder(text: &str) -> Option<(&str, &str)> {
    let after = text.strip_prefix('<')?;
    let end = after.find('>')?;
    let key = &after[..end];
    if key.is_empty() || key.contains(['<', '\n', '\r', '"']) {
        return None;
    }
    Some((key, &after[end + 1..]))
}

/// The characters a selected value contributes to TEXT: what the value holds,
/// with no JSON string quoting at any level.
///
/// A string contributes its own characters, a number and a boolean their
/// digits and words, `null` the four characters `null` — and a container the
/// same reading applied to what it holds: `[` its elements, `,`-separated, `]`
/// and `{` its members as `<name>:<value>`, `,`-separated, `}`. So an array of
/// strings reads `[a,b]` rather than `["a","b"]`: where a placeholder carries
/// CHARACTERS rather than a JSON value, nothing of JSON's own punctuation for
/// quoting a string is part of them, however deep in the value the string sits.
///
/// This is the one reading of a selected value as text, so a sentence template
/// and a placeholder standing inside a JSON template's own string (where it is
/// escaped for that string, [`escaped_for_a_string`]) carry the same characters.
/// A placeholder standing as a JSON VALUE carries the value's JSON text instead —
/// that one is a value, not characters — which is where the quotes come back.
///
/// The walk descends one frame per level of the value, which is bounded by the
/// nesting the event was admitted under: a `Detail` is charged against the
/// published [`super::MAX_DETAIL_DEPTH`] BEFORE anything parses it
/// ([`super::parse_detail`]), so no caller can present a value deeper than that
/// here — the same bound `serde_json`'s own rendering of such a value relies on.
fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(elements) => {
            let read: Vec<String> = elements.iter().map(text).collect();
            format!("[{}]", read.join(","))
        }
        Value::Object(members) => {
            let read: Vec<String> = members
                .iter()
                .map(|(name, member)| format!("{name}:{}", text(member)))
                .collect();
            format!("{{{}}}", read.join(","))
        }
        other => other.to_string(),
    }
}

/// `characters` escaped for the JSON string literal they are written into, so a
/// selected value carrying a quote, a backslash or a control character cannot
/// break the template's own JSON.
fn escaped_for_a_string(characters: &str) -> String {
    let quoted = Value::String(characters.to_string()).to_string();
    // `Value::String`'s rendering is the QUOTED form, and what the
    // placeholder stands for is its content: the quotes around it belong to
    // the template's own string, which already carries them.
    quoted
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(&quoted)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{render, selected, Delivery};


    use ::serde_json::json;

    /// The event every row below transforms.
    fn event() -> ::serde_json::Value {
        json!({
            "id": "e-1",
            "detail-type": "Order Created",
            "source": "com.demo",
            "time": "2026-01-01T00:00:00Z",
            "detail": {
                "order": "A-1",
                "amount": 7,
                "items": ["a", "b", "c"],
                "lines": [{ "sku": "s1" }, { "sku": "s2" }],
                "nul": null,
                "quoted": "he said \"hi\"",
            },
        })
    }

    fn delivery() -> Delivery<'static> {
        Delivery {
            rule: "r",
            rule_arn: "arn:aws:events:eu-west-2:000000000000:rule/r",
            ingestion_time: "2026-01-01T00:00:00Z",
        }
    }

    /// What a path selects, over a TABLE whose rows differ along the KIND of
    /// place the path names: a member, an element, a slice, a place the
    /// event does not carry, and a path this service does not evaluate at
    /// all — which is NOT a path that selected nothing.
    #[test]
    fn a_path_selects_its_place_and_an_absent_one_selects_null() {
        // (case, path, the value it selects, `None` for a path this service
        // did not evaluate)
        let rows = [
            ("a member", "$.detail.order", Some(json!("A-1"))),
            ("a number", "$.detail.amount", Some(json!(7))),
            (
                "a hyphenated member",
                "$.detail-type",
                Some(json!("Order Created")),
            ),
            ("an element", "$.detail.items[1]", Some(json!("b"))),
            ("a slice", "$.detail.items[0:2]", Some(json!(["a", "b"]))),
            ("an open slice", "$.detail.items[1:]", Some(json!(["b", "c"]))),
            ("an absent member", "$.detail.nope", Some(json!(null))),
            (
                "an element past the array",
                "$.detail.items[9]",
                Some(json!(null)),
            ),
            // Each of these is a form the bounded reader declines, so the
            // selection is REFUSED rather than answered with the empty
            // value: nothing was evaluated, so nothing may be rendered.
            ("a wildcard member", "$.detail.*", None),
            ("a descendant", "$..order", None),
            ("a union", "$.detail.items[0,1]", None),
            ("a filter", "$.detail.items[?@=='a']", None),
            ("a negative index", "$.detail.items[-1]", None),
            ("a slice with a step", "$.detail.items[0:2:1]", None),
            ("text that is not a path", "detail.order", None),
        ];
        let event = event();
        for (case, path, expected) in rows {
            let actual = selected(path, &event);
            assert_eq!(
                actual.clone().ok(),
                expected,
                "case {case:?}: expected {path} to select {expected:?}, got {actual:?}"
            );
            if expected.is_none() {
                assert!(
                    actual.is_err_and(|refusal| refusal.contains(path)),
                    "case {case:?}: the refusal names the path that was not evaluated"
                );
            }
        }
    }

    /// How a placeholder renders, over a TABLE whose rows differ along where
    /// the placeholder stands (in a string or out), what the path selects,
    /// and whether the template is JSON at all.
    #[test]
    fn a_placeholder_renders_the_value_where_it_stands() {
        let paths: ::std::collections::BTreeMap<String, String> = [
            ("order", "$.detail.order"),
            ("amount", "$.detail.amount"),
            ("items", "$.detail.items[0:2]"),
            ("lines", "$.detail.lines"),
            ("absent", "$.detail.nope"),
            ("beyond", "$.detail.items[9]"),
            ("nul", "$.detail.nul"),
            ("quoted", "$.detail.quoted"),
        ]
        .into_iter()
        .map(|(key, path)| (key.to_string(), path.to_string()))
        .collect();
        let rows = [
            ("a bare value", "{\"n\": <amount>}", "{\"n\": 7}"),
            (
                "a value in a string",
                "{\"n\": \"<amount>\"}",
                "{\"n\": \"7\"}",
            ),
            (
                "a string in a string",
                "{\"o\": \"<order>\"}",
                "{\"o\": \"A-1\"}",
            ),
            ("a bare string", "{\"o\": <order>}", "{\"o\": \"A-1\"}"),
            ("a slice", "{\"i\": <items>}", "{\"i\": [\"a\",\"b\"]}"),
            // A path that selects no place of the event fills the
            // placeholder with the empty string, wherever it stands.
            ("an absent path", "{\"a\": <absent>}", "{\"a\": \"\"}"),
            (
                "an absent path in a string",
                "{\"a\": \"<absent>\"}",
                "{\"a\": \"\"}",
            ),
            (
                "an index past the array",
                "{\"a\": <beyond>}",
                "{\"a\": \"\"}",
            ),
            (
                "an absent element in a sentence",
                "before <absent> after",
                "before  after",
            ),
            // A place that IS there and holds `null` is the other case the
            // rendering keeps apart: the literal unquoted, its characters
            // quoted.
            ("a null path", "{\"n\": <nul>}", "{\"n\": null}"),
            ("a null path in a string", "{\"n\": \"<nul>\"}", "{\"n\": \"null\"}"),
            (
                // The quotes the selected value carries are escaped for the
                // string the placeholder stands in, so the template stays the
                // JSON it was written as.
                "a value carrying quotes",
                "{\"q\": \"<quoted>\"}",
                "{\"q\": \"he said \\\"hi\\\"\"}",
            ),
            (
                // A TEXT template escapes nothing: the value's characters are
                // written as they read, which is what can leave a rendering that
                // is no JSON document at all.
                "a value carrying quotes in a sentence",
                "he told me: <quoted>",
                "he told me: he said \"hi\"",
            ),
            (
                "a quoted-string template keeps its own quoting",
                "\"<order>\"",
                "\"A-1\"",
            ),
            (
                // A selected list written inside the template's own string
                // carries its CHARACTERS, where nothing of JSON's quoting of
                // a string is part of them.
                "a list inside the template's own string",
                "\"<items>\"",
                "\"[a,b]\"",
            ),
            (
                "a list in a sentence",
                "items: <items>",
                "items: [a,b]",
            ),
            (
                "a list of objects in a sentence",
                "lines: <lines>",
                "lines: [{sku:s1},{sku:s2}]",
            ),
            (
                "a sentence template",
                "<order> is for <amount>",
                "A-1 is for 7",
            ),
            (
                "a reserved placeholder",
                "{\"r\": \"<aws.events.rule-name>\"}",
                "{\"r\": \"r\"}",
            ),
            (
                // Nothing says what an unknown key stands for, so the
                // template's own text is delivered.
                "a key nothing names",
                "{\"u\": \"<nope>\"}",
                "{\"u\": \"<nope>\"}",
            ),
        ];
        let event = event();
        for (case, template, expected) in rows {
            let actual = render(template, &paths, &event, &delivery());
            assert_eq!(
                actual.as_deref(),
                Ok(expected),
                "case {case:?}: expected {template:?} to render {expected:?}, got {actual:?}"
            );
        }
    }

    /// A rendering past [`super::MAX_RENDERED_BYTES`] is refused rather than
    /// built: a template may repeat one placeholder as often as its own
    /// length allows, so the product of the template's length and the size
    /// of what a placeholder carries is what the bound holds.
    #[test]
    fn a_rendering_past_the_bound_is_refused() {
        let large = "x".repeat(super::MAX_RENDERED_BYTES / 2 + 1);
        let event = json!({ "detail": { "large": large } });
        let paths: ::std::collections::BTreeMap<String, String> =
            [("large".to_string(), "$.detail.large".to_string())]
                .into_iter()
                .collect();
        // (case, the template, whether it renders at all)
        let rows = [
            ("one fill, inside the bound", "{\"a\": \"<large>\"}", true),
            ("two fills, past it", "{\"a\": \"<large><large>\"}", false),
        ];
        for (case, template, renders) in rows {
            let actual = render(template, &paths, &event, &delivery());
            assert_eq!(
                actual.is_ok(),
                renders,
                "case {case:?}: expected a rendering={renders} for a template filling \
                 {} bytes at a time, got {:?}",
                large.len(),
                actual.map(|rendered| rendered.len())
            );
        }
    }
}
