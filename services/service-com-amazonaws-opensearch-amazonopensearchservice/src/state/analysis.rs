// SPDX-License-Identifier: CC0-1.0

//! The ANALYSIS a domain's cluster runs over text: the tokenizer and the token filters an
//! index's analysis settings put together, the dictionary file an associated package mounts
//! for one of those filters to read, and the token stream the chain produces.
//!
//! An index names its text pipeline in its own settings — `analysis.analyzer.<name>` puts a
//! tokenizer and a list of filters together, `analysis.filter.<name>` configures one filter
//! — and a field's mapping names which analyzer its text goes through. That chain decides
//! two things a client observes, and both are COMPUTED from it here rather than guessed at:
//!
//! * what the analyze API reports for a text, token by token, and
//! * which documents a `match` query selects, because a `match` is evaluated over the TERMS
//!   the field's analyzer produces — for the query's text and for the document's alike.
//!
//! A filter may name its dictionary BY PATH instead of inline (`stopwords_path`,
//! `synonyms_path`), and that path is the mount point of a custom package the domain holds
//! in place (`analyzers/<PackageID>`) — so the content a `CreatePackage` imported is what
//! the filter reads ([`PackageFiles`]). An index whose settings name a path the domain
//! mounts nothing at is refused where it is created
//! ([`Domain::unresolved_package_reference`]), so a chain built here is built from files the
//! domain does hold; a path that resolves to a package whose content this process never
//! read contributes no entries, which is all that can be said about a file nothing saw.
//!
//! What the chain is built FROM is the engine's own catalog, reached through maintained
//! crates rather than restated here: the standard tokenizer's boundaries are UAX#29's (the
//! `unicode-segmentation` crate), a language analyzer's function words are the published
//! per-language lists (`stop-words`) and its stemmer is Snowball's (`rust-stemmers`). A
//! language those two catalogs carry neither a list nor a stemmer for is tokenized and
//! folded to lower case only — what this engine can genuinely run for it — and never given
//! invented output.

use super::analyzers::{ANALYSIS_ANALYZERS, MAPPING_ANALYZER_MEMBERS};
use super::indexes::{
    referenced_package, sections_named, MAPPING_CONTAINERS, MAPPING_TYPE, SCHEMA_ANALYSIS,
};
use super::*;

use ::serde_json::{Map, Value};
use ::unicode_segmentation::UnicodeSegmentation;

/// The analyzer an index uses for a field that names none, and the one the analyze API
/// reports with when neither the request nor the index names one.
pub const DEFAULT_ANALYZER: &str = "standard";

/// The `type` an analyzer definition carries when it composes a tokenizer and filters of
/// its own rather than naming one of the engine's built-in analyzers.
const CUSTOM_ANALYZER_TYPE: &str = "custom";

/// The token type the standard tokenizer gives a token of letters or digits.
pub const TOKEN_TYPE_ALPHANUM: &str = "<ALPHANUM>";

/// The token type the standard tokenizer gives a token made only of digits.
pub const TOKEN_TYPE_NUM: &str = "<NUM>";

/// The token type every other tokenizer here gives its tokens.
pub const TOKEN_TYPE_WORD: &str = "word";

/// The token type a synonym filter gives a token it put in place of, or beside, another.
pub const TOKEN_TYPE_SYNONYM: &str = "SYNONYM";

/// The most tokens one analysis produces.
///
/// The engine's own ceiling on the same work, which it publishes as the index-level setting
/// `index.analyze.max_token_count` and defaults to this number: text past it is not
/// analyzed, so the answer is not sized by the text one request carries.
pub const MAX_TOKEN_COUNT: usize = 10_000;

/// The most tokens one analysis MATERIALISES: one past the published ceiling, so a stream
/// that would run beyond it is recognisable as one that did rather than as one that ended
/// exactly at the limit — which is what lets the analyze API refuse it the way the engine
/// does instead of reporting part of an analysis as the whole.
const TOKEN_CEILING: usize = MAX_TOKEN_COUNT + 1;

/// The most entries one dictionary file contributes.
///
/// The emulator's OWN bound on how much of a mounted file one analysis reads, so the work a
/// request causes is not sized by the content a package happens to carry.
pub const MAX_DICTIONARY_ENTRIES: usize = 10_000;

/// The most terms ONE line of a dictionary file contributes to either side of a rule.
///
/// The emulator's own bound beside [`MAX_DICTIONARY_ENTRIES`], which bounds the number of
/// LINES: without it one line could carry as many terms as the file has bytes.
const MAX_TERMS_PER_RULE: usize = 100;

/// The most synonyms one term stands for, however many rules name it.
///
/// The emulator's own bound on what a single token can expand into, so the stream one
/// request produces is not sized by the dictionary's shape.
const MAX_SYNONYMS_PER_TERM: usize = 100;

/// The most input terms ONE synonym filter indexes, and the most synonym strings it holds
/// in total.
///
/// The AGGREGATE bound beside the per-line ones: a dictionary may hold
/// [`MAX_DICTIONARY_ENTRIES`] lines and a line [`MAX_TERMS_PER_RULE`] terms on each side,
/// so without a cap on the index as a whole their product would size what one filter holds
/// in this process. Ingestion stops at the cap, so a filter costs these constants and not
/// the content a caller wrote — and the dedup each output is checked against is then a scan
/// of at most [`MAX_SYNONYMS_PER_TERM`] entries.
const MAX_SYNONYM_TERMS: usize = 10_000;

/// The most synonym strings one filter stores across every term. See
/// [`MAX_SYNONYM_TERMS`].
const MAX_SYNONYM_STRINGS: usize = 20_000;

/// The most candidate synonyms one filter's index CONSIDERS while it is built.
///
/// The bound on the WORK, beside the bounds on what is stored: a dictionary whose lines
/// repeat stores nothing and so never reaches a storage bound, while each of its lines is
/// still read — so this is what stops ingestion whatever the content turns out to hold.
const MAX_EXAMINED_SYNONYMS: usize = 100_000;

/// The most filters one analyzer chain may name — the emulator's own bound on the chain one
/// analysis walks.
const MAX_FILTERS: usize = 32;

/// One token of an analyzed text, as the analyze API reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The token's own text, after every filter of the chain has acted on it.
    pub token: String,
    /// Where in the text the token started, counted in characters.
    pub start_offset: usize,
    /// Where in the text the token ended, counted in characters.
    pub end_offset: usize,
    /// The token's type, as the tokenizer or the filter that produced it names it.
    pub kind: &'static str,
    /// The token's position in the stream. A filter that drops a token leaves its position
    /// behind — which is what makes a dropped stop word observable as a gap — and a synonym
    /// carries the position of the token it stands for.
    pub position: usize,
}

/// The dictionary files the packages in place on a domain mount, keyed by the `PackageID`
/// whose mount point names them.
///
/// A TXT-DICTIONARY package's content is ONE file, mounted at `analyzers/<PackageID>`, so a
/// path an analysis setting names resolves to the package that id names
/// ([`referenced_package`]) and then to that package's content. Lines are kept as the file
/// holds them; what a line MEANS is the reading of the filter that asked for it
/// ([`word_list`], [`synonym_rules`]).
#[derive(Debug, Clone, Default)]
pub struct PackageFiles {
    /// `PackageID` -> the lines of the file that package mounts.
    mounted: BTreeMap<String, Vec<String>>,
}

impl PackageFiles {
    /// Mount `content` as the file the package `package_id` provides.
    ///
    /// Blank lines and `#` comments are dropped as they are read — how the engine's own
    /// word-list loader reads a dictionary file — so no filter has to do it again.
    pub fn mount(&mut self, package_id: &str, content: &str) {
        let lines: Vec<String> = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .take(MAX_DICTIONARY_ENTRIES)
            .map(str::to_owned)
            .collect();
        self.mounted.insert(package_id.to_string(), lines);
    }

    /// The lines of the file `path` names, or `None` where nothing is mounted there.
    pub fn at(&self, path: &str) -> Option<&[String]> {
        let package_id = referenced_package(path)?;
        self.mounted.get(&package_id).map(Vec::as_slice)
    }

    /// Whether this domain mounts no file at all.
    pub fn is_empty(&self) -> bool {
        self.mounted.is_empty()
    }
}

/// The files the packages IN PLACE on `domain` mount, read from the packages the account
/// holds.
///
/// A package is in place from the moment its association landed until the record leaves the
/// domain ([`PackageAssociation::in_place`]), and the content in place is the version the
/// association pinned — a later import of the package's content does not change what a
/// domain already has.
///
/// Reads ONLY. Call it AFTER the clock's owed moves are materialised, so an association
/// that has landed is in place and one whose removal has fallen due is already gone.
pub fn package_files(packages: &BTreeMap<String, Package>, domain: &Domain) -> PackageFiles {
    let mut files = PackageFiles::default();
    for (package_id, association) in &domain.package_associations {
        if !association.in_place() {
            continue;
        }
        let Some(package) = packages.get(package_id) else {
            continue;
        };
        if let Some(content) = package.content_at(&association.package_version) {
            files.mount(package_id, content);
        }
    }
    files
}

/// What splits a text into tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tokenizer {
    /// `standard`: UAX#29 word boundaries.
    Standard,
    /// `whitespace`: runs of non-whitespace characters.
    Whitespace,
    /// `letter`: runs of letters.
    Letter,
    /// `lowercase`: runs of letters, folded to lower case.
    Lowercase,
    /// `keyword`: the whole text as one token.
    Keyword,
    /// `pattern` with its default expression `\W+`: runs of word characters — letters,
    /// digits and the underscore.
    NonWord,
}

impl Tokenizer {
    /// The tokenizer `name` names, or `None` where this engine registers none under it.
    ///
    /// `classic` and `uax_url_email` are the standard tokenizer's own word boundaries —
    /// what sets them apart is which runs they keep WHOLE (an acronym, a URL, an email
    /// address), and a text carrying none of those is split identically, so they resolve to
    /// that tokenizer here. `simple_pattern` resolves to NO tokenizer: its own default
    /// expression matches nothing at all, so answering it as another tokenizer's output
    /// would be answering a chain the caller did not ask for.
    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "standard" | "classic" | "uax_url_email" => Self::Standard,
            "whitespace" => Self::Whitespace,
            "letter" => Self::Letter,
            "lowercase" => Self::Lowercase,
            "keyword" => Self::Keyword,
            "pattern" => Self::NonWord,
            _ => return None,
        })
    }

    /// Whether `ch` is part of a token of this tokenizer, for the ones that split on a
    /// character class.
    fn admits(self, ch: char) -> bool {
        match self {
            Self::Whitespace => !ch.is_whitespace(),
            Self::Letter | Self::Lowercase => ch.is_alphabetic(),
            Self::NonWord => ch.is_alphanumeric() || ch == '_',
            // Neither splits on a character class: both are answered in [`Self::split`].
            Self::Standard | Self::Keyword => false,
        }
    }

    /// The tokens `text` splits into, each carrying where it came from and its position.
    fn split(self, text: &str) -> Vec<Token> {
        match self {
            Self::Keyword => {
                let characters = text.chars().count();
                vec![Token {
                    token: text.to_string(),
                    start_offset: 0,
                    end_offset: characters,
                    kind: TOKEN_TYPE_WORD,
                    position: 0,
                }]
            }
            Self::Standard => {
                // The offsets are counted in CHARACTERS while the iterator reports BYTE
                // positions, so the text is walked once with a cursor that only moves
                // forward: counting from the start per token would make one analysis cost
                // the square of the text's length. `get` rather than an index, so a
                // position this walk did not produce could only shorten the count rather
                // than panic.
                let mut tokens: Vec<Token> = Vec::new();
                let mut counted_to = 0_usize;
                let mut characters = 0_usize;
                for (at, word) in text.unicode_word_indices().take(TOKEN_CEILING) {
                    characters += text
                        .get(counted_to..at)
                        .map_or(0, |gap| gap.chars().count());
                    let start_offset = characters;
                    characters += word.chars().count();
                    counted_to = at.saturating_add(word.len());
                    tokens.push(Token {
                        token: word.to_string(),
                        start_offset,
                        end_offset: characters,
                        kind: if word.chars().all(char::is_numeric) {
                            TOKEN_TYPE_NUM
                        } else {
                            TOKEN_TYPE_ALPHANUM
                        },
                        position: tokens.len(),
                    });
                }
                tokens
            }
            class => {
                let mut tokens: Vec<Token> = Vec::new();
                let mut current = String::new();
                let mut started_at = 0_usize;
                let mut offset = 0_usize;
                for ch in text.chars() {
                    if class.admits(ch) {
                        if current.is_empty() {
                            started_at = offset;
                        }
                        current.push(ch);
                    } else if !current.is_empty() {
                        class.flush(&mut tokens, &mut current, started_at, offset);
                    }
                    offset += 1;
                }
                class.flush(&mut tokens, &mut current, started_at, offset);
                tokens
            }
        }
    }

    /// Take the token being built and append it, up to [`TOKEN_CEILING`].
    fn flush(
        self,
        tokens: &mut Vec<Token>,
        current: &mut String,
        start_offset: usize,
        end_offset: usize,
    ) {
        if current.is_empty() || tokens.len() >= TOKEN_CEILING {
            current.clear();
            return;
        }
        let token = ::std::mem::take(current);
        tokens.push(Token {
            token: if matches!(self, Self::Lowercase) {
                token.to_lowercase()
            } else {
                token
            },
            start_offset,
            end_offset,
            kind: TOKEN_TYPE_WORD,
            position: tokens.len(),
        });
    }
}

/// One rule of a synonym file or list, in the engine's own (Solr) format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynonymRule {
    /// The terms the rule acts on.
    inputs: Vec<String>,
    /// The terms it puts in their place, or beside them.
    outputs: Vec<String>,
    /// Whether the rule REPLACES its input (`a => b`) rather than making every term of a
    /// group stand for the others (`a, b`).
    replaces: bool,
}

/// What a `synonym` filter puts in place of, or beside, one token: the rules INDEXED by the
/// input term they act on.
///
/// Indexed rather than kept as a list because a filter runs over every token of the stream:
/// a lookup per token makes the work one request causes the SIZE of its text, where a scan
/// over every rule would make it the product of that text and the whole dictionary. Built
/// once per filter, where the chain is resolved.
#[derive(Debug, Clone, Default)]
pub struct Synonyms {
    /// The input term -> what it stands for.
    by_term: BTreeMap<String, Expansion>,
}

/// What one input term expands to.
#[derive(Debug, Clone, Default)]
struct Expansion {
    /// The terms put in its place, or beside it, each once and in the order the rules name
    /// them. Bounded by [`MAX_SYNONYMS_PER_TERM`].
    outputs: Vec<String>,
    /// Whether a rule naming this term REPLACES it.
    replaces: bool,
}

impl Synonyms {
    /// The rules `rules` states, indexed by the term each acts on.
    ///
    /// An output of several words becomes one term per word, as the graph form of the
    /// filter places them in the stream — done once here rather than per token.
    ///
    /// Both what this HOLDS and what building it COSTS are bounded: ingestion stops at
    /// [`MAX_SYNONYM_TERMS`] indexed terms, [`MAX_SYNONYM_STRINGS`] stored synonyms or
    /// [`MAX_EXAMINED_SYNONYMS`] candidates considered, whichever comes first — the last of
    /// those is what bounds a dictionary whose lines repeat, where nothing is stored and so
    /// no storage bound is ever reached. Each candidate is tested against a SET of what the
    /// term already stands for, so a candidate costs a lookup rather than a scan of the
    /// terms held.
    fn of(rules: Vec<SynonymRule>) -> Self {
        let mut building: BTreeMap<String, (Expansion, BTreeSet<String>)> = BTreeMap::new();
        let mut stored = 0_usize;
        let mut examined = 0_usize;
        'ingest: for rule in rules {
            for input in &rule.inputs {
                if stored >= MAX_SYNONYM_STRINGS
                    || examined >= MAX_EXAMINED_SYNONYMS
                    || (building.len() >= MAX_SYNONYM_TERMS && !building.contains_key(input))
                {
                    break 'ingest;
                }
                let (expansion, held) = building.entry(input.clone()).or_default();
                expansion.replaces |= rule.replaces;
                for output in rule
                    .outputs
                    .iter()
                    .flat_map(|output| output.split_whitespace())
                {
                    examined += 1;
                    if expansion.outputs.len() >= MAX_SYNONYMS_PER_TERM
                        || stored >= MAX_SYNONYM_STRINGS
                        || examined >= MAX_EXAMINED_SYNONYMS
                    {
                        break;
                    }
                    if output != input.as_str() && !held.contains(output) {
                        held.insert(output.to_string());
                        expansion.outputs.push(output.to_string());
                        stored += 1;
                    }
                }
            }
        }
        Self {
            by_term: building
                .into_iter()
                .map(|(term, (expansion, _held))| (term, expansion))
                .collect(),
        }
    }
}

/// What one filter of a chain does to the token stream.
#[derive(Debug, Clone)]
pub enum Filter {
    /// `lowercase`.
    Lowercase,
    /// `uppercase`.
    Uppercase,
    /// `trim`: each token's surrounding whitespace.
    Trim,
    /// `stop`: drop the tokens the list holds, leaving their positions behind.
    Stop(BTreeSet<String>),
    /// `synonym` / `synonym_graph`.
    Synonym(Synonyms),
    /// `stemmer` / `snowball`, at the named language's Snowball algorithm. The language is
    /// held by NAME and the stemmer built where the filter runs; a filter is only BUILT for
    /// a language this process ships an algorithm for ([`configured_filter`]), so a chain
    /// that holds this filter is one whose stemming really runs.
    Stem(String),
    /// `length`: keep the tokens whose character count is within the bounds.
    Length { min: usize, max: usize },
    /// `fingerprint`: the distinct tokens, sorted and joined, as one token.
    Fingerprint,
}

impl Filter {
    /// The stream `tokens` becomes after this filter has acted on it.
    fn apply(&self, tokens: Vec<Token>) -> Vec<Token> {
        match self {
            Self::Lowercase => folded(tokens, str::to_lowercase),
            Self::Uppercase => folded(tokens, str::to_uppercase),
            Self::Trim => folded(tokens, |token| token.trim().to_string()),
            Self::Stop(words) => tokens
                .into_iter()
                .filter(|token| !words.contains(&token.token))
                .collect(),
            Self::Synonym(synonyms) => expanded(tokens, synonyms),
            Self::Stem(language) => match stemmer(language) {
                Some(stemmer) => folded(tokens, |token| stemmer.stem(token).to_string()),
                None => tokens,
            },
            Self::Length { min, max } => tokens
                .into_iter()
                .filter(|token| {
                    let characters = token.token.chars().count();
                    characters >= *min && characters <= *max
                })
                .collect(),
            Self::Fingerprint => fingerprint(tokens),
        }
    }
}

/// `tokens` with `fold` applied to each token's text, dropping one folded to nothing.
fn folded(tokens: Vec<Token>, fold: impl Fn(&str) -> String) -> Vec<Token> {
    tokens
        .into_iter()
        .filter_map(|mut token| {
            token.token = fold(&token.token);
            (!token.token.is_empty()).then_some(token)
        })
        .collect()
}

/// `tokens` with every term's synonyms put in place of, or beside, the tokens they stand
/// for.
///
/// A term is matched against ONE token: a multi-word input is written as the whole phrase
/// it is and so matches no single token, which is what this engine can decide from a token
/// stream. An output of several words is emitted as one token per word, each at the
/// position of the token it stands for, as the graph form of the filter places them.
///
/// The stream is bounded as it is built, not after: every push is checked against
/// [`TOKEN_CEILING`], so a dictionary that expands each token many times cannot size what
/// one request materialises.
fn expanded(tokens: Vec<Token>, synonyms: &Synonyms) -> Vec<Token> {
    let mut expanded: Vec<Token> = Vec::new();
    for token in tokens {
        let Some(expansion) = synonyms.by_term.get(&token.token) else {
            if expanded.len() >= TOKEN_CEILING {
                return expanded;
            }
            expanded.push(token);
            continue;
        };
        let (position, start_offset, end_offset) =
            (token.position, token.start_offset, token.end_offset);
        if !expansion.replaces {
            expanded.push(token);
        }
        for synonym in &expansion.outputs {
            if expanded.len() >= TOKEN_CEILING {
                return expanded;
            }
            expanded.push(Token {
                token: synonym.clone(),
                start_offset,
                end_offset,
                kind: TOKEN_TYPE_SYNONYM,
                position,
            });
        }
    }
    expanded
}

/// The one token a fingerprint filter leaves: the distinct tokens, sorted, joined by a
/// single space.
fn fingerprint(tokens: Vec<Token>) -> Vec<Token> {
    if tokens.is_empty() {
        return tokens;
    }
    let end_offset = tokens
        .iter()
        .map(|token| token.end_offset)
        .max()
        .unwrap_or_default();
    let distinct: BTreeSet<String> = tokens.into_iter().map(|token| token.token).collect();
    vec![Token {
        token: distinct.into_iter().collect::<Vec<_>>().join(" "),
        start_offset: 0,
        end_offset,
        kind: TOKEN_TYPE_WORD,
        position: 0,
    }]
}

/// One analysis chain: what splits the text, and what acts on the tokens.
#[derive(Debug, Clone)]
pub struct Analyzer {
    tokenizer: Tokenizer,
    filters: Vec<Filter>,
}

impl Analyzer {
    /// The tokens `text` produces through this chain.
    pub fn analyze(&self, text: &str) -> Vec<Token> {
        let mut tokens = self.tokenizer.split(text);
        for filter in &self.filters {
            tokens = filter.apply(tokens);
        }
        tokens
    }

    /// The TERMS `text` produces through this chain — the token texts, which is what a
    /// `match` query compares.
    pub fn terms(&self, text: &str) -> Vec<String> {
        self.analyze(text)
            .into_iter()
            .map(|token| token.token)
            .collect()
    }
}

/// What an analysis request named that no chain can be built from.
///
/// The engine refuses such a request while BUILDING the chain, before any text is analyzed,
/// and states what it could not find; the caller words it as the engine's own
/// illegal-argument failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unresolved {
    /// No analyzer of that name: neither one the index defines nor one the engine ships.
    Analyzer(String),
    /// No tokenizer of that name.
    Tokenizer(String),
    /// No token filter of that name, or one whose own type nothing registers.
    Filter(String),
    /// The mappings map no such field, so there is no field analyzer to use.
    Field(String),
    /// More token filters than one chain may name ([`MAX_FILTERS`]) — the emulator's own
    /// bound on the chain one analysis walks, so the request is refused rather than
    /// answered from part of the chain it asked for.
    TooManyFilters(usize),
    /// A tokenizer or filter whose TYPE this engine registers but whose CONFIGURATION it
    /// cannot run, so no chain is built from it rather than one built from another
    /// configuration — which would answer tokens the caller's own chain never produced.
    Configuration {
        kind: String,
        setting: String,
        value: String,
    },
}

impl Unresolved {
    /// The sentence the engine states for this, as it words each of them.
    ///
    /// `global` says the name was resolved against the CLUSTER's own registry rather than
    /// against an index's analysis settings — which is the case for an analyze request the
    /// path scoped to no index — and the engine says so in the sentence. A FIELD is an
    /// index's, so that one is worded the same either way, and so is a chain this emulator
    /// bounds rather than the engine.
    pub fn message(&self, global: bool) -> String {
        let scope = if global { "global " } else { "" };
        match self {
            Self::Analyzer(name) => format!("failed to find {scope}analyzer [{name}]"),
            Self::Tokenizer(name) => format!("failed to find {scope}tokenizer under [{name}]"),
            Self::Filter(name) => format!("failed to find {scope}token filter under [{name}]"),
            Self::Field(name) => format!("failed to find field [{name}]"),
            Self::TooManyFilters(named) => format!(
                "a chain may name at most {MAX_FILTERS} token filters, and this one names \
                 {named}"
            ),
            Self::Configuration {
                kind,
                setting,
                value,
            } => format!("[{kind}] does not support {setting} [{value}]"),
        }
    }
}

/// The member an analysis section defines its token filters under.
const ANALYSIS_FILTERS: &str = "filter";

/// The member an analysis section defines its tokenizers under.
const ANALYSIS_TOKENIZERS: &str = "tokenizer";

/// The member an analyzer or filter definition names its kind with.
const DEFINITION_TYPE: &str = "type";

/// The field types whose text is ANALYZED, so a `match` on one is evaluated over terms;
/// every other mapped type holds its value whole.
const ANALYZED_FIELD_TYPES: [&str; 3] = ["text", "match_only_text", "search_as_you_type"];

/// The member a mapping lists an object's own fields under: the first of the containers
/// this crate names ([`MAPPING_CONTAINERS`]), which is also the one an index's mappings
/// list their top-level fields under.
const MAPPING_PROPERTIES: &str = MAPPING_CONTAINERS[0];

/// The member a mapping lists a field's MULTI-FIELDS under: the second of the containers
/// this crate names ([`MAPPING_CONTAINERS`]). A field under it is another indexing of the
/// field above it rather than a member of the document.
const MAPPING_MULTI_FIELDS: &str = MAPPING_CONTAINERS[1];

/// The analyzers, filters and tokenizers an index's `analysis` settings DEFINE, plus its
/// field mappings — everything a chain for one of its fields is built from.
///
/// The analysis sections are read wherever they appear in the schema, for the reason
/// [`SCHEMA_ANALYSIS`] gives: the same settings reach the cluster both directly under the
/// index's settings and under the `index` section inside them. Where two sections define
/// one name the FIRST definition stands, so one reading of a schema always builds one
/// chain.
#[derive(Debug, Clone, Default)]
pub struct IndexAnalysis {
    analyzers: Map<String, Value>,
    filters: Map<String, Value>,
    tokenizers: Map<String, Value>,
    mappings: Map<String, Value>,
}

impl IndexAnalysis {
    /// What `schema` — an index's stored `IndexSchema` — defines.
    pub fn of(schema: &::aws_smithy_types::Document) -> Self {
        let mut analysis = Self::default();
        for section in sections_named(schema, SCHEMA_ANALYSIS, true) {
            let Value::Object(members) = json_from_document(section) else {
                continue;
            };
            for (member, into) in [
                (ANALYSIS_ANALYZERS, &mut analysis.analyzers),
                (ANALYSIS_FILTERS, &mut analysis.filters),
                (ANALYSIS_TOKENIZERS, &mut analysis.tokenizers),
            ] {
                if let Some(Value::Object(defined)) = members.get(member) {
                    for (name, definition) in defined {
                        into.entry(name.clone())
                            .or_insert_with(|| definition.clone());
                    }
                }
            }
        }
        for section in sections_named(schema, SCHEMA_MAPPINGS, false) {
            if let Value::Object(members) = json_from_document(section) {
                for (name, definition) in members {
                    analysis.mappings.entry(name).or_insert(definition);
                }
            }
        }
        analysis
    }

    /// The chain the analyzer `name` names, from this index's own definitions or from the
    /// engine's catalog.
    pub fn analyzer(&self, name: &str, files: &PackageFiles) -> Result<Analyzer, Unresolved> {
        match self.analyzers.get(name) {
            Some(definition) => self.defined(name, definition, files),
            None => built_in(name).ok_or_else(|| Unresolved::Analyzer(name.to_string())),
        }
    }

    /// The chain an analyzer this index DEFINES states: a tokenizer and the filters it
    /// lists, or — where its `type` names one of the engine's own analyzers — that one.
    fn defined(
        &self,
        name: &str,
        definition: &Value,
        files: &PackageFiles,
    ) -> Result<Analyzer, Unresolved> {
        let kind = definition
            .get(DEFINITION_TYPE)
            .and_then(Value::as_str)
            .unwrap_or(CUSTOM_ANALYZER_TYPE);
        if kind != CUSTOM_ANALYZER_TYPE {
            return built_in(kind).ok_or_else(|| Unresolved::Analyzer(name.to_string()));
        }
        self.composed(
            definition.get(ANALYSIS_TOKENIZERS),
            definition.get(ANALYSIS_FILTERS),
            files,
        )
    }

    /// The chain a request composes itself: the tokenizer it names and the filters it
    /// lists, each resolved against this index's definitions and then the engine's catalog.
    ///
    /// A tokenizer is NAMED (a string) or configured inline (an object carrying its
    /// `type`), the two forms a filter may take as well, and a form or a configuration this
    /// engine cannot build a tokenizer from is refused — never replaced by the default one,
    /// which would answer another chain's tokens for the chain the caller asked for.
    pub fn composed(
        &self,
        tokenizer: Option<&Value>,
        filters: Option<&Value>,
        files: &PackageFiles,
    ) -> Result<Analyzer, Unresolved> {
        let tokenizer = match tokenizer {
            // A tokenizer the request names: one this index defines, or one of the
            // engine's own.
            Some(Value::String(named)) => {
                let definition = self.tokenizers.get(named.as_str());
                match definition {
                    Some(definition) => configured_tokenizer(definition, named)?,
                    None => Tokenizer::named(named)
                        .ok_or_else(|| Unresolved::Tokenizer(named.clone()))?,
                }
            }
            // One the request configures inline.
            Some(definition @ Value::Object(_)) => {
                let kind = definition
                    .get(DEFINITION_TYPE)
                    .and_then(Value::as_str)
                    .ok_or_else(|| Unresolved::Tokenizer(definition.to_string()))?;
                configured_tokenizer(definition, kind)?
            }
            Some(other) => return Err(Unresolved::Tokenizer(other.to_string())),
            // A chain that names no tokenizer is tokenized the way the engine's default
            // analyzer is.
            None => Tokenizer::Standard,
        };
        let listed: Vec<&Value> = match filters {
            Some(Value::Array(elements)) => {
                // Past the bound the chain is refused rather than built from part of what
                // the request listed: an analysis of a shorter chain than the caller asked
                // for is one it cannot tell from the chain it asked for.
                if elements.len() > MAX_FILTERS {
                    return Err(Unresolved::TooManyFilters(elements.len()));
                }
                elements.iter().collect()
            }
            Some(one @ (Value::String(_) | Value::Object(_))) => vec![one],
            _ => Vec::new(),
        };
        let mut chain = Vec::new();
        for named in listed {
            chain.push(self.filter(named, files)?);
        }
        Ok(Analyzer {
            tokenizer,
            filters: chain,
        })
    }

    /// The chain the field `field` is analyzed with: the analyzer its mapping names, or the
    /// index's default.
    ///
    /// `search` says which of the two analyzers a mapping may name is wanted — the one a
    /// QUERY's text goes through (`search_analyzer`, falling back to `analyzer`) or the one
    /// the field's own text went through when it was indexed.
    pub fn field_analyzer(
        &self,
        field: &str,
        search: bool,
        files: &PackageFiles,
    ) -> Result<Analyzer, Unresolved> {
        self.analyzer(&self.field_analyzer_name(field, search)?, files)
    }

    /// The NAME of the chain the field `field` is analyzed with, without building it.
    ///
    /// Held apart from [`Self::field_analyzer`] because many fields commonly name one
    /// analyzer: a caller resolving a whole query's fields reads the names here and builds
    /// each chain once, instead of building one per field.
    pub fn field_analyzer_name(&self, field: &str, search: bool) -> Result<String, Unresolved> {
        let mapped = self
            .field(field)
            .ok_or_else(|| Unresolved::Field(field.to_string()))?;
        // `search_analyzer` is the one a QUERY's text goes through and `analyzer` the one
        // the field's own text went through, which is the order a search resolves them in
        // ([`MAPPING_ANALYZER_MEMBERS`] is where this crate states those member names).
        let [indexing, searching, _] = MAPPING_ANALYZER_MEMBERS;
        Ok(search
            .then(|| mapped.get(searching).and_then(Value::as_str))
            .flatten()
            .or_else(|| mapped.get(indexing).and_then(Value::as_str))
            .unwrap_or(DEFAULT_ANALYZER)
            .to_string())
    }

    /// The chain a field's text is analyzed with where the mappings map no such field: the
    /// index's own default analyzer, which is the engine's `standard` unless the index
    /// defines one of its own under that name.
    pub fn default_analyzer(&self, files: &PackageFiles) -> Analyzer {
        self.analyzer(DEFAULT_ANALYZER, files)
            .or_else(|_| self.analyzer("default", files))
            .unwrap_or_else(|_| standard())
    }

    /// The mapping of the field `field` names — which may be a dotted path through the
    /// object fields or multi-fields above it ([`MAPPING_CONTAINERS`]) — or `None` where
    /// the mappings map no such field.
    pub fn field(&self, field: &str) -> Option<&Value> {
        let mut properties: &Value = self.mappings.get(MAPPING_PROPERTIES)?;
        let mut segments = field.split('.').peekable();
        while let Some(segment) = segments.next() {
            let mapped = properties.get(segment)?;
            if segments.peek().is_none() {
                return Some(mapped);
            }
            properties = MAPPING_CONTAINERS
                .iter()
                .find_map(|container| mapped.get(container))?;
        }
        None
    }

    /// The path in a document's SOURCE the field `field` takes its values from.
    ///
    /// A MULTI-FIELD is "a way to index the same field in different ways": `title.raw`
    /// names a second indexing OF `title` and not a member of it, so its values are the
    /// parent field's own — while a field of an OBJECT field (`author.name`) is a member
    /// and keeps its own path. A path the mappings do not map at all is its own path, since
    /// a dynamically mapped field is a member of the document.
    #[must_use]
    pub fn source_field(&self, field: &str) -> String {
        let Some(mut properties) = self.mappings.get(MAPPING_PROPERTIES) else {
            return field.to_string();
        };
        let mut path: Vec<&str> = Vec::new();
        let mut segments = field.split('.').peekable();
        while let Some(segment) = segments.next() {
            let Some(mapped) = properties.get(segment) else {
                return field.to_string();
            };
            path.push(segment);
            if segments.peek().is_none() {
                break;
            }
            // An object field's own members are members of the document too, so the path
            // continues through them.
            if let Some(members) = mapped.get(MAPPING_PROPERTIES) {
                properties = members;
                continue;
            }
            // A multi-field of this field holds THIS field's value, so the path stops here.
            let multi = mapped.get(MAPPING_MULTI_FIELDS);
            if multi.is_some_and(|fields| segments.peek().is_some_and(|next| fields.get(next).is_some()))
            {
                return path.join(".");
            }
            return field.to_string();
        }
        path.join(".")
    }

    /// Whether the field `field` holds its value WHOLE rather than analyzing it — a
    /// keyword, a number, a boolean, a date — which is what decides whether a `match` on it
    /// is analyzed at all. A field the mappings do not map is analyzed, because a
    /// dynamically mapped text field is.
    pub fn holds_whole_values(&self, field: &str) -> bool {
        self.field(field)
            .and_then(|mapped| mapped.get(MAPPING_TYPE))
            .and_then(Value::as_str)
            .is_some_and(|kind| !ANALYZED_FIELD_TYPES.contains(&kind))
    }

    /// The filter one element of a chain names: one this index defines, one configured
    /// inline, or one of the engine's own.
    fn filter(&self, named: &Value, files: &PackageFiles) -> Result<Filter, Unresolved> {
        let definition = match named {
            Value::String(name) => match self.filters.get(name) {
                Some(defined) => defined,
                None => {
                    return built_in_filter(name)
                        .ok_or_else(|| Unresolved::Filter(name.to_string()))
                }
            },
            object @ Value::Object(_) => object,
            other => return Err(Unresolved::Filter(other.to_string())),
        };
        let kind = definition
            .get(DEFINITION_TYPE)
            .and_then(Value::as_str)
            .ok_or_else(|| Unresolved::Filter(definition.to_string()))?;
        configured_filter(kind, definition, files)
    }
}

/// The member a `pattern` tokenizer names the expression it splits on in.
const PATTERN_SETTING: &str = "pattern";

/// That setting's own default — "non-word character", which is the class
/// [`Tokenizer::NonWord`] splits on.
const DEFAULT_PATTERN: &str = r"\W+";

/// The tokenizer `definition` configures, under the type or name `kind`, or the refusal for
/// one this engine cannot build.
///
/// A definition may CONFIGURE the tokenizer it names, and a configuration this engine
/// cannot run is refused rather than answered with the tokenizer's default behaviour: the
/// `pattern` family splits on an expression, and the only expression this engine splits on
/// is that setting's own default, so a definition naming another one resolves to no
/// tokenizer at all.
fn configured_tokenizer(definition: &Value, kind: &str) -> Result<Tokenizer, Unresolved> {
    let kind = definition
        .get(DEFINITION_TYPE)
        .and_then(Value::as_str)
        .unwrap_or(kind);
    let tokenizer =
        Tokenizer::named(kind).ok_or_else(|| Unresolved::Tokenizer(kind.to_string()))?;
    let pattern = definition.get(PATTERN_SETTING).and_then(Value::as_str);
    if matches!(tokenizer, Tokenizer::NonWord)
        && pattern.is_some_and(|pattern| pattern != DEFAULT_PATTERN)
    {
        return Err(Unresolved::Configuration {
            kind: kind.to_string(),
            setting: PATTERN_SETTING.to_string(),
            value: pattern.unwrap_or_default().to_string(),
        });
    }
    Ok(tokenizer)
}

/// The member a `stop` filter names its word list in, and the member a `stemmer` filter
/// names its language in.
const STOPWORDS_SETTING: &str = "stopwords";
const LANGUAGE_SETTING: &str = "language";

/// The value either setting takes for NO list at all: "`_none_` ... no stop words".
const NO_PREDEFINED_LIST: &str = "none";

/// The filter `kind` names, configured as `definition` states it, or the refusal for a type
/// this engine registers no filter for, or a configuration it cannot run.
///
/// A configuration it cannot run is REFUSED rather than replaced by another one: a filter
/// the caller configured and this engine ran as something else — another language's word
/// list, or nothing at all — would answer tokens the chain the caller asked for never
/// produced.
fn configured_filter(
    kind: &str,
    definition: &Value,
    files: &PackageFiles,
) -> Result<Filter, Unresolved> {
    Ok(match kind {
        "lowercase" => Filter::Lowercase,
        "uppercase" => Filter::Uppercase,
        "trim" => Filter::Trim,
        "fingerprint" => Filter::Fingerprint,
        "stop" => Filter::Stop(stop_words(definition, files)?),
        "synonym" | "synonym_graph" => Filter::Synonym(synonyms(definition, files)),
        "stemmer" | "snowball" => {
            let language = definition
                .get(LANGUAGE_SETTING)
                .or_else(|| definition.get("name"))
                .and_then(Value::as_str)
                .unwrap_or(ENGLISH)
                .trim_matches('_')
                .to_lowercase();
            // The stemmer has to BE there: a language this process ships no Snowball
            // algorithm for is a filter this engine cannot run, and running it as nothing
            // would answer unstemmed terms for a chain the caller asked to stem.
            if stemmer(&language).is_none() {
                return Err(Unresolved::Configuration {
                    kind: kind.to_string(),
                    setting: LANGUAGE_SETTING.to_string(),
                    value: language,
                });
            }
            Filter::Stem(language)
        }
        "porter_stem" | "kstem" => Filter::Stem(ENGLISH.to_string()),
        "length" => Filter::Length {
            min: bounded(definition, "min").unwrap_or(0),
            max: bounded(definition, "max").unwrap_or(usize::MAX),
        },
        _ => return Err(Unresolved::Filter(kind.to_string())),
    })
}

/// The whole number `definition` gives `member`, where it gives one at all.
fn bounded(definition: &Value, member: &str) -> Option<usize> {
    usize::try_from(definition.get(member)?.as_u64()?).ok()
}

/// The built-in filter `name` names — one the engine ships ready to use, which a chain may
/// name without defining anything.
fn built_in_filter(name: &str) -> Option<Filter> {
    Some(match name {
        "lowercase" => Filter::Lowercase,
        "uppercase" => Filter::Uppercase,
        "trim" => Filter::Trim,
        "fingerprint" => Filter::Fingerprint,
        "porter_stem" | "kstem" | "snowball" | "stemmer" => Filter::Stem(ENGLISH.to_string()),
        "stop" => Filter::Stop(language_stop_words(ENGLISH).unwrap_or_default()),
        _ => return None,
    })
}

/// The words a `stop` filter drops: the list it carries inline, the predefined list for the
/// language it names, or the file it names by path.
///
/// `_none_` is "no stop words" and resolves to the EMPTY set, which is what the engine
/// documents it as. A predefined list this engine's catalog does not carry is refused: the
/// English list stood in for it would drop words the chain the caller configured never
/// drops.
fn stop_words(definition: &Value, files: &PackageFiles) -> Result<BTreeSet<String>, Unresolved> {
    match definition.get(STOPWORDS_SETTING) {
        Some(Value::Array(listed)) => {
            return Ok(listed
                .iter()
                .filter_map(Value::as_str)
                .take(MAX_DICTIONARY_ENTRIES)
                .map(str::to_owned)
                .collect())
        }
        Some(Value::String(named)) => {
            let language = named.trim_matches('_').to_lowercase();
            if language == NO_PREDEFINED_LIST {
                return Ok(BTreeSet::new());
            }
            return language_stop_words(&language).ok_or(Unresolved::Configuration {
                kind: "stop".to_string(),
                setting: STOPWORDS_SETTING.to_string(),
                value: named.clone(),
            });
        }
        _ => {}
    }
    match definition.get("stopwords_path").and_then(Value::as_str) {
        Some(path) => Ok(word_list(files.at(path).unwrap_or_default())),
        // "A pre-defined stop words list like _english_ or an array containing a list of
        // stop words. Defaults to _english_."
        None => Ok(language_stop_words(ENGLISH).unwrap_or_default()),
    }
}

/// The rules a `synonym` filter applies: the ones it carries inline, or the ones the file
/// it names by path holds.
fn synonyms(definition: &Value, files: &PackageFiles) -> Synonyms {
    if let Some(Value::Array(listed)) = definition.get("synonyms") {
        let lines: Vec<String> = listed
            .iter()
            .filter_map(Value::as_str)
            .take(MAX_DICTIONARY_ENTRIES)
            .map(str::to_owned)
            .collect();
        return Synonyms::of(synonym_rules(&lines));
    }
    match definition.get("synonyms_path").and_then(Value::as_str) {
        Some(path) => Synonyms::of(synonym_rules(files.at(path).unwrap_or_default())),
        None => Synonyms::default(),
    }
}

/// The words a dictionary file lists: one word per line, which is how the engine's own
/// word-list loader reads a stop-word file.
fn word_list(lines: &[String]) -> BTreeSet<String> {
    lines
        .iter()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// The rules a synonym file lists, in the engine's own (Solr) format: `a, b, c` makes every
/// term of the group stand for the others, and `a, b => c` replaces either input with the
/// outputs.
///
/// Each side of one line contributes at most [`MAX_TERMS_PER_RULE`] terms, so a single long
/// line cannot size the work a later analysis does.
fn synonym_rules(lines: &[String]) -> Vec<SynonymRule> {
    let terms = |side: &str| -> Vec<String> {
        side.split(',')
            .map(str::trim)
            .filter(|term| !term.is_empty())
            .take(MAX_TERMS_PER_RULE)
            .map(str::to_owned)
            .collect()
    };
    lines
        .iter()
        .filter_map(|line| match line.split_once("=>") {
            Some((inputs, outputs)) => {
                let (inputs, outputs) = (terms(inputs), terms(outputs));
                (!inputs.is_empty() && !outputs.is_empty()).then_some(SynonymRule {
                    inputs,
                    outputs,
                    replaces: true,
                })
            }
            None => {
                let group = terms(line);
                (group.len() > 1).then(|| SynonymRule {
                    inputs: group.clone(),
                    outputs: group,
                    replaces: false,
                })
            }
        })
        .collect()
}

/// The engine's `standard` analyzer: UAX#29 word boundaries, folded to lower case.
fn standard() -> Analyzer {
    Analyzer {
        tokenizer: Tokenizer::Standard,
        filters: vec![Filter::Lowercase],
    }
}

/// The built-in analyzer `name` names, or `None` where the engine ships none under it.
///
/// The generic ones are the chains the engine documents for them; a LANGUAGE analyzer is
/// the standard tokenizer folded to lower case, with that language's function words dropped
/// and its terms stemmed — each taken from the published catalogs
/// ([`language_stop_words`], [`stemmer`]) and left out for a language neither carries.
pub fn built_in(name: &str) -> Option<Analyzer> {
    let chain = |tokenizer: Tokenizer, filters: Vec<Filter>| Analyzer {
        tokenizer,
        filters,
    };
    Some(match name {
        "standard" | "default" | "classic" => standard(),
        "simple" => chain(Tokenizer::Lowercase, Vec::new()),
        "whitespace" => chain(Tokenizer::Whitespace, Vec::new()),
        "keyword" => chain(Tokenizer::Keyword, Vec::new()),
        "pattern" => chain(Tokenizer::NonWord, vec![Filter::Lowercase]),
        "fingerprint" => chain(
            Tokenizer::Standard,
            vec![
                Filter::Lowercase,
                Filter::Stop(language_stop_words(ENGLISH).unwrap_or_default()),
                Filter::Fingerprint,
            ],
        ),
        "stop" => chain(
            Tokenizer::Lowercase,
            vec![Filter::Stop(
                language_stop_words(ENGLISH).unwrap_or_default(),
            )],
        ),
        "snowball" => chain(
            Tokenizer::Standard,
            vec![
                Filter::Lowercase,
                Filter::Stop(language_stop_words(ENGLISH).unwrap_or_default()),
                Filter::Stem(ENGLISH.to_string()),
            ],
        ),
        language if BUILT_IN_ANALYZERS.contains(&language) => {
            let mut filters = vec![Filter::Lowercase];
            if let Some(words) = language_stop_words(language) {
                filters.push(Filter::Stop(words));
            }
            if stemmer(language).is_some() {
                filters.push(Filter::Stem(language.to_string()));
            }
            chain(Tokenizer::Standard, filters)
        }
        _ => return None,
    })
}

/// The language whose list and stemmer the engine's own defaults name.
const ENGLISH: &str = "english";

/// The published function words of the language `language` names, lowercased, or `None`
/// where the catalog carries no list for it.
///
/// The names are the engine's own language-analyzer names and the lists are the
/// `stop-words` catalog's, so neither is written out here. `brazilian` is Brazilian
/// Portuguese, which is why it reads the Portuguese list.
fn language_stop_words(language: &str) -> Option<BTreeSet<String>> {
    use ::stop_words::LANGUAGE;

    let published = ::stop_words::get(match language {
        "arabic" => LANGUAGE::Arabic,
        "armenian" => LANGUAGE::Armenian,
        "basque" => LANGUAGE::Basque,
        "bengali" => LANGUAGE::Bengali,
        "brazilian" | "portuguese" => LANGUAGE::Portuguese,
        "bulgarian" => LANGUAGE::Bulgarian,
        "catalan" => LANGUAGE::Catalan,
        "czech" => LANGUAGE::Czech,
        "danish" => LANGUAGE::Danish,
        "dutch" => LANGUAGE::Dutch,
        ENGLISH => LANGUAGE::English,
        "estonian" => LANGUAGE::Estonian,
        "finnish" => LANGUAGE::Finnish,
        "french" => LANGUAGE::French,
        "galician" => LANGUAGE::Galician,
        "german" => LANGUAGE::German,
        "greek" => LANGUAGE::Greek,
        "hindi" => LANGUAGE::Hindi,
        "hungarian" => LANGUAGE::Hungarian,
        "indonesian" => LANGUAGE::Indonesian,
        "irish" => LANGUAGE::Irish,
        "italian" => LANGUAGE::Italian,
        "latvian" => LANGUAGE::Latvian,
        "lithuanian" => LANGUAGE::Lithuanian,
        "norwegian" => LANGUAGE::Norwegian,
        "persian" => LANGUAGE::Persian,
        "romanian" => LANGUAGE::Romanian,
        "russian" => LANGUAGE::Russian,
        "spanish" => LANGUAGE::Spanish,
        "swedish" => LANGUAGE::Swedish,
        "thai" => LANGUAGE::Thai,
        "turkish" => LANGUAGE::Turkish,
        _ => return None,
    });
    Some(published.into_iter().map(|word| word.to_lowercase()).collect())
}

/// The Snowball stemmer for the language `language` names, or `None` where this process
/// ships none for it.
///
/// The algorithms are the `rust-stemmers` catalog's, under the engine's own
/// language-analyzer names; a language that catalog carries no algorithm for has no
/// stemmer here, so its terms are left as the tokenizer and the filters before this one
/// left them rather than stemmed as another language.
fn stemmer(language: &str) -> Option<::rust_stemmers::Stemmer> {
    use ::rust_stemmers::Algorithm;

    Some(::rust_stemmers::Stemmer::create(match language {
        "arabic" => Algorithm::Arabic,
        "danish" => Algorithm::Danish,
        "dutch" => Algorithm::Dutch,
        ENGLISH => Algorithm::English,
        "finnish" => Algorithm::Finnish,
        "french" => Algorithm::French,
        "german" => Algorithm::German,
        "greek" => Algorithm::Greek,
        "hungarian" => Algorithm::Hungarian,
        "italian" => Algorithm::Italian,
        "norwegian" => Algorithm::Norwegian,
        "brazilian" | "portuguese" => Algorithm::Portuguese,
        "romanian" => Algorithm::Romanian,
        "russian" => Algorithm::Russian,
        "spanish" => Algorithm::Spanish,
        "swedish" => Algorithm::Swedish,
        "turkish" => Algorithm::Turkish,
        _ => return None,
    }))
}
