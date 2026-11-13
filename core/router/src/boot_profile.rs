// SPDX-License-Identifier: CC0-1.0

//! Resolve one payload/identity/persistence scope before serving any requests.
//! No filesystem is touched by parsing. Durable namespace admission is owned by B5
//! and occurs before opening the store. Snapshots receive this same frozen selection.

use engine::InMemoryEngine;
use service_common::host_runtime::{DEFAULT_NAMESPACE, NAMESPACE_VARIABLE};
use service_common::payload_store::{
    PayloadStore, PayloadStoreBinding, StoreConfig, StoreError, StoreLimits,
};
use service_common::persist::{PersistError, STATE_DIR_VARIABLE, StateDir};
use std::ffi::OsString;
use std::path::{Component, PathBuf};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
// Store format generation, deliberately independent of boot time and TLS/key rotation.
const STORE_GENERATION: &str = "payload-v1";

#[derive(Debug)]
pub(crate) enum BootError {
    InvalidInput(&'static str),
    AlreadyInstalled,
    Store(StoreError),
    State(PersistError),
    Identity(emulator_server::boot_identity::IdentityError),
    Network(emulator_server::ServeError),
}

impl std::fmt::Display for BootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(name) => write!(f, "invalid boot setting {name}"),
            Self::AlreadyInstalled => f.write_str("payload store binding already installed"),
            Self::Store(error) => write!(f, "{error}"),
            Self::State(error) => write!(f, "state directory: {error}"),
            Self::Identity(error) => write!(f, "{error}"),
            Self::Network(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BootError {}

impl From<StoreError> for BootError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<PersistError> for BootError {
    fn from(error: PersistError) -> Self {
        Self::State(error)
    }
}

pub(crate) struct BootProfile {
    // These exact resolved values will also feed B5 and boot_identity::prepare;
    // neither consumer may independently reinterpret the environment.
    pub(crate) state_dir: Option<StateDir>,
    pub(crate) namespace: String,
    store_config: StoreConfig,
}

impl BootProfile {
    pub(crate) fn from_environment() -> Result<Self, BootError> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    pub(crate) fn prepare_identity(
        &self,
    ) -> Result<emulator_server::boot_identity::IdentityInfo, BootError> {
        use emulator_server::boot_identity::{IdentityConfig, RetainedIdentityAction};
        let network = *emulator_server::runtime_profile::network().map_err(BootError::Network)?;
        let config = match &self.state_dir {
            Some(selected) => {
                selected.admit()?;
                IdentityConfig::Retained {
                    state_dir: selected.root().to_path_buf(),
                    namespace: selected.namespace().to_string(),
                    network,
                    action: RetainedIdentityAction::KeepOrCreate,
                }
            }
            None => IdentityConfig::Ephemeral,
        };
        emulator_server::boot_identity::prepare(config).map_err(BootError::Identity)
    }

    fn from_lookup(
        mut lookup: impl FnMut(&'static str) -> Option<OsString>,
    ) -> Result<Self, BootError> {
        let state_dir = match lookup(STATE_DIR_VARIABLE) {
            None => None,
            Some(raw) => {
                if raw.to_str().is_none() {
                    return Err(BootError::InvalidInput(STATE_DIR_VARIABLE));
                }
                let path = PathBuf::from(raw);
                if !path.is_absolute()
                    || path
                        .components()
                        .any(|part| matches!(part, Component::ParentDir))
                {
                    return Err(BootError::InvalidInput(STATE_DIR_VARIABLE));
                }
                Some(path)
            }
        };
        let namespace = match lookup(NAMESPACE_VARIABLE) {
            None => DEFAULT_NAMESPACE.to_string(),
            Some(value) => value
                .into_string()
                .map_err(|_| BootError::InvalidInput(NAMESPACE_VARIABLE))?,
        };
        if namespace.is_empty()
            || namespace.len() > 64
            || matches!(namespace.as_str(), "." | "..")
            || !namespace
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(BootError::InvalidInput(NAMESPACE_VARIABLE));
        }
        let durable = state_dir.is_some();
        let limits = StoreLimits {
            max_storage_bytes: number(
                &mut lookup,
                "EMULATOR_STORE_MAX_STORAGE_BYTES",
                if durable { 64 * GIB } else { 256 * MIB },
            )?,
            max_catalog_bytes: number(
                &mut lookup,
                "EMULATOR_STORE_MAX_CATALOG_BYTES",
                if durable { 256 * MIB } else { 32 * MIB },
            )?,
            max_stage_bytes: number(
                &mut lookup,
                "EMULATOR_STORE_MAX_STAGE_BYTES",
                if durable { 8 * GIB } else { 256 * MIB },
            )?,
            max_chunk_bytes: MIB as usize,
            max_metadata_bytes: MIB as usize,
            max_records_per_transaction: 256,
            max_references_per_record: 16384,
            max_page_records: 256,
            max_page_bytes: 4 * MIB as usize,
            max_stages: 10000,
            max_records: 100000,
            max_commits: 100000,
            max_transaction_bytes: 16 * MIB as usize,
            max_inflight_operations: 16,
            max_read_leases: 64,
            max_data_files: 100000,
        };
        let state_dir = state_dir
            .map(|root| StateDir::new(root, namespace.clone()))
            .transpose()?;
        // Both constructors validate the complete quota relation without opening any files.
        let store_config = match &state_dir {
            Some(selected) => StoreConfig::volume(
                selected.dir(),
                selected.namespace(),
                STORE_GENERATION,
                limits,
            )?,
            None => StoreConfig::memory(&namespace, STORE_GENERATION, limits)?,
        };
        Ok(Self {
            state_dir,
            namespace,
            store_config,
        })
    }

    pub(crate) async fn install_payload_store(
        &self,
        engine: &InMemoryEngine,
    ) -> Result<(), BootError> {
        let binding = engine.state::<PayloadStoreBinding>();
        if binding.lock().store.is_some() {
            return Err(BootError::AlreadyInstalled);
        }
        if let Some(selected) = &self.state_dir {
            selected.admit()?;
        }
        let store = PayloadStore::open_store(self.store_config.clone()).await?;
        let mut binding = binding.lock();
        if binding.store.is_some() {
            return Err(BootError::AlreadyInstalled);
        }
        binding.store = Some(store);
        Ok(())
    }
}

fn number(
    lookup: &mut impl FnMut(&'static str) -> Option<OsString>,
    name: &'static str,
    default: u64,
) -> Result<u64, BootError> {
    let Some(raw) = lookup(name) else {
        return Ok(default);
    };
    let raw = raw
        .into_string()
        .map_err(|_| BootError::InvalidInput(name))?;
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(BootError::InvalidInput(name));
    }
    raw.parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or(BootError::InvalidInput(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(values: &[(&str, &str)]) -> Result<BootProfile, BootError> {
        BootProfile::from_lookup(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(*value))
        })
    }

    #[test]
    fn unset_is_explicitly_ephemeral_and_namespace_matches_host_runtime() {
        let profile = profile(&[]).unwrap();
        assert!(profile.state_dir.is_none());
        assert_eq!(profile.namespace, DEFAULT_NAMESPACE);
    }

    #[test]
    fn state_and_namespace_selection_is_strict() {
        for root in ["", "relative", "/tmp/../other"] {
            assert!(profile(&[(STATE_DIR_VARIABLE, root)]).is_err());
        }
        for namespace in ["", ".", "..", "../x", "/x", "has space", "é"] {
            assert!(profile(&[(NAMESPACE_VARIABLE, namespace)]).is_err());
        }
        assert!(profile(&[(NAMESPACE_VARIABLE, &"x".repeat(65))]).is_err());
        let selected = profile(&[
            (STATE_DIR_VARIABLE, "/selected-state"),
            (NAMESPACE_VARIABLE, "test.v1_a-2"),
        ])
        .unwrap();
        assert_eq!(
            selected.state_dir.as_ref().map(StateDir::root),
            Some(std::path::Path::new("/selected-state"))
        );
        assert_eq!(selected.namespace, "test.v1_a-2");
    }

    #[test]
    fn invalid_or_incoherent_quotas_refuse_without_environment_changes() {
        for bad in ["", "0", "-1", "+1", " 123", "18446744073709551616"] {
            assert!(profile(&[("EMULATOR_STORE_MAX_STORAGE_BYTES", bad)]).is_err());
        }
        assert!(profile(&[("EMULATOR_STORE_MAX_CATALOG_BYTES", "1")]).is_err());
        assert!(profile(&[("EMULATOR_STORE_MAX_STORAGE_BYTES", "1048576")]).is_err());
    }

    #[tokio::test]
    async fn memory_binding_is_installed_once_before_consumers() {
        let engine = InMemoryEngine::new();
        let profile = profile(&[]).unwrap();
        profile.install_payload_store(&engine).await.unwrap();
        assert!(engine.state::<PayloadStoreBinding>().lock().store.is_some());
        assert!(matches!(
            profile.install_payload_store(&engine).await,
            Err(BootError::AlreadyInstalled)
        ));
    }

    #[tokio::test]
    async fn durable_selection_admits_one_scope_and_reopens_checkpoint() {
        use service_common::payload_store::{Durability, OwnerKey, ProtocolMetadata, StageSpec};
        let fixture = Fixture::new();
        let profile = profile(&[
            (STATE_DIR_VARIABLE, fixture.0.to_str().unwrap()),
            (NAMESPACE_VARIABLE, "test"),
        ])
        .unwrap();
        let selected = profile.state_dir.as_ref().unwrap();
        assert_eq!(selected.namespace(), profile.namespace);
        let owner = OwnerKey::new("test", "upload", "v1").unwrap();
        let upload;
        {
            let engine = InMemoryEngine::new();
            profile.install_payload_store(&engine).await.unwrap();
            let store = engine
                .state::<PayloadStoreBinding>()
                .lock()
                .store
                .clone()
                .unwrap();
            assert_eq!(store.durability(), Durability::Durable);
            assert!(selected.dir().is_dir());
            let mut stage = store
                .begin(StageSpec {
                    owner: owner.clone(),
                    upload_key: None,
                    expected_length: Some(4),
                    max_bytes: 4,
                })
                .await
                .unwrap();
            stage
                .append(0, bytes::Bytes::from_static(b"data"))
                .await
                .unwrap();
            stage
                .checkpoint(
                    ProtocolMetadata::new("fixture", 1, bytes::Bytes::from_static(b"{}")).unwrap(),
                )
                .await
                .unwrap();
            upload = stage.id().as_str().to_string();
        }
        let engine = InMemoryEngine::new();
        profile.install_payload_store(&engine).await.unwrap();
        let store = engine
            .state::<PayloadStoreBinding>()
            .lock()
            .store
            .clone()
            .unwrap();
        assert!(
            store
                .lookup_upload(&owner, &upload)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(store.inventory().await.unwrap().stages, 1);
    }

    #[tokio::test]
    async fn refused_namespace_does_not_install_a_memory_fallback() {
        let fixture = Fixture::new();
        std::fs::write(fixture.0.join("test"), b"not a directory").unwrap();
        let profile = profile(&[
            (STATE_DIR_VARIABLE, fixture.0.to_str().unwrap()),
            (NAMESPACE_VARIABLE, "test"),
        ])
        .unwrap();
        let engine = InMemoryEngine::new();
        assert!(profile.install_payload_store(&engine).await.is_err());
        assert!(engine.state::<PayloadStoreBinding>().lock().store.is_none());
        assert_eq!(
            std::fs::read(fixture.0.join("test")).unwrap(),
            b"not a directory"
        );
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "emulator-boot-test-{}-{stamp}-{id}",
                std::process::id()
            ));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
