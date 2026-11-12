// SPDX-License-Identifier: CC0-1.0

//! `GRANT` and `REVOKE` on the PostgreSQL wire: the role memberships and the object
//! privileges a client changes, and the catalogs the server keeps them in.
//!
//! PostgreSQL's grammar defines two statements under each keyword, and this serves both:
//!
//! * ROLE MEMBERSHIP — `GRANT <role> TO <role>` / `REVOKE <role> FROM <role>`, which make
//!   one role a member of another. Membership is server state, so it is kept in
//!   `pg_auth_members` ("role membership … each row shows that one role is a member of
//!   another"), keyed by the object identifiers of the roles in `pg_roles`, exactly as
//!   PostgreSQL keys it — so a client reads a membership back by joining the two catalogs.
//!   This is the engine-side half of IAM database authentication: a database user becomes
//!   one that authenticates with an authentication token by being made a member of
//!   `rds_iam`, which is what [`super::iam_auth_token`] then admits.
//! * OBJECT PRIVILEGES — `GRANT <privileges> ON <object> TO <grantee>` and its `REVOKE`,
//!   which change what a role may do with an object. PostgreSQL holds those on the OBJECT,
//!   as the access control list of its catalog row (`pg_class.relacl` for a table or view,
//!   `pg_namespace.nspacl` for a schema, `pg_database.datacl` for a database), written in
//!   the `grantee=privileges/grantor` form its `aclitem` type renders — which is where a
//!   client reads them back from, and what this writes.
//!
//! Nothing here is answered from the shape of the statement's text: the roles, the objects
//! and the privileges a statement names are resolved against the catalogs this instance
//! holds, and a statement naming a role or an object that is not there is refused with
//! PostgreSQL's own sentence for that condition. An object class this server keeps no
//! catalog of holds no objects either, so a grant on one names something that does not
//! exist and is refused as such.

use sqlparser::tokenizer::Token;

use super::sql_engine::{Cell, EngineError, EngineErrorKind, InstanceDatabase, ResultSet};
use super::wire_session::{SessionFacts, StatementAnswer};
/// The access control list an object carries its privileges in, and the catalog row it
/// lives on: the `aclitem` text PostgreSQL writes them as.
mod access_list;
use access_list::*;

/// The pseudo-role every role is a member of: "the special role name PUBLIC … can be
/// thought of as an implicitly defined group that always includes all roles". It is not a
/// row in `pg_roles`, and in an access control list it is the entry with an empty grantee.
const PUBLIC_ROLE: &str = "public";

/// How many names one `GRANT`/`REVOKE` may name on either side of its separator, and how
/// many (name, grantee) PAIRS one statement may touch.
///
/// An EMULATOR bound, not a PostgreSQL one. The engine work such a statement drives is the
/// PRODUCT of its two lists, while the statement's own length
/// ([`super::wire_session::MAX_STATEMENT_BYTES`]) bounds only their SUM — so these caps are
/// what bound the product, which is the number of catalog writes and lookups one statement
/// can ask for. Both sit far above what a statement names in practice (a grant names a
/// handful of roles, objects and grantees) and far below where the work matters; a statement
/// past either is refused before any of that work is done, rather than being served
/// unbounded.
pub const MAX_IDENTIFIERS_PER_SIDE: usize = 64;
pub const MAX_GRANT_PAIRS: usize = MAX_IDENTIFIERS_PER_SIDE * MAX_IDENTIFIERS_PER_SIDE;

/// What a statement naming more than [`MAX_IDENTIFIERS_PER_SIDE`] names on one side is
/// refused with: this endpoint's own limit, worded as the endpoint's other limits are.
fn too_many_identifiers() -> EngineError {
    EngineError::new(
        EngineErrorKind::LimitExceeded,
        format!(
            "this endpoint serves a GRANT or REVOKE naming at most \
             {MAX_IDENTIFIERS_PER_SIDE} names on either side"
        ),
    )
}

/// What a statement whose two lists would touch more than [`MAX_GRANT_PAIRS`] pairs is
/// refused with, for the same reason.
fn too_many_pairs() -> EngineError {
    EngineError::new(
        EngineErrorKind::LimitExceeded,
        format!(
            "this endpoint serves a GRANT or REVOKE touching at most {MAX_GRANT_PAIRS} \
             name and grantee pairs"
        ),
    )
}

/// Whether `left` and `right` are lists this endpoint serves a statement over: each within
/// [`MAX_IDENTIFIERS_PER_SIDE`], and their product within [`MAX_GRANT_PAIRS`].
///
/// Checked BEFORE any engine work, so the loops below are bounded by this and a statement
/// past it performs nothing.
fn bounded_lists<T, U>(left: &[T], right: &[U]) -> Result<(), EngineError> {
    if left.len() > MAX_IDENTIFIERS_PER_SIDE || right.len() > MAX_IDENTIFIERS_PER_SIDE {
        return Err(too_many_identifiers());
    }
    if left.len().saturating_mul(right.len()) > MAX_GRANT_PAIRS {
        return Err(too_many_pairs());
    }
    Ok(())
}

/// One privilege, as a statement names it and as an `aclitem` abbreviates it.
///
/// Source: the "Privileges" table of the PostgreSQL documentation, which publishes each
/// privilege's abbreviation and the object types it applies to.
struct Privilege {
    /// The keyword a statement names it with.
    name: &'static str,
    /// The letter an access control list spells it with.
    letter: char,
    /// The object classes it applies to.
    applies_to: &'static [ObjectClass],
}

/// The classes of object this server holds a catalog of, which are the objects a privilege
/// can be granted on here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectClass {
    /// A table or a view: `pg_class`, whose `relacl` carries its privileges.
    Relation,
    /// A schema: `pg_namespace.nspacl`.
    Schema,
    /// A database: `pg_database.datacl`.
    Database,
}

impl ObjectClass {
    /// The catalog, the column naming the object, and the column holding its access
    /// control list.
    fn catalog(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Relation => ("pg_class", "relname", "relacl"),
            Self::Schema => ("pg_namespace", "nspname", "nspacl"),
            Self::Database => ("pg_database", "datname", "datacl"),
        }
    }

    /// What the statement calls this class of object, which is how a refusal names it.
    fn noun(self) -> &'static str {
        match self {
            Self::Relation => "relation",
            Self::Schema => "schema",
            Self::Database => "database",
        }
    }

    /// The object type a refusal about a privilege names, as PostgreSQL words it.
    fn privilege_noun(self) -> &'static str {
        match self {
            Self::Relation => "table",
            Self::Schema => "schema",
            Self::Database => "database",
        }
    }
}

/// Every privilege this server serves a grant of, with the object classes each applies to.
///
/// In the order PostgreSQL renders an access control list in, which is the order of the
/// privilege bits themselves: `arwdDxt` for a table, `UC` for a schema, `CTc` for a
/// database.
const PRIVILEGES: &[Privilege] = &[
    Privilege {
        name: "INSERT",
        letter: 'a',
        applies_to: &[ObjectClass::Relation],
    },
    Privilege {
        name: "SELECT",
        letter: 'r',
        applies_to: &[ObjectClass::Relation],
    },
    Privilege {
        name: "UPDATE",
        letter: 'w',
        applies_to: &[ObjectClass::Relation],
    },
    Privilege {
        name: "DELETE",
        letter: 'd',
        applies_to: &[ObjectClass::Relation],
    },
    Privilege {
        name: "TRUNCATE",
        letter: 'D',
        applies_to: &[ObjectClass::Relation],
    },
    Privilege {
        name: "REFERENCES",
        letter: 'x',
        applies_to: &[ObjectClass::Relation],
    },
    Privilege {
        name: "TRIGGER",
        letter: 't',
        applies_to: &[ObjectClass::Relation],
    },
    Privilege {
        name: "USAGE",
        letter: 'U',
        applies_to: &[ObjectClass::Schema],
    },
    Privilege {
        name: "CREATE",
        letter: 'C',
        applies_to: &[ObjectClass::Schema, ObjectClass::Database],
    },
    Privilege {
        name: "TEMPORARY",
        letter: 'T',
        applies_to: &[ObjectClass::Database],
    },
    Privilege {
        name: "CONNECT",
        letter: 'c',
        applies_to: &[ObjectClass::Database],
    },
    // A privilege PostgreSQL defines for objects this server holds none of (a function, a
    // procedure): it is a privilege the grammar knows, so a statement naming it is READ —
    // and then refused for the object it names, or as a privilege the named class does not
    // admit, rather than as a statement the grammar does not define.
    Privilege {
        name: "EXECUTE",
        letter: 'X',
        applies_to: &[],
    },
];

/// The privilege the keyword `word` names, with `TEMP` read as `TEMPORARY`, which
/// PostgreSQL documents as its synonym.
fn privilege(word: &str) -> Option<&'static Privilege> {
    let named = if word.eq_ignore_ascii_case("TEMP") {
        "TEMPORARY"
    } else {
        word
    };
    PRIVILEGES
        .iter()
        .find(|privilege| privilege.name.eq_ignore_ascii_case(named))
}

/// Every privilege `class` admits, in the order [`PRIVILEGES`] lists them — which is the
/// order PostgreSQL renders an access control list in, and what `ALL PRIVILEGES` grants.
fn all_privileges(class: ObjectClass) -> Vec<char> {
    PRIVILEGES
        .iter()
        .filter(|privilege| privilege.applies_to.contains(&class))
        .map(|privilege| privilege.letter)
        .collect()
}

/// Serve `GRANT` / `REVOKE` for a PostgreSQL session, or `None` for a statement that is
/// neither — which is then the engine's to run.
pub fn grant_statement(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    words: &[String],
    tokens: &[Token],
) -> Result<Option<StatementAnswer>, EngineError> {
    let verb = match words.first().map(String::as_str).unwrap_or_default() {
        "GRANT" => Verb::Grant,
        "REVOKE" => Verb::Revoke,
        _ => return Ok(None),
    };
    // The catalogs this statement reads and writes, brought up to date with the instance
    // first: the roles it resolves against, and the objects it can name.
    super::pg_server::refresh_catalogs(
        database,
        facts,
        &[
            "pg_roles",
            "pg_auth_members",
            "pg_database",
            "pg_namespace",
            "pg_class",
        ],
    )?;
    match statement_parts(verb, tokens) {
        Ok(Parts::RoleMembership { roles, members }) => {
            role_membership(database, facts, verb, &roles, &members).map(Some)
        }
        Ok(Parts::ObjectPrivileges {
            privileges,
            objects,
            grantees,
        }) => object_privileges(database, facts, verb, &privileges, &objects, &grantees).map(Some),
        // A statement whose grammar this server does not define: the keyword itself is
        // read, so the refusal names the token the reading stopped on — PostgreSQL's own
        // parse error — or the end of the statement where it ran out.
        Err(unreadable) => Err(EngineError::new(
            EngineErrorKind::Syntax,
            match unreadable {
                Unreadable::At(token) => format!("syntax error at or near \"{token}\""),
                Unreadable::EndOfInput => "syntax error at end of input".to_string(),
            },
        )),
    }
}

/// Why a statement could not be read: the token the reading stopped on, or the end of the
/// statement.
enum Unreadable {
    EndOfInput,
    At(String),
}

/// Which direction a statement moves a membership or a privilege in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Grant,
    Revoke,
}

impl Verb {
    /// The `CommandComplete` tag the statement answers with.
    fn tag(self) -> &'static str {
        match self {
            Self::Grant => "GRANT",
            Self::Revoke => "REVOKE",
        }
    }

    /// The keyword that separates what is granted from whom it is granted to.
    fn separator(self) -> &'static str {
        match self {
            Self::Grant => "TO",
            Self::Revoke => "FROM",
        }
    }
}

/// What one statement says, read from its tokens.
enum Parts {
    /// `GRANT <roles> TO <members>` and its `REVOKE`.
    RoleMembership {
        roles: Vec<String>,
        members: Vec<String>,
    },
    /// `GRANT <privileges> ON [<class>] <objects> TO <grantees>` and its `REVOKE`.
    ObjectPrivileges {
        /// The privilege letters the statement names; empty for `ALL PRIVILEGES`, which
        /// stands for every privilege the class admits.
        privileges: Vec<char>,
        objects: Objects,
        grantees: Vec<String>,
    },
}

/// Read a `GRANT`/`REVOKE` into its parts, or why it could not be read.
fn statement_parts(verb: Verb, tokens: &[Token]) -> Result<Parts, Unreadable> {
    let words: Vec<String> = tokens
        .iter()
        .map(|token| match token {
            Token::Word(word) => word.value.to_ascii_uppercase(),
            other => other.to_string(),
        })
        .collect();
    // The keyword that separates the two halves, and the `ON` that makes the statement a
    // privilege grant rather than a role grant.
    let at_separator = words
        .iter()
        .position(|word| word.as_str() == verb.separator())
        .filter(|at| *at > 0)
        .ok_or(Unreadable::EndOfInput)?;
    let at_on = words
        .iter()
        .take(at_separator)
        .position(|word| word.as_str() == "ON");
    // `WITH {ADMIN|GRANT} OPTION`, `GRANTED BY <role>`, `CASCADE` and `RESTRICT` are
    // clauses about the grant rather than part of the list of grantees.
    let tail = words
        .iter()
        .enumerate()
        .skip(at_separator + 1)
        .find(|(_, word)| {
            matches!(
                word.as_str(),
                "WITH" | "GRANTED" | "CASCADE" | "RESTRICT"
            )
        })
        .map_or(words.len(), |(at, _)| at);
    let grantees = identifiers(tokens.get(at_separator + 1..tail).unwrap_or_default());
    if grantees.is_empty() {
        return Err(Unreadable::EndOfInput);
    }
    // `REVOKE {ADMIN|GRANT} OPTION FOR <what>`: what the statement is about starts after
    // the `FOR`.
    let subject_from = words
        .iter()
        .take(at_on.unwrap_or(at_separator))
        .position(|word| word.as_str() == "FOR")
        .map_or(1, |at| at + 1);
    match at_on {
        None => Ok(Parts::RoleMembership {
            roles: non_empty(identifiers(
                tokens.get(subject_from..at_separator).unwrap_or_default(),
            ))
            .ok_or(Unreadable::EndOfInput)?,
            members: grantees,
        }),
        Some(at_on) => {
            let named = words.get(subject_from..at_on).unwrap_or_default();
            let privileges = named_privileges(named)?;
            let objects = object_list(tokens.get(at_on + 1..at_separator).unwrap_or_default())?;
            Ok(Parts::ObjectPrivileges {
                privileges,
                objects,
                grantees,
            })
        }
    }
}

/// The object CLASS a `GRANT`/`REVOKE` names, as PostgreSQL's grammar spells it, and what
/// this server holds of it.
///
/// Source: the object types `GRANT` takes in the PostgreSQL documentation. Three of them
/// this server keeps a catalog of ([`ObjectClass`]); the rest name objects it holds NONE
/// of — there is no sequence, function or type on this endpoint — so a grant on one names
/// something that does not exist, and is refused with that class's own sentence rather than
/// being read as a table name or answered with a success that granted nothing.
struct ObjectKeyword {
    /// The keyword the statement names the class with.
    word: &'static str,
    /// The catalog this server keeps for it, or `None` for a class it holds no object of.
    held: Option<ObjectClass>,
    /// What PostgreSQL calls such an object where it does not exist, and the SQLSTATE that
    /// condition carries.
    absent: (&'static str, &'static str),
    /// Whether that sentence quotes the name: PostgreSQL writes `relation "x" does not
    /// exist` but `function x does not exist`.
    quoted: bool,
}

const OBJECT_KEYWORDS: &[ObjectKeyword] = &[
    ObjectKeyword {
        word: "TABLE",
        held: Some(ObjectClass::Relation),
        absent: ("relation", "42P01"),
        quoted: true,
    },
    ObjectKeyword {
        word: "DATABASE",
        held: Some(ObjectClass::Database),
        absent: ("database", "3D000"),
        quoted: true,
    },
    ObjectKeyword {
        word: "SCHEMA",
        held: Some(ObjectClass::Schema),
        absent: ("schema", "3F000"),
        quoted: true,
    },
    // A sequence is a relation of its own kind, and this server holds none.
    ObjectKeyword {
        word: "SEQUENCE",
        held: None,
        absent: ("relation", "42P01"),
        quoted: true,
    },
    ObjectKeyword {
        word: "FUNCTION",
        held: None,
        absent: ("function", "42883"),
        quoted: false,
    },
    ObjectKeyword {
        word: "PROCEDURE",
        held: None,
        absent: ("procedure", "42883"),
        quoted: false,
    },
    ObjectKeyword {
        word: "ROUTINE",
        held: None,
        absent: ("routine", "42883"),
        quoted: false,
    },
    ObjectKeyword {
        word: "TYPE",
        held: None,
        absent: ("type", "42704"),
        quoted: true,
    },
    ObjectKeyword {
        word: "DOMAIN",
        held: None,
        absent: ("type", "42704"),
        quoted: true,
    },
    ObjectKeyword {
        word: "LANGUAGE",
        held: None,
        absent: ("language", "42704"),
        quoted: true,
    },
    ObjectKeyword {
        word: "TABLESPACE",
        held: None,
        absent: ("tablespace", "42704"),
        quoted: true,
    },
];

/// The class the keyword `word` names, for a word that names one.
fn object_keyword(word: &str) -> Option<&'static ObjectKeyword> {
    OBJECT_KEYWORDS
        .iter()
        .find(|keyword| keyword.word.eq_ignore_ascii_case(word))
}

/// What a grant on an object this server holds none of is refused with: that class's own
/// "does not exist", naming the object the statement named.
fn object_absent(keyword: &ObjectKeyword, name: &str) -> EngineError {
    let (noun, sqlstate) = keyword.absent;
    let named = if keyword.quoted {
        format!("\"{name}\"")
    } else {
        name.to_string()
    };
    EngineError::new(
        EngineErrorKind::UndefinedObject,
        format!("{noun} {named} does not exist"),
    )
    .with_sqlstate(sqlstate)
}

/// The list the words `named` state, as privilege letters: empty for `ALL [PRIVILEGES]`,
/// which stands for every privilege the object class admits.
///
/// A word that names no privilege at all is where the reading stops, and PostgreSQL's parse
/// error names that token.
fn named_privileges(named: &[String]) -> Result<Vec<char>, Unreadable> {
    if named.first().is_some_and(|word| word.as_str() == "ALL") {
        return Ok(Vec::new());
    }
    let mut letters = Vec::new();
    for word in named {
        // A privilege may name the columns it is granted on; the privilege itself is what
        // decides the statement's form, and the column list is read as part of the object.
        if matches!(word.as_str(), "," | "(" | ")") {
            continue;
        }
        letters.push(
            privilege(word)
                .ok_or_else(|| Unreadable::At(word.clone()))?
                .letter,
        );
    }
    non_empty_letters(letters).ok_or(Unreadable::EndOfInput)
}

/// `Some(letters)` for a list with something in it.
fn non_empty_letters(letters: Vec<char>) -> Option<Vec<char>> {
    (!letters.is_empty()).then_some(letters)
}

/// What one `GRANT`/`REVOKE` names on the object side.
enum Objects {
    /// Objects of a class this server keeps a catalog of, by name.
    Held {
        class: ObjectClass,
        names: Vec<String>,
    },
    /// Every object of one CLASS in one schema (`ALL TABLES IN SCHEMA <schema>`): the
    /// schema, and the catalog to expand it against — `None` for a class this server holds
    /// no object of, which is an empty set in an existing schema.
    EveryOne {
        class: Option<ObjectClass>,
        schema: String,
    },
    /// Objects of a class this server holds NONE of, which is what the statement is refused
    /// with.
    Absent {
        keyword: &'static ObjectKeyword,
        names: Vec<String>,
    },
}

/// The objects the statement names: `[TABLE] <names>`, `<class> <names>`, or
/// `ALL <class>S IN SCHEMA <schema>`.
fn object_list(tokens: &[Token]) -> Result<Objects, Unreadable> {
    let leading = tokens
        .iter()
        .find_map(|token| match token {
            Token::Word(word) if word.quote_style.is_none() => {
                Some(word.value.to_ascii_uppercase())
            }
            _ => None,
        })
        .ok_or(Unreadable::EndOfInput)?;
    // `ALL <class>S IN SCHEMA <schema>` names every object of one class in one schema.
    if leading == "ALL" {
        return every_object_in_schema(tokens);
    }
    let keyword = object_keyword(&leading);
    let names = identifiers(tokens);
    // The class keyword is not one of the names; a statement that names none spells its
    // objects straight after `ON`, which is a TABLE list.
    let named: Vec<String> = names
        .into_iter()
        .skip(usize::from(keyword.is_some()))
        .collect();
    let named = non_empty(named).ok_or(Unreadable::EndOfInput)?;
    Ok(match keyword {
        Some(keyword) => match keyword.held {
            Some(class) => Objects::Held { class, names: named },
            None => Objects::Absent {
                keyword,
                names: named,
            },
        },
        None => Objects::Held {
            class: ObjectClass::Relation,
            names: named,
        },
    })
}

/// `ALL <class>S IN SCHEMA <schema>`: the class whose objects the grant covers, and the
/// schema it covers them in.
///
/// The plural class keyword is read as the class it names (`TABLES` -> a table), so a class
/// this server keeps no catalog of is an EMPTY set of objects in that schema rather than a
/// schema name read out of the keyword.
fn every_object_in_schema(tokens: &[Token]) -> Result<Objects, Unreadable> {
    let mut words = tokens.iter().filter_map(|token| match token {
        Token::Word(word) => Some((word.value.to_ascii_uppercase(), word.value.clone())),
        _ => None,
    });
    let _all = words.next().ok_or(Unreadable::EndOfInput)?;
    let (plural, _) = words.next().ok_or(Unreadable::EndOfInput)?;
    // `TABLES` names a table, `SEQUENCES` a sequence: the class is the singular.
    let singular = plural.strip_suffix('S').unwrap_or(&plural);
    let keyword = object_keyword(singular).ok_or(Unreadable::At(plural.clone()))?;
    // `IN SCHEMA <schema>` is the only form this list takes.
    for expected in ["IN", "SCHEMA"] {
        let (word, _) = words.next().ok_or(Unreadable::EndOfInput)?;
        if word != expected {
            return Err(Unreadable::At(word));
        }
    }
    let (upper, written) = words.next().ok_or(Unreadable::EndOfInput)?;
    let schema = if tokens.iter().any(|token| {
        matches!(token, Token::Word(word)
            if word.quote_style.is_some() && word.value == written)
    }) {
        written
    } else {
        upper.to_ascii_lowercase()
    };
    Ok(Objects::EveryOne {
        class: keyword.held,
        schema,
    })
}

/// The identifiers a token run names, comma-separated: an unquoted one folded to lower
/// case and a quoted one kept as written, exactly as PostgreSQL folds an identifier, and a
/// qualified name reduced to the object it names.
///
/// A name the run repeats is one name: a statement naming the same role or object twice
/// grants the same thing twice, so the list a grant works over carries each name once —
/// which is also what keeps the work it drives bounded by the names it actually touches.
fn identifiers(tokens: &[Token]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut after_period = false;
    for token in tokens {
        match token {
            Token::Period => {
                after_period = true;
                continue;
            }
            Token::Word(word) => {
                let name = match word.quote_style {
                    Some(_) => word.value.clone(),
                    None => word.value.to_ascii_lowercase(),
                };
                // `public.widgets` names the table, and the qualifier is the schema this
                // server's objects live in anyway.
                if after_period {
                    if let Some(last) = names.last_mut() {
                        *last = name;
                    }
                } else {
                    names.push(name);
                }
            }
            Token::SingleQuotedString(value) => names.push(value.clone()),
            _ => {}
        }
        after_period = false;
    }
    let mut once: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        if !once.contains(&name) {
            once.push(name);
        }
    }
    once
}

/// `Some(list)` for a list with something in it.
fn non_empty(list: Vec<String>) -> Option<Vec<String>> {
    (!list.is_empty()).then_some(list)
}

/// Serve the role-membership form: every role named must exist, and the membership rows
/// are written to (or removed from) `pg_auth_members`.
fn role_membership(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    verb: Verb,
    roles: &[String],
    members: &[String],
) -> Result<StatementAnswer, EngineError> {
    // The work below is one engine write per (role, member) PAIR, so the two lists are
    // bounded before any of it is done.
    bounded_lists(roles, members)?;
    let grantor = role_oid(database, facts, &facts.user)?;
    for role in roles.iter().chain(members) {
        if role == PUBLIC_ROLE {
            // `PUBLIC` is not a role a membership can name: it is not a row in `pg_roles`,
            // and PostgreSQL refuses a membership that names it.
            return Err(EngineError::new(
                EngineErrorKind::Syntax,
                "role \"public\" is reserved".to_string(),
            )
            .with_sqlstate("42939"));
        }
    }
    let role_oids = resolve_roles(database, facts, roles)?;
    let member_oids = resolve_roles(database, facts, members)?;
    for role in &role_oids {
        for member in &member_oids {
            match verb {
                Verb::Grant => {
                    database.execute_in(
                        facts.connection_id,
                        &facts.database,
                        "INSERT OR REPLACE INTO pg_auth_members \
                           (oid, roleid, member, grantor, admin_option) \
                         VALUES ((SELECT IFNULL(MAX(oid), 0) + 1 FROM pg_auth_members), \
                                 ?, ?, ?, 0)",
                        &[
                            Some(role.to_string()),
                            Some(member.to_string()),
                            grantor.map(|oid| oid.to_string()),
                        ],
                    )?;
                }
                Verb::Revoke => {
                    database.execute_in(
                        facts.connection_id,
                        &facts.database,
                        "DELETE FROM pg_auth_members WHERE roleid = ? AND member = ?",
                        &[Some(role.to_string()), Some(member.to_string())],
                    )?;
                }
            }
        }
    }
    Ok(StatementAnswer::new(verb.tag(), ResultSet::changed(0, 0)))
}

/// The object identifiers of `roles`, refusing the first one the instance does not have —
/// which is PostgreSQL's own answer for a role that is not there.
fn resolve_roles(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    roles: &[String],
) -> Result<Vec<i64>, EngineError> {
    let mut oids = Vec::with_capacity(roles.len());
    for role in roles {
        match role_oid(database, facts, role)? {
            Some(oid) => oids.push(oid),
            None => {
                return Err(EngineError::new(
                    EngineErrorKind::UndefinedObject,
                    format!("role \"{role}\" does not exist"),
                )
                .with_sqlstate("42704"))
            }
        }
    }
    Ok(oids)
}

/// The object identifier of the role `name`, or `None` for a role this instance does not
/// have.
fn role_oid(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    name: &str,
) -> Result<Option<i64>, EngineError> {
    let found = database.execute_in(
        facts.connection_id,
        &facts.database,
        "SELECT oid FROM pg_roles WHERE rolname = ?",
        &[Some(name.to_string())],
    )?;
    Ok(found
        .rows
        .first()
        .and_then(|row| row.first())
        .and_then(|cell| match cell {
            Cell::Integer(oid) => Some(*oid),
            _ => None,
        }))
}

/// Whether the database holds the schema `name`, as `pg_namespace` reports it.
fn schema_exists(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    name: &str,
) -> Result<bool, EngineError> {
    let found = database.execute_in(
        facts.connection_id,
        &facts.database,
        "SELECT 1 FROM pg_namespace WHERE nspname = ?",
        &[Some(name.to_string())],
    )?;
    Ok(!found.rows.is_empty())
}

/// The predefined role an RDS for PostgreSQL database user is made a member of to
/// authenticate with an IAM authentication token instead of a password.
pub const IAM_AUTHENTICATION_ROLE: &str = "rds_iam";

/// Whether the role `member` is a member of the role `group` in the database `db_name`, as
/// `pg_auth_members` holds it.
pub fn is_member_of(
    database: &InstanceDatabase,
    db_name: &str,
    member: &str,
    group: &str,
) -> Result<bool, EngineError> {
    let found = database.execute(
        db_name,
        "SELECT 1 FROM pg_auth_members AS m \
           JOIN pg_roles AS r ON r.oid = m.roleid \
           JOIN pg_roles AS g ON g.oid = m.member \
         WHERE r.rolname = ? AND g.rolname = ? AND g.rolcanlogin",
        &[Some(group.to_string()), Some(member.to_string())],
    )?;
    Ok(!found.rows.is_empty())
}

/// Whether the database user `user` of `db_name` is one enabled for IAM database
/// authentication: a role that may log in and is a member of
/// [`IAM_AUTHENTICATION_ROLE`] — the engine-side half of
/// `EnableIAMDatabaseAuthentication`.
///
/// A READ, and nothing else: the catalogs are not created here, so a login decided before
/// any session has run on this database leaves nothing behind. A database whose role
/// catalog does not exist yet holds no role and no membership, so no user of it is enabled
/// for a token — which is what the unreadable case answers, since a login is admitted on
/// what the catalogs SAY and never on a question that could not be asked.
#[must_use]
pub fn user_authenticates_with_a_token(
    database: &InstanceDatabase,
    db_name: &str,
    user: &str,
) -> bool {
    is_member_of(database, db_name, user, IAM_AUTHENTICATION_ROLE).unwrap_or(false)
}

/// Serve the object-privilege form: the privileges are merged into (or removed from) the
/// access control list of each object's catalog row.
fn object_privileges(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    verb: Verb,
    privileges: &[char],
    objects: &Objects,
    grantees: &[String],
) -> Result<StatementAnswer, EngineError> {
    // An object class this server holds NO object of: every name the statement gives is a
    // name that does not exist, which is that class's own condition.
    let (class, named) = match objects {
        Objects::Absent { keyword, names } => {
            return Err(object_absent(
                keyword,
                names.first().map(String::as_str).unwrap_or_default(),
            ))
        }
        Objects::Held { class, names } => (*class, names.clone()),
        Objects::EveryOne { class, schema } => {
            // `ALL <class>S IN SCHEMA <schema>`: the schema has to be one the database has
            // — PostgreSQL answers `schema "x" does not exist` for one it does not — and
            // the objects are then the ones that schema holds of that class, read from the
            // catalog. A class this server keeps no catalog of holds none of them, which is
            // an empty set in an existing schema: the grant covers nothing, exactly as it
            // covers nothing in a schema holding no object of that class.
            if !schema_exists(database, facts, schema)? {
                return Err(EngineError::new(
                    EngineErrorKind::UndefinedObject,
                    format!("schema \"{schema}\" does not exist"),
                )
                .with_sqlstate("3F000"));
            }
            match class {
                Some(class) => (*class, tables_in_schema(database, facts, schema)?),
                None => return Ok(StatementAnswer::new(verb.tag(), ResultSet::changed(0, 0))),
            }
        }
    };
    // A privilege the object class does not admit: PostgreSQL names the privilege and the
    // kind of object it was asked for.
    let admitted = all_privileges(class);
    if let Some(wrong) = privileges.iter().find(|letter| !admitted.contains(letter)) {
        let named = PRIVILEGES
            .iter()
            .find(|privilege| privilege.letter == *wrong)
            .map_or("", |privilege| privilege.name);
        return Err(EngineError::new(
            EngineErrorKind::Syntax,
            format!(
                "invalid privilege type {named} for {}",
                class.privilege_noun()
            ),
        )
        .with_sqlstate("0LP01"));
    }
    let letters = if privileges.is_empty() {
        admitted
    } else {
        privileges.to_vec()
    };
    let grantor = facts.user.clone();
    let (catalog, name_column, acl_column) = class.catalog();
    // The work below is a lookup and a write per (object, grantee) PAIR — and the object
    // list is the expanded one, so the bound is taken after the expansion.
    bounded_lists(&named, grantees)?;
    for object in &named {
        let Some(acl) = object_acl(database, facts, class, object)? else {
            return Err(EngineError::new(
                EngineErrorKind::UndefinedObject,
                format!("{} \"{object}\" does not exist", class.noun()),
            )
            .with_sqlstate(match class {
                ObjectClass::Relation => "42P01",
                ObjectClass::Schema => "3F000",
                ObjectClass::Database => "3D000",
            }));
        };
        let mut acl = AccessList::read(acl.as_deref());
        for grantee in grantees {
            let grantee = if grantee == PUBLIC_ROLE {
                String::new()
            } else {
                // Every grantee must be a role the instance has.
                resolve_roles(database, facts, std::slice::from_ref(grantee))?;
                grantee.clone()
            };
            match verb {
                Verb::Grant => acl.grant(&grantee, &grantor, &letters),
                Verb::Revoke => acl.revoke(&grantee, &letters),
            }
        }
        database.execute_in(
            facts.connection_id,
            &facts.database,
            &format!("UPDATE {catalog} SET {acl_column} = ? WHERE {name_column} = ?"),
            &[acl.write(), Some(object.clone())],
        )?;
    }
    Ok(StatementAnswer::new(verb.tag(), ResultSet::changed(0, 0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ALL PRIVILEGES` stands for every privilege the object class admits, and no
    /// privilege of another class.
    #[test]
    fn all_privileges_are_the_ones_the_class_admits() {
        for (case, class, expected) in [
            ("a table", ObjectClass::Relation, "arwdDxt"),
            ("a schema", ObjectClass::Schema, "UC"),
            ("a database", ObjectClass::Database, "CTc"),
        ] {
            let letters: String = all_privileges(class).into_iter().collect();
            assert_eq!(
                letters, expected,
                "case {case:?}: expected {expected:?}, got {letters:?}"
            );
        }
    }
}
