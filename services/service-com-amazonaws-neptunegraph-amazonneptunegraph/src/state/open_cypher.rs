// SPDX-License-Identifier: CC0-1.0

//! The openCypher data plane: how a graph's STORED nodes and edges are read by the
//! query engine, and how a result set and a refused query are reported.
//!
//! `ExecuteQuery` is the one operation that runs a caller-supplied query, and it runs it
//! for real: the text is lexed and parsed by the `cypher-parser` crate's openCypher
//! parser and evaluated by that crate's executor, which reads the graph ONLY through its
//! [`GraphProvider`] trait. [`StoredGraph`] is this service's implementation of that
//! trait over [`super::GraphData`], so a query answers from the same nodes and edges
//! every other reading of the graph (a summary, an export, a restore) is computed from —
//! there is no second copy of a graph's contents and nothing is synthesised to look
//! computed.
//!
//! What the engine decides and what this module decides are kept apart:
//!
//! * whether the query is WELL FORMED, and what it evaluates to, is the engine's —
//!   including the position a malformed query is refused at, which this module only
//!   words ([`syntax_refusal`]);
//! * how a node, an edge property or a result row is SPELLED on the wire is this
//!   service's, because that is the service's own protocol ([`run_query`]).
//!
//! The engine's value domain is openCypher's scalars (`null`, boolean, integer, string),
//! nodes, lists and maps, which is narrower than the JSON a property can be written
//! with: a FRACTIONAL number has no integer form, so it is read as the text of its JSON
//! number rather than silently truncated to an integer. Nothing else about a stored
//! property changes on the way in or out.
//!
//! ## What a query may do
//!
//! The evaluation happens IN THIS PROCESS over the data it is handed and nothing else:
//! the engine opens no socket, reads no file and starts nothing. Its grammar accepts
//! only `MATCH`, `OPTIONAL MATCH`, `UNWIND` and `WITH` before a `RETURN`, so a query
//! naming any other clause is reported as the clause this endpoint does not run
//! ([`unsupported_clause`]) rather than as a text that does not parse, and a function it
//! does not implement is refused by the engine itself.
//!
//! What a caller's query does choose is how much work it asks for, so every bound on
//! that is stated here, on this side of the engine, and each is taken BEFORE the work it
//! caps: the text one request may have read ([`super::MAX_QUERY_STRING_CHARS`]), how
//! deep that text may nest ([`MAX_QUERY_NESTING_DEPTH`]), the rows a query's own
//! literals may make ([`literal_rows`]), the candidate elements one evaluation may be
//! handed ([`MAX_QUERY_WORK_UNITS`]) and the span it may spend being handed them
//! ([`query_run_timeout`]). [`run_query`] takes no lock and reads no store, so the
//! operation runs it with the store's guard already given up, off the thread that serves
//! requests and under one of a bounded number of permits ([`MAX_CONCURRENT_QUERIES`]).

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use ::cypher_parser::{CypherValue, GraphProvider, ResultSet};

use super::GraphData;

/// Why one evaluation stopped before it had answered.
///
/// The engine reads the graph only through [`StoredGraph`], so what it is handed is
/// where an evaluation is STOPPED: once one of these is recorded the provider answers
/// nothing more, the engine unwinds, and the operation reports the limit that was met
/// rather than the short result the engine was left holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// The evaluation asked for more of the graph than one query may read.
    Exhausted,
    /// The span the query was given ran out while it was still asking.
    TimedOut,
}

/// What one evaluation may spend, and why it stopped if it did.
///
/// Charged in CANDIDATE ELEMENTS: every node the provider hands the engine costs one,
/// and every question the engine asks costs one — which is what makes the ceiling bound
/// the rows the engine can build, since the executor produces a row per candidate it was
/// handed and asks the provider again for every row it carries forward. The deadline is
/// read once per question, so a query whose cost diverges stops ASKING at the deadline
/// rather than running on past it.
#[derive(Debug)]
struct Budget {
    /// How many candidate elements this evaluation may still be handed.
    left: Cell<usize>,
    /// The instant after which it may be handed none.
    deadline: Instant,
    /// Why it stopped, once it has.
    stopped: Cell<Option<Stopped>>,
}

impl Budget {
    /// Charge `units` of the budget, answering whether the provider may answer at all.
    ///
    /// Checked BEFORE the work it pays for, so nothing past the ceiling is computed, and
    /// the first charge a question cannot afford is what records the stop: from then on
    /// every charge fails, which is how the engine is brought to a halt through the only
    /// surface it has.
    fn charge(&self, units: usize) -> bool {
        if self.stopped.get().is_some() {
            return false;
        }
        if Instant::now() >= self.deadline {
            self.stopped.set(Some(Stopped::TimedOut));
            return false;
        }
        let left = self.left.get();
        if units > left {
            self.stopped.set(Some(Stopped::Exhausted));
            return false;
        }
        self.left.set(left - units);
        true
    }
}

/// One graph's stored data, as the query engine reads it, under the budget one
/// evaluation may spend.
///
/// Built once per query from the graph's own [`super::GraphData`] and dropped with the
/// call: the engine asks about labels, adjacency and properties many times per query, so
/// the two adjacency indexes below are built in ONE pass over the edges rather than
/// walked per question — a query over a graph of n nodes and m edges costs one pass over
/// each, whatever the pattern it traverses.
#[derive(Debug)]
pub struct StoredGraph<'data> {
    /// The data every answer is read from.
    data: &'data GraphData,
    /// Every node id, in the store's own order: a node is addressed by its INDEX here,
    /// which is the cheap `Copy` handle the engine's `NodeId` asks for.
    nodes: Vec<&'data str>,
    /// For each relationship type, the targets of every node that leaves an edge of it.
    outgoing: BTreeMap<&'data str, BTreeMap<usize, Vec<usize>>>,
    /// The same adjacency read backwards: for each relationship type, the sources of
    /// every node an edge of it enters.
    incoming: BTreeMap<&'data str, BTreeMap<usize, Vec<usize>>>,
    /// What this evaluation may still spend, and why it stopped if it did.
    budget: Budget,
}

impl<'data> StoredGraph<'data> {
    /// Index `data` for one query that may be handed `units` candidate elements and must
    /// stop asking at `deadline`.
    #[must_use]
    pub fn over_within(data: &'data GraphData, units: usize, deadline: Instant) -> Self {
        let nodes: Vec<&str> = data.nodes.keys().map(String::as_str).collect();
        // Where each node id sits in `nodes`, so an edge's endpoints resolve to handles
        // in one lookup rather than a scan over the nodes per edge.
        let index_of: BTreeMap<&str, usize> = nodes
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, index))
            .collect();
        let mut outgoing: BTreeMap<&str, BTreeMap<usize, Vec<usize>>> = BTreeMap::new();
        let mut incoming: BTreeMap<&str, BTreeMap<usize, Vec<usize>>> = BTreeMap::new();
        for edge in data.edges.values() {
            // An edge the store holds always joins two nodes it holds
            // (`GraphData::insert_edge` refuses any other), so an endpoint that does not
            // resolve is nothing to answer about rather than a case to invent.
            let (Some(from), Some(to)) = (
                index_of.get(edge.from_node.as_str()),
                index_of.get(edge.to_node.as_str()),
            ) else {
                continue;
            };
            outgoing
                .entry(edge.label.as_str())
                .or_default()
                .entry(*from)
                .or_default()
                .push(*to);
            incoming
                .entry(edge.label.as_str())
                .or_default()
                .entry(*to)
                .or_default()
                .push(*from);
        }
        Self {
            data,
            nodes,
            outgoing,
            incoming,
            budget: Budget {
                left: Cell::new(units),
                deadline,
                stopped: Cell::new(None),
            },
        }
    }

    /// Why the evaluation over this graph stopped, if it did.
    #[must_use]
    pub fn stopped(&self) -> Option<Stopped> {
        self.budget.stopped.get()
    }

    /// The candidates `answers` once the budget has paid for them, or none at all when
    /// it cannot: a question the budget cannot afford is answered with nothing, which is
    /// what stops the engine asking more.
    fn afford(&self, answers: Vec<usize>) -> Vec<usize> {
        if self.budget.charge(answers.len()) {
            answers
        } else {
            Vec::new()
        }
    }

    /// The stored node one handle names, or `None` for a handle this graph does not hold.
    fn node(&self, node: usize) -> Option<&'data super::GraphNode> {
        self.nodes.get(node).and_then(|id| self.data.nodes.get(*id))
    }

    /// The neighbours of `node` along `rel_type` in one direction's index.
    fn neighbours(
        index: &BTreeMap<&'data str, BTreeMap<usize, Vec<usize>>>,
        node: usize,
        rel_type: &str,
    ) -> Vec<usize> {
        index
            .get(rel_type)
            .and_then(|by_node| by_node.get(&node))
            .cloned()
            .unwrap_or_default()
    }
}

impl GraphProvider for StoredGraph<'_> {
    type NodeId = usize;

    fn scan(&self, labels: &[String]) -> Vec<Self::NodeId> {
        // The question itself is charged BEFORE the scan runs, so a budget that is spent
        // pays for no further pass over the nodes.
        if !self.budget.charge(1) {
            return Vec::new();
        }
        let selected: Vec<usize> = (0..self.nodes.len())
            .filter(|node| {
                // "An empty slice matches every node", and a node carries several
                // labels, so a disjunction selects it when ANY of its labels is named.
                labels.is_empty()
                    || self.node(*node).is_some_and(|stored| {
                        labels.iter().any(|label| stored.labels.contains(label))
                    })
            })
            .collect();
        self.afford(selected)
    }

    fn matches_label(&self, node: Self::NodeId, label: &str) -> bool {
        if !self.budget.charge(1) {
            return false;
        }
        self.node(node)
            .is_some_and(|stored| stored.labels.contains(label))
    }

    fn relationship_types(&self) -> Vec<String> {
        if !self.budget.charge(1) {
            return Vec::new();
        }
        self.data
            .edges
            .values()
            .map(|edge| edge.label.clone())
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect()
    }

    fn expand(&self, node: Self::NodeId, rel_type: &str) -> Vec<Self::NodeId> {
        if !self.budget.charge(1) {
            return Vec::new();
        }
        self.afford(Self::neighbours(&self.outgoing, node, rel_type))
    }

    fn rel_sources(&self, rel_type: &str) -> Vec<Self::NodeId> {
        if !self.budget.charge(1) {
            return Vec::new();
        }
        let sources = self
            .outgoing
            .get(rel_type)
            .map(|by_node| by_node.keys().copied().collect())
            .unwrap_or_default();
        self.afford(sources)
    }

    fn expand_in(&self, node: Self::NodeId, rel_type: &str) -> Option<Vec<Self::NodeId>> {
        // The store holds both ends of every edge, so the reverse step is answered from
        // the index built with the forward one rather than left to the engine to rebuild
        // the whole graph's reverse adjacency for.
        if !self.budget.charge(1) {
            return Some(Vec::new());
        }
        Some(self.afford(Self::neighbours(&self.incoming, node, rel_type)))
    }

    fn property(&self, node: Self::NodeId, prop: &str) -> CypherValue {
        if !self.budget.charge(1) {
            return CypherValue::Null;
        }
        self.node(node)
            .and_then(|stored| stored.properties.get(prop))
            .map_or(CypherValue::Null, engine_value)
    }

    fn node_id(&self, node: Self::NodeId) -> String {
        if !self.budget.charge(1) {
            return String::new();
        }
        self.nodes.get(node).map_or_else(String::new, |id| (*id).to_string())
    }

    fn label(&self, node: Self::NodeId) -> String {
        // openCypher lets a node carry several labels and the engine asks for ONE; the
        // store holds them as an ordered set, so the one reported is the same on every
        // call rather than whichever a hash order yields.
        if !self.budget.charge(1) {
            return String::new();
        }
        self.node(node)
            .and_then(|stored| stored.labels.iter().next().cloned())
            .unwrap_or_default()
    }

    fn name(&self, node: Self::NodeId) -> String {
        // The display name of a node that carries one: the `name` property is what
        // openCypher examples and this service's own bulk-load formats name an element
        // by. A node without one is displayed by the id it is addressed by.
        if !self.budget.charge(1) {
            return String::new();
        }
        self.node(node)
            .and_then(|stored| stored.properties.get(NAME_PROPERTY))
            .and_then(|value| value.as_str().map(ToString::to_string))
            .unwrap_or_else(|| self.node_id(node))
    }
}

/// The property a node's display name is read from when it carries one.
const NAME_PROPERTY: &str = "name";

/// The reading clause a query that reads NOTHING of the graph is parsed behind.
///
/// It binds exactly one row and no element of the graph: `UNWIND` over a one-item list
/// is the engine's own way of making rows out of a literal, so the projection behind it
/// is evaluated once, which is what openCypher answers a query with no reading clause
/// with.
const ONE_ROW_CLAUSE: &str = "UNWIND [0] AS unitrow";

/// Parse `query` as openCypher reads it, or report where it could not be read.
///
/// openCypher lets a query PROJECT without reading the graph at all (`RETURN 1 AS one`),
/// which answers one row of the projected expressions. The engine's grammar requires a
/// reading clause in front of a projection (`MATCH`, `OPTIONAL MATCH`, `UNWIND` or
/// `WITH`), so such a query is read behind the clause that binds one row and nothing of
/// the graph ([`ONE_ROW_CLAUSE`]) — the caller's own projection is still what the engine
/// parses and evaluates, and the row count is the one openCypher states.
///
/// A text neither reading accepts is refused with the error of the query AS THE CALLER
/// SENT IT, so the position a refusal states is a position in the caller's own text and
/// never in a text this service composed.
pub fn parse_query(query: &str) -> Result<::cypher_parser::Query, ::cypher_parser::CypherError> {
    match ::cypher_parser::parse(query) {
        Ok(parsed) => Ok(parsed),
        Err(refused) => {
            ::cypher_parser::parse(&format!("{ONE_ROW_CLAUSE} {query}")).map_err(|_| refused)
        }
    }
}

/// Why a query this data plane was handed could not be answered.
///
/// Each variant is a different fact about the REQUEST, which is why they are kept apart:
/// the operation reports each one through the modelled error that states it.
#[derive(Debug)]
pub enum QueryFailure {
    /// The text could not be read as openCypher, or the query it reads as could not be
    /// evaluated.
    Refused(::cypher_parser::CypherError),
    /// The query text is longer than this emulator reads
    /// ([`super::MAX_QUERY_STRING_CHARS`]).
    TooLong { characters: usize, ceiling: usize },
    /// The query text nests deeper than this emulator reads
    /// ([`MAX_QUERY_NESTING_DEPTH`]).
    TooDeep { depth: usize, ceiling: usize },
    /// The result set is larger than this emulator answers with
    /// ([`MAX_QUERY_RESULT_ROWS`]).
    TooLarge { rows: usize, ceiling: usize },
    /// The query names a clause this data plane's engine has no grammar for
    /// ([`UNSUPPORTED_CLAUSES`]).
    Unsupported { clause: &'static str },
    /// The evaluation was stopped before it had answered: it asked for more of the graph
    /// than one query may read, or the span it was given ran out.
    Stopped(Stopped),
}

/// The most rows one query's result may carry.
///
/// Emulator resource guard, NOT a quota AWS publishes: the data a graph holds is bounded
/// ([`super::MAX_NODES_PER_GRAPH`], [`super::MAX_EDGES_PER_GRAPH`]) but a pattern naming
/// several elements answers a row per COMBINATION of them. What keeps that amplification
/// from being allocated at all is [`MAX_QUERY_WORK_UNITS`], which the engine spends as it
/// builds those rows; this ceiling is the last check on what is rendered, and a result
/// past it is reported as the limit it met rather than truncated to look complete.
pub const MAX_QUERY_RESULT_ROWS: usize = 50_000;

/// The most candidate elements one evaluation may be handed.
///
/// Emulator resource guard, and the bound that DOMINATES the rows an evaluation can
/// build: the executor produces a row per candidate the provider hands it and asks again
/// for every row it carries into the next step, so an evaluation that would answer a
/// combinatorial number of rows is stopped as it is building them rather than after.
/// Set above the cost of reading a full graph end to end — one pass over
/// [`super::MAX_NODES_PER_GRAPH`] nodes plus one over [`super::MAX_EDGES_PER_GRAPH`]
/// edges, several times over — and far below the memory a combinatorial expansion would
/// take.
pub const MAX_QUERY_WORK_UNITS: usize = 200_000;

/// The deepest a query text may nest before it is read.
///
/// Emulator resource guard: the engine's parser descends through a nested expression, so
/// a text nesting arbitrarily deep would recurse arbitrarily deep on a caller's input.
/// The depth is counted off the text's own brackets ([`nesting_depth`]) BEFORE the parser
/// sees it, and set well above any query a local emulator is asked to run.
pub const MAX_QUERY_NESTING_DEPTH: usize = 64;

/// How many queries this emulator evaluates AT ONCE.
///
/// Emulator resource guard: each evaluation holds a thread and a copy of the graph it
/// runs over, so without this bound a caller could start as many as it can open
/// connections. A request that finds every slot taken is shed as the throttle it is,
/// which is what keeps the host's threads and memory bounded by this number rather than
/// by the request rate.
pub const MAX_CONCURRENT_QUERIES: usize = 16;

/// The permits [`MAX_CONCURRENT_QUERIES`] is enforced with, held for the whole run of one
/// evaluation.
pub static QUERY_SLOTS: ::tokio::sync::Semaphore =
    ::tokio::sync::Semaphore::const_new(MAX_CONCURRENT_QUERIES);

/// The stack one evaluation is given.
///
/// The engine's parser and executor descend through a nested query, so the thread that
/// runs them is given a stack sized for the depth this service admits
/// ([`MAX_QUERY_NESTING_DEPTH`]) with room to spare, rather than left on whatever the
/// platform's default happens to be: an overflowing stack aborts the process, which is
/// not a failure one caller's query may cause.
pub const QUERY_STACK_BYTES: usize = 16 * 1024 * 1024;

/// How deep `query`'s brackets nest, read off the token stream the engine's own lexer
/// produces.
///
/// Not a parse — the engine owns that — but the one measurement that has to be taken
/// BEFORE one: how far a parser reading this text would descend. It is counted over the
/// ENGINE'S tokens rather than over the raw characters, so what this measures and what
/// the parser then recurses through cannot disagree: the lexer is what decides where a
/// string literal and a comment start and end, and a bracket inside either of them is
/// not a token at all. A text the lexer cannot read tokenises to nothing and nests
/// nothing; the parse that follows refuses it for the same reason the lexer did.
#[must_use]
pub fn nesting_depth(query: &str) -> usize {
    let Ok(tokens) = ::cypher_parser::lexer::tokenize(query) else {
        return 0;
    };
    let mut depth: usize = 0;
    let mut deepest: usize = 0;
    for token in &tokens {
        match token.kind {
            ::cypher_parser::lexer::TokenKind::LParen
            | ::cypher_parser::lexer::TokenKind::LBracket
            | ::cypher_parser::lexer::TokenKind::LBrace => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            ::cypher_parser::lexer::TokenKind::RParen
            | ::cypher_parser::lexer::TokenKind::RBracket
            | ::cypher_parser::lexer::TokenKind::RBrace => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// The clause keywords this data plane's engine has no grammar for: openCypher's write
/// clauses, which its own documentation states are "intentionally not supported", and the
/// clauses that would reach outside the graph at all (a bulk load from a file, a
/// procedure call).
///
/// Source: the clauses the engine's grammar accepts are `MATCH`, `OPTIONAL MATCH`,
/// `UNWIND` and `WITH` before a `RETURN`, so every other clause keyword is outside it.
/// They are listed here to say WHICH one a refused query named, because "this endpoint
/// does not run that clause" and "this text is not a query" are different answers to the
/// caller.
const UNSUPPORTED_CLAUSES: &[&str] = &[
    "CALL", "CREATE", "DELETE", "DETACH", "FOREACH", "LOAD", "MERGE", "REMOVE", "SET",
];

/// The clause of [`UNSUPPORTED_CLAUSES`] `query` names, if it names one.
///
/// Read off the engine's own token stream, so a keyword inside a string literal or a
/// comment is not one: the lexer has already decided what is a word of the query. A word
/// that follows a `.` is a property name, a word that follows `AS` is an alias the query
/// chose, and a word a `:` follows is a map key — none of which is a clause, so none of
/// them counts.
#[must_use]
pub fn unsupported_clause(query: &str) -> Option<&'static str> {
    use ::cypher_parser::lexer::TokenKind;
    let tokens = ::cypher_parser::lexer::tokenize(query).ok()?;
    for (at, token) in tokens.iter().enumerate() {
        let TokenKind::Ident(word) = &token.kind else {
            continue;
        };
        let names_something_else = match at.checked_sub(1).and_then(|before| tokens.get(before)) {
            Some(before) => match &before.kind {
                TokenKind::Dot => true,
                TokenKind::Ident(keyword) => keyword.eq_ignore_ascii_case(ALIAS_KEYWORD),
                _ => false,
            },
            None => false,
        };
        if names_something_else {
            continue;
        }
        if matches!(tokens.get(at + 1).map(|after| &after.kind), Some(TokenKind::Colon)) {
            continue;
        }
        if let Some(clause) = UNSUPPORTED_CLAUSES
            .iter()
            .find(|clause| word.eq_ignore_ascii_case(clause))
        {
            return Some(clause);
        }
    }
    None
}

/// The keyword a projection names a column by, after which a word is the caller's own
/// alias rather than a clause.
const ALIAS_KEYWORD: &str = "AS";

/// The rows a query could make from its own LITERALS, before any of the graph is read —
/// a WORST CASE, read off the token stream the engine's own lexer produces.
///
/// `UNWIND <list> AS x` makes one row per item of the list, so a query chaining several
/// multiplies them. Those rows are the ones no reading of the graph is charged for, so
/// the work budget the provider spends ([`MAX_QUERY_WORK_UNITS`]) never sees them, and
/// they have to be bounded from the text instead. What is measured is deliberately
/// INDEPENDENT of where each `UNWIND` reads from — a list written in place, a variable a
/// `WITH` bound to one, an alias of an alias — because the question is only how many rows
/// the text could possibly expand to: the longest list literal the text contains, raised
/// to the number of `UNWIND` clauses that could each expand one. No list a query expands
/// can be longer than the longest one it writes, so this cannot be under-counted by a
/// binding, and it saturates rather than wrapping, so a text that would answer more rows
/// than a `usize` counts reports the largest count.
#[must_use]
pub fn literal_rows(query: &str) -> usize {
    use ::cypher_parser::lexer::TokenKind;
    let Ok(tokens) = ::cypher_parser::lexer::tokenize(query) else {
        return 1;
    };
    // The items of each list the text is inside of, innermost last: a `,` counts for the
    // list it sits directly in, so a list of lists counts its own items and not theirs.
    let mut open_lists: Vec<usize> = Vec::new();
    let mut longest_list: usize = 0;
    let mut expansions: u32 = 0;
    for (at, token) in tokens.iter().enumerate() {
        match &token.kind {
            TokenKind::LBracket => open_lists.push(1),
            TokenKind::RBracket => {
                if let Some(items) = open_lists.pop() {
                    longest_list = longest_list.max(items);
                }
            }
            TokenKind::Comma => {
                if let Some(items) = open_lists.last_mut() {
                    *items += 1;
                }
            }
            TokenKind::Ident(word) if word.eq_ignore_ascii_case(UNWIND_KEYWORD) => {
                // A word that follows `.` or `AS` names a property or an alias rather
                // than the clause, the same reading [`unsupported_clause`] applies.
                let names_the_clause = !matches!(
                    at.checked_sub(1).and_then(|before| tokens.get(before)).map(|before| &before.kind),
                    Some(TokenKind::Dot)
                ) && !matches!(
                    at.checked_sub(1).and_then(|before| tokens.get(before)).map(|before| &before.kind),
                    Some(TokenKind::Ident(keyword)) if keyword.eq_ignore_ascii_case(ALIAS_KEYWORD)
                );
                if names_the_clause {
                    expansions = expansions.saturating_add(1);
                }
            }
            _ => {}
        }
    }
    if expansions == 0 {
        // A query that expands no list makes one row from its literals: the projection.
        return 1;
    }
    longest_list.max(1).saturating_pow(expansions)
}

/// The clause that expands a list into rows, and the one source of rows the graph is
/// never asked about.
const UNWIND_KEYWORD: &str = "UNWIND";

/// Run `query` over `data` and answer the payload the operation reports, or why it could
/// not be answered.
///
/// Every step of reading and evaluating the query is the engine's: the text is lexed and
/// parsed by its parser ([`parse_query`]) and evaluated by its executor over the graph
/// [`StoredGraph`] exposes. What this adds is the bounds a caller-chosen input needs,
/// each taken BEFORE the work it caps — the text this emulator reads at most, how deep
/// that text may nest, the rows a query's own literals may make, the candidate elements
/// one evaluation may be handed and the span it may spend being handed them — and the
/// rendering of the result in this service's own wire form.
///
/// `deadline` is the instant the evaluation must stop ASKING at, so a query whose cost
/// diverges stops doing work rather than being left running after its caller has been
/// answered. It touches no store and takes no lock, so the caller runs it off the request
/// path.
pub fn run_query(
    data: &GraphData,
    query: &str,
    deadline: Instant,
) -> Result<Vec<u8>, QueryFailure> {
    // Both text bounds are checked BEFORE the parser sees the text: `queryString` is
    // modelled as an unconstrained `String`, so these are what stop an arbitrarily large
    // or arbitrarily nested text from being parsed at all.
    let characters = query.chars().count();
    if characters > super::MAX_QUERY_STRING_CHARS {
        return Err(QueryFailure::TooLong {
            characters,
            ceiling: super::MAX_QUERY_STRING_CHARS,
        });
    }
    let depth = nesting_depth(query);
    if depth > MAX_QUERY_NESTING_DEPTH {
        return Err(QueryFailure::TooDeep {
            depth,
            ceiling: MAX_QUERY_NESTING_DEPTH,
        });
    }
    // The rows the query's own literals could expand to are counted from the TEXT and
    // refused before the engine builds any of them: they are rows the provider's budget
    // never sees, because the engine makes them without asking the graph anything.
    let literal_rows = literal_rows(query);
    if literal_rows > MAX_QUERY_RESULT_ROWS {
        return Err(QueryFailure::TooLarge {
            rows: literal_rows,
            ceiling: MAX_QUERY_RESULT_ROWS,
        });
    }
    let parsed = match parse_query(query) {
        Ok(parsed) => parsed,
        // A text the grammar could not read may be a well-formed query naming a clause
        // this engine has no grammar for, which is a different answer to the caller than
        // "this text is not a query": it is reported as the clause it named.
        Err(refused) => {
            return Err(match unsupported_clause(query) {
                Some(clause) => QueryFailure::Unsupported { clause },
                None => QueryFailure::Refused(refused),
            });
        }
    };
    let graph = StoredGraph::over_within(data, MAX_QUERY_WORK_UNITS, deadline);
    let results = ::cypher_parser::execute(&graph, &parsed).map_err(QueryFailure::Refused)?;
    // The evaluation may have been stopped while it was building: what the engine was
    // left holding is a SHORT answer, so the limit that stopped it is reported and the
    // rows are dropped.
    if let Some(stopped) = graph.stopped() {
        return Err(QueryFailure::Stopped(stopped));
    }
    if results.rows.len() > MAX_QUERY_RESULT_ROWS {
        return Err(QueryFailure::TooLarge {
            rows: results.rows.len(),
            ceiling: MAX_QUERY_RESULT_ROWS,
        });
    }
    Ok(results_payload(&results, data))
}

/// How long one query may run for: what the request asked for, clamped to the span a
/// graph's queries run under ([`super::DEFAULT_QUERY_TIMEOUT`]).
///
/// "Specifies the query timeout duration, in milliseconds"
/// (`smithy_query(query="operation",operation="ExecuteQuery")` ->
/// input.queryTimeoutMilliseconds) is the caller's choice, so it is honoured — but only
/// DOWNWARD: the documented timeout of a graph's queries is the maximum the service runs
/// one for, and a caller cannot ask for longer. A value at or below zero names no span to
/// run in and is read as the request having asked for nothing.
#[must_use]
pub fn query_run_timeout(requested_milliseconds: Option<i32>) -> ::std::time::Duration {
    let requested = requested_milliseconds
        .filter(|asked| *asked > 0)
        .map(|asked| ::std::time::Duration::from_millis(u64::from(asked.unsigned_abs())));
    requested.map_or(super::DEFAULT_QUERY_TIMEOUT, |asked| {
        asked.min(super::DEFAULT_QUERY_TIMEOUT)
    })
}

/// One stored property as the engine's value domain holds it.
///
/// Every JSON form a property can be written with has a counterpart except a FRACTIONAL
/// number, which openCypher's integer has no form for: it is read as the text of its own
/// JSON number, so the value is reported as written rather than truncated to a different
/// number (see the module docs).
fn engine_value(stored: &::serde_json::Value) -> CypherValue {
    match stored {
        ::serde_json::Value::Null => CypherValue::Null,
        ::serde_json::Value::Bool(flag) => CypherValue::Bool(*flag),
        ::serde_json::Value::Number(number) => number
            .as_i64()
            .map_or_else(|| CypherValue::Str(number.to_string()), CypherValue::Int),
        ::serde_json::Value::String(text) => CypherValue::Str(text.clone()),
        ::serde_json::Value::Array(items) => {
            CypherValue::List(items.iter().map(engine_value).collect())
        }
        ::serde_json::Value::Object(entries) => CypherValue::Map(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), engine_value(value)))
                .collect(),
        ),
    }
}

/// The member a result row names a column by: an openCypher result is a list of rows,
/// each an object keyed by the query's own return items.
const RESULTS_MEMBER: &str = "results";

/// The members a node is spelled with on this service's wire: its id, the kind of
/// element it is, its labels and its properties.
const NODE_ID_MEMBER: &str = "~id";
const NODE_ENTITY_TYPE_MEMBER: &str = "~entityType";
const NODE_LABELS_MEMBER: &str = "~labels";
const NODE_PROPERTIES_MEMBER: &str = "~properties";

/// The `~entityType` of a node.
const NODE_ENTITY_TYPE: &str = "node";

/// One result set as the operation's `payload` carries it: `{"results": [ … ]}`, one
/// object per row keyed by the query's return items, in the order the engine produced
/// them.
///
/// `data` is the graph the query ran over, so a row carrying a NODE reports that node as
/// this service reports one — its id, its labels and its properties, read from the store
/// rather than from the engine's display projection of it.
#[must_use]
fn results_payload(result: &ResultSet, data: &GraphData) -> Vec<u8> {
    let rows: Vec<::serde_json::Value> = result
        .rows
        .iter()
        .map(|row| {
            let mut object = ::serde_json::Map::new();
            for (column, value) in result.columns.iter().zip(row) {
                object.insert(column.clone(), wire_value(value, data));
            }
            ::serde_json::Value::Object(object)
        })
        .collect();
    ::serde_json::json!({ RESULTS_MEMBER: rows }).to_string().into_bytes()
}

/// One value the engine produced, as this service spells it on the wire.
fn wire_value(value: &CypherValue, data: &GraphData) -> ::serde_json::Value {
    match value {
        CypherValue::Null => ::serde_json::Value::Null,
        CypherValue::Bool(flag) => ::serde_json::Value::Bool(*flag),
        CypherValue::Int(number) => ::serde_json::Value::from(*number),
        CypherValue::Str(text) => ::serde_json::Value::String(text.clone()),
        CypherValue::Node { id, label, .. } => node_value(id, label, data),
        CypherValue::List(items) => ::serde_json::Value::Array(
            items.iter().map(|item| wire_value(item, data)).collect(),
        ),
        CypherValue::Map(entries) => ::serde_json::Value::Object(
            entries
                .iter()
                .map(|(key, entry)| (key.clone(), wire_value(entry, data)))
                .collect(),
        ),
    }
}

/// One bound node, spelled from the STORE: the id it is addressed by, every label it
/// carries and every property set on it.
///
/// A node the store no longer holds — a graph written between the query and this
/// rendering could not reach here, since both read one locked store — is reported with
/// the engine's own projection of it, so a row is never dropped from a result set.
fn node_value(id: &str, label: &str, data: &GraphData) -> ::serde_json::Value {
    let (labels, properties) = match data.nodes.get(id) {
        Some(stored) => (
            stored.labels.iter().cloned().collect::<Vec<String>>(),
            stored
                .properties
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<::serde_json::Map<String, ::serde_json::Value>>(),
        ),
        None => (
            vec![label.to_string()],
            ::serde_json::Map::new(),
        ),
    };
    ::serde_json::json!({
        NODE_ID_MEMBER: id,
        NODE_ENTITY_TYPE_MEMBER: NODE_ENTITY_TYPE,
        NODE_LABELS_MEMBER: labels,
        NODE_PROPERTIES_MEMBER: properties,
    })
}

/// The sentence a query the parser could not read is refused with: what was found where
/// the parser could not continue, what it was looking for there, and WHERE in the query
/// text that was — as a line, a column and the byte offset the parser reported.
///
/// The position is reported in all three forms because a client fixing a query needs the
/// line and column, while the offset is what the parser itself decided at; the clause
/// about what was expected is the engine's own, since what may follow a given prefix is
/// the grammar's statement and not this service's.
#[must_use]
pub fn syntax_refusal(query: &str, message: &str, position: usize) -> String {
    // A position past the end of the text, or inside a multi-byte character, is a
    // position this wording cannot split the text at: it is taken back to the nearest
    // character boundary at or before it, so no caller's text can make this panic.
    let offset = (0..=position.min(query.len()))
        .rev()
        .find(|at| query.is_char_boundary(*at))
        .unwrap_or_default();
    let before = query.get(..offset).unwrap_or_default();
    let line = before.matches('\n').count() + 1;
    let column = before
        .rfind('\n')
        .map_or(offset, |start| offset - start - 1)
        + 1;
    let found = query
        .get(offset..)
        .unwrap_or_default()
        .chars()
        .next()
        .map_or_else(String::new, |found| found.to_string());
    format!("Invalid input '{found}': {message} (line {line}, column {column} (offset: {offset}))")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{GraphEdge, GraphNode};

    /// A graph holding two cities joined by one route, written through the store's own
    /// writers — which is where every write of a graph's data goes.
    fn two_cities() -> GraphData {
        let mut data = GraphData::empty(::std::time::SystemTime::UNIX_EPOCH);
        for (id, name) in [("c1", "Berlin"), ("c2", "Paris")] {
            data.insert_node(
                id,
                GraphNode {
                    labels: BTreeSet::from([String::from("city")]),
                    properties: BTreeMap::from([(
                        String::from("name"),
                        ::serde_json::Value::String(name.to_string()),
                    )]),
                },
                ::std::time::SystemTime::UNIX_EPOCH,
            )
            .expect("the node fits the store");
        }
        data.insert_edge(
            "r1",
            GraphEdge {
                label: String::from("route"),
                from_node: String::from("c1"),
                to_node: String::from("c2"),
                properties: BTreeMap::new(),
            },
            ::std::time::SystemTime::UNIX_EPOCH,
        )
        .expect("the edge fits the store");
        data
    }

    /// A graph read with the budget and the span one query is given, which is what a
    /// served request runs under.
    fn within_the_usual_bounds(data: &GraphData) -> StoredGraph<'_> {
        StoredGraph::over_within(data, MAX_QUERY_WORK_UNITS, soon())
    }

    /// A deadline no test's evaluation can reach, so a row measures what it is about
    /// rather than how fast the host runs.
    fn soon() -> Instant {
        Instant::now() + ::std::time::Duration::from_secs(600)
    }

    /// RULE: the engine reads the graph through the trait, so adjacency answers the
    /// edges the store holds — in both directions, and nothing for a type it has none of.
    #[test]
    fn adjacency_answers_the_stored_edges() {
        let data = two_cities();
        let graph = within_the_usual_bounds(&data);
        let berlin = graph.scan(&[String::from("city")]);
        assert_eq!(
            berlin.len(),
            2,
            "case \"both cities\": expected 2 nodes, got {berlin:?}"
        );
        let from = graph.expand(0, "route");
        assert_eq!(
            from,
            vec![1],
            "case \"the outgoing route\": expected node 1, got {from:?}"
        );
        let into = graph.expand_in(1, "route");
        assert_eq!(
            into,
            Some(vec![0]),
            "case \"the incoming route\": expected node 0, got {into:?}"
        );
        let absent = graph.expand(0, "flight");
        assert!(
            absent.is_empty(),
            "case \"a type the graph has no edge of\": expected none, got {absent:?}"
        );
    }

    /// RULE: an evaluation is handed at most the candidates one query may read, and it
    /// is STOPPED when it asks for more — the engine is answered nothing from then on,
    /// and the limit is reported rather than the short result it was left holding.
    #[test]
    fn an_evaluation_that_asks_for_too_much_is_stopped() {
        let data = two_cities();
        // One unit buys the first question and nothing it answers, so the scan the
        // simplest pattern starts from cannot be paid for.
        let pinched = StoredGraph::over_within(&data, 1, soon());
        let parsed = parse_query("MATCH (c:city) RETURN c.name AS name").expect("a parse");
        let answered = ::cypher_parser::execute(&pinched, &parsed).expect("an evaluation");
        assert_eq!(
            pinched.stopped(),
            Some(Stopped::Exhausted),
            "case \"a budget of one element\": expected the evaluation to be stopped"
        );
        assert!(
            answered.rows.is_empty(),
            "case \"a budget of one element\": the engine is answered nothing once the \
             budget is spent, got {:?}",
            answered.rows
        );
    }

    /// RULE: an evaluation stops ASKING at its deadline, so a query whose cost diverges
    /// stops doing work rather than running on; the span is reported as the timeout it is.
    #[test]
    fn an_evaluation_past_its_deadline_is_stopped() {
        let data = two_cities();
        // A deadline already behind us: the first question the engine asks is the one
        // that finds it.
        let expired = StoredGraph::over_within(&data, MAX_QUERY_WORK_UNITS, Instant::now());
        let parsed = parse_query("MATCH (c:city) RETURN c.name AS name").expect("a parse");
        let answered = ::cypher_parser::execute(&expired, &parsed).expect("an evaluation");
        assert_eq!(
            expired.stopped(),
            Some(Stopped::TimedOut),
            "case \"a deadline already passed\": expected the evaluation to be stopped"
        );
        assert!(
            answered.rows.is_empty(),
            "case \"a deadline already passed\": expected no rows, got {:?}",
            answered.rows
        );
        let reported = run_query(&data, "MATCH (c:city) RETURN c.name AS name", Instant::now());
        assert!(
            matches!(reported, Err(QueryFailure::Stopped(Stopped::TimedOut))),
            "case \"a query run past its deadline\": expected the timeout to be reported, \
             got {reported:?}"
        );
    }

    /// RULE: how deep a text nests is measured over the ENGINE'S OWN tokens before it
    /// reads them, so a bracket inside a string literal or a comment nests nothing and a
    /// bracket after one still counts; a text past the depth this service reads is
    /// refused before it is parsed.
    #[test]
    fn a_query_text_is_refused_past_the_depth_it_may_nest() {
        // (case, the text, the depth it nests)
        let rows: &[(&str, String, usize)] = &[
            ("no brackets at all", String::from("MATCH n RETURN n"), 0),
            (
                "one node pattern",
                String::from("MATCH (n) RETURN n"),
                1,
            ),
            (
                "a property map inside a pattern",
                String::from("MATCH (n {name: 'a'}) RETURN n"),
                2,
            ),
            (
                "brackets inside a string literal",
                String::from("MATCH (n) RETURN '((((((((((' AS text"),
                1,
            ),
            (
                "a list of lists",
                String::from("UNWIND [[1, 2], [3]] AS row RETURN row"),
                2,
            ),
            (
                "brackets after a quote inside a line comment",
                format!(
                    "MATCH (n) // '\n RETURN {}0{} AS deep",
                    "(".repeat(70),
                    ")".repeat(70)
                ),
                70,
            ),
            (
                "brackets inside a block comment",
                String::from("MATCH (n) /* ((((( */ RETURN n"),
                1,
            ),
            (
                "at the depth this service reads",
                format!(
                    "MATCH (n) RETURN {}0{} AS deep",
                    "(".repeat(MAX_QUERY_NESTING_DEPTH),
                    ")".repeat(MAX_QUERY_NESTING_DEPTH)
                ),
                MAX_QUERY_NESTING_DEPTH,
            ),
            (
                "one level past it",
                format!(
                    "MATCH (n) RETURN {}0{} AS deep",
                    "(".repeat(MAX_QUERY_NESTING_DEPTH + 1),
                    ")".repeat(MAX_QUERY_NESTING_DEPTH + 1)
                ),
                MAX_QUERY_NESTING_DEPTH + 1,
            ),
        ];
        let data = GraphData::empty(::std::time::SystemTime::UNIX_EPOCH);
        for (case, query, expected) in rows {
            let depth = nesting_depth(query);
            assert_eq!(
                depth, *expected,
                "case {case:?}: expected a depth of {expected}, got {depth}"
            );
            if depth <= MAX_QUERY_NESTING_DEPTH {
                continue;
            }
            let refused = run_query(&data, query, soon());
            assert!(
                matches!(refused, Err(QueryFailure::TooDeep { .. })),
                "case {case:?}: expected the depth refusal, got {refused:?}"
            );
        }
    }

    /// RULE: the rows a query's own LITERALS could expand to are counted from the text
    /// and refused before the engine builds any of them — whatever the `UNWIND` reads
    /// from, a list written in place or a variable a `WITH` bound to one — while a query
    /// whose literals stay inside the ceiling is answered.
    #[test]
    fn literal_rows_are_bounded_before_the_engine_builds_them() {
        let data = GraphData::empty(::std::time::SystemTime::UNIX_EPOCH);
        /// A list literal of `items` integers, as a query text carries one.
        fn list(items: usize) -> String {
            let elements: Vec<String> = (0..items).map(|item| item.to_string()).collect();
            format!("[{}]", elements.join(","))
        }
        // (case, the query, the rows its literals could make, whether it is answered)
        let rows: Vec<(&str, String, usize, bool)> = vec![
            (
                "one small list",
                format!("UNWIND {} AS a RETURN a", list(3)),
                3,
                true,
            ),
            (
                "two small lists",
                format!("UNWIND {} AS a UNWIND {} AS b RETURN a, b", list(3), list(4)),
                16,
                true,
            ),
            (
                "a projection that reads nothing",
                String::from("RETURN 1 AS one"),
                1,
                true,
            ),
            (
                "a list of lists",
                String::from("UNWIND [[1, 2], [3, 4]] AS row RETURN row"),
                2,
                true,
            ),
            (
                "a pattern carrying no list at all",
                String::from("MATCH (c:city) RETURN c"),
                1,
                true,
            ),
            (
                "two lists whose product is past the ceiling",
                format!(
                    "UNWIND {} AS a UNWIND {} AS b RETURN a, b",
                    list(300),
                    list(300)
                ),
                90_000,
                false,
            ),
            (
                "one list expanded three times through a binding",
                format!(
                    "WITH {} AS big UNWIND big AS a UNWIND big AS b UNWIND big AS c \
                     RETURN a, b, c",
                    list(100)
                ),
                1_000_000,
                false,
            ),
        ];
        for (case, query, expected, answered) in &rows {
            let counted = literal_rows(query);
            assert_eq!(
                counted, *expected,
                "case {case:?}: expected {expected} literal row(s) at worst, got {counted}"
            );
            let run = run_query(&data, query, soon());
            if *answered {
                assert!(
                    run.is_ok(),
                    "case {case:?}: expected the query to be answered, got {run:?}"
                );
                continue;
            }
            assert!(
                matches!(run, Err(QueryFailure::TooLarge { .. })),
                "case {case:?}: expected the row ceiling to refuse it, got {run:?}"
            );
        }
    }

    /// RULE: a well-formed query naming a clause the engine has no grammar for is
    /// reported as THAT clause, not as a text that does not parse — and a word that is
    /// not a clause (a property name, a map key, a word inside a string or a comment) is
    /// not one.
    #[test]
    fn a_clause_the_engine_does_not_run_is_named() {
        // (case, the query, the clause it names)
        let rows: &[(&str, &str, Option<&str>)] = &[
            ("a create", "CREATE (n:Person) RETURN n", Some("CREATE")),
            ("a merge", "MERGE (n:Person) RETURN n", Some("MERGE")),
            (
                "a write to a property",
                "MATCH (n) SET n.name = 'a' RETURN n",
                Some("SET"),
            ),
            ("a delete", "MATCH (n) DELETE n RETURN n", Some("DELETE")),
            (
                "a detach delete",
                "MATCH (n) DETACH DELETE n RETURN n",
                Some("DETACH"),
            ),
            (
                "a procedure call",
                "CALL dbms.listConfig() YIELD name RETURN name",
                Some("CALL"),
            ),
            (
                "a bulk load",
                "LOAD CSV FROM 'file:///etc/passwd' AS line RETURN line",
                Some("LOAD"),
            ),
            (
                "a property whose name reads like a clause",
                "MATCH (n) RETURN n.set AS set",
                None,
            ),
            (
                "a map key that reads like a clause",
                "MATCH (n {create: 'a'}) RETURN n",
                None,
            ),
            (
                "a string that reads like a clause",
                "MATCH (n) RETURN 'DELETE' AS text",
                None,
            ),
            (
                "a comment that reads like a clause",
                "MATCH (n) // CREATE\n RETURN n",
                None,
            ),
            ("a query naming no clause at all", "MATCH (n) RETURN n", None),
        ];
        for (case, query, expected) in rows {
            let named = unsupported_clause(query);
            assert_eq!(
                named, *expected,
                "case {case:?}: expected {expected:?}, got {named:?}"
            );
        }
    }

    /// RULE: a query runs for the span the request asked for, clamped DOWNWARD to the
    /// span a graph's queries run under, and a request that asks for no usable span runs
    /// under that documented one.
    #[test]
    fn a_requested_timeout_is_honoured_only_downward() {
        // (case, the `queryTimeoutMilliseconds` the request sent, the span it runs under)
        let rows: &[(&str, Option<i32>, ::std::time::Duration)] = &[
            ("no timeout at all", None, crate::state::DEFAULT_QUERY_TIMEOUT),
            (
                "a span inside the documented one",
                Some(1_500),
                ::std::time::Duration::from_millis(1_500),
            ),
            (
                "a span past the documented one",
                Some(i32::MAX),
                crate::state::DEFAULT_QUERY_TIMEOUT,
            ),
            ("no span at all", Some(0), crate::state::DEFAULT_QUERY_TIMEOUT),
            (
                "a span below zero",
                Some(-1),
                crate::state::DEFAULT_QUERY_TIMEOUT,
            ),
        ];
        for (case, requested, expected) in rows {
            let actual = query_run_timeout(*requested);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the text one request may have read is bounded, and a text past that bound is
    /// reported as the oversized query it is rather than parsed.
    #[test]
    fn an_oversized_query_text_is_not_parsed() {
        let data = GraphData::empty(::std::time::SystemTime::UNIX_EPOCH);
        let ceiling = crate::state::MAX_QUERY_STRING_CHARS;
        // (case, how many characters the text carries, whether it is read at all)
        let rows: &[(&str, usize, bool)] = &[
            ("at the bound", ceiling, true),
            ("one character past the bound", ceiling + 1, false),
            ("far past the bound", ceiling * 2, false),
        ];
        for (case, characters, read) in rows {
            // A text of the row's length that is a query as far as its length goes: the
            // row is about the LENGTH, so the text is padded with a comment the grammar
            // accepts.
            let query = format!("MATCH (n) RETURN n // {}", "x".repeat(characters - 22));
            assert_eq!(
                query.chars().count(),
                *characters,
                "case {case:?}: the row's text must carry {characters} characters"
            );
            match run_query(&data, &query, soon()) {
                Ok(_) => assert!(
                    *read,
                    "case {case:?}: a text past the bound must not be read"
                ),
                Err(QueryFailure::TooLong { characters: counted, ceiling: bound }) => {
                    assert!(
                        !*read,
                        "case {case:?}: a text at the bound must be read, got the \
                         oversized refusal"
                    );
                    assert_eq!(
                        (counted, bound),
                        (*characters, ceiling),
                        "case {case:?}: expected the refusal to count the text it read"
                    );
                }
                Err(other) => panic!("case {case:?}: unexpected failure {other:?}"),
            }
        }
    }

    /// RULE: a query that reads nothing of the graph projects ONE row, whatever the
    /// graph holds, and a text no reading accepts is refused at a position in the
    /// caller's own text.
    #[test]
    fn a_query_with_no_reading_clause_projects_one_row() {
        let empty = GraphData::empty(::std::time::SystemTime::UNIX_EPOCH);
        let data = two_cities();
        // (case, the query, the data it runs over, the rows it answers)
        let rows: &[(&str, &str, &GraphData, usize)] = &[
            ("a constant over an empty graph", "RETURN 1 AS one", &empty, 1),
            ("a constant over a graph holding data", "RETURN 1 AS one", &data, 1),
            (
                "several constants",
                "RETURN 'a' AS left, 2 AS right",
                &empty,
                1,
            ),
            (
                "a reading clause the caller wrote",
                "MATCH (c:city) RETURN c.name AS name",
                &data,
                2,
            ),
        ];
        for (case, query, over, expected) in rows {
            let parsed = parse_query(query)
                .unwrap_or_else(|refused| panic!("case {case:?}: expected a parse, got {refused}"));
            let answered = ::cypher_parser::execute(&within_the_usual_bounds(over), &parsed)
                .unwrap_or_else(|refused| panic!("case {case:?}: expected a result, got {refused}"));
            assert_eq!(
                answered.rows.len(),
                *expected,
                "case {case:?}: expected {expected} row(s), got {:?}",
                answered.rows
            );
        }
        let refused = parse_query("RETURN").expect_err("a projection of nothing is no query");
        let ::cypher_parser::CypherError::Syntax { position, .. } = refused else {
            panic!("case \"a projection of nothing\": expected a syntax refusal, got {refused}");
        };
        assert!(
            position <= "RETURN".len(),
            "case \"a projection of nothing\": expected a position in the caller's own \
             text, got {position}"
        );
    }

    /// RULE: a position the parser reports is worded as a line, a column and the offset
    /// it was reported at, so the same refusal reads the same for a query on one line
    /// and one spread over several.
    #[test]
    fn a_syntax_refusal_states_the_position_it_was_reported_at() {
        // (case, query, offset, the line and column it must report)
        let rows: &[(&str, &str, usize, usize, usize)] = &[
            ("the first character", "X", 0, 1, 1),
            ("mid-line", "MATCH (n RETURN n", 9, 1, 10),
            ("the second line", "MATCH (n)\nRETRN n", 10, 2, 1),
            ("past the end", "MATCH", 99, 1, 6),
        ];
        for (case, query, offset, line, column) in rows {
            let worded = syntax_refusal(query, "expected something else", *offset);
            let expected = format!("(line {line}, column {column}");
            assert!(
                worded.contains(&expected),
                "case {case:?}: expected {expected:?} in the refusal, got {worded:?}"
            );
        }
    }
}
