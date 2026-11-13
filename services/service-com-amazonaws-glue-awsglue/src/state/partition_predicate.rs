// SPDX-License-Identifier: CC0-1.0

//! The `Expression` a partition listing selects a table's partitions with.
//!
//! "An expression that filters the partitions to be returned. The expression uses SQL
//! syntax similar to the SQL `WHERE` filter clause"
//! (`GetPartitionsRequest$Expression`), over the PARTITION KEYS of the addressed table:
//! the operators the member documents are `=`, `<>`, `>`, `<`, `>=`, `<=`, `AND`, `OR`,
//! `IN`, `BETWEEN`, `LIKE`, `NOT` and `IS NULL`, and the partition key types it filters
//! are `string`, `date`, `timestamp`, `int`, `bigint`, `long`, `tinyint`, `smallint` and
//! `decimal` — "if a type is encountered that is not valid, an exception is thrown".
//!
//! The expression is PARSED as SQL by the SQL parser this workspace already uses rather
//! than scanned for operators here, and the tree it yields is read ONCE per request into
//! a condition this module then evaluates against each stored partition's values. So the
//! filter a request carries decides which partitions are reported, and the reading of it
//! costs one parse however many partitions the table holds.
//!
//! A comparison is made in the TYPE the addressed partition key declares, not on text: a
//! numeric key compares numerically (so `9` is below `10`), a `date` or `timestamp` key
//! compares by its calendar components (so an hour is below the next day), and a `string`
//! key compares lexicographically. A literal the key's own type cannot read is refused
//! rather than quietly matching nothing.
//!
//! It lives beside the stored catalog rather than in one operation's file because a
//! partition listing is not the only reader of this filter: the Lake Formation
//! application-integration listing takes the same `Expression` over the same partitions.

use std::cmp::Ordering;

use ::sqlparser::ast::{BinaryOperator, Expr, UnaryOperator, Value as SqlValue, ValueWithSpan};
use ::sqlparser::dialect::GenericDialect;
use ::sqlparser::parser::Parser;
use ::sqlparser::tokenizer::Token;

use super::messages;
use crate::ops::types::Column;

/// How deep a nesting of the filter this module reads and evaluates.
///
/// Counted on the TREE rather than on brackets — a chain of `AND`s nests the tree without
/// nesting a bracket — so it bounds the parser's own recursion, this module's reading of
/// the parsed tree, the drop of the condition that reading builds, and the walk of that
/// condition per partition. The expression is request text (`PredicateString`, at most
/// 2048 characters), so the work one of them can buy is bounded here rather than left to
/// grow with the text.
const MAX_FILTER_DEPTH: usize = 32;

/// The partition key types an `Expression` filters, as the member documents them.
///
/// Source: `GetPartitionsRequest$Expression`, "Supported Partition Key Types": `string`,
/// `date`, `timestamp`, `int`, `bigint`, `long`, `tinyint`, `smallint`, `decimal`. A
/// declared type outside this list is one the filter cannot compare, which is what
/// [`PartitionFilterRefusal::KeyTypeNotSupported`] reports.
pub const FILTERABLE_PARTITION_KEY_TYPES: &[&str] = &[
    "string",
    "date",
    "timestamp",
    "int",
    "bigint",
    "long",
    "tinyint",
    "smallint",
    "decimal",
];

/// Why an `Expression` is no filter on the addressed table's partitions.
///
/// Every one of these is a fault of the REQUEST — a filter the service cannot run against
/// this table — so each is answered with the listing's own `InvalidInputException`, and
/// none of them depends on which partitions the table holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionFilterRefusal {
    /// The value is not a `WHERE`-clause expression over the documented operators at all:
    /// it does not parse as one, it carries text beyond the expression, it nests deeper
    /// than [`MAX_FILTER_DEPTH`], it uses an operator the member does not document, or it
    /// compares something other than a partition key against a literal.
    NotAFilter,
    /// The expression names a column that is no partition key of the addressed table. A
    /// filter selects partitions, so the only columns it can read are the ones a partition
    /// is addressed by.
    UnknownPartitionKey {
        /// The column name the expression named, as it named it.
        key: String,
    },
    /// The expression filters a partition key whose declared type is outside
    /// [`FILTERABLE_PARTITION_KEY_TYPES`] — "if a type is encountered that is not valid,
    /// an exception is thrown".
    KeyTypeNotSupported {
        /// The partition key the expression filters.
        key: String,
        /// The type that key is declared with.
        declared: String,
    },
    /// The expression compares a partition key against a literal that is no value of the
    /// key's own type: a word against a numeric key, a value that is no instant against a
    /// `date` or `timestamp` key, `NULL` or a boolean against any of them.
    ValueNotOfKeyType {
        /// The partition key the comparison addresses.
        key: String,
        /// The type that key is declared with, which the literal has to be a value of.
        declared: String,
    },
}

impl PartitionFilterRefusal {
    /// This refusal's wording, the one a listing answers `InvalidInputException` with.
    ///
    /// `expression` is the filter as the request sent it, carried only by the refusal that
    /// is about the expression as a whole.
    pub fn message(&self, expression: &str) -> String {
        match self {
            Self::NotAFilter => messages::expression_is_not_a_partition_filter(expression),
            Self::UnknownPartitionKey { key } => messages::expression_names_no_partition_key(key),
            Self::KeyTypeNotSupported { key, declared } => {
                messages::partition_key_type_not_filterable(key, declared)
            }
            Self::ValueNotOfKeyType { key, declared } => {
                messages::expression_value_not_of_partition_key_type(key, declared)
            }
        }
    }
}

/// One `Expression` PARSED but not yet bound to the partition keys it filters.
///
/// The parse is the part that does not need the table — it reads the request's own text —
/// so a listing parses the filter before it locks the catalog store and binds it to the
/// addressed table's keys once it holds that table, exactly as a table listing compiles its
/// own pattern before locking ([`super::compile_name_pattern`]).
#[derive(Debug)]
pub struct ParsedPartitionFilter {
    /// The expression tree the request's text spells.
    expression: Expr,
}

impl ParsedPartitionFilter {
    /// Read `expression` as a `WHERE`-clause expression, or say it is none.
    ///
    /// Nothing about the addressed table is consulted here: which columns the filter may
    /// read and in which type they compare is [`Self::bind`]'s question.
    pub fn parse(expression: &str) -> Result<Self, PartitionFilterRefusal> {
        let mut parser = Parser::new(&GenericDialect {})
            .with_recursion_limit(MAX_FILTER_DEPTH)
            .try_with_sql(expression)
            .map_err(|_| PartitionFilterRefusal::NotAFilter)?;
        let parsed = parser
            .parse_expr()
            .map_err(|_| PartitionFilterRefusal::NotAFilter)?;
        // Everything the request sent has to BE the filter: text left over past the
        // expression is not a filter whose remainder can be dropped.
        if parser.peek_token().token != Token::EOF {
            return Err(PartitionFilterRefusal::NotAFilter);
        }
        Ok(Self { expression: parsed })
    }

    /// Bind this expression to `partition_keys`, the addressed table's own key list in its
    /// own order, or say why it is no filter over them.
    ///
    /// A column that is no key of THAT table, a key whose declared type a filter cannot
    /// compare, and a literal that is no value of the key's type are refused here and not
    /// when a partition is examined — so the filter is read ONCE however many partitions
    /// the table holds, over a tree whose size the member's own 2048-character ceiling and
    /// [`MAX_FILTER_DEPTH`] bound.
    pub fn bind(
        &self,
        partition_keys: &[Column],
    ) -> Result<PartitionFilter, PartitionFilterRefusal> {
        Ok(PartitionFilter {
            condition: read_condition(&self.expression, partition_keys, 0)?,
        })
    }
}

/// A filter read from one `Expression`, ready to be asked about any partition of the table
/// it was bound to.
///
/// Holds no borrow of the table: the condition addresses a partition key by its INDEX in
/// the table's key list, which is also the index of that key's value in a partition's
/// value list ("ordered in the same order as the partition keys", `PartitionInput$Values`).
#[derive(Debug)]
pub struct PartitionFilter {
    condition: Condition,
}

impl PartitionFilter {
    /// Whether the partition addressed by `values` is one this filter selects.
    pub fn matches(&self, values: &[String]) -> bool {
        self.condition.holds(values)
    }
}

/// One filter, as this module evaluates it.
#[derive(Debug)]
enum Condition {
    And(Box<Condition>, Box<Condition>),
    Or(Box<Condition>, Box<Condition>),
    Not(Box<Condition>),
    /// A partition key compared against a literal of its own type.
    Compare {
        key: KeyRef,
        comparison: Comparison,
        value: Scalar,
    },
    /// `IN (..)`, and `NOT IN (..)` when `negated`.
    In {
        key: KeyRef,
        values: Vec<Scalar>,
        negated: bool,
    },
    /// `BETWEEN low AND high`, inclusive at both ends as SQL's own is.
    Between {
        key: KeyRef,
        low: Scalar,
        high: Scalar,
        negated: bool,
    },
    /// `LIKE`, as the matcher its pattern compiles to ([`like_matcher`]).
    Like {
        key: KeyRef,
        matcher: ::regex::Regex,
        negated: bool,
    },
    /// `IS NULL`, and `IS NOT NULL` when `negated`.
    IsNull { key: KeyRef, negated: bool },
}

impl Condition {
    /// Whether this condition holds for the partition addressed by `values`.
    ///
    /// A partition that carries NO value for a key the condition reads — which a table
    /// whose partition keys were changed after that partition was created can hold — is
    /// null for that key: it satisfies `IS NULL` and satisfies no comparison, exactly as a
    /// null does in a `WHERE` clause.
    fn holds(&self, values: &[String]) -> bool {
        match self {
            Self::And(left, right) => left.holds(values) && right.holds(values),
            Self::Or(left, right) => left.holds(values) || right.holds(values),
            Self::Not(inner) => !inner.holds(values),
            Self::Compare {
                key,
                comparison,
                value,
            } => key
                .read(values)
                .and_then(|stored| compare(&stored, value))
                .is_some_and(|ordering| comparison.holds(ordering)),
            Self::In {
                key,
                values: wanted,
                negated,
            } => match key.read(values) {
                Some(stored) => {
                    let found = wanted
                        .iter()
                        .any(|value| compare(&stored, value) == Some(Ordering::Equal));
                    found != *negated
                }
                None => false,
            },
            Self::Between {
                key,
                low,
                high,
                negated,
            } => match key.read(values) {
                Some(stored) => {
                    let inside = compare(&stored, low).is_some_and(Ordering::is_ge)
                        && compare(&stored, high).is_some_and(Ordering::is_le);
                    inside != *negated
                }
                None => false,
            },
            Self::Like {
                key,
                matcher,
                negated,
            } => match key.text(values) {
                Some(text) => matcher.is_match(text) != *negated,
                None => false,
            },
            Self::IsNull { key, negated } => key.text(values).is_none() != *negated,
        }
    }
}

/// Which orderings a comparison operator admits.
#[derive(Debug, Clone, Copy)]
enum Comparison {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

impl Comparison {
    /// The operator this module reads a SQL binary operator as, or `None` when the
    /// operator is not one the member documents.
    fn of(operator: &BinaryOperator) -> Option<Self> {
        match operator {
            BinaryOperator::Eq => Some(Self::Equal),
            BinaryOperator::NotEq => Some(Self::NotEqual),
            BinaryOperator::Lt => Some(Self::Less),
            BinaryOperator::LtEq => Some(Self::LessOrEqual),
            BinaryOperator::Gt => Some(Self::Greater),
            BinaryOperator::GtEq => Some(Self::GreaterOrEqual),
            _ => None,
        }
    }

    /// The operator with its operands swapped, so `'2024' = year` reads as
    /// `year = '2024'`.
    fn mirrored(self) -> Self {
        match self {
            Self::Equal => Self::Equal,
            Self::NotEqual => Self::NotEqual,
            Self::Less => Self::Greater,
            Self::LessOrEqual => Self::GreaterOrEqual,
            Self::Greater => Self::Less,
            Self::GreaterOrEqual => Self::LessOrEqual,
        }
    }

    /// Whether this operator holds for the ordering of the stored value against the
    /// literal.
    fn holds(self, ordering: Ordering) -> bool {
        match self {
            Self::Equal => ordering.is_eq(),
            Self::NotEqual => ordering.is_ne(),
            Self::Less => ordering.is_lt(),
            Self::LessOrEqual => ordering.is_le(),
            Self::Greater => ordering.is_gt(),
            Self::GreaterOrEqual => ordering.is_ge(),
        }
    }
}

/// How a partition key's values are compared: the class its declared type falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyKind {
    /// `string`: compared as text.
    Text,
    /// `int`, `bigint`, `long`, `tinyint`, `smallint`, `decimal`: compared as numbers.
    Numeric,
    /// `date`, `timestamp`: compared by calendar component.
    Temporal,
}

impl KeyKind {
    /// The class `declared` is compared in, or `None` when it is outside
    /// [`FILTERABLE_PARTITION_KEY_TYPES`].
    ///
    /// The parameters of a parameterised type are not part of its name
    /// (`decimal(10,2)` is a `decimal`), so the name is read up to the opening bracket,
    /// and a declared type is matched without regard to case, the way a Hive column type
    /// is written either way.
    fn of(declared: &str) -> Option<Self> {
        let name = declared
            .split('(')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if !FILTERABLE_PARTITION_KEY_TYPES.contains(&name.as_str()) {
            return None;
        }
        match name.as_str() {
            "string" => Some(Self::Text),
            "date" | "timestamp" => Some(Self::Temporal),
            _ => Some(Self::Numeric),
        }
    }

    /// `text` read as a value of this class, or `None` when it is no value of it.
    fn read(self, text: &str) -> Option<Scalar> {
        match self {
            Self::Text => Some(Scalar::Text(text.to_string())),
            Self::Numeric => read_number(text),
            Self::Temporal => read_instant(text).map(Scalar::Temporal),
        }
    }
}

/// One partition key of the addressed table, as the filter addresses it.
#[derive(Debug)]
struct KeyRef {
    /// The key's position in the table's partition key list, which is the position of its
    /// value in a partition's value list.
    index: usize,
    /// How that key's values are compared.
    kind: KeyKind,
}

impl KeyRef {
    /// The stored text of this key's value, or `None` when the partition carries none.
    fn text<'values>(&self, values: &'values [String]) -> Option<&'values str> {
        values.get(self.index).map(String::as_str)
    }

    /// This key's stored value read in the class it is compared in, or `None` when the
    /// partition carries no value for it or the value stored is no value of that class.
    fn read(&self, values: &[String]) -> Option<Scalar> {
        self.kind.read(self.text(values)?)
    }
}

/// One comparable value: a stored partition value or a literal from the expression, read
/// in the class its partition key is compared in.
#[derive(Debug, Clone, PartialEq)]
enum Scalar {
    Text(String),
    /// A number that is whole, kept exactly so a `bigint` beyond the range a float holds
    /// exactly still compares exactly.
    Integer(i128),
    /// A number that is not whole.
    Real(f64),
    /// An instant, as the calendar components it names in descending significance: year,
    /// month, day, hour, minute, second, and the fraction of a second in nanoseconds.
    Temporal([i64; 7]),
}

/// The ordering of two values of the same class, or `None` when they are of two classes
/// and so are not comparable at all.
fn compare(left: &Scalar, right: &Scalar) -> Option<Ordering> {
    match (left, right) {
        (Scalar::Text(left), Scalar::Text(right)) => Some(left.as_str().cmp(right.as_str())),
        (Scalar::Integer(left), Scalar::Integer(right)) => Some(left.cmp(right)),
        (Scalar::Integer(left), Scalar::Real(right)) => (*left as f64).partial_cmp(right),
        (Scalar::Real(left), Scalar::Integer(right)) => left.partial_cmp(&(*right as f64)),
        (Scalar::Real(left), Scalar::Real(right)) => left.partial_cmp(right),
        (Scalar::Temporal(left), Scalar::Temporal(right)) => Some(left.cmp(right)),
        _ => None,
    }
}

/// `text` read as a number: whole where it spells a whole number, so the comparison of a
/// `bigint` is exact, and otherwise as the real number it spells.
fn read_number(text: &str) -> Option<Scalar> {
    let trimmed = text.trim();
    if let Ok(whole) = trimmed.parse::<i128>() {
        return Some(Scalar::Integer(whole));
    }
    trimmed
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
        .map(Scalar::Real)
}

/// How many digits the fraction of a second is held to, so two fractions written to
/// different lengths compare by value: `.5` is half a second and so is `.500`.
const FRACTION_DIGITS: usize = 9;

/// How many calendar components an instant is read into: year, month, day, hour, minute,
/// second, fraction of a second.
const INSTANT_COMPONENTS: usize = 7;

/// `text` read as the calendar components of an instant, or `None` when it names none.
///
/// A Hive `date` is `2024-01-31` and a Hive `timestamp` is `2024-01-31 12:30:00[.123]`, so
/// the components are the runs of digits in the text, in the order written, with the parts
/// a shorter form omits left at zero — which is what makes a `date` and a `timestamp`
/// comparable against each other and a component written without a leading zero compare by
/// its value. Any character between those runs has to be one an instant is written with; a
/// value carrying anything else (a word, a name) is no instant.
///
/// Whether the components name a real day and time of day is the CALENDAR's question, not
/// this module's: they are handed to the calendar library this workspace already dates by
/// ([`read_calendar_day`]), so a thirteenth month, a thirty-first of February and a
/// twenty-fifth hour are no instants and are refused as values of the key's type rather
/// than compared.
fn read_instant(text: &str) -> Option<[i64; INSTANT_COMPONENTS]> {
    let trimmed = text.trim();
    let mut components = [0_i64; INSTANT_COMPONENTS];
    let mut filled = 0_usize;
    let mut digits = String::new();
    // The separators an instant is written with: the date's dashes, the time's colons, the
    // space or `T` between them, and the dot ahead of the fraction.
    let separators = ['-', ':', '.', ' ', 'T', 't', '/'];
    for character in trimmed.chars().chain(std::iter::once('\0')) {
        if character.is_ascii_digit() {
            digits.push(character);
            continue;
        }
        if !digits.is_empty() {
            if filled == INSTANT_COMPONENTS {
                return None;
            }
            // The last component is a FRACTION of a second, so it is held to a fixed
            // number of digits rather than read as the integer it is written as.
            if filled == INSTANT_COMPONENTS - 1 {
                let padded: String = digits
                    .chars()
                    .chain(std::iter::repeat('0'))
                    .take(FRACTION_DIGITS)
                    .collect();
                components[filled] = padded.parse::<i64>().ok()?;
            } else {
                components[filled] = digits.parse::<i64>().ok()?;
            }
            filled += 1;
            digits.clear();
        }
        if character != '\0' && !separators.contains(&character) {
            return None;
        }
    }
    // A date names at least its year, month and day; anything shorter is no instant.
    if filled < 3 {
        return None;
    }
    read_calendar_day(&components)?;
    Some(components)
}

/// The day and time of day `components` name, as the calendar library reads them — or
/// `None` when they name none.
///
/// The components are a year, a month, a day, an hour, a minute, a second and a fraction of
/// a second in nanoseconds, each a number the text spelled, so this is where a value that
/// looks like an instant but is none (`2024-13-45`) stops being one.
fn read_calendar_day(components: &[i64; INSTANT_COMPONENTS]) -> Option<::chrono::NaiveDateTime> {
    let [year, month, day, hour, minute, second, fraction] = *components;
    let day = ::chrono::NaiveDate::from_ymd_opt(
        i32::try_from(year).ok()?,
        u32::try_from(month).ok()?,
        u32::try_from(day).ok()?,
    )?;
    let time = ::chrono::NaiveTime::from_hms_nano_opt(
        u32::try_from(hour).ok()?,
        u32::try_from(minute).ok()?,
        u32::try_from(second).ok()?,
        u32::try_from(fraction).ok()?,
    )?;
    Some(day.and_time(time))
}

/// The matcher a `LIKE` pattern selects values with, or `None` when the pattern compiles
/// to a matcher larger than one request may build.
///
/// SQL's own wildcards: `%` stands for any run of characters including none, `_` for
/// exactly one, every other character stands for itself, and the pattern selects the value
/// as a WHOLE. `escape` is the `ESCAPE` clause's character when the pattern carries one,
/// which makes the character after it stand for itself.
fn like_matcher(pattern: &str, escape: Option<char>) -> Option<::regex::Regex> {
    let mut translated = String::with_capacity(pattern.len());
    let mut escaped = false;
    for character in pattern.chars() {
        if escaped {
            translated.push_str(&::regex::escape(&character.to_string()));
            escaped = false;
            continue;
        }
        if Some(character) == escape {
            escaped = true;
            continue;
        }
        match character {
            '%' => translated.push_str("(?s:.)*"),
            '_' => translated.push_str("(?s:.)"),
            other => translated.push_str(&::regex::escape(&other.to_string())),
        }
    }
    ::regex::RegexBuilder::new(&format!("^(?:{translated})$"))
        .size_limit(super::MAX_PATTERN_PROGRAM_BYTES)
        .build()
        .ok()
}

/// Read one parsed expression as a condition over `partition_keys`.
///
/// `depth` is how deep this reading already is, bounded by [`MAX_FILTER_DEPTH`] on the
/// tree so a filter that nests without bracketing is bounded too.
fn read_condition(
    expression: &Expr,
    partition_keys: &[Column],
    depth: usize,
) -> Result<Condition, PartitionFilterRefusal> {
    if depth > MAX_FILTER_DEPTH {
        return Err(PartitionFilterRefusal::NotAFilter);
    }
    let depth = depth + 1;
    match expression {
        Expr::Nested(inner) => read_condition(inner, partition_keys, depth),
        Expr::BinaryOp { left, op, right } => match op {
            BinaryOperator::And => Ok(Condition::And(
                Box::new(read_condition(left, partition_keys, depth)?),
                Box::new(read_condition(right, partition_keys, depth)?),
            )),
            BinaryOperator::Or => Ok(Condition::Or(
                Box::new(read_condition(left, partition_keys, depth)?),
                Box::new(read_condition(right, partition_keys, depth)?),
            )),
            operator => {
                let comparison =
                    Comparison::of(operator).ok_or(PartitionFilterRefusal::NotAFilter)?;
                // A comparison names a partition key and a literal, in either order.
                if let Some(name) = column_of(left) {
                    let (key, column) = resolve_key(&name, partition_keys)?;
                    let value = read_scalar(right, &column, key.kind)?;
                    return Ok(Condition::Compare {
                        key,
                        comparison,
                        value,
                    });
                }
                let name = column_of(right).ok_or(PartitionFilterRefusal::NotAFilter)?;
                let (key, column) = resolve_key(&name, partition_keys)?;
                let value = read_scalar(left, &column, key.kind)?;
                Ok(Condition::Compare {
                    key,
                    comparison: comparison.mirrored(),
                    value,
                })
            }
        },
        Expr::UnaryOp {
            op: UnaryOperator::Not,
            expr,
        } => Ok(Condition::Not(Box::new(read_condition(
            expr,
            partition_keys,
            depth,
        )?))),
        Expr::IsNull(inner) => {
            let (key, _) = resolve_key(
                &column_of(inner).ok_or(PartitionFilterRefusal::NotAFilter)?,
                partition_keys,
            )?;
            Ok(Condition::IsNull {
                key,
                negated: false,
            })
        }
        Expr::IsNotNull(inner) => {
            let (key, _) = resolve_key(
                &column_of(inner).ok_or(PartitionFilterRefusal::NotAFilter)?,
                partition_keys,
            )?;
            Ok(Condition::IsNull { key, negated: true })
        }
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            let (key, column) = resolve_key(
                &column_of(expr).ok_or(PartitionFilterRefusal::NotAFilter)?,
                partition_keys,
            )?;
            let mut values = Vec::with_capacity(list.len());
            for item in list {
                values.push(read_scalar(item, &column, key.kind)?);
            }
            Ok(Condition::In {
                key,
                values,
                negated: *negated,
            })
        }
        Expr::Between {
            expr,
            negated,
            low,
            high,
        } => {
            let (key, column) = resolve_key(
                &column_of(expr).ok_or(PartitionFilterRefusal::NotAFilter)?,
                partition_keys,
            )?;
            let kind = key.kind;
            Ok(Condition::Between {
                key,
                low: read_scalar(low, &column, kind)?,
                high: read_scalar(high, &column, kind)?,
                negated: *negated,
            })
        }
        Expr::Like {
            negated,
            expr,
            pattern,
            escape_char,
            ..
        } => {
            let (key, _) = resolve_key(
                &column_of(expr).ok_or(PartitionFilterRefusal::NotAFilter)?,
                partition_keys,
            )?;
            // A `LIKE` selects by the TEXT of the value, so its operand is a text pattern
            // whatever type the key it filters is declared with.
            let pattern = literal_text(pattern).ok_or(PartitionFilterRefusal::NotAFilter)?;
            let escape = escape_char.as_ref().and_then(|clause| {
                let text = clause.to_string();
                text.chars().next()
            });
            let matcher =
                like_matcher(&pattern, escape).ok_or(PartitionFilterRefusal::NotAFilter)?;
            Ok(Condition::Like {
                key,
                matcher,
                negated: *negated,
            })
        }
        // Anything else is no filter over a table's partitions.
        _ => Err(PartitionFilterRefusal::NotAFilter),
    }
}

/// The partition key `name` names, as the filter addresses it, together with the column
/// that declares it.
///
/// A column name is matched without regard to case: a Hive column name is case
/// insensitive, and the catalog stores a partition key under the name the table was
/// created with.
fn resolve_key(
    name: &str,
    partition_keys: &[Column],
) -> Result<(KeyRef, Column), PartitionFilterRefusal> {
    let (index, column) = partition_keys
        .iter()
        .enumerate()
        .find(|(_, column)| column.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| PartitionFilterRefusal::UnknownPartitionKey {
            key: name.to_string(),
        })?;
    let declared = column.r#type.clone().unwrap_or_default();
    let kind = KeyKind::of(&declared).ok_or_else(|| {
        PartitionFilterRefusal::KeyTypeNotSupported {
            key: column.name.clone(),
            declared: declared.clone(),
        }
    })?;
    Ok((KeyRef { index, kind }, column.clone()))
}

/// The column name `expression` names, if it names one.
///
/// A qualified name (`t.year`) names the column its LAST part spells, the parts ahead of
/// it being the relation that column belongs to — and a filter over one table's partitions
/// reads the columns of that one table.
fn column_of(expression: &Expr) -> Option<String> {
    match expression {
        Expr::Identifier(identifier) => Some(identifier.value.clone()),
        Expr::CompoundIdentifier(identifiers) => identifiers
            .iter()
            .next_back()
            .map(|identifier| identifier.value.clone()),
        Expr::Nested(inner) => column_of(inner),
        _ => None,
    }
}

/// Read one literal of the expression as a value of the partition key it is compared
/// against.
///
/// A literal written as text is read in the key's own type, so `year = '2024'` filters a
/// numeric key by the number it spells, and a literal written as a number is read as the
/// text it spells when the key is a `string`, so `year = 2024` filters a string key by the
/// characters it is stored as. A literal of neither — `NULL`, a boolean, a value that is no
/// number for a numeric key or no instant for a `date` — is no value of the key's type.
fn read_scalar(
    expression: &Expr,
    column: &Column,
    kind: KeyKind,
) -> Result<Scalar, PartitionFilterRefusal> {
    let not_of_key_type = || PartitionFilterRefusal::ValueNotOfKeyType {
        key: column.name.clone(),
        declared: column.r#type.clone().unwrap_or_default(),
    };
    match literal_of(expression) {
        Some(Literal::Value(text)) => kind.read(&text).ok_or_else(not_of_key_type),
        Some(Literal::OutsideKeyTypes) => Err(not_of_key_type()),
        None => Err(PartitionFilterRefusal::NotAFilter),
    }
}

/// One literal of the expression, as far as a partition key filter is concerned.
enum Literal {
    /// A literal carrying text to read in the key's own type: a quoted string, the digits
    /// of a number, or the quoted part of a typed literal.
    Value(String),
    /// A literal that is no value of any partition key type a filter compares: `NULL`,
    /// which no stored partition value is, and a boolean, which no filtered partition key
    /// is declared as.
    OutsideKeyTypes,
}

/// The literal `expression` is, if it is one. A sign ahead of a number belongs to the
/// number.
fn literal_of(expression: &Expr) -> Option<Literal> {
    match expression {
        Expr::Nested(inner) => literal_of(inner),
        Expr::UnaryOp {
            op: UnaryOperator::Plus,
            expr,
        } => literal_of(expr),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => match literal_of(expr) {
            Some(Literal::Value(text)) => Some(Literal::Value(format!("-{text}"))),
            other => other,
        },
        Expr::Value(value) => Some(value_literal(value)),
        Expr::TypedString(typed) => Some(value_literal(&typed.value)),
        _ => None,
    }
}

/// The text of one literal of the expression, for the operand that is text whatever the
/// key it filters is declared with: a `LIKE` pattern.
fn literal_text(expression: &Expr) -> Option<String> {
    match literal_of(expression) {
        Some(Literal::Value(text)) => Some(text),
        _ => None,
    }
}

/// The literal one parsed SQL value spells.
fn value_literal(value: &ValueWithSpan) -> Literal {
    match &value.value {
        SqlValue::Number(digits, _) => Literal::Value(digits.clone()),
        SqlValue::SingleQuotedString(text)
        | SqlValue::DoubleQuotedString(text)
        | SqlValue::EscapedStringLiteral(text)
        | SqlValue::UnicodeStringLiteral(text) => Literal::Value(text.clone()),
        // `NULL` and a boolean are values of no partition key type a filter compares.
        _ => Literal::OutsideKeyTypes,
    }
}
