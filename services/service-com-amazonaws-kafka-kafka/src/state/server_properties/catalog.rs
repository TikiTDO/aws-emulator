// SPDX-License-Identifier: CC0-1.0

//! The broker properties an Amazon MSK configuration may set.
//!
//! Written out rather than derived: the set is the service's published surface
//! (the Amazon MSK Developer Guide, "Amazon MSK configuration properties" /
//! "Custom MSK configurations", in that document's alphabetical order), and
//! nothing in this emulator's state implies it. A property MSK reserves for
//! itself — broker identity, the listeners, the log directories, the
//! ZooKeeper/KRaft wiring — is absent from it by construction, which is what
//! refuses `broker.id` and every other property MSK configures a cluster's
//! brokers with itself.
//!
//! Its own file so the reading in [`super`] keeps to the admission and the
//! refusal: this one is data.

/// The broker properties an Amazon MSK configuration may set.
pub const CONFIGURABLE_PROPERTIES: [&str; 77] = [
    "allow.everyone.if.no.acl.found",
    "auto.create.topics.enable",
    "auto.leader.rebalance.enable",
    "compression.type",
    "connections.max.idle.ms",
    "connections.max.reauth.ms",
    "default.replication.factor",
    "delete.topic.enable",
    "fetch.max.bytes",
    "follower.replication.throttled.replicas",
    "group.initial.rebalance.delay.ms",
    "group.max.session.timeout.ms",
    "group.max.size",
    "group.min.session.timeout.ms",
    "leader.imbalance.check.interval.seconds",
    "leader.imbalance.per.broker.percentage",
    "leader.replication.throttled.replicas",
    "log.cleaner.backoff.ms",
    "log.cleaner.dedupe.buffer.size",
    "log.cleaner.delete.retention.ms",
    "log.cleaner.io.buffer.load.factor",
    "log.cleaner.io.buffer.size",
    "log.cleaner.io.max.bytes.per.second",
    "log.cleaner.min.cleanable.ratio",
    "log.cleaner.min.compaction.lag.ms",
    "log.cleaner.threads",
    "log.cleanup.policy",
    "log.flush.interval.messages",
    "log.flush.interval.ms",
    "log.flush.offset.checkpoint.interval.ms",
    "log.index.interval.bytes",
    "log.index.size.max.bytes",
    "log.local.retention.bytes",
    "log.local.retention.ms",
    "log.message.downconversion.enable",
    "log.message.format.version",
    "log.message.timestamp.after.max.ms",
    "log.message.timestamp.before.max.ms",
    "log.message.timestamp.difference.max.ms",
    "log.message.timestamp.type",
    "log.preallocate",
    "log.retention.bytes",
    "log.retention.hours",
    "log.retention.minutes",
    "log.retention.ms",
    "log.roll.jitter.ms",
    "log.roll.ms",
    "log.segment.bytes",
    "log.segment.delete.delay.ms",
    "max.connections",
    "max.connections.per.ip",
    "max.connections.per.ip.overrides",
    "max.incremental.fetch.session.cache.slots",
    "message.max.bytes",
    "min.insync.replicas",
    "num.io.threads",
    "num.network.threads",
    "num.partitions",
    "num.recovery.threads.per.data.dir",
    "num.replica.fetchers",
    "offsets.retention.minutes",
    "offsets.topic.replication.factor",
    "remote.log.msk.disable.policy",
    "replica.fetch.max.bytes",
    "replica.fetch.response.max.bytes",
    "replica.fetch.wait.max.ms",
    "replica.lag.time.max.ms",
    "replica.selector.class",
    "replica.socket.receive.buffer.bytes",
    "socket.receive.buffer.bytes",
    "socket.request.max.bytes",
    "socket.send.buffer.bytes",
    "transaction.max.timeout.ms",
    "transaction.state.log.min.isr",
    "transaction.state.log.replication.factor",
    "transactional.id.expiration.ms",
    "unclean.leader.election.enable",
];
