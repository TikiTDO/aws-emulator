// SPDX-License-Identifier: CC0-1.0

//! The DB instance log `DescribeDBLogFiles` lists and `DownloadDBLogFilePortion`
//! serves.
//!
//! No database engine runs behind this emulator, so there is no engine log to copy. What
//! this module holds instead is the log THIS SERVICE keeps for a DB instance: one line per
//! lifecycle event it actually performed on that instance (the create it accepted, each
//! status the clock then made due), timestamped when it happened. That is real, stored
//! state — a line appears only because the service did the thing it records — and it is
//! what the two log operations report: the file's size is the size of those lines, and its
//! last-written time is the instant of the last one.
//!
//! The log lives ON the DB instance record, so it is removed with the instance and is
//! bounded by [`MAX_LOG_ENTRIES`] lines of at most [`MAX_LOG_LINE_LEN`] characters each.

/// How many lines one instance's log keeps. A DB instance's lifecycle produces a line per
/// accepted change, and a caller can drive those without limit, so the log is a ring: past
/// this many lines the oldest goes, exactly as a real engine log rotates.
pub const MAX_LOG_ENTRIES: usize = 64;

/// The longest log line this service writes. Every line is built from a status or an
/// identifier this service already bounds, so this is a backstop rather than a filter.
pub const MAX_LOG_LINE_LEN: usize = 256;

/// One line of a DB instance's log: when this service recorded it, and what it recorded.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    /// When the event this line records happened.
    #[serde(with = "::service_common::persist::smithy_time")]
    pub at: ::aws_smithy_types::DateTime,
    /// What happened, as the line reads after the timestamp.
    pub text: String,
}

/// A DB instance's log: the lifecycle lines this service has recorded for it, oldest
/// first.
#[derive(Debug, Clone, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct InstanceLog {
    /// The lines, oldest first, at most [`MAX_LOG_ENTRIES`] of them.
    pub entries: Vec<LogEntry>,
}

impl InstanceLog {
    /// Record one line, timestamped now, dropping the oldest line when the log is full.
    pub fn record(&mut self, text: impl Into<String>) {
        let mut text: String = text.into();
        text.truncate(
            text.char_indices()
                .nth(MAX_LOG_LINE_LEN)
                .map_or(text.len(), |(offset, _)| offset),
        );
        self.entries.push(LogEntry {
            at: super::now_as_wire_timestamp(),
            text,
        });
        if self.entries.len() > MAX_LOG_ENTRIES {
            self.entries.remove(0);
        }
    }

    /// The file's lines as a client downloading it reads them: the timestamp of each
    /// event, then what was recorded.
    pub fn lines(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| {
                let at = entry
                    .at
                    .fmt(::aws_smithy_types::date_time::Format::DateTime)
                    .unwrap_or_else(|_| entry.at.as_secs_f64().to_string());
                format!("{at} {}", entry.text)
            })
            .collect()
    }

    /// `DescribeDBLogFilesDetails$Size`: "The size, in bytes, of the log file" — the size
    /// of the lines above, each with its newline, so it cannot disagree with what a
    /// download returns.
    pub fn size(&self) -> i64 {
        let bytes: usize = self
            .lines()
            .iter()
            .map(|line| line.len() + "\n".len())
            .sum();
        i64::try_from(bytes).unwrap_or(i64::MAX)
    }

    /// `DescribeDBLogFilesDetails$LastWritten`: "A POSIX timestamp when the last log entry
    /// was written", which Amazon RDS reports in MILLISECONDS, or `None` for a log with no
    /// lines yet.
    pub fn last_written(&self) -> Option<i64> {
        self.entries
            .last()
            .map(|entry| (entry.at.as_secs_f64() * 1_000.0) as i64)
    }
}

/// The name of the log file a DB instance running `engine` writes.
///
/// Source: the per-engine log files Amazon RDS publishes in "Monitoring Amazon RDS log
/// files" (Amazon RDS User Guide) and lists through `DescribeDBLogFiles` — the MySQL and
/// MariaDB error log `error/mysql-error.log`, the PostgreSQL one under `error/`, the SQL
/// Server `log/ERRORLOG`, the Oracle alert log under `trace/` and the Db2 `db2diag.log`.
/// An engine this table does not carry gets the neutral name below rather than another
/// engine's: the file is this service's own log either way (see the module docs).
pub fn log_file_name(engine: &str) -> &'static str {
    /// The published names, matched on the prefix of the `Engine` value, longest-matching
    /// family first.
    const PUBLISHED: &[(&str, &str)] = &[
        ("aurora-mysql", "error/mysql-error.log"),
        ("aurora-postgresql", "error/postgresql.log"),
        ("mariadb", "error/mysql-error.log"),
        ("mysql", "error/mysql-error.log"),
        ("postgres", "error/postgresql.log"),
        ("custom-sqlserver", "log/ERRORLOG"),
        ("sqlserver", "log/ERRORLOG"),
        ("custom-oracle", "trace/alert.log"),
        ("oracle", "trace/alert.log"),
        ("db2", "db2diag.log"),
    ];

    PUBLISHED
        .iter()
        .find(|(family, _)| engine.starts_with(family))
        .map_or("error/engine.log", |(_, name)| *name)
}
