# Shared payload store (B2)

`service_common::payload_store` owns immutable bytes, upload checkpoints and **one**
transactional reference catalog. It does not authorize requests, disable signatures,
provision an estate volume or create a store from environment variables.

## Boot and ownership

Boot supplies `StoreConfig::memory(namespace,generation,limits)` or
`StoreConfig::volume(selected_local_root,namespace,generation,limits)` to async
`PayloadStore::open_store`. The root must already exist and be an admitted active
local filesystem (NAS/archive selection is not inferred). The store creates only
its encoded child namespace. An existing child needs the exact ownership envelope
and catalog identity; incomplete/foreign initialization is refused, never reset.

Install ONE cloneable handle in the engine's existing
`state::<PayloadStoreBinding>()` slot **before** restoring payload-backed service
indexes and opening listeners. `PayloadStoreBinding::default()` is None, not an
automatically-created memory fallback. Services construct/read the same handle;
`service-common` does not depend on the engine or server boot owner.

Namespaces and store generations are immutable. Reuse the same selected values on
restart. Per-resource `OwnerKey(service,resource,generation)` separates a recreated
bucket/repository from its predecessor. IDs/handles/cursors/receipts have private
fields and are scoped to the store. Keys and client names are SQLite values, never
host paths. Public results/errors contain no host path or underlying credential/error.

## Writer and request protocol

1. `begin(StageSpec)` yields one writer lease and a store-created UploadId. An optional
   bounded upload_key lets the service find it after restart via `lookup_upload_key`;
   `lookup_upload` resolves opaque IDs only, so aliases cannot shadow another ID. For protocol initiation,
   publish its upload-index record before returning the protocol's successful ID.
2. `append(offset,Bytes)` accepts only the exact current offset, within
   `stage.max_append_bytes()` and finite stage/storage limits. Do not collect a body
   into Vec first. In-flight jobs retain their lease even if their awaiting future is
   cancelled, so another resume cannot race an entered append.
3. After the **actual original-wire receipt** and service checksum/policy checks,
   call `checkpoint(ProtocolMetadata{schema,version,bytes})`. It fsyncs the stage,
   then persists prefix length/SHA256/revision plus bounded metadata. It is not an
   authentication boolean or service-success publication.
4. A request failure may `rollback_to(last_durable_checkpoint)`. Rollback refuses
   older/different checkpoints: it never discards an already advanced durable ACK.
   Dropping a handle deletes nothing. Explicit `resume(id,checkpoint)` validates
   that prefix and truncates only its unacknowledged suffix; boot itself does not.
5. `seal(self,IntegrityExpectation)` validates actual stored count/SHA256 and creates
   immutable content. It is NOT a wire/service authority grant. A digest mismatch
   consumes only the lease; lookup_upload still exposes the prior checkpoint for
   resume/retry. A successful seal is retained in lookup_upload.sealed; after an
   unrelated commit Conflict it can be recommitted without reopening body writes.
6. Publish via `ReferenceTransaction`, grouping object/layer/tag/manifest records
   and optional exact-checkpoint stage retirements. Any/Absent/Exact are explicit;
   Absent means no live record (a tombstone can be replaced). Records have closed
   service schema versions, bounded metadata and a set of scoped payload references.
   Retirement also supports explicit cancellation of an UNSEALED checkpointed
   upload; bytes are not immediately reclaimed. `abort()` only removes an
   unacknowledged/unsealed stage. No request Drop erases an earlier ACK.

For resumed PATCH, B1's receipt digest/count describes THIS decoded request suffix,
not the full stage. The consumer sink tracks suffix hash/count while calling append;
compare those to the terminal receipt. `stage.progress()` is the full prefix+suffix
length/digest; final seal checks total content. B1's sink trait is adapted by the
consumer, not a fabricated server/store authentication shortcut.

## Persistence, retries, recovery

Both modes use the same rusqlite catalog; memory has ephemeral SQLite/content.
Volume uses immutable CAS files and SQLite DELETE journal with synchronous EXTRA,
fullfsync and exclusive catalog ownership. Content is fsynced/installed and its
directory synced before sealed catalog publication. Record/ref groups plus a stable
CommitId receipt commit in ONE SQL transaction. There is no competing snapshot/index
file. Record envelopes reuse Art's `aws-emulator.state`, service/namespace/version,
kind/key/meta fields with a reference set; service snapshots must not duplicate their
authoritative payload records. Rebuild service indexes through record/list_records.

`new_commit_id()` mints correlation before calling commit. Keep that ID across timeout
or response loss; `commit_id(opaque)` restores correlation on restart. Reusing an
already-committed ID with different changes refuses. `CommitReceipt.references` is
that original committed set, not today's overwritten/deleted records.

`recover` returns Committed, **InProgress**, or NotCommitted. A transient admission
token is registered before the blocking job is queued and held until actual completion;
no pending original job may masquerade as NotCommitted merely because its awaiting
future was cancelled. SQLite is the only durable outcome authority. Unknown outcomes
retain IDs/refs; do not abort/release from a timeout or OutcomeUnknown error. An unhealthy
connection still in a transaction refuses reads/recovery rather than exposing an
uncommitted view. Memory receipts explicitly say Ephemeral, never crash-durable.

No GC, reset, namespace wipe, adoption of unknown files or destructive boot cleanup.
If a crash retained an unacknowledged orphan beyond the data-byte budget, boot
reports `inventory.data_quota_exceeded` but preserves access to prior durable records;
new data writes refuse rather than deleting bytes or hiding acknowledged objects.
Unknown/abandoned files are metadata-inventoried and charged to data quota. Corrupt
sealed content is integrity/degraded, not a normal missing/empty object. Unaffected
records remain readable. Commit recovery still reports a committed transaction even
if its referenced content later degrades; that is not a fresh integrity guarantee.

## Budgets / costs that do NOT disappear

All limits are boot-supplied and finite. `max_storage_bytes` covers retained stage,
CAS and temporary data-file bytes; `max_catalog_bytes` bounds SQLite pages separately.
The DELETE rollback journal can add a catalog-sized transient copy; filesystem inode,
directory and SQLite fixed bookkeeping overhead are not payload bytes. Plan disk for
these separately, not just the incoming object. Schema/metadata/transaction/page/ref/
stage/record/CommitId counts are bounded. `max_data_files` is the SAME admission
ceiling at boot and write time; CAS installation reserves temporary plus final
names, so a successful write cannot create a namespace its next boot rejects. Operation and read-lease permits bound queued
IO and open readers. Busy is a refusal/backpressure signal, not a fallback store.

Sealing copies a stage to a separate immutable CAS file so resume can never mutate a
hard-link alias to published content. This costs an extra payload-sized disk allocation
and bounded IO; quota can refuse that peak. Retired stage data remains until a future
explicit reclamation policy. `compose` bounded-copies sealed parts, never one whole Vec;
its part count and total size are checked before creating the candidate.

Volume open audits content/checkpoints before service restore, without truncation or
GC. Range open currently verifies immutable content with bounded-buffer full hashing
before returning a seeked lease. This is intentionally not an O(range-length) IO claim;
chunk-integrity indexing could remove that extra read later. Normal record publication
does not rehash the same immutable referenced payload for each tag/shard: seal/boot
established availability, and actual readers verify content. Read leases survive
concurrent reference removal. A payload reader exposes AsyncRead/next_chunk, no disk
path or whole-buffer convenience API; chunks are at most64KiB and bounded by config.

## First validation

Only synthetic temp roots in unit fixtures; no estate volume/environment selection.
Focused cases cover memory+volume publish/ranges/read leases; exact offsets;
checkpoint rollback/restart/partial writes; failed seal and sealed-conflict retry;
CAS-before-catalog fault; atomic record conflicts; commit-response-loss/restart;
actual cancelled queued commit/InProgress; owner/namespace separation; bounded
pagination/stale cursor and byte-limited row materialization; same boot/write file quota; quotas/unsupported checksum; composition/empty ranges;
corruption and retained unknown files. Fault seams and namespace path introspection
are cfg(test) only. These are authored fixtures, not test results.

Sol's integration must additionally exercise real productionTLS + embedded service
adapters, real CDK/Docker/SDK body shapes, memory slope, forced process death and
restart, and actual service metadata reconstruction before acknowledging full slice.
No library unit result substitutes for that runtime/persistence witness.
