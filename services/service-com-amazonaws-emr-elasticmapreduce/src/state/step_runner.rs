// SPDX-License-Identifier: CC0-1.0

//! What a step's WORK answered: the one thing about a step's run that needs state beyond
//! this service's own.
//!
//! One concern, split out of [`super`]: the cluster's step runner
//! ([`ClusterRecord::advance_steps`]) owns the queue, the concurrency slots, the timeline
//! and the stops, and ends a step in the modelled failure state saying no application ran
//! ([`super::STEP_NOT_EXECUTED_REASON`]). This module is what lets ONE class of step end
//! in the state its own work answered instead, because that answer follows from state the
//! emulator really holds:
//!
//! * `command-runner.jar` "lets you run commands on your cluster" (Amazon EMR
//!   documentation), so its arguments ARE the command line, and a SQL statement the
//!   command carries is the work the cluster's Hive and Spark SQL engines are asked to
//!   run. It is read off the command by PARSING it ([`sqlparser`]), never by matching
//!   text, and every table it names is resolved in the cluster's Hive metastore
//!   ([`read_step_work`]).
//! * That metastore is resolvable only when the cluster's configuration selects the AWS
//!   Glue Data Catalog ([`uses_glue_data_catalog`]) — the one catalog this emulator holds.
//!   A cluster's own local metastore lives on a master node this emulator does not run, so
//!   its contents are unknown and no outcome can be grounded in it.
//! * The statement's own effect has to be nothing but READING ([`StepWork::reads_only`]),
//!   because that is the class whose outcome follows from resolving it: a statement that
//!   writes, defines or drops leaves state behind that this emulator does not apply, and
//!   reporting it `COMPLETED` would make a discarded write look like a stored one.
//!
//! So a read whose every table resolves ends `COMPLETED` ("for a step to be considered
//! complete, the main function must exit with a zero exit code", `AddJobFlowSteps` doc),
//! one naming a table the metastore holds nothing under ends `FAILED` with the driver's
//! table-not-found, and anything else keeps the cluster runner's own not-executed failure.
//! Nothing here reports a success for work nothing performed.

use std::ops::ControlFlow;
use std::time::SystemTime;

use ::service_com_amazonaws_glue_awsglue::state as glue_state;
use sqlparser::ast::{visit_relations, ObjectName, SetExpr, Statement, Use};
use sqlparser::dialect::{Dialect, GenericDialect, HiveDialect};
use sqlparser::parser::Parser;

use super::*;

/// The jar a step names to run a COMMAND on the cluster instead of a jar of its own:
/// "command-runner.jar … lets you run commands on your cluster" (Amazon EMR
/// documentation). Its arguments are the command line, which is why the work a step
/// submits is read off them.
pub const COMMAND_RUNNER_JAR: &str = "command-runner.jar";
/// The property whose value selects the Hive metastore client a cluster's Hive and Spark
/// SQL engines talk to, as a cluster configuration carries it.
pub const HIVE_METASTORE_FACTORY_PROPERTY: &str = "hive.metastore.client.factory.class";

/// The metastore client factory that makes a cluster's Hive metastore the AWS Glue Data
/// Catalog: the value of [`HIVE_METASTORE_FACTORY_PROPERTY`] Amazon EMR documents for
/// "using the AWS Glue Data Catalog as the metastore".
pub const GLUE_METASTORE_FACTORY_CLASS: &str =
    "com.amazonaws.glue.catalog.metastore.AWSGlueDataCatalogHiveClientFactory";

/// The database a table name with no database qualifier is resolved in: Hive and Spark
/// SQL resolve an unqualified name in the session's current database, which is `default`
/// until a `USE` statement names another.
pub const DEFAULT_DATABASE: &str = "default";

/// How deep a command line is lexed before its tokens are read as statements: the
/// `bash -c '<command line>'` form, where the whole command is one argument of the step's
/// own command, and one such wrapping inside another.
const MAX_COMMAND_NESTING: usize = 2;

/// The longest argument of a step's command this runner tries to read as SQL. CHOSEN: the
/// model caps "the string values passed into HadoopJarStep" at 10240 characters in total
/// (`AddJobFlowSteps` doc), so an argument longer than that cap carries no statement this
/// runner has to read.
const MAX_SQL_ARGUMENT_BYTES: usize = 10_240;

/// How deeply a submitted statement may nest before the parser gives up on it. CHOSEN: a
/// bound on the work one request can cost, in the parser's own units.
const SQL_RECURSION_LIMIT: usize = 32;

/// How many distinct tables one step's work is resolved against. CHOSEN: a bound on the
/// metastore lookups one step can cost.
const MAX_TABLES_PER_STEP: usize = 64;

/// How many configuration structures the metastore lookup walks before giving up, the
/// same cap the cluster launch bounds a configuration tree by.
const MAX_CONFIGURATIONS_WALKED: usize = MAX_CONFIGURATION_ENTRIES;

/// A table a step's work names, as the statement spells it and the database it resolves
/// in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableReference {
    pub database: String,
    pub name: String,
}

/// The work a step's command submits, as the runner reads it off that command.
///
/// `statements` is how many SQL statements the command carries — a command carrying none
/// submits no work this runner runs. `reads_only` is whether EVERY one of them has no
/// effect but reading, which is the only class whose outcome this runner can ground
/// without applying an effect; `tables` is every table those statements read, in the order
/// they read them, and is collected only for that class.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StepWork {
    pub statements: usize,
    pub reads_only: bool,
    pub tables: Vec<TableReference>,
}

/// What a step's work ANSWERED: the state the step ends in, and the failure that answer
/// is when it is one.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    pub state: String,
    pub failure: Option<StepFailure>,
}

/// A failure a step's own work answered with, in the two members the model gives one:
/// "the reason for the step failure" (`FailureDetails$Reason`) and the descriptive message
/// beside it.
#[derive(Debug, Clone)]
pub struct StepFailure {
    pub reason: String,
    pub message: String,
}

/// `FailureDetails$Reason` for a step whose statement named a table the cluster's Hive
/// metastore holds nothing under: the class of the failure, beside the driver's own
/// message for it ([`table_not_found_message`]).
pub const TABLE_NOT_FOUND_REASON: &str = "The metastore holds no such table.";

/// What a Spark SQL or Hive driver raises when the metastore holds no such table, which
/// is what fails the step: the statement named a table, the metastore answered that it
/// holds none, and the name it was asked for is the one variable part.
pub fn table_not_found_message(reference: &TableReference) -> String {
    format!(
        "Table or view not found: {}.{}",
        reference.database, reference.name
    )
}

/// `FailureDetails$Message` for a step whose work this emulator did not run.
///
/// A step whose work cannot be accounted for ends the way the cluster's step runner ends
/// every step it ran no application for ([`super::STEP_NOT_EXECUTED_REASON`]): in the
/// modelled failure state with the reason that says so, never `COMPLETED` for work nothing
/// performed — and for a statement that WRITES, never a discarded write no later read
/// could tell from a stored one.
/// See the module docs: the one class of work this service can account for.
pub const RESOLVABLE_WORK: &str = "a statement whose only effect is reading";

/// Whether `jar` names [`COMMAND_RUNNER_JAR`], however the submitter spelled its path.
pub fn names_command_runner(jar: &str) -> bool {
    jar.split('/')
        .next_back()
        .is_some_and(|basename| basename.eq_ignore_ascii_case(COMMAND_RUNNER_JAR))
}

/// The work a step's command submits, or `None` when it submits none this runner runs.
///
/// The command is the step's `Args` ([`COMMAND_RUNNER_JAR`]); each argument is read as a
/// SQL statement list by the parser, and an argument that is no statement is lexed as a
/// command line of its own (the `bash -c '…'` form) and its tokens read the same way, to
/// [`MAX_COMMAND_NESTING`]. A `USE` statement moves the database the statements after it
/// resolve an unqualified name in, exactly as it does in a SQL session.
///
/// A statement whose effect is not reading alone leaves [`StepWork::reads_only`] false and
/// contributes no table: its effect is one this emulator does not apply, so nothing about
/// its outcome can be grounded in what the metastore holds.
pub fn read_step_work(jar: &str, args: &[String]) -> Option<StepWork> {
    if !names_command_runner(jar) {
        return None;
    }
    let mut work = StepWork {
        reads_only: true,
        ..StepWork::default()
    };
    let mut database = DEFAULT_DATABASE.to_string();
    collect_statements(args, 0, &mut work, &mut database);
    if !work.reads_only {
        work.tables.clear();
    }
    (work.statements > 0).then_some(work)
}

/// Read every SQL statement `tokens` carries into `work`, descending into a token that is
/// itself a command line.
fn collect_statements(
    tokens: &[String],
    depth: usize,
    work: &mut StepWork,
    database: &mut String,
) {
    for token in tokens {
        if token.len() > MAX_SQL_ARGUMENT_BYTES {
            continue;
        }
        match parse_statements(token) {
            Some(statements) if !statements.is_empty() => {
                work.statements += statements.len();
                for statement in &statements {
                    if let Some(named) = database_used(statement) {
                        *database = named;
                        continue;
                    }
                    if !reads_only(statement) {
                        work.reads_only = false;
                        continue;
                    }
                    collect_tables(statement, database, work);
                }
            }
            // Not a statement: the token may be a command line carrying one.
            _ => {
                if depth >= MAX_COMMAND_NESTING {
                    continue;
                }
                let Ok(inner) = shell_words::split(token) else {
                    continue;
                };
                // A token that lexes to itself is a plain word, not a command line.
                if inner.len() > 1 {
                    collect_statements(&inner, depth + 1, work, database);
                }
            }
        }
    }
}

/// The statements `sql` is, or `None` when it is no SQL at all.
fn parse_statements(sql: &str) -> Option<Vec<Statement>> {
    // Hive's dialect is the one a cluster's Hive and Spark SQL engines read a statement
    // in, and the parser is what decides whether this argument is a statement at all; the
    // generic dialect is what reads a statement the Hive one does not accept, so a
    // statement is not missed over the dialect it is spelled in.
    [
        &HiveDialect {} as &dyn Dialect,
        &GenericDialect {} as &dyn Dialect,
    ]
    .into_iter()
    .find_map(|dialect| {
        Parser::new(dialect)
            .with_recursion_limit(SQL_RECURSION_LIMIT)
            .try_with_sql(sql)
            .ok()?
            .parse_statements()
            .ok()
            .filter(|statements| !statements.is_empty())
    })
}

/// Whether running `statement` would leave nothing behind: its only effect is reading.
///
/// A query is that — with the exception of the `WITH … INSERT` / `WITH … UPDATE` forms,
/// which parse as a query whose BODY writes — and every other statement either defines,
/// drops or changes something, which is an effect this emulator does not apply.
fn reads_only(statement: &Statement) -> bool {
    let Statement::Query(query) = statement else {
        return false;
    };
    body_reads_only(&query.body)
}

/// See [`reads_only`]: whether a parsed query body only READS.
fn body_reads_only(body: &SetExpr) -> bool {
    match body {
        SetExpr::Query(inner) => body_reads_only(&inner.body),
        SetExpr::SetOperation { left, right, .. } => {
            body_reads_only(left) && body_reads_only(right)
        }
        other => !matches!(other, SetExpr::Insert(_) | SetExpr::Update(_)),
    }
}

/// The database a `USE` statement makes current, read off the parse.
///
/// The parser distinguishes the forms a dialect spells this in (`USE <name>`,
/// `USE DATABASE <name>`, `USE SCHEMA <name>`); each names the database a later
/// unqualified name resolves in. The other forms — a warehouse, a role, a catalog, the
/// dialect's default — name no database, so they move nothing.
fn database_used(statement: &Statement) -> Option<String> {
    let Statement::Use(used) = statement else {
        return None;
    };
    let named = match used {
        Use::Object(name) | Use::Database(name) | Use::Schema(name) => name,
        _ => return None,
    };
    let rendered = named.to_string();
    let last = rendered.split('.').next_back().map(unquote)?;
    (!last.is_empty()).then(|| last.to_string())
}

/// Every table `statement` names, resolved in `database` when the name carries no
/// qualifier of its own, appended to `work`.
fn collect_tables(statement: &Statement, database: &str, work: &mut StepWork) {
    let defined = cte_names(statement);
    let _: ControlFlow<()> = visit_relations(statement, |name| {
        if work.tables.len() >= MAX_TABLES_PER_STEP {
            return ControlFlow::Continue(());
        }
        if let Some(reference) = table_reference(name, database, &defined) {
            if !work.tables.contains(&reference) {
                work.tables.push(reference);
            }
        }
        ControlFlow::Continue(())
    });
}

/// The table a relation of a statement names, or `None` when it names one the statement
/// defines itself (a common table expression) or carries more qualifiers than a
/// `<catalog>.<database>.<table>` name has.
///
/// A three-part name carries the CATALOG first, and this emulator holds one catalog, so
/// the database and table are its last two parts.
fn table_reference(
    name: &ObjectName,
    database: &str,
    defined: &[String],
) -> Option<TableReference> {
    let rendered = name.to_string();
    let parts: Vec<&str> = rendered.split('.').map(unquote).collect();
    match parts.as_slice() {
        [table] => {
            let table = (*table).to_string();
            (!table.is_empty() && !defined.contains(&glue_state::fold_name(&table))).then(|| {
                TableReference {
                    database: database.to_string(),
                    name: table,
                }
            })
        }
        [qualifier, table] | [_, qualifier, table] => {
            (!qualifier.is_empty() && !table.is_empty()).then(|| TableReference {
                database: (*qualifier).to_string(),
                name: (*table).to_string(),
            })
        }
        _ => None,
    }
}

/// The names a statement's own `WITH` clause defines, folded: those name no table of the
/// metastore.
fn cte_names(statement: &Statement) -> Vec<String> {
    let Statement::Query(query) = statement else {
        return Vec::new();
    };
    query
        .with
        .as_ref()
        .map(|with| {
            with.cte_tables
                .iter()
                .map(|cte| glue_state::fold_name(&cte.alias.name.value))
                .collect()
        })
        .unwrap_or_default()
}

/// An identifier as the parse renders it, without the quoting a dialect spells it with.
fn unquote(value: &str) -> &str {
    value.trim_matches(|character| character == '`' || character == '"' || character == '\'')
}

/// Whether this cluster's Hive metastore is the AWS Glue Data Catalog, which is what
/// makes the tables a step's work names resolvable.
///
/// The cluster's configurations are what select the metastore client, so the whole
/// configuration tree is searched for [`HIVE_METASTORE_FACTORY_PROPERTY`], whatever
/// classification carries it.
pub fn uses_glue_data_catalog(cluster: &ClusterRecord) -> bool {
    configuration_property(&cluster.configurations, HIVE_METASTORE_FACTORY_PROPERTY)
        .is_some_and(|value| value.trim() == GLUE_METASTORE_FACTORY_CLASS)
}

/// The value `property` carries anywhere in a configuration tree, searched to
/// [`MAX_CONFIGURATIONS_WALKED`] structures.
///
/// A property is matched by the END of its key, because a configuration that passes a
/// Hadoop property through another engine's configuration carries it under a prefix of
/// that engine's own (`spark.hadoop.<property>`), and it is the same property.
fn configuration_property<'a>(
    configurations: &'a [Configuration],
    property: &str,
) -> Option<&'a String> {
    let mut pending: Vec<&Configuration> = configurations.iter().collect();
    let mut walked = 0usize;
    while let Some(configuration) = pending.pop() {
        walked += 1;
        if walked > MAX_CONFIGURATIONS_WALKED {
            return None;
        }
        if let Some(value) = configuration.properties.as_ref().and_then(|properties| {
            properties
                .iter()
                .find(|(key, _)| key.as_str() == property || key.ends_with(&format!(".{property}")))
                .map(|(_, value)| value)
        }) {
            return Some(value);
        }
        if let Some(children) = configuration.configurations.as_deref() {
            pending.extend(children.iter());
        }
    }
    None
}

/// Resolve what the work of every step of the cluster `cluster_id` names ANSWERS, and run
/// the cluster's step queue up to `now`.
///
/// The queue, the concurrency slots, the timeline and the stops are the cluster's own step
/// runner's ([`ClusterRecord::advance_to`]); this adds the one thing that needs state
/// beyond this service's own: what the work a step submitted answered, resolved against
/// the metastore the cluster's configuration selects ([`resolve_outcome`]). A step whose
/// answer is resolved ends in it; every other step ends as the cluster's runner ends it,
/// in the modelled failure state saying no application ran ([`STEP_NOT_EXECUTED_REASON`]).
///
/// An answer is resolved ONCE and then stored, before the step's own end comes due, so two
/// reads of a settled step report the same thing. Scoped to the ONE cluster the request
/// names, so what a request costs follows from that cluster's own steps rather than from
/// everything the account holds.
///
/// Every transition this applies was owed whether or not anybody called, so a reporting
/// operation may apply it; nothing else about a step changes because it was read.
pub fn run_due_steps<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &mut ComAmazonawsEmrElasticmapreduceState,
    cluster_id: &str,
    now: SystemTime,
) {
    let Some(cluster) = store.clusters.get_mut(cluster_id) else {
        return;
    };
    resolve_cluster_outcomes(engine, cluster);
    cluster.advance_to(now);
}

/// Resolve the answer of every step of this cluster whose work this service can account
/// for and that has none yet.
///
/// The metastore is read through the ONE Glue Data Catalog store this emulator holds.
/// LOCK ORDER: the caller holds this service's own store, and this takes the catalog's
/// guard inside it and releases it before any step is written; the Glue crate never
/// reaches back into this service's store, so the one direction is all there is and a
/// cycle between the two is unreachable.
fn resolve_cluster_outcomes<Eng: Send + Sync + 'static>(engine: &Eng, cluster: &mut ClusterRecord) {
    // A statement's tables are resolvable only when the cluster's metastore is the Glue
    // Data Catalog AND this emulator holds that catalog; on any other cluster there is
    // nothing to resolve a name in.
    if !uses_glue_data_catalog(cluster) {
        return;
    }
    let unresolved: Vec<usize> = cluster
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.outcome.is_none() && !step.is_terminal())
        .filter(|(_, step)| step.work.as_ref().is_some_and(|work| work.reads_only))
        .map(|(index, _)| index)
        .collect();
    if unresolved.is_empty() {
        return;
    }
    let Some(catalog) = glue_state::service_state(engine) else {
        return;
    };
    let resolved: Vec<(usize, StepOutcome)> = {
        let catalog = catalog.lock();
        let catalog_id = glue_state::resolve_catalog_id(None);
        unresolved
            .into_iter()
            .filter_map(|index| {
                let work = cluster.steps[index].work.as_ref()?;
                Some((index, resolve_outcome(&catalog, &catalog_id, work)))
            })
            .collect()
    };
    for (index, outcome) in resolved {
        cluster.steps[index].outcome = Some(outcome);
    }
}

/// What a read-only statement submitted to the Glue Data Catalog metastore ANSWERS.
///
/// Every table it names is resolved in the catalog: one the catalog holds nothing under
/// does not resolve, so the driver raises and the step FAILED; with every name resolved the
/// read ran and left nothing behind, which is the one outcome this service can account for
/// without applying an effect — "for a step to be considered complete, the main function
/// must exit with a zero exit code" (`AddJobFlowSteps` doc).
fn resolve_outcome(
    catalog: &glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    work: &StepWork,
) -> StepOutcome {
    match work
        .tables
        .iter()
        .find(|reference| !holds_table(catalog, catalog_id, reference))
    {
        Some(reference) => StepOutcome {
            state: step_state::FAILED.to_string(),
            failure: Some(StepFailure {
                reason: TABLE_NOT_FOUND_REASON.to_string(),
                message: table_not_found_message(reference),
            }),
        },
        None => StepOutcome {
            state: step_state::COMPLETED.to_string(),
            failure: None,
        },
    }
}

/// Whether the Glue Data Catalog holds the table `reference` names.
///
/// The catalog keys a database and a table by its folded name and folds a requested name
/// the same way, so a table is reachable by any casing a statement spells it in.
fn holds_table(
    catalog: &glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    reference: &TableReference,
) -> bool {
    catalog
        .database(catalog_id, &reference.database)
        .is_some_and(|database| {
            database
                .tables
                .contains_key(&glue_state::fold_name(&reference.name))
        })
}
