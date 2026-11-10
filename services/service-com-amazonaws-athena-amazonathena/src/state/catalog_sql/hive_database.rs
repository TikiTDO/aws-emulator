// SPDX-License-Identifier: CC0-1.0

//! The `CREATE DATABASE` clauses Hive's DDL grammar takes, which the general SQL parser
//! does not read.
//!
//! Athena serves the Hive DDL form of a database definition —
//! `CREATE DATABASE [IF NOT EXISTS] <name> [COMMENT '<text>'] [LOCATION '<uri>']
//! [WITH DBPROPERTIES ('<key>' = '<value>', …)]` — and the parser this crate reads
//! statements with reads the name, the tolerance clause and the location but not the
//! description or the properties. This module reads THAT clause sequence off the same
//! maintained TOKENIZER the parser lexes with, so a definition carrying those clauses is
//! understood as the catalog statement it is instead of being handed to the engine as
//! text it cannot run.
//!
//! Nothing here parses SQL in general: it reads one fixed clause sequence, and answers
//! `None` for anything else — including a `CREATE DATABASE` whose name or clauses are not
//! what the grammar allows, which is then reported as the malformed statement it is
//! ([`super::grammar`]).

use ::sqlparser::dialect::GenericDialect;
use ::sqlparser::keywords::Keyword;
use ::sqlparser::tokenizer::{Token, TokenWithSpan, Tokenizer, Word};

use super::statement::Position;

/// One `CREATE DATABASE` statement, as its clauses state it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseDefinition {
    /// The database name, as written.
    pub name: String,
    /// Where the name appears in the submitted text.
    pub at: Position,
    /// Whether the statement tolerates a database of that name already being there.
    pub if_not_exists: bool,
    /// The `COMMENT`, which the catalog holds as the database's description.
    pub comment: Option<String>,
    /// The `LOCATION`, which the catalog holds as the database's location URI.
    pub location: Option<String>,
    /// The `WITH DBPROPERTIES` entries, which the catalog holds as its parameters.
    pub properties: Vec<(String, String)>,
}

/// The database definition `sql` states, or `None` for text that is not one.
pub fn read_database_definition(sql: &str) -> Option<DatabaseDefinition> {
    let tokens = significant_tokens(sql)?;
    let mut rest = tokens.as_slice();
    rest = keyword(rest, Keyword::CREATE)?;
    rest = keyword(rest, Keyword::DATABASE).or_else(|| keyword(rest, Keyword::SCHEMA))?;
    let if_not_exists = match keyword(rest, Keyword::IF)
        .and_then(|rest| keyword(rest, Keyword::NOT))
        .and_then(|rest| keyword(rest, Keyword::EXISTS))
    {
        Some(after) => {
            rest = after;
            true
        }
        None => false,
    };
    let (name, at, mut rest) = name_of(rest)?;
    let mut definition = DatabaseDefinition {
        name,
        at,
        if_not_exists,
        comment: None,
        location: None,
        properties: Vec::new(),
    };
    // The clauses, in any order and each at most once: a clause stated twice is not a
    // statement this grammar takes.
    loop {
        if let Some(after) = keyword(rest, Keyword::COMMENT) {
            let (text, after) = quoted_text(after)?;
            if definition.comment.replace(text).is_some() {
                return None;
            }
            rest = after;
            continue;
        }
        if let Some(after) = keyword(rest, Keyword::LOCATION) {
            let (text, after) = quoted_text(after)?;
            if definition.location.replace(text).is_some() {
                return None;
            }
            rest = after;
            continue;
        }
        let properties = keyword(rest, Keyword::WITH)
            .and_then(|after| unquoted_word(after, DBPROPERTIES));
        if let Some(after) = properties {
            if !definition.properties.is_empty() {
                return None;
            }
            let (entries, after) = property_entries(after)?;
            definition.properties = entries;
            rest = after;
            continue;
        }
        break;
    }
    // A statement is read to its END: text carrying anything after the clauses is not
    // this statement, whatever its first words said.
    match rest {
        [] => Some(definition),
        [last] if matches!(last.token, Token::SemiColon) => Some(definition),
        _ => None,
    }
}

/// The tokens `sql` carries that the grammar reads, or `None` for text the tokenizer
/// cannot read at all.
fn significant_tokens(sql: &str) -> Option<Vec<TokenWithSpan>> {
    Some(
        Tokenizer::new(&GenericDialect {}, sql)
            .tokenize_with_location()
            .ok()?
            .into_iter()
            .filter(|token| !matches!(token.token, Token::Whitespace(..)))
            .collect(),
    )
}

/// What is left after `keyword`, or `None` when the next token is not that keyword.
fn keyword(tokens: &[TokenWithSpan], keyword: Keyword) -> Option<&[TokenWithSpan]> {
    let (first, rest) = tokens.split_first()?;
    match &first.token {
        // A KEYWORD is matched as the keyword the tokenizer recognised, so a name written
        // in quotes is never read as one.
        Token::Word(Word {
            keyword: written,
            quote_style: None,
            ..
        }) if *written == keyword => Some(rest),
        _ => None,
    }
}

/// What is left after the word `text`, or `None` when the next token is not that word
/// written without quotes. The tokenizer recognises no keyword for the word a properties
/// clause is named by, so it is matched as the word it is.
fn unquoted_word<'t>(tokens: &'t [TokenWithSpan], text: &str) -> Option<&'t [TokenWithSpan]> {
    let (first, rest) = tokens.split_first()?;
    match &first.token {
        Token::Word(Word {
            value,
            quote_style: None,
            ..
        }) if value.eq_ignore_ascii_case(text) => Some(rest),
        _ => None,
    }
}

/// The word Hive's grammar names a database's properties clause by.
const DBPROPERTIES: &str = "DBPROPERTIES";

/// The name the next token states, where it is, and what is left after it.
fn name_of(tokens: &[TokenWithSpan]) -> Option<(String, Position, &[TokenWithSpan])> {
    let (first, rest) = tokens.split_first()?;
    match &first.token {
        Token::Word(word) if word.quote_style.is_some() || word.keyword == Keyword::NoKeyword => {
            Some((word.value.clone(), position_of(first), rest))
        }
        _ => None,
    }
}

/// The text the next token states as a string literal, and what is left after it.
fn quoted_text(tokens: &[TokenWithSpan]) -> Option<(String, &[TokenWithSpan])> {
    let (first, rest) = tokens.split_first()?;
    match &first.token {
        Token::SingleQuotedString(text) | Token::DoubleQuotedString(text) => {
            Some((text.clone(), rest))
        }
        _ => None,
    }
}

/// The entries a properties clause states, and what is left of the tokens after them.
type Entries<'t> = (Vec<(String, String)>, &'t [TokenWithSpan]);

/// The `('<key>' = '<value>', …)` entries a properties clause states, and what is left
/// after them.
fn property_entries(tokens: &[TokenWithSpan]) -> Option<Entries<'_>> {
    let (open, mut rest) = tokens.split_first()?;
    if !matches!(open.token, Token::LParen) {
        return None;
    }
    let mut entries: Vec<(String, String)> = Vec::new();
    loop {
        let (key, after) = quoted_text(rest)?;
        let (equals, after) = after.split_first()?;
        if !matches!(equals.token, Token::Eq) {
            return None;
        }
        let (value, after) = quoted_text(after)?;
        entries.push((key, value));
        let (next, after) = after.split_first()?;
        match next.token {
            Token::Comma => rest = after,
            Token::RParen => return Some((entries, after)),
            _ => return None,
        }
    }
}

/// Where `token` starts in the submitted text.
fn position_of(token: &TokenWithSpan) -> Position {
    let start = token.span.start;
    if start.line == 0 {
        return Position::START;
    }
    Position {
        line: start.line,
        column: start.column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clause sequence is read whole, and text that is not one is not read as one:
    /// the rows differ along which clauses are present, their order, the quoting of the
    /// name, and \u2014 for the rows that are read as nothing \u2014 what makes the text another
    /// statement.
    #[test]
    fn the_hive_database_clauses_are_read_off_the_tokens() {
        /// What one row states: the name, whether it tolerates the database being there,
        /// the description, the location, and how many properties it declared.
        type Stated = (&'static str, bool, Option<&'static str>, Option<&'static str>, usize);

        // (case, the text, what it states)
        let cases: Vec<(&str, &str, Option<Stated>)> = vec![
            (
                "a bare definition",
                "CREATE DATABASE shop",
                Some(("shop", false, None, None, 0)),
            ),
            (
                "a back-quoted name with a description",
                "CREATE DATABASE `shop-ddl` COMMENT 'the shop'",
                Some(("shop-ddl", false, Some("the shop"), None, 0)),
            ),
            (
                "every clause, in the grammar's order",
                "CREATE SCHEMA IF NOT EXISTS shop COMMENT 'c' LOCATION 's3://b/p/' \
                 WITH DBPROPERTIES ('owner' = 'shop', 'tier' = 'gold')",
                Some(("shop", true, Some("c"), Some("s3://b/p/"), 2)),
            ),
            (
                "the clauses in another order",
                "CREATE DATABASE shop LOCATION 's3://b/p/' COMMENT 'c'",
                Some(("shop", false, Some("c"), Some("s3://b/p/"), 0)),
            ),
            (
                "a trailing statement separator",
                "CREATE DATABASE shop;",
                Some(("shop", false, None, None, 0)),
            ),
            (
                "a name the grammar does not allow",
                "CREATE DATABASE shop-ddl",
                None,
            ),
            (
                "a clause stated twice",
                "CREATE DATABASE shop COMMENT 'one' COMMENT 'two'",
                None,
            ),
            (
                "a clause the grammar does not take",
                "CREATE DATABASE shop STORED AS TEXTFILE",
                None,
            ),
            (
                "another statement entirely",
                "CREATE EXTERNAL TABLE shop.items (id int)",
                None,
            ),
            ("no statement at all", "SELECT 1", None),
        ];

        for (case, sql, expected) in cases {
            let read = read_database_definition(sql);
            let actual = read.as_ref().map(|definition| {
                (
                    definition.name.as_str(),
                    definition.if_not_exists,
                    definition.comment.as_deref(),
                    definition.location.as_deref(),
                    definition.properties.len(),
                )
            });
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
