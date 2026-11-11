// SPDX-License-Identifier: CC0-1.0

//! The two sections of a `_search` body that decide HOW the documents a query selected are
//! answered, rather than WHICH ones it selects: the `sort` the hits are ordered by and the
//! `highlight` fragments each hit carries.
//!
//! Both are read against the index's OWN mappings, because both are about the values an
//! index holds for a field rather than about the text a query carries:
//!
//! * a `sort` key names a field whose per-document values order the hits — and a MULTI-FIELD
//!   is another analysis of the field above it rather than a member the document carries, so
//!   its values are that field's own ([`IndexAnalysis::source_field`]). A field the index
//!   holds no per-document values for — an analyzed text field with `fielddata` left off —
//!   can order nothing, so a sort naming one is refused rather than ignored
//!   ([`Sorting::without_doc_values`]);
//! * a `highlight` field names a field whose own value is handed back with the matched terms
//!   wrapped in tags. What matches is decided by that field's own analysis chain — the one
//!   [`crate::state::analysis`] builds — so an analyzed field wraps exactly the tokens the
//!   query asked about and a field holding its value whole is wrapped only where that whole
//!   value is what the query asked for. A field carrying no asked-for term contributes no
//!   key at all, which is how a caller tells a correctly absent fragment from a field that
//!   produced one.
//!
//! Nothing here invents a value: a sort value is read off the document, and a fragment is
//! the document's own text with the tags the request named inserted around the spans the
//! chain's tokens report.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use ::serde_json::{json, Map, Value};
use ::unicode_segmentation::UnicodeSegmentation;
use aws_smithy_runtime_api::http::Response as RtResponse;

use crate::state::analysis::{IndexAnalysis, Token};
use crate::state::ingest;

use super::{error, scalar_text, Request, Terms, Wanted, PARSING_EXCEPTION};

/// The score every hit of this emulator's search carries: the query vocabulary this surface
/// serves is a filter — every selected document satisfies it exactly — so one score stands
/// for all of them, and it is what a `_score` sort key reads.
pub(super) const MATCH_SCORE: f64 = 1.0;

/// The member a search names the order its hits are answered in, in the body and in the
/// query string alike.
const SORT: &str = "sort";

/// The member one sort key names its direction in, where it carries options rather than the
/// direction itself.
const ORDER: &str = "order";

/// The two directions a sort key reads in, as a request writes them.
const ASCENDING: &str = "asc";
const DESCENDING: &str = "desc";

/// The separator the query-string form of a sort key writes its direction after:
/// `sort=<field>:<direction>`.
const DIRECTION_SEPARATOR: char = ':';

/// The sort keys that name no field of the document: its score, and the order the shard
/// holds it in.
const SCORE_KEY: &str = "_score";
const DOC_KEY: &str = "_doc";

/// How many keys one `sort` may name.
///
/// The emulator's OWN bound on the work one request's ordering costs: every key is read off
/// every document the scan selects, so this is what keeps that work the documents selected
/// rather than their product with a count the request chose.
const MAX_SORT_KEYS: usize = 32;

/// How many fields one `highlight` may ask fragments from, after its patterns have been
/// expanded — the emulator's own bound, for the same reason: each field's value is analyzed
/// per hit.
const MAX_HIGHLIGHT_FIELDS: usize = 32;

/// How many passages one value is broken into while its fragments are chosen — the
/// emulator's own bound on that walk, so a long value costs a constant number of passages.
const MAX_PASSAGES: usize = 1_000;

/// The member a search names the fragments it wants with, and the member that section names
/// the fields it wants them from with.
const HIGHLIGHT: &str = "highlight";
const HIGHLIGHT_FIELDS: &str = "fields";

/// The members a highlight — and one field of it, where that field states its own — carries
/// its options in.
const PRE_TAGS: &str = "pre_tags";
const POST_TAGS: &str = "post_tags";
const REQUIRE_FIELD_MATCH: &str = "require_field_match";
const NUMBER_OF_FRAGMENTS: &str = "number_of_fragments";
const FRAGMENT_SIZE: &str = "fragment_size";

/// The tags a fragment wraps a matched term in when the request names none: "the default
/// pre-tags and post-tags" of the highlighter.
const DEFAULT_PRE_TAG: &str = "<em>";
const DEFAULT_POST_TAG: &str = "</em>";

/// The options' own documented defaults: a fragment of 100 characters, at most 5 of them per
/// field, and only the fields the query itself named.
const DEFAULT_FRAGMENT_SIZE: usize = 100;
const DEFAULT_FRAGMENTS: usize = 5;
const DEFAULT_REQUIRE_FIELD_MATCH: bool = true;

/// The largest fragment one field may ask for, and the most fragments it may ask for — the
/// emulator's own bounds on what one field of one hit materialises.
const MAX_FRAGMENT_SIZE: usize = 10_000;
const MAX_FRAGMENTS: usize = 100;

/// The longest tag a request may wrap a matched term in.
///
/// The emulator's OWN bound, because a tag is inserted once per matched term: what one hit's
/// fragments materialise is the terms they wrap TIMES the tag's own length, so a tag past any
/// the engine wraps with is refused rather than multiplied out.
const MAX_TAG_CHARACTERS: usize = 64;

/// The longest field PATTERN a highlight may name.
///
/// The emulator's own bound beside [`MAX_HIGHLIGHT_FIELDS`], on the other factor of the same
/// product: a pattern is matched against every field the mappings name, and one match costs
/// the pattern's own length.
const MAX_FIELD_PATTERN_CHARACTERS: usize = 256;

/// The most bytes one HIT's fragments may materialise, over every field it answers.
///
/// The emulator's own bound on the response one request can be made to carry: the fragments
/// of a field are the document's own text with a tag around each matched term, and a request
/// asking for the whole field as one fragment (`number_of_fragments: 0`) asks for as much of
/// it as the document holds — so this is what keeps a hit's fragments a constant rather than
/// the product of the value's length and the terms it matched.
const MAX_HIGHLIGHT_BYTES: usize = 64 * 1024;

/// Which way a sort key orders the hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Order {
    Ascending,
    Descending,
}

/// One key of a `sort`: the field whose per-document values order the hits, and which way
/// they order them.
#[derive(Debug, Clone)]
struct SortKey {
    field: String,
    order: Order,
}

/// The order a search answers its hits in — empty for a request that names none, where the
/// hits are answered in the order the scan reaches them.
#[derive(Debug, Clone, Default)]
pub(super) struct Sorting {
    keys: Vec<SortKey>,
}

impl Sorting {
    /// The order `body` names, or the one the query string names where the body names none.
    pub(super) fn of(body: &Value, request: &Request<'_>) -> Result<Self, Box<RtResponse>> {
        let named = match body.get(SORT) {
            Some(named) => named.clone(),
            None => match request.parameter(SORT) {
                Some(named) => Value::String(named),
                None => return Ok(Self::default()),
            },
        };
        let entries: Vec<Value> = match named {
            Value::Array(entries) => entries,
            one => vec![one],
        };
        let mut keys: Vec<SortKey> = Vec::new();
        for entry in entries {
            // Checked BEFORE each key is taken rather than after the entry it came from, so
            // the keys this materialises are bounded by the constant and not by the members
            // one entry happens to carry.
            match entry {
                // A key written as text names a field, and may carry its direction after a
                // colon — which is how the query-string form writes one — while a body names
                // the direction as the field's value.
                Value::String(written) => {
                    for part in written.split(',').filter(|part| !part.is_empty()) {
                        if keys.len() >= MAX_SORT_KEYS {
                            return Err(too_many_sort_keys(keys.len().saturating_add(1)));
                        }
                        keys.push(written_key(part)?);
                    }
                }
                Value::Object(members) => {
                    for (field, options) in &members {
                        if keys.len() >= MAX_SORT_KEYS {
                            return Err(too_many_sort_keys(keys.len().saturating_add(1)));
                        }
                        keys.push(key_of(field, options)?);
                    }
                }
                other => return Err(unparsed_sort(&other)),
            }
        }
        Ok(Self { keys })
    }

    /// Whether this names no order at all.
    pub(super) fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The first key this sort names that the index whose mappings are `analysis` holds no
    /// per-document values for, where it names one: the condition a sort cannot be answered
    /// under.
    pub(super) fn without_doc_values(&self, analysis: &IndexAnalysis) -> Option<&str> {
        self.keys
            .iter()
            .map(|key| key.field.as_str())
            .find(|field| analysis.needs_fielddata(field))
    }

    /// The values this sort reads off one document — one per key, in the order the request
    /// named them. That is both what the order compares and what the hit carries beside its
    /// source, which is what `search_after` paging is built on.
    pub(super) fn values(&self, terms: &Terms<'_>, source: &Value, ordinal: usize) -> Vec<Value> {
        self.keys
            .iter()
            .map(|key| key.value(terms, source, ordinal))
            .collect()
    }

    /// How two documents' sort values order: by the first key they differ on, with a document
    /// holding no value for a key last whichever way that key orders — the `missing` default.
    pub(super) fn compare(&self, left: &[Value], right: &[Value]) -> Ordering {
        for (key, (held, other)) in self.keys.iter().zip(left.iter().zip(right.iter())) {
            let ordering = match (held.is_null(), other.is_null()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    let ordering = value_order(held, other);
                    match key.order {
                        Order::Ascending => ordering,
                        Order::Descending => ordering.reverse(),
                    }
                }
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    }
}

impl SortKey {
    /// The value this key reads off one document: its score, the order the scan reached it
    /// in, or the value the document holds at the key's field.
    ///
    /// A field holding SEVERAL values orders the document by one of them — the smallest on an
    /// ascending key and the largest on a descending one, which is the sort mode's own
    /// default — and a document holding none orders by no value at all.
    fn value(&self, terms: &Terms<'_>, source: &Value, ordinal: usize) -> Value {
        match self.field.as_str() {
            SCORE_KEY => json!(MATCH_SCORE),
            DOC_KEY => json!(ordinal),
            field => {
                let mut best: Option<Value> = None;
                for held in terms.held(source, field) {
                    if held.is_null() {
                        continue;
                    }
                    let better = match &best {
                        None => true,
                        Some(current) => matches!(
                            (value_order(&held, current), self.order),
                            (Ordering::Less, Order::Ascending)
                                | (Ordering::Greater, Order::Descending)
                        ),
                    };
                    if better {
                        best = Some(held);
                    }
                }
                best.unwrap_or(Value::Null)
            }
        }
    }
}

/// The key a sort written as text names: the field, and the direction it may carry after a
/// colon — which is how the query-string form writes one.
///
/// A suffix after that colon naming neither direction is REFUSED, exactly as the same
/// mistake is refused where a body states it: reading it as part of the field name would sort
/// by a field nothing maps, which is an order the caller did not ask for.
fn written_key(written: &str) -> Result<SortKey, Box<RtResponse>> {
    let (field, direction) = match written.rsplit_once(DIRECTION_SEPARATOR) {
        Some((field, direction)) => (field, Some(direction)),
        None => (written, None),
    };
    let order = match direction {
        None => default_order(field),
        Some(ASCENDING) => Order::Ascending,
        Some(DESCENDING) => Order::Descending,
        Some(_) => return Err(unparsed_order()),
    };
    Ok(SortKey {
        field: field.to_string(),
        order,
    })
}

/// The key one member of a sort object states: the field it names, and the direction it
/// gives as its value or under [`ORDER`].
fn key_of(field: &str, options: &Value) -> Result<SortKey, Box<RtResponse>> {
    let direction = match options {
        Value::String(direction) => Some(direction.as_str()),
        Value::Object(_) => match options.get(ORDER) {
            None | Some(Value::Null) => None,
            Some(Value::String(direction)) => Some(direction.as_str()),
            Some(_) => return Err(unparsed_order()),
        },
        Value::Null => None,
        other => return Err(unparsed_sort(other)),
    };
    let order = match direction {
        None => default_order(field),
        Some(ASCENDING) => Order::Ascending,
        Some(DESCENDING) => Order::Descending,
        Some(_) => return Err(unparsed_order()),
    };
    Ok(SortKey {
        field: field.to_string(),
        order,
    })
}

/// The direction a key naming none orders in: a field ascends, while the score descends —
/// the most relevant hit first.
fn default_order(field: &str) -> Order {
    if field == SCORE_KEY {
        Order::Descending
    } else {
        Order::Ascending
    }
}

/// How two values of one sort key order: numbers by their magnitude, text by its code
/// points, booleans with `false` first, and two values of different KINDS by the order of
/// the kinds themselves ([`value_kind`]).
///
/// A TOTAL order, which it has to be: it is what the page's ranking is taken with, and a
/// comparison that put a number before a text, that text before a second number and that
/// number before the first would order no sequence at all. Comparing two kinds by their
/// rendered text is exactly that mistake — `10 < "5"` as text while `9 < 10` as numbers — so
/// the kind decides first and the values are compared only within one kind.
fn value_order(left: &Value, right: &Value) -> Ordering {
    match (left, right) {
        (Value::Number(one), Value::Number(two)) => one
            .as_f64()
            .unwrap_or_default()
            .total_cmp(&two.as_f64().unwrap_or_default()),
        (Value::String(one), Value::String(two)) => one.cmp(two),
        (Value::Bool(one), Value::Bool(two)) => one.cmp(two),
        (one, two) => value_kind(one)
            .cmp(&value_kind(two))
            .then_with(|| scalar_text(one).cmp(&scalar_text(two))),
    }
}

/// How the kinds of value one key's documents may hold rank against each other, so a key
/// whose documents hold values of several kinds still orders them one way.
fn value_kind(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) | Value::Object(_) => 4,
    }
}

/// The refusal a `sort` this surface cannot read earns, worded as the engine words a body
/// whose fields are not the ones the request declares.
fn unparsed_sort(named: &Value) -> Box<RtResponse> {
    Box::new(error(
        400,
        PARSING_EXCEPTION,
        &format!(
            "failed to parse: expected a sort key to name a field, and this one is [{named}]"
        ),
    ))
}

/// The refusal a sort key naming a direction the engine does not read earns.
fn unparsed_order() -> Box<RtResponse> {
    Box::new(error(
        400,
        super::X_CONTENT_PARSE_EXCEPTION,
        &format!("[field_sort] failed to parse field [{ORDER}]"),
    ))
}

/// What a sort naming more keys than [`MAX_SORT_KEYS`] is refused with: the emulator's own
/// bound, stated as the engine's illegal-argument failure.
fn too_many_sort_keys(named: usize) -> Box<RtResponse> {
    Box::new(error(
        400,
        ingest::ILLEGAL_ARGUMENT_EXCEPTION,
        &format!("a search may sort on at most {MAX_SORT_KEYS} keys, and this one names {named}"),
    ))
}

/// What one field's fragments are built with.
#[derive(Debug, Clone)]
struct Options {
    /// What a matched term is wrapped in.
    pre: String,
    post: String,
    /// Whether only the terms the query asked of THIS field produce a fragment.
    require_field_match: bool,
    /// How many fragments one field answers with, `0` for the whole field as one.
    fragments: usize,
    /// How long a fragment is, in characters.
    fragment_size: usize,
}

impl Options {
    /// The options the highlighter applies where nothing names any.
    fn defaults() -> Self {
        Self {
            pre: DEFAULT_PRE_TAG.to_string(),
            post: DEFAULT_POST_TAG.to_string(),
            require_field_match: DEFAULT_REQUIRE_FIELD_MATCH,
            fragments: DEFAULT_FRAGMENTS,
            fragment_size: DEFAULT_FRAGMENT_SIZE,
        }
    }

    /// The options `named` states, falling back to `outer` — the section's own options,
    /// which one field may override member by member.
    ///
    /// A tag the request names is bounded ([`MAX_TAG_CHARACTERS`]) rather than taken as
    /// written: a tag is inserted once per matched term, so its length multiplies what one
    /// hit's fragments materialise, and a tag longer than any the engine wraps with is
    /// refused rather than wrapped around every term of every value.
    fn of(named: Option<&Value>, outer: &Self) -> Result<Self, Box<RtResponse>> {
        let tag = |member: &str, fallback: &String| -> Result<String, Box<RtResponse>> {
            let Some(named) = named.and_then(|options| options.get(member)).and_then(first_tag)
            else {
                return Ok(fallback.clone());
            };
            if named.chars().count() > MAX_TAG_CHARACTERS {
                return Err(Box::new(error(
                    400,
                    ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                    &format!(
                        "a highlight tag may be at most {MAX_TAG_CHARACTERS} characters, and \
                         [{member}] names one of {}",
                        named.chars().count()
                    ),
                )));
            }
            Ok(named)
        };
        let counted = |member: &str, fallback: usize, ceiling: usize| -> usize {
            named
                .and_then(|options| options.get(member))
                .and_then(Value::as_u64)
                .and_then(|counted| usize::try_from(counted).ok())
                .unwrap_or(fallback)
                .min(ceiling)
        };
        Ok(Self {
            pre: tag(PRE_TAGS, &outer.pre)?,
            post: tag(POST_TAGS, &outer.post)?,
            require_field_match: named
                .and_then(|options| options.get(REQUIRE_FIELD_MATCH))
                .and_then(Value::as_bool)
                .unwrap_or(outer.require_field_match),
            fragments: counted(NUMBER_OF_FRAGMENTS, outer.fragments, MAX_FRAGMENTS),
            fragment_size: counted(FRAGMENT_SIZE, outer.fragment_size, MAX_FRAGMENT_SIZE)
                .max(1),
        })
    }
}

/// The tag a `pre_tags`/`post_tags` member names: those members are LISTS, and the first
/// entry is the one this highlighter wraps with, while a request writing one tag on its own
/// names that one.
fn first_tag(named: &Value) -> Option<String> {
    match named {
        Value::String(tag) => Some(tag.clone()),
        Value::Array(tags) => tags.first().and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}

/// The fragments a search asked for: the field patterns it named, each with the options that
/// apply to it.
#[derive(Debug, Clone, Default)]
pub(super) struct Highlighting {
    requested: Vec<(String, Options)>,
}

impl Highlighting {
    /// The highlight `body` asks for, or none at all where it asks for none.
    pub(super) fn of(body: &Value) -> Result<Self, Box<RtResponse>> {
        let Some(highlight) = body.get(HIGHLIGHT) else {
            return Ok(Self::default());
        };
        if !highlight.is_object() {
            return Err(Box::new(error(
                400,
                PARSING_EXCEPTION,
                "failed to parse: expected a highlight to be a JSON object",
            )));
        }
        // The section's own options stand for every field it names, and each field may state
        // its own over them.
        let outer = Options::of(Some(highlight), &Options::defaults())?;
        let mut requested: Vec<(String, Options)> = Vec::new();
        // The bound is checked BEFORE each field is taken, so what one section materialises is
        // the constant rather than the members its body happens to carry.
        let take = |field: &String, options: Option<&Value>, held: &mut Vec<(String, Options)>|
         -> Result<(), Box<RtResponse>> {
            if held.len() >= MAX_HIGHLIGHT_FIELDS {
                return Err(too_many_highlight_fields());
            }
            // A name carrying a PATTERN is matched against every field the mappings of every
            // index the request scans name, so its own length is bounded too: a pattern costs
            // its length per candidate field.
            if (field.contains('*') || field.contains('?'))
                && field.chars().count() > MAX_FIELD_PATTERN_CHARACTERS
            {
                return Err(Box::new(error(
                    400,
                    ingest::ILLEGAL_ARGUMENT_EXCEPTION,
                    &format!(
                        "a highlight field pattern may be at most \
                         {MAX_FIELD_PATTERN_CHARACTERS} characters, and this one is {}",
                        field.chars().count()
                    ),
                )));
            }
            held.push((field.clone(), Options::of(options, &outer)?));
            Ok(())
        };
        match highlight.get(HIGHLIGHT_FIELDS) {
            // A highlight naming no field asks for no fragment, so nothing is highlighted
            // rather than every field being.
            None | Some(Value::Null) => return Ok(Self::default()),
            Some(Value::Object(fields)) => {
                for (field, options) in fields {
                    take(field, Some(options), &mut requested)?;
                }
            }
            Some(Value::Array(fields)) => {
                for entry in fields {
                    match entry {
                        Value::String(field) => take(field, None, &mut requested)?,
                        Value::Object(members) => {
                            for (field, options) in members {
                                take(field, Some(options), &mut requested)?;
                            }
                        }
                        _ => return Err(unparsed_highlight()),
                    }
                }
            }
            Some(_) => return Err(unparsed_highlight()),
        }
        Ok(Self { requested })
    }

    /// Whether this asks for no fragment at all.
    pub(super) fn is_empty(&self) -> bool {
        self.requested.is_empty()
    }

    /// This highlight resolved against ONE index's mappings: the fields each pattern it names
    /// expands to there.
    ///
    /// A name carrying no pattern is taken as written, so a field the mappings map nothing at
    /// — one a write mapped dynamically — is highlighted too; a pattern expands against the
    /// fields the mappings do name, which is the only set of names an index can resolve one
    /// against.
    ///
    /// The expansion STOPS at [`MAX_HIGHLIGHT_FIELDS`] rather than being cut back to it
    /// afterwards, so the candidates one pattern is compared against are bounded by that
    /// constant too and not by the mapping a caller wrote.
    pub(super) fn of_index<'h>(&'h self, analysis: &IndexAnalysis) -> Highlighted<'h> {
        let mut fields: Vec<(String, &Options)> = Vec::new();
        let mut mapped: Option<Vec<String>> = None;
        for (pattern, options) in &self.requested {
            if fields.len() >= MAX_HIGHLIGHT_FIELDS {
                break;
            }
            if !pattern.contains('*') && !pattern.contains('?') {
                fields.push((pattern.clone(), options));
                continue;
            }
            let named = mapped.get_or_insert_with(|| analysis.mapped_fields());
            for field in named {
                if fields.len() >= MAX_HIGHLIGHT_FIELDS {
                    break;
                }
                if ::service_common::iam_authorization::wildcard_match(pattern, field, true) {
                    fields.push((field.clone(), options));
                }
            }
        }
        Highlighted { fields }
    }
}

/// The refusal a `highlight` whose `fields` this surface cannot read earns.
fn unparsed_highlight() -> Box<RtResponse> {
    Box::new(error(
        400,
        super::X_CONTENT_PARSE_EXCEPTION,
        &format!("[{HIGHLIGHT}] failed to parse field [{HIGHLIGHT_FIELDS}]"),
    ))
}

/// What a highlight naming more fields than [`MAX_HIGHLIGHT_FIELDS`] is refused with: the
/// emulator's own bound, stated as the engine's illegal-argument failure.
fn too_many_highlight_fields() -> Box<RtResponse> {
    Box::new(error(
        400,
        ingest::ILLEGAL_ARGUMENT_EXCEPTION,
        &format!("a search may highlight at most {MAX_HIGHLIGHT_FIELDS} fields"),
    ))
}

/// A highlight resolved against one index: the fields it reads fragments off there.
#[derive(Debug)]
pub(super) struct Highlighted<'h> {
    fields: Vec<(String, &'h Options)>,
}

impl Highlighted<'_> {
    /// The fields these fragments are read off, whose own indexing chains the scan therefore
    /// resolves beside the ones the query's clauses name.
    pub(super) fn fields(&self) -> BTreeSet<String> {
        self.fields.iter().map(|(field, _)| field.clone()).collect()
    }

    /// The `highlight` object one hit carries, or `None` where no field it names produced a
    /// fragment — a field holding no term the query asked about contributes no key at all.
    ///
    /// `wanted` is what the query asks of each field, as THIS index's chains analyze it
    /// ([`super::Selector::wanted`]); `terms` is that index's own analysis.
    ///
    /// What ONE hit's fragments may materialise is bounded over every field together
    /// ([`MAX_HIGHLIGHT_BYTES`]): a fragment is the document's own text with a tag around each
    /// matched term, so without that allowance a request asking for whole fields of long
    /// values would choose how large one hit is.
    pub(super) fn fragments(
        &self,
        terms: &Terms<'_>,
        wanted: &Wanted,
        source: &Value,
    ) -> Option<Value> {
        let mut highlighted = Map::new();
        let mut spent = 0_usize;
        for (field, options) in &self.fields {
            let allowance = MAX_HIGHLIGHT_BYTES.saturating_sub(spent);
            if allowance == 0 {
                break;
            }
            let asked = asked_terms(wanted, field, options.require_field_match);
            if asked.is_empty() {
                continue;
            }
            let mut fragments: Vec<Value> = Vec::new();
            let mut taken = 0_usize;
            for held in terms.held(source, field) {
                let text = scalar_text(&held);
                // A field the index holds its value WHOLE for carries one term, the value
                // itself, so it is wrapped only where that whole value is what was asked
                // for — which is why an analyzed-token query highlights no keyword sibling.
                if terms.holds_whole_values(field) {
                    if asked.contains(text.as_str()) {
                        let fragment = format!("{}{text}{}", options.pre, options.post);
                        taken = taken.saturating_add(fragment.len());
                        fragments.push(Value::String(fragment));
                    }
                    continue;
                }
                let Some(tokens) = terms.analyzed_tokens(field, &text) else {
                    continue;
                };
                for fragment in wrapped(
                    &text,
                    &tokens,
                    &asked,
                    options,
                    allowance.saturating_sub(taken),
                ) {
                    taken = taken.saturating_add(fragment.len());
                    fragments.push(Value::String(fragment));
                }
                if taken >= allowance || (options.fragments > 0 && fragments.len() >= options.fragments)
                {
                    break;
                }
            }
            spent = spent.saturating_add(taken);
            if fragments.is_empty() {
                continue;
            }
            if options.fragments > 0 {
                fragments.truncate(options.fragments);
            }
            highlighted.insert(field.clone(), Value::Array(fragments));
        }
        (!highlighted.is_empty()).then_some(Value::Object(highlighted))
    }
}

/// The terms a field's fragments are wrapped around: the ones the query asked of THAT field,
/// or — where the request turned `require_field_match` off — the ones it asked of any field.
fn asked_terms<'w>(wanted: &'w Wanted, field: &str, require_field_match: bool) -> BTreeSet<&'w str> {
    if require_field_match {
        return wanted
            .get(field)
            .map(|asked| asked.iter().map(String::as_str).collect())
            .unwrap_or_default();
    }
    wanted
        .values()
        .flatten()
        .map(String::as_str)
        .collect()
}

/// The fragments one value produces: the passages of it that carry a term that was asked
/// for, each handed back verbatim with those terms wrapped in the options' tags.
///
/// The spans wrapped are the ones the field's own chain reports for its tokens, so exactly
/// the matched token is wrapped and the rest of the value is answered as the document holds
/// it.
///
/// `allowance` is how many bytes these fragments may materialise together, which is what the
/// hit has left of [`MAX_HIGHLIGHT_BYTES`]: the wrapping stops at it rather than wrapping
/// every term of however long a value the index holds.
fn wrapped(
    text: &str,
    tokens: &[Token],
    asked: &BTreeSet<&str>,
    options: &Options,
    allowance: usize,
) -> Vec<String> {
    // A token's offsets are counted in CHARACTERS while the text is sliced by BYTES, so the
    // text is indexed once here and every offset below is a byte offset into it.
    let bytes: Vec<usize> = text
        .char_indices()
        .map(|(at, _)| at)
        .chain([text.len()])
        .collect();
    let mut matched: Vec<(usize, usize)> = Vec::new();
    for token in tokens {
        if !asked.contains(token.token.as_str()) {
            continue;
        }
        let (Some(start), Some(end)) = (bytes.get(token.start_offset), bytes.get(token.end_offset))
        else {
            continue;
        };
        if end > start {
            matched.push((*start, *end));
        }
    }
    if matched.is_empty() {
        return Vec::new();
    }
    matched.sort_unstable();
    // Two tokens may report ONE span — a synonym carries the offsets of the token it stands
    // for — so the spans are merged before anything is wrapped, and no span is wrapped twice.
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for (start, end) in matched {
        match spans.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => spans.push((start, end)),
        }
    }
    // `number_of_fragments: 0` asks for the whole field as one fragment, which is what the
    // highlighter answers it with; otherwise the value is broken into passages and the ones
    // carrying a span are the fragments.
    let windows = if options.fragments == 0 {
        vec![(0, text.len())]
    } else {
        passages(text, options.fragment_size)
    };
    let mut fragments: Vec<String> = Vec::new();
    // The spans and the passages are both in the order they appear in the text, so ONE pass
    // over the spans serves every passage: each span is reached once, where the passage
    // holding its start is rendered. That is what keeps this the length of the value rather
    // than the product of the passages it breaks into and the terms it matched.
    let mut pending: &[(usize, usize)] = &spans;
    let mut taken = 0_usize;
    for (start, end) in windows {
        if options.fragments > 0 && fragments.len() >= options.fragments {
            break;
        }
        let room = allowance.saturating_sub(taken);
        if room == 0 {
            break;
        }
        // A span that began before this passage was already answered with the passage it
        // starts in, or belongs to one the caller's fragment count stopped before.
        while pending.first().is_some_and(|(from, _)| *from < start) {
            pending = pending.get(1..).unwrap_or_default();
        }
        let inside = pending.iter().take_while(|(from, _)| *from < end).count();
        let (wrap, rest) = pending.split_at(inside.min(pending.len()));
        pending = rest;
        if wrap.is_empty() {
            continue;
        }
        let mut cursor = start;
        let mut rendered = String::new();
        for (from, to) in wrap {
            // A span is wrapped within the passage its start falls in, so one that reaches
            // past that passage's end is wrapped up to it rather than dropped.
            let to = (*to).min(end);
            if *from < cursor || to <= *from {
                continue;
            }
            // The fragment ends where the hit's allowance does, so what one value
            // materialises is bounded however many terms it matched.
            if rendered.len() >= room {
                break;
            }
            rendered.push_str(text.get(cursor..*from).unwrap_or_default());
            rendered.push_str(&options.pre);
            rendered.push_str(text.get(*from..to).unwrap_or_default());
            rendered.push_str(&options.post);
            cursor = to;
        }
        if cursor == start {
            continue;
        }
        let tail = text.get(cursor..end).unwrap_or_default();
        rendered.push_str(kept(tail, room.saturating_sub(rendered.len())));
        let fragment = rendered.trim();
        if !fragment.is_empty() {
            taken = taken.saturating_add(fragment.len());
            fragments.push(fragment.to_string());
        }
    }
    fragments
}

/// As much of `text` as `room` bytes hold, ending on a character boundary — what a fragment
/// keeps of the value's own tail once the hit's allowance is nearly spent.
fn kept(text: &str, room: usize) -> &str {
    if text.len() <= room {
        return text;
    }
    let end = text
        .char_indices()
        .map(|(at, character)| at.saturating_add(character.len_utf8()))
        .take_while(|end| *end <= room)
        .last()
        .unwrap_or(0);
    text.get(..end).unwrap_or_default()
}

/// The passages a value is broken into: its sentences, grouped until a passage is at least
/// `fragment_size` characters long — which is what the option names, "the size of the
/// highlighted fragment in characters".
///
/// The sentence boundaries are UAX#29's, read through the same crate the standard tokenizer's
/// word boundaries come from, so this module states no grammar of its own. Bounded by
/// [`MAX_PASSAGES`].
fn passages(text: &str, fragment_size: usize) -> Vec<(usize, usize)> {
    let mut passages: Vec<(usize, usize)> = Vec::new();
    let mut from: Option<usize> = None;
    let mut counted = 0_usize;
    for (at, sentence) in text.split_sentence_bound_indices() {
        let start = *from.get_or_insert(at);
        counted = counted.saturating_add(sentence.chars().count());
        if counted >= fragment_size {
            passages.push((start, at.saturating_add(sentence.len())));
            from = None;
            counted = 0;
            if passages.len() >= MAX_PASSAGES {
                return passages;
            }
        }
    }
    if let Some(start) = from {
        passages.push((start, text.len()));
    }
    passages
}

/// The members a hit carries what these two sections produced in: the values the sort keys
/// read off it, and the fragments its fields produced.
pub(super) const SORT_MEMBER: &str = SORT;
pub(super) const HIGHLIGHT_MEMBER: &str = HIGHLIGHT;

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a value is broken into passages at its sentence boundaries, grouped until each
    /// passage is at least `fragment_size` characters long.
    #[test]
    fn passages_group_sentences_up_to_the_fragment_size() {
        let text = "One. Two. Three.";
        let whole = passages(text, 100);
        assert_eq!(
            whole.len(),
            1,
            "case \"a value shorter than one fragment\": expected one passage, got {whole:?}"
        );
        let split = passages(text, 5);
        assert!(
            split.len() > 1,
            "case \"a fragment size shorter than the value\": expected several passages, got \
             {split:?}"
        );
        for (from, to) in &split {
            assert!(
                text.get(*from..*to).is_some(),
                "case \"a passage's bounds\": expected a slice of the text, got {from}..{to}"
            );
        }
    }

    /// RULE: exactly the asked-for token is wrapped, and the rest of the value is answered
    /// verbatim.
    #[test]
    fn only_the_asked_for_token_is_wrapped() {
        let text = "the quick brown fox";
        let tokens: Vec<Token> = text
            .split(' ')
            .scan(0_usize, |at, word| {
                let start = *at;
                *at += word.chars().count() + 1;
                Some(Token {
                    token: word.to_string(),
                    start_offset: start,
                    end_offset: start + word.chars().count(),
                    kind: crate::state::analysis::TOKEN_TYPE_ALPHANUM,
                    position: 0,
                })
            })
            .collect();
        let asked: BTreeSet<&str> = BTreeSet::from(["brown"]);
        let fragments = wrapped(
            text,
            &tokens,
            &asked,
            &Options::defaults(),
            MAX_HIGHLIGHT_BYTES,
        );
        assert_eq!(
            fragments,
            vec!["the quick <em>brown</em> fox".to_string()],
            "case \"one matched token\": expected the token wrapped in the default tags, got \
             {fragments:?}"
        );
        // RULE: a fragment stops once the allowance the hit has left is spent — so one with
        // almost none left carries the span it was already wrapping and none of the value's
        // tail, where the unbounded one above carries all of it.
        let bounded = wrapped(text, &tokens, &asked, &Options::defaults(), 4);
        assert_eq!(
            bounded,
            vec!["the quick <em>brown</em>".to_string()],
            "case \"an allowance of 4 bytes\": expected the fragment to stop at it, got \
             {bounded:?}"
        );
    }
}
