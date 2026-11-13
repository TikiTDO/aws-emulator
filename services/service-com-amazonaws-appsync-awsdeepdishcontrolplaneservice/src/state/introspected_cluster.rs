// SPDX-License-Identifier: CC0-1.0

//! The schema of the cluster a data source introspection reads, asked of the service that
//! owns it.
//!
//! `RdsDataApiConfig` "contains the metadata required to introspect the RDS cluster" and
//! carries exactly what a Data API call carries — the cluster's ARN, the secret holding
//! the login, and the database to run against — so an introspection reads the schema the
//! way any client of that cluster reads it: through the RDS Data API, driven over this
//! emulator's own in-process transport. Nothing about the cluster, the secret or the
//! database is decided here: whether the cluster serves the Data API at all, whether the
//! secret can be retrieved and whether the database exists are the Data API's own
//! preconditions, and whatever it refuses the call with becomes the reason the
//! introspection failed ("the exact error that was generated during the operation").
//!
//! What is read, for every table of that database:
//!
//! * its COLUMNS, from the `columnMetadata` the Data API describes a result set with —
//!   each column's name, the PostgreSQL type the catalog holds for it, the length that
//!   type's declaration bounds it to and whether it admits a null. A select that matches
//!   no row describes every column of the table and returns no data, so the schema is
//!   read without reading anything stored in it;
//! * its PRIMARY KEY, from the schema introspection the engine behind that API admits;
//! * the INDEXES its schema declares, whose columns are read out of each index's own
//!   `CREATE INDEX` statement with the same SQL parser that API parses statements with.
//!   An index the engine created for a constraint carries no declaration of its own; the
//!   primary key is reported as the model's own `primaryKey` rather than as one of these.

use std::collections::BTreeMap;

use super::data_source_introspection::{
    IntrospectedField, IntrospectedModel, IntrospectionOutcome,
};
use crate::ops::types::RdsDataApiConfig;

/// Read the schema of the database `config` names and answer what the introspection of it
/// settles to.
///
/// Either outcome is the DATABASE's own answer: the models are the tables it holds, and a
/// failure is what the service that owns it refused the read with.
pub async fn introspect(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    config: &RdsDataApiConfig,
) -> IntrospectionOutcome {
    // A cluster is resolved in the Region its own ARN names, which is the Region the Data
    // API that serves it answers in. The member's modelled pattern is what bounds that
    // field, so nothing else can arrive in the authority this endpoint is addressed by.
    let region = super::arn_region(&config.resource_arn)
        .map_or_else(|| super::request_region(ctx), str::to_string);
    let reader = SchemaReader {
        client: data_api(ctx, &region),
        config: config.clone(),
    };
    match reader.read().await {
        Ok(models) => IntrospectionOutcome::Introspected(models),
        Err(detail) => IntrospectionOutcome::Failed(detail),
    }
}

/// The Data API statement that lists what the database holds: a table of its own is a
/// table the introspection reports a model for.
const TABLES_STATEMENT: &str = "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name";

/// The statement that lists the indexes the schema holds, with the declaration each one
/// names its columns in.
const INDEXES_STATEMENT: &str =
    "SELECT name, tbl_name, sql FROM sqlite_master WHERE type = 'index' ORDER BY name";

/// The prefix the engine reserves for the objects it keeps its own bookkeeping in. Such a
/// table is not a table of the caller's schema, so no model is reported for it.
const RESERVED_TABLE_PREFIX: &str = "sqlite_";

/// How many tables ONE introspection reads.
///
/// A bound on the work one call starts: reading a table costs two statements against the
/// service that owns the database, and nothing else bounds how many tables a database
/// holds, so a single introspection could otherwise run as much engine work as the schema
/// it was pointed at is large. Set far above any schema an emulator is pointed at, and a
/// schema past it is reported as this emulator's capacity, the way the service that owns
/// the cluster reports its own caps.
const MAX_INTROSPECTED_TABLES: usize = 64;

/// What an introspection reports for a schema with more tables than it reads.
fn too_many_tables_detail(tables: usize) -> String {
    format!(
        "this emulator introspects at most {MAX_INTROSPECTED_TABLES} tables per data source, \
         and this database holds {tables}"
    )
}

/// The reading of one database's schema: the client it is read through and the call's own
/// configuration, which every statement carries.
struct SchemaReader {
    client: ::aws_sdk_rdsdata::Client,
    config: RdsDataApiConfig,
}

impl SchemaReader {
    /// Every model the database defines, in the order its tables are named, or the reason
    /// the schema could not be read.
    async fn read(&self) -> Result<Vec<IntrospectedModel>, String> {
        let tables: Vec<String> = self
            .query(TABLES_STATEMENT)
            .await?
            .into_iter()
            .filter_map(|row| row.first().and_then(|value| value.text()))
            .filter(|table| !table.starts_with(RESERVED_TABLE_PREFIX))
            .collect();
        // Reading one table costs two statements against the service that owns the
        // database, so the work ONE call starts grows with the schema it was pointed at.
        // Past the cap the introspection reports this emulator's own capacity rather than
        // running that work or reporting part of the schema as if it were all of it.
        if tables.len() > MAX_INTROSPECTED_TABLES {
            return Err(too_many_tables_detail(tables.len()));
        }
        let mut indexes = self.read_indexes().await?;
        let mut models = Vec::with_capacity(tables.len());
        for table in tables {
            let fields = self.read_fields(&table).await?;
            let primary_key = self.read_primary_key(&table).await?;
            models.push(IntrospectedModel {
                indexes: indexes.remove(&table).unwrap_or_default(),
                name: table,
                fields,
                primary_key,
            });
        }
        Ok(models)
    }

    /// One field per column `table` declares, in declaration order, as the Data API
    /// describes them.
    async fn read_fields(&self, table: &str) -> Result<Vec<IntrospectedField>, String> {
        let described = self
            .execute(&format!("SELECT * FROM {} WHERE 1 = 0", quoted(table)))
            .await?;
        Ok(described
            .column_metadata()
            .iter()
            .map(|column| {
                let type_name = column.type_name().unwrap_or_default();
                IntrospectedField {
                    name: column.name().unwrap_or_default().to_string(),
                    scalar: appsync_scalar(type_name).to_string(),
                    non_null: column.nullable() == COLUMN_NO_NULLS,
                    length: declared_length(type_name, column.precision()),
                }
            })
            .collect())
    }

    /// The columns the PRIMARY KEY of `table` covers, in key order, and empty for a table
    /// that declares none.
    async fn read_primary_key(&self, table: &str) -> Result<Vec<String>, String> {
        let rows = self
            .query(&format!(
                "SELECT name FROM pragma_table_info({}) WHERE pk > 0 ORDER BY pk",
                literal(table)
            ))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| row.first().and_then(|value| value.text()))
            .collect())
    }

    /// The indexes the schema declares, grouped by the table each one covers.
    ///
    /// An index row carrying no declaration is one the engine created for a constraint
    /// rather than one the schema declares, and the statement it would name its columns in
    /// does not exist, so it is not reported.
    async fn read_indexes(&self) -> Result<BTreeMap<String, BTreeMap<String, Vec<String>>>, String> {
        let mut grouped: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
        for row in self.query(INDEXES_STATEMENT).await? {
            let (Some(name), Some(table), Some(declaration)) = (
                row.first().and_then(|value| value.text()),
                row.get(1).and_then(|value| value.text()),
                row.get(2).and_then(|value| value.text()),
            ) else {
                continue;
            };
            let columns = index_columns(&declaration);
            if columns.is_empty() {
                continue;
            }
            grouped.entry(table).or_default().insert(name, columns);
        }
        Ok(grouped)
    }

    /// Run `sql` and answer its rows.
    async fn query(&self, sql: &str) -> Result<Vec<Vec<::aws_sdk_rdsdata::types::Field>>, String> {
        Ok(self.execute(sql).await?.records.unwrap_or_default())
    }

    /// Run one statement against the database the call named.
    ///
    /// Every refusal is the answer of the service that owns the cluster, passed back as
    /// the reason the introspection failed. A request that reached no Data API at all
    /// resolves nothing about the cluster, so the introspection reports that the cluster
    /// could not be reached rather than an empty schema.
    async fn execute(
        &self,
        sql: &str,
    ) -> Result<::aws_sdk_rdsdata::operation::execute_statement::ExecuteStatementOutput, String>
    {
        use ::aws_sdk_rdsdata::error::ProvideErrorMetadata;
        self.client
            .execute_statement()
            .resource_arn(&self.config.resource_arn)
            .secret_arn(&self.config.secret_arn)
            .database(&self.config.database_name)
            .sql(sql)
            .include_result_metadata(true)
            .send()
            .await
            .map_err(|error| {
                let code = error.code().map(str::to_string);
                if code
                    .as_deref()
                    .is_some_and(|code| TRANSPORT_REFUSALS.contains(&code))
                {
                    ::tracing::debug!(
                        "no Data API answered, so the cluster a data source introspection \
                         names could not be read"
                    );
                    return unreachable_cluster_detail(&self.config.resource_arn);
                }
                error
                    .message()
                    .map(str::to_string)
                    .unwrap_or_else(|| unreachable_cluster_detail(&self.config.resource_arn))
            })
    }
}

/// The error codes that are the in-process TRANSPORT's own refusal rather than an answer
/// from the Data API: this emulator holds no Data API at the endpoint that was called, or
/// more than one of its services claims it. Either way the request reached no Data API, so
/// it says nothing about the cluster's schema.
const TRANSPORT_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// What an introspection reports for a cluster whose Data API endpoint answered nothing at
/// all: the schema was not read, and the ARN that was addressed is what a caller has to
/// act on.
fn unreachable_cluster_detail(resource_arn: &str) -> String {
    format!("The data source {resource_arn} could not be reached.")
}

/// `java.sql.ResultSetMetaData`'s nullability for a column that admits no null — the value
/// `ColumnMetadata$nullable` carries for one, which is what makes a field non-null.
const COLUMN_NO_NULLS: i32 = 0;

/// What a value of one column of one row holds, where it holds text or nothing.
trait TextValue {
    /// The text this value carries, and `None` where it carries none.
    fn text(&self) -> Option<String>;
}

impl TextValue for ::aws_sdk_rdsdata::types::Field {
    fn text(&self) -> Option<String> {
        match self {
            ::aws_sdk_rdsdata::types::Field::StringValue(text) => Some(text.clone()),
            _ => None,
        }
    }
}

/// `table` as an identifier a statement names it by, quoted as SQL quotes one so a name
/// carrying a separator or a keyword still names the one table.
///
/// Every name this is given came from the database's OWN catalog rather than from a
/// request, and it is quoted and escaped on the way back in, so a statement names the
/// table the catalog listed and nothing else.
fn quoted(table: &str) -> String {
    format!("\"{}\"", table.replace('"', "\"\""))
}

/// `table` as a string literal a statement passes it as, escaped as SQL escapes one (the
/// name's origin is the one [`quoted`] states).
fn literal(table: &str) -> String {
    format!("'{}'", table.replace('\'', "''"))
}

/// The columns an index covers, read out of the `CREATE INDEX` statement its schema holds.
///
/// Parsed with the same maintained parser, and the same PostgreSQL dialect, the Data API
/// itself parses a statement with — nothing here reads the declaration's text. A
/// declaration this parser cannot read names no column, so the index it declares is not
/// reported rather than reported with a column list guessed from the text.
fn index_columns(declaration: &str) -> Vec<String> {
    let Ok(statements) =
        ::sqlparser::parser::Parser::parse_sql(&::sqlparser::dialect::PostgreSqlDialect {}, declaration)
    else {
        return Vec::new();
    };
    statements
        .into_iter()
        .find_map(|statement| match statement {
            ::sqlparser::ast::Statement::CreateIndex(create) => Some(
                create
                    .columns
                    .into_iter()
                    .filter_map(|column| match column.column.expr {
                        ::sqlparser::ast::Expr::Identifier(name) => Some(name.value),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// The AppSync scalar a column's values are reported as, from the PostgreSQL type the
/// cluster's catalog holds for it.
///
/// The names on the left are PostgreSQL's own type names and the aliases its catalog
/// accepts for them ("Data Types"); the ones on the right are the scalars every AppSync
/// schema holds without defining them ([`super::BUILT_IN_SCALARS`]). A type the catalog
/// does not classify as one of these carries its values as text, which is what `String`
/// holds.
fn appsync_scalar(type_name: &str) -> &'static str {
    match type_name.to_ascii_lowercase().as_str() {
        "bool" | "boolean" => "Boolean",
        "int2" | "smallint" | "smallserial" | "serial2" | "int4" | "int" | "integer" | "serial"
        | "serial4" | "int8" | "bigint" | "bigserial" | "serial8" | "oid" => "Int",
        "float4" | "real" | "float8" | "double precision" | "numeric" | "decimal" | "dec"
        | "money" => "Float",
        "date" => "AWSDate",
        "time" | "time without time zone" | "timetz" | "time with time zone" => "AWSTime",
        "timestamp" | "timestamp without time zone" | "timestamptz"
        | "timestamp with time zone" => "AWSDateTime",
        "json" | "jsonb" => "AWSJSON",
        _ => "String",
    }
}

/// "The length value of the introspected field": the number of characters the column's
/// declaration bounds it to.
///
/// A length belongs to a CHARACTER type that was declared with one (`varchar(50)`,
/// `char(2)`): the catalog holds it as that type's precision. Every other type carries no
/// declared length — a character type declared without one is bounded only by what the
/// catalog can represent at all ([`UNBOUNDED_LENGTH`]) — and reports none.
fn declared_length(type_name: &str, precision: i32) -> i64 {
    let bounded = (1..UNBOUNDED_LENGTH).contains(&precision);
    match type_name.to_ascii_lowercase().as_str() {
        "varchar" | "character varying" | "char" | "character" | "bpchar" | "name" if bounded => {
            i64::from(precision)
        }
        _ => 0,
    }
}

/// The length the catalog reports for a character type that was declared without one: the
/// largest value the signed 32-bit column it is reported in can hold, i.e. no bound at
/// all.
const UNBOUNDED_LENGTH: i32 = i32::MAX;

/// The Data API client a cluster's schema is read through.
///
/// Given the callback's own HTTP client, so the endpoint URL is only the address that
/// transport routes on IN PROCESS: no socket is opened, no name is resolved and nothing
/// leaves this process. The credentials are the callback's, which sign this in-process hop
/// and never appear in a response. The transport is in-process, so there is nothing to
/// retry, time out or watch for a stalled stream — and each of those would need a sleep
/// implementation this client is not given.
fn data_api(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_rdsdata::Client {
    let config = ::aws_sdk_rdsdata::Config::builder()
        .behavior_version(::aws_sdk_rdsdata::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_rdsdata::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://rds-data.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_rdsdata::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_rdsdata::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_rdsdata::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_rdsdata::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_rdsdata::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: every scalar an introspected column is reported as is one of the scalars an
    /// AppSync schema holds without defining it, and a length is reported only for a
    /// character type that was declared with one.
    ///
    /// The rows span the type classes PostgreSQL's catalog holds — boolean, the integer
    /// widths and their sequence spellings, the inexact and exact numbers, the character
    /// types with and without a declared bound, the date and time types, a JSON document,
    /// and a type this mapping does not name.
    #[test]
    fn every_column_is_reported_as_an_appsync_scalar() {
        // (the catalog's type name, its precision, the scalar, the length)
        let rows: &[(&str, i32, &str, i64)] = &[
            ("bool", 1, "Boolean", 0),
            ("int2", 5, "Int", 0),
            ("int4", 10, "Int", 0),
            ("int8", 19, "Int", 0),
            ("serial", 10, "Int", 0),
            ("float8", 17, "Float", 0),
            ("numeric", 10, "Float", 0),
            ("varchar", 50, "String", 50),
            ("varchar", UNBOUNDED_LENGTH, "String", 0),
            ("bpchar", 2, "String", 2),
            ("text", UNBOUNDED_LENGTH, "String", 0),
            ("date", 13, "AWSDate", 0),
            ("time", 15, "AWSTime", 0),
            ("timestamp", 29, "AWSDateTime", 0),
            ("timestamptz", 35, "AWSDateTime", 0),
            ("jsonb", UNBOUNDED_LENGTH, "AWSJSON", 0),
            ("interval", 49, "String", 0),
        ];
        for (type_name, precision, scalar, length) in rows {
            let reported = appsync_scalar(type_name);
            assert_eq!(
                reported, *scalar,
                "case {type_name}: expected {scalar}, got {reported}"
            );
            assert!(
                super::super::BUILT_IN_SCALARS.contains(&reported),
                "case {type_name}: expected an AppSync scalar, got {reported}"
            );
            let reported = declared_length(type_name, *precision);
            assert_eq!(
                reported, *length,
                "case {type_name}({precision}): expected length {length}, got {reported}"
            );
        }
    }

    /// RULE: an index's columns are the ones its own `CREATE INDEX` statement names, in
    /// the order that statement names them, and a declaration that names none is reported
    /// as no index at all.
    ///
    /// The rows span one column and several, quoted and unquoted names, a declaration
    /// carrying an expression rather than a column, and text that is no statement.
    #[test]
    fn an_index_covers_the_columns_its_declaration_names() {
        let rows: &[(&str, &[&str])] = &[
            ("CREATE INDEX i ON t (a)", &["a"]),
            ("CREATE INDEX i ON \"t\"(\"a\", \"b\")", &["a", "b"]),
            ("CREATE UNIQUE INDEX i ON t (b, a)", &["b", "a"]),
            ("CREATE INDEX i ON t (lower(a))", &[]),
            ("not a statement at all", &[]),
        ];
        for (declaration, expected) in rows {
            let columns = index_columns(declaration);
            assert_eq!(
                columns, *expected,
                "case {declaration:?}: expected {expected:?}, got {columns:?}"
            );
        }
    }
}
