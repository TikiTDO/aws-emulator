// SPDX-License-Identifier: CC0-1.0

//! What a definition is VALIDATED against: every problem one carries, where it is,
//! and whether it prevents a state machine from being created with it.
//!
//! One concern, split out of [`super`] so that what the interpreter RUNS is read
//! apart from what the service ACCEPTS. A definition this emulator cannot run is
//! still valid Amazon States Language, so this says nothing about the walk: it
//! reports the language's own structural rules, which `CreateStateMachine`,
//! `UpdateStateMachine` and `ValidateStateMachineDefinition` read through
//! [`validate`], [`validate_single_state`] and [`diagnose`].
//!
//! Every budget here is spent against a CALLER-SIZED document (the modelled
//! `Definition` is 1 MiB): the diagnostics one walk collects, the characters one
//! location carries and the states the variable-scope analysis visits each carry
//! their own ceiling, so the work of one request is never the product of the state
//! count and the nesting.

use super::*;

/// The diagnostic code `INVALID_JSON_DESCRIPTION`: "JSON syntax problem found".
///
/// The codes below are the ones this emulator's validation can reach, out of
/// the error-code list the model documents on
/// `ValidateStateMachineDefinitionDiagnostic`
/// (`smithy_query(query="doc",shape="ValidateStateMachineDefinitionDiagnostic")`).
/// That same documentation is why the code is the stable part of an
/// `InvalidDefinition` message and the detail after it is not: "might add new
/// diagnostics in the future, adjust diagnostic codes, or change the message
/// wording. […] Do not rely on the exact order, count, or wording of diagnostic
/// messages."
pub const INVALID_JSON_DESCRIPTION: &str = "INVALID_JSON_DESCRIPTION";

/// `SCHEMA_VALIDATION_FAILED`: "Schema validation reported errors" — the definition
/// is JSON, but not the shape the Amazon States Language declares.
pub const SCHEMA_VALIDATION_FAILED: &str = "SCHEMA_VALIDATION_FAILED";

/// `UNSUPPORTED_JSONATA_EXPRESSION`: the definition is the shape the language
/// declares, but one of its JSONata expressions is not one the service accepts —
/// a reserved term read where the language does not bind it, which is a problem with
/// the EXPRESSION rather than with the shape of the document around it. The model's
/// own list of codes is not closed ("might add new diagnostics in the future"), so a
/// code it does not list is still a code the service reports.
pub const UNSUPPORTED_JSONATA_EXPRESSION: &str = "UNSUPPORTED_JSONATA_EXPRESSION";

/// `MISSING_TRANSITION_TARGET`: "The value of \"Next\" field doesn't match a known
/// state name" — reported for every transition target that names no declared state,
/// `StartAt`'s included.
pub const MISSING_TRANSITION_TARGET: &str = "MISSING_TRANSITION_TARGET";

/// `INVALID_STATE_NAME`: "The state name does not follow the naming convention".
pub const INVALID_STATE_NAME: &str = "INVALID_STATE_NAME";

/// `MISSING_END_STATE`: "The workflow does not have a terminal state" — no state the
/// state machine DECLARES ends an execution, so nothing it can run ever stops.
pub const MISSING_END_STATE: &str = "MISSING_END_STATE";

/// `MISSING_DESCRIPTION`: "Received a null or empty workflow input" — the request
/// carried no document to validate at all.
pub const MISSING_DESCRIPTION: &str = "MISSING_DESCRIPTION";

/// `TOO_DEEPLY_NESTED`: "The states are too deeply nested".
pub const TOO_DEEPLY_NESTED: &str = "TOO_DEEPLY_NESTED";

/// `NO_DOLLAR`: "No `.$` on a field that appears to be a JSONPath or Intrinsic
/// Function" — a WARNING, because the field is rendered as the literal it is
/// rather than refused.
pub const NO_DOLLAR: &str = "NO_DOLLAR";

/// `NO_PATH`: "Field value looks like a path, but field name does not end with
/// 'Path'" — a WARNING, for the same reason.
pub const NO_PATH: &str = "NO_PATH";

/// `PASS_RESULT_IS_STATIC`: "Attempt to use a path in the result of a pass state"
/// — a WARNING: a `Pass` state's `Result` is its literal result.
pub const PASS_RESULT_IS_STATIC: &str = "PASS_RESULT_IS_STATIC";

/// `POSSIBLY_UNDEFINED_VARIABLE`: a field reads a variable that some run of the
/// definition can reach that field without having assigned — a WARNING, because the
/// read resolves to nothing rather than preventing the definition from being created.
/// The model's list of codes is not closed ("might add new diagnostics in the
/// future"), so a code it does not list is still a code the service reports.
pub const POSSIBLY_UNDEFINED_VARIABLE: &str = "POSSIBLY_UNDEFINED_VARIABLE";

/// The most diagnostics `ValidateStateMachineDefinition` reports in one call, which
/// is also what it reports when the request asks for none in particular
/// (`maxResults`: "The maximum number of diagnostics that are returned per call.
/// The default and maximum value is 100. Setting the value to 0 will also use the
/// default of 100.").
pub const DEFAULT_MAX_DIAGNOSTICS: usize = 100;

/// How many diagnostics of ONE severity a walk collects.
///
/// A definition is caller-sized (the modelled `Definition` is 1 MiB), so the list
/// this walk builds carries its own ceiling like every other walk here. It is one
/// past [`DEFAULT_MAX_DIAGNOSTICS`], which is `maxResults`' modelled maximum: no
/// request can ask for more than that, so one more than the maximum is all it takes
/// to report `truncated`.
///
/// The budget is per SEVERITY, and the walk stops only once the ERROR budget is
/// spent: a definition's warnings can therefore never crowd out an error, which is
/// what [`validate`] — and so what `CreateStateMachine` refuses — reads.
pub const MAX_DIAGNOSTICS: usize = DEFAULT_MAX_DIAGNOSTICS + 1;

/// The most characters of a caller's own value one diagnostic echoes.
///
/// A definition is caller-sized and its member names, state names and string values
/// are caller-chosen, so a message that echoed one whole would let one request be
/// amplified into a much larger response. The cap is the longest a state name may
/// be ([`MAX_STATE_NAME_CHARS`]), so a value inside the language's own bounds is
/// quoted in full and only one past them is clipped.
pub const MAX_ECHOED_CHARS: usize = MAX_STATE_NAME_CHARS;
/// The most characters one diagnostic's `location` carries where that location is a
/// PATH into the definition, and the most any path the walk builds carries: the path
/// to a state nests a caller-chosen name per level, so it is clipped AS IT IS BUILT
/// ([`nested_at`]) rather than only where it is reported.
///
/// ONE diagnostic is located somewhere else: a definition that is not JSON at all has
/// no path into it, so it is located at the position the parse ended at
/// ([`crate::state::json_parse_definition_location`]), whose own quoting of the
/// document is what bounds it.
pub const MAX_LOCATION_CHARS: usize = 256;

/// `value` as a diagnostic echoes it: itself when it is short, and its first
/// [`MAX_ECHOED_CHARS`] characters followed by an ellipsis when it is not.
///
/// Public so the one clipping of a quoted caller value serves every cause this
/// crate words, including the ones [`super::super::integration`] builds for a `Task`
/// state's `Resource`.
pub fn echoed(value: &str) -> String {
    clipped(value, MAX_ECHOED_CHARS)
}

/// `value` clipped to `cap` characters, with an ellipsis marking what was dropped.
///
/// The ellipsis is one of the `cap`, so the result never carries more characters
/// than the cap allows.
///
/// Not [`super::clip_cause`], which is the other truncation this module does and a
/// different one: that cuts a `cause` to the BYTES the modelled `SensitiveCause`
/// carries and must leave the text it keeps exactly as the run produced it, since
/// it is a reported value rather than a quotation of one.
pub fn clipped(value: &str, cap: usize) -> String {
    if value.chars().count() <= cap {
        return value.to_string();
    }
    let kept: String = value.chars().take(cap.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

/// One problem found in a definition: what it is, where it is, and whether it
/// prevents creating or updating a state machine with that definition.
///
/// `severity` is [`crate::state::SEVERITY_ERROR`] or
/// [`crate::state::SEVERITY_WARNING`], `code` one of the codes above, and
/// `location` the `/States/<StateName>/<FieldName>` form the `location` member of
/// `ValidateStateMachineDefinitionDiagnostic` documents, for a diagnostic a field
/// or a state owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: &'static str,
    pub code: &'static str,
    pub message: String,
    pub location: Option<String>,
}

/// The place an `InvalidDefinition` subject states for a diagnostic that sits at no
/// position in the definition ([`Diagnostic::stated`]).
///
/// RECORDED text: the refusal states the place of every subject, and for a diagnostic
/// about the definition as a whole that place is the bare word `null`.
const NO_LOCATION_STATED: &str = "null";

impl Diagnostic {
    /// Whether this diagnostic prevents a create or an update: "A value of `ERROR`
    /// means that you cannot create or update a state machine with this
    /// definition."
    pub fn is_error(&self) -> bool {
        self.severity == crate::state::SEVERITY_ERROR
    }

    /// This diagnostic as the SUBJECT of an `InvalidDefinition` message: the code
    /// that classifies it, the detail describing it, and where it was found.
    ///
    /// The place is joined to the detail by the bare word ` at ` and nothing else, so
    /// the subject reads as one sentence and a client that parses the location out of
    /// a refusal finds it unwrapped.
    ///
    /// EVERY subject states a place, including a diagnostic about the definition AS A
    /// WHOLE — a workflow that declares no terminal state is the recorded one — which
    /// sits at no position in the document: that place is stated as
    /// [`NO_LOCATION_STATED`], the way the refusal RECORDS it, so a client parsing the
    /// location out of a subject finds the same field in every one of them. The
    /// `location` MEMBER of a `ValidateStateMachineDefinition` diagnostic is a
    /// different report of the same fact and stays absent there.
    pub(super) fn stated(&self) -> String {
        let location = self.location.as_deref().unwrap_or(NO_LOCATION_STATED);
        format!("{}: {} at {location}", self.code, self.message)
    }
}

/// One diagnostic of the given severity.
pub fn diagnostic(
    severity: &'static str,
    code: &'static str,
    location: Option<String>,
    message: impl ::std::fmt::Display,
) -> Diagnostic {
    Diagnostic {
        severity,
        code,
        message: message.to_string(),
        location,
    }
}

/// One ERROR diagnostic: a definition carrying it cannot be created or updated.
pub fn error(code: &'static str, location: Option<String>, message: impl ::std::fmt::Display) -> Diagnostic {
    diagnostic(crate::state::SEVERITY_ERROR, code, location, message)
}

/// One WARNING diagnostic: a definition carrying it is still created.
pub fn warning(code: &'static str, location: Option<String>, message: impl ::std::fmt::Display) -> Diagnostic {
    diagnostic(crate::state::SEVERITY_WARNING, code, location, message)
}

/// Where one diagnostic was found: the enclosing machine's own location followed
/// by `member`, or `None` for the definition as a whole ([`Diagnostic::stated`] is
/// what a refusal states an absent one as).
///
/// `prefix` is itself bounded (every caller builds one through [`nested_at`]), so
/// the only caller-sized part of a location is the one member named here.
pub fn at(prefix: &str, member: &str) -> Option<String> {
    let location = format!("{prefix}{member}");
    (!location.is_empty()).then(|| clipped(&location, MAX_LOCATION_CHARS))
}

/// The location to hand to something nested inside `prefix`: `prefix` followed by
/// `member`, clipped AS IT IS BUILT.
///
/// The path to a state nests one caller-chosen name per level and is then built
/// once per state, so a path that grew with the definition would make the work of
/// one request the PRODUCT of the state count and the nesting; clipping here is
/// what keeps every path handed down O([`MAX_LOCATION_CHARS`]).
pub fn nested_at(prefix: &str, member: impl ::std::fmt::Display) -> String {
    clipped(&format!("{prefix}{member}"), MAX_LOCATION_CHARS)
}

/// The location of the element at `index` of the array `array` locates: the array's
/// own location with the index BRACKETED onto it.
///
/// RECORDED form, and the ONE place an index is written into a location, so every
/// diagnostic about an element of a `Retry`, `Catch`, `Choices`, `Branches` or
/// `ErrorEquals` array addresses it the same way. The index belongs to the FIELD that
/// holds the array rather than being a segment of its own, so a client reading the
/// location to point at the offending element of the document it submitted resolves
/// the node the diagnostic is about.
pub fn indexed_at(array: &str, index: usize) -> String {
    nested_at(array, format_args!("[{index}]"))
}

/// The location of the element at `index` of the array held by `member` of the object
/// at `prefix` — [`indexed_at`] of that member's own location.
pub fn element_at(prefix: &str, member: impl ::std::fmt::Display, index: usize) -> String {
    indexed_at(&nested_at(prefix, format_args!("/{member}")), index)
}

/// Where a diagnostic about the state machine OBJECT itself was found: the object's
/// own location, which for the definition as a whole is the document's root.
///
/// [`at`] answers `None` for the root, because a diagnostic about a MEMBER of the
/// root carries that member's name; a diagnostic about the object carries the object,
/// and the root object is `/`.
pub fn machine_at(prefix: &str) -> Option<String> {
    Some(if prefix.is_empty() {
        "/".to_string()
    } else {
        clipped(prefix, MAX_LOCATION_CHARS)
    })
}

/// The fields the Amazon States Language declares on a state machine object, in the
/// order its schema states them.
///
/// From the language specification's "State Machine Structure": a state machine
/// carries `Comment`, `StartAt`, `TimeoutSeconds`, `Version`, `States` and
/// `QueryLanguage`. The set is CLOSED — a field outside it is a field the language
/// does not define — which is what [`field_not_supported_message`] reports.
const MACHINE_FIELDS: &[&str] = &[
    "Comment",
    "QueryLanguage",
    "StartAt",
    "States",
    "TimeoutSeconds",
    "Version",
];

/// The fields a state machine declared as a `Map` state's processor carries: the
/// machine's own, and the `ProcessorConfig` that says how that state runs it.
///
/// A processor is the one position the language declares that field at, so a
/// definition or a `Parallel` branch carrying it is carrying a field the language
/// does not declare THERE — which is what [`machine_fields`] keys on.
const PROCESSOR_FIELDS: &[&str] = &[
    "Comment",
    "ProcessorConfig",
    "QueryLanguage",
    "StartAt",
    "States",
    "TimeoutSeconds",
    "Version",
];

/// Which closed set a state machine object's fields are judged against: a `Map`
/// state's processor admits one field more than a definition or a branch does.
pub fn machine_fields(processor: bool) -> &'static [&'static str] {
    if processor {
        PROCESSOR_FIELDS
    } else {
        MACHINE_FIELDS
    }
}

/// The fields a state machine object must carry, in the order the refusal lists them.
///
/// "A State Machine MUST have an object field named `States`" and "a string field
/// named `StartAt`", which are the two the schema requires; the order here is the
/// order the service lists them in, and it is the LIST that is reported rather than
/// one diagnostic per field.
pub const MACHINE_REQUIRED_FIELDS: &[&str] = &["States", "StartAt"];

/// What a schema diagnostic says about required fields an object does not carry: the
/// fields, in brackets, as a list.
///
/// RECORDED text. One diagnostic names them all, so a caller is told everything the
/// object owes rather than one field per attempt.
pub fn required_fields_message(fields: &[&str]) -> String {
    format!("These fields are required: [{}]", fields.join(", "))
}

/// What a schema diagnostic says about a field the language does not declare: the
/// field, quoted, and that it is not supported.
///
/// RECORDED text. The field name is the caller's own JSON key, so it is clipped by
/// [`echoed`] like every other caller value a diagnostic quotes.
pub fn field_not_supported_message(field: &str) -> String {
    format!("Field '{}' is not supported", echoed(field))
}

/// The fields the Amazon States Language declares on a state of each type, in the
/// order its schema states them.
///
/// Each set is CLOSED, exactly as a state machine object's is ([`machine_fields`]): a
/// state's `Type` selects what the state DOES, and the language declares per type the
/// fields that say how it does it, so a field outside the set of the type the state
/// declares is a field the language does not define THERE —
/// [`field_not_supported_message`] is what reports it. A type the language does not
/// define at all carries no set, because nothing says what such a state would do; it
/// is reported as the type it names instead ([`diagnose_state`]).
///
/// Each set is the UNION of what the two query languages declare for the type — the
/// `*Path` family and the payload templates of JSONPath beside `Arguments`, `Output`
/// and `Assign` of JSONata ([`super::executable_members`] and
/// [`super::jsonata_members`] are the same split for what this emulator RUNS) — plus
/// the fields the language declares that this emulator does not execute, since a
/// definition using one is still valid Amazon States Language. Which language's
/// members a state may MIX is a different rule, reported on its own
/// ([`diagnose_jsonata_state`]).
pub fn state_fields(state_type: &str) -> &'static [&'static str] {
    match state_type {
        "Pass" => &[
            "Assign",
            "Comment",
            "End",
            "InputPath",
            "Next",
            "Output",
            "OutputPath",
            "Parameters",
            "QueryLanguage",
            "Result",
            "ResultPath",
            "Type",
        ],
        "Wait" => &[
            "Assign",
            "Comment",
            "End",
            "InputPath",
            "Next",
            "Output",
            "OutputPath",
            "QueryLanguage",
            "Seconds",
            "SecondsPath",
            "Timestamp",
            "TimestampPath",
            "Type",
        ],
        // A `Choice` goes where its own rules send it, so it declares neither `Next`
        // nor `End`, and it produces no result of its own, so none of the result-side
        // members either.
        "Choice" => &[
            "Assign",
            "Choices",
            "Comment",
            "Default",
            "InputPath",
            "Output",
            "OutputPath",
            "QueryLanguage",
            "Type",
        ],
        // The two types that END an execution: there is nowhere for them to go, and no
        // failure of theirs for a retrier or a catcher to take over. The output a
        // `Succeed` puts out is the execution's own.
        "Succeed" => &[
            "Assign",
            "Comment",
            "InputPath",
            "Output",
            "OutputPath",
            "QueryLanguage",
            "Type",
        ],
        "Fail" => &[
            "Cause",
            "CausePath",
            "Comment",
            "Error",
            "ErrorPath",
            "QueryLanguage",
            "Type",
        ],
        // The three types whose work can FAIL — a `Task`'s invocation, and the
        // machines a `Map` and a `Parallel` run — are the ones that declare `Retry`
        // and `Catch`.
        "Task" => &[
            "Arguments",
            "Assign",
            "Catch",
            "Comment",
            "Credentials",
            "End",
            "HeartbeatSeconds",
            "HeartbeatSecondsPath",
            "InputPath",
            "Next",
            "Output",
            "OutputPath",
            "Parameters",
            "QueryLanguage",
            "Resource",
            "ResultPath",
            "ResultSelector",
            "Retry",
            "TimeoutSeconds",
            "TimeoutSecondsPath",
            "Type",
        ],
        "Parallel" => &[
            "Arguments",
            "Assign",
            "Branches",
            "Catch",
            "Comment",
            "End",
            "InputPath",
            "Next",
            "Output",
            "OutputPath",
            "Parameters",
            "QueryLanguage",
            "ResultPath",
            "ResultSelector",
            "Retry",
            "Type",
        ],
        "Map" => &[
            "Assign",
            "Catch",
            "Comment",
            "End",
            "InputPath",
            "ItemBatcher",
            "ItemProcessor",
            "ItemReader",
            "ItemSelector",
            "Items",
            "ItemsPath",
            "Iterator",
            "Label",
            "MaxConcurrency",
            "MaxConcurrencyPath",
            "Next",
            "Output",
            "OutputPath",
            "Parameters",
            "QueryLanguage",
            "ResultPath",
            "ResultSelector",
            "ResultWriter",
            "Retry",
            "ToleratedFailureCount",
            "ToleratedFailureCountPath",
            "ToleratedFailurePercentage",
            "ToleratedFailurePercentagePath",
            "Type",
        ],
        _ => &[],
    }
}

/// What a diagnostic says about a transition target no state declares: the name it
/// named, as the value of the transition field the language calls `Next`.
///
/// RECORDED text. One construction serves every member a transition is named
/// through — a state's own `Next`, a `Choice` rule's, a `Catch` catcher's and the
/// `Default` beside them, and the machine's own `StartAt` — because they are one
/// problem: a name that resolves to no state of the enclosing `States` map. The field
/// the sentence names is `Next` whichever member declared the transition, as the code
/// it is reported under documents ("The value of \"Next\" field doesn't match a known
/// state name"); WHICH member that was is what `location` carries. The target is the
/// caller's own string, so it is clipped by [`echoed`].
pub fn missing_target_message(target: &str) -> String {
    format!("Missing 'Next' target: {}", echoed(target))
}

/// What a diagnostic says about a state nothing in the definition leads to: the
/// state, quoted, and that nothing reaches it.
///
/// RECORDED text. It is reported under the same code an unresolved transition target
/// is, because it is the other half of one rule about the transition graph: a state
/// machine's states and the transitions between them must agree, so a name that
/// resolves to no state and a state no name resolves to are both reported. The name is
/// the caller's own, so it is clipped by [`echoed`].
pub fn unreachable_state_message(name: &str) -> String {
    format!("State \"{}\" is not reachable.", echoed(name))
}

/// The reserved error name that stands for every error a state can report.
pub const ALL_ERRORS: &str = "States.ALL";

/// The fields the Amazon States Language declares on one `Retry` retrier, in the
/// order its schema states them.
///
/// From the language's own error-handling rules: a retrier names the errors it takes
/// over (`ErrorEquals`) and how the attempts it owes are spaced (`IntervalSeconds`,
/// `MaxAttempts`, `BackoffRate`, `MaxDelaySeconds`, `JitterStrategy`). The set is
/// CLOSED, like every other object's here.
const RETRIER_FIELDS: &[&str] = &[
    "BackoffRate",
    "Comment",
    "ErrorEquals",
    "IntervalSeconds",
    "JitterStrategy",
    "MaxAttempts",
    "MaxDelaySeconds",
];

/// The field a retrier must carry: the errors it takes over.
const RETRIER_REQUIRED_FIELDS: &[&str] = &["ErrorEquals"];

/// The fields the language declares on one `Catch` catcher: the errors it takes over,
/// where the execution continues, and what it puts out on the way there.
const CATCHER_FIELDS: &[&str] = &[
    "Assign",
    "Comment",
    "ErrorEquals",
    "Next",
    "Output",
    "ResultPath",
];

/// The fields a catcher must carry, in the order the refusal lists them: the errors it
/// takes over, and the state the execution continues at.
const CATCHER_REQUIRED_FIELDS: &[&str] = &["ErrorEquals", "Next"];

/// What a schema diagnostic says about an `ErrorEquals` list naming [`ALL_ERRORS`]
/// beside other error names.
///
/// RECORDED text. The language admits the catch-all only as the single entry of a
/// retrier's or a catcher's `ErrorEquals`, since a list that holds it matches every
/// error already and the names beside it can never decide anything.
pub const ALL_ERRORS_ALONE_MESSAGE: &str = "States.ALL must appear alone and at end of list";

/// What a schema diagnostic says about a member the language declares as a LIST that
/// a definition carries as something else: the member, and what it holds.
///
/// The sentence is this service's own, as every diagnostic about the SHAPE of a value
/// is: what is recorded is the wording for a field the language does not declare at
/// all ([`field_not_supported_message`]) and for the fields an object owes
/// ([`required_fields_message`]); a value of the wrong shape is stated as the rule it
/// broke, in the words this module states every other such rule in.
pub fn array_member_message(member: &str) -> String {
    format!("'{}' must be an array", echoed(member))
}

/// What a schema diagnostic says about an entry of such a list that is not the object
/// the language declares there.
pub fn entry_object_message(member: &str) -> String {
    format!("every entry of '{}' must be a JSON object", echoed(member))
}

/// What a schema diagnostic says about an `ErrorEquals` that names nothing: the list
/// decides which errors the entry takes over, so an empty one takes over none and the
/// entry can never do anything.
pub const EMPTY_ERROR_EQUALS_MESSAGE: &str = "'ErrorEquals' must name at least one error";

/// What a schema diagnostic says about an entry of an `ErrorEquals` that is not an
/// error NAME.
pub const ERROR_NAME_MESSAGE: &str = "'ErrorEquals' must name errors as strings";

/// What the diagnostic for a state machine that cannot stop says.
///
/// RECORDED text. It names no state and no member: what is missing is a terminal
/// state anywhere in the machine, so there is no position in the document to place it
/// at.
pub const MISSING_END_STATE_MESSAGE: &str = "Workflow has no terminal state";

/// What a diagnostic says about a variable a field reads and nothing reaching that
/// field assigns: the variable, quoted.
///
/// RECORDED text. The name is the caller's own, so it is clipped by [`echoed`].
pub fn possibly_undefined_variable_message(name: &str) -> String {
    format!("Variable '{}' is possibly not defined.", echoed(name))
}

/// Everything ONE walk of a definition found: the diagnostics it reports, and
/// whether it had to drop any of them.
///
/// The walk collects at most [`MAX_DIAGNOSTICS`] of each severity, so a caller is
/// told that more exist (`truncated`) rather than being handed a list that grows
/// with the definition.
#[derive(Debug, Clone)]
pub struct Diagnosis {
    /// What was found, in the order it was found.
    pub diagnostics: Vec<Diagnostic>,
    /// Whether an ERROR was found and not reported, the error budget being spent.
    pub errors_dropped: bool,
    /// Whether a WARNING was found and not reported, that budget being spent.
    pub warnings_dropped: bool,
    /// How many of each severity have been reported, which is what spends each
    /// budget.
    errors: usize,
    warnings: usize,
    /// Whether this walk reports no WARNING at all — what reads it ([`validate`],
    /// [`validate_single_state`]) refuses a definition for its ERRORS, so a warning
    /// would be built only to be discarded.
    errors_only: bool,
    /// Whether this walk reads the FIRST error and nothing else, so there is no reason
    /// to keep walking past that error.
    first_error_only: bool,
    /// What is LEFT of this diagnosis's variable-scope budget
    /// ([`MAX_SCOPE_VISITS`]), spent by every state that analysis visits, in this
    /// definition and in every machine nested in it.
    scope_visits: usize,
}

/// A walk with every budget of it unspent.
impl Default for Diagnosis {
    fn default() -> Self {
        Self {
            diagnostics: Vec::new(),
            errors_dropped: false,
            warnings_dropped: false,
            errors: 0,
            warnings: 0,
            errors_only: false,
            first_error_only: false,
            scope_visits: MAX_SCOPE_VISITS,
        }
    }
}

impl Diagnosis {
    /// A walk that stops at the first ERROR, for [`validate_single_state`]: no warning
    /// is built and nothing past that error is looked at. What it reports about a
    /// definition is the same verdict a full walk reports, because an error is an error
    /// wherever in the definition it was found.
    pub(super) fn until_first_error() -> Self {
        Self {
            errors_only: true,
            first_error_only: true,
            ..Self::default()
        }
    }

    /// A walk that reports every ERROR and no warning, for [`validate`]: the refusal it
    /// words states one subject per error the definition carries, and a warning is not
    /// one of them.
    pub(super) fn errors_only() -> Self {
        Self {
            errors_only: true,
            ..Self::default()
        }
    }

    /// Whether the definition carries a problem that prevents creating or updating
    /// a state machine with it — what `result` reports as `FAIL`.
    pub fn failed(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }

    /// Whether a diagnostic of a severity this request reports was dropped, so the
    /// reported list is short of what the definition carries.
    pub fn dropped(&self, severity: &str) -> bool {
        self.errors_dropped
            || (severity == crate::state::SEVERITY_WARNING && self.warnings_dropped)
    }

    /// Record one diagnostic, or that its severity's budget is spent.
    pub(super) fn push(&mut self, found: Diagnostic) {
        let (reported, dropped) = if found.is_error() {
            (&mut self.errors, &mut self.errors_dropped)
        } else {
            (&mut self.warnings, &mut self.warnings_dropped)
        };
        if *reported >= MAX_DIAGNOSTICS || (self.errors_only && !found.is_error()) {
            *dropped = true;
            return;
        }
        *reported += 1;
        self.diagnostics.push(found);
    }

    /// Whether this walk reports WARNINGS at all: the lints that cost a walk of their
    /// own are built only where one can be reported.
    pub(super) fn warnings_wanted(&self) -> bool {
        !self.errors_only
    }

    /// Spend `visits` states of the variable-scope budget, or report that what is left
    /// does not cover them.
    ///
    /// The budget is the ONE this diagnosis has for every machine it walks, so the
    /// analysis's cost cannot be the product of the machines a definition declares and
    /// the states each of them holds.
    pub(super) fn spend_scope_visits(&mut self, visits: usize) -> bool {
        match self.scope_visits.checked_sub(visits) {
            Some(left) => {
                self.scope_visits = left;
                true
            }
            None => false,
        }
    }

    /// Whether the walk keeps looking: it stops once it can report no further ERROR
    /// — its error budget spent, or, for a [`validate_single_state`], the one error it
    /// reads already found. Every error past that point would change neither the result
    /// nor the reported list.
    pub(super) fn looking(&self) -> bool {
        !(self.errors_dropped || (self.first_error_only && self.errors > 0))
    }
}

/// What separates the subjects of an `InvalidDefinition` message: a definition
/// carrying several problems is refused with one subject per problem, written one
/// after another in the order the walk found them.
pub(super) const SUBJECT_SEPARATOR: &str = ", ";

/// Why `definition` is not a definition this service accepts, or `Ok` when it is.
///
/// These are the Amazon States Language's own structural rules, so the refusal is
/// `CreateStateMachine`'s modelled `InvalidDefinition`: the definition is a JSON
/// object carrying `StartAt` and a non-empty `States` map, `StartAt` names a declared
/// state, every state is an object with a string `Type`, every `Next` names a
/// declared state, a terminal state carries no `Next`, and a continuing state carries
/// `Next` or `"End": true`.
///
/// The `Err` is the SUBJECT of that error's message, which the operation states
/// through [`crate::state::resource_error_message`] with
/// [`crate::state::INVALID_STATE_MACHINE_DEFINITION`]: ONE subject per error
/// [`diagnose`] found, each `<code>: <detail> at <location>` ([`Diagnostic::stated`]),
/// separated by [`SUBJECT_SEPARATOR`]. Every error the definition carries is stated,
/// so a caller fixing a definition is told all of them rather than one per attempt,
/// and what a create refuses a definition for cannot disagree with what
/// `ValidateStateMachineDefinition` reports `FAIL` for.
///
/// The message is bounded by the walk itself rather than by the definition: the walk
/// reports at most [`MAX_DIAGNOSTICS`] errors, each one's quoted value is capped by
/// [`MAX_ECHOED_CHARS`] and its location by [`MAX_LOCATION_CHARS`], so a caller-sized
/// definition cannot be amplified into a larger refusal.
///
/// A definition may use states this emulator does not EXECUTE and still be valid,
/// because it is valid ASL: what cannot be run surfaces at execution time, as a
/// `FAILED` execution, not as a rejected create.
pub fn validate(definition: &str) -> Result<(), String> {
    let stated: Vec<String> = collect(definition, Diagnosis::errors_only())
        .diagnostics
        .iter()
        .filter(|found| found.is_error())
        .map(Diagnostic::stated)
        .collect();
    if stated.is_empty() {
        return Ok(());
    }
    Err(stated.join(SUBJECT_SEPARATOR))
}

/// Every problem `definition` carries: the errors that would prevent creating or
/// updating a state machine with it, and the warnings that would not.
///
/// This is what `ValidateStateMachineDefinition` reports — "Validation will look
/// for problems in your state machine definition and return a result and a list of
/// diagnostic elements" — and, through [`validate`], what `CreateStateMachine` and
/// `UpdateStateMachine` refuse a definition for.
///
/// The rules are applied to EVERY state list the definition declares, not only the
/// top-level one: a `Map` state's `ItemProcessor`/`Iterator` and a `Parallel` state's
/// `Branches` each declare their own state machine, whose state names reach the same
/// execution history and whose states inherit the enclosing query language.
///
/// The list is bounded by [`MAX_DIAGNOSTICS`] per severity; a definition carrying
/// more problems than that is reported as truncated rather than reported in full.
///
/// The walk costs the same ORDER as the parse that precedes it: it visits each
/// member once, every path it builds is bounded by [`MAX_LOCATION_CHARS`] and every
/// value it quotes by [`MAX_ECHOED_CHARS`], so no cost that is the product of the
/// state count and the nesting can be built from a caller's definition.
pub fn diagnose(definition: &str) -> Diagnosis {
    collect(definition, Diagnosis::default())
}

/// [`diagnose`] into the walk `found` describes: every problem, only the errors — or,
/// for [`super::validate_single_state`], the first error and nothing else.
pub(super) fn collect(definition: &str, mut found: Diagnosis) -> Diagnosis {
    if definition.trim().is_empty() {
        found.push(error(
            MISSING_DESCRIPTION,
            None,
            "the request carried no definition to validate",
        ));
        return found;
    }
    match serde_json::from_str::<Value>(definition) {
        Ok(document) => diagnose_machine(
            &document,
            "",
            JSONPATH,
            1,
            false,
            Outer::Nothing,
            &mut found,
        ),
        // The diagnostic is the service's own, naming where the definition stopped
        // parsing, the mark it left unmatched and what was expected there — the one
        // construction every document this service reads is reported through — and
        // it is LOCATED at the position in the definition the parse ended at, which
        // is where a client has to look.
        Err(why) => found.push(error(
            INVALID_JSON_DESCRIPTION,
            Some(crate::state::json_parse_definition_location(definition, &why)),
            crate::state::json_parse_definition_diagnostic(definition, &why),
        )),
    }
    found
}

/// [`diagnose`] for one state machine — the definition itself, or one nested inside
/// it — carrying where it was found (`prefix`), the query language it inherits from
/// the state that declares it, how deeply it is nested, and whether it is a `Map`
/// state's PROCESSOR, which admits one field more than a definition or a `Parallel`
/// branch does ([`machine_fields`]), and what was already in scope where it is
/// declared ([`Outer`]).
pub(super) fn diagnose_machine(
    document: &Value,
    prefix: &str,
    inherited: &str,
    depth: usize,
    processor: bool,
    outer: Outer<'_>,
    into: &mut Diagnosis,
) {
    if !into.looking() {
        return;
    }
    if depth > MAX_DEFINITION_DEPTH {
        into.push(error(
            TOO_DEEPLY_NESTED,
            machine_at(prefix),
            format!(
                "the definition nests state machines more than the {MAX_DEFINITION_DEPTH} levels \
                 this service reads"
            ),
        ));
        return;
    }
    let Some(document) = document.as_object() else {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            machine_at(prefix),
            "a state machine definition must be a JSON object",
        ));
        return;
    };
    // `QueryLanguage` selects which members the states below are written with, so
    // it is read before them; a definition that names neither language is written
    // in the one `QueryLanguage` defaults to.
    let language = match declared_language(document.get("QueryLanguage")) {
        Ok(declared) => declared.unwrap_or(inherited),
        Err(why) => {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(prefix, "/QueryLanguage"),
                why,
            ));
            inherited
        }
    };
    // The language's schema for a state machine object is CLOSED: it declares the
    // fields a definition may carry ([`MACHINE_FIELDS`]) and the two it must
    // ([`MACHINE_REQUIRED_FIELDS`]). Fields the object does not carry are reported
    // TOGETHER, in one diagnostic that lists them, and each field the schema does
    // not declare is reported on its own — both located at the OBJECT, which is
    // what a field belongs to, rather than at a position the document has not got.
    let object = machine_at(prefix);
    let missing: Vec<&str> = MACHINE_REQUIRED_FIELDS
        .iter()
        .copied()
        .filter(|field| !document.contains_key(*field))
        .collect();
    if !missing.is_empty() {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            object.clone(),
            required_fields_message(&missing),
        ));
    }
    for field in document.keys() {
        if !machine_fields(processor).contains(&field.as_str()) {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                object.clone(),
                field_not_supported_message(field),
            ));
        }
    }
    let states = document.get("States").and_then(Value::as_object);
    // A field the object DOES carry is judged on its own: that it is absent is
    // already stated above, so what is left to say about it is its value.
    let mut start_at: Option<&str> = None;
    if let Some(declared) = document.get("StartAt") {
        match declared.as_str() {
            Some(declared) => {
                start_at = Some(declared);
                if states.is_some_and(|states| !states.contains_key(declared)) {
                    into.push(error(
                        MISSING_TRANSITION_TARGET,
                        at(prefix, "/StartAt"),
                        missing_target_message(declared),
                    ));
                }
            }
            None => into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(prefix, "/StartAt"),
                "a state machine definition must carry a string 'StartAt' member",
            )),
        }
    }
    let Some(states) = states else {
        if document.contains_key("States") {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(prefix, "/States"),
                "a state machine definition must carry an object 'States' member",
            ));
        }
        return;
    };
    if states.is_empty() {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(prefix, "/States"),
            "'States' must declare at least one state",
        ));
        return;
    }
    // What the definition's transitions NAME: the state its `StartAt` names, and
    // every state a transition of a declared state names. Read once per machine,
    // before any state is judged, because it is a property of the whole graph rather
    // than of one state.
    let named = named_states(states, start_at);
    // Which variables each state of THIS machine can count on having been assigned
    // by the time it runs, which is what its own reads are judged against. Built
    // once per machine, before any state is judged, because it is a property of the
    // whole graph rather than of one state; only a walk that reports WARNINGS needs
    // it ([`POSSIBLY_UNDEFINED_VARIABLE`] is one).
    let scopes = if into.warnings_wanted() {
        Scopes::of(states, start_at, outer, into)
    } else {
        Scopes::none()
    };
    // Whether the machine declares a state that ENDS an execution, read before the
    // states are judged and reported after them: the diagnostic is about the machine,
    // so it follows the ones about its states.
    let terminal = states.values().any(ends_an_execution);
    for (name, state) in states {
        if !into.looking() {
            break;
        }
        diagnose_state(
            name,
            state,
            &Machine {
                states,
                language,
                prefix,
                depth,
            },
            &scopes,
            into,
        );
    }
    // The transition graph is also read the other way round: a state nothing in the
    // definition names is work this state machine can never do, so it is reported
    // under the code the transition graph's problems are reported under, at the state
    // itself — what is wrong is not a member the state carries but that nothing leads
    // to it. These follow the diagnostics about the states, because they are about
    // the graph those states form.
    //
    // A machine that declares no entry point at all has no graph to judge this way:
    // that it carries no `StartAt` is already reported above, and every state of it
    // would be unreached for that one reason.
    for name in states.keys() {
        if !into.looking() {
            break;
        }
        if start_at.is_none() || named.contains(name.as_str()) {
            continue;
        }
        into.push(error(
            MISSING_TRANSITION_TARGET,
            at(
                &nested_at(prefix, format_args!("/States/{}", echoed(name))),
                "",
            ),
            unreachable_state_message(name),
        ));
    }
    // "MISSING_END_STATE: The workflow does not have a terminal state." A state
    // machine is a graph that must be able to STOP, so at least one of the states it
    // declares ends an execution; one that declares none describes a workflow no run
    // of it can leave. It is read BEFORE the states are judged and reported AFTER
    // them, so it follows the diagnostics about them; whether there is room left to
    // report it is `push`'s own answer, as it is for every other diagnostic.
    if !terminal {
        into.push(error(
            MISSING_END_STATE,
            at(prefix, ""),
            MISSING_END_STATE_MESSAGE,
        ));
    }
}

/// What every state of ONE state machine is judged in: the state list its transition
/// targets resolve against, the query language it inherits from the machine, where
/// that machine was found and how deeply it is nested.
struct Machine<'a> {
    states: &'a Map<String, Value>,
    language: &'a str,
    prefix: &'a str,
    depth: usize,
}

/// The states of `states` that something NAMES: the one `start_at` names, and every
/// one a transition of a declared state names ([`transitions`]).
///
/// A state outside this set is one nothing in the definition leads to, which is what
/// [`unreachable_state_message`] reports. A transition naming a state the machine does
/// not declare names nothing here and is reported on its own
/// ([`MISSING_TRANSITION_TARGET`]); an entry point that names no declared state leaves
/// the machine with nothing named but what its states name.
///
/// Each state's transitions are read once, so this costs one pass over the machine
/// whatever shape its graph has.
fn named_states<'a>(
    states: &'a Map<String, Value>,
    start_at: Option<&str>,
) -> BTreeSet<&'a str> {
    // The name is taken from the `States` map rather than from what named it, so the
    // set holds the machine's own text once however many transitions name a state.
    let declared = |name: &str| {
        states
            .get_key_value(name)
            .map(|(declared, _)| declared.as_str())
    };
    let mut named: BTreeSet<&str> = start_at.and_then(declared).into_iter().collect();
    for state in states.values() {
        let Some(state) = state.as_object() else {
            continue;
        };
        for transition in transitions(state) {
            named.extend(declared(transition.target.as_str()));
        }
    }
    named
}

/// Whether `state` ENDS an execution: a state of a type that terminates one
/// ([`TERMINAL_STATE_TYPES`]), or any state declaring `"End": true`.
///
/// This is what [`MISSING_END_STATE`] is read off, over every state a machine
/// DECLARES: what that diagnostic reports is that the workflow has no terminal state,
/// and a state machine that declares one has one — whether anything this definition
/// declares LEADS to it is a different question, and the one [`named_states`] answers.
fn ends_an_execution(state: &Value) -> bool {
    let Some(state) = state.as_object() else {
        return false;
    };
    if state.get("End").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    state
        .get("Type")
        .and_then(Value::as_str)
        .is_some_and(|state_type| TERMINAL_STATE_TYPES.contains(&state_type))
}

/// [`diagnose_machine`] for ONE state of a state machine: the rules the state's own
/// members must satisfy, then the state machines nested inside it.
///
/// `scopes` is its machine's variable-scope analysis, which answers what every run
/// reaching this state has already assigned — what its own reads are judged against.
fn diagnose_state<'a>(
    name: &'a str,
    state: &Value,
    machine: &Machine<'_>,
    scopes: &'a Scopes<'a>,
    into: &mut Diagnosis,
) {
    let Machine {
        states,
        language: inherited,
        prefix,
        depth,
    } = *machine;
    // The name the state is DECLARED under, which is what its machine's analysis
    // answers for; the one quoted below is the clipped one.
    let declared = name;
    // A state name is a caller-chosen JSON object key and lands in every history
    // event of that state, so its LENGTH is bounded here, not only the number of
    // events: this is what keeps a history's bytes bounded. The name a message
    // quotes, and the one the location carries, is the clipped one, for the reason
    // [`echoed`] and [`nested_at`] record; the length checked and reported is the
    // DECLARED one.
    let declared_chars = name.chars().count();
    let shown = echoed(name);
    let name = shown.as_str();
    let here = nested_at(prefix, format_args!("/States/{name}"));
    if declared_chars > MAX_STATE_NAME_CHARS {
        into.push(error(
            INVALID_STATE_NAME,
            at(&here, ""),
            format!(
                "a state name may carry at most {MAX_STATE_NAME_CHARS} characters, and this \
                 definition declares one of {declared_chars}"
            ),
        ));
    }
    let Some(state) = state.as_object() else {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(&here, ""),
            format!("state '{name}' must be a JSON object"),
        ));
        return;
    };
    let Some(state_type) = state.get("Type").and_then(Value::as_str) else {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(&here, "/Type"),
            format!("state '{name}' must carry a string 'Type' member"),
        ));
        return;
    };
    // `Type` selects one of the state types the Amazon States Language DEFINES
    // ([`STATE_TYPES`]), so a state naming any other one describes no state at all:
    // nothing says what it reads, what it writes or where it goes next, which is why
    // the walk stops here rather than judging the state's other members under rules
    // that are keyed on its type.
    if !STATE_TYPES.contains(&state_type) {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(&here, "/Type"),
            format!(
                "state '{name}' is of type '{}', which is not one of the state types the \
                 Amazon States Language defines ({})",
                echoed(state_type),
                STATE_TYPES.join(", ")
            ),
        ));
        return;
    }
    // The language's schema for a state is CLOSED per TYPE, exactly as a state
    // machine object's is: the type selects what the state does, and the fields
    // beside it say how ([`state_fields`]), so each field outside that set is
    // reported on its own, located at the STATE — the field belongs to the state
    // rather than to a position under it.
    for field in state.keys() {
        if !state_fields(state_type).contains(&field.as_str()) {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(&here, ""),
                field_not_supported_message(field),
            ));
        }
    }
    // What the state does with a failure of its own work, for a type that declares
    // it.
    diagnose_error_handlers(state, state_type, &here, into);
    // A `Fail` state's `Error` and `Cause` are reported verbatim by
    // `DescribeExecution`, through shapes with their own maxima, so a
    // definition carrying a longer one could never be reported.
    for (member, cap) in [("Error", MAX_ERROR_BYTES), ("Cause", MAX_CAUSE_BYTES)] {
        if let Some(value) = state.get(member).and_then(Value::as_str) {
            if value.len() > cap {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&here, &format!("/{member}")),
                    format!(
                        "state '{name}' carries a '{member}' of {} bytes, more than the {cap} \
                         this service reports",
                        value.len()
                    ),
                ));
            }
        }
    }
    // Every member the state names a transition through — its own `Next`, the
    // `Default` of a `Choice`, each `Choice` rule's own `Next` and each `Catch`
    // catcher's `Next` — resolves by ONE rule: the name must be one the enclosing
    // `States` map declares. Each is reported at the member that names it.
    for transition in transitions(state) {
        if !states.contains_key(transition.target.as_str()) {
            into.push(error(
                MISSING_TRANSITION_TARGET,
                at(&here, &transition.path),
                missing_target_message(&transition.target),
            ));
        }
    }
    match state.get("Next") {
        Some(next) => match next.as_str() {
            Some(_) => {
                if TERMINAL_STATE_TYPES.contains(&state_type) {
                    into.push(error(
                        SCHEMA_VALIDATION_FAILED,
                        at(&here, "/Next"),
                        format!(
                            "state '{name}' is a '{}' state, which ends an execution \
                             and must not carry 'Next'",
                            echoed(state_type)
                        ),
                    ));
                }
            }
            None => into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(&here, "/Next"),
                format!("state '{name}' has a non-string 'Next' member"),
            )),
        },
        None => {
            if CONTINUING_STATE_TYPES.contains(&state_type)
                && state.get("End").and_then(Value::as_bool) != Some(true)
            {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&here, ""),
                    format!(
                        "state '{name}' is a '{}' state, so it must carry either \
                         'Next' or \"End\": true",
                        echoed(state_type)
                    ),
                ));
            }
        }
    }
    // A state may be written in a query language of its own — but only in the
    // direction the service allows: a JSONPath state machine may hold a JSONata
    // state, a JSONata one may not hold a JSONPath state.
    let language = match declared_language(state.get("QueryLanguage")) {
        Ok(Some(declared)) => {
            if declared == JSONPATH && inherited == JSONATA {
                // The service blames the overriding STATE rather than the member it
                // declares the override in: the state is the thing a state machine of
                // this query language may not hold. Which state that is, is reported
                // by `location` alone — the sentence is about the member and is the
                // same for every state that carries it.
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&here, ""),
                    format!(
                        "'QueryLanguage' can not be '{JSONPATH}' if set to '{JSONATA}' \
                         for whole state machine"
                    ),
                ));
            }
            declared
        }
        Ok(None) => inherited,
        Err(why) => {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(&here, "/QueryLanguage"),
                why,
            ));
            inherited
        }
    };
    if language == JSONATA {
        diagnose_jsonata_state(name, state, state_type, &here, into);
    } else {
        diagnose_jsonpath_state(name, state, state_type, &here, into);
    }
    // A variable the state ASSIGNS is in scope for the state's own expressions,
    // because the language evaluates a state's assignments before the output they
    // feed; everything else it may read is what reaching this state already assigned.
    let own = assigned_variables(state);
    if into.warnings_wanted() {
        // POSSIBLY_UNDEFINED_VARIABLE: one diagnostic per FIELD that reads a
        // variable nothing reaching that field assigns, which is the granularity
        // `location` documents (`/States/<StateName>/<FieldName>`).
        for (field, read) in variable_reads(state) {
            if own.contains(&read) || scopes.holds(declared, &read) {
                continue;
            }
            into.push(warning(
                POSSIBLY_UNDEFINED_VARIABLE,
                at(&here, &field),
                possibly_undefined_variable_message(&read),
            ));
        }
    }
    // What the state machines nested in this state inherit: what reaching this state
    // had in scope, asked of its machine rather than copied into each of them.
    //
    // NOT this state's own `Assign`: only a `Map` and a `Parallel` declare a machine,
    // and their own assignments are evaluated when the work those machines DO has
    // completed, so a state inside one cannot count on them.
    let outer = Outer::State {
        machine: scopes,
        state: declared,
    };
    // A nested state machine's names reach the same history as this one's, so every
    // rule above holds for it too, and its states inherit this state's language — and
    // the variables this state has in scope, which the states it declares read.
    for member in ["ItemProcessor", "Iterator"] {
        if let Some(processor) = state.get(member) {
            diagnose_machine(
                processor,
                &nested_at(&here, format_args!("/{member}")),
                language,
                depth + 1,
                // A processor is the one position the language declares
                // `ProcessorConfig` at.
                true,
                outer,
                into,
            );
        }
    }
    if let Some(branches) = state.get("Branches").and_then(Value::as_array) {
        for (index, branch) in branches.iter().enumerate() {
            diagnose_machine(
                branch,
                &element_at(&here, "Branches", index),
                language,
                depth + 1,
                false,
                outer,
                into,
            );
        }
    }
}

/// The `Retry` and `Catch` rules of one state: what the language requires of the
/// retriers and catchers a state of this type declares.
///
/// A member the type does not declare at all is not judged here: that it is there is
/// already reported ([`state_fields`]), and what it holds says nothing more about a
/// definition whose state cannot carry it.
fn diagnose_error_handlers(
    state: &Map<String, Value>,
    state_type: &str,
    here: &str,
    into: &mut Diagnosis,
) {
    for member in [RETRY_MEMBER, CATCH_MEMBER] {
        if !state_fields(state_type).contains(&member) {
            continue;
        }
        let (fields, required) = if member == CATCH_MEMBER {
            (CATCHER_FIELDS, CATCHER_REQUIRED_FIELDS)
        } else {
            (RETRIER_FIELDS, RETRIER_REQUIRED_FIELDS)
        };
        let Some(declared) = state.get(member) else {
            continue;
        };
        // The member holds a LIST of entries, so a value that is not one carries no
        // entry to judge: it is reported as the member it is instead.
        let Some(entries) = declared.as_array() else {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(here, &format!("/{member}")),
                array_member_message(member),
            ));
            continue;
        };
        for (index, entry) in entries.iter().enumerate() {
            let at_entry = element_at(here, member, index);
            let Some(entry) = entry.as_object() else {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&at_entry, ""),
                    entry_object_message(member),
                ));
                continue;
            };
            // An entry of either array is an object the language declares a closed set
            // of fields on, reported exactly as a state machine object's and a state's
            // are: the fields it owes together, each field outside the set on its own,
            // both located at the ENTRY the fields belong to.
            let missing: Vec<&str> = required
                .iter()
                .copied()
                .filter(|field| !entry.contains_key(*field))
                .collect();
            if !missing.is_empty() {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&at_entry, ""),
                    required_fields_message(&missing),
                ));
            }
            for field in entry.keys() {
                if !fields.contains(&field.as_str()) {
                    into.push(error(
                        SCHEMA_VALIDATION_FAILED,
                        at(&at_entry, ""),
                        field_not_supported_message(field),
                    ));
                }
            }
            // `ErrorEquals` names the errors the entry takes over, so what it holds is
            // a non-empty list of error NAMES: a list that is neither is reported at
            // the member, and one name that is not a string at that name's own
            // position.
            let Some(declared) = entry.get(ERROR_EQUALS_MEMBER) else {
                continue;
            };
            let array = nested_at(&at_entry, format_args!("/{ERROR_EQUALS_MEMBER}"));
            let Some(names) = declared.as_array() else {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&array, ""),
                    array_member_message(ERROR_EQUALS_MEMBER),
                ));
                continue;
            };
            if names.is_empty() {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&array, ""),
                    EMPTY_ERROR_EQUALS_MESSAGE,
                ));
            }
            for (position, name) in names.iter().enumerate() {
                if !name.is_string() {
                    into.push(error(
                        SCHEMA_VALIDATION_FAILED,
                        at(&indexed_at(&array, position), ""),
                        ERROR_NAME_MESSAGE,
                    ));
                }
            }
            // `States.ALL` matches every error there is, so the language admits it
            // only as the ONE entry of the list it is named in, and only in the LAST
            // entry of the array that holds that list: a name beside it, and an entry
            // after it, can never decide anything. Each entry that names it outside
            // those two positions is reported at the name itself, which is what the
            // caller has to move.
            if names.len() < 2 && index + 1 == entries.len() {
                continue;
            }
            for (position, name) in names.iter().enumerate() {
                if name.as_str() == Some(ALL_ERRORS) {
                    into.push(error(
                        SCHEMA_VALIDATION_FAILED,
                        at(&indexed_at(&array, position), ""),
                        ALL_ERRORS_ALONE_MESSAGE,
                    ));
                }
            }
        }
    }
}

/// The most distinct variables one state machine's scope analysis follows through its
/// state graph ([`Scopes`]).
///
/// Following one variable costs one walk of the graph and a definition is
/// caller-sized, so the number followed is bounded like every other dimension of this
/// walk, and the bound is the one the reported list already carries
/// ([`DEFAULT_MAX_DIAGNOSTICS`]). A variable NO state assigns needs no walk at all and
/// is not counted against it: it is out of scope wherever it is read. A variable past
/// the bound is taken to be in scope, so the bound can only leave a warning unsaid,
/// never invent one.
///
/// It is also what fits one state's scope in a single [`ScopeMask`], which is what
/// keeps the analysis from holding the PRODUCT of the state count and the variable
/// count.
const MAX_SCOPED_VARIABLES: usize = DEFAULT_MAX_DIAGNOSTICS;

/// The most states the variable-scope analysis visits across ONE diagnosis.
///
/// Following one variable costs one walk of its machine's graph, so the analysis's
/// cost is the PRODUCT of the variables it follows and the states it walks, and a
/// definition chooses both — as it chooses how many machines it nests. The product is
/// therefore spent from ONE budget for the whole diagnosis
/// ([`Diagnosis::spend_scope_visits`]) rather than bounded per machine: the budget is
/// the diagnostics one call reports ([`DEFAULT_MAX_DIAGNOSTICS`]) times the
/// transitions one execution may make ([`MAX_STATE_TRANSITIONS`]), which is the
/// largest state graph this service walks at all. Past it no further variable is
/// followed, and a variable that was not followed is taken to be in scope, so a spent
/// budget can only leave a warning unsaid.
const MAX_SCOPE_VISITS: usize = DEFAULT_MAX_DIAGNOSTICS * MAX_STATE_TRANSITIONS;

/// Which of the variables one machine's analysis followed a state can count on: one
/// BIT each, at the index [`Scopes::assigned`] records for the variable.
///
/// A set of NAMES per state would cost the product of the state count and the variable
/// count, and a definition chooses both, so a state's scope is a mask: the analysis
/// holds one of these per state and nothing that grows with the names.
type ScopeMask = u128;

/// [`MAX_SCOPED_VARIABLES`] variables must fit the mask that carries them.
const _: () = assert!(MAX_SCOPED_VARIABLES <= ScopeMask::BITS as usize);

/// The member a state assigns variables in: "Assign".
pub(super) const ASSIGN_MEMBER: &str = "Assign";

/// The member a JSONata state's own output is built in: "Output".
pub(super) const OUTPUT_MEMBER: &str = "Output";

/// The member a JSONata `Task`, `Map` or `Parallel` state builds the document it hands
/// its work in: "Arguments".
pub(super) const ARGUMENTS_MEMBER: &str = "Arguments";

/// The member a `Map` state projects each of its items through: "ItemSelector".
pub(super) const ITEM_SELECTOR_MEMBER: &str = "ItemSelector";

/// What was already in scope where a state machine is DECLARED, which its states
/// inherit.
///
/// A QUESTION asked up the chain of enclosing states, never a set copied into each
/// level: a copy per level would hold the caller's variable names once per nesting
/// level, and the nesting is the caller's too.
#[derive(Clone, Copy)]
pub(super) enum Outer<'a> {
    /// The definition itself: a run has assigned nothing where it starts.
    Nothing,
    /// A machine one state declares: what reaching that state had in scope, which is
    /// its own machine's answer for it.
    State {
        machine: &'a Scopes<'a>,
        state: &'a str,
    },
}

impl Outer<'_> {
    /// Whether the enclosing scope holds `name`.
    fn holds(&self, name: &str) -> bool {
        match self {
            Outer::Nothing => false,
            Outer::State { machine, state } => machine.holds(state, name),
        }
    }
}

/// Which variables each state of ONE state machine can count on having been assigned
/// by the time it runs: those EVERY path from the machine's entry point to that state
/// assigns, together with the ones inherited from the state that declares the machine.
///
/// A variable is in a state's scope only when no run can reach that state without
/// having assigned it — which is what makes the ones outside it "possibly" undefined:
/// the read resolves on some runs and not on others. A state no run reaches has
/// nothing but the inherited variables in scope.
pub(super) struct Scopes<'a> {
    /// Every variable some state of this machine assigns, and which bit of a state's
    /// [`ScopeMask`] answers for it. A variable the analysis did NOT follow — the
    /// enclosing scope already holds it, or the bound or the budget is spent — carries
    /// no bit and is taken to be in scope.
    assigned: BTreeMap<String, Option<usize>>,
    /// Per state, which of the followed variables every run reaching it has assigned.
    masks: BTreeMap<&'a str, ScopeMask>,
    /// What this machine is declared inside.
    outer: Outer<'a>,
}

impl<'a> Scopes<'a> {
    /// A machine whose variables are not judged at all: what a walk that reports no
    /// WARNING needs, since the lint this feeds is one.
    fn none() -> Self {
        Self {
            assigned: BTreeMap::new(),
            masks: BTreeMap::new(),
            outer: Outer::Nothing,
        }
    }

    /// Follow each variable `states` assigns through the transition graph from
    /// `start_at`, with `outer` already in scope.
    ///
    /// Each variable is followed with ONE walk of the graph, from the entry point
    /// along the transitions that do NOT assign it, and each walk is spent from the
    /// diagnosis's own [`MAX_SCOPE_VISITS`] budget, so the analysis holds one walk's
    /// states at a time and costs the same whatever the definition declares.
    fn of(
        states: &'a Map<String, Value>,
        start_at: Option<&str>,
        outer: Outer<'a>,
        into: &mut Diagnosis,
    ) -> Self {
        // Which assignment block runs on the way along each transition: a state's own
        // when its work completed, and a `Catch` catcher's when it did not. The blocks
        // are held ONCE and the transitions name them by index, so a state with many
        // transitions does not hold its block once per transition.
        let mut blocks: Vec<BTreeSet<String>> = Vec::new();
        let mut graph: BTreeMap<&str, Vec<ScopeEdge>> = BTreeMap::new();
        for (name, state) in states {
            let mut out = Vec::new();
            if let Some(state) = state.as_object() {
                let Assignments { own, caught } = assignments(state);
                let own_block = blocks.len();
                blocks.push(own);
                let caught_blocks: Vec<usize> = caught
                    .into_iter()
                    .map(|block| {
                        blocks.push(block);
                        blocks.len() - 1
                    })
                    .collect();
                for transition in transitions(state) {
                    if !states.contains_key(transition.target.as_str()) {
                        continue;
                    }
                    let assigns = transition
                        .caught
                        .and_then(|index| caught_blocks.get(index).copied())
                        .unwrap_or(own_block);
                    out.push(ScopeEdge {
                        target: transition.target,
                        assigns,
                    });
                }
            }
            graph.insert(name.as_str(), out);
        }
        // Each walk below visits each state at most once, so one walk costs at most
        // this many visits of the budget.
        let per_walk = states.len();
        let mut scopes = Self {
            assigned: BTreeMap::new(),
            masks: states.keys().map(|name| (name.as_str(), 0)).collect(),
            outer,
        };
        // Every variable this machine assigns somewhere. One set per MACHINE, so what
        // this holds is the machine's own text rather than that text once per state.
        let declared: BTreeSet<&String> = blocks.iter().flatten().collect();
        if !into.spend_scope_visits(per_walk) {
            // Nothing can be followed, so nothing this machine assigns is judged.
            scopes.assigned = declared.into_iter().map(|name| (name.clone(), None)).collect();
            return scopes;
        }
        // A machine whose entry point names no declared state has no run to reason
        // about, so no state of it reaches a read with anything assigned.
        let entry = start_at.filter(|start_at| states.contains_key(*start_at));
        // Where a run can be at all: the states a read of a variable is judged in.
        let reachable = reached_from(entry, &graph, &blocks, None);
        let mut followed = 0_usize;
        for name in declared {
            // A variable the enclosing scope already holds is in scope at every state
            // of this machine whatever this machine does with it, and one the bound or
            // the budget cannot reach is taken to be, so neither is followed.
            if followed >= MAX_SCOPED_VARIABLES
                || scopes.outer.holds(name)
                || !into.spend_scope_visits(per_walk)
            {
                scopes.assigned.insert(name.clone(), None);
                continue;
            }
            let index = followed;
            followed += 1;
            // Where the variable is NOT yet assigned: every state a run can reach
            // without having taken a transition that assigns it.
            let unassigned = reached_from(entry, &graph, &blocks, Some(name));
            for (state, mask) in &mut scopes.masks {
                if reachable.contains(state) && !unassigned.contains(state) {
                    *mask |= 1 << index;
                }
            }
            scopes.assigned.insert(name.clone(), Some(index));
        }
        scopes
    }

    /// Whether `state` can count on `name` having been assigned by the time it runs.
    fn holds(&self, state: &str, name: &str) -> bool {
        match self.assigned.get(name) {
            // A variable this machine assigns and the analysis followed: in scope
            // where every run reaching the state has passed an assignment of it.
            Some(Some(index)) => {
                self.masks.get(state).copied().unwrap_or(0) & (1 << index) != 0
            }
            // One it did not follow, which is taken to be in scope.
            Some(None) => true,
            // One no state of this machine assigns: only the enclosing scope can hold
            // it.
            None => self.outer.holds(name),
        }
    }
}

/// One transition of a machine's graph as the scope analysis walks it: where it goes,
/// and which of the machine's assignment blocks runs on the way there.
struct ScopeEdge {
    target: String,
    /// Index into the machine's blocks.
    assigns: usize,
}

/// The states one run can reach from `entry`, following `graph` — past a transition
/// whose own block assigns `without`, that run has assigned it, so such a transition
/// is not followed.
///
/// `without` `None` makes this the states reachable at all. The walk visits each state
/// once, so it costs one pass over the graph.
fn reached_from<'a>(
    entry: Option<&'a str>,
    graph: &BTreeMap<&'a str, Vec<ScopeEdge>>,
    blocks: &[BTreeSet<String>],
    without: Option<&str>,
) -> BTreeSet<&'a str> {
    let mut reached = BTreeSet::new();
    let mut pending: Vec<&str> = entry.into_iter().collect();
    while let Some(state) = pending.pop() {
        let Some((state, edges)) = graph.get_key_value(state) else {
            continue;
        };
        if !reached.insert(*state) {
            continue;
        }
        for edge in edges {
            let assigned = without.is_some_and(|name| {
                blocks
                    .get(edge.assigns)
                    .is_some_and(|block| block.contains(name))
            });
            if !assigned {
                pending.push(edge.target.as_str());
            }
        }
    }
    reached
}

/// What one state ASSIGNS, by the path the assignment runs on.
///
/// A key written as a JSONPath reference field (`"<name>.$"`) assigns the variable
/// `<name>`, the suffix being how that language spells a value read from a path. A
/// state machine NESTED in this state assigns in a scope of its own, so its states'
/// blocks are not read here.
struct Assignments {
    /// The keys of the state's own `Assign` block, which the language evaluates when
    /// the state's work completed.
    own: BTreeSet<String>,
    /// The keys of each `Catch` catcher's own `Assign`, in the catchers' own order,
    /// which are evaluated INSTEAD when the work failed and that catcher took over.
    caught: Vec<BTreeSet<String>>,
}

/// [`Assignments`] of one state.
fn assignments(state: &Map<String, Value>) -> Assignments {
    let names = |block: Option<&Value>| -> BTreeSet<String> {
        block
            .and_then(Value::as_object)
            .map(|block| {
                block
                    .keys()
                    .map(|key| key.strip_suffix(".$").unwrap_or(key).to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    Assignments {
        own: names(state.get(ASSIGN_MEMBER)),
        caught: state
            .get(CATCH_MEMBER)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|catcher| names(catcher.get(ASSIGN_MEMBER)))
            .collect(),
    }
}

/// Every variable one state can assign, on whichever path it runs: what its OWN
/// fields may read, since the language evaluates a state's assignments — its own, or
/// its catcher's where that catcher took over — before the output they feed.
///
/// Which of them a state the transition LEADS TO can count on is a different question,
/// and the one [`Scopes`] answers per transition.
fn assigned_variables(state: &Map<String, Value>) -> BTreeSet<String> {
    let Assignments { mut own, caught } = assignments(state);
    for block in caught {
        own.extend(block);
    }
    own
}

/// Every variable one state's own fields READ: the path from the state to the field
/// that reads it, and the variable's name, each pair once and in a deterministic
/// order.
///
/// A read is what [`super::variable_references`] reports a state referencing a
/// variable through, so the lint below and `DescribeStateMachine`'s
/// `variableReferences` cannot disagree about what a read is: a `$name` term of a
/// JSONata expression (which the language delimits with `{%` and `%}`) or of a
/// JSONPath reference field (a member whose name ends in `.$`). A state machine NESTED
/// in this state is walked as the states it declares, so its reads are reported under
/// their own names.
///
/// The walk is the one [`walk_fields`] does for every field this module reads, so the
/// path a field is reported at is the same one every other diagnostic about it
/// carries.
fn variable_reads(state: &Map<String, Value>) -> Vec<(String, String)> {
    let mut found: BTreeSet<(String, String)> = BTreeSet::new();
    let roots: Vec<(String, &Value, bool)> = state
        .iter()
        .filter(|(member, _)| !NESTED_MACHINE_MEMBERS.contains(&member.as_str()))
        .map(|(member, value)| {
            (
                nested_at("", format_args!("/{}", echoed(member))),
                value,
                member.ends_with(".$"),
            )
        })
        .collect();
    walk_fields(roots, |path, text, reference_field| {
        let mut names = BTreeSet::new();
        for expression in jsonata_expressions(text) {
            scan_expression(expression, &mut names);
        }
        if reference_field {
            scan_expression(text, &mut names);
        }
        for name in names {
            found.insert((path.to_string(), name));
        }
    });
    found.into_iter().collect()
}

/// The query language `declared` names, `None` when the member is absent, or why it
/// is not a query language the Amazon States Language defines.
pub(super) fn declared_language(declared: Option<&Value>) -> Result<Option<&'static str>, String> {
    let Some(declared) = declared else {
        return Ok(None);
    };
    let Some(named) = declared.as_str() else {
        return Err("'QueryLanguage' must be a string".to_string());
    };
    QUERY_LANGUAGES
        .iter()
        .find(|language| **language == named)
        .copied()
        .map(Some)
        .ok_or_else(|| {
            format!(
                "'QueryLanguage' must be one of {}, and this definition names '{}'",
                QUERY_LANGUAGES.join(", "),
                echoed(named)
            )
        })
}

/// The members a state reads its data through in JSONPath, which a state written in
/// JSONata must not carry: that language reads its data through members of its own
/// (`Arguments`, `Output`, `Items`, a `Choice` rule's `Condition`), so each of these
/// is a field of the OTHER query language.
///
/// These are the JSONPath data-flow members the Amazon States Language declares, as
/// [`super::executable_members`] lists them per state type: the `*Path` family, the
/// payload templates and `Pass`'s static `Result`.
pub(super) const JSONPATH_ONLY_MEMBERS: &[&str] = &[
    "CausePath",
    "ErrorPath",
    "HeartbeatSecondsPath",
    "InputPath",
    "ItemsPath",
    "MaxConcurrencyPath",
    "OutputPath",
    "Parameters",
    "Result",
    "ResultPath",
    "ResultSelector",
    "SecondsPath",
    "TimeoutSecondsPath",
    "TimestampPath",
    "ToleratedFailureCountPath",
    "ToleratedFailurePercentagePath",
];

/// The members that declare a state machine of their own, walked as one rather than
/// scanned as this state's data.
///
/// `pub(in crate::state)`, not private to this module: every reader that has to descend
/// a definition's nested state lists — the diagnostics here, the one that echoes a
/// document the definition wrote, the one that finds the state an event belongs to —
/// descends the SAME members, so there is one statement of which they are. A member
/// naming ONE machine carries it as an object and `Branches` carries a list of them, so
/// a reader takes both shapes.
pub(in crate::state) const NESTED_MACHINE_MEMBERS: &[&str] =
    &["Branches", "ItemProcessor", "Iterator"];

/// The member a state hands a failure to, whose own `Assign` and `Output` are the
/// only ones the error output is in scope in.
pub(super) const CATCH_MEMBER: &str = "Catch";

/// The member a state declares the retries of its own work in.
pub(super) const RETRY_MEMBER: &str = "Retry";

/// The member a retrier and a catcher name the errors they take over in.
pub(super) const ERROR_EQUALS_MEMBER: &str = "ErrorEquals";

/// The reserved term a JSONata expression reads the error of a caught failure
/// through, which the language scopes to a `Catch`: `$states.errorOutput` holds "the
/// error output" of the state that failed, and only that `Catch`'s own `Assign` and
/// `Output` are evaluated with it in scope.
const ERROR_OUTPUT: &str = "$states.errorOutput";

/// The reserved term a JSONata expression reads the state's own outcome through,
/// which the language binds only where there IS one: `$states.result` holds what the
/// state's work produced, so it is in scope in a state whose work produces a result
/// ([`RESULT_STATE_TYPES`]) and on that state's success path — never in a `Catch`,
/// which runs because the work produced no result at all.
const RESULT: &str = "$states.result";

/// The state types whose work produces a result, which is what binds [`RESULT`]: a
/// `Task`'s own work, and the per-item and per-branch results a `Map` and a
/// `Parallel` gather. Every other type carries its input through, so there is no
/// result for an expression of it to read.
const RESULT_STATE_TYPES: &[&str] = &["Map", "Parallel", "Task"];

/// How the service states a field an expression reads that the expression's scope does
/// not bind: the field is not there to read, so it is reported as the field it named.
///
/// `field` is the whole field expression as the definition wrote it — the reserved term
/// together with the member accessors written after it ([`super::field_read`]) —
/// because that, and not the term it starts with, is what the caller has to change.
pub(super) fn field_does_not_exist(field: &str) -> String {
    format!("Field '{field}' does not exist.")
}

/// The reserved terms of the state's own outcome that a `member` of a state of this
/// type must NOT read.
///
/// One rule, read off the member and the state's type: a term is reported wherever
/// the language does not bind it, so an expression that could never resolve is
/// refused at validation time rather than at run time.
pub(super) fn out_of_scope_terms(state_type: &str, member: &str) -> Vec<&'static str> {
    let caught = member == CATCH_MEMBER;
    let mut terms = Vec::new();
    if !caught {
        terms.push(ERROR_OUTPUT);
    }
    if caught || !RESULT_STATE_TYPES.contains(&state_type) {
        terms.push(RESULT);
    }
    terms
}

/// The rules a state written in JSONata must satisfy: it carries none of the other
/// language's data-flow members, and each reserved term of its own outcome is read
/// only where the language binds it ([`out_of_scope_terms`]).
fn diagnose_jsonata_state(
    name: &str,
    state: &Map<String, Value>,
    state_type: &str,
    here: &str,
    into: &mut Diagnosis,
) {
    for member in state.keys() {
        // A `"<name>.$"` member is a JSONPath reference field, which is how that
        // language — and only that one — spells a member read from the state's data.
        if JSONPATH_ONLY_MEMBERS.contains(&member.as_str()) || member.ends_with(".$") {
            let member = echoed(member);
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(here, &format!("/{member}")),
                format!(
                    "state '{name}' is written in '{JSONATA}', so it must not carry the \
                     '{JSONPATH}' member '{member}'"
                ),
            ));
        }
    }
    if state_type == "Choice" {
        if let Some(rules) = state.get("Choices").and_then(Value::as_array) {
            for (index, rule) in rules.iter().enumerate() {
                diagnose_jsonata_rule(
                    name,
                    rule,
                    &element_at(here, "Choices", index),
                    MAX_EXPRESSION_DEPTH,
                    into,
                );
            }
        }
    }
    for (member, value) in state {
        if NESTED_MACHINE_MEMBERS.contains(&member.as_str()) {
            continue;
        }
        let shown = echoed(member);
        for term in out_of_scope_terms(state_type, member) {
            // One diagnostic per FIELD that reads the term, which is the granularity
            // `location` documents (`/States/<StateName>/<FieldName>`): a member
            // carrying several such fields is several problems to fix, each at its own
            // place in the caller's document.
            for (field, read) in fields_reading(value, term) {
                into.push(error(
                    UNSUPPORTED_JSONATA_EXPRESSION,
                    at(here, &format!("/{shown}{field}")),
                    field_does_not_exist(&read),
                ));
            }
        }
    }
}

/// One `Choice` rule of a JSONata state, and every rule nested in its `And`, `Or`
/// and `Not`: each one tests a `Condition`, never a JSONPath `Variable`.
///
/// `depth` bounds the nesting this follows, as every other expression tree here is
/// bounded: the rule tree is caller-shaped.
fn diagnose_jsonata_rule(
    name: &str,
    rule: &Value,
    here: &str,
    depth: usize,
    into: &mut Diagnosis,
) {
    let Some(depth) = depth.checked_sub(1) else {
        return;
    };
    let Some(rule) = rule.as_object() else {
        return;
    };
    for (member, value) in rule {
        if member == "Variable" || member.ends_with(".$") {
            let shown = echoed(member);
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(here, &format!("/{shown}")),
                format!(
                    "a 'Choice' rule of state '{name}' is written in '{JSONATA}', so it must not \
                     carry the '{JSONPATH}' member '{shown}'"
                ),
            ));
        }
        match member.as_str() {
            "And" | "Or" => {
                if let Some(nested) = value.as_array() {
                    for (index, rule) in nested.iter().enumerate() {
                        diagnose_jsonata_rule(
                            name,
                            rule,
                            &element_at(here, echoed(member), index),
                            depth,
                            into,
                        );
                    }
                }
            }
            "Not" => diagnose_jsonata_rule(name, value, &nested_at(here, "/Not"), depth, into),
            _ => {}
        }
    }
}

/// Every FIELD inside `value` whose JSONata expression reads `term`: the path from the
/// member that carries `value` down to that field — the empty path for a member that is
/// itself such an expression — paired with the field expression that read the term
/// there ([`field_read`]).
///
/// The list is sorted lexicographically, which is what makes the per-field list of one
/// member deterministic whatever shape the document has.
fn fields_reading(value: &Value, term: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    walk_fields(vec![(String::new(), value, false)], |path, text, _| {
        // One field is one problem to fix, so it is reported once: the field
        // expression reported for it is the FIRST its expressions read the term
        // through.
        if let Some(read) = jsonata_expressions(text)
            .iter()
            .find_map(|expression| field_read(expression, term))
        {
            found.push((path.to_string(), read));
        }
    });
    found.sort();
    found
}

/// Visit every string `roots` carries, with the path from its root down to it and
/// whether it is the value of a JSONPath reference field (a member whose name ends in
/// `.$`), which is what makes a string a reference rather than a literal.
///
/// The one walk of a caller-shaped document this module reads a state's fields with,
/// so the path a field is reported at cannot differ between what reads it. It is
/// ITERATIVE: the document is a caller's, so it carries its own stack rather than the
/// thread's, and each path is built through [`nested_at`], so it is bounded as it
/// grows.
fn walk_fields(roots: Vec<(String, &Value, bool)>, mut visit: impl FnMut(&str, &str, bool)) {
    let mut pending = roots;
    while let Some((path, value, reference_field)) = pending.pop() {
        match value {
            Value::String(text) => visit(&path, text, reference_field),
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    pending.push((indexed_at(&path, index), item, reference_field));
                }
            }
            Value::Object(members) => {
                for (member, child) in members {
                    pending.push((
                        nested_at(&path, format_args!("/{}", echoed(member))),
                        child,
                        member.ends_with(".$"),
                    ));
                }
            }
            _ => {}
        }
    }
}

/// The FIELD a JSONata `expression` reads `term` through, or `None` when it reads it
/// nowhere.
///
/// A field expression is the term together with the member accessors written after it:
/// the language reads a member of a value by a `.` followed by that member's name, so
/// the field runs from the term through every such step that follows it and ends at the
/// first character that can be part of neither. A term that is itself only the start of
/// a longer name is read to the end of that name, since what was written there is a
/// different field and not the term at all.
///
/// The result is [`echoed`]: the accessors are the caller's own text.
fn field_read(expression: &str, term: &str) -> Option<String> {
    let read = &expression[expression.find(term)?..];
    let bytes = read.as_bytes();
    let mut end = term.len();
    while bytes.get(end).is_some_and(|byte| is_member_byte(*byte)) {
        end += 1;
    }
    while bytes.get(end) == Some(&b'.') {
        let mut step = end + 1;
        let name = step;
        while bytes.get(step).is_some_and(|byte| is_member_byte(*byte)) {
            step += 1;
        }
        if step == name {
            break;
        }
        end = step;
    }
    Some(echoed(&read[..end]))
}

/// Whether `byte` can be part of a name written without quoting: what bounds the field
/// expression [`field_read`] reads, and the `$name` term
/// [`super::scan_expression`] reads.
pub(super) fn is_member_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The payload templates a JSONPath state renders: every member of one is a LITERAL
/// unless its name ends in `.$`, which is what makes its value a path.
const PAYLOAD_TEMPLATE_MEMBERS: &[&str] = &["ItemSelector", "Parameters", "ResultSelector"];

/// Members whose string value names something — a state, a type, a language — rather
/// than holding the state's data, so a `$` in one is part of that name.
const NAMING_MEMBERS: &[&str] = &[
    "Comment",
    "Default",
    "Next",
    "QueryLanguage",
    "StartAt",
    "Type",
];

/// The WARNINGS a state written in JSONPath can carry: each one is a value that
/// READS as a path or an intrinsic invocation where the language takes a literal, so
/// the definition is created and the value is used verbatim.
fn diagnose_jsonpath_state(
    name: &str,
    state: &Map<String, Value>,
    state_type: &str,
    here: &str,
    into: &mut Diagnosis,
) {
    // PASS_RESULT_IS_STATIC: "Attempt to use a path in the result of a pass state".
    if state_type == "Pass" {
        if let Some(result) = state.get("Result").and_then(Value::as_str) {
            if looks_like_a_path(result) {
                into.push(warning(
                    PASS_RESULT_IS_STATIC,
                    at(here, "/Result"),
                    format!(
                        "the 'Result' of state '{name}' holds '{}', which reads as a path, \
                         but a 'Result' is the state's literal result: 'Parameters' or \
                         'ResultPath' read a path",
                        echoed(result)
                    ),
                ));
            }
        }
    }
    // NO_DOLLAR: "No `.$` on a field that appears to be a JSONPath or Intrinsic
    // Function".
    for member in PAYLOAD_TEMPLATE_MEMBERS {
        let Some(template) = state.get(*member).and_then(Value::as_object) else {
            continue;
        };
        for (key, value) in template {
            let Some(text) = value.as_str() else {
                continue;
            };
            if key.ends_with(".$") || !(looks_like_a_path(text) || looks_like_an_intrinsic(text)) {
                continue;
            }
            let key = echoed(key);
            let text = echoed(text);
            into.push(warning(
                NO_DOLLAR,
                at(here, &format!("/{member}/{key}")),
                format!(
                    "'{key}' of the '{member}' of state '{name}' holds '{text}', which reads as a \
                     path or an intrinsic function, but a member evaluated as one is named \
                     '{key}.$'"
                ),
            ));
        }
    }
    // NO_PATH: "Field value looks like a path, but field name does not end with
    // 'Path'".
    for (member, value) in state {
        let Some(text) = value.as_str() else {
            continue;
        };
        if NAMING_MEMBERS.contains(&member.as_str())
            || member.ends_with("Path")
            || member.ends_with(".$")
            || (state_type == "Pass" && member == "Result")
            || !looks_like_a_path(text)
        {
            continue;
        }
        let member = echoed(member);
        let text = echoed(text);
        into.push(warning(
            NO_PATH,
            at(here, &format!("/{member}")),
            format!(
                "the '{member}' of state '{name}' holds '{text}', which reads as a path, but \
                 '{member}' does not end in 'Path', so it is used as the literal it is"
            ),
        ));
    }
}

/// Whether `text` reads as a JSONPath: every path the language takes starts at the
/// state's data (`$`) or at the context object (`$$`).
fn looks_like_a_path(text: &str) -> bool {
    text.starts_with('$')
}

/// Whether `text` reads as an intrinsic-function invocation, which the language
/// spells `States.<Name>(<arguments>)`.
fn looks_like_an_intrinsic(text: &str) -> bool {
    text.starts_with("States.") && text.contains('(') && text.ends_with(')')
}

/// One transition a state declares: where it goes, where in the state the member that
/// names it sits, and whether it is taken because the state FAILED.
pub(super) struct Transition {
    pub(super) target: String,
    /// The path from the state to the member naming the target.
    path: String,
    /// `Some(index)` for the `Next` of the `Catch` catcher at that index: a
    /// transition the state's own work never completed, so the state's own `Assign`
    /// did not run and that catcher's did.
    caught: Option<usize>,
}

/// Every transition `state` declares, in the order the members that name them are
/// judged: its own `Next`, the `Default` of a `Choice`, each `Choice` rule's own
/// `Next`, and each `Catch` catcher's `Next`.
///
/// ONE enumeration, read both by the diagnostic that resolves a target against the
/// enclosing `States` map and by the state graph the variable-scope analysis walks, so
/// the two cannot disagree about where a state can go.
///
/// Linear in the state's own size, which the modelled `Definition` (1 MiB) bounds, so
/// the list this builds is bounded by the request exactly as the state is.
pub(super) fn transitions(state: &Map<String, Value>) -> Vec<Transition> {
    let mut transitions = Vec::new();
    for member in ["Next", "Default"] {
        if let Some(target) = state.get(member).and_then(Value::as_str) {
            transitions.push(Transition {
                target: target.to_string(),
                path: format!("/{member}"),
                caught: None,
            });
        }
    }
    for (member, caught) in [("Choices", false), (CATCH_MEMBER, true)] {
        let entries = state
            .get(member)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (index, entry) in entries.iter().enumerate() {
            if let Some(target) = entry.get("Next").and_then(Value::as_str) {
                transitions.push(Transition {
                    target: target.to_string(),
                    path: nested_at(&element_at("", member, index), "/Next"),
                    caught: caught.then_some(index),
                });
            }
        }
    }
    transitions
}

/// The states `state` can transition to, however it names them: what the lone state
/// of a [`super::validate_single_state`] has declared beside it.
pub(super) fn transition_targets(state: &Value) -> Vec<String> {
    state
        .as_object()
        .map(|state| {
            transitions(state)
                .into_iter()
                .map(|transition| transition.target)
                .collect()
        })
        .unwrap_or_default()
}


