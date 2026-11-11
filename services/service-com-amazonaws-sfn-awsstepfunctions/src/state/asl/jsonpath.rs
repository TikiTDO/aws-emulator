// SPDX-License-Identifier: CC0-1.0

//! JSONPath, as the Amazon States Language reads it: the paths a state selects data
//! with, the payload templates it builds documents with, and the comparators one
//! `Choice` rule decides on.
//!
//! One concern, split out of [`super`] so that the language's OTHER half — what a
//! definition is and what running one does — is read apart from how one path or one
//! template is evaluated. Every item is `pub(super)` and reached exactly as it was
//! before: the module root re-exports this file's items, so the walk
//! ([`super::walk`]) and the definition's own validation ([`super::diagnose`]) name
//! them unqualified through their `use super::*`.

use super::*;

/// [`payload_template`], carrying how deeply the template itself nests so the
/// descent is bounded by [`MAX_PAYLOAD_DEPTH`] rather than by the thread's stack, and
/// what the whole template may still build.
pub(super) fn render_template(
    node: &Value,
    input: &Value,
    context: &Value,
    depth: usize,
    budget: &mut TemplateBudget,
) -> Result<Value, Failure> {
    if depth > MAX_PAYLOAD_DEPTH {
        return Err(named(
            PARAMETER_PATH_ERROR,
            format!(
                "the payload template nests more than the {MAX_PAYLOAD_DEPTH} levels this service \
                 evaluates"
            ),
        ));
    }
    match node {
        Value::Object(members) => {
            let mut rendered = Map::with_capacity(members.len());
            for (key, value) in members {
                budget.spend(key.len() + 3)?;
                match key.strip_suffix(".$") {
                    Some(stripped) => {
                        let text = value.as_str().ok_or_else(|| {
                            named(
                                PARAMETER_PATH_ERROR,
                                format!(
                                    "'{key}' must be a JSONPath or intrinsic-function string, not \
                                     {value}"
                                ),
                            )
                        })?;
                        budget.charge_path()?;
                        let built = template_expression(key, text, input, context)?;
                        budget.spend(measure_payload(&built, UNNAMED_PRODUCER)?)?;
                        rendered.insert(stripped.to_string(), built);
                    }
                    None => {
                        rendered.insert(
                            key.clone(),
                            render_template(value, input, context, depth + 1, budget)?,
                        );
                    }
                }
            }
            Ok(Value::Object(rendered))
        }
        Value::Array(items) => {
            let mut rendered = Vec::with_capacity(items.len());
            for item in items {
                budget.spend(1)?;
                rendered.push(render_template(item, input, context, depth + 1, budget)?);
            }
            Ok(Value::Array(rendered))
        }
        other => {
            budget.spend(measure_payload(other, UNNAMED_PRODUCER)?)?;
            Ok(other.clone())
        }
    }
}

/// Evaluate the value of a `"<name>.$"` member of a payload template, saying which
/// member a path that selected nothing was written for.
///
/// A template member names the field it builds, so that is what the service reports a
/// path it could not resolve against: "The JSONPath '<path>' specified for the field
/// '<field>' could not be found in the input '<document>'". The document is the one the
/// path was evaluated against — the context object for a path that begins with `$$`,
/// the template's own input for every other — and it is reported WHOLE: showing what
/// the path was looked for in is what the message exists for, so nothing of it is
/// clipped here (the cause as a whole is held to the bytes the modelled `SensitiveCause`
/// carries). The state names itself around the diagnosis ([`names_its_state`]).
pub(super) fn template_expression(
    field: &str,
    text: &str,
    input: &Value,
    context: &Value,
) -> Result<Value, Failure> {
    if text.starts_with("States.") {
        return intrinsic(text, input, context);
    }
    select_optional(text, input, context)?.ok_or_else(|| {
        let searched = if is_context_path(text) { context } else { input };
        runtime(format!(
            "{CONTEXT_PATH_OPENING}{text}' specified for the field '{field}' could not be found in \
             the input '{}'",
            reported_document(searched)
        ))
    })
}

/// Evaluate the value of a `"<name>.$"` member: a path or an intrinsic function.
pub(super) fn expression(text: &str, input: &Value, context: &Value) -> Result<Value, Failure> {
    if text.starts_with("States.") {
        return intrinsic(text, input, context);
    }
    select(text, input, context)
}

/// Evaluate a JSONPath against the state's input, or against the context object when
/// it begins with `$$`.
///
/// A path the document does not carry a value for is the state's own failure, reported
/// with [`no_results_for_path`] — which the state that evaluated it names itself
/// around ([`diagnosed`]).
pub(super) fn select(path: &str, input: &Value, context: &Value) -> Result<Value, Failure> {
    let selected = select_optional(path, input, context)?;
    selected.ok_or_else(|| runtime(no_results_for_path(path)))
}

/// [`select`], telling a value the document does not carry (`Ok(None)`) apart from a
/// path this service REFUSES (`Err`).
///
/// ASL's presence and type tests (`IsPresent`, `IsNull`, …) are the only comparators
/// defined for a path the input may not carry, so they are the only caller that reads
/// the `None`: a path the service refuses stays a failure there too, exactly as it is
/// under every other comparator.
pub(super) fn select_optional(
    path: &str,
    input: &Value,
    context: &Value,
) -> Result<Option<Value>, Failure> {
    // "paths that begin with $$ ... are evaluated against the Context Object", with
    // the first `$` naming that object and the rest being the path into it.
    if is_context_path(path) {
        return select_path(&path[1..], context);
    }
    match path.strip_prefix('$') {
        Some(_) => select_path(path, input),
        None => Err(runtime(format!(
            "'{path}' is not a JSONPath: a path begins with '$'"
        ))),
    }
}

/// Whether `path` is evaluated against the context object rather than against the
/// state's data: "paths that begin with $$ ... are evaluated against the Context
/// Object."
pub(super) fn is_context_path(path: &str) -> bool {
    path.starts_with("$$")
}

/// Evaluate `expression` against `root`, which must select exactly one value — ASL
/// paths are reference paths.
///
/// The expression is a CALLER's, so what it may ask for is bounded BEFORE the engine
/// evaluates it, because a node list the engine has already built cannot be refused
/// after the fact: it must be no longer than [`MAX_PATH_CHARS`] and it must be a
/// REFERENCE path — `$`, `.name`, `['name']`, `[index]`, at most
/// [`MAX_PATH_SEGMENTS`] of them — which is what the Amazon States Language's own
/// paths are ("a Path with syntax limited so that it can identify only a single
/// node"). A reference path identifies ONE node, so the cost of evaluating it is its
/// segment count and does NOT grow with the size of the document: a descendant
/// (`..`), a wildcard, a union or a filter would walk the whole document, or multiply
/// the node list by its own length, and is refused here.
///
/// That guard is STRUCTURAL, not textual: the engine is handed
/// [`canonical_path`]'s rendering of the segments the validator produced, never the
/// caller's own text, so no syntax the validator does not produce can reach the
/// engine however the caller's text would otherwise have parsed. The node count is
/// still checked afterwards, as defence in depth.
pub(super) fn select_path(expression: &str, root: &Value) -> Result<Option<Value>, Failure> {
    if expression.len() > MAX_PATH_CHARS {
        return Err(runtime(format!(
            "a path of {} bytes is longer than the {MAX_PATH_CHARS} this service evaluates",
            expression.len()
        )));
    }
    // Parsing the path and following its segments is work the definition asked for, and
    // a reference path identifies one node, so what it costs is the length of the path
    // itself ([`work_meter`]).
    if !spend_work(u64::try_from(expression.len()).unwrap_or(u64::MAX)) {
        return Err(out_of_work());
    }
    let segments = reference_segments(expression).map_err(|why| {
        runtime(format!(
            "'{expression}' is not a reference path this service evaluates: {why}"
        ))
    })?;
    let canonical = canonical_path(&segments);
    let parsed = JsonPath::parse(&canonical).map_err(|error| {
        runtime(format!(
            "'{expression}' is not a JSONPath this service can evaluate: {error}"
        ))
    })?;
    let nodes = parsed.query(root);
    match nodes.len() {
        1 => Ok(nodes.first().cloned()),
        // The document does not carry this path, which is an ANSWER for ASL's
        // presence and type tests and a failure for every other comparator.
        0 => Ok(None),
        many => Err(runtime(format!(
            "the path '{expression}' selected {many} values, and a reference path must select \
             exactly one"
        ))),
    }
}

/// One segment of a reference path.
pub(super) enum Segment {
    Key(String),
    Index(usize),
}

/// Split a reference path (`$`, `$.a.b`, `$['a'][0]`) into its segments.
///
/// The path text comes from a definition the model bounds only at 1 MiB, so the
/// number of segments is capped at [`MAX_PATH_SEGMENTS`]: a reference path names a
/// place in a document, so it is short, and a longer one is refused rather than
/// walked.
///
/// A segment names exactly ONE place, which is what makes a reference path's cost its
/// segment count: the wildcard `*` and a comma-separated union are therefore refused
/// here in both the `.name` and the `['name']` form, since either one would select a
/// list of nodes whose length the path text does not bound.
pub(super) fn reference_segments(path: &str) -> Result<Vec<Segment>, String> {
    let mut rest = path
        .strip_prefix('$')
        .ok_or_else(|| format!("'{path}' is not a reference path: one begins with '$'"))?;
    let mut segments = Vec::new();
    while !rest.is_empty() {
        if segments.len() >= MAX_PATH_SEGMENTS {
            return Err(format!(
                "'{path}' names more than the {MAX_PATH_SEGMENTS} reference-path segments this \
                 service follows"
            ));
        }
        if let Some(tail) = rest.strip_prefix('.') {
            let end = tail
                .find(['.', '['])
                .unwrap_or(tail.len());
            let (name, tail) = tail.split_at(end);
            if name.is_empty() {
                return Err(format!("'{path}' carries an empty member name"));
            }
            if name.contains(SELECTOR_CHARS) {
                return Err(format!(
                    "'{path}' carries the member name '{name}', and a reference path names one \
                     member rather than a wildcard or a list of them"
                ));
            }
            segments.push(Segment::Key(name.to_string()));
            rest = tail;
            continue;
        }
        if let Some(tail) = rest.strip_prefix('[') {
            let end = tail
                .find(']')
                .ok_or_else(|| format!("'{path}' has an unclosed '['"))?;
            let (inside, tail) = tail.split_at(end);
            rest = &tail[1..];
            let inside = inside.trim();
            let quoted = inside
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
                .or_else(|| {
                    inside
                        .strip_prefix('"')
                        .and_then(|value| value.strip_suffix('"'))
                });
            match quoted {
                // A subscript still carrying a quote or a comma inside the quotes it
                // was stripped of is a UNION of names, which selects as many nodes as
                // it lists; `*` is the wildcard. Neither names one place, so neither
                // is a reference path, whatever the two outer quotes suggest.
                Some(name) if name.contains(SELECTOR_CHARS) => {
                    return Err(format!(
                        "'{path}' carries the subscript '{inside}', and a reference path names one \
                         member rather than a wildcard or a list of them"
                    ));
                }
                Some(name) => segments.push(Segment::Key(name.to_string())),
                None => {
                    let index = inside
                        .parse::<usize>()
                        .map_err(|_| format!("'{path}' carries the subscript '{inside}', which is neither a quoted member name nor an array index"))?;
                    segments.push(Segment::Index(index));
                }
            }
            continue;
        }
        return Err(format!(
            "'{path}' is not a reference path: a segment is either '.name' or '[...]'"
        ));
    }
    Ok(segments)
}

/// The characters a member name of a reference path may not carry, because each one
/// turns a single-node selector into one that selects a list: the wildcard `*`, the
/// union separator `,` and the two quotes a union's members are written in.
pub(super) const SELECTOR_CHARS: &[char] = &['*', ',', '\'', '"'];

/// The canonical reference path for `segments`: `$['name'][0]`, with every member
/// name quoted and every quote and backslash inside it escaped.
///
/// This is the ONLY expression handed to the JSONPath engine. The caller's path text
/// is validated into segments and then WRITTEN OUT again from them, so the engine
/// evaluates a path this module produced rather than one a caller wrote: a text that
/// would otherwise have parsed as a union, a wildcard, a slice or a filter cannot
/// reach it, and the node list it builds is bounded by the segment count the
/// validator counted.
pub(super) fn canonical_path(segments: &[Segment]) -> String {
    let mut path = String::from("$");
    for segment in segments {
        match segment {
            Segment::Key(key) => {
                path.push_str("['");
                for character in key.chars() {
                    if character == '\\' || character == '\'' {
                        path.push('\\');
                    }
                    path.push(character);
                }
                path.push_str("']");
            }
            Segment::Index(index) => {
                path.push('[');
                path.push_str(&index.to_string());
                path.push(']');
            }
        }
    }
    path
}

/// `raw` with `result` placed at the reference path `path`.
///
/// "If the ResultPath matches an item in the state's input, only that input item is
/// overwritten", and a path the input does not carry yet is created.
///
/// The two ways this fails are different failures, and the language names each: a text
/// that is not a reference path at all was applied to nothing
/// ([`RESULT_PATH_ERROR`]), while a reference path whose segments CONFLICT with the
/// document it names a place in — a member of a value that is not an object, an index
/// of a value that is not an array — is the conflict
/// [`REFERENCE_PATH_CONFLICT_ERROR`] reports, which a definition's `Retry` and `Catch`
/// answer for by that name.
pub(super) fn set_at_reference_path(raw: &Value, path: &str, result: Value) -> Result<Value, Failure> {
    let segments = reference_segments(path).map_err(|why| named(RESULT_PATH_ERROR, why))?;
    let mut target = raw.clone();
    assign(&mut target, &segments, result)
        .map_err(|why| named(REFERENCE_PATH_CONFLICT_ERROR, why))?;
    Ok(target)
}

/// Place `result` at `segments` within `target`, creating the objects it names.
///
/// A segment that names no place in the document it is applied to is the conflict the
/// language reports ([`unable_to_apply_step`]): the step itself, and the document it
/// could not be applied to.
///
/// Walked ITERATIVELY, so the descent costs no stack: the path's segment count is a
/// caller's choice, and [`MAX_PATH_SEGMENTS`] bounds the WORK rather than having to
/// bound the recursion depth.
pub(super) fn assign(
    target: &mut Value,
    segments: &[Segment],
    result: Value,
) -> Result<(), String> {
    let mut cursor = target;
    for segment in segments {
        cursor = match segment {
            Segment::Key(key) => {
                if cursor.is_null() {
                    *cursor = Value::Object(Map::new());
                }
                if !cursor.is_object() {
                    return Err(unable_to_apply_step(key, cursor));
                }
                let Some(object) = cursor.as_object_mut() else {
                    return Err(unable_to_apply_step(key, &Value::Null));
                };
                object.entry(key.clone()).or_insert(Value::Null)
            }
            Segment::Index(index) => {
                // The conflict is worded from the IMMUTABLE reading of the document,
                // before the slot is taken mutably: an index of a value that is not an
                // array, and one the array does not carry, each name no place in it.
                let conflicted = match cursor.as_array() {
                    Some(items) if *index < items.len() => None,
                    _ => Some(unable_to_apply_step(&index.to_string(), cursor)),
                };
                if let Some(conflict) = conflicted {
                    return Err(conflict);
                }
                let Some(slot) = cursor
                    .as_array_mut()
                    .and_then(|items| items.get_mut(*index))
                else {
                    return Err(unable_to_apply_step(&index.to_string(), &Value::Null));
                };
                slot
            }
        };
    }
    *cursor = result;
    Ok(())
}

/// The diagnosis for a reference-path STEP that names no place in the document it was
/// applied to: the step as the path wrote it, and that document as this service reports
/// it ([`reported_document`]).
///
/// It is what a `ResultPath` the state's result cannot be placed through reports
/// ([`REFERENCE_PATH_CONFLICT_ERROR`]): a member of a value that is not an object, an
/// index of a value that is not an array.
fn unable_to_apply_step(step: &str, applied_to: &Value) -> String {
    format!(
        "Unable to apply step \"{step}\" to input {}",
        reported_document(applied_to)
    )
}

// ---------------------------------------------------------------------------
// Choice conditions
// ---------------------------------------------------------------------------

/// Whether one choice rule matches.
pub(super) fn condition_matches(
    rule: &Map<String, Value>,
    input: &Value,
    context: &Value,
) -> Result<bool, Failure> {
    condition_matches_at(rule, input, context, 1)
}

/// [`condition_matches`] for one rule — or one nested `And`/`Or`/`Not` condition —
/// carrying how deeply the condition tree nests, so the descent is bounded by
/// [`MAX_EXPRESSION_DEPTH`] rather than by the thread's stack: the tree is a
/// caller's, in a definition bounded only in bytes.
pub(super) fn condition_matches_at(
    rule: &Map<String, Value>,
    input: &Value,
    context: &Value,
    depth: usize,
) -> Result<bool, Failure> {
    if depth > MAX_EXPRESSION_DEPTH {
        return Err(runtime(format!(
            "a choice rule nests conditions more than the {MAX_EXPRESSION_DEPTH} levels this \
             service evaluates"
        )));
    }
    if let Some(conditions) = rule.get("And") {
        for condition in nested_conditions(conditions, "And")? {
            if !condition_matches_at(condition, input, context, depth + 1)? {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if let Some(conditions) = rule.get("Or") {
        for condition in nested_conditions(conditions, "Or")? {
            if condition_matches_at(condition, input, context, depth + 1)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if let Some(condition) = rule.get("Not") {
        let condition = condition
            .as_object()
            .ok_or_else(|| runtime("'Not' must carry a single condition object".to_string()))?;
        return Ok(!condition_matches_at(condition, input, context, depth + 1)?);
    }

    let variable = rule
        .get("Variable")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            runtime("a choice rule must carry a string 'Variable' and one comparator".to_string())
        })?;
    let (operator, operand) = rule
        .iter()
        .find(|(key, _)| !matches!(key.as_str(), "Variable" | "Next" | "Comment"))
        .ok_or_else(|| {
            runtime(format!(
                "the choice rule on '{variable}' carries no comparator"
            ))
        })?;
    compare(operator, variable, operand, input, context)
}

/// The condition objects of an `And`/`Or` member.
pub(super) fn nested_conditions<'a>(
    node: &'a Value,
    member: &str,
) -> Result<Vec<&'a Map<String, Value>>, Failure> {
    let items = node.as_array().filter(|items| !items.is_empty()).ok_or_else(|| {
        runtime(format!("'{member}' must carry a non-empty array of conditions"))
    })?;
    if items.len() > MAX_CHOICE_RULES {
        return Err(runtime(format!(
            "'{member}' carries {} conditions, more than the {MAX_CHOICE_RULES} this service \
             evaluates",
            items.len()
        )));
    }
    items
        .iter()
        .map(|item| {
            item.as_object().ok_or_else(|| {
                runtime(format!("every condition of '{member}' must be a JSON object"))
            })
        })
        .collect()
}

/// Evaluate one comparator of a choice rule.
///
/// Two kinds of problem are told apart here. A STRUCTURAL defect of the definition —
/// a comparator name the language does not define, an operand that is not a JSONPath
/// string for a `*Path` comparator, a path this service refuses — fails the execution,
/// because the rule says nothing that can be decided. A judgement about the DATA does
/// not: ASL's typed comparators are type-strict and never coerce (the service "doesn't
/// attempt to match a numeric field to a string value"), so a rule whose operands are
/// not of the type it compares simply does NOT match, and the `And`/`Or`/`Not` around
/// it and the rules after it go on being evaluated — down to the state's `Default`.
pub(super) fn compare(
    operator: &str,
    variable: &str,
    operand: &Value,
    input: &Value,
    context: &Value,
) -> Result<bool, Failure> {
    // The presence and type tests are the only comparators ASL defines for a path the
    // input may not carry at all, so they are answered before the variable is read.
    if let Some(test) = operator.strip_prefix("Is") {
        let expected = operand.as_bool().ok_or_else(|| {
            runtime(format!("'{operator}' must be compared against a boolean"))
        })?;
        // A path this service REFUSES is a failure here too; only a value the
        // document does not carry is the absence these tests are defined for.
        let value = select_optional(variable, input, context)?;
        let holds = match (test, &value) {
            ("Present", value) => value.is_some(),
            (_, None) => false,
            ("Null", Some(value)) => value.is_null(),
            ("Numeric", Some(value)) => value.is_number(),
            ("String", Some(value)) => value.is_string(),
            ("Boolean", Some(value)) => value.is_boolean(),
            ("Timestamp", Some(value)) => value
                .as_str()
                .is_some_and(|text| iso_millis(text).is_ok()),
            (other, _) => {
                return Err(runtime(format!(
                    "'Is{other}' is not a comparator this service evaluates"
                )));
            }
        };
        return Ok(holds == expected);
    }

    let value =
        select(variable, input, context).map_err(|failure| choice_path_failure(failure, variable))?;
    let (family, operand) = match operator.strip_suffix("Path") {
        Some(family) => {
            let path = operand.as_str().ok_or_else(|| {
                runtime(format!("'{operator}' must be given a JSONPath string"))
            })?;
            let operand = select(path, input, context)
                .map_err(|failure| choice_path_failure(failure, path))?;
            (family, operand)
        }
        None => (operator, operand.clone()),
    };

    let undefined = || {
        runtime(format!(
            "'{operator}' is not a comparator this service evaluates"
        ))
    };
    if let Some(comparison) = family.strip_prefix("String") {
        // "StringMatches […] a string that may contain one or more wildcards": the
        // only comparator of this family that is not one of the four orderings.
        if comparison == "Matches" {
            // Type-strict: only a string is a candidate for a string comparator, and a
            // value of any other type is simply not matched.
            let (Some(text), Some(pattern)) = (value.as_str(), operand.as_str()) else {
                return Ok(false);
            };
            return glob_matches(pattern, text);
        }
        let comparison = Comparison::named(comparison).ok_or_else(undefined)?;
        let (Some(left), Some(right)) = (value.as_str(), operand.as_str()) else {
            return Ok(false);
        };
        return Ok(comparison.holds_for(left.cmp(right)));
    }
    if let Some(comparison) = family.strip_prefix("Numeric") {
        let comparison = Comparison::named(comparison).ok_or_else(undefined)?;
        let (Some(left), Some(right)) = (value.as_f64(), operand.as_f64()) else {
            return Ok(false);
        };
        let Some(order) = left.partial_cmp(&right) else {
            return Ok(false);
        };
        return Ok(comparison.holds_for(order));
    }
    if let Some(comparison) = family.strip_prefix("Boolean") {
        // `BooleanEquals` is the whole family: the language defines no ordering over
        // booleans.
        if comparison != "Equals" {
            return Err(undefined());
        }
        let (Some(left), Some(right)) = (value.as_bool(), operand.as_bool()) else {
            return Ok(false);
        };
        return Ok(left == right);
    }
    if let Some(comparison) = family.strip_prefix("Timestamp") {
        let comparison = Comparison::named(comparison).ok_or_else(undefined)?;
        let (Some(left), Some(right)) = (value.as_str(), operand.as_str()) else {
            return Ok(false);
        };
        // A string that is no ISO 8601 timestamp is not a value of the type this
        // family compares either, so it does not match rather than failing the run.
        let (Ok(left), Ok(right)) = (iso_millis(left), iso_millis(right)) else {
            return Ok(false);
        };
        return Ok(comparison.holds_for(left.cmp(&right)));
    }
    Err(undefined())
}

/// One of the five comparisons ASL's `String*`, `Numeric*` and `Timestamp*` comparator
/// families each define, read from the comparator's own suffix.
#[derive(Clone, Copy, Debug)]
pub(super) enum Comparison {
    Equals,
    LessThan,
    LessThanEquals,
    GreaterThan,
    GreaterThanEquals,
}

impl Comparison {
    /// The comparison `suffix` names, or `None` when the language defines none by that
    /// name — which is a comparator this service cannot evaluate rather than a
    /// comparison that holds for nothing.
    pub(super) fn named(suffix: &str) -> Option<Self> {
        match suffix {
            "Equals" => Some(Self::Equals),
            "LessThan" => Some(Self::LessThan),
            "LessThanEquals" => Some(Self::LessThanEquals),
            "GreaterThan" => Some(Self::GreaterThan),
            "GreaterThanEquals" => Some(Self::GreaterThanEquals),
            _ => None,
        }
    }

    /// Whether the candidate ordering `order` satisfies this comparison.
    pub(super) fn holds_for(self, order: std::cmp::Ordering) -> bool {
        use std::cmp::Ordering::{Equal, Greater, Less};
        match self {
            Self::Equals => order == Equal,
            Self::LessThan => order == Less,
            Self::LessThanEquals => order != Greater,
            Self::GreaterThan => order == Greater,
            Self::GreaterThanEquals => order != Less,
        }
    }
}

/// One token of a `StringMatches` glob.
pub(super) enum Glob {
    /// A literal character, which an escape (`\\*`) also produces.
    Char(char),
    /// `*`, which "matches zero or more characters".
    Star,
}

/// `StringMatches`' glob: "The only character that has any special meaning is '*',
/// which matches zero or more characters", escaped as `\\*`.
///
/// Matched with explicit BACKTRACKING over the last wildcard, so a pattern whose
/// literal also occurs earlier in the text still matches: a leftmost-first scan that
/// never reconsidered would answer `false` for `a*bc` against `abcbc`, which the glob
/// does match, and a `Choice` would then take a branch the definition does not
/// dictate. A pattern longer than [`MAX_GLOB_CHARS`] is REFUSED rather than reported
/// as no match, for the same reason: a ceiling this emulator sets is a failure, never
/// an answer about the data.
pub(super) fn glob_matches(pattern: &str, text: &str) -> Result<bool, Failure> {
    if pattern.chars().count() > MAX_GLOB_CHARS {
        return Err(runtime(format!(
            "a 'StringMatches' pattern of {} characters is longer than the {MAX_GLOB_CHARS} this \
             service matches",
            pattern.chars().count()
        )));
    }
    // Matching backtracks over the last wildcard, so the match costs at most the
    // pattern's length times the text's: the PRODUCT of a pattern a definition wrote
    // and a value its data carries, which is what this walk is charged for
    // ([`work_meter`]) before the match is made.
    let cost = u64::try_from(pattern.chars().count())
        .unwrap_or(u64::MAX)
        .saturating_mul(u64::try_from(text.chars().count()).unwrap_or(u64::MAX));
    if !spend_work(cost) {
        return Err(out_of_work());
    }
    let mut tokens: Vec<Glob> = Vec::new();
    let mut characters = pattern.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                if let Some(escaped) = characters.next() {
                    tokens.push(Glob::Char(escaped));
                }
            }
            '*' => tokens.push(Glob::Star),
            other => tokens.push(Glob::Char(other)),
        }
    }

    let text: Vec<char> = text.chars().collect();
    let mut token = 0usize;
    let mut at = 0usize;
    // Where to resume if the current attempt fails: the wildcard last passed, and
    // the position it had consumed up to.
    let mut star: Option<usize> = None;
    let mut after_star = 0usize;
    while at < text.len() {
        match tokens.get(token) {
            Some(Glob::Char(expected)) if *expected == text[at] => {
                token += 1;
                at += 1;
            }
            Some(Glob::Star) => {
                star = Some(token);
                after_star = at;
                token += 1;
            }
            _ => match star {
                // The wildcard consumes one more character and the attempt is made
                // again from there.
                Some(resume) => {
                    token = resume + 1;
                    after_star += 1;
                    at = after_star;
                }
                None => return Ok(false),
            },
        }
    }
    // Trailing wildcards may match nothing at all.
    while matches!(tokens.get(token), Some(Glob::Star)) {
        token += 1;
    }
    Ok(token == tokens.len())
}
