// SPDX-License-Identifier: CC0-1.0

//! Bounded evaluation of a caller-written JSONata expression.
//!
//! Two of this emulator's services evaluate JSONata that arrives in a request: the
//! B2BI transformer's mapping template, and the Amazon States Language's own
//! JSONata-mode fields. The expression and the data it reads are both a caller's, so
//! what has to be stated once — here — is not how to call the engine but what CONFINES
//! it: an engine that evaluates caller-written text is an execution substrate, and the
//! ceilings below are what keep one request's expression from reaching the host.
//!
//! This is where a service whose API carries such an expression reaches the engine.
//! The Amazon States Language's JSONata mode does
//! (`services/service-com-amazonaws-sfn-awsstepfunctions/src/state/jsonata.rs`); the
//! B2BI mapping template still carries its own containment of the same engine, with
//! its own ceilings sized for the much larger document that operation maps, and
//! moving it onto this seam is work of its own rather than part of the change that
//! brought this module here.
//!
//! The engine is [`::jsonata_rs`], the maintained Rust implementation of the language,
//! pinned exactly: the parse shapes this module reads are that version's.
//!
//! ## What the engine gives us, and what this module enforces
//!
//! INHERITED from the engine's own fixed binding set: its functions compute over the
//! data they are handed — strings, numbers, arrays, objects, dates — and it exposes no
//! filesystem, process, socket, environment, module-import or `eval` function, so a
//! caller's expression has no primitive with which to reach outside this process. No
//! host surface is opened by evaluating one, and nothing here has to close one.
//!
//! ENFORCED HERE, because the engine does not bound them:
//!
//! * the EXPRESSION TEXT: at most [`MAX_EXPRESSION_BYTES`], carrying at most
//!   [`MAX_EXPRESSION_GROUPINGS`] openings of a bracket, brace or parenthesis —
//!   checked BEFORE the parse, which is recursive descent and therefore costs a frame
//!   per nested construct;
//! * the CONTEXT DOCUMENT the expression reads: at most [`MAX_INPUT_BYTES`], nesting
//!   at most [`MAX_EVALUATION_DEPTH`] levels, both checked before the engine is handed
//!   its text — the engine parses that text recursively too, so its depth is a frame
//!   count exactly as the expression's is. The ceiling is applied to a document this
//!   process has ALREADY parsed ([`Expression::input`] is a value, not text) and the
//!   depth check over it is ITERATIVE, so neither this module's own reading of a
//!   caller's document nor the one the caller already did can recurse at all;
//! * the STACK the engine's parses run on: the evaluation happens on a thread of its
//!   own with [`EVALUATION_STACK_BYTES`] reserved, sized for the frames BOTH of those
//!   ceilings admit, because a parser stack overflow ABORTS the process instead of
//!   unwinding into a refusal, so this bound may not depend on the cheap scan above
//!   having read a caller's text correctly;
//! * the CONSTRUCTS whose single interpreter step has no bound the engine can
//!   interrupt ([`UNBOUNDED_CONSTRUCT_NODES`]), refused on the engine's own parse
//!   before anything is evaluated;
//! * the DEPTH and the WALL CLOCK of the evaluation ([`MAX_EVALUATION_DEPTH`], the
//!   caller's own millisecond allowance), and the MEMORY its values take
//!   ([`MAX_EVALUATION_ARENA_BYTES`], installed on the arena so `bumpalo` refuses an
//!   over-ceiling layout before asking the system for it);
//! * the SIZE of the value it produced, decided on the engine's own value BEFORE the
//!   JSON text of it is built, because that text is allocated on the global heap where
//!   the arena's limit does not reach and a value aliased into many places is small in
//!   the arena and large as a document;
//! * how MANY evaluations may be alive in this process at once
//!   ([`MAX_LIVE_EVALUATIONS`]), so a flood of expression-bearing requests cannot
//!   consume the host's threads and cores.
//!
//! A refusal is a [`Refused`], which the calling service reports in its own modelled
//! terms. Nothing here panics on a caller's expression: the parse and the evaluation
//! run inside [`::std::panic::catch_unwind`], because the arena's refusal and the
//! engine's own `unreachable!()` on some malformed expressions are unwinding panics,
//! and a caller's expression must not be able to end the process.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

/// The most bytes of expression TEXT this module evaluates.
///
/// Emulator bound: the text comes out of a document bounded only at the model's own
/// ceiling (a state machine definition is 1 MiB, a mapping template 350 KB), and it is
/// what the parse's cost and recursion are proportional to. An expression any service
/// here evaluates is a field of a definition, so it is short; this is far above every
/// one of them and is what [`EVALUATION_STACK_BYTES`] is sized against.
pub const MAX_EXPRESSION_BYTES: usize = 1024;

/// The most openings of a bracket, a brace or a parenthesis one expression may carry.
///
/// An EARLY, CHEAP refusal, not the safety bound: every nested construct costs the
/// recursive-descent parse a frame, and this refuses the ordinary deep-nesting case
/// before the parse touches the stack. It counts OPENINGS rather than depth, so no
/// reading of what is inside a string literal, a backtick-quoted name or a regular
/// expression is needed for the count to bound the depth — what makes the parse safe
/// whatever the text is, is the sized stack it runs on.
pub const MAX_EXPRESSION_GROUPINGS: usize = 128;

/// The most bytes of the CONTEXT DOCUMENT an expression reads.
///
/// Emulator bound: the engine parses the document's text as recursively as it parses
/// the expression, and holds the whole of it in its arena, so the document is bounded
/// in bytes as well as in nesting ([`MAX_EVALUATION_DEPTH`], checked iteratively over
/// the value the caller hands in). It is the ceiling a state's own document carries in
/// the Amazon States Language, which is the larger of this emulator's two consumers'
/// per-document ceilings.
pub const MAX_INPUT_BYTES: usize = 256 * 1024;

/// The most levels one evaluation recurses, handed to the engine's own depth box.
pub const MAX_EVALUATION_DEPTH: usize = 128;

/// The most bytes one evaluation allocates into its arena.
///
/// Installed with `set_allocation_limit`, so `bumpalo` compares every chunk it is
/// about to request against it and REFUSES before asking the system for the memory:
/// the clamp precedes the allocation. A refusal unwinds (it is `bumpalo`'s own panic,
/// not `std::alloc::handle_alloc_error`), so it frees the arena and becomes a
/// [`Refused`].
pub const MAX_EVALUATION_ARENA_BYTES: usize = 8 * 1024 * 1024;

/// The stack the evaluation thread is given.
///
/// Sized so that NOTHING the ceilings above admit can overflow it, with nothing about
/// a caller's text needing to be understood first — a parser stack overflow ABORTS the
/// process instead of unwinding into a refusal, so this bound may not depend on a scan
/// of caller-supplied text being accurate. The frames it has to hold are those of the
/// engine's two recursive parses and of the evaluation:
///
/// * the EXPRESSION parse, one frame per BYTE of [`MAX_EXPRESSION_BYTES`] in the worst
///   case — one frame costs this parser on the order of 16 KiB in the test profile, so
///   that is on the order of 16 MiB;
/// * the CONTEXT DOCUMENT parse, one frame per LEVEL it nests, bounded at
///   [`MAX_EVALUATION_DEPTH`] before the engine sees it — three orders of magnitude
///   fewer frames than the expression's worst case, however many BYTES the document
///   carries;
/// * the EVALUATION itself, at most [`MAX_EVALUATION_DEPTH`] frames.
///
/// This is a 2x margin over their sum. It is RESERVED address space, committed only as
/// a parse touches it, and at most [`MAX_LIVE_EVALUATIONS`] of these threads exist at
/// once.
const EVALUATION_STACK_BYTES: usize = 32 * 1024 * 1024;

/// The most evaluations alive in this process at once.
///
/// Each one holds a thread and a core for as long as it runs, so this is what keeps a
/// flood of expression-bearing requests from taking the host's threads: an evaluation
/// that cannot be admitted is refused rather than queued, because the caller is
/// already inside a request with a deadline of its own.
pub const MAX_LIVE_EVALUATIONS: usize = 32;

/// How many evaluation threads are alive right now, in this process.
static LIVE_EVALUATIONS: AtomicUsize = AtomicUsize::new(0);

/// The containment above needs unwinding panics: `catch_unwind` cannot intercept an
/// aborting panic, so a profile built with `panic = "abort"` would turn the arena's
/// refusal — and the engine's own `unreachable!()` on some malformed expressions —
/// into the death of the serving process instead of a refusal.
#[cfg(panic = "abort")]
compile_error!(
    "evaluating a caller's JSONata expression needs catch_unwind, so this crate requires \
     panic = \"unwind\" (the default)"
);

/// The constructs this module will not evaluate, as the node each appears under in the
/// engine's parsed expression, with the name a refusal gives it.
///
/// Each is ONE interpreter step with no bound that can be placed ahead of it: the
/// engine checks its wall clock BETWEEN steps and its memory clamp is on the arena, so
/// a step that cannot be bounded cannot be interrupted either. The reasons, measured
/// against this pinned version of the engine:
///
/// * a regular-expression literal is the only way an expression can obtain a regex,
///   and the matcher behind it backtracks with no step budget, so one match of a
///   caller-written pattern can run for an unbounded time inside a single step
///   (`$match`, and `$split`/`$contains`/`$replace` given a PATTERN, all need a
///   literal, so this covers them while the same functions over plain strings are
///   evaluated);
/// * `$distinct` compares each member against the ones before it, so its one step is
///   QUADRATIC in a member count the arena ceiling alone does not bound;
/// * `$pad` builds its result on the GLOBAL heap, sized by a NUMBER written in the
///   expression rather than by any value it was given, so neither the arena's limit
///   nor the answer ceiling precedes it and the system allocator's refusal ABORTS
///   where the arena's unwinds.
///
/// EVERYTHING ELSE IS EVALUATED: the growth of every other function is charged to the
/// arena, which refuses an over-ceiling layout before asking the system for it, and
/// the ones that take a function call it once per member, which is a step the engine's
/// own clock can interrupt.
///
/// The decision is read off the ENGINE's own parse, never off the expression's text:
/// whether a `/` opens a regular expression or divides, and whether `$pad` is the
/// function or part of a longer name, are questions only the engine's lexer can
/// answer. Its `Debug` rendering escapes the quotes inside a string literal, so an
/// expression carrying this text in a literal is evaluated normally, while a literal
/// holding the quote-free opening and an expression whose own variable is NAMED after
/// a refused function are over-refused — the safe direction.
const UNBOUNDED_CONSTRUCT_NODES: &[(&str, &str)] = &[
    ("Regex(", "a regular-expression literal"),
    (r#"Var("distinct")"#, "$distinct"),
    (r#"Var("pad")"#, "$pad"),
];

/// Why an expression was not evaluated.
///
/// The classes are the ones a calling service reports differently: a value larger
/// than the allowance it was given, an expression the engine EVALUATED and that threw,
/// and everything else.
#[derive(Debug, Clone)]
pub enum Refused {
    /// The value the expression produced is larger than the allowance.
    TooLarge(String),
    /// The engine evaluated the expression and it THREW, carrying the engine's own
    /// account of the failure: the language's error code for it, and the message that
    /// code carries ([`thrown_account`]).
    ///
    /// It is held apart from [`Refused::NotEvaluated`] because it is the ONE class a
    /// calling service reports the expression's own failure from: everything else is a
    /// text this module refuses or a ceiling it holds an evaluation to, which is about
    /// the request rather than about what the expression computed.
    Threw(String),
    /// The expression was not evaluated: a text this module refuses, or a ceiling the
    /// evaluation reached.
    NotEvaluated(String),
}

/// One expression to evaluate.
#[derive(Debug, Clone, Copy)]
pub struct Expression<'a> {
    /// The expression itself, without whatever delimiters the calling service's own
    /// language wraps one in.
    pub text: &'a str,
    /// The document the expression reads as its context item (`$`), as a value this
    /// process holds rather than as text: its size and its nesting are checked here
    /// ([`MAX_INPUT_BYTES`], [`MAX_EVALUATION_DEPTH`]) before the engine is handed the
    /// text of it.
    pub input: &'a Value,
    /// The variables bound for it, each read under `$<name>`.
    pub variables: &'a [(&'a str, &'a Value)],
    /// The most milliseconds this evaluation may run for: the calling service's own,
    /// so an operation with a deadline can hand over what is left of it.
    pub allowed_ms: usize,
    /// The most bytes the value it produces may serialize to.
    pub allowance: usize,
}

/// Evaluate `expression`, and report the value it produced beside the bytes that value
/// serializes to.
///
/// `Ok(None)` is an expression that produced NOTHING — the language's own `undefined`
/// — which is not a document at all, so a field whose expression produces one carries
/// no value.
///
/// Every ceiling the module header lists is applied here, each before the work it
/// bounds: the text, the groupings, the live-evaluation count and the thread's stack
/// before the parse; the refused constructs on the parse; the depth, the clock and the
/// arena during the evaluation; and the size of the value before its JSON text exists.
pub fn evaluate(expression: &Expression<'_>) -> Result<Option<(Value, usize)>, Refused> {
    if expression.text.len() > MAX_EXPRESSION_BYTES {
        return Err(Refused::NotEvaluated(format!(
            "an expression of {} bytes is longer than the {MAX_EXPRESSION_BYTES} this emulator \
             evaluates",
            expression.text.len()
        )));
    }
    let groupings = expression
        .text
        .bytes()
        .filter(|byte| matches!(byte, b'(' | b'[' | b'{'))
        .count();
    if groupings > MAX_EXPRESSION_GROUPINGS {
        return Err(Refused::NotEvaluated(format!(
            "an expression carrying {groupings} brackets nests more deeply than the \
             {MAX_EXPRESSION_GROUPINGS} levels this emulator evaluates"
        )));
    }
    // The CONTEXT DOCUMENT is a caller's too, and the engine parses its text as
    // recursively as it parses the expression, so both of its dimensions are bounded
    // before the engine is handed it: how deeply it nests, which is what that parse
    // costs frames for, and how many bytes it carries.
    let nesting = nesting(expression.input);
    if nesting > MAX_EVALUATION_DEPTH {
        return Err(Refused::NotEvaluated(format!(
            "the document an expression reads nests {nesting} levels, deeper than the \
             {MAX_EVALUATION_DEPTH} this emulator evaluates over"
        )));
    }
    let input = expression.input.to_string();
    if input.len() > MAX_INPUT_BYTES {
        return Err(Refused::NotEvaluated(format!(
            "the document an expression reads is {} bytes, more than the {MAX_INPUT_BYTES} this \
             emulator evaluates over",
            input.len()
        )));
    }
    // The parse is recursive descent over a caller's text, so it runs on a thread with
    // a stack sized for it, and the evaluation stays on that thread: ONE thread per
    // expression, counted against the live-evaluation ceiling.
    let _live = LiveEvaluation::start().ok_or_else(|| {
        Refused::NotEvaluated(format!(
            "more than the {MAX_LIVE_EVALUATIONS} expressions this emulator evaluates at once are \
             already being evaluated, so this one was not"
        ))
    })?;
    let answered = ::std::thread::scope(|scope| {
        let spawned = ::std::thread::Builder::new()
            .stack_size(EVALUATION_STACK_BYTES)
            .spawn_scoped(scope, || contained(expression, &input));
        match spawned {
            Ok(thread) => thread.join().map_err(|_| {
                Refused::NotEvaluated(
                    "evaluating the expression ended in a way this emulator could not report, so \
                     nothing was evaluated for it"
                        .to_string(),
                )
            }),
            // A host that will not give the thread its stack: the expression was not
            // evaluated, which is what the caller is told.
            Err(_) => Err(Refused::NotEvaluated(
                "this emulator could not start the thread an expression is evaluated on, so \
                 nothing was evaluated for it"
                    .to_string(),
            )),
        }
    })?;
    answered
}

/// How deeply `value` nests, counted ITERATIVELY — over an explicit stack rather than
/// by recursion — so reading a caller's document is not itself bounded by this
/// thread's own stack.
fn nesting(value: &Value) -> usize {
    let mut deepest = 1;
    let mut pending: Vec<(&Value, usize)> = vec![(value, 1)];
    while let Some((value, level)) = pending.pop() {
        deepest = deepest.max(level);
        // Nothing deeper than the ceiling needs counting: the caller refuses at it, so
        // the walk stops as soon as it is passed.
        if level > MAX_EVALUATION_DEPTH {
            return level;
        }
        match value {
            Value::Array(items) => pending.extend(items.iter().map(|item| (item, level + 1))),
            Value::Object(members) => {
                pending.extend(members.values().map(|member| (member, level + 1)));
            }
            _ => {}
        }
    }
    deepest
}

/// The parse and the evaluation themselves, on the thread [`evaluate`] sized for them.
///
/// `input` is the TEXT of the context document, rendered from the value `evaluate`
/// bounded, so what the engine parses here is a document whose size and nesting are
/// already inside the ceilings this thread's stack was sized for.
///
/// Every failure is the caller's expression failing, so each is reported rather than
/// left to unwind: the whole of it runs inside [`::std::panic::catch_unwind`], since
/// the arena's refusal of an over-ceiling allocation and the engine's own
/// `unreachable!()` on some malformed expressions are panics.
fn contained(
    expression: &Expression<'_>,
    input: &str,
) -> Result<Option<(Value, usize)>, Refused> {
    let text = expression.text;
    let evaluated = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
        let arena = ::bumpalo::Bump::new();
        arena.set_allocation_limit(Some(MAX_EVALUATION_ARENA_BYTES));
        let parsed = ::jsonata_rs::JsonAta::new(text, &arena).map_err(|why| {
            Refused::NotEvaluated(format!("'{text}' is not a valid JSONata expression: {why}"))
        })?;
        // Read off the ENGINE's own parse, before anything is evaluated: a step this
        // emulator cannot bound cannot be interrupted either.
        if let Some(construct) = unbounded_construct(&parsed.ast()) {
            return Err(Refused::NotEvaluated(format!(
                "'{text}' uses {construct}, whose cost this emulator cannot bound, so it was not \
                 evaluated"
            )));
        }
        for (name, value) in expression.variables {
            let bound = arena_value(&arena, value, MAX_EVALUATION_DEPTH)?;
            parsed.assign_var(name, bound);
        }
        let answer = parsed
            .evaluate_timeboxed(
                Some(input),
                Some(MAX_EVALUATION_DEPTH),
                Some(expression.allowed_ms),
            )
            .map_err(|why| evaluation_failure(text, &why, expression.allowed_ms))?;
        if answer.is_undefined() {
            return Ok(None);
        }
        // The size is decided on the ENGINE's value, before the JSON text of it is
        // built: that text is allocated on the global heap, which the arena's limit
        // does not govern, so a value aliased into many places would otherwise be
        // measured only once it already existed.
        let mut allowance = expression.allowance;
        if !fits(answer, &mut allowance, MAX_EVALUATION_DEPTH) {
            return Err(Refused::TooLarge(format!(
                "'{text}' produces more than the {} bytes left of what this may build",
                expression.allowance
            )));
        }
        let built = expression.allowance.saturating_sub(allowance);
        let serialized = answer.serialize(false);
        ::serde_json::from_str::<Value>(&serialized)
            .map(|value| Some((value, built.max(serialized.len()))))
            .map_err(|_| {
                Refused::NotEvaluated(format!(
                    "'{text}' produces a value that is not a JSON document"
                ))
            })
    }));
    match evaluated {
        Ok(answered) => answered,
        Err(_) => Err(Refused::NotEvaluated(format!(
            "evaluating '{text}' asked for more than the {MAX_EVALUATION_ARENA_BYTES} bytes this \
             emulator evaluates one expression in, or could not be parsed at all"
        ))),
    }
}

/// One live evaluation thread, counted in [`LIVE_EVALUATIONS`] for as long as it runs.
#[derive(Debug)]
struct LiveEvaluation;

impl LiveEvaluation {
    /// Count one more live evaluation, or `None` when [`MAX_LIVE_EVALUATIONS`] already
    /// are.
    fn start() -> Option<Self> {
        let mut live = LIVE_EVALUATIONS.load(Ordering::Acquire);
        loop {
            if live >= MAX_LIVE_EVALUATIONS {
                return None;
            }
            match LIVE_EVALUATIONS.compare_exchange_weak(
                live,
                live + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self),
                Err(seen) => live = seen,
            }
        }
    }
}

impl Drop for LiveEvaluation {
    fn drop(&mut self) {
        LIVE_EVALUATIONS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The construct in a PARSED expression whose one interpreter step this emulator
/// cannot bound, named for the refusal, or `None` when the expression uses none. The
/// list, and why each entry is on it, is [`UNBOUNDED_CONSTRUCT_NODES`].
fn unbounded_construct(parsed: &impl ::std::fmt::Debug) -> Option<&'static str> {
    let rendered = format!("{parsed:?}");
    UNBOUNDED_CONSTRUCT_NODES
        .iter()
        .find(|(node, _)| rendered.contains(node))
        .map(|(_, name)| *name)
}

/// How one evaluation's own failure reads: the engine's account of it, with the two
/// ceilings it reports by name said in the terms the caller can act on.
fn evaluation_failure(text: &str, why: &::jsonata_rs::Error, allowed_ms: usize) -> Refused {
    match why {
        ::jsonata_rs::Error::U1001Timeout => Refused::NotEvaluated(format!(
            "'{text}' did not finish evaluating within the {allowed_ms} ms this emulator allowed \
             it"
        )),
        ::jsonata_rs::Error::U1001StackOverflow => Refused::NotEvaluated(format!(
            "'{text}' nested deeper than the {MAX_EVALUATION_DEPTH} evaluations this emulator \
             allows one expression"
        )),
        thrown => Refused::Threw(thrown_account(thrown)),
    }
}

/// The engine's own account of an expression that THREW, as the language states one:
/// the error CODE the failure carries, then the message for it.
///
/// The engine renders both in one line, with the offset into the expression text at
/// which it stopped and with the token the message quotes in the backquotes this
/// implementation writes it in. The OFFSET is a position in a caller's text rather than
/// part of the account, and the quoting of a token is the implementation's rather than
/// the language's, so the account is normalised here: the code and the message, with
/// every quoted token in the double quotes the language's own messages carry.
fn thrown_account(why: &::jsonata_rs::Error) -> String {
    let rendered = why.to_string().replace('`', "\"");
    let Some((code, message)) = rendered.split_once(": ") else {
        return rendered;
    };
    match code.split_once(" @ ") {
        Some((code, _offset)) => format!("{code}: {message}"),
        None => rendered,
    }
}

/// Whether the JSON text of `value` fits in what is left of `allowance`, spending it
/// as the walk goes.
///
/// The charge is what the COMPACT rendering writes — the quotes, the separators, the
/// digits of a number, the escapes of a string — so the allowance is spent the way the
/// document will be written and the text is never built when it would not fit. The
/// walk is bounded by the allowance it spends (every node costs at least one byte) and
/// refuses past `depth` levels rather than recursing further.
fn fits<'a>(value: &'a ::jsonata_rs::Value<'a>, allowance: &mut usize, depth: usize) -> bool {
    /// Spend `bytes` of the allowance.
    fn charge(allowance: &mut usize, bytes: usize) -> bool {
        match allowance.checked_sub(bytes) {
            Some(left) => {
                *allowance = left;
                true
            }
            None => false,
        }
    }

    /// What the serializer writes for `text`, quotes included: a byte it escapes with
    /// a `\` costs two, a control byte it writes as `\u00xx` costs six.
    fn quoted_len(text: &str) -> usize {
        2 + text
            .bytes()
            .map(|byte| match byte {
                b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 0x08 | 0x0c => 2,
                0x00..=0x1f => 6,
                _ => 1,
            })
            .sum::<usize>()
    }

    /// What the serializer writes for a number: its own digits, not the widest
    /// number's. A non-finite one is written as `null`.
    fn number_len(number: f64) -> usize {
        if !number.is_finite() {
            return 4;
        }
        let magnitude = number.abs();
        let sign = usize::from(number.is_sign_negative());
        if number.fract() == 0.0 && magnitude < 1e15 {
            let digits = if magnitude < 1.0 {
                1
            } else {
                magnitude.log10().floor() as usize + 1
            };
            return sign + digits;
        }
        // A fraction or an exponent: the widest the 15-significant-digit rendering
        // gets (sign, leading digit, point, 14 digits, `e`, exponent sign, three
        // exponent digits).
        sign + 24
    }

    if depth == 0 {
        return false;
    }
    // The one value the serializer writes nothing for.
    if value.is_undefined() {
        return true;
    }
    if value.is_null() {
        return charge(allowance, 4);
    }
    if value.is_bool() {
        return charge(allowance, 5);
    }
    if value.is_number() {
        return charge(allowance, number_len(value.as_f64()));
    }
    if value.is_string() {
        return charge(allowance, quoted_len(&value.as_str()));
    }
    if value.is_array() {
        if !charge(allowance, 2) {
            return false;
        }
        for member in value.members() {
            // The comma before every member but the first, charged for each: one byte
            // either way.
            if !charge(allowance, 1) || !fits(member, allowance, depth - 1) {
                return false;
            }
        }
        return true;
    }
    if value.is_object() {
        if !charge(allowance, 2) {
            return false;
        }
        for (key, member) in value.entries() {
            if !charge(allowance, quoted_len(key) + 2) || !fits(member, allowance, depth - 1) {
                return false;
            }
        }
        return true;
    }
    // A function or a regular-expression value, which has no JSON rendering: charged a
    // fixed cost, since the serialization below is what reports it as no document.
    charge(allowance, 2)
}

/// `value` as the engine's own value, allocated in `arena`.
///
/// `depth` bounds the recursion: the document comes from a caller, and this walk is
/// recursive, so a document nested past it is refused rather than followed.
fn arena_value<'a>(
    arena: &'a ::bumpalo::Bump,
    value: &Value,
    depth: usize,
) -> Result<&'a ::jsonata_rs::Value<'a>, Refused> {
    let Some(depth) = depth.checked_sub(1) else {
        return Err(Refused::NotEvaluated(format!(
            "a value an expression reads nests deeper than the {MAX_EVALUATION_DEPTH} levels this \
             emulator evaluates"
        )));
    };
    Ok(match value {
        Value::Null => ::jsonata_rs::Value::null(arena),
        Value::Bool(yes) => ::jsonata_rs::Value::bool(*yes),
        // A JSON number the language reads is a number: JSONata's own numeric type is
        // the double every JSON number is read as.
        Value::Number(number) => {
            ::jsonata_rs::Value::number(arena, number.as_f64().unwrap_or(f64::NAN))
        }
        Value::String(text) => ::jsonata_rs::Value::string(arena, text),
        Value::Array(items) => {
            let array = ::jsonata_rs::Value::array(arena, ::jsonata_rs::ArrayFlags::empty());
            for item in items {
                array.push(arena_value(arena, item, depth)?);
            }
            array
        }
        Value::Object(members) => {
            let object = ::jsonata_rs::Value::object(arena);
            for (member, child) in members {
                object.insert(member, arena_value(arena, child, depth)?);
            }
            object
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    /// One expression over `input`, with the ceilings this module's callers hand in.
    fn over(text: &str, input: &Value) -> Result<Option<(Value, usize)>, Refused> {
        evaluate(&Expression {
            text,
            input,
            variables: &[],
            allowed_ms: 250,
            allowance: 1024,
        })
    }

    /// The name of the class a refusal belongs to, for a test's own message.
    fn class(refused: &Refused) -> &'static str {
        match refused {
            Refused::TooLarge(_) => "TooLarge",
            Refused::Threw(_) => "Threw",
            Refused::NotEvaluated(_) => "NotEvaluated",
        }
    }

    #[test]
    fn an_expression_is_evaluated_over_its_document_and_its_variables() {
        let input = json!({ "n": 20 });
        let total = json!(2);
        let answered = evaluate(&Expression {
            text: "$states.n + $extra",
            input: &input,
            variables: &[("states", &input), ("extra", &total)],
            allowed_ms: 250,
            allowance: 1024,
        })
        .expect("the expression is evaluated");
        let (value, bytes) = answered.expect("it produces a value");
        assert_eq!(value, json!(22), "the expression computes over both, got {value}");
        assert!(
            bytes >= 2,
            "the bytes reported are what the value serializes to, got {bytes}"
        );
    }

    #[test]
    fn an_expression_that_produces_nothing_produces_no_value() {
        let answered = over("$absent", &json!({ "present": 1 })).expect("it is evaluated");
        assert!(
            answered.is_none(),
            "a field an expression resolves nothing for carries no value, got {answered:?}"
        );
    }

    /// Every ceiling on what a caller may hand in, over a table that varies WHICH one
    /// the input reaches.
    #[test]
    fn every_ceiling_on_a_callers_expression_refuses_it() {
        // (case, the expression, the document it reads, the allowance, the class)
        let long = "1 + ".repeat(MAX_EXPRESSION_BYTES) + "1";
        let brackets = format!("[{}]", "[".repeat(MAX_EXPRESSION_GROUPINGS));
        let deep = {
            let mut document = json!(1);
            for _ in 0..=MAX_EVALUATION_DEPTH {
                document = Value::Array(vec![document]);
            }
            document
        };
        let wide = Value::String("x".repeat(MAX_INPUT_BYTES));
        let cases: Vec<(&str, &str, Value, usize, &str)> = vec![
            (
                "an expression longer than the text this evaluates",
                long.as_str(),
                json!({}),
                1024,
                "NotEvaluated",
            ),
            (
                "an expression carrying more brackets than it nests levels",
                brackets.as_str(),
                json!({}),
                1024,
                "NotEvaluated",
            ),
            (
                "a document nested deeper than this evaluates over",
                "$",
                deep,
                1024,
                "NotEvaluated",
            ),
            (
                "a document larger than this evaluates over",
                "$",
                wide,
                usize::MAX,
                "NotEvaluated",
            ),
            (
                "an expression carrying a regular-expression literal",
                "$match('xxx', /x+/)",
                json!({}),
                1024,
                "NotEvaluated",
            ),
            (
                "an expression carrying $distinct",
                "$distinct([1,2,2])",
                json!({}),
                1024,
                "NotEvaluated",
            ),
            (
                "an expression carrying $pad",
                "$pad('x', 8)",
                json!({}),
                1024,
                "NotEvaluated",
            ),
            (
                "a value larger than the allowance it was given",
                "$join([1..100].('xxxxxxxx'), '')",
                json!({}),
                64,
                "TooLarge",
            ),
            (
                "an expression that is not the language at all",
                "1 +",
                json!({}),
                1024,
                "NotEvaluated",
            ),
            (
                "an expression the engine evaluates and that throws",
                "'text' + 1",
                json!({}),
                1024,
                "Threw",
            ),
        ];

        for (case, text, input, allowance, expected) in cases {
            let answered = evaluate(&Expression {
                text,
                input: &input,
                variables: &[],
                allowed_ms: 250,
                allowance,
            });
            let refused = answered
                .as_ref()
                .err()
                .unwrap_or_else(|| panic!("case {case:?}: expected a refusal, got {answered:?}"));
            assert_eq!(
                class(refused),
                expected,
                "case {case:?}: expected a {expected} refusal, got {refused:?}"
            );
        }
    }

    /// How deeply a document nests is counted without recursing, so the count itself
    /// cannot overflow this thread: a document past the ceiling answers a depth past
    /// it rather than walking the whole of it.
    #[test]
    fn the_nesting_of_a_document_is_counted_iteratively() {
        // (case, the document, the depth it nests)
        let cases: Vec<(&str, Value, usize)> = vec![
            ("a scalar", json!(1), 1),
            ("an empty object", json!({}), 1),
            ("one level of members", json!({ "a": 1, "b": 2 }), 2),
            ("an array of objects", json!([{ "a": 1 }]), 3),
            ("the deepest member decides", json!({ "a": 1, "b": { "c": [1] } }), 4),
        ];
        for (case, document, expected) in cases {
            assert_eq!(
                nesting(&document),
                expected,
                "case {case:?}: expected {expected} levels for {document}"
            );
        }
        let mut deep = json!(1);
        for _ in 0..(MAX_EVALUATION_DEPTH + 10) {
            deep = Value::Array(vec![deep]);
        }
        assert!(
            nesting(&deep) > MAX_EVALUATION_DEPTH,
            "a document past the ceiling is reported past it"
        );
    }

    /// The size of a value is charged the way its JSON text is written, so the text is
    /// never built for a value that would not fit.
    #[test]
    fn a_values_size_is_charged_the_way_its_text_is_written() {
        // (case, the expression, what it must cost at most, what it must cost at least)
        let cases: [(&str, &str, usize, usize); 4] = [
            ("a number", "7", 1, 1),
            ("a string with an escape", r#""a\"b""#, 6, 6),
            ("an array of three numbers", "[1,2,3]", 9, 5),
            ("an object of one member", r#"{ "a": 1 }"#, 10, 7),
        ];
        for (case, text, most, least) in cases {
            let answered = over(text, &json!({})).expect("it is evaluated");
            let (value, bytes) = answered.expect("it produces a value");
            let written = value.to_string().len();
            assert!(
                bytes >= written && bytes >= least && bytes <= most,
                "case {case:?}: expected a charge between {least} and {most} covering the \
                 {written} bytes of {value}, got {bytes}"
            );
        }
    }
}
