// SPDX-License-Identifier: CC0-1.0

//! EventBridge's event-pattern language: validate a pattern, and decide whether
//! an event envelope matches one.
//!
//! Moved out of `crate::state` unchanged, so one file holds one concern; the
//! language and the operators it admits are stated where the module is declared
//! ([`crate::state`]).

use std::collections::BTreeMap;
use std::net::IpAddr;

use ::serde_json::value::RawValue;
use ::serde_json::{Map, Value};

/// The comparison operators `numeric` admits.
const NUMERIC_OPERATORS: [&str; 5] = ["=", "<", "<=", ">", ">="];

/// The operators that take a single string and may also appear nested
/// inside `anything-but`.
const STRING_OPERATORS: [&str; 4] = ["prefix", "suffix", "equals-ignore-case", "wildcard"];

/// How deeply a pattern may nest objects.
///
/// This module's OWN bound on recursion, not a claim about any limit AWS
/// publishes: `EventPattern` is modelled `length 0..4096`, which bounds a
/// pattern's text but not its depth, so the guard is stated here instead of
/// relying on `serde_json`'s parse-depth default. It is far above any real
/// pattern — the deepest field path an event envelope offers is a few
/// levels inside `detail`.
const MAX_PATTERN_DEPTH: usize = 32;

/// What every refusal of a pattern opens with.
///
/// EventBridge reports an unacceptable pattern as "Event pattern is not
/// valid. Reason: <what is wrong>", so the reason this module computes is
/// stated after it and the whole message is the one
/// `InvalidEventPatternException` carries, whichever operation was given the
/// pattern. A caller therefore never restates the opening itself — the
/// message this module answers with is already the whole one.
pub(super) const INVALID_PATTERN: &str = "Event pattern is not valid. Reason:";

/// A term object names ONE match type, so one carrying several names no
/// single one; this is the reason EventBridge gives for it.
const ONE_KEY_ONLY: &str = "Only one key allowed in match expression";

/// A list with no member lists no value to match, and an object with no
/// member names no match type: neither is a term EventBridge admits.
const EMPTY_ARRAY: &str = "Empty arrays are not allowed";
const EMPTY_OBJECT: &str = "Empty objects are not allowed";

/// `exists` compares against the PRESENCE of a leaf, so its argument is a
/// boolean and nothing else; this is the reason EventBridge gives for any
/// other argument.
const EXISTS_NOT_BOOLEAN: &str = "exists match pattern must be either true or false.";

/// A `numeric` range whose bounds admit no number at all: the reason
/// EventBridge gives for it, in the service's own wording.
const BOTTOM_BELOW_TOP: &str = "Bottom must be less than top";

/// A pattern is a FILTER over an event's fields, so the pattern itself is an
/// object; this is the reason EventBridge gives for text that parses as
/// anything else.
const FILTER_NOT_AN_OBJECT: &str = "Filter is not an object";

/// How many pattern objects an `$or` relationship relates.
///
/// `$or` is a RELATIONSHIP between pattern objects, and a relationship needs
/// two sides: the service states the bound itself in the refusal the checker
/// gives for a shorter list ([`OR_NEEDS_TWO_OBJECTS`]).
const OR_RELATED_OBJECTS: usize = 2;

/// The reason the operation that only CHECKS a pattern gives for a `$or`
/// relating fewer than [`OR_RELATED_OBJECTS`] pattern objects.
///
/// The two validators this service runs a pattern through agree on every
/// refusal but this one: the checker words it for the relationship, while the
/// compiler reads a list too short to relate anything as the ordinary match
/// term list it is (see [`validate_object`]).
const OR_NEEDS_TWO_OBJECTS: &str = "There must have at least 2 Objects in $or relationship.";

/// The character `wildcard` matches any run of characters with.
const WILDCARD: char = '*';

/// Which of the two validators this service runs a pattern through is reading
/// it.
///
/// They refuse the same patterns for the same reasons everywhere but one
/// construct ([`OR_NEEDS_TWO_OBJECTS`]), so the distinction travels as a
/// parameter of the one parse rather than as a second copy of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validator {
    /// The compiler a pattern that is about to be STORED goes through:
    /// `PutRule`, `CreateArchive`, `UpdateArchive`, and the parse routing
    /// takes of a stored pattern.
    Compiler,
    /// The checker `TestEventPattern` answers from, which stores nothing.
    Checker,
}

/// The most characters the event pattern of a RULE may carry.
///
/// A rule's event pattern is bounded by the service's pattern quota, which counts
/// the pattern's characters AS SUBMITTED — the whitespace between its tokens
/// included, since the bound is on the text and not on a normalisation of it. The
/// modelled `@length` on `EventPattern` is looser (0..4096) and is what the
/// boundary validator enforces, so the quota itself is charged by
/// [`length_violation`], where a rule is written.
pub const MAX_RULE_PATTERN_CHARS: usize = 2048;

/// Why `pattern` is too long for a rule to carry, or `None` when it is within
/// the quota.
///
/// What comes back is the WHOLE message of the `InvalidEventPatternException` a
/// caller answers with, like every other refusal of this module ([`parse`]): the
/// opening ([`INVALID_PATTERN`]) and then the reason. The reason states the
/// CONDITION and nothing of the submitted text — a pattern past the bound is as
/// long as the request, and there is no position in it to report the way a
/// reader's refusal has one ([`Refusal::message`]).
pub fn length_violation(pattern: &str) -> Option<String> {
    (pattern.chars().count() > MAX_RULE_PATTERN_CHARS)
        .then(|| format!("{INVALID_PATTERN} {PATTERN_TOO_LONG}"))
}

/// The reason a pattern past [`MAX_RULE_PATTERN_CHARS`] is refused for.
const PATTERN_TOO_LONG: &str = "Event pattern is too long.";

/// Parse `pattern` as the pattern COMPILER reads it — the validator every
/// operation that stores a pattern, and routing itself, goes through.
///
/// What comes back is the WHOLE message of the `InvalidEventPatternException`
/// a caller answers with: the opening ([`INVALID_PATTERN`]), the reason, and
/// the source location the reader had reached in the submitted text
/// ([`Refusal::message`]). A reason the service words for the offending
/// CONSTRUCT carries that wording and no field path; one this module words
/// for itself names the field path, so a caller can find what to change.
pub fn parse(pattern: &str) -> Result<Map<String, Value>, String> {
    parse_as(pattern, Validator::Compiler)
}

/// [`parse`], read by `validator`: the same parse, worded as that validator
/// words it.
pub fn parse_as(pattern: &str, validator: Validator) -> Result<Map<String, Value>, String> {
    parse_reason(pattern, validator).map_err(|refusal| refusal.message(pattern))
}

/// The parse itself, reporting the reason the refusal states and where in
/// the submitted text the reader stood when it stopped.
fn parse_reason(pattern: &str, validator: Validator) -> Result<Map<String, Value>, Refusal> {
    let value: Value =
        ::serde_json::from_str(pattern).map_err(|error| unreadable_json(pattern, &error))?;
    let Value::Object(map) = value else {
        return Err(At::root().refuse(Read::Opening, FILTER_NOT_AN_OBJECT));
    };
    if map.is_empty() {
        return Err(At::root().refuse(
            Read::Whole,
            "Event pattern must name at least one field",
        ));
    }
    validate_object(&map, &At::root(), 1, validator)?;
    Ok(map)
}

/// The refusal of text that is not JSON at all: WHERE the parse that ran
/// stopped, and what it was expecting there.
///
/// EventBridge refuses a pattern it cannot read with its JSON parser's own
/// diagnostic rather than with a restatement of the member's type, and the
/// two readings that diagnostic is built from — the unquoted word a parse
/// stopped on where a value belongs, and the character or end-of-input it
/// stopped on otherwise, with what was expected there — are the tree's
/// shared ones ([`::service_common::json_parse_fault`]), taken from the parse
/// that actually ran, so this service and every other one handed the same
/// text cannot disagree about where it went wrong. A fault class worded
/// wrongly or missing is corrected THERE, for every service that quotes it.
///
/// What is this service's own is the framing: the opening
/// ([`INVALID_PATTERN`]) and the location clause [`Refusal::message`] builds,
/// which echoes the pattern as submitted (`(String)"<pattern>"`) rather than
/// redacting it, because that is what this service answers.
fn unreadable_json(pattern: &str, error: &::serde_json::Error) -> Refusal {
    use ::service_common::json_parse_fault as fault;

    // A word where a value belongs is named WHOLE, at the position it
    // STARTS; every other fault names the character, or the end of the
    // input, the parse stopped on, and the position is the parse's own.
    match fault::unrecognized_token(pattern, error) {
        Some((word, line, column)) => Refusal {
            reason: format!(
                "Unrecognized token '{word}': was expecting ({})",
                fault::VALUE_CLASSES
            ),
            spot: Some(Spot::Position { line, column }),
        },
        None => Refusal {
            reason: fault::unexpected_input(pattern, error),
            spot: Some(Spot::Position {
                line: error.line(),
                column: error.column(),
            }),
        },
    }
}

/// A pattern the language does not admit: the reason, and where in the
/// submitted text the reader stood when it refused.
#[derive(Debug)]
struct Refusal {
    /// What is wrong, in the wording the refusal states after
    /// [`INVALID_PATTERN`].
    reason: String,
    /// Where the reader was when it found that out, and `None` for a reason
    /// the reader did not itself produce ([`Refusal::unplaced`]).
    spot: Option<Spot>,
}

impl Refusal {
    /// A refusal the pattern READER did not state: the operand's own parser
    /// did, so there is no reading position to report and the message is the
    /// reason alone.
    fn unplaced(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            spot: None,
        }
    }

    /// The whole message the refusal carries: the opening, the reason, and —
    /// for a reason the reader stated — the source location, which is the
    /// pattern exactly as it was submitted and the line and column of the
    /// position the reader had reached in it.
    ///
    /// Every part of the location is derived from the request, so the
    /// caller's own text is what it points into: a tool that reads the
    /// location can show the offending character of the pattern its user
    /// wrote. The pattern travels as part of a message VALUE, which the
    /// protocol's own serializer encodes, so text carrying quotes or
    /// newlines cannot break the error envelope it is rendered into.
    fn message(&self, pattern: &str) -> String {
        let opening = format!("{INVALID_PATTERN} {}", self.reason);
        match &self.spot {
            None => opening,
            Some(spot) => {
                let (line, column) = spot.line_and_column(pattern);
                format!(
                    "{opening}\n at [Source: (String)\"{pattern}\"; line: {line}, \
                     column: {column}]"
                )
            }
        }
    }
}

/// Where in the submitted pattern text a refusal happened.
#[derive(Debug)]
enum Spot {
    /// At the node `steps` names, with `read` of that node consumed: the
    /// position is found by walking the submitted TEXT to it, so it is the
    /// caller's own spelling — whitespace, key order and all — that decides
    /// the line and column.
    Node { steps: Vec<Step>, read: Read },
    /// At a line and column already known, which is all there is to know
    /// about text that does not parse as JSON at all.
    Position { line: usize, column: usize },
}

impl Spot {
    /// The line and column, both counted from 1, this spot is at in
    /// `pattern`.
    ///
    /// A node the walk cannot follow leaves the position at the end of the
    /// text: every node a refusal names comes from the parse of this very
    /// text, so there is no such node — reporting the end of what was read
    /// is the honest answer rather than an invented position.
    fn line_and_column(&self, pattern: &str) -> (usize, usize) {
        match self {
            Self::Position { line, column } => (*line, *column),
            Self::Node { steps, read } => {
                let offset = offset_of(pattern, steps, *read).unwrap_or(pattern.len());
                line_and_column(pattern, offset)
            }
        }
    }
}

/// One step from the pattern's root to one of its nodes, as the walk over
/// the submitted text follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// The value of the member with this name.
    Key(String),
    /// The value of the member at this position, counted in the order the
    /// TEXT writes the members: how a refusal about a SECOND member of an
    /// object finds it.
    KeyAt(usize),
    /// The member of a list at this position.
    Index(usize),
}

/// How much of the node a refusal names the reader had consumed when it
/// refused — which is what decides the position it reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    /// Its first character, which is where reading the member NAME that
    /// introduces it leaves the reader: a reader that has just read a
    /// member name has consumed the colon after it and the first character
    /// of the value as well. What a refusal about a member name itself had
    /// read.
    Entered,
    /// Its opening token only — a string's opening quote, an object's or a
    /// list's bracket, or the whole of any other scalar, which is one token.
    /// What a refusal decided from the node's TYPE had read.
    Opening,
    /// All of it, up to and past its closing token: what a refusal that had
    /// to read the node's CONTENT had read.
    Whole,
}

/// Where one step of the validation is: the dotted field path the reasons
/// this module words itself name, and the steps that find the same node in
/// the submitted text.
#[derive(Debug, Clone)]
struct At {
    /// The dotted path of event fields this node sits under.
    field: String,
    /// The steps from the pattern's root to this node.
    steps: Vec<Step>,
}

impl At {
    /// The pattern as a whole.
    fn root() -> Self {
        Self {
            field: String::new(),
            steps: Vec::new(),
        }
    }

    /// The value of `key`, which names a FIELD of the event: the dotted
    /// field path grows with it.
    fn field(&self, key: &str) -> Self {
        let field = if self.field.is_empty() {
            key.to_string()
        } else {
            format!("{}.{key}", self.field)
        };
        Self {
            field,
            steps: self.then(Step::Key(key.to_string())),
        }
    }

    /// The value of `key` of this node where `key` is NOT a field of the
    /// event — an operator and its argument — so the field path stays the
    /// one the operator was found under.
    fn key(&self, key: &str) -> Self {
        Self {
            field: self.field.clone(),
            steps: self.then(Step::Key(key.to_string())),
        }
    }

    /// The value of the member this node writes at `nth`.
    fn nth_key(&self, nth: usize) -> Self {
        Self {
            field: self.field.clone(),
            steps: self.then(Step::KeyAt(nth)),
        }
    }

    /// The member of this list at `index`.
    fn index(&self, index: usize) -> Self {
        Self {
            field: self.field.clone(),
            steps: self.then(Step::Index(index)),
        }
    }

    /// This node's steps, with `step` appended.
    fn then(&self, step: Step) -> Vec<Step> {
        let mut steps = self.steps.clone();
        steps.push(step);
        steps
    }

    /// Refuse the pattern here, the reader having consumed `read` of this
    /// node.
    fn refuse(&self, read: Read, reason: impl Into<String>) -> Refusal {
        Refusal {
            reason: reason.into(),
            spot: Some(Spot::Node {
                steps: self.steps.clone(),
                read,
            }),
        }
    }
}

/// The offset in `pattern` the reader stands at, having walked to the node
/// `steps` names and consumed `read` of it.
///
/// The position a refusal reports is a fact about the submitted TEXT rather
/// than about the parse tree — two patterns that parse to the same tree point
/// at different characters — so the walk needs each node's own characters,
/// which is exactly what `serde_json`'s `RawValue` is: a node read as a
/// WINDOW onto the text it came from, borrowed from it and never
/// re-interpreted, so where that window begins is where the node begins
/// ([`offset_in`]). The reading is `serde_json`'s throughout: nothing here
/// re-scans the document.
fn offset_of(pattern: &str, steps: &[Step], read: Read) -> Option<usize> {
    let mut node: &RawValue = ::serde_json::from_str(pattern).ok()?;
    for step in steps {
        node = match step {
            Step::Index(index) => ::serde_json::from_str::<Vec<&RawValue>>(node.get())
                .ok()?
                .get(*index)
                .copied()?,
            Step::Key(key) => ::serde_json::from_str::<BTreeMap<String, &RawValue>>(node.get())
                .ok()?
                .remove(key.as_str())?,
            // The nth member the TEXT writes, which is the nth by where it
            // begins: a parsed object does not keep the order the caller
            // wrote its members in.
            Step::KeyAt(nth) => {
                let members =
                    ::serde_json::from_str::<BTreeMap<String, &RawValue>>(node.get()).ok()?;
                let mut written: Vec<&RawValue> = members.into_values().collect();
                written.sort_by_key(|member| {
                    offset_in(pattern, member.get()).unwrap_or(usize::MAX)
                });
                *written.get(*nth)?
            }
        };
    }
    let text = node.get();
    let start = offset_in(pattern, text)?;
    Some(match read {
        // Reading the member name that introduces a value consumes the
        // colon after the name and the value's first character too, so a
        // refusal about the NAME stands one character into the value.
        Read::Entered => start + text.chars().next().map_or(0, char::len_utf8),
        // A string is read as far as its opening quote and an object or a
        // list as far as its bracket; every other value is a single token,
        // so reading it at all reads the whole of it.
        Read::Opening => match text.as_bytes().first() {
            Some(b'"' | b'{' | b'[') => start + 1,
            _ => start + text.len(),
        },
        Read::Whole => start + text.len(),
    })
}

/// The byte offset at which `window` sits inside `text`, or `None` when it is
/// not a slice of it.
///
/// Both are slices of the one submitted string, so the offset is the distance
/// between where they start — read from the two slices' addresses, never
/// dereferenced — and the bound check is what keeps that an answer about this
/// text rather than about two unrelated strings.
fn offset_in(text: &str, window: &str) -> Option<usize> {
    let offset = (window.as_ptr() as usize).checked_sub(text.as_ptr() as usize)?;
    offset
        .checked_add(window.len())
        .filter(|end| *end <= text.len())
        .map(|_| offset)
}

/// The line and column, both counted from 1, of `offset` in `text`.
///
/// The column counts CHARACTERS from the start of the line, so a multi-byte
/// character before the position counts once.
fn line_and_column(text: &str, offset: usize) -> (usize, usize) {
    let read = text.get(..offset).unwrap_or(text);
    let line = 1 + read.matches('\n').count();
    let last = read.rsplit_once('\n').map_or(read, |(_, after)| after);
    (line, 1 + last.chars().count())
}

/// Whether `event` matches an ALREADY-PARSED pattern, spending `budget`.
///
/// Routing parses each rule's pattern once per request and evaluates it
/// against every entry, so the parse is not repeated per (entry, rule)
/// pair; `budget` is what bounds the total comparison work those pairs can
/// demand. A match that runs out of budget answers "no match", and the
/// caller reports the exhaustion (see [`Budget::spent`]) rather than
/// passing a half-evaluated verdict off as a real one.
pub fn matches_parsed(
    pattern: &Map<String, Value>,
    event: &Value,
    budget: &mut Budget,
) -> bool {
    match_object(pattern, event, budget)
}

/// A work allowance for pattern matching, counted in bytes of text the
/// matcher is allowed to walk.
///
/// Every factor of the routing work is individually bounded — entries per
/// request by the modelled `Entries` maximum, rules per bus by
/// [`super::MAX_RULES_PER_BUS`], an event by
/// [`super::MAX_ENTRY_SIZE_BYTES`], a pattern by `EventPattern`'s modelled
/// 4096 characters — but their PRODUCT is not, so a single request could
/// otherwise demand far more comparison work than any real one does. One
/// budget is created per entry and spent across every rule that entry is
/// matched against, which caps the whole request at the budget times the
/// modelled maximum number of entries.
#[derive(Debug)]
pub struct Budget {
    remaining: u64,
}

impl Budget {
    /// A budget of `units`, each unit one byte of text the matcher may walk.
    pub fn new(units: u64) -> Self {
        Self { remaining: units }
    }

    /// Whether this budget ran out, which means the verdict it produced is
    /// not a real one and the caller must say so.
    pub fn spent(&self) -> bool {
        self.remaining == 0
    }

    /// Charge `units` against the allowance, answering `false` once it is
    /// gone. Charged BEFORE the work it pays for, so no comparison runs
    /// outside the bound.
    fn charge(&mut self, units: usize) -> bool {
        let units = u64::try_from(units).unwrap_or(u64::MAX);
        if self.remaining <= units {
            self.remaining = 0;
            return false;
        }
        self.remaining -= units;
        true
    }
}

/// How many bytes comparing against this leaf may walk: a string's own
/// length, a small constant otherwise. Computed in constant time, so
/// charging is never itself the expensive step.
fn leaf_cost(leaf: &Value) -> usize {
    match leaf {
        Value::String(text) => text.len().max(1),
        Value::Array(members) => members.len().max(1),
        Value::Object(map) => map.len().max(1),
        _ => 1,
    }
}

/// How many bytes of PATTERN a term's argument may make the matcher walk:
/// every string it carries and every member it holds, AT EVERY LEVEL.
///
/// Every level, because the walks this pays for descend: a nested
/// `anything-but` argument (`{"anything-but": {"wildcard": ["*a*", "*b*"]}}`)
/// compares the leaf against every member of its inner list, so a charge that
/// stopped at the outer object would pay one unit for all of it. A pattern is
/// bounded by `EventPattern`'s modelled 4096 characters, so this walk is
/// itself cheap — but the same reading is asked of an EVENT's value (see
/// [`match_term`]'s `exists` arm), which nests as deep as
/// [`super::MAX_DETAIL_DEPTH`] admits. It therefore descends on an EXPLICIT
/// stack rather than on the thread's: how deeply a document nests decides how
/// much heap this walk takes and never whether the process survives it, which
/// is the same guarantee the parse of that document is given
/// ([`super::parse_json_document`]).
fn argument_cost(argument: &Value) -> usize {
    let mut total = 0usize;
    let mut pending: Vec<&Value> = ::std::vec![argument];
    while let Some(value) = pending.pop() {
        let charge = match value {
            Value::String(text) => text.len().max(1),
            Value::Array(members) => {
                pending.extend(members.iter());
                members.len().max(1)
            }
            Value::Object(map) => {
                pending.extend(map.values());
                map.len().max(1)
            }
            _ => 1,
        };
        total = total.saturating_add(charge);
    }
    total
}

/// What comparing ONE leaf against `argument` may walk: the argument's own
/// text at every level, PLUS the leaf once for EVERY argument the comparison
/// can try it against.
///
/// The leaf is charged per argument because each try walks it again — a
/// `wildcard` or `prefix` comparison scans the whole leaf — so a list of
/// arguments costs the leaf's length times its length, which is the work
/// [`match_operator`] can actually perform.
fn comparison_cost(argument: &Value, leaf: &Value) -> usize {
    argument_cost(argument)
        .saturating_add(leaf_cost(leaf).saturating_mul(argument_terms(argument)))
}

/// How many separate arguments one comparison can try against the leaf: the
/// members of a list, the members of a nested operator's list, and one for a
/// single argument.
fn argument_terms(argument: &Value) -> usize {
    match argument {
        Value::Array(members) => members.len().max(1),
        Value::Object(map) => map
            .values()
            .map(|nested| match nested {
                Value::Array(members) => members.len().max(1),
                _ => 1,
            })
            .sum::<usize>()
            .max(1),
        _ => 1,
    }
}

/// Every field of a pattern object: an array of match terms, a nested
/// object, or the `$or` branch list. `depth` counts the objects entered so
/// far, so recursion is bounded by [`MAX_PATTERN_DEPTH`] whatever a caller
/// sends.
fn validate_object(
    pattern: &Map<String, Value>,
    at: &At,
    depth: usize,
    validator: Validator,
) -> Result<(), Refusal> {
    if depth > MAX_PATTERN_DEPTH {
        return Err(at.refuse(
            Read::Opening,
            format!("Event pattern nests objects more than {MAX_PATTERN_DEPTH} levels deep"),
        ));
    }
    for (key, spec) in pattern {
        let at = at.field(key);
        let field = at.field.clone();
        if key == "$or" {
            let Some(branches) = spec.as_array() else {
                return Err(at.refuse(
                    Read::Opening,
                    format!("\"{field}\" must be an array of patterns"),
                ));
            };
            if branches.is_empty() {
                return Err(at.refuse(Read::Whole, EMPTY_ARRAY));
            }
            // `$or` relates pattern objects to one another, and a
            // relationship needs [`OR_RELATED_OBJECTS`] sides: a list too
            // short to relate anything is no relationship, so the checker
            // says so ([`OR_NEEDS_TWO_OBJECTS`]) and the compiler reads the
            // list as the ordinary match TERM list a field's value is —
            // which is what makes a lone object naming no match type an
            // unrecognized match type rather than a branch of an `$or`.
            if branches.len() < OR_RELATED_OBJECTS {
                match validator {
                    Validator::Checker => {
                        return Err(at.refuse(Read::Whole, OR_NEEDS_TWO_OBJECTS));
                    }
                    Validator::Compiler => branches
                        .iter()
                        .enumerate()
                        .try_for_each(|(index, term)| validate_term(term, &at.index(index)))?,
                }
                continue;
            }
            for (index, branch) in branches.iter().enumerate() {
                let at = at.index(index);
                let Some(object) = branch.as_object() else {
                    return Err(at.refuse(
                        Read::Opening,
                        format!("every branch of {field} must be a pattern object"),
                    ));
                };
                if object.is_empty() {
                    return Err(at.refuse(Read::Whole, EMPTY_OBJECT));
                }
                validate_object(object, &at, depth + 1, validator)?;
            }
            continue;
        }
        match spec {
            Value::Array(terms) => {
                if terms.is_empty() {
                    return Err(at.refuse(Read::Whole, EMPTY_ARRAY));
                }
                for (index, term) in terms.iter().enumerate() {
                    validate_term(term, &at.index(index))?;
                }
            }
            Value::Object(child) => {
                if child.is_empty() {
                    return Err(at.refuse(Read::Whole, EMPTY_OBJECT));
                }
                validate_object(child, &at, depth + 1, validator)?;
            }
            _ => {
                // The reader names the MEMBER whose value is neither, as it
                // is spelled in the pattern — not the dotted field path it
                // sits under, which is this module's own way of naming a
                // field and belongs only to the reasons it words itself.
                return Err(at.refuse(
                    Read::Opening,
                    format!("\"{key}\" must be an object or an array"),
                ));
            }
        }
    }
    Ok(())
}

/// One match term: a literal value, or a single-key operator object. `at`
/// locates the term itself in the submitted text.
///
/// A term object is a MATCH EXPRESSION, which names one match type: an
/// object carrying several keys names several ([`ONE_KEY_ONLY`]) and one
/// carrying none names no match type at all ([`EMPTY_OBJECT`]).
fn validate_term(term: &Value, at: &At) -> Result<(), Refusal> {
    match term {
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => Ok(()),
        Value::Object(map) if map.len() == 1 => {
            let (operator, argument) = map
                .iter()
                .next()
                .expect("a single-entry map has a first entry");
            validate_operator(operator, argument, &at.key(operator))
        }
        Value::Object(map) if map.is_empty() => Err(at.refuse(Read::Whole, EMPTY_OBJECT)),
        // The reader finds this out on the SECOND match type the object
        // names, having read that name and entered its argument, which is
        // as far as it had got.
        Value::Object(_) => Err(at.nth_key(1).refuse(Read::Entered, ONE_KEY_ONLY)),
        _ => Err(at.refuse(
            Read::Opening,
            format!(
                "\"{}\" admits literal values and single-operator objects only",
                at.field
            ),
        )),
    }
}

/// The argument an operator takes. `at` locates that ARGUMENT in the
/// submitted text and carries the field path the reasons this module words
/// itself name.
fn validate_operator(operator: &str, argument: &Value, at: &At) -> Result<(), Refusal> {
    let field = at.field.clone();
    // A type the operator does not take is decided from the argument's
    // opening token alone, which is as much of it as the reader had read.
    let bad = |what: &str| {
        Err(at.refuse(
            Read::Opening,
            format!("{field}: {operator} takes {what}"),
        ))
    };
    match operator {
        "exists" => {
            if argument.is_boolean() {
                Ok(())
            } else {
                Err(at.refuse(Read::Opening, EXISTS_NOT_BOOLEAN))
            }
        }
        "prefix" | "suffix" => match argument {
            Value::String(_) => Ok(()),
            Value::Object(map) if map.len() == 1 => {
                let (nested, nested_argument) = map
                    .iter()
                    .next()
                    .expect("a single-entry map has a first entry");
                if nested == "equals-ignore-case" && nested_argument.is_string() {
                    Ok(())
                } else {
                    bad("a string, or {\"equals-ignore-case\": \"<string>\"}")
                }
            }
            _ => bad("a string, or {\"equals-ignore-case\": \"<string>\"}"),
        },
        "equals-ignore-case" => {
            if argument.is_string() {
                Ok(())
            } else {
                bad("a string")
            }
        }
        // A wildcard value is a string whose `*` stands for any run of
        // characters, and the language admits no two of them in a row.
        "wildcard" => match argument.as_str() {
            // Read from the value's own text, so the whole of it had been
            // read before the refusal.
            Some(text) => wildcard_violation(text)
                .map_or(Ok(()), |reason| Err(at.refuse(Read::Whole, reason))),
            None => bad("a string"),
        },
        "cidr" => match argument.as_str().map(parse_cidr) {
            Some(Ok(_)) => Ok(()),
            // The mask width is a rule of the CIDR NOTATION, which the
            // address parser reads out of the block's own text and the
            // service words for the notation, so what that parser says is
            // the whole reason and there is no reading position to state.
            Some(Err(CidrRefusal::MaskNotNarrowerThanTheFamily(reason))) => {
                Err(Refusal::unplaced(reason))
            }
            // Text that is no block at all is this operator's own argument
            // check, worded and placed as every other one is — whether what
            // was given is a string that names no block or no string at all.
            Some(Err(CidrRefusal::NotABlock)) | None => bad("an IPv4 or IPv6 CIDR block"),
        },
        "anything-but" => match argument {
            Value::String(_) | Value::Number(_) => Ok(()),
            Value::Array(values) if !values.is_empty() => {
                if values
                    .iter()
                    .all(|value| value.is_string() || value.is_number())
                {
                    Ok(())
                } else {
                    bad("a list of strings or a list of numbers")
                }
            }
            Value::Object(map) if map.len() == 1 => {
                let (nested, nested_argument) = map
                    .iter()
                    .next()
                    .expect("a single-entry map has a first entry");
                // A nested string operator may take a LIST of arguments,
                // any one of which excludes the value: `anything-but` is the
                // one place the pattern language admits that form. Any other
                // match type nested under it is one it does not negate, and
                // the reader has entered that operator's argument by the
                // time it can say so.
                let nested_at = at.key(nested);
                if !STRING_OPERATORS.contains(&nested.as_str()) {
                    return Err(nested_at.refuse(
                        Read::Opening,
                        format!("Unsupported {operator} pattern: {nested}"),
                    ));
                }
                match nested_argument {
                    Value::Array(arguments) if !arguments.is_empty() => {
                        arguments.iter().enumerate().try_for_each(|(index, one)| {
                            validate_operator(nested, one, &nested_at.index(index))
                        })
                    }
                    one => validate_operator(nested, one, &nested_at),
                }
            }
            _ => bad("a value, a list of values, or one nested string operator"),
        },
        "numeric" => {
            let Some(terms) = argument.as_array() else {
                return bad("an array of operator/value pairs");
            };
            if terms.is_empty() {
                return Err(at.refuse(Read::Whole, EMPTY_ARRAY));
            }
            // The list is read as the operator/value PAIRS it is: an
            // operator the comparison set does not hold, and a bound that
            // is not a number (a bound the list stops short of included),
            // are each named for what they are.
            for (pair_index, pair) in terms.chunks(2).enumerate() {
                // `chunks` never yields an empty chunk; reading the
                // operator as an OPTION keeps that a fact about this code
                // rather than an assumption about a caller's list.
                let Some(spelled) = pair.first() else {
                    continue;
                };
                let spelled_at = pair_index * 2;
                let Some(comparison) = spelled
                    .as_str()
                    .filter(|operator| NUMERIC_OPERATORS.contains(operator))
                else {
                    // The list is read PAIR by pair — an operator and the
                    // bound it compares against — so the reader has taken
                    // in the whole pair by the time it can say that the
                    // operator names no comparison, and the position it
                    // reports is just past the pair's last member. A pair
                    // the list stops short of a bound for ends at the list
                    // itself, which is as far as the reader got.
                    let reason = format!(
                        "Unrecognized numeric range operator: {}",
                        spelled_out(spelled)
                    );
                    return Err(match pair.len() {
                        1 => at.refuse(Read::Whole, reason),
                        _ => at.index(spelled_at + 1).refuse(Read::Whole, reason),
                    });
                };
                match pair.get(1) {
                    Some(bound) if bound.is_number() => {}
                    Some(_) => {
                        return Err(at
                            .index(spelled_at + 1)
                            .refuse(Read::Opening, format!("Value of {comparison} must be numeric")));
                    }
                    // The list stops short of the bound, so the reader has
                    // read the whole of it before it can tell.
                    None => {
                        return Err(at.refuse(
                            Read::Whole,
                            format!("Value of {comparison} must be numeric"),
                        ));
                    }
                }
            }
            // A range whose bottom is not below its top admits no number at
            // all, so it is a pattern that can never match rather than a
            // filter: refused instead of stored, which the whole list had to
            // be read to decide.
            if let Some(reason) = degenerate_range(terms) {
                return Err(at.refuse(Read::Whole, reason));
            }
            Ok(())
        }
        // The reader has entered the argument this name introduces, which
        // is as far as it gets before the name turns out to name no match
        // type at all.
        _ => Err(at.refuse(
            Read::Opening,
            format!("Unrecognized match type {operator}"),
        )),
    }
}

/// A pattern value as the refusal spells it back: a string as its own text,
/// anything else as the JSON it was written as.
fn spelled_out(value: &Value) -> String {
    match value.as_str() {
        Some(text) => text.to_string(),
        None => value.to_string(),
    }
}

/// Why `value` is not a wildcard value the language admits, or `None` when
/// it is one.
///
/// A `*` stands for any run of characters, so two in a row ask for nothing
/// the one does not: EventBridge refuses them and names the POSITION of the
/// second of the pair, counted in characters from the start of the value.
fn wildcard_violation(value: &str) -> Option<String> {
    let mut previous_was_wildcard = false;
    for (at, character) in value.chars().enumerate() {
        let is_wildcard = character == WILDCARD;
        if is_wildcard && previous_was_wildcard {
            return Some(format!("Consecutive wildcard characters at pos {at}"));
        }
        previous_was_wildcard = is_wildcard;
    }
    None
}

/// Why the bounds of a `numeric` term list admit no number, or `None` when at
/// least one number satisfies all of them.
///
/// The bounds are reduced to the TIGHTEST lower and upper one, which is what
/// the conjunction of the pairs means, and the range is empty when the lower
/// one is above the upper one — or equal to it while either end is exclusive.
fn degenerate_range(terms: &[Value]) -> Option<&'static str> {
    // (the bound, whether that end is exclusive)
    let mut lower: Option<(f64, bool)> = None;
    let mut upper: Option<(f64, bool)> = None;
    for pair in terms.chunks(2) {
        let (Some(operator), Some(bound)) = (
            pair.first().and_then(Value::as_str),
            pair.get(1).and_then(Value::as_f64),
        ) else {
            continue;
        };
        let tighten = |held: Option<(f64, bool)>, bound: f64, exclusive: bool, upwards: bool| {
            match held {
                Some((kept, kept_exclusive))
                    if (upwards && kept > bound) || (!upwards && kept < bound) =>
                {
                    Some((kept, kept_exclusive))
                }
                Some((kept, kept_exclusive)) if kept == bound => {
                    Some((kept, kept_exclusive || exclusive))
                }
                _ => Some((bound, exclusive)),
            }
        };
        match operator {
            ">" => lower = tighten(lower, bound, true, true),
            ">=" => lower = tighten(lower, bound, false, true),
            "<" => upper = tighten(upper, bound, true, false),
            "<=" => upper = tighten(upper, bound, false, false),
            "=" => {
                lower = tighten(lower, bound, false, true);
                upper = tighten(upper, bound, false, false);
            }
            _ => {}
        }
    }
    match (lower, upper) {
        (Some((low, low_exclusive)), Some((high, high_exclusive)))
            if low > high || (low == high && (low_exclusive || high_exclusive)) =>
        {
            Some(BOTTOM_BELOW_TOP)
        }
        _ => None,
    }
}

/// Every field of a pattern object must match, which is what makes a
/// pattern a conjunction over fields.
fn match_object(pattern: &Map<String, Value>, event: &Value, budget: &mut Budget) -> bool {
    for (key, spec) in pattern {
        if !budget.charge(1) {
            return false;
        }
        let held = if key == "$or" && relates_objects(spec) {
            match_any_branch(spec, event, budget)
        } else {
            let field = event.get(key);
            match spec {
                // One of the listed terms must match, which is what makes a
                // term list a disjunction.
                Value::Array(terms) => {
                    let mut held = false;
                    for term in terms {
                        if match_term(term, field, budget) {
                            held = true;
                            break;
                        }
                    }
                    held
                }
                Value::Object(child) => {
                    // "exists only works on leaf nodes": a sub-pattern whose
                    // PARENT the event does not carry is evaluated against
                    // absence rather than failed outright, so an
                    // `exists: false` term holds when an intermediate parent
                    // is missing — the leaf is absent either way.
                    match field {
                        Some(value) => match_object(child, value, budget),
                        None => match_object(child, &Value::Null, budget),
                    }
                }
                _ => false,
            }
        };
        if !held {
            return false;
        }
    }
    true
}

/// Whether `spec` is the or-RELATIONSHIP `$or` names: a list of at least
/// [`OR_RELATED_OBJECTS`] pattern objects.
///
/// The same reading the parse takes ([`validate_object`]), so what a pattern
/// was ADMITTED as is what it is matched as: a `$or` relating too little to
/// be a relationship is the ordinary field — named `$or`, which no event
/// envelope carries — that the parse read it as.
fn relates_objects(spec: &Value) -> bool {
    spec.as_array().is_some_and(|branches| {
        branches.len() >= OR_RELATED_OBJECTS && branches.iter().all(Value::is_object)
    })
}

/// `$or`: at least one branch, itself a pattern object, must match.
fn match_any_branch(spec: &Value, event: &Value, budget: &mut Budget) -> bool {
    let Some(branches) = spec.as_array() else {
        return false;
    };
    for branch in branches {
        let Some(object) = branch.as_object() else {
            continue;
        };
        if match_object(object, event, budget) {
            return true;
        }
    }
    false
}

/// One term against the event's value for that field, absent or present.
fn match_term(term: &Value, field: Option<&Value>, budget: &mut Budget) -> bool {
    let Value::Object(map) = term else {
        // A literal term matches the value, or any member of it when the
        // event carries a list there.
        let Some(value) = field else {
            return false;
        };
        return any_leaf(value, |leaf| {
            budget.charge(comparison_cost(term, leaf)) && same_value(leaf, term)
        });
    };
    let Some((operator, argument)) = map.iter().next().filter(|_| map.len() == 1) else {
        return false;
    };
    if operator == "exists" {
        // The only operator that is about PRESENCE, so it is the only one
        // that can match an absent field. "exists only works on leaf nodes.
        // It does not work on intermediate nodes": a field whose value is an
        // object, or an array carrying no leaf at all, is therefore absent as
        // far as this operator is concerned ([`leaf_exists`]).
        // Charged by what the presence test may WALK: "exists only works on
        // leaf nodes", and finding whether the field carries one descends
        // every member of an array-valued field ([`leaf_exists`]), so the
        // charge is the size of the value rather than one unit.
        return budget.charge(field.map_or(1, argument_cost))
            && argument.as_bool() == Some(field.is_some_and(leaf_exists));
    }
    let Some(value) = field else {
        return false;
    };
    any_leaf(value, |leaf| {
        // Charged BEFORE the comparison, by the number of bytes the
        // comparison may walk — the argument at every level, and the leaf
        // once per argument tried against it ([`comparison_cost`]) — so the
        // budget bounds the work rather than merely counting calls.
        budget.charge(comparison_cost(argument, leaf))
            && match_operator(operator, argument, leaf)
    })
}

/// Whether `value` is a LEAF the `exists` operator sees: any scalar — a
/// string, a number, a boolean, or `null` — or an array carrying at least
/// one such member.
///
/// `null` IS such a leaf: a field whose value is `null` is one the event
/// carries, so `exists: true` holds for it and `exists: false` does not. An
/// object and an empty array are what carry no leaf, so a field holding one
/// of them is absent for `exists` — which is what "exists only works on leaf
/// nodes. It does not work on intermediate nodes" means.
///
/// The search descends an EVENT's value, which nests as deep as
/// [`super::MAX_DETAIL_DEPTH`] admits, so it descends on an explicit stack for
/// the reason [`argument_cost`] gives.
fn leaf_exists(value: &Value) -> bool {
    let mut pending: Vec<&Value> = ::std::vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(_) => {}
            Value::Array(members) => pending.extend(members.iter()),
            _ => return true,
        }
    }
    false
}

/// Whether a pattern's literal term and an event's leaf are the same value.
///
/// Type-sensitive, as EventBridge's exact matching is — a string never equals a
/// number — but two NUMBERS are compared numerically rather than by their JSON
/// spelling, so `1` and `1.0` are one value and a pattern does not depend on
/// how the publisher wrote it.
fn same_value(leaf: &Value, term: &Value) -> bool {
    match (leaf, term) {
        (Value::Number(found), Value::Number(wanted)) => {
            found == wanted || found.as_f64() == wanted.as_f64()
        }
        (found, wanted) => found == wanted,
    }
}

/// Apply `predicate` to `value`, or to any member of it when the event
/// carries a list: EventBridge matches a pattern term against the MEMBERS
/// of an array-valued field.
fn any_leaf<F: FnMut(&Value) -> bool>(value: &Value, mut predicate: F) -> bool {
    match value {
        Value::Array(members) => members.iter().any(&mut predicate),
        leaf => predicate(leaf),
    }
}

/// One operator against one leaf value.
fn match_operator(operator: &str, argument: &Value, leaf: &Value) -> bool {
    match operator {
        "prefix" => with_string_argument(argument, leaf, Affix::Prefix),
        "suffix" => with_string_argument(argument, leaf, Affix::Suffix),
        "equals-ignore-case" => match (leaf.as_str(), argument.as_str()) {
            (Some(text), Some(wanted)) => text.eq_ignore_ascii_case(wanted),
            _ => false,
        },
        "wildcard" => match (leaf.as_str(), argument.as_str()) {
            (Some(text), Some(wanted)) => matches_wildcard(text, wanted),
            _ => false,
        },
        "cidr" => match (
            leaf.as_str().and_then(|text| text.parse::<IpAddr>().ok()),
            argument.as_str().and_then(|block| parse_cidr(block).ok()),
        ) {
            (Some(address), Some((network, bits))) => in_cidr(address, network, bits),
            _ => false,
        },
        "anything-but" => !matches_anything_but(argument, leaf),
        "numeric" => match (leaf.as_f64(), argument.as_array()) {
            (Some(number), Some(terms)) => {
                terms.chunks(2).all(|pair| matches_numeric(pair, number))
            }
            _ => false,
        },
        _ => false,
    }
}

/// Which end of the leaf a `prefix`/`suffix` term anchors at.
#[derive(Clone, Copy)]
enum Affix {
    Prefix,
    Suffix,
}

/// `prefix`/`suffix`, whose argument is either a plain string or a nested
/// `{"equals-ignore-case": "<string>"}` asking for a case-folded compare.
fn with_string_argument(argument: &Value, leaf: &Value, affix: Affix) -> bool {
    let Some(text) = leaf.as_str() else {
        return false;
    };
    match argument {
        Value::String(wanted) => match affix {
            Affix::Prefix => text.starts_with(wanted.as_str()),
            Affix::Suffix => text.ends_with(wanted.as_str()),
        },
        Value::Object(map) => map
            .get("equals-ignore-case")
            .and_then(Value::as_str)
            .is_some_and(|wanted| matches_folded_affix(text, wanted, affix)),
        _ => false,
    }
}

/// A case-folded `prefix`/`suffix` compare that walks only as many
/// characters as the ARGUMENT has.
///
/// Deliberately not `text.to_lowercase()`: folding the whole leaf would
/// allocate a copy of it — up to a whole event's `Detail` — for every term
/// of every rule, while only the argument's own length is ever compared.
fn matches_folded_affix(text: &str, wanted: &str, affix: Affix) -> bool {
    match affix {
        Affix::Prefix => {
            let mut found = text.chars();
            wanted
                .chars()
                .all(|expected| found.next().is_some_and(|actual| folds_to(actual, expected)))
        }
        Affix::Suffix => {
            let mut found = text.chars().rev();
            wanted
                .chars()
                .rev()
                .all(|expected| found.next().is_some_and(|actual| folds_to(actual, expected)))
        }
    }
}

/// Whether two characters are equal ignoring case.
fn folds_to(left: char, right: char) -> bool {
    left == right
        || left
            .to_lowercase()
            .eq(right.to_lowercase())
}

/// What `anything-but` negates: a value, any member of a list of values, or
/// one nested string operator.
fn matches_anything_but(argument: &Value, leaf: &Value) -> bool {
    match argument {
        Value::Array(values) => values.iter().any(|value| same_value(leaf, value)),
        Value::Object(map) => map.iter().next().is_some_and(|(nested, nested_argument)| {
            match nested_argument {
                // A nested string operator may list several arguments, any
                // one of which excludes the leaf.
                Value::Array(arguments) => arguments
                    .iter()
                    .any(|one| match_operator(nested, one, leaf)),
                one => match_operator(nested, one, leaf),
            }
        }),
        value => same_value(leaf, value),
    }
}

/// One `numeric` operator/value pair against a number.
fn matches_numeric(pair: &[Value], number: f64) -> bool {
    let (Some(operator), Some(bound)) = (
        pair.first().and_then(Value::as_str),
        pair.get(1).and_then(Value::as_f64),
    ) else {
        return false;
    };
    match operator {
        "=" => number == bound,
        "<" => number < bound,
        "<=" => number <= bound,
        ">" => number > bound,
        ">=" => number >= bound,
        _ => false,
    }
}

/// `wildcard`: `*` stands for any run of characters, every other character
/// is literal.
///
/// Matched by splitting the pattern on `*` and locating each literal
/// segment in order, advancing through the text MONOTONICALLY: the first
/// segment anchors at the start, the last at the end, and each one in
/// between is found at its leftmost remaining position. Greedy-leftmost is
/// exact for a `*`-only glob — a `*` absorbs any run, so a later segment
/// can always use whatever an earlier one left — and because the cursor
/// never rewinds, the whole match costs O(len(text) + len(pattern)). The
/// previous shape (a cursor that rewound to the last `*` on a mismatch) was
/// O(len(text) x len(pattern)) and could be driven quadratically by a
/// caller choosing both operands.
fn matches_wildcard(text: &str, pattern: &str) -> bool {
    let segments: Vec<&str> = pattern.split('*').collect();
    // `split` always yields at least one segment; exactly one means the
    // pattern has no `*` at all and is therefore a literal.
    let (Some(first), Some(last)) = (segments.first(), segments.last()) else {
        return false;
    };
    if segments.len() == 1 {
        return text == pattern;
    }
    if !text.starts_with(first) || !text.ends_with(last) {
        return false;
    }
    // The two anchors must not overlap: `a*b` needs room for both.
    let mut rest = &text[first.len()..];
    if rest.len() < last.len() {
        return false;
    }
    rest = &rest[..rest.len() - last.len()];
    for segment in &segments[1..segments.len() - 1] {
        match rest.find(segment) {
            Some(at) => rest = &rest[at + segment.len()..],
            None => return false,
        }
    }
    true
}

/// Why text is not a CIDR block this language compares against.
///
/// The two cases are refused by DIFFERENT wordings, so they are kept apart
/// rather than flattened into one message: a mask too wide is a rule of the
/// notation, which the service states for the notation, while text that is
/// no block at all is the `cidr` operator's own argument check (see
/// [`validate_operator`]).
enum CidrRefusal {
    /// The mask leaves no RANGE of addresses, carrying the reason that
    /// states the family's width.
    MaskNotNarrowerThanTheFamily(String),
    /// The text is not an address and a mask length at all.
    NotABlock,
}

/// Split `a.b.c.d/len` into its network address and prefix length, or say
/// why the text is not a CIDR block this language compares against.
///
/// The mask must leave a RANGE of addresses for the comparison to be a
/// range comparison at all, so it is narrower than the address family's
/// width — a mask as wide as the family names one address rather than a
/// block, and EventBridge refuses it naming that width. A CIDR block also
/// names a NETWORK, so every bit beyond its prefix is zero; text whose
/// address sets one of them names a host inside a block rather than the
/// block.
fn parse_cidr(block: &str) -> Result<(IpAddr, u8), CidrRefusal> {
    let Some((address, length)) = block.split_once('/') else {
        return Err(CidrRefusal::NotABlock);
    };
    let Ok(network) = address.parse::<IpAddr>() else {
        return Err(CidrRefusal::NotABlock);
    };
    let Ok(bits) = length.parse::<u8>() else {
        return Err(CidrRefusal::NotABlock);
    };
    let (family, width) = if network.is_ipv4() {
        ("IPv4", 32u8)
    } else {
        ("IPv6", 128u8)
    };
    if bits >= width {
        return Err(CidrRefusal::MaskNotNarrowerThanTheFamily(format!(
            "{family} mask bits must be < {width}"
        )));
    }
    if !is_network_address(network, bits) {
        return Err(CidrRefusal::NotABlock);
    }
    Ok((network, bits))
}

/// Whether every bit of `network` outside its first `bits` is zero.
fn is_network_address(network: IpAddr, bits: u8) -> bool {
    let octets: Vec<u8> = match network {
        IpAddr::V4(address) => address.octets().to_vec(),
        IpAddr::V6(address) => address.octets().to_vec(),
    };
    let mut remaining = usize::from(bits);
    for octet in octets {
        let take = remaining.min(8);
        remaining -= take;
        let mask = u8::try_from(0xffu16 << (8 - take) & 0xff).unwrap_or(0xff);
        if octet & !mask != 0 {
            return false;
        }
    }
    true
}

/// Whether `address` is inside the block `network/bits`.
fn in_cidr(address: IpAddr, network: IpAddr, bits: u8) -> bool {
    let (left, right) = match (address, network) {
        (IpAddr::V4(address), IpAddr::V4(network)) => {
            (address.octets().to_vec(), network.octets().to_vec())
        }
        (IpAddr::V6(address), IpAddr::V6(network)) => {
            (address.octets().to_vec(), network.octets().to_vec())
        }
        // Different families never overlap.
        _ => return false,
    };
    let mut remaining = usize::from(bits);
    for (a, b) in left.iter().zip(right.iter()) {
        if remaining == 0 {
            break;
        }
        let take = remaining.min(8);
        let mask = u8::try_from(0xffu16 << (8 - take) & 0xff).unwrap_or(0xff);
        if a & mask != b & mask {
            return false;
        }
        remaining -= take;
    }
    true
}
