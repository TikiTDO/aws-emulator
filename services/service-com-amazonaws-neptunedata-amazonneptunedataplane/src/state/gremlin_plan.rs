// SPDX-License-Identifier: CC0-1.0

//! Reading a Gremlin traversal, and the explain report this engine answers with for it.
//!
//! The Gremlin explain endpoint reports how the engine would evaluate the traversal that
//! was submitted, so every line of the report below is derived from THAT traversal:
//!
//! 1. the query string, as it was submitted;
//! 2. the steps the query is made of, printed the way Apache TinkerPop prints a compiled
//!    traversal — a step's class name and the arguments it was compiled with;
//! 3. the plan: the quad patterns the pattern-matching part of the traversal matches,
//!    plus the steps left for the step iterators.
//!
//! The patterns come from the property-graph data model, where every statement about an
//! element is a quad: the element, the property key or edge label, the value or the
//! element reached, and the graph the statement belongs to (`<~>`, the default graph,
//! for a property statement; the edge's own id for an edge statement).
//!
//! Nothing here EVALUATES the traversal: an explain reports the plan for a query, not its
//! results, so this module reads no graph data and needs none.

use std::fmt::Write as _;

use ::logos::Logos;

use crate::ops::types::{MalformedQueryException, QueryLimitException, QueryTooLargeException};

/// The longest query this module reads, in bytes. A request past it is refused as too
/// large rather than read: the bound is what keeps one request's work — and the report
/// it produces, whose length is a bounded multiple of the query's — bounded too.
pub const MAX_QUERY_LENGTH: usize = 65_536;

/// The deepest nesting of argument lists this module reads. A query nested deeper is
/// refused instead of read, so one request cannot exhaust the stack the recursive descent
/// below runs on.
pub const MAX_ARGUMENT_DEPTH: usize = 32;

/// The traversal source a Gremlin query starts from.
pub const GRAPH_TRAVERSAL_SOURCE: &str = "g";

/// The predicate position of a statement about an element's label, which is how the
/// elements of a label are found and how `hasLabel` and `label()` are matched.
const LABEL_PREDICATE: &str = "<~label>";

/// The graph position of a property statement: the default graph.
const DEFAULT_GRAPH: &str = "<~>";

/// The step that labels the step it follows, so that a later step can select it.
const STEP_LABEL: &str = "as";

/// The traversal ACTIONS: they ask a finished traversal for its answer rather than adding
/// a step to it, so a plan carries none of them.
///
/// Source: the terminal methods of Apache TinkerPop's `Traversal` (the TinkerPop version
/// this engine reports in its status, [`super::GREMLIN_LANGUAGE_VERSION`]).
const TRAVERSAL_ACTIONS: &[&str] = &[
    "next",
    "tryNext",
    "hasNext",
    "toList",
    "toSet",
    "toBulkSet",
    "toStream",
    "fill",
    "iterate",
    "explain",
    "profile",
];

/// The MODULATORS: each configures the step it follows instead of being a step of its
/// own, so each is printed on that step rather than beside it.
///
/// Source: the modulating methods of Apache TinkerPop's `GraphTraversal`.
const MODULATORS: &[&str] = &["by", "option", "times", "until", "emit", "from", "to"];

/// The predicates a `has`-family step may carry in place of a value.
///
/// Source: the predicate methods of Apache TinkerPop's `P` and `TextP`.
const PREDICATES: &[&str] = &[
    "eq",
    "neq",
    "lt",
    "lte",
    "gt",
    "gte",
    "inside",
    "outside",
    "between",
    "within",
    "without",
    "not",
    "startingWith",
    "endingWith",
    "containing",
    "notStartingWith",
    "notEndingWith",
    "notContaining",
];

/// The banner the explain report opens with.
const REPORT_RULE: &str = "*******************************************************";

/// Whether `name` configures the traversal SOURCE (`withSideEffect`, `withStrategies`,
/// …) rather than adding a step to the traversal, so no step is printed for it.
fn is_source_configuration(name: &str) -> bool {
    name.starts_with("with")
}

/// The lexical tokens of a Gremlin traversal in the string form the Gremlin endpoints
/// accept: a dotted chain of named steps, each optionally applied to a parenthesised
/// argument list.
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
enum Token {
    #[token(".")]
    Dot,
    #[token(",")]
    Comma,
    #[token("(")]
    OpenParen,
    #[token(")")]
    CloseParen,
    #[token("[")]
    OpenBracket,
    #[token("]")]
    CloseBracket,
    /// A name: the traversal source (`g`), a step (`V`), the anonymous traversal (`__`),
    /// an enum token (`asc`) or a keyword (`true`).
    #[regex(r"[A-Za-z_$][A-Za-z0-9_$]*", |lexed| lexed.slice().to_owned())]
    Name(String),
    /// A quoted string, in either quotation the Gremlin string form accepts, with its
    /// quotes removed and its escapes resolved.
    #[regex(r"'([^'\\]|\\.)*'", |lexed| unquoted(lexed.slice()))]
    #[regex(r#""([^"\\]|\\.)*""#, |lexed| unquoted(lexed.slice()))]
    Text(String),
    /// A numeric literal, as written apart from a type suffix (`10L`).
    #[regex(r"-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?[LlDdFfGgIi]?", |lexed| number(lexed.slice()))]
    Number(String),
}

/// A quoted literal's text: the quotes removed and the escapes resolved.
fn unquoted(literal: &str) -> String {
    let inner = literal
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .or_else(|| {
            literal
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
        })
        // Unreachable from the token regexes above, which match both quotes; an
        // unquotable literal is read as itself rather than refused with a panic.
        .unwrap_or(literal);
    let mut text = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            text.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => text.push('\n'),
            Some('r') => text.push('\r'),
            Some('t') => text.push('\t'),
            Some('b') => text.push('\u{8}'),
            Some('f') => text.push('\u{c}'),
            Some(escaped) => text.push(escaped),
            None => text.push('\\'),
        }
    }
    text
}

/// A numeric literal's value, as written minus the type suffix the string form allows.
fn number(literal: &str) -> String {
    literal
        .trim_end_matches(|character: char| {
            matches!(
                character,
                'L' | 'l' | 'D' | 'd' | 'F' | 'f' | 'G' | 'g' | 'I' | 'i'
            )
        })
        .to_owned()
}

/// One segment of a dotted Gremlin expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// The segment's name, as written.
    pub name: String,
    /// The arguments it was applied to, or `None` when the segment carries no argument
    /// list at all (`g`, or the `desc` of `Order.desc`), so a step written without
    /// parentheses is not reported as one applied to nothing.
    pub arguments: Option<Vec<Argument>>,
}

impl Call {
    /// The arguments this segment was applied to — empty both for `step()` and for a
    /// segment with no argument list.
    pub fn arguments(&self) -> &[Argument] {
        self.arguments.as_deref().unwrap_or_default()
    }
}

/// One argument of a Gremlin step.
#[derive(Debug, Clone, PartialEq)]
pub enum Argument {
    /// A quoted string.
    Text(String),
    /// A numeric literal.
    Number(String),
    Boolean(bool),
    Null,
    /// A dotted expression: a predicate (`gt(30)`), an anonymous traversal
    /// (`__.out('route')`), an enum token (`Order.desc`) or a bare name.
    Expression(Vec<Call>),
    /// A bracketed list (`['a','b']`).
    List(Vec<Argument>),
}

/// A Gremlin traversal this engine has read: the source it starts from and the steps
/// applied to it, in the order they were written.
#[derive(Debug, Clone, PartialEq)]
pub struct Traversal {
    /// The traversal source, which Gremlin spells `g`.
    pub source: String,
    pub steps: Vec<Call>,
}

/// Why a submitted query is not one this engine can plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryRejection {
    /// The query is not a syntactically valid Gremlin traversal.
    Malformed(String),
    /// The query is longer than this engine reads.
    TooLarge(String),
    /// The query nests deeper than this engine reads.
    TooComplex(String),
}

impl QueryRejection {
    /// Why the query was refused, for the message the refusal carries.
    pub fn reason(&self) -> &str {
        match self {
            Self::Malformed(reason) | Self::TooLarge(reason) | Self::TooComplex(reason) => reason,
        }
    }
}

/// The `MalformedQueryException` payload for a query this engine could not read as a
/// Gremlin traversal. `reason` says what was wrong and where, so the caller can fix the
/// query from the message alone, and `code` carries the HTTP status the member's own
/// documentation says it carries.
pub fn malformed_gremlin_query(reason: &str) -> MalformedQueryException {
    MalformedQueryException {
        detailed_message: format!("Failed to interpret Gremlin query: {reason}"),
        request_id: super::new_request_id(),
        code: "400".to_string(),
    }
}

/// The `QueryTooLargeException` payload for a query longer than this engine reads.
pub fn gremlin_query_too_large(reason: &str) -> QueryTooLargeException {
    QueryTooLargeException {
        detailed_message: format!("Gremlin query is too large: {reason}"),
        request_id: super::new_request_id(),
        code: "400".to_string(),
    }
}

/// The `QueryLimitException` payload for a query whose nesting exceeds what this engine
/// reads — the system limit this query is past.
pub fn gremlin_query_too_complex(reason: &str) -> QueryLimitException {
    QueryLimitException {
        detailed_message: format!("Gremlin query exceeds a system limit: {reason}"),
        request_id: super::new_request_id(),
        code: "400".to_string(),
    }
}

/// Read `query` as a Gremlin traversal.
pub fn parse_gremlin_traversal(query: &str) -> Result<Traversal, QueryRejection> {
    if query.len() > MAX_QUERY_LENGTH {
        return Err(QueryRejection::TooLarge(format!(
            "the query is {} characters, and this engine reads at most {MAX_QUERY_LENGTH}",
            query.len()
        )));
    }

    let mut tokens = Vec::new();
    for (lexed, span) in Token::lexer(query).spanned() {
        match lexed {
            Ok(token) => tokens.push((token, span)),
            Err(()) => {
                return Err(QueryRejection::Malformed(format!(
                    "unexpected character {} at offset {}",
                    super::echoed(query.get(span.clone()).unwrap_or_default()),
                    span.start
                )))
            }
        }
    }

    let mut stream = TokenStream {
        tokens,
        position: 0,
        end: query.len(),
    };
    if stream.peek().is_none() {
        return Err(QueryRejection::Malformed(
            "the query carries no Gremlin traversal".to_string(),
        ));
    }

    let chain = stream.parse_chain(0)?;
    if stream.peek().is_some() {
        return Err(stream.unexpected("the end of the query"));
    }

    let mut segments = chain.into_iter();
    let Some(source) = segments.next() else {
        return Err(QueryRejection::Malformed(
            "the query carries no Gremlin traversal".to_string(),
        ));
    };
    if source.name != GRAPH_TRAVERSAL_SOURCE || source.arguments.is_some() {
        return Err(QueryRejection::Malformed(format!(
            "a Gremlin query starts from the traversal source '{GRAPH_TRAVERSAL_SOURCE}', not '{}'",
            super::echoed(&source.name)
        )));
    }
    let steps: Vec<Call> = segments.collect();

    // A modulator and a step label configure the step they follow, so a traversal that
    // reaches one before any step has been added is not a traversal at all.
    let mut has_step = false;
    for step in &steps {
        let name = step.name.as_str();
        if is_source_configuration(name) || TRAVERSAL_ACTIONS.contains(&name) {
            continue;
        }
        if MODULATORS.contains(&name) || name == STEP_LABEL {
            if !has_step {
                return Err(QueryRejection::Malformed(format!(
                    "'{}' configures the step it follows, but no step precedes it",
                    super::echoed(name)
                )));
            }
            continue;
        }
        has_step = true;
    }

    Ok(Traversal {
        source: source.name,
        steps,
    })
}

/// The tokens of one query, and how far through them the parse has got.
struct TokenStream {
    tokens: Vec<(Token, ::std::ops::Range<usize>)>,
    position: usize,
    /// The length of the query text, so "the query ends here" can name an offset.
    end: usize,
}

impl TokenStream {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position).map(|(token, _)| token)
    }

    /// The token after the next one, which is what tells a keyword (`true`) from the head
    /// of a dotted expression (`Order.asc`).
    fn peek_after(&self) -> Option<&Token> {
        self.tokens.get(self.position + 1).map(|(token, _)| token)
    }

    fn advance(&mut self) {
        self.position = (self.position + 1).min(self.tokens.len());
    }

    /// The refusal for a token that cannot appear where the parse is, naming what was
    /// expected, what was found and the offset it was found at.
    fn unexpected(&self, expected: &str) -> QueryRejection {
        match self.tokens.get(self.position) {
            Some((token, span)) => QueryRejection::Malformed(format!(
                "expected {expected} at offset {}, found {}",
                span.start,
                describe(token)
            )),
            None => QueryRejection::Malformed(format!(
                "expected {expected}, but the query ends at offset {}",
                self.end
            )),
        }
    }

    /// A dotted chain of segments: `g.V().has('code','ANC')`, `__.out()`, `Order.desc`.
    fn parse_chain(&mut self, depth: usize) -> Result<Vec<Call>, QueryRejection> {
        let mut chain = vec![self.parse_segment(depth)?];
        while self.peek() == Some(&Token::Dot) {
            self.advance();
            chain.push(self.parse_segment(depth)?);
        }
        Ok(chain)
    }

    /// One segment: a name, optionally applied to an argument list.
    fn parse_segment(&mut self, depth: usize) -> Result<Call, QueryRejection> {
        let Some(Token::Name(name)) = self.peek().cloned() else {
            return Err(self.unexpected("a step name"));
        };
        self.advance();
        if self.peek() != Some(&Token::OpenParen) {
            return Ok(Call {
                name,
                arguments: None,
            });
        }
        self.advance();
        let arguments = self.parse_arguments(depth + 1, &Token::CloseParen, "')'")?;
        Ok(Call {
            name,
            arguments: Some(arguments),
        })
    }

    /// The arguments up to `closing`, which must be there.
    fn parse_arguments(
        &mut self,
        depth: usize,
        closing: &Token,
        closing_name: &str,
    ) -> Result<Vec<Argument>, QueryRejection> {
        let mut arguments = Vec::new();
        if self.peek() != Some(closing) {
            loop {
                arguments.push(self.parse_argument(depth)?);
                if self.peek() == Some(&Token::Comma) {
                    self.advance();
                    continue;
                }
                break;
            }
        }
        if self.peek() != Some(closing) {
            return Err(self.unexpected(closing_name));
        }
        self.advance();
        Ok(arguments)
    }

    fn parse_argument(&mut self, depth: usize) -> Result<Argument, QueryRejection> {
        if depth > MAX_ARGUMENT_DEPTH {
            return Err(QueryRejection::TooComplex(format!(
                "the query nests arguments more than {MAX_ARGUMENT_DEPTH} deep"
            )));
        }
        match self.peek().cloned() {
            Some(Token::Text(text)) => {
                self.advance();
                Ok(Argument::Text(text))
            }
            Some(Token::Number(value)) => {
                self.advance();
                Ok(Argument::Number(value))
            }
            Some(Token::OpenBracket) => {
                self.advance();
                let items = self.parse_arguments(depth + 1, &Token::CloseBracket, "']'")?;
                Ok(Argument::List(items))
            }
            Some(Token::Name(name)) => {
                // `true`, `false` and `null` are keywords — unless the name heads a
                // dotted expression, which an enum token or a predicate does.
                let heads_expression = matches!(
                    self.peek_after(),
                    Some(Token::Dot) | Some(Token::OpenParen)
                );
                if !heads_expression {
                    let keyword = match name.as_str() {
                        "true" => Some(Argument::Boolean(true)),
                        "false" => Some(Argument::Boolean(false)),
                        "null" => Some(Argument::Null),
                        _ => None,
                    };
                    if let Some(keyword) = keyword {
                        self.advance();
                        return Ok(keyword);
                    }
                }
                Ok(Argument::Expression(self.parse_chain(depth)?))
            }
            _ => Err(self.unexpected("an argument")),
        }
    }
}

/// A token as a refusal message names it.
fn describe(token: &Token) -> String {
    match token {
        Token::Dot => "'.'".to_string(),
        Token::Comma => "','".to_string(),
        Token::OpenParen => "'('".to_string(),
        Token::CloseParen => "')'".to_string(),
        Token::OpenBracket => "'['".to_string(),
        Token::CloseBracket => "']'".to_string(),
        Token::Name(name) => format!("'{}'", super::echoed(name)),
        Token::Text(text) => format!("the string '{}'", super::echoed(text)),
        Token::Number(value) => format!("the number {}", super::echoed(value)),
    }
}

/// One step as the report prints it, and what it was printed from.
struct PrintedStep {
    /// The step as the report prints it, including whatever was folded onto it.
    text: String,
    /// Which of the traversal's steps this is the printing of.
    index: usize,
    /// Whether a modulator or a step label was folded onto it. The planner does not push
    /// a configured step down: it plans what the step alone says.
    modulated: bool,
}

/// The traversal's steps, printed in order.
///
/// A traversal action is not printed (it asks for the answer rather than adding a step),
/// a source configuration is not printed (it configures the source), and a modulator or
/// a step label is printed ON the step it configures.
fn printed_steps(traversal: &Traversal) -> Vec<PrintedStep> {
    let mut printed: Vec<PrintedStep> = Vec::new();
    for (index, step) in traversal.steps.iter().enumerate() {
        let name = step.name.as_str();
        if TRAVERSAL_ACTIONS.contains(&name) || is_source_configuration(name) {
            continue;
        }
        if name == STEP_LABEL || MODULATORS.contains(&name) {
            if let Some(last) = printed.last_mut() {
                last.modulated = true;
                if name == STEP_LABEL {
                    let labels = step
                        .arguments()
                        .iter()
                        .map(render_argument)
                        .collect::<Vec<_>>()
                        .join(",");
                    let _ = write!(last.text, "@[{labels}]");
                } else {
                    let _ = write!(last.text, ".{}", render_call(step));
                }
                continue;
            }
        }
        printed.push(PrintedStep {
            text: tinkerpop_step(step),
            index,
            modulated: false,
        });
    }
    printed
}

/// A segment as it was written: its name, and its arguments when it carries any.
fn render_call(call: &Call) -> String {
    match &call.arguments {
        None => call.name.clone(),
        Some(arguments) => format!(
            "{}({})",
            call.name,
            arguments
                .iter()
                .map(render_argument)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// An argument as the printed traversal shows it.
fn render_argument(argument: &Argument) -> String {
    match argument {
        Argument::Text(text) => text.escape_debug().collect(),
        Argument::Number(value) => value.clone(),
        Argument::Boolean(value) => value.to_string(),
        Argument::Null => "null".to_string(),
        Argument::Expression(chain) => chain
            .iter()
            .map(render_call)
            .collect::<Vec<_>>()
            .join("."),
        Argument::List(items) => format!(
            "[{}]",
            items
                .iter()
                .map(render_argument)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// How a step is printed in the report's traversal sections: the name of the step class
/// it compiles to, applied to the arguments it was compiled with.
///
/// Source: the step classes of Apache TinkerPop. A step this table does not name is
/// printed by TinkerPop's own naming convention — see [`generic_step`] — so a traversal
/// using a step not named here is still explained rather than refused.
fn tinkerpop_step(step: &Call) -> String {
    let arguments = step.arguments();
    let rendered: Vec<String> = arguments.iter().map(render_argument).collect();
    match step.name.as_str() {
        "V" => format!("GraphStep(vertex,[{}])", rendered.join(",")),
        "E" => format!("GraphStep(edge,[{}])", rendered.join(",")),
        "out" | "in" | "both" => vertex_step(&step.name, &rendered, "vertex"),
        "outE" | "inE" | "bothE" => vertex_step(&step.name, &rendered, "edge"),
        "outV" => "EdgeVertexStep(OUT)".to_string(),
        "inV" => "EdgeVertexStep(IN)".to_string(),
        "bothV" => "EdgeVertexStep(BOTH)".to_string(),
        "otherV" => "EdgeOtherVertexStep".to_string(),
        "has" | "hasLabel" | "hasId" | "hasKey" | "hasValue" => {
            format!("HasStep([{}])", has_predicate(&step.name, arguments))
        }
        "values" => format!("PropertiesStep([{}],value)", rendered.join(",")),
        "properties" => format!("PropertiesStep([{}],property)", rendered.join(",")),
        "valueMap" => "PropertyMapStep(value)".to_string(),
        "propertyMap" => "PropertyMapStep(property)".to_string(),
        "elementMap" => format!("ElementMapStep([{}])", rendered.join(",")),
        "id" => "IdStep".to_string(),
        "label" => "LabelStep".to_string(),
        "key" => "PropertyKeyStep".to_string(),
        "value" => "PropertyValueStep".to_string(),
        "count" => "CountGlobalStep".to_string(),
        "sum" => "SumGlobalStep".to_string(),
        "max" => "MaxGlobalStep".to_string(),
        "min" => "MinGlobalStep".to_string(),
        "mean" => "MeanGlobalStep".to_string(),
        "fold" => "FoldStep".to_string(),
        "unfold" => "UnfoldStep".to_string(),
        "limit" => format!("RangeGlobalStep(0,{})", rendered.join(",")),
        "range" => format!("RangeGlobalStep({})", rendered.join(",")),
        "skip" => format!("RangeGlobalStep({},-1)", rendered.join(",")),
        "tail" => format!("TailGlobalStep({})", rendered.join(",")),
        "order" => "OrderGlobalStep".to_string(),
        "dedup" => "DedupGlobalStep".to_string(),
        "path" => "PathStep".to_string(),
        "group" => "GroupStep".to_string(),
        "groupCount" => "GroupCountStep".to_string(),
        "drop" => "DropStep".to_string(),
        // One key selects that key; several select a map of them, which is a different
        // step class.
        "select" => match rendered.as_slice() {
            [only] => format!("SelectOneStep(last,{only})"),
            many => format!("SelectStep(last,[{}])", many.join(", ")),
        },
        _ => generic_step(&step.name, &rendered),
    }
}

/// `out`/`in`/`both` and their edge forms compile to TinkerPop's `VertexStep`, which
/// prints the direction it walks, the edge labels it is restricted to (when it is) and
/// whether it answers with the vertices reached or the edges walked.
fn vertex_step(name: &str, labels: &[String], returns: &str) -> String {
    let direction = if name.starts_with("out") {
        "OUT"
    } else if name.starts_with("in") {
        "IN"
    } else {
        "BOTH"
    };
    if labels.is_empty() {
        format!("VertexStep({direction},{returns})")
    } else {
        format!("VertexStep({direction},[{}],{returns})", labels.join(","))
    }
}

/// The predicate list a `has`-family step compiles to: the key it filters on, and the
/// predicate applied to that key.
///
/// `~label`, `~id`, `~key` and `~value` are the keys TinkerPop prints for the steps that
/// filter on an element's label, its id, a property key or a property value rather than
/// on a property of its own. One value is an equality and several are a membership,
/// which is how those steps compile; a predicate the query wrote itself is printed as
/// written.
fn has_predicate(name: &str, arguments: &[Argument]) -> String {
    let (key, values) = match name {
        "hasLabel" => ("~label".to_string(), arguments),
        "hasId" => ("~id".to_string(), arguments),
        "hasKey" => ("~key".to_string(), arguments),
        "hasValue" => ("~value".to_string(), arguments),
        // `has(key, …)`: the first argument is the property key it filters on.
        _ => match arguments.split_first() {
            Some((first, rest)) => (render_argument(first), rest),
            None => (String::new(), &[][..]),
        },
    };
    match values {
        [] => key,
        [only] => match only {
            Argument::Expression(chain) if is_predicate(chain) => {
                format!("{key}.{}", render_argument(only))
            }
            _ => format!("{key}.eq({})", render_argument(only)),
        },
        many => format!(
            "{key}.within({})",
            many.iter()
                .map(render_argument)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// Whether a dotted expression is one of the predicates a `has` step may carry.
fn is_predicate(chain: &[Call]) -> bool {
    chain
        .last()
        .is_some_and(|call| call.arguments.is_some() && PREDICATES.contains(&call.name.as_str()))
}

/// A step this module does not name explicitly, printed by TinkerPop's own naming
/// convention: the step name capitalised and suffixed with `Step`, applied to the
/// arguments the query wrote.
fn generic_step(name: &str, rendered: &[String]) -> String {
    let mut printed = String::with_capacity(name.len() + 4);
    let mut characters = name.chars();
    if let Some(first) = characters.next() {
        printed.extend(first.to_uppercase());
        printed.push_str(characters.as_str());
    }
    printed.push_str("Step");
    if rendered.is_empty() {
        printed
    } else {
        format!("{printed}({})", rendered.join(","))
    }
}

/// One position of a quad pattern: a variable this plan allocated, or a value the query
/// bound the position to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Term {
    Variable(usize),
    Literal(String),
}

impl Term {
    fn render(&self) -> String {
        match self {
            Self::Variable(id) => format!("?{id}"),
            Self::Literal(value) => value.clone(),
        }
    }

    fn variable(&self) -> Option<usize> {
        match self {
            Self::Variable(id) => Some(*id),
            Self::Literal(_) => None,
        }
    }
}

/// A value as a quad pattern prints it, or `None` for an argument that is not a value
/// (a predicate, a nested traversal), which no pattern position can carry.
fn pattern_value(argument: &Argument) -> Option<String> {
    match argument {
        Argument::Text(text) => Some(format!("\"{}\"", text.escape_debug())),
        Argument::Number(value) => Some(value.clone()),
        Argument::Boolean(value) => Some(value.to_string()),
        Argument::Null | Argument::Expression(_) | Argument::List(_) => None,
    }
}

/// A property key or edge label in the predicate position of a pattern.
fn pattern_key(key: &str) -> String {
    format!("<{}>", key.escape_debug())
}

/// What the pattern-matching part of the traversal is positioned on.
#[derive(Debug, Clone)]
enum Position {
    Vertex(Term),
    /// An edge, which the pattern that matched it also bound both endpoints of.
    Edge { id: Term, from: Term, to: Term },
}

/// How a `has`-family filter turns into a quad pattern.
enum FilterPattern {
    /// Both the predicate and the object position are bound (`has(key, value)`).
    Value { predicate: String, object: String },
    /// The property must be there, so its value is a variable the pattern binds.
    Existence { predicate: String },
}

/// The quad pattern a `has`-family step matches, or `None` when it matches none that this
/// planner builds — several labels (an alternative), or a predicate rather than a value.
/// Such a step is left to the step iterators instead.
fn filter_pattern(step: &Call) -> Option<FilterPattern> {
    let arguments = step.arguments();
    match (step.name.as_str(), arguments) {
        ("has", [Argument::Text(key)]) => Some(FilterPattern::Existence {
            predicate: pattern_key(key),
        }),
        ("has", [Argument::Text(key), value]) => Some(FilterPattern::Value {
            predicate: pattern_key(key),
            object: pattern_value(value)?,
        }),
        ("hasLabel", [label]) => Some(FilterPattern::Value {
            predicate: LABEL_PREDICATE.to_string(),
            object: pattern_value(label)?,
        }),
        _ => None,
    }
}

/// The plan the report's "Optimized Traversal" section prints.
#[derive(Debug, Default)]
struct ExplainPlan {
    /// The quad patterns the pushed-down part of the traversal matches.
    patterns: Vec<String>,
    /// The elements and values those patterns bind, in the order they were bound.
    path: Vec<String>,
    /// The first variable id no pattern uses.
    max_var_id: usize,
    /// What the graph query answers with: the kind of the last thing it bound.
    answers: &'static str,
    /// The steps left for the step iterators, in order.
    remaining: Vec<String>,
}

/// Builds [`ExplainPlan`] by walking the traversal's steps and pushing down the ones it
/// can match with quad patterns.
struct PlanBuilder {
    patterns: Vec<String>,
    path: Vec<String>,
    next_var: usize,
    position: Option<Position>,
    answers: &'static str,
    /// Set once a step has bound a VALUE: the pattern-matching part of a plan walks
    /// elements, so it ends where the values begin.
    finished: bool,
}

impl PlanBuilder {
    fn new() -> Self {
        Self {
            patterns: Vec::new(),
            path: Vec::new(),
            next_var: 1,
            position: None,
            answers: "Vertex",
            finished: false,
        }
    }

    /// The next unused variable.
    fn fresh(&mut self) -> Term {
        let id = self.next_var;
        self.next_var += 1;
        Term::Variable(id)
    }

    /// Record that `term` is bound, which is what the plan's `path` annotation lists.
    fn bind(&mut self, kind: &str, term: &Term, node: &str) {
        self.path.push(format!("{kind}({}):{node}", term.render()));
    }

    /// Add the pattern `(subject, predicate, object, graph)`, projecting out the
    /// variables of `projected` it binds.
    fn add_pattern(
        &mut self,
        subject: &Term,
        predicate: &Term,
        object: &Term,
        graph: &Term,
        projected: &[&Term],
    ) {
        let mut variables: Vec<usize> = projected.iter().filter_map(|term| term.variable()).collect();
        variables.sort_unstable();
        variables.dedup();
        let projection = variables
            .iter()
            .map(|id| format!("?{id}"))
            .collect::<Vec<_>>()
            .join(",");
        self.patterns.push(format!(
            "PatternNode[({}, {}, {}, {}) . project {} .]",
            subject.render(),
            predicate.render(),
            object.render(),
            graph.render(),
            projection
        ));
    }

    /// Push `step` down into the patterns, and report whether it went down. A step that
    /// did not is evaluated by the step iterators, and so is every step after it: the
    /// patterns match a prefix of the traversal.
    fn push_down(&mut self, step: &Call, next: Option<&Call>) -> bool {
        if self.finished {
            return false;
        }
        match self.position.clone() {
            None => match step.name.as_str() {
                "V" => self.start_vertices(step, next),
                "E" => self.start_edges(step),
                _ => false,
            },
            Some(position) => self.extend(&position, step),
        }
    }

    /// `g.V()`: bind the vertex the traversal starts from.
    ///
    /// Given an id, the start is bound to it, so nothing has to be looked up. Given none,
    /// the vertices are found by their label — unless the next step is a filter that goes
    /// down too, in which case that filter is what finds them.
    fn start_vertices(&mut self, step: &Call, next: Option<&Call>) -> bool {
        let arguments = step.arguments();
        let start = match arguments {
            [] => self.fresh(),
            [only] => match pattern_value(only) {
                Some(value) => Term::Literal(value),
                None => return false,
            },
            // Several ids are alternatives, which these patterns (a conjunction) do not
            // express, so the start is left to the step iterators.
            _ => return false,
        };
        self.bind("Vertex", &start, "GraphNode");
        self.answers = "Vertex";
        let found_by_filter =
            arguments.is_empty() && next.is_some_and(|next| filter_pattern(next).is_some());
        if !found_by_filter {
            let label = self.fresh();
            let graph = Term::Literal(DEFAULT_GRAPH.to_string());
            let predicate = Term::Literal(LABEL_PREDICATE.to_string());
            self.add_pattern(&start, &predicate, &label, &graph, &[&start, &label]);
        }
        self.position = Some(Position::Vertex(start));
        true
    }

    /// `g.E()`: bind the edge the traversal starts from, which is the edge statement's own
    /// quad — so the pattern that finds the edge binds both of its endpoints as well.
    ///
    /// An `E(id)` naming ids is left to the step iterators: an edge id is the graph
    /// position of its statement, and matching ids as alternatives is not something one
    /// pattern expresses.
    fn start_edges(&mut self, step: &Call) -> bool {
        if !step.arguments().is_empty() {
            return false;
        }
        let from = self.fresh();
        let label = self.fresh();
        let to = self.fresh();
        let edge = self.fresh();
        self.add_pattern(&from, &label, &to, &edge, &[&from, &to, &edge]);
        self.bind("Edge", &edge, "GraphNode");
        self.answers = "Edge";
        self.position = Some(Position::Edge {
            id: edge,
            from,
            to,
        });
        true
    }

    /// A step applied to the element the plan is positioned on.
    fn extend(&mut self, position: &Position, step: &Call) -> bool {
        let element = match position {
            Position::Vertex(term) => term.clone(),
            Position::Edge { id, .. } => id.clone(),
        };
        if let Some(pattern) = filter_pattern(step) {
            let graph = Term::Literal(DEFAULT_GRAPH.to_string());
            match pattern {
                FilterPattern::Value { predicate, object } => {
                    let predicate = Term::Literal(predicate);
                    let object = Term::Literal(object);
                    self.add_pattern(&element, &predicate, &object, &graph, &[&element]);
                }
                FilterPattern::Existence { predicate } => {
                    let predicate = Term::Literal(predicate);
                    let value = self.fresh();
                    self.add_pattern(&element, &predicate, &value, &graph, &[&element]);
                }
            }
            return true;
        }
        match step.name.as_str() {
            "out" | "in" | "outE" | "inE" => self.walk(position, step),
            "inV" | "outV" => self.endpoint(position, step),
            "values" | "label" => self.property(&element, step),
            _ => false,
        }
    }

    /// `out`/`in`/`outE`/`inE`: match the edge statement leaving the vertex the plan is
    /// positioned on. An edge statement is the quad (from-vertex, edge label, to-vertex,
    /// edge id), so the direction decides which endpoint is the one already bound.
    fn walk(&mut self, position: &Position, step: &Call) -> bool {
        // A traversal step walks from a VERTEX; from an edge it would need an endpoint
        // step first.
        let Position::Vertex(start) = position else {
            return false;
        };
        let predicate = match step.arguments() {
            [] => self.fresh(),
            [Argument::Text(label)] => Term::Literal(pattern_key(label)),
            // Several labels are alternatives, which one pattern does not express.
            _ => return false,
        };
        let other = self.fresh();
        let edge = self.fresh();
        let outgoing = step.name.starts_with("out");
        let (from, to) = if outgoing {
            (start.clone(), other.clone())
        } else {
            (other.clone(), start.clone())
        };
        let walks_edges = step.name.ends_with('E');
        if walks_edges {
            self.add_pattern(&from, &predicate, &to, &edge, &[&from, &to, &edge]);
        } else {
            self.add_pattern(&from, &predicate, &to, &edge, &[&from, &to]);
        }
        self.bind("Vertex", &other, "GraphNode");
        if walks_edges {
            self.bind("Edge", &edge, "GraphNode");
            self.answers = "Edge";
            self.position = Some(Position::Edge {
                id: edge,
                from,
                to,
            });
        } else {
            self.answers = "Vertex";
            self.position = Some(Position::Vertex(other));
        }
        true
    }

    /// `inV`/`outV`: the endpoint is already bound by the edge's own pattern, so the plan
    /// moves onto it without matching anything more.
    fn endpoint(&mut self, position: &Position, step: &Call) -> bool {
        let Position::Edge { from, to, .. } = position else {
            return false;
        };
        if !step.arguments().is_empty() {
            return false;
        }
        let vertex = if step.name == "inV" {
            to.clone()
        } else {
            from.clone()
        };
        self.answers = "Vertex";
        self.position = Some(Position::Vertex(vertex));
        true
    }

    /// `values(key)` / `label()`: match the property statement of the element the plan is
    /// positioned on, which binds a VALUE rather than another element — so the graph
    /// query answers with values, and the pattern-matching part of the plan ends here.
    fn property(&mut self, element: &Term, step: &Call) -> bool {
        let predicate = match (step.name.as_str(), step.arguments()) {
            ("label", []) => Term::Literal(LABEL_PREDICATE.to_string()),
            ("values", []) => self.fresh(),
            ("values", [Argument::Text(key)]) => Term::Literal(pattern_key(key)),
            _ => return false,
        };
        let value = self.fresh();
        let graph = Term::Literal(DEFAULT_GRAPH.to_string());
        self.add_pattern(element, &predicate, &value, &graph, &[element, &value]);
        self.bind("Value", &value, "ValueNode");
        self.answers = "Value";
        self.finished = true;
        true
    }

    fn finish(self, remaining: Vec<String>) -> ExplainPlan {
        ExplainPlan {
            patterns: self.patterns,
            path: self.path,
            max_var_id: self.next_var,
            answers: self.answers,
            remaining,
        }
    }
}

/// The plan for `traversal`: the patterns its pushed-down prefix matches, and the steps
/// left for the step iterators.
fn build_plan(traversal: &Traversal, printed: &[PrintedStep]) -> ExplainPlan {
    let mut builder = PlanBuilder::new();
    let mut remaining = Vec::new();
    let mut pushing = true;
    for (entry, printed_step) in printed.iter().enumerate() {
        let Some(parsed) = traversal.steps.get(printed_step.index) else {
            continue;
        };
        // A configured step (one carrying a modulator or a step label) is not pushed
        // down: what it answers depends on the configuration too.
        if pushing && !printed_step.modulated {
            let next = printed
                .get(entry + 1)
                .filter(|next| !next.modulated)
                .and_then(|next| traversal.steps.get(next.index));
            if builder.push_down(parsed, next) {
                continue;
            }
        }
        pushing = false;
        remaining.push(printed_step.text.clone());
    }
    builder.finish(remaining)
}

/// The explain report for `query`, which `traversal` is the reading of.
///
/// Derived from the traversal alone, so the same query always explains the same way.
pub fn gremlin_explain_report(query: &str, traversal: &Traversal) -> String {
    let printed = printed_steps(traversal);
    let plan = build_plan(traversal, &printed);

    let mut report = String::new();
    let _ = writeln!(report, "{REPORT_RULE}");
    let _ = writeln!(report, "Neptune Gremlin Explain");
    let _ = writeln!(report, "{REPORT_RULE}");
    let _ = writeln!(report);
    let _ = writeln!(report, "Query String");
    let _ = writeln!(report, "============");
    let _ = writeln!(report, "{}", query.trim());
    let _ = writeln!(report);
    let _ = writeln!(report, "Original Traversal");
    let _ = writeln!(report, "==================");
    let _ = writeln!(
        report,
        "[{}]",
        printed
            .iter()
            .map(|step| step.text.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(report);
    let _ = writeln!(report, "Optimized Traversal");
    let _ = writeln!(report, "===================");
    let _ = writeln!(report, "Neptune steps:");
    let _ = writeln!(report, "[");
    if !plan.patterns.is_empty() {
        let _ = writeln!(report, "    +- NeptuneGraphQueryStep({}) {{", plan.answers);
        let _ = writeln!(report, "        JoinGroupNode {{");
        for pattern in &plan.patterns {
            let _ = writeln!(report, "            {pattern}");
        }
        let _ = writeln!(
            report,
            "        }}, annotations={{path=[{}], maxVarId={}}}",
            plan.path.join(", "),
            plan.max_var_id
        );
        let _ = writeln!(report, "    }}");
        let _ = writeln!(report, "    +- NeptuneTraverserConverterStep");
    }
    for step in &plan.remaining {
        let _ = writeln!(report, "    +- {step}");
    }
    let _ = writeln!(report, "]");
    report
}
