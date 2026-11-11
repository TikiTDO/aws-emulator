// SPDX-License-Identifier: CC0-1.0

//! The SQL engine behind a DB instance's engine endpoint: one in-process SQLite
//! database per DATABASE of a DB instance.
//!
//! A statement a client sends over the engine's native wire is RUN here and the rows
//! this module returns are the rows the wire reports — nothing is synthesised from the
//! statement's text. The engine is SQLite (`rusqlite`, the `bundled` build already in
//! this workspace), which is not PostgreSQL and not MySQL: what it cannot do it refuses
//! with its own error, which the wire renders as that engine's error response, rather
//! than answering a fabricated success.
//!
//! A DATABASE (a MySQL schema, a PostgreSQL database) is a namespace of its own on both
//! engines, so each one this instance holds is its own SQLite database — the same shape
//! the RDS Data API's engine keeps (`service-com-amazonaws-rdsdata-rdsdataservice`), one
//! database per name. A name is created when the client's DDL creates it
//! ([`InstanceDatabase::create_database`]) and opened on first use for a name the instance
//! already has (the one `DBName` named, the engine's own default, a MySQL system schema),
//! up to [`MAX_DATABASES_PER_INSTANCE`].
//!
//! One connection per database, taken under a mutex for the duration of a single
//! statement:
//!
//! * every database is in memory and private to the instance, so one instance's rows never
//!   reach another's session and nothing is written to the host's disk;
//! * a TRANSACTION belongs to the SESSION that opened it rather than to the database's
//!   connection: each session's transaction keeps the database as it stood before its first
//!   statement and the rows it has changed since, so its `ROLLBACK` puts exactly those rows
//!   back ([`super::session_transactions`]), and the rows it has written or reserved are
//!   held against every other transaction until it ends ([`super::row_locks`]);
//! * every statement runs with the connection's own bounds in force (below) and under a
//!   DEFAULT-DENY authorizer ([`confine_to_the_instance`]), and the result set a session
//!   can draw out of one statement is capped in rows and in bytes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rusqlite::functions::FunctionFlags;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::ValueRef;
use rusqlite::Connection;

use super::row_locks::LockTable;
use super::session_transactions::{self, ClosedTransaction, OpenTransactions};

/// How many rows one statement may hand back to a session.
pub const MAX_RESULT_ROWS: usize = 10_000;

/// How many bytes of result values one statement may hand back to a session, charged as
/// each value is read out of the engine, so a statement that would build a larger answer
/// is refused instead of allocating it.
pub const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;

/// The wall-clock budget one statement gets. Enforced by the engine's own progress
/// handler, which runs on the thread executing the statement — a timeout on the task
/// could not interrupt a CPU-bound statement, and this can.
pub const STATEMENT_DEADLINE: Duration = Duration::from_secs(5);

/// What PostgreSQL tells a client whose statement its `statement_timeout` cancelled. The
/// engine's own sentence for the cancellation, which is what the wire reports with
/// SQLSTATE `57014`.
pub const STATEMENT_TIMEOUT_CANCELLATION: &str =
    "canceling statement due to statement timeout";

/// What this endpoint tells a client whose statement ran past [`STATEMENT_DEADLINE`] with
/// no `statement_timeout` of its own in force.
pub const ENDPOINT_DEADLINE_CANCELLATION: &str =
    "canceling statement: it ran longer than this endpoint serves";

/// How often the engine asks the progress handler whether to carry on.
const PROGRESS_HANDLER_OPS: std::os::raw::c_int = 1_000;

/// The engine's own limits, which bound what one statement can ask of it. The values
/// mirror the ones this emulator's other SQL data plane uses
/// (`service-com-amazonaws-rdsdata-rdsdataservice`), so the two cannot drift on what a
/// statement may cost.
const MAX_VALUE_BYTES: i32 = 1024 * 1024;
const MAX_SQL_BYTES: i32 = 65_536;
const MAX_EXPRESSION_DEPTH: i32 = 200;
const MAX_COMPOUND_SELECT: i32 = 20;
const MAX_BOUND_PARAMETERS: i32 = 1_000;

/// The page ceiling on each database the connection has, so the rows one instance holds
/// cannot grow the process without limit (16,384 pages of 4 KiB, 64 MiB per schema).
const MAX_DATABASE_PAGES: i64 = 16_384;

/// How many pages one step of an online backup copies; see
/// [`InstanceDatabase::load_from`]. The copy is driven a step at a time rather than in one
/// call so its deadline is checked between steps, which is the cancellation point a
/// bounded request needs.
const BACKUP_PAGES_PER_STEP: std::os::raw::c_int = 256;

/// The wall-clock budget ONE copy of a database gets: the same budget one statement gets,
/// because a copy is engine work of the same kind. Checked while waiting for each end of
/// the copy and between its steps, so a copy always gives up rather than holding either
/// database or spinning on a step that does not progress.
pub const COPY_DEADLINE: Duration = STATEMENT_DEADLINE;

/// How long a copy waits before asking again for a database another thread is using.
const COPY_LOCK_POLL: Duration = Duration::from_millis(10);

/// `PRAGMA temp_store = MEMORY`, so the engine's temporary database is in memory rather
/// than a file in the host's temp directory.
const TEMP_STORE_MEMORY: i32 = 2;

/// `PRAGMA cache_size`, in KiB (the pragma reads a negative value as a byte budget): what
/// ONE instance's connection may hold in its page cache. One emulator serves many DB
/// instances at once, each with its own connection, so the per-connection cache is set
/// well below the engine's default rather than left to multiply.
const MAX_PAGE_CACHE_KIB: i64 = -256;

/// The schemas the page ceiling is applied to: the instance's own tables and the
/// temporary database a sort or a materialised CTE spills into.
const CAPPED_SCHEMAS: [&str; 2] = ["main", "temp"];

/// How many databases one DB instance's engine holds at once.
///
/// Each one is a connection of its own with its own page ceiling, and naming a database is
/// what opens it, so a session that keeps naming fresh ones is bounded here rather than
/// growing the process: past the cap a statement that would open another is refused.
pub const MAX_DATABASES_PER_INSTANCE: usize = 64;

/// What went wrong with a statement, in terms both wires can render: each one maps a kind
/// to its own engine's code (a PostgreSQL SQLSTATE, a MySQL error number) and sends the
/// engine's real message with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineErrorKind {
    /// The statement does not parse, or names something the engine does not define.
    Syntax,
    /// A table, column or function the statement names does not exist.
    UndefinedObject,
    /// A database the statement names does not exist.
    UndefinedDatabase,
    /// A unique or primary-key constraint refused the row.
    UniqueViolation,
    /// A NOT NULL constraint refused the row.
    NotNullViolation,
    /// A CHECK constraint refused the row.
    CheckViolation,
    /// A foreign-key constraint refused the row.
    ForeignKeyViolation,
    /// A row the statement asked for is held by another transaction and the statement asked
    /// not to wait for it.
    LockNotAvailable,
    /// Waiting for a row would close a cycle of transactions each waiting for the other, so
    /// the engine ended this one.
    Deadlock,
    /// A value is not one its column accepts.
    DataError,
    /// The statement ran past [`STATEMENT_DEADLINE`].
    Cancelled,
    /// The statement asked for more than this emulator's endpoint serves: a result set
    /// past its caps, or more session state than one session may hold.
    LimitExceeded,
    /// The caller is not allowed to do this.
    InsufficientPrivilege,
    /// The statement would write and the session is on an endpoint that serves no writes:
    /// see [`super::read_only`].
    ReadOnlySqlTransaction,
    /// An object the statement would create is already there.
    DuplicateObject,
    /// The object the statement would remove is still in use, or something still depends
    /// on it.
    ObjectInUse,
    /// The engine does not serve this capability at all.
    FeatureNotSupported,
    /// The engine cannot serve this at all.
    Internal,
}

/// One failed statement: the kind of failure and the ENGINE's own message for it.
#[derive(Debug, Clone)]
pub struct EngineError {
    pub kind: EngineErrorKind,
    pub message: String,
    /// Where in the statement the engine stopped reading it, for the wire that reports a
    /// position in its own parse error.
    pub offset: Option<usize>,
    /// The SQLSTATE the PostgreSQL wire reports this refusal under, where the CONDITION
    /// names one of its own rather than the kind's ([`EngineError::with_sqlstate`]).
    pub sqlstate: Option<&'static str>,
    /// The error number and SQLSTATE the MySQL wire reports this refusal under, where the
    /// CONDITION names one of its own ([`EngineError::with_mysql_error`]).
    pub mysql_error: Option<(u16, &'static str)>,
}

impl EngineError {
    /// A failure of this emulator's own engine, with `message` as the engine worded it.
    pub fn new(kind: EngineErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            offset: None,
            sqlstate: None,
            mysql_error: None,
        }
    }

    /// The same failure, reported on the PostgreSQL wire under `sqlstate`.
    ///
    /// A statement class has its own SQLSTATE where the kind alone does not name it — a
    /// duplicate DATABASE (`42P04`) is not a duplicate object in general (`42710`) — and
    /// what a client branches on is the one the condition carries.
    #[must_use]
    pub fn with_sqlstate(mut self, sqlstate: &'static str) -> Self {
        self.sqlstate = Some(sqlstate);
        self
    }

    /// The same failure, reported on the MySQL wire under `number` and `state`: the
    /// server error number the statement class defines for this condition.
    #[must_use]
    pub fn with_mysql_error(mut self, number: u16, state: &'static str) -> Self {
        self.mysql_error = Some((number, state));
        self
    }

    /// The same failure, carrying where in the statement the engine stopped.
    #[must_use]
    pub fn at_offset(mut self, offset: Option<usize>) -> Self {
        self.offset = offset;
        self
    }
}

/// One column of a result set: the name the engine reports for it and the type its
/// declaration gave, which is what decides the type code the wire reports.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub declared_type: Option<String>,
}

/// One value in a result set, as the engine holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Cell {
    /// What this value costs against [`MAX_RESULT_BYTES`].
    fn byte_cost(&self) -> usize {
        match self {
            Self::Null => 1,
            Self::Integer(_) | Self::Real(_) => 8,
            Self::Text(text) => text.len(),
            Self::Blob(bytes) => bytes.len(),
        }
    }
}

/// What one statement produced: its columns and rows (empty for a statement that returns
/// none), how many rows it changed, and the key the engine assigned on an insert.
#[derive(Debug, Clone, Default)]
pub struct ResultSet {
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Vec<Cell>>,
    pub affected: u64,
    pub last_insert_id: i64,
}

impl ResultSet {
    /// A result set with no columns at all — what a statement that returns no rows
    /// produced, carrying only what it changed.
    pub fn changed(affected: u64, last_insert_id: i64) -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            affected,
            last_insert_id,
        }
    }

    /// A result set built here rather than by the engine: the columns and rows a SERVER
    /// command answers with (`SHOW`), which are the server's own state and not a query
    /// over the instance's tables.
    pub fn of_rows(columns: Vec<ColumnInfo>, rows: Vec<Vec<Cell>>) -> Self {
        Self {
            columns,
            rows,
            affected: 0,
            last_insert_id: 0,
        }
    }
}

/// What the server functions registered on the engine's connections answer from: the
/// instance's own runtime and the session running the statement.
///
/// Set under the engine's lock for the duration of ONE statement
/// ([`InstanceDatabase::execute_as`]), which is the only time a function can run, so a
/// function always reads the session that called it.
#[derive(Debug, Clone, Default)]
pub struct ServerView {
    /// The configuration parameters in force for this session, by lowercased name: what
    /// `current_setting` resolves against.
    pub settings: BTreeMap<String, String>,
    /// When the instance's engine last came up, as epoch seconds:
    /// `pg_postmaster_start_time()`.
    pub postmaster_start_time: f64,
    /// Whether the instance is replaying from a primary: `pg_is_in_recovery()`.
    pub in_recovery: bool,
    /// The session's own backend pid: `pg_backend_pid()`.
    pub backend_pid: u32,
    /// `statement_timeout` for this statement, in milliseconds; 0 disables it, as the
    /// parameter documents.
    pub statement_timeout_ms: u64,
    /// When the statement running now must be cancelled, which is what `pg_sleep` waits
    /// against.
    pub deadline: Option<Instant>,
}

/// How many rows ONE statement may take row locks on.
///
/// A statement over more rows than this is run with no row locks rather than with a lock
/// table the size of the table it reads: the same ceiling the result of one statement is
/// held to ([`MAX_RESULT_ROWS`]).
pub const MAX_LOCKED_ROWS: usize = MAX_RESULT_ROWS;

/// The wall-clock budget ONE rollback gets: the same budget one statement gets, because
/// putting a transaction's rows back is engine work of the same kind. Checked between the
/// rows and enforced inside each statement by the engine's own progress handler, so a
/// rollback always gives up rather than holding the engine.
pub const ROLLBACK_DEADLINE: Duration = STATEMENT_DEADLINE;

/// The size of one page of the engine's databases, which is what the budget below is
/// counted in: SQLite's own default page size.
const PAGE_BYTES: u32 = 4096;

/// How many PAGES the states this endpoint's open transactions hold may add up to.
///
/// A transaction keeps the database as it stood before its first statement ([`copy_database`])
/// so its `ROLLBACK` can put its rows back, and the sessions of one endpoint can hold many at
/// once: this is what bounds the memory they add up to, rather than leaving it to the product
/// of the per-database page ceiling, the databases per instance and the sessions a server
/// admits. 256 MiB of pages, checked before each copy.
const MAX_UNCOMMITTED_COPY_PAGES: i64 = 65_536;

/// One DB instance's engine: its databases, each with its own connection, the transactions
/// its sessions have open and the rows those hold.
pub struct InstanceDatabase {
    databases: Mutex<Databases>,
    /// Shared with the server functions registered on every one of those connections; see
    /// [`ServerView`].
    view: Arc<Mutex<ServerView>>,
    /// The transaction each session has open, and the rows it has not committed, shared
    /// with the recording hook on every one of those connections; see
    /// [`super::session_transactions`].
    transactions: Arc<OpenTransactions>,
    /// The rows those transactions hold; see [`super::row_locks`].
    locks: LockTable,
}

/// The databases one instance holds.
#[derive(Default)]
struct Databases {
    /// One connection per database name, keyed by the name as it was created.
    open: BTreeMap<String, Connection>,
    /// Whether the databases the INSTANCE comes with ([`InstanceDatabase::seed`]) have
    /// been created yet. They are created once, on the first login, after which this
    /// engine is the only thing that says which databases the instance has — so a
    /// database a client created is there for every later session, and one a client
    /// dropped stays gone.
    seeded: bool,
}

/// Written by hand rather than derived: a connection is not `Debug`, and what a reader
/// needs here is which databases the instance holds.
impl std::fmt::Debug for InstanceDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceDatabase")
            .field("databases", &self.database_names())
            .finish()
    }
}

impl InstanceDatabase {
    /// Bring the engine up for a DB instance that has no database open yet.
    ///
    /// The engine itself is started once here — on a host where it cannot start at all,
    /// the caller answers the create with a failure rather than publishing an endpoint
    /// nothing serves — and each database is opened empty when it is first named.
    pub fn open() -> Result<Self, EngineError> {
        let view = Arc::new(Mutex::new(ServerView::default()));
        probe_the_engine_starts(&view)?;
        Ok(Self {
            databases: Mutex::new(Databases::default()),
            view,
            transactions: Arc::new(OpenTransactions::default()),
            locks: LockTable::default(),
        })
    }

    /// The rows the transactions on this engine hold, which is what decides whether one
    /// session's statement can have the rows another session's transaction is holding.
    #[must_use]
    pub fn locks(&self) -> &LockTable {
        &self.locks
    }

    /// The instance's databases, recovering a lock a panicking session poisoned: a
    /// long-lived emulator has to keep serving every later session.
    fn lock(&self) -> std::sync::MutexGuard<'_, Databases> {
        self.databases.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The instance's databases, waited for only until `deadline`: `None` where another
    /// thread held them for that long.
    ///
    /// What [`Self::load_from`] takes each end of a copy with, so a copy can neither park
    /// a request on an engine that is busy nor hold one it could not finish with. A lock
    /// a panicking session poisoned is recovered exactly as [`Self::lock`] recovers it.
    fn lock_until(&self, deadline: Instant) -> Option<std::sync::MutexGuard<'_, Databases>> {
        loop {
            match self.databases.try_lock() {
                Ok(guard) => return Some(guard),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    return Some(poisoned.into_inner())
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(COPY_LOCK_POLL);
                }
            }
        }
    }

    /// Create the databases the INSTANCE itself comes with — the one `DBName` named, the
    /// engine's own default, a MySQL server's system schemas — once.
    ///
    /// Called as a session is admitted, because that is the first moment this emulator
    /// knows what the instance's record says they are. After it, the engine is the only
    /// thing that says which databases the instance has: a later call adds nothing, so a
    /// database a client dropped is not quietly created again.
    pub fn seed(&self, names: &[String]) {
        let mut databases = self.lock();
        if databases.seeded {
            return;
        }
        databases.seeded = true;
        for name in names {
            if name.is_empty() || resolve(&databases.open, name).is_some() {
                continue;
            }
            if admit_another_database(&databases).is_err() {
                return;
            }
            match open_recorded_database(&self.view, &self.transactions, name) {
                Ok(conn) => {
                    databases.open.insert(name.clone(), conn);
                }
                Err(_) => return,
            }
        }
    }

    /// Run `sql` with `params` bound to its placeholders in order against the database
    /// `database`, and answer with what the engine produced.
    ///
    /// Blocking, CPU-bound work: a wire handler calls it on a blocking task so a long
    /// statement cannot starve the async runtime, and the statement itself is bounded by
    /// [`STATEMENT_DEADLINE`] and by the result caps.
    ///
    /// This is the form the server's OWN statements use (the catalog rebuilds, the
    /// statements a server command runs for a session): it runs with no session view, so a
    /// server function in it would see none. A client's own statement goes through
    /// [`Self::execute_as`].
    pub fn execute(
        &self,
        database: &str,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<ResultSet, EngineError> {
        self.execute_as(database, &ServerView::default(), sql, params)
    }

    /// Run `sql` against `database` as the session `view` describes: the server functions
    /// it calls answer from that session, and the statement is cancelled at the earlier of
    /// this endpoint's own deadline and the session's `statement_timeout`.
    ///
    /// The cancellation is reported the way the engine that cancelled it words it — a
    /// `statement_timeout` cancellation is PostgreSQL's own sentence — because that is the
    /// text a client branches on.
    pub fn execute_as(
        &self,
        database: &str,
        view: &ServerView,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<ResultSet, EngineError> {
        self.run_for(None, database, view, sql, params)
    }

    /// Run `sql` for the SESSION `session`, exactly as [`Self::execute_as`] runs it, and
    /// record the rows it changes against that session's transaction where it has one open —
    /// so its own `ROLLBACK` puts back the rows IT wrote and no others.
    pub fn execute_for(
        &self,
        session: u32,
        database: &str,
        view: &ServerView,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<ResultSet, EngineError> {
        self.run_for(Some(session), database, view, sql, params)
    }

    /// One statement, with its row changes recorded against `session`'s transaction when it
    /// has one.
    ///
    /// The recording is turned on and off INSIDE the engine's own lock, which serialises the
    /// statements of every session on this endpoint: the rows one statement changes can
    /// therefore only be recorded against the session that ran it.
    fn run_for(
        &self,
        session: Option<u32>,
        database: &str,
        view: &ServerView,
        sql: &str,
        params: &[Option<String>],
    ) -> Result<ResultSet, EngineError> {
        let mut databases = self.lock();
        self.remember_the_state_before(&mut databases, session, database, sql)?;
        self.transactions.recording_for(session);
        let (deadline, timeout_first) = statement_deadline(view);
        {
            let mut current = self.view.lock().unwrap_or_else(PoisonError::into_inner);
            *current = view.clone();
            current.deadline = Some(deadline);
        }
        let conn = connection(&mut databases, &self.view, &self.transactions, database)?;
        conn.progress_handler(PROGRESS_HANDLER_OPS, Some(move || Instant::now() >= deadline))
            .map_err(from_engine)?;
        let answer = run_statement(conn, sql, params);
        // The bound is removed once the statement is over. The next statement installs its
        // own, so a failure to remove this one cannot leave anything running unbounded.
        let _ = conn.progress_handler(PROGRESS_HANDLER_OPS, None::<fn() -> bool>);
        self.transactions.recording_for(None);
        match answer {
            // The statement did not finish inside its budget: whatever the engine said
            // about the interruption, what happened is the cancellation, worded by
            // whichever deadline came first.
            Err(_) if Instant::now() >= deadline => Err(EngineError::new(
                EngineErrorKind::Cancelled,
                if timeout_first {
                    STATEMENT_TIMEOUT_CANCELLATION
                } else {
                    ENDPOINT_DEADLINE_CANCELLATION
                },
            )),
            other => other,
        }
    }

    /// Keep the database `database` as it stands now for `session`'s transaction, where it
    /// has one open and has not touched that database yet.
    ///
    /// The copy is the engine's own page-for-page backup ([`copy_database`]) and is taken
    /// ONCE per database per transaction — before the first statement of it that WRITES in
    /// that database, which is the state its `ROLLBACK` puts its rows back to.
    ///
    /// The copies the OPEN transactions of this endpoint hold are charged against
    /// [`MAX_UNCOMMITTED_COPY_PAGES`]: the budget is checked before each copy, so what the
    /// uncommitted state of every session together can hold is that budget plus the page
    /// ceiling of one database, and a statement that would exceed it is refused instead of
    /// growing the process.
    fn remember_the_state_before(
        &self,
        databases: &mut Databases,
        session: Option<u32>,
        database: &str,
        sql: &str,
    ) -> Result<(), EngineError> {
        let Some(session) = session else {
            return Ok(());
        };
        let key = resolve(&databases.open, database).unwrap_or_else(|| database.to_string());
        if !self.transactions.is_open(session) || self.transactions.holds_before(session, &key) {
            return Ok(());
        }
        // Only a statement that WRITES needs the state before it kept, and whether this one
        // does is the ENGINE's own answer about it (`sqlite3_stmt_readonly`): a transaction
        // that only reads copies nothing. A statement the engine cannot prepare is treated as
        // one that writes, so the copy is there if it turns out to.
        let reads_only = {
            let conn = connection(databases, &self.view, &self.transactions, &key)?;
            conn.prepare(sql)
                .map(|statement| statement.readonly())
                .unwrap_or(false)
        };
        if reads_only {
            return Ok(());
        }
        if self.transactions.charged_pages() >= MAX_UNCOMMITTED_COPY_PAGES {
            return Err(EngineError::new(
                EngineErrorKind::LimitExceeded,
                format!(
                    "this endpoint holds at most {} MiB of uncommitted state across its open \
                     transactions",
                    MAX_UNCOMMITTED_COPY_PAGES * i64::from(PAGE_BYTES) / (1024 * 1024)
                ),
            ));
        }
        let deadline = Instant::now() + COPY_DEADLINE;
        let mut copy = open_confined_database(&self.view)?;
        let conn = connection(databases, &self.view, &self.transactions, &key)?;
        let pages = copy_database(conn, &mut copy, deadline)?;
        self.transactions.keep_before(session, &key, copy, pages);
        Ok(())
    }

    /// Record the rows `rows` as changed by `session`'s transaction, where it has one open.
    ///
    /// What the statement itself knows and the engine's hook does not report: see
    /// [`session_transactions::OpenTransactions::note_rows_for`].
    pub fn note_changed_rows(&self, session: u32, database: &str, rows: &[(String, String, i64)]) {
        if rows.is_empty() || !self.transactions.is_open(session) {
            return;
        }
        self.transactions.note_rows_for(
            session,
            database,
            rows.iter()
                .map(|(_, table, rowid)| (table.clone(), *rowid)),
        );
    }

    /// Whether the SESSION `session` is inside a transaction it opened, which is what a
    /// PostgreSQL `ReadyForQuery` and a MySQL status flag report.
    #[must_use]
    pub fn in_transaction(&self, session: u32) -> bool {
        self.transactions.is_open(session)
    }

    /// Open a transaction for `session`, answering whether it had none open already.
    pub fn begin_transaction(&self, session: u32) -> bool {
        self.transactions.open(session)
    }

    /// Commit `session`'s transaction: the rows it wrote stay and the rows it held are let
    /// go. Answers whether it had one open.
    pub fn commit_transaction(&self, session: u32) -> bool {
        let committed = self.transactions.close(session).is_some();
        self.locks.release(session);
        committed
    }

    /// Roll back `session`'s transaction: every row it changed goes back to the value the
    /// database held before, and the rows it held are let go. Answers whether it had one
    /// open.
    ///
    /// The transaction ENDS either way: a rollback the engine could not finish (a recording
    /// that overflowed, or one cut short by [`ROLLBACK_DEADLINE`]) is reported as the failure
    /// it is, and the session is left out of its transaction with the rows that could not be
    /// put back still changed.
    ///
    /// Blocking, CPU-bound work bounded by [`ROLLBACK_DEADLINE`]: a caller runs it on a
    /// blocking task, as a wire handler runs a statement.
    pub fn rollback_transaction(&self, session: u32) -> Result<bool, EngineError> {
        let closed = self.transactions.close(session);
        let undone = match &closed {
            Some(closed) => self.put_the_rows_back(closed),
            None => Ok(()),
        };
        self.locks.release(session);
        undone.map(|()| closed.is_some())
    }

    /// Put one ended transaction's rows back, database by database.
    ///
    /// Bounded by [`ROLLBACK_DEADLINE`] twice over: the engine's own progress handler is
    /// installed for the duration, so no single statement of the restore can run past it,
    /// and the deadline is checked between rows. A restore the deadline cuts short is
    /// reported as the cancellation it is rather than left to run.
    ///
    /// The authorizer is lifted for the duration: putting a row back is the SERVER's own
    /// work on the session's behalf, and it needs the one thing a session may not ask for —
    /// the engine's `defer_foreign_keys`, so the constraints are checked once the whole
    /// transaction is undone rather than between its rows. No statement of the client's runs
    /// under it: what runs is built from the rows the engine itself reported changed and the
    /// values its own copy of the database holds for them. The lift is a GUARD, so the
    /// connection is confined again on every path out, an unwind included.
    fn put_the_rows_back(&self, closed: &ClosedTransaction) -> Result<(), EngineError> {
        if closed.overflowed {
            return Err(EngineError::new(
                EngineErrorKind::Internal,
                format!(
                    "this endpoint holds at most {} uncommitted rows per transaction, so this \
                     transaction could not be rolled back",
                    session_transactions::MAX_UNCOMMITTED_ROWS
                ),
            ));
        }
        let deadline = Instant::now() + ROLLBACK_DEADLINE;
        let mut databases = self.lock();
        self.transactions.recording_for(None);
        for (name, rows) in &closed.changed {
            let Some(before) = closed.before.get(name) else {
                continue;
            };
            let live = connection(&mut databases, &self.view, &self.transactions, name)?;
            let undone = {
                let mut unconfined = Unconfined::of(live);
                let live = unconfined.connection();
                live.progress_handler(
                    PROGRESS_HANDLER_OPS,
                    Some(move || Instant::now() >= deadline),
                )
                .map_err(from_engine)?;
                let undone = session_transactions::put_back(live, before, rows, deadline);
                // The bound is removed once the restore is over. The next statement on this
                // connection installs its own, so a failure to remove this one cannot leave
                // anything running unbounded.
                let _ = live.progress_handler(PROGRESS_HANDLER_OPS, None::<fn() -> bool>);
                undone
            };
            undone?;
        }
        Ok(())
    }

    /// The engine's own row ids of the rows `rows_sql` selects, for the statement `sql` the
    /// caller is about to run with `params` bound to it.
    ///
    /// The row query is the CALLER's predicate, so it is bounded exactly as the caller's own
    /// statement is: this session's view, carrying the same deadline [`Self::execute_as`]
    /// gives a statement (this endpoint's budget, clamped by the session's
    /// `statement_timeout`), is published on the engine BEFORE the scan runs, and the
    /// engine's progress handler is installed with that deadline. The publication is what
    /// bounds a server function the progress handler cannot reach into — one that waits —
    /// because such a function reads its budget from the published view; the handler bounds
    /// the scan itself. So the scan cannot hold this endpoint's engine for longer than one
    /// statement may.
    ///
    /// `None` where the rows cannot be identified: a row query the engine will not run or
    /// could not finish inside that budget, a statement whose bound parameters cannot be
    /// matched to the row query by name, or more rows than [`MAX_LOCKED_ROWS`]. The caller
    /// then takes NO row locks rather than the wrong ones.
    pub fn row_ids(
        &self,
        database: &str,
        view: &ServerView,
        rows_sql: &str,
        sql: &str,
        params: &[Option<String>],
    ) -> Option<Vec<i64>> {
        let (deadline, _) = statement_deadline(view);
        let mut databases = self.lock();
        {
            let mut current = self.view.lock().unwrap_or_else(PoisonError::into_inner);
            *current = view.clone();
            current.deadline = Some(deadline);
        }
        let conn: &mut Connection =
            connection(&mut databases, &self.view, &self.transactions, database).ok()?;
        conn.progress_handler(PROGRESS_HANDLER_OPS, Some(move || Instant::now() >= deadline))
            .ok()?;
        let ids = identified_rows(conn, rows_sql, sql, params);
        // As above: the next statement installs its own bound.
        let _ = conn.progress_handler(PROGRESS_HANDLER_OPS, None::<fn() -> bool>);
        ids
    }

    /// The columns `sql` would produce against `database`, without running it: what the
    /// extended query protocol's `Describe` answers.
    pub fn describe(&self, database: &str, sql: &str) -> Result<Vec<ColumnInfo>, EngineError> {
        let mut databases = self.lock();
        let conn = connection(&mut databases, &self.view, &self.transactions, database)?;
        let statement = conn.prepare(sql).map_err(from_engine)?;
        Ok(statement
            .columns()
            .map(|column| named_column(column.name(), column.decl_type()))
            .collect())
    }

    /// Copy every database this instance holds into a NEW engine, which is what TAKING a
    /// snapshot of a DB instance's storage is: the copy holds the databases, the schemas
    /// and the rows as they stand at this instant, and nothing written to this instance
    /// afterwards reaches it.
    ///
    /// Blocking, CPU-bound work, bounded exactly as [`Self::load_from`] is: a caller runs
    /// it on a blocking task, as a wire handler runs a statement.
    pub fn capture(&self) -> Result<Self, EngineError> {
        let copy = Self::open()?;
        copy.load_from(self)?;
        Ok(copy)
    }

    /// Replace this engine's databases with `source`'s, which is what RESTORING a snapshot
    /// into a new DB instance's storage is.
    ///
    /// This instance ends up holding exactly the databases the source holds, each with
    /// exactly its contents: a restore REPLACES what the engine had rather than merging
    /// into it. Each database is copied by the engine's own online backup
    /// ([`copy_database`]), and the databases the instance comes with are the source's, so
    /// a restored instance does not seed its own over them.
    ///
    /// Bounded and interruptible, so no request can wedge either engine: `source` must be
    /// a DIFFERENT instance and is REFUSED when it is this one, each end is taken with
    /// [`Self::lock_until`] under [`COPY_DEADLINE`] rather than waited on, and each copy is
    /// driven one step at a time against the same deadline. The number of databases is
    /// bounded by [`MAX_DATABASES_PER_INSTANCE`], which the source itself is held to.
    ///
    /// Blocking, CPU-bound work: a caller runs it on a blocking task.
    pub fn load_from(&self, source: &Self) -> Result<(), EngineError> {
        // One engine cannot be both ends of a copy: the second lock would be the one this
        // thread already holds.
        if std::ptr::eq(self, source) {
            return Err(EngineError::new(
                EngineErrorKind::Internal,
                "an instance's databases cannot be copied over themselves",
            ));
        }
        let deadline = Instant::now() + COPY_DEADLINE;
        let busy = |end: &str| {
            EngineError::new(
                EngineErrorKind::Cancelled,
                format!(
                    "the {end} of a copy was in use for longer than {} seconds",
                    COPY_DEADLINE.as_secs()
                ),
            )
        };
        // Both ends are taken under the one deadline, so two copies that each hold what
        // the other needs both give up instead of deadlocking.
        let source = source.lock_until(deadline).ok_or_else(|| busy("source"))?;
        let mut destination = self.lock_until(deadline).ok_or_else(|| busy("destination"))?;
        destination.open.clear();
        // The databases the INSTANCE comes with are the ones the source had: a restored
        // instance that has already been handed a database must not seed another over it.
        destination.seeded = source.seeded;
        for (name, held) in &source.open {
            let mut copy = open_recorded_database(&self.view, &self.transactions, name)?;
            let _pages = copy_database(held, &mut copy, deadline)?;
            destination.open.insert(name.clone(), copy);
        }
        Ok(())
    }

    /// The databases this instance has, in name order — what a login may name and what
    /// `SHOW DATABASES` lists.
    ///
    /// A database with no name is not one of them: a MySQL session opened without naming
    /// one runs its unqualified statements against a database of its own, which no client
    /// can name and no listing shows.
    #[must_use]
    pub fn database_names(&self) -> Vec<String> {
        self.lock()
            .open
            .keys()
            .filter(|name| !name.is_empty())
            .cloned()
            .collect()
    }

    /// Whether this instance holds the database `name`.
    #[must_use]
    pub fn has_database(&self, name: &str) -> bool {
        !name.is_empty() && resolve(&self.lock().open, name).is_some()
    }

    /// Create the database `name` empty, and answer whether it was created — `false`
    /// where the instance already held it, which is what makes an `IF NOT EXISTS` create a
    /// no-op rather than a second database.
    pub fn create_database(&self, name: &str) -> Result<bool, EngineError> {
        let mut databases = self.lock();
        if resolve(&databases.open, name).is_some() {
            return Ok(false);
        }
        admit_another_database(&databases)?;
        let opened = open_recorded_database(&self.view, &self.transactions, name)?;
        databases.open.insert(name.to_string(), opened);
        Ok(true)
    }

    /// Drop the database `name` with every table in it, and answer whether it was there.
    pub fn drop_database(&self, name: &str) -> bool {
        let mut databases = self.lock();
        match resolve(&databases.open, name) {
            Some(name) => databases.open.remove(&name).is_some(),
            None => false,
        }
    }

    /// Roll back the transaction every session that still holds one left open.
    ///
    /// What the end of an engine's RUN does: a reboot or a stop severs every session, and
    /// work none of them committed does not survive it. A session that ends on its own rolls
    /// back its own transaction ([`Self::rollback_transaction`]).
    pub fn rollback_open_transactions(&self) {
        for session in self.transactions.open_sessions() {
            let _ = self.rollback_transaction(session);
        }
    }
}
/// The name `name` names among `databases`, which is the one it matches ignoring case:
/// the engines resolve a database name case-insensitively on the platforms RDS runs them
/// on, so `USE APPDB` reaches the database `appdb` rather than opening a second one.
fn resolve(databases: &BTreeMap<String, Connection>, name: &str) -> Option<String> {
    databases
        .keys()
        .find(|known| known.eq_ignore_ascii_case(name))
        .cloned()
}

/// Whether one more database may be opened on this instance; see
/// [`MAX_DATABASES_PER_INSTANCE`].
fn admit_another_database(databases: &Databases) -> Result<(), EngineError> {
    if databases.open.len() >= MAX_DATABASES_PER_INSTANCE {
        return Err(EngineError::new(
            EngineErrorKind::LimitExceeded,
            format!(
                "this endpoint serves at most {MAX_DATABASES_PER_INSTANCE} databases per DB \
                 instance"
            ),
        ));
    }
    Ok(())
}

/// The connection the database `name` runs on, opened empty on first use.
///
/// A name reaches this only after the wire layer has admitted it (a login, a `USE`, the
/// DDL that created it), so opening on first use is what gives the instance's own
/// databases — the one `DBName` named, the engine's default, a MySQL system schema — a
/// database each without the engine having to be told their names when it starts.
fn connection<'a>(
    databases: &'a mut Databases,
    view: &Arc<Mutex<ServerView>>,
    transactions: &Arc<OpenTransactions>,
    name: &str,
) -> Result<&'a mut Connection, EngineError> {
    let key = resolve(&databases.open, name).unwrap_or_else(|| name.to_string());
    if !databases.open.contains_key(&key) {
        admit_another_database(databases)?;
        let opened = open_recorded_database(view, transactions, &key)?;
        databases.open.insert(key.clone(), opened);
    }
    databases
        .open
        .get_mut(&key)
        .ok_or_else(|| EngineError::new(EngineErrorKind::Internal, "the database is not open"))
}

/// One database of the instance, with the rows its connection changes reported to
/// `transactions` — which is what makes a session's `ROLLBACK` able to put its own rows
/// back.
fn open_recorded_database(
    view: &Arc<Mutex<ServerView>>,
    transactions: &Arc<OpenTransactions>,
    name: &str,
) -> Result<Connection, EngineError> {
    let mut conn = open_confined_database(view)?;
    session_transactions::record_changes_of(&mut conn, name, transactions).map_err(unavailable)?;
    Ok(conn)
}

/// Whether the engine can be started on this host at all: one database is opened under
/// the bounds above and closed again, so a DB instance whose engine cannot run is refused
/// at create time rather than publishing an endpoint nothing serves.
fn probe_the_engine_starts(view: &Arc<Mutex<ServerView>>) -> Result<(), EngineError> {
    drop(open_confined_database(view)?);
    Ok(())
}

/// When a statement the session `view` describes must be cancelled, and whether it is the
/// session's own `statement_timeout` that decides it rather than this endpoint's budget.
///
/// The ONE reading of a statement's budget: a statement the engine runs, the row query that
/// identifies the rows it locks and the wait for a row another transaction holds are bounded
/// by the same instant, so nothing a session asks for outlives the budget that session's
/// statement has. It is also the one place the session's own number is read, so the instant
/// is computed with a CHECKED addition: a timeout no clock can represent leaves this
/// endpoint's own budget in force instead of failing the thread that is running the
/// statement.
pub fn statement_deadline(view: &ServerView) -> (Instant, bool) {
    let endpoint_deadline = Instant::now() + STATEMENT_DEADLINE;
    // `statement_timeout` is a value the SESSION sets over the wire, and
    // `Instant + Duration` PANICS on overflow rather than saturating, so the addition is
    // checked: a timeout too large to represent cannot be smaller than this endpoint's
    // budget, so it loses the comparison below exactly as a representable large one does.
    let timeout = (view.statement_timeout_ms > 0)
        .then(|| Instant::now().checked_add(Duration::from_millis(view.statement_timeout_ms)))
        .flatten();
    match timeout {
        Some(timeout) if timeout <= endpoint_deadline => (timeout, true),
        _ => (endpoint_deadline, false),
    }
}

/// The row ids `rows_sql` selects, under whatever bound the caller installed on `conn`; see
/// [`InstanceDatabase::row_ids`], which is the only caller.
fn identified_rows(
    conn: &Connection,
    rows_sql: &str,
    sql: &str,
    params: &[Option<String>],
) -> Option<Vec<i64>> {
    let mut rows = conn.prepare(rows_sql).ok()?;
    if rows.parameter_count() > 0 {
        // The row query carries the statement's own placeholders, so each value is bound to
        // the placeholder of the same NAME in the statement the client sent — the positions
        // of the two differ.
        let original = conn.prepare(sql).ok()?;
        for index in 1..=rows.parameter_count() {
            let name = rows.parameter_name(index)?.to_string();
            let at = original.parameter_index(&name).ok()??;
            let value = params.get(at.checked_sub(1)?)?;
            rows.raw_bind_parameter(index, value.as_deref()).ok()?;
        }
    }
    let mut ids = Vec::new();
    let mut cursor = rows.raw_query();
    while let Some(row) = cursor.next().ok()? {
        if ids.len() >= MAX_LOCKED_ROWS {
            return None;
        }
        ids.push(row.get::<usize, i64>(0).ok()?);
    }
    Some(ids)
}

/// The connection with its authorizer lifted, which CONFINES IT AGAIN when dropped.
///
/// The one span this emulator runs a statement of its own outside the default-deny authorizer
/// is putting a rolled-back transaction's rows back, which needs the engine's
/// `defer_foreign_keys` ([`InstanceDatabase::put_the_rows_back`]). Holding the lift in a guard
/// is what makes the confinement come back on every path out of that span, an early return and
/// an unwind included — a connection left unconfined would stay so for every later session.
struct Unconfined<'a>(&'a mut Connection);

impl<'a> Unconfined<'a> {
    /// Lift the authorizer on `conn` until the guard is dropped.
    ///
    /// A lift the engine refuses leaves the connection CONFINED, which the restore below
    /// then fails on with the engine's own refusal — never running unconfined.
    fn of(conn: &'a mut Connection) -> Self {
        let _ = conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
        Self(conn)
    }

    /// The lifted connection, for the server's own work inside this span. Reached through
    /// the guard so the connection cannot be used while the lift is not held.
    fn connection(&mut self) -> &mut Connection {
        self.0
    }
}

impl Drop for Unconfined<'_> {
    fn drop(&mut self) {
        // The confinement comes back on every path out of the span. The engine refuses to
        // set an authorizer only on a connection that is no longer usable at all — every
        // later statement on it fails — and a drop has nothing to report, so the refusal is
        // not propagated here.
        let _ = self.0.authorizer(Some(confine_to_the_instance));
    }
}

/// One empty in-memory database, with every bound above in force — and with the server
/// functions this endpoint defines registered on it ([`register_server_functions`]) —
/// before any client statement can reach it.
fn open_confined_database(view: &Arc<Mutex<ServerView>>) -> Result<Connection, EngineError> {
    let mut conn = Connection::open_in_memory().map_err(unavailable)?;
    // Temp database in memory FIRST, so naming `temp` below opens it in memory and
    // not as a host file; then the page ceiling on both schemas.
    conn.pragma_update(None, "temp_store", TEMP_STORE_MEMORY)
        .map_err(unavailable)?;
    conn.pragma_update(None, "cache_size", MAX_PAGE_CACHE_KIB)
        .map_err(unavailable)?;
    for schema in CAPPED_SCHEMAS {
        conn.pragma_update(Some(schema), "max_page_count", MAX_DATABASE_PAGES)
            .map_err(unavailable)?;
    }
    for (limit, value) in [
        // No second database, ever: this also takes `VACUUM INTO` away, which the
        // engine implements by attaching its output file.
        (Limit::SQLITE_LIMIT_ATTACHED, 0),
        (Limit::SQLITE_LIMIT_LENGTH, MAX_VALUE_BYTES),
        (Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_BYTES),
        (Limit::SQLITE_LIMIT_EXPR_DEPTH, MAX_EXPRESSION_DEPTH),
        (Limit::SQLITE_LIMIT_COMPOUND_SELECT, MAX_COMPOUND_SELECT),
        (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, MAX_BOUND_PARAMETERS),
    ] {
        conn.set_limit(limit, value).map_err(unavailable)?;
    }
    for (config, enabled) in [
        (rusqlite::config::DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false),
        (rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true),
    ] {
        conn.set_db_config(config, enabled).map_err(unavailable)?;
    }
    // Installed LAST, so the settings above — themselves pragmas — are not refused
    // by it.
    conn.authorizer(Some(confine_to_the_instance))
        .map_err(unavailable)?;
    register_server_functions(&conn, view)?;
    Ok(conn)
}

/// Copy `source` over `destination` with the engine's own page-level copy.
///
/// `sqlite3_backup_*` between two OPEN connections, so no path is named and no file is
/// opened — unlike `VACUUM INTO`, which the authorizer denies. It also runs beneath SQL,
/// so it copies every table, index, view and trigger of the source exactly, with no
/// dialect or quoting of its own to get wrong.
///
/// Driven one step at a time rather than through `run_to_completion`, so `deadline` is
/// checked between steps: that is the cancellation point a bounded request needs. The
/// page ceiling is a property of the destination CONNECTION and not of the pages copied
/// into it, so the destination keeps the bound [`open_confined_database`] installed and the
/// copy cannot exceed it — the engine answers `SQLITE_FULL` if a source ever did.
///
/// Answers with how many PAGES the copy holds, which is what a caller keeping it charges
/// against its own budget.
fn copy_database(
    source: &Connection,
    destination: &mut Connection,
    deadline: Instant,
) -> Result<i64, EngineError> {
    let backup = ::rusqlite::backup::Backup::new(source, destination).map_err(unavailable)?;
    loop {
        if Instant::now() >= deadline {
            return Err(EngineError::new(
                EngineErrorKind::Cancelled,
                format!(
                    "copying a database took longer than {} seconds",
                    COPY_DEADLINE.as_secs()
                ),
            ));
        }
        match backup.step(BACKUP_PAGES_PER_STEP).map_err(unavailable)? {
            ::rusqlite::backup::StepResult::Done => {
                return Ok(i64::from(backup.progress().pagecount))
            }
            ::rusqlite::backup::StepResult::More => {}
            // Neither end is shared with anything but this copy, which holds both ends'
            // engines, so a busy or locked step is the engine reporting a state this copy
            // cannot resolve by waiting.
            unresolvable => {
                return Err(EngineError::new(
                    EngineErrorKind::Internal,
                    format!("the engine answered {unresolvable:?} while copying a database"),
                ))
            }
        }
    }
}

/// An engine that could not be brought up or configured at all.
fn unavailable(error: rusqlite::Error) -> EngineError {
    EngineError::new(EngineErrorKind::Internal, error.to_string())
}

/// How long `pg_sleep` waits between checks of its statement's deadline, so a sleep that is
/// cut short by a `statement_timeout` is cut short promptly rather than at its end.
const SLEEP_SLICE: Duration = Duration::from_millis(20);

/// Register the PostgreSQL server functions on this connection, so a statement that calls
/// one is planned and run by the engine like any other.
///
/// Each answers from the session view the statement is running under
/// ([`InstanceDatabase::execute_as`]) — the instance's own runtime and the calling session
/// — and nothing else; the catalogue of which functions exist and what each returns is
/// [`super::pg_server::SERVER_FUNCTIONS`].
fn register_server_functions(
    conn: &Connection,
    view: &Arc<Mutex<ServerView>>,
) -> Result<(), EngineError> {
    let read = |view: &Arc<Mutex<ServerView>>| -> ServerView {
        view.lock().unwrap_or_else(PoisonError::into_inner).clone()
    };

    let start_time = Arc::clone(view);
    conn.create_scalar_function(
        "pg_postmaster_start_time",
        0,
        FunctionFlags::SQLITE_UTF8,
        move |_| Ok(super::pg_server::timestamptz_text(read(&start_time).postmaster_start_time)),
    )
    .map_err(unavailable)?;

    let recovery = Arc::clone(view);
    conn.create_scalar_function(
        "pg_is_in_recovery",
        0,
        FunctionFlags::SQLITE_UTF8,
        move |_| Ok(i64::from(read(&recovery).in_recovery)),
    )
    .map_err(unavailable)?;

    let backend = Arc::clone(view);
    conn.create_scalar_function("pg_backend_pid", 0, FunctionFlags::SQLITE_UTF8, move |_| {
        Ok(i64::from(read(&backend).backend_pid))
    })
    .map_err(unavailable)?;

    // `pg_sleep(seconds)` — "Makes the current session's process sleep until the given
    // number of seconds have elapsed". It really waits, and it stops waiting when the
    // statement's deadline passes, which is what makes a `statement_timeout` observable
    // through it. The wait is bounded three times over: by the deadline the statement runs
    // under, by this endpoint's own budget from the moment the wait starts — which is the
    // bound that holds when no statement has published a deadline on this connection yet —
    // and by an hour for the argument itself, so the connection this holds, and the
    // instance's engine lock with it, cannot be held by a caller's own number.
    let sleeping = Arc::clone(view);
    conn.create_scalar_function("pg_sleep", 1, FunctionFlags::SQLITE_UTF8, move |context| {
        let seconds = context.get::<f64>(0).unwrap_or(0.0).max(0.0);
        let view = read(&sleeping);
        let entered = Instant::now();
        let endpoint_ceiling = entered + STATEMENT_DEADLINE;
        let ceiling = view
            .deadline
            .map_or(endpoint_ceiling, |deadline| deadline.min(endpoint_ceiling));
        let until = entered
            .checked_add(Duration::from_secs_f64(seconds.min(3_600.0)))
            .unwrap_or(ceiling);
        loop {
            let now = Instant::now();
            if now >= until {
                return Ok(Option::<String>::None);
            }
            if now >= ceiling {
                return Err(rusqlite::Error::UserFunctionError(
                    STATEMENT_TIMEOUT_CANCELLATION.into(),
                ));
            }
            std::thread::sleep(SLEEP_SLICE.min(until.saturating_duration_since(now)));
        }
    })
    .map_err(unavailable)?;

    // `current_setting(name [, missing_ok])` — "Returns the current value of the setting",
    // erroring on an unrecognised name unless the second argument asks for NULL instead.
    let settings = Arc::clone(view);
    conn.create_scalar_function(
        "current_setting",
        -1,
        FunctionFlags::SQLITE_UTF8,
        move |context| {
            let name = context.get::<String>(0).unwrap_or_default();
            let missing_ok = context.len() > 1 && context.get::<i64>(1).unwrap_or(0) != 0;
            match read(&settings).settings.get(&name.to_ascii_lowercase()) {
                Some(value) => Ok(Some(value.clone())),
                None if missing_ok => Ok(None),
                None => Err(rusqlite::Error::UserFunctionError(
                    format!("unrecognized configuration parameter \"{name}\"").into(),
                )),
            }
        },
    )
    .map_err(unavailable)?;
    Ok(())
}

/// Prepare, bind and run one statement, reading its rows out under the result caps.
fn run_statement(
    conn: &Connection,
    sql: &str,
    params: &[Option<String>],
) -> Result<ResultSet, EngineError> {
    let mut statement = conn.prepare(sql).map_err(from_engine)?;
    if statement.parameter_count() != params.len() {
        return Err(EngineError::new(
            EngineErrorKind::Syntax,
            format!(
                "bind message supplies {} parameters, but the statement requires {}",
                params.len(),
                statement.parameter_count()
            ),
        ));
    }
    for (position, value) in params.iter().enumerate() {
        statement
            .raw_bind_parameter(position + 1, value.as_deref())
            .map_err(from_engine)?;
    }
    let columns: Vec<ColumnInfo> = statement
        .columns()
        .map(|column| named_column(column.name(), column.decl_type()))
        .collect();
    if columns.is_empty() {
        let affected = statement.raw_execute().map_err(from_engine)?;
        return Ok(ResultSet::changed(
            u64::try_from(affected).unwrap_or(u64::MAX),
            conn.last_insert_rowid(),
        ));
    }

    let mut rows = Vec::new();
    let mut bytes = 0usize;
    let mut cursor = statement.raw_query();
    while let Some(row) = cursor.next().map_err(from_engine)? {
        if rows.len() >= MAX_RESULT_ROWS {
            return Err(EngineError::new(
                EngineErrorKind::LimitExceeded,
                format!("the result set is larger than this emulator serves ({MAX_RESULT_ROWS} rows)"),
            ));
        }
        let mut values = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            let cell = match row.get_ref(index).map_err(from_engine)? {
                ValueRef::Null => Cell::Null,
                ValueRef::Integer(value) => Cell::Integer(value),
                ValueRef::Real(value) => Cell::Real(value),
                ValueRef::Text(bytes) => {
                    Cell::Text(String::from_utf8_lossy(bytes).into_owned())
                }
                ValueRef::Blob(bytes) => Cell::Blob(bytes.to_vec()),
            };
            bytes = bytes.saturating_add(cell.byte_cost());
            if bytes > MAX_RESULT_BYTES {
                return Err(EngineError::new(
                    EngineErrorKind::LimitExceeded,
                    format!(
                        "the result set is larger than this emulator serves ({} MiB of values)",
                        MAX_RESULT_BYTES / (1024 * 1024)
                    ),
                ));
            }
            values.push(cell);
        }
        rows.push(values);
    }
    // `changes()` and the assigned key belong to a statement that wrote rows — an
    // `INSERT ... RETURNING` both returns rows and changes them.
    Ok(ResultSet {
        columns,
        rows,
        affected: conn.changes(),
        last_insert_id: conn.last_insert_rowid(),
    })
}

/// One column as the wire reports it.
///
/// A column the engine produced from a TABLE carries the type its declaration names. A
/// column that is a call of one of this server's own functions carries no declaration, so
/// it takes the two facts PostgreSQL reports for it: the column is named after the function
/// and carries that function's return type
/// ([`super::pg_server::function_result_type`]) — which is what lets a client decode
/// `pg_is_in_recovery()` as a boolean and `pg_postmaster_start_time()` as a timestamp.
fn named_column(name: &str, declared: Option<&str>) -> ColumnInfo {
    if declared.is_none() {
        if let Some(function) = name.strip_suffix(')').and_then(|call| call.split('(').next()) {
            if let Some(result) = super::pg_server::function_result_type(function) {
                return ColumnInfo {
                    name: function.to_string(),
                    declared_type: Some(result.to_string()),
                };
            }
        }
    }
    ColumnInfo {
        name: name.to_string(),
        declared_type: declared.map(str::to_string),
    }
}

/// Classify what the engine refused, keeping ITS message.
fn from_engine(error: rusqlite::Error) -> EngineError {
    // A statement the engine could not READ at all arrives as its own variant, carrying
    // the engine's bare message and where in the statement it stopped — which is what a
    // wire needs to word its own parse error. Classified by the failure inside it, so it
    // is not reported as an engine that broke.
    if let rusqlite::Error::SqlInputError {
        error: failure,
        msg,
        offset,
        ..
    } = &error
    {
        return EngineError::new(failure_kind(failure), msg.clone())
            .at_offset(usize::try_from(*offset).ok());
    }
    let message = engine_message(&error);
    let kind = match &error {
        rusqlite::Error::SqliteFailure(failure, _) => failure_kind(failure),
        rusqlite::Error::InvalidParameterCount(..) | rusqlite::Error::InvalidQuery => {
            EngineErrorKind::Syntax
        }
        _ => EngineErrorKind::Internal,
    };
    // SQLite words a missing table or column as "no such <thing>", which is the one
    // failure class every client branches on separately. A configuration parameter the
    // server does not have is the same class: PostgreSQL answers it as an undefined
    // object, and the sentence comes from `current_setting` above.
    let kind = if message.contains("no such table")
        || message.contains("no such column")
        || message.contains("no such function")
        || message.contains("no such index")
        || message.contains("no such view")
        || message.starts_with("unrecognized configuration parameter")
    {
        EngineErrorKind::UndefinedObject
    } else if message == STATEMENT_TIMEOUT_CANCELLATION {
        EngineErrorKind::Cancelled
    } else {
        kind
    };
    EngineError::new(kind, message)
}

/// What one engine failure code means, in terms both wires can render.
fn failure_kind(failure: &rusqlite::ffi::Error) -> EngineErrorKind {
    match failure.code {
        rusqlite::ErrorCode::ConstraintViolation => constraint_kind(failure.extended_code),
        rusqlite::ErrorCode::OperationInterrupted => EngineErrorKind::Cancelled,
        rusqlite::ErrorCode::AuthorizationForStatementDenied => {
            EngineErrorKind::InsufficientPrivilege
        }
        rusqlite::ErrorCode::TypeMismatch => EngineErrorKind::DataError,
        rusqlite::ErrorCode::Unknown => EngineErrorKind::Syntax,
        _ => EngineErrorKind::Internal,
    }
}

/// Which constraint the engine refused the row for, from its extended result code.
fn constraint_kind(extended_code: std::os::raw::c_int) -> EngineErrorKind {
    // The extended codes SQLite documents for SQLITE_CONSTRAINT: the primary result code
    // (19) in the low byte and the constraint in the high byte.
    const SQLITE_CONSTRAINT_CHECK: std::os::raw::c_int = 275;
    const SQLITE_CONSTRAINT_FOREIGNKEY: std::os::raw::c_int = 787;
    const SQLITE_CONSTRAINT_NOTNULL: std::os::raw::c_int = 1299;
    const SQLITE_CONSTRAINT_PRIMARYKEY: std::os::raw::c_int = 1555;
    const SQLITE_CONSTRAINT_UNIQUE: std::os::raw::c_int = 2067;
    match extended_code {
        SQLITE_CONSTRAINT_CHECK => EngineErrorKind::CheckViolation,
        SQLITE_CONSTRAINT_FOREIGNKEY => EngineErrorKind::ForeignKeyViolation,
        SQLITE_CONSTRAINT_NOTNULL => EngineErrorKind::NotNullViolation,
        SQLITE_CONSTRAINT_PRIMARYKEY | SQLITE_CONSTRAINT_UNIQUE => {
            EngineErrorKind::UniqueViolation
        }
        _ => EngineErrorKind::CheckViolation,
    }
}

/// The engine's own sentence for a failure, which is what the client is told.
fn engine_message(error: &rusqlite::Error) -> String {
    match error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => message.clone(),
        other => other.to_string(),
    }
}

/// What a client's SQL may do inside one DB instance's database.
///
/// DEFAULT DENY: an action not named here is refused, so an engine capability this
/// emulator has not considered — including one a later SQLite build adds, which arrives as
/// a variant this match has never seen — cannot be reached from a session. What is allowed
/// is what the data plane exists for: reading and writing the instance's own tables, the
/// DDL that defines them, transactions, and pure computation over the values in them.
///
/// What that refuses, and why it matters: `ATTACH`/`DETACH` would make a host path visible
/// to SQL (and `VACUUM INTO` is implemented by attaching its output file), `PRAGMA` reaches
/// engine configuration including temp-file paths, and a virtual table is arbitrary
/// registered code. None of them is something a PostgreSQL or MySQL client can ask for over
/// its own wire anyway, so refusing them takes nothing away from the engine this plane
/// emulates. The functions are an allow-list of their own
/// ([`::service_common::sqlite_confinement`]), which is where the host-reaching ones are
/// named absent.
fn confine_to_the_instance(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Select
        | AuthAction::Read { .. }
        | AuthAction::Insert { .. }
        | AuthAction::Update { .. }
        | AuthAction::Delete { .. }
        | AuthAction::Transaction { .. }
        | AuthAction::Savepoint { .. }
        | AuthAction::Recursive
        | AuthAction::CreateTable { .. }
        | AuthAction::CreateTempTable { .. }
        | AuthAction::CreateIndex { .. }
        | AuthAction::CreateTempIndex { .. }
        | AuthAction::CreateView { .. }
        | AuthAction::CreateTempView { .. }
        | AuthAction::CreateTrigger { .. }
        | AuthAction::CreateTempTrigger { .. }
        | AuthAction::DropTable { .. }
        | AuthAction::DropTempTable { .. }
        | AuthAction::DropIndex { .. }
        | AuthAction::DropTempIndex { .. }
        | AuthAction::DropView { .. }
        | AuthAction::DropTempView { .. }
        | AuthAction::DropTrigger { .. }
        | AuthAction::DropTempTrigger { .. }
        | AuthAction::AlterTable { .. }
        | AuthAction::Reindex { .. }
        | AuthAction::Analyze { .. } => Authorization::Allow,
        // Pure computation over the values in those tables, as an ALLOW-LIST: a function
        // this emulator has not reviewed — including one a future engine build defines —
        // falls through to the deny arm below.
        AuthAction::Function { function_name } => {
            if ::service_common::sqlite_confinement::sql_function_is_permitted(function_name)
                || super::pg_server::is_server_function(function_name)
            {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }
        // Everything that could reach outside this instance's own database, and everything
        // this emulator has not considered.
        _ => Authorization::Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session view asking for `statement_timeout_ms` and nothing else.
    fn asking_for(statement_timeout_ms: u64) -> ServerView {
        ServerView {
            statement_timeout_ms,
            ..ServerView::default()
        }
    }

    /// RULE: the statement's deadline is the session's `statement_timeout` where that is
    /// shorter than this endpoint's own budget, and the endpoint's budget otherwise — for
    /// EVERY number a session can put on the wire, the ones no clock can represent included.
    #[test]
    fn a_statement_deadline_is_never_past_the_endpoint_budget_whatever_the_session_asks() {
        // (case, the session's statement_timeout_ms, whether the session's own value decides)
        let rows: &[(&str, u64, bool)] = &[
            ("no timeout at all, so the endpoint's budget decides", 0, false),
            ("one millisecond, far inside the endpoint's budget", 1, true),
            (
                "a value comfortably inside the endpoint's budget",
                STATEMENT_DEADLINE.as_millis() as u64 / 2,
                true,
            ),
            (
                "a value past the endpoint's budget",
                STATEMENT_DEADLINE.as_millis() as u64 * 10,
                false,
            ),
            (
                // PostgreSQL's own ceiling for this parameter: `statement_timeout` is an
                // integer number of milliseconds, so this is the largest value a session
                // can set ("statement_timeout (integer)", client-connection defaults).
                "the largest value the parameter admits",
                2_147_483_647,
                false,
            ),
            (
                "a value past what the parameter admits",
                2_147_483_648,
                false,
            ),
            ("the largest number the wire can carry", u64::MAX, false),
        ];
        for (case, asked, session_decides) in rows {
            let (deadline, timeout_first) = statement_deadline(&asking_for(*asked));
            // Read AFTER the call, so the comparison needs no slack: the deadline was
            // computed from an earlier instant than this one.
            let ceiling = Instant::now() + STATEMENT_DEADLINE;
            assert_eq!(
                timeout_first, *session_decides,
                "case {case:?}: expected the session's timeout to decide == {session_decides}, \
                 got {timeout_first}"
            );
            assert!(
                deadline <= ceiling,
                "case {case:?}: expected a deadline inside the endpoint's budget of \
                 {STATEMENT_DEADLINE:?}, got one {:?} past it",
                deadline.saturating_duration_since(ceiling)
            );
        }
    }
}
