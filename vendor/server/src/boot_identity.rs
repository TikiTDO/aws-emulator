// SPDX-License-Identifier: CC0-1.0

//! One private, atomic TLS/SigV4 identity generation for an explicitly selected boot.
//!
//! No environment discovery, AWS credential adoption, service snapshot, or live reload.
//! Call before restoring services or opening listeners. Existing embeddings remain lazy
//! and ephemeral. The state root and namespace must already be admitted/created by the
//! boot profile's directory owner; this module creates only its private `identity` child.
//! Retention is a caller policy choice, not enabled by linking this module.

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

use aws_lc_rs::digest::{SHA256, digest};
use aws_smithy_types::base64;
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use serde::{Deserialize, Serialize};

use crate::request_authentication::SigV4Authenticator;
use crate::runtime_profile::NetworkProfile;
use crate::tls_certificate_authority::TlsCertificateAuthority;

pub enum IdentityConfig {
    Ephemeral,
    Retained {
        state_dir: PathBuf,
        namespace: String,
        network: NetworkProfile,
        action: RetainedIdentityAction,
    },
}

pub enum RetainedIdentityAction {
    KeepOrCreate,
    /// Offline only: a live process holds the namespace lease. No CLI/API wires this action.
    Rotate {
        expected_generation: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityInfo {
    Ephemeral,
    Retained {
        generation: String,
        certificate_sha256: [u8; 32],
        disposition: IdentityDisposition,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityDisposition {
    Created,
    Restored,
    Rotated,
}

/// Deliberately contains no paths, parser inputs, credentials or private record debug text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    InvalidConfiguration,
    UnsupportedPlatform,
    UnsafePath,
    PermissionMismatch,
    Busy,
    UnknownFormat,
    NamespaceMismatch,
    InvalidIdentity,
    CertificateMismatch,
    AlreadyInitialized,
    GenerationConflict,
    Io {
        operation: &'static str,
        kind: std::io::ErrorKind,
    },
    /// Rename succeeded, but directory durability could not be established. Do not install.
    DurabilityUnknown,
}
impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "boot identity: {self:?}")
    }
}
impl std::error::Error for IdentityError {}

pub(crate) struct RetainedIdentity {
    pub(crate) tls: TlsCertificateAuthority,
    pub(crate) sigv4: SigV4Authenticator,
    // Ownership lasts through all listeners; never unlink the lease leaf on Drop.
    #[cfg(target_os = "linux")]
    _lease: std::os::fd::OwnedFd,
}
enum Mode {
    // An ignored preparation error must never silently choose fresh ephemeral roots.
    Failed,
    Ephemeral,
    Retained(RetainedIdentity),
}
static MODE: OnceLock<Mode> = OnceLock::new();
static INITIALIZING: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

// Also closes a catchable unwind during preparation. A failed boot is not a live retry API.
struct SelectionAttempt;
impl Drop for SelectionAttempt {
    fn drop(&mut self) {
        let _ = MODE.set(Mode::Failed);
    }
}

/// A legacy getter selects ephemeral mode without eagerly generating the other identity.
/// This preserves the embedding's ability to choose TLS names after first requesting SigV4.
pub(crate) fn retained_or_ephemeral() -> Option<&'static RetainedIdentity> {
    let _guard = INITIALIZING.lock();
    match MODE.get_or_init(|| Mode::Ephemeral) {
        Mode::Failed => {
            panic!("retained boot identity preparation failed; refusing ephemeral fallback")
        }
        Mode::Ephemeral => None,
        Mode::Retained(identity) => Some(identity),
    }
}

pub fn prepare(config: IdentityConfig) -> Result<IdentityInfo, IdentityError> {
    let _guard = INITIALIZING.lock();
    if let Some(mode) = MODE.get() {
        return match (mode, &config) {
            (Mode::Ephemeral, IdentityConfig::Ephemeral) => Ok(IdentityInfo::Ephemeral),
            _ => Err(IdentityError::AlreadyInitialized),
        };
    }
    let _attempt = SelectionAttempt;
    let (mode, info) = match config {
        IdentityConfig::Ephemeral => (Mode::Ephemeral, IdentityInfo::Ephemeral),
        IdentityConfig::Retained {
            state_dir,
            namespace,
            network,
            action,
        } => {
            validate_selection(&state_dir, &namespace)?;
            #[cfg(target_os = "linux")]
            {
                let (identity, info) = disk::prepare(&state_dir, &namespace, network, action)?;
                (Mode::Retained(identity), info)
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (network, action);
                return Err(IdentityError::UnsupportedPlatform);
            }
        }
    };
    MODE.set(mode)
        .map_err(|_| IdentityError::AlreadyInitialized)?;
    Ok(info)
}

fn validate_selection(root: &Path, namespace: &str) -> Result<(), IdentityError> {
    if !root.is_absolute()
        || root
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
        || namespace.is_empty()
        || namespace.len() > 64
        || matches!(namespace, "." | "..")
        || !namespace
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(IdentityError::InvalidConfiguration);
    }
    Ok(())
}

const MAX_RECORD: usize = 64 * 1024;
const FORMAT: &str = "emulator-private-boot-identity";

// Struct decoding rejects duplicate fields; deny_unknown_fields refuses schema extension.
// No Debug: these types contain secrets. Serialize is used only by the private file codec.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    format: String,
    version: u32,
    namespace: String,
    generation: String,
    names: Vec<String>,
    certificate_der_base64: String,
    private_pkcs8_der_base64: String,
    root_access_key_id: String,
    root_secret_access_key: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    payload: Payload,
    sha256_base64: String,
}

fn canonical(payload: &Payload) -> Result<Vec<u8>, IdentityError> {
    serde_json::to_vec(payload).map_err(|_| IdentityError::InvalidIdentity)
}
fn sha256(bytes: &[u8]) -> [u8; 32] {
    let value = digest(&SHA256, bytes);
    let mut result = [0; 32];
    result.copy_from_slice(value.as_ref());
    result
}
fn encode(payload: Payload) -> Result<Vec<u8>, IdentityError> {
    let checksum = base64::encode(sha256(&canonical(&payload)?));
    let bytes = serde_json::to_vec(&Record {
        payload,
        sha256_base64: checksum,
    })
    .map_err(|_| IdentityError::InvalidIdentity)?;
    if bytes.len() > MAX_RECORD {
        return Err(IdentityError::InvalidIdentity);
    }
    Ok(bytes)
}
fn decode(bytes: &[u8]) -> Result<Payload, IdentityError> {
    if bytes.len() > MAX_RECORD {
        return Err(IdentityError::InvalidIdentity);
    }
    let record: Record =
        serde_json::from_slice(bytes).map_err(|_| IdentityError::InvalidIdentity)?;
    if record.payload.format != FORMAT || record.payload.version != 1 {
        return Err(IdentityError::UnknownFormat);
    }
    if record.sha256_base64 != base64::encode(sha256(&canonical(&record.payload)?)) {
        return Err(IdentityError::InvalidIdentity);
    }
    Ok(record.payload)
}

fn required_names(network: NetworkProfile) -> Vec<String> {
    let mut names = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    for ip in [network.bind_address(), network.advertised_ip()] {
        if !ip.is_unspecified() {
            names.push(ip.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}
fn fresh(namespace: &str, network: NetworkProfile) -> Result<Payload, IdentityError> {
    let names = required_names(network);
    let ca = TlsCertificateAuthority::generate(names.clone())
        .map_err(|_| IdentityError::InvalidIdentity)?;
    let root = SigV4Authenticator::generate();
    Ok(Payload {
        format: FORMAT.into(),
        version: 1,
        namespace: namespace.into(),
        generation: uuid::Uuid::new_v4().to_string(),
        names,
        certificate_der_base64: base64::encode(ca.der()),
        private_pkcs8_der_base64: base64::encode(ca.private_key_der()),
        root_access_key_id: root.access_key_id,
        root_secret_access_key: root.secret_access_key,
    })
}
fn validate(
    payload: &Payload,
    namespace: &str,
    network: NetworkProfile,
) -> Result<(TlsCertificateAuthority, SigV4Authenticator), IdentityError> {
    if payload.namespace != namespace {
        return Err(IdentityError::NamespaceMismatch);
    }
    if payload.names != required_names(network) {
        return Err(IdentityError::CertificateMismatch);
    }
    if uuid::Uuid::parse_str(&payload.generation)
        .map(|id| id.to_string())
        .ok()
        .as_deref()
        != Some(payload.generation.as_str())
        || payload.root_access_key_id.len() != 20
        || !payload.root_access_key_id.starts_with("EMULATOR")
        || !payload.root_access_key_id[8..].bytes().all(upper_hex)
        || payload.root_secret_access_key.len() != 40
        || !payload.root_secret_access_key.bytes().all(upper_hex)
    {
        return Err(IdentityError::InvalidIdentity);
    }
    let der = base64::decode(&payload.certificate_der_base64)
        .map_err(|_| IdentityError::InvalidIdentity)?;
    let key = base64::decode(&payload.private_pkcs8_der_base64)
        .map_err(|_| IdentityError::InvalidIdentity)?;
    if der.is_empty() || der.len() > 32 * 1024 || key.is_empty() || key.len() > 16 * 1024 {
        return Err(IdentityError::InvalidIdentity);
    }
    let cert = CertificateDer::from(der.clone());
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let certified = rustls::sign::CertifiedKey::from_der(
        vec![cert.clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.clone())),
        &provider,
    )
    .map_err(|_| IdentityError::InvalidIdentity)?;
    // from_der accepts Unknown consistency. Persisted inputs must positively match.
    certified
        .keys_match()
        .map_err(|_| IdentityError::InvalidIdentity)?;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(cert.clone())
        .map_err(|_| IdentityError::InvalidIdentity)?;
    let verifier =
        rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .map_err(|_| IdentityError::InvalidIdentity)?;
    for name in &payload.names {
        let name =
            ServerName::try_from(name.clone()).map_err(|_| IdentityError::CertificateMismatch)?;
        verifier
            .verify_server_cert(&cert, &[], &name, &[], UnixTime::now())
            .map_err(|_| IdentityError::CertificateMismatch)?;
    }
    Ok((
        TlsCertificateAuthority::restored(der, key, payload.names.clone()),
        SigV4Authenticator {
            access_key_id: payload.root_access_key_id.clone(),
            secret_access_key: payload.root_secret_access_key.clone(),
        },
    ))
}
fn upper_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte)
}

#[cfg(target_os = "linux")]
mod disk {
    use super::*;
    use rustix::fs::{
        self, AtFlags, FileType, FlockOperation, Mode as FileMode, OFlags, RenameFlags,
    };
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::OwnedFd;
    const FINAL: &str = "boot-identity.v1";
    const DIRECTORY: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);
    const FILE: OFlags = OFlags::NOFOLLOW
        .union(OFlags::CLOEXEC)
        .union(OFlags::NONBLOCK);
    fn io(operation: &'static str, error: impl Into<std::io::Error>) -> IdentityError {
        IdentityError::Io {
            operation,
            kind: error.into().kind(),
        }
    }
    fn directory(fd: &OwnedFd, private: bool, own: bool) -> Result<(), IdentityError> {
        let stat = fs::fstat(fd).map_err(|e| io("stat directory", e))?;
        let uid = rustix::process::geteuid().as_raw();
        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
            return Err(IdentityError::UnsafePath);
        }
        let sticky_root = !private && !own && stat.st_uid == 0 && stat.st_mode & 0o1000 != 0;
        if (stat.st_uid != uid && (own || stat.st_uid != 0))
            || (private && stat.st_mode & 0o7777 != 0o700)
            || (stat.st_mode & 0o022 != 0 && !sticky_root)
        {
            return Err(IdentityError::PermissionMismatch);
        }
        Ok(())
    }
    fn file(fd: &OwnedFd) -> Result<(), IdentityError> {
        let stat = fs::fstat(fd).map_err(|e| io("stat file", e))?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
            return Err(IdentityError::UnsafePath);
        }
        if stat.st_uid != rustix::process::geteuid().as_raw() || stat.st_mode & 0o7777 != 0o600 {
            return Err(IdentityError::PermissionMismatch);
        }
        Ok(())
    }
    fn private_directory(root: &Path, namespace: &str) -> Result<OwnedFd, IdentityError> {
        let mut fd = fs::open("/", DIRECTORY, FileMode::empty()).map_err(|e| io("open root", e))?;
        directory(&fd, false, false)?;
        for part in root.components() {
            if let Component::Normal(name) = part {
                fd = fs::openat(&fd, name, DIRECTORY, FileMode::empty())
                    .map_err(|e| io("open ancestor", e))?;
                directory(&fd, false, false)?;
            }
        }
        // Root/namespace creation and profile adoption belong to B5, not this module.
        directory(&fd, false, true)?;
        let namespace = fs::openat(&fd, namespace, DIRECTORY, FileMode::empty())
            .map_err(|e| io("open namespace", e))?;
        directory(&namespace, false, true)?;
        match fs::mkdirat(&namespace, "identity", FileMode::from_raw_mode(0o700)) {
            Ok(()) => fs::fsync(&namespace).map_err(|e| io("sync namespace", e))?,
            Err(rustix::io::Errno::EXIST) => {}
            Err(e) => return Err(io("create identity directory", e)),
        }
        let fd = fs::openat(&namespace, "identity", DIRECTORY, FileMode::empty())
            .map_err(|e| io("open identity directory", e))?;
        directory(&fd, true, true)?;
        Ok(fd)
    }
    fn lease(dir: &OwnedFd) -> Result<OwnedFd, IdentityError> {
        let fd = fs::openat(
            dir,
            "owner.lock",
            FILE | OFlags::RDWR | OFlags::CREATE,
            FileMode::from_raw_mode(0o600),
        )
        .map_err(|e| io("open lease", e))?;
        file(&fd)?;
        fs::flock(&fd, FlockOperation::NonBlockingLockExclusive).map_err(|e| {
            if e == rustix::io::Errno::WOULDBLOCK {
                IdentityError::Busy
            } else {
                io("lock identity", e)
            }
        })?;
        Ok(fd)
    }
    fn read(dir: &OwnedFd) -> Result<Option<Payload>, IdentityError> {
        let fd = match fs::openat(dir, FINAL, FILE | OFlags::RDONLY, FileMode::empty()) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(e) => return Err(io("open identity", e)),
        };
        file(&fd)?;
        let mut bytes = Vec::new();
        File::from(fd)
            .take((MAX_RECORD + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| io("read identity", e))?;
        decode(&bytes).map(Some)
    }
    fn publish(dir: &OwnedFd, payload: Payload, replace: bool) -> Result<(), IdentityError> {
        let bytes = encode(payload)?;
        let name = format!(".identity-{}.tmp", uuid::Uuid::new_v4());
        let fd = fs::openat(
            dir,
            name.as_str(),
            FILE | OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
            FileMode::from_raw_mode(0o600),
        )
        .map_err(|e| io("create staging file", e))?;
        let staged = (|| {
            file(&fd)?;
            let mut out = File::from(fd);
            out.write_all(&bytes).map_err(|e| io("write identity", e))?;
            out.sync_all().map_err(|e| io("sync identity", e))?;
            let flags = if replace {
                RenameFlags::empty()
            } else {
                RenameFlags::NOREPLACE
            };
            fs::renameat_with(dir, name.as_str(), dir, FINAL, flags)
                .map_err(|e| io("publish identity", e))
        })();
        if staged.is_err() {
            // Only this create-new leaf is ours. Never remove a final/foreign generation.
            let _ = fs::unlinkat(dir, name.as_str(), AtFlags::empty());
            return staged;
        }
        fs::fsync(dir).map_err(|_| IdentityError::DurabilityUnknown)
    }
    pub(super) fn prepare(
        root: &Path,
        namespace: &str,
        network: NetworkProfile,
        action: RetainedIdentityAction,
    ) -> Result<(RetainedIdentity, IdentityInfo), IdentityError> {
        validate_selection(root, namespace)?;
        let dir = private_directory(root, namespace)?;
        let lease = lease(&dir)?;
        let previous = read(&dir)?;
        // Even rotation refuses malformed current identity; it is not a recovery bypass.
        if let Some(payload) = &previous {
            validate(payload, namespace, network)?;
        }
        let (payload, disposition) = match (previous, action) {
            (Some(payload), RetainedIdentityAction::KeepOrCreate) => {
                (payload, IdentityDisposition::Restored)
            }
            (None, RetainedIdentityAction::KeepOrCreate) => {
                (fresh(namespace, network)?, IdentityDisposition::Created)
            }
            (
                Some(payload),
                RetainedIdentityAction::Rotate {
                    expected_generation,
                },
            ) if payload.generation == expected_generation => {
                (fresh(namespace, network)?, IdentityDisposition::Rotated)
            }
            _ => return Err(IdentityError::GenerationConflict),
        };
        let (tls, sigv4) = validate(&payload, namespace, network)?;
        let info = IdentityInfo::Retained {
            generation: payload.generation.clone(),
            certificate_sha256: sha256(tls.der()),
            disposition,
        };
        if disposition != IdentityDisposition::Restored {
            publish(&dir, payload, disposition == IdentityDisposition::Rotated)?;
        }
        Ok((
            RetainedIdentity {
                tls,
                sigv4,
                _lease: lease,
            },
            info,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(ip: &str) -> NetworkProfile {
        NetworkProfile::resolve(Some(ip.parse().unwrap()), Some(0), None, None).unwrap()
    }
    fn loopback() -> NetworkProfile {
        network("127.0.0.1")
    }

    #[test]
    fn private_identity_codec_and_native_pair_validation() {
        let payload = fresh("test", loopback()).unwrap();
        let generation = payload.generation.clone();
        let original = validate(&payload, "test", loopback()).unwrap();
        let restored = decode(&encode(payload).unwrap()).unwrap();
        let pair = validate(&restored, "test", loopback()).unwrap();
        assert_eq!(generation, restored.generation);
        assert_eq!(original.0.der(), pair.0.der());
        // Do not put even synthetic credentials in failure debug output.
        assert!(original.1.access_key_id == pair.1.access_key_id);
        assert!(original.1.secret_access_key == pair.1.secret_access_key);
    }

    #[test]
    fn private_identity_rejects_duplicate_unknown_corrupt_and_oversize_records() {
        let bytes = encode(fresh("test", loopback()).unwrap()).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        let duplicate = text.replacen("\"version\":1", "\"version\":1,\"version\":1", 1);
        assert!(decode(duplicate.as_bytes()).is_err());
        let unknown = text.replacen("\"version\":1", "\"version\":1,\"surprise\":true", 1);
        assert!(decode(unknown.as_bytes()).is_err());
        let changed = text.replacen("\"namespace\":\"test\"", "\"namespace\":\"other\"", 1);
        assert!(decode(changed.as_bytes()).is_err());
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        assert!(decode(&vec![b' '; MAX_RECORD + 1]).is_err());
        let mut payload = fresh("test", loopback()).unwrap();
        payload.version = 2;
        assert!(matches!(
            decode(&encode(payload).unwrap()),
            Err(IdentityError::UnknownFormat)
        ));
    }

    #[test]
    fn private_identity_rejects_wrong_namespace_key_names_and_root_class() {
        let mut payload = fresh("test", loopback()).unwrap();
        assert!(matches!(
            validate(&payload, "other", loopback()),
            Err(IdentityError::NamespaceMismatch)
        ));
        assert!(matches!(
            validate(&payload, "test", network("127.0.0.2")),
            Err(IdentityError::CertificateMismatch)
        ));
        // Updating the manifest is not enough: actual certificate SAN verification must fail.
        payload.names = required_names(network("127.0.0.2"));
        assert!(validate(&payload, "test", network("127.0.0.2")).is_err());
        payload.names = required_names(loopback());
        payload.private_pkcs8_der_base64 =
            fresh("test", loopback()).unwrap().private_pkcs8_der_base64;
        assert!(validate(&payload, "test", loopback()).is_err());
        let mut payload = fresh("test", loopback()).unwrap();
        payload.root_access_key_id = "AKIA0000000000000000".into();
        assert!(validate(&payload, "test", loopback()).is_err());
    }

    #[test]
    fn private_identity_selection_is_explicit_and_bounded() {
        for bad in ["", ".", "..", "a/b", "a\\b", "a b", "é"] {
            assert!(validate_selection(Path::new("/tmp"), bad).is_err());
        }
        assert!(validate_selection(Path::new("relative"), "test").is_err());
        assert!(validate_selection(Path::new("/tmp/../etc"), "test").is_err());
        assert!(validate_selection(Path::new("/tmp"), "a.b-c_d1").is_ok());
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use super::*;
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
        struct Fixture {
            root: PathBuf,
        }
        impl Fixture {
            fn new() -> Self {
                let root = std::env::temp_dir()
                    .join(format!("emulator-identity-test-{}", uuid::Uuid::new_v4()));
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&root)
                    .unwrap();
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(root.join("test"))
                    .unwrap();
                Self { root }
            }
            fn load(
                &self,
                action: RetainedIdentityAction,
            ) -> Result<(RetainedIdentity, IdentityInfo), IdentityError> {
                disk::prepare(&self.root, "test", loopback(), action)
            }
            fn record(&self) -> PathBuf {
                self.root.join("test/identity/boot-identity.v1")
            }
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.root);
            }
        }
        fn generation(info: &IdentityInfo) -> String {
            match info {
                IdentityInfo::Retained { generation, .. } => generation.clone(),
                _ => panic!("retained fixture"),
            }
        }

        const CHILD_TEST: &str = "boot_identity::tests::linux::private_identity_process_child";
        const CASE_ENV: &str = "EMULATOR_IDENTITY_TEST_CASE";
        const ROOT_ENV: &str = "EMULATOR_IDENTITY_TEST_ROOT";

        // Only a handle returned by our own successful spawn is signalled. No raw PID/group.
        struct ChildOwner(Option<std::process::Child>);
        impl ChildOwner {
            fn stop(&mut self) -> Result<(), &'static str> {
                let Some(child) = self.0.as_mut() else {
                    return Ok(());
                };
                if matches!(child.try_wait(), Ok(Some(_))) {
                    self.0 = None;
                    return Ok(());
                }
                // A process may exit between observation and kill; the reap decides completion.
                let _ = child.kill();
                let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            self.0 = None;
                            return Ok(());
                        }
                        Err(_) => return Err("child reap failed; fixture retained"),
                        Ok(None) if std::time::Instant::now() >= until => {
                            return Err("child termination deadline; fixture retained");
                        }
                        Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
                    }
                }
            }
        }
        impl Drop for ChildOwner {
            fn drop(&mut self) {
                let _ = self.stop();
            }
        }

        fn run_process_case(root: &Path, case: &str) -> Result<(), &'static str> {
            use std::os::unix::fs::OpenOptionsExt;
            let log = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(root.join(format!("{case}.log")))
                .map_err(|_| "create child log")?;
            let errors = log.try_clone().map_err(|_| "clone child log")?;
            let executable =
                std::env::current_exe().map_err(|_| "locate native test executable")?;
            let child = std::process::Command::new(executable)
                .args(["--ignored", "--exact", CHILD_TEST, "--test-threads=1"])
                .env_clear()
                .env("HOME", root)
                .env("TMPDIR", root)
                .env("PATH", "")
                .env(CASE_ENV, case)
                .env(ROOT_ENV, root)
                .current_dir(root)
                .stdin(std::process::Stdio::null())
                .stdout(log)
                .stderr(errors)
                .spawn()
                .map_err(|_| "spawn native test child")?;
            let mut owner = ChildOwner(Some(child));
            let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
            let outcome = loop {
                match owner.0.as_mut().unwrap().try_wait() {
                    Ok(Some(status)) => {
                        owner.0 = None;
                        break if status.success() {
                            Ok(())
                        } else {
                            Err("native child failed; private fixture log retained")
                        };
                    }
                    Err(_) => break Err("observe native child failed"),
                    Ok(None) if std::time::Instant::now() >= until => {
                        break Err("native child deadline");
                    }
                    Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
                }
            };
            owner.stop()?;
            outcome?;
            let marker = std::fs::read_to_string(root.join(format!("{case}.done")))
                .map_err(|_| "child did not execute the exact requested witness")?;
            if marker != format!("identity-case:{case}\n") {
                return Err("child witness marker mismatch");
            }
            Ok(())
        }

        #[test]
        fn private_identity_fresh_process_getters_restart_and_failure_boundaries() {
            // On any failure/unwind, retain this private fixture rather than delete beneath a
            // child whose cleanup could not be confirmed. Successful children are all reaped.
            let fixture = std::mem::ManuallyDrop::new(Fixture::new());
            for case in [
                "ephemeral",
                "create",
                "restore",
                "late-auth",
                "late-tls",
                "failure",
            ] {
                if let Err(reason) = run_process_case(&fixture.root, case) {
                    panic!(
                        "identity child {case}: {reason}; private fixture retained at {}",
                        fixture.root.display()
                    );
                }
            }
            // No process or inherited log handle remains after all six successful joins.
            std::fs::remove_dir_all(&fixture.root).expect("remove completed private fixture");
            drop(std::mem::ManuallyDrop::into_inner(fixture));
        }

        #[test]
        #[ignore = "only the fresh-process parent supplies this child's disposable inputs"]
        fn private_identity_process_child() {
            let case = std::env::var(CASE_ENV).expect("fixed identity child case required");
            let root =
                PathBuf::from(std::env::var_os(ROOT_ENV).expect("private fixture root required"));
            assert!(root.is_absolute());
            let config = || IdentityConfig::Retained {
                state_dir: root.clone(),
                namespace: "test".into(),
                network: loopback(),
                action: RetainedIdentityAction::KeepOrCreate,
            };
            assert!(SigV4Authenticator::minted_credentials().is_none());
            match case.as_str() {
                "ephemeral" => {
                    let auth = SigV4Authenticator::for_this_process();
                    assert!(std::ptr::eq(
                        auth,
                        SigV4Authenticator::minted_credentials().unwrap()
                    ));
                    let ca = TlsCertificateAuthority::covering(Some("127.0.0.2".parse().unwrap()))
                        .unwrap();
                    assert!(
                        ca.covers("127.0.0.2".parse().unwrap()),
                        "auth must not eagerly freeze TLS names"
                    );
                    assert!(std::ptr::eq(
                        ca,
                        TlsCertificateAuthority::for_this_process()
                    ));
                    assert!(!root.join("test/identity").exists());
                }
                "create" | "restore" => {
                    let info = prepare(config()).unwrap();
                    let expected = if case == "create" {
                        IdentityDisposition::Created
                    } else {
                        IdentityDisposition::Restored
                    };
                    let IdentityInfo::Retained {
                        generation,
                        certificate_sha256,
                        disposition,
                    } = info
                    else {
                        panic!("retained result required")
                    };
                    assert_eq!(disposition, expected);
                    let ca = TlsCertificateAuthority::for_this_process();
                    assert!(
                        SigV4Authenticator::minted_credentials().is_none(),
                        "TLS must not publish an unrequested authenticator"
                    );
                    let auth = SigV4Authenticator::for_this_process();
                    assert!(std::ptr::eq(
                        ca,
                        TlsCertificateAuthority::for_this_process()
                    ));
                    assert!(std::ptr::eq(auth, SigV4Authenticator::for_this_process()));
                    assert!(std::ptr::eq(
                        auth,
                        SigV4Authenticator::minted_credentials().unwrap()
                    ));
                    let bytes = std::fs::read(root.join("test/identity/boot-identity.v1")).unwrap();
                    let recorded = decode(&bytes).unwrap();
                    assert_eq!(generation, recorded.generation);
                    assert_eq!(base64::encode(ca.der()), recorded.certificate_der_base64);
                    assert!(auth.access_key_id == recorded.root_access_key_id);
                    assert!(auth.secret_access_key == recorded.root_secret_access_key);
                    let public =
                        format!("{}\n{}\n", generation, base64::encode(certificate_sha256));
                    let receipt = root.join("public-generation.txt");
                    if case == "create" {
                        use std::io::Write;
                        use std::os::unix::fs::OpenOptionsExt;
                        let mut out = std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .mode(0o600)
                            .open(receipt)
                            .unwrap();
                        out.write_all(public.as_bytes()).unwrap();
                    } else {
                        assert_eq!(std::fs::read_to_string(receipt).unwrap(), public);
                    }
                }
                "late-auth" | "late-tls" => {
                    let original_auth =
                        (case == "late-auth").then(SigV4Authenticator::for_this_process);
                    let original_tls =
                        (case == "late-tls").then(TlsCertificateAuthority::for_this_process);
                    let absent = root.join(&case);
                    assert!(!absent.exists());
                    assert_eq!(
                        prepare(IdentityConfig::Retained {
                            state_dir: absent.clone(),
                            namespace: "test".into(),
                            network: loopback(),
                            action: RetainedIdentityAction::KeepOrCreate,
                        }),
                        Err(IdentityError::AlreadyInitialized)
                    );
                    assert!(!absent.exists());
                    let auth = SigV4Authenticator::for_this_process();
                    let ca = TlsCertificateAuthority::for_this_process();
                    if let Some(original) = original_auth {
                        assert!(std::ptr::eq(original, auth));
                    }
                    if let Some(original) = original_tls {
                        assert!(std::ptr::eq(original, ca));
                    }
                    assert!(std::ptr::eq(auth, SigV4Authenticator::for_this_process()));
                    assert!(std::ptr::eq(
                        ca,
                        TlsCertificateAuthority::for_this_process()
                    ));
                }
                "failure" => {
                    // This is the disposable record made by the preceding child, not a host key.
                    let path = root.join("test/identity/boot-identity.v1");
                    std::fs::write(&path, b"deliberately invalid synthetic identity").unwrap();
                    assert_eq!(prepare(config()), Err(IdentityError::InvalidIdentity));
                    assert_eq!(
                        std::fs::read(path).unwrap(),
                        b"deliberately invalid synthetic identity"
                    );
                    assert!(
                        std::panic::catch_unwind(SigV4Authenticator::for_this_process).is_err()
                    );
                    assert!(
                        std::panic::catch_unwind(TlsCertificateAuthority::for_this_process)
                            .is_err()
                    );
                    assert!(SigV4Authenticator::minted_credentials().is_none());
                    assert_eq!(
                        prepare(IdentityConfig::Ephemeral),
                        Err(IdentityError::AlreadyInitialized)
                    );
                }
                _ => panic!("unknown identity child case"),
            }
            // A zero-test libtest invocation must not look like an executed witness.
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut marker = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(root.join(format!("{case}.done")))
                .unwrap();
            writeln!(marker, "identity-case:{case}").unwrap();
        }

        #[test]
        fn private_identity_disk_restart_lease_and_explicit_atomic_rotation() {
            let fixture = Fixture::new();
            let (first, info) = fixture.load(RetainedIdentityAction::KeepOrCreate).unwrap();
            assert!(matches!(
                fixture.load(RetainedIdentityAction::KeepOrCreate),
                Err(IdentityError::Busy)
            ));
            let cert = first.tls.der().clone();
            let root_id = first.sigv4.access_key_id.clone();
            let root_secret = first.sigv4.secret_access_key.clone();
            drop(first);
            let (restored, restored_info) =
                fixture.load(RetainedIdentityAction::KeepOrCreate).unwrap();
            assert_eq!(cert, *restored.tls.der());
            assert!(
                root_id == restored.sigv4.access_key_id
                    && root_secret == restored.sigv4.secret_access_key
            );
            assert_eq!(generation(&info), generation(&restored_info));
            drop(restored);
            assert!(matches!(
                fixture.load(RetainedIdentityAction::Rotate {
                    expected_generation: "stale".into()
                }),
                Err(IdentityError::GenerationConflict)
            ));
            let (rotated, rotated_info) = fixture
                .load(RetainedIdentityAction::Rotate {
                    expected_generation: generation(&info),
                })
                .unwrap();
            assert_ne!(generation(&info), generation(&rotated_info));
            assert_ne!(cert, *rotated.tls.der());
            assert!(
                root_id != rotated.sigv4.access_key_id
                    && root_secret != rotated.sigv4.secret_access_key
            );
            assert_eq!(
                std::fs::metadata(fixture.record())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o600
            );
        }

        #[test]
        fn private_identity_present_invalid_never_remints_or_adopts_unsafe_leaf() {
            let fixture = Fixture::new();
            drop(fixture.load(RetainedIdentityAction::KeepOrCreate).unwrap());
            let path = fixture.record();
            std::fs::write(&path, b"not an identity").unwrap();
            assert!(fixture.load(RetainedIdentityAction::KeepOrCreate).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"not an identity");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(matches!(
                fixture.load(RetainedIdentityAction::KeepOrCreate),
                Err(IdentityError::PermissionMismatch)
            ));
            std::fs::remove_file(&path).unwrap();
            let sentinel = fixture.root.join("sentinel");
            std::fs::write(&sentinel, b"untouched").unwrap();
            symlink(&sentinel, &path).unwrap();
            assert!(fixture.load(RetainedIdentityAction::KeepOrCreate).is_err());
            assert_eq!(std::fs::read(&sentinel).unwrap(), b"untouched");
        }

        #[test]
        fn private_identity_refuses_namespace_symlink_and_missing_unadmitted_root() {
            let fixture = Fixture::new();
            std::fs::rename(fixture.root.join("test"), fixture.root.join("other")).unwrap();
            symlink("other", fixture.root.join("test")).unwrap();
            assert!(fixture.load(RetainedIdentityAction::KeepOrCreate).is_err());
            assert!(!fixture.root.join("other/identity").exists());
            let absent = fixture.root.join("unadmitted");
            assert!(
                disk::prepare(
                    &absent,
                    "test",
                    loopback(),
                    RetainedIdentityAction::KeepOrCreate
                )
                .is_err()
            );
            assert!(!absent.exists());
        }
    }
}
