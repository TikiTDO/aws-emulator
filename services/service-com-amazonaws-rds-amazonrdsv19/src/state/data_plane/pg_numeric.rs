// SPDX-License-Identifier: CC0-1.0

//! The EXACT DECIMAL type both engines call `numeric` (and `decimal`), and the arithmetic
//! over it.
//!
//! `numeric` is not floating point: PostgreSQL documents it as "especially recommended for
//! storing monetary amounts and other quantities where exactness is required", with
//! "calculations … done exactly" and "very slow" compared with the binary floating types.
//! The embedded engine (SQLite) has no such type — its `NUMERIC` affinity holds an integer
//! or a binary float — so a value of this type is held as the DIGITS it has, and the
//! arithmetic over it is done on those digits by [`::bigdecimal`], an arbitrary-precision
//! decimal.
//!
//! Two halves, which is why this is one module:
//!
//! * the engine functions this registers on every connection
//!   ([`register_numeric_functions`]): one that READS a value as an exact decimal (a cast)
//!   and one per arithmetic operator;
//! * the rewriting that puts them in a statement's place ([`exact_decimal_arithmetic`]): a
//!   cast to `numeric` becomes the reading function, and an operator one of whose operands
//!   is a `numeric` expression becomes that operator's function, so `0.1::numeric +
//!   0.2::numeric` is added as decimals rather than as doubles. The statement is REWRITTEN
//!   through its parse, so the operators keep their own precedence and nothing is matched in
//!   its text.
//!
//! The result column is reported under the `numeric` type the value has
//! (`super::pg_server::function_result_type`), so a client decodes it as its own decimal
//! type rather than as a float.
//!
//! What stays binary floating point, stated rather than hidden: a value this reaches is one
//! a statement CASTS to `numeric`, and the arithmetic over the expression that cast roots. A
//! column of a table declared `numeric` is held by the engine in its own affinity, so a sum
//! over such a column is the engine's.

use bigdecimal::{BigDecimal, RoundingMode, Zero};
use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use sqlparser::ast::{DataType, Expr, ExactNumberInfo, UnaryOperator};
use sqlparser::dialect::GenericDialect;

use super::sql_engine::EngineError;

/// How many digits beyond the point a QUOTIENT is carried to before it is rounded.
///
/// PostgreSQL computes a numeric division to a scale of at least sixteen significant
/// digits (`select_div_scale` in its own `numeric.c`), which for operands of small scale is
/// what `1::numeric / 3` answers with: twenty digits after the point. A quotient that
/// divides exactly is reported with its trailing zeros removed, so this is a ceiling and
/// not a fixed scale.
const DIVISION_SCALE: i64 = 20;

/// How many DIGITS one value may carry, which bounds the work one statement can ask of the
/// decimal arithmetic: every value read, computed or rendered here is held to it, so no
/// number a statement names — however short the text that names it — can grow the process.
/// A product of two values at this bound is twice as long, and a statement that would build
/// a longer value is refused rather than computed.
const MAX_DIGITS: i64 = 1_000;

/// The largest PRECISION a declared `numeric(p, s)` may carry: "the precision must be
/// positive, [and] up to 1000" (PostgreSQL, "Numeric Types"). A declaration past it is
/// refused as the invalid declaration it is, which is also what the real service answers.
const MAX_PRECISION: i64 = 1_000;

/// The range a declared SCALE may lie in: "the scale … can be specified as between
/// -1000 and 1000" (PostgreSQL, "Numeric Types").
const MAX_SCALE: i64 = 1_000;

/// The functions this module's rewriting puts in a statement's place, each under the name
/// the rewriting uses: the reading of a value as an exact decimal and the arithmetic over
/// it.
///
/// `super::pg_server::function_result_type` reports each as `numeric`, which is the type the
/// wire then reports for a column one of them produced.
pub const NUMERIC_FUNCTIONS: [&str; 6] = [
    "pg_numeric",
    "pg_numeric_add",
    "pg_numeric_sub",
    "pg_numeric_mul",
    "pg_numeric_div",
    "pg_numeric_neg",
];

/// The DIVISION the engines' own `/` is, which is not exact-decimal arithmetic: its value is
/// an integer where both operands are integers and a double otherwise, exactly as the
/// embedded engine computes it, and what it adds is the refusal a divisor of zero gets. It is
/// therefore not one of [`NUMERIC_FUNCTIONS`] — the column it produces carries the type of
/// the value it answers with — while the engine still has to admit it
/// ([`is_engine_function`]).
pub const DIVISION_FUNCTION: &str = "pg_divide";

/// Whether `function` is one of [`NUMERIC_FUNCTIONS`].
#[must_use]
pub fn is_numeric_function(function: &str) -> bool {
    NUMERIC_FUNCTIONS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(function))
}

/// Whether `function` is one this module registers on the engine's connections, which is
/// what the engine's own function allow-list admits (`super::sql_engine`).
#[must_use]
pub fn is_engine_function(function: &str) -> bool {
    is_numeric_function(function) || function.eq_ignore_ascii_case(DIVISION_FUNCTION)
}

/// Register the exact-decimal functions on `conn`, so a statement the rewriting below has
/// put one in is planned and run by the engine like any other.
pub fn register_numeric_functions(conn: &Connection) -> Result<(), EngineError> {
    // `pg_numeric(value [, precision, scale])` — a value READ as an exact decimal, which is
    // what a cast to `numeric` is. With a precision and a scale it is rounded to that scale
    // ("the value will be rounded to the specified number of fractional digits") and refused
    // where it no longer fits the precision, which is PostgreSQL's `numeric field overflow`.
    scalar(conn, "pg_numeric", -1, |context| {
        // The call shape FIRST: the function takes a value, or a value with the precision
        // and the scale its declaration carries, and nothing else is a call of it.
        if !matches!(context.len(), 1 | 3) {
            return Err(failed(
                "pg_numeric takes a value, or a value with a precision and a scale".to_string(),
            ));
        }
        let Some(value) = decimal_argument(context, 0)? else {
            return Ok(None);
        };
        if context.len() < 3 {
            return Ok(Some(rendered(&value)));
        }
        // The precision and the scale the DECLARATION carries, which are the client's own
        // numbers: a declaration outside the range PostgreSQL publishes for them is refused
        // as the invalid declaration it is, BEFORE the value is rescaled to it — rescaling
        // to a scale of a client's choosing is what would grow the process.
        let precision = context.get::<i64>(1).unwrap_or_default();
        let scale = context.get::<i64>(2).unwrap_or_default();
        if let Err(refusal) = declaration_within_its_range(precision, scale) {
            return Err(failed(refusal));
        }
        let rounded = value.with_scale_round(scale, RoundingMode::HalfUp);
        if digits_before_the_point(&rounded) > precision - scale {
            // PostgreSQL's own `numeric_value_out_of_range` sentence; what a client reads
            // beside it is the precision and the scale it declared.
            return Err(failed(
                super::sql_engine::NUMERIC_FIELD_OVERFLOW.to_string(),
            ));
        }
        Ok(Some(rounded.to_plain_string()))
    })?;
    scalar(conn, "pg_numeric_add", 2, |context| {
        binary(context, |left, right| Ok(left + right))
    })?;
    scalar(conn, "pg_numeric_sub", 2, |context| {
        binary(context, |left, right| Ok(left - right))
    })?;
    scalar(conn, "pg_numeric_mul", 2, |context| {
        binary(context, |left, right| Ok(left * right))
    })?;
    scalar(conn, "pg_numeric_div", 2, |context| {
        binary(context, |left, right| {
            if right.is_zero() {
                // PostgreSQL's own `division_by_zero` sentence.
                return Err(failed(super::sql_engine::DIVISION_BY_ZERO.to_string()));
            }
            Ok((left / right).with_scale_round(DIVISION_SCALE, RoundingMode::HalfUp))
        })
    })?;
    // `pg_divide(dividend, divisor)` — the division the ENGINE itself does, with the one
    // thing PostgreSQL does differently: a divisor of zero is its `division_by_zero` error
    // where the embedded engine answers NULL. Integer operands divide to an integer and
    // anything else to a double, which is what PostgreSQL and the embedded engine both do —
    // so the value it answers with carries the same type the operator's own answer would.
    conn.create_scalar_function(
        DIVISION_FUNCTION,
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |context| {
            let (dividend, divisor) = (context.get_raw(0), context.get_raw(1));
            if matches!(dividend, ValueRef::Null) || matches!(divisor, ValueRef::Null) {
                return Ok(rusqlite::types::Value::Null);
            }
            if let (ValueRef::Integer(dividend), ValueRef::Integer(divisor)) = (dividend, divisor)
            {
                if divisor == 0 {
                    return Err(failed(super::sql_engine::DIVISION_BY_ZERO.to_string()));
                }
                return Ok(rusqlite::types::Value::Integer(
                    dividend.wrapping_div(divisor),
                ));
            }
            let (dividend, divisor) = (as_double(dividend), as_double(divisor));
            if divisor == 0.0 {
                return Err(failed(super::sql_engine::DIVISION_BY_ZERO.to_string()));
            }
            Ok(rusqlite::types::Value::Real(dividend / divisor))
        },
    )
    .map_err(|error| {
        EngineError::new(
            super::sql_engine::EngineErrorKind::Internal,
            error.to_string(),
        )
    })?;
    scalar(conn, "pg_numeric_neg", 1, |context| {
        let Some(value) = decimal_argument(context, 0)? else {
            return Ok(None);
        };
        Ok(Some(rendered(&(-value))))
    })?;    Ok(())
}

/// One of this module's functions, registered on `conn`.
fn scalar<F>(conn: &Connection, name: &str, arguments: i32, body: F) -> Result<(), EngineError>
where
    F: Fn(&Context<'_>) -> rusqlite::Result<Option<String>> + Send + 'static,
{
    conn.create_scalar_function(
        name,
        arguments,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |context| body(context),
    )
    .map_err(|error| {
        EngineError::new(
            super::sql_engine::EngineErrorKind::Internal,
            error.to_string(),
        )
    })
}

/// One arithmetic operator over two exact decimals: `NULL` where either operand is SQL
/// NULL, as every arithmetic operator of both engines answers.
///
/// The RESULT is held to [`MAX_DIGITS`] as the operands are, so a product or a quotient
/// cannot grow past the bound either.
fn binary<F>(context: &Context<'_>, apply: F) -> rusqlite::Result<Option<String>>
where
    F: Fn(BigDecimal, BigDecimal) -> rusqlite::Result<BigDecimal>,
{
    let (Some(left), Some(right)) = (decimal_argument(context, 0)?, decimal_argument(context, 1)?)
    else {
        return Ok(None);
    };
    let computed = apply(left, right)?;
    within_the_bound(&computed)?;
    Ok(Some(rendered(&computed)))
}

/// Whether a DECLARED `numeric(precision, scale)` lies in the range PostgreSQL publishes for
/// it, as the sentence it refuses one outside it with.
///
/// Checked BEFORE a value is rescaled to the declaration, because the scale decides how many
/// digits the rescaled value has: a declaration a client chose is not a width this endpoint
/// will build.
fn declaration_within_its_range(precision: i64, scale: i64) -> Result<(), String> {
    if !(1..=MAX_PRECISION).contains(&precision) {
        return Err(format!(
            "NUMERIC precision {precision} must be between 1 and {MAX_PRECISION}"
        ));
    }
    if !(-MAX_SCALE..=MAX_SCALE).contains(&scale) || scale > precision {
        return Err(format!(
            "NUMERIC scale {scale} must be between {} and {precision}",
            -MAX_SCALE
        ));
    }
    Ok(())
}

/// Whether `value` carries no more than [`MAX_DIGITS`] digits, as the refusal that says so.
///
/// The MAGNITUDE is what is bounded, not the length of any text: a value's digits are the
/// digits of its mantissa plus the zeros its exponent puts before or after them, which is
/// what rendering it has to write out.
fn within_the_bound(value: &BigDecimal) -> rusqlite::Result<()> {
    let significant = i64::try_from(value.digits()).unwrap_or(i64::MAX);
    let scale = value.fractional_digit_count().saturating_abs();
    if significant.saturating_add(scale) > MAX_DIGITS {
        return Err(failed(format!(
            "this endpoint serves numeric values of at most {MAX_DIGITS} digits"
        )));
    }
    Ok(())
}

/// The argument at `index` as an exact decimal: `None` for SQL NULL, and the engine's own
/// refusal for a value that is no decimal at all — PostgreSQL's `invalid input syntax` — or
/// one past [`MAX_DIGITS`].
fn decimal_argument(
    context: &Context<'_>,
    index: usize,
) -> rusqlite::Result<Option<BigDecimal>> {
    // The argument has to BE there: a function of this module is reached through the
    // rewriting above, which calls it with the arguments it takes, but the name is also one
    // a client's own statement can write.
    if index >= context.len() {
        return Err(failed(format!(
            "this function takes more arguments than the {} it was called with",
            context.len()
        )));
    }
    let text = match context.get_raw(index) {
        ValueRef::Null => return Ok(None),
        ValueRef::Integer(number) => number.to_string(),
        ValueRef::Real(number) => number.to_string(),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            String::from_utf8_lossy(bytes).trim().to_string()
        }
    };
    // The text first, so an operand longer than any value this serves is refused before it
    // is parsed at all, and then the VALUE it denotes: a short text can name a number with
    // more digits than the process should hold (`1e1000000000`).
    if i64::try_from(text.len()).unwrap_or(i64::MAX) > MAX_DIGITS {
        return Err(failed(format!(
            "this endpoint serves numeric values of at most {MAX_DIGITS} digits"
        )));
    }
    let value = text.parse::<BigDecimal>().map_err(|_| {
        failed(format!(
            "invalid input syntax for type numeric: \"{text}\""
        ))
    })?;
    within_the_bound(&value)?;
    Ok(Some(value))
}

/// A value as the wire carries it: the digits it has, with the trailing zeros a computation
/// left behind removed — the form both engines report an exact decimal in.
fn rendered(value: &BigDecimal) -> String {
    value.normalized().to_plain_string()
}

/// How many digits `value` has BEFORE the point, which is what a declared precision bounds
/// together with its scale.
fn digits_before_the_point(value: &BigDecimal) -> i64 {
    if value.is_zero() {
        return 0;
    }
    // `digits()` is how many significant digits the value's own mantissa carries, and the
    // fractional count is how many of them are after the point.
    let significant = i64::try_from(value.digits()).unwrap_or(i64::MAX);
    (significant - value.fractional_digit_count()).max(0)
}

/// A value as the engine reads it in arithmetic that is not exact-decimal: the number it
/// is, and the number its digits spell for a text value — which is the coercion the embedded
/// engine applies to the operands of an arithmetic operator.
fn as_double(value: ValueRef<'_>) -> f64 {
    match value {
        ValueRef::Integer(number) => number as f64,
        ValueRef::Real(number) => number,
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => String::from_utf8_lossy(bytes)
            .trim()
            .parse()
            .unwrap_or_default(),
        ValueRef::Null => 0.0,
    }
}

/// The engine's own refusal, carrying the sentence a client reads.
fn failed(message: String) -> rusqlite::Error {
    rusqlite::Error::UserFunctionError(message.into())
}

/// The statement with its arithmetic as the ENGINES compute it, or `None` for a statement
/// that carries none — which is handed to the embedded engine exactly as it was.
///
/// Two rewritings, both of them over the statement's PARSE, so each operator keeps the
/// operands its own precedence gives it and nothing is matched in the statement's text:
///
/// * a cast to `numeric` or `decimal` becomes the reading function, and an operator one of
/// whose operands is such an expression becomes that operator's exact-decimal function;
/// * on the PostgreSQL wire, every other DIVISION becomes the engine's own division with the
///   refusal PostgreSQL answers a divisor of zero with (`division by zero`), which the
///   embedded engine answers NULL for. The MySQL wire keeps the embedded engine's own answer:
///   `SELECT 1/0` is NULL there, because MySQL's `ERROR_FOR_DIVISION_BY_ZERO` governs the
///   statements that CHANGE data, not a query.
///
/// A statement the parser does not read is left alone, and the engine's own refusal is what
/// the client is told. So is a statement LARGER or DEEPER than this pass serves
/// ([`MAX_REWRITTEN_BYTES`], [`MAX_EXPRESSION_DEPTH`], [`MAX_REWRITES`]): the rewriting is
/// bounded in the statement's size, in the depth it follows and in how many nodes it
/// replaces, so no statement can drive it to the depth of its own operator chain or make it
/// re-render a subtree without limit.
#[must_use]
pub fn exact_decimal_arithmetic(wire: super::EngineWire, sql: &str) -> Option<String> {
    let upper = sql.to_ascii_uppercase();
    if sql.len() > MAX_REWRITTEN_BYTES
        || (!upper.contains("NUMERIC") && !upper.contains("DECIMAL") && !sql.contains('/'))
    {
        return None;
    }
    let mut statements = sqlparser::parser::Parser::parse_sql(&GenericDialect {}, sql).ok()?;
    let [statement] = statements.as_mut_slice() else {
        return None;
    };
    let mut budget = Budget {
        rewrites: 0,
        rewritten: false,
        guard_divisions: wire == super::EngineWire::Postgres,
    };
    let _ = sqlparser::ast::visit_expressions_mut(statement, |expr| {
        // The visitor reaches every expression of the statement, so the budget is what ends
        // the pass: once it is spent nothing more is rendered or parsed.
        if budget.spent() {
            return std::ops::ControlFlow::Break(());
        }
        exactly(expr, 0, &mut budget);
        std::ops::ControlFlow::<()>::Continue(())
    });
    budget.rewritten.then(|| statement.to_string())
}

/// How many BYTES of statement this pass rewrites. A longer statement is handed to the
/// engine as the client wrote it: the pass re-renders the subtree it replaces, so its work is
/// this bound times [`MAX_EXPRESSION_DEPTH`] and no statement can make it more. Well above
/// any statement whose arithmetic a client writes by hand, and below the statement ceiling
/// the session already carries (`super::wire_session::MAX_STATEMENT_BYTES`) — which the
/// extended protocol's `Describe` path does not apply, so the bound has to live here.
const MAX_REWRITTEN_BYTES: usize = 16 * 1024;

/// How DEEP into an expression this pass follows. An operator chain is a left-deep tree, so
/// its depth is its length: past this the expression is left as the client wrote it rather
/// than recursed into, which is what keeps this pass off the native stack's limit.
const MAX_EXPRESSION_DEPTH: usize = 32;

/// How many nodes one statement may have REPLACED. A replacement re-renders the subtree it
/// covers, and the budget is taken BEFORE that rendering, so this is what bounds the pass's
/// total work rather than the node count of the statement.
const MAX_REWRITES: usize = 256;

/// What one pass of the rewriting has spent and whether it changed anything; see
/// [`MAX_REWRITES`].
struct Budget {
    rewrites: usize,
    rewritten: bool,
    /// Whether a division that is not over exact decimals carries the refusal a divisor of
    /// zero gets, which is the PostgreSQL wire's answer and not the MySQL wire's.
    guard_divisions: bool,
}

impl Budget {
    /// Whether the budget is spent, which is what ends the pass: checked BEFORE a
    /// replacement is built, so no subtree is rendered for a replacement that would not be
    /// made.
    fn spent(&self) -> bool {
        self.rewrites >= MAX_REWRITES
    }

    /// Take one replacement out of the budget, answering whether there was one to take.
    fn spend(&mut self) -> bool {
        if self.spent() {
            return false;
        }
        self.rewrites += 1;
        self.rewritten = true;
        true
    }
}

/// Rewrite `expr` so that its arithmetic is the engines' own, answering whether its VALUE is
/// an exact decimal.
///
/// The answer is what decides the arithmetic around it: an operator with one exact-decimal
/// operand is an exact-decimal operator, which is the rule both engines follow for a
/// `numeric` operand (PostgreSQL: the numeric types are resolved to the exact one, so
/// `numeric + integer` is numeric).
///
/// `depth` is how far into the expression this already is, and `budget` what the pass has
/// left to spend: past either bound the expression is left exactly as the client wrote it.
fn exactly(expr: &mut Expr, depth: usize, budget: &mut Budget) -> bool {
    if depth >= MAX_EXPRESSION_DEPTH || budget.spent() {
        return false;
    }
    match expr {
        // A cast TO the exact-decimal type is the reading of its operand as one.
        Expr::Cast {
            expr: value,
            data_type,
            ..
        } => {
            let inner = exactly(value, depth + 1, budget);
            let Some(info) = exact_decimal(data_type) else {
                return false;
            };
            // The budget comes before the work: the replacement below renders the operand,
            // so an exhausted budget leaves the cast as the client wrote it without
            // rendering anything.
            if !budget.spend() {
                return inner;
            }
            let read = match info {
                Some((precision, scale)) => {
                    call("pg_numeric", &[value.to_string(), precision, scale])
                }
                None => call("pg_numeric", &[value.to_string()]),
            };
            match read {
                Some(read) => {
                    *expr = read;
                    true
                }
                // The call would not parse: the cast is left as it was and the engine
                // answers for it.
                None => inner,
            }
        }
        Expr::Nested(inner) => exactly(inner, depth + 1, budget),
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr: value,
        } => {
            if !exactly(value, depth + 1, budget) {
                return false;
            }
            if !budget.spend() {
                return true;
            }
            if let Some(negated) = call("pg_numeric_neg", &[value.to_string()]) {
                *expr = negated;
            }
            true
        }
        Expr::UnaryOp {
            op: UnaryOperator::Plus,
            expr: value,
        } => exactly(value, depth + 1, budget),
        Expr::BinaryOp { left, op, right } => {
            let exact = exactly(left, depth + 1, budget) || exactly(right, depth + 1, budget);
            let function = arithmetic_function(op);
            if exact {
                let Some(function) = function else {
                    return false;
                };
                if !budget.spend() {
                    return true;
                }
                if let Some(applied) = call(function, &[left.to_string(), right.to_string()]) {
                    *expr = applied;
                }
                return true;
            }
            // A DIVISION that is not over exact decimals is still the engine's own, which on
            // the PostgreSQL wire refuses a divisor of zero where the embedded engine answers
            // NULL: the engine's division with that refusal in it. Its value is not an exact
            // decimal.
            if budget.guard_divisions
                && matches!(op, sqlparser::ast::BinaryOperator::Divide)
                && budget.spend()
            {
                if let Some(guarded) =
                    call(DIVISION_FUNCTION, &[left.to_string(), right.to_string()])
                {
                    *expr = guarded;
                }
            }
            false
        }
        // A call of one of this module's own functions is already an exact decimal, which is
        // what makes the rewriting idempotent. The division guard is not one: its value is
        // the engine's own.
        Expr::Function(function) => is_numeric_function(&function.name.to_string()),
        _ => false,
    }
}

/// The function that computes `op` over exact decimals, or `None` for an operator that is
/// not arithmetic over them (a comparison, a concatenation, a logical operator): the engine
/// compares and concatenates the digits the value carries.
fn arithmetic_function(op: &sqlparser::ast::BinaryOperator) -> Option<&'static str> {
    use sqlparser::ast::BinaryOperator;
    match op {
        BinaryOperator::Plus => Some("pg_numeric_add"),
        BinaryOperator::Minus => Some("pg_numeric_sub"),
        BinaryOperator::Multiply => Some("pg_numeric_mul"),
        BinaryOperator::Divide => Some("pg_numeric_div"),
        _ => None,
    }
}

/// Whether `data_type` is the EXACT-DECIMAL type, and the precision and scale it declares
/// where it declares both: `Some(None)` for a bare `numeric`, which carries a value of any
/// precision, and `None` for every other type.
fn exact_decimal(data_type: &DataType) -> Option<Option<(String, String)>> {
    let info = match data_type {
        DataType::Numeric(info) | DataType::Decimal(info) | DataType::Dec(info) => info,
        _ => return None,
    };
    Some(match info {
        ExactNumberInfo::PrecisionAndScale(precision, scale) => {
            Some((precision.to_string(), scale.to_string()))
        }
        // A precision with no scale is a scale of zero, which is what the standard and both
        // engines read it as.
        ExactNumberInfo::Precision(precision) => Some((precision.to_string(), "0".to_string())),
        ExactNumberInfo::None => None,
    })
}

/// One call of `function` over `arguments`, built by parsing it: `None` where an operand
/// does not round-trip through the parser, which leaves the statement as it was.
fn call(function: &str, arguments: &[String]) -> Option<Expr> {
    let sql = format!("{function}({})", arguments.join(", "));
    sqlparser::parser::Parser::new(&GenericDialect {})
        .try_with_sql(&sql)
        .ok()?
        .parse_expr()
        .ok()
}

/// The statement is rewritten only where its parse says so; these pin the two halves of
/// that reading — which types are the exact-decimal one, and that the rewriting keeps the
/// operators' own precedence.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_statements_exact_decimal_arithmetic_is_rewritten_keeping_its_precedence() {
        // The rows span the forms the rule generalises over: a cast with and without a
        // precision, both spellings of the type, an operator per arithmetic operator, mixed
        // precedence, a comparison (which is not arithmetic), a nested cast and a statement
        // that carries no exact decimal at all.
        let cases: [(&str, Option<&str>); 11] = [
            (
                "SELECT CAST(0.1 AS NUMERIC) + CAST(0.2 AS NUMERIC)",
                Some("SELECT pg_numeric_add(pg_numeric(0.1), pg_numeric(0.2))"),
            ),
            (
                "SELECT CAST(CAST(1 AS NUMERIC) / 3 AS NUMERIC(10,4))",
                Some("SELECT pg_numeric(pg_numeric_div(pg_numeric(1), 3), 10, 4)"),
            ),
            (
                "SELECT CAST(2 AS DECIMAL) * 3 - 1",
                Some("SELECT pg_numeric_sub(pg_numeric_mul(pg_numeric(2), 3), 1)"),
            ),
            (
                // Precedence: the multiplication binds tighter than the addition, so the
                // exact operator is the one over the product.
                "SELECT 1 + CAST(2 AS NUMERIC) * 3",
                Some("SELECT pg_numeric_add(1, pg_numeric_mul(pg_numeric(2), 3))"),
            ),
            (
                "SELECT -CAST(2.5 AS NUMERIC)",
                Some("SELECT pg_numeric_neg(pg_numeric(2.5))"),
            ),
            (
                // A comparison is not arithmetic: the digits are compared as they are.
                "SELECT CAST(1 AS NUMERIC) = 1",
                Some("SELECT pg_numeric(1) = 1"),
            ),
            (
                "SELECT CAST(amount AS NUMERIC(12,2)) FROM t WHERE id = 1",
                Some("SELECT pg_numeric(amount, 12, 2) FROM t WHERE id = 1"),
            ),
            // A division that is not over exact decimals is the engine's own, with the
            // refusal a divisor of zero gets.
            ("SELECT 1 / 0", Some("SELECT pg_divide(1, 0)")),
            (
                "SELECT a / b FROM t",
                Some("SELECT pg_divide(a, b) FROM t"),
            ),
            // A statement with no exact-decimal cast and no division is left exactly as it
            // was.
            ("SELECT 0.1 + 0.2", None),
            ("SELECT count(*) FROM t", None),
        ];
        for (sql, expected) in cases {
            let rewritten = exact_decimal_arithmetic(super::super::EngineWire::Postgres, sql);
            assert_eq!(
                rewritten.as_deref(),
                expected,
                "case {sql:?}: expected {expected:?}, got {rewritten:?}"
            );
        }
        // The MySQL wire keeps the embedded engine's own division, which answers NULL for a
        // divisor of zero as MySQL does for a query; its exact-decimal casts are rewritten
        // exactly as PostgreSQL's are.
        for (sql, expected) in [
            ("SELECT 1 / 0", None),
            (
                "SELECT CAST(0.1 AS DECIMAL) + CAST(0.2 AS DECIMAL)",
                Some("SELECT pg_numeric_add(pg_numeric(0.1), pg_numeric(0.2))"),
            ),
        ] {
            let rewritten = exact_decimal_arithmetic(super::super::EngineWire::MySql, sql);
            assert_eq!(
                rewritten.as_deref(),
                expected,
                "case {sql:?} on the MySQL wire: expected {expected:?}, got {rewritten:?}"
            );
        }
    }

    #[test]
    fn a_value_or_a_declaration_past_what_this_endpoint_serves_is_refused() {
        // The rows span what the bounds generalise over: a value whose MAGNITUDE is past the
        // bound although its text is short, one whose text is long, one that fits, and the
        // declarations outside the range PostgreSQL publishes.
        for (value, within) in [
            ("1", true),
            ("0.000000001", true),
            ("1e999", true),
            ("1e1000000000", false),
            ("1e-1000000000", false),
        ] {
            let parsed = value.parse::<BigDecimal>().expect("the value parses");
            let answered = within_the_bound(&parsed).is_ok();
            assert_eq!(
                answered, within,
                "case {value:?}: expected within={within}, got {answered}"
            );
        }
        for (precision, scale, declared) in [
            (10i64, 4i64, true),
            (1, 0, true),
            (1_000, 1_000, true),
            (0, 0, false),
            (1_001, 2, false),
            (10, 11, false),
            (10, -1_001, false),
            (2_000_000_000, 1_000_000_000, false),
        ] {
            let answered = declaration_within_its_range(precision, scale).is_ok();
            assert_eq!(
                answered, declared,
                "case numeric({precision},{scale}): expected declared={declared}, got \
                 {answered}"
            );
        }
    }

    #[test]
    fn a_value_past_the_precision_its_cast_declares_is_refused() {
        // The rows span what the bound generalises over: a value that fits, one whose
        // rounding makes it fit, one past the precision, a negative one, and zero.
        let cases: [(&str, i64, i64, bool); 5] = [
            ("123.45", 5, 2, true),
            ("123.456", 5, 2, true),
            ("12345.678", 5, 2, false),
            ("-1234.5", 5, 2, false),
            ("0", 5, 2, true),
        ];
        for (value, precision, scale, fits) in cases {
            let rounded = value
                .parse::<BigDecimal>()
                .expect("the value parses")
                .with_scale_round(scale, RoundingMode::HalfUp);
            let within = digits_before_the_point(&rounded) <= precision - scale;
            assert_eq!(
                within, fits,
                "case {value:?} as numeric({precision},{scale}): expected fits={fits}, got \
                 {within} for {rounded}"
            );
        }
    }

    #[test]
    fn an_exact_decimal_is_rendered_with_the_digits_it_has() {
        // The rows span what the rendering generalises over: a sum that is exact in decimal
        // and not in binary floating point, a quotient that does not divide, a value with
        // trailing zeros, and a negative value.
        let cases: [(&str, &str); 4] = [
            ("0.30", "0.3"),
            ("0.33333333333333333333", "0.33333333333333333333"),
            ("12.500", "12.5"),
            ("-0.10", "-0.1"),
        ];
        for (value, expected) in cases {
            let rendered = rendered(&value.parse::<BigDecimal>().expect("the value parses"));
            assert_eq!(
                rendered, expected,
                "case {value:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }
}
