// SPDX-License-Identifier: CC0-1.0

//! What ONE submitted Athena statement IS: the class it belongs to, and the plan for
//! running it.
//!
//! Athena is a query service in front of a DATA CATALOG: a statement either defines,
//! drops or reports a catalog object (`CREATE DATABASE`, `DROP DATABASE`,
//! `CREATE EXTERNAL TABLE`, `DROP TABLE`, `CREATE VIEW`, `DROP VIEW`, `SHOW TABLES`,
//! `SHOW DATABASES`), or it reads data through relations the catalog resolves
//! (`SELECT … FROM <database>.<table>`). Which of those a statement is, and the names it
//! addresses, are facts about its GRAMMAR, so they are read off a PARSE by the
//! maintained SQL parser this crate already depends on — never off the statement's
//! leading word or a match on its text.
//!
//! The parse is also where the POSITION of a name comes from: a catalog refusal names
//! the line and column the name it could not resolve appears at, and those are the
//! parser's own, taken from the submitted text.

use ::std::collections::HashSet;
use ::std::ops::ControlFlow;

use ::service_com_amazonaws_glue_awsglue::state as glue_state;
use ::sqlparser::ast::{
    ColumnDef, ColumnOption, CreateTable, CreateTableOptions, Expr, HiveDelimiter, HiveFormat,
    HiveIOFormat, HiveRowFormat, Ident, ObjectName, ObjectNamePart, ObjectType,
    ShowCreateObject, ShowStatementOptions, SqlOption, Statement as SqlStatement, Value, Visit,
    Visitor,
};
use ::sqlparser::dialect::{GenericDialect, HiveDialect};
use ::sqlparser::parser::Parser;

use super::query_engine::{STATEMENT_TYPE_DDL, STATEMENT_TYPE_DML, STATEMENT_TYPE_UTILITY};

/// How deep a submitted statement may nest before the parser gives up.
///
/// The EMULATOR's own bound: the parse and every walk over the tree it builds are
/// recursive, so a caller-supplied statement must not be able to recurse them off the
/// stack. A statement deeper than this does not parse, and is handed to the engine as
/// text exactly as any other unparsable statement is.
pub(super) const MAX_PARSER_RECURSION: usize = 48;

/// How many tokens a submitted statement may carry before this emulator stops reading it
/// as one of its own statements.
///
/// The EMULATOR's own bound, and the one that caps the DEPTH of the tree a parse builds.
/// [`MAX_PARSER_RECURSION`] counts the parser's own call depth, which a construct the
/// parser reads in a LOOP does not grow — a left-associative operator chain is read at
/// constant parser depth and still builds a tree one node deep per operator — while every
/// walk over the tree here (the relation collection, the rendering, and the tree's own
/// drop) recurses with its depth. A node of that tree costs at least one token, so
/// bounding the TOKENS bounds the depth, and tokenizing is a flat pass that can be done
/// before anything recursive runs. A statement past this bound is not read here: it goes
/// to the engine as text, under the engine's own expression-depth limit.
pub(super) const MAX_STATEMENT_TOKENS: usize = 2_048;

/// How many relations one statement may name. Each one is resolved against the catalog
/// and may be materialised, so the work one statement can ask for is bounded.
const MAX_RELATIONS: usize = 64;

/// The `SubstatementType` of a statement that defines a database.
pub const SUBSTATEMENT_CREATE_DATABASE: &str = "CREATE_DATABASE";
/// The `SubstatementType` of a statement that drops a database.
pub const SUBSTATEMENT_DROP_DATABASE: &str = "DROP_DATABASE";
/// The `SubstatementType` of a statement that projects rows.
pub const SUBSTATEMENT_SELECT: &str = "SELECT";
/// The `SubstatementType` of a statement that writes rows into an existing table.
pub const SUBSTATEMENT_INSERT_INTO: &str = "INSERT_INTO";
/// The `SubstatementType` of a statement that defines a table.
pub const SUBSTATEMENT_CREATE_TABLE: &str = "CREATE_TABLE";
/// The `SubstatementType` of a statement that defines a table from the rows a query
/// produced.
pub const SUBSTATEMENT_CREATE_TABLE_AS_SELECT: &str = "CREATE_TABLE_AS_SELECT";
/// The `SubstatementType` of a statement that defines a view.
pub const SUBSTATEMENT_CREATE_VIEW: &str = "CREATE_VIEW";
/// The `SubstatementType` of a statement that drops a table.
pub const SUBSTATEMENT_DROP_TABLE: &str = "DROP_TABLE";
/// The `SubstatementType` of a statement that drops a view.
pub const SUBSTATEMENT_DROP_VIEW: &str = "DROP_VIEW";

/// The database an `information_schema` relation addresses: the schema every SQL engine
/// reports its own catalog through, which Athena serves out of the Data Catalog.
pub const INFORMATION_SCHEMA: &str = "information_schema";

/// Where in the submitted text a name appears, as a catalog refusal reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// The 1-based line the name starts on.
    pub line: u64,
    /// The 1-based column the name starts at.
    pub column: u64,
}

impl Position {
    /// The start of the submitted text, for a name the parser recorded no span for.
    pub const START: Self = Self { line: 1, column: 1 };

    /// Where `ident` appears in the submitted text.
    fn of(ident: &Ident) -> Self {
        let start = ident.span.start;
        if start.line == 0 {
            return Self::START;
        }
        Self {
            line: start.line,
            column: start.column,
        }
    }
}

/// The catalog and database an unqualified name resolves against: the
/// `QueryExecutionContext` the request carried, with Athena's own defaults applied.
#[derive(Debug, Clone)]
pub struct QueryContext {
    /// The data catalog the query addresses.
    pub catalog: String,
    /// The database the query addresses.
    pub database: String,
}

/// One relation a statement names, resolved to the catalog, database and table it
/// addresses.
///
/// The database and table are FOLDED the way the Data Catalog folds a name, because
/// that is the name the catalog is keyed by and Athena's query layer resolves an
/// identifier case-insensitively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    /// The data catalog this relation lives in.
    pub catalog: String,
    /// The database, folded.
    pub database: String,
    /// The table or view, folded.
    pub table: String,
    /// The name as the statement wrote it, which is the key a rewrite replaces.
    pub written: String,
    /// Where the name appears in the submitted text.
    pub at: Position,
}

impl Relation {
    /// The name this relation's rows are reachable under inside one engine database.
    ///
    /// A catalog relation is materialised under its qualified name, so two tables of the
    /// same name in different databases never collide and nothing a statement defined in
    /// the engine itself is shadowed.
    pub fn local_name(&self) -> String {
        format!("{}.{}", self.database, self.table)
    }

    /// Whether this relation addresses the catalog's own reporting schema.
    pub fn is_information_schema(&self) -> bool {
        self.database == INFORMATION_SCHEMA
    }
}

/// A database a statement addresses without naming a relation in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    /// The data catalog the database lives in.
    pub catalog: String,
    /// The database, folded.
    pub database: String,
    /// Where the name appears in the submitted text.
    pub at: Position,
}

/// What reading the submitted SQL established.
#[derive(Debug)]
pub struct Reading {
    /// How many statements the text parses as, or `None` when it does not parse at all —
    /// in which case the engine is the one that reports why.
    pub statements: Option<usize>,
    /// The `StatementType` of the single statement submitted, when there is one.
    pub statement_type: Option<&'static str>,
    /// The `SubstatementType` — "the kind of query statement that was run" — of the
    /// single statement submitted, for the kinds Athena names.
    pub substatement_type: Option<&'static str>,
    /// Whether this statement is one whose written rows the engine counts: an `INSERT`,
    /// `UPDATE`, `DELETE` or `MERGE`.
    pub counts_rows: bool,
    /// How to run it.
    pub plan: Plan,
}

impl Reading {
    /// Why this submission is refused AT SUBMISSION rather than run, if it is.
    ///
    /// Athena judges what a statement DECLARES when the statement is submitted, not when
    /// it runs: a table over data that already exists has to say WHERE that data is, so a
    /// definition that names no location names no table Athena could ever read, and the
    /// submission itself is refused.
    pub fn declaration_refusal(&self) -> Option<&'static str> {
        let Plan::Catalog(statement) = &self.plan else {
            return None;
        };
        match statement.as_ref() {
            CatalogStatement::CreateExternalTable(table)
                if table
                    .location
                    .as_deref()
                    .map(str::trim)
                    .is_none_or(str::is_empty) =>
            {
                Some(NO_TABLE_LOCATION)
            }
            _ => None,
        }
    }
}

/// The message a table definition that names no location is refused with. Wholly fixed
/// text: it states the rule, and interpolates nothing from the statement.
pub const NO_TABLE_LOCATION: &str =
    "No location was specified for table. An S3 location must be specified";

/// How one statement runs.
#[derive(Debug)]
pub enum Plan {
    /// Text no parser could read, or a statement that neither the catalog owns nor names
    /// a relation: the engine runs the submitted text as it stands.
    Engine,
    /// A statement the Data Catalog itself performs.
    Catalog(Box<CatalogStatement>),
    /// A statement the engine runs over relations the catalog resolves.
    Relations(Box<RelationalStatement>),
}

/// A statement the engine runs once its relations are resolved.
#[derive(Debug)]
pub struct RelationalStatement {
    /// The parsed statement, so each resolved relation can be rewritten to the name its
    /// rows are materialised under.
    pub statement: SqlStatement,
    /// The relations it reads, in first-appearance order.
    pub relations: Vec<Relation>,
}

/// A statement whose whole effect is on the Data Catalog.
#[derive(Debug)]
pub enum CatalogStatement {
    /// `CREATE DATABASE` / `CREATE SCHEMA`: registers a database — the container the
    /// catalog holds tables in.
    CreateDatabase(DatabaseStatement),
    /// `DROP DATABASE` / `DROP SCHEMA`.
    DropDatabase {
        /// The database to drop.
        target: Schema,
        /// Whether the statement tolerates its absence.
        if_exists: bool,
        /// Whether the statement drops the tables the database still holds with it,
        /// which is what `CASCADE` asks for.
        cascade: bool,
    },
    /// `SHOW DATABASES` and `SHOW SCHEMAS`.
    ShowDatabases {
        /// The data catalog to list the databases of.
        catalog: String,
        /// The pattern the statement narrows its answer by, if it gave one.
        filter: Option<RelationFilter>,
    },
    /// `CREATE EXTERNAL TABLE`: registers a table over data that already exists.
    CreateExternalTable(ExternalTable),
    /// `DROP TABLE`.
    DropTable {
        /// The table to drop.
        target: Relation,
        /// Whether the statement tolerates its absence.
        if_exists: bool,
    },
    /// `CREATE [OR REPLACE] VIEW`: registers a view over a query.
    CreateView(ViewStatement),
    /// `DROP VIEW`.
    DropView {
        /// The view to drop.
        target: Relation,
        /// Whether the statement tolerates its absence.
        if_exists: bool,
    },
    /// `SHOW CREATE VIEW`.
    ShowCreateView {
        /// The view to report.
        target: Relation,
    },
    /// `SHOW TABLES` and `SHOW VIEWS`.
    ShowRelations {
        /// The database to list.
        schema: Schema,
        /// Whether only views are listed, which is what tells `SHOW VIEWS` from
        /// `SHOW TABLES`.
        views_only: bool,
        /// The pattern the statement narrows its answer by, if it gave one.
        filter: Option<RelationFilter>,
    },
}

/// The pattern a `SHOW TABLES` or `SHOW VIEWS` statement narrows its answer by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationFilter {
    /// A bare pattern, as Hive's own `SHOW TABLES '<pattern>'` takes one: the pattern
    /// language the Data Catalog's listings match a name by.
    Pattern(String),
    /// `LIKE '<pattern>'`: the SQL pattern language, where `%` stands for any run of
    /// characters and `_` for one.
    Like {
        /// The pattern itself.
        pattern: String,
        /// Whether it is matched without regard to case, which is what `ILIKE` asks for.
        case_insensitive: bool,
    },
    /// A form that narrows by something other than a pattern, carrying it as written.
    Other(String),
}

/// A `CREATE DATABASE` or `CREATE SCHEMA` statement, as the catalog entry it defines.
///
/// A database and a schema are the SAME catalog object to Athena — its catalogs hold
/// databases, and `SCHEMA` is the word the Presto-derived grammar uses for one — so both
/// spellings read as this statement.
#[derive(Debug, Clone)]
pub struct DatabaseStatement {
    /// The database it defines.
    pub target: Schema,
    /// Whether an existing database of that name leaves the statement successful.
    pub if_not_exists: bool,
    /// `LOCATION`, as submitted: where the data of the tables in it lives by default.
    pub location: Option<String>,
    /// The database `COMMENT`, which the catalog holds as its description.
    pub comment: Option<String>,
    /// The `DBPROPERTIES` / `WITH` entries, which the catalog holds as its parameters.
    pub properties: Vec<(String, String)>,
}

/// A `CREATE EXTERNAL TABLE` statement, as the catalog entry it defines.
#[derive(Debug, Clone)]
pub struct ExternalTable {
    /// The table it defines.
    pub target: Relation,
    /// Whether an existing entry of that name leaves the statement successful.
    pub if_not_exists: bool,
    /// The declared columns, each as its name and the Hive type it was declared with.
    pub columns: Vec<DeclaredColumn>,
    /// The declared partition columns.
    pub partition_columns: Vec<DeclaredColumn>,
    /// `LOCATION`, as submitted.
    pub location: Option<String>,
    /// The serialization library `ROW FORMAT SERDE` named, if any.
    pub serde_library: Option<String>,
    /// The SerDe parameters the statement declared: the delimiters of
    /// `ROW FORMAT DELIMITED` and every `WITH SERDEPROPERTIES` entry.
    pub serde_parameters: Vec<(String, String)>,
    /// The `TBLPROPERTIES` entries.
    pub table_properties: Vec<(String, String)>,
    /// The `STORED AS` storage format the statement declared, which decides the input
    /// and output formats the catalog records and the SerDe a table with no
    /// `ROW FORMAT SERDE` of its own is read with.
    pub storage: Option<StorageFormat>,
    /// The table `COMMENT`, which the catalog holds as the table's description.
    pub comment: Option<String>,
}

/// The storage format a `STORED AS` clause declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageFormat {
    /// `STORED AS <format>`: one of the file formats the grammar names.
    Named(String),
    /// `STORED AS INPUTFORMAT '<in>' OUTPUTFORMAT '<out>'`: the two Hive formats
    /// themselves, as the statement named them.
    Formats {
        /// The input format the data is read with.
        input: String,
        /// The output format it is written with.
        output: String,
    },
}

/// One column a statement declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredColumn {
    /// The column name, folded the way the catalog folds a column name.
    pub name: String,
    /// The Hive type it was declared with, lowercased as the catalog records it.
    pub data_type: String,
    /// Its `COMMENT`, if the statement gave one.
    pub comment: Option<String>,
}

/// A `CREATE [OR REPLACE] VIEW` statement.
#[derive(Debug, Clone)]
pub struct ViewStatement {
    /// The view it defines.
    pub target: Relation,
    /// Whether an existing view of that name is replaced rather than refused.
    pub or_replace: bool,
    /// The view's query, as the SQL text it was submitted as.
    pub query: String,
    /// The relations that query reads, which must exist for the view to be created.
    pub sources: Vec<Relation>,
    /// The columns that query PROJECTS, which are the view's own columns.
    pub projection: Vec<Projection>,
    /// The column names the statement declared for the view, if it declared any.
    pub columns: Vec<String>,
}

/// One column a view's query projects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Projection {
    /// `*`: every column the query's sources hold, in their own order.
    All,
    /// One named column.
    Column {
        /// The name the column is projected under.
        name: String,
        /// The source column it reads, when the projection is a plain reference to one
        /// rather than an expression over several.
        source: Option<String>,
    },
}

/// Read `sql` as the one statement it carries, and plan how to run it.
pub fn read(sql: &str, context: &QueryContext) -> Reading {
    let Some(mut statements) = parse_statements(sql) else {
        // A database definition carrying the clauses Hive's DDL grammar takes and the
        // general parser does not read is still that statement, so it is read as one
        // rather than handed to the engine as text (see [`super::hive_database`]).
        if let Some(definition) = super::hive_database::read_database_definition(sql) {
            return Reading {
                statements: Some(1),
                statement_type: Some(STATEMENT_TYPE_DDL),
                substatement_type: Some(SUBSTATEMENT_CREATE_DATABASE),
                counts_rows: false,
                plan: Plan::Catalog(Box::new(CatalogStatement::CreateDatabase(
                    DatabaseStatement {
                        target: Schema {
                            catalog: context.catalog.clone(),
                            database: glue_state::fold_name(&definition.name),
                            at: definition.at,
                        },
                        if_not_exists: definition.if_not_exists,
                        location: definition.location,
                        comment: definition.comment,
                        properties: definition.properties,
                    },
                ))),
            };
        }
        return Reading {
            statements: None,
            statement_type: None,
            substatement_type: None,
            counts_rows: false,
            plan: Plan::Engine,
        };
    };
    let count = statements.len();
    if count != 1 {
        return Reading {
            statements: Some(count),
            statement_type: None,
            substatement_type: None,
            counts_rows: false,
            plan: Plan::Engine,
        };
    }
    let statement = statements.remove(0);
    let statement_type = statement_type_of(&statement);
    let substatement_type = substatement_type_of(&statement);
    let counts_rows = counts_rows(&statement);
    Reading {
        statements: Some(1),
        statement_type: Some(statement_type),
        substatement_type,
        counts_rows,
        plan: plan_of(statement, context),
    }
}

/// The statements `sql` parses as, or `None` when no dialect this crate reads can parse
/// it.
///
/// Athena takes statements of TWO grammars — its Presto-derived query language, and the
/// Hive DDL its `CREATE EXTERNAL TABLE` belongs to — and a statement of either is read
/// by the dialect of the parser that implements it, in that order, so a statement is
/// understood whichever of the two it is written in.
pub fn parse_statements(sql: &str) -> Option<Vec<SqlStatement>> {
    // The TOKENS are counted first, on a flat pass, so the bound on the tree's depth is
    // established before anything recursive runs over it.
    if token_count(sql)? > MAX_STATEMENT_TOKENS {
        return None;
    }
    let generic = GenericDialect {};
    let hive = HiveDialect {};
    Parser::new(&generic)
        .with_recursion_limit(MAX_PARSER_RECURSION)
        .try_with_sql(sql)
        .and_then(|mut parser| parser.parse_statements())
        .or_else(|_| {
            Parser::new(&hive)
                .with_recursion_limit(MAX_PARSER_RECURSION)
                .try_with_sql(sql)
                .and_then(|mut parser| parser.parse_statements())
        })
        .ok()
}

/// How many tokens, other than whitespace, a submitted statement carries, or `None` for
/// text the tokenizer cannot read at all — which is no statement of this grammar.
///
/// Lexing is a FLAT pass — the tokenizer recurses over nothing — which is what makes this
/// usable as the bound in front of the parse (see [`MAX_STATEMENT_TOKENS`]). Whitespace
/// and comments carry no node of the tree, so they are not counted.
fn token_count(sql: &str) -> Option<usize> {
    use ::sqlparser::tokenizer::{Token, Tokenizer};
    let tokens = Tokenizer::new(&GenericDialect {}, sql).tokenize().ok()?;
    Some(
        tokens
            .iter()
            .filter(|token| !matches!(token, Token::Whitespace(..)))
            .count(),
    )
}

/// The columns one query PROJECTS, in projection order.
///
/// A projection that is a plain column reference names the source column it reads, so a
/// view's own column takes that column's type; one that is an expression carries the name
/// it is projected under — its alias, or the positional name a query engine gives an
/// unnamed expression.
pub fn projection_of(query: &::sqlparser::ast::Query) -> Vec<Projection> {
    use ::sqlparser::ast::{Expr, SelectItem, SetExpr};
    let mut body = query.body.as_ref();
    // A set operation projects the columns of its first branch, which is the one that
    // names them.
    while let SetExpr::SetOperation { left, .. } = body {
        body = left.as_ref();
    }
    let SetExpr::Select(select) = body else {
        return vec![Projection::All];
    };
    select
        .projection
        .iter()
        .enumerate()
        .map(|(position, item)| match item {
            SelectItem::Wildcard(..) | SelectItem::QualifiedWildcard(..) => Projection::All,
            SelectItem::UnnamedExpr(Expr::Identifier(ident)) => Projection::Column {
                name: glue_state::fold_name(&ident.value),
                source: Some(glue_state::fold_name(&ident.value)),
            },
            SelectItem::UnnamedExpr(Expr::CompoundIdentifier(parts)) => {
                let named = parts
                    .last()
                    .map_or_else(String::new, |ident| glue_state::fold_name(&ident.value));
                Projection::Column {
                    name: named.clone(),
                    source: Some(named),
                }
            }
            SelectItem::UnnamedExpr(..) => Projection::Column {
                name: unnamed_column(position),
                source: None,
            },
            SelectItem::ExprWithAlias { expr, alias } => Projection::Column {
                name: glue_state::fold_name(&alias.value),
                source: match expr {
                    Expr::Identifier(ident) => Some(glue_state::fold_name(&ident.value)),
                    Expr::CompoundIdentifier(parts) => parts
                        .last()
                        .map(|ident| glue_state::fold_name(&ident.value)),
                    _ => None,
                },
            },
        })
        .collect()
}

/// The name a query engine gives the column of an unnamed expression: its position in the
/// projection, counted from zero.
fn unnamed_column(position: usize) -> String {
    format!("_col{position}")
}

/// One query as the engine RENDERS it back: the SQL of its own parse, in which every
/// predicate stands as the expression it is.
///
/// A view's definition is reported as the statement the engine would run, not as the text
/// the client typed, so the text comes from the PARSE: a predicate is parenthesised,
/// because that is the expression the engine holds, and the whole statement is rendered
/// from the tree rather than echoed.
pub fn formatted_query(sql: &str) -> String {
    let parsed = parse_statements(sql)
        .and_then(|mut statements| (statements.len() == 1).then(|| statements.remove(0)));
    let Some(mut parsed) = parsed else {
        return sql.to_string();
    };
    if let SqlStatement::Query(query) = &mut parsed {
        parenthesise(query);
    }
    parsed.to_string()
}

/// Parenthesise the predicates of one query, as the engine's own rendering of them.
fn parenthesise(query: &mut ::sqlparser::ast::Query) {
    use ::sqlparser::ast::SetExpr;
    let mut pending: Vec<&mut SetExpr> = vec![query.body.as_mut()];
    while let Some(body) = pending.pop() {
        match body {
            SetExpr::Select(select) => {
                select.selection = select.selection.take().map(nested);
                select.having = select.having.take().map(nested);
            }
            SetExpr::Query(nested) => pending.push(nested.body.as_mut()),
            SetExpr::SetOperation { left, right, .. } => {
                pending.push(left.as_mut());
                pending.push(right.as_mut());
            }
            _ => {}
        }
    }
}

/// One predicate as the engine renders it: an expression that is not already a
/// parenthesised one, parenthesised.
fn nested(predicate: ::sqlparser::ast::Expr) -> ::sqlparser::ast::Expr {
    use ::sqlparser::ast::Expr;
    if matches!(predicate, Expr::Nested(..)) {
        predicate
    } else {
        Expr::Nested(Box::new(predicate))
    }
}

/// Whether the engine counts the rows this statement writes.
///
/// A `CREATE TABLE AS SELECT` is a data-manipulating statement too, but the engine does
/// not count the rows it writes, so it is not one of these.
fn counts_rows(statement: &SqlStatement) -> bool {
    matches!(
        statement,
        SqlStatement::Insert(..)
            | SqlStatement::Update { .. }
            | SqlStatement::Delete(..)
            | SqlStatement::Merge { .. }
    )
}

/// The `StatementType` of one parsed statement.
///
/// Athena documents the three classes `StatementType` names: a statement that queries or
/// changes DATA is DML, one that DEFINES an object is DDL, and everything else — the
/// statements that ask the engine about itself or set up a session — is UTILITY.
fn statement_type_of(statement: &SqlStatement) -> &'static str {
    match statement {
        // `CREATE TABLE AS SELECT` defines a table AND writes the rows a query
        // produced, and Athena classifies it with the statements that manipulate data.
        SqlStatement::CreateTable(create) if create.query.is_some() => STATEMENT_TYPE_DML,
        SqlStatement::Query(..)
        | SqlStatement::Insert(..)
        | SqlStatement::Update { .. }
        | SqlStatement::Delete(..)
        | SqlStatement::Merge { .. } => STATEMENT_TYPE_DML,
        SqlStatement::CreateTable(..)
        | SqlStatement::CreateView { .. }
        | SqlStatement::CreateIndex(..)
        | SqlStatement::CreateSchema { .. }
        | SqlStatement::CreateDatabase { .. }
        | SqlStatement::AlterTable { .. }
        | SqlStatement::Drop { .. }
        | SqlStatement::Truncate { .. } => STATEMENT_TYPE_DDL,
        _ => STATEMENT_TYPE_UTILITY,
    }
}

/// The `SubstatementType` of one parsed statement.
///
/// `QueryExecution$SubstatementType` documents it as "The kind of query statement that
/// was run" — a finer classification than the three `StatementType` classes, naming the
/// statement's own kind in upper snake case. Read off the PARSE, like
/// [`statement_type_of`], and `None` for a kind that has no such name here: a statement
/// whose kind cannot be named reports none rather than a guess.
fn substatement_type_of(statement: &SqlStatement) -> Option<&'static str> {
    Some(match statement {
        // A `CREATE TABLE AS SELECT` is named after both halves of what it does; see
        // `statement_type_of`, which classifies it with the statements that manipulate
        // data for the same reason.
        SqlStatement::CreateTable(create) if create.query.is_some() => {
            SUBSTATEMENT_CREATE_TABLE_AS_SELECT
        }
        SqlStatement::CreateTable(..) => SUBSTATEMENT_CREATE_TABLE,
        SqlStatement::Query(..) => SUBSTATEMENT_SELECT,
        SqlStatement::Insert(..) => SUBSTATEMENT_INSERT_INTO,
        SqlStatement::Update { .. } => "UPDATE",
        SqlStatement::Delete(..) => "DELETE",
        SqlStatement::Merge { .. } => "MERGE",
        SqlStatement::CreateView { .. } => SUBSTATEMENT_CREATE_VIEW,
        // A schema and a database are the same object to Athena, whose catalogs hold
        // databases.
        SqlStatement::CreateDatabase { .. } | SqlStatement::CreateSchema { .. } => {
            SUBSTATEMENT_CREATE_DATABASE
        }
        SqlStatement::AlterTable { .. } => "ALTER_TABLE",
        SqlStatement::Truncate { .. } => "TRUNCATE_TABLE",
        SqlStatement::Drop { object_type, .. } => match object_type {
            ObjectType::Table => SUBSTATEMENT_DROP_TABLE,
            ObjectType::View => SUBSTATEMENT_DROP_VIEW,
            ObjectType::Schema | ObjectType::Database => SUBSTATEMENT_DROP_DATABASE,
            _ => return None,
        },
        SqlStatement::ShowTables { .. } => "SHOW_TABLES",
        SqlStatement::ShowDatabases { .. } | SqlStatement::ShowSchemas { .. } => "SHOW_DATABASES",
        SqlStatement::ShowColumns { .. } => "SHOW_COLUMNS",
        SqlStatement::ShowCreate { .. } => "SHOW_CREATE_TABLE",
        SqlStatement::Explain { .. } | SqlStatement::ExplainTable { .. } => "EXPLAIN",
        _ => return None,
    })
}

/// How one parsed statement runs: what the catalog performs, what it resolves relations
/// for, and what is the engine's own business.
fn plan_of(statement: SqlStatement, context: &QueryContext) -> Plan {
    match &statement {
        // A database is a CATALOG object: the statement defines, drops or lists the
        // container the catalog holds tables in, and reads no data at all. A `SCHEMA` is
        // the same object under the other spelling of the word.
        SqlStatement::CreateDatabase {
            db_name,
            if_not_exists,
            location,
            ..
        } => match database_statement(db_name, context) {
            Some(target) => Plan::Catalog(Box::new(CatalogStatement::CreateDatabase(
                DatabaseStatement {
                    target,
                    if_not_exists: *if_not_exists,
                    location: location.clone(),
                    comment: None,
                    properties: Vec::new(),
                },
            ))),
            None => Plan::Engine,
        },
        SqlStatement::CreateSchema {
            schema_name,
            if_not_exists,
            options,
            ..
        } => match schema_statement_name(schema_name)
            .and_then(|name| database_statement(name, context))
        {
            Some(target) => Plan::Catalog(Box::new(CatalogStatement::CreateDatabase(
                DatabaseStatement {
                    target,
                    if_not_exists: *if_not_exists,
                    location: None,
                    comment: None,
                    properties: options
                        .iter()
                        .flatten()
                        .filter_map(key_value)
                        .collect(),
                },
            ))),
            None => Plan::Engine,
        },
        SqlStatement::Drop {
            object_type: ObjectType::Database | ObjectType::Schema,
            if_exists,
            names,
            cascade,
            ..
        } => match names.as_slice() {
            [name] => match database_statement(name, context) {
                Some(target) => Plan::Catalog(Box::new(CatalogStatement::DropDatabase {
                    target,
                    if_exists: *if_exists,
                    cascade: *cascade,
                })),
                None => Plan::Engine,
            },
            _ => Plan::Engine,
        },
        SqlStatement::ShowDatabases { show_options, .. }
        | SqlStatement::ShowSchemas { show_options, .. } => {
            Plan::Catalog(Box::new(CatalogStatement::ShowDatabases {
                catalog: context.catalog.clone(),
                filter: filter_of(show_options),
            }))
        }
        // A table over data that already exists is a CATALOG entry: the statement
        // registers where the data lives and how to read it, and writes no rows.
        SqlStatement::CreateTable(create) if defines_an_external_table(create) => {
            match external_table(create, context) {
                Some(table) => {
                    Plan::Catalog(Box::new(CatalogStatement::CreateExternalTable(table)))
                }
                None => Plan::Engine,
            }
        }
        SqlStatement::CreateView {
            or_replace,
            name,
            columns,
            query,
            materialized: false,
            ..
        } => match relation_of(name, context) {
            Some(target) => Plan::Catalog(Box::new(CatalogStatement::CreateView(ViewStatement {
                target,
                or_replace: *or_replace,
                query: query.to_string(),
                sources: relations_of(query.as_ref(), context),
                projection: projection_of(query),
                columns: columns
                    .iter()
                    .map(|column| glue_state::fold_name(&column.name.value))
                    .collect(),
            }))),
            None => Plan::Engine,
        },
        SqlStatement::Drop {
            object_type: ObjectType::View,
            if_exists,
            names,
            ..
        } => match names.as_slice() {
            [name] => match relation_of(name, context) {
                Some(target) => Plan::Catalog(Box::new(CatalogStatement::DropView {
                    target,
                    if_exists: *if_exists,
                })),
                None => Plan::Engine,
            },
            _ => Plan::Engine,
        },
        SqlStatement::Drop {
            object_type: ObjectType::Table,
            if_exists,
            names,
            ..
        } => match names.as_slice() {
            [name] => match relation_of(name, context) {
                Some(target) => Plan::Catalog(Box::new(CatalogStatement::DropTable {
                    target,
                    if_exists: *if_exists,
                })),
                None => Plan::Engine,
            },
            _ => Plan::Engine,
        },
        SqlStatement::ShowCreate {
            obj_type: ShowCreateObject::View,
            obj_name,
        } => match relation_of(obj_name, context) {
            Some(target) => Plan::Catalog(Box::new(CatalogStatement::ShowCreateView { target })),
            None => Plan::Engine,
        },
        SqlStatement::ShowTables { show_options, .. } => {
            Plan::Catalog(Box::new(CatalogStatement::ShowRelations {
                schema: schema_of(show_options, context),
                views_only: false,
                filter: filter_of(show_options),
            }))
        }
        SqlStatement::ShowViews { show_options, .. } => {
            Plan::Catalog(Box::new(CatalogStatement::ShowRelations {
                schema: schema_of(show_options, context),
                views_only: true,
                filter: filter_of(show_options),
            }))
        }
        _ => {
            let relations = relations_of(&statement, context);
            if relations.is_empty() {
                return Plan::Engine;
            }
            Plan::Relations(Box::new(RelationalStatement {
                statement,
                relations,
            }))
        }
    }
}

/// The database `name` addresses, given the catalog an unqualified name resolves
/// against, or `None` for a name that addresses no catalog database.
///
/// The name is FOLDED the way the Data Catalog folds a database name, exactly as a
/// relation's is ([`relation_of`]): that is the name the catalog is keyed by.
fn database_statement(name: &ObjectName, context: &QueryContext) -> Option<Schema> {
    let parts = idents_of(name);
    let at = Position::of(parts.first()?);
    let (catalog, database) = match parts.as_slice() {
        [database] => (context.catalog.clone(), database.value.clone()),
        [catalog, database] => (catalog.value.clone(), database.value.clone()),
        _ => return None,
    };
    if database.is_empty() {
        return None;
    }
    Some(Schema {
        catalog,
        database: glue_state::fold_name(&database),
        at,
    })
}

/// The name a `CREATE SCHEMA` defines, or `None` for a form that names no schema of its
/// own — an authorization clause names the owner of the schema, not the schema.
fn schema_statement_name(name: &::sqlparser::ast::SchemaName) -> Option<&ObjectName> {
    use ::sqlparser::ast::SchemaName;
    match name {
        SchemaName::Simple(name) | SchemaName::NamedAuthorization(name, ..) => Some(name),
        SchemaName::UnnamedAuthorization(..) => None,
    }
}

/// Whether this `CREATE TABLE` defines a table over data that already exists: one
/// declared `EXTERNAL`, or one that names where its data lives.
fn defines_an_external_table(create: &CreateTable) -> bool {
    create.query.is_none()
        && (create.external
            || create.location.is_some()
            || create
                .hive_formats
                .as_ref()
                .is_some_and(|formats| formats.location.is_some()))
}

/// The pattern a `SHOW` statement narrows its answer by, as the clause that carries it
/// states it.
fn filter_of(options: &ShowStatementOptions) -> Option<RelationFilter> {
    use ::sqlparser::ast::ShowStatementFilter;
    match options.filter_position.as_ref()? {
        ::sqlparser::ast::ShowStatementFilterPosition::Infix(filter)
        | ::sqlparser::ast::ShowStatementFilterPosition::Suffix(filter) => Some(match filter {
            ShowStatementFilter::Like(pattern) => RelationFilter::Like {
                pattern: pattern.clone(),
                case_insensitive: false,
            },
            ShowStatementFilter::ILike(pattern) => RelationFilter::Like {
                pattern: pattern.clone(),
                case_insensitive: true,
            },
            ShowStatementFilter::NoKeyword(pattern) => RelationFilter::Pattern(pattern.clone()),
            ShowStatementFilter::Where(expression) => {
                RelationFilter::Other(expression.to_string())
            }
        }),
    }
}

/// The database a `SHOW` statement lists, from its own `IN <database>` clause or, with
/// none, from the context the query runs in.
fn schema_of(options: &ShowStatementOptions, context: &QueryContext) -> Schema {
    let named = options
        .show_in
        .as_ref()
        .and_then(|show_in| show_in.parent_name.as_ref());
    let Some(name) = named else {
        return Schema {
            catalog: context.catalog.clone(),
            database: glue_state::fold_name(&context.database),
            at: Position::START,
        };
    };
    let parts = idents_of(name);
    let at = parts.first().map_or(Position::START, |part| Position::of(part));
    match parts.as_slice() {
        [database] => Schema {
            catalog: context.catalog.clone(),
            database: glue_state::fold_name(&database.value),
            at,
        },
        [catalog, database] => Schema {
            catalog: catalog.value.clone(),
            database: glue_state::fold_name(&database.value),
            at,
        },
        _ => Schema {
            catalog: context.catalog.clone(),
            database: glue_state::fold_name(&context.database),
            at,
        },
    }
}

/// The `CREATE EXTERNAL TABLE` statement `create` defines, or `None` when its name is
/// not one that addresses a catalog table.
fn external_table(create: &CreateTable, context: &QueryContext) -> Option<ExternalTable> {
    let target = relation_of(&create.name, context)?;
    let hive = create.hive_formats.as_ref();
    let mut serde_parameters = Vec::new();
    let mut serde_library = None;
    if let Some(format) = hive {
        match &format.row_format {
            Some(HiveRowFormat::SERDE { class }) => serde_library = Some(class.clone()),
            Some(HiveRowFormat::DELIMITED { delimiters }) => {
                for delimiter in delimiters {
                    let character = unescaped(&delimiter.char.value);
                    // `FIELDS TERMINATED BY` sets BOTH parameters Hive records a field
                    // separator in: the field delimiter and the serialization format the
                    // SerDe reads it back from.
                    if matches!(delimiter.delimiter, HiveDelimiter::FieldsTerminatedBy) {
                        serde_parameters.push((
                            SERIALIZATION_FORMAT_PARAMETER.to_string(),
                            character.clone(),
                        ));
                    }
                    serde_parameters.push((
                        delimiter_parameter(&delimiter.delimiter).to_string(),
                        character,
                    ));
                }
            }
            None => {}
        }
        for option in format.serde_properties.iter().flatten() {
            if let Some(entry) = key_value(option) {
                serde_parameters.push(entry);
            }
        }
    }
    Some(ExternalTable {
        target,
        if_not_exists: create.if_not_exists,
        columns: create.columns.iter().map(declared_column).collect(),
        partition_columns: partition_columns(create),
        location: create
            .location
            .clone()
            .or_else(|| hive.and_then(|format| format.location.clone())),
        serde_library,
        serde_parameters,
        table_properties: table_properties(&create.table_options),
        storage: storage_format(hive, create.file_format.as_ref()),
        comment: create.comment.as_ref().map(ToString::to_string),
    })
}

/// The Hive SerDe parameter that carries the character fields are separated by, which is
/// what `ROW FORMAT DELIMITED FIELDS TERMINATED BY` sets.
pub const FIELD_DELIMITER_PARAMETER: &str = "field.delim";

/// The other SerDe parameter a declared field separator sets: the format the SerDe
/// serializes a row in, which for delimited text IS that separator.
pub const SERIALIZATION_FORMAT_PARAMETER: &str = "serialization.format";

/// The character a delimiter clause names, with the escape sequences Hive writes a
/// non-printing separator as resolved to the character each one denotes: the named
/// escapes (`\t`, `\n`, `\r`, `\b`, `\f`, `\0`), an OCTAL escape of up to three digits
/// (`\001`, the unit separator Hive's own default field delimiter is written as) and a
/// 16-bit escape (`\u0001`). A backslash before anything else stands for itself, so a
/// clause naming a literal backslash keeps it.
fn unescaped(text: &str) -> String {
    let mut resolved = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            resolved.push(character);
            continue;
        }
        let Some(escape) = characters.next() else {
            resolved.push('\\');
            break;
        };
        match escape {
            't' => resolved.push('\t'),
            'n' => resolved.push('\n'),
            'r' => resolved.push('\r'),
            'b' => resolved.push('\u{8}'),
            'f' => resolved.push('\u{c}'),
            '\\' => resolved.push('\\'),
            'u' => {
                let digits: String = (0..4)
                    .filter_map(|_| characters.next_if(|next| next.is_ascii_hexdigit()))
                    .collect();
                match u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
                    Some(resolved_character) => resolved.push(resolved_character),
                    None => {
                        resolved.push('\\');
                        resolved.push('u');
                        resolved.push_str(&digits);
                    }
                }
            }
            digit if digit.is_digit(8) => {
                let mut digits = String::from(digit);
                while digits.len() < 3 {
                    match characters.next_if(|next| next.is_digit(8)) {
                        Some(next) => digits.push(next),
                        None => break,
                    }
                }
                match u32::from_str_radix(&digits, 8).ok().and_then(char::from_u32) {
                    Some(resolved_character) => resolved.push(resolved_character),
                    None => resolved.push_str(&digits),
                }
            }
            other => {
                resolved.push('\\');
                resolved.push(other);
            }
        }
    }
    resolved
}

/// The SerDe parameter one `ROW FORMAT DELIMITED` clause sets: each clause of that row/// format names one separator or escape of the text the table holds, and the catalog
/// records it as the SerDe parameter Hive reads it back from.
fn delimiter_parameter(delimiter: &HiveDelimiter) -> &'static str {
    match delimiter {
        HiveDelimiter::FieldsTerminatedBy => FIELD_DELIMITER_PARAMETER,
        HiveDelimiter::FieldsEscapedBy => "escape.delim",
        HiveDelimiter::CollectionItemsTerminatedBy => "collection.delim",
        HiveDelimiter::MapKeysTerminatedBy => "mapkey.delim",
        HiveDelimiter::LinesTerminatedBy => "line.delim",
        HiveDelimiter::NullDefinedAs => "serialization.null.format",
    }
}

/// The partition columns a `CREATE EXTERNAL TABLE` declares in its
/// `PARTITIONED BY (…)` clause.
fn partition_columns(create: &CreateTable) -> Vec<DeclaredColumn> {
    use ::sqlparser::ast::HiveDistributionStyle;
    match &create.hive_distribution {
        HiveDistributionStyle::PARTITIONED { columns } => {
            columns.iter().map(declared_column).collect()
        }
        _ => Vec::new(),
    }
}

/// The storage format a statement declared: the `STORED AS` clause of its row format,
/// or the file format the statement named on its own.
fn storage_format(
    hive: Option<&HiveFormat>,
    file_format: Option<&::sqlparser::ast::FileFormat>,
) -> Option<StorageFormat> {
    match hive.and_then(|format| format.storage.as_ref()) {
        Some(HiveIOFormat::IOF {
            input_format,
            output_format,
        }) => {
            return Some(StorageFormat::Formats {
                input: literal_text(input_format),
                output: literal_text(output_format),
            })
        }
        Some(HiveIOFormat::FileFormat { format }) => {
            return Some(StorageFormat::Named(format.to_string()))
        }
        None => {}
    }
    file_format.map(|format| StorageFormat::Named(format.to_string()))
}

/// One declared column, as the catalog records it.
fn declared_column(column: &ColumnDef) -> DeclaredColumn {
    let comment = column.options.iter().find_map(|option| match &option.option {
        ColumnOption::Comment(comment) => Some(comment.clone()),
        _ => None,
    });
    DeclaredColumn {
        name: glue_state::fold_name(&column.name.value),
        data_type: column.data_type.to_string().to_ascii_lowercase(),
        comment,
    }
}

/// The `TBLPROPERTIES` a statement declared, whichever option list the parser read them
/// into.
fn table_properties(options: &CreateTableOptions) -> Vec<(String, String)> {
    let declared = match options {
        CreateTableOptions::None => &[] as &[SqlOption],
        CreateTableOptions::With(options)
        | CreateTableOptions::Options(options)
        | CreateTableOptions::Plain(options)
        | CreateTableOptions::TableProperties(options) => options.as_slice(),
    };
    declared.iter().filter_map(key_value).collect()
}

/// One `key = 'value'` option, as the pair it states.
fn key_value(option: &SqlOption) -> Option<(String, String)> {
    match option {
        SqlOption::KeyValue { key, value } => Some((key.value.clone(), literal_text(value))),
        _ => None,
    }
}

/// One option's value as the text it denotes: the contents of a string literal, or the
/// expression as written for anything else.
fn literal_text(value: &Expr) -> String {
    match value {
        Expr::Value(value) => match &value.value {
            Value::SingleQuotedString(text)
            | Value::DoubleQuotedString(text)
            | Value::EscapedStringLiteral(text) => text.clone(),
            other => other.to_string(),
        },
        other => other.to_string(),
    }
}

/// The identifiers a possibly-qualified name is made of, or an empty list when a part of
/// it is not an identifier at all.
fn idents_of(name: &ObjectName) -> Vec<&Ident> {
    let parts: Vec<&Ident> = name.0.iter().filter_map(ObjectNamePart::as_ident).collect();
    if parts.len() == name.0.len() {
        parts
    } else {
        Vec::new()
    }
}

/// The relation `name` addresses, given the context an unqualified name resolves
/// against, or `None` for a name that addresses no catalog relation.
fn relation_of(name: &ObjectName, context: &QueryContext) -> Option<Relation> {
    let parts = idents_of(name);
    let at = Position::of(parts.first()?);
    let (catalog, database, table) = match parts.as_slice() {
        [table] => (
            context.catalog.clone(),
            context.database.clone(),
            table.value.clone(),
        ),
        [database, table] => (
            context.catalog.clone(),
            database.value.clone(),
            table.value.clone(),
        ),
        [catalog, database, table] => (
            catalog.value.clone(),
            database.value.clone(),
            table.value.clone(),
        ),
        _ => return None,
    };
    if table.is_empty() || database.is_empty() {
        return None;
    }
    Some(Relation {
        catalog,
        database: glue_state::fold_name(&database),
        table: glue_state::fold_name(&table),
        written: name.to_string(),
        at,
    })
}

/// The relations a parsed node READS, in first-appearance order.
///
/// Only the relations inside a QUERY are collected: a name a statement defines, drops or
/// writes into is not a relation the catalog has to resolve for it, and neither is a name
/// the statement defined for itself in a `WITH` clause.
pub fn relations_of<N: Visit>(node: &N, context: &QueryContext) -> Vec<Relation> {
    let mut collector = Relations {
        context: Some(context),
        ..Relations::default()
    };
    let _ = node.visit(&mut collector);
    collector.relations
}

/// Collects the relations a query reads; see [`relations_of`].
#[derive(Debug, Default)]
struct Relations<'c> {
    /// What an unqualified name resolves against.
    context: Option<&'c QueryContext>,
    /// The relations collected, in first-appearance order.
    relations: Vec<Relation>,
    /// The written names already collected, so the walk stays linear.
    seen: HashSet<String>,
    /// The names the statement defined for itself in a `WITH` clause.
    defined: HashSet<String>,
    /// How many query nodes the walk is inside.
    depth: usize,
}

impl Visitor for Relations<'_> {
    type Break = ();

    fn pre_visit_query(&mut self, query: &::sqlparser::ast::Query) -> ControlFlow<Self::Break> {
        self.depth = self.depth.saturating_add(1);
        for cte in query.with.iter().flat_map(|with| with.cte_tables.iter()) {
            self.defined
                .insert(glue_state::fold_name(&cte.alias.name.value));
        }
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, _query: &::sqlparser::ast::Query) -> ControlFlow<Self::Break> {
        self.depth = self.depth.saturating_sub(1);
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<Self::Break> {
        if self.depth == 0 {
            return ControlFlow::Continue(());
        }
        let Some(context) = self.context else {
            return ControlFlow::Continue(());
        };
        let Some(resolved) = relation_of(relation, context) else {
            return ControlFlow::Continue(());
        };
        // A name the statement defined for itself is not the catalog's to resolve.
        if relation.0.len() == 1 && self.defined.contains(&resolved.table) {
            return ControlFlow::Continue(());
        }
        if !self.seen.insert(resolved.written.clone()) {
            return ControlFlow::Continue(());
        }
        if self.relations.len() >= MAX_RELATIONS {
            return ControlFlow::Break(());
        }
        self.relations.push(resolved);
        ControlFlow::Continue(())
    }
}

/// Rewrite every relation `local_names` names to the single name its rows are reachable
/// under in the engine database, and render the statement as the SQL the engine runs.
///
/// The name is replaced in the PARSE and the statement re-rendered from it, so nothing
/// is spliced into the submitted text and a name that happens to appear in a string
/// literal is left exactly as it was.
pub fn rewritten(
    statement: &mut SqlStatement,
    local_names: &::std::collections::HashMap<String, String>,
) -> String {
    let _ = ::sqlparser::ast::visit_relations_mut(statement, |relation: &mut ObjectName| {
        if let Some(local) = local_names.get(&relation.to_string()) {
            *relation = ObjectName(vec![ObjectNamePart::Identifier(Ident::with_quote(
                '"', local,
            ))]);
        }
        ControlFlow::<()>::Continue(())
    });
    statement.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The context the cases below resolve an unqualified name against.
    fn context() -> QueryContext {
        QueryContext {
            catalog: "AwsDataCatalog".to_string(),
            database: "sales".to_string(),
        }
    }

    /// Every form Athena's DDL engine takes a database definition in is READ as the
    /// catalog statement it is: the rows differ along the quoting of the name, the
    /// tolerance clause, and the options Hive's grammar lets a database carry.
    #[test]
    fn database_definitions_are_read_in_every_form_the_ddl_engine_takes() {
        // (case, the statement)
        let cases: Vec<(&str, &str)> = vec![
            ("a back-quoted name", "CREATE DATABASE `shop-ddl`"),
            (
                "a back-quoted name it tolerates the presence of",
                "CREATE DATABASE IF NOT EXISTS `shop-ddl`",
            ),
            (
                "a description",
                "CREATE DATABASE `shop-ddl` COMMENT 'the shop'",
            ),
            (
                "a default location for the tables in it",
                "CREATE DATABASE `shop-ddl` LOCATION 's3://bucket/shop/'",
            ),
            (
                "properties",
                "CREATE DATABASE `shop-ddl` WITH DBPROPERTIES ('owner' = 'shop')",
            ),
            (
                "every clause at once",
                "CREATE DATABASE IF NOT EXISTS `shop-ddl` COMMENT 'the shop' \
                 LOCATION 's3://bucket/shop/' WITH DBPROPERTIES ('owner' = 'shop')",
            ),
        ];

        let mut unread: Vec<(&str, String)> = Vec::new();
        for (case, sql) in cases {
            let reading = read(sql, &context());
            let plan = match &reading.plan {
                Plan::Catalog(statement) => match statement.as_ref() {
                    CatalogStatement::CreateDatabase(database) => {
                        database.target.database.clone()
                    }
                    other => format!("{other:?}"),
                },
                other => format!("{other:?}"),
            };
            if plan != "shop-ddl" {
                unread.push((case, plan));
            }
        }
        assert!(
            unread.is_empty(),
            "every form must be read as the database definition, and these were not: \
             {unread:?}"
        );
    }

    /// Hive DDL and Presto DDL are both READ, and each lands on the plan that performs
    /// it: the rows differ along the grammar the statement is written in, how its name is
    /// qualified and quoted, and which catalog statement it is.
    #[test]
    fn catalog_statements_are_read_off_the_parse() {
        // (case, the statement, the plan it must land on)
        let cases: Vec<(&str, &str, &str)> = vec![
            (
                "Hive DDL defining a database with a back-quoted name",
                "CREATE DATABASE `shop_ddl`",
                "create database shop_ddl",
            ),
            (
                "a database defined only if it is absent",
                "CREATE DATABASE IF NOT EXISTS Shop_DDL",
                "create database shop_ddl if absent",
            ),
            (
                "the Presto spelling of the same object",
                "CREATE SCHEMA shop_ddl",
                "create database shop_ddl",
            ),
            (
                "a database dropped",
                "DROP DATABASE shop_ddl",
                "drop database shop_ddl",
            ),
            (
                "a database dropped with everything in it",
                "DROP DATABASE IF EXISTS shop_ddl CASCADE",
                "drop database shop_ddl if present cascade",
            ),
            (
                "the Presto spelling of the drop",
                "DROP SCHEMA shop_ddl",
                "drop database shop_ddl",
            ),
            ("the databases listed", "SHOW DATABASES", "show databases"),
            (
                "the databases narrowed by a SQL pattern",
                "SHOW SCHEMAS LIKE 'shop%'",
                "show databases like shop%",
            ),
            (
                "a table dropped",
                "DROP TABLE shop.events",
                "drop table shop.events",
            ),
            (
                "Hive DDL with back-quoted names and a location",
                "CREATE EXTERNAL TABLE `shop`.`ddl_tbl` (id bigint) LOCATION 's3://b/ddl/'",
                "external table shop.ddl_tbl",
            ),
            (
                "Hive DDL with IF NOT EXISTS, a row format and table properties",
                "CREATE EXTERNAL TABLE IF NOT EXISTS ddl_tbl (id int, name string) \
                 ROW FORMAT DELIMITED FIELDS TERMINATED BY ',' LOCATION 's3://b/p/' \
                 TBLPROPERTIES ('skip.header.line.count' = '1')",
                "external table sales.ddl_tbl",
            ),
            (
                "a Presto view over a double-quoted qualified name",
                "CREATE VIEW \"shop\".\"v_recent\" AS SELECT id FROM \"shop\".\"events\"",
                "view shop.v_recent",
            ),
            (
                "a view replaced",
                "CREATE OR REPLACE VIEW v AS SELECT 1 AS n",
                "view sales.v",
            ),
            ("a view dropped", "DROP VIEW shop.v", "drop view shop.v"),
            (
                "the definition of a view reported",
                "SHOW CREATE VIEW shop.v",
                "show create view shop.v",
            ),
            ("the tables of a database listed", "SHOW TABLES IN shop", "show tables shop"),
            (
                "the tables of a database narrowed by the catalog's pattern language",
                "SHOW TABLES IN shop 'v_*'",
                "show tables shop matching v_*",
            ),
            (
                "the views of a database narrowed by a SQL pattern",
                "SHOW VIEWS IN shop LIKE 'v%'",
                "show views shop like v%",
            ),
            ("the views of a database listed", "SHOW VIEWS IN shop", "show views shop"),
            (
                "a projection over a qualified relation",
                "SELECT count(*) FROM \"shop\".\"events\"",
                "relations shop.events",
            ),
            (
                "a projection over an unqualified relation",
                "SELECT * FROM events",
                "relations sales.events",
            ),
            (
                "a projection the catalog resolves nothing for",
                "SELECT 1 AS n",
                "engine",
            ),
            (
                "a name the statement defined for itself",
                "WITH counted(n) AS (SELECT 1) SELECT n FROM counted",
                "engine",
            ),
            (
                "a table the statement defines in the engine",
                "CREATE TABLE orders (id integer, item varchar)",
                "engine",
            ),
        ];

        for (case, sql, expected) in cases {
            let reading = read(sql, &context());
            let actual = match &reading.plan {
                Plan::Engine => "engine".to_string(),
                Plan::Catalog(statement) => match statement.as_ref() {
                    CatalogStatement::CreateDatabase(database) => format!(
                        "create database {}{}",
                        database.target.database,
                        if database.if_not_exists { " if absent" } else { "" }
                    ),
                    CatalogStatement::DropDatabase {
                        target,
                        if_exists,
                        cascade,
                    } => format!(
                        "drop database {}{}{}",
                        target.database,
                        if *if_exists { " if present" } else { "" },
                        if *cascade { " cascade" } else { "" }
                    ),
                    CatalogStatement::ShowDatabases { filter, .. } => format!(
                        "show databases{}",
                        match filter {
                            None => String::new(),
                            Some(RelationFilter::Pattern(pattern)) => {
                                format!(" matching {pattern}")
                            }
                            Some(RelationFilter::Like { pattern, .. }) => {
                                format!(" like {pattern}")
                            }
                            Some(RelationFilter::Other(form)) => format!(" where {form}"),
                        }
                    ),
                    CatalogStatement::DropTable { target, .. } => {
                        format!("drop table {}.{}", target.database, target.table)
                    }
                    CatalogStatement::CreateExternalTable(table) => {
                        format!(
                            "external table {}.{}",
                            table.target.database, table.target.table
                        )
                    }
                    CatalogStatement::CreateView(view) => {
                        format!("view {}.{}", view.target.database, view.target.table)
                    }
                    CatalogStatement::DropView { target, .. } => {
                        format!("drop view {}.{}", target.database, target.table)
                    }
                    CatalogStatement::ShowCreateView { target } => {
                        format!("show create view {}.{}", target.database, target.table)
                    }
                    CatalogStatement::ShowRelations {
                        schema,
                        views_only,
                        filter,
                    } => format!(
                        "show {} {}{}",
                        if *views_only { "views" } else { "tables" },
                        schema.database,
                        match filter {
                            None => String::new(),
                            Some(RelationFilter::Pattern(pattern)) => {
                                format!(" matching {pattern}")
                            }
                            Some(RelationFilter::Like { pattern, .. }) => {
                                format!(" like {pattern}")
                            }
                            Some(RelationFilter::Other(form)) => format!(" where {form}"),
                        }
                    ),
                },
                Plan::Relations(relational) => format!(
                    "relations {}",
                    relational
                        .relations
                        .iter()
                        .map(Relation::local_name)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            };
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the plan {expected:?}, got {actual:?}"
            );
        }
    }

    /// What a `CREATE EXTERNAL TABLE` declares is read off the parse: its columns and
    /// their types, where the data lives, the delimiter fields are separated by and the
    /// table properties.
    #[test]
    fn an_external_table_definition_carries_what_the_statement_declared() {        let reading = read(
            "CREATE EXTERNAL TABLE shop.ddl_tbl (id bigint, name string) \
             PARTITIONED BY (day string) \
             ROW FORMAT DELIMITED FIELDS TERMINATED BY '\\t' \
             STORED AS TEXTFILE LOCATION 's3://bucket/ddl/' \
             TBLPROPERTIES ('skip.header.line.count' = '1')",
            &context(),
        );
        let Plan::Catalog(statement) = reading.plan else {
            panic!("case declaration: expected a catalog statement, got {reading:?}");
        };
        let CatalogStatement::CreateExternalTable(table) = *statement else {
            panic!("case declaration: expected an external table");
        };
        assert_eq!(
            table
                .columns
                .iter()
                .map(|column| (column.name.as_str(), column.data_type.as_str()))
                .collect::<Vec<_>>(),
            vec![("id", "bigint"), ("name", "string")],
            "case declaration: unexpected columns in {:?}",
            table.columns
        );
        assert_eq!(
            table
                .partition_columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["day"],
            "case declaration: unexpected partition columns in {:?}",
            table.partition_columns
        );
        assert_eq!(
            table.location.as_deref(),
            Some("s3://bucket/ddl/"),
            "case declaration: unexpected location in {table:?}"
        );
        assert!(
            table
                .serde_parameters
                .iter()
                .any(|(name, value)| name == FIELD_DELIMITER_PARAMETER && value == "\t"),
            "case declaration: expected the declared field delimiter, got {:?}",
            table.serde_parameters
        );
        assert_eq!(
            table.table_properties,
            vec![("skip.header.line.count".to_string(), "1".to_string())],
            "case declaration: unexpected table properties"
        );
    }

    /// A statement bigger than this emulator reads as one of its own is NOT read here:
    /// the token bound in front of the parse is what keeps a tree whose depth grows with
    /// the input length — which a parser recursion limit does not catch, because such a
    /// statement is parsed at constant parser depth — away from the recursive walks over
    /// it. The rows differ along the shape that grows the tree, with an ordinary statement
    /// of the same kind beside each one.
    #[test]
    fn a_statement_past_the_token_bound_is_not_read_here() {
        let chain = |terms: usize| {
            let mut sql = String::from("SELECT 1");
            for _ in 0..terms {
                sql.push_str(" + 1");
            }
            sql
        };
        let projection = |columns: usize| {
            let mut sql = String::from("SELECT 1 AS c0");
            for column in 1..columns {
                sql.push_str(&format!(", 1 AS c{column}"));
            }
            sql
        };

        // (case, the statement, whether this reader reads it)
        let cases: Vec<(&str, String, bool)> = vec![
            ("an ordinary projection", chain(4), true),
            (
                "a left-associative chain this reader still reads",
                chain(MAX_STATEMENT_TOKENS / 4),
                true,
            ),
            (
                "a left-associative chain past the bound",
                chain(MAX_STATEMENT_TOKENS),
                false,
            ),
            (
                "a wide projection past the bound",
                projection(MAX_STATEMENT_TOKENS),
                false,
            ),
            (
                "deep nesting, which the parser's own recursion limit refuses",
                format!(
                    "SELECT {}1{}",
                    "(".repeat(MAX_PARSER_RECURSION * 2),
                    ")".repeat(MAX_PARSER_RECURSION * 2)
                ),
                false,
            ),
        ];

        for (case, sql, expected) in cases {
            let actual = parse_statements(&sql).is_some();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected read={expected}, got {actual} for a statement of \
                 {} bytes",
                sql.len()
            );
        }
    }

    /// A delimiter clause names a CHARACTER, and the escape sequences Hive writes a    /// non-printing one as stand for that character: the rows differ along the form of
    /// the escape, and one row is a printable separator that is no escape at all.
    #[test]
    fn a_declared_delimiter_resolves_the_escape_it_is_written_as() {
        // (the text the clause carries, the character it names)
        let cases: Vec<(&str, &str)> = vec![
            ("\\001", "\u{1}"),
            ("\\t", "\t"),
            ("\\n", "\n"),
            ("\\r", "\r"),
            ("\\u0001", "\u{1}"),
            ("\\\\", "\\"),
            (",", ","),
            ("|", "|"),
            ("\\q", "\\q"),
        ];

        for (declared, expected) in cases {
            let actual = unescaped(declared);
            assert_eq!(
                actual, expected,
                "case {declared:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
