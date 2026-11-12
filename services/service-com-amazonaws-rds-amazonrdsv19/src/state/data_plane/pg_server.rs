// SPDX-License-Identifier: CC0-1.0

//! The PostgreSQL SERVER surface behind a DB instance's endpoint: the configuration
//! parameters a session reads and sets, the system catalogs it queries, and the functions
//! the server itself answers.
//!
//! None of it is a canned answer. Every value here is derived from this service's own
//! state: the parameters are the instance's DB parameter group resolved against the engine
//! defaults its family publishes, the database list is the databases the instance holds,
//! the role list is the master user the create minted plus the roles sessions have created
//! since, the activity list is the sessions this endpoint currently holds open, and the
//! start time is when the instance's engine last came up.
//!
//! The catalogs are served as REAL TABLES in the instance's own database, rebuilt from that
//! state when a statement names one, so a client's `SELECT ... FROM pg_settings WHERE name
//! = $1` is a query the engine plans and runs like any other rather than a result set
//! assembled from the statement's text. The functions are registered on the engine's
//! connection for the same reason ([`super::sql_engine`]).

use std::collections::BTreeMap;

use super::sql_engine::{EngineError, EngineErrorKind, InstanceDatabase, ResultSet};
use super::wire_session::{SessionFacts, StatementAnswer};
use crate::state::{ClusterParameterSpec, ParameterSetting};

/// The system catalogs this server serves, under the names PostgreSQL gives them.
///
/// A statement that names one has that catalog rebuilt from state before it runs
/// ([`refresh_catalogs`]); a statement that names none pays nothing.
pub const CATALOGS: [&str; 8] = [
    "pg_settings",
    "pg_database",
    "pg_roles",
    "pg_user",
    "pg_stat_activity",
    "pg_auth_members",
    "pg_namespace",
    "pg_class",
];

/// The catalog tables' definitions, with the columns PostgreSQL publishes for each and the
/// types a client decodes them under.
///
/// Source: the "System Views" and "System Catalogs" chapters of the PostgreSQL
/// documentation — `pg_settings`, `pg_database`, `pg_roles`, `pg_stat_activity`,
/// `pg_auth_members`, `pg_namespace` and `pg_class` — reduced
/// to the columns this server has state behind. A column it has no state for is left out of
/// the view rather than filled with a value nothing stands behind.
const CATALOG_SCHEMA: [&str; 8] = [
    "CREATE TABLE IF NOT EXISTS pg_settings (\
       name text PRIMARY KEY, setting text, unit text, category text, short_desc text, \
       context text, vartype text, source text, min_val text, max_val text, enumvals text, \
       boot_val text, reset_val text, pending_restart boolean)",
    "CREATE TABLE IF NOT EXISTS pg_database (\
       oid integer PRIMARY KEY, datname text UNIQUE, datdba integer, encoding integer, \
       datcollate text, datctype text, datistemplate boolean, datallowconn boolean, \
       datconnlimit integer, datacl text)",
    "CREATE TABLE IF NOT EXISTS pg_roles (\
       oid integer PRIMARY KEY, rolname text UNIQUE, rolsuper boolean, rolinherit boolean, \
       rolcreaterole boolean, rolcreatedb boolean, rolcanlogin boolean, \
       rolreplication boolean, rolconnlimit integer, rolbypassrls boolean, \
       rolvaliduntil text)",
    "CREATE TABLE IF NOT EXISTS pg_stat_activity (\
       pid integer PRIMARY KEY, datid integer, datname text, usesysid integer, \
       usename text, application_name text, client_addr text, backend_start text, \
       xact_start text, query_start text, state text, query text, backend_type text)",
    // `pg_user` is PostgreSQL's view of the roles that can LOG IN, which is why its
    // `passwd` column is the fixed `********` the view substitutes rather than anything
    // derived from a password.
    "CREATE VIEW IF NOT EXISTS pg_user AS SELECT rolname AS usename, oid AS usesysid, \
       rolcreatedb AS usecreatedb, rolsuper AS usesuper, rolreplication AS userepl, \
       rolbypassrls AS usebypassrls, '********' AS passwd, rolvaliduntil AS valuntil \
       FROM pg_roles WHERE rolcanlogin",
    // `pg_auth_members` is the role MEMBERSHIPS (`GRANT <role> TO <role>`), keyed by the
    // object identifiers of the two roles in `pg_roles`, as PostgreSQL keys them.
    "CREATE TABLE IF NOT EXISTS pg_auth_members (\
       oid integer PRIMARY KEY, roleid integer, member integer, grantor integer, \
       admin_option boolean, UNIQUE (roleid, member))",
    // The schema the objects of a database live in, and its access control list.
    "CREATE TABLE IF NOT EXISTS pg_namespace (\
       oid integer PRIMARY KEY, nspname text UNIQUE, nspowner integer, nspacl text)",
    // The relations a database holds, with the privileges granted on each.
    "CREATE TABLE IF NOT EXISTS pg_class (\
       oid integer PRIMARY KEY, relname text UNIQUE, relnamespace integer, \
       relowner integer, relkind text, relacl text)",
];

/// The encoding every database on an RDS for PostgreSQL instance is created with, as
/// `pg_database.encoding` reports it: 6, the `pg_encoding` number of `UTF8`.
const UTF8_ENCODING: i64 = 6;

/// The collation and character classification an RDS for PostgreSQL database is created
/// with, as `pg_database.datcollate` / `datctype` report them.
const DEFAULT_COLLATION: &str = "en_US.UTF-8";

/// `pg_database.datconnlimit` / `pg_roles.rolconnlimit` for a database or role with no
/// per-object connection limit, which PostgreSQL reports as -1.
const NO_CONNECTION_LIMIT: i64 = -1;

/// The first object identifier PostgreSQL hands to an object a user creates; the built-in
/// catalog rows below are numbered from here so a role a session creates gets the next one.
const FIRST_USER_OID: i64 = 16_384;

/// The predefined roles an RDS for PostgreSQL instance has beside the master user.
///
/// Source: the Amazon RDS User Guide's "Role-based access control" / "PostgreSQL roles"
/// material, which names the roles RDS creates on every instance — the superuser account
/// RDS keeps for itself, the group the master user belongs to, and the grantable roles that
/// carry the IAM, password and replication capabilities. `rolsuper` is true for exactly the
/// account RDS does not give out.
const RDS_ROLES: [(&str, bool, bool); 5] = [
    // (name, rolsuper, rolcanlogin)
    ("rdsadmin", true, true),
    ("rds_superuser", false, false),
    ("rds_replication", false, false),
    ("rds_iam", false, false),
    ("rds_password", false, false),
];

/// One configuration parameter as it stands on a RUNNING instance.
#[derive(Debug, Clone)]
pub struct RunningParameter {
    /// The parameter's name, lowercased as the engine holds it.
    pub name: String,
    /// The value the engine is RUNNING with — which is not the group's value for a static
    /// parameter whose change has not been picked up by a restart yet.
    pub running: String,
    /// `pg_settings.boot_val`: the value the engine starts with when nothing sets one.
    pub boot_value: String,
    /// `pg_settings.reset_val`: the value the parameter returns to, which is what the
    /// instance's parameter group holds for it now.
    pub reset_value: String,
    /// `pg_settings.pending_restart`: the group holds a value this engine has not picked
    /// up, and a restart is what applies it.
    pub pending_restart: bool,
    /// `pg_settings.source`: `database` for a value the instance's parameter group sets,
    /// `default` for the engine's own.
    pub source: &'static str,
    /// `pg_settings.vartype`.
    pub vartype: &'static str,
    /// `pg_settings.context`.
    pub context: &'static str,
    /// `pg_settings.min_val` / `max_val`, for a parameter published with a range.
    pub bounds: Option<(String, String)>,
    /// `pg_settings.enumvals`, in PostgreSQL's own array text form, for a parameter
    /// published with an enumeration of values.
    pub enum_values: Option<String>,
    /// `pg_settings.short_desc`.
    pub description: String,
}

/// `pg_settings.source` for a value the instance's DB parameter group sets. PostgreSQL
/// reports `database` for a setting that came from the server's per-database configuration,
/// which is where a DB parameter group's value lands.
const SOURCE_DATABASE: &str = "database";
/// `pg_settings.source` for a parameter still on the engine's own value.
const SOURCE_DEFAULT: &str = "default";

/// The engine's own compiled default for a parameter whose RDS default is an instance-class
/// FORMULA rather than a value.
///
/// Amazon RDS writes some defaults as an expression over the instance class's memory
/// (`LEAST({DBInstanceClassMemory/9531392},5000)`). This emulator models no instance-class
/// memory, so it cannot evaluate those expressions, and reporting the expression as the
/// running value would report something that is not a value at all. What a server runs with
/// when its configuration sets nothing is ITS OWN engine's compiled default, which each
/// engine's documentation publishes per parameter — PostgreSQL's `postgresql.conf` ships
/// `max_connections = 100` and `shared_buffers = 128MB` (16384 pages of 8 kB), MySQL's
/// server ships `max_connections = 151` — so the default is read from the engine the
/// parameter's FAMILY belongs to, and a parameter the named engine does not have resolves
/// to nothing.
fn engine_compiled_default(family: &str, name: &str) -> Option<&'static str> {
    let family = family.to_ascii_lowercase();
    // Which engine the family's own name says it runs — the one predicate the catalogs are
    // selected by (`describe_engine_default_parameters::is_postgres_engine`).
    let engine = crate::ops::describe_engine_default_parameters::family_engine(&family);
    let postgres = engine
        .is_some_and(crate::ops::describe_engine_default_parameters::is_postgres_engine);
    let mysql = family.contains("mysql") || family.starts_with("mariadb");
    match name {
        "max_connections" if postgres => Some("100"),
        "max_connections" if mysql => Some("151"),
        "shared_buffers" if postgres => Some("16384"),
        _ => None,
    }
}

/// The value a parameter's published default stands for: the default column of the record
/// the FAMILY publishes (already resolved by the caller through
/// [`crate::state::published_spec_for_family`]), or the engine's compiled default where the
/// published one is a formula ([`engine_compiled_default`]).
fn resolved_default(family: &str, spec: &ClusterParameterSpec) -> String {
    if spec.default_value.contains('{') || spec.default_value.contains('(') {
        if let Some(compiled) = engine_compiled_default(family, spec.name) {
            return compiled.to_string();
        }
    }
    spec.default_value.to_string()
}

/// `pg_settings.vartype` for a parameter the catalog publishes, from the data type and the
/// shape of the values it admits: an enumeration of admitted strings is PostgreSQL's `enum`
/// and a free string is `string`.
fn vartype(spec: &ClusterParameterSpec) -> &'static str {
    match spec.data_type {
        "boolean" => "bool",
        "integer" => "integer",
        "float" => "real",
        _ if spec.allowed_values.contains(',') => "enum",
        _ => "string",
    }
}

/// `pg_settings.context`: when a change to the parameter can take effect. A parameter whose
/// apply type is static needs the server restarted (`postmaster`); a dynamic one can be set
/// by a session (`user`).
fn context(spec: &ClusterParameterSpec) -> &'static str {
    if spec.apply_type == crate::state::APPLY_TYPE_STATIC {
        "postmaster"
    } else {
        "user"
    }
}

/// The parameters a running instance's engine reports, resolved from the engine defaults
/// its parameter-group family publishes and the values the instance's own group sets.
///
/// A value a group sets is in force unless applying it needs a restart the engine has not
/// had: `ModifyDBParameterGroup` documents that "changes to dynamic parameters are applied
/// immediately" while "changes to static parameters require a reboot", so a change whose
/// apply method or published apply type makes it pending counts only once the engine has
/// come up again after it was made. Until then the engine runs the previous value and the
/// parameter reports `pending_restart`.
///
/// `engine_started_at` is when this instance's engine last came up, as epoch seconds, and
/// `family` is the parameter-group family whose engine the compiled defaults come from.
pub fn running_parameters(
    family: &str,
    catalog: Option<&'static [ClusterParameterSpec]>,
    group: Option<&BTreeMap<String, ParameterSetting>>,
    engine_started_at: f64,
) -> Vec<RunningParameter> {
    let mut parameters: Vec<RunningParameter> = Vec::new();
    for spec in catalog.unwrap_or_default() {
        // The record the FAMILY publishes for this parameter, which for one the engine moved
        // between versions is the one this family's version carries: the engine runs, and
        // reports, what its own release publishes.
        let spec = &crate::state::published_spec_for_family(family, spec);
        let default = resolved_default(family, spec);
        let setting = group.and_then(|group| group.get(spec.name));
        let pending = setting.is_some_and(|setting| {
            crate::state::change_is_pending_for(Some(spec), &setting.apply_method)
                && setting.set_at.as_secs_f64() > engine_started_at
        });
        let bounds = spec
            .publishes_a_range()
            .then(|| spec.range())
            .flatten()
            .map(|(low, high)| (low.trim().to_string(), high.trim().to_string()));
        let enum_values = (vartype(spec) == "enum")
            .then(|| format!("{{{}}}", spec.allowed_values));
        parameters.push(RunningParameter {
            name: spec.name.to_string(),
            running: match setting {
                Some(setting) if !pending => setting.value.clone(),
                _ => default.clone(),
            },
            boot_value: default,
            reset_value: setting.map_or_else(
                || resolved_default(family, spec),
                |setting| setting.value.clone(),
            ),
            pending_restart: pending,
            source: if setting.is_some() && !pending {
                SOURCE_DATABASE
            } else {
                SOURCE_DEFAULT
            },
            vartype: vartype(spec),
            context: context(spec),
            bounds,
            enum_values,
            description: spec.description.to_string(),
        });
    }
    // A parameter the group sets that its family publishes no entry for is still a
    // parameter the engine runs with, so it is reported too — with the metadata the
    // catalog did not supply left out rather than guessed.
    for (name, setting) in group.into_iter().flatten() {
        if parameters.iter().any(|known| &known.name == name) {
            continue;
        }
        parameters.push(RunningParameter {
            name: name.clone(),
            running: setting.value.clone(),
            boot_value: String::new(),
            reset_value: setting.value.clone(),
            pending_restart: false,
            source: SOURCE_DATABASE,
            vartype: "string",
            context: "user",
            bounds: None,
            enum_values: None,
            description: String::new(),
        });
    }
    parameters.sort_by(|left, right| left.name.cmp(&right.name));
    parameters
}

/// Create the catalog tables if this database has not served one yet. Called once per
/// session, before any statement of it runs.
pub fn ensure_catalogs(database: &InstanceDatabase, name: &str) -> Result<(), EngineError> {
    for schema in CATALOG_SCHEMA {
        database.execute(name, schema, &[])?;
    }
    Ok(())
}

/// Rebuild the catalogs `named` from the state `facts` carries, so a statement that reads
/// one reads the instance as it is at this moment.
pub fn refresh_catalogs(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    named: &[&str],
) -> Result<(), EngineError> {
    for catalog in named {
        match *catalog {
            "pg_settings" => refresh_settings(database, facts)?,
            "pg_database" => refresh_databases(database, facts)?,
            "pg_roles" => refresh_roles(database, facts)?,
            // `pg_user` is a VIEW over the roles, so refreshing the roles is what refreshes
            // it; it carries no state of its own.
            "pg_user" => refresh_roles(database, facts)?,
            "pg_stat_activity" => refresh_activity(database, facts)?,
            // The role memberships and the object catalogs are state of their own: a
            // membership a `GRANT` recorded and a privilege it wrote belong to the instance
            // from then on, so what a refresh does is SEED the rows the instance comes with
            // (the one schema its objects live in, the relations the engine holds) and leave
            // every granted row alone.
            "pg_auth_members" => refresh_roles(database, facts)?,
            "pg_namespace" => refresh_namespaces(database, facts)?,
            "pg_class" => refresh_relations(database, facts)?,
            _ => {}
        }
    }
    Ok(())
}

/// Replace `pg_settings` with the parameters the engine is running with, plus the settings
/// the server itself answers for and the ones this SESSION has set for itself — which is
/// what a session reads back from the view.
fn refresh_settings(
    database: &InstanceDatabase,
    facts: &SessionFacts,
) -> Result<(), EngineError> {
    database.execute_in(
        facts.connection_id,
        &facts.database,
        "DELETE FROM pg_settings",
        &[],
    )?;
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut named: Vec<String> = Vec::new();
    for parameter in &facts.runtime.parameters {
        named.push(parameter.name.clone());
        let session = facts.settings.get(&parameter.name);
        rows.push(vec![
            Some(parameter.name.clone()),
            Some(session.cloned().unwrap_or_else(|| parameter.running.clone())),
            None,
            Some("Customized Options".to_string()),
            Some(parameter.description.clone()),
            Some(parameter.context.to_string()),
            Some(parameter.vartype.to_string()),
            Some(
                if session.is_some() {
                    "session"
                } else {
                    parameter.source
                }
                .to_string(),
            ),
            parameter.bounds.as_ref().map(|(low, _)| low.clone()),
            parameter.bounds.as_ref().map(|(_, high)| high.clone()),
            parameter.enum_values.clone(),
            Some(parameter.boot_value.clone()),
            Some(parameter.reset_value.clone()),
            Some(i64::from(parameter.pending_restart).to_string()),
        ]);
    }
    // The settings that are not parameter-group parameters at all: the ones the server
    // answers from the session's own socket and the instance's own identity (see
    // `wire_session::server_setting`), and anything this session has `SET` for itself.
    for name in super::wire_session::SERVER_SETTING_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .chain(facts.settings.keys().cloned())
    {
        if named.contains(&name) {
            continue;
        }
        let Some(value) = facts.setting(&name) else {
            continue;
        };
        named.push(name.clone());
        rows.push(vec![
            Some(name.clone()),
            Some(value.clone()),
            None,
            Some("Customized Options".to_string()),
            None,
            Some("user".to_string()),
            Some("string".to_string()),
            Some(
                if facts.settings.contains_key(&name) {
                    "session"
                } else {
                    SOURCE_DEFAULT
                }
                .to_string(),
            ),
            None,
            None,
            None,
            Some(value.clone()),
            Some(value),
            Some("0".to_string()),
        ]);
    }
    insert_rows(database, facts, "pg_settings", 14, &rows)
}

/// Replace `pg_database` with the databases this instance holds.
///
/// Exactly the databases a login may name — the one `DBName` created and the `postgres`
/// database RDS creates with every instance — so the catalog a client enumerates and the
/// set a connection is admitted against are the same set, and a database the instance does
/// not have is absent from both.
fn refresh_databases(
    database: &InstanceDatabase,
    facts: &SessionFacts,
) -> Result<(), EngineError> {
    // The privileges granted on a database are state of their own, so they survive the
    // rebuild of the rest of its row.
    let held = database.execute_in(
        facts.connection_id,
        &facts.database,
        "SELECT datname, datacl FROM pg_database",
        &[],
    )?;
    let granted: Vec<(String, Option<String>)> = held
        .rows
        .iter()
        .filter_map(|row| match row.first() {
            Some(super::sql_engine::Cell::Text(name)) => Some((
                name.clone(),
                match row.get(1) {
                    Some(super::sql_engine::Cell::Text(acl)) => Some(acl.clone()),
                    _ => None,
                },
            )),
            _ => None,
        })
        .collect();
    database.execute_in(
        facts.connection_id,
        &facts.database,
        "DELETE FROM pg_database",
        &[],
    )?;
    // `datdba` is "owner of the database, usually the user who created it", and PostgreSQL
    // keys it by the OBJECT IDENTIFIER of that role in `pg_roles` — the same way
    // `pg_namespace.nspowner` and `pg_class.relowner` are keyed ([`owner_oid`]) — so
    // `pg_get_userbyid(datdba)` resolves it to the role's name.
    let owner = owner_oid(facts);
    let rows: Vec<Vec<Option<String>>> = facts
        .databases
        .iter()
        .enumerate()
        .map(|(offset, name)| {
            vec![
                Some((FIRST_USER_OID + offset as i64).to_string()),
                Some(name.clone()),
                owner.clone(),
                Some(UTF8_ENCODING.to_string()),
                Some(DEFAULT_COLLATION.to_string()),
                Some(DEFAULT_COLLATION.to_string()),
                Some("0".to_string()),
                Some("1".to_string()),
                Some(NO_CONNECTION_LIMIT.to_string()),
                granted
                    .iter()
                    .find(|(held, _)| held == name)
                    .and_then(|(_, acl)| acl.clone()),
            ]
        })
        .collect();
    insert_rows(database, facts, "pg_database", 10, &rows)
}

/// The schemas a database on this server holds: the one a client's own objects live in and
/// the one the system catalogs live in, which PostgreSQL creates with every database.
///
/// This server serves unqualified names, so these two are the whole namespace of a
/// database; a schema a statement names that is not one of them is a schema the database
/// does not have.
const SCHEMAS: [&str; 2] = ["pg_catalog", "public"];

/// The first object identifier the schemas take, above the range the roles occupy.
const FIRST_NAMESPACE_OID: i64 = FIRST_USER_OID + 100;

/// The first object identifier a relation takes, above the schemas'.
const FIRST_RELATION_OID: i64 = FIRST_USER_OID + 200;

/// The object identifier of the role a database's objects belong to: the master user, who
/// owns the instance's databases.
fn owner_oid(facts: &SessionFacts) -> Option<String> {
    facts
        .runtime
        .master_username
        .as_ref()
        .map(|_| (FIRST_USER_OID + RDS_ROLES.len() as i64).to_string())
}

/// Seed `pg_namespace` with the schemas every database has ([`SCHEMAS`]).
///
/// Seeded rather than replaced: the access control list a `GRANT` wrote on a schema belongs
/// to the instance from then on, so an existing row is left exactly as it is.
fn refresh_namespaces(
    database: &InstanceDatabase,
    facts: &SessionFacts,
) -> Result<(), EngineError> {
    for (offset, name) in SCHEMAS.iter().enumerate() {
        database.execute_in(
            facts.connection_id,
            &facts.database,
            "INSERT OR IGNORE INTO pg_namespace (oid, nspname, nspowner, nspacl) \
             VALUES (?, ?, ?, NULL)",
            &[
                Some((FIRST_NAMESPACE_OID + offset as i64).to_string()),
                Some((*name).to_string()),
                owner_oid(facts),
            ],
        )?;
    }
    Ok(())
}

/// Bring `pg_class` up to date with the relations the session's database actually holds:
/// one row per table and view the engine has, in the schema its name puts it in.
///
/// The privileges granted on a relation are state of their own, so a relation that is still
/// there keeps its row — and with it its access control list — while one that has been
/// dropped loses its row, exactly as PostgreSQL drops the catalog row with the object.
fn refresh_relations(
    database: &InstanceDatabase,
    facts: &SessionFacts,
) -> Result<(), EngineError> {
    let present = database.execute_in(
        facts.connection_id,
        &facts.database,
        "SELECT name, type FROM sqlite_master WHERE type IN ('table', 'view') ORDER BY name",
        &[],
    )?;
    let relations: Vec<(String, &'static str)> = present
        .rows
        .iter()
        .filter_map(|row| match (row.first(), row.get(1)) {
            (
                Some(super::sql_engine::Cell::Text(name)),
                Some(super::sql_engine::Cell::Text(kind)),
            ) => Some((
                name.clone(),
                // `pg_class.relkind`: `r` for an ordinary table, `v` for a view.
                if kind == "view" { "v" } else { "r" },
            )),
            _ => None,
        })
        .collect();
    for (name, kind) in &relations {
        // A catalog the server keeps is a relation of the schema the catalogs live in;
        // everything else is a relation of the schema a client's objects live in.
        let namespace = if name.starts_with("pg_") || name.starts_with("sqlite_") {
            FIRST_NAMESPACE_OID
        } else {
            FIRST_NAMESPACE_OID + 1
        };
        database.execute_in(
            facts.connection_id,
            &facts.database,
            "INSERT OR IGNORE INTO pg_class \
               (oid, relname, relnamespace, relowner, relkind, relacl) \
             VALUES ((SELECT IFNULL(MAX(oid), ?) + 1 FROM pg_class), ?, ?, ?, ?, NULL)",
            &[
                Some(FIRST_RELATION_OID.to_string()),
                Some(name.clone()),
                Some(namespace.to_string()),
                owner_oid(facts),
                Some((*kind).to_string()),
            ],
        )?;
    }
    // The rows of relations the database no longer holds. A database holding NO relation at
    // all is the empty case: `NOT IN ()` is not SQL, so what that asks for is said plainly.
    let held: Vec<Option<String>> = relations
        .iter()
        .map(|(name, _)| Some(name.clone()))
        .collect();
    if held.is_empty() {
        database.execute_in(
            facts.connection_id,
            &facts.database,
            "DELETE FROM pg_class",
            &[],
        )?;
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", held.len())
        .collect::<Vec<_>>()
        .join(", ");
    database.execute_in(
        facts.connection_id,
        &facts.database,
        &format!("DELETE FROM pg_class WHERE relname NOT IN ({placeholders})"),
        &held,
    )?;
    Ok(())
}

/// Seed `pg_roles` with the roles the instance was created with: the master user and the
/// roles RDS provides ([`RDS_ROLES`]).
///
/// Seeded rather than replaced, because a role a session CREATED is engine state that
/// belongs to the instance from then on: the built-in rows are inserted only where they are
/// missing, and the rows `CREATE ROLE` added stay.
fn refresh_roles(database: &InstanceDatabase, facts: &SessionFacts) -> Result<(), EngineError> {
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    for (offset, (name, superuser, can_login)) in RDS_ROLES.iter().enumerate() {
        rows.push(role_row(
            FIRST_USER_OID + offset as i64,
            name,
            RoleAttributes {
                superuser: *superuser,
                create_role: *superuser,
                create_db: *superuser,
                can_login: *can_login,
            },
        ));
    }
    if let Some(master) = &facts.runtime.master_username {
        // The master user: "the user account you create … is not a PostgreSQL superuser",
        // which is what every privilege refusal on this endpoint rests on. It is created
        // with `CREATEROLE` and `CREATEDB`, which is how RDS withholds superuser while
        // leaving the master user able to make the roles and the databases an application
        // needs ("the master user … has the CREATEDB and CREATEROLE privileges", Amazon RDS
        // User Guide, "Master user account privileges").
        rows.push(role_row(
            FIRST_USER_OID + RDS_ROLES.len() as i64,
            master,
            RoleAttributes {
                superuser: false,
                create_role: true,
                create_db: true,
                can_login: true,
            },
        ));
    }
    for row in &rows {
        insert_rows(database, facts, "pg_roles", 11, std::slice::from_ref(row))?;
    }
    // The MEMBERSHIP the instance comes with: "the master user … is a member of the
    // rds_superuser role" (Amazon RDS User Guide, "Master user account privileges"), which
    // is how it holds the privileges RDS withholds the superuser account for. Seeded like
    // the rows above, so a membership a `GRANT` recorded since is left exactly as it is.
    if facts.runtime.master_username.is_some() {
        let group = FIRST_USER_OID
            + RDS_ROLES
                .iter()
                .position(|(name, _, _)| *name == RDS_SUPERUSER_ROLE)
                .unwrap_or_default() as i64;
        database.execute_in(
            facts.connection_id,
            &facts.database,
            "INSERT OR IGNORE INTO pg_auth_members \
               (oid, roleid, member, grantor, admin_option) \
             VALUES ((SELECT IFNULL(MAX(oid), 0) + 1 FROM pg_auth_members), ?, ?, ?, 0)",
            &[
                Some(group.to_string()),
                Some((FIRST_USER_OID + RDS_ROLES.len() as i64).to_string()),
                Some(FIRST_USER_OID.to_string()),
            ],
        )?;
    }
    Ok(())
}

/// The role the master user is a member of, which is where its privileges come from; see
/// [`RDS_ROLES`].
const RDS_SUPERUSER_ROLE: &str = "rds_superuser";

/// The attributes a role carries, as `pg_roles` reports them.
#[derive(Debug, Clone, Copy)]
struct RoleAttributes {
    superuser: bool,
    create_role: bool,
    create_db: bool,
    can_login: bool,
}

/// One `pg_roles` row, in the column order [`CATALOG_SCHEMA`] declares (which is
/// PostgreSQL's own).
fn role_row(oid: i64, name: &str, attributes: RoleAttributes) -> Vec<Option<String>> {
    let flag = |value: bool| Some(i64::from(value).to_string());
    vec![
        Some(oid.to_string()),
        Some(name.to_string()),
        flag(attributes.superuser),
        // Every role here inherits the privileges of the roles it is a member of, which is
        // the default PostgreSQL creates a role with.
        flag(true),
        flag(attributes.create_role),
        flag(attributes.create_db),
        flag(attributes.can_login),
        flag(false),
        Some(NO_CONNECTION_LIMIT.to_string()),
        flag(attributes.superuser),
        None,
    ]
}

/// Replace `pg_stat_activity` with the sessions this endpoint holds open right now, which
/// is what the endpoint's own session registry holds.
fn refresh_activity(
    database: &InstanceDatabase,
    facts: &SessionFacts,
) -> Result<(), EngineError> {
    database.execute_in(
        facts.connection_id,
        &facts.database,
        "DELETE FROM pg_stat_activity",
        &[],
    )?;
    let rows: Vec<Vec<Option<String>>> = facts
        .live_sessions()
        .into_iter()
        .map(|session| {
            vec![
                Some(session.pid.to_string()),
                Some(FIRST_USER_OID.to_string()),
                Some(session.database),
                Some((FIRST_USER_OID + RDS_ROLES.len() as i64).to_string()),
                Some(session.user),
                Some(session.application_name),
                Some(session.client_address),
                Some(session.backend_start),
                None,
                session.query_start,
                Some(session.state),
                Some(session.query),
                Some("client backend".to_string()),
            ]
        })
        .collect();
    insert_rows(database, facts, "pg_stat_activity", 13, &rows)
}

/// Insert `rows` into `table` in ONE statement, with every value bound rather than written
/// into the SQL: a parameter value or a role name a client chose never becomes SQL text.
fn insert_rows(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    table: &str,
    columns: usize,
    rows: &[Vec<Option<String>>],
) -> Result<(), EngineError> {
    if rows.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", columns)
        .collect::<Vec<_>>()
        .join(", ");
    let tuples = std::iter::repeat_n(format!("({placeholders})"), rows.len())
        .collect::<Vec<_>>()
        .join(", ");
    let values: Vec<Option<String>> = rows.iter().flatten().cloned().collect();
    database.execute_in(
        facts.connection_id,
        &facts.database,
        &format!("INSERT OR REPLACE INTO {table} VALUES {tuples}"),
        &values,
    )?;
    Ok(())
}

/// The role statements a client sends, served from this server's own role catalog.
///
/// `CREATE ROLE`, `CREATE USER` (which PostgreSQL documents as "`CREATE ROLE` … with the
/// `LOGIN` option"), `ALTER ROLE` and `DROP ROLE` change WHICH ROLES THE SERVER HAS, which
/// is server state and not a row in one of the instance's tables — so they are applied to
/// `pg_roles` here, and answered in PostgreSQL's own words for the two things that can go
/// wrong: a role that is already there and a role that is not there at all.
///
/// `None` means the statement is not a role statement and belongs to the engine.
pub fn role_statement(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    words: &[String],
    tokens: &[sqlparser::tokenizer::Token],
) -> Result<Option<StatementAnswer>, EngineError> {
    let verb = words.first().map(String::as_str).unwrap_or_default();
    let object = words.get(1).map(String::as_str).unwrap_or_default();
    if !matches!(verb, "CREATE" | "ALTER" | "DROP") || !matches!(object, "ROLE" | "USER" | "GROUP") {
        return Ok(None);
    }
    let Some(name) = role_name(tokens) else {
        return Err(EngineError::new(
            EngineErrorKind::Syntax,
            format!("syntax error at or near \"{verb} {object}\""),
        ));
    };
    refresh_roles(database, facts)?;
    let exists = !database
        .execute_in(
            facts.connection_id,
            &facts.database,
            "SELECT rolname FROM pg_roles WHERE rolname = ?",
            &[Some(name.clone())],
        )?
        .rows
        .is_empty();
    let tag = format!("{verb} {object}");
    // `IF EXISTS` on a `DROP ROLE` and `IF NOT EXISTS` on a `CREATE ROLE` make the
    // statement a no-op instead of an error when the role is not in the state the
    // statement assumes: "Do not throw an error if the role does not exist. A notice is
    // issued in this case."
    let conditional = role_option(tokens, "EXISTS");
    match verb {
        "CREATE" => {
            if exists {
                if conditional {
                    return Ok(Some(StatementAnswer::new(tag, ResultSet::default())));
                }
                return Err(EngineError::new(
                    EngineErrorKind::UniqueViolation,
                    format!("role \"{name}\" already exists"),
                ));
            }
            // `CREATE USER` is `CREATE ROLE … LOGIN`; `CREATE ROLE` is `NOLOGIN` unless the
            // statement says `LOGIN`. A role this server holds is never a superuser: the
            // one superuser on an RDS instance is the account RDS keeps ([`RDS_ROLES`]).
            let can_login = object == "USER" || role_option(tokens, "LOGIN");
            let next_oid = database
                .execute_in(
                    facts.connection_id,
                    &facts.database,
                    "SELECT IFNULL(MAX(oid), ?) + 1 FROM pg_roles",
                    &[Some(FIRST_USER_OID.to_string())],
                )?
                .rows
                .first()
                .and_then(|row| row.first().cloned())
                .map_or(FIRST_USER_OID, |cell| match cell {
                    super::sql_engine::Cell::Integer(value) => value,
                    _ => FIRST_USER_OID,
                });
            insert_rows(
                database,
                facts,
                "pg_roles",
                11,
                &[role_row(
                    next_oid,
                    &name,
                    RoleAttributes {
                        superuser: false,
                        create_role: false,
                        create_db: false,
                        can_login,
                    },
                )],
            )?;
        }
        "ALTER" => {
            if !exists {
                return Err(role_missing(&name));
            }
            if role_option(tokens, "LOGIN") || role_option(tokens, "NOLOGIN") {
                database.execute_in(
                    facts.connection_id,
                    &facts.database,
                    "UPDATE pg_roles SET rolcanlogin = ? WHERE rolname = ?",
                    &[
                        Some(i64::from(role_option(tokens, "LOGIN")).to_string()),
                        Some(name.clone()),
                    ],
                )?;
            }
        }
        _ => {
            if !exists {
                if conditional {
                    return Ok(Some(StatementAnswer::new(tag, ResultSet::default())));
                }
                return Err(role_missing(&name));
            }
            database.execute_in(
                facts.connection_id,
                &facts.database,
                "DELETE FROM pg_roles WHERE rolname = ?",
                &[Some(name.clone())],
            )?;
        }
    }
    Ok(Some(StatementAnswer::new(tag, ResultSet::default())))
}

/// What PostgreSQL tells a client that named a role the server does not have.
fn role_missing(name: &str) -> EngineError {
    EngineError::new(
        EngineErrorKind::UndefinedObject,
        format!("role \"{name}\" does not exist"),
    )
}

/// The role a role statement names: the first identifier after the object word that is not
/// part of an `IF [NOT] EXISTS` clause, quoted or not. A role name is case-SENSITIVE when it
/// is quoted and folded to lower case when it is not, exactly as PostgreSQL folds an
/// unquoted identifier.
fn role_name(tokens: &[sqlparser::tokenizer::Token]) -> Option<String> {
    use sqlparser::tokenizer::Token;
    tokens.iter().skip(2).find_map(|token| match token {
        Token::Word(word) if word.quote_style.is_some() => Some(word.value.clone()),
        Token::Word(word)
            if !matches!(
                word.value.to_ascii_uppercase().as_str(),
                "IF" | "NOT" | "EXISTS"
            ) =>
        {
            Some(word.value.to_ascii_lowercase())
        }
        Token::SingleQuotedString(value) => Some(value.clone()),
        _ => None,
    })
}

/// Whether the statement carries the role option `option` (`LOGIN`, `NOLOGIN`).
fn role_option(tokens: &[sqlparser::tokenizer::Token], option: &str) -> bool {
    use sqlparser::tokenizer::Token;
    tokens.iter().skip(2).any(|token| match token {
        Token::Word(word) => word.quote_style.is_none() && word.value.eq_ignore_ascii_case(option),
        _ => false,
    })
}

/// The PostgreSQL type each server function returns, which is the type the wire reports for
/// a column produced by calling it — PostgreSQL's own `pg_proc.prorettype`, in the type
/// names [`super::pg_wire`] maps to OIDs.
///
/// A function this does not name is not one this server defines, and a statement calling it
/// is refused by the engine as a function that does not exist.
pub fn function_result_type(function: &str) -> Option<&'static str> {
    if super::pg_numeric::is_numeric_function(function) {
        // The exact-decimal functions a cast to `numeric` and the arithmetic over it are
        // served by: the value they answer with is of that type.
        return Some("numeric");
    }
    match function {
        "pg_postmaster_start_time" => Some("timestamptz"),
        "pg_is_in_recovery" => Some("boolean"),
        "pg_backend_pid" => Some("integer"),
        "current_setting" => Some("text"),
        "md5" => Some("text"),
        // `pg_has_role` answers a question about the role catalog, which this server serves
        // as the lookup in that catalog ([`catalog_function`]) wrapped in the call itself,
        // so the column carries the boolean the function returns.
        "pg_has_role" => Some("boolean"),
        "pg_sleep" => Some("void"),
        _ => None,
    }
}

/// The CATALOG-ACCESSOR functions this server defines: a function PostgreSQL answers out of
/// one of its system catalogs, served as the lookup in that catalog that it IS.
///
/// * `pg_get_userbyid(role_oid)` is documented as returning the "role name with given OID",
///   so the statement the engine runs carries the query over `pg_roles` — the catalog this
///   server rebuilds from the instance's own roles ([`refresh_roles`]) — in place of the
///   call.
/// * `pg_has_role(user, role, privilege)` is documented as "does user have privilege for
///   role", which is a question about the MEMBERSHIPS in `pg_auth_members`: a role has every
///   privilege for itself, and otherwise it has them through being a member of that role.
///   Its two-argument form asks about the CURRENT user, which the session's own value is
///   substituted for before this runs (`super::dialect`).
///
/// Nothing is answered from the statement's text: the engine plans and runs the lookup like
/// any other subquery, and a role the instance does not have resolves to nothing, as
/// PostgreSQL's own does.
///
/// Answers the catalog the function reads, so a statement that calls it has that catalog
/// rebuilt first (`super::wire_session::catalogs_named`); [`catalog_function_lookup`] is the
/// lookup itself.
#[must_use]
pub fn catalog_function(function: &str) -> Option<&'static str> {
    match function.to_ascii_lowercase().as_str() {
        "pg_get_userbyid" => Some("pg_roles"),
        "pg_has_role" => Some("pg_auth_members"),
        _ => None,
    }
}

/// The lookup [`catalog_function`] names, over the arguments the client wrote; `None` for a
/// function that is not one, or for a call whose argument count is not one it takes — which
/// the engine then answers for as the call it is.
///
/// `session_user` is the SESSION's own user as a SQL literal, which is the user the
/// two-argument form of `pg_has_role` asks about ("the current user is assumed if
/// `user` … is omitted").
#[must_use]
pub fn catalog_function_lookup(
    function: &str,
    arguments: &[String],
    session_user: &str,
) -> Option<String> {
    match (function.to_ascii_lowercase().as_str(), arguments) {
        ("pg_get_userbyid", [oid]) => Some(format!(
            "(SELECT rolname FROM pg_roles WHERE oid = {oid})"
        )),
        // The privilege the last argument names is not read: `MEMBER` and `USAGE` are both
        // held exactly by the members of the role, which is the one membership this server
        // records.
        ("pg_has_role", [role, _privilege]) => Some(has_role_lookup(session_user, role)),
        ("pg_has_role", [user, role, _privilege]) => Some(has_role_lookup(user, role)),
        _ => None,
    }
}

/// Whether `user` has the privileges of `role`: it is that role, or it is a member of it, as
/// `pg_auth_members` records the memberships.
///
/// Wrapped in the call of `pg_has_role` itself, which the engine answers as the truth of its
/// argument: that is what makes the column PostgreSQL's own — named after the function and
/// carrying the boolean it returns ([`function_result_type`]).
fn has_role_lookup(user: &str, role: &str) -> String {
    format!(
        "pg_has_role({user} = {role} OR EXISTS (\
           SELECT 1 FROM pg_auth_members AS m \
             JOIN pg_roles AS r ON r.oid = m.roleid \
             JOIN pg_roles AS g ON g.oid = m.member \
           WHERE r.rolname = {role} AND g.rolname = {user}))"
    )
}

/// The server functions this endpoint defines, with how many arguments each takes (`-1` for
/// a variable number).
///
/// Each is served by the engine itself ([`super::sql_engine`] registers them on the
/// connection), so a statement that calls one is planned and run by the engine like any
/// other — and so a function this list does not carry is refused as undefined rather than
/// silently answered.
pub const SERVER_FUNCTIONS: [(&str, i32); 7] = [
    ("pg_postmaster_start_time", 0),
    ("pg_is_in_recovery", 0),
    ("pg_backend_pid", 0),
    ("pg_sleep", 1),
    ("current_setting", -1),
    // `md5(text)` — "Computes the MD5 hash of the argument, with the result written in
    // hexadecimal" (PostgreSQL, "String Functions and Operators"), which MySQL's `MD5()`
    // computes identically. The digest is computed over the argument's bytes by the engine's
    // own call of it.
    ("md5", 1),
    // `pg_has_role`, as the ONE argument the lookup above leaves it: the truth of the
    // membership the engine resolved out of the role catalog.
    ("pg_has_role", 1),
];

/// Whether `function` is one of [`SERVER_FUNCTIONS`].
pub fn is_server_function(function: &str) -> bool {
    SERVER_FUNCTIONS
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case(function))
}

/// A timestamp in the text form PostgreSQL renders `timestamp with time zone` in: the date
/// and time to microseconds, separated by a space, with the UTC offset the server runs at.
///
/// The calendar is the SDK timestamp type's: `aws_smithy_types::DateTime` holds the instant
/// and renders it as RFC 3339, which is the same fields in the same order — this only
/// re-spells the two separators PostgreSQL writes differently (a space instead of `T`, and
/// the offset instead of `Z`) and pads the fractional seconds to the six digits it always
/// prints.
pub fn timestamptz_text(epoch_seconds: f64) -> String {
    let at = ::aws_smithy_types::DateTime::from_secs_f64(epoch_seconds);
    let Ok(rendered) = at.fmt(::aws_smithy_types::date_time::Format::DateTime) else {
        // Only reachable for an instant outside the range the format covers at all, which
        // no clock of this emulator produces.
        return epoch_seconds.to_string();
    };
    let (date, rest) = rendered.split_once('T').unwrap_or((rendered.as_str(), ""));
    let time = rest.trim_end_matches('Z');
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let micros = format!("{fraction:0<6}");
    format!("{date} {clock}.{}+00", &micros[..6])
}
