// SPDX-License-Identifier: CC0-1.0

//! JSONata, the other query language the Amazon States Language selects between.
//!
//! A state machine — or one state of it — declares `"QueryLanguage": "JSONata"`, and
//! every expression that state carries is then written in JSONata rather than in
//! JSONPath: its `Output`, its `Assign`, a `Task`'s `Arguments`, a `Choice` rule's
//! `Condition`, a `Map`'s `Items`, a `Wait`'s `Seconds`. The language delimits one
//! expression with `{%` and `%}`, and a field that carries no such delimiter is the
//! literal it is written as.
//!
//! The expressions are EVALUATED, through [`::service_common::jsonata`] — this
//! workspace's one bounded seam onto the maintained JSONata interpreter — rather than
//! by a hand-written subset of the language. That seam is where an engine evaluating a
//! caller's own text is CONFINED: the ceilings on one expression's text, its parse,
//! its stack, its depth, its memory, the value it builds and how many evaluations may
//! be alive at once are stated and met there, once, for every service whose API
//! carries such an expression.
//!
//! What THIS module adds is the Amazon States Language's own half: which reserved
//! terms an expression of a state reads, what one visit to a state may ask for, and
//! how an expression that MISEVALUATED is reported.
//!
//! ## A template and a field
//!
//! A TEMPLATE — an `Output`, an `Assign`, a `Task`'s `Arguments` — builds a document
//! out of whatever its members produce, and a member whose expression produced nothing
//! is left out of it ([`render`]). A FIELD holds ONE value the language requires a
//! type of — a `Choice` rule's boolean `Condition`, a `Wait`'s numeric `Seconds`, a
//! `Map`'s array of `Items`, a `Fail`'s string `Error` — so a field whose expression
//! produced another type, or produced nothing, is reported rather than carried
//! ([`typed`]). The diagnosis is the service's own: the expression as the definition
//! wrote it, the field of the definition it is written in, and the types.
//!
//! ## What an expression reads
//!
//! `$states` is the reserved term: `$states.input` is the state's own input,
//! `$states.context` the context object, `$states.result` what the state's work
//! produced (a `Task`'s answer, a `Map`'s or `Parallel`'s gathered results) and
//! `$states.errorOutput` the error output inside a `Catch`. Each is bound only where
//! the language binds it, which is what the definition's own validation refuses an
//! expression for reading out of scope. Every variable a state `Assign`ed is bound
//! under its own name (`$total`), and the expression's context item (`$`) is the
//! state's input, so a bare field reference reads the input.
//!
//! ## What bounds one state's whole template
//!
//! A state's template is the PRODUCT of how many expressions it carries and how large
//! a value each builds, and a definition — bounded only at the modelled 1 MiB —
//! chooses both. [`Budget`] is what bounds that product, as the members are built
//! rather than after: one allowance of what a state's document may carry, spent by
//! every member the template renders, and one count of the expressions it may
//! evaluate — the two the other query language's payload template is bounded by
//! ([`super::asl::MAX_PAYLOAD_BYTES`], [`super::asl::MAX_TEMPLATE_PATHS`]).
//!
//! One allowance covers one TEMPLATE or FIELD: the state's `Assign` and the `Output`
//! those assignments feed share one, and each of the other members a state can carry
//! — a `Task`'s or `Parallel`'s `Arguments`, a `Map`'s `Items` and `ItemSelector`, a
//! `Wait`'s own members, a `Choice` rule's `Condition` — is given its own, because
//! each is a separate document the state builds. How many of those one visit can ask
//! for is fixed by the state's TYPE, and for a `Choice` by its rule count, which is
//! itself bounded ([`super::asl::MAX_CHOICE_RULES`]); each is built and spent in turn.
//! Every evaluation is also CHARGED to the work allowance of the interpretation
//! around it ([`super::asl::MAX_INTERPRET_WORK`]): the span an expression runs for is
//! the caller's own code running inside the engine, so the engine's per-expression
//! ceiling bounds one of them and that allowance bounds what they cost together,
//! leaving no clock between a definition and the outcome of its execution.

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::{Map, Value};

use ::service_common::jsonata as engine;

/// The term an expression reads the state's own data through: `$states.input`,
/// `$states.context`, `$states.result`, `$states.errorOutput`.
const STATES_TERM: &str = "states";

/// The members of [`STATES_TERM`], as the language spells them.
const INPUT_MEMBER: &str = "input";
const CONTEXT_MEMBER: &str = "context";
const RESULT_MEMBER: &str = "result";
const ERROR_OUTPUT_MEMBER: &str = "errorOutput";

/// What opens and closes one JSONata expression inside a definition's field.
pub const OPEN: &str = "{%";
pub const CLOSE: &str = "%}";

/// The longest ONE expression of a state is evaluated for, in milliseconds.
///
/// Emulator bound, NOT an AWS limit: a state evaluates several expressions and the
/// walk runs on a blocking thread, so one of them is bounded here — and what each one
/// TOOK is charged to the interpretation's own work allowance
/// ([`Budget::charge`], [`super::asl::WORK_PER_EVALUATED_MILLISECOND`]), so the
/// expressions of one run cannot hold the thread past what the whole run may spend.
pub const EVALUATION_TIME_LIMIT_MS: usize = 250;

/// What one expression of a state is evaluated with: the reserved terms the language
/// binds there, and the variables the run has assigned.
#[derive(Debug, Clone, Copy)]
pub struct Bindings<'b> {
    /// `$states.input`, which is also the expression's own context item (`$`).
    pub input: &'b Value,
    /// `$states.context`: the context object a definition reads its execution's
    /// identity through.
    pub context: &'b Value,
    /// `$states.result`: what the state's work produced, bound only in a state whose
    /// work produces one and on its success path.
    pub result: Option<&'b Value>,
    /// `$states.errorOutput`: the error output of a state whose failure was caught,
    /// bound only inside the catcher that took it.
    pub error_output: Option<&'b Value>,
    /// The variables in scope, each read under its own name (`$total`).
    pub variables: &'b BTreeMap<String, Value>,
}

impl Bindings<'_> {
    /// `$states` as a document: the members the language binds, and no member for one
    /// it does not bind here — an expression reading an unbound member reads nothing,
    /// exactly as it does on the service.
    fn states(&self) -> Value {
        let mut states = Map::new();
        states.insert(INPUT_MEMBER.to_string(), self.input.clone());
        states.insert(CONTEXT_MEMBER.to_string(), self.context.clone());
        if let Some(result) = self.result {
            states.insert(RESULT_MEMBER.to_string(), result.clone());
        }
        if let Some(error_output) = self.error_output {
            states.insert(ERROR_OUTPUT_MEMBER.to_string(), error_output.clone());
        }
        Value::Object(states)
    }
}

/// Why a state's expressions were not evaluated, in the classes the Amazon States
/// Language reports them under.
///
/// The classes are the caller's to act on: a template that outgrew what a state may
/// carry is the language's data-limit failure, and everything else — an expression
/// that could not be parsed or evaluated, and every ceiling on the work one visit to
/// a state may do — is its query-evaluation failure. An expression the engine
/// EVALUATED and that threw is held apart from the rest of those, because the field it
/// is written in is reported around it ([`in_field`]).
#[derive(Debug, Clone)]
pub enum Refusal {
    /// The template built more than one state's document may carry.
    TooLarge(String),
    /// The expression was evaluated and THREW, carrying the engine's own account of
    /// the failure: its code and its message.
    Threw(String),
    /// The expressions were not evaluated.
    NotEvaluated(String),
}

impl From<engine::Refused> for Refusal {
    /// A refusal of the shared seam, in this language's own classes.
    fn from(refused: engine::Refused) -> Self {
        match refused {
            engine::Refused::TooLarge(why) => Refusal::TooLarge(why),
            engine::Refused::Threw(account) => Refusal::Threw(account),
            engine::Refused::NotEvaluated(why) => Refusal::NotEvaluated(why),
        }
    }
}

/// What the expressions of ONE template or field may build, and may evaluate.
///
/// A template's cost is the PRODUCT of how many expressions it carries and how large
/// a value each builds, and a definition chooses both, so the product is spent from
/// one allowance as the members are built rather than measured once they are whole. A
/// state's `Assign` and the `Output` it feeds share one allowance; every other member
/// a state can carry is a document of its own and is given its own, as the module
/// header states. What each evaluation COSTS is charged to the work allowance of the
/// interpretation around it, so a state's expressions cannot hold the blocking thread
/// the walk runs on past what the whole run may spend.
#[derive(Debug)]
pub struct Budget {
    bytes: usize,
    expressions: usize,
}

impl Default for Budget {
    /// The allowance one template or field gets.
    fn default() -> Self {
        Self {
            bytes: super::asl::MAX_PAYLOAD_BYTES,
            expressions: super::asl::MAX_TEMPLATE_PATHS,
        }
    }
}

impl Budget {
    /// The allowance one template or field gets.
    pub fn new() -> Self {
        Self::default()
    }

    /// Spend `bytes` of what this visit may still build.
    fn spend(&mut self, bytes: usize) -> Result<(), Refusal> {
        match self.bytes.checked_sub(bytes) {
            Some(left) => {
                self.bytes = left;
                Ok(())
            }
            None => Err(Refusal::TooLarge(format!(
                "this state's expressions build more than the {} bytes a state's output may carry",
                super::asl::MAX_PAYLOAD_BYTES
            ))),
        }
    }

    /// Charge ONE evaluation, and answer how many milliseconds it may run for: the
    /// ceiling one expression is allowed on its own.
    ///
    /// A visit whose interpretation has already spent the work it may do evaluates
    /// nothing further, exactly as it makes no further transition.
    fn charge(&mut self) -> Result<usize, Refusal> {
        let Some(left) = self.expressions.checked_sub(1) else {
            return Err(Refusal::NotEvaluated(format!(
                "this template carries more than the {} expressions this service evaluates",
                super::asl::MAX_TEMPLATE_PATHS
            )));
        };
        self.expressions = left;
        if super::asl::work_spent() {
            return Err(Refusal::NotEvaluated(format!(
                "the execution asked for more than the {} units of interpretation work this \
                 emulator does for one definition, so its expressions were not evaluated",
                super::asl::MAX_INTERPRET_WORK
            )));
        }
        Ok(EVALUATION_TIME_LIMIT_MS)
    }
}

/// Whether `text` is ONE whole JSONata expression — the only form a field's value is
/// evaluated in, since the language delimits an expression with [`OPEN`] and
/// [`CLOSE`] and a field carrying anything else is the literal it is written as.
pub fn is_expression(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.len() > OPEN.len() + CLOSE.len()
        && trimmed.starts_with(OPEN)
        && trimmed.ends_with(CLOSE)
}

/// The expression inside the delimiters of `text`, for a `text` [`is_expression`]
/// answers for.
fn inner(text: &str) -> &str {
    let trimmed = text.trim();
    trimmed[OPEN.len()..trimmed.len() - CLOSE.len()].trim()
}

/// Evaluate the expression `text` carries, and report the value it produced.
///
/// `Ok(None)` is an expression that resolved to NOTHING, which the language's own
/// `undefined` is: a field whose expression produces nothing carries no value, so the
/// caller drops the member rather than writing a null the definition did not ask for.
///
/// The evaluation is charged to `budget` BEFORE it is made — which is also what hands
/// the seam the time it may run for — and what remains of the visit's allowance is the
/// size the value it produces is held to, so neither the number of expressions nor the
/// size of what they build is left to the definition.
fn evaluate(
    text: &str,
    bindings: &Bindings<'_>,
    budget: &mut Budget,
) -> Result<Option<Value>, Refusal> {
    let allowed_ms = budget.charge()?;
    let expression = inner(text);
    let states = bindings.states();
    // The reserved terms and the assigned variables, as the seam binds them: an
    // expression reads them exactly as it does on the service.
    let mut variables: Vec<(&str, &Value)> = Vec::with_capacity(bindings.variables.len() + 1);
    variables.push((STATES_TERM, &states));
    for (name, value) in bindings.variables {
        variables.push((name.as_str(), value));
    }
    let began = Instant::now();
    let answered = engine::evaluate(&engine::Expression {
        text: expression,
        input: bindings.input,
        variables: &variables,
        allowed_ms,
        allowance: budget.bytes,
    });
    // An expression is a caller's own code running inside the engine, so what it costs
    // the interpretation around it is the span it RAN for: charged whether it produced
    // a value or was refused, at the rate one millisecond of evaluation costs.
    let spent = u64::try_from(began.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .saturating_mul(super::asl::WORK_PER_EVALUATED_MILLISECOND)
        // The text of the expression is work of its own, so an evaluation that took
        // less than a millisecond still costs what parsing it did.
        .max(u64::try_from(expression.len()).unwrap_or(u64::MAX));
    super::asl::spend_work(spent);
    let answered = answered?;
    let Some((value, built)) = answered else {
        return Ok(None);
    };
    // What the expression produced is spent from the visit's allowance, which is the
    // allowance the seam already held it to: a template of many large members is
    // stopped at the member that takes it past what a state may carry.
    budget.spend(built)?;
    Ok(Some(value))
}

/// A JSONata-mode TEMPLATE — an `Output`, an `Arguments`, an `Assign` — as the value
/// it builds.
///
/// One rule, applied to every node: a string that is one whole expression is replaced
/// by what that expression produced, and every other value is the literal it is
/// written as. An object member whose expression produced NOTHING is left out of the
/// result, which is what the language does with `undefined`; an array element that
/// produced nothing is left out for the same reason.
///
/// Every member — the literal ones included — is spent from `budget` as it is built,
/// and every expression is charged to it, so what ONE visit to a state renders is
/// bounded in bytes and in evaluations however many members the definition declares.
pub fn render(
    node: &Value,
    field: &str,
    bindings: &Bindings<'_>,
    budget: &mut Budget,
) -> Result<Value, Refusal> {
    render_at(node, field, bindings, engine::MAX_EVALUATION_DEPTH, budget)
}

fn render_at(
    node: &Value,
    field: &str,
    bindings: &Bindings<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Result<Value, Refusal> {
    let Some(depth) = depth.checked_sub(1) else {
        return Err(Refusal::NotEvaluated(format!(
            "a field nests deeper than the {} levels this emulator evaluates",
            engine::MAX_EVALUATION_DEPTH
        )));
    };
    match node {
        Value::String(text) if is_expression(text) => Ok(in_field(
            evaluate(text, bindings, budget),
            text,
            field,
        )?
        .unwrap_or(Value::Null)),
        Value::Array(items) => {
            let mut rendered = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                // The comma and the brackets each element costs the document it is
                // built into, charged before the element itself is.
                budget.spend(1)?;
                let at = element_field(field, index);
                match item {
                    Value::String(text) if is_expression(text) => {
                        if let Some(value) =
                            in_field(evaluate(text, bindings, budget), text, &at)?
                        {
                            rendered.push(value);
                        }
                    }
                    other => rendered.push(render_at(other, &at, bindings, depth, budget)?),
                }
            }
            Ok(Value::Array(rendered))
        }
        Value::Object(members) => {
            let mut rendered = Map::new();
            for (member, child) in members {
                // The member's own name, its quotes, its colon and the comma after
                // it: what the document carries for the member whatever its value is.
                budget.spend(member.len() + 4)?;
                let at = member_field(field, member);
                match child {
                    Value::String(text) if is_expression(text) => {
                        if let Some(value) =
                            in_field(evaluate(text, bindings, budget), text, &at)?
                        {
                            rendered.insert(member.clone(), value);
                        }
                    }
                    other => {
                        rendered.insert(
                            member.clone(),
                            render_at(other, &at, bindings, depth, budget)?,
                        );
                    }
                }
            }
            Ok(Value::Object(rendered))
        }
        // A literal the template carries through: it is part of the document the
        // state will put out, so it is spent from the same allowance.
        other => {
            budget.spend(other.to_string().len())?;
            Ok(other.clone())
        }
    }
}

/// Where the member `member` of the template at `field` is written, as a diagnosis
/// names a field of a state: the members joined by `/`, which is how the language
/// addresses a field nested inside one (`Assign/total`).
pub fn member_field(field: &str, member: &str) -> String {
    format!("{field}/{member}")
}

/// Where the element at `index` of the array at `field` is written: the array's own
/// field with the index bracketed onto it (`Arguments/values[0]`), which is how the
/// language addresses one element of a template.
fn element_field(field: &str, index: usize) -> String {
    format!("{field}[{index}]")
}

/// How the language opens its diagnosis of a field's expression that MISEVALUATED,
/// and the clause that names the field inside it.
///
/// RECORDED wording: a diagnosis names the expression as the definition wrote it
/// (inside the delimiters, which it does not repeat), then the field of the definition
/// that expression is written in, then what was wrong with the value it produced. The
/// two are also what reads the field back OUT of a diagnosis this module worded
/// ([`diagnosed_field`]), so the one place that words it is the one place that knows
/// its shape.
pub const DIAGNOSIS_OPENING: &str = "The JSONata expression '";
/// The field a diagnosis this module worded names — "the location of the field in the
/// state in which the evaluation error occurred".
const FIELD_CLAUSE: &str = "' specified for the field '";

/// What the diagnosis says of an expression that produced NOTHING, which the
/// language's own `undefined` is: a field whose value the language requires has none.
const RETURNED_NOTHING: &str = "returned nothing (undefined).";

/// What the diagnosis says of an expression that THREW: the engine's own account of
/// the failure follows it.
const THREW_AN_ERROR: &str = "threw an error during evaluation.";

/// The JSON type ONE field of a state requires the expression in it to produce.
///
/// A field is not a TEMPLATE: a template builds a document of whatever its members
/// produce, while a field holds one value the language requires a type of — a boolean
/// `Condition`, a numeric `Seconds`, a string `Error`, an array of `Items` — so a
/// value of another type, and a field with no value at all, are reported rather than
/// carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Boolean,
    Number,
    String,
    Array,
}

impl Kind {
    /// This type's name, as the JSON types are spelled.
    fn named(self) -> &'static str {
        match self {
            Kind::Boolean => "boolean",
            Kind::Number => "number",
            Kind::String => "string",
            Kind::Array => "array",
        }
    }

    /// Whether `value` is of this type.
    fn holds(self, value: &Value) -> bool {
        match self {
            Kind::Boolean => value.is_boolean(),
            Kind::Number => value.is_number(),
            Kind::String => value.is_string(),
            Kind::Array => value.is_array(),
        }
    }
}

/// The JSON type `value` IS, as the types are spelled.
fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => Kind::Boolean.named(),
        Value::Number(_) => Kind::Number.named(),
        Value::String(_) => Kind::String.named(),
        Value::Array(_) => Kind::Array.named(),
        Value::Object(_) => "object",
    }
}

/// The diagnosis of the expression `text`, written in the definition field `field`:
/// the expression as the definition wrote it, the field it is written in, and `what`
/// was wrong with the value it produced.
///
/// The field is remembered beside the diagnosis ([`failed_field`]), which is what the
/// event reporting the failure carries.
fn misevaluated(text: &str, field: &str, what: &str) -> Refusal {
    failed_in(field);
    Refusal::NotEvaluated(format!(
        "{DIAGNOSIS_OPENING}{}{FIELD_CLAUSE}{field}' {what}",
        inner(text)
    ))
}

/// What the definition field `field` carries, held to the type the language requires
/// there.
///
/// A field whose value is ONE whole expression is evaluated and the value checked:
/// an expression that produced nothing, and one that produced a value of another
/// type, are the language's own query-evaluation failures, diagnosed as the service
/// diagnoses them ([`misevaluated`]) so a caller reads the expression, the field it is
/// written in and the type it expected. Every other field value is the literal it is
/// written as, carried through for the caller's own reading of the member to answer
/// for.
pub fn typed(
    node: &Value,
    field: &str,
    expected: Kind,
    bindings: &Bindings<'_>,
    budget: &mut Budget,
) -> Result<Value, Refusal> {
    let text = match node {
        Value::String(text) if is_expression(text) => text,
        other => return Ok(other.clone()),
    };
    match in_field(evaluate(text, bindings, budget), text, field)? {
        None => Err(misevaluated(text, field, RETURNED_NOTHING)),
        Some(value) if expected.holds(&value) => Ok(value),
        Some(value) => Err(misevaluated(
            text,
            field,
            &format!(
                "returned an unexpected result type. Expected '{}', but was '{}' for value: \
                 {value}",
                expected.named(),
                type_of(&value)
            ),
        )),
    }
}

/// `evaluated`, as the field `field` of a state reports an expression that THREW.
///
/// The engine's own account of the failure says only what went wrong inside the
/// expression; which FIELD of which state was being evaluated is what a caller acts on,
/// so an evaluation that failed is reported under the same diagnosis a field's value of
/// the wrong type is ([`misevaluated`]), with the engine's account as its detail. Every
/// other refusal — a ceiling this emulator holds an expression to, a template that
/// outgrew what a state may carry — is carried as raised: it is about the request rather
/// than about the expression's own evaluation.
fn in_field<T>(
    evaluated: Result<T, Refusal>,
    text: &str,
    field: &str,
) -> Result<T, Refusal> {
    evaluated.map_err(|refusal| match refusal {
        Refusal::Threw(account) => {
            misevaluated(text, field, &format!("{THREW_AN_ERROR} {account}"))
        }
        // A template that outgrew what a state may carry is reported in the service's
        // own words about a state's result rather than about the expression, so the
        // refusal is carried as raised — but the FIELD it was building is remembered,
        // which is what the event reporting the failure names.
        Refusal::TooLarge(why) => {
            failed_in(field);
            Refusal::TooLarge(why)
        }
        other => other,
    })
}

/// The diagnosis of a field's expression that produced a value the language does not
/// accept THERE: the expression, the field, what that field must carry, and the value.
///
/// The field's own type check is [`typed`]; this is the one for a value of the right
/// JSON type that the field cannot take all the same — a `Wait`'s `Timestamp` that is a
/// string but not an instant, a `Seconds` that is a number but not a whole one — which
/// is the same expression misevaluating and is diagnosed the same way.
///
/// `None` for a field that carries a literal rather than an expression: nothing was
/// evaluated for it, so its value is the definition's own.
pub fn invalid_value(node: &Value, field: &str, must_be: &str, value: &Value) -> Option<Refusal> {
    let Value::String(text) = node else {
        return None;
    };
    if !is_expression(text) {
        return None;
    }
    Some(misevaluated(
        text,
        field,
        &format!(
            "returned an unexpected result type. Expected '{must_be}', but was '{}' for value: \
             {value}",
            type_of(value)
        ),
    ))
}

/// The field a diagnosis this module worded names — "the location of the field in the
/// state in which the evaluation error occurred", which the event reporting the
/// failure carries, TAKEN from the thread it was remembered on.
///
/// It is REMEMBERED where the diagnosis is worded rather than read back out of it: the
/// expression a diagnosis echoes is the DEFINITION's text and can carry anything the
/// sentence around it does, so no reading of the finished cause can tell the field this
/// module named from one a definition wrote inside its own expression. The state about
/// to be run empties the cell ([`clear_failed_field`]), so the field reported for a
/// state is one of ITS own expressions or none at all.
pub fn take_failed_field() -> Option<String> {
    FAILED_FIELD.with(|field| field.borrow_mut().take())
}

/// Forget the field remembered on this thread, for the state about to evaluate its own
/// expressions: what an earlier state's failure left behind is not this state's to
/// report ([`take_failed_field`]).
pub fn clear_failed_field() {
    let _forgotten = take_failed_field();
}

thread_local! {
    /// The field of a state the last expression that FAILED on this thread is written
    /// in ([`take_failed_field`]).
    ///
    /// One state's expressions are evaluated, and the event their failure writes is
    /// recorded, on the one thread that walk runs on, so the field travels with the
    /// failure without every refusal having to carry it — the same way the work one
    /// interpretation may do is ambient to the thread it runs on. A thread no walk is
    /// running on holds none.
    static FAILED_FIELD: ::std::cell::RefCell<Option<String>> =
        const { ::std::cell::RefCell::new(None) };
}

/// Remember `field` as the field the expression that just failed is written in.
fn failed_in(field: &str) {
    FAILED_FIELD.with(|held| {
        held.replace(Some(field.to_string()));
    });
}

/// A field that is EITHER one whole expression or the literal value it is written as:
/// a `Wait`'s `Seconds`, a `Map`'s `Items`, a `Fail`'s `Error`.
///
/// `Ok(None)` is an expression that resolved to nothing, exactly as the evaluation
/// reports one.
pub fn field(
    node: &Value,
    field: &str,
    bindings: &Bindings<'_>,
    budget: &mut Budget,
) -> Result<Option<Value>, Refusal> {
    match node {
        Value::String(text) if is_expression(text) => {
            in_field(evaluate(text, bindings, budget), text, field)
        }
        other => Ok(Some(other.clone())),
    }
}

/// Whether the `Condition` of one `Choice` rule holds.
///
/// "Condition — a boolean expression": a rule whose expression produces anything
/// other than a boolean, and one whose expression produces nothing at all, is a rule
/// the service cannot decide, so each is reported — under the language's own
/// query-evaluation failure, diagnosed as the service diagnoses a field's expression
/// that misevaluated ([`typed`]) — rather than taken as false.
///
/// `field` is the rule's own place in the definition (`Choices[<index>]/Condition`),
/// which is what tells the offending rule from every other rule of a state that
/// declares many.
pub fn condition(
    node: &Value,
    field: &str,
    bindings: &Bindings<'_>,
    budget: &mut Budget,
) -> Result<bool, Refusal> {
    let Value::String(text) = node else {
        return Err(Refusal::NotEvaluated(format!(
            "a 'Choice' rule's 'Condition' must be a JSONata expression, not {node}"
        )));
    };
    if !is_expression(text) {
        return Err(Refusal::NotEvaluated(format!(
            "a 'Choice' rule's 'Condition' must be a JSONata expression delimited with \
             '{OPEN}' and '{CLOSE}', and this rule carries '{}'",
            super::asl::echoed(text)
        )));
    }
    // Every other value and an expression that produced nothing are already refused,
    // so what comes back is the boolean the rule decides on.
    Ok(typed(node, field, Kind::Boolean, bindings, budget)?
        .as_bool()
        .unwrap_or(false))
}
