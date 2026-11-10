// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The `Parameter$Source` of a value that is the engine's own default rather
/// than one a customer set.
///
/// `DescribeCacheParameters$Source` documents the sources a parameter is
/// reported under ("The parameter types to return. Valid values: user | system |
/// engine-default"); an engine default is the service's own value, so it is
/// reported as `system`.
pub const PARAMETER_SOURCE_SYSTEM: &str = "system";

/// The `Parameter$Source` of a value a customer set on the group being read,
/// from the same `DescribeCacheParameters$Source` vocabulary as
/// [`PARAMETER_SOURCE_SYSTEM`].
pub const PARAMETER_SOURCE_USER: &str = "user";

/// The third spelling `DescribeCacheParameters$Source` accepts as a FILTER
/// ("Valid values: user | system | engine-default").
///
/// It selects the same parameters as [`PARAMETER_SOURCE_SYSTEM`] — the ones a
/// group reports the engine's own default for — because an engine default IS
/// the service's value and is reported under the `system` spelling; see
/// [`PARAMETER_SOURCE_SYSTEM`].
pub const PARAMETER_SOURCE_FILTER_ENGINE_DEFAULT: &str = "engine-default";

/// The `Parameter$Source` the `DescribeCacheParameters$Source` filter `value`
/// selects, or the reason that value is refused.
///
/// `Source` is a plain string with NO modelled enum or pattern, so the
/// vocabulary its own documentation states ("Valid values: user | system |
/// engine-default") is enforced here, and it is a SUPERSET of the two spellings
/// a response reports: `engine-default` and `system` both select the parameters
/// whose value is the engine's own. Matched ignoring ASCII case, as this
/// service's other unconstrained filter vocabularies are (see
/// [`snapshot_source_filter`]): a source is a vocabulary word, not an identity.
pub fn parameter_source_filter(value: &str) -> Result<&'static str, String> {
    for (spelling, selected) in [
        (PARAMETER_SOURCE_USER, PARAMETER_SOURCE_USER),
        (PARAMETER_SOURCE_SYSTEM, PARAMETER_SOURCE_SYSTEM),
        (
            PARAMETER_SOURCE_FILTER_ENGINE_DEFAULT,
            PARAMETER_SOURCE_SYSTEM,
        ),
    ] {
        if value.eq_ignore_ascii_case(spelling) {
            return Ok(selected);
        }
    }
    Err(format!(
        "Invalid value {value} for Source: must be one of {PARAMETER_SOURCE_USER} | \
         {PARAMETER_SOURCE_SYSTEM} | {PARAMETER_SOURCE_FILTER_ENGINE_DEFAULT}."
    ))
}

/// `ChangeType`'s values (`smithy_query(query="shape", shape="ChangeType")`:
/// `immediate | requires-reboot`): whether a change to the parameter is applied
/// at once or only once the nodes are rebooted.
pub const CHANGE_TYPE_IMMEDIATE: &str = "immediate";
/// See [`CHANGE_TYPE_IMMEDIATE`].
pub const CHANGE_TYPE_REQUIRES_REBOOT: &str = "requires-reboot";

/// The two parameter catalogs this service publishes, one per cache engine.
///
/// Valkey is the Redis OSS-compatible engine, so the Valkey families draw on the
/// same catalog as the Redis OSS ones; Memcached has a catalog of its own.
pub(super) const PARAMETER_ENGINE_MEMCACHED: &str = "memcached";
/// See [`PARAMETER_ENGINE_MEMCACHED`].
pub(super) const PARAMETER_ENGINE_REDIS: &str = "redis";

/// The minor version of a family line that covers every minor of its major
/// (`redis7` covers every Redis OSS 7.x), so every parameter that major
/// introduced belongs to it.
pub(super) const ANY_MINOR: i32 = i32::MAX;

/// Each published family's parameter catalog and the version its parameters are
/// selected by.
///
/// A family's parameters are its ENGINE's, which is why each row names the
/// catalog the family draws on. The version is the HIGHEST engine version the
/// family's line covers, because a family carries every parameter any version on
/// its line introduced: `redis6.x` covers Redis OSS 6.0 through 6.2, and
/// `redis7` / `valkey7` / `valkey8` cover every minor of their major — see
/// [`ANY_MINOR`].
///
/// The family names are [`PARAMETER_GROUP_FAMILIES`], this service's published
/// set; this table says what each of them MEANS to the parameter catalog.
pub const FAMILY_LINES: [(&str, &str, (i32, i32)); PARAMETER_GROUP_FAMILIES.len()] = [
    ("memcached1.4", PARAMETER_ENGINE_MEMCACHED, (1, 4)),
    ("memcached1.5", PARAMETER_ENGINE_MEMCACHED, (1, 5)),
    ("memcached1.6", PARAMETER_ENGINE_MEMCACHED, (1, ANY_MINOR)),
    ("redis2.6", PARAMETER_ENGINE_REDIS, (2, 6)),
    ("redis2.8", PARAMETER_ENGINE_REDIS, (2, 8)),
    ("redis3.2", PARAMETER_ENGINE_REDIS, (3, 2)),
    ("redis4.0", PARAMETER_ENGINE_REDIS, (4, ANY_MINOR)),
    ("redis5.0", PARAMETER_ENGINE_REDIS, (5, ANY_MINOR)),
    ("redis6.x", PARAMETER_ENGINE_REDIS, (6, ANY_MINOR)),
    ("redis7", PARAMETER_ENGINE_REDIS, (7, ANY_MINOR)),
    ("valkey7", PARAMETER_ENGINE_REDIS, (7, ANY_MINOR)),
    ("valkey8", PARAMETER_ENGINE_REDIS, (8, ANY_MINOR)),
];

/// The parameter catalog `family` draws on and the version its parameters are
/// selected by, or `None` when no published family carries that name.
pub fn family_line(family: &str) -> Option<(&'static str, (i32, i32))> {
    FAMILY_LINES
        .iter()
        .find(|(name, _, _)| *name == family)
        .map(|(_, engine, version)| (*engine, *version))
}

/// The ENGINE a published parameter group family belongs to.
///
/// "A family's name is the engine followed by its version line" (see
/// [`PARAMETER_GROUP_FAMILIES`]), so the engine is the family name up to its
/// first digit: `redis7` is Redis OSS, `valkey8` is Valkey, `memcached1.6` is
/// Memcached. This is the engine a group's parameters are MEANT for, which is a
/// different question from the catalog they are drawn from ([`family_line`]:
/// Valkey draws on the Redis OSS catalog).
pub fn family_engine(family: &str) -> &str {
    let end = family
        .find(|character: char| character.is_ascii_digit())
        .unwrap_or(family.len());
    &family[..end]
}

/// How ElastiCache words a request whose cache parameter group belongs to another
/// engine than the one the resource runs: `<group-engine> parameter group is not
/// applicable to engine <engine>`.
///
/// The variable parts are the engine the group's family belongs to and the engine
/// the request named; the rest is fixed text, and it carries no trailing stop.
pub fn parameter_group_engine_mismatch_message(group_engine: &str, engine: &str) -> String {
    format!("{group_engine} parameter group is not applicable to engine {engine}")
}

/// `version` as the `(major, minor)` pair a family line is compared against, so
/// a parameter's `MinimumEngineVersion` ("2.6.13", "6.2.6") and a family's line
/// are ordered by the same rule. A part that is not a number counts as 0, which
/// keeps the comparison total.
pub(super) fn version_rank(version: &str) -> (i32, i32) {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    (major, minor)
}

/// One row of an engine's parameter catalog, in column order: the parameter's
/// name, the engine's default value for it, its description, its data type, the
/// values it may take, whether a customer may modify it, the earliest engine
/// version it applies to, whether a change to it is immediate or needs a reboot,
/// and whether its value depends on the cache node type.
///
/// The allowed-values column is the `Parameter$AllowedValues` the catalog
/// publishes, in the three forms ElastiCache writes it in: an enumeration of the
/// literal values (`yes,no`), a numeric RANGE as `<lo>-<hi>` where the engine
/// bounds the value at both ends and `<lo>-` where it bounds only the bottom
/// (optionally with the discrete values that sit outside the range in front of
/// it, `0,20-`), and the EMPTY string only where a customer cannot set the
/// parameter at all — a value the engine or the node type fixes
/// (`IsModifiable: false`), which is the one case there is no set to discover.
/// So every MODIFIABLE numeric parameter publishes its range: a client that
/// reads the catalog to decide what it may set finds the bounds to validate
/// against.
pub(super) struct ParameterRow(
    pub(super) &'static str,
    pub(super) &'static str,
    pub(super) &'static str,
    pub(super) &'static str,
    pub(super) &'static str,
    pub(super) bool,
    pub(super) &'static str,
    pub(super) &'static str,
    pub(super) bool,
);

/// The parameters ElastiCache publishes for its Redis OSS-compatible engines
/// (Redis OSS and Valkey).
///
/// Source: the parameter tables of the ElastiCache User Guide's "Redis-specific
/// parameters" / "Valkey-specific parameters" sections — the names, data types,
/// value sets, modifiability, change types and minimum engine versions below are
/// that published catalog, and the engine's own defaults are the values. Nothing
/// in the Smithy model enumerates parameters (`Parameter` describes only the
/// shape of one), so this table is the emulator's single statement of what the
/// Redis OSS-compatible engine defaults ARE; it is deliberately a representative
/// catalog rather than every parameter AWS ships, and every family that draws on
/// it reports exactly the rows whose minimum engine version its line covers.
///
/// The allowed-values column follows [`ParameterRow`]: each numeric parameter a
/// customer may modify carries the range the ENGINE constrains it to, as the
/// Redis OSS / Valkey configuration reference bounds it (for example active
/// defragmentation's effort is a percentage, `maxmemory-samples` is 1 to 64, a
/// replication backlog is at least 16 KiB, and a protocol element is 1 MiB to
/// 512 MiB), so a client scanning the catalog for what it may tune finds both
/// bounds wherever the engine publishes both.
pub(super) const REDIS_PARAMETERS: &[ParameterRow] = &[
    ParameterRow(
        "acl-pubsub-default",
        "resetchannels",
        "The default pub/sub channel permissions a newly created user is given.",
        "string",
        "allchannels,resetchannels",
        true,
        "6.2.6",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-cycle-max",
        "75",
        "The largest share of CPU, as a percentage, active defragmentation may use.",
        "integer",
        "1-75",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-cycle-min",
        "5",
        "The smallest share of CPU, as a percentage, active defragmentation uses once it \
         starts.",
        "integer",
        "1-75",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-ignore-bytes",
        "104857600",
        "How much fragmentation, in bytes, is tolerated before active defragmentation starts.",
        "integer",
        "1048576-",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-max-scan-fields",
        "1000",
        "The largest number of fields of a hash, set, sorted set or list active \
         defragmentation processes in one scan.",
        "integer",
        "1-1000000",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-threshold-lower",
        "10",
        "The fragmentation, as a percentage, at which active defragmentation starts.",
        "integer",
        "1-100",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-defrag-threshold-upper",
        "100",
        "The fragmentation, as a percentage, at which active defragmentation uses its \
         largest share of CPU.",
        "integer",
        "1-100",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "active-expire-effort",
        "1",
        "How much effort the node spends expiring keys, from the least to the most eager.",
        "integer",
        "1-10",
        true,
        "7.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "activedefrag",
        "no",
        "Whether active memory defragmentation is enabled.",
        "string",
        "yes,no",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "activerehashing",
        "yes",
        "Whether the main dictionary is actively rehashed.",
        "string",
        "yes,no",
        false,
        "2.6.13",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "client-output-buffer-limit-pubsub-hard-limit",
        "33554432",
        "The output buffer size, in bytes, a pub/sub client is disconnected at.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "client-output-buffer-limit-pubsub-soft-limit",
        "8388608",
        "The output buffer size, in bytes, a pub/sub client may stay above only for \
         client-output-buffer-limit-pubsub-soft-seconds.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "client-output-buffer-limit-pubsub-soft-seconds",
        "60",
        "How long, in seconds, a pub/sub client may stay above the soft limit.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "cluster-enabled",
        "no",
        "Whether cluster mode is enabled for the cluster.",
        "string",
        "yes,no",
        false,
        "3.2.4",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "databases",
        "16",
        "The number of logical databases a node holds.",
        "integer",
        "1-",
        false,
        "2.6.13",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "hash-max-listpack-entries",
        "128",
        "The largest hash, in entries, kept in the memory-efficient listpack encoding.",
        "integer",
        "0-",
        true,
        "7.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "hash-max-ziplist-entries",
        "512",
        "The largest hash, in entries, kept in the memory-efficient ziplist encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "hash-max-ziplist-value",
        "64",
        "The largest hash value, in bytes, kept in the memory-efficient ziplist encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "hll-sparse-max-bytes",
        "3000",
        "The largest HyperLogLog, in bytes, kept in the memory-efficient sparse \
         representation.",
        "integer",
        "1-16000",
        true,
        "2.8.6",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "lazyfree-lazy-eviction",
        "no",
        "Whether an evicted key is freed in a background thread.",
        "string",
        "yes,no",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "lazyfree-lazy-expire",
        "no",
        "Whether an expired key is freed in a background thread.",
        "string",
        "yes,no",
        true,
        "4.0.9",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "list-max-ziplist-entries",
        "128",
        "The largest list, in entries, kept in the memory-efficient ziplist encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "lua-time-limit",
        "5000",
        "How long, in milliseconds, a Lua script may run before the node starts answering \
         other clients with BUSY.",
        "integer",
        "",
        false,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "maxclients",
        "65000",
        "The maximum number of client connections a node accepts.",
        "integer",
        "",
        false,
        "2.6.13",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "maxmemory-policy",
        "volatile-lru",
        "The eviction policy applied once the data stored reaches maxmemory.",
        "string",
        "volatile-lru,allkeys-lru,volatile-lfu,allkeys-lfu,volatile-random,allkeys-random,\
         volatile-ttl,noeviction",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "maxmemory-samples",
        "3",
        "How many keys an approximated eviction policy samples before it picks one.",
        "integer",
        "1-64",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "min-replicas-max-lag",
        "10",
        "How long, in seconds, a replica may lag before it stops counting towards \
         min-replicas-to-write.",
        "integer",
        "0-",
        true,
        "5.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "min-replicas-to-write",
        "0",
        "How many connected replicas the primary needs before it accepts writes.",
        "integer",
        "0-",
        true,
        "5.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "notify-keyspace-events",
        "",
        "The classes of keyspace event published to pub/sub.",
        "string",
        "",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "proto-max-bulk-len",
        "536870912",
        "The largest single protocol element, in bytes, a request may carry.",
        "integer",
        "1048576-536870912",
        true,
        "5.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "repl-backlog-size",
        "1048576",
        "The size, in bytes, of the replication backlog a disconnected replica resynchronises \
         from.",
        "integer",
        "16384-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "repl-backlog-ttl",
        "3600",
        "How long, in seconds, the primary keeps the replication backlog after the last \
         replica disconnects.",
        "integer",
        "0-",
        true,
        "2.8.6",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "reserved-memory-percent",
        "25",
        "The percentage of a node's memory reserved for non-data use.",
        "integer",
        "0-100",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "set-max-intset-entries",
        "512",
        "The largest set of integers, in entries, kept in the memory-efficient intset \
         encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "slowlog-log-slower-than",
        "10000",
        "How long, in microseconds, a command must take to be recorded in the slow log.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "slowlog-max-len",
        "128",
        "How many entries the slow log keeps.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "stream-node-max-bytes",
        "4096",
        "The largest stream macro node, in bytes.",
        "integer",
        "0-",
        true,
        "5.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "stream-node-max-entries",
        "100",
        "The largest stream macro node, in entries.",
        "integer",
        "0-",
        true,
        "5.0.0",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "tcp-keepalive",
        "300",
        "How often, in seconds, a TCP ACK is sent to an idle client, or 0 to send none.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "timeout",
        "0",
        "How long, in seconds, a client may stay idle before it is closed, or 0 to never \
         close one.",
        "integer",
        "0,20-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "zset-max-ziplist-entries",
        "128",
        "The largest sorted set, in entries, kept in the memory-efficient ziplist encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "zset-max-ziplist-value",
        "64",
        "The largest sorted set value, in bytes, kept in the memory-efficient ziplist \
         encoding.",
        "integer",
        "0-",
        true,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        false,
    ),
    ParameterRow(
        "maxmemory",
        "",
        "The memory, in bytes, available to the data stored on a node.",
        "integer",
        "",
        false,
        "2.6.13",
        CHANGE_TYPE_IMMEDIATE,
        // The node's own capacity, so the value is per cache node type.
        true,
    ),
];

/// The parameters ElastiCache publishes for its Memcached engine.
///
/// Source: the parameter tables of the ElastiCache User Guide's "Memcached
/// specific parameters" section, read the same way as [`REDIS_PARAMETERS`].
pub(super) const MEMCACHED_PARAMETERS: &[ParameterRow] = &[
    ParameterRow(
        "backlog_queue_limit",
        "1024",
        "How many connections may wait in the backlog queue.",
        "integer",
        "",
        false,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "binding_protocol",
        "auto",
        "The protocol a client connection is read as.",
        "string",
        "auto,ascii,binary",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "cas_disabled",
        "0",
        "Whether check-and-set (CAS) operations are disabled.",
        "boolean",
        "0,1",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "chunk_size",
        "48",
        "The smallest chunk size, in bytes, a key-value pair is stored in.",
        "integer",
        "1-",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "chunk_size_growth_factor",
        "1.25",
        "The growth factor between the chunk sizes of consecutive slab classes.",
        "float",
        "1.0-",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "disable_flush_all",
        "0",
        "Whether the flush_all command is refused.",
        "boolean",
        "0,1",
        true,
        "1.4.14",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "error_on_memory_exhausted",
        "1",
        "Whether a store with no memory left is answered with an error instead of evicting.",
        "boolean",
        "0,1",
        true,
        "1.4.14",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "hashpower_init",
        "0",
        "The initial size of the hash table, as a power of two.",
        "integer",
        "0-",
        true,
        "1.4.14",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "large_memory_pages",
        "0",
        "Whether large memory pages are used.",
        "boolean",
        "0,1",
        false,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "lock_down_paged_memory",
        "0",
        "Whether paged memory is locked down.",
        "boolean",
        "0,1",
        false,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "max_item_size",
        "1048576",
        "The largest item, in bytes, that may be stored.",
        "integer",
        "1024-",
        true,
        "1.4.14",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "maxconns_fast",
        "0",
        "Whether a connection over the limit is closed immediately rather than queued.",
        "boolean",
        "0,1",
        true,
        "1.4.14",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "memcached_connections_overhead",
        "100",
        "The memory, in megabytes, reserved for connection overhead.",
        "integer",
        "0-",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "requests_per_event",
        "20",
        "How many requests one connection is served per event before the next is served.",
        "integer",
        "1-",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "slab_automove",
        "1",
        "How a slab page is moved between slab classes.",
        "integer",
        "0,1,2",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "slab_chunk_max",
        "524288",
        "The largest chunk size, in bytes, a slab page is divided into.",
        "integer",
        "1-",
        true,
        "1.5.10",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "slab_reassign",
        "1",
        "Whether a slab page may be reassigned between slab classes.",
        "boolean",
        "0,1",
        true,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        false,
    ),
    ParameterRow(
        "max_cache_memory",
        "",
        "The memory, in megabytes, available to the cache on a node.",
        "integer",
        "",
        false,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        // The node's own capacity, so the value is per cache node type.
        true,
    ),
    ParameterRow(
        "num_threads",
        "",
        "How many threads a node serves requests with.",
        "integer",
        "",
        false,
        "1.4.5",
        CHANGE_TYPE_REQUIRES_REBOOT,
        // One per vCPU of the node type, so the value is per cache node type.
        true,
    ),
];

/// The memory, in mebibytes, and the vCPU count the emulator attributes to each
/// cache node SIZE — the last segment of a `cache.<family>.<size>` node type.
///
/// Like [`OFFERING_NODE_TYPES`]' hourly rates these are the EMULATOR's own
/// scale, not a claim about AWS's published node capacities: AWS publishes those
/// outside this API and the values below are derived per node size rather than
/// copied from it. They DO reach the wire — `DescribeEngineDefaultParameters`
/// reports `maxmemory` / `max_cache_memory` / `num_threads` per cache node type
/// from them — so read them as this emulator's approximations of those
/// capacities, not as AWS's numbers. They rise with the
/// node size, so the node-type-specific values this catalog reports keep the
/// ordering a caller picks a node type for, and they are fixed, so every read
/// reports the same ones.
pub(super) const NODE_SIZES: [(&str, i64, i64); 7] = [
    ("micro", 512, 2),
    ("small", 1_024, 2),
    ("medium", 3_072, 2),
    ("large", 6_144, 2),
    ("xlarge", 12_288, 4),
    ("2xlarge", 24_576, 8),
    ("4xlarge", 49_152, 16),
];

/// The memory (mebibytes) and vCPU count of `cache_node_type`, or `None` when
/// its size is not one [`NODE_SIZES`] carries.
pub(super) fn node_type_capacity(cache_node_type: &str) -> Option<(i64, i64)> {
    let size = cache_node_type.split('.').next_back()?;
    NODE_SIZES
        .iter()
        .find(|(name, _, _)| *name == size)
        .map(|(_, memory_mib, vcpus)| (*memory_mib, *vcpus))
}

/// The value the parameter `name` takes on a node type of this capacity, or
/// `None` when that parameter's value does not depend on the node type.
///
/// This is the one place a node-type-specific parameter's value is computed, so
/// the two catalogs cannot derive it differently.
pub(super) fn node_type_specific_value(name: &str, memory_mib: i64, vcpus: i64) -> Option<String> {
    /// Mebibytes to bytes.
    const BYTES_PER_MIB: i64 = 1024 * 1024;
    match name {
        // The data a Redis OSS / Valkey node may hold, in bytes.
        "maxmemory" => Some((memory_mib * BYTES_PER_MIB).to_string()),
        // The cache a Memcached node may hold, in megabytes.
        "max_cache_memory" => Some(memory_mib.to_string()),
        // One Memcached worker thread per vCPU.
        "num_threads" => Some(vcpus.to_string()),
        _ => None,
    }
}

/// One engine default parameter of one cache parameter group family, as this
/// emulator persists it.
///
/// A parameter whose value depends on the cache node type carries that value per
/// node type in `node_type_values` and no single `value`, which is what makes it
/// a `CacheNodeTypeSpecificParameter` rather than a `Parameter` on the wire.
#[derive(Debug, Clone)]
pub struct ParameterRecord {
    /// `Parameter$ParameterName`.
    pub name: String,
    /// `Parameter$ParameterValue` — the engine's default for it, `None` for a
    /// parameter whose value is per cache node type.
    pub value: Option<String>,
    /// `Parameter$Description`.
    pub description: String,
    /// `Parameter$DataType`.
    pub data_type: String,
    /// `Parameter$AllowedValues`, absent where the engine constrains the value to
    /// no published set.
    pub allowed_values: Option<String>,
    /// `Parameter$IsModifiable`.
    pub is_modifiable: bool,
    /// `Parameter$MinimumEngineVersion`.
    pub minimum_engine_version: String,
    /// `Parameter$ChangeType`: [`CHANGE_TYPE_IMMEDIATE`] or
    /// [`CHANGE_TYPE_REQUIRES_REBOOT`].
    pub change_type: String,
    /// `(CacheNodeType, Value)` for every node type this service publishes —
    /// non-empty exactly for a parameter whose value is per cache node type.
    pub node_type_values: Vec<(String, String)>,
}

impl ParameterRecord {
    /// Whether this parameter's value depends on the cache node type.
    pub fn is_node_type_specific(&self) -> bool {
        !self.node_type_values.is_empty()
    }

    /// The key this parameter is held and PAGED under.
    ///
    /// `EngineDefaults` carries two parameter lists and ONE `Marker`, so both are
    /// paged as one sequence: the plain parameters first, then the
    /// node-type-specific ones, each ordered by name. Prefixing the name with the
    /// list's position is what puts the whole catalog in the single total order a
    /// `Marker` resumes from.
    pub fn page_key(&self) -> String {
        parameter_page_key(&self.name, self.is_node_type_specific())
    }

    /// This record as the modelled `Parameter`.
    pub fn to_modeled(&self) -> Parameter {
        Parameter {
            parameter_name: Some(self.name.clone()),
            parameter_value: self.value.clone(),
            description: Some(self.description.clone()),
            source: Some(PARAMETER_SOURCE_SYSTEM.to_string()),
            data_type: Some(self.data_type.clone()),
            allowed_values: self.allowed_values.clone(),
            is_modifiable: Some(self.is_modifiable),
            minimum_engine_version: Some(self.minimum_engine_version.clone()),
            change_type: Some(self.change_type.clone()),
        }
    }

    /// The `Parameter$Source` a group reporting this parameter answers under:
    /// the customer's when the group holds a value for it, the service's
    /// otherwise; see [`CacheParameterGroupRecord::parameter_values`].
    ///
    /// A parameter a customer cannot set is never the customer's, whatever value
    /// the group holds for it: a parameter whose value is per cache node type
    /// takes the node's own capacity, and one the engine fixes
    /// (`IsModifiable: false`) can only carry a value the SERVICE put there —
    /// the cluster-mode flag of a cluster-mode-on default group, say.
    pub fn source_in_group(&self, set_value: Option<&str>) -> &'static str {
        if set_value.is_some() && !self.is_node_type_specific() && self.is_modifiable {
            PARAMETER_SOURCE_USER
        } else {
            PARAMETER_SOURCE_SYSTEM
        }
    }

    /// This record as the modelled `Parameter` of a group that holds
    /// `set_value` for it: the value the group was given, reported as the
    /// customer's, or the engine's own default reported as the service's.
    pub fn to_modeled_in_group(&self, set_value: Option<&str>) -> Parameter {
        let mut parameter = self.to_modeled();
        parameter.source = Some(self.source_in_group(set_value).to_string());
        if let Some(value) = set_value {
            parameter.parameter_value = Some(value.to_string());
        }
        parameter
    }

    /// This record as the modelled `CacheNodeTypeSpecificParameter`, for a
    /// parameter whose value is per cache node type.
    pub fn to_node_type_specific(&self) -> CacheNodeTypeSpecificParameter {
        CacheNodeTypeSpecificParameter {
            parameter_name: Some(self.name.clone()),
            description: Some(self.description.clone()),
            source: Some(PARAMETER_SOURCE_SYSTEM.to_string()),
            data_type: Some(self.data_type.clone()),
            allowed_values: self.allowed_values.clone(),
            is_modifiable: Some(self.is_modifiable),
            minimum_engine_version: Some(self.minimum_engine_version.clone()),
            cache_node_type_specific_values: Some(
                self.node_type_values
                    .iter()
                    .map(|(cache_node_type, value)| CacheNodeTypeSpecificValue {
                        cache_node_type: Some(cache_node_type.clone()),
                        value: Some(value.clone()),
                    })
                    .collect(),
            ),
            change_type: Some(self.change_type.clone()),
        }
    }
}

/// The key the parameter `name` is held and paged under, given which of
/// `EngineDefaults`' two lists reports it; see [`ParameterRecord::page_key`].
///
/// A free function because a lookup BY NAME has to build the same key without
/// holding the record it is looking for
/// ([`ComAmazonawsElasticacheAmazonelasticachev9State::family_parameter`]), and
/// two spellings of one key would let a write land where no read finds it.
pub(super) fn parameter_page_key(name: &str, node_type_specific: bool) -> String {
    let list = if node_type_specific { 2 } else { 1 };
    format!("{list}:{name}")
}

/// `Parameter$DataType`'s spellings in the catalogs this service publishes: the
/// engine's own types, which are what decide whether a value a request asks to
/// set is a number at all and how it is compared against a published range.
pub const PARAMETER_DATA_TYPE_INTEGER: &str = "integer";
/// See [`PARAMETER_DATA_TYPE_INTEGER`].
pub const PARAMETER_DATA_TYPE_FLOAT: &str = "float";
/// See [`PARAMETER_DATA_TYPE_INTEGER`].
pub const PARAMETER_DATA_TYPE_BOOLEAN: &str = "boolean";
/// See [`PARAMETER_DATA_TYPE_INTEGER`].
pub const PARAMETER_DATA_TYPE_STRING: &str = "string";

/// `value` as the number a parameter of `data_type` takes, or `None` when it is
/// not one.
///
/// An `integer` (and a `boolean`, which the engines write as `0` / `1`) takes
/// only a whole number, so `1.5` is not one of its values; a `float` takes any
/// finite decimal number. A `string` parameter has no numeric reading at all.
/// The number is used only to COMPARE the value against a published range — the
/// text a request sent is what gets stored, so a value's own precision is never
/// re-rendered from this.
pub(super) fn parameter_number(data_type: &str, value: &str) -> Option<f64> {
    let finite = value.parse::<f64>().ok().filter(|number| number.is_finite());
    match data_type {
        // The engines write a boolean as the flag `0` or `1`, and nothing else
        // is a value of that type, whether or not the row also publishes the
        // pair as its allowed values.
        PARAMETER_DATA_TYPE_BOOLEAN => finite.filter(|number| *number == 0.0 || *number == 1.0),
        PARAMETER_DATA_TYPE_INTEGER if value.parse::<i64>().is_err() => None,
        PARAMETER_DATA_TYPE_INTEGER | PARAMETER_DATA_TYPE_FLOAT => finite,
        _ => None,
    }
}

/// The bounds of `term` when it is a numeric RANGE of a published
/// `Parameter$AllowedValues` list — `<lo>-<hi>`, or `<lo>-` where the engine
/// bounds only the bottom — and `None` when it is a literal value.
///
/// A term is a range exactly when what precedes the `-` is a number, which is
/// what tells `1-75` apart from a literal that happens to carry a hyphen
/// (`volatile-lru`); see [`ParameterRow`]'s allowed-values column. A bound is
/// therefore never negative — every published bound is a count, a size or a
/// percentage — and a term written with one would read as a literal.
pub(super) fn allowed_value_range(term: &str) -> Option<(f64, Option<f64>)> {
    let (low, high) = term.split_once('-')?;
    let low: f64 = low.parse().ok()?;
    if high.is_empty() {
        return Some((low, None));
    }
    Some((low, Some(high.parse().ok()?)))
}

/// Whether the engine takes `value` for the parameter `parameter` describes, or
/// the reason it does not.
///
/// The published catalog row IS the rule: a parameter the engine or the node
/// type fixes (`Parameter$IsModifiable: false`, and every node-type-specific
/// one) takes no customer value at all; a numeric parameter takes only a value
/// of its `Parameter$DataType`; and a parameter that publishes
/// `Parameter$AllowedValues` takes only a value that list covers — one of its
/// literal values, or a number inside one of its ranges. A parameter that
/// publishes no allowed values is constrained to no published set, so only its
/// data type is checked.
///
/// The refusal says which of those the value fell outside: the data type it is
/// not a value of, or the published set it is not in.
///
/// The one place a value a request asks to SET is held against the catalog, so
/// every operation that writes a parameter value applies the same rule.
pub fn check_parameter_value(parameter: &ParameterRecord, value: &str) -> Result<(), String> {
    check_parameter_modifiable(parameter)?;
    let number = parameter_number(&parameter.data_type, value);
    // A value that is not one of the parameter's own DATA TYPE is refused for
    // that, naming the type it is not a value of and the value that is not one:
    // it is outside every published set the engine could name, so the set is
    // not what the client got it wrong about.
    if parameter.data_type != PARAMETER_DATA_TYPE_STRING && number.is_none() {
        return Err(format!("invalid {} value: {value}", parameter.data_type));
    }
    let Some(allowed) = parameter.allowed_values.as_deref() else {
        // Constrained to no published set: the value is held only to the
        // parameter's own data type, which it already satisfies.
        return Ok(());
    };
    for term in allowed.split(',') {
        let covered = match allowed_value_range(term) {
            Some((low, high)) => number
                .is_some_and(|number| number >= low && high.is_none_or(|high| number <= high)),
            None => term == value,
        };
        if covered {
            return Ok(());
        }
    }
    // The refusal names the published set the value is outside of, which is the
    // parameter's own `Parameter$AllowedValues` as a read reports it: the client
    // that asked for the value learns which values it may ask for instead.
    Err(format!("{INVALID_PARAMETER_VALUE}, allowed values are:{allowed}"))
}

/// Whether a customer may change what the parameter `parameter` describes is
/// worth at all, or the reason the engine does not let them.
///
/// A parameter the engine fixes (`Parameter$IsModifiable: false`) and one whose
/// value is per cache node type both take whatever value the service or the
/// node's own capacity gives them, so neither a request SETTING a value nor one
/// RESETTING it to the engine default asks for a change that can be made. The
/// one place that rule is stated, so [`check_parameter_value`] and every
/// operation that only names a parameter word the same refusal.
pub fn check_parameter_modifiable(parameter: &ParameterRecord) -> Result<(), String> {
    if !parameter.is_modifiable || parameter.is_node_type_specific() {
        return Err(format!(
            "The parameter {} cannot be modified.",
            parameter.name
        ));
    }
    Ok(())
}

/// How ElastiCache opens the refusal of a value a parameter does not take; see
/// [`check_parameter_value`], which is where the published set the value is
/// outside of is appended to it.
pub(super) const INVALID_PARAMETER_VALUE: &str = "invalid parameter value";

/// How ElastiCache refuses a request that names a parameter the group's family
/// publishes none of: the refusal names the parameter that was asked for, which
/// is the one thing the client got wrong.
///
/// The one place that refusal is worded, so a request SETTING such a parameter
/// and one RESETTING it answer a name the published catalog does not carry
/// identically.
pub fn unknown_parameter_message(name: &str) -> String {
    format!("Could not find parameter with name: {name}")
}

/// How many parameters one request may change: "A maximum of 20 parameters may
/// be modified per request"
/// (`ModifyCacheParameterGroup$ParameterNameValues`), which
/// `ResetCacheParameterGroup` names the same bound for.
pub const MAX_PARAMETERS_PER_REQUEST: usize = 20;

/// Every engine default parameter of `family`, keyed by
/// [`ParameterRecord::page_key`], or `None` when no published family carries
/// that name.
///
/// A family reports the rows of its engine's catalog whose minimum engine
/// version its line covers, so two families of one engine differ by exactly the
/// parameters the later versions introduced.
pub fn family_default_parameters(family: &str) -> Option<BTreeMap<String, ParameterRecord>> {
    let (engine, line) = family_line(family)?;
    let rows = if engine == PARAMETER_ENGINE_MEMCACHED {
        MEMCACHED_PARAMETERS
    } else {
        REDIS_PARAMETERS
    };
    let mut parameters = BTreeMap::new();
    for row in rows {
        let ParameterRow(
            name,
            value,
            description,
            data_type,
            allowed_values,
            is_modifiable,
            minimum_engine_version,
            change_type,
            node_type_specific,
        ) = *row;
        if version_rank(minimum_engine_version) > line {
            continue;
        }
        // The node types a node-type-specific parameter carries a value for are
        // [`OFFERING_NODE_TYPES`]' — the one list of cache node types this crate
        // publishes, so a parameter and an offering cannot disagree about which
        // node types exist.
        let node_type_values: Vec<(String, String)> = if node_type_specific {
            let values: Vec<(String, String)> = OFFERING_NODE_TYPES
                .iter()
                .filter_map(|(cache_node_type, _)| {
                    let (memory_mib, vcpus) = node_type_capacity(cache_node_type)?;
                    let value = node_type_specific_value(name, memory_mib, vcpus)?;
                    Some(((*cache_node_type).to_string(), value))
                })
                .collect();
            // A row flagged node-type-specific that no node type has a value
            // for would be reported as a plain parameter with NO value at all,
            // so it is refused here instead: it means a catalog row and
            // [`node_type_specific_value`] (or [`NODE_SIZES`] and a node type
            // added to [`OFFERING_NODE_TYPES`]) disagree, which is a bug in this
            // module rather than anything a request can ask for — the same
            // reading as [`offering_rate_share`]'s `expect`s.
            assert!(
                values.len() == OFFERING_NODE_TYPES.len(),
                "the node-type-specific parameter {name} must carry a value for every node \
                 type of OFFERING_NODE_TYPES, got {}",
                values.len()
            );
            values
        } else {
            Vec::new()
        };
        let record = ParameterRecord {
            name: name.to_string(),
            value: (!node_type_specific).then(|| value.to_string()),
            description: description.to_string(),
            data_type: data_type.to_string(),
            allowed_values: (!allowed_values.is_empty()).then(|| allowed_values.to_string()),
            is_modifiable,
            minimum_engine_version: minimum_engine_version.to_string(),
            change_type: change_type.to_string(),
            node_type_values,
        };
        parameters.insert(record.page_key(), record);
    }
    Some(parameters)
}

/// The engine default parameters of every published family, keyed by family.
///
/// Seeded into
/// [`ComAmazonawsElasticacheAmazonelasticachev9State::engine_default_parameters`]
/// when the store is created: the engine's defaults are the service's own
/// catalog, which exists before anything has been created and which no operation
/// creates or removes an entry from.
pub fn published_engine_default_parameters() -> BTreeMap<String, BTreeMap<String, ParameterRecord>>
{
    PARAMETER_GROUP_FAMILIES
        .iter()
        .filter_map(|family| {
            family_default_parameters(family).map(|parameters| ((*family).to_string(), parameters))
        })
        .collect()
}
