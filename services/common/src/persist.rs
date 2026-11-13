// SPDX-License-Identifier: CC0-1.0

//! PERSISTED STATE: what of a service survives the emulator restarting.
//!
//! The engine keeps every service's state in memory, so by default a restart is an empty
//! emulator. A service that must survive one implements [`Persisted`]: it writes a plain data
//! view of its records ([`Persisted::snapshot`]) — never its live handles, connections or
//! listeners — and rebuilds itself from that view ([`Persisted::restore`]).
//!
//! Nothing is written unless [`STATE_DIR_VARIABLE`] names a directory: an emulator embedded in
//! a test, or started without it, stays exactly as ephemeral as before. With it set, each
//! service's snapshot is one file, `<dir>/<namespace>/<service>.json`, where the namespace is
//! the emulator instance's ([`super::host_runtime::NAMESPACE_VARIABLE`]). Every write is atomic
//! (a temporary file in the same directory, flushed to disk, then renamed over the old one), so a
//! crash mid-write leaves the previous snapshot, never half of a new one.
//!
//! A snapshot written by a newer version of a service, or for another service or namespace, is
//! refused with a [`PersistError`] rather than half-read: a restart that cannot restore honestly
//! says so.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The environment variable naming the directory snapshots live under. Unset: nothing persists.
pub const STATE_DIR_VARIABLE: &str = "EMULATOR_STATE_DIR";
/// The `format` every snapshot envelope carries.
const FORMAT: &str = "aws-emulator.state";

/// A service whose state survives a restart.
pub trait Persisted: Sized {
    /// The file name the service's snapshot is stored under; stable across releases.
    const SERVICE: &'static str;
    /// The version of [`Persisted::snapshot`]'s shape. Raise it when the shape changes, and
    /// accept the older versions [`Persisted::restore`] still understands.
    const VERSION: u32;

    /// The service's records as plain data.
    fn snapshot(&self) -> Value;

    /// The service rebuilt from a snapshot of `version` (at most [`Persisted::VERSION`]).
    fn restore(version: u32, snapshot: Value) -> Result<Self, String>;
}

/// Why a snapshot could not be written or restored.
#[derive(Debug)]
pub enum PersistError {
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    /// The file is not a snapshot envelope for this service and namespace.
    Foreign { path: PathBuf, detail: String },
    /// Written by a newer version of the service than this emulator knows.
    Newer {
        path: PathBuf,
        found: u32,
        known: u32,
    },
    /// The service refused its own snapshot.
    Restore { path: PathBuf, detail: String },
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Foreign { path, detail } => {
                write!(
                    f,
                    "{} is not this service's snapshot: {detail}",
                    path.display()
                )
            }
            Self::Newer { path, found, known } => write!(
                f,
                "{} was written by snapshot version {found}; this emulator knows up to {known}",
                path.display()
            ),
            Self::Restore { path, detail } => {
                write!(f, "{} could not be restored: {detail}", path.display())
            }
        }
    }
}

impl std::error::Error for PersistError {}

/// Where snapshots are kept for this emulator instance, or `None` when persistence is off.
pub fn namespace_dir() -> Option<PathBuf> {
    let root = std::env::var_os(STATE_DIR_VARIABLE).filter(|dir| !dir.is_empty())?;
    Some(PathBuf::from(root).join(namespace()))
}

fn namespace() -> String {
    std::env::var(super::host_runtime::NAMESPACE_VARIABLE)
        .unwrap_or_else(|_| super::host_runtime::DEFAULT_NAMESPACE.to_string())
}

/// Write `state`'s snapshot, when persistence is on. Returns the file written.
pub fn save<T: Persisted>(state: &T) -> Result<Option<PathBuf>, PersistError> {
    let Some(dir) = namespace_dir() else {
        return Ok(None);
    };
    save_in(&dir, &namespace(), state).map(Some)
}

/// Restore the service from its snapshot, when persistence is on and one exists.
pub fn load<T: Persisted>() -> Result<Option<T>, PersistError> {
    let Some(dir) = namespace_dir() else {
        return Ok(None);
    };
    load_in(&dir, &namespace())
}

fn file_for<T: Persisted>(dir: &Path) -> PathBuf {
    dir.join(format!("{}.json", T::SERVICE))
}

fn save_in<T: Persisted>(dir: &Path, namespace: &str, state: &T) -> Result<PathBuf, PersistError> {
    let path = file_for::<T>(dir);
    let io = |error| PersistError::Io {
        path: path.clone(),
        error,
    };
    std::fs::create_dir_all(dir).map_err(io)?;
    let envelope = json!({
        "format": FORMAT,
        "service": T::SERVICE,
        "namespace": namespace,
        "version": T::VERSION,
        "written_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default(),
        "state": state.snapshot(),
    });
    let bytes = serde_json::to_vec(&envelope).map_err(|error| io(error.into()))?;
    let temporary = dir.join(format!(".{}.json.{}.tmp", T::SERVICE, std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        // The rename is durable only once the directory entry is.
        std::fs::File::open(dir)?.sync_all()
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(io(error));
    }
    Ok(path)
}

fn load_in<T: Persisted>(dir: &Path, namespace: &str) -> Result<Option<T>, PersistError> {
    let path = file_for::<T>(dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PersistError::Io { path, error }),
    };
    let foreign = |detail: String| PersistError::Foreign {
        path: path.clone(),
        detail,
    };
    let mut envelope: Value =
        serde_json::from_slice(&bytes).map_err(|error| foreign(error.to_string()))?;
    for (field, expected) in [
        ("format", FORMAT),
        ("service", T::SERVICE),
        ("namespace", namespace),
    ] {
        if envelope[field] != expected {
            return Err(foreign(format!(
                "{field} is {}, not {expected:?}",
                envelope[field]
            )));
        }
    }
    let version = envelope["version"]
        .as_u64()
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| foreign("no version".to_string()))?;
    if version > T::VERSION {
        return Err(PersistError::Newer {
            path,
            found: version,
            known: T::VERSION,
        });
    }
    T::restore(version, envelope["state"].take())
        .map(Some)
        .map_err(|detail| PersistError::Restore { path, detail })
}

/// Serde for the SDK's [`aws_smithy_types::DateTime`], which carries none of its own: stored as
/// `[seconds, subsecond nanos]` since the epoch, so a restored timestamp is exactly the one saved.
/// Use as `#[serde(with = "::service_common::persist::smithy_time")]`.
pub mod smithy_time {
    use aws_smithy_types::DateTime;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(time: &DateTime, serializer: S) -> Result<S::Ok, S::Error> {
        (time.secs(), time.subsec_nanos()).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<DateTime, D::Error> {
        let (secs, nanos) = <(i64, u32)>::deserialize(deserializer)?;
        Ok(DateTime::from_secs_and_nanos(secs, nanos))
    }
}

/// [`smithy_time`] for an optional timestamp.
pub mod smithy_time_opt {
    use aws_smithy_types::DateTime;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        time: &Option<DateTime>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        time.map(|time| (time.secs(), time.subsec_nanos()))
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<DateTime>, D::Error> {
        let parts = Option::<(i64, u32)>::deserialize(deserializer)?;
        Ok(parts.map(|(secs, nanos)| DateTime::from_secs_and_nanos(secs, nanos)))
    }
}

/// Serde for a monotonic [`std::time::Instant`], which means nothing to another process: stored
/// as the wall-clock time it corresponds to, and rebuilt against the restoring process's clock.
/// A deadline already past stays past (due at once), one still ahead keeps its remaining span.
/// Use as `#[serde(with = "::service_common::persist::instant")]`.
pub mod instant {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    pub(super) fn to_wall(instant: Instant) -> (u64, u32) {
        let (now, wall) = (Instant::now(), SystemTime::now());
        let at = if instant >= now {
            wall + (instant - now)
        } else {
            wall - (now - instant)
        };
        let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
        (since.as_secs(), since.subsec_nanos())
    }

    pub(super) fn from_wall((secs, nanos): (u64, u32)) -> Instant {
        let at = UNIX_EPOCH + Duration::new(secs, nanos);
        let (now, wall) = (Instant::now(), SystemTime::now());
        match at.duration_since(wall) {
            Ok(ahead) => now + ahead,
            Err(behind) => now.checked_sub(behind.duration()).unwrap_or(now),
        }
    }

    pub fn serialize<S: Serializer>(instant: &Instant, serializer: S) -> Result<S::Ok, S::Error> {
        to_wall(*instant).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Instant, D::Error> {
        Ok(from_wall(<(u64, u32)>::deserialize(deserializer)?))
    }
}

/// [`instant`] for an optional deadline.
pub mod instant_opt {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Instant;

    pub fn serialize<S: Serializer>(
        instant: &Option<Instant>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        instant.map(super::instant::to_wall).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Instant>, D::Error> {
        Ok(Option::<(u64, u32)>::deserialize(deserializer)?.map(super::instant::from_wall))
    }
}

/// A map whose keys JSON cannot hold (a tuple, a struct), stored as a list of `[key, value]`
/// pairs. JSON object keys must be strings, and `serde_json` only finds out when the map is
/// non-empty, so an untagged one passes every test with an empty account and fails the first
/// real snapshot. Use as `#[serde(with = "::service_common::persist::entries")]` on a
/// `BTreeMap` or `HashMap`.
pub mod entries {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<'a, M, K, V, S>(map: &'a M, serializer: S) -> Result<S::Ok, S::Error>
    where
        &'a M: IntoIterator<Item = (&'a K, &'a V)>,
        K: Serialize + 'a,
        V: Serialize + 'a,
        S: Serializer,
    {
        serializer.collect_seq(map)
    }

    pub fn deserialize<'de, M, K, V, D>(deserializer: D) -> Result<M, D::Error>
    where
        M: FromIterator<(K, V)>,
        K: Deserialize<'de>,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Ok(Vec::<(K, V)>::deserialize(deserializer)?.into_iter().collect())
    }
}

/// A map of deadlines ([`instant`]) under any key, stored as `[key, wall-clock]` pairs.
pub mod instant_map {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Instant;

    pub fn serialize<'a, M, K, S>(map: &'a M, serializer: S) -> Result<S::Ok, S::Error>
    where
        &'a M: IntoIterator<Item = (&'a K, &'a Instant)>,
        K: Serialize + 'a,
        S: Serializer,
    {
        serializer.collect_seq(
            map.into_iter()
                .map(|(key, instant)| (key, super::instant::to_wall(*instant))),
        )
    }

    pub fn deserialize<'de, M, K, D>(deserializer: D) -> Result<M, D::Error>
    where
        M: FromIterator<(K, Instant)>,
        K: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Ok(Vec::<(K, (u64, u32))>::deserialize(deserializer)?
            .into_iter()
            .map(|(key, wall)| (key, super::instant::from_wall(wall)))
            .collect())
    }
}

/// Restore a state whose records keep catalog words as `&'static str` (`"available"`,
/// `"pending"`): such a type deserializes only from data that lives forever, so the snapshot is
/// kept for the life of the process and the words borrow from it. That is one copy of the
/// snapshot per restore, which happens once, at boot. Derive the record types with
/// `#[serde(bound(deserialize = "'de: 'static"))]`.
pub fn restore_static<T: serde::Deserialize<'static>>(snapshot: Value) -> Result<T, String> {
    let snapshot: &'static Value = Box::leak(Box::new(snapshot));
    T::deserialize(snapshot).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Counter(u64);

    impl Persisted for Counter {
        const SERVICE: &'static str = "counter";
        const VERSION: u32 = 2;
        fn snapshot(&self) -> Value {
            json!({ "count": self.0 })
        }
        fn restore(version: u32, snapshot: Value) -> Result<Self, String> {
            // Version 1 stored a bare number.
            let count = if version == 1 {
                snapshot.as_u64()
            } else {
                snapshot["count"].as_u64()
            };
            count.map(Counter).ok_or_else(|| "no count".to_string())
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aws-emulator-persist-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_saved_state_restores_and_a_missing_one_is_none() {
        let dir = scratch("roundtrip");
        assert!(load_in::<Counter>(&dir, "ns").unwrap().is_none());
        let path = save_in(&dir, "ns", &Counter(7)).unwrap();
        assert_eq!(path, dir.join("counter.json"));
        assert_eq!(load_in::<Counter>(&dir, "ns").unwrap(), Some(Counter(7)));
        // No temporary file is left beside it.
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("counter.json")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_older_version_is_restored_and_a_newer_one_refused() {
        let dir = scratch("versions");
        std::fs::create_dir_all(&dir).unwrap();
        let write = |version: u32, state: Value| {
            let envelope = json!({
                "format": FORMAT, "service": "counter", "namespace": "ns",
                "version": version, "state": state,
            });
            std::fs::write(dir.join("counter.json"), envelope.to_string()).unwrap();
        };
        write(1, json!(3));
        assert_eq!(load_in::<Counter>(&dir, "ns").unwrap(), Some(Counter(3)));
        write(3, json!({ "count": 3 }));
        assert!(matches!(
            load_in::<Counter>(&dir, "ns"),
            Err(PersistError::Newer {
                found: 3,
                known: 2,
                ..
            })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn another_namespace_or_a_damaged_file_is_refused() {
        let dir = scratch("foreign");
        save_in(&dir, "ns-a", &Counter(1)).unwrap();
        assert!(matches!(
            load_in::<Counter>(&dir, "ns-b"),
            Err(PersistError::Foreign { .. })
        ));
        std::fs::write(dir.join("counter.json"), b"{not json").unwrap();
        assert!(matches!(
            load_in::<Counter>(&dir, "ns-a"),
            Err(PersistError::Foreign { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A direct `&'static str` field is borrowed by serde on its own; a word nested inside
    // another type needs `#[serde(bound(deserialize = "'de: 'static"))]` on the record.
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Record {
        state: &'static str,
        #[serde(with = "entries")]
        by_pair: std::collections::BTreeMap<(String, String), u32>,
        #[serde(with = "instant_map")]
        due: std::collections::BTreeMap<(String, String), std::time::Instant>,
        #[serde(with = "instant")]
        started: std::time::Instant,
    }

    #[test]
    fn tuple_keys_deadlines_and_catalog_words_survive_json_text() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let key = ("vol-1".to_string(), "i-1".to_string());
        let record = Record {
            state: "attaching",
            by_pair: [(key.clone(), 7)].into(),
            due: [(key.clone(), now + Duration::from_secs(30))].into(),
            started: now,
        };
        // A plain JSON map would refuse the tuple key here.
        let text = serde_json::to_string(&record).unwrap();
        let back: Record = restore_static(serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(back.state, "attaching");
        assert_eq!(back.by_pair, record.by_pair);
        // A deadline keeps its remaining span, to within the time this test takes.
        let ahead = back.due[&key].saturating_duration_since(Instant::now());
        assert!(ahead > Duration::from_secs(28) && ahead <= Duration::from_secs(30), "{ahead:?}");
        assert!(back.started <= Instant::now());
    }
}
