// SPDX-License-Identifier: CC0-1.0

//! The COMMANDS a cache answers, and what each one does to its keyspace.
//!
//! One catalog ([`CATALOG`]) carries, per command, the shape the engine documents for it:
//! how many arguments it takes, whether it WRITES, which of its arguments are KEYS, and
//! which ACL categories it belongs to — which is what lets the session layer refuse a
//! command on a read-only replica, refuse a key an RBAC user may not reach, and refuse a
//! command its access string denies, from the command's own record rather than from a list
//! repeated per check.
//!
//! The semantics each command is answered with are the engine's own, over the keyspace
//! [`super::keyspace`] holds: nothing here is synthesised from the request, and a command
//! this emulator does not serve is answered with the engine's own `unknown command` error
//! rather than a success it did not perform.
//!
//! # Bounds
//!
//! Every command's work is bounded by the keyspace it runs over, which is itself bounded
//! ([`super::keyspace::MAX_KEYS`], [`super::keyspace::MAX_KEYSPACE_BYTES`]): the commands
//! that walk the whole keyspace (`KEYS`, `SCAN`, `DBSIZE`) walk at most that many keys, and
//! the ones that take a list of keys (`MGET`, `MSET`, `DEL`) take at most what one command
//! may carry ([`super::resp_frame::MAX_COMMAND_BYTES`]). `SCAN` answers at most
//! [`SCAN_PAGE`] keys per call.

use std::time::{Duration, SystemTime};

use super::keyspace::{Keyspace, Limits, WriteRefusal};
use super::resp_frame::Reply;

/// How many keys one `SCAN` answers with, which is the cursor's step.
pub const SCAN_PAGE: usize = 100;

/// What one command's record says about it.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    /// The command's name, lowercase, as a client sends it (case-insensitively).
    pub name: &'static str,
    /// How many arguments the command takes INCLUDING its own name: an exact count, or a
    /// negative number whose absolute value is the minimum — the engine's own `arity`.
    pub arity: i32,
    /// Whether the command changes the keyspace, which is what a read-only replica refuses
    /// and what the `@write` category names.
    pub write: bool,
    /// Where this command's keys are among its arguments: `(first, last, step)`, with
    /// `last` negative counting from the end (`-1` being the last argument). `None` for a
    /// command that names no key.
    pub keys: Option<(usize, i32, usize)>,
    /// The ACL categories this command belongs to, as the engine's own `ACL CAT` groups
    /// it: what a `+@category` / `-@category` rule in an access string matches.
    pub categories: &'static [&'static str],
}

/// Every command this data plane serves.
///
/// The arity, key positions and categories of each are the engine's own for that command
/// (its `COMMAND INFO` record); the set is the string, keyspace, connection and server
/// surface a cache's clients use, which is what this emulator's keyspace can answer
/// faithfully. A command outside it is answered `unknown command` rather than guessed at.
pub const CATALOG: &[CommandSpec] = &[
    // Connection and server.
    CommandSpec { name: "ping", arity: -1, write: false, keys: None, categories: &["fast", "connection"] },
    CommandSpec { name: "echo", arity: 2, write: false, keys: None, categories: &["fast", "connection"] },
    CommandSpec { name: "auth", arity: -2, write: false, keys: None, categories: &["fast", "connection"] },
    CommandSpec { name: "hello", arity: -1, write: false, keys: None, categories: &["fast", "connection"] },
    CommandSpec { name: "quit", arity: -1, write: false, keys: None, categories: &["fast", "connection"] },
    CommandSpec { name: "reset", arity: 1, write: false, keys: None, categories: &["fast", "connection"] },
    CommandSpec { name: "select", arity: 2, write: false, keys: None, categories: &["fast", "connection"] },
    CommandSpec { name: "client", arity: -2, write: false, keys: None, categories: &["slow", "connection"] },
    CommandSpec { name: "command", arity: -1, write: false, keys: None, categories: &["slow", "connection"] },
    CommandSpec { name: "info", arity: -1, write: false, keys: None, categories: &["slow", "dangerous"] },
    CommandSpec { name: "config", arity: -2, write: false, keys: None, categories: &["admin", "slow", "dangerous"] },
    CommandSpec { name: "cluster", arity: -2, write: false, keys: None, categories: &["slow"] },
    CommandSpec { name: "dbsize", arity: 1, write: false, keys: None, categories: &["keyspace", "read", "fast"] },
    CommandSpec { name: "flushall", arity: -1, write: true, keys: None, categories: &["keyspace", "write", "slow", "dangerous"] },
    CommandSpec { name: "flushdb", arity: -1, write: true, keys: None, categories: &["keyspace", "write", "slow", "dangerous"] },
    CommandSpec { name: "keys", arity: 2, write: false, keys: None, categories: &["keyspace", "read", "slow", "dangerous"] },
    CommandSpec { name: "scan", arity: -2, write: false, keys: None, categories: &["keyspace", "read", "slow"] },
    CommandSpec { name: "randomkey", arity: 1, write: false, keys: None, categories: &["keyspace", "read", "slow"] },
    // Strings.
    CommandSpec { name: "get", arity: 2, write: false, keys: Some((1, 1, 1)), categories: &["read", "string", "fast"] },
    CommandSpec { name: "set", arity: -3, write: true, keys: Some((1, 1, 1)), categories: &["write", "string", "slow"] },
    CommandSpec { name: "setnx", arity: 3, write: true, keys: Some((1, 1, 1)), categories: &["write", "string", "fast"] },
    CommandSpec { name: "setex", arity: 4, write: true, keys: Some((1, 1, 1)), categories: &["write", "string", "slow"] },
    CommandSpec { name: "psetex", arity: 4, write: true, keys: Some((1, 1, 1)), categories: &["write", "string", "slow"] },
    CommandSpec { name: "getset", arity: 3, write: true, keys: Some((1, 1, 1)), categories: &["write", "string", "fast"] },
    CommandSpec { name: "getdel", arity: 2, write: true, keys: Some((1, 1, 1)), categories: &["write", "string", "fast"] },
    CommandSpec { name: "append", arity: 3, write: true, keys: Some((1, 1, 1)), categories: &["write", "string", "fast"] },
    CommandSpec { name: "strlen", arity: 2, write: false, keys: Some((1, 1, 1)), categories: &["read", "string", "fast"] },
    CommandSpec { name: "mget", arity: -2, write: false, keys: Some((1, -1, 1)), categories: &["read", "string", "fast"] },
    CommandSpec { name: "mset", arity: -3, write: true, keys: Some((1, -1, 2)), categories: &["write", "string", "slow"] },
    CommandSpec { name: "incr", arity: 2, write: true, keys: Some((1, 1, 1)), categories: &["write", "string", "fast"] },
    CommandSpec { name: "decr", arity: 2, write: true, keys: Some((1, 1, 1)), categories: &["write", "string", "fast"] },
    CommandSpec { name: "incrby", arity: 3, write: true, keys: Some((1, 1, 1)), categories: &["write", "string", "fast"] },
    CommandSpec { name: "decrby", arity: 3, write: true, keys: Some((1, 1, 1)), categories: &["write", "string", "fast"] },
    // Keyspace, per key.
    CommandSpec { name: "del", arity: -2, write: true, keys: Some((1, -1, 1)), categories: &["keyspace", "write", "slow"] },
    CommandSpec { name: "unlink", arity: -2, write: true, keys: Some((1, -1, 1)), categories: &["keyspace", "write", "fast"] },
    CommandSpec { name: "exists", arity: -2, write: false, keys: Some((1, -1, 1)), categories: &["keyspace", "read", "fast"] },
    CommandSpec { name: "type", arity: 2, write: false, keys: Some((1, 1, 1)), categories: &["keyspace", "read", "fast"] },
    CommandSpec { name: "expire", arity: -3, write: true, keys: Some((1, 1, 1)), categories: &["keyspace", "write", "fast"] },
    CommandSpec { name: "pexpire", arity: -3, write: true, keys: Some((1, 1, 1)), categories: &["keyspace", "write", "fast"] },
    CommandSpec { name: "expireat", arity: -3, write: true, keys: Some((1, 1, 1)), categories: &["keyspace", "write", "fast"] },
    CommandSpec { name: "pexpireat", arity: -3, write: true, keys: Some((1, 1, 1)), categories: &["keyspace", "write", "fast"] },
    CommandSpec { name: "ttl", arity: 2, write: false, keys: Some((1, 1, 1)), categories: &["keyspace", "read", "fast"] },
    CommandSpec { name: "pttl", arity: 2, write: false, keys: Some((1, 1, 1)), categories: &["keyspace", "read", "fast"] },
    CommandSpec { name: "persist", arity: 2, write: true, keys: Some((1, 1, 1)), categories: &["keyspace", "write", "fast"] },
    CommandSpec { name: "object", arity: -2, write: false, keys: Some((2, 2, 1)), categories: &["keyspace", "read", "slow"] },
];

/// The record of the command `name`, or `None` for one this data plane does not serve.
pub fn spec(name: &str) -> Option<&'static CommandSpec> {
    let lowered = name.to_ascii_lowercase();
    CATALOG.iter().find(|spec| spec.name == lowered)
}

/// The keys the command `spec` names among `arguments` (the command's own name included,
/// as the engine counts them).
pub fn command_keys(spec: &CommandSpec, arguments: &[Vec<u8>]) -> Vec<String> {
    let Some((first, last, step)) = spec.keys else {
        return Vec::new();
    };
    let step = step.max(1);
    let end = if last < 0 {
        arguments
            .len()
            .saturating_sub(usize::try_from(-last).unwrap_or(1).saturating_sub(1))
    } else {
        usize::try_from(last).unwrap_or(0).saturating_add(1)
    };
    let mut keys = Vec::new();
    let mut index = first;
    while index < end.min(arguments.len()) {
        keys.push(String::from_utf8_lossy(&arguments[index]).to_string());
        index = index.saturating_add(step);
    }
    keys
}

/// Whether `arguments` satisfies the command's documented arity.
pub fn arity_satisfied(spec: &CommandSpec, arguments: &[Vec<u8>]) -> bool {
    let given = i32::try_from(arguments.len()).unwrap_or(i32::MAX);
    if spec.arity < 0 {
        given >= -spec.arity
    } else {
        given == spec.arity
    }
}

/// The engine's own error for a command given the wrong number of arguments.
pub fn wrong_arity(name: &str) -> Reply {
    Reply::Error(format!(
        "ERR wrong number of arguments for '{}' command",
        name.to_ascii_lowercase()
    ))
}

/// The engine's own error for a value that is not an integer, and for the other refusals a
/// write can come back with.
fn refusal_reply(refusal: WriteRefusal) -> Reply {
    match refusal {
        WriteRefusal::OutOfMemory => Reply::Error(
            "OOM command not allowed when used memory > 'maxmemory'.".to_string(),
        ),
        WriteRefusal::ValueTooLarge => Reply::Error(format!(
            "ERR string exceeds maximum allowed size ({} bytes)",
            super::keyspace::MAX_VALUE_BYTES
        )),
        WriteRefusal::KeyTooLarge => Reply::Error(format!(
            "ERR key exceeds maximum allowed size ({} bytes)",
            super::keyspace::MAX_KEY_BYTES
        )),
        WriteRefusal::NotAnInteger => {
            Reply::Error("ERR value is not an integer or out of range".to_string())
        }
        WriteRefusal::OutOfRange => {
            Reply::Error("ERR increment or decrement would overflow".to_string())
        }
    }
}

/// One argument as text, for the arguments that are not keys or values.
fn text(argument: &[u8]) -> String {
    String::from_utf8_lossy(argument).to_string()
}

/// One argument as a 64-bit integer, or the engine's own error for one that is not.
fn integer(argument: &[u8]) -> Result<i64, Reply> {
    text(argument)
        .trim()
        .parse::<i64>()
        .map_err(|_| Reply::Error("ERR value is not an integer or out of range".to_string()))
}

/// Run the command `arguments` against `keyspace`, under the cache's `limits`.
///
/// Only the commands that READ or WRITE the keyspace are here; the connection's own
/// (`AUTH`, `HELLO`, `INFO`, …) are answered by the session, which knows the cache and the
/// user rather than the data.
pub fn execute(
    spec: &CommandSpec,
    arguments: &[Vec<u8>],
    keyspace: &mut Keyspace,
    limits: &Limits,
    now: SystemTime,
) -> Reply {
    let keys = command_keys(spec, arguments);
    let key = keys.first().cloned().unwrap_or_default();
    match spec.name {
        "get" => match keyspace.get(&key, now) {
            Some(value) => Reply::Bulk(value),
            None => Reply::Nil,
        },
        "getdel" => match keyspace.get(&key, now) {
            Some(value) => {
                keyspace.remove(&key, now);
                Reply::Bulk(value)
            }
            None => Reply::Nil,
        },
        "set" => set_command(&key, arguments, keyspace, limits, now),
        "setnx" => {
            if keyspace.exists(&key, now) {
                return Reply::Integer(0);
            }
            match keyspace.set(&key, arguments[2].clone(), None, false, limits, now) {
                Ok(()) => Reply::Integer(1),
                Err(refusal) => refusal_reply(refusal),
            }
        }
        "setex" | "psetex" => {
            let span = match integer(&arguments[2]) {
                Ok(span) if span > 0 => span,
                Ok(_) => {
                    return Reply::Error(format!(
                        "ERR invalid expire time in '{}' command",
                        spec.name
                    ))
                }
                Err(reply) => return reply,
            };
            // Checked, because the span is the caller's: one past what an instant can
            // carry is an invalid expire time, not a panic.
            let due = now.checked_add(if spec.name == "setex" {
                Duration::from_secs(span.unsigned_abs())
            } else {
                Duration::from_millis(span.unsigned_abs())
            });
            let Some(due) = due else {
                return Reply::Error(format!(
                    "ERR invalid expire time in '{}' command",
                    spec.name
                ));
            };
            match keyspace.set(&key, arguments[3].clone(), Some(due), false, limits, now) {
                Ok(()) => Reply::ok(),
                Err(refusal) => refusal_reply(refusal),
            }
        }
        "getset" => {
            let held = keyspace.get(&key, now);
            match keyspace.set(&key, arguments[2].clone(), None, false, limits, now) {
                Ok(()) => held.map_or(Reply::Nil, Reply::Bulk),
                Err(refusal) => refusal_reply(refusal),
            }
        }
        "append" => match keyspace.append(&key, &arguments[2], limits, now) {
            Ok(length) => Reply::Integer(i64::try_from(length).unwrap_or(i64::MAX)),
            Err(refusal) => refusal_reply(refusal),
        },
        "strlen" => Reply::Integer(
            keyspace
                .get(&key, now)
                .map(|value| i64::try_from(value.len()).unwrap_or(i64::MAX))
                .unwrap_or_default(),
        ),
        "mget" => Reply::Array(
            keys.iter()
                .map(|key| keyspace.get(key, now).map_or(Reply::Nil, Reply::Bulk))
                .collect(),
        ),
        "mset" => {
            if arguments.len() % 2 != 1 {
                return wrong_arity(spec.name);
            }
            for pair in arguments[1..].chunks(2) {
                let key = String::from_utf8_lossy(&pair[0]).to_string();
                if let Err(refusal) =
                    keyspace.set(&key, pair[1].clone(), None, false, limits, now)
                {
                    return refusal_reply(refusal);
                }
            }
            Reply::ok()
        }
        "incr" | "decr" | "incrby" | "decrby" => {
            let magnitude = match spec.name {
                "incr" | "decr" => 1,
                _ => match integer(&arguments[2]) {
                    Ok(value) => value,
                    Err(reply) => return reply,
                },
            };
            let delta = if spec.name.starts_with("decr") {
                match magnitude.checked_neg() {
                    Some(delta) => delta,
                    None => return refusal_reply(WriteRefusal::OutOfRange),
                }
            } else {
                magnitude
            };
            match keyspace.increment(&key, delta, limits, now) {
                Ok(total) => Reply::Integer(total),
                Err(refusal) => refusal_reply(refusal),
            }
        }
        "del" | "unlink" => Reply::Integer(
            keys.iter()
                .filter(|key| keyspace.remove(key, now))
                .count()
                .try_into()
                .unwrap_or(i64::MAX),
        ),
        "exists" => Reply::Integer(
            keys.iter()
                .filter(|key| keyspace.exists(key, now))
                .count()
                .try_into()
                .unwrap_or(i64::MAX),
        ),
        "type" => Reply::Status(
            if keyspace.holds(&key, now) {
                "string"
            } else {
                "none"
            }
            .to_string(),
        ),
        "expire" | "pexpire" | "expireat" | "pexpireat" => {
            let span = match integer(&arguments[2]) {
                Ok(span) => span,
                Err(reply) => return reply,
            };
            let due = match spec.name {
                "expire" => now.checked_add(Duration::from_secs(span.unsigned_abs())),
                "pexpire" => now.checked_add(Duration::from_millis(span.unsigned_abs())),
                "expireat" => SystemTime::UNIX_EPOCH
                    .checked_add(Duration::from_secs(span.unsigned_abs())),
                _ => SystemTime::UNIX_EPOCH
                    .checked_add(Duration::from_millis(span.unsigned_abs())),
            };
            let Some(due) = due else {
                return Reply::Error(format!(
                    "ERR invalid expire time in '{}' command",
                    spec.name
                ));
            };
            // A negative span, or an instant already past, deletes the key — "if a key is
            // set to expire in the past it is deleted straight away".
            if span < 0 || due <= now {
                return Reply::Integer(i64::from(keyspace.remove(&key, now)));
            }
            Reply::Integer(i64::from(keyspace.expire_at(&key, due, now)))
        }
        "ttl" | "pttl" => match keyspace.time_to_live(&key, now) {
            // "-2 if the key does not exist, -1 if the key exists but has no expiry".
            None => Reply::Integer(-2),
            Some(None) => Reply::Integer(-1),
            Some(Some(left)) => Reply::Integer(if spec.name == "ttl" {
                // Rounded UP, as the engine reports whole seconds of a remaining life.
                i64::try_from(left.as_secs() + u64::from(left.subsec_millis() > 0))
                    .unwrap_or(i64::MAX)
            } else {
                i64::try_from(left.as_millis()).unwrap_or(i64::MAX)
            }),
        },
        "persist" => Reply::Integer(i64::from(keyspace.persist(&key, now))),
        "object" => object_command(arguments, &key, keyspace, now),
        "dbsize" => Reply::Integer(i64::try_from(keyspace.len(now)).unwrap_or(i64::MAX)),
        "flushall" | "flushdb" => {
            keyspace.clear();
            Reply::ok()
        }
        "keys" => {
            let pattern = text(&arguments[1]);
            Reply::Array(
                keyspace
                    .keys(now)
                    .into_iter()
                    .filter(|key| super::keyspace::glob_matches(&pattern, key))
                    .map(Reply::text)
                    .collect(),
            )
        }
        "randomkey" => keyspace
            .keys(now)
            .into_iter()
            .next()
            .map_or(Reply::Nil, Reply::text),
        "scan" => scan_command(arguments, keyspace, now),
        // Every command in the catalog is either answered above or by the session; a name
        // that reaches here is one the catalog carries and nothing serves, which is an
        // honest `unknown command` rather than a silent success.
        other => Reply::Error(format!(
            "ERR unknown command '{other}', with args beginning with: "
        )),
    }
}

/// `OBJECT ENCODING key` / `OBJECT REFCOUNT key`: what the engine holds the value AS.
///
/// The encoding is the engine's own reading of the value it stored: a string that is a
/// 64-bit integer is held in its INTEGER encoding (`int`), which is what makes an `INCR` an
/// increment rather than a read-modify-write over bytes; a short string is `embstr` and a
/// longer one `raw`, at the engine's documented boundary ([`EMBEDDED_STRING_BYTES`]).
fn object_command(
    arguments: &[Vec<u8>],
    key: &str,
    keyspace: &mut Keyspace,
    now: SystemTime,
) -> Reply {
    let subcommand = text(&arguments[1]).to_ascii_lowercase();
    if !matches!(subcommand.as_str(), "encoding" | "refcount" | "idletime") {
        return Reply::Error(format!(
            "ERR Unknown subcommand or wrong number of arguments for '{subcommand}'. Try OBJECT \
             HELP."
        ));
    }
    if arguments.len() != 3 {
        return wrong_arity(&format!("object|{subcommand}"));
    }
    let Some(value) = keyspace.peek(key, now) else {
        // The engine's own answer for a key it does not hold.
        return Reply::Error("ERR no such key".to_string());
    };
    match subcommand.as_str() {
        "encoding" => Reply::Bulk(
            if std::str::from_utf8(value)
                .ok()
                .and_then(|text| text.parse::<i64>().ok())
                .is_some()
            {
                b"int".to_vec()
            } else if value.len() <= EMBEDDED_STRING_BYTES {
                b"embstr".to_vec()
            } else {
                b"raw".to_vec()
            },
        ),
        // One keyspace holds one reference to its value, and this server shares none.
        "refcount" => Reply::Integer(1),
        // How long the value has been idle is not kept: the keyspace holds a logical
        // recency for eviction, not a wall-clock idle time, so the honest answer is 0.
        _ => Reply::Integer(0),
    }
}

/// The length up to which the engine holds a string value EMBEDDED in its object
/// (`embstr`) rather than as a separate allocation (`raw`) — the engine's own
/// `OBJ_ENCODING_EMBSTR_SIZE_LIMIT`.
const EMBEDDED_STRING_BYTES: usize = 44;

/// `SET key value [EX s] [PX ms] [EXAT s] [PXAT ms] [NX|XX] [KEEPTTL] [GET]`.
fn set_command(
    key: &str,
    arguments: &[Vec<u8>],
    keyspace: &mut Keyspace,
    limits: &Limits,
    now: SystemTime,
) -> Reply {
    let value = arguments[2].clone();
    let mut expires_at = None;
    let mut keep_ttl = false;
    let mut only_if_absent = false;
    let mut only_if_present = false;
    let mut answer_held = false;
    let mut index = 3;
    while index < arguments.len() {
        let option = text(&arguments[index]).to_ascii_uppercase();
        match option.as_str() {
            "NX" => only_if_absent = true,
            "XX" => only_if_present = true,
            "KEEPTTL" => keep_ttl = true,
            "GET" => answer_held = true,
            "EX" | "PX" | "EXAT" | "PXAT" => {
                let Some(raw) = arguments.get(index + 1) else {
                    return Reply::Error("ERR syntax error".to_string());
                };
                let span = match integer(raw) {
                    Ok(span) => span,
                    Err(reply) => return reply,
                };
                if span <= 0 && (option == "EX" || option == "PX") {
                    return Reply::Error("ERR invalid expire time in 'set' command".to_string());
                }
                let magnitude = span.unsigned_abs();
                expires_at = match option.as_str() {
                    "EX" => now.checked_add(Duration::from_secs(magnitude)),
                    "PX" => now.checked_add(Duration::from_millis(magnitude)),
                    "EXAT" => {
                        SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(magnitude))
                    }
                    _ => SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(magnitude)),
                };
                if expires_at.is_none() {
                    return Reply::Error("ERR invalid expire time in 'set' command".to_string());
                }
                index = index.saturating_add(1);
            }
            _ => return Reply::Error("ERR syntax error".to_string()),
        }
        index = index.saturating_add(1);
    }
    let held = keyspace.get(key, now);
    if only_if_absent && held.is_some() || only_if_present && held.is_none() {
        // "SET … NX returns nil when the key already holds a value"; the same for `XX`
        // and a key that holds none.
        return Reply::Nil;
    }
    match keyspace.set(key, value, expires_at, keep_ttl, limits, now) {
        Ok(()) if answer_held => held.map_or(Reply::Nil, Reply::Bulk),
        Ok(()) => Reply::ok(),
        Err(refusal) => refusal_reply(refusal),
    }
}

/// `SCAN cursor [MATCH pattern] [COUNT n]`: one page of the keyspace, with the cursor the
/// next call resumes from — the index into the keyspace's own order, and `0` once the walk
/// is done.
fn scan_command(arguments: &[Vec<u8>], keyspace: &mut Keyspace, now: SystemTime) -> Reply {
    let cursor = match integer(&arguments[1]) {
        Ok(cursor) if cursor >= 0 => usize::try_from(cursor).unwrap_or(0),
        _ => return Reply::Error("ERR invalid cursor".to_string()),
    };
    let mut pattern = None;
    let mut count = SCAN_PAGE;
    // Whether the type this scan asks for is the one the keyspace holds; see the `TYPE`
    // arm below.
    let mut selects_strings = true;
    let mut index = 2;
    while index < arguments.len() {
        match text(&arguments[index]).to_ascii_uppercase().as_str() {
            "MATCH" => {
                let Some(value) = arguments.get(index + 1) else {
                    return Reply::Error("ERR syntax error".to_string());
                };
                pattern = Some(text(value));
                index = index.saturating_add(2);
            }
            "COUNT" => {
                let Some(value) = arguments.get(index + 1) else {
                    return Reply::Error("ERR syntax error".to_string());
                };
                let asked = match integer(value) {
                    Ok(asked) if asked > 0 => usize::try_from(asked).unwrap_or(SCAN_PAGE),
                    _ => return Reply::Error("ERR syntax error".to_string()),
                };
                // The page a caller asks for is clamped to this server's own, so one call
                // cannot ask for an unbounded answer.
                count = asked.min(SCAN_PAGE);
                index = index.saturating_add(2);
            }
            // `TYPE` narrows a scan to one data type. This keyspace holds STRINGS, so a
            // scan for any other type selects nothing — the type is honoured rather than
            // read and dropped.
            "TYPE" => {
                let Some(value) = arguments.get(index + 1) else {
                    return Reply::Error("ERR syntax error".to_string());
                };
                selects_strings = text(value).eq_ignore_ascii_case("string");
                index = index.saturating_add(2);
            }
            _ => return Reply::Error("ERR syntax error".to_string()),
        }
    }
    if !selects_strings {
        // Nothing of that type is held, and the walk is over: the cursor a client resumes
        // from is 0 and the page is empty.
        return Reply::Array(vec![Reply::text("0"), Reply::Array(Vec::new())]);
    }
    let keys = keyspace.keys(now);
    let page: Vec<Reply> = keys
        .iter()
        .skip(cursor)
        .take(count)
        .filter(|key| {
            pattern
                .as_deref()
                .is_none_or(|pattern| super::keyspace::glob_matches(pattern, key))
        })
        .cloned()
        .map(Reply::text)
        .collect();
    let walked = cursor.saturating_add(count);
    let next = if walked >= keys.len() { 0 } else { walked };
    Reply::Array(vec![Reply::text(next.to_string()), Reply::Array(page)])
}
