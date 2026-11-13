// SPDX-License-Identifier: CC0-1.0

//! The one half of a constraint refusal the framework's walk cannot state — the
//! COLLECTION a clause about a collection member is about — served as the per-operation
//! wrapper that is the only layer able to state it.
//!
//! # The rule
//!
//! A value outside a constraint the model declares is refused before any handler runs,
//! over one clause per violation, each naming the value that was rejected, the member path
//! it sat at and the requirement it failed. The value is the only place the request's own
//! input is reported back, so a client comparing it against what it sent has to see its
//! own input.
//!
//! A `@length` on a map or a list is checked ON THE MEMBER, before the walk descends into
//! the entries, so the only rendering the framework has for the rejected value is a
//! SUMMARY of its size — `<n> items` for a list, `<n> entries` for a map — which is not a
//! value any caller sent (`vendor/smithy-schema-server/src/constraints.rs`). AWS states
//! the collection itself there: a map as `{<key>=<value>, …}`, a list as `[<element>, …]`.
//!
//! A constraint declared on a map's VALUE SHAPE is the other half: it is checked on each
//! ENTRY, so the framework reports it at the entry (`<map member>.<key>`) with the one
//! constraint that failed, where AWS reports it at the MAP MEMBER — quoting the value that
//! member held and listing the value shape's WHOLE declared constraint set, so one offending
//! map is ONE validation error however many of its entries, or of one value's constraints,
//! were broken. A service whose recordings show that states it
//! ([`StatedClause::grouping_map_values`]); one that does not keeps the framework's
//! per-entry clause.
//!
//! # Why a wrapper
//!
//! [`Service::override_refusal`](smithy_server_runtime::Service::override_refusal) is
//! handed the violations alone and never the request they were found in, so that value
//! cannot be recovered there. [`QuotingSubmittedCollections`] is where it is: a
//! per-operation wrapper, which has the request and can re-read it.
//!
//! Nothing is re-implemented and no check is repeated in other words: the violations and
//! the submitted collections come from this crate's shared collecting walk
//! ([`crate::validation::refusing_violations_with_mapped_echoes`], which runs the
//! protocol's own deserializer and the framework's own `constraints::validate` and owns
//! every bound on what one clause may quote), the restoring rule is
//! [`crate::validation_message::quoting_the_submitted_collection`], each clause is the
//! framework's own `Violation::message`, the envelope around them is the framework's own,
//! and the status and the wire envelope come from the framework's refusal channel. A
//! refusal with NO such clause is left to that channel untouched.
//!
//! # What a service states, and what it does not
//!
//! THE HOME for this wrapper: a service states only the facts that are its own — the error
//! CODE its refusals carry, the ORDER it states a map's entries in, how it answers a
//! rendered response, and which of its operations are served through the pass
//! ([`StatedClauseTable`]). Everything else is here, so two services cannot disagree about
//! how one collection is quoted.
//!
//! A map's entries have an order on the wire, none in the value the codec builds from it (a
//! string map arrives as a `HashMap`), and possibly a third in the clause the service
//! states — so the shared walk, which reads the BUILT input, can only state one map in key
//! order. The order the request wrote is still in the request, which this layer has, so a
//! service that states another order supplies it as a [`MapEntryOrder`] over the entries
//! read back from the request ([`submitted_map_entries_at`], at the member path the clause
//! names, so a map declared on a nested structure member is reached too);
//! [`in_submitted_order`] is the order the request carried, which is what a service whose
//! recordings show the submitted order passes.
//!
//! A service that words its constraint refusal in its own PRE-DISPATCH layer, rather than
//! through the wrapper, states the same one fact and reaches the same restating through
//! [`quoting_submitted_maps`]: such a layer holds the request but has no echo to restate
//! from, so the map is quoted from the body under the shared echo ceiling. Both paths share
//! the guards ([`restated_map`]) and the rendering, so neither can word one collection
//! differently from the other.
//!
//! What this layer may NOT do is enlarge the clause: every bound on what one message quotes
//! belongs to the shared walk, so a restated map is taken only when it renders to exactly
//! the length that walk had already decided to carry — the same entries in another order,
//! and therefore the same bytes. A clause that fails any of the guards keeps the shared
//! walk's own rendering: a value that is not a map (a list, whose order the built value does
//! keep), a member the body carries no map of strings at, a body this cannot read, a member
//! whose entry count differs from the one the walk counted, and an order the service's own
//! rule declines to state. A clause stated at a map MEMBER is bounded the same way: it quotes
//! the map up to the ceiling the shared walk quotes one clause to
//! ([`crate::validation::quoted_echo`]).
//!
//! The request is read AT MOST ONCE PER MEMBER PATH per refusal, and for at most a fixed
//! number of paths ([`SubmittedMaps`], [`MAX_PARSED_PATHS`]), the negative answer included —
//! so both the parses one refusal performs and the keys it retains are bounded by constants
//! rather than by the violation count a request chooses. That bound is what keeps a clause
//! stated at `<map member>.<key>`, whose last step is the caller's own text, from being able
//! to drive one parse per entry sent.

use std::collections::{HashMap, HashSet};

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::ViolationKind;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refused,
    Service, Violation,
};

use crate::validation::{quoted_echo, refusing_violations_with_mapped_echoes, rendered_map};
use crate::validation_message::{
    element_constraint_set, envelope_over, map_entry_count, map_value_set_requirement,
    quoting_the_submitted_collection, requirement,
};

/// The order a service states a map's entries in inside a clause, over the entries as the
/// REQUEST carried them — or `None` where that service's own rule declines to state an
/// order for them, which keeps the shared walk's own rendering.
pub type MapEntryOrder = fn(&[(String, String)]) -> Option<Vec<(&str, &str)>>;

/// The entries in the order the request submitted them: the order the caller's own JSON
/// object wrote them.
///
/// The [`MapEntryOrder`] of a service whose recordings quote a map in the order it was
/// sent, which is the order the validating layer in front of such a service iterates a
/// map that KEEPS its insertion order.
#[must_use]
pub fn in_submitted_order(entries: &[(String, String)]) -> Option<Vec<(&str, &str)>> {
    Some(
        entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect(),
    )
}

/// How a service renders a `@pattern` expression inside a requirement sentence it states:
/// the expression as the MODEL writes it (`str::to_string`), or without the whole-value
/// anchors a model adds to express the match AWS's own pattern makes implicitly
/// ([`crate::validation::unanchored`]) for a service whose recordings drop them. The CHECK
/// always ran the model's expression as written; this is the rendering alone.
pub type PatternRendering = fn(&str) -> String;

/// How a service answers a response one of its layers rendered — the seam a service whose
/// wire form differs from the one a renderer wrote states that difference at (the member an
/// error body carries its text in, say).
pub type AnsweredAs = fn(RtResponse) -> RtResponse;

/// A response answered exactly as its renderer wrote it: the [`AnsweredAs`] of a service
/// that restates nothing.
#[must_use]
pub fn as_rendered(resp: RtResponse) -> RtResponse {
    resp
}

/// The facts ONE service states about its own constraint clauses: the error code they
/// carry, the order it states a map's entries in, and how it answers a rendered response.
#[derive(Clone, Copy)]
pub struct StatedClause {
    /// The error code a constraint refusal of this service carries — the one its own
    /// refusal seam picks for the class, because the restated message has to travel with
    /// the code it would have travelled with: a client branches on the code.
    pub refusal_code: &'static str,
    /// The order this service states a map's entries in ([`MapEntryOrder`]).
    pub entry_order: MapEntryOrder,
    /// How this service answers a rendered response ([`AnsweredAs`]).
    pub answered_as: AnsweredAs,
    /// How this service renders a `@pattern` inside the constraint set it states at a MAP
    /// member for a constraint that map's VALUES broke, or `None` where it keeps the
    /// framework's per-entry clause instead ([`StatedClause::grouping_map_values`]).
    pub map_value_patterns: Option<PatternRendering>,
}

impl StatedClause {
    /// The clause of a service that quotes a map in the order the request submitted it and
    /// answers a rendered response unchanged, under `refusal_code`.
    #[must_use]
    pub fn under(refusal_code: &'static str) -> Self {
        Self {
            refusal_code,
            entry_order: in_submitted_order,
            answered_as: as_rendered,
            map_value_patterns: None,
        }
    }

    /// The same clause, stating a constraint broken by one of a MAP's values at the map
    /// MEMBER — the whole map as the value, the value shape's complete declared constraint
    /// set as the requirement — with a `@pattern` in that set rendered as `pattern` renders
    /// one.
    ///
    /// For a service whose recordings show that clause. The grouping, the set (read off the
    /// operation's input schema, so it is the MODEL's constraints and not the one that
    /// happened to fail) and every sentence in it stay here; what a service chooses is
    /// whether it states that clause at all and how the expression is quoted.
    #[must_use]
    pub fn grouping_map_values(self, pattern: PatternRendering) -> Self {
        Self {
            map_value_patterns: Some(pattern),
            ..self
        }
    }

    /// The same clause, with the order this service states a map's entries in.
    #[must_use]
    pub fn ordering_entries(self, entry_order: MapEntryOrder) -> Self {
        Self {
            entry_order,
            ..self
        }
    }

    /// The same clause, with how this service answers a rendered response.
    #[must_use]
    pub fn answered(self, answered_as: AnsweredAs) -> Self {
        Self {
            answered_as,
            ..self
        }
    }
}

/// One modelled operation, served so that a constraint clause about a collection member
/// states the collection the request carried.
///
/// It makes no routing decision and holds no state of its own: the operation it wraps is
/// the one the generated routing table names for the same key, built from the SAME shared
/// engine, and a request this wrapper does not refuse is handed to it untouched — so the
/// framework still owns the whole deserialize -> validate -> handle -> serialize sequence
/// and every status, shape and message it answers with.
///
/// The cost is DELIBERATE: a request's body is read twice, here and in the operation's own
/// sequence, because the framework hands no already-built input to an operation. Which
/// operations pay it is the service's own choice ([`StatedClauseTable::quoting`] against
/// [`StatedClauseTable::answering`]).
pub struct QuotingSubmittedCollections<Op> {
    operation: Op,
    stated: StatedClause,
}

impl<Op> QuotingSubmittedCollections<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: Op, stated: StatedClause) -> Self {
        Self { operation, stated }
    }
}

impl<Op> ErasedOperation for QuotingSubmittedCollections<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The whole refusal is decided — and the borrow of the definition it is
            // rendered through taken and dropped — BEFORE the await below:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding one across that
            // await would make this future non-`Send`.
            if let Some(refused) = quoting_refusal::<Op>(svc, req, &self.stated) {
                return (self.stated.answered_as)(refused);
            }
            (self.stated.answered_as)(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// One modelled operation, served so that its responses are answered as this service
/// answers them ([`AnsweredAs`]) and NOTHING else changes.
///
/// For an operation whose model declares no bounded collection member, where the pass
/// above could never produce a clause and would only read the body a second time.
pub struct AnsweringAs<Op> {
    operation: Op,
    answered_as: AnsweredAs,
}

impl<Op> AnsweringAs<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: Op, answered_as: AnsweredAs) -> Self {
        Self {
            operation,
            answered_as,
        }
    }
}

impl<Op> ErasedOperation for AnsweringAs<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { (self.answered_as)(self.operation.run(svc, req, ctx).await) })
    }

    /// The wrapped operation's modelled auth schemes, for the same reason as above.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The routing table a service serves its operations through so that their error bodies
/// are the ones IT answers with: every operation wrapped, keyed by the routing key the
/// protocol resolves (the operation's own modelled name).
///
/// A service builds one in its `EmulatedService::new` and answers
/// `Service::override_operation` from it; [`quoting`](Self::quoting) and
/// [`answering`](Self::answering) each take one already-constructed operation and file it
/// under [`Operation::NAME`], so a wrapped operation cannot land under a key the protocol
/// never resolves.
pub struct StatedClauseTable {
    table: OperationRoutingTable,
    stated: StatedClause,
}

impl StatedClauseTable {
    /// An empty table whose operations state `stated`'s clauses.
    #[must_use]
    pub fn new(stated: StatedClause) -> Self {
        Self {
            table: OperationRoutingTable::new(),
            stated,
        }
    }

    /// Serve `operation` with a constraint clause about a collection member QUOTING the
    /// collection the request submitted — for an operation whose model declares a bounded
    /// collection member, which is the only kind that can produce such a clause.
    pub fn quoting<O: Operation + ErasedOperation>(&mut self, operation: O) -> &mut Self {
        self.table.add_operation(
            O::NAME,
            Box::new(QuotingSubmittedCollections::new(operation, self.stated)),
        );
        self
    }

    /// Serve `operation` with its responses answered as this service answers them and no
    /// collection pass at all.
    pub fn answering<O: Operation + ErasedOperation>(&mut self, operation: O) -> &mut Self {
        self.table.add_operation(
            O::NAME,
            Box::new(AnsweringAs::new(operation, self.stated.answered_as)),
        );
        self
    }

    /// The table, for the service to hold and route from.
    #[must_use]
    pub fn finish(self) -> OperationRoutingTable {
        self.table
    }
}

/// The refusal `req` earns from `Op`'s own constraints when at least one of its clauses is
/// about a COLLECTION whose submitted value could be stated, or about a MAP's VALUE for a
/// service that states those ([`StatedClause::grouping_map_values`]) — or `None`, which is
/// every other request, including one refused only by clauses this changes nothing about, so
/// the framework's own channel answers it exactly as before.
pub fn quoting_refusal<Op: Operation>(
    svc: &dyn Service,
    req: &RtRequest,
    stated: &StatedClause,
) -> Option<RtResponse> {
    let refusing = refusing_violations_with_mapped_echoes::<Op>(svc.definition().protocol(), req).0;
    let mut restated_a_clause = false;
    // The request's own maps, read at most once per member whatever the clause count.
    let mut submitted = SubmittedMaps::of(req);
    // The MAP members already stated. Every value of one map has to satisfy the same set, so
    // the clause is the same sentence however many entries of that map broke it and
    // whichever of the set's constraints each broke: stating it once is what makes one
    // offending map ONE validation error.
    let mut grouped_maps: HashSet<String> = HashSet::new();
    let mut clauses: Vec<String> = Vec::new();
    let mut violations: Vec<Violation> = Vec::new();
    for (violation, echo) in refusing {
        if let Some(pattern) = stated.map_value_patterns {
            // A further violation under a map whose clause is ALREADY stated needs no second
            // clause. Every one of them is one of that map's VALUES: the shared walk reports
            // a map's KEY as a path segment and never as a constrained leaf
            // (`vendor/smithy-schema-server/src/constraints.rs`), so nothing else is
            // reported below a map member.
            if entry_of(&violation.path).is_some_and(|(member, _)| grouped_maps.contains(member))
            {
                violations.push(violation);
                continue;
            }
            if let Some((member, clause)) =
                map_value_clause::<Op>(&violation, &mut submitted, pattern)
            {
                restated_a_clause = true;
                violations.push(violation);
                if grouped_maps.insert(member) {
                    clauses.push(clause);
                }
                continue;
            }
        }
        let quoted = quoting_the_submitted_collection(violation.clone(), echo);
        let restated = as_stated(&violation, quoted, &mut submitted, stated.entry_order);
        let violation = if restated.value == violation.value {
            violation
        } else {
            restated_a_clause = true;
            restated
        };
        clauses.push(violation.message());
        violations.push(violation);
    }
    if !restated_a_clause {
        return None;
    }
    // Every other clause is the framework's own sentence over the violation as stated, and
    // the envelope over them is the framework's own too, so the only differences from the
    // refusal that channel would have rendered are the collection a clause about a
    // collection quotes and the member a clause about a map's values is stated at.
    let message = envelope_over(&clauses)?;
    Some(svc.definition().refusal_response(
        ::aws_smithy_schema_server::ProtocolError::Invalid(violations),
        Some(Refused {
            code: stated.refusal_code.to_string(),
            message,
        }),
        req,
    ))
}

/// The MAP member `violation` belongs to and the clause stated at it, or `None` when
/// `violation` is not a map VALUE's.
///
/// A map entry's value is reported by the framework at `<map member>.<key>`, so the member
/// is that path's first segment and the key is everything after it (a key may itself carry a
/// `.`). THREE facts have to hold together before a clause is stated, and each is read
/// rather than assumed:
///
/// * the operation's input declares that member with a constraint set for its
///   collection's elements/values ([`element_constraint_set`], which reads the element
///   constraints codegen attaches to a collection member and nothing else, so no scalar and
///   no structure member can match);
/// * the REQUEST carried that member as an object of strings, in which
/// * `<key>` is one of the keys.
///
/// So a nested structure member whose own name happens to collide with a key keeps the
/// framework's clause, and a LIST — whose element path is `<list>.<position>.member` and
/// whose body is an array — never reaches here.
///
/// The value half is the whole map the request carried, in the order that service states
/// entries in, exactly as a clause about that same member's own `@length` quotes it
/// ([`as_stated`]): a clause quotes the value held at the member it names.
fn map_value_clause<Op: Operation>(
    violation: &Violation,
    submitted: &mut SubmittedMaps<'_>,
    pattern: PatternRendering,
) -> Option<(String, String)> {
    let (member, key) = entry_of(&violation.path)?;
    let declared = element_constraint_set(&[Op::INPUT_SCHEMA], member)?;
    let entries = submitted.of_path(member)?;
    if !entries.iter().any(|(held, _)| held == key) {
        return None;
    }
    let pairs: Vec<(&str, &str)> = entries
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let requirements: Vec<String> = declared
        .into_iter()
        .map(|kind| stated_requirement(kind, pattern))
        .collect();
    Some((
        member.to_string(),
        format!(
            "Value '{}' at '{member}' failed to satisfy constraint: {}",
            quoted_echo(&rendered_map(&pairs)),
            map_value_set_requirement(&requirements),
        ),
    ))
}

/// The member and the entry KEY a path of the form `<member>.<key>` names, or `None` for a
/// path with no step at all. The key is everything after the FIRST step, because a map key
/// may itself carry a `.`.
fn entry_of(path: &str) -> Option<(&str, &str)> {
    path.split_once('.')
}

/// One constraint of a collection's element/value shape as a clause states it: the
/// framework's own words for the class ([`requirement`]), with a `@pattern` rendered as the
/// service renders one — so a requirement nested into a map clause reads exactly like one a
/// reported member gets.
fn stated_requirement(kind: ViolationKind, pattern: PatternRendering) -> String {
    match kind {
        ViolationKind::Pattern(expression) => {
            requirement(ViolationKind::Pattern(pattern(&expression)))
        }
        other => requirement(other),
    }
}

/// `quoted` — the shared rendering of the collection `violation` is about — with a MAP's
/// entries restated in the order the service states them (`order`) rather than in the key
/// order the built input leaves them in.
///
/// Only the ORDER changes: the entries, the braces and the `<key>=<value>` form are the
/// shared rendering's ([`rendered_map`]), and the restated map is taken only when it
/// renders to exactly the length the shared walk had already decided to carry — the same
/// entries in another order, and therefore the same bytes. See the module header for every
/// guard that keeps `quoted` instead.
fn as_stated(
    violation: &Violation,
    quoted: Violation,
    submitted: &mut SubmittedMaps<'_>,
    order: MapEntryOrder,
) -> Violation {
    let Some(rendered) = quoted.value.as_deref() else {
        return quoted;
    };
    let Some(restated) = restated_map(violation, submitted, order) else {
        return quoted;
    };
    if restated.len() != rendered.len() {
        return quoted;
    }
    Violation {
        value: Some(restated),
        ..quoted
    }
}

/// `violations` with every clause about a MAP member quoting the map `req` carried, in the
/// order the service states one (`order`), where the validating walk stored its own summary
/// of that map's SIZE.
///
/// For a service that words its constraint refusal in its own PRE-DISPATCH layer rather
/// than through [`QuotingSubmittedCollections`]: such a layer holds the request, which is
/// where the entry order still is, but it has no echo to restate from — the shared walk
/// renders a map it reaches as the built input's own unordered map, so a clause about one
/// arrives carrying `<n> entries`, which is not a value any caller sent. This quotes it
/// from the body instead, under [`crate::validation::quoted_echo`]'s ceiling, which is the
/// same ceiling the shared walk quotes one clause to.
///
/// Everything but the order is this module's, exactly as for the wrapper: the guards
/// ([`restated_map`]), the rendering ([`rendered_map`]) and the reading
/// ([`submitted_map_entries_at`]) — so a pre-dispatch layer and the wrapper cannot word one
/// collection two ways. The request is read at most once per member path asked for and for
/// at most [`MAX_PARSED_PATHS`] paths ([`SubmittedMaps`]), so the parses one refusal
/// performs are bounded by constants rather than by the violation count a request chooses.
#[must_use]
pub fn quoting_submitted_maps(
    violations: Vec<Violation>,
    req: &RtRequest,
    order: MapEntryOrder,
) -> Vec<Violation> {
    let mut submitted = SubmittedMaps::of(req);
    violations
        .into_iter()
        .map(|violation| {
            match restated_map(&violation, &mut submitted, order) {
                Some(restated) => Violation {
                    value: Some(crate::validation::quoted_echo(&restated).into_owned()),
                    ..violation
                },
                None => violation,
            }
        })
        .collect()
}

/// The MAP the request carried at the member `violation` names, rendered in `order` — or
/// `None` for a clause this states nothing about.
///
/// The one place the guards live, so the two restating paths above cannot disagree about
/// which clause is a map's and which map is the one it is about: the clause has to be a
/// `@length` one whose stored value IS the walk's rendering of a map's size
/// ([`map_entry_count`]), the request has to carry a map of strings at that member's path,
/// its entry count has to be the one the walk counted, and the service's own rule has to
/// state an order for those entries.
fn restated_map(
    violation: &Violation,
    submitted: &mut SubmittedMaps<'_>,
    order: MapEntryOrder,
) -> Option<String> {
    if !matches!(
        violation.kind,
        ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
    ) {
        return None;
    }
    let entries = map_entry_count(violation.value.as_deref()?)?;
    let held = submitted.of_path(&violation.path)?;
    if held.len() != entries {
        return None;
    }
    let ordered = order(held)?;
    Some(rendered_map(&ordered))
}

/// The most distinct member paths ONE refusal reads the body for.
///
/// The parse count has to be bounded by the MODEL and not by the request, and a clause's
/// member path is not always the model's: a constraint broken by one of a map's VALUES is
/// reported at `<map member>.<key>`, where the key is the caller's own text, so a request
/// can present as many distinct paths as it sent entries. This is that bound — well above
/// the map members any emulated input declares, and a constant — so a refusal reads the
/// body a fixed number of times whatever a request chooses. A path past it is answered as
/// a member no map was found at, which keeps the validating walk's own rendering.
const MAX_PARSED_PATHS: usize = 16;

/// The maps the REQUEST carried, each read at most ONCE however many clauses ask for it,
/// and at most [`MAX_PARSED_PATHS`] of them read at all.
///
/// Why it exists: a clause is restated per VIOLATION, and the walk can report one violation
/// per element of a collection, so reading the body inside that loop would re-parse the whole
/// body once per clause — work one request chooses twice over, in its body's length and in
/// its violation count. Here the answer for a member path is remembered the first time it is
/// asked for, the NEGATIVE answer included (a path the body does not carry a map of strings
/// at is never re-read), and only [`MAX_PARSED_PATHS`] paths are read at all — so both the
/// parses one refusal performs and the keys it retains are bounded by constants rather than
/// by anything the request chooses.
struct SubmittedMaps<'r> {
    /// The buffered request body, already read and deserialized once by the shared walk
    /// before this layer looks at it.
    body: &'r [u8],
    /// What the body holds at each member path asked for so far, `None` for a path this
    /// layer found no map of strings at. At most [`MAX_PARSED_PATHS`] entries.
    read: HashMap<String, Option<Vec<(String, String)>>>,
    /// How many more distinct paths this refusal may read the body for.
    budget: usize,
}

impl<'r> SubmittedMaps<'r> {
    /// Read `req`'s body lazily: nothing is parsed until a clause asks for a member.
    fn of(req: &'r RtRequest) -> Self {
        Self {
            body: req.body().bytes().unwrap_or_default(),
            read: HashMap::new(),
            budget: MAX_PARSED_PATHS,
        }
    }

    /// The entries the member at `path` carried, in the order the request wrote them, or
    /// `None` when the body carries no map of strings there — and `None` for every path
    /// past this refusal's [`MAX_PARSED_PATHS`] reads, without reading the body again.
    ///
    /// `path` is the member path a clause is stated at, so a map declared on a nested
    /// structure member is reached at the dotted path it is reported under
    /// ([`submitted_map_entries_at`]) and a top-level one at its own name.
    fn of_path(&mut self, path: &str) -> Option<&[(String, String)]> {
        if !self.read.contains_key(path) {
            if self.budget == 0 {
                return None;
            }
            self.budget -= 1;
            let steps: Vec<&str> = path.split('.').collect();
            let read = submitted_map_entries_at(self.body, &steps);
            self.read.insert(path.to_string(), read);
        }
        self.read.get(path)?.as_deref()
    }
}

/// The entries the request's `member` carried, in the order its JSON object wrote them, or
/// `None` when the body is not an object, carries no such member, or carries one that is
/// not a map of strings.
///
/// The JSON is read by `serde_json`'s own parser, through `serde`'s deserializer seam:
/// nothing here reads the body's bytes itself. The seam is what keeps the ORDER, which a
/// parse into a `Value` would not — that type holds its members in a map of its own.
///
/// It reads the body the server has already buffered and the framework has already
/// deserialized into the operation's input, over the one member a clause is about and
/// skipping every other without building it, so it adds no growth a request can drive
/// beyond that one parse; and what it produces is only used while it renders to the length
/// the shared walk's own bounds had already admitted (see [`as_stated`]).
///
/// The member is matched without regard to case because a violation names it in the
/// lowerCamel path the framework states a clause at, while the body carries the wire name
/// the model declares. A model that declared two members of one shape differing only in
/// case would make the first one the body carries the one read.
#[must_use]
pub fn submitted_map_entries(body: &[u8], member: &str) -> Option<Vec<(String, String)>> {
    submitted_map_entries_at(body, &[member])
}

/// The entries the request carried at `path` — the member names from the body's root down
/// to the map, as the dotted member path a clause is stated at spells them — in the order
/// its JSON object wrote them, or `None` when the body does not carry a map of strings
/// there.
///
/// [`submitted_map_entries`] is this over a path of one step, which is the member a clause
/// at the TOP level of the body names; a map declared on a nested structure member is
/// reported at `<member>.<member>.…`, and that is the whole difference. Every guard, the
/// parser seam that keeps the order and the case-insensitive matching of a member are the
/// same, and so is the one parse per question: the members not on the path are skipped
/// without being built.
#[must_use]
pub fn submitted_map_entries_at(body: &[u8], path: &[&str]) -> Option<Vec<(String, String)>> {
    if path.is_empty() {
        return None;
    }
    let mut deserializer = ::serde_json::Deserializer::from_slice(body);
    ::serde::de::DeserializeSeed::deserialize(MemberEntries { path }, &mut deserializer)
        .ok()
        .flatten()
}

/// Reads the member a PATH names as the string map it holds, keeping the order the
/// document wrote the entries in. Every member off the path is skipped without being
/// built, and a path of more than one step descends through the structure members ahead of
/// the map by reading each of them with this same seed.
struct MemberEntries<'a> {
    path: &'a [&'a str],
}

impl<'de> ::serde::de::DeserializeSeed<'de> for MemberEntries<'_> {
    type Value = Option<Vec<(String, String)>>;

    fn deserialize<D: ::serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> ::serde::de::Visitor<'de> for MemberEntries<'_> {
    type Value = Option<Vec<(String, String)>>;

    fn expecting(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        write!(
            formatter,
            "a JSON object carrying the member {}",
            self.path.first().copied().unwrap_or_default()
        )
    }

    fn visit_map<A: ::serde::de::MapAccess<'de>>(
        self,
        mut members: A,
    ) -> Result<Self::Value, A::Error> {
        let Some((step, rest)) = self.path.split_first() else {
            return Ok(None);
        };
        let mut found: Option<Vec<(String, String)>> = None;
        while let Some(name) = members.next_key::<String>()? {
            if found.is_none() && name.eq_ignore_ascii_case(step) {
                found = if rest.is_empty() {
                    Some(members.next_value::<OrderedStringMap>()?.0)
                } else {
                    // The map is deeper: the member on the path is itself read with this
                    // same seed, over the rest of the path.
                    members.next_value_seed(MemberEntries { path: rest })?
                };
            } else {
                members.next_value::<::serde::de::IgnoredAny>()?;
            }
        }
        Ok(found)
    }
}

/// A JSON object of strings as the entries it holds, in document order.
struct OrderedStringMap(Vec<(String, String)>);

impl<'de> ::serde::Deserialize<'de> for OrderedStringMap {
    fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;

        impl<'de> ::serde::de::Visitor<'de> for Entries {
            type Value = OrderedStringMap;

            fn expecting(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                formatter.write_str("a JSON object of strings")
            }

            fn visit_map<A: ::serde::de::MapAccess<'de>>(
                self,
                mut entries: A,
            ) -> Result<Self::Value, A::Error> {
                let mut held: Vec<(String, String)> = Vec::new();
                while let Some(entry) = entries.next_entry::<String, String>()? {
                    held.push(entry);
                }
                Ok(OrderedStringMap(held))
            }
        }

        deserializer.deserialize_map(Entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: one refusal reads the body for at most [`MAX_PARSED_PATHS`] distinct member
    /// paths, however many clauses ask — a clause stated at `<map member>.<key>` carries
    /// the CALLER's own text in its last step, so the path count is not the model's.
    #[test]
    fn one_refusal_reads_the_body_for_a_bounded_number_of_paths() {
        let body = ::aws_smithy_runtime_api::http::Request::new(
            ::aws_smithy_types::body::SdkBody::from(r#"{"Attributes":{"a":"1"}}"#),
        );
        let mut submitted = SubmittedMaps::of(&body);

        // The member the body does carry a map at answers it, and answers it again
        // without spending a second read.
        for read in 1..=3 {
            assert!(
                submitted.of_path("attributes").is_some(),
                "read {read}: the member carrying a map must be read back"
            );
        }
        assert_eq!(
            submitted.budget,
            MAX_PARSED_PATHS - 1,
            "one member path must spend one read however often it is asked for"
        );

        // Every path a caller could shape spends at most the rest of the budget: past it
        // nothing is read and nothing is kept.
        for index in 0..100 {
            assert!(
                submitted.of_path(&format!("attributes.key-{index}")).is_none(),
                "path {index}: a caller-shaped path carries no map of strings"
            );
        }
        assert_eq!(submitted.budget, 0, "the reads must be spent, not exceeded");
        assert_eq!(
            submitted.read.len(),
            MAX_PARSED_PATHS,
            "at most one remembered answer per read, got {}",
            submitted.read.len()
        );
    }

    /// RULE: the member a clause is about is read back in the order the document wrote it,
    /// whatever else the body carries — and nothing is read where there is nothing to read.
    #[test]
    fn the_named_member_is_read_back_in_document_order() {
        let body = br#"{"Before":"x","Attributes":{"z":"1","a":"2","m":"3"},"After":[1,2]}"#;
        let entries = submitted_map_entries(body, "attributes").expect("the member is a map");
        assert_eq!(
            entries,
            vec![
                ("z".to_string(), "1".to_string()),
                ("a".to_string(), "2".to_string()),
                ("m".to_string(), "3".to_string()),
            ],
            "expected the entries in document order, got {entries:?}"
        );

        // (case, body, member)
        for (case, body, member) in [
            ("no such member", &br#"{"Other":{"a":"1"}}"#[..], "attributes"),
            ("not an object", &br#"[1,2,3]"#[..], "attributes"),
            ("not JSON at all", &b"<Error/>"[..], "attributes"),
            (
                "a member that is not a map of strings",
                &br#"{"Attributes":{"a":1}}"#[..],
                "attributes",
            ),
        ] {
            assert!(
                submitted_map_entries(body, member).is_none(),
                "case {case:?}: expected nothing to be read"
            );
        }
    }

    /// RULE: a map a NESTED member carries is read back at the dotted path a clause names
    /// it by, in document order — and a path that names nothing reads nothing.
    #[test]
    fn a_nested_members_map_is_read_back_at_its_path() {
        let body = br#"{"SamplingRule":{"RuleName":"r","Attributes":{"z":"1","a":"2"}},"Other":3}"#;
        let entries = submitted_map_entries_at(body, &["samplingRule", "attributes"])
            .expect("the nested member is a map");
        assert_eq!(
            entries,
            vec![
                ("z".to_string(), "1".to_string()),
                ("a".to_string(), "2".to_string()),
            ],
            "expected the entries in document order, got {entries:?}"
        );

        // (case, path)
        for (case, path) in [
            ("a step that is not there", &["rule", "attributes"][..]),
            ("a leaf that is not there", &["samplingRule", "tags"][..]),
            ("a leaf that is not a map", &["samplingRule", "ruleName"][..]),
            ("no path at all", &[][..]),
            (
                "a path deeper than the body nests",
                &["samplingRule", "attributes", "z"][..],
            ),
        ] {
            assert!(
                submitted_map_entries_at(body, path).is_none(),
                "case {case:?}: expected nothing to be read"
            );
        }
    }

    /// RULE: the submitted order is the entries as they were read, unchanged — the order a
    /// service whose layer keeps a map's insertion order states.
    #[test]
    fn the_submitted_order_states_the_entries_as_they_were_sent() {
        let entries: Vec<(String, String)> = [("z", "1"), ("a", "2")]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            in_submitted_order(&entries),
            Some(vec![("z", "1"), ("a", "2")]),
            "expected the submitted order"
        );
    }
}
