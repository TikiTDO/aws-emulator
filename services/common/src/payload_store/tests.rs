// SPDX-License-Identifier: CC0-1.0

use super::backend::Fault;
use super::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::future::Future;

fn limits() -> StoreLimits {
    StoreLimits {
        max_storage_bytes: 16 * 1024 * 1024,
        max_catalog_bytes: 1024 * 1024,
        max_stage_bytes: 4 * 1024 * 1024,
        max_chunk_bytes: 64 * 1024,
        max_metadata_bytes: 64 * 1024,
        max_records_per_transaction: 32,
        max_references_per_record: 64,
        max_page_records: 32,
        max_page_bytes: 128 * 1024,
        max_stages: 128,
        max_records: 128,
        max_commits: 128,
        max_transaction_bytes: 1024 * 1024,
        max_inflight_operations: 16,
        max_read_leases: 8,
        max_data_files: 512,
    }
}
fn owner() -> OwnerKey {
    OwnerKey::new("fixture", "resource", "generation-1").unwrap()
}
fn spec() -> StageSpec {
    StageSpec {
        owner: owner(),
        upload_key: None,
        expected_length: None,
        max_bytes: 4 * 1024 * 1024,
    }
}
fn metadata() -> ProtocolMetadata {
    ProtocolMetadata::new(
        "fixture-upload",
        1,
        Bytes::from_static(b"part acknowledged"),
    )
    .unwrap()
}
fn sha(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest(Sha256::digest(bytes).into())
}
fn key(name: &str) -> RecordKey {
    RecordKey::new(owner(), Bytes::copy_from_slice(name.as_bytes())).unwrap()
}
fn put(name: &str, condition: RecordCondition, handles: Vec<PayloadHandle>) -> RecordChange {
    RecordChange {
        key: key(name),
        condition,
        value: Some(RecordData {
            schema_version: 1,
            metadata: json!({"type":"fixture"}),
            references: handles,
        }),
    }
}
fn transaction(store: &PayloadStore, changes: Vec<RecordChange>) -> ReferenceTransaction {
    ReferenceTransaction {
        id: store.new_commit_id(),
        owner: owner(),
        changes,
        retire: Vec::new(),
    }
}
async fn memory() -> PayloadStore {
    PayloadStore::open_store(StoreConfig::memory("test", "generation", limits()).unwrap())
        .await
        .unwrap()
}
async fn seal_bytes(store: &PayloadStore, bytes: &[u8]) -> SealedPayload {
    let mut stage = store.begin(spec()).await.unwrap();
    let mut offset = 0;
    for bytes in bytes.chunks(stage.max_append_bytes()) {
        offset = stage
            .append(offset, Bytes::copy_from_slice(bytes))
            .await
            .unwrap();
    }
    stage
        .seal(IntegrityExpectation {
            length: Some(bytes.len() as u64),
            sha256: Some(sha(bytes)),
        })
        .await
        .unwrap()
}
async fn collected(mut reader: PayloadReader) -> Vec<u8> {
    let mut bytes = Vec::new();
    while let Some(chunk) = reader.next_chunk().await.unwrap() {
        bytes.extend_from_slice(&chunk);
    }
    bytes
}
fn volume_config(root: &tempfile::TempDir) -> StoreConfig {
    StoreConfig::volume(root.path().to_path_buf(), "test", "generation", limits()).unwrap()
}

#[tokio::test]
async fn ordered_checkpoint_rollback_seal_atomic_publish_range_and_live_read_lease() {
    for volume in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = if volume {
            PayloadStore::open_store(volume_config(&root))
                .await
                .unwrap()
        } else {
            memory().await
        };
        let mut stage = store.begin(spec()).await.unwrap();
        assert_eq!(
            stage.append(0, Bytes::from_static(b"abc")).await.unwrap(),
            3
        );
        assert_eq!(
            stage
                .append(1, Bytes::from_static(b"wrong"))
                .await
                .unwrap_err(),
            StoreError::OffsetMismatch {
                expected: 3,
                supplied: 1
            }
        );
        let checkpoint = stage.checkpoint(metadata()).await.unwrap();
        stage
            .append(3, Bytes::from_static(b"discard"))
            .await
            .unwrap();
        stage.rollback_to(checkpoint.clone()).await.unwrap();
        assert_eq!(stage.progress().await.unwrap().sha256, sha(b"abc"));
        stage.append(3, Bytes::from_static(b"def")).await.unwrap();
        let sealed = stage
            .seal(IntegrityExpectation {
                length: Some(6),
                sha256: Some(sha(b"abcdef")),
            })
            .await
            .unwrap();
        let mut tx = transaction(
            &store,
            vec![put(
                "object",
                RecordCondition::Absent,
                vec![sealed.handle().clone()],
            )],
        );
        tx.retire.push(StageRetirement {
            upload: sealed.upload().clone(),
            checkpoint,
        });
        let receipt = store.commit(tx).await.unwrap();
        assert_eq!(receipt.references, vec![sealed.handle().clone()]);
        assert_eq!(
            receipt.durability,
            if volume {
                Durability::Durable
            } else {
                Durability::Ephemeral
            }
        );
        let reader = store.open(sealed.handle(), Some(1..5)).await.unwrap();
        let current = store.record(&key("object")).await.unwrap().unwrap();
        store
            .commit(transaction(
                &store,
                vec![RecordChange {
                    key: key("object"),
                    condition: RecordCondition::Exact(current.revision),
                    value: None,
                }],
            ))
            .await
            .unwrap();
        assert_eq!(collected(reader).await, b"bcde");
        assert!(
            store
                .record(&key("object"))
                .await
                .unwrap()
                .unwrap()
                .value
                .is_none()
        );
        assert!(
            store
                .lookup_upload(&owner(), sealed.upload().as_str())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            collected(store.open(sealed.handle(), None).await.unwrap()).await,
            b"abcdef"
        );
    }
}

#[tokio::test]
async fn failed_consuming_seal_preserves_last_acknowledged_prefix_for_retry() {
    let root = tempfile::tempdir().unwrap();
    let store = PayloadStore::open_store(volume_config(&root))
        .await
        .unwrap();
    let mut stage = store.begin(spec()).await.unwrap();
    let id = stage.id().clone();
    stage.append(0, Bytes::from_static(b"abc")).await.unwrap();
    let checkpoint = stage.checkpoint(metadata()).await.unwrap();
    stage.append(3, Bytes::from_static(b"bad")).await.unwrap();
    assert!(matches!(
        stage
            .seal(IntegrityExpectation {
                length: Some(6),
                sha256: Some(sha(b"abcdef"))
            })
            .await,
        Err(StoreError::Integrity)
    ));
    let retained = store
        .lookup_upload(&owner(), id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.checkpoint, checkpoint);
    assert!(retained.sealed.is_none());
    let mut resumed = store.resume(&id, checkpoint).await.unwrap();
    resumed.append(3, Bytes::from_static(b"def")).await.unwrap();
    let payload = resumed
        .seal(IntegrityExpectation {
            length: Some(6),
            sha256: Some(sha(b"abcdef")),
        })
        .await
        .unwrap();
    assert_eq!(
        collected(store.open(payload.handle(), None).await.unwrap()).await,
        b"abcdef"
    );
}

#[tokio::test]
async fn restart_keeps_ack_and_unacknowledged_tail_until_explicit_resume() {
    let root = tempfile::tempdir().unwrap();
    let config = volume_config(&root);
    let (opaque, checkpoint_bytes) = {
        let store = PayloadStore::open_store(config.clone()).await.unwrap();
        let mut stage = store.begin(spec()).await.unwrap();
        stage
            .append(0, Bytes::from_static(b"prefix"))
            .await
            .unwrap();
        let checkpoint = stage.checkpoint(metadata()).await.unwrap();
        stage
            .append(6, Bytes::from_static(b"unacknowledged"))
            .await
            .unwrap();
        (stage.id().as_str().to_string(), checkpoint.offset())
    };
    let store = PayloadStore::open_store(config).await.unwrap();
    assert_eq!(store.inventory().await.unwrap().retained_bytes, 20);
    let snapshot = store
        .lookup_upload(&owner(), &opaque)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.checkpoint.offset(), checkpoint_bytes);
    assert_eq!(snapshot.protocol.as_ref().unwrap().bytes, metadata().bytes);
    let stage = store
        .resume(&snapshot.id, snapshot.checkpoint)
        .await
        .unwrap();
    assert_eq!(stage.progress().await.unwrap().sha256, sha(b"prefix"));
    assert_eq!(store.inventory().await.unwrap().retained_bytes, 6);
}

#[tokio::test]
async fn partial_write_and_checkpoint_response_loss_do_not_destroy_previous_ack() {
    let store = memory().await;
    let mut stage = store.begin(spec()).await.unwrap();
    stage
        .append(0, Bytes::from_static(b"prefix"))
        .await
        .unwrap();
    let checkpoint = stage.checkpoint(metadata()).await.unwrap();
    store.inner.lock().unwrap().fault = Some(Fault::PartialAppend);
    assert_eq!(
        stage
            .append(6, Bytes::from_static(b"suffix"))
            .await
            .unwrap_err(),
        StoreError::Io
    );
    assert!(matches!(
        stage.progress().await,
        Err(StoreError::Interrupted)
    ));
    stage.rollback_to(checkpoint).await.unwrap();
    stage.append(6, Bytes::from_static(b"ok")).await.unwrap();
    store.inner.lock().unwrap().fault = Some(Fault::AfterCheckpoint);
    assert_eq!(
        stage.checkpoint(metadata()).await.unwrap_err(),
        StoreError::Interrupted
    );
    let snapshot = store
        .lookup_upload(&owner(), stage.id().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.checkpoint.offset(), 8);
    assert_eq!(snapshot.checkpoint.sha256(), sha(b"prefixok"));
}

#[tokio::test]
async fn installed_orphan_is_kept_but_does_not_destroy_restart_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let config = volume_config(&root);
    let opaque = {
        let store = PayloadStore::open_store(config.clone()).await.unwrap();
        let mut stage = store.begin(spec()).await.unwrap();
        stage
            .append(0, Bytes::from_static(b"durable"))
            .await
            .unwrap();
        stage.checkpoint(metadata()).await.unwrap();
        let id = stage.id().as_str().to_string();
        store.inner.lock().unwrap().fault = Some(Fault::AfterInstall);
        assert!(matches!(
            stage.seal(IntegrityExpectation::none()).await,
            Err(StoreError::Io)
        ));
        id
    };
    let store = PayloadStore::open_store(config).await.unwrap();
    assert_eq!(store.inventory().await.unwrap().retained_files, 2);
    let snapshot = store
        .lookup_upload(&owner(), &opaque)
        .await
        .unwrap()
        .unwrap();
    assert!(snapshot.sealed.is_none());
    let stage = store
        .resume(&snapshot.id, snapshot.checkpoint)
        .await
        .unwrap();
    let sealed = stage.seal(IntegrityExpectation::none()).await.unwrap();
    assert_eq!(
        collected(store.open(sealed.handle(), None).await.unwrap()).await,
        b"durable"
    );
}

#[tokio::test]
async fn grouped_conflict_never_publishes_partial_record_set_and_any_is_explicit() {
    let store = memory().await;
    let first = store
        .commit(transaction(
            &store,
            vec![
                put("a", RecordCondition::Absent, vec![]),
                put("b", RecordCondition::Absent, vec![]),
            ],
        ))
        .await
        .unwrap();
    let changed = store
        .commit(transaction(
            &store,
            vec![put("b", RecordCondition::Any, vec![])],
        ))
        .await
        .unwrap();
    assert_ne!(first.revision, changed.revision);
    let tx = transaction(
        &store,
        vec![
            put("a", RecordCondition::Exact(first.revision), vec![]),
            put("b", RecordCondition::Exact(first.revision), vec![]),
        ],
    );
    let id = tx.id.clone();
    assert_eq!(store.commit(tx).await.unwrap_err(), CommitError::Conflict);
    assert_eq!(
        store.record(&key("a")).await.unwrap().unwrap().revision,
        first.revision
    );
    assert_eq!(
        store.record(&key("b")).await.unwrap().unwrap().revision,
        changed.revision
    );
    assert_eq!(
        store.recover(&id).await.unwrap(),
        CommitStatus::NotCommitted
    );
}

#[tokio::test]
async fn duplicate_and_lost_commit_response_recover_original_refs_after_later_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let config = volume_config(&root);
    let (opaque, digest) = {
        let store = PayloadStore::open_store(config.clone()).await.unwrap();
        let sealed = seal_bytes(&store, b"first").await;
        let tx = transaction(
            &store,
            vec![put(
                "a",
                RecordCondition::Any,
                vec![sealed.handle().clone()],
            )],
        );
        let id = tx.id.clone();
        store.inner.lock().unwrap().fault = Some(Fault::AfterCommit);
        assert_eq!(
            store.commit(tx.clone()).await.unwrap_err(),
            CommitError::OutcomeUnknown(id.clone())
        );
        let receipt = store.commit(tx).await.unwrap();
        assert_eq!(receipt.references, vec![sealed.handle().clone()]);
        store
            .commit(transaction(
                &store,
                vec![put("a", RecordCondition::Any, vec![])],
            ))
            .await
            .unwrap();
        (id.as_str().to_string(), sealed.handle().sha256())
    };
    let store = PayloadStore::open_store(config).await.unwrap();
    let id = store.commit_id(&opaque).unwrap();
    let CommitStatus::Committed(receipt) = store.recover(&id).await.unwrap() else {
        panic!("durable receipt missing")
    };
    assert_eq!(receipt.references.len(), 1);
    assert_eq!(receipt.references[0].sha256(), digest);
    assert_eq!(
        collected(store.open(&receipt.references[0], None).await.unwrap()).await,
        b"first"
    );
    assert!(
        store
            .record(&key("a"))
            .await
            .unwrap()
            .unwrap()
            .value
            .unwrap()
            .references
            .is_empty()
    );
    let wrong = ReferenceTransaction {
        id,
        owner: owner(),
        changes: vec![put("other", RecordCondition::Any, vec![])],
        retire: vec![],
    };
    assert!(matches!(
        store.commit(wrong).await,
        Err(CommitError::Store(StoreError::Invalid(_)))
    ));
}

#[tokio::test]
async fn before_commit_fault_rolls_back_all_metadata_and_preserves_sealed_bytes() {
    let root = tempfile::tempdir().unwrap();
    let config = volume_config(&root);
    let opaque = {
        let store = PayloadStore::open_store(config.clone()).await.unwrap();
        let payload = seal_bytes(&store, b"retained").await;
        store.inner.lock().unwrap().fault = Some(Fault::BeforeCommit);
        let tx = transaction(
            &store,
            vec![
                put("a", RecordCondition::Any, vec![payload.handle().clone()]),
                put("b", RecordCondition::Any, vec![]),
            ],
        );
        let id = tx.id.as_str().to_string();
        assert!(matches!(
            store.commit(tx).await,
            Err(CommitError::Store(StoreError::Io))
        ));
        id
    };
    let store = PayloadStore::open_store(config).await.unwrap();
    assert!(store.record(&key("a")).await.unwrap().is_none());
    assert!(store.record(&key("b")).await.unwrap().is_none());
    assert_eq!(
        store
            .recover(&store.commit_id(&opaque).unwrap())
            .await
            .unwrap(),
        CommitStatus::NotCommitted
    );
    assert!(store.resolve_payload(sha(b"retained"), 8).await.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_commit_keeps_queued_job_admission_until_actual_completion() {
    let store = memory().await;
    let gate = Arc::new(CommitGate::default());
    let _release_on_failure = ReleaseGate(gate.clone());
    *store.commit_gate.lock().unwrap() = Some(gate.clone());
    let tx = transaction(&store, vec![put("a", RecordCondition::Any, vec![])]);
    let id = tx.id.clone();
    let mut future = Box::pin(store.commit(tx));
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    let entered = gate.clone();
    tokio::task::spawn_blocking(move || entered.entered())
        .await
        .unwrap();
    drop(future);
    let status = store.recover(&id).await;
    // Release before assertions, so a failed assertion cannot strand an owned blocking worker.
    gate.release();
    let finished = gate.clone();
    tokio::task::spawn_blocking(move || finished.wait_finished())
        .await
        .unwrap();
    assert_eq!(status.unwrap(), CommitStatus::InProgress);
    assert!(matches!(
        store.recover(&id).await.unwrap(),
        CommitStatus::Committed(_)
    ));
    assert!(store.record(&key("a")).await.unwrap().is_some());
}

#[tokio::test]
async fn namespace_generation_and_record_owner_are_not_interchangeable() {
    let a = memory().await;
    let b = PayloadStore::open_store(StoreConfig::memory("other", "generation", limits()).unwrap())
        .await
        .unwrap();
    let payload = seal_bytes(&a, b"private to namespace").await;
    assert!(matches!(
        b.open(payload.handle(), None).await,
        Err(StoreError::NamespaceMismatch)
    ));
    assert_eq!(
        b.recover(&a.new_commit_id()).await.unwrap_err(),
        StoreError::NamespaceMismatch
    );
    assert!(matches!(
        b.commit(transaction(
            &b,
            vec![put(
                "a",
                RecordCondition::Any,
                vec![payload.handle().clone()]
            )]
        ))
        .await,
        Err(CommitError::Store(StoreError::NamespaceMismatch))
    ));
    let other = OwnerKey::new("fixture", "resource", "generation-2").unwrap();
    a.commit(transaction(
        &a,
        vec![put("a", RecordCondition::Any, vec![])],
    ))
    .await
    .unwrap();
    assert!(
        a.list_records(&other, None, 4)
            .await
            .unwrap()
            .records
            .is_empty()
    );
}

#[tokio::test]
async fn owner_enumeration_is_bounded_and_refuses_a_changed_page_snapshot() {
    let store = memory().await;
    store
        .commit(transaction(
            &store,
            vec![
                put("a", RecordCondition::Any, vec![]),
                put("b", RecordCondition::Any, vec![]),
                put("c", RecordCondition::Any, vec![]),
            ],
        ))
        .await
        .unwrap();
    let page = store.list_records(&owner(), None, 1).await.unwrap();
    assert_eq!(page.records.len(), 1);
    let cursor = page.next.unwrap();
    let next = store
        .list_records(&owner(), Some(cursor.clone()), 1)
        .await
        .unwrap();
    assert_eq!(next.records[0].key.key(), &Bytes::from_static(b"b"));
    store
        .commit(transaction(
            &store,
            vec![put("d", RecordCondition::Any, vec![])],
        ))
        .await
        .unwrap();
    assert!(matches!(
        store.list_records(&owner(), Some(cursor), 1).await,
        Err(StoreError::EnumerationChanged)
    ));
    assert!(store.list_records(&owner(), None, 0).await.is_err());
}

#[tokio::test]
async fn quotas_and_unsupported_integrity_refuse_without_touching_prior_ack() {
    assert_eq!(
        IntegrityExpectation::checksum("md5", "ignored").unwrap_err(),
        StoreError::UnsupportedChecksum
    );
    let mut small = limits();
    small.max_chunk_bytes = 3;
    small.max_stage_bytes = 8;
    small.max_storage_bytes = 10;
    let store =
        PayloadStore::open_store(StoreConfig::memory("small", "generation", small).unwrap())
            .await
            .unwrap();
    let mut stage = store
        .begin(StageSpec {
            max_bytes: 8,
            ..spec()
        })
        .await
        .unwrap();
    assert_eq!(stage.max_append_bytes(), 3);
    assert_eq!(
        stage
            .append(0, Bytes::from_static(b"four"))
            .await
            .unwrap_err(),
        StoreError::QuotaExceeded
    );
    stage.append(0, Bytes::from_static(b"abc")).await.unwrap();
    let cp = stage.checkpoint(metadata()).await.unwrap();
    stage.append(3, Bytes::from_static(b"def")).await.unwrap();
    assert!(matches!(
        stage.seal(IntegrityExpectation::none()).await,
        Err(StoreError::QuotaExceeded)
    ));
    let snapshot = store
        .lookup_upload(&owner(), cp.upload().as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.checkpoint, cp);
    assert!(snapshot.sealed.is_none());
}

#[tokio::test]
async fn empty_range_composition_and_reader_limits_remain_streamed() {
    let mut config = limits();
    config.max_read_leases = 1;
    let store =
        PayloadStore::open_store(StoreConfig::memory("ranges", "generation", config).unwrap())
            .await
            .unwrap();
    let empty = seal_bytes(&store, b"").await;
    assert!(
        store
            .open(empty.handle(), None)
            .await
            .unwrap()
            .next_chunk()
            .await
            .unwrap()
            .is_none()
    );
    let a = seal_bytes(&store, b"abc").await;
    let b = seal_bytes(&store, b"def").await;
    let composed = store
        .compose(
            spec(),
            &[a.handle().clone(), b.handle().clone()],
            IntegrityExpectation {
                length: Some(6),
                sha256: Some(sha(b"abcdef")),
            },
        )
        .await
        .unwrap();
    let reader = store.open(composed.handle(), Some(3..3)).await.unwrap();
    assert!(matches!(
        store.open(a.handle(), None).await,
        Err(StoreError::Busy)
    ));
    drop(reader);
    assert_eq!(
        collected(store.open(composed.handle(), None).await.unwrap()).await,
        b"abcdef"
    );
    assert!(matches!(
        store.open(composed.handle(), Some(5..3)).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        store.open(composed.handle(), Some(0..7)).await,
        Err(StoreError::Invalid(_))
    ));
}

#[tokio::test]
async fn abort_never_erases_ack_and_sealed_conflict_can_be_recommitted_without_resume() {
    let store = memory().await;
    let mut stage = store.begin(spec()).await.unwrap();
    stage.append(0, Bytes::from_static(b"ack")).await.unwrap();
    let cp = stage.checkpoint(metadata()).await.unwrap();
    let id = stage.id().clone();
    assert!(matches!(stage.abort().await, Err(StoreError::Invalid(_))));
    let stage = store.resume(&id, cp.clone()).await.unwrap();
    let sealed = stage.seal(IntegrityExpectation::none()).await.unwrap();
    let snapshot = store
        .lookup_upload(&owner(), id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.sealed.as_ref(), Some(sealed.handle()));
    assert!(matches!(
        store.resume(&id, cp.clone()).await,
        Err(StoreError::Invalid(_))
    ));
    store
        .commit(transaction(
            &store,
            vec![put("a", RecordCondition::Any, vec![])],
        ))
        .await
        .unwrap();
    assert_eq!(
        store
            .commit(transaction(
                &store,
                vec![put(
                    "a",
                    RecordCondition::Absent,
                    vec![sealed.handle().clone()]
                )]
            ))
            .await
            .unwrap_err(),
        CommitError::Conflict
    );
    let mut tx = transaction(
        &store,
        vec![put(
            "a",
            RecordCondition::Any,
            vec![snapshot.sealed.unwrap()],
        )],
    );
    tx.retire.push(StageRetirement {
        upload: id,
        checkpoint: cp,
    });
    store.commit(tx).await.unwrap();
}

#[tokio::test]
async fn corrupt_committed_bytes_are_integrity_not_missing_or_empty_and_other_records_work() {
    let root = tempfile::tempdir().unwrap();
    let config = volume_config(&root);
    let (directory, digest, opaque) = {
        let store = PayloadStore::open_store(config.clone()).await.unwrap();
        let bad = seal_bytes(&store, b"bad target").await;
        let good = seal_bytes(&store, b"good").await;
        let tx = transaction(
            &store,
            vec![
                put("bad", RecordCondition::Any, vec![bad.handle().clone()]),
                put("good", RecordCondition::Any, vec![good.handle().clone()]),
            ],
        );
        let opaque = tx.id.as_str().to_string();
        store.commit(tx).await.unwrap();
        let directory = store.inner.lock().unwrap().volume_directory().unwrap();
        (directory, bad.handle().sha256(), opaque)
    };
    std::fs::write(directory.join("blobs").join(digest.as_hex()), b"corruption").unwrap();
    let store = PayloadStore::open_store(config).await.unwrap();
    assert_eq!(store.inventory().await.unwrap().degraded_payloads, 1);
    assert!(matches!(
        store.record(&key("bad")).await,
        Err(StoreError::Integrity)
    ));
    let good = store
        .record(&key("good"))
        .await
        .unwrap()
        .unwrap()
        .value
        .unwrap();
    assert_eq!(
        collected(store.open(&good.references[0], None).await.unwrap()).await,
        b"good"
    );
    assert!(matches!(
        store
            .recover(&store.commit_id(&opaque).unwrap())
            .await
            .unwrap(),
        CommitStatus::Committed(_)
    ));
}

#[tokio::test]
async fn unknown_files_are_retained_and_foreign_identity_is_never_reset() {
    let root = tempfile::tempdir().unwrap();
    let config = volume_config(&root);
    let directory = {
        let store = PayloadStore::open_store(config.clone()).await.unwrap();
        let path = store.inner.lock().unwrap().volume_directory().unwrap();
        path
    };
    let unknown = directory.join("temporary").join("unknown-kept");
    std::fs::write(&unknown, b"retained").unwrap();
    {
        let store = PayloadStore::open_store(config.clone()).await.unwrap();
        assert_eq!(store.inventory().await.unwrap().retained_bytes, 8);
        assert!(unknown.exists());
    }
    std::fs::write(directory.join("identity.json"), b"{}").unwrap();
    assert!(matches!(
        PayloadStore::open_store(config).await,
        Err(StoreError::NamespaceMismatch)
    ));
    assert_eq!(std::fs::read(&unknown).unwrap(), b"retained");
}

#[tokio::test]
async fn upload_aliases_cannot_shadow_an_opaque_upload_id() {
    let store = memory().await;
    let first = store.begin(spec()).await.unwrap();
    let mut second_spec = spec();
    second_spec.upload_key = Some(first.id().as_str().to_string());
    let second = store.begin(second_spec).await.unwrap();
    assert_eq!(
        store
            .lookup_upload(&owner(), first.id().as_str())
            .await
            .unwrap()
            .unwrap()
            .id,
        *first.id()
    );
    assert_eq!(
        store
            .lookup_upload_key(&owner(), first.id().as_str())
            .await
            .unwrap()
            .unwrap()
            .id,
        *second.id()
    );
}

#[tokio::test]
async fn catalog_byte_ceiling_preserves_previously_committed_record() {
    let mut config = limits();
    config.max_catalog_bytes = 256 * 1024;
    let store = PayloadStore::open_store(
        StoreConfig::memory("catalog-quota", "generation", config).unwrap(),
    )
    .await
    .unwrap();
    store
        .commit(transaction(
            &store,
            vec![put("ack", RecordCondition::Any, vec![])],
        ))
        .await
        .unwrap();
    let mut reached = false;
    for index in 0..16 {
        let mut change = put(&format!("large-{index}"), RecordCondition::Any, vec![]);
        change.value.as_mut().unwrap().metadata = json!({"bounded":"x".repeat(40 * 1024)});
        match store.commit(transaction(&store, vec![change])).await {
            Ok(_) => {}
            Err(CommitError::Store(StoreError::QuotaExceeded)) => {
                reached = true;
                break;
            }
            Err(other) => panic!("unexpected bounded catalog outcome: {other:?}"),
        }
    }
    assert!(
        reached,
        "fixed finite fill must encounter the admitted SQLite page ceiling"
    );
    assert!(
        store
            .record(&key("ack"))
            .await
            .unwrap()
            .unwrap()
            .value
            .is_some()
    );
}

#[tokio::test]
async fn one_volume_catalog_rejects_a_second_independent_writer() {
    let root = tempfile::tempdir().unwrap();
    let config = volume_config(&root);
    let _owner = PayloadStore::open_store(config.clone()).await.unwrap();
    assert!(matches!(
        PayloadStore::open_store(config).await,
        Err(StoreError::Busy)
    ));
}

#[tokio::test]
async fn enumeration_enforces_byte_budget_before_loading_a_requested_full_page() {
    let mut config = limits();
    config.max_page_bytes = config.max_metadata_bytes;
    let store =
        PayloadStore::open_store(StoreConfig::memory("page-bytes", "generation", config).unwrap())
            .await
            .unwrap();
    let changes = ["a", "b", "c"]
        .into_iter()
        .map(|name| {
            let mut change = put(name, RecordCondition::Any, vec![]);
            change.value.as_mut().unwrap().metadata = json!({"bounded":"x".repeat(40 * 1024)});
            change
        })
        .collect();
    store.commit(transaction(&store, changes)).await.unwrap();
    let first = store.list_records(&owner(), None, 32).await.unwrap();
    assert_eq!(first.records.len(), 1);
    let second = store.list_records(&owner(), first.next, 32).await.unwrap();
    assert_eq!(second.records.len(), 1);
    assert_eq!(second.records[0].key.key(), &Bytes::from_static(b"b"));
    let third = store.list_records(&owner(), second.next, 32).await.unwrap();
    assert_eq!(third.records.len(), 1);
    assert!(third.next.is_none());
}

#[tokio::test]
async fn file_admission_matches_boot_and_reserves_temporary_publication_names() {
    let root = tempfile::tempdir().unwrap();
    let mut config = limits();
    config.max_data_files = 3;
    let config = StoreConfig::volume(
        root.path().to_path_buf(),
        "file-quota",
        "generation",
        config,
    )
    .unwrap();
    {
        let store = PayloadStore::open_store(config.clone()).await.unwrap();
        let first = seal_bytes(&store, b"ack").await;
        store
            .commit(transaction(
                &store,
                vec![put(
                    "ack",
                    RecordCondition::Any,
                    vec![first.handle().clone()],
                )],
            ))
            .await
            .unwrap();
        let mut second = store.begin(spec()).await.unwrap();
        second.append(0, Bytes::from_static(b"new")).await.unwrap();
        assert!(matches!(
            second.seal(IntegrityExpectation::none()).await,
            Err(StoreError::QuotaExceeded)
        ));
        assert_eq!(store.inventory().await.unwrap().retained_files, 3);
        assert!(matches!(
            store.begin(spec()).await,
            Err(StoreError::QuotaExceeded)
        ));
    }
    let store = PayloadStore::open_store(config).await.unwrap();
    assert_eq!(store.inventory().await.unwrap().retained_files, 3);
    let ack = store
        .record(&key("ack"))
        .await
        .unwrap()
        .unwrap()
        .value
        .unwrap();
    assert_eq!(
        collected(store.open(&ack.references[0], None).await.unwrap()).await,
        b"ack"
    );
}
